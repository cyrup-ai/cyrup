//! Tests for the `ui` module: port of the behaviour of pi's
//! `packages/coding-agent/src/extensions/llama/ui.ts` (the `/llama` overlay components,
//! `HuggingFaceSearch`, `LlamaView` and `runWithProgress`).
//!
//! The view is a pure state machine, so almost everything here drives [`LlamaView`] directly:
//! keys in, `render(width)` out, a [`ManualClock`] for the 500 ms debounce and a scripted search
//! function standing in for the Hugging Face client. The few tests that need the real bridge
//! ([`LlamaOverlay`] + [`LlamaOverlayUi`] + [`show_llama_ui`]) use a fake [`HostServices`] whose
//! `open_overlay` is a paint-and-key loop on a blocking thread, the way the TUI drives it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_ext::host::{
    HostServices, InteractiveOverlay, NotifyKind, OverlayColor, OverlayKey, OverlayKeyCode,
    OverlayLine, OverlayMouse, OverlayMouseOutcome, ThemeRole,
};
use serde_json::json;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::fake_server::{model_with, model_with_status};
use crate::client::{LlamaModelInfo, LlamaProgress, ProgressField};
use crate::error::LlamaError;
use crate::huggingface::HuggingFaceModel;
use crate::ui::{
    Binding, Clock, ConnectionChoice, LlamaKeys, LlamaManagerAction, LlamaUi, LlamaUiOutcome,
    LlamaView, ProgressOptions, ProgressOutcome, ProgressState, SearchFn, TextInput, compact_count,
    context_label, fuzzy_filter, fuzzy_match, llama_overlay, locale_compare, model_description,
    progress_bar, root_collator_loads, run_with_progress, search_fn, show_llama_ui, to_fixed,
    truncate_to_width, visible_width, wrap_ranges,
};

// ------------------------------------------------------------------------------------- helpers --

/// A clock tests move by hand.
struct ManualClock(AtomicU64);

impl ManualClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(0)))
    }
    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn press(code: OverlayKeyCode) -> OverlayKey {
    OverlayKey::plain(code)
}
fn chr(c: char) -> OverlayKey {
    OverlayKey::plain(OverlayKeyCode::Char(c))
}
fn ctrl(c: char) -> OverlayKey {
    OverlayKey::ctrl(OverlayKeyCode::Char(c))
}
fn alt(c: char) -> OverlayKey {
    OverlayKey {
        code: OverlayKeyCode::Char(c),
        ctrl: false,
        alt: true,
        shift: false,
    }
}
fn escape() -> OverlayKey {
    press(OverlayKeyCode::Escape)
}
fn enter() -> OverlayKey {
    press(OverlayKeyCode::Enter)
}
fn down() -> OverlayKey {
    press(OverlayKeyCode::Down)
}
fn up() -> OverlayKey {
    press(OverlayKeyCode::Up)
}

fn type_text(view: &mut LlamaView, text: &str) {
    for c in text.chars() {
        view.handle_key(&chr(c));
    }
}

fn new_view() -> (LlamaView, Arc<ManualClock>) {
    new_view_with(LlamaKeys::default())
}

fn new_view_with(keys: LlamaKeys) -> (LlamaView, Arc<ManualClock>) {
    let clock = ManualClock::new();
    let view = LlamaView::new(
        Arc::new(keys),
        clock.clone(),
        tokio::runtime::Handle::current(),
    );
    (view, clock)
}

/// The rendered frame as plain text, trailing padding trimmed.
fn snapshot(view: &mut LlamaView, width: usize) -> Vec<String> {
    plain(&view.render(width))
}

fn plain(lines: &[OverlayLine]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.plain_text().trim_end().to_string())
        .collect()
}

fn rule(width: usize) -> String {
    "─".repeat(width)
}

fn user_keys(bindings: serde_json::Value) -> LlamaKeys {
    let entries: Vec<(String, serde_json::Value)> = bindings
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    LlamaKeys::from_user_bindings(&entries)
}

fn info(id: &str, status: &str) -> LlamaModelInfo {
    serde_json::from_value(model_with(id, status, json!({}))).unwrap()
}

fn info_with(id: &str, status: &str, extra: serde_json::Value) -> LlamaModelInfo {
    serde_json::from_value(model_with(id, status, extra)).unwrap()
}

fn info_with_args(id: &str, status: &str, args: &[&str]) -> LlamaModelInfo {
    serde_json::from_value(model_with_status(id, status, json!({ "args": args }))).unwrap()
}

fn hf(id: &str, downloads: f64) -> HuggingFaceModel {
    HuggingFaceModel {
        id: id.to_string(),
        downloads,
    }
}

/// Tick, let spawned tasks run, tick again; `true` when the host would repaint. Two ticks: the first
/// fires a due debounce (which spawns the search task), the yields let that task run, the second
/// drains its answer, the order the host's periodic `tick` produces over two cycles.
async fn settle(view: &mut LlamaView) -> bool {
    let mut changed = view.tick();
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    changed |= view.tick();
    changed
}

type SearchReply = Result<Vec<HuggingFaceModel>, LlamaError>;

enum Reply {
    Now(SearchReply),
    Gate(oneshot::Receiver<SearchReply>),
}

/// A scripted `search(query, signal)`: records every call and answers from a queue.
struct FakeSearch {
    calls: Mutex<Vec<(String, CancellationToken)>>,
    replies: Mutex<VecDeque<Reply>>,
}

impl FakeSearch {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            replies: Mutex::new(VecDeque::new()),
        })
    }

    fn reply(&self, reply: Reply) {
        self.replies.lock().unwrap().push_back(reply);
    }

    fn reply_ok(&self, models: Vec<HuggingFaceModel>) {
        self.reply(Reply::Now(Ok(models)));
    }

    fn queries(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(query, _)| query.clone())
            .collect()
    }

    fn token(&self, call: usize) -> CancellationToken {
        self.calls.lock().unwrap()[call].1.clone()
    }

    fn as_fn(self: &Arc<Self>) -> SearchFn {
        let this = Arc::clone(self);
        search_fn(move |query, signal| {
            this.calls.lock().unwrap().push((query, signal));
            let reply = this.replies.lock().unwrap().pop_front();
            async move {
                match reply {
                    Some(Reply::Now(result)) => result,
                    Some(Reply::Gate(gate)) => gate.await.unwrap_or(Err(LlamaError::Cancelled)),
                    None => Ok(Vec::new()),
                }
            }
        })
    }
}

// =================================================================================================
// Model list text (`ui.ts:32-52`)
// =================================================================================================

/// `contextLabel` (`ui.ts:32-42`): `n_ctx` wins over `n_ctx_train`, thousands round half up with a
/// `k`, `--ctx-size`/`-c`/`-ctx` arguments are the fallback, and nothing usable is `None`.
#[test]
fn context_label_table() {
    let label = |model: &LlamaModelInfo| context_label(model);
    assert_eq!(
        label(&info_with("m", "loaded", json!({"meta": {"n_ctx": 32768}}))).as_deref(),
        Some("33k")
    );
    assert_eq!(
        label(&info_with(
            "m",
            "loaded",
            json!({"meta": {"n_ctx": 4096, "n_ctx_train": 131072}})
        ))
        .as_deref(),
        Some("4k"),
        "n_ctx wins over n_ctx_train"
    );
    assert_eq!(
        label(&info_with(
            "m",
            "loaded",
            json!({"meta": {"n_ctx_train": 131072}})
        ))
        .as_deref(),
        Some("131k")
    );
    assert_eq!(
        label(&info_with("m", "loaded", json!({"meta": {"n_ctx": 512}}))).as_deref(),
        Some("512"),
        "under 1000 is printed whole"
    );
    assert_eq!(
        label(&info_with("m", "loaded", json!({"meta": {"n_ctx": 1500}}))).as_deref(),
        Some("2k"),
        "Math.round rounds .5 up"
    );
    assert_eq!(
        label(&info_with("m", "loaded", json!({"meta": {"n_ctx": 1499}}))).as_deref(),
        Some("1k")
    );
    // A zero `n_ctx` is falsy upstream (`if (context)`), so the args are consulted.
    assert_eq!(
        label(&info_with("m", "loaded", json!({"meta": {"n_ctx": 0}}))),
        None
    );
    assert_eq!(
        label(&info_with_args("m", "loaded", &["--ctx-size", "8192"])).as_deref(),
        Some("8k")
    );
    assert_eq!(
        label(&info_with_args("m", "loaded", &["-c", "512"])).as_deref(),
        Some("512")
    );
    assert_eq!(
        label(&info_with_args("m", "loaded", &["-ctx", "16384"])).as_deref(),
        Some("16k")
    );
    assert_eq!(
        label(&info_with_args(
            "m",
            "loaded",
            &["--model", "x.gguf", "-c", "2048"]
        ))
        .as_deref(),
        Some("2k")
    );
    assert_eq!(
        label(&info_with_args(
            "m",
            "loaded",
            &["-c", "abc", "--ctx-size", "0"]
        )),
        None,
        "non-numeric and non-positive values are skipped"
    );
    assert_eq!(
        label(&info_with_args("m", "loaded", &["--ctx-size"])),
        None,
        "a flag with no value is skipped"
    );
    assert_eq!(
        label(&info_with_args("m", "loaded", &["-c", "0x1000"])).as_deref(),
        Some("4k"),
        "Number('0x1000') is 4096"
    );
    assert_eq!(
        label(&info_with_args("m", "loaded", &["-c", "1500"])).as_deref(),
        Some("2k"),
        "the argument fallback rounds half up too (Math.round)"
    );
    assert_eq!(
        label(&info_with_args("m", "loaded", &["-c", "1499"])).as_deref(),
        Some("1k")
    );
    assert_eq!(label(&info("m", "loaded")), None);
}

/// `modelDescription` (`ui.ts:44-52`): `loaded`/`sleeping` read `loaded · <ctx> context`; every
/// other non-`unloaded` status prints its own name, with no context.
#[test]
fn model_description_table() {
    let ctx = json!({"meta": {"n_ctx": 32000}});
    assert_eq!(
        model_description(&info_with("m", "loaded", ctx.clone())),
        "loaded · 32k context"
    );
    assert_eq!(
        model_description(&info_with("m", "sleeping", ctx.clone())),
        "loaded · 32k context"
    );
    assert_eq!(model_description(&info("m", "loaded")), "loaded");
    assert_eq!(model_description(&info("m", "unloaded")), "");
    assert_eq!(
        model_description(&info_with("m", "loading", ctx.clone())),
        "loading",
        "a non-loaded model shows its status and no context"
    );
    assert_eq!(model_description(&info("m", "downloading")), "downloading");
    assert_eq!(
        model_description(&info("m", "paused")),
        "paused",
        "an unknown status prints verbatim"
    );
}

// =================================================================================================
// The model list (`showModels`, `ui.ts:321-358`)
// =================================================================================================

fn three_models() -> Vec<LlamaModelInfo> {
    vec![
        info("zeta", "unloaded"),
        info("alpha", "unloaded"),
        info_with("mid", "loaded", json!({"meta": {"n_ctx": 32000}})),
    ]
}

/// The frame at 60 columns: border, bold title, server url, blank, rows (`→` on the selected one,
/// description in a second column from column 38, truncated to fit), blank, footer, border.
#[tokio::test]
async fn models_view_snapshot_at_60_columns() {
    let (mut view, _clock) = new_view();
    let _rx = view.show_models("http://127.0.0.1:8080", &three_models());
    let expected = vec![
        rule(60),
        " llama.cpp models".to_string(),
        " http://127.0.0.1:8080".to_string(),
        String::new(),
        format!("→ mid{}loaded · 32k context", " ".repeat(33)),
        "  alpha".to_string(),
        "  zeta".to_string(),
        format!("  Download model…{}Hugging Face owner/r", " ".repeat(21)),
        String::new(),
        " enter load/unload/download • escape/ctrl+c close".to_string(),
        rule(60),
    ];
    assert_eq!(snapshot(&mut view, 60), expected);
}

/// A frame of 40 columns or fewer drops the description column (`select-list.ts`: `width > 40`),
/// one column wider shows it.
#[tokio::test]
async fn model_descriptions_need_more_than_forty_columns() {
    let (mut view, _clock) = new_view();
    let _rx = view.show_models("http://h", &three_models());
    let narrow = snapshot(&mut view, 40);
    assert!(narrow.contains(&"→ mid".to_string()), "{narrow:#?}");
    assert!(
        !narrow.iter().any(|l| l.contains("loaded")),
        "no description at 40 columns: {narrow:#?}"
    );
    let wide = snapshot(&mut view, 60);
    assert!(
        wide.iter().any(|l| l.contains("loaded · 32k context")),
        "{wide:#?}"
    );
}

/// The same frame at 100 columns: the description column is wider so nothing is truncated.
#[tokio::test]
async fn models_view_snapshot_at_100_columns() {
    let (mut view, _clock) = new_view();
    let _rx = view.show_models("http://127.0.0.1:8080", &three_models());
    let expected = vec![
        rule(100),
        " llama.cpp models".to_string(),
        " http://127.0.0.1:8080".to_string(),
        String::new(),
        format!("→ mid{}loaded · 32k context", " ".repeat(33)),
        "  alpha".to_string(),
        "  zeta".to_string(),
        format!(
            "  Download model…{}Hugging Face owner/repository[:quant]",
            " ".repeat(21)
        ),
        String::new(),
        " enter load/unload/download • escape/ctrl+c close".to_string(),
        rule(100),
    ];
    assert_eq!(snapshot(&mut view, 100), expected);
}

/// Styling of the list: the border and the selected row are accent (cyan), the title is bold
/// accent, the footer key text is dim and its description muted, unselected descriptions are muted.
#[tokio::test]
async fn models_view_styles() {
    let (mut view, _clock) = new_view();
    let _rx = view.show_models("http://h", &three_models());
    let lines = view.render(100);
    let border = lines.first().unwrap();
    assert_eq!(
        border.spans[0].fg,
        Some(OverlayColor::Theme(ThemeRole::Accent))
    );
    let title = &lines[1];
    let title_span = title
        .spans
        .iter()
        .find(|s| s.text == "llama.cpp models")
        .unwrap();
    assert!(title_span.bold);
    assert_eq!(title_span.fg, Some(OverlayColor::Theme(ThemeRole::Accent)));
    let selected = &lines[4];
    assert_eq!(selected.spans.len(), 1);
    assert_eq!(
        selected.spans[0].fg,
        Some(OverlayColor::Theme(ThemeRole::Accent))
    );
    let download = &lines[7];
    assert_eq!(
        download.spans[0].fg, None,
        "an unselected label is unstyled"
    );
    assert_eq!(
        download.spans[1].fg,
        Some(OverlayColor::Theme(ThemeRole::Muted)),
        "its description is muted"
    );
    let footer = &lines[9];
    let key = footer.spans.iter().find(|s| s.text == "enter").unwrap();
    assert_eq!(key.fg, Some(OverlayColor::Theme(ThemeRole::Dim)));
    let described = footer
        .spans
        .iter()
        .find(|s| s.text == " load/unload/download")
        .unwrap();
    assert_eq!(described.fg, Some(OverlayColor::Theme(ThemeRole::Muted)));
}

/// Loaded models first (only `loaded`, not `sleeping`), then alphabetical regardless of case
/// (`ui.ts:322-325`).
#[tokio::test]
async fn models_sorted_loaded_first_then_alphabetical() {
    let (mut view, _clock) = new_view();
    let models = vec![
        info("beta", "unloaded"),
        info("Zulu", "loaded"),
        info("alpha", "sleeping"),
        info("Alpha2", "unloaded"),
        info("able", "loaded"),
    ];
    let _rx = view.show_models("http://h", &models);
    let rows: Vec<String> = snapshot(&mut view, 100)
        .into_iter()
        .filter(|line| line.starts_with("→ ") || line.starts_with("  "))
        .map(|line| {
            let rest: String = line.chars().skip(2).collect();
            rest.split("  ").next().unwrap().trim().to_string()
        })
        .collect();
    assert_eq!(
        rows,
        vec!["able", "Zulu", "alpha", "Alpha2", "beta", "Download model…"],
        "loaded (able, Zulu) first by id, then sleeping/unloaded alphabetically"
    );
}

