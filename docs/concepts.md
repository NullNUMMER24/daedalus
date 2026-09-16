# Concepts

The vocabulary Daedalus uses. Read this before the architecture document —
everything else assumes these words.

## The two-sentence summary

Daedalus stores **desired state** in Git, discovers **observed state** from your
hypervisors and clusters, and reconciles the difference. Every resource belongs
to exactly one **tenant**, and every action is authorised, quota-checked, and
recorded.

---

## Core nouns

### Tenant

The isolation boundary. A tenant is a person, a family member, a friend, or a
team who shares your hardware. A tenant owns resources, has a quota, has its own
encryption key, its own VLAN, and its own storage. Nothing crosses a tenant
boundary without an explicit, audited grant.

Every resource in Daedalus has exactly one owning tenant. There is no such thing
as an unowned resource — the platform's own infrastructure belongs to a reserved
`system` tenant.

### Environment

A subdivision *within* a tenant: `prod`, `staging`, `lab`. Environments are a
convenience for organising and for applying different policies (e.g. "production
changes require approval"). They are **not** a security boundary — a tenant
operator can see all of their tenant's environments. If you need a hard boundary,
use a second tenant.

### Principal

Anything that can authenticate: a **user** (via OIDC or a local account) or a
**service account** (via an API token). Principals are granted **roles** on
tenants. A principal may hold roles on several tenants and switches between them
with a context (`dae ctx use acme/prod`).

### Provider

An external system Daedalus drives: a Proxmox cluster, a libvirt host, a
Kubernetes API server. Providers hold credentials and are registered by the
platform admin. Crucially, a provider can be **scoped per tenant** — Daedalus
talks to Proxmox using the *tenant's own* pool-restricted API token, so the
hypervisor enforces the boundary independently of Daedalus's own checks.

### Resource

Anything Daedalus manages. Every resource has:

- an **apiVersion** and **kind** (`daedalus.io/v1alpha1`, `Machine`)
- a **name**, unique within `(tenant, environment, kind)`
- a **UID** — a ULID assigned on first creation, stable forever
- a **spec** — what you want, from Git
- a **status** — what is actually true, discovered from the provider
- **labels** and **annotations** for selection and metadata

This shape is deliberately Kubernetes-like. You already know how to read it, and
it gives you a free mental model for `get`/`describe`/`apply`.

---

## The resource kinds

| Kind | Purpose | Phase |
| --- | --- | --- |
| `Tenant` | An isolation boundary, its quota and its keys | 1 |
| `Environment` | A named subdivision within a tenant — a directory, not a manifest | 1 |
| `Provider` | Connection details for a hypervisor or cluster | 3 |
| `Image` | A bootable disk image or VM template | 3 |
| `MachineClass` | A named CPU/RAM/disk shape (`standard-4x8`) | 3 |
| `Machine` | One virtual machine | 3 |
| `Network` | An environment's network: provider, CIDR, gateway. The VLAN behind it is platform-allocated | 8 |
| `Subnet` | An L3 range with IPAM and DNS settings | 8 |
| `Volume` | A block device independent of a machine's lifecycle | 8 |
| `Cluster` | A Kubernetes cluster | 7 |
| `NodePool` | A group of identically-shaped cluster nodes | 7 |
| `Workload` | A Helm release or Kustomization on a cluster | 7 |
| `Secret` | An age/SOPS-encrypted value | 8 |
| `Quota` | Resource limits applied to a tenant | 5 |
| `Policy` | A Cedar authorisation policy | 5 |

The phase is when Daedalus starts *acting* on a kind. `Provider`, `Image`,
`MachineClass`, `Tenant`, `Network` and `Machine` manifests are already parsed
and validated — `dae validate` checks their fields, references and addresses.

`MachineClass` and `Image` are worth calling out: they live in a shared
**catalog** that the platform admin curates. Tenants reference them but cannot
edit them. This is how you offer "a medium VM running Debian 12" without every
tenant hand-rolling disk geometry.

---

## The three states

This is the single most important idea in Daedalus. Confusing these three is the
source of most bugs in infrastructure tools.

```
  DESIRED                RECORDED                 OBSERVED
  (Git)                  (Ariadne / database)     (the provider, live)

  "web-01 should         "I created web-01 as     "VMID 104 exists, is
   have 4 vCPU"           Proxmox VMID 104,        running, and has
                          from spec hash abc123"   2 vCPU"
```

- **Desired** — YAML in Git. The source of truth for *intent*. Only humans (and
  the web UI, via commits) write here.
- **Observed** — polled live from the provider. The source of truth for *reality*.
  Never assume it matches what you last applied.
- **Recorded** — Daedalus's database. The mapping between the two: which provider
  object corresponds to which Daedalus resource, and what spec was last applied.

Reconciliation is a pure function of all three:

```
plan = reconcile(desired, observed, recorded)
```

### Recorded state is a cache, not a treasure

Terraform's great weakness is that losing `terraform.tfstate` is a disaster.
Daedalus avoids this: **every provider object Daedalus creates is tagged** with

```
daedalus.io/uid       = 01J8ZQ4M...       (the resource ULID)
daedalus.io/tenant    = acme
daedalus.io/kind      = Machine
daedalus.io/generation = 7
```

on whatever the provider offers — Proxmox tags and description, Kubernetes
labels, libvirt metadata. Which means recorded state can always be **rebuilt by
scanning the providers** (`dae admin reindex`). Lose the database and you lose
history, not control.

This has a second benefit: adoption. Tag an existing VM by hand and Daedalus will
pick it up on the next scan instead of trying to create a duplicate.

---

## The Labyrinth — the resource graph

Resources depend on each other. A `Machine` needs its `Network` to exist first; a
`Cluster` needs its control-plane `Machine`s; a `Workload` needs its `Cluster`.

Daedalus builds a directed acyclic graph of these dependencies, from:

- **explicit** edges — a `dependsOn: [Network/acme-prod]` field
- **implicit** edges — any field that references another resource by name

Then it walks the graph in topological order, applying independent resources in
parallel within each wave. Deletion walks it in reverse. Cycles are rejected at
validation time, before anything is touched.

```
        Network/acme-prod
         ├── Machine/cp-01 ──┐
         ├── Machine/cp-02 ──┼── Cluster/prod ── Workload/ingress
         └── Machine/wk-01 ──┘
```

## Ariadne — the state thread

An append-only event log. Every plan, apply, drift detection, error, and status
change is an event, tagged with tenant, principal, resource UID, and a
correlation ID that ties a whole apply run together.

The current state of a resource is a fold over its events. This gives you three
things for the price of one:

1. **Audit** — "who changed web-01's memory, and when?"
2. **Rollback** — "restore the state as of commit `abc123`", because you have
   every intermediate spec.
3. **Debugging** — a complete, ordered narrative of why a resource is the way it is.

The thread is what lets you find your way back out of the Labyrinth.

## Icarus — the host agent

An optional, small agent on hosts that have no good remote API — bare metal you
want to onboard, or a machine whose metrics you want. It dials **outbound** to
`daedalusd` over mTLS, so no inbound firewall holes are needed.

The name is a deliberate warning. Icarus falls. The server therefore treats agent
input as **untrusted claims**, verifies anything actionable against another
source, and is designed to keep working when agents disappear. Agents report;
they do not command.

Not needed until Phase 8. Proxmox and Kubernetes both have APIs good enough that
Daedalus is agentless for VMs and clusters.

---

## Verbs

| Verb | Meaning |
| --- | --- |
| **validate** | Parse and check manifests. No network access. |
| **plan** | Compute the difference between desired and observed. Read-only. |
| **apply** | Execute a plan. The only thing that mutates. |
| **observe** | Poll a provider and refresh status. |
| **drift** | Report resources whose observed state has diverged from desired. |
| **adopt** | Take ownership of an existing, untagged provider object. |
| **prune** | Delete provider objects tagged as Daedalus-managed but absent from Git. |

`plan` and `apply` are separate on purpose. `apply` without a fresh plan is
refused unless you pass `--auto-approve`.
