//! Loading: files in, a validated [`Repository`] — or every problem — out.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;

use serde::de::DeserializeOwned;
use serde_saphyr::Spanned;

use crate::diagnostic::{Code, SourceFile, line_col};
use crate::document::{self, Document};
use crate::manifest::{
    Header, Identity, ImageSpec, MachineClassSpec, MachineSpec, Manifest, MetadataSpec,
    NetworkSpec, ProviderSpec, TenantSpec,
};
use crate::resource::{API_VERSION, Kind, Repository, ResourceKey, Scope};
use crate::suggest::{did_you_mean, list, parse_unknown};
use crate::yaml::{self, YamlError};
use crate::{CoreError, Name, Source, ValidationError, resolve};

/// Validates a repository.
///
/// Every problem in every file is reported, not just the first. Only a
/// repository with no problems at all produces a [`Repository`].
///
/// ```
/// use dae_core::{Source, load};
///
/// let result = load([Source::new("catalog/images/debian.yaml", "kind: Image\n")]);
/// assert!(result.is_err(), "no apiVersion, no metadata, no spec");
/// ```
pub fn load(sources: impl IntoIterator<Item = Source>) -> Result<Repository, CoreError> {
    let mut files: Vec<SourceFile> = sources.into_iter().map(SourceFile::new).collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut loader = Loader::default();
    for file in &files {
        loader.load_file(file);
    }
    let index = loader.index();
    loader.check_tenants(&index);

    let resources = resolve::resolve(&loader.parsed, &index, &loader.poisoned, &mut loader.errors);

    if loader.errors.is_empty() {
        Ok(Repository::new(resources))
    } else {
        loader
            .errors
            .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        Err(CoreError::Validation(loader.errors))
    }
}

/// Where a document came from, so diagnostics can point back into its file.
#[derive(Debug, Clone)]
pub(crate) struct Origin {
    pub(crate) file: SourceFile,
    /// Byte offset of the document within the file.
    pub(crate) offset: usize,
}

impl Origin {
    pub(crate) fn error(&self, code: Code, message: impl Into<String>) -> ValidationError {
        ValidationError::at(code, &self.file, message)
    }

    /// Absolute byte range of a spanned value.
    pub(crate) fn span<T>(&self, value: &Spanned<T>) -> Option<Range<usize>> {
        yaml::span(value).map(|r| self.shift(r))
    }

    pub(crate) const fn shift(&self, range: Range<usize>) -> Range<usize> {
        range.start + self.offset..range.end + self.offset
    }

    /// `path:line` of a span, for "first defined at …" hints.
    pub(crate) fn locate(&self, span: Option<&Range<usize>>) -> String {
        match span.and_then(|s| line_col(&self.file.text, s.start)) {
            Some((line, _)) => format!("{}:{line}", self.file.path),
            None => self.file.path.to_string(),
        }
    }
}

/// A document that parsed as a known kind.
#[derive(Debug)]
pub(crate) struct Parsed {
    pub(crate) origin: Origin,
    pub(crate) scope: Scope,
    pub(crate) metadata: MetadataSpec,
    pub(crate) spec: Spec,
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "one per document, alive only while a repository loads; boxing buys nothing measurable"
)]
pub(crate) enum Spec {
    Provider(ProviderSpec),
    Image(ImageSpec),
    MachineClass(MachineClassSpec),
    Tenant(TenantSpec),
    Network(NetworkSpec),
    Machine(MachineSpec),
}

impl Spec {
    pub(crate) const fn kind(&self) -> Kind {
        match self {
            Self::Provider(_) => Kind::Provider,
            Self::Image(_) => Kind::Image,
            Self::MachineClass(_) => Kind::MachineClass,
            Self::Tenant(_) => Kind::Tenant,
            Self::Network(_) => Kind::Network,
            Self::Machine(_) => Kind::Machine,
        }
    }
}

impl Parsed {
    pub(crate) fn key(&self) -> ResourceKey {
        ResourceKey {
            scope: self.scope.clone(),
            kind: self.spec.kind(),
            name: self.metadata.name.value.clone(),
        }
    }
}

/// Resources that exist but failed to load. References to them are not
/// reported as unresolved: one typo in a network should produce one error,
/// not one more for every machine attached to it.
pub(crate) type Poisoned = HashSet<(Scope, Kind, String)>;

/// Which part of the repository layout a file sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Area {
    Platform,
    Catalog,
    TenantManifest,
    Environment,
}

#[derive(Debug, Default)]
struct Loader {
    parsed: Vec<Parsed>,
    errors: Vec<ValidationError>,
    poisoned: Poisoned,
    /// Tenants with a `tenant.yaml`, and those whose `tenant.yaml` had errors.
    tenant_manifest_seen: BTreeSet<Name>,
    tenant_manifest_failed: BTreeSet<Name>,
}

