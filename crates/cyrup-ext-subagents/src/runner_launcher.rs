//! SUBA-178 — runner launchers: named argv prefixes, declared ONLY in the user `config.json`
//! (`runnerLaunchers`), that wrap the local background runner when an agent selects one with
//! `launcher: <name>` frontmatter. Port of pi-subagents `e4b52b4b` (#2720):
//! `src/runs/shared/runner-launcher.ts` (whole file, 25 lines @ad11b7ab), the name rule and the
//! `RunnerLauncher` shape from `src/shared/types.ts:2426-2432`, and `validateRunnerLaunchersConfig`
//! from `src/extension/config.ts:106-116`.
//!
//! The point of the feature is an isolation boundary, so every path here FAILS CLOSED: an agent
//! that names a launcher never runs unwrapped. An undefined name, a launcher combined with machine
//! placement or an external runner, or a foreground request is refused with upstream's own
//! sentence before anything is created.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::runner::AgentRunnerConfig;
use crate::spawn::SpawnCommand;

/// pi `RUNNER_LAUNCHER_NAME_RULE` (`shared/types.ts:2428` @ad11b7ab), verbatim.
pub const RUNNER_LAUNCHER_NAME_RULE: &str = "must start with a letter or digit and use only letters, digits, '.', '_' or '-' (at most 128 characters)";

/// The `runnerLaunchers` user-config map: launcher name to argv prefix.
pub type RunnerLaunchers = BTreeMap<String, Vec<String>>;

/// pi `RUNNER_LAUNCHER_NAME_PATTERN` (`shared/types.ts:2427` @ad11b7ab),
/// `/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u`, hand-written. JS `$` without the `m` flag matches only
/// at the end of input, so a trailing `\n` fails here as it does upstream.
#[must_use]
pub fn is_runner_launcher_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let mut count = 1usize;
    for c in chars {
        count += 1;
        if !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
            return false;
        }
    }
    count <= 128
}

/// pi `RunnerLauncher` (`shared/types.ts:2429-2432` @ad11b7ab): the launcher a run was started
/// under, recorded in `runner-config.json` and `status.json` (argv only, never environment).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerLauncher {
    /// The `runnerLaunchers` key the agent selected.
    pub name: String,
    /// The argv prefix the runner command is appended to.
    pub argv: Vec<String>,
}

/// pi `lookupRunnerLauncher` (`runner-launcher.ts:5-8` @ad11b7ab): own-key lookup (a `BTreeMap`
/// has no inherited keys), copying the argv.
#[must_use]
pub fn lookup_runner_launcher(
    runner_launchers: Option<&RunnerLaunchers>,
    name: &str,
) -> Option<RunnerLauncher> {
    runner_launchers
        .and_then(|map| map.get(name))
        .map(|argv| RunnerLauncher {
            name: name.to_owned(),
            argv: argv.clone(),
        })
}

/// `external-cli` / `external-job`, or `None` for the local runner (pi's `runnerType`,
/// `runner-launcher.ts:13`).
fn external_runner_type(runner: Option<&AgentRunnerConfig>) -> Option<&'static str> {
    match runner {
        Some(runner @ (AgentRunnerConfig::ExternalCli(_) | AgentRunnerConfig::ExternalJob(_))) => {
            Some(runner.type_str())
        }
        Some(AgentRunnerConfig::Pi) | None => None,
    }
}

/// pi `runnerLauncherPlacementError` (`runner-launcher.ts:11-16` @ad11b7ab): a launcher wraps the
/// local background runner, so it cannot combine with machine placement or an external runner.
/// `machine` is the EFFECTIVE requested machine (`params.machine ?? agent.machine`).
#[must_use]
pub fn runner_launcher_placement_error(
    agent_name: &str,
    launcher: Option<&str>,
    runner: Option<&AgentRunnerConfig>,
    machine: Option<&str>,
) -> Option<String> {
    let launcher = launcher?;
    // pi `!machine`: an empty string is falsy.
    let machine = machine.filter(|m| !m.is_empty());
    let runner_type = external_runner_type(runner);
    let placement = match (machine, runner_type) {
        (Some(machine), _) => format!("on machine '{machine}'"),
        (None, Some(runner_type)) => format!("with runner.type='{runner_type}'"),
        (None, None) => return None,
    };
    Some(format!(
        "Agent '{agent_name}' uses launcher '{launcher}', which wraps the local Pi background runner, so it cannot run {placement}."
    ))
}

