//! [`HostStepNode`] and its vocabulary — pi `shared/types.ts:46-76` (SCOPE_3e SUBTASK0).
//!
//! Plain data: the bounded, provider-agnostic status of a host-owned workflow monitor, sourced
//! only from [`crate::workflows::WorkflowChecklistInput`]'s `host_steps` in this task. Every field
//! is read by the checklist's `host_item` (`workflow-checklist.ts:218-236`) or carried onto a
//! [`crate::workflows::WorkflowChecklistItem`].
//!
//! `version: 1` and `kind: "host-step"` are unit types (the [`crate::workflows::SummaryVersion`]
//! pattern): "this is a version-1 host step" is a *parse* outcome, not a field a reader must
//! remember to check. The three word-sets ([`HostStepMonitorKind`], [`HostStepState`],
//! [`HostStepVerdict`]) are domain enums matched exhaustively — never with a catch-all arm;
//! [`HostStepFreshness`] is the one member of the family that is a struct upstream too
//! (`shared/types.ts:50-54`), because it carries the expected/observed ref pair a word cannot.

/// The literal `version: 1` on a [`HostStepNode`] — pi `HostStepNode.version`
/// (`shared/types.ts:58`). Any other value fails to parse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostStepVersion;

impl HostStepVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = 1;
}

impl serde::Serialize for HostStepVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for HostStepVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported host step version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// The literal `kind: "host-step"` on a [`HostStepNode`] — pi `HostStepNode.kind`
/// (`shared/types.ts:59`). Same unit-type reasoning as [`HostStepVersion`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostStepKind;

impl HostStepKind {
    /// The only value this type represents.
    pub const VALUE: &'static str = "host-step";
}

impl serde::Serialize for HostStepKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for HostStepKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported host step kind '{raw}' (expected '{}')",
                Self::VALUE
            )))
        }
    }
}

/// The explicit monitor category — pi `HostStepMonitorKind` (`shared/types.ts:46`). "Never
/// inferred from labels or commands" (upstream's own doc on the field).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostStepMonitorKind {
    /// A one-shot host command (`runs.host`).
    Command,
    /// A CI pipeline watch.
    Ci,
    /// A gating check.
    Gate,
}

impl HostStepMonitorKind {
    /// The serde word, for display seams that print the kind verbatim
    /// (`formatWorkflowChecklistItem`'s first detail cell).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Ci => "ci",
            Self::Gate => "gate",
        }
    }
}

/// A host step's lifecycle word — pi `HostStepState` (`shared/types.ts:47`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostStepState {
    /// Declared but not yet started.
    Pending,
    /// Currently running.
    Running,
    /// Finished (its verdict says how).
    Done,
    /// Cancelled before settling.
    Cancelled,
    /// Failed with a host-side error.
    Error,
}

impl HostStepState {
    /// The serde word — what `checklistState({ status: host.state, … })` receives upstream
    /// (`workflow-checklist.ts:219`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        }
    }
}

/// A settled host step's verdict word — pi `HostStepVerdict` (`shared/types.ts:48`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostStepVerdict {
    /// The monitored check passed.
    Pass,
    /// The monitored check failed.
    Fail,
    /// The monitor could not decide — `explicitBlocked` treats this as a block signal
    /// (`workflow-checklist.ts:150`).
    Inconclusive,
}

impl HostStepVerdict {
    /// The serde word — what `explicitBlocked`'s `source.verdict === "inconclusive"` compares
    /// against upstream.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Inconclusive => "inconclusive",
        }
    }
}

/// Freshness of a host step's observation — pi `HostStepFreshness` (`shared/types.ts:50-54`). A
/// struct, not a word: it carries the expected/observed ref pair.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStepFreshness {
    /// The ref the monitor is expected to report on.
    pub expected_ref: String,
    /// The ref it actually reported on, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_ref: Option<String>,
    /// Whether the observation is stale — `explicitBlocked` treats `true` as a block signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
}

/// pi `HOST_STEP_MAX_COUNT` (`runs/shared/host-step-status.ts:20`): the per-workflow ceiling on
/// host steps — `runs.host` calls beyond it are refused with the verbatim
/// `workflowScript supports at most 32 runs.host calls.` (consumed by SCOPE_3f's engine; declared
/// here because this file owns the host-step vocabulary).
pub const HOST_STEP_MAX_COUNT: usize = 32;

