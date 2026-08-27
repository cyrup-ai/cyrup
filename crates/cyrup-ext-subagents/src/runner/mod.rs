//! SUBA-074 stage 1 — the agent `runner:` frontmatter schema: parse, validate, and the refusal that
//! keeps a non-`pi` runner from silently launching as a full-capability native child.
//!
//! Upstream: `parseAgentRunnerFrontmatter` / `validateExternalRunnerProfile`
//! (`pi-subagents/src/agents/agents.ts:1803`/`:1864` @v0.57.0) plus the two contract validators in
//! [`contract`]. Actually EXECUTING a non-`pi` runner is stage 2 (`external-cli-runner.ts` and the
//! six code-owned adapters); until it lands, [`AgentRunnerConfig::refusal_reason`] is what stands
//! between a declared external profile and a native child with the full builtin tool surface.
//!
//! ## Two serializations, deliberately
//!
//! [`AgentRunnerConfig`] carries `serde` derives for ONE consumer: the plan-time persona map that
//! crosses the hop-2 detached-runner process boundary
//! ([`crate::exec::ResolvedAgentPersona`] inside `runner-config.json`). That is a
//! cyrup-internal handoff, so its field spellings need only round-trip with themselves.
//!
//! The AGENT-FILE `runner:` block is a separate representation with its own reader and writer —
//! [`parse_agent_runner_frontmatter`] and [`runner_to_json_string`] — because it must match
//! upstream's key names exactly (`promptDelivery`, not serde's `promptDeliveryStdin`) and must omit
//! absent optionals the way upstream's object spread does, so an author's file round-trips
//! byte-stably through a management rewrite. Do not replace either with the other.

pub mod contract;

use serde_json::Value;

use contract::ExternalCliCapabilityNarrowing;

/// `AgentRunnerConfig` (`shared/types.ts:1403` @v0.57.0).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AgentRunnerConfig {
    /// `{ type: "pi" }` — the native child this crate already spawns. Explicitly declaring it is
    /// identical to declaring nothing.
    Pi,
    /// `{ type: "external-cli", … }`.
    ExternalCli(ExternalCliRunner),
    /// `{ type: "external-job", … }`.
    ExternalJob(ExternalJobRunner),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliRunner {
    /// One of [`contract::CODE_OWNED_ADAPTER_IDS`], or `None` for the generic adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// Non-empty, trimmed.
    pub command: String,
    /// Empty when omitted. Never non-empty alongside `adapter` (the adapter owns its argv).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Upstream's only legal value is `"stdin"`, so this is a flag: `true` iff the key was present.
    #[serde(default)]
    pub prompt_delivery_stdin: bool,
    /// Narrowing-only; see [`contract::parse_capability_narrowing`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ExternalCliCapabilityNarrowing>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalJobRunner {
    /// Non-empty AND already trimmed — upstream rejects `" x "` rather than trimming it.
    pub provider: String,
    /// A JSON-serializable object. Carried raw; no consumer exists until stage 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
}

impl AgentRunnerConfig {
    /// Upstream's `runner.type` spelling, for refusal text.
    #[must_use]
    pub fn type_str(&self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::ExternalCli(_) => "external-cli",
            Self::ExternalJob(_) => "external-job",
        }
    }

    /// The adapter id to hand [`contract::validate_code_owned_profile_runner`] — `Some` only for an
    /// `external-cli` runner that names one.
    #[must_use]
    pub fn code_owned_adapter(&self) -> Option<&str> {
        match self {
            Self::ExternalCli(cli) => cli.adapter.as_deref(),
            _ => None,
        }
    }

    /// **Stage-1 gate.** `Some(reason)` when this crate cannot honour the declared runner, so the
    /// launch must be REFUSED rather than silently downgraded to a native child.
    ///
    /// `None` for [`Self::Pi`] — which is the native child, and therefore always honourable.
    ///
    /// [CYRUP-DELTA] upstream has no equivalent: it implements every runner type, so it never
    /// refuses one. This refusal is a deliberate, temporary cyrup-only state that exists precisely
    /// because the execution half is stage 2 — the alternative is the present silent widening. It
    /// is deleted, not relaxed, when stage 2 lands.
    #[must_use]
    pub fn refusal_reason(&self) -> Option<String> {
        match self {
            Self::Pi => None,
            Self::ExternalCli(cli) => Some(format!(
                "Agent runner.type='external-cli'{} is declared but not yet supported by cyrup \
                 (SUBA-074 stage 2). Refusing to launch rather than running this profile as a \
                 full-capability native child.",
                cli.adapter
                    .as_deref()
                    .map_or_else(String::new, |a| format!(" (adapter '{a}')")),
            )),
            Self::ExternalJob(job) => Some(format!(
                "Agent runner.type='external-job' (provider '{}') is declared but not yet \
                 supported by cyrup (SUBA-074 stage 2). Refusing to launch rather than running \
                 this profile as a full-capability native child.",
                job.provider,
            )),
        }
    }
}

