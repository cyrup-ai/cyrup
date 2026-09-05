use std::path::PathBuf;

use cyrup_config::{AppMode, ConfigDirs};
use cyrup_session_svc::{
    ExtensionFlagValue as SvcExtensionFlagValue, NoTools, SessionConfig, SessionTarget,
};

use crate::diagnostics::Diagnostic;

use super::args::Cli;
use super::argv::ExtFlagValue;
use super::enums::ThinkingArg;

/// Does this run write a session JSONL? — the ONE definition of Pi's
/// `persist = !noSession && (explicitSession || interactive)` rule.
///
/// ACP-213 — this function exists because the expression was written out **twice, verbatim**: here
/// in [`Cli::to_session_config`] and again in `crate::prelaunch::resolve_session`, which recomputes
/// it once the `--session`/`--fork`/`--session-id` resolution has settled `config.target`. Adding a
/// mode to one and not the other is a live foot-gun: an ACP `session/new` would build a
/// `MemStore`-backed session on whichever path it took, `session_file()` would return `None`, and
/// the session would be invisible to `session/list` and unloadable by `session/load` on the next
/// connection — with nothing reporting a fault. Both call sites now route through here.
///
/// `AppMode::Acp` persists for the same reason `Interactive` does, and for a stronger one: every
/// feature in area 4d — `session/list`, `session/load`, `session/delete`, replay — presupposes a
/// JSONL, where for the TUI persistence is merely the expected default. `--no-session` still wins,
/// as it does for every other host.
pub(crate) fn persists(no_session: bool, explicit_session: bool, mode: AppMode) -> bool {
    !no_session && (explicit_session || matches!(mode, AppMode::Interactive | AppMode::Acp))
}

impl Cli {
    /// `--approve` (Some(true)) / `--no-approve` (Some(false)) / neither (None). Approve wins if both.
    pub fn trust_override(&self) -> Option<bool> {
        if self.approve {
            Some(true)
        } else if self.no_approve {
            Some(false)
        } else {
            None
        }
    }

    /// The default tool-suppression mode (`--no-tools` ⇒ all; `--no-builtin-tools` ⇒ builtin; else
    /// none). `--no-tools` wins if both are given (it is strictly broader).
    pub fn no_tools_mode(&self) -> Option<NoTools> {
        if self.no_tools {
            Some(NoTools::All)
        } else if self.no_builtin_tools {
            Some(NoTools::Builtin)
        } else {
            None
        }
    }

