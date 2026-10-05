//! `cross-machine-discovery.ts` (`v0.16.0`) — resolving `name@machine` to one enabled Herdr saved
//! machine and one live agent on it.
//!
//! **Exactly one, or a refusal.** Every lookup here is `=== 1` (`:120-128,:136-141`, tightened by
//! `747326b`): zero matches and many matches get DIFFERENT sentences, and neither ever picks a
//! winner. A relay delivers a message to a machine the sender cannot see, so guessing between two
//! `reviewer`s is the one failure mode that cannot be noticed afterwards.
//!
//! **Why this is not [`cyrup_herdr::machine`].** That module ports pi-subagents'
//! `herdr-machine.ts` reader, which is a DIFFERENT contract over the same `herdr machine list
//! --json` output: it keys on `id` and drops a row with no usable `id`, where intercom's
//! `parseSavedMachines` requires `label` and `target` and keys on `label`. Upstream carries both
//! readers for the same reason — the two surfaces address machines by different fields — so this is
//! a second reader by parity, not by duplication. The shared mechanics (spawning a binary under a
//! deadline) are [`super::transport::run_command`], one runner for `herdr` and `ssh` alike, exactly
//! as upstream threads one `CommandRunner` through both files.

use std::time::Duration;

use super::transport::{CommandResult, CommandRunner};

/// `DISCOVERY_TIMEOUT_MS = 5_000` (`v0.16.0 cross-machine-discovery.ts:3`).
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_millis(5_000);

/// `FULL_SESSION_UUID` (`:1`) — a target that looks like this is matched against an agent's
/// SESSION ID instead of its name, which is what makes an unnamed remote session addressable.
fn is_full_session_uuid(value: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let mut parts = value.split('-');
    for expected in groups {
        let Some(part) = parts.next() else {
            return false;
        };
        if part.len() != expected || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
    }
    parts.next().is_none()
}

/// `SESSION_ID_IN_PATH` (`:2`, added by `93c5a01`) — the `_<uuid>.jsonl` suffix of a session file,
/// which is how a session id is recovered from the path Herdr reports.
///
/// The regex carries the `i` flag, so the extension is matched case-insensitively too
/// (`session_<uuid>.JSONL` counts). The id is returned as written: `FULL_SESSION_UUID` is
/// case-insensitive as well, but the later comparison against a caller's target is `===`.
fn session_id_in_path(path: &str) -> Option<String> {
    let stem_len = path.len().checked_sub(".jsonl".len())?;
    let (stem, extension) = (path.get(..stem_len)?, path.get(stem_len..)?);
    if !extension.eq_ignore_ascii_case(".jsonl") {
        return None;
    }
    let (_, candidate) = stem.rsplit_once('_')?;
    is_full_session_uuid(candidate).then(|| candidate.to_string())
}

/// `SavedMachine` (`v0.16.0 cross-machine-discovery.ts:5-9`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedMachine {
    /// The sidebar label peers address the machine by — the `@machine` half of a target.
    pub label: String,
    /// The SSH target `ssh` is invoked with.
    pub target: String,
    /// `row.enabled !== false` (`:54`) — absent means enabled.
    pub enabled: bool,
}

/// `RemoteAgent` (`v0.16.0 cross-machine-discovery.ts:11-18`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteAgent {
    /// Herdr's pane name, else the session id recovered from the session-file path (`:69`).
    pub name: String,
    /// The session id, when the session-file path carried one.
    pub session_id: Option<String>,
    /// The agent's working directory, when Herdr reported one.
    pub cwd: Option<String>,
    /// Herdr `agent_status`, e.g. `"idle"` or `"working"`.
    pub status: Option<String>,
}

/// `DiscoveredRemoteAgent` (`v0.16.0 cross-machine-discovery.ts:20-23`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredRemoteAgent {
    /// The one enabled saved machine the label resolved to.
    pub machine: SavedMachine,
    /// The one live agent on it the name or session id resolved to.
    pub agent: RemoteAgent,
}

