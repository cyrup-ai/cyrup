//! SUBA-104 — management `list`'s `capabilities: true` mode, end to end through the real
//! `subagent` tool entry (`SubagentTool::execute` → `route_management_action` →
//! `handle_management_action_with` → `handle_list`), the same path every management action a
//! model sends takes.
//!
//! Upstream: `handleList` / `formatAgentCapabilitiesLine` / `agentCapabilityRow`
//! (`agents/agent-management.ts:735-850,971-1005` @v0.68.0), row types `shared/types.ts:1373-1395`,
//! schema `extension/schemas.ts:287`; the cases mirror `test/unit/agent-management.test.ts:70-340`.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::extension::testsupport::{FixedSessionHost, dispatch_tool, scoped_tool, tool_text};

fn write_agent(dir: &Path, name: &str, frontmatter: &str, body: &str) {
    let agents = dir.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents).expect("mkdir agents");
    std::fs::write(
        agents.join(format!("{name}.md")),
        format!("---\nname: {name}\n{frontmatter}\n---\n{body}\n"),
    )
    .expect("write agent");
}

/// `{ action: "list", agentScope: "project", capabilities: true }` through the tool, returning
/// `(text, details)`.
async fn capability_list(dir: &Path) -> (String, Value) {
    let tool = scoped_tool(dir).await;
    list_with(
        &tool,
        json!({ "action": "list", "agentScope": "project", "capabilities": true }),
    )
    .await
}

async fn list_with(tool: &crate::extension::SubagentTool, params: Value) -> (String, Value) {
    let out = dispatch_tool(tool, params).await.expect("list succeeds");
    let details = out.details.clone().expect("management details");
    (tool_text(&out), details)
}

fn row<'a>(details: &'a Value, name: &str) -> &'a Value {
    details["agentCapabilities"]["agents"]
        .as_array()
        .expect("agentCapabilities.agents")
        .iter()
        .find(|row| row["name"] == json!(name))
        .unwrap_or_else(|| panic!("no capability row for {name}: {details}"))
}

fn line<'a>(text: &'a str, name: &str) -> &'a str {
    let prefix = format!("- {name} (");
    let rows: Vec<&str> = text.lines().filter(|l| l.starts_with(&prefix)).collect();
    assert_eq!(rows.len(), 1, "exactly one row for {name}:\n{text}");
    rows[0]
}

