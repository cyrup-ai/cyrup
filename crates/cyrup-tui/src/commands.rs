//! Slash-command registry, parse & dispatch (spec/tui/04 §2; gaps 2/19/20).
//!
//! A 1:1 port of Pi's builtin command surface (`packages/coding-agent/src/core/slash-commands.ts:19-42`
//! `BUILTIN_SLASH_COMMANDS` @v0.83.0) plus the submit-handler dispatch chain
//! (`interactive-mode.ts:2660-2846` `setupEditorSubmitHandler` @v0.83.0). The registry is **display order**
//! (NOT alphabetical) and feeds both autocomplete (`autocomplete.rs`) and dispatch.
//!
//! Dispatch is intentionally **neutral**: [`CommandRegistry::dispatch`] classifies a submitted line
//! into a [`Dispatch`] (a builtin command + argument, a bash invocation, or an agent prompt) without
//! performing any side effect — the app shell runs the resulting action (opening an overlay, calling
//! the runtime, etc.). This mirrors Pi, where completion never executes and dispatch is a pure
//! `trim()`-then-if-chain on the exact text (`interactive-mode.ts:2662` @v0.83.0).
use std::borrow::Cow;

/// Where a command came from. Only [`Builtin`] commands ship in this crate; the dynamic sources are
/// merged by the app from session resources (`interactive-mode.ts:592-621` @v0.83.0 builds pi's
/// three dynamic blocks — templates, extension commands, skills).
///
/// TUI-086, and the reason this enum is NOT the port of pi's `SlashCommandSource`: upstream's union
/// is exactly `"extension" | "prompt" | "skill"` (`core/slash-commands.ts:4` @v0.83.0), and
/// `BuiltinSlashCommand` (`:13-17`) has **no `source` field at all** — builtins are a different
/// TYPE upstream. cyrup unified the two into one struct, which forces this fourth variant.
/// [`Builtin`] is therefore cyrup-only and **must never be serialized as a `SlashCommandSource`**:
/// anything crossing the RPC or WIT boundary with it emits a value pi has no word for. TUI-087: the
/// `interactive-mode.ts:443-467` this used to cite is a getter and the constructor's opening at the
/// ported tag — it has nothing to do with command provenance.
///
/// [`Builtin`]: CommandSource::Builtin
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandSource {
    Builtin,
    Prompt,
    Extension,
    Skill,
}

/// Which builtin argument completer a command owns — cyrup's stand-in for pi's
/// `SlashCommand.getArgumentCompletions?(argumentPrefix)` callback
/// (`packages/tui/src/autocomplete.ts:238` @v0.84.3), installed per builtin in
/// `createBaseAutocompleteProvider` (`interactive-mode.ts:685-736` @v0.84.3).
///
/// A **data tag** rather than the callback itself: [`BUILTIN_SLASH_COMMANDS`] is a `const` array
/// built by `const fn`s and [`SlashCommand`] derives `Clone`/`Debug`/`PartialEq`/`Eq` — neither
/// survives a boxed closure. The data each variant ranks is threaded in at
/// [`crate::Autocomplete::compute`] time ([`crate::ArgumentSources`]), which is what keeps `compute`
/// synchronous.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgumentCompleter {
    /// No argument completion — pi's absent `getArgumentCompletions`, which makes the whole
    /// argument branch return `null` (`autocomplete.ts:351-353` @v0.84.3).
    None,
    /// The scoped-else-available model catalog, inserted as `provider/id`
    /// (`interactive-mode.ts:687-710` @v0.84.3).
    Models,
    /// The known login providers, inserted as the bare provider id
    /// (`interactive-mode.ts:728-735` @v0.84.3).
    LoginProviders,
    /// The current model's reasoning ladder (`interactive-mode.ts:713-725` @v0.84.3).
    ///
    /// Wired to the `/thinking` builtin (`commands.rs:131`). Thinking is *also* a Shift+Tab cycle
    /// here (`app/submit.rs:49-52`); the two are independent entry points onto the same ladder.
    ThinkingLevels,
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
    /// Which argument completer this command owns — the port of pi assigning
    /// `getArgumentCompletions` to a builtin in `createBaseAutocompleteProvider`.
    ///
    /// pi wires **three** builtins: `/model` (`interactive-mode.ts:687` @v0.84.3), `/thinking`
    /// (`:713`) and `/login` (`:728`). cyrup has no `/thinking` command, so only the first and
    /// third are non-[`ArgumentCompleter::None`] in [`BUILTIN_SLASH_COMMANDS`]. Every dynamic
    /// (prompt/extension/skill) row is `None` — see the note on `arg_completion` in
    /// [`dynamic_commands_from_catalog_gated`].
    pub arg_completion: ArgumentCompleter,
}

