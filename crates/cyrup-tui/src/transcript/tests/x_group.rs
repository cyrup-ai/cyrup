//! Batch-11 group X — the eight transcript items scheduled by every plan and delivered by none:
//! X6 (`read`/`write` syntax highlighting + `replaceTabs`), X7 (compact `read` classification, =
//! `G30b`), X8 (`edit` preview-state tint), X9 (dim-key/muted-word expand hints resolved from the
//! live keymap), X11 (extension-rendered custom message keeps its own colour), X14 (collapsed
//! branch/compaction summaries), X15 (renderer-failure box).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
use serde_json::json;

use crate::transcript::*;

fn txt(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}
fn texts(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(txt).collect()
}
fn joined(lines: &[Line<'static>]) -> String {
    texts(lines).join("\n")
}
/// The one rendered row whose text contains `needle`.
fn row<'a>(lines: &'a [Line<'static>], needle: &str) -> &'a Line<'static> {
    lines
        .iter()
        .find(|l| txt(l).contains(needle))
        .unwrap_or_else(|| panic!("no row containing {needle:?} in:\n{}", joined(lines)))
}
fn text_result(text: &str, details: Value) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "details": details })
}
/// One settled tool run, rendered through the real [`tool_lines`] dispatch.
fn run_lines(
    name: &str,
    args: Value,
    result: Option<Value>,
    expanded: bool,
    opts: ImageOpts<'_>,
) -> Vec<Line<'static>> {
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    view.push_tool_start(name, args);
    if let Some(r) = result {
        view.push_tool_end(name, false, Some(r));
    }
    let run = view.active_tools()[0].clone();
    let mut out = Vec::new();
    out.extend(tool_lines(&run, expanded, 100, &theme, opts));
    out
}

// --- X6 -------------------------------------------------------------------------------------

/// **X6 — an expanded `read` of a source file is SYNTAX HIGHLIGHTED, not one flat grey wall.**
///
/// `read.ts:184-190`:
/// ```ts
/// const lang = !isError && rawPath ? getLanguageFromPath(rawPath) : undefined;
/// const renderedLines = lang ? highlightCode(replaceTabs(output), lang) : output.split("\n");
/// …displayLines.map((line) => (lang ? replaceTabs(line) : theme.fg("toolOutput", replaceTabs(line))))
/// ```
/// so with a language in hand the body carries the highlighter's colours and NOT `toolOutput`.
#[test]
fn x6_expanded_read_of_a_rust_file_is_highlighted() {
    let theme = UiTheme::dark();
    let lines = run_lines(
        "read",
        json!({ "path": "src/main.rs" }),
        Some(text_result("// a comment\nfn main() {}", json!(null))),
        true,
        ImageOpts::default(),
    );
    let comment = row(&lines, "a comment");
    let comment_span = comment
        .spans
        .iter()
        .find(|s| s.content.contains("comment"))
        .expect("the comment text is on the row");
    assert_ne!(
        comment_span.style,
        theme.tool_output_style(),
        "a highlighted row is NOT painted `toolOutput`:\n{}",
        joined(&lines)
    );
    assert_eq!(
        comment_span.style,
        theme.syntax_style_for_scope("comment.line").unwrap(),
        "`// a comment` takes the syntaxComment role"
    );
    // The `fn` keyword on the next row proves the highlighter ran over the whole body, not just
    // the first line.
    let decl = row(&lines, "fn main");
    assert!(
        decl.spans
            .iter()
            .any(|s| s.style == theme.syntax_style_for_scope("keyword").unwrap()),
        "`fn` takes the syntaxKeyword role:\n{}",
        joined(&lines)
    );

    // MIRROR: a path whose extension `getLanguageFromPath` does not know has `lang === undefined`,
    // so every row stays flat `theme.fg("toolOutput", …)` — the `: ` arm of the same ternary.
    let flat = run_lines(
        "read",
        json!({ "path": "notes.unknownext" }),
        Some(text_result("// a comment\nfn main() {}", json!(null))),
        true,
        ImageOpts::default(),
    );
    // The flat arm is one `Line::styled(replaceTabs(line), toolOutput)` — the colour rides on
    // the row, not on per-token spans, because there are no tokens.
    assert_eq!(
        row(&flat, "a comment").style.fg,
        theme.tool_output_style().fg,
        "unknown extension ⇒ flat toolOutput:\n{}",
        joined(&flat)
    );
}

