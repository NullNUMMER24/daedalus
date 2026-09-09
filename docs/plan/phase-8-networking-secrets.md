# Phase 8 — Networking, secrets, and a second provider

**Goal.** Close the multi-tenancy story: VLAN per tenant, IPAM, DNS, real secret
management. Then prove the provider abstraction with a second implementation.

**Demo.** Creating a tenant provisions their VLAN. VMs get IPs and DNS records
automatically. Secrets are per-tenant encrypted. A libvirt (or KubeVirt) VM is
created through the same code path as a Proxmox one.

**Estimate.** 3–4 weeks.

**Rust you will learn.** Cryptography APIs, careful `Drop` and zeroisation,
possibly FFI, protocol clients.

---

## Step 8.1 — Network and Subnet resources

```yaml
apiVersion: daedalus.io/v1alpha1
kind: Network
metadata: { name: prod-net, tenant: acme }
spec:
  provider: pve-main
  type: vlan
  vlanId: 20                        # allocated by the platform, not chosen
  bridge: vmbr0
---
apiVersion: daedalus.io/v1alpha1
kind: Subnet
metadata: { name: prod-v4, tenant: acme }
spec:
  network: prod-net
  cidr: 10.20.10.0/24
  gateway: 10.20.10.1
  dns: [ 10.20.0.53 ]
  pools:
    - { name: static, range: "10.20.10.10-10.20.10.99" }
    - { name: dhcp,   range: "10.20.10.100-10.20.10.199" }
    - { name: lb,     range: "10.20.10.200-10.20.10.220" }   # MetalLB
  domain: prod.acme.lab
```

⚠️ **VLAN IDs are platform-allocated, never tenant-chosen.** If a tenant could
pick `vlanId: 20` when another tenant already has it, you have handed them a
network-level cross-tenant path. Allocate from a configured range at tenant
creation and reject any tenant-supplied value.

**Check:** creating a tenant allocates the next free VLAN. A manifest specifying
a VLAN owned by another tenant is rejected at validation.

---

## Step 8.2 — Proxmox SDN

```
POST /cluster/sdn/zones           create a zone (once, platform-level)
POST /cluster/sdn/vnets           create the VNet with the tenant's VLAN tag
POST /cluster/sdn/vnets/{v}/subnets
POST /cluster/sdn/                apply pending changes  ← ⚠️ easy to forget
```

⚠️ Proxmox SDN changes are staged and require an explicit **apply** to take
effect. Forget it and everything looks fine in the API while nothing works on the
wire — a genuinely confusing hour of debugging.

If you are not using SDN, fall back to pre-created bridges with VLAN tags on the
VM's NIC (`net0=virtio,bridge=vmbr0,tag=20`). Simpler, and fine for a static lab.

**Check:** two VMs on different tenant VLANs cannot reach each other; both reach
their gateway.

---

## Step 8.3 — IPAM

```rust
pub struct Ipam { /* ... */ }

impl Ipam {
    /// Allocate atomically. Two concurrent applies must never get the same IP.
    pub async fn allocate(&self, scope: &TenantScope, subnet: &Name,
                          pool: &str, owner: Uid) -> Result<IpAddr>;
    pub async fn release(&self, scope: &TenantScope, addr: IpAddr) -> Result<()>;
    /// Pin a specific address, failing if taken.
    pub async fn reserve(&self, scope: &TenantScope, addr: IpAddr, owner: Uid)
        -> Result<()>;
}
```

```sql
CREATE TABLE ip_allocations (
    address    TEXT PRIMARY KEY,           -- unique constraint = the lock
    subnet_id  TEXT NOT NULL,
    tenant_id  TEXT NOT NULL,
    owner_uid  TEXT,
    state      TEXT NOT NULL,              -- allocated | reserved | released
    updated_at TEXT NOT NULL
);
```

⚠️ Allocation must be a **single atomic transaction**: select the lowest free
address and insert it in one statement, relying on the primary key to reject
duplicates, then retry on conflict. A read-then-write will hand the same IP to
two VMs under concurrency, and the resulting duplicate-IP bug is horrible to
diagnose.

⚠️ Do not reuse a released IP immediately — hold it in `released` for a grace
period (say an hour). ARP caches and DNS TTLs outlive the VM.

**Check:** a concurrency test allocating 100 addresses from 10 tasks yields 100
distinct IPs and no errors.

---

## Step 8.4 — DNS

Register `A` and `PTR` records when a machine gets an IP; remove them on delete.

Support pluggable backends behind a small trait:

| Backend | Notes |
| --- | --- |
| **PowerDNS** | Clean REST API. The easiest to implement. |
| **Technitium** | Popular in homelabs, has an HTTP API |
| **RFC 2136** | Dynamic updates with TSIG; works with BIND/Knot |
| **Hosts file** | Write `/etc/hosts` entries. Crude, useful for testing. |

```rust
#[async_trait]
pub trait DnsProvider: Send + Sync {
    async fn upsert(&self, fqdn: &str, addr: IpAddr, ttl: u32) -> Result<()>;
    async fn remove(&self, fqdn: &str) -> Result<()>;
}
```

⚠️ DNS records are **per-tenant-domain** (`web-01.prod.acme.lab`). A tenant must
not be able to create a record in another tenant's zone — validate the FQDN
against the tenant's own domain before every write.

**Check:** creating a VM produces a resolvable name; deleting it removes the
record.

---

## Step 8.5 — Secrets, properly

```rust
use secrecy::{SecretString, ExposeSecret};

pub struct TenantKeyring {
    tenant: TenantId,
    identity: age::x25519::Identity,      // private — held only in memory
}

impl TenantKeyring {
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<SecretVec<u8>>;
    pub fn recipient(&self) -> age::x25519::Recipient;   // public
}
```

