//! WASM COMPACTION-OVERRIDE END-TO-END (L4 gap #5 — closes the split-observation hedge).
//!
//! The typed session-control payload (`session_before_compact` → `CompactionPreparation`, Pi
//! `SessionBeforeCompactResult.compaction`, agent-session.ts:1672-1693) was previously proven across
//! TWO SEPARATE tests, never one:
//!   * `round9_l5res.rs::compaction_before_compact_override_lands_in_entry` — a NATIVE ext whose
//!     override lands in the persisted entry, driven through the real `AgentSession::compact()`.
//!   * `cyrup-ext/tests/wasm_dispatch.rs` — a REAL `wasm32-wasip2` guest that reads the typed
//!     `CompactionPreparation` across the boundary and returns an override, but driven via
//!     `host.emit_session_before_compact` DIRECTLY, NOT through `AgentSession::compact()`.
//!
//! This test removes that hedge with ONE end-to-end proof: it loads the REAL compiled wasm guest
//! (the bundled `cyrup-ext-sdk` demo — its `example.rs::on_session_before_compact` reads the typed
//! preparation and returns `demo-summary[<reason>|firstKept=<firstKeptEntryId>]`) INTO an assembled
//! `AgentSession`, drives two real turns to build compactable content, then calls the PRODUCTION
//! `AgentSession::compact()` path (NOT `host.emit_*`). It asserts the guest's typed-payload-derived
//! override summary ACTUALLY lands in the resulting `CompactionResult` AND the exported JSONL
//! compaction entry — i.e. the wasm guest's override took effect through the real compact() path,
//! observed in real persisted state. Because the guest derives the summary from the REAL preparation
//! (`reason` = "manual" and the live `firstKeptEntryId`), the assertion proves the typed payload
//! crossed the wasm boundary — not a fabricated constant.
// The original `#![cfg(feature = "wasm-host")]` is deliberately GONE. It named
// cyrup-session-svc's own feature, which that crate enables in its `default` — so it was
// always true here. Re-spelled in cyrup-it it would name THIS crate's `wasm-host`, which
// `--features it` does not enable, and every test below would SILENTLY not compile in.
// See the `[[test]]` note in crates/cyrup-it/Cargo.toml.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{EventKind, Extension};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

// MIGRATION (docs/TEST-ARCHITECTURE.md §3.4): this file used to carry its own `fixture_component()`
// that shelled out to `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` into the SHARED, fixed
// `std::env::temp_dir()/cyrup-session-svc-fixture-target` — one of ten byte-identical copies that
// serialized on each other's cargo build lock and never cleaned up. `cyrup-it`'s `build.rs` now
// builds the component ONCE for the whole suite and exports its path; `CYRUP_EXT_FIXTURE_COMPONENT`
// still overrides it, at that one place instead of ten.
use crate::support::bins;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    // Disable on-disk extension auto-discovery so ONLY the explicitly-loaded wasm guest is present.
    cfg.no_extensions = true;
    cfg
}

/// Compaction settings that force even a small session to compact (keep nothing, reserve nothing) —
/// same knobs as `round9_l5res.rs::aggressive_compaction_settings`.
fn aggressive_compaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

/// L4 gap #5 (single end-to-end proof): a LIVE `wasm32-wasip2` guest loaded INTO an assembled
/// `AgentSession` reads the typed `CompactionPreparation` and returns a custom-summary override that
/// lands in the persisted compaction entry — observed through the REAL production `compact()` path,
/// not a hand-called `host.emit_*`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_guest_compaction_override_lands_through_agent_session_compact() {
    let bytes = bins::component_bytes();

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Only the two turn responses — the guest override skips the model summarizer entirely (no
    // summary completion is ever requested), exactly like the native proof in round9_l5res.rs.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build");

    // Load the guest COMPONENT through the session's OWN host, injecting the session's
    // LiveHostServices (arch-08 §5.6). This is the production injection seam, identical to the
    // wasm_slash_command headline proof.
    let ext = session
        .load_wasm_extension(ExtensionId::from("demo"), &bytes)
        .await
        .expect("load + init the live wasm extension");

    // The guest's `init` declared the `session_before_compact` subscription across the WIT boundary —
    // so the assembled host's `emit_before_compact` will actually dispatch to it (not short-circuit
    // on `no_subscribers`).
    assert!(
        ext.subscriptions().contains(EventKind::SessionBeforeCompact),
        "the live wasm guest subscribed to SessionBeforeCompact"
    );

    // Two real turns over the faux provider build a compactable transcript.
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    // Drive the REAL production compaction path (Pi `agent-session.ts:1648-1693`): compute the real
    // preparation, fire `session_before_compact` at the LIVE wasm guest, apply its override. This is
    // NOT `host.emit_session_before_compact` hand-called — it is the assembled `AgentSession::compact`.
    let cr = session
        .compact(None)
        .await
        .expect("an aggressive-keep compaction over two turns produces a result");

    // The guest's override summary landed in the CompactionResult, replacing the default model
    // summarization. The demo derives it from the reason + the REAL `firstKeptEntryId` it read off the
    // typed preparation — so the override is a value derived from the payload that crossed the wasm
    // boundary, and it byte-matches the entry's own first-kept cut point.
    assert!(
        cr.summary.starts_with("demo-summary[manual|firstKept="),
        "the wasm guest's derived override summary lands in the compaction result: {}",
        cr.summary
    );
    assert_eq!(
        cr.summary,
        format!("demo-summary[manual|firstKept={}]", cr.first_kept_entry_id),
        "the guest embedded the REAL preparation's firstKeptEntryId — proving the typed payload \
         crossed the live wasm boundary and drove the persisted cut point"
    );

    // And it is durable in the exported JSONL as a `fromExtension` compaction entry (the guest's
    // override took effect in real persisted state, observed through the real compact() path).
    let jsonl = session.export_to_jsonl(None).await.unwrap().expect("jsonl");
    assert!(
        jsonl.contains("demo-summary[manual|firstKept="),
        "the wasm guest override summary is persisted in the exported JSONL: {jsonl}"
    );
    assert!(
        jsonl.contains("\"type\":\"compaction\""),
        "a compaction entry was appended to the session tree: {jsonl}"
    );
}

/// Guard: the fixture path resolves to a real file (the nested build actually produced a component).
#[test]
fn fixture_component_exists() {
    let p = bins::component();
    assert!(Path::new(&p).exists(), "fixture component missing at {}", p.display());
}
