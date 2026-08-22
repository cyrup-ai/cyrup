//! The `models` WIT import: the model registry view. `set_model` is command-tier and lives in
//! `command`; `set_thinking_level` is not tier-gated (EXT-074) and lives here.

use serde_json::Value;

/// The model registry view (Pi `ctx.modelRegistry: ModelRegistry`, `extensions/types.ts:319`
/// @v0.83.0, plus `ctx.scopedModels` `:326`, `getContextUsage()` `:341`, `pi.setModel(model)`
/// `:1336` and `pi.setThinkingLevel(level)` `:1342`).
///
/// EXT-036: this cited `types.ts:1273-1279`, which is `registerMessageRenderer`/
/// `registerEntryRenderer` — a different surface entirely, not an off-by-N.
#[derive(Clone, Copy, Debug, Default)]
pub struct Models;

impl Models {
    /// The models scoped to this session — pi `ctx.scopedModels: readonly ScopedModel[]`
    /// (`extensions/types.ts:326` @v0.83.0): "Models scoped to this session (resolved from
    /// `--models` / `enabledModels` settings against the available catalogue). Same set the
    /// `/scoped-models` command shows. Empty when no scoping is configured (all available models
    /// are usable)." EXT-045 — without it a guest could not offer a model picker restricted to the
    /// session's scoped set.
    pub fn scoped(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::models::scoped_models());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }

    pub fn list(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::models::list_models());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }
    pub fn current(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::current();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    pub fn context_usage(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::models::context_usage());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Null
    }
    pub fn thinking_level(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::thinking_level();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }

    /// Set the thinking level (Pi `setThinkingLevel(level)`, `types.ts:1342` @v0.83.0; EXT-036
    /// corrected `:1288`; sdk gap #25 / GAP-11).
    ///
    /// Pi allows `setThinkingLevel` from ANY handler (factory-tier `pi.*`, `loader.ts:369-372` /
    /// `runner.ts:336`, no tier gate) and it takes effect. cyrup now matches this: the call is QUEUED
    /// as a control op and applied at the store-free turn-boundary drain
    /// (`AgentSession::apply_pending_control`), so its `thinking_level_select` re-emit
    /// (`agent-session.ts:1560-1567`) runs as a fresh top-level guest call after the event hook's wasm
    /// store guard is released — never a re-entry into the suspended single-instance store (the
    /// R-08-008 deadlock the old command-tier gate guarded against is dissolved by deferral). So this
    /// returns `Ok(())` and the new level takes effect on the SUBSEQUENT turn, whether called from a
    /// command handler or an event handler.
    pub fn set_thinking_level(&self, level: &str) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::set_thinking_level(level);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = level;
            Ok(())
        }
    }
}
