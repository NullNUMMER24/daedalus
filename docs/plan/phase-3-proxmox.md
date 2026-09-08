# Phase 3 — The Proxmox provider

**Goal.** Create, update, and delete real virtual machines from YAML.

**Demo.** You write `web-01.yaml`, run `dae apply`, and a VM boots. **This is the
phase where the project becomes real.** Everything before it existed to make this
clean; everything after makes it safe and pleasant.

**Estimate.** 3–4 weeks. Async lands here, so budget generously.

**Rust you will learn.** `async`/`await`, `Future`, `tokio`, `Arc`, trait
objects, `#[async_trait]`, `reqwest`, retry and timeout patterns, `wiremock`.

---

## Before you start

You need a Proxmox host (a spare box, a NUC, or nested virtualisation on your
desktop). Get these working **by hand first**, with `curl`, before writing any
Rust:

```bash
# On the Proxmox host: a token for development
pveum user add daedalus@pve
pveum user token add daedalus@pve dev --privsep 0
pveum acl modify / --user daedalus@pve --role Administrator   # dev only!

# From your machine: prove it works
curl -k -H "Authorization: PVEAPIToken=daedalus@pve!dev=SECRET" \
  https://pve.home.arpa:8006/api2/json/version
```

⚠️ `--privsep 0` and `Administrator` are **development shortcuts**. Step 3.10
replaces them with scoped tokens. Do not ship this.

💡 Understanding the API by hand before automating it will save you days of
debugging code that was fine while your assumptions were wrong.

---

## Step 3.1 — Introduce async

Add `tokio` to `dae-cli` and `dae-proxmox`. Not to `dae-core` — the CI check from
Phase 0 will stop you.

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> { /* ... */ }
```

💡 The mental model that unlocks async Rust: an `async fn` returns a `Future`
that **does nothing until awaited**. `.await` yields to the runtime, letting other
tasks progress. `Send` bounds appear because tokio may move your task between
threads — which is why anything held across an `.await` must be `Send`.

The error you will hit within the first hour is *"future is not `Send`"*, usually
because a `std::sync::MutexGuard` or an `Rc` is alive across an `.await`. The fix
is nearly always to shorten the scope of the guard, or use `tokio::sync::Mutex`.

**Check:** `dae plan` still works, now on a tokio runtime.

---

## Step 3.2 — The HTTP client

```rust
// crates/dae-proxmox/src/client.rs

pub struct ProxmoxClient {
    http:     reqwest::Client,
    base_url: Url,               // https://pve.home.arpa:8006/api2/json
    auth:     ProxmoxAuth,
}

pub enum ProxmoxAuth {
    ApiToken { user: String, realm: String, token_id: String,
               secret: SecretString },
}

impl ProxmoxClient {
    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http.request(method, self.base_url.join(path).unwrap())
            .header(AUTHORIZATION, self.auth.header_value())
    }
}
```

Client configuration that matters:

```rust
reqwest::Client::builder()
    .timeout(Duration::from_secs(30))
    .connect_timeout(Duration::from_secs(5))
    .pool_idle_timeout(Duration::from_secs(90))
    // Homelab Proxmox usually has a self-signed cert. Support pinning
    // the CA rather than telling people to disable verification.
    .add_root_certificate(ca_cert)
    .build()?
```

⚠️ Offer `danger_accept_invalid_certs` **only** behind an explicit
`insecure_skip_tls_verify: true` in config, and log a warning every time it is
used. Making the insecure path convenient is how it becomes the default.

**Check:** `GET /version` returns the Proxmox version. Add an integration test
gated behind `#[cfg(feature = "integration-tests")]`.

---

## Step 3.3 — Response envelopes

Proxmox wraps everything in `{"data": ...}`:

```rust
#[derive(Deserialize)]
struct PveResponse<T> { data: T }

async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
    let resp = self.request(Method::GET, path).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        // ⚠️ Proxmox error bodies are inconsistent. Capture the raw text —
        // it is often the only clue you get.
        return Err(ProxmoxError::Api { status, body });
    }
    let parsed: PveResponse<T> = serde_json::from_str(&body)
        .map_err(|e| ProxmoxError::Decode { source: e, body })?;
    Ok(parsed.data)
}
```

