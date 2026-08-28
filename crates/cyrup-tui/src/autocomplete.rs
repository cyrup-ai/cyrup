//! The autocomplete engine — context resolution, slash + path completion, apply (spec/tui/04 §3).
//!
//! Port of `pi-tui/src/autocomplete.ts` (`CombinedAutocompleteProvider`) for the two **synchronous**
//! contexts cyrup can resolve without spawning a subprocess:
//!
//! 1. **Slash command** (`autocomplete.ts:313-363` @v0.84.3): the line begins with `/`, split on the
//!    first space. No space → the fuzzy-filtered command NAME list (via [`crate::fuzzy`] + the
//!    [`CommandRegistry`]); space → the ARGUMENT list of whichever completer the named command owns
//!    ([`crate::commands::ArgumentCompleter`]), ranked over the live [`ArgumentSources`] the app
//!    pushes in — including an extension command's own completer, whose answers the app fetches
//!    from the guest and pushes in the same way (see
//!    [`crate::commands::ArgumentCompleter::Extension`]).
//! 2. **Bare path** (`extractPathPrefix`, `:480-507`; `getFileSuggestions`, `:560-693`): the trailing
//!    token looks path-like (contains `/`, or starts with `.`/`~/`) or `Tab` is forced → a single
//!    `read_dir` scan, directories-first.
//!
//! The `@`-mention **fuzzy file search** (`fd`-backed, whole-tree, `autocomplete.ts:719-772`) is a
//! tracked residual (it needs an async `tokio::process` lifecycle + cancellation; see the residual
//! ledger). The popup widget and key routing are identical for it, so it slots in without reshaping
//! this module.
//!
//! Completion never executes a command — it only edits the buffer text (spec/tui/04 §1). Execution
//! happens on submit via [`CommandRegistry::dispatch`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::commands::{ArgumentCompleter, CommandRegistry};
use crate::fuzzy;
use crate::select_list::{ColumnLayout, SelectItem, SelectList};

/// The live data pi's argument completers close over — the three builtin closures
/// (`interactive-mode.ts:685-736` @v0.84.3) plus each extension command's own callback (`:753`).
///
/// Pushed in by the app ([`crate::App::refresh_argument_sources`]) rather than reached for globally:
/// [`Autocomplete::compute`] is synchronous and holds no session. [`Default`] (all empty) until the
/// first push, which makes an argument popup an honest no-op rather than a stale one.
#[derive(Clone, Debug, Default)]
pub struct ArgumentSources {
    /// `/model` candidates — the scoped set when one is active, else the available catalog
    /// (`interactive-mode.ts:689-691` @v0.84.3).
    pub models: Vec<ModelArgument>,
    /// `/login` candidates, already merged per provider (`getLoginProviderCompletionOptions`,
    /// `interactive-mode.ts:299-318` @v0.84.3).
    pub login_providers: Vec<LoginProviderArgument>,
    /// `/thinking` candidates (`interactive-mode.ts:713-725` @v0.84.3) — the current model's
    /// reasoning ladder, ranked for the `/thinking` builtin (`commands.rs:130`).
    pub thinking_levels: Vec<String>,
    /// The most recent answer each EXTENSION command's own completer gave
    /// (`getArgumentCompletions`, `interactive-mode.ts:753` @v0.84.3), keyed by the command's
    /// INVOCATION name — the name that appears on the line and in the registry.
    ///
    /// Unlike the three fields above this is not a periodic snapshot: it is refreshed per keystroke
    /// against the argument the user has actually typed, by
    /// [`crate::App::refresh_extension_completions`]. See [`ArgumentCompleter::Extension`] for why
    /// the guest call lives there and not in [`Autocomplete::compute`].
    pub extension_completions: HashMap<String, ExtensionCompletions>,
}

/// One extension command's completions plus the argument prefix they were fetched for.
///
/// The prefix is kept because pi does NOT re-filter what the completer returns — the callback is
/// handed `argumentText` and its answer is used verbatim (`autocomplete.ts:355-363` @v0.84.3). That
/// is only faithful while the cached answer belongs to the prefix on screen; between the keystroke
/// and the fetch it does not, so a stale set is narrowed locally instead of shown unfiltered. See
/// [`extension_rows`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtensionCompletions {
    /// The argument text the guest was asked about.
    pub prefix: String,
    /// What it answered, in its own order (pi preserves the completer's order).
    pub items: Vec<String>,
}

