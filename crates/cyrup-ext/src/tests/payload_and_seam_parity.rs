//! Regression tests for the 2026-08-14 area-06 pass.
//!
//! Every test here would be RED before its named item's change and GREEN after; the "before" state
//! is stated per test so a reviewer can revert one edit and watch exactly one test flip.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{
    CommandDescriptor, Dispatcher, EventKind, ExtError, ExtKind, ExtMode, Extension,
    ExtensionError, ExtensionHost, ExtensionRegistry, HookOutcome, HostConfig, HostCtx, HostEvent,
    InitApi, NativeExtension, Reduced, Subscriptions, ToolDescriptor,
};
use cyrup_agent::AgentEvent;
use cyrup_core::{CancelToken, ExtensionId, TerminateHint};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("/tmp/cyrup-t"),
    }
}

// ---------------------------------------------------------------------------
// EXT-029 — a run abort during a gated tool-call dispatch is not an extension fault.
// ---------------------------------------------------------------------------

/// An extension whose `tool_call` handler never returns until the token is cancelled, then reports
/// `ExtError::Cancelled` — exactly what `LiveExtension::invoke_event` / `NativeHandle` produce when
/// the run token fires mid-flight (`facade.rs`, `native.rs`, `host/instance.rs` all race
/// `cancel.cancelled()` and return `ExtError::Cancelled`).
struct CancelledGate {
    id: ExtensionId,
    subs: Subscriptions,
}

#[async_trait::async_trait]
impl Extension for CancelledGate {
    fn id(&self) -> &ExtensionId {
        &self.id
    }
    fn kind(&self) -> ExtKind {
        ExtKind::Native
    }
    fn subscriptions(&self) -> Subscriptions {
        self.subs
    }
    async fn invoke_event(
        &self,
        _ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        cancel.cancelled().await;
        Err(ExtError::Cancelled)
    }
}

/// BEFORE EXT-029: `report` fired for `ExtError::Cancelled`, so every `onError` listener — and,
/// since EXT-S03, the interactive transcript — got `Extension "<id>" error: cancelled` out of a
/// perfectly healthy extension whenever the user pressed Esc during a tool call.
///
/// pi has no such path at all: `emitToolCall`
/// (`pi/packages/coding-agent/src/core/extensions/runner.ts:932-953` @v0.83.0) takes no signal and
/// has no cancellation race.
#[tokio::test]
async fn a_run_abort_during_tool_call_dispatch_is_not_reported_as_an_extension_fault() {
    let dispatcher = Dispatcher::new();
    dispatcher
        .add(Arc::new(CancelledGate {
            id: "gate".into(),
            subs: Subscriptions::empty().with(EventKind::ToolCall),
        }))
        .unwrap();

    let errors: Arc<Mutex<Vec<ExtensionError>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = errors.clone();
    dispatcher.add_error_listener(Arc::new(move |e: &ExtensionError| {
        sink.lock().unwrap().push(e.clone());
    }));

    let cancel = CancelToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        c.cancel();
    });

    let ev = HostEvent::ToolCall {
        call_id: "tc1".into(),
        name: "bash".into(),
        input: json!({ "command": "ls" }),
    };
    let reduced = dispatcher.dispatch_block_mutate(ev, &cancel).await;

    assert!(
        errors.lock().unwrap().is_empty(),
        "an abort is the USER's action, not an extension fault; nothing may reach onError: {:?}",
        errors.lock().unwrap()
    );
    match reduced {
        // Still BLOCKS — `fails_closed` is untouched (EXT-001), the gated tool must not run.
        Reduced::Blocked { reason, .. } => assert_eq!(
            reason, None,
            "no synthesized \"Extension failed, blocking execution: cancelled\" — a reason-less \
             block lets cyrup-agent's own re-check produce pi's \"Operation aborted\" text"
        ),
        other => panic!("a cancelled fail-closed dispatch must still block, got {other:?}"),
    }
}

