//! [`Bounded<N>`] / [`BoundedUtf16<N>`] — the crate's ONE rejecting bounded-string helper family
//! (SCOPE_3 §A.4).
//!
//! Upstream carries nine independent bounded-string helpers that split into exactly two
//! behaviours: **reject** when over the limit (`bounded`, `workflow-child-summary.ts:41`;
//! `normalizeDisplayString`, `workflow-preflight.ts:50`; `boundedNonEmptyString`,
//! `lane-metadata.ts:20`; `boundedString`, `parallel-handoff.ts:212`) and **truncate**
//! (`boundedText`, `host-command.ts:45`; `text`, `workflow-checklist.ts:118`). This module is the
//! rejecting family; the truncating family is a pair of free functions that land with SCOPE_3e
//! beside it — a truncated value has no invariant worth carrying, so it never gets a type.
//!
//! Upstream also disagrees on the *unit*: most limits are UTF-8 **bytes** (`Buffer.byteLength`,
//! `workflow-child-summary.ts:42`), one is UTF-16 **code units** (`.length`,
//! `workflow-preflight.ts:57`). [`Bounded`] and [`BoundedUtf16`] are distinct types so the two
//! units can never be transposed.

/// A non-blank string proven to fit `N` UTF-8 **bytes**. REJECTS — the
/// `bounded`/`normalizeDisplayString` family (SCOPE_3 §A.4); never truncates, so there is no
/// char-boundary problem and no half-formed identifier.
///
/// The per-field limit is a type parameter, not an argument: `thinking: Bounded<32>` cannot be
/// assigned a `Bounded<256>`, so the 32/256 transposition upstream's positional
/// `bounded(value, maxBytes)` invites is unrepresentable.
///
/// No `Deserialize` derive — reads route through [`Bounded::parse`] via the hand-written impl
/// below (SCOPE_3d §0.9); `Serialize` stays transparent (writing is not a trust boundary).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct Bounded<const N: usize>(String);

impl<const N: usize> Bounded<N> {
    /// pi `bounded` (`workflow-child-summary.ts:41-44`): `None` for blank-after-trim or over `N`
    /// UTF-8 bytes. Returns the **original** value, not the trimmed one — the trim is only the
    /// emptiness test.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value.trim().is_empty() || value.len() > N {
            None
        } else {
            Some(Self(value.to_string()))
        }
    }

    /// Borrows the value for comparison, display and serialization boundaries.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<const N: usize> std::fmt::Display for Bounded<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes THROUGH [`Bounded::parse`] — same idiom as
/// [`WorkflowKey`](crate::workflows::WorkflowKey) (SCOPE_3d §0.9).
impl<'de, const N: usize> serde::Deserialize<'de> for Bounded<N> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "expected a non-blank string of at most {N} UTF-8 bytes"
            ))
        })
    }
}

/// A non-blank string proven to fit `N` UTF-16 **code units** — the ONE limit upstream measures
/// that way (`workflow-preflight.ts:57`'s `.length`, a JS string length). A distinct type from
/// [`Bounded`] so the two units can never be transposed; declared here (SCOPE_3d, per §A.2) and
/// consumed by SCOPE_3e's preflight display strings.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct BoundedUtf16<const N: usize>(String);

impl<const N: usize> BoundedUtf16<N> {
    /// `None` for blank-after-trim or over `N` UTF-16 code units; returns the original value
    /// (same contract as [`Bounded::parse`], different unit).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value.trim().is_empty() || value.encode_utf16().count() > N {
            None
        } else {
            Some(Self(value.to_string()))
        }
    }

    /// Borrows the value for comparison, display and serialization boundaries.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<const N: usize> std::fmt::Display for BoundedUtf16<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes THROUGH [`BoundedUtf16::parse`] (SCOPE_3d §0.9).
impl<'de, const N: usize> serde::Deserialize<'de> for BoundedUtf16<N> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "expected a non-blank string of at most {N} UTF-16 code units"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// pi `bounded`: blank-after-trim or over-limit (UTF-8 BYTES) ⇒ `None`; the ORIGINAL value —
    /// whitespace and all — is what a passing parse keeps.
    #[test]
    fn bounded_rejects_blank_and_over_limit_and_keeps_the_original() {
        assert!(Bounded::<4>::parse("").is_none());
        assert!(Bounded::<4>::parse("   ").is_none());
        assert!(Bounded::<4>::parse("abcde").is_none());
        // Bytes, not chars: 'é' is 2 UTF-8 bytes, so two of them blow a 3-byte limit.
        assert!(Bounded::<3>::parse("éé").is_none());
        let kept = Bounded::<8>::parse(" a ").expect("non-blank fits");
        assert_eq!(kept.as_str(), " a ", "the original, not the trimmed value");
    }

    /// The UTF-16 twin counts code units: 'é' is ONE UTF-16 unit (two UTF-8 bytes), '𝄞' is TWO
    /// (a surrogate pair).
    #[test]
    fn bounded_utf16_counts_code_units_not_bytes() {
        assert!(BoundedUtf16::<2>::parse("éé").is_some(), "2 units, 4 bytes");
        assert!(
            Bounded::<2>::parse("éé").is_none(),
            "the byte twin rejects it"
        );
        assert!(
            BoundedUtf16::<1>::parse("𝄞").is_none(),
            "a surrogate pair is 2 units"
        );
        assert!(BoundedUtf16::<2>::parse("𝄞").is_some());
    }

    /// §0.9: reads route through `parse`; writes stay transparent.
    #[test]
    fn deserialize_goes_through_parse() {
        let ok: Bounded<8> = serde_json::from_str("\"hi\"").expect("fits");
        assert_eq!(ok.as_str(), "hi");
        assert!(serde_json::from_str::<Bounded<2>>("\"long\"").is_err());
        assert!(serde_json::from_str::<Bounded<8>>("\" \"").is_err());
        assert_eq!(serde_json::to_string(&ok).expect("serializes"), "\"hi\"");
        assert!(serde_json::from_str::<BoundedUtf16<1>>("\"𝄞\"").is_err());
    }
}
