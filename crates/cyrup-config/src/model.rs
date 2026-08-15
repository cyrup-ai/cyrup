//! Model resolution: pattern matching (`provider/id`, bare id, partial/alias), the `:level`
//! thinking shorthand, per-provider defaults, scoping + cycling, and custom `models.json`
//! (arch-07 §3.6/§6.4, R-07-019…R-07-023).

use std::path::Path;

use cyrup_core::{ModelThinkingLevel, ProviderId};
use cyrup_provider::Model;

use crate::error::ConfigError;

/// Parse a thinking-level token (`off|minimal|low|medium|high|xhigh|max` — Pi
/// `VALID_THINKING_LEVELS`, args.ts:59).
pub fn parse_thinking_level(s: &str) -> Option<ModelThinkingLevel> {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" => Some(ModelThinkingLevel::Off),
        "minimal" => Some(ModelThinkingLevel::Minimal),
        "low" => Some(ModelThinkingLevel::Low),
        "medium" => Some(ModelThinkingLevel::Medium),
        "high" => Some(ModelThinkingLevel::High),
        "xhigh" => Some(ModelThinkingLevel::Xhigh),
        "max" => Some(ModelThinkingLevel::Max),
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

    /// Exact model-reference match, with NO partial fallback (Pi
    /// `findExactModelReferenceMatch`, model-resolver.ts:79-120 @v0.83.0). Accepts either a
    /// canonical `provider/modelId` reference or a bare model id; a bare id carried by more than
    /// one provider is ambiguous and yields `None` (`:118`).
    ///
    /// Split out of [`ModelResolver::match_reference`] because Pi calls it in TWO places the
    /// partial matcher must not run: `tryMatchModel`'s first step (`:128`) and — the case CFG-018
    /// records — INSIDE the glob branch of `resolveModelScope` (`:297`), before the minimatch
    /// filter.
    fn exact_reference_match(&self, reference: &str) -> Option<&'a Model> {
        let reference = reference.trim();
        if reference.is_empty() {
            return None;
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
                return Some(m);
            }
        }

        // 2. bare exact id. Pi's `findExactModelReferenceMatch` returns the model ONLY when exactly
        // one id matches; a bare id present on >1 provider returns `undefined` (it does NOT error),
        // so it falls through to partial matching in `tryMatchModel` (model-resolver.ts:116-118).
        // Likewise a zero-hit exact match falls through.
        let exact: Vec<&Model> = self
            .available
            .iter()
            .filter(|m| m.id.as_str().to_ascii_lowercase() == lower)
            .collect();
        if exact.len() == 1 {
            return exact.first().copied();
        }
        None
    }

    fn match_reference(&self, reference: &str) -> Match<'a> {
        let reference = reference.trim();
        if reference.is_empty() {
            return Match::None;
        }
        let lower = reference.to_ascii_lowercase();

        // 1-2. exact reference (canonical or bare id) — Pi `tryMatchModel`'s first step, :128.
        if let Some(m) = self.exact_reference_match(reference) {
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
                    // Pi's exact sentence (model-resolver.ts:243 @v0.83.0):
                    // `Invalid thinking level "X" in pattern "Y". Using default instead.` — `Y` is
                    // the pattern at THIS recursion level, which is what upstream interpolates.
                    warning: Some(format!(
                        "Invalid thinking level \"{suffix}\" in pattern \"{pattern}\". Using default instead."
                    )),
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
    ///
    /// Diagnostic-free convenience wrapper over [`ModelResolver::resolve_scope_reporting`]; Pi's
    /// `resolveModelScope` always returns `{ scopedModels, diagnostics }` (model-resolver.ts:270
    /// @v0.83.0), so a caller that wants the warnings must use the reporting form.
    pub fn resolve_scope(&self, patterns: &[String]) -> Vec<ScopedModel> {
        self.resolve_scope_reporting(patterns).models
    }

    /// Expand scope patterns AND report Pi's `ModelScopeDiagnostic`s (`model-resolver.ts:261-270`
    /// @v0.83.0): `no-match` for a pattern that resolves to nothing (pushed at `:316` on the glob
    /// path and `:340` on the reference path) and `invalid-thinking-level` for a bad `:level`
    /// suffix (minted at `:243`, pushed at `:334`).
    pub fn resolve_scope_reporting(&self, patterns: &[String]) -> ModelScopeResult {
        let mut diagnostics: Vec<ModelScopeDiagnostic> = Vec::new();
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
                // Pi tries an EXACT reference match before the minimatch filter (`:297-303`), so a
                // pattern that happens to carry a glob metacharacter (`[`, `?`) but names a real
                // model resolves directly. CFG-018.
                if let Some(exact) = self.exact_reference_match(glob_pattern) {
                    push(exact.clone(), level, &mut seen, &mut out);
                    continue;
                }
                let matching: Vec<&Model> = self
                    .available
                    .iter()
                    .filter(|m| {
                        glob_match(glob_pattern, &format!("{}/{}", m.provider, m.id))
                            || glob_match(glob_pattern, m.id.as_str())
                    })
                    .collect();
                if matching.is_empty() {
                    diagnostics.push(ModelScopeDiagnostic {
                        level: ModelScopeDiagnosticLevel::Warning,
                        code: ModelScopeDiagnosticCode::NoMatch,
                        message: format!("No models match pattern \"{pattern}\""),
                        pattern: pattern.clone(),
                    });
                    continue;
                }
                for m in matching {
                    push(m.clone(), level, &mut seen, &mut out);
                }
            } else {
                let parsed = self.parse_pattern(pattern, false);
                if let Some(warning) = parsed.warning {
                    diagnostics.push(ModelScopeDiagnostic {
                        level: ModelScopeDiagnosticLevel::Warning,
                        code: ModelScopeDiagnosticCode::InvalidThinkingLevel,
                        message: warning,
                        pattern: pattern.clone(),
                    });
                }
                match parsed.model {
                    Some(m) => push(m, parsed.thinking_level, &mut seen, &mut out),
                    None => diagnostics.push(ModelScopeDiagnostic {
                        level: ModelScopeDiagnosticLevel::Warning,
                        code: ModelScopeDiagnosticCode::NoMatch,
                        message: format!("No models match pattern \"{pattern}\""),
                        pattern: pattern.clone(),
                    }),
                }
            }
        }
        ModelScopeResult {
            models: out,
            diagnostics,
        }
    }
}

/// Severity of a [`ModelScopeDiagnostic`] (Pi's `type: "warning"`, model-resolver.ts:262 @v0.83.0 —
/// upstream mints only warnings today, so the enum has one arm).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelScopeDiagnosticLevel {
    Warning,
}

/// Machine-readable diagnostic code (Pi `code: "no-match" | "invalid-thinking-level"`,
/// model-resolver.ts:263 @v0.83.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelScopeDiagnosticCode {
    NoMatch,
    InvalidThinkingLevel,
}

/// One warning emitted while expanding `--models` scope patterns (Pi `ModelScopeDiagnostic`,
/// model-resolver.ts:261-268 @v0.83.0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelScopeDiagnostic {
    pub level: ModelScopeDiagnosticLevel,
    pub code: ModelScopeDiagnosticCode,
    pub message: String,
    /// The originating pattern, verbatim (Pi carries `pattern` on every diagnostic, `:267`).
    pub pattern: String,
}

