# Phase 2 — Git integration

**Goal.** Read desired state from a Git repository, store recorded state in
SQLite, and compute the difference.

**Demo.** `dae plan` shows what would change between a Git repo and a database.
Still no providers — the "observed" side is faked. But the diff engine is real.

**Estimate.** 1–2 weeks.

**Rust you will learn.** Lifetimes in earnest, iterator chains, `sqlx` and
compile-time-checked SQL, error aggregation, `BTreeMap` ordering.

> Still mostly synchronous. `sqlx` is async, so this is where you meet
> `#[tokio::main]` — but only at the edges. The diff logic in `dae-core` stays
> pure and sync.

---

## Step 2.1 — Choose a Git crate

| | `git2` | `gix` |
| --- | --- | --- |
| Backing | libgit2 (C, FFI) | Pure Rust |
| Maturity | Very mature | Rapidly improving |
| Build deps | Needs a C toolchain | None |
| API stability | Stable | Still evolving |

**Recommendation:** `git2`. Clone, fetch, commit and push all work today with
well-documented APIs, and Phase 6's write-back needs the write paths to be
reliable. Revisit `gix` later if the C dependency becomes a packaging problem.

```toml
git2 = { version = "0.19", default-features = false,
         features = ["vendored-libgit2", "vendored-openssl"] }
```

⚠️ Vendoring avoids "works on my machine" for libgit2 and OpenSSL versions.

**Check:** `cargo build` works in a clean container with no `libgit2-dev`.

---

## Step 2.2 — The repository abstraction

```rust
// crates/dae-git/src/repo.rs

pub struct GitRepo {
    inner: git2::Repository,
    workdir: PathBuf,
}

impl GitRepo {
    /// Clone if absent, fetch if present. Idempotent.
    pub fn open_or_clone(url: &str, into: &Path, auth: &GitAuth)
        -> Result<Self, GitError>;

    pub fn fetch(&self) -> Result<(), GitError>;

    /// Check out a ref into the working directory.
    pub fn checkout(&self, reference: &str) -> Result<Oid, GitError>;

    pub fn head_oid(&self) -> Result<Oid, GitError>;

    /// Paths changed between two commits — used to work out which tenants
    /// need re-planning, so one VM edit does not re-plan the whole lab.
    pub fn changed_paths(&self, from: Oid, to: Oid)
        -> Result<Vec<PathBuf>, GitError>;
}

pub enum GitAuth {
    None,
    SshKey { private_key: PathBuf, passphrase: Option<SecretString> },
    Token  { username: String, token: SecretString },
}
```

⚠️ `git2`'s SSH authentication uses a **callback** that may be invoked several
times (it retries with different credentials). Return the same key each time and
count attempts so you can fail cleanly rather than looping forever. This trips up
almost everyone the first time.

**Check:** clone a public repo over HTTPS and a private one over SSH, into a
temp dir. Then run it again and confirm it fetches instead of re-cloning.

---

## Step 2.3 — The tenant root abstraction

