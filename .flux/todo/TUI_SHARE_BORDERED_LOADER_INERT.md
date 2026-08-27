---
stage: todo
status: pending
updated: 2026-08-27
---

# Drive And Cancel `/share`'s BorderedLoader Instead Of Blocking The Run Loop On `gh`

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** medium · **Kind:** partial-behaviour · **Area:** Interactive mode shell, footer, status and execution views

## Objective

`/share` currently freezes the whole TUI for the duration of the gist upload: no spinner, no
"Creating gist..." message, no repaint, and Escape/Ctrl+C do nothing — they are not even read until
`gh` exits. The widget that should be on screen is already built and unit-tested; what is missing is
the drive/cancel wiring around it. After this, `/share` shows the bordered loader over the editor,
spins at 80 ms, advertises `escape/ctrl+c cancel` truthfully, and Escape kills the `gh` child and
prints `Share cancelled`.

## Upstream reference

[`packages/coding-agent/src/modes/interactive/session-share.ts`](../../tmp/pi/packages/coding-agent/src/modes/interactive/session-share.ts):

- `:152-203` `shareViaGist` — builds `new BorderedLoader(context.ui, theme, "Creating gist...")`,
  clears the editor container and mounts it, `context.ui.setFocus(loader)` +
  `context.ui.requestRender()`, then spawns `gh gist create --public=false` **asynchronously** while
  the render loop keeps running.
- `:157-161` `loader.onAbort = () => { proc?.kill(); restoreEditor(loader, context); context.showStatus("Share cancelled"); }`.
- `:174`, `:191-199` — every completion path re-checks `loader.signal.aborted` before touching the UI.
- `:60-70` — a `gh auth status` pre-check that reports
  `"GitHub CLI is not logged in. Run 'gh auth login' first."`.
- Animation: [`packages/tui/src/components/loader.ts:74-80`](../../tmp/pi/packages/tui/src/components/loader.ts)
  `Loader.restartAnimation`'s `setInterval(…, this.intervalMs)` at **80 ms**.
- Cancellation: [`cancellable-loader.ts:31-37`](../../tmp/pi/packages/tui/src/components/cancellable-loader.ts)
  `CancellableLoader.handleInput` → `kb.matches(data, "tui.select.cancel")` →
  `abortController.abort(); onAbort?.()`; [`bordered-loader.ts:34-36`](../../tmp/pi/packages/tui/src/components/bordered-loader.ts)
  renders the `keyHint("tui.select.cancel", "cancel")` row that advertises it.
- The Radius upload branch (`session-share.ts:100-147`) is a pi-operated service and is **out of
  scope**.

## Current state in cyrup-tui

**The widget is done.** [`chrome.rs:305-321`](../../crates/cyrup-tui/src/chrome.rs)
(`struct BorderedLoader`, `cancellable()` at `:319`) and its `render` at `:345` are a complete port
— row count, colours and the cancel-hint label are pinned by
`crates/cyrup-tui/src/tests/chrome.rs:89-118` and
`crates/cyrup-tui/src/tests/footer_chrome_fidelity.rs:553-590`. **Do not rebuild it.**

What is missing is everything around it:

| symptom | evidence |
|---|---|
| no frame is ever produced with the loader set | [`app/execute_misc.rs:338`](../../crates/cyrup-tui/src/app/execute_misc.rs) `share_session` sets `self.state.loader = Some(BorderedLoader::cancellable(...))` at `:363`, then immediately `Command::new("gh")` (`:370`) `.output().await` (`:373`) with **no** `draw`/`draw_synchronized()` in between, and clears it at `:375`. |
| the run loop cannot draw or read keys during the upload | [`app/run_action.rs:152-153`](../../crates/cyrup-tui/src/app/run_action.rs) — `AppAction::Command(cmd) => self.execute_command(cmd, …).await` is awaited **inline on the run loop's own task**. Neither the spinner arm nor the input arm can run while `gh` is in flight. |
| the spinner would be frozen on frame 0 even if drawn | `state.loader_tick` has exactly **two** non-test occurrences workspace-wide: `loader_tick: 0` at [`app/state.rs:314`](../../crates/cyrup-tui/src/app/state.rs) and the read at [`app/render.rs:120`](../../crates/cyrup-tui/src/app/render.rs) (`loader.render(frame, slot_area, &state.theme, state.loader_tick)`). Nothing increments it — despite its own doc at `state.rs:108-110` claiming it is "advanced by the run-loop tick". |
| the existing 80 ms arm does not touch it | [`app/run_arms.rs:305-316`](../../crates/cyrup-tui/src/app/run_arms.rs) `on_spinner_tick` only bumps the transcript render tick and redraws. |
| the advertised cancel key is wired to nothing | no `Child`/`AbortHandle` is retained anywhere in `share_session`, and `grep -rn "Share cancelled" crates/` returns nothing. |
| no logged-out guidance | `gh auth` appears nowhere in the repo (the only `provider_auth_status` hits are unrelated login paths), so pi's pre-check at `session-share.ts:60-70` has no counterpart. `share_session`'s error arms (`execute_misc.rs:410-424`) surface raw `gh` stderr. |

