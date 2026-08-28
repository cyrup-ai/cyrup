---
stage: aug
status: done
updated: 2026-08-27 22:33
---

# Persist pending blocking-ask records for observability

> **Upstream parity gap.** `cyrup-intercom` is a port of `pi-intercom` **v0.9.2**; upstream is now
> **v0.12.0** (`ef95f19`, 2026-08-22) at [`nicobailon/pi-intercom`](https://github.com/nicobailon/pi-intercom).
> Reference checkout: [`./tmp/pi-intercom`](../../tmp/pi-intercom). Gap analysis:
> `docs/gap-analysis/11-cyrup-intercom.md` — **ICOM-057**.

**Observability only.** Nothing in this task may change which frame the broker routes, which session
receives it, or how a reply resolves. Every line added sits *beside* an existing
`ask_edges` mutation and reads nothing back on the delivery path.

---

## 1. What upstream does

`d69854d` (v0.11.0, issue #104) makes the **broker** write one JSON file per *delivered* blocking
ask into `<intercomDir>/pending-asks/`, and unlink it at the same instant it drops the in-memory ask
edge. The whole feature is 106 lines in [`broker/broker.ts`](../../tmp/pi-intercom/broker/broker.ts);
v0.12.0 kept it unchanged apart from adding a `scopeId` hash to the filename.

### The record

[`broker/broker.ts:99-107`](../../tmp/pi-intercom/broker/broker.ts) (v0.12.0):

```ts
interface PendingAskRecord {
  askId: string;
  messageId: string;
  asker: { sessionId: string; name: string | null };
  target: { sessionId: string; name: string | null };
  question: string;
  createdAt: number;
  expiresAt: number;
}
```

### Where it lives, and how it is written

`PENDING_ASKS_DIR = join(INTERCOM_DIR, "pending-asks")` ([`:30`](../../tmp/pi-intercom/broker/broker.ts)),
created at the same `0o700` as the intercom dir itself and re-`chmod`ed on every touch
([`:195-200`](../../tmp/pi-intercom/broker/broker.ts)):

```ts
function pendingAskRecordPath(messageId: string): string {
  return join(PENDING_ASKS_DIR, `${encodeURIComponent(messageId)}.json`);
}

function ensurePendingAskRecordDir(): void {
  mkdirSync(PENDING_ASKS_DIR, { recursive: true, mode: INTERCOM_DIR_MODE });
  if (process.platform !== "win32") {
    chmodSync(PENDING_ASKS_DIR, INTERCOM_DIR_MODE);
  }
}
```

The write is the last thing before the ask edge is inserted, on the **live-delivery path only**
([`:1185-1199`](../../tmp/pi-intercom/broker/broker.ts)):

```ts
  private writePendingAskRecord(message: Message, from: ConnectedSession, target: SessionInfo, createdAt: number): void {
    ensurePendingAskRecordDir();
    const record: PendingAskRecord = {
      askId: message.id,
      messageId: message.id,
      asker: { sessionId: from.info.id, name: from.info.name ?? null },
      target: { sessionId: target.id, name: target.name ?? null },
      question: message.content.text,
      createdAt,
      expiresAt: createdAt + this.askTimeoutMs,
    };
    const filePath = scopedPendingAskRecordPath(from.scopeId, message.id);
    writeFileSync(filePath, `${JSON.stringify(record, null, 2)}\n`, { mode: INTERCOM_RUNTIME_FILE_MODE });
    restrictIntercomRuntimeFile(filePath);
  }
```

called from the `expectsReply` branch of `send` ([`:684`](../../tmp/pi-intercom/broker/broker.ts)),
paired one-for-one with the edge insert:

```ts
            this.writePendingAskRecord(message, fromSession, target.info, brokerReceivedAt);
            this.askEdges.set(message.id, { from: currentKey, to: target.key, ..., createdAt: brokerReceivedAt });
```

`createdAt` is `brokerReceivedAt`, not `Date.now()` — the same commit retargeted the edge's own
`createdAt` to it in the same line, so the record and the edge expire together.

### Removal and pruning

[`:1201-1229`](../../tmp/pi-intercom/broker/broker.ts):

```ts
  private removePendingAskRecord(messageId: string, scopeId?: string): void {
    try {
      unlinkSync(scopedPendingAskRecordPath(scopeId, messageId));
    } catch (error) {
      if (!isRecord(error) || error.code !== "ENOENT") {
        throw error;
      }
    }
  }

  private prunePendingAskRecords(now = Date.now()): void {
    ensurePendingAskRecordDir();
    for (const entry of readdirSync(PENDING_ASKS_DIR, { withFileTypes: true })) {
      if (!entry.isFile() || !entry.name.endsWith(".json")) {
        continue;
      }
      const filePath = join(PENDING_ASKS_DIR, entry.name);
      let parsed: unknown;
      try {
        parsed = JSON.parse(readFileSync(filePath, "utf-8"));
      } catch {
        unlinkSync(filePath);
        continue;
      }
      if (!isPendingAskRecord(parsed) || now > parsed.expiresAt) {
        unlinkSync(filePath);
      }
    }
  }
```

`isPendingAskRecord` ([`:175-189`](../../tmp/pi-intercom/broker/broker.ts)) is a total structural
validator — every key present and correctly typed, `name` either a string or an explicit `null`,
and `expiresAt >= createdAt`; anything else is deleted rather than kept.

`prunePendingAskRecords` runs at broker construction, right after `assertNoLiveBroker`
([`:220-222`](../../tmp/pi-intercom/broker/broker.ts)), and then at the head of `pruneAskEdges`
([`:1166-1175`](../../tmp/pi-intercom/broker/broker.ts)) — i.e. on every `send` frame.

### Upstream's `removePendingAskRecord` call sites

Every one of them is glued to an existing `askEdges.delete`. There are eight:
[`:718`](../../tmp/pi-intercom/broker/broker.ts) and [`:773`](../../tmp/pi-intercom/broker/broker.ts)
(reply consumed the edge, live and mailbox arms), [`:827`](../../tmp/pi-intercom/broker/broker.ts)
and [`:854`](../../tmp/pi-intercom/broker/broker.ts) (`cancel_message`, parked and live arms),
[`:872`](../../tmp/pi-intercom/broker/broker.ts) (`cancel_ask`),
[`:1002`](../../tmp/pi-intercom/broker/broker.ts) and [`:1018`](../../tmp/pi-intercom/broker/broker.ts)
(mailbox expiry and cap eviction), [`:1171`](../../tmp/pi-intercom/broker/broker.ts) (ask-edge
timeout prune) and [`:1180`](../../tmp/pi-intercom/broker/broker.ts) (identity takeover).

---

## 2. Where this goes in the port — and where it does **not**

**It goes in the broker** — [`crates/cyrup-intercom/src/broker/`](../../crates/cyrup-intercom/src/broker),
the standalone `cyrup __intercom-broker` process
([`broker/mod.rs:1-11`](../../crates/cyrup-intercom/src/broker/mod.rs)), which is already a 1:1 port
of `broker.ts` and already carries upstream's complete ask-edge lifecycle. This is the required
placement, for four reasons that are not stylistic:

1. **The broker is the only holder of the authoritative pending-ask set.** `ask_edges`
   ([`broker/state.rs:50`](../../crates/cyrup-intercom/src/broker/state.rs)) is what decides whether
   a reply is legal; a record derived from anything else can disagree with it.
2. **Only the broker has both parties.** The record needs the asker's *and* the target's
   `SessionInfo` (`sessionId` + `name`). `handle_send` has both in hand
   ([`send.rs:99-104`](../../crates/cyrup-intercom/src/broker/send.rs) and
   `self.sessions.get(&target_id)`); the asking session knows only the string it typed.
3. **Only the broker owns `expiresAt`.** `ask_timeout_ms`
   ([`state.rs:69`](../../crates/cyrup-intercom/src/broker/state.rs)) is the prune deadline the edge
   actually dies on.
4. **Single writer, and a startup sweep.** Exactly one broker owns `<intercomDir>` at a time
   (`runtime_claim::assert_no_live_broker`,
   [`lifecycle.rs:132`](../../crates/cyrup-intercom/src/broker/lifecycle.rs)), so no two processes
   race on the directory — and the startup prune is the only thing that can reclaim records left by
   a `SIGKILL`ed predecessor. A session-side writer has neither property.

**These files are deliberately NOT modified**, and that is how "no change to ask delivery or reply
resolution" is guaranteed by construction rather than by review:

- [`session_state.rs`](../../crates/cyrup-intercom/src/session_state.rs) — `ask_and_wait` (`:683`),
  `ask_and_wait_with_reply_to` (`:730`), the `tokio::select!` at `:793-833`.
- [`reply_tracker.rs`](../../crates/cyrup-intercom/src/reply_tracker.rs) — the outbound single-slot
  waiter (`register` `:310`, `clear_matching` `:371`) and the *inbound* pending list
  (`list_pending` `:258`), which is a different thing entirely: it is what
  [`tools/intercom/pending.rs`](../../crates/cyrup-intercom/src/tools/intercom/pending.rs) prints,
  and it holds asks this session has *received*, not asks it has *issued*.
- [`tools/intercom/ask.rs`](../../crates/cyrup-intercom/src/tools/intercom/ask.rs),
  [`cancel.rs`](../../crates/cyrup-intercom/src/tools/intercom/cancel.rs),
  [`pending.rs`](../../crates/cyrup-intercom/src/tools/intercom/pending.rs).

The `intercom_received` audit entry at
[`ask.rs:116-129`](../../crates/cyrup-intercom/src/tools/intercom/ask.rs) is **not** the vehicle for
this record and must not be extended into one: `HostServices::append_entry` is an append-only
transcript write, so it can record that an ask *happened* but can never satisfy "the record clears on
all three exits". What is reused from it is its **failure policy** — see §3.

---

## 3. What already exists in the port and must be reused

Nothing on this list may be re-implemented, duplicated, or worked around.

| Need | Existing code — reuse verbatim |
| --- | --- |
| Where on disk this crate may write | [`paths::agent_dir_path`](../../crates/cyrup-intercom/src/paths.rs) (`:53`) → [`paths::intercom_dir_path`](../../crates/cyrup-intercom/src/paths.rs) (`:95`). The record dir is `<intercomDir>/pending-asks`. **No new root, no new env var, no `dirs`-style lookup.** |
| `mkdir -p` at `0o700` + re-chmod | [`paths::ensure_intercom_runtime_dir`](../../crates/cyrup-intercom/src/paths.rs) (`:126`) — this *is* upstream's `ensurePendingAskRecordDir` body, already ported and already Windows-guarded. |
| `chmod 0o600` on the file | [`paths::restrict_intercom_runtime_file`](../../crates/cyrup-intercom/src/paths.rs) (`:146`). |
| The two modes | `paths::INTERCOM_DIR_MODE` (`:31`), `paths::INTERCOM_RUNTIME_FILE_MODE` (`:33`). |
| Write-then-restrict ordering | [`lifecycle.rs:186-187`](../../crates/cyrup-intercom/src/broker/lifecycle.rs) (`broker.pid`) and `:184-185` (`broker.port.json`) — `std::fs::write` then `restrict_intercom_runtime_file`, the port's established shape. |
| Where a broker constant goes | [`broker/limits.rs`](../../crates/cyrup-intercom/src/broker/limits.rs) — all eleven ported constants live in one file so a reviewer checks them against `broker.ts` in one pass. |
| Cap-then-evict-oldest | [`mailbox.rs:92-99`](../../crates/cyrup-intercom/src/broker/mailbox.rs) (`queue_mailbox_message` / `MAX_MAILBOX_MESSAGES`) — the bound in §5 is the same shape. |
| Collect-then-act instead of side effects inside `retain` | [`mailbox.rs:63-79`](../../crates/cyrup-intercom/src/broker/mailbox.rs) (`prune_mailbox_messages`). |
| "Real `run()` supplies what unit state cannot" builder | [`state.rs:132-136`](../../crates/cyrup-intercom/src/broker/state.rs) `with_listen_endpoint`, chained at [`lifecycle.rs:197-198`](../../crates/cyrup-intercom/src/broker/lifecycle.rs). The record store is wired the same way. |
| Best-effort-I/O failure policy | [`ask.rs:109-111`](../../crates/cyrup-intercom/src/tools/intercom/ask.rs) — `tracing::warn!(error = %e, …, "intercom: failed to append audit entry")` and carry on. An observability write must never fail an ask the broker has already routed. |
| Ask timeout | `BrokerState::ask_timeout_ms` ([`state.rs:69`](../../crates/cyrup-intercom/src/broker/state.rs)), from [`config::ask_timeout_ms`](../../crates/cyrup-intercom/src/config.rs) (`:184`, default 10 min at `:15`). |
| camelCase JSON | `#[serde(rename_all = "camelCase")]`, as on every type in [`transport/protocol.rs`](../../crates/cyrup-intercom/src/transport/protocol.rs). |

**Scope is out of scope.** `grep -rn 'scope_id\|scopeId' --include=*.rs crates/cyrup-intercom` returns
nothing — the port has no `scopeId` at any layer. Port the `d69854d` (unscoped) filename form.
`scopedPendingAskRecordPath` belongs to whichever future item lands scoping.

---

## 4. The three exits — the exact existing code paths

Every one already exists, already fires on the right condition, and already mutates `ask_edges`.
The work is one `self.pending_asks.remove(...)` beside each.

### Exit 1 — reply

| Port | Upstream |
| --- | --- |
| [`broker/send.rs:192-194`](../../crates/cyrup-intercom/src/broker/send.rs) — `handle_send`, live target: `if let Some(rt) = &message.reply_to { self.ask_edges.remove(rt); }` | [`broker.ts:717-719`](../../tmp/pi-intercom/broker/broker.ts) |
| [`broker/send.rs:325-327`](../../crates/cyrup-intercom/src/broker/send.rs) — `handle_send_to_disconnected`, mailbox arm, same two lines | [`broker.ts:772-774`](../../tmp/pi-intercom/broker/broker.ts) |

### Exit 2 — cancel

| Port | Upstream | Reached from |
| --- | --- | --- |
| [`broker/send.rs:344-348`](../../crates/cyrup-intercom/src/broker/send.rs) — `handle_cancel_ask`, guarded on `owns_socket && owns_edge` | [`broker.ts:865-873`](../../tmp/pi-intercom/broker/broker.ts) | `client.cancel_ask(&question_id)`, [`session_state.rs:801`](../../crates/cyrup-intercom/src/session_state.rs), `:807`, `:821`, `:826` |
| [`broker/receipts.rs:119-121`](../../crates/cyrup-intercom/src/broker/receipts.rs) — `handle_cancel_message`, parked-mail arm | [`broker.ts:825-829`](../../tmp/pi-intercom/broker/broker.ts) | `intercom{action:"cancel"}` → [`tools/intercom/cancel.rs:33`](../../crates/cyrup-intercom/src/tools/intercom/cancel.rs) |
| [`broker/receipts.rs:149-151`](../../crates/cyrup-intercom/src/broker/receipts.rs) — `handle_cancel_message`, live arm | [`broker.ts:851-856`](../../tmp/pi-intercom/broker/broker.ts) | same |

### Exit 3 — timeout

| Port | Upstream |
| --- | --- |
| [`broker/state.rs:212-215`](../../crates/cyrup-intercom/src/broker/state.rs) — `prune_ask_edges(now)`, called on every `send` frame from [`send.rs:54`](../../crates/cyrup-intercom/src/broker/send.rs) | [`broker.ts:1166-1175`](../../tmp/pi-intercom/broker/broker.ts) |

### The client half of the same three exits (unchanged, and why it needs no change)

[`session_state.rs:793-833`](../../crates/cyrup-intercom/src/session_state.rs) is a `biased`
`tokio::select!` with exactly three arms, and **each already terminates in one of the broker exits
above**:

- `reply = rx` (`:794`) — the reply reached this session only because the peer's `send{replyTo}` was
  routed through **exit 1**, which dropped the edge before the envelope was handed to the asker.
- `() = cancel.cancelled()` (`:819`) and `() = tokio::time::sleep(timeout)` (`:824`) — both call
  `client.cancel_ask(&question_id)` (`:821`, `:826`), i.e. **exit 2**.

So all three client exits already clear the broker's ask edge, and gluing the record removal to the
edge removal makes the record clear on all three with zero client-side change. This is the whole
argument for the placement in §2.

### Two more cleanups that must stay in lockstep (not exits, but leaks if missed)

- Identity takeover — [`state.rs:208-210`](../../crates/cyrup-intercom/src/broker/state.rs)
  `clear_ask_edges_for_session`, whose only caller is
  [`session.rs:76`](../../crates/cyrup-intercom/src/broker/session.rs)
  ([`broker.ts:1177-1183`](../../tmp/pi-intercom/broker/broker.ts)).
- Mailbox expiry and cap eviction — [`mailbox.rs:73-77`](../../crates/cyrup-intercom/src/broker/mailbox.rs)
  and [`mailbox.rs:94-99`](../../crates/cyrup-intercom/src/broker/mailbox.rs)
  ([`broker.ts:1000-1004`, `:1016-1019`](../../tmp/pi-intercom/broker/broker.ts)). At HEAD these are
  **defensive**: a blocking ask to a disconnected target is refused outright at
  [`send.rs:294-301`](../../crates/cyrup-intercom/src/broker/send.rs) (ICOM-045), so no ask edge can
  be parked. Port the removals anyway — they are upstream's, and they cost nothing.

The `edge.to` re-point at
[`mailbox.rs:226-229`](../../crates/cyrup-intercom/src/broker/mailbox.rs) needs **no** record update
for the same reason: unreachable for asks at HEAD, and upstream does not touch the record there.

---

## 5. Implementation plan

### Step 1 — `paths.rs`: the one new path helper

Add beside [`intercom_dir_path`](../../crates/cyrup-intercom/src/paths.rs) (`:95`):

```rust
/// `<intercomDir>/pending-asks` (`PENDING_ASKS_DIR`, `broker/broker.ts:30`) — the only directory
/// this crate writes per-message data into. Resolved through the same
/// [`agent_dir_path`] → [`intercom_dir_path`] chain as `broker.pid` and `broker.sock`, so a
/// `CYRUP_CODING_AGENT_DIR`/`CYRUP_HOME` override moves the records with everything else.
#[must_use]
pub fn pending_asks_dir_path(intercom_dir: &Path) -> PathBuf {
    intercom_dir.join("pending-asks")
}
```

### Step 2 — `broker/limits.rs`: the hard bound

```rust
/// Hard cap on the persisted pending-ask records. **No upstream counterpart** — `broker.ts` bounds
/// the directory by `expiresAt` alone, which bounds it only *within* one ask-timeout window
/// (10 min by default, `config.rs:15`); a session issuing asks in a loop fills it without limit
/// inside that window. 256 is `MAX_MAILBOX_MESSAGES`' value and twice `MAX_SESSIONS`, so no
/// reachable steady state — one outstanding blocking ask per connected session — can reach it, and
/// the cap only ever bites on a pathological producer.
pub(super) const MAX_PENDING_ASK_RECORDS: usize = 256;
```

### Step 3 — new module `broker/pending_asks.rs`

Register it in [`broker/mod.rs`](../../crates/cyrup-intercom/src/broker/mod.rs) alongside `mailbox`
(`mod pending_asks;` — private; the crate's public surface stays `run` alone).

```rust
//! Durable pending-ask observability records (`d69854d`, upstream v0.11.0, issue #104): one JSON
//! file per DELIVERED blocking ask under `<intercomDir>/pending-asks/`, written beside the ask edge
//! and unlinked with it (`broker.ts:99-107,191-200,1185-1229`).
//!
//! **Observability only.** Nothing here is ever read back on the delivery path — the routing
//! decision is `BrokerState::ask_edges` and stays in memory. Every operation is best-effort: an
//! ask the broker has already routed must not fail because a chmod did, so I/O errors are logged in
//! the audit-entry idiom of `tools/intercom/ask.rs:109-111` and never propagated.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::{ensure_intercom_runtime_dir, restrict_intercom_runtime_file};

use super::limits::MAX_PENDING_ASK_RECORDS;

/// One delivered-but-unanswered blocking ask (`PendingAskRecord`, `broker.ts:99-107`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PendingAskRecord {
    /// Upstream writes the message id into BOTH `askId` and `messageId` (`broker.ts:1189-1190`);
    /// both keys are kept so a reader cannot tell a cyrup record from a pi one.
    pub(super) ask_id: String,
    pub(super) message_id: String,
    pub(super) asker: PendingAskParty,
    pub(super) target: PendingAskParty,
    /// `message.content.text` (`broker.ts:1193`).
    pub(super) question: String,
    /// `brokerReceivedAt` — the moment the broker accepted the ask, NOT `Date.now()`. `d69854d`
    /// retargeted the ask edge's own `createdAt` to the same value in the same line
    /// (`broker.ts:685-690`) so record and edge expire together.
    pub(super) created_at: u64,
    /// `createdAt + askTimeoutMs` (`broker.ts:1195`).
    pub(super) expires_at: u64,
}

/// `{ sessionId, name }` (`broker.ts:102-103`).
///
/// `name` is `string | null` upstream and is written as an explicit JSON `null`, never omitted:
/// `isPendingAskRecord` (`broker.ts:179-182`) accepts `null` and REJECTS a missing key. So this must
/// NOT carry the `skip_serializing_if = "Option::is_none"` that `SessionInfo::name` does
/// (`transport/protocol.rs:241`), and must NOT carry `#[serde(default)]` — a record missing the key
/// is invalid and is pruned.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PendingAskParty {
    pub(super) session_id: String,
    pub(super) name: Option<String>,
}

/// The record directory for this broker run.
///
/// `None` is the inert store every unit-driven [`BrokerState`](super::state::BrokerState) gets from
/// `BrokerState::new`, for exactly the reason `trusted_local`/`endpoint_state_id` get placeholders
/// there (`state.rs:120-136`): only the real `run()` has resolved an intercom dir, and a state built
/// in-process must never reach into the developer's real `~/.cyrup`.
pub(super) struct PendingAskStore {
    dir: Option<PathBuf>,
}

impl PendingAskStore {
    pub(super) const fn inert() -> Self {
        Self { dir: None }
    }

    pub(super) const fn at(dir: PathBuf) -> Self {
        Self { dir: Some(dir) }
    }

    /// `writePendingAskRecord` (`broker.ts:1185-1199`).
    pub(super) fn write(&self, record: &PendingAskRecord) {
        let Some(dir) = &self.dir else { return };
        if let Err(e) = write_record(dir, record) {
            tracing::warn!(
                error = %e,
                message_id = %record.message_id,
                "intercom: failed to persist the pending-ask record"
            );
        }
    }

    /// `removePendingAskRecord` (`broker.ts:1201-1209`). A record that is already gone is not an
    /// error — the startup sweep or a prune may have reclaimed it (upstream's `ENOENT` re-throw
    /// guard).
    pub(super) fn remove(&self, message_id: &str) {
        let Some(dir) = &self.dir else { return };
        let path = dir.join(record_file_name(message_id));
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                error = %e,
                message_id,
                "intercom: failed to clear the pending-ask record"
            ),
        }
    }

    /// `prunePendingAskRecords` (`broker.ts:1211-1229`) plus cyrup's hard cap.
    pub(super) fn prune(&self, now: u64) {
        let Some(dir) = &self.dir else { return };
        if let Err(e) = prune_dir(dir, now) {
            tracing::warn!(error = %e, "intercom: failed to prune the pending-ask records");
        }
    }
}

