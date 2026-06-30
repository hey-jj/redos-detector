//! Behavior table covering hundreds of patterns.
//!
//! Each row runs `is_safe(source, flags, {max_score: inf, max_steps: 5000})` and
//! checks the safe/unsafe verdict, the score for the exact cases, and the
//! downgrade cross-checks. The throw cases are checked by catching the panic the
//! library raises.
//!
//! `exact_trail_counts` pins the number of trails per pattern against the
//! canonical table. Verdict-only checks miss drift that changes trail
//! enumeration or score without flipping safe to unsafe, so this test guards the
//! checker against that class of regression.

mod cases_data;

use cases_data::{Exp, CASES, TRAIL_COUNTS};
use redos_detector::{downgrade_pattern, is_safe, Config, RedosDetectorResult, Score};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn run(source: &str, flags: &str) -> RedosDetectorResult {
    let config = Config {
        max_score: f64::INFINITY,
        max_steps: 5000.0,
        ..Config::default()
    };
    is_safe(source, flags, &config)
}

fn panic_message<F: FnOnce()>(f: F) -> Option<String> {
    let result = catch_unwind(AssertUnwindSafe(f));
    match result {
        Ok(()) => None,
        Err(payload) => {
            if let Some(s) = payload.downcast_ref::<String>() {
                Some(s.clone())
            } else if let Some(s) = payload.downcast_ref::<&str>() {
                Some(s.to_string())
            } else {
                Some(String::new())
            }
        }
    }
}

fn unsupported_reference_message(message: &str) -> bool {
    let a = message.starts_with("Unsupported reference (")
        && message.ends_with("). Pattern needs downgrading. See the `downgradePattern` option.");
    let b = message.starts_with("Unsupported reference to group ")
        && message.ends_with(
            " as group is not a finite size. Pattern needs downgrading. See the `downgradePattern` option.",
        );
    a || b
}

#[test]
fn behavior_table() {
    let mut failures: Vec<String> = Vec::new();

    for case in CASES {
        let label = format!("/{}/{}", case.source, case.flags);
        let result = run(case.source, case.flags);

        match case.expected {
            Exp::Safe => {
                if result.error.is_some() {
                    failures.push(format!("{label}: expected no error"));
                }
                if !result.trails.is_empty() {
                    failures.push(format!(
                        "{label}: expected 0 trails, got {}",
                        result.trails.len()
                    ));
                }
                if result.score != Score::Finite(1.0) {
                    failures.push(format!("{label}: expected score 1, got {:?}", result.score));
                }
            }
            Exp::Unsafe => {
                if result.trails.is_empty() {
                    failures.push(format!("{label}: expected trails, got 0"));
                }
            }
            Exp::Score(value) => {
                if result.score != Score::Finite(value) {
                    failures.push(format!(
                        "{label}: expected score {value}, got {:?}",
                        result.score
                    ));
                }
            }
        }

        if case.infinite && result.score != Score::Infinite {
            failures.push(format!(
                "{label}: expected infinite score, got {:?}",
                result.score
            ));
        }

        if result.safe != result.error.is_none() {
            failures.push(format!("{label}: safe must equal error.is_none()"));
        }

        // Downgrade cross-check.
        let unicode = case.flags.contains('u');
        let downgraded = downgrade_pattern(case.source, unicode);
        let needed_downgrade = downgraded.pattern != case.source;

        if case.missing_anchor {
            if !needed_downgrade {
                failures.push(format!("{label}: expected downgrade to be needed"));
            }
            let msg = panic_message(|| {
                let config = Config {
                    downgrade_pattern: false,
                    ..Config::default()
                };
                let _ = is_safe(case.source, case.flags, &config);
            });
            match msg {
                Some(m)
                    if m == "Pattern is not bounded at the start and needs downgrading. See the `downgradePattern` option." => {}
                other => failures.push(format!(
                    "{label}: expected start-anchor throw, got {:?}",
                    other
                )),
            }
        } else if needed_downgrade {
            let msg = panic_message(|| {
                let config = Config {
                    downgrade_pattern: false,
                    ..Config::default()
                };
                let _ = is_safe(case.source, case.flags, &config);
            });
            match msg {
                Some(m) if unsupported_reference_message(&m) => {}
                other => failures.push(format!(
                    "{label}: expected unsupported-reference throw, got {:?}",
                    other
                )),
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn exact_trail_counts() {
    let config = Config {
        max_score: f64::INFINITY,
        max_steps: 5000.0,
        ..Config::default()
    };

    let mut failures: Vec<String> = Vec::new();
    for case in TRAIL_COUNTS {
        let result = is_safe(case.source, case.flags, &config);
        if result.trails.len() != case.trails {
            failures.push(format!(
                "/{}/{}: expected {} trails, got {}",
                case.source,
                case.flags,
                case.trails,
                result.trails.len()
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} trail-count mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn no_anchor_unbounded_side_does_not_inflate() {
    // A pattern with no end anchor downgrades to `[^]*?...`, so the checker runs
    // the differing-vs-unbounded side path. A side that can reach a non-bounded
    // end is not a real ambiguity and must drop its branch. Without that, the
    // DFS keeps walking and surfaces extra trails. This case is the minimal
    // witness: two trails and score 2, not eight and three.
    let config = Config {
        max_score: f64::INFINITY,
        max_steps: 5000.0,
        ..Config::default()
    };
    let result = is_safe("^(aa)*a?(aaa)?", "", &config);
    assert_eq!(result.trails.len(), 2);
    assert_eq!(result.score, Score::Finite(2.0));
    assert!(result.safe);
    assert_eq!(result.error, None);

    // The end-anchored form reaches a bounded end, so it keeps its trails.
    let control = is_safe("^(aa)*(aaa)?$", "", &config);
    assert_eq!(control.trails.len(), 4);
    assert_eq!(control.score, Score::Finite(3.0));
}