/// Result of [`ModelResolver::resolve_scope_reporting`] (Pi's `{ scopedModels, diagnostics }`,
/// model-resolver.ts:270 @v0.83.0).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelScopeResult {
    pub models: Vec<ScopedModel>,
    pub diagnostics: Vec<ModelScopeDiagnostic>,
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
                i += if p.get(i + 1).copied() == Some('\\') {
                    4
                } else {
                    3
                };
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
                out.push(
                    char::from_u32(x as u32)
                        .map(String::from)
                        .unwrap_or_default(),
                );
                x += step;
            }
        } else {
            while x >= b {
                out.push(
                    char::from_u32(x as u32)
                        .map(String::from)
                        .unwrap_or_default(),
                );
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
/// model-resolver.ts:14-53 at v0.83.0). Returns `None` for an unknown provider.
pub fn default_model_per_provider(provider: &str) -> Option<&'static str> {
    let id = match provider {
        "amazon-bedrock" => "us.anthropic.claude-opus-4-6-v1",
        "ant-ling" => "Ring-2.6-1T",
        "anthropic" => "claude-opus-4-8",
        "openai" => "gpt-5.5",
        "azure-openai-responses" => "gpt-5.4",
        "openai-codex" => "gpt-5.5",
        "radius" => "auto",
        "nvidia" => "nvidia/nemotron-3-super-120b-a12b",
        "deepseek" => "deepseek-v4-pro",
        "google" => "gemini-3.1-pro-preview",
        "google-vertex" => "gemini-3.1-pro-preview",
        "github-copilot" => "gpt-5.4",
        "openrouter" => "moonshotai/kimi-k2.6",
        "vercel-ai-gateway" => "zai/glm-5.1",
        "xai" => "grok-4.5",
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
        "baseten" => "zai-org/GLM-5.2",
        "opencode" => "kimi-k2.6",
        "opencode-go" => "kimi-k2.6",
        "kimi-coding" => "kimi-for-coding",
        "cloudflare-workers-ai" => "@cf/moonshotai/kimi-k2.6",
        "cloudflare-ai-gateway" => "workers-ai/@cf/moonshotai/kimi-k2.6",
        // Alibaba Cloud Model Studio "Token Plan" — two regions, identical catalogs, separate
        // endpoints and API keys (`ai/scripts/generate-models.ts:1993-2012`). Both name the same
        // curated default, which is pi's own value at `model-resolver.ts:47-48` and NOT an
        // extrapolation from the `-cn` sibling: upstream writes `qwen3.7-max` on both keys.
        "qwen-token-plan" => "qwen3.7-max",
        "qwen-token-plan-cn" => "qwen3.7-max",
        "qwen-token-plan-individual" => "qwen3.8-max",
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
    "radius",
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
    "baseten",
    "opencode",
    "opencode-go",
    "kimi-coding",
    "cloudflare-workers-ai",
    "cloudflare-ai-gateway",
    // Position is load-bearing: [`first_default_or_first`] returns the FIRST provider in this list
    // with an available curated-default match, so the order must be pi's `Object.keys` order —
    // insertion order of `defaultModelPerProvider` (`model-resolver.ts:14-53`), where the two
    // qwen keys sit between `cloudflare-ai-gateway` and `xiaomi`.
    "qwen-token-plan",
    "qwen-token-plan-cn",
    "qwen-token-plan-individual",
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

    // 3. Saved default from settings if auth is configured (Pi model-resolver.ts:621-630
    //    @v0.83.0: `if (found && modelRuntime.hasConfiguredAuth(found.provider))`, falling through
    //    to step 4 at `:632` when the check fails).
    if let (Some(dp), Some(dm)) = (default_provider, default_model_id)
        && let Some(found) = all
            .iter()
            .find(|m| m.provider.as_str() == dp && m.id.as_str() == dm)
        && has_configured_auth(found)
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

/// The OAuth auth mode a `models.json` provider block may declare (Pi
/// `ProviderConfigSchema.oauth`, model-config.ts:194).
///
/// Pi types this as `Type.Literal("radius")` — `radius` is the ONLY accepted spelling, and any
/// other value is a whole-file schema rejection (model-config.ts:265-272), not a silently ignored
/// key. Modelling it as a single-variant enum reproduces that: serde fails the load, and
/// [`load_models_file_reporting`] turns the failure into Pi's empty-snapshot-plus-one-message
/// contract.
///
/// **[CYRUP-DELTA]** cyrup does not port the `radius` provider itself (`configureRadiusProviders`,
/// model-runtime.ts:175-191, synthesizes a built-in from the block's `baseUrl`), so a `radius`
/// block currently composes against no base models and contributes none. The composition-layer
/// semantics below are ported regardless, so the block is ACCEPTED rather than rejected with a
/// misleading "must specify baseUrl, headers, compat, modelOverrides, or models".
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelsJsonOauth {
    Radius,
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
    /// OAuth auth mode (Pi `ProviderConfigSchema.oauth`, model-config.ts:194). Setting it makes
    /// `baseUrl` mandatory (provider-composer.ts:167-169) because that URL is the auth GATEWAY, and
    /// for the same reason suppresses the provider-level rewrite of the built-ins' request
    /// `baseUrl` (:188). It also counts as a distinguishing key in the empty-block guard (:178).
    #[serde(default)]
    pub oauth: Option<ModelsJsonOauth>,
    /// Provider-level compatibility overrides applied to every model of this provider (Pi
    /// `ProviderConfigSchema.compat`, model-config.ts:196).
    #[serde(default)]
    pub compat: Option<cyrup_provider::api::compat::OpenAiCompletionsCompat>,
    /// Inline model definitions (Pi `ProviderConfigSchema.models`, model-config.ts:197).
    #[serde(default)]
    pub models: Vec<ModelDefinition>,
    /// Per-model patches applied LAST, over built-ins and custom models alike (Pi
    /// `ProviderConfigSchema.modelOverrides`, model-config.ts:198; applied at
    /// provider-composer.ts:433-436).
    #[serde(default)]
    pub model_overrides: std::collections::BTreeMap<String, ModelOverride>,
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

/// A single `models` entry inside a `models.json` provider block (Pi `ModelDefinitionSchema`,
/// model-config.ts:152-166). Every field but `id` is optional and inherits from the provider block
/// or from the same-id built-in model (Pi `modelFromJson`, provider-composer.ts:124-159).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDefinition {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub thinking_level_map: Option<cyrup_provider::model::ThinkingLevelMap>,
    #[serde(default)]
    pub input: Option<Vec<cyrup_provider::Modality>>,
    #[serde(default)]
    pub cost: Option<cyrup_provider::ModelCost>,
    /// `Type.Optional(Type.Number())` (model-config.ts:163 @v0.83.0) — SIGNED, because pi accepts a
    /// negative value at the schema layer and rejects it in `modelFromJson` with
    /// `invalid contextWindow`, per PROVIDER, keeping the rest of the file. A `u64` would turn the
    /// same document into a whole-file parse failure (CFG-046).
    #[serde(default)]
    pub context_window: Option<i64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    /// `Type.Optional(Type.Record(Type.String(), Type.Unknown()))` (model-config.ts:167 @v0.84.1) —
    /// arbitrary OpenAI-compatible sampling keys (`top_p`, `top_k`, `min_p`,
    /// `repetition_penalty`, …) that become the composed model's defaults. `modelFromJson` copies
    /// it straight across (`provider-composer.ts:158`); it is NOT inherited from the provider block
    /// or from the same-id built-in, because pi's `ModelDefinitionSchema` has no provider-level
    /// twin. CFG-039.
    #[serde(default)]
    pub sampling_params: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub compat: Option<cyrup_provider::api::compat::OpenAiCompletionsCompat>,
}

/// A `modelOverrides` entry: a partial patch applied to an already-composed model (Pi
/// `ModelOverrideSchema`, model-config.ts:168-186; applied last, provider-composer.ts:433-436).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOverride {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub thinking_level_map: Option<cyrup_provider::model::ThinkingLevelMap>,
    #[serde(default)]
    pub input: Option<Vec<cyrup_provider::Modality>>,
    #[serde(default)]
    pub cost: Option<ModelCostOverride>,
    /// Signed for the same reason as [`ModelDefinition::context_window`] (model-config.ts:184).
    #[serde(default)]
    pub context_window: Option<i64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    /// `Type.Optional(Type.Record(Type.String(), Type.Unknown()))` (model-config.ts:188 @v0.84.1).
    /// Unlike every other override field this one MERGES per key rather than replacing:
    /// `override.samplingParams ? { ...model.samplingParams, ...override.samplingParams } :
    /// model.samplingParams` (`provider-composer.ts:123-125`). CFG-039.
    #[serde(default)]
    pub sampling_params: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub compat: Option<cyrup_provider::api::compat::OpenAiCompletionsCompat>,
}

/// The partial `cost` shape a `modelOverrides` entry may carry (model-config.ts:174-182): every rate
/// is individually optional and patches the composed model's cost.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostOverride {
    #[serde(default)]
    pub input: Option<f64>,
    #[serde(default)]
    pub output: Option<f64>,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    #[serde(default)]
    pub tiers: Option<Vec<cyrup_provider::ModelCostTier>>,
}

/// A parsed `models.json` in Pi's `{ providers: { <name>: ProviderConfig } }` shape
/// (model-registry.ts:216-218 / model-config.ts:188-190).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct ModelFile {
    #[serde(default)]
    pub providers: std::collections::BTreeMap<String, ProviderConfig>,
}

/// Strip `//` line comments and trailing commas from JSON, leaving string literals untouched — a
/// 1:1 port of Pi's `stripJsonComments` (coding-agent/src/utils/json.ts), which every `models.json`
/// read goes through (`JSON.parse(stripJsonComments(content))`, model-config.ts:257).
///
/// Written as a single scanning pass rather than the two regex replaces, because Rust's `regex`
/// crate has no backreference-free equivalent of the alternation trick and a scanner is exact.
fn strip_json_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    // Byte offsets in `out` of pending `,` characters that may turn out to be trailing.
    let mut pending_comma: Option<usize> = None;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                pending_comma = None;
                out.push(c);
                let mut escaped = false;
                for sc in chars.by_ref() {
                    out.push(sc);
                    if escaped {
                        escaped = false;
                    } else if sc == '\\' {
                        escaped = true;
                    } else if sc == '"' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                for sc in chars.by_ref() {
                    if sc == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ',' => {
                pending_comma = Some(out.len());
                out.push(c);
            }
            '}' | ']' => {
                if let Some(at) = pending_comma.take() {
                    // Everything between the comma and here is whitespace (any other char cleared
                    // `pending_comma`), so the comma is trailing: drop it.
                    out.remove(at);
                }
                out.push(c);
            }
            c if c.is_whitespace() => out.push(c),
            c => {
                pending_comma = None;
                out.push(c);
            }
        }
    }
    out
}

/// Load a `models.json` provider-config file (Pi's `{ providers: {...} }` shape). A missing or
/// empty file yields an empty [`ModelFile`]. JSONC `//` comments and trailing commas are stripped
/// first, exactly as Pi does (model-config.ts:257). This is additive alongside
/// [`load_custom_models`] (which reads the legacy flat `Vec<Model>` shape).
pub fn load_models_file(path: &Path) -> Result<ModelFile, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ModelFile::default()),
        Err(e) => return Err(ConfigError::Io(e)),
    };
    if text.trim().is_empty() {
        return Ok(ModelFile::default());
    }
    let file: ModelFile = serde_json::from_str(&strip_json_comments(&text))?;
    Ok(file)
}

/// One `models.json` schema violation, in the shape Pi renders it at model-config.ts:274-277
/// @v0.83.0 — `  - ${formatValidationPath(error)}: ${error.message}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelsSchemaError {
    /// The dotted instance path (`formatValidationPath`, model-config.ts:217-228): the JSON pointer
    /// with its leading `/` stripped and the rest of the `/`s turned into `.`; `root` when empty,
    /// and `<basePath>.<missingProperty>` for a `required` failure.
    pub path: String,
    /// The validator's message, e.g. `Expected number`.
    pub message: String,
}