/// One `/model` candidate (`interactive-mode.ts:694-709` @v0.84.3): the popup shows `id` with
/// `provider` as the description and INSERTS `provider/id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelArgument {
    pub id: String,
    pub provider: String,
    pub name: String,
}

/// One `/login` candidate, already merged per provider the way `getLoginProviderCompletionOptions`
/// (`interactive-mode.ts:299-318` @v0.84.3) merges its per-`(provider, authType)` rows.
/// `auth_types` is in pi's `AUTH_TYPE_ORDER` (`:286`) — oauth before api_key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginProviderArgument {
    pub id: String,
    pub name: String,
    pub auth_types: Vec<cyrup_config::login::AuthType>,
}

/// Which completion context produced the active popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionContext {
    /// Slash-command name completion (replaces the entire `/…` token, re-adds `/` + trailing space).
    Slash,
    /// Slash-command ARGUMENT completion (`autocomplete.ts:344-363` @v0.84.3): `prefix` is the
    /// argument text only (`:345` `slice(spaceIndex + 1)`, echoed back at `:362`), so the replaced
    /// span never includes `/name ` and the inserted value carries no `/`.
    SlashArgument,
    /// Bare path completion (replaces the trailing path token).
    Path,
    /// `@`-mention whole-tree fuzzy file search (`autocomplete.ts:101,164,408`): replaces the trailing
    /// `@query` token with `@path` (quoting paths that contain spaces).
    Mention,
}

/// One candidate: the inserted `value` (replacing `prefix`) plus popup display fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// Text inserted in place of `prefix` (no leading `/` for slash; the path string for path).
    pub value: String,
    /// Whether the candidate is a directory (controls trailing `/` + no trailing space).
    pub is_dir: bool,
}

/// The result of applying a completion: the new buffer + cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
}

/// The active autocomplete popup state (the editor owns an `Option<Autocomplete>`).
#[derive(Clone, Debug)]
pub struct Autocomplete {
    pub context: CompletionContext,
    /// The token under the cursor that candidates replace (`/mod`, `src/ed`, …).
    pub prefix: String,
    /// Parallel to `list.items()`: the value each row inserts.
    completions: Vec<Completion>,
    /// The rendered popup widget (selection/scroll state).
    pub list: SelectList,
}

impl Autocomplete {
    /// The currently selected completion, if any.
    pub fn selected(&self) -> Option<&Completion> {
        self.completions.get(self.list.selected())
    }

    /// Compute suggestions for `(lines, cursor)` (spec/tui/04 §3.2). `force` is the explicit-`Tab`
    /// path that completes a bare token even when it is not obviously path-like. Returns `None` when
    /// no context matches or the candidate set is empty (which closes the popup).
    pub fn compute(
        registry: &CommandRegistry,
        arguments: &ArgumentSources,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
        cwd: &Path,
    ) -> Option<Autocomplete> {
        let line = lines.get(cursor_line).map(String::as_str).unwrap_or("");
        let before: String = line.chars().take(cursor_col).collect();

        // 1. Slash command (`autocomplete.ts:313-363` @v0.84.3) — the name list before the first
        //    space, the argument list after it.
        //
        //    DEVIATION, deliberate: upstream gates this whole arm on `!options.force` (`:313`), so
        //    `/mod` + Tab there falls through to `extractPathPrefix` and lists the working
        //    directory. cyrup tries it on the forced path too — completing a command (or its
        //    argument) is exactly what Tab is for, and answering `/model g<Tab>` with a directory
        //    listing is a wrong answer rather than a missing one.
        if let Some(ac) = slash_context(registry, arguments, &before) {
            return Some(ac);
        }
        // A `/name <arg>` line whose command OWNS a completer is TERMINAL: upstream returns out of
        // the slash branch whether or not it found items (`:351-353` no completer → `null`,
        // `:356-357` empty result → `null`, `:358-363` otherwise). Without this the no-match case
        // would fall through and answer a model query with a directory listing.
        //
        // A `/`-line with NO completer still falls through, which is a pre-existing cyrup
        // deviation and the reason `/export ./sr<Tab>` completes a path.
        if argument_completer(registry, &before).is_some() {
            return None;
        }
        // 2. Bare path.
        path_context(&before, force, cwd)
    }

