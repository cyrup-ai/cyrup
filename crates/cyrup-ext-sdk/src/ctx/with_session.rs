//! The guest-side `withSession` re-binding registry (pi `ReplacedSessionContext`,
//! types.ts:394-404 @v0.83.0; EXT-072 corrected `:374-390`; sdk gap #3).
//!
//! A `newSession`/`fork`/`switchSession` invalidates the captured context, so post-replacement work
//! is handed over as a closure: the guest stores it here, embeds its id in the `control.*` opts, and
//! the host calls back through the `with-session` export once the session is re-bound.

use core::cell::RefCell;
use std::collections::HashMap;

use serde::Serialize;
use serde_json::json;

use super::CommandCtx;

/// A guest `withSession(ctx)` re-binding closure (Pi types.ts:382).
pub type WithSessionFn = Box<dyn Fn(&ReplacedSessionContext) -> Result<(), String> + 'static>;

thread_local! {
    /// `(next_id, id -> closure)` — the pending `withSession` closures (single-threaded wasm guest).
    static WITH_SESSION: RefCell<(u64, HashMap<String, WithSessionFn>)> =
        RefCell::new((0, HashMap::new()));
}

/// Store a `withSession` closure, returning the id embedded in the `control.*` opts so the host can
/// schedule the matching `with-session` export call after re-binding the session (sdk gap #3).
#[doc(hidden)]
pub fn register_with_session(f: WithSessionFn) -> String {
    WITH_SESSION.with(|c| {
        let mut g = c.borrow_mut();
        g.0 += 1;
        let id = format!("ws-{}", g.0);
        g.1.insert(id.clone(), f);
        id
    })
}

/// Run (and consume) the stored `withSession` closure for `id` against a freshly-bound
/// [`ReplacedSessionContext`] — the host calls this via the `with-session` export after the session
/// is re-bound. An unknown id is a no-op (never an error).
#[doc(hidden)]
pub fn run_with_session(id: &str) -> Result<(), String> {
    let f = WITH_SESSION.with(|c| c.borrow_mut().1.remove(id));
    match f {
        Some(f) => f(&ReplacedSessionContext::new()),
        None => Ok(()),
    }
}

/// Serialize `opts` and inject the registered `withSession` callback id (sdk gap #3).
///
/// **On an `opts` encode failure the options are replaced with `{}`** and only the callback id
/// survives, so the re-binding still happens but with host defaults. Every in-crate caller
/// (`new_session_with_callback`, `fork_with_callback`, `switch_session_with_callback`) passes a
/// concrete SDK options struct whose `Serialize` is derived and cannot fail, so the substitution is
/// unreachable through the public API; it exists only because the parameter is `impl Serialize`.
pub(super) fn opts_with_callback(opts: impl Serialize, with_session: WithSessionFn) -> String {
    let id = register_with_session(with_session);
    // Encode failure -> `{}` (unreachable through the public API, see the doc comment above).
    let mut v = serde_json::to_value(&opts).unwrap_or_else(|_| json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert("withSessionCallbackId".into(), json!(id));
    }
    v.to_string()
}

/// A fresh command-capable context bound to the replacement session after `newSession`/`fork`/
/// `switchSession` (pi `ReplacedSessionContext extends ExtensionCommandContext`, types.ts:394-404
/// @v0.83.0;
/// sdk gap #3). Passed to the `withSession` closure. Derefs to [`CommandCtx`], so every command-tier
/// op (incl. `send_message`/`send_user_message`) is available on the re-bound session.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReplacedSessionContext {
    cmd: CommandCtx,
}

impl ReplacedSessionContext {
    /// A context over the replacement session. [`CommandCtx`] is a unit struct reaching the host
    /// through WIT imports, so this binds nothing and is what [`run_with_session`] hands the stored
    /// closure.
    pub fn new() -> Self {
        Self { cmd: CommandCtx::new() }
    }
    /// The underlying command-tier context bound to the replacement session.
    pub fn command(&self) -> &CommandCtx {
        &self.cmd
    }
}

impl core::ops::Deref for ReplacedSessionContext {
    type Target = CommandCtx;
    fn deref(&self) -> &CommandCtx {
        &self.cmd
    }
}
