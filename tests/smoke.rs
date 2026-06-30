use redos_detector::{is_safe, Config, Score};

fn check(source: &str, flags: &str) -> redos_detector::Report {
    let config = Config {
        max_score: f64::INFINITY,
        max_steps: 5000.0,
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