/// `${encodeURIComponent(messageId)}.json` (`broker.ts:191-193`).
///
/// A message id is CALLER-SUPPLIED on the `send` frame, so it can contain `/`, `..`, a NUL or a
/// newline. Every byte outside the unreserved set is percent-encoded — `encodeURIComponent`
/// restricted further (it leaves `!~*'()` bare), so still injective: two distinct ids can never
/// collide on one file, and no id can name a path outside the directory.
fn record_file_name(message_id: &str) -> String {
    let mut out = String::with_capacity(message_id.len() + 5);
    for byte in message_id.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(*byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out.push_str(".json");
    out
}

fn write_record(dir: &Path, record: &PendingAskRecord) -> std::io::Result<()> {
    // `ensurePendingAskRecordDir()` (`broker.ts:195-200`) — `mkdir -p` at 0700 then re-chmod, which
    // is `paths::ensure_intercom_runtime_dir` verbatim (`paths.rs:126-134`).
    ensure_intercom_runtime_dir(dir)?;
    let path = dir.join(record_file_name(&record.message_id));
    // `${JSON.stringify(record, null, 2)}\n` (`broker.ts:1197`) — two-space pretty print, trailing
    // newline, so the file is readable by the operator who is staring at a hung ask.
    let mut body = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
    body.push('\n');
    std::fs::write(&path, body)?;
    // pi passes `{ mode: 0o600 }` on create AND chmods after; `std::fs::write` has no mode
    // argument, so the chmod is what closes the window — the same write-then-restrict pair
    // `lifecycle.rs:186-187` already uses for `broker.pid`.
    restrict_intercom_runtime_file(&path)
}

/// One directory pass. Deletes anything unreadable, unparseable, structurally invalid
/// (`isPendingAskRecord`, `broker.ts:175-189` — including its `expiresAt >= createdAt` clause, which
/// serde cannot express) or expired, then enforces [`MAX_PENDING_ASK_RECORDS`] over the survivors by
/// dropping the OLDEST `created_at` first — the cap-then-evict-oldest shape
/// `queue_mailbox_message` already uses (`mailbox.rs:92-99`).
fn prune_dir(dir: &Path, now: u64) -> std::io::Result<()> {
    ensure_intercom_runtime_dir(dir)?;
    let mut live: Vec<(u64, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || path.extension().and_then(std::ffi::OsStr::to_str) != Some("json")
        {
            continue;
        }
        let parsed = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PendingAskRecord>(&bytes).ok())
            .filter(|record| record.expires_at >= record.created_at);
        match parsed {
            // `if (!isPendingAskRecord(parsed) || now > parsed.expiresAt) unlinkSync(filePath)`.
            Some(record) if now <= record.expires_at => live.push((record.created_at, path)),
            _ => drop(std::fs::remove_file(&path)),
        }
    }
    if live.len() > MAX_PENDING_ASK_RECORDS {
        live.sort_unstable();
        for (_, path) in live.iter().take(live.len() - MAX_PENDING_ASK_RECORDS) {
            drop(std::fs::remove_file(path));
        }
    }
    Ok(())
}
```

### Step 4 — `broker/state.rs`: hold the store, and clear with the edge

Add the field beside `ask_edges` ([`:50`](../../crates/cyrup-intercom/src/broker/state.rs)):

```rust
    /// The on-disk mirror of `ask_edges` (`PENDING_ASKS_DIR`, `broker.ts:30`). Observability only —
    /// nothing reads it back; every mutation is glued to the `ask_edges` mutation above it.
    pub(super) pending_asks: PendingAskStore,
