//! Drives the checker and turns trails into a score.
//!
//! Each emitted trail describes an input prefix. The collector groups trails of
//! the same length and counts, for each trail, how many distinct full-length
//! side-sequences match its input schema. The score is the largest such count.
//! Above a work budget it falls back to incrementing by one per new trail.

use crate::ast::RcNode;
use crate::character_groups::{
    intersect_character_groups, is_empty_character_groups, CharacterGroups,
};
use crate::character_reader::level2::build_character_reader_level2;
use crate::checker_reader::{
    CheckerInput, CheckerReader, CheckerReturn, Clock, Trail, TrailEntrySide,
};
use crate::node_extra::build_node_extra;
use crate::reader::{Reader, Step};
use crate::result_cache::ResultCache;
use crate::tree::Tree;
use crate::{CheckError, RedosDetectorError, Score};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

const WORK_LIMIT: u64 = 25_000;

/// A key for the empty-intersection cache, derived from a character set.
type GroupKey = (bool, Vec<(u64, u64)>, Vec<(String, bool)>);

fn group_key(group: &CharacterGroups) -> GroupKey {
    let ranges = group
        .ranges
        .iter()
        .map(|(a, b)| (a.to_bits(), b.to_bits()))
        .collect();
    (group.ranges_negated, ranges, group.escapes_vec())
}

type EmptyCache = Rc<RefCell<ResultCache<bool, GroupKey>>>;

struct EnhancedTrail {
    length: usize,
    trail: Trail,
    input_string_schema: Vec<CharacterGroups>,
    tree: Tree<Vec<TrailEntrySide>, String>,
    empty_cache: EmptyCache,
}

impl EnhancedTrail {
    fn new(trail: Trail, empty_cache: EmptyCache) -> Rc<RefCell<Self>> {
        let input_string_schema: Vec<CharacterGroups> =
            trail.iter().map(|e| e.intersection.clone()).collect();
        let length = trail.len();
        let enhanced = Rc::new(RefCell::new(EnhancedTrail {
            length,
            trail,
            input_string_schema,
            tree: Tree::new(|sides: &Vec<TrailEntrySide>| {
                sides.iter().map(|s| s.hash.clone()).collect()
            }),
            empty_cache,
        }));
        let own_trail = enhanced.borrow().trail.clone();
        enhanced.borrow_mut().on_new_trail(&own_trail);
        enhanced
    }

    fn matches_len(&self) -> usize {
        self.tree.items().len()
    }

    fn get_longest_match(
        &self,
        schema: &[CharacterGroups],
        sides: &[TrailEntrySide],
    ) -> Vec<TrailEntrySide> {
        let mut no_match = None;
        for (i, side) in sides.iter().enumerate() {
            let key_a = group_key(&side.character_groups);
            let key_b = group_key(&schema[i]);
            let is_empty = {
                let cached = self.empty_cache.borrow().get_result(&key_a, &key_b);
                match cached {
                    Some(value) => value,
                    None => {
                        let value = is_empty_character_groups(&intersect_character_groups(
                            &side.character_groups,
                            &schema[i],
                        ));
                        self.empty_cache
                            .borrow_mut()
                            .add_result(key_a, key_b, value);
                        value
                    }
                }
            };
            if is_empty {
                no_match = Some(i);
                break;
            }
        }
        match no_match {
            Some(i) => sides[..i].to_vec(),
            None => sides.to_vec(),
        }
    }

    fn on_new_trail(&mut self, other_trail: &Trail) {
        let mut left_side: Vec<TrailEntrySide> = Vec::new();
        let mut right_side: Vec<TrailEntrySide> = Vec::new();
        for entry in other_trail {
            left_side.push(entry.left.clone());
            right_side.push(entry.right.clone());
        }

        let left_match = self.get_longest_match(&self.input_string_schema, &left_side);
        if left_match.len() == self.length {
            self.tree.add(left_match);
        }
        let right_match = self.get_longest_match(&self.input_string_schema, &right_side);
        if right_match.len() == self.length {
            self.tree.add(right_match);
        }
    }
}

/// The result of running the collector.
pub(crate) struct CollectResults {
    /// The error, if any.
    pub(crate) error: Option<RedosDetectorError>,
    /// The discovered trails.
    pub(crate) trails: Vec<Trail>,
    /// The score, where `None` means infinite.
    pub(crate) score: Score,
}

