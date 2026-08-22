use super::*;

/// A coalesced string argument (`args.file_path ?? args.path`, then Pi's `str()`): a present non-string
/// → [`StrArg::Invalid`] (`[invalid arg]`), absent/null/`""` → [`StrArg::Missing`], else the string.
pub(super) enum StrArg {
    Invalid,
    Missing,
    Value(String),
}

/// `args[key0] ?? args[key1] ?? …` then `str()` (render-utils.ts:25-29): skip absent/JSON-null keys, a
/// non-string value is `Invalid`, an empty string is `Missing`.
pub(super) fn str_arg(args: &Value, keys: &[&str]) -> StrArg {
    for k in keys {
        match args.get(k) {
            None | Some(Value::Null) => continue,
            Some(Value::String(s)) => {
                return if s.is_empty() { StrArg::Missing } else { StrArg::Value(s.clone()) };
            }
            Some(_) => return StrArg::Invalid,
        }
    }
    StrArg::Missing
}

/// `renderToolPath` (render-utils.ts:75-85): `[invalid arg]` for a non-string, the `emptyFallback`
/// (else `...`) for an empty/absent path, otherwise the `~`-shortened path in accent. Hyperlinks are a
/// terminal escape the cell grid does not carry (tracked residual).
pub(super) fn tool_path_span(
    args: &Value,
    keys: &[&str],
    empty_fallback: Option<&str>,
    theme: &UiTheme,
) -> Span<'static> {
    match str_arg(args, keys) {
        StrArg::Invalid => Span::styled("[invalid arg]".to_string(), theme.error_style()),
        StrArg::Missing => match empty_fallback {
            Some(f) => Span::styled(shorten_path(f), theme.accent_style()),
            None => Span::styled("...".to_string(), theme.tool_output_style()),
        },
        StrArg::Value(p) => Span::styled(shorten_path(&p), theme.accent_style()),
    }
}

/// The `" in <path>"` tail shared by grep/find (`path = shortenPath(rawPath || ".")` in `toolOutput`, a
/// non-string → `[invalid arg]`). The caller has already pushed the `" in "` label span.
pub(super) fn push_search_path(args: &Value, theme: &UiTheme, spans: &mut Vec<Span<'static>>) {
    match str_arg(args, &["path"]) {
        StrArg::Invalid => spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style())),
        StrArg::Missing => {
            spans.push(Span::styled(shorten_path("."), theme.tool_output_style()));
        }
        StrArg::Value(p) => spans.push(Span::styled(shorten_path(&p), theme.tool_output_style())),
    }
}

/// `formatReadLineRange` (read.ts:67-72): `:<start>` or `:<start>-<end>` from `offset`/`limit`.
pub(super) fn read_line_range(args: &Value) -> Option<String> {
    let offset = args.get("offset").and_then(Value::as_i64);
    let limit = args.get("limit").and_then(Value::as_i64);
    if offset.is_none() && limit.is_none() {
        return None;
    }
    let start = offset.unwrap_or(1);
    Some(match limit {
        Some(l) => format!(":{start}-{}", start + l - 1),
        None => format!(":{start}"),
    })
}

/// Port of `keyHint(keybinding, description)` (`keybinding-hints.ts:42-44`):
///
/// ```ts
/// return theme.fg("dim", keyText(keybinding)) + theme.fg("muted", ` ${description}`);
/// ```
///
/// TWO runs, not one — the key label alone is `dim` and the words after it are `muted`, and the
/// separating space belongs to the muted run. `bash.rs`'s X16 hint already renders exactly this
/// shape; X9 is the same primitive extracted so the transcript's hints stop disagreeing with it.
pub(super) fn key_hint_spans(key: &str, description: &str, theme: &UiTheme) -> [Span<'static>; 2] {
    [
        Span::styled(key.to_string(), theme.dim_style()),
        Span::styled(format!(" {description}"), theme.muted_style()),
    ]
}

/// A `... (N more lines[, M total], <key> to expand)` hint (read/write/grep/find/ls collapsed tail).
///
/// X9 — upstream is one interpolation with THREE colour runs
/// (`read.ts:192` = `grep.ts:111` = `find.ts:108` = `ls.ts:85`, and `write.ts:162` with the extra
/// `N total,`):
///
/// ```ts
/// theme.fg("muted", `\n... (${remaining} more lines,`) + " " + keyHint("app.tools.expand", "to expand") + theme.fg("muted", ")")
/// ```
///
/// so the key label is `dim` against `muted` words — and the space between the count and the key is
/// OUTSIDE both `fg()` calls, i.e. unstyled. cyrup painted the whole sentence one flat `muted` and
/// spelled the key as the compile-time literal `ctrl+o`, so a rebound `app.tools.expand` still
/// printed `ctrl+o`; `key` is now the live `keyText` label.
pub(super) fn more_lines_hint(
    remaining: usize,
    total: Option<usize>,
    key: &str,
    theme: &UiTheme,
) -> Line<'static> {
    let lead = match total {
        Some(t) => format!("... ({remaining} more lines, {t} total,"),
        None => format!("... ({remaining} more lines,"),
    };
    let mut spans = vec![Span::styled(lead, theme.muted_style()), Span::raw(" ")];
    spans.extend(key_hint_spans(key, "to expand", theme));
    spans.push(Span::styled(")".to_string(), theme.muted_style()));
    Line::from(spans)
}

