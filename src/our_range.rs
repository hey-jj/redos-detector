//! Inclusive integer ranges with infinite endpoints.
//!
//! Endpoints are stored as `f64` so the negative and positive infinity
//! sentinels survive range inversion. Code points stay well within the exact
//! integer range of `f64`, so arithmetic on real endpoints is lossless.

/// An inclusive range `[start, end]`.
pub(crate) type OurRange = (f64, f64);

/// Returns the parts of `source` that are not covered by `to_subtract`.
///
/// The result has at most two ranges: the part below `to_subtract` and the
/// part above it.
pub(crate) fn subtract_ranges(source: OurRange, to_subtract: OurRange) -> Vec<OurRange> {
    let mut res = Vec::new();
    if source.0 < to_subtract.0 {
        res.push((source.0, (to_subtract.0 - 1.0).min(source.1)));
    }
    if source.1 > to_subtract.1 {
        res.push(((to_subtract.1 + 1.0).max(source.0), source.1));
    }
    res
}

/// Returns the overlap of two ranges, or `None` when they are disjoint.
pub(crate) fn intersect_ranges(a: OurRange, b: OurRange) -> Option<OurRange> {
    let start = a.0.max(b.0);
    let end = a.1.min(b.1);
    if start > end {
        return None;
    }
    Some((start, end))
}

/// Coalesces a set of integers into sorted contiguous ranges.
pub(crate) fn create_ranges(set: &std::collections::HashSet<i64>) -> Vec<OurRange> {
    let mut ascending: Vec<i64> = set.iter().copied().collect();
    ascending.sort_unstable();

    let mut ranges = Vec::new();
    let mut start_index = 0usize;
    for i in 0..ascending.len() {
        let start_value = ascending[start_index];
        let current_value = ascending[i];
        let next_value = if i + 1 < ascending.len() {
            Some(ascending[i + 1])
        } else {
            None
        };
        let advance = i as i64 - start_index as i64 + 1;
        if next_value.is_none() || next_value.unwrap() - start_value != advance {
            ranges.push((start_value as f64, current_value as f64));
            start_index = i + 1;
        }
    }
    ranges
}

/// Returns the complement of `ranges` over `[-inf, +inf]`.
///
/// Input ranges must be sorted and non-overlapping. Panics with the internal
/// error message when a computed range is invalid.
pub(crate) fn invert_ranges(ranges: &[OurRange]) -> Vec<OurRange> {
    let mut result = Vec::new();
    for i in 0..ranges.len() + 1 {
        let prev = if i >= 1 { Some(ranges[i - 1]) } else { None };
        let current = if i < ranges.len() {
            Some(ranges[i])
        } else {
            None
        };

        let start = prev.map(|p| p.1 + 1.0).unwrap_or(f64::NEG_INFINITY);
        let end = current.map(|c| c.0 - 1.0).unwrap_or(f64::INFINITY);
        if start - end == 1.0 {
            continue;
        }
        if start > end {
            panic!("Internal error: invalid ranges input");
        }
        result.push((start, end));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn set(values: &[i64]) -> HashSet<i64> {
        values.iter().copied().collect()
    }

    #[test]
    fn intersect_ranges_works() {
        assert_eq!(intersect_ranges((0.0, 1.0), (2.0, 3.0)), None);
        assert_eq!(intersect_ranges((1.0, 1.0), (1.0, 1.0)), Some((1.0, 1.0)));
        assert_eq!(intersect_ranges((0.0, 1.0), (1.0, 2.0)), Some((1.0, 1.0)));
        assert_eq!(intersect_ranges((1.0, 2.0), (0.0, 3.0)), Some((1.0, 2.0)));
        assert_eq!(intersect_ranges((1.0, 4.0), (3.0, 6.0)), Some((3.0, 4.0)));
    }

    #[test]
    fn subtract_ranges_works() {
        assert_eq!(subtract_ranges((1.0, 3.0), (1.0, 3.0)), vec![]);
        assert_eq!(subtract_ranges((1.0, 3.0), (1.0, 2.0)), vec![(3.0, 3.0)]);
        assert_eq!(
            subtract_ranges((0.0, 3.0), (1.0, 2.0)),
            vec![(0.0, 0.0), (3.0, 3.0)]
        );
        assert_eq!(
            subtract_ranges((0.0, 5.0), (2.0, 3.0)),
            vec![(0.0, 1.0), (4.0, 5.0)]
        );
    }

    #[test]
    fn create_ranges_works() {
        assert_eq!(create_ranges(&set(&[])), vec![]);
        assert_eq!(create_ranges(&set(&[1])), vec![(1.0, 1.0)]);
        assert_eq!(create_ranges(&set(&[1, 2])), vec![(1.0, 2.0)]);
        assert_eq!(create_ranges(&set(&[1, 3])), vec![(1.0, 1.0), (3.0, 3.0)]);
        assert_eq!(create_ranges(&set(&[1, 2, 3])), vec![(1.0, 3.0)]);
        assert_eq!(
            create_ranges(&set(&[5, 4, 1, 0])),
            vec![(0.0, 1.0), (4.0, 5.0)]
        );
    }

    #[test]
    fn invert_ranges_works() {
        let inf = f64::INFINITY;
        let ninf = f64::NEG_INFINITY;
        assert_eq!(invert_ranges(&[]), vec![(ninf, inf)]);
        assert_eq!(
            invert_ranges(&[(0.0, 0.0)]),
            vec![(ninf, -1.0), (1.0, inf)]
        );
        assert_eq!(invert_ranges(&[(1.0, 2.0)]), vec![(ninf, 0.0), (3.0, inf)]);
        assert_eq!(
            invert_ranges(&[(1.0, 2.0), (3.0, 4.0)]),
            vec![(ninf, 0.0), (5.0, inf)]
        );
        assert_eq!(
            invert_ranges(&[(1.0, 2.0), (4.0, 6.0), (10.0, 12.0)]),
            vec![(ninf, 0.0), (3.0, 3.0), (7.0, 9.0), (13.0, inf)]
        );
    }

    #[test]
    #[should_panic(expected = "Internal error: invalid ranges input")]
    fn invert_ranges_rejects_unsorted() {
        invert_ranges(&[(1.0, 1.0), (0.0, 0.0)]);
    }
}
