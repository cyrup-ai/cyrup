//! CFG-067 — the per-tool-call deadline subsystem: pi-subagents v0.71.0
//! `src/runs/shared/tool-timeout.ts` in full, plus the parent-side enforcement its consumers
//! (`runs/foreground/execution.ts:1218-1281`, `runs/background/run-child-session.ts:361-408`) build
//! on top of it.
//!
//! The env override this module owns is the last of the twelve `pi-subagents` variables `CFG-067`
//! enumerates that still exists upstream and still had no `CYRUP_` counterpart. It is deliberately
//! landed together with the mechanism it configures: a bare constant would be a knob wired to
//! nothing, and the row asks that enforcement be in the FIRST slice.

use std::time::Duration;

use tokio::time::Instant;

/// pi `TOOL_TIMEOUT_ENV = "PI_SUBAGENT_TOOL_TIMEOUT_MS"` (`tool-timeout.ts:1`), under this
/// workspace's `CYRUP_`-only env surface (`dd44b3c` dropped every `PI_*` alias, so there is no
/// read-side alias here either — deliberately, to match every other variable this crate reads).
pub const TOOL_TIMEOUT_ENV: &str = "CYRUP_SUBAGENT_TOOL_TIMEOUT_MS";

/// pi `MAX_TIMER_DELAY_MS` (`tool-timeout.ts:4`) — "maximum delay a Node.js timer accepts without
/// overflow". Rust's timers have no such ceiling, but the VALIDATION is observable (it is the
/// boundary the refusal message quotes and the agent-frontmatter validator already enforces at
/// `discovery/frontmatter.rs`), so the bound is upstream's, not the host's.
pub const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;

/// pi `DEFAULT_FAST_TOOL_TIMEOUT_MS` (`tool-timeout.ts:6`).
pub const DEFAULT_FAST_TOOL_TIMEOUT_MS: u64 = 300_000;

/// pi `DEFAULT_FAST_TOOL_TIMEOUT_TOOLS` (`tool-timeout.ts:8-16`): the tools that get a default
/// deadline even when nothing configured one, because none of them has a legitimate reason to run
/// for five minutes.
///
/// The six file/search builtins are spelled literally because that is how they reach this crate — on
/// the child's NDJSON wire as `tool_execution_start.toolName`, with no constant on this side of the
/// process boundary to bind to. `structured_output` DOES have one
/// ([`crate::prompt_runtime::STRUCTURED_OUTPUT_TOOL_NAME`]), so it is bound rather than repeated: it
/// is the one name in this set this workspace itself decides.
pub const DEFAULT_FAST_TOOL_TIMEOUT_TOOLS: [&str; 7] = [
    "read",
    "grep",
    "find",
    "ls",
    "edit",
    "write",
    crate::prompt_runtime::STRUCTURED_OUTPUT_TOOL_NAME,
];

/// pi `TOOL_TIMEOUT_EXEMPT_TOOLS` (`tool-timeout.ts:19`): "tools whose normal job can be to wait
/// for a person or another run". An exempt tool gets NO deadline at all — not even a configured
/// one, which is what makes `contact_supervisor` survive an operator who is at lunch.
///
/// Bound to this crate's OWN tool-name constants rather than repeating upstream's three literals,
/// because all three tools are registered by this crate and the exemption is only as correct as the
/// names agreeing. A literal set would let a rename of any of them silently put a clock on the one
/// tool whose job is to block on a human — the failure would look like a subagent that keeps dying
/// five minutes into every `contact_supervisor` ask, with nothing naming the cause. The names
/// themselves are upstream's, verbatim, which is what this module's
/// `the_exempt_set_is_bound_to_this_crates_own_tool_names` test pins.
pub const TOOL_TIMEOUT_EXEMPT_TOOLS: [&str; 3] = [
    crate::native_supervisor::CONTACT_SUPERVISOR_TOOL_NAME,
    crate::native_supervisor::INTERCOM_TOOL_NAME,
    crate::extension::wait_tool::WAIT_TOOL_NAME,
];

