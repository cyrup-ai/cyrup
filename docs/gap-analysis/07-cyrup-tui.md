# 07 — cyrup-tui

This area covers `cyrup/crates/cyrup-tui` (the interactive chat UI: transcript, editor, footer, selectors, themes, images, keymap, autocomplete, startup panel) plus the TUI wiring in `cyrup/crates/cyrup/src/main.rs`. It is measured against `pi/packages/tui/` (rendering primitives) and `pi/packages/coding-agent/src/modes/interactive/` (components and `interactive-mode.ts`) at pi v0.83.0. Headline finding: commit `d2c5509` (TUI-002..007) landed a large amount of real, correctly-ported behavior — thinking rendering, session replay, terminal theme probes, Escape queue restore, the startup panel and tool-result images — but every one of those six items shipped with residue, one of them (TUI-006) with residue large enough that its previous `closed` verdict is **overturned**, and the tool-result-image work is missing the capability gate its own commit message claims. Re-baselined against HEAD `1806375` on 2026-08-03 by reading both sides of every claim; no cargo, no npm.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| TUI-001 | **closed** | `6396841`. `stop_reason_notice()` (`cyrup/crates/cyrup-tui/src/app.rs:2886-2912`) matches `pi/packages/coding-agent/src/modes/interactive/components/assistant-message.ts:170-201` branch-for-branch, including pi's `length`-before-`hasToolCalls` asymmetry (`:175`). Wired on the live arm (`app.rs:2867`) **and** the replay walk (`app.rs:800-802`) — both checked, not assumed. Test `cyrup/crates/cyrup-tui/tests/stop_reason.rs:60-87` drives the real `App::ingest_event` seam. |
| TUI-002 | partially closed — **open** | `d2c5509`. `Entry::Thinking`, `thinkingText` role, hidden label, `set_hide_thinking_block` all real. Three residues remain: block-ordering fold, no markdown rendering, no `hasVisibleContentAfter` spacer. |
| TUI-003 | partially closed — **open** | `d2c5509`. `App::replay_session` (`app.rs:759-863`) is a faithful walk over `raw_context_messages()` with tool-call-id pairing. Residues: no `Session compacted N time(s)` status, no trust warning (now TUI-N04), async custom renderers unavailable on a sync walk, scrollback stacking (now TUI-N07). |
| TUI-004 | partially closed — **open** | `d2c5509`. `terminal_query.rs` (OSC 11 + DSR ?996 + DA1 sentinel) and `ThemeController::sync_with_terminal` are faithful. Residues: no live mode-2031 colour-scheme sync (documented, defensible); `/reload` never re-runs `applyFromSettings`/`setRegisteredThemes`. |
| TUI-005 | partially closed — **open** | `d2c5509`. `InterruptRestoreQueued` + `restore_queued_to_editor` match `restoreQueuedMessagesToEditor({abort:true})`. Residues: pi's bash-**mode** Escape branch missing; cyrup cancels a bash child even while streaming where pi's `else if` chain does not. |
| TUI-006 | **OVERTURNED** `closed` → partially closed — **open** | `d2c5509` built the panel and the quiet-startup diagnostics gating correctly, but `[Extension issues]` is 1 of pi's 4 sources. Command conflicts, built-in shadowing and shortcut conflicts remain exactly as invisible as when the item was filed. |
| TUI-007 | **closed** (with cross-reference) | `d2c5509`. The literal `[image]` placeholder is gone (grepped: zero hits) and images render. **Do not read this closure as tool-result-image parity** — the commit message's claimed capability fallback does not exist in the code; filed as TUI-N01. |
| TUI-008 | open | Seven unbound keybinding ids; refs re-checked at HEAD. `app.thinking.toggle` downgraded to S — its destination now exists. |
| TUI-009 | open | Unchanged; refs re-checked. |
| TUI-010 | open | Unchanged; refs re-checked. `d2c5509` added two more commit-time-frozen entry kinds to the same family. |
| TUI-011 | open | Unchanged; refs re-checked. |
| TUI-012 | open | Unchanged; refs re-checked. |
| TUI-013 | open | Unchanged; refs re-checked. |
| TUI-014 | open | Unchanged; refs re-checked. `1d87913` made dialogs and custom/tool renderers live, leaving widgets the last unrendered `ui.*` surface. |
| TUI-015 | open | Unchanged; refs re-checked. `d2c5509` raised the per-draw cost (thinking re-layout, image re-rasterization per token). |
| TUI-016 | open | Unchanged; refs re-checked. TUI-005 raised its value — Escape now performs the action pi's missing hint line advertises. |
| TUI-017 | partially closed — **open** | `d2c5509` ported `image_fallback_text` and gave `imageWidthCells` a reader on the tool-result path only. The attachment strip is untouched: every site re-read. |
| TUI-018 | open | Unchanged; refs re-checked. |
| TUI-019 | open | Unchanged; refs re-checked. |
| TUI-020 | open | Unchanged; refs re-checked. |
| TUI-021 | open | Unchanged; refs re-checked. |
| TUI-022 | open | Unchanged; refs re-checked. Two in-repo precedents for the direct-fd write now exist. |
| TUI-023 | open | Unchanged; refs re-checked. `ace01cb`'s summarization-retry arm has the identical frozen-delay shape. |
| TUI-024 | open | Unchanged; refs re-checked. |
| TUI-025 | open | Unchanged; refs re-checked. |
| TUI-026 | open | Unchanged; refs re-checked. Documented cyrup-original — do not change without asking. |

## Open items

> **⚠ THIS TABLE IS NOT THE COMPLETE OPEN SET.** 11 further items from the 2026-08-03
> surface-driven sweep live in their own table under `## Surface-sweep findings` (line ~580), with
> `-S` ids — **including 1 rated critical/high**. Enumerating only this table undercounts the
> area by 11 items, which is exactly how `SEAM-S01` (high) escaped a full audit pass on
> 2026-08-07. Count BOTH tables. See structural defect A in `00-residual-ledger.md`.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| TUI-005 | medium | not-ported | S | Escape branches: bash-mode clear missing; bash child killed while streaming |
| TUI-006 | medium | not-ported | M | `[Extension issues]` renders 1 of pi's 4 diagnostic sources |
| TUI-008 | medium | not-ported | M | Seven upstream global keybinding ids are unbound |
| TUI-009 | medium | not-ported | S | Double-Escape → tree/fork never implemented although `doubleEscapeAction` ships in /settings |
| TUI-012 | medium | not-ported | M | No argument autocomplete for `/model <prefix>` or `/login <prefix>` |
| TUI-014 | medium | not-ported | M | Extension widgets (`ui.set-widget`) stored host-side but never rendered |
| TUI-015 | medium | cyrup-original | M | No render coalescing — one draw per streaming event, no frame budget |
| TUI-016 | medium | parity-bug | M | Queued messages show a footer count, not pi's per-message list |
| TUI-017 | medium | parity-bug | S | Attachment image strip: rasterizes without a protocol, invented placeholder, no 60-cell cap |
| TUI-N01 | medium | parity-bug | S | Tool-result images rasterize on terminals with no image protocol |
| TUI-N02 | medium | not-ported | S | `/reload` does not re-emit the loaded-resources / diagnostics panel |
| TUI-N03 | medium | parity-bug | S | A theme chosen in `/settings` is applied live but never persisted |
| TUI-N04 | medium | not-ported | S | The untrusted-project warning banner is never rendered at startup |
| TUI-002 | low | parity-bug | M | Thinking blocks: fold-ordering, no markdown, no visible-content spacer |
| TUI-003 | low | parity-bug | S | Replay omits the compaction-count status |
| TUI-004 | low | upstream-drift | M | No live colour-scheme sync; `/reload` does not re-apply themes |
| TUI-010 | low | parity-bug | M | Ctrl+O expands only the live block; no status; committed entries always expanded |
| TUI-011 | low | not-ported | M | `/changelog` is a hardcoded stub; no "What's New" startup notice |
| TUI-013 | low | parity-bug | S | Quoted paths with spaces break `@`-mention autocomplete |
| TUI-018 | low | not-ported | M | Expanded startup help missing — only the 5-item compact bar |
| TUI-019 | low | upstream-drift | L | No alt-screen UI mode, mouse, scrollbars, prompt navigation |
| TUI-020 | low | not-ported | S | OSC-8 hyperlink capability detected and tested but never emitted |
| TUI-021 | low | upstream-drift | M | Cache-miss notices not implemented |
| TUI-022 | low | not-ported | S | `terminal.showTerminalProgress` is a dead setting — OSC 9;4 never emitted |
| TUI-023 | low | parity-bug | S | Retry indicator shows a frozen delay instead of counting down |
| TUI-024 | low | parity-bug | S | Footer context segment vanishes when usage is unknown instead of `?/{window}` |
| TUI-025 | low | stale-port | S | Slash-command metadata one baseline behind |
| TUI-026 | low | cyrup-original | S | Transcript prefixes `you:` / `assistant:` labels |
| TUI-N05 | low | parity-bug | S | Extension shortcuts can never override a built-in key; no conflict reported |
| TUI-N06 | low | parity-bug | L | `Entry::Thinking` freezes hide/show at commit time |
| TUI-N07 | low | parity-bug | L | Mid-session `/resume` cannot erase the previous session's scrollback |
| TUI-N08 | low | test-defect | S | `tests/image.rs` pins the invented `🖼` placeholder and the rasterize-anyway fallback |
| TUI-N09 | low | test-defect | S | `extension_dialog_countdown` asserts an exact countdown it cannot control |

## TUI-005 — Escape branches: bash-mode clear missing; bash child killed while streaming

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:1242-1263` `Action::Interrupt` cancels the bash block unconditionally at `:1244-1247` *before* testing `self.state.status.streaming`, and has no branch for "editor buffer is in bash mode". The restore half is correct: `AppAction::InterruptRestoreQueued` → run-loop arm `app.rs:3944-3959` (`drain_queue`, steering then follow-up, `restore_queued_to_editor` at `app.rs:890-902`, then `abort()`), matching pi's ordering and no-status contract. Bash mode is known to cyrup only for the editor rule colour (`cyrup/crates/cyrup-tui/src/editor.rs:1509-1520`).

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2635-2660` — four mutually exclusive `else if` branches on `onEscape`: streaming → `restoreQueuedMessagesToEditor({abort:true})`; bash child running → kill; bash *mode* (`text.trimStart().startsWith("!")`) → clear editor and exit bash mode (`:2640-2643`); empty editor → double-escape. Because they are exclusive, pi never touches a bash child while streaming. `getAllQueuedMessages` (`:4030-4042`) also folds `compactionQueuedMessages`.

**Impact** Escape in bash mode with a typed-but-unsent `!cmd` does nothing in cyrup where pi clears it. Escape during a turn that also has a `!`-child kills the child as collateral. Both are uncommon paths but the second is destructive.