/// The file names `getCompactReadClassification` treats as a "resource" read
/// (`core/tools/read.ts:42` `COMPACT_RESOURCE_FILE_NAMES`). Verbatim, including the two `.MD`
/// spellings — the set is matched case-SENSITIVELY upstream (`Set.has(basename(absolutePath))`), so
/// `agents.md` is deliberately not in it.
pub(super) const COMPACT_RESOURCE_FILE_NAMES: [&str; 5] =
    ["AGENTS.override.md", "AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

/// One `CompactReadClassification` (`read.ts:37-40`) — `kind` is `"docs" | "resource" | "skill"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CompactRead {
    kind: &'static str,
    label: String,
}

/// Port of `getCompactReadClassification` (`core/tools/read.ts:122-143`). **X7 = `G30b`** in
/// `docs/gap-analysis/PARITY-GAPS.md` §2.5 — the same unported function; porting it here closes both, and neither
/// backlog should re-land it.
///
/// ```ts
/// const absolutePath = resolveToCwd(rawPath, cwd);
/// const fileName = basename(absolutePath);
/// if (fileName === "SKILL.md") return { kind: "skill", label: basename(dirname(absolutePath)) || fileName };
/// const docsClassification = getPiDocsClassification(absolutePath);
/// if (docsClassification) return docsClassification;
/// if (COMPACT_RESOURCE_FILE_NAMES.has(fileName)) return { kind: "resource", label: formatPathRelativeToCwdOrAbsolute(absolutePath, cwd) };
/// return undefined;
/// ```
///
/// The `docs` arm is the one piece that cannot be ported here, and the missing seam is specific:
/// `getPiDocsClassification` (`:103-120`) resolves the read path against `dirname(getReadmePath())`
/// — the directory of the SHIPPED package's `README.md` (`coding-agent/src/config.ts`) — to label
/// `README.md`/`docs/…`/`examples/…` inside pi's own install. `getReadmePath` has no counterpart
/// anywhere in `crates/` (`grep -rn "readme_path\|getReadmePath" crates --include=*.rs` is empty),
/// and a Rust binary ships no such tree, so there is no path to compare against. `skill` and
/// `resource` are complete; `docs` needs a packaged-docs locator to exist first.
pub(super) fn compact_read_classification(
    args: &Value,
    cwd: Option<&std::path::Path>,
) -> Option<CompactRead> {
    let raw_path = match str_arg(args, &["file_path", "path"]) {
        StrArg::Value(p) => p,
        // `if (!rawPath) return undefined` (`:127`) — covers both the empty and the non-string case,
        // since `str()` yields `""`/`null` and both are falsy.
        _ => return None,
    };
    // `resolveToCwd(rawPath, cwd)` — an absolute path is kept, a relative one is joined to the
    // session cwd. `Path::join` has exactly that semantic for an absolute right-hand side.
    let base = match cwd {
        Some(c) => c.to_path_buf(),
        None => std::env::current_dir().ok()?,
    };
    let absolute = base.join(&raw_path);
    let file_name = absolute.file_name()?.to_string_lossy().into_owned();
    if file_name == "SKILL.md" {
        // `basename(dirname(absolutePath)) || fileName` — the containing directory names the skill,
        // and a `SKILL.md` at the filesystem root falls back to the file name itself.
        let label = absolute
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or(file_name);
        return Some(CompactRead { kind: "skill", label });
    }
    if COMPACT_RESOURCE_FILE_NAMES.contains(&file_name.as_str()) {
        // `formatPathRelativeToCwdOrAbsolute(absolutePath, cwd)`: the cwd-relative form when the file
        // is under it, else the absolute path.
        let label = absolute
            .strip_prefix(&base)
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_else(|_| absolute.to_string_lossy().into_owned());
        return Some(CompactRead { kind: "resource", label });
    }
    None
}

/// Port of `formatCompactReadCall` (`core/tools/read.ts:145-167`).
///
/// ```ts
/// const expandHint = theme.fg("dim", ` (${keyText("app.tools.expand")} to expand)`);
/// if (classification.kind === "skill")
///     return theme.fg("customMessageLabel", `\x1b[1m[skill]\x1b[22m `) +
///            theme.fg("customMessageText", classification.label) + formatReadLineRange(args, theme) + expandHint;
/// return theme.fg("toolTitle", theme.bold(`read ${classification.kind}`)) + " " +
///        theme.fg("accent", classification.label) + formatReadLineRange(args, theme) + expandHint;
/// ```
///
/// Note the expand hint here is **not** `keyHint`: it is one whole `dim` run including the words and
/// the parentheses (`:150`), unlike [`more_lines_hint`]'s dim-key/muted-words split. That asymmetry
/// is upstream's, and copying `keyHint`'s two-tone shape onto it would be the wrong fix.
pub(super) fn compact_read_call(
    c: &CompactRead,
    args: &Value,
    expand_key: &str,
    theme: &UiTheme,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if c.kind == "skill" {
        // The `\x1b[1m…\x1b[22m` pair inside the interpolation is bold-on/bold-off around the
        // bracket label only; `custom_message_label_style` already carries BOLD.
        spans.push(Span::styled("[skill] ".to_string(), theme.custom_message_label_style()));
        spans.push(Span::styled(c.label.clone(), theme.custom_message_text_style()));
    } else {
        spans.push(Span::styled(format!("read {}", c.kind), theme.tool_title_style()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(c.label.clone(), theme.accent_style()));
    }
    if let Some(range) = read_line_range(args) {
        spans.push(Span::styled(range, theme.warning_style()));
    }
    spans.push(Span::styled(format!(" ({expand_key} to expand)"), theme.dim_style()));
    Line::from(spans)
}
