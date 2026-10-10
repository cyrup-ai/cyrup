//! SEAM-148 — the tool set a session STARTS with for a `--tools` list of `+name`/`-name` entries,
//! driven through the real CLI pipeline (`main.rs`'s order: short-alias normalization →
//! `apply_arg_leniency` → `partition_extension_flags` → clap → `normalize_list_flags` →
//! `to_session_config`) into the real `session_launch::build_factory`, which attaches the native
//! extensions (`codemode` among them) exactly as every mode arm does.
//!
//! pi v1.1.0 (`ddaa0a034`) applies a modifier-only list to the default selection
//! (`core/sdk.ts:280-294` @f1b2e77f5) — `--help` documents `pi --tools +codemode` as "Add codemode to
//! the default tools" (`cli/args.ts:405-406`) — and refuses a mixed or patterned list with the error
//! diagnostic `${arg}: ${getToolListError(tools)}` (`args.ts:151-161`). RED before the fix: every
//! modifier case started with NO tools (`+codemode` was an allowlist of one unknown name), and
//! `-t -bash` died in clap.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use clap::Parser;
use cyrup_config::{AppMode, CliConfigOverrides, ConfigDirs, EnvVars};
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::AgentSession;

use crate::cli::{Cli, normalize_short_aliases, partition_extension_flags};
use crate::diagnostics::{Diagnostic, DiagnosticLevel, apply_arg_leniency};

/// The pre-clap → clap pipeline `main.rs` runs, keeping the diagnostics `main.rs` reports (an
/// `Error` exits 1 there, before any session is built).
fn parse(args: &[&str]) -> Result<Cli, Vec<Diagnostic>> {
    let raw = normalize_short_aliases(args.iter().map(|s| (*s).to_string()));
    let (lenient, diags) = apply_arg_leniency(&raw);
    if diags.iter().any(|d| d.level == DiagnosticLevel::Error) {
        return Err(diags);
    }
    let (clean, extension_flags) = partition_extension_flags(&lenient);
    let mut full = vec!["cyrup".to_string()];
    full.extend(clean);
    let mut cli = Cli::try_parse_from(full).unwrap_or_else(|e| panic!("{args:?}: {e}"));
    cli.extension_flags = extension_flags;
    cli.normalize_list_flags();
    Ok(cli)
}

/// The session `cyrup <args> -p hi` starts, built through `build_factory` in a hermetic home.
async fn session_for(args: &[&str]) -> (AgentSession, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let env = EnvVars {
        home: Some(agent_dir.clone()),
        ..EnvVars::default()
    };
    let overrides = CliConfigOverrides {
        agent_dir: Some(agent_dir.clone()),
        cwd: Some(cwd.clone()),
        ..Default::default()
    };
    let dirs = ConfigDirs::resolve(&overrides, &env).unwrap();

    let mut argv: Vec<&str> = args.to_vec();
    argv.extend(["--no-session", "-p", "hi"]);
    let cli = parse(&argv).unwrap_or_else(|d| panic!("{args:?}: {d:?}"));
    let mut config = cli.to_session_config(&dirs, AppMode::Print);
    config.trust_override = Some(true);
    let target = config.target.clone();
    let factory = crate::session_launch::build_factory(
        Arc::new(FauxProvider::new()),
        config,
        crate::file_settings_store(&dirs),
        Arc::new(cyrup_config::AuthStore::at(agent_dir.join("auth.json"))),
        &dirs,
        Arc::new(cyrup_config::ModelFile::default()),
        None,
    )
    .unwrap();
    let session = factory.build(target, None).await.unwrap();
    (session, tmp)
}

async fn started_with(args: &[&str]) -> Vec<String> {
    let (session, _tmp) = session_for(args).await;
    let mut names = session.active_tool_names();
    names.sort();
    names
}

fn sorted(names: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
    names.sort();
    names
}

