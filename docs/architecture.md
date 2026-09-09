# Daedalus Architecture

Assumes you have read [Concepts](concepts.md).

## System overview

```
        ┌──────────┐    ┌──────────────┐   ┌────────────┐
        │   dae    │    │   Browser    │   │  CI / bots │
        │   CLI    │    │  (htmx UI)   │   │  (tokens)  │
        └────┬─────┘    └──────┬───────┘   └─────┬──────┘
             │  HTTPS + token  │                 │
             └────────┬────────┴─────────────────┘
                      ▼
        ╔═════════════════════════════════════════════════╗
        ║                  daedalusd                      ║
        ║                                                 ║
        ║  ┌───────────────────────────────────────────┐  ║
        ║  │ API layer  (axum)                         │  ║
        ║  │  authn → tenant resolve → authz → quota   │  ║
        ║  └────────────────────┬──────────────────────┘  ║
        ║                       ▼                         ║
        ║  ┌──────────┐  ┌─────────────┐  ┌────────────┐  ║
        ║  │ Git sync │→ │   Engine    │→ │  Providers │  ║
        ║  │ (desired)│  │  Labyrinth  │  │  (drivers) │  ║
        ║  └──────────┘  │  + planner  │  └─────┬──────┘  ║
        ║                └──────┬──────┘        │         ║
        ║                       ▼               │         ║
        ║              ┌─────────────────┐      │         ║
        ║              │ Ariadne (store) │      │         ║
        ║              │ SQLite/Postgres │      │         ║
        ║              └─────────────────┘      │         ║
        ╚═══════════════════════════════════════╪═════════╝
                                                ▼
              ┌────────────┬──────────────┬─────────────┐
              │ Proxmox VE │ Kubernetes   │ libvirt /   │
              │ (REST)     │ (kube API)   │ KubeVirt    │
              └────────────┴──────────────┴─────────────┘
```

Everything mutating goes through the API layer. The CLI is a thin HTTP client;
the web UI is server-rendered inside the same process. There is deliberately **no
path that reaches a provider without passing authn, authz, and quota checks** —
that is what makes the multi-tenancy claim credible.

---

## Components

### `daedalusd` — the server

A single binary containing:

| Subsystem | Responsibility |
| --- | --- |
| **API** | HTTP surface, OpenAPI schema, auth middleware |
| **Web** | Server-rendered htmx dashboard, same process, same auth |
| **Git sync** | Clone/fetch tenant repos, parse manifests, detect changes, write back |
| **Engine** | Build the Labyrinth, compute plans, execute applies |
| **Providers** | Drivers implementing the `Provider` trait |
| **Ariadne** | Persistence: resources, events, audit, leases |
| **Policy** | Cedar authorisation + quota enforcement |
| **Scheduler** | Periodic loops: git poll, drift detect, health probe, GC |

Runs as a single process on one box. Phase 9 adds an optional second instance
with leader election for availability — but a homelab control plane being briefly
down is not an outage of the homelab, only of its management. Design for
*restartability*, not for five nines.

### `dae` — the CLI

A thin client over the HTTP API, plus a `--local` mode that runs `validate` and
an offline `plan` without a server (useful in CI and pre-commit hooks). It never
talks to a provider directly — that would duplicate the enforcement logic and
create exactly the back door multi-tenancy cannot tolerate.

### Web UI