/// Every way `name@machine` fails to resolve. `Display` is upstream's sentence for each arm, with
/// the crate's `pi` → `cyrup` product-name substitution in [`Self::NoAgentMatch`] and
/// [`Self::AmbiguousAgent`] (upstream says "Pi agent"; the agent kind cyrup reports to Herdr is
/// [`cyrup_ext_subagents::herdr::AGENT`]).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    /// `parseCrossMachineTarget` (`:82`): not exactly one `@`, or a blank or whitespace-bearing
    /// half.
    #[error(
        "Invalid remote target \"{target}\"; expected name@machine or full-session-uuid@machine."
    )]
    InvalidTarget {
        /// The target as given.
        target: String,
    },
    /// `herdr machine list` could not be run or exited non-zero (`:89`).
    #[error("Could not list Herdr saved machines: {detail}")]
    MachineListFailed {
        /// `timedOut ? "timed out" : stderr.trim() || `exit ${code}``.
        detail: String,
    },
    /// `${operation} returned invalid JSON.` (`:44`) — the only arm whose text is not about a
    /// machine, because `parseSavedMachines` is also called on its own.
    #[error("{operation} returned invalid JSON.")]
    InvalidJson {
        /// `"herdr machine list"` or `"herdr agent list"`.
        operation: String,
    },
    /// Zero enabled saved machines carry the label (`:122`).
    #[error("Saved Herdr machine \"{label}\" is unknown or disabled.")]
    UnknownMachine {
        /// The `@machine` half as given.
        label: String,
    },
    /// More than one enabled saved machine carries the label (`:125`).
    #[error(
        "Saved Herdr machine label \"{label}\" is ambiguous; expected exactly one enabled machine."
    )]
    AmbiguousMachine {
        /// The `@machine` half as given.
        label: String,
    },
    /// `herdr --machine <label> agent list` failed, or its output would not parse (`:104-115`).
    #[error("Saved Herdr machine \"{label}\" is unreachable: {detail}{hint}")]
    MachineUnreachable {
        /// The resolved machine's label.
        label: String,
        /// The reason, already stripped of a trailing `.` on the run-failure path.
        detail: String,
        /// The "your Herdr server is too old" sentence, else empty.
        hint: String,
    },
    /// No live agent on the machine matches (`:138`).
    #[error("No live cyrup agent on saved Herdr machine \"{label}\" exactly matches \"{target}\".")]
    NoAgentMatch {
        /// The resolved machine's label.
        label: String,
        /// The `name@` half as given.
        target: String,
    },
    /// More than one does (`:141`).
    #[error(
        "Multiple live cyrup agents on saved Herdr machine \"{label}\" exactly match \"{target}\"; target is ambiguous."
    )]
    AmbiguousAgent {
        /// The resolved machine's label.
        label: String,
        /// The `name@` half as given.
        target: String,
    },
}

/// `parseJsonOutput(raw, operation)` (`v0.16.0 cross-machine-discovery.ts:40-47`):
/// `isRecord(parsed) && "result" in parsed ? parsed.result : parsed`.
///
/// The `result` unwrap is what lets the same reader take herdr's CLI envelope and a bare payload.
fn parse_json_output(raw: &str, operation: &str) -> Result<serde_json::Value, DiscoveryError> {
    let parsed: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| DiscoveryError::InvalidJson {
            operation: operation.to_string(),
        })?;
    Ok(match &parsed {
        serde_json::Value::Object(map) if map.contains_key("result") => map
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        _ => parsed,
    })
}

/// `parseSavedMachines(raw)` (`v0.16.0 cross-machine-discovery.ts:49-58`).
///
/// A bare array or `{machines: [...]}`; anything else is an EMPTY catalog rather than an error
/// (`: []` at `:51`), and a row missing a string `label` or `target` is skipped rather than failing
/// the catalog (`flatMap` + `return []`). An unreadable catalog and an empty one therefore report
/// differently: the first raises [`DiscoveryError::InvalidJson`], the second
/// [`DiscoveryError::UnknownMachine`].
///
/// # Errors
/// [`DiscoveryError::InvalidJson`] when stdout is not JSON.
pub fn parse_saved_machines(raw: &str) -> Result<Vec<SavedMachine>, DiscoveryError> {
    let value = parse_json_output(raw, "herdr machine list")?;
    let rows = match &value {
        serde_json::Value::Array(rows) => rows.as_slice(),
        serde_json::Value::Object(map) => match map.get("machines") {
            Some(serde_json::Value::Array(rows)) => rows.as_slice(),
            _ => &[],
        },
        _ => &[],
    };
    Ok(rows
        .iter()
        .filter_map(|row| {
            let record = row.as_object()?;
            Some(SavedMachine {
                label: record.get("label")?.as_str()?.to_string(),
                target: record.get("target")?.as_str()?.to_string(),
                enabled: record.get("enabled") != Some(&serde_json::Value::Bool(false)),
            })
        })
        .collect())
}

