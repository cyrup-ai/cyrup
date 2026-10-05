//! The author-facing descriptor builder for pi's tool exposure (`ToolDefinition.exposure`,
//! `namespace` and `defaultActive`, `core/extensions/types.ts:509`, `:527`, `:608` @v1.0.1) and
//! the wire values `guest::lower_tool_descriptor` hands the host.
//!
//! `guest.rs` only compiles for `wasm32`, so the part of the lowering that is a decision rather
//! than a field copy lives in `ToolDescriptor::wire_exposure` / `wire_default_active`, which this
//! host-target test can reach; `lower_tool_descriptor` calls them and its exhaustive-destructure
//! guard fails the wasm build if a descriptor field is added without a lowering.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::descriptor::{ToolDescriptor, ToolExposure, ToolNamespace};
use serde_json::json;

/// pi's five string literals — the spelling the host parses.
const WIRE: [(ToolExposure, &str); 5] = [
    (ToolExposure::Direct, "direct"),
    (ToolExposure::ModelOnly, "model-only"),
    (ToolExposure::Codemode, "codemode"),
    (ToolExposure::Deferred, "deferred"),
    (ToolExposure::Hidden, "hidden"),
];

#[test]
fn every_exposure_has_pis_wire_spelling() {
    for (exposure, literal) in WIRE {
        assert_eq!(exposure.as_str(), literal);
        assert_eq!(
            serde_json::to_value(exposure).unwrap(),
            json!(literal),
            "the serde spelling is the wire spelling"
        );
        assert_eq!(
            serde_json::from_value::<ToolExposure>(json!(literal)).unwrap(),
            exposure
        );
    }
}

#[test]
fn the_builder_sets_exposure_namespace_and_default_active() {
    let d = ToolDescriptor::new("t", json!({"type": "object"}));
    assert_eq!(d.exposure, ToolExposure::Direct);
    assert!(d.namespace.is_none());
    assert!(d.default_active);

    let d = d
        .exposure(ToolExposure::Deferred)
        .namespace(
            ToolNamespace::new("mcp__docs")
                .description("docs server")
                .instructions("use the docs"),
        )
        .default_active(false);
    assert_eq!(d.exposure, ToolExposure::Deferred);
    assert_eq!(
        d.namespace,
        Some(ToolNamespace {
            name: "mcp__docs".into(),
            description: Some("docs server".into()),
            instructions: Some("use the docs".into()),
        })
    );
    assert!(!d.default_active);
}

/// What crosses the boundary: each non-default exposure as pi's literal, `direct` and
/// `default_active == true` as the omitted field (`none`), which the host reads back as the
/// same defaults.
#[test]
fn the_lowered_wire_values_are_pis_literals_and_omit_the_defaults() {
    for (exposure, literal) in WIRE {
        let d = ToolDescriptor::new("t", json!({})).exposure(exposure);
        let expected = (exposure != ToolExposure::Direct).then_some(literal);
        assert_eq!(d.wire_exposure(), expected, "{literal}");
    }
    let d = ToolDescriptor::new("t", json!({}));
    assert_eq!(d.wire_default_active(), None);
    assert_eq!(d.default_active(false).wire_default_active(), Some(false));
}

/// A descriptor written before these fields existed still deserializes, to pi's defaults.
#[test]
fn a_descriptor_without_the_new_fields_deserializes_to_the_defaults() {
    let d: ToolDescriptor = serde_json::from_value(json!({
        "name": "old",
        "label": "Old",
        "description": "",
        "parameters": {},
    }))
    .unwrap();
    assert_eq!(d.exposure, ToolExposure::Direct);
    assert!(d.namespace.is_none());
    assert!(d.default_active);
}
