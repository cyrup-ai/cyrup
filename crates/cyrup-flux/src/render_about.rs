//! `/flux/about` overview — a Rust port of
//! [`flux_about.py`](../../../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_about.py)'s
//! text pipeline, minus the Rich markdown rendering: the body is already terminal-friendly
//! markdown, and the plain notification channel shows it as-is (port doc §3.4.3).
//!
//! Frontmatter stripping and the AI-only preamble drop both happened at VENDOR time
//! (`_docs/about.md`, FLUX_08 SUBTASK 1) — the source `.py` transforms
//! `extract_body`/`drop_ai_preamble` are one-time content edits, not renderer behaviour. The
//! only transform that must run at render time is the `//cmd` -> `/cmd` normalization, because
//! it is presentation (Wibey-era double-slash spellings), not content.

const ABOUT_MD: &str = include_str!("../resources/prompts/flux/_docs/about.md");

/// `SLASH_CMD_RE = re.compile(r"(?<![:\w/])//(?=\w)")` has no direct Rust `regex` equivalent
/// (this crate carries no `regex` dependency) — implemented as the direct character predicate the
/// lookbehind encodes: rewrite `//` to `/` only when the following character is `[A-Za-z0-9_]`
/// and the preceding character is ABSENT or is none of `:`, `/`, or `[A-Za-z0-9_]`. This is why
/// `https://example.com//path` is untouched (preceded by `:`, then by `/`) while `//flux/about`
/// becomes `/flux/about` (preceded by nothing, or by whitespace/punctuation).
fn normalize_slash_cmd(body: &str) -> String {
    fn is_word(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_'
    }
    let chars: Vec<char> = body.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(body.len());
    let mut i = 0usize;
    while i < n {
        let Some(&c0) = chars.get(i) else { break };
        let next_is_slash = chars.get(i + 1).copied() == Some('/');
        let after_is_word = chars.get(i + 2).copied().is_some_and(is_word);
        let prev_blocks = i > 0
            && chars
                .get(i - 1)
                .copied()
                .is_some_and(|p| p == ':' || p == '/' || is_word(p));
        if c0 == '/' && next_is_slash && after_is_word && !prev_blocks {
            out.push('/');
            i += 2; // consume both slashes, emit only one
        } else {
            out.push(c0);
            i += 1;
        }
    }
    out
}

/// Render the `/flux/about` body: the vendored, already frontmatter/preamble-stripped markdown,
/// with the `//cmd` -> `/cmd` normalization applied. No arguments; always succeeds.
#[must_use]
pub fn render() -> String {
    normalize_slash_cmd(ABOUT_MD.trim())
}
