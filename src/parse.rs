//! An ECMA-262 regular-expression parser.
//!
//! The parser produces the AST in [`crate::ast`] with UTF-16 offsets and exact
//! `raw` slices. It supports the grammar the analyzer needs: literals, classes,
//! escapes, groups including lookbehind, quantifiers, anchors, numbered
//! backreferences, and unicode property escapes in unicode mode. Named groups,
//! modifier groups, and the `v` flag are out of scope.
//!
//! Forward backreferences force a second pass, matching how an engine resolves
//! `\2()()`. The first pass counts capturing groups, then the input is parsed
//! again with the final count.

use crate::ast::{AnchorKind, ClassEscape, GroupBehavior, Node, NodeKind, RcNode};
use std::cell::Cell;
use std::rc::Rc;

/// A parse failure carrying the engine error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseError(pub(crate) String);

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

thread_local! {
    static NODE_ID: Cell<usize> = const { Cell::new(0) };
}

fn next_id() -> usize {
    NODE_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    })
}

struct Parser<'a> {
    units: &'a [u16],
    pos: usize,
    unicode: bool,
    closed_capture_counter: usize,
    first_iteration: bool,
    should_reparse: bool,
    backref_denied: Vec<usize>,
}

/// Parses `pattern` into an AST. `unicode` selects unicode mode.
pub(crate) fn parse(pattern: &str, unicode: bool) -> Result<RcNode, ParseError> {
    let units: Vec<u16> = pattern.encode_utf16().collect();

    let mut parser = Parser {
        units: &units,
        pos: 0,
        unicode,
        closed_capture_counter: 0,
        first_iteration: true,
        should_reparse: false,
        backref_denied: Vec::new(),
    };

    let result = parser.parse_disjunction()?;
    if result.range.1 != units.len() {
        return Err(ParseError(
            "Could not parse entire input - got stuck".to_string(),
        ));
    }

    let counter = parser.closed_capture_counter;
    let reparse = parser.should_reparse || parser.backref_denied.iter().any(|&r| r <= counter);
    if reparse {
        parser.pos = 0;
        parser.first_iteration = false;
        return parser.parse_disjunction();
    }

    Ok(result)
}

impl<'a> Parser<'a> {
    fn raw(&self, from: usize, to: usize) -> String {
        String::from_utf16_lossy(&self.units[from..to])
    }

    fn node(&self, range: (usize, usize), kind: NodeKind) -> RcNode {
        Rc::new(Node {
            id: next_id(),
            raw: self.raw(range.0, range.1),
            range,
            kind,
        })
    }

    fn unit(&self, at: usize) -> Option<u16> {
        self.units.get(at).copied()
    }

    fn current(&self) -> Option<u16> {
        self.unit(self.pos)
    }

    fn next_unit(&self) -> Option<u16> {
        self.unit(self.pos + 1)
    }

    fn match_str(&mut self, value: &str) -> bool {
        let v: Vec<u16> = value.encode_utf16().collect();
        if self.pos + v.len() <= self.units.len()
            && self.units[self.pos..self.pos + v.len()] == v[..]
        {
            self.pos += v.len();
            true
        } else {
            false
        }
    }

    fn current_one(&self, ch: char) -> bool {
        self.current() == Some(ch as u16)
    }

