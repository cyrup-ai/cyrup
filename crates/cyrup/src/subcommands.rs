//! Package + config subcommands (Pi `package-manager-cli.ts`): `install` / `remove` / `uninstall`
//! (alias) / `update` / `list` / `config`. Pi dispatches these BEFORE arg parsing (main.ts:486), so
//! the bin peeks the first non-flag token and, when it is a known subcommand, runs it and exits
//! instead of falling through to the interactive/one-shot CLI. Wired to the cyrup-resources
//! [`PackageManager`].
//!
//! This is a 1:1 port of `parsePackageCommand` + `handlePackageCommand` + `handleConfigCommand`
//! (package-manager-cli.ts): per-command `--help`, the `invalidOption`/`missingOptionValue`/
//! `invalidArgument`/`conflictingOptions` diagnostics (each with a usage line + exit 1), the
//! `update` target matrix (`--self`/`--extensions`/`--models`/`--all`/`--extension <source>`
//! combos) including the foreground catalog refresh `--models` dispatches to
//! (`refreshModelCatalogs`, `:397-423` → [`crate::provider::refresh_model_catalogs`]), the
//! User/Project-grouped `list` with `(filtered)` tags + dim installed paths, and the
//! saved-trust-store lookup + project-write guard. The interactive trust prompt is the gated tail
//! (residual ledger); the binary self-update lands on upstream's own "this installation cannot
//! self-update" branch, since cyrup has no release endpoint to fetch from.

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
/// `config`, plus cyrup's `mcp`). `config` and `mcp` are both dispatched specially — neither takes
/// `PackageCommand`'s flag grammar.
const SUBCOMMANDS: [&str; 7] =
    ["install", "remove", "uninstall", "update", "list", "config", "mcp"];

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

/// The `update` target (Pi `UpdateTarget = {all} | {self} | {extensions, source?} | {models}`,
/// package-manager-cli.ts:35 @v0.83.0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateTargetSel {
    All,
    SelfUpdate,
    Extensions(Option<String>),
    /// `--models` — refresh the model catalogs and nothing else (pi `{ type: "models" }`,
    /// dispatched at `package-manager-cli.ts:726-735` before any settings/trust work).
    Models,
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
    let mut models_flag = false;
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
            // pi package-manager-cli.ts:250-257 — accepted for `update` only, an `invalidOption`
            // everywhere else, exactly like `--self`/`--extensions`.
            "--models" => {
                if command == PackageCommand::Update {
                    models_flag = true;
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
        if all_flag && (self_flag || extensions_flag || models_flag || extension_flag_source.is_some())
        {
            conflicting_options.get_or_insert_with(|| {
                "--all cannot be combined with --self, --extensions, --models, or --extension"
                    .to_string()
            });
        }
        if all_flag && source.is_some() {
            conflicting_options.get_or_insert_with(|| {
                "--all cannot be combined with a positional source".to_string()
            });
        }

        // `--models` is checked FIRST (pi `:329`), so it owns the target whenever it is present and
        // its own two conflict messages are the ones a user sees.
        if models_flag {
            if self_flag || extensions_flag || all_flag || extension_flag_source.is_some() {
                conflicting_options.get_or_insert_with(|| {
                    "--models cannot be combined with --self, --extensions, --all, or --extension"
                        .to_string()
                });
            }
            if source.is_some() {
                conflicting_options.get_or_insert_with(|| {
                    "--models cannot be combined with a positional source".to_string()
                });
            }
            update_target = Some(UpdateTargetSel::Models);
        } else if let Some(ext_source) = extension_flag_source.clone() {
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
            // CYRUP-DELTA (SEAM-110) — upstream is `source === "self" || source === "pi"`
            // (pi v0.83.0 `packages/coding-agent/src/package-manager-cli.ts:348`): exactly TWO self
            // aliases. cyrup accepts a third, its own name, because that is the spelling a user of a
            // binary called `cyrup` would actually guess; `self` and `pi` stay accepted as the legacy
            // spellings so an upstream muscle-memory invocation keeps working. The superset is
            // harmless — what was NOT harmless is that the help advertised only the `pi` alias and
            // never the `cyrup` one, so the guessable spelling was the undocumented one. The short
            // forms in [`render_command_help`] now name `cyrup` and record the other two.
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
            "{APP} update [source|self|pi] [--self|--extensions|--models|--all] [--extension <source>] [--approve|--no-approve] [--force]"
        ),
        PackageCommand::List => format!("{APP} list [--approve|--no-approve]"),
    }
}

