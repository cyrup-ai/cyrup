//! The watchdog permission arbiter — a 1:1 port of
//! `pi-subagents/src/watchdog/permission-arbiter.ts` (145 lines @v0.43.0).
//!
//! When a CHILD subagent hits a tool whose policy is `ask`, there is no human in that process to
//! ask. This is the substitute: a single-purpose model call, run inside the child, that must call
//! `watchdog_permission_decision` exactly once with `approve` or `deny`.
//!
//! **Every path fails CLOSED.** That is the property, and it is worth enumerating because upstream
//! spends most of its lines on it — there are seven distinct ways to not get an approval and all
//! seven deny (`:66-72,127-137`):
//!
//! | cause | `decision` recorded | message |
//! |---|---|---|
//! | the child config does not decode | `unavailable` | `…configuration is invalid: <detail>` |
//! | there is no child watchdog | `unavailable` | `…unavailable because the child watchdog is disabled.` |
//! | the request or the context was cancelled BEFORE the turn | `cancelled` | `Watchdog permission decision was cancelled.` |
//! | it is cancelled DURING the turn | `error` | `…failed closed: Watchdog permission decision was aborted.` |
//! | the model called no tool | `malformed` | `…returned no decision.` |
//! | the turn exceeded `agentEndTimeoutMs` | `timeout` | `…failed closed: Watchdog permission decision timed out.` |
//! | anything else threw | `error` | `…failed closed: <detail>` |
//! | the model answered `deny` | `deny` | the model's own reason |
//!
//! Both audit records are written whatever happens (`:44,52-61`): a `permission.request` before any
//! work, and a `permission.decision` afterwards carrying `requestCreatedAt` so the two join. A
//! decision that is never written is indistinguishable from one that never happened, so the audit
//! append is the first and last thing this function does.
//!
//! ## Where it is wired
//!
//! Upstream has exactly one caller: `registerPermissionGate` (`subagent-prompt-runtime.ts:281-305`,
//! installed at `:475`) subscribes `tool_call` in the CHILD process, resolves the tool against the
//! policy the parent shipped in `PERMISSION_POLICY_ENV`, and calls `requestWatchdogPermission` for
//! every tool whose rule is `ask`. cyrup's port is
//! [`crate::prompt_runtime::PermissionGate`], reached from
//! [`crate::prompt_runtime::SubagentPromptRuntime`]'s `on_event` `ToolCall` arm.
//!
//! **This is not `cyrup-permission-system`'s gate, and "cyrup already has permissions" was never a
//! reason to leave this module uncalled.** The two are different policies answering different
//! questions in different processes, and both are upstream-real (pi CORE ships no permission
//! system at all, so neither one can be attributed to `pi/`):
//!
//! * `cyrup-permission-system` ports the third-party `pi-permission-system` extension. Its rules
//!   come from `cyrup-permissions.jsonc`, its `ask` tier means *ask a human*, and inside a subagent
//!   child it installs the `ForwardingAskChannel` that writes the question into the PARENT's
//!   `<agentDir>/sessions/permission-forwarding/` spool so the parent's operator answers it
//!   (`crate::spawn::nested_events`'s `child_role_env` doc, consumer #3).
//! * THIS gate ports `pi-subagents`' own policy. Its rules come from the agent definition and the
//!   extension config, travel to the child in `PERMISSION_POLICY_ENV`, and its `ask` tier means
//!   *ask a model* — precisely because a subagent child has no human to reach. It also refuses to
//!   gate `bash` (left to pi-guard) and the four internal coordination tools, which the other
//!   system does gate.
//!
//! A child can be under both at once; they compose the way two `tool_call` handlers compose, and
//! either one blocking is a block.
//!
//! ## The `permissions.ts` helpers ported inline
//!
//! `permissionArgsPreview`, `appendPermissionAudit` and — since the gate needs them — the policy
//! decode/decision half of `runs/shared/permissions.ts` live here, that file having no other cyrup
//! port. The parent-side half (`validatePermissionConfig`, `resolvePermissionRules`,
//! `encodePermissionRules`, and `pi-args.ts:713-758`'s env writes) is still unported, so a policy
//! reaches a child today only if something outside this crate sets
//! [`PERMISSION_POLICY_ENV`]; that is the remaining work, and it lives in `exec/`, not here.
//!
//! [CYRUP-DELTA] the model turn is the [`WatchdogPermissionAgent`] seam, for the same reason
//! [`super::review::WatchdogReviewAgent`] is — see that module's doc. Everything upstream does
//! AROUND the turn (config decode, cancellation, the timeout, the audit pair, the fail-closed
//! mapping, the reason truncation) is ported and runs regardless of which agent is bound; a
//! deployment with no agent bound denies with the `malformed` reason, which is the correct
//! fail-closed answer rather than a silent approval.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use cyrup_core::CancelToken;
use serde_json::{Map, Value, json};

use super::child_status::{ChildWatchdogConfig, decode_child_watchdog_config};

/// Redact any value under a key that looks like a credential (`permissions.ts:14`).
const SECRET_KEY_FRAGMENTS: &[&str] = &[
    "authorization",
    "cookie",
    "credential",
    "password",
    "secret",
    "token",
    "apikey",
    "api-key",
    "api_key",
];

