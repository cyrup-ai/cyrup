//! A tiny, dependency-free, case-insensitive regular-expression matcher.
//!
//! cyrup may not pull in the `regex` crate (workspace dep budget is frozen for this round), yet
//! Pi's `utils/overflow.ts` and `utils/retry.ts` classify provider errors with a fixed set of
//! case-insensitive (`/…/i`) patterns. Those patterns use only a small, well-bounded slice of the
//! regex grammar — literals, `.`, `\d`, `\s`, escaped metacharacters, character classes `[…]` with
//! ranges, the quantifiers `?`/`*`/`+`/`{n}`/`{n,m}`, non-capturing groups `(?:…)`, alternation
//! `|`, and the start anchor `^`. This module implements exactly that slice with a backtracking
//! matcher, so the overflow/retry classifiers can be ported **1:1** without a new dependency.
//!
//! Matching is always **case-insensitive** (every Pi pattern carries the `i` flag) and **unanchored**
//! (a match anywhere in the haystack succeeds, exactly like JS `RegExp.prototype.test`), unless an
//! explicit `^` anchor pins the start. Never panics: a malformed pattern degrades to never matching
//! rather than aborting.

/// One element of a compiled pattern.
#[derive(Clone, Debug)]
enum Node {
    /// A single literal character (compared case-insensitively).
    Char(char),
    /// `.` — any single character.
    Any,
    /// `\d` — an ASCII digit.
    Digit,
    /// `\s` — ASCII whitespace (` \t\n\r\x0c\x0b`), matching JS `\s` for the inputs we see.
    Space,
    /// `[…]` character class.
    Class { items: Vec<ClassItem>, negated: bool },
    /// `(?:…|…)` non-capturing group: a list of alternative sequences.
    Group(Vec<Vec<Node>>),
    /// A quantifier applied to the preceding single element.
    Quant { inner: Box<Node>, min: usize, max: usize },
    /// `^` start-of-string anchor.
    Start,
}

#[derive(Clone, Debug)]
enum ClassItem {
    Char(char),
    Range(char, char),
    Digit,
    Space,
}

/// A compiled, case-insensitive pattern: a top-level alternation of sequences.
pub struct Regex {
    alternatives: Vec<Vec<Node>>,
}

impl Regex {
    /// Compile `pattern`. An unparseable pattern compiles to one that never matches (no panic).
    pub fn new(pattern: &str) -> Self {
        let chars: Vec<char> = pattern.chars().collect();
        let mut parser = Parser { chars: &chars, pos: 0 };
        let alternatives = parser.parse_alternation().unwrap_or_default();
        // A trailing unconsumed tail means a parse error → never-match.
        if parser.pos != chars.len() {
            return Regex { alternatives: Vec::new() };
        }
        Regex { alternatives }
    }

    /// `true` if the pattern matches anywhere in `text` (case-insensitive, JS `test` semantics).
    pub fn is_match(&self, text: &str) -> bool {
        if self.alternatives.is_empty() {
            return false;
        }
        let chars: Vec<char> = text.chars().collect();
        for start in 0..=chars.len() {
            for alt in &self.alternatives {
                if !match_nodes(alt, &chars, start).is_empty() {
                    return true;
                }
            }
        }
        false
    }
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    /// Parse `seq (| seq)*` until end-of-input or an unmatched `)`.
    fn parse_alternation(&mut self) -> Option<Vec<Vec<Node>>> {
        let mut alts = Vec::new();
        alts.push(self.parse_sequence()?);
        while self.peek() == Some('|') {
            self.pos += 1;
            alts.push(self.parse_sequence()?);
        }
        Some(alts)
    }

