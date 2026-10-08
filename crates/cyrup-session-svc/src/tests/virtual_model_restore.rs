//! `SESS-067`, the RESTORE half — what a resumed session's model selection is when the branch
//! records a virtual one, and the `_recordSelection` write that keeps a `/tree` navigation
//! resumable.
//!
//! Upstream is pi @ **v1.0.4**, read through git objects:
//!
//! | upstream | cyrup |
//! |---|---|
//! | `sdk.ts:207-222` — `getBranchSelection` off the RAW branch, then `getModel` + `hasConfiguredAuth`, then `Could not restore model <selection>` | `builder.rs`'s `session_selection` + `resolve_model`'s step 2 |
//! | `agent-session.ts:609-625` — `_recordSelection`, called at the head of `_runAgentPrompt` (`:1812`) | `AgentSession::record_selection`, called from `spawn_run` and `run_injection` |
//! | `sdk.ts:430-441` — a RESUMED session gets no `model_change` at open | `builder.rs`'s step-7 seeding, unchanged |
//!
//! # Why the branches here are hand-built
//!
//! Three of these cases need an assistant message that FAILED ROUTING — one carrying the virtual
//! model and `api: "pi-virtual"`. Driving that through a prompt would require a router that fails,
//! a routed run, and a settled error turn, and would then also append entries this file counts. A
//! hand-built persisted branch is the same input the restore path reads and nothing else, so each
//! case tests exactly what it names. The branch is written through the real
//! [`cyrup_session::SessionManager`] (never by hand-rolling JSONL), so the entry shapes are the
//! ones production writes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::{
    AssistantMessage, Content, Message, ModelThinkingLevel, StopReason, Usage, VIRTUAL_MODEL_API,
};
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use cyrup_provider::{
    Model, ModelRoute, ModelRouteError, ModelRouteRequest, ModelRouter, Provider,
    VirtualModelDefinition, VirtualModelRegistry, VirtualModelSpec,
};
use cyrup_session::{NewSessionOpts, SessionLayout, SessionManager};
use serde_json::Value;
use tempfile::TempDir;

use crate::{AgentSession, SessionBuilder, SessionConfig, SessionTarget};

// ------------------------------------------------------------------------------ fixtures ----

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    sessions: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let sessions = tmp.path().join("sessions");
    for d in [&cwd, &agent_dir, &sessions] {
        std::fs::create_dir_all(d).unwrap();
    }
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
        sessions,
    }
}

impl Fixture {
    /// A PERSISTED session whose branch `build` writes, returned as the file to resume.
    fn branch(&self, build: impl FnOnce(&mut SessionManager)) -> PathBuf {
        let layout = SessionLayout::literal(self.sessions.clone(), self.cwd.clone());
        let mut m = SessionManager::create(&self.cwd, &layout, NewSessionOpts::default()).unwrap();
        build(&mut m);
        m.session_file()
            .expect("a persisted session file")
            .to_path_buf()
    }

    fn config(&self, target: SessionTarget) -> SessionConfig {
        let mut cfg = SessionConfig::new(self.cwd.clone(), self.agent_dir.clone());
        cfg.trust_override = Some(true);
        cfg.no_extensions = true;
        cfg.session_dir = Some(self.sessions.clone());
        cfg.target = target;
        cfg
    }
}

/// `faux` with `small` and `large`, matching upstream's own fixture shape
/// (`test/suite/virtual-models.test.ts:43-62`) with windows big enough that nothing here compacts.
fn two_model_faux() -> Arc<FauxProvider> {
    faux_with(&["small", "large"])
}

fn faux_with(ids: &[&str]) -> Arc<FauxProvider> {
    let models = ids
        .iter()
        .map(|id| {
            let mut m = FauxModelDefinition::new(*id);
            m.context_window = 200_000;
            m.reasoning = true;
            m
        })
        .collect();
    Arc::new(FauxProvider::with_config(FauxConfig {
        models,
        ..FauxConfig::default()
    }))
}

fn user(s: &str) -> Message {
    Message::User {
        content: vec![Content::text(s)],
        timestamp: 0,
    }
}

/// An assistant turn naming the model that answered. `api` is what `isVirtualModel` reads, so a
/// FAILED ROUTING is built with [`VIRTUAL_MODEL_API`] and `StopReason::Error` — which is exactly
/// what `emit_run_failure` persists when the router refuses.
fn answered(provider: &str, model: &str, api: &str, stop: StopReason) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::text("ok")],
        provider: provider.into(),
        model: model.into(),
        api: api.into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    })
}

