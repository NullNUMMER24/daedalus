# Architecture decisions

Numbered, with the reasoning preserved. When you disagree with one in six months,
you will at least know what you were thinking.

---

### D-001 — Daedalus is a standalone control plane, not a Kubernetes operator

**Decision.** `daedalusd` is a plain binary with its own database. It does not
require Kubernetes to run.

**Why.** The alternative — CRDs plus a controller, in the style of Crossplane or
Cluster API — requires a management cluster to exist before you can manage
anything. In a homelab that is circular: the cluster you would bootstrap from is
the cluster you want the tool to create. A standalone binary can be the *first*
thing installed on a fresh lab.

Secondary reason: writing your own reconciler teaches far more Rust than gluing
together `kube-rs` CRD types.

**Cost.** You reimplement things Kubernetes gives free: leader election, watches,
RBAC, the resource model. Mitigated by copying its *shapes* (see D-003) without
its runtime.

---

### D-002 — Git is the source of truth; the database is a cache

**Decision.** Desired state lives only in Git. The database holds recorded state,
events, and audit.

**Why.** Version history, review, rollback, and offline editing for free. And it
makes the web UI honest — the UI writes commits, so there is no divergence
between "what the UI did" and "what the repo says". A UI that can apply changes
Git does not know about destroys the entire value proposition.

**Cost.** UI mutations need format-preserving YAML round-tripping, which is
fiddly. Worth it. See [gitops.md](gitops.md#round-tripping-yaml-without-destroying-it).

---

### D-003 — Manifests use the Kubernetes resource shape

**Decision.** `apiVersion` / `kind` / `metadata` / `spec` / `status`.

**Why.** It is a good design, it is familiar to anyone who would want this tool,
and it makes `get`/`describe`/`apply` mean something obvious. Free UX from
existing knowledge.

**Cost.** Some verbosity. Accepted.

---

### D-004 — Provider objects are tagged; recorded state is rebuildable

**Decision.** Every object Daedalus creates carries `daedalus.io/uid`,
`daedalus.io/tenant`, and `daedalus.io/generation` in whatever metadata the
provider offers. Recorded state can always be reconstructed by scanning.

**Why.** Terraform's worst failure mode is a lost or corrupted state file. Here,
losing the database costs you history, not control — `dae admin reindex`
reconstructs the mapping. It also makes adoption trivial: tag an existing VM by
hand and Daedalus picks it up instead of creating a duplicate.

**Cost.** Every provider must offer some tagging mechanism. All the realistic
candidates do.

---

### D-005 — Plan and apply are separate, and drift is reported not corrected

**Decision.** `plan` is read-only and side-effect free. `apply` needs a fresh
plan. Detected drift is reported; auto-correction is opt-in per resource.

**Why.** Plans can then run safely on every commit in CI. And silently reverting
a manual change at 3am is hostile: in a homelab you *will* poke at a VM by hand,
and the tool should tell you about it rather than fight you.

**Cost.** Two steps instead of one. `--auto-approve` exists for automation, and
still refuses destructive recreates.

---

### D-006 — Tenant isolation is enforced at seven layers, with the hypervisor as backstop

**Decision.** See [multitenancy.md](multitenancy.md). The load-bearing layer is
per-tenant, pool-scoped provider credentials.

**Why.** With real people sharing hardware, "the application filters by tenant_id"
is not good enough — one missing `WHERE` clause becomes a breach. Using Proxmox's
own pools, users, tokens and ACLs means a total compromise of Daedalus's logic
still returns `403` from the hypervisor.

**Cost.** Tenant creation becomes a real provisioning workflow with many failure
modes, rather than a database insert. This is the right cost to pay.

---

### D-007 — Tenant scoping is enforced by the type system

**Decision.** Store methods take a `TenantScope` that only the auth middleware
can construct. There is no method that omits it.

**Why.** Discipline fails; compilers do not. This converts the most likely
multi-tenancy bug class into a compile error.

**Cost.** More parameter passing. This is also the single best Rust lesson in the
project.

---

### D-008 — The web UI is server-rendered with htmx, not a SPA

**Decision.** Axum + Maud + htmx, compiled into the binary via `rust-embed`. No
npm, no bundler.

**Why.** The UI is tables, forms, and status badges. A SPA means a second
language, a second toolchain, a second dependency tree, and CORS. Server-side
rendering with htmx gets a genuinely good dashboard for a fraction of the effort,
and `cargo build` produces one deployable file.

**Cost.** Highly interactive views (a drag-and-drop topology editor) would be
awkward. If one is ever needed, Leptos can be added for that page alone.

---

### D-009 — Proxmox first; KubeVirt is provider three

**Decision.** Phase 3 targets Proxmox VE. The `MachineProvider` trait keeps
libvirt, KubeVirt and Harvester viable later.

**Why.** Proxmox is a plain REST API (no FFI while learning Rust), its ACL model
is the tenancy backstop from D-006, and template cloning makes the develop-test
loop fast. KubeVirt would require solving bare-metal Kubernetes provisioning
*before* the first VM — all the risk, front-loaded. Full analysis in
[providers.md](providers.md#where-kubevirt-fits).

**Cost.** Coupling risk. Mitigated by writing libvirt as provider two
specifically to prove the abstraction is real.

---

### D-010 — Application delivery is delegated to Flux

**Decision.** Daedalus provisions clusters and bootstraps a GitOps agent into
them. It does not reimplement Argo CD.

**Why.** Scope discipline. Cluster lifecycle is the unsolved problem in a
homelab; app delivery is thoroughly solved. A thin `Workload` kind wrapping Helm
exists for people who do not want Flux, and stays thin.

**Cost.** Two GitOps systems in the stack. Acceptable — they operate at different
layers, and the boundary is clean.

---

### D-011 — SQLite by default, Postgres optional

**Decision.** SQLite in WAL mode is the default. Postgres behind a feature flag.

**Why.** One file, no daemon, trivially backed up, entirely sufficient for a
homelab's write volume. Zero operational burden for the common case.

**Cost.** No multi-writer, which constrains HA in Phase 9. The repository trait
is written against both from the start so it is a config change, not a rewrite.

---

### D-012 — `dae-core` has no I/O dependencies

**Decision.** The domain crate depends on no async runtime, database, or HTTP
client. Enforced by a CI check.

**Why.** The interesting logic — diffing, merging, graph ordering — becomes
testable in microseconds with no fixtures or mocks. It also prevents the slow
architectural rot where everything ends up depending on everything.

**Cost.** Occasional awkwardness moving data across the boundary. Small, and it
forces you to think about where logic belongs.

---

### D-013 — No transactional rollback of infrastructure

**Decision.** Failed applies leave resources `Degraded` and retry with backoff.
Daedalus does not attempt to undo partial changes.

**Why.** Infrastructure rollback is largely a fiction — you cannot un-delete a
disk. Honest convergence-over-time is more reliable than a rollback that works
until the one time it matters.

**Cost.** Transient inconsistent states. Mitigated by idempotent operations, a
clear `Degraded` phase, and a plan step that shows exactly what will be retried.
