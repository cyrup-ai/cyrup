//! The `control` WIT import: the COMMAND-tier ops. [`CommandCtx`] is the type-level half of the
//! deadlock rule (R-08-008) — the host check is authoritative and rejects any `control.*` op that
//! arrives from an event handler.

use serde::Serialize;

use crate::descriptor::{
    CompactOptions, ForkOptions, NavigateOptions, NewSessionOptions, SwitchSessionOptions,
};

use super::with_session::opts_with_callback;
use super::{Ctx, Models, ReplacedSessionContext, Session, Ui};

/// The command-tier context (pi `ExtensionCommandContext`, types.ts:353-387 @v0.83.0; EXT-072: the
/// `:339-373` this cited starts on `shutdown`'s doc line). Adds the COMMAND-only
/// `control` ops to [`Ctx`]; the host rejects any control op from an event handler (R-08-008).
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandCtx {
    base: Ctx,
}

impl CommandCtx {
    /// A command-tier context. The wrapped [`Ctx`] is a unit struct reaching the host through WIT
    /// imports, so this binds nothing and talks to no host.
    pub fn new() -> Self {
        Self { base: Ctx }
    }
    /// The base context underneath — everything an event handler gets, without the `control` ops
    /// this type adds.
    pub fn ctx(&self) -> &Ctx {
        &self.base
    }
    /// UI surface, delegating to [`Ctx::ui`].
    pub fn ui(&self) -> Ui {
        self.base.ui()
    }
    /// Read-only session view + state persistence, delegating to [`Ctx::session`].
    pub fn session(&self) -> Session {
        self.base.session()
    }
    /// Model registry view, delegating to [`Ctx::models`].
    pub fn models(&self) -> Models {
        self.base.models()
    }

