//! Slash-command registry, parse & dispatch (spec/tui/04 §2; gaps 2/19/20).
//!
//! A 1:1 port of Pi's builtin command surface (`core/slash-commands.ts:18-41`
//! `BUILTIN_SLASH_COMMANDS`) plus the submit-handler dispatch chain
//! (`interactive-mode.ts:2549-2734` `setupEditorSubmitHandler`). The registry is **display order**
//! (NOT alphabetical) and feeds both autocomplete (`autocomplete.rs`) and dispatch.
//!
//! Dispatch is intentionally **neutral**: [`CommandRegistry::dispatch`] classifies a submitted line
//! into a [`Dispatch`] (a builtin command + argument, a bash invocation, or an agent prompt) without
//! performing any side effect — the app shell runs the resulting action (opening an overlay, calling
//! the runtime, etc.). This mirrors Pi, where completion never executes and dispatch is a pure
//! `trim()`-then-if-chain on the exact text (`interactive-mode.ts:2554`).
use std::borrow::Cow;

/// Where a command came from (`slash-commands.ts`; `interactive-mode.ts:443-467`). Only [`Builtin`]
/// commands ship in this crate; the dynamic sources are merged by the app from session resources.
///
/// [`Builtin`]: CommandSource::Builtin
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandSource {
    Builtin,
    Prompt,
    Extension,
    Skill,
}

/// One slash command's metadata (spec/tui/04 §2.2). `name` carries no leading `/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    /// Command name without the leading `/` (e.g. `"model"`).
    ///
    /// [`Cow`] rather than `&'static str` because a registered extension command's name is only
    /// known at runtime (`InitApi::register_command(name: impl Into<String>)`), as are a prompt
    /// template's and a skill's. The builtin table stays a `const` array of `Cow::Borrowed`, so
    /// nothing is allocated for the common case.
    pub name: Cow<'static, str>,
    /// Human description (Pi's `description`), source-tag-prefixed for non-builtins.
    pub description: Cow<'static, str>,
    /// Argument hint shown before the description (e.g. `"<model>"`); `None` for arg-less commands.
    pub argument_hint: Option<Cow<'static, str>>,
    /// Provenance.
    pub source: CommandSource,
    /// Whether the command provides argument completion (only `/model` in Pi, `:498-528`).
    pub has_arg_completion: bool,
}

/// The 22 builtin slash commands in Pi display/autocomplete order (`slash-commands.ts:18-41`).
/// NOT alphabetical — order is user-visible and preserved exactly.
pub const BUILTIN_SLASH_COMMANDS: &[SlashCommand] = &[
    cmd("settings", "Open settings menu", None),
    // TUI-025 — pi's hint is `"<provider/model>"` (`slash-commands.ts:21` @v0.84.1); `"<model>"`
    // understated the required form and left the user guessing at the `provider/` half.
    arg_cmd("model", "Select model (opens selector UI)", "<provider/model>"),
    cmd("scoped-models", "Enable/disable models for Ctrl+P cycling", None),
    cmd("export", "Export session (HTML default, or specify path: .html/.jsonl)", None),
    cmd("import", "Import and resume a session from a JSONL file", None),
    cmd("share", "Share session as a secret GitHub gist", None),
    cmd("copy", "Copy last agent message to clipboard", None),
    cmd("name", "Set session display name", None),
    cmd("session", "Show session info and stats", None),
    cmd("changelog", "Show changelog entries", None),
    cmd("hotkeys", "Show all keyboard shortcuts", None),
    cmd("fork", "Create a new fork from a previous user message", None),
    cmd("clone", "Duplicate the current session at the current position", None),
    cmd("tree", "Navigate session tree (switch branches)", None),
    cmd("trust", "Save project trust decision for future sessions", None),
    // TUI-025 — pi carries `argumentHint: "<provider>"` here (`slash-commands.ts:35`); cyrup had no
    // hint at all, which is also what left `has_arg_completion` false for `/login`.
    arg_cmd("login", "Configure provider authentication", "<provider>"),
    cmd("logout", "Remove provider authentication", None),
    cmd("new", "Start a new session", None),
    cmd("compact", "Manually compact the session context", None),
    cmd("resume", "Resume a different session", None),
    // TUI-025 — `slash-commands.ts:40`: `"Reload keybindings, extensions, skills, prompts, themes,
    // and context files"`. `/reload` does reload context files, so the shorter sentence was wrong,
    // not merely shorter.
    cmd(
        "reload",
        "Reload keybindings, extensions, skills, prompts, themes, and context files",
        None,
    ),
    cmd("quit", "Quit cyrup", None),
];