💡 Always include the raw body in decode errors. Proxmox returns some numbers as
strings and some as numbers, inconsistently across endpoints, and you will need
to see the actual payload to work out which.

**Check:** a `wiremock` test for both a success and an error response.

---

## Step 3.4 — Task polling ⚠️

**Do this before anything else that mutates.** Most Proxmox write operations are
asynchronous: they return a **UPID** immediately and do the work in the
background.

```
UPID:node-01:00001A2B:0000C3D4:65F1A2B3:qmcreate:104:daedalus@pve!dev:
```

```rust
impl ProxmoxClient {
    /// Poll a task to completion. Every mutating call goes through this.
    pub async fn wait_for_task(&self, node: &str, upid: &Upid,
                               timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut backoff = Duration::from_millis(200);

        loop {
            let status: TaskStatus = self.get(
                &format!("/nodes/{node}/tasks/{upid}/status")).await?;

            if status.status == "stopped" {
                return match status.exitstatus.as_deref() {
                    Some("OK") => Ok(()),
                    Some(e)    => Err(ProxmoxError::TaskFailed {
                                        upid: upid.clone(), reason: e.into() }),
                    None       => Err(ProxmoxError::TaskFailed {
                                        upid: upid.clone(),
                                        reason: "no exit status".into() }),
                };
            }
            if Instant::now() > deadline {
                return Err(ProxmoxError::TaskTimeout { upid: upid.clone() });
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(5));
        }
    }
}
```

⚠️ If you skip this and treat writes as synchronous, everything will appear to
work in single-VM tests and fail intermittently the moment you create two VMs at
once. Retrofitting it across a finished provider is genuinely miserable. Build it
now, and make every mutating method call it.

Also fetch the task log on failure (`/nodes/{node}/tasks/{upid}/log`) — that is
where the actual error message lives.

**Check:** create a VM by hand via the API; your code polls the UPID and reports
success. Then create one that fails (e.g. a bad storage name) and confirm you
surface the real reason from the log.

---

## Step 3.5 — Observe

Read-only first. It is safe, and it immediately validates your tagging design.

```rust
pub async fn list_vms(&self) -> Result<Vec<ClusterResourceVm>> {
    // One call for the whole cluster — far better than iterating nodes.
    self.get("/cluster/resources?type=vm").await
}
```

Map Proxmox VMs into `ObservedMachine`, extracting Daedalus metadata from tags
and the description field:

```rust
fn parse_daedalus_metadata(vm: &ClusterResourceVm) -> Option<DaedalusTag> {
    let tags: HashSet<&str> = vm.tags.as_deref()
        .unwrap_or("").split(';').collect();
    if !tags.contains("daedalus") { return None; }
    // uid and generation live in the description, since Proxmox tags
    // are restricted to a limited character set.
    parse_description(vm.description.as_deref()?)
}
```

⚠️ Proxmox tags allow only `[a-z0-9_-]` (and are lowercased). Do not try to store
a ULID's case or a full key=value there. Tags are for *filtering*
(`tenant-acme`); the description holds the structured metadata.

**Check:** `dae get machines --from-provider` lists real VMs from your host,
distinguishing Daedalus-managed ones from the rest.

---

## Step 3.6 — Define the trait

Now that you have one concrete implementation working, extract the trait from it.

```rust
// crates/dae-provider/src/lib.rs
#[async_trait]
pub trait MachineProvider: Send + Sync {
    fn kind(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    async fn health(&self) -> Result<ProviderHealth>;
    async fn observe(&self, tenant: &Name) -> Result<Vec<ObservedMachine>>;
    async fn create(&self, tenant: &Name, spec: &ResolvedMachineSpec)
        -> Result<ProviderRef>;
    fn  update_strategy(&self, from: &ResolvedMachineSpec,
                        to: &ResolvedMachineSpec) -> UpdateStrategy;
    async fn update(&self, r: &ProviderRef, to: &ResolvedMachineSpec)
        -> Result<()>;
    async fn delete(&self, r: &ProviderRef, opts: DeleteOptions) -> Result<()>;
    async fn power(&self, r: &ProviderRef, op: PowerOp) -> Result<()>;
}
```