**Fix** Restructure `Action::Interrupt` (`app.rs:1242-1263`) into pi's exclusive chain: streaming first (return `InterruptRestoreQueued` without touching bash), then bash-child cancel, then a new bash-mode branch clearing the editor and dropping the `!` prefix, then the empty-editor double-tap (TUI-009). Land with TUI-009 so the precedence is right in one pass. Separately confirm whether cyrup has a compaction queue for `drain_queue` to fold.

**Verify** App tests: (a) `!sleep 100` running + streaming, press Esc, assert the child is still alive and the queue restored; (b) editor holds `!foo`, nothing streaming/running, press Esc, assert the editor is empty and bash mode off.

## TUI-006 — `[Extension issues]` renders 1 of pi's 4 diagnostic sources

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/startup.rs:204-220` `extension_diagnostics` maps `ExtensionLoadDiagnostic` and nothing else. `StartupDiagnostics` (`cyrup/crates/cyrup-session-svc/src/services.rs:34-46`) has no field for command diagnostics, built-in command conflicts or shortcut diagnostics — a workspace grep for `command_diagnostics|shortcut_diagnostics|restrict_override` in `.rs` returns nothing. The panel itself is correct: `build_startup_lines` (`startup.rs:222-246`) emits pi's five listing sections gated on `show_listing()` (`:148-151`) then the diagnostic blocks unconditionally, matching `{force:false, showDiagnosticsWhenQuiet:true}`. cyrup *computes* the same invocation-name disambiguation pi does (`cyrup/crates/cyrup-ext/src/registry.rs:336-375`) and reports nothing about it. `StartupDiagnostics::models` is collected but never rendered in the TUI (only stderr pre-TUI, `cyrup/crates/cyrup/src/main.rs:222-234`).

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:1658-1680` — `[Extension issues]` is the union of extension load errors, `extensionRunner.getCommandDiagnostics()` (`pi/packages/coding-agent/src/core/extensions/runner.ts:648`), `getBuiltInCommandConflictDiagnostics()` (`interactive-mode.ts:569-582`) and `extensionRunner.getShortcutDiagnostics()` (`runner.ts:539`, populated at `:494-534`). Expanded listing bodies are scope-grouped per-path (`interactive-mode.ts:1574-1637`). `models.json error:` is shown at boot (`:937-940`) and on reload (`:5482-5485`).

**Impact** Two extensions registering `/deploy`, an extension shadowing built-in `/model`, or two extensions claiming the same shortcut are all silent — the exact invisibility class this item was filed for. The user sees an extension "loaded" and one of its commands simply never runs.

**Fix** Add `commands: Vec<CommandDiagnostic>`, `builtin_conflicts: Vec<…>` and `shortcuts: Vec<ShortcutDiagnostic>` to `StartupDiagnostics` (`services.rs:34-46`); emit them from `cyrup-ext`'s `registry.rs` disambiguation path (`:336-375`) and from `register_shortcut` (`registry.rs:388-395`, see TUI-N05); fold all three into `extension_diagnostics` (`startup.rs:204-220`) under the existing `[Extension issues]` heading. Secondary: scope-group the listing bodies in `push_listing` (`startup.rs:248-262`) and render `diagnostics.models` in the panel.

**Verify** `crates/cyrup-tui/tests/startup_resources_panel.rs`: two extensions declaring the same slash command produce a conflict line; an extension shadowing a built-in produces the built-in-conflict line; both appear with `quietStartup=true`.

## TUI-008 — Seven upstream global keybinding ids are unbound

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/keymap.rs:97-114` `Action::from_id` recognizes exactly 14 ids; `app.model.select`, `app.thinking.toggle`, `app.message.copy` and `app.session.{new,tree,fork,resume}` are all absent, and `Keymap::default` binds no Ctrl+L / Ctrl+T / Ctrl+X. cyrup also spells `app.pageUp`/`app.pageDown` (`keymap.rs:101-102`) where upstream has `tui.editor.pageUp`/`pageDown`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2673-2686` wires all seven: `app.model.select` → `showModelSelector`, `app.thinking.toggle` → `toggleThinkingBlockVisibility`, `app.message.copy` → `handleCopyCommand`, `app.session.{new,tree,fork,resume}`.

**Impact** A user's `keybindings.json` carrying any of these ids silently does nothing, and the documented default chords are dead keys.

**Fix** Extend `Action` and `Action::from_id` (`keymap.rs:97-114`) with the seven ids plus the `tui.editor.page*` names, and route them. `app.thinking.toggle` is now mechanical: `TranscriptView::set_hide_thinking_block` exists (`cyrup/crates/cyrup-tui/src/transcript.rs:480-482`) and the settings row already persists `hideThinkingBlock` (`app.rs:3364`). `app.model.select` has its destination built (`app.rs:1449-1470` `open_data_selector`) with no key routed to it.

**Verify** Keymap unit tests round-tripping each id, plus app tests asserting Ctrl+T flips `hide_thinking` and Ctrl+L opens the model selector.

## TUI-009 — Double-Escape → tree/fork never implemented although `doubleEscapeAction` ships in /settings

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:3395-3400` renders `SettingRow::choice("doubleEscapeAction", …, ["fork","tree","none"])` backed by `cyrup/crates/cyrup-config/src/settings.rs:768-772` (default `tree`); a workspace grep for `double_escape|doubleEscape|last_escape` finds only those two sites plus the unit test at `settings.rs:1795`. `Action::Interrupt` (`app.rs:1242-1263`) has no empty-editor/double-tap branch and `AppState` has no `last_escape` field (contrast `last_sigint`, used at `app.rs:1270-1281`).

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2635-2660`, the fourth `else if`.

**Impact** A live, persisted, documented setting has no consumer: choosing `fork` or `tree` changes nothing.

**Fix** Add `last_escape: Option<Instant>` to `AppState` mirroring `last_sigint`; in the final `Action::Interrupt` branch, on a second Escape inside the window dispatch `/tree` or `/fork` per `settings.double_escape_action()`. Must land with TUI-005 so the branch order matches pi.

**Verify** App test: two Escapes on an empty editor within the window emit the tree command; outside the window, nothing; with `none`, nothing.

## TUI-012 — No argument autocomplete for `/model <prefix>` or `/login <prefix>`

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/commands.rs:38` declares `has_arg_completion`; its only other occurrences in the crate are the two `const fn` constructors at `:82`/`:96` — there is no reader. `cyrup/crates/cyrup-tui/src/autocomplete.rs:140` `slash_context` bails as soon as `before` contains whitespace; the only other contexts are `path_context` (`:171`) and `mention_query` (`:253`). No `CompletionContext::CommandArg` exists.

**upstream** — `pi/packages/tui/src/autocomplete.ts:338-358` plus `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:592-616` (`/model` via `getArgumentCompletions`) and `:619-629` (`/login`).

**Impact** Typing `/model anth<Tab>` gives nothing; the user must know the exact `provider/model` string or open the selector. `has_arg_completion` is dead metadata claiming a feature that does not exist.

**Fix** Add `CompletionContext::CommandArg { command, prefix }` to `autocomplete.rs`, produced when `before` starts with `/`, names a command with `has_arg_completion`, and has exactly one whitespace run; feed it from the model catalog and the provider list already reachable through the session services.

**Verify** Autocomplete unit tests for `/model anth`, `/login op`, and a negative for a command without `has_arg_completion`.

## TUI-014 — Extension widgets (`ui.set-widget`) stored host-side but never rendered

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/src/host/services.rs:1342-1345` pushes the payload onto `widgets: Mutex<Vec<Value>>` (`:1042`), with the default trait impl an empty body (`:234`). A grep for `widget` across `cyrup/crates/cyrup-tui/src` returns only ratatui's own `render_widget`/`widgets::` plus `flatten_widget_json` for extension *dialog* bodies (`app.rs:3080-3111`) — a different mechanism. `AppState` has no widget container and `live_region_height` reserves no rows.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:441-444, 503-504, 759-776, 2000-2038` — widgets are mounted into the live region above the editor and updated in place.

**Impact** An extension calling `ui.set-widget` gets a success return and draws nothing. Status widgets — the intended surface for long-running extension state — are unusable.

**Fix** Give `AppState` a widget list keyed by extension id, populate it from the host service callback through the existing extension→TUI command channel (the same path `1d87913` used for dialogs), render it in the live region above the editor, and include its height in `live_region_height`.

**Verify** App test: an extension emits `ui.set-widget`, assert the text appears in the live region and that a second emit replaces rather than appends.

