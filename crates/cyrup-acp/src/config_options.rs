//! The request surface: the `initialize` advertisement, and the one enum that both advertises and
//! accepts a session config option.
//!
//! **Owner: agent E (area 4b — `ACP-052`…`ACP-054`, `ACP-062`…`ACP-064`, `ACP-072`…`ACP-077`).**
//!
//! ADR-0028 finding F5. Port of pi-acp v0.0.33 `src/acp/agent.ts`'s `MODEL_CONFIG_ID` /
//! `THOUGHT_LEVEL_CONFIG_ID` constants, `isThinkingLevel`, `getThinkingState`, `buildConfigOptions`,
//! `getModelState`, `getSessionConfiguration`, `emitConfigOptionsUpdate`, `setSessionModel`,
//! `setSessionMode` and `setSessionConfigOption`, plus `src/acp/auth.ts`'s `getAuthMethods` and the
//! `agentCapabilities` literal in `initialize`.
//!
//! # The bug this shape exists to prevent
//!
//! Upstream's advertised option list and its accepted-value validator are **two independent
//! literals over the same string space**: `getThinkingState`'s `available: ThinkingLevel[]` lists
//! six values and `isThinkingLevel` is a predicate listing the same six. cyrup's
//! `ModelThinkingLevel` (`crates/cyrup-core/src/message/thinking.rs`) has a **seventh** rung,
//! `Max` ("Pi added `max` in fbdd4638"). Advertise from `AgentSession::available_thinking_levels()`
//! — the natural in-process source — while porting `isThinkingLevel`'s six-string predicate
//! verbatim, and the client renders a `max` entry in its dropdown that the agent then rejects with
//! `invalidParams: Unknown thinking level: max`. The user sees an option that does not work.
//!
//! One enum, one id space, [`SessionConfigKnob::advertise`] and [`SessionConfigKnob::parse`] as
//! exact inverses, so they cannot drift. [`thinking_level_id`] and [`thinking_level_from_id`] are
//! the single spelling of a level, used by the advertiser, both parsers and the mode list.
//!
//! # `session/set_mode` is `Thinking` under another name
//!
//! `ACP-062` — **the ACP mode list IS the thinking-level ladder.** `setSessionMode` and
//! `setSessionConfigOption('thought_level')` are two code paths for one operation upstream, with
//! two different error strings. [`SessionConfigKnob::parse_mode`] is the second entry point onto
//! the same validator, and both routes converge on [`SessionConfigKnob::apply`].
//!
//! # [CYRUP-DELTA] — `ACP-Q20`: the setters do not notify, the pump does (`ACP-077`)
//!
//! **What differs.** Upstream emits `current_mode_update` and `config_option_update` from inside
//! `setSessionMode` / `setSessionConfigOption`, and emits **nothing** when the same state changes
//! by any other route — an extension calling `pi.setModel`, a queued command, `cycle_model`. That
//! is a latent defect upstream (`ACP-077`): the client's dropdown silently goes stale.
//! `AgentSessionEvent::{ModelChanged, ThinkingLevelChanged, SessionInfoChanged}` are emitted by
//! `set_model_resolved` / `apply_model_change` / `set_thinking_level` / `set_session_name` for
//! **every** cause, so the ACP event pump can be the single emitter — and if the setters also
//! pushed, every ACP-originated change would emit two identical updates.
//!
//! So [`SessionConfigKnob::apply`] performs the mutation and returns the **applied** value; it
//! sends nothing. The same decision is taken once for all three notifications the question names —
//! `config_option_update`, `current_mode_update` and `session_info_update` (`ACP-285`, whose
//! `/name` arm therefore emits no `session_info_update` of its own; see [`crate::commands`]).
//!
//! **What it costs.** `ACP-072`'s and `ACP-073`'s pinned notification counts are counts *at the
//! setter*, and here they are zero. The client still sees exactly one `current_mode_update` and one
//! `config_option_update` per change — they arrive from the pump, one event later — but a test
//! written against the setter alone sees none, and a pump that fails to subscribe
//! `ThinkingLevelChanged` makes the dropdown stale with no error anywhere. That risk is real and it
//! is the price of not double-emitting; `ACP-077`'s verify (change the model through a non-ACP
//! route and assert the client is told) is the assertion that covers it, and it can only be written
//! at the pump.
//!
//! **The one exception, and why it is not a hole in the rule (`ACP-072`).** "The pump emits it" is
//! only true of a *change*: `AgentSession::set_thinking_level` early-returns without emitting
//! `ThinkingLevelChanged` when the clamped level equals the level already in effect, so a
//! `session/set_mode` that applies the current level produced no notification from anywhere — and
//! `SetSessionModeResponse` is `_meta`-only, so the response could not carry the correction either.
//! [`apply_mode`] therefore returns [`ModeApplication::echo`], populated **only** in that case
//! ([`pump_emits_mode_change`] is the whole test), for its caller to put on the wire. Every other
//! set still leaves `echo` empty and stays the pump's alone.
//!
//! `ACP-075`'s discipline survives unchanged and moves to [`config_options_update`]: the pushed set
//! is **re-derived** from the session, never patched from the request, so a clamped thinking level
//! and a provider-swapped model stay honest in the client's dropdown.
//!
//! # `ACP-065` / `ACP-Q14` — there is no `models` field, and no `_meta` shim either
//!
//! `NewSessionResponse` and `LoadSessionResponse` have exactly four fields each and neither has
//! `models`; upstream rode an extra key along on TypeScript's structural typing. **Decision: drop
//! it, do not shim it into `_meta`.** The `model` config option carries the same information in the
//! spec-sanctioned place, `SessionConfigKnob::advertise` is where it is minted, and an `_meta` key
//! no observed client reads is a second source of truth that can disagree with the first. The cost
//! is that a Zed build reading `response.models` gets `undefined` — the same result it gets from
//! any other Rust ACP agent, since the field is unrepresentable in the schema.

use std::collections::HashMap;

use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v1::{
    AgentAuthCapabilities, AgentCapabilities, AuthMethod, AuthMethodTerminal, McpCapabilities,
    Meta, PromptCapabilities, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigOptionValue, SessionConfigSelectOption, SessionDeleteCapabilities,
    SessionInfoUpdate, SessionListCapabilities, SessionMode, SessionModeId, SessionModeState,
    SessionUpdate, SetSessionConfigOptionResponse, SetSessionModeResponse,
};
use cyrup_core::ModelThinkingLevel;
use cyrup_session_svc::AgentSession;

use crate::connection::ClientView;
use crate::error::AcpFailure;

// ===================================================================================================
// The `initialize` advertisement — `ACP-052`, `ACP-053`, `ACP-054`, `ACP-294`
// ===================================================================================================

/// The stable id the client echoes back in `authenticate` (`ACP-010`).
///
/// Port of pi-acp v0.0.33 `auth.ts`'s `PI_SETUP_METHOD_ID = 'pi_terminal_login'`.
///
/// # [CYRUP-DELTA] — the three strings are rebranded deliberately
///
/// **What differs.** `pi_terminal_login` / `Launch pi in the terminal` / `Start pi in an
/// interactive terminal to configure API keys or login` become the `cyrup` spellings below. All
/// three are user-visible in Zed's Authenticate banner.
///
/// **What it costs.** A client that persisted the upstream id sees an unknown method id — which is
/// correct, because it would be launching a different program. There is no migration to write:
/// no cyrup ever advertised `pi_terminal_login`.
pub const CYRUP_SETUP_METHOD_ID: &str = "cyrup_terminal_login";
/// See [`CYRUP_SETUP_METHOD_ID`].
pub const CYRUP_SETUP_METHOD_NAME: &str = "Launch cyrup in the terminal";
/// See [`CYRUP_SETUP_METHOD_ID`].
pub const CYRUP_SETUP_METHOD_DESCRIPTION: &str =
    "Start cyrup in an interactive terminal to configure API keys or login";

/// The environment variable that opts `promptCapabilities.embeddedContext` in (`ACP-053`,
/// `ACP-294`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `process.env.PI_ACP_ENABLE_EMBEDDED_CONTEXT === 'true'`,
/// pinned by its `test/unit/pi-enable-embed-context-flag.test.ts`.
///
/// # [CYRUP-DELTA] — the name is declared, not read ad hoc, and it is declared here
///
/// **What differs.** `ACP-294` asks for the name to live in `cyrup_config::env_keys` beside every
/// other `CYRUP_*` variable. This crate cannot add a constant to `cyrup-config` (that file belongs
/// to another owner), so the declaration is here and the move is filed as an interface change. The
/// half that matters is already true: [`embedded_context_enabled`] takes the **value** as an
/// argument, so the predicate is pure and table-testable with no process environment.
///
/// **What it costs.** One grep for `CYRUP_ACP_ENABLE_EMBEDDED_CONTEXT` finds two sites instead of
/// one until the constant moves.
pub const EMBEDDED_CONTEXT_ENV: &str = "CYRUP_ACP_ENABLE_EMBEDDED_CONTEXT";

/// Is embedded context (`ContentBlock::Resource` in a prompt) advertised? (`ACP-053`)
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `=== 'true'` test. Deliberately **strict**: `"1"`, `"TRUE"`
/// and `"yes"` are all false, exactly as upstream's is, because the advertisement is a promise
/// about what the translator will do with a block it receives and a loose truthiness test turns a
/// typo into a silently-degraded prompt (`crate::commands::prompt_to_user_input`'s `Resource` arm
/// renders a marker, not the resource).
#[must_use]
pub fn embedded_context_enabled(raw: Option<&str>) -> bool {
    raw == Some("true")
}

