//! **The inline-image lifecycle while the alternate screen owns every cell** — capability
//! suppression on the way in, a placement registry under pi's three retention caps while the screen
//! is up, and the kitty teardown on the way out. cyrup's port of the image half of `TuiAltScreen`
//! (`packages/tui/src/tui-alt-screen.ts` @v0.84.3: the fields at `:180-182`, `beforeTerminalStart`'s
//! `:264-270`, `beforeTerminalStop`'s `:306-308`, `afterTerminalStop`'s `:330-333`,
//! `deleteKittyImages` `:336-338` and `prepareKittyScreen` `:340-387`, under the caps at `:65-67`).
//! ADR-0005 §Decision B-12, whose `:220-226` / `:285-350` / `:58-60` citations are the @v0.84.1 line
//! numbering for the same members.
//!
//! # What actually emits a graphics protocol in cyrup, and what does not
//! Upstream renders every image through the negotiated protocol, so *any* screen line can carry a
//! kitty transmission and `prepareKittyScreen` has to walk the whole rendered screen (`:341-359`).
//! cyrup does not: a tool-result image in the transcript is rasterized to ordinary half-block cells
//! (`crate::image::ImageBlock::halfblock_lines`, whose doc gives the reason — the transcript's
//! `Line`s are re-wrapped by `Paragraph … .wrap()`, and a re-wrapped escape sequence is corrupt
//! output rather than an image). Half-blocks upload nothing and leave nothing behind, in either
//! renderer.
//!
//! The **attachment strip** is therefore the only surface in this crate that emits a real
//! Kitty/iTerm2 sequence: `crate::app::render_images` draws each pending [`ImageBlock`] through
//! `ImageRenderer::render`, which plants the protocol's escape in the frame buffer. [`place`] is
//! that painter under alternate-screen ownership — the same geometry, plus the suppression gate and
//! the registry this unit owns — and the alternate-screen renderer must paint the strip through it
//! rather than through the inline `render_images`, because everything below is scoped to the one
//! surface that can leak.
//!
//! # Suppressing iterm2, and the three handles that need it
//! Upstream turns iTerm2 images OFF for the duration of the excursion and restores the saved
//! capability record on exit (`:267-269`, `:330-332`). cyrup honours that through three handles,
//! because it has no single `getCapabilities()` read on the render path:
//!
//! 1. The process-global capability record ([`crate::image::set_capabilities`],
//!    [`crate::image::cached_capabilities`]) — pi's `setCapabilities`/`getCapabilities`
//!    (`terminal-image.ts:193-195`, `:164-173`) exactly. Saved and restored byte-identically:
//!    [`ImageLifecycle::restore`] writes back the *same* [`TerminalCapabilities`] value that
//!    [`ImageLifecycle::enter`] read, never a rebuilt one.
//! 2. `TranscriptView::set_graphical_images` — the transcript's own gate on tool-result images
//!    (`ImageOpts::graphical`, pi's `getCapabilities().images` test at `tool-execution.ts:331`).
//!    Setting it is also what stands in for upstream's `this.invalidate()` at `:270`: the flag is
//!    part of the render-cache key (`transcript/cache.rs:143`), so changing it invalidates the
//!    cached document with no separate call.
//! 3. The `show_images` argument [`place`] hands `ImageRenderer::render` — the strip's own gate,
//!    which draws `ImageBlock::placeholder_line` (pi's `imageFallback`) instead of the protocol.
//!    This is the handle that does the real work, because `AppState::image_renderer` was built from
//!    the capabilities ONCE at startup (`app/shell.rs:401`) and does not re-read the global; it is
//!    not this unit's to rebuild, and [`ImageLifecycle::allows_graphics`] is the flag any other
//!    chrome painter should consult for the same reason.
//!
//! # The registry, and the one thing it cannot do
//! [`PlacementRegistry`] is pi's `uploadedKittyImages` map (`:182`): insertion-ordered, so
//! re-inserting a visible entry moves it to the back and the front is the least-recently-visible —
//! upstream's `delete` then `set` at `:353-354`, and the ordering its eviction walk depends on.
//! Each entry carries the transmitted and decoded byte estimates pi records as
//! `transmissionBytes` / `estimatedDecodedBytes` (`terminal-image.ts:418-419`), and eviction is
//! upstream's loop verbatim (`:361-386`): the caps bound only the OFF-screen residue, a visible
//! entry is never evicted, and the walk stops as soon as all three are satisfied.
//!
//! `[CYRUP-DELTA]` — **eviction cannot free the terminal-side upload here, and does not pretend
//! to.** Upstream owns its kitty ids, so it evicts with `deleteKittyImage(imageId)` (`:381`,
//! `terminal-image.ts:269-271`). cyrup transmits through `ratatui-image`, which allocates the id
//! itself with `rand::random()` (`picker.rs:248`) and keeps it in a private field
//! (`protocol/kitty.rs:25`); no accessor exposes it, so no per-image delete is addressable. What
//! the registry therefore bounds is cyrup's own bookkeeping, and what it records is which uploads
//! exist — the input a future per-image free would need. Freeing one upload additionally requires
//! dropping the matching `Protocol` from `ImageRenderer`'s cache (`image.rs:50-58`), since the
//! transmit sequence is emitted once per `Protocol` and never again (`protocol/kitty.rs:41-51`) —
//! delete without that and the image is gone for the rest of the session. Both halves live in
//! `image.rs`, which is outside this unit's write set; [`PlacementId`] is deliberately the same
//! (label, source dimensions, cell size) triple that keys that cache, so the two line up when the
//! hook lands.
//!
//! # Why the teardown delete is gated on `preserve_screen`
//! Upstream deletes every uploaded kitty image in `beforeTerminalStop` unconditionally (`:306`,
//! `:336-338`), and can, because its renderers do not share an upload cache: a mode switch builds
//! fresh components that re-transmit. cyrup's two renderers share ONE `AppState::image_renderer`
//! and therefore one `Protocol` cache, and a `Protocol` transmits exactly once. Emitting `d=A` on a
//! mode switch would free uploads the returning inline renderer still references through unicode
//! placeholders, and nothing would ever re-transmit them — the attachment strip would go blank for
//! the rest of the session. That is precisely the "do not change the inline renderer's behaviour"
//! rule (ADR-0005 §Decision B), so [`ImageLifecycle::delete_all`] takes the same `preserve_screen`
//! flag `terminal::TerminalSetup::leave` does and emits the escape only on a real stop.
//!
//! Nothing is leaked by the gated-off branch: a kitty *placement* on the alternate screen is made
//! by the cells that reference it, and those cells are discarded by the terminal when the alternate
//! screen is left (`terminal.rs`, `EXIT_ALT_SCREEN`). What survives a mode switch is the upload,
//! which is exactly what the returning inline renderer needs.
//!
//! # The inline renderer is untouched
//! Nothing here runs in regular mode. [`ImageLifecycle`] is constructed by the alternate-screen
//! renderer and dropped with it; until [`ImageLifecycle::enter`] runs, the capability global,
//! the transcript gate and the strip painter are all exactly what they were before ADR-0005.

