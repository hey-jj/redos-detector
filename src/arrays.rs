//! Small slice helpers used across the analyzer.

/// Returns the last element, or `None` when the slice is empty.
pub(crate) fn last<T: Copy>(input: &[T]) -> Option<T> {
    input.last().copied()
}

/// Returns `true` when both slices have the same length and equal elements.
pub(crate) fn are_arrays_equal<T: PartialEq>(a: &[T], b: &[T]) -> bool {
    a == b
}

/// Strips the common prefix of two slices and returns the remaining tails.
///
/// The comparison walks both slices in lockstep and stops at the first index
/// where the elements differ or either slice ends.
pub(crate) fn drop_common<'a, T: PartialEq>(a: &'a [T], b: &'a [T]) -> (&'a [T], &'a [T]) {
    let mut common = 0;
    while common < a.len() && common < b.len() && a[common] == b[common] {
        common += 1;
    }
    (&a[common..], &b[common..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_works() {
        assert_eq!(last::<i32>(&[]), None);
        assert_eq!(last(&[0]), Some(0));
        assert_eq!(last(&[0, 1]), Some(1));
    }

    #[test]
    fn are_arrays_equal_works() {
        assert!(are_arrays_equal::<i32>(&[], &[]));
        assert!(are_arrays_equal(&[1], &[1]));
        assert!(!are_arrays_equal(&[1], &[]));
        assert!(!are_arrays_equal(&[1], &[1, 2]));
        assert!(are_arrays_equal(&[1, 2], &[1, 2]));
    }

    #[test]
    fn drop_common_works() {
        let empty: [i32; 0] = [];
        assert_eq!(drop_common::<i32>(&empty, &empty), (&empty[..], &empty[..]));
        assert_eq!(drop_common(&[1], &[2]), (&[1][..], &[2][..]));
        assert_eq!(drop_common(&[1], &[1]), (&empty[..], &empty[..]));
        assert_eq!(drop_common(&[1, 2], &[1, 2]), (&empty[..], &empty[..]));
        assert_eq!(
            drop_common(&[0, 1, 2], &[1, 2]),
            (&[0, 1, 2][..], &[1, 2][..])
        );
        assert_eq!(drop_common(&[1, 2], &[1, 2, 3]), (&empty[..], &[3][..]));
        assert_eq!(drop_common(&[1, 2, 3], &[1, 2]), (&[3][..], &empty[..]));
        assert_eq!(drop_common(&[1, 2], &[1, 3, 4]), (&[2][..], &[3, 4][..]));
    }
}