/// The four advertised capability blocks (`ACP-052`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `initialize` `agentCapabilities` literal.
///
/// `SessionCapabilities.list` / `.delete` are `Option<T>` where `Some(T::new())` serializes to `{}`
/// and `None` **omits** the key — that `Option` *is* the advertisement, so passing `None` silently
/// un-advertises the session picker. `McpCapabilities::new()` already defaults `http`/`sse` to
/// `false`, which is upstream's literal.
///
/// `promptCapabilities.image: true` is a **static** claim. cyrup knows per-model vision
/// (`cyrup_provider::Modality::Image`, `Model::supports_image_input`), but `initialize` runs before
/// any session exists, so the static `true` is correct here and the per-model truth belongs to the
/// prompt path. `audio: false` is the other half of `ACP-280`: the translator has no audio arm that
/// does anything but emit a not-supported marker, and the two must agree.
///
/// # [CYRUP-DELTA] — two capabilities the Rust schema offers and pi-acp could not
///
/// **What differs.** 1.7.0 adds `SessionCapabilities.additional_directories` and
/// `AgentAuthCapabilities.logout`. **Neither is advertised**, because neither has an
/// implementation: `NewSessionRequest.additional_directories` reaches no cyrup seam, and there is
/// no `authenticate`-side logout. An advertised capability with no implementation is worse than an
/// absent one — the client offers the affordance and the call fails.
///
/// **What it costs.** A Zed user cannot add a second root to an ACP session, and must use the TUI's
/// `/logout`. Both are additive units, not port units.
///
/// # `ACP-Q9`, decided — the session capabilities are advertised, not gated
///
/// `loadSession: true` and `sessionCapabilities.{list,delete}` are promises about surfaces
/// `crate::sessions` owns. **Decision: advertise them**, which is what `ACP-052`'s verify asserts,
/// because they land in the same pass and a runtime gate would make the `initialize` response
/// depend on which modules happen to be finished — a value that changes between commits for reasons
/// no client can see. The cost is stated plainly: for as long as `session/load`, `session/list` or
/// `session/delete` answer an error, this advertisement is a lie the client discovers by calling.
#[must_use]
pub fn agent_capabilities(embedded_context: bool) -> AgentCapabilities {
    AgentCapabilities::new()
        .load_session(true)
        .prompt_capabilities(
            PromptCapabilities::new()
                .image(true)
                .audio(false)
                .embedded_context(embedded_context),
        )
        // Upstream's `{http:false, sse:false}` — pi has no MCP over ACP. cyrup has a real MCP tier,
        // but it is configured through cyrup's own settings rather than through
        // `NewSessionRequest.mcp_servers`, so the honest advertisement is still `false`.
        .mcp_capabilities(McpCapabilities::new())
        .session_capabilities(
            agent_client_protocol::schema::v1::SessionCapabilities::new()
                .list(SessionListCapabilities::new())
                .delete(SessionDeleteCapabilities::new()),
        )
        // Required, non-`Option`, and always serializes an `"auth":{}` key pi-acp never emits —
        // which is why `ACP-052`'s golden is a SUBSET assertion over the keys this port controls
        // rather than the byte-for-byte fixture the survey proposed.
        .auth(AgentAuthCapabilities::new())
}

/// The `authMethods` list and its conditional legacy `_meta` shim (`ACP-054`, `ACP-010`…`ACP-013`).
///
/// Port of pi-acp v0.0.33 `auth.ts`'s `getAuthMethods`, called from `agent.ts`'s `initialize` —
/// the one place `supportsTerminalAuthMeta` is computed rather than defaulted.
///
/// Exactly one method is ever advertised, and it is `AuthMethod::Terminal`: 2.1.0 models the
/// registry `type`/`args`/`env` triple first-class, so upstream's `method as AuthMethod` cast
/// disappears. `args` is [`crate::TERMINAL_LOGIN_ARG`], whose cross-test against the binary's
/// `acp_terminal_login_cmd::SUBCOMMAND` is what keeps "what the client is told to send" and "what
/// `main` recognises" in step.
///
/// # [CYRUP-DELTA] — the typed negotiation is preferred, and `env: {}` cannot be emitted
///
/// **What differs.** Two things. (1) 2.1.0 gives a *typed* negotiation,
/// `ClientCapabilities.auth.terminal`, which is exactly what Zed's `_meta["terminal-auth"]` probe
/// stood in for; the shim is emitted only when the typed flag is **false** and the legacy probe is
/// present, i.e. for an older Zed, rather than unconditionally. (2) `AuthMethodTerminal.env` is
/// `#[serde(skip_serializing_if = "HashMap::is_empty")]`, so upstream's explicit `env: {}` key
/// cannot be produced at all.
///
/// **What it costs.** (1) A client that sets *both* the typed capability and the legacy probe gets
/// only the typed shape — correct, and it makes the shim's lifetime observable: when no client
/// sends the probe, this branch is dead and can be deleted. (2) A registry validator that requires
/// a literal `env` key sees it absent; the field is optional in the schema, so this is a
/// schema-imposed divergence with no handler-side fix.
#[must_use]
pub fn auth_methods(view: &ClientView) -> Vec<AuthMethod> {
    let mut method = AuthMethodTerminal::new(CYRUP_SETUP_METHOD_ID, CYRUP_SETUP_METHOD_NAME)
        .description(CYRUP_SETUP_METHOD_DESCRIPTION.to_string())
        .args(vec![crate::TERMINAL_LOGIN_ARG.to_string()])
        .env(HashMap::new());

    // Upstream's `if (supportsTerminalAuthMeta)`, gated per `ACP-012` on the client's own STRICT
    // `=== true` probe — which `ClientView::from_request` has already evaluated — and additionally
    // suppressed when the typed capability made it redundant.
    if !view.auth_terminal && view.terminal_auth_meta {
        let mut launch = Meta::new();
        // `ACP-013` — name THIS executable. Upstream sniffed `argv[0]`/`argv[1]` for a
        // node-plus-`.js` pair and fell back to a bare `pi-acp` on PATH, a heuristic that produces
        // a spec naming an uninstalled binary under its own `npm run dev`. `current_exe()` is total
        // and is already this workspace's answer (`cyrup_intercom::transport::spawn`,
        // `crate::subcommands`, `cyrup_config::paths`), with the same `"cyrup"`-on-PATH fallback.
        let command = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "cyrup".to_string());
        launch.insert("command".into(), serde_json::Value::from(command));
        launch.insert(
            "args".into(),
            serde_json::Value::from(vec![crate::TERMINAL_LOGIN_ARG]),
        );
        launch.insert("label".into(), serde_json::Value::from("Launch cyrup"));

        let mut meta = Meta::new();
        meta.insert("terminal-auth".into(), serde_json::Value::Object(launch));
        method = method.meta(meta);
    }

    vec![AuthMethod::Terminal(method)]
}

/// The auth-method list attached to an `AUTH_REQUIRED` error payload (`ACP-016`, `ACP-022`).
///
/// Upstream calls `getAuthMethods()` with **no options** at its three error sites, which defaults
/// `supportsTerminalAuthMeta` to `true` and therefore ships the legacy `_meta["terminal-auth"]`
/// shim even to a client that never probed for it. `ACP-016` says to resolve that asymmetry
/// deliberately: this is the resolution, and it goes the other way.
///
/// # [CYRUP-DELTA] — the error payload carries the typed method only
///
/// **What differs.** [`auth_methods`] gates the `_meta` shim on the client's own strict probe; an
/// error is raised from sites that hold no [`ClientView`] (the turn-settle boundary, a command
/// arm), so it is built against [`ClientView::default`] — every capability off — and the shim is
/// therefore never attached to an error.
///
/// **What it costs.** A pre-2.1 Zed that reads the button out of `data.authMethods[0]._meta` sees
/// no button on the *error*; it still sees one in the `initialize` response, which is where it
/// looks first and which `ACP-012` does gate on its probe. Attaching a shim to a client that did
/// not ask for it is the worse half of the trade, because it is unconditional.
#[must_use]
pub fn auth_methods_for_error() -> Vec<AuthMethod> {
    auth_methods(&ClientView::default())
}

// ===================================================================================================
// The thinking ladder — one spelling, `ACP-062`
// ===================================================================================================

/// The wire spelling of a thinking level.
///
/// Replaces pi-acp v0.0.33 `agent.ts`'s `isThinkingLevel` six-string predicate **and** its separate
/// `available: ThinkingLevel[]` literal with one total match over cyrup's own ladder, which has a
/// seventh rung. Written out rather than routed through serde so the wire strings are visible at
/// the site and a `#[serde(rename)]` in `cyrup-core` cannot silently change the protocol; the
/// round-trip against `ModelThinkingLevel`'s own camelCase serialization is asserted by
/// `the_wire_spelling_matches_cyrups_own_serialization`.
#[must_use]
pub fn thinking_level_id(level: ModelThinkingLevel) -> &'static str {
    match level {
        ModelThinkingLevel::Off => "off",
        ModelThinkingLevel::Minimal => "minimal",
        ModelThinkingLevel::Low => "low",
        ModelThinkingLevel::Medium => "medium",
        ModelThinkingLevel::High => "high",
        ModelThinkingLevel::Xhigh => "xhigh",
        ModelThinkingLevel::Max => "max",
    }
}

/// The exact inverse of [`thinking_level_id`]. `None` for anything that is not a rung.
#[must_use]
pub fn thinking_level_from_id(id: &str) -> Option<ModelThinkingLevel> {
    [
        ModelThinkingLevel::Off,
        ModelThinkingLevel::Minimal,
        ModelThinkingLevel::Low,
        ModelThinkingLevel::Medium,
        ModelThinkingLevel::High,
        ModelThinkingLevel::Xhigh,
        ModelThinkingLevel::Max,
    ]
    .into_iter()
    .find(|l| thinking_level_id(*l) == id)
}

