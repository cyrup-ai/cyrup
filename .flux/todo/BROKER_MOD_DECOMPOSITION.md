---
stage: aug
status: done
updated: 2026-08-22 17:35
---

# Decompose Broker Mod Into Submodules — Header Correction Pass

## Objective

The decomposition itself is landed and verified (commit `25ba657`). This pass fixes the `//!` module
headers written during that commit. QA flagged five defects; a full re-audit of all fourteen headers
found **nine false or misleading claims across eight files** — three of them the QA pass did not
catch, and two of those misdescribe runtime behaviour rather than layout.

Every claim below was checked against the source with a command; the replacement text is the checked
version. **This is the whole point of the pass:** the previous round wrote header prose from memory,
and this crate's entire review affordance is docs a reviewer can trust against `broker.ts`.

Scope is `//!` header blocks only. No `///` item doc, no code, no test.

---

## Verified complete — do not touch

* **Relocation.** 47/47 moved regions byte-identical to their original ranges, in order, contiguous,
  modulo `pub(super)` prefixes and four sanctioned intra-doc re-paths.
* **Public API.** Exactly one bare `pub` item across the fourteen new files —
  [`lifecycle.rs:100`](../../crates/cyrup-intercom/src/broker/lifecycle.rs) `pub async fn run()`.
* **Baselines.** clippy 3 → 3, `cargo doc` 20 → 20, `cargo test --lib` 275 passing, 41 broker tests
  by identical leaf name, `cargo build -p cyrup` green.
* **Positional prose** (`above`/`below`) in relocated `///` comments: every referent stayed in the
  same file in the same order.

## Explicitly out of scope

* `handle_presence` has no `///` doc comment. **This is pre-existing** — `mod.rs.orig:1560` is a bare
  `fn handle_presence(` preceded by a blank line. Do not add one; it is not this task.
* The three pre-existing clippy warnings (`too_many_arguments` on `register_named`, two
  `doc_lazy_continuation`). They were present before the split and must stay at 3.

---

## The nine corrections

### 1. `limits.rs:1` — two false claims in one header

```
//! The broker's ported module-level constants (`broker.ts:25-42`).
//! Every value here is upstream's, named after upstream's constant and carrying its citation. They
//! are gathered in one file because they are consumed across the whole broker — the caps by
//! `state`/`session`, the retentions by `mailbox`/`receipts`, the two delays by `conn`/`lifecycle` —
```

Three defects. Verified constant-by-constant:

| Constant | Own citation | In `25-42`? |
|---|---|---|
| `MAX_SESSIONS` | `broker.ts:25` | yes |
| `MAX_UNREGISTERED_CONNECTIONS` | `broker.ts:26` | yes |
| `REGISTRATION_TIMEOUT_MS` | `broker.ts:27` | yes |
| `PRESENCE_HEARTBEAT_MS` | `broker.ts:30` | yes |
| `SHUTDOWN_DELAY_MS` | `broker.ts:295` | **no** |
| `MESSAGE_RECEIPT_ROUTE_RETENTION_MS` | `v0.10.1 :39` | yes |
| `DISCONNECTED_SESSION_RETENTION_MS` | `v0.10.1 :40` | yes |
| `MAILBOX_MESSAGE_RETENTION_MS` | `v0.10.1 :41` | yes |
| `MAX_MAILBOX_MESSAGES` | `v0.10.1 :42` | yes |
| `READ_BUF` | **none** — "implementation detail" | **no** |
| `MAX_EXTENSIONS_PER_SESSION` | `v0.9.2 :35` | yes |

* `(broker.ts:25-42)` excludes `SHUTDOWN_DELAY_MS` and `READ_BUF`.
* "Every value here is upstream's, named after upstream's constant and carrying its citation" is
  false for `READ_BUF` on all three counts.
* The consumer list is wrong and incomplete: `receipts` consumes **no** constant;
  `MESSAGE_RECEIPT_ROUTE_RETENTION_MS` goes to `state`; `presence` and `extensions` are missing.

Replace lines 1–6 with:

