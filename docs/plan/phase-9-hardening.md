# Phase 9 — Hardening and release

**Goal.** Turn a working project into software someone else can install, run, and
trust.

**Demo.** A stranger follows your README and has Daedalus managing a VM within
thirty minutes.

**Estimate.** 2–3 weeks.

**Rust you will learn.** Profiling, release engineering, cross-compilation,
supply-chain tooling.

---

## Step 9.1 — Backup and restore

The database holds recorded state and the audit log. Losing it is survivable
(see [D-004](../decisions.md#d-004--provider-objects-are-tagged-recorded-state-is-rebuildable))
but you should not have to rely on that.

```bash
dae admin backup --out /backups/dae-$(date +%F).tar.zst
dae admin restore --from /backups/dae-2026-09-08.tar.zst
dae admin reindex          # rebuild recorded state by scanning providers
```

A backup contains: the database (via `VACUUM INTO`, not a file copy of a live
WAL database), the sealed secret store, and the config. **Not** the Git repos —
those are already replicated by being Git.

⚠️ `dae admin reindex` is the feature that makes D-004 true rather than
aspirational. Test it properly: delete the database entirely, run reindex, and
confirm the next `plan` reports **no changes**. If it wants to recreate
everything, your tagging is broken and you have been carrying a false sense of
safety.

**Check:** delete the database, restore from backup, `plan` reports no changes.
Then delete it again, `reindex` instead, and get the same result.

---

## Step 9.2 — Upgrades

- **Database migrations** run automatically on start, forward-only, inside a
  transaction, with a backup taken first.
- **API versioning**: `/api/v1` is stable. Breaking changes get `/api/v2`, with
  `v1` supported for at least one minor release.
- **Manifest versioning**: `v1alpha1` may break; document every change and ship a
  `dae migrate manifests` converter.
- **CLI/server skew**: the CLI reports the API version it negotiated and warns on
  mismatch rather than failing mysteriously.

⚠️ Test the upgrade path, not just the fresh install. Keep a fixture database at
each released schema version and run the migrations against all of them in CI.
Fresh-install-only testing is how projects ship migrations that destroy data.

**Check:** a database from the previous release upgrades cleanly, with the backup
present afterwards.

---

## Step 9.3 — Optional high availability

SQLite has one writer, so HA means Postgres.

- Leader election via a Postgres advisory lock, with a lease and heartbeat.
- Only the leader runs the control loops. Followers serve read traffic.
- Failover in under 30 seconds.

⚠️ Be honest about whether you need this. A homelab control plane being down for
five minutes is not an outage of the homelab — the VMs keep running, the clusters
keep serving. **Restartability matters far more than availability here.** Build
HA because you want to learn distributed coordination, not because the use case
demands it.

**Check:** kill the leader; a follower takes over within 30s and no action runs
twice.

---

## Step 9.4 — Performance

Measure before optimising. Realistic homelab scale is 50–200 resources, which is
small — the likely problems are algorithmic, not constant-factor.

```bash
cargo install flamegraph
cargo flamegraph --bin daedalusd -- serve
```

Where problems actually appear:

- **N+1 queries** in list endpoints — the most likely real issue
- **Unbatched provider calls** — one HTTP request per VM instead of one for all
- **Graph rebuild on every reconcile** — cache it, keyed by Git commit
- **Full re-observe when one resource changed**

Add `criterion` benchmarks for `diff()` and graph construction with 1000
resources. They are pure functions, so they benchmark cleanly.

**Check:** a plan over 200 resources completes in under two seconds.

---

## Step 9.5 — Observability

```
# Prometheus metrics
daedalus_reconcile_duration_seconds{tenant,result}
daedalus_actions_total{tenant,kind,action,result}
daedalus_resources{tenant,kind,phase}
daedalus_provider_requests_total{provider,endpoint,status}
daedalus_provider_request_duration_seconds{provider,endpoint}
daedalus_drift_detected_total{tenant,kind}
daedalus_quota_usage_ratio{tenant,dimension}
daedalus_api_requests_total{method,route,status}
```

⚠️ **Never put a resource name or UID in a metric label.** That is unbounded
cardinality and it will eventually take down your Prometheus. Tenant and kind are
bounded; names are not.

Ship a Grafana dashboard JSON in the repo. Export traces via OTLP.

**Check:** `/metrics` scrapes cleanly; the dashboard imports and shows real data.

---

## Step 9.6 — Security review

Work through this list deliberately, not as a formality:

- [ ] `cargo audit` and `cargo deny` clean, running in CI
- [ ] No secrets in logs — grep the test suite output for known test secrets
- [ ] All eight isolation tests from Phase 5 passing, none `#[ignore]`d
- [ ] TLS by default; HTTP only with an explicit `--insecure` and a warning
- [ ] Security headers: HSTS, `X-Content-Type-Options`, CSP, `X-Frame-Options`
- [ ] Rate limiting on auth endpoints (`tower_governor`)
- [ ] Session cookies `HttpOnly; Secure; SameSite=Lax`
- [ ] CSRF on every mutating form
- [ ] Constant-time token comparison
- [ ] No `unsafe` anywhere (`unsafe_code = "forbid"` from Phase 0)
- [ ] Dependency licences reviewed
- [ ] `SECURITY.md` with a disclosure contact and expected response time

💡 Run the `/security-review` skill on the diff before release, and consider
having someone else read the auth and tenancy code. You have been staring at it
for six months; you cannot see it any more.

**Check:** every box ticked, with the evidence recorded somewhere.

---

## Step 9.7 — Documentation

Written for someone who is not you:

| Document | Purpose |
| --- | --- |
| **Quickstart** | Zero to a running VM in 30 minutes |
| **Installation** | Binary, Docker, systemd unit, reverse proxy |
| **Configuration** | Every option, with defaults and examples |
| **Tenant guide** | For your tenants, not for you |
| **Operations** | Backup, restore, upgrade, troubleshoot |
| **Manifest reference** | Every kind, every field — generate from `schemars` |
| **API reference** | From OpenAPI |
| **Troubleshooting** | Real error messages and what they mean |

💡 Generate the manifest reference from your `schemars` schemas. Hand-written
field documentation goes stale within one release, without fail.

⚠️ Have someone else follow the quickstart on a clean machine. You have too much
implicit context to test it yourself — you will skip a step without noticing that
you skipped it.

**Check:** a person who has not seen the project gets to a running VM by
following the quickstart alone, without asking you anything.

---

## Step 9.8 — Packaging

```yaml
# .github/workflows/release.yml — on tag
targets:
  - x86_64-unknown-linux-gnu
  - x86_64-unknown-linux-musl      # static; runs anywhere
  - aarch64-unknown-linux-gnu      # Raspberry Pi, Ampere
  - aarch64-unknown-linux-musl
artifacts:
  - dae-{version}-{target}.tar.gz
  - daedalusd-{version}-{target}.tar.gz
  - SHA256SUMS
  - container image (ghcr.io), multi-arch
```

Use `cargo-dist` to generate the release workflow and an installer script. Ship:

- a systemd unit with hardening (`ProtectSystem=strict`, `NoNewPrivileges`,
  `PrivateTmp`, a dedicated user)
- a `docker-compose.yml` for the quickstart
- shell completions for bash, zsh, fish, powershell
- a man page (`clap_mangen`)

**Check:** `curl -sSf https://.../install.sh | sh` works on a clean Debian box
and on an ARM machine.

---

## Step 9.9 — Release v0.1.0

- [ ] `CHANGELOG.md` in Keep-a-Changelog format
- [ ] Versioning policy stated: `0.x` may break; document every break
- [ ] `SECURITY.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`
- [ ] Issue and PR templates
- [ ] Tag, build, publish, write release notes
- [ ] Screenshots or a short demo recording in the README

💡 Be explicit that `0.x` is not stable. It sets expectations correctly and buys
you the freedom to fix the design mistakes you have found by now.

---

## Step 9.10 — Look back

Write a retrospective. Genuinely useful, and it makes the next project better.

- Which decisions in [decisions.md](../decisions.md) proved right? Which were
  wrong? Add a `**Revisited:**` note to each rather than editing the original —
  the reasoning is the valuable part.
- Where did you spend far more time than expected?
- What Rust did you actually learn? (Compare against the table in
  [tech-stack.md](../tech-stack.md#rust-learning-path-mapped-to-phases).)
- What would you cut if starting again?

---

## Definition of done

- [ ] Backup, restore, and reindex all verified against a deleted database
- [ ] Upgrade path tested from the previous release's schema
- [ ] Metrics, traces, and a Grafana dashboard
- [ ] Security checklist fully worked through
- [ ] Documentation validated by someone else on a clean machine
- [ ] Multi-arch binaries and container images published
- [ ] v0.1.0 tagged and released
- [ ] Retrospective written

## Pitfalls

- **Never testing restore.** An untested backup is not a backup.
- **Testing only fresh installs.** Migrations break on real data, not empty
  databases.
- **Unbounded metric cardinality.** Resource names as labels will kill Prometheus.
- **Writing docs for yourself.** You have context nobody else has.
- **Building HA because it sounds professional.** Restartability matters more here.
- **Claiming 1.0 too early.** `0.x` is honest and gives you room to fix things.
