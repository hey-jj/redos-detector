//! Level 0: per-node character readers and the value types.

use crate::ast::{AnchorKind, ClassEscape, GroupBehavior, NodeKind, RcNode};
use crate::character_groups::{CharacterGroups, EscapeMap};
use crate::character_reader::join::{join, join_array, JoinAction};
use crate::character_reader::map::map;
use crate::code_point::{build_code_point_ranges, to_upper_case_code_point};
use crate::our_range::{invert_ranges, OurRange};
use crate::reader::{build_array_reader, chain_readers, empty_reader, BoxReader, Reader, Step};
use std::rc::Rc;

/// A lookaround marker on a split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitSubType {
    /// A plain branch (disjunction or quantifier stop).
    None,
    /// A positive lookahead.
    Lookahead,
    /// A negative lookahead.
    NegativeLookahead,
    /// A positive lookbehind.
    Lookbehind,
    /// A negative lookbehind.
    NegativeLookbehind,
}

impl SplitSubType {
    /// Maps a group behavior to the matching split sub-type.
    pub(crate) fn from_behavior(behavior: GroupBehavior) -> Self {
        match behavior {
            GroupBehavior::Lookahead => SplitSubType::Lookahead,
            GroupBehavior::NegativeLookahead => SplitSubType::NegativeLookahead,
            GroupBehavior::Lookbehind => SplitSubType::Lookbehind,
            GroupBehavior::NegativeLookbehind => SplitSubType::NegativeLookbehind,
            _ => SplitSubType::None,
        }
    }
}

/// One stack frame describing the nesting that produced an entry.
#[derive(Clone, Debug)]
pub(crate) enum StackEntry {
    /// A group frame.
    Group { group: RcNode },
    /// A quantifier frame at a given iteration.
    Quantifier { iteration: u64, quantifier: RcNode },
    /// A backreference frame added by level 2.
    Reference { reference: RcNode },
}

/// The stack carried by an entry.
pub(crate) type Stack = Vec<StackEntry>;

/// A factory that builds a fresh reader for a split branch.
pub(crate) type ReaderFactory = Rc<dyn Fn() -> BoxReader<CharacterReaderValue, ()>>;

/// A level-0 reader value.
#[derive(Clone)]
pub(crate) enum CharacterReaderValue {
    /// A branch point. `reader` builds the branch's reader.
    Split {
        reader: ReaderFactory,
        sub_type: SplitSubType,
    },
    /// A consumed character with its character set.
    Groups {
        character_groups: CharacterGroups,
        node: RcNode,
        stack: Stack,
    },
    /// A backreference not yet expanded.
    Reference {
        node: RcNode,
        reference_index: u64,
        stack: Stack,
    },
    /// The pattern end. `bounded` is `true` for a hard `$`.
    End {
        bounded: bool,
        offset: usize,
        stack: Stack,
    },
    /// A zero-width null marker.
    Null { offset: usize, stack: Stack },
    /// A `^` start anchor was passed.
    Start { offset: usize, stack: Stack },
}

impl CharacterReaderValue {
    /// Returns `true` for null, start, and end markers.
    pub(crate) fn is_zero_width_or_end(&self) -> bool {
        matches!(
            self,
            CharacterReaderValue::Null { .. }
                | CharacterReaderValue::Start { .. }
                | CharacterReaderValue::End { .. }
        )
    }
}

/// A level-0 reader.
pub(crate) type CharacterReader = BoxReader<CharacterReaderValue, ()>;

/// Builds the level-0 reader for `node`.
pub(crate) fn build_character_reader(
    case_insensitive: bool,
    dot_all: bool,
    node: &RcNode,
) -> CharacterReader {
    match &node.kind {
        NodeKind::Anchor { kind } => build_anchor_reader(node, *kind),
        NodeKind::CharacterClass { .. } => build_character_class_reader(case_insensitive, node),
        NodeKind::CharacterClassEscape { value } => build_class_escape_reader(node, *value),
        NodeKind::UnicodePropertyEscape { negative, value } => {
            build_unicode_property_escape_reader(node, *negative, value.clone())
        }
        NodeKind::Reference { match_index } => build_reference_reader(node, *match_index),
        NodeKind::Value { code_point } => build_value_reader(case_insensitive, node, *code_point),
        NodeKind::Dot => build_dot_reader(dot_all, node),
        NodeKind::Alternative { body } => build_sequence_reader(case_insensitive, dot_all, body),
        NodeKind::Disjunction { body } => build_disjunction_reader(case_insensitive, dot_all, body),
        NodeKind::Group { behavior, body } => {
            build_group_reader(case_insensitive, dot_all, node, *behavior, body)
        }
        NodeKind::Quantifier { min, max, body, .. } => {
            build_quantifier_reader(case_insensitive, dot_all, node, *min, *max, body)
        }
        NodeKind::CharacterClassRange { .. } => {
            // Ranges only appear inside a character class, never standalone.
            empty_reader()
        }
    }
}