/// `parseRemoteAgents(raw)` (`v0.16.0 cross-machine-discovery.ts:60-78`).
///
/// Only `{agents: [...]}` — a bare array is NOT accepted here, unlike the machine catalog (`:62`).
/// Each row must carry the agent kind this product reports to Herdr
/// ([`cyrup_ext_subagents::herdr::AGENT`], upstream's `row.agent !== "pi"`), and the name falls
/// back to the session id recovered from `agent_session.value` because "Herdr only reports a name
/// for renamed panes; an unnamed Pi session is addressed by its session id" (`:68`). A row with
/// neither is skipped — it has no address.
///
/// # Errors
/// [`DiscoveryError::InvalidJson`] when stdout is not JSON.
pub fn parse_remote_agents(raw: &str) -> Result<Vec<RemoteAgent>, DiscoveryError> {
    let value = parse_json_output(raw, "herdr agent list")?;
    let rows = match value.get("agents") {
        Some(serde_json::Value::Array(rows)) => rows.clone(),
        _ => Vec::new(),
    };
    Ok(rows
        .iter()
        .filter_map(|row| {
            let record = row.as_object()?;
            if record.get("agent").and_then(serde_json::Value::as_str)
                != Some(cyrup_ext_subagents::herdr::AGENT)
            {
                return None;
            }
            let session_id = record
                .get("agent_session")
                .and_then(serde_json::Value::as_object)
                .filter(|session| {
                    session.get("kind").and_then(serde_json::Value::as_str) == Some("path")
                })
                .and_then(|session| session.get("value"))
                .and_then(serde_json::Value::as_str)
                .and_then(session_id_in_path);
            let string = |key: &str| {
                record
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            };
            // JS `typeof row.name === "string" && row.name ? row.name : sessionId` — a BLANK name
            // is falsy and falls through to the session id.
            let name = string("name")
                .filter(|name| !name.is_empty())
                .or_else(|| session_id.clone())?;
            Some(RemoteAgent {
                name,
                session_id,
                cwd: string("cwd"),
                status: string("agent_status"),
            })
        })
        .collect())
}

/// `parseCrossMachineTarget(target)` (`v0.16.0 cross-machine-discovery.ts:80-86`) — the
/// `(agent_target, machine_label)` split.
///
/// Exactly two halves, neither blank and neither containing whitespace (JS `/\s/`, see
/// [`super::is_js_whitespace`]). Two `@`s is a refusal, not
/// a last-`@` split: the receiving relay also refuses a `target` containing `@` (`cli.ts:184`), so
/// a relay-of-a-relay has no legal spelling on either side.
///
/// # Errors
/// [`DiscoveryError::InvalidTarget`].
pub fn parse_cross_machine_target(target: &str) -> Result<(String, String), DiscoveryError> {
    let parts: Vec<&str> = target.split('@').collect();
    let invalid = || DiscoveryError::InvalidTarget {
        target: target.to_string(),
    };
    let [agent_target, machine_label] = parts.as_slice() else {
        return Err(invalid());
    };
    if [agent_target, machine_label]
        .iter()
        .any(|part| part.is_empty() || part.chars().any(super::is_js_whitespace))
    {
        return Err(invalid());
    }
    Ok(((*agent_target).to_string(), (*machine_label).to_string()))
}

/// Everything discovery needs to run a process: the runner, the `herdr` binary, and the deadline.
pub struct DiscoveryDeps<'a> {
    /// The process runner — [`super::transport::SpawnRunner`] in production, a double in tests.
    pub run: &'a dyn CommandRunner,
    /// The `herdr` binary name or path.
    pub herdr_bin: &'a str,
    /// `deps.discoveryTimeoutMs ?? DISCOVERY_TIMEOUT_MS` (`:89`).
    pub discovery_timeout: Duration,
}

