//! Active-tool / command introspection (Pi getActiveTools/…/getCommands, `types.ts:1320`/`:1329`
//! @v0.83.0; EXT-036 corrected `:1257-1266`) — the `ext-tools` and `registration` WIT imports. Pi
//! puts these on the base context, so they are `impl Ctx`, not `impl CommandCtx`.

use serde_json::Value;

use super::Ctx;

impl Ctx {
    /// The names of the currently-active tools (Pi `getActiveTools`).
    ///
    /// The returned `Vec` is EMPTY both when no tool is active and when the host sent JSON this SDK
    /// could not parse: `super::parse_json` yields [`Value::Null`] on a parse failure, whose
    /// `as_array()` is `None`, which this folds to the same empty `Vec` as an empty array.
    pub fn active_tools(&self) -> Vec<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::ext_tools::get_active_tools())
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
        }
        #[cfg(not(target_arch = "wasm32"))]
        Vec::new()
    }
    /// All configured tools with metadata (Pi `getAllTools` → `ToolInfo[]`).
    ///
    /// [`Value::Null`] — NOT an empty array — when the host sent JSON this SDK could not parse
    /// (`super::parse_json`'s fallback), so a caller that treats a non-array as "no tools" cannot
    /// tell the two apart. The WIT import promises a JSON array (`wit/world.wit:969`).
    pub fn all_tools(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::ext_tools::get_all_tools());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }
    /// Restrict the active tool set by name (Pi `setActiveTools`; plan-mode-style restriction).
    pub fn set_active_tools(&self, names: &[&str]) {
        let names_json = serde_json::to_string(names).unwrap_or_else(|_| "[]".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ext_tools::set_active_tools(&names_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = names_json;
    }
    /// Available slash commands (Pi `getCommands` → `SlashCommandInfo[]`).
    ///
    /// [`Value::Null`] — NOT an empty array — when the host sent JSON this SDK could not parse
    /// (`super::parse_json`'s fallback), so a caller that treats a non-array as "no commands"
    /// cannot tell the two apart. The WIT import promises a JSON array (`wit/world.wit:974`).
    pub fn commands(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::ext_tools::get_commands());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }

    /// Read a registered flag's resolved VALUE (Pi `getFlag(name)`, `types.ts:1269` @v0.83.0 (EXT-036 corrected `:1218`); sdk gap #23). The
    /// WIT `registration.get-flag` import returns the value (its default / CLI override) as JSON; this
    /// wraps it. `None` in THREE cases: the flag is unregistered, the flag has no value (Pi
    /// `undefined` — the host's own two, `cyrup-ext/src/host/services.rs:1824-1825`), or the host
    /// returned a string this SDK could not parse as JSON. The third is folded into the same `None`
    /// because Pi's `getFlag` has no error channel; the host serializes a `serde_json::Value`
    /// (`services.rs:1840`/`:1850`, `.to_string()`), so it is defensive against a non-cyrup host
    /// rather than reachable today.
    pub fn flag(&self, name: &str) -> Option<Value> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::registration::get_flag(name)
                .and_then(|s| serde_json::from_str(&s).ok());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = name;
            None
        }
    }

    /// Unregister a custom provider previously registered by this extension (Pi `unregisterProvider`,
    /// `types.ts:1416` @v0.83.0 (EXT-036 corrected `:1361`); sdk gap #24). Wraps the existing WIT `registration.unregister-provider` import.
    pub fn unregister_provider(&self, id: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::registration::unregister_provider(id);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = id;
    }
}
