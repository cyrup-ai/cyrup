//! SUBA-074 stage 2 — the status/receipt half of
//! `pi-subagents/src/runs/shared/external-cli-contract.ts` (@v0.64.0):
//! `resolveExternalCliRunnerStatus` (`:77-116`), `normalizeExternalCliRunnerStatus` (`:119-144`)
//! and `externalCliReceiptMetadata` (`:146-167`), plus `ExternalProcessStatus`
//! (`shared/types.ts:1772-1786`).
//!
//! Everything in here is pure serde over the two closed enums in [`super::contract`]: no clock, no
//! filesystem, no process. That is what makes the per-adapter safety block and the seven
//! unsupported reasons testable as data rather than only through a live foreign process.

use serde_json::Value;

use super::contract::{AdapterId, Capability, PromptDeliveryKind};

/// The `adapter.id` a receipt carries — upstream's eight-member union
/// (`shared/types.ts:1709`), which is the six code-owned ids PLUS the generic `"external-cli"` and
/// the legacy `"grok-build"` that survives only in already-written receipts.
///
/// A closed enum rather than a `String` for the same reason [`AdapterId`] is: everything derived
/// from it (the safety block, the execution mode) is an exhaustive `match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReceiptAdapterId {
    /// No adapter declared — upstream's `input.adapter ?? "external-cli"` (`:88`).
    Generic,
    /// One of the six code-owned adapters.
    Owned(AdapterId),
    /// `grok-build` — the pre-rename `cursor-agent` id. Never produced by a new run; recognised
    /// only when reading a receipt back (`:126-138`).
    LegacyGrokBuild,
}

impl ReceiptAdapterId {
    /// The wire spelling.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Generic => "external-cli",
            Self::Owned(id) => id.wire(),
            Self::LegacyGrokBuild => "grok-build",
        }
    }

    /// The prompt delivery this id implies. The legacy `grok-build` id is a prompt-file adapter
    /// (`:131`), which is why it cannot simply be folded into [`Self::Generic`].
    #[must_use]
    pub const fn prompt_delivery(self) -> PromptDeliveryKind {
        match self {
            Self::Generic => PromptDeliveryKind::Stdin,
            Self::Owned(id) => id.prompt_delivery(),
            Self::LegacyGrokBuild => PromptDeliveryKind::PromptFile,
        }
    }
}

impl serde::Serialize for ReceiptAdapterId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.wire())
    }
}

impl<'de> serde::Deserialize<'de> for ReceiptAdapterId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "external-cli" => Self::Generic,
            "grok-build" => Self::LegacyGrokBuild,
            other => AdapterId::try_from(other).map(Self::Owned).map_err(|()| {
                serde::de::Error::custom(format!("unknown external-cli receipt adapter id '{raw}'"))
            })?,
        })
    }
}

/// `ExternalCliRunnerStatus["adapter"]` (`shared/types.ts:1709`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliAdapterDescriptor {
    /// The adapter id, or `external-cli` for the generic path.
    pub id: ReceiptAdapterId,
    /// Upstream's literal `1`.
    pub version: u8,
    /// `one-shot-stdin` / `one-shot-prompt-file` — a function of the id's delivery mode.
    pub execution_mode: String,
}

impl ExternalCliAdapterDescriptor {
    /// The descriptor upstream builds inline at `external-cli-contract.ts:88`.
    #[must_use]
    pub fn new(id: ReceiptAdapterId) -> Self {
        Self {
            id,
            version: 1,
            execution_mode: id.prompt_delivery().execution_mode().to_string(),
        }
    }
}

