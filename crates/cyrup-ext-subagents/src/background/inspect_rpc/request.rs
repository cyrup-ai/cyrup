//! The request side: what an inspect call names, and how the slash form spells it.

use super::{MAX_ID_LENGTH, is_valid_request_id};

/// pi `InspectErrorCode` (`inspect-rpc.ts:19-25`). A closed set, so the host can branch on the
/// code rather than on the prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectErrorCode {
    /// The arguments did not parse, or the run id was not a safe token.
    InvalidRequest,
    /// No such async run (or no such child under it).
    NotFound,
    /// The run exists and belongs to a different session.
    ForeignSession,
    /// The run resolved but its artifacts are gone.
    Stale,
    /// The caller has no session identity, so nothing can be attributed to it.
    NoActiveSession,
    /// Anything the read path threw.
    Internal,
}

impl InspectErrorCode {
    /// The wire spelling, for callers rendering the code outside a serializer.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::NotFound => "not_found",
            Self::ForeignSession => "foreign_session",
            Self::Stale => "stale",
            Self::NoActiveSession => "no_active_session",
            Self::Internal => "internal",
        }
    }
}

/// pi `InspectRequest` (`inspect-rpc.ts:27-32`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectRequest {
    /// The caller's correlation token, matching [`is_valid_request_id`].
    pub request_id: String,
    /// The run id (or unique prefix) to inspect.
    pub async_id: String,
    /// One child of that run, by any of its
    /// [`crate::background::child_identity`] spellings.
    pub child_id: Option<String>,
    /// How many transcript messages to return. Clamped into `1..=MAX_MESSAGE_LINES` at read time,
    /// never here — upstream validates only positivity at parse (`:133`) and clamps at `:371`.
    pub lines: Option<i64>,
}

/// The subset of a request an error reply echoes back — pi's `Partial<InspectRequest>` (`:97`).
///
/// A distinct type rather than an `InspectRequest` with empty fields: `handle_inspect_rpc_args`'s
/// parse-failure path (`:438-440`) has nothing but a possibly-valid first token, and an
/// `InspectRequest` with a fabricated `async_id` would be echoed back as though the caller had
/// asked for it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InspectRequestEcho {
    /// Echoed verbatim when it matches [`is_valid_request_id`], else the literal `invalid`.
    pub request_id: Option<String>,
    /// Echoed truncated to [`MAX_ID_LENGTH`], when present.
    pub async_id: Option<String>,
    /// Echoed truncated to [`MAX_ID_LENGTH`], when present.
    pub child_id: Option<String>,
}

impl From<&InspectRequest> for InspectRequestEcho {
    fn from(request: &InspectRequest) -> Self {
        Self {
            request_id: Some(request.request_id.clone()),
            async_id: Some(request.async_id.clone()),
            child_id: request.child_id.clone(),
        }
    }
}