/// Typebox message strings. These are the messages the LIBRARY produces (`typebox/error`), not
/// literals present in `model-config.ts`; the pi code opened at v0.83.0 only interpolates
/// `error.message` (`:276`). Recorded here so the rendered report is one place, not eight.
mod schema_msg {
    pub const REQUIRED: &str = "Expected required property";
    pub const OBJECT: &str = "Expected object";
    pub const ARRAY: &str = "Expected array";
    pub const STRING: &str = "Expected string";
    pub const NUMBER: &str = "Expected number";
    pub const BOOLEAN: &str = "Expected boolean";
    pub const UNION: &str = "Expected union value";
    /// `Type.String({ minLength: 1 })` — the check CFG-046 exists for.
    pub const MIN_LENGTH_1: &str = "Expected string length greater or equal to 1";
}

/// Render a JSON-pointer-ish path segment list the way `formatValidationPath` does
/// (model-config.ts:217-228 @v0.83.0): dotted, `root` when empty.
fn schema_path(segments: &[String]) -> String {
    if segments.is_empty() {
        "root".to_string()
    } else {
        segments.join(".")
    }
}

fn push_err(errs: &mut Vec<ModelsSchemaError>, segments: &[String], message: &str) {
    errs.push(ModelsSchemaError {
        path: schema_path(segments),
        message: message.to_string(),
    });
}

fn child(segments: &[String], key: &str) -> Vec<String> {
    let mut out = segments.to_vec();
    out.push(key.to_string());
    out
}

/// `Type.Optional(Type.String({ minLength: 1 }))` — the shape carried by `name`, `baseUrl`,
/// `apiKey` and `api` on `ProviderConfigSchema` (model-config.ts:188-198 @v0.83.0) and by `name` /
/// `api` / `baseUrl` on `ModelDefinitionSchema` (`:155-158`).
fn check_opt_string_min1(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(v) = obj.get(key) else { return };
    let here = child(at, key);
    match v.as_str() {
        None => push_err(errs, &here, schema_msg::STRING),
        Some(s) if s.is_empty() => push_err(errs, &here, schema_msg::MIN_LENGTH_1),
        Some(_) => {}
    }
}

fn check_opt_number(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    if let Some(v) = obj.get(key)
        && !v.is_number()
    {
        push_err(errs, &child(at, key), schema_msg::NUMBER);
    }
}

fn check_opt_bool(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    if let Some(v) = obj.get(key)
        && !v.is_boolean()
    {
        push_err(errs, &child(at, key), schema_msg::BOOLEAN);
    }
}

/// `Type.Optional(Type.Record(Type.String(), Type.String()))` — `headers`.
fn check_opt_string_record(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(v) = obj.get(key) else { return };
    let here = child(at, key);
    let Some(map) = v.as_object() else {
        push_err(errs, &here, schema_msg::OBJECT);
        return;
    };
    for (k, hv) in map {
        if !hv.is_string() {
            push_err(errs, &child(&here, k), schema_msg::STRING);
        }
    }
}

/// `Type.Optional(Type.Array(Type.Union([Type.Literal("text"), Type.Literal("image")])))` —
/// `input` (model-config.ts:161 / :172 @v0.83.0).
fn check_opt_modalities(
    obj: &serde_json::Map<String, serde_json::Value>,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(v) = obj.get("input") else { return };
    let here = child(at, "input");
    let Some(arr) = v.as_array() else {
        push_err(errs, &here, schema_msg::ARRAY);
        return;
    };
    for (i, item) in arr.iter().enumerate() {
        if !matches!(item.as_str(), Some("text" | "image")) {
            push_err(errs, &child(&here, &i.to_string()), schema_msg::UNION);
        }
    }
}

/// `ModelCostSchema` (model-config.ts:149-152 @v0.83.0): the four rates are REQUIRED, `tiers` is an
/// optional array of `ModelCostTierSchema` (`:145-148`, whose `inputTokensAbove` plus the same four
/// rates are all required).
fn check_cost(
    obj: &serde_json::Map<String, serde_json::Value>,
    at: &[String],
    required_rates: bool,
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(v) = obj.get("cost") else { return };
    let here = child(at, "cost");
    let Some(map) = v.as_object() else {
        push_err(errs, &here, schema_msg::OBJECT);
        return;
    };
    for rate in ["input", "output", "cacheRead", "cacheWrite"] {
        match map.get(rate) {
            None if required_rates => push_err(errs, &child(&here, rate), schema_msg::REQUIRED),
            None => {}
            Some(rv) if !rv.is_number() => push_err(errs, &child(&here, rate), schema_msg::NUMBER),
            Some(_) => {}
        }
    }
    let Some(tiers) = map.get("tiers") else {
        return;
    };
    let tiers_at = child(&here, "tiers");
    let Some(arr) = tiers.as_array() else {
        push_err(errs, &tiers_at, schema_msg::ARRAY);
        return;
    };
    for (i, tier) in arr.iter().enumerate() {
        let tier_at = child(&tiers_at, &i.to_string());
        let Some(tm) = tier.as_object() else {
            push_err(errs, &tier_at, schema_msg::OBJECT);
            continue;
        };
        for field in [
            "inputTokensAbove",
            "input",
            "output",
            "cacheRead",
            "cacheWrite",
        ] {
            match tm.get(field) {
                None => push_err(errs, &child(&tier_at, field), schema_msg::REQUIRED),
                Some(fv) if !fv.is_number() => {
                    push_err(errs, &child(&tier_at, field), schema_msg::NUMBER);
                }
                Some(_) => {}
            }
        }
    }
}

/// `ModelDefinitionSchema` (model-config.ts:154-166 @v0.83.0).
fn check_model_definition(
    value: &serde_json::Value,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(obj) = value.as_object() else {
        push_err(errs, at, schema_msg::OBJECT);
        return;
    };
    match obj.get("id") {
        None => push_err(errs, &child(at, "id"), schema_msg::REQUIRED),
        Some(v) => match v.as_str() {
            None => push_err(errs, &child(at, "id"), schema_msg::STRING),
            Some(s) if s.is_empty() => push_err(errs, &child(at, "id"), schema_msg::MIN_LENGTH_1),
            Some(_) => {}
        },
    }
    for key in ["name", "api", "baseUrl"] {
        check_opt_string_min1(obj, key, at, errs);
    }
    check_opt_bool(obj, "reasoning", at, errs);
    check_opt_modalities(obj, at, errs);
    check_cost(obj, at, true, errs);
    check_opt_number(obj, "contextWindow", at, errs);
    check_opt_number(obj, "maxTokens", at, errs);
    check_opt_string_record(obj, "headers", at, errs);
}

/// `ModelOverrideSchema` (model-config.ts:168-186 @v0.83.0). Its `cost` block differs from a model
/// definition's: every rate is individually optional (`:174-182`).
fn check_model_override(
    value: &serde_json::Value,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(obj) = value.as_object() else {
        push_err(errs, at, schema_msg::OBJECT);
        return;
    };
    check_opt_string_min1(obj, "name", at, errs);
    check_opt_bool(obj, "reasoning", at, errs);
    check_opt_modalities(obj, at, errs);
    check_cost(obj, at, false, errs);
    check_opt_number(obj, "contextWindow", at, errs);
    check_opt_number(obj, "maxTokens", at, errs);
    check_opt_string_record(obj, "headers", at, errs);
}

/// Validate a parsed `models.json` against Pi's `ModelsConfigSchema`
/// (`validateModelsConfig.Check(parsed)`, model-config.ts:265 @v0.83.0) and return every failure,
/// which is what Pi renders — `.Errors(parsed).map(...)` at `:272-277`, not just the first.
///
/// **[CYRUP-DELTA]** `compat` is left to serde. Upstream types it as a three-way union of ~40
/// optional keys (`ProviderCompatSchema`, model-config.ts:133-137); reproducing that union's
/// per-arm error text here would duplicate `cyrup_provider::api::compat`'s own definition. A
/// malformed `compat` therefore surfaces through the serde pass below, still under the
/// `Invalid models.json schema:` heading and still naming the offending key.
pub fn validate_models_config(value: &serde_json::Value) -> Vec<ModelsSchemaError> {
    let mut errs: Vec<ModelsSchemaError> = Vec::new();
    let root: Vec<String> = Vec::new();
    let Some(obj) = value.as_object() else {
        push_err(&mut errs, &root, schema_msg::OBJECT);
        return errs;
    };
    // `providers: Type.Record(...)` is NOT optional (model-config.ts:201-203).
    let Some(providers) = obj.get("providers") else {
        push_err(&mut errs, &["providers".to_string()], schema_msg::REQUIRED);
        return errs;
    };
    let providers_at = vec!["providers".to_string()];
    let Some(providers) = providers.as_object() else {
        push_err(&mut errs, &providers_at, schema_msg::OBJECT);
        return errs;
    };
    for (provider_id, provider) in providers {
        let at = child(&providers_at, provider_id);
        let Some(pobj) = provider.as_object() else {
            push_err(&mut errs, &at, schema_msg::OBJECT);
            continue;
        };
        for key in ["name", "baseUrl", "apiKey", "api"] {
            check_opt_string_min1(pobj, key, &at, &mut errs);
        }
        // `oauth: Type.Optional(Type.Literal("radius"))` (model-config.ts:194).
        if let Some(oauth) = pobj.get("oauth")
            && oauth.as_str() != Some("radius")
        {
            push_err(&mut errs, &child(&at, "oauth"), "Expected \"radius\"");
        }
        check_opt_string_record(pobj, "headers", &at, &mut errs);
        check_opt_bool(pobj, "authHeader", &at, &mut errs);
        if let Some(models) = pobj.get("models") {
            let models_at = child(&at, "models");
            match models.as_array() {
                None => push_err(&mut errs, &models_at, schema_msg::ARRAY),
                Some(arr) => {
                    for (i, m) in arr.iter().enumerate() {
                        check_model_definition(m, &child(&models_at, &i.to_string()), &mut errs);
                    }
                }
            }
        }
        if let Some(overrides) = pobj.get("modelOverrides") {
            let ov_at = child(&at, "modelOverrides");
            match overrides.as_object() {
                None => push_err(&mut errs, &ov_at, schema_msg::OBJECT),
                Some(map) => {
                    for (id, ov) in map {
                        check_model_override(ov, &child(&ov_at, id), &mut errs);
                    }
                }
            }
        }
    }
    errs
}

