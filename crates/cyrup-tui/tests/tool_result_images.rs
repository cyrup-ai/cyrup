//! Tool-result **image** content blocks (TUI-007; `tool-execution.ts:324-350` +
//! `getTextOutput`/`imageFallback`, render-utils.ts:49-59 / terminal-image.ts:546-558).
//!
//! A tool that returns `{type:"image", data, mimeType}` — `read` on a PNG is the built-in case
//! (`core/tools/read.ts:247-263`) — used to render as the literal string `[image]`. Pi instead draws
//! the picture, and falls back to `[Image: [{mime}] {w}x{h}]` when images are off or the terminal
//! cannot show them.
//!
//! These tests drive the real `App::ingest_event` tool seam and read the committed `insert_before`
//! scrollback and the live viewport, so they assert what is actually painted:
//!
//! * pixels — the half-block raster writes `▀` cells carrying the image's colours;
//! * the `showImages=false` fallback wording;
//! * that the literal `[image]` placeholder is gone.
//!
//! **Known limit, stated rather than papered over**: cyrup rasterizes tool-result images with the
//! portable Unicode half-block protocol, not the negotiated Kitty/iTerm2 one. Those protocols plant
//! an escape sequence inside a terminal cell, and cyrup's transcript re-wraps its rendered lines
//! (`Paragraph … .wrap()`, and `Terminal::insert_before` for scrollback), which would corrupt such a
//! sequence. The upside over Pi is that the image survives into native scrollback instead of
//! degrading to text once the turn commits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use base64::Engine as _;
use cyrup_session_svc::AgentSessionEvent;
use cyrup_tui::{App, UiTheme};
use image::{Rgba, RgbaImage};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

const RED: Color = Color::Rgb(220, 30, 30);

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap()
}

/// A solid-red PNG of `w×h` pixels, base64-encoded exactly as a tool result carries it.
fn red_png_base64(w: u32, h: u32) -> String {
    let mut img = RgbaImage::new(w, h);
    for px in img.pixels_mut() {
        *px = Rgba([220, 30, 30, 255]);
    }
    let mut bytes: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The `read`-an-image result shape (`read.ts:256-262`): a text note plus the image block.
fn image_result(data: &str) -> serde_json::Value {
    serde_json::json!({
        "content": [
            { "type": "text", "text": "Read image file [image/png]" },
            { "type": "image", "data": data, "mimeType": "image/png" },
        ]
    })
}

/// Drive one finished `read` tool through the real event seam.
fn run_read_tool(app: &mut App<TestBackend>, result: serde_json::Value) {
    app.ingest_event(&AgentSessionEvent::ToolExecutionStart {
        tool_call_id: cyrup_core::ToolCallId::from("call_1"),
        tool_name: "read".into(),
        args: serde_json::json!({ "file_path": "/tmp/shot.png" }),
    });
    app.ingest_event(&AgentSessionEvent::ToolExecutionEnd {
        tool_call_id: cyrup_core::ToolCallId::from("call_1"),
        tool_name: "read".into(),
        is_error: false,
        result,
    });
}

/// Any buffer cell painted in the test image's red — proof real pixels landed, not text.
fn painted_red(app: &App<TestBackend>) -> bool {
    app.terminal().backend().buffer().content().iter().any(|c| c.fg == RED || c.bg == RED)
}

/// Any committed scrollback span painted in the test image's red.
fn scrollback_painted_red(app: &App<TestBackend>) -> bool {
    app.scrollback_lines()
        .iter()
        .flat_map(|l| l.spans.iter())
        .any(|s| s.style.fg == Some(RED) || s.style.bg == Some(RED))
}

/// With `showImages` on (the default), a tool-result image rasterizes into real cells — and the old
/// literal `[image]` placeholder is gone.
#[test]
fn tool_result_image_renders_inline_instead_of_the_literal_placeholder() {
    let mut app = new_app();
    run_read_tool(&mut app, image_result(&red_png_base64(24, 24)));
    app.draw().unwrap();

    assert!(painted_red(&app), "the tool-result image must paint real cells:\n{}", live_text(&app));
    let out = app.scrollback_text();
    assert!(!out.contains("[image]"), "the literal `[image]` placeholder must be gone; got:\n{out}");
}

/// A half-block raster is ordinary cells, so it survives the commit into native scrollback.
#[test]
fn a_committed_tool_result_image_keeps_its_pixels_in_scrollback() {
    let mut app = new_app();
    run_read_tool(&mut app, image_result(&red_png_base64(24, 24)));
    // End the turn: the live tool block commits and flushes through `insert_before`.
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: Vec::new(), will_retry: false });
    app.draw().unwrap();

    assert!(
        scrollback_painted_red(&app),
        "the committed tool block must keep the rasterized image; got:\n{}",
        app.scrollback_text()
    );
}

