---
stage: aug
status: done
updated: 2026-08-27 22:41
---

# Port opt-in broker-enforced scoped routing (CYRUP_INTERCOM_SCOPE_ID)

> **Upstream parity gap.** `cyrup-intercom` is a port of `pi-intercom` **v0.9.2**; upstream is now
> **v0.12.0** (`ef95f19`, 2026-08-22) at [`nicobailon/pi-intercom`](https://github.com/nicobailon/pi-intercom).
> Reference checkout: [`./tmp/pi-intercom`](../../tmp/pi-intercom) (checked out at `v0.12.0`).
> Gap analysis: `docs/gap-analysis/11-cyrup-intercom.md` — **ICOM-055**, `medium` / `not-ported` / `M`.

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

### 2. Put it on the register wire — [`broker/client.ts:284-290`](../../tmp/pi-intercom/broker/client.ts) and [`types.ts:99`](../../tmp/pi-intercom/types.ts)

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

### 6. Send / resolution — [`broker/broker.ts:603-660,1246-1280`](../../tmp/pi-intercom/broker/broker.ts)

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

`findDisconnectedSessions` gets the identical treatment (`:1263-1280`). Note the prefix tier also
changed **shape**: it used to filter on the map key (`[id]) => id.startsWith(…)`), which is now a
composite key, so it filters on `session.info.id` instead. A cross-scope target therefore falls out
of the live ladder, falls out of the disconnected ladder, and lands on the existing
`"Session not found"` refusal — the same answer a never-seen name gets, leaking nothing.

### 7. Fan-out, mailbox and identity — [`broker.ts:1298-1318`](../../tmp/pi-intercom/broker/broker.ts)

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
so it can compare `sameScope(session.scopeId, sessionInfo.scopeId)` alongside name+cwd (`:1298-1310`),
and `flushMailboxForSession` (`:1112-1131`) skips any parked entry whose `targetScopeId` differs and
requires `sameScope(entry.fromScopeId, session.scopeId)` before the sender-identity guard can fire.

### 8. Out of scope for the port

`089b631` also scopes the **extension bus** (`scopedExtensionKey`, `scopedExtensionStateNamespace`,
namespace-owner election, `extension_publish`/`extension_state_commit` fan-out) and the on-disk
**pending-ask records** (`scopedPendingAskRecordPath`, a sha256 of the scope prefixed onto the
filename). cyrup ports **neither subsystem**: `broker/extensions.rs` deliberately implements only
pi's not-advertised miss branch (it never records `session.extensions`, never advertises
`EXTENSION_BUS_FEATURE`), and the crate has no `PENDING_ASKS_DIR` at all — its ask state is the
in-memory `ask_edges` map plus the client-side `reply_tracker.rs`. Both are correctly skipped here;
neither is a gap this task opens.

## What already exists in the port and must be reused

**Do not build any of this again.** Every seam below is present, tested and documented; this task
threads a scope through them.

| Already there | Reuse it for |
| --- | --- |
| [`identity.rs`](../../crates/cyrup-intercom/src/identity.rs) — the crate's **single env inventory**, `ENV_INTERCOM_SESSION_ID` / `ENV_INTERCOM_STABLE_ID` / `ENV_INTERCOM_NAME_POLL_MS` (`:24`) / `ENV_INTERCOM_ASK_TIMEOUT_MS` | The `CYRUP_INTERCOM_SCOPE_ID` constant, in the identical doc-comment shape |
| [`config.rs`](../../crates/cyrup-intercom/src/config.rs) — `ask_timeout_ms()` + `ask_timeout_ms_from(env)` (`:184-203`), the const-in-`identity`/resolver-in-`config` split for every `config.ts`-origin function | `getIntercomScopeId` is a `config.ts` function, so its resolver goes here, same split, same `_from` pure core |
| [`broker/routing.rs`](../../crates/cyrup-intercom/src/broker/routing.rs) — `find_session_ids`, the exact-id → exact-name → prefix ladder, with its four unit tests | Becomes the scoped resolver; the ladder itself is already correct |
| [`broker/state.rs`](../../crates/cyrup-intercom/src/broker/state.rs) — `sessions` + `session_order` (join order), `insert_session`/`remove_session`/`sessions_in_order`/`session_infos`/`broadcast` | Every one of these is a re-key site; the join-order invariant is already solved and must survive |
| [`broker/mailbox.rs`](../../crates/cyrup-intercom/src/broker/mailbox.rs) — `find_live_sessions_sharing_mailbox_identity`, `find_disconnected_session_ids`, `flush_mailbox_for_session`, `queue_mailbox_message` | The scoped mailbox rules; the name+cwd identity logic and `js_truthy_alias` stay untouched |
| [`broker/test_support.rs`](../../crates/cyrup-intercom/src/broker/test_support.rs) — `make_state`, `register`, `register_named`, `send_frame`, `payloads` | The fixtures the new behaviour is driven through; extend, never duplicate |
| [`transport/protocol.rs`](../../crates/cyrup-intercom/src/transport/protocol.rs) — `ClientMessage::Register` (`:675-692`) with `present_non_null` + `skip_serializing_if = "Option::is_none"` on `session_id`/`state_id` | `scope_id` gets the identical attribute pair — that is what makes the unscoped frame byte-identical to today's |
| [`transport/client.rs`](../../crates/cyrup-intercom/src/transport/client.rs) — `connect_target` / `connect_target_with_liveness` (`:374-393`), where `LivenessConfig::from_env()` is resolved inside the client exactly as `client.ts` does | The scope is resolved in the same place, with the same injectable-for-tests escape hatch |

