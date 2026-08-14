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

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use cyrup_config::{ConfigDirs, Settings, SettingsManager, SettingsScope, TrustStore};
use cyrup_resources::{
    discover, DiscoveryConfig, InstallScope, PackageManager, PackageSource, PackageStore,
    ResourceOrigin, ResourceOverrides, ResourceScope, UpdateTarget,
};
use cyrup_sdk::core::{CancelToken, PackageId};
use cyrup_tui::{
    run_startup_selector, ConfigKind, ConfigRow, ConfigScope, ConfigSelector, ConfigToggle,
    ConfigWriteScope, ProjectOverrideState,
};

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
            // The Pi `npm:@foo/bar` example is dropped here: `PackageSource::parse` hard-rejects the
            // `npm:` channel in the Rust port (source.rs:79-81 — no JS runtime, R-09-021), so an
            // `install npm:@foo/bar` is guaranteed to error. Advertising it in cyrup's OWN help would be
            // dead-but-advertised (gap-analysis 13-cyrup §D). The remaining git/https/ssh/path examples
            // mirror Pi's list (package-manager-cli.ts:104-110) minus that unsupported channel.
            "Usage:\n  {}\n\nInstall a package and add it to settings.\n\nOptions:\n  -l, --local       Install project-locally ({CFG}/settings.json)\n  -a, --approve     Trust project-local files for this command\n  -na, --no-approve Ignore project-local files for this command\n\nExamples:\n  {APP} install git:github.com/user/repo\n  {APP} install git:git@github.com:user/repo\n  {APP} install https://github.com/user/repo\n  {APP} install ssh://git@github.com/user/repo\n  {APP} install ./local/path\n",
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
        // Resolve trust the same way the other verbs do (Pi `createCommandSettingsManager`): the
        // `--approve`/`-a` override, else the saved decision for this folder. Project-scope toggles
        // require trust to persist (R-07-004).
        let trusted = trust_override(argv)
            .or(cli_trust_override)
            .unwrap_or_else(|| saved_trusted(dirs));
        // `-l` / `--local` opens the editor in PROJECT write scope (Pi `handleConfigCommand`,
        // package-manager-cli.ts:622-624,670) — the flag is the whole user-facing route to
        // `ConfigWriteScope::Project`, and upstream refuses it in an untrusted folder
        // (`:650-654`).
        let local = config_local_flag(argv);
        if local && !trusted {
            eprintln!("Project is not trusted. Use --approve to modify local resource config.");
            return Ok(Some(1));
        }
        return Ok(Some(run_config(dirs, trusted, local).await?));
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

