//! Model resolution: pattern matching (`provider/id`, bare id, partial/alias), the `:level`
//! thinking shorthand, per-provider defaults, scoping + cycling, and custom `models.json`
//! (arch-07 §3.6/§6.4, R-07-019…R-07-023).

use std::path::Path;

use cyrup_core::{ModelThinkingLevel, ProviderId};
use cyrup_provider::Model;

use crate::error::ConfigError;

/// Parse a thinking-level token (`off|minimal|low|medium|high|xhigh`).
pub fn parse_thinking_level(s: &str) -> Option<ModelThinkingLevel> {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" => Some(ModelThinkingLevel::Off),
        "minimal" => Some(ModelThinkingLevel::Minimal),
        "low" => Some(ModelThinkingLevel::Low),
        "medium" => Some(ModelThinkingLevel::Medium),
        "high" => Some(ModelThinkingLevel::High),
        "xhigh" => Some(ModelThinkingLevel::Xhigh),
        _ => None,
    }
}

/// A model + an optional scoped thinking level.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopedModel {
    pub model: Model,
    pub thinking_level: Option<ModelThinkingLevel>,
}

/// Outcome of parsing a model pattern (arch-07 §3.6). `warning` is surfaced, never panics.
/// Mirrors Pi's `ParsedModelResult` (`{ model, thinkingLevel, warning }`, model-resolver.ts:156-161):
/// Pi has no "ambiguous" concept — an ambiguous bare id resolves via partial matching, never errors.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedModel {
    pub model: Option<Model>,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub warning: Option<String>,
}

/// Result of resolving a reference (Pi `tryMatchModel` return: `Model | undefined`).
/// Pi never errors on ambiguity: `findExactModelReferenceMatch` returns `undefined` for a bare id
/// present on >1 provider, so resolution falls through to partial matching
/// (model-resolver.ts:90-118, 124-154).
enum Match<'a> {
    None,
    One(&'a Model),
}

/// `true` if a model id looks like a dated version (`…-YYYYMMDD`), i.e. NOT an alias.
fn is_dated(id: &str) -> bool {
    match id.rsplit('-').next() {
        Some(tail) => tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

fn is_alias(id: &str) -> bool {
    id.ends_with("-latest") || !is_dated(id)
}

/// Resolves model patterns against an available model set (R-07-019).
pub struct ModelResolver<'a> {
    available: &'a [Model],
}

impl<'a> ModelResolver<'a> {
    pub fn new(available: &'a [Model]) -> Self {
        Self { available }
    }

    fn match_reference(&self, reference: &str) -> Match<'a> {
        let reference = reference.trim();
        if reference.is_empty() {
            return Match::None;
        }
        let lower = reference.to_ascii_lowercase();

        // 1. exact provider/id (case-insensitive) — unambiguous.
        if let Some((prov, id)) = reference.split_once('/') {
            let prov = prov.to_ascii_lowercase();
            let id = id.to_ascii_lowercase();
            if let Some(m) = self.available.iter().find(|m| {
                m.provider.as_str().to_ascii_lowercase() == prov
                    && m.id.as_str().to_ascii_lowercase() == id
            }) {
                return Match::One(m);
            }
        }

        // 2. bare exact id. Pi's `findExactModelReferenceMatch` returns the model ONLY when exactly
        // one id matches; a bare id present on >1 provider returns `undefined` (it does NOT error),
        // so it falls through to partial matching below (model-resolver.ts:116-118). Likewise a
        // zero-hit exact match falls through.
        let exact: Vec<&Model> = self
            .available
            .iter()
            .filter(|m| m.id.as_str().to_ascii_lowercase() == lower)
            .collect();
        if exact.len() == 1
            && let Some(m) = exact.first()
        {
            return Match::One(m);
        }

        // 3. partial match against id or name; alias-preferred, highest-sorting on ties.
        let mut partial: Vec<&Model> = self
            .available
            .iter()
            .filter(|m| {
                m.id.as_str().to_ascii_lowercase().contains(&lower)
                    || m.name.to_ascii_lowercase().contains(&lower)
            })
            .collect();
        if partial.is_empty() {
            return Match::None;
        }
        // alias first, then highest-sorting id (descending). Pi tie-breaks with
        // `b.id.localeCompare(a.id)` (model-resolver.ts:147,151) — locale-aware UCA collation, NOT a
        // Unicode-scalar `String::cmp`. The two diverge when matched alias ids differ only by case or
        // by `-`/`_`/`.` (e.g. byte-order puts `B` < `a` and `-`(0x2d) < `.`(0x2e) < `_`(0x5f), while
        // localeCompare/UCA orders case as a tertiary weight and weights punctuation differently).
        // Reuse the same `feruca` (pure-Rust UCA) collator config proven to match Node's default
        // `localeCompare` for `cyrup-tools` `ls` (ls.rs:85-87): CLDR-root tailoring, non-ignorable
        // ("not shifted") variable handling, byte-value final tiebreak. `collate(b, a)` reproduces
        // the descending `b.localeCompare(a)`.
        let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
        partial.sort_by(|a, b| {
            let aa = is_alias(a.id.as_str());
            let ba = is_alias(b.id.as_str());
            ba.cmp(&aa)
                .then_with(|| collator.collate(b.id.as_str(), a.id.as_str()))
        });
        match partial.first() {
            Some(m) => Match::One(m),
            None => Match::None,
        }
    }

    /// Exact-then-partial lookup (Pi `tryMatchModel`, model-resolver.ts:124-154). `None` = no match.
    /// Pi never errors on an ambiguous bare id; it falls through to partial matching, which always
    /// resolves to a single (alias-preferred, highest-sorting) model.
    pub fn find_exact(&self, reference: &str) -> Option<&'a Model> {
        match self.match_reference(reference) {
            Match::One(m) => Some(m),
            Match::None => None,
        }
    }

    /// Parse a `pattern[:level]` (R-07-020). `strict` (CLI `--model`) refuses to guess on an
    /// invalid trailing token; non-strict (scope mode) warns and recurses on the prefix.
    pub fn parse_pattern(&self, pattern: &str, strict: bool) -> ParsedModel {
        // Try a full exact/partial match first (Pi `tryMatchModel`, model-resolver.ts:198-201).
        if let Match::One(m) = self.match_reference(pattern) {
            return ParsedModel {
                model: Some(m.clone()),
                thinking_level: None,
                warning: None,
            };
        }

        // No match — split on the LAST colon if present (Pi model-resolver.ts:203-211).
        let Some(idx) = pattern.rfind(':') else {
            return ParsedModel {
                model: None,
                thinking_level: None,
                warning: None,
            };
        };
        let (prefix, rest) = pattern.split_at(idx);
        let suffix = rest.get(1..).unwrap_or("");

        if let Some(level) = parse_thinking_level(suffix) {
            // Valid level — recurse on the prefix. Keep the level only when the inner parse is
            // clean (Pi `thinkingLevel: result.warning ? undefined : suffix`; :213-224). When the
            // prefix itself does not resolve, return the inner result verbatim (:224).
            let inner = self.parse_pattern(prefix, strict);
            if inner.model.is_some() {
                let thinking = if inner.warning.is_some() {
                    None
                } else {
                    Some(level)
                };
                ParsedModel {
                    model: inner.model,
                    thinking_level: thinking,
                    warning: inner.warning,
                }
            } else {
                inner
            }
        } else if strict {
            // Strict (CLI `--model`): don't guess — treat the suffix as part of the id and fail
            // (Pi :228-232).
            ParsedModel {
                model: None,
                thinking_level: None,
                warning: None,
            }
        } else {
            // Scope mode: recurse on the prefix and warn (Pi :234-244).
            let inner = self.parse_pattern(prefix, strict);
            if inner.model.is_some() {
                ParsedModel {
                    model: inner.model,
                    thinking_level: None,
                    warning: Some(format!("invalid thinking level '{suffix}'")),
                }
            } else {
                inner
            }
        }
    }

    /// Per-provider default model (R-07-021): an alias-preferred model for the provider.
    pub fn provider_default(&self, provider: &ProviderId) -> Option<&'a Model> {
        let lower = provider.as_str().to_ascii_lowercase();
        let mut candidates: Vec<&Model> = self
            .available
            .iter()
            .filter(|m| m.provider.as_str().to_ascii_lowercase() == lower)
            .collect();
        candidates.sort_by(|a, b| {
            let aa = is_alias(a.id.as_str());
            let ba = is_alias(b.id.as_str());
            ba.cmp(&aa).then_with(|| b.id.as_str().cmp(a.id.as_str()))
        });
        candidates.into_iter().next()
    }

    /// Expand scope patterns (incl. simple `*` globs) into an ordered, de-duplicated candidate set
    /// (R-07-022).
    pub fn resolve_scope(&self, patterns: &[String]) -> Vec<ScopedModel> {
        let mut out: Vec<ScopedModel> = Vec::new();
        let mut seen: Vec<(String, String)> = Vec::new();
        let push = |model: Model,
                    level: Option<ModelThinkingLevel>,
                    seen: &mut Vec<(String, String)>,
                    out: &mut Vec<ScopedModel>| {
            let key = (
                model.provider.as_str().to_string(),
                model.id.as_str().to_string(),
            );
            if !seen.contains(&key) {
                seen.push(key);
                out.push(ScopedModel {
                    model,
                    thinking_level: level,
                });
            }
        };

        for pattern in patterns {
            // Pi treats `*`, `?`, or `[` as glob characters (model-resolver.ts:264).
            if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
                // Strip an optional `:level` thinking suffix (e.g. `provider/*:high`; :266-276).
                let mut glob_pattern = pattern.as_str();
                let mut level: Option<ModelThinkingLevel> = None;
                if let Some(idx) = pattern.rfind(':') {
                    let suffix = pattern.get(idx + 1..).unwrap_or("");
                    if let Some(lvl) = parse_thinking_level(suffix) {
                        level = Some(lvl);
                        glob_pattern = pattern.get(..idx).unwrap_or(pattern);
                    }
                }
                for m in self.available.iter().filter(|m| {
                    glob_match(glob_pattern, &format!("{}/{}", m.provider, m.id))
                        || glob_match(glob_pattern, m.id.as_str())
                }) {
                    push(m.clone(), level, &mut seen, &mut out);
                }
            } else {
                let parsed = self.parse_pattern(pattern, false);
                if let Some(m) = parsed.model {
                    push(m, parsed.thinking_level, &mut seen, &mut out);
                }
            }
        }
        out
    }
}

