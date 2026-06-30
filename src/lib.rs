//! Static ReDoS analysis for ECMA-262 regular expressions.
//!
//! This crate scores how vulnerable a regular expression is to ReDoS
//! (regular-expression denial of service). It does not run the pattern. It
//! enumerates the ways an input prefix could match and counts how many distinct
//! paths can match the same prefix. More ambiguity means more backtracking, so a
//! higher score. A score of `1` means every input matches at most one way. An
//! infinite score means backtracking is unbounded.
//!
//! Start with [`is_safe`] for a check that reads flags from the pattern string,
//! or [`is_safe_pattern`] for a raw pattern with explicit options. [`to_friendly`]
//! renders a result as text. [`downgrade_pattern`] rewrites unsupported patterns
//! into supported ones.
//!
//! Invalid input is reported by panicking with the same message an ECMA-262
//! engine would surface. Validation errors, unsupported flags, oversized
//! quantifier counts, and references that need downgrading all panic.
//!
//! ```
//! use redos_detector::{is_safe, Config};
//!
//! let result = is_safe("(a+)+$", "", &Config::default());
//! assert!(!result.is_safe());
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod arrays;
mod ast;
mod character_groups;
mod character_reader;
mod checker_reader;
mod code_point;
mod collect_results;
mod downgrade_pattern;
mod infinite_loop_tracker;
mod is_unbounded_reader;
mod map;
mod node_extra;
mod our_range;
mod parse;
mod quantifier;
mod reader;
mod result_cache;
mod sets;
mod to_friendly;
mod tree;

use crate::ast::{NodeKind, RcNode};
use crate::character_reader::level0::StackEntry;
use crate::checker_reader::{Clock, Trail as CheckerTrail};
use crate::collect_results::{collect_results, CollectInput};
use crate::downgrade_pattern::is_missing_start_anchor;
use std::collections::HashSet;

pub use crate::downgrade_pattern::{downgrade_pattern, DowngradedRegexPattern};
pub use crate::to_friendly::{to_friendly, ToFriendlyConfig};

/// The package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default timeout: never time out.
pub const DEFAULT_TIMEOUT: f64 = f64::INFINITY;
/// Default score cap.
pub const DEFAULT_MAX_SCORE: f64 = 200.0;
/// Default step cap.
pub const DEFAULT_MAX_STEPS: f64 = 20000.0;
/// Default multi-line mode.
pub const DEFAULT_MULTI_LINE: bool = false;
/// Default unicode mode.
pub const DEFAULT_UNICODE: bool = false;
/// Default case-insensitive mode.
pub const DEFAULT_CASE_INSENSITIVE: bool = false;
/// Default dot-all mode.
pub const DEFAULT_DOT_ALL: bool = false;

/// A location in the checked pattern. The first character has offset `0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeLocation {
    /// The UTF-16 offset.
    pub offset: usize,
}

/// A node in the checked pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// The end location (exclusive).
    pub end: NodeLocation,
    /// The node's source text.
    pub source: String,
    /// The start location (inclusive).
    pub start: NodeLocation,
}

/// A backreference in a trail side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackReference {
    /// The 1-based index of the group the backreference points at.
    pub index: u64,
    /// The backreference node.
    pub node: Node,
}

/// A quantifier iteration in a trail side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantifierIteration {
    /// The iteration number. The first iteration is `0`.
    pub iteration: u64,
    /// The quantifier node.
    pub node: Node,
}

/// One side of a trail entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrailEntrySide {
    /// The backreference stack the node is part of, outermost last.
    pub backreference_stack: Vec<BackReference>,
    /// The node.
    pub node: Node,
    /// The iteration of each quantifier the node is part of.
    pub quantifier_iterations: Vec<QuantifierIteration>,
}

/// A trail entry showing two ways through the pattern side by side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrailEntry {
    /// The `a` side.
    pub a: TrailEntrySide,
    /// The `b` side.
    pub b: TrailEntrySide,
}

/// A trail: a pair of distinct ways to match the same input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trail {
    /// The entries of the trail.
    pub entries: Vec<TrailEntry>,
}

