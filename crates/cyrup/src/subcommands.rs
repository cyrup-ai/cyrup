//! Package + config subcommands (Pi `package-manager-cli.ts`): `install` / `remove` / `uninstall`
//! (alias) / `update` / `list` / `config`. Pi dispatches these BEFORE arg parsing (main.ts:486), so
//! the bin peeks the first non-flag token and, when it is a known subcommand, runs it and exits
//! instead of falling through to the interactive/one-shot CLI. Wired to the cyrup-resources
//! [`PackageManager`].
//!
//! This is a 1:1 port of `parsePackageCommand` + `handlePackageCommand` (package-manager-cli.ts):
//! per-command `--help`, the `invalidOption`/`missingOptionValue`/`invalidArgument`/
//! `conflictingOptions` diagnostics (each with a usage line + exit 1), the `update` target matrix
//! (`--self`/`--extensions`/`--all`/`--extension <source>` combos), the User/Project-grouped `list`
//! with `(filtered)` tags + dim installed paths, and the saved-trust-store lookup + project-write
//! guard. The interactive trust prompt + the binary self-update are the gated tails (residual ledger).

use anyhow::Result;
use cyrup_config::{ConfigDirs, TrustStore};
use cyrup_resources::{InstallScope, PackageManager, PackageSource, PackageStore, UpdateTarget};
use cyrup_sdk::core::{CancelToken, PackageId};

/// The recognized subcommand verbs (Pi: `install`/`remove`/`update`/`list` + `uninstall` alias +
/// `config`). `config` is dispatched specially (Pi `handleConfigCommand`).
const SUBCOMMANDS: [&str; 6] = ["install", "remove", "uninstall", "update", "list", "config"];

/// Whether `argv` (program name already stripped) begins with a package/config subcommand.
pub fn first_subcommand(argv: &[String]) -> Option<&str> {
    let first = argv.first()?;
    if first.starts_with('-') || first.starts_with('@') {
        return None;
    }
    SUBCOMMANDS.iter().copied().find(|&c| c == first.as_str())
}

/// Parse `--approve`/`-a` / `--no-approve`/`-na` anywhere in the args (Pi `parseProjectTrustOverride`,
/// package-manager-cli.ts:464-474). The last one wins.
pub fn trust_override(argv: &[String]) -> Option<bool> {
    let mut over: Option<bool> = None;
    for a in argv {
        match a.as_str() {
            "--approve" | "-a" => over = Some(true),
            "--no-approve" | "-na" => over = Some(false),
            _ => {}
        }
    }
    over
}

/// The verb a package command targets (Pi `PackageCommand`; `config` is dispatched separately).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageCommand {
    Install,
    Remove,
    Update,
    List,
}

impl PackageCommand {
    fn name(self) -> &'static str {
        match self {
            PackageCommand::Install => "install",
            PackageCommand::Remove => "remove",
            PackageCommand::Update => "update",
            PackageCommand::List => "list",
        }
    }
}

/// The `update` target (Pi `UpdateTarget = {all} | {self} | {extensions, source?}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateTargetSel {
    All,
    SelfUpdate,
    Extensions(Option<String>),
}

/// A parsed package command + its diagnostics (Pi `PackageCommandOptions`,
/// package-manager-cli.ts:52-65).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCommand {
    pub command: PackageCommand,
    pub source: Option<String>,
    pub update_target: Option<UpdateTargetSel>,
    pub show_extensions_skipped_note: bool,
    pub local: bool,
    pub force: bool,
    pub project_trust_override: Option<bool>,
    pub help: bool,
    pub invalid_option: Option<String>,
    pub invalid_argument: Option<String>,
    pub missing_option_value: Option<String>,
    pub conflicting_options: Option<String>,
}