/// The other direction, unchanged: a REAL fault on the fail-closed seam is still reported AND still
/// synthesizes pi's blocking-fault reason. Reverting EXT-029 must not be provable by weakening this.
#[tokio::test]
async fn a_real_fault_on_the_fail_closed_seam_still_reports_and_still_names_the_fault() {
    struct Trapping(ExtensionId, Subscriptions);
    #[async_trait::async_trait]
    impl Extension for Trapping {
        fn id(&self) -> &ExtensionId {
            &self.0
        }
        fn kind(&self) -> ExtKind {
            ExtKind::Native
        }
        fn subscriptions(&self) -> Subscriptions {
            self.1
        }
        async fn invoke_event(
            &self,
            _ev: &HostEvent,
            _cancel: &CancelToken,
        ) -> Result<HookOutcome, ExtError> {
            Err(ExtError::Trap("unreachable".into()))
        }
    }

    let dispatcher = Dispatcher::new();
    dispatcher
        .add(Arc::new(Trapping(
            "trap".into(),
            Subscriptions::empty().with(EventKind::ToolCall),
        )))
        .unwrap();
    let errors: Arc<Mutex<Vec<ExtensionError>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = errors.clone();
    dispatcher.add_error_listener(Arc::new(move |e: &ExtensionError| {
        sink.lock().unwrap().push(e.clone());
    }));

    let ev = HostEvent::ToolCall {
        call_id: "tc".into(),
        name: "bash".into(),
        input: json!({}),
    };
    let reduced = dispatcher
        .dispatch_block_mutate(ev, &CancelToken::new())
        .await;

    assert_eq!(
        errors.lock().unwrap().len(),
        1,
        "a genuine trap is still surfaced"
    );
    match reduced {
        Reduced::Blocked { reason, .. } => assert!(
            reason
                .as_deref()
                .unwrap_or_default()
                .contains("Extension failed, blocking execution"),
            "pi's text (agent-session.ts:475-487 @v0.83.0) still wins for a real fault: {reason:?}"
        ),
        other => panic!("expected Blocked, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// EXT-049 — `terminate` reaches the reduction.
// ---------------------------------------------------------------------------

/// BEFORE EXT-049: `HookOutcome::Block` had only `reason`, so pi's
/// `ToolCallEventResult.terminate` (`extensions/types.ts:1072-1079` @v0.84.1) was unrepresentable —
/// a permission gate that denied hard could not end the run and the model kept retrying against a
/// gate that would deny every call.
#[tokio::test]
async fn a_blocking_handler_can_hint_terminate_and_it_survives_the_reduction() {
    struct Denier(ExtensionId, Subscriptions);
    #[async_trait::async_trait]
    impl Extension for Denier {
        fn id(&self) -> &ExtensionId {
            &self.0
        }
        fn kind(&self) -> ExtKind {
            ExtKind::Native
        }
        fn subscriptions(&self) -> Subscriptions {
            self.1
        }
        async fn invoke_event(
            &self,
            _ev: &HostEvent,
            _cancel: &CancelToken,
        ) -> Result<HookOutcome, ExtError> {
            Ok(HookOutcome::Block {
                reason: Some("denied by policy".into()),
                terminate: TerminateHint::Terminate,
            })
        }
    }

    let dispatcher = Dispatcher::new();
    dispatcher
        .add(Arc::new(Denier(
            "deny".into(),
            Subscriptions::empty().with(EventKind::ToolCall),
        )))
        .unwrap();
    let ev = HostEvent::ToolCall {
        call_id: "tc".into(),
        name: "bash".into(),
        input: json!({}),
    };
    match dispatcher
        .dispatch_block_mutate(ev, &CancelToken::new())
        .await
    {
        Reduced::Blocked {
            reason, terminate, ..
        } => {
            assert_eq!(reason.as_deref(), Some("denied by policy"));
            assert!(
                terminate.requested(),
                "pi's terminate hint must survive to the agent, which applies the \
                                every()-rule (agent-loop.ts:583 @v0.84.1)"
            );
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// EXT-014 — tool_execution_update / _end keep toolName (and args).
// ---------------------------------------------------------------------------

/// BEFORE EXT-014: `HostEvent::from_agent` discarded the rest of both arms with `..`
/// (`event.rs:445`/`:451`), while `ToolExecStart` directly above kept `tool_name` AND `args`.
/// Upstream carries them: `ToolExecutionUpdateEvent {type, toolCallId, toolName, args,
/// partialResult}` (`extensions/types.ts:770-776` @v0.83.0) and `ToolExecutionEndEvent {type,
/// toolCallId, toolName, result, isError}` (`:779-785`).
#[test]
fn tool_execution_update_and_end_carry_the_tool_name_and_the_update_carries_args() {
    let upd = HostEvent::from_agent(&AgentEvent::ToolExecutionUpdate {
        tool_call_id: "tc1".into(),
        tool_name: "bash".into(),
        args: json!({ "command": "ls -la" }),
        partial_result: json!({ "chunk": "total 0" }),
    })
    .expect("tool_execution_update maps to a HostEvent");
    match upd {
        HostEvent::ToolExecUpdate {
            call_id,
            name,
            args,
            chunk,
        } => {
            assert_eq!(call_id.as_str(), "tc1");
            assert_eq!(name, "bash");
            assert_eq!(args, json!({ "command": "ls -la" }));
            assert_eq!(chunk, json!({ "chunk": "total 0" }));
        }
        other => panic!("wrong arm: {other:?}"),
    }

    let end = HostEvent::from_agent(&AgentEvent::ToolExecutionEnd {
        tool_call_id: "tc1".into(),
        tool_name: "bash".into(),
        result: json!({ "ok": true }),
        is_error: false,
    })
    .expect("tool_execution_end maps to a HostEvent");
    match end {
        HostEvent::ToolExecEnd {
            call_id,
            name,
            is_error,
            ..
        } => {
            assert_eq!(call_id.as_str(), "tc1");
            assert_eq!(
                name, "bash",
                "an observer that missed tool_execution_start must still be \
                                      able to filter by tool"
            );
            assert!(!is_error);
        }
        other => panic!("wrong arm: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// EXT-009 — before_provider_headers exists, and a `null` value DELETES.
// ---------------------------------------------------------------------------

/// pi's doc is explicit (`extensions/types.ts:681-685` @v0.83.0): "Handlers mutate `headers` in
/// place … the return value is ignored. A `null` value deletes that header." BEFORE EXT-009 the
/// event did not exist at all — 31 kinds ending at `AgentSettled = 30`.
#[test]
fn before_provider_headers_patches_in_place_and_a_null_value_deletes_the_header() {
    assert_eq!(
        EventKind::COUNT,
        33,
        "31 + before_provider_headers + session_info_changed"
    );
    assert_eq!(
        EventKind::from_u8(31),
        Some(EventKind::BeforeProviderHeaders)
    );
    assert_eq!(
        EventKind::BeforeProviderHeaders.name(),
        "before_provider_headers"
    );
    assert_eq!(EventKind::from_u8(32), Some(EventKind::SessionInfoChanged));
    assert_eq!(EventKind::SessionInfoChanged.name(), "session_info_changed");
    assert_eq!(EventKind::from_u8(33), None, "COUNT is the exclusive bound");

    let mut ev = HostEvent::BeforeProviderHeaders {
        headers: json!({ "authorization": "Bearer x", "x-trace": "keep" }),
    };
    ev.apply_patch(crate::EventPatch::ProviderHeaders(
        json!({ "authorization": Value::Null, "x-added": "1" }),
    ));
    match ev {
        HostEvent::BeforeProviderHeaders { headers } => {
            assert!(
                headers.get("authorization").is_none(),
                "a null value DELETES"
            );
            assert_eq!(headers.get("x-added").and_then(|v| v.as_str()), Some("1"));
            assert_eq!(
                headers.get("x-trace").and_then(|v| v.as_str()),
                Some("keep")
            );
        }
        other => panic!("wrong arm: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// EXT-043 / EXT-016 — the two startup hooks carry their cwd.
// ---------------------------------------------------------------------------

struct TrustProbe {
    id: ExtensionId,
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl NativeExtension for TrustProbe {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ProjectTrust, EventKind::ResourcesDiscover]);
        Ok(())
    }
    fn decides_project_trust(&self) -> bool {
        true
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::ProjectTrust { cwd } => {
                self.seen.lock().unwrap().push(format!("trust:{cwd}"));
                HookOutcome::Handled(crate::HandledValue(json!({ "trusted": "yes" })))
            }
            HostEvent::ResourcesDiscover { cwd, reason } => {
                self.seen
                    .lock()
                    .unwrap()
                    .push(format!("resources:{cwd}:{reason}"));
                HookOutcome::Handled(crate::HandledValue(json!({})))
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// BEFORE EXT-043/EXT-016 both variants were payload-less units, so the one security-relevant hook
/// in the catalog could not key its verdict on the directory it was voting about — even though the
/// facade held the value at `HostConfig.cwd` four lines from the dispatch. pi:
/// `ProjectTrustEvent {type, cwd}` (`extensions/types.ts:519-522` @v0.83.0),
/// `ResourcesDiscoverEvent {type, cwd, reason}` (`:544-548`).
#[tokio::test]
async fn project_trust_and_resources_discover_carry_the_cwd_they_are_asked_about() {
    let host = ExtensionHost::new(cfg());
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(TrustProbe {
        id: "trust".into(),
        seen: seen.clone(),
    }))
    .await
    .unwrap();

    let cancel = CancelToken::new();
    let _ = host.aggregate_project_trust(&cancel).await;
    let _ = host.aggregate_resources(&cancel).await;

    let got = seen.lock().unwrap().clone();
    assert_eq!(
        got,
        vec![
            "trust:/tmp/cyrup-t".to_string(),
            "resources:/tmp/cyrup-t:startup".to_string()
        ],
        "both hooks see the host's cwd, and resources_discover distinguishes startup from reload"
    );
}

// ---------------------------------------------------------------------------
// EXT-030 — the materializer's own re-arm survives.
// ---------------------------------------------------------------------------

/// BEFORE EXT-030: `materialize_guest_tools` ended with `if changed { take_tools_dirty(); }`, a
/// wholesale `swap(false)` that also cleared the deliberate `mark_tools_dirty()` re-arm it raised
/// three lines earlier for a descriptor whose owner was not yet live — and any mark another
/// extension raised concurrently. That descriptor was then dropped for the rest of the session.
///
/// This pins the mechanism that made the loss possible: the quiet registration path does not raise
/// the flag, so a mark raised DURING a materialization pass is still there afterwards.
#[test]
fn a_registration_during_a_materialization_pass_is_not_swallowed() {
    let reg = ExtensionRegistry::new();
    // Simulate the materializer: it consumed the flag at entry (`refresh_tools`) …
    reg.mark_tools_dirty();
    assert!(reg.take_tools_dirty());
    // … then registers what it materialized. QUIETLY (EXT-030) — this is not new signal.
    reg.register_materialized_tool("a".into(), Arc::new(NoopTool::new("alpha")))
        .unwrap();
    assert!(
        !reg.take_tools_dirty(),
        "the materializer's OWN re-registrations must not re-dirty the flag it is already consuming"
    );

    // A concurrent registration DURING the pass, and a deliberate re-arm for a not-yet-live owner,
    // both survive: nothing clears them wholesale afterwards.
    reg.mark_tools_dirty(); // the not-yet-live re-arm
    reg.register_tool("b".into(), Arc::new(NoopTool::new("beta")))
        .unwrap(); // concurrent arrival
    assert!(
        reg.take_tools_dirty(),
        "a mark raised while the materializer ran must survive it — otherwise the tool is dropped \
         permanently for the session (pi cannot lose it: registerTool ends with refreshTools() on \
         EVERY registration, extensions/loader.ts:245-252 @v0.83.0)"
    );
}

struct NoopTool {
    name: String,
    params: Value,
}

impl NoopTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            params: json!({ "type": "object", "properties": {} }),
        }
    }
}

#[async_trait::async_trait]
impl cyrup_core::Tool for NoopTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
        Ok(cyrup_core::ToolResult::default())
    }
}

// ---------------------------------------------------------------------------
// EXT-056 — register_tool_renderer is first-wins, like every sibling table.
// ---------------------------------------------------------------------------

/// BEFORE EXT-056 this was a bare last-wins `insert`, so a later extension could re-point rendering
/// to an extension that had LOST (or never made) the tool registration — the tool executed as one
/// extension's and drew as another's. pi has no separate table: `renderCall`/`renderResult` ride on
/// the tool's own `ToolDefinition` and `getToolDefinition` returns the FIRST extension in load order
/// whose `ext.tools` map has the name (`extensions/runner.ts:463-471` @v0.83.0).
#[test]
fn a_second_extension_cannot_steal_a_tool_renderer_and_the_drop_is_diagnosable() {
    let reg = ExtensionRegistry::new();
    reg.register_tool_renderer(ExtensionId::from("first"), "bash")
        .unwrap();
    reg.register_tool_renderer(ExtensionId::from("second"), "bash")
        .unwrap();

    assert_eq!(
        reg.tool_renderer_owner("bash").unwrap(),
        Some(ExtensionId::from("first")),
        "whoever wins the tool wins its renderer"
    );
    let conflicts = reg.conflicts().unwrap();
    assert_eq!(
        conflicts.len(),
        1,
        "the drop is recorded, not silent: {conflicts:?}"
    );
    assert_eq!(conflicts[0].path, ExtensionId::from("second"));
    assert!(conflicts[0].message.contains("bash") && conflicts[0].message.contains("first"));

    // The SAME owner re-registering is not a conflict (hot reload / re-declaration).
    reg.register_tool_renderer(ExtensionId::from("first"), "bash")
        .unwrap();
    assert_eq!(reg.conflicts().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// EXT-039 / EXT-040 — shortcut resolution and the description.
// ---------------------------------------------------------------------------

/// BEFORE EXT-039 registration was a bare `insert` with no keymap comparison at all: an extension
/// binding a RESERVED key (Ctrl+C, Enter) was accepted, listed by `/hotkeys` as if live, and never
/// fired — an advertised-but-dead binding with no diagnostic; and there was no counterpart to
/// `getShortcutDiagnostics` anywhere in the workspace.
///
/// Every rule asserted here is read off pi's `getShortcuts`
/// (`pi/packages/coding-agent/src/core/extensions/runner.ts:492-534` @v0.83.0).
#[test]
fn shortcut_resolution_refuses_reserved_keys_warns_on_the_rest_and_records_every_diagnostic() {
    let reg = ExtensionRegistry::new();
    // `app.interrupt` IS in RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS (runner.ts:70-89);
    // `app.help` is not.
    let keymap = vec![
        ("app.interrupt".to_string(), vec!["ctrl+c".to_string()]),
        ("app.help".to_string(), vec!["ctrl+h".to_string()]),
    ];

    reg.register_shortcut("ext-a".into(), "Ctrl+C", Some("steal interrupt".into()))
        .unwrap();
    reg.register_shortcut("ext-a".into(), "ctrl+h", Some("override help".into()))
        .unwrap();
    reg.register_shortcut("ext-a".into(), "ctrl+t", None)
        .unwrap();
    reg.register_shortcut("ext-b".into(), "CTRL+T", Some("also ctrl+t".into()))
        .unwrap();

    let resolved = reg.resolve_shortcuts(&keymap).unwrap();
    let keys: Vec<&str> = resolved.iter().map(|(k, _)| k.as_str()).collect();

    // Rule 2 (runner.ts:513-520): a reserved collision is SKIPPED — it never enters the map.
    assert!(
        !keys.contains(&"ctrl+c"),
        "a reserved key must be refused, not silently dead: {keys:?}"
    );
    // Rule 3 (runner.ts:522-528): a NON-reserved built-in collision warns but the extension WINS.
    assert!(
        keys.contains(&"ctrl+h"),
        "a non-reserved built-in key is overridable: {keys:?}"
    );
    // Rule 4 (runner.ts:530-536): extension-vs-extension is LAST-wins — deliberately NOT the
    // first-wins rule the tool/command/renderer tables use; pi's own warning says
    // "Using ${shortcut.extensionPath}".
    assert_eq!(
        resolved
            .iter()
            .find(|(k, _)| k == "ctrl+t")
            .map(|(_, o)| o.clone()),
        Some(ExtensionId::from("ext-b")),
        "last registrant wins the key, matching pi's unconditional `extensionShortcuts.set`"
    );
    // Keys are lowercased at resolution (runner.ts:510), so `Ctrl+C`/`CTRL+T` normalize.
    assert!(keys.iter().all(|k| *k == k.to_lowercase()));

    let diags = reg.shortcut_diagnostics().unwrap();
    assert_eq!(diags.len(), 3, "one per rule that fired: {diags:?}");
    assert!(diags.iter().any(|d| {
        d.message
            .contains("conflicts with built-in shortcut. Skipping.")
    }));
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("is built-in shortcut for app.help"))
    );
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("registered by both"))
    );
}

/// EXT-039, the shape production installs — `resolve_shortcut_specs` is `resolve_shortcuts` with
/// the DESCRIPTION kept, because upstream's `getShortcuts` returns whole `ExtensionShortcut`
/// records (`extensions/types.ts:1547-1552` @v0.84.4) and `/hotkeys` renders the same map it hands
/// the editor — `shortcut.description ?? shortcut.extensionPath`
/// (`modes/interactive/interactive-mode.ts:6364-6377`).
///
/// So a key the gate REFUSES cannot be listed either, and the `?? extensionPath` fallback resolves
/// to the extension id, never to the key id.
#[test]
fn resolve_shortcut_specs_keeps_descriptions_and_drops_the_refused_key() {
    let reg = ExtensionRegistry::new();
    let keymap = vec![
        ("app.interrupt".to_string(), vec!["ctrl+c".to_string()]),
        ("app.help".to_string(), vec!["ctrl+h".to_string()]),
    ];
    reg.register_shortcut("ext-a".into(), "Ctrl+C", Some("steal interrupt".into()))
        .unwrap();
    reg.register_shortcut("ext-a".into(), "ctrl+h", Some("override help".into()))
        .unwrap();
    reg.register_shortcut("ext-b".into(), "ctrl+t", None)
        .unwrap();

    let specs = reg.resolve_shortcut_specs(&keymap).unwrap();
    assert_eq!(
        specs,
        vec![
            ("ctrl+h".to_string(), Some("override help".to_string())),
            // `description ?? extensionPath` — the OWNER, not the key id.
            ("ctrl+t".to_string(), Some("ext-b".to_string())),
        ],
        "the reserved `ctrl+c` must be absent, so `/hotkeys` cannot advertise it"
    );
    // Same rules, same diagnostics as the owner-only shape: the two must not drift.
    assert_eq!(
        reg.resolve_shortcuts(&keymap)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>(),
        specs.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()
    );
    // Rule 2 (`ctrl+c` skipped) and rule 3 (`ctrl+h` overridden) each recorded one.
    assert_eq!(reg.shortcut_diagnostics().unwrap().len(), 2);
}

/// EXT-040. BEFORE: `register_shortcut` took only `(owner, key)` and the host discarded `desc` one
/// line inside `register_shortcut` (`host/live.rs:98-101`), so `/hotkeys` printed the key id as its
/// own label. pi renders `shortcut.description ?? shortcut.extensionPath`
/// (`modes/interactive/interactive-mode.ts:5856` @v0.83.0) — the fallback is the extension, never
/// the key.
#[test]
fn a_shortcut_description_survives_registration_and_falls_back_to_the_extension_id() {
    let reg = ExtensionRegistry::new();
    reg.register_shortcut(
        "fleet".into(),
        "ctrl+alt+f",
        Some("Show the subagent fleet".into()),
    )
    .unwrap();
    reg.register_shortcut("plain".into(), "ctrl+alt+g", None)
        .unwrap();

    let specs = reg.shortcut_specs().unwrap();
    assert_eq!(specs[0].0, "ctrl+alt+f");
    assert_eq!(specs[0].1.as_deref(), Some("Show the subagent fleet"));
    assert_eq!(
        specs[1].1.as_deref(),
        Some("plain"),
        "no description falls back to the extension ID, as pi does — never to the key id"
    );
}

// ---------------------------------------------------------------------------
// EXT-018 / EXT-057 — the bus reaches natives, and its faults are visible.
// ---------------------------------------------------------------------------

struct BusNative {
    id: ExtensionId,
    got: Arc<Mutex<Vec<(String, Value)>>>,
    fail: bool,
}

#[async_trait::async_trait]
impl NativeExtension for BusNative {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe_bus("demo:bus");
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    async fn on_bus_event(
        &self,
        topic: &str,
        payload: &Value,
        _ctx: &HostCtx,
    ) -> Result<(), ExtError> {
        if self.fail {
            return Err(ExtError::Panicked("bus listener exploded".into()));
        }
        self.got
            .lock()
            .unwrap()
            .push((topic.to_string(), payload.clone()));
        Ok(())
    }
}

/// BEFORE EXT-018 the bus was `#[cfg(feature = "wasm-host")]` and `deliver_bus_events` resolved
/// subscribers out of `self.live` only, so the three extensions cyrup actually ships — all natives —
/// had no `pi.events` at all. pi hangs the ONE bus on the ONE `ExtensionAPI` it builds for every
/// extension it loads (`events: eventBus,`, `extensions/loader.ts:389` @v0.83.0).
#[tokio::test]
async fn a_native_extension_receives_inter_extension_bus_events() {
    let host = ExtensionHost::new(cfg());
    let got = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(BusNative {
        id: "listener".into(),
        got: got.clone(),
        fail: false,
    }))
    .await
    .unwrap();

    host.bus()
        .emit("demo:bus".into(), json!({ "hello": "world" }));
    host.deliver_bus_events(&CancelToken::new()).await;

    let got = got.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "a native subscriber must be reached: {got:?}");
    assert_eq!(got[0].0, "demo:bus");
    assert_eq!(got[0].1, json!({ "hello": "world" }));
}

/// EXT-057b. BEFORE: a contained `bus_deliver` fault was `tracing::warn!` only, so it could never
/// reach `App::show_extension_error` / the `[Extension issues]` surface EXT-S03 exists to make
/// faults visible in. pi's `on` wrapper does surface handler faults (`catch (err) {
/// console.error(\`Event handler error (${channel}):\`, err); }`, `core/event-bus.ts` @v0.83.0).
#[tokio::test]
async fn a_faulting_bus_listener_reaches_the_error_listener_channel() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(BusNative {
        id: "boom".into(),
        got: Arc::new(Mutex::new(Vec::new())),
        fail: true,
    }))
    .await
    .unwrap();

    let errors: Arc<Mutex<Vec<ExtensionError>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = errors.clone();
    host.add_error_listener(Arc::new(move |e: &ExtensionError| {
        sink.lock().unwrap().push(e.clone());
    }));

    host.bus().emit("demo:bus".into(), json!({}));
    host.deliver_bus_events(&CancelToken::new()).await;

    let errors = errors.lock().unwrap().clone();
    assert_eq!(
        errors.len(),
        1,
        "the fault must be visible, not just logged: {errors:?}"
    );
    assert_eq!(errors[0].extension, ExtensionId::from("boom"));
    assert!(errors[0].error.contains("demo:bus"));
}

/// EXT-057a. BEFORE: reaching `MAX_ROUNDS` fell out of the `for` loop with events still sitting in
/// `SharedBus.pending` — no diagnostic, no error, no record that anything was dropped, so a chatty
/// A→B→A topic pattern lost messages silently. pi cannot drop anything: `emit` runs every listener
/// synchronously over a node `EventEmitter` (`core/event-bus.ts:12-32` @v0.83.0), so the round bound
/// is a cyrup-original mechanism and its failure must be a cyrup-original diagnostic.
#[tokio::test]
async fn exhausting_the_bus_round_bound_drops_explicitly_and_says_so() {
    struct PingPong {
        id: ExtensionId,
        bus: Arc<crate::SharedBus>,
        rounds: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl NativeExtension for PingPong {
        fn id(&self) -> ExtensionId {
            self.id.clone()
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
            api.subscribe_bus("demo:pingpong");
            Ok(())
        }
        async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            HookOutcome::Noop
        }
        async fn on_bus_event(
            &self,
            topic: &str,
            _payload: &Value,
            _ctx: &HostCtx,
        ) -> Result<(), ExtError> {
            // Emit on every delivery: the queue can never drain, so the bound must fire.
            self.rounds.fetch_add(1, Ordering::Relaxed);
            self.bus.emit(topic.to_string(), json!({}));
            Ok(())
        }
    }

    let host = ExtensionHost::new(cfg());
    let rounds = Arc::new(AtomicUsize::new(0));
    host.load_native(Arc::new(PingPong {
        id: "pong".into(),
        bus: host.bus().clone(),
        rounds: rounds.clone(),
    }))
    .await
    .unwrap();

    let errors: Arc<Mutex<Vec<ExtensionError>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = errors.clone();
    host.add_error_listener(Arc::new(move |e: &ExtensionError| {
        sink.lock().unwrap().push(e.clone());
    }));

    host.bus().emit("demo:pingpong".into(), json!({}));
    host.deliver_bus_events(&CancelToken::new()).await;

    assert_eq!(
        rounds.load(Ordering::Relaxed),
        64,
        "the bound is MAX_ROUNDS"
    );
    let errors = errors.lock().unwrap().clone();
    assert_eq!(
        errors.len(),
        1,
        "exactly one diagnostic names the bound: {errors:?}"
    );
    assert!(errors[0].error.contains("64"), "{:?}", errors[0]);
    assert!(errors[0].error.contains("dropped"), "{:?}", errors[0]);
    assert_eq!(
        host.bus().pending_len(),
        0,
        "the remainder is dropped EXPLICITLY, not left queued"
    );
}

// ---------------------------------------------------------------------------
// EXT-050 — a subscription can be taken down.
// ---------------------------------------------------------------------------

/// BEFORE EXT-050 `interface bus` was emit + subscribe only, with no unsubscribe and no handle, so a
/// `bus.subscribe` was permanent for the instance's life. pi's `on()` has always returned an
/// unsubscribe closure (`core/event-bus.ts:18-27` @v0.83.0) and the loader tracks it since v0.84.1
/// (`extensions/loader.ts:413-421`).
#[test]
fn a_bus_subscription_can_be_taken_down_individually_and_per_owner() {
    let bus = crate::SharedBus::new();
    bus.subscribe("a".into(), "t1".into());
    bus.subscribe("a".into(), "t2".into());
    bus.subscribe("b".into(), "t1".into());

    assert!(bus.unsubscribe(&ExtensionId::from("a"), "t1"));
    assert!(
        !bus.unsubscribe(&ExtensionId::from("a"), "t1"),
        "idempotent"
    );
    assert_eq!(bus.subscribers_for("t1"), vec![ExtensionId::from("b")]);

    // The teardown pi's `invalidate()` performs (loader.ts:206-214 @v0.84.1).
    assert_eq!(bus.unsubscribe_all(&ExtensionId::from("a")), 1);
    assert!(bus.subscribers_for("t2").is_empty());
}

/// EXT-050's other half: pi's `events` wrapper calls `runtime.assertActive()` before BOTH `emit` and
/// `on` (`extensions/loader.ts:413-421` @v0.84.1), and `invalidate()` (`:208-214`) sets the stale
/// message and runs every tracked unsubscribe. cyrup had neither: a whole-bus `clear()` inside
/// `reload` was the only teardown, and an instance that had already been replaced could still
/// publish onto the bus the FRESH set was listening on.
#[cfg(feature = "wasm-host")]
#[test]
fn an_invalidated_instance_loses_its_subscriptions_and_may_no_longer_publish() {
    use crate::GuestState;
    use std::sync::Arc;

    let bus = Arc::new(crate::SharedBus::new());
    let guest =
        GuestState::new("a".into(), Arc::new(ExtensionRegistry::new())).with_bus(bus.clone());

    // PRESENCE first — while active, both halves work.
    guest.bus_subscribe("t1".into());
    assert_eq!(bus.subscribers_for("t1"), vec![ExtensionId::from("a")]);
    guest.bus_emit("t1".into(), serde_json::json!({"n": 1}));
    assert_eq!(bus.pending_len(), 1, "an active instance publishes");
    assert!(guest.stale_reason().is_none());

    guest.invalidate(None);

    // pi's `invalidate` runs every tracked unsubscribe...
    assert!(
        bus.subscribers_for("t1").is_empty(),
        "invalidate tears down this owner's subscriptions"
    );
    // ...and `assertActive` refuses both verbs afterwards.
    guest.bus_emit("t1".into(), serde_json::json!({"n": 2}));
    assert_eq!(
        bus.pending_len(),
        1,
        "a stale instance may not publish onto the fresh set's bus"
    );
    guest.bus_subscribe("t1".into());
    assert!(bus.subscribers_for("t1").is_empty(), "nor re-subscribe");

    // Idempotent, and the FIRST reason wins (upstream `if (state.staleMessage) return;`).
    let first = guest
        .stale_reason()
        .expect("a stale instance carries its reason");
    guest.invalidate(Some("a later, vaguer reason".into()));
    assert_eq!(guest.stale_reason().as_deref(), Some(first.as_str()));
}

// ---------------------------------------------------------------------------
// EXT-017 — a colliding command is reachable at its own suffixed name.
// ---------------------------------------------------------------------------

/// BEFORE EXT-017 `live_for_command` resolved through `command_owner` — a last-wins `HashMap`
/// lookup on the RAW name — so when two extensions registered `deploy` only the LAST registrant was
/// executable and the other was silently unreachable. pi disambiguates in LOAD ORDER with a
/// `takenInvocationNames` bump loop (`extensions/runner.ts:598-631` @v0.83.0) and
/// `name: cmd.invocationName` is what reaches autocomplete (`interactive-mode.ts:605`).
#[test]
fn a_second_extension_registering_deploy_is_reachable_as_deploy_2() {
    let reg = ExtensionRegistry::new();
    reg.register_command("first".into(), "deploy", CommandDescriptor::default())
        .unwrap();
    reg.register_command("second".into(), "deploy", CommandDescriptor::default())
        .unwrap();

    let resolved = reg.resolved_commands().unwrap();
    let names: Vec<&str> = resolved
        .iter()
        .map(|r| r.invocation_name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["deploy:1", "deploy:2"],
        "load-order suffixing, pi's rule"
    );

    // The lookup `ExtensionHost::live_for_command` now uses reaches BOTH owners.
    assert_eq!(
        reg.resolved_command_owner("deploy:1").unwrap(),
        Some(ExtensionId::from("first"))
    );
    assert_eq!(
        reg.resolved_command_owner("deploy:2").unwrap(),
        Some(ExtensionId::from("second"))
    );
}

// ---------------------------------------------------------------------------
// EXT-023 / EXT-024 — the two descriptor fields reach the registry.
// ---------------------------------------------------------------------------

/// BEFORE: the string `prepare` did not occur in `world.wit` at all and `tool-descriptor` had eight
/// fields, so pi's `prepareArguments` (`extensions/types.ts:468` @v0.83.0) and `renderShell`
/// (`:465`) were unrepresentable — the SDK accepted both and `lower_tool_descriptor` copied 8 of 10
/// fields into a different struct by struct literal, so there was no compile error and no warning.
#[test]
fn the_tool_descriptor_carries_prepare_arguments_and_render_shell() {
    let d = ToolDescriptor {
        name: "coerce".into(),
        label: "Coerce".into(),
        description: String::new(),
        parameters: json!({ "type": "object", "properties": {} }),
        execution_mode: None,
        prompt_snippet: None,
        prompt_guidelines: Vec::new(),
        has_renderer: false,
        prepare_arguments: true,
        render_shell: Some("self".into()),
        constrained_sampling: None,
    };
    d.validate().unwrap();

    // Both survive a serde round trip in pi's camelCase wire shape.
    let wire = serde_json::to_value(&d).unwrap();
    assert_eq!(wire.get("prepareArguments"), Some(&json!(true)));
    assert_eq!(wire.get("renderShell"), Some(&json!("self")));
    let back: ToolDescriptor = serde_json::from_value(wire).unwrap();
    assert!(back.prepare_arguments);
    assert_eq!(back.render_shell.as_deref(), Some("self"));
}

/// PROV-011 / EXT-024 — `constrainedSampling` on the descriptor, and a `WasmTool` built from it
/// answering `Tool::constrained_sampling`.
///
/// BEFORE: `tool-descriptor` had no such field and `cyrup_core::Tool` had no such accessor, so a
/// guest tool asking for grammar-constrained sampling was answered with silence at BOTH ends —
/// the provider-side resolvers (`cyrup-provider/src/utils/constrained_sampling.rs`) existed and
/// could never be reached from a tool.
#[test]
fn the_tool_descriptor_carries_constrained_sampling_in_pis_wire_shape() {
    use cyrup_core::constrained_sampling::{
        ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants, StrictSampling,
    };

    // The ABSENCE side first, so the presence assertions below cannot pass vacuously.
    let absent = ToolDescriptor {
        name: "plain".into(),
        label: "Plain".into(),
        description: String::new(),
        parameters: json!({ "type": "object", "properties": {} }),
        execution_mode: None,
        prompt_snippet: None,
        prompt_guidelines: Vec::new(),
        has_renderer: false,
        prepare_arguments: false,
        render_shell: None,
        constrained_sampling: None,
    };
    assert!(
        serde_json::to_value(&absent)
            .unwrap()
            .get("constrainedSampling")
            .is_none()
    );

    // pi's grammar config: `{"type":"grammar","variants":{"openai_lark":"…"}}`.
    let grammar = ToolDescriptor {
        constrained_sampling: Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::Grammar {
                variants: GrammarVariants {
                    openai_lark: Some("start: /[a-z]+/".into()),
                    openai_regex: None,
                },
            },
        )),
        ..absent.clone()
    };
    let wire = serde_json::to_value(&grammar).unwrap();
    assert_eq!(
        wire.get("constrainedSampling"),
        Some(&json!({"type": "grammar", "variants": {"openai_lark": "start: /[a-z]+/"}}))
    );
    let back: ToolDescriptor = serde_json::from_value(wire).unwrap();
    assert!(matches!(
        back.constrained_sampling.as_ref().and_then(|c| c.config()),
        Some(ConstrainedSamplingConfig::Grammar { .. })
    ));

    // pi's `false` is the bare literal, NOT an object, and resolves to "no config".
    let disabled = ToolDescriptor {
        constrained_sampling: Some(ConstrainedSampling::Disabled(false)),
        ..absent.clone()
    };
    let wire = serde_json::to_value(&disabled).unwrap();
    assert_eq!(wire.get("constrainedSampling"), Some(&json!(false)));
    let back: ToolDescriptor = serde_json::from_value(wire).unwrap();
    assert!(
        back.constrained_sampling
            .as_ref()
            .unwrap()
            .config()
            .is_none()
    );

    // And the strict-JSON-schema arm keeps pi's snake_case tag + lowercase strictness.
    let strict = ToolDescriptor {
        constrained_sampling: Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::JsonSchema {
                strict: StrictSampling::Require,
            },
        )),
        ..absent
    };
    assert_eq!(
        serde_json::to_value(&strict)
            .unwrap()
            .get("constrainedSampling"),
        Some(&json!({"type": "json_schema", "strict": "require"}))
    );
}