/// `minimatch`-style glob matcher (Pi uses `minimatch(.., { nocase: true })`,
/// model-resolver.ts:282). Faithful to minimatch's path-segment semantics: `*`/`?`/`[...]` do NOT
/// cross `/` (they match within a single segment), `**` is a globstar matching zero or more whole
/// segments, and `{a,b}`/`{1..3}` brace lists and ranges are expanded before matching. Per-segment
/// it supports backslash escaping, the dot-rule (`dot:false`), and the FULL extglob set
/// `@(..)`/`?(..)`/`*(..)`/`+(..)`/`!(..)` including the negative extglob (see [`segment_matches`]).
/// A leading `!` on the whole pattern is minimatch's whole-match negation (`nonegate:false`), so
/// `!(foo)` is `!`+literal `(foo)` — the "standalone `!()` quirk" — not a negative extglob.
/// Case-insensitive. No external dep. Verified byte-for-byte against Pi's `minimatch` on a large
/// captured table (see `glob_matches_pi_minimatch_byte_for_byte`).
fn glob_match(pattern: &str, text: &str) -> bool {
    let pat_lower = pattern.to_ascii_lowercase();
    let text_lower = text.to_ascii_lowercase();
    // minimatch `parseNegate` (default `nonegate:false`): strip a run of leading `!` from the WHOLE
    // pattern; an odd count negates the final result. This runs before brace-expansion/segmenting,
    // so a leading `!(..)` is negation + a literal `(..)` group, never a negative extglob.
    let raw: Vec<char> = pat_lower.chars().collect();
    let mut neg_off = 0usize;
    while raw.get(neg_off).copied() == Some('!') {
        neg_off += 1;
    }
    let negate = neg_off % 2 == 1;
    let stripped: String = raw.get(neg_off..).unwrap_or(&[]).iter().collect();
    let text_parts: Vec<&str> = text_lower.split('/').collect();
    let any_hit = brace_expand(&stripped).into_iter().any(|expanded| {
        let pat_parts: Vec<&str> = expanded.split('/').collect();
        match_segments(&pat_parts, &text_parts)
    });
    negate ^ any_hit
}

/// Match a path-segment-split glob against a path-segment-split text. A `**` segment is a globstar
/// (matches zero or more whole segments); every other segment matches exactly one text segment via
/// [`glob_match_chars`] (which never crosses `/`).
fn match_segments(pat: &[&str], text: &[&str]) -> bool {
    match pat.split_first() {
        None => text.is_empty(),
        Some((&first, rest)) => {
            if first == "**" {
                // Globstar: consume 0..=N whole segments, but under the dot-rule (`dot:false`) it
                // must NOT traverse a segment that begins with `.` (minimatch: `**/*` does not match
                // `.git/a`, and `**` does not match `.git`). Cap the run at the first dot-segment.
                let mut max_k = 0usize;
                while text.get(max_k).is_some_and(|s| !s.starts_with('.')) {
                    max_k += 1;
                }
                (0..=max_k).any(|k| match_segments(rest, text.get(k..).unwrap_or(&[])))
            } else {
                match text.split_first() {
                    Some((&t0, trest)) if segment_matches(first, t0) => match_segments(rest, trest),
                    _ => false,
                }
            }
        }
    }
}

/// Match a single glob segment (no `/`) against a single text segment, case already folded.
///
/// Implements minimatch's per-segment semantics (default options, the `{ nocase: true }` call at
/// model-resolver.ts:282): `*`/`?`/`[...]` wildcards, backslash escaping (`\x` = literal `x`), the
/// full extglob set `@(..)`/`?(..)`/`*(..)`/`+(..)`/`!(..)` (see [`m`]), the dot-rule (`dot:false`):
/// a text segment that begins with `.` is matched only when the pattern's first unit is a LITERAL
/// `.` — where, per minimatch's single-char bracket rule, a non-negated `[.]`/`[\.]` counts as a
/// literal `.` (the `[.]`-at-segment-start dot-bracket corner); and the `justDots`/`needNoTrav`
/// rule: the literal segments `.`/`..` are matched ONLY by a purely-literal pattern equal to them.
fn segment_matches(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // `justDots`/`needNoTrav`: `.`/`..` text segments only match a purely-literal pattern == text.
    if text == "." || text == ".." {
        return as_pure_literal(&p).as_deref() == Some(text);
    }
    // Dot-rule (minimatch default `dot:false`).
    if t.first() == Some(&'.') && !leading_matches_literal_dot(&p) {
        return false;
    }
    m(&p, &t, 0, &K::End, &K::End)
}

/// Whether the segment pattern's first construct can match a leading `.` under `dot:false`: a
/// literal `.` (bare or escaped), or — per minimatch's `parseClass` single-char rule — a non-negated
/// single-char class `[.]`/`[\.]` (which compiles to the literal `\.`, so its regex does not start
/// with `[` and thus is not blocked by `startNoDot`). This is the `[.]`-at-segment-start corner.
fn leading_matches_literal_dot(p: &[char]) -> bool {
    match p.first().copied() {
        Some('.') => true,
        Some('\\') => p.get(1).copied() == Some('.'),
        Some('[') => single_char_class_literal(p) == Some('.'),
        _ => false,
    }
}

/// If `p` begins with a non-negated bracket class that (per minimatch's `parseClass` single-char
/// shortcut) reduces to a single literal character (`[x]`, `[\x]`), return that character.
fn single_char_class_literal(p: &[char]) -> Option<char> {
    if p.first().copied() != Some('[') || matches!(p.get(1).copied(), Some('!') | Some('^')) {
        return None;
    }
    match p.get(1).copied() {
        // `[\X]` — escaped single member.
        Some('\\') if p.get(3).copied() == Some(']') => p.get(2).copied(),
        Some('\\') => None,
        // `[X]` — a bare single member (a leading `]` is itself a literal member, so `[]]` -> ']').
        Some(c) if p.get(2).copied() == Some(']') => Some(c),
        _ => None,
    }
}

