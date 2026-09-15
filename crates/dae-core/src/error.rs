//! Errors produced by the domain model.

use thiserror::Error;

use crate::ValidationError;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Code;

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
            ValidationError::file(Code::Unreadable, MANIFEST, "not UTF-8"),
            ValidationError::file(Code::Unreadable, MANIFEST, "not UTF-8 either"),
        ]);
        assert_eq!(err.to_string(), "validation failed with 2 error(s)");

        let CoreError::Validation(errors) = err else {
            panic!("expected CoreError::Validation");
        };
        assert_eq!(errors.len(), 2, "no error may be dropped");
    }
}
