---
stage: aug
status: done
updated: 2026-08-22 18:50
---

# Decompose Broker Mod Into Submodules — Final Header Pass

## Objective

Close out the `//!` header corrections. Three prior rounds each found errors the previous round
missed, because each round **sampled** claims and read them for plausibility. This pass ran the
enumeration mechanically over **all fourteen headers** instead, and the result is a closed set: five
items to change, and an explicit verified-clean list for everything else so the next round has
nothing left to discover.

Scope is `//!` blocks only. No `///` item doc, no code, no test.

---

## The audit, run rather than read

Two sweeps, both reproducible.

### Sweep A — wire frame names (rule 2)

`BrokerMessage` is `#[serde(tag = "type", rename_all = "snake_case")]`
([`transport/protocol.rs`](../../crates/cyrup-intercom/src/transport/protocol.rs)), so every
snake_case frame name in a header maps to exactly one variant. For each backticked snake_case token
in a header, check whether that variant appears in the same file's body:

```sh
# for each //! token `foo_bar` -> BrokerMessage::FooBar, grep the body of that same file
```

Result: **2 hits, 1 real.**

| File | Header says | Variant | In body | Verdict |
|---|---|---|---|---|
| `session.rs` | `presence_update` | `PresenceUpdate` | **no** | **defect — item 1** |
| `extensions.rs` | `registered` | `Registered` | no | **false positive** |

The `extensions.rs` hit is legitimate: its sentence is about what *cyrup as a whole* never
advertises, not about what this file emits. Verified true at the place that does emit it —
`session.rs:130` builds `BrokerMessage::Registered { .. , features: None }`, so nothing is
advertised. **This exposed a refinement to rule 2, carried into the rules below:** grep the file the
sentence *attributes the behaviour to*, which is not always the file the sentence is in.

### Sweep B — universal quantifiers (rule 1)

Every `every`/`only`/`all`/`never`/`each`/`always`/`the one` in a `//!` line of the fourteen files,
enumerated and counted. **17 claims checked, 4 flagged.**

| Claim | Enumeration | Verdict |
|---|---|---|
| `dispatch.rs:8` "the one place that names every handler" | 11/11 `handle_*` called only in `dispatch.rs` | ✅ |
| `dispatch.rs:11` "the routing decision is only ever made here" | `value.get("type")` appears only in `dispatch.rs` | ⚠️ **item 4** — true, but ambiguous |
| `frame.rs:7` "every handler module can depend on this plumbing" | 7/7 handler modules `use super::frame::` | ✅ |
| `js.rs:7` "no broker state involved at all" | 0 `BrokerState`/`&self` refs in `js.rs` | ✅ |
| `state.rs:5` "the bookkeeping every frame handler builds on" | 4/6 — `dispatch` and `extensions` call **no** `state.rs` method | ⚠️ **item 5** |
| `state.rs:8` "all of them are `impl BrokerState` blocks" | 5/5 | ✅ |
| `mailbox.rs:9` "Nothing here calls a frame handler" | 0 `self.handle_*` | ✅ |
| `mailbox.rs:9` "`state`, `session`, `send`, `receipts` all call inward" | 4/4 genuinely call mailbox methods | ✅ |
| `receipts.rs:6` "answers every miss with a silent `break`" | only `send_msg` is on the success path | ✅ |
| `receipts.rs:9` "like every handler, destroys the connection" | `handle_presence` returns `cont()` | ❌ **item 2** |
| `send.rs:7` "the only handler-to-handler call in the broker" | 1/1 | ✅ |
| `session.rs:6` "the one concern that owns arrival and departure" | departure also in `state.rs:255-262` | ❌ **item 3** |
| `lifecycle.rs:5` "only public item the module root contributes" | 4 sibling `pub mod`s export 8 items of their own | ✅ |
| `limits.rs:4` "Nine of the eleven are upstream's" | 11 constants, 9 cited | ✅ |
| `extensions.rs:4` "never advertises `EXTENSION_BUS_FEATURE`" | `features: None` at `session.rs:130` | ✅ |
| `extensions.rs:6` "each handler ports upstream's validation prefix and miss branch" | 3/3 | ✅ |
| `mod.rs:28` "each an `impl BrokerState` block" | 5/5 | ✅ |

---

## The five changes

### 1 + 3. `session.rs:4-7` — two errors in one sentence

