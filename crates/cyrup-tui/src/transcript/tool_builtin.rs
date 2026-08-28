use super::*;

/// `read`'s `renderCall` — the header `read <path>:<range>` (`read.ts:329-345`).
pub(super) fn render_read_call(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
    out: &mut Vec<Line<'static>>,
) {
    // X7 — `renderCall` picks between two headers (`read.ts:334-343`):
    // `const classification = !context.expanded ? getCompactReadClassification(args, context.cwd) : undefined;`
    // so the compact `[skill] name` / `read resource <label>` form is COLLAPSED-only; expanding a
    // skill read falls back to the plain `read <path>` header plus the body.
    let classification =
        if expanded { None } else { compact_read_classification(&run.args, opts.cwd) };
    match classification {
        Some(c) => out.push(compact_read_call(&c, &run.args, opts.expand_key, theme)),
        None => {
            let mut spans = vec![Span::styled("read ", theme.tool_title_style())];
            spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme, opts));
            if let Some(range) = read_line_range(&run.args) {
                spans.push(Span::styled(range, theme.warning_style()));
            }
            out.push(Line::from(spans));
        }
    }
}

/// `read`'s `renderResult` — the file body, shown only when expanded or on error
/// (`formatReadResult`, `read.ts:173-201`).
pub(super) fn render_read_result(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
    out: &mut Vec<Line<'static>>,
) {
    // `formatReadResult`: nothing below the header when collapsed & not an error (read.ts:173-175).
    let Some(result) = &run.result else { return };
    if !expanded && !run.is_error {
        return;
    }
    let output = result_text(result);
    // `const lang = !isError && rawPath ? getLanguageFromPath(rawPath) : undefined` (`read.ts:184`).
    let raw_path = match str_arg(&run.args, &["file_path", "path"]) {
        StrArg::Value(p) => p,
        _ => String::new(),
    };
    let lang = if run.is_error || raw_path.is_empty() {
        None
    } else {
        crate::theme::language_from_path(&raw_path)
    };
    // `highlightCode(replaceTabs(output), lang)` — the tabs are replaced BEFORE the highlighter runs
    // on this side of the ternary (`read.ts:185`), so a leading tab is three highlighted spaces.
    let highlighted =
        lang.and_then(|l| crate::markdown::highlight_code_lines(&replace_tabs(&output), l, theme));
    let all = trim_trailing_empty(output.split('\n').collect());
    let total = all.len();
    let shown = if expanded { total } else { total.min(10) };
    out.push(Line::default());
    for (i, l) in all.iter().take(shown).enumerate() {
        out.push(body_line(l, highlighted.as_ref(), i, theme));
    }
    let remaining = total.saturating_sub(shown);
    if remaining > 0 {
        out.push(more_lines_hint(remaining, None, opts.expand_key, theme));
    }
    push_read_truncation(result, theme, out);
}

/// `write`'s `renderCall` — the header `write <path>` + the content preview, which is built from the
/// call ARGUMENTS and so belongs to the call side upstream too (`formatWriteCall`, `write.ts:131-163`).
pub(super) fn render_write_call(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
    out: &mut Vec<Line<'static>>,
) {
    let mut spans = vec![Span::styled("write ", theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme, opts));
    out.push(Line::from(spans));
    match str_arg(&run.args, &["content"]) {
        StrArg::Invalid => {
            out.push(Line::default());
            out.push(Line::styled(
                "[invalid content arg - expected string]".to_string(),
                theme.error_style(),
            ));
        }
        StrArg::Missing => {}
        StrArg::Value(content) => {
            let display = content.replace('\r', "");
            // X6 — `const lang = rawPath ? getLanguageFromPath(rawPath) : undefined` (`write.ts:151`).
            // Unlike `read` there is no `isError` leg: the preview comes from the ARGUMENTS, so it is
            // highlighted whether or not the write went on to fail.
            let raw_path = match str_arg(&run.args, &["file_path", "path"]) {
                StrArg::Value(p) => p,
                _ => String::new(),
            };
            let lang = if raw_path.is_empty() {
                None
            } else {
                crate::theme::language_from_path(&raw_path)
            };
            let highlighted = lang.and_then(|l| {
                crate::markdown::highlight_code_lines(&replace_tabs(&display), l, theme)
            });
            let all = trim_trailing_empty(display.split('\n').collect());
            let total = all.len();
            let shown = if expanded { total } else { total.min(10) };
            out.push(Line::default());
            for (i, l) in all.iter().take(shown).enumerate() {
                out.push(body_line(l, highlighted.as_ref(), i, theme));
            }
            let remaining = total.saturating_sub(shown);
            if remaining > 0 {
                out.push(more_lines_hint(remaining, Some(total), opts.expand_key, theme));
            }
        }
    }
}

