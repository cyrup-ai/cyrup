//! PROV-083a — the `supportsMidConvoSystemMessages` / `supportsMidConvoToolChanges` runtime
//! defaults, hand-rolled from `packages/ai/scripts/generate-models.ts:606-611` and its two provider
//! gates at `:1208-1216` @v0.87.1.
//!
//! This is the test that stops the mid-conversation port shipping as DEAD CODE. Not one of cyrup's
//! 35 catalog files under `providers/catalog/` carries a `supportsMidConvo*` key, so if the flags
//! defaulted to a constant `false` — as they do on the completions route, where upstream assigns them
//! from explicit model-id sets rather than a pattern — `resolve_transcript` would collapse every
//! transcript and `utils::transcript` would be unreachable.

use super::*;
use crate::api::anthropic_messages::{
    default_supports_mid_convo_system_messages, default_supports_mid_convo_tool_changes,
    supports_mid_convo_system_messages, supports_mid_convo_tool_changes,
};

fn with_id(id: &str) -> Model {
    Model {
        id: id.into(),
        ..model()
    }
}

fn with(provider: &str, id: &str) -> Model {
    Model {
        id: id.into(),
        provider: provider.into(),
        ..model()
    }
}

/// `^claude-opus-(?:4[.-]8|5(?:[.-]5)?)(?:-\d{8})?$` and
/// `^claude-(?:fable|mythos)-5(?:[.-]1)?(?:-\d{8})?$` (`generate-models.ts:606-611`).
#[test]
fn mid_convo_system_messages_default_matches_upstreams_two_anchored_regexes() {
    for id in [
        // `4[.-]8` — both spellings, with and without the 8-digit date suffix.
        "claude-opus-4-8",
        "claude-opus-4.8",
        "claude-opus-4-8-20260115",
        // `5(?:[.-]5)?` — bare 5, and 5.5 in both spellings.
        "claude-opus-5",
        "claude-opus-5-20260115",
        "claude-opus-5-5",
        "claude-opus-5.5",
        "claude-opus-5-5-20260115",
        // The fable/mythos alternative: `-5` with an OPTIONAL `.1`/`-1`.
        "claude-fable-5",
        "claude-fable-5-1",
        "claude-fable-5.1",
        "claude-fable-5-1-20260115",
        "claude-mythos-5",
        "claude-mythos-5-1",
    ] {
        assert!(
            default_supports_mid_convo_system_messages(&with_id(id)),
            "{id} must match"
        );
        assert!(
            default_supports_mid_convo_tool_changes(&with_id(id)),
            "{id} must match (anthropic provider)"
        );
    }

    for id in [
        // Below the cut: Opus 4.6/4.7 are adaptive-thinking models, not mid-convo ones.
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-5",
        // The first alternative's minor is MANDATORY for the `4` branch — a bare `4` is out.
        "claude-opus-4",
        // Only the `5`/`5.5` minors participate; `5.1`/`5-6` do not.
        "claude-opus-5-1",
        "claude-opus-5-6",
        // Sonnet and Haiku appear in NEITHER alternative.
        "claude-sonnet-4-5",
        "claude-sonnet-5",
        "claude-haiku-5",
        // Anchors: no prefix, no suffix, no partial.
        "anthropic/claude-opus-5-5",
        "claude-opus-5-5-preview",
        "claude-opus-5-5x",
        // The date suffix is exactly 8 DIGITS.
        "claude-opus-5-5-2026011",
        "claude-opus-5-5-202601155",
        "claude-opus-5-5-2026011a",
        // fable/mythos take `-5` only, and only `1` as a minor.
        "claude-fable-4",
        "claude-fable-5-2",
        "claude-mythos-6",
    ] {
        assert!(
            !default_supports_mid_convo_system_messages(&with_id(id)),
            "{id} must NOT match"
        );
        assert!(
            !default_supports_mid_convo_tool_changes(&with_id(id)),
            "{id} must NOT match"
        );
    }
}

/// The two provider gates (`generate-models.ts:1208-1216`): `anthropic` gets BOTH flags;
/// `opencode` and `github-copilot` get system messages ONLY, because they *"forward
/// mid-conversation system messages but reject `tool_addition`/`tool_removal` blocks"*; every other
/// provider gets neither.
#[test]
fn provider_gates_split_system_messages_from_tool_changes() {
    for provider in ["opencode", "github-copilot"] {
        let m = with(provider, "claude-opus-5-5");
        assert!(
            default_supports_mid_convo_system_messages(&m),
            "{provider} forwards system messages"
        );
        assert!(
            !default_supports_mid_convo_tool_changes(&m),
            "{provider} rejects tool_addition/tool_removal blocks"
        );
    }
    for provider in ["openrouter", "bedrock", "vertex", "opencode-go", "xiaomi"] {
        let m = with(provider, "claude-opus-5-5");
        assert!(
            !default_supports_mid_convo_system_messages(&m),
            "{provider} is not a verified mid-convo provider"
        );
        assert!(!default_supports_mid_convo_tool_changes(&m));
    }
}

/// A DECLARED catalog value wins over the runtime default, in both directions — the `??` precedence
/// of `getAnthropicCompat` (`anthropic-messages.ts:170-181`).
#[test]
fn declared_compat_overrides_the_runtime_default_in_both_directions() {
    // Default-true model, declared off.
    let mut off = with_id("claude-opus-5-5");
    off.compat = Some(ModelCompat {
        supports_mid_convo_system_messages: Some(false),
        supports_mid_convo_tool_changes: Some(false),
        ..Default::default()
    });
    assert!(!supports_mid_convo_system_messages(&off));
    assert!(!supports_mid_convo_tool_changes(&off));

    // Default-false model, declared on.
    let mut on = with("openrouter", "claude-sonnet-4-5");
    on.compat = Some(ModelCompat {
        supports_mid_convo_system_messages: Some(true),
        supports_mid_convo_tool_changes: Some(true),
        ..Default::default()
    });
    assert!(supports_mid_convo_system_messages(&on));
    assert!(supports_mid_convo_tool_changes(&on));

    // No declaration at all ⇒ the runtime default, which is the whole point.
    let bare = with_id("claude-opus-5-5");
    assert!(bare.compat.is_none());
    assert!(supports_mid_convo_system_messages(&bare));
    assert!(supports_mid_convo_tool_changes(&bare));
}

/// The wire keys round-trip through `ModelCompat`'s `camelCase` serde, and are ABSENT when unset.
#[test]
fn compat_keys_are_pis_camel_case_spellings_and_absent_when_unset() {
    let declared: ModelCompat = serde_json::from_value(json!({
        "supportsMidConvoSystemMessages": true,
        "supportsMidConvoToolChanges": true,
        "supportsMidConvoToolAdditions": true,
    }))
    .expect("pi's spellings deserialize");
    assert_eq!(declared.supports_mid_convo_system_messages, Some(true));
    assert_eq!(declared.supports_mid_convo_tool_changes, Some(true));
    assert_eq!(declared.supports_mid_convo_tool_additions, Some(true));
    let back = serde_json::to_value(&declared).expect("serialize");
    assert_eq!(back["supportsMidConvoSystemMessages"], json!(true));
    assert_eq!(back["supportsMidConvoToolChanges"], json!(true));
    assert_eq!(back["supportsMidConvoToolAdditions"], json!(true));

    let empty = serde_json::to_value(ModelCompat::default()).expect("serialize");
    for key in [
        "supportsMidConvoSystemMessages",
        "supportsMidConvoToolChanges",
        "supportsMidConvoToolAdditions",
    ] {
        assert!(empty.get(key).is_none(), "{key} is absent, not null");
    }
}
