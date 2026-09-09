# Git as the source of truth

Git holds **desired state**. Nothing reaches a provider that is not in a commit.
That single rule buys you version history, code review, rollback, offline
editing, and a complete answer to "why is this VM here?" — for free, using tools
you already know.

---

## Which repository layout?

Two supported models. You can mix them: run the monorepo for your own tenants and
give a friend their own repo.

### A. Monorepo (start here)

One repository, all tenants. Simplest to operate; isolation is enforced by
Daedalus rather than by Git.

```
homelab/
├── daedalus.yaml                 # repo-level config, schema version
├── platform/                     # platform-admin only
│   ├── providers/
│   │   ├── pve-main.yaml
│   │   └── pve-edge.yaml
│   ├── policies/
│   │   ├── base.cedar
│   │   └── production-guardrails.cedar
│   └── quotas/
│       └── templates.yaml        # small / medium / large
├── catalog/                      # curated, shared, read-only to tenants
│   ├── images/
│   │   ├── debian-12-cloud.yaml
│   │   └── talos-1.9.yaml
│   └── machine-classes/
│       ├── small-2x4.yaml
│       └── standard-4x8.yaml
└── tenants/
    ├── acme/
    │   ├── tenant.yaml           # metadata; quota is platform-owned
    │   ├── environments/
    │   │   ├── prod/
    │   │   │   ├── networks/prod-net.yaml
    │   │   │   ├── machines/web-01.yaml
    │   │   │   ├── machines/db-01.yaml
    │   │   │   └── clusters/prod.yaml
    │   │   └── staging/
    │   │       └── machines/web-01.yaml
    │   └── secrets/
    │       └── prod.sops.yaml
    └── bob/
        └── ...
```

### B. Repo per tenant (stronger)

The tenant owns their repository entirely; Daedalus is given a read-only deploy
key plus write access to a `daedalus/*` branch namespace for UI write-back.

```
acme-infra/
├── daedalus.yaml        # declares: tenant: acme
├── environments/
└── secrets/
```

The tenant registers it:

```bash
dae tenant repo set acme --url git@github.com:acme/infra.git --branch main
```

**Trade-off:** repo-per-tenant is genuinely stronger — a Git ACL mistake cannot
leak another tenant's manifests, and tenants can use their own CI. It costs you N
repositories to keep track of, and shared catalog references have to be resolved
from the platform repo rather than a relative path.

**Recommendation:** build the loader so the *tenant root* is an abstraction from
day one. Then both layouts are the same code path, and you never have to migrate.

---

## Manifest format

Deliberately Kubernetes-shaped. You already know how to read it, and it gives
`get`/`describe`/`apply` an obvious meaning.

```yaml
apiVersion: daedalus.io/v1alpha1
kind: Machine
metadata:
  name: web-01
  # tenant and environment are inferred from the file's path and may be
  # omitted; if present they must match, which catches copy-paste mistakes.
  tenant: acme
  environment: prod
  labels:
    role: web
    tier: frontend
  annotations:
    daedalus.io/protected: "true"          # refuse deletion
    daedalus.io/drift-policy: report        # report | correct | ignore
spec:
  class: standard-4x8                       # from the catalog
  provider: pve-main
  image: debian-12-cloud

  # Explicit fields override the class.
  memory: 16Gi

  disks:
    - name: root
      size: 40Gi
      storage: tenant-acme-ssd
      discard: true
    - name: data
      size: 200Gi
      storage: tenant-acme-hdd

  networks:
    - ref: prod-net
      ipam: static
      address: 10.20.10.11/24
      gateway: 10.20.10.1

  cloudInit:
    hostname: web-01
    sshKeysFrom:
      - secretRef: acme-ssh-keys
    userDataFrom:
      secretRef: web-01-userdata

  lifecycle:
    startOnBoot: true
    protection: true          # hypervisor-level delete protection too

  dependsOn:
    - Network/prod-net
```

### Every kind shares this shape

`apiVersion`, `kind`, `metadata`, `spec`. Status is never written in Git — it is
observed, and putting it in a manifest would be a category error.

### Composition, not templating

Prefer `MachineClass` composition over string templating. Templating YAML is how
configuration becomes unreadable.

```yaml
# catalog/machine-classes/standard-4x8.yaml
apiVersion: daedalus.io/v1alpha1
kind: MachineClass
metadata:
  name: standard-4x8
spec:
  cpu: { cores: 4, type: host }
  memory: 8Gi
  disks:
    - name: root
      size: 40Gi
  defaults:
    lifecycle: { startOnBoot: true }
```

Merge semantics: the class provides defaults; the machine's own fields override
them; lists merge by the `name` key rather than being replaced wholesale.

