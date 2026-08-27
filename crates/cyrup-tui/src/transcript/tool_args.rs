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
/// (`core/tools/read.ts:43` `COMPACT_RESOURCE_FILE_NAMES`). Verbatim, including the two `.MD`
/// spellings — the set is matched case-SENSITIVELY upstream (`Set.has(basename(absolutePath))`), so
/// `agents.md` is deliberately not in it.
pub(super) const COMPACT_RESOURCE_FILE_NAMES: [&str; 5] =
    ["AGENTS.override.md", "AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

/// The `kind` union of `CompactReadClassification` (`read.ts:38-41`):
/// `kind: "docs" | "resource" | "skill"`. A closed enum rather than a `&'static str` so the
/// renderer cannot silently grow a fourth spelling, and so every `match` on it has to name all
/// three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompactReadKind {
    Docs,
    Resource,
    Skill,
}

impl CompactReadKind {
    /// The word interpolated into ``read ${classification.kind}`` (`read.ts:162`).
    fn as_str(self) -> &'static str {
        match self {
            CompactReadKind::Docs => "docs",
            CompactReadKind::Resource => "resource",
            CompactReadKind::Skill => "skill",
        }
    }
}

/// One `CompactReadClassification` (`read.ts:38-41`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CompactRead {
    kind: CompactReadKind,
    label: String,
}

/// Port of `getPiDocsClassification` (`read.ts:104-121`) — a read of the agent's OWN shipped
/// `README.md`, `docs/…` or `examples/…`.
///
/// `absolute` is already lexically resolved by the caller and [`cyrup_config::asset_dir`] is
/// normalized at construction, so `Path::strip_prefix` — which compares whole components — is the
/// entire `relative()` guard upstream spells out at `:107-112`: it fails for a sibling, for an
/// ancestor and for a different volume, and yields an EMPTY relative path for the root itself,
/// which `:107` rejects too.
fn docs_classification(absolute: &std::path::Path) -> Option<CompactRead> {
    let package_root = cyrup_config::asset_dir()?;
    let relative = absolute.strip_prefix(package_root).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    let label = to_posix_label(relative);
    // `label === "README.md" || label.startsWith("docs/") || label.startsWith("examples/")`
    // (`:117`). The trailing separator is REQUIRED: a read of the `docs` directory itself is not a
    // docs read upstream, and must not become one here.
    if label == "README.md" || label.starts_with("docs/") || label.starts_with("examples/") {
        return Some(CompactRead { kind: CompactReadKind::Docs, label });
    }
    None
}

/// `toPosixPath` (`read.ts:100-102`) — `filePath.split(sep).join("/")`. A no-op on unix; on Windows
/// it is what keeps the label reading `docs/providers.md` rather than `docs\providers.md`.
fn to_posix_label(path: &std::path::Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Port of `getCompactReadClassification` (`core/tools/read.ts:123-144`). **X7 = `G30b`** in
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
/// All three arms are ported: `SKILL.md` first, then [`docs_classification`]
/// (`getPiDocsClassification`, `:104-121`), then `COMPACT_RESOURCE_FILE_NAMES` (`read.ts:43`).
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
    let base = match cwd {
        Some(c) => c.to_path_buf(),
        None => std::env::current_dir().ok()?,
    };
    // `resolveToCwd(rawPath, cwd)` (`read.ts:130`). `Path::join` is NOT that function: it keeps
    // `.`/`..` segments and expands neither `~` nor `@` nor `file://`, so a path pi resolves INTO
    // the asset root (`docs/../docs/x.md`, `~/pkg/README.md`) would miss `strip_prefix` below.
    // `cyrup_tools::path::resolve_to_cwd` IS the port of `resolveToCwd` (`path.rs:248-271`), and
    // `crate::app::event_extract` already reaches for it on the same argument.
    let absolute = cyrup_tools::path::resolve_to_cwd(&raw_path, &base);
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
        return Some(CompactRead { kind: CompactReadKind::Skill, label });
    }
    // `const docsClassification = getPiDocsClassification(absolutePath);` (`:136-137`) — SECOND,
    // ahead of the resource set. A `docs/AGENTS.md` inside the shipped tree is a `docs` read
    // upstream, not a `resource` read, and the order is what decides that.
    if let Some(docs) = docs_classification(&absolute) {
        return Some(docs);
    }
    if COMPACT_RESOURCE_FILE_NAMES.contains(&file_name.as_str()) {
        // `formatPathRelativeToCwdOrAbsolute(absolutePath, cwd)` (`utils/paths.ts:119-122`): the
        // cwd-relative form when the file is under it, else the absolute path — and `.split(sep)
        // .join("/")` on the result, which is the same posix fold the docs label takes.
        let label = absolute
            .strip_prefix(&base)
            .map(to_posix_label)
            .unwrap_or_else(|_| to_posix_label(&absolute));
        return Some(CompactRead { kind: CompactReadKind::Resource, label });
    }
    None
}

/// Port of `formatCompactReadCall` (`core/tools/read.ts:146-168`).
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
    match c.kind {
        // The `\x1b[1m…\x1b[22m` pair inside the interpolation is bold-on/bold-off around the
        // bracket label only; `custom_message_label_style` already carries BOLD.
        CompactReadKind::Skill => {
            spans.push(Span::styled("[skill] ".to_string(), theme.custom_message_label_style()));
            spans.push(Span::styled(c.label.clone(), theme.custom_message_text_style()));
        }
        // `read.ts:161-167` — docs and resource share ONE branch upstream; the kind word is
        // interpolated into the bold title and the label follows in accent.
        CompactReadKind::Docs | CompactReadKind::Resource => {
            spans.push(Span::styled(
                format!("read {}", c.kind.as_str()),
                theme.tool_title_style(),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(c.label.clone(), theme.accent_style()));
        }
    }
    if let Some(range) = read_line_range(args) {
        spans.push(Span::styled(range, theme.warning_style()));
    }
    spans.push(Span::styled(format!(" ({expand_key} to expand)"), theme.dim_style()));
    Line::from(spans)
}