/// Re-emit an [`AgentRunnerConfig`] as the compact JSON an agent file's `runner:` line carries.
///
/// Mirrors upstream's object spread exactly — every absent optional is OMITTED rather than written
/// as `null`, so an author's block survives a management rewrite byte-stably. This is the writer
/// half of [`parse_agent_runner_frontmatter`]; see the module doc for why it is hand-written
/// instead of derived.
#[must_use]
pub fn runner_to_json_string(runner: &AgentRunnerConfig) -> String {
    // Built as an ordered list of `"key":value` fragments rather than a `serde_json::Map`: this
    // crate does not enable serde_json's `preserve_order` feature, so a `Map` is a `BTreeMap` and
    // would emit keys ALPHABETICALLY (`command` before `type`), breaking the byte-stable
    // round-trip an author's file depends on. Values still go through `serde_json` so escaping is
    // correct; only the ORDER is hand-controlled, and it is upstream's own.
    let mut fields: Vec<String> = Vec::new();
    let mut push = |key: &str, value: &Value| {
        fields.push(format!(
            "{}:{}",
            Value::String(key.to_string()),
            serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
        ));
    };
    push("type", &Value::String(runner.type_str().to_string()));
    match runner {
        AgentRunnerConfig::Pi => {}
        AgentRunnerConfig::ExternalCli(cli) => {
            if let Some(adapter) = &cli.adapter {
                push("adapter", &Value::String(adapter.clone()));
            }
            push("command", &Value::String(cli.command.clone()));
            if !cli.args.is_empty() {
                push(
                    "args",
                    &Value::Array(cli.args.iter().cloned().map(Value::String).collect()),
                );
            }
            if cli.prompt_delivery_stdin {
                push("promptDelivery", &Value::String("stdin".to_string()));
            }
            if let Some(capabilities) = &cli.capabilities {
                push(
                    "capabilities",
                    &Value::Object(
                        capabilities
                            .iter()
                            .map(|(key, value)| (key.clone(), Value::Bool(*value)))
                            .collect(),
                    ),
                );
            }
        }
        AgentRunnerConfig::ExternalJob(job) => {
            push("provider", &Value::String(job.provider.clone()));
            if let Some(options) = &job.options {
                push("options", options);
            }
        }
    }
    format!("{{{}}}", fields.join(","))
}

/// The fourteen Pi-only frontmatter keys an external profile may not declare
/// (`agents.ts:1866-1867`). Order is upstream's and reaches the user in the refusal.
const PI_ONLY_FIELDS: [&str; 14] = [
    "tools",
    "model",
    "fallbackModels",
    "thinking",
    "extensions",
    "subagentOnlyExtensions",
    "maxSubagentDepth",
    "completionGuard",
    "skills",
    "skill",
    "skillPath",
    "toolBudget",
    "permission",
    "permissions",
];