```

`new()` ([`:104-127`](../../crates/cyrup-intercom/src/broker/state.rs)) gets
`pending_asks: PendingAskStore::inert(),` — signature unchanged, so
[`test_support::make_state`](../../crates/cyrup-intercom/src/broker/test_support.rs) (`:19-21`) and
every `BrokerState` built in-process keep working and touch no filesystem.

Add the builder next to `with_listen_endpoint` ([`:132`](../../crates/cyrup-intercom/src/broker/state.rs)):

```rust
    /// Adopt the record directory the real `run()` resolved — separate from [`Self::new`] for the
    /// same reason `with_listen_endpoint` is: only `run()` has an intercom dir.
    pub(super) fn with_pending_ask_store(mut self, pending_asks: PendingAskStore) -> Self {
        self.pending_asks = pending_asks;
        self
    }
```

Rewrite the two `retain` bookkeepers so the record follows the edge — collect-then-act, the shape
`prune_mailbox_messages` already uses ([`mailbox.rs:63-79`](../../crates/cyrup-intercom/src/broker/mailbox.rs)):

```rust
    pub(super) fn clear_ask_edges_for_session(&mut self, session_id: &str) {
        let mut cleared: Vec<String> = Vec::new();
        self.ask_edges.retain(|message_id, edge| {
            if edge.from == session_id || edge.to == session_id {
                cleared.push(message_id.clone());
                return false;
            }
            true
        });
        for message_id in cleared {
            self.pending_asks.remove(&message_id);
        }
    }

    pub(super) fn prune_ask_edges(&mut self, now: u64) {
        // `this.prunePendingAskRecords(now)` FIRST, in upstream's own position
        // (`broker.ts:1167-1168`): it is the only sweep that reclaims records THIS process never
        // wrote — left by a broker that was SIGKILLed mid-ask — which no edge in this map accounts
        // for. It is also where the MAX_PENDING_ASK_RECORDS cap is enforced.
        self.pending_asks.prune(now);
        let timeout = self.ask_timeout_ms;
        let mut expired: Vec<String> = Vec::new();
        self.ask_edges.retain(|message_id, edge| {
            if now.saturating_sub(edge.created_at) > timeout {
                expired.push(message_id.clone());
                return false;
            }
            true
        });
        for message_id in expired {
            self.pending_asks.remove(&message_id);
        }
    }
