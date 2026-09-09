# Phase 1 — Core model

**Goal.** Typed resources, YAML loading, and validation with excellent error
messages. No network, no database, no async.

**Demo.** `dae validate ./examples/lab` parses a folder of manifests and either
succeeds or prints beautiful, precise errors with file, line, and a suggestion.

**Estimate.** 2–3 weeks. The biggest phase in raw Rust learning.

**Rust you will learn.** Traits, generics, `serde` derive and custom
deserialisation, newtype patterns, `thiserror`, enums with data, `Option`
handling, iterators, unit testing.

> ⚠️ **Everything here is synchronous.** No `tokio`, no `async fn`. Learn
> ownership and borrowing on their own before adding futures to the mix. This is
> the most important sequencing decision in the whole plan.

---

## Step 1.1 — Newtypes for identifiers

Create `crates/dae-core/src/ids.rs`:

```rust
use std::fmt;
use serde::{Deserialize, Serialize};

/// A resource's permanent unique identifier. Assigned once, never changes,
/// survives renames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Uid(ulid::Ulid);

impl Uid {
    pub fn new() -> Self { Self(ulid::Ulid::new()) }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A DNS-safe name, unique within (tenant, environment, kind).
/// Validated on construction — an invalid `Name` cannot exist.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Name(String);

impl Name {
    pub fn parse(s: impl Into<String>) -> Result<Self, CoreError> {
        let s = s.into();
        if s.is_empty() || s.len() > 63 {
            return Err(CoreError::InvalidName {
                name: s, reason: "must be 1-63 characters".into() });
        }
        if !s.chars().all(|c| c.is_ascii_lowercase()
                            || c.is_ascii_digit() || c == '-') {
            return Err(CoreError::InvalidName {
                name: s,
                reason: "only lowercase letters, digits and hyphens".into() });
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(CoreError::InvalidName {
                name: s, reason: "must not start or end with a hyphen".into() });
        }
        Ok(Self(s))
    }
    pub fn as_str(&self) -> &str { &self.0 }
}

// Deserialize goes through parse(), so YAML cannot produce an invalid Name.
impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: serde::Deserializer<'de>>(d: D)
        -> Result<Self, D::Error>
    {
        let s = String::deserialize(d)?;
        Name::parse(s).map_err(serde::de::Error::custom)
    }
}

pub struct TenantId(Uid);
pub struct PrincipalId(Uid);
```

💡 **This is the single most valuable Rust habit in the project.** A `Name` that
exists is valid, by construction. You never write "is this name valid?" again
anywhere in the codebase, because invalid ones cannot be built. Compare with
passing `String` everywhere and validating at each use site — which you will
eventually forget to do.

**Check:** unit tests. `Name::parse("web-01")` is `Ok`; `"Web01"`, `""`, `"-x"`,
and a 64-character string are all `Err` with distinguishable reasons.

---

## Step 1.2 — Quantities

Infrastructure is full of `8Gi` and `500m`. Parse them once, properly.

```rust
// crates/dae-core/src/quantity.rs

/// Bytes, parsed from Kubernetes-style suffixes.
/// Accepts: 1024, 1Ki, 1Mi, 1Gi, 1Ti (binary) and 1k, 1M, 1G, 1T (decimal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteSize(u64);

impl ByteSize {
    pub fn parse(s: &str) -> Result<Self, CoreError> { /* ... */ }
    pub fn as_bytes(&self)     -> u64 { self.0 }
    pub fn as_mebibytes(&self) -> u64 { self.0 / (1024 * 1024) }
}

// Display renders back to the most compact exact form: 8589934592 -> "8Gi"
impl fmt::Display for ByteSize { /* ... */ }
```

⚠️ Round-tripping matters: `parse(x).to_string()` must equal `x` for canonical
inputs, or the UI write-back in Phase 6 will produce noisy diffs.

**Check:** a `proptest` that `ByteSize::parse(&b.to_string()) == Ok(b)` for
arbitrary `b`. This is a perfect first property test.

---

## Step 1.3 — The resource envelope

