//! Runs two character streams against each other to find ambiguity.
//!
//! A left and a right level-2 reader walk the same input. Every time they can
//! match the same input prefix through different nodes, the checker emits a
//! trail: a pair of distinct ways to match. Two ways to match means a
//! backtracking engine can be forced to backtrack. The checker also detects when
//! the ambiguity is unbounded.

use crate::ast::{NodeKind, RcNode};
use crate::character_groups::{
    intersect_character_groups, is_empty_character_groups, CharacterGroups,
};
use crate::character_reader::level0::{SplitSubType, StackEntry};
use crate::character_reader::level1::{ZeroWidthEntry, ZeroWidthKind};
use crate::character_reader::level2::{Level2Reader, Level2Return, Level2Value};
use crate::infinite_loop_tracker::{Entry, InfiniteLoopTracker};
use crate::is_unbounded_reader::IsUnboundedReader;
use crate::node_extra::NodeExtra;
use crate::quantifier::build_quantifiers_in_infinite_portion;
use crate::reader::{build_forkable_reader, ForkableReader, Reader, Step};
use crate::sets::are_sets_equal;
use std::collections::HashSet;
use std::rc::Rc;

/// One side of a trail entry.
#[derive(Clone)]
pub(crate) struct TrailEntrySide {
    /// Atomic groups both sides entered together and have not yet left.
    pub(crate) atomic_groups_entered_together: HashSet<String>,
    /// The matched character set.
    pub(crate) character_groups: CharacterGroups,
    /// The node identity and stack context, as a string.
    pub(crate) hash: String,
    /// The node that matched.
    pub(crate) node: RcNode,
    /// The full stack including reference frames.
    pub(crate) stack: Vec<StackEntry>,
}

/// A pair of sides that matched the same input position.
#[derive(Clone)]
pub(crate) struct TrailEntry {
    /// Unique identity.
    pub(crate) id: usize,
    /// The intersection of the two sides' character sets.
    pub(crate) intersection: CharacterGroups,
    /// The left side.
    pub(crate) left: TrailEntrySide,
    /// The right side.
    pub(crate) right: TrailEntrySide,
    /// Whether the two sides have diverged by this point.
    pub(crate) diverged: bool,
}

/// A sequence of matched positions.
pub(crate) type Trail = Vec<TrailEntry>;

/// The checker's return value.
#[derive(Clone, Debug)]
pub(crate) enum CheckerReturn {
    /// The checker finished without hitting a limit.
    Ok { infinite: bool },
    /// A step or time limit was hit.
    Error(crate::CheckError),
}

/// Inputs for the checker.
pub(crate) struct CheckerInput {
    /// Offsets of groups to treat as atomic.
    pub(crate) atomic_group_offsets: HashSet<usize>,
    /// The left stream.
    pub(crate) left_stream_reader: Level2Reader,
    /// The maximum number of steps.
    pub(crate) max_steps: f64,
    /// Whether multi-line mode is on.
    pub(crate) multi_line: bool,
    /// The right stream.
    pub(crate) right_stream_reader: Level2Reader,
    /// The timeout in milliseconds.
    pub(crate) timeout: f64,
    /// Pattern facts.
    pub(crate) node_extra: Rc<NodeExtra>,
}

struct StreamReader {
    reader: ForkableReader<Level2Value, Level2Return>,
    pulled: Option<Step<Level2Value, Level2Return>>,
}

impl StreamReader {
    fn fresh(reader: ForkableReader<Level2Value, Level2Return>) -> Self {
        StreamReader {
            reader,
            pulled: None,
        }
    }

    /// Returns the memoized current step, pulling once if needed.
    fn get(&mut self) -> &Step<Level2Value, Level2Return> {
        if self.pulled.is_none() {
            self.pulled = Some(self.reader.next());
        }
        self.pulled.as_ref().unwrap()
    }
}

struct StackFrame {
    infinite_loop_tracker: InfiniteLoopTracker,
    stream_readers: Vec<StreamReader>,
    trail: Trail,
}

/// A clock the checker reads once per outer-loop iteration.
pub(crate) trait Clock {
    /// Returns the current time in milliseconds.
    fn now(&self) -> f64;
}

