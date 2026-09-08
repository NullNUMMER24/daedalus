# Phase 7 — Kubernetes

**Goal.** Provision k3s clusters onto Daedalus-managed VMs, manage their
kubeconfigs, and bootstrap Flux.

**Demo.** `dae cluster create prod --nodes 3` produces a working cluster;
`dae kubeconfig prod | kubectl get nodes` shows three ready nodes.

**Estimate.** 3–4 weeks.

**Rust you will learn.** Complex async orchestration, timeouts and deadlines,
state machines across long-running operations, `kube-rs`.

---

## Step 7.1 — The Cluster spec

```yaml
apiVersion: daedalus.io/v1alpha1
kind: Cluster
metadata:
  name: prod
  tenant: acme
spec:
  distribution: k3s
  version: v1.31.4+k3s1
  provider: pve-main

  controlPlane:
    replicas: 1                    # 3 for HA — needs an embedded-etcd k3s
    class: standard-4x8
    image: debian-12-cloud

  nodePools:
    - name: workers
      replicas: 3
      class: standard-4x8
      image: debian-12-cloud
      labels: { workload: general }

  network:
    ref: prod-net
    podCidr:     10.42.0.0/16
    serviceCidr: 10.43.0.0/16
    apiEndpoint: 10.20.10.10       # kube-vip or a single CP node's address

  addons:
    flux:
      enabled: true
      repo: git@github.com:acme/k8s-apps.git
      path: clusters/prod
    metallb:
      enabled: true
      pool: 10.20.10.200-10.20.10.220

  dependsOn: [ Network/prod-net ]
```

⚠️ A `Cluster` **owns** its `Machine`s. They are created by the cluster
controller, not written by hand in Git. Model this explicitly with an
`ownerRef` so deleting the cluster deletes its VMs, and so a stray `dae apply`
does not try to prune them as orphans.

**Check:** the spec parses; the graph shows `Cluster` depending on `Network` and
owning the machines it will create.

---

## Step 7.2 — The bootstrap state machine

Cluster creation is long (minutes) and fails in interesting ways. Model it
explicitly, persist the state, and make every step idempotent.

```rust
pub enum ClusterPhase {
    Pending,
    CreatingControlPlane,     // VMs being created
    WaitingForControlPlane,   // waiting for cloud-init and the API to answer
    InstallingK3sServer,
    FetchingKubeconfig,
    CreatingWorkers,
    JoiningWorkers,
    InstallingAddons,
    Ready,
    Degraded { at: Box<ClusterPhase>, error: String },
}
```

💡 The Rust lesson: an enum with data makes the whole lifecycle visible in one
place, and the compiler forces you to handle every phase in your `match`. Add a
phase later and every incomplete match becomes a build error pointing you at what
you forgot.

⚠️ Persist the phase after **every** transition. A `daedalusd` restart in the
middle of cluster creation must resume, not restart — restarting means orphaned
VMs and a half-joined cluster.

**Check:** kill the server during `CreatingWorkers`; on restart it resumes from
that phase and completes.

---

## Step 7.3 — Bootstrapping via cloud-init

k3s installs in one line, which is exactly why it is first.

**Control plane:**

```yaml
#cloud-config
write_files:
  - path: /etc/rancher/k3s/config.yaml
    content: |
      token: ${K3S_TOKEN}
      tls-san: [ "${API_ENDPOINT}" ]
      cluster-cidr: ${POD_CIDR}
      service-cidr: ${SERVICE_CIDR}
      disable: [ traefik, servicelb ]     # Flux and MetalLB own these
      write-kubeconfig-mode: "0640"
runcmd:
  - curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION=${VERSION} sh -
```

**Workers:**

```yaml
runcmd:
  - curl -sfL https://get.k3s.io | K3S_URL=https://${CP_IP}:6443 \
      K3S_TOKEN=${TOKEN} INSTALL_K3S_VERSION=${VERSION} sh -
```

