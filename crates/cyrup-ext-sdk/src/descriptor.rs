//! Serializable descriptors authored by a guest extension (arch-08 §3.5/§3.6). These mirror the
//! host-side registration records; `parameters` stays JSON-Schema (Pi-interop, R-ARCH-EXT-008).
//! camelCase per arch-00 §4.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Per-tool execution mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecMode {
    Parallel,
    Sequential,
}

/// How a tool's execution row is framed (Pi `ToolDefinition.renderShell`, types.ts:448-449).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderShell {
    #[default]
    Default,
    #[serde(rename = "self")]
    SelfRendered,
}

/// pi `ConstrainedSamplingConfig` — `packages/ai/src/types.ts:469-477` @v0.83.0. Declared on a
/// tool with [`ToolDescriptor::constrained_sampling`]; resolved provider-side by
/// `packages/ai/src/api/constrained-sampling.ts`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstrainedSamplingConfig {
    /// Ask the provider for a strict JSON-schema-constrained tool call.
    JsonSchema { strict: StrictSampling },
    /// Ask the provider for a Lark/regex grammar-constrained tool call. The tool's parameter
    /// schema must be an object with EXACTLY ONE required string property — upstream's
    /// `inferGrammarInputProperty` (`constrained-sampling.ts:69-88` @v0.83.0) rejects anything
    /// else and the host reports it as a provider error.
    Grammar { variants: GrammarVariants },
}

/// pi's `strict: "prefer" | "require"` (`packages/ai/src/types.ts:472` @v0.83.0). `Require` makes
/// an unsupporting model an ERROR ("requires JSON-schema constrained sampling, but strict tools
/// are unsupported"); `Prefer` silently falls back to unconstrained sampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StrictSampling {
    Prefer,
    Require,
}

/// pi `GrammarVariants = Partial<Record<GrammarFormat, string>>` where
/// `GrammarFormat = "openai_lark" | "openai_regex"` (`packages/ai/src/types.ts:459-461`
/// @v0.83.0). Lark wins when both are present, matching
/// `resolveGrammarConstrainedSampling`'s `hasLarkDefinition ? … : …`. The keys are snake_case
/// upstream, so this struct deliberately carries no `rename_all`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GrammarVariants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_lark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_regex: Option<String>,
}

/// pi `ToolDefinition.constrainedSampling?: false | ConstrainedSamplingConfig`
/// (`extensions/types.ts:463` @v0.83.0): "Optional provider-side constrained sampling request for
/// this tool. Set false to explicitly disable it, equivalent to leaving it undefined."
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConstrainedSampling {
    Config(ConstrainedSamplingConfig),
    /// pi's `false` — serialized as the bare JSON literal, not an object.
    Disabled(bool),
}

/// What a guest sends to register a tool (R-08-012/013; Pi `ToolDefinition`, types.ts:435-482).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub name: String,
    pub label: String,
    pub description: String,
    /// JSON-Schema for the parameters.
    pub parameters: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<ExecMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_snippet: Option<String>,
    #[serde(default)]
    pub prompt_guidelines: Vec<String>,
    #[serde(default)]
    pub has_renderer: bool,
    /// `renderShell` (types.ts:449): runtime-drawn shell vs. self-rendered.
    #[serde(default)]
    pub render_shell: RenderShell,
    /// Whether the tool supplies a `prepareArguments` shim (types.ts:452); the host coerces args
    /// before validation when set.
    #[serde(default)]
    pub prepare_arguments: bool,
    /// `constrainedSampling` (`extensions/types.ts:463` @v0.83.0) — opt in to provider-side
    /// grammar- or strict-JSON-schema-constrained sampling for this tool. `None` = the omitted
    /// field, which upstream is indistinguishable from
    /// [`ConstrainedSampling::Disabled`]`(false)`. Set with
    /// [`ToolDescriptor::constrained_sampling`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constrained_sampling: Option<ConstrainedSampling>,
}

