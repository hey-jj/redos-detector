//! Maps each value of a level-0 reader, passing splits through.
//!
//! Character and zero-width entries go through `handle`. Splits keep their
//! sub-type but their branch reader is wrapped so the same mapping applies to
//! everything the branch produces.

use crate::character_reader::level0::{CharacterReader, CharacterReaderValue, ReaderFactory};
use crate::reader::{Reader, Step};
use std::rc::Rc;

type Handler = Rc<dyn Fn(CharacterReaderValue) -> CharacterReaderValue>;

struct MapReader {
    inner: CharacterReader,
    handle: Handler,
}

impl Reader<CharacterReaderValue, ()> for MapReader {
    fn next(&mut self) -> Step<CharacterReaderValue, ()> {
        match self.inner.next() {
            Step::Done(()) => Step::Done(()),
            Step::Value(value) => match value {
                CharacterReaderValue::Split { reader, sub_type } => {
                    let handle = Rc::clone(&self.handle);
                    let wrapped: ReaderFactory =
                        Rc::new(move || start_thread(reader(), Rc::clone(&handle)));
                    Step::Value(CharacterReaderValue::Split {
                        reader: wrapped,
                        sub_type,
                    })
                }
                other => Step::Value((self.handle)(other)),
            },
        }
    }
}

fn start_thread(inner: CharacterReader, handle: Handler) -> CharacterReader {
    Box::new(MapReader { inner, handle })
}

/// Maps each value of `reader` through `handle`, wrapping split branches.
pub(crate) fn map<F>(reader: CharacterReader, handle: F) -> CharacterReader
where
    F: Fn(CharacterReaderValue) -> CharacterReaderValue + 'static,
{
    start_thread(reader, Rc::new(handle))
}
