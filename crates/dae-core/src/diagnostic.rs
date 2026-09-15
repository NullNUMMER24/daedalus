//! Validation diagnostics that point at the exact place in a manifest.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use miette::{Diagnostic, LabeledSpan, NamedSource, SourceCode};

/// A stable identifier for each kind of problem.
///
/// Rendered as `dae::unknown-field`. Tests and tooling should match on these,
/// not on message wording, which is free to improve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Code {
    /// Malformed YAML, or a value of the wrong type.
    Yaml,
    /// A file that is not UTF-8, or could not be read.
    Unreadable,
    UnknownField,
    UnknownVariant,
    MissingField,
    DuplicateKey,
    MissingApiVersion,
    UnsupportedApiVersion,
    MissingKind,
    UnknownKind,
    /// A file outside the layout, or a kind in the wrong directory.
    Placement,
    /// `metadata.tenant` or `metadata.environment` disagrees with the path.
    ScopeMismatch,
    DuplicateResource,
    MissingTenant,
    UnresolvedReference,
    /// A field that is neither set nor provided by the machine's class.
    MissingValue,
    InvalidValue,
    AddressConflict,
}

impl Code {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Yaml => "dae::yaml",
            Self::Unreadable => "dae::unreadable",
            Self::UnknownField => "dae::unknown-field",
            Self::UnknownVariant => "dae::unknown-variant",
            Self::MissingField => "dae::missing-field",
            Self::DuplicateKey => "dae::duplicate-key",
            Self::MissingApiVersion => "dae::missing-api-version",
            Self::UnsupportedApiVersion => "dae::unsupported-api-version",
            Self::MissingKind => "dae::missing-kind",
            Self::UnknownKind => "dae::unknown-kind",
            Self::Placement => "dae::placement",
            Self::ScopeMismatch => "dae::scope-mismatch",
            Self::DuplicateResource => "dae::duplicate-resource",
            Self::MissingTenant => "dae::missing-tenant",
            Self::UnresolvedReference => "dae::unresolved-reference",
            Self::MissingValue => "dae::missing-value",
            Self::InvalidValue => "dae::invalid-value",
            Self::AddressConflict => "dae::address-conflict",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One file handed to the loader.
///
/// `path` is relative to the repository root with `/` separators, e.g.
/// `tenants/acme/environments/prod/machines/web-01.yaml`. The loader never
/// touches the filesystem: the CLI reads a directory into these, and Phase 2
/// will build them from Git blobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: String,
    pub text: String,
}

impl Source {
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            text: text.into(),
        }
    }
}

/// A loaded file, shared by every diagnostic that points into it.
#[derive(Debug, Clone)]
pub(crate) struct SourceFile {
    pub(crate) path: Arc<str>,
    pub(crate) text: Arc<str>,
}

impl SourceFile {
    pub(crate) fn new(source: Source) -> Self {
        Self {
            path: source.path.into(),
            text: source.text.into(),
        }
    }
}

/// A problem found while validating a repository.
#[derive(Debug, Clone)]
pub struct ValidationError {
    code: Code,
    path: Arc<str>,
    message: String,
    source: Option<NamedSource<Arc<str>>>,
    /// Byte ranges into `source`. The first is the primary location.
    labels: Vec<(Range<usize>, String)>,
    help: Option<String>,
}