impl Loader {
    fn load_file(&mut self, file: &SourceFile) {
        let (area, scope) = match placement(&file.path) {
            Ok(placed) => placed,
            Err(message) => {
                self.errors
                    .push(ValidationError::file(Code::Placement, &*file.path, message));
                return;
            }
        };
        if let (Area::TenantManifest, Scope::Tenant(tenant)) = (area, &scope) {
            self.tenant_manifest_seen.insert(tenant.clone());
        }

        let before = self.errors.len();
        for doc in document::split(&file.text) {
            let origin = Origin {
                file: file.clone(),
                offset: doc.offset,
            };
            self.load_document(&origin, doc, area, &scope);
        }
        if let (Area::TenantManifest, Scope::Tenant(tenant)) = (area, &scope)
            && self.errors.len() > before
        {
            self.tenant_manifest_failed.insert(tenant.clone());
        }
    }

    fn load_document(&mut self, origin: &Origin, doc: Document<'_>, area: Area, scope: &Scope) {
        let header = match yaml::parse::<Header>(doc.text) {
            Ok(header) => header,
            Err(err) => return self.yaml_error(origin, err),
        };
        let Some(kind) = self.check_header(origin, doc, scope, &header) else {
            if header
                .kind
                .as_ref()
                .is_none_or(|k| k.value.parse::<Kind>().is_err())
            {
                for kind in kinds_in(area) {
                    self.poison(doc, scope, *kind);
                }
            }
            return;
        };
        if let Some(home) = misplaced(kind, area) {
            self.poison(doc, scope, kind);
            let span = header.kind.as_ref().and_then(|k| origin.span(k));
            self.errors.push(
                origin
                    .error(
                        Code::Placement,
                        format!("a {kind} does not belong in `{}`", origin.file.path),
                    )
                    .label(span, "wrong directory for this kind")
                    .with_help(format!("move this file under `{home}`")),
            );
            return;
        }

        match kind {
            Kind::Provider => self.typed(origin, doc, scope, kind, Spec::Provider),
            Kind::Image => self.typed(origin, doc, scope, kind, Spec::Image),
            Kind::MachineClass => self.typed(origin, doc, scope, kind, Spec::MachineClass),
            Kind::Tenant => self.typed(origin, doc, scope, kind, Spec::Tenant),
            Kind::Network => self.typed(origin, doc, scope, kind, Spec::Network),
            Kind::Machine => self.typed(origin, doc, scope, kind, Spec::Machine),
        }
    }

    /// Parses a document as `Manifest<S>` once its kind is known.
    fn typed<S: DeserializeOwned>(
        &mut self,
        origin: &Origin,
        doc: Document<'_>,
        scope: &Scope,
        kind: Kind,
        wrap: fn(S) -> Spec,
    ) {
        match yaml::parse::<Manifest<S>>(doc.text) {
            Ok(manifest) => {
                self.check_metadata(origin, scope, kind, &manifest.metadata);
                self.parsed.push(Parsed {
                    origin: origin.clone(),
                    scope: scope.clone(),
                    metadata: manifest.metadata,
                    spec: wrap(manifest.spec),
                });
            }
            Err(err) => {
                self.poison(doc, scope, kind);
                self.yaml_error(origin, err);
            }
        }
    }

    /// Checks `apiVersion` and `kind`, returning the kind if the document can
    /// be parsed further.
    fn check_header(
        &mut self,
        origin: &Origin,
        doc: Document<'_>,
        scope: &Scope,
        header: &Header,
    ) -> Option<Kind> {
        let start = yaml::first_token(doc.text).map(|r| origin.shift(r));
        let kinds: Vec<_> = Kind::ALL.iter().map(|k| k.as_str()).collect();

        let kind = match &header.kind {
            None => {
                self.errors.push(
                    origin
                        .error(Code::MissingKind, "document has no `kind`")
                        .label(start.clone(), "")
                        .with_help(format!("add `kind:` with one of {}", list(&kinds, 8))),
                );
                None
            }
            Some(raw) => {
                let parsed = raw.value.parse::<Kind>().ok();
                if parsed.is_none() {
                    let help = did_you_mean(&raw.value, kinds.iter().copied()).map_or_else(
                        || format!("expected one of {}", list(&kinds, 8)),
                        |best| format!("did you mean `{best}`?"),
                    );
                    self.errors.push(
                        origin
                            .error(
                                Code::UnknownKind,
                                format!("unknown kind `{}`", raw.value.escape_debug()),
                            )
                            .label(origin.span(raw), "")
                            .with_help(help),
                    );
                }
                parsed
            }
        };

        let api_ok = match &header.api_version {
            Some(version) if version.value == API_VERSION => true,
            Some(version) => {
                self.errors.push(
                    origin
                        .error(
                            Code::UnsupportedApiVersion,
                            format!("unsupported apiVersion `{}`", version.value.escape_debug()),
                        )
                        .label(origin.span(version), "")
                        .with_help(format!("this version of dae reads `{API_VERSION}`")),
                );
                false
            }
            None => {
                self.errors.push(
                    origin
                        .error(Code::MissingApiVersion, "document has no `apiVersion`")
                        .label(start, "")
                        .with_help(format!("add `apiVersion: {API_VERSION}`")),
                );
                false
            }
        };

        match kind {
            Some(kind) if !api_ok => {
                // Not parsed further, but it still exists as far as references go.
                self.poison(doc, scope, kind);
                None
            }
            other => other,
        }
    }

