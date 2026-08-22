//! The structured text-signature payload carried in a text block's `text_signature`
//! ([`TextPhase`], [`TextSignatureV1`]; Pi types.ts:316-320).

/// The reasoning phase of a structured text signature (Pi `TextSignatureV1.phase`, types.ts:319):
/// distinguishes commentary from the final answer for OpenAI-responses replay.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextPhase {
    Commentary,
    FinalAnswer,
}

/// Structured text-signature payload (Pi `TextSignatureV1`, types.ts:316-320). A
/// [`crate::Content::Text`]'s `text_signature` is EITHER a legacy opaque id string OR a JSON-encoded
/// `TextSignatureV1`; use [`TextSignatureV1::parse`]/[`TextSignatureV1::encode`] to round-trip the
/// structured form through that string field.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSignatureV1 {
    /// Schema version — always `1` (Pi `v: 1`). Private so the invariant is enforced by
    /// [`TextSignatureV1::new`] rather than merely documented; serde still reads/writes it, so the
    /// wire form is unchanged. Read it via [`TextSignatureV1::version`].
    v: u8,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub phase: Option<TextPhase>,
}

impl TextSignatureV1 {
    /// Build a V1 signature (`v` fixed to 1).
    pub fn new(id: impl Into<String>, phase: Option<TextPhase>) -> Self {
        Self { v: 1, id: id.into(), phase }
    }

    /// The schema version held by this signature — always `1` for values built by
    /// [`TextSignatureV1::new`] or accepted by [`TextSignatureV1::parse`].
    pub fn version(&self) -> u8 {
        self.v
    }

    /// Parse a structured V1 signature from a `text_signature` string, or `None` for a legacy id
    /// string / non-V1 JSON (Pi reads `textSignature` as `legacy id string or TextSignatureV1 JSON`,
    /// types.ts:325).
    pub fn parse(text_signature: &str) -> Option<Self> {
        let parsed: TextSignatureV1 = serde_json::from_str(text_signature).ok()?;
        (parsed.v == 1).then_some(parsed)
    }

    /// Encode to the JSON string stored in `text_signature` (never panics; falls back to the id).
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| self.id.clone())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn text_signature_v1_roundtrips_through_string_field() {
        let sig = TextSignatureV1::new("resp_123", Some(TextPhase::FinalAnswer));
        let encoded = sig.encode();
        // Encodes camelCase + snake_case phase, v:1.
        let v: serde_json::Value = serde_json::from_str(&encoded).expect("json");
        assert_eq!(v["v"], 1);
        assert_eq!(v["id"], "resp_123");
        assert_eq!(v["phase"], "final_answer");
        // Parses back; a legacy id string yields None.
        assert_eq!(TextSignatureV1::parse(&encoded), Some(sig));
        assert_eq!(TextSignatureV1::parse("legacy-opaque-id"), None);
    }
}
