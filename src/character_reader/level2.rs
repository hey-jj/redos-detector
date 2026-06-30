//! Level 2: record group contents and expand backreferences.
//!
//! Level 2 watches every consumed character and stores it under the capturing
//! groups it belongs to. When a backreference appears, it replays the stored
//! characters of the referenced group with a reference frame pushed on the
//! stack. References into a positive lookaround the reference is not inside, or
//! into an infinite-size group, are unsupported and panic with the engine error
//! message, which the downgrade step is expected to remove first.

use crate::arrays::drop_common;
use crate::ast::{GroupBehavior, NodeKind, RcNode};
use crate::character_groups::CharacterGroups;
use crate::character_reader::level0::SplitSubType;
use crate::character_reader::level0::{get_groups, get_lookahead_stack, Stack, StackEntry};
use crate::character_reader::level1::{
    build_character_reader_level1, Level1Reader, Level1Return, Level1Value, ZeroWidthEntry,
};
use crate::map::must_get;
use crate::node_extra::NodeExtra;
use crate::quantifier::{build_quantifier_iterations, have_had_complete_iteration};
use crate::reader::{build_forkable_reader, BoxReader, ForkableReader, Reader, Step};
use std::collections::HashMap;
use std::rc::Rc;

/// A level-2 reader value.
#[derive(Clone)]
pub(crate) enum Level2Value {
    /// A branch point.
    Split {
        reader: Level2Factory,
        sub_type: SplitSubType,
    },
    /// A consumed character with its group membership and lookaround stack.
    Entry(Level2Entry),
}

/// A consumed-character entry at level 2.
#[derive(Clone)]
pub(crate) struct Level2Entry {
    /// The character set.
    pub(crate) character_groups: CharacterGroups,
    /// The capturing groups this entry is inside.
    pub(crate) groups: Vec<RcNode>,
    /// The lookaround groups this entry is inside, in stack order.
    pub(crate) lookahead_stack: Vec<RcNode>,
    /// The AST node that produced the character.
    pub(crate) node: RcNode,
    /// Zero-width entries carried before this one.
    pub(crate) preceding_zero_width_entries: Vec<ZeroWidthEntry>,
    /// The full stack including reference frames.
    pub(crate) stack: Stack,
}

/// A level-2 return value.
#[derive(Clone, Debug)]
pub(crate) enum Level2Return {
    /// The pattern ended.
    End {
        bounded: bool,
        preceding_zero_width_entries: Vec<ZeroWidthEntry>,
    },
    /// The reader bailed out of an empty-reference loop.
    Abort,
}

/// A level-2 reader.
pub(crate) type Level2Reader = BoxReader<Level2Value, Level2Return>;

/// A factory that builds a level-2 reader for a split branch.
pub(crate) type Level2Factory = Rc<dyn Fn() -> Level2Reader>;

/// A recorded group-contents entry.
#[derive(Clone)]
struct GroupEntry {
    character_groups: CharacterGroups,
    node: RcNode,
    preceding_zero_width_entries: Vec<ZeroWidthEntry>,
    stack: Stack,
}

#[derive(Clone)]
struct GroupContents {
    contents: Vec<GroupEntry>,
    group: RcNode,
}

/// An internal value derived from level 1 whose stack can include references.
#[derive(Clone)]
enum InternalValue {
    Split {
        reader: Rc<dyn Fn() -> ForkableReader<InternalValue, Level1Return>>,
        sub_type: SplitSubType,
    },
    Groups {
        character_groups: CharacterGroups,
        node: RcNode,
        stack: Stack,
        preceding_zero_width_entries: Vec<ZeroWidthEntry>,
    },
    Reference {
        node: RcNode,
        reference_index: u64,
        stack: Stack,
        preceding_zero_width_entries: Vec<ZeroWidthEntry>,
    },
}

/// Wraps a level-1 reader so its values become internal values.
struct InternalFromLevel1 {
    inner: Level1Reader,
}

