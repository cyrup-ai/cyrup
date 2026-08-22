//! The `minimatch`-compatible glob engine `resolve_scope` filters with: path-segment `*`/`?`/
//! `[...]`, globstar `**`, brace lists and ranges, backslash escaping, the dot-rule, and the
//! full extglob set. Depends on nothing but `std`.

/// Input bounds. This matcher is reachable from a user-supplied `--models` pattern
/// ([`super::resolver`] enters the glob branch on any `*`, `?` or `[`), and both its recursions and
/// its brace expansion are otherwise unbounded. Every limit here FAILS CLOSED — an over-large input
/// yields `false` or a truncated expansion, never a panic and never a `Result` — because
/// [`glob_match`] returns `bool` and has no error channel. All four sit orders of magnitude above
/// any real model pattern, so no case in the captured Pi/minimatch parity table is affected.
///
/// Longest pattern accepted. Bounds [`m`]'s recursion transitively: `m` recurses at most once per
/// pattern character, so a bounded pattern is a bounded stack.
const MAX_PATTERN_LEN: usize = 4096;
/// Deepest `{...}` nesting expanded. Beyond it the brace is left literal, exactly as an unbalanced
/// brace already is.
const MAX_BRACE_DEPTH: usize = 32;
/// Ceiling on strings produced by one brace expansion, capping the sibling cross-product.
const MAX_EXPANSIONS: usize = 4096;
/// Ceiling on items from one `{a..b}` range.
const MAX_RANGE_ITEMS: usize = 4096;

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
pub(super) fn glob_match(pattern: &str, text: &str) -> bool {
    // Fail closed on a pattern too large to match safely (see [`MAX_PATTERN_LEN`]).
    if pattern.len() > MAX_PATTERN_LEN {
        return false;
    }
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
    brace_expand_at(s, 0)
}

fn brace_expand_at(s: &str, depth: usize) -> Vec<String> {
    // Past the depth bound the brace is left literal — the same fallback an unbalanced brace takes.
    if depth >= MAX_BRACE_DEPTH {
        return vec![s.to_string()];
    }
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
    let post_expanded = brace_expand_at(&post, depth + 1);

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
    'expand: for opt in options {
        for opt_expanded in brace_expand_at(&opt, depth + 1) {
            for tail in &post_expanded {
                if out.len() >= MAX_EXPANSIONS {
                    break 'expand;
                }
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
                if out.len() >= MAX_RANGE_ITEMS {
                    break;
                }
                out.push(format_range_num(x, padded, width));
                match x.checked_add(step) {
                    Some(next) => x = next,
                    None => break,
                }
            }
        } else {
            while x >= b {
                if out.len() >= MAX_RANGE_ITEMS {
                    break;
                }
                out.push(format_range_num(x, padded, width));
                match x.checked_sub(step) {
                    Some(next) => x = next,
                    None => break,
                }
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
                if out.len() >= MAX_RANGE_ITEMS {
                    break;
                }
                out.push(
                    char::from_u32(x as u32)
                        .map(String::from)
                        .unwrap_or_default(),
                );
                match x.checked_add(step) {
                    Some(next) => x = next,
                    None => break,
                }
            }
        } else {
            while x >= b {
                if out.len() >= MAX_RANGE_ITEMS {
                    break;
                }
                out.push(
                    char::from_u32(x as u32)
                        .map(String::from)
                        .unwrap_or_default(),
                );
                match x.checked_sub(step) {
                    Some(next) => x = next,
                    None => break,
                }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

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
        let fixture = include_str!("../testdata/glob_minimatch.json");
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
}