## TUI-015 — No render coalescing — one draw per streaming event, no frame budget

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:4118-4135`: the run loop's event arm is `maybe_ev = events.next() => { … self.draw_synchronized()?; }` — one full draw per `MessageUpdate`/`TextDelta`. `draw` (`app.rs:906-945`) queries backend size, recomputes `live_region_height`, may rebuild the viewport and calls `flush_committed` every time. No dirty flag, no interval batching.

**upstream** — `pi/packages/tui/src/tui.ts:332` `MIN_RENDER_INTERVAL_MS = 16` and `:745-780` `requestRender` — renders are requested, coalesced and rate-limited.

**Impact** On a fast stream cyrup redraws per token. Since `d2c5509` the per-draw cost is higher: every frame re-runs `thinking_lines` for the live block, and on any turn whose tools returned images `image_raster_lines` (`transcript.rs:970-981`) re-rasterizes each image — an image re-encode per token.

**Fix** Introduce a `needs_render` flag plus a `MIN_RENDER_INTERVAL` (16 ms) timer in the run loop: event arms set the flag, a `tokio::time::interval` arm performs the draw. Memoize `image_raster_lines` output per `(image, width_cells)` in `ToolRun`.

**Verify** Bench/counter test: N streaming deltas within one interval produce one draw; an image-bearing turn rasterizes once, not per delta.

## TUI-016 — Queued messages show a footer count, not pi's per-message list

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:2659-2661`: the `QueueUpdate` arm calls only `status.set_queued(steering.len() + follow_up.len())`, discarding the texts; `cyrup/crates/cyrup-tui/src/status.rs:124` and `:275-279` render that as the cyrup-invented `N queued` footer segment.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:4045-4062` `updatePendingMessagesDisplay` — per-message `Steering: {text}` / `Follow-up: {text}` `TruncatedText` rows above the editor, plus the `↳ {key} to edit all queued messages` hint; it folds `compactionQueuedMessages` in via `getAllQueuedMessages` (`:4030-4042`).

**Impact** The user cannot see *what* is queued, cannot tell steering from follow-up, and is never told the queue can be edited — which matters more now that TUI-005 landed the Escape restore that hint advertises.

**Fix** Carry the message texts on `QueueUpdate` into `AppState`, render truncated rows in the live region above the editor, and append the hint line resolved from the keymap. Keep the footer count or drop it, but not as the only surface.

**Verify** App test: two steering + one follow-up produce three labelled rows in the right order plus the hint line; the rows clear when the queue drains.

## TUI-017 — Attachment image strip: rasterizes without a protocol, invented placeholder, no 60-cell cap

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — every site re-read at HEAD. `ImageRenderer::from_capabilities` (`cyrup/crates/cyrup-tui/src/image.rs:66-74`) keeps the half-block picker when `caps.images == None`; `ImageRenderer::render` (`image.rs:107-129`) rasterizes for that case and takes the placeholder branch only on `!show_images`, a zero area, or an encode error; `cell_size` (`image.rs:89-105`) clamps to the passed-in width only; `render_images` (`app.rs:3661-3673`) passes `area.width` with no `image_width_cells` cap; `ImageBlock::placeholder_line` (`image.rs:197-207`) emits the cyrup-invented `🖼 {label} ({w}×{h})`. `image_fallback_text` (`image.rs:311-330`) — pi's real format — exists in the same file and is used only by the tool-result path.

**upstream** — `pi/packages/tui/src/components/image.ts:65` `Math.max(1, Math.min(width - 2, this.options.maxWidthCells ?? 60))` and `:114-118`, which emit one `imageFallback` line whenever `!caps.images`. Format defined at `pi/packages/tui/src/terminal-image.ts:546-558`.

**Impact** Attaching an image on a terminal with no image protocol dumps a coloured half-block raster (unreadable on monochrome, ~20-30 rows of scrollback) instead of one `[Image: …]` line, and on a wide terminal the raster is unbounded where pi caps at 60 cells.

**Fix** In `image.rs`: (a) take the placeholder branch in `render` when `caps.images.is_none()`; (b) replace `placeholder_line`'s `🖼 …` with `image_fallback_text` from the same file; (c) clamp `cell_size`/`render_images` to `min(width - 2, image_width_cells)` with pi's 60 default. The tool-result path's own capability gate is TUI-N01 — different call site, different fix. `crates/cyrup-tui/tests/image.rs` currently pins (a) and (b) — see TUI-N08.

**Verify** With a no-protocol capability set, an attachment renders exactly one `[Image: …]` line; with a graphical set, the raster paints and never exceeds 60 cells.

## TUI-N01 — Tool-result images rasterize on terminals with no image protocol

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/transcript.rs:920`: `let inline = images.show && !run.images.is_empty() && run.images.iter().all(|i| i.block.is_some());` — the gate consults `terminal.showImages` and decodability only. `ImageOpts` (`transcript.rs:937-940`) carries just `show` and `width_cells`, built at `transcript.rs:824` from `TranscriptView` fields whose only writers are settings (`app.rs:1814`, `:2320`, `:3867`, `:4169`). `App::detect_image_support` (`app.rs:660-664`) does set `state.capabilities` and `state.image_renderer` from the real probe, but neither reaches the transcript. The raster is `ImageBlock::halfblock_lines` (`image.rs:227+`) off a hard-coded `Picker::halfblocks()`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/tool-execution.ts:331-334`: `const caps = getCapabilities(); … if (caps.images && this.showImages && img.data && img.mimeType)` — no protocol means no `Image` child at all, and `getTextOutput` supplies the one-line `imageFallback`. Same rule at the component level, `pi/packages/tui/src/components/image.ts:70,114-118`.

**Impact** On a plain xterm, the Linux console, CI, or a pipe, a `read` of a screenshot dumps ~20-30 rows of coloured `▀` into scrollback where pi prints one `[Image: shot.png [image/png] 1920x1080]` line — unreadable on monochrome. `d2c5509`'s commit message asserts it "falls back to `[image]` where the terminal cannot display them"; the code does not, so this defect is currently invisible to anyone reading history. `crates/cyrup-tui/tests/tool_result_images.rs:16-21` frames the substitution as an upside, which holds only where the terminal shows colour.

**Fix** Add `graphical: bool` (or the whole `TerminalCapabilities`) to `ImageOpts` (`transcript.rs:937`), seed it from `AppState::image_renderer.is_graphical()` wherever `show_images` is already pushed into the transcript (`app.rs:1814`, `:2320`, `:3867`, `:4169`), and require it in the `inline` gate at `transcript.rs:920` so a no-protocol terminal takes the existing `push_image_fallbacks` branch. Keep the half-block raster for the graphical case — that ADR-0001 rationale (`image.rs:216-226`) is sound and orthogonal.

**Verify** Extend `crates/cyrup-tui/tests/tool_result_images.rs`: with a no-protocol capability set, a finished `read` whose result carries a PNG commits exactly one `[Image: [image/png] WxH]` line and paints zero red cells; with a graphical capability set the raster still paints.

## TUI-N02 — `/reload` does not re-emit the loaded-resources / diagnostics panel

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:2435-2444` (`C::Reload`) only sets `pending_swap_status = "reloaded resources"`. Traced end-to-end: `rt.reload(None)` (`cyrup/crates/cyrup-session-svc/src/runtime.rs:507-524`) → `install_inner` bumps `generation` and sends `gen_tx` (`:309-317`) → the run loop's `session_swapped` arm (`app.rs:4143-4189`) re-subscribes, `rebind_session`, re-reads outputPad/hideThinkingBlock/showImages/imageWidthCells, replays the conversation, re-sources shortcuts — and never calls `App::push_loaded_resources` (`app.rs:874-876`), whose only call site is `cyrup/crates/cyrup/src/main.rs:1346` on the boot path. `startup.rs:145-147` states the omission outright.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:5477-5480`: inside `handleReloadCommand`, after `session.reload`, `keybindings.reload`, `setRegisteredThemes` and `themeController.applyFromSettings`, pi calls `showLoadedResources({ force: false, showDiagnosticsWhenQuiet: true })` — the identical options object it uses at boot (`:1769`). It also re-surfaces `models.json error:` (`:5482-5485`).

**Impact** `/reload` is the command a user runs right after editing an extension, skill or prompt. If the edit broke the extension, shadowed a skill name or introduced a prompt conflict, cyrup says only `reloaded resources` and the diagnostics TUI-006 built are never shown. The data is re-collected server-side by the factory and discarded.

**Fix** In the `session_swapped` arm (`app.rs:4143-4189`), after the settings re-read and before the replay, push the panel for the swapped-in session. `build_startup_report` is private in `cyrup/crates/cyrup/src/main.rs:1198`; move it to `cyrup-session-svc` or expose it as `cyrup_tui::StartupReport::from_session` behind the existing services accessors. pi does not gate it by swap reason, so neither should we initially. Pair with TUI-025's `/reload` status string and TUI-004's remaining `applyFromSettings`.

**Verify** In `crates/cyrup-tui/tests/startup_resources_panel.rs`, drive a `session_swapped` whose services report one extension load error and assert `[Extension issues]` lands in committed scrollback even with `quietStartup=true`.