    /// Apply the selected completion over `prefix` at the cursor (spec/tui/04 §3.6). Pure text
    /// transform; returns the rewritten buffer + new cursor.
    pub fn apply(&self, lines: &[String], cursor_line: usize, cursor_col: usize) -> Option<Applied> {
        let completion = self.selected()?;
        let line = lines.get(cursor_line).map(String::as_str).unwrap_or("");
        let before: Vec<char> = line.chars().take(cursor_col).collect();
        let after: String = line.chars().skip(cursor_col).collect();
        let prefix_len = self.prefix.chars().count();
        if prefix_len > before.len() {
            return None;
        }
        let head: String = before.get(..before.len() - prefix_len)?.iter().collect();

        let (insert, trailing) = match self.context {
            // Slash: `/{value} ` (trailing space; `:393-404`).
            CompletionContext::Slash => (format!("/{}", completion.value), " "),
            // Slash argument: `beforePrefix + item.value`, cursor immediately after the value and
            // NO trailing space — upstream's argument branch is
            // `newLine = beforePrefix + item.value + adjustedAfterCursor` with
            // `cursorCol = beforePrefix.length + cursorOffset` (`autocomplete.ts:432-448`
            // @v0.84.3). The trailing space lives only in the command-NAME branch (`:399-408`,
            // `+2 for "/" and space`) and the `@`-mention branch (`:412-428`).
            CompletionContext::SlashArgument => (completion.value.clone(), ""),
            // Path: directories keep drilling (no space), files get a trailing space (`:408-425`).
            CompletionContext::Path => {
                (completion.value.clone(), if completion.is_dir { "" } else { " " })
            }
            // Mention: `@{path} ` — quote the path when it contains whitespace (`autocomplete.ts:408`
            // `@"…"`); always a trailing space (a mention is a complete token).
            CompletionContext::Mention => {
                let path = &completion.value;
                let rendered = if path.contains(char::is_whitespace) {
                    format!("@\"{path}\"")
                } else {
                    format!("@{path}")
                };
                (rendered, " ")
            }
        };
        let new_before = format!("{head}{insert}{trailing}");
        let cursor_col = new_before.chars().count();
        let new_line = format!("{new_before}{after}");
        let mut new_lines = lines.to_vec();
        if let Some(slot) = new_lines.get_mut(cursor_line) {
            *slot = new_line;
        }
        Some(Applied { lines: new_lines, cursor_line, cursor_col })
    }
}

/// Slash context (`autocomplete.ts:313-363` @v0.84.3): `before` starts with `/`, split on the FIRST
/// SPACE (`:314` `textBeforeCursor.indexOf(" ")` — a literal space, not any whitespace, so a tab
/// keeps the line in the name branch). No space → the command-name list (`:316-341`); a space → the
/// argument list (`:344-363`), which never falls back to the name list.
fn slash_context(
    registry: &CommandRegistry,
    arguments: &ArgumentSources,
    before: &str,
) -> Option<Autocomplete> {
    if !before.starts_with('/') {
        return None;
    }
    if before.contains(' ') {
        let (completer, name, argument) = argument_completer(registry, before)?;
        return argument_context(arguments, completer, name, argument);
    }
    command_name_context(registry, before)
}