/// **X6 — tabs become exactly three spaces (`replaceTabs`, `render-utils.ts:31-33`).**
#[test]
fn x6_tabs_are_replaced_with_three_spaces() {
    let lines = run_lines(
        "read",
        json!({ "path": "notes.unknownext" }),
        Some(text_result("a\tb", json!(null))),
        true,
        ImageOpts::default(),
    );
    assert!(
        joined(&lines).contains("a   b"),
        "tab ⇒ three spaces:\n{}",
        joined(&lines)
    );
    assert!(!joined(&lines).contains('\t'), "no raw tab survives");

    // MIRROR: `write`'s content preview runs through the same `replaceTabs` (`write.ts:160`).
    let w = run_lines(
        "write",
        json!({ "path": "notes.unknownext", "content": "a\tb" }),
        None,
        false,
        ImageOpts::default(),
    );
    assert!(
        joined(&w).contains("a   b"),
        "write preview too:\n{}",
        joined(&w)
    );
}

// --- X7 -------------------------------------------------------------------------------------

/// **X7 (= `G30b`) — a collapsed `read` of a `SKILL.md` is `[skill] <dir> (key to expand)`.**
///
/// `read.ts:336` picks `formatCompactReadCall` only when NOT expanded, and `:130-133` labels a
/// `SKILL.md` with `basename(dirname(absolutePath))`.
#[test]
fn x7_collapsed_read_of_a_skill_md_uses_the_compact_header() {
    let theme = UiTheme::dark();
    let cwd = std::path::Path::new("/home/u/.cyrup");
    let opts = ImageOpts {
        cwd: Some(cwd),
        ..ImageOpts::default()
    };
    let lines = run_lines(
        "read",
        json!({ "path": "skills/commit-helper/SKILL.md" }),
        None,
        false,
        opts,
    );
    let header = row(&lines, "[skill]");
    assert_eq!(
        txt(header).trim_end(),
        " [skill] commit-helper (ctrl+o to expand)",
        "compact skill header (the leading space is the Box's paddingX)"
    );
    assert_eq!(
        header.spans[1].style,
        theme.custom_message_label_style(),
        "`theme.fg(\"customMessageLabel\", …)` on the bracket (read.ts:153)"
    );
    assert_eq!(
        header.spans[2].style,
        theme.custom_message_text_style(),
        "`theme.fg(\"customMessageText\", label)` (read.ts:154)"
    );
    assert!(
        !joined(&lines).contains("SKILL.md"),
        "the raw path is gone:\n{}",
        joined(&lines)
    );

    // MIRROR 1: EXPANDING the same read falls back to the plain `read <path>` header —
    // `!context.expanded ? getCompactReadClassification(...) : undefined` (read.ts:336).
    let expanded = run_lines(
        "read",
        json!({ "path": "skills/commit-helper/SKILL.md" }),
        None,
        true,
        opts,
    );
    assert!(
        joined(&expanded).contains("read skills/commit-helper/SKILL.md"),
        "expanded ⇒ plain header:\n{}",
        joined(&expanded)
    );
    assert!(!joined(&expanded).contains("[skill]"));

    // MIRROR 2: an ordinary source file classifies as nothing and keeps the plain header.
    let plain = run_lines("read", json!({ "path": "src/main.rs" }), None, false, opts);
    assert!(
        joined(&plain).contains("read src/main.rs"),
        "{}",
        joined(&plain)
    );
    assert!(
        !joined(&plain).contains("to expand"),
        "no compact hint on a plain read"
    );
}