## TUI-N03 — A theme chosen in `/settings` is applied live but never persisted

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:1794-1799`: `SelectorKind::Theme => { self.set_theme(UiTheme::builtin(value)); self.state.transcript.push_status(format!("theme → {value}")); None }`. Returning `None` means `handle_selector_key` (`app.rs:1713-1720`) yields `AppAction::Redraw`, no `AppCommand` reaches the run loop, and the `persist_setting(Global, …)` call at `app.rs:2327` is never made. The design is stated at `app.rs:1788-1791`. The submenu is the only entry point (`app.rs:1774-1777`, from `SettingRow::submenu("theme", …)` at `app.rs:3313`) — `open_selector(` has exactly one call site workspace-wide.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:4300-4302`: `onThemeChange: (themeSetting) => { this.settingsManager.setTheme(themeSetting); void this.themeController.applyFromSettings(); }`. `onThemePreview` (`:4303` → `themeController.preview`) is the separate, non-persisting hook — pi distinguishes preview from confirm; cyrup treats confirm as a preview that sticks until exit.

**Impact** The only in-app way to change theme does not survive the session. Worse with TUI-004 landed: `ThemeController::sync_with_terminal` persists a high-confidence OSC-11 detection into `settings.theme` only when the setting is UNSET (`cyrup/crates/cyrup-tui/src/theme.rs:1073-1081`) — exactly the state a never-persisted user choice leaves behind — so the next launch writes the auto-detected theme over the user's choice.

**Fix** Give `confirm_selector`'s `Theme` arm an `AppCommand::ApplySetting { id: "theme", value }` return instead of `None`; the arm at `app.rs:2297-2330` already persists to Global and pushes the status. Keep `set_theme` for the immediate repaint. While there: `SelectorKind::Thinking` (`app.rs:1800-1808`) and `SelectorKind::ShowImages` (`:1809-1816`) have the same non-persisting shape but are currently **unreachable** — no `open_selector` call site constructs them, and the live paths are Shift+Tab (`app.rs:1320` → `AppCommand::CycleThinking` → `session.cycle_thinking_level()`, correct) and the `terminal.showImages` settings-grid toggle (correct). Either wire them like pi or delete them; do not leave a third half-live path.

**Verify** App test: open the settings selector, drive the `theme` submenu to `light`, confirm, assert an `ApplySetting{id:"theme", value:"light"}` command is emitted, plus a settings-layer assertion that the global layer holds `"theme": "light"`.

## TUI-N04 — The untrusted-project warning banner is never rendered at startup

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — a grep for `not trusted` across `cyrup/crates/cyrup-tui/src` and `cyrup/crates/cyrup/src` finds only `cyrup/crates/cyrup/src/subcommands.rs:408` (an `--approve` message on a non-interactive package path). The interactive boot sequence (`cyrup/crates/cyrup/src/main.rs:1264-1372`) pushes the startup panel (`:1346`) and replays the session (`:1357-1360`) with no trust check, and `App::replay_session` (`app.rs:759-858`) ends at `commit_tools()`. Both halves already exist: `AgentSessionServices::project_trusted` (`cyrup/crates/cyrup-session-svc/src/services.rs:68`, read at `app.rs:1998` for the `/trust` dialog) and `cyrup_config::has_trust_requiring_resources` (`cyrup/crates/cyrup-config/src/trust.rs:196-208`).

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3554` — `renderInitialMessages()` calls `renderProjectTrustWarningIfNeeded()`, body at `:3565-3583`: when `!isProjectTrusted() && hasTrustRequiringProjectResources(cwd)` it emits a `theme.fg("warning", …)` line reading `This project is not trusted. Project {CONFIG_DIR_NAME} resources and packages are ignored. Use /trust to save a trust decision, then restart pi.`

**Impact** Open cyrup in a repo shipping `.cyrup/` skills, prompts, themes or settings that has not been trusted, and those resources are silently ignored with no indication and no pointer to `/trust`. It is the surface that tells the user a security decision is in force.

**Fix** After `push_loaded_resources` and before the replay in `cyrup/crates/cyrup/src/main.rs:1346-1357`, and in the `session_swapped` arm alongside the other per-session re-reads, evaluate `!services.project_trusted && cyrup_config::has_trust_requiring_resources(cwd, home)` and push a warning-styled entry with pi's string rebranded (`.cyrup`, `/trust`, `cyrup`). Reuse `StartupRole::Warning` (`startup.rs:39-41`) rather than inventing a role.

**Verify** App test over a temp cwd containing `.cyrup/skills/x.md` with `project_trusted = false`: committed scrollback contains `This project is not trusted` in warning style; absent when `project_trusted = true` or when the cwd has no trust-requiring resources.

## TUI-002 — Thinking blocks: fold-ordering, no markdown, no visible-content spacer

**Kind** parity-bug · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `thinking_text` (`cyrup/crates/cyrup-tui/src/transcript.rs:1857-1867`) folds *every* thinking block of the message into one string joined by `\n\n`, committed before the answer text (`app.rs:2842-2853`); the replay path does the same fold (`app.rs:779-786`). `thinking_lines` (`transcript.rs:868-878`) emits raw `Line::styled` per source line — documented as deliberate at `transcript.rs:861-867`. No `hasVisibleContentAfter` spacer rule.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/assistant-message.ts:104-166` walks `message.content` in index order, keeping each *run* of adjacent thinking blocks as its own `Markdown` section (run collector at `:115-127`) interleaved with text sections, renders each through `Markdown(..., {color, italic})` (`:145-165`), and adds a blank only when more visible assistant content follows (`:133-137`, `:163-165`).

**Impact** On interleaved-thinking models, all reasoning is hoisted above all prose instead of being interleaved as the model produced it; markdown in reasoning renders as literal syntax; spacing differs at the end of a message. `crates/cyrup-tui/tests/thinking.rs:147-163` exercises only adjacent blocks, so no test pins the divergence either way.

**Fix** Change `thinking_text` to return `Vec<String>` of adjacent-run sections and have `app.rs:2842-2853` (and the replay walk at `app.rs:779-786`) commit them interleaved with text sections in content order. Route each section through the existing markdown renderer with the `thinkingText` style. Add the `has_visible_content_after` spacer condition.

**Verify** Extend `tests/thinking.rs` with an interleaved `[think, text, think, text]` message and assert commit order; a markdown assertion that `**bold**` in reasoning renders bold-italic, not literally.

## TUI-003 — Replay omits the compaction-count status

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `App::replay_session` (`cyrup/crates/cyrup-tui/src/app.rs:759-863`) ends at `commit_tools()` (`:857`) with no compaction status. A grep for `Session compacted|compaction_count` over `cyrup/crates/cyrup-tui/src` and `cyrup/crates/cyrup/src` returns nothing. (A replayed extension `custom` message also draws with the built-in `[type] body` framing because the renderer lookup is async and this walk is synchronous — self-documented at `app.rs:753-758`; accepted, not owed work.)

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3557-3562` — after `renderInitialMessages`, pi emits `Session compacted {N} time(s)` when the count is non-zero.

**Impact** A resumed session that has been compacted gives no indication that earlier context was summarized away, so the user cannot explain why the model has forgotten something.

**Fix** Expose the compaction count on the session accessor already used by `replay_session`, and push a status line after `commit_tools()` (`app.rs:857`) when it is non-zero.

**Verify** `crates/cyrup-tui/tests/session_replay.rs`: a session whose context carries two compaction summaries produces `Session compacted 2 time(s)` after the replayed conversation; a fresh session produces nothing.

## TUI-004 — No live colour-scheme sync; `/reload` does not re-apply themes

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

> **RE-RATED 2026-08-06, and two claims in the original entry were wrong.** This entry previously read
> *"the rationale … is sound"* and *"the probe machinery itself is faithful"*, and was rated `low`.
> Both statements were false, and a user hit the consequence live: a stray OSC-11 reply
> (`11;rgb:0c0c/0b0b/1313`) typed itself into the prompt on launch. See **TUI-N09** for the bug and
> its fix (`b0e57b3`). Two lessons were recorded from this, both structural rather than local:
>
> 1. **The ADR-0001 substrate carve-out was applied too broadly.** "cyrup delegates rendering to
>    ratatui, so pi's hand-rolled renderer is out of scope" is correct for the *drawing* layer. It
>    was silently extended to behaviour that lives in pi's `TuiBase` but is not rendering at all —
>    input sanitation, terminal-reply handling, mode negotiation. Those are portable and in scope.
>    Before invoking ADR-0001 on a `tui.ts` line, check whether it draws anything.
> 2. **Not-enabling a feature does not make its hazards moot.** The reasoning "mode 2031 is off, so
>    unsolicited pushes cannot arrive" ignored that cyrup *does* issue the OSC-11 background query
>    and so must handle its reply — and that a reply can arrive late, after the read loop has moved
>    on. pi handles both in one place (`handleTerminalInput`); cyrup handled neither.

**cyrup** — mode `2031` is never enabled and nothing consumes an unsolicited `CSI ? 997 ; N n`. The
rationale at `cyrup/crates/cyrup-tui/src/theme.rs:1088-1097` states crossterm surfaces no such
event — true as far as it goes, but it was written as a justification for *not handling terminal
replies at all*, which is the part that was wrong (TUI-N09). Separately, `/reload` never touches the
`ThemeController`: the `C::Reload` arm (`app.rs:2435-2444`) and the swap arm (`app.rs:4143-4189`) do
not call it, and the controller is owned by `cyrup/crates/cyrup/src/main.rs` and consulted once at
boot (`main.rs:1272-1294`). The probe machinery (`terminal_query.rs`, `theme.rs:918-947`,
`:1056-1084`) is faithful *in what it sends*; what it does with what comes back was not, until
`crates/cyrup-tui/src/stray_reply.rs` landed.

**upstream** — `pi/packages/tui/src/tui.ts:686,716,737` enables/disables mode 2031 and re-themes via `onTerminalColorSchemeChange` (`pi/packages/coding-agent/src/modes/interactive/theme/theme-controller.ts:34`). `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:5471` awaits `themeController.applyFromSettings()` on reload, between `setRegisteredThemes` and `showLoadedResources`.

**Impact** (1) Flipping the OS/terminal to dark mode mid-session leaves cyrup on the old palette until restart — a documented divergence, keep as such. (2) Editing a custom theme file and running `/reload` does not pick it up, and newly-registered extension themes are not re-registered.

**Fix** For (2): plumb the `ThemeController` into the app or expose a `ReapplyThemes` command, and in the `session_swapped` arm call `set_registered_themes(...)` then `apply_from_settings()` before the replay — the same place TUI-N02 adds the panel re-emit. Leave (1) as a recorded divergence in `theme.rs:1088-1097` unless crossterm gains an event.

**Verify** App test: `/reload` with a changed `settings.theme` and a newly-registered extension theme repaints to the new palette and lists the new theme.

## TUI-010 — Ctrl+O expands only the live block; no status; committed entries always expanded

**Kind** parity-bug · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:1284-1294` `Action::ToolsExpand` toggles only `toggle_bash_expanded()`/`toggle_tool_expanded()` and returns `Redraw` with no status line. Committed entries render permanently expanded (`transcript.rs:1715` `tool_lines(run, true, …)`), and `Entry::CompactionSummary` emits its whole summary unconditionally (`transcript.rs:1743-1746`).

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3883-3903` `setToolsExpanded` fans out to every message component and calls `showStatus('Tool output: expanded'|'collapsed')`; `pi/packages/coding-agent/src/modes/interactive/components/compaction-summary-message.ts:11,39-57` collapses by default.

**Impact** Compaction summaries, branch summaries and skill bodies flood the transcript with no way to collapse them, and Ctrl+O gives no feedback that it did anything.

**Fix** Adopt collapsed-at-commit-time (now the established pattern — `startup.rs:19-25` states the concession, and `d2c5509` added `Entry::Thinking` to the same family): render `CompactionSummary`, branch and skill entries collapsed with a `Ctrl+O`-style hint, apply the current expand flag at commit time, and add the `showStatus` line to `Action::ToolsExpand`.

**Verify** App test: a committed compaction summary renders as one collapsed line by default; with expand on before commit it renders in full; Ctrl+O pushes the status string.

## TUI-011 — `/changelog` is a hardcoded stub; no "What's New" startup notice

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:1179-1181` is verbatim `"changelog" => { self.state.transcript.push_block("What's New", "No changelog entries found."); }`. `collapseChangelog` is a live settings row with no consumer (`app.rs:3366`); `last_changelog_version` a live accessor with none (`cyrup/crates/cyrup-config/src/settings.rs:882`).

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:687-716` (parse and render the shipped changelog, honouring `collapseChangelog`) and `:2777-2781` (the startup notice gated on `lastChangelogVersion` vs the current version).

**Impact** `/changelog` always claims there is nothing, and two settings with persisted state have no effect. Users are never told what changed on upgrade.

**Fix** Embed the changelog at build time (or read it from the install dir), parse per-version sections, render honouring `collapseChangelog`, and at startup compare the current version against `last_changelog_version`, showing the notice and then persisting the version.

**Verify** Unit test on the parser plus an app test: with `last_changelog_version` behind, boot emits the notice and persists the current version; a second boot does not.

## TUI-013 — Quoted paths with spaces break `@`-mention autocomplete

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/autocomplete.rs:168` `PATH_DELIMS = [' ', '\t', '"', '\'', '=']`; `:200-207` `trailing_token` splits on the *last* delimiter (`before.rfind(PATH_DELIMS)`), so `see @"my dir/fi` yields `dir/fi` and `mention_query` (`:253-258`) fails its `strip_prefix('@')`. There is no unclosed-quote scan in the file.

**upstream** — `pi/packages/tui/src/autocomplete.ts:54-72` `findUnclosedQuoteStart`, `:74-92` `extractQuotedPrefix`, `:463-470` `extractAtPrefix` — pi scans back for an unclosed quote first and treats everything after it as one token.

**Impact** Any path containing a space cannot be completed via `@`, on a project layout where such paths are common.

**Fix** Port `findUnclosedQuoteStart` into `autocomplete.rs` and consult it in `trailing_token` (`:200-207`) before falling back to `rfind(PATH_DELIMS)`; make `mention_query` accept the quoted form.

**Verify** Autocomplete unit tests for `@"my dir/fi`, `@'my dir/fi`, and a closed-quote negative.

## TUI-018 — Expanded startup help missing — only the 5-item compact bar

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/chrome.rs:75-87` `compact_hints` builds exactly pi's five `compactInstructions`; `app.rs:3578` renders that single row gated on `show_startup_hints`. There is no expanded form and no expandable header state; a grep for `CARGO_PKG_VERSION|logo` across `cyrup/crates/cyrup-tui/src` returns nothing.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:789-836` — the expanded header with logo, version, cwd, model and the onboarding lines, toggled by Ctrl+O.

**Impact** New users get five terse hints and no orientation; the version is not visible anywhere in the UI. TUI-006 landed the loaded-resources half of pi's `Press {ctrl+o} to show full startup help and loaded resources.` line, but cyrup's panel is unconditionally expanded (`startup.rs:19-25`), so the Ctrl+O affordance still has nothing to toggle.

**Fix** Add an expanded startup header in `chrome.rs` (logo, `CARGO_PKG_VERSION`, cwd, model, onboarding lines), hold an `expanded` flag in `AppState`, and route Ctrl+O to it alongside TUI-010's tool expansion, matching pi's shared toggle.

**Verify** App test: default boot shows the compact bar; Ctrl+O shows the version string and onboarding lines; a second Ctrl+O collapses.

## TUI-019 — No alt-screen UI mode, mouse, scrollbars, prompt navigation

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/lib.rs:3` states the inline viewport (not the alternate screen); `App::into_stdout` (`app.rs:3677-3695`) enables raw mode, `EnableBracketedPaste` and the Kitty disambiguation flag only — no `EnterAlternateScreen`, no `EnableMouseCapture`. The crate's only `EnterAlternateScreen` is the pre-session startup wizard (`cyrup/crates/cyrup-tui/src/startup_selector.rs:20,44`), not the chat UI. No `uiMode`/`fullscreenScrollbar` settings row, no `tui.altScreen.*` ids.

**upstream** — `pi/packages/tui/src/TuiAltScreen.ts` (post-baseline) plus the `uiMode` setting and the scrollbar / mouse / prompt-navigation surfaces built on it.

**Impact** No fullscreen mode, no mouse scrolling, no scrollbar, no jump-to-previous-prompt. Users on the inline viewport rely entirely on native terminal scrollback.

**Fix** Large and architectural (ADR-0001 chose the inline viewport deliberately). If pursued: add an alt-screen `App` variant behind a `uiMode` setting, owning its own scroll state and rendering the full transcript, with `EnableMouseCapture` and a scrollbar; keep the inline path as default.

**Verify** Out of scope until scoped as a project; at minimum a decision recorded in `lib.rs`'s ADR notes.

## TUI-020 — OSC-8 hyperlink capability detected and tested but never emitted

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/image.rs:365` carries `TerminalCapabilities.hyperlinks` with the tmux `client_termfeatures` probe (`:378-381`), exercised by `crates/cyrup-tui/tests/image_capabilities.rs`, and nothing reads it: `cyrup/crates/cyrup-tui/src/markdown.rs:398-407` `TagEnd::Link` unconditionally appends ` ({href})`. `image_fallback_text`'s own doc comment (`image.rs:309-310`) records the same omission for the filename.

**upstream** — `pi/packages/tui/src/components/markdown.ts:543-546` emits OSC-8 when the capability is present; `pi/packages/tui/src/terminal-image.ts:552-556` links the image filename.

**Impact** Markdown links render as noisy `text (https://…)` even on terminals that support clickable links, and image fallback filenames are not clickable. Fully-built detection is dead code.

**Fix** Thread `TerminalCapabilities.hyperlinks` into the markdown renderer and `image_fallback_text`; when true, wrap in `\x1b]8;;{href}\x1b\\{text}\x1b]8;;\x1b\\` and drop the parenthetical.

**Verify** Markdown unit test: with `hyperlinks = true` the output contains the OSC-8 wrapper and no parenthetical; with false, current behavior.

## TUI-021 — Cache-miss notices not implemented

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — a workspace grep for `cache_miss|CacheMiss|cacheMiss` in `.rs` finds only an unrelated subagents test name (`cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:1050`); `settings_rows` (`cyrup/crates/cyrup-tui/src/app.rs:3308-3412`) has no cache-miss row. The footer computes a per-turn cache-hit rate (`cyrup/crates/cyrup-tui/src/status.rs:311-318`) but nothing warns on a miss.

**upstream** — `pi/packages/coding-agent/src/core/cache-stats.ts:158` detects a miss; `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3524-3546` renders the notice, with the `onShowCacheMissNoticesChange` settings hook at `:4316-4319`.

**Impact** A prompt-cache miss — often caused by an edit to a system prompt or a tool-set change, and directly expensive — passes unremarked.

**Fix** Port `cache-stats.ts`'s miss detection into the session layer, add a `showCacheMissNotices` settings row, and push a transcript notice from the usage-update arm in `app.rs`.

**Verify** App test: two turns whose usage shows cache creation without a read emit exactly one notice; with the setting off, none.

## TUI-022 — `terminal.showTerminalProgress` is a dead setting — OSC 9;4 never emitted

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:3373-3377` exposes the toggle backed by `cyrup/crates/cyrup-config/src/settings.rs:748`; a workspace grep for `9;4|setProgress|show_terminal_progress` in `.rs` finds only those two sites — no emitter.

**upstream** — `pi/packages/tui/src/terminal.ts:12-13,509-522` — OSC `9;4` progress states driven by run start/end.

**Impact** A persisted setting does nothing; taskbar/tab progress indicators never appear during a run.

**Fix** Emit `ESC]9;4;1;0BEL` on run start and `ESC]9;4;0;0BEL` on end, gated on the setting, from the run-loop arms in `app.rs`. `draw_synchronized` (`app.rs:3065-3071`) already writes raw BSU/ESU bytes outside the ratatui buffer and `terminal_query.rs` established the direct-fd write pattern — two precedents.

**Verify** App test over a capturing writer: run start/end with the setting on produce the two sequences; with it off, none.

## TUI-023 — Retry indicator shows a frozen delay instead of counting down

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:2686-2694` `AutoRetryStart` computes `seconds = delay_ms.div_ceil(1000)` once and calls `indicator.set(IndicatorKind::Retry, Some(msg))` with nothing decrementing it; the 1-second `dialog_countdown` interval is gated exclusively on a pending UI reply with a deadline. `ace01cb`'s `SummarizationRetryScheduled` arm (`app.rs:2695-2709`) has the identical frozen shape.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/status-indicator.ts:42-65` plus `countdown-timer.ts:21-30` — the remaining seconds tick down each second.

