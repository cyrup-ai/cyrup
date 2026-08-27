//! Claude Code tool-name mapping (Pi anthropic-messages.ts:98-109)

use crate::auth::AuthResult;
use crate::model::Model;

/// Claude Code 2.x canonical tool names (Pi `claudeCodeTools`, anthropic-messages.ts:78-96).
const CLAUDE_CODE_TOOLS: [&str; 17] = [
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

/// Map a tool name to Claude Code canonical casing if it matches case-insensitively (Pi
/// `toClaudeCodeName`).
pub(super) fn to_claude_code_name(name: &str) -> String {
    let lower = name.to_lowercase();
    for t in CLAUDE_CODE_TOOLS {
        if t.to_lowercase() == lower {
            return t.to_string();
        }
    }
    name.to_string()
}

/// Map a Claude Code tool name back to a caller-declared tool name by case-insensitive match (Pi
/// `fromClaudeCodeName`, anthropic-messages.ts:102-109).
pub(super) fn remap_decoded_tool_name(tool_names: &[String], name: &str) -> String {
    let lower = name.to_lowercase();
    for declared in tool_names {
        if declared.to_lowercase() == lower {
            return declared.clone();
        }
    }
    name.to_string()
}

/// `apiKey.includes("sk-ant-oat")` (Pi `isOAuthToken`, anthropic-messages.ts:809-811).
pub(super) fn is_oauth_token(api_key: &str) -> bool {
    api_key.contains("sk-ant-oat")
}

/// `model.provider === "github-copilot"` — the branch Pi tests FIRST inside `createClient`
/// (anthropic-messages.ts:868). Copilot's 9 anthropic-messages rows are routed here, not through
/// the `isOAuthToken` sniff, because a Copilot token (`tid=…;exp=…;proxy-ep=…`) contains no
/// `sk-ant-oat` marker and would otherwise fall through to `x-api-key` — which Copilot's edge
/// rejects (PROV-027).
pub(super) fn is_github_copilot(model: &Model) -> bool {
    model.provider.as_str() == crate::api::github_copilot_headers::GITHUB_COPILOT_PROVIDER
}

/// The `isOAuthToken` value Pi's `createClient` RETURNS, which is what `buildParams` consumes
/// (anthropic-messages.ts:536-546, consumed by `buildParams` at `:938`). The Copilot branch returns
/// `false` unconditionally (`:887`), so Copilot never gets the Claude-Code tool-name normalization
/// even if its token happened to contain the marker; only the second branch (`:891`) reports `true`.
pub(super) fn resolve_is_oauth(model: &Model, auth: &AuthResult) -> bool {
    if is_github_copilot(model) {
        return false;
    }
    auth.auth
        .api_key
        .as_deref()
        .map(is_oauth_token)
        .unwrap_or(false)
}