/// `validateExternalRunnerProfile(frontmatter, agentName, runner)` (`agents.ts:1864-1871`).
///
/// Tests **key presence in the raw frontmatter map**, not a parsed value — upstream's
/// `frontmatter[field] !== undefined` — so a present-but-empty `tools:` trips it too. `present`
/// is the parser's raw key set for this file.
///
/// # Errors
///
/// Upstream's single refusal, verbatim.
pub fn validate_external_runner_profile(
    agent_name: &str,
    runner: Option<&AgentRunnerConfig>,
    present: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let Some(runner) = runner else { return Ok(()) };
    if matches!(runner, AgentRunnerConfig::Pi) {
        return Ok(());
    }
    let unsupported: Vec<&str> = PI_ONLY_FIELDS
        .into_iter()
        .filter(|field| present(field))
        .collect();
    if unsupported.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Agent '{agent_name}' uses runner.type='{}' and declares unsupported Pi-only fields: {}.",
        runner.type_str(),
        unsupported.join(", ")
    ))
}

/// `parseAgentRunnerFrontmatter(raw, agentName)` (`agents.ts:1803-1862`).
///
/// [CYRUP-DELTA] upstream parses the block with `parseYaml`; this crate has no YAML parser and its
/// settled convention for every object-valued frontmatter key (`toolBudget`, `turnBudget`,
/// `permission`) is `serde_json`. JSON is a strict subset of YAML flow style, so every value
/// accepted here is also valid upstream; the divergence is that a block-style `runner:` is REFUSED
/// (loudly, per the caller's warn-and-skip) rather than parsed.
///
/// # Errors
///
/// Upstream's thirteen refusals, verbatim, in upstream's own order. The JSON-parse failure is
/// cyrup's own (upstream's YAML-parse failure has no equivalent text).
pub fn parse_agent_runner_frontmatter(
    raw: Option<&str>,
    agent_name: &str,
) -> Result<Option<AgentRunnerConfig>, String> {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let parsed: Value = serde_json::from_str(raw)
        .map_err(|err| format!("Agent '{agent_name}' has invalid runner frontmatter: {err}"))?;
    let Some(object) = parsed.as_object() else {
        return Err(format!(
            "Agent '{agent_name}' has invalid runner frontmatter; expected an object."
        ));
    };

    match object.get("type").and_then(Value::as_str) {
        Some("pi") => {
            if object.keys().any(|key| key != "type") {
                return Err(format!(
                    "Agent '{agent_name}' has invalid Pi runner frontmatter; only 'type' is \
                     supported."
                ));
            }
            Ok(Some(AgentRunnerConfig::Pi))
        }
        Some("external-job") => {
            let provider = object.get("provider").and_then(Value::as_str).unwrap_or_default();
            if provider.is_empty() || provider.trim() != provider {
                return Err(format!(
                    "Agent '{agent_name}' external-job runner requires a non-empty trimmed \
                     provider string."
                ));
            }
            let options = match object.get("options") {
                None => None,
                // serde_json cannot represent a non-serializable value, so upstream's
                // `isJsonSerializable` reduces here to the object/array shape test alone.
                Some(value) if value.is_object() => Some(value.clone()),
                Some(_) => {
                    return Err(format!(
                        "Agent '{agent_name}' external-job runner options must be a \
                         JSON-serializable object."
                    ));
                }
            };
            let unknown: Vec<&str> = object
                .keys()
                .map(String::as_str)
                .filter(|k| !matches!(*k, "type" | "provider" | "options"))
                .collect();
            if !unknown.is_empty() {
                return Err(format!(
                    "Agent '{agent_name}' external-job runner has unsupported fields: {}.",
                    unknown.join(", ")
                ));
            }
            Ok(Some(AgentRunnerConfig::ExternalJob(ExternalJobRunner {
                provider: provider.to_string(),
                options,
            })))
        }
        Some("external-cli") => {
            let command = object.get("command").and_then(Value::as_str).unwrap_or_default();
            if command.trim().is_empty() {
                return Err(format!(
                    "Agent '{agent_name}' external-cli runner requires a non-empty command string."
                ));
            }
            let args: Vec<String> = match object.get("args") {
                None => Vec::new(),
                Some(Value::Array(items)) if items.iter().all(Value::is_string) => items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                Some(_) => {
                    return Err(format!(
                        "Agent '{agent_name}' external-cli runner args must be an array of strings."
                    ));
                }
            };
            let adapter = match object.get("adapter") {
                None => None,
                Some(value) => {
                    let id = value.as_str().unwrap_or_default();
                    if !contract::is_code_owned_adapter_id(id) {
                        return Err(format!(
                            "Agent '{agent_name}' external-cli runner adapter must be {}.",
                            contract::CODE_OWNED_ADAPTER_LABEL
                        ));
                    }
                    // Upstream orders this AFTER the id check, so a bad id reports the id problem.
                    if !args.is_empty() {
                        return Err(format!(
                            "Agent '{agent_name}' {id} adapter owns its argv; runner args are not \
                             supported."
                        ));
                    }
                    Some(id.to_string())
                }
            };
            let prompt_delivery_stdin = match object.get("promptDelivery") {
                None => false,
                Some(Value::String(s)) if s == "stdin" => true,
                Some(_) => {
                    return Err(format!(
                        "Agent '{agent_name}' external-cli runner promptDelivery must be 'stdin'."
                    ));
                }
            };
            let capabilities = contract::parse_capability_narrowing(
                object.get("capabilities"),
                &format!("Agent '{agent_name}' external-cli runner capabilities"),
            )?;
            let unknown: Vec<&str> = object
                .keys()
                .map(String::as_str)
                .filter(|k| {
                    !matches!(
                        *k,
                        "type" | "adapter" | "command" | "args" | "promptDelivery" | "capabilities"
                    )
                })
                .collect();
            if !unknown.is_empty() {
                return Err(format!(
                    "Agent '{agent_name}' external-cli runner has unsupported fields: {}.",
                    unknown.join(", ")
                ));
            }
            Ok(Some(AgentRunnerConfig::ExternalCli(ExternalCliRunner {
                adapter,
                command: command.trim().to_string(),
                args,
                prompt_delivery_stdin,
                capabilities,
            })))
        }
        _ => Err(format!(
            "Agent '{agent_name}' has invalid runner.type; expected 'pi', 'external-cli', or \
             'external-job'."
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn err(raw: &str) -> String {
        parse_agent_runner_frontmatter(Some(raw), "worker")
            .expect_err("expected a refusal")
    }

    /// `{ type: "pi" }` is the native child, accepted with no other key permitted
    /// (`agents.ts:1815-1818` @v0.57.0).
    #[test]
    fn a_pi_runner_accepts_only_its_type_key() {
        assert_eq!(
            parse_agent_runner_frontmatter(Some(r#"{"type":"pi"}"#), "worker").unwrap(),
            Some(AgentRunnerConfig::Pi)
        );
        assert_eq!(
            err(r#"{"type":"pi","command":"x"}"#),
            "Agent 'worker' has invalid Pi runner frontmatter; only 'type' is supported."
        );
        // An absent or blank block is simply "no runner declared".
        assert_eq!(parse_agent_runner_frontmatter(None, "worker").unwrap(), None);
        assert_eq!(parse_agent_runner_frontmatter(Some("   "), "worker").unwrap(), None);
    }

    /// Upstream's `external-cli` refusals, verbatim and in upstream's own order
    /// (`agents.ts:1835-1852`).
    #[test]
    fn external_cli_refusals_are_upstreams_verbatim_text_in_upstreams_order() {
        assert_eq!(
            err(r#"{"type":"external-cli"}"#),
            "Agent 'worker' external-cli runner requires a non-empty command string."
        );
        assert_eq!(
            err(r#"{"type":"external-cli","command":"c","args":[1]}"#),
            "Agent 'worker' external-cli runner args must be an array of strings."
        );
        assert_eq!(
            err(r#"{"type":"external-cli","command":"c","adapter":"nope"}"#),
            format!(
                "Agent 'worker' external-cli runner adapter must be {}.",
                contract::CODE_OWNED_ADAPTER_LABEL
            )
        );
        // The adapter owns its argv — and note this fires AFTER the id check, so a BAD id with
        // args reports the id problem, not the argv one.
        assert_eq!(
            err(r#"{"type":"external-cli","command":"c","adapter":"claude-code","args":["-p"]}"#),
            "Agent 'worker' claude-code adapter owns its argv; runner args are not supported."
        );
        assert_eq!(
            err(r#"{"type":"external-cli","command":"c","adapter":"nope","args":["-p"]}"#),
            format!(
                "Agent 'worker' external-cli runner adapter must be {}.",
                contract::CODE_OWNED_ADAPTER_LABEL
            ),
            "rule 9 precedes rule 10, so a bad adapter id reports the id problem"
        );
        assert_eq!(
            err(r#"{"type":"external-cli","command":"c","promptDelivery":"file"}"#),
            "Agent 'worker' external-cli runner promptDelivery must be 'stdin'."
        );
        assert_eq!(
            err(r#"{"type":"external-cli","command":"c","nope":1}"#),
            "Agent 'worker' external-cli runner has unsupported fields: nope."
        );
        assert_eq!(
            err(r#"{"type":"webhook"}"#),
            "Agent 'worker' has invalid runner.type; expected 'pi', 'external-cli', or 'external-job'."
        );
        assert_eq!(
            err("[]"),
            "Agent 'worker' has invalid runner frontmatter; expected an object."
        );
    }

    /// Capabilities may only be NARROWED — a `true` is a widening attempt and is refused
    /// (`external-cli-contract.ts:65-76`). `stop` is not narrowable at all.
    #[test]
    fn capability_narrowing_refuses_widening_and_unknown_keys() {
        assert_eq!(
            err(r#"{"type":"external-cli","command":"c","capabilities":{"steer":true}}"#),
            "Agent 'worker' external-cli runner capabilities.steer may only be false; user config \
             cannot widen code-owned external adapter capabilities."
        );
        assert_eq!(
            err(r#"{"type":"external-cli","command":"c","capabilities":{"stop":false}}"#),
            "Agent 'worker' external-cli runner capabilities has unsupported fields: stop.",
            "`stop` is the one capability an external adapter always has, so it is not narrowable"
        );
        let ok = parse_agent_runner_frontmatter(
            Some(r#"{"type":"external-cli","command":"c","capabilities":{"steer":false}}"#),
            "worker",
        )
        .unwrap();
        let AgentRunnerConfig::ExternalCli(cli) = ok.unwrap() else {
            panic!("expected an external-cli runner");
        };
        assert_eq!(cli.capabilities.unwrap().get("steer"), Some(&false));
    }

    /// `external-job` requires an already-TRIMMED provider — upstream rejects `" x "` rather than
    /// trimming it (`agents.ts:1820-1822`).
    #[test]
    fn external_job_requires_a_trimmed_provider_and_an_object_options() {
        assert_eq!(
            err(r#"{"type":"external-job","provider":" x "}"#),
            "Agent 'worker' external-job runner requires a non-empty trimmed provider string."
        );
        assert_eq!(
            err(r#"{"type":"external-job","provider":""}"#),
            "Agent 'worker' external-job runner requires a non-empty trimmed provider string."
        );
        assert_eq!(
            err(r#"{"type":"external-job","provider":"p","options":[]}"#),
            "Agent 'worker' external-job runner options must be a JSON-serializable object."
        );
        assert_eq!(
            err(r#"{"type":"external-job","provider":"p","nope":1}"#),
            "Agent 'worker' external-job runner has unsupported fields: nope."
        );
    }

    /// The reserved-selection-name guard (`external-cli-contract.ts:48-63`): a name reserved for a
    /// read-only adapter may only be claimed by that adapter. The `access` word differs per row.
    #[test]
    fn reserved_selection_names_may_only_be_claimed_by_their_own_adapter() {
        assert_eq!(
            contract::validate_code_owned_profile_runner(&["claude-code"], None),
            Some(
                "Selection name 'claude-code' is reserved for the read-only 'claude-code' adapter. \
                 Use 'claude-code-writer' for explicit file-write access."
                    .to_string()
            )
        );
        assert_eq!(
            contract::validate_code_owned_profile_runner(&["codex-exec"], None),
            Some(
                "Selection name 'codex-exec' is reserved for the read-only 'codex-exec' adapter. \
                 Use 'codex-exec-writer' for explicit workspace-write access."
                    .to_string()
            ),
            "the access word is per-adapter and must not be unified"
        );
        // The adapter itself may claim its own name; an unrelated name is unaffected.
        assert_eq!(
            contract::validate_code_owned_profile_runner(&["claude-code"], Some("claude-code")),
            None
        );
        assert_eq!(contract::validate_code_owned_profile_runner(&["reviewer"], None), None);
    }

    /// `validateExternalRunnerProfile` tests KEY PRESENCE, not a parsed value, so a
    /// present-but-empty `tools:` still trips it (`agents.ts:1864-1871`). A `pi` runner is exempt.
    #[test]
    fn an_external_profile_may_not_declare_pi_only_fields() {
        let cli = AgentRunnerConfig::ExternalCli(ExternalCliRunner {
            adapter: None,
            command: "c".to_string(),
            args: Vec::new(),
            prompt_delivery_stdin: false,
            capabilities: None,
        });
        assert_eq!(
            validate_external_runner_profile("worker", Some(&cli), |f| f == "tools").unwrap_err(),
            "Agent 'worker' uses runner.type='external-cli' and declares unsupported Pi-only \
             fields: tools."
        );
        // Order is upstream's own, not the caller's.
        assert_eq!(
            validate_external_runner_profile("worker", Some(&cli), |f| matches!(
                f,
                "permissions" | "model"
            ))
            .unwrap_err(),
            "Agent 'worker' uses runner.type='external-cli' and declares unsupported Pi-only \
             fields: model, permissions."
        );
        assert!(validate_external_runner_profile("worker", Some(&cli), |_| false).is_ok());
        // `pi` and "no runner" are both exempt.
        assert!(
            validate_external_runner_profile("worker", Some(&AgentRunnerConfig::Pi), |_| true)
                .is_ok()
        );
        assert!(validate_external_runner_profile("worker", None, |_| true).is_ok());
    }

    /// The stage-1 gate: `pi` is honourable, every other runner refuses the launch rather than
    /// silently downgrading to a full-capability native child.
    #[test]
    fn only_a_pi_runner_is_honourable_today() {
        assert_eq!(AgentRunnerConfig::Pi.refusal_reason(), None);

        let cli = AgentRunnerConfig::ExternalCli(ExternalCliRunner {
            adapter: Some("claude-code".to_string()),
            command: "claude".to_string(),
            args: Vec::new(),
            prompt_delivery_stdin: false,
            capabilities: None,
        });
        let reason = cli.refusal_reason().expect("external-cli must refuse");
        assert!(reason.contains("runner.type='external-cli'"), "{reason}");
        assert!(reason.contains("adapter 'claude-code'"), "{reason}");
        assert!(reason.contains("full-capability native child"), "{reason}");

        let job = AgentRunnerConfig::ExternalJob(ExternalJobRunner {
            provider: "acme".to_string(),
            options: None,
        });
        let reason = job.refusal_reason().expect("external-job must refuse");
        assert!(reason.contains("runner.type='external-job'"), "{reason}");
        assert!(reason.contains("provider 'acme'"), "{reason}");
    }

    /// The frontmatter writer omits absent optionals exactly as upstream's spread does, so an
    /// author's block round-trips byte-stably through a management rewrite.
    #[test]
    fn runner_json_round_trips_byte_stably_and_omits_absent_optionals() {
        for raw in [
            r#"{"type":"pi"}"#,
            r#"{"type":"external-cli","command":"claude"}"#,
            r#"{"type":"external-cli","adapter":"claude-code","command":"claude"}"#,
            r#"{"type":"external-cli","command":"c","args":["-p","x"]}"#,
            r#"{"type":"external-cli","command":"c","promptDelivery":"stdin"}"#,
            r#"{"type":"external-cli","command":"c","capabilities":{"steer":false}}"#,
            r#"{"type":"external-job","provider":"acme"}"#,
            r#"{"type":"external-job","provider":"acme","options":{"k":1}}"#,
        ] {
            let parsed = parse_agent_runner_frontmatter(Some(raw), "worker")
                .unwrap_or_else(|e| panic!("{raw} must parse: {e}"))
                .unwrap_or_else(|| panic!("{raw} must yield a runner"));
            assert_eq!(
                runner_to_json_string(&parsed),
                raw,
                "the writer must reproduce the author's block byte-for-byte"
            );
        }
    }
}