/// If the pattern segment is purely literal (every unit is a fixed char: a bare literal, a `\x`
/// escape, or a single-char class `[x]`/`[\x]`), return that literal string; else `None`. Used for
/// the `justDots` rule (`.`/`..` only match an exactly-equal literal pattern).
fn as_pure_literal(p: &[char]) -> Option<String> {
    let mut out = String::new();
    let mut i = 0usize;
    while let Some(c) = p.get(i).copied() {
        match c {
            '\\' => {
                out.push(p.get(i + 1).copied()?);
                i += 2;
            }
            '[' => {
                out.push(single_char_class_literal(p.get(i..).unwrap_or(&[]))?);
                i += if p.get(i + 1).copied() == Some('\\') { 4 } else { 3 };
            }
            '*' | '?' => return None,
            '@' | '+' | '!' if p.get(i + 1).copied() == Some('(') => return None,
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    Some(out)
}

/// Continuation for the CPS per-segment matcher. Two continuations are threaded: the ordinary
/// consume-continuation `k` (what to match after the current construct), and the negation-tail `nt`
/// (minimatch `fillNegs`: the pattern following the innermost enclosing group, used to bind a
/// `!(..)`'s negative lookahead to the segment end). They coincide everywhere except inside a
/// repeating extglob (`*`/`+`), where `k` loops but `nt` does not.
enum K<'a> {
    /// Require the whole segment to be consumed (regex `$`, i.e. `(?:$|/)` within a segment).
    End,
    /// Match pattern slice `p`, then continue with `next`.
    Pat(&'a [char], &'a K<'a>),
    /// Enter a `*(alts)rest`/`+(alts)` star: match `rest` then `outer`, or an alt then loop. `nt`
    /// is the negation-tail that applies AFTER the whole repetition (never the loop itself).
    StarEntry {
        alts: &'a [Vec<char>],
        rest: &'a [char],
        outer: &'a K<'a>,
        nt: &'a K<'a>,
    },
    /// A star iteration boundary: only continue looping if the position advanced past `start`.
    StarLoop {
        alts: &'a [Vec<char>],
        rest: &'a [char],
        outer: &'a K<'a>,
        nt: &'a K<'a>,
        start: usize,
    },
}

fn run(k: &K, t: &[char], pos: usize) -> bool {
    match k {
        K::End => pos == t.len(),
        K::Pat(p, next) => m(p, t, pos, next, next),
        K::StarEntry {
            alts,
            rest,
            outer,
            nt,
        } => star_entry(alts, rest, outer, nt, t, pos),
        K::StarLoop {
            alts,
            rest,
            outer,
            nt,
            start,
        } => pos > *start && star_entry(alts, rest, outer, nt, t, pos),
    }
}

fn star_entry<'a>(
    alts: &'a [Vec<char>],
    rest: &'a [char],
    outer: &'a K<'a>,
    nt: &'a K<'a>,
    t: &[char],
    pos: usize,
) -> bool {
    // Zero more repetitions: match the in-group siblings `rest`, then the outer continuation.
    if m(rest, t, pos, outer, nt) {
        return true;
    }
    // One more repetition: an alt (which must advance the position), then loop.
    let neg_inside = K::Pat(rest, nt);
    alts.iter().any(|alt| {
        m(
            alt,
            t,
            pos,
            &K::StarLoop {
                alts,
                rest,
                outer,
                nt,
                start: pos,
            },
            &neg_inside,
        )
    })
}

/// Match pattern `p` starting at position `pos` in segment `t`; on success continue via `k`. `nt`
/// carries the negation-tail (see [`K`]) used by any `!(..)` encountered at this level.
fn m(p: &[char], t: &[char], pos: usize, k: &K, nt: &K) -> bool {
    let Some(c0) = p.first().copied() else {
        return run(k, t, pos);
    };
    // Extglob lead: `@(` / `?(` / `*(` / `+(` / `!(`.
    if matches!(c0, '@' | '?' | '*' | '+' | '!') && p.get(1).copied() == Some('(') {
        return ext(c0, p, t, pos, k, nt);
    }
    let p_tail = p.get(1..).unwrap_or(&[]);
    match c0 {
        // `\x` -> literal `x`. A trailing `\` (no following char) cannot match.
        '\\' => match p.get(1).copied() {
            Some(lit) => {
                t.get(pos) == Some(&lit) && m(p.get(2..).unwrap_or(&[]), t, pos + 1, k, nt)
            }
            None => false,
        },
        // `*` = `[^/]*` (any chars within a segment; greedy/non-greedy is irrelevant to booleans).
        '*' => (pos..=t.len()).any(|p2| m(p_tail, t, p2, k, nt)),
        '?' => pos < t.len() && m(p_tail, t, pos + 1, k, nt),
        '[' => match_class(p, t, pos, k, nt),
        lit => t.get(pos) == Some(&lit) && m(p_tail, t, pos + 1, k, nt),
    }
}

/// Match an extglob `K(alt|alt|..)rest` at the head of `p` against `t[pos..]`, where
/// `K ∈ {@,?,*,+,!}`. Mirrors minimatch's extglob-to-regex semantics, including the `!` negative
/// extglob (`(?:(?!(?:alt·tail(?:$|/)))[^/]*?)`) whose lookahead binds to the segment end via `nt`.
fn ext(kind: char, p: &[char], t: &[char], pos: usize, k: &K, nt: &K) -> bool {
    // Find the matching `)` (the `(` is at p[1]); track nesting.
    let mut depth = 0usize;
    let mut close = None;
    let mut i = 1usize;
    while let Some(&c) = p.get(i) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let Some(close) = close else {
        // Unterminated `K(` -> match the lead char literally.
        return t.get(pos) == Some(&kind) && m(p.get(1..).unwrap_or(&[]), t, pos + 1, k, nt);
    };
    let body = p.get(2..close).unwrap_or(&[]);
    let rest = p.get(close + 1..).unwrap_or(&[]);
    let alts = split_top_alternatives(body);
    // Negation-tail seen from INSIDE this group: the in-group siblings `rest`, then the tail after
    // the whole group (`fillNegs`).
    let neg_inner = K::Pat(rest, nt);
    match kind {
        // Exactly one alternative, then `rest`.
        '@' => alts
            .iter()
            .any(|alt| m(alt, t, pos, &K::Pat(rest, k), &neg_inner)),
        // Zero or one.
        '?' => {
            m(rest, t, pos, k, nt)
                || alts
                    .iter()
                    .any(|alt| m(alt, t, pos, &K::Pat(rest, k), &neg_inner))
        }
        // Zero or more.
        '*' => star_entry(&alts, rest, k, nt, t, pos),
        // One or more = one alternative, then zero-or-more.
        '+' => alts.iter().any(|alt| {
            m(
                alt,
                t,
                pos,
                &K::StarEntry {
                    alts: &alts,
                    rest,
                    outer: k,
                    nt,
                },
                &neg_inner,
            )
        }),
        // Negative extglob. Lookahead: NO alternative followed by the in-group siblings `rest` and
        // the negation-tail `nt` reaches the segment end (minimatch `fillNegs`'s `alt·tail(?:$|/)`).
        // Otherwise consume `[^/]*?` (any prefix, no `/` in-segment) then `rest`, then `k`.
        '!' => {
            let looka = alts
                .iter()
                .any(|alt| m(alt, t, pos, &neg_inner, &neg_inner));
            if looka {
                return false;
            }
            (pos..=t.len()).any(|p2| m(rest, t, p2, k, nt))
        }
        _ => false,
    }
}

/// Match a `[...]` class at the head of `p` against `t[pos]`, then continue via `k`/`nt`. Handles
/// negation (`[!..]`/`[^..]`), a leading `]` as a literal member, ranges, `\x` escaped members, and
/// an unterminated `[` as a literal `[`.
fn match_class(p: &[char], t: &[char], pos: usize, k: &K, nt: &K) -> bool {
    let mut j = 1;
    let negate = matches!(p.get(j).copied(), Some('!') | Some('^'));
    if negate {
        j += 1;
    }
    let class_start = j;
    while let Some(cur) = p.get(j).copied() {
        if cur == ']' && j != class_start {
            break;
        }
        // A `\x` inside the class is a two-char literal member; the `x` (even `]`) is not a close.
        if cur == '\\' && p.get(j + 1).is_some() {
            j += 2;
            continue;
        }
        j += 1;
    }
    if p.get(j).copied() == Some(']') {
        let Some(c) = t.get(pos).copied() else {
            return false;
        };
        let inset = class_matches(p, class_start, j, c);
        inset != negate && m(p.get(j + 1..).unwrap_or(&[]), t, pos + 1, k, nt)
    } else {
        // Unterminated class -> treat `[` as a literal.
        t.get(pos) == Some(&'[') && m(p.get(1..).unwrap_or(&[]), t, pos + 1, k, nt)
    }
}

/// Split an extglob body into alternatives at the top-level `|` (respecting nested `(...)`).
fn split_top_alternatives(body: &[char]) -> Vec<Vec<char>> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, &c) in body.iter().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => {
                out.push(body.get(start..i).unwrap_or(&[]).to_vec());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(body.get(start..).unwrap_or(&[]).to_vec());
    out
}

/// Expand brace expressions (minimatch default; the `brace-expansion` package) into concrete
/// patterns. Handles top-level comma lists `{a,b,c}`, numeric/alpha RANGES `{1..3}`/`{a..c}` (with
/// optional `..step` and zero-padding), nesting, and escape-awareness (a `\{`/`\}` is literal). An
/// unbalanced or comma-less/range-less brace is left literal (matching brace-expansion's
/// "single element" behavior).
fn brace_expand(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    // First UNESCAPED `{`.
    let mut open = None;
    let mut i = 0usize;
    while let Some(&c) = chars.get(i) {
        match c {
            '\\' => i += 2,
            '{' => {
                open = Some(i);
                break;
            }
            _ => i += 1,
        }
    }
    let Some(open) = open else {
        return vec![s.to_string()];
    };
    // Find the matching `}` and the top-level (depth-1) comma positions, skipping escaped braces.
    let mut depth = 0usize;
    let mut close = None;
    let mut commas: Vec<usize> = Vec::new();
    let mut j = open;
    while let Some(&c) = chars.get(j) {
        match c {
            '\\' => {
                j += 2;
                continue;
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(j);
                    break;
                }
            }
            ',' if depth == 1 => commas.push(j),
            _ => {}
        }
        j += 1;
    }
    let Some(close) = close else {
        // Unbalanced brace — treat literally.
        return vec![s.to_string()];
    };
    let collect = |range: &[char]| -> String { range.iter().collect() };
    let pre: String = collect(chars.get(..open).unwrap_or(&[]));
    let post: String = collect(chars.get(close + 1..).unwrap_or(&[]));
    let post_expanded = brace_expand(&post);

    // The brace body (between `{` and `}`).
    let body = chars.get(open + 1..close).unwrap_or(&[]);

    // Determine this brace's expansion options: comma list, else a numeric/alpha range, else literal.
    let options: Vec<String> = if !commas.is_empty() {
        let mut opts: Vec<String> = Vec::new();
        let mut start = open + 1;
        for &c in &commas {
            opts.push(collect(chars.get(start..c).unwrap_or(&[])));
            start = c + 1;
        }
        opts.push(collect(chars.get(start..close).unwrap_or(&[])));
        opts
    } else if let Some(seq) = expand_brace_range(body) {
        seq
    } else {
        // No comma and not a range — keep this brace literal, expand only the remainder.
        let literal: String = collect(chars.get(open..=close).unwrap_or(&[]));
        return post_expanded
            .into_iter()
            .map(|tail| format!("{pre}{literal}{tail}"))
            .collect();
    };

    let mut out = Vec::new();
    for opt in options {
        for opt_expanded in brace_expand(&opt) {
            for tail in &post_expanded {
                out.push(format!("{pre}{opt_expanded}{tail}"));
            }
        }
    }
    out
}