/// The SEAM-148 Verify line, case by case: `--tools +grep` starts with `read,bash,edit,write,grep`;
/// `--tools=-bash` and `-t -bash` start with `read,edit,write`; `--tools read` is unchanged;
/// `--no-tools --tools +read` gives an allowlist of exactly `read`. Plus pi's own `--help`
/// example, `--tools +codemode`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modifier_tools_flag_adjusts_the_tools_the_session_starts_with() {
    // The baseline the modifiers apply to: pi's four built-ins plus whatever the always-attached
    // native extensions register active (they are not built-ins, so `defaultTools` and the
    // modifiers never touch them — `includeAllExtensionTools`). The cases below are read against it.
    let baseline = started_with(&[]).await;
    let extension_tools: Vec<&str> = baseline
        .iter()
        .map(String::as_str)
        .filter(|n| !["read", "bash", "edit", "write"].contains(n))
        .collect();
    let with_extensions = |builtins: &[&str]| -> Vec<String> {
        let mut all: Vec<&str> = builtins.to_vec();
        all.extend(&extension_tools);
        sorted(&all)
    };
    assert_eq!(
        baseline,
        with_extensions(&["read", "bash", "edit", "write"])
    );
    assert!(
        !extension_tools.contains(&"codemode") && !extension_tools.contains(&"grep"),
        "{baseline:?}"
    );

    assert_eq!(
        started_with(&["--tools", "+grep"]).await,
        with_extensions(&["read", "bash", "edit", "write", "grep"])
    );
    assert_eq!(
        started_with(&["--tools", "+codemode"]).await,
        with_extensions(&["read", "bash", "edit", "write", "codemode"])
    );
    assert_eq!(
        started_with(&["--tools=-bash"]).await,
        with_extensions(&["read", "edit", "write"])
    );
    assert_eq!(
        started_with(&["-t", "-bash"]).await,
        with_extensions(&["read", "edit", "write"])
    );
    // An allowlist is unchanged: exactly the named tools, extension tools included.
    assert_eq!(started_with(&["--tools", "read"]).await, sorted(&["read"]));

    let (session, _tmp) = session_for(&["--no-tools", "--tools", "+read"]).await;
    assert_eq!(session.active_tool_names(), vec!["read".to_string()]);
    let allowed: Option<Vec<String>> = session
        .services()
        .allowed_tool_names
        .as_ref()
        .map(|set| set.iter().cloned().collect());
    assert_eq!(allowed, Some(vec!["read".to_string()]));
}

/// SEAM-148 residuals, settled against pi @f1b2e77f5.
///
/// `--tools ""` (and `--tools " , "`): `args.ts:151-160` assigns the filtered `[]`, and
/// `sdk.ts:280-293` makes it both the selection and the allowlist (`options.tools ?? …`), so pi
/// starts with NO tools. RED before `Cli.tools` became an `Option`: the empty list read as an
/// absent flag and the session started with the full default set.
///
/// `--tools -nt`: pi's `args[++i]` takes `-nt` as the value, a modifier list removing a tool named
/// `nt`, so the defaults stand. cyrup's alias pre-pass rewrites the token to `--no-tools` first,
/// which `allow_hyphen_values` takes as the value: a modifier removing `-no-tools`, so the defaults
/// stand here too. Same starting set; pinned so the two cannot drift apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_tools_list_starts_with_no_tools_and_a_dash_nt_value_keeps_the_defaults() {
    let baseline = started_with(&[]).await;
    assert!(baseline.contains(&"read".to_string()), "{baseline:?}");
    assert_eq!(started_with(&["--tools", ""]).await, Vec::<String>::new());
    assert_eq!(
        started_with(&["--tools", " , "]).await,
        Vec::<String>::new()
    );
    assert_eq!(started_with(&["--tools", "-nt"]).await, baseline);
}

/// `--tools read,+grep` and `--tools +mcp__*` exit with pi's diagnostic text (`args.ts:156-160`
/// @f1b2e77f5), the flag spelled as typed.
#[test]
fn a_mixed_or_patterned_modifier_list_is_a_startup_error() {
    for (args, expected) in [
        (
            vec!["--tools", "read,+grep"],
            "--tools: tool names cannot be mixed with +name or -name entries",
        ),
        (
            vec!["-t", "read, +grep"],
            "-t: tool names cannot be mixed with +name or -name entries",
        ),
        (
            vec!["--tools", "+mcp__*"],
            "--tools: +name and -name entries take exact tool names, not patterns: +mcp__*",
        ),
        (
            vec!["--tools=-bash,+mcp__*"],
            "--tools: +name and -name entries take exact tool names, not patterns: +mcp__*",
        ),
    ] {
        let diags = parse(&args)
            .err()
            .unwrap_or_else(|| panic!("{args:?} parsed"));
        let errors: Vec<&str> = diags
            .iter()
            .filter(|d| d.level == DiagnosticLevel::Error)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(errors, vec![expected], "{args:?}");
    }
    // A valid list is not an error, and keeps its entries verbatim for the builder.
    assert_eq!(
        parse(&["--tools", "+grep, -bash,"]).unwrap().tools,
        Some(vec!["+grep".to_string(), "-bash".to_string()])
    );
}
