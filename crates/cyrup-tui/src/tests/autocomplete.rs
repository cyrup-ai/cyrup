//! Autocomplete + SelectList + fuzzy tests (spec/tui/04 §3-5; gaps 3/4).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::harness::key_event as key;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{
    App, Autocomplete, ColumnLayout, CommandRegistry, CommandSource, EditorOutcome, InputEditor,
    SelectItem, SelectList, SlashCommand, UiTheme, fuzzy_filter, fuzzy_match, fuzzy_score,
};
use ratatui::backend::TestBackend;
use std::path::Path;

fn type_str(ed: &mut InputEditor, s: &str) {
    for c in s.chars() {
        ed.handle_key(&key(KeyCode::Char(c)));
    }
}

// ---- fuzzy --------------------------------------------------------------------------------

#[test]
fn fuzzy_subsequence_and_ordering() {
    // Non-subsequence → None; subsequence → Some. (Pi: lower score = better.)
    assert!(fuzzy_score("settings", "xyz").is_none());
    assert!(fuzzy_score("settings", "set").is_some());
    // Prefix/boundary match outranks a scattered one → LOWER score (fuzzy.ts:35-49).
    let prefix = fuzzy_score("settings", "set").unwrap();
    let scattered = fuzzy_score("scoped-models", "set").unwrap_or(f64::MAX);
    assert!(
        prefix < scattered,
        "prefix {prefix} should beat (be lower than) scattered {scattered}"
    );
    // Empty query matches everything at score 0.
    assert_eq!(fuzzy_score("anything", ""), Some(0.0));
    // Query longer than text never matches (fuzzy.ts:21-23).
    assert!(fuzzy_score("se", "settings").is_none());
}

#[test]
fn fuzzy_exact_and_boundary_bonuses() {
    // Whole-string-exact gets the -100 bonus (fuzzy.ts:63-65), beating a mere prefix.
    let exact = fuzzy_match("set", "set").unwrap();
    let prefix = fuzzy_match("set", "settings").unwrap();
    assert!(
        exact < prefix - 90.0,
        "exact {exact} should be ~100 below prefix {prefix}"
    );
    // Word-boundary match (after '-') earns the -10 bonus vs a non-boundary interior match.
    let boundary = fuzzy_match("m", "scoped-models").unwrap();
    let interior = fuzzy_match("e", "scoped-models").unwrap();
    assert!(
        boundary < interior,
        "boundary {boundary} should beat interior {interior}"
    );
}

#[test]
fn fuzzy_alphanumeric_swap_fallback() {
    // "gpt4" should match "gpt-4o" directly; the swap fallback rescues "4gpt" → +5 penalty.
    let direct = fuzzy_match("gpt4", "gpt-4o");
    assert!(direct.is_some());
    let swapped = fuzzy_match("4gpt", "gpt-4o");
    assert!(
        swapped.is_some(),
        "alphanumeric-swap fallback should match (fuzzy.ts:75-92)"
    );
    // The swapped retry is penalized by +5 over the equivalent direct query.
    assert!(
        swapped.unwrap() > direct.unwrap(),
        "swap retry carries +5 penalty"
    );
    // No swap is possible for a pure-letter query that fails → None.
    assert!(fuzzy_match("zzz", "gpt-4o").is_none());
}

#[test]
fn fuzzy_filter_ranks_best_first() {
    let items = ["settings", "session", "scoped-models"];
    let ranked = fuzzy_filter(&items, "se", |s| *s);
    // "settings" and "session" both start with "se"; "scoped-models" matches s..e scattered.
    assert_eq!(
        ranked.first().map(|m| items[m.index]),
        Some("settings").or(Some("session"))
    );
    assert!(ranked.iter().any(|m| items[m.index] == "session"));
    // Scattered match ranks last (highest score).
    assert_eq!(ranked.last().map(|m| items[m.index]), Some("scoped-models"));
}

