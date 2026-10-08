//! EXT-082 — a registration pi's `ExtensionAPI` THROWS on fails the load, with pi's message.
//!
//! pi `createExtensionAPI` (`core/extensions/loader.ts` @v0.87.1): `registerTool` throws
//! `Tool "<name>" registered by extension "<path>" must define an object parameter schema.` for a
//! non-object `parameters` (`:274-279`, added v0.86.0 #9300), and `registerFlag` throws
//! `Invalid default for flag "<name>": expected <type>, got <typeof default>` (`:312-316`, added
//! v0.84.3 #8123). Both throw inside the factory, so `initializeExtension` discards the extension and
//! the load is reported as failed.
//!
//! Before the fix the WASM import built `parameters` with `unwrap_or(Value::Null)`, discarded
//! `register_guest_tool`'s refusal with `let _ =`, and returned `()` — the guest's `init` succeeded
//! with the tool silently absent. A mistyped flag default was stored and served on both paths.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use crate::registry::ExtensionRegistry;
use crate::{
    ExtError, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi,
    NativeExtension,
};
use cyrup_core::{ExtensionId, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use serde_json::{Value, json};

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

struct Schemaless(Value);

#[async_trait::async_trait]
impl Tool for Schemaless {
    fn name(&self) -> &str {
        "ext082_tool"
    }
    fn parameters(&self) -> &Value {
        &self.0
    }
    async fn execute(
        &self,
        _id: ToolCallId,
        _args: Value,
        _cancel: cyrup_core::CancelToken,
        _sink: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::default())
    }
}

/// A native extension registering one tool with `parameters` and one flag with `flag_spec`.
struct Registers {
    parameters: Value,
    flag_spec: Value,
}

#[async_trait::async_trait]
impl NativeExtension for Registers {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("ext082-native")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::new(Schemaless(self.parameters.clone())));
        api.register_flag("ext082_flag", self.flag_spec.clone());
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

const OBJECT_SCHEMA: fn() -> Value = || json!({"type": "object", "properties": {}});

#[tokio::test]
async fn a_native_tool_without_an_object_schema_fails_the_load_naming_tool_and_extension() {
    let host = ExtensionHost::new(cfg());
    let err = host
        .load_native(Arc::new(Registers {
            parameters: json!([]),
            flag_spec: json!({"type": "boolean", "default": false}),
        }))
        .await
        .expect_err("pi's registerTool throws, so the load fails");
    assert_eq!(
        err.to_string(),
        "Tool \"ext082_tool\" registered by extension \"ext082-native\" must define an object \
         parameter schema."
    );
    assert!(host.registry().tool("ext082_tool").unwrap().is_none());
}

#[tokio::test]
async fn a_native_flag_whose_default_mismatches_its_type_fails_the_load_with_pis_message() {
    let host = ExtensionHost::new(cfg());
    let err = host
        .load_native(Arc::new(Registers {
            parameters: OBJECT_SCHEMA(),
            flag_spec: json!({"type": "boolean", "default": "yes"}),
        }))
        .await
        .expect_err("pi's registerFlag throws, so the load fails");
    assert_eq!(
        err.to_string(),
        "Invalid default for flag \"ext082_flag\": expected boolean, got string"
    );
    assert!(host.registry().get_flag("ext082_flag").unwrap().is_none());
}

/// Control: a well-formed tool and a flag with a matching (or absent) default load.
#[tokio::test]
async fn well_formed_registrations_load() {
    for flag_spec in [
        json!({"type": "boolean", "default": true}),
        json!({"type": "string", "default": "x"}),
        json!({"type": "string"}),
        // `typeof null` is `"object"`, so a `null` default passes only an `"object"` type.
        json!({"type": "object", "default": null}),
    ] {
        let host = ExtensionHost::new(cfg());
        host.load_native(Arc::new(Registers {
            parameters: OBJECT_SCHEMA(),
            flag_spec: flag_spec.clone(),
        }))
        .await
        .unwrap_or_else(|e| panic!("{flag_spec} must load: {e}"));
        assert!(host.registry().tool("ext082_tool").unwrap().is_some());
    }
}

/// The message pi's `registerFlag` throws for `spec`, or `None` when it accepts it.
fn flag_refusal(spec: Value) -> Option<String> {
    ExtensionRegistry::new()
        .register_flag(ExtensionId::from("ext082-flags"), "x", spec)
        .err()
        .map(|e| e.to_string())
}