/// **X7 — `AGENTS.md`/`CLAUDE.md` classify as `resource`, labelled relative to the cwd.**
///
/// `read.ts:42` `COMPACT_RESOURCE_FILE_NAMES` + `:138-140`, rendered by `:160-165` as
/// `fg("toolTitle", bold("read resource")) + " " + fg("accent", label)`.
#[test]
fn x7_agents_md_is_a_compact_resource_read() {
    let theme = UiTheme::dark();
    let cwd = std::path::Path::new("/w/project");
    let opts = ImageOpts {
        cwd: Some(cwd),
        ..ImageOpts::default()
    };
    let lines = run_lines(
        "read",
        json!({ "path": "docs/AGENTS.md" }),
        None,
        false,
        opts,
    );
    let header = row(&lines, "read resource");
    assert_eq!(
        txt(header).trim_end(),
        " read resource docs/AGENTS.md (ctrl+o to expand)"
    );
    assert_eq!(header.spans[1].style, theme.tool_title_style());
    assert_eq!(
        header.spans[3].style,
        theme.accent_style(),
        "`fg(\"accent\", label)`"
    );

    // MIRROR: the set is matched case-sensitively on the BASENAME, so `agents.md` is not in it.
    let lower = run_lines(
        "read",
        json!({ "path": "docs/agents.md" }),
        None,
        false,
        opts,
    );
    assert!(
        !joined(&lower).contains("read resource"),
        "{}",
        joined(&lower)
    );
    assert!(
        joined(&lower).contains("read docs/agents.md"),
        "{}",
        joined(&lower)
    );
}

/// **X7b — a read under the SHIPPED asset root is a `docs` read, not a resource read.**
///
/// `getPiDocsClassification` (`read.ts:104-121`) + its position AHEAD of
/// `COMPACT_RESOURCE_FILE_NAMES` in `getCompactReadClassification` (`read.ts:136-141`).
#[test]
fn x7b_reads_under_the_asset_root_classify_as_docs() {
    // Tier 3: in a test binary this is the workspace root.
    let root = cyrup_config::asset_dir().expect("asset_dir resolves in a test binary");
    // A cwd deliberately OUTSIDE the asset root, so nothing here can pass by cwd-relative accident.
    let opts = ImageOpts {
        cwd: Some(std::path::Path::new("/w/project")),
        ..ImageOpts::default()
    };
    let read = |p: std::path::PathBuf| {
        run_lines(
            "read",
            json!({ "path": p.to_string_lossy() }),
            None,
            false,
            opts,
        )
    };

    // `label === "README.md"` (`:117`).
    let lines = read(root.join("README.md"));
    assert_eq!(
        txt(row(&lines, "read docs")).trim_end(),
        " read docs README.md (ctrl+o to expand)"
    );

    // `label.startsWith("docs/")` — a nested path keeps its posix-joined relative label.
    let lines = read(root.join("docs/guide/x.md"));
    assert_eq!(
        txt(row(&lines, "read docs")).trim_end(),
        " read docs docs/guide/x.md (ctrl+o to expand)"
    );

    // PRECEDENCE: `docs/AGENTS.md` inside the shipped tree is a DOCS read, not a resource read.
    // This is the ordering the `CompactReadKind` enum was introduced to protect.
    let lines = read(root.join("docs/AGENTS.md"));
    assert_eq!(
        txt(row(&lines, "read docs")).trim_end(),
        " read docs docs/AGENTS.md (ctrl+o to expand)"
    );
    assert!(
        !joined(&lines).contains("read resource"),
        "{}",
        joined(&lines)
    );

    // `resolveToCwd` normalizes lexically, so `docs/../docs/x.md` is the same read as `docs/x.md`.
    let lines = read(root.join("docs/../docs/x.md"));
    assert_eq!(
        txt(row(&lines, "read docs")).trim_end(),
        " read docs docs/x.md (ctrl+o to expand)"
    );
}

