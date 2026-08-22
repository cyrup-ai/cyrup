---
stage: qa
status: needs-rework
updated: 2026-08-22 18:25
---

# Decompose Broker Mod Into Submodules — Second Header Rework

**QA rating: 8/10.** The correction pass fixed all nine targeted claims and touched nothing but
`//!` lines — verified per-file against the pre-pass snapshot. Two headers still assert something
false. One of them is **newly introduced by the correction itself**, which is the finding that
matters: the defect rate dropped ~4.5x but the error *class* survived a round that was explicitly
about eliminating it.

## Verified complete — do not redo

* **Header-only confinement.** Every changed line in the pass is a `//!` line; each file's non-header
  content is byte-identical to the pre-pass snapshot.
* **Relocation.** 47/47 regions still byte-identical to the original ranges, in order, contiguous.
* **Gates**, independently re-run: clippy **3**, `cargo doc` **20**, `cargo test --lib` **275**. The
  two new intra-doc links in `receipts.rs` resolve — broker doc warnings still number exactly two,
  both pre-existing.
* **Seven of the nine corrections verified true against source:** `limits.rs` (11 constants, 9 cited,
  `SHUTDOWN_DELAY_MS` at `:295`, `READ_BUF` uncited), `mailbox.rs` (0 handler calls; `state`,
  `session`, `send`, `receipts` all genuinely call inward), `js.rs` (0 `BrokerState`/`&self` refs),
  `presence.rs` (frame-driven, single broadcast site), `dispatch.rs` (cross-imports confirmed),
  `lifecycle.rs`, `test_support.rs` (exactly six importers).
* `receipts.rs`'s claim that `handle_message_receipt` **"answers every miss with a silent `break`"**
  is correct — its only `send_msg` is on the success path; every miss falls through to
  `FrameResult::cont()` with no frame sent.
* **`mailbox.rs` left whole**, size criterion restated against body lines. Accepted: body is 247, max
  body across `broker/` is `send.rs` at 353, all under 400.

## Outstanding

### 1. `receipts.rs:9` — new error, introduced by this pass

```
//! authorise it — and, like every handler, destroys the connection on a malformed frame.
```

`handle_presence` does **not**. Its ownership filter returns `FrameResult::cont()` before the
type-checks, so a non-owning socket's malformed `presence` is ignored rather than fatal — and there
is a regression test pinning exactly that, `a_non_owning_socket_s_malformed_presence_is_ignored_not_fatal` (ICOM-014),
whose comment records that the pre-fix ordering destroyed the connection and that the reconnect
ladder makes the takeover race a live path.

The comparison adds nothing the sentence needs. Drop the quantifier and state only what this handler
does:

```rust
//! authorise it — and destroys the connection on a malformed frame.
```

### 2. `session.rs:5-6` — wrong frame name, present since the original split

```
//! join-order insert, the mailbox flush for a returning identity, and the `registered` +
//! `presence_update` fan-out.
```

`handle_register` emits no `presence_update` at all — `grep -c PresenceUpdate session.rs` is **0**.
The two frames it sends are at `session.rs:130-131`:

```rust
send_msg(self_tx, &BrokerMessage::Registered { session_id: id.clone(), features: None });
self.broadcast(&BrokerMessage::SessionJoined { session: info }, Some(&id));
```

so the fan-out is `registered` (unicast to the registrant) + `session_joined` (broadcast to everyone
else). Replace with:

```rust
//! join-order insert, the mailbox flush for a returning identity, and the `registered` reply plus
//! the `session_joined` broadcast.
```

This one survived the original QA and the nine-defect audit because both checked `session.rs`'s
claim by grepping for the words in the prose (`takeover`, `MAX_SESSIONS`) rather than for the frame
constructors the sentence names.

## The root cause, and the rule that ends it

Every defect across all three rounds — nine in the first audit, two here — has one shape: **a
specific or a universal asserted without enumerating the cases**.

* universals: "only public item", "only outward calls", "never need to know about each other", "each
  concern's `mod tests`", "always replies", "like every handler"
* specifics: "`presence_update` fan-out", "the retentions by `receipts`", "`broker.ts:25-42`"

Prose review does not catch these; only enumeration does. Before this task closes, apply to every
surviving header claim:

1. **Universal quantifier** (`every`, `only`, `all`, `never`, `each`, `always`) → enumerate the set
   and count it, or delete the quantifier. Seven of the nine corrections needed exactly this.
2. **Named identifier** (a frame, constant, type, function, module) → `grep` for that identifier in
   the file the sentence describes. Item 2 above is a one-line grep away and it was missed twice.
3. If a claim cannot be reduced to a command, it does not belong in a `//!` header of this crate.

## Definition of done

* Both corrections applied exactly as drafted.
* Rules 1 and 2 applied to every remaining `//!` claim in all fourteen files, with the checking
  command run — not read.
* Gates unchanged: clippy **3**, `cargo doc` **20**, `cargo test -p cyrup-intercom --lib` **275**.
* **No line outside a `//!` block touched**; the 47-region relocation proof must still pass.
