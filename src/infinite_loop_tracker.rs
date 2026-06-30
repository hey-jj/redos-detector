//! Detects a repeating history of checker entries.
//!
//! The checker appends a hash pair for each trail entry. When the most recent
//! entries repeat an earlier block of the same length, the pattern is looping.

/// A left/right pair of hashes plus the trail entry it came from.
///
/// Equality compares only the hash pair. The `trail_entry_id` rides along so the
/// checker can find the trail entry at the start of a detected loop.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    /// The left side hash.
    pub(crate) left: String,
    /// The right side hash.
    pub(crate) right: String,
    /// The id of the trail entry this came from.
    pub(crate) trail_entry_id: usize,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.left == other.left && self.right == other.right
    }
}

impl Eq for Entry {}

/// Tracks the history of checker entries to find a repeating block.
#[derive(Clone, Default)]
pub(crate) struct InfiniteLoopTracker {
    history: Vec<Entry>,
}

impl InfiniteLoopTracker {
    /// Builds an empty tracker.
    pub(crate) fn new() -> Self {
        InfiniteLoopTracker {
            history: Vec::new(),
        }
    }

    /// Appends an entry to the history.
    pub(crate) fn append(&mut self, entry: Entry) {
        self.history.push(entry);
    }

    /// Returns the repeating block at the end of the history, if any.
    pub(crate) fn get_repeating_entries(&self) -> Option<Vec<Entry>> {
        let length = self.history.len();
        let mut candidate_size = 1;
        while candidate_size <= length / 2 {
            let candidate_start = length - candidate_size * 2;
            let mut matched = true;
            for i in 0..candidate_size {
                if self.history[candidate_start + i]
                    != self.history[candidate_start + candidate_size + i]
                {
                    matched = false;
                    break;
                }
            }
            if matched {
                return Some(
                    self.history[candidate_start..candidate_start + candidate_size].to_vec(),
                );
            }
            candidate_size += 1;
        }
        None
    }
}