/// The human-readable mode name — upstream's `` `Thinking: ${id}` ``, byte-for-byte.
#[must_use]
pub fn thinking_level_name(level: ModelThinkingLevel) -> String {
    format!("Thinking: {}", thinking_level_id(level))
}

// ===================================================================================================
// The knob
// ===================================================================================================

/// A settable session config option, with advertise and parse unified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionConfigKnob {
    /// `id == "model"`. Holds the resolved model **value id**, not a parsed pair — see
    /// [`model_value_id`] for why.
    Model(String),
    /// `id == "thought_level"`, and the same value `session/set_mode` carries.
    Thinking(ModelThinkingLevel),
}

/// The **applied** value of a knob — what the session is on after the mutation, never what the
/// request asked for.
///
/// This is the `AppliedMode` newtype `ACP-072` names, generalised to both knobs because both clamp:
/// `AgentSession::set_thinking_level` returns the effective level after
/// `clamp_thinking_level(model, level)` (or `Off` for a modelless session), and
/// `AgentSession::set_model_resolved` returns the `ModelRef` it actually installed. The type exists
/// so the wrong value is unconstructible at the call site: [`SessionConfigKnob::apply`] is the only
/// constructor and it builds each variant **from the setter's return value**.
///
/// `ACP-072`'s verify is written against exactly this: a test asserting only that `{}` came back
/// passes the broken version that echoes the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppliedKnob {
    /// The model value id now selected.
    Model(String),
    /// The thinking level now in effect, **after** clamping.
    Thinking(ModelThinkingLevel),
}

/// The outcome of a `session/set_mode`: the level that took effect, and the notifications the
/// caller must send **itself** because [`config_pump`](crate::sessions) will not (`ACP-072`).
///
/// See [`apply_mode`] for the hole this closes and [`pump_emits_mode_change`] for the rule that
/// decides when `echo` is populated. `echo` is empty on every ordinary change — the pump owns
/// those (`ACP-Q20`) — so a caller that forwards it unconditionally cannot double-emit.
#[derive(Clone, Debug)]
pub struct ModeApplication {
    /// The thinking level now in effect, **after** clamping — never the request's id.
    pub applied: ModelThinkingLevel,
    /// `current_mode_update` + `config_option_update`, in that order, or empty.
    ///
    /// The caller addresses these to the request's `sessionId`:
    /// `SetSessionModeResponse` carries no session and `SessionScoped` for it yields `None`
    /// (`crate::lib`), so they cannot ride `HandlerOutcome::follow_up`.
    pub echo: Vec<SessionUpdate>,
}

/// What the advertiser needs to know about the session, so [`SessionConfigKnob::advertise`] stays
/// pure and `ACP-064`'s subset golden needs no runtime.
#[derive(Clone, Debug, Default)]
pub struct SessionConfigView {
    /// Every model the catalog offers, as `provider/id` value ids.
    pub models: Vec<String>,
    /// The display name for each entry of [`SessionConfigView::models`], positionally. Upstream's
    /// `` name: `${provider}/${name}` ``. Kept as a parallel `Vec` rather than a `Vec<(id, name)>`
    /// so `models` stays the plain catalog `model_from_value_id` looks up.
    pub model_names: Vec<String>,
    /// The current selection, if any. `ACP-063` asserts `currentValue` is a **member** of
    /// `options` — membership, not equality — and that an empty catalog emits **no** `model`
    /// option at all.
    pub current_model: Option<String>,
    /// The levels this model supports. `ACP-062`: a reasoning model yields 7 modes ending in
    /// `max`; a non-reasoning model yields exactly `[off]`.
    pub thinking_levels: Vec<ModelThinkingLevel>,
    /// The level the session is on.
    pub current_thinking: ModelThinkingLevel,
}

impl SessionConfigView {
    /// Read the view off a live session.
    ///
    /// Replaces pi-acp v0.0.33 `agent.ts`'s `getSessionConfiguration`, whose `Promise.all` over
    /// `getState()` and `getAvailableModels()` — each wrapped in `try { … } catch { return null }`
    /// — is four typed in-memory reads here. **The six swallowed probe failures go with it, and so
    /// does the class of bug they enabled**: `getModelState`'s `'default'` sentinel and
    /// `getThinkingState`'s silent fall back to `'medium'` were both reachable only through a
    /// swallowed failure.
    pub async fn of(session: &AgentSession) -> Self {
        let catalog = session.available_model_catalog();
        let models: Vec<String> = catalog
            .iter()
            .map(|m| model_value_id(m.provider.as_str(), m.id.as_str()))
            .collect();
        let model_names: Vec<String> = catalog
            .iter()
            .map(|m| format!("{}/{}", m.provider.as_str(), m.name))
            .collect();
        Self {
            models,
            model_names,
            current_model: session
                .model()
                .map(|r| model_value_id(r.provider.as_str(), r.model.as_str())),
            thinking_levels: session.available_thinking_levels(),
            current_thinking: session.thinking_level().await,
        }
    }

    /// The `currentValue` for the model selector, or `None` when there is nothing to advertise.
    ///
    /// # [CYRUP-DELTA] — the `'default'` sentinel is not ported, and the first-entry fallback is
    ///
    /// **What differs.** Upstream's ladder ends `availableModels[0]?.modelId ?? 'default'` at two
    /// sites. Both are **dead code**: `getModelState` returns `null` when the list is empty *and*
    /// there is no current model, so past that guard a falsy `currentModelId` implies a non-empty
    /// list and `availableModels[0].modelId` is a template literal that cannot be empty. cyrup's
    /// current selection is a real `Option<ModelRef>`, so the sentinel has nowhere to come from and
    /// is dropped; the first-entry fallback is kept, because it is the branch that actually runs.
    ///
    /// **What it costs.** A session with a non-empty catalog and no resolved model advertises the
    /// first catalog entry as `currentValue` — the client's dropdown shows a model as selected when
    /// nothing is. That is upstream's exact observable behaviour, and the alternative (omit the
    /// option) removes the only affordance such a session has for choosing a model, which is the
    /// one thing its user needs.
    #[must_use]
    fn model_current_value(&self) -> Option<&str> {
        self.current_model
            .as_deref()
            .filter(|id| self.models.iter().any(|m| m == id))
            .or_else(|| self.models.first().map(String::as_str))
    }
}

impl SessionConfigKnob {
    /// The `configId` for the model knob.
    pub const MODEL_ID: &'static str = "model";
    /// The `configId` for the thinking knob. Upstream's spelling, kept because it is the wire.
    pub const THINKING_ID: &'static str = "thought_level";

    /// The **only** place option ids and value ids are minted (`ACP-064`).
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `buildConfigOptions`. Two options, `model` then
    /// `thought_level`, in that order — upstream builds the thinking option first and `unshift`es
    /// the model one, which is the same list; here the order is written out because **the order is
    /// load-bearing and is not enforced by the type**.
    ///
    /// Every string is upstream's: `Model` / `Select the model for this session`, `Thinking` /
    /// `Set the reasoning effort for this session`, and the per-value name
    /// `` `Thinking: ${id}` ``.
    ///
    /// Two things the units settled and that must survive: the `model` option is **omitted
    /// entirely** for an empty catalog (`ACP-063`), and `SessionConfigSelectOption` is
    /// `#[skip_serializing_none]`, so `None` **omits** the key — there is no way to emit the
    /// explicit `description: null` upstream's fixture carries, which is why `ACP-064`'s golden is
    /// a **subset** assertion over the keys this port controls.
    ///
    /// # [CYRUP-DELTA] — the model list stays ungrouped, and the mode list is model-derived
    ///
    /// **What differs.** (1) 1.7.0 has `SessionConfigSelectOptions::Grouped`, so models could be
    /// grouped by provider instead of relying on the `provider/` string prefix; `Ungrouped` is kept
    /// for parity. (2) `ACP-062`(ii): upstream's mode list is a **fixed** six-element array that
    /// never varies with the model, while cyrup's comes from `available_thinking_levels()`, which
    /// returns `get_supported_thinking_levels(&model)` — so a non-reasoning model yields exactly
    /// `[off]` and the full seven-rung set appears only when no model is resolved.
    ///
    /// **What it costs.** (1) Nothing today; a client that wants provider headers has to split the
    /// string, exactly as it does against pi-acp. (2) `ACP-Q12`, decided: **a one-entry mode list
    /// is emitted rather than the whole surface being omitted.** Omitting `modes` when
    /// `supports_thinking()` is false would make the presence of a protocol field depend on the
    /// selected model, so the client would have to re-read it on every `config_option_update`; a
    /// one-entry dropdown is inert but honest. The cost is a `Thinking: off` dropdown with nothing
    /// to pick on a non-reasoning model.
    #[must_use]
    pub fn advertise(view: &SessionConfigView) -> Vec<SessionConfigOption> {
        let mut out = Vec::with_capacity(2);

        if let Some(current) = view.model_current_value() {
            let options: Vec<SessionConfigSelectOption> = view
                .models
                .iter()
                .enumerate()
                .map(|(i, id)| {
                    // Positional pairing with `model_names`; a missing name falls back to the value
                    // id, which is the only honest answer and is unreachable through
                    // `SessionConfigView::of`.
                    let name = view.model_names.get(i).unwrap_or(id);
                    SessionConfigSelectOption::new(id.clone(), name.clone())
                })
                .collect();
            out.push(
                SessionConfigOption::select(Self::MODEL_ID, "Model", current.to_string(), options)
                    .description("Select the model for this session".to_string())
                    .category(SessionConfigOptionCategory::Model),
            );
        }

        let modes: Vec<SessionConfigSelectOption> = view
            .thinking_levels
            .iter()
            .map(|l| SessionConfigSelectOption::new(thinking_level_id(*l), thinking_level_name(*l)))
            .collect();
        out.push(
            SessionConfigOption::select(
                Self::THINKING_ID,
                "Thinking",
                thinking_level_id(view.current_thinking),
                modes,
            )
            .description("Set the reasoning effort for this session".to_string())
            .category(SessionConfigOptionCategory::ThoughtLevel),
        );

        out
    }

