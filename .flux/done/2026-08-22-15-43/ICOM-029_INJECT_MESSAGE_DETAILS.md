---
stage: qa
status: completed
updated: 2026-08-28 01:42
---

# Carry `details` through HostServices::inject_message — outstanding verification

> **QA 2026-08-28 00:39 — implementation ACCEPTED, verification INCOMPLETE.** The seam change, the
> `AgentMessage::Custom` field, all 13 implementors and 13 call sites, the guest-path fix and the
> de-duplicated entry shape are complete in production quality and are NOT restated here. What
> remains is that three of the eight acceptance clauses are stated as *observable* outcomes and
> nothing observes them. The chain was verified by reading — the run loop clones the whole
> `AgentMessage` into `MessageStart`/`MessageEnd` (`cyrup-agent/src/agent/run/mod.rs:254-255`,
> `turn.rs:31-32`) and `subscriber.rs:184` persists `details.clone()` — but reading is not the
> evidence these clauses ask for.

## Outstanding

### 1. `details` on the PERSISTED entry is unasserted

Every `append_custom_message` call in the suite passes `None` for details
(`cyrup-session/src/tests/sessions.rs:338,675`, `tests/compaction.rs:1286,1376`), so no test
distinguishes "details persisted" from "details silently dropped again".

**Needed:** deliver an inbound intercom message to an IDLE session and assert the resulting
`custom_message` entry's `details` deserializes cleanly as `InlineMessage`, with `from.id` and
`message.id` matching the delivered message.

### 2. Only one of the three delivery arms is exercised

`AgentSession::inject_message` has three arms and each reaches persistence differently:

| arm | route to the entry |
| --- | --- |
| streaming | `agent.steer(msg)` -> run loop -> `subscriber.rs:184` |
| `trigger_turn` | `spawn_run(vec![msg])` -> run loop -> `subscriber.rs:184` |
| durable | `append_custom_message(&kind, …, details)` directly |

**Needed:** cover all three. The durable arm is reachable with `inbound_trigger: never`; the steer
arm with a busy interactive session. A regression in any one of them is currently invisible.

### 3. The live `MessageEnd` payload is unasserted

ICOM-024's renderer reads `details` off the serialized message taken from `message_end`
(`cyrup-tui/src/app/extension_render.rs`), so this is the clause that surface depends on, and it is
the one with no coverage at all — `grep -rn 'MessageEnd' crates/ | grep -i details` returns nothing.

**Needed:** assert the emitted `AgentSessionEvent::MessageEnd` for an injection serializes with a
`details` key carrying the same object as the persisted entry.

### 4. `injectedAt` / `bodyText` correspondence is unasserted

DoD 3 requires `message.injectedAt` and `bodyText` to match the `**From …**` header, `_id …_`
metadata line and body of the same entry's `content`. The only `injectedAt` assertion in the tree
(`cyrup-it/tests/intercom/protocol_forward_compat.rs:249`) is an unrelated wire-shape test.

**Needed:** assert the stamped `injectedAt` survives into the persisted `details`, and that
`bodyText` is the same string the `content` markdown was rendered from.

## Definition of Done

- A delivered inbound message on EACH of the three arms leaves a `custom_message` entry whose
  `details` round-trips as `InlineMessage`.
- The live `MessageEnd` for that injection carries the same `details` object as the entry.
- `injectedAt` and `bodyText` in the persisted `details` correspond to the entry's own `content`.
- No production behaviour changes: this work is verification only, and the suite stays green
  (workspace 7899, `cyrup-it --features it -E 'binary(intercom)'` 79).

## Accepted, not a defect

`InjectMessage` (`cyrup-session-svc/src/host_services.rs`) lost its `Eq` derive because
`serde_json::Value` is `PartialEq` but not `Eq`. Nothing in the workspace depends on it and the
reason is documented at the type. Recorded here so a later reader does not "restore" it.