/// `listSavedMachines(deps)` (`v0.16.0 cross-machine-discovery.ts:88-92`).
///
/// # Errors
/// [`DiscoveryError::MachineListFailed`] or [`DiscoveryError::InvalidJson`].
pub async fn list_saved_machines(
    deps: &DiscoveryDeps<'_>,
) -> Result<Vec<SavedMachine>, DiscoveryError> {
    let listed = run_or_fail(
        deps,
        &["machine", "list", "--json"],
        DiscoveryError::MachineListFailed {
            detail: "timed out".to_string(),
        },
    )
    .await?;
    if listed.code != 0 {
        return Err(DiscoveryError::MachineListFailed {
            detail: failure_detail(&listed),
        });
    }
    parse_saved_machines(&listed.stdout)
}

/// The `herdr` invocation both lookups share. A runner-level error (the process could not be
/// started at all) is upstream's REJECTED promise, which `discoverRemoteAgent`'s caller reports
/// the same way it reports a non-zero exit — so it folds into `on_spawn_error`'s sentence rather
/// than growing a ninth arm nobody renders differently.
async fn run_or_fail(
    deps: &DiscoveryDeps<'_>,
    args: &[&str],
    on_spawn_error: DiscoveryError,
) -> Result<CommandResult, DiscoveryError> {
    deps.run
        .run(deps.herdr_bin, args, None, Some(deps.discovery_timeout))
        .await
        .map_err(|error| match on_spawn_error {
            DiscoveryError::MachineListFailed { .. } => DiscoveryError::MachineListFailed {
                detail: error.to_string(),
            },
            other => other,
        })
}

/// `listed.timedOut ? "timed out" : listed.stderr.trim() || `exit ${code}`` (`:90`, `:107`).
fn failure_detail(result: &CommandResult) -> String {
    if result.timed_out {
        return "timed out".to_string();
    }
    let stderr = result.stderr.trim();
    if stderr.is_empty() {
        format!("exit {}", result.code)
    } else {
        stderr.to_string()
    }
}

/// herdr's Rust debug error unwrapped to a readable reason (`:105`):
/// `stderr.match(/error: "((?:[^"\\]|\\.)*)"/)?.[1]?.replace(/\\(.)/g, "$1")
/// .replace(/^machine '[^']*': /, "")`.
///
/// Hand-written rather than regex-driven — this crate has no `regex` dependency — and the scan is
/// the regex's semantics: the FIRST `error: "` whose quote actually closes (an unterminated one
/// moves the search on to the next occurrence, as a regex search does), characters up to the first
/// unescaped `"`, with every `\x` collapsing to `x`, then one leading `machine '…': ` stripped.
/// `.` does not match a line terminator, so a backslash before one cannot be consumed and that
/// candidate fails.
fn unwrap_herdr_error(stderr: &str) -> Option<String> {
    const NEEDLE: &str = "error: \"";
    let mut search_from = 0;
    let unescaped = loop {
        let found = search_from + stderr.get(search_from..)?.find(NEEDLE)?;
        if let Some(reason) = stderr
            .get(found + NEEDLE.len()..)
            .and_then(unescape_until_closing_quote)
        {
            break reason;
        }
        // `e` is ASCII, so `found + 1` is a character boundary.
        search_from = found + 1;
    };
    // `/^machine '[^']*': /` — the prefix is stripped only when the quote actually closes before
    // the `: `, so a reason that merely starts with the word `machine` survives intact.
    let stripped = unescaped
        .strip_prefix("machine '")
        .and_then(|rest| rest.split_once("': "))
        .filter(|(label, _)| !label.contains('\''))
        .map(|(_, reason)| reason.to_string());
    Some(stripped.unwrap_or(unescaped))
}

/// `((?:[^"\\]|\\.)*)"` followed by `.replace(/\\(.)/g, "$1")`: the text up to the first unescaped
/// `"` with escapes collapsed, or `None` when there is no closing quote or an escape cannot be
/// consumed.
fn unescape_until_closing_quote(rest: &str) -> Option<String> {
    let mut unescaped = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '\\' => match chars.next()? {
                '\n' | '\r' | '\u{2028}' | '\u{2029}' => return None,
                escaped => unescaped.push(escaped),
            },
            '"' => return Some(unescaped),
            other => unescaped.push(other),
        }
    }
}

