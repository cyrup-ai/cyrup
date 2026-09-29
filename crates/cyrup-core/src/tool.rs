//! The runtime-facing `Tool` trait (arch-00 §3.4; conformance: func-02 §4.3 / func-03 §11).
//!
//! Defined here in `cyrup-core`; built-in tools implement it in `cyrup-tools` (arch-03), and
//! extension tools implement it in `cyrup-ext` (arch-08). Tools signal failure by returning
//! `Err(ToolError)` (never error text — func-02 R-02-024).

use crate::ToolCallId;
use crate::cancel::CancelToken;
use crate::message::Content;

/// Per-tool execution mode (func-02 R-02-014).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExecMode {
    #[default]
    Parallel,
    Sequential,
}

/// A tool's final result (func-02 §4.3). `details` is app/extension metadata, NOT sent to the model.
///
/// Mirrors Pi's `AgentToolResult<T>` (agent/src/types.ts:354-368). Every field past
/// `content`/`details` is optional upstream, so each Rust analogue has a `Default` that means
/// "absent"; build with `..Default::default()` so a later widening stays source-compatible.
#[derive(Clone, Debug, Default)]
pub struct ToolResult {
    pub content: Vec<Content>,
    pub details: Option<serde_json::Value>,
    /// Usage from the tool execution itself, if available. NOT part of main LLM context accounting
    /// (Pi `AgentToolResult.usage`, types.ts:360-361, upstream `2fd38684`). Reaches the transcript
    /// as `ToolResultMessage.usage` and is patchable by `after_tool_call` (Pi
    /// `AfterToolCallResult.usage`, types.ts:83-84).
    pub usage: Option<crate::message::Usage>,
    /// Names of tools introduced by this result and available from this transcript point onward
    /// (Pi `AgentToolResult.addedToolNames`, types.ts:362-363, upstream `3d8f7435`).
    ///
    /// This does NOT by itself change the active tool set — that is driven by the runtime's tool
    /// list. It is a cache-placement record telling a provider adapter with native deferred tool
    /// loading WHERE in the transcript a tool definition first becomes available; adapters without
    /// that capability ignore it and use the normal tool list. Empty = absent on the wire.
    pub added_tool_names: Vec<String>,
    /// Hint to stop the loop after this batch (func-02 §7.7); runtime-only, never persisted.
    /// Three-valued — see [`TerminateHint`] for what each value puts on the wire.
    pub terminate: TerminateHint,
}

/// A streamed progress update (func-02 R-02-023). Mirrors Pi's `AgentToolResult` (the
/// `partialResult` payload, types.ts:350-360): besides `content`/`details` a partial may carry the
/// optional early-termination hint `terminate`, which surfaces on the `tool_execution_update` event
/// (agent-loop.ts:641-653). `None` = the field is absent on the wire (Pi `terminate?: boolean`).
#[derive(Clone, Debug, Default)]
pub struct ToolUpdate {
    pub content: Vec<Content>,
    pub details: Option<serde_json::Value>,
    /// Optional early-termination hint carried by the partial result (Pi `AgentToolResult.terminate`,
    /// types.ts:359). [`TerminateHint::Unspecified`] omits the field from the emitted update,
    /// exactly as Pi omits an `undefined` `terminate`.
    pub terminate: TerminateHint,
}

/// Pi's `AgentToolResult.terminate?: boolean` (types.ts:354-368) as the three values it actually
/// has. Replaces the four encodings that used to carry this one fact — `bool`, `Option<bool>`,
/// `bool`, `Option<bool>` — under which pi's explicit `false` was unrepresentable. The wire key is
/// emitted iff [`Self::wire`] is `Some`.
///
/// There is deliberately no `From<bool>`: an implicit `false → Continue` is exactly the ambiguity
/// this type removes. The two named constructors say which mapping a call site means.
///
/// ```compile_fail
/// // A bare bool has two possible meanings here; the type refuses to guess.
/// let _hint: cyrup_core::TerminateHint = true.into();
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminateHint {
    /// pi `undefined` — key ABSENT on the wire; contributes nothing to the batch fold.
    #[default]
    Unspecified,
    /// pi `true` — key present as `true`; the batch terminates iff every finalized result says this.
    Terminate,
    /// pi explicit `false` — key PRESENT as `false`. Representable now; was not before.
    Continue,
}

impl TerminateHint {
    /// Whether this result votes to end the loop after the current batch.
    pub const fn requested(self) -> bool {
        matches!(self, Self::Terminate)
    }