use std::io::{self, Write};

use ratatui::crossterm::queue;
use ratatui::crossterm::style::Print;
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::image::{ImageBlock, ImageProtocol, ImageRenderer, TerminalCapabilities};
use crate::theme::UiTheme;
use crate::transcript::TranscriptView;

/// pi's `MAX_CACHED_OFFSCREEN_KITTY_IMAGES` (`tui-alt-screen.ts:65`) — how many uploads may be
/// retained for images that are NOT on screen.
const MAX_CACHED_OFFSCREEN_IMAGES: usize = 16;

/// pi's `MAX_CACHED_OFFSCREEN_KITTY_TRANSMISSION_BYTES` (`tui-alt-screen.ts:66`), 32 MiB.
const MAX_CACHED_OFFSCREEN_TRANSMISSION_BYTES: u64 = 32 * 1024 * 1024;

/// pi's `MAX_CACHED_OFFSCREEN_KITTY_DECODED_BYTES` (`tui-alt-screen.ts:67`), 64 MiB.
const MAX_CACHED_OFFSCREEN_DECODED_BYTES: u64 = 64 * 1024 * 1024;

/// pi's `deleteAllKittyImages()` (`terminal-image.ts:277-279`): `a=d,d=A` deletes every visible
/// placement AND frees the uploaded data, `q=2` suppresses the terminal's reply.
const DELETE_ALL_KITTY_IMAGES: &str = "\x1b_Ga=d,d=A,q=2\x1b\\";

