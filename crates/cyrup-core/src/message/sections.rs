//! [`Sections`] — the named, ORDERED prompt sections carried by [`crate::Message::System`]
//! (PROV-083a).

/// Pi `SystemMessage.sections?: Record<string, string | null>`
/// (`packages/ai/src/types.ts:496-501` @v0.87.1).
///
/// Named prompt sections rendered verbatim after `content`. The leading system message declares
/// them; a later system message replaces a section by name, and a JSON `null` (`None` here)
/// removes one.
///
/// # Why an ordered `Vec` of pairs and not a `HashMap`
///
/// The ORDER of the entries is observable: `getSystemMessageText`
/// (`packages/ai/src/utils/text.ts:15-21`) renders the values in iteration order joined with
/// `"\n\n"`, and `renderSystemMessageUpdate` (`:28`) frames each change in iteration order too.
/// Upstream's iteration order is JS object/`Map` insertion order, which the docblock relies on
/// explicitly — *"Keep each section self-delimiting (a tag, a heading) so the model can relate an
/// update to the original. Avoid integer-like names; JSON objects reorder those."*
///
/// A `HashMap` would make the rendered prompt nondeterministic run to run, and a `BTreeMap` (what
/// `serde_json::Map` is without the `preserve_order` feature, which this workspace does not
/// enable) would silently re-sort it alphabetically. Neither `indexmap` nor a `preserve_order`
/// `serde_json` is a workspace dependency, so the order-preserving container is spelled out here:
/// a `Vec` of `(name, value)` pairs with JS `Map` semantics — [`Sections::set`] keeps an existing
/// name in its original position, a new name appends, and [`Sections::remove`] drops it.
///
/// Serialization is a JSON object in exactly this order; deserialization preserves the order the
/// keys appear on the wire (a `MapAccess` walk, not a sorted intermediate map).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sections(Vec<(String, Option<String>)>);

impl Sections {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Entries in wire/insertion order. `None` is pi's `null` — "remove this section".
    pub fn iter(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.0
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_ref().map(String::as_str)))
    }

    /// The values in order, skipping the `null` (removal) entries — what
    /// `Object.values(message.sections ?? {})` yields after `text !== null` filtering in
    /// `getSystemMessageText` (`utils/text.ts:17-19`).
    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.0.iter().filter_map(|(_, v)| v.as_deref())
    }

    /// `Map.prototype.get`. An existing name mapped to `null` yields `Some(None)`; an absent name
    /// yields `None`.
    pub fn get(&self, name: &str) -> Option<Option<&str>> {
        self.0
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_deref())
    }

    /// `Map.prototype.set` — an existing name keeps its POSITION and takes the new value; a new
    /// name is appended.
    pub fn set(&mut self, name: impl Into<String>, value: Option<String>) {
        let name = name.into();
        match self.0.iter_mut().find(|(k, _)| *k == name) {
            Some(slot) => slot.1 = value,
            None => self.0.push((name, value)),
        }
    }

    /// `Map.prototype.delete`.
    pub fn remove(&mut self, name: &str) -> bool {
        match self.0.iter().position(|(k, _)| k == name) {
            Some(i) => {
                self.0.remove(i);
                true
            }
            None => false,
        }
    }
}

impl<K: Into<String>> FromIterator<(K, Option<String>)> for Sections {
    /// Later duplicates patch the earlier entry in place, exactly as repeated `Map.set` does.
    fn from_iter<I: IntoIterator<Item = (K, Option<String>)>>(iter: I) -> Self {
        let mut out = Self::new();
        for (k, v) in iter {
            out.set(k, v);
        }
        out
    }
}

impl serde::Serialize for Sections {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in &self.0 {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

impl<'de> serde::Deserialize<'de> for Sections {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Sections;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map of section name to string or null")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Sections, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                // Walked in WIRE order and appended in that order — the whole reason this impl is
                // hand-written instead of going through `serde_json::Map` (a `BTreeMap` here,
                // which would alphabetize the sections and change the rendered prompt).
                let mut out = Sections::new();
                while let Some((k, v)) = access.next_entry::<String, Option<String>>()? {
                    out.set(k, v);
                }
                Ok(out)
            }
        }
        deserializer.deserialize_map(V)
    }
}