/// `config`: open the interactive resource-config TUI (Pi `handleConfigCommand` → `selectConfig`,
/// package-manager-cli.ts:543-572). Resolves the settings + trust, discovers every top-level
/// auto-discovered skill/prompt/theme with its current enabled state, mounts the [`ConfigSelector`],
/// and persists each space/enter toggle as a `+pattern`/`-pattern` override entry into the matching
/// `skills`/`prompts`/`themes` settings array (Pi `toggleTopLevelResource`, config-selector.ts:457-503)
/// — the SAME arrays discovery's `global_overrides`/`project_overrides` already read back. Esc closes.
///
/// Package-tier resource toggling (Pi `togglePackageResource`) is out of this bin's crate scope — it
/// needs the installed-package → live-session wiring (`DiscoveryConfig.installed`, gap-07 §1) and
/// `PackageManager::set_enabled`, both in `cyrup-resources`/`cyrup-session-svc`.
async fn run_config(dirs: &ConfigDirs, trusted: bool, local: bool) -> Result<i32> {
    let settings = SettingsManager::load(crate::file_settings_store(dirs), Settings::new(), trusted);
    let rows = resolve_config_rows(dirs, &settings, trusted).await?;

    if rows.is_empty() {
        println!("No configurable skills, prompts, or themes found.");
        return Ok(0);
    }

    // Seed the per-(scope,kind) settings arrays from disk; each toggle read-modify-writes its own
    // scope's array (Pi's `getGlobalSettings()`/`getProjectSettings()` array reads, config-selector.ts).
    let mut arrays: HashMap<(ConfigScope, ConfigKind), Vec<String>> = HashMap::new();
    for kind in [ConfigKind::Skills, ConfigKind::Prompts, ConfigKind::Themes] {
        arrays.insert((ConfigScope::User, kind), settings_array(settings.global(), kind));
        arrays.insert((ConfigScope::Project, kind), settings_array(settings.project(), kind));
    }

    // `globalResolvedPaths` (`package-manager-cli.ts:655-660`): the SAME resolve run against a
    // settings manager with `projectTrusted: false`, i.e. the set a project would inherit. Its keys
    // are `inheritedEnabledByKey` (`config-selector.ts:262`), the second arm of
    // `isInheritedGlobalItem` (`:781-783`). Skipped when the project is untrusted, because then the
    // two resolves are the same object upstream (`:661-663`).
    let inherited_keys: Vec<String> = if trusted {
        let global_settings =
            SettingsManager::load(crate::file_settings_store(dirs), Settings::new(), false);
        resolve_config_rows(dirs, &global_settings, false)
            .await?
            .iter()
            .map(ConfigSelector::resource_key)
            .collect()
    } else {
        rows.iter().map(ConfigSelector::resource_key).collect()
    };

    // SEAM-066/067 — pi's `cli/config-selector.ts:22` runs the SAME `createStartupTui` preamble as
    // every other pre-launch screen: `setRegisteredThemes` + `initTheme(resolveThemeSetting(...))`
    // and `setKeybindings(KeybindingsManager.create())` (`cli/startup-ui.ts:78-81`). Hardwiring
    // `UiTheme::default()` (= `dark()`) and `SelectKeymap::default()` here gave a `"theme": "light"`
    // user a dark `cyrup config` and made its hint row name keys they had rebound.
    let theme = crate::startup_ui::startup_theme(dirs);
    let (keymap, _) = crate::startup_ui::startup_keymaps(dirs);
    // `getProjectOverrideState` for a top-level resource (`config-selector.ts:741-746`):
    // `getOverrideStateFromEntries(projectSettings[resourceType], patterns, false)` — scan the
    // PROJECT array for an entry naming this resource and read its marker, `!`/`-` ⇒ unload, else
    // load; no entry ⇒ inherit (`:759-772`).
    let override_states = config_override_states(&rows, &arrays);

    let mut selector = ConfigSelector::new(rows);
    // `writeScope: local ? "project" : "global"` and
    // `projectModeAvailable: settingsManager.isProjectTrusted()` (`package-manager-cli.ts:670-671`).
    // `projectModeAvailable` is what arms `Tab` and shows the `tab switch mode` hint
    // (`config-selector.ts:205,920-925`).
    if local {
        selector.set_write_scope(ConfigWriteScope::Project);
    }
    selector.set_project_mode_available(trusted);
    selector.set_inherited_global_keys(inherited_keys);
    for (i, state) in override_states.into_iter().enumerate() {
        selector.set_override_state(i, state);
    }
    let mut persist_err: Option<String> = None;
    run_startup_selector(&theme, &keymap, &mut selector, |payload| {
        let Some(toggle) = ConfigToggle::from_payload(payload) else {
            return;
        };
        let settings_scope = match toggle.scope {
            ConfigScope::User => SettingsScope::Global,
            ConfigScope::Project => SettingsScope::Project,
        };
        let entry = arrays.entry((toggle.scope, toggle.kind)).or_default();
        // Drop any prior +/-/! entry for this exact pattern, then push the new decision (Pi
        // `toggleTopLevelResource`, config-selector.ts:471-480): enabling writes `+pattern`, disabling
        // `-pattern`.
        entry.retain(|p| strip_override_marker(p) != toggle.pattern);
        entry.push(format!("{}{}", if toggle.enabled { '+' } else { '-' }, toggle.pattern));
        let value = serde_json::Value::Array(
            entry.iter().cloned().map(serde_json::Value::String).collect(),
        );
        if let Err(e) = settings.persist_nested(settings_scope, &[toggle.kind.key()], value) {
            persist_err = Some(e.to_string());
        }
    })?;

    if let Some(e) = persist_err {
        // A project toggle in an untrusted folder is the usual cause (Pi requires trust to write
        // project settings). Surface it after teardown rather than silently swallowing.
        eprintln!("Some changes could not be saved: {e}");
        eprintln!("(use --approve to modify project settings in an untrusted folder)");
        return Ok(1);
    }
    Ok(0)
}

