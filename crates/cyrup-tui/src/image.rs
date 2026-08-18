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

use std::collections::HashMap;
use std::sync::Mutex;

use image::DynamicImage;
use ratatui::layout::{Rect, Size};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{FontSize, Image, Resize};

use crate::theme::UiTheme;

/// Identifies one attachment-strip image for protocol-cache purposes: its display label plus
/// source pixel dimensions (the same identity [`ImageBlock`]'s `PartialEq` already uses) and the
/// exact terminal-cell size it was last encoded at. Two blocks with the same label+dimensions
/// but different target sizes (e.g. after a terminal resize) get distinct entries — resize changes
/// what `new_protocol` must produce.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ImageCacheKey {
    label: String,
    dimensions: (u32, u32),
    size: (u16, u16),
}

/// The terminal's image-protocol capability + font-cell geometry (`terminal-image.ts` probe).
///
/// Built once at startup. In the production binary, [`ImageRenderer::detect`] queries the real TTY
/// (Kitty/iTerm2/sixel where present); everywhere else — tests, pipes, headless — it degrades to the
/// always-available [`ProtocolType::Halfblocks`] raster ([`ImageRenderer::halfblocks`]).
pub struct ImageRenderer {
    picker: Picker,
    /// Built protocols keyed by (image identity, target size) — building one is a raster clone +
    /// resize + encode, so it happens on CHANGE, not per frame (TUI-092 F7). One entry per
    /// currently-pending attachment; `render_images` calls `render` through `&AppState`, so this
    /// needs interior mutability. Entries for images no longer in `pending_images` simply stop
    /// being read once `render` is no longer called with their key (`clear_images`,
    /// `app/shell.rs`), so there is no separate invalidation hook to keep in sync.
    protocol_cache: Mutex<HashMap<ImageCacheKey, Protocol>>,
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
        ImageRenderer { picker: Picker::halfblocks(), protocol_cache: Mutex::new(HashMap::new()) }
    }

    /// Choose the image protocol from the **environment**, not an APC round-trip (feature #7; Pi
    /// `detectCapabilities`, terminal-image.ts:65-125). The old `Picker::from_query_stdio` sent an
    /// escape query to the TTY and blocked reading its reply — fragile under multiplexers and on
    /// terminals that never answer. This env-sniff (`TERM`/`TERM_PROGRAM`/`KITTY_WINDOW_ID`/… + the
    /// tmux/screen suppression) matches Pi's probe and never touches stdin. The negotiated protocol is
    /// forced onto a half-block picker (whose portable raster is the correct Pi fallback for a terminal
    /// with no native graphics).
    pub fn detect() -> Self {
        Self::from_capabilities(detect_capabilities())
    }

    /// Build a renderer for a resolved [`TerminalCapabilities`] set (the env-sniff result). `Kitty`/
    /// `Iterm2` force the matching `ratatui-image` protocol; `None` keeps the half-block raster. The
    /// font cell stays at the library default — see
    /// [`from_capabilities_with_cell_size`](Self::from_capabilities_with_cell_size) for the measured
    /// one.
    pub fn from_capabilities(caps: TerminalCapabilities) -> Self {
        Self::from_capabilities_with_cell_size(caps, None)
    }

    /// The same, with the terminal's **measured** cell size in pixels (`(width, height)`) — Pi's
    /// `queryCellSize` → `setCellDimensions` (`tui.ts:679-686`, `:877-890`), whose whole purpose is
    /// to replace the guessed `{widthPx: 9, heightPx: 18}` of `terminal-image.ts:37`.
    ///
    /// This is what makes an image occupy the right number of cells: [`Self::cell_size`] divides the
    /// image's pixel dimensions by the font cell, and the protocol encoders multiply back by it, so a
    /// guessed cell mis-sizes every image that is not width-clamped. cyrup's guess came from
    /// `Picker::halfblocks()` (`10x20`), which is neither Pi's default nor any real terminal's.
    ///
    /// `None` (no answer, or a terminal with no image protocol, which Pi never asks — `tui.ts:681`)
    /// keeps that default, exactly as Pi keeps its own.
    pub fn from_capabilities_with_cell_size(
        caps: TerminalCapabilities,
        cell_size: Option<(u16, u16)>,
    ) -> Self {
        let mut picker = match cell_size {
            // `Picker` exposes no `set_font_size`, and its fields are private: `from_fontsize` is the
            // only constructor that takes one. It is deprecated in favour of `from_query_stdio`,
            // which is precisely the blocking APC round-trip cyrup replaced with the env-sniff above
            // (feature #7) — so the deprecation's suggested replacement is the thing this crate
            // deliberately does not do, and the attribute is scoped to this one call.
            #[allow(deprecated)]
            Some((width, height)) if width > 0 && height > 0 => {
                Picker::from_fontsize(FontSize::new(width, height))
            }
            _ => Picker::halfblocks(),
        };
        // Set the protocol on EVERY path, not just the two `Some` arms: `from_fontsize` guesses one
        // from the environment (tmux/iTerm2), and the negotiated capability is authoritative.
        picker.set_protocol_type(match caps.images {
            Some(ImageProtocol::Kitty) => ProtocolType::Kitty,
            Some(ImageProtocol::Iterm2) => ProtocolType::Iterm2,
            None => ProtocolType::Halfblocks,
        });
        ImageRenderer { picker, protocol_cache: Mutex::new(HashMap::new()) }
    }

    /// The font cell the geometry is computed against, in pixels (`(width, height)`) — the measured
    /// value when the terminal answered `CSI 16 t`, the library default otherwise. Test/`/debug`
    /// visibility for what is otherwise an invisible input to every image's size.
    pub fn cell_pixels(&self) -> (u16, u16) {
        let font = self.picker.font_size();
        (font.width, font.height)
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
        // TUI-017 — the no-protocol case takes the SAME branch as `showImages: false`. Upstream's
        // `Image.render` is `if (caps.images) { …draw… } else { …one imageFallback line… }`
        // (`packages/tui/src/components/image.ts:70-118` @v0.83.0); there is no half-block
        // rasterizer anywhere in pi. cyrup's `from_capabilities` installs `Halfblocks` when
        // `caps.images == None` (`:102-106`), so on a plain xterm, the Linux console, CI or a pipe
        // an attachment used to dump ~20-30 rows of coloured `▀` into scrollback where pi prints
        // one `[Image: …]` line. `is_graphical()` is exactly `caps.images.is_some()` for the three
        // protocols `from_capabilities` maps.
        if !show_images || !self.is_graphical() || area.width == 0 || area.height == 0 {
            frame.render_widget(Paragraph::new(block.placeholder_line(theme)), area);
            return;
        }
        let size = Size::new(area.width, area.height);
        let key = ImageCacheKey {
            label: block.label().to_string(),
            dimensions: block.dimensions(),
            size: (area.width, area.height),
        };
        // TUI-092 F7 — memoise the built protocol keyed on (image identity, target size), so an
        // attachment redrawn at an unchanged key across consecutive frames costs zero further
        // raster clones/resizes/encodes. A poisoned lock (a prior panic mid-render) degrades to an
        // empty-cache read rather than propagating: a stale/missing entry only costs one rebuild,
        // never correctness (no-panic policy — this module must not `.unwrap()` a lock).
        let mut cache = match self.protocol_cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !cache.contains_key(&key) {
            match self.picker.new_protocol(block.image.clone(), size, Resize::Fit(None)) {
                Ok(protocol) => {
                    cache.insert(key.clone(), protocol);
                }
                // Encoding can fail on a degenerate area / unsupported pixel format — never panic,
                // fall back to the placeholder so the message still renders.
                Err(_) => {
                    frame.render_widget(Paragraph::new(block.placeholder_line(theme)), area);
                    return;
                }
            }
        }
        if let Some(protocol) = cache.get(&key) {
            frame.render_widget(Image::new(protocol).allow_clipping(true), area);
        }
    }
}

