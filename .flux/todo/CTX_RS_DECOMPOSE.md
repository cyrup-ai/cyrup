---
stage: exec
status: done
updated: 2026-08-22 16:59
---

# Fix Two False Rationale Comments In The ctx Decomposition

## Description

Two comments added by the last rework state things that do not survive checking. **No code changes
— both fixes are comment text only.** `scanned >= 13` stays, `register_tool` stays at
`ctx/base.rs:96`.

> **Read this before writing either comment.** The previous QA suggested replacement text for
> item 2, and **that suggestion is itself wrong twice over** — audited below. Do not paste it. This
> task is on its third pass of the same failure mode (a plausible sentence about the WIT surface
> that nobody checked), so every clause below has been verified against the source with a named
> command, and the verification tables are part of the deliverable. Anything you add beyond the
> given text must be checked the same way.

---

## Item 1 — `world_import_coverage.rs` threshold comment

[`crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs:78-81`](../../crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs)

Currently:

```rust
    // Non-vacuity: a scan that finds nothing satisfies the containment loop trivially, so the COUNT
    // is the only thing that proves this guard did any work. The literal is one per submodule plus
    // `mod.rs`, not `> 0`, so deleting a submodule without deleting its `include_str!` line is
    // caught here too — the direction the containment check cannot see.
```

The last two lines are false: deleting a submodule and leaving its `include_str!` is a **compile
error**, so the assertion never runs. The literal's real value is the *opposite* case.

### Replace lines 78-81 with exactly this

```rust
    // Non-vacuity: a scan that finds nothing satisfies the containment loop trivially, so the COUNT
    // is the only thing that proves this guard did any work. The literal is one per submodule plus
    // `mod.rs` rather than `> 0` so that REMOVING a submodule is deliberate too: dropping the file
    // and its `include_str!` line together leaves the containment loop passing over a smaller tree,
    // and only the count notices. (The opposite slip — dropping the file but keeping the
    // `include_str!` — needs no guard here; it does not compile.)
```

### Verification — every claim, checked

| Claim | Verdict | How it was checked |
|---|---|---|
| An empty scan satisfies the containment loop | TRUE | loop body never runs ⇒ `missing` empty ⇒ second `assert!` passes |
| "one per submodule plus `mod.rs`" = 13 | TRUE | `ls ctx/*.rs \| wc -l` → 13; `grep -c '^mod ' ctx/mod.rs` → 12 |
| Dropping file + its `include_str!` together leaves containment passing | TRUE | the 12 survivors are all still listed ⇒ `missing` empty; `scanned` 12 < 13 ⇒ count fires |
| Dropping the file but keeping `include_str!` does not compile | TRUE | `rustc` on a probe file: ``error: couldn't read `./definitely_not_here.rs`: No such file or directory (os error 2)`` |

All four hold. **Use this text as written.**

---

## Item 2 — `ctx/base.rs` `register_tool` clause

[`crates/cyrup-ext-sdk/src/ctx/base.rs:6-8`](../../crates/cyrup-ext-sdk/src/ctx/base.rs)

Currently claims `register_tool` "fronts no WIT import at all" — it reaches
`registration.register-tool` via [`guest::register_tool_late`](../../crates/cyrup-ext-sdk/src/guest.rs).

### The previous QA's suggested replacement is REJECTED — it is wrong twice

| Its claim | Verdict | Evidence |
|---|---|---|
| "it is the one member of this file that does front a WIT import" | **FALSE** | `grep -cE 'crate::guest::bindings::cyrup::ext::' ctx/base.rs` → **12** direct binding calls across 11 other methods (`bus::unsubscribe` `:75`, `bus::emit` `:84`, eight `ctx_state::*` `:120-216`, `control::abort` `:228`, `control::shutdown` `:240`) |
| "`tools` is the read-only introspection surface" | **FALSE** | `tools.rs:32` `set_active_tools` (write) and `tools.rs:67` `unregister_provider` — a **`registration` write**. The read/write axis does not separate `register_tool` from `tools.rs` at all |
| "buffers the executable half into `LATE_TOOLS`" | imprecise | `guest.rs:118` pushes the whole `RegisteredTool`, not a half |
| "so the host can re-materialize the tool at its next refresh" | conflated | the *import* marks the host's tool set dirty (`guest.rs:114`); `LATE_TOOLS` is **guest-side**, for the guest's own later dispatch |
| "runtime twin of the `init`-time registrations `push_registrations` flushes" | TRUE | `guest.rs:40`, inside `run_init` |