/// `MAX_PREVIEW_BYTES` (`permissions.ts:13`).
const MAX_PREVIEW_BYTES: usize = 2048;
/// The redaction recursion cap (`permissions.ts:65`).
const MAX_REDACT_DEPTH: usize = 3;
/// Per-array and per-object element caps (`permissions.ts:66-67`).
const MAX_ARRAY_ITEMS: usize = 10;
/// Per-object key cap (`permissions.ts:67`).
const MAX_OBJECT_KEYS: usize = 20;
/// The per-string cap (`permissions.ts:70`).
const MAX_STRING_CHARS: usize = 500;
/// The reason cap (`permission-arbiter.ts:39`).
const MAX_REASON_CHARS: usize = 500;

/// `SECRET_KEY.test(key)` (`permissions.ts:64`), case-insensitively and ignoring `-`/`_` so
/// `api_key`, `api-key` and `apiKey` all match.
fn is_secret_key(key: &str) -> bool {
    let folded: String = key
        .chars()
        .filter(|c| *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect();
    SECRET_KEY_FRAGMENTS
        .iter()
        .any(|fragment| folded.contains(&fragment.replace(['-', '_'], "")))
}

/// The literal prefixes of `SECRET_VALUE`'s second alternation (`permissions.ts:15`):
/// `(?:sk|ghp|github_pat|xox[baprs])`, with the character class expanded. The `[-_A-Za-z0-9]{8,}`
/// run that must follow is NOT part of the prefix — upstream counts those eight characters from
/// immediately after `sk`/`ghp`/…, separator included.
const SECRET_VALUE_PREFIXES: [&str; 8] = [
    "sk",
    "ghp",
    "github_pat",
    "xoxb",
    "xoxa",
    "xoxp",
    "xoxr",
    "xoxs",
];

/// The minimum `[-_A-Za-z0-9]{8,}` run length (`permissions.ts:15`).
const SECRET_VALUE_MIN_TOKEN: usize = 8;

/// `\w` — the character class both `\b` assertions are defined against. JS `\b` is ASCII-only
/// regardless of the `u` flag, so every byte of a multi-byte UTF-8 sequence is a NON-word byte
/// here, exactly as every non-ASCII code point is in JS.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `[-_A-Za-z0-9]` (`permissions.ts:15`).
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// `\b` at byte offset `at`: exactly one side is a word character.
fn at_word_boundary(bytes: &[u8], at: usize) -> bool {
    let before = at
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .is_some_and(|byte| is_word_byte(*byte));
    let after = bytes.get(at).is_some_and(|byte| is_word_byte(*byte));
    before != after
}

/// `Bearer\s+\S+` anchored at `start`, case-insensitively, with the trailing `\b` satisfied by
/// backtracking the greedy `\S+` — the same backtrack the regex engine performs, which is why
/// `Bearer abc.` redacts `Bearer abc` and leaves the `.`.
///
/// The backtrack can never stop mid-UTF-8: a boundary needs a word byte on one side, and neither a
/// lead byte nor a continuation byte is one.
fn match_bearer(bytes: &[u8], start: usize) -> Option<usize> {
    let head = bytes.get(start..start.checked_add("bearer".len())?)?;
    if !head.eq_ignore_ascii_case(b"bearer") {
        return None;
    }
    let after_keyword = start.checked_add("bearer".len())?;
    let spaces = bytes
        .get(after_keyword..)?
        .iter()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count();
    if spaces == 0 {
        return None;
    }
    let token_start = after_keyword.checked_add(spaces)?;
    let mut length = bytes
        .get(token_start..)?
        .iter()
        .take_while(|byte| !byte.is_ascii_whitespace())
        .count();
    while length >= 1 && !at_word_boundary(bytes, token_start.checked_add(length)?) {
        length -= 1;
    }
    (length >= 1).then(|| token_start.saturating_add(length))
}

/// `(?:sk|ghp|github_pat|xox[baprs])[-_A-Za-z0-9]{8,}\b` anchored at `start`, case-insensitively.
fn match_prefixed_token(bytes: &[u8], start: usize) -> Option<usize> {
    for prefix in SECRET_VALUE_PREFIXES {
        let Some(end_of_prefix) = start.checked_add(prefix.len()) else {
            continue;
        };
        let Some(head) = bytes.get(start..end_of_prefix) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(prefix.as_bytes()) {
            continue;
        }
        let Some(tail) = bytes.get(end_of_prefix..) else {
            continue;
        };
        let mut length = tail.iter().take_while(|byte| is_token_byte(**byte)).count();
        while length >= SECRET_VALUE_MIN_TOKEN
            && !at_word_boundary(bytes, end_of_prefix.saturating_add(length))
        {
            length -= 1;
        }
        if length >= SECRET_VALUE_MIN_TOKEN {
            return Some(end_of_prefix.saturating_add(length));
        }
    }
    None
}

/// `value.replace(SECRET_VALUE, "[redacted]")` (`permissions.ts:15,70`), where
/// `SECRET_VALUE = /\b(?:Bearer\s+\S+|(?:sk|ghp|github_pat|xox[baprs])[-_A-Za-z0-9]{8,})\b/gi`.
///
/// **The `i` flag is a security property, not a formatting nicety.** The previous port matched a
/// fixed list of literally-spelled prefixes (`"Bearer "`, `"sk-"`, `"ghp_"`, …), so a real
/// credential written `BEARER <token>`, `SK-…` or `GHP_…` — and every `sk`/`ghp`/`xox` token with
/// no separator, which the `-`/`_` in those literals also required — passed through unredacted
/// into the arbiter's prompt AND into the on-disk audit log. This walks the string the way the
/// regex does: at every `\b`, try the `Bearer` alternation and then the prefixed-token
/// alternation, both ASCII-case-insensitively.
fn redact_secret_values(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if at_word_boundary(bytes, index)
            && let Some(end) =
                match_bearer(bytes, index).or_else(|| match_prefixed_token(bytes, index))
            && end > index
        {
            out.extend_from_slice(b"[redacted]");
            index = end;
            continue;
        }
        if let Some(byte) = bytes.get(index) {
            out.push(*byte);
        }
        index = index.saturating_add(1);
    }
    // Lossless by construction: every byte is either copied verbatim or replaced with a whole
    // ASCII marker, and no match can end inside a multi-byte sequence (see [`match_bearer`]).
    String::from_utf8_lossy(&out).into_owned()
}

