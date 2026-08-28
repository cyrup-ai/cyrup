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
/// (else `...`) for an empty/absent path, otherwise the `~`-shortened path in accent — wrapped in an
/// OSC-8 hyperlink to the resolved path's `file://` URL when the terminal forwards them
/// (`linkPath`, `:19-23`).
///
/// The gate and the two unlinked arms are upstream's, exactly: `linkPath` is reached only from
/// `:84`'s `accent` branch, so `invalidArgText` (`:71-73`) and the `toolOutput` `...` (`:83`) stay
/// inert; and the href is built from `value` — the RAW path — while the visible text is the
/// `~`-shortened form, because `shortenPath` is display-only.
///
/// The escape itself is not in this `Span`. [`crate::osc`] explains why (`Span::styled_graphemes`
/// deletes `ESC`, and `Span::width` would miscount the rest as columns); the span carries a marker
/// in `Modifier`'s unallocated bits and [`crate::osc::inject`] converts marked cells into the
/// escape once the `Buffer` exists.
pub(super) fn tool_path_span(
    args: &Value,
    keys: &[&str],
    empty_fallback: Option<&str>,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
) -> Span<'static> {
    match str_arg(args, keys) {
        StrArg::Invalid => Span::styled("[invalid arg]".to_string(), theme.error_style()),
        StrArg::Missing => match empty_fallback {
            Some(f) => Span::styled(shorten_path(f), link_style(f, theme, opts)),
            None => Span::styled("...".to_string(), theme.tool_output_style()),
        },
        StrArg::Value(p) => Span::styled(shorten_path(&p), link_style(&p, theme, opts)),
    }
}