💡 **Extract the trait after the implementation, never before.** A trait designed
in the abstract encodes your guesses; one extracted from working code encodes
what the problem actually needs. This is why the plan does one provider first.

**Check:** `dae-proxmox` implements the trait; the CLI holds a
`Arc<dyn MachineProvider>` and no longer names Proxmox anywhere.

---

## Step 3.7 — Create a VM

Two strategies. Implement clone first — it is what you will use daily.

**Clone from a template (fast, seconds):**

```
POST /nodes/{node}/qemu/{template_vmid}/clone
     newid=104&name=web-01&full=1&storage=tenant-acme-ssd&pool=tenant-acme
```

**Create from scratch (slow, needs an install):** use only for building templates.

Then configure it:

```
PUT /nodes/{node}/qemu/104/config
    cores=4
    memory=16384                          # MiB, not bytes
    net0=virtio,bridge=vmbr0,tag=20
    scsi0=tenant-acme-ssd:40,discard=on
    ipconfig0=ip=10.20.10.11/24,gw=10.20.10.1
    ciuser=debian
    sshkeys=<URL-encoded>                 # ⚠️ double-encoded, see below
    tags=daedalus;tenant-acme;env-prod
    description=<metadata block>
    onboot=1
```

Three things that will cost you an evening each if you do not know them:

- **VMID allocation races.** `GET /cluster/nextid` then create — but another
  client may take it first. Catch the "already exists" error and retry with a
  fresh ID, up to a few attempts. Do not assume your read is still valid.
- **`sshkeys` is URL-encoded twice.** Once for the form body and once by Proxmox's
  own parameter handling. If your keys arrive mangled, this is why.
- **Memory is in MiB.** Your `ByteSize` is in bytes. Convert at exactly one place
  in the provider and write a test for it, or you will ship a VM with 16 GiB
  where you meant 16 MiB.

⚠️ **Tag before you finish.** If create succeeds but tagging fails, you have an
orphan VM that Daedalus cannot see. Set tags and description in the same
`PUT /config` call as everything else, and if that call fails, delete the clone.

**Check:** `dae apply` creates a real, running VM with the right CPU, RAM, disk,
network and IP. Then `dae plan` immediately after reports **no changes** — that
round-trip is the real test, and it will fail the first few times because your
observe and create disagree about some field.

---

## Step 3.8 — Update strategies

```rust
fn update_strategy(&self, from: &ResolvedMachineSpec, to: &ResolvedMachineSpec)
    -> UpdateStrategy
{
    // Destructive: the boot disk is replaced.
    if from.image != to.image { return UpdateStrategy::RequiresRecreate; }
    // Shrinking a disk destroys data; Proxmox refuses anyway.
    if to.disks.iter().any(|d| shrinks(from, d)) {
        return UpdateStrategy::Rejected("disks cannot shrink");
    }
    // Hot-pluggable if the guest supports it; safe to attempt.
    if from.memory != to.memory { return UpdateStrategy::InPlace; }
    // CPU count changes need a power cycle on most guests.
    if from.cpu.cores != to.cpu.cores { return UpdateStrategy::RequiresRestart; }
    UpdateStrategy::InPlace
}
```

💡 This is a **pure function** — no I/O, no async. Which means you can test every
transition exhaustively in milliseconds. Put it in `dae-provider` (or even
`dae-core`) and table-test it hard. This function is what stands between a user
and an accidentally destroyed VM.

**Check:** a table test covering every field, asserting the expected strategy.
Then change `memory` in YAML, apply, and confirm the VM resizes without a reboot.

---

## Step 3.9 — Delete, safely

```rust
pub struct DeleteOptions {
    pub purge_disks: bool,       // remove disks too
    pub keep_backups: bool,
    pub force: bool,             // ignore the protection flag
}
```

Layered safety, all of which must pass:

