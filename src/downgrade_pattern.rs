//! Rewrites unsupported patterns into supported ones.
//!
//! Three transforms run until the pattern stabilizes. A reference to a group in
//! a positive lookahead is replaced by a non-capturing group holding that
//! group's contents. A reference to a non-finite-size group is inlined the same
//! way. A pattern with no start anchor gets `[^]*?` prepended, wrapping the rest
//! in a non-capturing group when the root is a disjunction. A downgraded pattern
//! may introduce false positives.

use crate::arrays::drop_common;
use crate::ast::{AnchorKind, GroupBehavior, NodeKind, RcNode};
use crate::parse::parse;
use std::collections::HashSet;

/// A downgraded pattern and the offsets of any atomic groups it introduced.
#[derive(Clone, Debug)]
pub struct DowngradedRegexPattern {
    /// Offsets of groups to treat as atomic.
    pub atomic_group_offsets: HashSet<usize>,
    /// The downgraded pattern.
    pub pattern: String,
}

/// Returns whether each `range[0]` token a reference points to is atomic or optional.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AtomicOrOptional {
    Atomic,
    Optional,
    None,
}

struct Action {
    atomic_or_optional: AtomicOrOptional,
    group: RcNode,
    reference: RcNode,
    reference_start: usize,
    reference_end: usize,
}

fn utf16_slice_replace(input: &str, replacement: &str, start: usize, end: usize) -> String {
    let units: Vec<u16> = input.encode_utf16().collect();
    let mut out: Vec<u16> = Vec::new();
    out.extend_from_slice(&units[..start]);
    out.extend(replacement.encode_utf16());
    out.extend_from_slice(&units[end..]);
    String::from_utf16_lossy(&out)
}

fn shift_offsets(offsets: &HashSet<usize>, after: i64, shift_amount: i64) -> HashSet<usize> {
    offsets
        .iter()
        .map(|&offset| {
            if (offset as i64) > after {
                (offset as i64 + shift_amount) as usize
            } else {
                offset
            }
        })
        .collect()
}

fn quantifier_iterations_to_string(node: &RcNode) -> String {
    if let NodeKind::Quantifier {
        min,
        max,
        greedy,
        symbol,
        ..
    } = &node.kind
    {
        if let Some(sym) = symbol {
            return format!("{}{}", sym, if *greedy { "" } else { "?" });
        }
        if max.map(|m| m == *min).unwrap_or(false) {
            return format!("{{{}}}", min);
        }
        let max_str = max.map(|m| m.to_string()).unwrap_or_default();
        return format!("{{{},{}}}{}", min, max_str, if *greedy { "" } else { "?" });
    }
    String::new()
}

/// The result of stripping capturing groups and lookarounds from a node.
pub(crate) struct RawWithoutGroups {
    pub(crate) references_with_offset: Vec<(RcNode, usize)>,
    pub(crate) result: String,
}

/// Reconstructs `root`'s source with capturing and non-capturing groups turned
/// into `(?:...)` and lookarounds removed, tracking reference offsets.
pub(crate) fn get_raw_without_capturing_groups_or_lookaheads(root: &RcNode) -> RawWithoutGroups {
    let mut references_with_offset: Vec<(RcNode, usize)> = Vec::new();
    let result = walk(root, 0, &mut references_with_offset);
    RawWithoutGroups {
        references_with_offset,
        result,
    }
}

