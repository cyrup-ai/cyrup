---
stage: aug
status: done
updated: 2026-08-27 22:33
---

# Port the extension outbox API and message provenance

> **Upstream parity gap.** `cyrup-intercom` is a port of `pi-intercom` **v0.9.2**; upstream is now
> **v0.12.0** (`ef95f19`, 2026-08-22) at [`nicobailon/pi-intercom`](https://github.com/nicobailon/pi-intercom).
> Reference checkout: `./tmp/pi-intercom`. Gap analysis: `docs/gap-analysis/11-cyrup-intercom.md` — **ICOM-056**.

## 0. The headline: this surface is ENTIRELY ABSENT, and was never filed

```
$ grep -rin '<term>' crates/cyrup-intercom/src | wc -l
outbox            0
OutboxRequest     0
OutboxResult      0
extension_outbox  0
provenance        0
```

**Zero hits for all five.** No prior gap-analysis pass filed this surface; it was found by a surface
sweep of the clone (`docs/gap-analysis/11-cyrup-intercom.md:309`).

**This is the integration surface.** It is how anything *other than the agent itself* sends an
intercom message. Without it cyrup's intercom is closed to its own ecosystem: no extension can route
through it, and a message that did arrive from one could not be attributed to whoever sent it.

**Precise provenance of the four events** (verified with `git log -S` against the shared checkout —
worth correcting because the gap-analysis line rounds it off):

| Symbol | Landed | Commit |
| --- | --- | --- |
| `intercom:extension-register`, `intercom:extension-registry-ready` | **v0.8.0** | `db22c07` "feat: add silent extension bus" |
| `intercom:outbox-request`, `intercom:outbox-result`, both V1 interfaces, all 10 codes | **v0.12.0** | `ef95f19` |
| `MessageProvenance` / `isMessageProvenance` | **v0.12.0** | `ef95f19` |

So the *registry* half genuinely predates the audited `v0.10.1..v0.12.0` window by four minor
versions and is a long-standing blind spot; the *outbox + provenance* half is new at v0.12.0.
Neither is a regression — nothing was ever removed. `git show v0.10.1:index.ts | grep -c outbox` → `0`.

**Interop is non-fatal today**: the v0.9.2 envelope's `#[serde(flatten)] extra` capture
([`transport/protocol.rs:353-356`](../../crates/cyrup-intercom/src/transport/protocol.rs)) round-trips
an unknown `provenance` key, so a v0.12.0 peer is tolerated. The feature behind it is simply inert.

**The ten result codes are the contract.** An extension switches on `code` to decide whether to
retry, re-prompt, or give up. Porting a subset — or funnelling several conditions into a catch-all
`failed` — silently breaks every such extension. Every code below is mapped to a condition that is
*already reachable in the port*; none of them is speculative.

---

## 1. What upstream does

### 1.1 [`extension-api.ts`](../../tmp/pi-intercom/extension-api.ts) — the whole 84-line file

Four event names (`:3-6`) — these strings are the wire, and are ported byte-for-byte:

```ts
export const INTERCOM_EXTENSION_REGISTER_EVENT = "intercom:extension-register";
export const INTERCOM_EXTENSION_REGISTRY_READY_EVENT = "intercom:extension-registry-ready";
export const INTERCOM_OUTBOX_REQUEST_EVENT = "intercom:outbox-request";
export const INTERCOM_OUTBOX_RESULT_EVENT = "intercom:outbox-result";
```

The status and code vocabularies (`:8-20`):

```ts
export type IntercomOutboxResultStatus = "sent" | "rejected" | "blocked" | "failed";

export type IntercomOutboxResultCode =
  | "user_cancelled"
  | "confirmation_unavailable"
  | "session_unavailable"
  | "session_ended"
  | "invalid_request"
  | "duplicate_request"
  | "target_not_found"
  | "target_ambiguous"
  | "self_target"
  | "delivery_failed";
```

The two versioned envelopes (`:22-44`). Note the asymmetry: **every** request field is required,
**every** result field but `version`/`requestId`/`status` is optional and is *omitted* rather than
nulled (upstream builds it with conditional spreads, `index.ts:988-996`):

```ts
export interface IntercomOutboxRequestV1 {
  version: 1;
  requestId: string;
  extensionId: string;
  extensionName: string;
  to: string;
  message: string;
}

export interface IntercomOutboxResultV1 {
  version: 1;
  requestId: string;
  status: IntercomOutboxResultStatus;
  code?: IntercomOutboxResultCode;
  extensionId?: string;
  extensionName?: string;
  messageId?: string;
  detail?: string;
}
```

The rest of the file (`:46-84`) is the **registry** half — `IntercomExtensionEvent`,
`IntercomExtensionChannel`, `IntercomExtensionRegistration`. Its *effects* (owner election, fan-out,
the state store) are **ICOM-016** and are explicitly out of scope here; see §4.9 for exactly which
sliver of it this task does land and why.

### 1.2 Consumers in [`index.ts`](../../tmp/pi-intercom/index.ts)

Every site, in file order:

| Lines | What |
| --- | --- |
| `:15-28` | imports the four constants + both V1 types from `./extension-api.ts` |
| `:78-94` | `OutboxTarget {id,label}`, `OutboxRequestTrace {requestId, extensionId?, extensionName?, to?, message?}`, `PendingOutboxRequest {generation, request}` |
| `:471-507` | `parseOutboxRequestPayload` — the whole validator, source of `invalid_request` |
| `:645-646` | `outboxRequestIds: Set<string>` + `pendingOutboxRequests: Map<string, PendingOutboxRequest>` |
| `:970-982` | `currentSessionTargetMatches(to, resolvedTo?, activeClient?)` |
| `:984-998` | `buildOutboxResult` — the conditional-spread result builder |
| `:1000-1008` | `emitOutboxResult` — `appendEntry("intercom_outbox_result", …)` **then** `events.emit(RESULT)` |
| `:1009-1021` | `settleOutboxRequest` — pops from `pendingOutboxRequests`, emits once, returns whether it fired |
| `:1022-1028` | `failPendingOutboxRequests(generation, code, detail)` |
| `:1029-1046` | `resolveOutboxTarget` — the three-code target resolver |
| `:1047-1183` | `handleOutboxRequest` — the sync prelude + the `void (async () => …)()` body |
| `:1577` | `startSessionRuntime` → `failPendingOutboxRequests(gen, "session_ended", "Session replaced")` |
| `:1582` | `startSessionRuntime` → `outboxRequestIds.clear()` |
| `:1687-1698` | `pi.events.on(REGISTER, …)` → shape-check → `registerLocalExtension` |
| `:1700` | `pi.events.emit(REGISTRY_READY, { version: 1 })` — **unconditional, at load, right after the listener** |
| `:1716` | `pi.events.on(OUTBOX_REQUEST, handleOutboxRequest)` |
| `:1723-1731` | `session_shutdown` → all four unsubscribes + `failPendingOutboxRequests(gen, "session_ended", "Session shutting down")` |

`resolveOutboxTarget` (`:1029-1046`) is the reason the three target codes are distinguishable — note
that it does **not** reuse `resolveSessionTarget`, precisely because that function throws strings:

```ts
function resolveOutboxTarget(sessions, currentId, to): { ok: true; target } | { ok: false; code: "target_not_found" | "target_ambiguous" | "self_target"; detail } {
  const byId = sessions.find((session) => session.id === to);
  const lowerName = to.toLowerCase();
  const byName = byId ? [] : sessions.filter((s) => s.name?.toLowerCase() === lowerName);
  const byPrefix = byId || byName.length > 0 ? [] : sessions.filter((s) => s.id.startsWith(to));
  const matches = byId ? [byId] : byName.length > 0 ? byName : byPrefix;
  if (matches.length === 0) return { ok: false, code: "target_not_found", detail: `Session "${to}" is not currently connected.` };
  if (matches.length > 1)  return { ok: false, code: "target_ambiguous", detail: `Multiple sessions match "${to}".` };
  const target = matches[0]!;
  if (target.id === currentId) return { ok: false, code: "self_target", detail: "Cannot message the current session." };
  return { ok: true, target: { id: target.id, label: target.name || target.id } };
}
```

That precedence — **exact id → case-insensitive exact name (may be many) → id prefix** — is the same
ladder cyrup already ships in `broker::routing::find_session_ids`. See §2.1.

The async body's liveness fencing (`:1078-1182`) re-checks `getLiveContext(liveContext, outboxGeneration)`
at **six** points and picks a different code depending on where it fails: `session_unavailable` before
the connection is established, `session_ended` at every point after it:

```ts
const liveContext = getLiveContext(runtimeContext, outboxGeneration);
if (!liveContext) { settle("failed", { code: "session_unavailable", detail: "Intercom session is not active" }); return; }
if (config.confirmSend && !liveContext.hasUI) { settle("blocked", { code: "confirmation_unavailable", detail: "confirmSend is enabled but no UI is available" }); return; }
```

and the delivery leg (`:1145-1182`), which is where `provenance` is stamped:

```ts
if (!getLiveContext(liveContext, outboxGeneration) || client !== activeClient || !activeClient.isConnected()) {
  settleOutboxRequest(request.requestId, "failed", { code: "session_ended", detail: "Session ended before delivery" });
  return;
}
const result = await activeClient.send(target.id, {
  messageId: request.requestId,
  text: request.message,
  provenance: {
    type: "extension_outbox",
    extensionId: request.extensionId,
    extensionName: request.extensionName,
    requestId: request.requestId,
  },
});
…
if (!result.delivered) { settle("failed", { code: "delivery_failed", messageId: result.id, detail: result.reason ?? "Delivery failed" }); return; }
pi.appendEntry("intercom_sent", {
  to: target.label,
  message: { text: request.message },
  messageId: result.id,
  timestamp: Date.now(),
  extension: { id: request.extensionId, name: request.extensionName, requestId: request.requestId },
});
settleOutboxRequest(request.requestId, "sent", { messageId: result.id });
```

Two details that are easy to lose and are load-bearing:

- `messageId: request.requestId` — **the outbox message id IS the requestId.** That is what makes the
  emitted `messageId` correlatable back to the request, and what makes a broker-level replay
  idempotent at the receiver's own dedupe.
- the `intercom_sent` audit entry carries an `extension: {id, name, requestId}` key that the agent's
  own send never writes.

The `catch` at `:1177-1182` is the only place that *chooses* between two codes on the same failure:

```ts
const live = getLiveContext(liveContext, outboxGeneration);
settleOutboxRequest(request.requestId, "failed", { code: live ? "session_unavailable" : "session_ended", detail: getErrorMessage(error) });
```

### 1.3 [`broker/protocol.ts`](../../tmp/pi-intercom/broker/protocol.ts) @v0.12.0 — the wire guard

```ts
function isMessageProvenance(value: unknown): value is MessageProvenance {
  if (!isRecord(value)) return false;
  return value.type === "extension_outbox"
    && typeof value.extensionId === "string"
    && typeof value.extensionName === "string"
    && typeof value.requestId === "string";
}
```

used inside `isMessage` at `:114-116`:

```ts
if (value.provenance !== undefined && !isMessageProvenance(value.provenance)) {
  return false;
}
```

This is the `[NON-NULL]` shape the port already models with `present_non_null`: **absent is legal,
`null` is fatal, a malformed object is fatal** (`isRecord(null)` is `false`, so `provenance: null`
fails the whole message). The type itself, [`types.ts:57,64-69`](../../tmp/pi-intercom/types.ts):

```ts
export interface Message { …; provenance?: MessageProvenance; content: {…}; }

export interface MessageProvenance {
  type: "extension_outbox";
  extensionId: string;
  extensionName: string;
  requestId: string;
}
```

`type` is a **closed one-variant vocabulary**, exactly like `Attachment.type` — an unknown value
fails the message rather than being ignored.

[`broker/client.ts:21-30,635-640`](../../tmp/pi-intercom/broker/client.ts) threads it through
`SendOptions` onto the constructed `Message`. [`ui/inline-message.ts:70,109-112`](../../tmp/pi-intercom/ui/inline-message.ts)
is the only reader:

```ts
// collapsed meta, AFTER attachments and BEFORE the reply-to breadcrumb:
if (this.message.provenance?.type === "extension_outbox") meta.push(`Via ${this.message.provenance.extensionName}`);

// expanded, LAST block before the bottom border:
if (this.message.provenance?.type === "extension_outbox") {
  lines.push(frameLine(""));
  lines.push(frameLine(this.theme.fg("dim", ` Via extension: ${this.message.provenance.extensionName}`)));
}
```

---

## 2. What ALREADY EXISTS in the port and must be reused

Most of this task is wiring, not invention. Reuse these; **do not re-derive any of them.**

### 2.1 Target resolution — [`broker/routing.rs:18-39`](../../crates/cyrup-intercom/src/broker/routing.rs)

```rust
pub fn find_session_ids(entries: &[(String, Option<String>)], name_or_id: &str) -> Vec<String>
```

Already `pub`, already the exact upstream precedence (exact id → case-insensitive name, possibly many
→ id prefix), already tested (`ambiguous_prefix_returns_multiple`). It returns a `Vec`, so
`len() == 0` **is** `target_not_found` and `len() > 1` **is** `target_ambiguous`. It is currently
called only from the broker side; this task is its second caller. **This is the function
`resolve_outbox_target` is built on.**

**Do NOT reuse [`session_state.rs:620-655`](../../crates/cyrup-intercom/src/session_state.rs)
`SharedIntercomState::resolve_target`** for the outbox. It is the port of `resolveSessionTarget` and
is deliberately lossy for this purpose: it collapses both ambiguity classes into
`IntercomError::Client(String)` and returns `Ok(None)` for a miss, so the three codes are not
recoverable from it. Upstream made the same split for the same reason (§1.2). Keep `resolve_target`
where it is; it stays the tool path's resolver.

### 2.2 Self-target — [`session_state.rs:540`](../../crates/cyrup-intercom/src/session_state.rs) `current_session_target_matches`

```rust
pub fn current_session_target_matches(&self, to: &str, resolved_to: Option<&str>) -> bool
```

The port of `currentSessionTargetMatches` (`index.ts:970-982`). It already checks the resolved id
against the live client's own id **and** the name/alias set, and the relay seam already treats a
match as self-delivery at [`seams.rs:107`](../../crates/cyrup-intercom/src/seams.rs) and `:146`.
This is the `self_target` decision — it subsumes upstream's bare `target.id === currentId` and also
catches the same-name collision the port already refuses everywhere else. Making the outbox weaker
than the relay would be the divergence, not the parity.

The blunter guard `state.self_session_id()` ([`session_state.rs:671`](../../crates/cyrup-intercom/src/session_state.rs))
is the same value the tool arms compare against at
[`tools/intercom/send.rs:65-67`](../../crates/cyrup-intercom/src/tools/intercom/send.rs),
`ask.rs:63`, `reply.rs:31` (`"Cannot message the current session"` — one string, three sites, and now
the same sentence as the outbox `detail`).

### 2.3 Liveness / generation fencing — [`connect.rs`](../../crates/cyrup-intercom/src/connect.rs)

`getLiveContext(ctx, generation)` is **already ported**:

- [`connect.rs:283-300`](../../crates/cyrup-intercom/src/connect.rs) `pub fn is_live_at(state, generation) -> bool`
- `:229` `ConnectSupervisor::generation()` — the generation to stamp at request time
- `:385` `pub async fn ensure_connected(state, ConnectReason) -> Result<Arc<IntercomClient>>`
- `:90-99` `ConnectReason::Background` — and `Background` is the one reason that re-arms the reconnect
  ladder on failure, which is exactly what upstream's outbox uses (`ensureConnected("background")`,
  `index.ts:1088`)