/// The error reported for an unsafe result.
///
/// Match with a wildcard arm. New limits may be added in a future version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AnalysisLimit {
    /// The score passed the cap.
    HitMaxScore,
    /// The step cap was hit.
    HitMaxSteps,
    /// The timeout was hit.
    TimedOut,
}

/// An internal checker error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckError {
    /// The step cap was hit.
    HitMaxSteps,
    /// The timeout was hit.
    TimedOut,
}

/// A score. `Finite` carries the count; `Infinite` means unbounded backtracking.
///
/// A score is a count of distinct ways to match the same input prefix, so it is
/// a whole number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Score {
    /// A finite count.
    Finite(u64),
    /// Unbounded backtracking.
    Infinite,
}

/// The result of a check.
#[derive(Clone, Debug)]
pub struct Report {
    /// The error, or `None` when safe.
    pub error: Option<AnalysisLimit>,
    /// The checked pattern, downgraded if needed.
    pub pattern: String,
    /// Whether the pattern was downgraded.
    pub pattern_downgraded: bool,
    /// The score.
    pub score: Score,
    /// The discovered trails.
    pub trails: Vec<Trail>,
}

impl Report {
    /// Whether the pattern is safe. A pattern is safe when no analysis limit
    /// was hit.
    pub fn is_safe(&self) -> bool {
        self.error.is_none()
    }
}

/// Options for [`is_safe`] and [`is_safe_pattern`].
#[derive(Clone, Debug)]
pub struct Config {
    /// The score cap. Above this the pattern is considered unsafe.
    pub max_score: f64,
    /// The step cap.
    pub max_steps: f64,
    /// The timeout in milliseconds.
    pub timeout: f64,
    /// Case-insensitive mode.
    pub case_insensitive: bool,
    /// Dot-all mode.
    pub dot_all: bool,
    /// Multi-line mode.
    pub multi_line: bool,
    /// Unicode mode.
    pub unicode: bool,
    /// Whether to downgrade an unsupported pattern.
    pub downgrade_pattern: bool,
    /// Offsets of groups to treat as atomic. Only used with `downgrade_pattern`
    /// set to `false`.
    pub atomic_group_offsets: Option<HashSet<usize>>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_score: DEFAULT_MAX_SCORE,
            max_steps: DEFAULT_MAX_STEPS,
            timeout: DEFAULT_TIMEOUT,
            case_insensitive: DEFAULT_CASE_INSENSITIVE,
            dot_all: DEFAULT_DOT_ALL,
            multi_line: DEFAULT_MULTI_LINE,
            unicode: DEFAULT_UNICODE,
            downgrade_pattern: true,
            atomic_group_offsets: None,
        }
    }
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> f64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }
}

/// Checks whether `regex` and its inline flags are safe from ReDoS.
///
/// `regex` is a `source/flags` pair like `"a+a+/i"`. Flags are read from the
/// part after the last `/`. The flags `u i s m` change matching; `g y d` are
/// accepted and ignored. An unsupported flag panics.
pub fn is_safe(source: &str, flags: &str, config: &Config) -> Report {
    let mut unicode = false;
    let mut case_insensitive = false;
    let mut dot_all = false;
    let mut multi_line = false;
    for flag in flags.chars() {
        if !matches!(flag, 'u' | 'g' | 's' | 'y' | 'i' | 'd' | 'm') {
            panic!("Unsupported flag: {}", flag);
        }
        match flag {
            'u' => unicode = true,
            'i' => case_insensitive = true,
            's' => dot_all = true,
            'm' => multi_line = true,
            _ => {}
        }
    }

    let merged = Config {
        case_insensitive,
        dot_all,
        multi_line,
        unicode,
        ..config.clone()
    };
    is_safe_pattern(source, &merged)
}

/// Checks whether `input_pattern` is safe from ReDoS using explicit options.
pub fn is_safe_pattern(input_pattern: &str, config: &Config) -> Report {
    is_safe_pattern_with_clock(input_pattern, config, SystemClock)
}

