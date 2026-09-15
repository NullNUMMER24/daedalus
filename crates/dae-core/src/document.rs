//! Splitting a file into YAML documents.
//!
//! Each document is parsed on its own, so a syntax error in one does not hide
//! the others, and a document can be read twice — once to learn its `kind`,
//! once as that kind — with spans still pointing into the whole file.

/// One document: its text, and where that text starts in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Document<'a> {
    pub(crate) offset: usize,
    pub(crate) text: &'a str,
}

/// Splits `text` at `---` and `...` markers, returning only documents that
/// contain something other than comments.
///
/// This works on lines rather than a full parse, which is safe because YAML
/// forbids a line starting with `---` or `...` (followed by whitespace or end
/// of line) anywhere inside a scalar: such a line always ends the document.
/// Leading comments and `%` directives stay with the document that follows
/// them.
pub(crate) fn split(text: &str) -> Vec<Document<'_>> {
    let mut documents = Vec::new();
    let mut start = 0;
    let mut has_content = false;
    let mut has_marker = false;
    let mut line_start = 0;

    for raw_line in text.split_inclusive('\n') {
        let line_end = line_start + raw_line.len();
        let line = if line_start == 0 {
            raw_line.strip_prefix('\u{feff}').unwrap_or(raw_line)
        } else {
            raw_line
        };

        match marker(line) {
            Some((Marker::Start, rest)) => {
                if has_content || has_marker {
                    if has_content {
                        documents.push(Document {
                            offset: start,
                            text: &text[start..line_start],
                        });
                    }
                    start = line_start;
                }
                has_marker = true;
                has_content = is_content(rest);
            }
            Some((Marker::End, _)) => {
                if has_content {
                    documents.push(Document {
                        offset: start,
                        text: &text[start..line_start],
                    });
                }
                start = line_end;
                has_content = false;
                has_marker = false;
            }
            None => has_content |= is_content(line) && !line.starts_with('%'),
        }
        line_start = line_end;
    }

    if has_content {
        documents.push(Document {
            offset: start,
            text: &text[start..],
        });
    }
    documents
}

enum Marker {
    Start,
    End,
}

/// A `---` or `...` marker at the start of `line`, and whatever follows it.
fn marker(line: &str) -> Option<(Marker, &str)> {
    let (kind, rest) = if let Some(rest) = line.strip_prefix("---") {
        (Marker::Start, rest)
    } else {
        (Marker::End, line.strip_prefix("...")?)
    };
    match rest.chars().next() {
        None | Some(' ' | '\t' | '\r' | '\n') => Some((kind, rest)),
        _ => None,
    }
}

fn is_content(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && !trimmed.starts_with('#')
}

#[cfg(test)]
mod tests {
    use serde::de::IgnoredAny;

    use super::*;

    fn texts(input: &str) -> Vec<&str> {
        split(input).into_iter().map(|d| d.text).collect()
    }

    /// Every case must agree with the real parser on how many non-empty
    /// documents there are, and every slice must parse on its own.
    const CORPUS: &[(&str, &str, usize)] = &[
        ("empty file", "", 0),
        ("only comments", "# nothing\n\n# here\n", 0),
        ("one document, no markers", "kind: A\n", 1),
        ("no trailing newline", "kind: A", 1),
        ("leading marker", "---\nkind: A\n", 1),
        ("two documents", "kind: A\n---\nkind: B\n", 2),
        (
            "comment-only document between",
            "---\nkind: A\n---\n# gap\n---\nkind: B\n",
            2,
        ),
        ("end markers", "kind: A\n...\n---\nkind: B\n...\n", 2),
        ("empty documents", "---\n---\nkind: A\n---\n", 1),
        (
            "content on the marker line",
            "--- {kind: A}\n--- {kind: B}\n",
            2,
        ),
        (
            "leading comments",
            "# header\n# more\nkind: A\n---\nkind: B\n",
            2,
        ),
        ("directive", "%YAML 1.2\n---\nkind: A\n", 1),
        (
            "indented --- inside a block scalar",
            "kind: A\nnote: |\n  ---\n  not a marker\n",
            1,
        ),
        (
            "marker-like text that is not a marker",
            "kind: '---x'\n----: 1\n",
            1,
        ),
        ("CRLF line endings", "kind: A\r\n---\r\nkind: B\r\n", 2),
        ("byte-order mark", "\u{feff}---\nkind: A\n---\nkind: B\n", 2),
    ];

    #[test]
    fn agrees_with_the_yaml_parser() {
        for (name, input, expected) in CORPUS {
            let docs = split(input);
            assert_eq!(docs.len(), *expected, "{name}: {:?}", texts(input));

            let parsed = serde_saphyr::from_multiple::<IgnoredAny>(input)
                .unwrap_or_else(|e| panic!("{name}: corpus must be valid YAML: {e}"));
            assert_eq!(docs.len(), parsed.len(), "{name}: parser disagrees");

            for doc in docs {
                assert_eq!(&input[doc.offset..doc.offset + doc.text.len()], doc.text);
                serde_saphyr::from_str::<IgnoredAny>(doc.text)
                    .unwrap_or_else(|e| panic!("{name}: slice {:?} does not parse: {e}", doc.text));
            }
        }
    }

    #[test]
    fn offsets_point_into_the_original_text() {
        let input = "# Ünïcödé\nkind: A\n---\nkind: B\n";
        let docs = split(input);
        assert_eq!(docs[0].offset, 0);
        assert_eq!(docs[1].offset, input.find("---").unwrap());
    }

    #[test]
    fn a_broken_document_does_not_swallow_its_neighbours() {
        let input = "kind: A\n---\nkind: [unclosed\n---\nkind: C\n";
        assert_eq!(
            texts(input),
            ["kind: A\n", "---\nkind: [unclosed\n", "---\nkind: C\n"]
        );
    }
}
