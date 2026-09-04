//! SUBA-074 — the code-owned half of `pi-subagents/src/runs/shared/external-cli-contract.ts`
//! (@v0.64.0): the adapter id set, the seven narrowable capabilities **and their unsupported
//! reasons**, the capability-narrowing parser, and the reserved-selection-name guard.
//!
//! Everything here is a CLOSED set the compiler knows. That is deliberate and is the item's
//! central design decision: upstream derives its label, its reserved table, its per-adapter safety
//! block, its prompt-delivery mode and its launch resolver from the same six string literals, and
//! every one of those derivations is a fall-through waiting to happen when the id is an
//! `Option<String>`. Modelling the id as [`AdapterId`] and the capability as [`Capability`] makes
//! each derivation an exhaustive `match`, so adding a seventh adapter (or an eighth capability) is
//! a compile error in every place that must change rather than a silent generic fallback.
//!
//! The status/receipt half — `resolveExternalCliRunnerStatus`, `normalizeExternalCliRunnerStatus`,
//! `externalCliReceiptMetadata` — lives in [`super::status`], which is built entirely out of the
//! two enums below.

use std::collections::BTreeSet;

use serde_json::Value;

/// How the prompt actually reaches the external child. Upstream's `promptDelivery`
/// (`shared/types.ts:1729` @v0.64.0) — and note the AUTHOR only ever writes `"stdin"`
/// (`agents.ts:1886-1888`): the effective mode is the ADAPTER's, not the author's
/// (`external-cli-contract.ts:93` forces `"prompt-file"` for cursor-agent regardless of what the
/// file said).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PromptDeliveryKind {
    /// Written to the child's stdin, which is then closed.
    Stdin,
    /// Written to an adapter-owned file named in argv; stdin is closed empty.
    PromptFile,
}

impl PromptDeliveryKind {
    /// Upstream's `promptDelivery` wire value.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Stdin => "stdin",
            Self::PromptFile => "prompt-file",
        }
    }

    /// Upstream's `adapter.executionMode` (`external-cli-contract.ts:88`), which is a pure function
    /// of the delivery mode.
    #[must_use]
    pub const fn execution_mode(self) -> &'static str {
        match self {
            Self::Stdin => "one-shot-stdin",
            Self::PromptFile => "one-shot-prompt-file",
        }
    }
}

/// `CODE_OWNED_EXTERNAL_CLI_ADAPTER_IDS` (`external-cli-contract.ts:25-33` @v0.64.0) as a closed
/// enum rather than a string set.
///
/// **The invariant this encodes.** An adapter id is one of exactly six values, and the user-facing
/// label, the reserved-name table, the prompt-delivery mode, the safety receipt and the launch
/// resolver are all FUNCTIONS of that set — upstream literally builds its label by mapping over the
/// array (`:36`). With an `Option<String>` every one of those is a string comparison that falls
/// through silently on a typo: `resolveExternalCliRunnerStatus` (`:77-116`) is six such
/// comparisons, and a miss yields the GENERIC status — adapter id `"external-cli"`, **no `safety`
/// block at all**, and `promptDelivery: "stdin"` for an adapter that needs a prompt file. That
/// misreports the capability envelope of a run that was supposed to be sandboxed, which is the same
/// severity class as widening it.
///
/// [`Self::try_from`] is the only constructor, so a value of this type is proof of membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdapterId {
    /// Codex `exec`, read-only sandbox.
    CodexExec,
    /// Codex `exec`, workspace-write.
    CodexExecWriter,
    /// Claude Code, plan mode with no tools.
    ClaudeCode,
    /// Claude Code, `acceptEdits` with the five file tools.
    ClaudeCodeWriter,
    /// cursor-agent, ask mode.
    CursorAgent,
    /// cursor-agent, print mode.
    CursorAgentWriter,
}

impl AdapterId {
    /// Every code-owned id, in upstream's own array order — the order is load-bearing because
    /// [`CODE_OWNED_ADAPTER_LABEL`] is built from it and appears verbatim in a user-facing refusal.
    pub const ALL: [Self; 6] = [
        Self::CodexExec,
        Self::CodexExecWriter,
        Self::ClaudeCode,
        Self::ClaudeCodeWriter,
        Self::CursorAgent,
        Self::CursorAgentWriter,
    ];