    /// What goes on the wire: `None` = omit the key, `Some(b)` = emit `terminate: b`.
    pub const fn wire(self) -> Option<bool> {
        match self {
            Self::Unspecified => None,
            Self::Terminate => Some(true),
            Self::Continue => Some(false),
        }
    }

    /// The ONLY sanctioned `bool` → hint mapping, for the WASM host-side conversions of the WIT
    /// `tool-output.terminate` and `block-result.terminate` records: a guest `false` is "nothing
    /// said", never an explicit [`Self::Continue`], so the wire it produces (key absent) is
    /// byte-identical to before this type existed.
    pub const fn from_guest_bool(b: bool) -> Self {
        if b {
            Self::Terminate
        } else {
            Self::Unspecified
        }
    }

    /// A JSON `Option<bool>` (a tool-update chunk, a guest patch) maps 1:1 — here `Some(false)`
    /// IS [`Self::Continue`], because JSON can distinguish an absent key from a present `false`.
    pub const fn from_wire(o: Option<bool>) -> Self {
        match o {
            None => Self::Unspecified,
            Some(true) => Self::Terminate,
            Some(false) => Self::Continue,
        }
    }
}

/// Sink the runtime hands to a tool to stream progress. The runtime ignores updates after the
/// tool's execution settles (func-02 R-02-023).
pub type ToolUpdateSink = Box<dyn FnMut(ToolUpdate) + Send + 'static>;

/// How a tool's execution row is framed in the UI (Pi `ToolDefinition.renderShell`,
/// extensions/types.ts:448-449: `"default" | "self"`). `Default` = the runtime draws the standard
/// colored shell; `Selfish` = the tool renders its own framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToolRenderKind {
    /// The runtime renders the standard colored shell (Pi `"default"`).
    #[default]
    Default,
    /// The tool renders its own framing (Pi `"self"`).
    SelfRendered,
}

