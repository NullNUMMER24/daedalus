# Phase 4 — The engine

**Goal.** Dependency-ordered applies, drift detection, retries, and status
tracking. Turn "it can create a VM" into "it reliably converges a lab."

**Demo.** `dae apply` builds a network, then three VMs that depend on it, in the
right order and in parallel where possible. Change a VM by hand on Proxmox and
`dae drift` reports it.

**Estimate.** 2–3 weeks.

**Rust you will learn.** Graph algorithms, `tokio` task concurrency,
`JoinSet`, channels, `Semaphore`, cancellation, backoff, state machines.

---

## Step 4.1 — Build the Labyrinth

```rust
// crates/dae-engine/src/graph.rs

pub struct Labyrinth {
    nodes: Vec<ResourceKey>,
    edges: Vec<(usize, usize)>,        // (dependency, dependent)
    index: HashMap<ResourceKey, usize>,
}

impl Labyrinth {
    pub fn build(resources: &[ResolvedResource])
        -> Result<Self, GraphError>;

    /// Resources grouped by dependency depth. Everything in wave N may run
    /// concurrently; wave N+1 waits for wave N.
    pub fn waves(&self) -> Vec<Vec<ResourceKey>>;

    /// Reverse order, for deletion.
    pub fn reverse_waves(&self) -> Vec<Vec<ResourceKey>>;
}
```

Edges come from two sources:

- **Explicit** — the `dependsOn` field
- **Implicit** — any field referencing another resource (`networks[].ref`,
  `spec.class`, `cluster.controlPlane`)

💡 Use `petgraph` rather than writing your own. `petgraph::algo::toposort`
returns the cycle on failure, which is exactly what you need for the error
message. Writing your own DFS is a fine exercise but not the point of this phase.

⚠️ Cycle errors must **name the cycle**:

```
error: dependency cycle detected
  Machine/acme/prod/a  →  Machine/acme/prod/b
  Machine/acme/prod/b  →  Network/acme/prod/n
  Network/acme/prod/n  →  Machine/acme/prod/a
```

**Check:** a diamond dependency produces three waves. A cycle is rejected at
validation time, before anything is touched, with the cycle listed.

---

## Step 4.2 — The executor

```rust
pub struct Executor {
    providers: HashMap<Name, Arc<dyn MachineProvider>>,
    store:     Arc<ResourceRepo>,
    events:    Arc<EventRepo>,
    limits:    ConcurrencyLimits,
}

impl Executor {
    pub async fn apply(&self, plan: &Plan, ctx: &ApplyContext)
        -> Result<ApplyReport>;
}
```

Concurrency, per wave:

```rust
let sem = Arc::new(Semaphore::new(limits.per_provider));
let mut set = JoinSet::new();

for action in wave {
    let permit = sem.clone().acquire_owned().await?;
    set.spawn(async move {
        let _permit = permit;                 // released on drop
        execute_one(action).await
    });
}

// ⚠️ Collect ALL results. Do not `?` on the first error — the other
// actions in this wave are still running and their outcomes matter.
let mut results = Vec::new();
while let Some(res) = set.join_next().await {
    results.push(res);
}
```

💡 `JoinSet` is the right tool: it owns the tasks, yields results as they finish,
and aborts everything on drop. Compare with `futures::join_all`, which needs all
futures up front and gives you no incremental progress.

⚠️ Cap per-provider concurrency at 3–5. Proxmox does not enjoy twenty
simultaneous full clones, and you will get timeouts that look like bugs in your
code.

**Check:** three independent VMs in one wave create concurrently (visible in the
timing) while a dependent VM waits.

---

## Step 4.3 — Leases

Stop two applies fighting over one resource:

```sql
CREATE TABLE leases (
    resource_uid TEXT PRIMARY KEY,
    holder       TEXT NOT NULL,     -- correlation id
    acquired_at  TEXT NOT NULL,
    expires_at   TEXT NOT NULL
);
```

```rust
/// RAII lease. Released on drop, and expired automatically if the
/// process dies holding it.
pub struct Lease { uid: Uid, repo: Arc<LeaseRepo> }

impl Drop for Lease {
    fn drop(&mut self) {
        // Best-effort async release; the expiry is the real safety net.
        let (uid, repo) = (self.uid, self.repo.clone());
        tokio::spawn(async move { let _ = repo.release(uid).await; });
    }
}
```