/// Per-command `--help` body (Pi `printPackageCommandHelp`, package-manager-cli.ts:90-166).
pub fn render_command_help(command: PackageCommand) -> String {
    const APP: &str = "cyrup";
    const CFG: &str = ".cyrup";
    const REPO: &str = SELF_UPDATE_REPO;
    match command {
        PackageCommand::Install => format!(
            // The Pi `npm:@foo/bar` example is dropped here: `PackageSource::parse` hard-rejects the
            // `npm:` channel in the Rust port (source.rs:79-81 — no JS runtime, R-09-021), so an
            // `install npm:@foo/bar` is guaranteed to error. Advertising it in cyrup's OWN help would be
            // dead-but-advertised. The remaining git/https/ssh/path examples mirror Pi's list
            // (package-manager-cli.ts:104-110) minus that unsupported channel.
            //
            // SEAM-076 — the summary + `-l` lines used to name `settings.json`, which is pi's storage
            // (`installAndPersist` → `addSourceToSettings`, package-manager.ts:817-841 @v0.83.0) and
            // NOT cyrup's: the only write this path makes is `lock::save` into the package registry
            // (`cyrup-resources/src/package/install.rs:152-158`), the deliberate divergence recorded
            // at `cyrup-session-svc/src/builder.rs:936-945`. A user who followed the old text edited a
            // file the installer never touches, and a hand-added `packages` entry there is a SECOND,
            // additive channel (`cyrup-config/src/settings.rs:343-373`) — a duplicate, not a fix.
            "Usage:\n  {}\n\nInstall a package and record it in the package registry.\n\nOptions:\n  -l, --local       Install project-locally ({CFG}/packages.json)\n  -a, --approve     Trust project-local files for this command\n  -na, --no-approve Ignore project-local files for this command\n\nExamples:\n  {APP} install git:github.com/user/repo\n  {APP} install git:git@github.com:user/repo\n  {APP} install https://github.com/user/repo\n  {APP} install ssh://git@github.com/user/repo\n  {APP} install ./local/path\n",
            usage(command)
        ),
        PackageCommand::Remove => format!(
            // SEAM-076 (the registry, not settings — see the install arm) and SEAM-077: pi's two
            // examples are BOTH `npm:` (package-manager-cli.ts:145-146 @v0.83.0), a channel
            // `PackageSource::parse` hard-rejects here (`source.rs:79-81`), so the only two examples
            // this help showed named a source class that can never have been installed. They are the
            // git + path forms the install help already uses, which `parse` accepts.
            "Usage:\n  {}\n\nRemove a package and its source from the package registry.\nAlias: {APP} uninstall <source> [-l]\n\nOptions:\n  -l, --local       Remove from the project registry ({CFG}/packages.json)\n  -a, --approve     Trust project-local files for this command\n  -na, --no-approve Ignore project-local files for this command\n\nExamples:\n  {APP} remove git:github.com/user/repo\n  {APP} uninstall ./local/path\n",
            usage(command)
        ),
        // SEAM-078 — `--self`/`--all`/`--force` and the bare/`pi` short forms all resolve to the
        // self-update stub ([`self_update_unavailable`]), so they are marked unavailable here rather
        // than described as functional, and the stub's remedy names the route that actually works for
        // a source install. `--models` IS implemented (pi `refreshModelCatalogs`,
        // package-manager-cli.ts:397-423) and is described exactly as upstream describes it (`:159`,
        // `:169`).
        //
        // SEAM-110 — the last short form used to read `cyrup update pi   … (self works as alias to
        // pi)`, a straight rebrand of pi's `:170`. It advertised the one self alias a cyrup user is
        // least likely to type and never mentioned `cyrup`, the one they would guess. It now names
        // `cyrup` and records `self`/`pi` as the other accepted spellings; the accepting code and its
        // CYRUP-DELTA are at the `source_is_self` binding in [`parse_package_command`].
        PackageCommand::Update => format!(
            "Usage:\n  {}\n\nUpdate installed packages or model catalogs. Self-update is unavailable in this build.\n\nOptions:\n  --self                  Update {APP} only (UNAVAILABLE — see below; default when no target is given)\n  --extensions            Update installed packages only\n  --models                Refresh model catalogs only\n  --all                   Update {APP} (UNAVAILABLE) and installed packages\n  --extension <source>    Update one package only\n  -a, --approve           Trust project-local files for this command\n  -na, --no-approve       Ignore project-local files for this command\n  --force                 Reinstall {APP} even if the current version is latest (UNAVAILABLE)\n\nShort forms:\n  {APP} update                Update {APP} only (UNAVAILABLE)\n  {APP} update --all          Update {APP} (UNAVAILABLE) and all extensions\n  {APP} update --extensions   Update all installed packages\n  {APP} update --models       Refresh model catalogs only\n  {APP} update <source>       Update one package\n  {APP} update {APP}          Update {APP} only (self and pi also work as aliases, UNAVAILABLE)\n\nSelf-update: this build cannot replace its own binary. Update it with:\n  cargo install --git {REPO} {APP}\n",
            usage(command)
        ),
        PackageCommand::List => format!(
            "Usage:\n  {}\n\nList installed packages from user and project settings.\n\nOptions:\n  -a, --approve      Trust project-local files for this command\n  -na, --no-approve  Ignore project-local files for this command\n",
            usage(command)
        ),
    }
}

/// The repository a from-source install is updated from — the ONE route that works for this build
/// (`docs/guide/getting-started/install.md:22`). Read off the manifest so it cannot drift.
const SELF_UPDATE_REPO: &str = env!("CARGO_PKG_REPOSITORY");

/// `cyrup config`'s usage line (Pi `CONFIG_COMMAND_USAGE`, package-manager-cli.ts:92).
const CONFIG_COMMAND_USAGE: &str = "cyrup config [-l] [--approve|--no-approve]";

