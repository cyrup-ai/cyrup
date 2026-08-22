---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the session-approval pattern suggester and bash arity table

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | absent |
| **Upstream area** | prompts — pattern-suggest / bash-arity |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream derives the "for this session" grant from a curated command-arity dictionary (`git
checkout *`, `npm run build*`), an MCP `server:*` rule and a path pattern, and labels the dialog
option per surface — including a two-option subagent-vs-whole-session scope for forwarded asks;
the port stores the literal command/target/path with no suggestion, label, or scope choice.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/pattern-suggest.ts:25-39
(suggestBashPattern), :48-62 (suggestMcpPattern), :79-91 (buildForwardedScopeLabels), :94-114
(surface-aware buildLabel), :128-151 (suggestSessionPattern), :161-170
(suggestPathSessionPattern); /home/user/cyrup/tmp/pi-packages/packages/pi-permission-
system/src/bash-arity.ts:86-226 (ARITY table), :242-259 (longest-match prefix), :279-283
(stripBashCommentLines); consumed at handlers/gates/tool.ts:85-90,124 and authority/local-user-
authorizer.ts:70-86

**Port** (`crates/cyrup-permission-system`):

`rg -ni "arity|suggest_bash_pattern|suggest_mcp_pattern|suggest_session_pattern|strip_bash_comment
|session_label|subagent only" /home/user/cyrup/crates/cyrup-permission-system/src` → 0 matches.
The port's "Allow Always" path is /home/user/cyrup/crates/cyrup-permission-
system/src/extension/prompt.rs:296-306 storing `gate::get_pattern_approval_subject(check, input)`
verbatim (/home/user/cyrup/crates/cyrup-permission-system/src/gate.rs:152-170), i.e. the exact
command string / target / normalized path.

## Why it matters

Two losses. The dialog never tells the operator what breadth they are granting (no surface-aware
label naming the pattern), and a forwarded approval cannot be confined to the requesting subagent
— upstream's least-privilege default option — so the only available grant is the broad one.
Meanwhile every literal-only session rule means near-identical commands re-prompt indefinitely,
which is the prompt fatigue that pushes operators to yolo mode.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
