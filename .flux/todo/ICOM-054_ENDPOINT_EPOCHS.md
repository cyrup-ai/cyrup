---
stage: aug
status: done
updated: 2026-08-27 22:41
---

# Port broker-owned endpoint epochs so stale endpoints are refused

> **Upstream parity gap.** `cyrup-intercom` is a port of `pi-intercom` **v0.9.2**; upstream is now
> **v0.12.0** (`ef95f19`, 2026-08-22) at [`nicobailon/pi-intercom`](https://github.com/nicobailon/pi-intercom).
> Reference checkout: [`./tmp/pi-intercom`](../../tmp/pi-intercom). Gap analysis:
> `docs/gap-analysis/11-cyrup-intercom.md` — **ICOM-054**.
> The window `v0.10.1..v0.12.0` (8 commits, 15 files, +2006/−261) was never audited by any prior pass;
> it was found on 2026-08-27 by cloning upstream, which no pass had done before.

---

## 0. READ THIS FIRST — the name collision that will make you think this is already done

`grep -rn epoch crates/cyrup-intercom/src` returns **36 hits. Not one of them is this feature.**

| hit | what it actually is |
|---|---|
| [`src/connect.rs:152`](../../crates/cyrup-intercom/src/connect.rs) `epoch: AtomicU64` (+ `:35-36`, `:409`, `:417`, `:432`) | a **local reconnect-attempt generation counter** inside `ConnectSupervisor`, so a caller that waited on the connect gate can tell whether the attempt it waited for is the one that failed. Never leaves the process. Nothing to do with endpoints. |
| [`src/ui/inline_message.rs:269`](../../crates/cyrup-intercom/src/ui/inline_message.rs) `civil_from_days(days_since_epoch)` (+ `:270`, `:445`) | a **date helper** — days since the Unix epoch. |
| `owner_epoch` / `ownerEpoch` in [`src/transport/protocol.rs:568,771,785`](../../crates/cyrup-intercom/src/transport/protocol.rs) and `ExtensionOwnerRef` | the **extension-bus namespace ownership epoch**, a wholly different upstream concept (`extension_owner`, `extension_publish`, `extension_state_commit`). cyrup does not implement the bus. |
| `as_epoch_ms`, `started_at`, `first-seen epoch-ms`, … | epoch-millisecond timestamps. |

`grep -c endpointEpoch crates/cyrup-intercom/src/transport/protocol.rs` → **0**. The feature is absent.

**Naming rule for this task, so the grep stops lying:**

* The wire field is and must stay `endpointEpoch` — it is upstream's, and it is interop-load-bearing.
  In Rust it is `endpoint_epoch` on [`SessionInfo`](../../crates/cyrup-intercom/src/transport/protocol.rs)
  under the struct's existing `#[serde(rename_all = "camelCase")]`.
* Everything *else* this task introduces is named after **`target` / `delivery`**, never after
  "epoch": `ExactTarget`, `target_id`, `target_epoch`, `DeliveryState`, `DeliveryRecord`,
  `DeliveryFingerprint`, `delivery_records`, `EXACT_SEND_FEATURE`. Upstream calls the feature
  **`exact-send-v1`**, not "epochs" — follow that.
* **Rename the collider.** As part of this task, rename `ConnectSupervisor`'s local counter in
  [`src/connect.rs`](../../crates/cyrup-intercom/src/connect.rs): `epoch` → `reconnect_generation`,
  `epoch_at_start` (`:409`) → `generation_at_start`, and reword the two doc lines at `:35-36`
  ("attempt *epoch*" → "attempt *generation*"). Five identifiers and two comment lines, no behaviour
  change. The crate already uses "generation" for exactly this idea
  (`ConnectSupervisor::generation()`, `message_generation` in `inbound.rs`), so this is convergence,
  not invention. After it, `grep endpoint_epoch` and `grep epoch` both mean what they say.

**Interop is not broken today, and that is why this is inert rather than fatal.**
[`SessionInfo`](../../crates/cyrup-intercom/src/transport/protocol.rs) carries
`#[serde(flatten)] pub extra: UnknownFields` at `:296-299`, so a v0.11+ peer's `endpointEpoch`
survives a cyrup hop losslessly, and cyrup's own `registered` frame simply advertises no features so
a conforming pi client never sends `targetId`/`targetEpoch`. Nothing crashes. What is missing is the
**refusal**: cyrup will happily accept a send against an endpoint the broker has already replaced,
ack it `delivered`, and route it to whatever the *name* resolves to now — or to nothing. The user's
report is "the message just never arrived", with every visible surface saying it worked.

**Upstream is explicit that two behaviours must not change**, and the commit body says so:
"…so stale endpoints are refused **while replies and mailbox recovery keep their existing
behavior**." Both are already correct in cyrup and both have regression coverage. Do not disturb
them; §5 says exactly what that means.

---

## 1. What upstream does

Commit `636f61e` (issue #109, v0.11.0) — *"feat: bind intercom sends to endpoint epochs"*,
+422/−107 across `broker/broker.ts`, `broker/client.ts`, `broker/protocol.ts`, `index.ts`, `types.ts`.
Read it with `git -C tmp/pi-intercom show 636f61e`. It is three interlocking mechanisms.

### 1a. The broker mints an endpoint epoch on every `register`

[`tmp/pi-intercom/types.ts:14-16`](../../tmp/pi-intercom/types.ts):

```ts
export interface SessionInfo {
  id: string;
  /** Broker-owned lifetime of this live endpoint. */
  endpointEpoch?: string;
```

and `SessionRegistration` is widened to omit it, so a client can never supply one
([`types.ts:102`](../../tmp/pi-intercom/types.ts)):

```ts
export type SessionRegistration = Omit<SessionInfo, "id" | "endpointEpoch" | "peerUid" | "trustedLocal"> & {
```

The broker stamps a fresh `randomUUID()` at [`broker/broker.ts:464-466`](../../tmp/pi-intercom/broker/broker.ts):

```ts
        const info: SessionInfo = {
          id,
          endpointEpoch: randomUUID(),
```

The id may be re-adopted across a takeover; **the epoch may not**. That is the whole idea: the id
names the identity, the epoch names *this particular live socket binding of it*.

It is advertised as a negotiated feature, [`types.ts:2`](../../tmp/pi-intercom/types.ts) and
[`broker/broker.ts:498-502`](../../tmp/pi-intercom/broker/broker.ts):

```ts
export const EXACT_SEND_FEATURE = "exact-send-v1";
```
```ts
          features: [EXTENSION_BUS_FEATURE, EXACT_SEND_FEATURE],
```

and `isSessionInfo` grows one guard, [`broker/protocol.ts:142-144`](../../tmp/pi-intercom/broker/protocol.ts):

```ts
  if (value.endpointEpoch !== undefined && typeof value.endpointEpoch !== "string") {
    return false;
  }
```

**And one line is deleted.** At `broker.ts`'s register-time takeover, `636f61e` removes
`this.clearAskEdgesForSession(id);` — leaving `clearAskEdgesForSession` with **zero call sites** at
v0.12.0 (only the dead definition at `broker/broker.ts:1176` survives). This is the "replies keep
their existing behavior" half: once the epoch refuses stale *sends*, wiping the replaced endpoint's
ask edges is no longer needed and would break a reply that arrives after a stable-id replacement.

### 1b. A send may name an exact target, and a superseded one is refused

[`types.ts:111`](../../tmp/pi-intercom/types.ts):

```ts
  | { type: "send"; to: string; message: Message; targetId?: string; targetEpoch?: string }
```

[`broker/broker.ts:625-653`](../../tmp/pi-intercom/broker/broker.ts) — placed after the reply-edge
lookup and **before** `findSessions`, so a valid exact target *replaces* name routing:

```ts
        const hasTargetId = clientMessage.targetId !== undefined;
        const hasTargetEpoch = clientMessage.targetEpoch !== undefined;
        if (
          hasTargetId !== hasTargetEpoch
          || (hasTargetId && (typeof clientMessage.targetId !== "string" || clientMessage.targetId.length === 0))
          || (hasTargetEpoch && (typeof clientMessage.targetEpoch !== "string" || clientMessage.targetEpoch.length === 0))
        ) {
          this.writeDeliveryFailure(socket, message.id, "Exact target requires an id and endpoint epoch", "E_INVALID_TARGET");
          break;
        }
        if (hasTargetId && hasTargetEpoch) {
          const targetId = clientMessage.targetId as string;
          const targetEpoch = clientMessage.targetEpoch as string;
          const fingerprint = this.deliveryFingerprint(message, targetId);
          if (this.replayOrReject(socket, currentKey, message.id, fingerprint)) {
            break;
          }
          const exactTarget = this.sessions.get(targetId);
          if (!exactTarget || exactTarget.info.endpointEpoch !== targetEpoch) {
            this.recordDelivery(currentKey, message.id, fingerprint, "failed", "Target endpoint changed before delivery", "E_TARGET_REBOUND", true);
            this.writeDeliveryFailure(socket, message.id, "Target endpoint changed before delivery", "E_TARGET_REBOUND", true);
            break;
          }
          clientMessage.to = targetId;
        }
```

(The v0.12.0 file splits the missing-target case into its own `E_TARGET_NOT_FOUND` arm; that split
belongs to the scoped-routing commit `089b631` — **ICOM-055** — not here. Port `636f61e`'s single
combined arm above.)

Note the malformed-field arm: an empty-string or half-supplied pair is a `delivery_failed`, **not**
a `socket.destroy`, and it must **not** silently fall back to name routing.

Every `delivery_failed` in the file is rewritten through two helpers, and every existing reason
string keeps its exact wording while gaining a code
([`broker/broker.ts:1035-1041`](../../tmp/pi-intercom/broker/broker.ts)):

```ts
  private writeDeliverySuccess(socket: net.Socket, messageId: string, delivery: "socket_delivered" | "queued"): void {
    writeMessage(socket, { type: "delivered", messageId, delivery, retryable: false, outcomeKnown: true });
  }

  private writeDeliveryFailure(socket: net.Socket, messageId: string, reason: string, code: string, retryable = false): void {
    writeMessage(socket, { type: "delivery_failed", messageId, reason, delivery: "failed", code, retryable, outcomeKnown: true });
  }
```

The nine codes, each bound to a reason string that is unchanged from v0.10.1:
`E_INVALID_MESSAGE`, `E_INVALID_TARGET`, `E_TARGET_REBOUND`, `E_REPLY_TARGET`, `E_SENDER_NOT_FOUND`,
`E_SUPERSEDE_TARGET`, `E_MUTUAL_ASK`, `E_AMBIGUOUS_TARGET`, `E_TARGET_DISCONNECTED`,
`E_TARGET_NOT_FOUND`, plus `E_MESSAGE_ID_REUSE` from §1c and the four record-only codes
`E_DELIVERY_SUPERSEDED` / `E_DELIVERY_CANCELLED` / `E_DELIVERY_EXPIRED` / `E_DELIVERY_EVICTED`.

**`cancel_message`'s two acks are deliberately left bare** — `writeMessage(socket, { type: "delivered", messageId })`
at [`broker/broker.ts:829,856`](../../tmp/pi-intercom/broker/broker.ts) and the plain
`delivery_failed { reason: "Message cannot be cancelled by this session" }` at `:835-839`. They carry
no `delivery`/`code`/`retryable`/`outcomeKnown` and the client's defaults fill them in. Port that
asymmetry exactly; it is why the new envelope fields must be optional on the wire.

### 1c. Bounded delivery records give exactly-once-per-authored-content

[`broker/broker.ts:44-46,65-74`](../../tmp/pi-intercom/broker/broker.ts):

```ts
const DELIVERY_RECORD_RETENTION_MS = 60 * 60 * 1000;
const MAX_DELIVERY_RECORDS = 4096;
```
```ts
interface DeliveryRecord {
  fingerprint: string;
  state: DeliveryState;
  reason?: string;
  code?: string;
  retryable: boolean;
  outcomeKnown: boolean;
  createdAt: number;
}
```

[`broker/broker.ts:1043-1110`](../../tmp/pi-intercom/broker/broker.ts):

```ts
  private deliveryFingerprint(message: Message, targetId: string): string {
    return JSON.stringify({
      targetId,
      text: message.content.text,
      attachments: message.content.attachments,
      replyTo: message.replyTo,
      expectsReply: message.expectsReply,
      supersedes: message.supersedes,
      retryOf: message.retryOf,
    });
  }

  private deliveryRecordKey(fromSessionId: string, messageId: string): string {
    return JSON.stringify([fromSessionId, messageId]);
  }

  private replayOrReject(socket: net.Socket, fromSessionId: string, messageId: string, fingerprint: string): boolean {
    this.pruneDeliveryRecords();
    const record = this.deliveryRecords.get(this.deliveryRecordKey(fromSessionId, messageId));
    if (!record) return false;
    if (record.fingerprint !== fingerprint) {
      this.writeDeliveryFailure(socket, messageId, "Message id was reused with different authored content", "E_MESSAGE_ID_REUSE");
      return true;
    }
    if (record.code === "E_TARGET_REBOUND" && record.retryable) {
      return false;
    }
    if (record.state === "socket_delivered" || record.state === "queued") {
      this.writeDeliverySuccess(socket, messageId, record.state);
    } else {
      this.writeDeliveryFailure(socket, messageId, record.reason ?? "Previous delivery failed", record.code ?? "E_DELIVERY_FAILED", record.retryable);
    }
    return true;
  }
```

with FIFO eviction at the cap and TTL pruning
([`broker/broker.ts:1079-1110`](../../tmp/pi-intercom/broker/broker.ts)), and in-place status
updates (`updateDeliveryRecord`) wired to five later lifecycle events: supersede (`:709`), cancel of
queued mail (`:823`), cancel of delivered mail (`:856`), mailbox expiry (`:1005`), mailbox eviction
(`:1021`), and mailbox flush → `socket_delivered` (`:1162`).

`replayOrReject` runs on **all three** send arms — the exact-target block, the live-target arm
(`:664`) and the disconnected/mailbox arm (`:740`). So the guarantee is not scoped to exact sends:
**any** re-send of a message id with the same authored content replays the recorded outcome without
re-injecting the message; **any** re-send of that id with *different* content is refused
`E_MESSAGE_ID_REUSE`. The single escape hatch is the `E_TARGET_REBOUND && retryable` arm — which is
what lets the client retry a rebound target *under the same message id*.

### 1d. The client resolves an exact target and retries a rebound one exactly once

[`broker/client.ts:645-690`](../../tmp/pi-intercom/broker/client.ts):

```ts
    const sendOnce = (targetId?: string, targetEpoch?: string): Promise<SendResult> => new Promise((resolve, reject) => {
      …
        writeMessage(socket, { type: "send", to, message, ...(targetId && targetEpoch ? { targetId, targetEpoch } : {}) });
      …
    });

    if (!this.supportsFeature(EXACT_SEND_FEATURE) || options.replyTo) {
      return sendOnce();
    }

    const resolveTarget = async (): Promise<{ id: string; epoch: string } | null> => {
      const sessions = await this.listSessions();
      const byId = sessions.find((session) => session.id === to);
      const byName = byId ? [] : sessions.filter((session) => session.name?.toLowerCase() === to.toLowerCase());
      const byPrefix = byId || byName.length > 0 ? [] : sessions.filter((session) => session.id.startsWith(to));
      const matches = byId ? [byId] : byName.length > 0 ? byName : byPrefix;
      const target = matches.length === 1 ? matches[0]! : null;
      return target?.endpointEpoch ? { id: target.id, epoch: target.endpointEpoch } : null;
    };

    const target = await resolveTarget();
    if (!target) return sendOnce();
    const result = await sendOnce(target.id, target.epoch);
    if (result.code !== "E_TARGET_REBOUND") return result;
    const reboundTarget = await resolveTarget();
    return reboundTarget ? sendOnce(reboundTarget.id, reboundTarget.epoch) : result;
```

Four load-bearing details: a **reply is never exact-sent** (`|| options.replyTo` — that is the other
half of "replies keep their existing behavior"); the retry **reuses the same message id**; there is
**exactly one** retry; and a target with no `endpointEpoch` (an older broker) degrades silently to a
plain send.

### 1e. The ack envelope grows four fields

[`types.ts:4-11,140-141`](../../tmp/pi-intercom/types.ts):

```ts
export type DeliveryState = "socket_delivered" | "queued" | "failed" | "unknown";

export interface DeliveryDetails {
  delivery: DeliveryState;
  code?: string;
  retryable: boolean;
  outcomeKnown: boolean;
}
```
```ts
  | ({ type: "delivered"; messageId: string } & DeliveryDetails)
  | ({ type: "delivery_failed"; messageId: string; reason: string } & DeliveryDetails)
```

The client's two guards are **asymmetric acceptance sets**, and that asymmetry is the whole
validation contract ([`broker/client.ts:375,392`](../../tmp/pi-intercom/broker/client.ts)):
`delivered` accepts `delivery ∈ {undefined, "socket_delivered", "queued"}`; `delivery_failed`
accepts `delivery ∈ {undefined, "failed", "unknown"}`. Anything else throws → `socket.destroy()`.
Absent fields default to `socket_delivered` / `failed`, `retryable = false`, `outcomeKnown = true`.

Finally [`index.ts:91-100`](../../tmp/pi-intercom/index.ts) threads the details into every tool
result, and `:2170` replaces the old ternary with `deliveryState = sendResult.delivery;`.

---

## 2. What already exists in the port and must be reused

**Do not build any of this again.** Every mechanism this commit needs has a counterpart in the port,
usually with a doc comment explaining why it is shaped the way it is.

| upstream mechanism | reuse this |
|---|---|
| `randomUUID()` for a broker-owned id | `uuid::Uuid::new_v4().to_string()`, already how [`session.rs:41`](../../crates/cyrup-intercom/src/broker/session.rs) mints a session id |
| both-or-neither optional wire pair | [`ExtensionOwnerRef`](../../crates/cyrup-intercom/src/transport/protocol.rs) at `:562-602` — the exact shape `targetId`/`targetEpoch` needs, including the `#[serde(flatten)]` + `present_non_null` idiom (see §3b for where the *rule* goes) |
| `Map` insertion-order eviction | `sessions` + `session_order: Vec<String>` and `insert_session` / `remove_session` in [`state.rs`](../../crates/cyrup-intercom/src/broker/state.rs); also `unregistered: Vec<u64>` + `mark_unregistered`. The delivery-record store is the same pattern, third instance |
| TTL prune over a broker map | `prune_message_receipt_routes` / `prune_disconnected_sessions` / `prune_mailbox_messages` — all `retain` with `now.saturating_sub(created_at) > RETENTION` |
| FIFO eviction at a cap | `queue_mailbox_message`'s `while len >= MAX_MAILBOX_MESSAGES` in [`mailbox.rs`](../../crates/cyrup-intercom/src/broker/mailbox.rs) |
| ported constants block | [`limits.rs`](../../crates/cyrup-intercom/src/broker/limits.rs) — add the two new ones there, with citations, like every other |
| `findSessions` id→name→prefix ladder | [`broker::routing::find_session_ids`](../../crates/cyrup-intercom/src/broker/routing.rs). It is `pub(crate) mod routing`, so the **client** can call it — and it returns `Vec<String>`, so upstream's `matches.length === 1 ? matches[0] : null` is `match v.as_slice() { [only] => …, _ => None }`, the same shape `find_unique_live_session_for_disconnected_session` already uses |
| feature negotiation on `registered` | `BrokerMessage::Registered { features }` already exists in [`protocol.rs:804-813`](../../crates/cyrup-intercom/src/transport/protocol.rs); the broker currently sends `features: None` on purpose (it does not implement the extension bus) — see [`dispatch.rs:99`](../../crates/cyrup-intercom/src/broker/dispatch.rs) and [`extensions.rs:4`](../../crates/cyrup-intercom/src/broker/extensions.rs) |
| per-message ack correlation | `pending_sends: Mutex<HashMap<String, oneshot::Sender<SendResult>>>` in [`client.rs:138`](../../crates/cyrup-intercom/src/transport/client.rs) — already keyed by message id, so a retry under the same id needs no new plumbing |
| last-known delivery state for the ask timeout | `SessionState::latest_delivery_state` ([`session_state.rs:247`](../../crates/cyrup-intercom/src/session_state.rs)), read at `:838-839` with the hardcoded `"socket_delivered"` fallback that `:2170` replaces |
| tool result carrying `delivered: false` details | [`cancel.rs:44-52`](../../crates/cyrup-intercom/src/tools/intercom/cancel.rs) already emits such a details map through `detailed_result` |
| additive-field tolerance | `#[serde(flatten)] extra: UnknownFields` on `SessionInfo`, `Message`, `MessageContent`, `SessionRegistration` |

Two behaviours are **already correct and already pinned**, and this commit is what upstream did to
keep them correct — leave them alone:

* **Replies survive a peer's reconnect.** `on_connection_closed` deliberately does not clear ask
  edges ([`state.rs`](../../crates/cyrup-intercom/src/broker/state.rs), the long comment on that fn),
  and `flush_mailbox_for_session` re-points a parked ask's edge at the session that received it
  ([`mailbox.rs`](../../crates/cyrup-intercom/src/broker/mailbox.rs)).
* **Mailbox recovery.** The whole of [`mailbox.rs`](../../crates/cyrup-intercom/src/broker/mailbox.rs)
  — park, identity match on name+cwd, flush on re-register — is ported and correct. This task adds
  *records about* those deliveries and changes nothing about the deliveries themselves.

---

## 3. Required implementation

Prescriptive. Every choice below is made; there is no menu.

### 3a. `transport/protocol.rs` — the wire model

1. Add the feature constant beside `EXTENSION_BUS_FEATURE` (`:81-89`), with the same citation style:

```rust
/// `EXACT_SEND_FEATURE = "exact-send-v1"` (`v0.11.0 types.ts:2`) — the feature a broker advertises
/// on `registered` to tell a client it mints [`SessionInfo::endpoint_epoch`] and honours the
/// `targetId`/`targetEpoch` pair on `send`. Gated exactly like the bus feature: a conforming client
/// that does not see it never sends the pair (`v0.11.0 broker/client.ts:671`).
pub const EXACT_SEND_FEATURE: &str = "exact-send-v1";
```

2. Add `endpoint_epoch` to `SessionInfo`, immediately after `id` (upstream's declaration order):

```rust
    /// `endpointEpoch` (`v0.11.0 types.ts:14-16`): "Broker-owned lifetime of this live endpoint."
    ///
    /// Minted fresh by the broker on EVERY `register` (`v0.11.0 broker/broker.ts:466`), including a
    /// stable-id takeover — the id names the identity, this names the particular socket binding of
    /// it. Never supplied by a client: `SessionRegistration` omits it upstream (`types.ts:102`) and
    /// does not model it here.
    ///
    /// `[NON-NULL]`, per `isSessionInfo`'s new guard (`v0.11.0 broker/protocol.ts:142-144`):
    /// absent is legal (a v0.9.2 broker mints none), a non-string is fatal.
    #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
    pub endpoint_epoch: Option<String>,
```

3. Add the delivery vocabulary. Model the client's **asymmetric** acceptance sets as two narrow wire
   enums plus one wide crate-facing enum — this is the `ExtensionOwnerRef` discipline (encode the
   guard in the type, so serde cannot be looser or stricter than pi):

```rust
/// `DeliveryState` (`v0.11.0 types.ts:4`) — the crate-facing union of both acks' states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Handed to a live target's socket.
    SocketDelivered,
    /// Parked in the broker mailbox for a disconnected target.
    Queued,
    /// Refused; `code` says why.
    Failed,
    /// The outcome is genuinely not known. The BROKER never emits this — it is what
    /// [`crate::transport::client`] reports when a connection drops with sends in flight, the one
    /// case pi answers by rejecting the promise instead of resolving it.
    Unknown,
}

/// The `delivered` ack's acceptance set (`v0.11.0 broker/client.ts:375`): `socket_delivered` or
/// `queued` ONLY — `"failed"` on a `delivered` frame is fatal upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveredState { SocketDelivered, Queued }

/// The `delivery_failed` ack's acceptance set (`v0.11.0 broker/client.ts:392`): `failed` or
/// `unknown` ONLY.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailedState { Failed, Unknown }
```

   Give each a `From<…> for DeliveryState`.

4. Widen the two ack variants of `BrokerMessage` (`:848-859`). **All four new fields are optional on
   the wire** — because upstream's own `cancel_message` arms emit bare acks
   (`broker/broker.ts:829,835-839,856`), and because a v0.9.2 broker emits none:

```rust
    Delivered {
        message_id: String,
        /// `v0.11.0 types.ts:140`. Absent ⇒ `socket_delivered` (`broker/client.ts:386`), which is
        /// what `cancel_message`'s bare ack (`broker/broker.ts:829,856`) relies on.
        #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
        delivery: Option<DeliveredState>,
        #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
        retryable: Option<bool>,
        #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
        outcome_known: Option<bool>,
    },
    DeliveryFailed {
        message_id: String,
        reason: String,
        #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
        delivery: Option<FailedState>,
        #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
        retryable: Option<bool>,
        #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
        outcome_known: Option<bool>,
    },
```

   This touches every `BrokerMessage::Delivered { .. }` / `DeliveryFailed { .. }` construction site:
   [`broker/send.rs`](../../crates/cyrup-intercom/src/broker/send.rs) (8),
   [`broker/receipts.rs`](../../crates/cyrup-intercom/src/broker/receipts.rs) (3),
   [`transport/client.rs`](../../crates/cyrup-intercom/src/transport/client.rs) (match arms + tests).
   Add `Delivered::ok(message_id, DeliveredState)` and a bare `Delivered::bare(message_id)`
   constructor next to the enum rather than repeating four `Some(...)`s at every site.

### 3b. `transport/protocol.rs` — `ClientMessage::Send` grows the exact-target pair

```rust
/// The optional exact-target pair on `send` (`v0.11.0 types.ts:111`).
///
/// Shaped like [`ExtensionOwnerRef`] — one flattened type owning both halves — but with the
/// PAIRING RULE DELIBERATELY LEFT OUT OF `Deserialize`, and that difference is upstream's, not a
/// shortcut. pi checks `ownerId`/`ownerEpoch` inside a TYPE GUARD, so a half-set pair destroys the
/// connection; it checks `targetId`/`targetEpoch` inside `case "send"` (`broker/broker.ts:625-633`)
/// and answers a half-set or empty-string pair with a `delivery_failed` carrying
/// `E_INVALID_TARGET`. Enforcing it here would make cyrup fatally stricter than pi on a frame pi
/// merely refuses. The rule lives in [`crate::broker`]'s send handler, where pi puts it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactTarget {
    #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
    pub target_epoch: Option<String>,
}
```

and on the variant:

```rust
    Send {
        to: String,
        message: Message,
        /// Bind this send to one exact live endpoint; both halves or neither
        /// (`v0.11.0 types.ts:111`). `Default` (both `None`) serialises to nothing, which is the
        /// v0.9.2 frame byte-for-byte.
        #[serde(flatten)]
        target: ExactTarget,
    },
```

### 3c. `broker/limits.rs` — two constants

```rust
/// `DELIVERY_RECORD_RETENTION_MS = 60 * 60 * 1000` (`v0.11.0 broker/broker.ts:45`).
pub(super) const DELIVERY_RECORD_RETENTION_MS: u64 = 60 * 60 * 1000;
/// `MAX_DELIVERY_RECORDS = 4096` (`v0.11.0 broker/broker.ts:46`).
pub(super) const MAX_DELIVERY_RECORDS: usize = 4096;
```

### 3d. `broker/session.rs` — mint the epoch, advertise the feature, drop the edge-clear

Three edits inside `handle_register`:

```rust
        let info = SessionInfo {
            id: id.clone(),
            // `endpointEpoch: randomUUID()` (`v0.11.0 broker/broker.ts:466`). Minted on EVERY
            // register, takeover included: a re-registered id is a NEW endpoint, and that is the
            // only fact that makes a stale send detectable.
            endpoint_epoch: Some(uuid::Uuid::new_v4().to_string()),
            name: registration.name,
            …
```

```rust
        // `features: [EXTENSION_BUS_FEATURE, EXACT_SEND_FEATURE]` (`v0.11.0 broker/broker.ts:498-502`)
        // minus the bus, which this broker does not implement and therefore must not advertise
        // (see `broker/extensions.rs`). Advertising `exact-send-v1` is what unlocks the client's
        // `targetId`/`targetEpoch` path (`v0.11.0 broker/client.ts:671`).
        send_msg(self_tx, &BrokerMessage::Registered {
            session_id: id.clone(),
            features: Some(vec![EXACT_SEND_FEATURE.to_string()]),
        });
```

and **delete** `self.clear_ask_edges_for_session(&id);` from the takeover branch, replacing the
comment above it:

```rust
        if previous_conn.is_some() {
            // Identity takeover (`v0.11.0 broker/broker.ts:450-456`). `636f61e` REMOVED
            // `clearAskEdgesForSession(id)` from this branch, leaving that method with zero call
            // sites upstream (`broker/broker.ts:1176` is dead code at v0.12.0). The endpoint epoch
            // is what now invalidates a stale send, so wiping the replaced endpoint's ask edges is
            // both unnecessary and harmful: it would refuse a reply that arrives after a stable-id
            // replacement — exactly the case the commit body means by "replies … keep their
            // existing behavior". Receipt routes still go, because they key a
            // cancel/supersede against a socket that is gone.
            self.clear_message_receipt_routes_for_session(&id);
```

`clear_ask_edges_for_session` in [`state.rs`](../../crates/cyrup-intercom/src/broker/state.rs) then
has no callers — delete the method too, and update the paragraph in `on_connection_closed`'s comment
that cites "exactly one call site … the register-time identity takeover" to record that `636f61e`
removed it.

### 3e. `broker/state.rs` — the bounded delivery-record store

New module `broker/delivery.rs` (register it in [`broker/mod.rs`](../../crates/cyrup-intercom/src/broker/mod.rs)
beside `mailbox`), holding the record type and the four methods as an `impl BrokerState` block —
same layout rule the file's `## Layout` header already states.

```rust
//! Bounded per-sender delivery records (`v0.11.0 broker/broker.ts:44-46,65-74,1043-1110`).
//!
//! One record per `(sender session id, message id)`, so a re-sent id either REPLAYS its recorded
//! outcome (identical authored content) or is refused (different content) instead of being
//! delivered twice. Bounded two ways, both upstream's: a 1 h TTL and a 4096-entry FIFO cap.

use std::collections::HashMap;

use crate::transport::protocol::{Attachment, DeliveryState, Message};

use super::limits::{DELIVERY_RECORD_RETENTION_MS, MAX_DELIVERY_RECORDS};

/// `deliveryFingerprint` (`v0.11.0 broker/broker.ts:1043-1053`) — the AUTHORED content of a send,
/// the part a resend must not change.
///
/// Upstream `JSON.stringify`s a fixed-key object; this is a struct compared with `==` because the
/// fingerprint never crosses the wire, so byte-identity with JS's serialisation is not a
/// requirement and structural equality is stronger (no separator ambiguity, no key-order hazard).
/// The FIELD SET is upstream's exactly — note it takes `content.text` and `content.attachments`
/// individually rather than the whole `MessageContent`, so a differing `#[serde(flatten)] extra`
/// on the content object is NOT a fingerprint change, matching pi.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DeliveryFingerprint {
    target_id: String,
    text: String,
    attachments: Option<Vec<Attachment>>,
    reply_to: Option<String>,
    expects_reply: Option<bool>,
    supersedes: Option<String>,
    retry_of: Option<String>,
}

impl DeliveryFingerprint {
    pub(super) fn of(message: &Message, target_id: &str) -> Self {
        Self {
            target_id: target_id.to_string(),
            text: message.content.text.clone(),
            attachments: message.content.attachments.clone(),
            reply_to: message.reply_to.clone(),
            expects_reply: message.expects_reply,
            supersedes: message.supersedes.clone(),
            retry_of: message.retry_of.clone(),
        }
    }
}

/// `interface DeliveryRecord` (`v0.11.0 broker/broker.ts:65-74`).
pub(super) struct DeliveryRecord {
    pub(super) fingerprint: DeliveryFingerprint,
    pub(super) state: DeliveryState,
    pub(super) reason: Option<String>,
    pub(super) code: Option<String>,
    pub(super) retryable: bool,
    pub(super) created_at: u64,
}

/// `deliveryRecordKey` (`:1055-1057`). Upstream needs `JSON.stringify([from, id])` because a JS
/// `Map` key must be a primitive and a naive `from + ":" + id` collides (sender `a:b` + id `c`
/// against sender `a` + id `b:c`). A Rust tuple key is structurally unambiguous, so the escaping
/// problem does not arise.
pub(super) type DeliveryRecordKey = (String, String);
```

Two new `BrokerState` fields, following `sessions`/`session_order` verbatim:

```rust
    /// `deliveryRecords` (`v0.11.0 broker/broker.ts:158`), keyed by `(sender id, message id)`.
    pub(super) delivery_records: HashMap<DeliveryRecordKey, DeliveryRecord>,
    /// Insertion order for `deliveryRecords`, because the cap evicts the OLDEST INSERTED entry
    /// (`this.deliveryRecords.keys().next().value`, `:1082-1085`) and a `HashMap`'s iteration order
    /// is arbitrary. Same device as `session_order` and `unregistered`. `record_delivery` pushes
    /// only on a NEW key, because JS `Map.set` on an existing key keeps its original position.
    pub(super) delivery_record_order: Vec<DeliveryRecordKey>,
```

and the four methods:

```rust
impl BrokerState {
    /// `pruneDeliveryRecords` (`v0.11.0 broker/broker.ts:1097-1101`).
    pub(super) fn prune_delivery_records(&mut self, now: u64) {
        let ttl = DELIVERY_RECORD_RETENTION_MS;
        self.delivery_records
            .retain(|_, r| now.saturating_sub(r.created_at) <= ttl);
        self.delivery_record_order.retain(|k| self.delivery_records.contains_key(k));
    }

    /// `recordDelivery` (`:1079-1095`) — prune, evict oldest-first to the cap, then insert.
    pub(super) fn record_delivery(
        &mut self,
        from: &str,
        message_id: &str,
        fingerprint: DeliveryFingerprint,
        state: DeliveryState,
        reason: Option<&str>,
        code: Option<&str>,
        retryable: bool,
        now: u64,
    ) {
        self.prune_delivery_records(now);
        while self.delivery_records.len() >= MAX_DELIVERY_RECORDS {
            let Some(oldest) = self.delivery_record_order.first().cloned() else { break };
            self.delivery_record_order.remove(0);
            self.delivery_records.remove(&oldest);
        }
        let key = (from.to_string(), message_id.to_string());
        let record = DeliveryRecord {
            fingerprint,
            state,
            reason: reason.map(str::to_string),
            code: code.map(str::to_string),
            retryable,
            created_at: now,
        };
        if self.delivery_records.insert(key.clone(), record).is_none() {
            self.delivery_record_order.push(key);
        }
    }

    /// `updateDeliveryRecord` (`:1103-1110`) — a later lifecycle event overwrites the outcome in
    /// place. A miss is a silent no-op, exactly as upstream's `if (!record) return`. Never touches
    /// the fingerprint, the insertion order, or `created_at`.
    pub(super) fn update_delivery_record(
        &mut self,
        from: &str,
        message_id: &str,
        state: DeliveryState,
        reason: Option<&str>,
        code: Option<&str>,
    ) {
        if let Some(record) =
            self.delivery_records.get_mut(&(from.to_string(), message_id.to_string()))
        {
            record.state = state;
            record.reason = reason.map(str::to_string);
            record.code = code.map(str::to_string);
            record.retryable = false;
        }
    }
}
```

### 3f. `broker/send.rs` — the refusal path

Add a private `replay_or_reject` beside the two handlers (it writes to the sender's socket, so it
belongs with them, not in `delivery.rs`):

```rust
    /// `replayOrReject` (`v0.11.0 broker/broker.ts:1060-1077`). `true` ⇒ this frame is fully
    /// answered; the caller must return without delivering.
    ///
    /// The `E_TARGET_REBOUND && retryable` arm is the ONE hole in the replay rule, and it is what
    /// makes the client's single retry work: the rebound refusal is recorded so a *changed* resend
    /// under that id is still caught by the fingerprint check above it, but an identical resend is
    /// allowed to proceed against the target's new epoch.
    fn replay_or_reject(
        &mut self,
        self_tx: &UnboundedSender<Vec<u8>>,
        from: &str,
        message_id: &str,
        fingerprint: &DeliveryFingerprint,
        now: u64,
    ) -> bool {
        self.prune_delivery_records(now);
        let Some(record) = self.delivery_records.get(&(from.to_string(), message_id.to_string()))
        else {
            return false;
        };
        if &record.fingerprint != fingerprint {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message_id.to_string(),
                reason: "Message id was reused with different authored content".to_string(),
                delivery: Some(FailedState::Failed),
                code: Some("E_MESSAGE_ID_REUSE".to_string()),
                retryable: Some(false),
                outcome_known: Some(true),
            });
            return true;
        }
        if record.code.as_deref() == Some("E_TARGET_REBOUND") && record.retryable {
            return false;
        }
        match record.state {
            DeliveryState::SocketDelivered => {
                send_msg(self_tx, &BrokerMessage::delivered(message_id, DeliveredState::SocketDelivered));
            }
            DeliveryState::Queued => {
                send_msg(self_tx, &BrokerMessage::delivered(message_id, DeliveredState::Queued));
            }
            DeliveryState::Failed | DeliveryState::Unknown => {
                send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                    message_id: message_id.to_string(),
                    reason: record.reason.clone().unwrap_or_else(|| "Previous delivery failed".to_string()),
                    delivery: Some(FailedState::Failed),
                    code: Some(record.code.clone().unwrap_or_else(|| "E_DELIVERY_FAILED".to_string())),
                    retryable: Some(record.retryable),
                    outcome_known: Some(true),
                });
            }
        }
        true
    }