/// One decoded image plus a human label (source path, `pasted image`, …), the unit the renderer draws.
#[derive(Clone)]
pub struct ImageBlock {
    image: DynamicImage,
    label: String,
    /// The MIME type Pi's fallback line prints (`[Image: {name} [{mime}] {w}x{h}]`,
    /// `terminal-image.ts:546-558`). Sniffed from the encoded bytes in [`ImageBlock::decode`].
    mime: String,
}

impl std::fmt::Debug for ImageBlock {
    /// Identity, not pixels: a `DynamicImage`'s derived `Debug` would dump the whole raster into any
    /// `{:?}` of a transcript entry.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.dimensions();
        f.debug_struct("ImageBlock").field("label", &self.label).field("size", &(w, h)).finish()
    }
}

impl PartialEq for ImageBlock {
    /// Compares the label + pixel dimensions, NOT the raster — enough to tell two transcript entries
    /// apart (which is all `Entry: PartialEq` is used for) without a full per-pixel scan.
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label && self.dimensions() == other.dimensions()
    }
}

impl ImageBlock {
    /// Wrap an already-decoded [`DynamicImage`] with a display `label`.
    ///
    /// The MIME type defaults to `image/png` because there are no bytes left to sniff — use
    /// [`ImageBlock::decode`] or [`ImageBlock::from_path`] whenever the encoded bytes are in hand,
    /// since the MIME reaches the user through Pi's `[Image: … [{mimeType}] …]` fallback line
    /// ([`image_fallback_text`], `terminal-image.ts:546-558`).
    pub fn new(image: DynamicImage, label: impl Into<String>) -> Self {
        ImageBlock { image, label: label.into(), mime: "image/png".to_string() }
    }