/// Parse a package command (Pi `parsePackageCommand`, package-manager-cli.ts:168-347). Returns `None`
/// when the first token is not a package verb (the `config` verb is handled separately).
pub fn parse_package_command(argv: &[String]) -> Option<ParsedCommand> {
    let (raw, rest) = argv.split_first()?;
    let command = match raw.as_str() {
        "uninstall" | "remove" => PackageCommand::Remove,
        "install" => PackageCommand::Install,
        "update" => PackageCommand::Update,
        "list" => PackageCommand::List,
        _ => return None,
    };

    let mut local = false;
    let mut force = false;
    let mut project_trust_override: Option<bool> = None;
    let mut help = false;
    let mut invalid_option: Option<String> = None;
    let mut invalid_argument: Option<String> = None;
    let mut missing_option_value: Option<String> = None;
    let mut conflicting_options: Option<String> = None;
    let mut source: Option<String> = None;
    let mut self_flag = false;
    let mut extensions_flag = false;
    let mut all_flag = false;
    let mut extension_flag_source: Option<String> = None;

    let mut index = 0usize;
    while let Some(arg) = rest.get(index) {
        match arg.as_str() {
            "-h" | "--help" => help = true,
            "-l" | "--local" => {
                if matches!(command, PackageCommand::Install | PackageCommand::Remove) {
                    local = true;
                } else {
                    invalid_option.get_or_insert_with(|| arg.clone());
                }
            }
            "--self" => {
                if command == PackageCommand::Update {
                    self_flag = true;
                } else {
                    invalid_option.get_or_insert_with(|| arg.clone());
                }
            }
            "--extensions" => {
                if command == PackageCommand::Update {
                    extensions_flag = true;
                } else {
                    invalid_option.get_or_insert_with(|| arg.clone());
                }
            }
            "--all" => {
                if command == PackageCommand::Update {
                    all_flag = true;
                } else {
                    invalid_option.get_or_insert_with(|| arg.clone());
                }
            }
            "--approve" | "-a" => project_trust_override = Some(true),
            "--no-approve" | "-na" => project_trust_override = Some(false),
            "--force" => {
                if command == PackageCommand::Update {
                    force = true;
                } else {
                    invalid_option.get_or_insert_with(|| arg.clone());
                }
            }
            "--extension" => {
                if command != PackageCommand::Update {
                    invalid_option.get_or_insert_with(|| arg.clone());
                } else {
                    match rest.get(index + 1) {
                        None => {
                            missing_option_value.get_or_insert_with(|| arg.clone());
                        }
                        Some(value) if value.starts_with('-') => {
                            missing_option_value.get_or_insert_with(|| arg.clone());
                        }
                        Some(value) => {
                            if extension_flag_source.is_some() {
                                conflicting_options.get_or_insert_with(|| {
                                    "--extension can only be provided once".to_string()
                                });
                                index += 1;
                            } else {
                                extension_flag_source = Some(value.clone());
                                index += 1;
                            }
                        }
                    }
                }
            }
            other if other.starts_with('-') => {
                invalid_option.get_or_insert_with(|| arg.clone());
            }
            _ => {
                if source.is_none() {
                    source = Some(arg.clone());
                } else {
                    invalid_argument.get_or_insert_with(|| arg.clone());
                }
            }
        }
        index += 1;
    }

    let mut update_target: Option<UpdateTargetSel> = None;
    let mut show_extensions_skipped_note = false;
    if command == PackageCommand::Update {
        if all_flag && (self_flag || extensions_flag || extension_flag_source.is_some()) {
            conflicting_options.get_or_insert_with(|| {
                "--all cannot be combined with --self, --extensions, or --extension".to_string()
            });
        }
        if all_flag && source.is_some() {
            conflicting_options.get_or_insert_with(|| {
                "--all cannot be combined with a positional source".to_string()
            });
        }

        if let Some(ext_source) = extension_flag_source.clone() {
            if self_flag || extensions_flag || all_flag {
                conflicting_options.get_or_insert_with(|| {
                    "--extension cannot be combined with --self, --extensions, or --all".to_string()
                });
            }
            if source.is_some() {
                conflicting_options.get_or_insert_with(|| {
                    "--extension cannot be combined with a positional source".to_string()
                });
            }
            update_target = Some(UpdateTargetSel::Extensions(Some(ext_source)));
        } else if let Some(src) = source.clone() {
            let source_is_self = src == "self" || src == "pi" || src == "cyrup";
            if source_is_self {
                update_target = Some(if extensions_flag {
                    UpdateTargetSel::All
                } else {
                    UpdateTargetSel::SelfUpdate
                });
            } else {
                if extensions_flag || self_flag || all_flag {
                    conflicting_options.get_or_insert_with(|| {
                        "positional update targets cannot be combined with --self, --extensions, or --all".to_string()
                    });
                }
                update_target = Some(UpdateTargetSel::Extensions(Some(src)));
            }
        } else if all_flag || (self_flag && extensions_flag) {
            // Bare `--all`, or `--self --extensions` together, both mean "update everything".
            update_target = Some(UpdateTargetSel::All);
        } else if self_flag {
            update_target = Some(UpdateTargetSel::SelfUpdate);
        } else if extensions_flag {
            update_target = Some(UpdateTargetSel::Extensions(None));
        } else {
            update_target = Some(UpdateTargetSel::SelfUpdate);
            show_extensions_skipped_note = true;
        }
    }

    Some(ParsedCommand {
        command,
        source,
        update_target,
        show_extensions_skipped_note,
        local,
        force,
        project_trust_override,
        help,
        invalid_option,
        invalid_argument,
        missing_option_value,
        conflicting_options,
    })
}