/// Render a schema-failure list as Pi's report body (model-config.ts:272-278 @v0.83.0), including
/// its `|| "Unknown schema error"` fallback for an empty list.
fn render_schema_errors(errs: &[ModelsSchemaError]) -> String {
    if errs.is_empty() {
        return "Unknown schema error".to_string();
    }
    errs.iter()
        .map(|e| format!("  - {}: {}", e.path, e.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Load `<agent_dir>/models.json` into a composed [`ModelFile`], turning EVERY failure mode into a
/// human-readable message instead of an error the caller might treat as fatal.
///
/// Pi keeps a `ModelConfig` with an empty provider map plus one distinct error string per failure —
/// load / parse / schema (model-config.ts:251, :261, :271) — and the agent starts normally with the
/// built-in registry. This mirrors that contract: the returned `ModelFile` is empty on failure and
/// the `Option<String>` is the diagnostic the startup panel renders.
pub fn load_models_file_reporting(path: &Path) -> (ModelFile, Option<String>) {
    let empty = |msg: String| (ModelFile::default(), Some(msg));
    // Tier 1 — read (`ModelConfig.load`'s catch at model-config.ts:251-256 @v0.83.0). ENOENT is an
    // empty snapshot with NO message (`:250`).
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (ModelFile::default(), None),
        Err(e) => {
            return empty(format!(
                "Failed to load models.json: {e}\n\nFile: {}",
                path.display()
            ));
        }
    };
    if text.trim().is_empty() {
        return (ModelFile::default(), None);
    }
    // Tier 2 — JSON syntax (`JSON.parse(stripJsonComments(content))`, `:259-270`).
    let value: serde_json::Value = match serde_json::from_str(&strip_json_comments(&text)) {
        Ok(v) => v,
        Err(e) => {
            return empty(format!(
                "Failed to parse models.json: {e}\n\nFile: {}",
                path.display()
            ));
        }
    };
    // Tier 3 — schema (`validateModelsConfig.Check`, `:265-279`). EVERY failing field is reported,
    // by dotted key path, under a heading distinct from the syntax one.
    let schema_errors = validate_models_config(&value);
    if !schema_errors.is_empty() {
        return empty(format!(
            "Invalid models.json schema:\n{}\n\nFile: {}",
            render_schema_errors(&schema_errors),
            path.display()
        ));
    }
    match serde_json::from_value::<ModelFile>(value) {
        Ok(file) => (file, None),
        // A typing failure the hand-written validator above does not cover (today: only `compat`'s
        // three-arm union) is still a SCHEMA failure in Pi's model, not a syntax one.
        Err(e) => empty(format!(
            "Invalid models.json schema:\n  - {e}\n\nFile: {}",
            path.display()
        )),
    }
}

impl ModelFile {
    /// Compose `base` (the built-in / provider-supplied registry) with this `models.json`, returning
    /// the effective model list plus one message per rejected provider block.
    ///
    /// 1:1 with Pi's `composeModelProvider` restricted to the credential-blind layers
    /// (provider-composer.ts:411-437): for every provider id in the union of `base` and the file,
    /// `applyModelsJson` rewrites `baseUrl`/`compat` on the built-ins and upserts the declared
    /// `models` (:161-199), then `modelOverrides` patches the result last (:433-436).
    ///
    /// A provider block that Pi would `throw` on (no distinguishing key, a custom model with no
    /// resolvable `api`/`baseUrl`, a non-positive `contextWindow`/`maxTokens`) is REJECTED WHOLE —
    /// its built-in models are kept untouched — and its message is returned. Pi's own
    /// `compositionErrors` map does exactly this (model-runtime.ts:104), so a single bad block never
    /// costs the user the rest of the registry.
    ///
    /// Provider ORDER follows Pi's `rebuildProviders` (model-runtime.ts:225-231): it iterates
    /// `providerIds()` = `builtins ∪ … ∪ config.getProviderIds()`, a `Set` whose iteration order is
    /// insertion order, so the built-ins keep their registration order and a provider that exists
    /// only in `models.json` is appended after them. Composition REPLACES a provider's entries in
    /// place (`models.setProvider(...)`, :215) — it never appends a second, shadowed copy.
    pub fn compose(&self, base: &[Model]) -> (Vec<Model>, Vec<String>) {
        let mut errors: Vec<String> = Vec::new();
        let mut out: Vec<Model> = Vec::new();
        // Pi's `providerIds()` order: base providers first (first-seen), then the file's own.
        let mut order: Vec<&str> = Vec::new();
        for m in base {
            if !order.contains(&m.provider.as_str()) {
                order.push(m.provider.as_str());
            }
        }
        for provider_id in self.providers.keys() {
            if !order.contains(&provider_id.as_str()) {
                order.push(provider_id.as_str());
            }
        }
        for provider_id in order {
            let base_models: Vec<Model> = base
                .iter()
                .filter(|m| m.provider.as_str() == provider_id)
                .cloned()
                .collect();
            let Some(config) = self.providers.get(provider_id) else {
                // No overlay: the built-in stands untouched (Pi :210-214).
                out.extend(base_models);
                continue;
            };
            match apply_models_json(provider_id, &base_models, config) {
                Ok(models) => out.extend(models),
                Err(msg) => {
                    errors.push(msg);
                    // Keep the untouched built-ins for this provider (Pi records the error and
                    // re-registers `base`, model-runtime.ts:218-221).
                    out.extend(base_models);
                }
            }
        }
        (out, errors)
    }
}

/// Whether `provider` has configured auth, across BOTH credential channels Pi's `hasConfiguredAuth`
/// sees — the credential store AND `models.json` (CFG-022).
///
/// Pi's `hasConfiguredAuth` (model-runtime.ts:372-374) is a set-membership test against
/// `snapshot.configuredProviders`, and that set is filled by running `models.checkAuth` over EVERY
/// composed provider. A provider that exists only in `models.json` is composed like any other, and
/// its `check` closure is `composeApiKeyAuth`'s (provider-composer.ts:314-332). So a user-declared
/// provider carrying its own `apiKey` counts as configured with nothing in `auth.json` at all.
///
/// cyrup had two disagreeing predicates: [`AuthStore::has_auth`] alone on the binary's default-launch
/// path (which knows only `--api-key`, an `auth.json` entry, and the `env_keys` table of KNOWN
/// provider ids, so a user-declared provider matched none of the three), and a second, models.json-
/// aware one inside the session. This is the single predicate both call.
///
/// `env` is the optional provider-scoped override map; it is consulted ahead of the process
/// environment by both tiers.
pub fn provider_is_configured(
    auth: &crate::auth::AuthStore,
    models_json: &ModelFile,
    provider: &ProviderId,
    env: Option<&std::collections::HashMap<String, String>>,
) -> bool {
    auth.has_auth(provider, env)
        || models_json_provider_is_configured(models_json, provider.as_str(), env)
}

/// The `models.json` tier of [`provider_is_configured`]: a declared `apiKey` that is *configured*
/// in the config-value sense (Pi `composeApiKeyAuth`'s `check`, provider-composer.ts:320-329).
///
/// **NEVER RESOLVES THE VALUE.** Pi's own check is deliberately pure — a `!command` value returns
/// "configured API key" on the strength of *being* a command (`isCommandConfigValue`, :321) without
/// running it, and a `$VAR` template is configured exactly when every name it references is defined
/// (:322-328). Resolving here would execute a shell command out of `models.json` on a *status*
/// query, on a predicate that runs inside filter loops; resolution belongs on the request path.
///
/// The env-var arm is what distinguishes this from a bare `api_key.is_some()`: a template naming an
/// unset variable is NOT configured, which is the same judgement Pi makes.
pub fn models_json_provider_is_configured(
    models_json: &ModelFile,
    provider_id: &str,
    env: Option<&std::collections::HashMap<String, String>>,
) -> bool {
    let Some(raw) = models_json
        .providers
        .get(provider_id)
        .and_then(|c| c.api_key.as_deref())
    else {
        // Credential *acquisition* (an `oauth` block) is deliberately out of scope: Pi's
        // `composeApiKeyAuth` returns `undefined` outright for an oauth-only provider (:302).
        return false;
    };
    crate::config_value::is_command_config_value(raw)
        || crate::config_value::is_config_value_configured(raw, env)
}

/// Pi `applyModelsJson` + `modelFromJson` + the `modelOverrides` map
/// (provider-composer.ts:161-199, 124-159, 433-436), as one fallible composition over ONE provider's
/// models. Returns the provider's effective model list, or Pi's own error string.
pub(crate) fn apply_models_json(
    provider_id: &str,
    base_models: &[Model],
    config: &ProviderConfig,
) -> Result<Vec<Model>, String> {
    // `oauth` names an auth gateway, and the gateway has to live somewhere: Pi checks this FIRST,
    // ahead of the empty-block guard, so `{"oauth":"radius"}` reports the missing `baseUrl` rather
    // than the generic "must specify …" (provider-composer.ts:167-169).
    if config.oauth.is_some() && config.base_url.is_none() {
        return Err(format!(
            "Provider {provider_id}: \"baseUrl\" is required when \"oauth\" is set."
        ));
    }
    let has_overrides = !config.model_overrides.is_empty();
    if config.models.is_empty()
        && config.base_url.is_none()
        && config.headers.is_none()
        && config.compat.is_none()
        && !has_overrides
        && config.api_key.is_none()
        // `!config.oauth` (:178) — an oauth mode is itself a distinguishing key.
        && config.oauth.is_none()
        && config.auth_header.is_none()
    {
        return Err(format!(
            "Provider {provider_id}: must specify \"baseUrl\", \"headers\", \"compat\", \
             \"modelOverrides\", or \"models\"."
        ));
    }

    // Step 1: rewrite every built-in with the provider-level baseUrl + compat (:186-190).
    let mut models: Vec<Model> = base_models
        .iter()
        .map(|m| {
            let mut m = m.clone();
            // `config.oauth === "radius" ? model.baseUrl : (config.baseUrl ?? model.baseUrl)`
            // (:188): under an oauth mode the block's `baseUrl` is the auth gateway, so the models
            // keep their own request endpoints. `oauth` is single-valued, so `is_none()` is the
            // exact negation of Pi's `=== "radius"`.
            if let Some(base_url) = &config.base_url
                && config.oauth.is_none()
            {
                m.base_url = base_url.clone();
            }
            m.compat = merge_compat(m.compat.as_ref(), config.compat.as_ref());
            m
        })
        .collect();

    // Step 2: upsert each declared model (:191-197).
    for definition in &config.models {
        let existing = models.iter().position(|m| m.id.as_str() == definition.id);
        let defaults = existing.map_or_else(|| models.first(), |i| models.get(i));
        let model = model_from_json(provider_id, definition, config, defaults)?;
        match existing {
            Some(i) => {
                if let Some(slot) = models.get_mut(i) {
                    *slot = model;
                }
            }
            None => models.push(model),
        }
    }

    // Step 3: modelOverrides are the topmost user-config layer (:433-436).
    for m in &mut models {
        if let Some(ov) = config.model_overrides.get(m.id.as_str()) {
            apply_model_override(m, ov);
        }
    }
    Ok(models)
}

/// Pi `modelFromJson` (provider-composer.ts:124-159): build one `Model` from a `models.json`
/// definition, inheriting `api`/`baseUrl` from the provider block and then from the same-id built-in.
fn model_from_json(
    provider_id: &str,
    definition: &ModelDefinition,
    provider_config: &ProviderConfig,
    defaults: Option<&Model>,
) -> Result<Model, String> {
    let api = definition
        .api
        .clone()
        .or_else(|| provider_config.api.clone())
        .or_else(|| defaults.map(|d| d.api.as_str().to_string()))
        .ok_or_else(|| {
            format!(
                "Provider {provider_id}, model {}: no \"api\" specified. Set at provider or model \
                 level.",
                definition.id
            )
        })?;
    let base_url = definition
        .base_url
        .clone()
        .or_else(|| provider_config.base_url.clone())
        .or_else(|| defaults.map(|d| d.base_url.clone()))
        .ok_or_else(|| {
            format!("Provider {provider_id}: \"baseUrl\" is required when defining custom models.")
        })?;
    // `definition.contextWindow !== undefined && definition.contextWindow <= 0`
    // (provider-composer.ts:138-143 @v0.83.0) — NOT `=== 0`. CFG-046.
    if definition.context_window.is_some_and(|v| v <= 0) {
        return Err(format!(
            "Provider {provider_id}, model {}: invalid contextWindow",
            definition.id
        ));
    }
    if definition.max_tokens.is_some_and(|v| v <= 0) {
        return Err(format!(
            "Provider {provider_id}, model {}: invalid maxTokens",
            definition.id
        ));
    }
    Ok(Model {
        id: definition.id.as_str().into(),
        name: definition
            .name
            .clone()
            .unwrap_or_else(|| definition.id.clone()),
        api: api.as_str().into(),
        provider: provider_id.into(),
        base_url,
        reasoning: definition.reasoning.unwrap_or(false),
        input: definition
            .input
            .clone()
            .unwrap_or_else(|| vec![cyrup_provider::Modality::Text]),
        cost: definition.cost.clone().unwrap_or_default(),
        // Both are guaranteed `> 0` by the checks above, so the cast is total.
        context_window: definition.context_window.map_or(128_000, |v| v as u64),
        max_tokens: definition.max_tokens.map_or(16_384, |v| v as u64),
        // `samplingParams: definition.samplingParams` (provider-composer.ts:158 @v0.84.1) — copied
        // verbatim, with NO fallback to `providerConfig` or `defaults`: the provider block has no
        // `samplingParams` key in `ProviderConfigSchema`, and a same-id built-in's defaults are
        // deliberately not inherited here. CFG-039.
        sampling_params: definition.sampling_params.clone(),
        thinking_level_map: definition.thinking_level_map.clone(),
        // Pi sets `headers: undefined` on the composed model — `models.json` headers are REQUEST
        // config resolved separately through `resolveConfiguredModelHeaders` (:156, :501-511), so
        // they never leak into the credential-blind snapshot. cyrup's counterpart of that separate
        // resolution is [`crate::provider_compose::raw_model_headers`], applied per request in
        // `ConfiguredApiKeyAuth::resolve`; without it the declared header would be inert.
        headers: None,
        compat: merge_compat(provider_config.compat.as_ref(), definition.compat.as_ref()),
    })
}

/// Pi `applyModelOverride` (provider-composer.ts): patch a composed model with a `modelOverrides`
/// entry. Every field is individually optional; an absent field leaves the model unchanged.
fn apply_model_override(model: &mut Model, ov: &ModelOverride) {
    if let Some(name) = &ov.name {
        model.name = name.clone();
    }
    if let Some(r) = ov.reasoning {
        model.reasoning = r;
    }
    // Pi `:104-106`: `override.thinkingLevelMap ? { ...model.thinkingLevelMap,
    // ...override.thinkingLevelMap } : model.thinkingLevelMap` — a PARTIAL override patches the
    // named levels and keeps the model's other entries. Replacing the map wholesale would silently
    // change what every unmentioned thinking level sends on the wire. (The `modelFromJson` path is
    // different and correct as written: a model DEFINITION's map is used verbatim, `:141`.)
    if let Some(map) = &ov.thinking_level_map {
        let mut merged = model.thinking_level_map.clone().unwrap_or_default();
        for (level, value) in map {
            merged.insert(level.clone(), value.clone());
        }
        model.thinking_level_map = Some(merged);
    }
    if let Some(input) = &ov.input {
        model.input = input.clone();
    }
    // `contextWindow: override.contextWindow ?? model.contextWindow` (provider-composer.ts:118-119
    // @v0.83.0) — the override path has NO positivity check, unlike `modelFromJson`'s.
    //
    // [CYRUP-DELTA] pi stores a negative override verbatim (JS `number`); `Model::context_window`
    // is `u64`, so a negative value saturates to 0 here rather than wrapping. Upstream's own
    // behaviour on a negative override is an unguarded hole (a negative window reaches the request
    // builder), and reproducing the wrap would be strictly worse than reproducing the intent.
    if let Some(cw) = ov.context_window {
        model.context_window = cw.max(0) as u64;
    }
    if let Some(mt) = ov.max_tokens {
        model.max_tokens = mt.max(0) as u64;
    }
    // Pi `:123-125` @v0.84.1: `override.samplingParams ? { ...model.samplingParams,
    // ...override.samplingParams } : model.samplingParams`. This is a per-key MERGE, not a
    // replacement — the same shape as `thinkingLevelMap` above and unlike every other field here —
    // so an override naming only `top_p` must leave a model-level `top_k` in place. CFG-039.
    if let Some(params) = &ov.sampling_params {
        let mut merged = model.sampling_params.clone().unwrap_or_default();
        for (key, value) in params {
            merged.insert(key.clone(), value.clone());
        }
        model.sampling_params = Some(merged);
    }
    if let Some(cost) = &ov.cost {
        if let Some(v) = cost.input {
            model.cost.input = v;
        }
        if let Some(v) = cost.output {
            model.cost.output = v;
        }
        if let Some(v) = cost.cache_read {
            model.cost.cache_read = v;
        }
        if let Some(v) = cost.cache_write {
            model.cost.cache_write = v;
        }
        if let Some(t) = &cost.tiers {
            model.cost.tiers = Some(t.clone());
        }
    }
    if let Some(compat) = &ov.compat {
        model.compat = merge_compat(model.compat.as_ref(), Some(compat));
    }
}

/// The three `compat` members Pi deep-merges instead of replacing (`mergeCompat`,
/// provider-composer.ts:87). Spelled in Pi's own wire (camelCase) form, because [`merge_compat`]
/// merges over the serialized JSON — the same names the file on disk uses.
const NESTED_COMPAT_KEYS: [&str; 3] = [
    "openRouterRouting",
    "vercelGatewayRouting",
    "chatTemplateKwargs",
];

/// Pi `mergeCompat` (provider-composer.ts:78-96): the more specific layer wins per field, EXCEPT
/// that the three object-valued members in [`NESTED_COMPAT_KEYS`] are themselves merged one level
/// deep. Either side may be absent.
///
/// Both halves matter and Pi writes them as two passes:
/// 1. `{ ...base, ...override }` — implemented over the serialized form so every present key of
///    `over`, and only the present keys, lands on `base`;
/// 2. the nested pass (`:87-95`) — for each of the three keys, `{ ...baseValue, ...overrideValue }`,
///    so declaring e.g. `"openRouterRouting": { "zdr": true }` in `models.json` KEEPS the built-in's
///    other routing fields instead of replacing the object wholesale (which would silently change
///    the wire payload).
///
/// Pi's guard is `typeof value === "object" && value !== null` on EITHER side. When only one side is
/// an object the spread reduces to that side, which pass 1 has already produced, so pass 2 below
/// only has to act when both sides are objects. The one input where this differs from Pi is a
/// non-object scalar overriding an object (Pi's spread would index the scalar's characters into the
/// merged object, producing `{0:"x",…}` garbage that cannot deserialize); cyrup keeps the override
/// scalar, which is pass 1's result.
fn merge_compat(
    base: Option<&cyrup_provider::api::compat::OpenAiCompletionsCompat>,
    over: Option<&cyrup_provider::api::compat::OpenAiCompletionsCompat>,
) -> Option<cyrup_provider::api::compat::OpenAiCompletionsCompat> {
    match (base, over) {
        (b, None) => b.cloned(),
        (None, Some(o)) => Some(o.clone()),
        (Some(b), Some(o)) => {
            let (Ok(serde_json::Value::Object(mut bm)), Ok(serde_json::Value::Object(om))) =
                (serde_json::to_value(b), serde_json::to_value(o))
            else {
                return Some(o.clone());
            };
            // Capture both sides of the nested keys BEFORE the shallow spread overwrites them.
            let nested: Vec<(&str, Option<serde_json::Value>, Option<serde_json::Value>)> =
                NESTED_COMPAT_KEYS
                    .iter()
                    .map(|k| (*k, bm.get(*k).cloned(), om.get(*k).cloned()))
                    .collect();
            for (k, v) in om {
                bm.insert(k, v);
            }
            for (key, base_value, over_value) in nested {
                let (Some(base_obj), Some(over_obj)) = (
                    base_value.as_ref().and_then(serde_json::Value::as_object),
                    over_value.as_ref().and_then(serde_json::Value::as_object),
                ) else {
                    continue;
                };
                let mut merged = base_obj.clone();
                for (k, v) in over_obj {
                    merged.insert(k.clone(), v.clone());
                }
                bm.insert(key.to_string(), serde_json::Value::Object(merged));
            }
            serde_json::from_value(serde_json::Value::Object(bm))
                .map_or_else(|_| Some(o.clone()), Some)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
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
            sampling_params: None,
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

    /// PROV-002: `max` is a valid level token (Pi `VALID_THINKING_LEVELS`, args.ts:59), so a
    /// `model:max` shorthand must SPLIT. Before the fix `parse_thinking_level("max")` returned
    /// `None`, so `:max` was swallowed into the model id and the pattern failed to resolve.
    #[test]
    fn max_thinking_shorthand_parses() {
        assert_eq!(parse_thinking_level("max"), Some(ModelThinkingLevel::Max));
        assert_eq!(parse_thinking_level("MAX"), Some(ModelThinkingLevel::Max));
        assert_eq!(parse_thinking_level("bogus"), None);

        let models = vec![model("anthropic", "claude-opus-4-6", "Claude Opus 4.6")];
        let r = ModelResolver::new(&models);
        let parsed = r.parse_pattern("claude-opus-4-6:max", true);
        assert_eq!(
            parsed.model.as_ref().expect("model resolves").id.as_str(),
            "claude-opus-4-6"
        );
        assert_eq!(parsed.thinking_level, Some(ModelThinkingLevel::Max));

        // …and on the glob/scope path too.
        let scoped = r.resolve_scope(&["anthropic/*:max".to_string()]);
        assert!(!scoped.is_empty());
        assert!(
            scoped
                .iter()
                .all(|s| s.thinking_level == Some(ModelThinkingLevel::Max))
        );
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
        let found = r
            .find_exact("shared")
            .expect("ambiguous bare id resolves, never errors");
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
        assert_eq!(
            scoped.len(),
            1,
            "Pi resolves an ambiguous bare id to 1 model"
        );
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

    /// **G16/G42.** The two Qwen Token Plan parents are `KnownProvider`s at v0.83.0
    /// (`ai/src/types.ts:67-68`) and carry a curated default (`model-resolver.ts:47-48`); cyrup's
    /// table had neither, so `--provider qwen-token-plan` fell through `default_model_per_provider`
    /// to `None`.
    ///
    /// The user action: a `models.json` that declares a `qwen-token-plan` provider block (R-07-023 —
    /// the only way to reach these two today, since the built-in registration is still blocked on
    /// catalog data that has never existed in pi's git history), then
    /// `cyrup --provider qwen-token-plan --model <an id the block does not list>`. Pi's
    /// `buildFallbackModel` clones the provider's CURATED default to carry its api/compat/window
    /// onto the custom id; with no table entry it cloned whichever model happened to be first.
    #[test]
    fn qwen_token_plan_custom_id_clones_the_curated_default_not_the_first_model() {
        // A models.json block listing the plan's models in catalog order — `MiniMax-M2.5` sorts
        // first and is exactly the wrong base: it is the ONE model of the fifteen that pi's own
        // `qwen-token-plan-models.test.ts` excludes from the Qwen thinking set.
        let mut minimax = model("qwen-token-plan", "MiniMax-M2.5", "MiniMax M2.5");
        minimax.reasoning = false;
        minimax.context_window = 200_000;
        let mut curated = model("qwen-token-plan", "qwen3.7-max", "Qwen3.7 Max");
        curated.context_window = 1_000_000;
        let available = vec![minimax, curated];

        let built = build_fallback_model("qwen-token-plan", "qwen3.9-max", &available)
            .expect("a provider with models must yield a fallback");
        assert_eq!(built.id.as_str(), "qwen3.9-max");
        assert_eq!(
            built.context_window, 1_000_000,
            "the clone base must be the curated qwen3.7-max, not the first-listed MiniMax-M2.5"
        );
        assert!(
            built.reasoning,
            "…and must therefore inherit the curated model's reasoning flag"
        );

        // Both regions name the SAME default (`model-resolver.ts:47-48`), and both must be known.
        assert_eq!(
            default_model_per_provider("qwen-token-plan"),
            Some("qwen3.7-max")
        );
        assert_eq!(
            default_model_per_provider("qwen-token-plan-cn"),
            Some("qwen3.7-max")
        );
        assert!(
            KNOWN_PROVIDERS.contains(&"qwen-token-plan")
                && KNOWN_PROVIDERS.contains(&"qwen-token-plan-cn"),
            "an entry absent from KNOWN_PROVIDERS is never scanned by first_default_or_first"
        );
    }

    /// MIRROR — the scan ORDER. `first_default_or_first` returns the first KNOWN_PROVIDERS entry
    /// with an available match, so inserting the qwen keys anywhere but pi's `Object.keys` position
    /// would silently re-rank every other provider's claim on the initial model. Pi's
    /// `defaultModelPerProvider` puts all THREE qwen keys — `qwen-token-plan`,
    /// `qwen-token-plan-cn`, then `qwen-token-plan-individual` — between `cloudflare-ai-gateway`
    /// and `xiaomi` (`model-resolver.ts:53-57`).
    #[test]
    fn mirror_qwen_keys_sit_where_pi_puts_them_in_the_scan_order() {
        let pos = |id: &str| KNOWN_PROVIDERS.iter().position(|p| *p == id);
        let (gateway, qwen, qwen_cn, qwen_individual, xiaomi) = (
            pos("cloudflare-ai-gateway").unwrap(),
            pos("qwen-token-plan").unwrap(),
            pos("qwen-token-plan-cn").unwrap(),
            pos("qwen-token-plan-individual").unwrap(),
            pos("xiaomi").unwrap(),
        );
        assert_eq!(qwen, gateway + 1);
        assert_eq!(qwen_cn, qwen + 1);
        assert_eq!(qwen_individual, qwen_cn + 1);
        assert_eq!(xiaomi, qwen_individual + 1);

        // And the consequence: with BOTH an xiaomi and a qwen default available, qwen wins.
        let available = vec![
            model("xiaomi", "mimo-v2.5-pro", "MiMo"),
            model("qwen-token-plan", "qwen3.7-max", "Qwen3.7 Max"),
        ];
        let chosen = first_default_or_first(&available).unwrap();
        assert_eq!(chosen.provider.as_str(), "qwen-token-plan");
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
            r.resolve_scope(&["cloudflare-workers-ai/*".to_string()])
                .len(),
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

    // ---- models.json composition (CFG-002) --------------------------------------------------

    fn oai(provider: &str, id: &str) -> Model {
        Model {
            id: id.into(),
            name: id.to_string(),
            api: ApiId::from("openai-completions"),
            provider: provider.into(),
            base_url: "https://builtin.example/v1".into(),
            reasoning: false,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 128_000,
            max_tokens: 16_384,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    #[test]
    fn models_json_jsonc_comments_and_trailing_commas_are_stripped() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            "{\n  // leading comment\n  \"providers\": {\n    \"acme\": {\n      \"baseUrl\": \"https://acme.test/v1\", // trailing note\n      \"models\": [{ \"id\": \"a1\" },]\n    },\n  }\n}\n",
        )
        .unwrap();
        let file = load_models_file(&path)
            .expect("JSONC models.json must parse like Pi's stripJsonComments");
        assert_eq!(file.providers.len(), 1);
        assert_eq!(file.providers["acme"].models.len(), 1);
        // A `//` sequence INSIDE a string literal survives.
        std::fs::write(
            &path,
            r#"{"providers":{"acme":{"baseUrl":"https://acme.test/v1"}}}"#,
        )
        .unwrap();
        let file = load_models_file(&path).unwrap();
        assert_eq!(
            file.providers["acme"].base_url.as_deref(),
            Some("https://acme.test/v1")
        );
    }

    #[test]
    fn models_json_upserts_a_custom_model_and_rewrites_the_builtin_base_url() {
        let base = vec![oai("acme", "old")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"acme":{"baseUrl":"https://proxy.test/v1","models":[{"id":"new","name":"New"}]}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert!(errors.is_empty(), "{errors:?}");
        let old = out
            .iter()
            .find(|m| m.id.as_str() == "old")
            .expect("built-in kept");
        assert_eq!(
            old.base_url, "https://proxy.test/v1",
            "baseUrl rewrites the built-in"
        );
        let new = out
            .iter()
            .find(|m| m.id.as_str() == "new")
            .expect("custom model added");
        assert_eq!(new.name, "New");
        assert_eq!(
            new.api.as_str(),
            "openai-completions",
            "api inherits from the built-in defaults"
        );
        assert_eq!(new.base_url, "https://proxy.test/v1");
    }

    #[test]
    fn models_json_model_overrides_patch_a_builtin_last() {
        let base = vec![oai("acme", "m1")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"acme":{"modelOverrides":{"m1":{"name":"Renamed","contextWindow":42,"cost":{"input":1.5}}}}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert!(errors.is_empty(), "{errors:?}");
        let m = out.iter().find(|m| m.id.as_str() == "m1").unwrap();
        assert_eq!(m.name, "Renamed");
        assert_eq!(m.context_window, 42);
        assert!((m.cost.input - 1.5).abs() < f64::EPSILON);
        // Untouched fields survive the patch.
        assert_eq!(m.max_tokens, 16_384);
    }

    #[test]
    fn a_rejected_provider_block_keeps_its_builtins_and_reports() {
        // No distinguishing key at all — Pi throws (provider-composer.ts:181-184).
        let base = vec![oai("acme", "m1")];
        let file: ModelFile =
            serde_json::from_str(r#"{"providers":{"acme":{"name":"Acme"}}}"#).unwrap();
        let (out, errors) = file.compose(&base);
        assert_eq!(errors.len(), 1, "the bad block is reported");
        assert!(errors[0].contains("must specify"), "{errors:?}");
        assert!(
            out.iter().any(|m| m.id.as_str() == "m1"),
            "the built-ins survive a rejected block"
        );
    }

    #[test]
    fn a_custom_model_with_no_resolvable_base_url_is_rejected_loudly() {
        // No built-ins to inherit from, no provider baseUrl → Pi throws (provider-composer.ts:137).
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"ghost":{"api":"openai-completions","models":[{"id":"x"}]}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&[]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("baseUrl"), "{errors:?}");
        assert!(out.is_empty());
    }

    /// CFG-019 + CFG-041: `defaultModelPerProvider` must equal pi v0.84.1's 40 entries key for key
    /// AND in order — `Object.keys(defaultModelPerProvider)` IS the launch scan order at step 4
    /// (`model-resolver.ts:683-692` @v0.84.1), so a missing or misplaced key changes which model a
    /// user launches on.
    ///
    /// Red at HEAD: 37 entries; `xai` was the retired `grok-4.20-0309-reasoning`; `radius`,
    /// `baseten` and `qwen-token-plan-individual` were absent entirely.
    #[test]
    fn default_model_per_provider_matches_pi_v0_84_1_key_for_key_and_in_order() {
        // `git show v0.84.1:packages/coding-agent/src/core/model-resolver.ts`, `:20-61`.
        const PI: &[(&str, &str)] = &[
            ("amazon-bedrock", "us.anthropic.claude-opus-4-6-v1"),
            ("ant-ling", "Ring-2.6-1T"),
            ("anthropic", "claude-opus-4-8"),
            ("openai", "gpt-5.5"),
            ("azure-openai-responses", "gpt-5.4"),
            ("openai-codex", "gpt-5.5"),
            ("radius", "auto"),
            ("nvidia", "nvidia/nemotron-3-super-120b-a12b"),
            ("deepseek", "deepseek-v4-pro"),
            ("google", "gemini-3.1-pro-preview"),
            ("google-vertex", "gemini-3.1-pro-preview"),
            ("github-copilot", "gpt-5.4"),
            ("openrouter", "moonshotai/kimi-k2.6"),
            ("vercel-ai-gateway", "zai/glm-5.1"),
            ("xai", "grok-4.5"),
            ("groq", "openai/gpt-oss-120b"),
            ("cerebras", "zai-glm-4.7"),
            ("zai", "glm-5.1"),
            ("zai-coding-cn", "glm-5.1"),
            ("mistral", "devstral-medium-latest"),
            ("minimax", "MiniMax-M2.7"),
            ("minimax-cn", "MiniMax-M2.7"),
            ("moonshotai", "kimi-k2.6"),
            ("moonshotai-cn", "kimi-k2.6"),
            ("huggingface", "moonshotai/Kimi-K2.6"),
            ("fireworks", "accounts/fireworks/models/kimi-k2p6"),
            ("together", "moonshotai/Kimi-K2.6"),
            ("baseten", "zai-org/GLM-5.2"),
            ("opencode", "kimi-k2.6"),
            ("opencode-go", "kimi-k2.6"),
            ("kimi-coding", "kimi-for-coding"),
            ("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
            (
                "cloudflare-ai-gateway",
                "workers-ai/@cf/moonshotai/kimi-k2.6",
            ),
            ("qwen-token-plan", "qwen3.7-max"),
            ("qwen-token-plan-cn", "qwen3.7-max"),
            ("qwen-token-plan-individual", "qwen3.8-max"),
            ("xiaomi", "mimo-v2.5-pro"),
            ("xiaomi-token-plan-cn", "mimo-v2.5-pro"),
            ("xiaomi-token-plan-ams", "mimo-v2.5-pro"),
            ("xiaomi-token-plan-sgp", "mimo-v2.5-pro"),
        ];
        let ours: Vec<(&str, &str)> = KNOWN_PROVIDERS
            .iter()
            .map(|p| (*p, default_model_per_provider(p).unwrap_or("<missing>")))
            .collect();
        assert_eq!(ours, PI.to_vec());
        assert_eq!(KNOWN_PROVIDERS.len(), 40);
    }

    /// CFG-023: step 3 accepts the saved default ONLY when its provider has configured auth
    /// (`if (found && modelRuntime.hasConfiguredAuth(found.provider))`, model-resolver.ts:621-630
    /// @v0.83.0), otherwise falling through to step 4 (`:632`).
    ///
    /// Red at HEAD: step 3 returned the saved default unconditionally, so a user who removed a
    /// provider's credentials kept launching into it and got an auth error per turn.
    #[test]
    fn saved_default_is_skipped_when_its_provider_has_no_configured_auth() {
        let all = vec![
            model("anthropic", "claude-opus-4-8", "Claude Opus"),
            model("openai", "gpt-5.5", "GPT 5.5"),
        ];
        let available = vec![model("openai", "gpt-5.5", "GPT 5.5")];
        let has_auth = |m: &Model| m.provider.as_str() == "openai";

        let r = find_initial_model(
            None,
            None,
            &[],
            false,
            Some("anthropic"),
            Some("claude-opus-4-8"),
            None,
            &all,
            &available,
            &has_auth,
        );
        // Step 4's curated default for the only configured provider, NOT the saved anthropic one.
        assert_eq!(r.model.as_ref().unwrap().provider.as_str(), "openai");

        // With auth present the saved default still wins.
        let has_auth_all = |_: &Model| true;
        let r = find_initial_model(
            None,
            None,
            &[],
            false,
            Some("anthropic"),
            Some("claude-opus-4-8"),
            None,
            &all,
            &available,
            &has_auth_all,
        );
        assert_eq!(r.model.as_ref().unwrap().provider.as_str(), "anthropic");
    }

    /// CFG-018: the glob branch tries `findExactModelReferenceMatch` BEFORE minimatch
    /// (`model-resolver.ts:297-303` @v0.83.0), so an id carrying a glob metacharacter resolves to
    /// itself. CFG-008: the same call now reports pi's `no-match` / `invalid-thinking-level`
    /// diagnostics (`:316`, `:334`, `:243`).
    ///
    /// Red at HEAD: `resolve_scope` went straight to the filter (so `qwen[chat]` matched nothing)
    /// and returned a bare `Vec`, dropping every diagnostic.
    #[test]
    fn glob_scope_short_circuits_on_an_exact_reference_and_reports_diagnostics() {
        let models = vec![
            model("qwen", "qwen[chat]", "Qwen Chat"),
            model("anthropic", "claude-opus-4-8", "Claude Opus"),
        ];
        let r = ModelResolver::new(&models);

        let out = r.resolve_scope_reporting(&["qwen/qwen[chat]".to_string()]);
        assert_eq!(out.models.len(), 1);
        assert_eq!(out.models[0].model.id.as_str(), "qwen[chat]");
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);

        let out = r.resolve_scope_reporting(&["anthorpic/*".to_string()]);
        assert!(out.models.is_empty());
        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0].code, ModelScopeDiagnosticCode::NoMatch);
        assert_eq!(
            out.diagnostics[0].message,
            "No models match pattern \"anthorpic/*\""
        );

        let out = r.resolve_scope_reporting(&["claude-opus-4-8:bogus".to_string()]);
        assert_eq!(out.models.len(), 1);
        assert_eq!(
            out.diagnostics[0].code,
            ModelScopeDiagnosticCode::InvalidThinkingLevel
        );
        assert_eq!(
            out.diagnostics[0].message,
            "Invalid thinking level \"bogus\" in pattern \"claude-opus-4-8:bogus\". Using default instead."
        );
    }

    /// CFG-046 + CFG-043: pi types `name`/`baseUrl`/`apiKey`/`api` as
    /// `Type.Optional(Type.String({ minLength: 1 }))` (model-config.ts:188-198 @v0.83.0), so an
    /// empty string FAILS `validateModelsConfig.Check` and `ModelConfig.load` returns an empty
    /// provider map plus `Invalid models.json schema:` with one `  - <dotted.path>: <message>` line
    /// per failure (`:272-279`) — a heading distinct from the JSON-syntax one.
    ///
    /// Red at HEAD: no length check anywhere, so `"baseUrl": ""` composed every model of that
    /// provider onto an empty endpoint while the file was reported as VALID; and a wrong-typed
    /// field surfaced as serde's byte-offset message under `Failed to parse models.json`.
    #[test]
    fn models_json_schema_failures_are_reported_per_field_not_as_a_parse_error() {
        let dir = crate::test_util::temp_dir();

        let path = dir.join("empty-base-url.json");
        std::fs::write(&path, r#"{"providers":{"x":{"baseUrl":""}}}"#).unwrap();
        let (file, err) = load_models_file_reporting(&path);
        assert!(file.providers.is_empty());
        let err = err.expect("an empty baseUrl must be a schema failure");
        assert!(err.starts_with("Invalid models.json schema:"), "{err}");
        assert!(
            err.contains("  - providers.x.baseUrl: Expected string length greater or equal to 1"),
            "{err}"
        );

        let path = dir.join("wrong-type.json");
        std::fs::write(
            &path,
            r#"{"providers":{"mycorp":{"models":[{"id":"m","contextWindow":"big"}]}}}"#,
        )
        .unwrap();
        let (_file, err) = load_models_file_reporting(&path);
        let err = err.expect("a wrong-typed field must be a schema failure");
        assert!(err.starts_with("Invalid models.json schema:"), "{err}");
        assert!(
            err.contains("providers.mycorp.models.0.contextWindow: Expected number"),
            "{err}"
        );

        // A JSON SYNTAX error keeps its own distinct heading (model-config.ts:265-270).
        let path = dir.join("syntax.json");
        std::fs::write(&path, "{ not json").unwrap();
        let (_file, err) = load_models_file_reporting(&path);
        assert!(
            err.unwrap().starts_with("Failed to parse models.json"),
            "syntax errors must not be relabelled as schema errors"
        );
    }

    /// CFG-046, composition half: `definition.contextWindow <= 0` — not `=== 0` —
    /// (provider-composer.ts:138-143 @v0.83.0), rejecting ONLY that provider block. A custom model
    /// with an empty inherited `baseUrl` must still hit pi's
    /// `"baseUrl" is required when defining custom models.`
    #[test]
    fn a_non_positive_context_window_rejects_only_its_own_provider_block() {
        let base = vec![model("anthropic", "claude-opus-4-8", "Claude Opus")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{
                 "mycorp":{"baseUrl":"https://x","api":"openai-completions",
                           "models":[{"id":"m","contextWindow":-1}]},
                 "anthropic":{"baseUrl":"https://ok"}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("invalid contextWindow"), "{errors:?}");
        // The good block still composed.
        assert!(out.iter().any(|m| m.base_url == "https://ok"));
    }

    #[test]
    fn malformed_models_json_reports_instead_of_erroring_out() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        std::fs::write(&path, "{ not json").unwrap();
        let (file, err) = load_models_file_reporting(&path);
        assert!(file.providers.is_empty());
        let err = err.expect("a parse failure must be reported");
        assert!(err.contains("Failed to parse models.json"), "{err}");
        // A missing file is NOT an error (Pi returns an empty snapshot on ENOENT, model-config.ts:248).
        let (file, err) = load_models_file_reporting(&dir.join("absent.json"));
        assert!(file.providers.is_empty() && err.is_none());
    }

    /// CFG-022 — the ONE `hasConfiguredAuth` predicate, over an `auth.json` that does not exist.
    ///
    /// Pi fills `configuredProviders` by running `checkAuth` over every COMPOSED provider
    /// (model-runtime.ts:372-374), so a provider declared only in `models.json` is configured on the
    /// strength of its own `apiKey`. cyrup's launch path consulted the credential store alone, which
    /// knows only `--api-key`, an `auth.json` entry and the `env_keys` table of KNOWN provider ids —
    /// none of which a user-declared provider can match.
    #[test]
    fn models_json_api_key_configures_a_provider_with_no_stored_credential() {
        let dir = crate::test_util::temp_dir();
        let auth = crate::auth::AuthStore::at(dir.join("auth.json"));
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{
                 "mycorp":  {"baseUrl":"https://g.test/v1","apiKey":"sk-literal","models":[{"id":"m"}]},
                 "keyless": {"baseUrl":"https://k.test/v1","models":[{"id":"m"}]}
               }}"#,
        )
        .unwrap();

        assert!(provider_is_configured(
            &auth,
            &file,
            &ProviderId::from("mycorp"),
            None
        ));
        assert!(
            !provider_is_configured(&auth, &file, &ProviderId::from("keyless"), None),
            "a baseUrl-only overlay carries no credential of its own"
        );
        assert!(
            !provider_is_configured(&auth, &file, &ProviderId::from("absent"), None),
            "a provider the file does not mention is not configured"
        );
    }

    /// The env-var arm of Pi's check (provider-composer.ts:322-328): a `$VAR` template is configured
    /// exactly when every name it references is defined. A bare `api_key.is_some()` would call the
    /// unset case configured.
    #[test]
    fn a_models_json_api_key_template_needs_its_env_vars_defined() {
        let dir = crate::test_util::temp_dir();
        let auth = crate::auth::AuthStore::at(dir.join("auth.json"));
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"mycorp":{"baseUrl":"https://g.test/v1","apiKey":"${MYCORP_TOKEN}",
                 "models":[{"id":"m"}]}}}"#,
        )
        .unwrap();
        let provider = ProviderId::from("mycorp");

        let empty = std::collections::HashMap::new();
        assert!(
            !provider_is_configured(&auth, &file, &provider, Some(&empty)),
            "MYCORP_TOKEN is not defined, so the key is not configured"
        );

        let mut env = std::collections::HashMap::new();
        env.insert("MYCORP_TOKEN".to_string(), "sk-from-env".to_string());
        assert!(provider_is_configured(&auth, &file, &provider, Some(&env)));
    }

    /// A `!command` `apiKey` counts as configured on the strength of BEING a command
    /// (`isCommandConfigValue`, provider-composer.ts:321) — the command must NOT run. This predicate
    /// is a status query called inside filter loops; resolving here would execute a shell command
    /// written in `models.json`.
    #[test]
    fn a_command_api_key_is_configured_without_ever_being_executed() {
        let dir = crate::test_util::temp_dir();
        let auth = crate::auth::AuthStore::at(dir.join("auth.json"));
        let marker = dir.join("executed-marker");
        let file: ModelFile = serde_json::from_str(&format!(
            r#"{{"providers":{{"mycorp":{{"baseUrl":"https://g.test/v1",
                 "apiKey":"!touch {}","models":[{{"id":"m"}}]}}}}}}"#,
            marker.display()
        ))
        .unwrap();

        assert!(provider_is_configured(
            &auth,
            &file,
            &ProviderId::from("mycorp"),
            None
        ));
        assert!(
            !marker.exists(),
            "the status predicate executed the `apiKey` command — it must never resolve the value"
        );
    }

    /// CFG-002 — the `oauth` half of `applyModelsJson` (provider-composer.ts:167-169, :178, :188).
    ///
    /// `oauth` names an auth GATEWAY, so Pi rejects a block that sets it without the `baseUrl` that
    /// gateway lives at, counts it as a distinguishing key in the empty-block guard, and — because
    /// the gateway URL is an auth endpoint rather than a request endpoint — does NOT let it rewrite
    /// the built-in models' `baseUrl`. cyrup modelled none of that: the key was not even a field, so
    /// serde dropped it silently.
    #[test]
    fn models_json_oauth_requires_a_base_url() {
        let base = vec![oai("acme", "m1")];
        let file: ModelFile =
            serde_json::from_str(r#"{"providers":{"acme":{"oauth":"radius"}}}"#).unwrap();
        let (out, errors) = file.compose(&base);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(
            errors[0], r#"Provider acme: "baseUrl" is required when "oauth" is set."#,
            "Pi's exact text, provider-composer.ts:168"
        );
        assert!(
            out.iter().any(|m| m.id.as_str() == "m1"),
            "a rejected block still keeps the provider's built-ins"
        );
    }

    /// `oauth` alone (with its required `baseUrl`) is a COMPLETE block — Pi's empty-block guard
    /// carries a `!config.oauth` term (provider-composer.ts:178) that cyrup omitted, so cyrup
    /// rejected it with the misleading `must specify "baseUrl", "headers", …` message.
    #[test]
    fn models_json_oauth_satisfies_the_empty_block_guard_without_rewriting_base_urls() {
        let base = vec![oai("acme", "m1")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"acme":{"oauth":"radius","baseUrl":"https://gateway.acme.test/v1"}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert!(
            errors.is_empty(),
            "an oauth block is a distinguishing key: {errors:?}"
        );
        let m = out
            .iter()
            .find(|m| m.id.as_str() == "m1")
            .expect("built-in kept");
        assert_eq!(
            m.base_url, "https://builtin.example/v1",
            "with `oauth` set the provider baseUrl is the AUTH gateway and must not become the \
             request endpoint (provider-composer.ts:188)"
        );
    }

    /// Without `oauth`, the very same `baseUrl` DOES rewrite the built-ins — the guard above must
    /// not weaken the ordinary proxy-override path.
    #[test]
    fn models_json_base_url_still_rewrites_builtins_without_oauth() {
        let base = vec![oai("acme", "m1")];
        let file: ModelFile = serde_json::from_str(
            r#"{"providers":{"acme":{"baseUrl":"https://gateway.acme.test/v1"}}}"#,
        )
        .unwrap();
        let (out, errors) = file.compose(&base);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(out[0].base_url, "https://gateway.acme.test/v1");
    }

    /// Pi types `oauth` as `Type.Literal("radius")` (model-config.ts:194), so any other spelling is
    /// a SCHEMA failure that empties the whole file and reports one error (model-config.ts:265-272)
    /// — not a silently-ignored key. cyrup's serde loader reaches the same contract through
    /// `load_models_file_reporting`.
    #[test]
    fn models_json_rejects_an_unknown_oauth_mode_for_the_whole_file() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            r#"{"providers":{"acme":{"oauth":"anthropic","baseUrl":"https://x.test/v1"}}}"#,
        )
        .unwrap();
        let (file, err) = load_models_file_reporting(&path);
        assert!(
            file.providers.is_empty(),
            "an invalid schema empties the file"
        );
        let err = err.expect("and reports why");
        assert!(
            err.contains("radius"),
            "the message names the legal value: {err}"
        );
    }
}
