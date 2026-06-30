//! Code point helpers and ECMA-262 simple case folding.

use crate::our_range::{create_ranges, OurRange};
use std::collections::HashSet;

/// Returns the single code point of `input`, or `None` when `input` spans more
/// than one UTF-16 code unit.
///
/// This mirrors the ECMA-262 `Canonicalize` rule: a case mapping that produces
/// more than one code unit does not canonicalize, so the caller keeps the
/// original.
pub(crate) fn to_code_point(input: &str) -> Option<u32> {
    if input.encode_utf16().count() > 1 {
        return None;
    }
    let code_point = input.chars().next();
    match code_point {
        Some(c) => Some(c as u32),
        None => panic!("Internal error: expected codepoint"),
    }
}

/// Canonicalizes a code point to its simple uppercase form.
///
/// Uses full Unicode uppercase mapping but keeps the original when uppercasing
/// changes the UTF-16 length (for example `ß` uppercases to `SS`).
pub(crate) fn to_upper_case_code_point(code_point: u32) -> u32 {
    let ch = match char::from_u32(code_point) {
        Some(c) => c,
        None => return code_point,
    };
    let upper: String = ch.to_uppercase().collect();
    to_code_point(&upper).unwrap_or(code_point)
}

/// Builds the code point ranges a single literal or class range covers.
///
/// Without `case_insensitive` this is just `[low, high]`. With it, every code
/// point in the span is folded to uppercase and the results are coalesced into
/// sorted ranges.
pub(crate) fn build_code_point_ranges(
    case_insensitive: bool,
    low_code_point: u32,
    high_code_point: u32,
) -> Vec<OurRange> {
    if !case_insensitive {
        return vec![(low_code_point as f64, high_code_point as f64)];
    }

    let mut code_points: HashSet<i64> = HashSet::new();
    for code_point in low_code_point..=high_code_point {
        code_points.insert(to_upper_case_code_point(code_point) as i64);
    }
    create_ranges(&code_points)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(input: &str) -> u32 {
        to_code_point(input).expect("Did not map to one code point")
    }

    #[test]
    fn build_ranges_works() {
        assert_eq!(
            build_code_point_ranges(true, cp("a"), cp("a")),
            vec![(cp("A") as f64, cp("A") as f64)]
        );
        assert_eq!(
            build_code_point_ranges(true, cp("A"), cp("a")),
            vec![(cp("A") as f64, cp("`") as f64)]
        );
        assert_eq!(
            build_code_point_ranges(true, cp("C"), cp("a")),
            vec![
                (cp("A") as f64, cp("A") as f64),
                (cp("C") as f64, cp("`") as f64)
            ]
        );
        assert_eq!(
            build_code_point_ranges(true, cp("["), cp("]")),
            vec![(cp("[") as f64, cp("]") as f64)]
        );
        assert_eq!(
            build_code_point_ranges(true, cp("Z"), cp("}")),
            vec![
                (cp("A") as f64, cp("`") as f64),
                (cp("{") as f64, cp("}") as f64)
            ]
        );
        assert_eq!(
            build_code_point_ranges(false, cp("Z"), cp("}")),
            vec![(cp("Z") as f64, cp("}") as f64)]
        );
        assert_eq!(
            build_code_point_ranges(true, cp("Ω"), cp("Ω")),
            vec![(cp("Ω") as f64, cp("Ω") as f64)]
        );
        assert_eq!(
            build_code_point_ranges(true, cp("ß"), cp("ß")),
            vec![(cp("ß") as f64, cp("ß") as f64)]
        );
    }

    #[test]
    fn to_code_point_rejects_astral() {
        assert_eq!(to_code_point("👍"), None);
        assert_eq!(to_code_point("a"), Some(0x61));
    }
}
