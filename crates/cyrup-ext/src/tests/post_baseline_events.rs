//! The event-API surface pi grew after the ported baseline, driven through the production host:
//! `context_with_system` (EXT-079), the remover `on()` returns (EXT-080) and
//! `ui_prompt_start`/`ui_prompt_end` (EXT-075). The WASM halves load a real component
//! ([`super::wat_guest`]) through `ExtensionHost::load_wasm_with_caps` and dispatch through the
//! host's own dispatcher and hooks.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use crate::{
    EventKind, EventPatch, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent,
    InitApi, NativeExtension,
};
use cyrup_agent::AgentMessage;
use cyrup_core::{CancelToken, Content, ExtensionId};

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

fn text_of(m: &AgentMessage) -> String {
    match m {
        AgentMessage::User { content, .. } => content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect(),
        other => format!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// EXT-079 — `context_with_system` runs after the whole `context` chain; its result is sent as
// returned (pi `emitContext`, `core/extensions/runner.ts:1190-1253` @v0.87.1).
// ---------------------------------------------------------------------------

/// A native that answers `context` with `[context_reply]` and records what `context_with_system`
/// showed it, answering that with `[cws_reply]`. Either reply may be absent.
struct ContextPhases {
    id: &'static str,
    kinds: Vec<EventKind>,
    context_reply: Option<&'static str>,
    cws_reply: Option<&'static str>,
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl NativeExtension for ContextPhases {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&self.kinds);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let (label, messages, reply) = match ev {
            HostEvent::Context { messages } => ("context", messages, self.context_reply),
            HostEvent::ContextWithSystem { messages } => ("cws", messages, self.cws_reply),
            _ => return HookOutcome::Noop,
        };
        let texts: Vec<String> = messages.iter().map(|m| text_of(m)).collect();
        self.seen
            .lock()
            .unwrap()
            .push(format!("{} {label} {texts:?}", self.id));
        match reply {
            Some(t) => HookOutcome::Mutate(EventPatch::Context {
                messages: vec![Arc::new(AgentMessage::user_text(t))],
            }),
            None => HookOutcome::Noop,
        }
    }
}

#[tokio::test]
async fn context_with_system_sees_the_context_chains_result_and_its_own_result_is_sent() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let host = ExtensionHost::new(cfg());
    // Loaded FIRST but subscribed only to the second phase: it still runs after `b`'s `context`
    // handler, because every `context` handler runs before any `context_with_system` one.
    host.load_native(Arc::new(ContextPhases {
        id: "a",
        kinds: vec![EventKind::ContextWithSystem],
        context_reply: None,
        cws_reply: Some("from-a-cws"),
        seen: seen.clone(),
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(ContextPhases {
        id: "b",
        kinds: vec![EventKind::Context, EventKind::ContextWithSystem],
        context_reply: Some("from-b-context"),
        cws_reply: None,
        seen: seen.clone(),
    }))
    .await
    .unwrap();

    let out = host
        .hooks()
        .transform_context(
            vec![Arc::new(AgentMessage::user_text("original"))],
            CancelToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            r#"b context ["original"]"#.to_string(),
            r#"a cws ["from-b-context"]"#.to_string(),
            r#"b cws ["from-a-cws"]"#.to_string(),
        ],
        "phase one is the whole `context` chain; phase two chains on its result, in load order"
    );
    assert_eq!(
        out.iter().map(|m| text_of(m)).collect::<Vec<_>>(),
        vec!["from-a-cws"],
        "the last `context_with_system` result is what the request carries"
    );
}

#[cfg(feature = "wasm-host")]
mod wasm {
    use super::*;
    use crate::host::HostServices;
    use crate::manifest::Capabilities;
    use crate::tests::wat_guest::{
        Lowered, REGISTRATION_FLAG_AND_UNSUBSCRIBE, UI_STATUS_AND_SELECT, WatGuest, wat_str,
    };
    use std::time::Duration;

    fn le32(v: u32) -> String {
        v.to_le_bytes()
            .iter()
            .map(|b| format!("\\{b:02x}"))
            .collect()
    }

    const SUBSCRIBE: Lowered = Lowered {
        core_name: "subscribe",
        component_func: "$subscribe",
        core_sig: "(param i32 i32)",
        needs_realloc: false,
    };
    const UNSUBSCRIBE: Lowered = Lowered {
        core_name: "unsubscribe",
        component_func: "$unsubscribe",
        core_sig: "(param i32 i32)",
        needs_realloc: false,
    };
    const SET_STATUS: Lowered = Lowered {
        core_name: "set_status",
        component_func: "$set-status",
        core_sig: "(param i32 i32 i32 i32 i32)",
        needs_realloc: false,
    };
    const SELECT: Lowered = Lowered {
        core_name: "select",
        component_func: "$select",
        core_sig: "(param i32 i32 i32 i32 i32 i32 i32)",
        needs_realloc: true,
    };

    /// `init` subscribes the kinds whose bytes sit at 16384.
    fn init_subscribing(count: usize) -> String {
        format!(
            "    (func (export \"init\") (result i32) \
             (call $subscribe (i32.const 16384) (i32.const {count})) i32.const 16)"
        )
    }

    fn turn_end() -> HostEvent {
        HostEvent::TurnEnd {
            turn_index: 0,
            message: AgentMessage::user_text("turn"),
            tool_results: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------------------------
    // EXT-079 on the WIT path: `on-context-with-system`'s `mutate` is decoded and sent.
    // -----------------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_guests_context_with_system_mutate_is_the_request_transcript() {
        let reply = serde_json::to_string(&vec![AgentMessage::user_text("from-guest")]).unwrap();
        let guest = WatGuest {
            component: REGISTRATION_FLAG_AND_UNSUBSCRIBE.to_string(),
            lowered: vec![SUBSCRIBE],
            overrides: vec![
                ("init", init_subscribing(1)),
                (
                    "on-context-with-system",
                    "    (func (export \"on-context-with-system\") (param i32 i32) (result i32) \
                     i32.const 16400)"
                        .to_string(),
                ),
            ],
            data: vec![
                (
                    16384,
                    format!("\\{:02x}", EventKind::ContextWithSystem as u8),
                ),
                // hook-outcome::mutate (case 2), payload string at +4.
                (
                    16400,
                    format!(
                        "\\02\\00\\00\\00{}{}",
                        le32(16416),
                        le32(reply.len() as u32)
                    ),
                ),
                (16416, wat_str(&reply)),
            ],
        };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        host.load_native(Arc::new(ContextPhases {
            id: "n",
            kinds: vec![EventKind::Context],
            context_reply: Some("from-native-context"),
            cws_reply: None,
            seen: seen.clone(),
        }))
        .await
        .unwrap();
        host.load_wasm_with_caps(
            "cws-guest".into(),
            &guest.build(),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .unwrap();

        let out = host
            .hooks()
            .transform_context(
                vec![Arc::new(AgentMessage::user_text("original"))],
                CancelToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            out.iter().map(|m| text_of(m)).collect::<Vec<_>>(),
            vec!["from-guest"]
        );
    }

    // -----------------------------------------------------------------------------------------
    // EXT-080 — a guest that unsubscribes inside its own handler is called for the current
    // dispatch and not the next one (pi `snapshotEventHandlers`, `runner.ts:265-267` @v0.87.1).
    // -----------------------------------------------------------------------------------------

    #[tokio::test]
    async fn unsubscribing_inside_its_own_handler_applies_from_the_next_dispatch() {
        let guest = WatGuest {
            component: format!("{REGISTRATION_FLAG_AND_UNSUBSCRIBE}{UI_STATUS_AND_SELECT}"),
            lowered: vec![SUBSCRIBE, UNSUBSCRIBE, SET_STATUS],
            overrides: vec![
                ("init", init_subscribing(1)),
                (
                    "on-turn-end",
                    "    (func (export \"on-turn-end\") (param i32 i32 i32 i32 i32) \
                     (call $set_status (i32.const 16400) (i32.const 3) (i32.const 0) (i32.const 0) (i32.const 0)) \
                     (call $unsubscribe (i32.const 16384) (i32.const 1)))"
                        .to_string(),
                ),
            ],
            data: vec![
                (16384, format!("\\{:02x}", EventKind::TurnEnd as u8)),
                (16400, "hit".to_string()),
            ],
        };
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        let live = host
            .load_wasm_with_caps(
                "unsub-guest".into(),
                &guest.build(),
                Arc::new(crate::DenyServices),
                &Capabilities::host_granted(),
            )
            .await
            .unwrap();
        assert!(live.guest().subscriptions().contains(EventKind::TurnEnd));

        let cancel = CancelToken::new();
        host.dispatcher()
            .dispatch_notify(&turn_end(), &cancel)
            .await;
        assert_eq!(
            live.guest().statuses().len(),
            1,
            "the dispatch in which it unsubscribed still reached the handler"
        );
        assert!(!live.guest().subscriptions().contains(EventKind::TurnEnd));

        host.dispatcher()
            .dispatch_notify(&turn_end(), &cancel)
            .await;
        assert_eq!(
            live.guest().statuses().len(),
            1,
            "the next dispatch did not call it"
        );
    }

    // -----------------------------------------------------------------------------------------
    // EXT-075 — a guest's blocking `ui.select` reaches every OTHER extension as `ui_prompt_start`
    // while the prompt is open, then `ui_prompt_end` (pi `withUIPrompt`, `runner.ts:539-566`).
    // -----------------------------------------------------------------------------------------

    /// Records the prompt events a native receives; `started` flips on the first start.
    #[derive(Default)]
    struct Watcher {
        events: Mutex<Vec<String>>,
        started: std::sync::atomic::AtomicBool,
    }

    struct WatcherExt(Arc<Watcher>);

    #[async_trait::async_trait]
    impl NativeExtension for WatcherExt {
        fn id(&self) -> ExtensionId {
            ExtensionId::from("prompt-watcher")
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
            api.subscribe(&[EventKind::UiPromptStart, EventKind::UiPromptEnd]);
            Ok(())
        }
        async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            let line = match ev {
                HostEvent::UiPromptStart { kind, title } => {
                    self.0
                        .started
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    format!("start {kind} {title:?}")
                }
                HostEvent::UiPromptEnd { kind, title } => format!("end {kind} {title:?}"),
                _ => return HookOutcome::Noop,
            };
            self.0.events.lock().unwrap().push(line);
            HookOutcome::Noop
        }
    }

    /// A prompt that stays open until the watcher has been told it opened (or 5s pass), and
    /// records which happened — the human's side of pi's microtask delivery.
    struct HeldSelect {
        watcher: Arc<Watcher>,
        saw_start_while_open: std::sync::atomic::AtomicBool,
    }

    impl HostServices for HeldSelect {
        fn select(
            &self,
            _prompt: &str,
            _options: &serde_json::Value,
            _opts: &crate::DialogOptions,
        ) -> Option<String> {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                if self
                    .watcher
                    .started
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    self.saw_start_while_open
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Some("a".into())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_guest_prompt_reaches_other_extensions_while_open_and_not_the_prompter() {
        let guest = WatGuest {
            component: format!("{REGISTRATION_FLAG_AND_UNSUBSCRIBE}{UI_STATUS_AND_SELECT}"),
            lowered: vec![SUBSCRIBE, SET_STATUS, SELECT],
            overrides: vec![
                ("init", init_subscribing(3)),
                (
                    "on-turn-end",
                    "    (func (export \"on-turn-end\") (param i32 i32 i32 i32 i32) \
                     (call $select (i32.const 16400) (i32.const 4) (i32.const 16410) (i32.const 2) \
                     (i32.const 16420) (i32.const 2) (i32.const 16448)))"
                        .to_string(),
                ),
                // The prompter's own start handler marks a status, so a self-delivery would show.
                (
                    "on-ui-prompt-start",
                    "    (func (export \"on-ui-prompt-start\") (param i32 i32 i32 i32 i32) \
                     (call $set_status (i32.const 16430) (i32.const 4) (i32.const 0) (i32.const 0) (i32.const 0)))"
                        .to_string(),
                ),
            ],
            data: vec![
                (
                    16384,
                    format!(
                        "\\{:02x}\\{:02x}\\{:02x}",
                        EventKind::TurnEnd as u8,
                        EventKind::UiPromptStart as u8,
                        EventKind::UiPromptEnd as u8
                    ),
                ),
                (16400, "Pick".to_string()),
                (16410, "[]".to_string()),
                (16420, "{}".to_string()),
                (16430, "self".to_string()),
            ],
        };
        let watcher = Arc::new(Watcher::default());
        let services = Arc::new(HeldSelect {
            watcher: watcher.clone(),
            saw_start_while_open: std::sync::atomic::AtomicBool::new(false),
        });
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        host.load_native(Arc::new(WatcherExt(watcher.clone())))
            .await
            .unwrap();
        let live = host
            .load_wasm_with_caps(
                "prompting-guest".into(),
                &guest.build(),
                services.clone(),
                &Capabilities::host_granted(),
            )
            .await
            .unwrap();

        host.dispatcher()
            .dispatch_notify(&turn_end(), &CancelToken::new())
            .await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while watcher.events.lock().unwrap().len() < 2 {
            assert!(tokio::time::Instant::now() < deadline, "no end delivered");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // A late self-delivery would land once the prompter's store is free again.
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(
            services
                .saw_start_while_open
                .load(std::sync::atomic::Ordering::SeqCst),
            "`ui_prompt_start` must reach other extensions while the prompt is still open"
        );
        assert_eq!(
            *watcher.events.lock().unwrap(),
            vec![
                r#"start select Some("Pick")"#.to_string(),
                r#"end select Some("Pick")"#.to_string(),
            ]
        );
        assert!(
            live.guest().statuses().is_empty(),
            "the prompting guest is suspended in its own import and is not delivered its own \
             prompt's events: {:?}",
            live.guest().statuses()
        );
    }
}
