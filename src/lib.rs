//! Static ReDoS analysis for ECMA-262 regular expressions.
//!
//! This crate scores how vulnerable a regular expression is to ReDoS
//! (regular-expression denial of service). It does not run the pattern. It
//! enumerates the ways an input prefix could match and counts how many distinct
//! paths can match the same prefix. More ambiguity means more backtracking, so a
//! higher score. A score of `1` means every input matches at most one way.
//!
//! Start with [`is_safe`] for a parsed-flag check or [`is_safe_pattern`] for a
//! raw pattern with explicit options.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod arrays;
mod ast;
mod character_groups;
mod character_reader;
mod code_point;
mod map;
mod node_extra;
mod once;
mod our_range;
mod parse;
mod quantifier;
mod reader;
mod result_cache;
mod sets;
mod tree;