/// [`DELETE_ALL_KITTY_IMAGES`] wrapped in tmux's passthrough (`\x1bPtmux;` … `\x1b\\`, with every
/// inner `ESC` doubled) — the same wrapping `ratatui-image` applies to the transmissions this
/// deletes (`picker/cap_parser.rs:81-86`). tmux consumes an unwrapped APC rather than forwarding
/// it, so without this the delete would never reach the outer terminal.
const DELETE_ALL_KITTY_IMAGES_TMUX: &str = "\x1bPtmux;\x1b\x1b_Ga=d,d=A,q=2\x1b\x1b\\\x1b\\";

/// Whether the graphics escapes this module writes must be tmux-wrapped.
///
/// `[CYRUP-DELTA]` — deliberately **not** [`crate::tmux::in_tmux`] and not
/// [`super::mouse::under_multiplexer`]. The only correct test here is the one `ratatui-image` used
/// when it wrote the transmissions being deleted (`picker.rs:296-300`: a `TERM` beginning `tmux`,
/// or `TERM_PROGRAM` exactly `tmux`), because a delete wrapped differently from its transmission
/// either never arrives or prints as garbage. `TMUX` alone is not that test: it is set inside a
/// tmux pane whatever `TERM` says, and `ratatui-image` would not have wrapped there.
fn wraps_for_tmux() -> bool {
    if std::env::var("TERM").is_ok_and(|term| term.starts_with("tmux")) {
        return true;
    }
    std::env::var("TERM_PROGRAM").is_ok_and(|program| program == "tmux")
}

/// One retained upload's identity — cyrup's stand-in for pi's kitty image id, which keys
/// `uploadedKittyImages` (`tui-alt-screen.ts:182`).
///
/// The id itself is unavailable (see the module doc's `[CYRUP-DELTA]`), so the identity used is the
/// one that actually distinguishes one upload from another in this crate: the (label, source
/// dimensions, target cell size) triple `ImageRenderer`'s protocol cache is keyed on
/// (`image.rs:39-43`). One cache entry is one `Protocol` is one transmission, so this is one
/// registry entry per upload — the same 1:1 pi gets from the id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlacementId {
    /// The block's display label — `ImageBlock::label`.
    label: String,
    /// The block's SOURCE pixel dimensions — `ImageBlock::dimensions`.
    dimensions: (u32, u32),
    /// The terminal-cell area it was last drawn at. A resize produces a different protocol, hence a
    /// different upload, hence a different entry.
    cells: (u16, u16),
}

/// One entry of [`PlacementRegistry`] — pi's `CachedKittyImage` (`tui-alt-screen.ts:74-78`).
struct Placement {
    /// Which upload this is.
    id: PlacementId,
    /// Bytes the transmission cost — pi's `transmissionBytes` (`terminal-image.ts:414`).
    transmission_bytes: u64,
    /// Bytes the terminal holds decoded — pi's `estimatedDecodedBytes` (`terminal-image.ts:419`,
    /// `widthPx * heightPx * 4`).
    decoded_bytes: u64,
    /// The [`PlacementRegistry::generation`] this entry was last drawn in. Equal to the current
    /// generation ⇒ on screen this frame, which is pi's `visibleImageIds` membership (`:341`,
    /// `:365`, `:379`).
    last_visible: u64,
}

/// The retained uploads, least-recently-visible first — pi's
/// `uploadedKittyImages: Map<number, CachedKittyImage>` (`tui-alt-screen.ts:182`).
///
/// A `Vec` rather than a `HashMap` because upstream depends on the **insertion order** a JavaScript
/// `Map` guarantees: `prepareKittyScreen` re-inserts every visible image (`:353-354`) so the
/// eviction walk at `:372` meets the least-recently-visible entries first. A `HashMap` has no such
/// order, and the counts involved are bounded by [`MAX_CACHED_OFFSCREEN_IMAGES`] plus what one
/// screen can show, so the linear scans are smaller than a hash would be.
#[derive(Default)]
pub(super) struct PlacementRegistry {
    /// Least-recently-visible first; [`Self::touch`] moves an entry to the back.
    entries: Vec<Placement>,
    /// Frame counter. [`Self::begin_frame`] bumps it, and an entry whose `last_visible` equals it
    /// is on screen this frame.
    generation: u64,
}

