//! [`crate::QuarantinedNative`] — pi's `load.discard()` + `Failed to load extension: <message>`,
//! for a native built-in that refuses to be built before any factory exists to throw out of.
//!
//! Upstream runs every extension factory inside a `try`: a throw is caught, `load.discard()`ed and
//! recorded, and the loader CONTINUES
//! (`pi/packages/coding-agent/src/core/extensions/loader.ts:613-630` + `:655` @v1.0.1; the
//! inline tier has the same catch at `core/resource-loader.ts:1130-1141`). cyrup's natives are
//! constructed by the embedder BEFORE the loader sees them, so a built-in that declines to exist at
//! all — a `config.json` it will not accept (SUBA-166), an env payload that does not decode
//! (CFG-080) — had no throw to be caught, and the embedder's only options were to drop it silently
//! (a fail-open) or to carry the error out of the attach point, which aborted the whole launch.
//!
//! This placeholder is the missing throw. Two properties make it exact, and each is asserted here:
//! it registers nothing, so the `discard()` is trivially complete; and it fails `init` with its
//! message VERBATIM, so `cyrup-session-svc`'s EXT-S01 containment renders upstream's
//! `Failed to load extension "<id>": <message>` with no wrapper of its own.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use crate::{
    ExtMode, ExtensionHost, HostConfig, NativeExtension, QuarantinedNative, QuarantinedTier,
};
use cyrup_core::ExtensionId;

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

const REASON: &str = "/home/u/.cyrup/subagents/config.json is invalid (config.authorityPolicy.stopRuns is unknown) \
     and sets authorityPolicy";

fn quarantined(id: &str, tier: QuarantinedTier) -> Arc<dyn NativeExtension> {
    Arc::new(QuarantinedNative::new(ExtensionId::from(id), REASON, tier))
        as Arc<dyn NativeExtension>
}

/// The load FAILS, under the refused built-in's own id, and the error carries the refusal's
/// sentence and nothing else — no `extension panicked:` / `component load failed:` prefix, which
/// would appear inside pi's `Failed to load extension "<id>": …` frame and change the message an
/// operator reads.
#[tokio::test]
async fn a_quarantined_native_fails_its_load_with_the_refusal_verbatim() {
    let host = ExtensionHost::new(cfg());
    let ext = quarantined("subagents", QuarantinedTier::Ambient);
    assert_eq!(ext.id(), ExtensionId::from("subagents"));

    let err = host
        .load_native(ext)
        .await
        .expect_err("a quarantined built-in must not load");
    assert_eq!(
        err.to_string(),
        REASON,
        "the message is the refusal's own sentence, with no variant prefix"
    );
    assert!(
        !host.loaded_ids().iter().any(|i| i.as_str() == "subagents"),
        "a failed load must not leave the id registered: {:?}",
        host.loaded_ids()
    );
}

/// It registers NOTHING, which is what makes pi's `discard()` trivially exact here: there is no
/// tool, command or flag for the host to have to sweep back out.
#[tokio::test]
async fn a_quarantined_native_registers_nothing_at_all() {
    let host = ExtensionHost::new(cfg());
    let _ = host
        .load_native(quarantined(
            "subagent-prompt-runtime",
            QuarantinedTier::Ambient,
        ))
        .await;
    let tools = host.active_tools(&[]).expect("the tool set is readable");
    assert!(
        tools.is_empty(),
        "no tool: {:?}",
        tools
            .iter()
            .map(|t| t.name().to_string())
            .collect::<Vec<_>>()
    );
    assert!(
        host.native_command_names().is_empty(),
        "no command: {:?}",
        host.native_command_names()
    );
}

/// The tier is mirrored onto [`NativeExtension::is_ambient`], so `--no-extensions` treats the
/// placeholder exactly as it would have treated the built-in it stands in for (SEAM-071/SEAM-074).
///
/// This is the half with a one-directional cost: a placeholder that claimed the inline tier while
/// standing in for an AMBIENT built-in (both of cyrup's refusing built-ins are ambient) would
/// survive the flag that drops the real thing, so `cyrup --no-extensions` would report a load
/// failure for an extension that was never going to load.
#[test]
fn the_tier_decides_whether_no_extensions_drops_the_placeholder() {
    assert!(
        quarantined("subagents", QuarantinedTier::Ambient).is_ambient(),
        "an ambient built-in's placeholder is ambient too, so the flag drops both"
    );
    assert!(
        !quarantined("inline-ext", QuarantinedTier::Inline).is_ambient(),
        "pi's inline-factory tier is never gated by a flag about discovery"
    );
}
