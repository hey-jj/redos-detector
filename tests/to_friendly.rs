//! Golden text output for the friendly renderer.

use redos_detector::{
    to_friendly, AnalysisLimit, BackReference, Node, NodeLocation, Report, Score, ToFriendlyConfig,
    Trail, TrailEntry, TrailEntrySide,
};

fn node(start: usize, end: usize, source: &str) -> Node {
    Node {
        start: NodeLocation { offset: start },
        end: NodeLocation { offset: end },
        source: source.to_string(),
    }
}

fn side(backrefs: Vec<BackReference>, n: Node) -> TrailEntrySide {
    TrailEntrySide {
        backreference_stack: backrefs,
        node: n,
        quantifier_iterations: Vec::new(),
    }
}

fn backref(index: u64, start: usize, end: usize, source: &str) -> BackReference {
    BackReference {
        index,
        node: node(start, end, source),
    }
}

fn mock_trails() -> Vec<Trail> {
    let trail1 = Trail {
        entries: vec![TrailEntry {
            a: side(vec![], node(3, 4, "a")),
            b: side(vec![], node(1, 2, "a")),
        }],
    };
    let trail2 = Trail {
        entries: vec![TrailEntry {
            a: side(vec![], node(3, 4, "a")),
            b: side(vec![], node(1, 2, "a")),
        }],
    };
    let trail3 = Trail {
        entries: vec![
            TrailEntry {
                a: side(vec![], node(1, 2, "a")),
                b: side(vec![], node(1, 2, "a")),
            },
            TrailEntry {
                a: side(vec![backref(1, 4, 6, "\\1")], node(1, 2, "a")),
                b: side(vec![backref(1, 4, 6, "\\1")], node(1, 2, "a")),
            },
            TrailEntry {
                a: side(
                    vec![backref(1, 4, 6, "\\1"), backref(2, 8, 10, "\\2")],
                    node(1, 2, "a"),
                ),
                b: side(
                    vec![backref(1, 4, 6, "\\1"), backref(2, 8, 10, "\\2")],
                    node(1, 2, "a"),
                ),
            },
            TrailEntry {
                a: side(
                    vec![
                        backref(1, 4, 6, "\\1"),
                        backref(2, 8, 10, "\\2"),
                        backref(3, 11, 13, "\\3"),
                    ],
                    node(1, 2, "a"),
                ),
                b: side(vec![], node(14, 15, "a")),
            },
        ],
    };
    vec![trail1, trail2, trail3]
}

fn result(
    error: Option<AnalysisLimit>,
    score: Score,
    pattern_downgraded: bool,
    trails: Vec<Trail>,
) -> Report {
    Report {
        error,
        pattern: "pattern".to_string(),
        pattern_downgraded,
        score,
        trails,
    }
}

fn cfg(always: bool, limit: f64) -> ToFriendlyConfig {
    ToFriendlyConfig {
        always_include_trails: always,
        results_limit: limit,
    }
}

const LIMIT: f64 = 15.0;

const TRAILS_BLOCK: &str = "3: `a` | 1: `a`
===============
3: `a` | 1: `a`
===============
       1: `a` |     1: `a`
     4→1: `a` |   4→1: `a`
   8→4→1: `a` | 8→4→1: `a`
11→8→4→1: `a` |    14: `a`
==========================";

#[test]
fn safe_results() {
    // alwaysIncludeTrails=false
    let r = result(None, Score::Finite(1), false, vec![]);
    assert_eq!(
        to_friendly(&r, &cfg(false, LIMIT)),
        "Regex is safe. Score: 1"
    );

    let r = result(None, Score::Finite(2), false, mock_trails());
    assert_eq!(
        to_friendly(&r, &cfg(false, LIMIT)),
        "Regex is safe. Score: 2"
    );

    let r = result(None, Score::Infinite, false, vec![]);
    assert_eq!(
        to_friendly(&r, &cfg(false, LIMIT)),
        "Regex is safe. There could be infinite backtracks."
    );

    // alwaysIncludeTrails=true changes the value-2 case.
    let r = result(None, Score::Finite(2), false, mock_trails());
    let expected = format!(
        "Regex is safe. Score: 2\n\nThe following trails show how the same input can be matched multiple ways.\n{TRAILS_BLOCK}\n"
    );
    assert_eq!(to_friendly(&r, &cfg(true, LIMIT)), expected);
}

#[test]
fn unsafe_no_trails() {
    for always in [false, true] {
        let r = result(
            Some(AnalysisLimit::HitMaxSteps),
            Score::Infinite,
            false,
            vec![],
        );
        assert_eq!(
            to_friendly(&r, &cfg(always, LIMIT)),
            "Regex may not be safe. There could be infinite backtracks. Reached steps limit. The pattern may have too many variations."
        );

        let r = result(
            Some(AnalysisLimit::TimedOut),
            Score::Infinite,
            false,
            vec![],
        );
        assert_eq!(
            to_friendly(&r, &cfg(always, LIMIT)),
            "Regex may not be safe. There could be infinite backtracks. Timed out. The pattern may have too many variations."
        );

        let r = result(
            Some(AnalysisLimit::HitMaxSteps),
            Score::Finite(0),
            false,
            mock_trails(),
        );
        let expected = format!(
            "Regex is not safe. Score: 0\n\nThe following trails show how the same input can be matched multiple ways.\n{TRAILS_BLOCK}\n\nHit maximum number of steps so there may be more results than shown here."
        );
        assert_eq!(to_friendly(&r, &cfg(always, LIMIT)), expected);
    }
}

