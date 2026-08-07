//! The terminal cell-size query (`CSI 16 t`) — Pi `queryCellSize` / `consumeCellSizeResponse`
//! (`pi/packages/tui/src/tui.ts:647`, `:679-686`, `:877-890`) feeding `setCellDimensions`
//! (`terminal-image.ts`, whose un-measured default is `{widthPx: 9, heightPx: 18}` at `:37`).
//!
//! cyrup asked the terminal nothing and laid every inline image out against `ratatui-image`'s
//! `Picker::halfblocks()` placeholder cell (`10x20`) — which is neither Pi's default nor any real
//! terminal's. The damage lands on images small enough not to be width-clamped: their reserved row
//! count and their drawn scale both come from that cell.
//!
//! These tests drive the public seams the startup path uses: the reply parsers
//! ([`cyrup_tui::parse_cell_size_report`] / [`cyrup_tui::find_cell_size_report`]) and
//! [`cyrup_tui::ImageRenderer::from_capabilities_with_cell_size`], which is what
//! `App::detect_image_support` now builds the live renderer with.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_tui::{
    find_cell_size_report, parse_cell_size_report, ImageBlock, ImageProtocol, ImageRenderer,
    TerminalCapabilities, CELL_SIZE_QUERY,
};
use image::{DynamicImage, Rgba, RgbaImage};

/// A solid-red test image of `w×h` pixels (no file IO / decode needed).
fn red_image(w: u32, h: u32) -> ImageBlock {
    let mut img = RgbaImage::new(w, h);
    for px in img.pixels_mut() {
        *px = Rgba([220, 30, 30, 255]);
    }
    ImageBlock::new(DynamicImage::ImageRgba8(img), "red.png")
}

fn kitty_caps() -> TerminalCapabilities {
    TerminalCapabilities { images: Some(ImageProtocol::Kitty), true_color: true, hyperlinks: true }
}

/// The query Pi writes, and the reply shape it accepts (`tui.ts:686`, `:879`).
#[test]
fn the_query_and_its_reply_are_pis() {
    assert_eq!(CELL_SIZE_QUERY, "\x1b[16t");
    // `CSI 6 ; height ; width t` — the reply is height-first; the parsed tuple is (width, height).
    assert_eq!(parse_cell_size_report("\x1b[6;18;9t"), Some((9, 18)));
    // Found alongside the DA1 sentinel reply the probe appends, in one read.
    assert_eq!(find_cell_size_report("\x1b[6;36;15t\x1b[?62;1;2c"), Some((15, 36)));
    // A terminal that answers only DA1 leaves the cell unmeasured (Pi keeps its own default).
    assert_eq!(find_cell_size_report("\x1b[?62;1;2c"), None);
    // Pi's `heightPx <= 0 || widthPx <= 0` guard (`:885`) — a zero cell would divide geometry by 0.
    assert_eq!(parse_cell_size_report("\x1b[6;0;9t"), None);
}

/// The measured cell must actually reach the geometry — this is the whole point of the query.
///
/// A 90×180 px image is 9 cols × 9 rows on a real 10×20 cell and 10 cols × 10 rows on a 9×18 one.
/// `width` is large enough that no clamping is involved, so the numbers come from the cell alone.
#[test]
fn a_measured_cell_changes_the_image_geometry() {
    let block = red_image(90, 180);

    // Unmeasured: `ratatui-image`'s placeholder cell.
    let guessed = ImageRenderer::from_capabilities_with_cell_size(kitty_caps(), None);
    assert_eq!(guessed.cell_pixels(), (10, 20), "the un-measured default cyrup used to be stuck on");
    assert_eq!(guessed.cell_size(&block, 200), (9, 9));

    // Measured (`CSI 6 ; 18 ; 9 t` — Pi's own documented default cell).
    let measured = ImageRenderer::from_capabilities_with_cell_size(kitty_caps(), Some((9, 18)));
    assert_eq!(measured.cell_pixels(), (9, 18));
    assert_eq!(
        measured.cell_size(&block, 200),
        (10, 10),
        "the measured cell must drive the reserved cols/rows, not the library placeholder"
    );
}

/// A degenerate answer is ignored rather than propagated, and the negotiated protocol survives the
/// new constructor — `from_fontsize` guesses one from the environment, so it is overridden.
#[test]
fn a_degenerate_cell_is_ignored_and_the_protocol_is_preserved() {
    for bogus in [Some((0, 18)), Some((9, 0)), None] {
        let r = ImageRenderer::from_capabilities_with_cell_size(kitty_caps(), bogus);
        assert_eq!(r.cell_pixels(), (10, 20), "{bogus:?} must fall back to the default cell");
    }
    // A terminal with no image protocol keeps the half-block raster (and is never asked, upstream).
    let none_caps =
        TerminalCapabilities { images: None, true_color: true, hyperlinks: false };
    let plain = ImageRenderer::from_capabilities_with_cell_size(none_caps, Some((9, 18)));
    assert!(!plain.is_graphical(), "no image capability ⇒ half-blocks regardless of the cell size");
}
