//! Serializable descriptors authored by a guest extension (arch-08 §3.5/§3.6). These mirror the
//! host-side registration records; `parameters` stays JSON-Schema (Pi-interop, R-ARCH-EXT-008).
//! camelCase per arch-00 §4.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Per-tool execution mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecMode {
    /// The tool may run alongside the other calls in its batch.
    Parallel,
    /// The tool must not run alongside others — and ONE such tool serializes the WHOLE batch: the
    /// runner takes the sequential path if `calls.iter().any(|c| … == ExecMode::Sequential)`
    /// (`cyrup-agent/src/agent/run/tools/mod.rs:57-60`).
    Sequential,
}

/// How a tool's execution row is framed (Pi `ToolDefinition.renderShell`, types.ts:448-449).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderShell {
    /// The runtime draws the standard tool shell. Upstream's `"default"`, and its OMITTED field —
    /// which is why the guest lowers this variant to `none` rather than to the string
    /// (`src/guest.rs`'s `lower_tool_descriptor`, EXT-024).
    #[default]
    Default,
    /// pi's `"self"`: the tool renders its own framing instead of the standard shell.
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
    JsonSchema {
        /// Whether an unsupporting model is an error or a silent fallback — see
        /// [`StrictSampling`].
        strict: StrictSampling,
    },
    /// Ask the provider for a Lark/regex grammar-constrained tool call. The tool's parameter
    /// schema must be an object with EXACTLY ONE required string property — upstream's
    /// `inferGrammarInputProperty` (`constrained-sampling.ts:69-88` @v0.83.0) rejects anything
    /// else and the host reports it as a provider error.
    Grammar {
        /// The grammar to constrain with, one entry per upstream format — see
        /// [`GrammarVariants`].
        variants: GrammarVariants,
    },
}

/// pi's `strict: "prefer" | "require"` (`packages/ai/src/types.ts:472` @v0.83.0). `Require` makes
/// an unsupporting model an ERROR ("requires JSON-schema constrained sampling, but strict tools
/// are unsupported"); `Prefer` silently falls back to unconstrained sampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StrictSampling {
    /// pi's `"prefer"`: a model that does not support strict tools silently falls back to
    /// unconstrained sampling.
    Prefer,
    /// pi's `"require"`: a model that does not support strict tools is an ERROR — see the type doc
    /// for the message.
    Require,
}

/// pi `GrammarVariants = Partial<Record<GrammarFormat, string>>` where
/// `GrammarFormat = "openai_lark" | "openai_regex"` (`packages/ai/src/types.ts:459-461`
/// @v0.83.0). Lark wins when both are present, matching
/// `resolveGrammarConstrainedSampling`'s `hasLarkDefinition ? … : …`. The keys are snake_case
/// upstream, so this struct deliberately carries no `rename_all`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GrammarVariants {
    /// The `openai_lark` grammar. Wins over [`Self::openai_regex`] when both are present (see the
    /// type doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_lark: Option<String>,
    /// The `openai_regex` grammar, used when [`Self::openai_lark`] is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_regex: Option<String>,
}

/// pi `ToolDefinition.constrainedSampling?: false | ConstrainedSamplingConfig`
/// (`extensions/types.ts:463` @v0.83.0): "Optional provider-side constrained sampling request for
/// this tool. Set false to explicitly disable it, equivalent to leaving it undefined."
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConstrainedSampling {
    /// A real request. Serialized as [`ConstrainedSamplingConfig`]'s own `{"type": …}` object,
    /// because this enum is `untagged`.
    Config(ConstrainedSamplingConfig),
    /// pi's `false` — serialized as the bare JSON literal, not an object.
    Disabled(bool),
}