A constrained [minijinja](https://docs.rs/minijinja/) pass is available for
cloud-init user-data only, where genuine templating is unavoidable. It runs in a
sandbox with no filesystem, no environment, and no arbitrary function calls.

### Generators, for when you really do need N of something

```yaml
apiVersion: daedalus.io/v1alpha1
kind: MachineSet
metadata:
  name: worker
spec:
  replicas: 3
  nameTemplate: "worker-{{ index }}"     # worker-01, worker-02, worker-03
  template:
    spec:
      class: standard-4x8
      networks: [{ ref: prod-net, ipam: pool }]
```

Expanded at load time into individual `Machine` resources, which then plan and
apply exactly like hand-written ones. `MachineSet` members are still individually
addressable and individually protectable.

---

## The loading pipeline

```
walk files → parse YAML → identify kind → deserialize to typed struct
    → infer tenant/env from path → validate schema → expand generators
    → merge classes → resolve references → build graph → detect cycles
```

Errors are collected, not fatal-on-first — you want to see all twelve mistakes,
with file and line, not one at a time:

```
error: unknown field `memroy`
  --> tenants/acme/environments/prod/machines/web-01.yaml:14:5
   |
14 |     memroy: 16Gi
   |     ^^^^^^ did you mean `memory`?

error: unresolved reference
  --> tenants/acme/environments/prod/machines/web-01.yaml:28:12
   |
28 |     - ref: prod-nett
   |            ^^^^^^^^^ no Network named `prod-nett` in tenant `acme`
   |
   = note: available: prod-net, mgmt-net
```

Good diagnostics are not polish. They are the difference between a tool you enjoy
and a tool you tolerate, and `serde` plus `miette`/`ariadne`-style span reporting
gets you most of the way for modest effort.

---

## Write-back: how the web UI stays honest

If the UI could apply changes directly, Git would immediately stop being the
source of truth and you would have built a worse Proxmox UI. So **UI mutations
become commits**:

```
User edits web-01's memory in the browser
        │
        ▼
Server renders the updated manifest (preserving comments and key order)
        │
        ▼
Commit on branch  daedalus/acme/01J8ZQ4M
        │
        ├── tenant policy = auto-merge  ──► fast-forward main, reconcile
        │
        └── tenant policy = require-review ──► open a PR, wait for approval
```

Commit message:

```
[daedalus] acme/prod: set Machine/web-01 memory to 16Gi

Changed via web UI.

Daedalus-Principal: user:01J8ZQ4M7X (cj)
Daedalus-Tenant: acme
Daedalus-Change-Id: 01J8ZQ4M7XABCDEF
Daedalus-Correlation-Id: 01J8ZQ4M7XABCDEF
```

Trailers make the audit log and Git history cross-referenceable in both
directions — a genuinely useful property when something goes wrong at 2am.

### Round-tripping YAML without destroying it

Naive `serde` round-tripping strips comments and reorders keys, which makes the
diff unreadable and makes people stop trusting the tool. Use a **format-preserving
edit**: parse to a CST, mutate only the target node, serialise back.
`yaml-rust2`/`saphyr` support this; a surgical line-range replacement is an
acceptable v1 shortcut for the narrow set of fields the UI can edit.

Write a round-trip property test early: *load → edit one field → dump* must
change exactly one line. It catches an entire category of "the UI mangled my
file" bugs.

### Conflicts

If `main` moved since the UI read the file, do not force-push. Rebase the change;
on conflict, surface it to the user with both versions and let them choose. The
UI is a Git client, and Git clients handle conflicts.

---

## Change detection

- **Webhook** (preferred) — GitHub/Gitea/Forgejo POST to `/api/v1/hooks/git`,
  HMAC-verified, reconcile within seconds.
- **Polling** (fallback, always on) — `git fetch` every 60s and compare HEAD.
  Works behind NAT with no inbound access, which is most homelabs.

On a change, Daedalus diffs the commit range to work out which tenants and which
resources are affected, and enqueues only those. Editing one VM should not
re-plan the entire lab.

### Commit signing

Optionally require signed commits per tenant:

```yaml
# tenant.yaml
spec:
  git:
    requireSignedCommits: true
    allowedSigners:
      - "cj <mil_jro@proton.me> ssh-ed25519 AAAAC3Nza..."
```

Unsigned commits are then rejected at load with a clear error rather than
silently applied. Worth enabling for production tenants.

---

## Bootstrapping

The platform repo describes the platform, including Daedalus itself. Chicken and
egg: to reconcile the repo you need a running `daedalusd`, which needs config.

Resolution: `daedalusd` reads a **small local config file** (database URL, listen
address, master key source, the platform repo URL, and one bootstrap admin
credential) — and *everything else* comes from Git. Keep that file under twenty
lines. If it grows, you are leaking configuration out of Git.

```toml
# /etc/daedalus/config.toml
[server]
listen = "0.0.0.0:8443"
external_url = "https://dae.home.arpa"

[database]
url = "sqlite:///var/lib/daedalus/daedalus.db"

[secrets]
master_key_file = "/etc/daedalus/master.key"   # 0600

[git]
platform_repo = "git@github.com:cj/homelab.git"
branch = "main"
ssh_key_file = "/etc/daedalus/git.key"
```