Support both repository layouts from [gitops.md](../gitops.md#which-repository-layout)
without branching all over the loader:

```rust
/// Where a tenant's manifests live. The loader only ever sees this.
pub struct TenantRoot {
    pub tenant: Name,
    pub repo:   Arc<GitRepo>,
    pub prefix: PathBuf,   // "tenants/acme" (monorepo) or "" (repo-per-tenant)
}
```

💡 Introducing this now costs an hour. Retrofitting it in Phase 5, once
repo-per-tenant is a requested feature, costs a week. This is the cheapest
abstraction in the plan.

**Check:** `load_tenant(&root)` returns identical resources for a monorepo
layout and an equivalent repo-per-tenant layout.

---

## Step 2.4 — Database schema

```sql
-- migrations/0001_initial.sql
CREATE TABLE tenants (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    state       TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE TABLE resources (
    uid           TEXT PRIMARY KEY,
    tenant_id     TEXT NOT NULL REFERENCES tenants(id),
    environment   TEXT NOT NULL,
    kind          TEXT NOT NULL,
    name          TEXT NOT NULL,
    generation    INTEGER NOT NULL DEFAULT 1,
    spec_json     TEXT NOT NULL,
    status_json   TEXT,
    desired_hash  TEXT NOT NULL,      -- blake3 of the resolved spec
    applied_hash  TEXT,               -- hash of what was last applied
    provider_ref  TEXT,               -- "pve://main/node-01/qemu/104"
    phase         TEXT NOT NULL,      -- Pending|Ready|Degraded|Stalled|Deleting
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    UNIQUE (tenant_id, environment, kind, name)
);
CREATE INDEX idx_resources_tenant ON resources(tenant_id);
CREATE INDEX idx_resources_phase  ON resources(phase);

-- Ariadne. Append-only: no UPDATE, no DELETE, ever.
CREATE TABLE events (
    id             TEXT PRIMARY KEY,
    ts             TEXT NOT NULL,
    tenant_id      TEXT,
    resource_uid   TEXT,
    principal_id   TEXT,
    correlation_id TEXT NOT NULL,
    kind           TEXT NOT NULL,
    payload_json   TEXT NOT NULL
);
CREATE INDEX idx_events_resource    ON events(resource_uid, ts DESC);
CREATE INDEX idx_events_correlation ON events(correlation_id);
CREATE INDEX idx_events_tenant_ts   ON events(tenant_id, ts DESC);
```

⚠️ Enable WAL mode and foreign keys on every connection, or SQLite will silently
ignore your `REFERENCES` clauses:

```rust
sqlx::sqlite::SqliteConnectOptions::from_str(&url)?
    .create_if_missing(true)
    .journal_mode(SqliteJournalMode::Wal)
    .foreign_keys(true)
    .busy_timeout(Duration::from_secs(5))
```

**Check:** `sqlx migrate run` creates the schema; `PRAGMA foreign_keys` returns 1.

---

## Step 2.5 — The repository layer

```rust
// crates/dae-store/src/resources.rs

pub struct ResourceRepo { pool: SqlitePool }

impl ResourceRepo {
    pub async fn list_by_tenant(&self, t: TenantId, kind: Option<Kind>)
        -> Result<Vec<StoredResource>>;
    pub async fn get_by_key(&self, key: &ResourceKey)
        -> Result<Option<StoredResource>>;
    pub async fn upsert(&self, r: &StoredResource) -> Result<()>;
    pub async fn set_phase(&self, uid: Uid, phase: Phase) -> Result<()>;
    pub async fn delete(&self, uid: Uid) -> Result<()>;
}
```

⚠️ These take a bare `TenantId` **for now**. Phase 5 replaces it with
`TenantScope` (see [D-007](../decisions.md#d-007--tenant-scoping-is-enforced-by-the-type-system)).
Write a `// TODO(phase-5): TenantScope` on each one so the refactor is
mechanical rather than archaeological.

💡 Use `sqlx::query!` (the macro), not `sqlx::query` (the function). The macro
checks your SQL against the real schema at compile time — a typo'd column name
becomes a build error. You will need `cargo sqlx prepare` to commit the offline
query cache so CI works without a database.

**Check:** round-trip a resource through `upsert` then `get_by_key`; the specs
compare equal.

---

## Step 2.6 — Spec hashing

```rust
/// Canonical hash of a resolved spec, for cheap "is this up to date?" checks.
pub fn spec_hash(spec: &ResolvedMachineSpec) -> Hash {
    // Serialize to canonical JSON: keys sorted, no whitespace, no floats.
    let canonical = serde_json::to_vec(&spec).expect("spec is serializable");
    blake3::hash(&canonical).into()
}
```

⚠️ **Canonical** is the load-bearing word. `HashMap` iteration order is random in
Rust, so use `BTreeMap` in your specs, or sort keys before hashing. Otherwise the
same spec hashes differently between runs and every plan shows spurious changes —
a maddening bug to track down after the fact.

**Check:** a test asserting the same spec hashes identically across 1000
iterations, and that reordering a `labels` map does not change the hash.

---

## Step 2.7 — The diff engine

The heart of the tool. Pure, synchronous, in `dae-core`.

```rust
pub fn diff(
    desired:  &[ResolvedResource],   // from Git
    recorded: &[StoredResource],     // from the database
    observed: &[ObservedResource],   // from providers (faked this phase)
) -> Plan;

pub struct Plan {
    pub actions: Vec<PlannedAction>,
    pub summary: PlanSummary,        // counts by action type
}

pub struct PlannedAction {
    pub key:      ResourceKey,
    pub action:   Action,
    pub reason:   String,            // human-readable "why"
    pub strategy: UpdateStrategy,
}
```

The decision table — write it out, then implement it:

| In Git? | In DB? | On provider? | Action |
| --- | --- | --- | --- |
| ✓ | ✗ | ✗ | **Create** |
| ✓ | ✗ | ✓ (tagged, same uid) | **Adopt** — reclaim after a database loss |
| ✓ | ✓ | ✗ | **Create** — deleted out of band; recreate |
| ✓ | ✓ | ✓, hash matches | **NoOp** |
| ✓ | ✓ | ✓, hash differs | **Update** |
| ✗ | ✓ | ✓ | **Delete** — removed from Git |
| ✗ | ✓ | ✗ | **Forget** — clean up the database row |
| ✗ | ✗ | ✓ (tagged) | **Prune** — orphan; needs explicit opt-in |

⚠️ The Adopt row is what makes [D-004](../decisions.md#d-004--provider-objects-are-tagged-recorded-state-is-rebuildable)
work. Do not skip it — it is what makes state loss survivable.

💡 This function is pure: three slices in, a `Plan` out. No I/O, no async, no
mocks needed. That is exactly why `dae-core` bans I/O dependencies — this is the
most important logic in the project and it tests in microseconds.

**Check:** a table-driven test with one case per row above. Then a `proptest`
asserting `diff(x, x, x)` always yields all-`NoOp` (idempotence).

---

## Step 2.8 — Rendering a plan

Human output first, then `-o json` for machines.

```
Plan: 2 to create, 1 to update, 1 to destroy.

  + Machine/acme/prod/web-02
      class:    standard-4x8
      networks: prod-net (10.20.10.13)

  ~ Machine/acme/prod/db-01
      spec.memory:  16Gi -> 32Gi        (in place)
      spec.disks[data].size: 200Gi -> 500Gi  (in place, grow only)

  - Machine/acme/staging/old-01
      removed from Git at commit abc123f
```

Colour: green `+`, yellow `~`, red `-`, bold for destructive. Respect `NO_COLOR`
and TTY detection.

**Check:** snapshot the rendered plan with `insta`. Now every future change to
the planner shows you exactly what moved.

---

## Step 2.9 — Wire up `dae plan`

```bash
dae plan --repo ./examples/lab --db ./local/dae.db
```

For this phase, `observed` is loaded from a JSON fixture file
(`--fake-observed ./fixtures/observed.json`). This lets you test the entire
pipeline with no infrastructure at all, and it becomes your integration test
fixture for the rest of the project.

**Check:** running `plan` twice with no changes reports "No changes.". This is
the idempotence property, end to end.

---

## Definition of done

- [ ] Clone/fetch a repo over HTTPS and SSH
- [ ] `TenantRoot` supports both layouts identically
- [ ] SQLite schema with migrations, WAL, foreign keys on
- [ ] `sqlx::query!` compile-time checking, offline cache committed
- [ ] Spec hashing is canonical and stable
- [ ] `diff()` handles all eight rows of the decision table, with tests
- [ ] Plan renders readably; snapshot-tested
- [ ] `dae plan` runs end to end against fake observed state
- [ ] Two consecutive plans with no changes both report no changes

## Pitfalls

- **Non-canonical hashing.** `HashMap` ordering will bite you. Use `BTreeMap`.
- **Forgetting `PRAGMA foreign_keys=ON`.** SQLite ignores foreign keys by
  default, silently, and you find out much later.
- **Putting diff logic in the store layer.** It belongs in `dae-core`, pure.
- **`git2` SSH callback loops.** Count attempts; fail after three.
- **Not committing `.sqlx/`.** CI cannot compile `query!` macros without the
  offline cache and the error message is not obvious.
- **Skipping the Adopt case** because it seems hypothetical. It is the property
  that makes losing your database a nuisance rather than a catastrophe.
