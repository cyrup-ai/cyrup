//! `formatContextUsage` / `formatTokenCount` — a 1:1 port of `pi-intercom` `v0.9.2
//! format-context.ts` (32 lines).
//!
//! The READBACK half of the context-usage feature `pi-intercom` v0.8.0 added
//! (`v0.9.2 CHANGELOG.md:33`, "Added live context-window usage to session presence **and list
//! output**"). The producer half is [`crate::session_state::SharedIntercomState::current_context_usage`],
//! which rides the presence heartbeat; this is what turns the numbers a peer publishes into the
//! ` · 72% ctx (144k/200k)` a user reads in `intercom({ action: "list" })` output
//! (`v0.9.2 index.ts:428`, the sole call site — note upstream's own `ui/session-list.ts` overlay
//! does NOT render it, and neither does cyrup's).
//!
//! VERSION-LAG: absent at cyrup's ported baseline (`git grep contextPct v0.7.0` returns nothing).

use crate::transport::protocol::SessionInfo;

/// `formatTokenCount(tokens)` (`v0.9.2 format-context.ts:5-12`, 8 lines):
///
/// ```text
/// if (tokens < 1000) return String(Math.max(0, Math.round(tokens)));
/// const k = tokens / 1000;
/// const value = k >= 100 ? String(Math.round(k)) : k.toFixed(1).replace(/\.0$/, "");
/// return `${value}k`;
/// ```
///
/// Its own comment: "Compact token count for display: 1432 -> \"1.4k\", 144000 -> \"144k\". Keeps
/// list rows short while staying legible." (`:3-4`).
///
/// Takes `f64` rather than an integer because the wire type is a JSON number
/// ([`SessionInfo::context_tokens`] is a `serde_json::Number`), and pi accepts any `typeof ===
/// "number"` there — a fractional count from a peer must render, not be silently dropped.
#[must_use]
pub fn format_token_count(tokens: f64) -> String {
    if tokens < 1000.0 {
        // `String(Math.max(0, Math.round(tokens)))` — an integer string, so `{}` on the rounded
        // f64 (Rust prints `42` for `42.0`) is the same rendering.
        return format!("{}", tokens.round().max(0.0));
    }
    let k = tokens / 1000.0;
    let value = if k >= 100.0 {
        format!("{}", k.round())
    } else {
        let fixed = format!("{k:.1}");
        // `.replace(/\.0$/, "")` — anchored at the END, so `10.0` → `10` but `1.05` → `1.1` stays.
        fixed
            .strip_suffix(".0")
            .map_or(fixed.clone(), ToString::to_string)
    };
    format!("{value}k")
}