    /// `metadata.tenant` and `metadata.environment` are optional, but must agree
    /// with the directory when present.
    fn check_metadata(&mut self, origin: &Origin, scope: &Scope, kind: Kind, meta: &MetadataSpec) {
        let (path_tenant, path_env) = match scope {
            Scope::Platform => (None, None),
            Scope::Tenant(t) => (Some(t), None),
            Scope::Environment {
                tenant,
                environment,
            } => (Some(tenant), Some(environment)),
        };

        for (field, declared, from_path, what) in [
            ("tenant", &meta.tenant, path_tenant, "tenants/"),
            ("environment", &meta.environment, path_env, "environments/"),
        ] {
            let Some(declared) = declared else { continue };
            let problem = match from_path {
                None => format!(
                    "a {kind} has no {field}, but `metadata.{field}` is `{}`",
                    declared.value
                ),
                Some(expected) if *expected != declared.value => format!(
                    "`metadata.{field}` is `{}`, but this file is under `{what}{expected}/`",
                    declared.value
                ),
                Some(_) => continue,
            };
            self.errors.push(
                origin
                    .error(Code::ScopeMismatch, problem)
                    .label(origin.span(declared), "does not match the file's location")
                    .with_help(format!(
                        "the directory decides the {field}: remove `metadata.{field}`, or move the file"
                    )),
            );
        }

        if let (Kind::Tenant, Scope::Tenant(dir)) = (kind, scope)
            && meta.name.value != *dir
        {
            self.errors.push(
                origin
                    .error(
                        Code::ScopeMismatch,
                        format!(
                            "Tenant `{}` is defined in `tenants/{dir}/`",
                            meta.name.value
                        ),
                    )
                    .label(origin.span(&meta.name), "does not match the directory")
                    .with_help(format!(
                        "a Tenant's name must match its directory: rename it to `{dir}`, or move the file"
                    )),
            );
        }
    }

    fn yaml_error(&mut self, origin: &Origin, err: YamlError) {
        let code = classify(&err.message);
        let label = match code {
            Code::UnknownField => "unknown field",
            Code::UnknownVariant => "unknown value",
            Code::DuplicateKey => "duplicate key",
            _ => "",
        };
        let help = parse_unknown(&err.message)
            .and_then(|(found, expected)| did_you_mean(found, expected))
            .map(|best| format!("did you mean `{best}`?"));

        let mut diagnostic = origin
            .error(code, &err.message)
            .label(err.span.map(|r| origin.shift(r)), label);
        if let Some(help) = help {
            diagnostic = diagnostic.with_help(help);
        }
        self.errors.push(diagnostic);
    }

    /// Records the name of a document that failed to load. See [`Poisoned`].
    fn poison(&mut self, doc: Document<'_>, scope: &Scope, kind: Kind) {
        if let Some(name) = identity(doc) {
            self.poisoned.insert((scope.clone(), kind, name));
        }
    }

    /// Indexes parsed resources, reporting names defined twice.
    fn index(&mut self) -> BTreeMap<ResourceKey, usize> {
        let mut index = BTreeMap::new();
        for (i, parsed) in self.parsed.iter().enumerate() {
            let key = parsed.key();
            if let Some(&first) = index.get(&key) {
                let first: &Parsed = &self.parsed[first];
                let first_span = first.origin.span(&first.metadata.name);
                self.errors.push(
                    parsed
                        .origin
                        .error(
                            Code::DuplicateResource,
                            format!("{key} is defined more than once"),
                        )
                        .label(
                            parsed.origin.span(&parsed.metadata.name),
                            "defined again here",
                        )
                        .with_help(format!(
                            "first defined at {}",
                            first.origin.locate(first_span.as_ref())
                        )),
                );
            } else {
                index.insert(key, i);
            }
        }
        index
    }

