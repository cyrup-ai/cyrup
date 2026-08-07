//! The provider HTTP error-body cap (1:1 with Pi `packages/ai/src/utils/error-body.ts`).
//!
//! Pi bounds every provider error body it folds into `AssistantMessage.errorMessage` at
//! [`MAX_PROVIDER_ERROR_BODY_CHARS`] (`error-body.ts:16`): `extractBody` trims the raw body,
//! discards it when empty, and runs it through `truncateErrorText` (`error-body.ts:76-82`) before
//! `formatProviderError` composes the `"<status>: <body>"` display string that becomes
//! `output.errorMessage` (e.g. `openai-completions.ts:595`).
//!
//! Cyrup's transport is raw `reqwest` + SSE rather than the vendor SDKs, so only the cap itself
//! ports: Pi's `normalizeProviderError`/`extractStatus`/`extractBody` exist to dig a status and a
//! body out of four different SDK error *objects* (Mistral `statusCode`, `openai` `error`,
//! `@google/genai` `status`, Bedrock `$metadata`/`$response`), and cyrup already has both in hand at
//! the one place a non-2xx is observed ([`crate::stream::sse::open_sse`]). `formatProviderError`'s
//! composition likewise already exists as `ProviderError::Http`'s `Display`
//! (`"http {status}: {message}"`).
//!
//! Without this cap a multi-megabyte gateway HTML error page reached `AssistantMessage::errored`
//! verbatim, was persisted into the session JSONL through the assistant serializer's
//! `error_message` field, and was replayed into the next turn's prompt.

/// Maximum characters of provider HTTP error body kept in an error message (Pi
/// `MAX_PROVIDER_ERROR_BODY_CHARS`, error-body.ts:16).
pub const MAX_PROVIDER_ERROR_BODY_CHARS: usize = 4000;

/// Truncate `text` to `max_chars`, appending Pi's exact overflow marker (Pi `truncateErrorText`,
/// error-body.ts:139-142):
///
/// ```text
/// `${text.slice(0, maxChars)}... [truncated ${text.length - maxChars} chars]`
/// ```
///
/// Text at or under the cap is returned unchanged, so the boundary is inclusive exactly as Pi's
/// `text.length <= maxChars` is.
///
/// `[CYRUP-DELTA]` Pi counts UTF-16 code units (JS `String.length`/`String.slice`); this counts
/// Unicode scalar values. The two agree for every BMP character — which is all of the HTML, JSON
/// and plain-text bodies gateways emit — and differ only for astral characters, where JS would also
/// be free to split a surrogate pair and produce a lone surrogate. Rust `String` cannot represent a
/// lone surrogate, so scalar values are the only sound unit here; the reported `[truncated N
/// chars]` count is in the same unit as the slice, keeping the message self-consistent.
pub fn truncate_error_text(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    let dropped = total.saturating_sub(max_chars);
    format!("{head}... [truncated {dropped} chars]")
}

/// Normalize a raw HTTP error body for display: trim it, then cap it at
/// [`MAX_PROVIDER_ERROR_BODY_CHARS`] (Pi `extractBody`, error-body.ts:76-82).
///
/// Pi maps an empty body to `undefined` so it never surfaces as `""`; here the empty case simply
/// yields an empty `String`, which `ProviderError::Http`'s `Display` renders the same way Pi's
/// `formatProviderError` renders a body-less error (the status alone carries the meaning).
pub fn normalize_error_body(raw: &str) -> String {
    truncate_error_text(raw.trim(), MAX_PROVIDER_ERROR_BODY_CHARS)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn text_at_or_under_the_cap_is_unchanged() {
        assert_eq!(truncate_error_text("", 4), "");
        assert_eq!(truncate_error_text("abcd", 4), "abcd");
        assert_eq!(truncate_error_text("abc", 4), "abc");
    }

    /// Pi `error-body.test.ts:160-171`: a body 50 chars over the cap gains exactly
    /// `"... [truncated 50 chars]"` and is shorter than the input.
    #[test]
    fn truncates_at_the_cap_with_pis_exact_marker() {
        let long = "x".repeat(MAX_PROVIDER_ERROR_BODY_CHARS + 50);
        let out = truncate_error_text(&long, MAX_PROVIDER_ERROR_BODY_CHARS);
        assert!(out.contains("... [truncated 50 chars]"));
        assert!(out.len() < long.len());
        assert_eq!(
            out,
            format!(
                "{}... [truncated 50 chars]",
                "x".repeat(MAX_PROVIDER_ERROR_BODY_CHARS)
            )
        );
    }

    #[test]
    fn the_boundary_is_inclusive() {
        let exact = "y".repeat(MAX_PROVIDER_ERROR_BODY_CHARS);
        assert_eq!(
            truncate_error_text(&exact, MAX_PROVIDER_ERROR_BODY_CHARS),
            exact,
            "length == cap must NOT be truncated (Pi: `text.length <= maxChars`)"
        );
        let over = format!("{exact}z");
        assert_eq!(
            truncate_error_text(&over, MAX_PROVIDER_ERROR_BODY_CHARS),
            format!("{exact}... [truncated 1 chars]"),
            "one char over must truncate, with Pi's un-pluralized `chars`"
        );
    }

    /// Multi-byte characters are counted (and cut) as characters, never bytes — cutting on a byte
    /// index would both mis-report the count and panic on a char boundary.
    #[test]
    fn multibyte_text_is_cut_on_character_boundaries() {
        let text = "é".repeat(10);
        let out = truncate_error_text(&text, 4);
        assert_eq!(out, "éééé... [truncated 6 chars]");
    }

    #[test]
    fn normalize_trims_then_truncates() {
        assert_eq!(normalize_error_body("   \n boom \t "), "boom");
        assert_eq!(normalize_error_body("   \n\t "), "");

        // Whitespace is stripped BEFORE the cap is applied, exactly as Pi trims before
        // `truncateErrorText`, so padding cannot consume the budget.
        let padded = format!("  {}  ", "q".repeat(MAX_PROVIDER_ERROR_BODY_CHARS));
        assert_eq!(
            normalize_error_body(&padded),
            "q".repeat(MAX_PROVIDER_ERROR_BODY_CHARS)
        );
    }

    #[test]
    fn a_megabyte_html_error_page_is_bounded() {
        let page = format!(
            "<html><body>{}</body></html>",
            "<p>gateway exploded</p>".repeat(50_000)
        );
        assert!(page.len() > 1_000_000);
        let out = normalize_error_body(&page);
        assert!(
            out.chars().count() < MAX_PROVIDER_ERROR_BODY_CHARS + 64,
            "capped body must be the 4000-char head plus the short marker, got {} chars",
            out.chars().count()
        );
    }
}