fn is_safe_pattern_with_clock<C: Clock>(input_pattern: &str, config: &Config, clock: C) -> Report {
    if config.case_insensitive && config.unicode {
        panic!("`caseInsensitive` cannot be used with `unicode`.");
    }
    if config.downgrade_pattern && config.atomic_group_offsets.is_some() {
        panic!("`atomicGroupOffsets` cannot be used with `downgradePattern: true`.");
    }
    if config.timeout <= 0.0 {
        panic!("`timeout` must be a positive number.");
    }
    if config.max_score < 0.0 {
        panic!("`maxScore` must be a positive number or 0.");
    }
    if config.max_steps <= 0.0 {
        panic!("`maxSteps` must be a positive number.");
    }

    let (pattern, atomic_group_offsets) = if config.downgrade_pattern {
        let downgraded = downgrade_pattern(input_pattern, config.unicode);
        (downgraded.pattern, downgraded.atomic_group_offsets)
    } else {
        (
            input_pattern.to_string(),
            config.atomic_group_offsets.clone().unwrap_or_default(),
        )
    };

    let pattern_downgraded = config.downgrade_pattern && input_pattern != pattern;

    let ast = match parse::parse(&pattern, config.unicode) {
        Ok(ast) => ast,
        Err(e) => panic!("{}", e.0),
    };

    if !config.downgrade_pattern && is_missing_start_anchor(&ast) {
        panic!("Pattern is not bounded at the start and needs downgrading. See the `downgradePattern` option.");
    }

    let result = collect_results(
        CollectInput {
            atomic_group_offsets,
            case_insensitive: config.case_insensitive,
            dot_all: config.dot_all,
            max_score: config.max_score,
            max_steps: config.max_steps,
            multi_line: config.multi_line,
            node: ast,
            timeout: config.timeout,
        },
        clock,
    );

    build_result(result, pattern, pattern_downgraded)
}

fn build_result(
    result: collect_results::CollectResults,
    pattern: String,
    pattern_downgraded: bool,
) -> Report {
    let trails = result.trails.iter().map(map_trail).collect::<Vec<Trail>>();

    Report {
        error: result.error,
        pattern,
        pattern_downgraded,
        score: result.score,
        trails,
    }
}

fn map_trail(trail: &CheckerTrail) -> Trail {
    Trail {
        entries: trail
            .iter()
            .map(|entry| TrailEntry {
                a: TrailEntrySide {
                    backreference_stack: to_backreference_stack(&entry.right.stack),
                    node: to_node(&entry.right.node),
                    quantifier_iterations: to_quantifier_iterations(&entry.right.stack),
                },
                b: TrailEntrySide {
                    backreference_stack: to_backreference_stack(&entry.left.stack),
                    node: to_node(&entry.left.node),
                    quantifier_iterations: to_quantifier_iterations(&entry.left.stack),
                },
            })
            .collect(),
    }
}

fn to_node(node: &RcNode) -> Node {
    Node {
        end: NodeLocation {
            offset: node.range.1,
        },
        source: node.raw.clone(),
        start: NodeLocation {
            offset: node.range.0,
        },
    }
}

fn to_backreference_stack(stack: &[StackEntry]) -> Vec<BackReference> {
    let mut references: Vec<RcNode> = stack
        .iter()
        .filter_map(|entry| match entry {
            StackEntry::Reference { reference } => Some(reference.clone()),
            _ => None,
        })
        .collect();
    references.reverse();
    references
        .iter()
        .map(|reference| {
            let index = match &reference.kind {
                NodeKind::Reference { match_index } => *match_index,
                _ => 0,
            };
            BackReference {
                index,
                node: to_node(reference),
            }
        })
        .collect()
}

