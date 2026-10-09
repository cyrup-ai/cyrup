//! EXT-084 — the host's `before_agent_start` chain over ONE `systemPromptOptions` object (pi
//! `emitBeforeAgentStart`, `core/extensions/runner.ts:1420-1476` @v1.1.0): an extension's edit is
//! what the next one is handed, `systemPrompt` is re-rendered from it, a returned prompt is
//! recorded as `forceSystemPrompt`, and the reduction hands the options back.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{
    EventKind, EventPatch, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent,
    InitApi, NativeExtension,
};
use cyrup_core::{CancelToken, ExtensionId};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(String, Value)>>>;

struct Step {
    id: &'static str,
    seen: Seen,
    patch: fn(&Value) -> Option<EventPatch>,
}

#[async_trait::async_trait]
impl NativeExtension for Step {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let HostEvent::BeforeAgentStart {
            system_prompt,
            options,
            ..
        } = ev
        else {
            return HookOutcome::Noop;
        };
        self.seen
            .lock()
            .unwrap()
            .push((system_prompt.clone(), options.clone()));
        (self.patch)(options).map_or(HookOutcome::Noop, HookOutcome::Mutate)
    }
}

fn host() -> ExtensionHost {
    ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    })
}

fn base() -> Value {
    json!({"selectedTools": ["read"], "toolSnippets": {"read": "Read files"}, "cwd": "/w"})
}

fn options_patch(options: Value) -> Option<EventPatch> {
    Some(EventPatch::SystemPromptAndInject {
        system: None,
        inject: Vec::new(),
        options: Some(options),
    })
}

#[tokio::test]
async fn an_options_edit_is_rendered_for_the_next_extension_and_handed_back() {
    let host = host();
    let seen: Seen = Arc::default();
    host.load_native(Arc::new(Step {
        id: "adds",
        seen: Arc::clone(&seen),
        patch: |options| {
            let mut edited = options.clone();
            edited["sections"] = json!({"team": "TEAM_RULE"});
            options_patch(edited)
        },
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(Step {
        id: "forces",
        seen: Arc::clone(&seen),
        patch: |_| {
            Some(EventPatch::SystemPromptAndInject {
                system: Some("FORCED".into()),
                inject: Vec::new(),
                options: None,
            })
        },
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(Step {
        id: "reads",
        seen: Arc::clone(&seen),
        patch: |_| None,
    }))
    .await
    .unwrap();

    let base_text = crate::render_system_prompt_options(&base()).unwrap();
    let reduced = host
        .emit_before_agent_start("hi", json!([]), &base_text, base(), &CancelToken::new())
        .await
        .expect("the handlers changed the options");

    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].0, base_text);
    assert!(
        seen[1].0.ends_with("<team>\nTEAM_RULE\n</team>"),
        "the second extension is handed the prompt the edited options render to: {}",
        seen[1].0
    );
    assert_eq!(seen[1].1["sections"], json!({"team": "TEAM_RULE"}));
    assert_eq!(seen[2].0, "FORCED");
    assert_eq!(seen[2].1["forceSystemPrompt"], json!("FORCED"));
    assert_eq!(reduced.system_prompt.as_deref(), Some("FORCED"));
    assert_eq!(reduced.options["forceSystemPrompt"], json!("FORCED"));
    assert_eq!(reduced.options["sections"], json!({"team": "TEAM_RULE"}));
}

/// An installed renderer is the one consulted (the session installs its own), and options it
/// refuses leave the last prompt in place.
#[tokio::test]
async fn the_installed_renderer_renders_and_a_refusal_keeps_the_last_prompt() {
    let host = host();
    let seen: Seen = Arc::default();
    host.dispatcher()
        .set_system_prompt_renderer(Arc::new(
            |options: &Value| match options["sections"]["mark"].as_str() {
                Some("bad") => Err("refused".into()),
                Some(mark) => Ok(format!("RENDERED:{mark}")),
                None => Ok("RENDERED".into()),
            },
        ));
    host.load_native(Arc::new(Step {
        id: "one",
        seen: Arc::clone(&seen),
        patch: |options| {
            let mut edited = options.clone();
            edited["sections"] = json!({"mark": "a"});
            options_patch(edited)
        },
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(Step {
        id: "two",
        seen: Arc::clone(&seen),
        patch: |options| {
            let mut edited = options.clone();
            edited["sections"] = json!({"mark": "bad"});
            options_patch(edited)
        },
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(Step {
        id: "three",
        seen: Arc::clone(&seen),
        patch: |_| None,
    }))
    .await
    .unwrap();

    let reduced = host
        .emit_before_agent_start("hi", json!([]), "BASE", base(), &CancelToken::new())
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen[1].0, "RENDERED:a");
    assert_eq!(
        seen[2].0, "RENDERED:a",
        "a refused render keeps the last prompt"
    );
    assert_eq!(seen[2].1["sections"], json!({"mark": "bad"}));
    assert_eq!(reduced.options["sections"], json!({"mark": "bad"}));
}
