---
stage: qa
status: completed
updated: 2026-08-28 05:19
---

# Make The Command Dispatcher Structurally Incapable Of Losing An Arm

## Objective

`Ctrl+S` in the `/model` picker did nothing for weeks: the dialog closed, no model changed,
no setting was written, and **no status line appeared**. Fixed in `cc19b87`. The cause was
not a wrong arm — the arm was correct — it was an arm sitting in a function that nothing
routes to.

The audit is done (below). It found **no second live instance** of a lost arm, and it found
exactly why: the outcome layer is compiler-enforced and the command layer is
comment-enforced. This task closes that asymmetry so the same defect cannot recur silently.

---

## Audit results — settled, do not re-investigate

### The outcome layer is safe by construction

Two independent drivers consume [`SelectorOutcome`](../../crates/cyrup-tui/src/selector/mod.rs)
(9 variants: `Ignored, Redraw, Preview, Confirm, ConfirmDefault, Apply, Cancel, OpenSubmenu,
OpenExternalEditor`), and **both are exhaustive with no catch-all**, so the compiler already
forbids the defect there:

| Driver | Location | Coverage |
| --- | --- | --- |
| TUI run loop | [`apply_selector_outcome`](../../crates/cyrup-tui/src/app/selectors.rs) `:205-206` | all 9 named at the outer level, no `_` arm |
| CLI / startup | [`startup_selector.rs`](../../crates/cyrup-tui/src/startup_selector.rs) `:110-127` | all 9 named explicitly, each ignored one carrying its reason |

`startup_selector.rs:115-122` is the pattern to copy elsewhere: it lists
`Preview | Redraw | Ignored | OpenExternalEditor | ConfirmDefault | OpenSubmenu` as no-ops
and states *why* each cannot arrive. Its claims check out against the emit matrix below.

The two `_ => {}` inside `apply_selector_outcome` (`selectors.rs:320`, `:362`) are **nested**
matches inside the `Cancel` and `OpenSubmenu` arms, not the outcome match itself. Both benign
— see the table further down.

### Emit matrix — which selector produces which outcome

Derived from every `impl Selector for` body. `Preview` is absent only because
`ListSelector::moved()` lives in a separate `impl ListSelector` block; it is emitted.

| Selector | Emits |
| --- | --- |
| `ModelSelector`, `ListSelector` | Cancel, Confirm, **ConfirmDefault**, Ignored, Redraw |
| `SettingsSelector` | Apply, Cancel, Ignored, **OpenSubmenu**, Redraw |
| `ExtensionEditorSelector` | Cancel, Confirm, Ignored, **OpenExternalEditor**, Redraw |
| `ConfigSelector` | Apply, Cancel, Ignored, Redraw |
| `SessionSelector` | Apply, Cancel, Confirm, Ignored, Redraw |
| `CheckboxSelector`, `LoginDialog`, `OAuthSelector`, `TextInputSelector`, `TreeSelector`, `TrustSelector`, `UserMessageSelector` | Cancel, Confirm, Ignored, Redraw |

### Kind coverage — one gap, verified unreachable

`SelectorKind` has 22 variants. `confirm_selector`
([`selectors.rs:409`](../../crates/cyrup-tui/src/app/selectors.rs)) services ten locally and
sends twelve on as `ConfirmSelection`; `execute_selector_command` has explicit arms for
eleven of those twelve.

The missing one is **`Settings`** — and it is genuinely unreachable: `SettingsSelector`
never emits `Confirm` (Apply / Cancel / Ignored / OpenSubmenu / Redraw only). The `Confirm`
at [`settings_selector.rs:745`](../../crates/cyrup-tui/src/settings_selector.rs) belongs to
`TrustSelector`, which shares that file but opens as kind `Trust` — and `Trust` *does* have
an arm (`execute.rs:556`). No change needed.

### Two more things that look wrong and are not

- **`ConfigSelector` is never opened by the TUI.** It is not stranded: the `cyrup config`
  subcommand drives it through `run_startup_selector`
  ([`crates/cyrup/src/subcommands.rs:860`](../../crates/cyrup/src/subcommands.rs)). This is
  why two drivers exist at all.
- **The `Apply` arm routes by payload shape, not by kind** (`selectors.rs:250-286`): Tree by
  kind, session actions by a tagged payload, everything else as a settings `id␟value` pair.
  Fragile-looking, but all three emitters are serviced and the ordering is deliberate — the
  session decode runs first precisely so it cannot mis-route into the settings branch. Leave it.

### The dispatch graph — four hand-offs, four catch-alls

Exactly five functions take `cmd: AppCommand`; `execute_command` is the only entry point
(called from `run_action.rs:163` and `run_arms.rs:603`). Every other one ends in `_ => {}`:

```
execute_command                execute.rs:31          ← sole entry, ends in `_ => misc`
├── execute_selector_command   execute.rs:52          catch-all execute.rs:584
│   └── execute_session_switch execute_session.rs:318 catch-all execute_session.rs:391  ← 2nd level
├── execute_session_command    execute_session.rs:7   catch-all execute_session.rs:311
└── execute_misc_command       execute_misc.rs:258    catch-all execute_misc.rs:743     ← the bug lived here
```

`execute_session_switch` is the one this audit nearly missed, and it is the most interesting
of the four: it is a **second-level** hand-off, reached only from a single arm of
`execute_selector_command` (`execute.rs:574-576`, `ConfirmSelection` of kind `Session` or
`UserMessage`). Its comment — *"only the two switch confirmations route here"* — is true
today, but it is enforced by the **caller's** pattern, in another file. Add a third kind to
that caller's arm without adding an arm here and the command vanishes silently. That is the
original bug's exact shape, one level deeper.

### The catch-alls, classified

There are exactly eight `_ => {}` arms in `crates/cyrup-tui/src/app/`
(`grep -rn '^\s*_ => {}$'`). **Four are benign:**

| Site | Matches | Why it is fine |
| --- | --- | --- |
| `events_fold.rs:93` | `Option<&str>` | string match, not an enum |
| `selectors.rs:362` | `id.as_str()` | string match on a submenu id |
| `selectors.rs:320` | `SelectorKind` | two special Escape destinations; the fallthrough reaches `AppAction::Redraw`, the correct default dismiss |
| `events_fold.rs:516` | `StreamEvent` | unhandled stream events are real no-ops; `MessageEnd` is the terminal path |

**The other four are the problem**, and all four are the command layer's sub-dispatchers:

| Site | Function | Comment it hides behind |
| --- | --- | --- |
| [`execute.rs:584`](../../crates/cyrup-tui/src/app/execute.rs) | `execute_selector_command` | *"the dispatcher only routes selector commands here"* |
| [`execute_misc.rs:743`](../../crates/cyrup-tui/src/app/execute_misc.rs) | `execute_misc_command` | *"the dispatcher covers every variant before here"* |
| [`execute_session.rs:311`](../../crates/cyrup-tui/src/app/execute_session.rs) | `execute_session_command` | *"the dispatcher only routes lifecycle commands here"* |
| [`execute_session.rs:391`](../../crates/cyrup-tui/src/app/execute_session.rs) | `execute_session_switch` | *"only the two switch confirmations route here"* |

Each is justified by a comment asserting an invariant that **nothing enforces**. The
`execute_misc.rs:743` assertion was flatly false for months, and that falsehood is the whole
bug.

### Which change would actually have caught this bug

Worth stating plainly, because it sets the priority and the obvious reading is wrong.

Both `ConfirmSelectionAsDefault` arms now live in `execute_misc.rs` (`Model` at `:376`,
`Thinking` at `:425`). Before `cc19b87` the `Thinking` arm was there and the `Model` arm was
in `execute_selector_command`. So under an exhaustive router (change 1 below),
`ConfirmSelectionAsDefault { .. }` is named in the `misc` bucket, everything compiles — and
`kind: Model` **still** falls to `execute_misc.rs:743` and **still** vanishes silently.

**Change 1 would not have caught this bug. Change 2 would have, immediately and loudly.**

The two cover different failure modes and both are wanted:

- **Change 1** stops a *new variant* being silently defaulted into `misc` by the `_` arm.
- **Change 2** catches a *bucket/arm mismatch* — a variant routed to a dispatcher that has no
  arm for it. That is what happened here, and it is the only one of the two that detects it.

Implement change 2 first if they are split.

### The one subtlety worth carrying forward

`events_fold.rs:511-515` matches `Done | Error` under a guard
(`if self.state.streaming_assistant`), with the catch-all at `:516`. A **guarded arm whose
guard fails falls through to the catch-all**, so a matched variant can still vanish. That is
the same defect class wearing a different hat, and no audit of variant names alone would
surface it. Here it is intentional and correct — but only the comment says so.

---

## The change

### 1. Make the router exhaustive — `crates/cyrup-tui/src/app/execute.rs:31-42`

Today the router ends in `_ => execute_misc_command(..)`, so a variant nobody thought about
is silently assigned to `misc`. Delete that `_` arm and name all 25 variants.

Verified against the enum at [`action.rs:95-195`](../../crates/cyrup-tui/src/app/action.rs):
25 variants, buckets partition them 3 + 9 + 13, every pattern shape matches its declaration.
This compiles as written:

```rust
match cmd {
    // selector family — arms live in this file
    C::OpenSelector(_)
    | C::ConfirmSelection { .. }
    | C::SetEntryLabel { .. } => {
        self.execute_selector_command(cmd, session, runtime).await
    }
    // session lifecycle — app/execute_session.rs
    C::NewSession
    | C::Reload
    | C::Import(_)
    | C::Compact(_)
    | C::Clone
    | C::DeleteSession(_)
    | C::RenameSession { .. }
    | C::Export(_)
    | C::SessionInfo => self.execute_session_command(cmd, session, runtime).await,
    // app/execute_misc.rs — LISTED, not `_`, so a new variant is a compile error until it
    // is deliberately bucketed. Note this alone does not prevent the `cc19b87` defect: that
    // was a variant routed to a dispatcher with no arm for it, which still compiles. The
    // loud catch-alls below are what catch that.
    C::ApplySetting { .. }
    | C::ConfirmSelectionAsDefault { .. }
    | C::Copy
    | C::CycleModel(_)
    | C::CycleThinking
    | C::LoginCommand(_)
    | C::ModelCommand(_)
    | C::SetModelThinkingLevel { .. }
    | C::SetName(_)
    | C::SetThinking(_)
    | C::Share
    | C::ShowName
    | C::ThinkingCommand(_) => self.execute_misc_command(cmd, session).await,
}
```

Keep `cmd` moved into the sub-calls exactly as now — the patterns above bind nothing, so
`cmd` is still available.

### 2. Make all four command catch-alls loud

`AppCommand` derives `Debug` ([`action.rs:94`](../../crates/cyrup-tui/src/app/action.rs)), so
the unrouted command can name itself. All four sites match on an owned `cmd: AppCommand` in a
`&mut self` method returning `()`, so binding `other` by move and pushing a notice compiles at
every one of them.

Replace the silent arm at each of the four sites listed in the table above:

```rust
// Was `_ => {}` under a comment asserting this is unreachable. Nothing enforced that
// assertion, and it was false for `ConfirmSelectionAsDefault { kind: Model, .. }` until
// cc19b87 — the arm existed, in a function the router never sent this variant to, so the
// command vanished with no model change, no settings write and no status. Silence is the
// one failure mode that hides a misrouted arm, so refuse to be silent.
other => {
    debug_assert!(false, "unrouted command in execute_misc_command: {other:?}");
    self.state
        .transcript
        .push_error(format!("internal: unrouted command {other:?}"));
}
```

Three details, each verified, none optional:

- **Name the function in the assert literally — do not use `module_path!()`.** Two of the four
  sites (`execute_session.rs:311` and `:391`) are in the same module, so `module_path!()`
  yields the identical string for both and the message cannot tell you which dispatcher
  dropped the command. Substitute the real function name at each site:
  `execute_selector_command`, `execute_misc_command`, `execute_session_command`,
  `execute_session_switch`.
- **`push_error`, not `push_status`.** An unrouted command is a genuine internal fault.
  [`push_error`](../../crates/cyrup-tui/src/transcript/notices.rs) `:29` pushes `Entry::Error`
  (coloured, prefixed as a problem); `push_status` `:5` is "dim and bulleted". The codebase
  already treats this distinction as a defect worth a comment — `execute_session.rs:297-300`
  criticises an earlier arm for "routing a real error to the neutral status line, where it is
  neither coloured nor prefixed as a problem." Do not repeat that mistake here.
- **Bind `other`, not `_`,** so the payload reaches both messages. `push_error` takes
  `impl Into<String>`, so `format!` passes directly.

### 3. Retire the four false comments

The four *"unreachable by construction"* / *"only … route here"* comments are replaced by the
text above. Do not leave a claim of unreachability standing next to a branch that now exists
precisely because unreachability was never guaranteed.

### 4. Record the guard case — `events_fold.rs:511-516`

Add one line to that arm's existing comment noting that a failing `streaming_assistant` guard
drops the event into the `:516` catch-all **by design**, so the fallthrough is intentional
rather than an oversight. No behavioural change.

---

## Out of scope

- `/thinking` — works end to end, confirmed by the reporter and pinned. Do not touch.
- The four benign catch-alls in the table above.
- The `Apply` payload-shape router.
- Narrowing each sub-dispatcher to its own command subtype. That would make change 2
  compiler-enforced rather than runtime-loud, but it is a type-level refactor across four
  files and is not justified by a defect this audit found.
- clippy and `cargo doc` are **already red on `main`** from `ansi.rs`, `theme.rs`,
  `editor/motion.rs` and `markdown/mermaid.rs` — untouched by this task, and separately
  queued. Do not fix them here, and do not read their output as caused by this change.

---

## Definition of done

1. `execute_command` has no `_` arm and names all 25 `AppCommand` variants across the three
   buckets.
2. All **four** command-layer catch-alls — `execute.rs:584`, `execute_misc.rs:743`,
   `execute_session.rs:311`, `execute_session.rs:391` — bind `other`, `debug_assert!` with
   their own function name spelled out, and `push_error` a visible notice.
3. None of those four still claims to be unreachable by construction.
4. The `events_fold.rs` guard note is present.
5. `cargo check --workspace --all-targets` is clean.
