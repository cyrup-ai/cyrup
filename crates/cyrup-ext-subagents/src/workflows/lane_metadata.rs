//! Launch-declared workflow lane metadata — pi `runs/shared/lane-metadata.ts:1-74`, the half
//! `workflow-receipt.ts` needs. The worktree-reference half (`:99-126`, `normalizeWorktreeStatusReference`/
//! `validateAsyncStatusLaneMetadata`) belongs to a different family and has no consumer here.

use serde_json::Value;

use super::bounded::Bounded;
use super::key::WorkflowKey;
use super::types::{LaneMetadataVersion, WorkflowLaneMetadata, WorkflowLaneMode};

/// pi `WORKFLOW_LANE_KEY_MAX_BYTES` (`lane-metadata.ts:3`) — 128, i.e. exactly the
/// [`WorkflowKey`] grammar's own ceiling, which is why the length check dissolves into the type
/// (`WorkflowLaneMetadata::key` is a `WorkflowKey`, not a bounded string).
pub const WORKFLOW_LANE_KEY_MAX_BYTES: usize = 128;
/// pi `WORKFLOW_LANE_SOURCE_REF_MAX_BYTES` (`:4`).
pub const WORKFLOW_LANE_SOURCE_REF_MAX_BYTES: usize = 128;
/// pi `WORKFLOW_LANE_CLAIM_MAX_BYTES` (`:5`).
pub const WORKFLOW_LANE_CLAIM_MAX_BYTES: usize = 160;
/// pi `WORKFLOW_LANE_CLAIMS_MAX` (`:6`).
pub const WORKFLOW_LANE_CLAIMS_MAX: usize = 20;
/// pi `WORKFLOW_LANE_OUTPUT_PATH_MAX_BYTES` (`:7`).
pub const WORKFLOW_LANE_OUTPUT_PATH_MAX_BYTES: usize = 256;
/// pi `WORKFLOW_LANE_OUTPUT_PATHS_MAX` (`:8`).
pub const WORKFLOW_LANE_OUTPUT_PATHS_MAX: usize = 10;

/// The top-level allow-list — pi's `assertKnownFields(value, [...], label)` call
/// (`lane-metadata.ts:48`).
const LANE_METADATA_FIELDS: [&str; 6] = [
    "version",
    "key",
    "mode",
    "sourceRef",
    "claims",
    "outputPaths",
];

/// A rejected lane — every `throw` in `normalizeWorkflowLaneMetadata`/`assertWorkflowLaneKey`,
/// carrying upstream's message verbatim (the caller's `label` already interpolated).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LaneMetadataError(String);

impl LaneMetadataError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// The upstream message, verbatim.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

/// pi `boundedNonEmptyString`'s third rule (`lane-metadata.ts:23`): no `\n`, `\r`, or NUL. NOT
/// folded into [`Bounded::parse`] (SCOPE_3 §A.4's "one helper, not nine" — widening `Bounded` to
/// cover a rule only this ONE upstream helper needs is the bug class §A.4 exists to prevent), so
/// it is a separate, named predicate applied only in this module.
fn has_forbidden_control_character(value: &str) -> bool {
    value.chars().any(|c| matches!(c, '\n' | '\r' | '\u{0}'))
}

/// pi `boundedNonEmptyString` (`lane-metadata.ts:20-26`), the two rules [`Bounded::parse`] alone
/// does not distinguish (non-string/blank and over-length collapse to one `None`, matching
/// `child_summary.rs`'s own `optional_bounded` precedent) plus the control-character rule above.
fn lane_bounded_string<const N: usize>(
    value: &Value,
    label: &str,
) -> Result<Bounded<N>, LaneMetadataError> {
    let Some(bounded) = value.as_str().and_then(Bounded::<N>::parse) else {
        return Err(LaneMetadataError::new(format!(
            "{label} must be a non-empty string."
        )));
    };
    if has_forbidden_control_character(bounded.as_str()) {
        return Err(LaneMetadataError::new(format!(
            "{label} must not contain newlines or NUL bytes."
        )));
    }
    Ok(bounded)
}

