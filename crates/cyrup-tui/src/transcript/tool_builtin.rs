use super::*;

/// `read` — header `read <path>:<range>` + (only when expanded/error) the file body (`read.ts:74-201`).
pub(super) fn render_read(
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

/// `write` — header `write <path>` + a content preview from the call args (`write.ts:131-179`).
pub(super) fn render_write(
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
    // `formatWriteResult` shows output only on error (write.ts:164-179).
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

pub(super) fn render_edit(
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

    if run.is_error
        && let Some(result) = &run.result
    {
        // `if (!errorText || errorText === previewError) return undefined` (edit.ts:215-217).
        if preview_error.is_some_and(|e| result_text(result).trim() == e.trim()) {
            return;
        }
        push_error_body(result, theme, out);
    }
}

/// `bash`/`powershell` — header `<prompt> <command> (timeout Ns)` + the output tail (collapsed =
/// last 5 visual lines) + truncation notices + a `Took {d}s` footer (`bash.ts:201-289/430-464`).
pub(super) fn render_bash(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
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
        // `${timeout}s` (bash.ts:204) — the same `String(n)` fold the read range uses; the `±0`
        // case `js_number` handles is already excluded by the filter above.
        spans.push(Span::styled(format!(" (timeout {}s)", js_number(t)), theme.muted_style()));
    }
    out.push(Line::from(spans));

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

/// `grep` — header `grep /<pattern>/ in <path> (glob) limit N` + matching lines (head-15) + a
/// `[Truncated: …]` notice (`grep.ts:68-121/370-379`).
pub(super) fn render_grep(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
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
    if let Some(limit) = run.args.get("limit").and_then(Value::as_i64) {
        spans.push(Span::styled(format!(" limit {limit}"), outp));
    }
    out.push(Line::from(spans));
    push_list_output(run, expanded, 15, theme, expand_key, out);
    push_grep_warnings(run.result.as_ref(), theme, out);
}

/// `find` — header `find <pattern> in <path> (limit N)` + matching paths (head-20) + a `[Truncated: …]`
/// notice (`find.ts:59-107/359-368`).
pub(super) fn render_find(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
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
    if let Some(limit) = run.args.get("limit").and_then(Value::as_i64) {
        spans.push(Span::styled(format!(" (limit {limit})"), outp));
    }
    out.push(Line::from(spans));
    push_list_output(run, expanded, 20, theme, expand_key, out);
    push_find_warnings(run.result.as_ref(), theme, out);
}

/// `ls` — header `ls <path> (limit N)` + entries (head-20) + a `[Truncated: …]` notice
/// (`ls.ts:52-93/210-219`).
pub(super) fn render_ls(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
    out: &mut Vec<Line<'static>>,
) {
    let mut spans = vec![Span::styled("ls ".to_string(), theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["path"], Some("."), theme, opts));
    if let Some(limit) = run.args.get("limit").and_then(Value::as_i64) {
        spans.push(Span::styled(format!(" (limit {limit})"), theme.tool_output_style()));
    }
    out.push(Line::from(spans));
    push_list_output(run, expanded, 20, theme, opts.expand_key, out);
    push_ls_warnings(run.result.as_ref(), theme, out);
}

/// Non-built-in tools fall back to Pi's `formatToolExecution` (tool-execution.ts:365-376): the bold
/// tool name + pretty-printed args + any text output.
/// Draw a tool whose renderer an extension supplied (EXT-006). The extension's `renderCall` text
/// is the header; its `renderResult` text is the body, shown once the run finishes (collapsed runs
/// keep the header only, matching every built-in's collapsed form). A half-supplied renderer
/// degrades gracefully: a missing call text falls back to the tool NAME header, a missing result
/// text simply omits the body.
pub(super) fn render_extension(run: &ToolRun, expanded: bool, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    match &run.rendered_call {
        Some(call) => {
            for l in call.split('\n') {
                out.push(Line::styled(l.to_string(), theme.tool_title_style()));
            }
        }
        None => out.push(Line::styled(run.name.clone(), theme.tool_title_style())),
    }
    if let Some(result) = &run.rendered_result
        && (run.done || expanded)
        && !result.trim().is_empty()
    {
        for l in result.split('\n') {
            out.push(Line::styled(l.to_string(), theme.tool_output_style()));
        }
    }
}

pub(super) fn render_generic(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    out.push(Line::styled(run.name.clone(), theme.tool_title_style()));
    if !run.args.is_null()
        && let Ok(pretty) = serde_json::to_string_pretty(&run.args)
    {
        out.push(Line::default());
        for l in pretty.split('\n') {
            out.push(Line::styled(l.to_string(), theme.tool_output_style()));
        }
    }
    if let Some(result) = &run.result {
        let output = result_text(result);
        if !output.trim().is_empty() {
            for l in output.split('\n') {
                out.push(Line::styled(l.to_string(), theme.tool_output_style()));
            }
        }
    }
}
