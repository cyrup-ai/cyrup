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
//! | it is cancelled DURING the turn | `cancelled` | `Watchdog permission decision was cancelled.` |
//! | the model called no tool | `malformed` | `…returned no decision.` |
//! | the turn exceeded `agentEndTimeoutMs` | `timeout` | `Watchdog permission decision timed out.` |
//! | anything else threw | `error` | `…failed closed: <detail>` |
//! | the model answered `deny` | `deny` | the model's own reason |
//!
//! **The last three rows moved to v0.68.0 with UW-5, deliberately.** `git diff v0.43.0 v0.68.0 --
//! src/watchdog/permission-arbiter.ts` restructured the `Promise.race` three ways: the timeout arm
//! now RESOLVES `finish(false, "…timed out.", "timeout")` (`:136 @v0.68.0`) instead of rejecting
//! into the catch, so its message no longer carries the `failed closed:` prefix; a mid-turn cancel
//! now resolves `finish(false, "…was cancelled.", "cancelled")` (`:138 @v0.68.0`) instead of only
//! calling `agent.abort()` and surfacing as `error`; and a `completed` latch (`:49,:52-53`) makes
//! `finish` idempotent so a losing race arm cannot append a SECOND `permission.decision` record.
//! All three still deny (`approved: false` on every arm), so the security property is unchanged and
//! the file now matches the tag its turn is ported from. The generic `failed closed:` catch arm
//! (`:143-145 @v0.68.0`) is untouched and still carries the prefix.
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
/// `pub(crate)` for [`crate::exec::model_exclusions::sanitize_model_exclusion_diagnostic`], which is
/// the port of upstream's `sanitizeModelExclusionDiagnostic` and calls the SAME `redactSecretValues`
/// this is (`model-fallback.ts:301`). Widening beats a second redactor: this one's behaviour —
/// including the case-insensitivity that is a security property, not a formatting nicety — is
/// pinned by tests in this module, and a second implementation would be a second thing to get
/// wrong on a path that reaches both a log and the operator's terminal.
pub(crate) fn redact_secret_values(value: &str) -> String {
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
// The policy the arbiter answers FOR (`runs/shared/permissions.ts:4-65` @v0.64.0)
// =================================================================================================
//
// **Version pin, and why these three citations carry one while the rest of this file does not.**
// Upstream transported the policy to the child through two env vars and decoded it on the far side.
// That transport is GONE at v0.68.0 — `PERMISSION_POLICY_ENV`, `PERMISSION_AUDIT_PATH_ENV` and
// `decodePermissionRules` do not exist anywhere in the tree any more:
//
// ```text
// $ git -C tmp/pi-subagents grep -n 'PERMISSION_POLICY_ENV\|PERMISSION_AUDIT_PATH_ENV\|decodePermissionRules' v0.68.0
// $ (no output)
// ```
//
// They were live and unchanged from `v0.45.2` through `v0.64.0` and were dropped in the
// `v0.64.0..v0.65.0` window. The line numbers below are therefore pinned to `@v0.64.0`, which is
// the last revision where they resolve, and they are NOT `[CYRUP-DELTA]`s: cyrup ported real
// upstream symbols, and upstream has since retired the mechanism. What survives at v0.68.0 in that
// file is `INTERNAL_TOOLS` — now at `:8`, the line `PERMISSION_POLICY_ENV` used to hold, which is
// exactly the coincidence that makes an unpinned `permissions.ts:8` read as correct when it is not.
// Re-porting the removal is its own decision and is not made here.

/// `PERMISSION_POLICY_ENV` (`permissions.ts:8` @v0.64.0; removed upstream by v0.65.0 — see the
/// version pin above), under cyrup's `CYRUP_SUBAGENT_*` spelling — the same rename every other
/// member of that family carries (`CYRUP_SUBAGENT_TOOL_BUDGET`, `CYRUP_SUBAGENT_STEER_INBOX`, …).
pub const PERMISSION_POLICY_ENV: &str = "CYRUP_SUBAGENT_PERMISSION_POLICY";

/// `PERMISSION_AUDIT_PATH_ENV` (`permissions.ts:9` @v0.64.0; removed upstream by v0.65.0).
pub const PERMISSION_AUDIT_PATH_ENV: &str = "CYRUP_SUBAGENT_PERMISSION_AUDIT_PATH";

/// `INTERNAL_TOOLS` (`permissions.ts:8` @v0.68.0) — the child's OWN coordination surface, which a
/// permission policy may not gate at all.
///
/// What the set is FOR: a parent writes rules about the tools a child uses to do WORK, not about
/// the tools it uses to report back. Gating one of these strands the child mid-run — it cannot
/// contact its supervisor, cannot answer on the intercom, cannot block on its own background work,
/// cannot emit its structured result — with no way for anyone to unstick it. So the set is
/// enforced twice: [`validate_permission_rules`] refuses to RECORD a rule for a member
/// (`permissions.ts:26`), and [`permission_decision`] short-circuits to
/// [`PermissionRuleDecision::Allow`] for one anyway (`permissions.ts:49`), because rules can also
/// arrive from a parent running a different version.
///
/// The wait entry is DERIVED from the registered tool name
/// (`crate::extension::wait_tool::WAIT_TOOL_NAME`) rather than repeated as a literal here. That is
/// this port's one structural divergence from upstream's inline `new Set([...])`, and it is the
/// point: the set is only protective if it names the tool that is actually registered. This port
/// previously carried the literal `"subagent_wait"`, a name cyrup has never registered, so the set
/// ungated a phantom while the real wait tool stayed gateable and a `{"wait": "deny"}` rule could
/// strand a child. A second literal is exactly how that happens.
const INTERNAL_TOOLS: [&str; 4] = [
    "contact_supervisor",
    "intercom",
    crate::extension::wait_tool::WAIT_TOOL_NAME,
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

    /// `DECISIONS.has(decision)` (`permissions.ts:9` @v0.68.0).
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

/// `validatePermissionRules(value, label)` (`permissions.ts:19-31` @v0.68.0).
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

/// `decodePermissionRules(encoded)` (`permissions.ts:62-65` @v0.64.0 — the body is
/// `if (!encoded?.trim()) return undefined; return validatePermissionRules(JSON.parse(encoded),
/// PERMISSION_POLICY_ENV);`; removed upstream by v0.65.0, see the version pin on
/// [`PERMISSION_POLICY_ENV`]) — the child's side of the policy: a blank value is the same as
/// unset.
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

/// `permissionDecision(rules, toolName)` (`permissions.ts:48-51` @v0.68.0).
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
    /// `request.ctx` as far as this module needs it (`permission-arbiter.ts:96 @v0.68.0`, which
    /// passes the whole `ExtensionContext` to `resolveWatchdogReviewModel`): the LIVE session model
    /// and reasoning level, snapshotted by the caller at the moment the `ask` fires.
    ///
    /// `None` is upstream's context-less call and leaves the arbiter with only
    /// `subagents.watchdog.children.model` to resolve from — which is unset in the DEFAULT config,
    /// so the arbiter then fails closed with "the current Pi session model is unavailable"
    /// ([`super::review::resolve_watchdog_review_model`]). Every ask in an ordinary session takes
    /// the inherited-model arm, so a production caller that leaves this `None` has wired an arbiter
    /// that can never approve.
    pub session: Option<super::review::WatchdogSessionContext>,
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

/// `const tool: AgentTool<typeof PermissionDecisionParams, { recorded: boolean }>`
/// (`permission-arbiter.ts:78-88 @v0.68.0`) — the complete descriptor of the ONE tool an arbiter
/// turn may call, and the only path from a model call to a decision.
///
/// It exists as a value, and not as loose helpers, for exactly the reason
/// [`super::review::WatchdogWarnTool`] does: before it the seam carried only
/// [`permission_decision_parameters_schema`], so a bound [`WatchdogPermissionAgent`] had no way to
/// learn the tool's NAME (`:79`), its LABEL (`:80`), its DESCRIPTION (`:81` — prompt text the model
/// reads to decide what the tool is for) or its `executionMode` (`:83`), and had to re-derive the
/// first-call-wins latch (`:85`) from prose.
///
/// [CYRUP-DELTA] this file is otherwise a v0.43.0 port; this type is pinned to **v0.68.0**, where
/// the tool object is unchanged from v0.43.0 except for its surrounding `run()`. Every citation on
/// it says `@v0.68.0` so the two tags never silently mix.
#[derive(Debug, Default)]
pub struct WatchdogPermissionDecisionTool {
    decision: std::sync::Mutex<Option<WatchdogPermissionDecision>>,
}

impl WatchdogPermissionDecisionTool {
    /// `name: "watchdog_permission_decision"` (`:79 @v0.68.0`).
    pub const NAME: &'static str = "watchdog_permission_decision";
    /// `label: "Watchdog permission decision"` (`:80 @v0.68.0`).
    pub const LABEL: &'static str = "Watchdog permission decision";
    /// `description` (`:81 @v0.68.0`).
    pub const DESCRIPTION: &'static str =
        "Approve or deny this exact child tool call. Call exactly once.";
    /// `executionMode: "sequential"` (`:83 @v0.68.0`) — distinct from the agent-wide
    /// `toolExecution: "sequential"` (`:127`), and load-bearing for the same reason the latch is:
    /// two calls in one assistant message must reach [`Self::record`] in order.
    pub const SEQUENTIAL: bool = true;
    /// `content: [{ type: "text", text: "Permission decision recorded." }]` (`:86 @v0.68.0`) —
    /// returned for EVERY well-formed call, including the ignored later ones.
    pub const RESULT_TEXT: &'static str = "Permission decision recorded.";

    /// A fresh, undecided latch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `parameters: PermissionDecisionParams` (`:82 @v0.68.0`).
    #[must_use]
    pub fn parameters(&self) -> Value {
        permission_decision_parameters_schema()
    }

    /// `execute(_toolCallId, params) { if (!decision) decision = params; … }` (`:84-87 @v0.68.0`)
    /// — the FIRST well-formed call wins and every later one is ignored but still answered
    /// [`Self::RESULT_TEXT`].
    ///
    /// # Errors
    ///
    /// The per-field message for arguments the schema would have rejected. Upstream's typebox
    /// validation happens in the harness before `execute` and surfaces to the model as a tool
    /// error; this reproduces that so a malformed decision becomes a correction the model can act
    /// on rather than a silent no-decision.
    pub fn record(&self, params: &Value) -> Result<(), String> {
        let object = params
            .as_object()
            .ok_or_else(|| format!("{} requires an object argument.", Self::NAME))?;
        let decision = object
            .get("decision")
            .and_then(Value::as_str)
            .filter(|value| *value == "approve" || *value == "deny")
            .ok_or_else(|| format!("{}.decision must be approve or deny.", Self::NAME))?;
        let reason = object
            .get("reason")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{}.reason must be a string.", Self::NAME))?;
        let mut slot = self
            .decision
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(WatchdogPermissionDecision {
                decision: decision.to_string(),
                reason: reason.to_string(),
            });
        }
        Ok(())
    }

    /// `if (!decision) …` (`:130 @v0.68.0`) — what the turn recorded, or `None` for the model that
    /// called no tool, which denies as `malformed`.
    #[must_use]
    pub fn decision(&self) -> Option<WatchdogPermissionDecision> {
        self.decision
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
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
    /// `request.ctx`'s live model + reasoning level (`permission-arbiter.ts:96 @v0.68.0`), carried
    /// from [`WatchdogPermissionRequest::session`]. See that field for why a `None` here is the
    /// difference between an arbiter that can approve and one that cannot.
    pub session: Option<&'a super::review::WatchdogSessionContext>,
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

/// `createWatchdogPermissionArbiter`'s own `run()` body (`permission-arbiter.ts:94-132 @v0.68.0`)
/// — the REAL arbiter: resolve the arbiter's model against the child's config and the live session,
/// run one nested [`cyrup_agent::Agent`] turn whose only tool is
/// [`WatchdogPermissionDecisionTool`], and report what it recorded.
///
/// Bound in production at `crate::prompt_runtime`'s `with_permission_gate` call, the single
/// production arbiter site.
///
/// [CYRUP-DELTA] this file is a v0.43.0 port; THIS type is pinned to **v0.68.0** and says so on
/// every citation. Nothing around it changed — the audit pair, the timeout, the fail-closed
/// mapping and the cancel checks all still live in [`request_watchdog_permission`], which is
/// upstream's `:44-93` + `:134-152`.
pub struct ModelTurnPermissionAgent {
    registry: std::sync::Arc<dyn super::model_selection::WatchdogModelRegistry>,
    auth_resolver: std::sync::Arc<dyn super::review::WatchdogReviewAuthResolver>,
    turn: super::agent_turn::WatchdogAgentTurn,
}

impl std::fmt::Debug for ModelTurnPermissionAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelTurnPermissionAgent")
            .finish_non_exhaustive()
    }
}

impl ModelTurnPermissionAgent {
    /// Bind the arbiter to a model registry, an auth resolver and the late-resolved capability
    /// backend the nested turn streams through.
    #[must_use]
    pub fn new(
        registry: std::sync::Arc<dyn super::model_selection::WatchdogModelRegistry>,
        auth_resolver: std::sync::Arc<dyn super::review::WatchdogReviewAuthResolver>,
        services: super::register_main::WatchdogServicesFn,
    ) -> Self {
        Self {
            registry,
            auth_resolver,
            turn: super::agent_turn::WatchdogAgentTurn::new(services),
        }
    }

    /// `beforeToolCall: async ({ toolCall }) => toolCall.name === tool.name ? undefined :
    /// { block: true, reason: … }` (`permission-arbiter.ts:126 @v0.68.0`) — the arbiter's
    /// execution-time tool policy, as an `Arc` so the value production installs on the nested
    /// agent is a value a caller can hold and exercise.
    ///
    /// This accessor is the ONLY source of the policy [`Self::turn_request`] installs, so a test
    /// over it is a test over the closure the running turn is built with rather than over a
    /// re-declared copy.
    #[must_use]
    #[allow(clippy::type_complexity)]
    pub fn tool_call_block_reason(
        &self,
    ) -> std::sync::Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
        std::sync::Arc::new(|name: &str| {
            (name != WatchdogPermissionDecisionTool::NAME)
                .then(|| format!("Permission arbiter tool '{name}' is not allowed."))
        })
    }

    /// The exact [`super::agent_turn::WatchdogTurnRequest`] [`WatchdogPermissionAgent::decide`]
    /// hands the nested agent once the model is resolved: the ONE decision tool
    /// (`permission-arbiter.ts:121 @v0.68.0`) bound to `latch`, and the execution-time policy
    /// (`:126`).
    ///
    /// Extracted from `decide` so the construction is reachable without a provider, a model or a
    /// stream: `decide` builds nothing of its own, so a test over this function is a test over
    /// what production installs.
    pub(crate) fn turn_request<'a>(
        &self,
        system_prompt: String,
        prompt: String,
        selection: &'a super::review::WatchdogReviewModelSelection,
        latch: &std::sync::Arc<WatchdogPermissionDecisionTool>,
        cancel: CancelToken,
    ) -> super::agent_turn::WatchdogTurnRequest<'a> {
        super::agent_turn::WatchdogTurnRequest {
            system_prompt,
            prompt,
            selection,
            // `tools: [tool]` (`:121 @v0.68.0`) — exactly one, and it writes the latch this call
            // reads back.
            tools: vec![std::sync::Arc::new(
                super::agent_turn::WatchdogPermissionDecisionAgentTool::new(std::sync::Arc::clone(
                    latch,
                )),
            )],
            block_reason: self.tool_call_block_reason(),
            cancel,
        }
    }
}

