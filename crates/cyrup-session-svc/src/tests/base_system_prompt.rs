//! The LIVE base system prompt across a `before_agent_start` reset (arch-11 §3.1).
//!
//! Pi keeps `_baseSystemPrompt` as MUTABLE state (`agent-session.ts:371`): `setActiveToolsByName`
//! reassigns it from `_rebuildSystemPrompt(validToolNames)` (:939) and the run path then reads the
//! LIVE field both when handing the prompt to `before_agent_start` (:1228) and when resetting the
//! agent because no handler replaced it (:1252).
//!
//! cyrup's `assemble_run_messages` used to read the builder-frozen `services.system_prompt`
//! instead, so the first run AFTER a `/tools` toggle reverted the prompt to the startup tool set —
//! but only once some extension subscribed to `BeforeAgentStart` (without a subscriber the fast
//! path returns early and the rebuild survives), which made it look like an extension bug.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::ExtensionId;
use cyrup_core::StopReason;
use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use futures::StreamExt;
use tempfile::TempDir;

/// A `before_agent_start` subscriber that changes NOTHING — the minimal condition that takes the
/// dispatch path instead of the `no_subscribers` fast path. Stands in for the real-world case:
/// `cyrup-permission-system` subscribes to `BeforeAgentStart` (extension.rs:1081) and arms itself
/// merely from the presence of a `cyrup-permissions.jsonc` file.
struct PassiveStartSubscriber;

#[async_trait::async_trait]
impl NativeExtension for PassiveStartSubscriber {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("passive-start-subscriber")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart]);
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

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
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true); // --approve: deterministic trusted project
    cfg
}

/// A tool-set rebuild becomes the new BASE prompt: the next run's `before_agent_start` reset must
/// restore the REBUILT prompt, not the one the builder assembled at session start.
#[tokio::test]
async fn tool_rebuild_updates_the_base_prompt_the_next_run_resets_to() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);

    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .with_native_extension(Arc::new(PassiveStartSubscriber))
        .build()
        .await
        .expect("build");

    let startup = session.system_prompt().to_string();

    // Narrow the active set to a single tool (Pi `setActiveToolsByName`, agent-session.ts:939).
    let all = session.all_tools();
    assert!(
        all.len() > 1,
        "fixture must expose more than one enable-able tool: {}",
        all.len()
    );
    let keep = all[0].name.clone();
    session
        .set_active_tools_by_name(std::slice::from_ref(&keep))
        .await;

    let rebuilt = session.current_system_prompt().await;
    assert_ne!(
        rebuilt, startup,
        "narrowing the tool set must change the assembled prompt"
    );
    assert_eq!(
        session.base_system_prompt(),
        rebuilt,
        "the rebuild is the new live base"
    );

    // A run with a `before_agent_start` subscriber that replaces nothing.
    let stream = session.prompt("hello").await.expect("prompt accepted");
    session.wait_for_idle().await;
    let _ = stream.collect::<Vec<_>>().await;

    assert_eq!(
        session.current_system_prompt().await,
        rebuilt,
        "before_agent_start reverted the agent to the STARTUP prompt, discarding the tool rebuild"
    );
    assert_eq!(
        session.base_system_prompt(),
        rebuilt,
        "the live base must still be the rebuild"
    );
    assert_eq!(
        session.active_tool_names(),
        vec![keep],
        "the active tool set must be unchanged"
    );
}