- `:305` `begin_runtime` (pi `startSessionRuntime`) and `:330` `shutdown` — the two places that must
  now also drain the pending outbox map

Every `if (!getLiveContext(liveContext, outboxGeneration))` becomes `if !connect::is_live_at(&state, gen)`.
Nothing new is needed.

### 2.4 The confirm gate — [`tools/intercom/send.rs:92-114`](../../crates/cyrup-intercom/src/tools/intercom/send.rs)

```rust
if params.reply_to.is_none() && self.state.config.confirm_send && self.state.has_ui()
    && let Some(services) = self.state.host_services()
{
    let confirmed = services.confirm("Send Message", &format!(…), &cyrup_ext::DialogOptions::default());
    if !confirmed { return Ok(text_result("Message cancelled by user")); }
}
```

`config.confirm_send`, `state.has_ui()` ([`session_state.rs:314`](../../crates/cyrup-intercom/src/session_state.rs))
and `state.host_services()` (`:299`) all exist and are the three inputs the outbox's
`confirmation_unavailable` / `user_cancelled` split needs. `HostServices::confirm` returns `bool`,
not `Result`, so the port has exactly two ways confirmation can be *unavailable* (no UI, or no bound
host services) and one way it can be *declined* — which is the clean mapping in §3.

### 2.5 The audit-entry seam — `HostServices::append_entry` / `emit_event`