    /// The trimmed `--name`, erroring when empty after trim (Pi main.ts:586-592). `Ok(None)` when no
    /// `--name` was given.
    pub fn validated_name(&self) -> Result<Option<String>, String> {
        match &self.name {
            None => Ok(None),
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    Err("--name requires a non-empty value".to_string())
                } else {
                    Ok(Some(trimmed.to_string()))
                }
            }
        }
    }

    /// The `--append-system-prompt` parts, each run through [`resolve_prompt_input`] and then joined
    /// (Pi keeps a `string[]` and resolves EVERY entry independently — resource-loader.ts:536-538 —
    /// then joins with a BLANK LINE at agent-session.ts:1039-1040; the builder takes one blob).
    /// `None` when empty. Any per-entry read warning is returned alongside.
    pub fn append_system_prompt_resolved(
        &self,
        cwd: &std::path::Path,
    ) -> (Option<String>, Vec<Diagnostic>) {
        if self.append_system_prompt.is_empty() {
            return (None, Vec::new());
        }
        let mut diags = Vec::new();
        let parts: Vec<String> = self
            .append_system_prompt
            .iter()
            .map(|raw| {
                let (text, warn) = resolve_prompt_input(cwd, raw, "append system prompt");
                diags.extend(warn);
                text
            })
            .collect();
        (Some(parts.join("\n\n")), diags)
    }

    /// Map the CLI + resolved directories + runtime mode onto a [`SessionConfig`] (arch-11 §3.7),
    /// discarding the prompt-file read warnings. Use [`Self::to_session_config_with_diagnostics`]
    /// on the production path so the warnings reach the user.
    pub fn to_session_config(&self, dirs: &ConfigDirs, mode: AppMode) -> SessionConfig {
        self.to_session_config_with_diagnostics(dirs, mode).0
    }

    /// Map the CLI + resolved directories + runtime mode onto a [`SessionConfig`] (arch-11 §3.7),
    /// plus any diagnostics produced while mapping (today: the `--system-prompt` /
    /// `--append-system-prompt` file-read warnings, Pi `resolvePromptInput`).
    ///
    /// Persistence (R-11-008): one-shot PRINT/JSON default to an ephemeral in-memory session unless a
    /// session is explicitly resumed/continued; interactive always persists. `--no-session` forces
    /// ephemeral in every mode (Pi `noSession`, args.ts:104).
    pub fn to_session_config_with_diagnostics(
        &self,
        dirs: &ConfigDirs,
        mode: AppMode,
    ) -> (SessionConfig, Vec<Diagnostic>) {
        let mut prompt_diagnostics: Vec<Diagnostic> = Vec::new();
        let mut config = SessionConfig::new(dirs.cwd.clone(), dirs.agent_dir.clone());
        // Thread the REAL user home (not the agent dir) so the resources ancestor-walk dedup
        // (`~/.agents/skills`) and the trust-requiring-resource walk resolve against `$HOME`, exactly
        // like Pi's `getHomeDir()` (`process.env.HOME || homedir()`, package-manager.ts:217) and
        // trust-manager.ts:185. `SessionConfig::new` defaults `home` to the agent dir; override it here.
        config.home = dirs.home.clone();
        // Thread the resolved package dir (CLI `--package-dir` > `CYRUP_PACKAGE_DIR`/`PI_PACKAGE_DIR`
        // env > `<agent_dir>/packages` default; env.rs:156-160) so the session builder reads installed
        // packages from the SAME root the `install` subcommand writes to
        // (`PackageStore::new(dirs.package_dir, Some(dirs.cwd))`, subcommands.rs:396). Pi resolves ONE
        // `agentDir` (main.ts:481 `getAgentDir()`) and threads it into BOTH the package manager and the
        // resource loader (resource-loader.ts:222-224 constructs `new DefaultPackageManager({ agentDir })`
        // from the same value used for resource discovery), so an install into a custom dir is always
        // visible to the assembled session. cyrup splits `package_dir` into its own knob, so it must be
        // threaded here too; `SessionConfig::new` otherwise leaves it at the `<agent_dir>/packages`
        // default and a non-default `--package-dir`/`CYRUP_PACKAGE_DIR` install never loads (gap-13 C1
        // residual — the default case landed in f5eee19).
        config.package_dir = dirs.package_dir.clone();
        // Preserve Pi's optional `sessionDir` distinction: `Some` ONLY when `--session-dir`/env was
        // explicitly supplied (used literally), `None` ⇒ the builder applies the cwd-encoded default
        // (gap-analysis 05, Finding 3). Collapsing this to `Some(resolved)` unconditionally would
        // make the builder treat the default root as an explicit dir and skip cwd-encoding.
        config.session_dir = dirs.session_dir_explicit.then(|| dirs.session_dir.clone());
        config.app_mode = mode;
        config.model_pattern = self.model.clone();
        // An explicit `--provider` lets the builder's custom-fallback fire for a bare unresolvable
        // `--model` id (Pi `cliProvider`, model-resolver.ts:369,475).
        config.cli_provider_explicit = self.provider.is_some();
        config.thinking_level = self.thinking.map(ThinkingArg::to_level);
        config.trust_override = self.trust_override();
        config.no_context_files = self.no_context_files;
        config.no_skills = self.no_skills;
        config.no_prompt_templates = self.no_prompt_templates;
        // `ACP-018` — the ACP host disables theme discovery unconditionally, and it is the one
        // mode-conditional resource knob here.
        //
        // Port of the `--no-themes` pi-acp v0.0.33 `pi-rpc/process.ts` passes to its child, with
        // upstream's own justifying comment: a theme is a TERMINAL rendering concern, and the ACP
        // client draws every pixel the user sees. Discovering, parsing and validating them costs
        // startup time on every `session/new` and can only produce diagnostics about something no
        // ACP user can observe.
        //
        // Scoped to themes ALONE, deliberately: `no_skills`, `no_prompt_templates`,
        // `no_context_files` and `no_extensions` all keep following their flags, because those
        // resources ARE observable over ACP — a skill is a slash command in the client's palette
        // (`ACP-268`), a prompt template expands server-side (`ACP-266`), and an extension can
        // register both. `--no-themes` is therefore a no-op under `--acp` rather than a
        // contradiction, and `--theme` still names a path that is simply never read.
        config.no_themes = self.no_themes || mode == AppMode::Acp;
        // `--no-extensions`/`-ne` disables extension discovery; explicit `--extension`/`-e` paths still
        // load (Pi `resourceLoaderOptions.noExtensions`/`additionalExtensionPaths`, main.ts:660,664).
        config.no_extensions = self.no_extensions;
        config.extra_extension_paths = resolve_cli_paths(&dirs.cwd, &self.extension);
        // Relative resource paths are resolved to absolute vs the cwd before threading (Pi
        // `resolveCliPaths`, main.ts:450-451,605-608); package-source specs (npm:/git:/…) are kept.
        config.extra_skill_paths = resolve_cli_paths(&dirs.cwd, &self.skill);
        config.extra_prompt_paths = resolve_cli_paths(&dirs.cwd, &self.prompt_template);
        config.extra_theme_paths = resolve_cli_paths(&dirs.cwd, &self.theme);
        // CFG-S01: `--system-prompt`/`--append-system-prompt` take EITHER literal text or a path,
        // decided purely by existence (Pi `resolvePromptInput`, resource-loader.ts:53-68, applied at
        // :526 and :536-538). cyrup used to thread the raw token straight through, so a path became
        // the literal system prompt text.
        config.system_prompt = self.system_prompt.as_deref().map(|raw| {
            let (text, warn) = resolve_prompt_input(&dirs.cwd, raw, "system prompt");
            prompt_diagnostics.extend(warn);
            text
        });
        let (append, append_diags) = self.append_system_prompt_resolved(&dirs.cwd);
        prompt_diagnostics.extend(append_diags);
        config.append_system_prompt = append;
        config.no_tools = self.no_tools_mode();
        if !self.tools.is_empty() {
            config.tools = Some(self.tools.clone());
        }
        config.exclude_tools = self.exclude_tools.clone();
        // Thread the captured extension flags (Pi `extensionFlagValues: parsed.unknownFlags`,
        // main.ts:634) onto the config so they reach the session services; a loaded extension reads
        // them via `applyExtensionFlagValues` (the WASM-guest consumption is the ext-host tier).
        config.extension_flag_values = self
            .extension_flags
            .iter()
            .map(|f| {
                let value = match &f.value {
                    ExtFlagValue::Bool(b) => SvcExtensionFlagValue::Bool(*b),
                    ExtFlagValue::Str(s) => SvcExtensionFlagValue::Str(s.clone()),
                };
                (f.name.clone(), value)
            })
            .collect();
        config.target = self.session_target(&dirs.session_dir);
        let explicit_session = matches!(
            config.target,
            SessionTarget::Resume(_) | SessionTarget::Continue
        );
        // ACP-213 — one definition, two call sites; see [`persists`].
        config.persist = persists(self.no_session, explicit_session, mode);
        (config, prompt_diagnostics)
    }

    /// Trim each comma-split segment of the delimited list flags, matching Pi's own post-split
    /// normalization (`args.ts:114,120-129`). clap's `value_delimiter = ','` splits the value but never
    /// trims, so a `--tools "read, grep"` would otherwise keep the leading-space `" grep"`, which then
    /// fails the exact tool-name match and silently drops the tool. This must be called once on the
    /// parsed CLI before any consumer reads these Vecs so every downstream site sees Pi-normalized names.
    ///
    /// The per-flag semantics are 1:1 with Pi:
    /// - `--models` (`args.ts:115`): `.split(",").map((s) => s.trim())` — trim only, empties KEPT (Pi
    ///   does not `.filter`; an empty pattern resolves to nothing, and keeping `[""]` for `--models ""`
    ///   preserves Pi's non-empty `parsed.models` so the `--api-key`-requires-a-model gate matches).
    /// - `--tools` / `--exclude-tools` (`args.ts:120-129`): `.split(",").map(trim).filter(len > 0)` —
    ///   trim AND drop empty segments.
    pub fn normalize_list_flags(&mut self) {
        for pattern in &mut self.models {
            *pattern = pattern.trim().to_string();
        }
        for list in [&mut self.tools, &mut self.exclude_tools] {
            list.iter_mut()
                .for_each(|name| *name = name.trim().to_string());
            list.retain(|name| !name.is_empty());
        }
    }

    /// Strip the NUL marker [`crate::diagnostics::apply_arg_leniency`] puts in front of a
    /// `-p ---…` escape-hatch message (pi args.ts:140-146 — `next.startsWith("---")`), restoring the
    /// token to the literal prompt word the user typed. SEAM-107.
    ///
    /// Runs after the clap parse, beside [`Self::normalize_list_flags`], and before any consumer
    /// reads `positionals` — including `split_positionals`, so the restored word is classified as a
    /// message or an `@file` on its real spelling.
    pub fn restore_escaped_positionals(&mut self) {
        for token in &mut self.positionals {
            if let Some(rest) = token.strip_prefix(crate::diagnostics::ESCAPED_MESSAGE_PREFIX) {
                *token = rest.to_string();
            }
        }
    }
}