/// `theme.fg("accent", …)`, plus [`crate::osc`]'s link marker when the terminal forwards OSC-8 —
/// `linkPath(styledText, rawPath, cwd)` (`render-utils.ts:19-23`) with pi's own early return:
///
/// ```ts
/// if (!getCapabilities().hyperlinks) return styledText;
/// const absolutePath = resolvePath(rawPath, cwd);
/// return hyperlink(styledText, pathToFileURL(absolutePath).href);
/// ```
///
/// `resolvePath(rawPath, cwd)` is `cyrup_tools::path::resolve_to_cwd`, the same port `read`'s
/// compact classification resolves through (`read.ts:336`). A `cwd` of `None` falls back to the
/// process cwd, matching [`compact_read_classification`]; if even that is unavailable the path
/// cannot be resolved and the span stays unlinked rather than pointing somewhere wrong.
fn link_style(raw_path: &str, theme: &UiTheme, opts: ImageOpts<'_>) -> Style {
    let accent = theme.accent_style();
    if !opts.hyperlinks {
        return accent;
    }
    let Some(sink) = opts.links else { return accent };
    let base = match opts.cwd {
        Some(c) => c.to_path_buf(),
        None => match std::env::current_dir() {
            Ok(c) => c,
            Err(_) => return accent,
        },
    };
    let absolute = cyrup_tools::path::resolve_to_cwd(raw_path, &base);
    let url = cyrup_tools::path::path_to_file_url(&absolute);
    accent.patch(sink.mark(url))
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

/// JS `String(n)` for a `Number` that came out of `JSON.parse` — the fold a template literal
/// applies when a double is interpolated (`` `:${startLine}` ``, read.ts:77; `` ` limit ${limit}` ``,
/// grep.ts:89; `` ` (limit ${limit})` ``, find.ts:85 / ls.ts:62; `` `${timeout}s` ``, bash.ts:241).
///
/// This is a full port of ECMA-262 `Number::toString(x, 10)`, NOT `format!("{n}")`. Rust's `Display`
/// agrees with JS on the *digits* — both emit the shortest round-tripping form, so `2.0` prints `2`
/// where `Debug` would print `2.0` — but disagrees with it twice:
///
/// 1. **Notation.** ECMA-262 switches to exponential form when the decimal point position `n` is
///    `> 21` or `<= -6`; Rust's `Display` is never exponential. `1e21` is `1e+21` in JS and
///    `1000000000000000000000` under `Display`; `1e-7` is `1e-7` in JS and `0.0000001` under
///    `Display`. JS also always signs the exponent (`1e+21`) where Rust's `{:e}` writes `1e21`.
/// 2. **Tie-breaking.** When two equally short decimals are exactly equidistant from `x`, ECMA-262
///    picks the even last digit; Rust's shortest-form `Display` does not. `-1149636667324797.25`
///    prints `-1149636667324797.2` in JS and `-1149636667324797.3` under `Display`. Formatting with
///    an *explicit* precision (`{:.*e}`) is correctly rounded ties-to-even, so re-rounding the
///    shortest digit count through that path recovers ECMA's choice.
///
/// Negative zero is the third: `String(-0) === "0"` where `Display` writes `-0`.
///
/// Cross-checked against V8's `String(n)` over 587,729 doubles (500k xorshift bit patterns biased
/// across the exponent range, plus every small integer, decade boundary and subnormal edge): zero
/// divergences.
///
/// This is deliberately NOT `cyrup_tools::jsnum::to_integer`: that is `ToIntegerOrInfinity`, the
/// coercion the READ path applies to pick a line window (read.ts:278-288). The HEADER interpolates
/// the number as given, and a fractional `offset` reaches the screen unrounded upstream.
pub(super) fn js_number(n: f64) -> String {
    // `String(-0) === "0"`; Rust's `Display` would print `-0`. Covers `+0.0` too.
    if n == 0.0 {
        return "0".to_string();
    }
    // JSON carries neither, but `String(n)` is total and these cost one line each.
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n < 0.0 { "-Infinity".to_string() } else { "Infinity".to_string() };
    }
    // Step 1 — the shortest round-tripping digit count `k`. Rust's `LowerExp` produces exactly that
    // many digits; only its tie-breaking differs, so re-rounding to the same `k` through the
    // explicit-precision path (correctly rounded, ties-to-even, as ECMA-262 specifies) fixes the
    // digits while keeping the length minimal. A `k`-digit decimal that round-trips exists by
    // construction, and the nearest one is at least as good, so the guard below only ever fires if
    // `{:.*e}` were not correctly rounded — in which case the shortest form is still a valid answer.
    let shortest = format!("{n:e}");
    let Some((shortest_mantissa, _)) = shortest.split_once('e') else {
        // `LowerExp` for `f64` always emits an `e`; unreachable defensiveness.
        return format!("{n}");
    };
    let digit_count = shortest_mantissa.chars().filter(char::is_ascii_digit).count();
    let rounded = format!("{:.*e}", digit_count.saturating_sub(1), n);
    let repr = if rounded.parse::<f64>().is_ok_and(|v| v == n) { rounded } else { shortest };
    // Step 2 — split into ECMA-262's `(s, k, n)`: `s` is the digit string, `k` its length, and `n`
    // the position of the decimal point, which is one more than `{:e}`'s exponent (that exponent is
    // the power of ten sitting on a single leading digit).
    let Some((mantissa, exponent)) = repr.split_once('e') else { return format!("{n}") };
    let Ok(exp) = exponent.parse::<i32>() else { return format!("{n}") };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let Ok(k) = i32::try_from(digits.chars().count()) else { return format!("{n}") };
    let point = exp + 1;
    let sign = if mantissa.starts_with('-') { "-" } else { "" };
    // Step 3 — ECMA-262 `Number::toString` steps 6-10, in order.
    if k <= point && point <= 21 {
        // The digits, then `n - k` trailing zeros.
        format!("{sign}{digits}{}", "0".repeat(usize::try_from(point - k).unwrap_or(0)))
    } else if 0 < point && point <= 21 {
        // A decimal point after `n` digits.
        let mut body = String::with_capacity(digits.len() + 1);
        for (i, c) in digits.chars().enumerate() {
            if i32::try_from(i).is_ok_and(|i| i == point) {
                body.push('.');
            }
            body.push(c);
        }
        format!("{sign}{body}")
    } else if -6 < point && point <= 0 {
        // `0.`, then `-n` leading zeros, then the digits.
        format!("{sign}0.{}{digits}", "0".repeat(usize::try_from(-point).unwrap_or(0)))
    } else {
        // Exponential, with the SIGNED exponent `n - 1` that JS always writes.
        let mut rest = digits.chars();
        let lead = rest.next().unwrap_or('0');
        let tail: String = rest.collect();
        let esign = if point - 1 < 0 { '-' } else { '+' };
        let emag = (point - 1).abs();
        if tail.is_empty() {
            format!("{sign}{lead}e{esign}{emag}")
        } else {
            format!("{sign}{lead}.{tail}e{esign}{emag}")
        }
    }
}