/// `cyrup config --help` (Pi `printConfigCommandHelp`, package-manager-cli.ts:94-107).
///
/// SEAM-079 — this had no counterpart at all: `-h`/`--help` fell through the flag scan and the
/// interactive picker opened instead (in a pipeline, the `No configurable …` line and exit 0). The
/// `config` verb is the one whose flags are least guessable, and it was the one that could not be
/// asked.
pub fn render_config_help() -> String {
    const CFG: &str = ".cyrup";
    format!(
        "Usage:\n  {CONFIG_COMMAND_USAGE}\n\nOpen the resource configuration TUI to enable or disable skills, prompts and themes.\nWithout -l, starts in global settings (~/{CFG}/agent/settings.json).\nPress Tab in the TUI to switch between global and project-local modes.\n\nOptions:\n  -l, --local       Edit project overrides ({CFG}/settings.json)\n  -a, --approve     Trust project-local files for this command with -l\n  -na, --no-approve Ignore project-local files for this command with -l\n\nEach toggle writes a `+pattern` (load) or `-pattern` (unload) entry into the matching\nskills/prompts/themes array of the settings file for the active write scope.\n"
    )
}

/// Pi's `printSelfUpdateUnavailable` (package-manager-cli.ts:424-436 @v0.83.0) with the one
/// instruction that is true for this build.
///
/// SEAM-078 — the old line was `Self-update is not available in this build; update cyrup via your
/// package manager.` printed on stdout with exit 0. There is no package manager for the only
/// supported install path, so the remedy named nothing a user could run, and the exit code said the
/// requested update had happened. Upstream prints to stderr, names a concrete route, echoes the
/// executable's location, and sets `process.exitCode = 1` (`:428-435`, `:855`).
fn self_update_unavailable() {
    eprintln!("error: cyrup cannot self-update this installation.");
    eprintln!("Update it with: cargo install --git {SELF_UPDATE_REPO} cyrup");
    // `const entrypoint = process.argv[1]; if (entrypoint) { … }` (`:431-435`).
    if let Ok(exe) = std::env::current_exe() {
        eprintln!();
        eprintln!("Location of cyrup executable: {}", exe.display());
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
async fn saved_trusted(dirs: &ConfigDirs) -> bool {
    let store = TrustStore::new(dirs.trust_path());
    store
        .nearest(&dirs.cwd)
        .await.ok()
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
    // `mcp` is dispatched specially, like `config` below: its verbs are not package verbs and it
    // takes none of `PackageCommand`'s flag grammar. MCP-049 (`cli.js:197-218`).
    if argv.first().map(String::as_str) == Some("mcp") {
        // `.get(1..)` rather than `&argv[1..]`: the slice is provably in bounds but the workspace
        // denies `clippy::indexing_slicing`.
        return Ok(Some(crate::mcp_cmd::run(argv.get(1..).unwrap_or_default(), dirs)));
    }

    // `config` is handled specially (Pi `handleConfigCommand`).
    if argv.first().map(String::as_str) == Some("config") {
        // `if (rest.includes("-h") || rest.includes("--help")) { printConfigCommandHelp(); return
        // true; }` (package-manager-cli.ts:612-615) — FIRST, before any flag scan, so `config
        // --help` never reaches the picker (SEAM-079).
        // `.get(1..)` rather than `&argv[1..]`: the slice is provably in bounds (the `first()` test
        // above just matched an element) but the workspace denies `clippy::indexing_slicing`.
        let rest = argv.get(1..).unwrap_or_default();
        if rest.iter().any(|a| a == "-h" || a == "--help") {
            print!("{}", render_config_help());
            return Ok(Some(0));
        }
        // The rest of upstream's scan (`:619-637`): every argument is either a known flag, an
        // unknown option, or an unexpected positional — the last two are diagnostics with the
        // config usage line and exit 1. Without them an unknown `config` flag was silently ignored
        // and the picker opened anyway.
        for arg in rest {
            match arg.as_str() {
                "-l" | "--local" | "-a" | "--approve" | "-na" | "--no-approve" => {}
                other if other.starts_with('-') => {
                    eprintln!("Unknown option {other} for \"config\".");
                    eprintln!("Use \"cyrup --help\" or \"{CONFIG_COMMAND_USAGE}\".");
                    return Ok(Some(1));
                }
                other => {
                    eprintln!("Unexpected argument {other}.");
                    eprintln!("Usage: {CONFIG_COMMAND_USAGE}");
                    return Ok(Some(1));
                }
            }
        }
        // Resolve trust the same way the other verbs do (Pi `createCommandSettingsManager`): the
        // `--approve`/`-a` override, else the saved decision for this folder. Project-scope toggles
        // require trust to persist (R-07-004).
        let trusted = match trust_override(argv).or(cli_trust_override) {
            Some(t) => t,
            None => saved_trusted(dirs).await,
        };
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

    // `cyrup update --models` (Pi package-manager-cli.ts:726-735): dispatched HERE — after the
    // diagnostics, before the settings-manager/trust block at `:737-752` — because a catalog refresh
    // touches no project resource and so must not be gated on project trust or pay for a settings
    // load. Upstream's `catch` renders `Error: {message}` and sets exit 1 (`:729-733`).
    if opts.command == PackageCommand::Update
        && opts.update_target == Some(UpdateTargetSel::Models)
    {
        return Ok(Some(match crate::provider::refresh_model_catalogs(dirs).await {
            Ok(()) => {
                println!("Model catalogs refreshed");
                0
            }
            Err(message) => {
                eprintln!("Error: {message}");
                1
            }
        }));
    }

    // Saved-trust lookup + override (Pi createCommandSettingsManager); the interactive trust prompt
    // for trust-requiring project resources is the gated outer layer (residual ledger #19).
    let trust_override = opts.project_trust_override.or(cli_trust_override);
    let effective_trusted = match trust_override {
        Some(t) => t,
        None => saved_trusted(dirs).await,
    };

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
    // CFG-054 — the package verbs are dispatched BEFORE `run_migrations` (main.rs:145-154), so the
    // doubled-root repair has to be reachable from here too: otherwise the first command a user runs
    // after upgrading (`cyrup list`, `cyrup update --extensions`) would work against the new layout
    // while every already-installed tree still sat under the old one.
    crate::migrations::migrate_packages_root(&dirs.package_dir);
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
            let mut removed = Err(());
            for id in remove_candidate_ids(&source) {
                if manager.remove(&id).await.is_ok() {
                    removed = Ok(());
                    break;
                }
            }
            match removed {
                Ok(()) => {
                    println!("Removed {source}");
                    Ok(0)
                }
                Err(()) => {
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

/// The [`PackageId`]s a `remove`/`update <source>` argument may name, most-normalized first
/// (CFG-055).
///
/// `install` records the id `PackageSource::parse(source).package_id()` produces —
/// `git:<host>/<user>/<repo>` with the scheme, `git@`, trailing `/` and `.git` stripped, or
/// `path:<canonical-abs-path>` (`cyrup-resources/src/package/source.rs:105-116`, `:175-188`). This
/// used to key the lookup off `PackageId::from(<raw argument>)`, so every spelling that normalizes —
/// an `https://` URL, an `scp`-style `git@host:u/r`, a relative path, a `.git` suffix — missed the
/// row `install` had written: `cyrup remove` said `No matching package found` (or, for a `path:`
/// row, silently left it in place) with no way for the user to learn the id, because `cyrup list`
/// prints the source display and never the id.
///
/// Upstream matches on a NORMALIZED key too, and this is its shape: `packageSourcesMatch`
/// (`package-manager.ts:1418-1422` @v0.83.0) compares `getSourceMatchKeyForSettings(existing)`
/// against `getSourceMatchKeyForInput(input)` (`:1362-1383`), both of which reduce a git source to
/// `git:<host>/<path>` and a local one to `local:<resolved>` before comparing — never the raw
/// string. `update`'s positional target does the same through `getPackageIdentity(source)`
/// (`:1051`).
///
/// The raw id is kept as a FALLBACK so a registry row written by an older build — whose id is
/// whatever string was typed — is still removable.
fn remove_candidate_ids(source: &str) -> Vec<PackageId> {
    let mut ids = Vec::new();
    if let Ok(parsed) = PackageSource::parse(source) {
        ids.push(parsed.package_id());
    }
    let raw = PackageId::from(source);
    if !ids.contains(&raw) {
        ids.push(raw);
    }
    ids
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

/// `update`: the target matrix (Pi package-manager-cli.ts:705-763), minus the `models` target, which
/// [`dispatch`] answers before this is reached (upstream `:726-735`). The self/binary update is the
/// deferred distribution tail (residual ledger #26): it reports failure rather than downloading
/// (SEAM-078).
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
    // `updateTargetIncludesExtensions`/`IncludesSelf` (package-manager-cli.ts:389-395): `Models` is
    // in NEITHER, and never reaches here anyway — `dispatch` returns on it upstream at `:726-735`
    // and here for the same reason.
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
                // The same normalization `remove` uses (CFG-055): upstream keys this target off
                // `getPackageIdentity(source)` (`package-manager.ts:1051`), not the raw argument,
                // so `cyrup update https://github.com/u/r` matches the row `install git:…` wrote.
                // The registry is walked once per candidate; the first id that matches anything
                // wins, and the raw fallback keeps legacy rows updatable.
                let mut report = None;
                for id in remove_candidate_ids(src) {
                    let r = manager.update(UpdateTarget::One(id), cancel.clone()).await?;
                    let matched = !r.updated.is_empty()
                        || !r.failed.is_empty()
                        || !r.skipped_pinned.is_empty();
                    if matched || report.is_none() {
                        report = Some(r);
                    }
                    if matched {
                        break;
                    }
                }
                report.unwrap_or_default()
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
        // The binary self-update (download + replace) is the deferred distribution tail: it needs a
        // cyrup release endpoint, which does not exist (`update_check.rs:14-23`). Upstream's own
        // "this installation cannot self-update" branch is what that maps onto — stderr, a route
        // the user can actually run, and exit 1 (SEAM-078).
        self_update_unavailable();
        code = 1;
    }
    Ok(code)
}

/// `config`: open the interactive resource-config TUI (Pi `handleConfigCommand` → `selectConfig`,
/// package-manager-cli.ts:543-572). Resolves the settings + trust, discovers every top-level
/// auto-discovered extension/skill/prompt/theme with its current enabled state, mounts the
/// [`ConfigSelector`], and persists each space/enter toggle as a `+pattern`/`-pattern` override entry
/// into the matching `extensions`/`skills`/`prompts`/`themes` settings array (Pi
/// `toggleTopLevelResource`, config-selector.ts:532-578) — the SAME arrays discovery's
/// `global_overrides`/`project_overrides` already read back. Esc closes.
///
/// Package-tier resource toggling (Pi `togglePackageResource`) is out of this bin's crate scope — it
/// needs the installed-package → live-session wiring (`DiscoveryConfig.installed`, gap-07 §1) and
/// `PackageManager::set_enabled`, both in `cyrup-resources`/`cyrup-session-svc`.
async fn run_config(dirs: &ConfigDirs, trusted: bool, local: bool) -> Result<i32> {
    let settings = SettingsManager::load(crate::file_settings_store(dirs), trusted);
    let rows = resolve_config_rows(dirs, &settings, trusted).await?;

    if rows.is_empty() {
        println!("No configurable extensions, skills, prompts, or themes found.");
        return Ok(0);
    }

    // Seed the per-(scope,kind) settings arrays from disk; each toggle read-modify-writes its own
    // scope's array (Pi's `getGlobalSettings()`/`getProjectSettings()` array reads, config-selector.ts).
    let mut arrays: HashMap<(ConfigScope, ConfigKind), Vec<String>> = HashMap::new();
    for kind in
        [ConfigKind::Extensions, ConfigKind::Skills, ConfigKind::Prompts, ConfigKind::Themes]
    {
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
            SettingsManager::load(crate::file_settings_store(dirs), false);
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
    run_startup_selector(&theme, &keymap, &mut selector, async |payload: &str| {
        let Some(toggle) = ConfigToggle::from_payload(payload) else {
            return;
        };
        let settings_scope = match toggle.scope {
            ConfigScope::User => SettingsScope::Global,
            ConfigScope::Project => SettingsScope::Project,
        };
        let entry = arrays.entry((toggle.scope, toggle.kind)).or_default();
        // Drop any prior +/-/! entry for this exact pattern, then push the new decision (Pi
        // `toggleTopLevelResource`, config-selector.ts:471-480): enabling writes `+pattern`,
        // disabling `-pattern`.
        entry.retain(|p| strip_override_marker(p) != toggle.pattern);
        entry.push(format!("{}{}", if toggle.enabled { '+' } else { '-' }, toggle.pattern));
        let value = serde_json::Value::Array(
            entry.iter().cloned().map(serde_json::Value::String).collect(),
        );
        // Awaited HERE, before the loop redraws: the row the selector is about to paint as
        // enabled/disabled is already on disk. `run_startup_selector`'s `Err` (a dead terminal
        // mid-session) can now only lose the in-flight toggle, never the ones already made.
        let written = settings
            .persist_nested(settings_scope, &[toggle.kind.key()], value)
            .await;
        if let Err(e) = written {
            persist_err = Some(e.to_string());
        }
    })
    .await?;

    if let Some(e) = persist_err {
        // A project toggle in an untrusted folder is the usual cause (Pi requires trust to write
        // project settings). Surface it after teardown rather than silently swallowing.
        eprintln!("Some changes could not be saved: {e}");
        eprintln!("(use --approve to modify project settings in an untrusted folder)");
        return Ok(1);
    }
    Ok(0)
}

/// The current `extensions`/`skills`/`prompts`/`themes` override array of a settings layer
/// (Pi `arrayKey`, config-selector.ts:537).
fn settings_array(layer: &Settings, kind: ConfigKind) -> Vec<String> {
    match kind {
        ConfigKind::Extensions => layer.extension_paths(),
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

/// Resolve every top-level auto-discovered extension/skill/prompt/theme with its **current** enabled
/// state, for the config editor. Runs discovery twice against the SAME dirs: once with the live
/// settings override patterns (the enabled set) and once with empty overrides (the full universe of
/// files). A resource is enabled iff it survived the override-filtered pass — reusing
/// cyrup-resources' own enable/disable logic without depending on its `pub(crate)` matcher. Mirrors
/// Pi's `packageManager.resolve()` returning every resource tagged with its `enabled` flag
/// (package-manager.ts:881-897).
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
    // `loose_extensions` carries the settings verdict on each entry rather than dropping the
    // disabled ones (`LooseExtension::enabled`), so the enabled set is the `enabled` pass's
    // surviving subset — the same shape the three sets above have.
    let enabled_extensions: HashSet<_> = enabled
        .registry
        .loose_extensions
        .iter()
        .filter(|e| e.enabled)
        .map(|e| e.path.clone())
        .collect();

    let mut rows = Vec::new();
    let mut seen: HashSet<(ConfigKind, std::path::PathBuf)> = HashSet::new();

    // Extensions first, matching Pi's `addToGroup(resolved.extensions, "extensions")`
    // (config-selector.ts:153, the first of four). The widget re-sorts by `ConfigKind::order()`
    // anyway, so this is presentation-neutral; it keeps the collector reading like upstream.
    for ext in &universe.registry.loose_extensions {
        let Some((cscope, pattern, base_dir)) = loose_pattern(ext.scope, &ext.root, &ext.path, dirs)
        else {
            continue;
        };
        if !seen.insert((ConfigKind::Extensions, ext.path.clone())) {
            continue;
        }
        rows.push(ConfigRow {
            scope: cscope,
            kind: ConfigKind::Extensions,
            display_name: ext_display_name(&ext.path),
            pattern,
            base_dir,
            enabled: enabled_extensions.contains(&ext.path),
        });
    }
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

/// A prompt/theme display name: the file name (Pi config-selector.ts:139).
fn file_display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// An extension display name (Pi config-selector.ts:131-140, the `resourceType === "extensions" &&
/// parentFolder !== "extensions"` branch at `:134`): `parentFolder/fileName` when the containing
/// directory is not literally `extensions`, otherwise the plain file name.
///
/// The two shapes it disambiguates are exactly the two the discovery scan accepts: a bare
/// `extensions/mytool.wasm` (parent IS `extensions` → `mytool.wasm`) and an extension directory
/// `extensions/demo/` — whose own name is what the row must show, and which upstream reaches by
/// naming its entry file, e.g. `demo/index.ts` (cyrup: `extensions/demo` → parent `extensions`, so
/// the directory name `demo` is what falls out of the same rule).
fn ext_display_name(path: &Path) -> String {
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let parent = path.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().to_string());
    match parent {
        Some(p) if p != "extensions" => format!("{p}/{file}"),
        _ => file,
    }
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

    /// SEAM-110 — all three self aliases resolve, and the help names the one a cyrup user would
    /// actually guess.
    ///
    /// Upstream accepts exactly two (`source === "self" || source === "pi"`, pi v0.83.0
    /// `package-manager-cli.ts:348`); cyrup's third is a deliberate CYRUP-DELTA recorded at the
    /// `source_is_self` binding. What was a defect is that the short-forms block advertised `pi` and
    /// never `cyrup`, so the accepted spelling closest to hand was the undocumented one.
    #[test]
    fn update_accepts_all_three_self_aliases_and_advertises_the_cyrup_one() {
        for alias in ["self", "pi", "cyrup"] {
            assert_eq!(
                parse_package_command(&v(&["update", alias]))
                    .unwrap()
                    .update_target,
                Some(UpdateTargetSel::SelfUpdate),
                "`cyrup update {alias}` must behave as --self"
            );
        }
        let help = render_command_help(PackageCommand::Update);
        assert!(
            help.contains("cyrup update cyrup"),
            "the guessable alias must be the advertised one: {help}"
        );
        assert!(
            !help.contains("self works as alias to pi"),
            "the old line named only the alias a cyrup user is least likely to type: {help}"
        );
        // Presence before absence: the other two spellings are still recorded as accepted.
        assert!(help.contains("self and pi also work as aliases"), "{help}");
        // A non-alias positional is still a package source, not a self-update.
        assert_eq!(
            parse_package_command(&v(&["update", "git:github.com/u/r"]))
                .unwrap()
                .update_target,
            Some(UpdateTargetSel::Extensions(Some(
                "git:github.com/u/r".to_string()
            )))
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

    /// `cyrup update --models` — the whole target, from the flag to the two conflict messages.
    ///
    /// RED before this pass: `--models` fell through to the `other if other.starts_with('-')` arm,
    /// so the shipped binary answered `Unknown option --models for "update".` and exit 1 — there was
    /// NO CLI route to refresh the model catalogs at all. Upstream has had one since
    /// `package-manager-cli.ts:250-257` (the flag), `:329-337` (this matrix) and `:726-735` (the
    /// dispatch) @v0.83.0.
    #[test]
    fn update_models_is_a_target_with_pis_two_conflict_messages() {
        let models = parse_package_command(&v(&["update", "--models"])).unwrap();
        assert_eq!(models.update_target, Some(UpdateTargetSel::Models));
        assert!(models.invalid_option.is_none(), "--models is a known update flag");
        assert!(models.conflicting_options.is_none());
        // It is NOT the bare-update path, so pi's extensions-skipped note must stay silent.
        assert!(!models.show_extensions_skipped_note);

        // `:330-333` — combined with another target flag. (`--all` is checked EARLIER, at `:321`,
        // and `conflictingOptions ??=` keeps the first message, so `--all --models` reports the
        // `--all` sentence; that case is asserted below.)
        for other in ["--self", "--extensions"] {
            let c = parse_package_command(&v(&["update", "--models", other])).unwrap();
            assert_eq!(
                c.conflicting_options.as_deref(),
                Some(
                    "--models cannot be combined with --self, --extensions, --all, or --extension"
                ),
                "expected pi's --models conflict message for {other}"
            );
        }
        let with_ext = parse_package_command(&v(&["update", "--models", "--extension", "p"])).unwrap();
        assert!(with_ext.conflicting_options.is_some());

        // `:334-336` — combined with a positional source.
        let with_source = parse_package_command(&v(&["update", "--models", "pkg"])).unwrap();
        assert_eq!(
            with_source.conflicting_options.as_deref(),
            Some("--models cannot be combined with a positional source")
        );

        // `--all`'s own message names `--models` too (`:322-323`).
        let all_models = parse_package_command(&v(&["update", "--all", "--models"])).unwrap();
        assert_eq!(
            all_models.conflicting_options.as_deref(),
            Some("--all cannot be combined with --self, --extensions, --models, or --extension")
        );

        // Not an `update` flag anywhere else (`:251-255`).
        for verb in ["install", "remove", "list"] {
            let c = parse_package_command(&v(&[verb, "--models"])).unwrap();
            assert_eq!(c.invalid_option.as_deref(), Some("--models"));
        }

        // …and it is advertised in both the usage line and the help body (`:86`, `:159`, `:169`).
        assert!(usage(PackageCommand::Update).contains("--models"));
        let help = render_command_help(PackageCommand::Update);
        assert!(help.contains("--models                Refresh model catalogs only"));
        assert!(help.contains("cyrup update --models"));
    }

    /// `--models` never reaches the package/self update body: upstream returns from `dispatch` at
    /// `package-manager-cli.ts:726-735`, before the settings manager is even built, and
    /// `updateTargetIncludesSelf`/`IncludesExtensions` (`:389-395`) exclude it on both sides. Were
    /// the arm to fall through, a `cyrup update --models` would print the self-update stub.
    ///
    /// The refresh itself is driven end to end — force flag, 15s bound, error report — against a
    /// loopback origin in `tests/catalog_refresh_modes.rs`; it must not be exercised from here,
    /// because the real `refresh_model_catalogs` resolves its provider set from the process
    /// environment (`AuthStore::has_auth(id, None)`), so a developer with `ANTHROPIC_API_KEY` set
    /// would have this test issue live requests to `https://pi.dev`.
    #[test]
    fn update_models_is_neither_a_self_nor_an_extensions_target() {
        let target = parse_package_command(&v(&["update", "--models"]))
            .unwrap()
            .update_target
            .unwrap();
        assert_eq!(target, UpdateTargetSel::Models);
        assert!(!matches!(
            target,
            UpdateTargetSel::All | UpdateTargetSel::SelfUpdate
        ));
        assert!(!matches!(
            target,
            UpdateTargetSel::All | UpdateTargetSel::Extensions(_)
        ));
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

    /// SEAM-076 — the `install`/`remove` help described a write this port does not make. cyrup's
    /// installer writes ONE file, the package registry (`lock::save(&reg_path, &reg)`,
    /// `cyrup-resources/src/package/install.rs:152-158`); `SettingsManager` is reached only by
    /// `cyrup config`. The help said "settings" because that is where pi writes
    /// (`addSourceToSettings`, `package-manager.ts:817-841` @v0.83.0) — the mechanism divergence is
    /// deliberate (`cyrup-session-svc/src/builder.rs:936-945`), the text was simply stale, and a
    /// user who followed it into `settings.json` could add a `packages` entry through a SECOND,
    /// additive channel and end up with a duplicate.
    ///
    /// SEAM-077 — the remove help's ONLY two examples were `npm:` sources (pi's, `:145-146`), and
    /// `PackageSource::parse` hard-rejects that prefix (`source.rs:79-81`), so neither could name
    /// anything installable. Assert presence (a working example) before absence (`npm:`).
    #[test]
    fn install_and_remove_help_name_the_registry_and_only_installable_examples() {
        let install = render_command_help(PackageCommand::Install);
        let remove = render_command_help(PackageCommand::Remove);

        assert!(install.contains("record it in the package registry"));
        assert!(remove.contains("from the package registry"));
        assert!(install.contains("packages.json"), "the -l flag names the file it writes");
        assert!(remove.contains("packages.json"));
        for (name, text) in [("install", &install), ("remove", &remove)] {
            assert!(
                !text.contains("settings"),
                "{name} help must not claim a settings write it never makes: {text}"
            );
        }

        // Every example the remove help shows must be a source `parse` accepts…
        let examples: Vec<&str> = remove
            .lines()
            .filter_map(|l| l.trim().strip_prefix("cyrup remove "))
            .chain(remove.lines().filter_map(|l| l.trim().strip_prefix("cyrup uninstall ")))
            .collect();
        assert!(!examples.is_empty(), "the remove help must keep working examples");
        for example in &examples {
            assert!(
                PackageSource::parse(example).is_ok(),
                "remove help example {example:?} is rejected by PackageSource::parse"
            );
        }
        // …which is exactly what `npm:` is not.
        assert!(!remove.contains("npm:"), "npm sources cannot be installed, so they cannot be removed");
    }

    /// SEAM-078 — `--self`, `--all`, `--force` and the bare/`pi` short forms all land on the
    /// self-update stub, so the help may not present them as functional, and the stub itself must
    /// name a route that WORKS. The old text pointed at "your package manager", which does not exist
    /// for the only supported install path (`cargo install --git …`), and the command exited 0 as if
    /// the update had happened.
    ///
    /// Pi's shape for an installation it cannot update in place is `printSelfUpdateUnavailable`
    /// (`package-manager-cli.ts:424-436`): stderr, a concrete instruction, the executable's
    /// location, and `process.exitCode = 1` (`:855`).
    #[tokio::test]
    async fn self_update_is_advertised_as_unavailable_and_exits_nonzero() {
        let help = render_command_help(PackageCommand::Update);
        assert!(help.contains("Self-update is unavailable in this build"));
        assert!(
            help.contains(&format!("cargo install --git {SELF_UPDATE_REPO} cyrup")),
            "the help must name the route that works: {help}"
        );
        assert!(!help.contains("package manager"), "there is no package manager for this build");
        for flag in ["--self ", "--all ", "--force "] {
            let line = help
                .lines()
                .find(|l| l.trim_start().starts_with(flag.trim_end()))
                .unwrap_or_else(|| panic!("no help line for {flag}"));
            assert!(
                line.contains("UNAVAILABLE"),
                "{flag} is documented as functional over a stub: {line}"
            );
        }

        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        // Every self-reaching spelling reports failure rather than a silent success.
        for args in [
            vec!["update"],
            vec!["update", "--self"],
            vec!["update", "self"],
            vec!["update", "pi"],
        ] {
            assert_eq!(
                dispatch(&v(&args), &d, None).await.unwrap(),
                Some(1),
                "{args:?} did nothing but exited 0"
            );
        }
    }

    /// SEAM-079 — `cyrup config --help` ran the interactive picker: the flag fell through the scan
    /// (there was none) and, in a pipeline, printed `No configurable skills, prompts, or themes
    /// found.` and exited 0, which reads as success.
    ///
    /// Upstream handles `-h`/`--help` FIRST (`package-manager-cli.ts:612-615`), before the flag scan
    /// (`:619-637`) and before the trust guard (`:648-652`) — which is what this test's `-l` case
    /// pins: with `-l` and no saved trust, the pre-fix path reached the guard and exited 1, so the
    /// help branch running first is directly observable.
    #[tokio::test]
    async fn config_help_prints_usage_instead_of_opening_the_picker() {
        let help = render_config_help();
        assert!(help.starts_with("Usage:\n  cyrup config [-l] [--approve|--no-approve]"));
        // The three flags a user cannot otherwise discover, plus the marker convention the toggles
        // write (`config-selector.ts:471-480`).
        assert!(help.contains("-l, --local"));
        assert!(help.contains("--approve"));
        assert!(help.contains("--no-approve"));
        assert!(help.contains("+pattern"));
        assert!(help.contains("-pattern"));

        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        assert_eq!(dispatch(&v(&["config", "--help"]), &d, None).await.unwrap(), Some(0));
        assert_eq!(dispatch(&v(&["config", "-h"]), &d, None).await.unwrap(), Some(0));
        // `--help` wins over the untrusted-project guard, i.e. it is checked first (`:612` < `:648`).
        assert_eq!(
            dispatch(&v(&["config", "-l", "--help"]), &d, None).await.unwrap(),
            Some(0),
            "help must be answered before the trust guard"
        );
        assert_eq!(
            dispatch(&v(&["config", "-l"]), &d, None).await.unwrap(),
            Some(1),
            "…and without --help the guard still fires (the discriminator for the assertion above)"
        );

        // The rest of upstream's scan (`:626-636`): an unknown option and an unexpected positional
        // are diagnostics, not silently-ignored arguments.
        assert_eq!(dispatch(&v(&["config", "--bogus"]), &d, None).await.unwrap(), Some(1));
        assert_eq!(dispatch(&v(&["config", "skills"]), &d, None).await.unwrap(), Some(1));
    }

    /// CFG-055 — `remove`/`update <source>` must key off the id `install` STORED, not the string the
    /// user typed. Upstream compares normalized match keys on both sides
    /// (`packageSourcesMatch` → `getSourceMatchKeyForInput`, `package-manager.ts:1362-1383`,
    /// `:1418-1422` @v0.83.0), so an https URL removes a package recorded from an scp-style remote.
    #[test]
    fn remove_matches_the_normalized_id_install_wrote_with_a_raw_fallback() {
        let installed = |source: &str| PackageSource::parse(source).unwrap().package_id();
        // Every accepted spelling of one repository resolves to the id `install` records…
        let canonical = installed("git:github.com/user/repo");
        for spelling in [
            "https://github.com/user/repo",
            "https://github.com/user/repo.git",
            "ssh://git@github.com/user/repo",
            "git:git@github.com:user/repo",
            "git:github.com/user/repo/",
        ] {
            assert_eq!(
                installed(spelling),
                canonical,
                "{spelling} installs as a different id than git:github.com/user/repo"
            );
            assert!(
                remove_candidate_ids(spelling).contains(&canonical),
                "remove {spelling} would never reach the row install wrote"
            );
            // The pre-fix behaviour, kept only as a FALLBACK for rows an older build wrote.
            assert!(remove_candidate_ids(spelling).contains(&PackageId::from(spelling)));
            assert_eq!(
                remove_candidate_ids(spelling).first(),
                Some(&canonical),
                "the normalized id must be tried first"
            );
        }
        // A relative path install records `path:<canonical-abs>`; the raw string never matched it.
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        let rel = pkg.to_string_lossy().to_string();
        assert_eq!(remove_candidate_ids(&rel).first(), Some(&installed(&rel)));
        assert_ne!(
            remove_candidate_ids(&rel).first(),
            Some(&PackageId::from(rel.as_str())),
            "a path id is canonicalized, so the raw argument cannot be the primary key"
        );
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
