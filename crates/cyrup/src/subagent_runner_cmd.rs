//! `__subagent-runner --config <path>` — the internal, never-user-facing CLI subcommand that is
//! hop 2 of the SubAgents extension's mandated background-execution mechanism (arch-SA §2.2/§6.5;
//! func-SA §1.1/§5.4). The orchestrator (running as an ordinary `cyrup` invocation) spawns this
//! exact subcommand, as a genuinely **detached** second OS process, via
//! `cyrup_ext_subagents::background::spawn_detached::spawn_detached_runner` — see that module's
//! docs for the detachment mechanism (new process group, stdio redirected to files, the spawned
//! [`tokio::process::Child`] handle dropped without ever being awaited).
//!
//! This module is the sole caller of [`cyrup_ext_subagents::background::runner_main::run`]: it
//! reads the `--config <path>` argv value, derives the run's [`RunPaths`] from that path per this
//! subsystem's fixed on-disk layout convention (`<AsyncRoot>/<run_id>/runner-config.json`, with
//! `ResultsDir` as `<AsyncRoot>'s parent>/results`, matching every existing test/call-site
//! convention in `cyrup-ext-subagents` — see `background::spawn_detached`'s "`cfg_path` is passed
//! verbatim" contract and every `RunPaths::for_run` call site across that crate's own test suite),
//! and calls `run` directly. There is no separate loader/interpreter hop — `cyrup` is already one
//! compiled binary (arch-SA §6.5).
//!
//! Never advertised to users: not listed in `--help`, not one of [`crate::subcommands::SUBCOMMANDS`]
//! (that list is for the package/config subcommands, a distinct concern), and dispatched from
//! `main()` **before** any user-facing arg leniency/clap parsing runs, exactly mirroring the
//! existing package/config subcommand pre-dispatch's own placement rationale (Pi `main.ts:486`).

use std::path::{Path, PathBuf};

use cyrup_ext_subagents::background::RunPaths;
use cyrup_ext_subagents::background::runner_main::run;

/// The literal argv[0] token identifying this internal subcommand (mirrors
/// `cyrup_ext_subagents::background::spawn_detached`'s private `SUBAGENT_RUNNER_SUBCOMMAND`
/// constant — kept as a second, independent literal here rather than an added public export,
/// since the two call sites — "what the orchestrator spawns" and "what `main` recognizes" — must
/// each be free-standing enough to unit-test without depending on the other crate's private
/// internals).
pub const SUBCOMMAND: &str = "__subagent-runner";

/// Returns `true` if `argv` (the process's own args, *including* argv[0]/the binary name at index
/// 0, matching [`std::env::args`]'s shape) selects this internal subcommand — i.e. its first
/// non-binary-name element is exactly [`SUBCOMMAND`].
///
/// This check MUST run before any user-facing CLI parsing (arg leniency, extension-flag
/// partitioning, `clap::Parser::parse_from`) because `__subagent-runner`'s own `--config <path>`
/// argument is not part of the user-facing [`crate::cli::Cli`] surface at all, and must never be
/// exposed to, or misinterpreted by, that parser.
#[must_use]
pub fn is_selected(argv: &[String]) -> bool {
    argv.get(1).map(String::as_str) == Some(SUBCOMMAND)
}

/// Parse `--config <path>` out of the subcommand's own remaining args (`argv[2..]`, i.e. everything
/// after `__subagent-runner` itself). Deliberately minimal: this internal subcommand has exactly
/// one required flag, so a hand-rolled scan is clearer and lighter than pulling `clap` in for a
/// one-flag internal contract never shown to a user.
///
/// # Errors
///
/// Returns a plain, human-readable message (never panics) when `--config` is missing, given with
/// no value, or given more than once with different values — every failure here is a programming
/// error in the orchestrator's own spawn call (never a user-facing input), so a sensible message on
/// stderr plus a non-zero exit is the correct terminal behavior; there is no caller left to hand a
/// structured error back to.
fn parse_config_flag(rest: &[String]) -> Result<PathBuf, String> {
    let mut found: Option<PathBuf> = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        if arg == "--config" {
            let value = iter
                .next()
                .ok_or_else(|| format!("{SUBCOMMAND}: --config requires a path argument"))?;
            match &found {
                Some(existing) if existing != Path::new(value) => {
                    return Err(format!(
                        "{SUBCOMMAND}: --config given more than once with different values"
                    ));
                }
                _ => found = Some(PathBuf::from(value)),
            }
        } else if let Some(value) = arg.strip_prefix("--config=") {
            match &found {
                Some(existing) if existing != Path::new(value) => {
                    return Err(format!(
                        "{SUBCOMMAND}: --config given more than once with different values"
                    ));
                }
                _ => found = Some(PathBuf::from(value)),
            }
        }
        // Unrecognized args are silently ignored: this internal subcommand's argv contract is
        // fixed and fully controlled by `spawn_detached_runner` (the only caller), so there is no
        // "unknown flag" user-facing error surface to maintain here.
    }
    found.ok_or_else(|| format!("{SUBCOMMAND}: missing required --config <path> argument"))
}

