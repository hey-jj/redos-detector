//! One-shot memoization of a zero-argument computation.

use std::cell::RefCell;
use std::rc::Rc;

/// Wraps a closure so it runs at most once and caches its result.
///
/// Later calls return the cached value without re-running the closure. This
/// matches the lazy-then-cached behavior the readers rely on when a fork
/// replays a step.
pub(crate) fn once<T, F>(f: F) -> impl FnMut() -> T
where
    T: Clone,
    F: FnOnce() -> T,
{
    let cell: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
    let mut f = Some(f);
    move || {
        if cell.borrow().is_none() {
            let value = (f.take().expect("once closure already taken"))();
            *cell.borrow_mut() = Some(value);
        }
        cell.borrow().clone().expect("value present after init")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn check<T: Clone + PartialEq + std::fmt::Debug>(value: T) {
        let calls = Rc::new(Cell::new(0));
        let calls_inner = calls.clone();
        let v = value.clone();
        let mut f = once(move || {
            calls_inner.set(calls_inner.get() + 1);
            v
        });

        assert_eq!(calls.get(), 0);
        assert_eq!(f(), value);
        assert_eq!(calls.get(), 1);
        assert_eq!(f(), value);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn works_for_value() {
        check(1);
    }

    #[test]
    fn works_for_unit() {
        check(());
    }
}
