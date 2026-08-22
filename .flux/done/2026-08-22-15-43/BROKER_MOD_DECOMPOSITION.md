---
stage: qa
status: completed
updated: 2026-08-22 20:35
---

# Decompose Broker Mod Into Submodules — Reflow Two Paragraphs

## Objective

Two `//!` paragraphs are wrapped in a way the rest of the module is not. Fix those two. **Wording is
not touched** — this is line-wrapping only, so no claim moves and no content sweep can regress.

## The QA premise was wrong; the conclusion survives

The QA that produced this task justified the fixes with:

> across the four untouched sibling files, every `//!` paragraph packs to **91–100 characters** and
> the only short lines are markdown headings or paragraph-final lines

**That is false.** Measuring every mid-paragraph wrapped line in
[`listener.rs`](../../crates/cyrup-intercom/src/broker/listener.rs),
[`ratelimit.rs`](../../crates/cyrup-intercom/src/broker/ratelimit.rs),
[`routing.rs`](../../crates/cyrup-intercom/src/broker/routing.rs) and
[`runtime_claim.rs`](../../crates/cyrup-intercom/src/broker/runtime_claim.rs) (n=43):

```
min=65  p10=77  median=96  max=100
65, 68, 74, 75, 77, 91, 91, 91, 91, 91, 92, ... 99, 99, 99, 100
```

Five mid-paragraph lines sit **below 91**. A line-length floor is not the convention.

The actual convention, and all three short untouched examples confirm it: **break early only when
the next long token would not fit.**

| Untouched site | line len | next token | len + token | |
|---|---|---|---|---|
| `listener.rs:9` | 65 | `` `tokio::net::windows::named_pipe::NamedPipeServer` `` | 116 | doesn't fit → justified |
| `listener.rs:6` | 68 | ``[`crate::transport::target::broker_listen_target`].`` | 120 | doesn't fit → justified |
| `runtime_claim.rs:1` | 75 | `` `broker/runtime-claim.ts:1-21` `` | 106 | doesn't fit → justified |

Every short line in a convention-correct file exists to keep a long backticked identifier or
intra-doc link whole. That is a deliberate style, not raggedness.

Under the corrected rule the two fixes still stand — for a better reason than QA gave.

## Scope: two files, not three

Applying the rule to every candidate:

| Candidate | line len | next token | total | verdict |
|---|---|---|---|---|
| `conn.rs:6` | 77 | ``[`spawn_connection`]`` | **98** | fits → **ragged, fix** |
| `conn.rs:7` | 79 | `in` | **82** | fits → **ragged, fix** |
| `lifecycle.rs:6` | 43 | ``[`super::listener::BrokerListener`],`` | **80** | fits → **ragged, fix** |
| `extensions.rs:10` | 90 | `` `case "extension_capabilities_update"`, `` | **130** | doesn't fit → **justified, leave alone** |

**`extensions.rs` is explicitly out of scope.** A naive greedy-fill check flags it, but that check
measures the next *word* (`` `case ``, 5 chars) rather than the next *token* (the full backticked
`` `case "extension_capabilities_update"` ``, 39 chars). Measured correctly it is the same
break-to-keep-a-token-whole pattern as `listener.rs:6`. Do not reflow it.

## The two changes

Both replacements were checked against the rule before being written here: every non-final line, plus
the first token of the line below it, exceeds 98; max width stays within the untouched files' 100.

### 1. `lifecycle.rs:4-8`

Line 6 is 43 characters mid-sentence, and the link it breaks for would have fit at 80.

```
4  (98) //! [`run`] is the `cyrup __intercom-broker` entrypoint, re-exported as `crate::broker::run` — the
5  (97) //! only public item the module root itself contributes; the four `pub mod` siblings export their
6  (43) //! own. It binds the listen target through
7 (100) //! [`super::listener::BrokerListener`], claims the runtime files, runs the accept loop, and returns
8  (80) //! once SIGTERM/SIGINT or the 5 s idle auto-shutdown has cleaned everything up.
```

Replace lines 4–8 with (widths 98 / 97 / 99 / 98 / 26, all breaks verified):

```rust
//! [`run`] is the `cyrup __intercom-broker` entrypoint, re-exported as `crate::broker::run` — the
//! only public item the module root itself contributes; the four `pub mod` siblings export their
//! own. It binds the listen target through [`super::listener::BrokerListener`], claims the runtime
//! files, runs the accept loop, and returns once SIGTERM/SIGINT or the 5 s idle auto-shutdown has
//! cleaned everything up.
```

### 2. `conn.rs:4-8`

Line 6 breaks for `` [`spawn_connection`] ``, which fits exactly at 98; line 7 breaks before the
preposition `in`, which is not a token worth protecting.

```
4 (95) //! [`writer_task`] drains queued frames; [`reader_task`] reassembles them, spends a rate-limit
5 (97) //! token, and dispatches each to [`super::state::BrokerState::handle_frame`] — the switch itself
6 (77) //! lives in `super::dispatch` — while honoring the 1 s registration timeout.
7 (79) //! [`spawn_connection`] is the pair's constructor, called from the accept loop
8 (26) //! in `super::lifecycle`.
```

Replace lines 4–8 with (widths 95 / 97 / 98 / 81, five lines become four):

```rust
//! [`writer_task`] drains queued frames; [`reader_task`] reassembles them, spends a rate-limit
//! token, and dispatches each to [`super::state::BrokerState::handle_frame`] — the switch itself
//! lives in `super::dispatch` — while honoring the 1 s registration timeout. [`spawn_connection`]
//! is the pair's constructor, called from the accept loop in `super::lifecycle`.
```

## The check, for reuse

```python
# A break is ragged iff the NEXT LINE'S FIRST TOKEN would have fit on this line.
# Token = a full backticked span or [`intra-doc link`], not a whitespace-delimited word.
# Threshold 98; untouched files observe a max of 100.
# Exempt: markdown headings, list items, paragraph-final lines, and the line-1/line-2
# title-then-citation split every header in this module uses.
```

Run it over all fourteen files; the four untouched siblings are the control and must stay at zero.

## Definition of done

* Both paragraphs replaced verbatim as drafted. No word changed, added, or removed — diff the
  whitespace-normalised text of each paragraph before and after and confirm it is identical.
* `extensions.rs` untouched.
* Ragged-break check: **0** across all fourteen files, **0** across the four untouched controls.
* Gates unchanged: clippy **3**, `cargo doc` **20**, `cargo test -p cyrup-intercom --lib` **275**.
* **No line outside a `//!` block touched**; the 47-region relocation proof still passes.