```

### Step 5 — `broker/send.rs`: write the record (the only write site)

At [`send.rs:145-160`](../../crates/cyrup-intercom/src/broker/send.rs), inside
`if message.expects_reply == Some(true)`, after the mutual-ask refusal and immediately before the
edge insert — upstream's own position ([`broker.ts:684-690`](../../tmp/pi-intercom/broker/broker.ts)):

```rust
            // `this.writePendingAskRecord(message, fromSession, target.info, brokerReceivedAt)`
            // (`broker.ts:684`). LIVE PATH ONLY: `handle_send_to_disconnected` refuses a blocking
            // ask outright (`send.rs:294-301`, ICOM-045), so a record exists exactly when an ask has
            // been accepted for delivery to a connected peer — which is the distinction the operator
            // staring at a hung ask needs: "delivered, peer silent" vs "never delivered".
            if let Some(target_info) = self.sessions.get(&target_id).map(|s| s.info.clone()) {
                self.pending_asks.write(&PendingAskRecord {
                    ask_id: message.id.clone(),
                    message_id: message.id.clone(),
                    asker: PendingAskParty {
                        session_id: from_info.id.clone(),
                        name: from_info.name.clone(),
                    },
                    target: PendingAskParty {
                        session_id: target_info.id,
                        name: target_info.name,
                    },
                    question: message.content.text.clone(),
                    created_at: now,
                    expires_at: now.saturating_add(self.ask_timeout_ms),
                });
            }
            self.ask_edges.insert(message.id.clone(), AskEdge {
                from: current_id.clone(),
                to: target_id.clone(),
                created_at: now,
            });