/// EXT-082 (1) — only an ABSENT default is pi's `undefined`. An explicit `null` is JS `null`, whose
/// `typeof` is `"object"`, so pi's `default !== undefined && typeof default !== type` throws.
#[test]
fn an_explicit_null_flag_default_is_refused_as_an_object() {
    assert_eq!(
        flag_refusal(json!({"type": "boolean", "default": null})).as_deref(),
        Some("Invalid default for flag \"x\": expected boolean, got object")
    );
    assert_eq!(
        flag_refusal(json!({"type": "boolean"})),
        None,
        "absent = undefined"
    );
}

/// EXT-082 (2) — pi interpolates `options.type` raw into its template literal, and compares it
/// with `!==`, so a non-string `type` never matches and renders as JS `String(type)` would.
#[test]
fn a_non_string_flag_type_is_interpolated_as_javascript_renders_it() {
    for (ty, default, expected) in [
        (json!(5), json!(true), "expected 5, got boolean"),
        (json!(1.5), json!("a"), "expected 1.5, got string"),
        (json!(1e21), json!(1), "expected 1e+21, got number"),
        (json!(1.5e-7), json!(1), "expected 1.5e-7, got number"),
        (json!(0.000001), json!(1), "expected 0.000001, got number"),
        (json!(true), json!(1), "expected true, got number"),
        (json!(null), json!(1), "expected null, got number"),
        (
            json!({"a": 1}),
            json!(1),
            "expected [object Object], got number",
        ),
        // `["boolean"] !== "boolean"`, so even a matching-looking array throws.
        (
            json!(["boolean"]),
            json!(true),
            "expected boolean, got boolean",
        ),
        (
            json!([1, null, "s"]),
            json!(true),
            "expected 1,,s, got boolean",
        ),
    ] {
        assert_eq!(
            flag_refusal(json!({"type": ty, "default": default})),
            Some(format!("Invalid default for flag \"x\": {expected}")),
            "type {ty}"
        );
    }
    assert_eq!(
        flag_refusal(json!({"default": true})).as_deref(),
        Some("Invalid default for flag \"x\": expected undefined, got boolean"),
        "an absent type is `undefined`"
    );
}

// ---------------------------------------------------------------------------
// The WASM path. Since world 0.13 `register-tool` / `register-flag` return the refusal to the
// guest as their `err` arm: pi's throw as a value, and that is its whole effect. A guest that
// propagates it fails its own call (an `init` that returns it fails the load); a guest that handles
// it carries on, as a pi `try`/`catch` around the throw does, with nothing registered.
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm-host")]
fn guest_host(id: &str) -> (Arc<ExtensionRegistry>, crate::host::HostState) {
    use crate::host::{GuestState, HostState, StoreLimits};
    let registry = Arc::new(ExtensionRegistry::new());
    let guest = Arc::new(GuestState::new(ExtensionId::from(id), registry.clone()));
    let state = HostState::with_guest(StoreLimits::default(), guest);
    (registry, state)
}

#[cfg(feature = "wasm-host")]
fn wit_tool(
    parameters_json: &str,
) -> crate::host::live::bindings::cyrup::ext::types::ToolDescriptor {
    crate::host::live::bindings::cyrup::ext::types::ToolDescriptor {
        name: "ext082_guest_tool".into(),
        label: "ext082".into(),
        description: String::new(),
        parameters_json: parameters_json.into(),
        exec_mode: None,
        prompt_snippet: None,
        prompt_guidelines: Vec::new(),
        has_renderer: false,
        prepare_arguments: false,
        render_shell: None,
        constrained_sampling: None,
        exposure: None,
        namespace: None,
        default_active: None,
    }
}

#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_guest_tool_without_an_object_schema_is_refused_to_the_guest_with_pis_message() {
    use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;

    for parameters_json in ["[]", "not json"] {
        let (registry, mut state) = guest_host("ext082-guest");
        let returned = state.register_tool(wit_tool(parameters_json)).await;
        assert_eq!(
            returned.as_ref().map_err(String::as_str),
            Err(
                "Tool \"ext082_guest_tool\" registered by extension \"ext082-guest\" must \
                 define an object parameter schema."
            ),
            "{parameters_json:?}: the guest is handed pi's message as the import's `err` arm"
        );
        assert!(!registry.has_guest_tool("ext082_guest_tool").unwrap());
    }
}