/// `write`'s `renderResult` — output only on error (`formatWriteResult`, `write.ts:164-179`).
pub(super) fn render_write_result(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    if run.is_error && let Some(result) = &run.result {
        push_error_body(result, theme, out);
    }
}

/// `edit` — header `edit <path>` + the diff (`edit.ts:200-227/244-262/363-431`, rendered via
/// [`crate::diff::render_diff`], the port of `diff.ts`).
///
/// Two sources feed that diff, in Pi's order:
///
/// 1. the **pre-execution preview** ([`ToolRun::preview`], Pi `buildEditCallComponent`
///    edit.ts:244-262): a `Spacer(1)` then the diff `computeEditsDiff` produced from the arguments
///    alone, or the failure message in the error colour. This is on screen while the call is still
///    PENDING — including for the whole time a permission prompt is up — and before anything is
///    written.
/// 2. the settled result's `details.diff`, which **replaces** the preview rather than being appended
///    below it. That is Pi's own ordering, and it is easy to misread: `renderResult` calls
///    `setEditPreview(callComponent, { diff: result.details.diff, … })` (edit.ts:196-204) BEFORE
///    handing `callComponent.preview` to `formatEditResult`, so by the time `formatEditResult` tests
///    `resultDiff !== previewDiff` (`:220-223`) the two are the same object and the result body
///    renders nothing. The diff is therefore drawn exactly once, by the call component, and it is
///    the authoritative post-write one.
///
/// The same de-duplication applies to failures: an error result whose text merely restates the
/// preview error is dropped (`:212-218`), while a preview that succeeded stays on screen next to an
/// error the tool itself hit.
/// X8 — which of Pi's three `getEditHeaderBg` preview states this run is in
/// (`core/tools/edit.ts:239-253`).
///
/// `EditCallRenderComponent.preview` is a single slot that BOTH the pre-execution `computeEditsDiff`
/// (`renderCall`, `:385`) and the settled result (`renderResult`'s `setEditPreview` from
/// `details.diff`, `:400-411`) write, the result overwriting the preview. So the result diff is
/// tested first here, exactly as `renderResult` runs before `buildEditCallComponent` rebuilds the
/// component. The two are read with the same accessors [`render_edit`] uses, so the tint can never
/// disagree with the body drawn inside it.
pub(super) fn edit_header_preview(run: &ToolRun) -> crate::theme::EditHeaderPreview {
    use crate::theme::EditHeaderPreview as P;
    let result_diff = run
        .result
        .as_ref()
        .filter(|_| !run.is_error)
        .and_then(|r| r.get("details"))
        .and_then(|d| d.get("diff"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    if result_diff.is_some() {
        return P::Computed;
    }
    match &run.preview {
        Some(Ok(d)) if !d.is_empty() => P::Computed,
        Some(Err(e)) if !e.trim().is_empty() => P::Failed,
        _ => P::Absent,
    }
}

/// `edit`'s `renderCall` — the header AND the diff. The diff really is the call component's
/// upstream: `buildEditCallComponent` draws `callComponent.preview` (`edit.ts:244-262`), which
/// `renderResult` has already overwritten with `details.diff` by the time it runs (`:196-204`), so
/// `formatEditResult` finds the two equal and emits nothing (`:220-223`). See the module note above.
pub(super) fn render_edit_call(
    run: &ToolRun,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
    out: &mut Vec<Line<'static>>,
) {
    let mut spans = vec![Span::styled("edit ", theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme, opts));
    out.push(Line::from(spans));

    let preview_diff = match &run.preview {
        Some(Ok(d)) if !d.is_empty() => Some(d.as_str()),
        _ => None,
    };
    let preview_error = match &run.preview {
        Some(Err(e)) if !e.trim().is_empty() => Some(e.as_str()),
        _ => None,
    };
    // The settled diff supersedes the preview (`setEditPreview` from `renderResult`, edit.ts:196-204).
    let result_diff = run
        .result
        .as_ref()
        .filter(|_| !run.is_error)
        .and_then(|r| r.get("details"))
        .and_then(|d| d.get("diff"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());

    if let Some(diff) = result_diff.or(preview_diff) {
        out.push(Line::default());
        out.extend(crate::diff::render_diff(diff, theme));
    } else if let Some(err) = preview_error {
        out.push(Line::default());
        for l in err.split('\n') {
            out.push(Line::styled(l.to_string(), theme.error_style()));
        }
    }
}

/// `edit`'s `renderResult` — only the error text the preview did not already say
/// (`formatEditResult`, `edit.ts:212-218`). The preview error is recomputed here rather than
/// threaded from [`render_edit_call`] so each side reads the same slot independently, exactly as
/// upstream's two components do.
pub(super) fn render_edit_result(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    if run.is_error
        && let Some(result) = &run.result
    {
        let preview_error = match &run.preview {
            Some(Err(e)) if !e.trim().is_empty() => Some(e.as_str()),
            _ => None,
        };
        // `if (!errorText || errorText === previewError) return undefined` (edit.ts:215-217).
        if preview_error.is_some_and(|e| result_text(result).trim() == e.trim()) {
            return;
        }
        push_error_body(result, theme, out);
    }
}

/// `bash`/`powershell`'s `renderCall` — the header `<prompt> <command> (timeout Ns)`
/// (`formatShellCall`, `bash.ts:238-244`).
pub(super) fn render_bash_call(
    run: &ToolRun,
    theme: &UiTheme,
    prompt: &str,
    out: &mut Vec<Line<'static>>,
) {
    // Header: `<prompt> command`, bold, + a muted ` (timeout Ns)` suffix (`formatShellCall`,
    // bash.ts:238-244, called with `config.prompt` at bash.ts:488 — `$` for bash, `PS>` for
    // PowerShell).
    let title = theme.tool_title_style();
    let mut spans = Vec::new();
    match str_arg(&run.args, &["command"]) {
        StrArg::Invalid => {
            spans.push(Span::styled(format!("{prompt} "), title));
            spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style()));
        }
        StrArg::Missing => {
            spans.push(Span::styled(format!("{prompt} "), title));
            spans.push(Span::styled("...".to_string(), theme.tool_output_style()));
        }
        StrArg::Value(cmd) => spans.push(Span::styled(format!("{prompt} {cmd}"), title)),
    }
    if let Some(t) = run.args.get("timeout").and_then(Value::as_f64).filter(|t| *t != 0.0) {
        // `${timeout}s` (bash.ts:241) — the same `String(n)` fold the read range uses; the `±0`
        // case `js_number` handles is already excluded by the filter above.
        spans.push(Span::styled(format!(" (timeout {}s)", js_number(t)), theme.muted_style()));
    }
    out.push(Line::from(spans));
}

/// `bash`/`powershell`'s `renderResult` — the output tail (collapsed = last 5 visual lines) +
/// truncation notices + the `Took`/`Elapsed {d}s` footer (`bash.ts:249-317/430-479`).
pub(super) fn render_bash_result(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    if let Some(result) = &run.result {
        let raw = result_text(result);
        let output = strip_bash_footer(raw.trim(), result, run.done);
        if !output.is_empty() {
            out.push(Line::default());
            let all: Vec<&str> = output.split('\n').collect();
            let total = all.len();
            if expanded {
                for l in &all {
                    out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
                }
            } else {
                let shown = total.min(5);
                let skipped = total - shown;
                if skipped > 0 {
                    // X9 — same three-run shape as [`more_lines_hint`], with `bash.ts:281-284`'s own
                    // wording:
                    // `fg("muted", `... (${skipped} earlier lines,`) + ` ${keyHint("app.tools.expand", "to expand")}` + fg("muted", ")")`.
                    let mut spans = vec![
                        Span::styled(
                            format!("... ({skipped} earlier lines,"),
                            theme.muted_style(),
                        ),
                        Span::raw(" "),
                    ];
                    spans.extend(key_hint_spans(expand_key, "to expand", theme));
                    spans.push(Span::styled(")".to_string(), theme.muted_style()));
                    out.push(Line::from(spans));
                }
                for l in all.iter().skip(skipped) {
                    out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
                }
            }
        }
        push_bash_warnings(result, theme, out);
        // The duration footer (bash.ts:309-313). Upstream is literally
        // `const label = options.isPartial ? "Elapsed" : "Took"` with
        // `formatDuration((endedAt ?? Date.now()) - startedAt)`, so a RUNNING command shows a live
        // `Elapsed 12.3s` that only becomes `Took 12.4s` when the call settles — the tool's
        // `renderResult` arms a 1 s `setInterval(() => context.invalidate())` (`:471-473`) precisely
        // to make it tick. It is gated on `startedAt`, which `renderCall` stamps the moment
        // execution begins (`:460-463`), NOT on the result being final; `run.result` is already
        // `Some` from the first frame because bash emits an initial empty update before it spawns
        // (bash.ts:384-385, ported at `cyrup-tools/src/tools/bash.rs:170`), which is what makes
        // upstream's `if (this.result)` renderResult gate (tool-execution.ts:281) pass too.
        //
        // Before this, cyrup keyed the line on `duration_ms`, which is written only on settle
        // (`push_tool_end_rendered`), so a long-running command rendered NO duration at all — the
        // one number that tells a user a 10-minute build is still alive.
        if let Some(started) = run.started_at {
            let (label, ms) = match run.duration_ms {
                Some(ms) => ("Took", ms),
                None => ("Elapsed", started.elapsed().as_millis() as u64),
            };
            // X10 — `bash.ts:317` is `new Text(`\n${theme.fg("muted", …)}`, 0, 0)`: the same
            // leading-`\n` blank row as the warnings block above.
            out.push(Line::default());
            out.push(Line::styled(
                format!("{label} {}", format_duration(ms)),
                theme.muted_style(),
            ));
        }
    }
}

/// `grep`'s `renderCall` — the header `grep /<pattern>/ in <path> (glob) limit N`
/// (`formatGrepCall`, `grep.ts:68-121`).
pub(super) fn render_grep_call(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let title = theme.tool_title_style();
    let outp = theme.tool_output_style();
    let mut spans = vec![Span::styled("grep ".to_string(), title)];
    match str_arg(&run.args, &["pattern"]) {
        StrArg::Invalid => spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style())),
        StrArg::Missing => spans.push(Span::styled("//".to_string(), theme.accent_style())),
        StrArg::Value(p) => spans.push(Span::styled(format!("/{p}/"), theme.accent_style())),
    }
    spans.push(Span::styled(" in ".to_string(), outp));
    push_search_path(&run.args, theme, &mut spans);
    if let StrArg::Value(glob) = str_arg(&run.args, &["glob"]) {
        spans.push(Span::styled(format!(" ({glob})"), outp));
    }
    // `if (limit !== undefined) text += theme.fg("toolOutput", ` limit ${limit}`)` (grep.ts:81/89,
    // `formatGrepCall`). A PRESENCE test, not the truthiness test `formatShellCall` applies to
    // `timeout` — so `limit: 0` renders. And `JSON.parse` yields the same double for `50` and
    // `50.0`, so [`Value::as_f64`] — `Some` for `Number::PosInt`, `NegInt` and `Float` alike — is
    // the extractor, not `as_i64`, which answers `None` for every float and dropped the whole
    // suffix. `js_number` is the `String(n)` fold the template literal applies.
    if let Some(limit) = run.args.get("limit").and_then(Value::as_f64) {
        spans.push(Span::styled(format!(" limit {}", js_number(limit)), outp));
    }
    out.push(Line::from(spans));
}

