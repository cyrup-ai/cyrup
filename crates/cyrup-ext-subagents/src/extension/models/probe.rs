//! Live model probing (pi `probeModel`/`resolveProbeStatus`, profiles.ts:150-335): a real
//! subprocess round-trip and the status it resolves to.
//!
//! pi's live `ctx.modelRegistry.getAvailable()` is bound in [`super`] to
//! [`super::registry_models`], the REAL built-in provider registry
//! (`cyrup_provider::catalog::builtin_catalog()`) — every registry `Model` carries required
//! (never-optional) `name`/`cost`/`context_window`/`max_tokens`/`reasoning` fields, so
//! [`super::classify::classify_model`] always takes pi's "official-metadata" branch (pi's
//! heuristic-only fallback branch is unreachable here, a direct consequence of the
//! embedded-catalog schema being fully populated rather than partial).

/// pi `ProbeStatus` (profiles.ts:13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbeStatus {
    Ok,
    Unavailable,
    Auth,
    Timeout,
    Error,
}

impl ProbeStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ProbeStatus::Ok => "ok",
            ProbeStatus::Unavailable => "unavailable",
            ProbeStatus::Auth => "auth",
            ProbeStatus::Timeout => "timeout",
            ProbeStatus::Error => "error",
        }
    }
}

/// The result of one [`probe_model`] call (pi's `{ status, message }` probe-result shape,
/// profiles.ts:322-335).
#[derive(Clone, Debug)]
pub(crate) struct ProbeOutcome {
    pub(crate) status: ProbeStatus,
    pub(crate) message: Option<String>,
}

/// Classify a non-zero probe exit's combined stderr/stdout text into a [`ProbeStatus`] (pi
/// `resolveProbeStatus`, profiles.ts:327-333): `timedOut` short-circuits to `Timeout` regardless
/// of text; empty text (no output at all) is `Error`; otherwise an auth/billing-shaped message
/// wins over an unavailable-shaped one (pi checks the auth regex first), and anything else falls
/// through to `Error`. Case-insensitive substring checks stand in for pi's `/i` regex alternations
/// (equivalent for these fixed keyword lists, and this crate has no `regex` dependency to spend on
/// them).
fn resolve_probe_status(text: &str, timed_out: bool) -> ProbeStatus {
    if timed_out {
        return ProbeStatus::Timeout;
    }
    if text.is_empty() {
        return ProbeStatus::Error;
    }
    let lower = text.to_lowercase();
    const AUTH_KEYWORDS: [&str; 7] = [
        "unauthorized",
        "unauthorised",
        "forbidden",
        "api key",
        "auth",
        "billing",
        "credit",
    ];
    if AUTH_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        return ProbeStatus::Auth;
    }
    const UNAVAILABLE_KEYWORDS: [&str; 6] = [
        "not found",
        "unknown model",
        "model unavailable",
        "model disabled",
        "unsupported model",
        "unavailable",
    ];
    if UNAVAILABLE_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        return ProbeStatus::Unavailable;
    }
    ProbeStatus::Error
}

/// The 45-second probe timeout (pi `probeModel`'s `timeout: 45_000`, profiles.ts:345).
const PROBE_TIMEOUT_MS: u64 = 45_000;

/// The fixed probe prompt (pi `probeModel`, profiles.ts:343).
const PROBE_PROMPT: &str = "Reply with exactly \"OK\".";

/// Real live-probe subprocess call (pi `probeModel`, profiles.ts:318-335): spawns this crate's own
/// resolved `cyrup` binary ([`crate::spawn::resolve_spawn_command`], the exact analog of pi's
/// literal `"pi"` binary invocation — R-SA-045 mirrors pi-subagents' `PI_SUBAGENT_PI_BINARY`) with
/// `-p --model <fullId> --no-tools "Reply with exactly \"OK\"."`, cwd = the system temp directory
/// (pi `os.tmpdir()`), a 45s timeout, and classifies the result exactly as pi does: exit code 0 is
/// always `Ok` (message = stdout, or "Probe succeeded." if stdout is blank); any other outcome
/// (non-zero exit, spawn failure, or timeout) is classified via [`resolve_probe_status`] over the
/// combined stderr+stdout text (`killed`/timed-out short-circuits to `Timeout`, matching pi's
/// `result.killed === true` check).
///
/// `spawn` supplies the command explicitly (`SubagentExtensionConfig::spawn_command`); `None`
/// resolves it from the environment exactly as before.
pub(crate) async fn probe_model(
    full_id: &str,
    spawn: Option<&crate::spawn::SpawnCommand>,
) -> ProbeOutcome {
    match spawn {
        Some(command) => probe_model_with(command, full_id, PROBE_TIMEOUT_MS).await,
        None => {
            probe_model_with(&crate::spawn::resolve_spawn_command(), full_id, PROBE_TIMEOUT_MS)
                .await
        }
    }
}

