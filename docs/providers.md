# Providers

A **provider** is a driver for an external system that actually owns
infrastructure. Daedalus talks to providers exclusively through one trait, which
is what keeps "Proxmox first" from becoming "Proxmox only".

---

## The trait

```rust
#[async_trait]
pub trait MachineProvider: Send + Sync {
    /// Stable identifier, e.g. "proxmox", "libvirt", "kubevirt".
    fn kind(&self) -> &'static str;

    /// Reachability and version check. Cheap; called by the health loop.
    async fn health(&self) -> Result<ProviderHealth>;

    /// List everything this provider holds that is tagged for the given tenant.
    /// This is how observed state and reindexing work.
    async fn observe(&self, scope: &TenantScope)
        -> Result<Vec<ObservedMachine>>;

    /// Fetch one, by the provider's own reference.
    async fn get(&self, r: &ProviderRef) -> Result<Option<ObservedMachine>>;

    async fn create(&self, scope: &TenantScope, spec: &MachineSpec)
        -> Result<ProviderRef>;

    /// Providers report which changes they can make without recreating,
    /// so the planner can classify updates correctly.
    fn update_strategy(&self, from: &MachineSpec, to: &MachineSpec)
        -> UpdateStrategy;

    async fn update(&self, r: &ProviderRef, to: &MachineSpec) -> Result<()>;
    async fn delete(&self, r: &ProviderRef, opts: DeleteOptions) -> Result<()>;

    async fn power(&self, r: &ProviderRef, op: PowerOp) -> Result<()>;

    /// Optional capabilities. Default implementations return
    /// `Err(Unsupported)`, so a provider only implements what it can do.
    async fn snapshot(&self, _r: &ProviderRef, _name: &str) -> Result<()> {
        Err(ProviderError::Unsupported("snapshot"))
    }
    async fn console(&self, _r: &ProviderRef) -> Result<ConsoleHandle> {
        Err(ProviderError::Unsupported("console"))
    }
    async fn migrate(&self, _r: &ProviderRef, _target: &str) -> Result<()> {
        Err(ProviderError::Unsupported("migrate"))
    }

    /// Declared capabilities, so the UI can hide buttons that would fail
    /// and the planner can reject impossible specs at validation time.
    fn capabilities(&self) -> Capabilities;
}
```

Three design points worth keeping:

1. **`observe` is tenant-scoped and returns a list.** Not "get by ID". This is
   what makes recorded state rebuildable and adoption possible.
2. **`update_strategy` is a pure function**, separate from `update`. The planner
   needs to know whether a change is destructive *before* asking for approval,
   without touching the provider.
3. **Capabilities are declared, not discovered by failure.** The web UI should
   not offer a "live migrate" button for a provider that cannot do it.

---

## Provider comparison

You asked specifically about KubeVirt, so here is the full comparison rather than
a verdict.

| | **Proxmox VE** | **libvirt / KVM** | **KubeVirt** | **Harvester** |
| --- | --- | --- | --- | --- |
| API style | REST + tokens | C library (FFI) or SSH | Kubernetes CRDs | Kubernetes CRDs |
| Rust client | `reqwest` + serde | `virt` crate (bindgen) | `kube-rs` | `kube-rs` |
| Needs K8s first | No | No | **Yes** | Ships its own |
| Clustering | Built in | No | Via K8s | Built in |
| Tenant primitive | **Pools + ACLs + tokens** | None | Namespaces + RBAC | Namespaces + RBAC |
| Storage | ZFS/LVM/Ceph, built in | Whatever you configure | Needs a CSI with block support | Longhorn, built in |
| VM networking | Bridges, SDN, VLANs | Bridges | Multus + NetworkAttachmentDefinition | Built in |
| Live migration | Yes | Manual | Needs RWX storage | Yes |
| Homelab RAM overhead | Low | Lowest | High (K8s + CSI + operators) | High |
| Cloud-init | Built in | Via config drive | Via `cloudInitNoCloud` | Built in |
| Effort to first VM | **Low** | Medium | High | Medium |