    /// The wire spelling — what an agent file writes and what a receipt records.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::CodexExec => "codex-exec",
            Self::CodexExecWriter => "codex-exec-writer",
            Self::ClaudeCode => "claude-code",
            Self::ClaudeCodeWriter => "claude-code-writer",
            Self::CursorAgent => "cursor-agent",
            Self::CursorAgentWriter => "cursor-agent-writer",
        }
    }

    /// `RESERVED_READ_ONLY_ADAPTERS` (`external-cli-contract.ts:42-46`) as a total function:
    /// `Some((writer twin, access word))` for a reserved READ-ONLY id, `None` for a writer id.
    ///
    /// The access word differs per adapter and is interpolated into the refusal, so the three rows
    /// are NOT interchangeable.
    #[must_use]
    pub const fn reserved_pair(self) -> Option<(Self, &'static str)> {
        match self {
            Self::ClaudeCode => Some((Self::ClaudeCodeWriter, "file-write")),
            Self::CodexExec => Some((Self::CodexExecWriter, "workspace-write")),
            Self::CursorAgent => Some((Self::CursorAgentWriter, "workspace-write")),
            Self::ClaudeCodeWriter | Self::CodexExecWriter | Self::CursorAgentWriter => None,
        }
    }

    /// The EFFECTIVE prompt delivery for this adapter (`external-cli-contract.ts:93`: `cursor ?
    /// "prompt-file" : input.promptDelivery ?? "stdin"`). Not the author's declaration — the
    /// adapter's own, which is why the author's `promptDelivery: "stdin"` is nearly free of
    /// information.
    #[must_use]
    pub const fn prompt_delivery(self) -> PromptDeliveryKind {
        match self {
            Self::CursorAgent | Self::CursorAgentWriter => PromptDeliveryKind::PromptFile,
            Self::CodexExec | Self::CodexExecWriter | Self::ClaudeCode | Self::ClaudeCodeWriter => {
                PromptDeliveryKind::Stdin
            }
        }
    }

    /// Whether this adapter grants the foreign process WRITE access to the workspace — the half of
    /// each reserved pair that is not read-only.
    #[must_use]
    pub const fn is_writer(self) -> bool {
        self.reserved_pair().is_none()
    }
}

impl TryFrom<&str> for AdapterId {
    type Error = ();

    /// `isCodeOwnedExternalCliAdapterId` (`external-cli-contract.ts:38-40`) — the ONLY constructor,
    /// so holding an [`AdapterId`] is proof the string was in the code-owned set.
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.wire() == value)
            .ok_or(())
    }
}

impl std::str::FromStr for AdapterId {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl std::fmt::Display for AdapterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire())
    }
}

impl serde::Serialize for AdapterId {
    /// The persisted form is the WIRE STRING, not the variant name: this field crosses the hop-2
    /// `runner-config.json` boundary (see the module doc on [`super`]) and is re-read by
    /// [`super::status::normalize_external_cli_runner_status`] out of a receipt.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.wire())
    }
}

impl<'de> serde::Deserialize<'de> for AdapterId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::try_from(raw.as_str()).map_err(|()| {
            serde::de::Error::custom(format!("unknown external-cli adapter id '{raw}'"))
        })
    }
}

/// `CODE_OWNED_EXTERNAL_CLI_ADAPTER_LABEL` — each id single-quoted, comma-joined. A `const` string
/// rather than a computed join so the refusal text cannot drift from [`AdapterId::ALL`] without the
/// accompanying assertion failing.
pub const CODE_OWNED_ADAPTER_LABEL: &str = "'codex-exec', 'codex-exec-writer', 'claude-code', 'claude-code-writer', 'cursor-agent', 'cursor-agent-writer'";

/// `isCodeOwnedExternalCliAdapterId` (`:38-40`), kept as a free predicate for the two frontmatter
/// parsers that must answer "is this string legal?" before they have anywhere to put the id.
#[must_use]
pub fn is_code_owned_adapter_id(value: &str) -> bool {
    AdapterId::try_from(value).is_ok()
}