💡 The `Drop` + expiry combination is the important part. `Drop` handles the
normal path; the expiry handles `kill -9`. Relying on `Drop` alone gives you
permanent deadlocks after a crash, which is a bad night.

Also serialise applies **per tenant**, so quota arithmetic cannot race. Different
tenants still run concurrently.

**Check:** two concurrent applies on the same resource — one proceeds, one waits.
Kill the process holding a lease; confirm it expires and is reclaimed.

---

## Step 4.4 — Resource phases

```rust
pub enum Phase {
    Pending,     // planned, not yet applied
    Creating,
    Ready,       // applied and observed as healthy
    Updating,
    Degraded,    // apply failed; will retry
    Stalled,     // failed too many times; needs a human
    Deleting,
    Deleted,
}
```

Transitions are explicit and every one emits an Ariadne event. Model it as a real
state machine — an enum with a `transition` function that rejects impossible
moves — rather than a `String` column you assign to from ten places.

💡 Making illegal states unrepresentable is the whole point of Rust enums.
`Ready → Creating` should not compile, let alone happen at runtime.

**Check:** a failing apply moves a resource to `Degraded` and records the reason;
a subsequent successful apply returns it to `Ready`.

---

## Step 4.5 — Retries and backoff

```rust
pub struct RetryPolicy {
    max_attempts:  u32,               // 5
    initial:       Duration,          // 1s
    max:           Duration,          // 5m
    multiplier:    f64,               // 2.0
    jitter:        f64,               // 0.2 — ±20%
}
```

⚠️ **Only retry retryable errors.** Classify them:

| Error | Retry? |
| --- | --- |
| Connection refused, timeout, 502/503 | ✅ yes |
| 429 rate limited | ✅ yes, honour `Retry-After` |
| Task failed: "storage full" | ❌ no — retrying cannot help |
| 401/403 | ❌ no — credentials will not fix themselves |
| 400 invalid parameter | ❌ no — the spec is wrong |

Retrying a non-retryable error wastes five minutes and buries the real message.
Put the classification on your error type as `fn is_retryable(&self) -> bool` so
it lives next to the variants.

Jitter matters: without it, five resources failing together retry in lockstep
forever.

**Check:** a wiremock server returning 503 three times then 200 — the operation
succeeds after retries. A 403 fails immediately without retrying.

---

## Step 4.6 — Drift detection

```rust
pub struct DriftReport {
    pub resource: ResourceKey,
    pub fields:   Vec<FieldDrift>,   // path, expected, actual
    pub since:    DateTime<Utc>,     // when first observed
}
```

