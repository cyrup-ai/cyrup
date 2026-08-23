---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the key-name log redactor into the JSONL writer

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | critical |
| **Kind** | absent |
| **Upstream area** | logging, redaction and log hygiene |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream masks every log value bound to a credential-shaped key name (authorization / api-key /
secret / token / password / credential / cookie / private-key) before the line is serialized; the
Rust writer serializes the details record verbatim, so those values land in the JSONL trail in the
clear.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/log-redaction.ts:17-44
(REDACTED_PLACEHOLDER, SENSITIVE_KEY_PATTERN, isSensitiveLogKey, redactedJsonStringify); wired as
the ONLY serializer at logging.ts:66-72 (`const line = redactedJsonStringify({timestamp,
extension, stream, event, ...bounded})`)

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/logging.rs:211 serializes with the plain
`safe_json_stringify` (logging.rs:109-111 = `serde_json::to_string(value).ok()`), no masking pass.
`rg -n "is_sensitive_log_key|SENSITIVE_KEY_PATTERN|REDACTED_PLACEHOLDER|redacted_json_stringify"
/home/user/cyrup/crates/cyrup-permission-system/src` returns nothing; `rg -n -i "redact" <src>`
returns only three doc-comment mentions (logging.rs:377,393,516) and no implementation.

## Why it matters

Any decision record whose details carry a credential-named key is written to disk in plaintext.
The review record built at extension/audit.rs:64-78 embeds the raw `toolInput` JSON of the gated
call, so a bash/mcp/write tool invocation carrying `{"env":{"API_KEY":"..."}}`, an `authorization`
header, or a `token` field is persisted verbatim into cyrup-permission-system-debug.jsonl.
Upstream's stated security boundary (a value bound to a sensitive key name is masked) does not
hold in the port at all.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
