//! Round-6 parity tests for the three closeable items the round-3 re-analysis flagged (gap-09):
//!   * #13b — the `input` extension event **transform** arm (Pi agent-session.ts:1029-1032 /
//!     runner.ts:1116-1119): a handler rewrites the submission text/images via
//!     `EventPatch::Input`, and the rewritten content is what actually runs.
//!   * #26 — `switchSession({cwdOverride})` threading the override into the resumed
//!     `SessionManager` (Pi runtime.ts:207 → `SessionManager.open(path, _, cwdOverride)`): the
//!     manager's own cwd is rebound, so the exported JSONL header reports the override
//!     (Pi exportToJsonl `cwd: sessionManager.getCwd()`, agent-session.ts:3061) while the persisted
//!     file header keeps its original cwd.
//!   * #17b — `extendResourcesFromExtensions("startup")` (Pi agent-session.ts:2112-2135): the
//!     skill/prompt/theme paths every `resources_discover` handler contributes are merged into the
//!     resource registry BEFORE skill pointers + the system prompt are derived.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{Content, ExtensionId, Message, StopReason};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HandledValue, HostCtx, HostEvent, HookOutcome, InitApi,
    NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use crate::{
    AgentSessionRuntime, SessionBuilder, SessionConfig, SessionFactory, SessionTarget,
    SwitchSessionOptions,
};
use tempfile::TempDir;

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
    cfg
}

fn first_user_text(messages: &[Message]) -> Option<String> {
    messages.iter().find_map(|m| match m {
        Message::User { content, .. } => Some(
            content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<String>(),
        ),
        _ => None,
    })
}

// ============================================================ #13b input-event `transform` arm ====

/// A native `input` handler that rewrites the submission text to upper-case via the
/// `EventPatch::Input` mutate arm (Pi `action:"transform"`).
struct UppercaseInput;
#[async_trait::async_trait]
impl NativeExtension for UppercaseInput {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("uppercase-input")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::Input]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::Input { text, .. } => {
                HookOutcome::Mutate(EventPatch::Input { text: text.to_uppercase(), images: None })
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// gap #13b: an `input` handler returning a `transform` patch rewrites the in-flight submission, and
/// the rewritten text is what is persisted + run (Pi agent-session.ts:1029-1032).
#[tokio::test]
async fn input_event_transform_rewrites_submission_text() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .with_native_extension(Arc::new(UppercaseInput))
        .build()
        .await
        .unwrap();

    let _ = session.prompt("hello world").await.unwrap();
    session.wait_for_idle().await;

    let messages = session.messages().await;
    assert_eq!(
        first_user_text(&messages).as_deref(),
        Some("HELLO WORLD"),
        "the transform handler's rewritten text is what runs + persists"
    );
}

// =========================================================== #26 cwdOverride → manager + export ====

/// gap #26: `switchSession({cwdOverride})` rebinds the resumed manager's own cwd, so the exported
/// JSONL header reports the override (Pi exportToJsonl `cwd: getCwd()`), while the persisted session
/// file keeps its original header cwd (Pi leaves `fileEntries`' header untouched).
#[tokio::test]
async fn switch_session_with_cwd_override_rebinds_manager_cwd_and_export_header() {
    let fx = fixture();
    let cwd2 = fx._tmp.path().join("project2");
    std::fs::create_dir_all(&cwd2).unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();
    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();

    // Drive a turn so the session file flushes (its header cwd == fx.cwd).
    let session_file = {
        let s = runtime.session().await;
        let file = s.session_file().await.expect("persisted");
        let _ = s.prompt("hi").await.unwrap();
        s.wait_for_idle().await;
        file
    };

    // Resume that file with a cwd override.
    let result = runtime
        .switch_session_with(
            session_file.clone(),
            SwitchSessionOptions { cwd_override: Some(cwd2.clone()) },
        )
        .await
        .unwrap();
    assert!(!result.cancelled);

    // The exported JSONL header reports the override (Pi `cwd: sessionManager.getCwd()`).
    let session = runtime.session().await;
    let jsonl = session.export_to_jsonl(None).await.unwrap().expect("jsonl text");
    let header: serde_json::Value =
        serde_json::from_str(jsonl.lines().next().expect("header line")).unwrap();
    assert_eq!(
        header["cwd"].as_str(),
        cwd2.to_str(),
        "the exported header cwd reflects the cwd override"
    );

    // The persisted session file on disk keeps its ORIGINAL header cwd (override is manager-only).
    let on_disk = std::fs::read_to_string(&session_file).unwrap();
    let disk_header: serde_json::Value =
        serde_json::from_str(on_disk.lines().next().expect("disk header line")).unwrap();
    assert_eq!(
        disk_header["cwd"].as_str(),
        fx.cwd.to_str(),
        "the persisted file header keeps its original cwd"
    );
}

// ================================================ #17b extendResourcesFromExtensions at startup ====

/// A native extension that contributes a skill path via `resources_discover` (Pi handler returning
/// `{ skillPaths: [...] }`).
struct ResourceContributor {
    skill_path: PathBuf,
}
#[async_trait::async_trait]
impl NativeExtension for ResourceContributor {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("resource-contributor")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ResourcesDiscover]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::ResourcesDiscover => HookOutcome::Handled(HandledValue(serde_json::json!({
                "skillPaths": [self.skill_path.to_string_lossy()],
            }))),
            _ => HookOutcome::Noop,
        }
    }
}

/// gap #17b: a skill contributed via `resources_discover` is merged into the registry at startup and
/// appears in the system prompt (Pi `extendResourcesFromExtensions` → `_rebuildSystemPrompt`).
#[tokio::test]
async fn extension_contributed_skill_is_merged_into_resources_and_system_prompt() {
    let fx = fixture();
    // An out-of-tree skill file the extension will contribute (NOT under any discovery root).
    let skill_dir = fx._tmp.path().join("ext-skills").join("extskill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_md = skill_dir.join("SKILL.md");
    std::fs::write(
        &skill_md,
        "---\nname: extskill\ndescription: contributed by an extension\n---\n\nEXT_SKILL_BODY\n",
    )
    .unwrap();

    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx))
        .with_native_extension(Arc::new(ResourceContributor { skill_path: skill_md.clone() }))
        .build()
        .await
        .unwrap();

    assert!(
        session.resources().skills.contains("extskill"),
        "the extension-contributed skill is merged into the resource registry"
    );
    assert!(
        session.system_prompt().contains("extskill"),
        "the contributed skill is listed in the rebuilt system prompt"
    );
}

/// gap #17b: with no `resources_discover` contribution the discovered registry is left untouched
/// (Pi's early returns at agent-session.ts:2118/2124) — no extension skill leaks in.
#[tokio::test]
async fn no_extension_contribution_leaves_registry_untouched() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();
    assert!(
        !session.resources().skills.contains("extskill"),
        "no contribution means no extension-supplied skill"
    );
}
