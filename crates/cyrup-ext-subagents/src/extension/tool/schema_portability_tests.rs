//! SUBA-176 — the provider-portability keyword walk over this crate's model-facing tool schemas.
//!
//! A port of upstream's `it("does not emit provider-rejected schema shapes")`
//! (`test/unit/schemas.test.ts:512-555` @ad11b7ab), keyword list only. It lives in this `#[path]`
//! sibling of `schema.rs` rather than inside it because the list must spell the rejected keyword
//! as a quoted string, and the ledger's Verify line greps `schema.rs` for exactly that literal.
//!
//! Upstream walks every object export of `src/extension/schemas.ts` (`SubagentParams`,
//! `SubagentWaitParams`); the cyrup counterparts are [`subagent_tool_parameters`] (plus every
//! feature-reduced variant [`subagent_tool_parameters_for`] can produce) and
//! [`crate::extension::wait_tool::wait_tool_parameters`]. The two free-standing watchdog tool
//! schema builders are walked as well, since they are model-facing tool schemas of this crate.
//!
//! NOT ported here: the same upstream test's four shape checks outside the keyword list
//! (non-string `enum` members, array-valued `type`, `type` beside `anyOf`, unanchored `pattern`).
//! The non-string-`enum` check would be red today on this crate's `{"type":"boolean","enum":[false]}`
//! branches (and `version`'s `{"type":"integer","enum":[1]}`) — separate drift (upstream
//! `11931c3f`, #1954) that changes advertised semantics, recorded as a lead in SUBA-176's close
//! rather than widened into it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::{subagent_tool_parameters, subagent_tool_parameters_for};
use crate::disabled_features::{SUBAGENT_FEATURES, resolve_disabled_feature_surface};

/// Upstream's `rejectedKeywords`, verbatim and in upstream's order (`schemas.test.ts:515`
/// @ad11b7ab). `deprecated` was added by `50c280f1` (#2721): "Strict tool-schema validators reject
/// `deprecated` with HTTP 400 (#2713)."
const PROVIDER_REJECTED_KEYWORDS: [&str; 6] = ["allOf", "const", "if", "then", "not", "deprecated"];

/// Upstream's walk: every object node is checked for an OWN rejected key (`Object.hasOwn`), and
/// the walk descends into every object member (`${path}.${key}`) and array element
/// (`${path}[${index}]`) — `properties` maps included, exactly as upstream does.
fn rejected_keyword_paths(root_name: &str, root: &serde_json::Value) -> Vec<String> {
    fn walk(path: &str, value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for keyword in PROVIDER_REJECTED_KEYWORDS {
                    if map.contains_key(keyword) {
                        out.push(format!("{path}.{keyword}"));
                    }
                }
                for (key, child) in map {
                    walk(&format!("{path}.{key}"), child, out);
                }
            }
            serde_json::Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    walk(&format!("{path}[{index}]"), child, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(root_name, root, &mut out);
    out
}

/// Every model-facing tool schema this crate builds from a free function, named for the report.
fn walked_tool_schemas() -> Vec<(String, serde_json::Value)> {
    let mut schemas = vec![("subagent".to_string(), subagent_tool_parameters())];
    for entry in SUBAGENT_FEATURES {
        let surface = resolve_disabled_feature_surface(&[entry.feature], true);
        schemas.push((
            format!("subagent[-{:?}]", entry.feature),
            subagent_tool_parameters_for(&surface),
        ));
    }
    schemas.push((
        "bg_wait".to_string(),
        crate::extension::wait_tool::wait_tool_parameters(),
    ));
    schemas.push((
        "watchdog_warn".to_string(),
        crate::watchdog::review::watchdog_warn_parameters_schema(),
    ));
    schemas.push((
        "watchdog_permission_decision".to_string(),
        crate::watchdog::permission_arbiter::permission_decision_parameters_schema(),
    ));
    schemas
}

/// SUBA-176 — upstream's "does not emit provider-rejected schema shapes", keyword list.
///
/// MUTATION (each observed RED, one run per site): restore `"deprecated": true` on the
/// `"reviewed"` branch of [`super::sj_acceptance_override`] and the walk reports
/// `subagent.properties.acceptance.anyOf[1].deprecated`; on `runId`,
/// `subagent.properties.runId.deprecated`; on `maxRuntimeMs`,
/// `subagent.properties.maxRuntimeMs.deprecated` (each also under every reduced variant that
/// keeps the property).
#[test]
fn the_tool_schemas_emit_no_provider_rejected_keyword() {
    let mut rejected = Vec::new();
    for (name, schema) in walked_tool_schemas() {
        rejected.extend(rejected_keyword_paths(&name, &schema));
    }
    assert_eq!(
        rejected,
        Vec::<String>::new(),
        "strict tool-schema validators reject these keywords with HTTP 400 (#2713)"
    );
}

/// Guards the walk itself, so the test above cannot pass by walking nothing.
///
/// MUTATION: drop the `Value::Array` arm (arrays are not descended) — the nested fixture reports
/// nothing. Observed RED. MUTATION: stop recursing into object members — likewise. Observed RED.
#[test]
fn the_walk_finds_a_planted_keyword_at_every_depth() {
    assert_eq!(
        rejected_keyword_paths("root", &serde_json::json!({ "deprecated": true })),
        ["root.deprecated"]
    );
    assert_eq!(
        rejected_keyword_paths(
            "root",
            &serde_json::json!({ "anyOf": [ { "properties": { "x": { "not": {} } } } ] })
        ),
        ["root.anyOf[0].properties.x.not"]
    );
    assert_eq!(
        rejected_keyword_paths(
            "root",
            &serde_json::json!({ "allOf": [], "const": 1, "if": {}, "then": {} })
        ),
        ["root.allOf", "root.const", "root.if", "root.then"]
    );
    // A keyword NAMED as a property is still an own key of the `properties` map, and upstream's
    // `Object.hasOwn` walk reports it there too; this port matches rather than special-casing it.
    assert_eq!(
        rejected_keyword_paths(
            "root",
            &serde_json::json!({ "properties": { "if": { "type": "string" } } })
        ),
        ["root.properties.if"]
    );
}