fn router_spec() -> VirtualModelSpec {
    VirtualModelSpec {
        provider: "router".into(),
        id: "auto".into(),
        name: "Auto".to_string(),
        thinking_levels: Some(vec![ModelThinkingLevel::Off, ModelThinkingLevel::High]),
        context_window: None,
        max_tokens: None,
        input: None,
    }
}

/// A router that always answers with the catalog row named `to`.
struct FixedRouter {
    catalog: Vec<Model>,
    to: &'static str,
}

#[async_trait::async_trait]
impl ModelRouter for FixedRouter {
    async fn route(&self, request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError> {
        let model = self
            .catalog
            .iter()
            .find(|m| m.id.as_str() == self.to)
            .cloned()
            .ok_or_else(|| ModelRouteError::new("no such model"))?;
        Ok(ModelRoute {
            model,
            thinking_level: request.thinking_level,
            state: None,
        })
    }
}

/// A registry already holding `router/auto`, as one drained from
/// `pendingVirtualModelRegistrations` would be BEFORE the session opens
/// (`agent-session-services.ts:182-194`).
fn registry_with_router(physical: &[Model], to: &'static str) -> Arc<VirtualModelRegistry> {
    let registry = Arc::new(VirtualModelRegistry::new());
    registry
        .register(
            VirtualModelDefinition::new(
                router_spec(),
                Arc::new(FixedRouter {
                    catalog: physical.to_vec(),
                    to,
                }),
            ),
            &cyrup_provider::NoCatalog,
        )
        .expect("register router/auto");
    registry
}

/// Every `model_change` on the active branch as `provider/id`, oldest first.
async fn model_changes(session: &AgentSession) -> Vec<String> {
    session
        .entries_json()
        .await
        .into_iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("model_change"))
        .filter_map(|e| {
            let p = e.get("provider").and_then(Value::as_str)?.to_string();
            let m = e.get("modelId").and_then(Value::as_str)?;
            Some(format!("{p}/{m}"))
        })
        .collect()
}

async fn prompt_and_settle(session: &AgentSession, text: &str) {
    let _ = session.prompt(text).await.expect("prompt");
    session.wait_for_idle().await;
}

async fn resume(
    fx: &Fixture,
    path: &Path,
    provider: Arc<dyn Provider>,
    registry: Option<Arc<VirtualModelRegistry>>,
    model_pattern: Option<&str>,
) -> AgentSession {
    let mut cfg = fx.config(SessionTarget::Resume(path.to_path_buf()));
    cfg.model_pattern = model_pattern.map(str::to_string);
    let mut b = SessionBuilder::new(provider, cfg);
    if let Some(r) = registry {
        b = b.virtual_models(r);
    }
    b.build().await.expect("build")
}

// ----------------------------------------------------------------------------------- T1 ----

/// **The headline.** A branch that selects `router/auto` and then holds a physical answer under it
/// restores the VIRTUAL selection — pi `sdk.ts:207-215`.
///
/// RED-PROVE: revert `resolve_model`'s step 2 to `existing.model.as_ref()` (the forward last-wins
/// projection) — the session restores `faux/large`, the model that answered, and the router the
/// user chose is silently gone.
#[tokio::test]
async fn restores_the_virtual_selection_instead_of_the_physical_model_that_answered() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let path = fx.branch(|m| {
        m.append_message(user("hi")).unwrap();
        m.append_model_change("router".into(), "auto".into())
            .unwrap();
        m.append_message(answered(
            "faux",
            "large",
            "openai-completions",
            StopReason::Stop,
        ))
        .unwrap();
    });

    let session = resume(
        &fx,
        &path,
        faux,
        Some(registry_with_router(&physical, "large")),
        None,
    )
    .await;

    let selection = session.model().expect("a restored selection");
    assert_eq!(selection.provider.as_str(), "router");
    assert_eq!(selection.model.as_str(), "auto");
    assert_eq!(
        selection.api.as_ref().map(cyrup_core::ApiId::as_str),
        Some(VIRTUAL_MODEL_API),
        "the restored selection is the VIRTUAL catalog row"
    );
    assert!(
        session.model_fallback_message().is_none(),
        "a selection that resolves reports nothing: {:?}",
        session.model_fallback_message()
    );
}

// ----------------------------------------------------------------------------------- T2 ----

