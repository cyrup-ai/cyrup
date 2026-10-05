//! GFM bare-URL and email autolinks — the part of marked's inline lexer pulldown-cmark does not have.
//!
//! Pi's `Markdown` lexes with `new Marked()`, whose `gfm: true` default adds the `url` inline
//! tokenizer: `https://x.com/a`, `www.x.com` and `me@x.com` written in running text become `link`
//! tokens, rendered like any other link (`markdown.ts:699-713`) — underlined in `mdLink`, wrapped in
//! an OSC-8 hyperlink on a capable terminal, else followed by ` (href)` when the text is not the
//! href. pulldown-cmark only recognises `<https://…>` and `[text](url)`, so a bare URL would stay
//! plain prose here.
//!
//! This is a port of marked's `url` rule and its `_backpedal` cleanup, which strip trailing
//! punctuation from the match:
//!
//! ```text
//! url:      ^((?:ftp|https?):\/\/|www\.)(?:[a-zA-Z0-9\-]+\.?)+[^\s<]*|^email
//! email:    [A-Za-z0-9._+-]+(@)[a-zA-Z0-9-_]+(?:\.[a-zA-Z0-9-_]*[a-zA-Z0-9])+(?![-_])
//! backpedal:(?:[^?!.,:;*_'"~()&]+|\([^)]*\)|&(?![a-zA-Z0-9]+;$)|[?!.,:;*_'"~)]+(?!$))+
//! ```
//!
//! Rust's `regex` has no look-around, so the three rules are matched by hand. marked tries `url` at
//! the start of a text run and wherever its text rule breaks (before `https?://`, `ftp://`, `www.`
//! and before an email's local part), which for ASCII scanning is the same as trying at every
//! character, left to right, taking the first match.

use super::MATH_START;

/// One bare link found in a text run.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Autolink {
    /// Byte range of the link text in the scanned string.
    pub(super) start: usize,
    pub(super) end: usize,
    /// `token.href`: the text itself, `http://`-prefixed for `www.`, `mailto:`-prefixed for email.
    pub(super) href: String,
}

/// Every autolink in `text`, in order and non-overlapping.
pub(super) fn find(text: &str) -> Vec<Autolink> {
    let mut found = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    // An email attempt that ran to the end of a local-part run without an `@` cannot succeed from
    // any later position inside that run either; skipping them keeps a long word linear.
    let mut email_skip_until = 0usize;
    while i < bytes.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = text.get(i..).unwrap_or("");
        let first = rest.as_bytes().first().copied().unwrap_or(0);
        let hit = if matches!(first, b'h' | b'f' | b'w') {
            url_at(rest).or_else(|| email_at(rest, i >= email_skip_until, &mut email_skip_until, i))
        } else if is_local(first) {
            email_at(rest, i >= email_skip_until, &mut email_skip_until, i)
        } else {
            None
        };
        match hit {
            Some((len, href)) => {
                found.push(Autolink {
                    start: i,
                    end: i + len,
                    href,
                });
                i += len;
            }
            None => i += 1,
        }
    }
    found
}

fn is_ws_or_stop(c: char) -> bool {
    c.is_whitespace() || c == '<' || c == MATH_START
}

/// The `url` rule's first branch, then `_backpedal`: `(length, href)`.
fn url_at(rest: &str) -> Option<(usize, String)> {
    let prefix = ["https://", "http://", "ftp://", "www."]
        .into_iter()
        .find(|p| rest.starts_with(p))?;
    let after = rest.get(prefix.len()..)?;
    // `(?:[a-zA-Z0-9\-]+\.?)+` needs at least one label character straight after the prefix; the
    // `[^\s<]*` that follows takes everything else up to whitespace or `<`.
    let first = after.chars().next()?;
    if !(first.is_ascii_alphanumeric() || first == '-') {
        return None;
    }
    let body_end = after
        .char_indices()
        .find(|&(_, c)| is_ws_or_stop(c))
        .map_or(after.len(), |(at, _)| at);
    let whole = rest.get(..prefix.len() + body_end)?;
    let text = backpedal(whole);
    if text.is_empty() {
        return None;
    }
    let href = if prefix == "www." {
        format!("http://{text}")
    } else {
        text.to_string()
    };
    Some((text.len(), href))
}

/// `_backpedal` applied until it stops changing the match (`Tokenizer.url`'s `do … while`).
fn backpedal(s: &str) -> &str {
    let mut cur = s;
    loop {
        let next = backpedal_once(cur);
        if next.len() == cur.len() {
            return next;
        }
        cur = next;
    }
}

const BACKPEDAL_STOP: &str = "?!.,:;*_'\"~()&";
const BACKPEDAL_TRAIL: &str = "?!.,:;*_'\"~)";

