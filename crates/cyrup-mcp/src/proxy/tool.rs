//! The registered tool — [`McpTool`], its [`cyrup_core::Tool`] impl and the
//! dispatch preamble. MCP-151, MCP-153, MCP-192, MCP-194, MCP-197, MCP-199.
//!
//! See [`crate::proxy`] for the module overview.

use std::sync::{Arc, OnceLock};

use serde::Deserialize;
use serde_json::{Map as JsonMap, Value, json};

use cyrup_core::{
    CancelToken, Tool, ToolCallId, ToolError, ToolRenderKind, ToolResult, ToolUpdateSink,
};

use crate::config::{McpSettings, ToolResultRendering};
use crate::errors::McpError;
use crate::owner::McpRuntimeOwner;
use crate::proxy::auth::{execute_auth_complete, execute_auth_start, execute_connect};
use crate::proxy::call::execute_call;
use crate::proxy::constants::{
    INIT_WAIT_TIMEOUT_MS, MCP_TOOL_GUIDELINE, MCP_TOOL_LABEL, MCP_TOOL_NAME,
    MCP_TOOL_PROMPT_SNIPPET,
};
use crate::proxy::discovery::{
    execute_describe, execute_instructions, execute_list, execute_search, execute_status,
};
use crate::proxy::env::ProxyCtx;
use crate::proxy::error_vocab::McpErrorCode;
use crate::proxy::results::{details_err, text_result};

// ==================================================================================================
// 14 · The registered tool (MCP-151, MCP-153, MCP-192, MCP-194, MCP-197, MCP-199)
// ==================================================================================================

/// `index.ts:829` — the JSON Schema handed to the provider.
///
/// Twelve properties, **all optional** (so no `required` is emitted), `args` a `string | object`
/// union. Upstream's `optionalNumber` helper exists only to dodge a TypeBox 1.x artefact — an
/// enumerable `~optional` key that Gemini rejects with `400 INVALID_ARGUMENT` — and both of its
/// branches serialise identically, so in Rust (where [`Tool::parameters`] returns a raw JSON Schema)
/// the shim evaporates.
///
/// **One cut-driven edit**: `action`'s description upstream reads
/// `"Action: 'ui-messages', 'auth-start', or 'auth-complete'"`. With MCP Apps out of scope there are
/// exactly two legal values and the description must say so — a model told about `ui-messages` will
/// call it and get a `mcp_status` fall-through with no explanation.
///
/// **All twelve keep their upstream names.** `cyrup_permission_system::manager`'s
/// `create_mcp_permission_targets` reads `{tool, server, connect, describe, search}` in that
/// precedence; renaming any of the five silently changes which permission rules apply (13d §13.2).
///
/// **MCP-194**: this serialises with keys in *alphabetical* order — `action, args, connect,
/// describe, includeSchemas, instructions, limit, offset, regex, search, server, tool` — because the
/// workspace builds `serde_json` without `preserve_order`, so `serde_json::Map` is a `BTreeMap`.
/// Accepted, per the recommendation. Holding the schema as a pre-rendered `&'static str` is the
/// trap: parsing still normalises into a `Map`.
#[must_use]
pub fn mcp_tool_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        json!({
            "type": "object",
            "properties": {
                "tool": {"type": "string", "description": "Tool name to call (e.g., 'xcodebuild_list_sims')"},
                "args": {
                    "anyOf": [
                        {"type": "string", "description": "Arguments as a JSON string (e.g., '{\"key\": \"value\"}')"},
                        {"type": "object", "additionalProperties": true, "description": "Arguments as a JSON object (e.g., { \"key\": \"value\" })"}
                    ],
                    "description": "Tool arguments as a JSON object, or as a JSON string encoding one"
                },
                "connect": {"type": "string", "description": "Server name to connect (lazy connect + metadata refresh)"},
                "describe": {"type": "string", "description": "Tool name to describe (shows parameters)"},
                "instructions": {"type": "string", "description": "Server name to show that server's usage instructions"},
                "search": {"type": "string", "description": "Search tools by name/description"},
                "regex": {"type": "boolean", "description": "Treat search as regex (default: substring match)"},
                "includeSchemas": {"type": "boolean", "description": "Include parameter schemas in search results (default: true)"},
                "limit": {"type": "number", "minimum": 1, "description": "Maximum search results to return (default: 12)"},
                "offset": {"type": "number", "minimum": 0, "description": "Search result offset (default: 0)"},
                "server": {"type": "string", "description": "Filter to specific server (also disambiguates tool calls)"},
                "action": {"type": "string", "description": "Action: 'auth-start' or 'auth-complete'"}
            }
        })
    })
}