impl ToolDescriptor {
    /// Minimal builder: name + JSON-Schema parameters.
    pub fn new(name: impl Into<String>, parameters: Value) -> Self {
        let name = name.into();
        Self {
            label: name.clone(),
            name,
            description: String::new(),
            parameters,
            execution_mode: None,
            prompt_snippet: None,
            prompt_guidelines: Vec::new(),
            has_renderer: false,
            render_shell: RenderShell::Default,
            prepare_arguments: false,
            constrained_sampling: None,
        }
    }

    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = l.into();
        self
    }

    pub fn prompt_snippet(mut self, s: impl Into<String>) -> Self {
        self.prompt_snippet = Some(s.into());
        self
    }

    pub fn execution_mode(mut self, m: ExecMode) -> Self {
        self.execution_mode = Some(m);
        self
    }

    /// Declare that this tool renders its own call/result rows (Pi `ToolDefinition.renderCall`/
    /// `renderResult`, extensions/types.ts:472-481). The host records the OWNER of the renderer for
    /// this tool NAME and routes rendering back through the guest's `render-call`/`render-result`
    /// exports (keyed by the tool name); register the matching renderer with
    /// [`crate::ExtensionApi::register_message_renderer`] under the SAME name.
    pub fn has_renderer(mut self, yes: bool) -> Self {
        self.has_renderer = yes;
        self
    }

    /// Opt this tool in to provider-side constrained sampling (pi
    /// `ToolDefinition.constrainedSampling`, `extensions/types.ts:463` @v0.83.0). The host copies
    /// the declaration onto the runtime tool — upstream's `wrapToolDefinition`
    /// (`core/tools/tool-definition-wrapper.ts:14`) — and the provider adapter resolves it per
    /// model. A model that does not support the requested mode ignores it, except for
    /// [`StrictSampling::Require`], which upstream turns into an error.
    pub fn constrained_sampling(mut self, cs: ConstrainedSamplingConfig) -> Self {
        self.constrained_sampling = Some(ConstrainedSampling::Config(cs));
        self
    }
}

/// What a guest sends to register a command (R-08-016; Pi types.ts:1105-1111).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDescriptor {
    pub description: String,
    /// Static completions; dynamic completions use the command handler's `getArgumentCompletions`.
    #[serde(default)]
    pub completions: Vec<String>,
}

impl CommandDescriptor {
    pub fn new(description: impl Into<String>) -> Self {
        Self { description: description.into(), completions: Vec::new() }
    }
}

/// Options for [`crate::Ctx::exec`] — 1:1 with Pi's REAL `ExecOptions` (the type `Ctx.exec` actually
/// uses, `extensions/types.ts:47,1277` imports it straight from `core/exec.ts:11-18`):
/// `{signal?, timeout?, cwd?}`. Deliberately has NO `env` field — Pi's `execCommand`
/// (`exec.ts:41-45`) never passes an `env` override to `spawn()` at all, so the child only ever
/// inherits the host's own ambient environment (Node's default when `env` is omitted). Adding one
/// here would be new ambient authority (arbitrary env injection for a spawned process) with no Pi
/// equivalent — do not re-add it without a real Pi ground-truth citation.
///
/// `signal_id` is the WASM-boundary adaptation of Pi's `options.signal: AbortSignal` (`exec.ts:65-
/// 72`): Pi extensions run in-process and can hand `execCommand` a live `AbortSignal` object
/// directly; a WASM guest cannot pass an object reference across the component boundary, so it
/// instead references a signal it already registered by ID via [`crate::Ctx::abort_signal`] (the
/// SAME id namespace [`DialogOptions::signal_id`] uses). Since the guest is wasm-suspended for the
/// whole duration of a host `exec` call, only Pi's "already aborted before the call" branch
/// (`exec.ts:66-68`) is reachable — the host checks `signal_id` once, at call time, and starts the
/// process pre-cancelled if it was already aborted (`cyrup-ext/src/host/live.rs`'s `exec::Host::run`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_id: Option<String>,
}

impl ExecOptions {
    /// Set the child's working directory (builder-style).
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
    /// Kill the child if it's still running after `ms` (builder-style).
    pub fn timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms);
        self
    }
    /// Bind an already-registered programmatic abort signal (builder-style; Pi `options.signal`,
    /// `exec.ts:65-72`): if `id` was aborted via [`crate::Ctx::abort_signal`] before this call, the
    /// exec starts pre-cancelled instead of running at all.
    pub fn signal_id(mut self, id: impl Into<String>) -> Self {
        self.signal_id = Some(id.into());
        self
    }
}

/// UI dialog options for `confirm`/`input`/`select` (pi `ExtensionUIDialogOptions`,
/// `extensions/types.ts:95-100` @v0.83.0): a live-countdown timeout and/or a programmatic-dismiss
/// `signal_id` (the host maps the id to an abort token). Both optional; the default `{}` is an
/// indefinite dialog.
///
/// **EXT-048 — the wire key is `timeout`, not `timeoutMs`.** Upstream is
/// `interface ExtensionUIDialogOptions { signal?: AbortSignal; timeout?: number; }` with
/// `timeout?: number` at `:100`, documented "Timeout in milliseconds. Dialog auto-dismisses with
/// live countdown display". `git grep -n timeoutMs v0.83.0 -- packages/coding-agent/src` returns
/// only unrelated startup-ui / http-dispatcher / package-manager hits, so there is no wire variant
/// spelled `timeoutMs` anywhere upstream — and the in-tree comments that cited `types.ts:89` for
/// it were citing a blank line. `timeout` is now the canonical name; `timeoutMs` is accepted as an
/// alias so bags cyrup's own SDK already wrote keep deserializing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogOptions {
    #[serde(rename = "timeout", alias = "timeoutMs", default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_id: Option<String>,
}

