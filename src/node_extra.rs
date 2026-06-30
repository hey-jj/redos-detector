//! Precomputed facts about a parsed pattern.
//!
//! A single pass over the AST assigns capturing-group indices, records the
//! lookaround stack enclosing each group and reference, and collects the
//! references that are reachable (not under a `{0}` quantifier).

use crate::ast::{GroupBehavior, NodeKind, RcNode};
use std::collections::HashMap;

/// Facts about a pattern, keyed by node id.
pub(crate) struct NodeExtra {
    /// Capturing group node id to its 1-based index.
    pub(crate) capturing_group_to_index: HashMap<usize, u64>,
    /// 1-based index to the capturing group node.
    pub(crate) index_to_capturing_group: HashMap<u64, RcNode>,
    /// Group or reference node id to the lookaround groups enclosing it.
    pub(crate) node_to_lookahead_stack: HashMap<usize, Vec<RcNode>>,
    /// References reached with a live path.
    pub(crate) reachable_references: Vec<RcNode>,
}

/// Builds [`NodeExtra`] for `root`.
pub(crate) fn build_node_extra(root: &RcNode) -> NodeExtra {
    let mut extra = NodeExtra {
        capturing_group_to_index: HashMap::new(),
        index_to_capturing_group: HashMap::new(),
        node_to_lookahead_stack: HashMap::new(),
        reachable_references: Vec::new(),
    };
    visit(root, &[], true, &mut extra);
    extra
}

fn visit(node: &RcNode, lookahead_stack: &[RcNode], reachable: bool, extra: &mut NodeExtra) {
    match &node.kind {
        NodeKind::Anchor { .. }
        | NodeKind::CharacterClass { .. }
        | NodeKind::CharacterClassEscape { .. }
        | NodeKind::CharacterClassRange { .. }
        | NodeKind::UnicodePropertyEscape { .. }
        | NodeKind::Value { .. }
        | NodeKind::Dot => {}
        NodeKind::Reference { .. } => {
            extra
                .node_to_lookahead_stack
                .insert(node.id, lookahead_stack.to_vec());
            if reachable {
                extra.reachable_references.push(node.clone());
            }
        }
        NodeKind::Alternative { body } | NodeKind::Disjunction { body } => {
            for child in body {
                visit(child, lookahead_stack, reachable, extra);
            }
        }
        NodeKind::Group { behavior, body } => {
            if *behavior == GroupBehavior::Normal {
                let index = extra.capturing_group_to_index.len() as u64 + 1;
                extra.capturing_group_to_index.insert(node.id, index);
                extra.index_to_capturing_group.insert(index, node.clone());
                extra
                    .node_to_lookahead_stack
                    .insert(node.id, lookahead_stack.to_vec());
            }

            let mut new_stack = lookahead_stack.to_vec();
            if behavior.is_lookaround() {
                new_stack.push(node.clone());
            }
            for child in body {
                visit(child, &new_stack, reachable, extra);
            }
        }
        NodeKind::Quantifier { max, body, .. } => {
            let child_reachable = reachable && *max != Some(0);
            visit(body, lookahead_stack, child_reachable, extra);
        }
    }
}