/// The usage line for a command (Pi `getPackageCommandUsage`, package-manager-cli.ts:77-88).
pub fn usage(command: PackageCommand) -> String {
    const APP: &str = "cyrup";
    match command {
        PackageCommand::Install => format!("{APP} install <source> [-l] [--approve|--no-approve]"),
        PackageCommand::Remove => format!("{APP} remove <source> [-l] [--approve|--no-approve]"),
        PackageCommand::Update => format!(
            "{APP} update [source|self|pi] [--self|--extensions|--all] [--extension <source>] [--approve|--no-approve] [--force]"
        ),
        PackageCommand::List => format!("{APP} list [--approve|--no-approve]"),
    }
}

/// Per-command `--help` body (Pi `printPackageCommandHelp`, package-manager-cli.ts:90-166).
pub fn render_command_help(command: PackageCommand) -> String {
    const APP: &str = "cyrup";
    const CFG: &str = ".cyrup";
    match command {
        PackageCommand::Install => format!(
            "Usage:\n  {}\n\nInstall a package and add it to settings.\n\nOptions:\n  -l, --local       Install project-locally ({CFG}/settings.json)\n  -a, --approve     Trust project-local files for this command\n  -na, --no-approve Ignore project-local files for this command\n\nExamples:\n  {APP} install npm:@foo/bar\n  {APP} install git:github.com/user/repo\n  {APP} install git:git@github.com:user/repo\n  {APP} install https://github.com/user/repo\n  {APP} install ssh://git@github.com/user/repo\n  {APP} install ./local/path\n",
            usage(command)
        ),
        PackageCommand::Remove => format!(
            "Usage:\n  {}\n\nRemove a package and its source from settings.\nAlias: {APP} uninstall <source> [-l]\n\nOptions:\n  -l, --local       Remove from project settings ({CFG}/settings.json)\n  -a, --approve     Trust project-local files for this command\n  -na, --no-approve Ignore project-local files for this command\n\nExamples:\n  {APP} remove npm:@foo/bar\n  {APP} uninstall npm:@foo/bar\n",
            usage(command)
        ),
        PackageCommand::Update => format!(
            "Usage:\n  {}\n\nUpdate {APP} and installed packages.\n\nOptions:\n  --self                  Update {APP} only (default when no target is given)\n  --extensions            Update installed packages only\n  --all                   Update {APP} and installed packages\n  --extension <source>    Update one package only\n  -a, --approve           Trust project-local files for this command\n  -na, --no-approve       Ignore project-local files for this command\n  --force                 Reinstall {APP} even if the current version is latest\n\nShort forms:\n  {APP} update                Update {APP} only\n  {APP} update --all          Update {APP} and all extensions\n  {APP} update <source>       Update one package\n  {APP} update pi             Update {APP} only (self works as alias to pi)\n",
            usage(command)
        ),
        PackageCommand::List => format!(
            "Usage:\n  {}\n\nList installed packages from user and project settings.\n\nOptions:\n  -a, --approve      Trust project-local files for this command\n  -na, --no-approve  Ignore project-local files for this command\n",
            usage(command)
        ),
    }
}

