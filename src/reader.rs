//! Lazy, forkable readers.
//!
//! A reader is an iterator that yields a stream of values and ends with a return
//! value. The analyzer explores branch points by forking a reader: each fork
//! replays the values produced so far and then pulls new ones from the shared
//! source. Forking is what lets the checker try alternative paths without
//! re-running a shared prefix.

use std::cell::RefCell;
use std::rc::Rc;

/// One step of a reader: a yielded value or the final return.
pub(crate) enum Step<T, R> {
    /// A yielded value.
    Value(T),
    /// The reader finished with this return value.
    Done(R),
}

/// A reader yields `T` values and finishes with an `R`.
pub(crate) trait Reader<T, R> {
    /// Advances the reader by one step.
    fn next(&mut self) -> Step<T, R>;
}

impl<T, R> Reader<T, R> for Box<dyn Reader<T, R>> {
    fn next(&mut self) -> Step<T, R> {
        (**self).next()
    }
}

/// A boxed reader.
pub(crate) type BoxReader<T, R> = Box<dyn Reader<T, R>>;

/// A reader that yields nothing and returns the default value.
pub(crate) struct EmptyReader<R> {
    value: Option<R>,
}

impl<R: Default> Default for EmptyReader<R> {
    fn default() -> Self {
        EmptyReader {
            value: Some(R::default()),
        }
    }
}

impl<T, R> Reader<T, R> for EmptyReader<R> {
    fn next(&mut self) -> Step<T, R> {
        Step::Done(self.value.take().expect("empty reader polled twice"))
    }
}

/// Builds a reader that yields nothing and returns `R::default()`.
pub(crate) fn empty_reader<T, R: Default + 'static>() -> BoxReader<T, R>
where
    T: 'static,
{
    Box::new(EmptyReader::<R>::default())
}

/// A reader that yields each item of a vector then returns the default.
pub(crate) struct ArrayReader<T, R> {
    items: std::vec::IntoIter<T>,
    value: Option<R>,
}

impl<T, R: Default> ArrayReader<T, R> {
    /// Builds a reader over `items`.
    pub(crate) fn new(items: Vec<T>) -> Self {
        ArrayReader {
            items: items.into_iter(),
            value: Some(R::default()),
        }
    }
}

impl<T, R> Reader<T, R> for ArrayReader<T, R> {
    fn next(&mut self) -> Step<T, R> {
        match self.items.next() {
            Some(item) => Step::Value(item),
            None => Step::Done(self.value.take().expect("array reader polled past end")),
        }
    }
}

/// Builds a reader that yields each item then returns `R::default()`.
pub(crate) fn build_array_reader<T: 'static, R: Default + 'static>(
    items: Vec<T>,
) -> BoxReader<T, R> {
    Box::new(ArrayReader::new(items))
}

/// A reader that runs a list of readers back to back.
///
/// The chained readers all return `()`; the chain itself returns `()`.
pub(crate) struct ChainReader<T> {
    readers: std::vec::IntoIter<BoxReader<T, ()>>,
    current: Option<BoxReader<T, ()>>,
}

impl<T: 'static> ChainReader<T> {
    /// Builds a chain over `readers`.
    pub(crate) fn new(readers: Vec<BoxReader<T, ()>>) -> Self {
        let mut iter = readers.into_iter();
        let current = iter.next();
        ChainReader {
            readers: iter,
            current,
        }
    }
}

impl<T> Reader<T, ()> for ChainReader<T> {
    fn next(&mut self) -> Step<T, ()> {
        loop {
            match &mut self.current {
                Some(reader) => match reader.next() {
                    Step::Value(value) => return Step::Value(value),
                    Step::Done(()) => {
                        self.current = self.readers.next();
                    }
                },
                None => return Step::Done(()),
            }
        }
    }
}

/// Builds a reader that concatenates `readers`.
pub(crate) fn chain_readers<T: 'static>(readers: Vec<BoxReader<T, ()>>) -> BoxReader<T, ()> {
    Box::new(ChainReader::new(readers))
}

/// Shared buffer backing a set of forks of one source reader.
struct ForkShared<T, R> {
    source: BoxReader<T, R>,
    buffer: Vec<T>,
    return_value: Option<R>,
}

/// A reader that can be cheaply forked.
///
/// Each fork holds a cursor into a shared buffer. Pulling past the buffer reads
/// from the single source and appends to the buffer so other forks can replay
/// it. Values must be cloneable and the return value must be cloneable.
pub(crate) struct ForkableReader<T: Clone, R: Clone> {
    shared: Rc<RefCell<ForkShared<T, R>>>,
    cursor: usize,
}

impl<T: Clone, R: Clone> ForkableReader<T, R> {
    /// Wraps `source` so it can be forked. The source must not be read directly.
    pub(crate) fn new(source: BoxReader<T, R>) -> Self {
        ForkableReader {
            shared: Rc::new(RefCell::new(ForkShared {
                source,
                buffer: Vec::new(),
                return_value: None,
            })),
            cursor: 0,
        }
    }

    /// Returns a fork that replays from the same point.
    pub(crate) fn fork(&self) -> Self {
        ForkableReader {
            shared: Rc::clone(&self.shared),
            cursor: self.cursor,
        }
    }
}

impl<T: Clone, R: Clone> Reader<T, R> for ForkableReader<T, R> {
    fn next(&mut self) -> Step<T, R> {
        let mut shared = self.shared.borrow_mut();
        if self.cursor < shared.buffer.len() {
            let value = shared.buffer[self.cursor].clone();
            self.cursor += 1;
            return Step::Value(value);
        }
        if let Some(ret) = &shared.return_value {
            return Step::Done(ret.clone());
        }
        match shared.source.next() {
            Step::Value(value) => {
                shared.buffer.push(value.clone());
                self.cursor += 1;
                Step::Value(value)
            }
            Step::Done(ret) => {
                shared.return_value = Some(ret.clone());
                Step::Done(ret)
            }
        }
    }
}

/// Builds a forkable reader around `source`.
pub(crate) fn build_forkable_reader<T: Clone, R: Clone>(
    source: BoxReader<T, R>,
) -> ForkableReader<T, R> {
    ForkableReader::new(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_yields_in_order() {
        let mut reader = chain_readers(vec![
            build_array_reader(vec![0, 1]),
            build_array_reader(vec![2, 3]),
        ]);
        assert!(matches!(reader.next(), Step::Value(0)));
        assert!(matches!(reader.next(), Step::Value(1)));
        assert!(matches!(reader.next(), Step::Value(2)));
        assert!(matches!(reader.next(), Step::Value(3)));
        assert!(matches!(reader.next(), Step::Done(())));
    }

    #[test]
    fn empty_is_immediately_done() {
        let mut reader: BoxReader<i32, ()> = empty_reader();
        assert!(matches!(reader.next(), Step::Done(())));
    }

    #[test]
    fn fork_replays() {
        let source: BoxReader<i32, ()> = build_array_reader(vec![1, 2, 3]);
        let mut a = build_forkable_reader(source);
        assert!(matches!(a.next(), Step::Value(1)));
        let mut b = a.fork();
        assert!(matches!(a.next(), Step::Value(2)));
        assert!(matches!(b.next(), Step::Value(2)));
        assert!(matches!(b.next(), Step::Value(3)));
        assert!(matches!(a.next(), Step::Value(3)));
        assert!(matches!(a.next(), Step::Done(())));
        assert!(matches!(b.next(), Step::Done(())));
    }
}
