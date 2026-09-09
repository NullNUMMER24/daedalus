# Multi-tenancy

You chose **hard isolation**: real people sharing your hardware. That raises the
bar considerably — the interesting question stops being "can the UI filter by
tenant" and becomes "if I have a bug, can tenant A still not touch tenant B?"

The answer has to be yes. So isolation is enforced at **seven independent
layers**, each of which would be sufficient on a good day, on the assumption that
some day will not be good.

---

## The seven layers

```
 1. Identity        who is this?
 2. Authorisation   may they do this?          ← Cedar policies
 3. Data access     can the query even see it? ← type system
 4. Quota           can they afford it?
 5. Secrets         can they decrypt it?       ← per-tenant keys
 6. Infrastructure  can the hypervisor see it? ← scoped credentials  ★
 7. Audit           what did they actually do?
```

Layer 6 is the one that matters most and the one most homegrown tools skip. Get
it right and a total compromise of Daedalus's own logic still cannot cross a
tenant boundary.

---

## Layer 1 — Identity

A **Principal** is one of:

- **User** — authenticated via OIDC against your existing IdP (Authentik,
  Keycloak, Pocket ID, Zitadel). Local accounts with Argon2id exist as a
  bootstrap and break-glass fallback, disabled by default once OIDC is configured.
- **Service account** — belongs to exactly one tenant, authenticates with a
  bearer token. Tokens are stored as SHA-256 hashes, are prefixed for
  identification (`dae_sa_...`), carry an expiry, and are individually revocable.

Principals are not tenant members by nature; they hold **role bindings**.

### Roles

| Role | Scope | Can |
| --- | --- | --- |
| `platform-admin` | Global | Everything, including creating tenants and registering providers |
| `platform-viewer` | Global | Read all tenants; change nothing |
| `tenant-owner` | One tenant | Everything within the tenant, incl. managing its members and repo |
| `tenant-operator` | One tenant | Plan, apply, manage resources; not members or quotas |
| `tenant-viewer` | One tenant | Read-only |
| `tenant-deployer` | One tenant + env | Apply only in a named environment (for CI) |

A principal may hold bindings on several tenants and selects one per request via
the context header (`X-Daedalus-Tenant`) or `dae ctx use`.

**Cross-tenant reads are never implicit.** A `platform-admin` acting across
tenants must pass `--all-tenants` explicitly, and every such read is audited
distinctly.

---

## Layer 2 — Authorisation

[Cedar](https://www.cedarpolicy.com/) — AWS's policy language, with a native Rust
crate. Chosen over hand-rolled `if` statements because policy becomes *data*: it
is versionable, testable, and analysable, and Cedar can prove properties about
policy sets.

Every request produces an explicit triple:

```
principal: DaedalusUser::"01J8Z..."
action:    Action::"machine:create"
resource:  Machine::"acme/prod/web-01"
context:   { environment: "prod", mfa: true, source_ip: "..." }
```

Policies live in Git under `platform/policies/` and are themselves a managed
resource:

```cedar
// Tenant operators may act on their own tenant's resources.
permit (
    principal,
    action in [Action::"machine:create", Action::"machine:update",
               Action::"machine:delete", Action::"plan", Action::"apply"],
    resource
)
when {
    resource.tenant == principal.tenant &&
    principal.role in ["tenant-owner", "tenant-operator"]
};

// Production changes require an approved plan. No exceptions for owners.
forbid (principal, action == Action::"apply", resource)
when  { context.environment == "prod" && !context.plan_approved };

// Nobody may delete a resource carrying the protected annotation.
forbid (principal, action == Action::"machine:delete", resource)
when  { resource.annotations has "daedalus.io/protected" };
```

`forbid` beats `permit` in Cedar, unconditionally. That makes guardrails
genuinely reliable rather than a matter of rule ordering.

**Test your policies.** `dae policy test` runs a fixture file of
`(principal, action, resource) → expected` cases. Policy without tests is
decoration.

---

## Layer 3 — Data access

Authorisation checks can be forgotten. The type system cannot be.

The store layer exposes **no method that takes a bare tenant ID**, and no method
that omits one. Instead, an authenticated request produces a `TenantScope` that
can only be constructed by the auth middleware:

```rust
/// Proof that the current principal is authorised to act within this tenant.
/// Constructible only by the auth layer — the private field enforces this
/// across module boundaries.
pub struct TenantScope {
    tenant_id: TenantId,
    _seal: PhantomData<()>,   // no literal construction outside this module
}

impl ResourceRepo {
    // Every read and write demands the proof.
    pub async fn list(&self, scope: &TenantScope, kind: Kind)
        -> Result<Vec<Resource>>;

    pub async fn get(&self, scope: &TenantScope, uid: Uid)
        -> Result<Option<Resource>>;

    pub async fn upsert(&self, scope: &TenantScope, r: &Resource)
        -> Result<()>;
}
```

Every SQL statement then carries `WHERE tenant_id = ?` because there is no way to
write one that does not. Forgetting tenant scoping becomes a **compile error**
rather than a data breach.

The handful of genuinely cross-tenant operations live behind a separate
`AdminScope`, constructible only for `platform-admin`, and every one of its
methods logs at `warn` level with the principal attached.

> **Rust learning note:** this is the *typestate* pattern, and it is one of the
> best arguments for writing this project in Rust at all. Encode the invariant in
> a type and the compiler enforces it on every future line of code you write.

---

## Layer 4 — Quota

Per-tenant limits, checked against the **post-apply footprint** rather than the
delta — so a plan that creates two VMs and deletes one is evaluated on the net
result, and a partially-applied plan cannot leave you over budget.

```yaml
apiVersion: daedalus.io/v1alpha1
kind: Quota
metadata:
  tenant: acme
spec:
  limits:
    machines:          20
    vcpu:              48
    memory:            128Gi
    storage:           4Ti
    clusters:          3
    cluster_nodes:     12
    networks:          4
    ipv4_addresses:    64
    snapshots:         50
  # Overcommit ratios: the lab tolerates more than the sum of parts.
  overcommit:
    vcpu:   4.0     # 48 vCPU of quota may back onto 12 physical cores
    memory: 1.0     # never overcommit RAM; ballooning is a trap
```

Quota is evaluated at plan time (fail early, with a readable message) *and* again
at apply time inside the tenant's serialised apply lock (so two concurrent plans
cannot both pass and jointly exceed).