fn walk(node: &RcNode, offset: usize, refs: &mut Vec<(RcNode, usize)>) -> String {
    let walk_all = |nodes: &[RcNode], start_offset: usize, refs: &mut Vec<(RcNode, usize)>| {
        let mut result = String::new();
        for n in nodes {
            let len = result.encode_utf16().count();
            result.push_str(&walk(n, start_offset + len, refs));
        }
        result
    };

    match &node.kind {
        NodeKind::Anchor { .. }
        | NodeKind::CharacterClass { .. }
        | NodeKind::CharacterClassEscape { .. }
        | NodeKind::UnicodePropertyEscape { .. }
        | NodeKind::Value { .. }
        | NodeKind::Dot => node.raw.clone(),
        NodeKind::Reference { .. } => {
            refs.push((node.clone(), offset));
            node.raw.clone()
        }
        NodeKind::Group { behavior, body } => match behavior {
            GroupBehavior::Normal | GroupBehavior::Ignore => {
                format!("(?:{})", walk_all(body, offset + 3, refs))
            }
            _ => String::new(),
        },
        NodeKind::Disjunction { body } => {
            let mut res = String::new();
            for (i, child) in body.iter().enumerate() {
                if i > 0 {
                    res.push('|');
                }
                let len = res.encode_utf16().count();
                res.push_str(&walk(child, offset + len, refs));
            }
            res
        }
        NodeKind::Alternative { body } => walk_all(body, offset, refs),
        NodeKind::Quantifier { body, .. } => {
            format!(
                "{}{}",
                walk(body, offset, refs),
                quantifier_iterations_to_string(node)
            )
        }
        NodeKind::CharacterClassRange { .. } => node.raw.clone(),
    }
}

fn already_has_start_anchor_replacement(node: &RcNode) -> bool {
    let body = match &node.kind {
        NodeKind::Alternative { body } => body,
        _ => return false,
    };
    if body.is_empty() {
        return false;
    }
    let maybe_quantifier = &body[0];
    let (min, max, greedy, q_body) = match &maybe_quantifier.kind {
        NodeKind::Quantifier {
            min,
            max,
            greedy,
            body,
            ..
        } => (*min, *max, *greedy, body),
        _ => return false,
    };
    if min != 0 || max.is_some() || greedy {
        return false;
    }
    match &q_body.kind {
        NodeKind::CharacterClass { negative, body } => body.is_empty() && *negative,
        _ => false,
    }
}