    /// Decode raw image `bytes` (PNG/JPEG/GIF/WebP/BMP — the workspace `image` feature set), labelled
    /// `label`. `None` when the bytes are not a recognized image (`terminal-image.ts` guards the same).
    pub fn decode(bytes: &[u8], label: impl Into<String>) -> Option<Self> {
        let image = image::load_from_memory(bytes).ok()?;
        // Sniffed from the same bytes rather than guessed from the label's extension, matching Pi's
        // `getImageDimensions(base64Data, mimeType)` pairing of the decoded payload with its type.
        let mime = image::guess_format(bytes)
            .map(|f| f.to_mime_type().to_string())
            .unwrap_or_else(|_| "image/png".to_string());
        Some(ImageBlock { image, label: label.into(), mime })
    }

    /// The image's MIME type, as it appears in Pi's `[Image: {name} [{mime}] {w}x{h}]` fallback.
    pub fn mime_type(&self) -> &str {
        &self.mime
    }

    /// Downscale the raster (aspect-preserved) so neither side exceeds `max_px`; a no-op when it
    /// already fits. Used for images that will only ever be shown as a half-block raster a few dozen
    /// cells wide: it bounds the per-render clone + resize cost of a screenshot-sized payload, which
    /// would otherwise be paid on every frame the picture is on screen. The label is kept, so
    /// callers wanting the SOURCE dimensions must read them before downscaling.
    #[must_use]
    pub fn downscaled(self, max_px: u32) -> Self {
        let (w, h) = self.dimensions();
        if w <= max_px && h <= max_px {
            return self;
        }
        ImageBlock {
            image: self.image.thumbnail(max_px, max_px),
            label: self.label,
            mime: self.mime,
        }
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

    /// The text stand-in shown when `showImages` is off or the terminal has no image protocol.
    ///
    /// **TUI-017.** This used to emit the cyrup-invented `🖼 {label} ({w}×{h})`. Upstream emits
    /// exactly one line, `truncateToWidth(theme.fallbackColor(imageFallback(mimeType, dimensions,
    /// filename)), width)` (`packages/tui/src/components/image.ts:114-118` @v0.83.0), i.e.
    /// `[Image: {name} [{mime}] {w}x{h}]` — the string [`image_fallback_text`] has produced in this
    /// same file all along, used only by the tool-result path. Pi has no emoji placeholder anywhere.
    /// Styled `fallbackColor`, which is the dim/muted role.
    pub fn placeholder_line(&self, theme: &UiTheme) -> Line<'static> {
        let (w, h) = self.dimensions();
        Line::from(Span::styled(
            image_fallback_text(&self.mime, Some((w, h)), Some(&self.label)),
            theme.dim_style(),
        ))
    }

