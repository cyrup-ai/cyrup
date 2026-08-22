---
stage: exec
status: done
updated: 2026-08-22 16:47
---

# Decompose Broker Mod Into Submodules

## Description

Decompose [`crates/cyrup-intercom/src/broker/mod.rs`](../../crates/cyrup-intercom/src/broker/mod.rs)
into submodules along its logical seams.

At **3,292 lines / 156 KB** it is the largest Rust source file in `cyrup-intercom` — ~15% of the
crate's 21,364 lines and nearly 2x the next largest
([`src/transport/client.rs`](../../crates/cyrup-intercom/src/transport/client.rs), 1,705). The
`broker/` directory already carries four sibling submodules, so the house pattern exists; `mod.rs`
simply never got split.

This is a **pure relocation**. No behaviour changes, no API changes, no new tests, no rewritten
logic. Every line that moves must move verbatim except for `use` paths and visibility keywords.

---

## Research — what is actually in the file

### Composition

| Region | Lines | Share |
|---|---|---|
| Module docs (1–21) + `mod` decls + imports (23–45) | 45 | 1% |
| Constants (47–68) | 22 | 1% |
| Validation + JS-coercion helpers (70–190) | 121 | 4% |
| State structs (192–291) | 100 | 3% |
| Frame plumbing (293–337) | 45 | 1% |
| **`impl BrokerState` (339–1653)** | **1,315** | **40%** |
| `apply_presence_context` (1655–1690) | 36 | 1% |
| Connection tasks + process lifecycle (1692–2122) | 431 | 13% |
| `mod tests` (2124–2407) | 284 | 9% |
| `mod presence_context_tests` (2409–3292) | 884 | 27% |

Two facts drive the whole plan: **`impl BrokerState` is 40% of the file**, and **tests are 35%**.

### The code is a cited 1:1 port

Every item carries a `broker.ts:NNN` citation and many carry `[CYRUP-DELTA]` / `ICOM-NNN` notes.
That traceability is the crate's main review affordance — the module split must **preserve every
doc comment verbatim with its item**, and each new file gets a `//!` header naming the upstream
region it covers, matching the existing house style in
[`routing.rs`](../../crates/cyrup-intercom/src/broker/routing.rs),
[`ratelimit.rs`](../../crates/cyrup-intercom/src/broker/ratelimit.rs) and
[`listener.rs`](../../crates/cyrup-intercom/src/broker/listener.rs) (the last of which even
documents *why the file exists* — do the same).

### Public API surface is tiny

The only `pub` item in `mod.rs` is `pub async fn run()` (1923–2066). External consumers:

- [`src/bin/cyrup-intercom-broker.rs:12`](../../crates/cyrup-intercom/src/bin/cyrup-intercom-broker.rs)
- [`crates/cyrup/src/intercom_broker_cmd.rs:49`](../../crates/cyrup/src/intercom_broker_cmd.rs)
- `crates/cyrup-it/tests/intercom/broker_startup_fail_fast.rs`, `broker_runtime_claim.rs` (prose)
- `broker/listener.rs:66` intra-doc link `[`crate::broker::run`]`

Everything else externally referenced (`broker::listener::BrokerListener`,
`broker::routing::find_session_ids`, `broker::runtime_claim::probe_pid`) already lives in a sibling
submodule. **So the entire contents of `mod.rs` except `run` is private and free to move.**

### The call graph is cleanly layered

Extracted from every `self.<method>(` inside `impl BrokerState`:

- `handle_frame` → all eleven `handle_*` handlers (pure dispatch, no other callers)
- handlers → state primitives and mailbox helpers
- exactly **one** handler→handler call: `handle_send` → `handle_send_to_disconnected` (same concern)
- mailbox helpers → state primitives; state primitives → state primitives

There is **no cycle at the concern level**, so the split below needs no back-references and no
trait indirection.

### Helper ownership (confirmed by call-site trace)

