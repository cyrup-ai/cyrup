---
stage: aug
status: done
updated: 2026-08-27 22:24
---

# Make the broker extension bus actually take effect

> **Upstream parity gap.** `cyrup-intercom` is a port of `pi-intercom` **v0.9.2**; upstream is now
> **v0.12.0** (`ef95f19`, 2026-08-22) at [`nicobailon/pi-intercom`](https://github.com/nicobailon/pi-intercom).
> Reference checkout: `./tmp/pi-intercom`. Gap analysis: `docs/gap-analysis/11-cyrup-intercom.md` — **ICOM-016**.

## Core objective

The port models the extension-bus **protocol** perfectly and implements **none of its effects**.
Every one of the three bus frames is answered with a refusal or ignored:
[`handle_extension_publish`](../../crates/cyrup-intercom/src/broker/extensions.rs) (`:136`) always
writes `error: "Session has not advertised extension capability"`,
[`handle_extension_state_commit`](../../crates/cyrup-intercom/src/broker/extensions.rs) (`:171`)
always writes `committed: false, revision: 0`, and
[`handle_extension_capabilities_update`](../../crates/cyrup-intercom/src/broker/extensions.rs)
(`:88`) validates and then `tracing::debug!("… ignored (bus not implemented)")`.

Land the three effects upstream has and the port does not:

1. **Capability bookkeeping** — a session's advertised `extensions` are stored on its
   `ConnectedSession` (register **and** `extension_capabilities_update`).
2. **Owner election** — a per-namespace owner, elected by broker-owned registration order, with a
   fresh `epoch` on every change and an `extension_owner` broadcast to every capable session.
3. **A persisted, revision-checked state store** — a 1:1 port of
   [`broker/extension-state.ts`](../../tmp/pi-intercom/broker/extension-state.ts) with
   compare-and-swap commits, an atomic crash-safe write, a `.bak` fallback and a sha256 payload
   integrity check.

Once those exist, publish fans out and commit commits — and the broker must finally advertise
`EXTENSION_BUS_FEATURE` on `registered`, which is what a conforming pi client gates every bus frame
on (`v0.9.2 broker/client.ts:648,817-819`).

**Target the v0.9.2 semantics, not v0.12.0's.** The only difference between the two in this area is
scope keying (`scopedExtensionKey`, `scopedExtensionStateNamespace`,
[`broker/broker.ts:152-161`](../../tmp/pi-intercom/broker/broker.ts)), and the port has **no**
`scopeId` concept anywhere (`grep -rn scope_id crates/cyrup-intercom/src` → 0 hits). Scoping is a
separate v0.10.1..v0.12.0 gap; adding a half of it here would be an unported feature pretending to
be ported. `broker/extension-state.ts` is otherwise **byte-identical between v0.9.2 and v0.12.0**
apart from a local-variable refactor in `readEnvelope` — verified with
`git -C tmp/pi-intercom show v0.9.2:broker/extension-state.ts | diff - tmp/pi-intercom/broker/extension-state.ts`.

## What upstream does

### The state manager — [`broker/extension-state.ts`](../../tmp/pi-intercom/broker/extension-state.ts)

One file, 197 lines, no broker state involved. Namespace → `{ revision, payload }`, cached in a
`Map` and persisted one file per namespace under `<INTERCOM_DIR>/extension-state/`.

```ts
const MAX_STATE_BYTES = 64 * 1024;                                    // :16

interface StateEnvelope {                                             // :18-25
  formatVersion: 1; namespace: string; revision: number;
  updatedAt: number; payloadSha256: string; payload: unknown;
}

export interface StateCommitResult {                                  // :27-32
  committed: boolean; revision: number; reason?: string; payload?: unknown;
}

function serializePayload(payload: unknown): string | null {          // :34-44
  try {
    const json = JSON.stringify(payload);
    if (json === undefined || Buffer.byteLength(json, "utf8") > MAX_STATE_BYTES) return null;
    return json;
  } catch { return null; }
}
```

`statePath` is `sha256(namespace)` hex + `.json`, `backupPath` is that + `.bak` (`:59-66`).
`readEnvelope` (`:68-108`) rejects anything whose `formatVersion !== 1`, whose `namespace` does not
match, whose `revision` is not a non-negative safe integer, or whose payload re-hashes to something
other than the stored `payloadSha256`. `loadState` (`:110-121`) answers from the cache, else the
primary file, else the `.bak`, and caches what it found.

`commitState` (`:123-192`) is the whole point — a compare-and-swap with three distinct refusals and
an atomic write:

```ts
  commitState(namespace: string, expectedRevision: number, payload: unknown): StateCommitResult {
    const payloadJson = serializePayload(payload);
    const current = this.loadState(namespace);
    const currentRevision = current?.revision ?? 0;

    if (payloadJson === null) {
      return { committed: false, revision: currentRevision,
        reason: "Invalid extension state or payload exceeds 64 KiB limit" };
    }
    if (!Number.isSafeInteger(expectedRevision) || expectedRevision < 0) {
      return { committed: false, revision: currentRevision, reason: "Invalid expected revision" };
    }
    if (expectedRevision !== currentRevision) {
      return { committed: false, revision: currentRevision, reason: "Revision mismatch",
        ...(current ? { payload: current.payload } : {}) };
    }

    const envelope: StateEnvelope = { formatVersion: 1, namespace, revision: currentRevision + 1,
      updatedAt: Date.now(), payloadSha256: payloadHash(payloadJson), payload };
    const tempPath = `${statePath}.tmp.${process.pid}.${randomUUID()}`;
    try {
      writeFileSync(tempPath, JSON.stringify(envelope), { mode: 0o600 });
      const file = openSync(tempPath, "r"); try { fsyncSync(file); } finally { closeSync(file); }
      if (this.readEnvelope(statePath, namespace)) copyFileSync(statePath, backupPath);
      renameSync(tempPath, statePath);
      try { const d = openSync(dirname(statePath), "r"); try { fsyncSync(d); } finally { closeSync(d); } }
      catch { /* Directory fsync is unavailable on some platforms. */ }
      this.states.set(namespace, { revision: envelope.revision, payload });
      return { committed: true, revision: envelope.revision };
    } catch {
      return { committed: false, revision: currentRevision, reason: "Failed to persist extension state" };
    } finally { rmSync(tempPath, { force: true }); }
  }

  getCurrentRevision(namespace: string): number {                     // :194-196
    return this.loadState(namespace)?.revision ?? 0;
  }
```

Note the ordering: **the backup is taken from the CURRENT file only if that file still parses**, and
only after the temp file is durable. That is what makes a torn write recoverable rather than fatal.

### Owner election — `recomputeNamespaceOwners` ([`broker/broker.ts:1184-1261`](../../tmp/pi-intercom/broker/broker.ts) @ v0.9.2)

```ts
  private recomputeNamespaceOwners(): void {
    const namespaces = new Set(this.namespaceOwners.keys());
    for (const session of this.sessions.values()) {
      for (const extension of session.extensions ?? []) namespaces.add(extension.namespace);
    }
    for (const namespace of namespaces) {
      const candidates: Array<{ sessionId: string; session: ConnectedSession }> = [];
      for (const [sessionId, session] of this.sessions) {
        if (session.extensions) {
          const hasNamespace = session.extensions.some(
            (ext) => ext.namespace === namespace && ext.ownerEligible);
          if (hasNamespace) candidates.push({ sessionId, session });
        }
      }
      if (candidates.length === 0) {
        if (this.namespaceOwners.delete(namespace)) {
          for (const session of this.sessions.values()) {
            const isCapable = session.extensions?.some((e) => e.namespace === namespace);
            if (isCapable) writeMessage(session.socket, { type: "extension_owner", namespace });
          }
        }
        continue;
      }
      // Use broker-owned registration order so clients cannot seize authority
      // by backdating their advertised session start time. Stable-ID socket
      // replacements preserve the original order.
      candidates.sort((a, b) => {
        if (a.session.ownerOrder !== b.session.ownerOrder) return a.session.ownerOrder - b.session.ownerOrder;
        return a.sessionId.localeCompare(b.sessionId);
      });
      const winner = candidates[0];
      const existing = this.namespaceOwners.get(namespace);
      const ownerChanged = !existing || existing.sessionId !== winner.sessionId;
      const socketChanged = existing && existing.socket !== winner.session.socket;
      if (ownerChanged || socketChanged) {
        const epoch = randomUUID();
        this.namespaceOwners.set(namespace, { sessionId: winner.sessionId, socket: winner.session.socket, epoch });
        for (const session of this.sessions.values()) {
          if (session.extensions?.length) {
            const isCapable = session.extensions.some((ext) => ext.namespace === namespace);
            if (isCapable) writeMessage(session.socket, {
              type: "extension_owner", namespace, ownerId: winner.sessionId, ownerEpoch: epoch });
          }
        }
      }
    }
  }
```

`ownerOrder` comes from a broker-owned monotonic counter, assigned at register and **preserved
across an identity takeover** — `ownerOrder: previous?.ownerOrder ?? this.nextOwnerOrder++`
([`:488`](../../tmp/pi-intercom/broker/broker.ts)). It is called from exactly four sites: socket
close (`:337`), register (`:509`), unregister (`:544`), capabilities update (`:569`).

### Register and `extension_capabilities_update` ([`:502-524`, `:548-585`](../../tmp/pi-intercom/broker/broker.ts) @ v0.9.2)

```ts
        writeMessage(socket, { type: "registered", sessionId: id, features: [EXTENSION_BUS_FEATURE] });
        this.broadcast({ type: "session_joined", session: info }, id);

        this.recomputeNamespaceOwners();
        this.flushMailboxForSession(connectedSession);

        if (extensions) {
          for (const ext of extensions) {
            const owner = this.namespaceOwners.get(ext.namespace);
            writeMessage(socket, { type: "extension_owner", namespace: ext.namespace,
              ...(owner ? { ownerId: owner.sessionId, ownerEpoch: owner.epoch } : {}) });
            const state = this.extensionStateManager.loadState(ext.namespace);
            if (state) writeMessage(socket, { type: "extension_state",
              namespace: ext.namespace, revision: state.revision, payload: state.payload });
          }
        }
```

`extension_capabilities_update` (`:565-585`) is the same tail, after `session.extensions = extensions`.
The **unconditional** `extension_owner` reply — sent even when nobody owns the namespace — is how a
freshly joined session learns "this namespace exists and is unowned".

### `handleExtensionPublish` ([`:1262-1356`](../../tmp/pi-intercom/broker/broker.ts) @ v0.9.2)

After the two miss branches the port already has, upstream runs, in this exact order: `namespace`
must be a string passing `validateNamespace` → `"Invalid namespace"`; `audience` must be `"owner"`
or `"capable"` → `"Invalid audience"`; `serializedPayloadSize(payload)` must be non-null and
`<= MAX_EXTENSION_MESSAGE_BYTES` (16 KiB) →
`"Invalid extension payload or payload exceeds 16 KiB limit"`; the sender must itself advertise the
namespace → `"Sender does not have capability for this namespace"`; an `owner`/`ownerOnly` publish
needs an owner → `"No owner for this namespace"`; and `ownerOnly` needs a string `ownerEpoch` →
`"ownerEpoch required for owner-only messages"`, matching session **and socket** and epoch →
`"Owner validation failed"`. Then:

```ts
    for (const [recipientId, recipientSession] of this.sessions) {
      if (!recipientSession.extensions?.length) continue;
      const isCapable = recipientSession.extensions.some((ext) => ext.namespace === namespace);
      if (!isCapable) continue;
      const shouldReceive = audience === "capable"
        || (audience === "owner" && owner !== undefined
            && recipientId === owner.sessionId && recipientSession.socket === owner.socket);
      if (shouldReceive) {
        writeMessage(recipientSession.socket, { type: "extension_message", namespace,
          fromSessionId: currentId,
          ...(owner ? { ownerId: owner.sessionId, ownerEpoch: owner.epoch } : {}),
          payload });
      }
    }
```

The publisher is **not** excluded from a `capable` fan-out — a session that advertised the namespace
receives its own publish. That is upstream behaviour, not an oversight; do not "fix" it.

### `handleExtensionStateCommit` ([`:1358-1495`](../../tmp/pi-intercom/broker/broker.ts) @ v0.9.2)

Every exit writes an `extension_state_result`. Order after the two ported miss branches:
`"Invalid namespace"` (revision `0`, `namespace: String(namespace)`), then — each carrying
`revision: getCurrentRevision(namespace)` — `"Invalid ownerEpoch"`, `"Invalid expectedRevision"`,
`"Invalid extension state or payload exceeds 64 KiB limit"` (64 KiB here, not 16),
`"Sender does not have capability for this namespace"`, `"No owner for this namespace"`,
`"Owner validation failed"`. Only then:

```ts
    const result = this.extensionStateManager.commitState(namespace, expectedRevision, payload);
    writeMessage(socket, { type: "extension_state_result", namespace,
      committed: result.committed, revision: result.revision, reason: result.reason });
    if (result.committed) {
      for (const recipientSession of this.sessions.values()) {
        if (!recipientSession.extensions?.length) continue;
        const isCapable = recipientSession.extensions.some((ext) => ext.namespace === namespace);
        if (isCapable) writeMessage(recipientSession.socket,
          { type: "extension_state", namespace, revision: result.revision, payload });
      }
    }
```

`StateCommitResult.payload` — the current payload returned on a `"Revision mismatch"` — is **not**
echoed onto the wire by the broker; only `committed`/`revision`/`reason` reach the committer. Model
the field anyway (it is part of the manager's contract) but do not invent a wire field for it.

## What already exists in the port and must be reused

**Never rewrite any of this.** The protocol half is complete and correct:

| Need | Already in the port |
|---|---|
| `extension_owner` / `extension_message` / `extension_state` / `extension_state_result` wire types | [`transport/protocol.rs:876-925`](../../crates/cyrup-intercom/src/transport/protocol.rs) — `BrokerMessage::ExtensionOwner/ExtensionMessage/ExtensionState/ExtensionStateResult` |
| The both-or-neither `ownerId`/`ownerEpoch` pair | [`ExtensionOwnerRef`](../../crates/cyrup-intercom/src/transport/protocol.rs) (`:556-603`), `#[serde(flatten)]` onto both frames — `ExtensionOwnerRef::default()` **is** the unowned form |
| `audience` | [`ExtensionAudience`](../../crates/cyrup-intercom/src/transport/protocol.rs) (`:604-613`) — `Owner` / `Capable` |
| `ExtensionCapability` incl. the `[MAP-ONLY]` array rejection | [`transport/protocol.rs:534-554`](../../crates/cyrup-intercom/src/transport/protocol.rs) |
| `EXTENSION_BUS_FEATURE` | [`transport/protocol.rs:89`](../../crates/cyrup-intercom/src/transport/protocol.rs) |
| `validateExtensionCapability` + the `extensions` array/cap guard | [`extensions_field_is_valid`](../../crates/cyrup-intercom/src/broker/extensions.rs) (`:32`) — already shared with `handle_register` |
| `validateNamespace` | [`namespace_is_valid`](../../crates/cyrup-intercom/src/broker/extensions.rs) (`:51`) |
| `session.socket !== socket` | [`session_owns_connection`](../../crates/cyrup-intercom/src/broker/extensions.rs) (`:117`) |
| `String(msg.namespace \|\| "")` | [`js_string_or_empty`](../../crates/cyrup-intercom/src/broker/js.rs) (`:22`) |
| Join-ordered session iteration (pi's `Map`) | [`BrokerState::sessions_in_order`](../../crates/cyrup-intercom/src/broker/state.rs) (`:173`) |
| `writeMessage` | [`send_msg`](../../crates/cyrup-intercom/src/broker/frame.rs) (`:350`) |
| `MAX_EXTENSIONS_PER_SESSION` | [`broker/limits.rs:31`](../../crates/cyrup-intercom/src/broker/limits.rs) |
| 0700 dir / 0600 file modes, `Date.now()` | [`paths::INTERCOM_DIR_MODE`, `INTERCOM_RUNTIME_FILE_MODE`, `restrict_intercom_runtime_file`](../../crates/cyrup-intercom/src/paths.rs) (`:30-33`, `:136-155`); [`protocol::now_ms`](../../crates/cyrup-intercom/src/transport/protocol.rs) (`:961`) |
| `Number.isSafeInteger` bound | `JS_MAX_SAFE_INTEGER` ([`transport/protocol.rs:92`](../../crates/cyrup-intercom/src/transport/protocol.rs)) and its `js_safe_integer` body (`:158-186`) — copy the *shape* into `broker/js.rs`, see step 2 |
| Fire-and-forget fan-out idiom | [`broker/send.rs:178-190`](../../crates/cyrup-intercom/src/broker/send.rs), [`broker/receipts.rs:70`](../../crates/cyrup-intercom/src/broker/receipts.rs) |

The client half needs **no** change: [`transport/client.rs:906-916`](../../crates/cyrup-intercom/src/transport/client.rs)
already decodes all four broker→client extension frames without tearing the connection down, and
cyrup's own `SessionRegistration` never advertises `extensions`, so cyrup clients still receive none
of them. Only the two stale comments there and at
[`transport/protocol.rs:84-88`](../../crates/cyrup-intercom/src/transport/protocol.rs) ("cyrup never
advertises it") must be corrected — see step 9.

## Implementation plan

### Step 1 — the hashing dependency

`sha256` is required twice (the state file name, and the envelope's payload integrity check) and the
crate has none. Add it to the workspace table in `Cargo.toml`, next to `ring`/`base64`, in the house
comment style, and take a direct edge from
[`crates/cyrup-intercom/Cargo.toml`](../../crates/cyrup-intercom/Cargo.toml):

```toml
# ICOM-016 — `createHash("sha256")` in `pi-intercom/broker/extension-state.ts:46,60`: the extension
# state file is NAMED by the sha256 of its namespace and CARRIES a sha256 of its payload, which is
# what lets a torn write be detected and fall back to the `.bak`. ADDS NO NEW CRATE TO THE GRAPH:
# `sha2` 0.10.9 is already resolved in Cargo.lock as a transitive dependency.
sha2                 = { version = "0.10" }
```

Do **not** reach for `cyrup-provider`'s hand-rolled `auth::oauth::sha256` — that would make
`cyrup-intercom` depend on the whole provider crate to get 40 lines of hashing.

`sha2` returns bytes; upstream needs lowercase hex. Add a private `fn hex(bytes: &[u8]) -> String`
to the new module (step 3) — do not add a `hex` crate for it.

### Step 2 — `broker/js.rs`: `Number.isSafeInteger` as a value test

`expectedRevision` arrives inside a raw `serde_json::Value` frame (the handlers never deserialize a
typed `ClientMessage`), so protocol.rs's `js_safe_integer` **deserializer** cannot be reused
directly. Add its value-level twin next to `js_string_or_empty`, which is the module that exists for
exactly this ("JavaScript value semantics reproduced verbatim"):

```rust
/// `typeof v === "number" && Number.isSafeInteger(v) && v >= 0` — the guard pi applies to
/// `msg.expectedRevision` before it reaches the state manager
/// (`v0.9.2 broker/broker.ts:1417`, and again at `extension-state.ts:132`).
///
/// `None` is every rejected shape at once: absent, `null`, non-numeric, fractional, negative, or
/// above `2^53 - 1`. JS treats `-0` as a safe integer that is not `< 0`, so it maps to `Some(0)`.
pub(super) fn js_safe_u64(v: Option<&serde_json::Value>) -> Option<u64> {
    let n = v?.as_number()?;
    if let Some(u) = n.as_u64() {
        return (u <= JS_MAX_SAFE_INTEGER).then_some(u);
    }
    let f = n.as_f64()?;
    if !f.is_finite() || f.fract() != 0.0 || f < 0.0 || f > JS_MAX_SAFE_INTEGER as f64 {
        return None;
    }
    Some(f as u64)
}
```

Give it the same `#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = …)]`
the existing `js_safe_integer` carries, and define `JS_MAX_SAFE_INTEGER` here rather than importing
protocol.rs's private one (`runtime_claim.rs:50` sets that precedent already).

### Step 3 — new module `crates/cyrup-intercom/src/broker/extension_state.rs`

A 1:1 port of [`broker/extension-state.ts`](../../tmp/pi-intercom/broker/extension-state.ts),
declared `mod extension_state;` in [`broker/mod.rs`](../../crates/cyrup-intercom/src/broker/mod.rs)
and named in its `## Layout` paragraph. Pure — no `BrokerState`, no sockets, exactly like upstream.

```rust
//! The persisted, revision-checked extension state store — a 1:1 port of
//! `pi-intercom/broker/extension-state.ts` (v0.9.2 and v0.12.0 are identical here apart from a
//! local-variable refactor in `readEnvelope`).
//!
//! One file per namespace under `<intercomDir>/extension-state/`, named by the sha256 of the
//! namespace, written through a temp file + `fsync` + rename with a `.bak` of the previous
//! envelope, and integrity-checked on read by re-hashing the payload. [`ExtensionStateManager`]
//! caches what it reads, which is why the broker owns exactly one of them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use super::limits::MAX_EXTENSION_STATE_BYTES;

/// `StateEnvelope` (`extension-state.ts:18-25`). `formatVersion` is not a field: it is always `1`
/// on write and any other value fails the read, so it is written literally and checked literally.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StateEnvelope {
    format_version: u8,
    namespace: String,
    revision: u64,
    updated_at: u64,
    payload_sha256: String,
    payload: serde_json::Value,
}

/// `StateCommitResult` (`extension-state.ts:27-32`).
///
/// [`Self::payload`] is the CURRENT payload returned alongside a `"Revision mismatch"`; the broker
/// does not put it on the wire (`v0.9.2 broker/broker.ts:1478-1484` echoes only committed/revision/
/// reason), so it exists for the manager's contract, not for a frame.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StateCommitResult {
    pub(super) committed: bool,
    pub(super) revision: u64,
    pub(super) reason: Option<&'static str>,
    pub(super) payload: Option<serde_json::Value>,
}

/// One namespace's cached state (`extension-state.ts:51`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NamespaceState {
    pub(super) revision: u64,
    pub(super) payload: serde_json::Value,
}

/// `serializePayload` (`extension-state.ts:34-44`) WITHOUT its 64 KiB cap, plus
/// `serializedPayloadSize` (`v0.9.2 broker/broker.ts:44-51`) — one function, because upstream's two
/// differ only in whether the cap is applied, and the callers apply different caps (16 KiB for a
/// publish, 64 KiB for a commit).
///
/// `None` is pi's `undefined`/`null` return: an ABSENT payload. `JSON.stringify(undefined)` is
/// `undefined`, so a frame with no `payload` key is refused by both call sites — that is upstream
/// behaviour, not a strictness the port adds.
///
/// [CYRUP-DELTA] `Buffer.byteLength(json, "utf8")` vs `String::len()`: both count UTF-8 bytes, and
/// serde_json escapes the same characters `JSON.stringify` does, so the two lengths agree for every
/// value that can come off the wire. Key ORDER differs (serde_json's `Map` is sorted, JS objects
/// keep insertion order) and the length is order-invariant; the persisted envelope's payload is
/// re-serialized from the same `Map`, so its hash round-trips within this implementation.
pub(super) fn serialize_payload(payload: Option<&serde_json::Value>) -> Option<String> {
    serde_json::to_string(payload?).ok()
}

fn payload_hash(payload_json: &str) -> String { hex(&Sha256::digest(payload_json.as_bytes())) }

fn hex(bytes: &[u8]) -> String { … }  // lowercase, `createHash(...).digest("hex")`

pub(super) struct ExtensionStateManager {
    states: HashMap<String, NamespaceState>,
    state_dir: PathBuf,
}

impl ExtensionStateManager {
    /// `new ExtensionStateManager(INTERCOM_DIR)` (`extension-state.ts:53-56`):
    /// `mkdirSync(stateDir, { recursive: true, mode: 0o700 })`.
    ///
    /// Upstream's `mkdirSync` THROWS out of the constructor, i.e. the broker never starts. Here the
    /// failure is logged and deferred to the first commit, which already has upstream's own
    /// `"Failed to persist extension state"` refusal for it — [`BrokerState::new`] is infallible
    /// and every non-persisting bus operation (owner election, publish fan-out) is unaffected by an
    /// unwritable state dir, so refusing to start would be strictly worse than upstream.
    pub(super) fn new(state_dir: PathBuf) -> Self { … }

    fn state_path(&self, namespace: &str) -> PathBuf {
        self.state_dir.join(format!("{}.json", hex(&Sha256::digest(namespace.as_bytes()))))
    }
    fn backup_path(&self, namespace: &str) -> PathBuf { … }        // `${statePath}.bak`
    fn read_envelope(&self, path: &Path, namespace: &str) -> Option<StateEnvelope> { … }

    /// `loadState` (`extension-state.ts:110-121`): cache, then the primary file, then the `.bak`.
    pub(super) fn load_state(&mut self, namespace: &str) -> Option<&NamespaceState> { … }

    /// `commitState` (`extension-state.ts:123-192`).
    pub(super) fn commit_state(
        &mut self,
        namespace: &str,
        expected_revision: u64,
        payload: Option<&serde_json::Value>,
        now: u64,
    ) -> StateCommitResult { … }

    /// `getCurrentRevision` (`extension-state.ts:194-196`).
    pub(super) fn current_revision(&mut self, namespace: &str) -> u64 {
        self.load_state(namespace).map_or(0, |s| s.revision)
    }
}
```

Requirements on `commit_state`, in upstream's order, each returning `revision: current_revision`:

- `serialize_payload(payload)` is `None` **or** longer than `MAX_EXTENSION_STATE_BYTES` →
  `reason: Some("Invalid extension state or payload exceeds 64 KiB limit")`.
- `expected_revision != current_revision` → `Some("Revision mismatch")` **plus** `payload: current`
  when a current state exists.
  (`Invalid expected revision` is unreachable from the broker — `js_safe_u64` has already rejected
  every value that would trip it — so the `u64` parameter type IS that check. Say so in a comment;
  do not add a second guard that cannot fire.)
- Otherwise build the envelope at `revision: current_revision + 1`, `updated_at: now` (pass
  `protocol::now_ms()` in from the handler, matching every other `now` in the broker), and persist:
  temp path `<state>.json.tmp.<pid>.<uuid v4>`; `std::fs::write` then
  `paths::restrict_intercom_runtime_file(&tmp)` (the port's own write-then-restrict idiom,
  `broker/lifecycle.rs:186-187`, and a no-op off POSIX); `File::open(&tmp)?.sync_all()?`;
  `read_envelope(state_path)` is `Some` → `std::fs::copy(state, backup)`; `std::fs::rename`;
  best-effort `File::open(dir)?.sync_all()` with the failure swallowed under upstream's own
  "Directory fsync is unavailable on some platforms." comment; cache; return
  `committed: true`. Any I/O error → `Some("Failed to persist extension state")`. Remove the temp
  file on **every** path (upstream's `finally`) — a `let _ = std::fs::remove_file(&tmp);` before each
  return, or a small guard type; do not leak it on the error path.

`read_envelope` must reject, exactly as upstream does: unreadable/unparseable file; a non-object
value; `format_version != 1`; `envelope.namespace != namespace`; a `revision` that is not a
non-negative safe integer; a non-numeric `updated_at`; a non-string `payload_sha256`; and a payload
that re-serializes to something whose hash differs from `payload_sha256`. Deserialize into
`serde_json::Value` first and check fields by hand rather than relying on serde's type errors — the
`revision` safe-integer bound and the `format_version` literal are not expressible as `#[derive]`
alone, and reusing `js::js_safe_u64` for the revision keeps one definition of that rule.

### Step 4 — `broker/limits.rs`

```rust
/// `MAX_EXTENSION_MESSAGE_BYTES = 16 * 1024` (`v0.9.2 broker/broker.ts:37`).
pub(super) const MAX_EXTENSION_MESSAGE_BYTES: usize = 16 * 1024;
/// `MAX_EXTENSION_STATE_BYTES = 64 * 1024` (`v0.9.2 broker/broker.ts:38`), which is the same bound
/// upstream spells a second time as `MAX_STATE_BYTES` (`broker/extension-state.ts:16`). One
/// constant here, because the two are one rule and a divergence between them would be a silent
/// accept-then-refuse.
pub(super) const MAX_EXTENSION_STATE_BYTES: usize = 64 * 1024;
```

### Step 5 — `broker/state.rs`: the state the bus needs

On [`ConnectedSession`](../../crates/cyrup-intercom/src/broker/state.rs) (`:26`):

```rust
    /// `ownerOrder` (`v0.9.2 broker/broker.ts:56`) — the broker-owned registration order the
    /// namespace-owner election sorts on, assigned from [`BrokerState::next_owner_order`] and
    /// PRESERVED across an identity takeover (`:488`), so a client cannot seize a namespace by
    /// reconnecting or by backdating its advertised `startedAt`.
    pub(super) owner_order: u64,
    /// `extensions` (`v0.9.2 broker/broker.ts:57`), as advertised on `register` or by a later
    /// `extension_capabilities_update`.
    ///
    /// Upstream's is `ExtensionCapability[] | undefined`; an EMPTY vec is the faithful stand-in for
    /// `undefined` because every reader is either `!session.extensions?.length` (`:1277`) or
    /// `session.extensions ?? []` (`:1188`) — no branch upstream can tell the two apart.
    pub(super) extensions: Vec<crate::transport::protocol::ExtensionCapability>,
```

On [`BrokerState`](../../crates/cyrup-intercom/src/broker/state.rs) (`:42`):

```rust
    /// `namespaceOwners` (`v0.9.2 broker/broker.ts:225`), keyed by namespace.
    ///
    /// [CYRUP-DELTA] A `BTreeMap` where pi has an insertion-ordered `Map`, so
    /// `recompute_namespace_owners` walks namespaces in lexicographic rather than first-seen order.
    /// The only thing that order can reach is the relative order of `extension_owner` frames for
    /// two DIFFERENT namespaces on one socket; every consumer of that frame is per-namespace and
    /// idempotent (`v0.9.2 broker/client.ts:538-552`), so no peer can observe the difference — and
    /// a `HashMap` here WOULD be observable as nondeterminism across runs, which is the failure
    /// `session_order` exists to prevent.
    pub(super) namespace_owners: std::collections::BTreeMap<String, NamespaceOwner>,
    /// `nextOwnerOrder = 1` (`v0.9.2 broker/broker.ts:226`).
    pub(super) next_owner_order: u64,
    /// `extensionStateManager` (`v0.9.2 broker/broker.ts:227,232`).
    pub(super) extension_state: ExtensionStateManager,
```

`BrokerState::new` takes the state directory — the manager is constructed in pi's constructor from
`INTERCOM_DIR`, and the port has nowhere else to put it:

```rust
    pub(super) fn new(ask_timeout_ms: u64, shutdown: Arc<Notify>, extension_state_dir: PathBuf) -> Self
```

`next_owner_order: 1`. Update every call site:
[`lifecycle.rs:197`](../../crates/cyrup-intercom/src/broker/lifecycle.rs) passes
`paths::extension_state_dir_path(&intercom_dir)` (new one-liner in
[`paths.rs`](../../crates/cyrup-intercom/src/paths.rs) next to `intercom_dir_path`, returning
`intercom_dir.join("extension-state")` and citing `extension-state.ts:54`); `lifecycle.rs:313`,
`dispatch.rs:145`, `dispatch.rs:201`, `presence.rs:201`, `presence.rs:249`, `presence.rs:281` and
`test_support.rs:20` pass a process-unique path under `std::env::temp_dir()`.

`on_connection_closed` ([`state.rs:250`](../../crates/cyrup-intercom/src/broker/state.rs)) gains
`self.recompute_namespace_owners();` immediately after the `SessionLeft` broadcast and before
`return true` — pi's `:337`, which is what re-elects a namespace whose owner just died.

### Step 6 — `broker/extensions.rs`: owner election

`NamespaceOwner` and `recompute_namespace_owners` belong here, not in `state.rs`: this module is the
extension-bus concern, and the port's rule is one protocol concern per module.

```rust
/// `NamespaceOwner` (`v0.9.2 broker/broker.ts:60-64`). pi's `socket` identity is this port's
/// `conn_id`: an identity takeover reassigns the id to a new connection, and upstream's
/// `existing.socket !== winner.session.socket` is precisely the check that a re-elected owner on a
/// NEW socket gets a NEW epoch — dropping it would let a superseded connection keep committing.
pub(super) struct NamespaceOwner {
    pub(super) session_id: String,
    pub(super) conn_id: u64,
    pub(super) epoch: String,
}

impl BrokerState {
    /// `recomputeNamespaceOwners` (`v0.9.2 broker/broker.ts:1184-1261`). Called from all four of
    /// pi's sites: register (`:509`), unregister (`:544`), socket close (`:337`) and
    /// `extension_capabilities_update` (`:569`).
    pub(super) fn recompute_namespace_owners(&mut self) {
        // `new Set(this.namespaceOwners.keys())` + every advertised namespace (`:1185-1189`).
        let mut namespaces: std::collections::BTreeSet<String> =
            self.namespace_owners.keys().cloned().collect();
        for (_, session) in self.sessions_in_order() {
            namespaces.extend(session.extensions.iter().map(|e| e.namespace.clone()));
        }

        for namespace in namespaces {
            // Candidates: sessions advertising this namespace with `ownerEligible` (`:1191-1201`).
            // Collected owned, so the elected winner can be written back under `&mut self` below.
            let mut candidates: Vec<(String, u64, u64)> = self
                .sessions_in_order()
                .filter(|(_, s)| {
                    s.extensions.iter().any(|e| e.namespace == namespace && e.owner_eligible)
                })
                .map(|(id, s)| (id.clone(), s.owner_order, s.conn_id))
                .collect();

            let Some(winner) = ({
                // `candidates.sort((a, b) => ownerOrder, then sessionId.localeCompare)` (`:1220-1226`).
                // The id tie-break is unreachable — `owner_order` comes from a monotonic counter and
                // is unique per LIVE session — so byte order stands in for `localeCompare` with no
                // reachable difference; it is ported for shape, not for effect.
                candidates.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
                candidates.first().cloned()
            }) else {
                // `if (this.namespaceOwners.delete(namespace))` (`:1203-1213`): only a namespace
                // that WAS owned announces its vacancy, and only to capable sessions.
                if self.namespace_owners.remove(&namespace).is_some() {
                    self.notify_namespace_capable(&namespace, ExtensionOwnerRef::default());
                }
                continue;
            };
            let (winner_id, _, winner_conn) = winner;

            let existing = self.namespace_owners.get(&namespace);
            let owner_changed = existing.is_none_or(|o| o.session_id != winner_id);
            let socket_changed = existing.is_some_and(|o| o.conn_id != winner_conn);
            if !owner_changed && !socket_changed {
                continue;
            }
            let epoch = uuid::Uuid::new_v4().to_string();
            self.namespace_owners.insert(namespace.clone(), NamespaceOwner {
                session_id: winner_id.clone(),
                conn_id: winner_conn,
                epoch: epoch.clone(),
            });
            self.notify_namespace_capable(&namespace, ExtensionOwnerRef {
                owner_id: Some(winner_id),
                owner_epoch: Some(epoch),
            });
        }
    }

    /// The `extension_owner` fan-out both arms of the election share
    /// (`v0.9.2 broker/broker.ts:1205-1211` and `:1243-1257`): every session that advertises the
    /// namespace, in join order.
    ///
    /// [CYRUP-DELTA] The vacancy arm upstream tests `session.extensions?.some(…)` and the
    /// election arm tests `session.extensions?.length && …some(…)`; `.some()` on an empty array is
    /// already `false`, so the two conditions are the same set and one helper serves both.
    fn notify_namespace_capable(&self, namespace: &str, owner: ExtensionOwnerRef) { … }
}
```

`ExtensionOwnerRef` is `Clone`, so the fan-out clones per recipient; that is the same cost
`send_msg` already pays encoding per recipient.

### Step 7 — `broker/session.rs`: store capabilities, advertise the feature, replay owner + state

In [`handle_register`](../../crates/cyrup-intercom/src/broker/session.rs):

1. Keep the existing `extensions_field_is_valid` guard in place, and **capture** the parsed value:
   `let extensions: Vec<ExtensionCapability> = registration.extra.get("extensions")
   .and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();` — the guard has
   already proved every element decodes, so the `ok()` arm is unreachable for a frame that got here.
2. `ConnectedSession { …, owner_order, extensions }` where
   `let owner_order = self.sessions.get(&id).map_or_else(|| { let n = self.next_owner_order;
   self.next_owner_order += 1; n }, |prev| prev.owner_order);` — read **before** the takeover path
   removes anything, matching `previous?.ownerOrder ?? this.nextOwnerOrder++` (`:488`).
3. `features: Some(vec![EXTENSION_BUS_FEATURE.to_string()])` on the `Registered` reply (`:502-506`).
   v0.9.2 advertises that one value only; `EXACT_SEND_FEATURE` is a v0.12.0 addition whose behaviour
   is not ported, so advertising it would be a lie.
4. After the `SessionJoined` broadcast and **before** `flush_mailbox_for_session` (pi's order,
   `:509-510`): `self.recompute_namespace_owners();`.
5. After the mailbox flush, the per-capability replay (`:512-528`), which must go out even when the
   namespace is unowned:

```rust
        // `if (extensions) { for (const ext of extensions) { … } }` (`v0.9.2 broker/broker.ts:512-528`).
        // The `extension_owner` frame is UNCONDITIONAL — an unowned namespace answers with the
        // ownerless `ExtensionOwnerRef::default()`, which is how a joining session learns the
        // namespace exists and has no owner. The `extension_state` frame follows only when a state
        // has ever been committed.
        for namespace in namespaces {
            let owner = self.namespace_owners.get(&namespace).map_or_else(
                ExtensionOwnerRef::default,
                |o| ExtensionOwnerRef {
                    owner_id: Some(o.session_id.clone()),
                    owner_epoch: Some(o.epoch.clone()),
                },
            );
            send_msg(self_tx, &BrokerMessage::ExtensionOwner { namespace: namespace.clone(), owner });
            if let Some(state) = self.extension_state.load_state(&namespace) {
                send_msg(self_tx, &BrokerMessage::ExtensionState {
                    namespace,
                    revision: state.revision,
                    payload: Some(state.payload.clone()),
                });
            }
        }
```

That replay block is byte-identical between `register` and `extension_capabilities_update`
(`:512-528` vs `:570-585`). Factor it once as
`fn replay_extension_state(&mut self, self_tx: &UnboundedSender<Vec<u8>>, namespaces: &[String])` in
`extensions.rs` and call it from both — upstream duplicates it, the port must not.

`handle_unregister` ([`session.rs:141`](../../crates/cyrup-intercom/src/broker/session.rs)) gains
`self.recompute_namespace_owners();` after the `SessionLeft` broadcast, inside the owned-session
branch (pi's `:544`).

### Step 8 — `broker/extensions.rs`: the three handlers

`handle_extension_capabilities_update` becomes `&mut self` and replaces its `tracing::debug!` with
pi's effects (`:565-585`):

```rust
        let Ok(capabilities) = serde_json::from_value::<Vec<ExtensionCapability>>(extensions.clone())
        else {
            return FrameResult::protocol_error();
        };
        let namespaces: Vec<String> = capabilities.iter().map(|c| c.namespace.clone()).collect();
        if let Some(session) = self.sessions.get_mut(current_id) {
            session.extensions = capabilities;      // `session.extensions = extensions` (`:568`)
        }
        self.recompute_namespace_owners();          // `:569`
        self.replay_extension_state(self_tx, &namespaces);   // `:570-585`
        FrameResult::cont()
```

(The handler must now take `self_tx`; update its `dispatch.rs` arm accordingly.)

`handle_extension_publish` — keep the existing two miss branches verbatim, then port
`:1281-1356` in upstream's order. Sketch of the shape and the borrow discipline:

```rust
    pub(super) fn handle_extension_publish(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_id: &Option<String>,
    ) -> FrameResult {
        …
        // `!session.extensions?.length` (`:1277-1280`) — now a REAL test, not a constant.
        let Some(session) = self.sessions.get(current_id).filter(|s| s.conn_id == conn_id) else {
            send_msg(self_tx, &BrokerMessage::Error { error: "Session not found".to_string() });
            return FrameResult::cont();
        };
        if session.extensions.is_empty() {
            send_msg(self_tx, &BrokerMessage::Error {
                error: "Session has not advertised extension capability".to_string(),
            });
            return FrameResult::cont();
        }

        let refuse = |error: &str| { send_msg(self_tx, &BrokerMessage::Error { error: error.to_string() }); FrameResult::cont() };

        // `typeof namespace !== "string" || !validateNamespace(namespace)` (`:1288-1291`).
        let Some(namespace) = value.get("namespace").and_then(|v| v.as_str()).filter(|ns| namespace_is_valid(ns))
        else { return refuse("Invalid namespace") };
        // `audience !== "owner" && audience !== "capable"` (`:1293-1296`).
        let Ok(audience) = serde_json::from_value::<ExtensionAudience>(
            value.get("audience").cloned().unwrap_or(serde_json::Value::Null))
        else { return refuse("Invalid audience") };
        // `msg.ownerOnly === true` — a STRICT equality, so any other value is `false` (`:1285`).
        let owner_only = value.get("ownerOnly") == Some(&serde_json::Value::Bool(true));
        // `serializedPayloadSize(payload)` (`:1298-1302`): absent payload is `null` here, i.e. a refusal.
        let Some(payload_len) = serialize_payload(value.get("payload")).map(|j| j.len())
        else { return refuse("Invalid extension payload or payload exceeds 16 KiB limit") };
        if payload_len > MAX_EXTENSION_MESSAGE_BYTES { return refuse("Invalid extension payload or payload exceeds 16 KiB limit") }
        // `hasCapability` (`:1305-1309`).
        if !session.extensions.iter().any(|e| e.namespace == namespace) {
            return refuse("Sender does not have capability for this namespace");
        }
        …
    }
```

then, per `:1311-1329`: `let owner = self.namespace_owners.get(namespace);` —
`(audience == Owner || owner_only) && owner.is_none()` → `"No owner for this namespace"`; when
`owner_only` and an owner exists, a non-string `ownerEpoch` →
`"ownerEpoch required for owner-only messages"` and any mismatch of
`current_id != owner.session_id || conn_id != owner.conn_id || epoch != owner.epoch` →
`"Owner validation failed"`; and finally the join-ordered fan-out of `:1332-1355`:

```rust
        let owner_ref = owner.map_or_else(ExtensionOwnerRef::default, |o| ExtensionOwnerRef {
            owner_id: Some(o.session_id.clone()),
            owner_epoch: Some(o.epoch.clone()),
        });
        let payload = value.get("payload").cloned();
        for (id, recipient) in self.sessions_in_order() {
            if !recipient.extensions.iter().any(|e| e.namespace == namespace) {
                continue;
            }
            // `shouldReceive` (`:1344-1348`). Note the publisher is NOT excluded from a `capable`
            // fan-out: pi routes a session's own publish back to it.
            let should_receive = audience == ExtensionAudience::Capable
                || owner.is_some_and(|o| id == &o.session_id && recipient.conn_id == o.conn_id);
            if should_receive {
                send_msg(&recipient.tx, &BrokerMessage::ExtensionMessage {
                    namespace: namespace.to_string(),
                    from_session_id: current_id.to_string(),
                    owner: owner_ref.clone(),
                    payload: payload.clone(),
                });
            }
        }
        FrameResult::cont()
```

`handle_extension_state_commit` — same treatment against `:1367-1495`. Every exit is an
`ExtensionStateResult`; keep [`js_string_or_empty`](../../crates/cyrup-intercom/src/broker/js.rs)
for the two pre-type-check echoes and use `String(namespace)` (not `String(namespace || "")`) for
the `"Invalid namespace"` arm, which is what `:1394` actually writes — the difference is observable
for `namespace: 0`, which echoes `"0"` there and `""` in the two earlier branches. A tiny closure
over the repeated frame keeps the seven refusals readable:

```rust
        // Every refusal past the namespace check reports the CURRENT revision, not 0 (`:1409`,
        // `:1420`, `:1432`, `:1445`, `:1457`, `:1469`).
        let mut refuse = |state: &mut ExtensionStateManager, reason: &str| {
            send_msg(self_tx, &BrokerMessage::ExtensionStateResult {
                namespace: namespace.to_string(),
                committed: false,
                revision: state.current_revision(namespace),
                reason: Some(reason.to_string()),
            });
            FrameResult::cont()
        };
```

with the guards in upstream's order: `ownerEpoch` must be a string → `"Invalid ownerEpoch"`;
`js_safe_u64(value.get("expectedRevision"))` → `"Invalid expectedRevision"`;
`serialize_payload(...)` present and `<= MAX_EXTENSION_STATE_BYTES` →
`"Invalid extension state or payload exceeds 64 KiB limit"`; sender capability →
`"Sender does not have capability for this namespace"`; an owner must exist →
`"No owner for this namespace"`; session + conn + epoch must all match →
`"Owner validation failed"`. Then `commit_state`, the result frame to the committer (`reason` from
`StateCommitResult::reason`, `None` on success), and — only when committed — the
`BrokerMessage::ExtensionState { namespace, revision, payload }` fan-out to every capable session in
join order, the committer included (`:1484-1495`).

### Step 9 — dispatch and the stale comments

- [`dispatch.rs:98-111`](../../crates/cyrup-intercom/src/broker/dispatch.rs): the comment block
  states the bus effects are unported and that not advertising the feature is what keeps these
  frames away. Both halves are now false — rewrite it to name the three handlers and the feature the
  broker now advertises, and thread `value`/`self_tx` into the two arms whose signatures changed.
- [`extensions.rs:1-11`](../../crates/cyrup-intercom/src/broker/extensions.rs) module doc: replace
  "cyrup does not implement the bus … the bus EFFECTS stay unported" with what the module now is —
  capabilities, owner election, publish fan-out, state commit — keeping the existing paragraph
  explaining why `extensions_field_is_valid` lives here.
- The three handler doc comments: every "cyrup does not implement the extension bus, so …"
  paragraph must go; keep the citations and the reachability notes (the takeover race that makes
  `session_owns_connection` reachable is still exactly right).
- [`transport/protocol.rs:84-88`](../../crates/cyrup-intercom/src/transport/protocol.rs): "cyrup
  never advertises it, and that is load-bearing" is now inverted — the broker advertises it, and
  that is what admits the frames.
- [`transport/client.rs:906-911`](../../crates/cyrup-intercom/src/transport/client.rs): the
  "Unreachable in practice" note stays TRUE (cyrup's `SessionRegistration` still advertises no
  `extensions`, so a cyrup client is never routed one) but its stated reason — "pi's broker only
  routes these to a session that advertised `extensions`" — must now say cyrup's broker does too.

## Definition of Done

- A `register` frame carrying `session.extensions` is answered with
  `{"type":"registered","sessionId":…,"features":["extension-bus-v1"]}`, then one
  `extension_owner` frame per advertised namespace — carrying `ownerId`/`ownerEpoch` when the
  namespace is owned and neither field when it is not — and an `extension_state` frame for each
  namespace that has a committed state, carrying its revision and payload.
- Two sessions advertising the same namespace with `ownerEligible: true`: the one that registered
  first is the owner, both receive an `extension_owner` naming it, and the second receives no new
  owner frame on its own join. When the owner disconnects or unregisters, the survivor receives an
  `extension_owner` naming itself with a **new** `ownerEpoch`; when the last eligible session leaves,
  every still-capable session receives an `extension_owner` with no owner fields. A session that
  re-registers under the same id keeps its original election order and does not displace an owner
  elected after it.
- `extension_publish` with `audience: "capable"` reaches every session advertising that namespace,
  the publisher included, as `extension_message` carrying `fromSessionId` and the current owner
  pair; with `audience: "owner"` it reaches only the owning session on the owning connection. A
  session that advertised nothing is never a recipient.
- `extension_publish` refuses, with upstream's exact `error` strings and without closing the
  connection: an unadvertised sender, a malformed namespace, an audience other than
  `owner`/`capable`, an absent or >16 KiB payload, a namespace the sender did not advertise, an
  `owner`/`ownerOnly` publish with no owner, an `ownerOnly` publish with no string `ownerEpoch`, and
  an `ownerOnly` publish whose session/connection/epoch does not match the elected owner.
- `extension_state_commit` by the owner at the current revision answers
  `{"committed":true,"revision":<previous+1>}` with no `reason`, and every capable session — the
  committer included — receives an `extension_state` at that revision with that payload.
- A commit at a stale revision answers `committed: false` with `reason: "Revision mismatch"` and the
  **current** revision, no state changes, and no `extension_state` is broadcast. A commit by a
  non-owner, with a mismatched epoch, on a superseded connection, with a >64 KiB payload, with an
  absent payload, or with a negative/fractional/unsafe `expectedRevision` is likewise refused with
  upstream's reason and the current revision.
- Committed state survives a broker restart: a new broker process replays the last committed
  revision and payload to a session that advertises the namespace, reading
  `<intercomDir>/extension-state/<sha256(namespace)>.json` — a 0600 file inside a 0700 directory
  whose `payloadSha256` matches its payload. Corrupting that file makes the broker fall back to the
  `.bak` and still replay the last good revision; corrupting both makes it replay nothing and report
  revision `0`, rather than failing to start.
- An unwritable state directory does not stop the broker: owner election and publish fan-out still
  work and a commit is refused with `"Failed to persist extension state"`.
- No `.tmp.` file remains in the state directory after a commit, successful or failed.
- A malformed `extensions` field on `register` or `extension_capabilities_update` still destroys the
  connection, and `namespace_is_valid` still accepts exactly `^[a-z0-9][a-z0-9._/-]{0,63}$`.