```

Then, in `handle_send`, **in upstream's order** — immediately after `prune_message_receipt_routes` /
`reply_edge` and **before** `find_session_ids`:

```rust
        // The exact-target block (`v0.11.0 broker/broker.ts:625-653`). Placed BEFORE name
        // resolution, because a valid exact target REPLACES `to` rather than filtering it.
        let exact_id = value.get("targetId");
        let exact_epoch = value.get("targetEpoch");
        let mut to = to;
        if exact_id.is_some() != exact_epoch.is_some()
            || exact_id.is_some_and(|v| v.as_str().is_none_or(str::is_empty))
            || exact_epoch.is_some_and(|v| v.as_str().is_none_or(str::is_empty))
        {
            // Half-supplied, non-string, or empty. NOT a connection kill, and NOT a silent
            // fallback to name routing: the sender asked for an exact endpoint and must be told
            // it did not get one.
            send_msg(self_tx, &BrokerMessage::delivery_failed(
                &message.id,
                "Exact target requires an id and endpoint epoch",
                "E_INVALID_TARGET",
                false,
            ));
            return FrameResult::cont();
        }
        if let (Some(target_id), Some(target_epoch)) =
            (exact_id.and_then(|v| v.as_str()), exact_epoch.and_then(|v| v.as_str()))
        {
            let fingerprint = DeliveryFingerprint::of(&message, target_id);
            if self.replay_or_reject(self_tx, &current_id, &message.id, &fingerprint, now) {
                return FrameResult::cont();
            }
            let bound = self
                .sessions
                .get(target_id)
                .is_some_and(|s| s.info.endpoint_epoch.as_deref() == Some(target_epoch));
            if !bound {
                // The refusal this whole task exists for. Recorded `retryable`, which is the ONLY
                // record `replay_or_reject` lets a resend past — that is how the client's one
                // retry reaches the replacement endpoint under the same message id.
                self.record_delivery(
                    &current_id,
                    &message.id,
                    fingerprint,
                    DeliveryState::Failed,
                    Some("Target endpoint changed before delivery"),
                    Some("E_TARGET_REBOUND"),
                    true,
                    now,
                );
                send_msg(self_tx, &BrokerMessage::delivery_failed(
                    &message.id,
                    "Target endpoint changed before delivery",
                    "E_TARGET_REBOUND",
                    true,
                ));
                return FrameResult::cont();
            }
            to = target_id.to_string();
        }
