//! Set helpers with the same semantics the analyzer expects.

use std::collections::HashSet;
use std::hash::Hash;

/// Returns `true` when both sets contain exactly the same elements.
pub(crate) fn are_sets_equal<T: Eq + Hash>(a: &HashSet<T>, b: &HashSet<T>) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x))
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
}