/// `ExternalCliCapabilities` (`shared/types.ts:1697-1706`) — every member is a LITERAL upstream, so
/// this type carries no information at all; it exists because it is written into every receipt and
/// read back by defensive normalization.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliCapabilities {
    /// Always `true`: an external run can always be stopped. This is why `stop` is not a
    /// [`Capability`] variant — there is nothing to narrow.
    pub stop: bool,
    /// Always `false`.
    pub steer: bool,
    /// Always `false`.
    pub resume: bool,
    /// Always `false`.
    pub structured_output: bool,
    /// Always `false`.
    pub tool_events: bool,
    /// Always the string `"unsupported"` — upstream's one non-boolean member.
    pub supervisor: String,
    /// Always `false`.
    pub fork_context: bool,
    /// Always `false`.
    pub extension_bindings: bool,
}

impl Default for ExternalCliCapabilities {
    fn default() -> Self {
        Self {
            stop: true,
            steer: false,
            resume: false,
            structured_output: false,
            tool_events: false,
            supervisor: "unsupported".to_string(),
            fork_context: false,
            extension_bindings: false,
        }
    }
}

/// `ExternalCliRunnerStatus["unsupportedReasons"]` (`shared/types.ts:1733`) — one reason per
/// narrowable capability.
///
/// Named fields in upstream's own key order rather than a map, so serialization reproduces
/// upstream's object key ORDER and so the seven are total at the type level. The link back to
/// [`Capability::unsupported_reason`] (which is where the strings actually live) is pinned by
/// `unsupported_reasons_are_exactly_the_capability_enums_reasons`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliUnsupportedReasons {
    /// Why live steering is unavailable.
    pub steer: String,
    /// Why the run is not resumable — also copied to
    /// [`ExternalCliRunnerStatus::non_resumable_reason`].
    pub resume: String,
    /// Why no trusted structured result is parsed.
    pub structured_output: String,
    /// Why stdout is not read as native tool events.
    pub tool_events: String,
    /// Why there is no supervisor transport.
    pub supervisor: String,
    /// Why native fork context is unavailable.
    pub fork_context: String,
    /// Why extension bindings never reach an external runner.
    pub extension_bindings: String,
}

impl ExternalCliUnsupportedReasons {
    /// `UNSUPPORTED` / `PROMPT_FILE_UNSUPPORTED` (`external-cli-contract.ts:8-24`), selected by the
    /// EFFECTIVE delivery mode.
    #[must_use]
    pub fn for_delivery(delivery: PromptDeliveryKind) -> Self {
        let reason = |capability: Capability| capability.unsupported_reason(delivery).to_string();
        Self {
            steer: reason(Capability::Steer),
            resume: reason(Capability::Resume),
            structured_output: reason(Capability::StructuredOutput),
            tool_events: reason(Capability::ToolEvents),
            supervisor: reason(Capability::Supervisor),
            fork_context: reason(Capability::ForkContext),
            extension_bindings: reason(Capability::ExtensionBindings),
        }
    }
}

/// `ExternalCliRunnerStatus` (`shared/types.ts:1725-1735`) — the runner descriptor a finished
/// external run publishes onto its result and its receipt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliRunnerStatus {
    /// Upstream's literal `"external-cli"` discriminant, carried so a persisted status is
    /// self-describing beside `ExternalJobRunnerStatus`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The command as declared (not the preflight-resolved binary path).
    pub command: String,
    /// The effective argv — the adapter's when one owns it, else the author's.
    pub args: Vec<String>,
    /// `stdin` or `prompt-file`.
    pub prompt_delivery: String,
    /// The adapter descriptor.
    pub adapter: ExternalCliAdapterDescriptor,
    /// The per-adapter sandbox block; absent for the generic adapter (upstream spreads nothing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety: Option<Value>,
    /// The fixed capability envelope.
    pub capabilities: ExternalCliCapabilities,
    /// One reason per narrowable capability.
    pub unsupported_reasons: ExternalCliUnsupportedReasons,
    /// `unsupported.resume`, hoisted because the steer/resume refusals read it directly
    /// (`runs/foreground/subagent-executor.ts:1589-1597`, `:1847-1852`).
    pub non_resumable_reason: String,
}