/// `commandName = textBeforeCursor.slice(1, spaceIndex)` (`:344`), `argumentText =
/// textBeforeCursor.slice(spaceIndex + 1)` (`:345`), the registry lookup (`:347-350`) and the
/// "no `getArgumentCompletions`" refusal (`:351-353`) as one resolution step — also the terminal-arm
/// predicate in [`Autocomplete::compute`]. `str::get`, never a slice expression
/// (`deny(clippy::string_slice)`).
///
/// The command NAME rides back out alongside the completer because
/// [`ArgumentCompleter::Extension`] is one tag shared by every registered extension command — the
/// name is how its own completer is identified, standing in for the per-command closure pi binds
/// (`interactive-mode.ts:753` @v0.84.3).
pub(crate) fn argument_completer<'a>(
    registry: &CommandRegistry,
    before: &'a str,
) -> Option<(ArgumentCompleter, &'a str, &'a str)> {
    let rest = before.strip_prefix('/')?;
    let space = rest.find(' ')?;
    let name = rest.get(..space)?;
    let argument = rest.get(space + 1..)?;
    let completer = registry.get(name)?.arg_completion;
    (completer != ArgumentCompleter::None).then_some((completer, name, argument))
}

/// Build an argument popup: one of the three builtin completers (`interactive-mode.ts:685-736`
/// @v0.84.3) or an extension command's own (`:753`).
///
/// Each builtin branch is `createFuzzyAutocompleteItems` (`:288-297`): fuzzy-filter the candidates
/// by their per-completer search text and return `None` — upstream's `null` — for an empty result.
/// The extension branch does not filter what the completer already narrowed; see
/// [`extension_rows`]. The layout is [`ColumnLayout::DEFAULT`], **not** `SLASH`: pi picks the slash
/// layout only when the PREFIX starts with `/` (`components/editor.ts:2148` @v0.84.3), and an
/// argument prefix never does.
fn argument_context(
    arguments: &ArgumentSources,
    completer: ArgumentCompleter,
    name: &str,
    argument: &str,
) -> Option<Autocomplete> {
    let (items, completions) = match completer {
        // Unreachable through `argument_completer`, which filters `None` out; matched rather than
        // `unreachable!()` because the workspace denies `clippy::panic`.
        ArgumentCompleter::None => return None,
        ArgumentCompleter::Models => model_rows(&arguments.models, argument)?,
        ArgumentCompleter::LoginProviders => {
            login_provider_rows(&arguments.login_providers, argument)?
        }
        ArgumentCompleter::ThinkingLevels => thinking_rows(&arguments.thinking_levels, argument)?,
        ArgumentCompleter::Extension => {
            extension_rows(arguments.extension_completions.get(name), argument)?
        }
    };
    let list = SelectList::new(items, ColumnLayout::DEFAULT).with_no_match("No matches");
    Some(Autocomplete {
        context: CompletionContext::SlashArgument,
        prefix: argument.to_string(),
        completions,
        list,
    })
}

/// `/model` rows (`interactive-mode.ts:687-710` @v0.84.3): label `id`, description `provider`,
/// inserted value `provider/id`.
fn model_rows(
    models: &[ModelArgument],
    argument: &str,
) -> Option<(Vec<SelectItem>, Vec<Completion>)> {
    // `getModelSearchText` verbatim (`modes/interactive/model-search.ts:7-11` @v0.84.3):
    // `` `${id} ${provider} ${provider}/${id} ${provider} ${id}${name}` ``. The repetition IS the
    // ranking — `fuzzy::filter` scores per token occurrence — so it is ported, not tidied.
    let texts: Vec<String> = models
        .iter()
        .map(|m| {
            let name = if m.name.is_empty() { String::new() } else { format!(" {}", m.name) };
            format!("{id} {p} {p}/{id} {p} {id}{name}", id = m.id, p = m.provider)
        })
        .collect();
    let matches = fuzzy::filter(&texts, argument, String::as_str);
    if matches.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(matches.len());
    let mut completions = Vec::with_capacity(matches.len());
    for m in &matches {
        let Some(model) = models.get(m.index) else { continue };
        items.push(SelectItem::new(model.id.clone(), Some(model.provider.clone())));
        completions.push(Completion {
            value: format!("{}/{}", model.provider, model.id),
            is_dir: false,
        });
    }
    Some((items, completions))
}