Axum handlers rendering [Maud](https://maud.lambda.xyz/) templates, with htmx for
partial updates and server-sent events for live plan/apply logs. No npm, no
bundler, no build step. The UI is a *client of the same API* conceptually, but
renders in-process to avoid a second round trip.

---

## The reconciliation pipeline

This is the heart of the system. Every change follows the same path.

```
 1. LOAD       Git ──────► manifests (raw YAML)
 2. PARSE      manifests ─► typed resources          [schema errors here]
 3. RESOLVE    references, catalog lookups, defaults [dangling refs here]
 4. GRAPH      build Labyrinth, topo-sort            [cycles here]
 5. OBSERVE    poll providers for current reality
 6. DIFF       desired vs observed vs recorded ─► actions
 7. POLICY     Cedar check every action              [authz denials here]
 8. QUOTA      sum the post-apply footprint          [quota denials here]
 9. PLAN       an ordered, human-readable list of actions
──────────────────────── approval gate ────────────────────────
10. APPLY      execute wave by wave, with retries
11. RECORD     append events to Ariadne, update recorded state
12. VERIFY     re-observe and confirm convergence
```

Steps 1–9 are **read-only and side-effect free**. That property is worth
defending aggressively: it means `plan` is always safe to run, it can run on
every commit in CI, and it makes the whole pipeline testable without any
infrastructure.

### Actions

The diff produces typed actions, not free-form scripts:

```rust
enum Action {
    Create  { uid: Uid, spec: Spec },
    Update  { uid: Uid, from: Spec, to: Spec, strategy: UpdateStrategy },
    Delete  { uid: Uid },
    Adopt   { uid: Uid, provider_ref: ProviderRef },
    NoOp    { uid: Uid },
}

enum UpdateStrategy {
    InPlace,            // hot-resize memory, add a disk
    RequiresRestart,    // change vCPU count on some hypervisors
    RequiresRecreate,   // change the boot image — destructive
}
```

`RequiresRecreate` is surfaced loudly in the plan and always needs explicit
confirmation, even with `--auto-approve` unless the resource is annotated
`daedalus.io/allow-recreate: "true"`. Accidentally rebuilding a VM because you
changed a field you thought was mutable is the classic infrastructure-tool
footgun, and it is worth designing against explicitly.

### Waves and concurrency

Actions are grouped into waves by topological depth. Within a wave, actions run
concurrently up to a per-provider concurrency limit (Proxmox does not enjoy
twenty simultaneous clone operations). Across waves, execution is strictly
ordered.

Locking:

- **Per-resource lease** — one apply may touch a given resource at a time.
- **Per-tenant serialisation** — a tenant runs one apply at a time, so their
  quota arithmetic cannot race.
- **Cross-tenant parallelism** — different tenants apply concurrently. Blast
  radius stays inside a tenant.

Leases live in the database with an expiry, so a crashed `daedalusd` releases
them on restart rather than deadlocking forever.

### Failure handling

An action can fail halfway. Daedalus does **not** attempt transactional rollback
of infrastructure — that is a lie most tools tell. Instead:

- The failed action is recorded with its error and marked `Degraded`.
- Dependent waves are skipped (their prerequisites are not met).
- Independent branches of the graph continue.
- The next reconcile retries with exponential backoff and jitter.
- After N failures the resource enters `Stalled` and stops retrying until a
  human intervenes or the spec changes.

Convergence over time, not atomicity. That is the correct model for
infrastructure, and it is why the pipeline is idempotent by construction.

---

## Control loops

`daedalusd` runs several independent loops, each with its own interval and its
own tracing span:

| Loop | Default interval | What it does |
| --- | --- | --- |
| **Git poll** | 60s | Fetch tenant repos, compare HEAD, enqueue reconcile on change |
| **Drift detect** | 5m | Observe providers, compare to desired, flag divergence |
| **Reconcile** | on demand | Drain the work queue, run the pipeline |
| **Health probe** | 30s | Check provider reachability, cluster node readiness |
| **Lease GC** | 30s | Expire abandoned leases |
| **Event compaction** | daily | Fold old Ariadne events into snapshots |

Drift is reported but **not auto-corrected by default**. A homelab operator
poking at a VM by hand and having Daedalus silently revert it at 3am is hostile
behaviour. Auto-heal is opt-in per resource:

```yaml
metadata:
  annotations:
    daedalus.io/drift-policy: report | correct | ignore
```

---

## Data model sketch

```sql
tenants        (id, name, state, created_at, ...)
principals     (id, kind, subject, display_name, ...)
role_bindings  (principal_id, tenant_id, role)
resources      (uid, tenant_id, environment, kind, name, generation,
                spec_json, status_json, desired_hash, applied_hash,
                provider_ref, phase, UNIQUE(tenant_id, environment, kind, name))
events         (id, ts, tenant_id, resource_uid, principal_id, correlation_id,
                kind, payload_json)                        -- Ariadne, append-only
providers      (id, tenant_id NULLABLE, kind, endpoint, credential_ref, ...)
quotas         (tenant_id, dimension, limit)
leases         (resource_uid, holder, expires_at)
```

Every table that holds tenant data carries `tenant_id`, and the store layer
exposes **no method that omits it** — see [Multi-tenancy](multitenancy.md#layer-3--data-access)
for how that is enforced in the type system rather than by discipline.

`resources.provider_ref` is the mapping to the provider's own identifier
(`pve://cluster-main/node-01/qemu/104`). `desired_hash` vs `applied_hash` gives a
cheap "is this resource up to date?" check without a full diff.

---

## Security architecture

Layered, on the assumption that any single layer will eventually have a bug:

1. **Transport** — TLS everywhere; mTLS for Icarus agents.
2. **Authentication** — OIDC (Authentik/Keycloak/Pocket ID) for humans; hashed
   API tokens for machines; Argon2id for the local-account fallback.
3. **Authorisation** — Cedar policies evaluated on every request, over an
   explicit `(principal, action, resource)` triple.
4. **Data scoping** — tenant-scoped repository types; unscoped queries do not
   compile.
5. **Quota** — checked against the *post-apply* footprint, not the delta.
6. **Provider credential scoping** — Daedalus uses per-tenant, pool-restricted
   provider tokens, so the hypervisor is the final backstop.
7. **Secrets** — age-encrypted at rest in Git; per-tenant keys; decrypted only in
   memory, only while acting for that tenant; never logged, never in the database.
8. **Audit** — every mutation, every denial, every plan, in Ariadne with a
   correlation ID.

### Threat model, briefly

| Threat | Mitigation |
| --- | --- |
| Tenant reads another's VM list | Layers 3 + 4; scoped repository types |
| Tenant creates a VM on another's network | Reference resolution is tenant-scoped; provider token cannot see the other VLAN |
| Compromised Daedalus process | Layer 6 limits blast radius per tenant; secrets are per-tenant keys, not one master key |
| Tenant exhausts the host | Layer 5 quota, plus hypervisor-level pool limits |
| Malicious manifest in Git | Manifests are data, never executed; templating is sandboxed; no shell interpolation |
| Stolen API token | Short-lived, scoped, revocable; all use audited |
| Rogue Icarus agent | Agent claims are untrusted; it reports, it does not command |

---

## What Daedalus deliberately does not do

Scope discipline matters more than features here.

- **It does not replace Flux or Argo CD.** Daedalus provisions the cluster and
  bootstraps a GitOps agent into it. Application delivery is then Flux's job.
  A thin `Workload` kind wrapping Helm exists for people who do not want Flux,
  but reimplementing Argo is out of scope.
- **It does not replace Proxmox Backup Server.** It orchestrates backup jobs; it
  does not move backup bytes.
- **It does not do monitoring.** It exposes Prometheus metrics and gets out of
  the way.
- **It is not a hypervisor.** It drives them.
- **It does not manage bare-metal provisioning** until Phase 8+, and even then via
  existing tooling (PXE/Talos), not a homegrown imaging stack.