/// `listMachineAgents(machine, deps)` (`v0.16.0 cross-machine-discovery.ts:94-117`).
///
/// # Errors
/// [`DiscoveryError::MachineUnreachable`].
pub async fn list_machine_agents(
    machine: &SavedMachine,
    deps: &DiscoveryDeps<'_>,
) -> Result<Vec<RemoteAgent>, DiscoveryError> {
    let unreachable = |detail: String, hint: String| DiscoveryError::MachineUnreachable {
        label: machine.label.clone(),
        detail,
        hint,
    };
    let result = deps
        .run
        .run(
            deps.herdr_bin,
            &["--machine", &machine.label, "agent", "list"],
            None,
            Some(deps.discovery_timeout),
        )
        .await
        .map_err(|error| unreachable(error.to_string(), String::new()))?;
    if result.code != 0 {
        let detail = if result.timed_out {
            "timed out".to_string()
        } else {
            unwrap_herdr_error(&result.stderr).unwrap_or_else(|| failure_detail(&result))
        };
        // `/does not support machine API forwarding/.test(detail)` (`:108`).
        let hint = if detail.contains("does not support machine API forwarding") {
            format!(
                " Its running Herdr server is too old; update Herdr there, then run \
                 `herdr --remote {}` in a terminal to replace the server.",
                machine.target
            )
        } else {
            String::new()
        };
        // `detail.replace(/\.$/, "")` then a literal `.` — so exactly one sentence-final period.
        return Err(unreachable(
            detail.strip_suffix('.').unwrap_or(&detail).to_string() + ".",
            hint,
        ));
    }
    // `:112-116` — a parse failure reports the SAME "is unreachable" sentence with no hint and no
    // period surgery, because the detail is already a sentence.
    parse_remote_agents(&result.stdout)
        .map_err(|error| unreachable(error.to_string(), String::new()))
}

/// `discoverRemoteAgent(target, deps)` (`v0.16.0 cross-machine-discovery.ts:119-145`) — the whole
/// resolution, exactly-one at both steps.
///
/// Both comparisons are case-INSENSITIVE on the label and the name and case-SENSITIVE on a session
/// id, because `FULL_SESSION_UUID` is matched with `===` (`:133`).
///
/// # Errors
/// Any [`DiscoveryError`].
pub async fn discover_remote_agent(
    target: &str,
    deps: &DiscoveryDeps<'_>,
) -> Result<DiscoveredRemoteAgent, DiscoveryError> {
    let (agent_target, machine_label) = parse_cross_machine_target(target)?;
    let mut selected: Vec<SavedMachine> = list_saved_machines(deps)
        .await?
        .into_iter()
        .filter(|machine| {
            machine.enabled && machine.label.to_lowercase() == machine_label.to_lowercase()
        })
        .collect();
    if selected.is_empty() {
        return Err(DiscoveryError::UnknownMachine {
            label: machine_label,
        });
    }
    if selected.len() > 1 {
        return Err(DiscoveryError::AmbiguousMachine {
            label: machine_label,
        });
    }
    let machine = selected.remove(0);
    let agents = list_machine_agents(&machine, deps).await?;

    let by_session_id = is_full_session_uuid(&agent_target);
    let mut matches: Vec<RemoteAgent> = agents
        .into_iter()
        .filter(|agent| {
            if by_session_id {
                agent.session_id.as_deref() == Some(agent_target.as_str())
            } else {
                agent.name.to_lowercase() == agent_target.to_lowercase()
            }
        })
        .collect();
    if matches.is_empty() {
        return Err(DiscoveryError::NoAgentMatch {
            label: machine.label,
            target: agent_target,
        });
    }
    if matches.len() > 1 {
        return Err(DiscoveryError::AmbiguousAgent {
            label: machine.label,
            target: agent_target,
        });
    }
    Ok(DiscoveredRemoteAgent {
        agent: matches.remove(0),
        machine,
    })
}