/// Hidden / undocumented commands dispatched but kept out of autocomplete
/// (`interactive-mode.ts:2657-2671`): `/debug` + the two easter eggs.
pub const HIDDEN_COMMANDS: &[&str] = &["debug", "arminsayshi", "dementedelves"];

const fn cmd(
    name: &'static str,
    description: &'static str,
    argument_hint: Option<&'static str>,
) -> SlashCommand {
    SlashCommand {
        name: Cow::Borrowed(name),
        description: Cow::Borrowed(description),
        argument_hint: match argument_hint {
            Some(hint) => Some(Cow::Borrowed(hint)),
            None => None,
        },
        source: CommandSource::Builtin,
        has_arg_completion: false,
    }
}

const fn arg_cmd(
    name: &'static str,
    description: &'static str,
    hint: &'static str,
) -> SlashCommand {
    SlashCommand {
        name: Cow::Borrowed(name),
        description: Cow::Borrowed(description),
        argument_hint: Some(Cow::Borrowed(hint)),
        source: CommandSource::Builtin,
        has_arg_completion: true,
    }
}

/// The classification of one submitted editor line (spec/tui/04 §2.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dispatch {
    /// A recognized slash command (builtin or hidden) plus its trimmed argument (`None` if empty).
    Command { name: String, arg: Option<String> },
    /// A bash invocation: `!cmd` (`excluded = false`) or `!!cmd` (`excluded = true`).
    /// `command` is the trimmed body (after the `!`/`!!`).
    Bash { command: String, excluded: bool },
    /// Plain prompt text for the agent (includes unknown `/foo` and `/modelX`).
    Prompt(String),
    /// Whitespace-only input → ignored (no submit).
    Empty,
}

/// The command registry: the builtin table plus any dynamic (prompt/extension/skill) commands the
/// app merges in. Built once at startup and rebuilt on `/reload` (spec/tui/04 §2.2).
#[derive(Clone, Debug)]
pub struct CommandRegistry {
    /// Builtin commands first, then prompt → extension → skill (the autocomplete display order).
    commands: Vec<SlashCommand>,
    /// Recognized dispatch names (builtins + hidden), for exact-or-prefix matching.
    dispatch_names: Vec<&'static str>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        CommandRegistry::new()
    }
}

impl CommandRegistry {
    /// A registry seeded with just the 22 builtins (no dynamic commands yet).
    pub fn new() -> Self {
        let mut dispatch_names: Vec<&'static str> = BUILTIN_SLASH_COMMANDS
            .iter()
            .filter_map(|c| match &c.name {
                Cow::Borrowed(name) => Some(*name),
                Cow::Owned(_) => None,
            })
            .collect();
        dispatch_names.extend_from_slice(HIDDEN_COMMANDS);
        CommandRegistry { commands: BUILTIN_SLASH_COMMANDS.to_vec(), dispatch_names }
    }

