//! Set helpers with the same semantics the analyzer expects.

use std::collections::HashSet;
use std::hash::Hash;

/// Returns the union of two sets.
pub(crate) fn merge_sets<T: Eq + Hash + Clone>(a: &HashSet<T>, b: &HashSet<T>) -> HashSet<T> {
    a.union(b).cloned().collect()
}

/// Returns `true` when both sets contain exactly the same elements.
pub(crate) fn are_sets_equal<T: Eq + Hash + Clone>(a: &HashSet<T>, b: &HashSet<T>) -> bool {
    a.len() == b.len() && merge_sets(a, b).len() == a.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(values: &[i32]) -> HashSet<i32> {
        values.iter().copied().collect()
    }

    #[test]
    fn are_sets_equal_works() {
        assert!(are_sets_equal(&s(&[]), &s(&[])));
        assert!(!are_sets_equal(&s(&[1]), &s(&[])));
        assert!(!are_sets_equal(&s(&[]), &s(&[1])));
        assert!(are_sets_equal(&s(&[1]), &s(&[1])));
        assert!(!are_sets_equal(&s(&[1, 2]), &s(&[1, 3])));
        assert!(are_sets_equal(&s(&[2, 1]), &s(&[1, 2])));
    }

    #[test]
    fn merge_sets_works() {
        assert_eq!(merge_sets(&s(&[]), &s(&[])), s(&[]));
        assert_eq!(merge_sets(&s(&[1]), &s(&[2])), s(&[1, 2]));
        assert_eq!(merge_sets(&s(&[1, 2]), &s(&[2])), s(&[1, 2]));
        assert_eq!(merge_sets(&s(&[1, 2]), &s(&[2, 3])), s(&[1, 2, 3]));
    }
}