/// `grep`'s `renderResult` — matching lines (head-15) + a `[Truncated: …]` notice
/// (`grep.ts:370-379`).
pub(super) fn render_grep_result(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    push_list_output(run, expanded, 15, theme, expand_key, out);
    push_grep_warnings(run.result.as_ref(), theme, out);
}

/// `find`'s `renderCall` — the header `find <pattern> in <path> (limit N)` (`formatFindCall`,
/// `find.ts:59-107`).
pub(super) fn render_find_call(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let title = theme.tool_title_style();
    let outp = theme.tool_output_style();
    let mut spans = vec![Span::styled("find ".to_string(), title)];
    match str_arg(&run.args, &["pattern"]) {
        StrArg::Invalid => spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style())),
        StrArg::Missing => {}
        StrArg::Value(p) => spans.push(Span::styled(p, theme.accent_style())),
    }
    spans.push(Span::styled(" in ".to_string(), outp));
    push_search_path(&run.args, theme, &mut spans);
    // `if (limit !== undefined) { text += theme.fg("toolOutput", ` (limit ${limit})`); }`
    // (find.ts:77/84-86, `formatFindCall`) — the same presence test and the same `String(n)` fold
    // as `render_grep`; only the parentheses differ.
    if let Some(limit) = run.args.get("limit").and_then(Value::as_f64) {
        spans.push(Span::styled(format!(" (limit {})", js_number(limit)), outp));
    }
    out.push(Line::from(spans));
}

