---
stage: exec
status: done
updated: 2026-08-22 22:15
---

# broker/mod.rs Declares Four Children `pub mod` While Its Own Doc States the Public Surface Is `run` Alone

> Source: `intercom-hygiene-audit` workflow. Severity **low**, effort **small**.
> Every claim below was produced by a finder agent and then reproduced by an independent
> adversarial verifier; findings that did not reproduce were dropped.

## Scope

- `crates/cyrup-intercom/src/broker/mod.rs`
- `crates/cyrup-intercom/src/broker/listener.rs`
- `crates/cyrup-intercom/src/broker/ratelimit.rs`
- `crates/cyrup-intercom/src/broker/routing.rs`
- `crates/cyrup-intercom/src/broker/runtime_claim.rs`

## Description

`broker/mod.rs` documents its own visibility contract at lines 33-35: "Items are `pub(super)` …
while the crate's public surface stays [`run`] alone." The declaration list two lines below
contradicts it: of the 18 child modules, four (`listener`, `ratelimit`, `routing`,
`runtime_claim`, lines 37-40) are `pub mod` and the other fourteen (`mod` at :42-54 plus
`#[cfg(test)] mod test_support` at :57) are private. Because `lib.rs:17` declares `pub mod
broker`, those four export eight public items into `cyrup_intercom::broker::*`, none of which has
any consumer outside the crate.

## Why it matters

The decomposition's visibility contract is stated two lines above a declaration list that breaks
it, so a reader cannot tell which is authoritative — and the four modules pin eight internals as
semver-public API of `cyrup_intercom` with zero consumers to justify it. This is not a naming
preference: the doc makes a checkable claim about the crate's public surface and the code does not
satisfy it.

## Evidence

- src/broker/mod.rs:33-35: "Items are `pub(super)`, which from a child of `broker` means \"visible throughout `broker`\" … while the crate's public surface stays [`run`] alone."
- src/broker/mod.rs:37-40 `pub mod listener; pub mod ratelimit; pub mod routing; pub mod runtime_claim;` vs :42-54 thirteen private `mod` declarations (conn, dispatch, extensions, frame, js, lifecycle, limits, mailbox, presence, receipts, send, session, state), :56-57 `#[cfg(test)] mod test_support;`, :59 `pub use lifecycle::run;` — 18 children total
- crates/cyrup-intercom/src/lib.rs:17 `pub mod broker;` — the four are reachable as `cyrup_intercom::broker::*`
- `grep -n '^pub ' src/broker/{listener,ratelimit,routing,runtime_claim}.rs` → exactly eight items: `BrokerListener` (listener.rs:34), `unlink_stale_endpoint` (listener.rs:194), `RATE_LIMIT_CAPACITY` (ratelimit.rs:6), `RATE_LIMIT_REFILL_PER_SECOND` (ratelimit.rs:8), `TokenBucket` (ratelimit.rs:14), `AskEdge` (routing.rs:5), `find_session_ids` (routing.rs:18), `assert_no_live_broker` (runtime_claim.rs:63)
- `grep -rn 'cyrup_intercom::broker::' --include=*.rs .` excluding crates/cyrup-intercom/src returns exactly two hits, both `cyrup_intercom::broker::run`: crates/cyrup/src/intercom_broker_cmd.rs:7 (doc link) and :49 (the call)
- `grep -rnE 'BrokerListener|unlink_stale_endpoint|RATE_LIMIT_CAPACITY|RATE_LIMIT_REFILL|TokenBucket|AskEdge|find_session_ids|assert_no_live_broker' --include=*.rs .` excluding crates/cyrup-intercom/src returns zero hits — including across the 22 files of crates/cyrup-it/tests/intercom/
- The only use of any of the four from outside the `broker` module is inside the library's own test tree: src/session_state.rs:1167 `crate::broker::listener::BrokerListener::bind(&target)`, inside the `#[cfg(test)]` block that opens at src/session_state.rs:871. `assert_no_live_broker` is referenced only from src/broker/lifecycle.rs:132 and its own tests (src/broker/runtime_claim.rs:226-339)

## Required fix

Downgrade the four declarations at src/broker/mod.rs:37-40 to `pub(crate) mod` — NOT plain `mod`,
which would break src/session_state.rs:1167 (a `#[cfg(test)]` use of
`crate::broker::listener::BrokerListener`). Nothing outside the crate references them, so
`pub(crate)` compiles as-is and the eight items drop out of the public API. If any of the four is
genuinely intended to be public (e.g. `listener::BrokerListener` as a documented extension seam),
leave that one `pub mod` and amend the sentence at src/broker/mod.rs:35 to name it, so doc and
declarations agree either way. Do not touch the module boundaries themselves — only the visibility
keyword.

## Acceptance Criteria

- [ ] The fix above applied as written; no scope beyond it.
- [ ] Port fidelity preserved — no `broker.ts`/`client.ts`/`paths.ts` citation dropped, no
      `[CYRUP-DELTA]` note removed, no ported branch collapsed.
- [ ] Baseline recorded before the change and matched after:
      `cargo clippy -p cyrup-intercom --all-targets`, `cargo test -p cyrup-intercom --lib`.
- [ ] `cargo build -p cyrup` still succeeds.