    /// The base system-prompt construction options — pi `ctx.getSystemPromptOptions()`
    /// (`extensions/types.ts:355` @v0.83.0, documented at `:354` "Get the current base
    /// system-prompt construction options"), the BAG behind the string [`Ctx::system_prompt`]
    /// returns (EXT-061).
    ///
    /// COMMAND-tier, and deliberately NOT on [`Ctx`]: upstream puts `getSystemPrompt()` on the base
    /// `ExtensionContext` and this on `ExtensionCommandContext` (`:353-387`), so an event handler
    /// has the string and not the bag. Calling it from an event tier is refused host-side with the
    /// same deadlock-guard error every `control.*` op gives.
    ///
    /// The bag is `core/system-prompt.ts:8-25` @v0.83.0 — `{customPrompt?, selectedTools?,
    /// toolSnippets?, promptGuidelines?, appendSystemPrompt?, cwd, contextFiles?, skills?}` — and
    /// with no session backend the host answers pi's own default, `{cwd}` alone
    /// (`core/extensions/runner.ts:287`), rather than an error: a one-key bag is a valid answer.
    ///
    /// The host target has no host to answer at all, so it takes the `Err` arm of the module rule
    /// (see [`crate::ctx`]) rather than fabricating a bag: `Ok(Value::Null)` would let a
    /// host-target `system_prompt_options()?.get("cwd")` return `None` and assert nothing.
    pub fn system_prompt_options(&self) -> Result<serde_json::Value, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let raw = crate::guest::bindings::cyrup::ext::ctx_state::get_system_prompt_options()?;
            return serde_json::from_str(&raw).map_err(|e| format!("system prompt options: {e}"));
        }
        #[cfg(not(target_arch = "wasm32"))]
        Err("system_prompt_options unavailable on host target".into())
    }

    /// Start a new session with no options — [`Self::new_session_with`] against a default
    /// [`NewSessionOptions`].
    pub fn new_session(&self) -> Result<(), String> {
        self.new_session_with(&NewSessionOptions::default())
    }
    /// Start a new session with typed options (Pi `newSession({parentSession, withSession})`).
    pub fn new_session_with(&self, opts: &NewSessionOptions) -> Result<(), String> {
        let opts = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::NewSession(&opts))
    }
    /// Switch to `session_id` with no options — [`Self::switch_session_with`] against a default
    /// [`SwitchSessionOptions`].
    pub fn switch_session(&self, session_id: &str) -> Result<(), String> {
        self.switch_session_with(session_id, &SwitchSessionOptions::default())
    }
    /// Switch sessions with typed options (Pi `switchSession({withSession})`).
    pub fn switch_session_with(
        &self,
        session_id: &str,
        opts: &SwitchSessionOptions,
    ) -> Result<(), String> {
        let opts = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::Switch(session_id, &opts))
    }
    /// Fork at `entry_id` with no options — [`Self::fork_with`] against a default [`ForkOptions`].
    pub fn fork(&self, entry_id: &str) -> Result<(), String> {
        self.fork_with(entry_id, &ForkOptions::default())
    }
    /// Fork with typed options (Pi `fork(entryId, {position, withSession})`).
    pub fn fork_with(&self, entry_id: &str, opts: &ForkOptions) -> Result<(), String> {
        let opts = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::Fork(entry_id, &opts))
    }

    // --- withSession re-binding callbacks (pi `ReplacedSessionContext`, types.ts:394-404 @v0.83.0;
    // EXT-072 corrected `:346-390`; sdk gap #3) ---

    /// Start a new session, then run `with_session` against the re-bound session (Pi
    /// `newSession({withSession})`, types.ts:361-365 @v0.83.0; EXT-072 corrected `:346`). The closure
    /// is stored guest-side and invoked by the
    /// host's `with-session` export after the replacement completes and the command body returns —
    /// move post-replacement work here (Pi: a captured `ctx` is stale after `newSession`, runner.ts:511).
    pub fn new_session_with_callback(
        &self,
        opts: &NewSessionOptions,
        with_session: impl Fn(&ReplacedSessionContext) -> Result<(), String> + 'static,
    ) -> Result<(), String> {
        let opts_json = opts_with_callback(opts, Box::new(with_session));
        control(Control::NewSession(&opts_json))
    }

    /// Fork, then run `with_session` against the re-bound session (Pi `fork(entryId, {withSession})`,
    /// types.ts:368-371 @v0.83.0). EXT-072: the `:355` this cited is `getSystemPromptOptions`, the
    /// one command-context member cyrup does not port (EXT-061) — a mis-citation that made the gap
    /// look covered.
    pub fn fork_with_callback(
        &self,
        entry_id: &str,
        opts: &ForkOptions,
        with_session: impl Fn(&ReplacedSessionContext) -> Result<(), String> + 'static,
    ) -> Result<(), String> {
        let opts_json = opts_with_callback(opts, Box::new(with_session));
        control(Control::Fork(entry_id, &opts_json))
    }

    /// Switch sessions, then run `with_session` against the re-bound session (Pi
    /// `switchSession({withSession})`, types.ts:380-383 @v0.83.0; EXT-072: `:368` is `fork(`).
    pub fn switch_session_with_callback(
        &self,
        session_id: &str,
        opts: &SwitchSessionOptions,
        with_session: impl Fn(&ReplacedSessionContext) -> Result<(), String> + 'static,
    ) -> Result<(), String> {
        let opts_json = opts_with_callback(opts, Box::new(with_session));
        control(Control::Switch(session_id, &opts_json))
    }
    /// Navigate to `entry_id` with author-supplied options.
    ///
    /// `opts` encoding is fallible; the failure is returned as `Err` rather than navigating with an
    /// empty option bag the author never asked for.
    pub fn navigate(&self, entry_id: &str, opts: impl Serialize) -> Result<(), String> {
        let opts = serde_json::to_string(&opts).map_err(|e| format!("navigate: {e}"))?;
        control(Control::Navigate(entry_id, &opts))
    }
    /// Navigate the session tree with typed options (Pi `navigateTree(targetId, {summarize, …})`).
    pub fn navigate_with(&self, entry_id: &str, opts: &NavigateOptions) -> Result<(), String> {
        self.navigate(entry_id, opts)
    }
    /// Send the host's `reload` control op (WIT `control.reload`, `wit/world.wit:1057`). Like every
    /// `control.*` op it is command-tier — the host's handler opens with `require_command_tier`
    /// (`cyrup-ext/src/host/live.rs:1130-1134`), so an event handler gets the deadlock-guard error.
    pub fn reload(&self) -> Result<(), String> {
        control(Control::Reload)
    }
    /// Trigger a compaction with no extra guidance (Pi `ctx.compact()`, types.ts:344).
    pub fn compact(&self) -> Result<(), String> {
        self.compact_with(&CompactOptions::default())
    }

    /// Trigger a compaction with typed options (Pi `ctx.compact(options)`, types.ts:344 +
    /// `CompactOptions`, types.ts:296-300). `custom_instructions` reaches the summarizer that
    /// writes the compaction summary. Fire-and-forget, exactly as in Pi: the call returns once the
    /// host has queued the op — subscribe to the `session_compact` event for the result (see
    /// [`CompactOptions`] on why the `onComplete`/`onError` callbacks have no cross-boundary form).
    pub fn compact_with(&self, opts: &CompactOptions) -> Result<(), String> {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::Compact(&opts_json))
    }
    /// Send the host's `wait-idle` control op (WIT `control.wait-idle`, `wit/world.wit:1063`).
    /// Command-tier: the host's handler opens with `require_command_tier`
    /// (`cyrup-ext/src/host/live.rs:1151-1155`).
    pub fn wait_idle(&self) -> Result<(), String> {
        control(Control::WaitIdle)
    }
    /// Set the model. NOT command-only (EXT-074 / GAP-11) — this is a delegating wrapper kept for
    /// source compatibility; the implementation and its citations live on [`Models::set_model`].
    ///
    /// `model` is encoded HERE, not in the delegate, so an author-type encode failure is returned
    /// as `Err` instead of setting the model to `null`. This is the one failure the WIT
    /// `set-model: func(model-json: string)` (void) lets the SDK see; a host-side rejection is not
    /// observable through the return value. The [`serde_json::Value`] handed to the delegate
    /// re-encodes infallibly.
    pub fn set_model(&self, model: impl Serialize) -> Result<(), String> {
        let model = serde_json::to_value(model).map_err(|e| format!("set_model: {e}"))?;
        self.models().set_model(model)
    }
    /// Queue an author-supplied message.
    ///
    /// Both `message` and `opts` are author-supplied; either encoding failing is returned as `Err`
    /// rather than sending a `null` message or dropping the options.
    pub fn send_message(&self, message: impl Serialize, opts: impl Serialize) -> Result<(), String> {
        let m = serde_json::to_string(&message).map_err(|e| format!("send_message message: {e}"))?;
        let o = serde_json::to_string(&opts).map_err(|e| format!("send_message opts: {e}"))?;
        control(Control::SendMessage(&m, &o))
    }
    /// Queue a user-authored message with author-supplied options.
    ///
    /// An `opts` encode failure is returned as `Err` rather than sending with an empty option bag.
    pub fn send_user_message(&self, content: &str, opts: impl Serialize) -> Result<(), String> {
        let o = serde_json::to_string(&opts).map_err(|e| format!("send_user_message opts: {e}"))?;
        control(Control::SendUserMessage(content, &o))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
enum Control<'a> {
    NewSession(&'a str),
    Switch(&'a str, &'a str),
    Fork(&'a str, &'a str),
    Navigate(&'a str, &'a str),
    Reload,
    Compact(&'a str),
    WaitIdle,
    SendMessage(&'a str, &'a str),
    SendUserMessage(&'a str, &'a str),
}

fn control(op: Control<'_>) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        use crate::guest::bindings::cyrup::ext::control as c;
        return match op {
            Control::NewSession(o) => c::new_session(o),
            Control::Switch(id, o) => c::switch(id, o),
            Control::Fork(id, o) => c::fork(id, o),
            Control::Navigate(id, o) => c::navigate(id, o),
            Control::Reload => c::reload(),
            Control::Compact(o) => c::compact(o),
            Control::WaitIdle => c::wait_idle(),
            Control::SendMessage(m, o) => c::send_message(m, o),
            Control::SendUserMessage(co, o) => c::send_user_message(co, o),
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = op;
        Ok(())
    }
}