impl SlashCommand {
    /// Whether this command offers argument completion at all — pi's
    /// `"getArgumentCompletions" in command && command.getArgumentCompletions`
    /// (`autocomplete.ts:351` @v0.84.3) reduced to a predicate.
    #[must_use]
    pub fn has_arg_completion(&self) -> bool {
        self.arg_completion != ArgumentCompleter::None
    }
}

/// The 22 builtin slash commands in Pi display/autocomplete order
/// (`core/slash-commands.ts:19-42` @v0.83.0).
/// NOT alphabetical — order is user-visible and preserved exactly.
pub const BUILTIN_SLASH_COMMANDS: &[SlashCommand] = &[
    cmd("settings", "Open settings menu", None),
    // TUI-025 — pi's hint is `"<provider/model>"` (`slash-commands.ts:21` @v0.83.0); `"<model>"`
    // understated the required form and left the user guessing at the `provider/` half.
    arg_cmd(
        "model",
        "Select model (opens selector UI)",
        "<provider/model>",
        ArgumentCompleter::Models,
    ),
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
    // GAP 5 — pi registers this immediately after `tree` (`slash-commands.ts:23`), with exactly
    // this description and hint. The list order is user-visible autocomplete order, so the slot
    // matters as much as the entry.
    arg_cmd("thinking", "Set thinking level", "<level>", ArgumentCompleter::ThinkingLevels),
    cmd("trust", "Save project trust decision for future sessions", None),
    // TUI-025 — pi carries `argumentHint: "<provider>"` here (`slash-commands.ts:35` @v0.83.0); cyrup had no
    // hint at all, which is also what left `/login` without an argument completer.
    arg_cmd(
        "login",
        "Configure provider authentication",
        "<provider>",
        ArgumentCompleter::LoginProviders,
    ),
    cmd("logout", "Remove provider authentication", None),
    cmd("new", "Start a new session", None),
    cmd("compact", "Manually compact the session context", None),
    cmd("resume", "Resume a different session", None),
    // TUI-025 — `slash-commands.ts:40` @v0.83.0: `"Reload keybindings, extensions, skills, prompts, themes,
    // and context files"`. `/reload` does reload context files, so the shorter sentence was wrong,
    // not merely shorter.
    cmd(
        "reload",
        "Reload keybindings, extensions, skills, prompts, themes, and context files",
        None,
    ),
    // TUI-083 — upstream this description is not a literal: `slash-commands.ts:41` @v0.83.0 reads
    // `` `Quit ${APP_NAME}` `` where `APP_NAME = piConfigName || "pi"` (`config.ts:489`), so a user
    // running under a renamed config sees their own app name. cyrup has no config-name override to
    // template against, so the literal is a DECISION, recorded here rather than left as an accident:
    // the moment such an override is added, this line is the one that has to change.
    cmd("quit", "Quit cyrup", None),
];

/// Hidden / undocumented commands dispatched but kept out of autocomplete
/// (`interactive-mode.ts:2769-2783` @v0.83.0 — `/debug` `:2769`, `/arminsayshi` `:2774`,
/// `/dementedelves` `:2779`): `/debug` + the two easter eggs. TUI-087: the `:2657-2671` this cited
/// is a v0.84.1-era offset; at the ported tag it lands inside `setupEditorSubmitHandler`'s opening.
pub const HIDDEN_COMMANDS: &[&str] = &["debug", "arminsayshi", "dementedelves"];

