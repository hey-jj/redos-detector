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
//! Recoverable input problems return [`Error`]: an unsupported flag, a
//! conflicting config, or a pattern that fails to parse. A successful check
//! returns a [`Report`]. When the analysis hits a cap the report carries an
//! [`AnalysisLimit`] in its `error` field, which is a different axis from input
//! validation.
//!
//! ```
//! use redos_detector::{is_safe, Config};
//!
//! let result = is_safe("(a+)+$", "", &Config::default()).unwrap();
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
use std::time::Duration;

pub use crate::downgrade_pattern::{downgrade_pattern, DowngradedRegexPattern};
pub use crate::to_friendly::{to_friendly, ToFriendlyConfig};

/// The package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default score cap.
pub const DEFAULT_MAX_SCORE: u64 = 200;
/// Default step cap.
pub const DEFAULT_MAX_STEPS: u64 = 20000;
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
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// An input error from a public entry point.
///
/// This covers recoverable problems with the call: a bad flag, a conflicting
/// config, or a pattern that fails to parse. It is separate from
/// [`AnalysisLimit`], which reports that the analysis hit a cap and is part of
/// a successful [`Report`].
///
/// Match with a wildcard arm. New variants may be added in a future version.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A flag letter outside `u g s y i d m`.
    UnsupportedFlag(char),
    /// `case_insensitive` was set together with `unicode`.
    CaseInsensitiveWithUnicode,
    /// `atomic_group_offsets` was set together with `downgrade_pattern: true`.
    AtomicGroupOffsetsWithDowngrade,
    /// `timeout` was zero.
    InvalidTimeout,
    /// `max_steps` was zero.
    InvalidMaxSteps,
    /// The pattern failed to parse. The string is the parser message.
    Parse(String),
    /// `downgrade_pattern` was off and the pattern has no start anchor.
    MissingStartAnchor,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnsupportedFlag(flag) => write!(f, "Unsupported flag: {}", flag),
            Error::CaseInsensitiveWithUnicode => {
                write!(f, "`caseInsensitive` cannot be used with `unicode`.")
            }
            Error::AtomicGroupOffsetsWithDowngrade => write!(
                f,
                "`atomicGroupOffsets` cannot be used with `downgradePattern: true`."
            ),
            Error::InvalidTimeout => write!(f, "`timeout` must be a positive number."),
            Error::InvalidMaxSteps => write!(f, "`maxSteps` must be a positive number."),
            Error::Parse(message) => write!(f, "{}", message),
            Error::MissingStartAnchor => write!(
                f,
                "Pattern is not bounded at the start and needs downgrading. See the `downgradePattern` option."
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Options for [`is_safe`] and [`is_safe_pattern`].
#[derive(Clone, Debug)]
pub struct Config {
    /// The score cap. Above this the pattern is considered unsafe. `None` means
    /// no cap.
    pub max_score: Option<u64>,
    /// The step cap. `None` means no cap.
    pub max_steps: Option<u64>,
    /// The wall-clock budget. `None` means no timeout.
    pub timeout: Option<Duration>,
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
            max_score: Some(DEFAULT_MAX_SCORE),
            max_steps: Some(DEFAULT_MAX_STEPS),
            timeout: None,
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

/// Parses a flag string into the four matching options.
///
/// The flags `u i s m` change matching. `g y d` are accepted and ignored. An
/// unsupported letter returns [`Error::UnsupportedFlag`].
fn parse_flags(flags: &str) -> Result<(bool, bool, bool, bool), Error> {
    let mut unicode = false;
    let mut case_insensitive = false;
    let mut dot_all = false;
    let mut multi_line = false;
    for flag in flags.chars() {
        match flag {
            'u' => unicode = true,
            'i' => case_insensitive = true,
            's' => dot_all = true,
            'm' => multi_line = true,
            'g' | 'y' | 'd' => {}
            other => return Err(Error::UnsupportedFlag(other)),
        }
    }
    Ok((unicode, case_insensitive, dot_all, multi_line))
}

/// Checks whether `source` and its inline `flags` are safe from ReDoS.
///
/// Flags are read from `flags`, the part after a regex literal's closing `/`.
/// The flags `u i s m` change matching. `g y d` are accepted and ignored. The
/// flags override the matching options on `config`.
///
/// Returns [`Error`] on an unsupported flag, a conflicting config, or a pattern
/// that fails to parse.
pub fn is_safe(source: &str, flags: &str, config: &Config) -> Result<Report, Error> {
    let (unicode, case_insensitive, dot_all, multi_line) = parse_flags(flags)?;

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
///
/// Returns [`Error`] on a conflicting config or a pattern that fails to parse.
pub fn is_safe_pattern(input_pattern: &str, config: &Config) -> Result<Report, Error> {
    is_safe_pattern_with_clock(input_pattern, config, SystemClock)
}

/// Maps a count cap to the float the collector compares against. `None` becomes
/// positive infinity, which never trips the cap.
fn cap_to_f64(cap: Option<u64>) -> f64 {
    cap.map_or(f64::INFINITY, |value| value as f64)
}

/// Maps a timeout to milliseconds for the checker clock. `None` becomes positive
/// infinity, which never times out.
fn timeout_to_f64(timeout: Option<Duration>) -> f64 {
    timeout.map_or(f64::INFINITY, |duration| duration.as_secs_f64() * 1000.0)
}

fn is_safe_pattern_with_clock<C: Clock>(
    input_pattern: &str,
    config: &Config,
    clock: C,
) -> Result<Report, Error> {
    if config.case_insensitive && config.unicode {
        return Err(Error::CaseInsensitiveWithUnicode);
    }
    if config.downgrade_pattern && config.atomic_group_offsets.is_some() {
        return Err(Error::AtomicGroupOffsetsWithDowngrade);
    }
    if config.timeout == Some(Duration::ZERO) {
        return Err(Error::InvalidTimeout);
    }
    if config.max_steps == Some(0) {
        return Err(Error::InvalidMaxSteps);
    }

    let (pattern, atomic_group_offsets) = if config.downgrade_pattern {
        let downgraded = downgrade_pattern(input_pattern, config.unicode)?;
        (downgraded.pattern, downgraded.atomic_group_offsets)
    } else {
        (
            input_pattern.to_string(),
            config.atomic_group_offsets.clone().unwrap_or_default(),
        )
    };

    let pattern_downgraded = config.downgrade_pattern && input_pattern != pattern;

    let ast = parse::parse(&pattern, config.unicode).map_err(|e| Error::Parse(e.0))?;

    if !config.downgrade_pattern && is_missing_start_anchor(&ast) {
        return Err(Error::MissingStartAnchor);
    }

    let result = collect_results(
        CollectInput {
            atomic_group_offsets,
            case_insensitive: config.case_insensitive,
            dot_all: config.dot_all,
            max_score: cap_to_f64(config.max_score),
            max_steps: cap_to_f64(config.max_steps),
            multi_line: config.multi_line,
            node: ast,
            timeout: timeout_to_f64(config.timeout),
        },
        clock,
    );

    Ok(build_result(result, pattern, pattern_downgraded))
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
            max_score: None,
            max_steps: Some(5000),
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
            max_score: Some(2),
            ..Config::default()
        };
        assert_eq!(is_safe("^a?a?$", "", &config).unwrap().error, None);
        let config = Config {
            max_score: Some(1),
            ..Config::default()
        };
        assert_eq!(
            is_safe("^a?a?$", "", &config).unwrap().error,
            Some(AnalysisLimit::HitMaxScore)
        );
    }

    #[test]
    fn respects_max_steps() {
        let config = Config {
            max_steps: Some(20),
            ..Config::default()
        };
        assert_eq!(
            is_safe("a?a?a?", "", &config).unwrap().error,
            Some(AnalysisLimit::HitMaxSteps)
        );
    }

    #[test]
    fn respects_timeout() {
        let config = Config {
            timeout: Some(Duration::from_millis(20)),
            ..Config::default()
        };
        let clock = FakeClock {
            time: Cell::new(0.0),
        };
        let result = is_safe_pattern_with_clock("a?a?a?", &config, clock).unwrap();
        assert_eq!(result.error, Some(AnalysisLimit::TimedOut));
    }

    #[test]
    fn rejects_zero_timeout() {
        let config = Config {
            timeout: Some(Duration::ZERO),
            ..Config::default()
        };
        assert_eq!(is_safe("a", "", &config), Err(Error::InvalidTimeout));
    }

    #[test]
    fn rejects_zero_max_steps() {
        let config = Config {
            max_steps: Some(0),
            ..Config::default()
        };
        assert_eq!(is_safe("a", "", &config), Err(Error::InvalidMaxSteps));
    }

    #[test]
    fn rejects_unsupported_flag() {
        assert_eq!(
            is_safe("a", "z", &Config::default()),
            Err(Error::UnsupportedFlag('z'))
        );
    }

    #[test]
    fn accepts_supported_flags() {
        for flag in ["u", "g", "s", "y", "i", "d", "m"] {
            assert!(is_safe("a", flag, &Config::default()).is_ok());
        }
    }

    #[test]
    fn rejects_case_insensitive_with_unicode() {
        let config = Config {
            case_insensitive: true,
            unicode: true,
            ..Config::default()
        };
        assert_eq!(
            is_safe_pattern("a", &config),
            Err(Error::CaseInsensitiveWithUnicode)
        );
    }

    #[test]
    fn rejects_iterations_above_max_safe_integer() {
        let err = is_safe_pattern("a{0,9007199254740992}", &Config::default()).unwrap_err();
        assert!(matches!(err, Error::Parse(message)
            if message.contains("iterations outside JS safe integer range")));
    }

    #[test]
    fn rejects_atomic_offsets_with_downgrade() {
        let config = Config {
            downgrade_pattern: true,
            atomic_group_offsets: Some(HashSet::new()),
            ..Config::default()
        };
        assert_eq!(
            is_safe_pattern("", &config),
            Err(Error::AtomicGroupOffsetsWithDowngrade)
        );
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
        assert!(is_safe_pattern("^(a?)a?$", &config)
            .unwrap()
            .trails
            .is_empty());
    }

    #[test]
    fn no_options_pattern() {
        assert_eq!(
            is_safe_pattern("a", &Config::default()).unwrap().error,
            None
        );
    }

    #[test]
    fn flags_override_config() {
        // The 'i' flag turns on case-insensitive matching regardless of config.
        assert!(is_safe("a", "i", &inf_steps()).is_ok());
    }

    #[test]
    fn missing_start_anchor_without_downgrade_errors() {
        let config = Config {
            downgrade_pattern: false,
            ..Config::default()
        };
        assert_eq!(
            is_safe_pattern("a", &config),
            Err(Error::MissingStartAnchor)
        );
    }
}