### What must NOT change

* [`tools/intercom/list.rs`](../../crates/cyrup-intercom/src/tools/intercom/list.rs) — **zero edits.**
  It renders `client.list_sessions()`, which the broker has already filtered. Adding a client-side
  scope filter here would be the exact anti-pattern this feature exists to avoid.
* [`project_target.rs`](../../crates/cyrup-intercom/src/project_target.rs) — **zero edits.**
  `resolve_target_in_cwd` operates over the broker-supplied roster; it is scoped for free.
* [`config.rs`](../../crates/cyrup-intercom/src/config.rs)'s `IntercomConfig` — **no `scopeId` field.**
  Upstream deliberately did not add one; `config.json` is machine-global.
* `broker/extensions.rs`, `broker/runtime_claim.rs`, `broker/ratelimit.rs`, `broker/limits.rs` — untouched.
  `MAX_SESSIONS` stays a global cap.

## Implementation plan

Work strictly in this order; each step leaves the crate coherent.

### Step 1 — the env constant, in `identity.rs`

Add beside `ENV_INTERCOM_STABLE_ID` (`identity.rs:39`), in the established shape:

```rust
/// `CYRUP_INTERCOM_SCOPE_ID` (pi `INTERCOM_SCOPE_ID_ENV = "PI_INTERCOM_SCOPE_ID"`,
/// `v0.12.0 config.ts:6`), resolved by `getIntercomScopeId` (`v0.12.0 config.ts:21-24`) and carried
/// on the register frame (`v0.12.0 broker/client.ts:284-290`).
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

**`transport/protocol.rs`** — add to `ClientMessage::Register` (`:677-692`), after `session_id`,
with the same attribute pair its neighbours carry:

```rust
        /// The broker routing scope this session registers into (`scopeId`,
        /// `v0.12.0 types.ts:99`, `v0.12.0 broker/client.ts:284-290`). Absent — never null, never
        /// blank — for an unscoped session, so an unscoped register frame is byte-identical to the
        /// pre-scope one. Normalized and enforced broker-side by `normalizeScopeId`
        /// (`v0.12.0 broker/broker.ts:133-142`): present-but-not-a-string is fatal.
        #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
        scope_id: Option<String>,
```

Update the `Register` construction in that module's own round-trip test (`:975`).

**`transport/client.rs`** — mirror `client.ts`, which resolves the scope inside `connect`:

* `connect_target` (`:374-381`) resolves it and forwards:
  `Self::connect_target_with_liveness(target, registration, session_id, crate::config::intercom_scope_id(), LivenessConfig::from_env())`.
* `connect_target_with_liveness` (`:389-393`) gains `scope_id: Option<String>` — the same
  inject-for-tests escape hatch `liveness` already has, and for the same documented reason
  (`#![forbid(unsafe_code)]` means a test cannot `set_var`).
* The register frame at `:423-427` gains `scope_id`.

Fix the one other `connect_target_with_liveness` shape assumption if any test names it positionally;
`session_state.rs:1226` calls `connect_target` and needs no change.

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

Then rewrite `find_session_ids` into the scoped resolver, keeping its three-tier ladder and its four
existing tests (extended, not replaced):

