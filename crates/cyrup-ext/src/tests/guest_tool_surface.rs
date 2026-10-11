//! CODE-015 / CODE-017 and the structured result of a guest tool, at the host seam.
//!
//! A hand-built `cyrup:ext@0.20` component (`wat_guest`) registers a tool through
//! `registration.register-tool` and answers `execute-tool` / `prepare-loadout`, so what is under
//! test is the host's lift of the `tool-descriptor` record, its read of the `tool-output` record
//! and its call of the `prepare-loadout` export, with no `wasm32-wasip2` toolchain. The SDK-built
//! component crosses the same seam in `cyrup-it`'s `wasm_guest_tool_surface`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
#![cfg(feature = "wasm-host")]

use std::collections::HashSet;
use std::sync::Arc;

use crate::manifest::Capabilities;
use crate::tests::wat_guest::{Lowered, WatGuest, wat_str};
use crate::{ExtMode, ExtensionHost, HostConfig};
use cyrup_core::{
    CancelToken, Tool, ToolAnnotations, ToolCallId, ToolError, ToolLoadout, ToolResult,
    ToolUpdateSink,
};
use serde_json::{Value, json};

const RET: u32 = 16512;
const SCHEMA: &str = r#"{"type":"object","properties":{"n":{"type":"integer"}}}"#;
const CONTENT: &str = r#"[{"type":"text","text":"hi"}]"#;
const STRUCTURED: &str = r#"{"n":2}"#;
const CHANGES: &str =
    r#"{"hiddenDeclarations":["secret"],"descriptions":{"gtool":"described by the guest"}}"#;

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

/// What the descriptor declares beyond its name and `{}` parameters.
#[derive(Default, Clone, Copy)]
struct Declares {
    /// `output-schema-json`: `Some(true)` a valid schema, `Some(false)` text that is not JSON.
    schema: Option<bool>,
    annotations: bool,
    prepare_loadout: bool,
}

