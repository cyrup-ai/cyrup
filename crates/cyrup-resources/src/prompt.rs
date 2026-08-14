//! Prompt templates — markdown expanded by `/name args` with shell-style positional argument
//! substitution (arch-09 §3.4, R-09-007..010).
//!
//! Ports Pi's `prompt-templates.ts` 1:1: `parseCommandArgs` (quote-aware tokenizer,
//! prompt-templates.ts:24-55), `substituteArgs` (`$1 $2 $@ $ARGUMENTS ${N:-default} ${@:N}
//! ${@:N:L}`, prompt-templates.ts:69-101), and `expandPromptTemplate` (`/name args` entry point,
//! prompt-templates.ts:268-284). The template `name` is the file basename minus `.md` with case
//! preserved (prompt-templates.ts:108); `description` comes from frontmatter or the first non-empty
//! body line truncated to 60 chars (prompt-templates.ts:110-119); `argument-hint` is read from
//! frontmatter (prompt-templates.ts:124).

use std::path::{Path, PathBuf};

use crate::discovery::Named;
use crate::error::ResourceError;
use crate::key::ResourceKey;
use crate::scope::{ResourceOrigin, ResourceScope};
use crate::skill::split_front_matter;

/// Frontmatter description truncation limit (prompt-templates.ts:116).
const DESCRIPTION_TRUNCATE: usize = 60;

/// A markdown prompt template. Bodies are small and eagerly cached (R-09-025).
#[derive(Clone, Debug)]
pub struct PromptTemplate {
    /// Normalized registry key (lower-cased) used for precedence/collision (R-09-024).
    pub key: ResourceKey,
    /// The command name = file basename minus `.md`, **case preserved**
    /// (prompt-templates.ts:108). `/name` matching is case-sensitive on this.
    pub name: String,
    /// From frontmatter `description`, or the first non-empty body line truncated to 60 chars +
    /// `...` (prompt-templates.ts:110-119).
    pub description: String,
    /// Frontmatter `argument-hint` field, surfaced in the command list (prompt-templates.ts:124).
    pub argument_hint: Option<String>,
    pub path: PathBuf,
    /// Body with frontmatter stripped — the template content that gets `substituteArgs` applied.
    pub body: String,
    pub scope: ResourceScope,
    pub origin: ResourceOrigin,
}

impl PromptTemplate {
    /// Load a template from a markdown file (prompt-templates.ts:103-132).
    pub fn load(
        path: &Path,
        scope: ResourceScope,
        origin: ResourceOrigin,
    ) -> Result<PromptTemplate, ResourceError> {
        let raw = std::fs::read_to_string(path)?;
        let (frontmatter, body) = parse_frontmatter(&raw);

        // name = basename minus `.md`, case preserved (prompt-templates.ts:108).
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|f| f.strip_suffix(".md").unwrap_or(f).to_string())
            .unwrap_or_default();
        let key = ResourceKey::normalize(&name);
        if key.is_empty() {
            return Err(ResourceError::Manifest(format!(
                "prompt template has no usable name: {}",
                path.display()
            )));
        }

        // description: frontmatter, else first non-empty body line (truncated) (…ts:110-119).
        let description = match frontmatter_str(&frontmatter, "description") {
            Some(d) if !d.is_empty() => d,
            _ => first_line_description(&body),
        };
        let argument_hint =
            frontmatter_str(&frontmatter, "argument-hint").filter(|s| !s.is_empty());

        Ok(PromptTemplate {
            key,
            name,
            description,
            argument_hint,
            path: path.to_path_buf(),
            body,
            scope,
            origin,
        })
    }

    /// Expand this template against a raw args string (everything after `/name `), tokenizing it
    /// with [`parse_command_args`] then applying [`substitute_args`] (prompt-templates.ts:279-280).
    pub fn expand(&self, args_string: &str) -> String {
        let args = parse_command_args(args_string);
        substitute_args(&self.body, &args)
    }
}

impl Named for PromptTemplate {
    fn key(&self) -> &ResourceKey {
        &self.key
    }
    fn scope(&self) -> ResourceScope {
        self.scope
    }
}

/// Expand a `/name args` line against a set of templates (prompt-templates.ts:268-284).
///
/// Returns the substituted content when `text` is `^/<name>(\s+<args>)?$` and `<name>` matches a
/// template (case-sensitive); otherwise returns `text` unchanged.
pub fn expand_prompt_template<'a, I>(text: &str, templates: I) -> String
where
    I: IntoIterator<Item = &'a PromptTemplate>,
{
    let Some((name, args_string)) = split_command(text) else {
        return text.to_string();
    };
    for t in templates {
        if t.name == name {
            let args = parse_command_args(args_string);
            return substitute_args(&t.body, &args);
        }
    }
    text.to_string()
}