/// How the model reaches a tool (pi `ToolExposure`, `extensions/types.ts:509` @v1.0.1). Set with
/// [`ToolDescriptor::exposure`].
///
/// The wire spelling is pi's string literal ([`Self::as_str`]); the host parses it back and
/// refuses a spelling that is not one of these five.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolExposure {
    /// Declared to the model while active, and callable while active. The default, and what an
    /// omitted field means.
    #[default]
    Direct,
    /// Declared to the model while active, never callable by other tools. For orchestrating or
    /// interactive tools.
    ModelOnly,
    /// Callable whenever registered. Not declared to the model unless something activates it by
    /// name; codemode tools list it in their description.
    Codemode,
    /// Like [`Self::Codemode`], but codemode tools do not list it; tool search can find it.
    Deferred,
    /// Registered but unreachable. Activating it has no effect.
    Hidden,
}

impl ToolExposure {
    /// The wire spelling, which is pi's string literal: `"direct"`, `"model-only"`, `"codemode"`,
    /// `"deferred"` or `"hidden"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::ModelOnly => "model-only",
            Self::Codemode => "codemode",
            Self::Deferred => "deferred",
            Self::Hidden => "hidden",
        }
    }
}

/// A group of related tools, such as the tools of one MCP server (pi `ToolNamespace`,
/// `extensions/types.ts:527` @v1.0.1). Set with [`ToolDescriptor::namespace`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolNamespace {
    /// The group's name, for example `mcp__docs`.
    pub name: String,
    /// Short summary shown once with the group in model-facing tool listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Longer usage guidance, such as MCP server instructions. Not part of tool listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

impl ToolNamespace {
    /// A namespace with only a name; add the rest with [`Self::description`] and
    /// [`Self::instructions`].
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            instructions: None,
        }
    }

    /// Set the short summary shown with the group (builder-style).
    #[must_use]
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }

    /// Set the longer usage guidance (builder-style).
    #[must_use]
    pub fn instructions(mut self, i: impl Into<String>) -> Self {
        self.instructions = Some(i.into());
        self
    }
}

/// What a guest sends to register a tool (R-08-012/013; Pi `ToolDefinition`, types.ts:435-482).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    /// The tool's name: what the model calls, and the key the host routes `execute-tool` and any
    /// renderer back by (see [`Self::has_renderer`]).
    pub name: String,
    /// The display label. [`Self::new`] seeds it from `name`; [`Self::label`] overrides it.
    pub label: String,
    /// The description the model reads. Empty out of [`Self::new`]; set it with
    /// [`Self::description`].
    pub description: String,
    /// JSON-Schema for the parameters.
    pub parameters: Value,
    /// The tool's [`ExecMode`], or `None` to leave the runtime's default. Set with
    /// [`Self::execution_mode`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<ExecMode>,
    /// One-line snippet for the "Available tools" section of the default system prompt (Pi
    /// `ToolDefinition.promptSnippet`, `extensions/types.ts:442-443`); `None` omits the tool from
    /// that section. Set with [`Self::prompt_snippet`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_snippet: Option<String>,
    /// Guideline bullets for the "Guidelines" section of the default system prompt (Pi
    /// `ToolDefinition.promptGuidelines`, `extensions/types.ts:444-446`). Per func-03 R-03-039 each
    /// string must NAME its tool, so it stays meaningful once the tool is disabled.
    #[serde(default)]
    pub prompt_guidelines: Vec<String>,
    /// Whether this tool renders its own call/result rows. Set with [`Self::has_renderer`], whose
    /// doc carries the routing rule.
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
    /// How the model reaches the tool (pi `ToolDefinition.exposure`, `extensions/types.ts:509`
    /// @v1.0.1). Defaults to [`ToolExposure::Direct`], pi's omitted field. Set with
    /// [`Self::exposure`].
    #[serde(default)]
    pub exposure: ToolExposure,
    /// The group the tool belongs to (pi `ToolDefinition.namespace`, `extensions/types.ts:527`
    /// @v1.0.1); `None` = no group. Set with [`Self::namespace`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<ToolNamespace>,
    /// Whether registering the tool activates it (pi `ToolDefinition.defaultActive`,
    /// `extensions/types.ts:608` @v1.0.1). `false` keeps a [`ToolExposure::Direct`] or
    /// [`ToolExposure::ModelOnly`] tool registered but inactive until something names it; it means
    /// nothing for the other three exposures, which are never activated on registration. Defaults
    /// to `true`. Set with [`Self::default_active`].
    #[serde(default = "default_active_true")]
    pub default_active: bool,
}

