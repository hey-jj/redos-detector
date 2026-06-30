//! Small slice helpers used across the analyzer.

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
