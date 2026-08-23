---
title: Deferred Config Write Loses Toggles On Err And Replays Snapshots
priority: MEDIUM
stage: aug
status: done
updated: 2026-08-23 03:49
---

# `cyrup config` buffers its writes past a fallible loop: they are lost on `Err` and replayed on success

## Objective

Restore the invariant the branch broke: **a toggle is durable the moment the selector acknowledges
it on screen.** Do that by finishing the async propagation the branch started — not by patching the
buffer that only exists because the propagation stopped one crate short.

## What is actually broken

`cyrup-config`'s `persist_nested` became `async` on this branch
([`manager.rs:323`](../../crates/cyrup-config/src/settings/manager.rs)). Its `cyrup config` caller
writes from inside [`run_startup_selector`](../../crates/cyrup-tui/src/startup_selector.rs)'s
`on_apply: impl FnMut(&str)`, which is sync and cannot await. Rather than make the callback async,
the branch buffered the writes and flushed them after the loop
([`subcommands.rs:880-905`](../../crates/cyrup/src/subcommands.rs)):

```rust
let mut pending: Vec<(SettingsScope, &'static str, serde_json::Value)> = Vec::new();
run_startup_selector(&theme, &keymap, &mut selector, |payload| {
    ...
    pending.push((settings_scope, toggle.kind.key(), value));   // one whole-array snapshot per toggle
})?;                                                            // <-- Err here drops `pending` unwritten

for (settings_scope, key, value) in pending {
    if let Err(e) = settings.persist_nested(settings_scope, &[key], value).await { ... }
}
```

### 1. Writes are lost when the selector returns `Err`

`run_startup_selector` returns `Result<SelectorOutcome, TuiError>` and produces `Err` from two
places that run on **every** loop iteration, i.e. after arbitrarily many `on_apply` calls have
already fired ([`startup_selector.rs:87,90`](../../crates/cyrup-tui/src/startup_selector.rs)):

```rust
.map_err(|e| TuiError::Backend(e.to_string()))?;              // terminal.draw
match event::read().map_err(|e| TuiError::Backend(e.to_string()))? {
```

`event::read()` errors on stdin EOF / a broken terminal (window closed, SSH transport drop, stdin
closed underneath the process); `terminal.draw` errors on a write failure to that same terminal.
These are exactly the "the user's terminal went away" cases where the buffered edits matter most.
Before the branch, each toggle was written the instant it happened, so the same failure left every
prior toggle safely on disk and lost only the in-flight interaction.

The `Cancel` path is already safe: `ConfigSelector::handle` never emits `Confirm`
([`config_selector.rs:725-852`](../../crates/cyrup-tui/src/config_selector.rs) produce only
`Apply`/`Redraw`/`Cancel`/`Ignored`), so `Esc` returns `Ok(SelectorOutcome::Cancel)`, `?` passes it
through and the flush runs. `Err` is the only non-signal exit that skips the flush, and it is the
only one whose behaviour changed.

### 2. Every superseded snapshot is replayed

`pending` stores one **whole-array snapshot per toggle** and the flush writes all of them in order.
There are at most **six** distinct `(scope, key)` pairs — `{Global, Project} x {skills, prompts,
themes}`, seeded at [`subcommands.rs:820-825`](../../crates/cyrup/src/subcommands.rs) — so a session
with 40 toggles performs 40 locked read-modify-write cycles where 6 would produce byte-identical
output. Each one takes the scope lock via `self.store.with_lock(scope, ...)`, parses the whole
settings document, sets the node and re-serialises with `to_pretty()`
([`manager.rs:323-353`](../../crates/cyrup-config/src/settings/manager.rs);
[`store.rs:63-87`](../../crates/cyrup-config/src/settings/store.rs)). All of it now lands in one
burst *after* teardown, so `cyrup config` pauses on exit in proportion to how much the user toggled
— on the command whose entire purpose is bulk toggling.

The final bytes are correct (last-write-wins over ordered snapshots resolves to the same state);
this half is cost and stale-intermediate-state only.