impl DialogOptions {
    /// A dialog that auto-dismisses after `ms` with a live countdown (Pi `{timeout}`).
    pub fn timeout(ms: u64) -> Self {
        Self { timeout_ms: Some(ms), signal_id: None }
    }
    /// A dialog dismissible via the named programmatic signal (Pi `{signal}`).
    pub fn signal(id: impl Into<String>) -> Self {
        Self { timeout_ms: None, signal_id: Some(id.into()) }
    }
}

/// CLI flag spec (Pi `registerFlag`, types.ts:1199-1209).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlagSpec {
    /// `"boolean"` | `"string"`.
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default)]
    pub description: String,
}

// --- Provider registration (Pi `ProviderConfig`, types.ts:1363-1421; R-08-019) ---

/// A custom LLM provider configuration registered via `register_provider`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The wire API family (e.g. `"anthropic"`, `"openai"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// API key, supporting Pi's env interpolation / `!command` resolution (resolved host-side).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Pi `authHeader?: boolean` (types.ts:1386): when true the host adds `Authorization: Bearer
    /// <resolved key>`. (sdk gap #27 — was a stringly-typed `Option<String>`.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<bool>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub models: Vec<ProviderModelConfig>,
    /// OAuth metadata (`{name}`): a static marker that this provider authenticates via OAuth. The
    /// dynamic `login`/`refreshToken`/`getApiKey`/`modifyModels` callbacks live guest-side in
    /// [`crate::ProviderHandlers`] and are invoked across the `provider-*` exports (sdk gap #1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<Value>,
    /// Whether the guest supplied a custom `streamSimple` handler (drives the host to invoke the
    /// `provider-stream-simple` export rather than a built-in API stream).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_stream_simple: bool,
}

/// One long-context pricing tier for a registered model (Pi `ModelCostTier`, ai/types.ts:750-753).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostTier {
    #[serde(default)]
    pub input_tokens_above: u64,
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

/// Per-token cost for a registered model (Pi `ProviderModelConfig.cost`, types.ts:1493 — rates plus
/// optional request-wide input pricing tiers).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<ModelCostTier>>,
}

/// Per-model config inside a [`ProviderConfig`] (Pi `ProviderModelConfig`, types.ts:1404-1429). Now
/// the FULL Pi shape (sdk gap #26): the prior 4-field struct dropped a model's cost/reasoning/
/// modality/api/baseUrl/thinking-level map/headers/compat. Open-shaped fields (`thinkingLevelMap`,
/// `compat`) cross as `serde_json::Value`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Whether the model supports extended thinking (Pi `reasoning`).
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<Value>,
    /// Supported input modalities (Pi `input: ("text"|"image")[]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<String>,
    #[serde(default)]
    pub cost: ModelCost,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Max output tokens (Pi `maxTokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<Value>,
}

// --- Command-tier option bags (Pi `ExtensionCommandContext`, types.ts:339-390; sdk gap #5) ---

/// Where a [`crate::CommandCtx::fork_with`] inserts (Pi `fork({position})`, types.ts:355).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForkPosition {
    /// Fork the session BEFORE the entry (the entry is excluded).
    Before,
    /// Fork AT the entry (the entry is included).
    #[default]
    At,
}

/// Options for `fork` (Pi types.ts:355). `with_session` requests the host re-bind the new session
/// and invoke the guest `with-session` re-binding callback after the switch (the `ReplacedSessionContext`
/// flow, types.ts:382).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<ForkPosition>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub with_session: bool,
}

/// Options for `navigateTree` (Pi types.ts:362): summarize the skipped span, with custom/replacement
/// instructions and an optional label.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigateOptions {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub summarize: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace_instructions: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Options for `compact` (Pi `CompactOptions`, types.ts:296-300): extra guidance handed to the
/// compaction summarizer.
///
/// Pi's bag also carries `onComplete(result)` / `onError(error)` callbacks. Those are function
/// VALUES and cannot cross the component boundary, so they have no field here: a guest that needs
/// the completion signal subscribes to the `session_compact` event
/// ([`crate::events::SessionCompact`], Pi's own `SessionCompactEvent`), which carries the produced
/// compaction entry. Pi's `compact()` is fire-and-forget on both sides — it "triggers compaction
/// without awaiting completion" — so the call itself returns as soon as the host has queued it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
}

/// Options for `newSession` (Pi types.ts:346): an optional parent session + the `with_session`
/// re-binding request (the `setup`/`withSession` closures map to the host re-binding flow).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub with_session: bool,
}

/// Options for `switchSession` (Pi types.ts:368): the `with_session` re-binding request.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchSessionOptions {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub with_session: bool,
}