fn default_active_true() -> bool {
    true
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
            exposure: ToolExposure::Direct,
            namespace: None,
            default_active: true,
        }
    }

    /// The `exposure` the guest hands the host: pi's string literal, or `None` for
    /// [`ToolExposure::Direct`], which is pi's OMITTED field (`_getToolExposure`'s `?? "direct"`,
    /// `core/agent-session.ts:1507` @v1.0.1), the same convention `render_shell` follows.
    pub fn wire_exposure(&self) -> Option<&'static str> {
        match self.exposure {
            ToolExposure::Direct => None,
            other => Some(other.as_str()),
        }
    }

    /// The `default-active` the guest hands the host: `None` for `true`, pi's omitted field
    /// (`defaultActive !== false`, `core/agent-session.ts:3554` @v1.0.1).
    pub fn wire_default_active(&self) -> Option<bool> {
        (!self.default_active).then_some(false)
    }

    /// Set the model-facing description (builder-style).
    #[must_use]
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    /// Override the display label, which [`Self::new`] seeded from the name (builder-style).
    #[must_use]
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = l.into();
        self
    }

    /// Set the "Available tools" system-prompt snippet (builder-style); see
    /// [`Self::prompt_snippet`].
    #[must_use]
    pub fn prompt_snippet(mut self, s: impl Into<String>) -> Self {
        self.prompt_snippet = Some(s.into());
        self
    }

    /// Set the tool's [`ExecMode`] (builder-style).
    #[must_use]
    pub fn execution_mode(mut self, m: ExecMode) -> Self {
        self.execution_mode = Some(m);
        self
    }

    /// Declare that this tool renders its own call/result rows (Pi `ToolDefinition.renderCall`/
    /// `renderResult`, extensions/types.ts:489-497). The host records the OWNER of the renderer for
    /// this tool NAME and routes rendering back through the guest's `render-call`/`render-result`
    /// exports (keyed by the tool name); register the matching renderer with
    /// [`crate::ExtensionApi::register_message_renderer`] under the SAME name.
    #[must_use]
    pub fn has_renderer(mut self, yes: bool) -> Self {
        self.has_renderer = yes;
        self
    }

    /// Set how the model reaches this tool (builder-style); see [`ToolExposure`]. A
    /// [`ToolExposure::Codemode`] or [`ToolExposure::Deferred`] tool is registered and callable but
    /// is not in model requests until something activates it by name.
    #[must_use]
    pub fn exposure(mut self, e: ToolExposure) -> Self {
        self.exposure = e;
        self
    }

    /// Put this tool in a [`ToolNamespace`] (builder-style).
    #[must_use]
    pub fn namespace(mut self, n: ToolNamespace) -> Self {
        self.namespace = Some(n);
        self
    }

    /// Set whether registering this tool activates it (builder-style); see
    /// [`Self::default_active`]. `false` registers a [`ToolExposure::Direct`] tool without
    /// activating it.
    #[must_use]
    pub fn default_active(mut self, active: bool) -> Self {
        self.default_active = active;
        self
    }

    /// Opt this tool in to provider-side constrained sampling (pi
    /// `ToolDefinition.constrainedSampling`, `extensions/types.ts:463` @v0.83.0). The host copies
    /// the declaration onto the runtime tool — upstream's `wrapToolDefinition`
    /// (`core/tools/tool-definition-wrapper.ts:14`) — and the provider adapter resolves it per
    /// model. A model that does not support the requested mode ignores it, except for
    /// [`StrictSampling::Require`], which upstream turns into an error.
    #[must_use]
    pub fn constrained_sampling(mut self, cs: ConstrainedSamplingConfig) -> Self {
        self.constrained_sampling = Some(ConstrainedSampling::Config(cs));
        self
    }
}

