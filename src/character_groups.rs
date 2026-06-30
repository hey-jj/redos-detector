//! Character sets and their intersection.
//!
//! A `CharacterGroups` is a set of code points described by inclusive ranges
//! (optionally negated) plus a set of unicode property escapes. The escapes are
//! kept as an insertion-ordered key/value list because the analyzer compares
//! their presence and sign, not a hash ordering.

use crate::our_range::{intersect_ranges, subtract_ranges, OurRange};

/// An ordered set of unicode property escapes mapped to their negated flag.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EscapeMap {
    entries: Vec<(String, bool)>,
}

impl EscapeMap {
    /// Returns an empty escape map.
    pub(crate) fn new() -> Self {
        EscapeMap {
            entries: Vec::new(),
        }
    }

    /// Builds a map with a single entry.
    pub(crate) fn single(key: String, negated: bool) -> Self {
        EscapeMap {
            entries: vec![(key, negated)],
        }
    }

    /// Inserts or overwrites an entry, preserving first-seen order.
    pub(crate) fn set(&mut self, key: String, negated: bool) {
        for entry in &mut self.entries {
            if entry.0 == key {
                entry.1 = negated;
                return;
            }
        }
        self.entries.push((key, negated));
    }

    /// Returns the negated flag for `key`, if present.
    pub(crate) fn get(&self, key: &str) -> Option<bool> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    /// Returns the number of entries.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when there are no entries.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A set of code points.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CharacterGroups {
    /// When `true`, `ranges` describes the excluded code points.
    pub(crate) ranges_negated: bool,
    /// Inclusive code point ranges.
    pub(crate) ranges: Vec<OurRange>,
    /// Unicode property escapes carried alongside the ranges.
    pub(crate) unicode_property_escapes: EscapeMap,
}

/// Returns `true` when the group definitely matches no characters.
///
/// A negated empty range matches everything, so it is not empty. Any property
/// escape keeps the group non-empty.
pub(crate) fn is_empty_character_groups(group: &CharacterGroups) -> bool {
    !group.ranges_negated && group.ranges.is_empty() && group.unicode_property_escapes.is_empty()
}

/// Returns the intersection of two character sets.
pub(crate) fn intersect_character_groups(
    a: &CharacterGroups,
    b: &CharacterGroups,
) -> CharacterGroups {
    let new_negated;
    let mut new_ranges: Vec<OurRange>;

    if !a.ranges_negated {
        if !b.ranges_negated {
            new_negated = false;
            new_ranges = Vec::new();
            for a_range in &a.ranges {
                for b_range in &b.ranges {
                    if let Some(intersection) = intersect_ranges(*a_range, *b_range) {
                        new_ranges.push(intersection);
                    }
                }
            }
        } else {
            new_negated = false;
            new_ranges = a.ranges.clone();
            for b_range in &b.ranges {
                let mut narrowed = Vec::new();
                for a_range in &new_ranges {
                    narrowed.extend(subtract_ranges(*a_range, *b_range));
                }
                new_ranges = narrowed;
            }
        }
    } else if !b.ranges_negated {
        new_negated = false;
        new_ranges = b.ranges.clone();
        for a_range in &a.ranges {
            let mut narrowed = Vec::new();
            for b_range in &new_ranges {
                narrowed.extend(subtract_ranges(*b_range, *a_range));
            }
            new_ranges = narrowed;
        }
    } else {
        new_negated = true;
        new_ranges = a.ranges.clone();
        new_ranges.extend(b.ranges.iter().copied());
    }

    let mut all_keys: Vec<&str> = Vec::new();
    for (key, _) in &a.unicode_property_escapes.entries {
        if !all_keys.contains(&key.as_str()) {
            all_keys.push(key);
        }
    }
    for (key, _) in &b.unicode_property_escapes.entries {
        if !all_keys.contains(&key.as_str()) {
            all_keys.push(key);
        }
    }

    let mut new_escapes = EscapeMap::new();
    for key in all_keys {
        let a_escape = a.unicode_property_escapes.get(key);
        let b_escape = b.unicode_property_escapes.get(key);
        match (a_escape, b_escape) {
            (Some(a_val), None) => new_escapes.set(key.to_string(), a_val),
            (None, Some(b_val)) => new_escapes.set(key.to_string(), b_val),
            (Some(a_val), Some(b_val)) if a_val == b_val => new_escapes.set(key.to_string(), a_val),
            _ => {}
        }
    }

    CharacterGroups {
        ranges: new_ranges,
        ranges_negated: new_negated,
        unicode_property_escapes: new_escapes,
    }
}