/// Returns the code point a value matches, folded to uppercase when requested.
pub(crate) fn code_point_from_value(case_insensitive: bool, code_point: u32) -> u32 {
    if case_insensitive {
        to_upper_case_code_point(code_point)
    } else {
        code_point
    }
}

fn build_value_reader(case_insensitive: bool, node: &RcNode, code_point: u32) -> CharacterReader {
    let cp = code_point_from_value(case_insensitive, code_point);
    build_array_reader(vec![CharacterReaderValue::Groups {
        character_groups: CharacterGroups {
            ranges: vec![(cp as f64, cp as f64)],
            ranges_negated: false,
            unicode_property_escapes: EscapeMap::new(),
        },
        node: node.clone(),
        stack: Vec::new(),
    }])
}

fn build_dot_reader(dot_all: bool, node: &RcNode) -> CharacterReader {
    let ranges: Vec<OurRange> = if dot_all {
        vec![]
    } else {
        vec![(10.0, 10.0), (13.0, 13.0), (8232.0, 8233.0)]
    };
    build_array_reader(vec![CharacterReaderValue::Groups {
        character_groups: CharacterGroups {
            ranges,
            ranges_negated: true,
            unicode_property_escapes: EscapeMap::new(),
        },
        node: node.clone(),
        stack: Vec::new(),
    }])
}

fn build_class_escape_reader(node: &RcNode, value: ClassEscape) -> CharacterReader {
    build_array_reader(vec![CharacterReaderValue::Groups {
        character_groups: CharacterGroups {
            ranges: class_escape_ranges(value),
            ranges_negated: false,
            unicode_property_escapes: EscapeMap::new(),
        },
        node: node.clone(),
        stack: Vec::new(),
    }])
}

/// Returns the code point ranges a `\d \D \w \W \s \S` escape matches.
pub(crate) fn class_escape_ranges(value: ClassEscape) -> Vec<OurRange> {
    let d: Vec<OurRange> = vec![(48.0, 57.0)];
    let w: Vec<OurRange> = vec![(48.0, 57.0), (65.0, 90.0), (95.0, 95.0), (97.0, 122.0)];
    let s: Vec<OurRange> = vec![
        (9.0, 9.0),
        (10.0, 10.0),
        (11.0, 11.0),
        (12.0, 12.0),
        (13.0, 13.0),
        (32.0, 32.0),
        (160.0, 160.0),
        (5760.0, 5760.0),
        (8192.0, 8202.0),
        (8232.0, 8233.0),
        (8239.0, 8239.0),
        (8287.0, 8287.0),
        (12288.0, 12288.0),
        (65279.0, 65279.0),
    ];
    match value {
        ClassEscape::D => d,
        ClassEscape::DUpper => invert_ranges(&d),
        ClassEscape::W => w,
        ClassEscape::WUpper => invert_ranges(&w),
        ClassEscape::S => s,
        ClassEscape::SUpper => invert_ranges(&s),
    }
}

fn build_unicode_property_escape_reader(
    node: &RcNode,
    negative: bool,
    value: String,
) -> CharacterReader {
    build_array_reader(vec![CharacterReaderValue::Groups {
        character_groups: CharacterGroups {
            ranges: vec![],
            ranges_negated: false,
            unicode_property_escapes: EscapeMap::single(value, negative),
        },
        node: node.clone(),
        stack: Vec::new(),
    }])
}

fn build_reference_reader(node: &RcNode, reference_index: u64) -> CharacterReader {
    build_array_reader(vec![CharacterReaderValue::Reference {
        node: node.clone(),
        reference_index,
        stack: Vec::new(),
    }])
}

fn build_anchor_reader(node: &RcNode, kind: AnchorKind) -> CharacterReader {
    match kind {
        AnchorKind::End => build_array_reader(vec![CharacterReaderValue::End {
            bounded: true,
            offset: node.range.0,
            stack: Vec::new(),
        }]),
        AnchorKind::Start => build_array_reader(vec![CharacterReaderValue::Start {
            offset: node.range.0,
            stack: Vec::new(),
        }]),
        AnchorKind::Boundary | AnchorKind::NotBoundary => empty_reader(),
    }
}

