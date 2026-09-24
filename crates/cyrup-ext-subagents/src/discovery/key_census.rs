//! The key census: which keys of a raw JSON object this port reads, which upstream declares but
//! this port has not ported, and which are neither.
//!
//! Every subagent config surface is deserialized by serde with no `deny_unknown_fields`, because a
//! strict parse would let one unexpected key take the whole file down. The price of that
//! tolerance is that a key serde does not recognise is dropped without a word — a typo'd
//! `authorityPolicy` action, a pi-authored `agentOverrides.<name>.machine`, an upstream
//! `config.json` key this port never implemented. This module is the one place that turns that
//! silence into a diagnostic: a caller hands it the raw object, the keys it reads, and the
//! upstream keys it knows it does not implement, and gets back what to say about the rest.
//!
//! Three callers share it: the `config.json` loader (`crates/cyrup/src/subagent_config.rs`, via
//! [`crate::registration::SubagentExtensionConfig::config_warnings`]), the settings parser's
//! `agentOverrides.<name>` walk ([`crate::discovery::parse_subagent_settings`]), and the runtime
//! agent registry's unknown-field and unrepresentable-field refusals
//! ([`crate::discovery::runtime_registry`]).

use serde_json::{Map, Value};

/// The result of [`census`]: the keys of one raw object that no reader will consume.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KeyCensus<'a> {
    /// Present keys that are neither read by this port nor declared-and-unported upstream, in
    /// the object's iteration order (sorted, since `serde_json` is built without
    /// `preserve_order`).
    pub unknown: Vec<&'a str>,
    /// Present keys that upstream declares and this port does not implement, each with the
    /// landing it is waiting on, in the ORDER OF THE UNPORTED TABLE — so a caller that refuses the
    /// first one refuses the same key every time.
    pub unported: Vec<(&'static str, &'static str)>,
}

impl KeyCensus<'_> {
    /// True iff every present key is read.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.unknown.is_empty() && self.unported.is_empty()
    }
}

/// Classify every key of `raw` against `known` (keys a reader consumes) and `unported`
/// (`(key, landing)`: keys upstream declares that this port has no reader for).
///
/// A key listed in BOTH is unported: listing it as known would be the silent drop this module
/// exists to prevent, and the runtime registry deliberately validates its unrepresentable keys
/// (so they sit in its supported list) before refusing them.
#[must_use]
pub fn census<'a>(
    raw: &'a Map<String, Value>,
    known: &[&str],
    unported: &[(&'static str, &'static str)],
) -> KeyCensus<'a> {
    let unported_present: Vec<(&'static str, &'static str)> = unported
        .iter()
        .copied()
        .filter(|(key, _)| raw.contains_key(*key))
        .collect();
    let unknown = raw
        .keys()
        .map(String::as_str)
        .filter(|key| !known.contains(key) && !unported.iter().any(|(u, _)| u == key))
        .collect();
    KeyCensus {
        unknown,
        unported: unported_present,
    }
}

/// The exact JSON field names serde's derived `Deserialize` for `T` accepts, read from the
/// `fields` list the derive hands to `deserialize_struct` — so a "known keys" list can never drift
/// from the struct it describes. `#[serde(skip)]`/`skip_deserializing` fields are absent, exactly
/// as they are absent from what serde reads; renames and `rename_all` are applied.
///
/// Returns an empty slice for a `T` whose `Deserialize` is not a derived struct.
#[must_use]
pub fn struct_fields<T: serde::de::DeserializeOwned>() -> &'static [&'static str] {
    let mut fields: &'static [&'static str] = &[];
    let _ = T::deserialize(FieldProbe {
        fields: &mut fields,
    });
    fields
}

/// A `Deserializer` that records the field list a derived struct asks for and then fails.
struct FieldProbe<'f> {
    fields: &'f mut &'static [&'static str],
}

#[derive(Debug)]
struct ProbeDone;

impl std::fmt::Display for ProbeDone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("field probe")
    }
}

impl std::error::Error for ProbeDone {}

impl serde::de::Error for ProbeDone {
    fn custom<T: std::fmt::Display>(_msg: T) -> Self {
        ProbeDone
    }
}

impl<'de> serde::Deserializer<'de> for FieldProbe<'_> {
    type Error = ProbeDone;

    fn deserialize_any<V: serde::de::Visitor<'de>>(
        self,
        _visitor: V,
    ) -> Result<V::Value, ProbeDone> {
        Err(ProbeDone)
    }

    fn deserialize_struct<V: serde::de::Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, ProbeDone> {
        *self.fields = fields;
        Err(ProbeDone)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct map enum identifier
        ignored_any
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn obj(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    /// Mutation killed: dropping the `unported` exclusion from the unknown filter reports an
    /// unported key twice (once as unknown); dropping the known filter reports every key.
    #[test]
    fn census_splits_known_unported_and_unknown() {
        let raw = obj(serde_json::json!({"a": 1, "machine": "m", "typo": true}));
        let c = census(&raw, &["a"], &[("machine", "herdr placement")]);
        assert_eq!(c.unknown, vec!["typo"]);
        assert_eq!(c.unported, vec![("machine", "herdr placement")]);
        assert!(!c.is_clean());
        let clean = obj(serde_json::json!({"a": 1}));
        assert!(census(&clean, &["a"], &[("machine", "x")]).is_clean());
    }

    /// Unported entries come back in TABLE order, not object order, so a refusal is stable.
    #[test]
    fn unported_keys_come_back_in_table_order() {
        let raw = obj(serde_json::json!({"b": 1, "a": 1}));
        let c = census(&raw, &[], &[("b", "B"), ("a", "A")]);
        assert_eq!(c.unported, vec![("b", "B"), ("a", "A")]);
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    struct Probe {
        some_field: u8,
        #[serde(skip)]
        skipped: u8,
        #[serde(rename = "renamed")]
        other: u8,
    }

    /// The probe reads the derive's own list: renames applied, skipped fields absent.
    #[test]
    fn struct_fields_reads_the_derived_field_list() {
        assert_eq!(struct_fields::<Probe>(), &["someField", "renamed"]);
        assert!(struct_fields::<u8>().is_empty());
    }
}
