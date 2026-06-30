//! Level 1: fold zero-width markers and turn the end into a return value.
//!
//! Level 1 carries null and start markers on `preceding_zero_width_entries` and
//! attaches them to the next character or reference entry. The pattern end stops
//! the reader and becomes its return value.

use crate::ast::RcNode;
use crate::character_groups::CharacterGroups;
use crate::character_reader::level0::{
    build_character_reader, CharacterReader, CharacterReaderValue, Stack,
};
use crate::reader::{BoxReader, Reader, Step};
use std::rc::Rc;

/// A zero-width marker carried to the next entry.
#[derive(Clone, Debug)]
pub(crate) struct ZeroWidthEntry {
    /// The source offset.
    pub(crate) offset: usize,
    /// The stack at the marker.
    pub(crate) stack: Stack,
    /// Whether this is a null or start marker.
    pub(crate) kind: ZeroWidthKind,
}

/// The kind of a zero-width marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ZeroWidthKind {
    /// A null marker.
    Null,
    /// A `^` start marker.
    Start,
}

/// A level-1 value.
#[derive(Clone)]
pub(crate) enum Level1Value {
    /// A branch point.
    Split {
        reader: Level1Factory,
        sub_type: crate::character_reader::level0::SplitSubType,
    },
    /// A consumed character with its carried zero-width entries.
    Groups {
        character_groups: CharacterGroups,
        node: RcNode,
        stack: Stack,
        preceding_zero_width_entries: Vec<ZeroWidthEntry>,
    },
    /// A backreference with its carried zero-width entries.
    Reference {
        node: RcNode,
        reference_index: u64,
        stack: Stack,
        preceding_zero_width_entries: Vec<ZeroWidthEntry>,
    },
}

/// A level-1 return value.
#[derive(Clone, Debug)]
pub(crate) struct Level1Return {
    /// Whether the end was a hard `$`.
    pub(crate) bounded: bool,
    /// Zero-width entries seen before the end.
    pub(crate) preceding_zero_width_entries: Vec<ZeroWidthEntry>,
}

/// A level-1 reader.
pub(crate) type Level1Reader = BoxReader<Level1Value, Level1Return>;

/// A factory that builds a level-1 reader for a split branch.
pub(crate) type Level1Factory = Rc<dyn Fn() -> Level1Reader>;

struct Level1ReaderImpl {
    inner: CharacterReader,
    preceding: Vec<ZeroWidthEntry>,
}

impl Reader<Level1Value, Level1Return> for Level1ReaderImpl {
    fn next(&mut self) -> Step<Level1Value, Level1Return> {
        loop {
            match self.inner.next() {
                Step::Done(()) => {
                    return Step::Done(Level1Return {
                        bounded: false,
                        preceding_zero_width_entries: std::mem::take(&mut self.preceding),
                    });
                }
                Step::Value(value) => match value {
                    CharacterReaderValue::Groups {
                        character_groups,
                        node,
                        stack,
                    } => {
                        let preceding = std::mem::take(&mut self.preceding);
                        return Step::Value(Level1Value::Groups {
                            character_groups,
                            node,
                            stack,
                            preceding_zero_width_entries: preceding,
                        });
                    }
                    CharacterReaderValue::Reference {
                        node,
                        reference_index,
                        stack,
                    } => {
                        let preceding = std::mem::take(&mut self.preceding);
                        return Step::Value(Level1Value::Reference {
                            node,
                            reference_index,
                            stack,
                            preceding_zero_width_entries: preceding,
                        });
                    }
                    CharacterReaderValue::End {
                        bounded,
                        offset: _,
                        stack: _,
                    } => {
                        return Step::Done(Level1Return {
                            bounded,
                            preceding_zero_width_entries: std::mem::take(&mut self.preceding),
                        });
                    }
                    CharacterReaderValue::Null { offset, stack } => {
                        self.preceding.push(ZeroWidthEntry {
                            offset,
                            stack,
                            kind: ZeroWidthKind::Null,
                        });
                    }
                    CharacterReaderValue::Start { offset, stack } => {
                        self.preceding.push(ZeroWidthEntry {
                            offset,
                            stack,
                            kind: ZeroWidthKind::Start,
                        });
                    }
                    CharacterReaderValue::Split { reader, sub_type } => {
                        let preceding = self.preceding.clone();
                        let wrapped: Level1Factory =
                            Rc::new(move || start_thread(reader(), preceding.clone()));
                        return Step::Value(Level1Value::Split {
                            reader: wrapped,
                            sub_type,
                        });
                    }
                },
            }
        }
    }
}

fn start_thread(inner: CharacterReader, preceding: Vec<ZeroWidthEntry>) -> Level1Reader {
    Box::new(Level1ReaderImpl { inner, preceding })
}

/// Builds the level-1 reader for `node`.
pub(crate) fn build_character_reader_level1(
    case_insensitive: bool,
    dot_all: bool,
    node: &RcNode,
) -> Level1Reader {
    start_thread(
        build_character_reader(case_insensitive, dot_all, node),
        Vec::new(),
    )
}