#[test]
fn fuzzy_filter_requires_all_tokens() {
    // Multi-token query (whitespace/'/'-separated): every token must match (fuzzy.ts:120-128).
    let items = ["scoped models", "settings", "session"];
    let ranked = fuzzy_filter(&items, "sc mo", |s| *s);
    assert_eq!(ranked.len(), 1);
    assert_eq!(
        ranked.first().map(|m| items[m.index]),
        Some("scoped models")
    );
    // Empty/whitespace query keeps every item in original order.
    let all = fuzzy_filter(&items, "   ", |s| *s);
    assert_eq!(
        all.iter().map(|m| m.index).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

// ---- SelectList ---------------------------------------------------------------------------

#[test]
fn select_list_wraps_navigation() {
    let items = vec![
        SelectItem::label("a"),
        SelectItem::label("b"),
        SelectItem::label("c"),
    ];
    let mut list = SelectList::new(items, ColumnLayout::DEFAULT);
    assert_eq!(list.selected(), 0);
    list.select_up(); // wraps to bottom
    assert_eq!(list.selected(), 2);
    list.select_down(); // wraps to top
    assert_eq!(list.selected(), 0);
}

#[test]
fn select_list_windows_and_indicates_scroll() {
    let items: Vec<SelectItem> = (0..22)
        .map(|i| SelectItem::label(format!("cmd{i}")))
        .collect();
    let mut list = SelectList::new(items, ColumnLayout::SLASH);
    list.set_max_visible(5);
    let theme = UiTheme::dark();
    let lines = list.lines(60, &theme);
    // 5 rows + 1 scroll indicator.
    assert_eq!(lines.len(), 6);
    let last: String = lines[5].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(last.contains("(1/22)"), "scroll indicator missing: {last}");
    // Selected row carries the → glyph.
    let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(first.starts_with("→ "), "selection glyph missing: {first}");
}

// ---- slash autocomplete in the editor -----------------------------------------------------

#[test]
fn typing_slash_opens_command_popup() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/se");
    assert!(ed.autocomplete_open(), "slash popup did not open");
    let ac = ed.autocomplete().unwrap();
    // Top candidate matches "se" — settings or session.
    let top = ac.list.selected_item().unwrap();
    assert!(
        top.label == "settings" || top.label == "session",
        "unexpected top: {}",
        top.label
    );
}

#[test]
fn tab_accepts_slash_completion_with_trailing_space() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/sett");
    // Tab accepts the selected item, keeps editing.
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "/settings ");
    // The popup closed after acceptance left no slash context (there is a trailing space now).
    assert!(!ed.autocomplete_open());
}

#[test]
fn enter_on_slash_popup_submits_immediately() {
    // spec/tui/04 §5 edge 15: accepting a slash item with Enter submits.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/tre");
    let out = ed.handle_key(&key(KeyCode::Enter));
    assert_eq!(out, EditorOutcome::Submit("/tree".to_string()));
    assert!(ed.is_empty());
}

#[test]
fn esc_cancels_popup_keeps_text() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/mod");
    assert!(ed.autocomplete_open());
    ed.handle_key(&key(KeyCode::Esc));
    assert!(!ed.autocomplete_open());
    assert_eq!(ed.text(), "/mod");
}

#[test]
fn autocomplete_max_visible_is_plumbed_and_clamped() {
    // Item #6 — the `autocompleteMaxVisible` setting drives the dropdown height (clamped 3–20).
    let mut ed = InputEditor::new();
    ed.set_autocomplete_max_visible(8);
    type_str(&mut ed, "/s");
    assert_eq!(
        ed.autocomplete().unwrap().list.max_visible(),
        8,
        "popup height not plumbed"
    );
    // Out-of-range values clamp to 3–20 (and re-apply to the open popup).
    ed.set_autocomplete_max_visible(99);
    assert_eq!(ed.autocomplete().unwrap().list.max_visible(), 20);
    ed.set_autocomplete_max_visible(1);
    assert_eq!(ed.autocomplete().unwrap().list.max_visible(), 3);
}

#[test]
fn best_match_is_preselected() {
    // Item #6 — the popup preselects the best fuzzy match (row 0 after the score sort), so a bare
    // Tab/Enter accepts the strongest candidate without navigating.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/sett");
    let ac = ed.autocomplete().unwrap();
    assert_eq!(
        ac.list.selected(),
        0,
        "best match must be preselected at row 0"
    );
    assert_eq!(ac.list.selected_item().unwrap().label, "settings");
}

