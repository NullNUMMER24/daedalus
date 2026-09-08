# Roadmap

Ten phases. Each one ends with something you can actually demonstrate — that
matters more than it sounds, because a project like this dies in the phase where
nothing visible happens for six weeks.

Estimates assume evenings and weekends, and assume you are learning Rust as you
go. Halve them if you already knew Rust; do not be alarmed if Phase 3 takes twice
as long, because it is where async lands.

| Phase | Name | You can demo | Est. |
| --- | --- | --- | --- |
| **0** | [Foundations](plan/phase-0-foundations.md) | `dae version`, CI green, a repo you enjoy working in | 1 week |
| **1** | [Core model](plan/phase-1-core-model.md) | `dae validate` on a folder of YAML, with beautiful errors | 2–3 weeks |
| **2** | [Git integration](plan/phase-2-git.md) | `dae plan` showing a diff between Git and a database | 1–2 weeks |
| **3** | [Proxmox provider](plan/phase-3-proxmox.md) | **A real VM created from a YAML file** | 3–4 weeks |
| **4** | [Engine](plan/phase-4-engine.md) | Dependency-ordered apply, drift detection, retries | 2–3 weeks |
| **5** | [API, auth, tenancy](plan/phase-5-api-auth.md) | Two users, two tenants, provably isolated | 3–4 weeks |
| **6** | [Web UI](plan/phase-6-web-ui.md) | A non-technical person creating a VM in a browser | 2–3 weeks |
| **7** | [Kubernetes](plan/phase-7-kubernetes.md) | `dae cluster create` producing a working kubeconfig | 3–4 weeks |
| **8** | [Networking, secrets, providers](plan/phase-8-networking-secrets.md) | VLAN-per-tenant, IPAM, SOPS secrets, a second provider | 3–4 weeks |
| **9** | [Hardening and release](plan/phase-9-hardening.md) | v0.1.0, installable by someone else | 2–3 weeks |

Roughly six months of steady part-time work to v0.1.0. **Phase 3 is the one that
matters** — it is where the project stops being an exercise and starts being a
tool. Everything before it exists to make Phase 3 clean.

## The shape of the plan

```
  0 ──► 1 ──► 2 ──► 3 ──► 4 ──► 5 ──► 6 ──► 7 ──► 8 ──► 9
  │     │     │     │     │     │     │     │     │     │
  set   the   git   IT    make  make  make  k8s   the   ship
  up    types      WORKS  it    it    it          rest
                          reli- safe  nice
                          able
```

Phases 1 and 2 are deliberately **synchronous Rust** — no `async` anywhere. You
will learn ownership, borrowing, and traits without simultaneously fighting
`Pin`, `Send` bounds, and lifetime errors inside futures. Async arrives in
Phase 3, once the fundamentals are comfortable. This is the single most important
sequencing decision in the plan.

## Cut lines

If you want something usable sooner, these are the honest places to stop:

- **After Phase 4** you have a working single-tenant GitOps VM manager driven from
  a CLI. Genuinely useful. Many people would stop here.
- **After Phase 6** you have the full product for your own use — CLI, web UI,
  multi-tenancy — minus Kubernetes.
- **After Phase 7** you have what you originally described.

Phases 8 and 9 are what make it something you could hand to someone else.

## What to resist

- **Do not start with the web UI.** It is the most fun and the least load-bearing.
  Building it before the engine means designing screens for a data model that does
  not exist yet, and rewriting them.
- **Do not add a second provider before Phase 8.** One working provider teaches
  you what the trait should be. Two half-working providers teach you nothing.
- **Do not skip Phase 0's CI setup.** An hour spent there saves days later, and it
  is the cheapest hour in the project.
- **Do not perfect Phase 1's type model.** It will be wrong. You will find out how
  in Phase 3. Make it easy to change instead of trying to make it right.