impl PlacementRegistry {
    /// Open a frame and return its generation — the marker [`Self::touch`] stamps and
    /// [`Self::evict`] tests, standing in for the `visibleImageIds` set upstream rebuilds per
    /// screen (`tui-alt-screen.ts:341`).
    fn begin_frame(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    /// Record that `id` is on screen this frame — pi's `delete` then `set` (`:353-354`), which both
    /// refreshes the entry and moves it to the back of the map's insertion order.
    ///
    /// The byte estimates are recomputed rather than kept from the first sighting, for the reason
    /// upstream overwrites its own (`:348-352`): the drawn cell area is part of the identity, so a
    /// re-drawn entry's estimate cannot silently describe a different size.
    fn touch(&mut self, id: PlacementId, cells: (u16, u16), cell_px: (u16, u16), generation: u64) {
        if let Some(index) = self.entries.iter().position(|entry| entry.id == id) {
            self.entries.remove(index);
        }
        let (transmission_bytes, decoded_bytes) = estimate_bytes(cells, cell_px);
        self.entries.push(Placement {
            id,
            transmission_bytes,
            decoded_bytes,
            last_visible: generation,
        });
    }

    /// Drop least-recently-visible off-screen entries until all three caps are satisfied — pi's
    /// eviction walk (`tui-alt-screen.ts:361-386`), including its three properties:
    ///
    /// 1. Only OFF-screen entries count toward the caps and only they are evicted (`:365`, `:379`);
    ///    an image the user is looking at is never dropped however large it is.
    /// 2. The walk stops the moment all three totals are back within bounds (`:373-378`) — this is
    ///    a bound on the residue, not a shrink-to-fit.
    /// 3. Order is least-recently-visible first, which is the map order [`Self::touch`] maintains.
    ///
    /// `Vec::retain` visits in order and is the `break` at `:377` expressed as "keep everything
    /// once the totals fit": the closure below stops evicting at exactly the entry upstream breaks
    /// on, and keeps the rest.
    fn evict(&mut self, generation: u64) {
        let mut offscreen_images: usize = 0;
        let mut offscreen_transmission: u64 = 0;
        let mut offscreen_decoded: u64 = 0;
        for entry in self.entries.iter().filter(|e| e.last_visible != generation) {
            offscreen_images = offscreen_images.saturating_add(1);
            offscreen_transmission =
                offscreen_transmission.saturating_add(entry.transmission_bytes);
            offscreen_decoded = offscreen_decoded.saturating_add(entry.decoded_bytes);
        }
        self.entries.retain(|entry| {
            let within_caps = offscreen_images <= MAX_CACHED_OFFSCREEN_IMAGES
                && offscreen_transmission <= MAX_CACHED_OFFSCREEN_TRANSMISSION_BYTES
                && offscreen_decoded <= MAX_CACHED_OFFSCREEN_DECODED_BYTES;
            if within_caps || entry.last_visible == generation {
                return true;
            }
            offscreen_images = offscreen_images.saturating_sub(1);
            offscreen_transmission =
                offscreen_transmission.saturating_sub(entry.transmission_bytes);
            offscreen_decoded = offscreen_decoded.saturating_sub(entry.decoded_bytes);
            false
        });
    }

    /// Forget every retained upload — pi's `this.uploadedKittyImages.clear()`, which it runs both
    /// on entry (`tui-alt-screen.ts:266`) and after the teardown delete (`:308`).
    fn clear(&mut self) {
        self.entries.clear();
    }

    /// How many uploads are currently retained. The renderer's window onto the registry; nothing
    /// outside this module may hold a [`Placement`], because the eviction order is the type's
    /// invariant and an external reordering would silently break it.
    pub(super) fn tracked(&self) -> usize {
        self.entries.len()
    }
}

/// Estimate what one drawn image costs the terminal, in transmitted and decoded bytes.
///
/// `ratatui-image` sends raw RGBA (`f=32,t=d` at `protocol/kitty.rs:254`) for an image already
/// resized to the drawn cell area, so the decoded size is that area in pixels times four —
/// upstream's own `widthPx * heightPx * 4` (`terminal-image.ts:419`) — and the transmission is that
/// payload base64-encoded, four output bytes per three input bytes (`:261`). The chunk framing
/// (~11 bytes per 4096-char chunk, `:240`) is under a thousandth of the payload and is left out,
/// exactly as upstream leaves out its own escape framing.
///
/// Every step saturates: these feed a comparison against a cap, so a pathological size must clamp
/// rather than overflow — an overflow would be a panic in a debug build, which the workspace's
/// `panic` lint exists to keep out of this crate.
fn estimate_bytes(cells: (u16, u16), cell_px: (u16, u16)) -> (u64, u64) {
    let width_px = u64::from(cells.0).saturating_mul(u64::from(cell_px.0));
    let height_px = u64::from(cells.1).saturating_mul(u64::from(cell_px.1));
    let decoded = width_px.saturating_mul(height_px).saturating_mul(4);
    let transmission = decoded.div_ceil(3).saturating_mul(4);
    (transmission, decoded)
}

/// The image half of one alternate-screen excursion — pi's `imageProtocol`, `savedCapabilities` and
/// `uploadedKittyImages` fields (`tui-alt-screen.ts:180-182`) with the three hooks that drive them.
///
/// Constructed empty by the renderer, told about the terminal by [`Self::enter`] immediately after
/// `terminal::AltTerminal::enter`, consulted by [`place`] on every frame, and unwound by
/// [`Self::delete_all`] (before the teardown write) and [`Self::restore`] (after it) — the same
/// before/after split `TuiBase.stop` forces on upstream (`tui.ts:752-762`).
#[derive(Default)]
pub(super) struct ImageLifecycle {
    /// pi's `imageProtocol` (`tui-alt-screen.ts:180`), latched at [`Self::enter`] from the
    /// capabilities as they were BEFORE any suppression — which is what makes the kitty tests below
    /// (and upstream's at `:337`) still fire on the way out.
    protocol: Option<ImageProtocol>,
    /// pi's `savedCapabilities` (`:181`): `Some` only while iterm2 is suppressed, and the exact
    /// record to write back. `Some`-ness is therefore also the "graphics are suppressed" flag —
    /// see [`Self::allows_graphics`].
    saved: Option<TerminalCapabilities>,
    /// pi's `uploadedKittyImages` (`:182`).
    placements: PlacementRegistry,
}

impl ImageLifecycle {
    /// Latch the terminal's protocol and suppress iterm2 images — pi's `beforeTerminalStart` image
    /// block (`tui-alt-screen.ts:264-270`), in upstream's order: read the capabilities, latch the
    /// protocol from them, clear the registry, and only then suppress.
    ///
    /// Call immediately after `terminal::AltTerminal::enter` and before the first frame. Idempotent
    /// in the sense that matters: a second call with the suppression already in force reads the
    /// suppressed record, so `images` is already `None` and nothing is saved over the original.
    ///
    /// `transcript` is the second of the three suppression handles (see the module doc): turning
    /// `graphical_images` off is what makes a tool-result image take its `[Image: …]` text branch
    /// while the alternate screen is up, and — because the flag is part of the render-cache key
    /// (`transcript/cache.rs:143`) — it is also this port's `this.invalidate()` (`:270`).
    pub(super) fn enter(&mut self, transcript: &mut TranscriptView) {
        let capabilities = crate::image::cached_capabilities();
        self.protocol = capabilities.images;
        self.placements.clear();
        if capabilities.images == Some(ImageProtocol::Iterm2) {
            self.saved = Some(capabilities);
            crate::image::set_capabilities(TerminalCapabilities {
                images: None,
                ..capabilities
            });
            transcript.set_graphical_images(false);
        }
    }