/// `showImages = false` swaps the raster for Pi's `imageFallback` wording
/// (`[Image: [{mimeType}] {w}x{h}]`, terminal-image.ts:546-558).
#[test]
fn show_images_off_renders_pis_image_fallback_text() {
    let mut app = new_app();
    app.state_mut().transcript.set_show_images(false);
    run_read_tool(&mut app, image_result(&red_png_base64(24, 24)));
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: Vec::new(), will_retry: false });
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(
        out.contains("[Image: [image/png] 24x24]"),
        "images-off must show Pi's fallback indicator; got:\n{out}"
    );
    assert!(!scrollback_painted_red(&app), "images-off must not paint the raster");
    assert!(!out.contains("[image]"), "the old literal placeholder must be gone; got:\n{out}");
}

/// An image block whose payload cannot be decoded still tells the user an image came back, rather
/// than silently vanishing.
#[test]
fn an_undecodable_image_block_falls_back_to_text() {
    let mut app = new_app();
    run_read_tool(
        &mut app,
        serde_json::json!({
            "content": [{ "type": "image", "data": "not-base64-!!", "mimeType": "image/png" }]
        }),
    );
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: Vec::new(), will_retry: false });
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(
        out.contains("[Image: [image/png]]"),
        "an undecodable block still gets the indicator (no dimensions); got:\n{out}"
    );
}

/// The text blocks of the result keep rendering alongside the image (Pi joins the `text` blocks and
/// adds the image separately).
#[test]
fn the_text_part_of_an_image_result_still_renders() {
    let mut app = new_app();
    app.state_mut().transcript.tool_expanded = true;
    run_read_tool(&mut app, image_result(&red_png_base64(16, 16)));
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: Vec::new(), will_retry: false });
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(out.contains("Read image file [image/png]"), "the text note still renders; got:\n{out}");
}

/// The `[Image: …]` indicator reports the SOURCE pixel size, even though the raster the renderer
/// keeps is downscaled to bound per-frame work (Pi reads `getImageDimensions` off the raw data).
#[test]
fn the_fallback_reports_the_source_dimensions_of_a_large_image() {
    let mut app = new_app();
    app.state_mut().transcript.set_show_images(false);
    run_read_tool(&mut app, image_result(&red_png_base64(2048, 1024)));
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: Vec::new(), will_retry: false });
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(
        out.contains("[Image: [image/png] 2048x1024]"),
        "the indicator must report the SOURCE size, not the bounded raster; got:\n{out}"
    );
}

/// `terminal.imageWidthCells` clamps the raster width (Pi `maxWidthCells`, tool-execution.ts:348).
#[test]
fn image_width_cells_clamps_the_raster() {
    let mut narrow = new_app();
    narrow.state_mut().transcript.set_image_width_cells(4);
    run_read_tool(&mut narrow, image_result(&red_png_base64(64, 64)));
    narrow.ingest_event(&AgentSessionEvent::AgentEnd { messages: Vec::new(), will_retry: false });
    narrow.draw().unwrap();

    let mut wide = new_app();
    wide.state_mut().transcript.set_image_width_cells(40);
    run_read_tool(&mut wide, image_result(&red_png_base64(64, 64)));
    wide.ingest_event(&AgentSessionEvent::AgentEnd { messages: Vec::new(), will_retry: false });
    wide.draw().unwrap();

    let cells = |app: &App<TestBackend>| -> usize {
        app.scrollback_lines()
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(RED) || s.style.bg == Some(RED))
            .map(|s| s.content.chars().count())
            .sum()
    };
    let (n, w) = (cells(&narrow), cells(&wide));
    assert!(n > 0 && w > 0, "both rasters painted ({n} vs {w})");
    assert!(w > n, "a wider imageWidthCells must paint more cells: {w} vs {n}");
}

fn live_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}
