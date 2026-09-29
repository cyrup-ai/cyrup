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

/// Control: a well-formed tool and a flag with a matching (or absent, or `null`) default load.
#[tokio::test]
async fn well_formed_registrations_load() {
    for flag_spec in [
        json!({"type": "boolean", "default": true}),
        json!({"type": "string", "default": "x"}),
        json!({"type": "string"}),
        json!({"type": "string", "default": null}),
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

// ---------------------------------------------------------------------------
// The WASM path: the `registration.*` imports have no error channel, so a refusal is collected on
// `GuestState` and `LiveExtension::load` fails with it once `init` returns
// (`GuestState::finish_init_registrations`, the step the load runs right after `call_init`).
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm-host")]
fn guest_host(
    id: &str,
) -> (
    Arc<ExtensionRegistry>,
    Arc<crate::host::GuestState>,
    crate::host::HostState,
) {
    use crate::host::{GuestState, HostState, StoreLimits};
    let registry = Arc::new(ExtensionRegistry::new());
    let guest = Arc::new(GuestState::new(ExtensionId::from(id), registry.clone()));
    let state = HostState::with_guest(StoreLimits::default(), guest.clone());
    (registry, guest, state)
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
    }
}

#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_guest_tool_without_an_object_schema_fails_init_instead_of_vanishing() {
    use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;

    for parameters_json in ["[]", "not json"] {
        let (registry, guest, mut state) = guest_host("ext082-guest");
        state.register_tool(wit_tool(parameters_json)).await;
        let Err(err) = guest.finish_init_registrations() else {
            panic!("{parameters_json:?}: the refusal must fail the load")
        };
        assert_eq!(
            err.to_string(),
            "Tool \"ext082_guest_tool\" registered by extension \"ext082-guest\" must define an \
             object parameter schema."
        );
        assert!(!registry.has_guest_tool("ext082_guest_tool").unwrap());
    }
}

#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_guest_flag_whose_default_mismatches_its_type_fails_init_and_is_not_served() {
    use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;

    let (registry, guest, mut state) = guest_host("ext082-guest-flag");
    state
        .register_flag(
            "ext082_flag".into(),
            json!({"type": "boolean", "default": "yes"}).to_string(),
        )
        .await;
    assert_eq!(
        state.get_flag("ext082_flag".into()).await,
        None,
        "a refused flag is not stored, so `getFlag` cannot serve its string default"
    );
    assert!(registry.get_flag("ext082_flag").unwrap().is_none());
    let Err(err) = guest.finish_init_registrations() else {
        panic!("the refusal must fail the load")
    };
    assert_eq!(
        err.to_string(),
        "Invalid default for flag \"ext082_flag\": expected boolean, got string"
    );
}

/// Control on the same seam: well-formed registrations close the `init` window cleanly, and a
/// refusal from a live handler AFTER it closed has no load to fail — the next close stays `Ok`.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn well_formed_guest_registrations_pass_and_a_late_refusal_does_not_fail_a_finished_load() {
    use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;

    let (registry, guest, mut state) = guest_host("ext082-guest-ok");
    state
        .register_tool(wit_tool(&OBJECT_SCHEMA().to_string()))
        .await;
    state
        .register_flag(
            "ext082_flag".into(),
            json!({"type": "boolean", "default": true}).to_string(),
        )
        .await;
    guest.finish_init_registrations().expect("nothing refused");
    assert!(registry.has_guest_tool("ext082_guest_tool").unwrap());
    assert_eq!(
        state.get_flag("ext082_flag".into()).await.as_deref(),
        Some("true")
    );

    state.register_tool(wit_tool("[]")).await;
    guest
        .finish_init_registrations()
        .expect("the init window is closed; a late refusal is logged, not queued");
}

/// End to end through `ExtensionHost::load_wasm_with_caps`: a real component whose `init` calls
/// `registration.register-flag` with a mistyped default fails to load with pi's message, and the
/// load's failure tail (EXT-081) leaves the flag unregistered. The same component with a matching
/// default loads — the control that the failure is the default, not the component.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_component_whose_init_registers_a_mistyped_flag_fails_to_load() {
    use crate::manifest::Capabilities;
    use crate::tests::wat_guest::{Lowered, REGISTRATION_FLAG_AND_UNSUBSCRIBE, WatGuest, wat_str};

    fn component(spec: &str) -> Vec<u8> {
        WatGuest {
            component: REGISTRATION_FLAG_AND_UNSUBSCRIBE.to_string(),
            lowered: vec![Lowered {
                core_name: "register_flag",
                component_func: "$register-flag",
                core_sig: "(param i32 i32 i32 i32)",
                needs_realloc: false,
            }],
            overrides: vec![(
                "init",
                format!(
                    "    (func (export \"init\") (result i32) (call $register_flag (i32.const 16384) \
                     (i32.const 11) (i32.const 16400) (i32.const {})) i32.const 16)",
                    spec.len()
                ),
            )],
            data: vec![(16384, "ext082_flag".into()), (16400, wat_str(spec))],
        }
        .build()
    }

    let host = ExtensionHost::with_wasm(cfg()).unwrap();
    let Err(err) = host
        .load_wasm_with_caps(
            "ext082-component".into(),
            &component(r#"{"type":"boolean","default":"yes"}"#),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
    else {
        panic!("a mistyped flag default must fail the load")
    };
    assert_eq!(
        err.to_string(),
        "Invalid default for flag \"ext082_flag\": expected boolean, got string"
    );
    assert!(host.registry().get_flag("ext082_flag").unwrap().is_none());

    host.load_wasm_with_caps(
        "ext082-component".into(),
        &component(r#"{"type":"boolean","default":true}"#),
        Arc::new(crate::DenyServices),
        &Capabilities::host_granted(),
    )
    .await
    .expect("a matching default loads");
    assert!(host.registry().get_flag("ext082_flag").unwrap().is_some());
}
