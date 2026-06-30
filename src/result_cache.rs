//! A symmetric two-key memo.
//!
//! Looking up `(a, b)` returns the same stored value as `(b, a)`. The analyzer
//! uses it to avoid recomputing whether two character sets have an empty
//! intersection.

use std::collections::HashMap;
use std::hash::Hash;

/// A cache keyed by an unordered pair of keys.
pub(crate) struct ResultCache<V, K: Eq + Hash + Clone> {
    cache: HashMap<K, HashMap<K, V>>,
}

impl<V: Clone, K: Eq + Hash + Clone> Default for ResultCache<V, K> {
    fn default() -> Self {
        ResultCache {
            cache: HashMap::new(),
        }
    }
}

impl<V: Clone, K: Eq + Hash + Clone> ResultCache<V, K> {
    /// Builds an empty cache.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Stores `result` under both orderings of the key pair.
    pub(crate) fn add_result(&mut self, a: K, b: K, result: V) {
        self.cache
            .entry(a.clone())
            .or_default()
            .insert(b.clone(), result.clone());
        self.cache.entry(b).or_default().insert(a, result);
    }

    /// Returns the stored value for the pair, if any.
    pub(crate) fn get_result(&self, a: &K, b: &K) -> Option<V> {
        self.cache.get(a).and_then(|m| m.get(b)).cloned()
    }
}