/// `find`'s `renderResult` — matching paths (head-20) + a `[Truncated: …]` notice
/// (`find.ts:359-368`).
pub(super) fn render_find_result(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    push_list_output(run, expanded, 20, theme, expand_key, out);
    push_find_warnings(run.result.as_ref(), theme, out);
}

/// `ls`'s `renderCall` — the header `ls <path> (limit N)` (`formatLsCall`, `ls.ts:52-93`).
pub(super) fn render_ls_call(
    run: &ToolRun,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
    out: &mut Vec<Line<'static>>,
) {
    let mut spans = vec![Span::styled("ls ".to_string(), theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["path"], Some("."), theme, opts));
    // `if (limit !== undefined) { text += theme.fg("toolOutput", ` (limit ${limit})`); }`
    // (ls.ts:58/61-63, `formatLsCall`).
    if let Some(limit) = run.args.get("limit").and_then(Value::as_f64) {
        spans.push(Span::styled(format!(" (limit {})", js_number(limit)), theme.tool_output_style()));
    }
    out.push(Line::from(spans));
}

/// `ls`'s `renderResult` — entries (head-20) + a `[Truncated: …]` notice (`ls.ts:210-219`).
pub(super) fn render_ls_result(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    push_list_output(run, expanded, 20, theme, expand_key, out);
    push_ls_warnings(run.result.as_ref(), theme, out);
}