    /// Whether a graphics protocol may be emitted right now — `false` exactly while iterm2 is
    /// suppressed (pi's `capabilities.images === null` for the duration, `:269`).
    ///
    /// [`place`] consults this itself. It is `pub(super)` for the other direction: any chrome the
    /// renderer paints through a path that reaches `ImageRenderer::render` must pass
    /// `show_images && lifecycle.allows_graphics()`, because that renderer was built from the
    /// capabilities once at startup (`app/shell.rs:401`) and never re-reads the global this
    /// module writes.
    pub(super) fn allows_graphics(&self) -> bool {
        self.saved.is_none()
    }

    /// The protocol the terminal negotiated before suppression — pi's `imageProtocol` (`:180`).
    /// `None` on a terminal with no native graphics, where every image is already half-blocks.
    pub(super) fn protocol(&self) -> Option<ImageProtocol> {
        self.protocol
    }

    /// The retained uploads — read-only, for the renderer's diagnostics.
    pub(super) fn placements(&self) -> &PlacementRegistry {
        &self.placements
    }

    /// Delete the session's kitty uploads and forget them — pi's `beforeTerminalStop` image work
    /// (`tui-alt-screen.ts:306-308`, via `deleteKittyImages` `:336-338`).
    ///
    /// Call immediately BEFORE `terminal::TerminalSetup::leave`, which is where upstream writes it:
    /// inside the teardown bracket and ahead of `LeaveAlternateScreen`, so the delete is addressed
    /// to the screen the placements are on.
    ///
    /// `preserve_screen` is the same flag `TerminalSetup::leave` takes, and it gates the escape but
    /// never the registry: on a mode switch (`true`) the uploads must survive for the returning
    /// inline renderer, which shares this process's one `Protocol` cache and would otherwise
    /// reference freed images forever. See the module doc for why that divergence from upstream's
    /// unconditional delete is required rather than optional; the placements themselves leak
    /// nothing either way, since the cells that make them are discarded with the alternate screen.
    ///
    /// Swallows every write error, as every other teardown step in this module tree does
    /// (`terminal.rs`, `mouse.rs`): this can run while the process is already failing, and one
    /// rejected escape must not stop the rest of the restore.
    pub(super) fn delete_all(&mut self, preserve_screen: bool) {
        // pi's `deleteKittyImages()` is `imageProtocol === "kitty" ? deleteAllKittyImages() : ""`
        // (`:336-338`) — unconditional on the registry's contents, so a kitty terminal that showed
        // no image still gets the (no-op) delete, exactly as upstream. The registry is cleared
        // either way, which is upstream's `uploadedKittyImages.clear()` at `:308`.
        let kitty = self.protocol == Some(ImageProtocol::Kitty);
        self.placements.clear();
        if preserve_screen || !kitty {
            return;
        }
        let sequence =
            if wraps_for_tmux() { DELETE_ALL_KITTY_IMAGES_TMUX } else { DELETE_ALL_KITTY_IMAGES };
        let mut out = io::stdout();
        let _ = queue!(out, Print(sequence));
        let _ = out.flush();
    }