/// The current `skills`/`prompts`/`themes` override array of a settings layer.
fn settings_array(layer: &Settings, kind: ConfigKind) -> Vec<String> {
    match kind {
        ConfigKind::Skills => layer.skill_paths(),
        ConfigKind::Prompts => layer.prompt_template_paths(),
        ConfigKind::Themes => layer.theme_paths(),
    }
}

/// `-l` / `--local`, the flag that opens `cyrup config` in PROJECT write scope (Pi
/// `handleConfigCommand`, package-manager-cli.ts:622-624 → `writeScope: local ? "project" :
/// "global"` at `:670`). The ONLY user-facing route to [`ConfigWriteScope::Project`].
fn config_local_flag(argv: &[String]) -> bool {
    argv.iter().any(|a| a == "-l" || a == "--local")
}

/// `getProjectOverrideState` for a top-level resource (Pi config-selector.ts:741-746 →
/// `getOverrideStateFromEntries`, `:759-772`): scan the **project** settings array for the
/// resource's `resourceType` and find the entries naming this resource; a `!`/`-` marker is an
/// unload, anything else a load, and no entry at all is inherit. Upstream's loop assigns rather
/// than breaks (`:766-770`), so the LAST matching entry wins.
///
/// `emptyArrayIsUnload` is upstream's `false` here — that arm is the package tier (`:755`), which
/// this editor does not manage.
fn config_override_states(
    rows: &[ConfigRow],
    arrays: &HashMap<(ConfigScope, ConfigKind), Vec<String>>,
) -> Vec<ProjectOverrideState> {
    rows.iter()
        .map(|row| {
            arrays
                .get(&(ConfigScope::Project, row.kind))
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter(|e| strip_override_marker(e) == row.pattern)
                .map(|e| match e.as_bytes().first() {
                    Some(b'!' | b'-') => ProjectOverrideState::Unload,
                    _ => ProjectOverrideState::Load,
                })
                .next_back()
                .unwrap_or(ProjectOverrideState::Inherit)
        })
        .collect()
}

/// Strip one leading `!`/`+`/`-` override marker (Pi's `p.slice(1)` for a marked pattern,
/// config-selector.ts:472).
fn strip_override_marker(p: &str) -> &str {
    match p.as_bytes().first() {
        Some(b'!' | b'+' | b'-') => p.get(1..).unwrap_or(p),
        _ => p,
    }
}