fn build_context_trail(stack: &[StackEntry], asterisk_infinite: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    for entry in stack {
        match entry {
            StackEntry::Quantifier {
                iteration,
                quantifier,
            } => {
                let suffix = match &quantifier.kind {
                    NodeKind::Quantifier { min, max, .. }
                        if asterisk_infinite && max.is_none() && *iteration >= *min =>
                    {
                        "*".to_string()
                    }
                    _ => iteration.to_string(),
                };
                parts.push(format!("q:{}:{}", quantifier.range.0, suffix));
            }
            StackEntry::Reference { reference } => {
                parts.push(format!("r:{}", reference.range.0));
            }
            StackEntry::Group { group } => {
                parts.push(format!("g:{}", group.range.0));
            }
        }
    }
    parts.join(",")
}

fn atomic_group_hashes_from_stack(
    stack: &[StackEntry],
    atomic_group_offsets: &HashSet<usize>,
) -> HashSet<String> {
    let mut res = HashSet::new();
    for (i, entry) in stack.iter().enumerate() {
        if let StackEntry::Group { group } = entry {
            if atomic_group_offsets.contains(&group.range.0) {
                res.insert(build_context_trail(&stack[..i + 1], false));
            }
        }
    }
    res
}

fn build_zero_width_hash(zw: &ZeroWidthEntry) -> String {
    let kind = match zw.kind {
        ZeroWidthKind::Null => "null",
        ZeroWidthKind::Start => "start",
    };
    format!(
        "{}:{}:{}",
        kind,
        zw.offset,
        build_context_trail(&zw.stack, false)
    )
}

struct StackSeqItem {
    hash: String,
    stack: Vec<StackEntry>,
}

fn get_stack_sequence(entry: &Level2EntrySnapshot, hash: &str) -> Vec<StackSeqItem> {
    let mut seq: Vec<StackSeqItem> = entry
        .preceding_zero_width_entries
        .iter()
        .map(|z| StackSeqItem {
            hash: format!("zw:{}", build_zero_width_hash(z)),
            stack: z.stack.clone(),
        })
        .collect();
    seq.push(StackSeqItem {
        hash: format!("s:{}", hash),
        stack: entry.stack.clone(),
    });
    seq
}

fn are_sides_equal(a: &TrailEntrySide, b: &TrailEntrySide) -> bool {
    a.hash == b.hash
}

/// A snapshot of a level-2 entry the checker compares.
#[derive(Clone)]
struct Level2EntrySnapshot {
    character_groups: CharacterGroups,
    lookahead_stack: Vec<RcNode>,
    node: RcNode,
    preceding_zero_width_entries: Vec<ZeroWidthEntry>,
    stack: Vec<StackEntry>,
}

/// Settings the checker reads while running, separate from the readers.
struct CheckerSettings {
    atomic_group_offsets: HashSet<usize>,
    max_steps: f64,
    multi_line: bool,
    node_extra: Rc<NodeExtra>,
}

/// The checker reader.
pub(crate) struct CheckerReader<C: Clock> {
    settings: CheckerSettings,
    clock: C,
    stack: Vec<StackFrame>,
    trails: Vec<Trail>,
    trail_entries_at_start_of_loop: HashSet<usize>,
    step_count: f64,
    latest_end_time: f64,
    timed_out: bool,
    next_trail_entry_id: usize,
    finished: Option<CheckerReturn>,
}

impl<C: Clock> CheckerReader<C> {
    /// Builds the checker over the two streams.
    pub(crate) fn new(input: CheckerInput, clock: C) -> Self {
        let left = build_forkable_reader(input.left_stream_reader);
        let right = build_forkable_reader(input.right_stream_reader);
        let frame = StackFrame {
            infinite_loop_tracker: InfiniteLoopTracker::new(),
            stream_readers: vec![StreamReader::fresh(left), StreamReader::fresh(right)],
            trail: Vec::new(),
        };
        let latest_end_time = clock.now() + input.timeout;
        let settings = CheckerSettings {
            atomic_group_offsets: input.atomic_group_offsets,
            max_steps: input.max_steps,
            multi_line: input.multi_line,
            node_extra: input.node_extra,
        };
        CheckerReader {
            settings,
            clock,
            stack: vec![frame],
            trails: Vec::new(),
            trail_entries_at_start_of_loop: HashSet::new(),
            step_count: 0.0,
            latest_end_time,
            timed_out: false,
            next_trail_entry_id: 0,
            finished: None,
        }
    }

