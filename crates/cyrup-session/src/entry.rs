//! Tree entry model (arch-04 §3.1/§4.1). On-disk discriminants and field names are
//! Pi-interoperable (R-00-013): `type` is snake_case, payload fields camelCase.
//!
//! Unknown / future `type` values round-trip verbatim via [`Entry::Unknown`] (R-04-007). The
//! `serde(tag=…)` + untagged-fallback shape is not expressible with `serde_derive`, so [`Entry`]
//! hand-implements `Serialize`/`Deserialize` and delegates known variants to [`KnownEntry`].

use cyrup_core::{EntryId, Message, ModelId, ProviderId};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

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
}

/// The set of entry types cyrup interprets. Tags are snake_case (`message`, `model_change`, …);
/// payload fields are camelCase (`modelId`, `firstKeptEntryId`, …), matching Pi.
// `Message` dominates allocations and is serde-`flatten`ed; boxing it would force `box`-patterns
// (unstable) at every match site — same rationale as `Entry` below.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum KnownEntry {
    Message {
        #[serde(flatten)]
        base: EntryBase,
        message: Message,
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
        first_kept_entry_id: EntryId,
        tokens_before: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_hook: Option<bool>,
    },
    BranchSummary {
        #[serde(flatten)]
        base: EntryBase,
        from_id: EntryId,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
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
    CustomMessage {
        #[serde(flatten)]
        base: EntryBase,
        custom_type: String,
        /// `string | (Text|Image)[]` — mirrored as raw JSON to match Pi exactly.
        content: Value,
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
            | KnownEntry::SessionInfo { base, .. } => base,
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
            | KnownEntry::SessionInfo { base, .. } => base,
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
            Entry::Unknown(v) => {
                v.get("parentId").and_then(Value::as_str).map(EntryId::from)
            }
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
