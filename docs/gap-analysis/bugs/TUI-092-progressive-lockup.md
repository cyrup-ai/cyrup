# TUI-092 — The TUI wedges and cannot be exited: input shares fate with the work loop

> **Task file for** the `TUI-092` row in [`../07-cyrup-tui.md`](../07-cyrup-tui.md) (filed 2026-08-15
> from live use, escalated to **critical** the same day).
>
> **Status** — **root cause identified structurally** by reading the run loop, the input stream and
> the signal handler at HEAD. The *structural* defect below is certain and is what this task fixes.
> The *specific* `.await` that wedges is not yet named; §5 makes naming it a by-product of the fix
> rather than a prerequisite, because the fix removes the class.
>
> **Kind** `cyrup-original` — the failing mechanism (a single serialized `biased!` loop that both
> drains input and awaits session work) has no upstream counterpart: pi's interactive mode runs on
> Node's event loop where a stalled handler cannot make `process.stdin` stop being read.
> · **Severity** **critical** · **Effort** **M**
>
> **Cross-references** — [`TUI-090`](TUI-090-post-turn-whitespace.md) (confirmed; supplies the
> *slowness*, not the *wedge*), `TUI-088` (no `Ctrl+C` binding — **fixed as part of this task**, it
> is the missing escape hatch), `TUI-091` (reasoning never renders — believed duplicate of
> `TUI-090`).

---

## 1. Symptom

Owner reports, live use 2026-08-15, in three increments:

1. *"the terminal is super fast and smooth but freezes up over time"*
2. *"keystrokes become unresponsive and rendering comes to a crawl"*
3. *"terminal gets totally locked up and can't even be killed with ctrl+d which is currently the only
   way to exit the terminal since ctrl+c is borked up"*

---

## 2. Root cause

### 2.1 Input is captured independently but **drained only by the work loop**

[`crossterm_input_stream`](../../../crates/cyrup-tui/src/app.rs) (`app.rs:8777`) spawns a **dedicated
OS thread** that blocks on `event::poll`/`event::read` and forwards over an **unbounded** channel:

```rust
pub fn crossterm_input_stream(cancel: CancelToken) -> EventStream<InputEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();
    std::thread::spawn(move || { /* blocking event::poll + read, forever */ });
    // …
}
```

**Keys are therefore never lost — only undrained.** They accumulate in the channel. The only code
that drains that channel is arm #5 of the run loop.

### 2.2 The run loop is one serialized task, and every arm body awaits

