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

#[test]
fn attached_image_renders_inline_halfblocks_when_show_images_on() {
    let mut app = App::new(TestBackend::new(40, 20), UiTheme::dark()).unwrap();
    // The default renderer is half-blocks (portable; renders to TestBackend).
    assert!(!app.state().image_renderer.is_graphical(), "test default must be the half-block raster");
    app.attach_image(red_image(16, 16));
    assert_eq!(app.pending_images().len(), 1);
    app.draw().unwrap();

    // The half-block raster paints the image's red into the buffer as fg/bg color on `▀` cells.
    let buf = app.terminal().backend().buffer();
    let painted = buf.content().iter().any(|c| {
        let red_fg = c.fg == ratatui::style::Color::Rgb(220, 30, 30);
        let red_bg = c.bg == ratatui::style::Color::Rgb(220, 30, 30);
        red_fg || red_bg
    });
    assert!(painted, "inline half-block image did not paint the source color into any cell");
    // No text placeholder when images render inline.
    assert!(!buf_text(&app).contains("🖼"), "placeholder must not show when images render inline");
}

#[test]
fn show_images_off_renders_text_placeholder() {
    let mut app = App::new(TestBackend::new(40, 20), UiTheme::dark()).unwrap();
    app.state_mut().show_images = false;
    app.attach_image(red_image(64, 48));
    app.draw().unwrap();

    let text = buf_text(&app);
    assert!(text.contains("🖼"), "placeholder glyph missing:\n{text}");
    assert!(text.contains("red.png"), "placeholder label missing:\n{text}");
    assert!(text.contains("64×48"), "placeholder dimensions missing:\n{text}");
    // No raster color painted when the toggle is off.
    let buf = app.terminal().backend().buffer();
    let painted = buf.content().iter().any(|c| c.bg == ratatui::style::Color::Rgb(220, 30, 30));
    assert!(!painted, "no raster should paint when show_images is off");
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