#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_guest_flag_whose_default_mismatches_its_type_is_refused_and_not_served() {
    use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;

    let (registry, mut state) = guest_host("ext082-guest-flag");
    let returned = state
        .register_flag(
            "ext082_flag".into(),
            json!({"type": "boolean", "default": "yes"}).to_string(),
        )
        .await;
    assert_eq!(
        returned.as_ref().map_err(String::as_str),
        Err("Invalid default for flag \"ext082_flag\": expected boolean, got string"),
        "the guest is handed pi's message as the import's `err` arm"
    );
    assert_eq!(
        state.get_flag("ext082_flag".into()).await,
        None,
        "a refused flag is not stored, so `getFlag` cannot serve its string default"
    );
    assert!(registry.get_flag("ext082_flag").unwrap().is_none());
}

/// Control on the same seam: well-formed registrations return `ok` and land.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn well_formed_guest_registrations_return_ok_and_land() {
    use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;

    let (registry, mut state) = guest_host("ext082-guest-ok");
    state
        .register_tool(wit_tool(&OBJECT_SCHEMA().to_string()))
        .await
        .expect("an object schema is accepted");
    state
        .register_flag(
            "ext082_flag".into(),
            json!({"type": "boolean", "default": true}).to_string(),
        )
        .await
        .expect("a matching default is accepted");
    assert!(registry.has_guest_tool("ext082_guest_tool").unwrap());
    assert_eq!(
        state.get_flag("ext082_flag".into()).await.as_deref(),
        Some("true")
    );
}

/// A `register-flag` lowered with its return pointer: `(name, spec, retptr)`. The `result<_, string>`
/// lands at `retptr` as `{u8 discriminant, ptr, len}`, so `i32.load8_u` of it is `1` on `err`.
#[cfg(feature = "wasm-host")]
fn register_flag_lowered() -> crate::tests::wat_guest::Lowered {
    crate::tests::wat_guest::Lowered {
        core_name: "register_flag",
        component_func: "$register-flag",
        core_sig: "(param i32 i32 i32 i32 i32)",
        needs_realloc: true,
    }
}

/// The `result<_, string>` return area every fixture below lowers its registrations into. It has
/// the same layout as `execute-shortcut`'s and `init`'s own `result<_, string>`, so returning this
/// pointer from one of those exports IS propagating the refusal (`?`).
#[cfg(feature = "wasm-host")]
const RET: u32 = 16512;