/// `/login` rows (`interactive-mode.ts:728-735` @v0.84.3): label and inserted value are the bare
/// provider id, the description is `formatLoginProviderCompletionDescription` (`:328-331`).
fn login_provider_rows(
    providers: &[LoginProviderArgument],
    argument: &str,
) -> Option<(Vec<SelectItem>, Vec<Completion>)> {
    // `getLoginProviderSearchText` (`:321-326`): `` `${id} ${name} ${authTypes}` `` where each auth
    // type contributes `` `${authType} ${formatAuthSelectorProviderType(authType)}` `` — the wire
    // key AND the human label, so both `oauth` and `subscription` find the row.
    let texts: Vec<String> = providers
        .iter()
        .map(|p| {
            let mut text = format!("{} {}", p.id, p.name);
            for auth in &p.auth_types {
                text.push_str(&format!(
                    " {} {}",
                    auth.as_str(),
                    crate::auth_select::format_auth_selector_provider_type(*auth)
                ));
            }
            text
        })
        .collect();
    let matches = fuzzy::filter(&texts, argument, String::as_str);
    if matches.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(matches.len());
    let mut completions = Vec::with_capacity(matches.len());
    for m in &matches {
        let Some(provider) = providers.get(m.index) else { continue };
        let labels: Vec<&str> = provider
            .auth_types
            .iter()
            .map(|a| crate::auth_select::format_auth_selector_provider_type(*a))
            .collect();
        let joined = labels.join("/");
        // `provider.name === provider.id ? authTypes : `${provider.name} · ${authTypes}`` (`:330`).
        let desc = if provider.name == provider.id {
            joined
        } else {
            format!("{} · {joined}", provider.name)
        };
        items.push(SelectItem::new(provider.id.clone(), Some(desc)));
        completions.push(Completion { value: provider.id.clone(), is_dir: false });
    }
    Some((items, completions))
}

/// An extension command's own rows (`getArgumentCompletions`, `interactive-mode.ts:753` @v0.84.3):
/// the guest answers a `list<string>` (`cyrup-ext/wit/world.wit:250`), so each string is its own
/// label and inserted value, and there is no description column.
///
/// `None` — upstream's `null` — for a command with nothing cached yet and for an empty answer
/// (`autocomplete.ts:356-357`), which closes the popup rather than falling through to a path
/// listing (`Autocomplete::compute`'s terminal arm).
///
/// Ranking: when the cached answer was fetched for exactly the argument on screen it is used
/// VERBATIM, because that is what pi does with a completer's return value — filtering a
/// server-side-narrowed list again would drop rows the completer deliberately offered. Between a
/// keystroke and the fetch that follows it the cache belongs to the previous prefix, and there the
/// stale set is narrowed with [`fuzzy::filter`]: the alternative is showing rows that no longer
/// match what is typed for one frame.
fn extension_rows(
    cached: Option<&ExtensionCompletions>,
    argument: &str,
) -> Option<(Vec<SelectItem>, Vec<Completion>)> {
    let cached = cached?;
    if cached.items.is_empty() {
        return None;
    }
    let selected: Vec<&String> = if cached.prefix == argument {
        cached.items.iter().collect()
    } else {
        fuzzy::filter(&cached.items, argument, String::as_str)
            .iter()
            .filter_map(|m| cached.items.get(m.index))
            .collect()
    };
    if selected.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(selected.len());
    let mut completions = Vec::with_capacity(selected.len());
    for value in selected {
        items.push(SelectItem::label(value.clone()));
        completions.push(Completion { value: value.clone(), is_dir: false });
    }
    Some((items, completions))
}

/// `/thinking` rows (`interactive-mode.ts:713-725` @v0.84.3): the level string is its own search
/// text, label and value.
fn thinking_rows(
    levels: &[String],
    argument: &str,
) -> Option<(Vec<SelectItem>, Vec<Completion>)> {
    let matches = fuzzy::filter(levels, argument, String::as_str);
    if matches.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(matches.len());
    let mut completions = Vec::with_capacity(matches.len());
    for m in &matches {
        let Some(level) = levels.get(m.index) else { continue };
        items.push(SelectItem::label(level.clone()));
        completions.push(Completion { value: level.clone(), is_dir: false });
    }
    Some((items, completions))
}

