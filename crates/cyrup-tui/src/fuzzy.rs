//! Fuzzy matching — a faithful port of `pi/packages/tui/src/fuzzy.ts` (spec/tui/04 §3.3).
//!
//! Matches when all query characters appear in order (not necessarily consecutive). **Lower score is
//! a better match** — exactly mirroring Pi, because the resulting order is user-visible in the
//! autocomplete popup and every fuzzy selector. The scorer reproduces Pi's constants verbatim:
//! consecutive-run reward (`-5·run`), gap penalty (`+2·gap`), word-boundary bonus (`-10`), a slight
//! later-match penalty (`+0.1·i`), and the whole-string-exact bonus (`-100`). It also carries Pi's
//! alphanumeric-swap fallback (`fuzzy.ts:75-92`): if `"abc123"` fails, `"123abc"` is retried at a
//! `+5` penalty.

/// A scored match: the candidate index plus its score (**lower = better**, per Pi).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Match {
    pub index: usize,
    pub score: f64,
}

/// True for the separators Pi treats as word boundaries: `/[\s\-_./:]/` (`fuzzy.ts:32`).
fn is_boundary_sep(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.' | '/' | ':')
}

/// Core scorer over an already-lowercased query/text (`matchQuery`, `fuzzy.ts:16-68`). `None` when
/// `query` is not an in-order subsequence of `text`; an empty query scores `0` (matches everything).
fn match_query(query: &[char], text: &[char]) -> Option<f64> {
    if query.is_empty() {
        return Some(0.0);
    }
    if query.len() > text.len() {
        return None;
    }

    let mut query_index = 0usize;
    let mut score = 0.0f64;
    let mut last_match_index: isize = -1;
    let mut consecutive_matches: i64 = 0;

    for (i, &tc) in text.iter().enumerate() {
        let Some(&qc) = query.get(query_index) else { break };
        if tc != qc {
            continue;
        }
        let is_word_boundary = i == 0
            || text
                .get(i.wrapping_sub(1))
                .copied()
                .is_some_and(is_boundary_sep);

        // Reward consecutive matches; otherwise penalize the gap since the previous match.
        if last_match_index == i as isize - 1 {
            consecutive_matches += 1;
            score -= (consecutive_matches * 5) as f64;
        } else {
            consecutive_matches = 0;
            if last_match_index >= 0 {
                score += (i as isize - last_match_index - 1) as f64 * 2.0;
            }
        }

        if is_word_boundary {
            score -= 10.0;
        }

        // Slight penalty for later matches.
        score += i as f64 * 0.1;

        last_match_index = i as isize;
        query_index += 1;
    }

    if query_index < query.len() {
        return None;
    }
    // Whole-string-exact bonus.
    if query == text {
        score -= 100.0;
    }
    Some(score)
}

/// Split a lowercased ASCII query into a leading letter run + trailing digit run (or the reverse),
/// returning the swapped form (`fuzzy.ts:75-81`). Empty when the query is not `letters+digits`.
fn swapped_query(query: &[char]) -> Option<Vec<char>> {
    if query.is_empty() {
        return None;
    }
    let is_alpha = |c: &char| c.is_ascii_lowercase();
    let is_digit = |c: &char| c.is_ascii_digit();
    let lead_alpha = query.iter().take_while(|c| is_alpha(c)).count();
    let lead_digit = query.iter().take_while(|c| is_digit(c)).count();

    // Swap the head and tail around `split`, panic-free via `get`.
    let build = |split: usize| -> Option<Vec<char>> {
        let head = query.get(..split)?;
        let tail = query.get(split..)?;
        let mut out = Vec::with_capacity(query.len());
        out.extend_from_slice(tail);
        out.extend_from_slice(head);
        Some(out)
    };

    // `^[a-z]+[0-9]+$`: letters then digits → digits then letters.
    if lead_alpha > 0
        && lead_alpha < query.len()
        && query.get(lead_alpha..).is_some_and(|rest| rest.iter().all(is_digit))
    {
        return build(lead_alpha);
    }
    // `^[0-9]+[a-z]+$`: digits then letters → letters then digits.
    if lead_digit > 0
        && lead_digit < query.len()
        && query.get(lead_digit..).is_some_and(|rest| rest.iter().all(is_alpha))
    {
        return build(lead_digit);
    }
    None
}

/// `fuzzyMatch(query, text)` (`fuzzy.ts:12-93`): score `text` against `query`, lower = better. `None`
/// when neither the query nor its alphanumeric-swap retry is a subsequence of `text`.
pub fn fuzzy_match(query: &str, text: &str) -> Option<f64> {
    let ql: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let tl: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();

    if let Some(primary) = match_query(&ql, &tl) {
        return Some(primary);
    }
    // Alphanumeric-swap fallback, at a +5 penalty.
    let swapped = swapped_query(&ql)?;
    match_query(&swapped, &tl).map(|s| s + 5.0)
}

/// Score `candidate` against `query` (Pi arg order is `(query, text)`; ours keeps `(candidate, query)`
/// for call-site symmetry). `None` when `query` does not fuzzy-match. Lower = better.
pub fn score(candidate: &str, query: &str) -> Option<f64> {
    fuzzy_match(query, candidate)
}

/// `fuzzyFilter` (`fuzzy.ts:99-137`): filter+rank `items` by `query`, best (lowest) score first.
/// The query is split on whitespace/`/` into tokens; **every** token must match, and scores sum.
/// An empty/whitespace query keeps every item in original order. The sort is stable, so equal totals
/// preserve input order — matching Pi's stable `Array.sort`.
pub fn filter<T>(items: &[T], query: &str, key: impl Fn(&T) -> &str) -> Vec<Match> {
    let tokens: Vec<&str> = query.split(|c: char| c.is_whitespace() || c == '/').filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return items
            .iter()
            .enumerate()
            .map(|(index, _)| Match { index, score: 0.0 })
            .collect();
    }

    let mut matches: Vec<Match> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let text = key(item);
        let mut total = 0.0f64;
        let mut all_match = true;
        for token in &tokens {
            match fuzzy_match(token, text) {
                Some(s) => total += s,
                None => {
                    all_match = false;
                    break;
                }
            }
        }
        if all_match {
            matches.push(Match { index, score: total });
        }
    }
    // Stable ascending sort (lower score first); equal scores keep input order.
    matches.sort_by(|a, b| a.score.total_cmp(&b.score));
    matches
}