```

Then, in the **live-target** arm, after `from_info` is bound and before the `supersedes` check:

```rust
        let fingerprint = DeliveryFingerprint::of(&message, &target_id);
        if self.replay_or_reject(self_tx, &current_id, &message.id, &fingerprint, now) {
            return FrameResult::cont();
        }
```

and replace the final ack with:

```rust
        self.record_delivery(&current_id, &message.id, fingerprint,
            DeliveryState::SocketDelivered, None, None, false, now);
        send_msg(self_tx, &BrokerMessage::delivered(&message.id, DeliveredState::SocketDelivered));
```

Do the same in `handle_send_to_disconnected` — fingerprint against `target.id`, `replay_or_reject`
after `from_info` and before the `supersedes` refusal, and the terminal ack keyed on which branch
ran:

```rust
        let state = if live_delivered { DeliveryState::SocketDelivered } else { DeliveryState::Queued };
        self.record_delivery(&current_id, &message.id, fingerprint, state, None, None, false, now);
        send_msg(self_tx, &BrokerMessage::delivered(&message.id, /* matching DeliveredState */));
```

Attach a `code` to every other refusal in both handlers, keeping each reason string **byte-identical**:
`E_INVALID_MESSAGE`, `E_REPLY_TARGET` (×4), `E_SENDER_NOT_FOUND` (×2), `E_SUPERSEDE_TARGET` (×2),
`E_MUTUAL_ASK`, `E_AMBIGUOUS_TARGET` (×2), `E_TARGET_DISCONNECTED`, `E_TARGET_NOT_FOUND` (×2 — both
`Session not found` sites). None of these record.

Finally, the supersede notice site gains
`self.update_delivery_record(&current_id, superseded, DeliveryState::Failed, Some(&format!("Superseded by {}", message.id)), Some("E_DELIVERY_SUPERSEDED"));`
(`broker/broker.ts:709`).

### 3g. `broker/receipts.rs` and `broker/mailbox.rs` — the four lifecycle updates

* `handle_cancel_message`, parked-mail arm: `update_delivery_record(current_id, message_id, Failed,
  "Sender cancelled the queued delivery", "E_DELIVERY_CANCELLED")` before the ack (`:823`).
* `handle_cancel_message`, delivered arm: same with `"Sender cancelled the delivery"` (`:856`).
  **Both acks stay bare `BrokerMessage::delivered_bare(...)`, and the "Message cannot be cancelled
  by this session" failure keeps no code** — upstream left all three untouched.
* `prune_mailbox_messages`: for each expired entry, `update_delivery_record(entry.from.id,
  entry.message.id, Failed, "Mailbox delivery expired", "E_DELIVERY_EXPIRED")` (`:1005`).
* `queue_mailbox_message`'s cap eviction: `"Mailbox capacity evicted the delivery"`,
  `"E_DELIVERY_EVICTED"` (`:1021`).
* `flush_mailbox_for_session`, per redelivered entry: `update_delivery_record(entry.from.id,
  entry.message.id, SocketDelivered, None, None)` (`:1162`) — a parked message's record flips from
  `queued` to `socket_delivered` when it actually lands.

`prune_mailbox_messages` and `queue_mailbox_message` take `&mut self` already; both borrow `self`
while iterating, so collect the `(from_id, message_id)` pairs first and apply the updates after the
`retain` / `remove`, the way `prune_mailbox_messages` already collects `expired` before touching
`ask_edges`.

### 3h. `transport/client.rs` — feature gate, exact resolve, one retry

1. Widen `SendResult`:

```rust
pub struct SendResult {
    pub id: String,
    pub delivered: bool,
    pub reason: Option<String>,
    /// `v0.11.0 types.ts:7`. Defaults applied at the ack (`broker/client.ts:386,403`):
    /// `socket_delivered` on a bare `delivered`, `failed` on a bare `delivery_failed`.
    pub delivery: DeliveryState,
    /// The broker's failure code, e.g. `E_TARGET_REBOUND` — the value [`IntercomClient::send`]
    /// keys its single retry on.
    pub code: Option<String>,
    pub retryable: bool,
    pub outcome_known: bool,
}
```

   The two disconnect-drain sites (`:184-185`, `:691-692`) are the crate's only producers of
   `DeliveryState::Unknown`: `delivery: Unknown, retryable: true, outcome_known: false`. That is the
   honest answer — the send may well have been delivered — and it is why the `unknown` variant
   exists in a union the broker never emits.

2. Store negotiated features. `ClientInner` gains `features: Mutex<Vec<String>>` (initialised empty
   at all three construction sites, including the two `#[cfg(test)]` ones), populated in the
   `BrokerMessage::Registered` arm — which currently discards them as `features: _`:

```rust
                BrokerMessage::Registered { session_id, features } => {
                    …
                    *guard(&inner.features) = features.unwrap_or_default();
```

   plus `pub fn supports_feature(&self, feature: &str) -> bool` (pi's `supportsFeature`,
   `broker/client.ts:817-819`).

3. Restructure `send` around a `send_once`, keeping every existing guard, the `SEND_TIMEOUT`, and
   the `pending_sends` bookkeeping exactly as they are — only the frame's `target` field and the
   surrounding control flow change:

```rust
    pub async fn send(&self, to: &str, options: SendOptions) -> Result<SendResult> {
        …build `message` exactly as today…

        // `v0.11.0 broker/client.ts:671`. A REPLY is never exact-sent: it must keep routing by the
        // ask edge, which is the "replies keep their existing behavior" half of the commit. An old
        // broker that never advertised `exact-send-v1` gets the v0.9.2 frame byte-for-byte.
        if !self.supports_feature(EXACT_SEND_FEATURE) || options.reply_to.is_some() {
            return self.send_once(to, &message, ExactTarget::default()).await;
        }

        let Some(target) = self.resolve_exact_target(to).await else {
            return self.send_once(to, &message, ExactTarget::default()).await;
        };
        let result = self.send_once(to, &message, target).await?;
        if result.code.as_deref() != Some("E_TARGET_REBOUND") {
            return Ok(result);
        }
        // EXACTLY ONE retry, under the SAME message id — which the broker permits only because it
        // recorded the rebound refusal as retryable (`broker/broker.ts:1068-1070`).
        match self.resolve_exact_target(to).await {
            Some(rebound) => self.send_once(to, &message, rebound).await,
            None => Ok(result),
        }
    }

    /// `resolveTarget` (`v0.11.0 broker/client.ts:675-683`) — the CLIENT-side resolver, which
    /// returns `None` on ambiguity rather than raising. Deliberately NOT
    /// [`crate::session_state::SessionState::resolve_target`], which raises two distinct
    /// disambiguation errors because a human is reading them; here an ambiguous name simply
    /// degrades to a plain name-routed send and the BROKER produces `E_AMBIGUOUS_TARGET`.
    ///
    /// The id → exact-name → id-prefix ladder is `findSessions`', so it reuses
    /// [`crate::broker::routing::find_session_ids`] rather than restating it.
    async fn resolve_exact_target(&self, to: &str) -> Option<ExactTarget> {
        let sessions = self.list_sessions().await.ok()?;
        let entries: Vec<(String, Option<String>)> =
            sessions.iter().map(|s| (s.id.clone(), s.name.clone())).collect();
        let [only] = crate::broker::routing::find_session_ids(&entries, to).as_slice() else {
            return None;
        };
        let target = sessions.iter().find(|s| &s.id == only)?;
        // A target with no epoch is an older broker: fall back to a plain send rather than
        // inventing one (`target?.endpointEpoch ? … : null`).
        Some(ExactTarget {
            target_id: Some(target.id.clone()),
            target_epoch: Some(target.endpoint_epoch.clone()?),
        })
    }
```

