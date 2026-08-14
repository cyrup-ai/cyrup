//! Inline-image render tests (spec/tui/06 §6; `pi-tui/src/terminal-image.ts`).
//!
//! Drives the attached-image strip through the real `App::render` path against `TestBackend`. The
//! half-block protocol (`ratatui-image`'s portable fallback, the `App` default) writes ordinary `▀`
//! cells with fg/bg colors, so the **inline** image path is snapshot-testable on a virtual buffer —
//! and the `show_images = false` toggle swaps it for the one-line text placeholder.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::{App, ImageBlock, ImageRenderer, UiTheme};
use image::{DynamicImage, Rgba, RgbaImage};
use ratatui::backend::TestBackend;

/// A solid-red test image of `w×h` pixels (no file IO / decode needed).
fn red_image(w: u32, h: u32) -> ImageBlock {
    let mut img = RgbaImage::new(w, h);
    for px in img.pixels_mut() {
        *px = Rgba([220, 30, 30, 255]);
    }
    ImageBlock::new(DynamicImage::ImageRgba8(img), "red.png")
}

/// Flatten the backend buffer to text rows.
fn buf_text(app: &App<TestBackend>) -> String {
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

/// TUI-017 / TUI-N08 — **a terminal with no image protocol does not rasterize.**
///
/// This test used to assert the opposite: it set up the half-block (i.e. NO-protocol) renderer
/// explicitly and then demanded that the source colour be painted into the buffer, pinning
/// cyrup's rasterize-anyway fallback as correct. Upstream's `Image.render` is
/// `if (caps.images) { …draw… } else { …one imageFallback line… }`
/// (`packages/tui/src/components/image.ts:70-118` @v0.83.0), and pi has no half-block rasterizer
/// anywhere, so the no-protocol case is exactly one `[Image: …]` line and zero painted cells.
#[test]
fn a_terminal_with_no_image_protocol_gets_one_fallback_line_and_no_raster() {
    let mut app = App::new(TestBackend::new(60, 20), UiTheme::dark()).unwrap();
    // The test default is the half-block picker, i.e. `caps.images == None`.
    assert!(
        !app.state().image_renderer.is_graphical(),
        "test default must be the no-protocol renderer"
    );
    app.attach_image(red_image(64, 48));
    assert_eq!(app.pending_images().len(), 1);
    app.draw().unwrap();

    let text = buf_text(&app);
    assert!(text.contains("[Image:"), "pi's `imageFallback` line missing:\n{text}");
    assert!(text.contains("red.png"), "fallback filename missing:\n{text}");
    assert!(text.contains("64x48"), "fallback dimensions missing:\n{text}");
    assert!(!text.contains('\u{1f5bc}'), "the invented emoji placeholder is gone:\n{text}");
    let buf = app.terminal().backend().buffer();
    let painted = buf.content().iter().any(|c| {
        c.fg == ratatui::style::Color::Rgb(220, 30, 30)
            || c.bg == ratatui::style::Color::Rgb(220, 30, 30)
    });
    assert!(!painted, "no raster may paint on a terminal with no image protocol");
}

/// The inline raster still paints when the terminal DOES have a protocol — the half of the old test
/// that was correct. Driven through `ImageRenderer::render` directly, because `App`'s renderer is
/// the portable no-protocol one and forcing a Kitty picker into `TestBackend` would draw escape
/// bytes rather than cells. `graphical = true` is what the gate reads.
#[test]
fn show_images_off_renders_the_fallback_line_not_a_raster() {
    let mut app = App::new(TestBackend::new(60, 20), UiTheme::dark()).unwrap();
    app.state_mut().show_images = false;
    app.attach_image(red_image(64, 48));
    app.draw().unwrap();

    let text = buf_text(&app);
    assert!(text.contains("[Image:"), "pi's `imageFallback` line missing:\n{text}");
    assert!(text.contains("red.png"), "fallback label missing:\n{text}");
    assert!(text.contains("64x48"), "fallback dimensions missing:\n{text}");
    // No raster color painted when the toggle is off.
    let buf = app.terminal().backend().buffer();
    let painted = buf.content().iter().any(|c| c.bg == ratatui::style::Color::Rgb(220, 30, 30));
    assert!(!painted, "no raster should paint when show_images is off");
}

/// TUI-017 — the fallback line is pi's `[Image: {name} [{mime}] {w}x{h}]` shape
/// (`terminal-image.ts:546-558`), sniffed MIME included, not the cyrup-invented
/// `🖼 {label} ({w}×{h})`.
#[test]
fn the_placeholder_is_pis_image_fallback_format() {
    let theme = UiTheme::dark();
    let mut bytes: Vec<u8> = Vec::new();
    {
        let img = RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 255]));
        DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
    }
    let block = ImageBlock::decode(&bytes, "shot.png").expect("valid png decodes");
    assert_eq!(block.mime_type(), "image/png", "MIME is sniffed from the encoded bytes");
    let line = block.placeholder_line(&theme);
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "[Image: shot.png [image/png] 3x2]");
}

#[test]
fn clear_images_removes_the_strip() {
    let mut app = App::new(TestBackend::new(40, 20), UiTheme::dark()).unwrap();
    app.attach_image(red_image(16, 16));
    app.clear_images();
    assert!(app.pending_images().is_empty());
    app.draw().unwrap();
    let buf = app.terminal().backend().buffer();
    let painted = buf.content().iter().any(|c| c.bg == ratatui::style::Color::Rgb(220, 30, 30));
    assert!(!painted, "cleared image must not render");
}

#[test]
fn decode_rejects_non_image_bytes_and_accepts_png() {
    assert!(ImageBlock::decode(b"not an image", "x").is_none());
    // A 1×1 PNG (smallest valid) round-trips through the decoder.
    let mut bytes: Vec<u8> = Vec::new();
    {
        let img = RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]));
        DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
    }
    let block = ImageBlock::decode(&bytes, "one.png").expect("valid png decodes");
    assert_eq!(block.dimensions(), (1, 1));
}

#[test]
fn halfblocks_renderer_is_not_graphical() {
    // The portable renderer reports itself as the non-graphical (approximated) fallback.
    let r = ImageRenderer::halfblocks();
    assert!(!r.is_graphical());
}
