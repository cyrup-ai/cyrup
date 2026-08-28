---
stage: done
status: completed
updated: 2026-08-28
---

# Render Mermaid Fences As Unicode Diagrams And Wire The Orphaned `markdown.mermaid` Setting

> Identified by the `cyrup-tui` <-> pi port audit (fan-out + adversarial verification).
> **Priority:** high · **Effort:** large · Area: Markdown, latex, images, diffs and message rendering

## Objective

An assistant reply containing a ```` ```mermaid ```` fence should draw the diagram in the transcript
(live while streaming, by default), the way pi does, instead of printing the raw mermaid source in a
code block — and `/settings` should carry the row that turns it off/final/streaming. The
`markdown.mermaid` setting is already fully parsed in cyrup and is read by nothing, so today a user
can set it and nothing changes.

## Upstream reference

[`packages/coding-agent/src/modes/interactive/components/mermaid.ts:1-89`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/mermaid.ts)
is the whole feature:

- `isMermaid` (`:14-16`) — a `code` token whose info string's **first whitespace-separated word**,
  lowercased, is `mermaid`.
- `render(token.text)` from the `grok-mermaid` package returns `MermaidArt { plain, width, warnings,
  spans }`. **This is an external layout engine with no cyrup counterpart** and is the bulk of the
  work.
- `themedLines` / `styleSpan` (`:38-56`) colour each span class: `border` -> `borderMuted`, `edge` ->
  `accent`, `edgeLabel` -> `muted`, `title` -> bold `accent`.
- `codeSpan` (`:18-36`) re-encodes every diagram row as an **inline code span** with a backtick fence
  longer than any run inside it (and a non-breaking space for blank rows), so spacing and
  box-drawing survive re-parsing; rows are joined with markdown hard breaks (`.join("  \n")`,
  `:85`).
- Width fallback (`:76`): `if (!art || art.width > context.availableWidth) return token.raw;`
- Warning surfacing (`:77-82`), non-streaming only:
  `` `Mermaid diagram not rendered: ${art.warnings[0]}${suffix}` `` where `suffix` is
  `` ` (+${n-1} more)` `` for n > 1, themed `warning`, appended after the untouched raw fence.
- Mode gate (`:62-70`): pass the markdown through untouched when `mode === "off"`, or
  `context.messageType === "assistant-thinking"`, or `context.isStreaming && mode !== "streaming"`.

Registered as a built-in markdown transformer at `interactive-mode.ts:484-486`
(`createMermaidMarkdownTransformer({ getMode: () => this.settingsManager.getMermaidRenderingMode(),
theme })`), and exposed as the `/settings` row "Mermaid diagrams" / "Render Mermaid code blocks as
Unicode diagrams" with values `off|final|streaming` (`settings-selector.ts:501-506`, applied at
`:855-857`).

## Current state in cyrup-tui

Nothing ports `mermaid.ts`. `grep -rni "mermaid\|diagram" crates/cyrup-tui/src` returns three hits,
all prose: [`markdown/walk.rs:434-435`](../../crates/cyrup-tui/src/markdown/walk.rs) and
`src/tests/markdown.rs:849`, each quoting `mermaid.ts:15` only to justify keeping the whole fence
info string. There is no layout/diagram engine anywhere in the workspace *today* — subtask 1 adds `mermansi`.

- The fence path is **unconditional**.
  [`markdown/walk.rs:429-444`](../../crates/cyrup-tui/src/markdown/walk.rs) (`Tag::CodeBlock`) stores
  the trimmed info string, and `emit_code_block` (`:613-641`) renders ```` ```lang ```` + a
  syntect-highlighted or flat body + ```` ``` ````. `src/tests/markdown.rs:844-870`
  (`m17_code_fence_keeps_the_whole_info_string`) actively pins that a multi-word info string prints
  verbatim.
- The setting is **orphaned**.
  [`cyrup-config/src/settings/effective.rs:414-427`](../../crates/cyrup-config/src/settings/effective.rs)
  defines `mermaid_rendering_mode()` — a faithful port including pi's validate-not-parse rule
  (`off`/`final` pass, everything else including absent falls to `Streaming`) — and
  [`settings/manager.rs:362-372`](../../crates/cyrup-config/src/settings/manager.rs) the setter.
  `grep -rn mermaid` over `cyrup-tui`, `cyrup-modes` and `cyrup-agent` finds **zero** consumers.
- There is no "Mermaid diagrams" row: read the complete row list at
  [`app/settings_rows.rs:47-198`](../../crates/cyrup-tui/src/app/settings_rows.rs).

## Remaining scope

The whole feature is absent, but the layout engine is **not** written from scratch — subtask 1
selects `mermansi`. What remains is the fence gate, the re-encoding, the gate and the wiring. Note the mode gate depends on
`messageType` / `isStreaming` / `availableWidth`, which means it needs the per-message
markdown-transform seam — coordinate with the sibling gap **MARKDOWN_TRANSFORM_HOOK_NOT_WIRED**, or
land an equivalent per-message hook here.

## Subtasks

1. **Use the `mermaid-text` crate as the diagram engine (decided 2026-08-28 — recorded in the
   module doc and the workspace manifest).** The original claim in this task that "`grok-mermaid`
   is a JS package with no Rust equivalent" was **wrong and is retracted**: `cargo search mermaid`
   returns 2258+ crates, several rendering Mermaid to Unicode text in pure Rust.

   | crate | licence | verdict |
   |---|---|---|
   | `mermaid-text` 0.57.0 | MIT | **chosen** — 3 direct deps (`ascii-dag`, `chrono`, `unicode-width`; the last two already in tree), typed `Error`, public `Grid` |
   | `mermansi` 0.1.6 | MIT OR Apache-2.0 | **tried first, then REJECTED** — see below |
   | `mmdflux` 2.6.1 | MIT | viable; 243 source files, far larger surface than needed |
   | `flowmaid` 0.25.0 | GPL-3.0-or-later | **ruled out** — cyrup is MIT |

   **Why `mermansi` was backed out after being implemented against.** It pulls 151 transitive
   crates including a CSS engine, and one — `merman-core` 0.8.0-alpha.3, an alpha — declares
   `serde_json` with a NON-OPTIONAL `features = ["preserve_order"]`. Cargo feature unification is
   graph-wide and additive, so that flips `serde_json::Map` from `BTreeMap` to `IndexMap` for EVERY
   crate in the workspace, silently changing map ordering in config persistence, provider request
   bodies, MCP payloads and session records. It broke two pre-existing `cyrup-ext-subagents` tests
   — and only under `cargo test --workspace`; `-p cyrup-ext-subagents` alone still passed. Do not
   re-pick it without solving that at the root.

   **Known limitation, accepted deliberately.** pi themes four span classes (`mermaid.ts:38-56`).
   The diagram is rendered COLOURLESS under the single `md_code_block` role. This is deferred, not
   unreachable: `mermaid_text` renders into a public `layout::Grid` holding
   `fg: Vec<Vec<Option<Rgb>>>` beside its char grid, with `Grid::get` already public — only a
   `get_fg` accessor is missing. A ~5-line upstream PR plus a grid walk buys the full four classes
   with NO ANSI parser. Filed as `TUI_MERMAID_PER_SPAN_THEMING`.

   Use `mermaid_text::render`, not `render_with_width`: the latter compacts the gap configuration
   to fit a budget, which changes the drawing. pi measures the finished art and falls back to the
   raw fence (`mermaid.ts:76`), so the width check stays in cyrup.

2. **New module `crates/cyrup-tui/src/markdown/mermaid.rs`**: the `is_mermaid` fence gate (first word
   of the info string, lowercased), the `code_span` re-encoding with its variable-length backtick
   fence and NBSP blank rows, the hard-break join, and the `art.width > available_width` fallback to
   the raw fence.
3. **Theming**: map the four span classes onto the existing `UiTheme` roles used elsewhere in
   [`markdown/walk.rs`](../../crates/cyrup-tui/src/markdown/walk.rs) (`md_code_block_border_style`
   and friends), matching pi's `borderMuted` / `accent` / `muted` / bold-`accent` assignment.
4. **Warnings**: non-streaming renders with warnings emit the raw fence followed by the warning line
   `Mermaid diagram not rendered: <first> (+N more)` in the warning colour.
5. **Gate + wire**: read
   [`EffectiveSettings::mermaid_rendering_mode()`](../../crates/cyrup-config/src/settings/effective.rs)
   and apply pi's three pass-through conditions (off / thinking body / streaming while mode !=
   streaming). Hook it into the transcript's markdown path so it sees `is_streaming`,
   `message_type` and `available_width`.
6. **Settings row**: add `SettingRow::choices("markdown.mermaid", "Mermaid diagrams", …,
   ["off","final","streaming"])` with the description "Render Mermaid code blocks as Unicode
   diagrams" in [`app/settings_rows.rs`](../../crates/cyrup-tui/src/app/settings_rows.rs), at pi's
   position (`settings-selector.ts:501-506`), persisting through the existing
   `AppCommand::ApplySetting` path so
   [`SettingsManager::set_mermaid_rendering_mode`](../../crates/cyrup-config/src/settings/manager.rs)
   is what writes the global layer.

## Acceptance criteria

- [ ] `grep -rn "mermaid_rendering_mode" crates/cyrup-tui/src` returns at least one production call
      site (today: zero across the whole workspace outside cyrup-config's own tests)
- [ ] A `is_mermaid`-equivalent exists and matches only when the **first word** of the trimmed,
      lowercased info string is `mermaid`, so ```` ```mermaid title="x" ```` still qualifies and
      ```` ```mermaidish ```` does not
- [ ] A diagram wider than the available content width falls back to printing the raw fence verbatim
- [ ] Mode `off` renders the raw fence; a thinking body renders the raw fence; a streaming render
      with mode `final` renders the raw fence and the same content renders as a diagram once
      streaming ends
- [ ] Non-streaming output with warnings appends
      `Mermaid diagram not rendered: <first warning>` (plus ` (+N more)` when N > 1) in the warning
      colour, after the untouched fence
- [ ] `grep -n 'Mermaid diagrams' crates/cyrup-tui/src/app/settings_rows.rs` returns a row whose
      values are exactly `off`, `final`, `streaming`
- [ ] The existing pin `m17_code_fence_keeps_the_whole_info_string` in `src/tests/markdown.rs` still
      passes — a non-mermaid fence's info string is untouched
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
