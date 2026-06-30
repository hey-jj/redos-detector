//! Quantifier accounting used by the readers and the checker.

use crate::ast::{GroupBehavior, NodeKind};
use crate::character_reader::level0::StackEntry;
use crate::map::must_get;
use crate::node_extra::NodeExtra;
use std::collections::HashMap;

/// Maps each quantifier in a stack to its most recent iteration.
///
/// Keyed by node id. Later frames overwrite earlier ones, so the deepest
/// iteration wins per quantifier.
pub(crate) fn build_quantifier_iterations(stack: &[StackEntry]) -> HashMap<usize, u64> {
    let mut res = HashMap::new();
    for entry in stack {
        if let StackEntry::Quantifier {
            iteration,
            quantifier,
        } = entry
        {
            res.insert(quantifier.id, *iteration);
        }
    }
    res
}

/// Returns the quantifiers whose post-minimum iterations can be collapsed.
///
/// If the stack is inside a capturing group that some reachable reference points
/// at, every iteration matters, so the result is empty. Otherwise it is the set
/// of quantifiers iterating beyond their first mandatory pass.
pub(crate) fn build_quantifiers_in_infinite_portion(
    stack: &[StackEntry],
    node_extra: &NodeExtra,
) -> Vec<usize> {
    let exhaustive = stack.iter().any(|entry| {
        if let StackEntry::Group { group } = entry {
            if let NodeKind::Group { behavior, .. } = &group.kind {
                if *behavior == GroupBehavior::Normal {
                    let index = *must_get(&node_extra.capturing_group_to_index, &group.id);
                    return node_extra.reachable_references.iter().any(
                        |reference| match &reference.kind {
                            NodeKind::Reference { match_index } => *match_index == index,
                            _ => false,
                        },
                    );
                }
            }
        }
        false
    });

    if exhaustive {
        return Vec::new();
    }

    let mut result = Vec::new();
    for entry in stack {
        if let StackEntry::Quantifier {
            iteration,
            quantifier,
        } = entry
        {
            if let NodeKind::Quantifier { min, .. } = &quantifier.kind {
                if *iteration >= 1 && *iteration >= *min && !result.contains(&quantifier.id) {
                    result.push(quantifier.id);
                }
            }
        }
    }
    result
}

/// Returns `true` when any quantifier advanced by more than one iteration.
pub(crate) fn have_had_complete_iteration(
    before: &HashMap<usize, u64>,
    now: &HashMap<usize, u64>,
) -> bool {
    for (quantifier, iterations_now) in now {
        let iterations_before = before.get(quantifier).copied().unwrap_or(0);
        if iterations_now.saturating_sub(iterations_before) > 1 {
            return true;
        }
    }
    false
}
