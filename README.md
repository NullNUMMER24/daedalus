# Daedalus

> Daedalus, the Greek inventor and master craftsman — who built the Labyrinth,
> and knew the way out of it.

Daedalus is a **self-hosted infrastructure control plane for homelabs**. One tool,
one API, and one Git repository to manage both **virtual machines** and
**Kubernetes clusters**, with **real multi-tenancy** so you can safely share
hardware with other people.

## The idea in one paragraph

You describe what your lab should look like in YAML, committed to Git. Daedalus
continuously compares that desired state against what actually exists on your
hypervisors and clusters, shows you a plan of the difference, and applies it.
Every change is attributable to a person, scoped to a tenant, checked against a
quota, and recorded in an audit log. You drive it from a terminal (`dae`) or from
a web dashboard, and both speak to the same API — there is no back door.

## Naming

| Name         | Role                | Why                                                                 |
| ------------ | ------------------- | ------------------------------------------------------------------- |
| **`Daedalus`* | The project         | The master craftsman who built the thing                            |
| **`Labyrinth`**| Resource graph      | Resources and their dependencies, walked in topological order       |
| **`Ariadne`**  | State thread        | The append-only thread of state — lets you retrace your steps (rollback, audit) |
| **`Icarus`**   | Host agent          | Optional per-host agent. Named for the one who falls: the server never trusts it with authority it cannot verify |
| **`dae`**    | CLI binary          | Short and typeable                                                  |
| **`daedalusd`** | Server binary    | API + reconciler + web UI                                           |

## What it manages

- **Machines** — VMs on Proxmox VE (first), with libvirt/KubeVirt/Harvester designed in
- **Clusters** — k3s first, Talos Linux later; provisioned onto Daedalus-managed VMs
- **Networks** — VLANs, subnets, IPAM, DNS records
- **Storage** — volumes and per-tenant storage pools
- **Secrets** — age/SOPS-encrypted in Git, decryptable only by the owning tenant
- **Tenants** — people or teams sharing your hardware, with hard isolation and quotas

## Interfaces

- **`dae`** — a thin, scriptable client over the HTTP API. Kubernetes-familiar verbs.
- **Web UI** — server-rendered (Axum + Maud + htmx), no JavaScript build step.
  Aimed at people who should not have to learn a CLI.
- **HTTP API** — the only way anything mutates state. OpenAPI-described.

## Documentation

| Document | What is in it |
| --- | --- |
| [Concepts](docs/concepts.md) | The domain model and vocabulary — read this first |
| [Architecture](docs/architecture.md) | Components, the three states, control loops, safety |
| [Multi-tenancy](docs/multitenancy.md) | The seven isolation layers, quotas, tenant lifecycle |
| [GitOps](docs/gitops.md) | Repository layout, manifest schema, UI write-back |
| [Providers](docs/providers.md) | The provider trait; Proxmox vs libvirt vs KubeVirt |
| [CLI](docs/cli.md) | Command surface and UX conventions |
| [Tech stack](docs/tech-stack.md) | Crate choices, workspace layout, and why |
| [Decisions](docs/decisions.md) | Numbered architecture decisions with rationale |
| [Roadmap](docs/roadmap.md) | The ten phases, at a glance |
| [Implementation plan](docs/plan/) | Detailed, step-by-step build instructions per phase |

## Status

Design phase. Nothing is implemented yet. Start at
[docs/plan/phase-0-foundations.md](docs/plan/phase-0-foundations.md).