/// The canonical-ABI `tool-descriptor` (160 bytes) for the tool `gtool`: `name` 0, `parameters-json`
/// 24, `output-schema-json` 136 (tag, ptr 140, len 144), `annotations` 148 (tag, then four
/// `option<bool>` byte pairs: read-only 149, destructive 151, idempotent 153, open-world 155) and
/// `prepare-loadout` 157.
fn descriptor(declares: Declares) -> String {
    let mut bytes = [0u8; 160];
    let mut put = |offset: usize, v: u32| {
        bytes[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
    };
    put(0, 17200);
    put(4, 5); // name -> "gtool"
    put(24, 17220);
    put(28, 2); // parameters-json -> "{}"
    if let Some(valid) = declares.schema {
        put(136, 1);
        put(140, if valid { 17240 } else { 17400 });
        put(144, if valid { SCHEMA.len() as u32 } else { 8 });
    }
    if declares.annotations {
        bytes[148] = 1;
        bytes[149..151].copy_from_slice(&[1, 1]); // read-only: some(true)
        bytes[153..155].copy_from_slice(&[1, 0]); // idempotent: some(false)
    }
    bytes[157] = u8::from(declares.prepare_loadout);
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

/// A component whose `init` registers `gtool` and returns the host's answer, so a refused
/// registration fails the load.
fn component(declares: Declares, overrides: Vec<(&'static str, String)>) -> Vec<u8> {
    let mut all = vec![(
        "init",
        format!(
            "    (func (export \"init\") (result i32) \
             (call $register_tool (i32.const 17000) (i32.const {RET})) i32.const {RET})"
        ),
    )];
    all.extend(overrides);
    WatGuest {
        component: crate::tests::tool_exposure::guest_registration_tool_import(),
        lowered: vec![Lowered {
            core_name: "register_tool",
            component_func: "$register-tool",
            core_sig: "(param i32 i32)",
            needs_realloc: true,
        }],
        overrides: all,
        data: vec![
            (17000, descriptor(declares)),
            (17200, "gtool".into()),
            (17220, "{}".into()),
            (17240, wat_str(SCHEMA)),
            (17300, wat_str(CONTENT)),
            (17400, wat_str("not json")),
            (17420, wat_str(STRUCTURED)),
            (17500, wat_str(CHANGES)),
        ],
    }
    .build()
}

/// `execute-tool` answers `ok(tool-output)` with text content and a structured value, flagged as a
/// failure when `is_error`. `tool-output` is 36 bytes after the result tag: `content-json` 0,
/// `details-json` 8, `is-error` 20, `terminate` 21, `structured-content-json` 24.
fn execute_tool_answering(is_error: bool, structured: bool) -> (&'static str, String) {
    let structured = if structured {
        format!(
            "(i32.store (i32.const 18128) (i32.const 1)) \
             (i32.store (i32.const 18132) (i32.const 17420)) \
             (i32.store (i32.const 18136) (i32.const {}))",
            STRUCTURED.len()
        )
    } else {
        String::new()
    };
    (
        "execute-tool",
        format!(
            "    (func (export \"execute-tool\") (param i32 i32 i32 i32 i32 i32) (result i32) \
             (i32.store (i32.const 18104) (i32.const 17300)) \
             (i32.store (i32.const 18108) (i32.const {})) \
             (i32.store8 (i32.const 18124) (i32.const {})) \
             {structured} \
             (i32.const 18100))",
            CONTENT.len(),
            u8::from(is_error)
        ),
    )
}

/// `prepare-loadout` answers `ok(some(CHANGES))`, or traps when `trap`: a guest that must not be
/// asked.
fn prepare_loadout_answering(trap: bool) -> (&'static str, String) {
    let body = if trap {
        "unreachable".to_string()
    } else {
        format!(
            "(i32.store (i32.const 18204) (i32.const 1)) \
             (i32.store (i32.const 18208) (i32.const 17500)) \
             (i32.store (i32.const 18212) (i32.const {})) \
             (i32.const 18200)",
            CHANGES.len()
        )
    };
    (
        "prepare-loadout",
        format!(
            "    (func (export \"prepare-loadout\") (param i32 i32 i32 i32) (result i32) {body})"
        ),
    )
}

async fn loaded(bytes: &[u8]) -> (ExtensionHost, Arc<dyn Tool>) {
    let host = ExtensionHost::with_wasm(cfg()).unwrap();
    host.load_wasm_with_caps(
        "gtool-guest".into(),
        bytes,
        Arc::new(crate::DenyServices),
        &Capabilities::host_granted(),
    )
    .await
    .expect("the guest loads");
    let none = HashSet::new();
    let tool = host
        .registered_tools_filtered(&[], None, &none)
        .unwrap()
        .into_iter()
        .find(|t| t.name() == "gtool")
        .expect("the guest's tool is registered");
    (host, tool)
}

async fn run(tool: &Arc<dyn Tool>) -> Result<ToolResult, ToolError> {
    let sink: ToolUpdateSink = Box::new(|_| {});
    tool.execute(ToolCallId::from("c1"), json!({}), CancelToken::new(), sink)
        .await
}

/// CODE-017 and `outputSchema`: the descriptor's schema and annotations reach the `Tool` the agent
/// holds, and the registry's `getAllTools` row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guests_output_schema_and_annotations_reach_its_tool_and_its_tool_info_row() {
    let bytes = component(
        Declares {
            schema: Some(true),
            annotations: true,
            ..Declares::default()
        },
        Vec::new(),
    );
    let (host, tool) = loaded(&bytes).await;

    assert_eq!(
        tool.output_schema(),
        Some(&serde_json::from_str::<Value>(SCHEMA).unwrap())
    );
    assert_eq!(
        tool.annotations().copied(),
        Some(ToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: None,
            idempotent_hint: Some(false),
            open_world_hint: None,
        })
    );
    let row = host
        .registry()
        .tool_info()
        .unwrap()
        .into_iter()
        .find(|r| r["name"] == "gtool")
        .unwrap();
    assert_eq!(
        row["annotations"],
        json!({ "readOnlyHint": true, "idempotentHint": false })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_that_declares_neither_reports_neither() {
    let (host, tool) = loaded(&component(Declares::default(), Vec::new())).await;

    assert!(tool.output_schema().is_none());
    assert!(tool.annotations().is_none());
    let row = host
        .registry()
        .tool_info()
        .unwrap()
        .into_iter()
        .find(|r| r["name"] == "gtool")
        .unwrap();
    assert!(row.get("annotations").is_none(), "{row}");
}

/// An `output-schema-json` that is not JSON is refused by `register-tool`, like an unknown
/// exposure, and a guest that propagates the refusal fails its load.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unparseable_output_schema_is_refused() {
    let bytes = component(
        Declares {
            schema: Some(false),
            ..Declares::default()
        },
        Vec::new(),
    );
    let host = ExtensionHost::with_wasm(cfg()).unwrap();
    let Err(err) = host
        .load_wasm_with_caps(
            "gtool-guest".into(),
            &bytes,
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
    else {
        panic!("an unparseable output schema must fail the registration");
    };
    assert!(
        err.to_string()
            .contains("`output-schema-json` is not a JSON Schema"),
        "{err}"
    );
}

/// `tool-output.structured-content-json` and `is-error` reach the `ToolResult` the loop and a
/// codemode script read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guests_structured_content_and_failure_flag_reach_the_tool_result() {
    let declares = Declares {
        schema: Some(true),
        ..Declares::default()
    };
    let (_host, ok) = loaded(&component(
        declares,
        vec![execute_tool_answering(false, true)],
    ))
    .await;
    let result = run(&ok).await.unwrap();
    assert_eq!(result.structured_content, Some(json!({ "n": 2 })));
    assert!(!result.is_error);

    let (_host, failed) = loaded(&component(
        declares,
        vec![execute_tool_answering(true, true)],
    ))
    .await;
    let result = run(&failed).await.unwrap();
    assert!(
        result.is_error,
        "a guest's `is-error` is a failed call, not a success"
    );
    assert_eq!(
        result.structured_content,
        Some(json!({ "n": 2 })),
        "pi keeps the structured content of a failed result"
    );

    let (_host, bare) = loaded(&component(
        declares,
        vec![execute_tool_answering(false, false)],
    ))
    .await;
    assert_eq!(run(&bare).await.unwrap().structured_content, None);
}

/// A tool that does nothing but sit in the registry beside the guest's.
struct Plain(&'static str, Value);

#[async_trait::async_trait]
impl Tool for Plain {
    fn name(&self) -> &str {
        self.0
    }
    fn parameters(&self) -> &Value {
        &self.1
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::default())
    }
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn registry(guest: &Arc<dyn Tool>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::clone(guest),
        Arc::new(Plain("secret", json!({ "type": "object" }))),
    ]
}

/// CODE-015: the guest's `prepare-loadout` answer hides a declaration and rewrites a description,
/// and it is asked only when its descriptor declared the hook. The guest that does not declare it
/// traps if asked.
#[tokio::test]
async fn a_guests_prepare_loadout_is_applied_when_declared_and_never_asked_when_not() {
    let declares = Declares {
        prepare_loadout: true,
        ..Declares::default()
    };
    let (_host, hooked) =
        loaded(&component(declares, vec![prepare_loadout_answering(false)])).await;
    let loadout = ToolLoadout::resolve(&names(&["gtool", "secret"]), &registry(&hooked));
    assert_eq!(loadout.hidden_declarations().len(), 1);
    assert!(loadout.hidden_declarations().contains("secret"));
    let described = loadout
        .executable()
        .iter()
        .find(|t| t.name() == "gtool")
        .unwrap();
    assert_eq!(described.description(), "described by the guest");
    assert!(loadout.hook_failures().is_empty());

    let (_host, unhooked) = loaded(&component(
        Declares::default(),
        vec![prepare_loadout_answering(true)],
    ))
    .await;
    let loadout = ToolLoadout::resolve(&names(&["gtool", "secret"]), &registry(&unhooked));
    assert!(
        loadout.hidden_declarations().is_empty() && loadout.hook_failures().is_empty(),
        "a guest that did not declare the hook is not called, so its trap never runs"
    );
}

/// A busy instance is not waited for: the session applies the loadout from inside the very call
/// that holds the instance when a running tool changes the active set. The host answers with the
/// last changes the guest gave, and does not deadlock.
#[tokio::test]
async fn a_busy_guest_is_answered_from_its_last_loadout_changes_and_not_waited_for() {
    let declares = Declares {
        prepare_loadout: true,
        ..Declares::default()
    };
    let bytes = component(declares, vec![prepare_loadout_answering(false)]);
    let host = ExtensionHost::with_wasm(cfg()).unwrap();
    let live = host
        .load_wasm_with_caps(
            "gtool-guest".into(),
            &bytes,
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .unwrap();
    let none = HashSet::new();
    let tool = host
        .registered_tools_filtered(&[], None, &none)
        .unwrap()
        .into_iter()
        .find(|t| t.name() == "gtool")
        .unwrap();
    let requested = names(&["gtool", "secret"]);

    // Idle: the guest is asked, and its answer is kept.
    let idle = ToolLoadout::resolve(&requested, &registry(&tool));
    assert!(idle.hidden_declarations().contains("secret"));

    // Busy: the instance is held, as it is while one of its tools runs.
    let held = live.hold_instance_for_test().await;
    let busy = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        ToolLoadout::resolve(&requested, &registry(&tool))
    })
    .await
    .expect("a busy guest is not waited for");
    assert!(
        busy.hidden_declarations().contains("secret"),
        "the last answer stands while the instance is busy"
    );
    assert_eq!(
        busy.executable()
            .iter()
            .find(|t| t.name() == "gtool")
            .unwrap()
            .description(),
        "described by the guest"
    );
    drop(held);
}