impl Reader<InternalValue, Level1Return> for InternalFromLevel1 {
    fn next(&mut self) -> Step<InternalValue, Level1Return> {
        match self.inner.next() {
            Step::Done(ret) => Step::Done(ret),
            Step::Value(value) => match value {
                Level1Value::Split { reader, sub_type } => {
                    let factory: Rc<dyn Fn() -> ForkableReader<InternalValue, Level1Return>> =
                        Rc::new(move || {
                            build_forkable_reader(Box::new(InternalFromLevel1 { inner: reader() }))
                        });
                    Step::Value(InternalValue::Split {
                        reader: factory,
                        sub_type,
                    })
                }
                Level1Value::Groups {
                    character_groups,
                    node,
                    stack,
                    preceding_zero_width_entries,
                } => Step::Value(InternalValue::Groups {
                    character_groups,
                    node,
                    stack,
                    preceding_zero_width_entries,
                }),
                Level1Value::Reference {
                    node,
                    reference_index,
                    stack,
                    preceding_zero_width_entries,
                } => Step::Value(InternalValue::Reference {
                    node,
                    reference_index,
                    stack,
                    preceding_zero_width_entries,
                }),
            },
        }
    }
}

type InternalReader = ForkableReader<InternalValue, Level1Return>;

struct Level2State {
    character_reader: InternalReader,
    group_contents_store: HashMap<u64, GroupContents>,
    groups_with_infinite_size: Vec<u64>,
    preceding_zero_width_entries: Vec<ZeroWidthEntry>,
    quantifier_iterations_at_last_group: HashMap<usize, u64>,
    reference_reader: Option<(InternalReader, RcNode)>,
}

struct Level2ReaderImpl {
    state: Option<Level2State>,
    node_extra: Rc<NodeExtra>,
}

impl Reader<Level2Value, Level2Return> for Level2ReaderImpl {
    fn next(&mut self) -> Step<Level2Value, Level2Return> {
        loop {
            let state = self.state.as_mut().expect("level 2 polled after end");

            let active = if state.reference_reader.is_some() {
                &mut state.reference_reader.as_mut().unwrap().0
            } else {
                &mut state.character_reader
            };

            let result = active.next();

            match result {
                Step::Done(ret) => {
                    if state.reference_reader.is_some() {
                        if ret.bounded {
                            panic!("Internal error: end of reference reader cannot be bounded");
                        }
                        state
                            .preceding_zero_width_entries
                            .extend(ret.preceding_zero_width_entries);
                        state.reference_reader = None;
                        continue;
                    }
                    let mut preceding = std::mem::take(&mut state.preceding_zero_width_entries);
                    preceding.extend(ret.preceding_zero_width_entries);
                    self.state = None;
                    return Step::Done(Level2Return::End {
                        bounded: ret.bounded,
                        preceding_zero_width_entries: preceding,
                    });
                }
                Step::Value(value) => match value {
                    InternalValue::Split { reader, sub_type } => {
                        if state.reference_reader.is_some() {
                            panic!("Internal error: should not be seeing a split from a reference reader");
                        }
                        let group_contents_store = state.group_contents_store.clone();
                        let groups_with_infinite_size = state.groups_with_infinite_size.clone();
                        let preceding = state.preceding_zero_width_entries.clone();
                        let quantifier_iterations_at_last_group =
                            state.quantifier_iterations_at_last_group.clone();
                        let node_extra = Rc::clone(&self.node_extra);
                        let factory: Level2Factory = Rc::new(move || {
                            Box::new(Level2ReaderImpl {
                                state: Some(Level2State {
                                    character_reader: reader(),
                                    group_contents_store: group_contents_store.clone(),
                                    groups_with_infinite_size: groups_with_infinite_size.clone(),
                                    preceding_zero_width_entries: preceding.clone(),
                                    quantifier_iterations_at_last_group:
                                        quantifier_iterations_at_last_group.clone(),
                                    reference_reader: None,
                                }),
                                node_extra: Rc::clone(&node_extra),
                            })
                        });
                        return Step::Value(Level2Value::Split {
                            reader: factory,
                            sub_type,
                        });
                    }
                    InternalValue::Reference {
                        node,
                        reference_index,
                        stack,
                        preceding_zero_width_entries,
                    } => {
                        state
                            .preceding_zero_width_entries
                            .extend(preceding_zero_width_entries);
                        if state.reference_reader.is_some() {
                            panic!("Internal error: should not be seeing a reference from a reference reader");
                        }
                        let quantifier_iterations = build_quantifier_iterations(&stack);
                        if have_had_complete_iteration(
                            &state.quantifier_iterations_at_last_group,
                            &quantifier_iterations,
                        ) {
                            self.state = None;
                            return Step::Done(Level2Return::Abort);
                        }
                        let groups = get_groups(&stack);
                        let contents_reader = build_group_contents_reader(
                            &state.group_contents_store,
                            &groups,
                            &state.groups_with_infinite_size,
                            &self.node_extra,
                            reference_index,
                            &node,
                            &stack,
                        );
                        state.reference_reader =
                            Some((build_forkable_reader(contents_reader), node));
                    }
                    InternalValue::Groups {
                        character_groups,
                        node,
                        stack,
                        preceding_zero_width_entries,
                    } => {
                        state
                            .preceding_zero_width_entries
                            .extend(preceding_zero_width_entries);
                        let quantifier_iterations = build_quantifier_iterations(&stack);
                        let lookahead_stack = get_lookahead_stack(&stack);
                        let groups = get_groups(&stack);

                        let inside_reference = state.reference_reader.is_some();
                        record_groups_entry(
                            state,
                            &self.node_extra,
                            &character_groups,
                            &node,
                            &stack,
                            inside_reference,
                        );
                        state.quantifier_iterations_at_last_group = quantifier_iterations;

                        let preceding = std::mem::take(&mut state.preceding_zero_width_entries);
                        return Step::Value(Level2Value::Entry(Level2Entry {
                            character_groups,
                            groups,
                            lookahead_stack,
                            node,
                            preceding_zero_width_entries: preceding,
                            stack,
                        }));
                    }
                },
            }
        }
    }
}

