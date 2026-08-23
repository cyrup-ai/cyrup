---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the authorizer chain: registry, composition, and delegation envelope

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | low |
| **Kind** | absent |
| **Upstream area** | authority: authorizer chain (ADR 0007) |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream has a whole live-authority chain — named links registered by sibling extensions, resolved
in operator `authorizerChain` order, each returning allow/deny/defer ahead of the terminal, with a
bounded-delegation checkpoint that caps a link's `allow` on excluded surfaces; none of it exists
in the port.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/authority/authorizer.ts:23-26 (`AuthorizerVerdict` allow/deny/defer), :37-56 (`Authorizer`,
`NamedAuthorizer`), :69-95 (`TerminalAuthorizer`, `SelectedAuthority.adjudicatesLocally`),
:127-156 (`selectAuthorizer` hasUI/isSubagent/deny dispatch); src/authority/authorizer-
chain.ts:30-90 (`composeAuthorizerChain`, `decideFromVerdict`); src/authority/authorizer-
registry.ts:48-119 (`AuthorizerRegistry` one-link-per-name + `ObservedAuthorizerRegistrar` vacancy
logging); src/authority/authorizer-selection.ts:110-165 (config-order resolution,
`authorizer_chain_delegated` / `_unregistered_link` / `_resolved` review entries), :199-215
(`escalate` composes per ask); src/authority/delegation-envelope.ts:21-53
(`DELEGATION_EXCLUDED_SURFACES` = external_directory + path; an `allow` there is downgraded to
`defer`, undetermined surface fail-safes to excluded)

**Port** (`crates/cyrup-permission-system`):

`rg -in "authorizer|authorizer_chain|registerAuthorizer" /home/user/cyrup/crates/cyrup-permission-
system/src` returns 0 matches across 58 files; `rg -in "delegation" /home/user/cyrup/crates/cyrup-
permission-system/src` returns no delegation-envelope code. The port's ask tier
(/home/user/cyrup/crates/cyrup-permission-system/src/extension/prompt.rs:70-140) goes straight
from dedup/yolo to a single `AskChannel`, with no pre-terminal links.

## Why it matters

No sibling extension can pre-screen asks (an operator's `authorizerChain` config is silently
inert), and because the bounded-delegation checkpoint does not exist either, any future link the
port does add would arrive with no surface-exclusion cap on grants — path and external_directory
allows would go through unbounded.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

CONFIRMED ABSENT as a subsystem: `grep -rin "authorizer|authoriser" src` returns 0 and `grep -rn
"authorizerChain|authorizer_chain" src` returns 0; the ask tier (/home/user/cyrup/crates/cyrup-
permission-system/src/extension/prompt.rs:70-140) goes dedup -> yolo -> one AskChannel. BUT the
finder's stated impact is factually wrong: the port has NO authorizerChain config key at all
(ext_config.rs writes only "debug" and "yoloMode", ext_config.rs:467-468), so no operator setting
is being silently ignored — there is nothing to be inert. And the delegation-envelope half is a
cap on a capability that does not exist, so its absence cannot let anything through today
(upstream itself notes the checkpoint is 'dormant while the only registered links are deny-first',
delegation-envelope.ts:14). SEVERITY LOWERED high->low: nothing additional reaches the gate; this
is an architectural extension point (ADR 0007) the port never had, and it is coupled to the v27
restructure the port predates. If it is ever added, delegation-envelope.ts:21-53 must land in the
same change, not after.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