/// Resolve every top-level auto-discovered skill/prompt/theme with its **current** enabled state, for
/// the config editor. Runs discovery twice against the SAME dirs: once with the live settings override
/// patterns (the enabled set) and once with empty overrides (the full universe of files). A resource
/// is enabled iff it survived the override-filtered pass — reusing cyrup-resources' own enable/disable
/// logic without depending on its `pub(crate)` matcher. Mirrors Pi's `packageManager.resolve()`
/// returning every resource tagged with its `enabled` flag (package-manager.ts:881-897).
async fn resolve_config_rows(
    dirs: &ConfigDirs,
    settings: &SettingsManager,
    trusted: bool,
) -> Result<Vec<ConfigRow>> {
    let base = |overrides_global: ResourceOverrides, overrides_project: ResourceOverrides| {
        let mut disc = DiscoveryConfig::new(dirs.cwd.clone(), dirs.agent_dir.clone());
        disc.user_agents_dir = Some(dirs.home.join(".agents"));
        disc.trusted_project = trusted;
        disc.project_root = Some(dirs.cwd.clone());
        disc.package_global_dir = dirs.package_dir.clone();
        // `installed` is left empty: this editor manages only the top-level (loose) resources; the
        // package tier is the gated cross-crate piece (gap-07 §1/§3 piece 2).
        disc.global_overrides = overrides_global;
        disc.project_overrides = overrides_project;
        disc
    };
    let enabled_disc = base(
        ResourceOverrides {
            skills: settings.global().skill_paths(),
            prompts: settings.global().prompt_template_paths(),
            themes: settings.global().theme_paths(),
            extensions: settings.global().extension_paths(),
        },
        ResourceOverrides {
            skills: settings.project().skill_paths(),
            prompts: settings.project().prompt_template_paths(),
            themes: settings.project().theme_paths(),
            extensions: settings.project().extension_paths(),
        },
    );
    let universe_disc = base(ResourceOverrides::default(), ResourceOverrides::default());

    let enabled = discover(&enabled_disc, CancelToken::new()).await?;
    let universe = discover(&universe_disc, CancelToken::new()).await?;

    let enabled_skills: HashSet<_> =
        enabled.registry.skills.all().iter().map(|s| s.skill_md.clone()).collect();
    let enabled_prompts: HashSet<_> =
        enabled.registry.prompts.all().iter().map(|p| p.path.clone()).collect();
    let enabled_themes: HashSet<_> =
        enabled.registry.themes.all().iter().filter_map(|t| t.origin_path.clone()).collect();

    let mut rows = Vec::new();
    let mut seen: HashSet<(ConfigKind, std::path::PathBuf)> = HashSet::new();

    for skill in universe.registry.skills.all() {
        let ResourceOrigin::LooseFile { scope, root } = &skill.origin else { continue };
        let Some((cscope, pattern, base_dir)) = loose_pattern(*scope, root, &skill.skill_md, dirs) else {
            continue;
        };
        if !seen.insert((ConfigKind::Skills, skill.skill_md.clone())) {
            continue;
        }
        rows.push(ConfigRow {
            scope: cscope,
            kind: ConfigKind::Skills,
            display_name: skill_display_name(&skill.skill_md),
            pattern,
            base_dir,
            enabled: enabled_skills.contains(&skill.skill_md),
        });
    }
    for prompt in universe.registry.prompts.all() {
        let ResourceOrigin::LooseFile { scope, root } = &prompt.origin else { continue };
        let Some((cscope, pattern, base_dir)) = loose_pattern(*scope, root, &prompt.path, dirs) else {
            continue;
        };
        if !seen.insert((ConfigKind::Prompts, prompt.path.clone())) {
            continue;
        }
        rows.push(ConfigRow {
            scope: cscope,
            kind: ConfigKind::Prompts,
            display_name: file_display_name(&prompt.path),
            pattern,
            base_dir,
            enabled: enabled_prompts.contains(&prompt.path),
        });
    }
    for theme in universe.registry.themes.all() {
        let ResourceOrigin::LooseFile { scope, root } = &theme.origin else { continue };
        let Some(path) = theme.origin_path.clone() else { continue };
        let Some((cscope, pattern, base_dir)) = loose_pattern(*scope, root, &path, dirs) else {
            continue;
        };
        if !seen.insert((ConfigKind::Themes, path.clone())) {
            continue;
        }
        rows.push(ConfigRow {
            scope: cscope,
            kind: ConfigKind::Themes,
            display_name: file_display_name(&path),
            pattern,
            base_dir,
            enabled: enabled_themes.contains(&path),
        });
    }
    Ok(rows)
}

/// Map a loose-file resource's `(scope, root, path)` to its config-editor `(scope, pattern, base dir)`.
/// The pattern is the resource path relative to the resource root's PARENT (`root.parent()`), matching
/// exactly the base cyrup-resources' `is_enabled_by_overrides` uses (discovery.rs `override_enabled`
/// = `root.parent()`), so a written `+`/`-pattern` round-trips through the next discovery. Only the
/// auto-discovered `Global`/`Project` tiers are editable here (settings-listed positive entries and
/// packages are skipped).
fn loose_pattern(
    scope: ResourceScope,
    root: &Path,
    path: &Path,
    dirs: &ConfigDirs,
) -> Option<(ConfigScope, String, String)> {
    let cscope = match scope {
        ResourceScope::Global => ConfigScope::User,
        ResourceScope::Project => ConfigScope::Project,
        _ => return None,
    };
    let base = root.parent()?;
    let pattern = to_posix(path.strip_prefix(base).ok()?);
    Some((cscope, pattern, display_base_dir(base, &dirs.home)))
}