/// What a guest sends to register a command (R-08-016; Pi `registerCommand`, types.ts:1247).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDescriptor {
    /// The command's description, as passed to [`Self::new`].
    pub description: String,
    /// Static completions; dynamic completions use the command handler's `getArgumentCompletions`.
    #[serde(default)]
    pub completions: Vec<String>,
}

impl CommandDescriptor {
    /// A command with `description` and no static completions.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            completions: Vec::new(),
        }
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
/// instead references a signal by ID — the id it aborts through `ctx.ui()`'s
/// [`crate::ctx::Ui::abort_signal`] (the SAME id namespace [`DialogOptions::signal_id`] uses).
/// Since the guest is wasm-suspended for the whole duration of a host `exec` call, only Pi's
/// "already aborted before the call" branch (`exec.ts:66-68`) is reachable — the host checks
/// `signal_id` once, at call time, and starts the process pre-cancelled if it was already aborted
/// (`cyrup-ext/src/host/live.rs`'s `exec::Host::run`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecOptions {
    /// The child's working directory (Pi `ExecOptions.cwd`). Set with [`Self::cwd`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Kill the child if it is still running after this many milliseconds. Set with
    /// [`Self::timeout_ms`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// The abort-signal id the host checks ONCE, at call time — see the type doc for why only
    /// upstream's "already aborted" branch is reachable here. Set with [`Self::signal_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_id: Option<String>,
}

impl ExecOptions {
    /// Set the child's working directory (builder-style).
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
    /// Kill the child if it's still running after `ms` (builder-style).
    #[must_use]
    pub fn timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms);
        self
    }
    /// Bind an already-registered programmatic abort signal (builder-style; Pi `options.signal`,
    /// `exec.ts:65-72`): if `id` was aborted via `ctx.ui()`'s [`crate::ctx::Ui::abort_signal`]
    /// before this call, the exec starts pre-cancelled instead of running at all.
    #[must_use]
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
    /// Auto-dismiss the dialog after this many milliseconds, with a live countdown. `timeout` on
    /// the wire, with `timeoutMs` accepted as an alias — see EXT-048 in the type doc. Set with
    /// [`Self::timeout`].
    #[serde(
        rename = "timeout",
        alias = "timeoutMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_ms: Option<u64>,
    /// The id of a programmatic-dismiss signal the host maps to an abort token. Set with
    /// [`Self::signal`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_id: Option<String>,
}

impl DialogOptions {
    /// A dialog that auto-dismisses after `ms` with a live countdown (Pi `{timeout}`).
    pub fn timeout(ms: u64) -> Self {
        Self {
            timeout_ms: Some(ms),
            signal_id: None,
        }
    }
    /// A dialog dismissible via the named programmatic signal (Pi `{signal}`).
    pub fn signal(id: impl Into<String>) -> Self {
        Self {
            timeout_ms: None,
            signal_id: Some(id.into()),
        }
    }
}

/// CLI flag spec (Pi `registerFlag`, types.ts:1199-1209).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlagSpec {
    /// `"boolean"` | `"string"`.
    pub r#type: String,
    /// The value the flag takes when the user does not pass it; `None` for no default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// The flag's help text — pi's optional `description?` (`ExtensionFlag`,
    /// `core/extensions/types.ts:1737-1743` @v0.87.1). `None` is omitted from the wire, so `--help`
    /// can fall back to `Registered by <extension>` exactly where pi does
    /// (`description ?? \`Registered by ${flag.extensionPath}\``, `cli/args.ts:267`) — an empty
    /// string would print an empty description instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// --- Provider registration (Pi `ProviderConfig`, types.ts:1363-1421; R-08-019) ---