/// The command-NAME list (`autocomplete.ts:316-341` @v0.84.3): fuzzy-filter every registered command
/// by the text after the `/`, with `prefix = textBeforeCursor` (`:340`) — the whole `/…` token, which
/// is what [`Autocomplete::apply`]'s slash arm replaces.
fn command_name_context(registry: &CommandRegistry, before: &str) -> Option<Autocomplete> {
    let query = before.get(1..).unwrap_or("");
    let commands = registry.commands();
    let matches = fuzzy::filter(commands, query, |c| c.name.as_ref());
    if matches.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(matches.len());
    let mut completions = Vec::with_capacity(matches.len());
    for m in &matches {
        let Some(cmd) = commands.get(m.index) else { continue };
        // Description composition (`:315-322`): hint ? (desc ? "{hint} — {desc}" : hint) : desc.
        let desc = match cmd.argument_hint.as_deref() {
            Some(hint) if !cmd.description.is_empty() => format!("{hint} — {}", cmd.description),
            Some(hint) => hint.to_string(),
            None => cmd.description.to_string(),
        };
        items.push(SelectItem::new(cmd.name.to_string(), Some(desc)));
        completions.push(Completion { value: cmd.name.to_string(), is_dir: false });
    }
    let list = SelectList::new(items, ColumnLayout::SLASH).with_no_match("No matching commands");
    Some(Autocomplete { context: CompletionContext::Slash, prefix: before.to_string(), completions, list })
}

/// Whether `query` (the text after the leading `/`) is a real PREFIX of at least one registered
/// command name.
///
/// CMDHINT_01 — deliberately **not** the fuzzy matcher `slash_context` uses (`fuzzy::filter`,
/// `fuzzy.rs:143`). Fuzzy is right for a *suggestion list* — it splits the query on `/` as well as
/// whitespace (`:144`) and scores non-contiguous subsequences — and wrong for this signal, which
/// claims "what you typed is literally the start of a real command": `/fa` fuzzy-matches `flux/aug`
/// (`f`→`a` is a subsequence) and must NOT be highlighted as a real path segment. An empty query (a
/// bare `/`) is false — `fuzzy::filter` returns EVERYTHING for it (`:145-151`), which is correct for
/// the popup and meaningless as a confirmation.
pub fn is_command_prefix(registry: &CommandRegistry, query: &str) -> bool {
    !query.is_empty() && registry.commands().iter().any(|c| c.name.as_ref().starts_with(query))
}

/// Path delimiters that bound the trailing token (`PATH_DELIMITERS`, `autocomplete.ts:7`).
const PATH_DELIMS: [char; 5] = [' ', '\t', '"', '\'', '='];

/// Bare-path context (`extractPathPrefix` `:480-507` + `getFileSuggestions` `:560-693`).
fn path_context(before: &str, force: bool, cwd: &Path) -> Option<Autocomplete> {
    let raw_token = trailing_token(before);
    // `parsePathPrefix` (`autocomplete.ts:94-105`): the opening `"` of a quoted prefix is not part
    // of the path. TUI-013 — `trailing_token` now returns the whole `"my dir/fi` span.
    let quoted = raw_token.starts_with('"');
    let token = raw_token.strip_prefix('"').unwrap_or(&raw_token).to_string();
    let looks_pathy =
        quoted || token.contains('/') || token.starts_with('.') || token.starts_with("~/");
    if !force && !looks_pathy {
        return None;
    }
    // Split the token into a directory part + a filename prefix.
    let (dir_part, name_prefix) = match token.rfind('/') {
        Some(idx) => (token.get(..=idx).unwrap_or(""), token.get(idx + 1..).unwrap_or("")),
        None => ("", token.as_str()),
    };
    let search_dir = resolve_dir(dir_part, cwd);
    let entries = read_dir_sorted(&search_dir, name_prefix)?;
    if entries.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(entries.len());
    let mut completions = Vec::with_capacity(entries.len());
    for (name, is_dir) in entries {
        let display = if is_dir { format!("{name}/") } else { name.clone() };
        // The inserted value re-prepends the directory part the user already typed.
        let value = format!("{dir_part}{display}");
        items.push(SelectItem::label(display.clone()));
        completions.push(Completion { value, is_dir });
    }
    let list = SelectList::new(items, ColumnLayout::DEFAULT).with_no_match("No matching files");
    // The replaced span is the RAW token, quote included, so applying a completion overwrites the
    // opening quote the user typed rather than leaving it stranded.
    Some(Autocomplete { context: CompletionContext::Path, prefix: raw_token, completions, list })
}