/// Bounded, provider-agnostic status for a host-owned workflow monitor — pi `HostStepNode`
/// (`shared/types.ts:57-76`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStepNode {
    /// Always `1`; any other value fails to parse.
    pub version: HostStepVersion,
    /// Always `"host-step"`; any other value fails to parse.
    pub kind: HostStepKind,
    /// Explicit monitor category; never inferred from labels or commands.
    pub monitor_kind: HostStepMonitorKind,
    /// The step's stable id (a workflow key by convention; raw here, as upstream).
    pub id: String,
    /// The human-readable label.
    pub label: String,
    /// The declared role, when any (`"ci"`/`"gate"` by convention).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The provider the monitor reads from, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The step's lifecycle state.
    pub state: HostStepState,
    /// The settled verdict, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<HostStepVerdict>,
    /// A bounded machine-readable reason code, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    /// Human-readable detail — the checklist renders it as the item's `error` cell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// What the monitor points at (a URL/ref/path), when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Freshness of the observation, when tracked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<HostStepFreshness>,
    /// A report artifact path, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_path: Option<String>,
    /// The monitored command's exit code, when it ran (upstream `number | null`; a JSON `null`
    /// reads as `None` here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<serde_json::Number>,
    /// Epoch-millis of the last update.
    pub updated_at: serde_json::Number,
    /// Epoch-millis deadline, when armed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<serde_json::Number>,
}

/// pi `assertUniqueHostStepIds` (`runs/shared/host-step-status.ts:116-122`) — no two host steps
/// in the same collection may share an `id`.
///
/// A `Vec` scan, not a `HashSet` build: [`HOST_STEP_MAX_COUNT`] is 32, so the quadratic form is
/// bounded at 496 comparisons and preserves upstream's FIRST-duplicate-wins message (the SECOND
/// occurrence of a repeated id is what triggers the error, naming that id — identical to a
/// forward `Set`-membership scan's own observable behaviour).
///
/// # Errors
///
/// `Invalid host step '<source>': duplicate host step id '<id>'.`, verbatim.
pub fn assert_unique_host_step_ids(host_steps: &[HostStepNode], source: &str) -> Result<(), String> {
    for (index, step) in host_steps.iter().enumerate() {
        let duplicate = host_steps.iter().take(index).any(|earlier| earlier.id == step.id);
        if duplicate {
            return Err(format!(
                "Invalid host step '{source}': duplicate host step id '{}'.",
                step.id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// `version: 1` and `kind: "host-step"` are parse outcomes; anything else is rejected at the
    /// boundary rather than checked by every reader.
    #[test]
    fn version_and_kind_are_parse_outcomes() {
        let json = serde_json::json!({
            "version": 1,
            "kind": "host-step",
            "monitorKind": "ci",
            "id": "ci.main",
            "label": "CI",
            "state": "running",
            "updatedAt": 1000,
        });
        let node: HostStepNode = serde_json::from_value(json).expect("well-formed node parses");
        assert_eq!(node.monitor_kind, HostStepMonitorKind::Ci);
        assert_eq!(node.state, HostStepState::Running);

        let bad_version = serde_json::json!({
            "version": 2, "kind": "host-step", "monitorKind": "ci",
            "id": "x", "label": "x", "state": "pending", "updatedAt": 0,
        });
        assert!(serde_json::from_value::<HostStepNode>(bad_version).is_err());

        let bad_kind = serde_json::json!({
            "version": 1, "kind": "step", "monitorKind": "ci",
            "id": "x", "label": "x", "state": "pending", "updatedAt": 0,
        });
        assert!(serde_json::from_value::<HostStepNode>(bad_kind).is_err());
    }

    fn node(id: &str) -> HostStepNode {
        HostStepNode {
            version: HostStepVersion,
            kind: HostStepKind,
            monitor_kind: HostStepMonitorKind::Ci,
            id: id.to_string(),
            label: id.to_string(),
            role: None,
            provider: None,
            state: HostStepState::Running,
            verdict: None,
            reason_code: None,
            detail: None,
            target: None,
            freshness: None,
            report_path: None,
            exit_code: None,
            updated_at: serde_json::Number::from(0),
            deadline_at: None,
        }
    }

    /// pi `assertUniqueHostStepIds`: distinct ids pass; the SECOND occurrence of a repeated id
    /// triggers the rejection, naming that id — first-duplicate-wins, upstream's own message.
    #[test]
    fn assert_unique_host_step_ids_rejects_the_second_occurrence_of_a_repeat() {
        assert!(assert_unique_host_step_ids(&[node("a"), node("b")], "status").is_ok());
        assert_eq!(
            assert_unique_host_step_ids(&[node("a"), node("b"), node("a")], "status"),
            Err("Invalid host step 'status': duplicate host step id 'a'.".to_string())
        );
        assert!(assert_unique_host_step_ids(&[], "status").is_ok());
    }

    /// The wire words are pi's exactly — the checklist state derivation compares against them.
    #[test]
    fn wire_words_match_upstream() {
        assert_eq!(HostStepState::Cancelled.as_str(), "cancelled");
        assert_eq!(HostStepVerdict::Inconclusive.as_str(), "inconclusive");
        assert_eq!(HostStepMonitorKind::Command.as_str(), "command");
        assert_eq!(
            serde_json::to_string(&HostStepState::Done).expect("serializes"),
            "\"done\""
        );
        assert_eq!(
            serde_json::to_string(&HostStepMonitorKind::Gate).expect("serializes"),
            "\"gate\""
        );
    }
}