/// Port of Pi `resolvePromptInput` (`coding-agent/src/core/resource-loader.ts:53-68`) — the
/// path-or-literal rule behind `--system-prompt` / `--append-system-prompt` (whose own help text
/// says "Append text or file contents", args.ts:261).
///
/// The decision is made **purely by existence**, not by a prefix, an extension, or a `@` sigil:
///
/// * `input` names something that exists ⇒ read it and use the CONTENTS;
/// * it exists but cannot be read (a directory, a permissions error) ⇒ warn and fall back to the
///   literal string — Pi's `console.error(chalk.yellow("Warning: Could not read <description> file
///   <path>: <err>"))` + `return input`. It is deliberately NOT fatal;
/// * it does not exist ⇒ the literal string, with no diagnostic.
///
/// `cwd` anchors a relative token, which is what Pi's bare `existsSync(input)`/`readFileSync(input)`
/// does against `process.cwd()`; passing it explicitly keeps the function testable. Contents are
/// decoded lossily to mirror Node's `readFileSync(path, "utf-8")` (which substitutes U+FFFD rather
/// than failing), so a non-UTF-8 file is not silently turned back into its own path.
pub fn resolve_prompt_input(
    cwd: &std::path::Path,
    input: &str,
    description: &str,
) -> (String, Vec<Diagnostic>) {
    // Pi's `if (!input) return undefined` guard: an empty token is never probed as a path (joining it
    // onto `cwd` would otherwise "exist" as the cwd itself and warn).
    if input.is_empty() {
        return (String::new(), Vec::new());
    }
    let candidate = std::path::Path::new(input);
    let path = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    };
    if !path.exists() {
        return (input.to_string(), Vec::new());
    }
    match std::fs::read(&path) {
        Ok(bytes) => (String::from_utf8_lossy(&bytes).into_owned(), Vec::new()),
        Err(e) => (
            input.to_string(),
            vec![Diagnostic::warning(format!(
                "Could not read {description} file {input}: {e}"
            ))],
        ),
    }
}

/// Is `value` a local filesystem path (vs a package-source spec)? Port of Pi `isLocalPath`
/// (paths.ts): `npm:`/`git:`/`github:`/`http:`/`https:`/`ssh:`-prefixed specs are NOT local.
pub fn is_local_path(value: &str) -> bool {
    let t = value.trim();
    !(t.starts_with("npm:")
        || t.starts_with("git:")
        || t.starts_with("github:")
        || t.starts_with("http:")
        || t.starts_with("https:")
        || t.starts_with("ssh:"))
}

/// Resolve relative CLI resource paths to absolute against `cwd`, leaving package-source specs alone
/// (Pi `resolveCliPaths`, main.ts:450-451). An already-absolute local path is kept verbatim.
pub fn resolve_cli_paths(cwd: &std::path::Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|p| {
            let s = p.to_string_lossy();
            if is_local_path(&s) && !p.is_absolute() {
                cwd.join(p)
            } else {
                p.clone()
            }
        })
        .collect()
}