/// Render a package source for the `list` output (Pi shows the original `pkg.source` string).
fn source_display(source: &PackageSource) -> String {
    match source {
        PackageSource::Git { url, .. } => url.clone(),
        PackageSource::Path { path } => path.display().to_string(),
        PackageSource::Oci { reference } => reference.clone(),
    }
}

/// The saved project-trust decision for `cwd` from the trust store (Pi `trustStore.get(cwd) === true`).
fn saved_trusted(dirs: &ConfigDirs) -> bool {
    let store = TrustStore::new(dirs.trust_path());
    store
        .nearest(&dirs.cwd)
        .ok()
        .flatten()
        .map(|entry| entry.decision.is_trusted())
        .unwrap_or(false)
}

/// Top-level entry: if `argv` begins with a subcommand, parse + run it and return the exit code;
/// otherwise return `None` so the caller falls through to the normal CLI.
pub async fn dispatch(
    argv: &[String],
    dirs: &ConfigDirs,
    cli_trust_override: Option<bool>,
) -> Result<Option<i32>> {
    // `config` is handled specially (Pi `handleConfigCommand`).
    if argv.first().map(String::as_str) == Some("config") {
        return Ok(Some(run_config(dirs).await?));
    }

    let Some(opts) = parse_package_command(argv) else {
        return Ok(None);
    };

    if opts.help {
        print!("{}", render_command_help(opts.command));
        return Ok(Some(0));
    }
    if let Some(flag) = &opts.invalid_option {
        eprintln!("Unknown option {flag} for \"{}\".", opts.command.name());
        eprintln!("Use \"cyrup --help\" or \"{}\".", usage(opts.command));
        return Ok(Some(1));
    }
    if let Some(flag) = &opts.missing_option_value {
        eprintln!("Missing value for {flag}.");
        eprintln!("Usage: {}", usage(opts.command));
        return Ok(Some(1));
    }
    if let Some(arg) = &opts.invalid_argument {
        eprintln!("Unexpected argument {arg}.");
        eprintln!("Usage: {}", usage(opts.command));
        return Ok(Some(1));
    }
    if let Some(msg) = &opts.conflicting_options {
        eprintln!("{msg}");
        eprintln!("Usage: {}", usage(opts.command));
        return Ok(Some(1));
    }
    if matches!(
        opts.command,
        PackageCommand::Install | PackageCommand::Remove
    ) && opts.source.is_none()
    {
        eprintln!("Missing {} source.", opts.command.name());
        eprintln!("Usage: {}", usage(opts.command));
        return Ok(Some(1));
    }

    // Saved-trust lookup + override (Pi createCommandSettingsManager); the interactive trust prompt
    // for trust-requiring project resources is the gated outer layer (residual ledger #19).
    let trust_override = opts.project_trust_override.or(cli_trust_override);
    let effective_trusted = trust_override.unwrap_or_else(|| saved_trusted(dirs));

    // Project-package-config write requires trust (Pi package-manager-cli.ts:635-639).
    let writes_project_config = matches!(
        opts.command,
        PackageCommand::Install | PackageCommand::Remove
    ) && opts.local;
    if writes_project_config && !effective_trusted {
        eprintln!("Project is not trusted. Use --approve to modify local package config.");
        return Ok(Some(1));
    }

    Ok(Some(run(&opts, dirs, effective_trusted).await?))
}

