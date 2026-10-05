//! Every built-in tool is `direct`, default-active, in no namespace, and has no loadout hook.
//!
//! pi's built-in tool definitions (`core/tools/*.ts` @v1.0.1) set none of `exposure`, `namespace`,
//! `defaultActive` or `prepareLoadout`, so each reads as the default: `direct` (`_getToolExposure`'s
//! `?? "direct"`, `core/agent-session.ts:1507`), active on registration
//! (`defaultActive !== false`, `:3554`), ungrouped, and no hook. Nothing in this crate may decide
//! otherwise — which tools a request declares is the loadout's job (`cyrup_core::exposure`), and
//! this crate's `Availability` only selects built-ins by name.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use cyrup_core::{
    CancelToken, LoadoutView, Tool, ToolCallId, ToolError, ToolExposure, ToolLoadout,
    ToolLoadoutChanges, ToolResult, ToolUpdateSink,
};

use crate::ToolsOptions;
use crate::ops::Backend;
use crate::registry::{Availability, ToolRegistry};

/// A registered tool whose own hook runs the built-ins' hooks against the view the loadout hands
/// it, which is the only way to call [`Tool::prepare_loadout`] (a [`LoadoutView`] is built by the
/// loadout and nothing else).
struct Probe {
    schema: serde_json::Value,
    results: Arc<Mutex<Vec<(String, ToolLoadoutChanges)>>>,
}

#[async_trait::async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        "probe"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.schema
    }
    fn prepare_loadout(&self, view: &LoadoutView<'_>) -> Result<ToolLoadoutChanges, ToolError> {
        for tool in view.registered() {
            if tool.name() == "probe" {
                continue;
            }
            let changes = tool.prepare_loadout(view)?;
            self.results
                .lock()
                .unwrap()
                .push((tool.name().to_string(), changes));
        }
        Ok(ToolLoadoutChanges::default())
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::default())
    }
}

#[test]
fn every_builtin_is_direct_default_active_ungrouped_and_has_no_loadout_hook() {
    let reg = ToolRegistry::with_builtins(
        std::env::temp_dir(),
        Backend::default(),
        ToolsOptions::default(),
    );
    let builtins = reg.visible(&Availability::All);
    assert_eq!(builtins.len(), 8, "the whole built-in registry is covered");

    for tool in &builtins {
        let name = tool.name();
        assert_eq!(tool.exposure(), ToolExposure::Direct, "{name}");
        assert!(tool.default_active(), "{name}");
        assert!(tool.namespace().is_none(), "{name}");
        assert!(
            tool.exposure()
                .activated_on_registration(tool.default_active()),
            "{name}: a built-in is active on registration"
        );
    }

    // `prepare_loadout` returns the empty default for every built-in: no descriptions, no hidden
    // declarations.
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut registry: Vec<Arc<dyn Tool>> = vec![Arc::new(Probe {
        schema: serde_json::json!({"type": "object"}),
        results: Arc::clone(&results),
    })];
    registry.extend(builtins.iter().cloned());
    let requested: Vec<String> = registry.iter().map(|t| t.name().to_string()).collect();
    let loadout = ToolLoadout::resolve(&requested, &registry);

    assert!(
        loadout.hook_failures().is_empty(),
        "{:?}",
        loadout.hook_failures()
    );
    let results = results.lock().unwrap();
    assert_eq!(
        results.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        builtins.iter().map(|t| t.name()).collect::<Vec<_>>(),
        "the probe ran every built-in's hook, in registry order"
    );
    for (name, changes) in results.iter() {
        assert_eq!(
            *changes,
            ToolLoadoutChanges::default(),
            "{name}: built-ins make no loadout changes"
        );
    }

    // And the loadout advertises all of them, unaltered.
    let advertised = loadout.advertised();
    for tool in &builtins {
        assert!(advertised.contains(tool.name()), "{}", tool.name());
    }
    for decl in advertised.declarations() {
        if let Some(tool) = builtins.iter().find(|t| t.name() == decl.name) {
            assert_eq!(decl.description, tool.description(), "{}", decl.name);
        }
    }
}
