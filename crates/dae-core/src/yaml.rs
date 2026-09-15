//! The one place that knows which YAML library is in use.

use std::borrow::Cow;
use std::ops::Range;

use serde::de::DeserializeOwned;
use serde_saphyr::{
    DefaultMessageFormatter, Localizer, Location, MessageFormatter, Options, RenderOptions,
    SnippetMode, Spanned,
};

/// A parse failure: the bare message, and where in the document it happened.
#[derive(Debug)]
pub(crate) struct YamlError {
    pub(crate) message: String,
    pub(crate) span: Option<Range<usize>>,
}

/// Parses one document. Spans are relative to `text`.
pub(crate) fn parse<T: DeserializeOwned>(text: &str) -> Result<T, YamlError> {
    serde_saphyr::from_str_with_options(text, options()).map_err(|err| YamlError {
        message: err.render_with_options(bare()),
        span: err.location().and_then(|loc| span_of_location(loc, text)),
    })
}

/// The first key or scalar in a document, for problems with the document as a
/// whole — a missing `kind`, say — that have no better place to point.
pub(crate) fn first_token(text: &str) -> Option<Range<usize>> {
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start_matches([' ', '\t', '\u{feff}']);
        let indent = line.len() - trimmed.len();
        let body = trimmed.strip_prefix("---").unwrap_or(trimmed).trim_start();
        let skipped = trimmed.len() - body.len();
        if !body.is_empty() && !body.starts_with('#') && !body.starts_with('%') {
            let start = line_start + indent + skipped;
            let len = body
                .find(|c: char| c.is_whitespace() || c == ':')
                .unwrap_or(body.len());
            return Some(start..start + len.max(1));
        }
        line_start += line.len();
    }
    None
}

/// Byte range of a spanned value, relative to the document it was parsed from.
pub(crate) fn span<T>(value: &Spanned<T>) -> Option<Range<usize>> {
    byte_range(value.referenced)
}

fn options() -> Options {
    let mut options = Options::default();
    // Only `true` and `false` are booleans. YAML 1.1 also accepted yes/no/on/off,
    // which turns `enabled: no` — or a value that merely looks like one — into
    // a boolean nobody meant.
    options.strict_booleans = true;
    // Locations and source snippets are rendered by miette, with the whole
    // file as context, not by the parser.
    options.with_snippet = false;
    options
}

fn byte_range(location: Location) -> Option<Range<usize>> {
    if location == Location::UNKNOWN {
        return None;
    }
    let span = location.span();
    let start = usize::try_from(span.byte_offset()?).ok()?;
    let len = usize::try_from(span.byte_len()?).ok()?;
    Some(start..start + len)
}

/// Errors often carry a zero-length position. Widen it to the token there —
/// a key, a scalar — so the underline covers something readable.
fn span_of_location(location: Location, text: &str) -> Option<Range<usize>> {
    let range = byte_range(location)?;
    if !range.is_empty() {
        return Some(range);
    }
    let rest = text.get(range.start..)?;
    let len = rest
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || matches!(c, ':' | ',' | '[' | ']' | '{' | '}' | '#'))
        .map_or(rest.len(), |(i, _)| i);
    Some(range.start..range.start + len)
}

/// Renders messages without the " at line N, column M" suffix, and with
/// control characters from the input neutralised by the library.
fn bare() -> RenderOptions<'static> {
    let mut options = RenderOptions::new(&BareMessages);
    options.snippets = SnippetMode::Off;
    options
}

struct BareMessages;

impl MessageFormatter for BareMessages {
    fn localizer(&self) -> &dyn Localizer {
        &NoLocationSuffix
    }

    fn format_message<'a>(&self, err: &'a serde_saphyr::Error) -> Cow<'a, str> {
        DefaultMessageFormatter.format_message(err)
    }
}

struct NoLocationSuffix;

impl Localizer for NoLocationSuffix {
    fn attach_location<'a>(&self, base: Cow<'a, str>, _: Location) -> Cow<'a, str> {
        base
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Spec {
        provider: Spanned<String>,
        #[serde(default)]
        discard: bool,
    }

    #[test]
    fn messages_have_no_location_suffix() {
        let err = parse::<Spec>("provider: a\nmemroy: 1\n").unwrap_err();
        assert!(err.message.starts_with("unknown field `memroy`"), "{err:?}");
        assert!(!err.message.contains("line"), "{err:?}");
    }

    #[test]
    fn error_spans_cover_the_offending_token() {
        let text = "# Ünïcödé\nprovider: a\nmemroy: 1\n";
        let err = parse::<Spec>(text).unwrap_err();
        assert_eq!(&text[err.span.unwrap()], "memroy");
    }

    #[test]
    fn spanned_values_carry_byte_ranges() {
        let text = "# Ünïcödé\nprovider: pve-main\n";
        let spec = parse::<Spec>(text).unwrap();
        assert_eq!(&text[span(&spec.provider).unwrap()], "pve-main");
    }

    #[test]
    fn only_true_and_false_are_booleans() {
        assert!(parse::<Spec>("provider: a\ndiscard: true\n").is_ok());
        assert!(parse::<Spec>("provider: a\ndiscard: yes\n").is_err());
        assert!(parse::<Spec>("provider: a\ndiscard: on\n").is_err());
    }

    #[test]
    fn control_characters_in_messages_are_neutralised() {
        let err = parse::<Spec>("provider: a\n\"mem\\e[31mroy\": 1\n").unwrap_err();
        assert!(!err.message.contains('\u{1b}'), "{:?}", err.message);
    }
}