/// Tool failure (arch-03 §8). Re-exported by `cyrup-tools` as its `ToolError`.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ToolError {
    pub message: String,
    /// The structured `details` payload this failure carries into
    /// [`ToolResult::details`](crate::ToolResult::details), if any.
    ///
    /// Pi's `createErrorToolResult` (`agent-loop.ts:700-703` @v0.83.0) hard-codes `details: {}`
    /// for a throwing tool, because a JS `throw` carries nothing structured. `None` reproduces
    /// that exactly and is what every constructor produces unless a tool opts in, so this is a
    /// widening rather than a change: the empty object is still what a failure without a payload
    /// serializes as.
    ///
    /// # [CYRUP-DELTA] — a failing tool may report structure, where pi can only report a string
    ///
    /// **What differs.** A tool that knows something machine-readable about its own failure — the
    /// bash tool's process exit code is the case this was added for — can put it here instead of
    /// leaving a front-end to parse it back out of the human-readable message. `details` is
    /// documented as "not shown to the model", so nothing the model reads changes; what changes is
    /// the `details` object on the persisted `ToolResultMessage` of a failing tool, which is `{}`
    /// in pi and in cyrup for every tool that does not opt in.
    ///
    /// **What it costs.** A byte-comparison of a cyrup session file against a pi one diverges on
    /// exactly those rows. The alternative is the front-end parsing
    /// `Command exited with code {n}` out of the message text, which turns a human-readable
    /// diagnostic into an API — a copy-edit of that sentence would then be a wire regression.
    pub details: Option<serde_json::Value>,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            details: None,
        }
    }

    /// Attach the structured payload. See [`ToolError::details`].
    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    /// JSON-Schema-compatible parameter schema (func-01 §10).
    fn parameters(&self) -> &serde_json::Value;
    fn execution_mode(&self) -> ExecMode {
        ExecMode::Parallel
    }

    /// Description shown to the model (Pi `ToolDefinition.description`, extensions/types.ts:441).
    /// Defaulted to `""` so existing impls compile unchanged; per-tool values land in `cyrup-tools`.
    fn description(&self) -> &str {
        ""
    }

    /// Human-readable label for the UI (Pi `ToolDefinition.label`, extensions/types.ts:438-439).
    /// Default `None` = the runtime falls back to the tool `name` (today's behavior).
    fn label(&self) -> Option<&str> {
        None
    }

    /// One-line snippet for the "Available tools" section of the default system prompt (Pi
    /// `ToolDefinition.promptSnippet`, extensions/types.ts:442-443). Default `None` omits the tool
    /// from that section (today's behavior).
    fn prompt_snippet(&self) -> Option<&str> {
        None
    }

    /// Tool-specific guideline bullets for the "Guidelines" section of the default system prompt
    /// (Pi `ToolDefinition.promptGuidelines`, extensions/types.ts:444-446). Per func-03 R-03-039
    /// each string MUST name its tool so it stays meaningful once the tool is disabled. Default
    /// empty contributes nothing (today's behavior).
    ///
    /// TOOL-021 / EXT-007: the return type is `Vec<&str>`, not `&[&str]`. A borrowed slice of
    /// `&'static str` can only be produced by a tool whose guidelines are a compile-time constant
    /// array — which every BUILT-IN is, and which a WASM guest tool can never be: its descriptor
    /// owns a `Vec<String>` decoded from the component (`cyrup-ext/wit/world.wit`
    /// `prompt-guidelines`, copied at `host/live.rs:84` and stored at `registry.rs:27`). So the
    /// data crossed the ABI, reached the host, and had no reader — a guest declaring
    /// `promptGuidelines` silently contributed nothing to the system prompt. Borrowing `&str` from
    /// `&self` keeps the zero-copy property for the built-ins (`SLICE.to_vec()` copies pointers,
    /// not strings) while making the owned case expressible at all.
    fn prompt_guidelines(&self) -> Vec<&str> {
        Vec::new()
    }

    /// Whether the runtime draws the standard tool shell or the tool renders its own framing (Pi
    /// `ToolDefinition.renderShell`, extensions/types.ts:448-449). Defaulted to
    /// [`ToolRenderKind::Default`] (today's behavior).
    fn render_kind(&self) -> ToolRenderKind {
        ToolRenderKind::Default
    }

    /// Per-tool opt-in to provider-side constrained sampling (Pi
    /// `ToolDefinition.constrainedSampling`, extensions/types.ts:463 @v0.83.0).
    ///
    /// PROV-011 / EXT-024. Upstream the declaration is copied verbatim from the `ToolDefinition`
    /// onto the runtime `AgentTool` by `wrapToolDefinition`
    /// (`packages/coding-agent/src/core/tools/tool-definition-wrapper.ts:14` @v0.83.0, and back at
    /// `:42`), reaches the loop as `Context.tools[].constrainedSampling`, and is resolved by
    /// `packages/ai/src/api/constrained-sampling.ts`. This accessor is that copy's Rust
    /// counterpart: the runtime `Tool` is where the loop reads it from.
    ///
    /// Default `None` = the field is absent, which upstream is indistinguishable from `false`
    /// (`ConstrainedSampling::Disabled`), and is what a tool with no opinion keeps.
    ///
    /// Five coding built-ins DO declare it, and **unconditionally** as of pi v0.86.0 — CHANGELOG
    /// 0.86.0: *"Enabled strict-prefer JSON-schema sampling by default for built-in `read`, `bash`,
    /// `powershell`, `edit`, and `write` tools, without requiring `PI_EXPERIMENTAL`"*. Re-derived at
    /// **v0.87.1** by `git grep -n constrainedSampling v0.87.1 -- packages/coding-agent/src`, which
    /// returns the literal `constrainedSampling: { type: "json_schema", strict: "prefer" }` at
    /// `core/tools/read.ts:80`, `bash.ts:243` (the shared `createShellToolDefinition`, so upstream
    /// `powershell` inherits it from that one line), `edit.ts:156` and `write.ts:57` — and nothing
    /// else outside the field declaration, the two `tool-definition-wrapper.ts` copies and
    /// `experimental/micro/`. `grep`, `find` and `ls` carry no key on either side and keep this
    /// default. `cyrup-tools` mirrors the five by returning
    /// `cyrup_tools::tools::prefer_strict_tool_sampling`, which hands out
    /// [`crate::prefer_strict_tool_sampling`] — this crate's single strict-`prefer` `static`, read
    /// off the built-in registry by
    /// `cyrup_tools::tests::pi_tool_semantics::only_pis_five_tools_declare_strict_prefer_constrained_sampling`
    /// and carried through `cyrup_ext::wrapper` into the provider request by that module's
    /// `real_built_ins_carry_strict_prefer_through_the_wrapper_into_the_provider_request`.
    ///
    /// **The history, because this doc has gone stale twice.** At v0.83.0 no built-in declared the
    /// field. pi `7915cdac` ("feat(ai): add strict tool schema conversion", first tagged v0.84.2)
    /// added `constrainedSampling: getExperimentalToolSampling()` — flag-gated — to those same five
    /// plus `server/create-harness.ts`. v0.86.0 removed the gate, and at v0.87.1
    /// `getExperimentalToolSampling` is gone from the whole repo (`git grep -n
    /// getExperimentalToolSampling v0.87.1` → no hits; `core/experimental.ts` exports only
    /// `areExperimentalFeaturesEnabled`) and `server/create-harness.ts` no longer exists. So do not
    /// carry this paragraph forward: re-derive it at the tag you are reading.
    fn constrained_sampling(&self) -> Option<&crate::ConstrainedSampling> {
        None
    }

    /// Compatibility shim to normalize raw tool-call arguments before schema validation (Pi
    /// `ToolDefinition.prepareArguments`, extensions/types.ts:451-452). Default: identity
    /// passthrough — the arguments are returned unchanged (today's behavior).
    async fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
        args
    }

    /// Custom rendering of a tool *call* for the UI (Pi `ToolDefinition.renderCall`,
    /// `core/extensions/types.ts:488-489` @v0.84.2). The returned string is the rendered
    /// representation; `None` = the runtime uses its standard call framing.
    ///
    /// **This is a live contribution point** — see [`Tool::render_result`] for the resolution
    /// order it participates in. It is the tier upstream reaches for a tool that is neither a
    /// built-in nor extension-registered: pi's resolver is
    /// `toolDefinition.renderCall ?? builtInToolDefinition.renderCall`
    /// (`modes/interactive/components/tool-execution.ts:84-91` @v0.84.2), where `toolDefinition` is
    /// `session.getToolDefinition(name)` (`interactive-mode.ts:1996-1998`) reading
    /// `_toolDefinitions` — a map built from the built-in table OVERLAID with `allCustomTools`,
    /// which is `extensionRunner.getAllRegisteredTools()` **plus `this._customTools`**, the SDK
    /// tools handed to `createAgentSession({customTools})` (`core/agent-session.ts:2471-2495`).
    /// So upstream a plain SDK tool object supplies its own renderer through the very same map an
    /// extension's does, and this method is that seam in cyrup.
    ///
    /// Implementors do not register anything: `SessionBuilder` hands every configured custom tool
    /// to `ExtensionHost::register_native_tool_renderer`, and the host consults it when no
    /// extension owns a renderer for the name.
    fn render_call(&self, _args: &serde_json::Value) -> Option<String> {
        None
    }

    /// Custom rendering of a tool *result* for the UI (Pi `ToolDefinition.renderResult`,
    /// `core/extensions/types.ts:491-497` @v0.84.2). The returned string is the rendered
    /// representation; `None` = the runtime uses its standard result framing.
    ///
    /// # Resolution order
    ///
    /// Upstream is `toolDefinition.renderResult ?? builtInToolDefinition.renderResult`
    /// (`modes/interactive/components/tool-execution.ts:94-101`), i.e. a tool's OWN renderer wins
    /// over the built-in table keyed by name. cyrup resolves the same three tiers in the same
    /// order, split across two crates because the built-in tier draws `ratatui` lines rather than
    /// returning a string:
    ///
    /// 1. an extension that registered a renderer for this tool NAME
    ///    (`ExtensionHost::render_tool_result_outcome` → `registry.tool_renderer_owner`);
    /// 2. **this method**, via the host's native-tool table
    ///    (`ExtensionHost::register_native_tool_renderer`) — tiers 1 and 2 are one map upstream
    ///    (`allCustomTools`, `core/agent-session.ts:2472-2478`);
    /// 3. the built-in per-name dispatch in `cyrup_tui::transcript::tool_lines`.
    ///
    /// # Why `&Value` and not `&ToolResult`
    ///
    /// CYRUP-DELTA vs the typed `AgentToolResult` upstream's `renderResult` receives. The renderer
    /// is resolved at the point of DISPLAY, and by then the result has crossed the session-event
    /// boundary as `AgentSessionEvent::ToolExecutionEnd { result: serde_json::Value, .. }`
    /// (`cyrup-session-svc/src/event.rs:143-148`) — [`ToolResult`] is not `Deserialize`, so no
    /// typed value survives to the seam. Taking the `Value` the seam actually carries is what
    /// makes this method reachable; taking `&ToolResult` is what kept it dead.
    ///
    /// RESIDUAL, shared with tier 1 and NOT introduced here: upstream also passes
    /// `ToolRenderResultOptions` (`expanded`, `isPartial`), the theme and a `ToolRenderContext`.
    /// cyrup renders once as the event is folded into the transcript rather than at draw time, so
    /// neither tier has an expansion state to pass; the extension tier
    /// (`ExtensionHost::render_tool_result`) has always had the same shape.
    fn render_result(&self, _result: &serde_json::Value) -> Option<String> {
        None
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError>;
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

    /// A tool that overrides only the required methods, so every new surface method exercises its
    /// default — proving the additive trait surface preserves today's behavior.
    struct BareTool {
        params: serde_json::Value,
    }

    #[async_trait::async_trait]
    impl Tool for BareTool {
        fn name(&self) -> &str {
            "bare"
        }
        fn parameters(&self) -> &serde_json::Value {
            &self.params
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

    #[tokio::test]
    async fn defaulted_surface_preserves_behavior() {
        let t = BareTool {
            params: serde_json::json!({}),
        };
        assert_eq!(t.description(), "");
        assert_eq!(t.label(), None);
        assert_eq!(t.prompt_snippet(), None);
        assert!(t.prompt_guidelines().is_empty());
        assert_eq!(t.render_kind(), ToolRenderKind::Default);
        assert!(t.constrained_sampling().is_none());
        assert_eq!(t.render_call(&serde_json::json!({"a": 1})), None);
        assert_eq!(t.render_result(&serde_json::json!({"content": []})), None);
        // prepare_arguments is an identity passthrough.
        let args = serde_json::json!({"x": [1, 2, 3]});
        assert_eq!(t.prepare_arguments(args.clone()).await, args);
        // The pre-existing defaults are unchanged.
        assert_eq!(t.execution_mode(), ExecMode::Parallel);
    }

    /// PROV-011: a tool that DOES declare `constrainedSampling` must be able to hand it to the
    /// loop. Without this the `is_none()` assertion above is vacuous — it would hold for a trait
    /// surface that had no way to say yes.
    struct OptingTool {
        params: serde_json::Value,
        cs: crate::ConstrainedSampling,
    }

    #[async_trait::async_trait]
    impl Tool for OptingTool {
        fn name(&self) -> &str {
            "opting"
        }
        fn parameters(&self) -> &serde_json::Value {
            &self.params
        }
        fn constrained_sampling(&self) -> Option<&crate::ConstrainedSampling> {
            Some(&self.cs)
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
    fn a_tool_can_opt_in_to_constrained_sampling() {
        use crate::constrained_sampling::{
            ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants,
        };
        let t = OptingTool {
            params: serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            }),
            cs: ConstrainedSampling::Config(ConstrainedSamplingConfig::Grammar {
                variants: GrammarVariants {
                    openai_lark: Some("start: /[a-z]+/".into()),
                    openai_regex: None,
                },
            }),
        };
        let declared = t.constrained_sampling().expect("the tool declared it");
        match declared.config() {
            Some(ConstrainedSamplingConfig::Grammar { variants }) => {
                assert_eq!(variants.openai_lark.as_deref(), Some("start: /[a-z]+/"));
            }
            other => panic!("expected a grammar config, got {other:?}"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod terminate_hint_tests {
    use super::TerminateHint;

    const ALL: [TerminateHint; 3] = [
        TerminateHint::Unspecified,
        TerminateHint::Terminate,
        TerminateHint::Continue,
    ];

    /// The wire table is the whole point of the type: `Unspecified` puts NO key on the wire,
    /// `Terminate` an explicit `true`, `Continue` an explicit `false`.
    #[test]
    fn wire_table() {
        assert_eq!(TerminateHint::Unspecified.wire(), None);
        assert_eq!(TerminateHint::Terminate.wire(), Some(true));
        assert_eq!(TerminateHint::Continue.wire(), Some(false));
    }

    /// `from_wire` is the exact inverse of `wire` on all three values — a JSON `Option<bool>`
    /// round-trips losslessly, which is what makes pi's explicit `false` representable.
    #[test]
    fn wire_round_trips_through_from_wire() {
        for hint in ALL {
            assert_eq!(TerminateHint::from_wire(hint.wire()), hint, "{hint:?}");
        }
        assert_eq!(
            TerminateHint::from_wire(Some(false)),
            TerminateHint::Continue
        );
    }

    /// The WIT `bool` mapping is NOT the wire mapping: a guest `false` is "nothing said", so the
    /// key stays absent exactly as it did before the type existed.
    #[test]
    fn guest_bool_false_is_unspecified_not_continue() {
        assert_eq!(
            TerminateHint::from_guest_bool(false),
            TerminateHint::Unspecified
        );
        assert_eq!(
            TerminateHint::from_guest_bool(true),
            TerminateHint::Terminate
        );
        assert_eq!(TerminateHint::from_guest_bool(false).wire(), None);
    }

    /// Only an explicit `Terminate` votes to end the batch; both other values are a "no" vote.
    #[test]
    fn only_terminate_is_requested() {
        assert!(TerminateHint::Terminate.requested());
        assert!(!TerminateHint::Continue.requested());
        assert!(!TerminateHint::Unspecified.requested());
        assert_eq!(TerminateHint::default(), TerminateHint::Unspecified);
    }
}

/// TOOL-046 — a source scan over [`Tool::constrained_sampling`]'s own doc block.
///
/// That doc is an upstream-citing comment, which this repo treats as the record of what pi does. It
/// has now gone stale twice in the same way: written for v0.83.0 ("no built-in declares it"),
/// corrected at v0.84.2 to the flag-gated `constrainedSampling: getExperimentalToolSampling()` with
/// per-tool line citations and `server/create-harness.ts`, and left asserting exactly that after pi
/// v0.86.0 made the declaration unconditional. By v0.87.1 every one of those citations is dead:
/// `getExperimentalToolSampling` is gone from the whole repo and `server/create-harness.ts` no
/// longer exists.
///
/// So the artifact under test is source text, and the pin is a scan — the same shape as
/// `cyrup_tools`' `bash_prompt_guideline_deltas_are_tagged_cyrup_delta`. It reads only the doc block
/// between that accessor's opening line and its signature, so this module's own prose (which must
/// name the dead citations in order to forbid them) cannot make it pass or fail for the wrong
/// reason.
#[cfg(test)]
#[allow(clippy::panic)]
mod constrained_sampling_doc_tests {
    const SRC: &str = include_str!("tool.rs");

    /// The `///` block documenting `fn constrained_sampling`, and nothing else in this file.
    fn doc_block() -> &'static str {
        const OPENS: &str = "/// Per-tool opt-in to provider-side constrained sampling";
        const CLOSES: &str = "fn constrained_sampling(&self)";
        let Some((_, after_open)) = SRC.split_once(OPENS) else {
            panic!("`{OPENS}` no longer opens the doc block — retarget this scan");
        };
        let Some((block, _)) = after_open.split_once(CLOSES) else {
            panic!("`{CLOSES}` no longer follows the doc block — retarget this scan");
        };
        block
    }

    #[test]
    fn the_constrained_sampling_doc_cites_v0_87_1_and_not_the_dead_v0_84_2_lines() {
        let block = doc_block();
        // Non-vacuity: prove the slice really is that doc block, so an over- or under-read cannot
        // pass the forbidden-string half by simply matching nothing.
        assert!(
            block.contains("PROV-011"),
            "the extracted slice is not the constrained-sampling doc block: {block:?}"
        );
        assert!(
            !block.contains("mod constrained_sampling_doc_tests"),
            "the extracted slice over-ran into this test module, so the assertions below would be              vacuous"
        );

        // Only the LINE citations are forbidden: the prose legitimately names
        // `getExperimentalToolSampling` and `server/create-harness.ts` in order to record that pi
        // deleted both, and a blunt name scan could not tell that apart from a live claim.
        for dead in ["read.ts:222", "bash.ts:354", "edit.ts:329", "write.ts:200"] {
            assert!(
                !block.contains(dead),
                "`{dead}` is a v0.84.2 fact that is dead at v0.87.1 — pi v0.86.0 dropped the \
                 `PI_EXPERIMENTAL` gate, and `getExperimentalToolSampling` and \
                 `server/create-harness.ts` no longer exist upstream. Re-derive the citation at the \
                 tag you read instead of carrying this one forward."
            );
        }

        for live in [
            "read.ts:80",
            "bash.ts:243",
            "edit.ts:156",
            "write.ts:57",
            "v0.87.1",
            // The whole substance of pi 0.86.0: no flag gates the declaration any more.
            "unconditional",
        ] {
            assert!(
                block.contains(live),
                "the doc must cite `{live}` — the tag and lines where pi actually declares \
                 `constrainedSampling: {{ type: \"json_schema\", strict: \"prefer\" }}`"
            );
        }
    }
}
