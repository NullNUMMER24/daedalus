//! "Did you mean …?"

/// The candidate most similar to `input`, if any is similar enough to be a
/// plausible typo. Case-only differences win outright.
pub(crate) fn did_you_mean<'a>(
    input: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut best: Option<(&str, f64)> = None;
    for candidate in candidates {
        if candidate == input {
            continue;
        }
        if candidate.eq_ignore_ascii_case(input) {
            return Some(candidate);
        }
        let score = strsim::normalized_damerau_levenshtein(input, candidate);
        if score >= 0.7 && best.is_none_or(|(_, b)| score > b) {
            best = Some((candidate, score));
        }
    }
    best.map(|(candidate, _)| candidate)
}

/// `a, b, c` — or `a, b, c and 4 more` past `max`.
pub(crate) fn list<S: AsRef<str>>(items: &[S], max: usize) -> String {
    let shown: Vec<_> = items
        .iter()
        .take(max)
        .map(|s| format!("`{}`", s.as_ref()))
        .collect();
    let mut out = shown.join(", ");
    if items.len() > max {
        out = format!("{out} and {} more", items.len() - max);
    }
    out
}

/// The name and the allowed alternatives from a serde "unknown field" or
/// "unknown variant" message, e.g.
/// ``unknown field `memroy`, expected one of memory, provider``.
pub(crate) fn parse_unknown(message: &str) -> Option<(&str, Vec<&str>)> {
    let rest = message
        .strip_prefix("unknown field `")
        .or_else(|| message.strip_prefix("unknown variant `"))?;
    let (found, rest) = rest.split_once('`')?;
    let expected = rest
        .strip_prefix(", expected ")
        .map(|list| {
            let list = list.strip_prefix("one of ").unwrap_or(list);
            list.split(", ")
                .flat_map(|part| part.split(" or "))
                .map(|name| name.trim().trim_matches('`'))
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Some((found, expected))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[test]
    fn suggests_close_matches_only() {
        let kinds = ["Machine", "MachineClass", "Network", "Provider"];
        assert_eq!(did_you_mean("Machnie", kinds), Some("Machine"));
        assert_eq!(did_you_mean("machine", kinds), Some("Machine"));
        assert_eq!(did_you_mean("Netwrok", kinds), Some("Network"));
        assert_eq!(did_you_mean("Cluster", kinds), None);
        assert_eq!(
            did_you_mean("prod-nett", ["prod-net", "mgmt-net"]),
            Some("prod-net")
        );
        assert_eq!(did_you_mean("x", Vec::<&str>::new()), None);
    }

    #[test]
    fn lists_with_a_limit() {
        assert_eq!(list(&["a", "b"], 5), "`a`, `b`");
        assert_eq!(list(&["a", "b", "c", "d"], 2), "`a`, `b` and 2 more");
        assert_eq!(list::<&str>(&[], 5), "");
    }

    /// Checks the parser against messages the YAML library actually produces,
    /// so a wording change upstream fails here rather than silently dropping
    /// suggestions.
    #[test]
    fn parses_real_unknown_field_and_variant_messages() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct One {
            memory: u8,
        }
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct Two {
            memory: u8,
            provider: u8,
        }
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct Three {
            memory: u8,
            provider: u8,
            disks: u8,
        }
        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum Type {
            Proxmox,
            Libvirt,
        }
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct Provider {
            r#type: Type,
        }

        fn msg<T: for<'de> Deserialize<'de> + std::fmt::Debug>(yaml: &str) -> String {
            crate::yaml::parse::<T>(yaml).unwrap_err().message
        }

        let cases = [
            (msg::<One>("memroy: 1\n"), vec!["memory"]),
            (msg::<Two>("memroy: 1\n"), vec!["memory", "provider"]),
            (
                msg::<Three>("memroy: 1\n"),
                vec!["memory", "provider", "disks"],
            ),
            (
                msg::<Provider>("type: proxmx\n"),
                vec!["proxmox", "libvirt"],
            ),
        ];
        for (message, expected) in cases {
            let (found, alternatives) =
                parse_unknown(&message).unwrap_or_else(|| panic!("unparsed: {message:?}"));
            assert!(found == "memroy" || found == "proxmx", "{message:?}");
            assert_eq!(alternatives, expected, "{message:?}");
        }
    }
}
