---
stage: qa
status: completed
updated: 2026-08-28 21:53
---

# ICOM-016 — Rework: `read_envelope` drops upstream's 64 KiB rejection

The port is complete and verified. One divergence from upstream remains, in the integrity check.

## Withdrawn: the previous rework item was wrong

A prior review claimed the `serialize_payload` CYRUP-DELTA was false because `serde_json::Map` is
insertion-ordered here. **It is not.** With build and proc-macro edges excluded — the actual link
graph — both `cyrup` and `cyrup-intercom` resolve `serde_json` to `[default, std]`: no
`preserve_order`, sorted `BTreeMap`. The comment as written is correct and must NOT be changed.

What produced the false finding, recorded so it is not repeated: `indexmap` appears in `Cargo.lock`'s
`serde_json` dependency list (the lock records a feature activated anywhere in the graph, including
build-dependencies and proc-macros), `cargo tree | grep -c preserve_order` counted a non-linking
edge, and a `preserve_order` grep hit in `xtask/Cargo.toml` is a comment explaining that xtask
deliberately AVOIDS `serde_json` for that very reason. `indexmap` does reach the normal graph, but via
`cyrup-mcp` and `gimli` as their own dependency, not as `serde_json`'s optional feature dep.

Use `cargo tree -p <crate> -e no-build,no-proc-macro --format "{p} [{f}]"` to settle questions of this
kind; nothing weaker is conclusive.

## Completed, not to be redone

All nine steps landed and were traced across two reviews: the `sha2` edges, `js_safe_u64`, the
318-line `broker/extension_state.rs`, both limit constants, the state fields with the constructor
threaded through all eight call sites, `NamespaceOwner` + `recompute_namespace_owners` +
`notify_namespace_capable`, the register path, the three real handlers, and every stale comment.

Verified correct and NOT to be revisited:

- All four `recompute_namespace_owners` sites match pi's `:337`, `:509`, `:544`, `:569`.
- A fresh epoch on owner change AND on socket change.
- `owner_order` read before `insert_session`, so a re-registering session cannot displace a later owner.
- Refusal ORDER matches upstream in both handlers: publish `:1271→:1311`, commit `:1368→:1465`.
- `read_envelope`'s other seven rejections match `extension-state.ts:68-108` exactly: missing file,
  unparseable, non-object (including arrays and `null`), `formatVersion != 1`, namespace mismatch,
  a revision that is not a non-negative safe integer, non-numeric `updatedAt`, non-string
  `payloadSha256`, and a payload whose re-hash differs.
- The two inverted `it` assertions are correct.

Gates at review: clippy 0, cyrup-intercom 280/280, workspace 8173/8173 (8 skipped), cyrup-it
`--features it` 477/477.

## Outstanding — the only item

`crates/cyrup-intercom/src/broker/extension_state.rs`, in `read_envelope`:

```rust
    let payload_json = serialize_payload(Some(&payload))?;
    if payload_hash(&payload_json) != stored_hash {
        return None;
    }
```

Upstream's equivalent (`extension-state.ts:92-95`) is:

```ts
    const payloadJson = serializePayload(envelope.payload);
    if (payloadJson === null || payloadHash(payloadJson) !== payloadSha256) return null;
```

and upstream's `serializePayload` (`:34-44`) returns `null` when the JSON exceeds `MAX_STATE_BYTES`.
So upstream REJECTS a stored envelope whose payload is over 64 KiB, even when its hash matches, and
falls through to the `.bak`.

This port's `serialize_payload` deliberately omits that cap — it is shared with the publish path,
which applies 16 KiB instead — so `read_envelope` currently accepts an oversized envelope, caches it,
and replays it to every capable session. That is a missing rejection in an integrity check, in a
module whose own doc calls itself a 1:1 port, and the definition of done covers the corruption case
explicitly.

Reachability is low and should be stated plainly rather than overstated: neither cyrup nor pi ever
writes such a file, because both cap on commit, and the state directory is 0700 so only the same user
can place one. It is reachable only by tampering or by a file from a modified implementation.

### Fix

Apply the state cap at the read, leaving `serialize_payload` itself uncapped so the publish path is
unaffected:

```rust
    let payload_json =
        serialize_payload(Some(&payload)).filter(|j| j.len() <= MAX_EXTENSION_STATE_BYTES)?;
```

Add a short comment naming `extension-state.ts:92` and the reason the cap lives at the call site
here but inside `serializePayload` upstream.

While in that function, decide the adjacent case deliberately and say which you chose: an envelope
with NO `payload` key. Upstream's `JSON.stringify(undefined)` is `undefined`, so `serializePayload`
returns `null` and the envelope is REJECTED. This port does
`obj.get("payload").cloned().unwrap_or(serde_json::Value::Null)`, so an absent key is read as `null`
and accepted if the stored hash happens to be `sha256("null")`. Matching upstream means rejecting
when the key is absent rather than defaulting it.

Do not change `serialize_payload`'s own signature or behaviour — the 16 KiB publish path depends on
it staying uncapped.

## Definition of done for this rework

- `read_envelope` rejects a stored payload over `MAX_EXTENSION_STATE_BYTES`.
- The absent-`payload`-key case is handled deliberately and documented, matching upstream's rejection.
- `serialize_payload` is unchanged, and the publish path still applies its own 16 KiB cap.
- `cargo clippy -p cyrup-intercom --all-targets` stays at 0.
- `cargo check --workspace --all-targets` stays clean, and `cargo nextest run -p cyrup-intercom`
  stays at 280/280.
- No file other than `crates/cyrup-intercom/src/broker/extension_state.rs` is touched.

Two lines on a read path with no behavioural reach into the handlers — re-running the `it` suite is
not required.

## Follow-up, file separately

`broker/extension_state.rs` still has no direct test coverage: `commit_state` has one call site
outside the module and no test reaches it. Restart replay, the `.bak` fallback, temp-file cleanup and
the unwritable-directory refusal are all verified by reading only — and this review found a real
divergence in that same unexercised function, which is the argument for the coverage.