#[test]
fn autocomplete_popup_keys_are_configurable() {
    // Item #6 — the popup nav/accept/cancel keys are no longer hardcoded: a `keybindings.json` rebind
    // (`tui.autocomplete.*`) takes effect. Rebind cancel from Esc to Ctrl+G, and accept to Ctrl+Y.
    let mut ed = InputEditor::new();
    ed.merge_keybindings_json(
        r#"{ "tui.autocomplete.cancel": "ctrl+g", "tui.autocomplete.accept": "ctrl+y" }"#,
    )
    .unwrap();
    type_str(&mut ed, "/sett");
    assert!(ed.autocomplete_open());
    // The rebound accept key applies the completion.
    ed.handle_key(&KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(ed.text(), "/settings ");

    // The rebound cancel key dismisses a fresh popup (clear the buffer so `/mod` is a command again).
    ed.clear();
    type_str(&mut ed, "/mod");
    assert!(ed.autocomplete_open());
    ed.handle_key(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert!(
        !ed.autocomplete_open(),
        "rebound cancel key did not dismiss the popup"
    );
}

#[test]
fn popup_renders_below_editor_in_viewport() {
    // The popup is appended below the editor in the live region (spec/tui/04 §7).
    let mut app = App::new(TestBackend::new(70, 16), UiTheme::dark()).unwrap();
    for c in "/se".chars() {
        app.editor_mut().handle_key(&key(KeyCode::Char(c)));
    }
    app.draw().unwrap();
    let buf = app.terminal().backend().buffer();
    let mut text = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell((x, y)) {
                text.push_str(cell.symbol());
            }
        }
        text.push('\n');
    }
    assert!(
        text.contains("settings"),
        "popup row 'settings' missing from viewport:\n{text}"
    );
    assert!(
        text.contains("Open settings menu"),
        "description column missing:\n{text}"
    );
}

// ---- @-mention search (autocomplete.ts:101,164,408) ---------------------------------------

#[test]
fn at_mention_auto_pops_and_fuzzy_filters_the_tree() {
    let mut ed = InputEditor::new();
    ed.set_mention_files(vec![
        "src/app.rs".to_string(),
        "src/editor.rs".to_string(),
        "Cargo.toml".to_string(),
        "README.md".to_string(),
    ]);
    // Typing `@` auto-opens the mention popup over the whole tree (no Tab needed).
    type_str(&mut ed, "@");
    assert!(
        ed.autocomplete_open(),
        "@ did not auto-open the mention popup"
    );
    // Narrowing by a fuzzy query keeps the popup and ranks matches.
    type_str(&mut ed, "edit");
    let ac = ed.autocomplete().unwrap();
    let top = ac.list.selected_item().unwrap();
    assert_eq!(
        top.label, "src/editor.rs",
        "fuzzy mention ranking wrong; got {}",
        top.label
    );
}

#[test]
fn at_mention_accept_inserts_path_with_trailing_space() {
    let mut ed = InputEditor::new();
    ed.set_mention_files(vec!["src/editor.rs".to_string(), "src/app.rs".to_string()]);
    type_str(&mut ed, "look at @edit");
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "look at @src/editor.rs ");
    assert!(
        !ed.autocomplete_open(),
        "popup should close once the mention completes"
    );
}

#[test]
fn at_mention_quotes_paths_with_spaces() {
    let mut ed = InputEditor::new();
    ed.set_mention_files(vec!["my docs/notes.md".to_string()]);
    type_str(&mut ed, "@notes");
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "@\"my docs/notes.md\" ");
}

#[test]
fn mention_list_files_walks_the_tree_skipping_vcs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("src/main.rs"), "").unwrap();
    std::fs::write(root.join("Cargo.toml"), "").unwrap();
    std::fs::write(root.join(".git/HEAD"), "").unwrap();
    // The walk fallback (used when `fd` is absent) is exercised directly via the public lister.
    let files = crate::mention_list_files(root, 100);
    assert!(
        files.contains(&"src/main.rs".to_string()),
        "missing nested file: {files:?}"
    );
    assert!(
        files.contains(&"Cargo.toml".to_string()),
        "missing root file: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.contains(".git")),
        ".git must be skipped: {files:?}"
    );
}

// ---- S35: multi-line descriptions in the slash popup ---------------------------------------

/// **S35.** `normalizeToSingleLine` (`select-list.ts:9`) —
/// `text.replace(/[\r\n]+/g, " ").trim()` — is applied at `:98`, *inside* `SelectList.render`,
/// before `renderItem` ever sees the description. The slash popup therefore inherits it by
/// construction: `Autocomplete` builds a real `SelectList` (`autocomplete.rs`
/// `slash_context`) and `App::draw` renders it through `ac.list.lines(...)`.
///
/// This is the property the audit doubted. The test pins it from the popup's own data path so a
/// future refactor that gives the popup its own row builder fails here.
#[test]
fn slash_popup_descriptions_are_collapsed_to_one_line() {
    let registry = CommandRegistry::with_dynamic([SlashCommand {
        name: "review".into(),
        // A prompt-template command whose front-matter description spans lines — the exact shape
        // that used to inject a raw control character into the popup row.
        description: "Review the diff\r\n\nfor correctness bugs".into(),
        argument_hint: None,
        source: CommandSource::Prompt,
        arg_completion: crate::commands::ArgumentCompleter::None,
    }]);
    let ac = Autocomplete::compute(
        &registry,
        &crate::autocomplete::ArgumentSources::default(),
        &["/review".to_string()],
        0,
        7,
        false,
        Path::new("."),
    )
    .expect("slash popup should open");
    let theme = UiTheme::dark();
    let lines = ac.list.lines(90, &theme);
    let row: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        !row.contains('\n'),
        "no raw newline reaches the row: {row:?}"
    );
    assert!(
        !row.contains('\r'),
        "no raw carriage return reaches the row: {row:?}"
    );
    // `[\r\n]+` is one regex alternation with a `+`, so the whole run collapses to ONE space.
    assert!(
        row.contains("Review the diff for correctness bugs"),
        "the run of breaks collapses to a single space: {row:?}"
    );
}

