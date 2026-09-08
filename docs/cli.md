# CLI

`dae` is a thin client over the HTTP API. Verbs are deliberately Kubernetes-shaped
so muscle memory transfers.

## Setup

```bash
dae login --server https://dae.home.arpa      # opens a browser for OIDC
dae ctx list
dae ctx use acme/prod                          # tenant/environment
dae ctx show
```

Config lives at `~/.config/daedalus/config.toml`; tokens go to the OS keyring
(`keyring` crate) with a file fallback at `0600`.

## Reading

```bash
dae get machines                      # in the current context
dae get machines --all-environments
dae get machine web-01
dae get machine web-01 -o yaml        # -o table|json|yaml|wide|name
dae get machines -l role=web          # label selector
dae describe machine web-01           # spec + status + recent events + drift
dae events --resource machine/web-01 --since 1h
dae graph --format dot | dot -Tpng > lab.png     # render the Labyrinth
```

Default output is an aligned table:

```
NAME     CLASS          STATUS    IP             AGE   DRIFT
web-01   standard-4x8   Running   10.20.10.11    12d   -
db-01    standard-8x32  Running   10.20.10.12    12d   memory
wk-01    standard-4x8   Stopped   10.20.10.20    3d    -
```

## Changing

```bash
dae validate ./                       # offline: parse + schema + refs + cycles
dae plan                              # against the API; read-only
dae plan --out plan.json              # save for later approval
dae apply                             # requires a fresh plan
dae apply --plan plan.json
dae apply --auto-approve              # still refuses RequiresRecreate
dae diff machine/web-01               # desired vs observed for one resource
```

Plan output puts the dangerous things where you cannot miss them:

```
Plan: 2 to create, 1 to update, 0 to destroy.

  + Machine/acme/prod/web-02        standard-4x8, 10.20.10.13
  + Machine/acme/prod/web-03        standard-4x8, 10.20.10.14
  ~ Machine/acme/prod/db-01
      spec.memory: 16Gi -> 32Gi     (in place, no restart)

Quota after apply:  vcpu 20/48   memory 72Gi/128Gi   machines 6/20
```

and when something is destructive:

```
  ! Machine/acme/prod/web-01        RECREATE REQUIRED
      spec.image: debian-12 -> debian-13
      This destroys the machine and its root disk. Data on `data` is preserved.
      To proceed:  dae apply --confirm-recreate Machine/web-01
```

## Day-to-day operations

```bash
dae start web-01
dae stop web-01 --graceful
dae restart web-01
dae console web-01                    # serial console over WebSocket
dae ssh web-01                        # resolves the IP, uses your key
dae logs web-01 --follow              # cloud-init and agent logs
dae snapshot create web-01 pre-upgrade
dae snapshot list web-01
dae drift                             # everything diverged in this context
```

## Kubernetes

```bash
dae cluster list
dae cluster create prod --nodes 3 --version v1.31.4 --class standard-4x8
dae cluster scale prod --pool workers --replicas 5
dae cluster upgrade prod --version v1.32.0
dae kubeconfig prod                          # to stdout
dae kubeconfig prod --merge                  # into ~/.kube/config
dae cluster import legacy --kubeconfig ./kc  # adopt an existing cluster
```

## Tenant administration

```bash
dae tenant list
dae tenant create acme --display "Acme Ltd" --quota-template medium
dae tenant show acme
dae tenant member add acme cj --role tenant-owner
dae tenant repo set acme --url git@github.com:acme/infra.git
dae tenant suspend acme
dae quota show
dae quota set acme --vcpu 64
dae audit --tenant acme --since 24h
dae policy test ./platform/policies/
```

## Secrets

```bash
dae secret set db-password --from-stdin
dae secret set ssh-keys --from-file ~/.ssh/id_ed25519.pub
dae secret list                       # names and versions only, never values
dae secret rotate-key acme            # re-encrypt everything to a new age key
```

Values are never printed. `dae secret get` does not exist, on purpose — if you
need the value, it is in your password manager, not your infrastructure tool.

## Conventions

**Exit codes** — designed for CI:

| Code | Meaning |
| --- | --- |
| 0 | Success; for `plan`, no changes |
| 1 | Error |
| 2 | Usage error |
| 3 | `plan` found changes (so CI can gate on it) |
| 4 | Policy or quota denial |

**Global flags:** `--context`, `--tenant`, `--output/-o`, `--no-color`,
`--verbose/-v`, `--quiet/-q`, `--yes`, `--timeout`.

**Behaviour:**

- Respect `NO_COLOR` and detect whether stdout is a TTY.
- Machine-readable output (`-o json`) goes to stdout; progress and prompts to
  stderr. This makes `dae get machines -o json | jq` work in a pipeline.
- Every destructive command prompts unless `--yes`, and the prompt names the
  resource.
- `dae completions bash|zsh|fish|powershell` via `clap_complete`.
- `dae --version` reports the version, the git SHA, and the API version it
  negotiated with the server.

## A note on `--local`

`dae validate` and `dae plan --local` work without a server, reading the repo
directly. This is for pre-commit hooks and CI. `--local` **cannot apply** —
applying requires the server, because that is where authorisation, quota, and
audit live. Keeping that boundary sharp is what stops the CLI from becoming a
second, unenforced code path.
