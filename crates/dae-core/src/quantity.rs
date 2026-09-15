//! Byte quantities: `8Gi`, `500M`, `1073741824`.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserialize, Deserializer, Visitor};

use crate::CoreError;

/// A number of bytes, written with Kubernetes-style suffixes.
///
/// Binary: `Ki` `Mi` `Gi` `Ti` `Pi` `Ei` (powers of 1024).
/// Decimal: `k` `M` `G` `T` `P` `E` (powers of 1000).
/// No suffix means bytes. Fractions are allowed when the result is a whole
/// number of bytes: `1.5Gi` is fine, `0.1Ki` is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ByteSize(u64);

const BINARY: [(&str, u32); 6] = [
    ("Ki", 10),
    ("Mi", 20),
    ("Gi", 30),
    ("Ti", 40),
    ("Pi", 50),
    ("Ei", 60),
];
const DECIMAL: [(&str, u32); 6] = [
    ("k", 3),
    ("M", 6),
    ("G", 9),
    ("T", 12),
    ("P", 15),
    ("E", 18),
];

impl ByteSize {
    #[must_use]
    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(self) -> u64 {
        self.0
    }

    /// Whole mebibytes, rounded down. Proxmox takes memory in MiB.
    #[must_use]
    pub const fn as_mebibytes(self) -> u64 {
        self.0 >> 20
    }

    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub fn parse(input: &str) -> Result<Self, CoreError> {
        parse(input).map_err(|reason| CoreError::InvalidQuantity {
            input: input.to_owned(),
            reason,
        })
    }
}

fn parse(input: &str) -> Result<ByteSize, String> {
    if input.is_empty() {
        return Err("must not be empty".into());
    }
    if input.contains(char::is_whitespace) {
        return Err(format!(
            "must not contain spaces: write `{}`",
            input.split_whitespace().collect::<String>()
        ));
    }
    if input.starts_with('-') {
        return Err("must not be negative".into());
    }

    let split = input
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(input.len());
    let (number, unit) = input.split_at(split);
    if number.is_empty() {
        return Err("must start with a number, e.g. `8Gi`".into());
    }

    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty() || fraction.contains('.') || (number.contains('.') && fraction.is_empty()) {
        return Err(format!("`{number}` is not a number"));
    }

    let multiplier = unit_multiplier(unit)?;
    let too_large = || format!("is larger than the maximum of {} bytes", u64::MAX);

    // Exact integer arithmetic: value = (whole.fraction) * multiplier.
    let scale = 10u128
        .checked_pow(u32::try_from(fraction.len()).map_err(|_| too_large())?)
        .ok_or_else(too_large)?;
    let whole: u128 = whole.parse().map_err(|_| too_large())?;
    let fraction: u128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().map_err(|_| too_large())?
    };
    let scaled = whole
        .checked_mul(scale)
        .and_then(|v| v.checked_add(fraction))
        .and_then(|v| v.checked_mul(multiplier))
        .ok_or_else(too_large)?;
    if scaled % scale != 0 {
        return Err("is not a whole number of bytes".into());
    }
    u64::try_from(scaled / scale)
        .map(ByteSize)
        .map_err(|_| too_large())
}

fn unit_multiplier(unit: &str) -> Result<u128, String> {
    if unit.is_empty() {
        return Ok(1);
    }
    if let Some((_, exp)) = BINARY.iter().find(|(u, _)| *u == unit) {
        return Ok(1u128 << exp);
    }
    if let Some((_, exp)) = DECIMAL.iter().find(|(u, _)| *u == unit) {
        return Ok(10u128.pow(*exp));
    }

    // Unknown. Work out what the author probably meant.
    let stem = unit
        .strip_suffix("iB")
        .or_else(|| unit.strip_suffix('B'))
        .unwrap_or(unit);
    if stem != unit {
        let stem = stem.strip_suffix('i').unwrap_or(stem);
        let decimal = if stem.eq_ignore_ascii_case("k") {
            "k".to_owned()
        } else {
            stem.to_ascii_uppercase()
        };
        if DECIMAL.iter().any(|(u, _)| *u == decimal) {
            return Err(format!(
                "`{unit}` is not a unit: use `{decimal}` (powers of 1000) or `{}i` (powers of 1024)",
                stem.to_ascii_uppercase()
            ));
        }
    }
    let all = BINARY.iter().chain(DECIMAL.iter()).map(|(u, _)| *u);
    if let Some(right) = all.clone().find(|u| u.eq_ignore_ascii_case(unit)) {
        return Err(format!("units are case-sensitive: use `{right}`"));
    }
    Err(format!(
        "unknown unit `{}`; expected one of {}",
        unit.escape_debug(),
        all.collect::<Vec<_>>().join(", ")
    ))
}

