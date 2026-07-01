use redos_detector::{is_safe, is_safe_pattern, Config, Error, Score};

fn check(source: &str, flags: &str) -> redos_detector::Report {
    let config = Config {
        max_score: None,
        max_steps: Some(5000),
        ..Config::default()
    };
    is_safe(source, flags, &config).unwrap()
}

#[test]
fn safe_simple() {
    let r = check("^a*ba*$", "");
    assert!(r.error.is_none(), "expected no error");
    assert!(
        r.trails.is_empty(),
        "expected no trails, got {}",
        r.trails.len()
    );
    assert_eq!(r.score, Score::Finite(1));
    assert!(r.is_safe());
}

#[test]
fn unsafe_simple() {
    let r = check("^a*b?a*$", "");
    assert!(!r.trails.is_empty(), "expected trails");
}

#[test]
fn nested_quantifier_unsafe() {
    let r = check("^(a*)*$", "");
    assert!(!r.trails.is_empty());
}

#[test]
fn star_star_safe() {
    let r = check("^(a*)*", "");
    assert!(r.error.is_none());
    assert!(r.trails.is_empty());
    assert_eq!(r.score, Score::Finite(1));
}

#[test]
fn unsupported_reference_returns_error() {
    let config = Config {
        downgrade_pattern: false,
        ..Config::default()
    };
    for pattern in [r"^(a+)\1", r"^(\w+)\1$"] {
        assert_eq!(
            is_safe_pattern(pattern, &config),
            Err(Error::UnsupportedReference),
            "pattern {pattern:?} should return an error"
        );
    }
}