/// pi `isToolTimeoutExempt` (`tool-timeout.ts:23-25`).
#[must_use]
pub fn is_tool_timeout_exempt(tool_name: Option<&str>) -> bool {
    tool_name.is_some_and(|name| TOOL_TIMEOUT_EXEMPT_TOOLS.contains(&name))
}

/// pi `defaultToolTimeoutMs` (`tool-timeout.ts:27-31`).
#[must_use]
pub fn default_tool_timeout_ms(tool_name: Option<&str>) -> Option<u64> {
    tool_name
        .filter(|name| DEFAULT_FAST_TOOL_TIMEOUT_TOOLS.contains(name))
        .map(|_| DEFAULT_FAST_TOOL_TIMEOUT_MS)
}

/// pi `effectiveToolTimeoutMs` (`tool-timeout.ts:33-36`):
///
/// ```text
/// if (isToolTimeoutExempt(toolName)) return undefined;
/// return configuredToolTimeoutMs ?? defaultToolTimeoutMs(toolName);
/// ```
///
/// The exemption is checked FIRST and therefore outranks an explicitly configured value: an
/// operator who sets `toolTimeoutMs` does not thereby put a clock on `contact_supervisor`.
#[must_use]
pub fn effective_tool_timeout_ms(
    tool_name: Option<&str>,
    configured_tool_timeout_ms: Option<u64>,
) -> Option<u64> {
    if is_tool_timeout_exempt(tool_name) {
        return None;
    }
    configured_tool_timeout_ms.or_else(|| default_tool_timeout_ms(tool_name))
}

/// pi `formatToolTimeoutMessage` (`tool-timeout.ts:38-40`) — verbatim, because it is what the
/// operator reads and what `result.error`/`result.finalOutput` both become.
#[must_use]
pub fn format_tool_timeout_message(tool_name: &str, timeout_ms: u64) -> String {
    format!("Tool '{tool_name}' exceeded its timeout of {timeout_ms}ms.")
}

/// pi `toolTimeoutCallKey` (`tool-timeout.ts:42-46`): a real `toolCallId` when the child supplied
/// one, else a per-run-unique anonymous key derived from the tool name and a sequence number.
///
/// cyrup's NDJSON wire types `tool_call_id` as a [`cyrup_core::ToolCallId`] rather than an
/// optional field, so the anonymous leg is reached only for the empty string — upstream's own
/// `length > 0` guard covers exactly that case, which is why the guard is ported rather than
/// collapsed into "always use the id".
#[must_use]
pub fn tool_timeout_call_key(
    tool_call_id: Option<&str>,
    tool_name: Option<&str>,
    fallback_id: u64,
) -> String {
    match tool_call_id {
        Some(id) if !id.is_empty() => format!("id:{id}"),
        _ => format!("anon:{}:{fallback_id}", tool_name.unwrap_or("tool")),
    }
}

/// pi `ToolTimeoutResolutionInput` (`tool-timeout.ts:48-57`) — the four rungs, highest first.
#[derive(Debug, Default, Clone)]
pub struct ToolTimeoutResolutionInput<'a> {
    /// Per-call value from the subagent tool params (highest precedence).
    pub call_value: Option<&'a serde_json::Value>,
    /// Agent frontmatter default (second precedence).
    pub agent_value: Option<u64>,
    /// Global extension `config.toolTimeoutMs` (third precedence).
    pub config_value: Option<&'a serde_json::Value>,
    /// [`TOOL_TIMEOUT_ENV`] override (lowest precedence).
    pub env_value: Option<String>,
}

