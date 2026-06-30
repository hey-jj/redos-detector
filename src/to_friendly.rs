//! Renders a result as human-readable text.
//!
//! Do not parse this output. It may change between versions. Column widths are
//! measured in UTF-16 code units to match the engine the analysis targets, and
//! breadcrumbs are joined with the arrow `→`.

use crate::{AnalysisLimit, Report, Score, TrailEntrySide};

const TRUNCATE_LENGTH: usize = 100;

/// Default number of trails to print.
pub const DEFAULT_RESULTS_LIMIT: u64 = 15;

/// Options for [`to_friendly`].
#[derive(Clone, Debug)]
pub struct ToFriendlyConfig {
    /// The maximum number of trails to print. `None` prints every trail.
    pub results_limit: Option<u64>,
    /// Always include trails even when the pattern is safe.
    pub always_include_trails: bool,
}

impl Default for ToFriendlyConfig {
    fn default() -> Self {
        ToFriendlyConfig {
            results_limit: Some(DEFAULT_RESULTS_LIMIT),
            always_include_trails: false,
        }
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn pad_start(s: &str, width: usize) -> String {
    let len = utf16_len(s);
    if len >= width {
        s.to_string()
    } else {
        format!("{}{}", " ".repeat(width - len), s)
    }
}

fn pad_end(s: &str, width: usize) -> String {
    let len = utf16_len(s);
    if len >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - len))
    }
}

fn get_breadcrumbs(side: &TrailEntrySide) -> String {
    let mut parts: Vec<String> = side
        .backreference_stack
        .iter()
        .map(|b| b.node.start.offset.to_string())
        .collect();
    parts.reverse();
    parts.push(side.node.start.offset.to_string());
    parts.join("\u{2192}")
}

fn score_string(score: Score) -> String {
    match score {
        Score::Infinite => "There could be infinite backtracks.".to_string(),
        Score::Finite(value) => format!("Score: {}", value),
    }
}

/// Renders `result` as text.
///
/// `results_limit` of `None` prints every trail.
pub fn to_friendly(result: &Report, config: &ToFriendlyConfig) -> String {
    let results_limit = config.results_limit;
    let score_str = score_string(result.score);

    if result.is_safe() && !config.always_include_trails {
        return format!("Regex is safe. {}", score_str);
    }

    let mut output_lines: Vec<String> = Vec::new();

    if result.pattern_downgraded {
        output_lines.push(format!("Pattern was downgraded to `{}`.", result.pattern));
    }

    if result.trails.is_empty() {
        let mut parts: Vec<String> = Vec::new();
        parts.push(if result.is_safe() {
            format!("Regex is safe. {}", score_str)
        } else {
            format!("Regex may not be safe. {}", score_str)
        });
        if result.error == Some(AnalysisLimit::TimedOut) {
            parts.push("Timed out.".to_string());
        }
        if result.error == Some(AnalysisLimit::HitMaxSteps) {
            parts.push("Reached steps limit.".to_string());
        }
        if !result.is_safe() {
            parts.push("The pattern may have too many variations.".to_string());
        }
        output_lines.push(parts.join(" "));
    } else {
        let limit = results_limit.map_or(result.trails.len(), |n| n as usize);
        let result_blocks: Vec<String> =
            result.trails.iter().take(limit).map(render_block).collect();

        output_lines.push(format!(
            "Regex is {}safe. {}",
            if !result.is_safe() { "not " } else { "" },
            score_str
        ));

        if results_limit != Some(0) {
            output_lines.push(String::new());
            let plural = if result.trails.len() > 1 { "s" } else { "" };
            let singular = if result.trails.len() == 1 { "s" } else { "" };
            output_lines.push(format!(
                "The following trail{} show{} how the same input can be matched multiple ways.",
                plural, singular
            ));
            output_lines.extend(result_blocks);
            output_lines.push(String::new());
            if let Some(error) = result.error {
                output_lines.push(error_message(error).to_string());
            }
            if results_limit.is_some_and(|n| result.trails.len() as u64 > n) {
                output_lines
                    .push("There are more results than this but hit results limit.".to_string());
            }
        }
    }

    output_lines.join("\n")
}

fn render_block(trail: &crate::Trail) -> String {
    let row_contents: Vec<[String; 4]> = trail
        .entries
        .iter()
        .take(TRUNCATE_LENGTH)
        .map(|entry| {
            [
                get_breadcrumbs(&entry.a),
                format!("`{}`", entry.a.node.source),
                get_breadcrumbs(&entry.b),
                format!("`{}`", entry.b.node.source),
            ]
        })
        .collect();

    let max_col1 = row_contents
        .iter()
        .map(|c| utf16_len(&c[0]))
        .max()
        .unwrap_or(0);
    let max_col2 = row_contents
        .iter()
        .map(|c| utf16_len(&c[1]))
        .max()
        .unwrap_or(0);
    let max_col3 = row_contents
        .iter()
        .map(|c| utf16_len(&c[2]))
        .max()
        .unwrap_or(0);

    let mut rows: Vec<String> = row_contents
        .iter()
        .map(|c| {
            format!(
                "{}: {} | {}: {}",
                pad_start(&c[0], max_col1),
                pad_end(&c[1], max_col2),
                pad_start(&c[2], max_col3),
                c[3]
            )
        })
        .collect();

    if trail.entries.len() > TRUNCATE_LENGTH {
        rows.push("\u{2026}".to_string());
    }

    let max_row_length = rows.iter().map(|r| utf16_len(r)).max().unwrap_or(0);
    rows.push("=".repeat(max_row_length));

    rows.join("\n")
}

fn error_message(error: AnalysisLimit) -> &'static str {
    match error {
        AnalysisLimit::HitMaxScore => {
            "Hit the max score so there may be more results than shown here."
        }
        AnalysisLimit::HitMaxSteps => {
            "Hit maximum number of steps so there may be more results than shown here."
        }
        AnalysisLimit::TimedOut => "Timed out so there may be more results than shown here.",
    }
}
