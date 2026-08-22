---
stage: qa
status: completed
updated: 2026-08-22 19:15
---

# Add `#[must_use]` To `AgentBuilder`'s Consuming Setters And To `drain_queues_for_restore`

## Description
The non-test source of `cyrup-agent` contains exactly one `#[must_use]`. Verified: `grep -rn must_use crates/cyrup-agent/src --include=*.rs | grep -v /tests/` returns two lines — [proxy.rs:736](../../crates/cyrup-agent/src/proxy.rs) (the attribute) and [agent/facade.rs:21](../../crates/cyrup-agent/src/agent/facade.rs), a doc comment reading `Deliberately NOT #[must_use]: pi's callers discard the returned closure...`. The convention exists and is documented where it is intentionally waived; [agent/builder.rs](../../crates/cyrup-agent/src/agent/builder.rs) is simply where it was never applied.

`builder.rs` is 249 lines and is a fully consuming builder: `grep -c "mut self"` = 25, and every one is `pub fn <name>(mut self, ...) -> Self` at lines 56, 61, 66, 71, 76, 81, 86, 91, 96, 101, 109, 115, 121, 127, 133, 139, 145, 153, 160, 168, 175, 184, 195, 202, 209 (`system_prompt`, `thinking_level`, `tools`, `messages`, `hooks`, `key_resolver`, `steering_mode`, `follow_up_mode`, `tool_execution`, `session_id`, `temperature`, `max_tokens`, `cache_retention`, `headers`, `transport`, `max_retry_delay_ms`, `max_retries`, `thinking_budgets`, `api_key`, `provider_env`, `metadata`, `websocket_connect_timeout_ms`, `timeout_ms`, `on_payload`, `on_response`), plus `pub fn new(...) -> Self` at :33 and `pub fn build(self) -> Agent` at :214. Because each takes `self` by value and returns `Self`, `builder.temperature(0.7);` written as a statement compiles silently and discards the builder along with everything chained before it. This is the crate's primary entry point — `lib.rs:22` re-exports `AgentBuilder` and `lib.rs:6` names it as the way an `Agent` is constructed.

`facade.rs:152` is `pub fn drain_queues_for_restore(&self) -> (Vec<AgentMessage>, Vec<AgentMessage>)` with body `(lock(&self.steering).take_all(), lock(&self.follow_up).take_all())`; [queue.rs:59-61](../../crates/cyrup-agent/src/queue.rs)'s `take_all` is `self.items.drain(..).collect()`. Dropping the returned tuple permanently loses every queued steering and follow-up message with no error and no log — the only destructive read on the queue surface.