/// `Some` for **any** present value including `null`; `None` only when the key is absent.
///
/// This is JavaScript's `"args" in params` / `params.args !== undefined` distinction, which serde's
/// `Option<Value>` erases by mapping a present `null` onto `None`. Only [`McpToolParams::args`]
/// needs it — see the field's own note.
fn present_value<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

/// The twelve gateway parameters, deserialised.
///
/// `limit`/`offset` are `f64` because the schema says `number` and [`crate::proxy::paginate`] reproduces JS's
/// `Number.isFinite` / `Math.trunc` handling of a fractional or absurd value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpToolParams {
    /// Tool name to call.
    pub tool: Option<String>,
    /// Arguments as a JSON object, or a JSON string encoding one.
    ///
    /// [`present_value`], not serde's own `Option`, because this field is the **only** one whose
    /// presence is load-bearing twice over: `parseArgs` rejects an explicit `null` (`index.ts:880`)
    /// and `1bf3671`'s rescue tests `params.args !== undefined` (`index.ts:903`). Serde folds a
    /// present `null` into `None`, which would answer `mcp({ args: null })` with a status envelope
    /// where upstream throws. The sibling `Option` fields keep the plain mapping: their modes are
    /// selected by `!== undefined` too, but no upstream arm distinguishes an explicit `null` from
    /// an absent key for them.
    #[serde(default, deserialize_with = "present_value")]
    pub args: Option<Value>,
    /// Server name to connect.
    pub connect: Option<String>,
    /// Tool name to describe.
    pub describe: Option<String>,
    /// Server name whose instructions to show.
    pub instructions: Option<String>,
    /// Search query. Dispatch tests `!== undefined`, so `""` reaches the mode.
    pub search: Option<String>,
    /// Treat `search` as a regex.
    pub regex: Option<bool>,
    /// Include parameter schemas in search results.
    pub include_schemas: Option<bool>,
    /// Maximum search results.
    pub limit: Option<f64>,
    /// Search result offset.
    pub offset: Option<f64>,
    /// Server filter / call disambiguator.
    pub server: Option<String>,
    /// `auth-start` or `auth-complete`.
    pub action: Option<String>,
}

impl McpToolParams {
    /// `index.ts:886` `hasGatewayMode(value)` — whether any of the seven dispatch-bearing keys is
    /// present. Drives the "gateway params were nested inside `args`" rescue.
    fn has_gateway_mode(&self) -> bool {
        self.tool.is_some()
            || self.connect.is_some()
            || self.describe.is_some()
            || self.instructions.is_some()
            || self.search.is_some()
            || self.server.is_some()
            || self.action.is_some()
    }
}

/// `index.ts:863` `parseArgs(value)`.
///
/// `undefined` and `""` yield `None`. A string is `JSON.parse`d and a `SyntaxError` rethrown as
/// `Invalid args JSON: <e.message>`; anything that is not a non-null, non-array object throws
/// `Invalid args: expected a JSON object, got <gotType>`.
///
/// **These two are thrown, not returned** — they surface as tool-execution errors
/// (`Err(ToolError)`), never as `details.error` codes.
fn parse_args(value: Option<&Value>) -> Result<Option<Value>, ToolError> {
    let Some(value) = value else { return Ok(None) };
    let parsed: Value = match value {
        // NOT an early `Ok(None)`: upstream's only early return is `value === undefined || value
        // === ""` (`index.ts:865`). A present `null` falls through to the object test, where
        // `typeof null === "object"` is defeated by the explicit `args === null` clause and it
        // throws `got null` (`index.ts:880-882`).
        Value::String(text) if text.is_empty() => return Ok(None),
        Value::String(text) => serde_json::from_str(text)
            .map_err(|error| ToolError::new(format!("Invalid args JSON: {error}")))?,
        other => other.clone(),
    };
    let got_type = match &parsed {
        Value::Object(_) => return Ok(Some(parsed)),
        Value::Array(_) => "array",
        Value::Null => "null",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
    };
    Err(ToolError::new(format!(
        "Invalid args: expected a JSON object, got {got_type}"
    )))
}