/// The per-adapter `safety` block (`external-cli-contract.ts:89-94`), as a total function of the
/// adapter id.
///
/// A `serde_json::Value` per arm rather than a seven-variant Rust enum on purpose: nothing ever
/// MATCHES on this value — it is written into a receipt and read back only by
/// [`normalize_external_cli_runner_status`], which REBUILDS it from the adapter id rather than
/// reading its fields (`:140-143`). An exhaustive match returning inline literals gives the same
/// drift protection as an enum with none of the type surface, and keeps the six field sets side by
/// side where a reviewer can diff them against upstream in one screen. That is only defensible
/// because [`AdapterId`] makes the match exhaustive; against an `Option<String>` it would be a
/// fall-through waiting to happen.
#[must_use]
pub fn safety_receipt(adapter: ReceiptAdapterId) -> Option<Value> {
    let ReceiptAdapterId::Owned(adapter) = adapter else {
        // The generic adapter spreads no `safety` key at all, and a legacy `grok-build` receipt
        // never carried one either (`:126-138` rebuilds without it).
        return None;
    };
    Some(match adapter {
        AdapterId::CodexExec => serde_json::json!({
            "sandbox": "read-only",
            "approvalPolicy": "never",
            "ephemeral": true,
        }),
        AdapterId::CodexExecWriter => serde_json::json!({
            "access": "workspace-write",
            "sandbox": "workspace-write",
            "approvalPolicy": "never",
            "ephemeral": true,
        }),
        AdapterId::ClaudeCode => serde_json::json!({
            "access": "read-only",
            "authentication": "existing-cli-required",
            "permissionMode": "plan",
            "tools": "none",
            "mcp": "empty-strict",
            "settingSources": "user",
            "userSettingsTrust": "required",
            "sessionPersistence": false,
        }),
        AdapterId::ClaudeCodeWriter => serde_json::json!({
            "access": "workspace-write",
            "authentication": "existing-cli-required",
            "permissionMode": "acceptEdits",
            "tools": "Read,Write,Edit,Glob,Grep",
            "mcp": "empty-strict",
            "settingSources": "user",
            "userSettingsTrust": "required",
            "sessionPersistence": false,
        }),
        AdapterId::CursorAgent => serde_json::json!({
            "access": "read-only",
            "authentication": "cursor-api-key-or-existing-login",
            "mode": "ask",
            "sandbox": "enabled",
            "workspaceTrust": "existing-required",
            "sessionReuse": false,
        }),
        AdapterId::CursorAgentWriter => serde_json::json!({
            "access": "workspace-write",
            "authentication": "cursor-api-key-or-existing-login",
            "mode": "print",
            "sandbox": "enabled",
            "workspaceTrust": "existing-required",
            "sessionReuse": false,
        }),
    })
}

/// `resolveExternalCliRunnerStatus(input)` (`external-cli-contract.ts:77-116`).
///
/// Note what it does NOT read: the author's `capabilities` narrowing. Upstream accepts the field on
/// its input type and never consults it — the published envelope is the code-owned literal, because
/// a narrowing can only ever remove something that is already `false`.
#[must_use]
pub fn resolve_external_cli_runner_status(
    adapter: Option<AdapterId>,
    command: &str,
    args: &[String],
) -> ExternalCliRunnerStatus {
    let id = adapter.map_or(ReceiptAdapterId::Generic, ReceiptAdapterId::Owned);
    let delivery = id.prompt_delivery();
    let unsupported_reasons = ExternalCliUnsupportedReasons::for_delivery(delivery);
    ExternalCliRunnerStatus {
        kind: "external-cli".to_string(),
        command: command.to_string(),
        args: args.to_vec(),
        prompt_delivery: delivery.wire().to_string(),
        adapter: ExternalCliAdapterDescriptor::new(id),
        safety: safety_receipt(id),
        capabilities: ExternalCliCapabilities::default(),
        non_resumable_reason: unsupported_reasons.resume.clone(),
        unsupported_reasons,
    }
}

