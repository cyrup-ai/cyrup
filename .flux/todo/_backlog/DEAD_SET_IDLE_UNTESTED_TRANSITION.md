---
stage: exec
status: done
updated: 2026-08-23
---

# IdleControlledHost::set_idle Is Never Called and Its Doc Comment Advertises Machinery Deleted Upstream at v0.9

> Source: `intercom-hygiene-audit` workflow. Severity **low**, effort **small**.
> Every claim below was produced by a finder agent and then reproduced by an independent
> adversarial verifier; findings that did not reproduce were dropped.

## Scope

- `crates/cyrup-intercom/src/inbound.rs`

## Description

src/inbound.rs:807-828 defines IdleControlledHost, documented at :807-809 as "A `HostServices`
double with a SETTABLE `is_idle` ... recording every `inject_message` so the pending-idle flush's
real delivery is observable." Both halves fail. `grep -rn 'set_idle' crates/cyrup-intercom/src/`
returns exactly two lines: the definition at :823 and a mention inside a doc comment at :949. Zero
call expressions. It carries `#[allow(dead_code)]` at :822, and `grep -rn 'allow(dead_code)'
--include='*.rs' src/` returns that one line and nothing else — it is the only dead-code
suppression in the crate. All seven constructions (:880, :954, :980, :1016, :1045, :1095, :1131)
pass a literal and never transition it, so the AtomicBool and its SeqCst loads model a mutability
nothing exercises. And the "pending-idle flush" the doc promises to make observable does not
exist: src/inbound.rs:72-75 records that upstream v0.9.3 (25ffb96) "deleted the whole park-until-
idle machine" and names the eight removed symbols, and crates/cyrup-
it/tests/intercom/dismiss_incoming_ask.rs:12-13 confirms flush_idle_messages and
pending_inbound_len no longer exist in cyrup either. The only live set_idle caller in the repo is
crates/cyrup-it/tests/intercom/dismiss_incoming_ask.rs:206, in a target gated behind `required-
features = ["it"]` (Cargo.toml:182) with `default = []` (:36), so `cargo test --workspace` never
builds it. I corrected the severity down and rewrote the fix: the original's preferred remedy (add
an in-crate transition test) buys almost nothing, because `decide_inbound_policy` is called as
`decide_inbound_policy(s.is_idle(), s.has_ui(), ...)` — a pure function re-reading the flag on
every call — and both boolean values are already covered by the seven fixed-value constructions.

## Why it matters

The doc comment at :807-809 tells the next reader that this double exists to observe "the pending-
idle flush's real delivery" — machinery that upstream deleted at v0.9.3 and that this very file
documents as gone forty lines earlier at :72-75. A reader trusting that sentence will go looking
for a flush path that does not exist. Alongside it sits the crate's only `#[allow(dead_code)]`,
suppressing a warning about a setter with zero callers, which is precisely the signal that would
otherwise have flagged the staleness. This is test-double hygiene with no production reach, hence
low rather than medium, but it is dead weight that actively misdirects.

## Evidence

- src/inbound.rs:822 — `#[allow(dead_code)]`; `grep -rn 'allow(dead_code)' --include='*.rs' crates/cyrup-intercom/src/ | wc -l` → 1, and the single hit is this line
- src/inbound.rs:823 — `fn set_idle(&self, idle: bool) {`; `grep -rn 'set_idle' --include='*.rs' crates/cyrup-intercom/src/` returns only :823 (the definition) and :949 (prose inside a doc comment) — zero call expressions
- src/inbound.rs:807-809 — `/// A \`HostServices\` double with a SETTABLE \`is_idle\` — the live run-in-flight signal` / `/// (\`cyrup_ext::HostServices::is_idle\`, pi \`ctx.isIdle()\`) — recording every \`inject_message\`` / `/// so the pending-idle flush's real delivery is observable.`
- src/inbound.rs:72-75 — `/// v0.9.3 (\`25ffb96\`, "fix: steer busy inbound messages promptly") deleted the whole` / `/// park-until-idle machine — \`pendingIdleMessages\`, \`queueIdleMessage\`, \`scheduleInboundFlush\`,` / `/// \`flushIdleMessages\`, \`clearInboundFlushTimer\`, \`expirePendingIdleMessages\`,` / `/// \`INBOUND_FLUSH_DELAY_MS\` and \`INBOUND_IDLE_RETRY_MS\` — and replaced it with this one line.`
- crates/cyrup-it/tests/intercom/dismiss_incoming_ask.rs:12-13 — `//! its tests with it; \`flush_idle_messages\` and \`SharedIntercomState::pending_inbound_len\` no` / `//! longer exist in cyrup either.`
- All 7 constructions pass a literal and never mutate: src/inbound.rs:880 `IdleControlledHost::new(true)`, :954 `new(false)`, :980 `new(false)`, :1016 `new(true)`, :1045 `new(true)`, :1095 `new(false)`, :1131 `new(true)`
- `grep -rn 'set_idle' --include=*.rs crates/` repo-wide → the in-crate definition, a FOURTH near-copy of IdleControlledHost at crates/cyrup-it/tests/intercom/dismiss_incoming_ask.rs:50-60 (with `injected: Mutex<Vec<String>>` rather than the in-crate `Vec<InjectedCall>`), its call at :206 `host.set_idle(true);`, and three unrelated `set_idle_shutdown_callback` hits in crates/cyrup-mcp/src/lifecycle.rs
- crates/cyrup-it/Cargo.toml:36 `default = []`, :53 `it = []`, :180-182 `name = "intercom"` / `path = "tests/intercom/main.rs"` / `required-features = ["it"]` — the only live caller is off under `cargo test --workspace`
- src/inbound.rs:959 — the in-crate tests call `decide_inbound_policy(s.is_idle(), s.has_ui(), s.config.inbound_trigger, &ask("first"))`, i.e. a pure function taking the flag by value, so a mid-test transition is not observable to it beyond what a second call with the other literal already covers

## Required fix

Rewrite src/inbound.rs:807-809 so it stops promising observability of the deleted pending-idle
flush; describe what the double actually does (records inject_message calls under a fixed is_idle
value) and cross-reference :72-75 for why the flush is gone. Then delete `set_idle` and the
`#[allow(dead_code)]` at :822, and collapse `idle: AtomicBool` to a plain `bool` (the SeqCst loads
model a mutability no in-crate caller uses), leaving a comment naming crates/cyrup-
it/tests/intercom/dismiss_incoming_ask.rs:206 as the sole owner of the busy-to-idle transition
contract. Do NOT add an in-crate transition test as the original fix preferred:
decide_inbound_policy takes the flag by value (:959), so both branches are already covered by the
seven fixed-value constructions and a mid-test flip would assert nothing new — the transition is
only meaningful over the real socket, where it is already pinned.

## Acceptance Criteria

- [ ] The fix above applied as written; no scope beyond it.
- [ ] Port fidelity preserved — no `broker.ts`/`client.ts`/`paths.ts` citation dropped, no
      `[CYRUP-DELTA]` note removed, no ported branch collapsed.
- [ ] Baseline recorded before the change and matched after:
      `cargo clippy -p cyrup-intercom --all-targets`, `cargo test -p cyrup-intercom --lib`.
- [ ] `cargo build -p cyrup` still succeeds.