/// Which built-in owns a tool name — cyrup's `builtInToolDefinition` lookup, i.e. Pi's
/// `createAllToolDefinitions()[toolName]` (`tool-execution.ts`, the field the two
/// `get*Renderer()` merges and `hasRendererDefinition()` all read).
///
/// Asked ONCE per block by [`tool_lines`](super::tool_render::tool_lines) so the call side and the
/// result side consult the same answer, which is what lets an extension override one side of a
/// BUILT-IN tool and still get the built-in's other side — upstream's
/// `this.toolDefinition.renderCall ?? this.builtInToolDefinition.renderCall`, resolved
/// independently per renderer (`tool-execution.ts:84-101`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Builtin {
    Read,
    Write,
    Edit,
    /// `bash` and `powershell` share one pair of renderers, differing only in the shell prompt the
    /// header is drawn with (`config.prompt`, `bash.ts:488`).
    Shell(&'static str),
    Grep,
    Find,
    Ls,
}

/// The built-in definition table (see [`Builtin`]). `None` = the name is not a built-in, so
/// whether anything at all is known about it is [`ToolRun::has_definition`]'s question.
pub(super) fn builtin_kind(name: &str) -> Option<Builtin> {
    match name {
        "read" => Some(Builtin::Read),
        "write" => Some(Builtin::Write),
        "edit" => Some(Builtin::Edit),
        "bash" => Some(Builtin::Shell("$")),
        "powershell" => Some(Builtin::Shell("PS>")),
        "grep" => Some(Builtin::Grep),
        "find" => Some(Builtin::Find),
        "ls" => Some(Builtin::Ls),
        _ => None,
    }
}