/// Posix-normalize a path for a settings pattern (Pi `toPosixPath`, package-manager.ts:212-214).
fn to_posix(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// A home-relative display of a base dir (Pi `formatBaseDir`, config-selector.ts:59-74): `~`-prefixed
/// when under the home dir, with a trailing slash.
fn display_base_dir(base: &Path, home: &Path) -> String {
    let shown = if base == home {
        "~".to_string()
    } else if let Ok(rest) = base.strip_prefix(home) {
        format!("~/{}", to_posix(rest))
    } else {
        to_posix(base)
    };
    if shown.ends_with('/') {
        shown
    } else {
        format!("{shown}/")
    }
}

/// A skill's display name (Pi config-selector.ts:129-133): the parent directory for a `SKILL.md`,
/// else the file name.
fn skill_display_name(path: &Path) -> String {
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if file == "SKILL.md" {
        path.parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or(file)
    } else {
        file
    }
}

/// A prompt/theme display name: the file name (Pi config-selector.ts:132).
fn file_display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
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
    fn config_local_flag_is_the_only_route_to_project_write_scope() {
        // Pi `handleConfigCommand` (package-manager-cli.ts:622-624): both spellings, and nothing
        // else. `cyrup config` alone stays in global scope, which is upstream's default (`:670`).
        assert!(config_local_flag(&v(&["config", "--local"])));
        assert!(config_local_flag(&v(&["config", "-l"])));
        assert!(!config_local_flag(&v(&["config"])));
        assert!(!config_local_flag(&v(&["config", "--approve"])));
    }

    /// `getProjectOverrideState` → `getOverrideStateFromEntries` (config-selector.ts:741-772): the
    /// `+`/`-` entries already in the PROJECT settings array are what the `[+]`/`[-]` checkboxes
    /// and the `  project load` / `  project unload` suffixes report. Before this wiring the
    /// selector's override vector was all-`Inherit` in the product, so project scope drew
    /// identically to global no matter what the settings file said.
    #[test]
    fn config_override_states_read_the_project_settings_arrays() {
        let row = |kind: ConfigKind, pattern: &str| ConfigRow {
            scope: ConfigScope::Project,
            kind,
            display_name: pattern.to_string(),
            pattern: pattern.to_string(),
            base_dir: "/repo/.cyrup/".to_string(),
            enabled: true,
        };
        let rows = vec![
            row(ConfigKind::Skills, "skills/a/SKILL.md"),
            row(ConfigKind::Skills, "skills/b/SKILL.md"),
            row(ConfigKind::Skills, "skills/c/SKILL.md"),
            row(ConfigKind::Prompts, "prompts/p.md"),
        ];
        let mut arrays: HashMap<(ConfigScope, ConfigKind), Vec<String>> = HashMap::new();
        arrays.insert(
            (ConfigScope::Project, ConfigKind::Skills),
            vec![
                "+skills/a/SKILL.md".to_string(),
                "-skills/b/SKILL.md".to_string(),
                // `!` is the third marker `strip_override_marker` accepts (`:838-840`).
                "!skills/c/SKILL.md".to_string(),
            ],
        );
        // The GLOBAL array must not leak into the project override state (`:743` reads
        // `getProjectSettings()`).
        arrays.insert(
            (ConfigScope::User, ConfigKind::Prompts),
            vec!["+prompts/p.md".to_string()],
        );
        assert_eq!(
            config_override_states(&rows, &arrays),
            vec![
                ProjectOverrideState::Load,
                ProjectOverrideState::Unload,
                ProjectOverrideState::Unload,
                ProjectOverrideState::Inherit,
            ]
        );
    }

    #[test]
    fn config_override_states_take_the_last_matching_entry() {
        // `:765-770` assigns instead of breaking, so a later entry supersedes an earlier one.
        let rows = vec![ConfigRow {
            scope: ConfigScope::Project,
            kind: ConfigKind::Skills,
            display_name: "a".to_string(),
            pattern: "skills/a/SKILL.md".to_string(),
            base_dir: "/repo/.cyrup/".to_string(),
            enabled: true,
        }];
        let mut arrays: HashMap<(ConfigScope, ConfigKind), Vec<String>> = HashMap::new();
        arrays.insert(
            (ConfigScope::Project, ConfigKind::Skills),
            vec!["-skills/a/SKILL.md".to_string(), "+skills/a/SKILL.md".to_string()],
        );
        assert_eq!(config_override_states(&rows, &arrays), vec![ProjectOverrideState::Load]);
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
