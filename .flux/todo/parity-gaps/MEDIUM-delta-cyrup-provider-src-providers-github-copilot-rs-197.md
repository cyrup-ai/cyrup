---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/providers/github_copilot.rs:197"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/providers/github_copilot.rs:197`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

`normalizeDomain` (packages/ai/src/auth/oauth/github-copilot.ts:41-49 @e8682309) is `new URL(...).hostname` with a `catch -> null`. Verified by running node 22 in this container: it punycodes IDNs (`münchen.example.com` -> `xn--mnchen-3ya.example.com`) and REJECTS malformed hosts (`exa mple.com` -> throws -> null). The caller then does `if (trimmed && !enterpriseDomain) throw new Error("Invalid GitHub Enterprise URL/domain")` (:444), and `copilotEnterpriseDomain` (:487-491) falls back to `github.com` for an invalid stored value.

## What cyrup does

Hand-rolled scan (path of scheme/userinfo/host/port), ASCII-lowercased, no IDNA and no host-character validation. `münchen.example.com` comes back as-is; `exa mple.com` comes back as `Some("exa mple.com")`.

## What a caller sees

(1) A GitHub Enterprise domain that pi rejects up front with `Invalid GitHub Enterprise URL/domain` is ACCEPTED by cyrup, which then builds `https://exa mple.com/login/device/code` and fails later with an opaque URL/transport error — a worse diagnostic at a later point in the login flow. (2) A non-ASCII enterprise domain yields a different stored/derived host than pi's punycoded one. The marker's claim that the substitution 'covers every form this flow accepts' is not correct as written.

## Decision required

One of:

1. **Close it** — bring cyrup to pi's behaviour.
2. **Accept it** — David explicitly accepts the divergence; the marker stays and is
   annotated as authorized, with the reason.
3. **Reshape it** — the divergence is right but the current form is wrong.

Do not silently keep option 2 by leaving the marker as-is; that is how this became a
backlog in the first place.

## Definition of done

1. The gap is closed, or the marker records an explicit authorized acceptance.
2. If closed, a test fails without the change.
3. No behaviour regression in the owning crate.