    fn finish(&mut self) -> CheckerReturn {
        let mut infinite = false;
        if !self.trail_entries_at_start_of_loop.is_empty() {
            for trail in &self.trails {
                if trail
                    .iter()
                    .any(|entry| self.trail_entries_at_start_of_loop.contains(&entry.id))
                {
                    infinite = true;
                    break;
                }
            }
        }
        if self.step_count > self.settings.max_steps {
            CheckerReturn::Error(crate::CheckError::HitMaxSteps)
        } else if self.timed_out {
            CheckerReturn::Error(crate::CheckError::TimedOut)
        } else {
            CheckerReturn::Ok { infinite }
        }
    }
}

impl<C: Clock> Reader<Trail, CheckerReturn> for CheckerReader<C> {
    fn next(&mut self) -> Step<Trail, CheckerReturn> {
        if let Some(result) = &self.finished {
            return Step::Done(result.clone());
        }
        match self.run() {
            Some(trail) => Step::Value(trail),
            None => {
                let result = self.finish();
                self.finished = Some(result.clone());
                Step::Done(result)
            }
        }
    }
}

impl<C: Clock> CheckerReader<C> {
    /// Drives the DFS until a trail is emitted or the work is done.
    fn run(&mut self) -> Option<Trail> {
        loop {
            self.timed_out = self.clock.now() > self.latest_end_time;
            if self.timed_out || self.step_count > self.settings.max_steps {
                return None;
            }

            let frame = match self.stack.pop() {
                Some(frame) => frame,
                None => return None,
            };

            let StackFrame {
                infinite_loop_tracker,
                mut stream_readers,
                mut trail,
            } = frame;

            // Pull both readers, handling splits by branching the DFS.
            let mut next_values: Vec<Step<Level2Value, Level2Return>> = Vec::new();
            let mut split_index: Option<usize> = None;
            for i in 0..stream_readers.len() {
                self.step_count += 0.5;
                let is_split = matches!(
                    stream_readers[i].get(),
                    Step::Value(Level2Value::Split { .. })
                );
                if is_split {
                    split_index = Some(i);
                    break;
                }
                let value = stream_readers[i].get().clone_step();
                next_values.push(value);
            }
            if let Some(i) = split_index {
                self.handle_split(i, &infinite_loop_tracker, stream_readers, &trail);
                continue;
            }

            if self.step_count > self.settings.max_steps
                || matches!(next_values[0], Step::Done(_))
                || matches!(next_values[1], Step::Done(_))
            {
                continue;
            }

            let left_value = snapshot(&next_values[0]);
            let right_value = snapshot(&next_values[1]);

            let left_start = left_value
                .preceding_zero_width_entries
                .iter()
                .any(|z| z.kind == ZeroWidthKind::Start);
            let right_start = right_value
                .preceding_zero_width_entries
                .iter()
                .any(|z| z.kind == ZeroWidthKind::Start);
            if !trail.is_empty() && (left_start || right_start) {
                continue;
            }

            let left_lookahead = left_value.lookahead_stack.last().map(|n| n.id);
            let right_lookahead = right_value.lookahead_stack.last().map(|n| n.id);
            if left_lookahead != right_lookahead {
                continue;
            }

            let intersection = intersect_character_groups(
                &left_value.character_groups,
                &right_value.character_groups,
            );
            if is_empty_character_groups(&intersection) {
                continue;
            }

            let left_context_trail = build_context_trail(&left_value.stack, false);
            let right_context_trail = build_context_trail(&right_value.stack, false);
            let new_left_hash = format!("{}:{}", left_value.node.range.0, left_context_trail);
            let new_right_hash = format!("{}:{}", right_value.node.range.0, right_context_trail);

            let last_trail_entry = trail.last();
            let mut atomic_left: HashSet<String> = last_trail_entry
                .map(|e| e.left.atomic_groups_entered_together.clone())
                .unwrap_or_default();
            let mut atomic_right: HashSet<String> = last_trail_entry
                .map(|e| e.right.atomic_groups_entered_together.clone())
                .unwrap_or_default();
            let mut diverged = last_trail_entry.map(|e| e.diverged).unwrap_or(false);

            if !diverged {
                let left_seq = get_stack_sequence(&left_value, &new_left_hash);
                let right_seq = get_stack_sequence(&right_value, &new_right_hash);
                let limit = left_seq.len().min(right_seq.len());
                for i in 0..limit {
                    if left_seq[i].hash != right_seq[i].hash {
                        diverged = true;
                        break;
                    }
                    let now = atomic_group_hashes_from_stack(
                        &left_seq[i].stack,
                        &self.settings.atomic_group_offsets,
                    );
                    for group in now {
                        atomic_left.insert(group.clone());
                        atomic_right.insert(group);
                    }
                }
            }

            let now_left = atomic_group_hashes_from_stack(
                &left_value.stack,
                &self.settings.atomic_group_offsets,
            );
            atomic_left.retain(|group| now_left.contains(group));
            let now_right = atomic_group_hashes_from_stack(
                &right_value.stack,
                &self.settings.atomic_group_offsets,
            );
            atomic_right.retain(|group| now_right.contains(group));

            let new_entry_left = TrailEntrySide {
                atomic_groups_entered_together: atomic_left,
                character_groups: left_value.character_groups.clone(),
                hash: new_left_hash.clone(),
                node: left_value.node.clone(),
                stack: left_value.stack.clone(),
            };
            let new_entry_right = TrailEntrySide {
                atomic_groups_entered_together: atomic_right,
                character_groups: right_value.character_groups.clone(),
                hash: new_right_hash.clone(),
                node: right_value.node.clone(),
                stack: right_value.stack.clone(),
            };

            let entry_id = self.next_trail_entry_id;
            self.next_trail_entry_id += 1;
            let new_entry = TrailEntry {
                id: entry_id,
                diverged,
                intersection: intersection.clone(),
                left: new_entry_left,
                right: new_entry_right,
            };
            trail.push(new_entry.clone());

            let left_quantifiers_in_infinite =
                build_quantifiers_in_infinite_portion(&left_value.stack, &self.settings.node_extra);
            if !left_quantifiers_in_infinite.is_empty() {
                let left_and_right_identical =
                    trail.iter().all(|e| are_sides_equal(&e.left, &e.right));
                if left_and_right_identical {
                    continue;
                }
            }

            let mut tracker = infinite_loop_tracker;
            let tracker_entry = Entry {
                left: format!(
                    "{}:{}",
                    left_value.node.range.0,
                    build_context_trail(&left_value.stack, true)
                ),
                right: format!(
                    "{}:{}",
                    right_value.node.range.0,
                    build_context_trail(&left_value.stack, true)
                ),
                trail_entry_id: entry_id,
            };
            tracker.append(tracker_entry);

            if let Some(repeating) = tracker.get_repeating_entries() {
                let start_id = repeating[0].trail_entry_id;
                self.trail_entries_at_start_of_loop.insert(start_id);
                continue;
            }

            if !are_sets_equal(
                &new_entry.left.atomic_groups_entered_together,
                &new_entry.right.atomic_groups_entered_together,
            ) {
                continue;
            }

            let sides_equal = are_sides_equal(&new_entry.left, &new_entry.right);
            let mut emitted: Option<Trail> = None;
            if !sides_equal {
                let left_unbounded = self.run_unbounded_check(&stream_readers[0].reader, true);
                if left_unbounded == UnboundedResult::Unbounded {
                    self.push_continue(tracker, stream_readers, trail);
                    continue;
                }
                if left_unbounded == UnboundedResult::HitMax {
                    return None;
                }
                let right_unbounded = self.run_unbounded_check(&stream_readers[1].reader, false);
                if right_unbounded == UnboundedResult::Unbounded {
                    self.push_continue(tracker, stream_readers, trail);
                    continue;
                }
                if right_unbounded == UnboundedResult::HitMax {
                    return None;
                }

                if self.should_emit_trail(&trail) {
                    self.trails.push(trail.clone());
                    emitted = Some(trail.clone());
                }
            }

            self.push_continue(tracker, stream_readers, trail);

            if let Some(trail) = emitted {
                return Some(trail);
            }
        }
    }