/// `localeCompare` (`ui.ts:324`): the expected order is node 22.22.0 (ICU 77.1, CLDR 47)
/// `[...ids].sort((a, b) => a.localeCompare(b))` of this exact list, which mixes case, accents,
/// ligatures, `ß`, digits, punctuation, Hangul, kana, Bopomofo and Han from the URO, Extension A,
/// Extension B and the compatibility block. ICU root orders Han radical-stroke (`一 𠀀 㐀 丽 中 乙`),
/// not by code point.
#[test]
fn locale_compare_sorts_accented_and_cjk_ids_as_node_does() {
    assert!(
        root_collator_loads(),
        "the embedded root collation data loads"
    );
    let mut ids = vec![
        "zeta",
        "Zeta",
        "éclair",
        "eclair",
        "Eclair",
        "ÉCLAIR",
        "ecl",
        "model-2",
        "model-10",
        "model_2",
        "model.2",
        "Model-2",
        "qwen3:8b",
        "Qwen3-8B",
        "ünicode",
        "unicode",
        "Ångström",
        "angstrom",
        "straße",
        "strasse",
        "한국어",
        "ひらがな",
        "カタカナ",
        "ㄅㄆ",
        "1abc",
        "_abc",
        "-abc",
        "~abc",
        "abc",
        "ABC",
        "aBc",
        "ø",
        "o",
        "z",
        "œuvre",
        "oe",
        "ﬁle",
        "file",
        "½",
        "2",
        "10",
        "中",
        "𠀀",
        "㐀",
        "一",
        "日本語",
        "中文",
        "Qwen-中文",
        "汉字",
        "丽",
        "乙",
        "漢字",
        "模型",
        "千问",
        "通义千问",
        "Qwen2.5-中文",
        "豈",
        "々",
    ];
    ids.sort_by(|a, b| locale_compare(a, b));
    assert_eq!(
        ids,
        vec![
            "_abc",
            "-abc",
            "~abc",
            "々",
            "½",
            "10",
            "1abc",
            "2",
            "abc",
            "aBc",
            "ABC",
            "angstrom",
            "Ångström",
            "ecl",
            "eclair",
            "Eclair",
            "éclair",
            "ÉCLAIR",
            "file",
            "ﬁle",
            "model_2",
            "model-10",
            "model-2",
            "Model-2",
            "model.2",
            "o",
            "ø",
            "oe",
            "œuvre",
            "Qwen-中文",
            "Qwen2.5-中文",
            "Qwen3-8B",
            "qwen3:8b",
            "strasse",
            "straße",
            "unicode",
            "ünicode",
            "z",
            "zeta",
            "Zeta",
            "한국어",
            "カタカナ",
            "ひらがな",
            "ㄅㄆ",
            "一",
            "𠀀",
            "㐀",
            "丽",
            "中",
            "中文",
            "乙",
            "千问",
            "日本語",
            "模型",
            "汉字",
            "漢字",
            "豈",
            "通义千问"
        ]
    );
    // node: `"é".localeCompare("é")`, `"x".localeCompare("x­")` and
    // `"ab".localeCompare("a​b")` are all 0; the stable sort keeps such ids in input order.
    assert_eq!(
        locale_compare("\u{e9}", "e\u{301}"),
        std::cmp::Ordering::Equal
    );
    assert_eq!(locale_compare("x", "x\u{ad}"), std::cmp::Ordering::Equal);
    assert_eq!(
        locale_compare("ab", "a\u{200b}b"),
        std::cmp::Ordering::Equal
    );
}

/// The hand-checked orders the approximation this replaced was written against still hold.
#[test]
fn locale_compare_orders_like_icu_for_model_ids() {
    use std::cmp::Ordering::{Greater, Less};
    assert_eq!(locale_compare("alpha", "beta"), Less);
    assert_eq!(
        locale_compare("Beta", "alpha"),
        Greater,
        "case does not decide the order"
    );
    assert_eq!(locale_compare("a", "A"), Less, "lowercase first on a tie");
    assert_eq!(
        locale_compare("a-b", "a_b"),
        Greater,
        "'_' sorts before '-'"
    );
    assert_eq!(locale_compare("a1", "ab"), Less, "digits before letters");
    assert_eq!(
        locale_compare("a/b", "a1"),
        Less,
        "punctuation before digits"
    );
    assert_eq!(locale_compare("ab", "abc"), Less, "a prefix sorts first");
}

/// The three answers of the list: Enter on a model row, Enter on the trailing download row, and
/// Escape (`ui.ts:340-347`). Navigation wraps in both directions.
#[tokio::test]
async fn models_keys_resolve_to_actions() {
    let (mut view, _clock) = new_view();
    let mut rx = view.show_models("http://h", &three_models());
    view.handle_key(&enter());
    assert_eq!(
        rx.try_recv().unwrap(),
        LlamaManagerAction::Model(Box::new(info_with(
            "mid",
            "loaded",
            json!({"meta": {"n_ctx": 32000}})
        )))
    );

    let mut rx = view.show_models("http://h", &three_models());
    view.handle_key(&down());
    view.handle_key(&down());
    view.handle_key(&enter());
    match rx.try_recv().unwrap() {
        LlamaManagerAction::Model(model) => assert_eq!(model.id, "zeta"),
        other => panic!("expected a model, got {other:?}"),
    }

    // Up from the first row wraps to the last: the download row.
    let mut rx = view.show_models("http://h", &three_models());
    view.handle_key(&up());
    view.handle_key(&enter());
    assert_eq!(rx.try_recv().unwrap(), LlamaManagerAction::Download);

    // Down from the last row wraps to the first.
    let mut rx = view.show_models("http://h", &three_models());
    for _ in 0..4 {
        view.handle_key(&down());
    }
    view.handle_key(&enter());
    match rx.try_recv().unwrap() {
        LlamaManagerAction::Model(model) => assert_eq!(model.id, "mid"),
        other => panic!("expected a model, got {other:?}"),
    }

    let mut rx = view.show_models("http://h", &three_models());
    view.handle_key(&escape());
    assert_eq!(rx.try_recv().unwrap(), LlamaManagerAction::Close);

    let mut rx = view.show_models("http://h", &three_models());
    view.handle_key(&ctrl('c'));
    assert_eq!(
        rx.try_recv().unwrap(),
        LlamaManagerAction::Close,
        "ctrl+c is the second default cancel key"
    );
}

/// Only the first answer counts (a promise resolves once): a second Enter after the first does not
/// change what was delivered.
#[tokio::test]
async fn models_answer_is_delivered_once() {
    let (mut view, _clock) = new_view();
    let mut rx = view.show_models("http://h", &three_models());
    view.handle_key(&enter());
    view.handle_key(&down());
    view.handle_key(&escape());
    match rx.try_recv().unwrap() {
        LlamaManagerAction::Model(model) => assert_eq!(model.id, "mid"),
        other => panic!("expected the first answer, got {other:?}"),
    }
}

/// The list walks the configured `tui.select.*` keys, not hard-coded ones (`ui.ts:244-264` goes
/// through `KeybindingsManager.matches`), and the footer names them (`keyHint`).
#[tokio::test]
async fn models_use_the_configured_select_keybindings() {
    let keys = user_keys(json!({
        "tui.select.down": ["j"],
        "tui.select.confirm": "ctrl+y",
    }));
    let (mut view, _clock) = new_view_with(keys);
    let mut rx = view.show_models("http://h", &three_models());
    view.handle_key(&down());
    view.handle_key(&enter());
    assert!(
        rx.try_recv().is_err(),
        "the default keys no longer act once rebound"
    );
    view.handle_key(&chr('j'));
    view.handle_key(&ctrl('y'));
    match rx.try_recv().unwrap() {
        LlamaManagerAction::Model(model) => assert_eq!(model.id, "alpha"),
        other => panic!("expected alpha, got {other:?}"),
    }
    let _rx = view.show_models("http://h", &three_models());
    let footer = snapshot(&mut view, 100)[9].clone();
    assert_eq!(footer, " ctrl+y load/unload/download • escape/ctrl+c close");
}

/// An explicitly empty list unbinds the action, and the hint loses its key text (`keyText` of no
/// keys is `""`).
#[tokio::test]
async fn an_empty_binding_list_unbinds_cancel() {
    let (mut view, _clock) = new_view_with(user_keys(json!({ "tui.select.cancel": [] })));
    let mut rx = view.show_models("http://h", &three_models());
    view.handle_key(&escape());
    view.handle_key(&ctrl('c'));
    assert!(rx.try_recv().is_err());
    assert_eq!(
        snapshot(&mut view, 100)[9],
        " enter load/unload/download •  close"
    );
}

/// `from_agent_dir` reads `keybindings.json` (migrating legacy ids) and a missing or malformed file
/// is the defaults.
#[test]
fn keys_load_from_the_agent_dir() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        LlamaKeys::from_agent_dir(dir.path())
            .matches(Binding::SelectUp, &press(OverlayKeyCode::Up))
    );

    std::fs::write(
        dir.path().join("keybindings.json"),
        r#"{"tui.select.up": "ctrl+p", "tui.select.cancel": ["escape"]}"#,
    )
    .unwrap();
    let keys = LlamaKeys::from_agent_dir(dir.path());
    assert!(keys.matches(Binding::SelectUp, &ctrl('p')));
    assert!(!keys.matches(Binding::SelectUp, &press(OverlayKeyCode::Up)));
    assert_eq!(keys.key_text(Binding::SelectCancel), "escape");
    assert_eq!(
        keys.key_text(Binding::SelectDown),
        "down",
        "an unmentioned id keeps its default"
    );

    // Legacy ids are migrated exactly as the TUI does (`selectUp` is `tui.select.up`).
    std::fs::write(
        dir.path().join("keybindings.json"),
        r#"{"selectUp": "ctrl+p"}"#,
    )
    .unwrap();
    let keys = LlamaKeys::from_agent_dir(dir.path());
    assert!(keys.matches(Binding::SelectUp, &ctrl('p')));
    assert!(!keys.matches(Binding::SelectUp, &press(OverlayKeyCode::Up)));

    std::fs::write(dir.path().join("keybindings.json"), "{ not json").unwrap();
    assert!(
        LlamaKeys::from_agent_dir(dir.path())
            .matches(Binding::SelectUp, &press(OverlayKeyCode::Up))
    );
    // A document that is not an object is no document: the defaults, not an empty table.
    std::fs::write(dir.path().join("keybindings.json"), "[1, 2]").unwrap();
    assert!(
        LlamaKeys::from_agent_dir(dir.path())
            .matches(Binding::SelectUp, &press(OverlayKeyCode::Up))
    );
}

// =================================================================================================
// select / confirm / connection error (`ui.ts:360-388`)
// =================================================================================================

/// The generic select frame: title, blank, options, blank, `enter select • escape/ctrl+c cancel`.
#[tokio::test]
async fn select_view_snapshots() {
    let (mut view, _clock) = new_view();
    let options = vec![
        "Unload all and load".to_string(),
        "Keep loaded".to_string(),
        "Cancel".to_string(),
    ];
    let _rx = view.show_select("Another model is loaded", &options);
    for width in [40usize, 80] {
        let expected = vec![
            rule(width),
            " Another model is loaded".to_string(),
            String::new(),
            "→ Unload all and load".to_string(),
            "  Keep loaded".to_string(),
            "  Cancel".to_string(),
            String::new(),
            " enter select • escape/ctrl+c cancel".to_string(),
            rule(width),
        ];
        assert_eq!(snapshot(&mut view, width), expected, "width {width}");
    }
}

#[tokio::test]
async fn select_resolves_to_the_option_or_none() {
    let (mut view, _clock) = new_view();
    let options = vec!["a".to_string(), "b".to_string(), "c".to_string()];

    let mut rx = view.show_select("t", &options);
    view.handle_key(&down());
    view.handle_key(&enter());
    assert_eq!(rx.try_recv().unwrap(), Some("b".to_string()));

    let mut rx = view.show_select("t", &options);
    view.handle_key(&up());
    view.handle_key(&enter());
    assert_eq!(
        rx.try_recv().unwrap(),
        Some("c".to_string()),
        "up from the top wraps"
    );

    let mut rx = view.show_select("t", &options);
    view.handle_key(&escape());
    assert_eq!(rx.try_recv().unwrap(), None);
}

/// A long option list shows 12 rows and the `(i/N)` indicator, centred on the selection.
#[tokio::test]
async fn select_windows_at_twelve_rows() {
    let (mut view, _clock) = new_view();
    let options: Vec<String> = (0..20).map(|i| format!("option-{i:02}")).collect();
    let _rx = view.show_select("t", &options);
    let lines = snapshot(&mut view, 60);
    let rows = lines.iter().filter(|l| l.contains("option-")).count();
    assert_eq!(rows, 12);
    assert!(lines.contains(&"  (1/20)".to_string()));
    for _ in 0..10 {
        view.handle_key(&down());
    }
    let lines = snapshot(&mut view, 60);
    assert!(lines.contains(&"  (11/20)".to_string()));
    assert!(lines.contains(&"→ option-10".to_string()));
    assert!(
        !lines.iter().any(|l| l.contains("option-00")),
        "the window scrolled"
    );
    // Centred on the selection: option 10 of 20 shows options 04..=15, not 10..=19.
    assert!(lines.contains(&"  option-04".to_string()), "{lines:#?}");
    assert!(!lines.contains(&"  option-03".to_string()), "{lines:#?}");
    assert!(lines.contains(&"  option-15".to_string()), "{lines:#?}");
    assert!(!lines.contains(&"  option-16".to_string()), "{lines:#?}");
}

/// `confirm(title, message)` is a Yes/No select titled `title\nmessage` (`ui.ts:381-383`); only
/// `Yes` is true. `connectionError` is a Retry/Close select (`ui.ts:385-388`).
#[tokio::test]
async fn confirm_and_connection_error_through_the_bridge() {
    let (overlay, ui, _attached) = llama_overlay(
        Arc::new(LlamaKeys::default()),
        ManualClock::new(),
        tokio::runtime::Handle::current(),
    );
    let mut overlay = overlay;

    // Yes.
    let mut confirm = Box::pin(ui.confirm("Unload model?", "qwen"));
    assert!(futures::poll!(confirm.as_mut()).is_pending());
    let frame = plain(&overlay.render(60, 24));
    assert_eq!(frame[1], " Unload model?");
    assert_eq!(frame[2], " qwen");
    assert_eq!(frame[4], "→ Yes");
    assert_eq!(frame[5], "  No");
    overlay.handle_key(enter());
    assert!(confirm.await);

    // No.
    let mut confirm = Box::pin(ui.confirm("Unload model?", "qwen"));
    assert!(futures::poll!(confirm.as_mut()).is_pending());
    overlay.handle_key(down());
    overlay.handle_key(enter());
    assert!(!confirm.await);

    // Cancel is a No.
    let mut confirm = Box::pin(ui.confirm("Unload model?", "qwen"));
    assert!(futures::poll!(confirm.as_mut()).is_pending());
    overlay.handle_key(escape());
    assert!(!confirm.await);

    // Connection error: title, url, blank line, message; Retry first.
    let mut error = Box::pin(ui.connection_error("http://127.0.0.1:8080", "fetch failed"));
    assert!(futures::poll!(error.as_mut()).is_pending());
    let frame = plain(&overlay.render(60, 24));
    assert_eq!(frame[1], " llama.cpp unavailable");
    assert_eq!(frame[2], " http://127.0.0.1:8080");
    assert_eq!(
        frame[3], "",
        "the blank line of the title is kept (padded to the width upstream)"
    );
    assert_eq!(frame[4], " fetch failed");
    assert_eq!(frame[6], "→ Retry");
    assert_eq!(frame[7], "  Close");
    overlay.handle_key(enter());
    assert_eq!(error.await, ConnectionChoice::Retry);

    let mut error = Box::pin(ui.connection_error("http://h", "m"));
    assert!(futures::poll!(error.as_mut()).is_pending());
    overlay.handle_key(down());
    overlay.handle_key(enter());
    assert_eq!(error.await, ConnectionChoice::Close);

    let mut error = Box::pin(ui.connection_error("http://h", "m"));
    assert!(futures::poll!(error.as_mut()).is_pending());
    overlay.handle_key(escape());
    assert_eq!(error.await, ConnectionChoice::Close);
}

// =================================================================================================
// Loading and status frames (`ui.ts:293`, `:415-417`)
// =================================================================================================

#[tokio::test]
async fn loading_and_status_snapshots() {
    let (mut view, _clock) = new_view();
    for width in [30usize, 70] {
        // `Text("Loading…", 1, 1)`: one blank padded row above and below.
        assert_eq!(
            snapshot(&mut view, width),
            vec![
                rule(width),
                " llama.cpp models".into(),
                String::new(),
                " Loading…".into(),
                String::new(),
                rule(width)
            ],
            "width {width}"
        );
    }
    view.show_status("Loading model details", "owner/repo");
    for width in [30usize, 70] {
        assert_eq!(
            snapshot(&mut view, width),
            vec![
                rule(width),
                " Loading model details".into(),
                String::new(),
                " owner/repo".into(),
                rule(width)
            ],
            "no footer on a status frame; width {width}"
        );
    }
}