Rules, all of which need a test:

1. One age key pair per tenant, generated at tenant creation.
2. Private keys sealed with the server master key (env, `0600` file, or KMS) —
   **never** in the database in plaintext.
3. Decryption happens in memory, only while acting for that tenant.
4. Plaintext is wrapped in `SecretString`/`SecretVec` — `Debug` and `Display`
   print `[REDACTED]`, and `zeroize` clears it on drop.
5. Secrets never appear in plan output. Render `(secret: web-01-userdata @ v3)`.
6. Secrets never reach logs. Never `tracing::debug!(?spec)` a spec containing one.

```rust
// A compile-time guard against the most likely mistake.
#[derive(Serialize, Deserialize)]
pub struct CloudInitSpec {
    #[serde(skip_serializing)]      // never serialised into plan output
    user_data: Option<SecretString>,
}
```

⚠️ `#[derive(Debug)]` on a struct containing a raw `String` secret will print it
the first time anyone logs that struct. `secrecy` exists precisely to make that
impossible. Use it everywhere, from the start.

**Check:** a test asserting `format!("{:?}", secret)` contains `REDACTED` and not
the value. A test that plan output for a machine with secrets contains no
plaintext.

---

## Step 8.6 — SOPS compatibility

Match the SOPS file format so `sops` and its editor integrations work directly:

```bash
sops --encrypt --age age1acme7k2... secrets.yaml > secrets.sops.yaml
dae secret sync              # load into Daedalus
```

Implement key rotation:

```bash
dae secret rotate-key acme
# 1. generate a new key pair
# 2. re-encrypt every tenant secret to the new recipient
# 3. commit the new public key to the tenant repo
# 4. retire the old private key after a grace period
```

⚠️ Rotation must be **atomic per secret** and resumable. Losing a key mid-rotation
means unrecoverable secrets — keep the old key until every secret is confirmed
re-encrypted and verified decryptable.

**Check:** `sops` can decrypt a Daedalus-written file, and vice versa. Rotation
interrupted halfway leaves everything decryptable by one key or the other.

---

## Step 8.7 — A second provider

Pick one. This step exists to **prove the trait is real**, so pick whichever
teaches you more.

### Option A — libvirt (the honest test)

Reveals whether `MachineProvider` accidentally encodes Proxmox concepts. Start
with `russh` driving `virsh` over SSH — no build dependency, no FFI, working in
days. Move to the `virt` crate only if you need events.

Expect to find leaks: Proxmox's `node` concept, pools, and VMID integers have
probably crept into your trait. **Fix the trait, do not special-case.** That is
the entire value of this step.

### Option B — KubeVirt (the strategic one)

Only worth it now if you already run bare-metal Kubernetes with real storage —
see [providers.md](../providers.md#where-kubevirt-fits) for why it is not the
foundation. With `kube-rs` the VM object is straightforward; the work is the CDI
`DataVolume` dance for disk images and Multus for VLAN attachment. Budget a week
once the trait is settled.

**Check:** the same `Machine` YAML, with only `spec.provider` changed, creates a
VM on either backend. If that requires touching anything outside the provider
crate, the abstraction has a leak — find it and fix it.

---

## Step 8.8 — Volumes

```yaml
apiVersion: daedalus.io/v1alpha1
kind: Volume
metadata: { name: db-data, tenant: acme }
spec:
  size: 500Gi
  storage: tenant-acme-ssd
  attachTo: { machine: db-01, device: scsi1 }
```

Volumes have a lifecycle independent of machines, so data survives a VM
recreation.

⚠️ Detach before deleting a machine, or you orphan the disk. And **never**
implicitly delete a volume when its machine is deleted — require an explicit
`Volume` removal from Git. Data loss should always require an explicit act.

**Check:** recreate a machine (image change); the attached volume survives with
its data intact.

---

## Step 8.9 — The Icarus agent (optional)

Only if you need bare-metal onboarding or per-host metrics. Design notes:

- **Outbound-only** connection: the agent dials `daedalusd`, so no inbound
  firewall rules on hosts.
- **mTLS** with per-agent certificates, issued at enrolment via a one-time token.
- **Untrusted by design** — the server verifies anything actionable against
  another source. See [concepts.md](../concepts.md#icarus--the-host-agent).
- Ship it as a single static binary with a systemd unit.

⚠️ Do not build this unless you have a concrete need. It is a whole second
deployment surface, and Proxmox plus Kubernetes cover almost everything a homelab
does.

---

## Definition of done

- [ ] VLAN allocated per tenant, platform-controlled
- [ ] Cross-VLAN traffic denied by default (tested with real pings)
- [ ] IPAM allocates atomically under concurrency; released IPs held in grace
- [ ] DNS records created and removed with machines, scoped to tenant domains
- [ ] Per-tenant age keys; secrets never in plaintext at rest, in logs, or in plans
- [ ] SOPS interoperability both directions
- [ ] Key rotation works and is resumable
- [ ] A second provider works via the same `Machine` YAML
- [ ] Volumes survive machine recreation

## Pitfalls

- **Tenant-chosen VLAN IDs.** A cross-tenant network path handed over on request.
- **Read-then-write IPAM.** Duplicate IPs under concurrency; nightmarish to debug.
- **Forgetting Proxmox's SDN apply step.** Everything looks configured; nothing works.
- **Immediate IP reuse.** ARP and DNS caches will haunt you.
- **`#[derive(Debug)]` on a struct holding a raw secret.** Use `secrecy`.
- **Special-casing the second provider** instead of fixing the trait. That
  defeats the entire purpose of building it.
- **Implicitly deleting volumes with machines.** Data loss must be explicit.