---

## Why Proxmox first

- **Pure HTTP.** `reqwest` + `serde` and you are done. No `bindgen`, no
  `pkg-config`, no C toolchain in CI. While you are learning Rust, fighting FFI
  at the same time is a tax with no learning payoff.
- **Its ACL model is your tenancy backstop.** Pools, users, per-pool API tokens
  and storage ACLs mean the hypervisor independently enforces the tenant
  boundary. See [Multi-tenancy layer 6](multitenancy.md#layer-6--infrastructure-).
  Nothing else on this list gives you that as cheaply.
- **Templates + linked clones** make VM creation take seconds, which matters
  enormously for your development loop. You will run `apply` hundreds of times.
- **It has already solved** clustering, backups (PBS), live migration and HA, so
  Daedalus does not have to.
- **Cloud-init is first-class**, which makes the Phase 7 k3s bootstrap a
  small script rather than an orchestration project.

### The Proxmox API in practice

```
POST /api2/json/nodes/{node}/qemu                 create VM
POST /api2/json/nodes/{node}/qemu/{vmid}/clone    clone a template  ← the fast path
PUT  /api2/json/nodes/{node}/qemu/{vmid}/config   update config
POST /api2/json/nodes/{node}/qemu/{vmid}/status/start
GET  /api2/json/cluster/resources?type=vm         list everything   ← observe
GET  /api2/json/pools/{poolid}                    pool membership
```

Auth header: `Authorization: PVEAPIToken=USER@REALM!TOKENID=SECRET`.

Two things that will bite you, so plan for them:

- **Most operations are asynchronous** and return a **UPID** task identifier. You
  must poll `/nodes/{node}/tasks/{upid}/status` until it completes. Build a
  `wait_for_task` helper in the very first commit of the provider and use it
  everywhere; retrofitting it later is miserable.
- **VMIDs are cluster-global integers**, not names, and allocation races if two
  clients pick simultaneously. Ask the cluster for the next free ID
  (`/cluster/nextid`) and handle the "already exists" error by retrying, rather
  than assuming your read is still valid by the time you write.

Tag Daedalus-managed VMs using Proxmox tags plus the description field:

```
tags: daedalus;tenant-acme;env-prod
description: |
  Managed by Daedalus. Do not edit by hand.
  daedalus.io/uid: 01J8ZQ4M7X...
  daedalus.io/generation: 7
```

---

## Where KubeVirt fits

**Not as the foundation. As provider number three, from Phase 8.**

The blocker is bootstrap ordering. KubeVirt runs VMs as pods, so it needs a
Kubernetes cluster to exist before it can make a VM. But you want Daedalus to
*create* your clusters, and on bare metal that is circular:

```
bare metal ──► Kubernetes ──► KubeVirt ──► VMs ──► more Kubernetes
              ▲                                          │
              └──── installed how? ──────────────────────┘
```

To break the circle you must solve **bare-metal provisioning** (PXE boot, Talos
on metal, or Tinkerbell) before your first VM exists. That is a much harder
Phase 3 than "make an HTTP call to Proxmox", and it front-loads all the risk.

Three further practical costs in a homelab:

- **Storage.** KubeVirt wants a CSI with `Block` volume mode and `ReadWriteMany`
  for live migration. In practice: Rook/Ceph (heavy, three-node minimum for
  sensible redundancy) or Longhorn (lighter, but RWX block is its weak spot).
- **Networking.** VM traffic on a VLAN needs Multus plus a bridge CNI plus
  `NetworkAttachmentDefinition`s per network. Workable, but considerably more
  moving parts than a Proxmox `vmbr0` with a tag.
- **Overhead.** The Kubernetes control plane, the CSI, and the KubeVirt operators
  consume RAM that your VMs would rather have.

And the strategic point, which matters most: **if every resource is a CRD, Flux
plus KubeVirt already does most of Daedalus's job.** Your tool shrinks to a
tenancy-and-UI layer over CRDs. That is a legitimate product, but it is a much
smaller Rust project — mostly `kube-rs` client code, rather than the graph
engine, planner, state model, and provider drivers you would actually learn from.

### When KubeVirt *does* become the right call

- You already run a stable bare-metal Kubernetes cluster with real storage.
- You want VMs and containers scheduled on the same nodes, sharing the same pool.
- You want one API for everything and are happy for Daedalus to be the tenancy
  and UX layer on top.

At that point the provider is genuinely pleasant to write: `kube-rs` gives you a
typed client, `VirtualMachine` is just a CRD, and `observe` becomes a label
selector. Budget roughly a week once the trait is settled — most of the work is
the CDI `DataVolume` dance for disk images, not the VM object itself.

**Harvester** is worth knowing about as the alternative: SUSE's productised
KubeVirt with Longhorn and networking pre-integrated. If the KubeVirt world
appeals, running Harvester and writing a Daedalus provider for it is far less
work than assembling KubeVirt yourself — and because Harvester exposes the
Kubernetes API, the same provider largely covers both.

---

## libvirt

Worth having as provider two, mostly because it is the honest test of whether the
trait abstraction is real. Proxmox is *itself* built on libvirt-adjacent
technology, so if your trait accidentally encodes Proxmox concepts, libvirt is
where that becomes obvious.

Options for the client:

- The `virt` crate — bindings to `libvirt.so`. Needs `libvirt-dev` at build time,
  which complicates cross-compilation and CI containers.
- Drive `virsh` over SSH with the `russh` crate. Ugly, no build dependency,
  perfectly adequate for a homelab, and much faster to get working.
- Speak libvirt's RPC protocol directly. Do not. It is not worth it.

Recommendation: SSH plus `virsh` first, native bindings later only if you need
events or performance.

---

## The Kubernetes provider

Distinct from KubeVirt: this one manages *clusters and what runs on them*, not
VMs.

```rust
#[async_trait]
pub trait ClusterProvider: Send + Sync {
    async fn observe(&self, scope: &TenantScope) -> Result<Vec<ObservedCluster>>;
    async fn create(&self, scope: &TenantScope, spec: &ClusterSpec)
        -> Result<ProviderRef>;
    async fn scale(&self, r: &ProviderRef, pool: &str, replicas: u32)
        -> Result<()>;
    async fn upgrade(&self, r: &ProviderRef, version: &str) -> Result<()>;
    async fn kubeconfig(&self, r: &ProviderRef) -> Result<Kubeconfig>;
    async fn delete(&self, r: &ProviderRef) -> Result<()>;
}
```

Implementations:

| Impl | Method | Phase |
| --- | --- | --- |
| **k3s** | Cloud-init runs the install script; join tokens from a `Secret` | 7 |
| **Talos** | `machineconfig` over the Talos gRPC API; no SSH at all | 8 |
| **Import** | Adopt an existing cluster from a kubeconfig | 7 |

k3s first because a single-node cluster is one cloud-init line and you get to the
interesting part — cluster lifecycle, kubeconfig management, Flux bootstrap —
in an afternoon. Talos second because it is the better long-term answer: its
machine config is declarative YAML driven over an API, which is exactly the model
Daedalus already speaks, and there is no SSH or configuration drift to manage.

---

## Writing a new provider: the checklist

1. Implement `health` first. Nothing else matters if you cannot connect.
2. Implement `observe` second. Read-only, and it immediately tells you whether
   your tagging scheme works.
3. Write the tagging code before the create code, so nothing untagged is ever
   created.
4. Implement `create`, then `delete`. Test the round trip a hundred times.
5. Implement `update_strategy` — pure, well-tested, no I/O.
6. Implement `update`.
7. Add optional capabilities last.
8. Record HTTP fixtures with `wiremock`, so the provider's tests run in CI with
   no infrastructure.
9. Keep a small integration suite gated behind `--features integration-tests`
   that runs against real hardware, and run it before every release.