/// `normalizeExternalCliRunnerStatus(value)` (`:119-144`) — the DEFENSIVE read-back of a persisted
/// status.
///
/// Rebuilds the whole status from the three fields it trusts (`command`, `args`, `adapter.id`)
/// rather than believing the stored capability envelope, so a receipt written by an older build —
/// or hand-edited — cannot claim a capability this build does not have. The `grok-build` arm is
/// upstream's own legacy branch for receipts written before `cursor-agent` was renamed.
#[must_use]
pub fn normalize_external_cli_runner_status(value: &Value) -> Option<ExternalCliRunnerStatus> {
    let object = value.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("external-cli") {
        return None;
    }
    let command = object.get("command").and_then(Value::as_str)?;
    if command.trim().is_empty() {
        return None;
    }
    // `Array.isArray(args) && args.every(isString)` — a mixed array is dropped whole, not filtered.
    let args: Vec<String> = match object.get("args") {
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    let adapter_id = object
        .get("adapter")
        .and_then(Value::as_object)
        .and_then(|adapter| adapter.get("id"))
        .and_then(Value::as_str);
    if adapter_id == Some("grok-build") {
        let unsupported_reasons =
            ExternalCliUnsupportedReasons::for_delivery(PromptDeliveryKind::PromptFile);
        return Some(ExternalCliRunnerStatus {
            kind: "external-cli".to_string(),
            command: command.to_string(),
            args,
            prompt_delivery: PromptDeliveryKind::PromptFile.wire().to_string(),
            adapter: ExternalCliAdapterDescriptor::new(ReceiptAdapterId::LegacyGrokBuild),
            safety: None,
            capabilities: ExternalCliCapabilities::default(),
            non_resumable_reason: unsupported_reasons.resume.clone(),
            unsupported_reasons,
        });
    }
    let adapter = adapter_id.and_then(|id| AdapterId::try_from(id).ok());
    Some(resolve_external_cli_runner_status(adapter, command, &args))
}

/// `ExternalProcessStatus` (`shared/types.ts:1772-1786`) — what the foreign process actually did.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalProcessStatus {
    /// The child's pid, absent when the spawn itself failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// `Date.now()` at spawn.
    pub started_at: i64,
    /// `Date.now()` at close.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    /// `endedAt - startedAt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// The process exit code, `None` for a signal death.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The killing signal's name, `None` on a normal exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_signal: Option<String>,
    /// The bounded stdout log.
    pub stdout_path: String,
    /// The bounded stderr log.
    pub stderr_path: String,
    /// An adapter-owned final-output artifact, when the adapter writes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_output_path: Option<String>,
    /// TOTAL stdout bytes the child produced — not the bytes written to the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_bytes: Option<u64>,
    /// TOTAL stderr bytes the child produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_bytes: Option<u64>,
    /// Set when the stdout log hit its cap.
    #[serde(default, skip_serializing_if = "crate::exec::is_false")]
    pub stdout_truncated: bool,
    /// Set when the stderr log hit its cap.
    #[serde(default, skip_serializing_if = "crate::exec::is_false")]
    pub stderr_truncated: bool,
}

