//! The weak session handles the extension host's capability backends hold.
//!
//! EXT-005 / EXT-037 / EXT-038. Each adapter backs a guest import (`ctx.isIdle()`,
//! `getCommands()`, the provenance half of `getAllTools()`) with a `Weak<AgentSession>`, so a
//! backend the session itself owns can never keep it alive.

use super::AgentSession;

/// The live [`crate::host_services::SessionActivity`] backing a guest's `ctx.isIdle()` /
/// `ctx.hasPendingMessages()` / `ctx.abort()` (EXT-005). Weak so the capability backend — which the
/// session itself owns — can never keep the session alive.
pub(super) struct SessionActivityHandle(pub(super) std::sync::Weak<AgentSession>);

impl crate::host_services::SessionActivity for SessionActivityHandle {
    fn is_idle(&self) -> bool {
        // A dropped session is not running anything: idle is the honest answer.
        self.0.upgrade().is_none_or(|s| s.is_idle())
    }

    fn pending_message_count(&self) -> usize {
        self.0.upgrade().map_or(0, |s| s.pending_message_count())
    }

    fn abort(&self) {
        if let Some(s) = self.0.upgrade() {
            s.abort();
        }
    }
}

/// The live [`crate::host_services::SessionCatalog`] backing a guest's `getCommands()` and the
/// extension-tool provenance half of its `getAllTools()` (EXT-037 / EXT-038). Weak for the same
/// reason [`SessionActivityHandle`] is.
pub(super) struct SessionCatalogHandle(pub(super) std::sync::Weak<AgentSession>);

impl crate::host_services::SessionCatalog for SessionCatalogHandle {
    fn commands(&self) -> Vec<serde_json::Value> {
        // Pi `getCommands()` = `[...extensionCommands, ...templates, ...skills]`
        // (agent-session.ts:2332-2354 @v0.83.0), which is exactly what `slash_command_catalog`
        // builds. A dropped session has no commands, which is the honest empty answer.
        self.0.upgrade().map(|s| s.slash_command_catalog()).unwrap_or_default()
    }

    fn extension_tool_source_info(&self) -> std::collections::HashMap<String, serde_json::Value> {
        // The `sourceInfo` half of pi's `_toolDefinitions` entry, for the extension-contributed
        // tools only (agent-session.ts:2482-2487: `definitionRegistry.set(name, {definition,
        // sourceInfo: tool.sourceInfo})`). `ExtensionRegistry::tool_info` already emits one row per
        // registered extension/guest tool carrying the same `SourceInfo` shape, so it is the map's
        // source; every other name in the dynamic registry is a built-in (or an SDK custom tool) and
        // gets the synthetic form on the host-services side.
        let Some(s) = self.0.upgrade() else { return std::collections::HashMap::new() };
        let Ok(rows) = s.services.ext_host.registry().tool_info() else {
            return std::collections::HashMap::new();
        };
        rows.into_iter()
            .filter_map(|r| {
                let name = r.get("name")?.as_str()?.to_string();
                Some((name, r.get("sourceInfo")?.clone()))
            })
            .collect()
    }
}