Two false premises and the distinguishing argument collapses. Discard it.

### Why a superlative cannot be used here

The obvious rewrite — "the one member that does not call its import directly" — is **also false**:
`base.rs:65` `notify` reaches `ui.notify` through `self.ui()`, and `new`/`ui`/`session`/`models`
(`:44`, `:48`, `:52`, `:60`) call nothing at all. The replacement below therefore states no
"only"/"one member" claim of any kind. Do not reintroduce one.

### The true distinguishing property

`register_tool` is the one place in `ctx` where the guest must **keep state**: the descriptor
crosses the boundary but the executor cannot, so it is parked guest-side for later dispatch. This is
already documented authoritatively at [`guest.rs:113-115`](../../crates/cyrup-ext-sdk/src/guest.rs) —

> Pushes the descriptor across the `registration.register-tool` import — which marks the host's tool
> set dirty — and stores the executor so the subsequent `execute-tool` can find it.

**Align with that wording rather than inventing a parallel account.** Restating it differently is
what produced the drift being fixed here.

### Replace lines 6-8 with exactly this

```rust
//! [`Ctx::register_tool`] sits here too. It does reach a WIT import — the descriptor crosses
//! `registration.register-tool` — but only through [`crate::guest::register_tool_late`], which also
//! stores the tool in the guest's `LATE_TOOLS` so `execute-tool` can find its executor by name: a
//! `Box<dyn ToolExec>` cannot cross the component boundary. It is that guest-side half, not the
//! import, that keeps it out of `tools`, where every method wraps `ext-tools`/`registration` and
//! nothing else.
```

### Verification — every claim, checked

| Claim | Verdict | How it was checked |
|---|---|---|
| The descriptor crosses `registration.register-tool` | TRUE | `guest.rs:117` `registration::register_tool(&lower_tool_descriptor(&tool.descriptor))`; `guest.rs:29` `use bindings::cyrup::ext::{registration, …}`; `wit/world.wit` `interface registration { register-tool: func(t: tool-descriptor); … }` |
| Only through `crate::guest::register_tool_late` | TRUE | `ctx/base.rs:103` is the sole call; no `bindings::` path in `register_tool`'s body (`:96-106`) |
| Stores the tool in `LATE_TOOLS` | TRUE | `guest.rs:118` `LATE_TOOLS.with(\|c\| c.borrow_mut().push(tool))` |
| `execute-tool` finds its executor by name | TRUE | `guest.rs:254-255` `…find(\|t\| t.descriptor.name == name).map(\|t\| t.exec.execute(…))` (also `prepare_arguments`, `:229-233`) |
| `Box<dyn ToolExec>` is the executor's type | TRUE | `api.rs:242` `pub exec: Box<dyn ToolExec>` |
| `tools` wraps `ext-tools`/`registration` and nothing else | TRUE | all six methods in `tools.rs` call only `ext_tools::*` or `registration::*` |

No superlative, no read/write claim, no "fronts no import" claim. **Use this text as written.**

---

## Definition of done

- [ ] `world_import_coverage.rs:78-81` replaced with the Item 1 text; `scanned >= 13` untouched
- [ ] `ctx/base.rs:6-8` replaced with the Item 2 text; `register_tool` still at `ctx/base.rs:96`
- [ ] Neither comment contains an "only"/"one member" claim, a "fronts no WIT import" claim, or a
      "read-only" characterisation of `tools`
- [ ] No non-comment line changed in either file — confirm with
      `grep -cE 'crate::guest::bindings::cyrup::ext::' ctx/base.rs` → still 12, and
      `grep -n 'scanned >= ' tests/world_import_coverage.rs` → still `13`
- [ ] `cargo check -p cyrup-ext-sdk` and `--target wasm32-wasip2` clean;
      `cargo clippy -p cyrup-ext-sdk --all-targets` clean
- [ ] `cargo test -p cyrup-ext-sdk` (17) and `cargo test -p cyrup-ext` (293) pass
- [ ] `cargo doc -p cyrup-ext-sdk --no-deps` reports zero warnings under `ctx/` — the new
      `[`crate::guest::register_tool_late`]` link must resolve (`guest` is `#[cfg(target_arch = "wasm32")]`
      in `lib.rs`, so if it warns on the host doc build, write the path as plain code ticks instead
      of a doc link rather than leaving a broken link)

Nothing else is in scope. `cyrup-it`'s `--features it` errors stay pre-existing
(`CYRUP_IT_COMPILE_ERRORS.md`). No third-party sources needed in `./tmp` — entirely in-tree.