/// The same normalization applies to `SelectList` directly, and it TRIMS the result — an
/// all-whitespace description normalizes to `""`, which is falsy in JS, so `:149`'s two-column gate
/// takes the single-column arm.
#[test]
fn select_list_normalizes_and_trims_descriptions() {
    let list = SelectList::new(
        vec![
            SelectItem::new("a", Some("\n  spaced\r\n\r\nout  \n".to_string())),
            SelectItem::new("b", Some("   \n  ".to_string())),
        ],
        ColumnLayout::SLASH,
    );
    let theme = UiTheme::dark();
    let lines = list.lines(90, &theme);
    let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        first.trim_end().ends_with("spaced out"),
        "collapsed + trimmed: {first:?}"
    );
    let second: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(
        second, "  b",
        "a whitespace-only description drops the second column: {second:?}"
    );
}

// ---- TUI-013: quoted paths with spaces ------------------------------------------------------

/// A path containing a space is completable only through the quoted form, and cyrup's
/// `trailing_token` split on `PATH_DELIMS` — which *includes* `"` and `' '` — so the token for
/// `see @"my dir/fi` was `dir/fi`, whose `strip_prefix('@')` then failed and returned `None`.
///
/// Upstream scans back for an unclosed quote FIRST and treats everything after it (plus a leading
/// `@`) as one token: `extractAtPrefix`/`extractPathPrefix` both open with
/// `const quotedPrefix = extractQuotedPrefix(text); if (quotedPrefix) return quotedPrefix;`
/// (`packages/tui/src/autocomplete.ts:463-470`, `:480-487` @v0.83.0), over
/// `findUnclosedQuoteStart` (`:54-68`).
#[test]
fn an_unclosed_quote_keeps_a_path_with_spaces_as_one_mention_token() {
    assert_eq!(
        crate::mention_query("see @\"my dir/fi").as_deref(),
        Some("my dir/fi")
    );
    // Single quotes are a `PATH_DELIMITERS` member upstream but are NOT what
    // `findUnclosedQuoteStart` scans for — it tests `text[i] === '"'` only — so `'` still splits.
    assert_eq!(crate::mention_query("see @'my dir/fi").as_deref(), None);
}

/// A CLOSED quote is not a token boundary override: with every `"` balanced,
/// `findUnclosedQuoteStart` returns `null` and the ordinary last-delimiter split applies.
#[test]
fn a_closed_quote_falls_back_to_the_delimiter_split() {
    // `@"done" @stil` — the first pair is balanced, so the token is the trailing `@stil`.
    assert_eq!(
        crate::mention_query("@\"done\" @stil").as_deref(),
        Some("stil")
    );
}

/// `isTokenStart` (`autocomplete.ts:70-72`) rejects a quote that opens mid-token, so an
/// apostrophe-style `"` glued to a word does not swallow the line.
#[test]
fn a_quote_that_does_not_start_a_token_is_not_a_quoted_prefix() {
    // The `"` is preceded by `o`, which is not a path delimiter → not a quoted prefix, so the
    // ordinary split runs and the trailing token is everything after the last `"`.
    assert_eq!(crate::mention_query("foo\"bar").as_deref(), None);
}

// ---- CMDHINT_01: is_command_prefix -----------------------------------------------------------

/// `is_command_prefix` is a strict `starts_with`, so a genuine command-name prefix matches and a
/// full exact name also matches (a name is trivially a prefix of itself).
#[test]
fn is_command_prefix_matches_a_genuine_prefix_and_the_full_name() {
    let reg = CommandRegistry::new();
    assert!(
        crate::autocomplete::is_command_prefix(&reg, "mod"),
        "\"mod\" prefixes \"model\""
    );
    assert!(
        crate::autocomplete::is_command_prefix(&reg, "model"),
        "a full name is its own prefix"
    );
    assert!(
        crate::autocomplete::is_command_prefix(&reg, "se"),
        "\"se\" prefixes \"settings\""
    );
}

