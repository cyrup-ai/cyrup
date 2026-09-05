//! Typed `details` payloads (arch-03 §3.2). Serialized into `ToolResult.details`; camelCase for
//! Pi-interop. Not shown to the model — a structured side-channel for the UI/agent.

use crate::truncate::Truncation;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReadDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<Truncation>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BashDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<Truncation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    /// The process exit code, on the non-zero-exit path only (`ACP-141`).
    ///
    /// Pi has no counterpart: its bash tool throws a string and the exit code survives only inside
    /// `Command exited with code {n}`. Every front-end that wants the number then has to parse it
    /// back out of a human-readable sentence — which is what pi-acp's `bashExitCode` does through
    /// a four-key `Record<string, unknown>` probe that hits nothing, and why an ACP client showed
    /// `terminal_exit.exit_code: 1` for `sh -c 'exit 42'`.
    ///
    /// Absent (not `null`) on every other path: a clean exit reports through the ordinary success
    /// result, and a timeout or a kill has no exit code to report. Carried to the client by
    /// `cyrup_acp::translate::bash_exit_code`, whose probe reads `details.exitCode` first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct EditDetails {
    pub diff: String,
    pub patch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_changed_line: Option<usize>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrepDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<Truncation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_limit_reached: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines_truncated: Option<bool>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FindDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<Truncation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_limit_reached: Option<usize>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LsDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<Truncation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_limit_reached: Option<usize>,
}

// NOTE: `write` has no `details` payload. Pi declares `ToolDefinition<…, undefined>` and returns
// `details: undefined` (write.ts:223), so there is intentionally no `WriteDetails` type here.