/// `formatContextUsage(session)` (`v0.9.2 format-context.ts:19-32`, 14 lines):
///
/// ```text
/// if (typeof session.contextPct !== "number") return "";
/// let out = ` · ${session.contextPct}% ctx`;
/// if (typeof session.contextTokens === "number" && typeof session.contextWindow === "number" && session.contextWindow > 0) {
///   out += ` (${formatTokenCount(session.contextTokens)}/${formatTokenCount(session.contextWindow)})`;
/// }
/// return out;
/// ```
///
/// Its own comment (`:14-18`) is the load-bearing part: "Unknown percent (e.g. right after a
/// compaction, before the next assistant response) renders nothing, so a frozen value is never shown
/// as a stale percentage." An absent `contextPct` is therefore the empty string, NOT `0% ctx` — the
/// same asymmetry the producer preserves by sending an explicit `null` rather than a `0`.
///
/// `typeof x !== "number"` maps to `Option::is_none` because [`SessionInfo`]'s three context fields
/// reject a non-number (and an explicit `null`) at deserialization already
/// (`transport/protocol.rs:266-285`, pi `broker/client.ts:182-186`), so "present" here already means
/// "present and numeric".
#[must_use]
pub fn format_context_usage(session: &SessionInfo) -> String {
    let Some(pct) = session.context_pct.as_ref() else {
        return String::new();
    };
    let mut out = format!(" · {pct}% ctx");
    if let Some(tokens) = session
        .context_tokens
        .as_ref()
        .and_then(serde_json::Number::as_f64)
        && let Some(window) = session
            .context_window
            .as_ref()
            .and_then(serde_json::Number::as_f64)
        && window > 0.0
    {
        out.push_str(&format!(
            " ({}/{})",
            format_token_count(tokens),
            format_token_count(window)
        ));
    }
    out
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

    /// `v0.9.2 format-context.test.ts:5-14` — the base row every case starts from.
    fn base() -> SessionInfo {
        serde_json::from_value(serde_json::json!({
            "id": "session-a",
            "name": "worker",
            "cwd": "/tmp/work",
            "model": "test-model",
            "pid": 1,
            "startedAt": 1,
            "lastActivity": 1,
        }))
        .expect("a minimal SessionInfo")
    }

    fn with(session: SessionInfo, key: &str, value: serde_json::Value) -> SessionInfo {
        let mut v = serde_json::to_value(session).expect("serialize");
        if let Some(map) = v.as_object_mut() {
            map.insert(key.to_string(), value);
        }
        serde_json::from_value(v).expect("deserialize")
    }

    /// `v0.9.2 format-context.test.ts:16-21`.
    #[test]
    fn renders_percent_plus_token_detail_when_all_known() {
        let s = with(
            with(
                with(base(), "contextPct", 72.into()),
                "contextTokens",
                144_000.into(),
            ),
            "contextWindow",
            200_000.into(),
        );
        assert_eq!(format_context_usage(&s), " · 72% ctx (144k/200k)");
    }

    /// `v0.9.2 format-context.test.ts:23-25`.
    #[test]
    fn shows_percent_only_when_token_counts_are_absent() {
        let s = with(base(), "contextPct", 30.into());
        assert_eq!(format_context_usage(&s), " · 30% ctx");
    }

    /// `v0.9.2 format-context.test.ts:27-31` — "never a stale %".
    #[test]
    fn renders_nothing_when_percent_is_unknown() {
        assert_eq!(format_context_usage(&base()), "");
        let s = with(
            with(base(), "contextTokens", 100.into()),
            "contextWindow",
            200.into(),
        );
        assert_eq!(
            format_context_usage(&s),
            "",
            "known token counts do NOT license inventing a percentage"
        );
    }

    /// The `formatTokenCount` ladder, from its own doc comment (`v0.9.2 format-context.ts:3-4`) plus
    /// each branch boundary.
    #[test]
    fn token_counts_render_compactly() {
        assert_eq!(format_token_count(0.0), "0");
        assert_eq!(format_token_count(999.0), "999");
        assert_eq!(
            format_token_count(-5.0),
            "0",
            "`Math.max(0, …)` floors at zero"
        );
        assert_eq!(
            format_token_count(1000.0),
            "1k",
            "`.toFixed(1)` = \"1.0\" → the `.0` is stripped"
        );
        assert_eq!(format_token_count(1432.0), "1.4k", "upstream's own example");
        assert_eq!(format_token_count(99_900.0), "99.9k");
        assert_eq!(
            format_token_count(100_000.0),
            "100k",
            "at k >= 100 the decimal is dropped"
        );
        assert_eq!(
            format_token_count(144_000.0),
            "144k",
            "upstream's own example"
        );
        assert_eq!(format_token_count(200_000.0), "200k");
    }

    /// A `contextWindow` of zero suppresses the token detail but NOT the percentage
    /// (`v0.9.2 format-context.ts:27` — the `> 0` guard sits inside the inner `if`).
    #[test]
    fn a_zero_window_suppresses_only_the_token_detail() {
        let s = with(
            with(
                with(base(), "contextPct", 12.into()),
                "contextTokens",
                100.into(),
            ),
            "contextWindow",
            0.into(),
        );
        assert_eq!(format_context_usage(&s), " · 12% ctx");
    }
}
