//! The autocomplete engine — context resolution, slash + path completion, apply (spec/tui/04 §3).
//!
//! Port of `pi-tui/src/autocomplete.ts` (`CombinedAutocompleteProvider`) for the two **synchronous**
//! contexts cyrup can resolve without spawning a subprocess:
//!
//! 1. **Slash command** (`autocomplete.ts:308-337`): the line begins with `/` and has no space yet →
//!    fuzzy-filtered command list (via [`crate::fuzzy`] + the [`CommandRegistry`]).
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

use std::path::{Path, PathBuf};

use crate::commands::CommandRegistry;
use crate::fuzzy;
use crate::select_list::{ColumnLayout, SelectItem, SelectList};

/// Which completion context produced the active popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionContext {
    /// Slash-command name completion (replaces the entire `/…` token, re-adds `/` + trailing space).
    Slash,
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
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
        cwd: &Path,
    ) -> Option<Autocomplete> {
        let line = lines.get(cursor_line).map(String::as_str).unwrap_or("");
        let before: String = line.chars().take(cursor_col).collect();

        // 1. Slash command — only when not a forced (Tab) path completion (`:308`).
        if !force
            && let Some(ac) = slash_context(registry, &before) {
                return Some(ac);
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

/// Slash-command context (`autocomplete.ts:308-337`): `before` starts with `/` and has no space.
fn slash_context(registry: &CommandRegistry, before: &str) -> Option<Autocomplete> {
    if !before.starts_with('/') || before.contains(char::is_whitespace) {
        return None;
    }
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

/// Path delimiters that bound the trailing token (`PATH_DELIMITERS`, `autocomplete.ts:7`).
const PATH_DELIMS: [char; 5] = [' ', '\t', '"', '\'', '='];

/// Bare-path context (`extractPathPrefix` `:480-507` + `getFileSuggestions` `:560-693`).
fn path_context(before: &str, force: bool, cwd: &Path) -> Option<Autocomplete> {
    let token = trailing_token(before);
    let looks_pathy = token.contains('/') || token.starts_with('.') || token.starts_with("~/");
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
    Some(Autocomplete { context: CompletionContext::Path, prefix: token, completions, list })
}

/// The trailing token of `before`, bounded by [`PATH_DELIMS`] / start-of-line.
fn trailing_token(before: &str) -> String {
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