/// Execute a parsed command against the package manager rooted at the resolved dirs.
async fn run(opts: &ParsedCommand, dirs: &ConfigDirs, trusted: bool) -> Result<i32> {
    let store = PackageStore::new(dirs.package_dir.clone(), Some(dirs.cwd.clone()));
    let manager = PackageManager::new(store);
    let cancel = CancelToken::new();
    let scope = if opts.local {
        InstallScope::Project
    } else {
        InstallScope::Global
    };

    match opts.command {
        PackageCommand::Install => {
            let source = opts.source.clone().unwrap_or_default();
            let parsed =
                PackageSource::parse(&source).map_err(|e| anyhow::anyhow!("install: {e}"))?;
            let (_pkg, notice) = manager.install(parsed, scope, trusted, cancel).await?;
            println!("Installed {source}");
            println!("{}", notice.message);
            Ok(0)
        }
        PackageCommand::Remove => {
            let source = opts.source.clone().unwrap_or_default();
            let id = PackageId::from(source.as_str());
            match manager.remove(&id).await {
                Ok(()) => {
                    println!("Removed {source}");
                    Ok(0)
                }
                Err(_) => {
                    eprintln!("No matching package found for {source}");
                    Ok(1)
                }
            }
        }
        PackageCommand::List => {
            print_list(&manager, dirs);
            Ok(0)
        }
        PackageCommand::Update => run_update(opts, &manager, cancel).await,
    }
}

/// `list`: User/Project-grouped, `(filtered)`-tagged, dim-installed-path block (Pi
/// package-manager-cli.ts:669-703).
fn print_list(manager: &PackageManager, dirs: &ConfigDirs) {
    let packages = manager.list();
    if packages.is_empty() {
        println!("No packages installed.");
        return;
    }
    let store = PackageStore::new(dirs.package_dir.clone(), Some(dirs.cwd.clone()));
    let user: Vec<_> = packages
        .iter()
        .filter(|p| p.scope == InstallScope::Global)
        .collect();
    let project: Vec<_> = packages
        .iter()
        .filter(|p| p.scope == InstallScope::Project)
        .collect();

    let format_one = |pkg: &cyrup_resources::InstalledPackage| {
        let filtered = !pkg.disabled.skills.is_empty()
            || !pkg.disabled.prompts.is_empty()
            || !pkg.disabled.themes.is_empty()
            || !pkg.disabled.extensions.is_empty();
        let display = source_display(&pkg.source);
        if filtered {
            println!("  {display} (filtered)");
        } else {
            println!("  {display}");
        }
        if let Some(path) = store.package_dir(pkg.scope, &pkg.id)
            && path.exists()
        {
            println!("    {}", path.display());
        }
    };

    if !user.is_empty() {
        println!("User packages:");
        for pkg in &user {
            format_one(pkg);
        }
    }
    if !project.is_empty() {
        if !user.is_empty() {
            println!();
        }
        println!("Project packages:");
        for pkg in &project {
            format_one(pkg);
        }
    }
}

/// `update`: the target matrix (Pi package-manager-cli.ts:705-763). The self/binary update is the
/// deferred distribution tail (residual ledger #26): it reports rather than downloading.
async fn run_update(
    opts: &ParsedCommand,
    manager: &PackageManager,
    cancel: CancelToken,
) -> Result<i32> {
    let target = opts
        .update_target
        .clone()
        .unwrap_or(UpdateTargetSel::SelfUpdate);
    if opts.show_extensions_skipped_note {
        println!("Extensions are skipped. Run cyrup update --extensions to update extensions.");
    }
    let includes_extensions = matches!(
        target,
        UpdateTargetSel::All | UpdateTargetSel::Extensions(_)
    );
    let includes_self = matches!(target, UpdateTargetSel::All | UpdateTargetSel::SelfUpdate);

    let mut code = 0;
    if includes_extensions {
        let update_source = match &target {
            UpdateTargetSel::Extensions(Some(src)) => Some(src.clone()),
            _ => None,
        };
        let report = match &update_source {
            Some(src) => {
                manager
                    .update(
                        UpdateTarget::One(PackageId::from(src.as_str())),
                        cancel.clone(),
                    )
                    .await?
            }
            None => manager.update(UpdateTarget::All, cancel.clone()).await?,
        };
        for id in &report.skipped_pinned {
            println!("Skipped (pinned) {id}");
        }
        for (id, err) in &report.failed {
            eprintln!("Failed {id}: {err}");
        }
        if !report.failed.is_empty() {
            code = 1;
        }
        match &update_source {
            Some(src) => println!("Updated {src}"),
            None => println!("Updated packages"),
        }
    }
    if includes_self {
        // The binary self-update (download + replace) is the deferred distribution tail; report.
        println!(
            "Self-update is not available in this build; update cyrup via your package manager."
        );
    }
    Ok(code)
}