/// **X7c — the `docs/` guard requires the separator, and a non-docs sibling is not a docs read.**
///
/// `startsWith("docs/")` (`:117`), NOT `startsWith("docs")`: a read of the `docs` DIRECTORY itself
/// is an ordinary read upstream, and so is any other file at the asset root.
#[test]
fn x7c_the_docs_guard_needs_the_separator() {
    let root = cyrup_config::asset_dir().expect("asset_dir resolves in a test binary");
    let opts = ImageOpts {
        cwd: Some(std::path::Path::new("/w/project")),
        ..ImageOpts::default()
    };
    for path in [root.join("docs"), root.join("CHANGELOG.md")] {
        let lines = run_lines(
            "read",
            json!({ "path": path.to_string_lossy() }),
            None,
            false,
            opts,
        );
        let out = joined(&lines);
        assert!(
            !out.contains("read docs "),
            "generic header expected:\n{out}"
        );
        assert!(
            !out.contains("read resource"),
            "generic header expected:\n{out}"
        );
        // A generic (non-compact) read carries no expand hint — `x_group.rs` MIRROR 2.
        assert!(
            !out.contains("to expand"),
            "generic header expected:\n{out}"
        );
    }
}

/// **X7d — a `resource` read that resolves OUTSIDE the cwd renders ONE leading slash.**
///
/// `formatPathRelativeToCwdOrAbsolute` (`utils/paths.ts:119-122`) falls back to the absolute path
/// and folds it with `.split(sep).join("/")`, where the leading empty segment rejoins to exactly
/// one `/`. The `resolveToCwd` port makes this the arm that `~`, `file://` and `@/abs` land in.
#[test]
fn x7d_a_resource_read_outside_the_cwd_keeps_one_leading_slash() {
    let cwd = std::path::Path::new("/w/project");
    let opts = ImageOpts {
        cwd: Some(cwd),
        ..ImageOpts::default()
    };
    let header = |raw: &str| {
        let lines = run_lines("read", json!({ "path": raw }), None, false, opts);
        txt(row(&lines, "read resource")).trim_end().to_string()
    };

    // A plain absolute path outside the cwd.
    assert_eq!(
        header("/etc/cyrup/AGENTS.md"),
        " read resource /etc/cyrup/AGENTS.md (ctrl+o to expand)"
    );

    // A `file://` URL — resolved by `resolve_to_cwd`, so it too takes the fallback.
    assert_eq!(
        header("file:///etc/cyrup/CLAUDE.md"),
        " read resource /etc/cyrup/CLAUDE.md (ctrl+o to expand)"
    );

    // A `~`-expanded path. The home dir is environment-dependent, so derive the expectation from
    // the same resolver the renderer uses and assert the LABEL SHAPE explicitly.
    let expected = cyrup_tools::path::resolve_to_cwd("~/.cyrup/AGENTS.md", cwd);
    let expected = expected
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    assert_eq!(
        header("~/.cyrup/AGENTS.md"),
        format!(" read resource {expected} (ctrl+o to expand)")
    );
    assert!(
        !expected.contains("//"),
        "sanity: the fixture itself must not be doubled"
    );
    assert!(
        !header("~/.cyrup/AGENTS.md").contains("//"),
        "the doubled-separator regression: {}",
        header("~/.cyrup/AGENTS.md")
    );

    // REGRESSION GUARD (item 1) in its most direct form.
    assert!(!header("/etc/cyrup/AGENTS.md").contains("//"));
}

/// **X7e — a `resource` read of the cwd ITSELF renders `.`, not an empty label.**
///
/// `formatPathRelativeToCwdOrAbsolute` → `getCwdRelativePath` returns `relativePath || "."`
/// (`utils/paths.ts:116`), so upstream can never emit an empty label. RED before the
/// `cyrup_tools::path::format_path_relative_to_cwd_or_absolute` port: `Path::strip_prefix` returns
/// `Ok("")` on equality, so the header came out `" read resource  (ctrl+o to expand)"` — two spaces
/// and no path at all.
///
/// The live blast radius is narrow, and that is exactly why this was easy to leave unfixed: the
/// `resource` arm only fires when the BASENAME is in `COMPACT_RESOURCE_FILE_NAMES`
/// (`AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`, `CLAUDE.MD`), so equality with the
/// cwd is only expressible when the cwd's own basename is one of those names. `/w/AGENTS.md` is also
/// outside `cyrup_config::asset_dir()`, so `docs_classification` cannot intercept first.
#[test]
fn x7e_a_resource_read_of_the_cwd_itself_renders_a_dot() {
    let cwd = std::path::Path::new("/w/AGENTS.md");
    let opts = ImageOpts {
        cwd: Some(cwd),
        ..ImageOpts::default()
    };
    let header = |raw: &str| {
        let lines = run_lines("read", json!({ "path": raw }), None, false, opts);
        txt(row(&lines, "read resource")).trim_end().to_string()
    };

    // Both spellings that resolve ONTO the cwd: the relative `.` and the absolute path itself.
    assert_eq!(header("."), " read resource . (ctrl+o to expand)");
    assert_eq!(
        header("/w/AGENTS.md"),
        " read resource . (ctrl+o to expand)"
    );
}

