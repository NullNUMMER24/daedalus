# Broken example

Every file here has one deliberate mistake, described in a comment at its top.
It exists to show — and to test — what `dae validate` tells you:

```bash
dae validate examples/broken
```

Note what is *not* reported. The provider has an invalid `type`, yet nothing
that references `pve-main` complains about it: a resource that fails to load
still counts as defined, so one mistake produces one error.
