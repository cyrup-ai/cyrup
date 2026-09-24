//! SUBA-104 — management `list`'s `capabilities: true` mode (pi `handleList`'s capability branch,
//! `agent-management.ts:735-850,971-1005` @v0.68.0; row types `shared/types.ts:1373-1395`).
//!
//! With `capabilities: true` a `list` answers `Executable agents (capabilities):` with one compact,
//! prompt-free line per agent (`formatAgentCapabilitiesLine`, `:735-753`) grouped into upstream's
//! source sections (`formatAgentListSections`, `:885-905`), and attaches the machine-readable
//! snapshot as `details.agentCapabilities` (`agentCapabilitiesSnapshot`, `:838-848`): per agent its
//! runner (with passive external-cli availability), declared tools, model/thinking, execution
//! defaults, acceptance, output and extensions, plus whether the session's capability ceiling lets
//! it run. A system prompt never appears in either half.
//!
//! Pure rendering over already-discovered [`AgentDefinition`]s plus two passive filesystem looks
//! (the external-cli binary on `PATH`, and local `ssh` for a placed agent) — nothing here starts a
//! process, reads a Herdr catalog, or preflights a machine, exactly as upstream
//! (`agent-management.test.ts` "does not preflight Herdr machines while listing capabilities").

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::super::types::{AgentDefinition, AgentSource, ToolRef};
use super::helpers::{context_str, source_str};
use crate::runner::AgentRunnerConfig;
use crate::workflows::preview_display_text;

/// pi's capability-mode header (`agent-management.ts:995` @v0.68.0).
pub(crate) const CAPABILITIES_HEADER: &str = "Executable agents (capabilities):";

/// pi `MAX_AVAILABILITY_REASON_LENGTH` (`runs/shared/external-cli-preflight.ts:9` @v0.68.0) —
/// `reason.slice(0, 256)`, in UTF-16 code units like upstream's `String#slice`.
const MAX_AVAILABILITY_REASON_UTF16: usize = 256;

/// pi `ExternalCliBinaryAvailability` (`external-cli-preflight.ts:38-40` @v0.68.0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BinaryAvailability {
    Available,
    Unavailable(String),
}

impl BinaryAvailability {
    fn available(&self) -> bool {
        matches!(self, Self::Available)
    }

    fn mark(&self) -> &'static str {
        if self.available() { "✓" } else { "missing" }
    }
}

/// pi `ExternalCliAvailabilityByCommand` (`agent-management.ts:689`), keyed by
/// [`availability_key`].
pub(crate) type ExternalCliAvailability = BTreeMap<String, BinaryAvailability>;

/// pi `externalCliAvailabilityKey` (`agent-management.ts:691-694`): a placed agent checks only
/// local ssh, so every placed agent shares one `ssh@<machine>` probe.
fn availability_key(command: &str, machine: Option<&str>) -> String {
    machine.map_or_else(|| command.to_string(), |machine| format!("ssh@{machine}"))
}

fn bounded_reason(reason: &str) -> String {
    let mut units = 0usize;
    reason
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= MAX_AVAILABILITY_REASON_UTF16
        })
        .collect()
}

/// pi `resolveExternalCliBinaryAvailability(command, process.env)` (`external-cli-preflight.ts:
/// 63-71`): the passive PATH/X_OK lookup of the configured command — the same `resolveBinary` the
/// launch preflight runs ([`crate::exec::external_cli::preflight::resolve_binary`]), minus the
/// probes. It never starts a process.
fn local_binary_availability(command: &str) -> BinaryAvailability {
    let path = std::env::var("PATH").ok();
    match crate::exec::external_cli::preflight::resolve_binary(command, path.as_deref()) {
        Ok(_) => BinaryAvailability::Available,
        Err(reason) => BinaryAvailability::Unavailable(bounded_reason(&reason)),
    }
}