/// Derive this run's [`RunPaths`] from the one-shot `runner-config.json` handoff file's own path,
/// per the fixed on-disk layout convention every other module/test in `cyrup-ext-subagents`
/// already assumes:
///
/// ```text
/// <AsyncRoot>/<run_id>/runner-config.json   (= cfg_path)
/// <AsyncRoot>/<run_id>/...                  (= run_dir  = cfg_path's parent)
/// <run_id>                                  (= run_dir's own final path component)
/// <AsyncRoot's parent>/results/<run_id>.json  (= the terminal ResultFile)
/// ```
///
/// This mirrors `background::runner_main::run_id_from_paths`'s own "the run dir's final path
/// component is always the run id" assumption, and every `RunPaths::for_run(&async_root,
/// &results_dir, &run_id)` call site's `async_root`/`results_dir` sibling-directory convention
/// (`tests/background_runner_main_integration.rs`, `background/mod.rs`'s own doctests, etc. all
/// construct `results_dir` as `<parent>/results` next to `<parent>/async`).
///
/// # Errors
///
/// Returns a human-readable message (never panics) if `cfg_path` is too shallow to have both a
/// parent (`run_dir`) and a grandparent (`async_root`) — a malformed `--config` value is an
/// orchestrator-side programming error, not a user input, so this degrades to a clean process-exit
/// message rather than a panic/`indexing_slicing`-flagged access.
fn derive_run_paths(cfg_path: &Path) -> Result<RunPaths, String> {
    let run_dir = cfg_path.parent().ok_or_else(|| {
        format!(
            "{SUBCOMMAND}: --config path {} has no parent directory (cannot derive the run's own \
             run_dir)",
            cfg_path.display()
        )
    })?;
    let run_id_token = run_dir
        .file_name()
        .ok_or_else(|| {
            format!(
                "{SUBCOMMAND}: --config path {} has no run-directory component (cannot derive \
                 this run's run id)",
                cfg_path.display()
            )
        })?
        .to_string_lossy()
        .into_owned();
    let async_root = run_dir.parent().ok_or_else(|| {
        format!(
            "{SUBCOMMAND}: --config path {} has no async-root directory (cannot derive \
             ResultsDir as its sibling)",
            cfg_path.display()
        )
    })?;
    let results_dir = async_root.parent().unwrap_or(async_root).join("results");

    let run_id = cyrup_ext_subagents::background::RunId::from_token(run_id_token);
    Ok(RunPaths::for_run(async_root, &results_dir, &run_id))
}

/// Run the `__subagent-runner --config <path>` internal subcommand to completion and return the
/// process exit code `main` should use.
///
/// Every failure path here (bad/missing `--config`, an unreadable/malformed config file, an
/// internal runner-loop error) is reported as a clean, non-panicking stderr message plus a
/// non-zero exit code — `background::runner_main::run` itself is documented as "effectively
/// infallible from the caller's point of view" (every internal failure is captured into a terminal
/// on-disk `Failed` record rather than propagated), so the only `Err` this function itself
/// surfaces is a pre-flight argument/path-derivation failure that happens strictly before `run` is
/// even called — there is nobody left to report a runtime failure to once `run` itself is
/// under way, since a detached runner has no live parent to answer to (R-SA-078).
pub async fn dispatch(argv: &[String]) -> i32 {
    let rest = argv.get(2..).unwrap_or(&[]);
    let cfg_path = match parse_config_flag(rest) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };

    let run_paths = match derive_run_paths(&cfg_path) {
        Ok(paths) => paths,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };

    match run(&cfg_path, &run_paths).await {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{SUBCOMMAND}: runner loop reported an internal error: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn is_selected_matches_the_exact_internal_subcommand_token() {
        assert!(is_selected(&[
            "cyrup".to_string(),
            "__subagent-runner".to_string(),
            "--config".to_string(),
            "/tmp/x".to_string(),
        ]));
        assert!(!is_selected(&["cyrup".to_string()]));
        assert!(!is_selected(&["cyrup".to_string(), "--help".to_string()]));
        assert!(!is_selected(&["cyrup".to_string(), "config".to_string()]));
        assert!(!is_selected(&[]));
    }

    #[test]
    fn parse_config_flag_reads_space_separated_value() {
        let rest = vec![
            "--config".to_string(),
            "/a/b/runner-config.json".to_string(),
        ];
        let path = parse_config_flag(&rest).expect("parses");
        assert_eq!(path, PathBuf::from("/a/b/runner-config.json"));
    }

    #[test]
    fn parse_config_flag_reads_equals_separated_value() {
        let rest = vec!["--config=/a/b/runner-config.json".to_string()];
        let path = parse_config_flag(&rest).expect("parses");
        assert_eq!(path, PathBuf::from("/a/b/runner-config.json"));
    }

    #[test]
    fn parse_config_flag_missing_is_a_clean_error_not_a_panic() {
        let result = parse_config_flag(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_config_flag_missing_value_is_a_clean_error_not_a_panic() {
        let rest = vec!["--config".to_string()];
        let result = parse_config_flag(&rest);
        assert!(result.is_err());
    }

    #[test]
    fn derive_run_paths_uses_run_dirs_final_component_as_run_id() {
        let cfg = PathBuf::from("/base/async/a1b2c3d4/runner-config.json");
        let paths = derive_run_paths(&cfg).expect("derives");
        assert_eq!(paths.run_dir, PathBuf::from("/base/async/a1b2c3d4"));
        assert_eq!(paths.result, PathBuf::from("/base/results/a1b2c3d4.json"));
    }

    #[test]
    fn derive_run_paths_rejects_a_too_shallow_config_path() {
        let cfg = PathBuf::from("runner-config.json");
        // Has a parent ("" via `parent()` on a relative single-component path is `Some("")`), but
        // that parent has no further parent to serve as `async_root` — must error, not panic.
        let result = derive_run_paths(&cfg);
        assert!(result.is_err());
    }
}