/// `findUnclosedQuoteStart` (`packages/tui/src/autocomplete.ts:54-68` @v0.83.0): the byte index of
/// the `"` that opened a quote never closed, or `None` when every `"` is balanced. TUI-013.
fn find_unclosed_quote_start(text: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut quote_start = 0usize;
    for (i, ch) in text.char_indices() {
        if ch == '"' {
            in_quotes = !in_quotes;
            if in_quotes {
                quote_start = i;
            }
        }
    }
    in_quotes.then_some(quote_start)
}

/// `isTokenStart` (`autocomplete.ts:70-72`): index 0, or preceded by a [`PATH_DELIMS`] character.
fn is_token_start(text: &str, index: usize) -> bool {
    if index == 0 {
        return true;
    }
    text.get(..index)
        .and_then(|s| s.chars().next_back())
        .is_some_and(|c| PATH_DELIMS.contains(&c))
}

/// `extractQuotedPrefix` (`autocomplete.ts:74-92`): when a quote is open, the token is everything
/// from that quote (or from the `@` immediately before it) to the cursor — spaces included.
fn extract_quoted_prefix(text: &str) -> Option<String> {
    let quote_start = find_unclosed_quote_start(text)?;
    // `@"my dir/fi` — the `@` belongs to the token (`:80-85`).
    if quote_start > 0
        && text.get(..quote_start).and_then(|s| s.chars().next_back()) == Some('@')
    {
        let at = quote_start - 1;
        if !is_token_start(text, at) {
            return None;
        }
        return text.get(at..).map(str::to_string);
    }
    if !is_token_start(text, quote_start) {
        return None;
    }
    text.get(quote_start..).map(str::to_string)
}

/// The trailing token of `before`, bounded by [`PATH_DELIMS`] / start-of-line.
///
/// **TUI-013.** An unclosed quote wins over the delimiter split, exactly as
/// `extractPathPrefix`/`extractAtPrefix` order the two upstream (`autocomplete.ts:463-470` and
/// `:480-487`: `const quotedPrefix = extractQuotedPrefix(text); if (quotedPrefix) return
/// quotedPrefix;` **before** `findLastDelimiter`). Without it `"` is itself a `PATH_DELIMS`
/// character, so `see @"my dir/fi` split on the SPACE INSIDE the quotes and yielded `dir/fi`, whose
/// `strip_prefix('@')` then failed — any path containing a space was uncompletable.
fn trailing_token(before: &str) -> String {
    if let Some(quoted) = extract_quoted_prefix(before) {
        return quoted;
    }
    match before.rfind(PATH_DELIMS) {
        Some(idx) => before.get(idx + 1..).unwrap_or("").to_string(),
        None => before.to_string(),
    }
}

/// Resolve the directory part of a path token against `cwd` (handles `""`, `./`, `~/`, absolute).
fn resolve_dir(dir_part: &str, cwd: &Path) -> PathBuf {
    if dir_part.is_empty() || dir_part == "./" {
        return cwd.to_path_buf();
    }
    if let Some(rest) = dir_part.strip_prefix("~/")
        && let Some(home) = home_dir() {
            return home.join(rest);
        }
    let p = Path::new(dir_part);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// Read `dir`, keep entries whose name case-insensitively starts with `prefix`, sort directories
/// first then by name (`getFileSuggestions` sort, `:680-686`). `None` if the dir cannot be read.
fn read_dir_sorted(dir: &Path, prefix: &str) -> Option<Vec<(String, bool)>> {
    let rd = std::fs::read_dir(dir).ok()?;
    let prefix_lower = prefix.to_lowercase();
    let mut out: Vec<(String, bool)> = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix_lower) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push((name, is_dir));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    Some(out)
}