/// pi "lists compact declared capabilities without exposing system prompts"
/// (`agent-management.test.ts:70-146`): the full line, every structured field, the section
/// layout, and no system prompt in either half — including upstream's `acceptance.report: on`,
/// which reaches the summary as the trailing `report: on` modifier (SUBA-105).
#[tokio::test]
async fn capability_list_renders_upstream_rows_without_system_prompts() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "capability-worker",
        concat!(
            "description: Capability worker\n",
            "aliases: capability\n",
            "tools: read, grep, mcp:github/search\n",
            "model: openai/gpt-5-mini\n",
            "async: true\n",
            "timeoutMs: 123\n",
            "thinking: high\n",
            "acceptance: {\"level\":\"checked\",\"report\":\"on\",\"evidence\":[\"changed-files\",\"commands-run\"],",
            "\"verify\":[{\"id\":\"unit\",\"command\":\"npm test\"}],\"criteria\":[\"Patch the bug\",\"Keep the diff narrow\"],",
            "\"stopRules\":[\"Do not stop after analysis\"],\"review\":{\"agent\":\"reviewer\"}}\n",
            "acceptanceRole: writer\n",
            "skills: typescript-code\n",
            "extensions: github\n",
            "subagentOnlyExtensions: surf\n",
            "mutationTools: edit, write\n",
            "output: report.md\n",
            "outputMode: file-only\n",
            "machine: workmac",
        ),
        "SYSTEM_PROMPT_SENTINEL",
    );

    let (text, details) = capability_list(dir.path()).await;

    assert!(
        text.starts_with("Executable agents (capabilities):\nProject agents\n"),
        "header then upstream's source sections:\n{text}"
    );
    assert!(
        text.contains("\n\nBuiltin agents\n"),
        "each further section is preceded by a blank line:\n{text}"
    );
    assert!(
        !text.contains("\nChains:"),
        "v0.68.0's listing carries no chain block:\n{text}"
    );
    assert_eq!(
        line(&text, "capability-worker"),
        "- capability-worker (project, machine: workmac (saved Herdr placement), aliases: capability): \
         Description: Capability worker; Tools: read, grep, mcp:github/search; Model: openai/gpt-5-mini; \
         Thinking: high; Machine: workmac (saved Herdr placement); Acceptance: checked (changed-files, \
         commands-run, verify: \"unit\", criteria: 2, stopRules: 1, review: required by \"reviewer\", \
         report: on); Acceptance role: writer"
    );
    assert!(!text.contains("SYSTEM_PROMPT_SENTINEL"), "{text}");
    assert!(!details.to_string().contains("SYSTEM_PROMPT_SENTINEL"));

    assert_eq!(details["mode"], json!("management"));
    assert_eq!(details["results"], json!([]));
    assert_eq!(details["agentCapabilities"]["restrictedCount"], json!(0));
    assert!(
        details["agentCapabilities"]
            .get("capabilityCeilingSources")
            .is_none(),
        "no ceiling, no sources key: {details}"
    );

    let worker = row(&details, "capability-worker");
    assert_eq!(
        *worker,
        json!({
            "name": "capability-worker",
            "description": "Capability worker",
            "source": "project",
            "executable": true,
            "aliases": ["capability"],
            "runner": { "type": "pi" },
            "tools": {
                "ambient": false,
                "names": ["read", "grep"],
                "mcpDirectTools": ["github/search"],
                "mutationTools": ["edit", "write"],
            },
            "model": { "value": "openai/gpt-5-mini", "thinking": "high" },
            "execution": { "defaultAsync": true, "timeoutMs": 123 },
            "acceptance": {
                "policy": {
                    "level": "checked",
                    "report": "on",
                    "evidence": ["changed-files", "commands-run"],
                    "verify": [{ "id": "unit", "command": "npm test" }],
                    "criteria": ["Patch the bug", "Keep the diff narrow"],
                    "stopRules": ["Do not stop after analysis"],
                    "review": { "agent": "reviewer" },
                },
                "role": "writer",
            },
            "output": { "path": "report.md", "mode": "file-only" },
            "extensions": { "names": ["github"], "subagentOnly": ["surf"], "skills": ["typescript-code"] },
        })
    );
}

/// An agent with no `tools:` key is `default/ambient` / `ambient: true`; declared tools with an
/// exclusion render `; excludes: …` and carry `excludeTools`; no model/thinking/acceptance renders
/// upstream's defaults and omits the empty detail objects (`presentDetails`).
#[tokio::test]
async fn capability_rows_report_ambient_and_excluded_tool_surfaces() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "ambient-worker",
        "description: Ambient",
        "Body.",
    );
    write_agent(
        dir.path(),
        "excluding-worker",
        "description: Excluding\ntools: read, bash\nexcludeTools: bash",
        "Body.",
    );

    let (text, details) = capability_list(dir.path()).await;

    assert_eq!(
        line(&text, "ambient-worker"),
        "- ambient-worker (project): Description: Ambient; Tools: default/ambient; \
         Model: inherits current session; Thinking: default"
    );
    assert_eq!(
        *row(&details, "ambient-worker"),
        json!({
            "name": "ambient-worker",
            "description": "Ambient",
            "source": "project",
            "executable": true,
            "runner": { "type": "pi" },
            "tools": { "ambient": true, "names": [], "mcpDirectTools": [] },
        })
    );
    assert!(
        line(&text, "excluding-worker").contains("; Tools: read, bash; excludes: bash; "),
        "{text}"
    );
    assert_eq!(
        row(&details, "excluding-worker")["tools"],
        json!({ "ambient": false, "names": ["read", "bash"], "excludeTools": ["bash"], "mcpDirectTools": [] })
    );
}

/// `thinking: false` is upstream's boolean off (`agents.ts:2186`): `Thinking: off` and
/// `model.thinking: false`.
#[tokio::test]
async fn capability_rows_render_thinking_false_as_off() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "quiet-worker",
        "description: Quiet\nthinking: false",
        "Body.",
    );

    let (text, details) = capability_list(dir.path()).await;

    assert!(
        line(&text, "quiet-worker").ends_with("; Thinking: off"),
        "{text}"
    );
    assert_eq!(
        row(&details, "quiet-worker")["model"],
        json!({ "thinking": false })
    );
}