/// Split `/name args` into `(name, args_string)`, mirroring the regex
/// `^/([^\s]+)(?:\s+([\s\S]*))?$` (prompt-templates.ts:271). Returns `None` when `text` does not
/// start with `/` or has no non-whitespace name.
fn split_command(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('/')?;
    // name = leading run of non-whitespace chars.
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = rest.get(..name_end)?;
    if name.is_empty() {
        return None;
    }
    // `(?:\s+([\s\S]*))?` — skip the whitespace run, rest is the args string.
    let after = rest.get(name_end..).unwrap_or("");
    let args = after.trim_start_matches(char::is_whitespace);
    Some((name, args))
}

/// Tokenize an args string respecting single/double quotes (prompt-templates.ts:24-55).
///
/// Quotes group whitespace-separated tokens; the quote characters themselves are dropped. There is
/// no escape handling (Pi has none) and unterminated quotes simply absorb the remainder.
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for ch in args_string.chars() {
        match in_quote {
            Some(q) => {
                if ch == q {
                    in_quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    in_quote = Some(ch);
                } else if ch.is_whitespace() {
                    if !current.is_empty() {
                        args.push(std::mem::take(&mut current));
                    }
                } else {
                    current.push(ch);
                }
            }
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Substitute positional argument placeholders in `content` (prompt-templates.ts:69-101).
///
/// Supported forms:
/// - `$1`, `$2`, … — positional args (1-indexed; missing → empty)
/// - `$@`, `$ARGUMENTS` — all args joined by a space
/// - `${N:-default}` — positional N, or `default` when missing/empty
/// - `${@:N}` — args from the Nth onward (1-indexed, bash slice)
/// - `${@:N:L}` — `L` args starting from the Nth
///
/// Substitution applies to the template only; argument/default values are not re-scanned.
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let all_args = args.join(" ");
    let bytes = content.as_bytes();
    let mut out = String::with_capacity(content.len());
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes.get(i) != Some(&b'$') {
            // Copy the next UTF-8 char intact.
            push_char_at(content, &mut out, &mut i);
            continue;
        }
        // We are at a `$`. Try the `${...}` forms first, then the simple `$X` forms.
        if bytes.get(i + 1) == Some(&b'{') {
            if let Some((repl, consumed)) = match_brace_form(content, i, args, &all_args) {
                out.push_str(&repl);
                i += consumed;
                continue;
            }
            // Unrecognized `${...}` — emit the `$` literally and continue.
            out.push('$');
            i += 1;
            continue;
        }
        if let Some((repl, consumed)) = match_simple_form(content, i, args, &all_args) {
            out.push_str(&repl);
            i += consumed;
            continue;
        }
        // A lone `$` not starting any placeholder.
        out.push('$');
        i += 1;
    }
    out
}

/// Copy one UTF-8 char starting at byte `*i`, advancing `*i` past it.
fn push_char_at(s: &str, out: &mut String, i: &mut usize) {
    if let Some(ch) = s.get(*i..).and_then(|t| t.chars().next()) {
        out.push(ch);
        *i += ch.len_utf8();
    } else {
        *i += 1;
    }
}