/// Expand a brace-range body `A..B` / `A..B..S` (numeric or single-char alpha) into its sequence,
/// matching the `brace-expansion` package: inclusive bounds, reverse ranges (`{3..1}`), an optional
/// positive step, and numeric zero-padding (`{01..03}` → `01,02,03`). `None` = not a valid range.
fn expand_brace_range(body: &[char]) -> Option<Vec<String>> {
    let s: String = body.iter().collect();
    let parts: Vec<&str> = s.split("..").collect();
    if parts.len() != 2 && parts.len() != 3 {
        return None;
    }
    let (Some(&start), Some(&end)) = (parts.first(), parts.get(1)) else {
        return None;
    };
    // Numeric range.
    if let (Ok(a), Ok(b)) = (start.parse::<i64>(), end.parse::<i64>()) {
        let step = match parts.get(2) {
            Some(s) => s.parse::<i64>().ok()?.abs(),
            None => 1,
        };
        let step = if step == 0 { 1 } else { step };
        let padded = is_zero_padded(start) || is_zero_padded(end);
        let width = start
            .trim_start_matches('-')
            .len()
            .max(end.trim_start_matches('-').len());
        let mut out = Vec::new();
        let mut x = a;
        if a <= b {
            while x <= b {
                out.push(format_range_num(x, padded, width));
                x += step;
            }
        } else {
            while x >= b {
                out.push(format_range_num(x, padded, width));
                x -= step;
            }
        }
        return Some(out);
    }
    // Single-char alpha range.
    let sc: Vec<char> = start.chars().collect();
    let ec: Vec<char> = end.chars().collect();
    let (Some(&sc0), Some(&ec0)) = (sc.first(), ec.first()) else {
        return None;
    };
    if sc.len() == 1 && ec.len() == 1 && sc0.is_ascii_alphabetic() && ec0.is_ascii_alphabetic() {
        let step = match parts.get(2) {
            Some(s) => s.parse::<i64>().ok()?.abs(),
            None => 1,
        };
        let step = if step == 0 { 1 } else { step };
        let (a, b) = (sc0 as i64, ec0 as i64);
        let mut out = Vec::new();
        let mut x = a;
        if a <= b {
            while x <= b {
                out.push(char::from_u32(x as u32).map(String::from).unwrap_or_default());
                x += step;
            }
        } else {
            while x >= b {
                out.push(char::from_u32(x as u32).map(String::from).unwrap_or_default());
                x -= step;
            }
        }
        return Some(out);
    }
    None
}

/// Whether a numeric range bound is zero-padded (e.g. `01`, `-05`) — `brace-expansion` then pads the
/// whole sequence to the max bound width.
fn is_zero_padded(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    digits.len() > 1 && digits.starts_with('0')
}

/// Format a numeric range element, zero-padding to `width` when the range was padded.
fn format_range_num(x: i64, padded: bool, width: usize) -> String {
    if !padded {
        return x.to_string();
    }
    if x < 0 {
        format!("-{:0>w$}", -x, w = width.saturating_sub(1))
    } else {
        format!("{:0>width$}", x)
    }
}

/// Whether char `c` is in the class body `p[start..end)` (ranges like `a-z`, bare chars, and `\x`
/// escaped members — a `\x` inside `[...]` is the literal `x`, per minimatch's brace-expression
/// escaping). A `-` that is the last body char (or has no member after it) is a literal `-`.
fn class_matches(p: &[char], start: usize, end: usize, c: char) -> bool {
    let mut j = start;
    while j < end {
        let (lo, after_lo) = read_class_member(p, j, end);
        if p.get(after_lo).copied() == Some('-') && after_lo + 1 < end {
            let (hi, after_hi) = read_class_member(p, after_lo + 1, end);
            if c >= lo && c <= hi {
                return true;
            }
            j = after_hi;
        } else {
            if c == lo {
                return true;
            }
            j = after_lo;
        }
    }
    false
}

/// Read one class member at `j`: a `\x` escape yields the literal `x` (consuming 2), otherwise the
/// bare char (consuming 1). Returns the member char and the next index.
fn read_class_member(p: &[char], j: usize, end: usize) -> (char, usize) {
    if p.get(j).copied() == Some('\\') && j + 1 < end {
        (p.get(j + 1).copied().unwrap_or('\\'), j + 2)
    } else {
        (p.get(j).copied().unwrap_or('\0'), j + 1)
    }
}

/// Cursor over candidate models for Ctrl+P / Ctrl+N cycling (R-07-022).
pub struct ModelCycler {
    candidates: Vec<ScopedModel>,
    idx: usize,
}

impl ModelCycler {
    pub fn new(candidates: Vec<ScopedModel>) -> Self {
        Self { candidates, idx: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Advance to the next candidate, reporting (model, current thinking level).
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<(&Model, ModelThinkingLevel)> {
        if self.candidates.is_empty() {
            return None;
        }
        self.idx = (self.idx + 1) % self.candidates.len();
        self.current()
    }

    pub fn prev(&mut self) -> Option<(&Model, ModelThinkingLevel)> {
        if self.candidates.is_empty() {
            return None;
        }
        self.idx = (self.idx + self.candidates.len() - 1) % self.candidates.len();
        self.current()
    }

    pub fn current(&self) -> Option<(&Model, ModelThinkingLevel)> {
        self.candidates
            .get(self.idx)
            .map(|sm| (&sm.model, sm.thinking_level.unwrap_or_default()))
    }
}

/// Load custom OpenAI/Anthropic/Google-compatible model defs from a `models.json` (R-07-023).
pub fn load_custom_models(path: &Path) -> Result<Vec<Model>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ConfigError::Io(e)),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let models: Vec<Model> = serde_json::from_str(&text)?;
    Ok(models)
}

/// `true` if two models refer to the same provider+id (Pi `modelsAreEqual`, models.ts:435).
fn models_are_equal(a: &Model, b: &Model) -> bool {
    a.id == b.id && a.provider == b.provider
}

/// Curated default model id per known provider (Pi `defaultModelPerProvider`,
/// model-resolver.ts:14-50). Returns `None` for an unknown provider.
pub fn default_model_per_provider(provider: &str) -> Option<&'static str> {
    let id = match provider {
        "amazon-bedrock" => "us.anthropic.claude-opus-4-6-v1",
        "ant-ling" => "Ring-2.6-1T",
        "anthropic" => "claude-opus-4-8",
        "openai" => "gpt-5.5",
        "azure-openai-responses" => "gpt-5.4",
        "openai-codex" => "gpt-5.5",
        "nvidia" => "nvidia/nemotron-3-super-120b-a12b",
        "deepseek" => "deepseek-v4-pro",
        "google" => "gemini-3.1-pro-preview",
        "google-vertex" => "gemini-3.1-pro-preview",
        "github-copilot" => "gpt-5.4",
        "openrouter" => "moonshotai/kimi-k2.6",
        "vercel-ai-gateway" => "zai/glm-5.1",
        "xai" => "grok-4.20-0309-reasoning",
        "groq" => "openai/gpt-oss-120b",
        "cerebras" => "zai-glm-4.7",
        "zai" => "glm-5.1",
        "zai-coding-cn" => "glm-5.1",
        "mistral" => "devstral-medium-latest",
        "minimax" => "MiniMax-M2.7",
        "minimax-cn" => "MiniMax-M2.7",
        "moonshotai" => "kimi-k2.6",
        "moonshotai-cn" => "kimi-k2.6",
        "huggingface" => "moonshotai/Kimi-K2.6",
        "fireworks" => "accounts/fireworks/models/kimi-k2p6",
        "together" => "moonshotai/Kimi-K2.6",
        "opencode" => "kimi-k2.6",
        "opencode-go" => "kimi-k2.6",
        "kimi-coding" => "kimi-for-coding",
        "cloudflare-workers-ai" => "@cf/moonshotai/kimi-k2.6",
        "cloudflare-ai-gateway" => "workers-ai/@cf/moonshotai/kimi-k2.6",
        "xiaomi" => "mimo-v2.5-pro",
        "xiaomi-token-plan-cn" => "mimo-v2.5-pro",
        "xiaomi-token-plan-ams" => "mimo-v2.5-pro",
        "xiaomi-token-plan-sgp" => "mimo-v2.5-pro",
        _ => return None,
    };
    Some(id)
}