```rust
// crates/dae-core/src/resource.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]     // ⚠️ typos become errors, not silence
pub struct Resource<S> {
    pub api_version: ApiVersion,
    pub kind: Kind,
    pub metadata: Metadata,
    pub spec: S,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Metadata {
    pub name: Name,
    #[serde(default)] pub tenant: Option<Name>,
    #[serde(default)] pub environment: Option<Name>,
    #[serde(default)] pub labels: BTreeMap<String, String>,
    #[serde(default)] pub annotations: BTreeMap<String, String>,
}

/// The type-erased form, for when you have a mixed bag of kinds from a
/// directory walk and do not yet know what each one is.
#[derive(Debug, Clone)]
pub enum AnyResource {
    Machine(Resource<MachineSpec>),
    Network(Resource<NetworkSpec>),
    Cluster(Resource<ClusterSpec>),
    MachineClass(Resource<MachineClassSpec>),
    Image(Resource<ImageSpec>),
    Tenant(Resource<TenantSpec>),
}

impl AnyResource {
    pub fn kind(&self)     -> Kind      { /* ... */ }
    pub fn metadata(&self) -> &Metadata { /* ... */ }
    /// Stable identity: "Machine/acme/prod/web-01"
    pub fn key(&self)      -> ResourceKey { /* ... */ }
}
```

💡 `#[serde(deny_unknown_fields)]` turns `memroy: 16Gi` into a loud error instead
of a silently ignored field. Silent field-dropping in a config tool is a genuine
data-loss bug — the user thinks they set something and it did nothing.

💡 `Resource<S>` generic over the spec, plus an `AnyResource` enum, is the
idiomatic Rust answer to "heterogeneous collection with type safety". Fight the
urge to reach for `Box<dyn Resource>`; you would lose the ability to match.

**Check:** deserialize a `Machine` YAML into `Resource<MachineSpec>`; an unknown
field fails with a message naming it.

---

## Step 1.4 — The Machine spec

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MachineSpec {
    #[serde(default)] pub class: Option<Name>,
    pub provider: Name,
    #[serde(default)] pub image: Option<Name>,
    #[serde(default)] pub cpu: Option<CpuSpec>,
    #[serde(default)] pub memory: Option<ByteSize>,
    #[serde(default)] pub disks: Vec<DiskSpec>,
    #[serde(default)] pub networks: Vec<NetworkAttachment>,
    #[serde(default)] pub cloud_init: Option<CloudInitSpec>,
    #[serde(default)] pub lifecycle: LifecycleSpec,
    #[serde(default)] pub depends_on: Vec<ResourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DiskSpec {
    pub name: Name,
    pub size: ByteSize,
    #[serde(default)] pub storage: Option<Name>,
    #[serde(default)] pub discard: bool,
}
```

⚠️ Note the `Option`s. Fields are optional in the *manifest* because a
`MachineClass` may supply them, but required in the *resolved* spec. Model this
explicitly rather than defaulting silently — see step 1.7.

**Check:** the example manifest from [gitops.md](../gitops.md#manifest-format)
deserializes cleanly.

---

## Step 1.5 — Loading a directory

```rust
// crates/dae-core/src/loader.rs

pub struct LoadedRepo {
    pub resources: Vec<AnyResource>,
    pub errors:    Vec<ValidationError>,   // collected, not fatal
}

pub fn load_dir(root: &Path) -> LoadedRepo {
    let mut out = LoadedRepo::default();
    for path in walk_yaml_files(root) {
        // A file may hold several documents separated by `---`.
        for (idx, doc) in split_documents(&read(&path)) {
            match parse_document(&doc) {
                Ok(r)  => out.resources.push(r),
                Err(e) => out.errors.push(e.at(&path, idx)),
            }
        }
    }
    out
}
```

Two-pass parsing is the trick: first deserialize only `{ apiVersion, kind }` to
learn what the document is, then deserialize the whole thing into the right type.

**Check:** load a directory of five files, one of which is malformed. You get
four resources *and* one error, not a panic and not an early return.

---

## Step 1.6 — Path inference

Tenant and environment come from the file's location:

```
tenants/acme/environments/prod/machines/web-01.yaml
        ^^^^              ^^^^
        tenant            environment
```

If `metadata.tenant` is also present it must **match**, otherwise it is an error.
That check catches copy-pasted files, which is the single most common way a
resource ends up in the wrong tenant.

**Check:** a file under `tenants/acme/` declaring `tenant: bob` produces a clear
mismatch error naming both values.

---

## Step 1.7 — Class merging

```rust
/// A spec with every field resolved. Only this can be planned against.
pub struct ResolvedMachineSpec {
    pub provider: Name,
    pub image: Name,
    pub cpu: CpuSpec,
    pub memory: ByteSize,
    pub disks: Vec<ResolvedDisk>,
    // ... no Options left
}