/// The init gate's four states — upstream's `state` slot crossed with its `initPromise` slot.
#[derive(Clone)]
pub enum InitPhase {
    /// `state === null && initPromise === undefined` — nothing is coming.
    NotInitialized,
    /// `state === null && initPromise !== undefined` — a build is live.
    Pending,
    /// The build resolved and committed.
    Ready(Arc<ProxyCtx>),
    /// The build rejected. Carries the message the `init_failed` envelope reports.
    Failed(String),
}

/// `index.ts:906`'s init gate: `awaitWithTimeout(initPromise, INIT_WAIT_TIMEOUT_MS)`.
///
/// A [`tokio::sync::watch`] rather than a promise, because a cyrup generation publishes its state
/// once and every later `execute` reads the same slot. `current_owner` is upstream's module-scoped
/// `currentOwner`, captured at the top of `execute` and used for the generation fence.
pub struct ProxyInitGate {
    phase: tokio::sync::watch::Receiver<InitPhase>,
    owner: arc_swap::ArcSwapOption<McpRuntimeOwner>,
}

/// What [`ProxyInitGate::wait`] resolved to.
enum InitWait {
    Ready(Arc<ProxyCtx>),
    TimedOut,
    Failed(String),
    NotInitialized,
}

impl ProxyInitGate {
    /// Build a gate over the generation's phase channel.
    #[must_use]
    pub fn new(phase: tokio::sync::watch::Receiver<InitPhase>) -> Self {
        Self {
            phase,
            owner: arc_swap::ArcSwapOption::empty(),
        }
    }

    /// Publish the generation's owner — upstream's `currentOwner = owner` assignment.
    pub fn set_owner(&self, owner: Option<Arc<McpRuntimeOwner>>) {
        self.owner.store(owner);
    }

    /// `const executeOwner = currentOwner;` — read once at the top of `execute`.
    fn current_owner(&self) -> Option<Arc<McpRuntimeOwner>> {
        self.owner.load_full()
    }

    /// Race the live init against [`INIT_WAIT_TIMEOUT_MS`], with the already-settled phases
    /// short-circuiting.
    async fn wait(&self) -> InitWait {
        let mut rx = self.phase.clone();
        loop {
            match rx.borrow_and_update().clone() {
                InitPhase::Ready(ctx) => return InitWait::Ready(ctx),
                InitPhase::Failed(message) => return InitWait::Failed(message),
                InitPhase::NotInitialized => return InitWait::NotInitialized,
                InitPhase::Pending => {}
            }
            let changed = tokio::time::timeout(
                std::time::Duration::from_millis(INIT_WAIT_TIMEOUT_MS),
                rx.changed(),
            )
            .await;
            match changed {
                // Timer won the race. Upstream's timer is `unref`'d; a `tokio::time::timeout`
                // future is dropped here, which is the same "does not hold the process open".
                Err(_) => return InitWait::TimedOut,
                // The sender was dropped: the generation went away without ever committing.
                Ok(Err(_)) => return InitWait::NotInitialized,
                Ok(Ok(())) => {}
            }
        }
    }
}

/// The one tool the model sees.
///
/// `renderShell` is **not a constant**: `index.ts:137` computes
/// `toolRenderShell = toolRenderOptions.resultRendering === "compact" ? "self" : "default"`, and
/// `tool-result-renderer.ts`'s `resolveMcpToolRenderOptions` sets
/// `resultRendering = settings?.toolResultRendering === "boxed" ? "boxed" : "compact"`. So the shell
/// is [`ToolRenderKind::SelfRendered`] **by default** and [`ToolRenderKind::Default`] exactly when
/// the user sets `settings.toolResultRendering: "boxed"` — read from the *early* config at load
/// time, so it never changes within a session (MCP-197).
pub struct McpTool {
    description: String,
    render_kind: ToolRenderKind,
    guidelines: Vec<String>,
    gate: Arc<ProxyInitGate>,
}

