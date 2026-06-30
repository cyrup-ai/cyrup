//! Inline terminal images (spec/tui/06 §6; `pi-tui/src/terminal-image.ts` +
//! `components/image.ts`).
//!
//! Pi renders images **inline** in the message stream using the terminal's native graphics protocol
//! (Kitty, iTerm2, or sixel), and falls back to a Unicode half-block raster on terminals without one
//! (`terminal-image.ts` capability probe). A `showImages` toggle (`show-images-selector.ts`) swaps the
//! whole pipeline for a one-line **text placeholder** so image-heavy sessions stay legible on slow or
//! headless terminals.
//!
//! This module realizes that with [`ratatui-image`](ratatui_image): an [`ImageRenderer`] owns a
//! `Picker` (the protocol + font-cell probe), and an [`ImageBlock`] holds one decoded
//! [`image::DynamicImage`] plus a human label. [`ImageRenderer::render`] honors the `show_images`
//! toggle — drawing the real protocol when on, the placeholder line when off (or when encoding fails).
//!
//! The half-block protocol ([`ratatui_image::picker::ProtocolType::Halfblocks`]) writes ordinary
//! `▀` cells with fg/bg colors into the frame buffer, so it renders to **any** backend — including
//! `ratatui::backend::TestBackend` — which is what makes the inline-image path snapshot-testable.

use image::DynamicImage;
use ratatui::layout::{Rect, Size};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::{Image, Resize};

use crate::theme::UiTheme;

/// The terminal's image-protocol capability + font-cell geometry (`terminal-image.ts` probe).
///
/// Built once at startup. In the production binary, [`ImageRenderer::detect`] queries the real TTY
/// (Kitty/iTerm2/sixel where present); everywhere else — tests, pipes, headless — it degrades to the
/// always-available [`ProtocolType::Halfblocks`] raster ([`ImageRenderer::halfblocks`]).
pub struct ImageRenderer {
    picker: Picker,
}

impl Default for ImageRenderer {
    /// The portable default: a half-block picker that needs no TTY query (so `AppState::default`
    /// constructs in tests and headless contexts without touching the terminal).
    fn default() -> Self {
        ImageRenderer::halfblocks()
    }
}

impl ImageRenderer {
    /// A renderer that always uses the Unicode half-block raster (`terminal-image.ts` fallback). Needs
    /// no terminal query — used by tests and as the safe default.
    pub fn halfblocks() -> Self {
        ImageRenderer { picker: Picker::halfblocks() }
    }

    /// Probe the controlling TTY for its image protocol + font-cell size (`Picker::from_query_stdio`,
    /// the `terminal-image.ts` capability handshake). Falls back to [`Self::halfblocks`] when the
    /// query fails (no TTY, unsupported terminal, pipe) so the front-end always has a working renderer.
    pub fn detect() -> Self {
        match Picker::from_query_stdio() {
            Ok(picker) => ImageRenderer { picker },
            Err(_) => ImageRenderer::halfblocks(),
        }
    }

    /// The negotiated protocol (`Kitty`/`Iterm2`/`Sixel`/`Halfblocks`) — drives the `/debug` report and
    /// lets the chrome decide whether images are "real" inline graphics or a half-block approximation.
    pub fn protocol(&self) -> ProtocolType {
        self.picker.protocol_type()
    }

    /// `true` when the negotiated protocol is a real terminal graphics protocol (not the half-block
    /// fallback) — i.e. images render as actual pixels, not approximated cells.
    pub fn is_graphical(&self) -> bool {
        !matches!(self.picker.protocol_type(), ProtocolType::Halfblocks)
    }

    /// One image's natural footprint in terminal **cells** at this picker's font size, clamped to the
    /// available `width` (so the live-region layout can reserve the right number of rows before draw).
    pub fn cell_size(&self, block: &ImageBlock, width: u16) -> (u16, u16) {
        let font = self.picker.font_size();
        let fw = u32::from(font.width.max(1));
        let fh = u32::from(font.height.max(1));
        let (iw, ih) = block.dimensions();
        let cols = iw.div_ceil(fw).max(1);
        let rows = ih.div_ceil(fh).max(1);
        let cols = cols.min(u32::from(width.max(1)));
        // Preserve aspect when width-clamped.
        let rows = if cols < iw.div_ceil(fw).max(1) {
            (rows * cols / iw.div_ceil(fw).max(1)).max(1)
        } else {
            rows
        };
        (cols.min(u32::from(u16::MAX)) as u16, rows.min(u32::from(u16::MAX)) as u16)
    }

    /// Render `block` into `area` (spec/tui/06 §6). When `show_images` is on the real protocol draws
    /// (Kitty/iTerm2/sixel pixels, or the half-block raster); when off — or when encoding fails, or the
    /// area is empty — the one-line text placeholder draws instead (`show-images-selector.ts` "No").
    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        block: &ImageBlock,
        theme: &UiTheme,
        show_images: bool,
    ) {
        if !show_images || area.width == 0 || area.height == 0 {
            frame.render_widget(Paragraph::new(block.placeholder_line(theme)), area);
            return;
        }
        let size = Size::new(area.width, area.height);
        match self.picker.new_protocol(block.image.clone(), size, Resize::Fit(None)) {
            Ok(protocol) => frame.render_widget(Image::new(&protocol).allow_clipping(true), area),
            // Encoding can fail on a degenerate area / unsupported pixel format — never panic, fall
            // back to the placeholder so the message still renders.
            Err(_) => frame.render_widget(Paragraph::new(block.placeholder_line(theme)), area),
        }
    }
}

/// One decoded image plus a human label (source path, `pasted image`, …), the unit the renderer draws.
#[derive(Clone)]
pub struct ImageBlock {
    image: DynamicImage,
    label: String,
}

impl ImageBlock {
    /// Wrap an already-decoded [`DynamicImage`] with a display `label`.
    pub fn new(image: DynamicImage, label: impl Into<String>) -> Self {
        ImageBlock { image, label: label.into() }
    }

    /// Decode raw image `bytes` (PNG/JPEG/GIF/WebP/BMP — the workspace `image` feature set), labelled
    /// `label`. `None` when the bytes are not a recognized image (`terminal-image.ts` guards the same).
    pub fn decode(bytes: &[u8], label: impl Into<String>) -> Option<Self> {
        let image = image::load_from_memory(bytes).ok()?;
        Some(ImageBlock { image, label: label.into() })
    }

    /// Read + decode an image file, labelling it with the path (the `@`-mention / attachment source).
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        Self::decode(&bytes, path.display().to_string())
    }

    /// The image's pixel dimensions `(width, height)`.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.image.width(), self.image.height())
    }

    /// The display label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The text-placeholder line shown when `show_images` is off (or on the scrollback/no-protocol
    /// path): `🖼 {label} ({w}×{h})` (`components/image.ts` placeholder, spec/tui/06 §6).
    pub fn placeholder_line(&self, theme: &UiTheme) -> Line<'static> {
        let (w, h) = self.dimensions();
        Line::from(vec![
            Span::styled("🖼 ", theme.accent_style()),
            Span::styled(self.label.clone(), theme.base_style()),
            Span::styled(format!(" ({w}×{h})"), theme.dim_style()),
        ])
    }
}
