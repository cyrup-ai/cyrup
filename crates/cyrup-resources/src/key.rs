//! Normalized resource name key + collision rules (arch-09 §2, key.rs).
//!
//! A [`ResourceKey`] is the case-insensitive, trimmed identity of a resource by name. Two
//! resources with the same key in different scopes collide; the lower-ranked scope wins per Pi's
//! `resourcePrecedenceRank` ([`crate::scope::ResourceScope::precedence_rank`]).

/// Normalized resource name (lower-cased, trimmed). Used as the map key for same-name precedence.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct ResourceKey(String);

impl ResourceKey {
    /// Normalize an arbitrary name into a key. Trims surrounding whitespace and lower-cases so
    /// `Review`, `review`, and ` review ` collide deterministically.
    pub fn normalize(name: &str) -> Self {
        Self(name.trim().to_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True if the key is empty after normalization (an invalid name).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for ResourceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Debug for ResourceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ResourceKey({:?})", self.0)
    }
}

impl From<&str> for ResourceKey {
    fn from(s: &str) -> Self {
        Self::normalize(s)
    }
}
