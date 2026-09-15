# Example lab

A small but realistic Daedalus repository: one Proxmox cluster shared by two
tenants. It is the fixture the test suite validates, so everything in it is
known to load.

```bash
dae validate examples/lab
```

| Path | What it shows |
| --- | --- |
| `platform/providers/` | A provider every tenant uses |
| `catalog/` | Images and machine classes tenants choose from |
| `tenants/acme/environments/prod/` | Networks in one multi-document file; machines that override their class, add disks, and order themselves with `dependsOn` |
| `tenants/acme/environments/staging/` | The same names as prod — environments are separate scopes — in one file |
| `tenants/bob/` | A second tenant on the same hardware: DHCP, no class, IPv6 |

For the mistakes Daedalus catches, see [`../broken`](../broken).