/// A query matching no registered name at all — including one that IS a fuzzy subsequence match —
/// is not a prefix. No builtin starts with `"fa"`.
#[test]
fn is_command_prefix_rejects_a_non_prefix() {
    let reg = CommandRegistry::new();
    assert!(!crate::autocomplete::is_command_prefix(&reg, "fa"));
    assert!(!crate::autocomplete::is_command_prefix(&reg, "zzz"));
    // Case matters: registry names are lowercase, `starts_with` is byte-exact.
    assert!(!crate::autocomplete::is_command_prefix(&reg, "MOD"));
}

/// An empty query is false, even though a bare `/` opens a full, unfiltered popup
/// (`fuzzy::filter` returns everything for an empty query, `fuzzy.rs:145-151`) — the two questions
/// are deliberately different: "is there anything to show" vs "does this confirm a real command".
#[test]
fn is_command_prefix_rejects_the_empty_query() {
    let reg = CommandRegistry::new();
    assert!(!crate::autocomplete::is_command_prefix(&reg, ""));
}

/// The counterexample the doc comment cites: fuzzy-matching is lenient enough to match a
/// non-contiguous subsequence (`f`→`a` inside `flux/aug`) where `is_command_prefix` correctly does
/// not. Proven against a REAL dynamic command name, not just an absence in the builtin table.
#[test]
fn fuzzy_matches_where_is_command_prefix_does_not() {
    let flux_aug = SlashCommand {
        name: std::borrow::Cow::Borrowed("flux/aug"),
        description: std::borrow::Cow::Borrowed("Augment a task"),
        argument_hint: None,
        source: CommandSource::Prompt,
        arg_completion: crate::commands::ArgumentCompleter::None,
    };
    let reg = CommandRegistry::with_dynamic(vec![flux_aug]);
    assert!(
        reg.get("flux/aug").is_some(),
        "the dynamic command registered"
    );

    // `is_command_prefix`: "fa" is not the start of "flux/aug".
    assert!(!crate::autocomplete::is_command_prefix(&reg, "fa"));

    // `fuzzy::filter` (what the popup itself uses): "fa" IS a subsequence of "flux/aug", so the
    // popup surfaces it — correct for a suggestion list, wrong as a highlight confirmation.
    let matches = crate::fuzzy_filter(reg.commands(), "fa", |c| c.name.as_ref());
    assert!(
        matches
            .iter()
            .any(|m| reg.commands()[m.index].name == "flux/aug"),
        "fuzzy::filter should still match \"fa\" against \"flux/aug\" (subsequence f→a): {matches:?}"
    );
}

// ---- TUI-077: the unforced slash branch is terminal ------------------------------------------

/// A scratch cwd holding `src/` and `srcmap.md`, so a path query for `sr` has two candidates and a
/// forced Tab does not auto-apply a lone match.
fn slash_path_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("srcmap.md"), "").unwrap();
    dir
}

fn compute_at_end(line: &str, force: bool, cwd: &Path) -> Option<Autocomplete> {
    Autocomplete::compute(
        &CommandRegistry::new(),
        &crate::autocomplete::ArgumentSources::default(),
        &[line.to_string()],
        0,
        line.chars().count(),
        force,
        cwd,
    )
}

/// The ledger's Verify: `/export ./sr` and `/Users/dav`-shaped lines produce no path completions
/// unforced, while `./sr` alone still does — every exit of pi's `!options.force &&
/// textBeforeCursor.startsWith("/")` block returns (`autocomplete.ts:318-373` @v0.86.0).
#[test]
fn an_unforced_slash_line_never_falls_through_to_paths() {
    let dir = slash_path_fixture();
    let cwd = dir.path();
    assert!(
        compute_at_end("/export ./sr", false, cwd).is_none(),
        "`/export` owns no completer, so its slash branch answers nothing"
    );
    let absolute = format!("{}/sr", cwd.display());
    assert!(absolute.starts_with('/'));
    assert!(
        compute_at_end(&absolute, false, cwd).is_none(),
        "an absolute path typed at line start is a slash line to pi, not a path: {absolute}"
    );
    let plain = compute_at_end("./sr", false, cwd).expect("a bare path still completes");
    assert_eq!(plain.context, crate::CompletionContext::Path);
    // pi's forced request skips the slash branch entirely, so Tab still completes the path.
    let forced = compute_at_end("/export ./sr", true, cwd).expect("forced path completion");
    assert_eq!(forced.context, crate::CompletionContext::Path);
}