/// The injectable core of [`probe_model`], parameterized over which [`crate::spawn::SpawnCommand`]
/// to spawn and how long to wait before treating the probe as timed out — mirrors this crate's own
/// `spawn_detached_runner`/`spawn_detached_runner_with_command` injectable-core convention, so a
/// test can substitute a fast, deterministic stand-in command (`true`/`false`/a scripted shell
/// invocation) and a short timeout instead of spawning a real provider-probing `cyrup -p` call.
async fn probe_model_with(
    spawn_command: &crate::spawn::SpawnCommand,
    full_id: &str,
    timeout_ms: u64,
) -> ProbeOutcome {
    let mut command = tokio::process::Command::new(&spawn_command.binary);
    command
        .args(&spawn_command.base_args)
        .arg("-p")
        .arg("--model")
        .arg(full_id)
        .arg("--no-tools")
        .arg(PROBE_PROMPT)
        .current_dir(std::env::temp_dir())
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return ProbeOutcome {
                status: ProbeStatus::Error,
                message: Some(format!("failed to spawn probe: {e}")),
            };
        }
    };

    match tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        child.wait_with_output(),
    )
    .await
    {
        Err(_elapsed) => ProbeOutcome {
            status: ProbeStatus::Timeout,
            message: Some(format!("Probe timed out after {timeout_ms}ms.")),
        },
        Ok(Err(e)) => ProbeOutcome {
            status: ProbeStatus::Error,
            message: Some(format!("probe wait failed: {e}")),
        },
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let combined = [stderr.as_str(), stdout.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let combined = combined.trim();
            if output.status.success() {
                let message = if stdout.is_empty() {
                    "Probe succeeded.".to_string()
                } else {
                    stdout
                };
                ProbeOutcome { status: ProbeStatus::Ok, message: Some(message) }
            } else {
                let status = resolve_probe_status(combined, false);
                let message = if combined.is_empty() {
                    format!(
                        "Probe exited with code {}.",
                        output
                            .status
                            .code()
                            .map_or_else(|| "unknown".to_string(), |c| c.to_string())
                    )
                } else {
                    combined.to_string()
                };
                ProbeOutcome { status, message: Some(message) }
            }
        }
    }
}

/// pi `catalogModelIsUsable` (profiles.ts:417-419): usable iff the probe did NOT come back
/// unavailable/auth/timeout/error (`observed.availableInRegistry` is trivially always `true` here,
/// since every candidate is already drawn from the model registry).
pub(crate) fn probe_status_is_usable(status: &str) -> bool {
    !matches!(status, "unavailable" | "auth" | "timeout" | "error")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use std::path::PathBuf;

    /// pi `resolveProbeStatus` (profiles.ts:327-333): `timedOut` always wins; empty text is
    /// `error`; an auth/billing-shaped message wins over an unavailable-shaped one; anything else
    /// is `error`.
    #[test]
    fn resolve_probe_status_matches_pi_precedence() {
        assert_eq!(resolve_probe_status("anything", true), ProbeStatus::Timeout);
        assert_eq!(resolve_probe_status("", false), ProbeStatus::Error);
        assert_eq!(resolve_probe_status("401 Unauthorized: bad API key", false), ProbeStatus::Auth);
        assert_eq!(resolve_probe_status("Error: model not found", false), ProbeStatus::Unavailable);
        assert_eq!(resolve_probe_status("connection reset by peer", false), ProbeStatus::Error);
    }

    /// pi `catalogModelIsUsable` (profiles.ts:417-419): only `unavailable`/`auth`/`timeout`/`error`
    /// probe outcomes are unusable; `ok` (and any legacy/unknown string) is usable.
    #[test]
    fn probe_status_is_usable_matches_pi_predicate() {
        assert!(probe_status_is_usable("ok"));
        assert!(!probe_status_is_usable("unavailable"));
        assert!(!probe_status_is_usable("auth"));
        assert!(!probe_status_is_usable("timeout"));
        assert!(!probe_status_is_usable("error"));
    }

    /// [`probe_model_with`] exercised against REAL, fast, deterministic stand-in subprocesses (no
    /// live provider network call): a zero exit is `Ok`, a non-zero exit with an auth-shaped
    /// stderr message classifies as `Auth`, and a command that outlives the timeout classifies as
    /// `Timeout` (and is actually killed — `kill_on_drop`).
    #[tokio::test]
    async fn probe_model_with_classifies_real_subprocess_outcomes() {
        let sh = crate::spawn::SpawnCommand {
            binary: PathBuf::from("/bin/sh"),
            base_args: vec!["-c".to_string(), "printf OK".to_string()],
        };
        let ok_outcome = probe_model_with(&sh, "irrelevant/model", 5_000).await;
        assert_eq!(ok_outcome.status, ProbeStatus::Ok);

        let auth_failure = crate::spawn::SpawnCommand {
            binary: PathBuf::from("/bin/sh"),
            base_args: vec![
                "-c".to_string(),
                "echo '401 Unauthorized: invalid API key' 1>&2; exit 1".to_string(),
            ],
        };
        let auth_outcome = probe_model_with(&auth_failure, "irrelevant/model", 5_000).await;
        assert_eq!(auth_outcome.status, ProbeStatus::Auth);

        let sleeper = crate::spawn::SpawnCommand {
            binary: PathBuf::from("/bin/sh"),
            base_args: vec!["-c".to_string(), "sleep 30".to_string()],
        };
        let timeout_outcome = probe_model_with(&sleeper, "irrelevant/model", 50).await;
        assert_eq!(timeout_outcome.status, ProbeStatus::Timeout);
    }

}