impl McpTool {
    /// Construct the tool with a description produced by [`crate::proxy::build_proxy_description`].
    ///
    /// **MCP-193 / `HA-1`**: the description is frozen *per instance* because
    /// `Tool::description(&self) -> &str` returns a *borrowed* `&str`, so an `RwLock` cannot satisfy
    /// the signature without leaking. That is not a limitation, because re-registration — not
    /// mutation — is the mechanism upstream uses (`syncProxyTool` → `pi.registerTool`), and it now
    /// exists here too: `McpExtension::sync_tool_surface` re-runs the resolution pass through
    /// `LateRegistrar` → `ExtensionHost::register_late_tool` → `refresh_tools` →
    /// `AgentSession::{refresh_extension_tools, push_active_tools}`, minting a FRESH `McpTool` with
    /// the new description and reaching the live agent at the next turn boundary.
    ///
    /// `syncProxyTool`'s description comparison is honoured by the sink's `should_register_proxy`,
    /// so an unchanged description does not re-register and does not invalidate the provider's
    /// prompt-cache prefix.
    ///
    /// HISTORICAL, and no longer true: this doc used to record that a cold `mcp-cache.json` left
    /// the first session's description naming no servers with no way to refresh it, and that
    /// `settings.disableProxyTool` therefore had to be treated as unsupported because hiding a tool
    /// you cannot re-register is one-way. Both followed from the missing handle, which HA-1 supplies.
    #[must_use]
    pub fn new(description: String, settings: &McpSettings, gate: Arc<ProxyInitGate>) -> Self {
        let render_kind = match settings.tool_result_rendering() {
            ToolResultRendering::Boxed => ToolRenderKind::Default,
            ToolResultRendering::Compact => ToolRenderKind::SelfRendered,
        };
        Self {
            description,
            render_kind,
            guidelines: vec![MCP_TOOL_GUIDELINE.to_string()],
            gate,
        }
    }
}