/// The seven narrowable capabilities — upstream's `Object.keys(UNSUPPORTED)`
/// (`external-cli-contract.ts:8-17`).
///
/// `stop` is deliberately NOT a variant: it is the one capability an external adapter always has
/// (`capabilities.stop: true`, `shared/types.ts:1698`), so it is not narrowable and naming it in a
/// `capabilities:` block is an unsupported-field error. Making that structural rather than a
/// comment is half the point of the enum.
///
/// The other half is [`Self::unsupported_reason`]: upstream defines the key set AS the reason map's
/// domain (`CAPABILITY_KEYS = new Set(Object.keys(UNSUPPORTED))`, `:17`), so the two can never
/// drift there. A Rust port that keeps a key array and adds a separate reason table reintroduces
/// exactly that drift, and the idiomatic `.unwrap_or_default()` on a lookup miss writes an EMPTY
/// `nonResumableReason` into a receipt — a run recorded as non-resumable for no stated reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// Live steer messages after launch.
    Steer,
    /// Resuming a durable external session.
    Resume,
    /// A trusted structured result.
    StructuredOutput,
    /// Native Pi tool events on the child's stream.
    ToolEvents,
    /// A trusted supervisor event transport.
    Supervisor,
    /// Native Pi fork context.
    ForkContext,
    /// Native Pi extension bindings.
    ExtensionBindings,
}

impl Capability {
    /// Upstream's key order (`:8-16`), which reaches the user through
    /// [`super::status::ExternalCliRunnerStatus::unsupported_reasons`].
    pub const ALL: [Self; 7] = [
        Self::Steer,
        Self::Resume,
        Self::StructuredOutput,
        Self::ToolEvents,
        Self::Supervisor,
        Self::ForkContext,
        Self::ExtensionBindings,
    ];

    /// The wire key an agent file's `capabilities:` block and a receipt both use.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Resume => "resume",
            Self::StructuredOutput => "structuredOutput",
            Self::ToolEvents => "toolEvents",
            Self::Supervisor => "supervisor",
            Self::ForkContext => "forkContext",
            Self::ExtensionBindings => "extensionBindings",
        }
    }

    /// `UNSUPPORTED[key]` (`:8-16`), with `PROMPT_FILE_UNSUPPORTED`'s two overrides (`:19-24`)
    /// folded in as the `PromptFile` arms.
    ///
    /// Total by construction: no capability can lack a reason, and the delivery-mode override
    /// cannot be applied to the wrong subset because it IS this match.
    #[must_use]
    pub const fn unsupported_reason(self, delivery: PromptDeliveryKind) -> &'static str {
        match (self, delivery) {
            (Self::Steer, PromptDeliveryKind::Stdin) => {
                "The one-shot stdin adapter closes input after launch and cannot accept live steer messages."
            }
            (Self::Steer, PromptDeliveryKind::PromptFile) => {
                "The one-shot prompt-file adapter closes input after launch and cannot accept live steer messages."
            }
            (Self::Resume, PromptDeliveryKind::Stdin) => {
                "The one-shot stdin adapter has no durable external session identity."
            }
            (Self::Resume, PromptDeliveryKind::PromptFile) => {
                "The one-shot prompt-file adapter does not retain a durable external session identity."
            }
            (Self::StructuredOutput, _) => {
                "The generic external CLI adapter does not parse a trusted structured result."
            }
            (Self::ToolEvents, _) => {
                "The generic external CLI adapter treats stdout as untrusted text, not native Pi tool events."
            }
            (Self::Supervisor, _) => {
                "The generic external CLI adapter has no trusted supervisor event transport."
            }
            (Self::ForkContext, _) => {
                "Native Pi fork context is not available without an adapter-owned handoff artifact."
            }
            (Self::ExtensionBindings, _) => {
                "Native Pi extension bindings are never passed to external runners."
            }
        }
    }
}

impl serde::Serialize for Capability {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.wire())
    }
}

impl<'de> serde::Deserialize<'de> for Capability {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.wire() == raw)
            .ok_or_else(|| {
                serde::de::Error::custom(format!("unknown external-cli capability '{raw}'"))
            })
    }
}

/// `ExternalCliCapabilityNarrowing` — the parsed narrowing.
///
/// A SET, not a map, because upstream's type is `Partial<Record<key, false>>`
/// (`shared/types.ts:1695`) and its parser refuses any value that is not literally `false`
/// (`:65-76`): every value in the map is `false` by construction, so a map is a set wearing a map's
/// clothes. Keeping it a set removes the only place a `true` could ever be represented.
pub type ExternalCliCapabilityNarrowing = BTreeSet<Capability>;