```rust
/// `findSessions` (`v0.12.0 broker/broker.ts:1246-1262`): resolve `name_or_id` **within `scope`** to
/// zero, one, or many sessions by the fixed precedence — exact id, then case-insensitive exact name
/// (may be multiple), then id prefix. `entries` is `(key, name)` for every session in the map;
/// EVERY tier filters on scope, so a peer in another scope is not merely unaddressable, it is
/// indistinguishable from a name the broker has never seen.
///
/// The prefix tier matches on `key.id`, not on the map key — upstream made the same change
/// (`.filter(([, session]) => … session.info.id.startsWith(nameOrId))`) once the key stopped being
/// the bare id.
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
  invariant and its regression test (`state.rs:280-308`) carry over verbatim, keyed by `SessionKey`.
* `clear_ask_edges_for_session(&mut self, key: &SessionKey)` and
  `clear_message_receipt_routes_for_session(&mut self, key: &SessionKey)`.
* `broadcast` takes the scope, mirroring `broker.ts:1311-1318`:

```rust
    /// `broadcast(msg, exclude, scopeId)` (`v0.12.0 broker/broker.ts:1311-1318`). `scope` is the
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

### Step 7 — `handle_list` (`broker/session.rs:176-188`)

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
        // `throw new Error("List session not found")` (`v0.12.0 broker/broker.ts:590-593`): the
        // roster is now scope-relative, so it can only be answered to a requester whose scope the
        // broker can read. A socket that no longer owns its session used to be answered with the
        // full roster; it is now fatal, because "answer with everything" is the one wrong reply.
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
        // reason and deleted its two later copies.
        let Some(from) = self.sessions.get(&current_key).filter(|s| s.conn_id == conn_id) else {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Sender session not found".to_string(),
            });
            return FrameResult::cont();
        };
        let from_info = from.info.clone();
        let scope = from.key.scope.clone();

        // Join-ordered and SCOPE-FILTERED (`v0.12.0 broker/broker.ts:1246-1262`).
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

`handle_send_to_disconnected` takes `scope` and passes it to `find_disconnected_session_keys`; its own
sender re-lookup is deleted in favour of the hoisted `from_info`. The `"Session not found"` refusal it
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
  (`broker.ts:1298-1310`), ahead of the alias/name/cwd tests — mailbox identity is name + cwd
  **within one scope**, so a same-named session in the same directory but a different scope can never
  inherit another scope's parked mail.
* `find_unique_live_session_for_disconnected_session` and `find_disconnected_session_keys` follow.
* `flush_mailbox_for_session(&mut self, key: &SessionKey, now)` gains the two scope guards at
  `broker.ts:1118-1131`, in upstream's own order — the `entry.target_key.scope != key.scope` skip
  BEFORE the id match, and `sameScope` as an added conjunct of `matches_sender_identity`. The
  ask-edge re-point (`edge.to == entry.target_key` → `edge.to = key.clone()`) stays as-is, now over
  keys.

### Step 10 — connection plumbing (`broker/conn.rs`, `broker/dispatch.rs`, `broker/extensions.rs`)

Rename the per-connection variable to match `broker.ts:268`'s `sessionKey`:
`conn.rs:58,81,90,103,133,148,166` and `dispatch.rs:30,84` become `session_key: &mut Option<SessionKey>`.
`extensions.rs:118`'s `session_owns_connection` takes `&SessionKey`. No behaviour change in either
file — `dispatch.rs`'s `stateId` gate, its before-register ordering rule and the health branch are
untouched.

### Step 11 — fixtures (`broker/test_support.rs`)

`register`/`register_named` take an `Option<&str>` scope and emit `scopeId` only when `Some` — so
every existing call site keeps producing today's exact register frame. `send_frame` is unchanged.
Add a `register_scoped` convenience only if it reads better than a `None` argument at 20 call sites;
prefer the extra parameter.

## Definition of Done

Observable behaviour, at the broker:

1. **Unscoped is unchanged.** With no `CYRUP_INTERCOM_SCOPE_ID` set, a session's register frame
   contains no `scopeId` key, and every roster, send, ask, reply, presence broadcast, mailbox
   queue/flush, supersede and cancel behaves exactly as before this task.
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
   `IntercomConfig` are byte-identical to their pre-task versions, and `grep -rn "scope" crates/cyrup-intercom/src/tools`
   returns nothing.
10. A bare `String` session id is no longer a key of `BrokerState::sessions`,
    `BrokerState::disconnected_sessions`, `AskEdge`, `MessageReceiptRoute` or `MailboxMessage`: each
    is keyed by `SessionKey`, so a lookup that omits a scope cannot be expressed.