    /// Put the saved capability record back — pi's `afterTerminalStop` tail (`:330-333`).
    ///
    /// Call AFTER `terminal::TerminalSetup::leave`, which is upstream's position. The record
    /// written back is the value [`Self::enter`] read, never a rebuilt one, so the restore is
    /// byte-identical; `renderer` supplies the transcript gate's value the same way startup does
    /// (`app/shell.rs:406-408`: `set_graphical_images(image_renderer.is_graphical())`), which is
    /// derived state rather than a second saved copy that could drift.
    ///
    /// A no-op when nothing was suppressed, and after the first call — the `take` is what makes a
    /// [`Self::restore`] and the [`Drop`] below correct with respect to each other.
    pub(super) fn restore(&mut self, transcript: &mut TranscriptView, renderer: &ImageRenderer) {
        let Some(saved) = self.saved.take() else {
            return;
        };
        crate::image::set_capabilities(saved);
        transcript.set_graphical_images(renderer.is_graphical());
    }
}

impl Drop for ImageLifecycle {
    /// The un-taken exit: a `?` early return during setup, an ordinary scope exit, or a dropped
    /// future — the same three cases `terminal.rs`'s guard is written for, and the reason this one
    /// exists at all: the suppression writes a **process-global** (`image.rs:525-526`), so leaving
    /// it set would follow the user into the inline renderer and every later session in the
    /// process.
    ///
    /// Only the global is restored. The transcript's gate is not reachable from here, and no kitty
    /// delete is emitted: this path cannot know whether an inline renderer is about to resume, and
    /// [`ImageLifecycle::delete_all`]'s `preserve_screen == true` branch is the safe answer to that
    /// question for the reason the module doc gives.
    fn drop(&mut self) {
        if let Some(saved) = self.saved.take() {
            crate::image::set_capabilities(saved);
        }
    }
}

/// Everything one [`place`] call needs about the frame it is painting into, grouped so the painter
/// keeps the narrow read-only-refs shape ADR-0005 §Part 4 R4 requires of every alternate-screen
/// painter (`altscreen/mod.rs`, structural rule 1).
pub struct Strip<'a> {
    /// The negotiated renderer — `AppState::image_renderer` (`app/state.rs:59`).
    pub renderer: &'a ImageRenderer,
    /// The pending attachments, in order — `AppState::pending_images` (`app/state.rs:62`).
    pub blocks: &'a [ImageBlock],
    /// The live theme, for the placeholder line the suppressed and `show_images: false` branches
    /// draw.
    pub theme: &'a UiTheme,
    /// `AppState::show_images` — the user's own toggle (`show-images-selector.ts`), independent of
    /// the suppression this module applies on top of it.
    pub show_images: bool,
    /// `terminal.imageWidthCells` — `TranscriptView::image_width_cells`, pi's `maxWidthCells`
    /// (`components/image.ts:66`).
    pub width_cells: u16,
}