1. `metadata.annotations."daedalus.io/protected"` blocks deletion outright.
2. Proxmox's own `protection` flag is set on VMs Daedalus considers protected.
3. `dae apply` prints the full list and requires confirmation.
4. `--auto-approve` still refuses deletion unless `--allow-destroy` is also given.

⚠️ Add a `dae-*-test` VM name prefix convention to your dev workflow and never
point development at VMs you care about. You *will* delete something by accident
during this phase. Everyone does.

**Check:** remove a machine's YAML, plan, and see the delete. Confirm a protected
machine is refused with a clear message naming the annotation.

---

## Step 3.10 — Tenant-scoped credentials

Now retire the Administrator token and implement the real model
([multi-tenancy layer 6](../multitenancy.md#layer-6--infrastructure-)):

```bash
pveum pool add tenant-acme
pveum user add daedalus-acme@pve
pveum user token add daedalus-acme@pve dae --privsep 1
pveum acl modify /pool/tenant-acme --user daedalus-acme@pve --role PVEVMAdmin
pveum acl modify /storage/tenant-acme-ssd --user daedalus-acme@pve --role PVEDatastoreUser
```

Daedalus then holds two credentials per Proxmox cluster:

- a **platform** token, used only for tenant lifecycle (creating pools, users,
  ACLs)
- a **per-tenant** token, used for every routine operation on that tenant

```rust
/// Returns a client authenticated as the tenant, not as the platform.
async fn client_for(&self, tenant: &Name) -> Result<Arc<ProxmoxClient>>;
```

💡 **This is the payoff for choosing Proxmox.** With this in place, a bug in
Daedalus's own tenant filtering produces a `403` from the hypervisor rather than
a cross-tenant write. Test it deliberately: hard-code the wrong tenant in a call
and confirm Proxmox rejects it.

**Check:** using tenant A's token, attempt to read a VM in tenant B's pool.
Proxmox returns 403. Write this as an integration test — it is the single most
valuable test in the project.

---

## Step 3.11 — Test infrastructure

Two tiers, because you cannot run CI against real hardware:

```rust
// Tier 1: wiremock. Runs in CI, no infrastructure, fast.
#[tokio::test]
async fn create_vm_clones_from_template() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api2/json/nodes/node-01/qemu/9000/clone"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_json(json!({"data": "UPID:node-01:...:qmclone:104:..."})))
        .mount(&server).await;
    // ... assert the request body carried the right parameters
}

// Tier 2: real hardware, gated, run before releases.
#[tokio::test]
#[cfg(feature = "integration-tests")]
async fn full_vm_lifecycle() { /* create, verify, update, delete */ }
```

💡 Capture real Proxmox responses once (`curl | jq > fixtures/`) and replay them
in wiremock. Real fixtures catch the string-vs-number inconsistencies that
hand-written ones never will.

**Check:** `cargo nextest run` passes with no Proxmox available.

---

## Definition of done

- [ ] `dae apply` creates a real VM matching its YAML
- [ ] `dae plan` immediately afterwards reports **no changes**
- [ ] Update in place works for memory; recreate is detected for image changes
- [ ] Delete works and respects protection at all four layers
- [ ] `observe` finds and correctly classifies Daedalus-managed VMs
- [ ] A VM tagged by hand is **adopted**, not duplicated
- [ ] Tenant-scoped tokens in use; cross-tenant access returns 403 (tested)
- [ ] All mutating calls go through `wait_for_task`
- [ ] wiremock tests pass in CI with no infrastructure

## Pitfalls

- **Skipping task polling.** The single biggest source of intermittent failures.
- **MiB vs bytes.** Convert in one place, test it.
- **Assuming `nextid` is still free.** Handle the race.
- **Creating before tagging.** Orphans that Daedalus cannot see.
- **Trusting Proxmox's JSON types.** Some numbers arrive as strings. Use
  `#[serde(deserialize_with = ...)]` helpers, and keep raw bodies in errors.
- **Developing against VMs you care about.** Use a prefix; use a spare host.
- **Designing the trait before the implementation.** You will guess wrong.
- **Getting stuck on "future is not `Send`".** It is nearly always a
  non-`Send` guard held across an `.await`. Shorten the scope.