#[async_trait]
impl WatchdogPermissionAgent for ModelTurnPermissionAgent {
    async fn decide(
        &self,
        turn: WatchdogPermissionTurn<'_>,
    ) -> Result<Option<WatchdogPermissionDecision>, String> {
        use std::sync::Arc;

        // `const config = childResolvedConfig(childConfig); const selection = await
        // resolveWatchdogReviewModel(request.ctx, config)` (`:95-96 @v0.68.0`). `request.ctx` is the
        // LIVE session — without it the default config has no model at all and every ask denies.
        let config = super::register_child::child_resolved_config(turn.config);
        let ctx = super::model_selection::WatchdogModelContext {
            registry: self.registry.as_ref(),
            current_model: turn.session.and_then(|session| session.model.clone()),
        };
        let selection = super::review::resolve_watchdog_review_model(
            &ctx,
            &config,
            self.auth_resolver.as_ref(),
            turn.session
                .and_then(|session| session.thinking_level.as_deref()),
        )?;
        let decision_tool = Arc::new(WatchdogPermissionDecisionTool::new());
        let messages = self
            .turn
            .run(self.turn_request(
                turn.system_prompt,
                turn.prompt,
                &selection,
                &decision_tool,
                turn.cancel.clone(),
            ))
            .await?;
        // A transport failure does not reject `prompt` in cyrup — it arrives as a terminal `error`
        // assistant message. Surfacing it as `Err` is what keeps the fail-closed table honest: the
        // decision is recorded as `error` rather than as `malformed`, which would claim the model
        // answered badly when it never answered at all.
        if super::review::final_stop_reason(&messages) == super::runtime::ReviewStopReason::Error {
            let detail = messages
                .iter()
                .rev()
                .find_map(|message| {
                    message
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "the arbiter model turn failed".to_string());
            return Err(detail);
        }
        Ok(decision_tool.decision())
    }
}

/// The no-agent stand-in: reaches no decision, so the arbiter denies as `malformed`.
///
/// **Test fixture only, as of UW-5.** Production binds [`ModelTurnPermissionAgent`]; this remains
/// because it is the honest double for "the model answered nothing", which is the arm the
/// fail-closed table's `malformed` row describes and which ~10 tests in this module (and
/// `prompt_runtime`'s `a_child_with_no_policy_installs_no_gate`) drive. It is NOT
/// `#[allow(dead_code)]`-ed and it is NOT bound anywhere in production — a `pub` type whose doc
/// claimed to be the deployment default while production bound something else would be exactly the
/// doc-asserts-wiring defect this task set out to remove.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDecisionPermissionAgent;

#[cfg(test)]
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

/// `finish` (`permission-arbiter.ts:50-65 @v0.68.0`): write the decision record, then return it.
///
/// `completed` is upstream's idempotence latch (`:49`, `:52-53`): the FIRST call writes the audit
/// record and every later one returns the same shape without appending a second. cyrup's
/// `tokio::select!` has exactly one winner so it cannot double-fire today, but the latch is what
/// makes that a property of this function rather than of its one caller's control flow.
fn finish(
    request: &WatchdogPermissionRequest,
    completed: &std::cell::Cell<bool>,
    created_at: i64,
    approved: bool,
    reason: &str,
    decision: &str,
) -> WatchdogPermissionResult {
    let reason = concise_reason(reason);
    if completed.replace(true) {
        return WatchdogPermissionResult {
            approved,
            reason,
            source: "watchdog",
        };
    }
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
    // `let completed = false` (`:49 @v0.68.0`) — see [`finish`].
    let completed = std::cell::Cell::new(false);
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
                &completed,
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
            &completed,
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
            &completed,
            created_at,
            false,
            "Watchdog permission decision was cancelled.",
            "cancelled",
        );
    }

    let turn = WatchdogPermissionTurn {
        config: &child_config,
        // `resolveWatchdogReviewModel(request.ctx, config)` (`:96 @v0.68.0`) — the live session,
        // snapshotted by the caller when the `ask` fired.
        session: request.session.as_ref(),
        system_prompt: permission_arbiter_system_prompt(),
        prompt: permission_arbiter_prompt(&request.tool_name, &preview),
        decision_tool_schema: permission_decision_parameters_schema(),
        cancel: cancel.clone(),
    };
    // `Promise.race([run(), timeout, abort])` (`:134-142 @v0.68.0`). The `agentEndTimeoutMs` bound
    // stays OUTSIDE the agent (upstream's `setTimeout`, `:136`) and cancels the token so the nested
    // run actually stops rather than being abandoned mid-stream.
    let outcome = tokio::select! {
        biased;
        // `:137-141 @v0.68.0` — the abort arm RESOLVES a `cancelled` decision. At v0.43.0 it only
        // called `agent.abort()` and the rejected prompt surfaced through the catch as `error`;
        // both deny, and this is the answer at the tag the turn is ported from.
        () = cancel.cancelled() => ArbiterOutcome::Cancelled,
        raced = tokio::time::timeout(
            Duration::from_millis(child_config.agent_end_timeout_ms),
            agent.decide(turn),
        ) => match raced {
            Ok(Ok(decision)) => ArbiterOutcome::Decided(decision),
            Ok(Err(reason)) => ArbiterOutcome::Failed(reason),
            Err(_) => {
                // `agent?.abort()` before resolving (`:136 @v0.68.0`).
                cancel.cancel();
                ArbiterOutcome::TimedOut
            }
        },
    };

    match outcome {
        // `:131-132 @v0.68.0`.
        ArbiterOutcome::Decided(Some(decision)) => {
            let approved = decision.decision == "approve";
            finish(
                request,
                &completed,
                created_at,
                approved,
                &decision.reason,
                &decision.decision,
            )
        }
        // `if (!decision) return finish(false, "…returned no decision.", "malformed")`
        // (`:130 @v0.68.0`).
        ArbiterOutcome::Decided(None) => finish(
            request,
            &completed,
            created_at,
            false,
            "Watchdog permission arbiter returned no decision.",
            "malformed",
        ),
        // `:136 @v0.68.0` — RESOLVED, so no `failed closed:` prefix.
        ArbiterOutcome::TimedOut => finish(
            request,
            &completed,
            created_at,
            false,
            "Watchdog permission decision timed out.",
            "timeout",
        ),
        // `:138 @v0.68.0`.
        ArbiterOutcome::Cancelled => finish(
            request,
            &completed,
            created_at,
            false,
            "Watchdog permission decision was cancelled.",
            "cancelled",
        ),
        // The generic `catch` (`:143-145 @v0.68.0`), which still carries the prefix.
        ArbiterOutcome::Failed(reason) => {
            let decision = if reason.contains("timed out") {
                "timeout"
            } else {
                "error"
            };
            finish(
                request,
                &completed,
                created_at,
                false,
                &format!("Watchdog permission arbiter failed closed: {reason}"),
                decision,
            )
        }
    }
}