/// `formatReadLineRange` (read.ts:73-78): `:<start>` or `:<start>-<end>` from `offset`/`limit`.
///
/// ```ts
/// if (args?.offset === undefined && args?.limit === undefined) return "";
/// const startLine = args.offset ?? 1;
/// const endLine = args.limit !== undefined ? startLine + args.limit - 1 : "";
/// return theme.fg("warning", `:${startLine}${endLine ? `-${endLine}` : ""}`);
/// ```
///
/// Upstream has no integer type to lose: `JSON.parse` yields an IEEE-754 double for `2` and for
/// `2.0` alike, so both spellings are literally the same value by the time this runs and both
/// render `:2`. [`Value::as_f64`] is that same "is this a JSON number" test — it answers `Some` for
/// `Number::PosInt`, `NegInt` and `Float` alike, where `as_i64` answers `None` for every float — so
/// it, and not `as_i64`, is the extractor. It is also the more faithful one at the top of the range:
/// `as_f64` narrows `9007199254740993` to `9007199254740992`, which is precisely what `JSON.parse`
/// does with the same literal.
///
/// The arithmetic stays in `f64` because `startLine + args.limit - 1` is double arithmetic
/// upstream; a fractional `offset` reaches the header unrounded there and must here.
pub(super) fn read_line_range(args: &Value) -> Option<String> {
    let offset = args.get("offset").and_then(Value::as_f64);
    let limit = args.get("limit").and_then(Value::as_f64);
    if offset.is_none() && limit.is_none() {
        return None;
    }
    let start = offset.unwrap_or(1.0);
    // `endLine ? …` is a JS TRUTHINESS test on a Number, not a presence test: an end line that
    // computes to zero (`{"offset":1,"limit":0}`) is falsy upstream and the `-<end>` half is
    // dropped. `NaN` is falsy for the same reason and is excluded here for the same reason.
    let end = limit.map(|l| start + l - 1.0).filter(|e| *e != 0.0 && !e.is_nan());
    Some(match end {
        Some(e) => format!(":{}-{}", js_number(start), js_number(e)),
        None => format!(":{}", js_number(start)),
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

/// `toPosixPath` (`read.ts:100-102`) — `filePath.split(sep).join("/")`, which is a plain
/// replace-every-`sep`-with-`/` over the STRING form. A no-op on unix; on Windows it is what keeps
/// the label reading `docs/providers.md` rather than `docs\providers.md`.
///
/// Deliberately NOT a `components()` walk joined on `"/"`: `Component::RootDir::as_os_str()` is
/// already `MAIN_SEP_STR`, so the join emits it a second time and every absolute label comes out
/// `//etc/…`; on Windows the `Prefix` + `RootDir` pair comes out `C:/\/a/b`. Upstream never
/// decomposes the path, and neither must this — a UNC path's `//server/share/a` is upstream's
/// output too, so a `//`-collapsing patch would be wrong in the other direction.
fn to_posix_label(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        s.into_owned()
    } else {
        s.replace(std::path::MAIN_SEPARATOR, "/")
    }
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
        // cwd-relative form when the file is under it, else the absolute path, `.split(sep)
        // .join("/")` either way. The port lives in `cyrup_tools::path` rather than inline here
        // because a bare `strip_prefix` loses Pi's `relativePath || "."` (`:116`) — a path that IS
        // the cwd rendered as the EMPTY label, not `.`.
        let label = cyrup_tools::path::format_path_relative_to_cwd_or_absolute(&absolute, &base);
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