impl FromStr for ByteSize {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// The exact form with the fewest digits, preferring binary units on a tie:
/// `8Gi`, `16G`, `1536Mi`, `1023`. Trying binary units first is not enough —
/// 16G is 2^13 * 5^9 bytes, divisible by 1024, and would print as `15625000Ki`.
///
/// `ByteSize::parse(&x.to_string()) == x` always holds.
impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0;
        if bytes == 0 {
            return f.write_str("0");
        }
        let largest_exact = |units: &[(&'static str, u64)]| {
            units
                .iter()
                .rev()
                .find(|(_, size)| bytes.is_multiple_of(*size))
                .map(|(unit, size)| (bytes / size, *unit))
        };
        let binary: Vec<_> = BINARY.iter().map(|(u, e)| (*u, 1u64 << e)).collect();
        let decimal: Vec<_> = DECIMAL.iter().map(|(u, e)| (*u, 10u64.pow(*e))).collect();

        // min_by_key keeps the first of equal elements, so order is the tie-break.
        let (count, unit) = [largest_exact(&binary), largest_exact(&decimal)]
            .into_iter()
            .flatten()
            .chain([(bytes, "")])
            .min_by_key(|(count, _)| count.ilog10())
            .unwrap_or((bytes, ""));
        write!(f, "{count}{unit}")
    }
}