/// The ordered list of known providers, used to scan for a curated default (Pi iterates
/// `Object.keys(defaultModelPerProvider)`, model-resolver.ts:593/655).
const KNOWN_PROVIDERS: &[&str] = &[
    "amazon-bedrock",
    "ant-ling",
    "anthropic",
    "openai",
    "azure-openai-responses",
    "openai-codex",
    "nvidia",
    "deepseek",
    "google",
    "google-vertex",
    "github-copilot",
    "openrouter",
    "vercel-ai-gateway",
    "xai",
    "groq",
    "cerebras",
    "zai",
    "zai-coding-cn",
    "mistral",
    "minimax",
    "minimax-cn",
    "moonshotai",
    "moonshotai-cn",
    "huggingface",
    "fireworks",
    "together",
    "opencode",
    "opencode-go",
    "kimi-coding",
    "cloudflare-workers-ai",
    "cloudflare-ai-gateway",
    "xiaomi",
    "xiaomi-token-plan-cn",
    "xiaomi-token-plan-ams",
    "xiaomi-token-plan-sgp",
];

/// Find the first available model whose (provider, id) matches a curated default, else the first
/// available model (Pi's loop at model-resolver.ts:593-602 / 655-667).
fn first_default_or_first(available: &[Model]) -> Option<Model> {
    for provider in KNOWN_PROVIDERS {
        if let Some(default_id) = default_model_per_provider(provider)
            && let Some(m) = available
                .iter()
                .find(|m| m.provider.as_str() == *provider && m.id.as_str() == default_id)
        {
            return Some(m.clone());
        }
    }
    available.first().cloned()
}

/// Synthesize a custom model for `(provider, model_id)` by cloning the provider's curated-default
/// (or first) model and overriding id/name (Pi `buildFallbackModel`, model-resolver.ts:163-177).
pub fn build_fallback_model(provider: &str, model_id: &str, available: &[Model]) -> Option<Model> {
    let provider_models: Vec<&Model> = available
        .iter()
        .filter(|m| m.provider.as_str() == provider)
        .collect();
    let base = provider_models.first().copied()?;
    let default_id = default_model_per_provider(provider);
    let base = match default_id {
        Some(did) => provider_models
            .iter()
            .find(|m| m.id.as_str() == did)
            .copied()
            .unwrap_or(base),
        None => base,
    };
    let mut model = base.clone();
    model.id = model_id.into();
    model.name = model_id.to_string();
    Some(model)
}

/// Result of [`resolve_cli_model`] (Pi `ResolveCliModelResult`, model-resolver.ts:318-327).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub warning: Option<String>,
    /// CLI-display error; when set, `model` is `None`.
    pub error: Option<String>,
}

/// Resolve a single model from CLI flags (Pi `resolveCliModel`, model-resolver.ts:340-511).
///
/// `all` is the full model set (NOT just authed models, so `--api-key` first-time setup works).
/// `has_configured_auth` reports whether a model has usable auth (Pi `modelRegistry.hasConfiguredAuth`).
pub fn resolve_cli_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    cli_thinking: Option<ModelThinkingLevel>,
    all: &[Model],
    has_configured_auth: &dyn Fn(&Model) -> bool,
) -> CliModelResult {
    let Some(cli_model) = cli_model else {
        return CliModelResult::default();
    };

    if all.is_empty() {
        return CliModelResult {
            error: Some(
                "No models available. Check your installation or add models to models.json."
                    .to_string(),
            ),
            ..Default::default()
        };
    }

    // Canonical (case-insensitive) provider lookup.
    let canonical_provider = |name: &str| -> Option<String> {
        all.iter()
            .find(|m| m.provider.as_str().eq_ignore_ascii_case(name))
            .map(|m| m.provider.as_str().to_string())
    };

    let mut provider = cli_provider.and_then(canonical_provider);
    if cli_provider.is_some() && provider.is_none() {
        return CliModelResult {
            error: Some(format!(
                "Unknown provider \"{}\". Use --list-models to see available providers/models.",
                cli_provider.unwrap_or("")
            )),
            ..Default::default()
        };
    }

    let mut pattern = cli_model.to_string();
    let mut inferred_provider = false;

    // Infer `provider/model` when the prefix matches a known provider.
    if provider.is_none()
        && let Some(slash) = cli_model.find('/')
    {
        let maybe = &cli_model[..slash];
        if let Some(canonical) = canonical_provider(maybe) {
            provider = Some(canonical);
            pattern = cli_model[slash + 1..].to_string();
            inferred_provider = true;
        }
    }

    // No provider inferred: try exact id / provider/id match across all models.
    if provider.is_none() {
        let lower = cli_model.to_ascii_lowercase();
        if let Some(exact) = all.iter().find(|m| {
            m.id.as_str().to_ascii_lowercase() == lower
                || format!("{}/{}", m.provider, m.id).to_ascii_lowercase() == lower
        }) {
            return CliModelResult {
                model: Some(exact.clone()),
                ..Default::default()
            };
        }
    }

    // Both --provider and --model <provider>/<pattern>: strip the redundant prefix.
    if let (Some(cp), Some(p)) = (cli_provider, provider.as_deref()) {
        let _ = cp;
        let prefix = format!("{p}/");
        if cli_model
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
        {
            pattern = cli_model[prefix.len()..].to_string();
        }
    }

    let candidates: Vec<Model> = match provider.as_deref() {
        Some(p) => all
            .iter()
            .filter(|m| m.provider.as_str() == p)
            .cloned()
            .collect(),
        None => all.to_vec(),
    };
    let resolver = ModelResolver::new(&candidates);
    let parsed = resolver.parse_pattern(&pattern, true);

    if let Some(model) = parsed.model.clone() {
        // Provider inference matched an unauthenticated pair: prefer an authed raw id match.
        if inferred_provider {
            let raw_exact: Vec<&Model> = all
                .iter()
                .filter(|m| {
                    m.id.as_str().eq_ignore_ascii_case(cli_model) && !models_are_equal(m, &model)
                })
                .collect();
            if !raw_exact.is_empty() && !has_configured_auth(&model) {
                let authed: Vec<&Model> = raw_exact
                    .into_iter()
                    .filter(|m| has_configured_auth(m))
                    .collect();
                if authed.len() == 1
                    && let Some(m) = authed.first()
                {
                    return CliModelResult {
                        model: Some((*m).clone()),
                        ..Default::default()
                    };
                }
            }
        }
        return CliModelResult {
            model: Some(model),
            thinking_level: parsed.thinking_level,
            warning: parsed.warning,
            error: None,
        };
    }

    // Inferred a provider but no match within it: fall back to a raw id match across all models.
    if inferred_provider {
        let lower = cli_model.to_ascii_lowercase();
        if let Some(exact) = all.iter().find(|m| {
            m.id.as_str().to_ascii_lowercase() == lower
                || format!("{}/{}", m.provider, m.id).to_ascii_lowercase() == lower
        }) {
            return CliModelResult {
                model: Some(exact.clone()),
                ..Default::default()
            };
        }
        let fallback = ModelResolver::new(all).parse_pattern(cli_model, true);
        if let Some(m) = fallback.model {
            return CliModelResult {
                model: Some(m),
                thinking_level: fallback.thinking_level,
                warning: fallback.warning,
                error: None,
            };
        }
    }

    if let Some(p) = provider.as_deref() {
        // Parse a `:level` suffix from the pattern before building the fallback model.
        let mut fallback_pattern = pattern.clone();
        let mut fallback_thinking: Option<ModelThinkingLevel> = None;
        if cli_thinking.is_none()
            && let Some(idx) = pattern.rfind(':')
        {
            let suffix = pattern.get(idx + 1..).unwrap_or("");
            if let Some(lvl) = parse_thinking_level(suffix) {
                fallback_pattern = pattern.get(..idx).unwrap_or(&pattern).to_string();
                fallback_thinking = Some(lvl);
            }
        }
        if let Some(mut fallback_model) = build_fallback_model(p, &fallback_pattern, all) {
            let requested = cli_thinking.or(fallback_thinking);
            if matches!(requested, Some(l) if l.is_on()) {
                fallback_model.reasoning = true;
            }
            let base_warn = format!(
                "Model \"{fallback_pattern}\" not found for provider \"{p}\". Using custom model id."
            );
            let warning = match parsed.warning {
                Some(w) => format!("{w} {base_warn}"),
                None => base_warn,
            };
            return CliModelResult {
                model: Some(fallback_model),
                thinking_level: fallback_thinking,
                warning: Some(warning),
                error: None,
            };
        }
    }

    let display = match provider.as_deref() {
        Some(p) => format!("{p}/{pattern}"),
        None => cli_model.to_string(),
    };
    CliModelResult {
        model: None,
        thinking_level: None,
        warning: parsed.warning,
        error: Some(format!(
            "Model \"{display}\" not found. Use --list-models to see available models."
        )),
    }
}