- `append_entry(&self, custom_type: &str, data: &Value) -> Result<String, String>` —
  [`cyrup-ext/src/host/services.rs:593`](../../crates/cyrup-ext/src/host/services.rs). Used at
  [`send.rs:146-161`](../../crates/cyrup-intercom/src/tools/intercom/send.rs), including the
  `tracing::warn!` on failure that this task copies.
- `emit_event(&self, topic: &str, payload: &Value)` — `services.rs:415`, the port of
  `pi.events.emit`. **Infallible and fire-and-forget**, matching `emit`'s `void`. Precedent for
  emitting from a native: `cyrup-permission-system/src/extension/events.rs:25`.

### 2.6 The inter-extension bus — `InitApi::subscribe_bus` / `NativeExtension::on_bus_event`

pi's `pi.events` is [`cyrup-ext/src/bus.rs`](../../crates/cyrup-ext/src/bus.rs) `SharedBus`, and the
native-tier half already exists but **has no consumer anywhere in the workspace** (`grep -rn
'subscribe_bus\|on_bus_event' crates/ | grep -v cyrup-ext/src` → empty). `cyrup-intercom` becomes the
first. The two halves:

- `InitApi::subscribe_bus(&mut self, topic: impl Into<String>)` — `cyrup-ext/src/native.rs:426`
- `async fn on_bus_event(&self, topic: &str, payload: &Value, ctx: &HostCtx) -> Result<(), ExtError>`
  — `cyrup-ext/src/native.rs:483`, default no-op, dispatched at `facade.rs:2383` with an EVENT-tier
  ctx; an `Err` is contained and reported on the `onError` channel.

