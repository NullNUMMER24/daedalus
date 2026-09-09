# Implementation plan

Ten phase documents, in order. Each contains:

- **Goal** — one sentence
- **Demo** — what you can show when it is done
- **Rust you will learn** — the new concepts this phase introduces
- **Steps** — numbered and small, each with an acceptance check
- **Definition of done** — the checklist to move on
- **Pitfalls** — the things that will actually go wrong

## How to use this

**Work top to bottom.** The ordering is deliberate: it front-loads Rust
fundamentals and defers async until you can handle it.

**Do not skip the acceptance checks.** They are what turn "I wrote some code"
into "this works". Most are a single command.

**Commit at every numbered step.** Small commits give you a working state to
return to when an experiment fails, which it will.

**Treat estimates as ranges, not promises.** They assume evenings and weekends
while learning the language. If a step takes three times as long, that is
information about the step, not about you.

**Change the plan.** By Phase 3 you will know things that make some of Phase 5
wrong. Update these documents as you go — a stale plan is worse than no plan.

## Conventions

- `$` — a shell command you run
- **Check:** — the acceptance criterion for a step
- ⚠️ — something that will bite you if you skip it
- 💡 — a Rust learning note

## Progress

- [ ] Phase 0 — Foundations
- [ ] Phase 1 — Core model
- [ ] Phase 2 — Git integration
- [ ] Phase 3 — Proxmox provider
- [ ] Phase 4 — Engine
- [ ] Phase 5 — API, auth, tenancy
- [ ] Phase 6 — Web UI
- [ ] Phase 7 — Kubernetes
- [ ] Phase 8 — Networking, secrets, providers
- [ ] Phase 9 — Hardening and release