/// `$HOME` as a `PathBuf`, if set.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

// ----------------------------------------------------------------- @-mention search ----

/// The `@`-mention query under the cursor (`autocomplete.ts:101` `@`-prefix detect), if the trailing
/// token is a mention. Returns the text *after* the `@`, with a leading `"` stripped (the `@"quoted
/// path"` form, `:408`). `Some("")` immediately after typing `@` (open the popup with the whole tree).
pub fn mention_query(before: &str) -> Option<String> {
    let token = trailing_token(before);
    let rest = token.strip_prefix('@')?;
    Some(rest.strip_prefix('"').unwrap_or(rest).to_string())
}

/// Build the `@`-mention completion popup (`autocomplete.ts:164,408`): fuzzy-rank the whole-tree
/// `candidates` (repo-relative paths from [`list_files`]) by the `@`-query, newest-best first. `prefix`
/// is the literal `@query` token the apply step replaces. `None` when nothing matches (closes the popup).
pub fn mention_autocomplete(before: &str, candidates: &[String]) -> Option<Autocomplete> {
    let query = mention_query(before)?;
    let token = trailing_token(before);
    let matches = fuzzy::filter(candidates, &query, |c| c.as_str());
    if matches.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(matches.len());
    let mut completions = Vec::with_capacity(matches.len());
    for m in &matches {
        let Some(path) = candidates.get(m.index) else { continue };
        items.push(SelectItem::label(path.clone()));
        completions.push(Completion { value: path.clone(), is_dir: false });
    }
    let list = SelectList::new(items, ColumnLayout::DEFAULT).with_no_match("No matching files");
    Some(Autocomplete { context: CompletionContext::Mention, prefix: token, completions, list })
}

/// List repo files for `@`-mention search (`autocomplete.ts:719-772`), capped at `limit`. Prefers
/// `fd` (fast + `.gitignore`-aware), falling back to a bounded in-process walk when `fd` is absent so
/// the feature works with no external tool. Returns paths **relative** to `cwd`, `/`-separated.
pub fn list_files(cwd: &Path, limit: usize) -> Vec<String> {
    if let Some(files) = fd_list(cwd, limit) {
        return files;
    }
    walk_list(cwd, limit)
}

/// Spawn `fd` to enumerate tracked files (`.gitignore`-aware, hidden excluded except dotfiles fd shows
/// by default-off). `--strip-cwd-prefix` yields repo-relative paths. `None` when `fd` is unavailable or
/// errors, so the caller falls back to the in-process walk.
fn fd_list(cwd: &Path, limit: usize) -> Option<Vec<String>> {
    let output = std::process::Command::new("fd")
        .args(["--type", "f", "--color", "never", "--strip-cwd-prefix", "--exclude", ".git"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut files: Vec<String> = text
        .lines()
        .filter(|l| !l.is_empty())
        .take(limit)
        .map(|l| l.replace('\\', "/"))
        .collect();
    files.sort();
    Some(files)
}

/// A bounded breadth-first walk used when `fd` is not installed: visits at most `limit * 8` entries,
/// skips VCS/build noise (`.git`, `node_modules`, `target`, `.cyrup`) and returns up to `limit`
/// `/`-separated repo-relative file paths.
fn walk_list(cwd: &Path, limit: usize) -> Vec<String> {
    const SKIP: [&str; 5] = [".git", "node_modules", "target", ".cyrup", ".jj"];
    let visit_cap = limit.saturating_mul(8).max(limit);
    let mut out: Vec<String> = Vec::new();
    let mut queue: std::collections::VecDeque<PathBuf> = std::collections::VecDeque::new();
    queue.push_back(cwd.to_path_buf());
    let mut visited = 0usize;
    while let Some(dir) = queue.pop_front() {
        if out.len() >= limit || visited >= visit_cap {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            visited += 1;
            if out.len() >= limit || visited >= visit_cap {
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            let path = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                queue.push_back(path);
            } else if let Ok(rel) = path.strip_prefix(cwd) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}