/// `parseExternalCliCapabilityNarrowing(value, label)` (`:65-76`).
///
/// # Errors
///
/// Upstream's three refusals, verbatim: a non-object, an unsupported key, and — the load-bearing
/// one — any value that is not literally `false`, because user config may only ever NARROW a
/// code-owned adapter's capabilities, never widen them.
pub fn parse_capability_narrowing(
    value: Option<&Value>,
    label: &str,
) -> Result<Option<ExternalCliCapabilityNarrowing>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(object) = value.as_object() else {
        return Err(format!("{label} must be an object."));
    };
    let unknown: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| Capability::ALL.iter().all(|known| known.wire() != *key))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "{label} has unsupported fields: {}.",
            unknown.join(", ")
        ));
    }
    let mut narrowed = ExternalCliCapabilityNarrowing::new();
    for (key, setting) in object {
        if setting != &Value::Bool(false) {
            return Err(format!(
                "{label}.{key} may only be false; user config cannot widen code-owned external \
                 adapter capabilities."
            ));
        }
        // The unknown-key sweep above already proved membership, so this cannot fail; expressing
        // it as a filter_map keeps the function free of a panic path.
        if let Some(capability) = Capability::ALL
            .into_iter()
            .find(|candidate| candidate.wire() == key)
        {
            narrowed.insert(capability);
        }
    }
    Ok(Some(narrowed))
}