    /// Rasterize this image into styled [`Line`]s using the portable Unicode **half-block** protocol
    /// — the form an inline image can take when it has to survive as ordinary terminal cells.
    ///
    /// `cols` bounds the width in cells (Pi's `maxWidthCells` / `terminal.imageWidthCells`,
    /// tool-execution.ts:348); the height follows from the source aspect ratio. Returns an empty
    /// vector when the image cannot be encoded at that size — callers then fall back to
    /// [`image_fallback_text`].
    ///
    /// **Why half-blocks and not the negotiated Kitty/iTerm2 protocol**: those protocols work by
    /// planting an escape sequence inside a terminal cell. cyrup's transcript hands its rendered
    /// `Line`s to `Paragraph … .wrap()` — both in the live viewport and, through
    /// `Terminal::insert_before`, into native scrollback — and a re-wrapped escape sequence is
    /// corrupt output, not an image. Half-blocks are ordinary `▀` cells with fg/bg colours, so they
    /// wrap, scroll and snapshot correctly. This is a deliberate downgrade from Pi's per-terminal
    /// protocol selection for TOOL-RESULT images, and is the reason
    /// [`ImageRenderer::render`] (the attachment strip, which draws a real widget into a frame) still
    /// uses the negotiated protocol.
    pub fn halfblock_lines(&self, cols: u16) -> Vec<Line<'static>> {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::Widget as _;

        let picker = Picker::halfblocks();
        let font = picker.font_size();
        let (iw, ih) = self.dimensions();
        let fw = u32::from(font.width.max(1));
        let fh = u32::from(font.height.max(1));
        // The image's NATURAL footprint in cells; `cols` only ever clamps it DOWN (Pi's
        // `maxWidthCells` is an upper bound, never an upscale).
        let natural_cols = iw.div_ceil(fw).max(1);
        let natural_rows = ih.div_ceil(fh).max(1);
        let cols = u32::from(cols.max(1)).min(natural_cols);
        // Give `Resize::Fit` the full natural height as headroom and let it pick the
        // aspect-preserving size; the unused rows come back blank and are trimmed below.
        let (cols, rows) = (
            cols.min(u32::from(u16::MAX)) as u16,
            natural_rows.min(u32::from(u16::MAX)) as u16,
        );

        let area = Rect { x: 0, y: 0, width: cols, height: rows };
        let Ok(protocol) =
            picker.new_protocol(self.image.clone(), Size::new(cols, rows), Resize::Fit(None))
        else {
            return Vec::new();
        };
        let mut buf = Buffer::empty(area);
        Image::new(&protocol).allow_clipping(true).render(area, &mut buf);
        buffer_to_lines(&buf)
    }
}