/// A bare model is shown with the agent's provider (`agent-management.ts:745-748`): here the
/// provider comes from the project's `subagents.defaultProvider`, the only producer of
/// `modelProvider` cyrup has. The structured `model.value` stays the declared model.
#[tokio::test]
async fn capability_line_prefixes_a_bare_model_with_its_provider() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "bare-model",
        "description: Bare model\nmodel: gpt-5-mini",
        "Body.",
    );
    std::fs::write(
        dir.path()
            .join(".cyrup")
            .join("agents")
            .join("settings.json"),
        r#"{ "subagents": { "defaultProvider": "openai" } }"#,
    )
    .expect("write settings");

    let (text, details) = capability_list(dir.path()).await;

    assert!(
        line(&text, "bare-model").contains("; Model: openai/gpt-5-mini; "),
        "{text}"
    );
    assert_eq!(
        row(&details, "bare-model")["model"],
        json!({ "value": "gpt-5-mini" })
    );
}

/// pi "reports bare and disabled acceptance declarations in capabilities"
/// (`agent-management.test.ts:164-183`).
#[tokio::test]
async fn capability_rows_report_bare_and_disabled_acceptance() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "accepted-worker",
        "description: Accepted worker\nacceptance: checked",
        "Accepted.",
    );
    write_agent(
        dir.path(),
        "open-worker",
        "description: Open worker\nacceptance: false",
        "Open.",
    );

    let (text, details) = capability_list(dir.path()).await;

    assert!(
        line(&text, "accepted-worker").ends_with("; Acceptance: checked"),
        "{text}"
    );
    assert!(
        line(&text, "open-worker").ends_with("; Acceptance: disabled"),
        "{text}"
    );
    assert_eq!(
        row(&details, "accepted-worker")["acceptance"],
        json!({ "policy": "checked" })
    );
    assert_eq!(
        row(&details, "open-worker")["acceptance"],
        json!({ "policy": false })
    );
}

/// pi "reports optional review gates as optional in capabilities"
/// (`agent-management.test.ts:185-206`), plus the `review: false` → `review: off` arm.
#[tokio::test]
async fn capability_rows_report_optional_and_disabled_review_gates() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "optional-review",
        "description: Optional review\nacceptance: {\"level\":\"checked\",\"review\":{\"required\":false}}",
        "Optional.",
    );
    write_agent(
        dir.path(),
        "named-optional-review",
        "description: Named optional review\nacceptance: {\"level\":\"checked\",\"review\":{\"agent\":\"reviewer\",\"required\":false}}",
        "Optional.",
    );
    write_agent(
        dir.path(),
        "no-review",
        "description: No review\nacceptance: {\"level\":\"checked\",\"review\":false}",
        "Off.",
    );

    let (text, details) = capability_list(dir.path()).await;

    assert!(
        line(&text, "optional-review").ends_with("; Acceptance: checked (review: optional)"),
        "{text}"
    );
    assert!(
        line(&text, "named-optional-review")
            .ends_with("; Acceptance: checked (review: optional by \"reviewer\")"),
        "{text}"
    );
    assert!(
        line(&text, "no-review").ends_with("; Acceptance: checked (review: off)"),
        "{text}"
    );
    assert_eq!(
        row(&details, "named-optional-review")["acceptance"],
        json!({ "policy": { "level": "checked", "review": { "agent": "reviewer", "required": false } } })
    );
}