/// Paint the attachment strip into `area` and reconcile the placement registry — the alternate
/// screen's `crate::app::render_images` (`app/render.rs:227-246`), plus the halves of
/// `prepareKittyScreen` (`tui-alt-screen.ts:340-387`) that cyrup's rendering model leaves standing.
///
/// The geometry is `render_images`' own, deliberately unchanged: the same
/// `max(1, min(width - 2, imageWidthCells))` width rule (pi `components/image.ts:66`), the same
/// natural-height-clamped-to-the-slot row count, the same top-down stacking that stops at the
/// bottom of `area`. What is added is the two things this unit owns:
///
/// - **Suppression.** `show_images` is ANDed with [`ImageLifecycle::allows_graphics`], so while
///   iterm2 is suppressed each block draws `ImageBlock::placeholder_line` — pi's `imageFallback`,
///   the same branch `showImages: false` takes — and no iTerm2 sequence is emitted.
/// - **Placement tracking.** Every block actually drawn through the kitty protocol is touched in
///   the registry (upstream's re-insert at `:353-354`), and blocks that did not fit in `area` are
///   left untouched, which is what makes them off-screen residue for [`PlacementRegistry::evict`] —
///   upstream's `visibleImageIds` complement (`:365`, `:379`). Eviction runs once per frame, after
///   the walk, exactly as `prepareKittyScreen` does it.
///
/// Only kitty is tracked, which is upstream's gate too (`:1306-1309` calls `prepareKittyScreen`
/// solely when `imageProtocol === "kitty"`): iterm2 is suppressed for the duration, and sixel —
/// which `ratatui-image` re-sends in full on every draw — retains nothing between frames.
pub(super) fn place(
    lifecycle: &mut ImageLifecycle,
    frame: &mut Frame,
    area: Rect,
    strip: &Strip<'_>,
) {
    let generation = lifecycle.placements.begin_frame();
    let graphics = strip.show_images && lifecycle.allows_graphics();
    let track = graphics && lifecycle.protocol == Some(ImageProtocol::Kitty);
    let cell_px = strip.renderer.cell_pixels();
    // `render_images`' width rule (`app/render.rs:234-235`), which is pi's
    // `Math.max(1, Math.min(width - 2, maxWidthCells))` (`components/image.ts:66`).
    let max_cells = strip.width_cells.max(1);
    let width = area.width.saturating_sub(2).min(max_cells).max(1);
    let bottom = area.y.saturating_add(area.height);
    let mut y = area.y;
    for block in strip.blocks {
        if y >= bottom {
            break;
        }
        let want = strip.renderer.cell_size(block, width).1.max(1);
        let height = want.min(bottom.saturating_sub(y));
        let cell = Rect { x: area.x, y, width, height };
        strip.renderer.render(frame, cell, block, strip.theme, graphics);
        if track {
            let id = PlacementId {
                label: block.label().to_string(),
                dimensions: block.dimensions(),
                cells: (cell.width, cell.height),
            };
            lifecycle.placements.touch(id, (cell.width, cell.height), cell_px, generation);
        }
        y = y.saturating_add(height);
    }
    // `prepareKittyScreen`'s second pass (`:361-386`) — one eviction sweep per frame, over what the
    // walk above did not touch.
    lifecycle.placements.evict(generation);
}