/// Returns whether the pattern is not bounded at the start.
pub fn is_missing_start_anchor(root: &RcNode) -> bool {
    if already_has_start_anchor_replacement(root) {
        return false;
    }
    check_anchor(root) == AnchorCheck::ConsumingNode
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum AnchorCheck {
    Anchor,
    ConsumingNode,
    Null,
}

fn check_anchor(node: &RcNode) -> AnchorCheck {
    match &node.kind {
        NodeKind::Anchor { kind } => {
            if *kind == AnchorKind::Start {
                AnchorCheck::Anchor
            } else {
                AnchorCheck::Null
            }
        }
        NodeKind::Quantifier { max, body, min, .. } => {
            if *max == Some(0) {
                return AnchorCheck::Null;
            }
            let may_be_skipped = *min == 0;
            check_children(std::slice::from_ref(body), may_be_skipped)
        }
        NodeKind::Alternative { body } | NodeKind::Group { body, .. } => {
            check_children(body, false)
        }
        NodeKind::Disjunction { body } => {
            let results: Vec<AnchorCheck> = body.iter().map(check_anchor).collect();
            if results.contains(&AnchorCheck::ConsumingNode) {
                AnchorCheck::ConsumingNode
            } else if results.iter().all(|r| *r == AnchorCheck::Anchor) {
                AnchorCheck::Anchor
            } else {
                AnchorCheck::Null
            }
        }
        _ => AnchorCheck::ConsumingNode,
    }
}

fn check_children(body: &[RcNode], may_be_skipped: bool) -> AnchorCheck {
    for child in body {
        let res = check_anchor(child);
        if res == AnchorCheck::ConsumingNode {
            return AnchorCheck::ConsumingNode;
        }
        if res == AnchorCheck::Anchor {
            if !may_be_skipped {
                return AnchorCheck::Anchor;
            }
            return AnchorCheck::Null;
        }
    }
    AnchorCheck::Null
}

/// Downgrades `pattern` if needed.
pub fn downgrade_pattern(pattern: &str, unicode: bool) -> DowngradedRegexPattern {
    let mut last = DowngradedRegexPattern {
        atomic_group_offsets: HashSet::new(),
        pattern: pattern.to_string(),
    };
    loop {
        let (need_rerun, result) = run(&last, unicode);
        last = result;
        if !need_rerun {
            break;
        }
    }
    last
}

struct GroupEntry {
    group: RcNode,
    stack: Vec<RcNode>,
}

struct Walker {
    actions: Vec<Action>,
    needs_start_anchor_replacement: bool,
    groups: std::collections::HashMap<u64, GroupEntry>,
    infinite_groups: HashSet<usize>,
    next_group_index: u64,
}

fn lookahead_only_contains_group(lookahead: &RcNode, group: &RcNode) -> bool {
    if let NodeKind::Group { body, .. } = &lookahead.kind {
        body.len() == 1 && body[0].id == group.id
    } else {
        false
    }
}

fn run(last: &DowngradedRegexPattern, unicode: bool) -> (bool, DowngradedRegexPattern) {
    let ast = parse(&last.pattern, unicode).unwrap_or_else(|e| panic!("{}", e.0));
    let needs_wrapping_in_group = matches!(ast.kind, NodeKind::Disjunction { .. });

    let mut walker = Walker {
        actions: Vec::new(),
        needs_start_anchor_replacement: false,
        groups: std::collections::HashMap::new(),
        infinite_groups: HashSet::new(),
        next_group_index: 1,
    };

    let mut passed = false;
    walk_downgrade(&ast, &[], &mut passed, None, &mut walker, true);

    let mut new_pattern = last.pattern.clone();
    let mut atomic_group_offsets = last.atomic_group_offsets.clone();
    let mut need_to_rerun = false;

    // Apply actions by descending reference start so earlier edits do not shift
    // later offsets.
    let mut order: Vec<usize> = (0..walker.actions.len()).collect();
    order.sort_by(|&a, &b| {
        walker.actions[b]
            .reference_start
            .cmp(&walker.actions[a].reference_start)
    });

    for &idx in &order {
        let reference_start = walker.actions[idx].reference_start;
        let reference_end = walker.actions[idx].reference_end;
        let atomic_or_optional = walker.actions[idx].atomic_or_optional;
        let raw = get_raw_without_capturing_groups_or_lookaheads(&walker.actions[idx].group);

        if !raw.references_with_offset.is_empty() {
            need_to_rerun = true;
        }

        let replacement = if atomic_or_optional == AtomicOrOptional::Optional {
            format!("(?:{}?)", raw.result)
        } else {
            raw.result.clone()
        };

        new_pattern =
            utf16_slice_replace(&new_pattern, &replacement, reference_start, reference_end);

        let shift_amount =
            replacement.encode_utf16().count() as i64 - (reference_end - reference_start) as i64;
        atomic_group_offsets =
            shift_offsets(&atomic_group_offsets, reference_start as i64, shift_amount);

        if atomic_or_optional == AtomicOrOptional::Atomic {
            atomic_group_offsets.insert(reference_start);
        }

        // For any other atomic action whose reference appears inside this
        // result, add its inner offset.
        for inner in &walker.actions {
            if inner.atomic_or_optional == AtomicOrOptional::Atomic {
                if let Some((_, offset)) = raw
                    .references_with_offset
                    .iter()
                    .find(|(r, _)| r.id == inner.reference.id)
                {
                    atomic_group_offsets.insert(reference_start + offset);
                }
            }
        }
    }

    if walker.needs_start_anchor_replacement && !already_has_start_anchor_replacement(&ast) {
        if needs_wrapping_in_group {
            new_pattern = format!("(?:{})", new_pattern);
        }
        new_pattern = format!("[^]*?{}", new_pattern);
        let shift = if needs_wrapping_in_group { 8 } else { 5 };
        atomic_group_offsets = shift_offsets(&atomic_group_offsets, -1, shift);
    }

    (
        need_to_rerun,
        DowngradedRegexPattern {
            atomic_group_offsets,
            pattern: new_pattern,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn walk_downgrade(
    node: &RcNode,
    node_stack: &[RcNode],
    passed_start_anchor: &mut bool,
    immediately_preceding_lookahead: Option<&RcNode>,
    walker: &mut Walker,
    _serial: bool,
) {
    let on_consuming_node = |passed: &bool, walker: &mut Walker| {
        if !passed {
            walker.needs_start_anchor_replacement = true;
        }
    };

    match &node.kind {
        NodeKind::CharacterClass { .. }
        | NodeKind::CharacterClassEscape { .. }
        | NodeKind::UnicodePropertyEscape { .. }
        | NodeKind::Value { .. }
        | NodeKind::Dot => {
            on_consuming_node(passed_start_anchor, walker);
        }
        NodeKind::Anchor { kind } => {
            if *kind == AnchorKind::Start {
                *passed_start_anchor = true;
            }
        }
        NodeKind::Group { behavior, body } => {
            let group_index = if *behavior == GroupBehavior::Normal {
                let idx = walker.next_group_index;
                walker.next_group_index += 1;
                Some(idx)
            } else {
                None
            };

            let mut new_stack = node_stack.to_vec();
            new_stack.push(node.clone());
            walk_all_serial(body, &new_stack, passed_start_anchor, walker);

            if let Some(idx) = group_index {
                walker.groups.insert(
                    idx,
                    GroupEntry {
                        group: node.clone(),
                        stack: node_stack.to_vec(),
                    },
                );
            }
        }
        NodeKind::Disjunction { body } => {
            let mut new_stack = node_stack.to_vec();
            new_stack.push(node.clone());
            walk_all_parallel(body, &new_stack, passed_start_anchor, walker);
        }
        NodeKind::Alternative { body } => {
            let mut new_stack = node_stack.to_vec();
            new_stack.push(node.clone());
            walk_all_serial(body, &new_stack, passed_start_anchor, walker);
        }
        NodeKind::Quantifier { min, max, body, .. } => {
            if *max != Some(0) {
                if max.is_none() {
                    for stack_node in node_stack.iter().rev() {
                        if let NodeKind::Group { behavior, .. } = &stack_node.kind {
                            if behavior.is_lookaround() {
                                break;
                            }
                            walker.infinite_groups.insert(stack_node.id);
                        }
                    }
                }
                let mut new_stack = node_stack.to_vec();
                new_stack.push(node.clone());
                if *min == 0 {
                    let mut copy = *passed_start_anchor;
                    walk_all_serial(std::slice::from_ref(body), &new_stack, &mut copy, walker);
                } else {
                    walk_all_serial(
                        std::slice::from_ref(body),
                        &new_stack,
                        passed_start_anchor,
                        walker,
                    );
                }
            }
        }
        NodeKind::Reference { match_index } => {
            on_consuming_node(passed_start_anchor, walker);
            if let Some(entry) = walker.groups.get(match_index) {
                let group = entry.group.clone();
                let group_stack = entry.stack.clone();
                let group_stack_ids: Vec<usize> = group_stack.iter().map(|n| n.id).collect();
                let node_stack_ids: Vec<usize> = node_stack.iter().map(|n| n.id).collect();
                let (local_ids, _) = drop_common(&group_stack_ids, &node_stack_ids);
                let local_stack: Vec<RcNode> = group_stack
                    .iter()
                    .filter(|n| local_ids.contains(&n.id))
                    .cloned()
                    .collect();

                let lookahead_stack: Vec<RcNode> = local_stack
                    .iter()
                    .filter(|n| {
                        matches!(&n.kind, NodeKind::Group { behavior, .. } if behavior.is_lookaround())
                    })
                    .cloned()
                    .collect();
                let group_may_not_be_reached = local_stack.iter().any(|n| match &n.kind {
                    NodeKind::Disjunction { .. } => true,
                    NodeKind::Quantifier { min, .. } => *min == 0,
                    _ => false,
                });
                let group_in_lookahead = !lookahead_stack.is_empty();
                let group_could_be_set = lookahead_stack.iter().all(|n| {
                    matches!(&n.kind, NodeKind::Group { behavior, .. } if !behavior.is_negative())
                });

                if group_could_be_set
                    && (group_in_lookahead || walker.infinite_groups.contains(&group.id))
                {
                    let atomic = immediately_preceding_lookahead
                        .map(|lh| lookahead_only_contains_group(lh, &group))
                        .unwrap_or(false);
                    let optional = group_in_lookahead && group_may_not_be_reached;
                    let atomic_or_optional = if atomic {
                        AtomicOrOptional::Atomic
                    } else if optional {
                        AtomicOrOptional::Optional
                    } else {
                        AtomicOrOptional::None
                    };
                    walker.actions.push(Action {
                        atomic_or_optional,
                        group,
                        reference: node.clone(),
                        reference_start: node.range.0,
                        reference_end: node.range.1,
                    });
                }
            }
        }
        NodeKind::CharacterClassRange { .. } => {}
    }
}

fn walk_all_serial(
    nodes: &[RcNode],
    node_stack: &[RcNode],
    passed_start_anchor: &mut bool,
    walker: &mut Walker,
) {
    let mut just_had_lookahead: Option<RcNode> = None;
    for expression in nodes {
        walk_downgrade(
            expression,
            node_stack,
            passed_start_anchor,
            just_had_lookahead.as_ref(),
            walker,
            true,
        );
        just_had_lookahead = match &expression.kind {
            NodeKind::Group {
                behavior: GroupBehavior::Lookahead,
                ..
            } => Some(expression.clone()),
            _ => None,
        };
    }
}

fn walk_all_parallel(
    nodes: &[RcNode],
    node_stack: &[RcNode],
    passed_start_anchor: &mut bool,
    walker: &mut Walker,
) {
    for expression in nodes {
        let mut copy = *passed_start_anchor;
        walk_downgrade(expression, node_stack, &mut copy, None, walker, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing(pattern: &str) -> bool {
        let ast = parse(pattern, false).unwrap();
        is_missing_start_anchor(&ast)
    }

    #[test]
    fn is_missing_start_anchor_cases() {
        assert!(!missing(""));
        assert!(missing("a"));
        assert!(!missing("^a"));
        assert!(!missing("$^a"));
        assert!(missing("$a"));
        assert!(missing("a^a"));
        assert!(!missing("a{0}^a"));
        assert!(missing("a{0,1}^a"));
        assert!(missing("a{1}^a"));
        assert!(missing("^a|b"));
        assert!(!missing("^a|^b"));
        assert!(!missing("|"));
        assert!(!missing("^(a|b)"));
        assert!(!missing("^ab"));
        assert!(missing("[ab]^c"));
        assert!(missing("(^)?c"));
        assert!(!missing("(^){1}c"));
        assert!(missing("[^]*a"));
        assert!(!missing("[^]*?a"));
        assert!(missing("[^a]*?a"));
        assert!(missing("[]*?a"));
        assert!(missing("b*?a"));
    }

    fn raw(pattern: &str) -> (String, Vec<(String, usize)>) {
        let ast = parse(pattern, false).unwrap();
        let result = get_raw_without_capturing_groups_or_lookaheads(&ast);
        let refs: Vec<(String, usize)> = result
            .references_with_offset
            .iter()
            .map(|(node, offset)| (node.raw.clone(), *offset))
            .collect();
        (result.result, refs)
    }

    #[test]
    fn get_raw_simple() {
        let (result, refs) = raw("(a)");
        assert_eq!(result, "(?:a)");
        assert!(refs.is_empty());
    }

    #[test]
    fn get_raw_every_node_type() {
        let (result, refs) =
            raw("^(a)b{1}c+d{1,2}e+(?:f)(?=g)(?!h)(?<=i)(?<!j)(k|l).(m(n))o+?[a\\d]\\1$");
        assert_eq!(
            result,
            "^(?:a)b{1}c+d{1,2}e+(?:f)(?:k|l).(?:m(?:n))o+?[a\\d]\\1$"
        );
        assert_eq!(refs, vec![("\\1".to_string(), 51)]);
    }

    #[test]
    fn get_raw_multi_reference() {
        let (result, refs) = raw("()()()()a(\\1)c(d|\\2|f)(?:\\3)(?:\\4)+");
        assert_eq!(
            result,
            "(?:)(?:)(?:)(?:)a(?:\\1)c(?:d|\\2|f)(?:\\3)(?:\\4)+"
        );
        assert_eq!(
            refs,
            vec![
                ("\\1".to_string(), 20),
                ("\\2".to_string(), 29),
                ("\\3".to_string(), 37),
                ("\\4".to_string(), 43),
            ]
        );
    }
}