    /// The ACP mode list and the current mode (`ACP-062`).
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `getThinkingState`. **The mode list IS the thinking
    /// ladder**, so it is derived from the same [`SessionConfigView`] the `thought_level` selector
    /// is, and `currentModeId` is the session's actual level — never upstream's hardcoded
    /// `'medium'` fallback, which was reachable only through a swallowed probe failure.
    ///
    /// `SessionMode.description` is `Option<String>` under `#[skip_serializing_none]`, so `None`
    /// **omits** the key where pi-acp emits an explicit `description: null`. There is no way to
    /// emit the null; this is a forced divergence, not a choice, and it is why `ACP-062`'s golden
    /// is a subset assertion.
    #[must_use]
    pub fn mode_state(view: &SessionConfigView) -> SessionModeState {
        SessionModeState::new(
            thinking_level_id(view.current_thinking),
            view.thinking_levels
                .iter()
                .map(|l| SessionMode::new(thinking_level_id(*l), thinking_level_name(*l)))
                .collect(),
        )
    }

    /// The exact inverse of [`SessionConfigKnob::advertise`], over the same id space (`ACP-073`).
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `setSessionConfigOption` routing.
    ///
    /// # Errors
    ///
    /// `Unknown config option: <id>` at `-32602` for an unrecognised `config_id` — byte-for-byte
    /// upstream's. `Expected string value for config option: <id>` for the `Boolean` arm, which is
    /// upstream's `typeof params.value !== 'string'` message: `SessionConfigOptionValue` is already
    /// a domain enum (`Boolean{value}` / untagged `ValueId{value}`), so the `typeof` test becomes a
    /// `match` that **forces** a decision about `Boolean` instead of silently rejecting it. The
    /// decision is to reject it with upstream's own sentence, because this port advertises no
    /// `SessionConfigKind::Boolean` option (`ACP-Q13`) and a boolean for `model` is a client bug.
    /// `Unknown thinking level: <value>` for a `thought_level` value that is not a rung.
    pub fn parse(config_id: &str, value: &SessionConfigOptionValue) -> Result<Self, AcpFailure> {
        let value_id = match value {
            SessionConfigOptionValue::ValueId { value } => value.to_string(),
            // `#[non_exhaustive]` plus the `Boolean` arm: anything that is not a value id gets
            // upstream's message, keyed by the option the client aimed at.
            _ => {
                return Err(AcpFailure::InvalidParams {
                    message: format!("Expected string value for config option: {config_id}"),
                });
            }
        };

        match config_id {
            Self::MODEL_ID => Ok(Self::Model(value_id)),
            Self::THINKING_ID => thinking_level_from_id(&value_id).map(Self::Thinking).ok_or(
                AcpFailure::InvalidParams {
                    message: format!("Unknown thinking level: {value_id}"),
                },
            ),
            other => Err(AcpFailure::InvalidParams {
                message: format!("Unknown config option: {other}"),
            }),
        }
    }

    /// `session/set_mode`'s entry onto the same validator — one validator, two entry points
    /// (`ACP-062`, `ACP-072`).
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `setSessionMode`, whose `isThinkingLevel` guard raises a
    /// **different** message from the `thought_level` path's. Both strings are kept, because both
    /// are user-visible and a client can reach either.
    ///
    /// # Errors
    ///
    /// `Unknown modeId: <mode>` at `-32602` for a mode id that is not a thinking level.
    pub fn parse_mode(mode_id: &SessionModeId) -> Result<Self, AcpFailure> {
        let id = mode_id.to_string();
        thinking_level_from_id(&id)
            .map(Self::Thinking)
            .ok_or(AcpFailure::InvalidParams {
                message: format!("Unknown modeId: {id}"),
            })
    }

    /// Apply the knob to a live session and report what was **actually** applied.
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `setSessionModel` and its `proc.setThinkingLevel(mode)`
    /// call, with two mechanisms replaced outright.
    ///
    /// **Model.** Upstream splits the requested id on the first `/`, rejoins the rest, and — when
    /// there is no `/` — re-queries `getAvailableModels` and takes the **first** `String(m?.id) ===
    /// modelId` match, so two providers exposing the same model id silently select the wrong one.
    /// Here the value id is looked up in the catalog it was minted from
    /// ([`model_value_id`] is the only minter), and the resolved `Model` goes to
    /// `AgentSession::set_model_resolved`, which runs the `has_configured_auth` precheck and
    /// `install_owning_provider`.
    ///
    /// # [CYRUP-DELTA] — the lookup is exact where `set_model(pattern)` is lenient
    ///
    /// **What differs.** `AgentSession::set_model(pattern)` resolves through
    /// `cyrup_config::ModelResolver::match_reference`, whose third step is a **substring** match —
    /// so a `value` of `"4"` would select a model where pi-acp raised `-32602`. This path does an
    /// exact membership check against `available_model_catalog()` first, which is what `ACP-073`
    /// prescribes when parity on the rejection path matters. It also closes the `/`-ambiguity:
    /// `openrouter/anthropic/claude-x` is one catalog entry, not a `split('/')` puzzle.
    ///
    /// **What it costs.** A client cannot use cyrup's friendlier partial patterns over ACP — but it
    /// has no reason to, because it is picking from the list this agent just advertised.
    ///
    /// **Thinking.** `set_thinking_level` **returns the effective level after clamping** and that
    /// return value — never the request's id — becomes the [`AppliedKnob`]. It also republishes
    /// `CYRUP_REASONING_LEVEL` for the next bash child and appends a `thinking_level_change` entry,
    /// so it must not be bypassed. `ACP-Q19`, decided: **accept and clamp**, do not reject an
    /// unsupported-but-well-formed level. The mode list is model-derived
    /// ([`SessionConfigKnob::mode_state`]), so an unsupported level is never advertised in the
    /// first place; clamping is `set_thinking_level`'s own contract, and the re-derived
    /// `config_option_update` ([`config_options_update`]) reports the clamped value back, so the
    /// client's dropdown corrects itself rather than showing a level the agent silently refused.
    ///
    /// This function sends **nothing** — see this module's `ACP-Q20` delta.
    ///
    /// # Errors
    ///
    /// `Unknown modelId: <id>` at `-32602` for a value id that is not in the catalog (upstream's
    /// string), and whatever [`AcpFailure::classify`](crate::AcpFailure::classify) makes of a
    /// failed set — which is how a `NoConfiguredAuth` on a model swap becomes `-32000` with
    /// `data.authMethods`, a distinction pi-acp had no way to draw.
    pub async fn apply(&self, session: &AgentSession) -> Result<AppliedKnob, AcpFailure> {
        match self {
            Self::Model(value_id) => {
                let model = session
                    .available_model_catalog()
                    .into_iter()
                    .find(|m| model_value_id(m.provider.as_str(), m.id.as_str()) == *value_id)
                    .ok_or(AcpFailure::InvalidParams {
                        message: format!("Unknown modelId: {value_id}"),
                    })?;
                let applied = session
                    .set_model_resolved(model)
                    .await
                    .map_err(|e| AcpFailure::classify(&e))?;
                Ok(AppliedKnob::Model(model_value_id(
                    applied.provider.as_str(),
                    applied.model.as_str(),
                )))
            }
            Self::Thinking(level) => {
                let applied = session
                    .set_thinking_level(*level)
                    .await
                    .map_err(|e| AcpFailure::classify(&e))?;
                Ok(AppliedKnob::Thinking(applied))
            }
        }
    }
}

/// Re-derive the whole option set and wrap it as the `config_option_update` payload (`ACP-075`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `emitConfigOptionsUpdate`, which is called with **no**
/// cached state precisely so the returned `currentValue` reflects what the agent actually applied
/// rather than what was requested. **That re-derive-rather-than-patch discipline is the whole
/// point** and is what keeps a clamped thinking level and a provider-swapped model honest in the
/// client's dropdown.
///
/// Upstream both sent the notification and returned the list; here it only builds the value, and
/// the caller decides. That split is what makes `ACP-Q20`'s single-emitter rule expressible: the
/// setter's response needs the list (`SetSessionConfigOptionResponse.configOptions`) while the
/// notification is the pump's to send.
pub async fn config_options(session: &AgentSession) -> Vec<SessionConfigOption> {
    SessionConfigKnob::advertise(&SessionConfigView::of(session).await)
}

/// The mode list and the config options from **one** view read (`ACP-062`, `ACP-064`).
///
/// What `session/new` and `session/load` put on their responses:
/// `NewSessionResponse::new(id).modes(modes).config_options(options)`. Upstream's
/// `getSessionConfiguration` returns the same pair (plus the `models` block `ACP-065` drops), and
/// it exists for the same reason: reading the session twice can straddle a model change and
/// advertise a mode list that does not match the `thought_level` selector beside it.
pub async fn session_surface(
    session: &AgentSession,
) -> (SessionModeState, Vec<SessionConfigOption>) {
    let view = SessionConfigView::of(session).await;
    (
        SessionConfigKnob::mode_state(&view),
        SessionConfigKnob::advertise(&view),
    )
}