### What is **not** broken — do not "fix" these

- **The identical-bytes claim holds.** The callback does `entry.retain(...); entry.push(...)` on
  `arrays.entry((scope, kind))` and serialises the **whole** entry, and `persist_nested` replaces
  the entire `[key]` node via `set_value_at_path`. Each snapshot is independent
  (`entry.iter().cloned()`), so replay order makes the last snapshot per key the final value.
- **`persist_err` semantics are unchanged.** Both the old callback and the new loop only ever *set*
  `persist_err` and never clear it on a later success, in the same iteration order.

## Why the small fix is not the right fix

The obvious minimal change is "capture the outcome, flush unconditionally, then `outcome?`", plus an
`IndexMap` to collapse the six keys. It is smaller. It is also the wrong shape, for four reasons:

1. **It preserves the broken invariant.** The window between "the screen says this skill is
   disabled" and "the byte is on disk" still spans the entire remainder of the session. Flushing on
   `Err` narrows the loss set from `{Err, panic, signal}` to `{panic, signal}`; it does not restore
   durability-on-acknowledgement.
2. **`catch_unwind` is a dead end here.** The release profile sets `panic = "abort"`
   ([`Cargo.toml:284`](../../Cargo.toml)), so there is no unwind to catch and no `Drop` to run — a
   panic anywhere in `inner.render`/`inner.handle` takes the whole buffer with it, in exactly the
   builds users run. `crates/cyrup-tui/src/panic_hook.rs` exists for precisely this reason and says
   so at `:17-20`. There is no buffer-preserving mitigation for the panic path; the only real
   mitigation is not having a buffer.
3. **It contradicts the branch's own governing spec.** [`CONFIG_LOCK_CONTENTION.md`
   §Step 3 / DoD](../done/2026-08-23-00-08/CONFIG_LOCK_CONTENTION.md) says "All eight call sites
   become async. One lock API, async throughout", "Restructure `builder.rs:623`'s `.and_then(…)`
   chain rather than forcing a block-on", and "every downstream caller in `cyrup` /
   `cyrup-session-svc` compile async-clean; no `block_on` introduced". The `pending` buffer is a
   block-on substitute in a trench coat: it exists solely because one downstream caller was not
   made async. Finishing the job is what the spec asks for.
4. **Problem 2 dissolves rather than being papered over.** With no buffer there is no replay to
   dedupe: writes go back to being one-per-toggle, interleaved with the user reading the screen,
   which is what the pre-branch code did and what upstream pi does in `toggleTopLevelResource`. A
   dedupe map would be new machinery whose only job is to compensate for machinery that should not
   exist.

The cost is real and must be stated plainly: this changes a `pub` signature in `cyrup-tui` and
turns two `pub fn`s in `cyrup` async. That ripple is enumerated exhaustively below and terminates
after **two** private functions in `main.rs`. It is bounded, mechanical, and already the direction
this branch is travelling (28 `pub` signatures moved — see
[`LOW-public-api-changes-beyond-the-async-keyword.md`](LOW-public-api-changes-beyond-the-async-keyword.md)).

## Required implementation path

### Step 1 — `run_startup_selector` becomes `async` and takes `impl AsyncFnMut(&str)`

[`crates/cyrup-tui/src/startup_selector.rs`](../../crates/cyrup-tui/src/startup_selector.rs)

Replace lines 36-66 with the following. Two things change beyond `async`: the callback type, and the
hand-rolled three-way teardown becomes one RAII guard (see the rationale in its doc comment — this
is a direct consequence of the async conversion, not an unrelated cleanup).