/// Records a groups entry into its capturing groups and tracks infinite size.
fn record_groups_entry(
    state: &mut Level2State,
    node_extra: &NodeExtra,
    character_groups: &CharacterGroups,
    node: &RcNode,
    stack: &[StackEntry],
    inside_reference: bool,
) {
    if !inside_reference {
        // Clear groups now ahead of the current position.
        let mut offsets: Vec<usize> = state
            .preceding_zero_width_entries
            .iter()
            .map(|z| z.offset)
            .collect();
        offsets.push(node.range.0);
        let to_remove: Vec<u64> = state
            .group_contents_store
            .iter()
            .filter_map(|(index, contents)| {
                if offsets
                    .iter()
                    .any(|&offset| contents.group.range.0 >= offset)
                {
                    Some(*index)
                } else {
                    None
                }
            })
            .collect();
        for index in to_remove {
            state.group_contents_store.remove(&index);
        }
    }

    let mut group_infinite_size = false;
    let reversed: Vec<&StackEntry> = stack.iter().rev().collect();
    let reference_index = reversed
        .iter()
        .position(|entry| matches!(entry, StackEntry::Reference { .. }));
    let portion_len = reference_index.unwrap_or(stack.len());

    for stack_entry in &reversed[..portion_len] {
        match stack_entry {
            StackEntry::Quantifier { quantifier, .. } => {
                if let NodeKind::Quantifier { max, .. } = &quantifier.kind {
                    if max.is_none() {
                        group_infinite_size = true;
                        continue;
                    }
                }
                continue;
            }
            StackEntry::Reference { .. } => continue,
            StackEntry::Group { group } => {
                let behavior = match &group.kind {
                    NodeKind::Group { behavior, .. } => *behavior,
                    _ => continue,
                };
                if behavior.is_lookaround() {
                    group_infinite_size = false;
                    continue;
                }
                if behavior != GroupBehavior::Normal {
                    continue;
                }
                let index = *must_get(&node_extra.capturing_group_to_index, &group.id);
                if group_infinite_size && !state.groups_with_infinite_size.contains(&index) {
                    state.groups_with_infinite_size.push(index);
                }
                let entry_preceding: Vec<ZeroWidthEntry> = state
                    .preceding_zero_width_entries
                    .iter()
                    .filter(|z| z.offset >= group.range.0 && z.offset <= group.range.1)
                    .cloned()
                    .collect();
                let contents =
                    state
                        .group_contents_store
                        .entry(index)
                        .or_insert_with(|| GroupContents {
                            contents: Vec::new(),
                            group: group.clone(),
                        });
                contents.contents.push(GroupEntry {
                    character_groups: character_groups.clone(),
                    node: node.clone(),
                    preceding_zero_width_entries: entry_preceding,
                    stack: stack.to_vec(),
                });
            }
        }
    }
}

