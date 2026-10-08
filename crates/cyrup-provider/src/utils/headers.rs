//! Request-header merging (1:1 port of pi `utils/headers.ts:11-23` `providerHeadersToRecord`
//! @ce950d78f), PROV-126.
//!
//! Pi merges its header sources **case-insensitively**: a later source overrides an earlier one
//! whatever either spells the name as, the surviving entry keeps the *last* writer's spelling, and a
//! later `null` removes the earlier source's header. Before PROV-126 cyrup's ports overlaid each
//! source into a case-sensitive [`HeaderMap`] under its own spelling, so `X-Tenant` over `x-tenant`
//! went on the wire twice.
//!
//! Pi callers @ce950d78f: `google-generative-ai.ts:358`, `google-vertex.ts:399`,
//! `pi-messages.ts:398`, `openrouter-images.ts:130`, `llama-cpp-classify.ts:236`,
//! `classifier-shared.ts:47` and `bedrock-converse-stream.ts:258` (whose port lower-cases its keys
//! in `bedrock_converse_stream/headers.rs`). `anthropic-messages.ts:293-301` deliberately does NOT
//! use it: its `mergeHeaders` is a plain case-sensitive `Object.assign`, so cyrup's
//! `anthropic_messages/headers.rs` keeps that shape.

use crate::HeaderMap;

/// pi `providerHeadersToRecord(...headerSources)` (`utils/headers.ts:11-23` @ce950d78f), returning
/// a [`HeaderMap`] for the adapters whose header pipeline is `Option`-valued.
///
/// Sources apply in order. Each entry first deletes every earlier entry whose name matches it
/// case-insensitively (`merged.delete(normalizedName)`, `:17`), then is inserted under its own
/// spelling (`merged.set(normalizedName, [name, value])`, `:18`). A `None` value is kept as a
/// tombstone instead of being dropped as pi's `value !== null` filter does: downstream of this merge
/// cyrup's transport and the `User-Agent` default (`user_agent::insert_default_user_agent`) read a
/// `None` as "suppress", so the tombstone is how the removal stays a removal past this function.
/// [`provider_headers_to_record`] drops the tombstones for callers that want pi's exact record.
pub(crate) fn merge_provider_headers(sources: &[Option<&HeaderMap>]) -> HeaderMap {
    let mut merged = HeaderMap::new();
    for source in sources.iter().copied().flatten() {
        for (name, value) in source {
            let normalized = name.to_lowercase();
            merged.retain(|existing, _| existing.to_lowercase() != normalized);
            merged.insert(name.clone(), value.clone());
        }
    }
    merged
}

/// pi `providerHeadersToRecord` (`utils/headers.ts:11-23` @ce950d78f) as pi returns it: the merged
/// name/value record with removed (`null`) headers absent, in pi's `Map` insertion order (a
/// re-set header moves to the end, `:17-18`).
pub(crate) fn provider_headers_to_record(sources: &[Option<&HeaderMap>]) -> Vec<(String, String)> {
    let mut merged: Vec<(String, String, String)> = Vec::new();
    for source in sources.iter().copied().flatten() {
        for (name, value) in source {
            let normalized = name.to_lowercase();
            merged.retain(|(existing, _, _)| *existing != normalized);
            if let Some(value) = value {
                merged.push((normalized, name.clone(), value.clone()));
            }
        }
    }
    merged
        .into_iter()
        .map(|(_, name, value)| (name, value))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, Option<&str>)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.map(str::to_string)))
            .collect()
    }

    /// `utils/headers.ts:16-18`: a later source overrides an earlier one regardless of casing.
    #[test]
    fn a_later_source_overrides_an_earlier_one_case_insensitively() {
        let base = map(&[("x-api-key", Some("default"))]);
        let overlay = map(&[("X-Api-Key", Some("override"))]);
        let merged = merge_provider_headers(&[Some(&base), Some(&overlay)]);
        assert_eq!(merged.len(), 1, "{merged:?}");
        assert_eq!(
            merged.get("X-Api-Key").cloned().flatten().as_deref(),
            Some("override")
        );
        let record = provider_headers_to_record(&[Some(&base), Some(&overlay)]);
        assert_eq!(
            record,
            vec![("X-Api-Key".to_string(), "override".to_string())]
        );
    }

    /// `utils/headers.ts:18`: the surviving entry keeps the last writer's spelling, in both
    /// directions.
    #[test]
    fn the_surviving_entry_keeps_the_last_writers_spelling() {
        let upper = map(&[("Anthropic-Beta", Some("a"))]);
        let lower = map(&[("anthropic-beta", Some("b"))]);
        let merged = merge_provider_headers(&[Some(&upper), Some(&lower)]);
        assert_eq!(merged, map(&[("anthropic-beta", Some("b"))]));
        let merged = merge_provider_headers(&[Some(&lower), Some(&upper)]);
        assert_eq!(merged, map(&[("Anthropic-Beta", Some("a"))]));
    }

    /// `utils/headers.ts:17-18`: a later `null` removes the earlier header whatever its casing.
    #[test]
    fn a_later_none_removes_the_earlier_header_in_any_casing() {
        let base = map(&[
            ("Content-Type", Some("application/json")),
            ("x-keep", Some("1")),
        ]);
        let overlay = map(&[("content-type", None)]);
        let merged = merge_provider_headers(&[Some(&base), Some(&overlay)]);
        assert_eq!(
            merged,
            map(&[("content-type", None), ("x-keep", Some("1"))])
        );
        let record = provider_headers_to_record(&[Some(&base), Some(&overlay)]);
        assert_eq!(record, vec![("x-keep".to_string(), "1".to_string())]);
    }

    /// `utils/headers.ts:14,23`: absent sources are skipped, and a re-set header moves to the end of
    /// pi's insertion-ordered record.
    #[test]
    fn absent_sources_are_skipped_and_a_reset_header_moves_last() {
        let base = map(&[("a", Some("1")), ("b", Some("2"))]);
        let overlay = map(&[("A", Some("3"))]);
        let record = provider_headers_to_record(&[Some(&base), None, Some(&overlay)]);
        assert_eq!(
            record,
            vec![
                ("b".to_string(), "2".to_string()),
                ("A".to_string(), "3".to_string())
            ]
        );
        assert!(provider_headers_to_record(&[None, None]).is_empty());
    }
}