/// EXT-082 (world 0.13) — pi's `registerFlag` THROWS, which ends the rest of the handler and, left
/// uncaught, fails it. A real component whose `execute-shortcut` registers `ext082_flag` and, on
/// `err`, RETURNS that `err` (the WAT spelling of `?`) before it can register `ext082_after`: the
/// call fails with pi's message and the second registration never happens. With a matching default
/// both land and the call succeeds. Red before the re-signing: the import returned `()`, so the
/// guest could neither stop nor propagate.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_refused_register_flag_propagated_with_question_mark_stops_and_fails_the_handler() {
    use crate::manifest::Capabilities;
    use crate::tests::wat_guest::{REGISTRATION_FLAG_AND_UNSUBSCRIBE, WatGuest, wat_str};

    const AFTER_SPEC: &str = r#"{"type":"boolean"}"#;
    fn component(spec: &str) -> Vec<u8> {
        WatGuest {
            component: REGISTRATION_FLAG_AND_UNSUBSCRIBE.to_string(),
            lowered: vec![register_flag_lowered()],
            overrides: vec![(
                "execute-shortcut",
                format!(
                    "    (func (export \"execute-shortcut\") (param i32 i32) (result i32) \
                     (call $register_flag (i32.const 16384) (i32.const 11) \
                     (i32.const 16400) (i32.const {}) (i32.const {RET})) \
                     (if (i32.load8_u (i32.const {RET})) (then (return (i32.const {RET})))) \
                     (call $register_flag (i32.const 16448) (i32.const 12) \
                     (i32.const 16464) (i32.const {}) (i32.const {RET})) \
                     i32.const {RET})",
                    spec.len(),
                    AFTER_SPEC.len()
                ),
            )],
            data: vec![
                (16384, "ext082_flag".into()),
                (16400, wat_str(spec)),
                (16448, "ext082_after".into()),
                (16464, wat_str(AFTER_SPEC)),
            ],
        }
        .build()
    }
    let cancel = cyrup_core::CancelToken::new();

    let host = ExtensionHost::with_wasm(cfg()).unwrap();
    let live = host
        .load_wasm_with_caps(
            "ext082-stop".into(),
            &component(r#"{"type":"boolean","default":"yes"}"#),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .expect("init registers nothing, so the load succeeds");
    let Err(err) = live.execute_shortcut("ctrl+x", &cancel).await else {
        panic!("a propagated refusal fails the handler call")
    };
    assert_eq!(
        err.to_string(),
        "component load failed: Invalid default for flag \"ext082_flag\": expected boolean, got \
         string",
        "the handler's error is pi's message"
    );
    assert!(
        host.registry().get_flag("ext082_after").unwrap().is_none(),
        "the guest stopped at `err`: the rest of its handler did not run"
    );

    let host = ExtensionHost::with_wasm(cfg()).unwrap();
    let live = host
        .load_wasm_with_caps(
            "ext082-stop".into(),
            &component(r#"{"type":"boolean","default":true}"#),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .unwrap();
    live.execute_shortcut("ctrl+x", &cancel)
        .await
        .expect("both registrations are accepted");
    assert!(host.registry().get_flag("ext082_flag").unwrap().is_some());
    assert!(
        host.registry().get_flag("ext082_after").unwrap().is_some(),
        "the control: an `ok` import lets the handler run on"
    );
}

/// `registration` import declaring `register-tool` (its `tool-descriptor` aliased from the `types`
/// import, as `use types.{tool-descriptor}` encodes it) and `register-flag`.
#[cfg(feature = "wasm-host")]
const REGISTRATION_TOOL_AND_FLAG: &str = r#"  (import "cyrup:ext/types@0.18.0" (instance $types
    (type $em (enum "parallel" "sequential"))
    (export "exec-mode" (type $em-x (eq $em)))
    (type $tn (record
      (field "name" string) (field "description" (option string))
      (field "instructions" (option string))))
    (export "tool-namespace" (type $tn-x (eq $tn)))
    (type $td (record
      (field "name" string) (field "label" string) (field "description" string)
      (field "parameters-json" string) (field "exec-mode" (option $em-x))
      (field "prompt-snippet" (option string)) (field "prompt-guidelines" (list string))
      (field "has-renderer" bool) (field "prepare-arguments" bool)
      (field "render-shell" (option string)) (field "constrained-sampling" (option string))
      (field "exposure" (option string)) (field "namespace" (option $tn-x))
      (field "default-active" (option bool))))
    (export "tool-descriptor" (type $td-x (eq $td)))))
  (alias export $types "tool-descriptor" (type $tool-descriptor))
  (import "cyrup:ext/registration@0.18.0" (instance $reg
    (alias outer 1 $tool-descriptor (type $td))
    (export "tool-descriptor" (type $td-x (eq $td)))
    (export "register-tool" (func (param "t" $td-x) (result (result (error string)))))
    (export "register-flag" (func (param "name" string) (param "spec-json" string) (result (result (error string)))))))
  (alias export $reg "register-tool" (func $register-tool))
  (alias export $reg "register-flag" (func $register-flag))
"#;

/// A little-endian `u32` as WAT data-string escapes.
#[cfg(feature = "wasm-host")]
fn le(v: u32) -> String {
    v.to_le_bytes()
        .iter()
        .map(|b| format!("\\{b:02x}"))
        .collect()
}

/// EXT-082 — handling the `err` is pi's `try`/`catch` around the throw: the handler carries on and
/// its call SUCCEEDS, with the refused tool not registered. A real component whose `execute-shortcut`
/// registers a tool with `parameters: []`, ignores the `err`, registers `ext082_after`, and returns
/// `ok`. Red before this pass: the host also recorded every refusal on a per-call registration
/// window and failed the call with it, whatever the guest did with the `err`.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_guest_that_handles_a_refused_register_tool_carries_on_and_its_call_succeeds() {
    use crate::manifest::Capabilities;
    use crate::tests::wat_guest::{Lowered, WatGuest, wat_str};

    const AFTER_SPEC: &str = r#"{"type":"boolean"}"#;
    // The flattened `tool-descriptor` exceeds 16 core params, so it is passed by pointer: the
    // canonical-ABI record (136 bytes) at 17000 — `name` at 0, `label` 8, `description` 16,
    // `parameters-json` 24, then zeroed options/list/bools.
    let mut record = String::new();
    record.push_str(&le(17200));
    record.push_str(&le(13)); // name → "ext082_caught"
    record.push_str(&le(0).repeat(4)); // label, description: ""
    record.push_str(&le(17220));
    record.push_str(&le(2)); // parameters-json → "[]"
    record.push_str(&"\\00".repeat(136 - 32));

    let component = |parameters: &str| {
        WatGuest {
            component: REGISTRATION_TOOL_AND_FLAG.to_string(),
            lowered: vec![
                Lowered {
                    core_name: "register_tool",
                    component_func: "$register-tool",
                    core_sig: "(param i32 i32)",
                    needs_realloc: true,
                },
                register_flag_lowered(),
            ],
            overrides: vec![(
                "execute-shortcut",
                format!(
                    "    (func (export \"execute-shortcut\") (param i32 i32) (result i32) \
                 (call $register_tool (i32.const 17000) (i32.const {RET})) \
                 (call $register_flag (i32.const 16448) (i32.const 12) \
                 (i32.const 16464) (i32.const {}) (i32.const {RET})) \
                 i32.const 16)",
                    AFTER_SPEC.len()
                ),
            )],
            data: vec![
                (16448, "ext082_after".into()),
                (16464, wat_str(AFTER_SPEC)),
                (17000, record.clone()),
                (17200, "ext082_caught".into()),
                (17220, parameters.into()),
            ],
        }
        .build()
    };

    let host = ExtensionHost::with_wasm(cfg()).unwrap();
    let live = host
        .load_wasm_with_caps(
            "ext082-caught".into(),
            &component("[]"),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .expect("init registers nothing, so the load succeeds");
    live.execute_shortcut("ctrl+x", &cyrup_core::CancelToken::new())
        .await
        .expect("a refusal the guest handled does not fail its call");
    assert!(
        !host.registry().has_guest_tool("ext082_caught").unwrap(),
        "the refused tool is not registered"
    );
    assert!(
        host.registry().get_flag("ext082_after").unwrap().is_some(),
        "the handler carried on past the handled refusal"
    );

    // The control: the same record with `parameters: {}` registers, so the refusal above is the
    // schema guard, not a record the host failed to read.
    let host = ExtensionHost::with_wasm(cfg()).unwrap();
    let live = host
        .load_wasm_with_caps(
            "ext082-caught".into(),
            &component("{}"),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .unwrap();
    live.execute_shortcut("ctrl+x", &cyrup_core::CancelToken::new())
        .await
        .expect("an object schema is accepted");
    assert!(host.registry().has_guest_tool("ext082_caught").unwrap());
}

/// End to end through `ExtensionHost::load_wasm_with_caps`: a real component whose `init` calls
/// `registration.register-flag` with a mistyped default and RETURNS the `err` (as the SDK's
/// `push_registrations` does with `?`) fails to load with pi's message — pi's throwing factory,
/// reported as `Failed to load extension: <message>` (`core/extensions/loader.ts:576-578`
/// @v0.87.1) — and the load's failure tail (EXT-081) leaves the flag unregistered. An `init` that
/// handles the `err` loads without the flag (pi's `try`/`catch` in the factory), and a matching
/// default loads with it.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_component_whose_init_propagates_a_refused_flag_fails_to_load() {
    use crate::manifest::Capabilities;
    use crate::tests::wat_guest::{REGISTRATION_FLAG_AND_UNSUBSCRIBE, WatGuest, wat_str};

    fn component(spec: &str, propagate: bool) -> Vec<u8> {
        let ret = if propagate { RET } else { 16 };
        WatGuest {
            component: REGISTRATION_FLAG_AND_UNSUBSCRIBE.to_string(),
            lowered: vec![register_flag_lowered()],
            overrides: vec![(
                "init",
                format!(
                    "    (func (export \"init\") (result i32) (call $register_flag (i32.const 16384) \
                     (i32.const 11) (i32.const 16400) (i32.const {}) (i32.const {RET})) \
                     i32.const {ret})",
                    spec.len()
                ),
            )],
            data: vec![(16384, "ext082_flag".into()), (16400, wat_str(spec))],
        }
        .build()
    }
    let load = |bytes: Vec<u8>| async move {
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        let loaded = host
            .load_wasm_with_caps(
                "ext082-component".into(),
                &bytes,
                Arc::new(crate::DenyServices),
                &Capabilities::host_granted(),
            )
            .await
            .map(|_| ());
        let flag = host.registry().get_flag("ext082_flag").unwrap();
        (loaded, flag)
    };

    let (loaded, flag) = load(component(r#"{"type":"boolean","default":"yes"}"#, true)).await;
    let Err(err) = loaded else {
        panic!("an `init` that propagates the refusal must fail the load")
    };
    assert_eq!(
        err.to_string(),
        "component load failed: init failed: Invalid default for flag \"ext082_flag\": expected \
         boolean, got string"
    );
    assert!(flag.is_none());

    let (loaded, flag) = load(component(r#"{"type":"boolean","default":"yes"}"#, false)).await;
    loaded.expect("an `init` that handles the refusal loads");
    assert!(flag.is_none(), "the refused flag is still not registered");

    let (loaded, flag) = load(component(r#"{"type":"boolean","default":true}"#, true)).await;
    loaded.expect("a matching default loads");
    assert!(flag.is_some());
}