/// Match `${N:-default}`, `${@:N}`, or `${@:N:L}` at byte `start`. Returns `(replacement,
/// bytes_consumed)` or `None` if the `${...}` is not one of these forms.
fn match_brace_form(
    content: &str,
    start: usize,
    args: &[String],
    all_args: &str,
) -> Option<(String, usize)> {
    let open = start + 2; // past `${`
    let rest = content.get(open..)?;
    let close_rel = rest.find('}')?;
    let inner = rest.get(..close_rel)?;
    let consumed = 2 + close_rel + 1; // `${` + inner + `}`

    // `${N:-default}` / `${@:-default}` / `${ARGUMENTS:-default}` — pi's first regex alternative is
    // `\$\{(\d+|ARGUMENTS|@):-([^}]*)\}` (prompt-templates.ts:74 @v0.83.0), i.e. the target may be
    // `@` or `ARGUMENTS` as well as a positional number (CFG-017), and the handler at `:78-79` is
    // `const value = target === "@" || target === "ARGUMENTS" ? allArgs : args[parseInt(target)-1];
    //  return value ? value : defaultValue;`.
    if let Some((target, default)) = inner.split_once(":-")
        && !target.is_empty()
    {
        let value: Option<String> = if target == "@" || target == "ARGUMENTS" {
            Some(all_args.to_string())
        } else if target.bytes().all(|b| b.is_ascii_digit()) {
            // `args[parseInt("0", 10) - 1]` is `args[-1]` — `undefined`, hence falsy, hence the
            // DEFAULT (CFG-016). `checked_sub(1)?` used to abort the whole form instead, leaving
            // `${0:-default}` in the rendered prompt verbatim.
            target
                .parse::<usize>()
                .ok()?
                .checked_sub(1)
                .and_then(|i| args.get(i))
                .cloned()
        } else {
            // Not one of pi's three targets: the regex alternative does not match, so the token is
            // not a placeholder at all.
            return None;
        };
        // JS truthiness: `undefined` AND `""` both take the default.
        let value = value.filter(|v| !v.is_empty());
        return Some((value.unwrap_or_else(|| default.to_string()), consumed));
    }

    // `${@:N}` / `${@:N:L}`
    if let Some(slice) = inner.strip_prefix("@:") {
        let (start_str, len_str) = match slice.split_once(':') {
            Some((a, b)) => (a, Some(b)),
            None => (slice, None),
        };
        if start_str.is_empty() || !start_str.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: usize = start_str.parse().ok()?;
        // 1-indexed → 0-indexed; bash treats 0 as 1 (prompt-templates.ts:82-84).
        let begin = n.saturating_sub(1);
        let joined = match len_str {
            Some(l) => {
                if !l.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let length: usize = l.parse().ok()?;
                let end = begin.saturating_add(length).min(args.len());
                args.get(begin..end).unwrap_or(&[]).join(" ")
            }
            None => args.get(begin..).unwrap_or(&[]).join(" "),
        };
        return Some((joined, consumed));
    }

    None
}

/// Match `$ARGUMENTS`, `$@`, or `$<digits>` at byte `start`. Returns `(replacement,
/// bytes_consumed)`.
fn match_simple_form(
    content: &str,
    start: usize,
    args: &[String],
    all_args: &str,
) -> Option<(String, usize)> {
    let rest = content.get(start + 1..)?;
    if let Some(after) = rest.strip_prefix("ARGUMENTS") {
        let consumed = 1 + "ARGUMENTS".len();
        let _ = after;
        return Some((all_args.to_string(), consumed));
    }
    if rest.starts_with('@') {
        return Some((all_args.to_string(), 2));
    }
    // `$<digits>`
    let digits_len = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits_len == 0 {
        return None;
    }
    let num = rest.get(..digits_len)?;
    let idx = num.parse::<usize>().ok()?.checked_sub(1);
    let value = idx.and_then(|i| args.get(i)).cloned().unwrap_or_default();
    Some((value, 1 + digits_len))
}

// ---------------------------------------------------------------------------
// frontmatter parsing for prompt templates (utils/frontmatter.ts)
// ---------------------------------------------------------------------------

/// Parse the leading `---` YAML frontmatter block, returning `(frontmatter_map, body)`. Mirrors
/// `parseFrontmatter` (utils/frontmatter.ts): no fence → empty map + whole content as body.
fn parse_frontmatter(raw: &str) -> (std::collections::BTreeMap<String, serde_yml::Value>, String) {
    let (yaml, body) = split_front_matter(raw);
    match yaml {
        Some(front) => {
            // Pi silently treats a YAML parse fault as `{}` (prompt-templates.ts:129-131).
            let map =
                serde_yml::from_str::<std::collections::BTreeMap<String, serde_yml::Value>>(&front)
                    .unwrap_or_default();
            (map, body)
        }
        // No fence → empty frontmatter + the normalized whole content (frontmatter.ts:14,19,33).
        None => (std::collections::BTreeMap::new(), body),
    }
}

/// Read a frontmatter key as a trimmed string (scalar values only).
fn frontmatter_str(
    map: &std::collections::BTreeMap<String, serde_yml::Value>,
    key: &str,
) -> Option<String> {
    let v = map.get(key)?;
    v.as_str().map(|s| s.to_string())
}

/// First non-empty body line, truncated to 60 chars with `...` appended when longer
/// (prompt-templates.ts:112-118).
fn first_line_description(body: &str) -> String {
    let Some(line) = body.lines().find(|l| !l.trim().is_empty()) else {
        return String::new();
    };
    if line.chars().count() > DESCRIPTION_TRUNCATE {
        let truncated: String = line.chars().take(DESCRIPTION_TRUNCATE).collect();
        format!("{truncated}...")
    } else {
        line.to_string()
    }
}