/// Convert a rendered off-screen [`ratatui::buffer::Buffer`] into [`Line`]s, coalescing runs of
/// same-styled cells into one [`Span`]. Trailing **untouched** rows (blank text AND default style)
/// are dropped so the aspect-preserving fit does not reserve empty scrollback below the raster. A
/// blank row that carries a background colour is part of the picture and is kept.
fn buffer_to_lines(buf: &ratatui::buffer::Buffer) -> Vec<Line<'static>> {
    let area = buf.area;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(area.height as usize);
    for y in area.y..area.y.saturating_add(area.height) {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run = String::new();
        let mut run_style: Option<ratatui::style::Style> = None;
        for x in area.x..area.x.saturating_add(area.width) {
            let Some(cell) = buf.cell((x, y)) else { continue };
            let style = cell.style();
            if run_style != Some(style) {
                if let Some(prev) = run_style.take()
                    && !run.is_empty()
                {
                    spans.push(Span::styled(std::mem::take(&mut run), prev));
                }
                run_style = Some(style);
            }
            run.push_str(cell.symbol());
        }
        if let Some(prev) = run_style
            && !run.is_empty()
        {
            spans.push(Span::styled(run, prev));
        }
        lines.push(Line::from(spans));
    }
    // `Buffer::empty` leaves cells blank with `Color::Reset` fg/bg, so "untouched" means blank text
    // AND no painted background — never merely blank, since a solid-colour image row IS spaces.
    let untouched = |l: &Line<'static>| {
        l.spans.iter().all(|s| {
            s.content.trim().is_empty()
                && matches!(s.style.bg, None | Some(ratatui::style::Color::Reset))
        })
    };
    while lines.last().is_some_and(untouched) {
        lines.pop();
    }
    lines
}

/// Pi's `imageFallback` (`tui/src/terminal-image.ts:546-558`): the one-line text stand-in shown when
/// the terminal cannot render inline images (or `showImages` is off). Format is
/// `[Image: {filename} [{mimeType}] {w}x{h}]`, with `filename` and the dimensions omitted when
/// unknown. cyrup drops Pi's OSC-8 hyperlink wrapping of the filename (hyperlinks are out of scope
/// here) but keeps the `~`-shortening of a `$HOME`-relative path (`shortenImagePath`, `:533-539`).
pub fn image_fallback_text(
    mime_type: &str,
    dimensions: Option<(u32, u32)>,
    filename: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(name) = filename.filter(|n| !n.is_empty()) {
        parts.push(shorten_image_path(name));
    }
    parts.push(format!("[{mime_type}]"));
    if let Some((w, h)) = dimensions {
        parts.push(format!("{w}x{h}"));
    }
    format!("[Image: {}]", parts.join(" "))
}

/// `shortenImagePath` (terminal-image.ts:533-539): rewrite a `$HOME`-rooted absolute path to `~/…`.
fn shorten_image_path(filename: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else { return filename.to_string() };
    let home = home.to_string_lossy();
    if home.is_empty() {
        return filename.to_string();
    }
    if filename == home {
        return "~".to_string();
    }
    match filename.strip_prefix(home.as_ref()) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => filename.to_string(),
    }
}

/// A terminal's native inline-image protocol (Pi `ImageProtocol`, terminal-image.ts:3). `None` (in the
/// [`TerminalCapabilities`] wrapper) is Pi's "no native graphics" — cyrup then renders the half-block
/// raster instead of dropping the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageProtocol {
    /// The Kitty graphics protocol (kitty, ghostty, wezterm, warp).
    Kitty,
    /// The iTerm2 inline-image protocol (iTerm2).
    Iterm2,
}

/// The env-sniffed terminal capabilities (Pi `TerminalCapabilities`, terminal-image.ts:65-125): the
/// inline-image protocol, whether 24-bit color is advertised, and whether OSC-8 hyperlinks are
/// forwarded to the outer terminal. Drives [`ImageRenderer::from_capabilities`] (feature #7) and the
/// OSC-8 hyperlink gate in rendered output (feature #8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCapabilities {
    /// The negotiated inline-image protocol, or `None` for the half-block fallback.
    pub images: Option<ImageProtocol>,
    /// Whether the terminal advertises 24-bit color (`COLORTERM=truecolor`/`24bit`).
    pub true_color: bool,
    /// Whether OSC-8 hyperlinks reach the outer terminal (off under screen / unidentified terminals).
    pub hyperlinks: bool,
}

impl TerminalCapabilities {
    /// The conservative default a headless / unidentified terminal gets: no inline images, no
    /// hyperlinks, truecolor only if `COLORTERM` hinted it (Pi terminal-image.ts:124).
    fn conservative(true_color: bool) -> Self {
        TerminalCapabilities { images: None, true_color, hyperlinks: false }
    }
}