/// Whether [`config_pump`](crate::sessions) will emit this mode change on its own (`ACP-072`,
/// `ACP-Q20`).
///
/// **This is the whole of `ACP-072`, as a predicate.** `ACP-Q20` made the pump the single emitter
/// of `current_mode_update` / `config_option_update`, and the pump is driven by
/// `AgentSessionEvent::ThinkingLevelChanged` — which `AgentSession::set_thinking_level` emits
/// **only on a real change**: `if effective == previous { return Ok(effective); }`
/// (`crates/cyrup-session-svc/src/session/thinking.rs`, pi's own `if (isChanging)` guard at
/// `agent-session.ts:1688-1697`). So a `session/set_mode` whose applied level equals the level
/// already in effect emits nothing anywhere, and `SetSessionModeResponse` is `_meta`-only — it has
/// no field that could carry the correction the way `SetSessionConfigOptionResponse.configOptions`
/// does for the `thought_level` arm.
///
/// That is reachable with a spec-compliant client, and it is the case `ACP-072` tables: a
/// non-reasoning model advertises the one-rung `[off]` ladder and the session sits at `off`; a
/// client restoring a persisted selection sends `{modeId:"medium"}`; `parse_mode` accepts any rung
/// by design (`ACP-Q19` — accept and clamp, never reject a well-formed level); `set_thinking_level`
/// clamps `Medium` to `Off`; `Off == previous`; nothing is emitted and the response is `{}`. The
/// client's mode selector then reads `Thinking: medium` for the rest of the session while the agent
/// is off. The same shape is the *normal* case on a modelless session, which `ACP-Q7` keeps alive:
/// `available_thinking_levels()` returns the full seven-rung ladder with no model resolved while
/// `set_thinking_level` clamps every rung to `Off`.
///
/// Upstream is wrong in the other direction and self-corrects: `setSessionMode`
/// (`src/acp/agent.ts:1151-1162` @v0.0.33) emits `current_mode_update` with the **requested** id
/// unconditionally and then re-derives the option set, so the dropdown lands on the truth one
/// notification later. This port emits the **applied** level and only when the pump will not, which
/// is upstream's guarantee without upstream's wrong intermediate value.
#[must_use]
pub fn pump_emits_mode_change(previous: ModelThinkingLevel, applied: ModelThinkingLevel) -> bool {
    previous != applied
}

/// The notifications a no-op `session/set_mode` must send in the pump's place (`ACP-072`).
///
/// `current_mode_update` **first** — it is what moves the client's mode selector — then the
/// re-derived `config_option_update` for a client that renders the `thought_level` dropdown
/// instead. Exactly the pair, in exactly the order, that
/// `crate::sessions::session_updates_for`'s `ThinkingLevelChanged` arm sends, so the two emitters
/// are indistinguishable on the wire and a client cannot tell which route a change came by.
///
/// Takes the applied level and an already-derived option set rather than a session, so the order
/// and the contents are assertable without a runtime.
#[must_use]
pub fn mode_echo(
    applied: ModelThinkingLevel,
    options: Vec<SessionConfigOption>,
) -> Vec<SessionUpdate> {
    vec![current_mode_update(applied), config_option_update(options)]
}

/// `session/set_mode`'s whole body below the `Unknown sessionId` gate (`ACP-072`, `ACP-079`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `setSessionMode`. The caller resolves the session against
/// `req.session_id` and answers
/// [`crate::SessionManager::unknown_session`](crate::sessions::SessionManager::unknown_session)
/// on a miss; everything after that is here.
///
/// # [CYRUP-DELTA] — `ACP-Q20` stands, with the one exception `ACP-072` needs
///
/// **What differs.** `ACP-Q20` says the setters do not notify and the pump does, and that is still
/// true for every change: [`ModeApplication::echo`] is **empty** whenever
/// [`pump_emits_mode_change`] holds, so an ordinary `session/set_mode` still produces exactly one
/// `current_mode_update` and one `config_option_update`, both from the pump. The exception is the
/// case the pump cannot see at all — a set whose applied level equals the current one, which emits
/// no `ThinkingLevelChanged` and therefore no pump output. There the setter carries the echo,
/// because the alternative is silence and a client left rendering a mode the agent is not in.
///
/// **What it costs.** There are now two emitters for one notification pair rather than one, and
/// the invariant that keeps them from doubling is [`pump_emits_mode_change`] alone — a single
/// `!=`, tested directly, and the reason `echo` is a field the caller forwards blindly rather than
/// a decision the caller makes.
///
/// The `previous` level is read **before** the set for the same reason `AppliedKnob` exists: it is
/// the only moment at which the no-op is observable, and re-reading afterwards would compare a
/// value against itself.
///
/// # The caller's obligation — [`ModeApplication::echo`] must be forwarded
///
/// This function still sends nothing; it cannot, because it holds an `AgentSession` and not the
/// connection. `crate::sessions::SessionManager::set_mode` is the only caller and it owns the
/// wire: `self.wire().notify(&req.session_id, update)` for each element of `echo`, before or after
/// `HandlerOutcome::plain(response)` — the pair describes a state the session is already in, so
/// their position relative to the response does not matter the way `ACP-068`'s does. They cannot
/// ride `HandlerOutcome::follow_up`: `SessionScoped for SetSessionModeResponse` yields `None`
/// (`crate::lib`) because the response names no session, so a follow-up on it is unaddressable and
/// is silently discarded. **An `echo` that is dropped puts `ACP-072` straight back**, and the
/// assertion that would catch it is a second `session/set_mode` for the level already applied,
/// asserting exactly one `current_mode_update` carrying that level.
///
/// This must not be awaited inline in the dispatch handler (`ACP-079`): `set_thinking_level`
/// appends to the session file and then dispatches `HostEvent::ThinkingLevelSelect` into guest
/// extension code, and `ConnectionTo`'s own doc is unambiguous that the connection cannot process
/// new messages while a handler runs — an inline await here blocks `session/cancel`.
/// `connection.rs`'s `set_mode` arm already `cx.spawn`s.
///
/// # Errors
///
/// `Unknown modeId: <mode>` at `-32602`, or whatever a failed set classifies as.
pub async fn apply_mode(
    session: &AgentSession,
    mode_id: &SessionModeId,
) -> Result<(ModeApplication, SetSessionModeResponse), AcpFailure> {
    let knob = SessionConfigKnob::parse_mode(mode_id)?;
    // `ACP-072` — before the set. After it, `previous` and `applied` are the same read.
    let previous = session.thinking_level().await;
    match knob.apply(session).await? {
        AppliedKnob::Thinking(applied) => {
            let echo = if pump_emits_mode_change(previous, applied) {
                Vec::new()
            } else {
                // `ACP-075` — re-derived from the session, never patched from the request.
                mode_echo(applied, config_options(session).await)
            };
            Ok((
                ModeApplication { applied, echo },
                SetSessionModeResponse::new(),
            ))
        }
        // Unreachable: `parse_mode` constructs only `Thinking`, and `apply` preserves the variant.
        // Answered rather than panicked, per the crate's no-panic rule.
        AppliedKnob::Model(id) => Err(AcpFailure::Internal {
            message: format!("set_mode resolved to the model knob ({id}), which cannot happen"),
        }),
    }
}

/// `session/set_config_option`'s whole body below the `Unknown sessionId` gate (`ACP-073`,
/// `ACP-075`, `ACP-079`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `setSessionConfigOption`, minus its notifications
/// (`ACP-Q20`). The returned `configOptions` are **re-derived** after the set (`ACP-075`), so a
/// clamped thinking level or a provider-swapped model comes back as what the session is on, never
/// as what was requested.
///
/// # Errors
///
/// `Unknown config option: <id>`, `Expected string value for config option: <id>`,
/// `Unknown thinking level: <v>` or `Unknown modelId: <v>`, all at `-32602`; or an auth failure on
/// a model swap at `-32000`.
pub async fn apply_config_option(
    session: &AgentSession,
    config_id: &str,
    value: &SessionConfigOptionValue,
) -> Result<(AppliedKnob, SetSessionConfigOptionResponse), AcpFailure> {
    let applied = SessionConfigKnob::parse(config_id, value)?
        .apply(session)
        .await?;
    let options = config_options(session).await;
    Ok((applied, SetSessionConfigOptionResponse::new(options)))
}

/// [`config_options`] as the `session/update` the pump pushes (`ACP-075`, `ACP-077`).
pub async fn config_options_update(session: &AgentSession) -> SessionUpdate {
    config_option_update(config_options(session).await)
}

/// An already-derived option set as its `session/update` (`ACP-075`, `ACP-077`).
///
/// Split from [`config_options_update`] so the notification can be built in a pure test — and so
/// [`mode_echo`] cannot accidentally re-read the session a second time and race the value it is
/// echoing. The list must still come from [`config_options`]: `ACP-075`'s re-derive-rather-than-
/// patch discipline is about where the list is read, not about who wraps it.
#[must_use]
pub fn config_option_update(options: Vec<SessionConfigOption>) -> SessionUpdate {
    SessionUpdate::ConfigOptionUpdate(agent_client_protocol::schema::v1::ConfigOptionUpdate::new(
        options,
    ))
}

/// The `current_mode_update` for a level that is already applied (`ACP-072`, `ACP-077`).
///
/// Takes an [`AppliedKnob`]-sourced level rather than a `SessionModeId` so the notification cannot
/// be built from a request value: that is the whole reason [`AppliedKnob`] exists.
#[must_use]
pub fn current_mode_update(applied: ModelThinkingLevel) -> SessionUpdate {
    SessionUpdate::CurrentModeUpdate(agent_client_protocol::schema::v1::CurrentModeUpdate::new(
        thinking_level_id(applied),
    ))
}