## Subtasks

1. **`crates/cyrup-tui/src/app/execute_misc.rs:363-375`** — add pi's pre-check before the loader is
   mounted: run `gh auth status`, and on failure push
   `"GitHub CLI is not logged in. Run 'gh auth login' first."` (`session-share.ts:60-70`) and return.
   Keep the existing `ErrorKind::NotFound` → "gh is not installed" arm (`:419-422`).
2. **`crates/cyrup-tui/src/app/execute_misc.rs`** — make the gist step non-blocking so the run loop
   keeps drawing: `spawn()` the `gh` child rather than `.output().await`, and either drive it from a
   run-loop arm or, at minimum, `draw_synchronized()` immediately after setting `state.loader` and
   then await the child inside a `select!` that also polls input and the spinner tick.
3. **`crates/cyrup-tui/src/app/run_arms.rs:305`** — advance `self.state.loader_tick` from the
   existing 80 ms `on_spinner_tick` arm whenever `self.state.loader.is_some()`, honouring the
   promise already written at `app/state.rs:108-110`. `app/render.rs:120` already consumes it.
4. **`crates/cyrup-tui/src/app/execute_misc.rs` (+ `crates/cyrup-tui/src/app/state.rs` if the handle
   must be stored)** — retain the `tokio::process::Child` (or an `AbortHandle`) for the in-flight
   `gh` run.
5. **`crates/cyrup-tui/src/app/execute_misc.rs`** — route `SelectAction::Cancel` (the very action
   whose label `BorderedLoader::cancellable` renders, `chrome.rs:317-320`) to `kill()` the child,
   clear `state.loader`, and `push_status("Share cancelled")` — `session-share.ts:157-161`.
6. **`crates/cyrup-tui/src/app/execute_misc.rs`** — after the await resolves, re-check the cancelled
   flag before touching the transcript, matching `session-share.ts:174,191-199`; a cancelled run must
   not also print a gist URL or an error. The temp file at `:352-358` must still be removed on every
   path, cancellation included.

## Acceptance criteria

- [ ] `grep -rn "loader_tick" crates/cyrup-tui/src --include=*.rs | grep -v '/tests/'` shows a
      **write** other than the `loader_tick: 0` initialiser at `app/state.rs:314`.
- [ ] That write lives in `App::on_spinner_tick` (`app/run_arms.rs:305`) and is conditioned on
      `state.loader.is_some()`.
- [ ] `crates/cyrup-tui/src/app/execute_misc.rs` no longer contains
      `Command::new("gh") … .output().await` on the gist path; the child is spawned and awaited
      alongside the input/tick arms.
- [ ] A `draw`/`draw_synchronized` call happens **after** `self.state.loader = Some(...)` and before
      the gist result is consumed.
- [ ] `grep -rn "Share cancelled" crates/cyrup-tui/src` returns a `push_status` call reachable from
      `SelectAction::Cancel` while the share loader is mounted.
- [ ] The spawned `gh` child (or its abort handle) is stored somewhere the cancel path can reach it,
      and the cancel path calls `kill()`.
- [ ] A cancelled `/share` prints `Share cancelled` and **not** a gist URL or a `share error:` line.
- [ ] `grep -rn "gh auth" crates/cyrup-tui/src` returns the pre-check, and a logged-out `gh` produces
      `GitHub CLI is not logged in. Run 'gh auth login' first.` rather than raw stderr.
- [ ] The temp HTML file written at `execute_misc.rs:352` is removed on the success, error and
      cancel paths.
- [ ] `crates/cyrup-tui/src/chrome.rs`'s `BorderedLoader` (`:305-321`, `render` at `:345`) is
      unchanged.
- [ ] `cargo build -p cyrup-tui` → 0 warnings; `cargo clippy -p cyrup-tui --all-targets` → no new
      diagnostics.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
