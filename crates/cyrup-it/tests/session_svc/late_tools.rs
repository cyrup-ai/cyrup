//! EXT-004 END TO END — a tool an extension registers AFTER `init` must reach the LIVE AGENT, not
//! just the extension host's own registry.
//!
//! Pi's `registerTool()` ends with `runtime.refreshTools()` on EVERY registration
//! (`core/extensions/loader.ts:249-256`), which is bound to `_refreshToolRegistry()`
//! (agent-session.ts:2396). That method rebuilds `_toolRegistry`/`_toolDefinitions`/
//! `_toolPromptSnippets` AND auto-activates every name that was not in `previousRegistryNames`,
//! finishing with `this.setActiveToolsByName([...new Set(nextActiveToolNames)])`
//! (agent-session.ts:2534-2545). `examples/extensions/dynamic-tools.ts` is the canonical shape:
//! register from a `session_start` handler, use the tool on the very next turn.
//!
//! cyrup snapshotted the tool set ONCE, at `SessionBuilder::build()` time (`DynamicToolState::new`),
//! and `session_start` is dispatched strictly LATER (`bind_extensions`). So a late registration
//! produced a descriptor that existed in `ExtensionHost` and was invisible to the model forever.
//!
//! The assertion below is deliberately NOT "`ExtensionHost::active_tools()` contains it" — that is
//! the extension-host layer and it already passed. It is "the AGENT offered the tool to the model
//! on a real turn", captured from the provider request itself.
// The original `#![cfg(feature = "wasm-host")]` is deliberately GONE. It named
// cyrup-session-svc's own feature, which that crate enables in its `default` — so it was
// always true here. Re-spelled in cyrup-it it would name THIS crate's `wasm-host`, which
// `--features it` does not enable, and every test below would SILENTLY not compile in.
// See the `[[test]]` note in crates/cyrup-it/Cargo.toml.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{ExtensionId, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider, FauxResponseStep};
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
    cfg.no_extensions = true;
    cfg
}

/// The tool sets an agent offered the model, one entry per turn (Pi request `tools`), plus the
/// system prompt it sent — both are rebuilt by `_refreshToolRegistry`, so both are evidence.
type CapturedTurns = Arc<Mutex<Vec<(Vec<String>, String)>>>;

fn faux_capturing_tools(captured: &CapturedTurns) -> Arc<FauxProvider> {
    let cap = captured.clone();
    let step = FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
        cap.lock().unwrap().push((
            ctx.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            ctx.system_prompt.clone().unwrap_or_default(),
        ));
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
    });
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![step.clone(), step.clone(), step]);
    faux
}

/// THE EXT-004 PROOF: the demo guest registers `demo_late` from its `session_start` handler; the
/// running agent offers that tool to the model on the very next turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_registered_from_session_start_reaches_the_live_agent() {
    let bytes = bins::component_bytes();
    let fx = fixture();
    let captured: CapturedTurns = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_capturing_tools(&captured);

    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap();
    let _ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");

    // BASELINE — `init` has run, `session_start` has NOT. The late tool cannot exist yet, and this
    // is exactly the state the whole session lived in before EXT-004.
    let before = session.active_tool_names();
    assert!(
        !before.contains(&"demo_late".to_string()),
        "the late tool is not registered before `session_start`: {before:?}"
    );

    // The seam every host calls exactly once for the initial session (Pi print-mode.ts:73,
    // rpc-mode.ts:318, interactive-mode.ts:1698). This is what dispatches `session_start`.
    session.bind_extensions().await;

    // (1) The session's dynamic-tool registry picked the tool up AND auto-activated it, matching
    //     Pi's `if (!previousRegistryNames.has(toolName)) nextActiveToolNames.push(toolName)`.
    let after = session.active_tool_names();
    assert!(
        after.contains(&"demo_late".to_string()),
        "a tool registered from `session_start` joined the ACTIVE set: {after:?}"
    );
    assert!(
        session.tool_definition("demo_late").is_some_and(|t| t.active),
        "the late tool is enable-able AND active in the session's registry"
    );
    // Nothing was lost: the built-ins the session started with are still active.
    for kept in &before {
        assert!(after.contains(kept), "the refresh kept `{kept}` active: {after:?}");
    }

    // (2) THE LIVE PROOF: drive a real turn and read the tool array the AGENT handed the model.
    //     Before EXT-004 this list never contained `demo_late`, whatever the extension host said.
    let _ = session.prompt("hello").await.unwrap();
    session.wait_for_idle().await;

    let turns = captured.lock().unwrap().clone();
    assert!(!turns.is_empty(), "the agent drove at least one real turn against the provider");
    let (tools, prompt) = turns.last().unwrap().clone();
    assert!(
        tools.iter().any(|t| t == "demo_late"),
        "the AGENT offered the late-registered tool to the model: {tools:?}"
    );
    // The refresh REBUILDS the base system prompt (Pi `_rebuildSystemPrompt`, agent-session.ts:2304)
    // rather than clearing it: the built-ins' guidance must survive the auto-activation pass. (The
    // demo tool itself declares no `promptSnippet`, so — as in Pi — it contributes no prompt text;
    // the tool ARRAY above is what makes it callable.)
    assert!(
        prompt.contains("read") && prompt.contains("bash"),
        "the rebuilt system prompt still carries the built-in tool guidance: {prompt:?}"
    );
}
