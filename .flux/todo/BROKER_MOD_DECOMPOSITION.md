---
stage: qa
status: needs-rework
updated: 2026-08-22 17:05
---

# Decompose Broker Mod Into Submodules — QA Rework

**QA rating: 8/10.** The decomposition itself is complete and verified: `broker/mod.rs` is a
59-line facade, the 14 new modules compile clean, and the relocation is provably faithful. What
remains is entirely in the `//!` module headers written during exec — four of them assert things a
reader can disprove from the same file — plus one stated size criterion that is missed.

Nothing below requires touching relocated code. Do not re-run the split.

## Verified complete — do not redo

* **Order-preserving relocation.** All **47** moved regions are byte-identical to their original
  ranges, in order, contiguous, modulo only `pub(super) ` prefixes and four sanctioned intra-doc
  re-paths. (Exec proved this with a *sorted* line diff, which cannot detect reordering inside a
  function; QA re-verified order-sensitively per region.)
* **Public API unchanged.** Exactly one bare `pub` item across all 14 new files —
  `lifecycle.rs:100 pub async fn run()`, re-exported as `crate::broker::run`. New modules are
  private `mod`; the four pre-existing siblings stay `pub mod`.
* **Warning parity.** clippy 3 → 3, `cargo doc` 20 → 20, both measured against the original file by
  temporarily restoring it. Only 2 doc warnings point into `broker/`: `mod.rs:19` (the re-pathed
  baseline warning) and `runtime_claim.rs:37` (pre-existing, untouched).
* **Tests.** 275 lib tests pass; 41 broker tests with leaf names identical to baseline; the 22 from
  `mod.rs` land in the intended modules (mailbox 5, presence 4, send 3, receipts 3, dispatch 2,
  state/session/conn/js/lifecycle 1 each).
* **`cargo build -p cyrup`** succeeds — the binary's `cyrup_intercom::broker::run` call still resolves.
* **Scope.** Only the 15 files under `crates/cyrup-intercom/src/broker/` changed.
* **Positional prose** (`above`/`below`) in relocated doc comments was audited: every referent stayed
  in the same file in the same order. No action needed.

## Outstanding

### 1. `mailbox.rs:9` — header states a dependency rule the file breaks

> `//! bookkeeping they do not depend on. Their only outward calls are to `state` primitives.`

False, and contradicted by this file's own `use` block three lines below: `mailbox` also calls
`frame::send_msg`, `js::js_truthy_alias`, `routing::find_session_ids`, and names
`receipts::MessageReceiptRoute` plus four `crate::transport::protocol` items.

Rewrite to say what is actually true and load-bearing: these eight methods sit **below** the frame
handlers — nothing in `mailbox` calls a `handle_*`, so the module is a leaf of the handler layer.
That is the real property, and it is what justifies the split.

### 2. `limits.rs:5` — consumer enumeration is wrong in one arm and incomplete

> `//! `state`/`session`, the retentions by `mailbox`/`receipts`, the two delays by `conn`/`lifecycle` —`

`receipts` consumes **no** constant at all; `MESSAGE_RECEIPT_ROUTE_RETENTION_MS` is used by `state`.
The list also omits `presence` and `extensions`. Actual map:

| Constant | Consumer |
|---|---|
| `MAX_SESSIONS` | `session` |
| `MAX_UNREGISTERED_CONNECTIONS` | `session`, `state` |
| `REGISTRATION_TIMEOUT_MS`, `READ_BUF` | `conn` |
| `PRESENCE_HEARTBEAT_MS` | `presence` |
| `SHUTDOWN_DELAY_MS` | `lifecycle` |
| `MESSAGE_RECEIPT_ROUTE_RETENTION_MS` | `state` |
| `DISCONNECTED_SESSION_RETENTION_MS`, `MAX_MAILBOX_MESSAGES` | `mailbox` |
| `MAILBOX_MESSAGE_RETENTION_MS` | `mailbox`, `lifecycle` |
| `MAX_EXTENSIONS_PER_SESSION` | `extensions` |

Either correct the enumeration or drop it — "consumed across the whole broker" already carries the
point without a list that can rot.

### 3. `lifecycle.rs:4-5` — overstates the public surface

> `//! ... re-exported as `crate::broker::run` — the //! only public item the broker has.`

`broker` also exposes `pub mod listener`, `ratelimit`, `routing` and `runtime_claim`, which export 8
public items between them. Narrow the claim to what is true and worth saying: `run` is the only
public item the **module root** contributes.

### 4. `js.rs:7` — imprecise signature claim

> `//! they are pure functions over `serde_json::Value` with no broker state involved at all.`

Four of the five are; `js_truthy_alias` takes `Option<bool>`. The "no broker state" half is the part
that justifies the split — keep it, and drop or widen the `serde_json::Value` half.

### 5. `conn.rs:5` — link sends the reader to the wrong file

> `//! token, and dispatches each to [`super::state::BrokerState::handle_frame`] ...`

The path resolves (rustdoc attaches inherent methods to the type), but `handle_frame`'s source lives
in `dispatch.rs`, so a reader following the reference lands in `state.rs` and does not find it. Point
at `dispatch` in the prose, keeping whatever link form rustdoc resolves without a new warning.

### 6. `mailbox.rs` is 532 lines against the stated "~500" ceiling

Cohesive (the mailbox concern plus its 5 tests, one of which is 113 lines), so this may be accepted —
but it is a criterion the task set and did not meet, and the call belongs to a human, not to the
implementor. Either raise the ceiling in the criterion deliberately, or move the two
retention/expiry tests to a `mailbox_retention` sibling. **Do not** split the eight methods
themselves; they are one concern.

## Definition of done for this rework

- Headers 1–5 corrected; each remaining claim checkable against the file it describes.
- Item 6 explicitly resolved — ceiling raised with a reason, or the file brought under it.
- `cargo clippy -p cyrup-intercom --all-targets` still 3 warnings;
  `cargo doc -p cyrup-intercom --no-deps --document-private-items` still 20.
- `cargo test -p cyrup-intercom --lib` still 275 passing.
- **No `.rs` line outside a `//!` header block is touched.** The 47-region relocation proof must
  still pass unchanged.
