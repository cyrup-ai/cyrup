//! `TUI-142` — the cell box an inline image is given, `ImageRenderer::cell_size`, ported from pi's
//! `calculateImageCellSize` (`packages/tui/src/terminal-image.ts:481-519` @f1b2e77f5). A
//! width-clamped image's rows come from one fractional scale (`:493-501`), not from rescaling
//! ceilinged counts. Pi's Kitty-only `chooseLessDistortedCellCount` step (`:472-479`) is not ported:
//! cyrup's renderer letterboxes rather than stretches, so every protocol keeps the ceiling.
//!
//! Every renderer here is built on pi's default `9x18` cell, so the numbers read directly.

use crate::{ImageBlock, ImageProtocol, ImageRenderer, TerminalCapabilities};
use image::{DynamicImage, RgbaImage};

fn block(w: u32, h: u32) -> ImageBlock {
    ImageBlock::new(DynamicImage::ImageRgba8(RgbaImage::new(w, h)), "t.png")
}

fn renderer(images: Option<ImageProtocol>) -> ImageRenderer {
    let caps = TerminalCapabilities {
        images,
        true_color: true,
        hyperlinks: true,
    };
    ImageRenderer::from_capabilities_with_cell_size(caps, Some((9, 18)))
}

/// Kitty keeps the ceiling, as iTerm2 and half-block do. Pi reserves `ceil - 1` rows under Kitty
/// when that is closer to the image's aspect (`chooseLessDistortedCellCount`), because pi's Kitty
/// placement stretches the image into the cell box. cyrup draws through `ratatui-image`
/// `Resize::Fit`, which scales proportionally and pads, and its Kitty transmit names no `c`/`r` box,
/// so a smaller box would only shrink the image (90x20 px into 10x1 cells is drawn 81x18 px, nine
/// columns), never undistort it. 90x20 px is 1.11 rows, where pi's Kitty would take one.
#[test]
fn every_protocol_keeps_the_ceiling_row_count() {
    for images in [
        Some(ImageProtocol::Kitty),
        Some(ImageProtocol::Iterm2),
        None,
    ] {
        let r = renderer(images);
        assert_eq!(r.cell_size(&block(90, 20), 80), (10, 2), "{images:?}");
        assert_eq!(r.cell_size(&block(90, 34), 80), (10, 2), "{images:?}");
        assert_eq!(r.cell_size(&block(90, 2), 80), (10, 1), "{images:?}");
    }
}

/// A width-clamped image fits the given width, and its rows come from the fractional scale:
/// 901x900 px into 10 columns scales by 90/901, so it is 89.9 px (4.99 rows) tall and needs 5 rows.
/// The old integer rescale (`50 * 10 / 101`) truncated to 4 and under-reserved a row.
#[test]
fn a_width_clamped_image_fits_the_width_and_takes_its_rows_from_the_scale() {
    for images in [
        Some(ImageProtocol::Kitty),
        Some(ImageProtocol::Iterm2),
        None,
    ] {
        let r = renderer(images);
        assert_eq!(r.cell_size(&block(901, 900), 10), (10, 5), "{images:?}");
        assert_eq!(r.cell_size(&block(900, 900), 10), (10, 5), "{images:?}");
        assert_eq!(r.cell_size(&block(5000, 10), 7).0, 7, "{images:?}");
    }
}

/// An image narrower than the width keeps its natural size: cyrup draws with `Resize::Fit`, which
/// never enlarges, so it does not take pi's scale-up to `maxWidthCells` (`:493-497`).
#[test]
fn a_narrow_image_is_not_enlarged() {
    let r = renderer(Some(ImageProtocol::Iterm2));
    assert_eq!(r.cell_size(&block(90, 180), 200), (10, 10));
}