/// One pass of `(?:[^?!.,:;*_'"~()&]+|\([^)]*\)|&(?![a-zA-Z0-9]+;$)|[?!.,:;*_'"~)]+(?!$))+`,
/// anchored at the start: the longest prefix the alternation consumes, or `""` when it matches
/// nothing.
fn backpedal_once(s: &str) -> &str {
    let mut i = 0usize;
    while let Some(rest) = s.get(i..).filter(|r| !r.is_empty()) {
        let Some(c) = rest.chars().next() else { break };
        // `[^?!.,:;*_'"~()&]+`
        if !BACKPEDAL_STOP.contains(c) {
            let run = rest
                .char_indices()
                .find(|&(_, ch)| BACKPEDAL_STOP.contains(ch))
                .map_or(rest.len(), |(at, _)| at);
            i += run;
            continue;
        }
        // `\([^)]*\)`
        if c == '(' {
            match rest.get(1..).and_then(|r| r.find(')')) {
                Some(close) => {
                    i += 1 + close + 1;
                    continue;
                }
                None => break,
            }
        }
        // `&(?![a-zA-Z0-9]+;$)`: an `&` that does not open a trailing entity.
        if c == '&' {
            let tail = rest.get(1..).unwrap_or("");
            let entity = tail.strip_suffix(';').is_some_and(|name| {
                !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric())
            });
            if entity {
                break;
            }
            i += 1;
            continue;
        }
        // `[?!.,:;*_'"~)]+(?!$)`: a run of trailing punctuation that does not reach the end of the
        // string (the engine gives back its last character when it would).
        let run = rest
            .char_indices()
            .find(|&(_, ch)| !BACKPEDAL_TRAIL.contains(ch))
            .map_or(rest.len(), |(at, _)| at);
        let take = if run == rest.len() { run - 1 } else { run };
        if take == 0 {
            break;
        }
        i += take;
    }
    s.get(..i).unwrap_or("")
}

fn is_local(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-')
}

fn is_domain(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// The `email` rule: `(length, "mailto:" + text)`.
///
/// `attempt` is false while inside a local-part run already known to end without an `@`;
/// `skip_until` is then advanced past any new such run.
fn email_at(
    rest: &str,
    attempt: bool,
    skip_until: &mut usize,
    offset: usize,
) -> Option<(usize, String)> {
    if !attempt {
        return None;
    }
    let b = rest.as_bytes();
    let local_end = b.iter().position(|&c| !is_local(c)).unwrap_or(b.len());
    if local_end == 0 || b.get(local_end) != Some(&b'@') {
        *skip_until = offset + local_end;
        return None;
    }
    let mut k = local_end + 1;
    let d1 = b
        .get(k..)?
        .iter()
        .position(|&c| !is_domain(c))
        .unwrap_or(b.len() - k);
    if d1 == 0 {
        return None;
    }
    k += d1;
    let mut groups = 0usize;
    // `(?:\.[a-zA-Z0-9-_]*[a-zA-Z0-9])+`
    while b.get(k) == Some(&b'.') {
        let run = b
            .get(k + 1..)?
            .iter()
            .position(|&c| !is_domain(c))
            .unwrap_or(b.len() - (k + 1));
        let last_alnum = b
            .get(k + 1..k + 1 + run)?
            .iter()
            .rposition(u8::is_ascii_alphanumeric)?;
        k = k + 1 + last_alnum + 1;
        groups += 1;
    }
    if groups == 0 || matches!(b.get(k), Some(b'-' | b'_')) {
        return None;
    }
    let text = rest.get(..k)?;
    Some((k, format!("mailto:{text}")))
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn links(text: &str) -> Vec<(&str, String)> {
        find(text)
            .into_iter()
            .map(|l| (text.get(l.start..l.end).unwrap_or(""), l.href))
            .collect()
    }

    #[test]
    fn a_bare_url_is_a_link_and_trailing_punctuation_is_not_part_of_it() {
        assert_eq!(
            links("see https://example.com/a?b=1, then go."),
            [(
                "https://example.com/a?b=1",
                "https://example.com/a?b=1".to_string()
            )]
        );
        assert_eq!(
            links("(https://example.com/x)"),
            [("https://example.com/x", "https://example.com/x".to_string())]
        );
        assert_eq!(
            links("ends https://example.com."),
            [("https://example.com", "https://example.com".to_string())]
        );
    }

    #[test]
    fn www_gets_an_http_href_and_an_email_a_mailto() {
        assert_eq!(
            links("go to www.example.com now"),
            [("www.example.com", "http://www.example.com".to_string())]
        );
        assert_eq!(
            links("mail me@example.org please"),
            [("me@example.org", "mailto:me@example.org".to_string())]
        );
    }

    #[test]
    fn text_that_only_looks_like_the_start_of_one_is_left_alone() {
        assert!(links("http:// nothing, www. nothing, user@ nothing").is_empty());
        assert!(links("a@b is not an email, nor is a@b.").is_empty());
        assert!(links("plain prose with no links at all").is_empty());
    }

    #[test]
    fn a_url_stops_at_whitespace_and_at_an_angle_bracket() {
        assert_eq!(
            links("https://a.com/x<b>"),
            [("https://a.com/x", "https://a.com/x".to_string())]
        );
    }

    #[test]
    fn several_links_in_one_run_are_all_found_in_order() {
        let got = links("a https://one.io b www.two.io c three@four.io");
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0].0, "https://one.io");
        assert_eq!(got[1].0, "www.two.io");
        assert_eq!(got[2].0, "three@four.io");
    }

    #[test]
    fn a_long_word_without_an_at_sign_is_scanned_in_linear_time() {
        let word = "a".repeat(200_000);
        assert!(find(&word).is_empty());
    }
}