4. The two ack arms apply upstream's defaults:

```rust
                BrokerMessage::Delivered { message_id, delivery, code, retryable, outcome_known } => {
                    if let Some(tx) = guard(&inner.pending_sends).remove(&message_id) {
                        let _ = tx.send(SendResult {
                            id: message_id,
                            delivered: true,
                            reason: None,
                            delivery: delivery.map_or(DeliveryState::SocketDelivered, Into::into),
                            code,
                            retryable: retryable.unwrap_or(false),
                            outcome_known: outcome_known.unwrap_or(true),
                        });
                    }
                }
```

   and the mirror for `DeliveryFailed` with `DeliveryState::Failed` as the default.

### 3i. Surface the details where the port already has a details map

Port `deliveryDetails` ([`index.ts:91-100`](../../tmp/pi-intercom/index.ts)) as one helper in
[`src/tools/mod.rs`](../../crates/cyrup-intercom/src/tools/mod.rs) beside `detailed_result`:

```rust
/// `deliveryDetails` (`v0.11.0 index.ts:91-100`) — the ack's full outcome, spread into a tool
/// result's `details`. `code`/`reason` are OMITTED when absent, not null (upstream's spread).
pub(crate) fn delivery_details(result: &SendResult) -> serde_json::Value { … }
```