⚠️ **The join token is a secret.** Generate it per cluster, store it as a
Daedalus `Secret` (age-encrypted), and inject it into cloud-init at render time.
It must never be written to Git in plaintext and never appear in a log line or
a plan output. Anyone with this token can join a node to your cluster.

Render with `minijinja` in its sandboxed mode — no filesystem, no environment
access, no arbitrary function calls.

**Check:** a single-node cluster comes up and `kubectl get nodes` shows Ready.

---

## Step 7.4 — Waiting well

The hardest part of this phase is knowing when to proceed. Four distinct waits:

```rust
async fn wait_for_cloud_init(&self, m: &Machine) -> Result<()>;  // guest agent
async fn wait_for_api(&self, endpoint: &str)     -> Result<()>;  // /readyz
async fn wait_for_node_ready(&self, node: &str)  -> Result<()>;  // kube API
async fn wait_for_all_nodes(&self, n: usize)     -> Result<()>;
```

Every wait needs: a **deadline**, a **poll interval**, and a **meaningful error**
on timeout.

```rust
async fn wait_for<F, Fut>(deadline: Duration, interval: Duration,
                          what: &str, mut f: F) -> Result<()>
where F: FnMut() -> Fut, Fut: Future<Output = Result<bool>>
{
    let start = Instant::now();
    loop {
        match f().await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            // ⚠️ Transient errors are expected while things start.
            Err(e) => debug!(?e, "still waiting for {what}"),
        }
        if start.elapsed() > deadline {
            return Err(Error::Timeout {
                what: what.into(), waited: start.elapsed() });
        }
        tokio::time::sleep(interval).await;
    }
}
```

⚠️ "Connection refused" while waiting for an API to start is **normal**, not a
failure. Treat errors as "not ready yet" until the deadline, then fail with a
message naming what you were waiting for and for how long.

⚠️ Get the timeouts from reality, not from optimism. A VM cloning and booting on
spinning rust takes minutes. Suggested: cloud-init 10m, API 5m, node ready 5m.
Make them configurable.

**Check:** timeouts produce `waited 10m0s for cloud-init on Machine/cp-01`, not
a bare `Timeout`.

---

## Step 7.5 — kubeconfig management

```rust
async fn fetch_kubeconfig(&self, cluster: &Cluster) -> Result<Kubeconfig> {
    // Read /etc/rancher/k3s/k3s.yaml over SSH,
    // then rewrite server: https://127.0.0.1:6443 to the real endpoint.
}
```

Store it as a Daedalus `Secret`, encrypted with the tenant's key.

```bash
dae kubeconfig prod              # to stdout
dae kubeconfig prod --merge      # into ~/.kube/config, as context "acme-prod"
```

⚠️ k3s ships a kubeconfig with `cluster-admin`. Do not hand that to
`tenant-viewer`. Generate **scoped** kubeconfigs per role by creating a
ServiceAccount and RBAC binding in the cluster and issuing a bound token:

| Daedalus role | Kubernetes binding |
| --- | --- |
| `tenant-owner` | `cluster-admin` |
| `tenant-operator` | `edit` on their namespaces |
| `tenant-viewer` | `view` |

**Check:** a viewer's kubeconfig can `get pods` but not `delete` them.

---

## Step 7.6 — The `kube-rs` client

```rust
let config = Config::from_custom_kubeconfig(kubeconfig, &Default::default()).await?;
let client = Client::try_from(config)?;
let nodes: Api<Node> = Api::all(client.clone());
for n in nodes.list(&ListParams::default()).await? { /* ... */ }
```

Use it for: node readiness, cluster version, installing addons, and reporting
cluster health into `status`.

⚠️ Cache clients per cluster (`HashMap<ClusterId, Client>`), and rebuild on
kubeconfig rotation. Constructing a client per request is slow and will exhaust
connections.

**Check:** `dae describe cluster prod` shows node count, versions, and readiness
pulled live.