/// The built-in's `renderCall` half (Pi `getCallRenderer()`, `tool-execution.ts:84-92`).
pub(super) fn render_builtin_call(
    kind: Builtin,
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
    out: &mut Vec<Line<'static>>,
) {
    match kind {
        Builtin::Read => render_read_call(run, expanded, theme, opts, out),
        Builtin::Write => render_write_call(run, expanded, theme, opts, out),
        Builtin::Edit => render_edit_call(run, theme, opts, out),
        Builtin::Shell(prompt) => render_bash_call(run, theme, prompt, out),
        Builtin::Grep => render_grep_call(run, theme, out),
        Builtin::Find => render_find_call(run, theme, out),
        Builtin::Ls => render_ls_call(run, theme, opts, out),
    }
}

/// The built-in's `renderResult` half (Pi `getResultRenderer()`, `tool-execution.ts:94-101`).
pub(super) fn render_builtin_result(
    kind: Builtin,
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
    out: &mut Vec<Line<'static>>,
) {
    match kind {
        Builtin::Read => render_read_result(run, expanded, theme, opts, out),
        Builtin::Write => render_write_result(run, theme, out),
        Builtin::Edit => render_edit_result(run, theme, out),
        Builtin::Shell(_) => render_bash_result(run, expanded, theme, opts.expand_key, out),
        Builtin::Grep => render_grep_result(run, expanded, theme, opts.expand_key, out),
        Builtin::Find => render_find_result(run, expanded, theme, opts.expand_key, out),
        Builtin::Ls => render_ls_result(run, expanded, theme, opts.expand_key, out),
    }
}

/// The CALL text an extension's registered renderer produced, as the block's header (EXT-006; Pi
/// `ToolDefinition.renderCall`, preferred over the built-in's at `tool-execution.ts:84-92`).
pub(super) fn render_extension_call(call: &str, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    for l in call.split('\n') {
        out.push(Line::styled(
            normalize_terminal_output(l).into_owned(),
            theme.tool_title_style(),
        ));
    }
}