/// Builds a reader that yields a single unbounded end marker.
pub(crate) fn build_end_reader(offset: usize) -> CharacterReader {
    build_array_reader(vec![CharacterReaderValue::End {
        bounded: false,
        offset,
        stack: Vec::new(),
    }])
}

/// Builds a reader that yields a single null marker.
pub(crate) fn build_null_reader(offset: usize) -> CharacterReader {
    build_array_reader(vec![CharacterReaderValue::Null {
        offset,
        stack: Vec::new(),
    }])
}

fn build_character_class_reader(case_insensitive: bool, node: &RcNode) -> CharacterReader {
    let (negative, body) = match &node.kind {
        NodeKind::CharacterClass { negative, body } => (*negative, body),
        _ => unreachable!("character class reader on non-class node"),
    };

    let mut ranges: Vec<OurRange> = Vec::new();
    let mut escapes = EscapeMap::new();
    let mut matches_everything = false;

    for expression in body {
        match &expression.kind {
            NodeKind::Value { code_point } => {
                let cp = code_point_from_value(case_insensitive, *code_point);
                ranges.push((cp as f64, cp as f64));
            }
            NodeKind::CharacterClassRange { min, max } => {
                let low = min.code_point().unwrap_or(0);
                let high = max.code_point().unwrap_or(0);
                ranges.extend(build_code_point_ranges(case_insensitive, low, high));
            }
            NodeKind::CharacterClassEscape { value } => {
                ranges.extend(class_escape_ranges(*value));
            }
            NodeKind::UnicodePropertyEscape {
                negative: expr_negative,
                value,
            } => {
                let resolved_negative = *expr_negative != negative;
                if escapes.get(value) == Some(!resolved_negative) {
                    matches_everything = true;
                    break;
                }
                escapes.set(value.clone(), resolved_negative);
            }
            _ => {}
        }
    }

    let character_groups = if matches_everything {
        CharacterGroups {
            ranges: vec![],
            ranges_negated: true,
            unicode_property_escapes: EscapeMap::new(),
        }
    } else {
        CharacterGroups {
            ranges,
            ranges_negated: negative,
            unicode_property_escapes: escapes,
        }
    };

    build_array_reader(vec![CharacterReaderValue::Groups {
        character_groups,
        node: node.clone(),
        stack: Vec::new(),
    }])
}

fn build_sequence_reader(
    case_insensitive: bool,
    dot_all: bool,
    nodes: &[RcNode],
) -> CharacterReader {
    let factories: Vec<Box<dyn Fn() -> CharacterReader>> = nodes
        .iter()
        .map(|node| {
            let node = node.clone();
            Box::new(move || build_character_reader(case_insensitive, dot_all, &node))
                as Box<dyn Fn() -> CharacterReader>
        })
        .collect();
    join_array(factories)
}

fn build_disjunction_reader(
    case_insensitive: bool,
    dot_all: bool,
    body: &[RcNode],
) -> CharacterReader {
    let last = body.len() - 1;
    let mut split_values: Vec<CharacterReaderValue> = Vec::new();
    for part in &body[..last] {
        let part = part.clone();
        let reader: ReaderFactory =
            Rc::new(move || build_character_reader(case_insensitive, dot_all, &part));
        split_values.push(CharacterReaderValue::Split {
            reader,
            sub_type: SplitSubType::None,
        });
    }
    let last_reader = build_character_reader(case_insensitive, dot_all, &body[last]);
    chain_readers(vec![build_array_reader(split_values), last_reader])
}

fn build_group_reader(
    case_insensitive: bool,
    dot_all: bool,
    node: &RcNode,
    behavior: GroupBehavior,
    body: &[RcNode],
) -> CharacterReader {
    match behavior {
        GroupBehavior::Lookbehind
        | GroupBehavior::NegativeLookbehind
        | GroupBehavior::Lookahead
        | GroupBehavior::NegativeLookahead => {
            let body: Vec<RcNode> = body.to_vec();
            let group = node.clone();
            let end_offset = node.range.1;
            let reader: ReaderFactory = Rc::new(move || {
                let mapped_factory: Box<dyn Fn() -> CharacterReader> = {
                    let body = body.clone();
                    let group = group.clone();
                    Box::new(move || {
                        let group_frame = group.clone();
                        map(
                            build_sequence_reader(case_insensitive, dot_all, &body),
                            move |value| push_group_frame(value, &group_frame),
                        )
                    })
                };
                let end_reader_factory: Box<dyn Fn() -> CharacterReader> =
                    Box::new(move || build_end_reader(end_offset));
                join_array(vec![mapped_factory, end_reader_factory])
            });
            build_array_reader(vec![CharacterReaderValue::Split {
                reader,
                sub_type: SplitSubType::from_behavior(behavior),
            }])
        }
        GroupBehavior::Ignore | GroupBehavior::Normal => {
            let group = node.clone();
            map(
                build_sequence_reader(case_insensitive, dot_all, body),
                move |value| push_group_frame(value, &group),
            )
        }
    }
}