/// Env-sniff the terminal capabilities (feature #7; Pi `detectCapabilities`, terminal-image.ts:65).
/// Reads the real process environment and, when running under tmux, probes whether the outer terminal
/// forwards OSC-8 hyperlinks (`tmux display-message -p '#{client_termfeatures}'`, Pi
/// `probeTmuxHyperlinks`; any error ⇒ `false`).
pub fn detect_capabilities() -> TerminalCapabilities {
    detect_capabilities_from(|k| std::env::var(k).ok(), probe_tmux_hyperlinks())
}

/// The process-wide OSC-8 answer, Pi's module-level `cachedCapabilities` + `getCapabilities()`
/// (`tui/src/terminal-image.ts:33`, `:138-143`): detect once, then hand the same answer to every
/// later caller. Renderers consult it through [`hyperlinks_supported`]; the app seeds it from the
/// capabilities it already detected at startup via [`seed_hyperlink_support`].
///
/// **TUI-N12.** This was a write-once `OnceLock` carrying only `hyperlinks`, so there was no way to
/// pin the global for a test and no way to reset it — the hermeticity hole that produced TUI-N11 (a
/// markdown test asserting a property of the developer's `TERM_PROGRAM`, red on ghostty, kitty,
/// iTerm2, WezTerm, Warp, vscode, alacritty, Windows Terminal and forwarding tmux, green only on an
/// unidentified terminal). Pi exports two mutators alongside the getter:
/// `resetCapabilitiesCache()` (`packages/tui/src/terminal-image.ts:137-139`) and
/// `setCapabilities(caps)` (`:142-144`), the latter doc-commented "Override the cached
/// capabilities. Useful in tests to exercise both code paths". Both are pure state mutation and
/// draw nothing, so ADR-0001 rule 2 puts them in scope with no substrate defence.
///
/// It is now a resettable cache over the whole [`TerminalCapabilities`] record, not just the one
/// field, so `images` and `true_color` get the same seam.
static CAPABILITIES: std::sync::RwLock<Option<TerminalCapabilities>> =
    std::sync::RwLock::new(None);

/// Whether the controlling terminal forwards OSC-8 hyperlinks — Pi `getCapabilities().hyperlinks`,
/// the gate on `markdown.ts:692`. Detected once and cached for the life of the process, exactly as
/// upstream caches `detectCapabilities()`.
///
/// **This is a pure environment read, unconditionally — it never spawns a subprocess**, which is
/// what makes it safe to call from a render path. The lazy fallback therefore uses
/// [`detect_capabilities_from`] with `tmux_forwards_hyperlinks = false` rather than
/// [`detect_capabilities`], whose `probe_tmux_hyperlinks` shells out to
/// `tmux display-message -p '#{client_termfeatures}'`. Redrawing a transcript must not fork.
///
/// The probe's answer is not lost: [`App::detect_image_support`](crate::App::detect_image_support)
/// runs the full [`detect_capabilities`] once at startup and seeds this cache via
/// [`seed_hyperlink_support`], so a real tmux session with forwarding enabled is already recorded
/// before the first frame. The non-probing fallback only decides for embedders that render without
/// ever detecting, and for them `false` is upstream's own stated conservative default — "Default to
/// the legacy `text (url)` behavior unless we have positively identified a hyperlink-capable
/// terminal above" (`tui/src/terminal-image.ts:130-134`) — which prints the URL rather than risking
/// a terminal that swallows OSC-8 and shows nothing.
pub fn hyperlinks_supported() -> bool {
    cached_capabilities().hyperlinks
}

