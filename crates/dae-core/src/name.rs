//! Resource names.

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer};

use crate::CoreError;

/// A resource name: an RFC 1123 DNS label.
///
/// 1–63 characters of lowercase ASCII letters, digits and hyphens, not starting
/// or ending with a hyphen. The same rule Kubernetes uses, so a name valid here
/// is valid as a hostname, a Proxmox VM name, and a Kubernetes object name.
///
/// A `Name` that exists is valid: the only ways to build one are [`Name::parse`]
/// and deserialization, which goes through it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

impl Name {
    pub const MAX_LEN: usize = 63;

    pub fn parse(input: &str) -> Result<Self, CoreError> {
        match problem(input) {
            None => Ok(Self(input.to_owned())),
            Some(reason) => Err(CoreError::InvalidName {
                name: input.to_owned(),
                reason,
            }),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why `input` is not a valid name, or `None` if it is.
fn problem(input: &str) -> Option<String> {
    if input.is_empty() {
        return Some("must not be empty".into());
    }
    let len = input.chars().count();
    if len > Name::MAX_LEN {
        return Some(format!(
            "is {len} characters long; the maximum is {}",
            Name::MAX_LEN
        ));
    }
    if let Some(bad) = input
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
    {
        let hint = suggest_fix(input)
            .map(|fixed| format!(" (try `{fixed}`)"))
            .unwrap_or_default();
        return Some(format!(
            "contains `{}`; only lowercase letters, digits and hyphens are allowed{hint}",
            bad.escape_debug()
        ));
    }
    if input.starts_with('-') || input.ends_with('-') {
        return Some("must not start or end with a hyphen".into());
    }
    None
}

/// `Web_01` -> `web-01`, if that is a valid name.
pub(crate) fn suggest_fix(input: &str) -> Option<String> {
    let fixed: String = input
        .chars()
        .map(|c| match c {
            '_' | '.' | ' ' => '-',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    (fixed != input && problem(&fixed).is_none()).then_some(fixed)
}

impl FromStr for Name {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Lets a `BTreeMap<Name, _>` be queried with a `&str`.
impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn reason(input: &str) -> String {
        match Name::parse(input) {
            Err(CoreError::InvalidName { reason, .. }) => reason,
            other => panic!("expected InvalidName for {input:?}, got {other:?}"),
        }
    }

    #[test]
    fn accepts_dns_labels() {
        for ok in ["web-01", "a", "0", "db", "k3s-cp-1", &"x".repeat(63)] {
            assert_eq!(Name::parse(ok).unwrap().as_str(), ok);
        }
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(reason(""), "must not be empty");
    }

    #[test]
    fn rejects_too_long() {
        assert_eq!(
            reason(&"x".repeat(64)),
            "is 64 characters long; the maximum is 63"
        );
    }

    #[test]
    fn rejects_uppercase_and_suggests_the_lowercase_form() {
        assert_eq!(
            reason("Web01"),
            "contains `W`; only lowercase letters, digits and hyphens are allowed (try `web01`)"
        );
    }

    #[test]
    fn suggests_hyphens_for_underscores_and_dots() {
        assert!(reason("web_01").ends_with("(try `web-01`)"));
        assert!(reason("db.primary").ends_with("(try `db-primary`)"));
    }

    #[test]
    fn rejects_leading_and_trailing_hyphens() {
        assert_eq!(reason("-web"), "must not start or end with a hyphen");
        assert_eq!(reason("web-"), "must not start or end with a hyphen");
    }

    #[test]
    fn rejects_non_ascii_letters_without_a_bogus_suggestion() {
        assert_eq!(
            reason("café"),
            "contains `é`; only lowercase letters, digits and hyphens are allowed"
        );
    }

    /// A name comes from a file anyone in the tenant can edit, and error
    /// messages are printed to a terminal.
    #[test]
    fn escapes_control_characters_in_errors() {
        let err = Name::parse("web\u{1b}[31m").unwrap_err().to_string();
        assert!(!err.contains('\u{1b}'), "raw escape in {err:?}");
        assert!(err.contains(r"\u{1b}"), "escape not shown in {err:?}");
    }

    #[test]
    fn borrows_as_str_for_map_lookups() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(Name::parse("prod-net").unwrap(), 1);
        assert_eq!(map.get("prod-net"), Some(&1));
    }

    proptest! {
        #[test]
        fn every_dns_label_parses_and_displays_unchanged(
            input in "[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?"
        ) {
            prop_assert_eq!(Name::parse(&input).unwrap().to_string(), input);
        }

        #[test]
        fn parse_never_panics(input in any::<String>()) {
            let _ = Name::parse(&input);
        }
    }
}