```

`from_info` is already bound at [`:99-104`](../../crates/cyrup-intercom/src/broker/send.rs) and is
not moved until `:189`; `now` is `brokerReceivedAt`, the same value stamped at `:171`.

Then glue the removals to the three existing edge drops in this file:

- [`:193`](../../crates/cyrup-intercom/src/broker/send.rs) → `self.pending_asks.remove(rt);` after `self.ask_edges.remove(rt);`
- [`:326`](../../crates/cyrup-intercom/src/broker/send.rs) → the same pair
- [`:347`](../../crates/cyrup-intercom/src/broker/send.rs) (`handle_cancel_ask`, inside `if owns_socket && owns_edge`) → the same pair

### Step 6 — `broker/receipts.rs` and `broker/mailbox.rs`: the remaining removals

- [`receipts.rs:120`](../../crates/cyrup-intercom/src/broker/receipts.rs) (parked arm) and
  [`:150`](../../crates/cyrup-intercom/src/broker/receipts.rs) (live arm): add
  `self.pending_asks.remove(&message_id);` inside the same `if self.ask_edges.get(...).is_some_and(...)`
  block, so the ownership guard covers both.
- [`mailbox.rs:75`](../../crates/cyrup-intercom/src/broker/mailbox.rs) (retention expiry) and
  [`:97`](../../crates/cyrup-intercom/src/broker/mailbox.rs) (cap eviction): add the removal inside
  the existing `if expects_reply` / `if evicted.message.expects_reply == Some(true)` guards.
  Defensive parity — unreachable at HEAD, see §4.

### Step 7 — `broker/lifecycle.rs`: build the store and sweep at startup

In `run()`, immediately after
[`runtime_claim::assert_no_live_broker(&pid_path)?;`](../../crates/cyrup-intercom/src/broker/lifecycle.rs) (`:132`):

```rust
    // `ensurePendingAskRecordDir(); this.prunePendingAskRecords();` (`broker.ts:221-222`), in
    // upstream's own position: AFTER `assertNoLiveBroker` (`:220`), so the startup sweep runs only
    // once this process owns the runtime dir and can never delete a live incumbent's records. This
    // is what reclaims records left by a broker that was SIGKILLed with asks outstanding — the one
    // leak no exit path can close.
    let pending_asks =
        pending_asks::PendingAskStore::at(paths::pending_asks_dir_path(&intercom_dir));
    pending_asks.prune(crate::transport::protocol::now_ms());
