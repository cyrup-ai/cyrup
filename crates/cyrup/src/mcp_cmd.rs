//! `cyrup mcp <verb>` — MCP-049's out-of-session surface (`cli.js:60-218`).
//!
//! Only `init` is recognised. It is dispatched specially, like `config`: its verbs are not package
//! verbs and it takes none of `PackageCommand`'s flag grammar.

use cyrup_config::ConfigDirs;
use cyrup_mcp::config::{ConfigContext, HostConfigDiscovery};
use cyrup_mcp::dirs::McpDirs;

/// `printHelp` (`cli.js:68-76`), with cyrup's own binary name.
fn print_help() {
    println!("cyrup mcp helper\n");
    println!("Run:");
    println!("  cyrup mcp init       Detect host configs and scaffold cyrup imports");
    println!("  cyrup mcp init --dry-run");
    println!("  cyrup mcp init --discover-host-configs  Opt in to host config fallback discovery");
}

/// `main` (`cli.js:197-218`) — the verb table. The returned code becomes the process exit code.
#[must_use]
pub fn run(argv: &[String], dirs: &ConfigDirs) -> i32 {
    let Some(command) = argv.first().map(String::as_str) else {
        print_help();
        return 0;
    };
    match command {
        "help" | "--help" | "-h" => {
            print_help();
            0
        }
        // The two retirement errors, on stderr, exactly as upstream.
        "install" => {
            eprintln!("The custom downloader has been retired.");
            eprintln!(
                "Use `cyrup install npm:pi-mcp-adapter` instead, then optionally run `cyrup mcp init`."
            );
            1
        }
        "init" => run_init(argv.get(1..).unwrap_or_default(), dirs),
        other => {
            eprintln!("Unknown command: {other}");
            print_help();
            1
        }
    }
}

/// `printDiscovery` (`cli.js:117-142`).
fn print_discovery(ctx: &ConfigContext, found: &[cyrup_mcp::config::DiscoveredImportConfig]) {
    println!("Config discovery:\n");
    for entry in ctx.config_discovery_paths() {
        let prefix = if entry.exists { "\u{2713}" } else { "-" };
        println!("{prefix} {}: {}", entry.label, entry.path.display());
    }
    println!("\nCompatibility imports:\n");
    if found.is_empty() {
        // Upstream RETURNS here — no import rows, and no trailing blank line.
        println!("- No host-specific MCP configs detected");
        return;
    }
    for entry in found {
        println!("\u{2713} {}: {}", entry.kind.as_str(), entry.path.display());
    }
}