/// `validateCodeOwnedProfileRunner` (`:48-63`) — the reserved-selection-name guard.
///
/// An agent reachable by the selection name `claude-code`/`codex-exec`/`cursor-agent` MUST actually
/// be that read-only adapter. Without this a hand-written agent can squat the reserved name and be
/// selected in place of the sandboxed read-only profile — the same class of silent widening this
/// whole item exists to close.
///
/// `selection_names` is upstream's `[name, localName?, ...aliases]`. The reserved rows are derived
/// from [`AdapterId::reserved_pair`], so they cannot drift from the id set.
#[must_use]
pub fn validate_code_owned_profile_runner(
    selection_names: &[&str],
    runner_adapter: Option<AdapterId>,
) -> Option<String> {
    for reserved in AdapterId::ALL {
        let Some((writer, access)) = reserved.reserved_pair() else {
            continue;
        };
        let name = reserved.wire();
        if selection_names.contains(&name) && runner_adapter != Some(reserved) {
            return Some(format!(
                "Selection name '{name}' is reserved for the read-only '{name}' adapter. Use '{}' \
                 for explicit {access} access.",
                writer.wire()
            ));
        }
    }
    None
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

    /// The label a user-facing refusal quotes is built from the SAME set the parser accepts
    /// (`external-cli-contract.ts:36`), so the two can never drift.
    #[test]
    fn the_adapter_label_is_the_id_set_and_nothing_else() {
        let derived = AdapterId::ALL
            .iter()
            .map(|id| format!("'{}'", id.wire()))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(derived, CODE_OWNED_ADAPTER_LABEL);
        for id in AdapterId::ALL {
            assert_eq!(AdapterId::try_from(id.wire()), Ok(id));
        }
        assert_eq!(AdapterId::try_from("cursor_agent"), Err(()));
        assert_eq!(AdapterId::try_from("grok-build"), Err(()));
        assert!(!is_code_owned_adapter_id("claude-code-reader"));
    }

    /// The three reserved read-only ids, their writer twins and their per-adapter access words —
    /// derived from the enum rather than a parallel table (`:42-46`).
    #[test]
    fn exactly_three_ids_are_reserved_and_each_names_its_own_writer_and_access_word() {
        let reserved: Vec<(&str, &str, &str)> = AdapterId::ALL
            .into_iter()
            .filter_map(|id| {
                id.reserved_pair()
                    .map(|(writer, access)| (id.wire(), writer.wire(), access))
            })
            .collect();
        assert_eq!(
            reserved,
            vec![
                ("codex-exec", "codex-exec-writer", "workspace-write"),
                ("claude-code", "claude-code-writer", "file-write"),
                ("cursor-agent", "cursor-agent-writer", "workspace-write"),
            ]
        );
        assert!(AdapterId::ClaudeCodeWriter.is_writer());
        assert!(!AdapterId::ClaudeCode.is_writer());
    }

    /// Only cursor-agent delivers its prompt by file (`:93`), and the execution mode follows the
    /// delivery rather than being a second literal that could disagree with it.
    #[test]
    fn only_cursor_agent_delivers_its_prompt_by_file() {
        for id in AdapterId::ALL {
            let expected = if id.wire().starts_with("cursor-agent") {
                PromptDeliveryKind::PromptFile
            } else {
                PromptDeliveryKind::Stdin
            };
            assert_eq!(id.prompt_delivery(), expected, "{id}");
        }
        assert_eq!(PromptDeliveryKind::Stdin.execution_mode(), "one-shot-stdin");
        assert_eq!(
            PromptDeliveryKind::PromptFile.execution_mode(),
            "one-shot-prompt-file"
        );
    }

    /// Every capability has a NON-EMPTY reason under BOTH delivery modes, and the prompt-file
    /// override touches exactly `steer` and `resume` (`:19-24`) — exhaustive over
    /// `Capability::ALL × PromptDeliveryKind`, so an eighth capability cannot silently arrive
    /// without one.
    #[test]
    fn every_capability_has_a_reason_and_only_steer_and_resume_change_by_delivery() {
        let mut overridden = Vec::new();
        for capability in Capability::ALL {
            let stdin = capability.unsupported_reason(PromptDeliveryKind::Stdin);
            let file = capability.unsupported_reason(PromptDeliveryKind::PromptFile);
            assert!(!stdin.is_empty(), "{}", capability.wire());
            assert!(!file.is_empty(), "{}", capability.wire());
            if stdin != file {
                overridden.push(capability.wire());
            }
        }
        assert_eq!(overridden, vec!["steer", "resume"]);
        assert_eq!(
            Capability::Resume.unsupported_reason(PromptDeliveryKind::Stdin),
            "The one-shot stdin adapter has no durable external session identity."
        );
        assert_eq!(
            Capability::Resume.unsupported_reason(PromptDeliveryKind::PromptFile),
            "The one-shot prompt-file adapter does not retain a durable external session identity."
        );
    }

    /// `stop` is outside the narrowable set entirely (`:8-17` vs `shared/types.ts:1698`).
    #[test]
    fn stop_is_not_a_narrowable_capability() {
        assert!(Capability::ALL.iter().all(|c| c.wire() != "stop"));
        assert_eq!(
            parse_capability_narrowing(
                Some(&serde_json::json!({"stop": false})),
                "runner capabilities"
            )
            .unwrap_err(),
            "runner capabilities has unsupported fields: stop."
        );
    }

    /// The narrowing parses into a SET — there is nowhere for a `true` to live.
    #[test]
    fn narrowing_parses_to_a_set_of_capabilities() {
        let parsed = parse_capability_narrowing(
            Some(&serde_json::json!({"steer": false, "resume": false})),
            "runner capabilities",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            parsed,
            [Capability::Steer, Capability::Resume]
                .into_iter()
                .collect::<ExternalCliCapabilityNarrowing>()
        );
        assert_eq!(
            parse_capability_narrowing(
                Some(&serde_json::json!({"steer": true})),
                "runner capabilities"
            )
            .unwrap_err(),
            "runner capabilities.steer may only be false; user config cannot widen code-owned \
             external adapter capabilities."
        );
        assert_eq!(
            parse_capability_narrowing(Some(&serde_json::json!([])), "runner capabilities")
                .unwrap_err(),
            "runner capabilities must be an object."
        );
        assert_eq!(
            parse_capability_narrowing(None, "runner capabilities").unwrap(),
            None
        );
    }

    /// An adapter id persists as its wire string, so a receipt or a hop-2 `runner-config.json`
    /// round-trips through the same spelling upstream writes.
    #[test]
    fn an_adapter_id_serializes_as_its_wire_string() {
        for id in AdapterId::ALL {
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, format!("\"{}\"", id.wire()));
            assert_eq!(serde_json::from_str::<AdapterId>(&json).unwrap(), id);
        }
        assert!(serde_json::from_str::<AdapterId>("\"grok-build\"").is_err());
        for capability in Capability::ALL {
            let json = serde_json::to_string(&capability).unwrap();
            assert_eq!(json, format!("\"{}\"", capability.wire()));
            assert_eq!(
                serde_json::from_str::<Capability>(&json).unwrap(),
                capability
            );
        }
    }
}