/// pi `resolveAgentRunnerLauncher` (`runner-launcher.ts:19-25` @ad11b7ab): `Ok(None)` for an agent
/// with no launcher; otherwise placement first, then the lookup. An undefined name is an error and
/// never falls back to an unwrapped launch.
///
/// # Errors
///
/// Upstream's placement sentence, or its "not defined in runnerLaunchers" sentence.
pub fn resolve_agent_runner_launcher(
    agent_name: &str,
    launcher: Option<&str>,
    runner: Option<&AgentRunnerConfig>,
    runner_launchers: Option<&RunnerLaunchers>,
    machine: Option<&str>,
) -> Result<Option<RunnerLauncher>, String> {
    let Some(name) = launcher else {
        return Ok(None);
    };
    if let Some(error) = runner_launcher_placement_error(agent_name, launcher, runner, machine) {
        return Err(error);
    }
    lookup_runner_launcher(runner_launchers, name)
        .map(Some)
        .ok_or_else(|| undefined_launcher_error(agent_name, name))
}

/// The "not defined" sentence of `resolveAgentRunnerLauncher` (`runner-launcher.ts:24`).
#[must_use]
pub fn undefined_launcher_error(agent_name: &str, launcher: &str) -> String {
    format!(
        "Agent '{agent_name}' uses launcher '{launcher}', which is not defined in runnerLaunchers in the user subagent config."
    )
}

/// pi's foreground refusal (`subagent-executor.ts:7541-7544` @ad11b7ab), verbatim. cyrup has no
/// `clarify` or `foregroundOnly` input (PB-9), but the sentence is kept as upstream wrote it.
#[must_use]
pub fn foreground_launcher_error(agent_name: &str, launcher: &str) -> String {
    format!(
        "Agent '{agent_name}' uses launcher '{launcher}', which wraps the background runner only. Foreground children run inside the parent process, so a launcher cannot wrap them. Omit async or pass async:true; clarify and foregroundOnly are unsupported."
    )
}

/// JS `String.prototype.trim` whitespace (ECMAScript WhiteSpace + LineTerminator): Rust's
/// `char::is_whitespace` plus U+FEFF, which JS trims and Rust does not, minus U+0085 (NEL), which
/// Rust's `White_Space` holds and JS does not trim (`"\u0085".trim().length === 1` in node). Without
/// the NEL exclusion an argv of `["\u0085"]`, which upstream accepts, would refuse the whole file.
fn is_js_whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}'
}

/// pi `validateRunnerLaunchersConfig` (`extension/config.ts:106-116` @ad11b7ab), on the RAW
/// `config.json` value. Absent is fine.
///
/// `[CYRUP-DELTA]` with several bad entries, the one reported first follows `serde_json`'s sorted
/// key order rather than the file's insertion order (the house rule for every raw validator).
///
/// # Errors
///
/// Upstream's own sentences, verbatim (the invalid-name one has no trailing period upstream).
pub fn validate_runner_launchers_config(value: Option<&serde_json::Value>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    let Some(object) = value.as_object() else {
        return Err(
            "config.runnerLaunchers must be a JSON object mapping launcher names to argv arrays"
                .to_owned(),
        );
    };
    for (name, argv) in object {
        let label = format!(
            "config.runnerLaunchers[{}]",
            serde_json::Value::String(name.clone())
        );
        if !is_runner_launcher_name(name) {
            return Err(format!(
                "{label} has an invalid name; launcher names {RUNNER_LAUNCHER_NAME_RULE}"
            ));
        }
        let valid = argv.as_array().is_some_and(|items| {
            !items.is_empty()
                && items.iter().all(|item| {
                    item.as_str().is_some_and(|arg| {
                        !arg.trim_matches(is_js_whitespace).is_empty() && !arg.contains('\0')
                    })
                })
        });
        if !valid {
            return Err(format!(
                "{label} must be a non-empty argv array of non-blank strings without NUL characters"
            ));
        }
    }
    Ok(())
}