/// SUBA-105 — `formatAcceptanceSummary`'s `if (policy.report) modifiers.push(`report: ${policy.report}`)`
/// (`agent-management.ts:777` @v0.68.0) for BOTH values, after every other modifier, and a
/// `report`-only policy renders at `auto`. A malformed value is refused at discovery with the
/// validator's own text rather than listed.
#[tokio::test]
async fn capability_rows_report_the_acceptance_report_toggle() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "report-off",
        "description: Report off\nacceptance: {\"level\":\"checked\",\"review\":false,\"report\":\"off\"}",
        "Off.",
    );
    write_agent(
        dir.path(),
        "report-only",
        "description: Report only\nacceptance: {\"report\":\"on\"}",
        "On.",
    );
    write_agent(
        dir.path(),
        "report-bogus",
        "description: Report bogus\nacceptance: {\"level\":\"checked\",\"report\":\"yes\"}",
        "Bogus.",
    );

    let (text, details) = capability_list(dir.path()).await;

    assert!(
        line(&text, "report-off").ends_with("; Acceptance: checked (review: off, report: off)"),
        "{text}"
    );
    assert!(
        line(&text, "report-only").ends_with("; Acceptance: auto (report: on)"),
        "{text}"
    );
    assert_eq!(
        row(&details, "report-only")["acceptance"],
        json!({ "policy": { "report": "on" } })
    );
    assert!(
        !details["agentCapabilities"]["agents"]
            .as_array()
            .expect("agents")
            .iter()
            .any(|row| row["name"] == json!("report-bogus")),
        "a malformed report value is not listed as an agent: {details}"
    );
    assert!(
        text.contains(
            "\nInvalid agent definitions:\n- report-bogus (project): Agent 'report-bogus' \
             acceptance frontmatter.report must be on or off."
        ),
        "the refusal is reported as a diagnostic:\n{text}"
    );
}

/// pi "safely bounds acceptance labels in capability summaries without changing structured
/// values" (`agent-management.test.ts:208-240`).
#[tokio::test]
async fn capability_summaries_bound_acceptance_labels_but_keep_structured_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let verify_id = format!(
        "unit, ); Acceptance role: forged\n- forged-verify\u{0007}{}",
        "v".repeat(100)
    );
    let review_agent = format!(
        "reviewer; report: forged\n- forged-review\u{001b}[31m{}",
        "r".repeat(100)
    );
    let policy = json!({ "level": "checked", "verify": [{ "id": verify_id, "command": "true" }], "review": { "agent": review_agent } });
    write_agent(
        dir.path(),
        "unsafe-acceptance",
        &format!("description: Unsafe acceptance labels\nacceptance: {policy}"),
        "Unsafe labels.",
    );

    let (text, details) = capability_list(dir.path()).await;

    let row_text = line(&text, "unsafe-acceptance");
    let verify_label = format!(
        "verify: \"unit, ); Acceptance role: forged - forged-verify {}...\"",
        "v".repeat(80 - 3 - "unit, ); Acceptance role: forged - forged-verify ".len())
    );
    assert!(row_text.contains(&verify_label), "{row_text}");
    assert!(
        row_text.contains("review: required by \"reviewer; report: forged - forged-review "),
        "{row_text}"
    );
    assert!(
        !text
            .chars()
            .any(|c| (c.is_control() && c != '\n') || c == '\u{001b}'),
        "no control characters survive into the text: {text:?}"
    );
    assert!(!text.contains("\n- forged-"), "{text}");
    assert_eq!(
        row(&details, "unsafe-acceptance")["acceptance"],
        json!({ "policy": policy })
    );
}

/// Descriptions are previewed: 240 UTF-16 units in the line, 1000 in the row, both sanitized.
#[tokio::test]
async fn capability_descriptions_are_previewed_for_line_and_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let long = "d".repeat(1200);
    write_agent(
        dir.path(),
        "wordy-worker",
        &format!("description: {long}"),
        "Body.",
    );

    let (text, details) = capability_list(dir.path()).await;

    assert!(
        line(&text, "wordy-worker")
            .contains(&format!("Description: {}...; Tools:", "d".repeat(237))),
        "{text}"
    );
    assert_eq!(
        row(&details, "wordy-worker")["description"],
        json!(format!("{}...", "d".repeat(997)))
    );
}