/// *"A virtual model that is no longer registered does not hold, so the selection falls back to the
/// physical model that answered last."* — and with NO fallback message, because the selection the
/// restore asked for is that physical model.
///
/// RED-PROVE: make the lookup closure in `builder.rs` claim every id virtual (drop the registry and
/// answer `Some(ModelRef { api: Some(VIRTUAL_MODEL_API), .. })` unconditionally) — the hold applies,
/// `router/auto` resolves nowhere, and a `Could not restore model router/auto` banner appears.
#[tokio::test]
async fn falls_back_to_the_physical_model_when_the_virtual_model_is_not_registered() {
    let fx = fixture();
    let faux = two_model_faux();
    let path = fx.branch(|m| {
        m.append_message(user("hi")).unwrap();
        m.append_model_change("router".into(), "auto".into())
            .unwrap();
        m.append_message(answered(
            "faux",
            "large",
            "openai-completions",
            StopReason::Stop,
        ))
        .unwrap();
    });

    // No registry: the router unloaded, exactly as after an extension is removed.
    let session = resume(&fx, &path, faux, None, None).await;

    let selection = session.model().expect("a restored selection");
    assert_eq!(selection.provider.as_str(), "faux");
    assert_eq!(selection.model.as_str(), "large");
    assert_eq!(
        session.model_fallback_message(),
        None,
        "the fallback IS the selection, so there is nothing to report"
    );
}

// ----------------------------------------------------------------------------------- T3 ----

/// A transcript ending in a FAILED ROUTING: the terminal assistant message names the virtual model
/// with `api: "pi-virtual"` (pi `virtual-models.ts:104`; cyrup's `emit_run_failure` builds exactly
/// that from the agent's state model). The walk skips it, so the selection is the last PHYSICAL
/// response — and there is no banner.
///
/// RED-PROVE: delete the `!is_virtual_api(Some(&a.api))` guard from
/// `cyrup_session::virtual_models::branch_selection`'s message arm — the failure message is taken
/// as the response, `router/auto` is then held by `find_last_model_change`, nothing resolves (no
/// registry here), and a `Could not restore model router/auto` banner appears.
///
/// This is the restore CONSEQUENCE of the already-green unit test
/// `an_assistant_message_from_failed_routing_is_skipped`, which cannot see the banner.
#[tokio::test]
async fn falls_back_to_the_last_physical_response_when_the_branch_ends_in_a_routing_failure() {
    let fx = fixture();
    let faux = two_model_faux();
    let path = fx.branch(|m| {
        m.append_model_change("router".into(), "auto".into())
            .unwrap();
        m.append_message(answered(
            "faux",
            "large",
            "openai-completions",
            StopReason::Stop,
        ))
        .unwrap();
        m.append_message(user("again")).unwrap();
        // The router refused: the run ended on the VIRTUAL model.
        m.append_message(answered(
            "router",
            "auto",
            VIRTUAL_MODEL_API,
            StopReason::Error,
        ))
        .unwrap();
    });

    let session = resume(&fx, &path, faux, None, None).await;

    let selection = session.model().expect("a restored selection");
    assert_eq!(selection.provider.as_str(), "faux");
    assert_eq!(
        selection.model.as_str(),
        "large",
        "the failed-routing turn is skipped, so the last physical response wins"
    );
    assert_eq!(
        session.model_fallback_message(),
        None,
        "nothing failed to restore"
    );
}

// ----------------------------------------------------------------------------------- T4 ----

/// The branch holds `router/auto` over an answer from a model that has since LEFT the catalog. The
/// selection is the router, which still resolves, so the session restores it and says nothing —
/// where the projection value would have failed to restore and produced a banner naming the wrong
/// model.
///
/// This is the one case where the two rules differ AND the difference is user-visible as a message,
/// which is why it exists beside T1.
///
/// RED-PROVE: revert step 2 to `existing.model.as_ref()` — the restore asks for `faux/large`, which
/// this provider no longer lists, and the session comes up on `faux/small` with
/// `Could not restore model faux/large. Using faux/small`.
#[tokio::test]
async fn a_held_virtual_selection_restores_when_the_model_that_answered_is_gone() {
    let fx = fixture();
    // The provider now lists ONLY `small`; `large` answered and was later removed.
    let faux = faux_with(&["small"]);
    let physical = faux.models().to_vec();
    let path = fx.branch(|m| {
        m.append_model_change("router".into(), "auto".into())
            .unwrap();
        m.append_message(answered(
            "faux",
            "large",
            "openai-completions",
            StopReason::Stop,
        ))
        .unwrap();
    });

    let session = resume(
        &fx,
        &path,
        faux,
        Some(registry_with_router(&physical, "small")),
        None,
    )
    .await;

    let selection = session.model().expect("a restored selection");
    assert_eq!(selection.provider.as_str(), "router");
    assert_eq!(selection.model.as_str(), "auto");
    assert_eq!(
        session.model_fallback_message(),
        None,
        "the SELECTION restored, so nothing is reported"
    );
}

