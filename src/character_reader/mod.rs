//! The three-level character reader stack.
//!
//! Level 0 walks the AST and yields one entry per consumed character, plus
//! split markers at branch points and zero-width markers for anchors and empty
//! iterations. Level 1 folds the zero-width markers onto the next real entry and
//! turns the pattern end into the reader's return value. Level 2 records group
//! contents and expands backreferences into the characters their group matched.

pub(crate) mod join;
pub(crate) mod level0;
pub(crate) mod level1;
pub(crate) mod level2;
pub(crate) mod map;
