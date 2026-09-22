---
stage: augment
status: ready
updated: 2026-09-22 00:00
---

# UW-7 — the fleet-status widget receives no keystrokes

> Branch `claude/subagents-fleet-input`, HEAD `14e6c56`.
> Upstream pinned: `git -C tmp/pi-subagents show v0.68.0:<path>`. The existing port targets
> **v0.43.0** (`fleet_status.rs:1-2` says so), so BOTH tags are cited below where they differ.

## Verdict in one line

**This is a WIRING job, not a seam job.** Both ends exist and are tested. Nothing connects them.
The missing wires are **two**, and §6a accounts for every line of plan beyond them.

---

## 1. Citation audit — what the PARITY-GAPS row gets wrong

The row is at `docs/gap-analysis/PARITY-GAPS.md:1982-1989`.

### 1a. The central claim is FALSE

> *"The host seam is still absent — `cyrup-ext/src/host/services.rs` has nothing resembling
> `on_terminal_input`"*

`host/services.rs` is the **`HostServices`** trait (the host's effects: `notify`, `set_widget`,
`open_overlay`, `editor_text`). The terminal-input seam was never going to live there — it is an
*inbound* callback on the **extension**, not an effect on the host. It exists, end to end, and is
tested:

| layer | symbol | file:line |
|---|---|---|
| native declare | `InitApi::subscribe_terminal_input` | `crates/cyrup-ext/src/native.rs:425` |
| native flag | `InitApi { terminal_input: bool }` | `crates/cyrup-ext/src/native.rs:337`, consumed `:489` |
| native handler | `NativeExtension::on_terminal_input(&self, data: &str) -> Option<TerminalInputResult>` | `crates/cyrup-ext/src/native.rs:757` |
| registry list | `terminal_input_subscribers: Vec<ExtensionId>` | `crates/cyrup-ext/src/registry.rs:319` |
| registry sub/unsub/read | `subscribe_terminal_input` / `unsubscribe_terminal_input` / `terminal_input_subscribers` | `registry.rs:595`, `:605`, `:613` |
| facade declare→subscribe | `if terminal_input { self.registry.subscribe_terminal_input(id)? }` | `crates/cyrup-ext/src/facade.rs:527-528` |
| facade fold (the public entry) | `pub async fn terminal_input(&self, data: &str) -> TerminalInputDecision` | `crates/cyrup-ext/src/facade.rs:1472` |
| per-tier dispatch | `terminal_input_via` → native `:1507`, wasm `:1522` | `facade.rs:1500-1532` |
| WASM host | `LiveExtension::on_terminal_input` | `crates/cyrup-ext/src/host/live.rs:1980` |
| WASM sub/unsub imports | `subscribe_terminal_input` / `unsubscribe_terminal_input` | `host/live.rs:323`, `:329` |
| WIT | `on-terminal-input: func(data: string) -> option<terminal-input-result>` | `crates/cyrup-ext/wit/world.wit:388`; record `:150`; imports `:680-681` |
| SDK guest macro | `fn on_terminal_input(data) -> Option<TerminalInputResult>` | `crates/cyrup-ext-sdk/src/macros.rs:118-128` |
| SDK guest ctx | `UiCtx::subscribe_terminal_input` / `unsubscribe_terminal_input` | `crates/cyrup-ext-sdk/src/ctx/ui.rs:157`, `:145` |
| contract types | `TerminalInputResult { consume: Option<bool>, data: Option<String> }` / `TerminalInputDecision::{Deliver(String), Consume}` | `crates/cyrup-ext/src/contract.rs:269`, `:278` |
| tests | fold order, rewrite, consume, empty-fold-drops, panic containment | `crates/cyrup-ext/src/tests/native_dispatch.rs:1568-1760` |

`git grep -c 'on_terminal_input\|terminal_input' -- crates/cyrup-ext crates/cyrup-ext-sdk` is not
zero; it is ~50 hits across 9 files. **The row must be corrected.**

#### The claim was false WHEN IT WAS WRITTEN, not merely stale

This is the strongest single piece of evidence that UW-7 is a wiring gap, so it is dated carefully.