/// `redact` (`permissions.ts:63-73`).
fn redact(value: &Value, key: &str, depth: usize) -> Value {
    if is_secret_key(key) {
        return Value::String("[redacted]".to_string());
    }
    if depth >= MAX_REDACT_DEPTH {
        return Value::String("[truncated]".to_string());
    }
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(MAX_ARRAY_ITEMS)
                .map(|item| redact(item, "", depth + 1))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .take(MAX_OBJECT_KEYS)
                .map(|(entry_key, entry_value)| {
                    (entry_key.clone(), redact(entry_value, entry_key, depth + 1))
                })
                .collect(),
        ),
        Value::String(text) => {
            let redacted = redact_secret_values(text);
            if redacted.chars().count() > MAX_STRING_CHARS {
                let kept: String = redacted.chars().take(MAX_STRING_CHARS).collect();
                Value::String(format!("{kept}\u{2026}"))
            } else {
                Value::String(redacted)
            }
        }
        other => other.clone(),
    }
}

/// `permissionArgsPreview` (`permissions.ts:75-89`) — the redacted, byte-capped argument summary the
/// arbiter shows the model and writes to the audit. The cap is in BYTES and truncates on a character
/// boundary, appending an ellipsis.
#[must_use]
pub fn permission_args_preview(input: &Value) -> String {
    let serialized = serde_json::to_string(&redact(input, "", 0)).unwrap_or_default();
    if serialized.is_empty() {
        return "{}".to_string();
    }
    if serialized.len() <= MAX_PREVIEW_BYTES {
        return serialized;
    }
    let max_content_bytes = MAX_PREVIEW_BYTES - "\u{2026}".len();
    let mut preview = String::new();
    let mut preview_bytes = 0usize;
    for character in serialized.chars() {
        let character_bytes = character.len_utf8();
        if preview_bytes + character_bytes > max_content_bytes {
            break;
        }
        preview.push(character);
        preview_bytes += character_bytes;
    }
    format!("{preview}\u{2026}")
}

/// `appendPermissionAudit` (`permissions.ts:91-95`) — one JSON line, `0700` on the directory and
/// `0600` on the file, and a no-op when there is no audit path configured.
///
/// Failures are swallowed exactly as upstream's un-caught-but-advisory append is not allowed to
/// take a decision down; the DECISION is what matters, and losing the record must not turn a deny
/// into an exception the caller mishandles.
pub fn append_permission_audit(file_path: Option<&Path>, record: &Value) {
    let Some(file_path) = file_path else {
        return;
    };
    if let Some(parent) = file_path.parent() {
        let _ = std::fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let Ok(line) = serde_json::to_string(record) else {
        return;
    };
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(file_path) {
        use std::io::Write;
        let _ = writeln!(file, "{line}");
    }
}

// =================================================================================================
// The policy the arbiter answers FOR (`runs/shared/permissions.ts:4-61`)
// =================================================================================================

/// `PERMISSION_POLICY_ENV` (`permissions.ts:8`), under cyrup's `CYRUP_SUBAGENT_*` spelling — the
/// same rename every other member of that family carries (`CYRUP_SUBAGENT_TOOL_BUDGET`,
/// `CYRUP_SUBAGENT_STEER_INBOX`, …).
pub const PERMISSION_POLICY_ENV: &str = "CYRUP_SUBAGENT_PERMISSION_POLICY";

/// `PERMISSION_AUDIT_PATH_ENV` (`permissions.ts:9`).
pub const PERMISSION_AUDIT_PATH_ENV: &str = "CYRUP_SUBAGENT_PERMISSION_AUDIT_PATH";

/// `INTERNAL_TOOLS` (`permissions.ts:10`) — the child's own coordination surface, which a policy
/// may not gate at all: gating it would let a rule strand a child that cannot report back.
const INTERNAL_TOOLS: [&str; 4] = [
    "contact_supervisor",
    "intercom",
    "subagent_wait",
    "structured_output",
];

/// `PermissionDecision` (`permissions.ts:4`).
///
/// SUBA-073 — `Serialize`/`Deserialize` added (`rename_all = "lowercase"`, matching
/// [`Self::as_str`]'s own wire spelling exactly) so a whole [`PermissionRules`] map can cross the
/// hop-2 detached-runner process boundary as ordinary JSON inside
/// [`crate::background::runner_main::RunnerConfig`], the same way every other resolved-config-rung
/// value there does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionRuleDecision {
    /// Run the tool.
    Allow,
    /// Ask the arbiter ([`request_watchdog_permission`]).
    Ask,
    /// Refuse the tool outright.
    Deny,
}