/// pi `parseInspectRequest` (`inspect-rpc.ts:108-135`) — the `/subagents-inspect-rpc` argument
/// form, whose usage string is quoted verbatim at `:128`/`:132`.
///
/// ```text
/// /subagents-inspect-rpc <requestId> <asyncId> [childId] [--lines N]
/// ```
///
/// # Errors
///
/// Upstream's own six refusal sentences, verbatim. Each is user-facing prose the host echoes, so
/// they are `String`s and not codes.
pub fn parse_inspect_request(args: &str) -> Result<InspectRequest, String> {
    const USAGE: &str =
        "Usage: /subagents-inspect-rpc <requestId> <asyncId> [childId] [--lines N].";

    let tokens: Vec<&str> = args.split_whitespace().collect();
    let mut positional: Vec<&str> = Vec::new();
    let mut lines: Option<i64> = None;
    let mut index = 0usize;
    while let Some(token) = tokens.get(index) {
        if *token == "--lines" {
            index += 1;
            // pi `:116` — a missing value is `NaN`, which fails `Number.isFinite`; a value that is
            // not an integer fails `parseInt` the same way. `parseInt` also accepts a numeric
            // PREFIX (`12abc` → 12), which `str::parse` does not; the difference only makes cyrup
            // stricter on input upstream would silently half-read, so it is kept.
            let parsed = tokens
                .get(index)
                .and_then(|value| value.parse::<i64>().ok());
            let Some(parsed) = parsed else {
                return Err("--lines requires an integer value.".to_string());
            };
            lines = Some(parsed);
            index += 1;
            continue;
        }
        if token.starts_with("--") {
            return Err(format!("Unknown flag: {token}. Supported: --lines N."));
        }
        positional.push(token);
        index += 1;
    }

    let request_id = positional.first().copied().unwrap_or_default();
    if !is_valid_request_id(request_id) {
        return Err("requestId must match [A-Za-z0-9_-]{1,64}.".to_string());
    }
    let Some(async_id) = positional.get(1).copied().filter(|id| !id.is_empty()) else {
        return Err(USAGE.to_string());
    };
    let child_id = positional.get(2).copied();
    if async_id.chars().count() > MAX_ID_LENGTH
        || child_id.is_some_and(|id| id.chars().count() > MAX_ID_LENGTH)
    {
        return Err("asyncId/childId exceed the maximum length.".to_string());
    }
    if positional.len() > 3 {
        return Err(format!("Too many positional arguments. {USAGE}"));
    }
    // pi `:133` — a non-positive `--lines` is refused here; the UPPER clamp is applied at read
    // time (`:371`), not at parse, so `--lines 10000` parses and then reads 200.
    if lines.is_some_and(|value| value < 1) {
        return Err("--lines must be a positive integer.".to_string());
    }

    Ok(InspectRequest {
        request_id: request_id.to_string(),
        async_id: async_id.to_string(),
        child_id: child_id.map(str::to_string),
        lines,
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    #[test]
    fn the_slash_form_parses_every_positional_and_the_flag() {
        let request = parse_inspect_request("req-1 run0001").expect("minimal form");
        assert_eq!(request.request_id, "req-1");
        assert_eq!(request.async_id, "run0001");
        assert_eq!(request.child_id, None);
        assert_eq!(request.lines, None);

        let request = parse_inspect_request("  req_2   run0001  step:1  --lines 25 ")
            .expect("full form, irregular whitespace");
        assert_eq!(request.child_id.as_deref(), Some("step:1"));
        assert_eq!(request.lines, Some(25));

        // The flag may precede the positionals — pi scans tokens, it does not require an order.
        let request = parse_inspect_request("--lines 3 req3 run0001").expect("flag first");
        assert_eq!(request.lines, Some(3));
        assert_eq!(request.async_id, "run0001");
    }

    #[test]
    fn every_refusal_is_upstreams_own_sentence() {
        assert_eq!(
            parse_inspect_request("").unwrap_err(),
            "requestId must match [A-Za-z0-9_-]{1,64}."
        );
        assert_eq!(
            parse_inspect_request("has.dot run1").unwrap_err(),
            "requestId must match [A-Za-z0-9_-]{1,64}."
        );
        assert_eq!(
            parse_inspect_request("req1").unwrap_err(),
            "Usage: /subagents-inspect-rpc <requestId> <asyncId> [childId] [--lines N]."
        );
        assert_eq!(
            parse_inspect_request("req1 run1 child1 extra").unwrap_err(),
            "Too many positional arguments. Usage: /subagents-inspect-rpc <requestId> <asyncId> \
             [childId] [--lines N]."
        );
        assert_eq!(
            parse_inspect_request("req1 run1 --lines").unwrap_err(),
            "--lines requires an integer value."
        );
        assert_eq!(
            parse_inspect_request("req1 run1 --lines abc").unwrap_err(),
            "--lines requires an integer value."
        );
        assert_eq!(
            parse_inspect_request("req1 run1 --lines 0").unwrap_err(),
            "--lines must be a positive integer."
        );
        assert_eq!(
            parse_inspect_request("req1 run1 --verbose").unwrap_err(),
            "Unknown flag: --verbose. Supported: --lines N."
        );
        assert_eq!(
            parse_inspect_request(&format!("req1 {}", "x".repeat(257))).unwrap_err(),
            "asyncId/childId exceed the maximum length."
        );
    }

    /// The upper bound is deliberately NOT a parse error — it is clamped at read time. Pinned
    /// because "validate it here too" is the natural-looking change that would turn a working
    /// request into a refusal.
    #[test]
    fn an_over_large_lines_value_parses_and_is_clamped_later() {
        let request = parse_inspect_request("req1 run1 --lines 100000").expect("parses");
        assert_eq!(request.lines, Some(100_000));
    }
}