/// Prepends a group frame to an entry's stack.
fn push_group_frame(value: CharacterReaderValue, group: &RcNode) -> CharacterReaderValue {
    with_pushed_frame(
        value,
        StackEntry::Group {
            group: group.clone(),
        },
    )
}

/// Returns a copy of `value` with `frame` prepended to its stack.
pub(crate) fn with_pushed_frame(
    value: CharacterReaderValue,
    frame: StackEntry,
) -> CharacterReaderValue {
    match value {
        CharacterReaderValue::Groups {
            character_groups,
            node,
            mut stack,
        } => {
            stack.insert(0, frame);
            CharacterReaderValue::Groups {
                character_groups,
                node,
                stack,
            }
        }
        CharacterReaderValue::Reference {
            node,
            reference_index,
            mut stack,
        } => {
            stack.insert(0, frame);
            CharacterReaderValue::Reference {
                node,
                reference_index,
                stack,
            }
        }
        CharacterReaderValue::End {
            bounded,
            offset,
            mut stack,
        } => {
            stack.insert(0, frame);
            CharacterReaderValue::End {
                bounded,
                offset,
                stack,
            }
        }
        CharacterReaderValue::Null { offset, mut stack } => {
            stack.insert(0, frame);
            CharacterReaderValue::Null { offset, stack }
        }
        CharacterReaderValue::Start { offset, mut stack } => {
            stack.insert(0, frame);
            CharacterReaderValue::Start { offset, stack }
        }
        CharacterReaderValue::Split { .. } => value,
    }
}

fn build_quantifier_reader(
    case_insensitive: bool,
    dot_all: bool,
    node: &RcNode,
    min: u64,
    max: Option<u64>,
    body: &RcNode,
) -> CharacterReader {
    let body_offset = body.range.0;
    let quantifier = node.clone();
    let body = body.clone();
    let max_value = max;

    let null_first: Box<dyn Fn() -> CharacterReader> =
        Box::new(move || build_null_reader(body_offset));

    let iterated: Box<dyn Fn() -> CharacterReader> = Box::new(move || {
        let quantifier = quantifier.clone();
        let body = body.clone();
        let get_action = move |i: u64, time_since_emit: u64| -> JoinAction {
            if time_since_emit > 1 {
                return JoinAction::Stop;
            }
            if let Some(m) = max_value {
                if i >= m {
                    return JoinAction::Stop;
                }
            }
            if i >= min {
                return JoinAction::Fork;
            }
            JoinAction::Continue
        };
        let get_reader = move |i: u64| -> CharacterReader {
            let quantifier = quantifier.clone();
            let body = body.clone();
            let mut factories: Vec<Box<dyn Fn() -> CharacterReader>> = Vec::new();
            if i > 0 {
                factories.push(Box::new(move || build_null_reader(body_offset)));
            }
            let body_for_reader = body.clone();
            factories.push(Box::new(move || {
                build_character_reader(case_insensitive, dot_all, &body_for_reader)
            }));
            map(join_array(factories), move |value| {
                with_pushed_frame(
                    value,
                    StackEntry::Quantifier {
                        iteration: i,
                        quantifier: quantifier.clone(),
                    },
                )
            })
        };
        join(get_action, get_reader)
    });

    join_array(vec![null_first, iterated])
}

/// Returns the group frames present in a stack.
pub(crate) fn get_groups(stack: &[StackEntry]) -> Vec<RcNode> {
    stack
        .iter()
        .filter_map(|entry| match entry {
            StackEntry::Group { group } => Some(group.clone()),
            _ => None,
        })
        .collect()
}

/// Returns the lookaround group frames in a stack, in stack order.
pub(crate) fn get_lookahead_stack(stack: &[StackEntry]) -> Vec<RcNode> {
    stack
        .iter()
        .filter_map(|entry| match entry {
            StackEntry::Group { group } => {
                if let NodeKind::Group { behavior, .. } = &group.kind {
                    if behavior.is_lookaround() {
                        return Some(group.clone());
                    }
                }
                None
            }
            _ => None,
        })
        .collect()
}

// Re-export the reader trait method so callers can poll without importing.
impl dyn Reader<CharacterReaderValue, ()> {
    /// Polls the reader by one step.
    pub(crate) fn step(&mut self) -> Step<CharacterReaderValue, ()> {
        self.next()
    }
}