**CYRUP-DELTA, already documented, do not re-litigate**: pi's `emit` is synchronous (node
`EventEmitter`); cyrup's is queued and fanned out at the next seam boundary
(`ExtensionHost::deliver_bus_events`, `bus.rs:78-88`). The outbox is asynchronous end to end anyway
(`void (async () => …)()`), so deferral changes nothing observable about it. The one ordering fact
that *is* preserved and matters: `subscribe_bus` runs inside `init` and the `registry-ready` emit
follows it in the same `init`, so no extension can ever see `registry-ready` before the request
listener is live.

### 2.7 The envelope — [`transport/protocol.rs:302-357`](../../crates/cyrup-intercom/src/transport/protocol.rs)

`Message` with `#[serde(rename_all = "camelCase")]`, the `present_non_null` deserializer (`:116-122`
— absent legal, `null` fatal, i.e. `isMessage`'s `x !== undefined && typeof x !== "T"` exactly), the
`#[serde(flatten)] extra: UnknownFields` capture at `:353-356`, and the hand-written `Default for
Message` at `:359-380`. `AttachmentKind` (`:420-429`) is the idiom for a closed `type` vocabulary.

### 2.8 The card — [`ui/inline_message.rs:100-190`](../../crates/cyrup-intercom/src/ui/inline_message.rs)

`InlineMessage::render` (expanded) and `render_collapsed` already hold the whole `Message`, already
build the attachments block and the reply-to breadcrumb, and already use `card_row` /
`theme.fg("dim", …)`. Adding the provenance line is two insertions, no new plumbing.

---

## 3. The code → existing-path map

**Every code is reachable from a real condition. There is no catch-all.**

| `code` | `status` | Condition | Existing port path |
| --- | --- | --- | --- |
| `invalid_request` | `rejected` | payload is not an object / `version != 1` / any of the five string fields missing or blank | new `parse_outbox_request`, a statement-for-statement port of `index.ts:471-507` |
| `duplicate_request` | `rejected` | `requestId` already seen in this runtime | `HashSet::insert` on the new `seen_request_ids`, cleared by `connect::begin_runtime` |
| `session_unavailable` | `failed` | not live at entry / `ensure_connected` errored / `client.session_id()` is `None` / `list_sessions()` errored / a later throw *while still live* | `connect::is_live_at` = false at entry; `connect::ensure_connected(Background)` `Err`; `IntercomClient::session_id()`; `IntercomClient::list_sessions()` `Err` |
| `session_ended` | `failed` | not live at any post-connect checkpoint, or the live client is no longer this `Arc`, or it disconnected; also every pending request at `begin_runtime` ("Session replaced") and `shutdown` ("Session shutting down") | `connect::is_live_at` = false; `Arc::ptr_eq(&state.client()?, &active)`; `IntercomClient::is_connected()`; `connect.rs:305`/`:330` |
| `confirmation_unavailable` | `blocked` | `config.confirm_send` is on but `!state.has_ui()`, **or** `state.host_services()` is `None` | `session_state.rs:314` / `:299`; upstream's "confirm threw" branch has no cyrup analogue because `HostServices::confirm -> bool` cannot fail — "no bound backend" is the same class of loss and takes it |
| `user_cancelled` | `rejected` | `services.confirm(...)` returned `false` | the same call as `send.rs:103-113` |
| `target_not_found` | `blocked` | `find_session_ids(...).is_empty()` | [`broker/routing.rs:18`](../../crates/cyrup-intercom/src/broker/routing.rs) |
| `target_ambiguous` | `blocked` | `find_session_ids(...).len() > 1` | same |
| `self_target` | `blocked` | `state.current_session_target_matches(&to, Some(&resolved))` | [`session_state.rs:540`](../../crates/cyrup-intercom/src/session_state.rs) |
| `delivery_failed` | `failed` | `SendResult.delivered == false`; `detail` = `result.reason` else `"Delivery failed"`; `messageId` = `result.id` | [`transport/client.rs:74-81`](../../crates/cyrup-intercom/src/transport/client.rs) `SendResult` |

Note the status pairing that must not be flattened: the three target codes are **`blocked`**, not
`failed` — the request was well-formed and the session was healthy; it is the *addressing* that was
refused, and an extension retries those differently from a transport failure.

---

## 4. Implementation plan

### 4.1 `transport/protocol.rs` — model provenance on the envelope

Add above `Message`:

```rust
/// `MessageProvenance` (`v0.12.0 types.ts:64-69`), guarded by `isMessageProvenance`
/// (`v0.12.0 broker/protocol.ts:73-81`) and enforced inside `isMessage` at `:114-116`.
///
/// `type` is a CLOSED one-variant vocabulary — `isMessageProvenance` compares it with `===`, so an
/// unknown tag fails the whole message rather than being ignored, exactly like [`AttachmentKind`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageProvenance {
    /// Always `"extension_outbox"` (`v0.12.0 broker/protocol.ts:78`).
    #[serde(rename = "type")]
    pub kind: ProvenanceKind,
    /// The originating extension's id (`:79`).
    pub extension_id: String,
    /// The originating extension's display name (`:80`) — the string the inbound card renders.
    pub extension_name: String,
    /// The `IntercomOutboxRequestV1.requestId` this delivery answers (`:81`).
    pub request_id: String,
    /// `[UNKNOWN-FIELDS]` + `[MAP-ONLY]`, as every sibling envelope struct carries.
    #[serde(flatten)]
    pub extra: UnknownFields,
}

/// The single `MessageProvenance.type` value (`v0.12.0 types.ts:65`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceKind {
    /// Delivered on behalf of an extension through the outbox.
    ExtensionOutbox,
}
```

Then, on `Message`, **between `expects_reply` and `content`** (upstream declaration order,
`types.ts:57`):

```rust
    /// Who originated this message, when it was not the agent itself
    /// (`v0.12.0 types.ts:57`, guarded at `v0.12.0 broker/protocol.ts:114-116`).
    ///
    /// `[NON-NULL]`: absent is legal, an explicit `null` is FATAL — `isRecord(null)` is `false`, so
    /// `isMessageProvenance(null)` fails and `isMessage` rejects the envelope. That is precisely
    /// what `present_non_null` reproduces. Until this field existed the key round-tripped through
    /// [`Message::extra`], which is why a v0.12.0 peer was tolerated but unattributable.
    #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
    pub provenance: Option<MessageProvenance>,
```

and `provenance: None` into `impl Default for Message` (`:359-380`).

### 4.2 `transport/client.rs` — thread it through the send

Add to `SendOptions` (`:51-70`, which already derives `Default`):

```rust
    /// `provenance` (`v0.12.0 broker/client.ts:29`) — stamped only by the extension outbox; the
    /// agent's own sends leave it `None`.
    pub provenance: Option<MessageProvenance>,
```

and set it in `IntercomClient::send` (`:488-497`), beside `supersedes`/`retry_of`:

```rust
            provenance: options.provenance,
```

Then add `provenance: None` at every exhaustive `SendOptions { … }` literal — the four `..Default::default()`
sites need nothing, these six do:
[`seams.rs:332`](../../crates/cyrup-intercom/src/seams.rs),
[`session_state.rs:749`](../../crates/cyrup-intercom/src/session_state.rs),
[`inbound.rs:324`](../../crates/cyrup-intercom/src/inbound.rs),
[`tools/intercom/send.rs:116`](../../crates/cyrup-intercom/src/tools/intercom/send.rs),
[`tools/intercom/reply.rs:38`](../../crates/cyrup-intercom/src/tools/intercom/reply.rs),
`src/bin/cyrup_intercom_child_fixture.rs:136`.

### 4.3 New module `src/outbox.rs`, declared in [`lib.rs`](../../crates/cyrup-intercom/src/lib.rs)

Add `pub mod outbox;` between `mod inbound;` and `mod paths;`.

**The four topics and the two envelopes.** `IntercomOutboxResultV1` uses
`skip_serializing_if = "Option::is_none"` throughout, which is how the JSON reproduces upstream's
conditional spreads — an absent optional must be **omitted**, never `null`:

```rust
//! The extension outbox (`v0.12.0 extension-api.ts`, driven from `v0.12.0 index.ts:1047-1183`) —
//! the surface through which an extension OTHER than the agent sends an intercom message, and the
//! ten-code result contract it switches on.

/// `INTERCOM_EXTENSION_REGISTER_EVENT` (`v0.12.0 extension-api.ts:3`).
pub const INTERCOM_EXTENSION_REGISTER_EVENT: &str = "intercom:extension-register";
/// `INTERCOM_EXTENSION_REGISTRY_READY_EVENT` (`:4`).
pub const INTERCOM_EXTENSION_REGISTRY_READY_EVENT: &str = "intercom:extension-registry-ready";
/// `INTERCOM_OUTBOX_REQUEST_EVENT` (`:5`).
pub const INTERCOM_OUTBOX_REQUEST_EVENT: &str = "intercom:outbox-request";
/// `INTERCOM_OUTBOX_RESULT_EVENT` (`:6`).
pub const INTERCOM_OUTBOX_RESULT_EVENT: &str = "intercom:outbox-result";

/// `IntercomOutboxResultStatus` (`:8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxResultStatus {
    /// The broker confirmed delivery.
    Sent,
    /// The request was well-formed but refused (bad target, duplicate, user declined).
    Rejected,
    /// Policy stopped it before it could be attempted (confirmation, addressing).
    Blocked,
    /// It was attempted and the session or the transport failed under it.
    Failed,
}

/// `IntercomOutboxResultCode` (`:10-20`) — the CONTRACT. An extension switches on this to decide
/// whether to retry, re-prompt, or give up, so every variant is reachable from its own condition
/// and none is a catch-all. See the code→path map in this task's brief.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxResultCode {
    /// The human declined the `confirmSend` dialog.
    UserCancelled,
    /// `confirmSend` is on but this session cannot ask (no UI, or no bound host services).
    ConfirmationUnavailable,
    /// No live intercom runtime, or the connect / roster lookup failed.
    SessionUnavailable,
    /// The runtime went away mid-flight (generation bumped, shutdown, client replaced).
    SessionEnded,
    /// The payload failed [`parse_outbox_request`].
    InvalidRequest,
    /// This `requestId` was already handled in this runtime.
    DuplicateRequest,
    /// `to` matched no connected session.
    TargetNotFound,
    /// `to` matched more than one connected session.
    TargetAmbiguous,
    /// `to` resolved to this very session.
    SelfTarget,
    /// The broker accepted the send and reported `delivered: false`.
    DeliveryFailed,
}