/// A custom LLM provider configuration registered via `register_provider`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// The provider's name (Pi `ProviderConfig.name`).
    pub name: String,
    /// Base URL for the provider's API; an individual model can carry its own in
    /// [`ProviderModelConfig::base_url`].
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
    /// Extra HTTP headers for this provider's requests; an individual model can carry its own in
    /// [`ProviderModelConfig::headers`].
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    /// The models this provider offers, each a [`ProviderModelConfig`].
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
    /// The request input-token count above which this tier's four rates apply.
    #[serde(default)]
    pub input_tokens_above: u64,
    /// This tier's input rate, in the same units as [`ModelCost::input`].
    #[serde(default)]
    pub input: f64,
    /// This tier's output rate, in the same units as [`ModelCost::output`].
    #[serde(default)]
    pub output: f64,
    /// This tier's cache-read rate, in the same units as [`ModelCost::cache_read`].
    #[serde(default)]
    pub cache_read: f64,
    /// This tier's cache-write rate, in the same units as [`ModelCost::cache_write`].
    #[serde(default)]
    pub cache_write: f64,
}

/// Per-token cost for a registered model (Pi `ProviderModelConfig.cost`, types.ts:1493 — rates plus
/// optional request-wide input pricing tiers).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    /// Per-token cost of input tokens.
    #[serde(default)]
    pub input: f64,
    /// Per-token cost of output tokens.
    #[serde(default)]
    pub output: f64,
    /// Per-token cost of tokens read from the prompt cache.
    #[serde(default)]
    pub cache_read: f64,
    /// Per-token cost of tokens written to the prompt cache.
    #[serde(default)]
    pub cache_write: f64,
    /// Optional request-wide input pricing tiers ([`ModelCostTier`]) for a model whose rates change
    /// above a token threshold; `None` means the four rates above are the whole story.
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
    /// The model's id.
    pub id: String,
    /// An optional display name for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The wire API family for this model, when it differs from the provider's
    /// [`ProviderConfig::api`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// A per-model base URL, when it differs from the provider's [`ProviderConfig::base_url`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Whether the model supports extended thinking (Pi `reasoning`).
    #[serde(default)]
    pub reasoning: bool,
    /// Pi `thinkingLevelMap`. Open-shaped, so it crosses as raw [`Value`] (see the type doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<Value>,
    /// Supported input modalities (Pi `input: ("text"|"image")[]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<String>,
    /// Per-token pricing for this model ([`ModelCost`]).
    #[serde(default)]
    pub cost: ModelCost,
    /// The model's context window in tokens; `None` when unspecified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Max output tokens (Pi `maxTokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Per-model HTTP headers, alongside the provider's [`ProviderConfig::headers`].
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    /// Pi `compat`. Open-shaped, so it crosses as raw [`Value`] (see the type doc).
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
    /// Where the fork lands relative to the entry ([`ForkPosition`]); `None` leaves the host's
    /// choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<ForkPosition>,
    /// Request the `with-session` re-binding callback after the fork — see the type doc. Set for
    /// you by [`crate::CommandCtx::fork_with_callback`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub with_session: bool,
}

/// Options for `navigateTree` (Pi `navigateTree(targetId, options)`, `extensions/types.ts:374-377` @v0.83.0, the options bag at `:376`; EXT-036 corrected `:362`): summarize the skipped span, with custom/replacement
/// instructions and an optional label.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigateOptions {
    /// Summarize the span the navigation skips.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub summarize: bool,
    /// Extra guidance for that summary; appended to the default prompt unless
    /// [`Self::replace_instructions`] is also set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    /// Use [`Self::custom_instructions`] INSTEAD of the default prompt rather than in addition to
    /// it. Load-bearing only together with it — upstream's selector is
    /// `if (replaceInstructions && customInstructions)`, so this flag alone falls through to the
    /// plain prompt (`branch-summarization.ts:326-334`, mirrored at
    /// `cyrup-session/src/compaction/branch.rs:242-252`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace_instructions: bool,
    /// An optional label for the summary entry the navigation produces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Options for `compact` (Pi `CompactOptions`, types.ts:296-300): extra guidance handed to the