/// pi `resolveToolTimeoutMs` (`tool-timeout.ts:60-86`).
///
/// Two details of upstream's shape are load-bearing and easy to "improve" away:
///
/// * the winner is the first rung that is PRESENT, not the first rung that is VALID — so a
///   malformed `toolTimeoutMs` on the call is an error even when the agent declares a perfectly
///   good one. Falling through would silently run with a timeout the caller did not ask for.
/// * the env rung is consulted only when all three typed rungs are absent, and only when it is
///   non-blank after trimming (`envValue.trim() !== ""`), so `CYRUP_SUBAGENT_TOOL_TIMEOUT_MS=""`
///   is "unset" rather than "invalid".
///
/// # Errors
///
/// Upstream's single message, with the losing rung's own label interpolated.
pub fn resolve_tool_timeout_ms(
    input: &ToolTimeoutResolutionInput<'_>,
) -> Result<Option<u64>, String> {
    let agent_value = input.agent_value.map(serde_json::Value::from);
    let candidates: [(&str, Option<&serde_json::Value>); 3] = [
        ("toolTimeoutMs", input.call_value),
        ("agent.toolTimeoutMs", agent_value.as_ref()),
        ("config.toolTimeoutMs", input.config_value),
    ];
    let mut winner: Option<(&str, serde_json::Value)> = None;
    for (label, value) in candidates {
        // pi's `if (candidate.value === undefined) continue;` — a JSON `null` is NOT `undefined`,
        // and upstream would take it as the winner and then refuse it. Same here.
        if let Some(value) = value {
            winner = Some((label, value.clone()));
            break;
        }
    }
    if winner.is_none()
        && let Some(raw) = input.env_value.as_ref()
        && !raw.trim().is_empty()
    {
        winner = Some((TOOL_TIMEOUT_ENV, serde_json::Value::String(raw.clone())));
    }
    let Some((label, raw)) = winner else {
        return Ok(None);
    };

    // `if (winner.label === TOOL_TIMEOUT_ENV && typeof raw === "string") { parsed = Number(raw);
    //  if (raw.trim() !== "" && !Number.isNaN(parsed)) raw = parsed; }` — the env rung, and ONLY
    // the env rung, gets JS `Number()` coercion applied to its string. A typed rung that happens to
    // hold a string is left a string and therefore refused, which is upstream's behaviour too.
    let coerced = if label == TOOL_TIMEOUT_ENV {
        raw.as_str().and_then(number_coercion)
    } else {
        None
    };
    let value = match coerced {
        Some(number) => number,
        None => match raw.as_f64() {
            Some(number) => number,
            None => return Err(invalid_tool_timeout_message(label)),
        },
    };
    // `typeof raw !== "number" || !Number.isInteger(raw) || raw <= 0 || raw > MAX_TIMER_DELAY_MS`
    if !value.is_finite()
        || value.fract() != 0.0
        || value <= 0.0
        || value > MAX_TIMER_DELAY_MS as f64
    {
        return Err(invalid_tool_timeout_message(label));
    }
    // Exact: the guards above pin `value` to an integer in `1..=MAX_TIMER_DELAY_MS`.
    Ok(Some(value as u64))
}

/// JS `Number(raw)` over the env string: leading/trailing whitespace is ignored, an empty string
/// is `0` (unreachable here — the caller already rejected a blank value), and anything else that is
/// not a numeric literal is `NaN`, which upstream leaves as a STRING and then refuses.
fn number_coercion(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }
    trimmed.parse::<f64>().ok()
}

/// pi's single refusal string (`tool-timeout.ts:82`).
fn invalid_tool_timeout_message(label: &str) -> String {
    format!("{label} must be a positive integer no larger than {MAX_TIMER_DELAY_MS}.")
}

/// pi `toolTimeoutFromEnv` (`tool-timeout.ts:88-90`).
#[must_use]
pub fn tool_timeout_from_env() -> Option<String> {
    std::env::var(TOOL_TIMEOUT_ENV).ok()
}

/// One armed per-tool-call deadline — pi's `activeToolTimeouts` map entry plus the `setTimeout`
/// handle it wraps (`execution.ts:1219`).
#[derive(Debug, Clone)]
struct ArmedToolTimeout {
    key: String,
    tool_name: String,
    timeout_ms: u64,
    fires_at: Instant,
}

/// What an expired deadline settles the attempt with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolTimeoutExpiry {
    /// [`format_tool_timeout_message`]'s text, which upstream assigns to BOTH `result.error` and
    /// `result.finalOutput` (`execution.ts:1249-1251`).
    pub(crate) message: String,
}

