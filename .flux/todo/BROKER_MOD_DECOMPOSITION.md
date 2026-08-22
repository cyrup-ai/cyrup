---
stage: qa
status: needs-rework
updated: 2026-08-22 19:30
---

# Decompose Broker Mod Into Submodules — Reflow Two Headers

**QA rating: 9/10.** Every factual claim in all fourteen headers is now verified clean across three
independent sweeps plus a direct identifier check. What remains is two ragged paragraphs, both
artifacts of the correction passes, in a module where no untouched file has one.

## Verified complete — do not redo

* **All five edits applied and correct.** `session.rs` now says `registered` reply + `session_joined`
  broadcast and names `BrokerState::on_connection_closed` as the other departure path;
  `receipts.rs` dropped the false generalization; `dispatch.rs` claims frame-type dispatch;
  `state.rs` dropped the unsupported quantifier.
* **Sweep A — frame names:** 5 tokens, 0 real violations. The single hit (`extensions.rs` naming
  `registered`) is the known cross-module reference, verified at `session.rs:130`
  (`Registered { .. features: None }`).
* **Sweep B — quantifiers:** 16 claims, each enumerated with a command, 0 violations.
* **Sweep C — numeric counts (new this round; neither prior sweep covered it):** 10 claims, 0
  violations. `limits.rs` 9-of-11 ✓, `frame.rs` three states / two bits ✓, `mailbox.rs` eight
  methods ✓, `lifecycle.rs` four siblings ✓ and 5 s = `SHUTDOWN_DELAY_MS` 5000 ✓, `conn.rs` 1 s =
  `REGISTRATION_TIMEOUT_MS` 1000 ✓, `dispatch.rs` one method ✓, `test_support.rs` six importers ✓
  (a naïve grep says 7 — the extra is `mod.rs:57 mod test_support;`, a declaration, not an import).
* **Non-frame identifiers:** `on_connection_closed` in `state.rs:250` ✓, `handle_frame` in
  `dispatch.rs` ✓, `queue_mailbox_message` in `mailbox.rs` ✓.
* **Relocation:** 47/47 regions byte-identical, in order, contiguous.
* **Gates:** clippy **3**, `cargo doc` **20**, `cargo test --lib` **275** — all at baseline.

## Known limit of this audit — not a defect, do not chase

`broker.ts` is **not vendored anywhere in this repo** (`find -iname 'broker.ts'` returns nothing, no
`./tmp`). Every `broker.ts:NNN` citation in every header is therefore **unverifiable in this
environment**, including the ones the correction passes edited. They are carried over from the
original file and were not invented, but nobody has checked them against upstream. Do not "verify"
them by inspection — either vendor upstream and check, or leave them.

## Outstanding

The house convention is unambiguous and was measured, not assumed. Across the four untouched sibling
files (`listener.rs`, `ratelimit.rs`, `routing.rs`, `runtime_claim.rs`), every `//!` paragraph packs
to **91–100 characters** and the only short lines are markdown headings (`listener.rs:16`) or
**paragraph-final** lines (`ratelimit.rs:3`, `runtime_claim.rs:17`). There is **not one
mid-paragraph orphan** in any of them.

Both files below break that, and both breaks were introduced by the correction passes.

### 1. `lifecycle.rs:4-8` — a 43-character line between a 97 and a 100

```
4 (100) //! [`run`] is the `cyrup __intercom-broker` entrypoint, re-exported as `crate::broker::run` — the
5  (97) //! only public item the module root itself contributes; the four `pub mod` siblings export their
6  (43) //! own. It binds the listen target through
7 (100) //! [`super::listener::BrokerListener`], claims the runtime files, runs the accept loop, and returns
8  (80) //! once SIGTERM/SIGINT or the 5 s idle auto-shutdown has cleaned everything up.
```

Line 6 is mid-sentence and mid-paragraph — the sentence continues onto line 7. Replace lines 4–8:

```rust
//! [`run`] is the `cyrup __intercom-broker` entrypoint, re-exported as `crate::broker::run` — the
//! only public item the module root itself contributes; the four `pub mod` siblings export their
//! own. It binds the listen target through [`super::listener::BrokerListener`], claims the runtime
//! files, runs the accept loop, and returns once SIGTERM/SIGINT or the 5 s idle auto-shutdown has
//! cleaned everything up.
```

### 2. `conn.rs:4-8` — two under-packed lines, five where four fit

```
4 (95) //! [`writer_task`] drains queued frames; [`reader_task`] reassembles them, spends a rate-limit
5 (99) //! token, and dispatches each to [`super::state::BrokerState::handle_frame`] — the switch itself
6 (79) //! lives in `super::dispatch` — while honoring the 1 s registration timeout.
7 (79) //! [`spawn_connection`] is the pair's constructor, called from the accept loop
8 (26) //! in `super::lifecycle`.
```

Lines 6 and 7 sit at 79 where the surrounding paragraph packs to 95–99. Replace lines 4–8:

```rust
//! [`writer_task`] drains queued frames; [`reader_task`] reassembles them, spends a rate-limit
//! token, and dispatches each to [`super::state::BrokerState::handle_frame`] — the switch itself
//! lives in `super::dispatch` — while honoring the 1 s registration timeout. [`spawn_connection`]
//! is the pair's constructor, called from the accept loop in `super::lifecycle`.
```

## Definition of done

* Both paragraphs reflowed exactly as drafted. **Wording is unchanged** — this is pure line wrapping,
  so no claim is touched and no sweep can regress.
* No `//!` paragraph in the fourteen files has a mid-paragraph line under ~90 characters:

  ```sh
  # a short //! line is acceptable only when the next line is `//!` (paragraph end) or it is a heading
  ```
* Gates unchanged: clippy **3**, `cargo doc` **20**, `cargo test -p cyrup-intercom --lib` **275**.
* **No line outside a `//!` block touched**; the 47-region relocation proof must still pass.