/// `IntercomOutboxRequestV1` (`:22-29`). EVERY field is required; `version` must be exactly `1`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntercomOutboxRequestV1 {
    /// Always `1` — a `2` is `invalid_request`, never a best-effort parse.
    pub version: u8,
    /// Caller-minted correlation id; also becomes the delivered `Message.id` (`index.ts:1150`).
    pub request_id: String,
    /// The originating extension's id.
    pub extension_id: String,
    /// The originating extension's display name (rendered on the recipient's card).
    pub extension_name: String,
    /// Session name or id to deliver to.
    pub to: String,
    /// The message body.
    pub message: String,
}

/// `IntercomOutboxResultV1` (`:33-42`). Every optional field is OMITTED when absent — upstream
/// builds this with conditional spreads (`index.ts:988-996`), so a `null` here is a wire change.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntercomOutboxResultV1 {
    /// Always `1`.
    pub version: u8,
    /// Echoes the request's `requestId`.
    pub request_id: String,
    /// The outcome class.
    pub status: OutboxResultStatus,
    /// The precise reason, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<OutboxResultCode>,
    /// Echoed from the request when it parsed far enough to carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_id: Option<String>,
    /// Echoed from the request when it parsed far enough to carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_name: Option<String>,
    /// The delivered `Message.id` (`sent`), or the attempted one (`delivery_failed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// Human-readable elaboration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