Apply it at the success sites that already build a details map:
[`tools/intercom/send.rs`](../../crates/cyrup-intercom/src/tools/intercom/send.rs) (merge into the
existing `{ messageId, delivered: true }` map, keeping the conditional `replyTo` insert) and
[`tools/intercom/reply.rs`](../../crates/cyrup-intercom/src/tools/intercom/reply.rs) (merge, keeping
the unconditional `replyTo`).

**Leave the failure arms' text and their `Err(ToolError::new(...))` shape alone.** cyrup returns a
`ToolError` where upstream returns a details-bearing result; converting that channel is ICOM-scope
of its own and would change every failure string. The reason strings are unchanged by `636f61e`.

In [`session_state.rs`](../../crates/cyrup-intercom/src/session_state.rs), replace the hardcoded
fallback at `:838-839` with the ack's own state — upstream's `deliveryState = sendResult.delivery`
(`index.ts:2170`) — so the ask-timeout message reports `queued` when the peer was offline instead of
claiming `socket_delivered`:

```rust
                let delivery_state = self.latest_delivery_state(
                    Some(&question_id),
                    delivery_state_from_send, // the `DeliveryState` the successful send returned
                );
```

### 3j. `connect.rs` — the rename from §0

Mechanical: `epoch` → `reconnect_generation`, `epoch_at_start` → `generation_at_start`, and the two
doc lines at `:35-36`.