Usage is computed from **recorded state**, reconciled against **observed state**
on every drift run — otherwise a tenant could create a VM out-of-band on the
hypervisor and quietly exceed their limit.

---

## Layer 5 — Secrets

Each tenant gets its own [age](https://age-encryption.org/) key pair at creation.

- The **public** key lives in the tenant's Git repo; anyone on the tenant can
  encrypt.
- The **private** key is held by `daedalusd`, itself encrypted with a server
  master key that comes from the environment, a file with `0600`, or an external
  KMS (Vault/OpenBao) — never from the database.
- Decryption happens **in memory**, **only** while executing an action for that
  tenant, and the plaintext is wrapped in a type that zeroises on drop
  (`secrecy` + `zeroize` crates) and whose `Debug` and `Display` impls print
  `[REDACTED]`.

There is no master key that decrypts everything. Compromising tenant A's key
yields tenant A's secrets and nothing else.

Secrets in Git are SOPS-compatible, so `sops` and existing editor integrations
work:

```yaml
apiVersion: daedalus.io/v1alpha1
kind: Secret
metadata:
  name: web-01-userdata
  tenant: acme
spec:
  data:
    password: ENC[AES256_GCM,data:xY9...,iv:...,tag:...,type:str]
sops:
  age:
    - recipient: age1acme7k2...
```

Secrets are **never** written to the database in plaintext, never appear in plan
output (they show as `(secret: web-01-userdata @ v3)`), and never reach logs.

---

## Layer 6 — Infrastructure ★

The backstop. If every layer above fails, this one still holds, because it is not
enforced by Daedalus at all — it is enforced by the hypervisor.

### Proxmox

Proxmox has a real, mature authorisation model, and Daedalus should ride on it
rather than reinvent it. Per tenant, provisioned automatically at tenant creation:

| Proxmox object | Purpose |
| --- | --- |
| **Pool** `tenant-acme` | Every VM the tenant owns lives in this pool |
| **User** `daedalus-acme@pve` | The identity Daedalus uses *for this tenant* |
| **API token** | The credential, `privsep=1` |
| **ACL** `/pool/tenant-acme → PVEVMAdmin` | Scoped permission, and nothing broader |
| **Storage ACL** `/storage/tenant-acme-*` | Only their storage |
| **SDN VNet** `acme-prod` (VLAN tag) | Only their L2 segment |

Daedalus then makes provider calls **as the tenant's token**, not as root. A bug
in Daedalus's own tenant filtering results in a `403` from Proxmox, not a
cross-tenant write. That property is worth a great deal, and it is essentially
free.

The platform-admin credential (`daedalus-platform@pve`, which *can* create pools
and users) is used only for tenant lifecycle operations, never for routine
resource management.

### Network

- **VLAN per tenant**, allocated from a configured range, realised via Proxmox
  SDN zones or manually-defined bridges.
- Firewall rules default to **deny inter-VLAN**; a tenant reaches the internet and
  their own services, nothing else.
- Shared services (DNS, NTP, package mirrors, the Daedalus API itself) live on a
  services VLAN reachable by explicit allow rules.
- Cross-tenant connectivity requires an explicit, audited `NetworkPeering`
  resource that both tenant owners must accept.

### Storage

- Dedicated ZFS dataset or LVM thin pool per tenant, with a hard quota set on the
  storage layer as well as in Daedalus.
- Backups land in per-tenant PBS namespaces with separate retention.

### Kubernetes

Two isolation grades, chosen per tenant:

| Grade | Mechanism | Use when |
| --- | --- | --- |
| **Hard** (default here) | A dedicated cluster on the tenant's own VMs | Real people; untrusted workloads |
| **Soft** | A shared cluster, namespace per tenant, plus `ResourceQuota`, `NetworkPolicy` default-deny, a restricted Pod Security Standard, and a separate node pool via taints | Your own environments; trusted, cost-sensitive |

Given hard isolation, the default is a cluster per tenant. Cheap in a homelab:
a single-node k3s VM is a perfectly good cluster.

---

## Layer 7 — Audit

Everything mutating, every denial, and every cross-tenant read goes into Ariadne:

```json
{
  "ts": "2026-09-08T14:22:31Z",
  "correlation_id": "01J8ZQ4M7X...",
  "principal": { "kind": "user", "id": "01J8...", "name": "cj" },
  "tenant": "acme",
  "action": "machine:update",
  "resource": "Machine/acme/prod/web-01",
  "decision": "allow",
  "policy_id": "tenant-operator-write",
  "changes": { "spec.memory": { "from": "8Gi", "to": "16Gi" } },
  "source": { "interface": "web", "ip": "10.20.0.5" },
  "git_commit": "abc123def"
}
```

Denials are logged as loudly as successes — a burst of denials is the signal that
something is wrong, and it is exactly the signal most systems throw away.

Tenants can read their own audit log (`dae audit`). Only `platform-admin` sees
all of them. The log is append-only; there is no delete endpoint at any privilege
level, and retention is enforced by a compaction job that snapshots rather than
erases.

---

## Tenant lifecycle

Creating a tenant is a real provisioning operation, not a database insert.

```
dae tenant create acme --display "Acme Ltd" --quota-template medium
```

1. Allocate a tenant ID (ULID) and validate the name is DNS-safe and unused.
2. Generate an age key pair; store the private half in the sealed secret store.
3. Allocate a VLAN ID from the configured range.
4. **Proxmox**: create pool, storage dataset, SDN VNet, user, API token, ACLs.
5. Create the Git repository (or the `tenants/acme/` subtree) and seed it with a
   skeleton, a README, and the tenant's age public key.
6. Write the default `Quota` from the named template.
7. Create the `tenant-owner` role binding for the requesting principal.
8. Emit `TenantCreated` to Ariadne.

Every step is idempotent and the whole thing is resumable — partial tenant
creation is a state the system must handle, because step 4 *will* fail sometimes.

### States

```
Provisioning ──► Active ──► Suspended ──► Deleting ──► Deleted
                    ▲           │
                    └───────────┘
```

- **Suspended** — VMs stopped, API access denied, data retained. For "you stopped
  paying" or "you did something alarming".
- **Deleting** — a two-phase process with a mandatory grace period. Resources are
  destroyed in reverse topological order; the Git repo is archived, not deleted;
  the audit log is retained per policy; backups follow their own retention.
  Requires `platform-admin` plus an explicit `--confirm <tenant-name>` echo.

---

## Open questions

Decide these before Phase 5.

1. **VLAN exhaustion.** 4094 VLANs is plenty for a homelab, but if you nest or
   share, consider VXLAN via Proxmox SDN from the start.
2. **Shared image catalog.** Tenants read platform images. Should a tenant be
   able to publish an image to other tenants? (Recommendation: no, not in v1 —
   image sharing is a supply-chain path between tenants.)
3. **Resource lending.** A tenant temporarily exceeding quota with owner approval
   is convenient and considerably complicates quota accounting. Defer.
4. **Self-service signup.** If tenants can create themselves, tenant creation
   becomes an unauthenticated attack surface. Recommendation: invite-only.