impl PermissionRuleDecision {
    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }

    /// `DECISIONS.has(decision)` (`permissions.ts:11`).
    fn parse(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// `PermissionRules` (`permissions.ts:5`) — tool name to decision.
pub type PermissionRules = std::collections::BTreeMap<String, PermissionRuleDecision>;

/// `validatePermissionRules(value, label)` (`permissions.ts:17-29`).
///
/// # Errors
///
/// Upstream's five throws, verbatim: a non-object policy, an empty tool name, `bash` (left to
/// pi-guard), a reserved internal tool, and an unrecognized decision.
pub fn validate_permission_rules(
    value: Option<&Value>,
    label: &str,
) -> Result<Option<PermissionRules>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(object) = value.as_object() else {
        return Err(format!(
            "{label} must be an object mapping tool names to allow, ask, or deny."
        ));
    };
    let mut result = PermissionRules::new();
    for (tool, decision) in object {
        if tool.trim().is_empty() {
            return Err(format!("{label} contains an empty tool name."));
        }
        if tool == "bash" {
            return Err(format!(
                "{label}.bash is unsupported; pi-subagents leaves bash policy to pi-guard."
            ));
        }
        if INTERNAL_TOOLS.contains(&tool.as_str()) {
            return Err(format!(
                "{label}.{tool} is reserved for child coordination and cannot be gated."
            ));
        }
        let Some(decision) = decision.as_str().and_then(PermissionRuleDecision::parse) else {
            return Err(format!("{label}.{tool} must be allow, ask, or deny."));
        };
        result.insert(tool.clone(), decision);
    }
    Ok((!result.is_empty()).then_some(result))
}

/// `decodePermissionRules(encoded)` (`permissions.ts:58-61`) — the child's side of the policy: a
/// blank value is the same as unset.
///
/// # Errors
///
/// Malformed JSON, or anything [`validate_permission_rules`] rejects.
pub fn decode_permission_rules(encoded: Option<&str>) -> Result<Option<PermissionRules>, String> {
    let Some(encoded) = encoded.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(encoded)
        .map_err(|error| format!("{PERMISSION_POLICY_ENV} is not valid JSON: {error}"))?;
    validate_permission_rules(Some(&value), PERMISSION_POLICY_ENV)
}

/// `permissionDecision(rules, toolName)` (`permissions.ts:46-49`).
///
/// `bash` and the internal coordination tools are ALWAYS allowed here regardless of the rules —
/// [`validate_permission_rules`] already refuses to record a rule for any of them, so this is the
/// second of two enforcement points, not a redundant one: the rules can also arrive from a
/// parent running a different version.
#[must_use]
pub fn permission_decision(
    rules: Option<&PermissionRules>,
    tool_name: &str,
) -> PermissionRuleDecision {
    if tool_name == "bash" || INTERNAL_TOOLS.contains(&tool_name) {
        return PermissionRuleDecision::Allow;
    }
    rules
        .and_then(|rules| rules.get(tool_name).copied())
        .unwrap_or(PermissionRuleDecision::Allow)
}

/// `WatchdogPermissionResult` (`permission-arbiter.ts:17-21`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogPermissionResult {
    /// Whether the child tool call may proceed.
    pub approved: bool,
    /// One concise sentence, always non-empty.
    pub reason: String,
    /// Always `"watchdog"`.
    pub source: &'static str,
}

/// `WatchdogPermissionRequest` (`permission-arbiter.ts:23-30`).
#[derive(Debug, Clone)]
pub struct WatchdogPermissionRequest {
    /// The tool the child wants to run.
    pub tool_name: String,
    /// Its arguments, redacted before they reach the model or the audit.
    pub args: Value,
    /// The raw [`super::child_status::CHILD_WATCHDOG_CONFIG_ENV`] value.
    pub raw_watchdog_config: Option<String>,
    /// Where to append the audit pair.
    pub audit_path: Option<PathBuf>,
    /// Cancellation.
    pub cancel: Option<CancelToken>,
}

/// The model's answer (`PermissionDecisionParams`, `permission-arbiter.ts:11-14`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogPermissionDecision {
    /// `approve` or `deny`.
    pub decision: String,
    /// One concise reason for this exact decision.
    pub reason: String,
}

/// The `watchdog_permission_decision` tool schema (`permission-arbiter.ts:11-14`).
#[must_use]
pub fn permission_decision_parameters_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["decision", "reason"],
        "properties": {
            "decision": { "type": "string", "enum": ["approve", "deny"] },
            "reason": { "type": "string", "description": "One concise reason for this exact decision." },
        },
    })
}

/// The arbiter's system prompt (`permission-arbiter.ts:130-135`), lines in upstream's order.
#[must_use]
pub fn permission_arbiter_system_prompt() -> String {
    [
        "You are the pi-subagents watchdog permission arbiter.",
        "Decide only whether this exact non-bash child tool call should proceed.",
        "Call watchdog_permission_decision exactly once with approve or deny and a concise reason.",
        "Deny when uncertain. Do not produce freeform advice or ask the parent orchestrator.",
    ]
    .join("\n")
}

/// The user prompt (`permission-arbiter.ts:151`).
#[must_use]
pub fn permission_arbiter_prompt(tool_name: &str, preview: &str) -> String {
    format!("Tool: {tool_name}\nRedacted arguments: {preview}")
}

/// One arbiter turn's input.
pub struct WatchdogPermissionTurn<'a> {
    /// The child watchdog config, for the model/thinking selection.
    pub config: &'a ChildWatchdogConfig,
    /// [`permission_arbiter_system_prompt`].
    pub system_prompt: String,
    /// [`permission_arbiter_prompt`].
    pub prompt: String,
    /// [`permission_decision_parameters_schema`].
    pub decision_tool_schema: Value,
    /// Cancellation.
    pub cancel: CancelToken,
}