// --- X8 -------------------------------------------------------------------------------------

/// **X8 — a PENDING `edit` with a computed preview is tinted `toolSuccessBg`, not `toolPendingBg`.**
///
/// `getEditHeaderBg` (`edit.ts:239-253`) tests the preview FIRST and never looks at `done`.
#[test]
fn x8_edit_tint_follows_the_preview_not_done() {
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    view.push_tool_start_rendered(
        "edit".to_string(),
        Some("call-1".to_string()),
        json!({ "path": "a.rs" }),
        None,
    );
    view.set_edit_preview(Some("call-1"), Ok("@@\n-old\n+new".to_string()));
    let run = view.active_tools()[0].clone();
    assert!(!run.done, "still pending — a permission prompt is up");
    let lines = tool_lines(&run, false, 60, &theme, ImageOpts::default());
    let success = theme.tool_bg_style(Style::default(), true, false);
    let pending = theme.tool_bg_style(Style::default(), false, false);
    assert_ne!(
        success, pending,
        "the dark theme distinguishes the two tints"
    );
    assert_eq!(
        lines[1].style, success,
        "a computed preview greens the pending block (edit.ts:244-248)"
    );

    // MIRROR 1: a preview that FAILED reds the same pending block (`"error" in preview`).
    let mut v2 = TranscriptView::new();
    v2.push_tool_start_rendered(
        "edit".to_string(),
        Some("c".to_string()),
        json!({ "path": "a.rs" }),
        None,
    );
    v2.set_edit_preview(Some("c"), Err("no match for oldText".to_string()));
    let r2 = v2.active_tools()[0].clone();
    assert_eq!(
        tool_lines(&r2, false, 60, &theme, ImageOpts::default())[1].style,
        theme.tool_bg_style(Style::default(), false, true),
        "a failed preview reds it (edit.ts:245-246)"
    );

    // MIRROR 2: no preview at all still means `toolPendingBg` — the fix must not green
    // everything (`edit.ts:253`).
    let mut v3 = TranscriptView::new();
    v3.push_tool_start("edit", json!({ "path": "a.rs" }));
    let r3 = v3.active_tools()[0].clone();
    assert_eq!(
        tool_lines(&r3, false, 60, &theme, ImageOpts::default())[1].style,
        pending
    );

    // MIRROR 3: every OTHER tool keeps the `done`/`is_error` keying — `getEditHeaderBg` is
    // `edit`-only, and a pending `read` must stay neutral.
    let r4 = run_lines(
        "read",
        json!({ "path": "a.rs" }),
        None,
        false,
        ImageOpts::default(),
    );
    assert_eq!(r4[1].style, pending, "pending read is untouched by X8");
}

// --- X9 -------------------------------------------------------------------------------------

