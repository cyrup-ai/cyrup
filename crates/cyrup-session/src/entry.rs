//! Tree entry model (arch-04 §3.1/§4.1). On-disk discriminants and field names are
//! Pi-interoperable (R-00-013): `type` is snake_case, payload fields camelCase.
//!
//! Unknown / future `type` values round-trip verbatim via [`Entry::Unknown`] (R-04-007). The
//! `serde(tag=…)` + untagged-fallback shape is not expressible with `serde_derive`, so [`Entry`]
//! hand-implements `Serialize`/`Deserialize` and delegates known variants to [`KnownEntry`].

use cyrup_core::{Content, EntryId, ModelId, ProviderId, SystemMessage, Usage};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::agent_message::AgentMessage;

/// Fields shared by every tree entry; flattened into each variant on the wire.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryBase {
    /// Short stable 8-hex token.
    pub id: EntryId,
    /// `None` == JSON `null` == first/root entry.
    #[serde(default)]
    pub parent_id: Option<EntryId>,
    /// RFC3339 timestamp.
    pub timestamp: String,
    /// Every top-level key cyrup does not model, preserved verbatim and re-emitted on write.
    ///
    /// Pi's reader is a bare `JSON.parse` with a blank/malformed→null guard and NO schema check
    /// (`parseSessionEntryLine`, `session-manager.ts:503-511`), so unknown keys survive any
    /// rewrite. cyrup's typed variants dropped them: a session annotated by a newer cyrup, a fork
    /// or an extension lost those annotations the first time this cyrup rewrote the file
    /// (migration rewrite, `SessionManager::branch_to_file`, export). `Entry::Unknown` already
    /// preserved the whole object for UNKNOWN tags (`entry.rs`'s `Err(_) => Entry::Unknown(v)`);
    /// this closes the same hole for KNOWN ones.
    ///
    /// Serde applies flattened maps last, so this receives exactly the keys neither the variant's
    /// own fields nor `id`/`parentId`/`timestamp` claimed. The internally-tagged `type`
    /// discriminant is consumed by the enum before the content is handed to the variant, so it
    /// never lands here and is never emitted twice.
    ///
    /// Standing caveat: keys nested INSIDE `AgentMessage`/`Content` are still not enumerated.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// The set of entry types cyrup interprets. Tags are snake_case (`message`, `model_change`, …);