/// The same through the editor: an open `/` popup must not turn into a file list when the rest of
/// the line lands in one edit.
#[test]
fn an_open_slash_popup_does_not_become_a_file_list() {
    let dir = slash_path_fixture();
    let mut ed = InputEditor::new();
    ed.set_cwd(dir.path().to_path_buf());
    type_str(&mut ed, "/");
    assert!(
        ed.autocomplete_open(),
        "baseline: `/` opens the command popup"
    );
    ed.insert_str("export ./sr");
    assert!(
        !ed.autocomplete_open(),
        "a path popup under a slash line: {:?}",
        ed.autocomplete().map(|ac| ac.context)
    );
}

/// pi's `handleTabCompletion` (`components/editor.ts:2263-2274` @v0.86.0): Tab on a first-line
/// `/name` with no space is an UNFORCED slash request, so an absolute path typed at line start gets
/// nothing — not a directory listing, and not a silently auto-applied lone match.
#[test]
fn tab_on_a_slash_line_without_a_space_is_not_a_file_request() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("only")).unwrap();
    let line = format!("{}/on", dir.path().display());
    let mut ed = InputEditor::new();
    ed.set_cwd(dir.path().to_path_buf());
    ed.set_text(&line);
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), line, "Tab must not complete a path here");
    assert!(!ed.autocomplete_open());
}

/// A Tab-opened popup stays FORCED while it is open — pi re-asks with `force:
/// this.autocompleteState === "force"` (`components/editor.ts:2468` @v0.86.0) — so narrowing a
/// non-path-looking argument on a `/`-line keeps the file list instead of closing it.
#[test]
fn a_forced_popup_keeps_narrowing_on_a_slash_line() {
    let dir = slash_path_fixture();
    let mut ed = InputEditor::new();
    ed.set_cwd(dir.path().to_path_buf());
    ed.set_text("/export sr");
    ed.handle_key(&key(KeyCode::Tab));
    let ac = ed.autocomplete().expect("Tab opens the forced file list");
    assert_eq!(ac.context, crate::CompletionContext::Path);
    type_str(&mut ed, "c");
    let ac = ed
        .autocomplete()
        .expect("the forced popup must survive the next keystroke");
    assert_eq!(ac.context, crate::CompletionContext::Path);
    assert_eq!(ac.prefix, "src");
}

// ---- TUI-100: skills rank on their bare name ---------------------------------------------------

fn review_registry() -> CommandRegistry {
    let command = |name: &'static str, source| SlashCommand {
        name: std::borrow::Cow::Borrowed(name),
        description: std::borrow::Cow::Borrowed(""),
        argument_hint: None,
        source,
        arg_completion: crate::commands::ArgumentCompleter::None,
    };
    CommandRegistry::with_dynamic([
        command("review-pr", CommandSource::Prompt),
        command("skill:review", CommandSource::Skill),
    ])
}

/// pi v0.86.0 (#9120): typing a skill's bare name ranks it on that name, so its exact match beats
/// a prompt that only starts with it.
#[test]
fn a_skill_ranks_on_its_bare_name() {
    let mut ed = InputEditor::new();
    ed.set_registry(review_registry());
    type_str(&mut ed, "/review");
    let ac = ed.autocomplete().expect("slash popup");
    let labels: Vec<&str> = ac.list.items().iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels.first(), Some(&"skill:review"), "order: {labels:?}");
    // The inserted value is still the full name.
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "/skill:review ");
}

/// A query that itself names the `skill:` prefix still reaches the skill, on its full name.
#[test]
fn a_skill_prefixed_query_still_matches_the_full_name() {
    let mut ed = InputEditor::new();
    ed.set_registry(review_registry());
    type_str(&mut ed, "/skill:rev");
    let ac = ed.autocomplete().expect("slash popup");
    let labels: Vec<&str> = ac.list.items().iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["skill:review"]);
}

/// `/skill` is a prefix of every skill's full name and a subsequence of none of the bare names;
/// the second pass (upstream `36af9dc48`, #9944) is what still lists them.
#[test]
fn a_bare_skill_query_still_lists_the_skills() {
    let mut ed = InputEditor::new();
    ed.set_registry(review_registry());
    type_str(&mut ed, "/skill");
    let ac = ed.autocomplete().expect("slash popup");
    let labels: Vec<&str> = ac.list.items().iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["skill:review"]);
}