```

Also in this module, ports of `index.ts:78-94` and `:471-507`:

- `pub struct OutboxRequestTrace { request_id: String, extension_id: Option<String>, extension_name: Option<String>, to: Option<String>, message: Option<String> }`
- `pub struct PendingOutboxRequest { generation: u64, request: OutboxRequestTrace }`
- `pub enum ParsedOutboxRequest { Ok(Box<IntercomOutboxRequestV1>), Invalid { trace: Option<OutboxRequestTrace>, detail: String } }`
- `pub fn parse_outbox_request(payload: &serde_json::Value) -> ParsedOutboxRequest` — **hand-written
  against `serde_json::Value`, not `serde_json::from_value`**, because upstream's validator returns
  the partially-recovered `requestId`/`extensionId`/`extensionName` *on failure* so a rejection can
  still be correlated (`index.ts:475-477`), and because a blank-after-trim string is invalid where
  serde would accept it. `to` is stored trimmed, `message` is stored verbatim (`:502-503`).
  When no `requestId` is recoverable, upstream emits **nothing at all** (`:1050-1059`) — an
  uncorrelatable result would be noise on the bus; reproduce that.
- `pub fn resolve_outbox_target(sessions: &[SessionInfo], to: &str) -> Result<OutboxTarget, (OutboxResultCode, String)>`
  — built on `crate::broker::routing::find_session_ids`, with upstream's two detail strings verbatim
  (`Session "{to}" is not currently connected.` / `Multiple sessions match "{to}".`). The `self_target`
  leg lives in the caller because it needs `SharedIntercomState`; its detail is
  `"Cannot message the current session."`.
- `fn build_outbox_result`, `fn emit_outbox_result`, `pub fn settle_outbox_request`,
  `pub fn fail_pending_outbox_requests` — direct ports of `index.ts:984-1028`. `emit_outbox_result`
  does `append_entry("intercom_outbox_result", …)` **first**, then `emit_event(INTERCOM_OUTBOX_RESULT_EVENT, …)`,
  in that order, with the entry carrying the result plus `to`, `message.text` and `timestamp`.
- `pub async fn handle_outbox_request(state: Arc<SharedIntercomState>, payload: serde_json::Value)`.

### 4.4 `session_state.rs` — the two maps

Add to `SharedIntercomState` (beside `seen_inbound_messages` / `latest_outbound_receipts`,
`:196`-ish), the port of `index.ts:645-646`:

```rust
    /// `outboxRequestIds` (`v0.12.0 index.ts:645`) — every `requestId` seen in THIS runtime.
    /// Cleared by [`crate::connect::begin_runtime`] (`index.ts:1582`), never pruned by time: the
    /// dedupe window is the runtime, so a replay across a session restart is legal.
    outbox_request_ids: Mutex<HashSet<String>>,
    /// `pendingOutboxRequests` (`:646`) — in-flight requests keyed by `requestId`, each stamped
    /// with the generation it started under so [`crate::outbox::fail_pending_outbox_requests`]
    /// can settle exactly the ones a runtime change invalidated.
    pending_outbox_requests: Mutex<HashMap<String, PendingOutboxRequest>>,