/// The RESULT text an extension's registered renderer produced, as the block's body (Pi
/// `ToolDefinition.renderResult`, `tool-execution.ts:94-101`).
///
/// The `run.done || expanded` gate is cyrup's, not Pi's — upstream runs `renderResult` whenever
/// `this.result` is set (`:296`) and lets the renderer decide. It is kept because it is vacuous
/// here rather than out of divergence: [`TranscriptView::push_tool_end_rendered`] is the only
/// writer of `rendered_result` and it sets `done` in the same breath, so the gate can never be the
/// reason a body is missing.
pub(super) fn render_extension_result(
    result: &str,
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    out: &mut Vec<Line<'static>>,
) {
    if (run.done || expanded) && !result.trim().is_empty() {
        for l in result.split('\n') {
            out.push(Line::styled(
                normalize_terminal_output(l).into_owned(),
                theme.tool_output_style(),
            ));
        }
    }
}

/// How many output lines the result fallback shows collapsed — `FALLBACK_PREVIEW_LINES`
/// (`tool-execution.ts:9`).
const FALLBACK_PREVIEW_LINES: usize = 10;

/// `createCallFallback()` (`tool-execution.ts:137-139`) — a tool that HAS a definition but no
/// `renderCall` shows `new Text(theme.fg("toolTitle", theme.bold(this.toolName)))` and nothing
/// else. No arguments: that is the whole difference from [`render_generic`], and it is why an
/// MCP-proxied tool no longer commits its entire argument JSON to scrollback.
///
/// (`tool_title_style` already carries `BOLD`, so it is both halves of upstream's
/// `fg("toolTitle", bold(...))`.)
pub(super) fn render_call_fallback(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    out.push(Line::styled(run.name.clone(), theme.tool_title_style()));
}

/// `createResultFallback()` (`tool-execution.ts:141-155`) — a tool that HAS a definition but no
/// `renderResult` shows its text output capped at [`FALLBACK_PREVIEW_LINES`] when collapsed, with
/// the shared `... (N more lines, <key> to expand)` line beneath it, and everything when expanded
/// (`:149`).
///
/// Empty output emits NOTHING, not even a blank row — upstream returns `undefined` and no child is
/// added (`:143-145`).
pub(super) fn render_result_fallback(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    let Some(result) = &run.result else { return };
    let output = result_text(result);
    // `if (!output) return undefined` (`:143-145`) — the empty string is JS-falsy.
    if output.is_empty() {
        return;
    }
    // `output.split("\n")`, then `this.expanded ? lines : lines.slice(0, FALLBACK_PREVIEW_LINES)`.
    // Taken with an iterator adapter rather than a slice, which `clippy::indexing_slicing` denies.
    let total = output.split('\n').count();
    let shown = if expanded { total } else { total.min(FALLBACK_PREVIEW_LINES) };
    for l in output.split('\n').take(shown) {
        out.push(Line::styled(
            normalize_terminal_output(l).into_owned(),
            theme.tool_output_style(),
        ));
    }
    let remaining = total.saturating_sub(shown);
    if remaining > 0 {
        out.push(more_lines_hint(remaining, None, expand_key, theme));
    }
}

/// A tool with NO definition at all falls back to Pi's `formatToolExecution`
/// (`tool-execution.ts:330-333` selects it, `:376-387` builds it): the bold tool name +
/// pretty-printed args + any text output, all unbounded.
///
/// This is the `else` of `hasRendererDefinition()` and NOT the shape a defined-but-unrendered tool
/// takes — see [`render_call_fallback`] / [`render_result_fallback`] for that one.
pub(super) fn render_generic(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    out.push(Line::styled(run.name.clone(), theme.tool_title_style()));
    if !run.args.is_null()
        && let Ok(pretty) = serde_json::to_string_pretty(&run.args)
    {
        out.push(Line::default());
        for l in pretty.split('\n') {
            out.push(Line::styled(
                normalize_terminal_output(l).into_owned(),
                theme.tool_output_style(),
            ));
        }
    }
    if let Some(result) = &run.result {
        let output = result_text(result);
        if !output.trim().is_empty() {
            for l in output.split('\n') {
                out.push(Line::styled(
                    normalize_terminal_output(l).into_owned(),
                    theme.tool_output_style(),
                ));
            }
        }
    }
}