    /// Parse a sequence of quantified atoms until `|`, `)`, or end.
    fn parse_sequence(&mut self) -> Option<Vec<Node>> {
        let mut nodes = Vec::new();
        loop {
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => {}
            }
            let atom = self.parse_atom()?;
            let node = self.maybe_quantifier(atom)?;
            nodes.push(node);
        }
        Some(nodes)
    }

    /// Apply a trailing `?`/`*`/`+`/`{n}`/`{n,m}` quantifier to `atom`, if present.
    fn maybe_quantifier(&mut self, atom: Node) -> Option<Node> {
        let (min, max) = match self.peek() {
            Some('?') => {
                self.pos += 1;
                (0, 1)
            }
            Some('*') => {
                self.pos += 1;
                (0, usize::MAX)
            }
            Some('+') => {
                self.pos += 1;
                (1, usize::MAX)
            }
            Some('{') => {
                if let Some(bounds) = self.parse_brace_quantifier() {
                    bounds
                } else {
                    return Some(atom);
                }
            }
            _ => return Some(atom),
        };
        Some(Node::Quant { inner: Box::new(atom), min, max })
    }

    /// Parse `{n}` or `{n,m}` (or `{n,}`). Returns `None` (and does not advance) if it is not a
    /// well-formed brace quantifier — then `{` is treated as a literal by the caller.
    fn parse_brace_quantifier(&mut self) -> Option<(usize, usize)> {
        let save = self.pos;
        self.pos += 1; // consume '{'
        let mut min_str = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                min_str.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        if min_str.is_empty() {
            self.pos = save;
            return None;
        }
        let min: usize = min_str.parse().ok()?;
        let max = if self.peek() == Some(',') {
            self.pos += 1;
            let mut max_str = String::new();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    max_str.push(c);
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if max_str.is_empty() {
                usize::MAX
            } else {
                max_str.parse().ok()?
            }
        } else {
            min
        };
        if self.peek() == Some('}') {
            self.pos += 1;
            Some((min, max))
        } else {
            self.pos = save;
            None
        }
    }

    fn parse_atom(&mut self) -> Option<Node> {
        let c = self.bump()?;
        match c {
            '^' => Some(Node::Start),
            '.' => Some(Node::Any),
            '\\' => self.parse_escape(),
            '[' => self.parse_class(),
            '(' => self.parse_group(),
            other => Some(Node::Char(other)),
        }
    }

    fn parse_escape(&mut self) -> Option<Node> {
        let c = self.bump()?;
        Some(match c {
            'd' => Node::Digit,
            's' => Node::Space,
            // Any other escaped char is its literal self (e.g. `\(`, `\)`, `\.`, `\\`).
            other => Node::Char(other),
        })
    }

    fn parse_class(&mut self) -> Option<Node> {
        let mut negated = false;
        if self.peek() == Some('^') {
            negated = true;
            self.pos += 1;
        }
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None => return None, // unterminated class
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                _ => {}
            }
            let lo = self.class_char()?;
            // A range `a-b` (but a trailing `-` before `]` is a literal dash).
            if self.peek() == Some('-') && self.chars.get(self.pos + 1) != Some(&']') {
                self.pos += 1; // consume '-'
                let hi = self.class_char()?;
                match (lo, hi) {
                    (ClassChar::Char(a), ClassChar::Char(b)) => items.push(ClassItem::Range(a, b)),
                    _ => return None, // ranges over shorthands are not used by Pi patterns
                }
            } else {
                items.push(match lo {
                    ClassChar::Char(a) => ClassItem::Char(a),
                    ClassChar::Digit => ClassItem::Digit,
                    ClassChar::Space => ClassItem::Space,
                });
            }
        }
        Some(Node::Class { items, negated })
    }

    fn class_char(&mut self) -> Option<ClassChar> {
        let c = self.bump()?;
        if c == '\\' {
            let e = self.bump()?;
            return Some(match e {
                'd' => ClassChar::Digit,
                's' => ClassChar::Space,
                other => ClassChar::Char(other),
            });
        }
        Some(ClassChar::Char(c))
    }

    fn parse_group(&mut self) -> Option<Node> {
        // Only `(?:…)` non-capturing groups appear in Pi patterns. A bare `(` is treated as a
        // capturing group with identical match semantics (we do not capture).
        if self.peek() == Some('?') && self.chars.get(self.pos + 1) == Some(&':') {
            self.pos += 2;
        }
        let alts = self.parse_alternation()?;
        if self.peek() == Some(')') {
            self.pos += 1;
            Some(Node::Group(alts))
        } else {
            None
        }
    }
}

enum ClassChar {
    Char(char),
    Digit,
    Space,
}

fn ascii_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{0b}' | '\u{0c}')
}

fn class_matches(items: &[ClassItem], negated: bool, c: char) -> bool {
    let lc = c.to_ascii_lowercase();
    let mut hit = false;
    for item in items {
        let m = match item {
            ClassItem::Char(a) => a.to_ascii_lowercase() == lc,
            ClassItem::Range(a, b) => {
                // Case-insensitive range membership: test both cases of `c`.
                (*a..=*b).contains(&c)
                    || (*a..=*b).contains(&c.to_ascii_lowercase())
                    || (*a..=*b).contains(&c.to_ascii_uppercase())
            }
            ClassItem::Digit => c.is_ascii_digit(),
            ClassItem::Space => ascii_space(c),
        };
        if m {
            hit = true;
            break;
        }
    }
    hit ^ negated
}

/// Does a single atomic `node` match the char at `i`? Returns the position(s) after a match.
fn match_one(node: &Node, chars: &[char], i: usize) -> Vec<usize> {
    match node {
        Node::Start => {
            if i == 0 {
                vec![0]
            } else {
                Vec::new()
            }
        }
        Node::Group(alts) => {
            let mut out = Vec::new();
            for alt in alts {
                for end in match_nodes(alt, chars, i) {
                    if !out.contains(&end) {
                        out.push(end);
                    }
                }
            }
            out
        }
        Node::Quant { inner, min, max } => match_repeat(inner, chars, i, *min, *max),
        atomic => match chars.get(i) {
            Some(&c) if atom_char_matches(atomic, c) => vec![i + 1],
            _ => Vec::new(),
        },
    }
}