/// pi `resolveExternalCliBinaryAvailability("ssh", env)` for a placed agent (`agent-management.ts:
/// 702`). The verdict is [`crate::placement::resolve::ssh_transport_available`]'s — the ssh the
/// transport would actually run, on its hardened `PATH` — so this row and the plain listing's
/// badge can never disagree; the reason is `resolveBinary`'s own sentence for that same binary.
fn ssh_availability() -> BinaryAvailability {
    if crate::placement::resolve::ssh_transport_available() {
        return BinaryAvailability::Available;
    }
    let transport = cyrup_herdr::remote::SshTransport::with_env(&cyrup_herdr::env::ProcessEnv);
    let bin = transport.bin();
    let reason = crate::exec::external_cli::preflight::resolve_binary(
        bin,
        Some(cyrup_herdr::remote::HARDENED_SSH_PATH),
    )
    .err()
    .unwrap_or_else(|| format!("External CLI binary '{bin}' was not found on PATH."));
    BinaryAvailability::Unavailable(bounded_reason(&reason))
}

/// pi `externalCliAvailabilityForAgents` (`agent-management.ts:696-706`): one passive probe per
/// distinct key, over executable AND restricted agents.
pub(crate) fn external_cli_availability_for_agents(
    agents: &[&AgentDefinition],
) -> ExternalCliAvailability {
    let mut availability = ExternalCliAvailability::new();
    for agent in agents {
        let Some(AgentRunnerConfig::ExternalCli(cli)) = &agent.runner else {
            continue;
        };
        let key = availability_key(&cli.command, agent.machine.as_deref());
        if availability.contains_key(&key) {
            continue;
        }
        let probed = if agent.machine.is_some() {
            ssh_availability()
        } else {
            local_binary_availability(&cli.command)
        };
        availability.insert(key, probed);
    }
    availability
}

/// pi `registeredExternalJobProviderStatus()` (`agent-management.ts:676-682`) names the providers
/// registered through `api/external-job-provider.ts`. cyrup has no provider registration surface
/// (`runner::dispatch::unported_external_job_refusal`), so the registry is present and EMPTY —
/// upstream's own answer when nothing registered: every `external-job` agent reads `missing` and
/// `available: false`, never `?` (which upstream reserves for a registry that threw).
fn registered_external_job_provider_names() -> BTreeSet<String> {
    BTreeSet::new()
}

/// pi `externalJobProviderSuffix` (`agent-management.ts:684-687`).
fn external_job_provider_suffix(provider: &str, names: &BTreeSet<String>) -> &'static str {
    if names.contains(provider) {
        "✓"
    } else {
        "missing"
    }
}

/// pi `runnerListBadge` (`agent-management.ts:708-719`) with the availability map present, which is
/// the only way capability mode calls it.
fn runner_list_badge(
    agent: &AgentDefinition,
    providers: &BTreeSet<String>,
    availability: &ExternalCliAvailability,
) -> Option<String> {
    match &agent.runner {
        Some(AgentRunnerConfig::ExternalJob(job)) => Some(format!(
            "external-job:{} {}",
            job.provider,
            external_job_provider_suffix(&job.provider, providers)
        )),
        Some(AgentRunnerConfig::ExternalCli(cli)) => {
            let placed = agent.machine.as_deref().map_or_else(
                || cli.command.clone(),
                |machine| format!("{} @ {machine}", cli.command),
            );
            let Some(found) =
                availability.get(&availability_key(&cli.command, agent.machine.as_deref()))
            else {
                return Some(format!("external-cli:{placed}"));
            };
            if agent.machine.is_some() {
                return Some(format!(
                    "external-cli:{placed} saved Herdr placement; transport {}; machine not preflighted",
                    found.mark()
                ));
            }
            Some(format!("external-cli:{placed} {}", found.mark()))
        }
        None | Some(AgentRunnerConfig::Pi) => agent
            .machine
            .as_deref()
            .map(|machine| format!("machine: {machine} (saved Herdr placement)")),
    }
}

/// pi `agentListMetadata` (`agent-management.ts:721-729`). cyrup has no package-source label
/// (`packageSourceLabel`): a package agent reads `package`, as the plain listing already does.
fn agent_list_metadata(
    agent: &AgentDefinition,
    providers: &BTreeSet<String>,
    availability: &ExternalCliAvailability,
) -> String {
    let mut parts: Vec<String> = vec![source_str(agent.source).to_string()];
    if let Some(badge) = runner_list_badge(agent, providers, availability) {
        parts.push(badge);
    }
    if let Some(context) = agent.default_context {
        parts.push(format!("context: {}", context_str(context)));
    }
    if !agent.aliases.is_empty() {
        parts.push(format!("aliases: {}", agent.aliases.join(", ")));
    }
    parts.join(", ")
}