```rust
/// Restore the terminal on EVERY exit from [`run_startup_selector`] — the two setup errors, the
/// loop's `?`, and (new with `async`) a **future-drop**: the loop now suspends at each
/// `on_apply(..).await`, so the caller's future can be dropped mid-selector where the sync version
/// could not be. The old straight-line restore at the foot of the function ran on none of those.
///
/// Total and idempotent: every step is `let _ =` so a terminal that rejects one escape does not
/// stop the rest, and leaving an alternate screen that was never entered is harmless.
///
/// This does NOT cover a panic — `panic = "abort"` in the release profile means no unwind and no
/// `Drop`. [`crate::panic_hook::restore_terminal_best_effort`] is that path's only recourse.
struct StartupTerminalRestore;

impl Drop for StartupTerminalRestore {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = out.execute(LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = out.execute(Show);
    }
}

pub async fn run_startup_selector(
    theme: &UiTheme,
    keymap: &SelectKeymap,
    inner: &mut dyn Selector,
    on_apply: impl AsyncFnMut(&str),
) -> Result<SelectorOutcome, TuiError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| TuiError::Backend(e.to_string()))?;
    // Armed the instant raw mode is on, so every exit below unwinds through `Drop`.
    let _restore = StartupTerminalRestore;
    stdout
        .execute(EnterAlternateScreen)
        .map_err(|e| TuiError::Backend(e.to_string()))?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))
        .map_err(|e| TuiError::Backend(e.to_string()))?;

    run_loop(&mut terminal, theme, keymap, inner, on_apply).await
}
```

Add `Show` to the crossterm imports at the top of the file:

```rust
use ratatui::crossterm::cursor::Show;
```

`Terminal::show_cursor` is exactly `execute!(writer, Show)`, so emitting `Show` on stdout from the
guard is equivalent and does not need the `Terminal` to still be alive.

`run_loop` (line 68) becomes `async fn`, its bound becomes `mut on_apply: impl AsyncFnMut(&str)`,
and the one `Apply` arm at line 96 gains an await:

```rust
SelectorOutcome::Apply(payload) => on_apply(&payload).await,
```

Nothing else inside `run_loop` changes. Update the function's doc comment: `on_apply` is now
awaited before the loop redraws, so an in-place mutation is durable before the frame that reflects
it is painted — which is the property the whole task exists to restore.

**Required doc comment on the blocking read.** `event::read()` stays a blocking call, so this
`async fn` parks its executor thread while waiting for a key. Say so in the function's doc rather
than leaving a reader to discover it — an `async fn` with a blocking syscall in it must announce
itself. Suggested wording, and it must carry the pointer:

```rust
/// `async` because [`SelectorOutcome::Apply`] is now awaited: `on_apply` persists the mutation
/// before the loop repaints the row that shows it. The **input** read is still the blocking
/// `event::read()`, so this parks its executor thread between keys — unchanged from the sync
/// version every caller already blocked on, and NOT fixable in isolation: see
/// `.flux/` task "unify the pre-launch input path with the app reader" for why a second
/// background reader on stdin is unsafe while [`crate::app::crossterm_input_stream`] is coupled
/// to `App::run`'s singleton statics.
```

Do **not** rewrite the loop onto [`crossterm_input_stream`](../../crates/cyrup-tui/src/app/input_reader.rs)
(`:312`) as part of this task. That was evaluated properly rather than deferred for size; the
evidence and the two concrete defects it would introduce are recorded below.

### Step 2 — the six call sites

All six live in `crates/cyrup`. Four are already inside `async fn`s and need only `.await` plus the
`async` keyword on the closure. Two are sync wrappers that must become async.

| # | Call site | Enclosing fn | Change |
| --- | --- | --- | --- |
| 1 | [`subcommands.rs:881`](../../crates/cyrup/src/subcommands.rs) | `run_config` — already `async fn` (`:813`) | rewritten in Step 4 |
| 2 | [`startup_ui.rs:425`](../../crates/cyrup/src/startup_ui.rs) | `run_trust_prompt` — already `pub async fn` (`:401`) | `async \|_\| {}` + `.await` |
| 3 | [`startup.rs:267`](../../crates/cyrup/src/startup.rs) | `run_first_time_setup` — already `pub async fn` (`:258`) | `async \|_\| {}` + `.await` |
| 4 | [`startup.rs:277`](../../crates/cyrup/src/startup.rs) | same fn | `async \|_\| {}` + `.await` |
| 5 | [`startup_ui.rs:498`](../../crates/cyrup/src/startup_ui.rs) | `run_missing_cwd_prompt` — `pub fn` (`:473`) | fn → `pub async fn`; `async \|_\| {}` + `.await` |
| 6 | [`startup_ui.rs:228`](../../crates/cyrup/src/startup_ui.rs) | `run_resume_picker` — `pub fn` (`:205`) | fn → `pub async fn`; closure → `async \|payload: &str\| { … }` + `.await` |