/// Which arm of the `Promise.race` (`permission-arbiter.ts:134-142 @v0.68.0`) won.
///
/// A named enum rather than a `Result<Option<_>, String>` because v0.68.0 has four distinct losing
/// arms with three different decision strings, and collapsing them onto one `Err` is exactly what
/// made the v0.43.0 shape report a mid-turn cancel as `error`.
enum ArbiterOutcome {
    /// `run()` returned — with or without a recorded decision.
    Decided(Option<WatchdogPermissionDecision>),
    /// The `agentEndTimeoutMs` arm.
    TimedOut,
    /// The abort arm.
    Cancelled,
    /// The agent itself reported a failure, which lands in upstream's `catch`.
    Failed(String),
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
    use crate::extension::wait_tool::WAIT_TOOL_NAME;
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
            session: None,
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

    /// A registry that knows no model at all — the "no model bound" configuration, which is what
    /// every unconfigured child has until a session model reaches it.
    struct EmptyRegistry;

    impl crate::watchdog::model_selection::WatchdogModelRegistry for EmptyRegistry {
        fn available(&self) -> Vec<crate::watchdog::model_selection::WatchdogModelInfo> {
            Vec::new()
        }
        fn find(
            &self,
            _provider: &str,
            _id: &str,
        ) -> Option<crate::watchdog::model_selection::WatchdogModelInfo> {
            None
        }
        fn has_configured_auth(
            &self,
            _model: &crate::watchdog::model_selection::WatchdogModelInfo,
        ) -> bool {
            false
        }
    }