```
//! [`BrokerState::handle_register`] is the crate's registration handshake — caps, takeover, the
//! join-order insert, the mailbox flush for a returning identity, and the `registered` +
//! `presence_update` fan-out. Split out of `broker/mod.rs` as the one concern that owns a session's
//! arrival and departure.
```

**Error 1 — wrong frame.** `grep -c PresenceUpdate session.rs` is **0**. The two frames at
`session.rs:130-131` are:

```rust
send_msg(self_tx, &BrokerMessage::Registered { session_id: id.clone(), features: None });
self.broadcast(&BrokerMessage::SessionJoined { session: info }, Some(&id));
```

**Error 2 — departure is not owned here.** A session leaves by two paths, and only one is in this
file:

| Path | Site | What it does |
|---|---|---|
| explicit `unregister` frame | `session.rs:159-161` | `remove_session` + `SessionLeft` broadcast |
| socket close | `state.rs:255-262` `on_connection_closed` | `remember_disconnected_session`, `remove_session`, `clear_message_receipt_routes_for_session`, `SessionLeft` broadcast |

Replace all four lines with:

```rust
//! [`BrokerState::handle_register`] is the crate's registration handshake — caps, takeover, the
//! join-order insert, the mailbox flush for a returning identity, and the `registered` reply plus
//! the `session_joined` broadcast. Split out of `broker/mod.rs` as the concern that owns the
//! `register`/`unregister`/`list` frames; a session can also leave without an `unregister` — that
//! path is `BrokerState::on_connection_closed` in `state`.
```

The added pointer is not decoration: the split is the reason the two departure paths now live in
different files, so the header is exactly where a reader needs to be told.

### 2. `receipts.rs:9` — drop the false generalization

```
//! authorise it — and, like every handler, destroys the connection on a malformed frame.
```

`handle_presence`'s ownership filter returns `FrameResult::cont()` before its type-checks, so a
non-owning socket's malformed `presence` is ignored rather than fatal —
`a_non_owning_socket_s_malformed_presence_is_ignored_not_fatal` (ICOM-014) pins it, and its comment
records that the pre-fix ordering destroyed the connection on a live takeover race. Replace with:

```rust
//! authorise it — and destroys the connection on a malformed frame.
```

### 4. `dispatch.rs:11` — make the true claim the stated one

```
//! `receipts::MessageReceiptRoute` — but the routing decision is only ever made here.
```

"Routing decision" reads as message routing, which `handle_send` also does. What is actually unique
here is the frame-type switch — `value.get("type")` appears in `dispatch.rs` and nowhere else.
Replace with:

```rust
//! `receipts::MessageReceiptRoute` — but the frame-type dispatch happens only here.
```

### 5. `state.rs:4-5` — delete a quantifier that does not survive enumeration

```
//! [`BrokerState`] is upstream's `IntercomBroker` field block; the methods here are the bookkeeping
//! every frame handler builds on — connection tracking, ...
```

Four of the six handler modules call a `state.rs` method; `dispatch` and `extensions` call none —
they read `BrokerState` fields directly (`self.endpoint_state_id`, `self.sessions`). Per rule 1,
delete the quantifier rather than defend it:

```rust
//! [`BrokerState`] is upstream's `IntercomBroker` field block; the methods here are the shared
//! bookkeeping the frame handlers build on — connection tracking, ...
```

---

## The rules, in final form

Applied to every header claim; carry them forward for any future `//!` edit in this crate.

1. **Universal quantifier** (`every`, `only`, `all`, `never`, `each`, `always`, `the one`) →
   enumerate the set with a command and count it. If the count is not total, delete the quantifier.
   Do not soften it to "mostly" — an unquantified sentence is honest, a hedged one is noise.
2. **Named identifier** (frame, constant, type, function, module) → grep it in the file **the
   sentence attributes the behaviour to**, which is not always the file the sentence sits in. A
   cross-module reference is legitimate; verify it at its real site, as `extensions.rs`'s
   `registered` was verified at `session.rs:130`.
3. **A claim that cannot be reduced to a command does not belong in a `//!` header of this crate.**

---

## Definition of done

* All five changes applied exactly as drafted.
* Sweeps A and B re-run after the edits: **zero** rule-1 and rule-2 violations across all fourteen
  files.
* Gates unchanged: `cargo clippy -p cyrup-intercom --all-targets` → **3**;
  `cargo doc -p cyrup-intercom --no-deps --document-private-items` → **20**;
  `cargo test -p cyrup-intercom --lib` → **275**.
* **No line outside a `//!` block touched**; the 47-region relocation proof still passes.