```rust
//! The broker's ported module-level constants (`broker.ts:25-42`), plus the two values that have no
//! named upstream counterpart.
//!
//! Nine of the eleven are upstream's, named after upstream's constant and carrying its citation. The
//! exceptions are called out where they are defined: `SHUTDOWN_DELAY_MS` is upstream's 5 s delay read
//! off `broker.ts:295` rather than a named constant, and `READ_BUF` is a cyrup reader detail with no
//! upstream counterpart at all. They are gathered in one file because reading them together is how a
//! reviewer checks the port against `broker.ts` in one pass.
```

Note the deliberate omission: **no consumer enumeration.** A per-module list is exactly what rotted
here, and "gathered so a reviewer can check them in one pass" already carries the reason.

### 2. `mailbox.rs:9` — states a dependency rule the file breaks

```
//! bookkeeping they do not depend on. Their only outward calls are to `state` primitives.
```

Contradicted by this file's own `use` block three lines below. Verified outward calls from
`mailbox.rs`: `routing::find_session_ids` ×1, `js::js_truthy_alias` ×2, `frame::send_msg` ×1,
`protocol::now_ms` ×1, plus `receipts::MessageReceiptRoute` and four `protocol` types by name.

The property that *is* true and that justifies the split: **nothing in `mailbox` calls a frame
handler.** Replace lines 8–9 with:

```rust
//! Split out of `broker/mod.rs`, where these eight methods sat interleaved with the connection
//! bookkeeping. Nothing here calls a frame handler — `session` and `send` call inward, never the
//! reverse — so `mailbox` is a leaf of the handler layer, reaching only `state` primitives,
//! `routing::find_session_ids`, `js::js_truthy_alias` and `frame::send_msg`.
```

### 3. `lifecycle.rs:4-5` — overstates the public surface

```
//! [`run`] is the `cyrup __intercom-broker` entrypoint, re-exported as `crate::broker::run` — the
//! only public item the broker has.
```

`broker` also exposes `pub mod listener` (2 public items), `ratelimit` (3), `routing` (2),
`runtime_claim` (1) — 8 more. Replace with:

```rust
//! [`run`] is the `cyrup __intercom-broker` entrypoint, re-exported as `crate::broker::run` — the
//! only public item the module root itself contributes; the four `pub mod` siblings export their own.
```

### 4. `js.rs:7` — imprecise signature claim

```
//! they are pure functions over `serde_json::Value` with no broker state involved at all.
```

Four of the five take `&serde_json::Value` / `Option<&serde_json::Value>` / `&serde_json::Number`;
`js_truthy_alias` takes `Option<bool>`. The load-bearing half is "no broker state". Replace with:

```rust
//! they are pure functions with no broker state involved at all.
```

### 5. `conn.rs:5` — link lands the reader in the wrong file

```
//! token, and dispatches each to [`super::state::BrokerState::handle_frame`] while honoring the 1 s
```

