# Tech stack

Rust, chosen partly to learn it. That is a legitimate reason, but it does shape
some choices below: where two crates are comparable, this picks the one that
teaches more useful Rust, and where a crate would mean fighting the language
instead of learning it, it picks the boring option.

> **Version note.** Pin versions when you start and check crates.io for current
> releases — this document names crates, not exact versions, because the numbers
> go stale. Run `cargo deny check` and `cargo audit` in CI from Phase 0.

---

## Workspace layout

```
daedalus/
├── Cargo.toml                # workspace root
├── crates/
│   ├── dae-core/             # domain types. NO I/O. no tokio, no sqlx, no reqwest
│   ├── dae-store/            # sqlx repositories + migrations (Ariadne)
│   ├── dae-git/              # repo sync, manifest loading, write-back
│   ├── dae-provider/         # the Provider traits + shared provider types
│   ├── dae-proxmox/          # Proxmox VE driver
│   ├── dae-k8s/              # cluster lifecycle: k3s, Talos, import
│   ├── dae-engine/           # Labyrinth graph, planner, reconciler
│   ├── dae-policy/           # Cedar integration + quota evaluation
│   ├── dae-api/              # axum router, handlers, OpenAPI
│   ├── dae-web/              # maud templates + htmx handlers
│   ├── dae-client/           # HTTP client, shared by CLI and tests
│   └── dae-cli/              # clap CLI (binary: dae)
├── bin/
│   └── daedalusd/            # server binary: wires everything together
└── xtask/                    # cargo xtask: migrations, codegen, release
```

**The one rule worth enforcing:** `dae-core` has no I/O dependencies. No tokio,
no sqlx, no reqwest. It is pure types and pure functions — the diff algorithm,
the merge semantics, the graph. This makes the interesting logic testable in
microseconds with no fixtures, and it forces a clean separation you will be
grateful for by Phase 5.

Add a CI check that fails if `dae-core`'s dependency tree gains `tokio`. It is
three lines of `cargo tree` and it will save the architecture.

---

## Crate choices

### Foundation

