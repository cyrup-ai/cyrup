//! The subagent-result relay body formatter — the cyrup analog of `pi-intercom`'s pre-formatted
//! `{to, message}` relay payload (`index.ts:969-1027`; the port doc §8.3).
//!
//! pi relays a **pre-formatted string**; cyrup's [`DeliveryChannel`](cyrup_ext_subagents::tui::intercom::DeliveryChannel)
//! seam instead receives a structured, allowlisted
//! [`IntercomPayload`](cyrup_ext_subagents::tui::intercom::IntercomPayload). [`format_result_relay`]
//! projects ONLY that payload's named, allowlisted fields into the relay string — it never widens
//! the allowlist (R-SA-124): there is no field on `IntercomPayload` for a `cwd`/`session_file`/
//! capability route, so none can appear in the relayed text.

use cyrup_ext_subagents::tui::intercom::IntercomPayload;

/// Format an allowlisted subagent-result payload into the human-readable relay body the
/// [`crate::seams::IntercomDeliveryChannel`] sends to the supervisor over the broker. Reads only
/// `agent`/`success`/`outputs`/`total_tokens`/`run_id` — the allowlist is preserved by construction.
#[must_use]
pub fn format_result_relay(payload: &IntercomPayload) -> String {
    let status = if payload.success { "succeeded" } else { "failed" };
    let mut lines = vec![
        format!("Subagent run {} ({}) {}.", payload.run_id.as_str(), payload.agent, status),
        format!("Total tokens: {}", payload.total_tokens),
    ];
    if !payload.outputs.is_empty() {
        lines.push(String::new());
        lines.push("Outputs:".to_string());
        for (i, out) in payload.outputs.iter().enumerate() {
            lines.push(format!("[{i}] {out}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use cyrup_ext_subagents::background::RunId;
    use cyrup_ext_subagents::tui::intercom::SubagentResultStatus;

    #[test]
    fn formats_allowlisted_fields_only() {
        let payload = IntercomPayload {
            run_id: RunId::from_token("run00000000000001"),
            agent: "researcher".to_string(),
            success: true,
            outputs: vec!["found the answer".to_string()],
            total_tokens: 1234,
            status: SubagentResultStatus::Completed,
            summary: "1 completed".to_string(),
            child_statuses: vec![SubagentResultStatus::Completed],
        };
        let text = format_result_relay(&payload);
        assert!(text.contains("run00000000000001"));
        assert!(text.contains("researcher"));
        assert!(text.contains("succeeded"));
        assert!(text.contains("Total tokens: 1234"));
        assert!(text.contains("found the answer"));
    }

    #[test]
    fn failed_run_reads_failed() {
        let payload = IntercomPayload {
            run_id: RunId::from_token("run00000000000002"),
            agent: "worker".to_string(),
            success: false,
            outputs: vec![],
            total_tokens: 0,
            status: SubagentResultStatus::Failed,
            summary: "1 failed".to_string(),
            child_statuses: vec![SubagentResultStatus::Failed],
        };
        assert!(format_result_relay(&payload).contains("failed"));
    }
}
