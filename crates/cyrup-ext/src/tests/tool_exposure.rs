//! Tool exposure on the extension host: which extension tools registration activates, which stay
//! registered only, how a guest declares an exposure, and how a failed `prepare_loadout` hook is
//! reported.
//!
//! pi `ToolExposure` (`core/extensions/types.ts:509` @v1.0.1). The session's active set is the
//! registry minus the extension tools that registration does not activate
//! (`_refreshToolRegistry`, `core/agent-session.ts:3445-3545`; `_isDeclarable` /
//! `_isActivatedOnRegistration`, `:3548-3555`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::wrapper::ActiveToolNames;
use crate::{
    ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_core::{
    CancelToken, ExtensionId, Tool, ToolCallId, ToolError, ToolExposure, ToolNamespace, ToolResult,
    ToolUpdateSink,
};
use serde_json::{Value, json};

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

/// An extension-contributed tool with a chosen exposure, namespace and `defaultActive`.
struct ExposedTool {
    name: &'static str,
    exposure: ToolExposure,
    default_active: bool,
    namespace: Option<ToolNamespace>,
    schema: Value,
}

impl ExposedTool {
    fn boxed(name: &'static str, exposure: ToolExposure) -> Arc<dyn Tool> {
        Arc::new(Self {
            name,
            exposure,
            default_active: true,
            namespace: None,
            schema: json!({"type": "object"}),
        })
    }
}

#[async_trait::async_trait]
impl Tool for ExposedTool {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    fn exposure(&self) -> ToolExposure {
        self.exposure
    }
    fn namespace(&self) -> Option<&ToolNamespace> {
        self.namespace.as_ref()
    }
    fn default_active(&self) -> bool {
        self.default_active
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

/// One extension that registers one tool of each exposure, plus a `direct` tool registered with
/// `defaultActive: false`.
struct ExposureExt;

#[async_trait::async_trait]
impl NativeExtension for ExposureExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("exposure-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.register_tool(ExposedTool::boxed("direct_tool", ToolExposure::Direct));
        api.register_tool(ExposedTool::boxed(
            "model_only_tool",
            ToolExposure::ModelOnly,
        ));
        api.register_tool(Arc::new(ExposedTool {
            name: "codemode_tool",
            exposure: ToolExposure::Codemode,
            default_active: true,
            namespace: Some(ToolNamespace {
                name: "mcp__docs".to_string(),
                description: Some("docs".to_string()),
                instructions: None,
            }),
            schema: json!({"type": "object"}),
        }));
        api.register_tool(ExposedTool::boxed("deferred_tool", ToolExposure::Deferred));
        api.register_tool(ExposedTool::boxed("hidden_tool", ToolExposure::Hidden));
        api.register_tool(Arc::new(ExposedTool {
            name: "inactive_tool",
            exposure: ToolExposure::Direct,
            default_active: false,
            namespace: None,
            schema: json!({"type": "object"}),
        }));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

async fn exposure_host() -> ExtensionHost {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(ExposureExt)).await.unwrap();
    host
}

/// `(extension, event, error)` as an error listener receives them.
type SeenError = (String, &'static str, String);

fn names(tools: &[Arc<dyn Tool>]) -> Vec<String> {
    tools.iter().map(|t| t.name().to_string()).collect()
}

fn set(names: &[&str]) -> HashSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

const ALL_SIX: [&str; 6] = [
    "direct_tool",
    "model_only_tool",
    "codemode_tool",
    "deferred_tool",
    "hidden_tool",
    "inactive_tool",
];

/// With no allowlist, registration activates exactly the declarable tools that did not register
/// `defaultActive: false` (pi `_isActivatedOnRegistration`, `agent-session.ts:3554`).
#[tokio::test]
async fn registration_activates_only_declarable_default_active_extension_tools() {
    let host = exposure_host().await;
    let none = HashSet::new();
    assert_eq!(
        names(&host.active_tools_filtered(&[], None, &none).unwrap()),
        ["direct_tool", "model_only_tool"],
        "codemode, deferred and hidden tools, and a `defaultActive: false` direct tool, are \
         registered but not active"
    );
    assert_eq!(
        names(&host.active_tools(&[]).unwrap()),
        ["direct_tool", "model_only_tool"],
        "the unrestricted form is the filtered form with no allowlist and no denylist"
    );
}

/// With an allowlist, naming a tool activates it iff its exposure is declarable, even when it
/// registered `defaultActive: false` (pi `agent-session.ts:3510-3516`).
#[tokio::test]
async fn an_allowlist_activates_a_named_tool_iff_its_exposure_is_declarable() {
    let host = exposure_host().await;
    let none = HashSet::new();

    let active = host
        .active_tools_filtered(
            &[],
            Some(&set(&["codemode_tool", "model_only_tool"])),
            &none,
        )
        .unwrap();
    assert_eq!(
        names(&active),
        ["model_only_tool"],
        "naming a codemode tool does not activate it; naming a model-only one does"
    );

    let active = host
        .active_tools_filtered(
            &[],
            Some(&set(&[
                "inactive_tool",
                "deferred_tool",
                "hidden_tool",
                "direct_tool",
            ])),
            &none,
        )
        .unwrap();
    assert_eq!(
        names(&active),
        ["direct_tool", "inactive_tool"],
        "naming a declarable tool activates it even with `defaultActive: false`; a deferred or \
         hidden one stays inactive"
    );

    assert!(
        host.active_tools_filtered(&[], Some(&HashSet::new()), &none)
            .unwrap()
            .is_empty(),
        "`Some(∅)` still denies everything"
    );
}

/// The registry holds every allowed extension tool whatever its exposure.
#[tokio::test]
async fn the_registry_holds_every_tool_whatever_its_exposure() {
    let host = exposure_host().await;
    let none = HashSet::new();
    assert_eq!(
        names(&host.registered_tools_filtered(&[], None, &none).unwrap()),
        ALL_SIX
    );
    assert_eq!(
        names(
            &host
                .registered_tools_filtered(
                    &[],
                    Some(&set(&["codemode_tool", "model_only_tool"])),
                    &none
                )
                .unwrap()
        ),
        ["model_only_tool", "codemode_tool"],
        "an allowlist narrows the registry by name, not by exposure"
    );
    assert_eq!(
        names(
            &host
                .registered_tools_filtered(&[], None, &set(&["hidden_tool"]))
                .unwrap()
        ),
        [
            "direct_tool",
            "model_only_tool",
            "codemode_tool",
            "deferred_tool",
            "inactive_tool"
        ],
        "the denylist applies to the registry too"
    );
}

/// Base tools are the caller's build-time selection: an extension override of a base tool replaces
/// it by name and stays in the active list whatever the override's exposure.
#[tokio::test]
async fn an_extension_override_of_a_base_tool_stays_in_the_active_list() {
    struct OverrideExt;
    #[async_trait::async_trait]
    impl NativeExtension for OverrideExt {
        fn id(&self) -> ExtensionId {
            ExtensionId::from("override-ext")
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
            api.register_tool(ExposedTool::boxed("read", ToolExposure::Codemode));
            Ok(())
        }
        async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            HookOutcome::Noop
        }
    }
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(OverrideExt)).await.unwrap();
    let base = vec![ExposedTool::boxed("read", ToolExposure::Direct)];
    let none = HashSet::new();

    let active = host.active_tools_filtered(&base, None, &none).unwrap();
    assert_eq!(names(&active), ["read"]);
    assert_eq!(
        active[0].exposure(),
        ToolExposure::Codemode,
        "the override replaced the base tool by name"
    );
    let active = host
        .active_tools_filtered(&base, Some(&set(&["read"])), &none)
        .unwrap();
    assert_eq!(names(&active), ["read"]);
}

/// Everything the host hands out is wrapped for `addedToolNames` derivation once a live source is
/// attached; the wrapper must still report each tool's real exposure.
#[tokio::test]
async fn host_wrapped_tools_keep_their_exposure() {
    struct NoAgent;
    impl ActiveToolNames for NoAgent {
        fn active_tool_names(&self) -> Option<Vec<String>> {
            None
        }
    }
    let host = exposure_host().await;
    host.set_active_tool_source(Arc::new(NoAgent));
    let none = HashSet::new();
    let registered = host.registered_tools_filtered(&[], None, &none).unwrap();
    let exposures: Vec<(String, ToolExposure, bool)> = registered
        .iter()
        .map(|t| (t.name().to_string(), t.exposure(), t.default_active()))
        .collect();
    assert_eq!(
        exposures,
        [
            ("direct_tool".to_string(), ToolExposure::Direct, true),
            ("model_only_tool".to_string(), ToolExposure::ModelOnly, true),
            ("codemode_tool".to_string(), ToolExposure::Codemode, true),
            ("deferred_tool".to_string(), ToolExposure::Deferred, true),
            ("hidden_tool".to_string(), ToolExposure::Hidden, true),
            ("inactive_tool".to_string(), ToolExposure::Direct, false),
        ]
    );
    let codemode = registered
        .iter()
        .find(|t| t.name() == "codemode_tool")
        .unwrap();
    assert_eq!(
        codemode.namespace().map(|n| n.name.as_str()),
        Some("mcp__docs")
    );
    assert_eq!(
        names(&host.active_tools_filtered(&[], None, &none).unwrap()),
        ["direct_tool", "model_only_tool"],
        "the wrapped active set is filtered by the same rule"
    );
}

/// The registry-only fallbacks a guest sees when no live session backend is attached carry pi's
/// `ToolInfo.exposure` / `ToolInfo.namespace` keys, and `getActiveTools()` answers the activated
/// names, not the whole registry.
#[tokio::test]
async fn the_registry_fallback_rows_carry_exposure_and_namespace() {
    let host = exposure_host().await;
    let rows = host.registry().tool_info().unwrap();
    let row = |name: &str| {
        rows.iter()
            .find(|r| r.get("name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("no row for {name}"))
            .clone()
    };
    for (name, exposure) in [
        ("direct_tool", "direct"),
        ("model_only_tool", "model-only"),
        ("codemode_tool", "codemode"),
        ("deferred_tool", "deferred"),
        ("hidden_tool", "hidden"),
        ("inactive_tool", "direct"),
    ] {
        assert_eq!(row(name).get("exposure"), Some(&json!(exposure)), "{name}");
    }
    assert_eq!(
        row("codemode_tool").get("namespace"),
        Some(&json!({"name": "mcp__docs", "description": "docs"})),
        "pi's camelCase ToolNamespace; absent members omitted"
    );
    assert!(
        row("direct_tool").get("namespace").is_none(),
        "`namespace` is omitted when the tool has none"
    );
    assert_eq!(
        host.registry().activated_tool_names().unwrap(),
        ["direct_tool", "model_only_tool"]
    );
}

/// A failed `prepare_loadout` hook is reported on the extension error channel with the event name
/// pi uses (`emitError({ event: "prepare_loadout", … })`, `agent-session.ts:1556-1561` @v1.0.1).
#[tokio::test]
async fn a_loadout_hook_failure_reaches_the_error_listener() {
    let host = exposure_host().await;
    let seen: Arc<Mutex<Vec<SeenError>>> = Arc::default();
    let sink = seen.clone();
    host.add_error_listener(Arc::new(move |e| {
        sink.lock()
            .unwrap()
            .push((e.extension.to_string(), e.event, e.error.clone()));
    }));

    host.report_loadout_failure("codemode_tool", "hook exploded".to_string());
    host.report_loadout_failure("read", "builtin hook exploded".to_string());

    let seen = seen.lock().unwrap();
    assert_eq!(
        *seen,
        [
            (
                "exposure-ext".to_string(),
                "prepare_loadout",
                "hook exploded".to_string()
            ),
            (
                "<builtin:read>".to_string(),
                "prepare_loadout",
                "builtin hook exploded".to_string()
            ),
        ],
        "an extension tool is attributed to its owner; a tool the registry does not know gets \
         pi's synthetic `<builtin:NAME>` path"
    );
}

/// The unknown-exposure refusal's wording, with the owner and tool named.
#[test]
fn an_unknown_exposure_spelling_is_refused_by_name() {
    let owner = ExtensionId::from("ext-x");
    assert_eq!(
        crate::registry::parse_exposure(&owner, "t", None).unwrap(),
        ToolExposure::Direct
    );
    for (raw, expected) in [
        ("direct", ToolExposure::Direct),
        ("model-only", ToolExposure::ModelOnly),
        ("codemode", ToolExposure::Codemode),
        ("deferred", ToolExposure::Deferred),
        ("hidden", ToolExposure::Hidden),
    ] {
        assert_eq!(
            crate::registry::parse_exposure(&owner, "t", Some(raw)).unwrap(),
            expected
        );
    }
    let err = crate::registry::parse_exposure(&owner, "t", Some("Codemode")).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Tool \"t\" registered by extension \"ext-x\" declares an invalid exposure: unknown tool \
         exposure `Codemode` (expected direct, model-only, codemode, deferred or hidden)."
    );
}

/// Serde: a descriptor serialized before the exposure fields existed still deserializes, to
/// pi's defaults.
#[test]
fn a_descriptor_without_exposure_fields_deserializes_to_pis_defaults() {
    let d: crate::registry::ToolDescriptor = serde_json::from_value(json!({
        "name": "old",
        "label": "Old",
        "description": "",
        "parameters": {"type": "object"},
    }))
    .unwrap();
    assert_eq!(d.exposure, ToolExposure::Direct);
    assert!(d.namespace.is_none());
    assert!(d.default_active);
    let wire = serde_json::to_value(&d).unwrap();
    assert!(
        wire.get("exposure").is_none(),
        "the default is omitted on the wire"
    );
    assert!(wire.get("defaultActive").is_none());

    let declared: crate::registry::ToolDescriptor = serde_json::from_value(json!({
        "name": "new",
        "label": "New",
        "description": "",
        "parameters": {"type": "object"},
        "exposure": "deferred",
        "namespace": {"name": "mcp__docs", "instructions": "read me"},
        "defaultActive": false,
    }))
    .unwrap();
    assert_eq!(declared.exposure, ToolExposure::Deferred);
    assert_eq!(
        declared.namespace.as_ref().map(|n| n.name.as_str()),
        Some("mcp__docs")
    );
    assert!(!declared.default_active);
}

// ---------------------------------------------------------------------------
// The WASM path: a guest declares exposure / namespace / default-active on `tool-descriptor`.
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm-host")]
mod guest {
    use super::*;
    use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;
    use crate::host::live::bindings::cyrup::ext::types as wit;
    use crate::registry::ExtensionRegistry;

    fn wit_tool(
        exposure: Option<&str>,
        namespace: Option<wit::ToolNamespace>,
        default_active: Option<bool>,
    ) -> wit::ToolDescriptor {
        wit::ToolDescriptor {
            name: "exposed_guest".into(),
            label: "Exposed".into(),
            description: String::new(),
            parameters_json: json!({"type": "object"}).to_string(),
            exec_mode: None,
            prompt_snippet: None,
            prompt_guidelines: Vec::new(),
            has_renderer: false,
            prepare_arguments: false,
            render_shell: None,
            constrained_sampling: None,
            exposure: exposure.map(str::to_string),
            namespace,
            default_active,
        }
    }

    fn guest_host(id: &str) -> (Arc<ExtensionRegistry>, crate::host::HostState) {
        use crate::host::{GuestState, HostState, StoreLimits};
        let registry = Arc::new(ExtensionRegistry::new());
        let guest = Arc::new(GuestState::new(ExtensionId::from(id), registry.clone()));
        (
            registry,
            HostState::with_guest(StoreLimits::default(), guest),
        )
    }

    /// The registration seam: the lowered WIT record lands in the registry's descriptor with the
    /// exposure parsed, the namespace converted and `default-active` carried.
    #[tokio::test]
    async fn the_register_tool_import_stores_exposure_namespace_and_default_active() {
        let (registry, mut state) = guest_host("guest-ext");
        state
            .register_tool(wit_tool(
                Some("codemode"),
                Some(wit::ToolNamespace {
                    name: "mcp__docs".into(),
                    description: Some("docs server".into()),
                    instructions: Some("use the docs".into()),
                }),
                Some(false),
            ))
            .await
            .unwrap();
        let descs = registry.guest_tool_descriptors().unwrap();
        let d = &descs[0];
        assert_eq!(d.exposure, ToolExposure::Codemode);
        assert_eq!(
            d.namespace,
            Some(ToolNamespace {
                name: "mcp__docs".into(),
                description: Some("docs server".into()),
                instructions: Some("use the docs".into()),
            })
        );
        assert!(!d.default_active);
        let rows = registry.tool_info().unwrap();
        assert_eq!(rows[0].get("exposure"), Some(&json!("codemode")));
        assert_eq!(
            rows[0].pointer("/namespace/instructions"),
            Some(&json!("use the docs"))
        );
        assert!(
            registry.activated_tool_names().unwrap().is_empty(),
            "a codemode guest tool is registered but not active"
        );

        // The absent fields are pi's defaults.
        let (registry, mut state) = guest_host("guest-ext");
        state
            .register_tool(wit_tool(None, None, None))
            .await
            .unwrap();
        let d = &registry.guest_tool_descriptors().unwrap()[0];
        assert_eq!(d.exposure, ToolExposure::Direct);
        assert!(d.namespace.is_none());
        assert!(d.default_active);
        assert_eq!(registry.activated_tool_names().unwrap(), ["exposed_guest"]);
    }

    /// An unknown spelling is refused at registration with an error naming the tool, the
    /// extension and the spelling; nothing is registered.
    #[tokio::test]
    async fn an_unknown_exposure_is_refused_at_registration_not_coerced() {
        let (registry, mut state) = guest_host("guest-ext");
        let returned = state
            .register_tool(wit_tool(Some("sometimes"), None, None))
            .await;
        assert_eq!(
            returned.as_ref().map_err(String::as_str),
            Err(
                "Tool \"exposed_guest\" registered by extension \"guest-ext\" declares an \
                 invalid exposure: unknown tool exposure `sometimes` (expected direct, \
                 model-only, codemode, deferred or hidden)."
            )
        );
        assert!(!registry.has_guest_tool("exposed_guest").unwrap());
    }

    fn le(v: u32) -> String {
        v.to_le_bytes()
            .iter()
            .map(|b| format!("\\{b:02x}"))
            .collect()
    }

    /// The canonical-ABI `tool-descriptor` record (136 bytes) with the exposure members set.
    /// Offsets: `name` 0, `label` 8, `description` 16, `parameters-json` 24, `exec-mode` 32,
    /// `prompt-snippet` 36, `prompt-guidelines` 48, the two bools 56-57, `render-shell` 60,
    /// `constrained-sampling` 72, `exposure` 84, `namespace` 96 (its `name` 100, `description`
    /// 108, `instructions` 120), `default-active` 132.
    fn record(exposure_ptr: u32, exposure_len: u32) -> String {
        let mut bytes = [0u32; 34];
        let mut put = |offset: usize, v: u32| bytes[offset / 4] = v;
        put(0, 17200);
        put(4, 13); // name -> "exposed_guest"
        put(24, 17220);
        put(28, 2); // parameters-json -> "{}"
        put(84, 1); // exposure: some
        put(88, exposure_ptr);
        put(92, exposure_len);
        put(96, 1); // namespace: some
        put(100, 17240);
        put(104, 9); // name -> "mcp__docs"
        put(108, 1); // description: some
        put(112, 17260);
        put(116, 11); // -> "docs server"
        put(120, 1); // instructions: some
        put(124, 17280);
        put(128, 12); // -> "use the docs"
        // `default-active` is an `option<bool>` at 132: tag byte 1, then the value byte 0
        // (`false`), which one little-endian word writes.
        put(132, 1);
        bytes.iter().map(|w| le(*w)).collect()
    }

    /// A real component whose `init` calls `registration.register-tool` with an exposure string:
    /// the production import lifts the record, the host materializes a `WasmTool`, and that
    /// tool answers `exposure()`, `namespace()` and `default_active()` from the descriptor.
    #[tokio::test]
    async fn a_guest_components_declared_exposure_reaches_the_wasm_tool() {
        use crate::manifest::Capabilities;
        use crate::tests::wat_guest::{Lowered, WatGuest, wat_str};

        const RET: u32 = 16512;
        let component = |exposure: &str, propagate: bool| {
            let ret = if propagate { RET } else { 16 };
            WatGuest {
                component: super::guest_registration_tool_import(),
                lowered: vec![Lowered {
                    core_name: "register_tool",
                    component_func: "$register-tool",
                    core_sig: "(param i32 i32)",
                    needs_realloc: true,
                }],
                overrides: vec![(
                    "init",
                    format!(
                        "    (func (export \"init\") (result i32) \
                         (call $register_tool (i32.const 17000) (i32.const {RET})) \
                         i32.const {ret})"
                    ),
                )],
                data: vec![
                    (17000, record(17230, exposure.len() as u32)),
                    (17200, "exposed_guest".into()),
                    (17220, "{}".into()),
                    (17230, wat_str(exposure)),
                    (17240, "mcp__docs".into()),
                    (17260, "docs server".into()),
                    (17280, "use the docs".into()),
                ],
            }
            .build()
        };

        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        host.load_wasm_with_caps(
            "exposed-guest".into(),
            &component("codemode", true),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .expect("a valid exposure registers");
        let none = HashSet::new();
        let registered = host.registered_tools_filtered(&[], None, &none).unwrap();
        let tool = registered
            .iter()
            .find(|t| t.name() == "exposed_guest")
            .expect("the guest's tool is in the registry");
        assert_eq!(tool.exposure(), ToolExposure::Codemode);
        assert_eq!(tool.namespace().map(|n| n.name.as_str()), Some("mcp__docs"));
        assert_eq!(
            tool.namespace().and_then(|n| n.description.as_deref()),
            Some("docs server")
        );
        assert_eq!(
            tool.namespace().and_then(|n| n.instructions.as_deref()),
            Some("use the docs")
        );
        assert!(!tool.default_active());
        assert!(
            host.active_tools_filtered(&[], None, &none)
                .unwrap()
                .iter()
                .all(|t| t.name() != "exposed_guest"),
            "a codemode guest tool is not in the active set"
        );

        // An `init` that propagates the refusal of an unknown spelling fails the load.
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        let Err(err) = host
            .load_wasm_with_caps(
                "exposed-guest".into(),
                &component("sometimes", true),
                Arc::new(crate::DenyServices),
                &Capabilities::host_granted(),
            )
            .await
        else {
            panic!("an unknown exposure must fail the load");
        };
        assert!(
            err.to_string()
                .contains("invalid exposure: unknown tool exposure `sometimes`"),
            "{err}"
        );
        assert!(!host.registry().has_guest_tool("exposed_guest").unwrap());
    }
}

/// The component-level `registration` import declaring only `register-tool`, with the
/// `tool-descriptor` record at its current shape.
#[cfg(feature = "wasm-host")]
fn guest_registration_tool_import() -> String {
    r#"  (import "cyrup:ext/types@0.14.0" (instance $types
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
  (import "cyrup:ext/registration@0.14.0" (instance $reg
    (alias outer 1 $tool-descriptor (type $td))
    (export "tool-descriptor" (type $td-x (eq $td)))
    (export "register-tool" (func (param "t" $td-x) (result (result (error string)))))))
  (alias export $reg "register-tool" (func $register-tool))
"#
    .to_string()
}