/// pi "reports passive external CLI availability for present and absent commands"
/// (`agent-management.test.ts:242-297`): a PATH lookup only, the reason bounded, and the
/// code-owned capability envelope on the row.
#[tokio::test]
async fn capability_rows_report_passive_external_cli_availability() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "present-external",
        "description: Present external CLI\nrunner: {\"type\":\"external-cli\",\"command\":\"sh\"}",
        "Present.",
    );
    write_agent(
        dir.path(),
        "missing-external",
        "description: Missing external CLI\nrunner: {\"type\":\"external-cli\",\"command\":\"cyrup-suba104-missing-cli\"}",
        "Missing.",
    );

    let (text, details) = capability_list(dir.path()).await;

    assert!(
        line(&text, "present-external")
            .starts_with("- present-external (project, external-cli:sh ✓): "),
        "{text}"
    );
    assert!(
        line(&text, "missing-external").starts_with(
            "- missing-external (project, external-cli:cyrup-suba104-missing-cli missing): "
        ),
        "{text}"
    );
    let envelope = serde_json::to_value(crate::runner::status::ExternalCliCapabilities::default())
        .expect("serializes");
    assert_eq!(
        row(&details, "present-external")["runner"],
        json!({ "type": "external-cli", "command": "sh", "available": true, "capabilities": envelope })
    );
    assert_eq!(
        row(&details, "missing-external")["runner"],
        json!({
            "type": "external-cli",
            "command": "cyrup-suba104-missing-cli",
            "available": false,
            "unavailableReason": "External CLI binary 'cyrup-suba104-missing-cli' was not found on PATH.",
            "capabilities": envelope,
        })
    );
    assert_eq!(row(&details, "missing-external")["executable"], json!(true));
}

/// pi "does not preflight Herdr machines while listing capabilities"
/// (`agent-management.test.ts:299-335`): a placed agent checks only local ssh, and its row carries
/// the adapter and the machine.
#[tokio::test]
async fn capability_rows_do_not_preflight_herdr_machines() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "remote-external",
        "description: Remote external CLI\nmachine: workmac\nrunner: {\"type\":\"external-cli\",\"adapter\":\"codex-exec\",\"command\":\"codex\"}",
        "Remote.",
    );

    let (text, details) = capability_list(dir.path()).await;

    let ssh = crate::placement::resolve::ssh_transport_available();
    let mark = if ssh { "✓" } else { "missing" };
    assert!(
        line(&text, "remote-external").starts_with(&format!(
            "- remote-external (project, external-cli:codex @ workmac saved Herdr placement; transport {mark}; machine not preflighted): "
        )),
        "{text}"
    );
    let runner = &row(&details, "remote-external")["runner"];
    assert_eq!(runner["type"], json!("external-cli"));
    assert_eq!(runner["adapter"], json!("codex-exec"));
    assert_eq!(runner["command"], json!("codex"));
    assert_eq!(runner["machine"], json!("workmac"));
    assert_eq!(runner["available"], json!(ssh));
}

/// An `external-job` agent: cyrup registers no providers, so the registry is empty — upstream's
/// answer when nothing is registered: `missing` and `available: false`, with the five-`false`
/// capability envelope (`EXTERNAL_JOB_CAPABILITIES`, `agent-management.ts:786`).
#[tokio::test]
async fn capability_rows_report_unregistered_external_job_providers() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "job-worker",
        "description: Job worker\nrunner: {\"type\":\"external-job\",\"provider\":\"acme\"}",
        "Job.",
    );

    let (text, details) = capability_list(dir.path()).await;

    assert!(
        line(&text, "job-worker")
            .starts_with("- job-worker (project, external-job:acme missing): "),
        "{text}"
    );
    assert_eq!(
        row(&details, "job-worker")["runner"],
        json!({
            "type": "external-job",
            "provider": "acme",
            "available": false,
            "capabilities": { "stop": false, "steer": false, "resume": false, "structuredOutput": false, "toolEvents": false },
        })
    );
}