/// The declared tool names split the way upstream's `AgentConfig` holds them: `tools` (builtins
/// and extension paths, in declaration order) and `mcpDirectTools` (the `mcp:` selectors, prefix
/// stripped — `splitToolList`, `agents.ts:733-747`).
fn split_declared_tools(agent: &AgentDefinition) -> (Vec<String>, Vec<String>) {
    let mut names = Vec::new();
    let mut mcp = Vec::new();
    for tool in agent.tools.iter().flatten() {
        match tool {
            ToolRef::Builtin(name) | ToolRef::ExtensionPath(name) => names.push(name.clone()),
            ToolRef::Mcp(name) => mcp.push(name.clone()),
        }
    }
    (names, mcp)
}

/// Upstream's `thinking: string | false`: a frontmatter `thinking: false` becomes the boolean
/// (`agents.ts:2186` @v0.68.0). cyrup keeps the raw string, so the spelling is mapped here.
fn thinking_is_false(agent: &AgentDefinition) -> bool {
    agent.thinking.as_deref() == Some("false")
}

/// JS template-literal display of a JSON scalar (`${value}`): a string bare, anything else as JSON.
fn scalar_display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// JS truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// pi `formatAcceptanceDisplayLabel` (`agent-management.ts:782-784`):
/// `JSON.stringify(previewDisplayText(value, 80))` — sanitized, bounded, and quoted, so an
/// author-controlled id or reviewer name cannot forge a new row or a new `; Key:` segment.
fn acceptance_display_label(value: &str) -> String {
    Value::String(preview_display_text(value, 80)).to_string()
}

/// pi `formatReviewGateLabel` (`runs/shared/acceptance.ts:511-514` @v0.68.0) over the display
/// review (agent already passed through [`acceptance_display_label`]).
fn review_gate_label(review: &Value) -> String {
    let status = if review.get("required") == Some(&Value::Bool(false)) {
        "optional"
    } else {
        "required"
    };
    match review.get("agent").filter(|agent| truthy(agent)) {
        Some(agent) => format!(
            "{status} by {}",
            acceptance_display_label(&scalar_display(agent))
        ),
        None => status.to_string(),
    }
}