// ---- TUI-101: CJK punctuation bounds a completion token --------------------------------------

/// pi v0.86.0 (#9746): `，` is an autocomplete separator, so `看看，@sr` is a mention.
#[test]
fn cjk_punctuation_starts_a_mention_token() {
    let mut ed = InputEditor::new();
    ed.set_mention_files(vec!["src/main.rs".to_string()]);
    type_str(&mut ed, "看看，@sr");
    let ac = ed.autocomplete().expect("the mention popup must open");
    assert_eq!(ac.context, crate::CompletionContext::Mention);
    assert_eq!(ac.prefix, "@sr");
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "看看，@src/main.rs ");
}

/// The `\p{Punctuation}` ∩ CJK-script half of `cjkPunctuationRegex` (`utils.ts:58-61` @v0.86.0),
/// not just its explicit list: `。` (U+3002) and `、` (U+3001) are separators, while ASCII
/// punctuation — `Script=Common` only — still is not.
#[test]
fn cjk_script_punctuation_beyond_the_explicit_list_is_a_separator() {
    for sep in ['。', '、'] {
        let mut ed = InputEditor::new();
        ed.set_mention_files(vec!["src/main.rs".to_string()]);
        type_str(&mut ed, &format!("好{sep}@sr"));
        let ac = ed
            .autocomplete()
            .unwrap_or_else(|| panic!("`{sep}` must start a mention token"));
        assert_eq!(ac.context, crate::CompletionContext::Mention);
    }
    let mut ed = InputEditor::new();
    ed.set_mention_files(vec!["src/main.rs".to_string()]);
    type_str(&mut ed, "a.@sr");
    assert!(
        ed.autocomplete()
            .is_none_or(|ac| ac.context != crate::CompletionContext::Mention),
        "`.` is not a CJK separator"
    );
}

/// A completed path containing CJK punctuation is quoted (`buildCompletionValue`,
/// `autocomplete.ts:116` @v0.86.0), since the separator would otherwise re-split it.
#[test]
fn a_path_with_cjk_punctuation_is_quoted() {
    let mut ed = InputEditor::new();
    ed.set_mention_files(vec!["a，b.md".to_string()]);
    type_str(&mut ed, "@ab");
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "@\"a，b.md\" ");
}

/// CJK LETTERS are not separators ("CJK letters remain part of words and paths", `utils.ts:57`):
/// a mention directly after an ideograph is not a token start.
#[test]
fn a_cjk_letter_does_not_start_a_mention_token() {
    let mut ed = InputEditor::new();
    ed.set_mention_files(vec!["src/main.rs".to_string()]);
    type_str(&mut ed, "看@sr");
    assert!(
        ed.autocomplete()
            .is_none_or(|ac| ac.context != crate::CompletionContext::Mention),
        "`看@sr` is one token"
    );
}

// ---- TUI-111: @-mention tie-break and the base-directory pass --------------------------------

/// pi v0.84.4 (#8669, `autocomplete.ts:771-784`): equal scores break by depth, then length, then
/// path. `ab/cd/e.rs` and `abzzzzzzzzzzzz.rs` score identically for `ab`; the shallower wins even
/// though it is longer and sorts later.
#[test]
fn equal_score_mentions_break_ties_by_depth_then_length() {
    let mut ed = InputEditor::new();
    ed.set_mention_files(vec![
        "ab/cd/e.rs".to_string(),
        "abzzzzzzzzzzzz.rs".to_string(),
        "abzz.rs".to_string(),
    ]);
    type_str(&mut ed, "@ab");
    let ac = ed.autocomplete().expect("mention popup");
    let labels: Vec<&str> = ac.list.items().iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["abzz.rs", "abzzzzzzzzzzzz.rs", "ab/cd/e.rs"]);
}

/// A bare `@` scores every entry the same upstream (`fdQuery ? … : 1`), so the tie-break is the
/// whole order: the top level first.
#[test]
fn a_bare_mention_lists_the_top_level_first() {
    let mut ed = InputEditor::new();
    ed.set_mention_files(vec![
        "a/b.rs".to_string(),
        "a/".to_string(),
        "zz.rs".to_string(),
    ]);
    type_str(&mut ed, "@");
    let ac = ed.autocomplete().expect("mention popup");
    let labels: Vec<&str> = ac.list.items().iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["a/", "zz.rs", "a/b.rs"]);
}