**Impact** During a 30-second backoff the UI reads "retrying in 30s" for the whole 30 seconds, so the user cannot tell whether it is progressing or hung.

**Fix** Store the retry deadline on `AppState` and un-gate the existing 1-second `dialog_countdown` interval to also fire for an active retry, recomputing the label from `deadline - Instant::now()`. Fix both arms together.

**Verify** `crates/cyrup-tui/tests/status_indicator.rs` currently asserts only the initial string (`:99-111,178`), so nothing blocks the fix — add a tick-driven assertion that the label decreases. Prefer an injectable instant (see TUI-N09) over sleeping.

## TUI-024 — Footer context segment vanishes when usage is unknown instead of `?/{window}`

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/status.rs:301-310` `context_text` returns `None` unless *both* `context_percent` and `context_window` are `Some`, and the caller at `:281` pushes the segment only when it is `Some`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/footer.ts:108-110` falls back to `state.model?.contextWindow ?? 0` and renders the percent as `"?"` when null; `:148-160` pushes `contextPercentStr` unconditionally.

**Impact** Before the first usage report the footer's segment layout shifts as the context indicator appears, and the user gets no context-window figure at all on a fresh session.

**Fix** In `context_text`, fall back to the model's context window and render `?` for an unknown percent; make the push at `status.rs:281` unconditional.

**Verify** The only tests touching this segment (`crates/cyrup-tui/tests/render.rs:131`, `crates/cyrup-tui/tests/assembled_render.rs:166`) assert the populated form, so nothing pins the vanishing case — add a fresh-session case asserting `?/200k`-shaped output.

## TUI-025 — Slash-command metadata one baseline behind

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — all three literals unchanged: `cyrup/crates/cyrup-tui/src/commands.rs:45` `arg_cmd("model", …, "<model>")`, `:59` `cmd("login", "Configure provider authentication", None)`, `:64` `"Reload keybindings, extensions, skills, prompts, and themes"`. The `/reload` *status* is `"reloaded resources"` (`app.rs:2440`).

**upstream** — `pi/packages/coding-agent/src/core/slash-commands.ts:21` `argumentHint: "<provider/model>"`, `:35` `"<provider>"`, `:40` `"… themes, and context files"`. pi's reload status is the longer wording with a trust variant (`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:5486-5490`: `Reloaded keybindings, extensions, skills, prompts, themes, and context files[; saved project trust]`).

**Impact** Cosmetic but misleading: `/model <model>` understates the required `provider/model` form, and `/reload` does not mention context files, which it does reload.

**Fix** Update the three literals in `commands.rs` and the status string in `app.rs:2440` including the trust variant. Fold into the same S as TUI-N02, which touches the same reload path.

**Verify** Snapshot/unit assertions on the command table and the reload status.

## TUI-026 — Transcript prefixes `you:` / `assistant:` labels

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/transcript.rs:1677` inserts `Span::styled("you: ", theme.user_style())` and `:1697` `"assistant: "`, with wrap widths reduced at `:1671` and `:1690`. Inline tests at `transcript.rs:1938-1951` assert the prefixes. Documented as deliberate at `transcript.rs:1663-1670`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/user-message.ts` (background Box, no label) and `components/assistant-message.ts:104-113` (bare Markdown).

**Impact** 5 and 11 columns of every wrapped line lost to labels pi does not draw; transcripts diff differently from pi's. Note `d2c5509`'s `Entry::Thinking` renders with **no** label (`transcript.rs:1704-1710`), so the transcript is now internally inconsistent about labelling.

**Fix** **Do not change without asking** — this is a documented cyrup-original design choice. If revisited: drop the labels, restore full wrap width, and distinguish roles by background/colour as pi does. At minimum, resolve the internal inconsistency with thinking entries one way or the other.

**Verify** Whichever way it is resolved, update `transcript.rs:1938-1951` to pin the decision.