/// **X9 — the `… to expand` hint is dim-key + muted-words, and the key is the LIVE binding.**
///
/// `read.ts:192` + `keybinding-hints.ts:42-43`.
#[test]
fn x9_more_lines_hint_splits_dim_key_from_muted_words() {
    let theme = UiTheme::dark();
    let body: String = (0..30)
        .map(|i| format!("line {i}\n"))
        .collect::<String>()
        .trim_end()
        .to_string();
    // A collapsed `read` renders no body at all (`read.ts:178-180`), so the hint is exercised
    // through `grep`, whose head-15 collapse uses the very same `more_lines_hint` (`grep.ts:111`
    // is byte-identical to `read.ts:192`).
    let g = run_lines(
        "grep",
        json!({ "pattern": "x" }),
        Some(text_result(&body, json!(null))),
        false,
        ImageOpts::default(),
    );
    let hint = row(&g, "more lines");
    let spans: Vec<(&str, Style)> = hint
        .spans
        .iter()
        .map(|s| (s.content.as_ref(), s.style))
        .collect();
    // [0] is the Box's paddingX margin.
    assert_eq!(spans[1].0, "... (15 more lines,");
    assert_eq!(spans[1].1, theme.muted_style());
    assert_eq!(spans[3].0, "ctrl+o", "the key label is its own span");
    assert_eq!(
        spans[3].1,
        theme.dim_style(),
        "`theme.fg(\"dim\", keyText(...))`"
    );
    assert_eq!(spans[4].0, " to expand");
    assert_eq!(
        spans[4].1,
        theme.muted_style(),
        "the description run is `muted`"
    );
    assert_ne!(
        theme.dim_style(),
        theme.muted_style(),
        "the two roles differ in this theme"
    );

    // MIRROR 1: a REBOUND `app.tools.expand` reaches the hint — the whole point of `keyText`.
    let rebound = run_lines(
        "grep",
        json!({ "pattern": "x" }),
        Some(text_result(&body, json!(null))),
        false,
        ImageOpts {
            expand_key: "ctrl+e/f4",
            ..ImageOpts::default()
        },
    );
    let h2 = row(&rebound, "more lines");
    assert_eq!(h2.spans[3].content.as_ref(), "ctrl+e/f4");
    assert!(
        !txt(h2).contains("ctrl+o"),
        "the literal is gone: {:?}",
        txt(h2)
    );

    // MIRROR 2: the same two-tone shape on the bash tool's `… earlier lines` hint
    // (`bash.ts:281-284`), which had the identical defect.
    let b = run_lines(
        "bash",
        json!({ "command": "ls" }),
        Some(text_result(&body, json!(null))),
        false,
        ImageOpts {
            expand_key: "ctrl+e",
            ..ImageOpts::default()
        },
    );
    let hb = row(&b, "earlier lines");
    assert_eq!(hb.spans[1].content.as_ref(), "... (25 earlier lines,");
    assert_eq!(hb.spans[1].style, theme.muted_style());
    assert_eq!(
        hb.spans[3].content.as_ref(),
        "ctrl+e",
        "resolved, not the `ctrl+o` literal"
    );
    assert_eq!(hb.spans[3].style, theme.dim_style());
    assert_eq!(hb.spans[4].content.as_ref(), " to expand");
    assert_eq!(hb.spans[4].style, theme.muted_style());
}

// --- X11 ------------------------------------------------------------------------------------

/// **X11 — an extension-rendered custom message keeps its own colour; the host adds none.**
///
/// `custom-message.ts:76-81` is `this.addChild(component); return;` — the component goes in
/// as-is. cyrup restyled every row `dim`.
#[test]
fn x11_extension_rendered_message_is_not_forced_dim() {
    let theme = UiTheme::dark();
    let entry = Entry::Custom {
        label: "demo".to_string(),
        body: "ignored".to_string(),
        rendered: Rendered::Text(crate::transcript::RenderedText::frozen(
            "Hello from the extension",
        )),
    };
    let lines = entry_lines(&entry, &theme, 60, 1, ImageOpts::default());
    let r = row(&lines, "Hello from the extension");
    // The old code was `Line::styled(l, theme.dim_style())`, which parks the colour on the ROW,
    // so both the row style and every span style have to be checked — asserting only on
    // `spans[0]` would pass against the defect.
    assert_ne!(
        r.style,
        theme.dim_style(),
        "the host must not repaint the renderer's output"
    );
    assert_eq!(
        r.style,
        Style::default(),
        "added as-is ⇒ no row-level host styling"
    );
    assert!(
        r.spans.iter().all(|s| s.style == Style::default()),
        "…and none on the spans either: {:?}",
        r.spans.iter().map(|s| s.style).collect::<Vec<_>>()
    );

    // MIRROR: the DEFAULT (no renderer) framing is unchanged — still the `[demo]` box whose body
    // is `customMessageText` (`custom-message.ts:92,107-111`).
    let default_entry = Entry::Custom {
        label: "demo".to_string(),
        body: "body text".to_string(),
        rendered: Rendered::None,
    };
    let d = entry_lines(&default_entry, &theme, 60, 1, ImageOpts::default());
    assert!(joined(&d).contains("[demo]"), "{}", joined(&d));
    assert!(joined(&d).contains("body text"), "{}", joined(&d));
}