/// Pi's `getCapabilities()` (`terminal-image.ts:33`, `:138-143`): the cached record, detected on
/// first use and reused for the life of the process unless [`set_capabilities`] or
/// [`reset_capabilities_cache`] intervenes. TUI-N12.
pub fn cached_capabilities() -> TerminalCapabilities {
    if let Ok(guard) = CAPABILITIES.read()
        && let Some(caps) = *guard
    {
        return caps;
    }
    let detected = detect_capabilities_from(|k| std::env::var(k).ok(), false);
    if let Ok(mut guard) = CAPABILITIES.write() {
        // First writer wins, so a `set_capabilities` that raced this detection is not clobbered.
        if guard.is_none() {
            *guard = Some(detected);
        }
        if let Some(caps) = *guard {
            return caps;
        }
    }
    detected
}

/// Pi's `setCapabilities(caps)` (`terminal-image.ts:142-144`) — "Override the cached capabilities.
/// Useful in tests to exercise both code paths". Unlike the old first-writer-wins seed this
/// REPLACES, which is what makes both branches reachable from a test. TUI-N12.
pub fn set_capabilities(caps: TerminalCapabilities) {
    if let Ok(mut guard) = CAPABILITIES.write() {
        *guard = Some(caps);
    }
}

/// Pi's `resetCapabilitiesCache()` (`terminal-image.ts:137-139`): drop the cache so the next read
/// re-detects. TUI-N12.
pub fn reset_capabilities_cache() {
    if let Ok(mut guard) = CAPABILITIES.write() {
        *guard = None;
    }
}

/// Seed the capability cache from capabilities that have already been detected, so startup's single
/// `detectCapabilities()` serves the renderer too. First writer wins, matching upstream's
/// `cachedCapabilities ??= detectCapabilities()`.
///
/// Kept as a `hyperlinks`-shaped entry point because `App::detect_image_support` is its only caller
/// and it has the whole record already; see [`seed_capabilities`] for that form.
pub fn seed_hyperlink_support(supported: bool) {
    if let Ok(mut guard) = CAPABILITIES.write()
        && guard.is_none()
    {
        *guard = Some(TerminalCapabilities {
            hyperlinks: supported,
            ..TerminalCapabilities::conservative(false)
        });
    }
}

/// Seed the whole record (the form `App::detect_image_support` should use — it already holds every
/// field, where [`seed_hyperlink_support`] discards two of them). First writer wins. TUI-N12.
pub fn seed_capabilities(caps: TerminalCapabilities) {
    if let Ok(mut guard) = CAPABILITIES.write()
        && guard.is_none()
    {
        *guard = Some(caps);
    }
}

/// The pure core of [`detect_capabilities`] for the *host* platform (Pi `detectCapabilities`,
/// v0.84.1 `tui/src/terminal-image.ts:68-132`), parameterised over an environment lookup + the
/// tmux-hyperlink-forwarding flag so both branches are deterministically testable.
///
/// Pi reads `process.platform` inline; here the platform is `cfg!(windows)`, which is a
/// compile-time constant and therefore untestable on a non-Windows builder. Use
/// [`detect_capabilities_on_platform`] to exercise the Windows-console branch anywhere.
pub fn detect_capabilities_from(
    env: impl Fn(&str) -> Option<String>,
    tmux_forwards_hyperlinks: bool,
) -> TerminalCapabilities {
    // Pi `const isWindowsConsole = process.platform === "win32"` (v0.84.1 terminal-image.ts:74).
    detect_capabilities_on_platform(env, tmux_forwards_hyperlinks, cfg!(windows))
}

