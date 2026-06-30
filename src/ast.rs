//! Regex AST shared by the parser and the analyzer.
//!
//! Offsets in [`Node::range`] are UTF-16 code unit indices into the source
//! pattern, matching how an ECMA-262 engine sees the string. Each node carries a
//! unique [`Node::id`] so the analyzer can use node identity as a map or set
//! key.

use std::rc::Rc;

/// The behavior of a `(...)` group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupBehavior {
    /// A capturing group `( )`.
    Normal,
    /// A non-capturing group `(?: )`.
    Ignore,
    /// A positive lookahead `(?= )`.
    Lookahead,
    /// A negative lookahead `(?! )`.
    NegativeLookahead,
    /// A positive lookbehind `(?<= )`.
    Lookbehind,
    /// A negative lookbehind `(?<! )`.
    NegativeLookbehind,
}

impl GroupBehavior {
    /// Returns `true` for the four lookaround behaviors.
    pub(crate) fn is_lookaround(self) -> bool {
        !matches!(self, GroupBehavior::Normal | GroupBehavior::Ignore)
    }

    /// Returns `true` for negative lookahead and negative lookbehind.
    pub(crate) fn is_negative(self) -> bool {
        matches!(
            self,
            GroupBehavior::NegativeLookahead | GroupBehavior::NegativeLookbehind
        )
    }
}

/// The kind of anchor an `anchor` node represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnchorKind {
    /// `^`
    Start,
    /// `$`
    End,
    /// `\b`
    Boundary,
    /// `\B`
    NotBoundary,
}

/// A character-class escape `\d \D \w \W \s \S`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClassEscape {
    /// `\d`
    D,
    /// `\D`
    DUpper,
    /// `\w`
    W,
    /// `\W`
    WUpper,
    /// `\s`
    S,
    /// `\S`
    SUpper,
}

/// One node of the parsed pattern.
#[derive(Debug)]
pub(crate) struct Node {
    /// Stable identity assigned at parse time.
    pub(crate) id: usize,
    /// `[start, end)` in UTF-16 code units.
    pub(crate) range: (usize, usize),
    /// The exact source text the node spans.
    pub(crate) raw: String,
    /// The node payload.
    pub(crate) kind: NodeKind,
}

/// A reference-counted AST node.
pub(crate) type RcNode = Rc<Node>;

/// The payload of an AST node.
#[derive(Debug)]
pub(crate) enum NodeKind {
    /// A sequence of terms (concatenation).
    Alternative { body: Vec<RcNode> },
    /// A choice of branches `a|b|c`.
    Disjunction { body: Vec<RcNode> },
    /// A `(...)` group of any behavior.
    Group {
        behavior: GroupBehavior,
        body: Vec<RcNode>,
    },
    /// A quantified term. `max` is `None` for unbounded.
    Quantifier {
        min: u64,
        max: Option<u64>,
        greedy: bool,
        symbol: Option<char>,
        body: RcNode,
    },
    /// A single literal code point.
    Value { code_point: u32 },
    /// A `[...]` character class.
    CharacterClass { negative: bool, body: Vec<RcNode> },
    /// A `min-max` range inside a character class.
    CharacterClassRange { min: RcNode, max: RcNode },
    /// A `\d \D \w \W \s \S` escape.
    CharacterClassEscape { value: ClassEscape },
    /// A `\p{...}` or `\P{...}` escape.
    UnicodePropertyEscape { negative: bool, value: String },
    /// The `.` metacharacter.
    Dot,
    /// `^ $ \b \B`.
    Anchor { kind: AnchorKind },
    /// A numbered backreference `\1`.
    Reference { match_index: u64 },
}

impl Node {
    /// Returns the code point when this node is a `value`, else `None`.
    pub(crate) fn code_point(&self) -> Option<u32> {
        match &self.kind {
            NodeKind::Value { code_point } => Some(*code_point),
            _ => None,
        }
    }
}