| Helper | Call sites | Owner module |
|---|---|---|
| `extensions_field_is_valid` | `handle_extension_capabilities_update` (890), `handle_register` (1096) | `extensions.rs`, `pub(super)` |
| `namespace_is_valid` | only `extensions_field_is_valid` (89) | `extensions.rs`, private |
| `js_string_or_empty` | `handle_extension_state_commit` (972) | `js.rs`, `pub(super)` |
| `js_is_falsy`/`js_to_string`/`js_number_to_string` | only inside `js_string_or_empty` | `js.rs`, private |
| `js_truthy_alias` | mailbox identity guard (559, 564) | `js.rs`, `pub(super)` |
| `session_owns_connection` | 884, 932, 966 — all extension handlers | `extensions.rs`, private |
| `apply_presence_context` | 1640–1642 only | `presence.rs`, private |
| `lock` | lifecycle + conn tasks + tests | `state.rs`, `pub(super)` |
| `send_msg` | every handler | `frame.rs`, `pub(super)` |

### `cfg` gating is already contained

The only `#[cfg(unix)]` / `#[cfg(windows)]` in the file is inside `TerminateSignal` (2076–2122),
which moves wholesale into `lifecycle.rs`. **No cfg block is split across the new boundary** — do
not create one.

### Lints

[`src/lib.rs:14`](../../crates/cyrup-intercom/src/lib.rs) carries
`#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` as a
crate-level inner attribute, so it applies unchanged to every new file. Clippy pedantic is **not**
enabled (`[workspace.lints.clippy]` in the root `Cargo.toml` sets only those four to `deny`), so no
`#[must_use]` churn is required on moved items.