/// `runInit` (`cli.js:150-195`), in upstream's exact order.
fn run_init(argv: &[String], dirs: &ConfigDirs) -> i32 {
    let dry_run = argv.iter().any(|arg| arg == "--dry-run");
    let discover_host_configs = argv.iter().any(|arg| arg == "--discover-host-configs");

    let mcp_dirs = McpDirs::new(dirs.agent_dir.clone(), dirs.cwd.clone());
    let ctx = ConfigContext::new(mcp_dirs, None).with_home(dirs.home.clone());

    // 1 — discovery, with its diagnostics on stderr so they cannot be confused with the report.
    let mut diagnostics = Vec::new();
    let found = ctx.find_available_import_configs(&mut diagnostics);
    for diagnostic in &diagnostics {
        eprintln!("{diagnostic}");
    }

    // 2 — what is already imported, and what is therefore new.
    let loaded = ctx.load();
    let existing: std::collections::HashSet<String> =
        loaded.config.imports.iter().cloned().collect();
    let to_add: Vec<cyrup_mcp::config::ImportKind> = found
        .iter()
        .map(|entry| entry.kind)
        .filter(|kind| !existing.contains(kind.as_str()))
        .collect();

    // 3 — the report, printed BEFORE any decision so a no-op run still shows what was found.
    print_discovery(&ctx, &found);

    // 4 — `discoverHostConfigs && settings?.hostConfigDiscovery !== "on"`.
    let discovery_setting_changed = discover_host_configs
        && loaded
            .config
            .settings
            .as_ref()
            .and_then(|settings| settings.host_config_discovery)
            != Some(HostConfigDiscovery::On);

    // 5 — nothing to do, decided BEFORE either writer is reached.
    if to_add.is_empty() && !discovery_setting_changed {
        println!("\nNo cyrup config changes needed.");
        println!(
            "Standard MCP configs are discovered automatically, and host-specific imports are already configured or unavailable."
        );
        return 0;
    }

    // 6 and 7 — what is about to happen, in upstream's order.
    if !to_add.is_empty() {
        let kinds: Vec<&str> = to_add.iter().map(|kind| kind.as_str()).collect();
        println!(
            "\nDetected host configs to import into cyrup: {}",
            kinds.join(", ")
        );
    }
    if discovery_setting_changed {
        println!(
            "Opting in to host-specific fallback discovery (standard and cyrup-owned configs still take precedence)."
        );
    }

    // 8 — `--dry-run` is tested HERE, before either writer, not by rolling back after.
    let target = ctx.user_path();
    if dry_run {
        println!("Dry run: would update {}", target.display());
        return 0;
    }

    // 9 — the two writers.
    if !to_add.is_empty()
        && let Err(error) = ctx.ensure_compatibility_imports(&to_add)
    {
        eprintln!("{error}");
        return 1;
    }
    if discovery_setting_changed && let Err(error) = ctx.enable_host_config_discovery() {
        eprintln!("{error}");
        return 1;
    }

    // 10 — one unconditional explanatory line, one conditional.
    println!("Updated {}", target.display());
    println!(
        "cyrup will now keep reading standard MCP configs automatically, while these imports cover host-specific config formats."
    );
    if discovery_setting_changed {
        println!(
            "Host config discovery is explicit and does not write to or execute commands from external host files."
        );
    }
    0
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// The same shape `migrations.rs`'s test constructor uses.
    fn dirs(root: &std::path::Path) -> ConfigDirs {
        ConfigDirs {
            agent_dir: root.join("agent"),
            session_dir: root.join("agent/sessions"),
            session_dir_explicit: false,
            package_dir: root.join("agent/packages"),
            cwd: root.join("project"),
            home: root.to_path_buf(),
        }
    }

    /// The verb table (`cli.js:197-218`): the exit CODE is the contract, because
    /// `run_predispatch` returns it as the process's.
    #[test]
    fn the_verb_table_returns_upstreams_exit_codes() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        // No verb, and the three help spellings, are all success.
        assert_eq!(run(&[], &dirs), 0);
        for verb in ["help", "--help", "-h"] {
            assert_eq!(run(&[verb.to_string()], &dirs), 0, "{verb}");
        }
        // The retired downloader and an unknown verb both fail.
        assert_eq!(run(&["install".to_string()], &dirs), 1);
        assert_eq!(run(&["wibble".to_string()], &dirs), 1);
    }

    /// `--dry-run` is tested BEFORE either writer, not by rolling back after: nothing may reach the
    /// file system.
    #[test]
    fn a_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        std::fs::create_dir_all(&dirs.agent_dir).unwrap();
        let code = run(
            &[
                "init".to_string(),
                "--dry-run".to_string(),
                "--discover-host-configs".to_string(),
            ],
            &dirs,
        );
        assert_eq!(code, 0);
        let mcp_dirs = McpDirs::new(dirs.agent_dir.clone(), dirs.cwd.clone());
        let target = ConfigContext::new(mcp_dirs, None)
            .with_home(dirs.home.clone())
            .user_path();
        assert!(
            !target.exists(),
            "a dry run must not create {}",
            target.display()
        );
    }

    /// The second run is the idempotence contract: `enable_host_config_discovery` returns `false`
    /// and writes nothing, so the file's mtime is untouched.
    #[test]
    fn opting_in_twice_does_not_rewrite_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        std::fs::create_dir_all(&dirs.agent_dir).unwrap();
        let mcp_dirs = McpDirs::new(dirs.agent_dir.clone(), dirs.cwd.clone());
        let ctx = ConfigContext::new(mcp_dirs, None).with_home(dirs.home.clone());

        assert!(
            ctx.enable_host_config_discovery().unwrap(),
            "first call writes"
        );
        let first = std::fs::read_to_string(ctx.user_path()).unwrap();
        assert!(first.contains("hostConfigDiscovery"));

        assert!(
            !ctx.enable_host_config_discovery().unwrap(),
            "second call must report no change"
        );
        assert_eq!(
            std::fs::read_to_string(ctx.user_path()).unwrap(),
            first,
            "and must not rewrite the document"
        );
    }

    /// The writer MERGES rather than rewriting the document from a spread, so a key it does not own
    /// survives — the CYRUP-DELTA its doc records.
    #[test]
    fn enabling_discovery_preserves_unrelated_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        std::fs::create_dir_all(&dirs.agent_dir).unwrap();
        let mcp_dirs = McpDirs::new(dirs.agent_dir.clone(), dirs.cwd.clone());
        let ctx = ConfigContext::new(mcp_dirs, None).with_home(dirs.home.clone());
        let target = ctx.user_path();
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(
            &target,
            r#"{"mcpServers":{"keep":{"command":"true"}},"settings":{"requestTimeoutMs":1234}}"#,
        )
        .unwrap();

        assert!(ctx.enable_host_config_discovery().unwrap());
        let after = std::fs::read_to_string(&target).unwrap();
        assert!(
            after.contains("\"keep\""),
            "an unrelated server survived: {after}"
        );
        assert!(
            after.contains("requestTimeoutMs"),
            "a sibling setting survived: {after}"
        );
        assert!(
            after.contains("hostConfigDiscovery"),
            "and the new key landed: {after}"
        );
    }
}