    fn real_arbiter() -> ModelTurnPermissionAgent {
        ModelTurnPermissionAgent::new(
            std::sync::Arc::new(EmptyRegistry),
            std::sync::Arc::new(crate::watchdog::review::AmbientReviewAuth),
            std::sync::Arc::new(|| None),
        )
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

    /// UPDATED DELIBERATELY at v0.68.0 (UW-5). The timeout arm now RESOLVES
    /// `finish(false, "Watchdog permission decision timed out.", "timeout")`
    /// (`permission-arbiter.ts:136 @v0.68.0`) instead of rejecting into the catch, so the message no
    /// longer carries the `failed closed:` prefix v0.43.0 gave it. The property this test exists for
    /// — a hung turn DENIES, on the CONFIGURED bound — is unchanged and is asserted first.
    #[tokio::test]
    async fn a_hanging_agent_denies_on_the_configured_timeout() {
        let mut config = default_watchdog_config();
        config.enabled = true;
        config.children.enabled = true;
        config.agent_end_timeout_ms = 20;
        let raw = encode_child_watchdog_config(
            resolve_child_watchdog_config(&config, None, None, None).as_ref(),
        );
        let dir = TempDir::new().unwrap();
        let audit = dir.path().join("audit.jsonl");
        let result =
            request_watchdog_permission(&request(raw, Some(audit.clone())), &HangingAgent).await;
        assert!(!result.approved);
        assert_eq!(result.reason, "Watchdog permission decision timed out.");
        let lines: Vec<Value> = std::fs::read_to_string(&audit)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2, "exactly ONE decision record (the latch)");
        assert_eq!(lines[1]["decision"], json!("timeout"));
        assert_eq!(lines[1]["approved"], json!(false));
    }