/// The session's capability ceiling (`resolveCurrentSubagentCapabilityCeiling(ctx.currentSessionId)`,
/// `agent-management.ts:975-979`): an agent outside `allowedAgents` leaves the executable
/// sections for the `Restricted agents` block, its row reads `executable: false` with the
/// ceiling's sources, and the snapshot counts it and names the sources.
#[tokio::test]
async fn capability_ceiling_moves_disallowed_agents_to_restricted_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(dir.path(), "alpha", "description: Alpha", "Body.");
    write_agent(dir.path(), "beta", "description: Beta", "Body.");
    let _ceiling = crate::exec::capability_ceiling::register_capability_ceiling(
        "suba104-capabilities",
        "test-ceiling",
        &json!({ "version": 1, "allowedAgents": ["alpha"] }),
    )
    .expect("register ceiling");
    let tool = scoped_tool(dir.path()).await;
    tool.executor()
        .set_host_services(Arc::new(FixedSessionHost("suba104-capabilities")));

    let (text, details) = list_with(
        &tool,
        json!({ "action": "list", "agentScope": "project", "capabilities": true }),
    )
    .await;

    let restricted_at = text
        .find("\n\nRestricted agents (not executable in this session; capability ceiling: test-ceiling):\n")
        .unwrap_or_else(|| panic!("restricted block missing:\n{text}"));
    let beta_at = text
        .find("- beta (project): Description: Beta;")
        .expect("beta row");
    let alpha_at = text
        .find("- alpha (project): Description: Alpha;")
        .expect("alpha row");
    assert!(
        alpha_at < restricted_at && restricted_at < beta_at,
        "{text}"
    );

    assert_eq!(row(&details, "alpha")["executable"], json!(true));
    assert!(row(&details, "alpha").get("restrictionSources").is_none());
    assert_eq!(row(&details, "beta")["executable"], json!(false));
    assert_eq!(
        row(&details, "beta")["restrictionSources"],
        json!(["test-ceiling"])
    );
    let snapshot = &details["agentCapabilities"];
    let restricted_rows = snapshot["agents"]
        .as_array()
        .expect("rows")
        .iter()
        .filter(|row| row["executable"] == json!(false))
        .count();
    assert!(restricted_rows >= 1);
    assert_eq!(snapshot["restrictedCount"], json!(restricted_rows));
    assert_eq!(
        snapshot["capabilityCeilingSources"],
        json!(["test-ceiling"])
    );
}

/// The plain listing honours the same ceiling (`appendRestrictedAgentLines` runs in both modes),
/// with the plain row format.
#[tokio::test]
async fn plain_list_splits_restricted_agents_under_the_ceiling() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(dir.path(), "alpha", "description: Alpha", "Body.");
    write_agent(dir.path(), "beta", "description: Beta", "Body.");
    let _ceiling = crate::exec::capability_ceiling::register_capability_ceiling(
        "suba104-plain",
        "plain-ceiling",
        &json!({ "version": 1, "allowedAgents": ["alpha"] }),
    )
    .expect("register ceiling");
    let tool = scoped_tool(dir.path()).await;
    tool.executor()
        .set_host_services(Arc::new(FixedSessionHost("suba104-plain")));

    let (text, details) =
        list_with(&tool, json!({ "action": "list", "agentScope": "project" })).await;

    let executable = text
        .split("\n\nRestricted agents (not executable in this session; capability ceiling: plain-ceiling):\n")
        .collect::<Vec<_>>();
    assert_eq!(executable.len(), 2, "one restricted block:\n{text}");
    assert!(
        executable[0].contains("- alpha (project, tools: (unpinned)): Alpha"),
        "{text}"
    );
    assert!(!executable[0].contains("- beta "), "{text}");
    assert!(
        executable[1].contains("- beta (project, tools: (unpinned)): Beta"),
        "{text}"
    );
    assert!(details.get("agentCapabilities").is_none());
}