---

## 4. Order of work

1. `transport/protocol.rs` (§3a, §3b) — nothing compiles until the wire model lands.
2. `broker/limits.rs` + `broker/delivery.rs` + the two `BrokerState` fields (§3c, §3e).
3. `broker/session.rs` (§3d) — mint, advertise, delete the edge-clear.
4. `broker/send.rs` (§3f) — the refusal path and all the codes.
5. `broker/receipts.rs`, `broker/mailbox.rs` (§3g) — the five lifecycle updates.
6. `transport/client.rs` (§3h).
7. `tools/`, `session_state.rs` (§3i), `connect.rs` (§3j).

---

## 5. Definition of Done

Observable behaviour, end to end.

1. **The broker owns an epoch and rotates it.** A `sessions` reply and every `session_joined` /
   `presence_update` frame carries a string `endpointEpoch` for each live session. Re-registering
   the same `sessionId` on a new connection yields a **different** `endpointEpoch`; the session id,
   its join-order position, and its mailbox identity are unchanged.
2. **A superseded endpoint is refused, not silently misrouted.** A send carrying `targetId` plus the
   *previous* epoch is answered `delivery_failed` with reason
   `Target endpoint changed before delivery`, `code: "E_TARGET_REBOUND"`, `retryable: true`,
   `delivery: "failed"`, `outcomeKnown: true` — and the replacement endpoint receives nothing.