/// [`detect_capabilities_from`] with Pi's `isWindowsConsole` (v0.84.1 `terminal-image.ts:74`) lifted
/// into a parameter. The ordered checks mirror Pi's exactly: multiplexer suppression first
/// (`:76-86`), then the positively-identified terminals (`:88-118`), then the bare Windows console
/// (`:124-129`), then the conservative default (`:131`).
pub fn detect_capabilities_on_platform(
    env: impl Fn(&str) -> Option<String>,
    tmux_forwards_hyperlinks: bool,
    is_windows_console: bool,
) -> TerminalCapabilities {
    let has = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    let lower = |k: &str| env(k).unwrap_or_default().to_ascii_lowercase();
    let term_program = lower("TERM_PROGRAM");
    let terminal_emulator = lower("TERMINAL_EMULATOR");
    let term = lower("TERM");
    let color_term = lower("COLORTERM");
    // Pi `colorTerm === "truecolor" || colorTerm === "24bit"` (v0.84.1 terminal-image.ts:73) — an
    // EQUALITY, not a substring test. `contains` (what this was) also fired on values that merely
    // embed the word, e.g. `COLORTERM=not-truecolor`. Port bug, present at v0.83.0 too.
    let has_true_color = color_term == "truecolor" || color_term == "24bit";
    let identified = |images: Option<ImageProtocol>, hyperlinks: bool| TerminalCapabilities {
        images,
        true_color: true,
        hyperlinks,
    };

    // Multiplexers first: image protocols are unreliable under tmux, so leave `images: None`; OSC-8
    // is emitted only when tmux confirms it forwards, and screen never forwards (terminal-image.ts:74-81).
    if has("TMUX") || term.starts_with("tmux") {
        return TerminalCapabilities {
            images: None,
            true_color: has_true_color,
            hyperlinks: tmux_forwards_hyperlinks,
        };
    }
    if term.starts_with("screen") {
        return TerminalCapabilities { images: None, true_color: has_true_color, hyperlinks: false };
    }
    // Positively-identified terminals (terminal-image.ts:83-118).
    if has("KITTY_WINDOW_ID") || term_program == "kitty" {
        return identified(Some(ImageProtocol::Kitty), true);
    }
    if term_program == "ghostty" || term.contains("ghostty") || has("GHOSTTY_RESOURCES_DIR") {
        return identified(Some(ImageProtocol::Kitty), true);
    }
    if has("WEZTERM_PANE") || term_program == "wezterm" {
        return identified(Some(ImageProtocol::Kitty), true);
    }
    if term_program == "warpterminal"
        || has("WARP_SESSION_ID")
        || has("WARP_TERMINAL_SESSION_UUID")
    {
        return identified(Some(ImageProtocol::Kitty), true);
    }
    if has("ITERM_SESSION_ID") || term_program == "iterm.app" {
        return identified(Some(ImageProtocol::Iterm2), true);
    }
    if has("WT_SESSION") || term_program == "vscode" || term_program == "alacritty" {
        return identified(None, true);
    }
    if terminal_emulator == "jetbrains-jediterm" {
        return identified(None, false);
    }
    // Pi terminal-image.ts:124-129 — "Windows Terminal does not always set WT_SESSION, for example
    // when it hosts a cmd.exe launched directly from Win+R. Modern Windows consoles support
    // truecolor; keep hyperlinks off unless we positively detected support above." So a bare Windows
    // console gets truecolor WITHOUT a `COLORTERM` hint, while still falling short of OSC-8.
    //
    // Version lag, not a port bug: upstream added this in `fa07e7bd9` ("fix(tui): detect truecolor
    // for Windows consoles"), after the v0.83.0 baseline this crate was ported against.
    if is_windows_console {
        return TerminalCapabilities { images: None, true_color: true, hyperlinks: false };
    }
    // Unknown terminal: conservative — OSC-8 off (an unforwarded hyperlink would vanish from output).
    TerminalCapabilities::conservative(has_true_color)
}

/// Probe whether the attached tmux client forwards OSC-8 hyperlinks (Pi `probeTmuxHyperlinks`,
/// terminal-image.ts:45-63): tmux only re-emits them when `client_termfeatures` lists `hyperlinks`.
/// Any error (not in tmux, tmux absent) ⇒ `false`.
fn probe_tmux_hyperlinks() -> bool {
    if std::env::var_os("TMUX").is_none() {
        return false;
    }
    std::process::Command::new("tmux")
        .args(["display-message", "-p", "#{client_termfeatures}"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("hyperlinks"))
        .unwrap_or(false)
}
