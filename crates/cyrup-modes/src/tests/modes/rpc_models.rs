//! SEAM-004 / G39 — `get_available_models`, `set_model` and `cycle_model` must resolve against the
//! FULL auth-filtered registry (pi's `modelRuntime.getAvailable()` = every CONFIGURED provider),
//! not just the catalog of the provider the session launched on. Each case credentials a second
//! provider by writing `auth.json` into the fixture's agent dir before building the runtime.

use std::io::Cursor;
use std::sync::Arc;

use super::support::{build_runtime, build_runtime_hermetic_auth, fixture, parse_lines};
use crate::run_rpc;
use cyrup_provider::faux::FauxProvider;

// ----------------------------------------------------------------------------------------------
// SEAM-004 — `set_model` / `get_available_models` / `get_state` must resolve against the FULL
// auth-filtered model registry, not just the currently-installed provider's own catalog. Pi reads
// `session.modelRuntime.getAvailable()` (rpc-mode.ts:468 for set_model, :486 for
// get_available_models), which is `ModelRegistry.getAll().filter(hasConfiguredAuth)` — every
// configured provider, not one. Pre-fix cyrup called `session.model_catalog()` (the active provider
// only), so an RPC embedder could neither see nor select a model owned by another configured
// provider.
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn rpc_model_commands_span_the_full_auth_filtered_registry() {
    let fx = fixture();
    // Give `anthropic` a stored credential so `has_configured_auth` is true for its catalog — the
    // "second configured provider" the active (faux) provider knows nothing about.
    std::fs::write(
        fx.agent_dir.join("auth.json"),
        r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
    )
    .expect("write auth.json");

    let runtime = build_runtime(&fx, Arc::new(FauxProvider::new())).await;

    // Phase 1 — `get_available_models` must list the OTHER configured provider's models.
    let reader = Cursor::new(
        concat!(r#"{"type":"get_available_models","id":"a"}"#, "\n")
            .as_bytes()
            .to_vec(),
    );
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out)
        .await
        .expect("rpc mode runs");
    let lines = parse_lines(&out);
    let listed = lines
        .iter()
        .find(|l| l["command"] == "get_available_models")
        .expect("get_available_models response");
    let models = listed["data"]["models"]
        .as_array()
        .expect("models array")
        .clone();
    let anthropic = models
        .iter()
        .find(|m| m["provider"] == "anthropic")
        .unwrap_or_else(|| {
            panic!(
                "get_available_models must span every CONFIGURED provider (Pi \
                 modelRuntime.getAvailable(), rpc-mode.ts:486), not just the active one; got:\n{models:#?}"
            )
        })
        .clone();
    let anthropic_id = anthropic["id"]
        .as_str()
        .expect("catalog model carries an id")
        .to_string();

    // Phase 2 — `set_model` onto that non-active provider must succeed, and `get_state` must then
    // report the FULL model record for it (not the two-field degraded stub).
    let script = format!(
        "{{\"type\":\"set_model\",\"id\":\"b\",\"provider\":\"anthropic\",\"modelId\":\"{anthropic_id}\"}}\n\
         {{\"type\":\"get_state\",\"id\":\"c\"}}\n"
    );
    let reader = Cursor::new(script.into_bytes());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out)
        .await
        .expect("rpc mode runs");
    let lines = parse_lines(&out);
    let set = lines
        .iter()
        .find(|l| l["command"] == "set_model")
        .expect("set_model response");
    assert_eq!(
        set["success"], true,
        "set_model onto a different CONFIGURED provider must succeed (Pi rpc-mode.ts:468-475): {set}"
    );
    let state = lines
        .iter()
        .find(|l| l["command"] == "get_state")
        .expect("get_state response");
    assert_eq!(
        state["data"]["model"]["provider"], "anthropic",
        "get_state model: {state}"
    );
    assert_eq!(
        state["data"]["model"]["id"].as_str(),
        Some(anthropic_id.as_str())
    );
    assert!(
        state["data"]["model"].get("contextWindow").is_some()
            || state["data"]["model"]
                .as_object()
                .map(|o| o.len())
                .unwrap_or(0)
                > 2,
        "get_state.model must be the FULL catalog record, not the degraded {{provider,id}} stub: {state}"
    );
}

// ----------------------------------------------------------------------------------------------
// G39 — the SEAM-004 hole `cycle_model` was left in. Pi's `_cycleAvailableModel` opens with
// `const availableModels = await this._modelRuntime.getAvailable()` (agent-session.ts:1644 at
// v0.83.0) — `getAll().filter(hasConfiguredAuth)` across EVERY provider (model-runtime.ts:315-329).
// cyrup cycled `provider.current().models()`, the ONE installed provider's own catalog, so the
// `cycle_model` RPC verb (rpc.rs:1116, the `{"type":"cycle_model"}` request a `cyrup --mode rpc`
// client sends) could never leave the provider the session launched on even when a second provider
// was fully credentialed — while `get_available_models` and `set_model`, fixed by SEAM-004, could.
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn rpc_cycle_model_spans_the_full_auth_filtered_registry() {
    let fx = fixture();
    // The same second configured provider SEAM-004 uses: a stored `auth.json` credential makes
    // `has_configured_auth` true for anthropic's whole catalog. The active provider stays faux.
    std::fs::write(
        fx.agent_dir.join("auth.json"),
        r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
    )
    .expect("write auth.json");

    let runtime = build_runtime_hermetic_auth(&fx, Arc::new(FauxProvider::new())).await;

    let reader = Cursor::new(
        concat!(
            r#"{"type":"cycle_model","id":"c"}"#,
            "\n",
            r#"{"type":"get_state","id":"s"}"#,
            "\n"
        )
        .as_bytes()
        .to_vec(),
    );
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out)
        .await
        .expect("rpc mode runs");
    let lines = parse_lines(&out);

    let cycled = lines
        .iter()
        .find(|l| l["command"] == "cycle_model")
        .expect("cycle_model response");
    assert_eq!(cycled["success"], true, "cycle_model response: {cycled}");
    assert!(
        !cycled["data"].is_null(),
        "one faux model + a CREDENTIALED anthropic catalog is >1 candidate, so Pi's \
         `availableModels.length <= 1` early return must NOT fire (agent-session.ts:1645); \
         cycling the active provider's own catalog alone is what makes this null: {cycled}"
    );
    assert_eq!(
        cycled["data"]["model"]["provider"], "anthropic",
        "cycle_model must step onto the OTHER configured provider (Pi \
         `_modelRuntime.getAvailable()`, agent-session.ts:1644): {cycled}"
    );
    assert_eq!(
        cycled["data"]["isScoped"], false,
        "no scoped set → the available arm: {cycled}"
    );

    // ...and the switch is real: the session's live model — what the next turn streams with —
    // reports the new provider, which also proves the owning provider was installed.
    let state = lines
        .iter()
        .find(|l| l["command"] == "get_state")
        .expect("get_state response");
    assert_eq!(
        state["data"]["model"]["provider"], "anthropic",
        "get_state after cycle: {state}"
    );
}