/// payload fields are camelCase (`modelId`, `firstKeptEntryId`, …), matching Pi.
// `Message` dominates allocations and is serde-`flatten`ed; boxing it would force `box`-patterns
// (unstable) at every match site — same rationale as `Entry` below.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum KnownEntry {
    Message {
        #[serde(flatten)]
        base: EntryBase,
        /// The full Pi `AgentMessage` union (`user`/`assistant`/`toolResult` plus the
        /// `bashExecution`/`custom`/`branchSummary`/`compactionSummary` roles), so those entries
        /// parse instead of degrading to [`Entry::Unknown`] and being dropped from context (Pi
        /// `session-manager.ts:954`; Pi parses session files without validation, so a fork or a
        /// hand-edited file carrying a summary role here is read, not discarded).
        message: AgentMessage,
    },
    ModelChange {
        #[serde(flatten)]
        base: EntryBase,
        provider: ProviderId,
        model_id: ModelId,
    },
    ThinkingLevelChange {
        #[serde(flatten)]
        base: EntryBase,
        thinking_level: String,
    },
    Compaction {
        #[serde(flatten)]
        base: EntryBase,
        summary: String,
        /// The first entry KEPT by this compaction, or `None` when it could not be resolved.
        ///
        /// Pi declares this `string` (`session-manager.ts:72`) but leaves it genuinely `undefined`
        /// on one live path: `migrateV1ToV2` (`session-manager.ts:245-255`) deletes a v1 entry's
        /// `firstKeptEntryIndex` UNCONDITIONALLY and only assigns `firstKeptEntryId` when the
        /// index resolves to a non-`session` entry — so index `0` (the session header) or an
        /// out-of-range index leaves the key absent. Pi parses session JSONL without validation, so
        /// the entry stays a compaction and `buildContextEntries`' `entry.id === firstKeptEntryId`
        /// test simply never matches (`session-manager.ts:445`): NOTHING before the compaction is
        /// re-admitted. The harness fork states the same contract explicitly —
        /// `firstKeptEntryId?: string` (`agent/src/harness/types.ts:406`) guarded by
        /// `if (compaction.firstKeptEntryId)` (`agent/src/harness/session/session.ts:80`).
        ///
        /// Declaring it non-optional made `from_value::<KnownEntry>` FAIL on such an entry,
        /// demoting it to [`Entry::Unknown`] — which contributes no summary AND defeats
        /// `latest_compaction`, re-admitting the entire pre-compaction history. Optional here is a
        /// read-time reinterpretation only; the JSONL is never rewritten.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_kept_entry_id: Option<EntryId>,
        tokens_before: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        /// Token spend of the LLM call(s) that produced `summary` (Pi `CompactionEntry.usage`,
        /// `session-manager.ts:69-80`). On a split turn Pi records the SUM of the history call and
        /// the turn-prefix call (`combineUsage`, `compaction.ts:877`). Optional and
        /// `skip_serializing_if`-elided so a hook-supplied compaction that reports no usage stays
        /// byte-identical to Pi's, and so a Pi-written entry that carries one round-trips (R-00-013).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_hook: Option<bool>,
        /// The complete prompt and tool state at this compaction boundary (Pi
        /// `CompactionEntry.systemMessage`, `session-manager.ts:103` @v1.0.1).
        ///
        /// Compaction drops the entries before the first kept one, and `buildContextEntries` drops
        /// every system message from the kept range as well (`:506`), so this is the ONLY carrier of
        /// the loadout across a compaction: the projection of the compaction entry is this message
        /// followed by the summary (`sessionEntryToContextMessages`, `:462-463`). Absent on an entry
        /// written before the transcript carried system messages, and when the replayed state was
        /// empty.
        ///
        /// Written in the order of a REPLAYED snapshot (`toolsAdded` before `timestamp`), which is
        /// how pi writes it (`session-manager.ts:1270-1283` @v1.0.0 spreads `getCurrentSystemMessage`'s
        /// result), and not in the order of a `message` entry's system row. See
        /// [`SystemMessage::serialize_replayed`].
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            serialize_with = "SystemMessage::serialize_replayed_opt"
        )]
        system_message: Option<SystemMessage>,
    },
    BranchSummary {
        #[serde(flatten)]
        base: EntryBase,
        from_id: EntryId,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        /// Token spend of the branch-summarization call (Pi `BranchSummaryEntry.usage`,
        /// `session-manager.ts:88-89`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_hook: Option<bool>,
    },
    Custom {
        #[serde(flatten)]
        base: EntryBase,
        custom_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    /// Model-attributed token spend that does NOT participate in LLM context — Pi `UsageEntry`
    /// (`session-manager.ts:80-89` @v1.0.4), written by `appendUsage`
    /// (`session-manager.ts:1244-1259`). `kind` is Pi's "arbitrary usage category, such as
    /// `cache_warm`"; `note` its "optional human-readable qualifier for usage notices".
    ///
    /// **Context-invisible, but a link in the chain.** Pi's `_appendEntry` sets
    /// `this.leafId = entry.id` for EVERY entry (`session-manager.ts:1191-1196`), so a usage entry
    /// advances the branch leaf and every backwards walk traverses it — while
    /// `sessionEntryToContextMessages` has no `usage` arm and falls through to `return []`
    /// (`:439-465`), so it projects no message, costs 0 tokens in the compaction budget walk, and
    /// is FOLDED INTO the kept region by the cut-point back-scan.
    ///
    /// Both of those behaviours arrive here through CATCH-ALLS — [`crate::context::push_as_raw`]'s
    /// and [`crate::context::context_message_role`]'s `_ =>`, and
    /// `crate::compaction::cutpoint`'s `is_context_visible` test. Nothing will make a future
    /// reviewer notice if one of them grows an arm by accident, so the invisibility is pinned by
    /// `tests::usage_entry::sess051_usage_entry_is_context_invisible_and_folded_by_the_back_scan`.
    ///
    /// Field order is the WIRE order Pi writes (`kind, provider, model, usage, note`), with `base`
    /// first so `type,id,parentId,timestamp` stay ahead of the payload. `note` is elided when
    /// absent because Pi spreads it in conditionally — `...(note ? { note } : {})`, so an empty
    /// string is ABSENT rather than `null`.
    Usage {
        #[serde(flatten)]
        base: EntryBase,
        kind: String,
        provider: ProviderId,
        model: ModelId,
        /// REQUIRED on the wire: Pi always writes it.
        usage: Usage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    CustomMessage {
        #[serde(flatten)]
        base: EntryBase,
        custom_type: String,
        /// `string | (Text|Image)[]` — mirrored as raw JSON to match Pi exactly.
        ///
        /// `#[serde(default)]` mirrors Pi's `entry.content ?? []`
        /// (`session-manager.ts:396-399`): an ABSENT `content` must not fail the whole entry into
        /// [`Entry::Unknown`], where it contributes nothing to context. The resulting
        /// [`Value::Null`] is normalized to zero content blocks by
        /// [`crate::agent_message::custom_to_message`], not rendered as the literal text `null`.
        #[serde(default)]
        content: Value,
        /// Pi passes `entry.display` straight through to `createCustomMessage`
        /// (`session-manager.ts:396-399`), where an absent key is simply falsy — it is never
        /// validated (`messages.ts:123-136`). Without `#[serde(default)]` an entry lacking the key
        /// failed `from_value::<KnownEntry>` and was demoted to [`Entry::Unknown`], silently
        /// vanishing from context.
        #[serde(default)]
        display: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
    Label {
        #[serde(flatten)]
        base: EntryBase,
        target_id: EntryId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    SessionInfo {
        #[serde(flatten)]
        base: EntryBase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// An append-only edit to one EARLIER entry's contribution to model context — Pi
    /// `ContextEditEntry` (`session-manager.ts:174-180` @v0.87.1).
    ///
    /// `replacement: null` OMITS the target from context entirely; a value replaces only its
    /// content, leaving every other field of the target's message untouched. The edit entry itself
    /// projects NO message (it is a known non-message kind, so
    /// `crate::context::push_as_raw`'s catch-all skips it), and only edits admitted by
    /// `build_context_entries` apply — an edit outside the live compaction window cannot resurrect
    /// or alter anything.
    ///
    /// Pi writes these on the DEFAULT path, not only from extensions: `_omitRecoveryAttempt`
    /// (`agent-session.ts:1015-1031` @v0.87.1) appends `appendContextEdit(targetId, null)` for an
    /// abandoned assistant attempt and for each of its tool results.
    ///
    /// CYRUP-DELTA: cyrup never WRITES one — there is no `append_context_edit` counterpart to Pi
    /// `appendContextEdit` (`session-manager.ts:1358-1395` @v0.87.1); that follows with EXT-078.
    /// Reading and projecting them is what this variant is for, so a pi-written session no longer
    /// degrades the edit to [`Entry::Unknown`] and silently projects the target UNEDITED. Because
    /// cyrup only reads, the string→text-block normalisation Pi performs at WRITE time is done
    /// defensively in the projection instead. See `session-manager.ts:174-180, 519-566 @v0.87.1`.
    ContextEdit {
        #[serde(flatten)]
        base: EntryBase,
        target_id: EntryId,
        /// REQUIRED on the wire, with `null` as a meaningful value (omit the target from context) —
        /// deliberately NOT `#[serde(default)]`: an absent key is not something Pi writes, and
        /// conflating it with an explicit `null` would lose the omit signal.
        replacement: Option<ContextEditReplacement>,
    },
}

/// The `{ content }` payload of a non-omitting [`KnownEntry::ContextEdit`] — Pi
/// `ContextEditEntry["replacement"]` (`session-manager.ts:177-179` @v0.87.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEditReplacement {
    pub content: ContextEditableContent,
}

/// Content an append-only context edit may substitute without changing message metadata — Pi
/// `ContextEditableContent` (`session-manager.ts:167-171` @v0.87.1), the union of the `user`,
/// `assistant`, `toolResult` and `custom` message content types. On the wire that is a bare string
/// or a block array, so this is `#[serde(untagged)]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContextEditableContent {
    Text(String),
    Blocks(Vec<Content>),
}

/// The wire tags cyrup recognizes. Anything else → [`Entry::Unknown`].
const KNOWN_TYPES: &[&str] = &[
    "message",
    "model_change",
    "thinking_level_change",
    "compaction",
    "branch_summary",
    "custom",
    "custom_message",
    "label",
    "session_info",
    "context_edit",
    "usage",
];

impl KnownEntry {
    pub fn base(&self) -> &EntryBase {
        match self {
            KnownEntry::Message { base, .. }
            | KnownEntry::ModelChange { base, .. }
            | KnownEntry::ThinkingLevelChange { base, .. }
            | KnownEntry::Compaction { base, .. }
            | KnownEntry::BranchSummary { base, .. }
            | KnownEntry::Custom { base, .. }
            | KnownEntry::CustomMessage { base, .. }
            | KnownEntry::Label { base, .. }
            | KnownEntry::SessionInfo { base, .. }
            | KnownEntry::ContextEdit { base, .. }
            | KnownEntry::Usage { base, .. } => base,
        }
    }

    pub fn base_mut(&mut self) -> &mut EntryBase {
        match self {
            KnownEntry::Message { base, .. }
            | KnownEntry::ModelChange { base, .. }
            | KnownEntry::ThinkingLevelChange { base, .. }
            | KnownEntry::Compaction { base, .. }
            | KnownEntry::BranchSummary { base, .. }
            | KnownEntry::Custom { base, .. }
            | KnownEntry::CustomMessage { base, .. }
            | KnownEntry::Label { base, .. }
            | KnownEntry::SessionInfo { base, .. }
            | KnownEntry::ContextEdit { base, .. }
            | KnownEntry::Usage { base, .. } => base,
        }
    }
}

/// A tree node (lines 2+). Known variants are interpreted; unknown `type` values are preserved
/// verbatim for forward-compatibility (R-04-007).
// `Known` is intentionally inline (no `Box`): boxing would force `box`-patterns at every match
// site (unstable) for a payload that dominates allocations anyway.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Entry {
    Known(KnownEntry),
    /// Verbatim JSON for an unrecognized `type` (or a known-tag-but-unparseable body); round-trips
    /// byte-faithfully so a Pi session with extension entries survives load+save.
    Unknown(Value),
}

impl Entry {
    /// Convenience constructor for an interpreted entry.
    pub fn known(k: KnownEntry) -> Self {
        Entry::Known(k)
    }

    /// `&EntryBase` for interpreted entries; `None` for `Unknown` (read leniently via accessors).
    pub fn base(&self) -> Option<&EntryBase> {
        match self {
            Entry::Known(k) => Some(k.base()),
            Entry::Unknown(_) => None,
        }
    }

    pub fn base_mut(&mut self) -> Option<&mut EntryBase> {
        match self {
            Entry::Known(k) => Some(k.base_mut()),
            Entry::Unknown(_) => None,
        }
    }

    /// The entry id. For `Unknown` it reads `id` from the raw JSON, synthesizing a stable
    /// placeholder if absent (so indexing never panics — arch-04 §8).
    pub fn id(&self) -> EntryId {
        match self {
            Entry::Known(k) => k.base().id.clone(),
            Entry::Unknown(v) => v
                .get("id")
                .and_then(Value::as_str)
                .map(EntryId::from)
                .unwrap_or_else(|| synth_id(v)),
        }
    }

    /// The parent id (`None` for a root entry).
    pub fn parent_id(&self) -> Option<EntryId> {
        match self {
            Entry::Known(k) => k.base().parent_id.clone(),
            Entry::Unknown(v) => v.get("parentId").and_then(Value::as_str).map(EntryId::from),
        }
    }

    /// The wire `type` tag.
    pub fn type_tag(&self) -> Option<String> {
        match self {
            Entry::Known(k) => serde_json::to_value(k)
                .ok()
                .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string)),
            Entry::Unknown(v) => v.get("type").and_then(Value::as_str).map(str::to_string),
        }
    }

    /// Serialize this entry to a single JSONL line (no trailing newline).
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Serialize for Entry {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Entry::Known(k) => k.serialize(s),
            Entry::Unknown(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let v = Value::deserialize(d)?;
        let is_known = v
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| KNOWN_TYPES.contains(&t));
        if is_known {
            // Known tag: parse strictly, but fall back to verbatim preservation if the body does
            // not fit our schema (keeps the file lossless rather than erroring).
            match serde_json::from_value::<KnownEntry>(v.clone()) {
                Ok(k) => Ok(Entry::Known(k)),
                Err(_) => Ok(Entry::Unknown(v)),
            }
        } else if v.is_object() {
            Ok(Entry::Unknown(v))
        } else {
            Err(D::Error::custom("session entry must be a JSON object"))
        }
    }
}

/// Stable FNV-1a-derived 8-hex id for an `Unknown` entry that lacks one, so it can be indexed
/// without colliding (in practice) with minted ids and without mutating the preserved bytes.
fn synth_id(v: &Value) -> EntryId {
    let s = v.to_string();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    EntryId::from(format!("{:08x}", (h & 0xffff_ffff)))
}