/// pi `boundedStringArray` (`lane-metadata.ts:36-44`). The sparse-array sub-check
/// (`Object.hasOwn(value, index)`) is structurally absent: a `serde_json::Value::Array` parsed
/// from JSON can never contain a hole the way a JS array literal can.
fn lane_bounded_string_array<const N: usize>(
    value: &Value,
    label: &str,
    max_items: usize,
) -> Result<Vec<Bounded<N>>, LaneMetadataError> {
    let Some(array) = value.as_array() else {
        return Err(LaneMetadataError::new(format!("{label} must be an array.")));
    };
    if array.len() > max_items {
        return Err(LaneMetadataError::new(format!(
            "{label} supports at most {max_items} entries."
        )));
    }
    array
        .iter()
        .enumerate()
        .map(|(index, entry)| lane_bounded_string(entry, &format!("{label}[{index}]")))
        .collect()
}

fn parse_lane_mode(value: &Value, label: &str) -> Result<WorkflowLaneMode, LaneMetadataError> {
    match value.as_str() {
        Some("mutation") => Ok(WorkflowLaneMode::Mutation),
        Some("review") => Ok(WorkflowLaneMode::Review),
        Some("scout") => Ok(WorkflowLaneMode::Scout),
        Some("gate") => Ok(WorkflowLaneMode::Gate),
        _ => Err(LaneMetadataError::new(format!("{label}.mode is invalid."))),
    }
}

/// pi `normalizeWorkflowLaneMetadata` (`lane-metadata.ts:49-70`).
///
/// `Ok(None)` is `value === undefined`; `Err` is every throw, message verbatim with `label`
/// interpolated — the labels are product surfaces (`buildWorkflowReceipt` passes
/// `workflow receipt child '<key>'.lane`, `workflow-receipt.ts:52`).
///
/// # Errors
///
/// The eight rejections in upstream's order: not a plain object; unsupported fields; `version`
/// not `1`; `key` invalid (non-empty-string-bound, THEN grammar — both collapse into
/// [`WorkflowKey::parse`]); `mode` invalid; `sourceRef`/`claims`/`outputPaths` each invalid.
pub fn normalize_workflow_lane_metadata(
    value: Option<&Value>,
    label: &str,
) -> Result<Option<WorkflowLaneMetadata>, LaneMetadataError> {
    let Some(value) = value else {
        return Ok(None);
    };
    // Rule 1 — `assertPlainObject`. A `serde_json::Value::Object` parsed from JSON has no
    // "foreign prototype" to reject; `.as_object()` alone already excludes null/array/scalar.
    let Some(map) = value.as_object() else {
        return Err(LaneMetadataError::new(format!(
            "{label} must be a plain JSON object."
        )));
    };
    // Rule 2 — `assertKnownFields`. `serde_json::Map` preserves insertion order in this
    // workspace (`serde_json/preserve_order` is enabled crate-wide), so this listing matches
    // upstream's `Object.keys(value)` order.
    let unknown: Vec<&str> = map
        .keys()
        .map(String::as_str)
        .filter(|field| !LANE_METADATA_FIELDS.contains(field))
        .collect();
    if !unknown.is_empty() {
        return Err(LaneMetadataError::new(format!(
            "{label} has unsupported fields: {}.",
            unknown.join(", ")
        )));
    }
    // Rule 3 — `version !== 1` (an ABSENT key also fails this: JS `undefined !== 1`).
    let version_ok = map
        .get("version")
        .and_then(Value::as_f64)
        .is_some_and(|version| version == 1.0);
    if !version_ok {
        return Err(LaneMetadataError::new(format!(
            "{label}.version must be 1."
        )));
    }
    // Rules 4+5 — both of upstream's `key` checks (bound + grammar) collapse into ONE
    // `WorkflowKey::parse` call: the grammar is byte-identical and its 128-byte ceiling IS
    // `WORKFLOW_LANE_KEY_MAX_BYTES`, so the type subsumes both.
    let key_label = format!("{label}.key");
    let key = map
        .get("key")
        .and_then(Value::as_str)
        .and_then(|raw| WorkflowKey::parse(raw).ok())
        .ok_or_else(|| LaneMetadataError::new(format!("{key_label} is invalid.")))?;
    // Rule 6 — `mode`, optional; an EXPLICIT `null` is present-and-invalid (JS `null !== undefined`).
    let mode = match map.get("mode") {
        None => None,
        Some(value) => Some(parse_lane_mode(value, label)?),
    };
    // Rule 7 — `sourceRef`, optional.
    let source_ref = match map.get("sourceRef") {
        None => None,
        Some(value) => Some(lane_bounded_string::<WORKFLOW_LANE_SOURCE_REF_MAX_BYTES>(
            value,
            &format!("{label}.sourceRef"),
        )?),
    };
    // Rule 8 — `claims`, optional.
    let claims = match map.get("claims") {
        None => None,
        Some(value) => Some(lane_bounded_string_array::<WORKFLOW_LANE_CLAIM_MAX_BYTES>(
            value,
            &format!("{label}.claims"),
            WORKFLOW_LANE_CLAIMS_MAX,
        )?),
    };
    // Rule 9 — `outputPaths`, optional.
    let output_paths = match map.get("outputPaths") {
        None => None,
        Some(value) => Some(lane_bounded_string_array::<
            WORKFLOW_LANE_OUTPUT_PATH_MAX_BYTES,
        >(
            value,
            &format!("{label}.outputPaths"),
            WORKFLOW_LANE_OUTPUT_PATHS_MAX,
        )?),
    };
    Ok(Some(WorkflowLaneMetadata {
        version: LaneMetadataVersion,
        key,
        mode,
        source_ref,
        claims,
        output_paths,
    }))
}