Both existing test modules open with:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
```

**Every new `mod tests` block and `test_support.rs` must repeat that allow line**, or the crate-level
`deny` will fail the build.

---

## The one technical fact that decides the design

Rust visibility is **"visible in module M" ⇒ visible in M and every descendant of M**.

`BrokerState` and its fields are private today, which works because `mod.rs` holds both the struct
and every `impl`. Once the `impl` blocks move to sibling files, a sibling is *not* a descendant of
`state.rs`, so private access is lost.

The fix is `pub(super)`, applied uniformly:

```rust
// broker/state.rs
pub(super) struct BrokerState {          // pub(super) here == "visible in crate::broker"
    pub(super) sessions: HashMap<String, ConnectedSession>,
    // ...
}
```

`pub(super)` written in `crate::broker::state` scopes visibility to `crate::broker` — which
transitively covers `crate::broker::send`, `crate::broker::dispatch`, and even
`crate::broker::send::tests`. One keyword, one rule, every direction.

**Therefore: keep all new modules as flat direct children of `broker`.** Do not nest a
`broker/handlers/` directory — at two levels deep `pub(super)` stops reaching siblings and you would
need `pub(in crate::broker)` everywhere instead.

Two corollaries:

- Inherent `impl BrokerState { ... }` blocks may be split across files freely; coherence is per-crate.
- Declare the new modules as plain `mod x;` (**not** `pub mod`). The four existing siblings stay
  `pub mod` because external code names them. Adding `pub mod state;` would *widen* the public API,
  breaking the acceptance criterion.

---

## Target layout

`crates/cyrup-intercom/src/broker/` — 4 existing files untouched, `mod.rs` gutted, 13 new files.

| File | Moved from (line ranges in current `mod.rs`) | ~lines |
|---|---|---|
| `mod.rs` | docs 1–21; `mod` decls; `pub use lifecycle::run;` | ~55 |
| `limits.rs` | 47–68 (all 11 consts) | ~32 |
| `js.rs` | 113–190 | ~88 |
| `frame.rs` | 293–337 (`FrameOutcome`, `FrameResult`, `impl FrameResult`, `send_msg`) | ~55 |
| `state.rs` | 192–205, 234–291, 324–326; methods 340–465, 671–708 | ~300 |
| `mailbox.rs` | 207–223; methods 466–670 | ~235 |
| `dispatch.rs` | 709–807 (`handle_frame`) | ~110 |
| `session.rs` | 1061–1229 (`handle_register`, `handle_unregister`, `handle_list`) | ~185 |
| `send.rs` | 1230–1559 (`handle_send`, `handle_send_to_disconnected`, `handle_cancel_ask`) | ~345 |
| `receipts.rs` | 225–232; 808–850, 980–1060 | ~145 |
| `presence.rs` | 1560–1690 (`handle_presence` + `apply_presence_context`) | ~145 |
| `extensions.rs` | 70–111; 851–979 | ~185 |
| `conn.rs` | 1721–1883 (`writer_task`, `PayloadOutcome`, `process_frame_payload`, `reader_task`, `spawn_connection`) | ~175 |
| `lifecycle.rs` | 1692–1719, 1885–2122 (`schedule_shutdown_check`, `shutdown_broker`, `run`, `describe_listen_target`, `TerminateSignal`) | ~285 |
| `test_support.rs` | deduplicated test helpers (see below) | ~70 |

Largest resulting file is `send.rs` at ~345 lines plus its tests — cohesive, and an order of
magnitude better than 3,292.

### Exact method ranges inside `impl BrokerState`

Doc-comment blocks included; ranges are contiguous and exhaustive over 340–1653.

**→ `state.rs`** (core bookkeeping)

```
340-360  new                                        361-373  with_listen_endpoint
374-380  add_connection                             381-402  mark_unregistered
403-406  remove_unregistered                        407-412  sessions_in_order
413-421  insert_session                             422-428  remove_session
429-443  broadcast                                  444-447  clear_ask_edges_for_session
448-452  prune_ask_edges                            453-458  clear_message_receipt_routes_for_session
459-465  prune_message_receipt_routes               671-677  session_infos
678-708  on_connection_closed
```

**→ `mailbox.rs`** (offline delivery, v0.10.1)

```
466-476  remember_disconnected_session              477-483  prune_disconnected_sessions
484-504  prune_mailbox_messages                     505-536  queue_mailbox_message
537-572  find_live_sessions_sharing_mailbox_identity
573-591  find_unique_live_session_for_disconnected_session
592-603  find_disconnected_session_ids              604-670  flush_mailbox_for_session
```

**→ `dispatch.rs`**: `709-807 handle_frame`

**→ `receipts.rs`**: `808-850 handle_message_receipt`, `980-1060 handle_cancel_message`

**→ `extensions.rs`**: `851-898 handle_extension_capabilities_update`,
`899-905 session_owns_connection`, `906-940 handle_extension_publish`,
`941-979 handle_extension_state_commit`

**→ `session.rs`**: `1061-1181 handle_register`, `1182-1214 handle_unregister`,
`1215-1229 handle_list`

**→ `send.rs`**: `1230-1414 handle_send`, `1415-1539 handle_send_to_disconnected`,
`1540-1559 handle_cancel_ask`

**→ `presence.rs`**: `1560-1653 handle_presence`, plus free fn `1655-1690 apply_presence_context`

### Free-item ranges outside the impl

```
1692-1719 schedule_shutdown_check  → lifecycle.rs
1721-1732 writer_task              → conn.rs
1734-1740 PayloadOutcome           → conn.rs
1742-1786 process_frame_payload    → conn.rs
1788-1866 reader_task              → conn.rs
1868-1883 spawn_connection         → conn.rs
1885-1921 shutdown_broker          → lifecycle.rs
1923-2066 run                      → lifecycle.rs  (stays `pub`)
2068-2074 describe_listen_target   → lifecycle.rs
2076-2085 TerminateSignal          → lifecycle.rs
2087-2122 impl TerminateSignal     → lifecycle.rs
```

---

## Required visibility changes

Mark `pub(super)` (everything else stays private):

- **`state.rs`** — `BrokerState` + **all 18 fields**; `ConnectedSession` + its 4 fields;
  `ConnHandle` + its field; `lock`; **all 15 methods** listed above.
- **`mailbox.rs`** — `DisconnectedSession` + 2 fields; `MailboxMessage` + 4 fields; all 8 methods.
- **`receipts.rs`** — `MessageReceiptRoute` + 3 fields; `handle_message_receipt`,
  `handle_cancel_message`.
- **`frame.rs`** — `FrameOutcome` (and its 3 variants are public with the enum);
  `FrameResult` + its 3 fields; `cont`/`close_self`/`protocol_error`; `send_msg`.
- **`limits.rs`** — all 11 consts.
- **`js.rs`** — `js_string_or_empty`, `js_truthy_alias` only.
- **`extensions.rs`** — `extensions_field_is_valid` and the three `handle_extension_*` methods only.
  `namespace_is_valid` and `session_owns_connection` stay private.
- **`session.rs`**, **`send.rs`**, **`presence.rs`**, **`dispatch.rs`** — only the methods
  `handle_frame` dispatches to. `handle_send_to_disconnected` stays private (single caller,
  same file). `apply_presence_context` stays private.
- **`conn.rs`** — `spawn_connection` only (called by `run`). `process_frame_payload` needs
  `pub(super)` only because `conn.rs`'s own tests are in-file — keep it private.
- **`lifecycle.rs`** — `run` stays `pub`; `schedule_shutdown_check` is `pub(super)` (called from
  `conn.rs`); everything else private.
- **`test_support.rs`** — all helpers `pub(super)`.

`state.rs` names types owned by `mailbox.rs` and `receipts.rs` in `BrokerState`'s fields. That is a
normal mutual module reference in Rust and needs no special handling — just `use super::mailbox::{
DisconnectedSession, MailboxMessage};` etc.

---

## Patterns to follow

### New file header (house style)

```rust
//! Mailbox + disconnected-session retention (`v0.10.1 broker/broker.ts:85-95,880-925,1010-1024`).
//!
//! Mail parked for a peer that has left, redelivered when that identity registers again. Split out
//! of `broker/mod.rs`, where it was interleaved with the connection bookkeeping it does not depend
//! on; the eight methods here form a closed set whose only outward calls are to `state.rs`
//! primitives.