For #3 and #4 the two-step wizard reads:

```rust
let theme = match run_startup_selector(ui, &keymap, &mut selector, async |_| {}).await? {
```

For #6 the body is unchanged — the delete/rename work stays synchronous inside an async closure,
which is legal and costs nothing:

```rust
let outcome = run_startup_selector(theme, &keymaps.0, &mut selector, async |payload: &str| {
    match SessionSelectorOutcome::parse_apply(payload) {
        ...   // body verbatim; `status.push(..)` still borrows `status` mutably
    }
})
.await?;
```

`AsyncFnMut` is the lending trait: the async closure may borrow both the `&str` argument and its
captured `&mut` state across an await. Do **not** hand-roll `F: FnMut(&str) -> Fut` — that shape
cannot express the argument's higher-ranked lifetime and forces every callback to copy the payload.

### Step 3 — the ripple from #5 and #6, in full

Only two functions call the two wrappers that turn async, and both are private to
[`crates/cyrup/src/main.rs`](../../crates/cyrup/src/main.rs):

| Function | Line | Why | Callers to fix |
| --- | --- | --- | --- |
| `resolve_session` | `:1351` | calls `run_missing_cwd_prompt` at `:1398` | `main.rs:510` |
| `resolve_startup_ui` | `:1434` | calls `run_resume_picker` at `:1455` and `run_missing_cwd_prompt` at `:1482` | `main.rs:520` |

Both become `async fn`. Both callers already sit inside `async fn run()` (`main.rs:94`), inside
`let`-chains that take `.await` without restructuring:

```rust
if (cli.fork.is_some() || cli.session.is_some() || cli.session_id.is_some())
    && let Some(code) = resolve_session(&cli, &dirs, mode, &mut config).await?
{
    return Ok(code);
}
...
if mode == AppMode::Interactive
    && let Some(code) = resolve_startup_ui(&cli, &dirs, mode, &mut config).await?
{
    return Ok(code);
}
```

**The ripple stops there.** Verified exhaustively:

- `run_trust_prompt` (#2) is reached only through `trust_prompt_callback` (`main.rs:1529`), which
  already wraps it in `Box::pin(async move { … .await })`; the builder awaits that plainly at
  [`builder.rs:684`](../../crates/cyrup-session-svc/src/builder.rs) with no `select!`/timeout.
- `run_first_time_setup` (#3/#4) is already awaited at `main.rs:382`.
- `resolve_session` / `resolve_startup_ui` have no other callers and no test callers (they are
  private `fn`s in `main.rs`).
- `crates/cyrup/tests/first_time_setup.rs` touches only the pure `should_run_first_time_setup*`
  predicates.
- The `pub use` re-exports at [`lib.rs:67-75`](../../crates/cyrup/src/lib.rs) and
  [`cyrup-tui/src/lib.rs:195`](../../crates/cyrup-tui/src/lib.rs) need no edit — a `pub use` of an
  item whose signature changed is unaffected.

### Step 4 — `run_config`: delete the buffer, write at toggle time

[`crates/cyrup/src/subcommands.rs:874-905`](../../crates/cyrup/src/subcommands.rs). Delete the
`pending` vector, delete the six-line comment above it that explains why it exists, delete the flush
loop, and put the write back where it belongs:

```rust
    let mut persist_err: Option<String> = None;
    run_startup_selector(&theme, &keymap, &mut selector, async |payload: &str| {
        let Some(toggle) = ConfigToggle::from_payload(payload) else {
            return;
        };
        let settings_scope = match toggle.scope {
            ConfigScope::User => SettingsScope::Global,
            ConfigScope::Project => SettingsScope::Project,
        };
        let entry = arrays.entry((toggle.scope, toggle.kind)).or_default();
        // Drop any prior +/-/! entry for this exact pattern, then push the new decision (Pi
        // `toggleTopLevelResource`, config-selector.ts:471-480): enabling writes `+pattern`,
        // disabling `-pattern`.
        entry.retain(|p| strip_override_marker(p) != toggle.pattern);
        entry.push(format!("{}{}", if toggle.enabled { '+' } else { '-' }, toggle.pattern));
        let value = serde_json::Value::Array(
            entry.iter().cloned().map(serde_json::Value::String).collect(),
        );
        // Awaited HERE, before the loop redraws: the row the selector is about to paint as
        // enabled/disabled is already on disk. `run_startup_selector`'s `Err` (a dead terminal
        // mid-session) can now only lose the in-flight toggle, never the ones already made.
        if let Err(e) = settings.persist_nested(settings_scope, &[toggle.kind.key()], value).await {
            persist_err = Some(e.to_string());
        }
    })
    .await?;

    if let Some(e) = persist_err {
        // A project toggle in an untrusted folder is the usual cause (Pi requires trust to write
        // project settings). Surface it after teardown rather than silently swallowing.
        eprintln!("Some changes could not be saved: {e}");
        eprintln!("(use --approve to modify project settings in an untrusted folder)");
        return Ok(1);
    }
    Ok(0)
```

This is byte-for-byte the pre-branch body of
[`4902cddf`'s `run_config`](../../crates/cyrup/src/subcommands.rs) plus `async`/`.await`, which is
the point: the restructure's *only* justified delta was the two keywords.

Borrow notes, all verified against the toolchain:

- The async closure captures `&settings` (immutable — `persist_nested` takes `&self`), `&mut arrays`
  and `&mut persist_err`. `selector` is borrowed separately as `&mut selector` by the call itself;
  the four locals are disjoint, so there is no conflict.
- `entry` is a `&mut Vec<String>` into `arrays`, and its last use (`entry.iter().cloned()`) precedes
  the await, so NLL ends that borrow before the suspension point. Nothing needs to be cloned or
  scoped by hand.
- `ConfigToggle` is fully owned (`pattern: String`,
  [`config_selector.rs:185-190`](../../crates/cyrup-tui/src/config_selector.rs)), so it carries no
  lifetime into the future.
- `arrays` is read before the call by `config_override_states(&rows, &arrays)` (`:855`) and not used
  after it — no conflict at either end.

## Toolchain viability — verified, not assumed

Workspace is `edition = "2024"`, `rust-version = "1.96"`
([`Cargo.toml:88-89`](../../Cargo.toml)), toolchain `stable` = rustc 1.98.0. `AsyncFnMut` and async
closures are stable. Three standalone `rustc --edition 2024` probes confirm the exact shapes this
spec requires compile:

1. `async fn f(.., on_apply: impl AsyncFnMut(&str))` called with `async |payload: &str| { … }` that
   borrows `payload` **and** mutably borrows two captured locals across an `.await` — clean.
2. The same parameter accepting both `async |_| {}` and an async closure with a purely synchronous
   body — clean.
3. `.await?` inside a `let`-chain condition (`if cond && let Some(x) = f().await? { … }`) — clean.

## Trade-offs, accepted

- **`persist_err` is still discarded when the selector returns `Err`.** `?` propagates the terminal
  failure and the "Some changes could not be saved" line never prints. This is identical to
  pre-branch behaviour and is the right precedence: a dead terminal is the more urgent report, and
  the writes that *did* succeed are already durable. Do not restructure the `?` to chase it.
- **N writes per session return.** Six would suffice for the final state, but they are now spread
  across the user's interaction instead of serialised into an exit-time burst, which is what the
  pre-branch code and upstream pi both do. Latency per toggle is one `FileLock::acquire` +
  parse + `to_pretty` + atomic write — the same work the old sync callback did inline, except it now
  suspends the task instead of blocking the thread.
- **A signal still loses the in-flight toggle.** Unfixable and out of scope; the acknowledged
  SIGKILL regression shrinks from "everything this session" to "the one keypress in flight".
- **`run_startup_selector`'s future is now droppable.** No call site races or times it out today
  (checked: all six await it plainly), and `StartupTerminalRestore` covers the case if one ever
  does.

## Evaluated: moving the pre-launch read onto `crossterm_input_stream`

Raised as a scoping challenge — the signature becomes `async` while the read stays blocking, which
is arguably the same impedance-mismatch-papered-over defect as the buffer being removed. It was
evaluated on the code rather than declined on size. **Verdict: excluded, for cause.** Three findings.

### Finding 1 — the premise that this is a "hot" moment does not survive the code

The argument for urgency was that `run_trust_prompt` is invoked through `TrustPromptFn` at
[`builder.rs:684`](../../crates/cyrup-session-svc/src/builder.rs), i.e. inside
`SessionBuilder::build`, on the runtime, so a blocking read there parks a worker while a session is
under construction. Checked:

- There is **no production `tokio::spawn` anywhere on the path from process start to that await**.
  `grep tokio::spawn` over [`main.rs`](../../crates/cyrup/src/main.rs) returns nothing before
  `:810`; over [`builder.rs`](../../crates/cyrup-session-svc/src/builder.rs) and
  [`factory.rs`](../../crates/cyrup-session-svc/src/factory.rs), nothing at all; the hits in
  [`host_services.rs`](../../crates/cyrup-session-svc/src/host_services.rs) are all inside
  `#[cfg(test)] mod tests` (opens at `:1877`).
- [`spawn_abort_on_signal`](../../crates/cyrup/src/main.rs) is called at `main.rs:804`, **after**
  `build()` has returned — so the signal watcher does not even exist during the trust prompt and
  cannot be starved by it.
- `build()` is one sequential await chain on one task. Parking its worker starves nothing, because
  nothing else is on the runtime.

So the blocking read at that site costs a parked worker thread on a `multi_thread` runtime with
nothing to run on it. It is a wart, not a live defect — and it is **exactly as true today**:
`run_trust_prompt` is already `pub async fn` on this branch and already calls the sync
`run_startup_selector` inline. This task neither adds nor removes one blocked-thread microsecond.

### Finding 2 — `crossterm_input_stream` is not a neutral input source; adopting it hard-exits the process

[`input_reader.rs:312`](../../crates/cyrup-tui/src/app/input_reader.rs) is `App::run`'s input path,
and its reader thread carries the TUI-092 wedge watchdog. `Escalation::on_press` (`:231-263`) counts
`Ctrl+C`/`Ctrl+D` chords (`is_escalate_chord`, `:176`) that arrive while the process-global
`INPUT_SERVICED` counter has not moved; at `PANIC_PRESSES = 3` (`:153`) spaced by
`PANIC_MIN_GAP = 250 ms` (`:165`) it calls `hard_exit_from_reader()` (`:187`) →
`std::process::exit(130)`. `INPUT_SERVICED` is bumped only by `App::run`'s input arm via
`mark_input_serviced()`.

The pre-launch modal has no `App::run`. So:

- `app.session.delete` is **`Ctrl+D`** ([`keymap.rs:1061,1075`](../../crates/cyrup-tui/src/keymap.rs)),
  bound in the `SessionSelector` that [`run_resume_picker`](../../crates/cyrup/src/startup_ui.rs)
  mounts pre-launch. Deleting three sessions from the `--resume` picker at a human pace — an
  entirely ordinary action, and one the branch already ported `delete_session_file_at` for —
  would trip the ladder against a frozen counter and **kill the process at exit 130**.
- `tui.select.cancel` is `Esc` / **`Ctrl+C`** ([`keymap.rs:430,445`](../../crates/cyrup-tui/src/keymap.rs)),
  so the same chord family is the cancel binding on all six mounts.

The statics are not an accident of implementation; `INPUT_SERVICED`'s own doc (`:41-49`) justifies
being a `static` on *"there is exactly one interactive run loop per process, and
[`crossterm_input_stream`] has a single production caller (`crates/cyrup/src/main.rs`)"*. Adopting
it here makes that sentence false, which is the root of the defect, not a side effect of it.

It is mitigable — one `mark_input_serviced()` call in the pre-launch loop resets the ladder on every
key — but the mitigation is a second, non-obvious coupling of the pre-launch modal to `App::run`'s
private watchdog protocol, and it requires rewriting that justification. That is a deliberate design
change to the wedge watchdog's contract, not a line of plumbing.

### Finding 3 — any background reader introduces a keystroke-destroying window that only a process singleton closes

This is the structural one, and it is independent of `crossterm_input_stream` specifically.

The reader loop is `while !tx.is_closed() && … { event::poll(wait) … }`
([`input_reader.rs:326-372`](../../crates/cyrup-tui/src/app/input_reader.rs)) with
`INPUT_POLL_INTERVAL = 100 ms` (`:19`). When the selector unmounts and the receiver drops, the
thread is already inside `poll(100 ms)`. A key pressed in that window is consumed by `event::read()`,
fails `tx.send`, and is **destroyed** — `break 'reader`, byte gone from crossterm's queue.

Today's design has no such window: the read happens on the calling thread and stops the instant the
loop returns, so typeahead stays in the OS buffer for the next reader. Adopting a background reader
would therefore add a fresh data-loss class to a task whose entire purpose is removing one — and it
is reachable: [`run_first_time_setup`](../../crates/cyrup/src/startup.rs) mounts two selectors
back to back (`:267`, `:277`), and `resolve_startup_ui` can mount the missing-cwd prompt immediately
after the resume picker confirms ([`main.rs:1455`](../../crates/cyrup/src/main.rs) → `:1482`). 100 ms
is well inside human key-to-key time on a wizard the user has seen before.

Shrinking `INPUT_POLL_INTERVAL` narrows the window; it cannot close it, because a thread parked in
`event::read()` cannot be cancelled and the read consumes the byte. **The only lifetime that has no
window is one reader for the life of the process** — which means one `EventStream<InputEvent>`
hoisted across the whole pre-launch phase and handed on to `App::run` (which already takes one:
[`app/run.rs:33-36`](../../crates/cyrup-tui/src/app/run.rs)).

That hoist is where the real cost is, and it is not size — it is a public type in a third crate.
`TrustPromptFn` ([`builder.rs:438-445`](../../crates/cyrup-session-svc/src/builder.rs)) is
`Arc<dyn for<'a> Fn(&'a [TrustOption], &'a Option<TrustEntry>) -> Pin<Box<dyn Future + Send + 'a>> + Send + Sync>`
— an `Arc<dyn Fn>`, `Clone`d into the builder at [`factory.rs:169,201`](../../crates/cyrup-session-svc/src/factory.rs).
It cannot own a `&mut EventStream`; threading the shared reader into it needs
`Arc<tokio::sync::Mutex<EventStream<InputEvent>>>` captured in the closure, on a type that already
has its own open finding ([`MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md`](MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md)).

### What this task does instead, and what the follow-up is

Excluded here because the unification is a **design change to two contracts in two other crates**
(the wedge watchdog's singleton invariant, and `TrustPromptFn`'s shape), with a hard-exit and a
keystroke-loss defect on the naive path — not because it is large. Bundling it would also make this
task unreviewable: the data-loss fix and the input-path unification would land as one diff with
independent risk.

Two things this task does carry, so nothing is papered over:

1. The blocking read is **stated in the source**, in `run_startup_selector`'s doc, with the pointer
   (see Step 1). It is not left for a reader to discover.
2. Recommended follow-up, to be filed with `/task`: **"Unify the pre-launch input path with the app
   reader."** Its scope is fixed by the findings above — (a) decouple `Escalation`/`INPUT_SERVICED`
   from the "one run loop per process" assumption, or give any input-servicing loop an explicit way
   to signal servicing; (b) construct one `EventStream<InputEvent>` in `main.rs` before the
   pre-launch phase and thread it through `run_startup_selector`, all six call sites,
   `resolve_session`, `resolve_startup_ui` and `App::run`; (c) redesign `TrustPromptFn` to carry it,
   coordinating with the open finding on that type. `cyrup config` keeps its own short-lived stream
   (single mount, process exits immediately after — no window that matters).

Two genuine wins are deferred with it and should be named in that task: the pre-launch selectors
would inherit `EscapeReassembler` (today a split escape sequence delivers a bare `Esc`, which is
`tui.select.cancel` — TUI-045 reaches these modals too) and `StrayReplyFilter`.

### On the RAII guard — the two changes do not collapse

Asked whether `StartupTerminalRestore` becomes unnecessary once the read is cancellable at an await
point. It does not, in either world:

- The guard exists for **future-drop**, not for the read. The sync function could not be cancelled;
  the async one can be dropped at any suspension point, and this change creates the first ones
  (`on_apply(..).await`). The old straight-line restore at the foot of the function runs on none of
  those paths.
- Adopting the stream would make the guard **more** necessary, not less: awaiting the input adds a
  suspension point on every single iteration rather than only on `Apply`, so the droppable surface
  grows from "while persisting a toggle" to "the entire modal".

The guard is required either way, and it is ~12 lines that also delete two hand-rolled
partial-restore blocks.

## Out of scope — do not do these

- Rewriting the pre-launch loop onto `crossterm_input_stream` / any other background reader — see
  the evaluation above for the two defects it introduces and the follow-up task it belongs to.
- Adding `IndexMap` or any dedupe structure: with the buffer gone there is nothing to dedupe.
- `std::panic::catch_unwind` around the selector: `panic = "abort"` makes it inert in release.
- `block_on` / `block_in_place` in any form — the branch's own spec bans it.
- Touching `persist_nested`, `with_lock` or `FileLock`; this task consumes them unchanged.

## Definition of done

- [ ] `cyrup_tui::run_startup_selector` is `pub async fn` with `on_apply: impl AsyncFnMut(&str)`, and
      `run_loop` awaits the callback in its `SelectorOutcome::Apply` arm.
- [ ] Terminal teardown in `startup_selector.rs` is a single `Drop` guard armed immediately after
      `enable_raw_mode()`; the two hand-rolled partial-restore blocks and the straight-line restore
      at the foot of the function are gone.
- [ ] All six call sites pass an async closure and `.await` the call.
- [ ] `run_resume_picker` and `run_missing_cwd_prompt` are `pub async fn`; `resolve_session` and
      `resolve_startup_ui` are `async fn`; `main.rs:510` and `main.rs:520` await them.
- [ ] `subcommands.rs` contains no `pending` vector, no post-loop flush loop, and no comment
      explaining why the write cannot happen in the callback — `persist_nested` is awaited inside
      the callback.
- [ ] No `block_on`, `block_in_place`, `catch_unwind`, `IndexMap`, or spawned writer task anywhere in
      the change.
- [ ] `run_startup_selector`'s doc states that `event::read()` still blocks the executor thread and
      points at the input-path unification follow-up; no background reader is introduced.
- [ ] `cargo check --workspace --all-targets` and `cargo clippy --workspace --all-targets` are clean.
- [ ] Manual: `cyrup config`, toggle several resources, then kill the terminal window (or close the
      SSH transport) rather than pressing `Esc`. Re-running `cyrup config` shows every toggle made
      before the kill. Pressing `Esc` normally is unchanged.