/// The INHERITED ceiling (`CYRUP_SUBAGENT_CAPABILITY_CEILING_V1`, pi
/// `resolveCurrentSubagentCapabilityCeiling`'s env half, `handleList` `:975` @v0.68.0) is read
/// through the extension's env seam (`SubagentExtensionConfig::env_overrides`), the same one its
/// launches read: a malformed value refuses the listing (fail closed — upstream's decoder throws
/// out of `handleList`), a well-formed one splits the listing exactly as a registered ceiling
/// does, and a scrubbed one lists every agent as executable.
#[tokio::test]
async fn list_reads_the_inherited_capability_ceiling_through_the_env_seam() {
    use crate::exec::capability_ceiling::{
        CAPABILITY_CEILING_ENV, CAPABILITY_CEILING_VERSION, ResolvedCapabilityCeiling,
        encode_capability_ceiling,
    };
    async fn list_with_pin(
        pin: Option<String>,
    ) -> (tempfile::TempDir, crate::extension::SubagentTool) {
        let dir = tempfile::tempdir().expect("tempdir");
        write_agent(dir.path(), "alpha", "description: Alpha", "Body.");
        write_agent(dir.path(), "beta", "description: Beta", "Body.");
        let tool = scoped_tool(dir.path()).await;
        tool.executor()
            .config_cell()
            .lock()
            .await
            .env_overrides
            .insert(CAPABILITY_CEILING_ENV.to_string(), pin);
        (dir, tool)
    }
    let params = json!({ "action": "list", "agentScope": "project" });

    use base64::Engine as _;
    let wrong_version = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(br#"{"version":99,"sources":["parent"]}"#);
    for (pin, expected) in [
        (
            "%%% not base64 %%%",
            "Invalid inherited capability ceiling: ",
        ),
        (
            wrong_version.as_str(),
            "Invalid inherited capability ceiling version.",
        ),
    ] {
        let (_dir, tool) = list_with_pin(Some(pin.to_string())).await;
        let refused = dispatch_tool(&tool, params.clone())
            .await
            .expect_err("a malformed inherited ceiling must refuse the listing")
            .to_string();
        assert!(refused.contains(expected), "{refused}");
    }

    let inherited = encode_capability_ceiling(Some(&ResolvedCapabilityCeiling {
        version: CAPABILITY_CEILING_VERSION,
        allowed_tools: None,
        allowed_agents: Some(vec!["alpha".to_string()]),
        deny_extensions: false,
        sources: vec!["parent-ceiling".to_string()],
    }));
    let (_dir, tool) = list_with_pin(inherited).await;
    let (text, _details) = list_with(&tool, params.clone()).await;
    let halves = text
        .split("\n\nRestricted agents (not executable in this session; capability ceiling: parent-ceiling):\n")
        .collect::<Vec<_>>();
    assert_eq!(halves.len(), 2, "one restricted block:\n{text}");
    assert!(halves[0].contains("- alpha (project"), "{text}");
    assert!(halves[1].contains("- beta (project"), "{text}");

    let (_dir, tool) = list_with_pin(None).await;
    let (text, _details) = list_with(&tool, params).await;
    assert!(!text.contains("Restricted agents"), "{text}");
    assert!(text.contains("- beta (project"), "{text}");
}

/// `capabilities: false` (and absent) is the plain listing: upstream tests `=== true`.
#[tokio::test]
async fn capabilities_false_keeps_the_plain_listing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    let (text, details) = list_with(
        &tool,
        json!({ "action": "list", "agentScope": "project", "capabilities": false }),
    )
    .await;

    assert!(text.starts_with("Executable agents:\n"), "{text}");
    assert!(text.contains("\nChains:\n"), "{text}");
    assert_eq!(details, json!({ "mode": "management", "results": [] }));
}

/// A non-boolean `capabilities` is refused at the parse, as upstream's `Type.Boolean` schema
/// refuses `"true"` (`schemas.test.ts:313-314`).
#[tokio::test]
async fn a_non_boolean_capabilities_flag_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    let err = dispatch_tool(&tool, json!({ "action": "list", "capabilities": "true" }))
        .await
        .expect_err("a string capabilities flag is not a boolean");

    let message = err.to_string();
    assert!(
        message.contains("invalid subagent tool call") && message.contains("expected a boolean"),
        "{message}"
    );
}

/// Capability mode keeps upstream's tail: the invalid-definition diagnostics
/// (`appendAgentDiagnosticLines`, `agent-management.ts:1000`).
#[tokio::test]
async fn capability_list_keeps_invalid_agent_diagnostics() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agent(
        dir.path(),
        "broken",
        "description: Broken\ntimeoutMs: nope",
        "Body.",
    );

    let (text, _details) = capability_list(dir.path()).await;

    assert!(
        text.contains("\n\nInvalid agent definitions:\n- broken (project): "),
        "{text}"
    );
}

/// The schema advertises the key as upstream does: a boolean, description verbatim.
#[tokio::test]
async fn the_capabilities_key_is_advertised_as_upstreams_boolean() {
    use cyrup_core::Tool;
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    assert_eq!(
        tool.parameters()["properties"]["capabilities"],
        json!({ "type": "boolean", "description": "list: compact capability rows/details without system prompts." })
    );
}