/// A status message longer than the frame wraps at word boundaries and breaks an over-long word.
#[tokio::test]
async fn status_wraps_to_the_width() {
    let (mut view, _clock) = new_view();
    view.show_status("T", "one two three four five six");
    let lines = snapshot(&mut view, 14);
    assert_eq!(
        lines,
        vec![
            rule(14),
            " T".into(),
            String::new(),
            " one two".into(),
            " three four".into(),
            " five six".into(),
            rule(14)
        ],
        "content width is the frame width less one column of margin each side"
    );
    view.show_status("T", "abcdefghijklmnop");
    let lines = snapshot(&mut view, 10);
    assert_eq!(lines[3], " abcdefgh");
    assert_eq!(lines[4], " ijklmnop");
}

/// A line wider than the view is cut to it (`ui.ts:469-473`), including a border (`width` cells).
#[tokio::test]
async fn render_never_exceeds_the_width() {
    let (mut view, _clock) = new_view();
    let _rx = view.show_models(
        "http://a-very-long-server-url.example.invalid:8080/with/a/path",
        &three_models(),
    );
    for width in [3usize, 10, 30, 45, 60, 120] {
        for line in view.render(width) {
            assert!(
                visible_width(&line.plain_text()) <= width,
                "width {width}: {:?}",
                line.plain_text()
            );
        }
    }
    // A wide character in a one-column frame wraps to a line wider than the frame; the view cuts it
    // (`ui.ts:469-473`).
    view.show_status("T", "日本語");
    for width in [1usize, 2] {
        for line in view.render(width) {
            assert!(
                visible_width(&line.plain_text()) <= width,
                "width {width}: {:?}",
                line.plain_text()
            );
        }
    }
}

// =================================================================================================
// Progress (`ui.ts:419-455`, `:432-451`)
// =================================================================================================

/// The 40-cell bar (`ui.ts:438-445`): fill rounds half up, is clamped to `[0, 1]`, and the percentage
/// is `Math.round(ratio * 100)` of the *unclamped* ratio.
#[test]
fn progress_bar_math() {
    let bar = |filled: usize, percent: &str| {
        format!(
            "{}{} {percent}",
            "█".repeat(filled),
            "─".repeat(40 - filled)
        )
    };
    assert_eq!(progress_bar(0.0), bar(0, "0%"));
    assert_eq!(progress_bar(0.5), bar(20, "50%"));
    assert_eq!(progress_bar(1.0), bar(40, "100%"));
    assert_eq!(progress_bar(0.333), bar(13, "33%"));
    assert_eq!(
        progress_bar(0.0125),
        bar(1, "1%"),
        "0.0125 * 40 is exactly .5 and rounds up"
    );
    assert_eq!(progress_bar(0.0124), bar(0, "1%"));
    assert_eq!(progress_bar(0.005), bar(0, "1%"), "0.5% rounds to 1%");
    assert_eq!(
        progress_bar(1.5),
        bar(40, "150%"),
        "fill clamps, the percentage does not"
    );
    assert_eq!(progress_bar(-0.2), bar(0, "-20%"));
    assert_eq!(
        progress_bar(0.99),
        bar(40, "99%"),
        "39.6 cells rounds to 40"
    );
    assert_eq!(progress_bar(f64::NAN), " NaN%", "NaN repeats nothing");
}

fn state(ratio: Option<f64>, detail: Option<&str>) -> ProgressState {
    ProgressState {
        title: "Downloading model".into(),
        model: "owner/repo:Q4_K_M".into(),
        message: "Downloading model".into(),
        ratio,
        detail: detail.map(str::to_string),
    }
}

/// The progress frame: title, model line, blank, message, bar with percent, dim detail, a
/// `escape/ctrl+c stop` footer.
#[tokio::test]
async fn progress_view_snapshots() {
    let (mut view, _clock) = new_view();
    let _waiter = view.show_progress(&state(Some(0.5), Some("512 B / 1.00 KiB")));
    for width in [60usize, 100] {
        let expected = vec![
            rule(width),
            " Downloading model".to_string(),
            " owner/repo:Q4_K_M".to_string(),
            String::new(),
            " Downloading model".to_string(),
            format!(" {}{} 50%", "█".repeat(20), "─".repeat(20)),
            " 512 B / 1.00 KiB".to_string(),
            String::new(),
            " escape/ctrl+c stop".to_string(),
            rule(width),
        ];
        assert_eq!(snapshot(&mut view, width), expected, "width {width}");
    }
    // No ratio: no bar line. No detail: no detail line.
    view.update_progress(&state(None, None));
    assert_eq!(
        snapshot(&mut view, 60),
        vec![
            rule(60),
            " Downloading model".into(),
            " owner/repo:Q4_K_M".into(),
            String::new(),
            " Downloading model".into(),
            String::new(),
            " escape/ctrl+c stop".into(),
            rule(60)
        ]
    );
}

/// On a narrow frame the 45-cell bar line wraps instead of overflowing.
#[tokio::test]
async fn progress_bar_wraps_on_a_narrow_frame() {
    let (mut view, _clock) = new_view();
    let _waiter = view.show_progress(&state(Some(1.0), None));
    for line in view.render(30) {
        assert!(visible_width(&line.plain_text()) <= 30);
    }
}

/// `updateProgress` repaints only while the progress view is the content (`ui.ts:431`).
#[tokio::test]
async fn update_progress_is_ignored_unless_progress_is_showing() {
    let (mut view, _clock) = new_view();
    view.show_status("Busy", "working");
    let before = snapshot(&mut view, 60);
    view.update_progress(&state(Some(0.9), Some("detail")));
    assert_eq!(snapshot(&mut view, 60), before);

    let _waiter = view.show_progress(&state(Some(0.1), None));
    view.update_progress(&state(Some(0.9), Some("detail")));
    let shown = snapshot(&mut view, 60);
    assert!(shown.iter().any(|l| l.contains("90%")));
    assert!(shown.iter().any(|l| l.contains("detail")));
}

/// Pressing `tui.select.cancel` while a `progress()` call is pending resolves it (`ui.ts:458-463`);
/// the progress frame stays up.
#[tokio::test]
async fn cancel_key_resolves_the_pending_progress() {
    let (mut view, _clock) = new_view();
    let mut waiter = view.show_progress(&state(Some(0.2), None));
    view.handle_key(&down());
    view.handle_key(&enter());
    assert!(waiter.try_recv().is_err(), "other keys do not stop it");
    view.handle_key(&escape());
    assert!(waiter.try_recv().is_ok());
    assert!(
        snapshot(&mut view, 60).iter().any(|l| l.contains("20%")),
        "the frame is still the progress view"
    );
}

/// Every caller waiting on the progress view shares one promise upstream (`ui.ts:420-424`): one
/// Escape resolves all of them.
#[tokio::test]
async fn one_cancel_resolves_every_progress_waiter() {
    let (mut view, _clock) = new_view();
    let mut first = view.show_progress(&state(None, None));
    let mut second = view.show_progress(&state(None, None));
    view.handle_key(&escape());
    assert!(first.try_recv().is_ok());
    assert!(second.try_recv().is_ok());
}

/// Replacing the content abandons the pending progress promise: it never resolves (`setContent`
/// clears the resolver, `ui.ts:311-312`), so a late Escape on the new view cannot "stop" it.
#[tokio::test]
async fn replaced_progress_never_resolves() {
    let (overlay, ui, _attached) = llama_overlay(
        Arc::new(LlamaKeys::default()),
        ManualClock::new(),
        tokio::runtime::Handle::current(),
    );
    let mut overlay = overlay;
    let progress_state = state(Some(0.3), None);
    let mut waiting = Box::pin(ui.progress(&progress_state));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    ui.show_status("Other", "view");
    overlay.handle_key(escape());
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        futures::poll!(waiting.as_mut()).is_pending(),
        "still pending forever"
    );
}

// =================================================================================================
// `runWithProgress` (`ui.ts:494-542`)
// =================================================================================================

/// A scripted [`LlamaUi`] for `run_with_progress`: `progress()` resolves ("the user pressed
/// Escape") or pends per a script, `confirm()` answers through channels the test controls.
struct FakeUi {
    stops: Mutex<VecDeque<bool>>,
    confirms: Mutex<VecDeque<oneshot::Receiver<bool>>>,
    log: Mutex<Vec<String>>,
    updates: Mutex<Vec<ProgressState>>,
    shown: Mutex<Vec<ProgressState>>,
}

impl FakeUi {
    fn new(stops: Vec<bool>) -> Arc<Self> {
        Arc::new(Self {
            stops: Mutex::new(stops.into()),
            confirms: Mutex::new(VecDeque::new()),
            log: Mutex::new(Vec::new()),
            updates: Mutex::new(Vec::new()),
            shown: Mutex::new(Vec::new()),
        })
    }

    fn answer_confirm(&self) -> oneshot::Sender<bool> {
        let (tx, rx) = oneshot::channel();
        self.confirms.lock().unwrap().push_back(rx);
        tx
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl LlamaUi for FakeUi {
    async fn show_models(
        &self,
        _server_url: &str,
        _models: &[LlamaModelInfo],
    ) -> LlamaManagerAction {
        LlamaManagerAction::Close
    }
    async fn select(&self, _title: &str, _options: &[String]) -> Option<String> {
        None
    }
    async fn confirm(&self, title: &str, message: &str) -> bool {
        self.log
            .lock()
            .unwrap()
            .push(format!("confirm:{title}|{message}"));
        let receiver = self.confirms.lock().unwrap().pop_front();
        match receiver {
            Some(receiver) => receiver.await.unwrap_or(false),
            None => false,
        }
    }
    async fn search_models(&self, _search: SearchFn) -> Option<String> {
        None
    }
    fn show_status(&self, _title: &str, _message: &str) {}
    async fn progress(&self, state: &ProgressState) {
        self.log
            .lock()
            .unwrap()
            .push(format!("progress:{}", state.message));
        self.shown.lock().unwrap().push(state.clone());
        let stop = self.stops.lock().unwrap().pop_front().unwrap_or(false);
        if !stop {
            std::future::pending::<()>().await;
        }
    }
    fn update_progress(&self, state: &ProgressState) {
        self.updates.lock().unwrap().push(state.clone());
    }
}

fn progress_options() -> ProgressOptions {
    ProgressOptions {
        title: "Loading model".into(),
        model: "qwen".into(),
        initial_message: "Starting…".into(),
        cancel_title: "Stop loading?".into(),
        cancel_message: "qwen".into(),
    }
}

async fn yield_a_lot() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

/// A run that settles returns its value; `update` merges into the state the view is given
/// (`Object.assign`, `ui.ts:510`) and keeps the title and model line.
#[tokio::test]
async fn run_with_progress_returns_the_value_and_forwards_updates() {
    let ui = FakeUi::new(vec![false]);
    let outcome = run_with_progress(
        ui.clone(),
        progress_options(),
        |_signal, update| async move {
            update(LlamaProgress {
                message: "Loading text model".into(),
                ratio: ProgressField::Set(0.5),
                detail: ProgressField::Set("1 / 2".into()),
            });
            tokio::task::yield_now().await;
            Ok::<_, String>(42)
        },
        || async { Ok(()) },
    )
    .await;
    assert_eq!(outcome, Ok(ProgressOutcome::Completed(42)));
    let shown = ui.shown.lock().unwrap().clone();
    assert_eq!(
        shown.first().unwrap(),
        &ProgressState {
            title: "Loading model".into(),
            model: "qwen".into(),
            message: "Starting…".into(),
            ratio: None,
            detail: None
        },
        "the first state shown is the initial message"
    );
    let updates = ui.updates.lock().unwrap().clone();
    assert_eq!(
        updates,
        vec![ProgressState {
            title: "Loading model".into(),
            model: "qwen".into(),
            message: "Loading text model".into(),
            ratio: Some(0.5),
            detail: Some("1 / 2".into()),
        }]
    );
    assert!(
        ui.log().iter().all(|entry| !entry.starts_with("confirm")),
        "no stop was requested"
    );
}

/// A run that fails is an error of `run_with_progress` (`ui.ts:540`).
#[tokio::test]
async fn run_with_progress_propagates_a_failure() {
    let ui = FakeUi::new(vec![false]);
    let outcome: Result<ProgressOutcome<()>, String> = run_with_progress(
        ui,
        progress_options(),
        |_signal, _update| async { Err("Model failed to load".to_string()) },
        || async { Ok(()) },
    )
    .await;
    assert_eq!(outcome, Err("Model failed to load".to_string()));
}

/// Escape, confirm Yes: `cancel()` runs first with the token still live, then the token is
/// aborted, the run is awaited to its end and its result discarded, and the outcome is
/// `Cancelled` (`ui.ts:528-536`).
#[tokio::test]
async fn run_with_progress_cancel_flow() {
    let ui = FakeUi::new(vec![true]);
    let yes = ui.answer_confirm();
    yes.send(true).unwrap();
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let token_slot: Arc<Mutex<Option<CancellationToken>>> = Arc::new(Mutex::new(None));

    let outcome = run_with_progress(
        ui.clone(),
        progress_options(),
        {
            let events = Arc::clone(&events);
            let token_slot = Arc::clone(&token_slot);
            move |signal, _update| {
                *token_slot.lock().unwrap() = Some(signal.clone());
                async move {
                    signal.cancelled().await;
                    events.lock().unwrap().push("run saw the abort".into());
                    Ok::<_, String>("discarded")
                }
            }
        },
        {
            let events = Arc::clone(&events);
            let token_slot = Arc::clone(&token_slot);
            move || async move {
                let live = !token_slot.lock().unwrap().as_ref().unwrap().is_cancelled();
                events
                    .lock()
                    .unwrap()
                    .push(format!("cancel() called, token live: {live}"));
                Ok(())
            }
        },
    )
    .await;

    assert_eq!(outcome, Ok(ProgressOutcome::Cancelled));
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "cancel() called, token live: true".to_string(),
            "run saw the abort".to_string()
        ]
    );
    assert!(ui.log().contains(&"confirm:Stop loading?|qwen".to_string()));
}