// ---------------------------------------------------------------------------
// EXT-046 — set_label can clear.
// ---------------------------------------------------------------------------

/// BEFORE EXT-046 `HostServices::set_label` took `&str`, so a label could be SET and never CLEARED —
/// and an empty string does not clear it either, it writes an empty label. pi:
/// `setLabel(entryId: string, label: string | undefined)` (`extensions/types.ts:1314` @v0.83.0),
/// "Set or clear a label on an entry."
#[cfg(feature = "wasm-host")]
#[test]
fn set_label_can_clear_a_label_not_just_replace_it() {
    use crate::host::{HostServices, RecordingServices};
    let svc = RecordingServices::default();
    svc.set_label("e1", Some("bookmark"));
    svc.set_label("e1", None);
    // The signature accepts the clear at all — before EXT-046 this line did not compile.
    let _: &dyn HostServices = &svc;
}

// ---------------------------------------------------------------------------
// EXT-033 — a configured path that resolves to nothing is reported.
// ---------------------------------------------------------------------------

/// BEFORE EXT-033's diagnostic half: a configured path that exists but is neither an extension dir
/// nor a `.wasm`, AND a nonexistent path, both fell into `scan_dir`, whose first statement is
/// `let Ok(rd) = read_dir(dir) else { return };` — a silent return yielding neither `loaded` nor
/// `errors`, so a typo'd `-e` was indistinguishable from a correct one.
///
/// pi guards the same shapes and surfaces the miss as a per-path
/// `LoadExtensionsResult.errors` entry (`extensions/loader.ts:704-717` @v0.83.0).
#[test]
fn a_configured_path_that_resolves_to_nothing_produces_exactly_one_diagnostic_naming_it() {
    let missing = std::path::PathBuf::from("/definitely/not/here/ext-abc123");
    let roots = crate::DiscoveryRoots {
        project_cwd: None,
        agent_dir: None,
        configured: vec![missing.clone()],
        disabled: Vec::new(),
    };
    let (found, diags) = crate::loader::discover_with_diagnostics(&roots);
    assert!(found.is_empty());
    assert_eq!(diags.len(), 1, "one entry, naming the path: {diags:?}");
    assert_eq!(diags[0].path, missing);
    assert!(diags[0].error.contains("does not exist"));
    assert!(!diags[0].fatal, "pi keeps going; only the message is added");

    // An existing NON-extension path is the other half of the same hole.
    let dir = std::env::temp_dir().join("cyrup-ext033-not-an-extension");
    std::fs::create_dir_all(&dir).unwrap();
    let roots = crate::DiscoveryRoots {
        project_cwd: None,
        agent_dir: None,
        configured: vec![dir.clone()],
        disabled: Vec::new(),
    };
    let (found, diags) = crate::loader::discover_with_diagnostics(&roots);
    assert!(found.is_empty());
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(diags[0].error.contains("neither"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// DRIFT-004 / SEAM-015 — `UserBashEventResult.operations` survives the reduction.
// ---------------------------------------------------------------------------

/// A `user_bash` handler that returns BOTH halves of pi's `UserBashEventResult`
/// (`extensions/types.ts:1076-1082` @v0.83.0): an `operations` backend override AND a `result`.
/// Upstream's two fields are independent — `rpc-mode.ts:566-579` short-circuits on `result` and
/// otherwise threads `operations` into `executeBash` — so a reduction that carried only one of them
/// would silently drop the other.
struct BashRedirect {
    id: ExtensionId,
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl NativeExtension for BashRedirect {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::UserBash]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::UserBash {
                command,
                exclude_from_context,
                cwd,
            } => {
                self.seen
                    .lock()
                    .unwrap()
                    .push(format!("{command}:{exclude_from_context}:{cwd}"));
                HookOutcome::Handled(crate::HandledValue(json!({
                    "operations": { "backend": "ssh", "remote": "build-box" },
                    "result": { "output": "Linux build-box\n", "exitCode": 0 },
                })))
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// The `operations` half of `UserBashEventResult` is NOT lost at the `cyrup-ext` boundary.
///
/// This test exists to pin WHERE the DRIFT-004 / SEAM-015 omission actually lives, because the item
/// was filed against `crates/cyrup-modes/src/rpc.rs` and that is the wrong site twice over:
///
///  1. `rpc.rs`'s literal `None` is the `on_chunk` argument — the port of pi's own `undefined`
///     second argument to `session.executeBash(command.command, undefined, {…})`
///     (`rpc-mode.ts:573` @v0.83.0). Nothing is dropped there.
///  2. `cyrup-ext` carries a `handled` payload through `decode_outcome`
///     (`host/live.rs`: `HookOutcome::Handled(s)` -> `serde_json::from_str` VERBATIM, with no
///     per-event key filter — `decode_patch`'s per-kind shaping applies to `mutate` only) and out of
///     [`ExtensionHost::emit_user_bash`] as the whole `UserBashReduction::Handled(Value)`.
///
/// The drop used to be downstream of both, in `cyrup-session-svc`. It no longer is: `BashOptions`
/// has an `operations` field and `execute_bash_with_user_event` fills it from the winning
/// `user_bash` result (SEAM-015). The KEY tested here is still the one a WASM guest can put in the
/// payload today, and the assertion below fails the moment anyone "fixes" anything by filtering
/// `operations` out at this boundary. Putting a CALLABLE behind the key is the OTHER half and is no
/// longer open: a native extension supplies its backend through
/// `NativeExtension::user_bash_operations`, and a guest through the
/// `register-bash-operations` + `bash-operations-exec` round-trip (DRIFT-004,
/// `crate::tests::bash_operations_seam` and `cyrup-it/tests/ext/wasm_bash_operations.rs`). This
/// test is still about the PAYLOAD: the key must survive the reduction whichever tier reads it.
///
/// Presence before absence: the `result` half is asserted first, so a reduction that dropped the
/// whole payload could not pass by vacuously satisfying the `operations` check.
#[tokio::test]
async fn user_bash_reduction_carries_the_operations_half_not_only_the_result_half() {
    let host = ExtensionHost::new(cfg());
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(BashRedirect {
        id: "bash-redirect".into(),
        seen: seen.clone(),
    }))
    .await
    .unwrap();

    let cancel = CancelToken::new();
    let reduced = host.emit_user_bash("uname -a", &cancel).await;

    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "the handler must have been reached"
    );

    let v = match reduced {
        crate::UserBashReduction::Handled(v) => v,
        other => panic!("wrong arm: {other:?}"),
    };
    // Presence: the half cyrup already consumes.
    assert_eq!(v["result"]["output"], json!("Linux build-box\n"));
    assert_eq!(v["result"]["exitCode"], json!(0));
    // The half under test: pi `UserBashEventResult.operations` (`extensions/types.ts:1078-1080`).
    assert_eq!(
        v["operations"],
        json!({ "backend": "ssh", "remote": "build-box" }),
        "the `operations` override must reach the caller intact; the seam that acts on it is \
         `cyrup_tools::ops::BashOperations`, supplied by a native extension directly or by a WASM \
         guest over the `bash-operations-exec` round-trip (DRIFT-004; see the CYRUP-DELTA register \
         in `crates/cyrup-ext/src/lib.rs`)"
    );
}