/// The `session_info_update` for a rename (`ACP-285`, `ACP-Q20`).
///
/// `SessionInfoUpdate.title` is `MaybeUndefined<String>`, **not** `Option`: it has three states and
/// `Null` **clears** the title. cyrup's `AgentSessionEvent::SessionInfoChanged { name:
/// Option<String> }` has two, so the mapping is stated once, here, and nowhere else:
/// `Some(name) => Value(name)`, and `None` — a session whose name was cleared — maps to `Null`,
/// which is the one place `Null` is correct because it is the message that means "cleared".
///
/// Lives beside the config updates because `ACP-Q20` is one decision over all three notifications
/// and they must be emitted from one place.
#[must_use]
pub fn session_info_update(name: Option<String>, updated_at: String) -> SessionUpdate {
    let title = match name {
        Some(name) => MaybeUndefined::Value(name),
        None => MaybeUndefined::Null,
    };
    SessionUpdate::SessionInfoUpdate(
        SessionInfoUpdate::new()
            .title(title)
            .updated_at(MaybeUndefined::Value(updated_at)),
    )
}

/// The advertised value id for a model. **Shared by both directions**, so a model value id is never
/// built ad hoc.
///
/// Upstream builds it by concatenation in `getModelState` (`` `${provider}/${id}` ``) and tears it
/// apart with `split('/')` in `setSessionModel`, with a fallback that re-queries
/// `getAvailableModels` and takes the **first** `String(m?.id) === modelId` match — so with two
/// providers exposing the same model id it silently selects the wrong provider's model.
///
/// **ADR-0028 F5's recommendation, taken:** the value id is **opaque and looked up rather than
/// parsed**. [`model_from_value_id`] is a catalog lookup, and
/// [`SessionConfigKnob::apply`]'s model arm re-derives the `Model` the same way, so a provider or
/// model id containing `/` — `openrouter/anthropic/claude-sonnet-4-5` is a real shape — cannot
/// split wrong because it is never split. That is why [`SessionConfigKnob::Model`] holds the value
/// id as a `String` rather than a parsed `ModelRef`: parsing it back is the thing that goes wrong.
///
/// No newtype: `cyrup_core::ModelRef` plus `cyrup_config::ModelResolver` (which
/// `AgentSession::set_model(pattern)` already uses) own that parsing, and wrapping the advertised
/// string would duplicate a parser cyrup owns (ADR-0028 §5).
#[must_use]
pub fn model_value_id(provider: &str, model: &str) -> String {
    format!("{provider}/{model}")
}

