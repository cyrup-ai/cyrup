---
title: Parity gap backlog — CYRUP-DELTA capability gaps + open questions
stage: new
status: pending
updated: 2026-08-28
---

# Parity gap backlog

Filed from the `close-parity-gaps` workflow (`wf_12c49023-adf`) and its CYRUP-DELTA
classification audit, run against pi at `e8682309`.

**Provenance matters here.** Every item was previously either marked "out of scope" by an
agent, or recorded as a `CYRUP-DELTA` by an agent. No human authorized any of it. These are
filed so they are David's decisions rather than accumulated artifacts.

## The audit

| classification | count |
| --- | --- |
| mechanism-only (caller cannot observe) | 55 |
| **capability gap (caller CAN observe)** | **31** |
| unverifiable on this host | 1 |
| total markers | 87 |

Of the 31, **6 are cross-references** to another site's gap — 25 are distinct.
Spread: 20 in `cyrup-provider`, 11 in `cyrup-tools`.

I previously reported this as "at least two real capability gaps". That was wrong by an
order of magnitude; the audit is the correction.

## The highest-impact ones

- `compat.rs:781` — `temperature` dropped for reasoning models over the Responses API
  (gpt-5, o-series). The audit calls this the common case, not an edge case.
- `google_adc.rs:19` — workload-identity-federation (`external_account`) credentials
  unsupported; the standard way to auth Google in CI.
- `grep.rs` — ripgrep config files not honoured, so the same query returns a different
  match set than pi for any user with an `rg` config.
- `read.rs:221` — a negative `limit` returns an empty window where pi returns a real slice.
- `bash.rs:236/312` — `PI_SESSION_ID` / `AI_AGENT` values differ, so a user hook that
  branches on them takes the other branch.

## Backlog

| priority | crate | task |
| --- | --- | --- |
| LOW | - | [`LOW-delta-cross-reference-sites.md`](LOW-delta-cross-reference-sites.md) |
| MEDIUM | - | [`MEDIUM-delta-unverifiable-on-this-host.md`](MEDIUM-delta-unverifiable-on-this-host.md) |
| MEDIUM | - | [`MEDIUM-open-questions-from-gap-closure.md`](MEDIUM-open-questions-from-gap-closure.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-api-compat-rs-781.md`](MEDIUM-delta-cyrup-provider-src-api-compat-rs-781.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-api-google-vertex-rs-41.md`](MEDIUM-delta-cyrup-provider-src-api-google-vertex-rs-41.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-api-openai-completions-params-rs-102.md`](MEDIUM-delta-cyrup-provider-src-api-openai-completions-params-rs-102.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-auth-google-adc-rs-19.md`](MEDIUM-delta-cyrup-provider-src-auth-google-adc-rs-19.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-auth-google-adc-rs-300.md`](MEDIUM-delta-cyrup-provider-src-auth-google-adc-rs-300.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-collection-rs-421.md`](MEDIUM-delta-cyrup-provider-src-collection-rs-421.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-collection-rs-534.md`](MEDIUM-delta-cyrup-provider-src-collection-rs-534.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-provider-rs-17.md`](MEDIUM-delta-cyrup-provider-src-provider-rs-17.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-providers-fleet-rs-270.md`](MEDIUM-delta-cyrup-provider-src-providers-fleet-rs-270.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-providers-github-copilot-rs-197.md`](MEDIUM-delta-cyrup-provider-src-providers-github-copilot-rs-197.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-remote-catalog-rs-144.md`](MEDIUM-delta-cyrup-provider-src-remote-catalog-rs-144.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-stream-sse-rs-27.md`](MEDIUM-delta-cyrup-provider-src-stream-sse-rs-27.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-utils-error-body-rs-35.md`](MEDIUM-delta-cyrup-provider-src-utils-error-body-rs-35.md) |
| MEDIUM | cyrup-provider | [`MEDIUM-delta-cyrup-provider-src-utils-retry-rs-43.md`](MEDIUM-delta-cyrup-provider-src-utils-retry-rs-43.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-isolation-mod-rs-12.md`](MEDIUM-delta-cyrup-tools-src-isolation-mod-rs-12.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-ops-local-fs-rs-154.md`](MEDIUM-delta-cyrup-tools-src-ops-local-fs-rs-154.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-ops-mod-rs-539.md`](MEDIUM-delta-cyrup-tools-src-ops-mod-rs-539.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-path-rs-161.md`](MEDIUM-delta-cyrup-tools-src-path-rs-161.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-tools-bash-rs-214.md`](MEDIUM-delta-cyrup-tools-src-tools-bash-rs-214.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-tools-bash-rs-236.md`](MEDIUM-delta-cyrup-tools-src-tools-bash-rs-236.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-tools-bash-rs-312.md`](MEDIUM-delta-cyrup-tools-src-tools-bash-rs-312.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-tools-bash-rs-72.md`](MEDIUM-delta-cyrup-tools-src-tools-bash-rs-72.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-tools-find-rs-1.md`](MEDIUM-delta-cyrup-tools-src-tools-find-rs-1.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-tools-grep-rs-1.md`](MEDIUM-delta-cyrup-tools-src-tools-grep-rs-1.md) |
| MEDIUM | cyrup-tools | [`MEDIUM-delta-cyrup-tools-src-tools-read-rs-221.md`](MEDIUM-delta-cyrup-tools-src-tools-read-rs-221.md) |

## Also open

`MEDIUM-open-questions-from-gap-closure.md` carries 26 items the closure agents surfaced
rather than deciding — including three concrete asks: extracting `build_matcher` so the
`multi_line` guard drives production code, porting JS coercion for non-number JSON args
across five sites, and a `cfg(windows)` case-folded compare for `cwd_relative_path`.