/// Declining the confirm returns to the progress view and the run carries on to completion.
#[tokio::test]
async fn run_with_progress_decline_keeps_running() {
    let ui = FakeUi::new(vec![true, false]);
    let no = ui.answer_confirm();
    let (finish_tx, finish_rx) = oneshot::channel::<u32>();
    let cancelled = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn({
        let ui = ui.clone();
        let cancelled = Arc::clone(&cancelled);
        async move {
            run_with_progress(
                ui,
                progress_options(),
                move |_signal, _update| async move { finish_rx.await.map_err(|e| e.to_string()) },
                move || async move {
                    cancelled.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        }
    });
    yield_a_lot().await;
    no.send(false).unwrap();
    yield_a_lot().await;
    assert!(
        !task.is_finished(),
        "still running behind the progress view"
    );
    assert_eq!(
        ui.log(),
        vec![
            "progress:Starting…",
            "confirm:Stop loading?|qwen",
            "progress:Starting…"
        ],
        "progress is shown again after the dialog is declined"
    );
    finish_tx.send(7).unwrap();
    assert_eq!(task.await.unwrap(), Ok(ProgressOutcome::Completed(7)));
    assert!(
        !cancelled.load(Ordering::SeqCst),
        "cancel() was never called"
    );
}

/// The run keeps going while the confirm is open; if it settles meanwhile, a "Yes" does not cancel
/// anything and the value is returned (`ui.ts:529`: `if (!stop || completed) continue`).
#[tokio::test]
async fn run_with_progress_a_finished_run_wins_over_a_late_yes() {
    let ui = FakeUi::new(vec![true]);
    let answer = ui.answer_confirm();
    let (finish_tx, finish_rx) = oneshot::channel::<u32>();
    let cancelled = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn({
        let ui = ui.clone();
        let cancelled = Arc::clone(&cancelled);
        async move {
            run_with_progress(
                ui,
                progress_options(),
                move |_signal, _update| async move { finish_rx.await.map_err(|e| e.to_string()) },
                move || async move {
                    cancelled.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        }
    });
    yield_a_lot().await;
    finish_tx.send(9).unwrap();
    yield_a_lot().await;
    assert!(
        !task.is_finished(),
        "the dialog stays up until it is answered, even though the run is done"
    );
    answer.send(true).unwrap();
    assert_eq!(task.await.unwrap(), Ok(ProgressOutcome::Completed(9)));
    assert!(!cancelled.load(Ordering::SeqCst));
}

/// A failing `cancel()` still aborts the token, and its error is the result (`try/finally`,
/// `ui.ts:530-534`).
#[tokio::test]
async fn run_with_progress_cancel_failure_still_aborts() {
    let ui = FakeUi::new(vec![true]);
    ui.answer_confirm().send(true).unwrap();
    let token_slot: Arc<Mutex<Option<CancellationToken>>> = Arc::new(Mutex::new(None));
    let outcome: Result<ProgressOutcome<()>, String> = run_with_progress(
        ui,
        progress_options(),
        {
            let token_slot = Arc::clone(&token_slot);
            move |signal, _update| {
                *token_slot.lock().unwrap() = Some(signal);
                async { std::future::pending::<Result<(), String>>().await }
            }
        },
        || async { Err("unload failed".to_string()) },
    )
    .await;
    assert_eq!(outcome, Err("unload failed".to_string()));
    assert!(token_slot.lock().unwrap().as_ref().unwrap().is_cancelled());
}

/// The same flow through the real view: Escape opens the "Stop loading?" dialog on screen, `Yes`
/// stops, `No` goes back to the progress frame. Updates from the run show up on the next tick.
#[tokio::test]
async fn run_with_progress_through_the_overlay() {
    let clock = ManualClock::new();
    let (overlay, ui, _attached) = llama_overlay(
        Arc::new(LlamaKeys::default()),
        clock,
        tokio::runtime::Handle::current(),
    );
    let overlay = Arc::new(Mutex::new(overlay));
    let ui: Arc<dyn LlamaUi> = Arc::new(ui);
    let frame = |width: usize| plain(&overlay.lock().unwrap().render(width, 24));
    let press_key = |key: OverlayKey| {
        overlay.lock().unwrap().handle_key(key);
    };

    let (updater_tx, updater_rx) = oneshot::channel::<()>();
    let cancel_called = Arc::new(AtomicBool::new(false));
    let aborted = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn({
        let ui = Arc::clone(&ui);
        let cancel_called = Arc::clone(&cancel_called);
        let aborted = Arc::clone(&aborted);
        async move {
            run_with_progress(
                ui,
                progress_options(),
                move |signal, update| async move {
                    updater_rx.await.ok();
                    update(LlamaProgress {
                        message: "Loading text model".into(),
                        ratio: ProgressField::Set(0.25),
                        detail: ProgressField::Keep,
                    });
                    signal.cancelled().await;
                    aborted.store(true, Ordering::SeqCst);
                    Ok::<_, String>(())
                },
                move || async move {
                    cancel_called.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        }
    });
    yield_a_lot().await;
    let shown = frame(60);
    assert_eq!(shown[1], " Loading model");
    assert_eq!(shown[2], " qwen");
    assert_eq!(shown[4], " Starting…");

    updater_tx.send(()).unwrap();
    yield_a_lot().await;
    let shown = frame(60);
    assert_eq!(shown[4], " Loading text model");
    assert_eq!(
        shown[5],
        format!(" {}{} 25%", "█".repeat(10), "─".repeat(30))
    );

    // Escape: the confirm select replaces the progress frame.
    press_key(escape());
    yield_a_lot().await;
    let shown = frame(60);
    assert_eq!(shown[1], " Stop loading?");
    assert_eq!(shown[2], " qwen");
    assert!(shown.contains(&"→ Yes".to_string()));

    // No: back to the progress frame, the run still going.
    press_key(down());
    press_key(enter());
    yield_a_lot().await;
    assert_eq!(frame(60)[1], " Loading model");
    assert!(!task.is_finished());
    assert!(!cancel_called.load(Ordering::SeqCst));

    // Escape, Yes: stopped.
    press_key(escape());
    yield_a_lot().await;
    press_key(enter());
    assert_eq!(task.await.unwrap(), Ok(ProgressOutcome::Cancelled));
    assert!(cancel_called.load(Ordering::SeqCst));
    assert!(
        aborted.load(Ordering::SeqCst),
        "the run was aborted and awaited"
    );
}

// =================================================================================================
// Text primitives
// =================================================================================================

#[test]
fn width_and_truncation_are_grapheme_atomic() {
    assert_eq!(visible_width("abc"), 3);
    assert_eq!(visible_width("日本"), 4, "wide characters take two columns");
    assert_eq!(visible_width("a\tb"), 5, "a tab costs three columns");
    assert_eq!(
        truncate_to_width("日本語", 5),
        "日本",
        "a wide character is never cut"
    );
    assert_eq!(
        truncate_to_width("e\u{301}x", 1),
        "e\u{301}",
        "a combining mark stays with its base"
    );
    assert_eq!(
        truncate_to_width("👨‍👩‍👧x", 2),
        "👨‍👩‍👧",
        "a ZWJ family is one cluster"
    );
    assert_eq!(truncate_to_width("abc", 0), "");
    assert_eq!(truncate_to_width("abc", 10), "abc");
}

#[test]
fn wrap_ranges_follow_wrap_text_with_ansi() {
    let wrap = |text: &str, width: usize| -> Vec<String> {
        wrap_ranges(text, width)
            .into_iter()
            .map(|(s, e)| text[s..e].to_string())
            .collect()
    };
    assert_eq!(wrap("hello world", 11), vec!["hello world"]);
    assert_eq!(wrap("hello world", 8), vec!["hello", "world"]);
    assert_eq!(wrap("a b c d e f", 5), vec!["a b c", "d e f"]);
    assert_eq!(
        wrap("abcdefghij", 4),
        vec!["abcd", "efgh", "ij"],
        "an over-long word is broken"
    );
    assert_eq!(wrap("ab cdefghij", 4), vec!["ab", "cdef", "ghij"]);
    assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
    assert_eq!(wrap("a\r\nb\rc", 10), vec!["a", "b", "c"]);
    assert_eq!(
        wrap("a\n\nb", 10),
        vec!["a", "", "b"],
        "an empty line is kept"
    );
    assert_eq!(wrap("", 10), vec![""]);
    assert_eq!(
        wrap("日本語日本語", 4),
        vec!["日本", "語日", "本語"],
        "each CJK character is a break point"
    );
}

/// `cjkBreakRegex` (`utils.ts:54-55`) is `Script_Extensions`, tested against the whole cluster.
/// Every expectation is pi-tui's own `wrapTextWithAnsi` at f1b2e77f5, run under node 22.22.0
/// (ICU 77.1, Unicode 16): `、`/`。` (scx Han/Hira/Kana/…), half-width kana, `々`, `・` and a
/// combining voicing mark after a Latin letter are break tokens; no fixed block list has them all.
#[test]
fn wrap_ranges_break_cjk_by_script_extensions_as_pi_does() {
    let wrap = |text: &str, width: usize| -> Vec<String> {
        wrap_ranges(text, width)
            .into_iter()
            .map(|(s, e)| text[s..e].to_string())
            .collect()
    };
    let ids = "Qwen3、日本語。ｶﾀｶﾅ-8B café 한국어ㄅㄆ";
    assert_eq!(
        wrap(ids, 4),
        vec![
            "Qwen",
            "3、",
            "日本",
            "語。",
            "ｶﾀｶﾅ",
            "-8B",
            "café",
            "한국",
            "어ㄅ",
            "ㄆ"
        ]
    );
    assert_eq!(
        wrap(ids, 6),
        vec![
            "Qwen3",
            "、日本",
            "語。ｶﾀ",
            "ｶﾅ-8B",
            "café",
            "한국어",
            "ㄅㄆ"
        ]
    );
    assert_eq!(
        wrap(ids, 9),
        vec!["Qwen3、日", "本語。ｶﾀｶ", "ﾅ-8B café", "한국어ㄅ", "ㄆ"]
    );
    assert_eq!(
        wrap(ids, 12),
        vec!["Qwen3、日本", "語。ｶﾀｶﾅ-8B", "café 한국어", "ㄅㄆ"]
    );
    assert_eq!(wrap("model、gguf。ok", 8), vec!["model、", "gguf。ok"]);
    assert_eq!(wrap("abc々def", 4), vec!["abc", "々", "def"]);
    assert_eq!(wrap("ｶﾀｶﾅabc", 5), vec!["ｶﾀｶﾅ", "abc"]);
    assert_eq!(wrap("ab\u{3099}cd", 3), vec!["ab\u{3099}", "cd"]);
    assert_eq!(wrap("x・y", 2), vec!["x", "・", "y"]);
    assert_eq!(wrap("中文模型 日本語", 6), vec!["中文模", "型 日", "本語"]);
    assert_eq!(wrap("llama ㄅㄆㄇ", 7), vec!["llama", "ㄅㄆㄇ"]);
    assert_eq!(wrap("qwen〆x", 5), vec!["qwen", "〆x"]);
}

// =================================================================================================
// Fuzzy matching (`fuzzy.ts`)
// =================================================================================================

#[test]
fn fuzzy_match_scores_as_fuzzy_ts() {
    let close = |a: Option<f64>, b: f64| (a.unwrap() - b).abs() < 1e-9;
    // Two consecutive hits at a word boundary, then the exact-string bonus.
    assert!(close(fuzzy_match("ab", "ab"), -10.0 - 5.0 + 0.1 - 100.0));
    // A gap of one, a boundary after `_`.
    assert!(close(fuzzy_match("ab", "a_b"), -10.0 + 2.0 - 10.0 + 0.2));
    assert_eq!(fuzzy_match("xyz", "abc"), None);
    assert_eq!(
        fuzzy_match("abcd", "abc"),
        None,
        "a query longer than the text cannot match"
    );
    assert!(close(fuzzy_match("", "anything"), 0.0));
    // Case-insensitive.
    assert_eq!(fuzzy_match("QW", "qwen"), fuzzy_match("qw", "QWEN"));
    // Subsequence order matters.
    assert_eq!(fuzzy_match("ba", "ab"), None);
}

#[test]
fn fuzzy_match_alphanumeric_swap_fallback() {
    // `abc123` is not a subsequence of `123-abc`, but its swap `123abc` is: +5 penalty.
    let direct = fuzzy_match("123abc", "123-abc").unwrap();
    let swapped = fuzzy_match("abc123", "123-abc").unwrap();
    assert!(
        (swapped - (direct + 5.0)).abs() < 1e-9,
        "{swapped} vs {direct}"
    );
    // `123abc` is also the swap of `abc123` the other way round.
    assert!(fuzzy_match("123abc", "abc-123").is_some());
    // Mixed shapes do not swap.
    assert_eq!(fuzzy_match("a1b2", "b2a1"), None);
}

#[test]
fn fuzzy_filter_tokens_order_and_empty_query() {
    let items = ["owner/qwen-7b", "owner/llama", "qwen/other", "q-w-e-n"];
    let ids = |query: &str| -> Vec<&str> {
        fuzzy_filter(&items, query, |s| s)
            .into_iter()
            .map(|i| items[i])
            .collect()
    };
    assert_eq!(
        ids(""),
        items.to_vec(),
        "an empty query keeps everything in order"
    );
    assert_eq!(ids("   "), items.to_vec());
    let qwen = ids("qwen");
    assert_eq!(qwen.len(), 3);
    assert!(!qwen.contains(&"owner/llama"));
    assert_eq!(
        qwen.last(),
        Some(&"q-w-e-n"),
        "a scattered match ranks last"
    );
    assert_eq!(
        ids("owner qwen"),
        vec!["owner/qwen-7b"],
        "every whitespace-separated token must match"
    );
    assert_eq!(
        ids("owner/llama"),
        vec!["owner/llama"],
        "slashes also separate tokens"
    );
    assert_eq!(
        ids("llama/owner"),
        vec!["owner/llama"],
        "slash-separated tokens match in any order, not as one subsequence"
    );
    assert_eq!(ids("nomatch"), Vec::<&str>::new());
    // Best score first: the scattered id listed first must come out last.
    let scattered_first = ["q-w-e-n", "owner/qwen-7b"];
    assert_eq!(
        fuzzy_filter(&scattered_first, "qwen", |s| s),
        vec![1, 0],
        "results are ordered by score, not by position"
    );
}

// =================================================================================================
// `Input` (`input.ts`)
// =================================================================================================

fn input_with(text: &str) -> TextInput {
    let keys = LlamaKeys::default();
    let mut input = TextInput::new();
    for c in text.chars() {
        input.handle_key(&keys, &chr(c));
    }
    input
}

#[test]
fn input_inserts_and_deletes_by_grapheme() {
    let keys = LlamaKeys::default();
    let mut input = input_with("ab");
    input.handle_key(&keys, &chr('é'));
    assert_eq!(input.value(), "abé");
    input.handle_key(&keys, &press(OverlayKeyCode::Backspace));
    assert_eq!(input.value(), "ab");

    // A cluster is one backspace.
    let mut input = input_with("a");
    for c in "👨‍👩‍👧".chars() {
        input.handle_key(&keys, &chr(c));
    }
    input.handle_key(&keys, &press(OverlayKeyCode::Backspace));
    assert_eq!(input.value(), "a");

    let mut input = input_with("a");
    for c in "e\u{301}".chars() {
        input.handle_key(&keys, &chr(c));
    }
    input.handle_key(&keys, &press(OverlayKeyCode::Backspace));
    assert_eq!(
        input.value(),
        "a",
        "a base and its combining mark go together"
    );

    // Forward delete, cursor movement.
    let mut input = input_with("abc");
    input.handle_key(&keys, &press(OverlayKeyCode::Left));
    input.handle_key(&keys, &press(OverlayKeyCode::Left));
    input.handle_key(&keys, &press(OverlayKeyCode::Delete));
    assert_eq!(input.value(), "ac");
    input.handle_key(&keys, &press(OverlayKeyCode::Home));
    input.handle_key(&keys, &chr('X'));
    assert_eq!(input.value(), "Xac");
    input.handle_key(&keys, &press(OverlayKeyCode::End));
    input.handle_key(&keys, &chr('Y'));
    assert_eq!(input.value(), "XacY");
    input.handle_key(&keys, &ctrl('a'));
    input.handle_key(&keys, &ctrl('d'));
    assert_eq!(input.value(), "acY");
    input.handle_key(&keys, &ctrl('e'));
    input.handle_key(&keys, &ctrl('b'));
    input.handle_key(&keys, &ctrl('f'));
    input.handle_key(&keys, &chr('!'));
    assert_eq!(input.value(), "acY!");
}

/// Control characters and alt-prefixed keys are not text (`input.ts:202-210`); a rebound key is
/// consumed by its action, not inserted.
#[test]
fn input_rejects_control_and_alt_keys() {
    let keys = LlamaKeys::default();
    let mut input = input_with("ab");
    input.handle_key(&keys, &ctrl('x'));
    input.handle_key(&keys, &alt('x'));
    input.handle_key(&keys, &enter());
    input.handle_key(&keys, &press(OverlayKeyCode::Tab));
    input.handle_key(&keys, &press(OverlayKeyCode::F(2)));
    // A bare control character (BEL) is not text either.
    input.handle_key(&keys, &chr('\u{7}'));
    assert_eq!(input.value(), "ab");
}

/// Undo puts back the cursor as well as the text (`input.ts` `undo`): after undoing a forward
/// delete made at the start of the line, typing lands at the start again.
#[test]
fn input_undo_restores_the_cursor() {
    let keys = LlamaKeys::default();
    let mut input = input_with("hello");
    input.handle_key(&keys, &press(OverlayKeyCode::Home));
    input.handle_key(&keys, &press(OverlayKeyCode::Delete));
    assert_eq!(input.value(), "ello");
    input.handle_key(&keys, &press(OverlayKeyCode::End));
    input.handle_key(&keys, &ctrl('-'));
    assert_eq!(input.value(), "hello");
    input.handle_key(&keys, &chr('X'));
    assert_eq!(
        input.value(),
        "Xhello",
        "the cursor came back with the text"
    );
}

/// Word motion and word deletion use the segmenter and the punctuation boundaries
/// (`word-navigation.ts`): `foo.bar` is two stops.
#[test]
fn input_word_motion_and_deletion() {
    let keys = LlamaKeys::default();

    // `bar.baz` is ONE UAX#29 word segment; backward motion stops after its last punctuation.
    let mut input = input_with("foo bar.baz qux");
    input.handle_key(&keys, &alt('b'));
    input.handle_key(&keys, &alt('b'));
    input.handle_key(&keys, &chr('|'));
    assert_eq!(input.value(), "foo bar.|baz qux");

    // Forward motion stops before the first punctuation of a word segment.
    let mut input = input_with("foo bar.baz qux");
    input.handle_key(&keys, &ctrl('a'));
    input.handle_key(&keys, &alt('f'));
    input.handle_key(&keys, &alt('f'));
    input.handle_key(&keys, &chr('|'));
    assert_eq!(input.value(), "foo bar|.baz qux");

    // Word deletion follows the same boundaries; the two kills accumulate into one ring entry.
    let mut input = input_with("foo bar.baz");
    input.handle_key(&keys, &ctrl('w'));
    assert_eq!(input.value(), "foo bar.");
    input.handle_key(&keys, &ctrl('w'));
    assert_eq!(
        input.value(),
        "foo bar",
        "a trailing punctuation run is one stop"
    );
    input.handle_key(&keys, &ctrl('y'));
    assert_eq!(input.value(), "foo bar.baz");

    // ctrl+left / ctrl+right are word motions too.
    let mut input = input_with("one two");
    input.handle_key(
        &keys,
        &OverlayKey {
            code: OverlayKeyCode::Left,
            ctrl: true,
            alt: false,
            shift: false,
        },
    );
    input.handle_key(&keys, &chr('|'));
    assert_eq!(input.value(), "one |two");
}

#[test]
fn input_kill_ring_undo_and_yank() {
    let keys = LlamaKeys::default();

    // Ctrl+W deletes a word backwards; consecutive kills accumulate into one ring entry, so one
    // yank restores both words.
    let mut input = input_with("one two three");
    input.handle_key(&keys, &ctrl('w'));
    assert_eq!(input.value(), "one two ");
    input.handle_key(&keys, &ctrl('w'));
    assert_eq!(input.value(), "one ");
    input.handle_key(&keys, &ctrl('y'));
    assert_eq!(input.value(), "one two three");

    // Ctrl+U kills to the start, Ctrl+K to the end.
    let mut input = input_with("hello world");
    input.handle_key(&keys, &press(OverlayKeyCode::Left));
    input.handle_key(&keys, &press(OverlayKeyCode::Left));
    input.handle_key(&keys, &ctrl('k'));
    assert_eq!(input.value(), "hello wor");
    input.handle_key(&keys, &ctrl('u'));
    assert_eq!(input.value(), "");
    input.handle_key(&keys, &ctrl('y'));
    assert_eq!(
        input.value(),
        "hello world",
        "back-to-back kills accumulate into one entry: the backward kill is prepended"
    );

    // Yank-pop rotates to the previous entry (only right after a yank).
    let mut input = input_with("first second");
    input.handle_key(&keys, &ctrl('w'));
    input.handle_key(&keys, &chr('!'));
    input.handle_key(&keys, &ctrl('w'));
    assert_eq!(input.value(), "first ");
    input.handle_key(&keys, &alt('y'));
    assert_eq!(
        input.value(),
        "first ",
        "alt+y without a preceding yank does nothing"
    );
    input.handle_key(&keys, &ctrl('y'));
    assert_eq!(input.value(), "first !");
    input.handle_key(&keys, &alt('y'));
    assert_eq!(input.value(), "first second");

    // Alt+D deletes a word forward.
    let mut input = input_with("alpha beta");
    input.handle_key(&keys, &ctrl('a'));
    input.handle_key(&keys, &alt('d'));
    assert_eq!(input.value(), " beta");
}

/// Undo coalesces consecutive word characters and snapshots at every whitespace
/// (`input.ts:215`): one undo removes the last word, not the line.
#[test]
fn input_undo_coalesces_typing_by_word() {
    let keys = LlamaKeys::default();
    let mut input = input_with("one two");
    input.handle_key(&keys, &ctrl('-'));
    assert_eq!(input.value(), "one", "the second word is one undo step");
    input.handle_key(&keys, &ctrl('-'));
    assert_eq!(input.value(), "");
    input.handle_key(&keys, &ctrl('-'));
    assert_eq!(input.value(), "", "an empty stack is a no-op");

    let mut input = input_with("abc");
    input.handle_key(&keys, &press(OverlayKeyCode::Backspace));
    assert_eq!(input.value(), "ab");
    input.handle_key(&keys, &ctrl('-'));
    assert_eq!(input.value(), "abc", "a deletion is its own undo step");
}

/// The line is `> ` + the text with a reverse-video cursor cell; with long text it scrolls
/// horizontally so the cursor stays visible, and never exceeds the width (`input.ts:266-376`).
#[test]
fn input_render_cursor_and_horizontal_scroll() {
    let keys = LlamaKeys::default();
    let input = input_with("abc");
    let line = input.render(20);
    assert_eq!(line.plain_text(), format!("> abc {}", " ".repeat(14)));
    let cursor = line.spans.iter().find(|s| s.reversed).unwrap();
    assert_eq!(
        cursor.text, " ",
        "at the end the cursor sits on a blank cell"
    );

    let mut input = input_with("abc");
    input.handle_key(&keys, &press(OverlayKeyCode::Left));
    let line = input.render(20);
    assert_eq!(
        line.spans.iter().find(|s| s.reversed).unwrap().text,
        "c",
        "mid-text it inverts the character"
    );

    let long = "0123456789abcdefghijklmnopqrstuvwxyz";
    let mut input = input_with(long);
    let line = input.render(14);
    assert_eq!(visible_width(&line.plain_text()), 14);
    assert!(
        line.plain_text().contains("wxyz"),
        "the cursor at the end keeps the tail in view: {:?}",
        line.plain_text()
    );
    input.handle_key(&keys, &press(OverlayKeyCode::Home));
    let line = input.render(14);
    assert!(
        line.plain_text().starts_with("> 0123"),
        "{:?}",
        line.plain_text()
    );
    assert_eq!(visible_width(&line.plain_text()), 14);
    for _ in 0..20 {
        input.handle_key(&keys, &press(OverlayKeyCode::Right));
    }
    let line = input.render(14);
    let text = line.plain_text();
    assert!(
        text.contains('k') && text.contains('l'),
        "the window followed the cursor: {text:?}"
    );
    assert_eq!(visible_width(&text), 14);
    // Degenerate widths do not panic.
    for width in 0..4 {
        let _ = input.render(width);
    }
}

// =================================================================================================
// Hugging Face search (`ui.ts:96-274`)
// =================================================================================================

#[test]
fn compact_count_formats_like_ui_ts() {
    assert_eq!(compact_count(0.0), "0");
    assert_eq!(compact_count(999.0), "999");
    assert_eq!(compact_count(1_000.0), "1.0k");
    assert_eq!(
        compact_count(1_250.0),
        "1.3k",
        "toFixed picks the larger candidate on a tie"
    );
    assert_eq!(compact_count(1_149.0), "1.1k");
    assert_eq!(compact_count(99_949.0), "99.9k");
    assert_eq!(compact_count(99_999.0), "100.0k");
    assert_eq!(compact_count(100_000.0), "100k");
    assert_eq!(compact_count(150_400.0), "150k");
    assert_eq!(compact_count(999_999.0), "1000k");
    assert_eq!(compact_count(1_000_000.0), "1.0M");
    assert_eq!(compact_count(1_500_000.0), "1.5M");
    assert_eq!(
        compact_count(9_950_000.0),
        "9.9M",
        "9.95 is just below the tie in binary, as in JS"
    );
    assert_eq!(compact_count(9_960_000.0), "10.0M");
    assert_eq!(compact_count(10_000_000.0), "10M");
    assert_eq!(compact_count(12_345_678.0), "12M");
}

#[test]
fn to_fixed_rounds_half_up_on_the_exact_value() {
    assert_eq!(
        to_fixed(1.25, 1),
        "1.3",
        "an exact tie goes up (Rust's {{:.1}} would say 1.2)"
    );
    assert_eq!(to_fixed(0.25, 1), "0.3");
    assert_eq!(to_fixed(2.5, 0), "3");
    assert_eq!(
        to_fixed(1.15, 1),
        "1.1",
        "1.15 is slightly below the tie in binary"
    );
    assert_eq!(to_fixed(9.96, 1), "10.0", "a carry grows the integer part");
    assert_eq!(to_fixed(99.99, 1), "100.0");
    assert_eq!(to_fixed(0.04, 1), "0.0");
    assert_eq!(to_fixed(7.0, 0), "7");
}

/// The empty search: the hint line, the `> ` prompt with its cursor, the status line, the
/// `select`/`back` footer.
#[tokio::test]
async fn search_view_initial_snapshot() {
    let (mut view, _clock) = new_view();
    let search = FakeSearch::new();
    let _rx = view.show_search(search.as_fn());
    for width in [60usize, 90] {
        let expected = vec![
            rule(width),
            " Download model".to_string(),
            String::new(),
            " Model name or owner/repository[:quant]".to_string(),
            ">".to_string(),
            String::new(),
            "  Type at least 2 characters".to_string(),
            String::new(),
            " enter select • escape/ctrl+c back".to_string(),
            rule(width),
        ];
        assert_eq!(snapshot(&mut view, width), expected, "width {width}");
    }
}

/// One character searches nothing; two start the 500 ms debounce; the search fires exactly when
/// the clock reaches it (`ui.ts:197-211`) with the trimmed query, and results render with a
/// compact download count and a `→` marker.
#[tokio::test]
async fn search_debounces_by_500_ms() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(vec![
        hf("unsloth/Qwen3-8B-GGUF", 1_250_000.0),
        hf("bartowski/Qwen3-8B-GGUF", 840.0),
    ]);
    let _rx = view.show_search(search.as_fn());

    type_text(&mut view, "q");
    clock.advance(10_000);
    settle(&mut view).await;
    assert!(search.queries().is_empty(), "one character never searches");
    assert!(snapshot(&mut view, 70).contains(&"  Type at least 2 characters".to_string()));

    type_text(&mut view, "w");
    assert!(
        snapshot(&mut view, 70).contains(&"  Searching Hugging Face…".to_string()),
        "the status shows at once"
    );
    clock.advance(499);
    settle(&mut view).await;
    assert!(search.queries().is_empty(), "499 ms is too early");
    clock.advance(1);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["qw"]);

    let frame = snapshot(&mut view, 70);
    assert!(
        frame.contains(&"→ unsloth/Qwen3-8B-GGUF  1.3M downloads".to_string()),
        "{frame:#?}"
    );
    assert!(
        frame.contains(&"  bartowski/Qwen3-8B-GGUF  840 downloads".to_string()),
        "{frame:#?}"
    );
    assert!(
        !frame.iter().any(|l| l.contains("Searching")),
        "no status once results are in"
    );
}

/// Typing again within the window restarts the debounce and only the final query is searched;
/// leading and trailing spaces are not part of it.
#[tokio::test]
async fn search_restarts_the_debounce_on_every_keystroke() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "qw");
    clock.advance(400);
    settle(&mut view).await;
    type_text(&mut view, "e");
    clock.advance(400);
    settle(&mut view).await;
    assert!(
        search.queries().is_empty(),
        "the first deadline was cancelled"
    );
    clock.advance(100);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["qwe"]);

    type_text(&mut view, " ");
    clock.advance(1_000);
    settle(&mut view).await;
    assert_eq!(
        search.queries(),
        vec!["qwe"],
        "a trailing space does not change the (trimmed) query"
    );
}

/// A new keystroke aborts the in-flight request (`ui.ts:195`), and its late answer is ignored
/// (`ui.ts:220`).
#[tokio::test]
async fn search_aborts_the_previous_request_and_ignores_its_answer() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    let (gate_tx, gate_rx) = oneshot::channel();
    search.reply(Reply::Gate(gate_rx));
    search.reply_ok(vec![hf("abc/second", 5.0)]);
    let _rx = view.show_search(search.as_fn());

    type_text(&mut view, "ab");
    clock.advance(500);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["ab"]);
    let first = search.token(0);
    assert!(!first.is_cancelled());

    type_text(&mut view, "c");
    assert!(first.is_cancelled(), "typing aborted the request in flight");

    // The first search answers anyway (it ignored the abort); nothing may show.
    gate_tx.send(Ok(vec![hf("owner/stale", 1.0)])).unwrap();
    settle(&mut view).await;
    let frame = snapshot(&mut view, 70);
    assert!(
        !frame.iter().any(|l| l.contains("owner/stale")),
        "{frame:#?}"
    );
    assert!(frame.contains(&"  Searching Hugging Face…".to_string()));

    clock.advance(500);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["ab", "abc"]);
    assert!(
        snapshot(&mut view, 70)
            .iter()
            .any(|l| l.contains("abc/second"))
    );
}