/// The message names the SELECTION, not the model that answered — pi's
/// `Could not restore model ${sessionModel.provider}/${sessionModel.modelId}` (`sdk.ts:220`) with
/// `findInitialModel`'s `. Using …` appended (`:239`). Reachable through a TRAILING
/// `model_change`, which `branch_selection` returns outright with no lookup at all.
///
/// RED-PROVE: format the message from the model finally chosen instead of from the selection — the
/// text becomes `Could not restore model faux/small. Using faux/small`.
#[tokio::test]
async fn could_not_restore_names_the_virtual_selection() {
    let fx = fixture();
    let faux = faux_with(&["small"]);
    let path = fx.branch(|m| {
        m.append_message(user("hi")).unwrap();
        m.append_message(answered(
            "faux",
            "small",
            "openai-completions",
            StopReason::Stop,
        ))
        .unwrap();
        // The user selected the router and never prompted again; nothing is registered now.
        m.append_model_change("router".into(), "auto".into())
            .unwrap();
    });

    let session = resume(&fx, &path, faux, None, None).await;

    assert_eq!(
        session.model_fallback_message(),
        Some("Could not restore model router/auto. Using faux/small"),
    );
    assert_eq!(session.model().expect("a model").model.as_str(), "small");
}

// ----------------------------------------------------------------------------------- T9 ----

/// `[CYRUP-DELTA]` in shape — step 3c. A virtual model registered AFTER the builder's step-3
/// resolve (which is where cyrup's extension registrations land, long after pi's
/// `pendingVirtualModelRegistrations` drain) is still restored, and the step-3 banner that named it
/// as unrestorable is withdrawn.
///
/// Driven through the registry the builder was handed, with the registration performed between the
/// two reads — which is the observable shape of a late registration.
///
/// RED-PROVE: delete step 3c from `builder.rs` — the selection comes up as `faux/small` with
/// `Could not restore model router/auto. Using faux/small` still in force.
#[tokio::test]
async fn restores_a_virtual_selection_registered_after_the_first_resolve() {
    let fx = fixture();
    let faux = faux_with(&["small"]);
    let physical = faux.models().to_vec();
    let path = fx.branch(|m| {
        m.append_message(user("hi")).unwrap();
        m.append_message(answered(
            "faux",
            "small",
            "openai-completions",
            StopReason::Stop,
        ))
        .unwrap();
        m.append_model_change("router".into(), "auto".into())
            .unwrap();
    });

    // The registry handed to the builder is EMPTY at step 3 and populated by a native extension's
    // `init`, which runs between step 3 and step 3c.
    let registry = Arc::new(VirtualModelRegistry::new());
    let session = {
        let late = Arc::clone(&registry);
        let physical = physical.clone();
        let mut cfg = fx.config(SessionTarget::Resume(path.clone()));
        cfg.no_extensions = false;
        let b = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
            .virtual_models(Arc::clone(&registry))
            .with_native_extension(Arc::new(LateRouterExt {
                registry: late,
                physical,
            }));
        b.build().await.expect("build")
    };

    let selection = session.model().expect("a restored selection");
    assert_eq!(
        selection.provider.as_str(),
        "router",
        "the late registration is seen by step 3c"
    );
    assert_eq!(selection.model.as_str(), "auto");
    assert_eq!(
        session.model_fallback_message(),
        None,
        "the step-3 banner is withdrawn once the selection resolves"
    );
}

