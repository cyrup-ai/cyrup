//! The virtual-model CATALOG layer (`crate::virtual_models`): pi `createVirtualModel`
//! (`packages/coding-agent/src/core/virtual-models.ts:169-187` @v1.0.4).
//!
//! Upstream's own assertions these mirror live in `packages/coding-agent/test/virtual-models.test.ts`
//! @v1.0.4: the entry shape and `getSupportedThinkingLevels` at `:117-122`.
//!
//! The decorator tests this file used to carry went with the decorator; `crate::unrouted_message`
//! records why. Its text is still pinned, at the two sites that now produce it:
//! `crates/cyrup-agent/src/tests/unrouted_virtual_model.rs` (the agent loop) and
//! `crates/cyrup-session-svc/src/host_services.rs`'s
//! `complete_standalone_refuses_a_virtual_selection_with_pis_unrouted_text` (the extension seam).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::collection::get_supported_thinking_levels;
use crate::model::Modality;
use crate::virtual_models::{
    VIRTUAL_MODEL_API, VirtualModelSpec, create_virtual_model, is_virtual_model,
};
use cyrup_core::ModelThinkingLevel;

// ---------------------------------------------------------------- fixtures ----

fn spec(provider: &str, id: &str) -> VirtualModelSpec {
    VirtualModelSpec {
        provider: provider.into(),
        id: id.into(),
        name: "Auto".to_string(),
        thinking_levels: None,
        context_window: None,
        max_tokens: None,
        input: None,
    }
}

/// Upstream's own definition: `{ provider: "router", id: "auto", name: "Auto",
/// thinkingLevels: ["low", "high"] }` (`test/virtual-models.test.ts:34-38`).
fn router_auto() -> crate::model::Model {
    create_virtual_model(&VirtualModelSpec {
        thinking_levels: Some(vec![ModelThinkingLevel::Low, ModelThinkingLevel::High]),
        ..spec("router", "auto")
    })
}

fn virtual_under(provider: &str, id: &str) -> crate::model::Model {
    create_virtual_model(&spec(provider, id))
}

// ------------------------------------------------- create_virtual_model ----

/// pi `createVirtualModel` (`virtual-models.ts:174-186`) and upstream's own shape assertion
/// `{ provider: "router", id: "auto", contextWindow: 0, maxTokens: 0 }` with
/// `input == ["text","image"]` (`test/virtual-models.test.ts:120-121`).
#[test]
fn create_virtual_model_shape() {
    let model = router_auto();
    assert_eq!(model.provider.as_str(), "router");
    assert_eq!(model.id.as_str(), "auto");
    assert_eq!(model.name, "Auto");
    assert_eq!(model.api.as_str(), VIRTUAL_MODEL_API);
    assert_eq!(model.api.as_str(), "pi-virtual");
    assert_eq!(model.base_url, "");
    assert_eq!(model.context_window, 0);
    assert_eq!(model.max_tokens, 0);
    assert_eq!(model.input, vec![Modality::Text, Modality::Image]);
    assert_eq!(model.cost.input, 0.0);
    assert_eq!(model.cost.output, 0.0);
    assert_eq!(model.cost.cache_read, 0.0);
    assert_eq!(model.cost.cache_write, 0.0);
    assert!(model.cost.tiers.is_none());
    assert!(model.sampling_params.is_none());
    assert!(model.compat.is_none());
    assert!(model.headers.is_none());
    assert!(is_virtual_model(&model));
}

/// Declared limits and input types are carried through rather than defaulted (the `??` arms of
/// `:182-185` taking the LEFT side).
#[test]
fn create_virtual_model_carries_declared_limits_and_input() {
    let model = create_virtual_model(&VirtualModelSpec {
        context_window: Some(1000),
        max_tokens: Some(4000),
        input: Some(vec![Modality::Text]),
        ..spec("router", "auto")
    });
    assert_eq!(model.context_window, 1000);
    assert_eq!(model.max_tokens, 4000);
    assert_eq!(model.input, vec![Modality::Text]);
}

/// The subtle half of `virtual-models.ts:173`: EVERY rung is written, offered ⇒ its own name and
/// unoffered ⇒ an explicit `null`, so `get_supported_thinking_levels` answers exactly the offered
/// set — upstream's `getSupportedThinkingLevels(virtual) == ["low","high"]`
/// (`test/virtual-models.test.ts:122`).
#[test]
fn create_virtual_model_writes_all_seven_thinking_levels() {
    let model = router_auto();
    // Asserted FIRST because it is the consequence the explicit nulls exist for: with only the
    // offered keys written, an absent `minimal`/`medium` reads as SUPPORTED and this returns
    // `[Minimal, Low, Medium, High]`.
    assert_eq!(
        get_supported_thinking_levels(&model),
        vec![ModelThinkingLevel::Low, ModelThinkingLevel::High]
    );
    let map = model.thinking_level_map.as_ref().expect("map present");
    assert_eq!(map.len(), 7, "all seven rungs are written: {map:?}");
    assert_eq!(map.get("low"), Some(&Some("low".to_string())));
    assert_eq!(map.get("high"), Some(&Some("high".to_string())));
    for unoffered in ["off", "minimal", "medium", "xhigh", "max"] {
        assert_eq!(
            map.get(unoffered),
            Some(&None),
            "{unoffered} must be an explicit null, not absent"
        );
    }
    assert!(model.reasoning);
}

/// `thinkingLevels` absent ⇒ `["off"]`, and `reasoning = levels.some(l => l !== "off")` is then
/// false (`:171`, `:180`).
#[test]
fn create_virtual_model_defaults_to_the_off_rung_only() {
    let model = virtual_under("router", "auto");
    let map = model.thinking_level_map.as_ref().expect("map present");
    assert_eq!(map.get("off"), Some(&Some("off".to_string())));
    for unoffered in ["minimal", "low", "medium", "high", "xhigh", "max"] {
        assert_eq!(map.get(unoffered), Some(&None), "{unoffered}");
    }
    assert!(!model.reasoning);
    assert_eq!(
        get_supported_thinking_levels(&model),
        vec![ModelThinkingLevel::Off]
    );
}

/// An explicitly EMPTY level list stays empty rather than collapsing to pi's `["off"]` default —
/// which is why every optional member of [`VirtualModelSpec`] is an `Option`.
#[test]
fn an_explicitly_empty_level_list_offers_nothing() {
    let model = create_virtual_model(&VirtualModelSpec {
        thinking_levels: Some(Vec::new()),
        ..spec("router", "auto")
    });
    let map = model.thinking_level_map.as_ref().expect("map present");
    assert_eq!(map.get("off"), Some(&None));
    assert!(!model.reasoning);
    // A non-reasoning model short-circuits to `[off]` in `get_supported_thinking_levels`.
    assert_eq!(
        get_supported_thinking_levels(&model),
        vec![ModelThinkingLevel::Off]
    );
}