impl ValidationError {
    /// A problem with a whole file, with no position inside it.
    pub fn file(code: Code, path: impl Into<Arc<str>>, message: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
            source: None,
            labels: Vec::new(),
            help: None,
        }
    }

    /// A problem inside `file`; add positions with [`Self::label`].
    pub(crate) fn at(code: Code, file: &SourceFile, message: impl Into<String>) -> Self {
        Self {
            code,
            path: Arc::clone(&file.path),
            message: message.into(),
            source: Some(NamedSource::new(&*file.path, Arc::clone(&file.text))),
            labels: Vec::new(),
            help: None,
        }
    }

    /// Points at `span` (byte offsets into the file). The first label added is
    /// the primary location. Ignored for file-level diagnostics.
    #[must_use]
    pub(crate) fn label(mut self, span: Option<Range<usize>>, text: impl Into<String>) -> Self {
        if let (Some(span), Some(_)) = (span, &self.source) {
            self.labels.push((span, text.into()));
        }
        self
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    #[must_use]
    pub const fn code(&self) -> Code {
        self.code
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn help_text(&self) -> Option<&str> {
        self.help.as_deref()
    }

    /// 1-based line and column (in characters) of the primary location.
    #[must_use]
    pub fn line_col(&self) -> Option<(usize, usize)> {
        line_col(self.source.as_ref()?.inner(), self.labels.first()?.0.start)
    }

    /// Orders diagnostics by file, then by position in the file.
    pub(crate) fn sort_key(&self) -> (&str, usize, &str) {
        let offset = self.labels.first().map_or(0, |(span, _)| span.start);
        (&self.path, offset, &self.message)
    }
}

/// 1-based line and column (in characters) of a byte offset into `text`.
pub(crate) fn line_col(text: &str, offset: usize) -> Option<(usize, usize)> {
    let before = text.get(..offset)?;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    Some((
        before.matches('\n').count() + 1,
        before[line_start..].chars().count() + 1,
    ))
}

/// The message alone. Renderers add the location and source snippet.
impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ValidationError {}

impl Diagnostic for ValidationError {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.code))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h) as Box<dyn fmt::Display>)
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        self.source.as_ref().map(|s| s as &dyn SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        if self.labels.is_empty() {
            return None;
        }
        Some(Box::new(self.labels.iter().enumerate().map(
            |(i, (span, text))| {
                let text = (!text.is_empty()).then(|| text.clone());
                if i == 0 {
                    LabeledSpan::new_primary_with_span(text, span.clone())
                } else {
                    LabeledSpan::new_with_span(text, span.clone())
                }
            },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "tenants/acme/environments/prod/machines/web-01.yaml";

    fn file(text: &str) -> SourceFile {
        SourceFile::new(Source::new(PATH, text))
    }

    #[test]
    fn codes_render_with_the_dae_prefix() {
        assert_eq!(Code::UnknownField.to_string(), "dae::unknown-field");
    }

    #[test]
    fn display_is_the_message_alone() {
        let err = ValidationError::file(Code::Unreadable, PATH, "not valid UTF-8");
        assert_eq!(err.to_string(), "not valid UTF-8");
        assert_eq!(err.path(), PATH);
        assert_eq!(err.line_col(), None);
    }

    #[test]
    fn line_and_column_count_characters_not_bytes() {
        let text = "# Ünïcödé\nspec:\n  memroy: 16Gi\n";
        let offset = text.find("memroy").unwrap();
        let err = ValidationError::at(Code::UnknownField, &file(text), "unknown field")
            .label(Some(offset..offset + 6), "here");
        assert_eq!(err.line_col(), Some((3, 3)));
    }

    #[test]
    fn exposes_code_help_labels_and_source_to_miette() {
        let text = "spec:\n  memroy: 16Gi\n";
        let err = ValidationError::at(Code::UnknownField, &file(text), "unknown field `memroy`")
            .label(Some(8..14), "not a field of Machine")
            .with_help("did you mean `memory`?");

        assert_eq!(
            Diagnostic::code(&err).unwrap().to_string(),
            "dae::unknown-field"
        );
        assert_eq!(
            Diagnostic::help(&err).unwrap().to_string(),
            "did you mean `memory`?"
        );
        assert!(err.source_code().is_some());
        let labels: Vec<_> = err.labels().unwrap().collect();
        assert_eq!(labels.len(), 1);
        assert!(labels[0].primary());
        assert_eq!(labels[0].offset(), 8);
        assert_eq!(labels[0].label(), Some("not a field of Machine"));
    }

    #[test]
    fn file_level_diagnostics_ignore_labels() {
        let err =
            ValidationError::file(Code::Unreadable, PATH, "unreadable").label(Some(0..1), "x");
        assert!(err.labels().is_none());
    }
}
