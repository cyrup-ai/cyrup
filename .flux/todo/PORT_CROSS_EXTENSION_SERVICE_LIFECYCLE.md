---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Publish the cross-extension permissions service and ready event

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | absent |
| **Upstream area** | service.ts / service-lifecycle.ts wiring |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream publishes a session-keyed PermissionsService (checkPermission/getToolPermission plus
registerAuthorizer / tool-input-formatter / tool-access-extractor surfaces) at session_start and
announces it on the `permissions:ready` channel again at the first before_agent_start; the port
publishes only the yolo runtime API and has no service, no registration surfaces, and no ready
channel.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

service.ts:70-76 (SERVICE_KEY / SESSION_SERVICES_KEY global slots), service.ts:82-133
(PermissionQuery / PermissionsService); service-lifecycle.ts:52-113 (activate publishes keyed +
root slot, emitReady; announceReady latch; teardown unpublishes); index.ts:245-259
(LocalPermissionsService with formatter/extractor/authorizer registries), :275-281,
handlers/lifecycle.ts:80 and handlers/session-turn-prep.ts:56

**Port** (`crates/cyrup-permission-system`):

`rg -n "PermissionsService|permissions_service|PermissionQuery|permission_query|register_authorize
r|formatter_registry|access_extractor|permissions:ready|announce_ready"
/home/user/cyrup/crates/cyrup-permission-system/src` returns nothing; the only process-global
publication in the crate is the three-method yolo API (src/runtime_api.rs:58-80, published at
src/extension/native.rs:92).

## Why it matters

A sibling extension cannot ask this node whether an action is permitted before performing it,
cannot register a preview formatter or access extractor (so its tool inputs are prompted
unrendered and its resources go unextracted), and cannot install an authorizer chain link — every
such policy decision falls outside the permission system instead of through it. No consumer can
discover when the node is ready to answer.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute, but over-rated. Upstream confirmed: service.ts:75-80 (SERVICE_KEY /
SESSION_SERVICES_KEY), :83-118 PermissionQuery, service-lifecycle.ts:53-113
(activate/announceReady/teardown). Port: `grep -rn "PermissionsService|permissions_service|permiss
ions:ready|announce_ready|authorizer|formatter_registry|access_extractor" src/` returns zero hits;
`grep -rni "formatter|extractor" src/` returns only std::fmt::Formatter impls and prose.
src/runtime_api.rs:64 is the only process-global slot and it holds only the three yolo methods
(get/set/toggle). Downgraded to medium because nothing gets through THIS node's gate:
src/extension/decide.rs:65-155 still evaluates every tool_call, and no cyrup sibling extension
exists today that would consume the service — this is a missing extension point, not a bypass.
Useful correction for the fixer: src/yolo_api.rs:27-31 still claims "cyrup-ext has no extension-
provided-API registry ... Until that lands in cyrup-ext, no other extension or front-end can read
or flip yolo mode" — that doc is STALE, src/runtime_api.rs:64-100 already built exactly that
registry as a `static Mutex<Option<Arc<dyn ...>>>` with pi's identity-guarded unregister, so the
same mechanism can carry a PermissionsService slot with no host work.

## Also reported independently

Other area agents found this same gap from a different angle:

- **Publish the cross-extension PermissionsService and ready event** (service / service lifecycle) — Upstream publishes a per-session `PermissionsService` on a process-global slot and announces
`permissions:ready`, giving other extensions a policy query at gate parity plus registration
of tool-input formatters, path access extractors and authorizer chain links; the port
publishes only a three-method yolo control API.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