/// An answer for a request that was aborted is dropped even when the box has since returned to
/// that very query (`ui.ts:220`: `request.signal.aborted` is checked on its own, not only
/// `this.query !== query`).
#[tokio::test]
async fn search_ignores_an_aborted_answer_even_when_the_query_is_back() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    let (gate_tx, gate_rx) = oneshot::channel();
    search.reply(Reply::Gate(gate_rx));
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "ab");
    clock.advance(500);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["ab"]);

    // `abc` aborts the request; deleting back to `ab` makes the box's query equal to the aborted
    // request's again.
    type_text(&mut view, "c");
    view.handle_key(&press(OverlayKeyCode::Backspace));
    gate_tx.send(Ok(vec![hf("ab/late", 1.0)])).unwrap();
    settle(&mut view).await;
    let frame = snapshot(&mut view, 70);
    assert!(
        !frame.iter().any(|l| l.contains("ab/late")),
        "the aborted request's answer must not show: {frame:#?}"
    );
    assert!(frame.contains(&"  Searching Hugging Face…".to_string()));
}

/// Results are cached per lowercased query (`ui.ts:202-208`): the same query in any case, typed
/// again, is served without a search, and an empty cached result reads `No GGUF models found`.
#[tokio::test]
async fn search_caches_by_lowercased_query() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(vec![hf("owner/qwen", 10.0)]);
    search.reply_ok(Vec::new());
    let _rx = view.show_search(search.as_fn());

    type_text(&mut view, "Qw");
    clock.advance(500);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["Qw"]);

    // Back to one character, then `qW`: the lowercased key is the same.
    view.handle_key(&press(OverlayKeyCode::Backspace));
    view.handle_key(&press(OverlayKeyCode::Backspace));
    type_text(&mut view, "qW");
    let frame = snapshot(&mut view, 70);
    assert!(
        frame.iter().any(|l| l.contains("owner/qwen")),
        "served from the cache at once: {frame:#?}"
    );
    assert!(!frame.iter().any(|l| l.contains("Searching")));
    clock.advance(10_000);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["Qw"], "no second search");

    // An empty result is cached too and reads as such.
    for _ in 0..2 {
        view.handle_key(&press(OverlayKeyCode::Backspace));
    }
    type_text(&mut view, "zz");
    clock.advance(500);
    settle(&mut view).await;
    assert!(snapshot(&mut view, 70).contains(&"  No GGUF models found".to_string()));
    for _ in 0..2 {
        view.handle_key(&press(OverlayKeyCode::Backspace));
    }
    type_text(&mut view, "ZZ");
    assert!(snapshot(&mut view, 70).contains(&"  No GGUF models found".to_string()));
    assert_eq!(search.queries(), vec!["Qw", "zz"]);
}

/// The cache is written before the staleness check (`ui.ts:219`), so an answer that arrives after
/// the query moved on is still remembered. The cache also outlives one search view: a second
/// `searchModels` in the same session reuses it (`ui.ts:280`).
#[tokio::test]
async fn search_cache_is_written_for_stale_answers_and_outlives_the_view() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    let (gate_tx, gate_rx) = oneshot::channel();
    search.reply(Reply::Gate(gate_rx));
    let mut rx = view.show_search(search.as_fn());

    type_text(&mut view, "ab");
    clock.advance(500);
    settle(&mut view).await;
    type_text(&mut view, "c");
    gate_tx.send(Ok(vec![hf("owner/ab-model", 3.0)])).unwrap();
    settle(&mut view).await;

    view.handle_key(&press(OverlayKeyCode::Backspace));
    let frame = snapshot(&mut view, 70);
    assert!(
        frame.iter().any(|l| l.contains("owner/ab-model")),
        "the stale answer was cached: {frame:#?}"
    );

    // Close, open a fresh search view: still cached.
    view.handle_key(&escape());
    assert_eq!(rx.try_recv().unwrap(), None);
    let _rx2 = view.show_search(search.as_fn());
    type_text(&mut view, "AB");
    assert!(
        snapshot(&mut view, 70)
            .iter()
            .any(|l| l.contains("owner/ab-model"))
    );
    clock.advance(10_000);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["ab"]);
}