/// The inverse of [`model_value_id`], resolved by **lookup in the advertised catalog** rather than
/// by splitting.
///
/// Lookup, not `split('/')`: that is what closes the ambiguous-`/` and
/// same-model-id-two-providers holes in one move, and it fails gracefully against a model that has
/// disappeared between the advertise and the set — which nothing here prevents, and which
/// [`SessionConfigKnob::apply`] turns into `Unknown modelId: <id>` rather than a panic.
#[must_use]
pub fn model_from_value_id(id: &str, catalog: &[String]) -> Option<String> {
    catalog.iter().find(|c| c.as_str() == id).cloned()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::ProtocolVersion;
    use agent_client_protocol::schema::v1::{
        AuthCapabilities, ClientCapabilities, InitializeRequest, SessionConfigKind,
        SessionConfigSelectOptions, SessionConfigValueId,
    };

    /// A view with a two-model catalog and a reasoning model's full ladder.
    fn view() -> SessionConfigView {
        SessionConfigView {
            models: vec![
                model_value_id("anthropic", "claude-sonnet-4-5"),
                model_value_id("openrouter", "anthropic/claude-sonnet-4-5"),
            ],
            model_names: vec![
                "anthropic/Claude Sonnet 4.5".into(),
                "openrouter/Claude Sonnet 4.5".into(),
            ],
            current_model: Some(model_value_id("anthropic", "claude-sonnet-4-5")),
            thinking_levels: vec![
                ModelThinkingLevel::Off,
                ModelThinkingLevel::Minimal,
                ModelThinkingLevel::Low,
                ModelThinkingLevel::Medium,
                ModelThinkingLevel::High,
                ModelThinkingLevel::Xhigh,
                ModelThinkingLevel::Max,
            ],
            current_thinking: ModelThinkingLevel::Medium,
        }
    }

    fn select_of(
        option: &SessionConfigOption,
    ) -> &agent_client_protocol::schema::v1::SessionConfigSelect {
        match &option.kind {
            SessionConfigKind::Select(s) => s,
            other => panic!("expected a select, got {other:?}"),
        }
    }

    fn values_of(option: &SessionConfigOption) -> Vec<String> {
        match &select_of(option).options {
            SessionConfigSelectOptions::Ungrouped(v) => {
                v.iter().map(|o| o.value.to_string()).collect()
            }
            other => panic!("ACP-063 keeps the model list Ungrouped; got {other:?}"),
        }
    }

    // ---- ACP-052 / ACP-053 / ACP-054 -----------------------------------------------------------

    /// ACP-052 — the SUBSET assertion the unit prescribes, over the keys this port controls. The
    /// byte-for-byte fixture cannot pass: `AgentCapabilities.auth` is required and always emits
    /// `"auth":{}`, which pi-acp never sent.
    #[test]
    fn the_four_capability_blocks_are_advertised_as_the_unit_pins_them() {
        let json = serde_json::to_value(agent_capabilities(false)).unwrap();
        assert_eq!(json["loadSession"], serde_json::json!(true));
        assert_eq!(json["promptCapabilities"]["image"], serde_json::json!(true));
        assert_eq!(
            json["promptCapabilities"]["audio"],
            serde_json::json!(false)
        );
        assert_eq!(
            json["mcpCapabilities"],
            serde_json::json!({"http": false, "sse": false})
        );
        // `Some(T::new())` serializes to `{}` and `None` OMITS the key — that Option IS the
        // advertisement, so this assertion is what stops a refactor un-advertising the picker.
        assert_eq!(json["sessionCapabilities"]["list"], serde_json::json!({}));
        assert_eq!(json["sessionCapabilities"]["delete"], serde_json::json!({}));
        // Not advertised without an implementation.
        assert!(json["sessionCapabilities"]["additionalDirectories"].is_null());
        assert_eq!(json["auth"], serde_json::json!({}));
    }

    /// ACP-053 — the pure predicate, taking the value as an argument. Only `Some("true")`.
    #[test]
    fn embedded_context_is_a_strict_opt_in() {
        assert!(embedded_context_enabled(Some("true")));
        for raw in [
            None,
            Some(""),
            Some("1"),
            Some("TRUE"),
            Some("yes"),
            Some("false"),
        ] {
            assert!(!embedded_context_enabled(raw), "{raw:?} must not opt in");
        }
        // And it reaches the advertisement, both ways.
        let on = serde_json::to_value(agent_capabilities(true)).unwrap();
        assert_eq!(
            on["promptCapabilities"]["embeddedContext"],
            serde_json::json!(true)
        );
        let off = serde_json::to_value(agent_capabilities(false)).unwrap();
        assert_eq!(
            off["promptCapabilities"]["embeddedContext"],
            serde_json::json!(false)
        );
        assert_eq!(EMBEDDED_CONTEXT_ENV, "CYRUP_ACP_ENABLE_EMBEDDED_CONTEXT");
    }

    /// ACP-010 / ACP-011 / ACP-054 — exactly one method, the three strings byte-for-byte, and the
    /// typed terminal shape with `args` equal to the advertised token.
    #[test]
    fn one_terminal_auth_method_with_the_registry_shape() {
        let view = ClientView::default();
        let methods = auth_methods(&view);
        assert_eq!(methods.len(), 1, "ACP-010: exactly one method, always");
        assert_eq!(methods[0].id().to_string(), "cyrup_terminal_login");
        assert_eq!(methods[0].name(), "Launch cyrup in the terminal");

        let json = serde_json::to_value(&methods[0]).unwrap();
        assert_eq!(json["type"], serde_json::json!("terminal"));
        assert_eq!(json["args"], serde_json::json!(["--terminal-login"]));
        assert_eq!(
            json["description"],
            serde_json::json!(
                "Start cyrup in an interactive terminal to configure API keys or login"
            )
        );
        // The advertised token IS `crate::TERMINAL_LOGIN_ARG`, whose cross-check against the
        // binary's own `SUBCOMMAND` lives in `crates/cyrup`.
        assert_eq!(
            json["args"][0],
            serde_json::json!(crate::TERMINAL_LOGIN_ARG)
        );
        // No shim for a client that asked for neither.
        assert!(
            json.get("_meta").is_none(),
            "ACP-012: no `_meta` when the probe is absent"
        );
    }

    /// ACP-012 / ACP-054 — the legacy `_meta` shim appears only for an older Zed: the probe true
    /// AND the typed capability false. A client advertising the typed capability gets the typed
    /// shape alone.
    #[test]
    fn the_legacy_terminal_auth_shim_is_gated_on_the_probe_and_suppressed_by_the_typed_flag() {
        let probed = |auth_terminal: bool, meta: bool| {
            let view = ClientView {
                protocol_version: ProtocolVersion::V1,
                auth_terminal,
                terminal_auth_meta: meta,
                terminal: false,
                elicitation: false,
            };
            serde_json::to_value(&auth_methods(&view)[0]).unwrap()
        };

        let legacy = probed(false, true);
        assert_eq!(
            legacy["_meta"]["terminal-auth"]["label"],
            serde_json::json!("Launch cyrup")
        );
        assert_eq!(
            legacy["_meta"]["terminal-auth"]["args"],
            serde_json::json!(["--terminal-login"])
        );
        assert!(
            legacy["_meta"]["terminal-auth"]["command"]
                .as_str()
                .is_some_and(|c| !c.is_empty()),
            "ACP-013: the launch spec names an executable"
        );

        // The typed negotiation makes the shim redundant, and the shim is then absent entirely.
        assert!(probed(true, true).get("_meta").is_none());
        assert!(probed(true, false).get("_meta").is_none());
        assert!(probed(false, false).get("_meta").is_none());
    }

    /// The gate is fed by the client's own STRICT `=== true` probe, end to end from an
    /// `InitializeRequest`. A truthy non-boolean must not light the shim.
    #[test]
    fn a_truthy_non_boolean_probe_does_not_produce_the_shim() {
        let mut meta = Meta::new();
        meta.insert("terminal-auth".into(), serde_json::json!(1));
        let req = InitializeRequest::new(ProtocolVersion::V1)
            .client_capabilities(ClientCapabilities::new().meta(meta));
        let view = ClientView::from_request(&req);
        let json = serde_json::to_value(&auth_methods(&view)[0]).unwrap();
        assert!(json.get("_meta").is_none());

        // And the typed path, which is the one a current Zed is expected to take.
        let typed = InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
            ClientCapabilities::new().auth(AuthCapabilities::new().terminal(true)),
        );
        let json =
            serde_json::to_value(&auth_methods(&ClientView::from_request(&typed))[0]).unwrap();
        assert_eq!(json["type"], serde_json::json!("terminal"));
        assert!(json.get("_meta").is_none());
    }

    // ---- ACP-062 -------------------------------------------------------------------------------

    /// The one spelling of a level, and its inverse — the pair that replaces `isThinkingLevel` plus
    /// the separate `available` literal.
    #[test]
    fn the_wire_spelling_matches_cyrups_own_serialization() {
        for level in [
            ModelThinkingLevel::Off,
            ModelThinkingLevel::Minimal,
            ModelThinkingLevel::Low,
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::High,
            ModelThinkingLevel::Xhigh,
            ModelThinkingLevel::Max,
        ] {
            let id = thinking_level_id(level);
            assert_eq!(thinking_level_from_id(id), Some(level));
            // The hand-written match must agree with `ModelThinkingLevel`'s camelCase serde, which
            // is what `cyrup_session_svc::builder::thinking_level_to_str` persists — if a rename
            // ever splits them, an ACP client and the session file would disagree about `xhigh`.
            assert_eq!(serde_json::to_value(level).unwrap(), serde_json::json!(id));
        }
        assert_eq!(thinking_level_from_id("MEDIUM"), None);
        assert_eq!(thinking_level_from_id(""), None);
        assert_eq!(
            thinking_level_name(ModelThinkingLevel::Max),
            "Thinking: max"
        );
    }

    /// ACP-062 — a reasoning model yields seven modes ending in `max`, each named
    /// `Thinking: <id>`, and `currentModeId` is the session's level, not a hardcoded `medium`.
    #[test]
    fn a_reasoning_model_yields_the_whole_ladder_and_the_real_current_level() {
        let state = SessionConfigKnob::mode_state(&view());
        let ids: Vec<String> = state
            .available_modes
            .iter()
            .map(|m| m.id.to_string())
            .collect();
        assert_eq!(
            ids,
            ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(state.available_modes[6].name, "Thinking: max");
        assert_eq!(state.current_mode_id.to_string(), "medium");

        // `description: null` is unrepresentable under `#[skip_serializing_none]`; the key is
        // ABSENT rather than null. Recorded as an assertion so the forced divergence is visible.
        let json = serde_json::to_value(&state.available_modes[0]).unwrap();
        assert!(json.get("description").is_none());
    }

    /// ACP-062(ii) — the list is MODEL-DERIVED, which upstream's fixed six-element array never was:
    /// a non-reasoning model yields exactly `[off]`, and `ACP-Q12`'s decision is that the surface
    /// is still emitted rather than omitted.
    #[test]
    fn a_non_reasoning_model_yields_exactly_off_and_the_surface_still_exists() {
        let mut v = view();
        v.thinking_levels = vec![ModelThinkingLevel::Off];
        v.current_thinking = ModelThinkingLevel::Off;

        let state = SessionConfigKnob::mode_state(&v);
        assert_eq!(state.available_modes.len(), 1);
        assert_eq!(state.current_mode_id.to_string(), "off");

        let options = SessionConfigKnob::advertise(&v);
        let thinking = options
            .iter()
            .find(|o| o.id.to_string() == SessionConfigKnob::THINKING_ID)
            .expect("ACP-Q12: the thought_level option is emitted even with one rung");
        assert_eq!(values_of(thinking), ["off"]);
    }

    // ---- ACP-063 / ACP-064 ---------------------------------------------------------------------

    /// ACP-064 — the subset golden: two options, `model` then `thought_level`, with their
    /// categories, names, descriptions and `currentValue`.
    #[test]
    fn the_two_options_are_in_order_with_upstreams_strings() {
        let options = SessionConfigKnob::advertise(&view());
        assert_eq!(options.len(), 2);

        let json = serde_json::to_value(&options).unwrap();
        assert_eq!(json[0]["id"], serde_json::json!("model"));
        assert_eq!(json[0]["category"], serde_json::json!("model"));
        assert_eq!(json[0]["name"], serde_json::json!("Model"));
        assert_eq!(
            json[0]["description"],
            serde_json::json!("Select the model for this session")
        );
        assert_eq!(json[0]["type"], serde_json::json!("select"));
        assert_eq!(
            json[0]["currentValue"],
            serde_json::json!("anthropic/claude-sonnet-4-5")
        );

        assert_eq!(json[1]["id"], serde_json::json!("thought_level"));
        assert_eq!(json[1]["category"], serde_json::json!("thought_level"));
        assert_eq!(json[1]["name"], serde_json::json!("Thinking"));
        assert_eq!(
            json[1]["description"],
            serde_json::json!("Set the reasoning effort for this session")
        );
        assert_eq!(json[1]["currentValue"], serde_json::json!("medium"));
        // `SessionConfigKind::Select` is `#[serde(flatten)]`ed under `type`, so `currentValue` and
        // `options` land at the same nesting depth as upstream's literal.
        assert!(json[1]["options"].is_array());
    }

    /// ACP-063 — `currentValue` is a MEMBER of `options`, never merely equal to what was asked for;
    /// and an empty catalog emits no `model` option at all.
    #[test]
    fn the_model_option_is_membership_checked_and_omitted_for_an_empty_catalog() {
        let options = SessionConfigKnob::advertise(&view());
        let model = &options[0];
        let current = select_of(model).current_value.to_string();
        assert!(
            values_of(model).contains(&current),
            "ACP-063: currentValue must be a member of options"
        );

        let mut empty = view();
        empty.models.clear();
        empty.model_names.clear();
        empty.current_model = None;
        let options = SessionConfigKnob::advertise(&empty);
        assert_eq!(options.len(), 1);
        assert!(
            !options
                .iter()
                .any(|o| o.id.to_string() == SessionConfigKnob::MODEL_ID),
            "ACP-063: no `model` option is emitted for an empty catalog"
        );
    }

    /// ACP-063 — a stale or absent current selection falls back to the first catalog entry, and
    /// the `'default'` sentinel is never produced. The delta on `model_current_value` is what this
    /// pins.
    #[test]
    fn a_missing_or_stale_current_model_falls_back_to_the_first_entry_never_to_default() {
        for current in [None, Some("gone/model".to_string())] {
            let mut v = view();
            v.current_model = current.clone();
            let options = SessionConfigKnob::advertise(&v);
            let value = select_of(&options[0]).current_value.to_string();
            assert_eq!(
                value, "anthropic/claude-sonnet-4-5",
                "current = {current:?}"
            );
            assert_ne!(
                value, "default",
                "the `'default'` sentinel must not be ported"
            );
        }
    }

    /// ACP-063 — the value id round-trips by LOOKUP, so an id containing `/` cannot split wrong.
    #[test]
    fn the_model_value_id_round_trips_by_lookup() {
        let catalog = vec![
            model_value_id("anthropic", "claude-sonnet-4-5"),
            model_value_id("openrouter", "anthropic/claude-sonnet-4-5"),
        ];
        assert_eq!(catalog[0], "anthropic/claude-sonnet-4-5");
        assert_eq!(catalog[1], "openrouter/anthropic/claude-sonnet-4-5");
        // Both resolve, and to DIFFERENT entries — `split('/')` cannot tell them apart.
        assert_eq!(
            model_from_value_id(&catalog[1], &catalog).as_deref(),
            Some(catalog[1].as_str())
        );
        assert_eq!(model_from_value_id("gone/model", &catalog), None);
    }

    /// The advertised value ids and the accepted ones are the same set, both directions — the F5
    /// invariant, asserted rather than asserted-by-comment.
    #[test]
    fn every_advertised_value_parses_and_every_parsed_value_is_advertised() {
        let v = view();
        for option in SessionConfigKnob::advertise(&v) {
            let id = option.id.to_string();
            for value in values_of(&option) {
                let parsed = SessionConfigKnob::parse(
                    &id,
                    &SessionConfigOptionValue::ValueId {
                        value: SessionConfigValueId::new(value.clone()),
                    },
                )
                .unwrap_or_else(|e| panic!("advertised {id}={value} is rejected: {e:?}"));
                match parsed {
                    SessionConfigKnob::Model(got) => assert_eq!(got, value),
                    SessionConfigKnob::Thinking(level) => {
                        assert_eq!(thinking_level_id(level), value);
                    }
                }
            }
        }
    }

    // ---- ACP-072 / ACP-073 ---------------------------------------------------------------------

    /// ACP-073 — the unrecognised-`configId` message is byte-for-byte upstream's, and the two known
    /// ids do NOT take the catch-all.
    #[test]
    fn an_unknown_config_id_is_invalid_params_with_the_exact_message() {
        let value = SessionConfigOptionValue::ValueId {
            value: SessionConfigValueId::new("x"),
        };
        assert_eq!(
            SessionConfigKnob::parse("nope", &value).unwrap_err(),
            AcpFailure::InvalidParams {
                message: "Unknown config option: nope".into()
            }
        );
        // `model` accepts any value id and defers the membership check to `apply`, which is where
        // the catalog is; `thought_level` rejects a non-rung with its own message.
        assert_eq!(
            SessionConfigKnob::parse(SessionConfigKnob::MODEL_ID, &value).unwrap(),
            SessionConfigKnob::Model("x".into())
        );
        assert_eq!(
            SessionConfigKnob::parse(SessionConfigKnob::THINKING_ID, &value).unwrap_err(),
            AcpFailure::InvalidParams {
                message: "Unknown thinking level: x".into()
            }
        );
    }

    /// ACP-073 — the `Boolean` arm the Rust enum forces a decision about gets upstream's own
    /// `typeof` message, keyed by the option the client aimed at.
    #[test]
    fn a_boolean_value_is_refused_with_upstreams_typeof_message() {
        let value = SessionConfigOptionValue::boolean(true);
        for id in [
            SessionConfigKnob::MODEL_ID,
            SessionConfigKnob::THINKING_ID,
            "nope",
        ] {
            assert_eq!(
                SessionConfigKnob::parse(id, &value).unwrap_err(),
                AcpFailure::InvalidParams {
                    message: format!("Expected string value for config option: {id}")
                }
            );
        }
    }

    /// ACP-072 — `set_mode` is the same validator under another name, with upstream's OTHER
    /// message. Both are user-visible and a client can reach either.
    #[test]
    fn set_mode_parses_the_same_ladder_with_its_own_error_string() {
        assert_eq!(
            SessionConfigKnob::parse_mode(&SessionModeId::new("xhigh")).unwrap(),
            SessionConfigKnob::Thinking(ModelThinkingLevel::Xhigh)
        );
        assert_eq!(
            SessionConfigKnob::parse_mode(&SessionModeId::new("max")).unwrap(),
            SessionConfigKnob::Thinking(ModelThinkingLevel::Max),
            "cyrup's seventh rung is accepted, because it is advertised"
        );
        assert_eq!(
            SessionConfigKnob::parse_mode(&SessionModeId::new("plan")).unwrap_err(),
            AcpFailure::InvalidParams {
                message: "Unknown modeId: plan".into()
            }
        );
        let wire: agent_client_protocol::Error =
            SessionConfigKnob::parse_mode(&SessionModeId::new("plan"))
                .unwrap_err()
                .into();
        assert_eq!(i32::from(wire.code), -32602);
        assert_eq!(wire.message, "Unknown modeId: plan");
    }

    /// ACP-072 / ACP-Q19 — the notification is built from the APPLIED level, and the type is what
    /// makes the echo-the-request bug unwritable: `current_mode_update` takes a
    /// `ModelThinkingLevel`, which only `AppliedKnob` and the ladder can produce, not a
    /// `SessionModeId` off the request.
    #[test]
    fn the_mode_update_carries_the_applied_level_not_the_requested_one() {
        // The shape a clamp produces: the client asked for `xhigh`, the model gave `medium`.
        let applied = AppliedKnob::Thinking(ModelThinkingLevel::Medium);
        let AppliedKnob::Thinking(level) = applied else {
            panic!("constructed as Thinking");
        };
        let update = current_mode_update(level);
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(
            json["sessionUpdate"],
            serde_json::json!("current_mode_update")
        );
        assert_eq!(json["currentModeId"], serde_json::json!("medium"));
        assert_ne!(
            json["currentModeId"],
            serde_json::json!("xhigh"),
            "ACP-072: a test asserting only that `{{}}` came back passes the broken version"
        );
    }

    /// ACP-072 — the no-op set the pump cannot see, which is the whole of the gap.
    ///
    /// `AgentSession::set_thinking_level` early-returns before `fanout_emit` when the clamped level
    /// equals the level already in effect (`crates/cyrup-session-svc/src/session/thinking.rs`), so
    /// `config_pump` — the single emitter under `ACP-Q20` — never runs for such a set. Before this
    /// predicate existed the setter also emitted nothing, and `SetSessionModeResponse` is
    /// `_meta`-only, so `session/set_mode` produced NO notification at all and the client's mode
    /// selector kept the value it optimistically rendered.
    ///
    /// The tabled case is the first assertion: a non-reasoning model advertises `[off]`, the
    /// session is at `off`, a client sends `{modeId:"medium"}`, `parse_mode` accepts it
    /// (`ACP-Q19` — accept and clamp), `set_thinking_level` clamps it to `off`, and `off == off`.
    #[test]
    fn a_set_mode_that_changes_nothing_is_the_case_the_pump_cannot_emit() {
        // `[off]`-only model: every rung clamps to `off`, so the applied level equals the current.
        assert!(
            !pump_emits_mode_change(ModelThinkingLevel::Off, ModelThinkingLevel::Off),
            "ACP-072: `medium` clamped to `off` on a session already at `off` emits no \
             `ThinkingLevelChanged`, so the setter must carry the echo itself"
        );
        // Re-picking the level already shown, on a reasoning model — harmless, but equally silent.
        assert!(!pump_emits_mode_change(
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::Medium
        ));
        // And every real change stays the pump's, so `ACP-Q20` is not weakened into double-emission.
        for (previous, applied) in [
            (ModelThinkingLevel::Off, ModelThinkingLevel::Medium),
            (ModelThinkingLevel::High, ModelThinkingLevel::Off),
            (ModelThinkingLevel::Xhigh, ModelThinkingLevel::Max),
        ] {
            assert!(
                pump_emits_mode_change(previous, applied),
                "ACP-Q20: {previous:?} -> {applied:?} is the pump's to emit, not the setter's"
            );
        }
    }

    /// ACP-072 — the echo is the pump's own pair, in the pump's own order, carrying the APPLIED
    /// level.
    ///
    /// `crate::sessions::session_updates_for`'s `ThinkingLevelChanged` arm sends
    /// `current_mode_update` then `config_option_update`. A client must not be able to tell which
    /// emitter a change came from, so this pins the same two, in the same order — and pins that the
    /// mode id is the clamped level, never the `modeId` the client sent.
    #[test]
    fn the_no_op_echo_is_indistinguishable_from_the_pumps_own_pair() {
        // The client asked for `xhigh`; the session clamped to `off` and was already there.
        let echo = mode_echo(
            ModelThinkingLevel::Off,
            SessionConfigKnob::advertise(&view()),
        );
        assert_eq!(echo.len(), 2, "exactly the pump's pair: {echo:?}");

        let first = serde_json::to_value(&echo[0]).unwrap();
        assert_eq!(
            first["sessionUpdate"],
            serde_json::json!("current_mode_update"),
            "ACP-072: `current_mode_update` FIRST — it is what moves the client's mode selector"
        );
        assert_eq!(first["currentModeId"], serde_json::json!("off"));
        assert_ne!(
            first["currentModeId"],
            serde_json::json!("xhigh"),
            "ACP-072: the applied level, never the requested modeId"
        );

        let second = serde_json::to_value(&echo[1]).unwrap();
        assert_eq!(
            second["sessionUpdate"],
            serde_json::json!("config_option_update"),
            "ACP-072: and the re-derived option set second, for a client that renders the \
             `thought_level` dropdown instead of the mode selector"
        );
        assert_eq!(second["configOptions"].as_array().map(Vec::len), Some(2));
    }

    /// ACP-285 / ACP-Q20 — `title` is `MaybeUndefined`, and the two-state-to-three-state mapping is
    /// stated once. `Some` must never become `Null`, which CLEARS the title in Zed.
    #[test]
    fn a_rename_sends_a_value_and_only_a_clear_sends_null() {
        let json = serde_json::to_value(session_info_update(
            Some("my session".into()),
            "2026-09-05T00:00:00Z".into(),
        ))
        .unwrap();
        assert_eq!(
            json["sessionUpdate"],
            serde_json::json!("session_info_update")
        );
        assert_eq!(json["title"], serde_json::json!("my session"));
        assert_eq!(json["updatedAt"], serde_json::json!("2026-09-05T00:00:00Z"));

        let cleared =
            serde_json::to_value(session_info_update(None, "2026-09-05T00:00:00Z".into())).unwrap();
        assert!(cleared["title"].is_null());
    }

    /// ACP-075 — the update wraps the re-derived set, and the payload key is the one the schema
    /// names. The re-derivation itself is asserted against a live session in `cyrup-it`.
    #[test]
    fn the_config_update_carries_the_whole_option_set() {
        let update = SessionUpdate::ConfigOptionUpdate(
            agent_client_protocol::schema::v1::ConfigOptionUpdate::new(
                SessionConfigKnob::advertise(&view()),
            ),
        );
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(
            json["sessionUpdate"],
            serde_json::json!("config_option_update")
        );
        assert_eq!(json["configOptions"].as_array().map(Vec::len), Some(2));
    }

    /// The seventh rung exists on cyrup's side and not on pi's — the divergence F5 is about.
    #[test]
    fn cyrup_has_a_thinking_level_pi_does_not() {
        // If this ever stops compiling, the advertise/parse pair has to be re-checked against the
        // new ladder rather than the six strings `isThinkingLevel` lists.
        let max = ModelThinkingLevel::Max;
        assert_ne!(max, ModelThinkingLevel::Xhigh);
        // And it is reachable through BOTH entry points, which is the bug the enum prevents.
        assert!(SessionConfigKnob::parse_mode(&SessionModeId::new("max")).is_ok());
        assert!(
            SessionConfigKnob::parse(
                SessionConfigKnob::THINKING_ID,
                &SessionConfigOptionValue::ValueId {
                    value: SessionConfigValueId::new("max")
                }
            )
            .is_ok()
        );
    }
}
