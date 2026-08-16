# TUI-092-F7 — Memoise the `ImageRenderer` protocol (don't re-encode every frame)

> **Part of** [`TUI-092-progressive-lockup.md`](TUI-092-progressive-lockup.md) (the umbrella audit).
> Self-contained change in `crates/cyrup-tui/src/image.rs`.
>
> **Kind** `cyrup-original` · **Severity** high · **Effort** S · **Phase driven** 2 while
> attachments sit (an attached screenshot idles at 12.5 encodes/s while the user types)

## Coordinates with

Nothing. Independent of F1–F6, F8. Natural companion to [F2](TUI-092-F2-transcript-render-cache.md)
— together they remove all per-frame image work (F2 removes the tool-result image rasterisation in
the transcript; F7 removes the attachment-strip protocol encode). Either lands alone.

---

## Evidence

[`image.rs:165-168`](../../../crates/cyrup-tui/src/image.rs#L165):
`self.picker.new_protocol(block.image.clone(), size, Resize::Fit(None))` — a full raster clone +
resize + (Kitty/iTerm2) base64 encode — runs **every frame** for **every** image in the attachment
strip ([`render_images`](../../../crates/cyrup-tui/src/app.rs#L7866) iterates
`state.pending_images` per frame). An attached screenshot idles at 12.5 encodes/s while the user
types. ([`Picker::new_protocol`](../../../tmp/ratatui-image-11.0.6/src/picker.rs#L256) returns an
owned, reusable `Protocol` — caching it is exactly what the library's own `StatefulImage` does.)

**Verified in the tree:** `ImageRenderer { picker }` at
[`image.rs:34`](../../../crates/cyrup-tui/src/image.rs#L34); `render(&self)` at
[`:152`](../../../crates/cyrup-tui/src/image.rs#L152); the per-frame `new_protocol(block.image.clone(),
…)` at [`:165`](../../../crates/cyrup-tui/src/image.rs#L165); `render_images` iterates per frame at
[`app.rs:7866`](../../../crates/cyrup-tui/src/app.rs#L7866); `pending_images.clear()` on submit at
[`app.rs:1462`](../../../crates/cyrup-tui/src/app.rs#L1462). `Picker::new_protocol` returns an owned,
reusable `Protocol` at
[`tmp/ratatui-image-11.0.6/src/picker.rs:256`](../../../tmp/ratatui-image-11.0.6/src/picker.rs#L256) —
caching it is the library's own `StatefulImage` pattern. `render` is `&self`, so the cache uses
interior mutability.

**Cost shape.** CPU/frame ∝ attached image px.

---

## FIX — memoise the built protocol inside `ImageRenderer`

Interior mutability; `render` takes `&self`:

```rust
// image.rs
pub struct ImageRenderer {
    picker: Picker,
    /// The last built protocol, keyed by (image identity, target size) — building it is a raster
    /// clone + resize + encode, so it happens on CHANGE, not per frame (TUI-092 F7).
    cache: std::sync::Mutex<Option<ImageProtocolCache>>,
}

struct ImageProtocolCache {
    label: String,
    dimensions: (u32, u32),
    size: ratatui::layout::Size,
    protocol: ratatui_image::Protocol,
}
```

`render` builds on a miss and draws `Image::new(&cached.protocol)` on a hit. The strip is cleared
on submit ([`app.rs:1462`](../../../crates/cyrup-tui/src/app.rs#L1462)), so the cache holds at most
one entry per distinct (image, size) — bounded by construction.

---

## Definition of done

* An attached image with an unchanged (image identity, target size) draws from the cache — zero
  `new_protocol` calls, zero raster clones, zero base64 encodes per frame after the first.
* The cache is bounded by construction (cleared on submit); no unbounded retention is introduced.

## Do not touch

The halfblock fallback path (`!self.is_graphical()` → placeholder line) and the
`pending_images.clear()` on submit — the cache lifetime is bounded by that clear, not by any new
eviction logic.