/// pi `assertWorkflowLaneKey` (`lane-metadata.ts:71-74`) — a lane's key must equal the workflow
/// key it is attached to. **Both guards are the point**: it is a no-op when either side is absent
/// (`:72`), so a caller with no key in hand cannot accidentally reject a valid lane.
///
/// # Errors
///
/// `<label>.key '<lane>' does not match workflow key '<key>'.`, verbatim.
pub fn assert_workflow_lane_key(
    lane: Option<&WorkflowLaneMetadata>,
    workflow_key: Option<&WorkflowKey>,
    label: &str,
) -> Result<(), LaneMetadataError> {
    let (Some(lane), Some(workflow_key)) = (lane, workflow_key) else {
        return Ok(());
    };
    if lane.key != *workflow_key {
        return Err(LaneMetadataError::new(format!(
            "{label}.key '{}' does not match workflow key '{}'.",
            lane.key.as_str(),
            workflow_key.as_str()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn key(raw: &str) -> WorkflowKey {
        WorkflowKey::parse(raw).expect("valid key")
    }

    #[test]
    fn absent_value_is_ok_none() {
        assert_eq!(normalize_workflow_lane_metadata(None, "lane"), Ok(None));
    }

    #[test]
    fn a_well_formed_lane_parses_with_every_field() {
        let value = serde_json::json!({
            "version": 1,
            "key": "lane.a",
            "mode": "review",
            "sourceRef": "main",
            "claims": ["src/a.rs"],
            "outputPaths": ["out/a.txt"],
        });
        let lane = normalize_workflow_lane_metadata(Some(&value), "lane")
            .expect("parses")
            .expect("present");
        assert_eq!(lane.key.as_str(), "lane.a");
        assert_eq!(lane.mode, Some(WorkflowLaneMode::Review));
        assert_eq!(lane.source_ref.as_ref().map(Bounded::as_str), Some("main"));
        assert_eq!(
            lane.claims.as_ref().map(|c| c.len()),
            Some(1),
            "claims parsed"
        );
        assert_eq!(
            lane.output_paths.as_ref().map(|o| o.len()),
            Some(1),
            "outputPaths parsed"
        );
    }

    #[test]
    fn a_minimal_lane_needs_only_version_and_key() {
        let value = serde_json::json!({ "version": 1, "key": "lane.a" });
        let lane = normalize_workflow_lane_metadata(Some(&value), "lane")
            .expect("parses")
            .expect("present");
        assert_eq!(lane.mode, None);
        assert_eq!(lane.source_ref, None);
    }

    #[test]
    fn rejects_a_non_object() {
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&serde_json::json!("nope")), "lane"),
            Err(LaneMetadataError::new("lane must be a plain JSON object."))
        );
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&serde_json::json!(null)), "lane"),
            Err(LaneMetadataError::new("lane must be a plain JSON object."))
        );
    }

    #[test]
    fn rejects_unsupported_fields_by_name_in_source_order() {
        let value = serde_json::json!({ "version": 1, "key": "lane.a", "bogus": true, "also": 1 });
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&value), "lane"),
            Err(LaneMetadataError::new(
                "lane has unsupported fields: bogus, also."
            ))
        );
    }

    #[test]
    fn rejects_a_wrong_version() {
        let value = serde_json::json!({ "version": 2, "key": "lane.a" });
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&value), "lane"),
            Err(LaneMetadataError::new("lane.version must be 1."))
        );
        let missing = serde_json::json!({ "key": "lane.a" });
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&missing), "lane"),
            Err(LaneMetadataError::new("lane.version must be 1."))
        );
    }

    #[test]
    fn rejects_a_bad_key_the_same_way_as_a_bad_key_grammar() {
        let value = serde_json::json!({ "version": 1, "key": ".bad" });
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&value), "lane"),
            Err(LaneMetadataError::new("lane.key is invalid."))
        );
        let missing = serde_json::json!({ "version": 1 });
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&missing), "lane"),
            Err(LaneMetadataError::new("lane.key is invalid."))
        );
    }

    #[test]
    fn rejects_a_bad_mode_but_not_an_absent_one() {
        let value = serde_json::json!({ "version": 1, "key": "lane.a", "mode": "bogus" });
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&value), "lane"),
            Err(LaneMetadataError::new("lane.mode is invalid."))
        );
        let null_mode = serde_json::json!({ "version": 1, "key": "lane.a", "mode": null });
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&null_mode), "lane"),
            Err(LaneMetadataError::new("lane.mode is invalid.")),
            "an explicit null is present, not absent"
        );
    }

    #[test]
    fn rejects_control_characters_the_bound_check_alone_does_not_catch() {
        let value =
            serde_json::json!({ "version": 1, "key": "lane.a", "sourceRef": "line1\nline2" });
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&value), "lane"),
            Err(LaneMetadataError::new(
                "lane.sourceRef must not contain newlines or NUL bytes."
            ))
        );
    }

    #[test]
    fn rejects_claims_over_the_count_or_byte_ceiling() {
        let too_many = serde_json::json!({
            "version": 1, "key": "lane.a",
            "claims": (0..21).map(|i| i.to_string()).collect::<Vec<_>>(),
        });
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&too_many), "lane"),
            Err(LaneMetadataError::new(
                "lane.claims supports at most 20 entries."
            ))
        );
        let too_long = serde_json::json!({
            "version": 1, "key": "lane.a",
            "claims": ["a".repeat(161)],
        });
        assert_eq!(
            normalize_workflow_lane_metadata(Some(&too_long), "lane"),
            Err(LaneMetadataError::new(
                "lane.claims[0] must be a non-empty string."
            ))
        );
    }

    #[test]
    fn assert_workflow_lane_key_is_a_noop_when_either_side_is_absent() {
        let lane = normalize_workflow_lane_metadata(
            Some(&serde_json::json!({ "version": 1, "key": "lane.a" })),
            "lane",
        )
        .expect("parses")
        .expect("present");
        assert_eq!(
            assert_workflow_lane_key(None, Some(&key("lane.a")), "lane"),
            Ok(())
        );
        assert_eq!(assert_workflow_lane_key(Some(&lane), None, "lane"), Ok(()));
        assert_eq!(
            assert_workflow_lane_key(Some(&lane), Some(&key("lane.a")), "lane"),
            Ok(())
        );
    }

    #[test]
    fn assert_workflow_lane_key_rejects_a_mismatch() {
        let lane = normalize_workflow_lane_metadata(
            Some(&serde_json::json!({ "version": 1, "key": "lane.a" })),
            "lane",
        )
        .expect("parses")
        .expect("present");
        assert_eq!(
            assert_workflow_lane_key(Some(&lane), Some(&key("lane.b")), "lane"),
            Err(LaneMetadataError::new(
                "lane.key 'lane.a' does not match workflow key 'lane.b'."
            ))
        );
    }
}
