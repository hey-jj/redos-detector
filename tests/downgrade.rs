//! Downgrade rewrites with exact expected patterns and atomic offsets.

use redos_detector::downgrade_pattern;

fn expect(pattern: &str, expected_pattern: &str, expected_offsets: &[usize]) {
    let result = downgrade_pattern(pattern, false).unwrap();
    assert_eq!(result.pattern, expected_pattern, "pattern for {pattern:?}");
    let mut offsets: Vec<usize> = result.atomic_group_offsets.into_iter().collect();
    offsets.sort_unstable();
    assert_eq!(offsets, expected_offsets, "offsets for {pattern:?}");
}

#[test]
fn does_not_downgrade_when_not_needed() {
    expect("^ab", "^ab", &[]);
    expect("^a(b)c\\1", "^a(b)c\\1", &[]);
    expect("^a(?=(b)\\1)c", "^a(?=(b)\\1)c", &[]);
    expect("^a(?=(b(?=(c\\1))))", "^a(?=(b(?=(c\\1))))", &[]);
    expect("^(a+\\1)", "^(a+\\1)", &[]);
}

#[test]
fn downgrades_when_group_in_positive_lookahead() {
    expect("^a(?=(b))c\\1", "^a(?=(b))c(?:b)", &[]);
    expect("^a(?=(b*))c\\1", "^a(?=(b*))c(?:b*)", &[]);
    expect("^a(?=(b))c\\1+", "^a(?=(b))c(?:b)+", &[]);
    expect("^a(?=(b)\\1)\\1c", "^a(?=(b)\\1)(?:b)c", &[]);
    expect("^a(?=(b))(c\\1)", "^a(?=(b))(c(?:b))", &[]);
    expect("^a(?=(b))((c\\1))", "^a(?=(b))((c(?:b)))", &[]);
    expect("^a(?=(b))(c\\1)\\2", "^a(?=(b))(c(?:b))\\2", &[]);
    expect("^a(b)(?=(c))(d\\1)\\2", "^a(b)(?=(c))(d\\1)(?:c)", &[]);
    expect("^a(?=(b(?=c)d))\\1", "^a(?=(b(?=c)d))(?:bd)", &[15]);
    expect(
        "^a(?=(b(?=(c\\1))))\\1\\2",
        "^a(?=(b(?=(c\\1))))(?:b)(?:c(?:b))",
        &[18],
    );
    expect("^a(?=(b)?)c\\1", "^a(?=(b)?)c(?:(?:b)?)", &[]);
    expect("^a(?=(b)|c)d\\1", "^a(?=(b)|c)d(?:(?:b)?)", &[]);
    expect("^a(?=(?:(b)|c))d\\1", "^a(?=(?:(b)|c))d(?:(?:b)?)", &[]);
    expect("^a(?=(?:(b))?)d\\1", "^a(?=(?:(b))?)d(?:(?:b)?)", &[]);
    expect("^a(?=(b{1,}?))c\\1", "^a(?=(b{1,}?))c(?:b{1,}?)", &[]);
    expect(
        "^(?=(a))\\1(?=(b))\\2",
        "^(?=(a))(?:a)(?=(b))(?:b)",
        &[8, 20],
    );
    expect("^(?:(?=(a))\\1)", "^(?:(?=(a))(?:a))", &[11]);
    expect("^((?=(a))\\2){1,2}", "^((?=(a))(?:a)){1,2}", &[9]);
    expect(
        "^(?=(a))(?=(b\\1))\\2",
        "^(?=(a))(?=(b(?:a)))(?:b(?:a))",
        &[20],
    );
    expect("^(?=(a))\\1(?=(b))\\1", "^(?=(a))(?:a)(?=(b))(?:a)", &[8]);
    expect(
        "^(?=(a))(?=(\\1))(?=(c))\\3\\2",
        "^(?=(a))(?=((?:a)))(?=(c))(?:c)(?:(?:a))",
        &[26],
    );
    expect("^a(?=(b(?!(c))))c\\1", "^a(?=(b(?!(c))))c(?:b)", &[]);
}

#[test]
fn handles_nested_atomic_group() {
    expect(
        "^(?=((?=(a*))\\2b*))\\1c*$",
        "^(?=((?=(a*))(?:a*)b*))(?:(?:a*)b*)c*$",
        &[13, 23, 26],
    );
}

#[test]
fn does_not_downgrade_when_group_in_negative_lookahead() {
    expect("^a(?!(b))c\\1", "^a(?!(b))c\\1", &[]);
    expect("^a(?=(b(?!(c))))c\\2", "^a(?=(b(?!(c))))c\\2", &[]);
    expect("^a(?!(b(?=(c))))c\\2", "^a(?!(b(?=(c))))c\\2", &[]);
}

#[test]
fn downgrades_when_group_is_non_finite() {
    expect("^(a*)\\1", "^(a*)(?:a*)", &[]);
    expect("^(a+)\\1", "^(a+)(?:a+)", &[]);
    expect("^(a+(?=b+))\\1", "^(a+(?=b+))(?:a+)", &[]);
}

#[test]
fn does_not_downgrade_when_lookahead_group_not_finite() {
    expect("^(a(?=b+))\\1", "^(a(?=b+))\\1", &[]);
}

#[test]
fn downgrades_when_no_start_anchor() {
    expect("a", "[^]*?a", &[]);
    expect("a|b", "[^]*?(?:a|b)", &[]);
    expect("(?=(a))\\1", "[^]*?(?=(a))(?:a)", &[12]);
    expect("(?=(a))\\1|b", "[^]*?(?:(?=(a))(?:a)|b)", &[15]);
    expect("(^)?a", "[^]*?(^)?a", &[]);
    expect("^a|b", "[^]*?(?:^a|b)", &[]);
    expect("a^b", "[^]*?a^b", &[]);
    expect("(^){0}a", "[^]*?(^){0}a", &[]);
}

#[test]
fn does_not_downgrade_when_start_anchor_in_quantifier_min_above_zero() {
    expect("(^){2}a", "(^){2}a", &[]);
}

#[test]
fn does_not_downgrade_when_replacement_already_applied() {
    expect("[^]*?a", "[^]*?a", &[]);
    expect("[^]*?(?:a|b)", "[^]*?(?:a|b)", &[]);
}