/// TUI-074 — the **only** dispatch names that accept an argument
/// (`interactive-mode.ts:2666-2793` @v0.83.0).
///
/// `setupEditorSubmitHandler` is a hand-written if-chain, and its guards are not uniform: six
/// commands are `text === "/x" || text.startsWith("/x ")` — `/model` (`:2676`), `/export` (`:2682`),
/// `/import` (`:2687`), `/name` (`:2702`), `/login` (`:2742`) and `/compact` (`:2758`) — while the
/// other nineteen, `/settings` (`:2666`) through `/quit` (`:2789`), are strict equality. So
/// upstream `/quit now` is **not** the quit command: it falls past the whole chain and is sent to
/// the model as a prompt. Matching that requires this list; a uniform matcher cannot express it.
const ARGUMENT_DISPATCH_NAMES: &[&str] =
    &["model", "thinking", "export", "import", "name", "login", "compact"];

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
        arg_completion: ArgumentCompleter::None,
    }
}

const fn arg_cmd(
    name: &'static str,
    description: &'static str,
    hint: &'static str,
    completer: ArgumentCompleter,
) -> SlashCommand {
    SlashCommand {
        name: Cow::Borrowed(name),
        description: Cow::Borrowed(description),
        argument_hint: Some(Cow::Borrowed(hint)),
        source: CommandSource::Builtin,
        arg_completion: completer,
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
    /// constructed, [`crate::editor::InputEditor::set_registry`] had zero callers, and
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
        // LATER one (`runner.ts:598-641` `invocationName`), never by shadowing an existing name.
        let existing: std::collections::HashSet<String> =
            registry.commands.iter().map(|c| c.name.to_string()).collect();
        let mut merged: Vec<SlashCommand> =
            dynamic.into_iter().filter(|c| !existing.contains(c.name.as_ref())).collect();
        // TUI-075 — pi's display order is builtins → PROMPT TEMPLATES → extension commands → skills
        // (`interactive-mode.ts:625` @v0.83.0, `[...slashCommands, ...templateCommands,
        // ...extensionCommands, ...skillCommandList]`). The catalog this list comes from emits
        // extensions FIRST (`cyrup-session-svc/src/session.rs:2503` extensions, `:2517` prompts,
        // `:2526` skills) because it is also the RPC `get_commands` payload, whose order is pi's RPC
        // order and must not be changed to fix an interactive display. So the reorder happens here,
        // at the one consumer that shows the list. Order is user-visible: an empty `/` query returns
        // the items unfiltered on both sides (pi `fuzzy.ts:100-102`, cyrup `fuzzy.rs::filter`), so a
        // user with several extensions had to scroll past them to reach their own prompt templates.
        //
        // A STABLE sort, so within a source the catalog's own order (extension LOAD order, and the
        // resource loaders' winner order) survives — that ordering is load-bearing upstream too.
        merged.sort_by_key(|c| match c.source {
            CommandSource::Prompt => 0u8,
            CommandSource::Extension => 1,
            CommandSource::Skill => 2,
            // Unreachable through `dynamic_commands_from_catalog_gated`, which never emits a builtin
            // row; ordered first so a hand-built one cannot land after the dynamic blocks.
            CommandSource::Builtin => 0,
        });
        registry.commands.extend(merged);
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
    /// Order (load-bearing, matching `interactive-mode.ts:2660-2846` @v0.83.0):
    /// 1. `trim()`; empty → [`Dispatch::Empty`].
    /// 2. A slash command via **exact** match, plus the `"name "`-prefix form for the six names in
    ///    [`ARGUMENT_DISPATCH_NAMES`] (so `/modelX` is NOT `/model`, and `/quit now` is NOT
    ///    `/quit`). The argument is `text[name.len()+1..].trim()`, `None` when empty.
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

    /// Match `slash` (the text **after** the leading `/`) against a recognized command. Exact match
    /// for every name; the additional `"name "`-prefix form **only** for the six names in
    /// [`ARGUMENT_DISPATCH_NAMES`], which is the set upstream's if-chain guards with
    /// `|| text.startsWith("/x ")`. Returns `(name, arg)` with the trimmed argument.
    fn match_command(&self, slash: &str) -> Option<(&'static str, Option<String>)> {
        for &name in &self.dispatch_names {
            if slash == name {
                return Some((name, None));
            }
            if !ARGUMENT_DISPATCH_NAMES.contains(&name) {
                // Strict equality upstream, so `/quit now`, `/copy that`, `/new session` and
                // `/trust me` are prompts, not commands (`interactive-mode.ts:2666-2793` @v0.83.0).
                continue;
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

/// pi `getAutocompleteSourceTag(sourceInfo?)` (`interactive-mode.ts:497-520` @v0.83.0): the short
/// provenance tag shown before a non-builtin command's description.
///
/// `None` when the row carries NO `sourceInfo` — pi's `if (!sourceInfo) return undefined` (`:498-500`).
/// TUI-085: this used to take a bare `scope` defaulted to `"temporary"` by the caller and always
/// return a tag, so a `sourceInfo`-less row rendered `[t] desc` — a provenance claim ("came from a
/// temporary scope") that upstream does not make and that may be false. A wrong tag is worse than
/// none. Unreachable through cyrup's own catalog, which always emits `sourceInfo`; kept honest
/// because the option-ness is the contract, not the current callers.
///
/// Otherwise `scope` picks the prefix (`user`→`u`, `project`→`p`, anything else→`t`), and `source`
/// only widens it for package origins: `npm:…` and git URLs get appended, while `auto`/`local`/`cli`
/// — and every unrecognized source, via pi's final `return scopePrefix` (`:519`) — stay bare.
///
/// TUI-086: `pub(crate)`, not `pub`. Upstream's is a PRIVATE method on `InteractiveMode`
/// (`interactive-mode.ts:497`), and there is no cross-crate caller at HEAD; a wider surface than
/// pi's invites callers upstream has no counterpart for.
#[must_use]
pub(crate) fn autocomplete_source_tag(source_info: Option<&serde_json::Value>) -> Option<String> {
    let info = source_info?;
    let scope_prefix = match info.get("scope").and_then(serde_json::Value::as_str) {
        Some("user") => "u",
        Some("project") => "p",
        _ => "t",
    };
    let source = info.get("source").and_then(serde_json::Value::as_str).unwrap_or("").trim();
    if source.starts_with("npm:") {
        return Some(format!("{scope_prefix}:{source}"));
    }
    // pi also special-cases a parseable git URL (`:513-518`). cyrup's catalog synthesizes
    // `source: "extension"|"prompt"|"skill"` for every row it emits, so no row reaches that arm
    // today; it falls through to pi's own `return scopePrefix` default either way.
    Some(scope_prefix.to_string())
}

/// pi `getPathCommandArgument` (`interactive-mode.ts:5450-5477` @v0.83.0), called at `:5435`
/// (`/export`) and `:5480` (`/import`) — ONE quote-aware token, not the whole remainder (TUI-079).
///
/// `arg` is what [`CommandRegistry::dispatch`] already produced: the text after `"/export "` /
/// `"/import "`, trimmed. Upstream takes the raw line and does the slicing itself; the outcome is
/// identical, because its `argsString` is `trimStart()`ed and any trailing whitespace is cut by the
/// same `search(/\s/)` that cuts a second token.
///
/// The three rules, in pi's order:
/// * a leading `"` or `'` runs to its MATCHING close, and the quotes are stripped (`:5464-5470`);
/// * an UNTERMINATED quote is `undefined` — a refusal, not a best-effort path (`:5467-5469`);
/// * otherwise the token ends at the first whitespace (`:5472-5476`).
///
/// Deliberately NOT a general dispatch rule: `/name`, `/compact`, `/model` and `/login` take their
/// remainder whole upstream, so this is applied at the `/export` and `/import` arms only.
#[must_use]
pub(crate) fn path_command_argument(arg: &str) -> Option<String> {
    let args = arg.trim_start();
    if args.is_empty() {
        return None;
    }
    let first = args.chars().next()?;
    if first == '"' || first == '\'' {
        let rest = args.get(first.len_utf8()..)?;
        // `indexOf(firstChar, 1)`: the matching close. `< 0` (none) is upstream's `undefined`.
        let close = rest.find(first)?;
        return rest.get(..close).map(str::to_string);
    }
    match args.find(char::is_whitespace) {
        Some(ws) => args.get(..ws).map(str::to_string),
        None => Some(args.to_string()),
    }
}

/// pi `prefixAutocompleteDescription` (`interactive-mode.ts:522-528` @v0.83.0): `[tag] description`,
/// or a bare `[tag]` when the command carries no description — and the description UNPREFIXED when
/// there is no tag at all (`if (!sourceTag) return description`, `:524-526`; TUI-085).
#[must_use]
fn prefix_autocomplete_description(description: &str, tag: Option<&str>) -> String {
    let Some(tag) = tag else { return description.to_string() };
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
/// (`interactive-mode.ts:609-621` @v0.83.0) and wraps the `skill:<name>` half in
/// `if (this.settingsManager.getEnableSkillCommands())` — so a `false` setting removes every skill
/// from the `/` menu while leaving extension commands and prompt templates alone. The gate is
/// **interactive-only**: Pi's `get_commands` RPC (`modes/rpc/rpc-mode.ts:677-707` @v0.83.0) emits skills
/// unconditionally, which is why `AgentSession::slash_command_catalog()` — the port of that RPC
/// handler — stays ungated and the filtering happens here, at the one consumer Pi gates.
///
/// TUI-087: this paragraph used to name `AgentSession::expand_slash_command`, a function that has
/// never existed in this workspace — a fabricated symbol inside an otherwise-correct behavioural
/// claim, which is the worst kind: it reads as verified and cannot be checked.
///
/// Note the gate is autocomplete visibility only, in cyrup as in Pi: dynamic commands are never
/// added to `dispatch_names`, and `/skill:<name>` expansion happens server-side in
/// `AgentSession::expand_input_text` / `expand_skill_command`
/// (`cyrup-session-svc/src/session.rs:1208`, `:1216`), so a hidden skill typed out in full still
/// runs — exactly
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
            // TUI-085: pass the whole `sourceInfo` (absent = `None`), the way pi passes
            // `cmd.sourceInfo` straight into `prefixAutocompleteDescription` (`:595`, `:606`,
            // `:619`). The old code defaulted a missing scope to `"temporary"`, which INVENTED a
            // `[t]` provenance for a row that declared none.
            let tag = autocomplete_source_tag(row.get("sourceInfo"));
            Some(SlashCommand {
                name: Cow::Owned(name.to_string()),
                description: Cow::Owned(prefix_autocomplete_description(
                    description,
                    tag.as_deref(),
                )),
                // CMDHINT_01 — prompt templates carry `argument-hint` frontmatter
                // (`cyrup-resources/src/prompt.rs:41,112-113`); extension commands and skills have no upstream
                // analog (`interactive-mode.ts:691-698` forwards a completer, not a hint; `cyrup-ext/src/registry.rs:94-98`
                // has no such field), so only `source:"prompt"` rows can produce one. The empty-string filter
                // matches pi's `&&`-spread truthiness test even though the producer already guarantees non-empty.
                argument_hint: row
                    .get("argumentHint")
                    .and_then(serde_json::Value::as_str)
                    .filter(|h| !h.is_empty())
                    .map(|h| Cow::Owned(h.to_string())),
                source: kind,
                // EXT-013 / TUI-012: still hardcoded, and it CANNOT be resolved in this crate —
                // for TWO reasons, only the first of which this note used to record.
                //
                // (a) No catalog key. `slash_command_catalog()`
                // (`cyrup-session-svc/src/session/commands.rs:174` — the `session.rs:2503-2532` this
                // used to cite predates that file being split into a module directory) emits no key
                // saying whether a registered command declared `getArgumentCompletions`; pi carries
                // the callback itself onto the autocomplete row (`interactive-mode.ts:753`
                // @v0.84.3).
                //
                // (b) No call path even with the key. `cyrup_ext::ExtensionRegistry::command_autocomplete()`
                // (`crates/cyrup-ext/src/registry.rs:1013`) only records WHICH commands opted in —
                // there is no completer to invoke, and invoking a wasm guest is async, which
                // [`crate::Autocomplete::compute`] is not (and must not become; the popup is
                // recomputed per keystroke on the render thread). A bare boolean would therefore
                // buy a completer that always yields zero items while suppressing the path
                // fall-through — strictly worse than `None`.
                arg_completion: ArgumentCompleter::None,
            })
        })
        .collect()
}
