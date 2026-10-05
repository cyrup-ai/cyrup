//! The `codemode` tool: the model writes JavaScript that calls other tools (pi
//! `extensions/codemode/tool.ts` + `execute.ts` @v1.0.1, CODE-007 … CODE-012).
//!
//! Scripts use `tools`, `ALL_TOOLS`, `text()`, `image()`, `exit()`, `store()`/`load()`,
//! `console.*` and `return <value>`, may start with a `// @options:` line, and reach the model
//! catalog, classifiers and image models through `models.*`. Results start with a "Script
//! completed" or "Script failed" header.
//!
//! Scripts can call the agent loop's nested tools: the active `direct` tools and every `codemode`
//! or `deferred` tool. Nested calls run through the agent loop's tool pipeline
//! ([`CodemodeHost::execute_nested`]), so validation, `tool_call`/`tool_result` hooks and
//! permission checks apply exactly as for direct calls. Only the script's output reaches the model;
//! nested results do not.
//!
//! Nested results are handed to the script as follows:
//! * A tool that declares an output schema resolves to its `structuredContent`, also for error
//!   results that carry one (MCP tools resolve to their `CallToolResult`, including `isError`).
//! * Any other tool resolves to its text content as one string.
//! * A failed, blocked or invalid call rejects with an Error carrying the tool's error text.
//!
//! A script that fails returns a normal error result that keeps its partial output, followed by
//! "Script error:" and the error. `store(key, value)` and `load(key)` keep JSON values across calls;
//! successful scripts append their writes to the session as `codemode-store` custom entries, so each
//! branch sees the values written on its own path.
//!
//! | module | upstream | ledger |
//! |---|---|---|
//! | `description` | `tool.ts:131-326` | CODE-007 |
//! | `loadout` | `tool.ts:324-359` | CODE-007 |
//! | `execute` | `execute.ts:300-433` | CODE-009, CODE-012 |
//! | `discovery` | `execute.ts:437-517` | CODE-010 |
//! | `models` | `execute.ts:524-630` | CODE-008 |
//! | `store` | `tool.ts:53`, `execute.ts:205-228` | CODE-009 |
//! | `host`, `factory` | `ExtensionToolContext`, `new CodemodeSandbox` | — |

pub mod description;
pub mod discovery;
pub mod execute;
pub mod factory;
mod globals;
pub mod host;
pub mod loadout;
pub mod models;
pub mod recorder;
pub mod store;

use std::sync::{Arc, LazyLock};

use cyrup_codemode::source::CODEMODE_SOURCE_GRAMMAR;
use cyrup_config::CodemodeMode;
use cyrup_core::{
    CancelToken, ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants, LoadoutView,
    Tool, ToolCallId, ToolError, ToolExposure, ToolLoadoutChanges, ToolResult, ToolUpdateSink,
};
use serde_json::{Value, json};

pub use description::{
    DescriptionOptions, callable_tools, create_codemode_description, describe_script_call,
    to_codemode_declaration,
};
pub use factory::{SandboxFactory, SandboxUnavailable, UnavailableSandboxFactory};
pub use host::{CodemodeHost, CodemodeHostSlot, NestedOutcome, StoreAppendFailed};
pub use models::CodemodeModels;
pub use store::{
    BranchCustomEntry, CODEMODE_STORE_ENTRY_TYPE, CodemodeStoreEntryData, read_codemode_store,
};

/// `CODEMODE_TOOL_NAME` (`tool.ts:50`).
pub const CODEMODE_TOOL_NAME: &str = "codemode";

/// Where the model finds the script reference: globals, tool results, `store()`, the `models` API
/// and limits (`CODEMODE_DOCS_PATH`, `tool.ts:116`). Shown relative to the docs directory, as the
/// other doc pointers of this port are (`cyrup-session-svc/src/auth_guidance.rs`).
pub const CODEMODE_DOCS_PATH: &str = "docs/codemode.md";

/// The prompt snippet and guidelines the tool contributes (`codemodeToolSystemPromptContribution`,
/// `tool.ts:104-109`).
pub const PROMPT_SNIPPET: &str = "Run JavaScript that calls other tools";
pub const PROMPT_GUIDELINES: [&str; 1] = [
    "Use codemode to batch independent tool calls (Promise.allSettled), chain them, or filter large output, instead of many separate calls.",
];

/// The tool's parameter schema (`codemodeSchema`, `tool.ts:90-94`): `{ code: string }`.
///
/// One `static`, so [`is_codemode_tool`] can tell this tool from another extension's tool of the
/// same name by identity, as upstream does by comparing the schema object.
#[must_use]
pub fn codemode_schema() -> &'static Value {
    static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
        json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "Raw JavaScript source." }
            },
            "required": ["code"]
        })
    });
    &SCHEMA
}

/// Whether a registered tool is this crate's `codemode` tool rather than another extension's tool
/// with the same name (`isCodemodeTool`, `tool.ts:98-100`). Compares the parameter schema by
/// identity, which every wrapper of the tool passes through by reference.
///
/// # Production call path
///
/// Upstream's caller is the MCP extension, which decides whether to activate codemode when MCP
/// tools are only reachable from scripts (`extensions/mcp/index.ts:478,1081`). cyrup's MCP adapter
/// does not port that decision yet, so this has no production caller until it does.
#[must_use]
pub fn is_codemode_tool(tool: &dyn Tool) -> bool {
    tool.name() == CODEMODE_TOOL_NAME && std::ptr::eq(tool.parameters(), codemode_schema())
}

