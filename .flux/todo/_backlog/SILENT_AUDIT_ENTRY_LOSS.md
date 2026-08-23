---
stage: exec
status: completed
updated: 2026-08-22 22:10
---

# Five Of Six append_entry Call Sites Drop The Error, Against Comments Demanding The Record Be Durable

> Source: `intercom-hygiene-audit` workflow. Severity **medium**, effort **small**.
> Every claim below was produced by a finder agent and then reproduced by an independent
> adversarial verifier; findings that did not reproduce were dropped.

## Scope

- `crates/cyrup-intercom/src/tools/intercom.rs`
- `crates/cyrup-intercom/src/extension.rs`
- `crates/cyrup-intercom/src/inbound.rs`

## Description

`HostServices::append_entry` returns `Result<String, String>` (trait declaration crates/cyrup-
ext/src/host/services.rs:593, whose DEFAULT body is `Err("append_entry not available")`). I re-ran
`grep -rn 'append_entry' crates/cyrup-intercom/src/` and separated call sites from doc mentions:
there are exactly six calls. One — src/inbound.rs:618-622 — matches on the Result and logs
`tracing::warn!(error = %e, ...)`. The other five (src/tools/intercom.rs:429, :550, :567, :628 and
src/extension.rs:400) are bare `let _ = services.append_entry(...)`, with no log and no fallback.
All five discards are the outbound half of the audit trail (`intercom_sent` x4,
`intercom_received` x1), and two of them sit directly under comments insisting that exact record
must not be lost.

## Why it matters

The audit trail is the only durable evidence that an intercom exchange happened, and the code's
own comments say a loss here is "undiscoverable afterwards". With `let _ =`, that is literally
true: if the host has no append_entry capability (the trait default returns Err unconditionally)
or the live session manager errors, every outbound `intercom_sent`/`intercom_received` entry
vanishes with zero diagnostic — no warn line, no metric, nothing to grep for. The tool still
reports the send as delivered, so the transcript and the wire disagree and nobody can tell why.
The inbound path already proves a cheap answer exists in this same crate.

## Evidence

- Re-ran `grep -rn 'append_entry' crates/cyrup-intercom/src/` and classified every hit: 6 call sites (src/tools/intercom.rs:429, :550, :567, :628; src/extension.rs:400; src/inbound.rs:618) plus module/doc mentions (src/seams.rs:22,143,490,520,534; src/ui/mod.rs:6,60; src/ui/inline_message.rs:9,21; src/inbound.rs:8,28,34,50,597,1041,1062; src/extension.rs:25,509) and two test-double impls (src/seams.rs:498, src/inbound.rs:839)
- crates/cyrup-intercom/src/inbound.rs:618-623 — the only handling site: `match services.append_entry(INBOUND_MESSAGE_CUSTOM_TYPE, &payload) { Ok(id) => Some(id), Err(e) => { tracing::warn!(error = %e, "intercom: failed to surface inbound message via append_entry"); None } }`
- crates/cyrup-intercom/src/tools/intercom.rs:429 and :550 — `let _ = services.append_entry("intercom_sent", ...)`; :567 — `let _ = services.append_entry("intercom_received", ...)`; :628 — `let _ = services.append_entry("intercom_sent", ...)`
- crates/cyrup-intercom/src/tools/intercom.rs:564-566, the comment immediately above the discarded :567 call — "The durable record of an exchange has to match what was exchanged, or the loss is undiscoverable afterwards."
- crates/cyrup-intercom/src/extension.rs:400 — `let _ = services.append_entry("intercom_sent", ...)`; the comment at :378-385 records that this leg was previously "the ONLY send in the crate that left no trace in the transcript" and that such a session "had an audit log that silently omitted its own outbound messages"
- crates/cyrup-ext/src/host/services.rs:592-595 — trait default: `fn append_entry(&self, _custom_type: &str, _data: &Value) -> Result<String, String> { Err("append_entry not available".into()) }`, i.e. on any host that does not override it EVERY call returns Err
- crates/cyrup-session-svc/src/host_services.rs:1561-1572 — the production override; it can fail via `with_manager(...)` / `append_custom_entry(...).map_err(|e| e.to_string())?`, so Err is reachable in the real host too, not only in the degraded default
- `grep -rn 'best-effort|fire-and-forget|ignore the error|deliberately discard' crates/cyrup-intercom/src/` returns hits only in cwd.rs, broker/frame.rs, transport/framing.rs and tools/contact_supervisor.rs — none at or near any append_entry call, so no documented discard policy covers these five

## Required fix

Adopt the inbound.rs:618 handling at the five outbound sites: replace each `let _ =
services.append_entry(kind, &payload);` with `if let Err(e) = services.append_entry(kind,
&payload) { tracing::warn!(error = %e, kind, "intercom: failed to append audit entry"); }` at
src/tools/intercom.rs:429, :550, :567, :628 and src/extension.rs:400. Do not change control flow —
the send genuinely happened and must still be reported delivered; the only goal is that the audit
loss stops being invisible, which is exactly what the comments at src/tools/intercom.rs:564-566
and src/extension.rs:378-385 already demand.

## Acceptance Criteria

- [ ] The fix above applied as written; no scope beyond it.
- [ ] Port fidelity preserved — no `broker.ts`/`client.ts`/`paths.ts` citation dropped, no
      `[CYRUP-DELTA]` note removed, no ported branch collapsed.
- [ ] Baseline recorded before the change and matched after:
      `cargo clippy -p cyrup-intercom --all-targets`, `cargo test -p cyrup-intercom --lib`.
- [ ] `cargo build -p cyrup` still succeeds.