    /// `:137-141 @v0.68.0` — a cancel that lands DURING the turn resolves `cancelled`, where
    /// v0.43.0 only aborted the agent and the rejection surfaced through the catch as `error`. Both
    /// deny; this pins WHICH answer, because the fail-closed table in the module doc names it.
    #[tokio::test]
    async fn a_mid_turn_cancel_denies_as_cancelled_not_as_error() {
        let cancel = CancelToken::new();
        let mut req = request(Some(enabled_child_config()), None);
        req.cancel = Some(cancel.clone());
        let spawned = tokio::spawn(async move { cancel.cancel() });
        let result = request_watchdog_permission(&req, &HangingAgent).await;
        spawned.await.unwrap();
        assert!(!result.approved);
        assert_eq!(result.reason, "Watchdog permission decision was cancelled.");
    }

    /// **The production construction, pinned.** [`ModelTurnPermissionAgent::decide`] builds
    /// nothing of its own once the model is resolved — the tool list and the execution-time policy
    /// both come out of [`ModelTurnPermissionAgent::turn_request`] — so this drives the EXACT
    /// [`crate::watchdog::agent_turn::WatchdogTurnRequest`] a real arbiter turn is built with,
    /// rather than a closure the test itself declared.
    ///
    /// `tools: [tool]` + `beforeToolCall` (`permission-arbiter.ts:121,126 @v0.68.0`): exactly one
    /// tool, and every other name refused at execution time. Gutting
    /// [`ModelTurnPermissionAgent::tool_call_block_reason`] to `Arc::new(|_| None)` — which would
    /// let an arbiter turn call anything the harness ever supplies — fails the refusal assertions
    /// below.
    #[test]
    fn the_production_arbiter_turn_installs_one_tool_and_refuses_every_other_name() {
        let agent = real_arbiter();
        let selection = crate::watchdog::review::WatchdogReviewModelSelection {
            model: crate::watchdog::model_selection::WatchdogModelInfo::new("anthropic", "m"),
            thinking_level: "off".to_string(),
            auth: crate::watchdog::review::WatchdogReviewAuth::default(),
            explicit: true,
        };
        let latch = std::sync::Arc::new(WatchdogPermissionDecisionTool::new());
        let request = agent.turn_request(
            permission_arbiter_system_prompt(),
            permission_arbiter_prompt("write", "{}"),
            &selection,
            &latch,
            CancelToken::new(),
        );

        let offered: Vec<&str> = request.tools.iter().map(|tool| tool.name()).collect();
        assert_eq!(
            offered,
            vec![WatchdogPermissionDecisionTool::NAME],
            "the arbiter turn gets exactly one tool"
        );

        // The execution-time layer, read off the value production installed on the turn.
        assert_eq!(
            (request.block_reason)(WatchdogPermissionDecisionTool::NAME),
            None
        );
        for forbidden in ["read", "write", "bash", "watchdog_warn", "subagent"] {
            assert_eq!(
                (request.block_reason)(forbidden).as_deref(),
                Some(format!("Permission arbiter tool '{forbidden}' is not allowed.").as_str()),
                "the arbiter must refuse '{forbidden}' at execution time"
            );
        }
    }