`App::run` (`app.rs:8019`) is a single `tokio::select!` with **21 arms** and `biased;`. Arm order is
`cancel` → 3 tickers → **`input.next()`** (#5) → … → `events.next()` (#19). Sixteen `.await`
points sit inside arm bodies — `ingest_session_event`, `execute_command`, `rt.session()`, and more.

Because it is one task, **an arm body that blocks indefinitely blocks the loop**, and because the
loop is the sole input drain, **it blocks input with it.** That is the entire bug:

> Input capture is decoupled from the work loop. Input *servicing* is not.

### 2.3 Why `Ctrl+D` cannot save you

`Ctrl+D` is not special. It arrives as an ordinary `InputEvent`, is drained by arm #5, mapped by
`handle_input`, and only then becomes `AppAction::Quit => break`:

```rust
maybe_in = input.next() => {
    let Some(ev) = maybe_in else { break };
    match self.handle_input(&ev) {
        AppAction::Quit => break,
```

If the loop is wedged, arm #5 never runs, so the quit key is sitting in the channel unread. **The
exit path is downstream of the thing that is broken.**

### 2.4 Why `Ctrl+C` cannot save you either — and this is the sharpest finding

A correct escalating signal handler already exists.
[`spawn_abort_on_signal`](../../../crates/cyrup/src/signals.rs) (`signals.rs:189`) reaps detached
children, arms a repeat watcher **before** awaiting teardown, and the second signal **hard-exits from
its own spawned task**:

```rust
let repeat = tokio::spawn(async move {
    let again = wait_for_signal().await;
    kill_tracked_detached_children();
    std::process::exit(again.exit_code());   // independent of the run loop
});
```

That task is not blocked by the wedged loop and would terminate the process. **It is unreachable**
because the TUI puts the terminal in **raw mode**, where `Ctrl+C` is delivered by crossterm as a
`KeyEvent` and *never becomes SIGINT* — and `TUI-088` established there is no global `Ctrl+C`
binding to catch it either. So both the signal path and the key path are dead, and the working
hard-exit is stranded behind them.

### 2.5 Why it is progressive

Nothing here grows unboundedly on its own; `TUI-090` supplies the slowdown that makes the wedge
reachable. Its confirmed mechanism keeps the inline viewport pinned at `term_h`, and every turn
pushes blank rows into native scrollback (~31 per turn, measured in its repro). Frame cost and
terminal-side cost climb with turn count, arm bodies take longer, and the window in which any
contended `.await` can stall widens — until one does not return.

**Already ruled out — do not re-check:** `TranscriptView::pending` is emptied by `drain_committed`
via `std::mem::take` (`transcript.rs:553-556`); `active_tools` is drained at `transcript.rs:899`
and `:935`; all five tickers set `MissedTickBehavior::Skip` (`app.rs:7908-7934`), so no tick burst;
`FooterGitBranch::poll` (`footer_data.rs:143`) is a `stat` fingerprint that returns early and draws
nothing when unchanged.

---

## 3. Required change 1 — an escape hatch that cannot be blocked

**Where:** [`crates/cyrup-tui/src/app.rs`](../../../crates/cyrup-tui/src/app.rs), inside
`crossterm_input_stream`'s reader thread.

The reader thread is already independent of the async runtime and already sees every key before
anything else. It is the only place in the process guaranteed to still be running when the loop is
wedged. **The escalation is recognised there, at the source, and does not travel through the
channel.**

Mirror `signals.rs`'s escalation exactly — first press requests teardown, second press hard-exits —
so keyboard and signal behave identically:

```rust
// In the reader thread, immediately after an event is read and BEFORE it is handed to
// EscapeReassembler / StrayReplyFilter (a held/reassembled key must not delay the escape).
//
// Raw mode means Ctrl+C never becomes SIGINT (`signals.rs:189` can therefore never fire from the
// keyboard). This reproduces that handler's contract on the key path: first = cooperative cancel,
// second = unconditional exit from a context the run loop cannot block.
const ESCALATE: [KeyCode; 2] = [KeyCode::Char('c'), KeyCode::Char('d')];
if let Event::Key(k) = &ev
    && k.modifiers.contains(KeyModifiers::CONTROL)
    && ESCALATE.contains(&k.code)
{
    if escalated.swap(true, Ordering::SeqCst) {
        // Second press: the cooperative path already failed. Reap and leave.
        cyrup_tools::ops::local::kill_tracked_detached_children();
        let _ = crossterm::terminal::disable_raw_mode();
        std::process::exit(130);
    }
    cancel.cancel();          // cooperative: unblocks the loop's `cancel` arm (#1, biased first)
    // fall through: still forward the key so the normal Quit/abort path runs when the loop is live
}
```

`escalated` is an `AtomicBool` owned by the thread closure. `disable_raw_mode` before exit is
mandatory — exiting raw leaves the user's terminal unusable, which is how this bug currently ends.

**This is the load-bearing half of the task.** With it, no future wedge can trap the user.

## 4. Required change 2 — `Ctrl+C` gets a real binding (closes `TUI-088`)

**Where:** [`crates/cyrup-tui/src/keymap.rs`](../../../crates/cyrup-tui/src/keymap.rs), the global
chord table, alongside the existing `ctrl_code` families.

Bind `Ctrl+C` to the interrupt action pi binds it to — abort the running turn when one is running,
otherwise the quit escalation. Derive the exact split from pi's interactive keybinding table before
writing it; `TUI-067`/`TUI-068` are the same class (chord parses, destination missing) and should be
checked in the same pass.

## 5. Required change 3 — the loop stops sharing fate with input

**Where:** [`crates/cyrup-tui/src/app.rs`](../../../crates/cyrup-tui/src/app.rs), `App::run`.

Every `.await` in an arm body is a place the sole input drain can stop. Two changes, both required:

**5a — bound every await.** Wrap each awaiting arm body in `tokio::time::timeout`. On elapse, log
the arm and continue the loop rather than hanging. This converts an indefinite wedge into a visible,
recoverable stall **and names the offending await in the log the first time it fires** — which is
how the specific culprit gets identified without a prior repro:

```rust
maybe_ev = events.next() => {
    let Some(ev) = maybe_ev else { continue };
    // A wedged handler must degrade the session, never the process. The elapsed branch is the
    // diagnostic: it names the arm and the event kind that stalled.
    if tokio::time::timeout(ARM_BUDGET, self.ingest_session_event(&ev, &session))
        .await
        .is_err()
    {
        tracing::error!(arm = "events", kind = ev.kind(), "run-loop arm exceeded budget");
    }
    // …
}
```

`ARM_BUDGET` is a module const; 5s is generous for any handler that is not broken.

**5b — long work is spawned, not awaited inline.** Any arm body whose work is unbounded by nature
(a command execution, a session build, a runtime swap) must `tokio::spawn` and return its result
through an existing channel arm. The loop's job is ordering and drawing, not executing.

---

## 6. Definition of done

1. `Ctrl+C` and `Ctrl+D` exit the TUI **while the loop is wedged** — verified by wedging it (a
   `sleep` injected into an arm body is sufficient) and pressing the key.
2. On that exit the terminal is left in **cooked mode** — the shell prompt works normally, no
   `reset` required.
3. First press requests cooperative teardown; second press exits unconditionally. Same contract as
   `signals.rs:209-219`.
4. `Ctrl+C` has a global binding; `TUI-088` closes with it.
5. Every awaiting arm in `App::run` is budget-bounded, and an exceeded budget logs the arm name and
   continues rather than hanging.
6. A long live session no longer wedges; if it still degrades, that residue belongs to `TUI-090` and
   is tracked there, **not reopened here**.

---

## 7. Sequencing

`TUI-090` is confirmed and supplies the slowdown that makes this reachable. Land its fix, then this
one; re-measure a long session before opening any further TUI performance item.

---

## 8. Evidence appendix

Owner reports, verbatim:

```
the terminal is super fast and smooth but freezes up over time
keystrokes become unresponsive and rendering comes to a crawl
terminal gets totally locked up and can't even be killed with ctrl+d
which is currently the only way to exit the terminal since ctrl+c is borked up
```

Read at HEAD:

```
app.rs:8019          App::run — tokio::select!, biased;, 21 arms, 16 .await in arm bodies
app.rs:8069          arm #5: maybe_in = input.next() => … AppAction::Quit => break
app.rs:8449          arm #19: maybe_ev = events.next() => self.ingest_session_event(..).await
app.rs:8777          crossterm_input_stream — dedicated std::thread, unbounded channel
app.rs:7908-7934     all five tickers set MissedTickBehavior::Skip
signals.rs:189       spawn_abort_on_signal — correct escalation, unreachable in raw mode
signals.rs:216-218   second signal: kill_tracked_detached_children() + process::exit
footer_data.rs:143   FooterGitBranch::poll — stat fingerprint, early return, no draw
transcript.rs:553    drain_committed — std::mem::take(&mut self.pending)
transcript.rs:899    active_tools.drain(..)
```