/// `[CYRUP-DELTA]` no upstream analogue (upstream has no `/proc/self/exe` tier): when the runner
/// binary resolved to the magic `/proc/self/exe` link (tier 3 of
/// [`crate::spawn::resolve_spawn_command`], used when the running image was replaced on disk),
/// handing that path to a wrapper as an ARGUMENT makes the wrapper resolve it to ITS OWN image, so
/// `env` would exec `env`. Refuse instead of running something other than the runner.
#[must_use]
pub fn refuse_bare_self_exe(command: &SpawnCommand, launcher: &RunnerLauncher) -> Option<String> {
    command.arg0().map(|_| {
        format!(
            "Launcher '{}' cannot wrap the background runner: this cyrup binary was replaced on disk after it started (it can only re-exec itself through /proc/self/exe, which a wrapper would resolve to itself). Restart cyrup.",
            launcher.name
        )
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    const OBJECT: &str =
        "config.runnerLaunchers must be a JSON object mapping launcher names to argv arrays";

    fn bad_name(name: &str) -> String {
        format!(
            "config.runnerLaunchers[{}] has an invalid name; launcher names must start with a letter or digit and use only letters, digits, '.', '_' or '-' (at most 128 characters)",
            serde_json::Value::String(name.to_owned())
        )
    }

    fn bad_argv(name: &str) -> String {
        format!(
            "config.runnerLaunchers[{}] must be a non-empty argv array of non-blank strings without NUL characters",
            serde_json::Value::String(name.to_owned())
        )
    }

    /// Upstream `validateRunnerLaunchersConfig`'s three sentences (`config.ts:106-116`), case by
    /// case. Mutations killed: dropping the U+FEFF clause, widening `{0,127}` to `{0,128}`,
    /// dropping the NUL check, accepting `null`.
    #[test]
    fn runner_launchers_validator_speaks_upstreams_sentences() {
        assert_eq!(validate_runner_launchers_config(None), Ok(()));
        for value in [json!(null), json!([]), json!("x"), json!(1)] {
            assert_eq!(
                validate_runner_launchers_config(Some(&value)),
                Err(OBJECT.to_owned()),
                "{value}"
            );
        }
        let long_ok = format!("a{}", "b".repeat(127));
        let too_long = format!("a{}", "b".repeat(128));
        for name in [
            "",
            "-a",
            ".a",
            "a b",
            "'net'",
            "a\n",
            "a/b",
            too_long.as_str(),
        ] {
            let mut map = serde_json::Map::new();
            map.insert(name.to_owned(), json!(["env"]));
            assert_eq!(
                validate_runner_launchers_config(Some(&serde_json::Value::Object(map))),
                Err(bad_name(name)),
                "{name:?}"
            );
        }
        for argv in [
            json!([]),
            json!("env"),
            json!([1]),
            json!([" "]),
            json!(["\u{FEFF}"]),
            json!(["a\u{0000}"]),
            json!(["env", ""]),
            json!(null),
        ] {
            let value = json!({ "net": argv });
            assert_eq!(
                validate_runner_launchers_config(Some(&value)),
                Err(bad_argv("net")),
                "{argv}"
            );
        }
        let mut map = serde_json::Map::new();
        map.insert(long_ok, json!(["env", "--", "X=1"]));
        map.insert(
            "a.b_c-1".to_owned(),
            json!(["bwrap", "--ro-bind", "/", "/"]),
        );
        map.insert("9".to_owned(), json!([" x "]));
        // U+0085 is Rust whitespace but not JS `trim` whitespace, so upstream accepts it.
        map.insert("nel".to_owned(), json!(["\u{85}"]));
        assert_eq!(
            validate_runner_launchers_config(Some(&serde_json::Value::Object(map))),
            Ok(())
        );
        assert_eq!(validate_runner_launchers_config(Some(&json!({}))), Ok(()));
    }

    /// `resolveAgentRunnerLauncher` / `runnerLauncherPlacementError` (`runner-launcher.ts`):
    /// no launcher resolves to nothing, placement is checked before the lookup, machine wins over
    /// the runner type, an undefined name is an error and never unwrapped.
    #[test]
    fn resolve_checks_placement_before_lookup_and_never_falls_back() {
        let mut map = RunnerLaunchers::new();
        map.insert("net".into(), vec!["env".into(), "--".into(), "X=1".into()]);
        assert_eq!(
            resolve_agent_runner_launcher("a", None, None, None, Some("m")),
            Ok(None)
        );
        assert_eq!(
            resolve_agent_runner_launcher("a", Some("net"), None, Some(&map), None),
            Ok(Some(RunnerLauncher {
                name: "net".into(),
                argv: vec!["env".into(), "--".into(), "X=1".into()],
            }))
        );
        assert_eq!(
            resolve_agent_runner_launcher("a", Some("nope"), None, Some(&map), None),
            Err("Agent 'a' uses launcher 'nope', which is not defined in runnerLaunchers in the user subagent config.".to_owned())
        );
        assert_eq!(
            resolve_agent_runner_launcher("a", Some("nope"), None, None, None),
            Err("Agent 'a' uses launcher 'nope', which is not defined in runnerLaunchers in the user subagent config.".to_owned())
        );
        let job = AgentRunnerConfig::ExternalJob(crate::runner::ExternalJobRunner {
            provider: "p".into(),
            options: None,
        });
        assert_eq!(
            resolve_agent_runner_launcher("a", Some("nope"), Some(&job), None, Some("m")),
            Err("Agent 'a' uses launcher 'nope', which wraps the local Pi background runner, so it cannot run on machine 'm'.".to_owned())
        );
        assert_eq!(
            resolve_agent_runner_launcher("a", Some("net"), Some(&job), Some(&map), None),
            Err("Agent 'a' uses launcher 'net', which wraps the local Pi background runner, so it cannot run with runner.type='external-job'.".to_owned())
        );
        assert_eq!(
            resolve_agent_runner_launcher(
                "a",
                Some("net"),
                Some(&AgentRunnerConfig::Pi),
                Some(&map),
                Some("")
            )
            .map(|l| l.map(|l| l.name)),
            Ok(Some("net".to_owned()))
        );
    }

    #[test]
    fn launcher_names_follow_upstreams_pattern() {
        for ok in ["a", "A", "0", "net", "a.b_c-1", "1-x"] {
            assert!(is_runner_launcher_name(ok), "{ok}");
        }
        for bad in ["", "_a", "a b", "a\n", "é", "a+b"] {
            assert!(!is_runner_launcher_name(bad), "{bad:?}");
        }
        assert!(is_runner_launcher_name(&"a".repeat(128)));
        assert!(!is_runner_launcher_name(&"a".repeat(129)));
    }

    /// `[CYRUP-DELTA]` a wrapper handed `/proc/self/exe` would exec itself, so that runner binary
    /// is refused under a launcher. Mutation killed: removing the guard.
    #[test]
    fn a_launcher_refuses_to_wrap_proc_self_exe() {
        let launcher = RunnerLauncher {
            name: "net".into(),
            argv: vec!["env".into()],
        };
        let self_exe = SpawnCommand {
            binary: PathBuf::from("/proc/self/exe"),
            base_args: Vec::new(),
        };
        let refusal = refuse_bare_self_exe(&self_exe, &launcher).unwrap();
        assert!(
            refusal.starts_with("Launcher 'net' cannot wrap the background runner"),
            "{refusal}"
        );
        let real = SpawnCommand {
            binary: PathBuf::from("/usr/bin/cyrup"),
            base_args: Vec::new(),
        };
        assert_eq!(refuse_bare_self_exe(&real, &launcher), None);
    }
}