    /// Every tenant with resources needs a `tenant.yaml` defining it.
    fn check_tenants(&mut self, index: &BTreeMap<ResourceKey, usize>) {
        let used: BTreeSet<Name> = self
            .parsed
            .iter()
            .filter_map(|p| match &p.scope {
                Scope::Environment { tenant, .. } => Some(tenant.clone()),
                _ => None,
            })
            .collect();

        for tenant in used {
            let key = ResourceKey {
                scope: Scope::Tenant(tenant.clone()),
                kind: Kind::Tenant,
                name: tenant.clone(),
            };
            let defined = index.contains_key(&key)
                || self
                    .poisoned
                    .contains(&(key.scope.clone(), Kind::Tenant, tenant.to_string()));
            if defined || self.tenant_manifest_failed.contains(&tenant) {
                continue;
            }
            let path = format!("tenants/{tenant}/tenant.yaml");
            let message = if self.tenant_manifest_seen.contains(&tenant) {
                format!("`{path}` does not define Tenant `{tenant}`")
            } else {
                format!("tenant `{tenant}` has resources but no `{path}`")
            };
            self.errors.push(
                ValidationError::file(Code::MissingTenant, path, message).with_help(format!(
                    "create it with `kind: Tenant` and `metadata.name: {tenant}`"
                )),
            );
        }
    }
}

/// A document's `metadata.name`, read leniently from a document that did not
/// parse as its kind.
fn identity(doc: Document<'_>) -> Option<String> {
    yaml::parse::<Identity>(doc.text).ok()?.metadata?.name
}

fn classify(message: &str) -> Code {
    let starts = |prefix| message.starts_with(prefix);
    if starts("unknown field") {
        Code::UnknownField
    } else if starts("unknown variant") {
        Code::UnknownVariant
    } else if starts("missing field") {
        Code::MissingField
    } else if starts("duplicate mapping key") {
        Code::DuplicateKey
    } else if starts("invalid ") {
        Code::InvalidValue
    } else {
        Code::Yaml
    }
}

/// Where a file sits in the layout, and the scope that gives its resources:
///
/// ```text
/// platform/**                                   platform-wide
/// catalog/**                                    platform-wide
/// tenants/<tenant>/tenant.yaml                  the tenant
/// tenants/<tenant>/environments/<env>/**        the environment
/// ```
fn placement(path: &str) -> Result<(Area, Scope), String> {
    let parts: Vec<&str> = path.split('/').collect();
    let dir_name = |raw: &str, what: &str| {
        Name::parse(raw).map_err(|err| match err {
            CoreError::InvalidName { reason, .. } => {
                format!("`{path}`: directory `{raw}` is not a valid {what} name: {reason}")
            }
            other => other.to_string(),
        })
    };

    match parts.as_slice() {
        ["platform", _, ..] => Ok((Area::Platform, Scope::Platform)),
        ["catalog", _, ..] => Ok((Area::Catalog, Scope::Platform)),
        ["tenants", tenant, "tenant.yaml" | "tenant.yml"] => Ok((
            Area::TenantManifest,
            Scope::Tenant(dir_name(tenant, "tenant")?),
        )),
        ["tenants", tenant, "environments", environment, _, ..] => Ok((
            Area::Environment,
            Scope::Environment {
                tenant: dir_name(tenant, "tenant")?,
                environment: dir_name(environment, "environment")?,
            },
        )),
        ["tenants", tenant, ..] => Err(format!(
            "`{path}` is not part of the repository layout: under `tenants/{tenant}/`, \
             manifests go in `tenant.yaml` or under `environments/<environment>/`"
        )),
        _ => Err(format!(
            "`{path}` is not part of the repository layout: manifests go under \
             `platform/`, `catalog/` or `tenants/`"
        )),
    }
}

/// The kinds that may live in `area`.
const fn kinds_in(area: Area) -> &'static [Kind] {
    match area {
        Area::Platform => &[Kind::Provider],
        Area::Catalog => &[Kind::Image, Kind::MachineClass],
        Area::TenantManifest => &[Kind::Tenant],
        Area::Environment => &[Kind::Network, Kind::Machine],
    }
}

/// `None` if `kind` may live in `area`; otherwise the directory it belongs in.
fn misplaced(kind: Kind, area: Area) -> Option<&'static str> {
    let (home, path) = match kind {
        Kind::Provider => (Area::Platform, "platform/"),
        Kind::Image | Kind::MachineClass => (Area::Catalog, "catalog/"),
        Kind::Tenant => (Area::TenantManifest, "tenants/<tenant>/tenant.yaml"),
        Kind::Network | Kind::Machine => (
            Area::Environment,
            "tenants/<tenant>/environments/<environment>/",
        ),
    };
    (home != area).then_some(path)
}
