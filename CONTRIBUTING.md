# Contributing

Daedalus is early: the design is in [docs/](docs/) and the build follows the
[implementation plan](docs/plan/), one phase at a time.

## Setup

```bash
nix develop
```

That is the whole setup — compiler, tools and shell. See
[docs/development.md](docs/development.md). Without Nix, install `rustup`
(`rust-toolchain.toml` selects the version), `just`, `cargo-nextest` and
`cargo-deny`.

## Before you push

```bash
just check
```

Formatting, clippy with warnings as errors, the tests, and the rule that
`dae-core` has no I/O dependencies. CI runs exactly this.

## Ground rules

- **No `unwrap()` outside tests.** It is a compile error. Use `?`, or
  `#[expect(clippy::…, reason = "…")]` when there is a real reason.
- **`dae-core` stays pure.** No tokio, sqlx, reqwest, axum or git2 — see
  [D-012](docs/decisions.md#d-012--dae-core-has-no-io-dependencies).
- **stdout is for output, stderr is for logs.** Scripts parse stdout.
- **Change the plan when reality disagrees.** Record departures in the phase's
  *As built* section rather than silently doing something else.
- **Decisions get written down.** Anything architectural goes in
  [docs/decisions.md](docs/decisions.md) with its reasoning.

## Commits

Small and focused, with a prefix: `build:`, `ci:`, `docs:`, `feat:`, `fix:`,
`test:`, `refactor:`. Say *why* in the body — the diff already shows what.