    fn match_one(&mut self, ch: char) -> bool {
        if self.current_one(ch) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_disjunction(&mut self) -> Result<RcNode, ParseError> {
        let from = self.pos;
        let mut res = vec![self.parse_alternative()?];
        while self.match_one('|') {
            res.push(self.parse_alternative()?);
        }
        if res.len() == 1 {
            return Ok(res.into_iter().next().unwrap());
        }
        let to = self.pos;
        Ok(self.node((from, to), NodeKind::Disjunction { body: res }))
    }

    fn parse_alternative(&mut self) -> Result<RcNode, ParseError> {
        let from = self.pos;
        let mut res = Vec::new();
        while let Some(term) = self.parse_term()? {
            res.push(term);
        }
        if res.len() == 1 {
            return Ok(res.into_iter().next().unwrap());
        }
        let to = self.pos;
        Ok(self.node((from, to), NodeKind::Alternative { body: res }))
    }

    /// Unwraps an `alternative` into its body, else wraps a node in a one-item
    /// vector. Group and quantifier bodies hold the flattened terms.
    fn flatten_body(node: RcNode) -> Vec<RcNode> {
        match &node.kind {
            NodeKind::Alternative { body } => body.iter().map(Rc::clone).collect(),
            _ => vec![node],
        }
    }

    fn parse_term(&mut self) -> Result<Option<RcNode>, ParseError> {
        if self.pos >= self.units.len() || self.current_one('|') || self.current_one(')') {
            return Ok(None);
        }

        if let Some(anchor) = self.parse_anchor()? {
            let pos_backup = self.pos;
            if let Some(mut quantifier) = self.parse_quantifier()? {
                if !self.unicode {
                    if let NodeKind::Group { .. } = &anchor.kind {
                        let start = anchor.range.0;
                        let body = Self::flatten_body(anchor);
                        quantifier = self.rebuild_quantifier_with_body(quantifier, body, start);
                        return Ok(Some(quantifier));
                    }
                }
                self.pos = pos_backup;
                return Err(ParseError("Expected atom".to_string()));
            }
            return Ok(Some(anchor));
        }

        let atom = self.parse_atom()?;
        let atom = match atom {
            Some(a) => a,
            None => {
                let pos_backup = self.pos;
                if self.parse_quantifier()?.is_some() {
                    self.pos = pos_backup;
                    return Err(ParseError("Expected atom".to_string()));
                }
                if !self.unicode && self.match_one('{') {
                    self.create_character('{' as u32, self.pos - 1, self.pos)
                } else {
                    return Err(ParseError("Expected atom".to_string()));
                }
            }
        };

        if let Some(quantifier) = self.parse_quantifier()? {
            if let NodeKind::Group { behavior, .. } = &atom.kind {
                if matches!(
                    behavior,
                    GroupBehavior::Lookbehind | GroupBehavior::NegativeLookbehind
                ) {
                    return Err(ParseError("Invalid quantifier".to_string()));
                }
            }
            let start = atom.range.0;
            let body = Self::flatten_body(atom);
            let quantifier = self.rebuild_quantifier_with_body(quantifier, body, start);
            return Ok(Some(quantifier));
        }
        Ok(Some(atom))
    }

    fn rebuild_quantifier_with_body(
        &self,
        quantifier: RcNode,
        body: Vec<RcNode>,
        start: usize,
    ) -> RcNode {
        let (min, max, greedy, symbol) = match &quantifier.kind {
            NodeKind::Quantifier {
                min,
                max,
                greedy,
                symbol,
                ..
            } => (*min, *max, *greedy, *symbol),
            _ => unreachable!("rebuild expects a quantifier"),
        };
        let inner = if body.len() == 1 {
            Rc::clone(&body[0])
        } else {
            let range = (body[0].range.0, body[body.len() - 1].range.1);
            self.node(range, NodeKind::Alternative { body })
        };
        let range = (start, quantifier.range.1);
        self.node(
            range,
            NodeKind::Quantifier {
                min,
                max,
                greedy,
                symbol,
                body: inner,
            },
        )
    }

    fn parse_anchor(&mut self) -> Result<Option<RcNode>, ParseError> {
        match self.current() {
            Some(c) if c == '^' as u16 => {
                self.pos += 1;
                Ok(Some(self.node(
                    (self.pos - 1, self.pos),
                    NodeKind::Anchor {
                        kind: AnchorKind::Start,
                    },
                )))
            }
            Some(c) if c == '$' as u16 => {
                self.pos += 1;
                Ok(Some(self.node(
                    (self.pos - 1, self.pos),
                    NodeKind::Anchor {
                        kind: AnchorKind::End,
                    },
                )))
            }
            Some(c) if c == '\\' as u16 => {
                if self.next_unit() == Some('b' as u16) {
                    self.pos += 2;
                    Ok(Some(self.node(
                        (self.pos - 2, self.pos),
                        NodeKind::Anchor {
                            kind: AnchorKind::Boundary,
                        },
                    )))
                } else if self.next_unit() == Some('B' as u16) {
                    self.pos += 2;
                    Ok(Some(self.node(
                        (self.pos - 2, self.pos),
                        NodeKind::Anchor {
                            kind: AnchorKind::NotBoundary,
                        },
                    )))
                } else {
                    Ok(None)
                }
            }
            Some(c) if c == '(' as u16 => {
                let from = self.pos;
                if self.match_str("(?=") {
                    Ok(Some(self.finish_group(GroupBehavior::Lookahead, from)?))
                } else if self.match_str("(?!") {
                    Ok(Some(
                        self.finish_group(GroupBehavior::NegativeLookahead, from)?,
                    ))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn finish_group(&mut self, behavior: GroupBehavior, from: usize) -> Result<RcNode, ParseError> {
        let body = self.parse_disjunction()?;
        if !self.match_one(')') {
            return Err(ParseError("Expected )".to_string()));
        }
        let body = Self::flatten_body(body);
        let to = self.pos;
        if behavior == GroupBehavior::Normal && self.first_iteration {
            self.closed_capture_counter += 1;
        }
        Ok(self.node((from, to), NodeKind::Group { behavior, body }))
    }

    fn parse_quantifier(&mut self) -> Result<Option<RcNode>, ParseError> {
        let from = self.pos;
        let mut min = 0u64;
        let mut max: Option<u64> = None;
        let mut symbol: Option<char> = None;
        let mut matched = false;

        match self.current() {
            Some(c) if c == '*' as u16 => {
                self.pos += 1;
                min = 0;
                max = None;
                symbol = Some('*');
                matched = true;
            }
            Some(c) if c == '+' as u16 => {
                self.pos += 1;
                min = 1;
                max = None;
                symbol = Some('+');
                matched = true;
            }
            Some(c) if c == '?' as u16 => {
                self.pos += 1;
                min = 0;
                max = Some(1);
                symbol = Some('?');
                matched = true;
            }
            Some(c) if c == '{' as u16 => {
                if let Some((m, mx, consumed)) = self.match_brace_quantifier() {
                    if mx.is_some() && mx.unwrap() < m {
                        return Err(ParseError(
                            "numbers out of order in {} quantifier".to_string(),
                        ));
                    }
                    self.pos += consumed;
                    min = m;
                    max = mx;
                    matched = true;
                    if min > MAX_SAFE_INTEGER || max.is_some_and(|v| v > MAX_SAFE_INTEGER) {
                        return Err(ParseError(
                            "iterations outside JS safe integer range in quantifier".to_string(),
                        ));
                    }
                }
            }
            _ => {}
        }

        if !matched {
            return Ok(None);
        }

        let mut to = self.pos;
        let mut greedy = true;
        if self.match_one('?') {
            greedy = false;
            to += 1;
        }

        // Quantifier body is attached by the caller. Use a placeholder body that
        // the caller replaces.
        let placeholder = self.node((from, to), NodeKind::Dot);
        Ok(Some(self.node(
            (from, to),
            NodeKind::Quantifier {
                min,
                max,
                greedy,
                symbol,
                body: placeholder,
            },
        )))
    }

    /// Parses `{n}`, `{n,}`, or `{n,m}` starting at the current `{`.
    ///
    /// Returns `(min, max, units_consumed)` or `None` when the brace does not
    /// form a quantifier.
    fn match_brace_quantifier(&self) -> Option<(u64, Option<u64>, usize)> {
        let mut i = self.pos;
        if self.unit(i) != Some('{' as u16) {
            return None;
        }
        i += 1;
        let digits_start = i;
        while self.unit(i).map(is_ascii_digit).unwrap_or(false) {
            i += 1;
        }
        if i == digits_start {
            return None;
        }
        let min = self.parse_uint(digits_start, i)?;
        match self.unit(i) {
            Some(c) if c == '}' as u16 => Some((min, Some(min), i + 1 - self.pos)),
            Some(c) if c == ',' as u16 => {
                i += 1;
                if self.unit(i) == Some('}' as u16) {
                    Some((min, None, i + 1 - self.pos))
                } else {
                    let max_start = i;
                    while self.unit(i).map(is_ascii_digit).unwrap_or(false) {
                        i += 1;
                    }
                    if i == max_start {
                        return None;
                    }
                    if self.unit(i) != Some('}' as u16) {
                        return None;
                    }
                    let max = self.parse_uint(max_start, i)?;
                    Some((min, Some(max), i + 1 - self.pos))
                }
            }
            _ => None,
        }
    }

    fn parse_uint(&self, from: usize, to: usize) -> Option<u64> {
        let mut value: u64 = 0;
        for &u in &self.units[from..to] {
            let digit = (u as u8 - b'0') as u64;
            // Saturate on overflow. The caller rejects any value above
            // MAX_SAFE_INTEGER, so u64::MAX still triggers the correct error.
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(digit))
                .unwrap_or(u64::MAX);
        }
        Some(value)
    }

    fn create_character(&self, code_point: u32, from: usize, to: usize) -> RcNode {
        self.node((from, to), NodeKind::Value { code_point })
    }

    fn parse_atom(&mut self) -> Result<Option<RcNode>, ParseError> {
        match self.current() {
            None => Ok(None),
            Some(c) if c == '.' as u16 => {
                self.pos += 1;
                Ok(Some(self.node((self.pos - 1, self.pos), NodeKind::Dot)))
            }
            Some(c) if c == '\\' as u16 => {
                self.pos += 1;
                match self.parse_atom_escape(false)? {
                    Some(node) => Ok(Some(node)),
                    None => {
                        if !self.unicode && self.current_one('c') {
                            return Ok(Some(self.create_character(92, self.pos - 1, self.pos)));
                        }
                        Err(ParseError("atomEscape".to_string()))
                    }
                }
            }
            Some(c) if c == '[' as u16 => Ok(Some(self.parse_character_class()?)),
            Some(c) if c == '(' as u16 => {
                let from = self.pos;
                if self.match_str("(?<=") {
                    Ok(Some(self.finish_group(GroupBehavior::Lookbehind, from)?))
                } else if self.match_str("(?<!") {
                    Ok(Some(
                        self.finish_group(GroupBehavior::NegativeLookbehind, from)?,
                    ))
                } else if self.match_str("(?:") {
                    Ok(Some(self.finish_group(GroupBehavior::Ignore, from)?))
                } else if self.match_str("(") {
                    Ok(Some(self.finish_group(GroupBehavior::Normal, from)?))
                } else {
                    Err(ParseError("group".to_string()))
                }
            }
            Some(c) if (c == ']' as u16 || c == '}' as u16) && !self.unicode => {
                self.pos += 1;
                Ok(Some(self.create_character(
                    c as u32,
                    self.pos - 1,
                    self.pos,
                )))
            }
            Some(c)
                if c == '^' as u16
                    || c == '$' as u16
                    || c == '*' as u16
                    || c == '+' as u16
                    || c == '?' as u16
                    || c == '{' as u16
                    || c == ')' as u16
                    || c == '|' as u16
                    || (self.unicode && (c == ']' as u16 || c == '}' as u16)) =>
            {
                Ok(None)
            }
            Some(_) => Ok(Some(self.parse_pattern_character())),
        }
    }

    fn parse_pattern_character(&mut self) -> RcNode {
        let first = self.current().unwrap();
        if self.unicode && (0xD800..=0xDBFF).contains(&first) {
            if let Some(second) = self.next_unit() {
                if (0xDC00..=0xDFFF).contains(&second) {
                    self.pos += 2;
                    let code_point =
                        (first as u32 - 0xD800) * 0x400 + (second as u32 - 0xDC00) + 0x10000;
                    return self.create_character(code_point, self.pos - 2, self.pos);
                }
            }
        }
        self.pos += 1;
        self.create_character(first as u32, self.pos - 1, self.pos)
    }

    fn parse_atom_escape(&mut self, inside_class: bool) -> Result<Option<RcNode>, ParseError> {
        let ch = match self.current() {
            Some(c) => c,
            None => return self.parse_identity_escape(),
        };
        let c = char::from_u32(ch as u32).unwrap_or('\u{0}');
        match c {
            '0'..='9' => self.parse_decimal_escape(inside_class),
            'B' => {
                if inside_class {
                    Err(ParseError(
                        "\\B not possible inside of CharacterClass".to_string(),
                    ))
                } else {
                    self.parse_identity_escape()
                }
            }
            'b' => {
                if inside_class {
                    self.pos += 1;
                    Ok(Some(self.create_character(0x0008, self.pos - 2, self.pos)))
                } else {
                    self.parse_identity_escape()
                }
            }
            'd' | 'D' | 'w' | 'W' | 's' | 'S' => {
                self.pos += 1;
                let value = match c {
                    'd' => ClassEscape::D,
                    'D' => ClassEscape::DUpper,
                    'w' => ClassEscape::W,
                    'W' => ClassEscape::WUpper,
                    's' => ClassEscape::S,
                    _ => ClassEscape::SUpper,
                };
                Ok(Some(self.node(
                    (self.pos - 2, self.pos),
                    NodeKind::CharacterClassEscape { value },
                )))
            }
            'p' | 'P' => match self.parse_unicode_property_escape()? {
                Some(node) => Ok(Some(node)),
                None => self.parse_identity_escape(),
            },
            '-' if inside_class && self.unicode => {
                self.pos += 1;
                Ok(Some(self.create_character(0x002d, self.pos - 2, self.pos)))
            }
            _ => self.parse_character_escape(),
        }
    }

    fn parse_decimal_escape(&mut self, inside_class: bool) -> Result<Option<RcNode>, ParseError> {
        let from = self.pos;
        // DecimalIntegerLiteral not starting with 0.
        if self.current() != Some('0' as u16) {
            let start = self.pos;
            while self.current().map(is_ascii_digit).unwrap_or(false) {
                self.pos += 1;
            }
            if self.pos > start {
                let digits = self.raw(start, self.pos);
                let ref_idx: usize = digits.parse().unwrap_or(usize::MAX);
                if ref_idx <= self.closed_capture_counter && !inside_class {
                    let node = self.node(
                        (from - 1, self.pos),
                        NodeKind::Reference {
                            match_index: ref_idx as u64,
                        },
                    );
                    return Ok(Some(node));
                }
                self.backref_denied.push(ref_idx);
                if self.first_iteration {
                    self.should_reparse = true;
                }
                if self.unicode && !self.first_iteration {
                    return Err(ParseError("Invalid escape".to_string()));
                }
                // Reset and re-match octal digits only.
                self.pos = start;
                let octal_start = self.pos;
                let mut count = 0;
                while count < 3 && self.current().map(is_octal_digit).unwrap_or(false) {
                    self.pos += 1;
                    count += 1;
                }
                if self.pos > octal_start {
                    let octal = self.raw(octal_start, self.pos);
                    let value = u32::from_str_radix(&octal, 8).unwrap_or(0);
                    return Ok(Some(self.create_character(value, from - 1, self.pos)));
                }
                // Case like \91: ignore slash, take a single 8 or 9.
                let cstart = self.pos;
                if let Some(u) = self.current() {
                    if u == '8' as u16 || u == '9' as u16 {
                        self.pos += 1;
                        return Ok(Some(self.create_character(u as u32, cstart - 1, self.pos)));
                    }
                }
                return Ok(None);
            }
        }
        // Octal numbers starting with 0.
        let octal_start = self.pos;
        let mut count = 0;
        while count < 3 && self.current().map(is_octal_digit).unwrap_or(false) {
            self.pos += 1;
            count += 1;
        }
        if self.pos > octal_start {
            let matched = self.raw(octal_start, self.pos);
            if matched.chars().all(|c| c == '0') {
                // All zeros: take the first one only.
                self.pos = octal_start + 1;
                return Ok(Some(self.create_character(0, self.pos - 1, self.pos)));
            }
            let value = u32::from_str_radix(&matched, 8).unwrap_or(0);
            return Ok(Some(self.create_character(value, from - 1, self.pos)));
        }
        Ok(None)
    }

    fn parse_unicode_property_escape(&mut self) -> Result<Option<RcNode>, ParseError> {
        if !self.unicode {
            return Ok(None);
        }
        let from = self.pos;
        let sign = self.current();
        let negative = sign == Some('P' as u16);
        if sign != Some('p' as u16) && sign != Some('P' as u16) {
            return Ok(None);
        }
        if self.unit(from + 1) != Some('{' as u16) {
            return Ok(None);
        }
        let mut i = from + 2;
        while self.unit(i).map(|u| u != '}' as u16).unwrap_or(false) {
            i += 1;
        }
        if self.unit(i) != Some('}' as u16) || i == from + 2 {
            return Ok(None);
        }
        let value = self.raw(from + 2, i);
        self.pos = i + 1;
        Ok(Some(self.node(
            (from - 1, self.pos),
            NodeKind::UnicodePropertyEscape { negative, value },
        )))
    }

    fn parse_character_escape(&mut self) -> Result<Option<RcNode>, ParseError> {
        let from = self.pos;
        match self.current().and_then(|c| char::from_u32(c as u32)) {
            Some('t') => {
                self.pos += 1;
                Ok(Some(self.create_character(0x09, from - 1, self.pos)))
            }
            Some('n') => {
                self.pos += 1;
                Ok(Some(self.create_character(0x0A, from - 1, self.pos)))
            }
            Some('v') => {
                self.pos += 1;
                Ok(Some(self.create_character(0x0B, from - 1, self.pos)))
            }
            Some('f') => {
                self.pos += 1;
                Ok(Some(self.create_character(0x0C, from - 1, self.pos)))
            }
            Some('r') => {
                self.pos += 1;
                Ok(Some(self.create_character(0x0D, from - 1, self.pos)))
            }
            Some('c') => {
                if let Some(letter) = self.unit(from + 1) {
                    let lc = letter as u32;
                    if (0x41..=0x5A).contains(&lc) || (0x61..=0x7A).contains(&lc) {
                        self.pos += 2;
                        return Ok(Some(self.create_character(lc % 32, from - 1, self.pos)));
                    }
                }
                self.parse_identity_escape()
            }
            Some('x') => {
                if let Some(value) = self.match_hex(from + 1, 2) {
                    self.pos = from + 3;
                    return Ok(Some(self.create_character(value, from - 1, self.pos)));
                }
                self.parse_identity_escape()
            }
            Some('u') => {
                if let Some(node) = self.parse_unicode_escape(from)? {
                    return Ok(Some(node));
                }
                self.parse_identity_escape()
            }
            _ => self.parse_identity_escape(),
        }
    }

    fn parse_unicode_escape(&mut self, from: usize) -> Result<Option<RcNode>, ParseError> {
        // \uXXXX
        if let Some(value) = self.match_hex(from + 1, 4) {
            let end = from + 5;
            // Surrogate pair in unicode mode.
            if self.unicode
                && (0xD800..=0xDBFF).contains(&value)
                && self.unit(end) == Some('\\' as u16)
                && self.unit(end + 1) == Some('u' as u16)
            {
                if let Some(second) = self.match_hex(end + 2, 4) {
                    if (0xDC00..=0xDFFF).contains(&second) {
                        let code_point = (value - 0xD800) * 0x400 + (second - 0xDC00) + 0x10000;
                        self.pos = end + 6;
                        return Ok(Some(self.create_character(code_point, from - 1, self.pos)));
                    }
                }
            }
            self.pos = end;
            return Ok(Some(self.create_character(value, from - 1, self.pos)));
        }
        // \u{XXXX}
        if self.unicode && self.unit(from + 1) == Some('{' as u16) {
            let mut i = from + 2;
            while self.unit(i).map(is_hex_digit).unwrap_or(false) {
                i += 1;
            }
            if i > from + 2 && self.unit(i) == Some('}' as u16) {
                let hex = self.raw(from + 2, i);
                let value = u32::from_str_radix(&hex, 16).unwrap_or(0);
                if value > 0x10FFFF {
                    return Err(ParseError("Invalid escape sequence".to_string()));
                }
                self.pos = i + 1;
                return Ok(Some(self.create_character(value, from - 1, self.pos)));
            }
        }
        Ok(None)
    }

    fn match_hex(&self, from: usize, len: usize) -> Option<u32> {
        if from + len > self.units.len() {
            return None;
        }
        let mut value = 0u32;
        for j in 0..len {
            let u = self.units[from + j];
            if !is_hex_digit(u) {
                return None;
            }
            let digit = char::from_u32(u as u32).unwrap().to_digit(16).unwrap();
            value = value * 16 + digit;
        }
        Some(value)
    }

    fn parse_identity_escape(&mut self) -> Result<Option<RcNode>, ParseError> {
        let l = match self.current() {
            Some(c) => c,
            None => return Ok(None),
        };
        let lc = char::from_u32(l as u32).unwrap_or('\u{0}');
        let syntax = "^$.*+?()\\[]{}|/";
        let allowed = (self.unicode && syntax.contains(lc)) || (!self.unicode && lc != 'c');
        if allowed {
            self.pos += 1;
            return Ok(Some(self.create_character(
                l as u32,
                self.pos - 1,
                self.pos,
            )));
        }
        Ok(None)
    }

    fn parse_character_class(&mut self) -> Result<RcNode, ParseError> {
        let from = self.pos;
        let negative = if self.match_str("[^") {
            true
        } else if self.match_one('[') {
            false
        } else {
            return Err(ParseError("character class".to_string()));
        };

        let body = self.parse_class_contents()?;
        if !self.match_one(']') {
            return Err(ParseError("Expected ]".to_string()));
        }
        let to = self.pos;
        Ok(self.node((from, to), NodeKind::CharacterClass { negative, body }))
    }

    fn parse_class_contents(&mut self) -> Result<Vec<RcNode>, ParseError> {
        if self.current_one(']') {
            return Ok(Vec::new());
        }
        self.parse_nonempty_class_ranges()
    }

    fn parse_nonempty_class_ranges(&mut self) -> Result<Vec<RcNode>, ParseError> {
        let atom = self.parse_class_atom()?;
        if self.current_one(']') {
            return Ok(vec![atom]);
        }
        self.parse_helper_class_contents(atom)
    }

    fn parse_helper_class_contents(&mut self, atom: RcNode) -> Result<Vec<RcNode>, ParseError> {
        if self.current_one('-') && self.next_unit() != Some(']' as u16) {
            let from = atom.range.0;
            self.pos += 1;
            let dash = self.create_character('-' as u32, self.pos - 1, self.pos);
            let atom_to = self.parse_class_atom()?;
            let to = self.pos;
            let rest = self.parse_class_contents()?;

            let mut res = if let (Some(min), Some(max)) = (atom.code_point(), atom_to.code_point())
            {
                if min > max {
                    return Err(ParseError(
                        "Range out of order in character class".to_string(),
                    ));
                }
                vec![self.node(
                    (from, to),
                    NodeKind::CharacterClassRange {
                        min: Rc::clone(&atom),
                        max: Rc::clone(&atom_to),
                    },
                )]
            } else if !self.unicode {
                vec![atom, dash, atom_to]
            } else {
                return Err(ParseError("invalid character class".to_string()));
            };
            res.extend(rest);
            return Ok(res);
        }

        let rest = self.parse_nonempty_class_ranges_no_dash()?;
        let mut res = vec![atom];
        res.extend(rest);
        Ok(res)
    }

    fn parse_nonempty_class_ranges_no_dash(&mut self) -> Result<Vec<RcNode>, ParseError> {
        let atom = self.parse_class_atom()?;
        if self.current_one(']') {
            return Ok(vec![atom]);
        }
        self.parse_helper_class_contents(atom)
    }

    fn parse_class_atom(&mut self) -> Result<RcNode, ParseError> {
        if self.match_one('-') {
            return Ok(self.create_character('-' as u32, self.pos - 1, self.pos));
        }
        self.parse_class_atom_no_dash()
    }

    fn parse_class_atom_no_dash(&mut self) -> Result<RcNode, ParseError> {
        match self.current() {
            Some(c) if c == '\\' as u16 => {
                self.pos += 1;
                match self.parse_atom_escape(true)? {
                    Some(node) => Ok(node),
                    None => {
                        if !self.unicode && self.current_one('c') {
                            Ok(self.create_character('\\' as u32, self.pos - 1, self.pos))
                        } else {
                            Err(ParseError("classEscape".to_string()))
                        }
                    }
                }
            }
            Some(c) if c == ']' as u16 || c == '-' as u16 => {
                Err(ParseError("classAtom".to_string()))
            }
            Some(_) => Ok(self.parse_pattern_character()),
            None => Err(ParseError("classAtom".to_string())),
        }
    }
}

fn is_ascii_digit(u: u16) -> bool {
    (0x30..=0x39).contains(&u)
}

fn is_octal_digit(u: u16) -> bool {
    (0x30..=0x37).contains(&u)
}

fn is_hex_digit(u: u16) -> bool {
    (0x30..=0x39).contains(&u) || (0x41..=0x46).contains(&u) || (0x61..=0x66).contains(&u)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raws(p: &str, unicode: bool) -> Vec<(usize, usize, String)> {
        fn walk(node: &RcNode, out: &mut Vec<(usize, usize, String)>) {
            out.push((node.range.0, node.range.1, node.raw.clone()));
            match &node.kind {
                NodeKind::Alternative { body } | NodeKind::Disjunction { body } => {
                    for b in body {
                        walk(b, out);
                    }
                }
                NodeKind::Group { body, .. } | NodeKind::CharacterClass { body, .. } => {
                    for b in body {
                        walk(b, out);
                    }
                }
                NodeKind::Quantifier { body, .. } => walk(body, out),
                NodeKind::CharacterClassRange { min, max } => {
                    walk(min, out);
                    walk(max, out);
                }
                _ => {}
            }
        }
        let ast = parse(p, unicode).unwrap();
        let mut out = Vec::new();
        walk(&ast, &mut out);
        out
    }

    #[test]
    fn ranges_and_raw_match() {
        // Top-level node spans the whole pattern with exact raw text.
        let r = raws("^a(b)c\\1", false);
        assert_eq!(r[0], (0, 8, "^a(b)c\\1".to_string()));
        assert!(r.iter().any(|e| e.2 == "\\1" && e.0 == 6 && e.1 == 8));
        assert!(r.iter().any(|e| e.2 == "(b)" && e.0 == 2 && e.1 == 5));

        // Quantifier range starts at its atom.
        let r = raws("a{0,2}", false);
        assert_eq!(r[0], (0, 6, "a{0,2}".to_string()));

        // Astral literal without unicode splits into two surrogate halves.
        let r = raws("\u{1f44d}", false);
        assert_eq!(r.len(), 3); // alternative + two values
                                // With unicode it is a single value.
        let r = raws("\u{1f44d}", true);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, 0);
        assert_eq!(r[0].1, 2);
    }

    #[test]
    fn empty_pattern_is_empty_alternative() {
        let ast = parse("", false).unwrap();
        assert!(matches!(&ast.kind, NodeKind::Alternative { body } if body.is_empty()));
        assert_eq!(ast.range, (0, 0));
        assert_eq!(ast.raw, "");
    }

    #[test]
    fn rejects_too_many_iterations() {
        assert!(parse("a{0,9007199254740992}", false).is_err());
        assert!(parse("a{0,9007199254740991}", false).is_ok());
    }

    #[test]
    fn out_of_range_backreference_is_literal() {
        // \99 with no 99 groups parses as octal/literal, not a reference.
        let ast = parse("^a+\\99$", false).unwrap();
        let mut found_ref = false;
        fn check(node: &RcNode, found: &mut bool) {
            if let NodeKind::Reference { .. } = node.kind {
                *found = true;
            }
            match &node.kind {
                NodeKind::Alternative { body } | NodeKind::Group { body, .. } => {
                    for b in body {
                        check(b, found);
                    }
                }
                NodeKind::Quantifier { body, .. } => check(body, found),
                _ => {}
            }
        }
        check(&ast, &mut found_ref);
        assert!(!found_ref);
    }
}