```

with accessors `claim_outbox_request_id(&self, id: &str) -> bool` (insert-returns-`true`-if-new, so
the duplicate check and the claim are one atomic step under the lock),
`clear_outbox_request_ids(&self)`, `track_pending_outbox(&self, id, PendingOutboxRequest)`,
`take_pending_outbox(&self, id) -> Option<PendingOutboxRequest>`, and
`drain_pending_outbox_at(&self, generation) -> Vec<(String, PendingOutboxRequest)>`.

### 4.5 `connect.rs` — drain on runtime change

- In `begin_runtime` (`:305`), **before** the generation bump — mirroring `index.ts:1577,1582`:
  `crate::outbox::fail_pending_outbox_requests(state, state.connect.generation(), OutboxResultCode::SessionEnded, "Session replaced");`
  then `state.clear_outbox_request_ids();`
- In `shutdown` (`:330`), before its generation bump (`index.ts:1731`): the same call with
  `"Session shutting down"`.

Order matters: settle first, bump second, or `is_live_at` inside the settle path sees the new
generation and the trace is dropped.

### 4.6 `extension.rs` — subscribe, emit ready, dispatch

In `IntercomExtension::init` ([`extension.rs:444-505`](../../crates/cyrup-intercom/src/extension.rs)),
after the `api.subscribe(&[…])` block at `:484-503` and before `Ok(())`:

```rust
        // ICOM-056 / `v0.12.0 index.ts:1687,1716`: the two inbound bus topics. cyrup-intercom is the
        // first native to use `pi.events` at all (`InitApi::subscribe_bus`, `cyrup-ext/src/native.rs:426`);
        // deliveries land in [`Self::on_bus_event`].
        api.subscribe_bus(crate::outbox::INTERCOM_EXTENSION_REGISTER_EVENT);
        api.subscribe_bus(crate::outbox::INTERCOM_OUTBOX_REQUEST_EVENT);
        // `pi.events.emit(INTERCOM_EXTENSION_REGISTRY_READY_EVENT, { version: 1 })`
        // (`v0.12.0 index.ts:1700`) — UNCONDITIONAL, and immediately after the listeners so no
        // extension can observe "ready" before the request topic is live. This is the handshake an
        // extension waits on before it emits its first outbox request; without it the outbox is
        // listening to a bus nobody knows is there. `set_host_services` runs BEFORE `init`
        // (facade.rs:181), so the backend is bound here.
        if let Some(services) = self.state.host_services() {
            services.emit_event(
                crate::outbox::INTERCOM_EXTENSION_REGISTRY_READY_EVENT,
                &serde_json::json!({ "version": 1 }),
            );
        }
```

Then add the trait method to the same `impl NativeExtension for IntercomExtension`:

```rust
    /// The `pi.events` listeners (`v0.12.0 index.ts:1687-1698,1716`). An `Err` here is contained by
    /// the host and reported on the `onError` channel, matching pi's per-listener `catch`.
    async fn on_bus_event(
        &self,
        topic: &str,
        payload: &serde_json::Value,
        _ctx: &HostCtx,
    ) -> Result<(), ExtError> {
        match topic {
            // `index.ts:1716`. The handler NEVER blocks the fan-out: upstream runs the body as
            // `void (async () => …)()` and so does this — the synchronous prelude (parse, dedupe,
            // track) settles inline so `invalid_request`/`duplicate_request` are ordered against
            // the emit exactly as upstream orders them, and the rest is spawned.
            crate::outbox::INTERCOM_OUTBOX_REQUEST_EVENT => {
                crate::outbox::handle_outbox_request(self.state.clone(), payload.clone());
                Ok(())
            }
            // `index.ts:1687-1698`: shape-check, then register. See §4.9 for the scope line.
            crate::outbox::INTERCOM_EXTENSION_REGISTER_EVENT => {
                crate::outbox::handle_extension_register(&self.state, payload);
                Ok(())
            }
            _ => Ok(()),
        }
    }
```

`handle_outbox_request` is therefore a **sync** function that internally `tokio::spawn`s its async
tail — the same shape `on_event(SessionStart)` already uses at `extension.rs:551`-ish for the
connect. The spawned body:

```rust
    let gen = state.connect.generation();
    tokio::spawn(async move {
        if !crate::connect::is_live_at(&state, gen) {
            settle(&state, &request_id, OutboxResultStatus::Failed, OutboxResultCode::SessionUnavailable, "Intercom session is not active", None);
            return;
        }
        if state.config.confirm_send && !state.has_ui() {
            settle(&state, &request_id, OutboxResultStatus::Blocked, OutboxResultCode::ConfirmationUnavailable, "confirmSend is enabled but no UI is available", None);
            return;
        }
        let Ok(client) = crate::connect::ensure_connected(&state, crate::connect::ConnectReason::Background).await else { … SessionUnavailable … };
        if !crate::connect::is_live_at(&state, gen) { … SessionEnded, "Session ended before target resolution" … }
        // … list_sessions → resolve_outbox_target → self_target → confirm → send(provenance) → settle
    });