Correction to the original evidence, verified by reading the call site: the sole in-tree caller, [cyrup-session-svc/src/session.rs:1494](../../crates/cyrup-session-svc/src/session.rs), does **not** bind the result — it is the bare statement `self.agent.drain_queues_for_restore();` inside `drain_queue`. That discard is deliberate (the facade mirrors above it supply the returned text; the agent's copies are duplicates), so this task must make it an explicit discard rather than treat it as a bug.

Sizing the regression guard: after `touch crates/cyrup-agent/src/lib.rs`, `cargo clippy -p cyrup-agent --all-targets -- -W clippy::pedantic` reports `clippy::return_self_not_must_use` at 25 unique locations, every one in `builder.rs`. The blanket `must_use_candidate` lint reports 345 unique candidates across the compiled workspace graph and is not worth adopting.

## Scope
In scope: adding `#[must_use]` attributes in `crates/cyrup-agent/src/agent/builder.rs` and `crates/cyrup-agent/src/agent/facade.rs`; making the one existing discard in `crates/cyrup-session-svc/src/session.rs` explicit; adding one lint line to the root `Cargo.toml`'s `[workspace.lints.clippy]`.

Out of scope: any behaviour change, signature change, or builder restructuring; adding docs to these methods (that belongs to the queued **CARGO_DOC_WARNINGS** task — do not fix missing-docs or intra-doc links here); adopting `clippy::pedantic` or `must_use_candidate` workspace-wide; touching any other crate's API. `Subscription` stays exempt — `facade.rs:21` already documents why. `RunHandle` stays exempt — it is only ever produced inside `Result<RunHandle, AgentError>` (`lifecycle.rs:143/:160/:168/:226`), and `Result` is already `#[must_use]`, so fire-and-forget after `?` is legitimate and used in-tree.

## Approach
1. In `crates/cyrup-agent/src/agent/builder.rs`, add `#[must_use]` to `new` (:33), to `build` (:214), and to each of the 25 chainable `-> Self` methods listed above — 27 attributes total. Place the attribute below any existing doc comment, directly above `pub fn`.
2. In `crates/cyrup-agent/src/agent/facade.rs`, add `#[must_use]` to `Agent::builder` (:44) and to `Agent::drain_queues_for_restore` (:152).
3. In `crates/cyrup-session-svc/src/session.rs:1494`, change the bare call to `let _ = self.agent.drain_queues_for_restore();` and keep a one-line comment saying the agent's copies duplicate the mirror text taken just above. Explicit over `#[allow]`: the point of the attribute is that every discard is visible at the call site.
4. In the root `Cargo.toml`, add `return_self_not_must_use = "warn"` to `[workspace.lints.clippy]` (which currently holds only `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`). Targeted rather than blanket pedantic: it catches exactly this builder shape without the 345-candidate blast radius of `must_use_candidate`. Do not raise it to `deny` — other workspace crates are not audited by this task and the crates all inherit via `[lints] workspace = true`.
5. If step 4 surfaces `return_self_not_must_use` warnings in crates outside `cyrup-agent`, leave them; `warn` is deliberately non-blocking and fixing them is separate work.

## Acceptance Criteria
- [ ] `grep -c '#\[must_use\]' crates/cyrup-agent/src/agent/builder.rs` returns `27`.
- [ ] `grep -c '#\[must_use\]' crates/cyrup-agent/src/agent/facade.rs` returns `2`, and `grep -n 'Deliberately NOT' crates/cyrup-agent/src/agent/facade.rs` still returns line 21 (the `Subscription` exemption is untouched).
- [ ] `grep -rn 'must_use' crates/cyrup-agent/src/agent/lifecycle.rs` returns nothing (`RunHandle` untouched).
- [ ] `touch crates/cyrup-agent/src/lib.rs && cargo clippy -p cyrup-agent --all-targets -- -W clippy::pedantic 2>&1 | grep -c return_self_not_must_use` returns `0`.
- [ ] `grep -n 'return_self_not_must_use' Cargo.toml` shows the line inside `[workspace.lints.clippy]`.
- [ ] `grep -n 'drain_queues_for_restore' crates/cyrup-session-svc/src/session.rs` shows the call bound with `let _ =` at ~:1494.
- [ ] `cargo clippy -p cyrup-agent --all-targets` still emits exactly 3 diagnostics (unchanged baseline), and `cargo clippy -p cyrup-session-svc --all-targets` emits no `unused_must_use`.
- [ ] `cargo test -p cyrup-agent` is 140/140 green; `cargo build -p cyrup-session-svc` succeeds.

---

## QA Record — 2026-08-22 19:15

Verified complete. Measured against the final tree: `cargo build -p cyrup-agent` clean, `cargo test -p cyrup-agent` 140 passed / 0 failed, 0 clippy diagnostics attributed to cyrup-agent (baseline was 3), `cargo clippy -p cyrup-agent --all-targets --no-deps -- -D warnings` exit 0, rustdoc holding at its 6 pre-existing warnings, `cargo build --workspace` clean.

27 `#[must_use]` on `AgentBuilder` (25 chainable setters plus `new` and `build`), 2 on `Agent`
(`builder`, `drain_queues_for_restore`), `return_self_not_must_use = "warn"` added to
`[workspace.lints.clippy]` with the four `deny` entries untouched, and the deliberate discard at
`cyrup-session-svc/src/session.rs:1495` made explicit with `let _ =`. `RunHandle` and `Subscription`
correctly left exempt.

**Two criteria were mis-scoped greps, not unmet work:**

1. `grep -c '#[must_use]' facade.rs` returns 3, not 2 — the third hit is the doc comment
   *"Deliberately NOT `#[must_use]`"* at `facade.rs:21`, which the same criterion requires to still
   be present. There are exactly 2 real attributes.
2. `grep -c return_self_not_must_use` over pedantic clippy returns 15, not 0 — all 15 are in
   `cyrup-core` and `cyrup-provider`. Zero remain in cyrup-agent, and step 5 of this task
   explicitly says to leave out-of-crate hits alone.
