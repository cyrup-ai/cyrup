//! EXT-107 — an extension draws calls to a tool it did not register, on the NATIVE tier.
//!
//! pi `pi.registerToolRenderer(resolver)` + `ExtensionRunner.resolveToolRenderers(toolName, base)`
//! (`core/extensions/types.ts:1696`, `runner.ts:790-795` @f1b2e77f5): resolvers run in extension
//! load order, `next()` is what the rest of the chain and then the registered tool would use.
//! Before EXT-107 `ExtensionHost::{has_tool_renderer, render_tool_*_outcome}` answered "nothing"
//! for any name with no exact-name owner, so no extension could draw such a call.
//!
//! Every test asserts through the host's public resolution surface (`has_tool_renderer` and the
//! two `render_tool_*_outcome` methods), never through the registry table.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{
    ExtError, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi,
    NativeExtension, RenderOptions, RenderOutcome, ToolRendererMode, ToolRenderers,
};
use cyrup_core::ExtensionId;
use serde_json::{Value, json};
use std::sync::Arc;

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

/// How a test extension declares its family.
#[derive(Clone)]
enum Declares {
    Pattern(&'static str, ToolRendererMode),
    /// A real callback: pi's MCP idiom, `next() ?? (name.startsWith(prefix) ? mine : undefined)`.
    FillInCallback(&'static str),
    /// An exact-name renderer only (the pre-EXT-107 surface).
    Exact(&'static str),
}

/// A native that declares one renderer and draws `<id>:<surface>[<key>]`.
struct Drawer {
    id: &'static str,
    declares: Declares,
    /// Register a broken provider AFTER the declaration, so the load fails late (EXT-081's shape).
    fail_late: bool,
}

#[async_trait::async_trait]
impl NativeExtension for Drawer {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        match self.declares.clone() {
            Declares::Pattern(p, mode) => api.register_tool_renderer_pattern(p, mode)?,
            Declares::FillInCallback(prefix) => {
                api.register_tool_renderer_resolver(move |name, next| {
                    next().or_else(|| name.starts_with(prefix).then(ToolRenderers::mine))
                })
            }
            Declares::Exact(name) => api.register_tool_renderer(name),
        }
        if self.fail_late {
            api.register_provider("ext107_broken", json!({"name": 7}));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn render_call(&self, key: &str, _call: &Value) -> Option<Value> {
        Some(Value::String(format!("{}:call[{key}]", self.id)))
    }
    fn render_result(&self, key: &str, _result: &Value) -> Option<Value> {
        Some(Value::String(format!("{}:result[{key}]", self.id)))
    }
}

fn drawer(id: &'static str, declares: Declares) -> Arc<Drawer> {
    Arc::new(Drawer {
        id,
        declares,
        fail_late: false,
    })
}

async fn host_with(exts: Vec<Arc<Drawer>>) -> ExtensionHost {
    let host = ExtensionHost::new(cfg());
    for ext in exts {
        host.load_native(ext).await.unwrap();
    }
    host
}

/// What the host draws for `name`'s CALL row: the renderer's text, or `None`.
async fn call_row(host: &ExtensionHost, name: &str) -> Option<String> {
    match host
        .render_tool_call_outcome(name, &json!({}), &RenderOptions::default())
        .await
    {
        RenderOutcome::Rendered(Value::String(s)) => Some(s),
        RenderOutcome::None => None,
        other => panic!("unexpected outcome for {name}: {other:?}"),
    }
}

async fn result_row(host: &ExtensionHost, name: &str) -> Option<String> {
    match host
        .render_tool_result_outcome(name, &json!({}), &RenderOptions::default())
        .await
    {
        RenderOutcome::Rendered(Value::String(s)) => Some(s),
        RenderOutcome::None => None,
        other => panic!("unexpected outcome for {name}: {other:?}"),
    }
}

/// Clause 1 — a declared family draws a call to a name NO extension registered, on the pre-check
/// and both surfaces; a name outside the family is still drawn by nobody.
#[tokio::test]
async fn a_declared_family_draws_a_tool_no_extension_registered() {
    let host = host_with(vec![drawer(
        "fam",
        Declares::Pattern("mcp__*", ToolRendererMode::FillIn),
    )])
    .await;

    assert!(host.has_tool_renderer("mcp__srv__ghost"));
    assert_eq!(
        call_row(&host, "mcp__srv__ghost").await.as_deref(),
        Some("fam:call[mcp__srv__ghost]")
    );
    assert_eq!(
        result_row(&host, "mcp__srv__ghost").await.as_deref(),
        Some("fam:result[mcp__srv__ghost]")
    );
    // Not a vacuous "nothing renders": the same host draws the family member above.
    assert!(!host.has_tool_renderer("other_ghost"));
    assert_eq!(call_row(&host, "other_ghost").await, None);
}

/// Clause 1, the CALLBACK form — pi's MCP resolver written as a native closure.
#[tokio::test]
async fn a_native_callback_resolver_draws_a_tool_no_extension_registered() {
    let host = host_with(vec![drawer("cb", Declares::FillInCallback("mcp__"))]).await;
    assert!(host.has_tool_renderer("mcp__srv__ghost"));
    assert_eq!(
        call_row(&host, "mcp__srv__ghost").await.as_deref(),
        Some("cb:call[mcp__srv__ghost]")
    );
    assert!(!host.has_tool_renderer("ghost"));
}

/// Clause 2 — a fill-in family leaves a tool that has its OWN renderer to that renderer (pi's
/// `next() ?? mine`), whichever side of the declaring extension it loaded on; and it leaves a
/// BUILT-IN to the host's built-in drawing rather than taking it over.
#[tokio::test]
async fn a_registered_renderer_still_wins_over_a_fill_in_family() {
    for owner_first in [true, false] {
        let fam = drawer("fam", Declares::Pattern("mcp__*", ToolRendererMode::FillIn));
        let own = drawer("own", Declares::Exact("mcp__srv__owned"));
        let host = host_with(if owner_first {
            vec![own, fam]
        } else {
            vec![fam, own]
        })
        .await;
        assert_eq!(
            call_row(&host, "mcp__srv__owned").await.as_deref(),
            Some("own:call[mcp__srv__owned]"),
            "owner loaded first = {owner_first}"
        );
        assert_eq!(
            result_row(&host, "mcp__srv__owned").await.as_deref(),
            Some("own:result[mcp__srv__owned]")
        );
        // ...while the family still draws its unowned sibling.
        assert_eq!(
            call_row(&host, "mcp__srv__ghost").await.as_deref(),
            Some("fam:call[mcp__srv__ghost]")
        );
    }

    let everything = host_with(vec![drawer(
        "all",
        Declares::Pattern("*", ToolRendererMode::FillIn),
    )])
    .await;
    assert!(
        !everything.has_tool_renderer("bash"),
        "a built-in keeps its built-in drawing (pi's base() is withBuiltInRenderers)"
    );
    assert_eq!(call_row(&everything, "bash").await, None);
    assert!(everything.has_tool_renderer("ghost"));
}

/// Clause 2's counterpart — a CLAIM draws a matching tool even when it has its own renderer
/// (pi's `matches ? mine : next()`).
#[tokio::test]
async fn a_claim_draws_ahead_of_the_registered_renderer() {
    let host = host_with(vec![
        drawer(
            "claim",
            Declares::Pattern("mcp__*", ToolRendererMode::Claim),
        ),
        drawer("own", Declares::Exact("mcp__srv__owned")),
    ])
    .await;
    assert_eq!(
        call_row(&host, "mcp__srv__owned").await.as_deref(),
        Some("claim:call[mcp__srv__owned]")
    );
    assert_eq!(
        call_row(&host, "owned_elsewhere").await,
        None,
        "outside the claim nothing changes"
    );
}

/// Clause 3 — two extensions claiming one family resolve in LOAD ORDER: reversing the load order
/// reverses the winner.
#[tokio::test]
async fn two_claims_resolve_in_load_order() {
    let a = || drawer("a", Declares::Pattern("mcp__*", ToolRendererMode::Claim));
    let b = || drawer("b", Declares::Pattern("mcp__*", ToolRendererMode::Claim));
    let ab = host_with(vec![a(), b()]).await;
    assert_eq!(
        call_row(&ab, "mcp__s__t").await.as_deref(),
        Some("a:call[mcp__s__t]")
    );
    let ba = host_with(vec![b(), a()]).await;
    assert_eq!(
        call_row(&ba, "mcp__s__t").await.as_deref(),
        Some("b:call[mcp__s__t]")
    );
}

/// Clause 3, fill-ins — pi's composition: the first-loaded `next() ?? mine` consults `next()`
/// first, so the LATER-loaded fill-in draws. Reversing the load order reverses it, and a callback
/// and a pattern compose in the same chain.
#[tokio::test]
async fn two_fill_ins_resolve_through_next_in_load_order() {
    let pat = || drawer("pat", Declares::Pattern("mcp__*", ToolRendererMode::FillIn));
    let cb = || drawer("cb", Declares::FillInCallback("mcp__"));
    let host = host_with(vec![cb(), pat()]).await;
    assert_eq!(
        call_row(&host, "mcp__s__t").await.as_deref(),
        Some("pat:call[mcp__s__t]")
    );
    let host = host_with(vec![pat(), cb()]).await;
    assert_eq!(
        call_row(&host, "mcp__s__t").await.as_deref(),
        Some("cb:call[mcp__s__t]")
    );
}

/// Clause 4 — a declaration goes with its owner. An extension whose load fails AFTER it declared
/// leaves no resolver behind (EXT-081's purge), and `purge_owner` (the same sweep) drops a loaded
/// one, while a sibling's declaration keeps drawing.
#[tokio::test]
async fn purging_the_declaring_extension_removes_its_declaration() {
    let host = host_with(vec![drawer(
        "sibling",
        Declares::Pattern("other_*", ToolRendererMode::FillIn),
    )])
    .await;
    let failing = Arc::new(Drawer {
        id: "failing",
        declares: Declares::Pattern("mcp__*", ToolRendererMode::Claim),
        fail_late: true,
    });
    assert!(host.load_native(failing).await.is_err());
    assert!(
        !host.has_tool_renderer("mcp__s__t"),
        "a failed load's declaration is gone"
    );
    assert_eq!(call_row(&host, "mcp__s__t").await, None);

    host.load_native(drawer(
        "loaded",
        Declares::Pattern("mcp__*", ToolRendererMode::Claim),
    ))
    .await
    .unwrap();
    assert!(host.has_tool_renderer("mcp__s__t"));
    host.registry()
        .purge_owner(&ExtensionId::from("loaded"))
        .unwrap();
    assert!(!host.has_tool_renderer("mcp__s__t"));
    assert_eq!(
        call_row(&host, "other_x").await.as_deref(),
        Some("sibling:call[other_x]"),
        "the sibling's declaration survives the purge"
    );
}

/// NON-REGRESSION GUARD (green before EXT-107): with no resolver at all, an exact-name renderer
/// still draws and an unknown name still does not.
#[tokio::test]
async fn non_regression_exact_renderers_without_any_resolver() {
    let host = host_with(vec![drawer("own", Declares::Exact("owned"))]).await;
    assert!(host.has_tool_renderer("owned"));
    assert_eq!(
        call_row(&host, "owned").await.as_deref(),
        Some("own:call[owned]")
    );
    assert!(!host.has_tool_renderer("ghost"));
    assert_eq!(call_row(&host, "ghost").await, None);
}

/// An empty pattern is refused at declaration, and refuses the load.
#[tokio::test]
async fn an_empty_pattern_is_refused() {
    let host = ExtensionHost::new(cfg());
    let err = host
        .load_native(drawer(
            "empty",
            Declares::Pattern("", ToolRendererMode::FillIn),
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, ExtError::Registration(_)), "{err:?}");
}