    /// The registry this crate's doc has always described: the builtin table PLUS the dynamic
    /// prompt/extension/skill commands the app merges in (spec/tui/04 §2.2).
    ///
    /// Until this existed, `CommandSource::{Prompt, Extension, Skill}` were declared and NEVER
    /// constructed, [`InputEditor::set_registry`] had zero callers, and
    /// `AgentSession::slash_command_catalog()` — which already merges all three sources — was
    /// consumed only by RPC mode. An RPC client saw every registered command; the interactive TUI
    /// showed builtins only, from the same session with the same registrations.
    ///
    /// `dispatch_names` is deliberately NOT extended. It drives the local builtin dispatch table,
    /// and a dynamic command is not dispatched locally — it routes to the extension/prompt/skill
    /// that registered it. Merging them here would make `/foo` resolve to a builtin `Dispatch`
    /// that no arm handles. They are autocomplete-visible, which is exactly the gap.
    #[must_use]
    pub fn with_dynamic(dynamic: impl IntoIterator<Item = SlashCommand>) -> Self {
        let mut registry = Self::new();
        // Builtins win a name collision: pi resolves duplicate registrations by suffixing the
        // LATER one (`runner.ts:556-595` `invocation_name`), never by shadowing an existing name.
        let existing: std::collections::HashSet<String> =
            registry.commands.iter().map(|c| c.name.to_string()).collect();
        for cmd in dynamic {
            if !existing.contains(cmd.name.as_ref()) {
                registry.commands.push(cmd);
            }
        }
        registry
    }

    /// All autocomplete-visible commands in display order (builtins first, then any dynamic
    /// commands merged via [`Self::with_dynamic`]).
    pub fn commands(&self) -> &[SlashCommand] {
        &self.commands
    }

    /// Look up a command's metadata by bare name.
    pub fn get(&self, name: &str) -> Option<&SlashCommand> {
        self.commands.iter().find(|c| c.name == name)
    }

    /// Classify a submitted line (spec/tui/04 §2.3). Pure: no side effects, no I/O.
    ///
    /// Order (load-bearing, matching `interactive-mode.ts:2554-2734`):
    /// 1. `trim()`; empty → [`Dispatch::Empty`].
    /// 2. A slash command via **exact-or-`"name "`-prefix** match (so `/modelX` is NOT `/model`).
    ///    The argument is `text[name.len()+1..].trim()`, `None` when empty.
    /// 3. `!`/`!!` → [`Dispatch::Bash`] (checked **after** the slash table, **before** the prompt
    ///    fallback). An empty body falls through to a prompt.
    /// 4. Everything else (incl. unknown `/foo`) → [`Dispatch::Prompt`].
    pub fn dispatch(&self, text: &str) -> Dispatch {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Dispatch::Empty;
        }
        if let Some(slash) = trimmed.strip_prefix('/') {
            if let Some((name, arg)) = self.match_command(slash) {
                return Dispatch::Command { name: name.to_string(), arg };
            }
            // Unknown `/foo` (or `/modelX`) is NOT an error — it is a literal prompt (§2.3 rule).
            return Dispatch::Prompt(trimmed.to_string());
        }
        if let Some(rest) = trimmed.strip_prefix('!') {
            let (excluded, body) = match rest.strip_prefix('!') {
                Some(inner) => (true, inner),
                None => (false, rest),
            };
            let command = body.trim();
            if !command.is_empty() {
                return Dispatch::Bash { command: command.to_string(), excluded };
            }
            // `!` with an empty body is normal text (§2.4 / edge-case 4).
        }
        Dispatch::Prompt(trimmed.to_string())
    }

    /// Match `slash` (the text **after** the leading `/`) against a recognized command using
    /// exact-or-`"name "`-prefix semantics. Returns `(name, arg)` with the trimmed argument.
    fn match_command(&self, slash: &str) -> Option<(&'static str, Option<String>)> {
        for &name in &self.dispatch_names {
            if slash == name {
                return Some((name, None));
            }
            // `"name "`-prefixed: an argument follows. `strip_prefix(name)` then a leading space.
            if let Some(rest) = slash.strip_prefix(name)
                && let Some(arg) = rest.strip_prefix(' ') {
                    let arg = arg.trim();
                    return Some((name, (!arg.is_empty()).then(|| arg.to_string())));
                }
        }
        None
    }
}

/// pi `getAutocompleteSourceTag` (`interactive-mode.ts:536-559`): the short provenance tag shown
/// before a non-builtin command's description.
///
/// `scope` picks the prefix (`user`→`u`, `project`→`p`, anything else→`t`), and the `source` only
/// widens it for package origins: `npm:…` and git URLs get appended, while `auto`/`local`/`cli`
/// — and every unrecognized source, via pi's final `return scopePrefix` — stay bare.
#[must_use]
pub fn autocomplete_source_tag(scope: &str, source: &str) -> String {
    let scope_prefix = match scope {
        "user" => "u",
        "project" => "p",
        _ => "t",
    };
    let source = source.trim();
    if source.starts_with("npm:") {
        return format!("{scope_prefix}:{source}");
    }
    // pi also special-cases a parseable git URL (`:552-556`). cyrup's catalog synthesizes
    // `source: "extension"|"prompt"|"skill"` for every row it emits, so no row reaches that arm
    // today; it falls through to pi's own `return scopePrefix` default either way.
    scope_prefix.to_string()
}

