//! Decides whether there could be nothing after the current point.
//!
//! The reader does a DFS over the remaining level-2 stream. It yields one step
//! per frame so the caller can count work, and returns `true` if any branch can
//! reach a non-bounded end. The `b` in `aab` against `^a+` is the classic case:
//! the pattern can stop, so the trailing text falls outside it.

use crate::character_groups::{
    intersect_character_groups, is_empty_character_groups, CharacterGroups, EscapeMap,
};
use crate::character_reader::level0::SplitSubType;
use crate::character_reader::level2::{Level2Return, Level2Value};
use crate::reader::{ForkableReader, Reader, Step};

/// A step marker yielded by [`IsUnboundedReader`].
pub(crate) struct IsUnboundedStep;

/// A reader that yields work steps and returns whether the point is unbounded.
pub(crate) struct IsUnboundedReader {
    multi_line: bool,
    stack: Vec<ForkableReader<Level2Value, Level2Return>>,
    finished: Option<bool>,
}

fn not_new_line() -> CharacterGroups {
    CharacterGroups {
        ranges: vec![(10.0, 10.0), (13.0, 13.0), (8232.0, 8233.0)],
        ranges_negated: true,
        unicode_property_escapes: EscapeMap::new(),
    }
}

impl IsUnboundedReader {
    /// Builds the reader over a fork of `reader`.
    pub(crate) fn new(
        multi_line: bool,
        reader: &ForkableReader<Level2Value, Level2Return>,
    ) -> Self {
        IsUnboundedReader {
            multi_line,
            stack: vec![reader.fork()],
            finished: None,
        }
    }
}

impl Reader<IsUnboundedStep, bool> for IsUnboundedReader {
    fn next(&mut self) -> Step<IsUnboundedStep, bool> {
        if let Some(result) = self.finished {
            return Step::Done(result);
        }

        let mut frame = match self.stack.pop() {
            Some(frame) => frame,
            None => {
                self.finished = Some(false);
                return Step::Done(false);
            }
        };

        match frame.next() {
            Step::Done(ret) => {
                if let Level2Return::End { bounded: false, .. } = ret {
                    self.finished = Some(true);
                    return Step::Value(IsUnboundedStep);
                }
            }
            Step::Value(value) => match value {
                Level2Value::Split { reader, sub_type } => {
                    self.stack.push(frame);
                    if sub_type == SplitSubType::None {
                        self.stack
                            .push(crate::reader::build_forkable_reader(reader()));
                    }
                }
                Level2Value::Entry(entry) => {
                    if self.multi_line {
                        let is_new_line = is_empty_character_groups(&intersect_character_groups(
                            &not_new_line(),
                            &entry.character_groups,
                        ));
                        if is_new_line {
                            self.stack.push(frame);
                        }
                    }
                }
            },
        }

        Step::Value(IsUnboundedStep)
    }
}
