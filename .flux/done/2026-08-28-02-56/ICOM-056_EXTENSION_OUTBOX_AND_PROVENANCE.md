---
stage: qa
status: completed
updated: 2026-08-28 20:24
---

# ICOM-056 — Rework: `delivery_failed` can emit an empty `messageId`

The port is complete and verified; one edge-case defect on the contract surface remains.

## Completed, not to be redone

The whole outbox landed and was verified by inspection: `MessageProvenance`/`ProvenanceKind` on the
envelope with `present_non_null` (absent legal, `null` fatal, unknown `type` fatal), `SendOptions.provenance`
threaded through `IntercomClient::send`, the new `outbox.rs` (four topics, both V1 envelopes,
`parse_outbox_request`, `resolve_outbox_target`, build/emit/settle/fail, the generation-fenced
delivery leg), the two state maps plus the registration store, the drains in `begin_runtime` and
`shutdown`, `subscribe_bus` ×2 with the `registry-ready` emit after them, and the card attribution in
both modes at their two different positions.

Confirmed green: clippy 0, cyrup-intercom 280/280, workspace 8173/8173 (8 skipped), cyrup-it
`--features it` 477/477. 11 files changed, none outside `crates/cyrup-intercom/src`.

Do NOT re-derive any of the above, and do not touch `broker/extensions.rs` or advertise
`EXTENSION_BUS_FEATURE` — that boundary is correct and is ICOM-016's.

## Outstanding — the only item

`crates/cyrup-intercom/src/outbox.rs:591-600` passes `Some(&result.id)` as the `messageId` on the
`delivery_failed` settle.

That is right for a broker `DeliveryFailed` frame: `transport/client.rs:871-878` builds
`SendResult { id: message_id, .. }` from the frame's own id.

It is wrong when the client disconnects with the send in flight. The teardown at
`transport/client.rs:189` and `:697` synthesizes `SendResult { id: String::new(), delivered: false,
reason: Some(..) }`. That reaches `send()` as `Ok(result)`, falls into the `!result.delivered` branch,
and emits `"messageId": ""` to the extension.

DoD item 5 requires this settle to carry **the attempted messageId**. An empty string is not that,
and `messageId` is the field an extension correlates a result back to its request on — an empty one
is worse than omitting the key entirely.

The attempted id is always knowable: `handle_outbox_request` sets `message_id: Some(request_id)` on
the send, so the attempted message id IS `request.request_id`.

### Fix

At the `!result.delivered` branch, fall back to the request id when the broker gave no id:

```rust
    if !result.delivered {
        let detail = result.reason.clone().unwrap_or_else(|| "Delivery failed".to_string());
        // `SendResult.id` is empty when the client tore down with the send in flight
        // (`transport/client.rs:189`, `:697`) rather than answering with a `DeliveryFailed` frame.
        // The attempted id is still known — it is the requestId we sent as `message_id`.
        let attempted = if result.id.is_empty() { rid } else { result.id.as_str() };
        settle(
            OutboxResultStatus::Failed,
            OutboxResultCode::DeliveryFailed,
            Some(attempted),
            &detail,
        );
        return;
    }
```

Keep `delivery_failed` as the code. A disconnect racing the send lands here upstream too; only the
empty id is a cyrup-specific artifact, and only that is being corrected.

Do not "fix" this in `transport/client.rs` by giving the teardown sites a real id — they have no
message id in scope at that point, and changing `SendResult`'s contract would ripple to every other
caller for one edge case that the outbox alone exposes.

## Definition of done for this rework

- `delivery_failed` never emits an empty `messageId`; when the broker supplies no id, the settle
  carries `request.request_id`.
- A broker `DeliveryFailed` frame still reports that frame's own id, unchanged.
- The code stays `delivery_failed` / `failed`; no status or code changes.
- `transport/client.rs` is NOT modified.
- `cargo clippy -p cyrup-intercom --all-targets` stays at 0.
- `cargo check --workspace --all-targets` stays clean.
- No file outside `crates/cyrup-intercom/src/outbox.rs` is touched.

One-line behavioural change on an error path — re-running the full `it` suite is not required; the
crate's own tests plus clippy are sufficient.