Runs on the drift loop (default 5 minutes). Per
[D-005](../decisions.md#d-005--plan-and-apply-are-separate-and-drift-is-reported-not-corrected),
it **reports** by default:

```yaml
metadata:
  annotations:
    daedalus.io/drift-policy: report | correct | ignore
```

⚠️ Distinguish drift from **transient state**. A VM mid-reboot is not drifted.
Require a field to differ across two consecutive observations before reporting,
and never treat `status`-only fields as drift.

**Check:** change a VM's memory in the Proxmox UI; `dae drift` reports it within
one interval, naming the field, old value, and new. Reboot a VM; nothing is
reported.

---

## Step 4.7 — Ariadne events

```rust
pub enum EventKind {
    PlanComputed { actions: usize },
    ActionStarted { action: Action },
    ActionSucceeded { action: Action, duration: Duration },
    ActionFailed { action: Action, error: String, will_retry: bool },
    PhaseChanged { from: Phase, to: Phase },
    DriftDetected { fields: Vec<FieldDrift> },
    DriftResolved,
}
```

Every event carries `correlation_id`, so one apply run is a single queryable
thread:

```bash
$ dae events --correlation 01J8ZQ4M7X
14:22:31  PlanComputed        4 actions
14:22:31  ActionStarted       Create Network/acme/prod/prod-net
14:22:34  ActionSucceeded     Create Network/acme/prod/prod-net  (2.8s)
14:22:34  ActionStarted       Create Machine/acme/prod/web-01
14:22:34  ActionStarted       Create Machine/acme/prod/web-02
14:22:51  ActionSucceeded     Create Machine/acme/prod/web-01  (17.2s)
14:22:53  ActionFailed        Create Machine/acme/prod/web-02
                              storage 'tenant-acme-ssd' full — will not retry
```

💡 Bridge `tracing` spans into events by attaching the correlation ID as a span
field. Then your logs and your event log tell the same story with the same key,
which is exactly what you want at 2am.

**Check:** a full apply produces a readable, ordered narrative under one ID.

---

## Step 4.8 — Control loops

```rust
pub struct Scheduler { /* ... */ }

impl Scheduler {
    pub async fn run(self, shutdown: CancellationToken) {
        let mut set = JoinSet::new();
        set.spawn(git_poll_loop(...,     Duration::from_secs(60)));
        set.spawn(drift_loop(...,        Duration::from_secs(300)));
        set.spawn(reconcile_worker(...));
        set.spawn(health_loop(...,       Duration::from_secs(30)));
        set.spawn(lease_gc_loop(...,     Duration::from_secs(30)));
        shutdown.cancelled().await;
        set.shutdown().await;
    }
}
```

Every loop pattern:

```rust
let mut ticker = tokio::time::interval(period);
ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);   // ⚠️
loop {
    tokio::select! {
        _ = ticker.tick() => { if let Err(e) = do_work().await {
                                   warn!(?e, "loop iteration failed"); } }
        _ = shutdown.cancelled() => break,
    }
}
```

⚠️ Two things here. `MissedTickBehavior::Skip` prevents a thundering herd of
catch-up ticks after a slow iteration — the default `Burst` will fire them all at
once. And **a loop must never exit on error**; log and continue, or your drift
detection silently stops after one bad night.

**Check:** `daedalusd` starts all loops, logs each tick, and shuts down cleanly
on SIGTERM within a couple of seconds.

---

## Step 4.9 — Graceful shutdown

```rust
tokio::select! {
    _ = signal::ctrl_c() => {},
    _ = sigterm() => {},
}
info!("shutting down; waiting for in-flight actions");
shutdown.cancel();
tokio::time::timeout(Duration::from_secs(30), set.shutdown()).await.ok();
```

⚠️ Never abort mid-apply if you can avoid it. An interrupted `create` leaves a
half-configured VM. Let in-flight actions finish (with a timeout), stop accepting
new ones, and release leases on the way out.

**Check:** SIGTERM during an apply completes the current action before exiting,
and the resource is left in a coherent phase.

---

## Step 4.10 — `dae apply` end to end

```bash
$ dae apply
Plan: 4 to create.
  + Network/acme/prod/prod-net
  + Machine/acme/prod/web-01
  + Machine/acme/prod/web-02
  + Machine/acme/prod/db-01

Apply? [y/N] y

Wave 1/2
  ✓ Network/acme/prod/prod-net              2.8s
Wave 2/2
  ✓ Machine/acme/prod/web-01               17.2s
  ✓ Machine/acme/prod/web-02               18.1s
  ✓ Machine/acme/prod/db-01                21.4s

Applied 4 resources in 24.3s.  correlation: 01J8ZQ4M7X
```

**Check:** apply the example lab from empty. Then apply again — no changes.

---

## Definition of done

- [ ] Dependency graph built from explicit and implicit references
- [ ] Cycles rejected at validation, naming the cycle
- [ ] Waves execute in order; independent actions run concurrently
- [ ] Per-provider concurrency limits enforced
- [ ] Leases prevent concurrent applies and expire after a crash
- [ ] Phases transition through a real state machine
- [ ] Retries with backoff and jitter, only for retryable errors
- [ ] Drift detected and reported; transient states not misreported
- [ ] Events form a readable narrative under one correlation ID
- [ ] All loops run, log, and shut down cleanly on SIGTERM
- [ ] Full apply of the example lab, then a clean no-op re-apply

## Pitfalls

- **Aborting the whole wave on first failure.** Independent branches should
  continue. Collect results.
- **Retrying non-retryable errors.** Wastes minutes and hides the real message.
- **`MissedTickBehavior::Burst`** (the default) causing tick storms.
- **A loop that exits on error.** It will, silently, and you will not notice for
  a week.
- **Leases without expiry.** One `kill -9` and the resource is locked forever.
- **Reporting reboots as drift.** Require two consecutive observations.
- **Unbounded concurrency.** Proxmox will time out and you will blame your code.