/// Builds a reader over the recorded contents of a referenced group.
#[allow(clippy::too_many_arguments)]
fn build_group_contents_reader(
    group_contents_store: &HashMap<u64, GroupContents>,
    groups: &[RcNode],
    groups_with_infinite_size: &[u64],
    node_extra: &NodeExtra,
    reference_index: u64,
    reference_node: &RcNode,
    reference_stack: &Stack,
) -> Box<dyn Reader<InternalValue, Level1Return>> {
    let group = must_get(&node_extra.index_to_capturing_group, &reference_index).clone();
    let group_contents = group_contents_store.get(&reference_index).cloned();
    let contents = group_contents
        .as_ref()
        .map(|c| c.contents.clone())
        .unwrap_or_default();
    let contents_group = group_contents
        .as_ref()
        .map(|c| c.group.clone())
        .unwrap_or_else(|| group.clone());

    let group_lookahead = must_get(&node_extra.node_to_lookahead_stack, &contents_group.id).clone();
    let reference_lookahead =
        must_get(&node_extra.node_to_lookahead_stack, &reference_node.id).clone();
    let group_ids: Vec<usize> = group_lookahead.iter().map(|n| n.id).collect();
    let reference_ids: Vec<usize> = reference_lookahead.iter().map(|n| n.id).collect();
    let (extra_group_ids, _) = drop_common(&group_ids, &reference_ids);

    if !extra_group_ids.is_empty() {
        let has_negative = group_lookahead.iter().any(|g| {
            extra_group_ids.contains(&g.id)
                && matches!(
                    &g.kind,
                    NodeKind::Group { behavior, .. } if behavior.is_negative()
                )
        });
        if has_negative {
            return empty_internal_reader();
        }
        panic!(
            "Unsupported reference ({} at position {}). Pattern needs downgrading. See the `downgradePattern` option.",
            reference_index, reference_node.range.0
        );
    }

    if groups.iter().any(|g| g.id == contents_group.id) {
        return empty_internal_reader();
    }

    if groups_with_infinite_size.contains(&reference_index) {
        panic!(
            "Unsupported reference to group {} as group is not a finite size. Pattern needs downgrading. See the `downgradePattern` option.",
            reference_index
        );
    }

    let reference_node = reference_node.clone();
    let reference_stack = reference_stack.clone();
    let values: Vec<InternalValue> = contents
        .into_iter()
        .map(|group_entry| {
            let mut stack = group_entry.stack;
            stack.push(StackEntry::Reference {
                reference: reference_node.clone(),
            });
            stack.extend(reference_stack.iter().cloned());
            InternalValue::Groups {
                character_groups: group_entry.character_groups,
                node: group_entry.node,
                stack,
                preceding_zero_width_entries: group_entry.preceding_zero_width_entries,
            }
        })
        .collect();

    Box::new(InternalArrayReader {
        items: values.into_iter(),
    })
}

struct InternalArrayReader {
    items: std::vec::IntoIter<InternalValue>,
}

impl Reader<InternalValue, Level1Return> for InternalArrayReader {
    fn next(&mut self) -> Step<InternalValue, Level1Return> {
        match self.items.next() {
            Some(value) => Step::Value(value),
            None => Step::Done(Level1Return {
                bounded: false,
                preceding_zero_width_entries: Vec::new(),
            }),
        }
    }
}

fn empty_internal_reader() -> Box<dyn Reader<InternalValue, Level1Return>> {
    Box::new(InternalArrayReader {
        items: Vec::new().into_iter(),
    })
}

/// Builds the level-2 reader for `node`.
pub(crate) fn build_character_reader_level2(
    case_insensitive: bool,
    dot_all: bool,
    node: &RcNode,
    node_extra: Rc<NodeExtra>,
) -> Level2Reader {
    let level1 = build_character_reader_level1(case_insensitive, dot_all, node);
    let internal =
        build_forkable_reader(Box::new(InternalFromLevel1 { inner: level1 })
            as BoxReader<InternalValue, Level1Return>);
    Box::new(Level2ReaderImpl {
        state: Some(Level2State {
            character_reader: internal,
            group_contents_store: HashMap::new(),
            groups_with_infinite_size: Vec::new(),
            preceding_zero_width_entries: Vec::new(),
            quantifier_iterations_at_last_group: HashMap::new(),
            reference_reader: None,
        }),
        node_extra,
    })
}