/// The result list is the API order narrowed by the fuzzy filter, not re-ranked by score
/// (`ui.ts:182-191` filters `results` by a `Set` of matched ids).
#[tokio::test]
async fn search_filters_locally_and_keeps_api_order() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(vec![
        // Scores for `qw`: this scattered id -6.7, `qwen/qwen-exact` -14.9 (lower is better), so a
        // re-ranking would put the second first.
        hf("zz/q_zzzz_w_e", 900.0),
        hf("x/unrelated", 800.0),
        hf("qwen/qwen-exact", 700.0),
    ]);
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "qw");
    clock.advance(500);
    settle(&mut view).await;
    let rows: Vec<String> = snapshot(&mut view, 70)
        .into_iter()
        .filter(|l| l.contains("downloads"))
        .collect();
    assert_eq!(
        rows,
        vec![
            "→ zz/q_zzzz_w_e  900 downloads".to_string(),
            "  qwen/qwen-exact  700 downloads".to_string()
        ],
        "`x/unrelated` is filtered out and the survivors keep the API order"
    );

    // Typing more narrows the same results locally (and starts a new search).
    type_text(&mut view, "e");
    let frame = snapshot(&mut view, 70);
    let rows: Vec<&String> = frame.iter().filter(|l| l.contains("downloads")).collect();
    assert_eq!(rows.len(), 2);
    assert!(
        frame.contains(&"  Searching Hugging Face…".to_string()),
        "the status stays under the rows while a search is pending: {frame:#?}"
    );
    type_text(&mut view, "z");
    assert!(
        snapshot(&mut view, 70)
            .iter()
            .all(|l| !l.contains("downloads")),
        "nothing matches `qwez`, so the status line shows instead"
    );
}

/// Up and down move through the results and wrap; confirm answers with the highlighted id.
#[tokio::test]
async fn search_navigation_and_confirm() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(vec![hf("a/one", 1.0), hf("a/two", 2.0), hf("a/three", 3.0)]);
    let mut rx = view.show_search(search.as_fn());
    type_text(&mut view, "a");
    type_text(&mut view, "/");
    clock.advance(500);
    settle(&mut view).await;
    view.handle_key(&up());
    assert!(
        snapshot(&mut view, 70).contains(&"→ a/three  3 downloads".to_string()),
        "up wraps to the last"
    );
    view.handle_key(&down());
    view.handle_key(&down());
    assert!(
        snapshot(&mut view, 70).contains(&"→ a/two  2 downloads".to_string()),
        "down wraps to the first, then moves on"
    );
    // The query `a/` is not an exact repository id, so the highlighted row is chosen.
    view.handle_key(&enter());
    assert_eq!(rx.try_recv().unwrap(), Some("a/two".to_string()));
}

/// `/^[^/\s]+\/[^:\s]+(?::[^\s:]+)?$/` (`ui.ts:259`): an exact `owner/repository[:quant]` typed
/// into the box wins over the highlighted row; anything else falls back to the row.
#[tokio::test]
async fn search_exact_repository_bypasses_the_list() {
    async fn confirm_with(query: &str, results: Vec<HuggingFaceModel>) -> Option<Option<String>> {
        let (mut view, clock) = new_view();
        let search = FakeSearch::new();
        search.reply_ok(results);
        let mut rx = view.show_search(search.as_fn());
        type_text(&mut view, query);
        clock.advance(500);
        settle(&mut view).await;
        view.handle_key(&enter());
        rx.try_recv().ok()
    }
    let listed = vec![hf("listed/model", 10.0)];

    // Exact forms bypass the (possibly empty) list.
    assert_eq!(
        confirm_with("owner/repo", Vec::new()).await,
        Some(Some("owner/repo".to_string()))
    );
    assert_eq!(
        confirm_with("owner/repo:Q4_K_M", listed.clone()).await,
        Some(Some("owner/repo:Q4_K_M".to_string())),
        "even with a different row highlighted"
    );
    assert_eq!(
        confirm_with("  owner/repo  ", Vec::new()).await,
        Some(Some("owner/repo".to_string())),
        "the query is trimmed"
    );
    assert_eq!(
        confirm_with("unsloth/gemma-3n-E4B-it-GGUF:UD-Q4_K_XL", Vec::new()).await,
        Some(Some("unsloth/gemma-3n-E4B-it-GGUF:UD-Q4_K_XL".to_string()))
    );

    // Not exact: the highlighted row is used (a fuzzy-matching list so the row survives).
    let fuzzy = vec![hf("some/model-name", 10.0)];
    assert_eq!(
        confirm_with("model", fuzzy.clone()).await,
        Some(Some("some/model-name".to_string())),
        "no slash"
    );
    assert_eq!(
        confirm_with("some/model name", fuzzy.clone()).await,
        Some(Some("some/model-name".to_string())),
        "a space breaks the pattern"
    );
    assert_eq!(
        confirm_with("some/model:a:b", fuzzy.clone()).await,
        None,
        "two colons break the pattern and nothing is highlighted"
    );
    assert_eq!(
        confirm_with("/model", fuzzy.clone()).await,
        Some(Some("some/model-name".to_string())),
        "an empty owner is not exact, so the highlighted row wins, not the literal query"
    );
    assert_eq!(
        confirm_with("some/", fuzzy.clone()).await,
        Some(Some("some/model-name".to_string())),
        "an empty repository is not exact"
    );
}

/// Confirm with nothing to choose does nothing; cancel closes with `None` (`ui.ts:258-267`).
#[tokio::test]
async fn search_confirm_without_a_choice_and_cancel() {
    let (mut view, _clock) = new_view();
    let search = FakeSearch::new();
    let mut rx = view.show_search(search.as_fn());
    view.handle_key(&enter());
    assert!(rx.try_recv().is_err(), "nothing selected, nothing resolved");
    view.handle_key(&escape());
    assert_eq!(rx.try_recv().unwrap(), None);

    let mut rx = view.show_search(search.as_fn());
    view.handle_key(&ctrl('c'));
    assert_eq!(rx.try_recv().unwrap(), None);
}

/// Closing aborts the request in flight and drops the pending debounce (`ui.ts:235-241`); a search
/// scheduled but not yet fired never fires, and keys after close do nothing.
#[tokio::test]
async fn search_close_aborts_and_clears_the_debounce() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    let (_gate_tx, gate_rx) = oneshot::channel();
    search.reply(Reply::Gate(gate_rx));
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "ab");
    clock.advance(500);
    settle(&mut view).await;
    let token = search.token(0);
    assert!(!token.is_cancelled());
    view.handle_key(&escape());
    assert!(token.is_cancelled(), "close aborts the in-flight request");

    // A debounce pending at close time never fires.
    let search2 = FakeSearch::new();
    let mut rx2 = view.show_search(search2.as_fn());
    type_text(&mut view, "cd");
    view.handle_key(&escape());
    assert_eq!(rx2.try_recv().unwrap(), None);
    clock.advance(10_000);
    settle(&mut view).await;
    assert!(search2.queries().is_empty());
}

/// Shortening the query below two characters aborts the request in flight and cancels the pending
/// debounce, and the status goes back to "Type at least 2 characters" (`ui.ts:193-201`).
#[tokio::test]
async fn search_short_query_cancels_everything_pending() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    let (_gate_tx, gate_rx) = oneshot::channel();
    search.reply(Reply::Gate(gate_rx));
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "ab");
    clock.advance(500);
    settle(&mut view).await;
    let token = search.token(0);
    view.handle_key(&press(OverlayKeyCode::Backspace));
    assert!(token.is_cancelled());
    assert!(snapshot(&mut view, 70).contains(&"  Type at least 2 characters".to_string()));

    // Pending (not yet fired) debounce, then shortened: never fires.
    type_text(&mut view, "c");
    view.handle_key(&press(OverlayKeyCode::Backspace));
    clock.advance(10_000);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["ab"]);
}

/// A failed search shows the error's message under the box (`ui.ts:225-229`); an empty answer reads
/// "No GGUF models found" (`ui.ts:223`). Both clear the previous results.
#[tokio::test]
async fn search_error_and_empty_status() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(vec![hf("ab/ok", 1.0)]);
    search.reply(Reply::Now(Err(LlamaError::Message(
        "Hugging Face rate limit reached".into(),
    ))));
    search.reply_ok(Vec::new());
    let _rx = view.show_search(search.as_fn());

    type_text(&mut view, "ab");
    clock.advance(500);
    settle(&mut view).await;
    assert!(snapshot(&mut view, 70).iter().any(|l| l.contains("ab/ok")));

    type_text(&mut view, "c");
    clock.advance(500);
    settle(&mut view).await;
    let frame = snapshot(&mut view, 70);
    assert!(
        frame.contains(&"  Hugging Face rate limit reached".to_string()),
        "{frame:#?}"
    );
    assert!(
        !frame.iter().any(|l| l.contains("ab/ok")),
        "the results were cleared"
    );

    type_text(&mut view, "d");
    clock.advance(500);
    settle(&mut view).await;
    assert!(snapshot(&mut view, 70).contains(&"  No GGUF models found".to_string()));
}

/// Ten rows are visible, centred on the selection, with an `(i/N)` indicator (`ui.ts:148-173`).
#[tokio::test]
async fn search_shows_ten_rows_with_a_scroll_indicator() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(
        (0..15)
            .map(|i| hf(&format!("owner/model-{i:02}"), 100.0 + f64::from(i)))
            .collect(),
    );
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "mo");
    clock.advance(500);
    settle(&mut view).await;
    let frame = snapshot(&mut view, 70);
    assert_eq!(frame.iter().filter(|l| l.contains("downloads")).count(), 10);
    assert!(frame.contains(&"  (1/15)".to_string()));
    assert!(frame.iter().any(|l| l.contains("model-09")));
    assert!(!frame.iter().any(|l| l.contains("model-10")));

    for _ in 0..7 {
        view.handle_key(&down());
    }
    let frame = snapshot(&mut view, 70);
    assert!(frame.contains(&"  (8/15)".to_string()));
    assert!(frame.iter().any(|l| l.contains("→ owner/model-07")));
    assert!(
        frame.iter().any(|l| l.contains("model-11")),
        "the window slid to rows 2..=11 (7 - 10/2 = 2)"
    );
    assert!(!frame.iter().any(|l| l.contains("model-12")));
    assert!(frame.iter().any(|l| l.contains("model-02")));
    assert!(!frame.iter().any(|l| l.contains("model-01")));

    for _ in 0..8 {
        view.handle_key(&down());
    }
    let frame = snapshot(&mut view, 70);
    assert!(
        frame.contains(&"  (1/15)".to_string()),
        "down from the last wraps to the first"
    );

    // With 10 or fewer, there is no indicator.
    let search = FakeSearch::new();
    search.reply_ok(
        (0..10)
            .map(|i| hf(&format!("owner/model-{i}"), 1.0))
            .collect(),
    );
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "zz");
    clock.advance(500);
    settle(&mut view).await;
    assert!(!snapshot(&mut view, 70).iter().any(|l| l.starts_with("  (")));
}

/// Result rows wrap like `Text` does (`ui.ts:159-166`), never overflowing the frame.
#[tokio::test]
async fn search_long_ids_wrap() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(vec![hf(
        "an-organisation/an-extremely-long-model-repository-name-GGUF",
        4_200.0,
    )]);
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "an");
    clock.advance(500);
    settle(&mut view).await;
    for line in view.render(30) {
        assert!(visible_width(&line.plain_text()) <= 30);
    }
    let rows = snapshot(&mut view, 30);
    assert!(rows.iter().any(|l| l.contains("4.2k")), "{rows:#?}");
}

// =================================================================================================
// The bridge (`showLlamaUi`, `ui.ts:480-492`)
// =================================================================================================

/// A host whose `open_overlay` paints and ticks the overlay in a loop on the calling (blocking)
/// thread, injecting scripted keys once a frame shows a given text, until the overlay asks to close
/// or the script says to walk away.
struct FakeHost {
    accept: bool,
    /// `(wait for this text in a frame, keys to send then)`.
    script: Mutex<VecDeque<(String, Vec<OverlayKey>)>>,
    /// Hang up on the overlay after this many loop turns, if set.
    dismiss_after: Option<usize>,
    frames: Mutex<Vec<Vec<String>>>,
    notices: Mutex<Vec<(String, NotifyKind)>>,
    opened: AtomicBool,
    /// The overlay itself asked to close (`should_close`), as opposed to the host giving up.
    closed_by_overlay: AtomicBool,
}

impl FakeHost {
    fn new(accept: bool, script: Vec<(&str, Vec<OverlayKey>)>) -> Arc<Self> {
        Arc::new(Self {
            accept,
            script: Mutex::new(
                script
                    .into_iter()
                    .map(|(t, k)| (t.to_string(), k))
                    .collect(),
            ),
            dismiss_after: None,
            frames: Mutex::new(Vec::new()),
            notices: Mutex::new(Vec::new()),
            opened: AtomicBool::new(false),
            closed_by_overlay: AtomicBool::new(false),
        })
    }

    fn dismissing(after: usize) -> Arc<Self> {
        Arc::new(Self {
            accept: true,
            script: Mutex::new(VecDeque::new()),
            dismiss_after: Some(after),
            frames: Mutex::new(Vec::new()),
            notices: Mutex::new(Vec::new()),
            opened: AtomicBool::new(false),
            closed_by_overlay: AtomicBool::new(false),
        })
    }

    fn saw(&self, text: &str) -> bool {
        self.frames
            .lock()
            .unwrap()
            .iter()
            .any(|frame| frame.iter().any(|line| line.contains(text)))
    }
}

impl HostServices for FakeHost {
    fn notify(&self, message: &str, kind: NotifyKind) {
        self.notices
            .lock()
            .unwrap()
            .push((message.to_string(), kind));
    }