```

and chain it at [`:197-198`](../../crates/cyrup-intercom/src/broker/lifecycle.rs):

```rust
        BrokerState::new(ask_timeout, shutdown.clone())
            .with_listen_endpoint(listener.is_trusted_local(), endpoint_state_id)
            .with_pending_ask_store(pending_asks),
```

`shutdown_broker` ([`:71`](../../crates/cyrup-intercom/src/broker/lifecycle.rs), `g.ask_edges.clear()`)
is **left alone**, deliberately and matching upstream: a broker that exits with asks outstanding
should leave the records behind — that is precisely the state an operator wants to find, and it is
what gives the next broker's startup sweep something to reclaim. Those records are still bounded by
`expiresAt` and by the cap.

---

## Definition of Done

Observable behavior, on a POSIX host with the broker running:

1. **A delivered blocking ask appears on disk.** While `intercom{action:"ask"}` is blocked waiting,
   `~/.cyrup/intercom/pending-asks/` contains exactly one `<encoded message id>.json` for it,
   holding `askId`, `messageId`, `asker.{sessionId,name}`, `target.{sessionId,name}`, `question`,
   `createdAt` and `expiresAt`, where `expiresAt - createdAt` equals the configured ask timeout and
   `name` is a JSON `null` for an unnamed session rather than an absent key.
2. **It clears on reply.** The peer answers; the file is gone by the time the asking tool call
   returns its `**Reply from …**` text.
3. **It clears on cancel.** Either flavour — the asker's tool call is cancelled (`cancel_ask` from
   `session_state.rs:821`) or a peer runs `intercom{action:"cancel"}` (`cancel_message`) — leaves the
   directory empty.
4. **It clears on timeout.** After the ask timeout elapses and any subsequent `send` frame drives
   `prune_ask_edges`, the file is gone; a record whose `expiresAt` has passed is deleted on the next
   sweep even if this broker never wrote it.
5. **No leak across a broker restart.** `SIGKILL` the broker mid-ask, restart it: the startup sweep
   empties the directory of expired and malformed records before the listener binds.
6. **Bounded.** After a loop issuing far more than `MAX_PENDING_ASK_RECORDS` asks inside one ask
   timeout window, the directory holds at most `MAX_PENDING_ASK_RECORDS` files and the survivors are
   the newest by `createdAt`.
7. **Permissions.** The directory is `0700` and every record file `0600`.
8. **Delivery is untouched.** `session_state.rs`, `reply_tracker.rs` and `tools/intercom/*.rs` are
   byte-identical to their pre-change state; a blocking ask, its reply, the mutual-ask refusal, the
   offline refusal and the timeout message all behave exactly as before. Making the record directory
   read-only produces `tracing::warn!` lines and fully working asks — never a failed or delayed
   delivery.