/// The other shape of a late registration, and the one step 3c must not miss: the branch's last
/// entry is a RESPONSE, so the selection depends on the hold rule and therefore on the registry.
/// At step 3 the registry is empty, so the walk answers `faux/small` (the response) and the session
/// resolves it without any banner at all; step 3c must re-walk with the now-populated registry and
/// find that `router/auto` holds after all.
///
/// RED-PROVE: have step 3c read the step-3 `session_selection` instead of re-walking the branch —
/// that value is `faux/small`, which the registry does not know, so nothing is adopted and the
/// session stays on `faux/small`. (This is exactly the hole the first version of step 3c had; it is
/// invisible to the trailing-`model_change` case above, where the walk needs no lookup.)
#[tokio::test]
async fn a_late_registration_is_re_walked_so_the_hold_rule_can_apply() {
    let fx = fixture();
    let faux = faux_with(&["small"]);
    let physical = faux.models().to_vec();
    let path = fx.branch(|m| {
        m.append_model_change("router".into(), "auto".into())
            .unwrap();
        m.append_message(user("hi")).unwrap();
        m.append_message(answered(
            "faux",
            "small",
            "openai-completions",
            StopReason::Stop,
        ))
        .unwrap();
    });

    let registry = Arc::new(VirtualModelRegistry::new());
    let mut cfg = fx.config(SessionTarget::Resume(path));
    cfg.no_extensions = false;
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .virtual_models(Arc::clone(&registry))
        .with_native_extension(Arc::new(LateRouterExt { registry, physical }))
        .build()
        .await
        .expect("build");

    let selection = session.model().expect("a restored selection");
    assert_eq!(
        (selection.provider.as_str(), selection.model.as_str()),
        ("router", "auto"),
        "the hold rule must be re-asked once the router is registered"
    );
    assert!(session.model_fallback_message().is_none());
}

/// A native extension that registers `router/auto` in `init`, i.e. after the builder's step-3
/// model resolve and before step 3c.
struct LateRouterExt {
    registry: Arc<VirtualModelRegistry>,
    physical: Vec<Model>,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for LateRouterExt {
    fn id(&self) -> cyrup_core::ExtensionId {
        cyrup_core::ExtensionId::from("late-router")
    }
    async fn init(&self, _api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        self.registry
            .register(
                VirtualModelDefinition::new(
                    router_spec(),
                    Arc::new(FixedRouter {
                        catalog: self.physical.clone(),
                        to: "small",
                    }),
                ),
                &cyrup_provider::NoCatalog,
            )
            .map_err(|e| cyrup_ext::ExtError::Registration(e.to_string()))
    }
    async fn on_event(
        &self,
        _ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        cyrup_ext::HookOutcome::Noop
    }
}

// ----------------------------------------------------------------------------------- T6 ----

/// `_recordSelection`'s guard (`agent-session.ts:623`): *"Responses do record a physical selection
/// unless the branch holds a virtual one; checking a physical selection against responses would
/// record it on every prompt while `prepareRequest` redirects to another model."*
///
/// The branch selects `faux/small` and then holds an answer from `faux/large`, so the branch's
/// recorded selection (`faux/large`) DIFFERS from the live selection (`faux/small`) with neither
/// side virtual. Nothing may be written, on this prompt or any later one.
///
/// RED-PROVE: drop the `!is_virtual_api(selection) && !recorded_is_virtual` clause from
/// `record_selection` — one `model_change` is appended per prompt and the count goes 1 → 3.
#[tokio::test]
async fn does_not_record_a_physical_selection_on_every_prompt() {
    let fx = fixture();
    let faux = two_model_faux();
    let path = fx.branch(|m| {
        m.append_model_change("faux".into(), "small".into())
            .unwrap();
        m.append_message(user("hi")).unwrap();
        m.append_message(answered(
            "faux",
            "large",
            "openai-completions",
            StopReason::Stop,
        ))
        .unwrap();
    });

    let session = resume(&fx, &path, faux, None, Some("faux/small"))
        .await
        .into_shared();
    assert_eq!(session.model().expect("a model").model.as_str(), "small");
    assert_eq!(
        model_changes(&session).await,
        vec!["faux/small".to_string()]
    );

    prompt_and_settle(&session, "one").await;
    prompt_and_settle(&session, "two").await;

    assert_eq!(
        model_changes(&session).await,
        vec!["faux/small".to_string()],
        "a physical selection under physical responses records nothing"
    );
}

// ----------------------------------------------------------------------------------- T7 ----

/// Opening a RESUMED session writes no `model_change`, even with an explicit `--model` that
/// disagrees with the branch — pi seeds one only for a NEW session (`sdk.ts:430-441`). The next
/// PROMPT records it, because the branch holds a VIRTUAL selection and the live one is physical, so
/// `_recordSelection`'s guard admits the write.
///
/// RED-PROVE (first half): move the `record_selection()` call out of the run entry points and into
/// `builder.rs` — the assertion that the branch still ends on `router/auto` right after `build()`
/// fails. Also guards the step-7 seeding, which must write nothing on a resume.
///
/// RED-PROVE (second half): drop the `record_selection()` call from `spawn_run` — the second
/// assertion fails and the branch still ends on `router/auto` after the prompt.
#[tokio::test]
async fn a_resumed_session_records_an_explicit_override_on_the_next_prompt() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let path = fx.branch(|m| {
        m.append_message(user("hi")).unwrap();
        m.append_model_change("router".into(), "auto".into())
            .unwrap();
        m.append_message(answered(
            "faux",
            "large",
            "openai-completions",
            StopReason::Stop,
        ))
        .unwrap();
    });