/// `config`: Pi opens the interactive resource-config TUI (`selectConfig`) — the ext-UI dialog host,
/// a gated outer layer (residual ledger). The CLI surfaces a read-only summary of installs.
async fn run_config(dirs: &ConfigDirs) -> Result<i32> {
    let store = PackageStore::new(dirs.package_dir.clone(), Some(dirs.cwd.clone()));
    let manager = PackageManager::new(store);
    let packages = manager.list();
    println!("Installed packages ({}):", packages.len());
    for pkg in packages {
        println!("  {}  [{:?}]", pkg.id, pkg.scope);
    }
    Ok(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::Path;

    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn dirs(root: &Path) -> ConfigDirs {
        ConfigDirs {
            agent_dir: root.join("agent"),
            session_dir: root.join("agent/sessions"),
            session_dir_explicit: false,
            package_dir: root.join("agent/packages"),
            cwd: root.join("work"),
            home: root.to_path_buf(),
        }
    }

    #[test]
    fn detects_only_known_subcommands() {
        assert_eq!(first_subcommand(&v(&["install", "x"])), Some("install"));
        assert_eq!(first_subcommand(&v(&["list"])), Some("list"));
        assert_eq!(first_subcommand(&v(&["config"])), Some("config"));
        assert_eq!(first_subcommand(&v(&["hello world"])), None);
        assert_eq!(first_subcommand(&v(&["--print", "hi"])), None);
        assert_eq!(first_subcommand(&v(&["@file.md"])), None);
        assert_eq!(first_subcommand(&[]), None);
    }

    #[test]
    fn install_scope_approve_and_missing_source() {
        let c = parse_package_command(&v(&["install", "./pkg", "-l", "--approve"])).unwrap();
        assert_eq!(c.command, PackageCommand::Install);
        assert_eq!(c.source.as_deref(), Some("./pkg"));
        assert!(c.local);
        assert_eq!(c.project_trust_override, Some(true));
        // Missing source is detected at dispatch (the parser leaves source = None).
        let none = parse_package_command(&v(&["install"])).unwrap();
        assert!(none.source.is_none());
        // Unknown option is a diagnostic.
        let bad = parse_package_command(&v(&["install", "x", "--bogus"])).unwrap();
        assert_eq!(bad.invalid_option.as_deref(), Some("--bogus"));
        // -l on a non-install/remove verb is an invalid option.
        let badl = parse_package_command(&v(&["update", "-l"])).unwrap();
        assert_eq!(badl.invalid_option.as_deref(), Some("-l"));
    }

    #[test]
    fn uninstall_is_remove_alias() {
        assert_eq!(
            parse_package_command(&v(&["uninstall", "pkg"]))
                .unwrap()
                .command,
            PackageCommand::Remove
        );
    }

    #[test]
    fn update_target_matrix() {
        let bare = parse_package_command(&v(&["update"])).unwrap();
        assert_eq!(bare.update_target, Some(UpdateTargetSel::SelfUpdate));
        assert!(bare.show_extensions_skipped_note);

        assert_eq!(
            parse_package_command(&v(&["update", "self"]))
                .unwrap()
                .update_target,
            Some(UpdateTargetSel::SelfUpdate)
        );
        assert_eq!(
            parse_package_command(&v(&["update", "pi"]))
                .unwrap()
                .update_target,
            Some(UpdateTargetSel::SelfUpdate)
        );
        assert_eq!(
            parse_package_command(&v(&["update", "--all"]))
                .unwrap()
                .update_target,
            Some(UpdateTargetSel::All)
        );
        assert_eq!(
            parse_package_command(&v(&["update", "--extensions"]))
                .unwrap()
                .update_target,
            Some(UpdateTargetSel::Extensions(None))
        );
        // self + extensions ⇒ all.
        assert_eq!(
            parse_package_command(&v(&["update", "--self", "--extensions"]))
                .unwrap()
                .update_target,
            Some(UpdateTargetSel::All)
        );
        // --extension <source> value form.
        assert_eq!(
            parse_package_command(&v(&["update", "--extension", "my-pkg"]))
                .unwrap()
                .update_target,
            Some(UpdateTargetSel::Extensions(Some("my-pkg".into())))
        );
        // positional source ⇒ one extension.
        assert_eq!(
            parse_package_command(&v(&["update", "my-pkg"]))
                .unwrap()
                .update_target,
            Some(UpdateTargetSel::Extensions(Some("my-pkg".into())))
        );
    }

    #[test]
    fn update_conflict_and_missing_value_diagnostics() {
        let conflict = parse_package_command(&v(&["update", "--all", "--self"])).unwrap();
        assert!(conflict.conflicting_options.is_some());
        let missing = parse_package_command(&v(&["update", "--extension"])).unwrap();
        assert_eq!(missing.missing_option_value.as_deref(), Some("--extension"));
        let missing2 = parse_package_command(&v(&["update", "--extension", "--all"])).unwrap();
        assert_eq!(
            missing2.missing_option_value.as_deref(),
            Some("--extension")
        );
        let ext_conflict =
            parse_package_command(&v(&["update", "--extension", "a", "--all"])).unwrap();
        assert!(ext_conflict.conflicting_options.is_some());
    }

    #[test]
    fn help_flag_and_usage_lines() {
        assert!(
            parse_package_command(&v(&["install", "--help"]))
                .unwrap()
                .help
        );
        assert!(render_command_help(PackageCommand::Install).contains("Install a package"));
        assert!(render_command_help(PackageCommand::Update).contains("--extension <source>"));
        assert!(usage(PackageCommand::Install).contains("install <source>"));
    }

    #[test]
    fn trust_override_parsing() {
        assert_eq!(
            trust_override(&v(&["install", "x", "--approve"])),
            Some(true)
        );
        assert_eq!(trust_override(&v(&["install", "x", "-na"])), Some(false));
        assert_eq!(trust_override(&v(&["install", "x"])), None);
    }

    #[tokio::test]
    async fn list_dispatch_runs_against_empty_store() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        let code = dispatch(&v(&["list"]), &d, None).await.unwrap();
        assert_eq!(code, Some(0));
    }

    #[tokio::test]
    async fn project_write_without_trust_is_guarded() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        // install -l with no saved trust + no --approve → guarded (exit 1).
        let code = dispatch(&v(&["install", "./pkg", "-l"]), &d, None)
            .await
            .unwrap();
        assert_eq!(code, Some(1));
    }

    #[tokio::test]
    async fn diagnostics_return_exit_1() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        assert_eq!(dispatch(&v(&["install"]), &d, None).await.unwrap(), Some(1));
        assert_eq!(
            dispatch(&v(&["install", "x", "--bogus"]), &d, None)
                .await
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            dispatch(&v(&["update", "--all", "--self"]), &d, None)
                .await
                .unwrap(),
            Some(1)
        );
        // help exits 0.
        assert_eq!(
            dispatch(&v(&["install", "--help"]), &d, None)
                .await
                .unwrap(),
            Some(0)
        );
    }
}