#[async_trait::async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        MCP_TOOL_NAME
    }

    fn parameters(&self) -> &Value {
        mcp_tool_schema()
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn label(&self) -> Option<&str> {
        Some(MCP_TOOL_LABEL)
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(MCP_TOOL_PROMPT_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Vec<&str> {
        self.guidelines.iter().map(String::as_str).collect()
    }

    fn render_kind(&self) -> ToolRenderKind {
        self.render_kind
    }

    /// `index.ts:849` `execute` — the dispatch preamble and the nine-arm router (MCP-153).
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let execute_owner = self.gate.current_owner();
        let mut params: McpToolParams = serde_json::from_value(params).unwrap_or_default();

        // 1 · Args coercion, and the "nested gateway params" rescue.
        let mut parsed_args = parse_args(params.args.as_ref())?;
        if !params.has_gateway_mode() {
            match parsed_args.clone() {
                Some(nested_value) => {
                    let nested: McpToolParams =
                        serde_json::from_value(nested_value).unwrap_or_default();
                    if nested.has_gateway_mode() {
                        parsed_args = parse_args(nested.args.as_ref())?;
                        params = nested;
                    } else {
                        return Err(ToolError::new(
                            "Gateway params were nested inside `args`; pass them top-level (for example, mcp({ search: \"...\" }) or mcp({ tool: \"...\", args: {} })).",
                        ));
                    }
                }
                None if params.args.is_some() => {
                    return Err(ToolError::new(
                        "Gateway params were nested inside `args`; pass them top-level (for example, mcp({ search: \"...\" }) or mcp({ tool: \"...\", args: {} })).",
                    ));
                }
                None => {}
            }
        }

        // 2 · The init-wait gate. These three envelopes carry **no `mode` key**.
        let ctx = match self.gate.wait().await {
            InitWait::Ready(ctx) => ctx,
            InitWait::TimedOut => {
                let mut map = JsonMap::new();
                map.insert(
                    "error".to_string(),
                    Value::String(McpErrorCode::InitTimeout.as_str().to_string()),
                );
                map.insert("timeoutMs".to_string(), json!(INIT_WAIT_TIMEOUT_MS));
                return Ok(text_result(
                    "MCP initialization is still in progress. Try again shortly.",
                    map,
                ));
            }
            InitWait::Failed(message) => {
                // An owner abort rethrows rather than reporting; anything else is `init_failed`.
                if let Some(owner) = execute_owner.as_ref()
                    && owner.token().is_cancelled()
                {
                    return Err(ToolError::new(message));
                }
                let mut map = JsonMap::new();
                map.insert(
                    "error".to_string(),
                    Value::String(McpErrorCode::InitFailed.as_str().to_string()),
                );
                map.insert("message".to_string(), Value::String(message.clone()));
                return Ok(text_result(
                    format!("MCP initialization failed: {message}"),
                    map,
                ));
            }
            InitWait::NotInitialized => {
                let mut map = JsonMap::new();
                map.insert(
                    "error".to_string(),
                    Value::String(McpErrorCode::NotInitialized.as_str().to_string()),
                );
                return Ok(text_result("MCP not initialized", map));
            }
        };

        // 3 · The generation fence — a stale lifecycle generation aborts rather than writing into a
        // restarted session.
        if let Some(owner) = execute_owner.as_ref() {
            owner
                .throw_if_inactive()
                .map_err(|error| ToolError::new(error.to_string()))?;
        }

        // 4 · Dispatch, first match wins. Nine arms after the cut, in unchanged relative order. An
        // unrecognised `action` (`"frobnicate"`, and now also `"ui-messages"`) falls through arms
        // 1-2 and lands on whichever of 3-9 matches — it is **not** an error.
        let to_tool_error = |error: McpError| ToolError::new(error.to_string());
        match params.action.as_deref() {
            Some("auth-start") => {
                let Some(server) = params.server.as_deref().filter(|value| !value.is_empty())
                else {
                    let map = details_err("auth-start", McpErrorCode::MissingServer);
                    return Ok(text_result(
                        "auth-start requires `server`. Example: mcp({ action: \"auth-start\", server: \"linear-server\" })",
                        map,
                    ));
                };
                return execute_auth_start(&ctx, server, &cancel)
                    .await
                    .map_err(to_tool_error);
            }
            Some("auth-complete") => {
                let Some(server) = params.server.as_deref().filter(|value| !value.is_empty())
                else {
                    let map = details_err("auth-complete", McpErrorCode::MissingServer);
                    return Ok(text_result("auth-complete requires `server`.", map));
                };
                let input = parsed_args
                    .as_ref()
                    .and_then(|args| {
                        args.get("redirectUrl")
                            .or_else(|| args.get("code"))
                            .or_else(|| args.get("input"))
                    })
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty());
                let Some(input) = input else {
                    let map = details_err("auth-complete", McpErrorCode::MissingInput);
                    return Ok(text_result(
                        "auth-complete requires args with `redirectUrl`, `code`, or `input`.",
                        map,
                    ));
                };
                return execute_auth_complete(&ctx, server, input, &cancel)
                    .await
                    .map_err(to_tool_error);
            }
            _ => {}
        }

        if let Some(tool) = params.tool.as_deref().filter(|value| !value.is_empty()) {
            // `origin` is left unset here; `executeCall` derives `resource` or `proxy`.
            return execute_call(
                &ctx,
                tool,
                parsed_args.as_ref(),
                params.server.as_deref().filter(|value| !value.is_empty()),
                &cancel,
                None,
            )
            .await
            .map_err(to_tool_error);
        }
        if let Some(server) = params.connect.as_deref().filter(|value| !value.is_empty()) {
            let result = execute_connect(&ctx, server, &cancel)
                .await
                .map_err(to_tool_error)?;
            // `syncToolSurface(ctx)` runs AFTER the mode returns and BEFORE the result is handed
            // back, so the next turn sees the refreshed surface.
            ctx.env.sync_tool_surface();
            return Ok(result);
        }
        if let Some(name) = params.describe.as_deref().filter(|value| !value.is_empty()) {
            return Ok(execute_describe(&ctx, name));
        }
        if let Some(server) = params
            .instructions
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return Ok(execute_instructions(&ctx, server));
        }
        // `!== undefined`, so `search: ""` reaches the mode rather than falling through to status.
        if let Some(query) = params.search.as_deref() {
            return Ok(execute_search(
                &ctx,
                query,
                params.regex,
                params.server.as_deref().filter(|value| !value.is_empty()),
                params.include_schemas,
                params.limit,
                params.offset,
            ));
        }
        if let Some(server) = params.server.as_deref().filter(|value| !value.is_empty()) {
            return Ok(execute_list(&ctx, server));
        }
        Ok(execute_status(&ctx))
    }
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
    use crate::proxy::testsupport::{FakeEnv, config_with, ctx_with, stdio};
    use crate::proxy::tool_metadata::ToolMetadata;
    use cyrup_core::Content;

    // ---- MCP-151 / MCP-194 · the tool schema --------------------------------------------------------

    #[test]
    fn tool_schema_declares_twelve_optional_properties_and_two_actions() {
        let schema = mcp_tool_schema();
        assert_eq!(schema["type"], json!("object"));
        assert!(
            schema.get("required").is_none(),
            "every property is optional"
        );
        let properties = schema["properties"].as_object().expect("properties");
        assert_eq!(properties.len(), 12);
        for name in [
            "tool",
            "args",
            "connect",
            "describe",
            "instructions",
            "search",
            "regex",
            "includeSchemas",
            "limit",
            "offset",
            "server",
            "action",
        ] {
            assert!(properties.contains_key(name), "missing property {name}");
        }
        // `args` is a union, not a bare string.
        assert!(properties["args"]["anyOf"].is_array());
        // The cut-driven edit: exactly two legal actions are named.
        let action = properties["action"]["description"]
            .as_str()
            .expect("description");
        assert_eq!(action, "Action: 'auth-start' or 'auth-complete'");
        assert!(!action.contains("ui-messages"));

        // MCP-194: the decision is visible in the test. `serde_json::Map` is a `BTreeMap` under this
        // workspace's features, so the properties serialise alphabetically.
        let order: Vec<&str> = properties.keys().map(String::as_str).collect();
        assert_eq!(
            order,
            vec![
                "action",
                "args",
                "connect",
                "describe",
                "includeSchemas",
                "instructions",
                "limit",
                "offset",
                "regex",
                "search",
                "server",
                "tool"
            ]
        );
    }

    // ---- MCP-153 · args coercion --------------------------------------------------------------------

    #[test]
    fn parse_args_accepts_objects_and_json_strings_and_throws_otherwise() {
        assert_eq!(parse_args(None).unwrap(), None);
        assert_eq!(parse_args(Some(&json!(""))).unwrap(), None);
        assert_eq!(
            parse_args(Some(&json!({"a": 1}))).unwrap(),
            Some(json!({"a": 1}))
        );
        assert_eq!(
            parse_args(Some(&json!("{\"a\":1}"))).unwrap(),
            Some(json!({"a": 1}))
        );

        let array = parse_args(Some(&json!([]))).unwrap_err();
        assert_eq!(
            array.message,
            "Invalid args: expected a JSON object, got array"
        );
        let null_literal = parse_args(Some(&json!("null"))).unwrap_err();
        assert_eq!(
            null_literal.message,
            "Invalid args: expected a JSON object, got null"
        );
        let number = parse_args(Some(&json!(7))).unwrap_err();
        assert_eq!(
            number.message,
            "Invalid args: expected a JSON object, got number"
        );
        let broken = parse_args(Some(&json!("{"))).unwrap_err();
        assert!(
            broken.message.starts_with("Invalid args JSON: "),
            "{}",
            broken.message
        );
    }

    #[test]
    fn has_gateway_mode_reads_exactly_the_seven_dispatch_keys() {
        let mut params = McpToolParams::default();
        assert!(!params.has_gateway_mode());
        params.args = Some(json!({"a": 1}));
        assert!(!params.has_gateway_mode(), "`args` alone is not a mode");
        params.regex = Some(true);
        params.include_schemas = Some(false);
        params.limit = Some(5.0);
        params.offset = Some(1.0);
        assert!(
            !params.has_gateway_mode(),
            "the four tuning keys are not modes"
        );
        params.search = Some(String::new());
        assert!(params.has_gateway_mode(), "`search: \"\"` IS a mode");
    }

    // ---- MCP-197 · the render-shell fork ----------------------------------------------------------------

    #[test]
    fn render_shell_defaults_to_self_and_flips_on_boxed() {
        let compact = McpSettings::default();
        assert_eq!(
            compact.tool_result_rendering(),
            ToolResultRendering::Compact
        );
        let boxed = McpSettings {
            tool_result_rendering: Some(ToolResultRendering::Boxed),
            ..McpSettings::default()
        };
        assert_eq!(boxed.tool_result_rendering(), ToolResultRendering::Boxed);

        let (_, rx) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let default_tool = McpTool::new(String::new(), &compact, Arc::clone(&gate));
        assert_eq!(default_tool.render_kind(), ToolRenderKind::SelfRendered);
        let boxed_tool = McpTool::new(String::new(), &boxed, gate);
        assert_eq!(boxed_tool.render_kind(), ToolRenderKind::Default);
        assert_eq!(boxed_tool.name(), "mcp");
        assert_eq!(boxed_tool.label(), Some("MCP"));
        assert_eq!(boxed_tool.prompt_snippet(), Some(MCP_TOOL_PROMPT_SNIPPET));
        assert_eq!(boxed_tool.prompt_guidelines(), vec![MCP_TOOL_GUIDELINE]);
    }

    // ---- the dispatch preamble's three mode-less envelopes ------------------------------------------------

    #[tokio::test]
    async fn not_initialized_and_timeout_envelopes_carry_no_mode_key() {
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);
        let result = tool
            .execute(
                ToolCallId::from("call-1"),
                json!({}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("the not-initialized envelope is returned, not thrown");
        let details = result.details.expect("details");
        assert_eq!(details["error"], json!("not_initialized"));
        assert!(
            details.get("mode").is_none(),
            "the init envelopes carry NO mode key"
        );
        match result.content.first() {
            Some(Content::Text { text, .. }) => assert_eq!(text, "MCP not initialized"),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_live_init_is_raced_against_the_thirty_second_timeout() {
        let (keep, rx) = tokio::sync::watch::channel(InitPhase::Pending);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);
        let result = tool
            .execute(
                ToolCallId::from("call-2"),
                json!({}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("the timeout envelope is returned, not thrown");
        drop(keep);
        let details = result.details.expect("details");
        assert_eq!(details["error"], json!("init_timeout"));
        assert_eq!(details["timeoutMs"], json!(30_000));
        assert!(details.get("mode").is_none());
    }

    #[tokio::test]
    async fn invalid_args_are_thrown_before_the_init_gate_is_consulted() {
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);
        let error = tool
            .execute(
                ToolCallId::from("call-3"),
                json!({"tool": "x", "args": "[]"}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect_err("a bad `args` is an Err(ToolError), never a details.error code");
        assert_eq!(
            error.message,
            "Invalid args: expected a JSON object, got array"
        );
    }

    /// `index.ts:886-906` (upstream `1bf3671` "fix: recover nested mcp proxy args", #364) — a model
    /// that wraps the WHOLE gateway request in `args` used to match no arm and silently get status
    /// back. The rescue re-reads `args` as the params object and re-parses ITS `args`, so a nested
    /// request dispatches exactly as if it had been passed top-level.
    #[tokio::test]
    async fn gateway_params_nested_inside_args_are_rescued_and_dispatched() {
        let config = config_with(&[("srv", stdio("a"))]);
        let tools: Vec<ToolMetadata> = (0..5)
            .map(|index| {
                ToolMetadata::new(
                    format!("srv_report_{index}"),
                    format!("report_{index}"),
                    "Reporting",
                )
            })
            .collect();
        let (ctx, _env) = ctx_with(config, &[("srv", tools)], &[], FakeEnv::default());
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::Ready(ctx));
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);

        // A JSON-STRING nesting. `search` AND its `limit` come out of the rescued object, so the
        // page is 3 wide rather than the 12-wide default — proof the whole object, not just the
        // dispatch key, is what dispatch now reads.
        let rescued = tool
            .execute(
                ToolCallId::from("call-1"),
                json!({"args": "{\"search\":\"report\",\"limit\":3}"}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a rescued request dispatches like a top-level one");
        let details = rescued.details.expect("details");
        assert_eq!(
            details["mode"],
            json!("search"),
            "`status` would mean the rescue never ran"
        );
        assert_eq!(details["count"], json!(5));
        assert_eq!(
            details["nextOffset"],
            json!(3),
            "`limit: 3` came from the nested object"
        );

        // An OBJECT nesting reaches the later arms too.
        let described = tool
            .execute(
                ToolCallId::from("call-2"),
                json!({"args": {"describe": "srv_report_0"}}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a rescued request dispatches like a top-level one");
        assert_eq!(
            described.details.expect("details")["mode"],
            json!("describe")
        );

        // `parsedArgs = parseArgs(nestedParams.args)` — the INNER `args` is parsed a second time, so
        // a broken inner string throws instead of searching with the outer object still in hand.
        let error = tool
            .execute(
                ToolCallId::from("call-3"),
                json!({"args": {"search": "report", "args": "{"}}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect_err("the nested `args` is re-parsed after the rescue");
        assert!(
            error.message.starts_with("Invalid args JSON: "),
            "{}",
            error.message
        );
    }

    /// `index.ts:902` and `index.ts:905` — an `args` that is NOT a gateway request is a hard error, never a silent
    /// status. Both throw sites carry the same sentence: a parsed-but-modeless object, and an `args`
    /// that parses to nothing (`""`) yet was still supplied.
    #[tokio::test]
    async fn non_gateway_params_nested_inside_args_are_rejected_before_the_gate() {
        // A gate that never initialises: status would still RETURN an envelope here, so an `Err`
        // also proves the rescue runs ahead of the init gate.
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);
        const NESTED: &str = "Gateway params were nested inside `args`; pass them top-level (for example, mcp({ search: \"...\" }) or mcp({ tool: \"...\", args: {} })).";

        for params in [
            json!({"args": "{\"query\":\"screenshot\"}"}),
            json!({"args": {}}),
            // `parseArgs("")` yields nothing, but `params.args !== undefined` still holds.
            json!({"args": ""}),
        ] {
            let error = tool
                .execute(
                    ToolCallId::from("call-1"),
                    params.clone(),
                    CancelToken::new(),
                    Box::new(|_| {}),
                )
                .await
                .expect_err("a modeless `args` is thrown, never answered with status");
            assert_eq!(error.message, NESTED, "{params}");
        }

        // No `args` at all is NOT the nested case — it is plain status, which the dead gate reports.
        let status = tool
            .execute(
                ToolCallId::from("call-2"),
                json!({}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a bare call is status, not the nested error");
        assert_eq!(
            status.details.expect("details")["error"],
            json!("not_initialized")
        );
    }

    /// `index.ts:880-882` — `parseArgs(null)` is NOT `parseArgs(undefined)`. `typeof null ===
    /// "object"` is why upstream spells the null test separately, and `1bf3671`'s
    /// `params.args !== undefined` arm (`index.ts:903`) makes the distinction load-bearing a second
    /// time. Serde maps a present `null` onto `None`, so without
    /// [`super::present_value`] both arms would miss and the call would answer with a status
    /// envelope instead of throwing.
    #[tokio::test]
    async fn an_explicit_null_args_is_thrown_where_an_absent_args_is_status() {
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);

        // Modeless AND with a gateway mode: `parseArgs` runs first either way, so both throw the
        // args sentence rather than the nested one or a status envelope.
        for params in [
            json!({"args": null}),
            json!({"tool": "demo_run", "args": null}),
        ] {
            let error = tool
                .execute(
                    ToolCallId::from("call-null"),
                    params.clone(),
                    CancelToken::new(),
                    Box::new(|_| {}),
                )
                .await
                .expect_err("an explicit null `args` throws");
            assert_eq!(
                error.message, "Invalid args: expected a JSON object, got null",
                "{params}"
            );
        }

        // The absent key still reaches status — the two are distinguished, not merged the other way.
        let status = tool
            .execute(
                ToolCallId::from("call-absent"),
                json!({}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("an absent `args` is status");
        assert_eq!(
            status.details.expect("details")["error"],
            json!("not_initialized")
        );

        // And the mapping is exactly "present or not" at the serde layer.
        let present: McpToolParams =
            serde_json::from_value(json!({"args": null})).expect("null is a valid args value");
        assert_eq!(present.args, Some(Value::Null));
        let absent: McpToolParams = serde_json::from_value(json!({})).expect("args is optional");
        assert_eq!(absent.args, None);
    }
}