// --- X15 ------------------------------------------------------------------------------------

/// **X15 — a THROWING renderer draws Pi's failure box, not nothing.**
///
/// `custom-entry.ts:47-52`: a `Box(1, 1, customMessageBg)` holding
/// `theme.fg("error", "[type] renderer failed: <message>")`, then `:59-60`'s `Spacer(1)`.
#[test]
fn x15_a_throwing_renderer_draws_the_failure_box() {
    let theme = UiTheme::dark();
    let entry = Entry::Custom {
        label: "demo".to_string(),
        body: "unused".to_string(),
        rendered: Rendered::Failed("boom".to_string()),
    };
    let lines = entry_lines(&entry, &theme, 60, 1, ImageOpts::default());
    assert!(!lines.is_empty(), "the entry must not vanish");
    assert_eq!(txt(&lines[0]), "", "`custom-entry.ts:59`'s Spacer(1)");
    let r = row(&lines, "renderer failed");
    assert_eq!(txt(r).trim_end(), " [demo] renderer failed: boom");
    assert_eq!(
        r.spans[1].style.fg,
        theme.error_style().fg,
        "`theme.fg(\"error\", …)`"
    );
    assert_eq!(
        r.style.bg,
        theme.custom_message_bg_style().bg,
        "inside a `Box(1, 1, customMessageBg)`"
    );
    assert!(
        !joined(&lines).contains("unused"),
        "the default body is not also drawn"
    );
}

// --- X14 ------------------------------------------------------------------------------------

/// **X14 — a collapsed branch summary is ONE row, and the expand key is the live one.**
///
/// `branch-summary-message.ts:46-56`.
#[test]
fn x14_collapsed_branch_summary_is_one_hint_row() {
    let theme = UiTheme::dark();
    let entry = Entry::BranchSummary {
        summary: "tried the async rewrite, abandoned it".to_string(),
    };
    let lines = entry_lines(&entry, &theme, 60, 1, ImageOpts::default());
    assert!(joined(&lines).contains("[branch]"), "{}", joined(&lines));
    let hint = row(&lines, "Branch summary");
    assert_eq!(txt(hint).trim_end(), " Branch summary (ctrl+o to expand)");
    assert_eq!(
        hint.spans[1].style,
        theme.custom_message_text_style(),
        "`fg(\"customMessageText\", \"Branch summary (\")` — NOT muted (`:49`)"
    );
    assert_eq!(
        hint.spans[2].style,
        theme.dim_style(),
        "`fg(\"dim\", keyText(...))` (`:50`)"
    );
    assert!(
        !joined(&lines).contains("async rewrite"),
        "the body is withheld:\n{}",
        joined(&lines)
    );

    // MIRROR 1: the live keymap label reaches it.
    let rebound = entry_lines(
        &entry,
        &theme,
        60,
        1,
        ImageOpts {
            expand_key: "f2",
            ..ImageOpts::default()
        },
    );
    assert!(
        joined(&rebound).contains("Branch summary (f2 to expand)"),
        "{}",
        joined(&rebound)
    );

    // MIRROR 2: expanded still renders the full markdown body + `**Branch Summary**` header.
    let open = entry_lines(
        &Entry::BranchSummary {
            summary: "tried the async rewrite, abandoned it".to_string(),
        },
        &theme,
        60,
        1,
        ImageOpts {
            tools_expanded: true,
            ..ImageOpts::default()
        },
    );
    assert!(joined(&open).contains("async rewrite"), "{}", joined(&open));
    assert!(
        joined(&open).contains("Branch Summary"),
        "{}",
        joined(&open)
    );

    // MIRROR 3: the compaction variant keeps its grouped token count in the collapsed lead
    // (`compaction-summary-message.ts:50`).
    let comp = entry_lines(
        &Entry::CompactionSummary {
            tokens_before: 123_456,
            summary: "condensed".to_string(),
        },
        &theme,
        60,
        1,
        ImageOpts::default(),
    );
    assert!(
        joined(&comp).contains("Compacted from 123,456 tokens (ctrl+o to expand)"),
        "{}",
        joined(&comp)
    );
    assert!(!joined(&comp).contains("condensed"));
}

