//! Errors produced by the domain model.

use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by the domain model.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A resource name does not satisfy the naming rules.
    #[error("invalid name `{}`: {reason}", .name.escape_debug())]
    InvalidName { name: String, reason: String },

    /// A byte quantity such as `8Gi` could not be parsed.
    #[error("invalid quantity `{}`: {reason}", .input.escape_debug())]
    InvalidQuantity { input: String, reason: String },

    /// A manifest declared a `kind` Daedalus does not know about.
    #[error("unknown kind `{}`", .0.escape_debug())]
    UnknownKind(String),

    /// One or more manifests failed validation.
    ///
    /// Carries every problem found, not just the first: users have more than
    /// one typo, and fixing them one run at a time is miserable.
    #[error("validation failed with {} error(s)", .0.len())]
    Validation(Vec<ValidationError>),
}

/// Shorthand for results whose error is a [`CoreError`].
pub type Result<T, E = CoreError> = std::result::Result<T, E>;

/// A single problem found while validating a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// The manifest the problem is in, relative to the repository root.
    pub path: PathBuf,
    /// 1-based line number, when the problem can be pinned to one.
    pub line: Option<usize>,
    /// What is wrong.
    pub message: String,
    /// How to fix it, e.g. "did you mean `memory`?".
    pub hint: Option<String>,
}

impl ValidationError {
    pub fn new(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            line: None,
            message: message.into(),
            hint: None,
        }
    }

    #[must_use]
    pub fn at_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Renders as `path:line: message (hint)`, the shape editors and terminals
/// recognise as a clickable location.
impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.path.display())?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        write!(f, ": {}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " ({hint})")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "tenants/acme/environments/prod/machines/web-01.yaml";

    #[test]
    fn invalid_name_names_the_value_and_the_rule() {
        let err = CoreError::InvalidName {
            name: "Web01".into(),
            reason: "only lowercase letters, digits and hyphens".into(),
        };
        assert_eq!(
            err.to_string(),
            "invalid name `Web01`: only lowercase letters, digits and hyphens"
        );
    }

    #[test]
    fn unknown_kind_quotes_the_kind() {
        let err = CoreError::UnknownKind("Machnie".into());
        assert_eq!(err.to_string(), "unknown kind `Machnie`");
    }

    #[test]
    fn validation_reports_how_many_errors_it_carries() {
        let err = CoreError::Validation(vec![
            ValidationError::new(MANIFEST, "unknown field `memroy`"),
            ValidationError::new(MANIFEST, "unresolved reference `prod-nett`"),
        ]);
        assert_eq!(err.to_string(), "validation failed with 2 error(s)");

        let CoreError::Validation(errors) = err else {
            panic!("expected CoreError::Validation");
        };
        assert_eq!(errors.len(), 2, "no error may be dropped");
    }

    #[test]
    fn validation_error_without_line_or_hint() {
        let err = ValidationError::new(MANIFEST, "document is empty");
        assert_eq!(err.to_string(), format!("{MANIFEST}: document is empty"));
    }

    #[test]
    fn validation_error_with_line_and_hint() {
        let err = ValidationError::new(MANIFEST, "unknown field `memroy`")
            .at_line(14)
            .with_hint("did you mean `memory`?");
        assert_eq!(
            err.to_string(),
            format!("{MANIFEST}:14: unknown field `memroy` (did you mean `memory`?)")
        );
    }
}