fn atom_char_matches(node: &Node, c: char) -> bool {
    match node {
        Node::Char(a) => a.eq_ignore_ascii_case(&c),
        Node::Any => c != '\n',
        Node::Digit => c.is_ascii_digit(),
        Node::Space => ascii_space(c),
        Node::Class { items, negated } => class_matches(items, *negated, c),
        _ => false,
    }
}

/// Match `inner` repeated between `min` and `max` times; return every reachable end position.
fn match_repeat(inner: &Node, chars: &[char], i: usize, min: usize, max: usize) -> Vec<usize> {
    let mut results = Vec::new();
    if min == 0 {
        results.push(i);
    }
    let mut frontier = vec![i];
    let mut seen = vec![i];
    let mut count = 0usize;
    let cap = max.min(chars.len() + 1);
    while count < cap && !frontier.is_empty() {
        let mut next = Vec::new();
        for &p in &frontier {
            for end in match_one(inner, chars, p) {
                if end > p && !next.contains(&end) {
                    next.push(end);
                }
            }
        }
        count += 1;
        if count >= min {
            for &p in &next {
                if !results.contains(&p) {
                    results.push(p);
                }
            }
        }
        // Avoid revisiting positions already explored (guards against zero-width loops).
        next.retain(|p| !seen.contains(p));
        seen.extend(next.iter().copied());
        frontier = next;
    }
    results
}

/// Match a whole sequence of nodes starting at `i`; return every reachable end position.
fn match_nodes(nodes: &[Node], chars: &[char], i: usize) -> Vec<usize> {
    let Some((first, rest)) = nodes.split_first() else {
        return vec![i];
    };
    let mut out = Vec::new();
    for mid in match_one(first, chars, i) {
        for end in match_nodes(rest, chars, mid) {
            if !out.contains(&end) {
                out.push(end);
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn literal_case_insensitive_unanchored() {
        let re = Regex::new("prompt is too long");
        assert!(re.is_match("error: PROMPT IS TOO LONG: 213462 tokens"));
        assert!(!re.is_match("everything is fine"));
    }

    #[test]
    fn optional_any_char_dot_question() {
        // `rate.?limit` — `.?` is an optional any-char (matches "ratelimit", "rate limit",
        // "rate-limit").
        let re = Regex::new("rate.?limit");
        assert!(re.is_match("RateLimit exceeded"));
        assert!(re.is_match("rate limit exceeded"));
        assert!(re.is_match("rate-limit exceeded"));
        assert!(!re.is_match("ratXlimitX is two chars apart? no"));
        assert!(re.is_match("xrateXlimitx")); // one char between still matches via `.?`
    }

    #[test]
    fn optional_literal_question() {
        // `timed? out` — optional literal `d`.
        let re = Regex::new("timed? out");
        assert!(re.is_match("the request time out"));
        assert!(re.is_match("the request timed out"));
        assert!(!re.is_match("timeout")); // needs the space
    }

    #[test]
    fn digit_plus_and_classes() {
        let re = Regex::new("maximum prompt length is \\d+");
        assert!(re.is_match("This model's maximum prompt length is 131072 but"));
        assert!(!re.is_match("maximum prompt length is unknown"));

        let cls = Regex::new("exceeds (?:the )?maximum allowed input length of [\\d,]+ tokens?");
        assert!(cls.is_match("exceeds the maximum allowed input length of 1,024 tokens"));
        assert!(cls.is_match("exceeds maximum allowed input length of 512 token"));
    }

    #[test]
    fn anchored_start_and_groups() {
        let re = Regex::new("^4(?:00|13)\\s*(?:status code)?\\s*\\(no body\\)");
        assert!(re.is_match("400 (no body)"));
        assert!(re.is_match("413 status code (no body)"));
        assert!(!re.is_match("a 400 (no body)")); // `^` pins to start
        assert!(!re.is_match("404 (no body)"));
    }

    #[test]
    fn alternation_join() {
        let re = Regex::new("overloaded|429|service.?unavailable");
        assert!(re.is_match("Error 429: too many requests"));
        assert!(re.is_match("model overloaded"));
        assert!(re.is_match("service unavailable"));
        assert!(!re.is_match("everything nominal"));
    }

    #[test]
    fn brace_quantifier_exact() {
        let re = Regex::new("a{3}");
        assert!(re.is_match("xaaax"));
        assert!(!re.is_match("xaax"));
    }

    #[test]
    fn together_complex_pattern() {
        let re = Regex::new(
            "input \\(\\d+ tokens\\) is longer than the model'?s context length \\(\\d+ tokens\\)",
        );
        assert!(re.is_match(
            "The input (5000 tokens) is longer than the model's context length (4096 tokens)."
        ));
    }
}