| Need | Crate | Why |
| --- | --- | --- |
| Async runtime | `tokio` | Full features. The ecosystem assumes it. |
| Errors (libraries) | `thiserror` | Typed, matchable errors per crate |
| Errors (binaries) | `anyhow` | Context chains where you just want to report |
| Diagnostics | `miette` | Source spans and pretty YAML errors — see [gitops.md](gitops.md#the-loading-pipeline) |
| Logging | `tracing` + `tracing-subscriber` | Structured spans, not lines. Essential for reconciliation. |
| IDs | `ulid` | Time-ordered, sortable, URL-safe. Better than UUIDv4 for DB index locality. |
| Time | `chrono` | Widest ecosystem/sqlx support. `jiff` is nicer if you can take the integration cost. |
| Config | `figment` | Layer TOML + env + CLI flags cleanly |
| Hashing | `blake3` | Fast spec hashing for the up-to-date check |

### Serialization

| Need | Crate | Note |
| --- | --- | --- |
| Core | `serde` | Obviously |
| JSON | `serde_json` | API, database columns |
| YAML | **check maintenance first** | `serde_yaml` was archived by its author in 2024. `serde_norway` and `serde_yaml_ng` are the maintained forks. Verify which is healthy when you start. |
| YAML round-trip | `saphyr` / `yaml-rust2` | CST-level editing that preserves comments, for UI write-back |
| Schema | `schemars` | Generate JSON Schema from your types → editor autocomplete for manifests, free |

`schemars` is an underrated win: derive it on your spec types, publish the
schemas, and every user gets validation and completion in VS Code via the YAML
extension. Very high value for very little work.

### Storage

| Need | Crate | Why |
| --- | --- | --- |
| Database | `sqlx` | Compile-time-checked SQL against a real schema. You will learn SQL *and* Rust rather than an ORM's DSL. |
| Migrations | `sqlx::migrate!` | Embedded in the binary; no separate tool to deploy |

SQLite by default (WAL mode, one file, trivially backed up), Postgres behind a
feature flag for anyone who wants it. Write the repository layer against a trait
from the start so the second backend is not a rewrite — but do not build the
Postgres path until someone needs it.

### HTTP

| Need | Crate | Why |
| --- | --- | --- |
| Server | `axum` | Tower middleware, excellent ergonomics, tokio-native |
| Middleware | `tower`, `tower-http` | Tracing, compression, timeouts, CORS, rate limiting |
| Client | `reqwest` | `rustls-tls`, not native-tls — no OpenSSL in your build |
| OpenAPI | `utoipa` + `utoipa-swagger-ui` | Derive the spec from handlers; keeps docs honest |
| WebSocket/SSE | `axum::extract::ws`, `axum::response::sse` | Live logs, VM console |

### Auth and policy

| Need | Crate | Why |
| --- | --- | --- |
| OIDC | `openidconnect` | Full, correct OIDC. Do not hand-roll this. |
| JWT | `jsonwebtoken` | Session tokens |
| Passwords | `argon2` | For the local-account fallback only |
| Policy | `cedar-policy` | Rust-native, formally analysable, `forbid` overrides `permit` |
| Secret handling | `secrecy` + `zeroize` | Redacted `Debug`, zeroed on drop |
| Encryption | `age` | Tenant secret encryption, SOPS-compatible |

### Providers

| Need | Crate |
| --- | --- |
| Proxmox | Hand-rolled on `reqwest` — third-party crates here are thin and often stale |
| Kubernetes | `kube` + `k8s-openapi` |
| SSH | `russh` (pure Rust) or `openssh` (wraps the binary; simpler, needs ssh installed) |
| Templating | `minijinja` — sandboxed, Jinja2-compatible, for cloud-init only |

### Web UI

| Need | Crate | Why |
| --- | --- | --- |
| Templating | `maud` | Compile-time-checked HTML in a macro. Typos are build errors; auto-escaping by default. |
| Interactivity | htmx (vendored JS file) | ~14KB, no build step, no node_modules |
| Assets | `rust-embed` | Compile CSS/JS/htmx into the binary. Single-file deployment. |
| CSS | Hand-written, with custom properties | A dashboard is tables, forms, and status badges. Tailwind means a node toolchain for very little gain here. |

The whole UI compiles with `cargo build`. No npm, no bundler, no lockfile drift,
no "works on my machine". For a homelab dashboard this is unambiguously the right
trade — and if you later want a rich interactive view, you can add Leptos for
that one page without rewriting anything.

### Observability

| Need | Crate |
| --- | --- |
| Metrics | `metrics` + `metrics-exporter-prometheus` |
| Tracing export | `tracing-opentelemetry` + `opentelemetry-otlp` |
| Health | Hand-rolled `/healthz` and `/readyz` |

### Testing

| Need | Crate | Use for |
| --- | --- | --- |
| Snapshots | `insta` | Plan output, CLI output, rendered HTML. Superb for a diff engine. |
| HTTP mocking | `wiremock` | Recorded Proxmox fixtures; CI needs no hardware |
| Containers | `testcontainers` | Postgres and Gitea integration tests |
| Property tests | `proptest` | The diff and merge functions — exactly the shape that rewards it |
| HTTP assertions | `reqwest` + `axum-test` | API endpoint tests |

`insta` deserves emphasis. Your planner's output is a big structured diff.
Snapshot-testing it means you see *exactly* what changed in a plan when you touch
the algorithm, reviewed with `cargo insta review`. It turns "did I break the
planner?" from a worry into a diff.

---

## Rust learning path, mapped to phases

The phases are ordered so that each introduces a manageable amount of new Rust.

| Phase | New Rust you will meet |
| --- | --- |
| 0 | Cargo workspaces, modules, `Result`, `?`, `clippy`, `rustfmt` |
| 1 | Traits, generics, `serde` derive, newtypes, `thiserror`, unit tests |
| 2 | Lifetimes in earnest, iterators, error aggregation, borrowck arguments |
| 3 | `async`/`await`, `Future`, `Arc`, trait objects, `#[async_trait]` |
| 4 | Graph algorithms, `tokio` concurrency, channels, backoff, cancellation |
| 5 | Tower middleware, extractors, **typestate** (`TenantScope`), `Send`/`Sync` |
| 6 | Macros (`maud!`), `rust-embed`, streaming responses |
| 7 | Complex async orchestration, timeouts, retries, state machines |
| 8 | FFI or protocol work, cryptography APIs, careful `Drop` |
| 9 | Performance profiling, `unsafe` review, release engineering |

Two deliberate sequencing decisions:

- **Async is deferred to Phase 3.** Learning ownership and borrowing while also
  learning `Pin`, `Send` bounds and lifetime errors in futures is how people
  bounce off Rust. Phases 1–2 are synchronous and give you a real feel for the
  ownership model first.
- **The typestate pattern lands in Phase 5**, where it secures multi-tenancy.
  By then you will have enough type-system fluency for it to feel like a tool
  rather than a trick — and it is the moment Rust's value proposition for this
  project becomes concrete.

---

## Things to avoid

- **An ORM (Diesel/SeaORM).** `sqlx` teaches you SQL, which you need anyway, and
  compile-time query checking gives you most of the safety without the DSL.
- **`unwrap()` outside tests.** Add `#![deny(clippy::unwrap_used)]` in Phase 0,
  before it is annoying to retrofit.
- **`Rc<RefCell<T>>` reaching for familiarity.** If you need it in this codebase,
  the design is usually wrong. Prefer passing ownership, or `Arc` for genuine
  sharing across tasks.
- **Premature `unsafe`.** There is no reason for a single `unsafe` block in this
  project. If you write one, something has gone sideways.
- **Generics before you need them.** Write the concrete type first, extract the
  trait when the second implementation arrives. Phase 3 does exactly this with
  the Proxmox provider.
- **`Box<dyn Error>` in library crates.** Use `thiserror`, so callers can match.