fn array_items(policy: &Value, key: &str) -> Vec<Value> {
    policy
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// pi `formatAcceptanceSummary` (`agent-management.ts:755-780`).
fn acceptance_summary(agent: &AgentDefinition) -> Option<String> {
    let mut summary: Vec<String> = Vec::new();
    match &agent.default_acceptance {
        Some(Value::Bool(false)) => summary.push("Acceptance: disabled".to_string()),
        Some(Value::String(level)) => summary.push(format!("Acceptance: {level}")),
        Some(policy) if truthy(policy) => {
            let mut modifiers: Vec<String> = array_items(policy, "evidence")
                .iter()
                .map(scalar_display)
                .collect();
            for command in array_items(policy, "verify") {
                let id = command.get("id").map(scalar_display).unwrap_or_default();
                modifiers.push(format!("verify: {}", acceptance_display_label(&id)));
            }
            let criteria = array_items(policy, "criteria").len();
            if criteria > 0 {
                modifiers.push(format!("criteria: {criteria}"));
            }
            let stop_rules = array_items(policy, "stopRules").len();
            if stop_rules > 0 {
                modifiers.push(format!("stopRules: {stop_rules}"));
            }
            match policy.get("review") {
                Some(Value::Bool(false)) => modifiers.push("review: off".to_string()),
                Some(review) if truthy(review) => {
                    modifiers.push(format!("review: {}", review_gate_label(review)));
                }
                _ => {}
            }
            if let Some(report) = policy.get("report").filter(|report| truthy(report)) {
                modifiers.push(format!("report: {}", scalar_display(report)));
            }
            let level = policy
                .get("level")
                .filter(|level| !level.is_null())
                .map_or_else(|| "auto".to_string(), scalar_display);
            if modifiers.is_empty() {
                summary.push(format!("Acceptance: {level}"));
            } else {
                summary.push(format!("Acceptance: {level} ({})", modifiers.join(", ")));
            }
        }
        _ => {}
    }
    if let Some(role) = agent.acceptance_role {
        summary.push(format!("Acceptance role: {}", role.as_str()));
    }
    (!summary.is_empty()).then(|| summary.join("; "))
}

/// pi `formatAgentCapabilitiesLine` (`agent-management.ts:735-753`).
fn format_agent_capabilities_line(
    agent: &AgentDefinition,
    providers: &BTreeSet<String>,
    availability: &ExternalCliAvailability,
) -> String {
    let (names, mcp) = split_declared_tools(agent);
    let declared: Vec<String> = names
        .into_iter()
        .chain(mcp.into_iter().map(|tool| format!("mcp:{tool}")))
        .collect();
    let mut tools = if agent.tools.is_none() {
        "default/ambient".to_string()
    } else if declared.is_empty() {
        "none".to_string()
    } else {
        declared.join(", ")
    };
    if let Some(excluded) = agent.exclude_tools.as_ref().filter(|e| !e.is_empty()) {
        tools = format!("{tools}; excludes: {}", excluded.join(", "));
    }
    let model = match &agent.model {
        None => "inherits current session".to_string(),
        Some(model) => match &agent.model_provider {
            Some(provider) if !model.as_str().contains('/') => {
                format!("{}/{}", provider.as_str(), model.as_str())
            }
            _ => model.as_str().to_string(),
        },
    };
    let thinking = if thinking_is_false(agent) {
        "off".to_string()
    } else {
        agent
            .thinking
            .clone()
            .unwrap_or_else(|| "default".to_string())
    };
    let machine = agent
        .machine
        .as_deref()
        .map(|machine| format!("; Machine: {machine} (saved Herdr placement)"))
        .unwrap_or_default();
    let acceptance = acceptance_summary(agent)
        .map(|summary| format!("; {summary}"))
        .unwrap_or_default();
    format!(
        "- {} ({}): Description: {}; Tools: {tools}; Model: {model}; Thinking: {thinking}{machine}{acceptance}",
        agent.name,
        agent_list_metadata(agent, providers, availability),
        preview_display_text(&agent.description, 240),
    )
}

/// pi `presentDetails` (`agent-management.ts:63-66`): an object with no defined member is
/// omitted rather than sent as `{}`.
fn present(map: Map<String, Value>) -> Option<Value> {
    (!map.is_empty()).then_some(Value::Object(map))
}

/// pi `EXTERNAL_JOB_CAPABILITIES` (`agent-management.ts:786`).
fn external_job_capabilities() -> Value {
    json!({ "stop": false, "steer": false, "resume": false, "structuredOutput": false, "toolEvents": false })
}

/// pi `agentCapabilityRunner` (`agent-management.ts:795-810`).
fn agent_capability_runner(
    agent: &AgentDefinition,
    providers: &BTreeSet<String>,
    availability: &ExternalCliAvailability,
) -> Value {
    match &agent.runner {
        None | Some(AgentRunnerConfig::Pi) => json!({ "type": "pi" }),
        Some(AgentRunnerConfig::ExternalCli(cli)) => {
            let mut runner = Map::new();
            runner.insert("type".into(), json!("external-cli"));
            if let Some(adapter) = cli.adapter {
                runner.insert("adapter".into(), json!(adapter.wire()));
            }
            runner.insert("command".into(), json!(cli.command));
            if let Some(machine) = &agent.machine {
                runner.insert("machine".into(), json!(machine));
            }
            // Upstream's non-null assertion: the map was built from these same agents. A miss
            // would be a caller bug, so it reads as unavailable rather than claiming a probe.
            match availability.get(&availability_key(&cli.command, agent.machine.as_deref())) {
                Some(BinaryAvailability::Available) => {
                    runner.insert("available".into(), json!(true));
                }
                Some(BinaryAvailability::Unavailable(reason)) => {
                    runner.insert("available".into(), json!(false));
                    runner.insert("unavailableReason".into(), json!(reason));
                }
                None => {
                    runner.insert("available".into(), json!(false));
                }
            }
            let capabilities = crate::runner::status::resolve_external_cli_runner_status(
                cli.adapter,
                &cli.command,
                &cli.args,
            )
            .capabilities;
            runner.insert(
                "capabilities".into(),
                serde_json::to_value(capabilities).unwrap_or(Value::Null),
            );
            Value::Object(runner)
        }
        Some(AgentRunnerConfig::ExternalJob(job)) => json!({
            "type": "external-job",
            "provider": job.provider,
            "available": providers.contains(&job.provider),
            "capabilities": external_job_capabilities(),
        }),
    }
}

/// pi `agentCapabilityTools` (`agent-management.ts:812-820`).
fn agent_capability_tools(agent: &AgentDefinition) -> Value {
    let (names, mcp) = split_declared_tools(agent);
    let mut tools = Map::new();
    // `agent.tools === undefined && agent.mcpDirectTools === undefined` — upstream can only hold
    // `mcpDirectTools` when a `tools:` key was declared, so "no `tools:` key" is the whole test.
    tools.insert("ambient".into(), json!(agent.tools.is_none()));
    tools.insert("names".into(), json!(names));
    if let Some(excluded) = &agent.exclude_tools {
        tools.insert("excludeTools".into(), json!(excluded));
    }
    tools.insert("mcpDirectTools".into(), json!(mcp));
    if let Some(mutation) = &agent.mutation_tools {
        tools.insert("mutationTools".into(), json!(mutation));
    }
    Value::Object(tools)
}

fn model_details(agent: &AgentDefinition) -> Option<Value> {
    let mut model = Map::new();
    if let Some(value) = &agent.model {
        model.insert("value".into(), json!(value.as_str()));
    }
    if thinking_is_false(agent) {
        model.insert("thinking".into(), json!(false));
    } else if let Some(thinking) = &agent.thinking {
        model.insert("thinking".into(), json!(thinking));
    }
    present(model)
}

fn execution_details(agent: &AgentDefinition) -> Option<Value> {
    let mut execution = Map::new();
    if let Some(default_async) = agent.default_async {
        execution.insert("defaultAsync".into(), json!(default_async));
    }
    if let Some(timeout) = agent.default_timeout_ms {
        execution.insert("timeoutMs".into(), json!(timeout));
    }
    present(execution)
}

fn acceptance_details(agent: &AgentDefinition) -> Option<Value> {
    let mut acceptance = Map::new();
    if let Some(policy) = &agent.default_acceptance {
        acceptance.insert("policy".into(), policy.clone());
    }
    if let Some(role) = agent.acceptance_role {
        acceptance.insert("role".into(), json!(role.as_str()));
    }
    present(acceptance)
}

fn output_details(agent: &AgentDefinition) -> Option<Value> {
    let mut output = Map::new();
    if let Some(spec) = &agent.output {
        if let Some(path) = &spec.path {
            output.insert("path".into(), json!(path.display().to_string()));
        }
        if let Some(mode) = spec.mode {
            output.insert(
                "mode".into(),
                serde_json::to_value(mode).unwrap_or(Value::Null),
            );
        }
    }
    present(output)
}

/// `extensions`/`subagentOnly`/`skills`. cyrup holds the last two as plain lists with no
/// declared-but-empty state, so an empty list is upstream's `undefined` (the same convention
/// `render::format_agent_detail` documents for `Subagent-only extensions`).
fn extension_details(agent: &AgentDefinition) -> Option<Value> {
    let mut extensions = Map::new();
    if let Some(names) = &agent.extensions {
        extensions.insert("names".into(), json!(names));
    }
    if !agent.subagent_only_extensions.is_empty() {
        extensions.insert("subagentOnly".into(), json!(agent.subagent_only_extensions));
    }
    if !agent.skills.is_empty() {
        extensions.insert("skills".into(), json!(agent.skills));
    }
    present(extensions)
}

/// pi `agentCapabilityRow` (`agent-management.ts:822-836`), in upstream's key order.
fn agent_capability_row(
    agent: &AgentDefinition,
    executable: bool,
    restriction_sources: Option<&[String]>,
    providers: &BTreeSet<String>,
    availability: &ExternalCliAvailability,
) -> Value {
    let mut row = Map::new();
    row.insert("name".into(), json!(agent.name));
    row.insert(
        "description".into(),
        json!(preview_display_text(&agent.description, 1000)),
    );
    row.insert("source".into(), json!(source_str(agent.source)));
    row.insert("executable".into(), json!(executable));
    if !executable {
        row.insert(
            "restrictionSources".into(),
            json!(restriction_sources.unwrap_or_default()),
        );
    }
    if !agent.aliases.is_empty() {
        row.insert("aliases".into(), json!(agent.aliases));
    }
    row.insert(
        "runner".into(),
        agent_capability_runner(agent, providers, availability),
    );
    row.insert("tools".into(), agent_capability_tools(agent));
    for (key, value) in [
        ("model", model_details(agent)),
        ("execution", execution_details(agent)),
        ("acceptance", acceptance_details(agent)),
        ("output", output_details(agent)),
        ("extensions", extension_details(agent)),
    ] {
        if let Some(value) = value {
            row.insert(key.into(), value);
        }
    }
    Value::Object(row)
}

/// pi `formatAgentListSections` (`agent-management.ts:885-905`): source-grouped, in upstream's
/// fixed section order, `- (none)` when there is nothing to list.
fn format_agent_list_sections(
    agents: &[&AgentDefinition],
    format_line: &dyn Fn(&AgentDefinition) -> String,
) -> Vec<String> {
    if agents.is_empty() {
        return vec!["- (none)".to_string()];
    }
    let sections = [
        (AgentSource::Package, "Package agents"),
        (AgentSource::User, "User agents"),
        (AgentSource::Project, "Project agents"),
        (AgentSource::Runtime, "Runtime agents"),
        (AgentSource::Builtin, "Builtin agents"),
    ];
    let mut lines: Vec<String> = Vec::new();
    for (source, label) in sections {
        let matches: Vec<&&AgentDefinition> = agents
            .iter()
            .filter(|agent| agent.source == source)
            .collect();
        if matches.is_empty() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(label.to_string());
        lines.extend(matches.into_iter().map(|agent| format_line(agent)));
    }
    lines
}

/// pi `appendRestrictedAgentLines` (`agent-management.ts:852-859`) — shared by the plain and the
/// capability listing, each passing its own line formatter.
pub(crate) fn append_restricted_agent_lines(
    lines: &mut Vec<String>,
    restricted: &[&AgentDefinition],
    sources: Option<&[String]>,
    format_line: &dyn Fn(&AgentDefinition) -> String,
) {
    if restricted.is_empty() {
        return;
    }
    let ceiling = sources
        .filter(|sources| !sources.is_empty())
        .map(|sources| format!("; capability ceiling: {}", sources.join(", ")))
        .unwrap_or_default();
    lines.push(String::new());
    lines.push(format!(
        "Restricted agents (not executable in this session{ceiling}):"
    ));
    lines.extend(restricted.iter().map(|agent| format_line(agent)));
}

/// The capability listing's agent half: the header, the source sections, the restricted block,
/// and the `agentCapabilities` snapshot (`agentCapabilitiesSnapshot`, `agent-management.ts:
/// 838-848`) — `{ agents, restrictedCount, capabilityCeilingSources? }`.
pub(crate) struct CapabilityListing {
    pub(crate) lines: Vec<String>,
    pub(crate) agent_capabilities: Value,
}

pub(crate) fn capability_listing(
    agents: &[&AgentDefinition],
    restricted: &[&AgentDefinition],
    restricted_sources: Option<&[String]>,
) -> CapabilityListing {
    let providers = registered_external_job_provider_names();
    let everyone: Vec<&AgentDefinition> = agents.iter().chain(restricted).copied().collect();
    let availability = external_cli_availability_for_agents(&everyone);
    let format_line =
        |agent: &AgentDefinition| format_agent_capabilities_line(agent, &providers, &availability);

    let mut lines = vec![CAPABILITIES_HEADER.to_string()];
    lines.extend(format_agent_list_sections(agents, &format_line));
    append_restricted_agent_lines(&mut lines, restricted, restricted_sources, &format_line);

    let mut rows: Vec<Value> = agents
        .iter()
        .map(|agent| agent_capability_row(agent, true, None, &providers, &availability))
        .collect();
    rows.extend(restricted.iter().map(|agent| {
        agent_capability_row(agent, false, restricted_sources, &providers, &availability)
    }));
    let mut snapshot = Map::new();
    snapshot.insert("agents".into(), Value::Array(rows));
    snapshot.insert("restrictedCount".into(), json!(restricted.len()));
    if let Some(sources) = restricted_sources.filter(|sources| !sources.is_empty()) {
        snapshot.insert("capabilityCeilingSources".into(), json!(sources));
    }
    CapabilityListing {
        lines,
        agent_capabilities: Value::Object(snapshot),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_reasons_are_bounded_to_256_utf16_units() {
        let long = "x".repeat(400);
        assert_eq!(bounded_reason(&long).len(), MAX_AVAILABILITY_REASON_UTF16);
        assert_eq!(bounded_reason("short"), "short");
    }
}