The link resolves — rustdoc attaches inherent methods to the type's page regardless of which file the
`impl` block is in — but `handle_frame`'s source is in `dispatch.rs`, so a reader following the path
opens `state.rs` and does not find it. **There is no path that points at `dispatch`**
(`super::dispatch::BrokerState` does not resolve; `BrokerState` is not in that module's namespace), so
the fix is prose, keeping the link that resolves:

```rust
//! token, and dispatches each to [`super::state::BrokerState::handle_frame`] — the switch itself
//! lives in `super::dispatch` — while honoring the 1 s
```

### 6. `receipts.rs:5-6` — wrong about one of its two handlers

```
//! to the original sender and so that sender can later `cancel` or `supersede` it. Both handlers
//! answer a miss with a silent `break` rather than an error frame, exactly as upstream does.
```

False for `handle_cancel_message`, which replies on every path — verified in the function body:
`send_msg(self_tx, &BrokerMessage::Delivered { .. })` on success and
`send_msg(self_tx, &BrokerMessage::DeliveryFailed { .. })` when the route does not authorise the
cancel. Only `handle_message_receipt` uses the silent-miss pattern. Replace with:

```rust
//! to the original sender and so that sender can later `cancel` or `supersede` it.
//! [`BrokerState::handle_message_receipt`] answers every miss with a silent `break`, exactly as
//! upstream does; [`BrokerState::handle_cancel_message`] always replies instead — `delivered` when
//! the cancel lands, `delivery_failed` with upstream's reason when the route does not authorise it.
```

### 7. `presence.rs:4-5` — inverts the coalescing rule

```
//! [`BrokerState::handle_presence`] coalesces presence into at most one broadcast per
//! [`super::limits::PRESENCE_HEARTBEAT_MS`];
```

The condition at `presence.rs:101-102` is:

```rust
let should_broadcast =
    changed || now.saturating_sub(session.last_presence_broadcast_at) >= PRESENCE_HEARTBEAT_MS;
```

`changed ||` — a change broadcasts **immediately**, whatever the elapsed time. So the heartbeat is a
floor, not a ceiling: "at most one per interval" is the opposite of what this does. Replace with:

```rust
//! [`BrokerState::handle_presence`] broadcasts on any change, and at least once per
//! [`super::limits::PRESENCE_HEARTBEAT_MS`] even when nothing changed;
```

### 8. `dispatch.rs:8-9` — overclaims handler independence

```
//! Split out of `broker/mod.rs` as the one place that names every handler, so the handler modules
//! themselves never need to know about each other.
```

They do know about each other, just not for dispatch. Verified cross-imports:
`session.rs:13 use super::extensions::extensions_field_is_valid;`,
`send.rs:17` and `mailbox.rs:18 use super::receipts::MessageReceiptRoute;`. Replace with:

```rust
//! Split out of `broker/mod.rs` as the one place that names every handler, so no handler module ever
//! dispatches to another. They still share types and validators across module lines — `session`
//! imports `extensions::extensions_field_is_valid`, `send` and `mailbox` name
//! `receipts::MessageReceiptRoute` — but the routing decision is only ever made here.
```

### 9. `test_support.rs:5` — "each" is six of ten

```
//! they are kept once here and imported by each concern's `mod tests`.
```

Verified: `state`, `session`, `send`, `receipts`, `mailbox`, `conn` import it; `js`, `dispatch`,
`presence`, `lifecycle` do not. Replace with:

```rust
//! they are kept once here and imported by the six `mod tests` that need them — `state`, `session`,
//! `send`, `receipts`, `mailbox` and `conn`.
```

---

## Item 10 — the `mailbox.rs` size ceiling: raise it, and measure the body

QA left this call to a human. The data says the ceiling measures the wrong thing:

| file | total | body | tests |
|---|---|---|---|
| `mailbox.rs` | **532** | 247 | 285 |
| `send.rs` | 488 | **353** | 135 |
| `receipts.rs` | 352 | 154 | 198 |
| `lifecycle.rs` | 341 | 293 | 48 |
| `state.rs` | 309 | 269 | 40 |

`mailbox.rs` has the **7th largest body of 15 files** at 247 lines. Its total is inflated purely by
having the largest test suite in the crate (285 lines, 5 tests, one of them 113) — because offline
delivery has the most behaviour to pin down. The largest actual body is `send.rs` at 353.

Splitting mailbox's tests across two files to satisfy a total-line count would violate the original
task's own acceptance criterion:

> Each new submodule groups a single, nameable concern; module boundaries follow how the code
> actually clusters, **not an arbitrary line-count target**.

**Required resolution: keep `mailbox.rs` whole** and restate the criterion against non-test body
lines, where every file is already comfortably inside it. Do not create a `mailbox_retention`
sibling. Record the decision in this task's closing note, not in a code comment.

---

## Definition of done

* All nine header corrections applied exactly as drafted above.
* `mailbox.rs` left whole; the size criterion restated as **body ≤ 400 lines** (current max:
  `send.rs` at 353), which every file satisfies.
* Every surviving header claim is checkable against the file it heads. The standing rule for this
  crate: **if a header asserts a relationship, the assertion must be derivable from that file's own
  `use` block, its function bodies, or a cited `broker.ts` line.**
* Regression gates unchanged from baseline:
  * `cargo clippy -p cyrup-intercom --all-targets` → **3** warnings
  * `cargo doc -p cyrup-intercom --no-deps --document-private-items` → **20** warnings
  * `cargo test -p cyrup-intercom --lib` → **275** passing
* **No line outside a `//!` block is touched.** The 47-region relocation proof must still pass
  byte-for-byte:

  ```sh
  # regions are unchanged if only //! lines differ from the landed commit
  ```