/// **X14 — the collapse state is Pi's LIVE `toolOutputExpanded`, read at RENDER time.**
///
/// `setToolsExpanded` does not merely store the flag; it walks `chatContainer.children` and
/// calls `setExpanded(expanded)` on every expandable child (`interactive-mode.ts:4032-4046`),
/// and `BranchSummaryMessageComponent.setExpanded` re-runs `updateDisplay()`
/// (`branch-summary-message.ts:22-25`). So a summary pushed while collapsed — the default,
/// `interactive-mode.ts:442` `private toolOutputExpanded = false` — MUST open when the flag is
/// toggled afterwards.
///
/// This replaces `x14_push_freezes_the_live_tools_expanded_flag`, which asserted
/// `Entry::BranchSummary { expanded: false, .. }` after a `set_tool_expanded(true)` and so
/// pinned the defect: with the flag frozen at push there was NO ordering in which the body
/// could ever be rendered.
#[test]
fn x14_toggling_tools_expanded_reveals_an_already_pushed_summary_body() {
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    view.push_branch_summary("we merged the spike");
    view.push_compaction_summary(1234, "condensed history");

    // Collapsed (Pi's initial `toolOutputExpanded = false`): one hint row each, no body.
    let entries = view.drain_committed();
    let render = |view: &TranscriptView, entries: &[Entry]| -> String {
        entries
            .iter()
            .flat_map(|e| {
                entry_lines(
                    e,
                    &theme,
                    60,
                    1,
                    ImageOpts {
                        tools_expanded: view.tool_expanded(),
                        ..ImageOpts::default()
                    },
                )
            })
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let collapsed = render(&view, &entries);
    assert!(
        collapsed.contains("Branch summary (ctrl+o to expand)"),
        "{collapsed}"
    );
    assert!(
        collapsed.contains("Compacted from 1,234 tokens (ctrl+o to expand)"),
        "{collapsed}"
    );
    assert!(!collapsed.contains("we merged the spike"), "{collapsed}");
    assert!(!collapsed.contains("condensed history"), "{collapsed}");

    // `Ctrl+O` AFTER the push. The SAME entries must now paint their bodies.
    assert!(view.set_tool_expanded(true), "the flag actually changed");
    let expanded = render(&view, &entries);
    assert!(
        expanded.contains("we merged the spike"),
        "the branch body is reachable after the toggle:\n{expanded}"
    );
    assert!(
        expanded.contains("condensed history"),
        "the compaction body is reachable after the toggle:\n{expanded}"
    );
    assert!(
        !expanded.contains("to expand"),
        "and the collapsed hints are gone:\n{expanded}"
    );

    // MIRROR: toggling back re-collapses the same entries — the flag is read, not latched.
    assert!(view.set_tool_expanded(false));
    let recollapsed = render(&view, &entries);
    assert!(
        !recollapsed.contains("we merged the spike"),
        "{recollapsed}"
    );
    assert!(
        recollapsed.contains("Branch summary (ctrl+o to expand)"),
        "{recollapsed}"
    );
}

/// **X7 — `language_from_path` is the `getLanguageFromPath` table verbatim
/// (`theme.ts:1184-1250`).**
#[test]
fn x6_language_from_path_matches_pis_table() {
    use crate::theme::language_from_path as lang;
    assert_eq!(lang("a.rs"), Some("rust"));
    assert_eq!(
        lang("a.TSX"),
        Some("typescript"),
        "the extension is lower-cased"
    );
    assert_eq!(lang("a.zsh"), Some("bash"));
    assert_eq!(lang("a.hpp"), Some("cpp"));
    assert_eq!(lang("a.yml"), Some("yaml"));
    assert_eq!(
        lang("nodots"),
        None,
        "`split(\".\").pop()` yields the whole name ⇒ no match"
    );
    assert_eq!(lang("a.nope"), None);
}