/// Accepts a string (`"8Gi"`) or a plain integer number of bytes.
impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ByteSizeVisitor;

        impl Visitor<'_> for ByteSizeVisitor {
            type Value = ByteSize;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a byte quantity such as `8Gi` or `500M`")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                ByteSize::parse(v).map_err(E::custom)
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(ByteSize(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                u64::try_from(v)
                    .map(ByteSize)
                    .map_err(|_| E::custom(format!("invalid quantity `{v}`: must not be negative")))
            }
        }

        deserializer.deserialize_any(ByteSizeVisitor)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const KI: u64 = 1 << 10;
    const MI: u64 = 1 << 20;
    const GI: u64 = 1 << 30;

    fn bytes(input: &str) -> u64 {
        ByteSize::parse(input)
            .unwrap_or_else(|e| panic!("{input:?}: {e}"))
            .as_bytes()
    }

    fn reason(input: &str) -> String {
        match ByteSize::parse(input) {
            Err(CoreError::InvalidQuantity { reason, .. }) => reason,
            other => panic!("expected InvalidQuantity for {input:?}, got {other:?}"),
        }
    }

    #[test]
    fn parses_binary_decimal_and_plain_bytes() {
        assert_eq!(bytes("0"), 0);
        assert_eq!(bytes("1024"), KI);
        assert_eq!(bytes("8Gi"), 8 * GI);
        assert_eq!(bytes("512Mi"), 512 * MI);
        assert_eq!(bytes("1Ti"), 1 << 40);
        assert_eq!(bytes("1k"), 1000);
        assert_eq!(bytes("500M"), 500_000_000);
        assert_eq!(bytes("16G"), 16_000_000_000);
        assert_eq!(bytes("15Ei"), 15 << 60);
    }

    #[test]
    fn parses_fractions_that_are_whole_bytes() {
        assert_eq!(bytes("1.5Gi"), GI + GI / 2);
        assert_eq!(bytes("0.5Ki"), 512);
        assert_eq!(bytes("2.25M"), 2_250_000);
    }

    #[test]
    fn rejects_fractions_of_a_byte() {
        assert_eq!(reason("0.1Ki"), "is not a whole number of bytes");
        assert_eq!(reason("1.5"), "is not a whole number of bytes");
    }

    #[test]
    fn explains_byte_suffixes() {
        assert_eq!(
            reason("16GB"),
            "`GB` is not a unit: use `G` (powers of 1000) or `Gi` (powers of 1024)"
        );
        assert_eq!(
            reason("16GiB"),
            "`GiB` is not a unit: use `G` (powers of 1000) or `Gi` (powers of 1024)"
        );
        assert_eq!(
            reason("512kB"),
            "`kB` is not a unit: use `k` (powers of 1000) or `Ki` (powers of 1024)"
        );
    }

    #[test]
    fn explains_case_mistakes() {
        assert_eq!(reason("8gi"), "units are case-sensitive: use `Gi`");
        assert_eq!(reason("8K"), "units are case-sensitive: use `k`");
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(reason(""), "must not be empty");
        assert_eq!(reason("16 Gi"), "must not contain spaces: write `16Gi`");
        assert_eq!(reason("-1Gi"), "must not be negative");
        assert_eq!(reason("Gi"), "must start with a number, e.g. `8Gi`");
        assert_eq!(reason("1.2.3Gi"), "`1.2.3` is not a number");
        assert_eq!(reason("1.Gi"), "`1.` is not a number");
        assert!(reason("8Xi").starts_with("unknown unit `Xi`; expected one of Ki, Mi"));
    }

    #[test]
    fn rejects_overflow() {
        assert_eq!(
            reason("16Ei"),
            "is larger than the maximum of 18446744073709551615 bytes"
        );
        assert!(reason(&"9".repeat(60)).starts_with("is larger than"));
    }

    #[test]
    fn displays_the_shortest_exact_form() {
        let show = |b: u64| ByteSize::from_bytes(b).to_string();
        assert_eq!(show(0), "0");
        assert_eq!(show(8 * GI), "8Gi");
        assert_eq!(show(1536 * MI), "1536Mi");
        assert_eq!(show(16_000_000_000), "16G");
        assert_eq!(show(1000), "1k");
        assert_eq!(show(1023), "1023");
        assert_eq!(show(1024), "1Ki");
        assert_eq!(show(500_000_000), "500M");
    }

    /// Whatever someone writes in a manifest should come back out unchanged
    /// when it is already the natural way to write that size.
    #[test]
    fn natural_spellings_survive_a_round_trip() {
        for written in [
            "8Gi", "16G", "500M", "512Mi", "1Ti", "40Gi", "1k", "100G", "2Ti",
        ] {
            assert_eq!(ByteSize::parse(written).unwrap().to_string(), written);
        }
    }

    #[test]
    fn converts_to_mebibytes_for_proxmox() {
        assert_eq!(ByteSize::parse("16Gi").unwrap().as_mebibytes(), 16_384);
    }

    #[test]
    fn escapes_control_characters_in_errors() {
        let err = ByteSize::parse("8\u{1b}Gi").unwrap_err().to_string();
        assert!(!err.contains('\u{1b}'), "raw escape in {err:?}");
    }

    proptest! {
        #[test]
        fn display_round_trips(b in any::<u64>()) {
            let size = ByteSize::from_bytes(b);
            prop_assert_eq!(ByteSize::parse(&size.to_string()).unwrap(), size);
        }

        #[test]
        fn parse_never_panics(input in any::<String>()) {
            let _ = ByteSize::parse(&input);
        }

        #[test]
        fn suffix_arithmetic_is_exact(n in 0u64..1024, unit in 0usize..4) {
            let (suffix, exp) = BINARY[unit];
            prop_assert_eq!(bytes(&format!("{n}{suffix}")), n << exp);
        }
    }
}