/// The single arbiter turn (`permission-arbiter.ts:126-155`) as a seam.
///
/// An implementation MUST expose exactly one tool, `watchdog_permission_decision`, and block every
/// other call (upstream's `beforeToolCall`, `:143`). `Ok(None)` is upstream's "the model called no
/// tool", which denies as `malformed`.
#[async_trait]
pub trait WatchdogPermissionAgent: Send + Sync {
    /// Run the arbiter turn.
    ///
    /// # Errors
    ///
    /// Any provider or transport failure, which denies as `error`.
    async fn decide(
        &self,
        turn: WatchdogPermissionTurn<'_>,
    ) -> Result<Option<WatchdogPermissionDecision>, String>;
}

/// The no-agent stand-in: reaches no decision, so the arbiter denies as `malformed`.
///
/// This is the correct behaviour for a deployment with no provider bound — the arbiter exists to
/// answer an `ask` in a process with no human, and "no answer" must never mean "allowed".
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDecisionPermissionAgent;

#[async_trait]
impl WatchdogPermissionAgent for NoDecisionPermissionAgent {
    async fn decide(
        &self,
        _turn: WatchdogPermissionTurn<'_>,
    ) -> Result<Option<WatchdogPermissionDecision>, String> {
        Ok(None)
    }
}

/// `conciseReason` (`permission-arbiter.ts:37-40`).
fn concise_reason(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "Watchdog returned an empty reason.".to_string();
    }
    trimmed.chars().take(MAX_REASON_CHARS).collect()
}

/// Epoch milliseconds (`Date.now()`).
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// `finish` (`permission-arbiter.ts:47-61`): write the decision record, then return it.
fn finish(
    request: &WatchdogPermissionRequest,
    created_at: i64,
    approved: bool,
    reason: &str,
    decision: &str,
) -> WatchdogPermissionResult {
    let reason = concise_reason(reason);
    let mut record = Map::new();
    record.insert("type".into(), json!("permission.decision"));
    record.insert("createdAt".into(), json!(now_millis()));
    record.insert("requestCreatedAt".into(), json!(created_at));
    record.insert("toolName".into(), json!(request.tool_name));
    record.insert("decision".into(), json!(decision));
    record.insert("approved".into(), json!(approved));
    record.insert("decisionSource".into(), json!("watchdog"));
    record.insert("reason".into(), json!(reason));
    append_permission_audit(request.audit_path.as_deref(), &Value::Object(record));
    WatchdogPermissionResult {
        approved,
        reason,
        source: "watchdog",
    }
}