/// Inputs for the collector.
pub(crate) struct CollectInput {
    /// Offsets of atomic groups.
    pub(crate) atomic_group_offsets: HashSet<usize>,
    /// Case-insensitive mode.
    pub(crate) case_insensitive: bool,
    /// Dot-all mode.
    pub(crate) dot_all: bool,
    /// The score cap.
    pub(crate) max_score: f64,
    /// The step cap.
    pub(crate) max_steps: f64,
    /// Multi-line mode.
    pub(crate) multi_line: bool,
    /// The parsed pattern.
    pub(crate) node: RcNode,
    /// The timeout in milliseconds.
    pub(crate) timeout: f64,
}

/// Runs the checker and computes the score.
pub(crate) fn collect_results<C: Clock>(input: CollectInput, clock: C) -> CollectResults {
    let empty_cache: EmptyCache = Rc::new(RefCell::new(ResultCache::new()));
    let node_extra = Rc::new(build_node_extra(&input.node));

    let left = build_character_reader_level2(
        input.case_insensitive,
        input.dot_all,
        &input.node,
        Rc::clone(&node_extra),
    );
    let right = build_character_reader_level2(
        input.case_insensitive,
        input.dot_all,
        &input.node,
        Rc::clone(&node_extra),
    );

    let checker_input = CheckerInput {
        atomic_group_offsets: input.atomic_group_offsets,
        left_stream_reader: left,
        max_steps: input.max_steps,
        multi_line: input.multi_line,
        right_stream_reader: right,
        timeout: input.timeout,
        node_extra: Rc::clone(&node_extra),
    };
    let mut reader = CheckerReader::new(checker_input, clock);

    let mut trails_tree: Tree<Rc<RefCell<EnhancedTrail>>, usize> =
        Tree::new(|t: &Rc<RefCell<EnhancedTrail>>| t.borrow().trail.iter().map(|e| e.id).collect());
    let mut score: f64 = 1.0;
    let mut work: u64 = 0;
    let mut hit_max_score = false;
    let final_return;

    loop {
        match reader.next() {
            Step::Value(value) => {
                let trail = EnhancedTrail::new(value, Rc::clone(&empty_cache));

                if work < WORK_LIMIT {
                    let trail_len = trail.borrow().length;
                    for existing in trails_tree.iter_items() {
                        if existing.borrow().length == trail_len {
                            work += trail_len as u64;
                            if work >= WORK_LIMIT {
                                break;
                            }
                            let existing_trail = existing.borrow().trail.clone();
                            let new_trail = trail.borrow().trail.clone();
                            trail.borrow_mut().on_new_trail(&existing_trail);
                            existing.borrow_mut().on_new_trail(&new_trail);
                            let m = existing.borrow().matches_len() as f64;
                            if m > score {
                                score = m;
                            }
                        }
                    }
                    let m = trail.borrow().matches_len() as f64;
                    if m > score {
                        score = m;
                    }
                }

                if work >= WORK_LIMIT {
                    score += 1.0;
                }

                trails_tree.add(trail);

                if score > input.max_score {
                    hit_max_score = true;
                    final_return = None;
                    break;
                }
            }
            Step::Done(ret) => {
                final_return = Some(ret);
                break;
            }
        }
    }

    let mut error: Option<RedosDetectorError> = None;
    let mut final_score = Score::Finite(score);
    if let Some(ret) = final_return {
        match ret {
            CheckerReturn::Error(CheckError::HitMaxSteps) => {
                final_score = Score::Infinite;
                error = Some(RedosDetectorError::HitMaxSteps);
            }
            CheckerReturn::Error(CheckError::TimedOut) => {
                final_score = Score::Infinite;
                error = Some(RedosDetectorError::TimedOut);
            }
            CheckerReturn::Ok { infinite } => {
                if infinite {
                    final_score = Score::Infinite;
                    error = Some(RedosDetectorError::HitMaxScore);
                }
            }
        }
    } else if hit_max_score {
        final_score = Score::Infinite;
        error = Some(RedosDetectorError::HitMaxScore);
    }

    let trails: Vec<Trail> = trails_tree
        .items()
        .into_iter()
        .map(|t| t.borrow().trail.clone())
        .collect();

    CollectResults {
        error,
        trails,
        score: final_score,
    }
}
