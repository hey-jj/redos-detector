//! Joins readers together with understanding of splits.
//!
//! `join` drives a loop indexed by `i`. For each `i` it asks `get_action`
//! whether to stop, fork, or continue, then reads everything from
//! `get_reader(i)`. A fork yields an empty split that represents stopping here.
//! A split inside the read reader is re-wrapped so the join resumes after the
//! branch finishes. `time_since_emit` counts iterations that produced no real
//! character, which stops zero-width infinite loops.

use crate::character_reader::level0::{
    CharacterReader, CharacterReaderValue, ReaderFactory, SplitSubType,
};
use crate::reader::{empty_reader, Reader, Step};
use std::rc::Rc;

/// What the loop should do at index `i`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JoinAction {
    /// Read `get_reader(i)`.
    Continue,
    /// Yield an empty split, then read `get_reader(i)`.
    Fork,
    /// End the join.
    Stop,
}

type GetAction = Rc<dyn Fn(u64, u64) -> JoinAction>;
type GetReader = Rc<dyn Fn(u64) -> CharacterReader>;

enum Phase {
    /// Need to call `get_action` for the current `i`.
    Start,
    /// Yield an empty split (a fork), then read the reader.
    EmitFork,
    /// Reading the current iteration's reader.
    Reading(CharacterReader),
}

struct JoinReader {
    get_action: GetAction,
    get_reader: GetReader,
    i: u64,
    time_since_emit: u64,
    emitted_something: bool,
    phase: Phase,
}

impl JoinReader {
    fn new(get_action: GetAction, get_reader: GetReader, time_since_emit: u64) -> Self {
        JoinReader {
            get_action,
            get_reader,
            i: 0,
            time_since_emit,
            emitted_something: false,
            phase: Phase::Start,
        }
    }
}

impl Reader<CharacterReaderValue, ()> for JoinReader {
    fn next(&mut self) -> Step<CharacterReaderValue, ()> {
        loop {
            match &mut self.phase {
                Phase::Start => {
                    let action = (self.get_action)(self.i, self.time_since_emit);
                    match action {
                        JoinAction::Stop => return Step::Done(()),
                        JoinAction::Fork => {
                            self.emitted_something = false;
                            self.phase = Phase::EmitFork;
                            return Step::Value(CharacterReaderValue::Split {
                                reader: Rc::new(|| empty_reader::<CharacterReaderValue, ()>()),
                                sub_type: SplitSubType::None,
                            });
                        }
                        JoinAction::Continue => {
                            self.emitted_something = false;
                            let reader = (self.get_reader)(self.i);
                            self.phase = Phase::Reading(reader);
                        }
                    }
                }
                Phase::EmitFork => {
                    let reader = (self.get_reader)(self.i);
                    self.phase = Phase::Reading(reader);
                }
                Phase::Reading(reader) => match reader.next() {
                    Step::Done(()) => {
                        self.time_since_emit = if self.emitted_something {
                            0
                        } else {
                            self.time_since_emit + 1
                        };
                        self.i += 1;
                        self.phase = Phase::Start;
                    }
                    Step::Value(value) => match value {
                        CharacterReaderValue::Split {
                            reader: branch,
                            sub_type,
                        } => {
                            let outer_action = Rc::clone(&self.get_action);
                            let outer_reader = Rc::clone(&self.get_reader);
                            let captured_i = self.i;
                            let captured_time = self.time_since_emit;
                            let wrapped: ReaderFactory = Rc::new(move || {
                                let outer_action = Rc::clone(&outer_action);
                                let outer_reader = Rc::clone(&outer_reader);
                                let branch = Rc::clone(&branch);
                                let inner_action: GetAction =
                                    Rc::new(move |inner_i, inner_time| {
                                        if inner_i == 0 {
                                            JoinAction::Continue
                                        } else {
                                            outer_action(inner_i + captured_i, inner_time)
                                        }
                                    });
                                let branch_for_reader = Rc::clone(&branch);
                                let inner_reader: GetReader = Rc::new(move |inner_i| {
                                    if inner_i == 0 {
                                        branch_for_reader()
                                    } else {
                                        outer_reader(inner_i + captured_i)
                                    }
                                });
                                Box::new(JoinReader::new(inner_action, inner_reader, captured_time))
                                    as CharacterReader
                            });
                            return Step::Value(CharacterReaderValue::Split {
                                reader: wrapped,
                                sub_type,
                            });
                        }
                        other => {
                            if !other.is_zero_width_or_end() {
                                self.emitted_something = true;
                            }
                            return Step::Value(other);
                        }
                    },
                },
            }
        }
    }
}

/// Joins readers with a custom loop driver.
pub(crate) fn join<A, G>(get_action: A, get_reader: G) -> CharacterReader
where
    A: Fn(u64, u64) -> JoinAction + 'static,
    G: Fn(u64) -> CharacterReader + 'static,
{
    Box::new(JoinReader::new(Rc::new(get_action), Rc::new(get_reader), 0))
}

/// Joins a fixed array of reader factories back to back.
pub(crate) fn join_array(input: Vec<Box<dyn Fn() -> CharacterReader>>) -> CharacterReader {
    let length = input.len() as u64;
    let input = Rc::new(input);
    let input_for_reader = Rc::clone(&input);
    join(
        move |i, _| {
            if i < length {
                JoinAction::Continue
            } else {
                JoinAction::Stop
            }
        },
        move |i| input_for_reader[i as usize](),
    )
}