    let session = resume(
        &fx,
        &path,
        faux,
        Some(registry_with_router(&physical, "large")),
        Some("faux/small"),
    )
    .await
    .into_shared();

    assert_eq!(session.model().expect("a model").model.as_str(), "small");
    assert_eq!(
        model_changes(&session).await,
        vec!["router/auto".to_string()],
        "opening a resumed session writes no model_change"
    );

    prompt_and_settle(&session, "go").await;

    assert_eq!(
        model_changes(&session).await,
        vec!["router/auto".to_string(), "faux/small".to_string()],
        "the prompt records the override, because the branch held a VIRTUAL selection"
    );
}

// ----------------------------------------------------------------------------------- T5 ----

/// `_recordSelection`'s reason for existing: *"Tree navigation can leave the latest `model_change`
/// on another branch"*. Run in BOTH switch directions, as upstream's own restore suite does.
///
/// Shape: prompt, switch the model, prompt, navigate back to the FIRST answer (which leaves the
/// post-switch `model_change` on the abandoned branch), prompt again, then resume the file. Without
/// the record the resumed session comes up on the PRE-switch model.
///
/// RED-PROVE: delete the `record_selection()` call from `spawn_run` — both directions restore the
/// pre-switch model and both assertions fail. This test is the only reason `record_selection` is in
/// scope, so it must discriminate on that call alone.
#[tokio::test]
async fn resumes_the_selection_made_before_tree_navigation() {
    for (first, second) in [("faux/small", "router/auto"), ("router/auto", "faux/small")] {
        let fx = fixture();
        let faux = two_model_faux();
        let physical = faux.models().to_vec();
        let registry = registry_with_router(&physical, "large");

        let mut cfg = fx.config(SessionTarget::New);
        cfg.model_pattern = Some("faux/small".to_string());
        let session = SessionBuilder::new(Arc::clone(&faux) as Arc<dyn Provider>, cfg)
            .virtual_models(Arc::clone(&registry))
            .build()
            .await
            .expect("build")
            .into_shared();
        let path = session
            .session_file()
            .await
            .expect("a persisted session file");

        session.set_model(first).await.expect("select first");
        prompt_and_settle(&session, "one").await;
        // The entry to come back to: the first answer.
        let entries = session.entries_json().await;
        let anchor = entries
            .iter()
            .rev()
            .find(|e| {
                e.get("type").and_then(Value::as_str) == Some("message")
                    && e.get("message")
                        .and_then(|m| m.get("role"))
                        .and_then(Value::as_str)
                        == Some("assistant")
            })
            .and_then(|e| e.get("id").and_then(Value::as_str).map(str::to_string))
            .expect("an assistant entry to navigate back to");

        session.set_model(second).await.expect("select second");
        prompt_and_settle(&session, "two").await;

        // /tree back: the `model_change` written by `set_model(second)` is now off-branch.
        session
            .branch(cyrup_core::EntryId::from(anchor))
            .await
            .expect("navigate");
        // …and THIS prompt is where `_recordSelection` writes the live selection onto the new
        // branch. The selection itself never changed.
        prompt_and_settle(&session, "three").await;
        let live = session.model().expect("a selection");
        drop(session);

        let resumed = resume(&fx, &path, faux, Some(Arc::clone(&registry)), None).await;
        let restored = resumed.model().expect("a restored selection");
        assert_eq!(
            (restored.provider.as_str(), restored.model.as_str()),
            (live.provider.as_str(), live.model.as_str()),
            "{first} -> {second}: the post-navigation selection must survive the resume"
        );
    }
}