/// The two `fd` runs merge base-first and de-duplicated BEFORE the cap (`autocomplete.ts:751-759`
/// @v0.84.4), so a top-level entry survives a recursive run that would have truncated it away.
#[test]
fn the_base_directory_pass_survives_the_cap() {
    let merged = crate::autocomplete::merge_base_first(
        vec!["README.md".to_string(), "src/".to_string()],
        vec![
            "deep/a.rs".to_string(),
            "src/".to_string(),
            "deep/b.rs".to_string(),
            "README.md".to_string(),
        ],
        3,
    );
    assert_eq!(merged, ["README.md", "src/", "deep/a.rs"]);
}

/// `fd_list`'s production shape, with the `fd` process swapped for a recording fake: the depth-1
/// pass runs FIRST, the recursive pass second, and the merge keeps the base directory ahead of a
/// recursive run that would otherwise fill the cap (pi `autocomplete.ts:749-759` @v0.84.4).
#[test]
fn fd_list_runs_the_base_pass_first_and_merges_it_ahead_of_the_cap() {
    let mut calls: Vec<Option<String>> = Vec::new();
    let files = crate::autocomplete::fd_list_with(3, |depth| {
        calls.push(depth.map(str::to_string));
        Some(match depth {
            Some(_) => vec!["zz-top.md".to_string()],
            None => vec![
                "a/1.rs".to_string(),
                "a/2.rs".to_string(),
                "a/3.rs".to_string(),
                "zz-top.md".to_string(),
            ],
        })
    })
    .expect("both passes succeed");
    assert_eq!(calls, [Some("1".to_string()), None]);
    assert_eq!(files, ["a/1.rs", "a/2.rs", "zz-top.md"]);
    // Either pass failing falls back to the walk.
    assert!(crate::autocomplete::fd_list_with(3, |d| d.map(|_| Vec::new())).is_none());
}

/// Only the base pass carries `--max-depth 1`; both keep pi's type/follow/hidden/.git flags.
#[test]
fn fd_args_add_max_depth_only_to_the_base_pass() {
    let base = crate::autocomplete::fd_args(Some("1"));
    let recursive = crate::autocomplete::fd_args(None);
    assert!(base.ends_with(&["--max-depth", "1"]));
    assert!(!recursive.contains(&"--max-depth"));
    assert_eq!(base[..base.len() - 2], recursive[..]);
}

/// Applying a completion drops the forced state, as pi's Tab-accept does with
/// `cancelAutocomplete()` (`components/editor.ts` @v0.86.0): the recompute after accepting a FILE
/// from a Tab-opened list is unforced, so the empty trailing token does not reopen a listing of
/// the whole working directory.
#[test]
fn accepting_a_file_from_a_forced_popup_does_not_reopen_a_cwd_listing() {
    let dir = slash_path_fixture();
    let mut ed = InputEditor::new();
    ed.set_cwd(dir.path().to_path_buf());
    ed.set_text("open sr");
    ed.handle_key(&key(KeyCode::Tab));
    let ac = ed.autocomplete().expect("Tab opens the forced file list");
    assert_eq!(ac.list.items().len(), 2);
    ed.handle_key(&key(KeyCode::Down));
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "open srcmap.md ");
    assert!(
        !ed.autocomplete_open(),
        "accepting a file must not reopen the cwd as a forced listing"
    );
}

/// Accepting a DIRECTORY from a forced popup still drills into it: the unforced recompute sees a
/// path-like token.
#[test]
fn accepting_a_directory_from_a_forced_popup_still_drills_down() {
    let dir = slash_path_fixture();
    std::fs::write(dir.path().join("src").join("main.rs"), "").unwrap();
    let mut ed = InputEditor::new();
    ed.set_cwd(dir.path().to_path_buf());
    ed.set_text("open sr");
    ed.handle_key(&key(KeyCode::Tab));
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "open src/");
    let ac = ed
        .autocomplete()
        .expect("the directory's contents are offered");
    assert_eq!(ac.context, crate::CompletionContext::Path);
}

/// The single-match auto-apply leaves no forced popup behind (pi's auto-apply returns without
/// setting a popup state), so the next word typed is not re-asked as a forced file request.
#[test]
fn a_single_match_auto_apply_leaves_no_forced_popup_behind() {
    let dir = slash_path_fixture();
    let mut ed = InputEditor::new();
    ed.set_cwd(dir.path().to_path_buf());
    ed.set_text("open srcm");
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "open srcmap.md ");
    assert!(!ed.autocomplete_open());
    type_str(&mut ed, "s");
    assert!(
        !ed.autocomplete_open(),
        "prose after an auto-applied file must not pop a path list"
    );
}
