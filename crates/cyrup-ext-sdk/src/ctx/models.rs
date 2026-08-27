//! The `models` WIT import: the model registry view. Neither `set_model` nor `set_thinking_level`
//! is tier-gated (EXT-074 / GAP-11), so both live here: the host takes the ungated `guest_of` for
//! each and QUEUES the op, which is applied at the store-free turn-boundary drain
//! (`cyrup-ext/src/host/live.rs:565-577` for `set_model`, `:584-601` for `set_thinking_level`), so
//! an event-tier call takes effect on the SUBSEQUENT turn instead of being dropped or rejected.
//! This is the same position as [`Ctx::models`](super::Ctx::models) (`src/ctx/base.rs:58-62`) and
//! the `set-model` block in `wit/world.wit:778-786`. [`CommandCtx::set_model`] remains only as a
//! delegating wrapper, for source compatibility.
//!
//! [`CommandCtx::set_model`]: crate::CommandCtx::set_model

use serde::Serialize;
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
    ///
    /// [`Value::Null`] — NOT the empty array that means "no scoping is configured" — when the host
    /// sent JSON this SDK could not parse (`super::parse_json`'s fallback). The two are different
    /// answers and a caller matching on `as_array()` must not treat them alike. The WIT import
    /// promises a JSON array (`wit/world.wit:776`).
    pub fn scoped(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::models::scoped_models());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }

    /// Every model in the registry, as the WIT `models.list-models` import's "json array of model
    /// refs" (`wit/world.wit:771`).
    ///
    /// [`Value::Null`] — NOT an empty array — when the host sent JSON this SDK could not parse
    /// (`super::parse_json`'s fallback), so a caller that treats a non-array as "no models" cannot
    /// tell the two apart. On the host (non-`wasm32`) target there is no host to ask and this is
    /// always an empty array.
    pub fn list(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::models::list_models());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }
    /// The model currently selected for this session (WIT `models.current`, `wit/world.wit:777`);
    /// `None` when the host has none to report, and always `None` on the host (non-`wasm32`)
    /// target, where there is no host to ask.
    pub fn current(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::current();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    /// The session's context-window usage, through the WIT `models.context-usage` import
    /// (`wit/world.wit:787`); see the [`Models`] type doc for the Pi surface this mirrors.
    ///
    /// [`Value::Null`] means ONLY "the host sent JSON this SDK could not parse"
    /// (`super::parse_json`'s fallback) — the host's own no-usage answer is the empty OBJECT `{}`
    /// (`cyrup-ext/src/host/live.rs:579`, and the `HostServices::context_usage` default at
    /// `cyrup-ext/src/host/services.rs:454-456`), which parses fine. On the host (non-`wasm32`)
    /// target there is no host to ask and this is always [`Value::Null`].
    pub fn context_usage(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::models::context_usage());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Null
    }
    /// The session's current thinking level (WIT `models.thinking-level`, `wit/world.wit:788`) —
    /// the value [`Self::set_thinking_level`] writes; `None` when the host has none to report, and
    /// always `None` on the host (non-`wasm32`) target.
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
    /// R-08-008 deadlock the old command-only gate guarded against is dissolved by deferral). So this
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

    /// Set the model (WIT `models.set-model`, `wit/world.wit:786`; EXT-074 / GAP-11).
    ///
    /// Callable from ANY tier, exactly like [`Self::set_thinking_level`]: the host's `set_model`
    /// (`cyrup-ext/src/host/live.rs:565-577`) opens with the ungated `guest_of` and QUEUES a
    /// `ControlOp::SetModel` applied at the store-free turn-boundary drain
    /// (`AgentSession::apply_pending_control`), so an event-tier call takes effect on the
    /// SUBSEQUENT turn rather than being dropped. The WIT import returns void, so the `Ok(())`
    /// here says only that the encoded ref reached the host, never that the model was accepted —
    /// the guest observes the EFFECT. [`CommandCtx::set_model`] delegates here.
    ///
    /// [`CommandCtx::set_model`]: crate::CommandCtx::set_model
    pub fn set_model(&self, model: impl Serialize) -> Result<(), String> {
        let m = serde_json::to_string(&model).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        {
            crate::guest::bindings::cyrup::ext::models::set_model(&m);
            return Ok(());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = m;
            Ok(())
        }
    }
}