/// `externalCliReceiptMetadata(input)` (`external-cli-contract.ts:146-167`) — the artifact-metadata
/// block a finished external run persists.
#[must_use]
pub fn external_cli_receipt_metadata(
    runner: &ExternalCliRunnerStatus,
    external_process: Option<&ExternalProcessStatus>,
    output_reference: Option<&str>,
) -> Value {
    let mut receipt = serde_json::Map::new();
    receipt.insert(
        "adapter".to_string(),
        serde_json::to_value(&runner.adapter).unwrap_or(Value::Null),
    );
    receipt.insert(
        "capabilities".to_string(),
        serde_json::to_value(&runner.capabilities).unwrap_or(Value::Null),
    );
    if let Some(safety) = &runner.safety {
        receipt.insert("safety".to_string(), safety.clone());
    }
    // `outputArtifacts` exists when there is a process (paths), OR when only an output reference
    // was resolved — upstream's nested ternary at `:152-159`.
    let artifacts = match (external_process, output_reference) {
        (Some(process), reference) => {
            let mut artifacts = serde_json::Map::new();
            artifacts.insert(
                "stdoutPath".to_string(),
                Value::String(process.stdout_path.clone()),
            );
            artifacts.insert(
                "stderrPath".to_string(),
                Value::String(process.stderr_path.clone()),
            );
            // `outputReference ?? finalOutputPath` — the reference WINS, and the key is omitted
            // entirely when neither exists.
            if let Some(final_output) = reference
                .map(str::to_string)
                .or_else(|| process.final_output_path.clone())
            {
                artifacts.insert("finalOutputPath".to_string(), Value::String(final_output));
            }
            Some(artifacts)
        }
        (None, Some(reference)) => {
            let mut artifacts = serde_json::Map::new();
            artifacts.insert(
                "finalOutputPath".to_string(),
                Value::String(reference.to_string()),
            );
            Some(artifacts)
        }
        (None, None) => None,
    };
    if let Some(artifacts) = artifacts {
        receipt.insert("outputArtifacts".to_string(), Value::Object(artifacts));
    }
    receipt.insert(
        "handoff".to_string(),
        serde_json::json!({ "mode": "fresh" }),
    );
    receipt.insert(
        "supervisor".to_string(),
        serde_json::json!({
            "mode": "unsupported",
            "reason": runner.unsupported_reasons.supervisor,
        }),
    );
    receipt.insert(
        "nonResumableReason".to_string(),
        Value::String(runner.non_resumable_reason.clone()),
    );
    Value::Object(receipt)
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

    /// The seven reasons a status publishes ARE [`Capability::unsupported_reason`]'s, under both
    /// delivery modes — so the named-field struct cannot drift from the enum the strings live on.
    #[test]
    fn unsupported_reasons_are_exactly_the_capability_enums_reasons() {
        for delivery in [PromptDeliveryKind::Stdin, PromptDeliveryKind::PromptFile] {
            let reasons = ExternalCliUnsupportedReasons::for_delivery(delivery);
            let serialized = serde_json::to_value(&reasons).unwrap();
            let object = serialized.as_object().unwrap();
            assert_eq!(object.len(), Capability::ALL.len());
            for capability in Capability::ALL {
                assert_eq!(
                    object.get(capability.wire()).and_then(Value::as_str),
                    Some(capability.unsupported_reason(delivery)),
                    "{} under {:?}",
                    capability.wire(),
                    delivery
                );
            }
        }
    }

    /// The generic (no-adapter) status: `adapter.id` is `external-cli`, stdin delivery, and **no
    /// safety block** (`:88-89`).
    #[test]
    fn the_generic_status_has_no_safety_block_and_delivers_on_stdin() {
        let status = resolve_external_cli_runner_status(None, "my-cli", &["-x".to_string()]);
        assert_eq!(status.adapter.id, ReceiptAdapterId::Generic);
        assert_eq!(status.adapter.execution_mode, "one-shot-stdin");
        assert_eq!(status.adapter.version, 1);
        assert_eq!(status.prompt_delivery, "stdin");
        assert_eq!(status.args, vec!["-x".to_string()]);
        assert!(status.safety.is_none());
        assert!(status.capabilities.stop);
        assert_eq!(status.capabilities.supervisor, "unsupported");
        assert_eq!(
            status.non_resumable_reason,
            Capability::Resume.unsupported_reason(PromptDeliveryKind::Stdin)
        );
    }

    /// Each of the six adapters gets its own safety block, verbatim from `:89-94`. A miss would
    /// publish a run as UNSANDBOXED that was in fact sandboxed (or the reverse), which is why the
    /// six literals are pinned rather than summarized.
    #[test]
    fn every_code_owned_adapter_publishes_its_own_safety_block() {
        let expected: [(AdapterId, Value); 6] = [
            (
                AdapterId::CodexExec,
                serde_json::json!({"sandbox":"read-only","approvalPolicy":"never","ephemeral":true}),
            ),
            (
                AdapterId::CodexExecWriter,
                serde_json::json!({"access":"workspace-write","sandbox":"workspace-write","approvalPolicy":"never","ephemeral":true}),
            ),
            (
                AdapterId::ClaudeCode,
                serde_json::json!({"access":"read-only","authentication":"existing-cli-required","permissionMode":"plan","tools":"none","mcp":"empty-strict","settingSources":"user","userSettingsTrust":"required","sessionPersistence":false}),
            ),
            (
                AdapterId::ClaudeCodeWriter,
                serde_json::json!({"access":"workspace-write","authentication":"existing-cli-required","permissionMode":"acceptEdits","tools":"Read,Write,Edit,Glob,Grep","mcp":"empty-strict","settingSources":"user","userSettingsTrust":"required","sessionPersistence":false}),
            ),
            (
                AdapterId::CursorAgent,
                serde_json::json!({"access":"read-only","authentication":"cursor-api-key-or-existing-login","mode":"ask","sandbox":"enabled","workspaceTrust":"existing-required","sessionReuse":false}),
            ),
            (
                AdapterId::CursorAgentWriter,
                serde_json::json!({"access":"workspace-write","authentication":"cursor-api-key-or-existing-login","mode":"print","sandbox":"enabled","workspaceTrust":"existing-required","sessionReuse":false}),
            ),
        ];
        for (adapter, safety) in expected {
            let status = resolve_external_cli_runner_status(Some(adapter), "c", &[]);
            assert_eq!(status.safety.as_ref(), Some(&safety), "{adapter}");
            assert_eq!(status.adapter.id, ReceiptAdapterId::Owned(adapter));
        }
    }

    /// cursor-agent is forced to prompt-file delivery and therefore gets the OVERRIDDEN steer and
    /// resume reasons (`:93`, `:19-24`) — the wording is factually about how the prompt arrived.
    #[test]
    fn cursor_agent_forces_prompt_file_delivery_and_its_overridden_reasons() {
        let status = resolve_external_cli_runner_status(Some(AdapterId::CursorAgent), "c", &[]);
        assert_eq!(status.prompt_delivery, "prompt-file");
        assert_eq!(status.adapter.execution_mode, "one-shot-prompt-file");
        assert!(status.unsupported_reasons.steer.contains("prompt-file"));
        assert!(status.non_resumable_reason.contains("prompt-file"));

        let claude = resolve_external_cli_runner_status(Some(AdapterId::ClaudeCode), "c", &[]);
        assert!(claude.unsupported_reasons.steer.contains("stdin"));
    }

    /// The read-back is defensive: it rebuilds from `adapter.id` rather than trusting a stored
    /// envelope, and it recognises the legacy `grok-build` id as a prompt-file adapter (`:126-138`).
    #[test]
    fn normalization_rebuilds_the_envelope_and_honours_the_legacy_grok_build_id() {
        let stored = serde_json::json!({
            "type": "external-cli",
            "command": "cursor-agent",
            "args": ["--print"],
            "adapter": {"id": "grok-build", "version": 1, "executionMode": "one-shot-prompt-file"},
            "capabilities": {"stop": true, "steer": true},
        });
        let status = normalize_external_cli_runner_status(&stored).unwrap();
        assert_eq!(status.adapter.id, ReceiptAdapterId::LegacyGrokBuild);
        assert_eq!(status.prompt_delivery, "prompt-file");
        assert!(status.safety.is_none());
        assert!(!status.capabilities.steer, "a stored widening is discarded");

        // An unknown adapter id degrades to the generic status rather than being trusted.
        let unknown = serde_json::json!({
            "type": "external-cli",
            "command": "c",
            "adapter": {"id": "cursor_agent"},
        });
        let status = normalize_external_cli_runner_status(&unknown).unwrap();
        assert_eq!(status.adapter.id, ReceiptAdapterId::Generic);

        // A code-owned id is honoured, with its safety block rebuilt from code.
        let owned = serde_json::json!({
            "type": "external-cli",
            "command": "claude",
            "adapter": {"id": "claude-code"},
        });
        let status = normalize_external_cli_runner_status(&owned).unwrap();
        assert_eq!(
            status.adapter.id,
            ReceiptAdapterId::Owned(AdapterId::ClaudeCode)
        );
        assert!(status.safety.is_some());

        // The three rejections: wrong type, missing/blank command, non-object.
        assert!(
            normalize_external_cli_runner_status(
                &serde_json::json!({"type":"external-job","command":"c"})
            )
            .is_none()
        );
        assert!(
            normalize_external_cli_runner_status(
                &serde_json::json!({"type":"external-cli","command":"  "})
            )
            .is_none()
        );
        assert!(normalize_external_cli_runner_status(&serde_json::json!([])).is_none());

        // A MIXED args array is dropped whole rather than filtered (`:122-124`).
        let mixed = serde_json::json!({"type":"external-cli","command":"c","args":["a", 1]});
        assert!(
            normalize_external_cli_runner_status(&mixed)
                .unwrap()
                .args
                .is_empty()
        );
    }

    /// The receipt names the adapter, the capability envelope, the safety block and the artifact
    /// paths, and the output REFERENCE wins over the process's own final-output path (`:152-159`).
    #[test]
    fn the_receipt_carries_the_adapter_safety_and_artifact_paths() {
        let status = resolve_external_cli_runner_status(Some(AdapterId::ClaudeCode), "claude", &[]);
        let process = ExternalProcessStatus {
            started_at: 10,
            stdout_path: "/scratch/external-0.stdout.log".to_string(),
            stderr_path: "/scratch/external-0.stderr.log".to_string(),
            final_output_path: Some("/scratch/final.txt".to_string()),
            ..ExternalProcessStatus::default()
        };
        let receipt = external_cli_receipt_metadata(&status, Some(&process), Some("/out/saved.md"));
        assert_eq!(receipt["adapter"]["id"], "claude-code");
        assert_eq!(receipt["safety"]["permissionMode"], "plan");
        assert_eq!(
            receipt["outputArtifacts"]["stdoutPath"],
            "/scratch/external-0.stdout.log"
        );
        assert_eq!(
            receipt["outputArtifacts"]["finalOutputPath"], "/out/saved.md",
            "the resolved output reference wins over the adapter's own artifact"
        );
        assert_eq!(receipt["handoff"]["mode"], "fresh");
        assert_eq!(receipt["supervisor"]["mode"], "unsupported");
        assert_eq!(
            receipt["supervisor"]["reason"],
            Value::String(status.unsupported_reasons.supervisor.clone())
        );
        assert_eq!(
            receipt["nonResumableReason"],
            Value::String(status.non_resumable_reason.clone())
        );

        // No process and no reference: `outputArtifacts` is omitted entirely.
        let bare = external_cli_receipt_metadata(&status, None, None);
        assert!(bare.get("outputArtifacts").is_none());
        // A reference with no process still publishes one.
        let reference_only = external_cli_receipt_metadata(&status, None, Some("/out/saved.md"));
        assert_eq!(
            reference_only["outputArtifacts"]["finalOutputPath"],
            "/out/saved.md"
        );
        assert!(
            reference_only["outputArtifacts"]
                .get("stdoutPath")
                .is_none()
        );
    }

    /// The whole status round-trips through JSON, because it is embedded in `SingleResult` and in
    /// `status.json`.
    #[test]
    fn a_status_round_trips_through_json() {
        for adapter in [
            None,
            Some(AdapterId::ClaudeCodeWriter),
            Some(AdapterId::CursorAgent),
        ] {
            let status = resolve_external_cli_runner_status(adapter, "c", &["-a".to_string()]);
            let json = serde_json::to_string(&status).unwrap();
            let back: ExternalCliRunnerStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, status);
            assert_eq!(
                serde_json::from_str::<Value>(&json).unwrap()["type"],
                "external-cli"
            );
        }
    }
}