    fn open_overlay(&self, mut overlay: Box<dyn InteractiveOverlay>) -> bool {
        if !self.accept {
            return false;
        }
        self.opened.store(true, Ordering::SeqCst);
        let mut turns = 0usize;
        loop {
            let frame = plain(&overlay.render(80, 24));
            let next_step = {
                let mut script = self.script.lock().unwrap();
                if script.front().is_some_and(|(wanted, _)| {
                    frame.iter().any(|line| line.contains(wanted.as_str()))
                }) {
                    script.pop_front().map(|(_, keys)| keys)
                } else {
                    None
                }
            };
            self.frames.lock().unwrap().push(frame);
            for key in next_step.into_iter().flatten() {
                let _ = overlay.handle_key(key);
            }
            let _ = overlay.tick();
            if overlay.should_close() {
                self.closed_by_overlay.store(true, Ordering::SeqCst);
                return true;
            }
            turns += 1;
            // A host that is never told to close must not hang the suite: give up after ~5 s.
            if self.dismiss_after.is_some_and(|limit| turns >= limit) || turns >= 2_500 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// Dropping the `show_llama_ui` future (a dispatcher cancel, a session shutting down) while the
/// overlay is open finishes the view, so the host tears the overlay down at its next tick instead
/// of leaving it up, parked in `open_overlay`, with nothing behind it.
#[tokio::test]
async fn dropping_show_llama_ui_while_the_overlay_is_open_closes_the_overlay() {
    let host = FakeHost::new(true, Vec::new());
    let task = tokio::spawn(show_llama_ui(
        host.clone(),
        LlamaKeys::default(),
        |_ui| async move { std::future::pending::<Result<(), String>>().await },
    ));
    for _ in 0..1000 {
        if host.opened.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(host.opened.load(Ordering::SeqCst), "the overlay opened");

    task.abort();
    let _ = task.await;

    // The fake host gives up on its own after ~5 s WITHOUT the overlay asking; closing by the
    // overlay is what the guard buys, and it happens within a few ticks.
    for _ in 0..1000 {
        if host.closed_by_overlay.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        host.closed_by_overlay.load(Ordering::SeqCst),
        "the overlay asked to close once its future was gone"
    );
}

/// A host with no interactive surface answers `false` at once; the command flow must never start
/// (pi's `ctx.ui.custom` does not run its factory without a UI).
#[tokio::test]
async fn show_llama_ui_without_a_surface_does_not_run_the_flow() {
    let host = FakeHost::new(false, Vec::new());
    let ran = Arc::new(AtomicBool::new(false));
    let outcome = show_llama_ui(host.clone(), LlamaKeys::default(), {
        let ran = Arc::clone(&ran);
        move |_ui| async move {
            ran.store(true, Ordering::SeqCst);
            Ok::<(), String>(())
        }
    })
    .await;
    assert_eq!(outcome, LlamaUiOutcome::NoSurface);
    assert!(!ran.load(Ordering::SeqCst));
}

/// The flow runs against the overlay; when it returns, the overlay closes and the call returns
/// `Finished`. What the flow showed was painted.
#[tokio::test]
async fn show_llama_ui_runs_the_flow_and_closes_the_overlay() {
    let host = FakeHost::new(true, Vec::new());
    let outcome = show_llama_ui(host.clone(), LlamaKeys::default(), |ui| async move {
        ui.show_status("Loading model details", "owner/repo");
        // Give the host a few frames to paint it.
        tokio::time::sleep(Duration::from_millis(40)).await;
        Ok::<(), String>(())
    })
    .await;
    assert_eq!(outcome, LlamaUiOutcome::Finished);
    assert!(host.opened.load(Ordering::SeqCst));
    assert!(
        host.closed_by_overlay.load(Ordering::SeqCst),
        "the end of the flow closed the overlay; the host did not have to give up on it"
    );
    assert!(
        host.saw("llama.cpp models"),
        "the initial Loading frame was painted"
    );
    assert!(host.saw("Loading model details"));
    assert!(host.notices.lock().unwrap().is_empty());
}

/// A failing flow is reported with `notify(message, error)` and the overlay still closes
/// (`ui.ts:484-488`).
#[tokio::test]
async fn show_llama_ui_reports_a_failing_flow() {
    let host = FakeHost::new(true, Vec::new());
    let outcome = show_llama_ui(host.clone(), LlamaKeys::default(), |_ui| async move {
        Err::<(), String>("Model failed to load".to_string())
    })
    .await;
    assert_eq!(outcome, LlamaUiOutcome::Finished);
    assert_eq!(
        *host.notices.lock().unwrap(),
        vec![("Model failed to load".to_string(), NotifyKind::Error)]
    );
}

/// The whole path: the host feeds keys into the overlay, the flow's `await`s resolve from them.
#[tokio::test]
async fn show_llama_ui_answers_the_flow_from_host_keys() {
    let host = FakeHost::new(
        true,
        vec![
            ("Download model…", vec![down(), enter()]),
            ("Unload model?", vec![enter()]),
        ],
    );
    let answers = Arc::new(Mutex::new(Vec::<String>::new()));
    let outcome = show_llama_ui(host.clone(), LlamaKeys::default(), {
        let answers = Arc::clone(&answers);
        move |ui| async move {
            let action = ui.show_models("http://h", &three_models()).await;
            match action {
                LlamaManagerAction::Model(model) => {
                    answers.lock().unwrap().push(format!("model:{}", model.id));
                    let yes = ui.confirm("Unload model?", &model.id).await;
                    answers.lock().unwrap().push(format!("confirm:{yes}"));
                }
                other => answers
                    .lock()
                    .unwrap()
                    .push(format!("unexpected:{other:?}")),
            }
            Ok::<(), String>(())
        }
    })
    .await;
    assert_eq!(outcome, LlamaUiOutcome::Finished);
    assert_eq!(
        *answers.lock().unwrap(),
        vec!["model:alpha".to_string(), "confirm:true".to_string()]
    );
}

/// If the host tears the overlay down while the flow is still waiting, the flow is dropped rather
/// than left hanging on an answer nobody can give.
#[tokio::test]
async fn show_llama_ui_drops_a_flow_when_the_host_dismisses_the_overlay() {
    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let host = FakeHost::dismissing(10);
    let outcome = show_llama_ui(host, LlamaKeys::default(), {
        let dropped = Arc::clone(&dropped);
        move |ui| async move {
            let _guard = DropFlag(dropped);
            let _ = ui.select("Stuck", &["a".to_string()]).await;
            Ok::<(), String>(())
        }
    })
    .await;
    assert_eq!(outcome, LlamaUiOutcome::Dismissed);
    assert!(
        dropped.load(Ordering::SeqCst),
        "the flow future was dropped"
    );
}

/// After the view is finished, `LlamaUi` calls answer immediately with their "cancelled" value
/// instead of hanging, and the overlay reports `should_close` (and `Close` on a key).
#[tokio::test]
async fn a_finished_view_answers_immediately_and_asks_to_close() {
    let (mut overlay, ui, _attached) = llama_overlay(
        Arc::new(LlamaKeys::default()),
        ManualClock::new(),
        tokio::runtime::Handle::current(),
    );
    assert!(!overlay.should_close());
    assert_eq!(
        overlay.handle_key(down()),
        cyrup_ext::host::OverlayOutcome::Redraw
    );
    ui.finish();
    assert!(overlay.should_close());
    assert_eq!(
        overlay.handle_key(down()),
        cyrup_ext::host::OverlayOutcome::Close
    );

    let answered = tokio::time::timeout(Duration::from_secs(2), async {
        assert_eq!(
            ui.show_models("http://h", &three_models()).await,
            LlamaManagerAction::Close
        );
        assert_eq!(ui.select("t", &["a".to_string()]).await, None);
        assert!(!ui.confirm("t", "m").await);
        assert_eq!(
            ui.connection_error("http://h", "m").await,
            ConnectionChoice::Close
        );
        assert_eq!(ui.search_models(FakeSearch::new().as_fn()).await, None);
        ui.progress(&state(None, None)).await;
    })
    .await;
    assert!(answered.is_ok(), "no call waited on a user who is gone");
}

/// The overlay's contract with the host: `tick` is `true` only when the next frame differs, and a
/// repaint-worthy event (a key, an update) makes it so exactly once.
#[tokio::test]
async fn tick_reports_a_change_once() {
    let (mut view, _clock) = new_view();
    assert!(view.tick(), "the first frame is new");
    assert!(!view.tick(), "nothing changed since");
    view.show_status("a", "b");
    assert!(view.tick());
    assert!(!view.tick());
    view.handle_key(&down());
    assert!(
        view.tick(),
        "a key always repaints, as pi's requestRender()"
    );
}

/// Result rows: the highlighted row is one accent run; the others are an unstyled id with a muted
/// download count; the `(i/N)` indicator and the status line are dim (`ui.ts:157-179`).
#[tokio::test]
async fn search_rows_are_styled() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(
        (0..12)
            .map(|i| hf(&format!("owner/model-{i:02}"), 1_000.0))
            .collect(),
    );
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "mo");
    clock.advance(500);
    settle(&mut view).await;
    let lines = view.render(70);
    let selected = lines
        .iter()
        .find(|l| l.plain_text().starts_with("→ owner/model-00"))
        .unwrap();
    assert_eq!(
        selected.spans[0].fg,
        Some(OverlayColor::Theme(ThemeRole::Accent))
    );
    assert_eq!(
        selected.spans[0].text, "→ owner/model-00  1.0k downloads",
        "the whole row, details included, is one accent run (the rest is padding)"
    );
    let other = lines
        .iter()
        .find(|l| l.plain_text().starts_with("  owner/model-01"))
        .unwrap();
    assert_eq!(other.spans[0].fg, None);
    assert_eq!(
        other.spans[1].fg,
        Some(OverlayColor::Theme(ThemeRole::Muted))
    );
    assert_eq!(other.spans[1].text.trim(), "1.0k downloads");
    let indicator = lines
        .iter()
        .find(|l| l.plain_text().trim() == "(1/12)")
        .unwrap();
    assert_eq!(
        indicator.spans[0].fg,
        Some(OverlayColor::Theme(ThemeRole::Dim))
    );

    // The status line ("Searching…") is dim too.
    type_text(&mut view, "x");
    let lines = view.render(70);
    let status = lines
        .iter()
        .find(|l| l.plain_text().contains("Searching Hugging Face…"))
        .unwrap();
    assert_eq!(
        status.spans[0].fg,
        Some(OverlayColor::Theme(ThemeRole::Dim))
    );
}

/// The whole bridge future can be driven from a spawned task (a command handler is one), i.e. it
/// is `Send` for a `Send` flow.
#[tokio::test]
async fn show_llama_ui_is_send() {
    let host = FakeHost::new(true, Vec::new());
    let task = tokio::spawn(show_llama_ui(host, LlamaKeys::default(), |ui| async move {
        ui.show_status("a", "b");
        Ok::<(), String>(())
    }));
    assert_eq!(task.await.unwrap(), LlamaUiOutcome::Finished);
}

/// The host repaints the overlay every `REFRESH_MS` (50 ms), the cadence of the debounce check.
#[tokio::test]
async fn the_overlay_asks_the_host_to_repaint_every_fifty_milliseconds() {
    let (overlay, _ui, _attached) = llama_overlay(
        Arc::new(LlamaKeys::default()),
        ManualClock::new(),
        tokio::runtime::Handle::current(),
    );
    assert_eq!(overlay.refresh_ms(), 50);
}

/// Dropping the search box (the view moved on to another screen) aborts its request in flight, so
/// no Hugging Face call outlives the box that asked for it.
#[tokio::test]
async fn replacing_the_search_view_aborts_its_request() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    let (gate_tx, gate_rx) = oneshot::channel();
    search.reply(Reply::Gate(gate_rx));
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "ab");
    clock.advance(500);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["ab"]);
    let token = search.token(0);
    assert!(!token.is_cancelled(), "the request is in flight");

    view.show_status("Loading model details", "owner/repo");
    assert!(
        token.is_cancelled(),
        "the replaced box took its request with it"
    );
    drop(gate_tx);
}

/// Narrowing the results moves a highlight that now points past the end back onto the list, so
/// confirm still answers with a row.
#[tokio::test]
async fn narrowing_the_results_clamps_the_highlight() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(vec![hf("a/one", 1.0), hf("a/two", 2.0), hf("a/three", 3.0)]);
    let mut rx = view.show_search(search.as_fn());
    type_text(&mut view, "a/");
    clock.advance(500);
    settle(&mut view).await;
    view.handle_key(&down());
    view.handle_key(&down());
    assert!(snapshot(&mut view, 70).contains(&"→ a/three  3 downloads".to_string()));

    // `a/ th` is not an exact repository id (it has a space) and keeps only `a/three`.
    type_text(&mut view, " th");
    let frame = snapshot(&mut view, 70);
    assert!(
        frame.contains(&"→ a/three  3 downloads".to_string()),
        "{frame:#?}"
    );
    view.handle_key(&enter());
    assert_eq!(rx.try_recv().unwrap(), Some("a/three".to_string()));
}

/// A select with no options says so, and confirm has nothing to answer with
/// (`select-list.ts`: "No matching commands").
#[tokio::test]
async fn an_empty_select_says_there_is_nothing_to_choose() {
    let (mut view, _clock) = new_view();
    let mut rx = view.show_select("Pick one", &[]);
    let frame = snapshot(&mut view, 60);
    assert!(
        frame.contains(&"  No matching commands".to_string()),
        "{frame:#?}"
    );
    view.handle_key(&enter());
    assert!(rx.try_recv().is_err(), "nothing was chosen");
}

/// A status name with a CRLF in it (an unknown status prints verbatim) reads as one space in the
/// list row (`normalizeToSingleLine`: `[\r\n]+` is one space).
#[tokio::test]
async fn a_description_with_a_crlf_is_one_line() {
    let (mut view, _clock) = new_view();
    let _rx = view.show_models("http://h", &[info("m", "odd\r\nstatus")]);
    let frame = snapshot(&mut view, 80);
    assert!(
        frame
            .iter()
            .any(|l| l.contains("m") && l.contains("odd status")),
        "{frame:#?}"
    );
    assert!(frame.iter().all(|l| !l.contains('\r')), "{frame:#?}");
}

/// The one-column padding of a text line shrinks with the frame (`min(padding, (width - 1) / 2)`):
/// at 1 and 2 columns the text gets every column, at 3 it has a one-column margin.
#[tokio::test]
async fn text_padding_gives_way_on_a_very_narrow_frame() {
    let (mut view, _clock) = new_view();
    view.show_status("Title", "message");
    let one = snapshot(&mut view, 1);
    assert_eq!(&one[1..7], ["T", "i", "t", "l", "e", ""], "{one:#?}");
    let two = snapshot(&mut view, 2);
    assert_eq!(&two[1..4], ["Ti", "tl", "e"], "{two:#?}");
    let three = snapshot(&mut view, 3);
    assert_eq!(&three[1..3], [" T", " i"], "{three:#?}");
}

// =================================================================================================
// Bracketed paste (`input.ts:62-98`, `:396-406`)
// =================================================================================================

/// `handlePaste` drops line breaks, turns tabs into four spaces and inserts at the cursor as one
/// undoable step.
#[test]
fn a_paste_into_the_input_is_cleaned_inserted_at_the_cursor_and_undoable() {
    let keys = LlamaKeys::default();
    let mut input = TextInput::new();
    for c in "ab".chars() {
        input.handle_key(&keys, &chr(c));
    }
    input.handle_key(&keys, &press(OverlayKeyCode::Left));

    input.handle_paste("x\r\ny\rz\n\tw");

    assert_eq!(input.value(), "axyz    wb");
    // The cursor sits after the pasted text: the next character lands there.
    input.handle_key(&keys, &chr('!'));
    assert_eq!(input.value(), "axyz    w!b");
    // One undo step takes back the typed `!`, the next the whole paste.
    input.handle_key(&keys, &ctrl('-'));
    assert_eq!(input.value(), "axyz    wb");
    input.handle_key(&keys, &ctrl('-'));
    assert_eq!(input.value(), "ab");
}

/// A paste into the Hugging Face search box reaches the search input, which searches for it
/// like for typed text: the main entry path (`owner/repository[:quant]`).
#[tokio::test]
async fn a_paste_into_the_search_box_fills_it_and_starts_the_search() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(vec![hf("unsloth/Qwen3-8B-GGUF", 10.0)]);
    let _rx = view.show_search(search.as_fn());

    view.handle_paste("unsloth/Qwen3-8B-GGUF\n");

    let frame = snapshot(&mut view, 70);
    assert!(
        frame.contains(&"> unsloth/Qwen3-8B-GGUF".to_string()),
        "{frame:#?}"
    );
    assert!(
        frame.contains(&"  Searching Hugging Face…".to_string()),
        "{frame:#?}"
    );
    clock.advance(500);
    settle(&mut view).await;
    assert_eq!(search.queries(), vec!["unsloth/Qwen3-8B-GGUF"]);
}

/// A paste while no text field is showing is ignored (upstream's select lists ignore data that is
/// no key); and a finished view ignores it too.
#[tokio::test]
async fn a_paste_is_ignored_where_there_is_no_text_field() {
    let (mut view, _clock) = new_view();
    let _rx = view.show_models("http://h", &three_models());
    let before = snapshot(&mut view, 60);
    view.handle_paste("pasted text");
    assert_eq!(snapshot(&mut view, 60), before);
}

/// The bridge the host drives: a paste is routed to the view and asks for a repaint, and a finished
/// view asks the host to close.
#[tokio::test]
async fn the_overlay_routes_a_paste_to_the_search_box() {
    let (mut overlay, ui, _attached) = llama_overlay(
        Arc::new(LlamaKeys::default()),
        ManualClock::new(),
        tokio::runtime::Handle::current(),
    );
    let search = FakeSearch::new();
    let searching = tokio::spawn({
        let ui = ui.clone();
        let search = search.as_fn();
        async move { ui.search_models(search).await }
    });
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    assert_eq!(
        overlay.handle_paste("owner/repo"),
        cyrup_ext::host::OverlayOutcome::Redraw
    );
    let frame = plain(&overlay.render(70, 40));
    assert!(frame.contains(&"> owner/repo".to_string()), "{frame:#?}");

    ui.finish();
    assert_eq!(
        overlay.handle_paste("more"),
        cyrup_ext::host::OverlayOutcome::Close
    );
    searching.abort();
}

// =================================================================================================
// A host frame shorter than the natural one
// =================================================================================================

fn many_models(count: usize) -> Vec<LlamaModelInfo> {
    (0..count)
        .map(|index| info(&format!("model-{index:02}"), "unloaded"))
        .collect()
}

/// The models frame is 20 rows at its natural size; a host that clips to fewer rows must lose list
/// rows, not the footer and the bottom border.
#[tokio::test]
async fn the_models_frame_shrinks_its_list_to_fit_the_hosts_rows() {
    let (mut view, _clock) = new_view();
    let _rx = view.show_models("http://h", &many_models(30));
    let natural = view.render(60);
    assert_eq!(natural.len(), 20, "the natural frame");

    // Nine rows is the floor: borders, title, url, spacers, the footer and a one-row list.
    for rows in 9..=20 {
        let lines = plain(&view.render_within(60, Some(rows)));
        assert!(lines.len() <= rows, "{rows} rows: {} lines", lines.len());
        assert_eq!(
            lines.last().unwrap(),
            &rule(60),
            "{rows} rows: bottom border"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("enter load/unload/download")),
            "{rows} rows: the key hints are kept\n{lines:#?}"
        );
    }
    // Plenty of room: nothing is taken away.
    assert_eq!(view.render_within(60, Some(40)), natural);
}

/// The same for the search frame, including the extra status row while searching.
#[tokio::test]
async fn the_search_frame_shrinks_its_results_to_fit_the_hosts_rows() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(
        (0..15)
            .map(|index| hf(&format!("owner/model-{index:02}"), 100.0))
            .collect(),
    );
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "mo");
    clock.advance(500);
    settle(&mut view).await;
    assert_eq!(view.render(70).len(), 20, "the natural frame");

    // Eleven rows is the floor here: the frame, the input and a one-row result list.
    for rows in 11..=20 {
        let lines = plain(&view.render_within(70, Some(rows)));
        assert!(lines.len() <= rows, "{rows} rows: {} lines", lines.len());
        assert_eq!(
            lines.last().unwrap(),
            &rule(70),
            "{rows} rows: bottom border"
        );
        assert!(
            lines.iter().any(|l| l.contains("enter select")),
            "{rows} rows: the key hints are kept\n{lines:#?}"
        );
    }
}