    fn push_continue(
        &mut self,
        tracker: InfiniteLoopTracker,
        mut stream_readers: Vec<StreamReader>,
        trail: Trail,
    ) {
        for sr in &mut stream_readers {
            sr.pulled = None;
        }
        self.stack.push(StackFrame {
            infinite_loop_tracker: tracker,
            stream_readers,
            trail,
        });
    }

    fn handle_split(
        &mut self,
        i: usize,
        tracker: &InfiniteLoopTracker,
        mut stream_readers: Vec<StreamReader>,
        trail: &Trail,
    ) {
        let split_reader = match stream_readers[i].get() {
            Step::Value(Level2Value::Split { reader, .. }) => reader.clone(),
            _ => unreachable!("handle_split called without a split"),
        };

        // Split path: reader i becomes a fresh forkable over the branch; the
        // others fork from their current position. Getters at j < i keep their
        // memoized pull, j >= i reset.
        let mut split_path: Vec<StreamReader> = Vec::with_capacity(stream_readers.len());
        for (j, sr) in stream_readers.iter().enumerate() {
            if j == i {
                split_path.push(StreamReader::fresh(build_forkable_reader(split_reader())));
            } else if j < i {
                split_path.push(StreamReader {
                    reader: sr.reader.fork(),
                    pulled: sr.pulled.clone_opt(),
                });
            } else {
                split_path.push(StreamReader::fresh(sr.reader.fork()));
            }
        }

        // Non-split path: keep the same readers, reset only getter i so it pulls
        // the value after the split.
        stream_readers[i].pulled = None;
        self.stack.push(StackFrame {
            infinite_loop_tracker: tracker.clone(),
            stream_readers,
            trail: trail.clone(),
        });
        self.stack.push(StackFrame {
            infinite_loop_tracker: tracker.clone(),
            stream_readers: split_path,
            trail: trail.clone(),
        });
    }