## TUI-N05 — Extension shortcuts can never override a built-in key; no conflict reported

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:1042-1072`: the global `Keymap::action_for(key)` is consulted first and, unless the action is `ClipboardPasteImage` with no image or a deferred `Interrupt`/`Quit`, returns immediately; only an unmatched key reaches `state.extension_shortcuts` at `:1069-1072`. The comment at `:1065-1068` frames this as protecting Ctrl+D/Esc, but it applies to *every* built-in binding. `cyrup/crates/cyrup-ext/src/registry.rs:388-395` `register_shortcut` is a bare `HashMap` insert — a later registration silently replaces an earlier one with no record. Workspace grep: no `restrict_override`, no `shortcut_diagnostics`.

**upstream** — `pi/packages/coding-agent/src/core/extensions/runner.ts:71-89` `RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS` lists 18 ids; `getShortcuts` (`:494-534`) skips an extension shortcut only when the colliding built-in is reserved (`restrictOverride === true`, `:511-517`), otherwise the extension *wins* and pi records `Extension shortcut conflict: '{key}' is built-in shortcut for {keybinding} and {path}. Using {path}.` (`:519-525`); a second extension on the same key also wins with its own warning (`:527-533`). All land in `shortcutDiagnostics`, surfaced under `[Extension issues]` (`interactive-mode.ts:1671-1680`).

**Impact** An extension binding a non-reserved key (any editor motion, page-up/down, history nav) silently never fires — the key does its built-in thing and the guest handler is dead, with no diagnostic to explain it. Two extensions on the same key is likewise silent last-wins.

**Fix** Port `RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS` as a const set of cyrup `Action`s; in `apply_key` check `state.extension_shortcuts` *before* the built-in keymap when the matched built-in action is not reserved, keeping the current precedence when it is. Have `register_shortcut` (`registry.rs:388-395`) record a `ShortcutDiagnostic` on replacement, add a `shortcuts` field to `StartupDiagnostics` (`cyrup/crates/cyrup-session-svc/src/services.rs:34-46`) and fold it into `[Extension issues]` — the same plumbing TUI-006 needs. Work them together.

**Verify** App test: register an extension shortcut on a non-reserved key, press it, assert `AppAction::ExtensionShortcut` (not the built-in action); register one on `Esc`, press it, assert `Action::Interrupt` still wins **and** a conflict diagnostic is recorded.

## TUI-N06 — `Entry::Thinking` freezes hide/show at commit time

**Kind** parity-bug · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/transcript.rs:467-475` `commit_thinking` stamps `hidden: self.hide_thinking` into `Entry::Thinking { text, hidden }`, and `entry_lines` (`transcript.rs:1704-1710`) reads that frozen field. `set_hide_thinking_block` (`:480-482`) affects only the in-flight block and entries committed afterwards. The constraint is structural: committed entries are drained to `Terminal::insert_before` (`app.rs:994-1010`) into the terminal's own scrollback. Self-documented at `transcript.rs:45-52` and in `d2c5509`'s message under KNOWN LIMITATION.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/assistant-message.ts:57-62` `setHideThinkingBlock` re-runs `updateDisplay()`; `interactive-mode.ts:4305-4313` `onHideThinkingBlockChange` walks every `AssistantMessageComponent`, calls `setHideThinkingBlock` on each, then `chatContainer.clear()` + `rebuildChatFromMessages()` — the toggle retroactively collapses or expands the whole visible conversation.

**Impact** Toggling "Hide thinking" mid-session changes only future turns. On the replay path the asymmetry is sharper: a `/resume` re-commits every historical turn's reasoning at the *current* setting, so the same conversation renders differently depending on when it was resumed.

**Fix** Decide, do not patch. (A) accept it and record it in `lib.rs`'s ADR-0001 notes plus the `/settings` row label; (B) keep the last N committed entries in a re-renderable tail above `insert_before`; (C) on a toggle, re-run `replay_session` so the conversation re-commits below in the new form, accepting duplicated scrollback (which the sibling `/resume` divergence already produces). Mutating already-flushed rows is not achievable under `insert_before`.

**Verify** Whatever is chosen, pin it: commit a `Thinking` entry with `hide_thinking = false`, flip `set_hide_thinking_block(true)`, assert the documented outcome so it cannot drift silently again.

## TUI-N07 — Mid-session `/resume` cannot erase the previous session's scrollback

**Kind** parity-bug · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:698-708` `rebind_session` resets `TranscriptView` and live state, but previously committed rows have already gone to native scrollback via `flush_committed` → `Terminal::insert_before` (`app.rs:994-1010`) and are unreachable; `replay_session` (`app.rs:759`) then appends the new session's conversation below the old one. Self-documented at `app.rs:749-753` and in `d2c5509`'s message.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:1737-1742` — the tree/fork navigate path does `this.chatContainer.clear(); this.renderInitialMessages();` and restores `result.editorText`; `:3599-3601` `rebuildChatFromMessages()` does the same after compaction. pi owns its whole viewport, so a clear is a real clear.

**Impact** After `/resume`, `/fork`, `/tree`, `/import` or `/clone` the terminal shows the old conversation, a `session replaced` status, then the new conversation in full. Scrolling up crosses a session boundary marked only by that one line. TUI-003's fix made this far more prominent — before, there was nothing below the boundary at all.

**Fix** Same ADR-0001 family as TUI-N06; decide rather than patch. Cheapest honest improvement: replace the plain `session replaced` status with a full-width rule plus the new session's id/name/branch so the seam reads as deliberate. A true clear needs either the alternate screen (TUI-019) or a raw `ESC[3J` outside the ratatui buffer, which destroys the user's pre-cyrup scrollback and must be opt-in at most.

**Verify** App test: commit two entries, call `rebind_session` + `replay_session` with a different message set, assert `scrollback_lines()` still contains the pre-swap text plus the chosen boundary marker — pinning the behaviour instead of leaving it in a commit message.

## TUI-N08 — `tests/image.rs` pins the invented `🖼` placeholder and the rasterize-anyway fallback

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/tests/image.rs:59-73` `show_images_off_renders_text_placeholder` asserts the buffer contains `🖼`, `red.png` and `64×48` — locking in `ImageBlock::placeholder_line`'s invented format (`cyrup/crates/cyrup-tui/src/image.rs:197-207`) as correct. `:38-57` `attached_image_renders_inline_halfblocks_when_show_images_on` asserts a half-block raster paints on a backend with no image protocol, and its `:42` precondition `assert!(!app.state().image_renderer.is_graphical(), ...)` makes the no-protocol case explicitly the tested one. Since `d2c5509` the same crate contains `image_fallback_text` (`image.rs:311-330`) producing pi's real format, so the test and the code disagree about what the placeholder should be.

**upstream** — `pi/packages/tui/src/components/image.ts:114-118`: when `!caps.images`, exactly one line, `truncateToWidth(this.theme.fallbackColor(imageFallback(...)), width)`; `pi/packages/tui/src/terminal-image.ts:546-558` defines that string as `[Image: {shortened path} [{mime}] {w}x{h}]`. pi has no half-block rasterizer anywhere.

**Impact** A green test pinning current-but-wrong behavior — the same shape that let `providers/anthropic.rs` assert the buggy xhigh→max mapping. Anyone implementing TUI-017 hits two failing assertions and must decide whether the test or the gap doc is authoritative, while the suite reports parity on a path that has none.

**Fix** Retarget both to state the divergence rather than the format: assert that *some* placeholder line is emitted, or annotate the two assertions `// TUI-017: pins the current cyrup format; flip to image_fallback_text when the attachment strip is ported` plus an `#[ignore]`d companion asserting pi's `[Image: …]` form. Delete the comment and un-ignore when TUI-017 lands.

**Verify** `grep -n '🖼' crates/cyrup-tui/tests/` returns nothing load-bearing, and `attached_image_fallback_matches_pi_format` passes once TUI-017 is fixed and fails loudly if the format regresses.

## TUI-N09 — `extension_dialog_countdown` asserts an exact countdown it cannot control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/tests/extension_dialog_countdown.rs:79-91`: opens a dialog with `timeout_ms = 3_000`, does `std::thread::sleep(Duration::from_millis(1_100))`, ticks, and asserts the rendered title contains the literal `"Proceed? (2s)"`. The displayed value is `remaining.div_ceil(1000)` off a real `Instant`, so the assertion holds only while total elapsed wall time stays under 2000 ms — a 900 ms budget for scheduler delay, in a workspace of ~3,180 tests. It is the only wall-clock-exact assertion in the file; the two expiry tests (`:106`, `:135`) sleep 120 ms against a 50 ms deadline and only require `elapsed >= deadline`, which is monotone-safe.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/countdown-timer.ts:21-30` drives the same countdown from an injected timer rather than a wall-clock sleep; cyrup has no equivalent injection point because `App` reads `Instant::now()` directly.

**Impact** The second test-defect shape this project keeps finding — the one that produced `1806375`'s fix in `caps/proc.rs`. A CI or loaded-laptop stall past 900 ms turns it red with a message pointing at the countdown logic rather than at the scheduler, which is how a suite starts being ignored.

**Fix** (a) weaken to a monotone assertion — after the sleep the title shows fewer seconds than at open and more than zero (`(2s)` or `(1s)`, never `(3s)`); or better (b) give the countdown an injectable instant (`tick_extension_dialog_countdown_at(Instant)`, mirroring `StatusIndicator::spinner_at(Duration)` in `cyrup/crates/cyrup-tui/src/status_indicator.rs`, already the deterministic pattern here) and drive it synthetically. (b) also removes 1.1 s of real sleep from the suite and unblocks TUI-023's countdown test.

**Verify** With (b): `tick_extension_dialog_countdown_at(open + 1_100ms)` renders `(2s)` and `+ 2_100ms` renders `(1s)`, with no `thread::sleep` and no dependence on wall time.

## Coverage

**Method.** Read-only and static at HEAD `1806375` (confirmed clean). No cargo, no npm. For every `closed` / `partially-closed` verdict I opened the named cyrup function at HEAD *and* the cited upstream file and compared branch-for-branch, rather than trusting commit messages. One commit message (`d2c5509`) was read deliberately to check it against the code; it overstates TUI-007.

**Read on the cyrup side.** `cyrup/crates/cyrup-tui/src/`: `app.rs`, `transcript.rs`, `startup.rs`, `image.rs`, `theme.rs`, `terminal_query.rs`, `status.rs`, `status_indicator.rs`, `keymap.rs`, `commands.rs`, `autocomplete.rs`, `chrome.rs`, `editor.rs`, `markdown.rs`, `lib.rs`, `startup_selector.rs`. Plus `cyrup/crates/cyrup/src/main.rs` (boot sequence), `cyrup/crates/cyrup-session-svc/src/{services.rs,runtime.rs}`, `cyrup/crates/cyrup-ext/src/{registry.rs,host/services.rs}`, `cyrup/crates/cyrup-config/src/{settings.rs,trust.rs}`, and all 46 files under `cyrup/crates/cyrup-tui/tests/`.

**Read on the upstream side.** `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts` and its `components/{assistant-message,user-message,tool-execution,footer,status-indicator,countdown-timer,compaction-summary-message}.ts`, `theme/{theme,theme-controller}.ts`; `pi/packages/tui/src/{tui,autocomplete,terminal,terminal-image}.ts` and `components/{image,markdown}.ts`; `pi/packages/coding-agent/src/core/{slash-commands,cache-stats}.ts` and `core/extensions/runner.ts`.

**Overturned.** TUI-006 downgraded `closed` → `partially-closed`: pi's `[Extension issues]` is a four-source union and cyrup renders one, so duplicate slash commands, built-in shadowing and duplicate shortcuts remain exactly as invisible as when the item was filed. TUI-007 was re-examined and *kept* closed, but only with the TUI-N01 cross-reference attached — its closure must not be read as tool-result-image parity.

**Test-defect sweep.** Grepped all 46 test files for `thread::sleep|Instant::now|elapsed` and for negative assertions. Exactly two timing-dependent sites: `extension_dialog_countdown.rs:85` (filed, TUI-N09) and `terminal_theme_query.rs:220-224` (a `< 1s` upper bound on a bounded probe — monotone-safe, not filed). Bug-pinning candidates examined and *not* filed, with reasons: `status_indicator.rs:99-111,178` assert only the retry band's initial string, so they do not block TUI-023; `render.rs:131` / `assembled_render.rs:166` assert only the populated context segment, so nothing pins TUI-024; `thinking.rs:147-163` uses only adjacent thinking blocks, so it does not pin TUI-002's fold; `transcript.rs:1938-1951` pins the `you:`/`assistant:` prefixes but TUI-026 is a documented cyrup-original the brief says not to change unasked.

**Taken on trust / could not verify.** (1) Runtime behavior of `terminal_query.rs` against a real TTY — the parsers are unit-tested against pi's byte forms and the DA1/timeout contract is argued in the module docs, but whether iTerm2/kitty/tmux answer within 100 ms without leaking bytes into crossterm is untestable here. (2) Whether cyrup has a compaction-queue analog to pi's `compactionQueuedMessages` (affects TUI-005 and TUI-016 scope). (3) Whether `image_width_cells` survives a settings round-trip through `/reload` — the swap arm re-reads it (`app.rs:4166`) but the settings layer was not traced. (4) `spec/architecture/*` and `R-NN-NNN` text: that tree is absent from this workspace; ids were used only as a grep index and no claim is made about their content.

**Blind spots.** `session_search.rs`, `tree_selector.rs`, `config_selector.rs`, `diff.rs`, `export.rs`, `fuzzy.rs` and the theme file-watcher / colour-precedence path were not compared against upstream, so this ledger is not exhaustive for those files. The ratatui-vs-hand-rolled substrate difference (OSC 133 prompt zones, focus reporting, non-re-renderable committed rows) is excluded per the brief except where a concrete pi behavior depends on it (TUI-N06, TUI-N07).



---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at
all, rather than checking a list of known items. That inversion exists because the
item-driven method missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`)
— a real, user-reported bug — and by construction cannot see behaviour nobody wrote an item
for. IDs use an `-SNN` suffix to mark their provenance.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| TUI-S01 | high | not-ported | M | The interactive TUI never installs a `UiEffectSink` — every fire-and-forget extension `ui.*` mutator is silently dropped |
| TUI-S02 | medium | not-ported | S | No panic hook — an abort leaves the terminal in raw mode with the cursor hidden (SIGHUP half is a duplicate) |
| TUI-S03 | medium | not-ported | M | The footer's `(git branch)` segment can never be populated — no git-HEAD resolution anywhere in cyrup |
| TUI-S04 | medium | not-ported | M | Kitty keyboard flags pushed blind — no capability negotiation and no `modifyOtherKeys` fallback |
| TUI-S05 | medium | not-ported | M | No terminal cell-size query (`CSI 16 t`) — image geometry uses ratatui-image's half-block cell |
| TUI-S06 | low | not-ported | S | The terminal window/tab title is never set (no OSC 0 write) |
| TUI-S07 | low | not-ported | S | No `checkTmuxKeyboardSetup` — tmux users get silently broken modified-Enter with no diagnosis |
| TUI-S08 | low | not-ported | S | No stdin drain before exit and no `stdin.pause()` — buffered escape bytes leak to the parent shell |
| TUI-S09 | low | not-ported | S | No resume-command hint printed on exit |
| TUI-S10 | low | not-ported | S | Shift+Ctrl+D global debug chord absent — `/debug` reachable only by typing into the editor |
| TUI-S11 | low | not-ported | M | No startup version-update or package-update notifications |

## TUI-S01 — The interactive TUI never installs a `UiEffectSink` — every fire-and-forget extension `ui.*` mutator is silently dropped

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2218-2270 `createExtensionUIContext` wires notify/setStatus/setTitle/setEditorText/pasteToEditor/setHeader/setFooter/setToolsExpanded/setWidget to live TUI actions.

**cyrup** — ABSENT. Re-ran independently. `grep -rn "UiEffect" crates/ --include=*.rs` → producers in cyrup-session-svc/src/host_services.rs:122-650 and exactly ONE consumer, cyrup-modes/src/rpc.rs:366-419 + :549. `grep -rn "set_ui_effect_sink" crates/` → definition at host_services.rs:408, call sites only rpc.rs:487/:550. cyrup-tui/src/app.rs:4127 and :4456 install `set_ui_sink` (the BLOCKING `UiRequest` dialog channel) and nothing else — no `set_ui_effect_sink` anywhere in cyrup-tui or crates/cyrup/src/main.rs. `grep -rn "set_extension_status" crates/` → definition cyrup-tui/src/status.rs:194 + only tests/footer_extensions.rs:31,32,46,48. `emit_ui_effect` (host_services.rs:414-418) drops silently when the sink is `None`, and its own doc at :407 says "Only interactive/rpc call this" — interactive does not.

**Impact** — In the shipped default mode an extension calling ui.notify / setStatus / setTitle / setEditorText / pasteEditorText / setHeader / setFooter / setToolsExpanded gets a success return and nothing happens. Extension self-reported errors (`ui.notify`) go silent in exactly the mode a human is watching, and the same call works over RPC, so it looks like a cyrup bug to the extension author. The footer extension-status row (status.rs:194/:367) is unreachable in production.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — The `setWidget` member alone is already TUI-014 ('Extension widgets stored host-side but never rendered'). The other seven variants are untracked; the correct framing is the missing sink, not seven separate items. Verified TUI-014's text scopes itself to widgets only.

## TUI-S02 — No panic hook — an abort leaves the terminal in raw mode with the cursor hidden (SIGHUP half is a duplicate)

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3750-3755 `process.prependListener("uncaughtException")` → `uncaughtCrash` (:3691-3708), whose doc names the exact failure: raw mode + hidden cursor left behind, requiring `stty sane && reset`. :3739-3748 routes stdout/stderr EIO to `emergencyTerminalExit`.

**cyrup** — ABSENT. `grep -rn "panic::set_hook" crates/` → ZERO (the only `catch_unwind` hits are cyrup-agent/src/agent.rs:167,387,1686 and cyrup-ext/src/{native.rs:480,facade.rs:323} — unrelated handler guards). `grep -rn "impl Drop" crates/cyrup-tui/src/app.rs` → zero. Cargo.toml:215 sets `panic = "abort"` for release, so no unwind and no Drop fallback exists either. `App::restore` (cyrup-tui/src/app.rs:3981-3988) is reached only from app.rs:3998, :4085, :4495 and crates/cyrup/src/main.rs:1219, :1413 — all normal paths (I checked: there is no `process::exit` in cyrup-tui/src or crates/cyrup/src that bypasses it). Note restore()'s own doc comment at app.rs:3979 claims it is 'total and idempotent so a `Drop` guard / error path always leaves a usable terminal' — the Drop guard it refers to does not exist.

**Impact** — Any panic on any thread (dependency arithmetic overflow, image-decode assert, ratatui slice, poisoned-mutex re-panic) aborts with raw mode on, bracketed paste on, kitty flags pushed and the cursor hidden; the user's shell is unusable until they blind-type `stty sane; reset`. `panic = "abort"` makes this strictly worse than pi — only a `set_hook` can save it. Downgraded from the claim's `high` because it needs a panic to occur (no normal path bypasses restore) and the workspace clippy no-panic policy lowers first-party probability; dependencies are still uncovered.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — The SIGHUP half of this claim is ALREADY TRACKED as SEAM-008 in gap-analysis/08-cyrup-session-svc-and-modes.md:217-229 ('SIGHUP ignored; no 143/129 signal exit codes'), which cites the same absence in crates/cyrup/src/signals.rs. Report only the panic-hook half as new.

## TUI-S03 — The footer's `(git branch)` segment can never be populated — no git-HEAD resolution anywhere in cyrup

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/footer-data-provider.ts:99-382 resolves the branch from .git/HEAD (worktree-aware, `detached` fallback, `git symbolic-ref` escape hatch) and watches it with a 500 ms debounce; interactive-mode.ts:516 constructs it, :866 subscribes a re-render, components/footer.ts:112-120 renders `pwd (branch)`.

**cyrup** — ABSENT. `grep -rn "set_branch" crates/ --include=*.rs` → setter at cyrup-tui/src/status.rs:156 and ONLY two test call sites (cyrup-tui/tests/assembled_render.rs:143, tests/render.rs:111). The one production footer populator, crates/cyrup/src/main.rs:1455-1481, sets model/provider/reasoning/provider_count/cwd/thinking_level — and its comment at :1475 literally says "Location line (`cwd (branch) • name`, footer.ts:116-130)" then calls only `set_cwd`. `grep -rni "symbolic-ref|refs/heads|git_branch|current_branch|head_ref|\.git/HEAD" crates/ --include=*.rs` → zero production hits (only cyrup-tui/tests/autocomplete.rs:276 writing a fixture). `gix::` is used only in cyrup-resources/src/package/install.rs (clone/checkout), never for HEAD resolution of the cwd.

**Impact** — The footer renders `~/path • session` where pi renders `~/path (david/cyrup) • session`, permanently, and a mid-session `git checkout` is never reflected. Invisible to the test suite because both rendering tests call `set_branch` themselves.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — Not tracked. Grepped all 12 gap files for 'branch' — hits are TUI-005/TUI-009 (escape branches), TUI-N07 (branch summaries) and SESS-023, none about the footer git branch.

## TUI-S04 — Kitty keyboard flags pushed blind — no capability negotiation and no `modifyOtherKeys` fallback

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/tui/src/terminal.ts:17 `\x1b[>7u\x1b[?u\x1b[c`; :228-250 `handleKeyboardProtocolNegotiationSequence` enables kitty only on a non-zero flags reply and calls `enableModifyOtherKeys()` (`\x1b[>4;2m`, :320-324) when flags are 0 or DA1 arrives first; :326-330 `disableModifyOtherKeys` on drain and stop. Re-read all three hunks — accurate as claimed.

**cyrup** — ABSENT. `grep -rn "modify_other_keys|modifyOtherKeys|4;2m|>4;" crates/` → ZERO. `grep -rn "supports_keyboard_enhancement" crates/` → ZERO. cyrup-tui/src/app.rs:3959-3966 unconditionally `PushKeyboardEnhancementFlags(DISAMBIGUATE_ESCAPE_CODES)` with the comment 'ignore failure (legacy terminals)' — a blind `CSI > 1 u` write with no reply read; repeated verbatim at :4010-4013 (resume) and :4093-4096 (external editor). cyrup-tui/src/terminal_query.rs handles only OSC 11, DSR ?996 and the DA1 sentinel — no `CSI ? u` flags query, no parser for a flags reply.

**Impact** — On any terminal without the kitty protocol (xterm, Terminal.app, older VTE, tmux without extended-keys) pi still disambiguates modified keys via modifyOtherKeys and cyrup gets nothing: Shift/Ctrl/Alt+Enter arrive indistinguishable from Enter, so newline-vs-submit and follow-up bindings silently do the wrong thing. Not substrate — crossterm exposes the push primitive and explicitly leaves negotiation to the caller.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — Not tracked. TUI-008 covers unbound keybinding *ids*, a different layer; the only other 'Kitty' mention in the gap files is TUI-019's alt-screen item noting the flag is pushed.

## TUI-S05 — No terminal cell-size query (`CSI 16 t`) — image geometry uses ratatui-image's half-block cell

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/tui/src/tui.ts:720-728 `queryCellSize()` writes `\x1b[16t` at start (gated on capabilities.images); :898-918 `consumeCellSizeResponse` parses `ESC[6;h;w t`, calls `setCellDimensions` then `invalidate()`+`requestRender()`. Default it replaces is {9,18} (terminal-image.ts:37). Both hunks read and verified.

**cyrup** — ABSENT. `grep -rn "16t|cell_dimensions|CellDimensions|set_cell_dimensions" crates/` → ZERO. cyrup-tui/src/image.rs:66-73 `from_capabilities` builds `Picker::halfblocks()` and only `set_protocol_type`s it for Kitty/Iterm2, so `self.picker.font_size()` at image.rs:91 (and :233) is always the half-block cell, never terminal-reported; image.rs:52-59 documents deliberately replacing `Picker::from_query_stdio` with an env sniff because the query blocked on stdin.

**Impact** — `cell_size` (image.rs:90-105) mis-sizes on graphical terminals. Nuance I verified that the claim did not: half-blocks are 1:2 (w:h) and a real cell is typically ~9:18 = 1:2, so a WIDTH-CLAMPED large image comes out nearly right by coincidence. The damage is on images narrower than the terminal — a 40x40px icon reserves 40 cols x 20 rows under half-blocks vs 5x3 under a real 9x18 cell, ~8x oversized, on exactly the kitty/iTerm2 terminals where real pixels are drawn. Severity medium stands but for the small-image case, not uniformly.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — Not tracked. TUI-017 and TUI-N01 cover protocol selection, the invented placeholder and the missing 60-cell cap — neither touches the pixels-per-cell input to the calculation.

## TUI-S06 — The terminal window/tab title is never set (no OSC 0 write)

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — interactive-mode.ts:877-885 `updateTerminalTitle()` → `terminal.setTitle` (pi/packages/tui/src/terminal.ts:504-507, `\x1b]0;<title>\x07`), re-applied after the Windows npm check clobbers it (:915-921).

**cyrup** — ABSENT. `grep -rn "SetTitle|]0;|]2;|set_terminal_title|window_title" crates/ --include=*.rs` → the only OSC-0 literals are ANSI *strippers* in cyrup-tui/src/bash.rs:272 and cyrup-session-svc/src/bash.rs:486 (test strings for stripping child output). host_services.rs:642 emits `UiEffect::SetTitle`, consumed only by rpc.rs:402 — i.e. dead in the TUI per finding 1. crossterm's `SetTitle` is never imported. The `set_title` methods in selector.rs/text_input.rs/extension_editor.rs are dialog widget titles.

**Impact** — Multiple cyrup sessions in tabs/panes are indistinguishable. Same primitive `ui.setTitle` needs, so both close together. Correctly classified as behaviour, not ADR-0001 substrate: crossterm exposes `SetTitle` and declines to decide when to use it.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — Not tracked in any gap file (grepped 'title'/'setTitle' across gap-analysis/ — only 08's SEAM item about the RPC widget wire shape mentions setTitle, and that says rpc.rs:401-407 already matches).

## TUI-S07 — No `checkTmuxKeyboardSetup` — tmux users get silently broken modified-Enter with no diagnosis

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — interactive-mode.ts:999-1044 (read in full — matches the claim): on $TMUX, spawns `tmux show -gv extended-keys` / `extended-keys-format` with a 2 s timeout and returns a warning naming the exact `set -g extended-keys on` remedy; wired at :928-932 through `showWarning`.

**cyrup** — ABSENT. `grep -rn "extended-keys|extended_keys" crates/` → ZERO. `grep -rn "TMUX" crates/ --include=*.rs` → only cyrup-tui/src/image.rs:407,450 (image-protocol suppression) and tests/image_capabilities.rs. No startup warning of this shape in cyrup-tui/src/startup.rs.

**Impact** — tmux defaults extended-keys off. A cyrup user in tmux finds Shift/Alt/Ctrl+Enter simply don't work, with no hint their multiplexer is the cause. Compounds with the missing modifyOtherKeys fallback above — tmux-without-extended-keys is exactly the case that fallback partially rescues.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — Not tracked (grepped 'tmux' across gap-analysis/ — only TUI-020's hyperlink-probe mention).

## TUI-S08 — No stdin drain before exit and no `stdin.pause()` — buffered escape bytes leak to the parent shell

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/tui/src/terminal.ts:368-404 `drainInput(maxMs=1000, idleMs=50)` — disables kitty (`\x1b[<u`) FIRST so no new sequences are generated, disables modifyOtherKeys, detaches the handler, idles to quiescence; interface doc :59-64 names the purpose (kitty key-releases leaking to the parent shell over slow SSH). :443-451 `process.stdin.pause()` before restoring raw mode, commented as fixing a Ctrl+D-closes-the-parent-shell race.

**cyrup** — ABSENT. `grep -rn "drain_input|drain_stdin" crates/` → ZERO. `App::restore` (cyrup-tui/src/app.rs:3981-3988) is exactly four statements — PopKeyboardEnhancementFlags, DisableBracketedPaste, disable_raw_mode, show_cursor — no drain, no pause; the crossterm reader thread is stopped only by its cancel token, unsynchronized with restore().

**Impact** — On exit over a high-latency link, in-flight bytes (query replies, a trailing Ctrl+D) surface on the parent shell prompt or close an SSH session. Lower exposure than pi's because cyrup requests only DISAMBIGUATE_ESCAPE_CODES (no REPORT_EVENT_TYPES → no key-release traffic); the query-reply and Ctrl+D races remain.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — Not tracked.

## TUI-S09 — No resume-command hint printed on exit

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — interactive-mode.ts:3660-3666 (read verbatim): after `this.stop()` and `runtimeHost.dispose()`, writes `To resume this session: <cmd>` to stdout; `formatResumeCommand` (:238-251) gates on isTTY + session persisted + file exists and shell-quotes.

**cyrup** — ABSENT. `grep -rni "to resume|resume this session|formatResumeCommand" crates/` → zero matches of the behaviour. The interactive exit path (crates/cyrup/src/main.rs:1403-1414: `app.run(...).await` then `let _ = app.restore();`) writes nothing to stdout after restore; the file's only println!/eprintln! sites (main.rs:45,456,809,902,1036,1040,1088,1106,1183,1188,1334,1434,1518,1547) are all other paths.

**Impact** — A user who quits loses the handle to the session; recovery means `/resume` into a picker instead of pasting one line. All the pieces (--session, --session-dir, persistence state) already exist.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — Not tracked. Straddles the TUI/bin lifecycle boundary, which is why no area file owns it.

## TUI-S10 — Shift+Ctrl+D global debug chord absent — `/debug` reachable only by typing into the editor

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/tui/src/tui.ts:818-822 (read verbatim): inside `handleTerminalInput`, BEFORE dispatch to the focused component, `if (matchesKey(data,"shift+ctrl+d") && this.onDebug) { this.onDebug(); return; }`; interactive-mode.ts:2672 wires it 'works regardless of focus'.

**cyrup** — ABSENT. `grep -rn "debug" crates/cyrup-tui/src/keymap.rs` → ZERO. The keymap's global table (keymap.rs:368-390) has no shift+ctrl chord other than ctrl+shift+p (ModelCycleBackward). `debug` appears only as commands.rs:70 HIDDEN_COMMANDS and its handler app.rs:1252 — i.e. reachable solely by typing `/debug`+Enter in the prompt editor.

**Impact** — When a selector, dialog or overlay has focus — precisely when a diagnostic dump is wanted — there is no route to it, because the only route is the editor that no longer has focus.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — TUI-008 audits configurable keybinding *ids*; `onDebug` is a hardcoded pre-dispatch chord and structurally outside it. Not tracked.

## TUI-S11 — No startup version-update or package-update notifications

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**upstream** — interactive-mode.ts:901-921 (read in full): `checkForNewPiVersion(this.version)` → `showNewVersionNotification`, and `checkForPackageUpdates()` → `showPackageUpdateNotification` (a bordered panel naming each stale package + the `pi update --extensions` remedy), both after the PI_OFFLINE gate at :894. `package-manager.ts:1159-1221 checkForAvailableUpdates` handles BOTH npm and git sources (git branch at :1204-1217, `gitHasAvailableUpdate`), skipping local and pinned.

**cyrup** — ABSENT. `grep -rni "available_update|check_for_new|new_version|update_available|checkForAvailableUpdates|outdated|latest_version" crates/` → ZERO. cyrup-resources/src/package/ (git_url/install/lock/manifest/source/store) has install + a packages.json registry but no update-availability check of any kind. The offline gate IS ported (cyrup-config/src/policy.rs:54-56, crates/cyrup/src/provider.rs:98,155 citing the exact `run()` lines) — the guard was ported and the two promises it guards were not.

**Impact** — A user with stale extension packages is never told; a package behind its source fails in ways that look like cyrup bugs. cyrup's package channel is git-first (PackageSource::Git, source.rs:13-20), so pi's git half of checkForAvailableUpdates maps directly — the npm half does not apply. Caveat on the other half: `checkForNewPiVersion` polls pi's own release feed, and a rebranded fork doing that is the same product decision the sweep author already excluded for `reportInstallTelemetry`. Recommend scoping any item to the extension-package check.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

**Overlap** — Not tracked.