3. **A malformed exact target never degrades into name routing.** `targetId`/`targetEpoch` supplied
   half, non-string, or empty is answered `delivery_failed` / `E_INVALID_TARGET`; the connection
   stays open and the message is not delivered to whatever the name resolves to.
4. **The client recovers from a rebound target by itself.** With `exact-send-v1` advertised, a send
   whose target is replaced between resolution and delivery lands on the replacement, under the
   **same message id**, after exactly one re-resolve — and the caller sees
   `delivered: true, delivery: "socket_delivered"`.
5. **Duplicate ids are exactly-once per authored content.** Re-sending an id with identical content
   replays the recorded ack and the receiver gets the message **once**; re-sending that id with
   different content is refused `E_MESSAGE_ID_REUSE` and is not delivered. Two different senders
   whose ids contain `:` (sender `a:b`/id `c` vs sender `a`/id `b:c`) do **not** collide.
6. **The record store is bounded.** Records older than one hour are gone, and the store never
   exceeds 4096 entries — the oldest inserted is evicted first, and the insertion order survives an
   in-place status update.
7. **Records track the message's real fate.** A parked message's record reads `queued` and flips to
   `socket_delivered` when the mailbox flush delivers it; a superseded, cancelled, expired or
   evicted message's record reads `failed` with `E_DELIVERY_SUPERSEDED` / `E_DELIVERY_CANCELLED` /
   `E_DELIVERY_EXPIRED` / `E_DELIVERY_EVICTED`.
8. **Replies keep their existing behavior.** A reply never carries `targetId`/`targetEpoch`. A peer
   that is asked, disconnects, and re-registers under the same stable id can still answer: the reply
   reaches the asker and is not refused with `Reply target does not match a pending ask`. The same
   holds across a *live* stable-id replacement, which previously wiped the edge.
9. **Mailbox recovery keeps its existing behavior.** Park-on-disconnect, name+cwd identity matching,
   the runtime-alias and sender-identity exclusions, FIFO order, the 24 h/256 bounds, and
   flush-on-register are all unchanged in what they deliver and in what order.
10. **Interop is lossless in both directions.** A v0.9.2 peer (no `endpointEpoch`, bare acks) still
    sends and receives normally against a cyrup broker, and against a cyrup client — the ack
    defaults `socket_delivered` / `failed`, `retryable: false`, `outcomeKnown: true` apply. A cyrup
    broker's `registered` advertises exactly `["exact-send-v1"]` and still does not advertise
    `extension-bus-v1`. `cancel_message`'s acks remain bare.
11. **The grep tells the truth.** `grep -rn 'epoch' crates/cyrup-intercom/src` returns
    `endpoint_epoch` (this feature), `owner_epoch` (the extension bus), and epoch-millisecond
    timestamps — and nothing named `epoch` that is a reconnect counter.