/// How one nested call ended, as the live details list reports it
/// (`CodemodeNestedCallStatus`, `tool.ts:102`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodemodeNestedCallStatus {
    Running,
    Ok,
    Error,
    Cancelled,
}

/// One nested call in the result's details (`CodemodeNestedCall`, `tool.ts:104-118`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodemodeNestedCall {
    /// Tool call id of the nested call, `<codemode call id>/<n>`.
    pub id: String,
    pub name: String,
    /// Compact JSON of the arguments, truncated for display.
    pub args: String,
    pub status: CodemodeNestedCallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    /// Error text, truncated for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Cost in USD of a `models.*` call that reported usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

/// The tool result's `details` (`CodemodeToolDetails`, `tool.ts:120-124`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodemodeToolDetails {
    pub calls: Vec<CodemodeNestedCall>,
    /// Temp file with the full text output, when the output was truncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
}

/// What configures a [`CodemodeTool`] (`CodemodeToolOptions`, `tool.ts:76-89`, plus the sandbox
/// factory and docs pointer that upstream reaches through imports).
#[derive(Clone)]
pub struct CodemodeToolOptions {
    /// The session the tool runs in. Empty is "no session context".
    pub host: CodemodeHostSlot,
    pub sandboxes: Arc<dyn SandboxFactory>,
    /// Expose the `models` namespace to scripts when the host has model access. Without model
    /// access, `models` is not declared.
    pub models: bool,
    /// Overrides the host's `codemode.mode`.
    pub mode: Option<CodemodeMode>,
    /// Overrides the host's `codemode.inlineBudget`.
    pub inline_budget: Option<f64>,
    /// The docs the description and `models` errors point the model to.
    pub docs_path: String,
}

impl CodemodeToolOptions {
    #[must_use]
    pub fn new(host: CodemodeHostSlot, sandboxes: Arc<dyn SandboxFactory>) -> Self {
        Self {
            host,
            sandboxes,
            models: true,
            mode: None,
            inline_budget: None,
            docs_path: CODEMODE_DOCS_PATH.to_owned(),
        }
    }

    /// `options.mode ?? readMode(pi)` (`index.ts:38`): the override, else the host's setting,
    /// else `on`.
    #[must_use]
    pub fn effective_mode(&self) -> CodemodeMode {
        self.mode.unwrap_or_else(|| {
            self.host
                .current()
                .map(|host| host.mode())
                .unwrap_or_default()
        })
    }

    /// `options.inlineBudget ?? readInlineBudget(pi)` (`index.ts:39`), defaulted to 3000.
    #[must_use]
    pub fn effective_inline_budget(&self) -> f64 {
        self.inline_budget.unwrap_or_else(|| {
            self.host
                .current()
                .map_or(cyrup_config::DEFAULT_CODEMODE_INLINE_BUDGET, |host| {
                    host.inline_budget()
                })
        })
    }

    /// Whether `models` is declared: asked for, and the host has model access.
    #[must_use]
    pub fn declares_models(&self) -> bool {
        self.models
            && self
                .host
                .current()
                .is_some_and(|host| host.has_model_access())
    }
}

/// The `codemode` tool (`createCodemodeToolDefinition`, `tool.ts:361-382`).
pub struct CodemodeTool {
    options: CodemodeToolOptions,
    /// Replaced with the declarations of the callable tools when the tool is activated
    /// ([`Tool::prepare_loadout`]).
    description: String,
    sampling: ConstrainedSampling,
}

impl CodemodeTool {
    #[must_use]
    pub fn new(options: CodemodeToolOptions) -> Self {
        let description = create_codemode_description(
            &[],
            &DescriptionOptions {
                models: options.models,
                namespaces: &std::collections::BTreeMap::new(),
                deferred: &std::collections::BTreeSet::new(),
                inline_budget: None,
                docs_path: &options.docs_path,
            },
        );
        Self {
            options,
            description,
            // Capable models write the script as raw text instead of a JSON-escaped string.
            sampling: ConstrainedSampling::Config(ConstrainedSamplingConfig::Grammar {
                variants: GrammarVariants {
                    openai_lark: Some(CODEMODE_SOURCE_GRAMMAR.to_owned()),
                    openai_regex: None,
                },
            }),
        }
    }
}

#[async_trait::async_trait]
impl Tool for CodemodeTool {
    fn name(&self) -> &str {
        CODEMODE_TOOL_NAME
    }

    fn parameters(&self) -> &Value {
        codemode_schema()
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn label(&self) -> Option<&str> {
        Some(CODEMODE_TOOL_NAME)
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(PROMPT_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Vec<&str> {
        PROMPT_GUIDELINES.to_vec()
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        Some(&self.sampling)
    }

    /// Scripts must not start other scripts (`tool.ts:371`).
    fn exposure(&self) -> ToolExposure {
        ToolExposure::ModelOnly
    }

    /// Registered inactive (`index.ts:42`): activate it with `--tools`, the `defaultTools` setting,
    /// or `set_active_tools_by_name`.
    fn default_active(&self) -> bool {
        false
    }

    fn prepare_loadout(&self, view: &LoadoutView<'_>) -> Result<ToolLoadoutChanges, ToolError> {
        Ok(loadout::prepare_codemode_loadout(view, &self.options))
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let code = params
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("codemode expects { code: string }"))?;
        execute::execute_codemode(&call_id, code, cancel, on_update, &self.options).await
    }
}

#[cfg(test)]
mod tests;
