//! The audit / debug JSONL trail: the two stream writers, the decision record every gate branch
//! funnels into, and the on-the-wire spellings of its `state` / `source` fields.

use serde_json::{Value, json};

use crate::ask::PermissionDecisionState;
use crate::dedup::DedupDetails;
use crate::types::CheckSource;

use super::PermissionSystemExtension;

/// The on-the-wire `state` string pi writes into a `resolution` field — the SAME strings the
/// `serde(rename_all = "snake_case")` derive on [`PermissionDecisionState`] produces
/// (`ask.rs:27-42`, pi `permission-dialog.ts:1`), spelled out here so the audit trail cannot drift
/// from the derive silently.
pub(super) fn decision_state_str(state: PermissionDecisionState) -> &'static str {
    match state {
        PermissionDecisionState::Approved => "approved",
        PermissionDecisionState::Denied => "denied",
        PermissionDecisionState::DeniedWithReason => "denied_with_reason",
        PermissionDecisionState::Once => "once",
        PermissionDecisionState::Always => "always",
        PermissionDecisionState::Reject => "reject",
    }
}

pub(super) fn source_str(s: CheckSource) -> &'static str {
    match s {
        CheckSource::Tool => "tool",
        CheckSource::Bash => "bash",
        CheckSource::Mcp => "mcp",
        CheckSource::Skill => "skill",
        CheckSource::Special => "special",
        CheckSource::Default => "default",
    }
}

impl PermissionSystemExtension {
    /// pi `writeDebugEntry` (`index.ts:171-176`): the diagnostic stream, with the logger's own
    /// failure funnelled into the dedup-once warning reporter.
    pub(super) fn write_debug_entry(&self, event: &str, details: &Value) {
        self.logger.debug(event, details);
    }

    /// pi `writeReviewEntry` (v0.8.0 `index.ts:200-202`, via `writeLogEntry` `:183-194`): the
    /// SECURITY-relevant decision stream — the
    /// "why was this blocked / who approved this" trail. Same warning funnel.
    pub(super) fn write_review_entry(&self, event: &str, details: &Value) {
        self.logger.review(event, details);
    }

    /// pi `reviewPermissionDecision` (`index.ts:1767-1793`): the ONE shaped `review` record every
    /// decision-point entry is built from — the prompt and denial reason accompanied by their
    /// `createSensitiveLogMetadata` digests, plus the resolution / persistence / scope fields.
    ///
    /// `details` is the same [`DedupDetails`] the dedup fingerprint is built from, which already
    /// mirrors pi's `PermissionPromptDetails` field for field (`dedup.rs:36-50`).
    pub(super) fn review_permission_decision(
        &self,
        event: &str,
        details: &DedupDetails,
        tail: Value,
    ) {
        let mut record = json!({
            "requestId": details.request_id,
            "source": details.source,
            "agentName": details.agent_name,
            "prompt": details.message,
            "promptMetadata": crate::logging::sensitive_log_metadata(Some(&details.message)),
            "toolCallId": details.tool_call_id,
            "toolName": details.tool_name,
            "skillName": details.skill_name,
            "path": details.path,
            "command": details.command,
            "commandMetadata": crate::logging::sensitive_log_metadata(details.command.as_deref()),
            "target": details.target,
            "toolInput": details.tool_input,
        });
        // pi spreads `...details` then the per-call-site resolution/persistence keys; the tail
        // overwrites, matching JS object-literal ordering.
        if let (Value::Object(base), Value::Object(extra)) = (&mut record, &tail) {
            for (key, value) in extra {
                base.insert(key.clone(), value.clone());
            }
        }
        self.write_review_entry(event, &record);
    }

    /// pi `getPermissionDecisionScope` (v0.8.0 `index.ts:581-592`): the first non-empty of
    /// `target`, `command`, `path`, `toolName`, `skillName`.
    ///
    /// **PERM-028 — the first three go through `getNonEmptyString`, the last two do not.** Upstream
    /// is `getNonEmptyString(details.target) ?? getNonEmptyString(details.command) ??
    /// getNonEmptyString(details.path) ?? details.toolName ?? details.skillName ?? null`, and
    /// `getNonEmptyString` TRIMS (`common.ts:15-22`). So `command: "  git status  "` keys as
    /// `"git status"` upstream, and a whitespace-ONLY command is skipped entirely rather than
    /// selected. Cyrup previously filtered on a raw `!is_empty()` across all five, which both kept
    /// the padding and let `"   "` win. The asymmetry is deliberate and is upstream's: do not
    /// "tidy" it by trimming `toolName`/`skillName` too.
    pub(super) fn permission_decision_scope(details: &DedupDetails) -> Value {
        // pi's first three arms — `getNonEmptyString` = trim, then drop if empty
        // (`common::get_non_empty_string`, `common.rs:20`).
        let trimmed = [details.target.as_deref(), details.command.as_deref(), details.path.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|s| !s.is_empty());
        if let Some(s) = trimmed {
            return Value::String(s.to_string());
        }
        // pi's last two arms — RAW `??` fallthrough, no trim and no empty check. `??` skips only
        // `null`/`undefined`, which is `Option::None` here, so an empty-string `toolName` is
        // selected upstream and must be selected here.
        [details.tool_name.as_deref(), details.skill_name.as_deref()]
            .into_iter()
            .flatten()
            .next()
            .map_or(Value::Null, |s| Value::String(s.to_string()))
    }
}