pub fn resolve_machine(
    m:       &Resource<MachineSpec>,
    classes: &HashMap<Name, MachineClassSpec>,
) -> Result<ResolvedMachineSpec, ValidationError>;
```

Merge rules, written down because you will forget them:

1. Start from the class's fields (if `spec.class` is set).
2. Machine fields override class fields.
3. Lists of named items (disks, networks) **merge by name**; they are not
   replaced wholesale.
4. Anything still missing after the merge is an error naming the field.

💡 Two distinct types — `MachineSpec` (what YAML holds, full of `Option`) and
`ResolvedMachineSpec` (what the planner consumes, no `Option`) — is the
type-driven way to make "unresolved" unrepresentable downstream. The compiler
then guarantees the planner never sees a `None` it must handle.

**Check:** a machine with `class: standard-4x8` and `memory: 16Gi` resolves to 4
cores from the class and 16Gi from the machine.

---

## Step 1.8 — Reference resolution

Build an index of every resource by `(tenant, environment, kind, name)`, then
walk every reference field and confirm it resolves. Unresolved references become
errors with a "did you mean?" hint via Levenshtein distance (`strsim` crate).

⚠️ **References are tenant-scoped.** `Machine/acme/web-01` may reference
`Network/acme/prod-net` but never `Network/bob/prod-net`. Enforce this here, in
the loader, in addition to Phase 5's checks — defence in depth starts early.

**Check:** a typo'd network reference produces
`no Network named 'prod-nett' in tenant 'acme'; available: prod-net, mgmt-net`.

---

## Step 1.9 — Beautiful errors

Wire up `miette` so errors carry source spans:

```
Error: unknown field `memroy`

   ╭─[tenants/acme/environments/prod/machines/web-01.yaml:14:5]
14 │     memroy: 16Gi
   ·     ───┬──
   ·        ╰── did you mean `memory`?
   ╰────
```

To get line numbers from `serde`, use a YAML crate that exposes spans, or
deserialize into a span-carrying intermediate. Budget real time for this — it is
fiddlier than it looks, and it is worth it.

💡 This is what separates a tool you like from one you tolerate. You will see
these errors hundreds of times.

**Check:** every error class prints with file, line, column, and a hint.

---

## Step 1.10 — `dae validate`

```bash
$ dae validate ./examples/lab
✓ 14 resources in 2 tenants, 3 environments
  Machine 8   Network 3   Cluster 2   MachineClass 1

$ dae validate ./examples/broken
✗ 2 errors
  [error output as above]
```

Exit 0 on success, 1 on errors.

**Check:** both example directories behave as shown, and CI runs `dae validate`
on `examples/lab` as a test.

---

## Step 1.11 — Test fixtures

Build `examples/lab/` as a realistic two-tenant repository. You will use it for
the entire rest of the project — as a test fixture, a demo, and documentation.

Add snapshot tests with `insta`:

```rust
#[test]
fn loads_example_lab() {
    let loaded = load_dir(Path::new("../../examples/lab"));
    assert!(loaded.errors.is_empty(), "{:#?}", loaded.errors);
    insta::assert_debug_snapshot!(loaded.resources);
}
```

💡 `cargo insta review` gives you a diff of exactly what changed in the parsed
output whenever you touch the loader. It is the highest-leverage testing tool for
this kind of code.

**Check:** `cargo nextest run` passes; `cargo insta review` shows no pending
changes.

---

## Definition of done

- [ ] `dae validate ./examples/lab` succeeds and reports a summary
- [ ] `dae validate ./examples/broken` prints spans, lines, and hints
- [ ] Unknown fields are errors, not silently ignored
- [ ] Tenant/environment inferred from path; mismatches rejected
- [ ] Class merging works with list-merge-by-name semantics
- [ ] Cross-tenant references are rejected
- [ ] `ByteSize` round-trips (property-tested)
- [ ] Snapshot tests cover the example repo
- [ ] **Zero `async` in `dae-core`**

## Pitfalls

- **Reaching for `async` because it seems more "real".** It is not. Nothing here
  does I/O worth awaiting, and you will learn ownership far better without it.
- **`String` instead of newtypes.** The refactor in Phase 5 is painful. Do it now.
- **One giant `Spec` enum with every field optional.** Per-kind structs are
  clearer and let the compiler help.
- **Failing on the first error.** Collect them. Users have more than one typo.
- **Perfecting the model.** It will be wrong; Phase 3 will show you how. Optimise
  for *changeability*, not correctness — keep it small and well-tested.
