---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_09 — `ctrl+f` interactive status overlay

## OBJECTIVE

Add the themed, interactive status panel to `cyrup-ext-flux` — the cyrup-native restoration of
Wibey's `ui-mode: flux-status` overlay (spec [§3.4.3](../flux.md)). `/flux/status` (FLUX_07)
owns the plain-text channel; this task owns real color, which only exists inside an overlay
because the TUI strips ANSI from external text (spec §0.4/§5.8).

## SUBTASKS

### SUBTASK 1: `overlay.rs` — `FluxStatusOverlay`

Implement [`InteractiveOverlay`](../../crates/cyrup-ext/src/host/overlay.rs) (contract:
`render(width, height) -> Vec<OverlayLine>`, `handle_key(OverlayKey) -> OverlayOutcome`,
`refresh_ms()`, `tick()`):

- State: the same `state.rs` model as FLUX_07 (re-collected on open; refresh on `tick` —
  `refresh_ms() = 2000` so the panel tracks exec/qa frontmatter transitions live).
- Rendering: reuse `render_status.rs`'s layout (columns, rules, glyphs) but emit styled
  `OverlaySpan` runs — this is where the Python palette finally lands: MAGENTA header/rules,
  CYAN names, WHITE column headers, GREY `(unknown)`/empty states, and the per-status colors
  (ORANGE in-progress, GREEN done/completed, RED needs-rework) and per-severity colors
  (critical RED, high ORANGE, medium TEAL, low GREEN). Palette values from
  [`flux_status.py`](../../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_status.py)'s
  ANSI table, mapped to the theme's named colors rather than raw ANSI.
- Keys: `Escape` → `OverlayOutcome::Close` (the Wibey behavior); everything else → `Ignored`.
- Header row paints `𝕱 FLUX STATUS` + a `ESC to close` hint (the overlay has no frame title —
  the component paints its own header, per the trait docs).
- The subagents fleet modal is the structural pattern:
  [`../../crates/cyrup-ext-subagents/src/background/fleet_view.rs`](../../crates/cyrup-ext-subagents/src/background/fleet_view.rs).

### SUBTASK 2: Shortcut registration + handler

In `extension.rs`:

```rust
// init:
api.register_shortcut("ctrl+f", Some("Flux status overlay".into()));
api.subscribe(&[]); // unchanged; no events needed for this task

// NativeExtension::execute_shortcut:
async fn execute_shortcut(&self, key: &str, ctx: &HostCtx) -> Result<(), ExtError> {
    ctx.require_command_tier()?;
    if key == "ctrl+f" {
        crate::overlay::open_status_overlay(&self.host_services);
    }
    Ok(())
}
```

`open_status_overlay(&OnceLock<Arc<dyn HostServices>>)`:

1. No host backend (headless print/json, or services not yet bound) → fall back to
   `notify` with the FLUX_07 plain table (the `open_overlay` `false`-return contract —
   [`services.rs`](../../crates/cyrup-ext/src/host/services.rs) :254: "a `false` return is the
   caller's cue to fall back … NOT an error").
2. Host present → `host.open_overlay(Box::new(FluxStatusOverlay::new()))`; on `false`, same
   notify fallback.

### SUBTASK 3: Build + behavioral check

```bash
cargo build -p cyrup-ext-flux && cargo build -p cyrup
```

- TUI, against the FLUX_07 fixture: `ctrl+f` opens the panel; colors match the palette;
  contents match `/flux/status`; the panel refreshes within ~2 s when a task file's
  frontmatter is edited on disk; `ESC` closes and focus returns to the editor.
- Headless: `cyrup -p "…"` (print mode) pressing no keys — instead invoke the fallback path by
  confirming `open_overlay` returns `false` there and the notify carries the plain table.
- `/hotkeys` (or the binary's shortcut-listing surface) shows `ctrl+f`.

## RESEARCH NOTES

- `register_shortcut` + `execute_shortcut` are the EXT-035 surface
  ([`native.rs`](../../crates/cyrup-ext/src/native.rs) :366, :589+); without the trait override
  the key is advertised but unroutable — implement both, as the trait docs warn.
- `OverlayKeyCode::Escape` arrives as an `OverlayKey`; no modifier parsing needed.
- Do not acquire the `HumanInteractionLock` here — the overlay is not a dialog prompt; the lock
  is for select/confirm/input (FLUX_10).

## DEFINITION OF DONE

- [ ] `ctrl+f` opens the themed overlay in the TUI; live-refresh works; `ESC` closes.
- [ ] Headless/no-host path falls back to the plain-table notify (no panic, no silent no-op).
- [ ] The shortcut appears in the hotkeys listing; `/flux/status` text output unchanged.

No tests to be written. No benchmarks to be written.