/// Result of [`find_initial_model`] (Pi `InitialModelResult`, model-resolver.ts:513-517).
#[derive(Clone, Debug, PartialEq)]
pub struct InitialModelResult {
    pub model: Option<Model>,
    pub thinking_level: ModelThinkingLevel,
    pub fallback_message: Option<String>,
    /// A CLI error surfaced from step 1 (`resolve_cli_model`). Pi calls `process.exit(1)`; the bin
    /// owns process exit, so we propagate the message instead (additive, non-panicking).
    pub error: Option<String>,
}

/// Find the initial model by priority (Pi `findInitialModel`, model-resolver.ts:527-607):
/// 1) CLI provider+model, 2) first scoped (unless continuing), 3) saved settings default,
/// 4) first available model matching a curated default (else first available), 5) none.
#[allow(clippy::too_many_arguments)]
pub fn find_initial_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    scoped_models: &[ScopedModel],
    is_continuing: bool,
    default_provider: Option<&str>,
    default_model_id: Option<&str>,
    default_thinking_level: Option<ModelThinkingLevel>,
    all: &[Model],
    available: &[Model],
    has_configured_auth: &dyn Fn(&Model) -> bool,
) -> InitialModelResult {
    let default_level = ModelThinkingLevel::default();

    // 1. CLI args take priority.
    if let (Some(_), Some(_)) = (cli_provider, cli_model) {
        let resolved = resolve_cli_model(cli_provider, cli_model, None, all, has_configured_auth);
        if let Some(err) = resolved.error {
            return InitialModelResult {
                model: None,
                thinking_level: default_level,
                fallback_message: None,
                error: Some(err),
            };
        }
        if let Some(model) = resolved.model {
            return InitialModelResult {
                model: Some(model),
                thinking_level: default_level,
                fallback_message: None,
                error: None,
            };
        }
    }

    // 2. First scoped model (unless continuing/resuming a session).
    if let Some(first) = scoped_models.first()
        && !is_continuing
    {
        return InitialModelResult {
            model: Some(first.model.clone()),
            thinking_level: first
                .thinking_level
                .or(default_thinking_level)
                .unwrap_or(default_level),
            fallback_message: None,
            error: None,
        };
    }

    // 3. Saved default from settings.
    if let (Some(dp), Some(dm)) = (default_provider, default_model_id)
        && let Some(found) = all
            .iter()
            .find(|m| m.provider.as_str() == dp && m.id.as_str() == dm)
    {
        return InitialModelResult {
            model: Some(found.clone()),
            thinking_level: default_thinking_level.unwrap_or(default_level),
            fallback_message: None,
            error: None,
        };
    }

    // 4. First available model with valid auth (curated-default first).
    if let Some(model) = first_default_or_first(available) {
        return InitialModelResult {
            model: Some(model),
            thinking_level: default_level,
            fallback_message: None,
            error: None,
        };
    }

    // 5. No model.
    InitialModelResult {
        model: None,
        thinking_level: default_level,
        fallback_message: None,
        error: None,
    }
}

/// Result of [`restore_model_from_session`] (Pi `restoreModelFromSession` return,
/// model-resolver.ts:612-681).
#[derive(Clone, Debug, PartialEq)]
pub struct RestoredModelResult {
    pub model: Option<Model>,
    pub fallback_message: Option<String>,
}

/// Restore a model saved in a session, re-checking auth and falling back (Pi
/// `restoreModelFromSession`, model-resolver.ts:612-681). The console messaging is a front-end
/// concern; only the model + fallback message are returned.
pub fn restore_model_from_session(
    saved_provider: &str,
    saved_model_id: &str,
    current_model: Option<&Model>,
    all: &[Model],
    available: &[Model],
    has_configured_auth: &dyn Fn(&Model) -> bool,
) -> RestoredModelResult {
    let restored = all
        .iter()
        .find(|m| m.provider.as_str() == saved_provider && m.id.as_str() == saved_model_id);
    let restored_has_auth = restored.is_some_and(has_configured_auth);

    if let Some(model) = restored
        && restored_has_auth
    {
        return RestoredModelResult {
            model: Some(model.clone()),
            fallback_message: None,
        };
    }

    let reason = if restored.is_none() {
        "model no longer exists"
    } else {
        "no auth configured"
    };

    if let Some(current) = current_model {
        return RestoredModelResult {
            model: Some(current.clone()),
            fallback_message: Some(format!(
                "Could not restore model {saved_provider}/{saved_model_id} ({reason}). Using {}/{}.",
                current.provider, current.id
            )),
        };
    }

    if let Some(fallback) = first_default_or_first(available) {
        let msg = format!(
            "Could not restore model {saved_provider}/{saved_model_id} ({reason}). Using {}/{}.",
            fallback.provider, fallback.id
        );
        return RestoredModelResult {
            model: Some(fallback),
            fallback_message: Some(msg),
        };
    }

    RestoredModelResult {
        model: None,
        fallback_message: None,
    }
}

/// A `models.json` provider request config (Pi `ProviderConfigSchema`, model-registry.ts:204-214):
/// the request-auth-relevant fields. `apiKey`/`headers` carry unresolved config-value templates;
/// resolve them with [`ProviderConfig::resolve_request_auth`].
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub auth_header: Option<bool>,
    /// Inline model definitions (preserved verbatim; full parsing lives in the model registry).
    #[serde(default)]
    pub models: Vec<serde_json::Value>,
}

/// Resolved request auth for a provider (Pi `ResolvedRequestAuth` ok-branch,
/// model-registry.ts:249-259).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedRequestAuth {
    pub api_key: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub auth_header: Option<bool>,
}

impl ProviderConfig {
    /// Resolve `apiKey` + `headers` through the config-value language (Pi
    /// `getApiKeyAndHeaders`/`resolveHeadersOrThrow`, model-registry.ts:659-736). `env` is an
    /// optional provider-scoped override map. Returns an error string on an unresolvable template.
    pub fn resolve_request_auth(
        &self,
        env: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<ResolvedRequestAuth, String> {
        let api_key = match &self.api_key {
            Some(raw) => Some(crate::config_value::resolve_config_value_or_throw(
                raw, "API key", env,
            )?),
            None => None,
        };
        let headers = match &self.headers {
            Some(map) => {
                let owned: std::collections::HashMap<String, String> =
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                crate::config_value::resolve_headers_or_throw(Some(&owned), "provider", env)?
            }
            None => None,
        };
        Ok(ResolvedRequestAuth {
            api_key,
            headers,
            auth_header: self.auth_header,
        })
    }
}

/// A parsed `models.json` in Pi's `{ providers: { <name>: ProviderConfig } }` shape
/// (model-registry.ts:216-218).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct ModelFile {
    #[serde(default)]
    pub providers: std::collections::BTreeMap<String, ProviderConfig>,
}