/// `createWatchdogPermissionArbiter(...)`'s returned function (`permission-arbiter.ts:42-145`).
///
/// Never returns an error: every failure mode is a DENIAL with an explanatory reason, because a
/// caller that has to interpret an error to decide whether a tool may run is one bad `match` away
/// from failing open.
pub async fn request_watchdog_permission(
    request: &WatchdogPermissionRequest,
    agent: &dyn WatchdogPermissionAgent,
) -> WatchdogPermissionResult {
    let preview = permission_args_preview(&request.args);
    let created_at = now_millis();
    let mut base = Map::new();
    base.insert("type".into(), json!("permission.request"));
    base.insert("createdAt".into(), json!(created_at));
    base.insert("toolName".into(), json!(request.tool_name));
    base.insert("preview".into(), json!(preview));
    base.insert("matchedRule".into(), json!("ask"));
    base.insert("decisionSource".into(), json!("watchdog"));
    append_permission_audit(request.audit_path.as_deref(), &Value::Object(base));

    let child_config = match decode_child_watchdog_config(request.raw_watchdog_config.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            return finish(
                request,
                created_at,
                false,
                &format!("Watchdog permission arbiter configuration is invalid: {error}"),
                "unavailable",
            );
        }
    };
    let Some(child_config) = child_config else {
        return finish(
            request,
            created_at,
            false,
            "Watchdog permission arbiter is unavailable because the child watchdog is disabled.",
            "unavailable",
        );
    };
    let cancel = request.cancel.clone().unwrap_or_default();
    if cancel.is_cancelled() {
        return finish(
            request,
            created_at,
            false,
            "Watchdog permission decision was cancelled.",
            "cancelled",
        );
    }

    let turn = WatchdogPermissionTurn {
        config: &child_config,
        system_prompt: permission_arbiter_system_prompt(),
        prompt: permission_arbiter_prompt(&request.tool_name, &preview),
        decision_tool_schema: permission_decision_parameters_schema(),
        cancel: cancel.clone(),
    };
    // `Promise.race([agent.prompt(...), timeout])` (`:147-153`): the timeout aborts the agent and
    // rejects, which lands in the catch below as a `timeout` decision.
    let outcome = tokio::select! {
        biased;
        () = cancel.cancelled() => Err("Watchdog permission decision was aborted.".to_string()),
        raced = tokio::time::timeout(
            Duration::from_millis(child_config.agent_end_timeout_ms),
            agent.decide(turn),
        ) => match raced {
            Ok(result) => result,
            Err(_) => {
                cancel.cancel();
                Err("Watchdog permission decision timed out.".to_string())
            }
        },
    };

    match outcome {
        Ok(Some(decision)) => {
            let approved = decision.decision == "approve";
            finish(
                request,
                created_at,
                approved,
                &decision.reason,
                &decision.decision,
            )
        }
        Ok(None) => finish(
            request,
            created_at,
            false,
            "Watchdog permission arbiter returned no decision.",
            "malformed",
        ),
        Err(reason) => {
            // A cancel that lands DURING the turn takes this generic catch arm, not the `cancelled`
            // one: upstream's mid-turn abort aborts the agent, whose rejected `prompt` lands in
            // `catch` (`:127-137`) and is reported as `error`. Only the PRE-turn check (`:73`)
            // produces the `cancelled` decision.
            let decision = if reason.contains("timed out") {
                "timeout"
            } else {
                "error"
            };
            finish(
                request,
                created_at,
                false,
                &format!("Watchdog permission arbiter failed closed: {reason}"),
                decision,
            )
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::watchdog::child_status::{
        encode_child_watchdog_config, resolve_child_watchdog_config,
    };
    use crate::watchdog::settings::default_watchdog_config;
    use tempfile::TempDir;

    fn enabled_child_config() -> String {
        let mut config = default_watchdog_config();
        config.enabled = true;
        config.children.enabled = true;
        config.agent_end_timeout_ms = 5_000;
        encode_child_watchdog_config(
            resolve_child_watchdog_config(&config, None, None, None).as_ref(),
        )
        .unwrap()
    }

    fn request(raw: Option<String>, audit: Option<PathBuf>) -> WatchdogPermissionRequest {
        WatchdogPermissionRequest {
            tool_name: "write".into(),
            args: json!({ "path": "a.rs", "apiKey": "sk-abcdefghijkl" }),
            raw_watchdog_config: raw,
            audit_path: audit,
            cancel: None,
        }
    }

    struct FixedAgent(Option<WatchdogPermissionDecision>);

    #[async_trait]
    impl WatchdogPermissionAgent for FixedAgent {
        async fn decide(
            &self,
            _turn: WatchdogPermissionTurn<'_>,
        ) -> Result<Option<WatchdogPermissionDecision>, String> {
            Ok(self.0.clone())
        }
    }

    struct FailingAgent(&'static str);

    #[async_trait]
    impl WatchdogPermissionAgent for FailingAgent {
        async fn decide(
            &self,
            _turn: WatchdogPermissionTurn<'_>,
        ) -> Result<Option<WatchdogPermissionDecision>, String> {
            Err(self.0.to_string())
        }
    }

    struct HangingAgent;

    #[async_trait]
    impl WatchdogPermissionAgent for HangingAgent {
        async fn decide(
            &self,
            _turn: WatchdogPermissionTurn<'_>,
        ) -> Result<Option<WatchdogPermissionDecision>, String> {
            std::future::pending::<()>().await;
            unreachable!("pending never resolves")
        }
    }

    #[test]
    fn secret_keys_and_secret_values_are_both_redacted() {
        let preview = permission_args_preview(&json!({
            "Authorization": "Bearer abc123",
            "api_key": "plain",
            "note": "token is sk-abcdefghijklmnop here",
            "safe": "kept",
        }));
        assert!(preview.contains("\"Authorization\":\"[redacted]\""));
        assert!(preview.contains("\"api_key\":\"[redacted]\""));
        assert!(preview.contains("\"note\":\"token is [redacted] here\""));
        assert!(preview.contains("\"safe\":\"kept\""));
    }

    /// `SECRET_VALUE` ends `/gi` (`permissions.ts:15`). Dropping the `i` under-redacts REAL
    /// credentials — a `Bearer` header is routinely spelled with any casing, and the token prefixes
    /// are matched case-insensitively upstream — and the un-redacted value went into the arbiter's
    /// prompt and into the on-disk audit log.
    #[test]
    fn secret_values_are_matched_case_insensitively_like_the_upstream_regex() {
        for value in [
            "Bearer abcdef",
            "bearer abcdef",
            "BEARER abcdef",
            "BeArEr abcdef",
        ] {
            assert_eq!(
                redact_secret_values(value),
                "[redacted]",
                "case-insensitive Bearer: {value}"
            );
        }
        for value in [
            "sk-abcdefgh",
            "SK-ABCDEFGH",
            "Sk_AbCdEfGh",
            "ghp-abcdefgh",
            "GHP_ABCDEFGH",
            "github_pat_abcdefgh",
            "GITHUB_PAT_ABCDEFGH",
            "xoxb-abcdefgh",
            "XOXP-ABCDEFGH",
            "xoxs_abcdefgh",
        ] {
            assert_eq!(
                redact_secret_values(value),
                "[redacted]",
                "case-insensitive prefix: {value}"
            );
        }
    }

    /// The regex's structure, not just its case folding: the `{8,}` run is counted from
    /// immediately after the literal prefix (separator included), both `\b` assertions hold, and
    /// `/g` replaces every occurrence.
    #[test]
    fn the_secret_value_scan_matches_the_regexs_boundaries_and_length_rule() {
        // Eight token characters counted from after `sk` — the separator is one of them.
        assert_eq!(redact_secret_values("sk-abcdefg"), "[redacted]");
        assert_eq!(redact_secret_values("sk-abcdef"), "sk-abcdef");
        // No separator at all still matches upstream (`sk` + 8 token chars).
        assert_eq!(redact_secret_values("skABCDEFGH"), "[redacted]");
        // Leading `\b`: a prefix glued to a preceding word character is not a match.
        assert_eq!(redact_secret_values("xsk-abcdefgh"), "xsk-abcdefgh");
        // Trailing `\b` backtracks off a non-word tail.
        assert_eq!(redact_secret_values("sk-abcdefgh."), "[redacted].");
        assert_eq!(redact_secret_values("Bearer abc."), "[redacted].");
        // `Bearer` needs at least one space and one non-space token.
        assert_eq!(redact_secret_values("Bearerabc"), "Bearerabc");
        assert_eq!(redact_secret_values("Bearer "), "Bearer ");
        // `/g` — every occurrence, and surrounding text is preserved verbatim.
        assert_eq!(
            redact_secret_values("a sk-abcdefgh b GHP_ABCDEFGH c"),
            "a [redacted] b [redacted] c"
        );
        // Non-ASCII survives intact.
        assert_eq!(
            redact_secret_values("héllo sk-abcdefgh ✓"),
            "héllo [redacted] ✓"
        );
        assert_eq!(redact_secret_values("Bearer café"), "[redacted]é");
    }

    #[test]
    fn redaction_truncates_depth_arrays_and_long_strings() {
        let deep = json!({ "a": { "b": { "c": { "d": 1 } } } });
        assert!(permission_args_preview(&deep).contains("[truncated]"));
        let wide = json!({ "items": (0..30).collect::<Vec<u32>>() });
        let preview = permission_args_preview(&wide);
        assert_eq!(preview.matches(',').count(), MAX_ARRAY_ITEMS - 1);
        let long = json!({ "s": "x".repeat(MAX_STRING_CHARS + 50) });
        assert!(permission_args_preview(&long).contains('\u{2026}'));
    }

    #[test]
    fn a_giant_preview_is_capped_in_bytes_with_an_ellipsis() {
        // Arrays cap at 10 items and strings at 500 chars, so the only way past the 2 048-BYTE
        // preview cap is many keys: 20 keys x ~500 chars is ~10 KB before the byte cap applies.
        let huge = json!(
            (0..MAX_OBJECT_KEYS)
                .map(|i| (format!("k{i}"), json!("v".repeat(400))))
                .collect::<serde_json::Map<String, Value>>()
        );
        let preview = permission_args_preview(&huge);
        assert!(preview.len() <= MAX_PREVIEW_BYTES, "{}", preview.len());
        assert!(preview.ends_with('\u{2026}'));
    }

    #[test]
    fn an_empty_reason_gets_the_placeholder_and_a_long_one_is_capped() {
        assert_eq!(concise_reason("   "), "Watchdog returned an empty reason.");
        assert_eq!(concise_reason("  ok  "), "ok");
        assert_eq!(
            concise_reason(&"y".repeat(MAX_REASON_CHARS + 100))
                .chars()
                .count(),
            MAX_REASON_CHARS
        );
    }

    #[tokio::test]
    async fn a_malformed_child_config_denies_as_unavailable() {
        let result = request_watchdog_permission(
            &request(Some("{\"enabled\":true}".into()), None),
            &NoDecisionPermissionAgent,
        )
        .await;
        assert!(!result.approved);
        assert!(
            result
                .reason
                .starts_with("Watchdog permission arbiter configuration is invalid:")
        );
        assert_eq!(result.source, "watchdog");
    }

    #[tokio::test]
    async fn an_absent_or_disabled_child_watchdog_denies_as_unavailable() {
        for raw in [None, Some("{\"enabled\":false}".to_string())] {
            let result =
                request_watchdog_permission(&request(raw, None), &NoDecisionPermissionAgent).await;
            assert!(!result.approved);
            assert_eq!(
                result.reason,
                "Watchdog permission arbiter is unavailable because the child watchdog is disabled."
            );
        }
    }

    #[tokio::test]
    async fn a_cancelled_request_denies_before_any_turn() {
        let mut req = request(Some(enabled_child_config()), None);
        let cancel = CancelToken::new();
        cancel.cancel();
        req.cancel = Some(cancel);
        let result = request_watchdog_permission(&req, &NoDecisionPermissionAgent).await;
        assert!(!result.approved);
        assert_eq!(result.reason, "Watchdog permission decision was cancelled.");
    }

    #[tokio::test]
    async fn no_decision_denies_as_malformed() {
        let result = request_watchdog_permission(
            &request(Some(enabled_child_config()), None),
            &NoDecisionPermissionAgent,
        )
        .await;
        assert!(!result.approved);
        assert_eq!(
            result.reason,
            "Watchdog permission arbiter returned no decision."
        );
    }

    #[tokio::test]
    async fn an_agent_failure_denies_as_error_and_keeps_the_detail() {
        let result = request_watchdog_permission(
            &request(Some(enabled_child_config()), None),
            &FailingAgent("provider exploded"),
        )
        .await;
        assert!(!result.approved);
        assert_eq!(
            result.reason,
            "Watchdog permission arbiter failed closed: provider exploded"
        );
    }

    #[tokio::test]
    async fn a_hanging_agent_denies_on_the_configured_timeout() {
        let mut config = default_watchdog_config();
        config.enabled = true;
        config.children.enabled = true;
        config.agent_end_timeout_ms = 20;
        let raw = encode_child_watchdog_config(
            resolve_child_watchdog_config(&config, None, None, None).as_ref(),
        );
        let result = request_watchdog_permission(&request(raw, None), &HangingAgent).await;
        assert!(!result.approved);
        assert_eq!(
            result.reason,
            "Watchdog permission arbiter failed closed: Watchdog permission decision timed out."
        );
    }

    #[tokio::test]
    async fn approve_is_the_only_value_that_approves() {
        let approved = request_watchdog_permission(
            &request(Some(enabled_child_config()), None),
            &FixedAgent(Some(WatchdogPermissionDecision {
                decision: "approve".into(),
                reason: "safe write inside the worktree".into(),
            })),
        )
        .await;
        assert!(approved.approved);
        assert_eq!(approved.reason, "safe write inside the worktree");
        for spelling in ["deny", "APPROVE", "allow", ""] {
            let result = request_watchdog_permission(
                &request(Some(enabled_child_config()), None),
                &FixedAgent(Some(WatchdogPermissionDecision {
                    decision: spelling.into(),
                    reason: "because".into(),
                })),
            )
            .await;
            assert!(!result.approved, "{spelling} must not approve");
        }
    }

    #[tokio::test]
    async fn both_audit_records_are_written_and_join_on_the_request_timestamp() {
        let tmp = TempDir::new().unwrap();
        let audit = tmp.path().join("nested").join("audit.jsonl");
        let result = request_watchdog_permission(
            &request(Some(enabled_child_config()), Some(audit.clone())),
            &FixedAgent(Some(WatchdogPermissionDecision {
                decision: "deny".into(),
                reason: "writes outside scope".into(),
            })),
        )
        .await;
        assert!(!result.approved);
        let lines: Vec<Value> = std::fs::read_to_string(&audit)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["type"], json!("permission.request"));
        assert_eq!(lines[0]["matchedRule"], json!("ask"));
        assert_eq!(lines[0]["decisionSource"], json!("watchdog"));
        // The preview in the audit is the REDACTED one.
        assert!(lines[0]["preview"].as_str().unwrap().contains("[redacted]"));
        assert_eq!(lines[1]["type"], json!("permission.decision"));
        assert_eq!(lines[1]["decision"], json!("deny"));
        assert_eq!(lines[1]["approved"], json!(false));
        assert_eq!(lines[1]["requestCreatedAt"], lines[0]["createdAt"]);
    }

    #[tokio::test]
    async fn the_audit_pair_is_written_even_when_the_config_never_decodes() {
        let tmp = TempDir::new().unwrap();
        let audit = tmp.path().join("audit.jsonl");
        let _ = request_watchdog_permission(
            &request(None, Some(audit.clone())),
            &NoDecisionPermissionAgent,
        )
        .await;
        assert_eq!(std::fs::read_to_string(&audit).unwrap().lines().count(), 2);
    }

    // ---- the policy the gate reads (`permissions.ts:17-61`) -------------------------------------

    #[test]
    fn a_policy_decodes_to_its_rules_and_a_blank_value_is_the_same_as_unset() {
        let rules = decode_permission_rules(Some("{\"write\":\"deny\",\"fetch\":\"ask\"}"))
            .unwrap()
            .unwrap();
        assert_eq!(rules.get("write"), Some(&PermissionRuleDecision::Deny));
        assert_eq!(rules.get("fetch"), Some(&PermissionRuleDecision::Ask));
        assert!(decode_permission_rules(None).unwrap().is_none());
        assert!(decode_permission_rules(Some("   ")).unwrap().is_none());
        // An all-`allow` policy is the same as no policy (`permissions.ts:28`).
        assert!(
            decode_permission_rules(Some("{}")).unwrap().is_none(),
            "an empty rule set installs no gate"
        );
    }

    #[test]
    fn the_five_upstream_validation_errors_all_fire() {
        let cases = [
            (
                "[]",
                "must be an object mapping tool names to allow, ask, or deny.",
            ),
            ("{\"  \":\"deny\"}", "contains an empty tool name."),
            (
                "{\"bash\":\"deny\"}",
                "bash is unsupported; pi-subagents leaves bash policy to pi-guard.",
            ),
            (
                "{\"contact_supervisor\":\"deny\"}",
                "reserved for child coordination and cannot be gated.",
            ),
            ("{\"write\":\"maybe\"}", "must be allow, ask, or deny."),
        ];
        for (raw, fragment) in cases {
            let error = decode_permission_rules(Some(raw)).unwrap_err();
            assert!(error.contains(fragment), "{raw} -> {error}");
        }
        assert!(
            decode_permission_rules(Some("not json"))
                .unwrap_err()
                .contains("not valid JSON"),
        );
    }

    #[test]
    fn bash_and_the_internal_tools_are_allowed_even_when_a_rule_says_otherwise() {
        // A rule set that validation would refuse, built directly — a parent on another version
        // could still ship it, which is why the decision function checks again.
        let mut rules = PermissionRules::new();
        for tool in [
            "bash",
            "contact_supervisor",
            "intercom",
            "subagent_wait",
            "structured_output",
        ] {
            rules.insert(tool.to_string(), PermissionRuleDecision::Deny);
            assert_eq!(
                permission_decision(Some(&rules), tool),
                PermissionRuleDecision::Allow,
                "{tool}"
            );
        }
        // An unlisted tool defaults to allow; a listed one gets its rule.
        rules.insert("write".to_string(), PermissionRuleDecision::Ask);
        assert_eq!(
            permission_decision(Some(&rules), "write"),
            PermissionRuleDecision::Ask
        );
        assert_eq!(
            permission_decision(Some(&rules), "read"),
            PermissionRuleDecision::Allow
        );
        assert_eq!(
            permission_decision(None, "write"),
            PermissionRuleDecision::Allow
        );
    }

    #[test]
    fn the_arbiter_prompts_name_the_tool_and_carry_the_redacted_preview() {
        assert!(
            permission_arbiter_system_prompt()
                .starts_with("You are the pi-subagents watchdog permission arbiter.")
        );
        assert!(permission_arbiter_system_prompt().contains("Deny when uncertain."));
        assert_eq!(
            permission_arbiter_prompt("write", "{\"a\":1}"),
            "Tool: write\nRedacted arguments: {\"a\":1}"
        );
        let schema = permission_decision_parameters_schema();
        assert_eq!(
            schema["properties"]["decision"]["enum"],
            json!(["approve", "deny"])
        );
        assert_eq!(schema["additionalProperties"], json!(false));
    }
}