/// pi `prefixAutocompleteDescription` (`interactive-mode.ts:561-567`): `[tag] description`, or a
/// bare `[tag]` when the command carries no description.
#[must_use]
fn prefix_autocomplete_description(description: &str, tag: &str) -> String {
    if description.is_empty() {
        format!("[{tag}]")
    } else {
        format!("[{tag}] {description}")
    }
}

/// Turn `AgentSession::slash_command_catalog()`'s JSON rows into autocomplete-visible commands.
///
/// That catalog already merges the three dynamic sources — registered extension commands, prompt
/// templates and skills — and was consumed ONLY by RPC mode (`cyrup-modes/src/rpc.rs`). This is the
/// interactive half of the same seam: without it the TUI's `/` menu listed builtins alone, so the
/// 13 slash commands the subagents extension registers (and every prompt template and skill) were
/// invisible in the default mode while an RPC client saw all of them from the same session.
///
/// Equivalent to [`dynamic_commands_from_catalog_gated`] with skill commands enabled; prefer that
/// entry point from the app so the `enableSkillCommands` setting is honored.
#[must_use]
pub fn dynamic_commands_from_catalog(catalog: &[serde_json::Value]) -> Vec<SlashCommand> {
    dynamic_commands_from_catalog_gated(catalog, true)
}

/// [`dynamic_commands_from_catalog`], with the `enableSkillCommands` setting applied.
///
/// Pi builds the interactive autocomplete list in `createBaseAutocompleteProvider`
/// (`interactive-mode.ts:610-622`) and wraps the `skill:<name>` half in
/// `if (this.settingsManager.getEnableSkillCommands())` — so a `false` setting removes every skill
/// from the `/` menu while leaving extension commands and prompt templates alone. The gate is
/// **interactive-only**: Pi's `get_commands` RPC (`rpc-mode.ts:676-690`) emits skills
/// unconditionally, which is why `AgentSession::slash_command_catalog()` — the port of that RPC
/// handler — stays ungated and the filtering happens here, at the one consumer Pi gates.
///
/// Note the gate is autocomplete visibility only, in cyrup as in Pi: dynamic commands are never
/// added to `dispatch_names`, and `/skill:<name>` expansion happens server-side in
/// `AgentSession::expand_slash_command`, so a hidden skill typed out in full still runs — exactly
/// as Pi's `skillCommands` map (populated at `:616`, read nowhere) leaves it.
#[must_use]
pub fn dynamic_commands_from_catalog_gated(
    catalog: &[serde_json::Value],
    enable_skill_commands: bool,
) -> Vec<SlashCommand> {
    catalog
        .iter()
        .filter_map(|row| {
            let name = row.get("name")?.as_str()?;
            if name.is_empty() {
                return None;
            }
            let source = row.get("source").and_then(serde_json::Value::as_str).unwrap_or("");
            let kind = match source {
                "extension" => CommandSource::Extension,
                "prompt" => CommandSource::Prompt,
                "skill" if enable_skill_commands => CommandSource::Skill,
                "skill" => return None,
                _ => return None,
            };
            let description = row
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let info = row.get("sourceInfo");
            let scope = info
                .and_then(|i| i.get("scope"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("temporary");
            let info_source = info
                .and_then(|i| i.get("source"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(source);
            let tag = autocomplete_source_tag(scope, info_source);
            Some(SlashCommand {
                name: Cow::Owned(name.to_string()),
                description: Cow::Owned(prefix_autocomplete_description(description, &tag)),
                argument_hint: None,
                source: kind,
                has_arg_completion: false,
            })
        })
        .collect()
}