/// Load a `models.json` provider-config file (Pi's `{ providers: {...} }` shape). A missing or
/// empty file yields an empty [`ModelFile`]. This is additive alongside [`load_custom_models`]
/// (which reads the legacy flat `Vec<Model>` shape) so existing consumers are unaffected.
pub fn load_models_file(path: &Path) -> Result<ModelFile, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ModelFile::default()),
        Err(e) => return Err(ConfigError::Io(e)),
    };
    if text.trim().is_empty() {
        return Ok(ModelFile::default());
    }
    let file: ModelFile = serde_json::from_str(&text)?;
    Ok(file)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use cyrup_provider::{ApiId, Modality, ModelCost};

    fn model(provider: &str, id: &str, name: &str) -> Model {
        Model {
            id: id.into(),
            name: name.to_string(),
            api: ApiId::from("anthropic-messages"),
            provider: provider.into(),
            base_url: String::new(),
            reasoning: true,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 200_000,
            max_tokens: 8192,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    #[test]
    fn thinking_shorthand_resolves_model_and_level() {
        // A-07-6: claude-opus:high
        let models = vec![model("anthropic", "claude-opus-4-latest", "Claude Opus 4")];
        let r = ModelResolver::new(&models);
        let parsed = r.parse_pattern("claude-opus:high", true);
        assert_eq!(
            parsed.model.as_ref().unwrap().id.as_str(),
            "claude-opus-4-latest"
        );
        assert_eq!(parsed.thinking_level, Some(ModelThinkingLevel::High));
    }

    #[test]
    fn ambiguous_bare_id_resolves_via_partial_like_pi() {
        // Pi never errors on an ambiguous bare id: `findExactModelReferenceMatch` returns
        // `undefined` for an id on >1 provider (model-resolver.ts:116-118), so `tryMatchModel`
        // falls through to partial matching, which always yields a single model
        // (alias-preferred, then `b.id.localeCompare(a.id)` descending → first in original order
        // on ties). Ground truth derived from Pi: both "shared" ids are aliases and equal, so the
        // first-listed (provider "a") wins.
        let models = vec![
            model("a", "shared", "A Shared"),
            model("b", "shared", "B Shared"),
        ];
        let r = ModelResolver::new(&models);
        // find_exact (Pi `tryMatchModel`) resolves to provider a, never erroring.
        let found = r.find_exact("shared").expect("ambiguous bare id resolves, never errors");
        assert_eq!(found.provider.as_str(), "a");
        // parse_pattern likewise resolves (no warning, a concrete model).
        let parsed = r.parse_pattern("shared", true);
        assert_eq!(parsed.model.as_ref().unwrap().provider.as_str(), "a");
        assert!(parsed.warning.is_none());
        // A realistic Pi case: `kimi-k2.6` is shared by moonshotai/moonshotai-cn/opencode/
        // opencode-go. Pi yields exactly 1 cycle entry (moonshotai, first-listed); the old crate
        // yielded 0 by erroring. Assert resolve_scope now returns 1.
        let shared_kimi = vec![
            model("moonshotai", "kimi-k2.6", "Kimi"),
            model("moonshotai-cn", "kimi-k2.6", "Kimi CN"),
            model("opencode", "kimi-k2.6", "Kimi OC"),
            model("opencode-go", "kimi-k2.6", "Kimi OCG"),
        ];
        let r = ModelResolver::new(&shared_kimi);
        let scoped = r.resolve_scope(&["kimi-k2.6".to_string()]);
        assert_eq!(scoped.len(), 1, "Pi resolves an ambiguous bare id to 1 model");
        assert_eq!(
            scoped.first().unwrap().model.provider.as_str(),
            "moonshotai"
        );
    }

    #[test]
    fn latest_preferred_over_dated() {
        // A-07-6: -latest preferred over a dated alias.
        let models = vec![
            model("anthropic", "claude-3-5-sonnet-20241022", "Sonnet dated"),
            model("anthropic", "claude-3-5-sonnet-latest", "Sonnet latest"),
        ];
        let r = ModelResolver::new(&models);
        let parsed = r.parse_pattern("claude-3-5-sonnet", false);
        assert_eq!(
            parsed.model.as_ref().unwrap().id.as_str(),
            "claude-3-5-sonnet-latest"
        );
    }

    #[test]
    fn exact_provider_id_case_insensitive() {
        let models = vec![model("OpenAI", "GPT-4o", "GPT-4o")];
        let r = ModelResolver::new(&models);
        let m = r.find_exact("openai/gpt-4o").unwrap();
        assert_eq!(m.id.as_str(), "GPT-4o");
    }

    #[test]
    fn colon_in_id_is_handled() {
        let models = vec![model("openai", "gpt-4o:extended", "GPT extended")];
        let r = ModelResolver::new(&models);
        // exact match on an id that contains a colon
        let parsed = r.parse_pattern("openai/gpt-4o:extended", true);
        assert_eq!(
            parsed.model.as_ref().unwrap().id.as_str(),
            "gpt-4o:extended"
        );
        assert_eq!(parsed.thinking_level, None);
    }

    #[test]
    fn invalid_thinking_level_strict_vs_scope() {
        let models = vec![model("anthropic", "claude-opus-latest", "Opus")];
        let r = ModelResolver::new(&models);
        // strict: don't guess
        let strict = r.parse_pattern("claude-opus:bogus", true);
        assert!(strict.model.is_none());
        // scope: warn + recurse on prefix
        let scope = r.parse_pattern("claude-opus:bogus", false);
        assert!(scope.model.is_some());
        assert!(scope.warning.is_some());
        assert_eq!(scope.thinking_level, None);
    }

    #[test]
    fn provider_default_picks_alias() {
        let models = vec![
            model("anthropic", "claude-3-5-sonnet-20241022", "dated"),
            model("anthropic", "claude-3-5-sonnet-latest", "latest"),
        ];
        let r = ModelResolver::new(&models);
        let d = r.provider_default(&ProviderId::from("anthropic")).unwrap();
        assert_eq!(d.id.as_str(), "claude-3-5-sonnet-latest");
    }

    #[test]
    fn scope_and_cycle() {
        // R-07-022
        let models = vec![
            model("anthropic", "claude-opus-latest", "Opus"),
            model("anthropic", "claude-haiku-latest", "Haiku"),
            model("openai", "gpt-4o", "GPT-4o"),
        ];
        let r = ModelResolver::new(&models);
        let scoped = r.resolve_scope(&["anthropic/*".to_string()]);
        assert_eq!(scoped.len(), 2);
        let mut cycler = ModelCycler::new(scoped);
        let (m1, _) = cycler.current().unwrap();
        let id1 = m1.id.as_str().to_string();
        let (m2, lvl) = cycler.next().unwrap();
        assert_ne!(m2.id.as_str(), id1);
        assert_eq!(lvl, ModelThinkingLevel::Off);
        // wraps around
        cycler.next();
        let (m_wrap, _) = cycler.current().unwrap();
        assert_eq!(m_wrap.id.as_str(), id1);
    }

    #[test]
    fn default_model_table_matches_pi() {
        // model-resolver.ts:14-50
        assert_eq!(
            default_model_per_provider("anthropic"),
            Some("claude-opus-4-8")
        );
        assert_eq!(default_model_per_provider("openai"), Some("gpt-5.5"));
        assert_eq!(
            default_model_per_provider("amazon-bedrock"),
            Some("us.anthropic.claude-opus-4-6-v1")
        );
        assert_eq!(default_model_per_provider("totally-unknown"), None);
    }

    #[test]
    fn resolve_cli_model_provider_and_pattern() {
        let models = vec![
            model("anthropic", "claude-opus-4-8", "Opus"),
            model("openai", "gpt-5.5", "GPT"),
        ];
        let auth = |_: &Model| true;
        // --provider anthropic --model opus → fuzzy match.
        let r = resolve_cli_model(Some("anthropic"), Some("opus"), None, &models, &auth);
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "claude-opus-4-8");
        assert!(r.error.is_none());
        // provider/model inference.
        let r = resolve_cli_model(None, Some("openai/gpt-5.5"), None, &models, &auth);
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "gpt-5.5");
        // unknown provider → error.
        let r = resolve_cli_model(Some("nope"), Some("x"), None, &models, &auth);
        assert!(r.error.as_ref().unwrap().contains("Unknown provider"));
    }

    #[test]
    fn resolve_cli_model_builds_fallback_custom_id() {
        // A custom model id under a known provider builds a fallback from the provider default.
        let models = vec![model("anthropic", "claude-opus-4-8", "Opus")];
        let auth = |_: &Model| true;
        let r = resolve_cli_model(
            Some("anthropic"),
            Some("my-custom-id"),
            None,
            &models,
            &auth,
        );
        let m = r.model.as_ref().unwrap();
        assert_eq!(m.id.as_str(), "my-custom-id");
        assert_eq!(m.provider.as_str(), "anthropic");
        assert!(
            r.warning
                .as_ref()
                .unwrap()
                .contains("Using custom model id")
        );
    }

    #[test]
    fn find_initial_model_priority() {
        let all = vec![
            model("anthropic", "claude-opus-4-8", "Opus"),
            model("openai", "gpt-5.5", "GPT"),
        ];
        let available = all.clone();
        let auth = |_: &Model| true;
        // CLI args win.
        let r = find_initial_model(
            Some("openai"),
            Some("gpt-5.5"),
            &[],
            false,
            None,
            None,
            None,
            &all,
            &available,
            &auth,
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "gpt-5.5");
        // No CLI, no scoped, no saved → curated default (anthropic first in table → opus).
        let r = find_initial_model(
            None,
            None,
            &[],
            false,
            None,
            None,
            None,
            &all,
            &available,
            &auth,
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "claude-opus-4-8");
        // Saved settings default beats curated default.
        let r = find_initial_model(
            None,
            None,
            &[],
            false,
            Some("openai"),
            Some("gpt-5.5"),
            None,
            &all,
            &available,
            &auth,
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "gpt-5.5");
    }

    #[test]
    fn restore_model_falls_back_when_no_auth() {
        let all = vec![model("anthropic", "claude-opus-4-8", "Opus")];
        let available = all.clone();
        // saved model has no auth → fall back to curated default with a message.
        let no_auth = |_: &Model| false;
        let r = restore_model_from_session(
            "anthropic",
            "claude-opus-4-8",
            None,
            &all,
            &available,
            &no_auth,
        );
        assert!(
            r.fallback_message
                .as_ref()
                .unwrap()
                .contains("no auth configured")
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "claude-opus-4-8");
        // saved model with auth → restored, no message.
        let yes_auth = |_: &Model| true;
        let r = restore_model_from_session(
            "anthropic",
            "claude-opus-4-8",
            None,
            &all,
            &available,
            &yes_auth,
        );
        assert!(r.fallback_message.is_none());
    }

    #[test]
    fn glob_matches_pi_minimatch_byte_for_byte() {
        // Byte-diff proof: every (pattern, text, expected) triple was captured from Pi's actual
        // `minimatch(text, pattern, { nocase: true })` (the exact call at model-resolver.ts:282),
        // committed to `src/testdata/glob_minimatch.json`. Assert the Rust matcher agrees on all
        // cases — covering the proven miss (`anthropic*` does NOT cross `/`, so it matches 0),
        // brace lists, globstar `**`, `?`, `[...]`, class negation, nocase, brace RANGES
        // (`{1..3}`/`{a..c}`/`{01..03}`/`{3..1}`/`{1..5..2}`), the extglobs
        // `@(..)`/`?(..)`/`*(..)`/`+(..)`, backslash escaping, and the dot-rule (`dot:false`). It
        // ALSO covers the residual-#3 minimatch corners now implemented 1:1: the `!(..)` negative
        // extglob in every position (mid-segment `a!(foo)b`/`x!(a|b)y`, across groups `a@(!(x))b`,
        // nested in repetition `+(!(a))`/`*(!(a|b))`, and after `/`), the leading-`!` whole-pattern
        // negation "standalone `!()` quirk" (`!(foo)`, `!!foo`, `!a/b`), the `[.]`-at-segment-start
        // dot-bracket corner (`[.]x` matches `.x`, `[.x]`/`[!.]` do not), class `\x` escaping
        // (`[\.]`), and the `justDots`/`needNoTrav` rule for `.`/`..` text segments.
        let fixture = include_str!("testdata/glob_minimatch.json");
        let cases: Vec<(String, String, bool)> =
            serde_json::from_str(fixture).expect("valid fixture");
        assert!(cases.len() >= 700, "fixture should be comprehensive");
        let mut mismatches = Vec::new();
        for (pattern, text, expected) in &cases {
            let got = glob_match(pattern, text);
            if got != *expected {
                mismatches.push(format!(
                    "minimatch({text:?}, {pattern:?}) = {expected}, glob_match = {got}"
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "glob_match diverges from Pi minimatch:\n{}",
            mismatches.join("\n")
        );
    }

    #[test]
    fn glob_negation_and_dot_bracket_corners() {
        // Focused assertions for residual-#3 (each value is Pi's `minimatch(text, pat, nocase)`).
        // Negative extglob `!(..)` (mid-segment; excludes the alternatives, includes the tail):
        assert!(glob_match("a!(foo)b", "axb"));
        assert!(!glob_match("a!(foo)b", "afoob"));
        assert!(!glob_match("x!(a|b)y", "xay"));
        assert!(glob_match("x!(a|b)y", "xcy"));
        // `!(..)` after `/` is a real extglob; `foo/!(bar)` excludes `bar`.
        assert!(glob_match("foo/!(bar)", "foo/baz"));
        assert!(!glob_match("foo/!(bar)", "foo/bar"));
        // fillNegs spans enclosing groups: `a@(!(x))b` lookahead is `xb(?:$|/)`.
        assert!(!glob_match("a@(!(x))b", "axb"));
        assert!(glob_match("a@(!(x))b", "axxb"));
        // `!(..)` nested in a repetition uses a segment-end lookahead, NOT the loop.
        assert!(!glob_match("+(!(foo))", "foo"));
        assert!(glob_match("+(!(foo))", "foobar"));
        assert!(!glob_match("*(!(foo))", "foo"));
        // Leading-`!` whole-pattern negation ("standalone `!()` quirk"): `!(foo)` == `!`+`(foo)`.
        assert!(glob_match("!(foo)", "foo")); // negate + literal `(foo)`; `foo` != `(foo)`
        assert!(glob_match("!(foo)", "(bar)"));
        assert!(!glob_match("!(foo)", "(foo)"));
        assert!(!glob_match("!foo", "foo")); // simple whole-pattern negation
        assert!(glob_match("!foo", "bar"));
        assert!(glob_match("!!foo", "foo")); // double negation
        assert!(!glob_match("!a/b", "a/b"));
        assert!(glob_match("!a/b", "a/c"));
        // `[.]`-at-segment-start dot-bracket corner: single-char `[.]`/`[\.]` == literal `.`.
        assert!(glob_match("[.]x", ".x"));
        assert!(!glob_match("[.x]", "."));
        assert!(!glob_match("[!.]", "."));
        assert!(glob_match("[\\.]", "."));
        assert!(!glob_match("[\\.]", "\\")); // class `\x` escaping: `[\.]` is `.`, not `\`
        // `justDots`: `.`/`..` only match a purely-literal equal pattern.
        assert!(glob_match("[.]", "."));
        assert!(!glob_match("[.]*", ".")); // has a wildcard -> not "just dots"
        assert!(!glob_match(".*", "."));
    }

    #[test]
    fn partial_tiebreak_collator_matches_pi_localecompare() {
        // Pi tie-breaks ambiguous partial matches with `b.id.localeCompare(a.id)`
        // (model-resolver.ts:147,151). Every (a, b, sign) triple is the EXACT sign Node returned for
        // `b.localeCompare(a)`, captured to `src/testdata/locale_compare.json`. Assert the `feruca`
        // collator we tie-break with (CLDR-root, non-ignorable, byte tiebreak — the same config the
        // sort uses) reproduces that sign for every pair, so the tie-break is byte-1:1 with Pi rather
        // than the old Unicode-scalar `String::cmp` (which diverges on case + `-`/`_`/`.`).
        let fixture = include_str!("testdata/locale_compare.json");
        let cases: Vec<(String, String, i32)> =
            serde_json::from_str(fixture).expect("valid locale_compare fixture");
        assert!(cases.len() >= 800, "fixture should be comprehensive");
        let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
        let mut mismatches = Vec::new();
        for (a, b, sign) in &cases {
            let got = match collator.collate(b.as_str(), a.as_str()) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            // Also prove the OLD scalar cmp would have diverged on the divergent pairs (informational
            // — the assertion is only on the collator matching Pi).
            if got != *sign {
                let scalar = match b.as_str().cmp(a.as_str()) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                };
                mismatches.push(format!(
                    "b={b:?} a={a:?}: Pi localeCompare={sign}, feruca={got} (scalar cmp={scalar})"
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "tie-break collator diverges from Pi localeCompare:\n{}",
            mismatches.join("\n")
        );
    }

    #[test]
    fn scope_glob_is_path_segment_aware_like_pi() {
        // Assembled behaviour: the proven miss is that `anthropic*` matched ALL anthropic models in
        // the old flat matcher but matches NONE in Pi (the `*` cannot cross `/` into the id, and no
        // bare id starts with "anthropic"). Conversely `anthropic/*` matches the two anthropic
        // models. Models with multi-segment ids (cloudflare) require `**` to traverse.
        let models = vec![
            model("anthropic", "claude-opus-4-8", "Opus"),
            model("anthropic", "claude-haiku-4", "Haiku"),
            model("openai", "gpt-5.5", "GPT"),
            model("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6", "Kimi"),
        ];
        let r = ModelResolver::new(&models);
        assert_eq!(
            r.resolve_scope(&["anthropic*".to_string()]).len(),
            0,
            "Pi: `anthropic*` crosses no `/`, matches 0"
        );
        assert_eq!(r.resolve_scope(&["anthropic/*".to_string()]).len(), 2);
        // `{anthropic,openai}/*` brace-expands to two segment patterns → 3 models.
        assert_eq!(
            r.resolve_scope(&["{anthropic,openai}/*".to_string()]).len(),
            3
        );
        // A single `*` segment does NOT match the multi-segment cloudflare id; `**` does.
        assert_eq!(
            r.resolve_scope(&["cloudflare-workers-ai/*".to_string()]).len(),
            0
        );
        assert_eq!(
            r.resolve_scope(&["cloudflare-workers-ai/**".to_string()])
                .len(),
            1
        );
    }

    #[test]
    fn glob_question_and_class_and_scope_level() {
        let models = vec![
            model("anthropic", "claude-opus-4-8", "Opus"),
            model("anthropic", "claude-haiku-4", "Haiku"),
            model("openai", "gpt-5.5", "GPT"),
        ];
        let r = ModelResolver::new(&models);
        // `?` matches one char; `[...]` class.
        let scoped = r.resolve_scope(&["anthropic/claude-opus-4-?".to_string()]);
        assert_eq!(scoped.len(), 1);
        let scoped = r.resolve_scope(&["anthropic/claude-[ho]*".to_string()]);
        assert_eq!(scoped.len(), 2);
        // `:level` suffix on a glob applies to every match.
        let scoped = r.resolve_scope(&["anthropic/*:high".to_string()]);
        assert!(
            scoped
                .iter()
                .all(|s| s.thinking_level == Some(ModelThinkingLevel::High))
        );
    }

    #[test]
    fn models_file_provider_config_resolves_auth() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            r#"{ "providers": { "acme": { "baseUrl": "https://api.acme.test", "apiKey": "literal-key", "authHeader": true } } }"#,
        )
        .unwrap();
        let file = load_models_file(&path).unwrap();
        let cfg = file.providers.get("acme").unwrap();
        assert_eq!(cfg.base_url.as_deref(), Some("https://api.acme.test"));
        let resolved = cfg.resolve_request_auth(None).unwrap();
        assert_eq!(resolved.api_key.as_deref(), Some("literal-key"));
        assert_eq!(resolved.auth_header, Some(true));
        // missing file → empty
        assert!(
            load_models_file(&dir.join("nope.json"))
                .unwrap()
                .providers
                .is_empty()
        );
    }

    #[test]
    fn load_custom_models_roundtrip() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        let models = vec![model("custom", "my-model", "My Model")];
        std::fs::write(&path, serde_json::to_string(&models).unwrap()).unwrap();
        let loaded = load_custom_models(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.first().unwrap().id.as_str(), "my-model");
        // missing file → empty
        assert!(
            load_custom_models(&dir.join("nope.json"))
                .unwrap()
                .is_empty()
        );
    }
}