/// pi's parent-side enforcement state (`execution.ts:1218-1281`): the armed per-call deadlines, the
/// name index `clearActiveToolTimeout` falls back to, and the anonymous-key sequence.
///
/// `[CYRUP-DELTA]` at full feature parity: upstream arms one `setTimeout` per call and lets the
/// event loop fire it; this holds the same deadlines as instants and lets the drive loop's existing
/// `select!` race the EARLIEST of them. The set of calls that time out, the instant each one fires
/// and the message it fires with are identical — a timer that is polled instead of scheduled is the
/// same timer, and cyrup's drive loop has no event loop to hang a callback off.
#[derive(Debug, Default, Clone)]
pub(crate) struct ToolTimeoutTracker {
    /// The resolved `toolTimeoutMs` for this run (pi `options.toolTimeoutMs`), or `None` when only
    /// the fast-tool defaults apply.
    configured_ms: Option<u64>,
    /// Insertion-ordered, because `clearActiveToolTimeout`'s by-name fallback takes the FIRST key
    /// registered under that name (`execution.ts:1235`).
    armed: Vec<ArmedToolTimeout>,
    /// pi `toolTimeoutSequence` (`:1218`).
    sequence: u64,
}

impl ToolTimeoutTracker {
    /// `configured_ms` is the already-resolved [`resolve_tool_timeout_ms`] value for the run.
    pub(crate) fn new(configured_ms: Option<u64>) -> Self {
        Self {
            configured_ms,
            armed: Vec::new(),
            sequence: 0,
        }
    }

    /// pi `armToolTimeout` (`execution.ts:1264-1281`), including its run-remaining guard:
    /// `if (runRemaining !== undefined && timeoutForTool >= runRemaining) return;` — a per-tool
    /// deadline that lands at or after the run's OWN deadline is not armed at all, so the run-level
    /// timeout owns that outcome and the operator gets one diagnosis rather than a race between two.
    ///
    /// `run_remaining_ms` is `None` for a run with no wall-clock deadline.
    pub(crate) fn arm(
        &mut self,
        tool_call_id: Option<&str>,
        tool_name: &str,
        now: Instant,
        run_remaining_ms: Option<u64>,
    ) {
        let Some(timeout_for_tool) = effective_tool_timeout_ms(Some(tool_name), self.configured_ms)
        else {
            return;
        };
        if run_remaining_ms.is_some_and(|remaining| timeout_for_tool >= remaining) {
            return;
        }
        self.sequence += 1;
        let key = tool_timeout_call_key(tool_call_id, Some(tool_name), self.sequence);
        self.armed.push(ArmedToolTimeout {
            key,
            tool_name: tool_name.to_string(),
            timeout_ms: timeout_for_tool,
            fires_at: now + Duration::from_millis(timeout_for_tool),
        });
    }

    /// pi `clearActiveToolTimeout` (`execution.ts:1231-1240`): the call's own id when it has one,
    /// else the first key still armed under that tool NAME, else — only when the event names no
    /// tool at all — the single armed key if there is exactly one.
    pub(crate) fn clear(&mut self, tool_call_id: Option<&str>, tool_name: Option<&str>) {
        let key = match tool_call_id {
            Some(id) if !id.is_empty() => Some(format!("id:{id}")),
            _ => match tool_name {
                Some(name) => self
                    .armed
                    .iter()
                    .find(|armed| armed.tool_name == name)
                    .map(|armed| armed.key.clone()),
                None if self.armed.len() == 1 => self.armed.first().map(|armed| armed.key.clone()),
                None => None,
            },
        };
        if let Some(key) = key {
            self.armed.retain(|armed| armed.key != key);
        }
    }

    /// pi `clearAllToolTimeouts` (`execution.ts:1241-1247`), armed on every path that ends the
    /// child's tool work: a terminal assistant stop, the run-level timeout, an interrupt and the
    /// settle itself.
    pub(crate) fn clear_all(&mut self) {
        self.armed.clear();
    }