    /// **Fail-closed, through the REAL agent: no model bound.** An armed child whose registry knows
    /// no model and whose session carries none cannot resolve an arbiter model, so
    /// [`ModelTurnPermissionAgent::decide`] returns `Err` before any turn — and the ask DENIES,
    /// recorded as `error` with the `failed closed:` prefix (`:143-145 @v0.68.0`).
    ///
    /// This is the arm an embedder hits first (the default config sets no model at all), and it is
    /// the one where a fail-OPEN would be silent: nothing failed loudly, the model simply was not
    /// there.
    #[tokio::test]
    async fn the_real_arbiter_denies_when_no_model_can_be_resolved() {
        let dir = TempDir::new().unwrap();
        let audit = dir.path().join("audit.jsonl");
        let result = request_watchdog_permission(
            &request(Some(enabled_child_config()), Some(audit.clone())),
            &real_arbiter(),
        )
        .await;
        assert!(
            !result.approved,
            "an arbiter with no model must NEVER approve: {result:?}"
        );
        assert!(
            result
                .reason
                .starts_with("Watchdog permission arbiter failed closed: "),
            "{}",
            result.reason
        );
        let lines: Vec<Value> = std::fs::read_to_string(&audit)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2, "both audit records, always: {lines:#?}");
        assert_eq!(lines[1]["approved"], json!(false));
        assert_eq!(lines[1]["decision"], json!("error"));
    }

    /// **Fail-closed, through the REAL agent: cancelled.** A token already cancelled when the ask
    /// fires denies as `cancelled` without reaching a model (`:137-141 @v0.68.0`) — asserted here
    /// against the production agent rather than against a double, because the fail-closed table is
    /// a property of the pair, not of the stand-in.
    #[tokio::test]
    async fn the_real_arbiter_denies_a_cancelled_request() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut req = request(Some(enabled_child_config()), None);
        req.cancel = Some(cancel);
        let result = request_watchdog_permission(&req, &real_arbiter()).await;
        assert!(!result.approved, "a cancelled ask must never approve");
        assert_eq!(result.reason, "Watchdog permission decision was cancelled.");
    }

    /// **Fail-closed, through the REAL agent: the child watchdog is disabled.** No config at all is
    /// the shipped default, and it denies as `unavailable` before any turn (`:74 @v0.68.0`).
    #[tokio::test]
    async fn the_real_arbiter_denies_when_the_child_watchdog_is_disabled() {
        let result = request_watchdog_permission(&request(None, None), &real_arbiter()).await;
        assert!(!result.approved);
        assert_eq!(
            result.reason,
            "Watchdog permission arbiter is unavailable because the child watchdog is disabled."
        );
    }

    /// `if (completed) return …` (`:52-53 @v0.68.0`) — the idempotence latch, asserted on the
    /// function rather than on its caller's control flow.
    #[test]
    fn finish_appends_exactly_one_decision_record() {
        let dir = TempDir::new().unwrap();
        let audit = dir.path().join("audit.jsonl");
        let req = request(None, Some(audit.clone()));
        let completed = std::cell::Cell::new(false);
        let first = finish(&req, &completed, 1, false, "first", "timeout");
        let second = finish(&req, &completed, 1, false, "second", "cancelled");
        assert_eq!(first.reason, "first");
        assert_eq!(
            second.reason, "second",
            "the caller still sees its own shape"
        );
        let lines: Vec<&str> = std::fs::read_to_string(&audit)
            .unwrap()
            .lines()
            .map(str::to_string)
            .map(|s| Box::leak(s.into_boxed_str()) as &str)
            .collect();
        assert_eq!(lines.len(), 1, "only the FIRST finish writes");
        let record: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(record["decision"], json!("timeout"));
    }

    /// GAP-3 — the first well-formed call wins and later ones are ignored but still answered
    /// (`:85-86 @v0.68.0`); a malformed one is a tool ERROR the model can correct.
    #[test]
    fn the_decision_tool_latches_the_first_well_formed_call() {
        let tool = WatchdogPermissionDecisionTool::new();
        assert!(tool.decision().is_none());
        assert!(
            tool.record(&json!({ "decision": "maybe", "reason": "x" }))
                .is_err()
        );
        assert!(tool.record(&json!({ "decision": "approve" })).is_err());
        assert!(tool.decision().is_none(), "a rejected call records nothing");
        tool.record(&json!({ "decision": "approve", "reason": "first" }))
            .unwrap();
        tool.record(&json!({ "decision": "deny", "reason": "second" }))
            .unwrap();
        let decision = tool.decision().unwrap();
        assert_eq!(decision.decision, "approve");
        assert_eq!(decision.reason, "first");
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
        // could still ship it, which is why the decision function checks again. The wait tool is
        // named through the const the registration uses, so this loop covers whatever
        // `INTERNAL_TOOLS` actually holds rather than a copy of it.
        let mut rules = PermissionRules::new();
        for tool in [
            "bash",
            "contact_supervisor",
            "intercom",
            WAIT_TOOL_NAME,
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

    /// SLASH_SURFACE §I.2 — the internal-tools ungate must protect the tool that is actually
    /// REGISTERED. pi `permissions.ts:49` @v0.68.0:
    /// `if (toolName === "bash" || INTERNAL_TOOLS.has(toolName)) return "allow";`
    ///
    /// This is the assert that makes the `bg_wait` rename observable rather than cosmetic. Before
    /// it, `INTERNAL_TOOLS` held the literal `"subagent_wait"` — a name this crate has never
    /// registered — so the set ungated nothing while the real wait tool stayed gateable, and a
    /// parent shipping a `{"bg_wait": "deny"}` rule could strand a child that had already launched
    /// background work: it would be refused the only tool that can collect it.
    ///
    /// **The mutation that fails this test:** replace
    /// `crate::extension::wait_tool::WAIT_TOOL_NAME` in `INTERNAL_TOOLS` with any literal that is
    /// not the registered name (`"subagent_wait"`, `"wait"`, a typo). The deny rule below is then
    /// honoured and the assert reads `Deny` instead of `Allow`.
    #[test]
    fn a_deny_rule_on_the_registered_wait_tool_cannot_strand_a_child() {
        let mut rules = PermissionRules::new();
        rules.insert(WAIT_TOOL_NAME.to_string(), PermissionRuleDecision::Deny);
        assert_eq!(
            permission_decision(Some(&rules), WAIT_TOOL_NAME),
            PermissionRuleDecision::Allow,
            "a parent policy must not be able to gate `{WAIT_TOOL_NAME}`"
        );
        // The same map still gates an ordinary tool, so the ungate is targeted rather than a
        // blanket "deny rules are ignored".
        rules.insert("write".to_string(), PermissionRuleDecision::Deny);
        assert_eq!(
            permission_decision(Some(&rules), "write"),
            PermissionRuleDecision::Deny
        );
    }

    /// SLASH_SURFACE §I.2, the validation half — a policy may not even RECORD a rule keyed on the
    /// registered wait tool. pi `permissions.ts:26` @v0.68.0:
    /// ``if (INTERNAL_TOOLS.has(tool)) throw new Error(`${label}.${tool} is reserved for child
    /// coordination and cannot be gated.`);``
    ///
    /// The sentence is asserted byte-for-byte because it is upstream's, unchanged: it carries no
    /// product name, so there is nothing to rebrand and no licence to reword it.
    ///
    /// **The mutation that fails this test:** the same one as
    /// `a_deny_rule_on_the_registered_wait_tool_cannot_strand_a_child` — a literal in
    /// `INTERNAL_TOOLS` that is not the registered name. `validate_permission_rules` then ACCEPTS
    /// the rule and returns `Ok`, so `unwrap_err` panics. Dropping the `INTERNAL_TOOLS` check from
    /// `validate_permission_rules` altogether fails it the same way.
    #[test]
    fn a_rule_keyed_on_the_registered_wait_tool_is_refused_at_validation() {
        let mut object = serde_json::Map::new();
        object.insert(WAIT_TOOL_NAME.to_string(), Value::from("deny"));
        let error = validate_permission_rules(Some(&Value::Object(object)), "config.permissions")
            .expect_err("a rule on the wait tool must be refused");
        let expected = format!(
            "config.permissions.{WAIT_TOOL_NAME} is reserved for child coordination and cannot \
             be gated."
        );
        assert_eq!(error, expected);
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