fn to_quantifier_iterations(stack: &[StackEntry]) -> Vec<QuantifierIteration> {
    let reversed: Vec<&StackEntry> = stack.iter().rev().collect();
    let reference_index = reversed
        .iter()
        .position(|entry| matches!(entry, StackEntry::Reference { .. }));
    let portion_len = reference_index.unwrap_or(stack.len());
    let mut portion: Vec<&StackEntry> = reversed[..portion_len].to_vec();
    portion.reverse();

    portion
        .iter()
        .filter_map(|entry| match entry {
            StackEntry::Quantifier {
                iteration,
                quantifier,
            } => Some(QuantifierIteration {
                iteration: *iteration,
                node: to_node(quantifier),
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod api_tests {
    use super::*;
    use std::cell::Cell;

    fn inf_steps() -> Config {
        Config {
            max_score: f64::INFINITY,
            max_steps: 5000.0,
            ..Config::default()
        }
    }

    /// A clock that advances by one millisecond on each read.
    struct FakeClock {
        time: Cell<f64>,
    }

    impl Clock for FakeClock {
        fn now(&self) -> f64 {
            let next = self.time.get() + 1.0;
            self.time.set(next);
            next
        }
    }

    #[test]
    fn version_is_crate_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn respects_max_score() {
        let config = Config {
            max_score: 2.0,
            ..Config::default()
        };
        assert_eq!(is_safe("^a?a?$", "", &config).error, None);
        let config = Config {
            max_score: 1.0,
            ..Config::default()
        };
        assert_eq!(
            is_safe("^a?a?$", "", &config).error,
            Some(AnalysisLimit::HitMaxScore)
        );
    }

    #[test]
    fn respects_max_steps() {
        let config = Config {
            max_steps: 20.0,
            ..Config::default()
        };
        assert_eq!(
            is_safe("a?a?a?", "", &config).error,
            Some(AnalysisLimit::HitMaxSteps)
        );
    }

    #[test]
    fn respects_timeout() {
        let config = Config {
            timeout: 20.0,
            ..Config::default()
        };
        let clock = FakeClock {
            time: Cell::new(0.0),
        };
        let result = is_safe_pattern_with_clock("a?a?a?", &config, clock);
        assert_eq!(result.error, Some(AnalysisLimit::TimedOut));
    }

    #[test]
    #[should_panic(expected = "`maxScore` must be a positive number or 0.")]
    fn rejects_negative_max_score() {
        let config = Config {
            max_score: -1.0,
            ..Config::default()
        };
        let _ = is_safe("a", "", &config);
    }

    #[test]
    #[should_panic(expected = "`timeout` must be a positive number.")]
    fn rejects_zero_timeout() {
        let config = Config {
            timeout: 0.0,
            ..Config::default()
        };
        let _ = is_safe("a", "", &config);
    }

    #[test]
    #[should_panic(expected = "`maxSteps` must be a positive number.")]
    fn rejects_zero_max_steps() {
        let config = Config {
            max_steps: 0.0,
            ..Config::default()
        };
        let _ = is_safe("a", "", &config);
    }

    #[test]
    #[should_panic(expected = "Unsupported flag: z")]
    fn rejects_unsupported_flag() {
        let _ = is_safe("a", "z", &Config::default());
    }

    #[test]
    fn accepts_supported_flags() {
        for flag in ["u", "g", "s", "y", "i", "d", "m"] {
            let _ = is_safe("a", flag, &Config::default());
        }
    }

    #[test]
    #[should_panic(expected = "`caseInsensitive` cannot be used with `unicode`.")]
    fn rejects_case_insensitive_with_unicode() {
        let config = Config {
            case_insensitive: true,
            unicode: true,
            ..Config::default()
        };
        let _ = is_safe_pattern("a", &config);
    }

    #[test]
    #[should_panic(expected = "iterations outside JS safe integer range")]
    fn rejects_iterations_above_max_safe_integer() {
        let _ = is_safe_pattern("a{0,9007199254740992}", &Config::default());
    }

    #[test]
    #[should_panic(expected = "`atomicGroupOffsets` cannot be used with `downgradePattern: true`.")]
    fn rejects_atomic_offsets_with_downgrade() {
        let config = Config {
            downgrade_pattern: true,
            atomic_group_offsets: Some(HashSet::new()),
            ..Config::default()
        };
        let _ = is_safe_pattern("", &config);
    }

    #[test]
    fn supports_atomic_group_offsets() {
        let mut offsets = HashSet::new();
        offsets.insert(1);
        let config = Config {
            downgrade_pattern: false,
            atomic_group_offsets: Some(offsets),
            ..Config::default()
        };
        assert!(is_safe_pattern("^(a?)a?$", &config).trails.is_empty());
    }

    #[test]
    fn no_options_pattern() {
        assert_eq!(is_safe_pattern("a", &Config::default()).error, None);
    }

    #[test]
    fn flags_override_config() {
        // The 'i' flag turns on case-insensitive matching regardless of config.
        let _ = is_safe("a", "i", &inf_steps());
    }
}