use std::collections::HashMap;

use crate::transport::protocol::{Message, SessionInfo};

use super::js::js_truthy_alias;
use super::limits::{MAILBOX_MESSAGE_RETENTION_MS, MAX_MAILBOX_MESSAGES};
use super::state::BrokerState;
```

### Split inherent impl

```rust
// broker/mailbox.rs
impl BrokerState {
    /// <verbatim doc comment moved from mod.rs, citations intact>
    pub(super) fn flush_mailbox_for_session(&mut self, session_id: &str, now: u64) {
        // ...body moved verbatim...
    }
}
```

### Final `mod.rs`

```rust
//! <lines 1-21 verbatim — the crate's broker overview, citations intact>

pub mod listener;
pub mod ratelimit;
pub mod routing;
pub mod runtime_claim;

mod conn;
mod dispatch;
mod extensions;
mod frame;
mod js;
mod lifecycle;
mod limits;
mod mailbox;
mod presence;
mod receipts;
mod send;
mod session;
mod state;

#[cfg(test)]
mod test_support;

pub use lifecycle::run;
```

`mod lifecycle;` stays private while `run` is re-exported publicly — the standard facade. This keeps
`crate::broker::run` resolving for both callers and the `[`crate::broker::run`]` doc link in
`listener.rs:66`.

### In-file tests

```rust
// bottom of broker/mailbox.rs
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::broker::test_support::{make_state, make_tx, payloads, register_named, send_frame};
    use serde_json::json;

    // ...moved test fns verbatim...
}
```

---

## Test redistribution

Tests move to the module they exercise. **No test is added, deleted, renamed, or edited** beyond its
`use` block.

### Deduplicate into `test_support.rs`

Both test modules define `make_state`, `make_tx` and `register`. Verified by extraction and diff:

- `make_state` — **byte-identical** in both (2130–2133 vs 2657–2662)
- `make_tx` — **byte-identical** in both (2257–2261 vs 2663–2667)
- `register` — identical token stream; the two copies differ **only in JSON literal line wrapping**
  (2262–2278 vs 2668–2680)

Take **one** copy of each into `test_support.rs`, along with the three helpers that exist once
already: `payloads` (2681–2692), `register_named` (2693–2715), `send_frame` (2716–2727). Use the
2668–2680 (compact) form of `register`. This is the only place where two source regions collapse to
one, and it is a safe dedup because the inputs are proven equivalent.

### Destination per test

| Test | Current lines | → |
|---|---|---|
| `the_tcp_endpoint_credential_gates_health_and_register_and_the_socket_endpoint_does_not` | 2134–2195 | `dispatch.rs` |
| `trusted_local_follows_the_bound_endpoint_not_the_platform` | 2196–2225 | `dispatch.rs` |
| `js_string_or_empty_matches_the_js_coercion` | 2226–2256 | `js.rs` |
| `session_infos_are_returned_in_join_order` | 2279–2313 | `state.rs` |
| `handle_unregister_evicts_oldest_unregistered_past_cap` | 2314–2362 | `session.rs` |
| `oversize_chunk_still_dispatches_frames_reassembled_earlier_in_the_same_chunk` | 2363–2406 | `conn.rs` |
| `apply_presence_context_matches_pis_tristate` | 2415–2450 | `presence.rs` |
| `handle_presence_rejects_non_number_context_but_accepts_null_and_numbers` | 2451–2495 | `presence.rs` |
| `a_non_owning_socket_s_malformed_presence_is_ignored_not_fatal` | 2496–2534 | `presence.rs` |
| `presence_carries_runtime_fallback_alias` | 2535–2577 | `presence.rs` |
| `a_blocking_ask_to_a_disconnected_peer_is_refused_with_the_not_queued_reason` | 2578–2626 | `send.rs` |
| `a_send_to_a_name_the_broker_has_never_seen_is_session_not_found` | 2627–2656 | `send.rs` |
| `mail_for_a_disconnected_peer_is_parked_and_flushed_on_re_register` | 2728–2780 | `mailbox.rs` |
| `a_relaunched_peer_inherits_mail_by_name_and_cwd_but_not_by_name_alone` | 2781–2827 | `mailbox.rs` |
| `mail_for_an_old_session_id_is_delivered_live_to_its_relaunched_identity` | 2828–2865 | `mailbox.rs` |
| `the_mailbox_cap_evicts_the_oldest_entry` | 2866–2896 | `mailbox.rs` |
| `parked_mail_and_the_disconnected_identity_both_expire_after_their_retention` | 2897–3009 | `mailbox.rs` |
| `a_disconnect_preserves_the_ask_edge_so_the_reply_still_lands_after_reconnect` | 3010–3061 | `send.rs` |
| `a_sender_can_cancel_both_parked_and_delivered_mail` | 3062–3138 | `receipts.rs` |
| `a_receipt_is_forwarded_to_the_original_sender` | 3139–3195 | `receipts.rs` |
| `a_supersede_is_validated_against_the_receipt_route_and_announced_before_the_replacement` | 3196–3255 | `receipts.rs` |
| `a_register_clears_the_pending_shutdown_so_a_later_disconnect_can_re_arm` | 3256–3291 | `lifecycle.rs` |

Note `mod presence_context_tests` currently holds mailbox, receipt, cancel and shutdown tests that
have nothing to do with presence — that misnaming is exactly the accumulation this task removes. The
name disappears; the 22 test fns survive one-for-one.

---

## Intra-doc links to re-path

Rustdoc resolves `[`Foo`]` in the module where the doc comment lands, so a link that moves with its
item may need a new path. There is a queued `CARGO_DOC_WARNINGS.md` task, so do not add new ones.

| Link | Count | Action |
|---|---|---|
| `[`run`]` | 5 (all in `mod.rs` module docs) | resolves via `pub use lifecycle::run;` — **no change** |
| `[`BrokerState::handle_frame`]` | 3 | `[`state::BrokerState::handle_frame`]` from `mod.rs`; `[`super::state::BrokerState::handle_frame`]` from a sibling |
| `[`BrokerState::flush_mailbox_for_session`]` | 2 | → `mailbox` path |
| `[`BrokerState::queue_mailbox_message`]` | 1 | → `mailbox` path |
| `[`BrokerState::unregistered`]`, `[`BrokerState::session_order`]` | 2 | stay within `state.rs` — no change |
| `[`process_frame_payload`]` | 1 | moves with it into `conn.rs` — no change |
| `[`js_string_or_empty`]`, `[`js_truthy_alias`]` | 2 | within `js.rs` — no change |
| `[`MAX_EXTENSIONS_PER_SESSION`]`, `[`DISCONNECTED_SESSION_RETENTION_MS`]` | 2 | → `limits::` path from the referencing module |
| `[`ConnectedSession`]`, `[`FrameOutcome`]`, `[`FrameResult::protocol_error`]` | 3 | → owning-module path |

Verify with `cargo doc -p cyrup-intercom --no-deps --document-private-items 2>&1 | grep -i 'broken\|unresolved'`.

---

## Execution order

Do it in this sequence so the crate compiles at every step and a mistake is bisectable.

0. **Baseline.** `cargo check -p cyrup-intercom && cargo test -p cyrup-intercom` and record the
   result. Anything already failing is out of scope and stays failing.
1. **Leaves first** — `limits.rs`, `js.rs`, `frame.rs`. No dependencies on anything else being moved.
   Compile.
2. **`state.rs`** — structs, `lock`, the 15 core methods. Compile.
3. **`mailbox.rs`**, then **`receipts.rs`**, **`extensions.rs`**, **`presence.rs`**, **`session.rs`**,
   **`send.rs`** — one module per commit-sized step, compiling after each.
4. **`dispatch.rs`** — `handle_frame` last of the state work, since it names every handler.
5. **`conn.rs`**, then **`lifecycle.rs`**.
6. **`test_support.rs`** + move the 22 tests into their destination modules; delete both old test
   modules.
7. Re-path the intra-doc links; gut `mod.rs` to the facade above.

Move code with `sed -n 'A,Bp'` extraction rather than retyping. Retyping is how citations get
dropped.

---

## Definition of done

- `crates/cyrup-intercom/src/broker/mod.rs` is a facade: module docs, `mod`/`pub mod` decls, and
  `pub use lifecycle::run;`. Under 60 lines.
- No file in `broker/` exceeds ~500 lines.
- `cargo build -p cyrup-intercom` and `cargo clippy -p cyrup-intercom --all-targets` are clean, with
  **no new warnings** versus the step-0 baseline.
- `cargo test -p cyrup-intercom` shows the **same 22 broker unit tests passing** as at baseline —
  same names, same count. No test written, deleted or renamed.
- `cargo build -p cyrup` still succeeds (`cyrup_intercom::broker::run` is reached from
  `crates/cyrup/src/intercom_broker_cmd.rs`).
- `cargo doc -p cyrup-intercom --no-deps --document-private-items` reports no broken intra-doc links.
- **Relocation proof** — the non-blank, non-comment code lines are a permutation of the original:

  ```sh
  git show HEAD:crates/cyrup-intercom/src/broker/mod.rs \
    | grep -vE '^\s*(//|$)' | sed 's/^\s*//' | sort > /tmp/before.txt
  cat crates/cyrup-intercom/src/broker/{mod,limits,js,frame,state,mailbox,dispatch,session,send,receipts,presence,extensions,conn,lifecycle,test_support}.rs \
    | grep -vE '^\s*(//|$)' | sed 's/^\s*//' | sort > /tmp/after.txt
  diff /tmp/before.txt /tmp/after.txt
  ```

  The diff must contain **only**: added `use`/`mod`/`pub use` lines, added `impl BrokerState {` +
  `}` wrappers, added `#[cfg(test)] mod tests {` + `#![allow(...)]` + `}` wrappers, `pub(super)`
  prefixes on moved items, and the removal of the two duplicated helper copies. **Any other
  difference is a rewrite and must be reverted.**
- No file outside `crates/cyrup-intercom/src/broker/` is modified.
