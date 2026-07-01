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
    /// `Iterm2` force the matching `ratatui-image` protocol; `None` keeps the half-block raster.
    pub fn from_capabilities(caps: TerminalCapabilities) -> Self {
        let mut picker = Picker::halfblocks();
        match caps.images {
            Some(ImageProtocol::Kitty) => picker.set_protocol_type(ProtocolType::Kitty),
            Some(ImageProtocol::Iterm2) => picker.set_protocol_type(ProtocolType::Iterm2),
            None => {}
        }
        ImageRenderer { picker }
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

/// The pure core of [`detect_capabilities`] (Pi `detectCapabilities`, terminal-image.ts:65-125),
/// parameterised over an environment lookup + the tmux-hyperlink-forwarding flag so both branches are
/// deterministically testable. The ordered checks mirror Pi's exactly (multiplexer suppression first,
/// then the positively-identified terminals, then the conservative default).
pub fn detect_capabilities_from(
    env: impl Fn(&str) -> Option<String>,
    tmux_forwards_hyperlinks: bool,
) -> TerminalCapabilities {
    let has = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    let lower = |k: &str| env(k).unwrap_or_default().to_ascii_lowercase();
    let term_program = lower("TERM_PROGRAM");
    let terminal_emulator = lower("TERMINAL_EMULATOR");
    let term = lower("TERM");
    let color_term = lower("COLORTERM");
    let has_true_color = color_term.contains("truecolor") || color_term.contains("24bit");
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
