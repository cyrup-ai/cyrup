//! An **insertion-order-preserving** JSON-ish value (`OrderedValue`). The policy engine's
//! last-match-wins semantics depend on the *document order* of pattern keys within a category (pi
//! iterates `Object.entries`, which is insertion order — `permission-manager.ts:363`). `serde_json`'s
//! default `Map` is a `BTreeMap` that re-sorts keys alphabetically, which would silently change which
//! pattern wins (e.g. `{ "git *": allow, "*": ask }` intends `*` to win by being LAST; sorted it
//! becomes `*, git *` and `git *` wins instead). So both policy sources — JSONC (`jsonc.rs`) and the
//! YAML frontmatter (`common.rs`) — parse into this type, which keeps entries in the exact order they
//! appear. `serde_json`'s `Deserializer` drives our `Visitor` in document order regardless of how a
//! `Value` would later store it, so deserializing directly here captures the true order.

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// An order-preserving value. Only the shapes the permission config uses are modeled richly
/// (strings + nested objects); everything else (arrays, numbers, bools, null) collapses to
/// [`OrderedValue::Other`] since the engine never reads non-string, non-object policy values.
#[derive(Debug, Clone, PartialEq)]
pub enum OrderedValue {
    Str(String),
    Object(Vec<(String, OrderedValue)>),
    Other,
}

impl OrderedValue {
    /// The string payload, if this is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            OrderedValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// The ordered entries, if this is an object.
    #[must_use]
    pub fn as_object(&self) -> Option<&[(String, OrderedValue)]> {
        match self {
            OrderedValue::Object(entries) => Some(entries.as_slice()),
            _ => None,
        }
    }

    /// The value for `key` — the LAST match (JS object-key semantics: a duplicate key overwrites).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&OrderedValue> {
        match self {
            OrderedValue::Object(entries) => {
                entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
            }
            _ => None,
        }
    }

    /// An empty object (the "absent / not-an-object" default the engine treats as no rules).
    #[must_use]
    pub fn empty_object() -> Self {
        OrderedValue::Object(Vec::new())
    }
}

impl<'de> Deserialize<'de> for OrderedValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(OrderedValueVisitor)
    }
}

struct OrderedValueVisitor;

impl<'de> Visitor<'de> for OrderedValueVisitor {
    type Value = OrderedValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
        Ok(OrderedValue::Str(v.to_string()))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
        Ok(OrderedValue::Str(v))
    }

    fn visit_bool<E>(self, _v: bool) -> Result<Self::Value, E> {
        Ok(OrderedValue::Other)
    }

    fn visit_i64<E>(self, _v: i64) -> Result<Self::Value, E> {
        Ok(OrderedValue::Other)
    }

    fn visit_u64<E>(self, _v: u64) -> Result<Self::Value, E> {
        Ok(OrderedValue::Other)
    }

    fn visit_f64<E>(self, _v: f64) -> Result<Self::Value, E> {
        Ok(OrderedValue::Other)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(OrderedValue::Other)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(OrderedValue::Other)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        // Drain the sequence (arrays are irrelevant to policy) so the parser stays well-formed.
        while seq.next_element::<OrderedValue>()?.is_some() {}
        Ok(OrderedValue::Other)
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut entries = Vec::new();
        while let Some((k, v)) = map.next_entry::<String, OrderedValue>()? {
            entries.push((k, v));
        }
        Ok(OrderedValue::Object(entries))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn preserves_object_key_order() {
        let v: OrderedValue = serde_json::from_str(r#"{"git *":"allow","*":"ask"}"#).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj[0].0, "git *");
        assert_eq!(obj[1].0, "*");
    }

    #[test]
    fn nested_object_order_preserved() {
        let v: OrderedValue =
            serde_json::from_str(r#"{"bash":{"z":"deny","a":"allow"}}"#).unwrap();
        let bash = v.get("bash").unwrap().as_object().unwrap();
        assert_eq!(bash[0].0, "z");
        assert_eq!(bash[1].0, "a");
    }
}