---

## Step 7.7 — Scaling and upgrades

```bash
dae cluster scale prod --pool workers --replicas 5
dae cluster upgrade prod --version v1.32.0
```

Scale up: create VMs, join them. Scale down: `cordon` → `drain` → remove from the
cluster → delete the VM. **In that order.**

⚠️ Never delete a node's VM before draining it. You will lose whatever was
running on it, and the failure mode is silent.

Upgrade: one node at a time, control plane first, drain and uncordon around each,
verify readiness before proceeding. Fail loudly and stop rather than continuing
through a broken upgrade.

**Check:** scale to 5 and back to 3 with a workload running; the workload stays
available throughout.

---

## Step 7.8 — Flux bootstrap

```rust
async fn bootstrap_flux(&self, cluster: &Cluster, cfg: &FluxConfig) -> Result<()> {
    // 1. Apply the flux-system manifests for the pinned version
    // 2. Create the deploy-key secret from a Daedalus Secret
    // 3. Apply a GitRepository + Kustomization for the tenant's app repo
    // 4. Wait for the Kustomization to report Ready
}
```

This is where [D-010](../decisions.md#d-010--application-delivery-is-delegated-to-flux)
becomes concrete: Daedalus hands off at the cluster boundary. Infrastructure is
Daedalus's Git repo; applications are Flux's.

**Check:** after cluster creation, Flux is running and has reconciled the
tenant's app repository.

---

## Step 7.9 — Importing an existing cluster

```bash
dae cluster import legacy --kubeconfig ./kc --tenant acme
```

Adopt a cluster Daedalus did not create: store the kubeconfig, mark it
`unmanaged: true`, report health and workloads, but refuse scale and upgrade
operations it cannot safely perform.

💡 This is the single feature most likely to make you use Daedalus for your
existing lab rather than only for new things — which is what turns it from a
project into a tool.

**Check:** an existing cluster appears in `dae get clusters` with health, and
`dae cluster scale` on it is refused with a clear reason.

---

## Step 7.10 — Talos groundwork (optional)

If you want to start on Talos now, do the design but not the implementation:

- Talos has **no SSH**. Everything goes over a gRPC API with mTLS.
- Machine config is declarative YAML — a much better fit for Daedalus than
  cloud-init scripting.
- `talosctl gen config` produces the PKI; store it as Daedalus `Secret`s.
- The Rust story is thin: you will likely shell out to `talosctl`, or generate
  the gRPC client from Talos's protobufs with `tonic`.

Recommendation: ship k3s, use it for a few months, then implement Talos as a
second `ClusterProvider` in Phase 8+. It will validate that the trait is real,
the same way libvirt validates `MachineProvider`.

---

## Definition of done

- [ ] `dae cluster create` produces a working single-node cluster
- [ ] Multi-node with separate control plane and workers
- [ ] Bootstrap resumes correctly after a `daedalusd` restart
- [ ] Join tokens stored encrypted; never logged, never in Git plaintext
- [ ] kubeconfig retrieved, stored encrypted, and role-scoped
- [ ] `dae kubeconfig prod --merge` gives a working `kubectl`
- [ ] Scale up and down with proper cordon/drain
- [ ] Rolling upgrade, one node at a time
- [ ] Flux bootstrapped and reconciling
- [ ] Existing clusters can be imported
- [ ] Cluster deletion removes its VMs

## Pitfalls

- **Deleting a node's VM before draining it.** Silent data loss.
- **Join tokens in logs or plan output.** Redact deliberately.
- **Optimistic timeouts.** Real VMs boot slowly. Measure, then double.
- **Treating "connection refused" as failure** while waiting for startup.
- **Handing cluster-admin to everyone** because k3s's default kubeconfig does.
- **Not persisting bootstrap phase.** A restart mid-create orphans VMs.
- **Reimplementing Argo CD.** Bootstrap Flux and stop.