/// The bridge sizes the frame against what the host will keep: the box is at most
/// `OverlayOptions::max_rows(height)` tall, so on a 24-row terminal (20 rows) and shorter ones the
/// footer survives the host's clip.
#[tokio::test]
async fn the_overlay_fits_the_box_the_host_allows_for_its_frame_height() {
    let (mut overlay, ui, _attached) = llama_overlay(
        Arc::new(LlamaKeys::default()),
        ManualClock::new(),
        tokio::runtime::Handle::current(),
    );
    let showing = tokio::spawn({
        let ui = ui.clone();
        async move { ui.show_models("http://h", &many_models(30)).await }
    });
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    for height in [14u16, 18, 24, 30, 50] {
        let allowed = usize::from(cyrup_ext::host::OverlayOptions::default().max_rows(height));
        let lines = plain(&overlay.render(70, usize::from(height)));
        assert!(
            lines.len() <= allowed,
            "height {height}: {} lines, the host keeps {allowed}",
            lines.len()
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("enter load/unload/download")),
            "height {height}: the host's clip must not take the footer\n{lines:#?}"
        );
    }
    showing.abort();
}

// =================================================================================================
// Progress updates are patches (`Object.assign`, `ui.ts:510`)
// =================================================================================================

fn progress_state(ratio: Option<f64>, detail: Option<&str>) -> ProgressState {
    ProgressState {
        title: "Downloading model".into(),
        model: "m".into(),
        message: "Downloading model".into(),
        ratio,
        detail: detail.map(str::to_string),
    }
}

/// A message-only update (`onProgress({ message: "Loading model" })`) keeps the bar and detail the
/// SSE watcher delivered; an update that carries the keys replaces them, `undefined` included.
#[test]
fn a_message_only_progress_update_keeps_the_ratio_and_detail() {
    let mut shown = progress_state(Some(0.5), Some("512 B / 1.00 KiB"));

    shown.apply(LlamaProgress::message("Loading model"));

    assert_eq!(shown.message, "Loading model");
    assert_eq!(shown.ratio, Some(0.5));
    assert_eq!(shown.detail.as_deref(), Some("512 B / 1.00 KiB"));
}

/// A load update is `{ message, ratio }` (`client.ts:107-110`): the ratio key is always there
/// (cleared when the server sent no value) and the detail key never is.
#[test]
fn a_load_progress_update_replaces_the_ratio_and_keeps_the_detail() {
    let mut shown = progress_state(Some(0.5), Some("detail"));

    shown.apply(LlamaProgress {
        message: "Loading text model".into(),
        ratio: ProgressField::Set(0.25),
        detail: ProgressField::Keep,
    });
    assert_eq!(shown.ratio, Some(0.25));
    assert_eq!(shown.detail.as_deref(), Some("detail"));

    shown.apply(LlamaProgress {
        message: "Loading model".into(),
        ratio: ProgressField::Clear,
        detail: ProgressField::Keep,
    });
    assert_eq!(shown.ratio, None, "an own `ratio: undefined` clears it");
    assert_eq!(shown.detail.as_deref(), Some("detail"));
}

/// A download update carries all three keys and replaces all three.
#[test]
fn a_download_progress_update_replaces_everything() {
    let mut shown = progress_state(None, None);

    shown.apply(LlamaProgress {
        message: "Downloading model".into(),
        ratio: ProgressField::Set(0.75),
        detail: ProgressField::Set("3 B / 4 B".into()),
    });

    assert_eq!(shown.ratio, Some(0.75));
    assert_eq!(shown.detail.as_deref(), Some("3 B / 4 B"));
}

// =================================================================================================
// The pointer (`select-list.ts` / `input.ts` `handleMouse`)
// =================================================================================================

fn wheel(row: u16, lines: i32) -> OverlayMouse {
    OverlayMouse::Wheel {
        column: 4,
        row,
        lines,
    }
}
fn mouse_press(row: u16) -> OverlayMouse {
    OverlayMouse::Press { column: 4, row }
}
fn mouse_click(row: u16) -> OverlayMouse {
    OverlayMouse::Click {
        column: 4,
        row,
        count: 1,
    }
}

/// The row a text was painted on, as a pointer coordinate.
fn row_of(view: &mut LlamaView, width: usize, text: &str) -> u16 {
    let lines = snapshot(view, width);
    let row = lines
        .iter()
        .position(|l| l.contains(text))
        .unwrap_or_else(|| panic!("{text:?} is not on screen: {lines:#?}"));
    u16::try_from(row).unwrap()
}

/// The text of the highlighted (`→`) row.
fn highlighted(view: &mut LlamaView, width: usize) -> String {
    snapshot(view, width)
        .into_iter()
        .find(|l| l.starts_with('→'))
        .unwrap_or_default()
}

/// The wheel moves the model list one row per notch and CLAMPS: the keys wrap from the first row
/// to the last, the wheel stops at the ends.
#[tokio::test]
async fn pointer_wheel_moves_the_model_list_one_row_and_clamps() {
    let (mut view, _clock) = new_view();
    let _rx = view.show_models("http://h", &three_models());
    let first = row_of(&mut view, 100, "mid");
    assert!(highlighted(&mut view, 100).contains("mid"));

    assert!(view.handle_mouse(wheel(first, -1)), "the list takes it");
    assert!(
        highlighted(&mut view, 100).contains("mid"),
        "the wheel never wraps from the first row to the last"
    );
    view.handle_mouse(wheel(first, 1));
    assert!(highlighted(&mut view, 100).contains("alpha"));
    for _ in 0..9 {
        view.handle_mouse(wheel(first, 1));
    }
    assert!(
        highlighted(&mut view, 100).contains("Download model"),
        "the wheel stops at the last row: {:?}",
        highlighted(&mut view, 100)
    );
}

/// A press selects the row under the pointer without activating it; the click activates it
/// (`SelectList.handleMouse`: select on press, `onSelect` on click).
#[tokio::test]
async fn pointer_press_selects_and_click_activates_a_model_row() {
    let (mut view, _clock) = new_view();
    let mut rx = view.show_models("http://h", &three_models());
    let zeta = row_of(&mut view, 100, "zeta");

    assert!(view.handle_mouse(mouse_press(zeta)));
    assert!(highlighted(&mut view, 100).contains("zeta"));
    assert!(
        rx.try_recv().is_err(),
        "a press only selects: nothing is answered yet"
    );

    assert!(view.handle_mouse(mouse_click(zeta)));
    match rx.try_recv().unwrap() {
        LlamaManagerAction::Model(model) => assert_eq!(model.id, "zeta"),
        other => panic!("expected the zeta model, got {other:?}"),
    }
}

/// The click activates the row the PRESS went down on, not whatever the pointer is over by the time
/// the release is reported.
#[tokio::test]
async fn pointer_click_activates_the_pressed_row() {
    let (mut view, _clock) = new_view();
    let mut rx = view.show_models("http://h", &three_models());
    let alpha = row_of(&mut view, 100, "alpha");
    let zeta = row_of(&mut view, 100, "zeta");
    view.handle_mouse(mouse_press(alpha));
    view.handle_mouse(mouse_click(zeta));
    match rx.try_recv().unwrap() {
        LlamaManagerAction::Model(model) => assert_eq!(model.id, "alpha"),
        other => panic!("expected the pressed row, got {other:?}"),
    }
}

/// "Download model…" is a row like any other: clicking it is Enter on it.
#[tokio::test]
async fn pointer_click_on_the_download_row_asks_to_download() {
    let (mut view, _clock) = new_view();
    let mut rx = view.show_models("http://h", &three_models());
    let download = row_of(&mut view, 100, "Download model");
    view.handle_mouse(mouse_click(download));
    assert_eq!(rx.try_recv().unwrap(), LlamaManagerAction::Download);
}

/// Chrome is not a row: a click on the border, the title, the server url or the footer answers
/// nothing and reports the event unhandled.
#[tokio::test]
async fn pointer_over_chrome_answers_nothing() {
    let (mut view, _clock) = new_view();
    let mut rx = view.show_models("http://h", &three_models());
    let footer = row_of(&mut view, 100, "enter");
    for row in [0, 1, 2, 3, footer] {
        assert!(!view.handle_mouse(mouse_press(row)), "row {row} is chrome");
        assert!(!view.handle_mouse(mouse_click(row)), "row {row} is chrome");
        assert!(!view.handle_mouse(wheel(row, 1)), "row {row} is chrome");
    }
    assert!(rx.try_recv().is_err());
    assert!(highlighted(&mut view, 100).contains("mid"));
}

/// A list longer than its window scrolls with the highlight; the click lands on the row that is
/// DRAWN under the pointer, not on the row at the same offset from the top of the list.
#[tokio::test]
async fn pointer_hits_the_row_drawn_in_a_scrolled_window() {
    let (mut view, _clock) = new_view();
    let options: Vec<String> = (0..30).map(|i| format!("option-{i:02}")).collect();
    let mut rx = view.show_select("Pick", &options);
    // Render once so the view knows where its rows are, then scroll the window.
    let first = row_of(&mut view, 60, "option-00");
    for _ in 0..20 {
        view.handle_mouse(wheel(first, 1));
        let _ = view.render(60);
    }
    let row = row_of(&mut view, 60, "option-22");
    view.handle_mouse(mouse_click(row));
    assert_eq!(rx.try_recv().unwrap().as_deref(), Some("option-22"));
}

/// A title that wraps pushes every row down; the pointer follows what was painted.
#[tokio::test]
async fn pointer_follows_rows_pushed_down_by_a_wrapped_title() {
    let (mut view, _clock) = new_view();
    let title = "a very long title that has to wrap onto several rows at this narrow width";
    let mut rx = view.show_select(title, &["first".to_string(), "second".to_string()]);
    let narrow = 30;
    let second = row_of(&mut view, narrow, "second");
    assert!(second > 5, "the title wrapped: row {second}");
    view.handle_mouse(mouse_click(second));
    assert_eq!(rx.try_recv().unwrap().as_deref(), Some("second"));
}

/// Through the bridge the host drives: a click inside a confirm dialog answers it, and the outcome
/// the host gets is a redraw (a miss on chrome is `Ignored`).
#[tokio::test]
async fn pointer_through_the_overlay_answers_a_confirm_dialog() {
    let (mut overlay, ui, _attached) = llama_overlay(
        Arc::new(LlamaKeys::default()),
        ManualClock::new(),
        tokio::runtime::Handle::current(),
    );
    let mut confirm = Box::pin(ui.confirm("Unload model?", "qwen"));
    assert!(futures::poll!(confirm.as_mut()).is_pending());
    let frame = plain(&overlay.render(60, 24));
    assert_eq!(frame[5], "  No");

    assert_eq!(
        overlay.handle_mouse(mouse_click(1)),
        OverlayMouseOutcome::Unhandled,
        "the title is not a row"
    );
    assert_eq!(
        overlay.handle_mouse(mouse_click(5)),
        OverlayMouseOutcome::Redraw,
        "the No row took the click"
    );
    assert!(!confirm.await, "clicking No answers the confirm with false");
}

/// A press in the search box puts the caret at the start of the grapheme under the pointer
/// (`Input.handleMouse`).
#[test]
fn pointer_press_places_the_caret_by_grapheme() {
    let keys = LlamaKeys::default();
    let place = |text: &str, column: usize, first_col: usize| {
        let mut input = input_with(text);
        input.press_at(column, first_col);
        input.handle_key(&keys, &chr('|'));
        input.value().to_string()
    };
    // The prompt is two cells; the text starts at column 2.
    assert_eq!(place("abcd", 2, 0), "|abcd", "on the first cell");
    assert_eq!(place("abcd", 4, 0), "ab|cd", "on `c`");
    assert_eq!(
        place("abcd", 0, 0),
        "|abcd",
        "a press on the prompt is the start"
    );
    assert_eq!(place("abcd", 40, 0), "abcd|", "past the text is the end");
    // A double-width grapheme is one target, whichever of its two cells is hit.
    assert_eq!(place("ab👍cd", 4, 0), "ab|👍cd");
    assert_eq!(place("ab👍cd", 5, 0), "ab|👍cd");
    assert_eq!(place("ab👍cd", 6, 0), "ab👍|cd");
    // A combining sequence is never split.
    assert_eq!(place("e\u{301}x", 3, 0), "e\u{301}|x");
    // A scrolled field: its left edge shows text column 10.
    assert_eq!(place("0123456789abcdef", 2, 10), "0123456789|abcdef");
}

/// The press reads the column the field was PAINTED at: a long query scrolls horizontally, and the
/// same screen column then means a different character.
#[test]
fn pointer_press_uses_the_columns_the_field_was_painted_with() {
    let keys = LlamaKeys::default();
    let mut input = input_with("0123456789abcdefghijklmnopqrstuvwxyz");
    let (line, first_col) = input.render_at(14);
    assert!(first_col > 0, "the field scrolled: {first_col}");
    let shown: String = line.plain_text().chars().skip(2).collect();
    let expected_char = shown.chars().next().unwrap();
    input.press_at(2, first_col);
    input.handle_key(&keys, &chr('|'));
    assert!(
        input.value().contains(&format!("|{expected_char}")),
        "the caret went in front of the first visible character {expected_char:?}: {:?}",
        input.value()
    );
}

/// In the search view a press on the box moves the caret, and what is typed next lands there. The
/// click that completes the gesture does not move it again, and only the box's own row takes it.
#[tokio::test]
async fn pointer_press_in_the_search_box_moves_the_caret() {
    let (mut view, _clock) = new_view();
    let search = FakeSearch::new();
    let _rx = view.show_search(search.as_fn());
    type_text(&mut view, "own");
    let input_row = row_of(&mut view, 70, "> own");

    // Not the box's row: the hint above it and the spacer below it place nothing.
    for row in [input_row - 1, input_row + 1] {
        assert!(!view.handle_mouse(OverlayMouse::Press { column: 3, row }));
    }

    assert!(view.handle_mouse(OverlayMouse::Press {
        column: 3,
        row: input_row,
    }));
    // The release lands somewhere else in the row's text: a click after a handled press is inert.
    view.handle_mouse(OverlayMouse::Click {
        column: 5,
        row: input_row,
        count: 1,
    });
    view.handle_key(&chr('X'));
    assert_eq!(snapshot(&mut view, 70)[usize::from(input_row)], "> oXwn");
}

/// The search results answer the pointer as a list does: the wheel moves the highlight (clamped),
/// a press selects a result, a click confirms the result it landed on.
#[tokio::test]
async fn pointer_drives_the_search_results() {
    let (mut view, clock) = new_view();
    let search = FakeSearch::new();
    search.reply_ok(vec![
        hf("owner/one", 10.0),
        hf("owner/two", 20.0),
        hf("owner/three", 30.0),
    ]);
    let mut rx = view.show_search(search.as_fn());
    type_text(&mut view, "own");
    clock.advance(500);
    settle(&mut view).await;

    let one = row_of(&mut view, 70, "owner/one");
    assert!(highlighted(&mut view, 70).contains("owner/one"));
    view.handle_mouse(wheel(one, 1));
    assert!(highlighted(&mut view, 70).contains("owner/two"));
    view.handle_mouse(wheel(one, -1));
    view.handle_mouse(wheel(one, -1));
    assert!(
        highlighted(&mut view, 70).contains("owner/one"),
        "clamped at the first result"
    );

    let three = row_of(&mut view, 70, "owner/three");
    view.handle_mouse(mouse_press(three));
    assert!(highlighted(&mut view, 70).contains("owner/three"));
    assert!(rx.try_recv().is_err(), "a press only selects");
    view.handle_mouse(mouse_click(three));
    assert_eq!(rx.try_recv().unwrap().as_deref(), Some("owner/three"));
}

/// Every style run names a theme ROLE, not a fixed colour: this is what lets the host paint the
/// view in the user's theme.
#[tokio::test]
async fn the_view_colours_its_runs_by_theme_role_not_by_fixed_colour() {
    let (mut view, _clock) = new_view();
    let _rx = view.show_models("http://h", &three_models());
    for line in view.render(100) {
        for span in &line.spans {
            assert!(
                matches!(span.fg, None | Some(OverlayColor::Theme(_))),
                "{:?} carries a literal colour: {:?}",
                span.text,
                span.fg
            );
            assert!(!span.dim, "dimness is the `dim` role, not the SGR modifier");
        }
    }
}