#[test]
fn unsafe_with_trails() {
    for always in [false, true] {
        let infinite_steps = format!(
            "Regex is not safe. There could be infinite backtracks.\n\nThe following trails show how the same input can be matched multiple ways.\n{TRAILS_BLOCK}\n\nHit maximum number of steps so there may be more results than shown here."
        );
        let r = result(
            Some(AnalysisLimit::HitMaxSteps),
            Score::Infinite,
            false,
            mock_trails(),
        );
        assert_eq!(to_friendly(&r, &cfg(always, LIMIT)), infinite_steps);

        // Single trail uses the singular form.
        let single = "Regex is not safe. There could be infinite backtracks.\n\nThe following trail shows how the same input can be matched multiple ways.\n3: `a` | 1: `a`\n===============\n\nHit maximum number of steps so there may be more results than shown here.";
        let r = result(
            Some(AnalysisLimit::HitMaxSteps),
            Score::Infinite,
            false,
            vec![mock_trails()[0].clone()],
        );
        assert_eq!(to_friendly(&r, &cfg(always, LIMIT)), single);

        let timed_out = format!(
            "Regex is not safe. There could be infinite backtracks.\n\nThe following trails show how the same input can be matched multiple ways.\n{TRAILS_BLOCK}\n\nTimed out so there may be more results than shown here."
        );
        let r = result(
            Some(AnalysisLimit::TimedOut),
            Score::Infinite,
            false,
            mock_trails(),
        );
        assert_eq!(to_friendly(&r, &cfg(always, LIMIT)), timed_out);

        let hit_score = format!(
            "Regex is not safe. There could be infinite backtracks.\n\nThe following trails show how the same input can be matched multiple ways.\n{TRAILS_BLOCK}\n\nHit the max score so there may be more results than shown here."
        );
        let r = result(
            Some(AnalysisLimit::HitMaxScore),
            Score::Infinite,
            false,
            mock_trails(),
        );
        assert_eq!(to_friendly(&r, &cfg(always, LIMIT)), hit_score);

        let hit_score_1 = format!(
            "Regex is not safe. Score: 1\n\nThe following trails show how the same input can be matched multiple ways.\n{TRAILS_BLOCK}\n\nHit the max score so there may be more results than shown here."
        );
        let r = result(
            Some(AnalysisLimit::HitMaxScore),
            Score::Finite(1),
            false,
            mock_trails(),
        );
        assert_eq!(to_friendly(&r, &cfg(always, LIMIT)), hit_score_1);

        // patternDowngraded adds a header line.
        let downgraded = format!(
            "Pattern was downgraded to `pattern`.\nRegex is not safe. There could be infinite backtracks.\n\nThe following trails show how the same input can be matched multiple ways.\n{TRAILS_BLOCK}\n\nHit maximum number of steps so there may be more results than shown here."
        );
        let r = result(
            Some(AnalysisLimit::HitMaxSteps),
            Score::Infinite,
            true,
            mock_trails(),
        );
        assert_eq!(to_friendly(&r, &cfg(always, LIMIT)), downgraded);

        // resultsLimit 0 drops the trail blocks.
        let r = result(
            Some(AnalysisLimit::HitMaxSteps),
            Score::Infinite,
            true,
            mock_trails(),
        );
        assert_eq!(
            to_friendly(&r, &cfg(always, 0.0)),
            "Pattern was downgraded to `pattern`.\nRegex is not safe. There could be infinite backtracks."
        );

        // resultsLimit 1 truncates trails and adds the limit note.
        let limited = "Pattern was downgraded to `pattern`.\nRegex is not safe. There could be infinite backtracks.\n\nThe following trails show how the same input can be matched multiple ways.\n3: `a` | 1: `a`\n===============\n\nHit maximum number of steps so there may be more results than shown here.\nThere are more results than this but hit results limit.";
        let r = result(
            Some(AnalysisLimit::HitMaxSteps),
            Score::Infinite,
            true,
            mock_trails(),
        );
        assert_eq!(to_friendly(&r, &cfg(always, 1.0)), limited);
    }
}

#[test]
fn negative_results_limit_clamps_to_zero() {
    let r = result(
        Some(AnalysisLimit::HitMaxSteps),
        Score::Infinite,
        true,
        mock_trails(),
    );
    // A negative limit is treated as 0, so no trails are printed.
    let zero = to_friendly(&r, &cfg(false, 0.0));
    let negative = to_friendly(&r, &cfg(false, -1.0));
    assert_eq!(negative, zero);
    assert_eq!(
        negative,
        "Pattern was downgraded to `pattern`.\nRegex is not safe. There could be infinite backtracks."
    );
}