    /// The earliest armed deadline, for the drive loop's `select!` arm. `None` while nothing is
    /// armed, which is every run that configures no timeout and calls no fast tool.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.armed.iter().map(|armed| armed.fires_at).min()
    }

    /// Fire the earliest deadline that has come due, exactly as upstream's `setTimeout` callback
    /// does: remove the key first (`removeToolTimeoutKey(key)`), then compose the message from the
    /// tool name and the timeout the call was armed with.
    pub(crate) fn expire(&mut self, now: Instant) -> Option<ToolTimeoutExpiry> {
        let due = self
            .armed
            .iter()
            .enumerate()
            .filter(|(_, armed)| armed.fires_at <= now)
            .min_by_key(|(_, armed)| armed.fires_at)
            .map(|(index, _)| index)?;
        // Exact: `due` came from `iter().enumerate()` over this same vector.
        let armed = self.armed.remove(due);
        Some(ToolTimeoutExpiry {
            message: format_tool_timeout_message(&armed.tool_name, armed.timeout_ms),
        })
    }

    /// The tool names with a deadline armed right now — test/diagnostic surface only.
    #[cfg(test)]
    fn armed_tools(&self) -> Vec<String> {
        self.armed
            .iter()
            .map(|armed| armed.tool_name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

/// CFG-067 — pi's two `SubagentTask` carriers for the rungs a DETACHED runner cannot re-derive:
/// `toolTimeoutMs` (the call rung, `shared/types.ts:2460`) and `configToolTimeoutMs`
/// ("Raw global config.toolTimeoutMs, used by the per-child resolver", `:2461-2462`).
///
/// Both are carried RAW across the process boundary for the same reason the resolver takes them
/// raw: the winner is the first rung that is PRESENT, so a malformed value must be refused with the
/// LOSING rung's own label rather than silently skipped. The AGENT rung is deliberately absent —
/// every step of a chain/fan-out names a different persona, so it is supplied per step by
/// [`resolve_tool_timeouts_by_agent`], which is exactly where upstream applies it
/// (`async-execution.ts:1006-1012`, resolving per step with that step's `a.defaultToolTimeoutMs`).
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolTimeoutRungs {
    /// The per-call `toolTimeoutMs` param, raw.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_timeout_ms: Option<serde_json::Value>,
    /// The global `config.toolTimeoutMs`, raw.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_tool_timeout_ms: Option<serde_json::Value>,
}

impl ToolTimeoutRungs {
    /// True when neither rung was supplied, i.e. only the agent and env rungs can decide.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tool_timeout_ms.is_none() && self.config_tool_timeout_ms.is_none()
    }
}

/// CFG-067 — resolve the effective `toolTimeoutMs` for EVERY agent a detached run may dispatch, in
/// one pass, so a malformed rung refuses the whole run up front (upstream's
/// `AsyncStartValidationError`, `async-execution.ts:1012`) instead of surfacing on whichever step
/// happens to reach it first.
///
/// The map's absence of an agent means "no configured deadline for it" — the fast-tool defaults
/// still apply per call, because those are decided by [`effective_tool_timeout_ms`] at the moment a
/// tool starts and never by this resolution.
///
/// # Errors
///
/// [`resolve_tool_timeout_ms`]'s message for the first agent whose ladder cannot resolve.
pub fn resolve_tool_timeouts_by_agent<'a>(
    rungs: &ToolTimeoutRungs,
    agents: impl IntoIterator<Item = (&'a str, Option<u64>)>,
) -> Result<std::collections::BTreeMap<String, Option<u64>>, String> {
    let env_value = tool_timeout_from_env();
    let mut resolved = std::collections::BTreeMap::new();
    for (name, agent_value) in agents {
        let value = resolve_tool_timeout_ms(&ToolTimeoutResolutionInput {
            call_value: rungs.tool_timeout_ms.as_ref(),
            agent_value,
            config_value: rungs.config_tool_timeout_ms.as_ref(),
            env_value: env_value.clone(),
        })?;
        resolved.insert(name.to_string(), value);
    }
    Ok(resolved)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// pi `effectiveToolTimeoutMs` (`tool-timeout.ts:33-36`), all three of its outcomes — and, in
    /// the first assertion, the ORDER of its two lines: the exemption is checked BEFORE the
    /// configured value, so an operator who sets `toolTimeoutMs` does not thereby put a clock on
    /// `contact_supervisor`. Killing mutation: swapping the two lines.
    #[test]
    fn an_exempt_tool_has_no_deadline_even_when_one_is_configured() {
        for tool in TOOL_TIMEOUT_EXEMPT_TOOLS {
            assert_eq!(effective_tool_timeout_ms(Some(tool), Some(1_000)), None);
            assert_eq!(effective_tool_timeout_ms(Some(tool), None), None);
        }
        // A fast builtin falls back to the five-minute default with nothing configured…
        for tool in DEFAULT_FAST_TOOL_TIMEOUT_TOOLS {
            assert_eq!(
                effective_tool_timeout_ms(Some(tool), None),
                Some(DEFAULT_FAST_TOOL_TIMEOUT_MS)
            );
            // …and the configured value REPLACES that default rather than being floored by it.
            assert_eq!(effective_tool_timeout_ms(Some(tool), Some(10)), Some(10));
        }
        // Any other tool has a deadline only if one was configured.
        assert_eq!(effective_tool_timeout_ms(Some("bash"), None), None);
        assert_eq!(effective_tool_timeout_ms(Some("bash"), Some(10)), Some(10));
        assert_eq!(effective_tool_timeout_ms(None, Some(10)), Some(10));
    }

    /// pi `resolveToolTimeoutMs` (`tool-timeout.ts:60-86`): the winner is the first rung that is
    /// PRESENT, not the first that is VALID. Killing mutation: `.filter(|v| valid(v))` anywhere in
    /// the ladder — the third case would then silently resolve to the agent's 7 000.
    #[test]
    fn the_first_present_rung_wins_even_when_it_is_the_malformed_one() {
        let five = serde_json::json!(5_000);
        let bad = serde_json::json!(0);
        assert_eq!(
            resolve_tool_timeout_ms(&ToolTimeoutResolutionInput {
                call_value: Some(&five),
                agent_value: Some(7_000),
                config_value: Some(&serde_json::json!(9_000)),
                env_value: Some("11000".to_string()),
            }),
            Ok(Some(5_000))
        );
        assert_eq!(
            resolve_tool_timeout_ms(&ToolTimeoutResolutionInput {
                agent_value: Some(7_000),
                config_value: Some(&serde_json::json!(9_000)),
                env_value: Some("11000".to_string()),
                ..ToolTimeoutResolutionInput::default()
            }),
            Ok(Some(7_000))
        );
        assert_eq!(
            resolve_tool_timeout_ms(&ToolTimeoutResolutionInput {
                call_value: Some(&bad),
                agent_value: Some(7_000),
                ..ToolTimeoutResolutionInput::default()
            }),
            Err("toolTimeoutMs must be a positive integer no larger than 2147483647.".to_string())
        );
        assert_eq!(
            resolve_tool_timeout_ms(&ToolTimeoutResolutionInput {
                config_value: Some(&serde_json::json!(9_000)),
                env_value: Some("11000".to_string()),
                ..ToolTimeoutResolutionInput::default()
            }),
            Ok(Some(9_000))
        );
        assert_eq!(
            resolve_tool_timeout_ms(&ToolTimeoutResolutionInput::default()),
            Ok(None)
        );
    }

    /// The env rung's own three rules (`tool-timeout.ts:73`, `:76-79`): it is consulted only when
    /// every typed rung is absent, a blank value is "unset" rather than "invalid", and it alone gets
    /// JS `Number()` coercion applied to its string.
    #[test]
    fn the_env_rung_is_lowest_blank_means_unset_and_only_it_is_number_coerced() {
        let env = |raw: &str| {
            resolve_tool_timeout_ms(&ToolTimeoutResolutionInput {
                env_value: Some(raw.to_string()),
                ..ToolTimeoutResolutionInput::default()
            })
        };
        assert_eq!(env("11000"), Ok(Some(11_000)));
        // `Number(" 11000 ")` is 11000 — whitespace is not a malformed value.
        assert_eq!(env("  11000  "), Ok(Some(11_000)));
        assert_eq!(env(""), Ok(None), "blank is unset, never a refusal");
        assert_eq!(env("   "), Ok(None));
        for bad in ["0", "-5", "1.5", "abc", "2147483648"] {
            assert_eq!(
                env(bad),
                Err(format!(
                    "{TOOL_TIMEOUT_ENV} must be a positive integer no larger than 2147483647."
                )),
                "{bad} must be refused under the env rung's own label"
            );
        }
        assert_eq!(env("2147483647"), Ok(Some(MAX_TIMER_DELAY_MS)));
        // A typed rung holding a STRING gets no coercion: upstream's `typeof raw !== "number"`
        // refusal applies. Killing mutation: coercing every rung.
        assert_eq!(
            resolve_tool_timeout_ms(&ToolTimeoutResolutionInput {
                config_value: Some(&serde_json::json!("9000")),
                ..ToolTimeoutResolutionInput::default()
            }),
            Err(
                "config.toolTimeoutMs must be a positive integer no larger than 2147483647."
                    .to_string()
            )
        );
    }

    /// CFG-067 — the exempt set is bound to this crate's own tool-name constants, and those
    /// constants still hold upstream's spellings. Both halves matter: the binding is what makes a
    /// rename impossible to get wrong, and this assertion is what catches a rename that DRIFTS from
    /// upstream (`tool-timeout.ts:19` @v0.71.0) rather than merely moving in step with it.
    #[test]
    fn the_exempt_set_is_bound_to_this_crates_own_tool_names() {
        assert_eq!(
            TOOL_TIMEOUT_EXEMPT_TOOLS,
            ["contact_supervisor", "intercom", "bg_wait"],
            "upstream's three exempt names, verbatim"
        );
        assert_eq!(
            DEFAULT_FAST_TOOL_TIMEOUT_TOOLS[6], "structured_output",
            "upstream's seventh fast-tool name, verbatim"
        );
    }

    /// pi `toolTimeoutCallKey` (`tool-timeout.ts:42-46`): the real call id when there is one, and a
    /// sequence-disambiguated anonymous key otherwise — so two concurrent anonymous calls to the
    /// same tool do not share one timer.
    #[test]
    fn the_call_key_falls_back_to_a_sequenced_anonymous_key() {
        assert_eq!(tool_timeout_call_key(Some("c1"), Some("read"), 7), "id:c1");
        assert_eq!(
            tool_timeout_call_key(Some(""), Some("read"), 7),
            "anon:read:7"
        );
        assert_eq!(tool_timeout_call_key(None, None, 7), "anon:tool:7");
    }

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    /// The tracker's arm/clear pairing, including `clearActiveToolTimeout`'s by-NAME fallback
    /// (`execution.ts:1235`) for a child that reports an end with no call id: the FIRST key still
    /// armed under that name is the one that clears.
    #[test]
    fn a_tool_end_clears_exactly_its_own_armed_deadline() {
        let now = Instant::now();
        let mut tracker = ToolTimeoutTracker::new(Some(1_000));
        tracker.arm(Some("c1"), "bash", now, None);
        tracker.arm(Some("c2"), "bash", now, None);
        tracker.arm(Some("c3"), "read", now, None);
        assert_eq!(
            tracker.armed_tools(),
            vec!["bash".to_string(), "read".to_string()]
        );
        tracker.clear(Some("c2"), Some("bash"));
        assert_eq!(tracker.armed.len(), 2);
        // No id: the first key still armed under `bash` (c1) goes, not c3's.
        tracker.clear(Some(""), Some("bash"));
        assert_eq!(
            tracker
                .armed
                .iter()
                .map(|a| a.key.clone())
                .collect::<Vec<_>>(),
            vec!["id:c3".to_string()]
        );
        // An unknown name clears nothing.
        tracker.clear(None, Some("grep"));
        assert_eq!(tracker.armed.len(), 1);
        tracker.clear_all();
        assert!(tracker.next_deadline().is_none());
    }

    /// The enforcement itself: the earliest armed deadline is the one the drive loop races, and
    /// firing it yields pi's verbatim message for THAT call's tool and timeout — not the run's.
    #[test]
    fn the_earliest_deadline_fires_first_and_carries_its_own_tools_message() {
        let now = Instant::now();
        let mut tracker = ToolTimeoutTracker::new(None);
        // `read` is a fast builtin: 300 000 ms by default.
        tracker.arm(Some("c1"), "read", now, None);
        // `bash` is not, and nothing is configured, so it is not armed at all.
        tracker.arm(Some("c2"), "bash", now, None);
        assert_eq!(tracker.armed_tools(), vec!["read".to_string()]);
        assert_eq!(
            tracker.next_deadline(),
            Some(at(now, DEFAULT_FAST_TOOL_TIMEOUT_MS))
        );
        assert!(
            tracker
                .expire(at(now, DEFAULT_FAST_TOOL_TIMEOUT_MS - 1))
                .is_none()
        );
        assert_eq!(
            tracker.expire(at(now, DEFAULT_FAST_TOOL_TIMEOUT_MS)),
            Some(ToolTimeoutExpiry {
                message: "Tool 'read' exceeded its timeout of 300000ms.".to_string()
            })
        );
        assert!(
            tracker.next_deadline().is_none(),
            "the fired key is removed"
        );
    }

    /// pi's run-remaining guard (`execution.ts:1267-1269`): a per-tool deadline that would land at
    /// or after the RUN's own deadline is never armed, so the run-level timeout owns that outcome
    /// and the operator gets one diagnosis instead of a race between two.
    /// Killing mutation: dropping the guard, or making it `>` instead of `>=`.
    #[test]
    fn a_deadline_at_or_past_the_runs_own_is_not_armed_at_all() {
        let now = Instant::now();
        let mut tracker = ToolTimeoutTracker::new(Some(1_000));
        tracker.arm(Some("c1"), "bash", now, Some(1_000));
        assert!(tracker.armed.is_empty(), "equal counts as past");
        tracker.arm(Some("c2"), "bash", now, Some(999));
        assert!(tracker.armed.is_empty());
        tracker.arm(Some("c3"), "bash", now, Some(1_001));
        assert_eq!(tracker.armed.len(), 1);
        // No run deadline at all: the guard never applies.
        tracker.clear_all();
        tracker.arm(Some("c4"), "bash", now, None);
        assert_eq!(tracker.armed.len(), 1);
    }

    /// The detached-runner resolution: one ladder per persona, with each persona's own
    /// `toolTimeoutMs:` between the call and config rungs, and a refusal that names the losing rung.
    #[test]
    fn the_per_agent_resolution_places_each_personas_own_value_between_the_call_and_config_rungs() {
        let rungs = ToolTimeoutRungs {
            tool_timeout_ms: None,
            config_tool_timeout_ms: Some(serde_json::json!(9_000)),
        };
        assert!(!rungs.is_empty());
        let resolved =
            resolve_tool_timeouts_by_agent(&rungs, [("scout", Some(7_000)), ("worker", None)])
                .expect("both ladders resolve");
        assert_eq!(resolved.get("scout").copied().flatten(), Some(7_000));
        assert_eq!(resolved.get("worker").copied().flatten(), Some(9_000));

        // A call rung outranks every persona.
        let with_call = ToolTimeoutRungs {
            tool_timeout_ms: Some(serde_json::json!(5_000)),
            config_tool_timeout_ms: Some(serde_json::json!(9_000)),
        };
        let resolved =
            resolve_tool_timeouts_by_agent(&with_call, [("scout", Some(7_000))]).expect("resolves");
        assert_eq!(resolved.get("scout").copied().flatten(), Some(5_000));

        // And a malformed rung refuses the whole run rather than one step.
        let bad = ToolTimeoutRungs {
            tool_timeout_ms: None,
            config_tool_timeout_ms: Some(serde_json::json!(-5)),
        };
        assert_eq!(
            resolve_tool_timeouts_by_agent(&bad, [("scout", None)]),
            Err(
                "config.toolTimeoutMs must be a positive integer no larger than 2147483647."
                    .to_string()
            )
        );
        assert!(
            ToolTimeoutRungs::default().is_empty(),
            "an empty pair means only the agent and env rungs can decide"
        );
    }
}
