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
    pub name: &'static str,
    /// Human description (Pi's `description`), source-tag-prefixed for non-builtins.
    pub description: &'static str,
    /// Argument hint shown before the description (e.g. `"<model>"`); `None` for arg-less commands.
    pub argument_hint: Option<&'static str>,
    /// Provenance.
    pub source: CommandSource,
    /// Whether the command provides argument completion (only `/model` in Pi, `:498-528`).
    pub has_arg_completion: bool,
}

/// The 22 builtin slash commands in Pi display/autocomplete order (`slash-commands.ts:18-41`).
/// NOT alphabetical — order is user-visible and preserved exactly.
pub const BUILTIN_SLASH_COMMANDS: &[SlashCommand] = &[
    cmd("settings", "Open settings menu", None),
    arg_cmd("model", "Select model (opens selector UI)", "<model>"),
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
    cmd("login", "Configure provider authentication", None),
    cmd("logout", "Remove provider authentication", None),
    cmd("new", "Start a new session", None),
    cmd("compact", "Manually compact the session context", None),
    cmd("resume", "Resume a different session", None),
    cmd("reload", "Reload keybindings, extensions, skills, prompts, and themes", None),
    cmd("quit", "Quit cyrup", None),
];

/// Hidden / undocumented commands dispatched but kept out of autocomplete
/// (`interactive-mode.ts:2657-2671`): `/debug` + the two easter eggs.
pub const HIDDEN_COMMANDS: &[&str] = &["debug", "arminsayshi", "dementedelves"];

/// cyrup dispatch-only affordances that open a dependency-free in-crate selector but are NOT part of
/// Pi's visible or hidden command surface. Pi reaches these three the same way cyrup ALSO now does —
/// theme via the `/settings` "Theme" row (`settings-selector.ts:603-610`), thinking level via Shift+Tab
/// (`app.thinking.cycle`, `keybindings.ts:72`), and show-images via the `/settings` "Show images" row —
/// so registering them here is a strict SUPERSET of reachability that leaves Pi's own paths intact. They
/// are dispatch-recognized (so `/theme` opens the theme picker instead of leaking to the agent as chat
/// text) but deliberately kept OUT of [`BUILTIN_SLASH_COMMANDS`] so the `/`-autocomplete surface stays
/// byte-for-byte 1:1 with Pi. Each name has a matching arm in `App::run_command`.
pub const DISPATCH_ONLY_COMMANDS: &[&str] = &["theme", "think", "show-images"];

const fn cmd(
    name: &'static str,
    description: &'static str,
    argument_hint: Option<&'static str>,
) -> SlashCommand {
    SlashCommand {
        name,
        description,
        argument_hint,
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
        name,
        description,
        argument_hint: Some(hint),
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
        let mut dispatch_names: Vec<&'static str> =
            BUILTIN_SLASH_COMMANDS.iter().map(|c| c.name).collect();
        dispatch_names.extend_from_slice(HIDDEN_COMMANDS);
        dispatch_names.extend_from_slice(DISPATCH_ONLY_COMMANDS);
        CommandRegistry { commands: BUILTIN_SLASH_COMMANDS.to_vec(), dispatch_names }
    }

    /// All autocomplete-visible commands in display order (builtins; dynamic appended by the app).
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
