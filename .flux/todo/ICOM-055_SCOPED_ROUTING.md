---
stage: aug
status: done
updated: 2026-08-29 02:41
---

# Port opt-in broker-enforced scoped routing (CYRUP_INTERCOM_SCOPE_ID)

> **Upstream parity gap.** `cyrup-intercom` is a port of `pi-intercom` **v0.9.2**; upstream is now
> **v0.12.0** (`ef95f19`, 2026-08-22) at [`nicobailon/pi-intercom`](https://github.com/nicobailon/pi-intercom).
> Reference checkout: [`./tmp/pi-intercom`](../../tmp/pi-intercom) (checked out at `v0.12.0`).
> Gap analysis: `docs/gap-analysis/11-cyrup-intercom.md` — **ICOM-055**, `medium` / `not-ported` / `M`.

## 0. Re-verified — pass 2, 2026-08-29 against `04fa125` (source tree identical to `761dc19`)

Pass 1 found the scope-growth. **This pass audited every remaining citation and every enumerated
inventory against the compiler and against `tmp/pi-intercom@ef95f19`, and found three defects that
would have cost exec a cycle each, plus five wrong counts.** Read §0.1–§0.3 before writing code.

### 0.1 Three findings that change the plan

**(a) The broker never deserializes `ClientMessage`. Step 3 does not enforce anything.**
[`broker/js.rs:19-20`](../../crates/cyrup-intercom/src/broker/js.rs) states it as settled fact — "the
extension handlers read a raw `serde_json::Value` frame and never deserialize a typed
`ClientMessage`" — and `dispatch.rs:90-97` confirms it for *every* handler: each takes
`value: &serde_json::Value`. So the `present_non_null` deserializer added to `ClientMessage::Register`
in Step 3 **never runs on the broker path**. It is there for the client's *serialize* side and the
round-trip test only, and of the attribute pair it is `skip_serializing_if` — not `present_non_null` —
that makes the unscoped frame byte-identical to today's.

Consequence: **Step 6's hand-rolled `value.get("scopeId")` match is the entire enforcement of
`normalizeScopeId`.** It is not a redundant belt-and-braces copy; delete it and a `scopeId: 7` frame
registers unscoped, which is the confidentiality failure the fatality exists to prevent. Step 6 is
also already the house pattern — `handle_register` parses `sessionId` exactly that way at
[`session.rs:40`](../../crates/cyrup-intercom/src/broker/session.rs).

**(b) DoD #9's grep is unsatisfiable today, before any edit.**
`grep -rn "scope" crates/cyrup-intercom/src/tools` returns **5 hits right now** — all prose
(`mod.rs:8` "one scope", `mod.rs:72`/`:302` "scopes target lookup", `contact_supervisor.rs:589`/`:657`
"scope decision"). The check must test for the *feature*, not the word. Corrected in the DoD below.

**(c) Step 7's new fatality is much narrower than the step claims — and DoD #1 survives.**
[`dispatch.rs:85`](../../crates/cyrup-intercom/src/broker/dispatch.rs) already refuses every non-
`register`/`health` frame while `session_id.is_none()`, so a `list` before registration is *already*
a protocol error. The only behaviour Step 7 changes is the case upstream's `requester.socket !== socket`
guard adds: `session_id` is `Some(k)` but `sessions[k].conn_id != conn_id`, i.e. a **superseded
socket** whose identity was taken over by a newer connection. That socket is answered with the full
roster today and is destroyed after. Nothing else about unscoped `list` changes, so DoD #1 ("unscoped
is unchanged") holds with exactly this one documented exception — state it in the code comment rather
than discovering it in review.

### 0.2 Five wrong counts — re-derive, do not trust a number in this file

Every prior task in this series shipped at least one wrong exhaustive inventory. These are this
brief's:

| Brief said | Actually | Evidence |
| --- | --- | --- |
| `find_session_ids` has "four unit tests" | **5** | `grep -c '#\[test\]' broker/routing.rs` → 5 (`:54,59,64,71,76`) |
| fixtures: "a `None` argument at 20 call sites" | **35** | `register` 9 + `register_named` 26 (mailbox 17, receipts 6, send 6, dispatch 3, session 1, state 2) |
| "`session_state.rs:1226` calls `connect_target`" | **3 callers, and the line is `:1319`** | `connect.rs:531`, `session_state.rs:1319`, `bin/cyrup_intercom_child_fixture.rs:121` |
| (unstated) `connect_target_with_liveness` callers | **2** | `client.rs:383`, `client.rs:1119` — the 4th positional arg touches both |
| Step 10's `conn.rs` site list | **incomplete** | omits `:85` and `:115`, and the whole `#[cfg(test)]` block at `:208-240` |

`connect_target` keeps its signature, so all three of its callers compile unchanged — but the child
fixture binary (`bin/cyrup_intercom_child_fixture.rs:121`) is the one that can drive a real
cross-process scope check, so know it exists.

### 0.3 Citation drift, corrected

Port side:

| Was | Is | Symbol |
| --- | --- | --- |
| `state.rs:280-308` (join-order regression test) | `state.rs:323`, in `mod tests` at `:310` | `session_infos_are_returned_in_join_order` — `:280-308` is the middle of `on_connection_closed` |
| `protocol.rs:975` (round-trip test) | `protocol.rs:1045` | the `ClientMessage::Register` construction |
| `client.rs:423-427` | `client.rs:427-431` | the register-frame literal |
| `client.rs:393-397` | `client.rs:393-399` | `connect_target_with_liveness` signature |
| `extensions.rs:118` | `extensions.rs:248` | `session_owns_connection` (`:118` is inside `recompute_namespace_owners`) |
| `receipts.rs:40-140` | `receipts.rs:38-140` | `handle_message_receipt` (`handle_cancel_message` at `:86`) |
| `presence.rs:20-110` | `presence.rs:18-110` | `handle_presence` |
| `dispatch.rs:84` | `dispatch.rs:85` | the before-register guard (`:84` is its comment) |

Upstream side (`tmp/pi-intercom@ef95f19`):

| Was | Is | Symbol |
| --- | --- | --- |
| `types.ts:99` | **`types.ts:107`** | the `register` union member with `scopeId?` |
| `broker/client.ts:284-290` | `:286-292` | `getIntercomScopeId()` at `:286`, the conditional spread at `:291` |
| `broker.ts:1246-1262` | `:1247-1262` | `findSessions` |
| `broker.ts:1263-1280` | `:1264-1279` | `findDisconnectedSessions` |
| `broker.ts:1298-1310` | `:1299-1310` | `findLiveSessionsSharingMailboxIdentity` |
| `broker.ts:1112-1131` | `:1113-1131` | `flushMailboxForSession` |
| `broker.ts:1311-1318` | `:1312-1318` | `broadcast` |
| `broker.ts:590-593` | `:592-595` | the `list` requester guard |

### 0.4 Re-verified CORRECT — do not re-litigate these

* The **seven** `namespace_owners` sites `extensions.rs:91,116,122,129,175,313,441` — exact.
* The **three** `ExtensionStateManager` calls `extensions.rs:186` (`load_state`), `:415`
  (`current_revision`), `:449` (`commit_state`) — exact.
* Upstream's **eight** scoped-extension call sites `broker.ts:511,517,568,574,1355,1484,1581,1630` — exact.
* `namespace_owners: BTreeMap<String, NamespaceOwner>` at `state.rs:121`; `sessions` `:58`,
  `session_order` `:64`, `disconnected_sessions` `:75`, `broadcast` `:229`, `session_infos` `:269`,
  `on_connection_closed` `:286`.
* `ENV_INTERCOM_STABLE_ID` at `identity.rs:39`; `ask_timeout_ms` at `config.rs:184`,
  `ask_timeout_ms_from` at `:192`. The const-in-`identity`/resolver-in-`config` split is real for
  `config.ts`-origin functions (`name_poll_ms_from` lives in `identity.rs` because
  `INTERCOM_NAME_POLL_MS` is not a `config.ts` export).
* `handle_register` `session.rs:20`, `handle_list` `:216`, `handle_send` `send.rs:22`,
  `handle_cancel_ask` `:332`, `find_session_ids` `routing.rs:18`.
* `sha2` is a direct dependency and `extension_state.rs:15` already imports
  `sha2::{Digest as _, Sha256}`; the private `hex` helper is at `:78`; the state filename is
  `sha256(namespace)` at `:124` — which is precisely why 12b's unscoped-returns-bare-namespace rule
  preserves every existing file.
* **Step 8's duplicated-sender-lookup claim is true**, with exact lines: the identical
  `"Sender session not found"` block sits at [`send.rs:99-110`](../../crates/cyrup-intercom/src/broker/send.rs)
  (in `handle_send`, *after* target resolution) and at `send.rs:263-274` (in
  `handle_send_to_disconnected`). Hoisting collapses both.
* The mailbox flush guard **order** is as Step 9 describes: `broker.ts:1119-1122` skips on
  `!sameScope(entry.targetScopeId, session.scopeId)` *before* `matchesId` at `:1123`, and
  `sameScope(entry.fromScopeId, …)` is the **second** conjunct of `matchesSenderIdentity` (`:1126`),
  immediately after the `sessionName` truthiness test.
* The `registered` reply (`broker.ts:498-502`) carries `sessionId` + `features` and **no `scopeId`**,
  so DoD #2 stands. `broker.ts:483`'s `...(scopeId ? { scopeId } : {})` is the `ConnectedSession`
  object literal, not a wire frame.

### 0.5 Standing findings from pass 1

**§8's exclusion of the extension bus was written against a codebase that no longer exists.**
ICOM-016 landed owner election, publish fan-out and state commit in `broker/extensions.rs`; all three
are scope-aware upstream, so Step 12 exists and "What must NOT change" no longer lists that file.
The `PENDING_ASKS_DIR` half of §8 is still true — zero hits — so §8a stands.

**Sequencing: do ICOM-055 before ICOM-057.** ICOM-057 adds the on-disk pending-ask records that
`089b631` scopes (`scopedPendingAskRecordPath`, `broker.ts:163`, used at `:1196,:1203`). Landing it
first would grow this task a second time, exactly as ICOM-016 already did once.

---

## Objective

Add `CYRUP_INTERCOM_SCOPE_ID`: an **opaque, opt-in, broker-enforced** routing boundary. A session
that registers under a scope id may list, address, receive presence for, and recover mailbox mail
from **only** sessions that registered the exact same scope id. A session that registers without one
keeps today's behaviour byte-for-byte.

Two properties are non-negotiable and everything below serves them:

1. **Opt-in.** No `CYRUP_INTERCOM_SCOPE_ID` in the environment ⇒ nothing changes. The register frame
   must not gain a field, the broker must not gain a filter that can observe a difference, and every
   existing broker test must pass unmodified. `None` is its own scope, and it is the scope every
   session is in today.
2. **Broker-side.** The filter lives in `crates/cyrup-intercom/src/broker/`, never in the tool layer.
   A client is never trusted to filter its own view: `intercom{list}` renders whatever roster the
   broker hands it, and a `send` naming a peer in another scope must be **refused by the broker**
   with the same "Session not found" it gives for a name it has never seen — never dropped silently,
   and never leaked as "that session exists but you may not talk to it".

`CYRUP_INTERCOM_SCOPE_ID` is the **only** `PI_INTERCOM_*` variable with no cyrup counterpart:
`NAME_POLL_MS`, `SESSION_ID`, `STABLE_ID`, `ASK_TIMEOUT_MS`, `LIVENESS_INTERVAL_MS` and
`LIVENESS_TIMEOUT_MS` are all already declared in
[`identity.rs`](../../crates/cyrup-intercom/src/identity.rs). The naming convention, the doc-comment
shape and the `_from(env)`-pure-core idiom are therefore **already established** — this task adds one
more entry to an existing inventory, it does not invent a convention.

## What upstream does

Upstream commit `089b631` (issue #112, thanks to @YeungKC), v0.12.0 — 8 files, +500/−174. The whole
mechanism is: **re-key every broker map from `sessionId` to `(scopeId, sessionId)`**, then filter
every fan-out and every lookup on scope equality.

### 1. Resolve the env var — [`config.ts:6,21-24`](../../tmp/pi-intercom/config.ts)

```ts
const INTERCOM_SCOPE_ID_ENV = "PI_INTERCOM_SCOPE_ID";

export function getIntercomScopeId(env: NodeJS.ProcessEnv = process.env): string | undefined {
  const scopeId = env[INTERCOM_SCOPE_ID_ENV]?.trim();
  return scopeId ? scopeId : undefined;
}
```

Trimmed; blank is **unscoped**, not an error. Note the `env` parameter default — upstream's own
testable-pure-core seam, which is exactly cyrup's `_from(env: impl Fn(&str) -> Option<String>)` idiom.
It is read from the **environment only**. It is deliberately *not* a `config.json` key: `config.json`
is machine-global (see `stableId`'s own warning in the upstream README), so a scope there would apply
to every session on the box and defeat the entire feature.

### 2. Put it on the register wire — [`broker/client.ts:286-292`](../../tmp/pi-intercom/broker/client.ts) and [`types.ts:107`](../../tmp/pi-intercom/types.ts)

```ts
const scopeId = getIntercomScopeId();
writeMessage(socket, {
  type: "register",
  session,
  ...(sessionId ? { sessionId } : {}),
  ...(scopeId ? { scopeId } : {}),
  ...(typeof target === "string" ? {} : { stateId: target.stateId }),
});
```

```ts
| { type: "register"; session: SessionRegistration; sessionId?: string; stateId?: string; scopeId?: string }
```

The spread is conditional: an unscoped client emits **exactly the frame it emits today**. That is
half of the opt-in guarantee, and it is what keeps a v0.12.0 broker wire-compatible with an older
client and vice-versa.

### 3. Normalize and key — [`broker/broker.ts:133-150`](../../tmp/pi-intercom/broker/broker.ts)

```ts
function normalizeScopeId(value: unknown): string | undefined {
  if (value === undefined) {
    return undefined;
  }
  if (typeof value !== "string") {
    throw new Error("Invalid register scopeId");
  }
  const trimmed = value.trim();
  return trimmed ? trimmed : undefined;
}

function sameScope(a: string | undefined, b: string | undefined): boolean {
  return a === b;
}

function scopedSessionKey(scopeId: string | undefined, sessionId: string): string {
  return JSON.stringify([scopeId ?? null, sessionId]);
}
```

Three facts in ten lines, all load-bearing:

* **Absent** `scopeId` is legal and means unscoped. **Present-but-not-a-string** (including explicit
  `null`) `throw`s, i.e. `socket.destroy` — a malformed scope is fatal, never silently unscoped.
  **Whitespace-only** trims to unscoped and is *not* fatal.
* `sameScope` is plain `===` over `string | undefined`, so `undefined` is a scope like any other and
  unscoped sessions can only see unscoped sessions.
* Every map is re-keyed by the **composite** `[scope, id]`, not filtered after the fact. This is the
  structural half of the enforcement: a bare session id can no longer index `this.sessions` at all,
  so a lookup that forgot to carry a scope does not compile-as-written — it simply misses.

`ConnectedSession`, `DisconnectedSession`, `MailboxMessage`, `AskEdge` and `NamespaceOwner` each gain
a stored `key` and/or `scopeId` (`broker.ts:57-131`); `MailboxMessage` carries **both** ends
(`fromKey`/`fromScopeId`, `targetKey`/`targetScopeId`) because a mailbox flush compares each
independently.

The per-connection variable is renamed throughout: `let sessionId: string | null` becomes
`let sessionKey: string | null` (`broker.ts:268`), and `handleMessage(socket, msg, currentId, setId)`
becomes `handleMessage(socket, msg, currentKey, setKey)`. Every `this.sessions.get(currentId)` in the
file becomes `this.sessions.get(currentKey)`.

### 4. Register — [`broker/broker.ts:436-437,483,504`](../../tmp/pi-intercom/broker/broker.ts)

```ts
const scopeId = normalizeScopeId(clientMessage.scopeId);
const key = scopedSessionKey(scopeId, id);
…
const previous = this.sessions.get(key);
…
this.sessions.set(key, connectedSession);
this.disconnectedSessions.delete(key);
…
this.broadcast({ type: "session_joined", session: info }, key, scopeId);
```

Consequences worth naming: the `MAX_SESSIONS` cap stays **global** (`this.sessions.size`), so a scope
cannot be used to mint an unbounded roster; and an identity takeover now only takes over *within one
scope* — the same `stableId` in two different scopes is two different sessions, which is precisely
what the upstream README change says ("the newest registration takes over that identity only within
the same `PI_INTERCOM_SCOPE_ID` boundary").

### 5. `list` — [`broker/broker.ts:587-600`](../../tmp/pi-intercom/broker/broker.ts)

```ts
const requester = currentKey ? this.sessions.get(currentKey) : undefined;
if (!requester || requester.socket !== socket) {
  throw new Error("List session not found");
}
const sessions = Array.from(this.sessions.values())
  .filter(session => sameScope(session.scopeId, requester.scopeId))
  .map(s => s.info);
```

The handler now needs the requester, and a `list` from a socket that no longer owns its session is
**fatal** where it used to be answered. This is the enforcement point that makes client-side
filtering unnecessary — and the reason a client is never asked to filter.

### 6. Send / resolution — [`broker/broker.ts:603-660,1247-1279`](../../tmp/pi-intercom/broker/broker.ts)

The sender's session is now resolved **first**, before any target work, because its scope is the
input to every lookup that follows:

```ts
const fromSession = this.sessions.get(currentKey);
if (!fromSession || fromSession.socket !== socket) {
  this.writeDeliveryFailure(socket, message.id, "Sender session not found", "E_SENDER_NOT_FOUND");
  break;
}
…
const targets = this.findSessions(clientMessage.to, fromSession.scopeId);
```

and both resolvers take the scope and enforce it in all three tiers — exact id, exact name, id prefix:

```ts
private findSessions(nameOrId: string, scopeId: string | undefined): ConnectedSession[] {
  const byId = this.sessions.get(scopedSessionKey(scopeId, nameOrId));
  if (byId) {
    return [byId];
  }

  const lowerName = nameOrId.toLowerCase();
  const byName = Array.from(this.sessions.values()).filter(session => sameScope(session.scopeId, scopeId) && session.info.name?.toLowerCase() === lowerName);
  if (byName.length > 0) {
    return byName;
  }

  return Array.from(this.sessions.entries())
    .filter(([, session]) => sameScope(session.scopeId, scopeId) && session.info.id.startsWith(nameOrId))
    .map(([, session]) => session);
}
```

`findDisconnectedSessions` gets the identical treatment (`:1264-1279`). Note the prefix tier also
changed **shape**: it used to filter on the map key (`[id]) => id.startsWith(…)`), which is now a
composite key, so it filters on `session.info.id` instead. A cross-scope target therefore falls out
of the live ladder, falls out of the disconnected ladder, and lands on the existing
`"Session not found"` refusal — the same answer a never-seen name gets, leaking nothing.

### 7. Fan-out, mailbox and identity — [`broker.ts:1299-1318`](../../tmp/pi-intercom/broker/broker.ts)

```ts
private broadcast(msg: BrokerMessage, exclude?: string, scopeId?: string): void {
  for (const [id, session] of this.sessions) {
    if (id !== exclude && sameScope(session.scopeId, scopeId)) {
      writeMessage(session.socket, msg);
    }
  }
}
```

Every call site passes the originating session's scope: `session_joined` (`:504`), `session_left`
(`:327`, `:540`) and `presence_update` (`:957`). Mailbox identity is scoped too —
`findLiveSessionsSharingMailboxIdentity` now takes the whole `ConnectedSession | DisconnectedSession`
so it can compare `sameScope(session.scopeId, sessionInfo.scopeId)` alongside name+cwd (`:1299-1310`,
where `sameScope` is the FIRST conjunct of the filter), and `flushMailboxForSession` (`:1113-1131`) skips any parked entry whose `targetScopeId` differs and
requires `sameScope(entry.fromScopeId, session.scopeId)` before the sender-identity guard can fire.

### 8. Scope for the extension bus — NOW IN SCOPE (see §0)

`089b631` also scopes the **extension bus**. When this brief was written cyrup had no bus to scope,
so this section excluded it. **ICOM-016 has since landed the bus in full**, which puts it squarely in
scope — see §0 for the evidence and §Step 12 for the work.

Upstream's two helpers ([`broker/broker.ts:152-161`](../../tmp/pi-intercom/broker/broker.ts)):

```ts
function scopedExtensionKey(scopeId: string | undefined, namespace: string): string {
  return JSON.stringify([scopeId ?? null, namespace]);
}

function scopedExtensionStateNamespace(scopeId: string | undefined, namespace: string): string {
  if (!scopeId) {
    return namespace;                       // ← unscoped is the BARE namespace
  }
  return JSON.stringify(["scope", createHash("sha256").update(scopeId).digest("hex"), namespace]);
}
```

The asymmetry is the whole opt-in guarantee for persistence: an unscoped session's state namespace is
the **bare** namespace, so every file already on disk under `<intercomDir>/extension-state/` keeps its
name and its contents. Only a scoped session gets the `["scope", sha256(scope), namespace]` form, and
therefore a different `sha256`-derived filename.

Upstream's eight call sites (`:511`, `:517`, `:568`, `:574`, `:1355`, `:1484`, `:1581`, `:1630`) map
onto cyrup's four functions: the shared replay, the owner election, the publish fan-out and the state
commit.

### 8a. Still out of scope — the on-disk pending-ask records

`089b631` also scopes `scopedPendingAskRecordPath` (a sha256 of the scope prefixed onto the filename).
That one **remains correctly excluded**: verified against this branch's base, the crate still has no
`PENDING_ASKS_DIR` and no `pending-asks` path anywhere — its ask state is the in-memory `ask_edges`
map plus the client-side `reply_tracker.rs`.

**Sequencing note.** Those records are exactly what **ICOM-057** adds. Landing ICOM-057 *before* this
task would grow this task a second time, in precisely the way ICOM-016 already grew it once. Landing
this task first means ICOM-057 writes its records scoped from birth, and inherits
`scope_id`/`SessionKey` as things that already exist. **Do ICOM-055 before ICOM-057.**

## What already exists in the port and must be reused

**Do not build any of this again.** Every seam below is present, tested and documented; this task
threads a scope through them.

| Already there | Reuse it for |
| --- | --- |
| [`identity.rs`](../../crates/cyrup-intercom/src/identity.rs) — the crate's **single env inventory**, `ENV_INTERCOM_SESSION_ID` / `ENV_INTERCOM_STABLE_ID` / `ENV_INTERCOM_NAME_POLL_MS` (`:24`) / `ENV_INTERCOM_ASK_TIMEOUT_MS` | The `CYRUP_INTERCOM_SCOPE_ID` constant, in the identical doc-comment shape |
| [`config.rs`](../../crates/cyrup-intercom/src/config.rs) — `ask_timeout_ms()` + `ask_timeout_ms_from(env)` (`:184-203`), the const-in-`identity`/resolver-in-`config` split for every `config.ts`-origin function | `getIntercomScopeId` is a `config.ts` function, so its resolver goes here, same split, same `_from` pure core |
| [`broker/routing.rs`](../../crates/cyrup-intercom/src/broker/routing.rs) — `find_session_ids`, the exact-id → exact-name → prefix ladder, with its five unit tests | Becomes the scoped resolver; the ladder itself is already correct |
| [`broker/state.rs`](../../crates/cyrup-intercom/src/broker/state.rs) — `sessions` + `session_order` (join order), `insert_session`/`remove_session`/`sessions_in_order`/`session_infos`/`broadcast` | Every one of these is a re-key site; the join-order invariant is already solved and must survive |
| [`broker/mailbox.rs`](../../crates/cyrup-intercom/src/broker/mailbox.rs) — `find_live_sessions_sharing_mailbox_identity`, `find_disconnected_session_ids`, `flush_mailbox_for_session`, `queue_mailbox_message` | The scoped mailbox rules; the name+cwd identity logic and `js_truthy_alias` stay untouched |
| [`broker/test_support.rs`](../../crates/cyrup-intercom/src/broker/test_support.rs) — `make_state`, `register`, `register_named`, `send_frame`, `payloads` | The fixtures the new behaviour is driven through; extend, never duplicate |
| [`transport/protocol.rs`](../../crates/cyrup-intercom/src/transport/protocol.rs) — `ClientMessage::Register` (`:741-757`) with `present_non_null` + `skip_serializing_if = "Option::is_none"` on `session_id`/`state_id` | `scope_id` gets the identical attribute pair — that is what makes the unscoped frame byte-identical to today's |
| [`transport/client.rs`](../../crates/cyrup-intercom/src/transport/client.rs) — `connect_target` / `connect_target_with_liveness` (`:378-399`), where `LivenessConfig::from_env()` is resolved inside the client exactly as `client.ts` does | The scope is resolved in the same place, with the same injectable-for-tests escape hatch |

### What must NOT change

* [`tools/intercom/list.rs`](../../crates/cyrup-intercom/src/tools/intercom/list.rs) — **zero edits.**
  It renders `client.list_sessions()`, which the broker has already filtered. Adding a client-side
  scope filter here would be the exact anti-pattern this feature exists to avoid.
* [`project_target.rs`](../../crates/cyrup-intercom/src/project_target.rs) — **zero edits.**
  `resolve_target_in_cwd` operates over the broker-supplied roster; it is scoped for free.
* [`config.rs`](../../crates/cyrup-intercom/src/config.rs)'s `IntercomConfig` — **no `scopeId` field.**
  Upstream deliberately did not add one; `config.json` is machine-global.
* `broker/runtime_claim.rs`, `broker/ratelimit.rs`, `broker/limits.rs` — untouched.
  `MAX_SESSIONS` stays a global cap.
* `broker/extensions.rs` — **this line used to say "untouched" and no longer can.** ICOM-016 put the
  owner election, the publish fan-out and the state commit in that file, and all three are scope-aware
  upstream. Step 12 changes it.

## Implementation plan

Work strictly in this order; each step leaves the crate coherent.

### Step 1 — the env constant, in `identity.rs`

Add beside `ENV_INTERCOM_STABLE_ID` (`identity.rs:39`), in the established shape:

```rust
/// `CYRUP_INTERCOM_SCOPE_ID` (pi `INTERCOM_SCOPE_ID_ENV = "PI_INTERCOM_SCOPE_ID"`,
/// `v0.12.0 config.ts:6`), resolved by `getIntercomScopeId` (`v0.12.0 config.ts:21-24`) and carried
/// on the register frame (`v0.12.0 broker/client.ts:286-292`).
///
/// The broker's OPAQUE routing boundary, and the third distinct meaning in this file's session
/// trio: [`ENV_INTERCOM_SESSION_ID`] is published DOWNWARD for a child to read back as its
/// supervisor's id, [`ENV_INTERCOM_STABLE_ID`] is read by a session as its own restart-stable
/// registration id, and this one is read by a session as the ISOLATION CLASS its id lives in. A
/// session id is unique only within one scope; two scopes may hold the same id and they are two
/// different sessions.
///
/// Opt-in: absent (or whitespace-only) means unscoped, which is the scope every session registers
/// into today. Never read from `config.json` — that file is machine-global, so a scope stored there
/// would apply to every session on the machine and erase the boundary it is meant to draw.
pub const ENV_INTERCOM_SCOPE_ID: &str = "CYRUP_INTERCOM_SCOPE_ID";
```

### Step 2 — the resolver, in `config.rs`

Beside `ask_timeout_ms` (`config.rs:184-203`), reusing that function's exact `_from` split:

```rust
/// `getIntercomScopeId()` (`v0.12.0 config.ts:21-24`):
///
/// ```text
/// const scopeId = env[INTERCOM_SCOPE_ID_ENV]?.trim();
/// return scopeId ? scopeId : undefined;
/// ```
///
/// Trimmed; blank is UNSCOPED, not an error — unlike [`ask_timeout_ms`], a malformed value here has
/// no malformed shape to reject. Fatality for a bad scope lives on the BROKER side, where a
/// non-string `scopeId` on the register frame is a protocol error (`normalizeScopeId`,
/// `v0.12.0 broker/broker.ts:133-142`).
#[must_use]
pub fn intercom_scope_id() -> Option<String> {
    intercom_scope_id_from(|k| std::env::var(k).ok())
}

/// The pure core of [`intercom_scope_id`].
#[must_use]
pub fn intercom_scope_id_from(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    env(ENV_INTERCOM_SCOPE_ID).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}
```

Import `ENV_INTERCOM_SCOPE_ID` alongside the existing `use crate::identity::ENV_INTERCOM_ASK_TIMEOUT_MS;`.

### Step 3 — the wire

**`transport/protocol.rs`** — add to `ClientMessage::Register` (`:741-757`), after `session_id`,
with the same attribute pair its neighbours carry.

> **This edit enforces nothing** (§0.1a). The broker reads raw `serde_json::Value` frames and never
> deserializes a `ClientMessage` — [`broker/js.rs:19-20`](../../crates/cyrup-intercom/src/broker/js.rs)
> says so outright, and `dispatch.rs:90-97` shows every handler taking `value: &serde_json::Value`.
> So `present_non_null` here runs only in the round-trip test; the attribute that matters on the
> write path is `skip_serializing_if`, which is what keeps the unscoped frame byte-identical. The
> real `normalizeScopeId` is Step 6. Add `present_non_null` anyway — the variant is the crate's
> documented statement of the wire shape, and every sibling field carries it — but do not treat it
> as the guard.

```rust
        /// The broker routing scope this session registers into (`scopeId`,
        /// `v0.12.0 types.ts:107`, `v0.12.0 broker/client.ts:286-292`). Absent — never null, never
        /// blank — for an unscoped session, so an unscoped register frame is byte-identical to the
        /// pre-scope one. Normalized and enforced broker-side by `normalizeScopeId`
        /// (`v0.12.0 broker/broker.ts:133-142`): present-but-not-a-string is fatal.
        #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
        scope_id: Option<String>,
```

Update the `Register` construction in that module's own round-trip test (`:1045`).

**`transport/client.rs`** — mirror `client.ts`, which resolves the scope inside `connect`:

* `connect_target` (`:378-385`) resolves it and forwards:
  `Self::connect_target_with_liveness(target, registration, session_id, crate::config::intercom_scope_id(), LivenessConfig::from_env())`.
* `connect_target_with_liveness` (`:393-397`) gains `scope_id: Option<String>` — the same
  inject-for-tests escape hatch `liveness` already has, and for the same documented reason
  (`#![forbid(unsafe_code)]` means a test cannot `set_var`).
* The register frame at `:423-427` gains `scope_id`.

`connect_target_with_liveness` has exactly **two** call sites — `client.rs:383` (the delegation
above) and `client.rs:1119` (a test) — and both name the arguments positionally, so both take the new
one. `connect_target` keeps its signature, so all **three** of its callers compile unchanged:
`connect.rs:531`, `session_state.rs:1319`, and `bin/cyrup_intercom_child_fixture.rs:121`.

### Step 4 — `SessionKey`, in `broker/routing.rs`

This is the load-bearing decision. Upstream buys structural safety with a stringified composite key;
Rust buys the same thing, better, with a type. **Introduce a `SessionKey` newtype and make it the key
of every broker map.** Do not add a parallel `scope` field next to a `String` id anywhere — a bare
`String` session id must stop being a usable map key, so that a lookup which forgot the scope cannot
be written at all.

```rust
/// `scopedSessionKey(scopeId, sessionId)` (`v0.12.0 broker/broker.ts:148-150`), as a type rather
/// than upstream's `JSON.stringify([scopeId ?? null, sessionId])` string.
///
/// A session id is unique only WITHIN a scope; the broker's identity is the pair. Deriving
/// `Hash`/`Eq` gives `sameScope` (`:144-146`, a plain `===` over `string | undefined`) for free and
/// makes `None` a scope like any other — which is exactly why an unscoped session can only ever
/// reach unscoped peers, and why the absent-scope path is bit-for-bit today's behaviour.
///
/// Upstream stringifies because a JS `Map` keys by reference; Rust does not need that indirection,
/// and skipping it is what makes a scope-less lookup a COMPILE error rather than a silent miss.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    /// The registered scope, `None` for unscoped (`normalizeScopeId`, `:133-142`).
    pub scope: Option<String>,
    /// The session id, unique only within [`Self::scope`].
    pub id: String,
}

impl SessionKey {
    /// The key for `id` in `scope`.
    #[must_use]
    pub fn new(scope: Option<String>, id: String) -> Self {
        Self { scope, id }
    }

    /// `sameScope(a, b)` (`:144-146`) against a borrowed scope.
    #[must_use]
    pub fn in_scope(&self, scope: Option<&str>) -> bool {
        self.scope.as_deref() == scope
    }
}
```

Then rewrite `find_session_ids` into the scoped resolver, keeping its three-tier ladder and its **five**
existing tests (`routing.rs:54,59,64,71,76` — extended, not replaced):

```rust
/// `findSessions` (`v0.12.0 broker/broker.ts:1247-1262`): resolve `name_or_id` **within `scope`** to
/// zero, one, or many sessions by the fixed precedence — exact id, then case-insensitive exact name
/// (may be multiple), then id prefix. `entries` is `(key, name)` for every session in the map;
/// EVERY tier filters on scope, so a peer in another scope is not merely unaddressable, it is
/// indistinguishable from a name the broker has never seen.
///
/// The prefix tier matches on `key.id`, not on the map key — upstream made the same change
/// (`.filter(([, session]) => … session.info.id.startsWith(nameOrId))`) once the key stopped being
/// the bare id. cyrup already builds `entries` from `s.info.id` (`send.rs:61-64`), so this tier is
/// unchanged in substance; only its element type moves.
#[must_use]
pub fn find_session_keys(
    entries: &[(SessionKey, Option<String>)],
    name_or_id: &str,
    scope: Option<&str>,
) -> Vec<SessionKey> {
    let in_scope = |key: &SessionKey| key.in_scope(scope);
    // 1. exact id.
    if let Some((key, _)) = entries.iter().find(|(k, _)| in_scope(k) && k.id == name_or_id) {
        return vec![key.clone()];
    }
    // 2. case-insensitive exact name (may be multiple).
    let lower = name_or_id.to_lowercase();
    let by_name: Vec<SessionKey> = entries
        .iter()
        .filter(|(k, name)| {
            in_scope(k) && name.as_deref().map(str::to_lowercase).as_deref() == Some(lower.as_str())
        })
        .map(|(k, _)| k.clone())
        .collect();
    if !by_name.is_empty() {
        return by_name;
    }
    // 3. id prefix.
    entries
        .iter()
        .filter(|(k, _)| in_scope(k) && k.id.starts_with(name_or_id))
        .map(|(k, _)| k.clone())
        .collect()
}
```

`AskEdge::{from, to}` become `SessionKey` in the same file.

### Step 5 — re-key `BrokerState` (`broker/state.rs`)

* `sessions: HashMap<SessionKey, ConnectedSession>`, `session_order: Vec<SessionKey>`,
  `disconnected_sessions: HashMap<SessionKey, DisconnectedSession>`.
* `ConnectedSession` gains `pub(super) key: SessionKey` (upstream's stored `key`, `:60`), so a
  handler holding a session never has to re-derive it. Its `scope` is `key.scope`; do **not** store a
  second `scope_id` field.
* `insert_session(&mut self, key: SessionKey, …)`, `remove_session(&mut self, key: &SessionKey)`,
  `sessions_in_order() -> impl Iterator<Item = (&SessionKey, &ConnectedSession)>` — the join-order
  invariant and its regression test (`session_infos_are_returned_in_join_order`,
  `state.rs:323`, in the `mod tests` at `:310`) carry over verbatim, keyed by `SessionKey`.
* `clear_ask_edges_for_session(&mut self, key: &SessionKey)` and
  `clear_message_receipt_routes_for_session(&mut self, key: &SessionKey)`.
* `broadcast` takes the scope, mirroring `broker.ts:1312-1318`:

```rust
    /// `broadcast(msg, exclude, scopeId)` (`v0.12.0 broker/broker.ts:1312-1318`). `scope` is the
    /// ORIGINATING session's scope: a `session_joined` / `session_left` / `presence_update` never
    /// crosses the boundary, so a scoped session's very existence is invisible outside it.
    pub(super) fn broadcast(&self, msg: &BrokerMessage, exclude: Option<&SessionKey>, scope: Option<&str>) {
        let frame = match encode_json(msg) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "failed to encode broadcast");
                return;
            }
        };
        for (key, session) in self.sessions_in_order() {
            if Some(key) != exclude && key.in_scope(scope) {
                let _ = session.tx.send(frame.clone());
            }
        }
    }
```

* `session_infos()` becomes `session_infos_in_scope(&self, scope: Option<&str>) -> Vec<SessionInfo>`
  — join-ordered as today, scope-filtered as at `broker.ts:592-597`.
* `on_connection_closed(&mut self, conn_id, session_key: &Option<SessionKey>, now)` — the
  `session_left` broadcast now reads `BrokerMessage::SessionLeft { session_id: key.id.clone() }` and
  passes `key.scope.as_deref()` as the broadcast scope. **The wire still carries the bare session
  id**: `scopeId` exists on the register frame only and is never echoed back to any client. A peer
  learns nothing about scopes, which is what keeps them opaque.
* `lifecycle.rs:69-79`'s shutdown drain is unchanged (it clears whole maps).

### Step 6 — `handle_register` (`broker/session.rs:20-137`)

Parse the scope in pi's own position — after the `sessionId` check, before the `extensions` guard:

```rust
        // `normalizeScopeId(clientMessage.scopeId)` (`v0.12.0 broker/broker.ts:133-142,436`).
        // Absent → unscoped. Present-but-not-a-string (including an explicit `null`, since
        // `typeof null !== "string"`) is a `throw`, i.e. `socket.destroy` — a malformed scope must
        // never silently degrade to "global", which would be a confidentiality failure rather than
        // a parse failure. Whitespace-only trims to unscoped and is NOT fatal.
        let scope = match value.get("scopeId") {
            None => None,
            Some(v) => match v.as_str() {
                Some(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
                None => return FrameResult::protocol_error(),
            },
        };
        let key = SessionKey::new(scope.clone(), id.clone());
```

Then thread `key` through the rest of the handler, replacing every `&id`:
`self.sessions.get(&key)` for the takeover probe, `self.clear_ask_edges_for_session(&key)`,
`self.clear_message_receipt_routes_for_session(&key)`, `*session_key = Some(key.clone())`,
`self.insert_session(key.clone(), ConnectedSession { conn_id, key: key.clone(), info: info.clone(), … })`,
`self.disconnected_sessions.remove(&key)`,
`self.broadcast(&BrokerMessage::SessionJoined { session: info }, Some(&key), scope.as_deref())`,
`self.flush_mailbox_for_session(&key, now)`.

`SessionInfo.id` stays the bare `id` — the scope never appears in a `SessionInfo`, so no roster row,
no `intercom{list}` line and no `Registered { session_id }` reply changes shape.

`handle_unregister` takes the same treatment, including the scoped `session_left` broadcast.

### Step 7 — `handle_list` (`broker/session.rs:216-228`)

Port `broker.ts:587-600` exactly, including its new fatality:

```rust
    pub(super) fn handle_list(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_key: &Option<SessionKey>,
    ) -> FrameResult {
        let Some(request_id) = value.get("requestId").and_then(|v| v.as_str()) else {
            return FrameResult::protocol_error();
        };
        // `throw new Error("List session not found")` (`v0.12.0 broker/broker.ts:592-595`): the
        // roster is now scope-relative, so it can only be answered to a requester whose scope the
        // broker can read. NOTE the exact width of this change: `dispatch.rs:85` ALREADY refuses
        // every non-register frame while `session_id.is_none()`, so a pre-registration `list` is a
        // protocol error today. The only newly-fatal case is upstream's `requester.socket !== socket`
        // — a SUPERSEDED socket, whose identity a newer connection took over. That socket is handed
        // the full roster today, and "answer with everything" is the one wrong reply to a peer whose
        // scope the broker can no longer attribute.
        let Some(requester) = session_key.as_ref().filter(|k| {
            self.sessions.get(*k).map(|s| s.conn_id) == Some(conn_id)
        }) else {
            return FrameResult::protocol_error();
        };
        send_msg(self_tx, &BrokerMessage::Sessions {
            request_id: request_id.to_string(),
            sessions: self.session_infos_in_scope(requester.scope.as_deref()),
        });
        FrameResult::cont()
    }
```

`dispatch.rs:92` must pass `conn_id` and `session_key` into it.

### Step 8 — `handle_send` (`broker/send.rs:22-206`)

Hoist the sender lookup to the top, as upstream did (`broker.ts:614-618`), because its scope is the
input to the target resolution — this also removes the duplicated sender-lookup block that currently
sits in both `handle_send` and `handle_send_to_disconnected`:

```rust
        // `const fromSession = this.sessions.get(currentKey); if (!fromSession || …)`
        // (`v0.12.0 broker/broker.ts:614-618`) — MOVED AHEAD of target resolution, because the
        // sender's scope is what every lookup below is relative to. Upstream moved it for the same
        // reason and deleted its two later copies; cyrup's two copies are `send.rs:99-110` (in
        // `handle_send`, currently AFTER target resolution) and `send.rs:263-274` (in
        // `handle_send_to_disconnected`), byte-identical apart from `&current_id` vs `current_id`.
        let Some(from) = self.sessions.get(&current_key).filter(|s| s.conn_id == conn_id) else {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Sender session not found".to_string(),
            });
            return FrameResult::cont();
        };
        let from_info = from.info.clone();
        let scope = from.key.scope.clone();

        // Join-ordered and SCOPE-FILTERED (`v0.12.0 broker/broker.ts:1247-1262`).
        let entries: Vec<(SessionKey, Option<String>)> = self
            .sessions_in_order()
            .map(|(key, s)| (key.clone(), s.info.name.clone()))
            .collect();
        let targets = find_session_keys(&entries, &to, scope.as_deref());
```

Everything downstream compares `SessionKey`s: the supersede route check
(`route.from == current_key && route.to == target_key`), the reply-edge check
(`edge.to != current_key || edge.from != target_key`), the mutual-ask reverse-edge scan, the
`AskEdge` insert, and the `MessageReceiptRoute` insert.

`handle_send_to_disconnected` (`send.rs:216`) takes `scope` and passes it to
`find_disconnected_session_keys`; its own sender re-lookup (`:263-274`) is deleted in favour of the
hoisted `from_info`, which it already receives the ingredients for via `current_id`.

**Do not add an error code.** Upstream's `writeDeliveryFailure` (`broker.ts:1039-1041`) emits
`code: "E_SENDER_NOT_FOUND"` alongside `delivery`/`retryable`/`outcomeKnown`; cyrup's
`BrokerMessage::DeliveryFailed` (`protocol.rs:924-929`) is the v0.9.2 shape and carries only
`message_id` + `reason`. That divergence is a different gap, not this one — keep the frame as it is. The `"Session not found"` refusal it
already emits is the cross-scope refusal — no new error string, no new code path.

`handle_cancel_ask` (`send.rs:332`), `handle_message_receipt` and `handle_cancel_message`
(`broker/receipts.rs:40-140`), and `handle_presence` (`broker/presence.rs:20-110`) each swap their
`current_id: String` for `current_key: SessionKey`; `handle_presence`'s broadcast passes
`session.key.scope.as_deref()`.

### Step 9 — mailbox (`broker/mailbox.rs`)

* `DisconnectedSession` gains `pub(super) key: SessionKey` (`broker.ts:114-120`);
  `remember_disconnected_session` takes the `ConnectedSession`'s key and stores under it.
* `MailboxMessage` gains `from_key: SessionKey` and `target_key: SessionKey` (`broker.ts:122-131`).
  Both ends, because the flush compares them independently.
* `find_live_sessions_sharing_mailbox_identity(&self, key: &SessionKey, info: &SessionInfo) -> Vec<SessionKey>`
  gains `s.key.in_scope(key.scope.as_deref())` as the FIRST conjunct of its filter
  (`broker.ts:1299-1310`, where it is the FIRST conjunct), ahead of the alias/name/cwd tests — mailbox identity is name + cwd
  **within one scope**, so a same-named session in the same directory but a different scope can never
  inherit another scope's parked mail.
* `find_unique_live_session_for_disconnected_session` and `find_disconnected_session_keys` follow.
* `flush_mailbox_for_session(&mut self, key: &SessionKey, now)` gains the two scope guards at
  `broker.ts:1119-1131`, in upstream's own order — the `entry.target_key.scope != key.scope` skip
  BEFORE the id match, and `sameScope` as an added conjunct of `matches_sender_identity`. The
  ask-edge re-point (`edge.to == entry.target_key` → `edge.to = key.clone()`) stays as-is, now over
  keys. **`matches_unique_name` gets no scope guard of its own** — upstream deliberately leaves it
  bare (`broker.ts:1132-1138`), because it is already covered twice: by the outer `targetScopeId`
  skip, and by `unique_mailbox_identity` being computed from the now-scope-aware
  `find_live_sessions_sharing_mailbox_identity`. Adding a third is a divergence, not a belt.

### Step 10 — connection plumbing (`broker/conn.rs`, `broker/dispatch.rs`, `broker/extensions.rs`)

Rename the per-connection variable to match `broker.ts:268`'s `sessionKey`:
`conn.rs:58,81,85,90,103,115,133,148,166` and `dispatch.rs:30,85` become
`session_key: &mut Option<SessionKey>` — and `conn.rs`'s own `#[cfg(test)]` block at `:208-240`
declares three more (`:212,236,240`), which the brief's earlier list omitted. Re-derive from the
compiler rather than from this list. `extensions.rs:248`'s `session_owns_connection` (NOT `:118`,
which is inside `recompute_namespace_owners`) takes `&SessionKey`. No behaviour change in either
file — `dispatch.rs`'s `stateId` gate, its before-register ordering rule and the health branch are
untouched.

### Step 11 — fixtures (`broker/test_support.rs`)

`register`/`register_named` take an `Option<&str>` scope and emit `scopeId` only when `Some` — so
every existing call site keeps producing today's exact register frame. `send_frame` is unchanged.
There are **35** call sites, not 20: `register` 9 and `register_named` 26, distributed
mailbox 17 / receipts 6 / send 6 / dispatch 3 / state 2 / session 1. Prefer the extra parameter
anyway — `register` goes 4→5 args, and `register_named` goes 8→9 under the
`#[allow(clippy::too_many_arguments)]` it already carries (`test_support.rs:65`). Both build a raw
`json!` frame, so emitting `scopeId` only when `Some` is a two-line change in each.

Note `send_frame` (15 call sites) is genuinely unchanged: it takes `sid: &mut Option<String>`, which
becomes `&mut Option<SessionKey>` by type substitution alone, with no new argument.

### Step 12 — scope the extension bus (`broker/extensions.rs`, `broker/state.rs`)

New with this augmentation; ICOM-016 created the surface this step scopes. Every site below was
re-derived against this branch's base — **re-derive again at exec time.**

**12a. Re-key `namespace_owners`.** It is `BTreeMap<String, NamespaceOwner>`
([`broker/state.rs:121`](../../crates/cyrup-intercom/src/broker/state.rs)) keyed by bare namespace.
Key it by the composite instead, using the same newtype discipline Step 4 applies to `SessionKey` —
a bare namespace must stop being a usable key so a scope-forgetting lookup cannot be written:

```rust
/// `scopedExtensionKey(scopeId, namespace)` (`v0.12.0 broker/broker.ts:152-154`), as a type rather
/// than upstream's `JSON.stringify([scopeId ?? null, namespace])` string. Ordering is by scope then
/// namespace, which keeps `recompute_namespace_owners`'s walk deterministic exactly as the
/// `BTreeMap` did before.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct NamespaceKey {
    pub(super) scope_id: Option<String>,
    pub(super) namespace: String,
}
```

The seven access sites, all in [`broker/extensions.rs`](../../crates/cyrup-intercom/src/broker/extensions.rs):
`:91` (the key sweep that seeds the candidate namespaces), `:116` (vacancy removal), `:122` (the
existing-owner read), `:129` (the winner insert), `:175` (the replay's owner lookup), `:313` (the
publish owner lookup) and `:441` (the commit owner lookup).

**The map's VALUE is also a re-key site, which the step above does not say.**
`NamespaceOwner` is `{ session_id: String, conn_id: u64, epoch: String }`
([`extensions.rs:35-39`](../../crates/cyrup-intercom/src/broker/extensions.rs)); once `sessions` is
keyed by `SessionKey`, that bare `session_id` can no longer index it, so it becomes
`session_key: SessionKey`. **`ExtensionOwnerRef` does not follow.** Its `owner_id`
([`transport/protocol.rs:619`](../../crates/cyrup-intercom/src/transport/protocol.rs)) is a WIRE
field, and DoD #2 forbids a scope on the wire — so `extensions.rs:139` sends `key.id.clone()`, the
bare id, exactly as it sends today.

`recompute_namespace_owners` needs one further change beyond re-keying: its candidate sweep at `:91-94`
collects namespaces from every session's `extensions`, and must now pair each with **that session's**
scope, so a namespace advertised in two scopes elects two independent owners
(`v0.12.0 broker/broker.ts:1355`).

`notify_namespace_capable` must also gain the scope: a vacancy or election in one scope is announced
only to sessions in that scope. Its current body iterates every session advertising the namespace;
it must additionally require `same_scope`.

**12b. Scope the state namespace.** The three `ExtensionStateManager` calls —
`load_state` (`extensions.rs:186`), `current_revision` (`:415`) and `commit_state` (`:449`) — must
pass a scoped namespace string, not the bare one:

```rust
/// `scopedExtensionStateNamespace` (`v0.12.0 broker/broker.ts:156-161`).
///
/// UNSCOPED RETURNS THE BARE NAMESPACE. That is not a shortcut — it is the opt-in guarantee for
/// persistence: every file already written under `<intercomDir>/extension-state/` keeps its name and
/// its contents, because `ExtensionStateManager` derives the filename from a sha256 of exactly this
/// string. Only a scoped session gets the tagged form, and therefore a different file.
fn scoped_extension_state_namespace(scope_id: Option<&str>, namespace: &str) -> String {
    match scope_id {
        None => namespace.to_string(),
        Some(scope) => serde_json::json!(["scope", hex(&Sha256::digest(scope.as_bytes())), namespace])
            .to_string(),
    }
}
```

`sha2` is ALREADY a direct dependency of this crate — ICOM-016 added it for the state store — so this
needs no manifest change. Reuse [`broker/extension_state.rs`](../../crates/cyrup-intercom/src/broker/extension_state.rs)'s
private `hex` helper rather than adding a second one; make it `pub(super)` if the scoping lives
outside that module.

Match upstream's JSON encoding exactly. The string is the persistence key, so any divergence in
separators or escaping silently orphans state written by a pi broker sharing the directory.
`serde_json::Value::to_string` and `JSON.stringify` agree here and it is worth knowing why rather
than hoping: both emit `["a","b","c"]` with no spaces, both escape only `"`, `\` and C0 controls
(as lowercase `\u00xx`), and neither escapes `/` or non-ASCII. The one divergence JS has — lone
surrogates, which ES2019 emits as `\udXXX` — is unreachable from a Rust `String`. `namespace_is_valid`
(`extensions.rs:70`) narrows the input further still.

`NamespaceKey`'s derived `Ord` puts `scope_id: None` before every `Some`, then orders by namespace —
total and deterministic, which is exactly the property `state.rs:115` documents the `BTreeMap` for.

**12c. What must NOT change here.** `ExtensionStateManager` itself takes no scope parameter —
upstream scopes the namespace at the CALL SITE and leaves the manager oblivious
(`broker/extension-state.ts` is untouched by `089b631`). Keep it that way: the manager stays a pure
namespace→state store.

## Definition of Done

Observable behaviour, at the broker:

1. **Unscoped is unchanged, with exactly one named exception.** With no `CYRUP_INTERCOM_SCOPE_ID`
   set, a session's register frame contains no `scopeId` key, and every roster, send, ask, reply,
   presence broadcast, mailbox queue/flush, supersede and cancel behaves exactly as before this
   task. The single exception, inherited from upstream and documented at the call site: a `list`
   arriving on a **superseded** socket — one whose session a newer connection has taken over — is
   now a protocol error instead of a full-roster reply (§0.1c). A `list` before registration was
   already a protocol error via `dispatch.rs:85` and is not a change.
2. **A scope is opaque and invisible.** No `SessionInfo`, no `sessions` reply, no `session_joined` /
   `session_left` / `presence_update`, no `registered` reply and no `intercom{list}` row ever carries
   a scope id. Scope appears on the register frame and nowhere else on the wire.
3. **Roster isolation.** Two sessions registered with `CYRUP_INTERCOM_SCOPE_ID=alpha` and one with
   `beta`: each `alpha` session's `intercom{list}` shows exactly the two `alpha` sessions; the `beta`
   session's shows exactly itself. An unscoped fourth session sees only itself — **not** everyone;
   unscoped is a scope, not a wildcard.
4. **Addressing isolation, by refusal.** An `alpha` session's `send` or `ask` naming the `beta`
   session — by full id, by name, by id prefix, or by cwd — comes back `delivery_failed` with
   `"Session not found"`, the identical answer a never-registered name gets. It is never delivered,
   never silently dropped, and the refusal reveals nothing about the target's existence.
5. **Presence isolation.** A `beta` session joining, leaving, or updating presence produces no frame
   of any kind on any `alpha` or unscoped socket.
6. **Mailbox isolation.** Mail parked for a disconnected `alpha` peer is redelivered only to an
   `alpha` session; a `beta` session with the same name in the same cwd never receives it, and never
   satisfies the unique-mailbox-identity test that would let it.
7. **Identity isolation.** The same `CYRUP_INTERCOM_STABLE_ID` used in `alpha` and in `beta` produces
   two coexisting sessions; neither takeover-evicts the other. The global `MAX_SESSIONS` cap still
   counts every session in every scope.
8. **Malformed scope is fatal, blank scope is not.** A register frame with `scopeId: 7` or
   `scopeId: null` destroys the connection. `CYRUP_INTERCOM_SCOPE_ID="   "` registers unscoped and
   connects normally.
9. **Enforcement is provably broker-side.** `tools/intercom/list.rs`, `project_target.rs` and
   `IntercomConfig` are byte-identical to their pre-task versions, and
   `grep -rnE 'scope_id|SessionKey|CYRUP_INTERCOM_SCOPE_ID|scoped_extension' crates/cyrup-intercom/src/tools`
   returns nothing. **Do not use a bare `grep -rn scope`** — it already matches 5 pre-existing prose
   hits (`tools/intercom/mod.rs:8,72,302`, `tools/contact_supervisor.rs:589,657`) and can never be
   made to return nothing (§0.1b).
10. `NamespaceOwner.session_id` is a `SessionKey`, `namespace_owners` is keyed by `NamespaceKey`,
    and `ExtensionOwnerRef.owner_id` still carries the bare session id — a scoped session's
    `extension_namespace_owner` frame is indistinguishable from an unscoped one's.
11. A bare `String` session id is no longer a key of `BrokerState::sessions`,
    `BrokerState::disconnected_sessions`, `AskEdge`, `MessageReceiptRoute` or `MailboxMessage`: each
    is keyed by `SessionKey`, so a lookup that omits a scope cannot be expressed.