```

### 4.7 `ui/inline_message.rs` — render the attribution

Expanded ([`:147-157`](../../crates/cyrup-intercom/src/ui/inline_message.rs)), **after** the
reply-to breadcrumb and immediately before the bottom border (`ui/inline-message.ts:109-112`):

```rust
        // `v0.12.0 ui/inline-message.ts:109-112`: who actually originated this message, when it was
        // not the peer agent itself. The LAST block, so it reads as a footnote to the body.
        if let Some(p) = &self.message.provenance {
            lines.push(card_row(theme, body_width, ""));
            let via = theme.fg("dim", &format!(" Via extension: {}", p.extension_name));
            lines.push(card_row(theme, body_width, &via));
        }
```

Collapsed (`render_collapsed`, `:162-190`): push `format!("Via {}", p.extension_name)` **after** the
attachments entry and **before** the reply-to entry — upstream's `meta` order at
`ui/inline-message.ts:66-71`, which is not the same as the expanded order.

### 4.8 The `intercom_sent` audit entry for an outbox delivery

Written by `outbox.rs`, **not** by routing through `tools/intercom/send.rs`. The tool's entry
(`send.rs:146-161`) carries `message.attachments`/`message.replyTo`; upstream's outbox entry
(`index.ts:1170-1176`) carries neither and adds an `extension` key the agent's own send never writes:

```rust
    services.append_entry("intercom_sent", &serde_json::json!({
        "to": target.label,
        "message": { "text": request.message },
        "messageId": result.id,
        "timestamp": now_ms(),
        "extension": { "id": request.extension_id, "name": request.extension_name, "requestId": request.request_id },
    }))
```

with the same `tracing::warn!(error = %e, kind = "intercom_sent", …)` on failure that `send.rs:159-161`
uses.

### 4.9 Scope line on `intercom:extension-register`

This task lands the **front door only**: the topic subscription, upstream's payload shape check
(`index.ts:1690-1697`), the namespace regex `^[a-z0-9][a-z0-9._/-]{0,63}$` and the
already-registered refusal (`index.ts:863-868`), and the `registry-ready` emit — recording the
`(namespace, ownerEligible)` pair on `SharedIntercomState` so
`updateExtensionCapabilities`/`currentExtensionCapabilities` (`index.ts:856-861`) has a source.

`registration.onEvent` / `onReady` **cannot** cross a JSON bus and are not stubbed. The channel
effects behind them — owner election, publish fan-out, the state store — are **ICOM-016** (the port
already refuses them honestly at `broker/extensions.rs:136,171`, and never advertises
`EXTENSION_BUS_FEATURE`). Do not stretch this task into ICOM-016; do not fake a channel.

The `registry-ready` emit is nonetheless **mandatory here**, not deferred with the rest of the
registry: it is the handshake that tells an extension the outbox exists. Without it, the outbox
listener is correct and unreachable.

---

## 5. Definition of Done

Observable behavior, end to end:

1. On session start, an `intercom:extension-registry-ready` event carrying `{"version":1}` appears on
   the inter-extension bus, after the `intercom:outbox-request` subscription is live — never before.
2. Emitting a well-formed `intercom:outbox-request` naming a connected peer delivers the message to
   that peer and produces exactly one `intercom:outbox-result` with `status: "sent"`, `version: 1`,
   the echoed `requestId`/`extensionId`/`extensionName`, and `messageId` equal to the `requestId`.
   No `code` key is present.
3. The recipient's inbound card shows ` Via extension: <extensionName>` as its last block when
   expanded, and `Via <extensionName>` in the collapsed meta line between the attachment count and
   the reply-to breadcrumb.
4. The sending session's transcript carries an `intercom_sent` entry with an
   `extension: {id, name, requestId}` key, and an `intercom_outbox_result` entry mirroring the emitted
   result plus `to`, `message.text` and `timestamp` — the entry appended before the event is emitted.
5. Each of the ten codes is produced by its own condition, and each is emitted exactly once per
   `requestId`:
   - a payload missing any field, or with `version: 2`, or with a blank-after-trim `to`/`message` →
     `rejected` / `invalid_request`; a payload with no recoverable `requestId` produces **no event
     at all**;
   - re-emitting a `requestId` already used in this runtime → `rejected` / `duplicate_request`;
     the same `requestId` after a session restart is accepted;
   - `to` naming nothing connected → `blocked` / `target_not_found`; `to` matching two peers by name
     or by id prefix → `blocked` / `target_ambiguous`; `to` naming this session (by id, by resolved
     id, or by its own presence name) → `blocked` / `self_target`;
   - `confirmSend` on with no UI, or with no bound host services → `blocked` /
     `confirmation_unavailable`; the human declining the dialog → `rejected` / `user_cancelled`;
   - no live runtime, a failed connect, or a failed roster lookup → `failed` / `session_unavailable`;
   - a session restart or shutdown while a request is in flight settles it as `failed` /
     `session_ended` with detail `"Session replaced"` / `"Session shutting down"`;
   - the broker reporting `delivered: false` → `failed` / `delivery_failed`, carrying the broker's
     reason and the attempted `messageId`.
   No condition produces a bare `failed` with no `code`.
6. A `Message` carrying a well-formed `provenance` decodes into the typed field rather than into
   `extra`; a `Message` carrying `provenance: null`, an unknown `type`, or a missing
   `extensionId`/`extensionName`/`requestId` is rejected as malformed, matching `isMessage`.
7. A `Message` with no `provenance` key serializes without one — no `"provenance": null` on the wire —
   and every non-outbox send path (agent tool, reply, relay, compose overlay, child fixture) leaves
   it unset.
8. Handling an outbox request never blocks the bus fan-out: a request whose target resolution or
   confirmation takes seconds does not delay delivery of the next bus event.