* **The documentary record.** Area 06's `EXT-021` row
  (`docs/gap-analysis/06-cyrup-ext.md:399`) records, in its own words:
  *"**RESIDUAL, NARROWED 2026-08-14 (sweep 6):** `onTerminalInput` (types.ts:145) is **CLOSED** —
  the `on-terminal-input` export plus `ui.subscribe-terminal-input`/`unsubscribe-terminal-input`
  imports landed with the 0.6 → 0.7 bump."* The row itself closed **2026-08-15**
  (`06-cyrup-ext.md:347` counts it among that pass's sixteen closures).
* **The git record cannot date it more precisely, and here is why.** `git log -S` for
  `on_terminal_input` (native.rs), `pub async fn terminal_input` (facade.rs),
  `terminal_input_subscribers` (registry.rs) and `on-terminal-input` (world.wit) all return the
  **same** commit, `9e2bfda` (2026-09-04). That commit is `2209 files changed, 995598 insertions(+)`
  — a history collapse, not an authoring commit. So 2026-09-04 is a **floor**, not the landing date.
* **Either clock settles it.** UW-7's row is stamped *"STILL OPEN at `cc7818b`; citations refreshed
  2026-09-16"* — **twelve days after** the git floor and **a month after** area 06 recorded the
  closure. The row was re-greped while the seam was in the tree, and reported it absent.

The likely mechanism is the one the row's own wording exposes: it greped **`host/services.rs`**, the
`HostServices` trait, which is the wrong file by design. The dispatcher's name is
**`ExtensionHost::terminal_input`** (`facade.rs:1472`) — that is what to grep for, and what
`cyrup-tui` must call.

### 1b. Every line number in the row has drifted

| row says | actual at `14e6c56` |
|---|---|
| `handle_key` at `tui/fleet_status.rs:740` | **`:917`** (doc block opens `:912`) |
| `press` at `:1169` | **`:1452`** |
| `is_widget_registered` at `:711` | **`:864`** |
| upstream `fleet-status.ts:282-283`, handler `:352` | correct **at v0.43.0** (verified). At the pinned **v0.68.0** they are **`:577-578`** and **`:696`**; the file grew 564 → 1109 lines |
| "The PUBLISH half is live (`extension.rs:9489`, `:9889`, `:9978`)" | `extension.rs` no longer exists as a single file; the publish half is `extension/host/slash.rs:298` (`refresh_fleet_status_widget`) → `services.set_widget(...)` at `slash.rs:366`/`:372` |

### 1c. What the row gets RIGHT

* `handle_key` has **zero production callers** — every caller is a test in the same file
  (`fleet_status.rs:1839`, `:1854`, `:1871`, `:1873`, …). Confirmed.
* `press` and `is_widget_registered` are likewise test-only. Confirmed.
* **`crates/cyrup-tui/` has zero references to the seam.** The only `terminal_input` hit in that
  crate is `native_modifiers.rs:127`, a doc-comment naming the *Windows console* flag
  `ENABLE_VIRTUAL_TERMINAL_INPUT` — unrelated. Confirmed.
* The observable is exactly as stated.

---

## 2. Upstream, quoted at the pinned tag

### 2a. Subscription — `fleet-status.ts:575-578` @v0.68.0

```ts
this.ui = ui;
if (typeof ui.onTerminalInput === "function") {
    this.inputUnsubscribe = ui.onTerminalInput((data) => this.handleKey(data));
}
```

Identical at v0.43.0 `:282-283`.

### 2b. The handler — `fleet-status.ts:696-752` @v0.68.0

```ts
handleKey(data: string): { consume?: boolean; data?: string } | undefined {
    if (this.state.widgetsSuspended) return undefined;
    const ctx = this.getActiveUiContext();
    if (!ctx || this.entries.length === 0 || isKeyRelease(data)) return undefined;
    if (this.inspectorOpen) return undefined;
    if (!this.editorHasFocus()) {
        if (this.active) this.deactivate();
        return undefined;
    }

    if (!this.active) {
        const activates = matchesKey(data, "down") || matchesKey(data, "left");
        if (!activates || ctx.ui.getEditorText() !== "") return undefined;
        this.active = true;
        this.selectedKey = "main";
        this.refresh();
        return { consume: true };
    }
    // … down/j, up/k (up at index 0 deactivates), escape, Key.enter …
    this.deactivate();
    return undefined;
}
```

**Answers to the four questions asked of upstream:**

1. **What does `handleKey` receive?** A **raw terminal chunk** — `data: string`, the bytes the TUI
   read off the tty. It is *not* pre-parsed. Matching is done by pi-tui's `matchesKey(data, "down")`
   and `Key.enter`, which compare the raw sequence against a key-id.
2. **What does it return, and what does the caller do with it?** `{consume?: boolean; data?: string}
   | undefined`. The caller is pi's `TUI.handleInput` fold (`packages/tui/src/tui.ts:773-788`),
   quoted verbatim in `facade.rs:1460-1470`: each listener sees the *current, possibly already
   rewritten* data; a truthy `consume` **stops the fold and drops the keystroke**; a returned `data`
   **replaces the buffer** for later listeners; a fold ending in an empty string also drops the key.
   `undefined` = "I looked and did nothing", and the keystroke falls through to the focused
   component. So: it **can** consume, and by default it does **not**.
3. **How does it decide it is focused enough to claim ↓/←?** Two independent gates, both required:
   * `editorHasFocus()` (`:965-976`) — structurally duck-types `tui.focusedComponent` for the five
     `EditorComponent` methods, with the in-source excuse *"pi-tui exposes focus mutation but no
     focus getter"* and *"instanceof is unreliable across jiti module boundaries"*.
   * `ctx.ui.getEditorText() !== ""` — **only while inactive**. Once `active`, the editor text is
     never re-read; the roster owns every key until Esc / Enter / up-past-top / any unmatched key.
4. **So normal typing still reaches the editor** because ↓/← on a *non-empty* editor returns
   `undefined`, and every printable character while inactive falls into the same `undefined` path.

### 2c. `Enter` is deliberately OFF the input path — `:735-750`

```ts
this.inspectorOpen = true;
this.refresh();
const selectedKey = this.selectedKey;
void Promise.resolve()
    .then(() => this.openInspector(selectedKey))
    .catch((error) => ctx.ui.notify(…, "error"))
    .finally(() => { this.inspectorOpen = false; this.refresh(); });
return { consume: true };
```

The inspector is opened on a **detached microtask** and the keystroke is consumed *immediately*.
This is load-bearing for cyrup (§5c).

---

## 3. cyrup's `handle_key` contract — the full vocabulary

`crates/cyrup-ext-subagents/src/tui/fleet_status.rs:917`:

```rust
pub fn handle_key(
    &mut self,
    key: &FleetStatusKey,
    editor_has_focus: bool,
    editor_text: &str,
) -> FleetStatusKeyOutcome
```

`FleetStatusKeyOutcome` — `fleet_status.rs:736`:

| variant | pi equivalent | what the owner MUST do |
|---|---|---|
| `Consume` | `{ consume: true }` | **Do not forward the key to the editor.** Republish the widget if `render_key` moved. |
| `Pass` | `undefined` | Forward the key onward unchanged. |
| `OpenInspector` | `{ consume: true }` from the `Key.enter` arm | Consume the key **and** open the fleet inspector on the currently selected roster key. `handle_key` has *already* set `inspector_open = true` (`:983`); the owner must call `set_inspector_open(false)` (`:834`) when the inspector closes — that is pi's `finally`. |

Supporting API, all public, all currently test-only or owner-only:

* `FleetStatusKey { code: FleetStatusKeyCode, is_release: bool }` — `:1444`; `press(code)` — `:1452`.
* `FleetStatusKeyCode::{Up, Down, Left, Escape, Enter, Char(char), Other}` — `:1421`.
* `set_ui_available(bool)` — `:817` (pi `getActiveUiContext`). Already driven from
  `slash.rs:346`.
* `set_inspector_open(bool)` — `:834`.
* `render_key(now)` — `:1265`; `refresh(state, now) -> bool` — `:875` (returns "changed").
* `widget_payload` `:1339` / `widget_lines` (used at `slash.rs:357`).

**Gap in the vocabulary:** `FleetStatusKey` has **no constructor from a `&str`**. The seam delivers
`data: &str`. `FleetStatusKeyCode` must gain a parser (§5d).

**A second gap:** `handle_key` does **not** call `refresh()` where pi does (`:711`, `:720`, `:726`).
The Rust port made refresh the owner's job (module doc `:36-37`). So a `Consume` that moved the
selection leaves the published widget stale until the owner republishes. The owner must republish on
every non-`Pass` outcome.

---

## 4. What cyrup already has (greped, not assumed)

| need | cyrup has it? | evidence |
|---|---|---|
| host seam, all four tiers | **yes** | table in §1a |
| the widget state machine | **yes, complete** | `fleet_status.rs:917-990` — every pi arm, including up-at-top-deactivates and the catch-all |
| an owner holding the widget | **yes** | `extension/host/mod.rs:109` `fleet_status: Arc<Mutex<SubagentFleetStatus>>`, built `:231` |
| a production republish path | **yes** | `extension/host/slash.rs:298` `refresh_fleet_status_widget` → `set_widget` `:366`/`:372` |
| `getEditorText` equivalent | **yes, live-wired** | `HostServices::editor_text` `crates/cyrup-ext/src/host/services.rs:336`; backed by `EditorTextMirror` (`crates/cyrup-session-svc/src/host_services.rs:415`), attached `:891`, published every frame by `App::publish_extension_readbacks` (`crates/cyrup-tui/src/app/extension_ui.rs:221-224`), installed `:190` |
| `editorHasFocus` equivalent **on the seam** | **NO** | `git grep -n focus -- crates/cyrup-ext/src/host/services.rs` → one hit, a doc line about overlays (`:322`). WIT: `git grep -n focus crates/cyrup-ext/wit/world.wit` → one hit, `:750`, a comment. **Not a blocker** — the state exists in the TUI (`Editor::is_focused`, `crates/cyrup-tui/src/editor/config.rs:247`) and needs a readback (§5b) |
| correct behaviour with **no** subscribers | **yes, already** | `facade.rs:1476-1479` early-returns `Deliver(data)` when the subscriber list is empty, and it is pinned first-class by `ext021_no_subscriber_delivers_the_input_untouched` (`native_dispatch.rs:1630-1641`). **Corrected from an earlier draft of this plan:** a `has_terminal_input_subscribers()` fast path is an *optimization* (it saves an `.await` + one registry read-lock per keystroke), **not** a missing wire. It is OPTIONAL and is sequenced last |
| a `KeyEvent` → terminal-bytes encoder | **NO** | crossterm parses at the reader (`crates/cyrup-tui/src/app/input_reader.rs:373` `event::poll`), so raw bytes are gone by the time the app sees an `InputEvent::Key`. No encoder anywhere: `git grep -l 'x1b\[A' -- crates/` hits only `cyrup-ext/src/tests/native_dispatch.rs` and three `cyrup-intercom/src/ui/*` **decoders** |
| a key **spec** parser and printer | **yes** | `Key::parse` `crates/cyrup-tui/src/keymap.rs:622` and `Key::label` `:721`, vocabulary `"up" "down" "left" "right" "escape" "enter" "space" …` — pi's own KeyId vocabulary |
| the extension-shortcut precedent (host callback → subagents behaviour) | **yes** | `crates/cyrup-ext-subagents/src/extension/host/shortcuts.rs` (whole file), registered `extension/host/native_impl.rs:275`, dispatched `:1010` → `dispatch_shortcut` (`shortcuts.rs:~205`) |

### 4a. The established calling convention, read off the dispatcher's own tests

`crates/cyrup-ext/src/tests/native_dispatch.rs:1555-1760` is the reference. It settles four things
this plan would otherwise have had to assume:

1. **The native handler is `fn`, not `async fn`.** The test extension's impl
   (`native_dispatch.rs:1576`) is `fn on_terminal_input(&self, data: &str) -> Option<TerminalInputResult>`,
   matching the trait default at `native.rs:757`. So the subagents handler needs **no runtime, no
   await, no `block_on`** — lock, `handle_key`, `set_widget`, return.
2. **The dispatcher is awaited.** Every call site is `host.terminal_input(…).await` inside a
   `#[tokio::test]` (`:1619`, `:1639`, `:1670`, `:1676`, `:1722`, `:1758`). This **confirms rather
   than resolves** the async-at-a-sync-site problem: `App::handle_input` is sync, so §5a's call-site
   relocation is still required. The tests show what the signature demands, not where cyrup can
   satisfy it.
3. **Consume vs decline is signalled by the return value, not by an error.** `Deliver(String)`
   carries the possibly-rewritten chunk; `Consume` means drop it. `None` from a handler is "looked
   and did nothing" and the chunk passes on (`:1584-1590` returns `consume: None` with a rewrite).
   A panicking handler is contained and treated as `None` (`:1697-1725`) — so the TUI never has to
   handle an error from this call.
4. **The wire format is raw terminal bytes.** `host.terminal_input("\x1b[A")` at `:1639` and
   `"k"` / `"q"` / `"!"` elsewhere. The convention §5d proposes is the one already in the tree.

**What these tests do NOT cover, and what phase 1 therefore adds:** there is no caller outside this
file (`git grep -n '\.terminal_input(' -- crates/` returns exactly these six lines), so nothing
pins the *host* side of the contract — who calls it, when, and what happens to the `Deliver` string
afterwards. That is precisely the gap.

---

## 5. The Rust shape of the change

### 5a. Where the seam goes in `cyrup-tui`

`App::handle_input` (`crates/cyrup-tui/src/app/input.rs:6`) is **synchronous** and returns
`AppAction`. `ExtensionHost::terminal_input` (`facade.rs:1472`) is **`async`**. The decision
(consume vs deliver) is needed *before* the editor sees the key, so the shortcut trick of
"return an `AppAction` and spawn it" (`run_action.rs:198-222`) **cannot be used** — that pattern
works only because a shortcut's effect is fire-and-forget.

The await must therefore happen at the one caller of `handle_input`, which is already `async`:

`crates/cyrup-tui/src/app/run_action.rs:278`, inside `App::on_input_event`:

```rust
while let Some(ev) = pending.pop_front() {
    let action = self.handle_input(&ev);           //  <-- line 278 today
    match self.dispatch_run_action(ctx, action).await? { … }
}
```

becomes:

```rust
while let Some(ev) = pending.pop_front() {
    // UW-7 — pi's `TUI.handleInput` listener fold (`packages/tui/src/tui.ts:773-788`) runs
    // BEFORE the focused component is offered the key. Gated on a subscriber count so the
    // ordinary keystroke path costs one Vec read (facade.rs:1476-1479 says the same).
    let ev = match self.offer_to_extensions(ctx, &ev).await {
        TerminalInputDecision::Consume => continue,
        TerminalInputDecision::Deliver(rewritten) => rewritten,
    };
    let action = self.handle_input(&ev);
    …
}
```

with

```rust
/// pi `TUI.handleInput`'s listener fold, at cyrup's one async input site.
async fn offer_to_extensions(&mut self, ctx: &RunCtx, ev: &InputEvent)
    -> TerminalInputOutcome;   // Consume | Deliver(InputEvent)
```

**Position, and why:** this is *ahead of* the overlay / selector / loader guards that open
`handle_input` (`input.rs:29`, `:75`, `:88`) — which is exactly pi's position, and is what lets the
fleet widget deactivate itself when focus moves to a selector (§2b gate 1). It is therefore
**required** that the extension can see focus; see 5b.

*Rejected alternative, recorded:* put the call at `input.rs:136` (just above the extension-shortcut
tier), where focus is implicit because the overlay/selector/loader guards have already returned.
That is simpler and needs no focus readback — but the widget then never learns that focus moved
away, so it stays `active` behind a selector and the first key after the selector closes is eaten.
It is a viable **phase-1** if 5b is sequenced separately; it is not the end state.

### 5b. The missing readback: `editor_has_focus`

Symmetric with the existing `editor_text` (SEAM-T02) in every respect.

1. `crates/cyrup-ext/src/host/services.rs`, next to `:336`:
   ```rust
   /// Whether the editor currently holds keyboard focus — pi's `editorHasFocus()`
   /// (`pi-subagents/src/tui/fleet-status.ts:965-976` @v0.68.0), which duck-types
   /// `tui.focusedComponent` because pi-tui has no focus getter. cyrup HAS one
   /// (`cyrup-tui/src/editor/config.rs:247`), so this is a plain read rather than a
   /// structural sniff. `false` for every non-interactive mode, matching pi's
   /// `noOpUIContext`.
   fn editor_has_focus(&self) -> bool { false }
   ```
2. `crates/cyrup-session-svc/src/host_services.rs` — widen `EditorTextMirror` (`:415`) to carry a
   `bool` beside the `String`, or add an `EditorFocusMirror` beside it; override
   `editor_has_focus` on `LiveHostServices` reading it (the `editor_text` override is at
   `host_services.rs`'s `impl HostServices`, attached `:891`).
3. `crates/cyrup-tui/src/app/extension_ui.rs:221` — one line in `publish_extension_readbacks`:
   `self.state.editor_mirror.publish_focus(self.state.editor.is_focused());`
4. WIT + `host/live.rs` forward + SDK `ctx/ui.rs` getter, mirroring `get-editor-text`
   (`host/live.rs:295-296` names the family).

### 5c. The subagents side

New module `crates/cyrup-ext-subagents/src/extension/host/terminal_input.rs`, built to the shape of
`extension/host/shortcuts.rs` (module doc → registration table → predicate → handler → dispatch fn
kept beside the registration "so the two cannot drift", `shortcuts.rs:199-201`).

1. **Declare**, in `extension/host/native_impl.rs::init` beside `api.register_shortcut(...)` at
   `:275`:
   ```rust
   // UW-7 — pi `ctx.ui.onTerminalInput((data) => this.handleKey(data))`
   // (`tui/fleet-status.ts:577-578` @v0.68.0). Gated on `fleet_view_enabled` for the same
   // reason `refresh_fleet_status_widget` is (`slash.rs:306-309`): with the fleet view off
   // there is no `SubagentFleetStatus` upstream at all.
   if self.fleet_view_enabled {
       api.subscribe_terminal_input();
   }
   ```
2. **Handle**, as a `NativeExtension` method beside `execute_shortcut` (`native_impl.rs:1010`):
   ```rust
   fn on_terminal_input(&self, data: &str) -> Option<cyrup_ext::TerminalInputResult> {
       self.dispatch_terminal_input(data)
   }
   ```
   **Note it is SYNC** (`native.rs:757`) — only the WASM tier is async. That is what makes this
   tractable: the whole body is lock → `handle_key` → maybe `set_widget`, no `.await`.
3. **`dispatch_terminal_input(&self, data: &str) -> Option<TerminalInputResult>`** in the new
   module:
   ```rust
   let Some(services) = self.executor.host_services() else { return None };
   let key = FleetStatusKey::from_terminal_data(data)?;      // §5d
   let (outcome, republish) = {
       let mut w = self.fleet_status.lock().ok()?;
       let before = w.render_key(now);
       let outcome = w.handle_key(&key, services.editor_has_focus(), &services.editor_text());
       (outcome, (w.render_key(now) != before).then(|| (w.widget_lines(100, now), w.placement())))
   };                                                        // lock dropped before any host call
   if let Some((lines, placement)) = republish {
       services.set_widget(FLEET_STATUS_WIDGET_KEY, lines.as_deref(), placement.into());
   }
   match outcome {
       FleetStatusKeyOutcome::Pass        => None,
       FleetStatusKeyOutcome::Consume     => Some(TerminalInputResult { consume: Some(true), data: None }),
       FleetStatusKeyOutcome::OpenInspector => { self.spawn_fleet_inspector(); Some(consume_true) }
   }
   ```
   `set_widget` is fire-and-forget (`services.rs`'s own wording, quoted at `fleet_status.rs:31`), so
   the republish is safe from a sync handler. `widget_lines`/`placement` are the same pair
   `slash.rs:357` already publishes.

4. **`spawn_fleet_inspector`** — this is pi's `void Promise.resolve().then(...)` (§2c) and it is
   **mandatory, not stylistic**. `show_fleet` reaches `HostServices::open_overlay`, which **blocks**
   the calling task until the user closes the modal (`crates/cyrup-session-svc/src/host_services.rs:1252`,
   `:1262`; the contract is stated at `cyrup-ext/src/host/services.rs:322` — *"BLOCK until the
   user…"*). From inside `on_terminal_input`, the calling task **is the TUI run loop**, the only
   task that can service the overlay. Blocking there is the identical self-deadlock
   `run_action.rs:200-218` spends eighteen lines explaining for shortcuts. So:
   `tokio::spawn` it, set `set_inspector_open(false)` in the spawned task's tail (pi's `finally`),
   notify on error (pi's `catch`).

### 5d. The encoding question — `KeyEvent` ⇄ `&str`

The seam carries `data: string` because pi's does; crossterm has already parsed the bytes away.
Two halves:

* **TUI → seam.** Encode `KeyEvent` back to the sequence a tty would have sent, for the subset that
  matters: `Down` → `\x1b[B`, `Up` → `\x1b[A`, `Left` → `\x1b[D`, `Right` → `\x1b[C`,
  `Enter` → `\r`, `Esc` → `\x1b`, `Char(c)` → `c`, `KeyEventKind::Release` → the kitty
  `CSI 27 u`-family release form. Anything not encodable is **delivered untouched to `handle_input`
  without consulting the seam** (never dropped). Precedent for the raw-escape convention: the
  in-tree tests already use it — `native_dispatch.rs:1639` feeds `host.terminal_input("\x1b[A")`.
  *Where:* a new `fn encode_key_event(&KeyEvent) -> Option<String>` in `crates/cyrup-tui/src/keymap.rs`,
  beside `Key::label` (`:721`), which is the existing inverse-of-parse.
* **Seam → widget.** `FleetStatusKey::from_terminal_data(&str) -> Option<FleetStatusKey>` in
  `fleet_status.rs`, next to `press` (`:1452`), implementing pi's `matchesKey` + `isKeyRelease` over
  the raw chunk for the seven `FleetStatusKeyCode` variants. `cyrup-ext-subagents` **must not**
  depend on `cyrup-tui` (arch-SA §1.1/§6.1, restated `fleet_status.rs:33`), so this parser is
  independent of `Key::parse` by design; that is correct, not duplication.

**Where cyrup can do better than upstream, and should say so in the doc comment:** pi's
`editorHasFocus` is an admitted hack. cyrup reads a real `bool` off a real editor. Record that in
the `[CYRUP-DELTA]` block of the new module.

### 5e. The facade fast path — OPTIONAL, sequence last

Not required for correctness: `facade.rs:1476-1479` already early-returns on an empty subscriber
list and `native_dispatch.rs:1630-1641` pins it. This only avoids an `.await` and one registry
read-lock per keystroke in an extension-less session. Add beside `has_markdown_transformers` (`facade.rs:1749`):

```rust
/// Whether ANY extension subscribed to raw terminal input. The TUI's key path tests this
/// before awaiting `terminal_input`, for the reason that method's own doc gives: with no
/// subscribers the fold is the identity, and upstream guards the whole block on
/// `inputListeners.size > 0` (`packages/tui/src/tui.ts:773`).
pub fn has_terminal_input_subscribers(&self) -> bool
```

---

## 6. Production call sites that must change

| # | file:line | change |
|---|---|---|
| 1 | `crates/cyrup-tui/src/app/run_action.rs:278` | fold the key through the seam before `handle_input` |
| 2 | `crates/cyrup-tui/src/app/mod.rs` (new `fn offer_to_extensions`, or a new `app/terminal_input.rs`) | the fold itself + `has_terminal_input_subscribers` gate + decode-back of a rewritten `Deliver` |
| 3 | `crates/cyrup-tui/src/keymap.rs` (~`:721`) | `encode_key_event` |
| 4 | `crates/cyrup-tui/src/app/extension_ui.rs:221` | publish editor focus |
| 5 | `crates/cyrup-tui/src/app/state.rs:264` | widen the mirror doc + field |
| 6 | `crates/cyrup-session-svc/src/host_services.rs:415`, `:891`, + `impl HostServices` | focus mirror + `editor_has_focus` override |
| 7 | `crates/cyrup-ext/src/host/services.rs:336` | `fn editor_has_focus(&self) -> bool { false }` |
| 8 | `crates/cyrup-ext/src/facade.rs:1749` | `has_terminal_input_subscribers` — **optional**, §5e |
| 9 | `crates/cyrup-ext/wit/world.wit` + `host/live.rs:295` family + `cyrup-ext-sdk/src/ctx/ui.rs` | `get-editor-has-focus` import, mirroring `get-editor-text` — **WASM tier only; deferrable** (no guest subscribes today: `git grep -rn subscribe_terminal_input -- crates/` finds no guest crate) |
| 10 | `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:275` | `api.subscribe_terminal_input()` under the `fleet_view_enabled` gate |
| 11 | `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:~1010` | `fn on_terminal_input` |
| 12 | `crates/cyrup-ext-subagents/src/extension/host/terminal_input.rs` (**new**) | dispatch, republish, inspector spawn |
| 13 | `crates/cyrup-ext-subagents/src/extension/host/mod.rs` | `mod terminal_input;` |
| 14 | `crates/cyrup-ext-subagents/src/tui/fleet_status.rs:~1452` | `FleetStatusKey::from_terminal_data` |
| 15 | `crates/cyrup-mcp/src/owner.rs:481` | the delegating-`HostServices` macro list gains `editor_has_focus` |

### 6a. Why this is more than "two wires plus the empty-editor gate"

The two wires are real and are items **1** and **10** above. Everything else is accounted for here,
because a plan that came out larger than the brief without saying why would be a plan to distrust.

| extra work | why it is not optional | evidence |
|---|---|---|
| **`handle_key` takes THREE arguments, not one** — `key`, `editor_has_focus`, `editor_text` | The brief's "empty-editor gate" is `editor_text`, which **is** already live-wired. The **focus** gate is upstream's *first* guard (`fleet-status.ts:701-704`) and has **no source on the seam at all** | `fleet_status.rs:917-921`; `git grep -n focus -- crates/cyrup-ext/src/host/services.rs` → one hit, a doc line about overlays |
| **`KeyEvent` ⇄ `&str`** | The seam's wire format is raw terminal bytes (§4a.4) and crossterm has already parsed them away by the time the app sees an `InputEvent`. Neither an encoder nor a parser exists anywhere | `app/input_reader.rs:373`; `git grep -l 'x1b\[A' -- crates/` → only the `cyrup-ext` tests and three `cyrup-intercom` **decoders** |
| **the call site must move** | `App::handle_input` is sync and returns `AppAction`; `ExtensionHost::terminal_input` is `async` and its answer is needed *before* the editor sees the key, so the shortcut spawn-and-forget pattern cannot be reused | `app/input.rs:6` vs `facade.rs:1472`; `run_action.rs:198-222` |
| **`OpenInspector` must spawn** | `show_fleet` → `open_overlay` **blocks** the calling task, which here is the TUI run loop — the only task that can close the modal. Upstream already spawns and the reason was invisible until read | `services.rs:322`; `host_services.rs:1252`,`:1262`; pi `fleet-status.ts:735-750` |
| **republish after a consumed key** | The Rust port deliberately moved pi's inline `this.refresh()` calls (`:711`,`:720`,`:726`) to the owner, so a `Consume` that moved the selection leaves the published widget stale | `fleet_status.rs` module doc `:36-37`; owner `slash.rs:298` |

Two items an earlier draft of this plan over-scoped, now trimmed: the facade fast path (§5e,
optional) and the WIT/SDK half of the focus readback (item 9, WASM-only and deferrable).

---

## 7. Tests, each with the mutation that makes it fail

| # | test | home | mutation that must break it |
|---|---|---|---|
| T1 | `down_on_empty_editor_while_running_consumes_and_activates` — drive `App::on_input_event` with a fake subscriber that returns `Consume`; assert the editor buffer is still empty and no `AppAction` was dispatched | `crates/cyrup-tui/src/tests/terminal_input_fold.rs` (new) | delete call site #1 → the key reaches `handle_input`, the editor gains a char |
| T2 | `pass_delivers_the_key_unchanged` — subscriber returns `Deliver(same)`; assert the char lands in the editor | same | make the fold return `Consume` on `None` |
| T3 | `rewritten_data_is_what_the_editor_receives` — subscriber returns `Deliver("x")` for `"a"` | same | drop the decode-back and always use the original `ev` |
| T4 | `no_subscribers_costs_no_await` — assert `has_terminal_input_subscribers()` is false and the fold is skipped | same | remove the gate (test asserts via a counting host) |
| T5 | `encode_key_event_round_trips_the_seven_codes` — `Down`→`"\x1b[B"`, `Enter`→`"\r"`, `Esc`→`"\x1b"`, `Char('j')`→`"j"`, … | `crates/cyrup-tui/src/tests/` beside the keymap tests | swap `\x1b[B` for `\x1b[A` |
| T6 | `from_terminal_data_parses_every_fleet_key` + `unknown_sequence_is_None` | `fleet_status.rs` tests module | make the parser return `Other` instead of `None` for junk → the widget deactivates on a mouse report |
| T7 | `editor_has_focus_reads_the_live_editor` — set focus false, assert `HostServices::editor_has_focus()` is false; mirrors `extension_theme_and_editor_readback.rs:87` | `crates/cyrup-tui/src/tests/extension_theme_and_editor_readback.rs` | leave the trait default → always `false` → the widget can never activate |
| T8 | `down_arrow_expands_the_roster_and_republishes` — a recording `HostServices` (pattern: `extension/host/slash_inspect_rpc.rs:208-230`); feed `"\x1b[B"` to `on_terminal_input`, assert a `set_widget` with the expanded lines | `crates/cyrup-ext-subagents/src/extension/host/terminal_input.rs` tests | drop the republish → the roster expands invisibly |
| T9 | `down_with_text_in_the_editor_is_not_consumed` — recording services report `editor_text() == "hi"` | same | pass `""` unconditionally → ↓ steals the key from a user mid-sentence |
| T10 | `losing_focus_deactivates` — `editor_has_focus() == false` after an active `↓` | same | gate the fold at `input.rs:136` instead of before the guards (the rejected alternative) → this fails, which is the whole reason it is rejected |
| T11 | `enter_opens_the_inspector_without_blocking` — assert `on_terminal_input` returns within a tight bound while the overlay sink is a blocking stub | same | await `show_fleet` inline → the test hangs (that hang IS the run-loop deadlock) |
| T12 | `subscribe_only_when_fleet_view_enabled` | `native_impl.rs` tests | drop the gate → an extension with `fleetView: false` still eats ↓ |
| T13 | end-to-end: boot the extension, start a background run, press `↓`, assert the widget payload gained the roster | `crates/cyrup-it/tests/subagents/fleet_inspector_integration.rs` (extend) | any of the above |

---

## 8. Size

**MEDIUM. Two sessions** — trimmed from an earlier MEDIUM-LARGE estimate after §4a showed the
no-subscriber path is already correct and already tested, and after the WASM half of the focus
readback was established as deferrable.

| slice | required? | production LOC (excl. doc comments, which this repo writes long) |
|---|---|---|
| TUI fold at `on_input_event` + decode-back | **yes** (wire 1) | ~70 |
| subagents `terminal_input.rs` + `init` subscribe + `on_terminal_input` | **yes** (wire 2) | ~120 |
| `encode_key_event` (`cyrup-tui`) | **yes**, §6a | ~60 |
| `FleetStatusKey::from_terminal_data` (`cyrup-ext-subagents`) | **yes**, §6a | ~70 |
| `editor_has_focus`: trait + mirror + TUI publish + mcp owner macro | **yes**, §6a | ~50 |
| `editor_has_focus`: WIT + `host/live.rs` + SDK | deferrable (WASM only) | ~40 |
| facade `has_terminal_input_subscribers` | optional (§5e) | ~12 |
| tests T1–T13 | **yes** | ~450 |

**Suggested split** (each independently landable and testable):

* **Phase 1 — the pipe.** The two wires, both halves of the encoding, and the fold at the
  *rejected-alternative* position `input.rs:136`, where focus is implicit so no readback is needed.
  Ships a working ↓/←/Enter roster. T1–T6, T8, T9, T11, T12 pass; **T10 fails and is written
  `#[ignore]`d with its reason in the attribute**.
* **Phase 2 — focus.** The `editor_has_focus` readback (native half), move the fold to
  `run_action.rs:278`, un-ignore T10. Optionally the WIT/SDK half and §5e here or later.

Do not merge phase 1 while calling UW-7 closed; the row closes at the end of phase 2.

---

## 9. Blockers

**None.** Every dependency exists:

* the host seam — §1a, 12 symbols across 5 crates;
* the widget state machine — `fleet_status.rs:917`;
* the owner and its republish path — `extension/host/mod.rs:109`, `slash.rs:298`;
* `getEditorText` — `services.rs:336`, live-wired `extension_ui.rs:190`;
* editor focus state — `crates/cyrup-tui/src/editor/config.rs:247`;
* the "host callback → subagents behaviour" precedent — `extension/host/shortcuts.rs`, whole file.

Two **hazards**, neither a blocker, both with a named mitigation:

* **H1 — run-loop self-deadlock.** The fold is awaited on the task that services `ui_rx`. Any
  handler that reaches a *blocking* `HostServices` capability (`open_overlay` `services.rs:322`;
  `confirm`/`select`/`input` via `ui_roundtrip`) from inside `on_terminal_input` deadlocks the whole
  TUI. The shortcut path escapes this by spawning (`run_action.rs:200-218`); the input path
  **cannot**, because it needs the answer synchronously. *Mitigation:* state the prohibition in
  `NativeExtension::on_terminal_input`'s doc (`native.rs:752-757`) — it already says "runs on the
  UI's input path" — and have the subagents handler spawn the inspector (§5c.4). T11 pins it.
* **H2 — the WASM tier is genuinely async** (`host/live.rs:1980`) and a guest could stall the key
  path. No guest subscribes today (`git grep subscribe_terminal_input` finds no guest crate), so
  this is not on the phase-1/2 critical path, but the fold should carry a budget; `crates/cyrup-ext/src/host/limits.rs`
  is where that already lives for other calls.

---

## 10. Does this unblock `SUBA-026`? — **No. Precisely, no.**

`SUBA-026`'s open half is the interactive `/subagents` admin UI and its selector
(`docs/gap-analysis/09-cyrup-ext-subagents.md:620`; upstream `src/slash/selector.ts`, 147 L @v0.68.0).

`git -C tmp/pi-subagents show v0.68.0:src/slash/selector.ts | grep -n 'onTerminalInput'` → **no
hits.** The file's only input entry point is:

```
125:	handleInput(keyData: string): void {
```

— a **component** method, and it is mounted through a different seam entirely
(`src/slash/subagents-admin.ts:216-219`):

```ts
if (typeof ctx.ui.custom === "function") {
    const result = await ctx.ui.custom<SelectorResult>(
        (tui, theme, kb, done) => new SelectorComponent(tui, theme, kb, { title, subtitle, items, done }),
```

`ctx.ui.custom` is pi's *focus-capturing component* API (`packages/coding-agent/src/core/extensions/types.ts:195-196`,
*"Show a custom component with keyboard focus"*). cyrup's counterpart is **`HostServices::open_overlay`**
(`crates/cyrup-ext/src/host/services.rs:332`), which **already exists and is already wired** —
`crates/cyrup-session-svc/src/host_services.rs:1262` implements it, `:1252` shows the
`open_overlay(Box::new(overlay))` call shape, and `crates/cyrup-tui/src/app/extension_ui.rs:230-241`
installs the sink.

So `SUBA-026` is blocked on **porting `SelectorComponent` as an `InteractiveOverlay`**, plus the
460-line `subagents-admin.ts` and the bare `subagents` command — not on UW-7's seam. The two are
independent, and UW-7 landing changes `SUBA-026`'s status by zero.

*(One genuine second-order benefit, stated as the small thing it is: the `KeyEvent → &str` encoder
of §5d is the same encoder any future seam that hands raw input to an extension will want, and
`FleetStatusKey::from_terminal_data` is a worked example of the `matchesKey` port that a
`SelectorComponent` port will need. Shared groundwork, not unblocking.)*