/// compaction summarizer.
///
/// Pi's bag also carries `onComplete(result)` / `onError(error)` callbacks. Those are function
/// VALUES and cannot cross the component boundary, so they have no field here: a guest that needs
/// the completion signal subscribes to the `session_compact` event
/// ([`crate::events::SessionCompactEvent`], Pi's own `SessionCompactEvent`), which carries the
/// produced compaction entry. Pi's `compact()` is fire-and-forget on both sides — it "triggers
/// compaction without awaiting completion" — so the call itself returns as soon as the host has
/// queued it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactOptions {
    /// Extra guidance handed to the compaction summarizer; `None` = no instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
}

/// How a message a guest sends reaches a RUNNING agent — pi
/// `sendUserMessage(content, {deliverAs})` (`core/agent-session.ts:2006-2035` @v0.87.1, the union at
/// `:2008`), threaded on to `prompt` as `streamingBehavior` (`:2032`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliverAs {
    /// Redirect the run in flight.
    Steer,
    /// Queue the message for after the current run settles.
    FollowUp,
}

/// Options for `sendUserMessage` (pi `core/agent-session.ts:2006-2009` @v0.87.1).
///
/// Both fields are `Option` because upstream reads them through `options?.` — an ABSENT key and a
/// present one are different, and each absence has its own documented consequence (`:2029-2034`).
/// Omitting the whole bag is legal and is what `Self::default()` serializes to.
///
/// [`Self::expand_prompt_templates`] defaults to FALSE upstream and its doc comment there is
/// explicit about what that covers: "Whether to dispatch extension commands and expand skill
/// commands and prompt templates. Default: false." So a guest that relays a literal `"/foo"` gets a
/// user message reading `/foo` and NOT a dispatched extension command, unless it asks.
///
/// The host end of both fields is `cyrup-session-svc`'s `ControlOp::SendUserMessage` arm
/// (`session/control.rs`), which decodes this exact shape and hands it to `prompt_with` the way pi
/// hands it to `prompt`. Pinned by that crate's `ext_083_send_user_message_options` tests; until
/// EXT-083 the bag was decoded nowhere and discarded one frame after this type built it.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendUserMessageOptions {
    /// Delivery mode while the agent is streaming (pi `options.deliverAs`,
    /// `agent-session.ts:2008`, passed on as `streamingBehavior` at `:2032`).
    ///
    /// `None` is upstream's absent key, which is NOT "the host decides": while the agent is
    /// streaming pi refuses the message — "Agent is already processing. Specify streamingBehavior
    /// ('steer' or 'followUp') to queue the message." (`:1655-1659`) — and when it is idle there is
    /// no queue to pick, so the message simply starts a turn. Spell a variant whenever the message
    /// may land mid-run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliver_as: Option<DeliverAs>,
    /// Dispatch extension commands and expand skill commands and prompt templates. `None` is
    /// upstream's `?? false` (`agent-session.ts:2030`) — opt IN, not opt out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand_prompt_templates: Option<bool>,
}

/// Options for `newSession` (Pi types.ts:346): an optional parent session + the `with_session`
/// re-binding request (the `setup`/`withSession` closures map to the host re-binding flow).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionOptions {
    /// The parent session the new one descends from; `None` for a root session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
    /// Request the `with-session` re-binding callback after the new session is bound. Set for you
    /// by [`crate::CommandCtx::new_session_with_callback`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub with_session: bool,
}

/// Options for `switchSession` (Pi types.ts:368): the `with_session` re-binding request.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchSessionOptions {
    /// Request the `with-session` re-binding callback after the switch. Set for you by
    /// [`crate::CommandCtx::switch_session_with_callback`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub with_session: bool,
}