    fn run_unbounded_check(
        &mut self,
        reader: &ForkableReader<Level2Value, Level2Return>,
        is_left: bool,
    ) -> UnboundedResult {
        let mut unbounded_reader = IsUnboundedReader::new(self.settings.multi_line, reader);
        loop {
            match unbounded_reader.next() {
                Step::Value(_) => {
                    self.step_count += 0.5;
                    if self.step_count > self.settings.max_steps {
                        // The left check breaks the outer loop; the right one is
                        // marked unreachable upstream but treated the same.
                        let _ = is_left;
                        return UnboundedResult::HitMax;
                    }
                }
                Step::Done(result) => {
                    return if result {
                        UnboundedResult::Unbounded
                    } else {
                        UnboundedResult::Bounded
                    };
                }
            }
        }
    }

    fn should_emit_trail(&self, trail: &Trail) -> bool {
        let already_exists = self.trails.iter().any(|existing| {
            if existing.len() != trail.len() {
                return false;
            }
            let forward = existing.iter().zip(trail.iter()).all(|(e, t)| {
                are_sides_equal(&e.left, &t.left) && are_sides_equal(&e.right, &t.right)
            });
            let swapped = existing.iter().zip(trail.iter()).all(|(e, t)| {
                are_sides_equal(&e.left, &t.right) && are_sides_equal(&e.right, &t.left)
            });
            forward || swapped
        });
        !already_exists
    }
}

#[derive(PartialEq, Eq)]
enum UnboundedResult {
    Unbounded,
    Bounded,
    HitMax,
}

fn snapshot(step: &Step<Level2Value, Level2Return>) -> Level2EntrySnapshot {
    match step {
        Step::Value(Level2Value::Entry(entry)) => Level2EntrySnapshot {
            character_groups: entry.character_groups.clone(),
            lookahead_stack: entry.lookahead_stack.clone(),
            node: entry.node.clone(),
            preceding_zero_width_entries: entry.preceding_zero_width_entries.clone(),
            stack: entry.stack.clone(),
        },
        _ => panic!("Internal error: impossible leftValue/rightValue type"),
    }
}

trait CloneStep {
    fn clone_step(&self) -> Step<Level2Value, Level2Return>;
}

impl CloneStep for Step<Level2Value, Level2Return> {
    fn clone_step(&self) -> Step<Level2Value, Level2Return> {
        match self {
            Step::Value(v) => Step::Value(v.clone()),
            Step::Done(r) => Step::Done(r.clone()),
        }
    }
}

trait CloneOpt {
    fn clone_opt(&self) -> Option<Step<Level2Value, Level2Return>>;
}

impl CloneOpt for Option<Step<Level2Value, Level2Return>> {
    fn clone_opt(&self) -> Option<Step<Level2Value, Level2Return>> {
        self.as_ref().map(|s| s.clone_step())
    }
}
