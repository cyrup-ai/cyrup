---
stage: new
status: done
updated: 2026-08-22 18:30
severity: medium
effort: small
category: public-api
---

# Manage The Crate's Public Surface: Demote `StateInner`/`reduce` And Settle The Re-Export Policy

## Description
[`src/lib.rs`](../../crates/cyrup-agent/src/lib.rs) is 46 lines: ten `pub mod` declarations (`lib.rs:11-20`) followed by a curated flat re-export list (`lib.rs:22-39`). Because the modules are `pub`, the module path is a *second, unmanaged* public API — anything `pub` inside a module escapes the crate whether or not lib.rs blessed it. Two symptoms of that missing policy, both verified:

**(1) `StateInner` and `reduce` leak.** [`src/state.rs`](../../crates/cyrup-agent/src/state.rs)`:88` declares `pub struct StateInner` (12 `pub` fields: system prompt, model, transcript, pending tool-call set, live header overlay, transport), `:117` `pub fn snapshot`, `:158` `pub fn reduce(st: &mut StateInner, ev: &AgentEvent)`. All three are reachable as `cyrup_agent::state::*` via `pub mod state;` while `lib.rs:37` re-exports only `AgentStateSnapshot, GenerationConfig`. No out-of-crate consumer exists: `grep -rnE "StateInner|state::reduce|cyrup_agent::state" --include=*.rs crates/` outside cyrup-agent returns exactly 4 hits, every one prose inside a comment — `cyrup-ext-subagents/src/background/tracker.rs:35`, `cyrup-session-svc/src/builder.rs:1499`, `.../attribution.rs:15`, `.../session.rs:645`. In-crate construction is `loop_fn.rs:121` and [`agent/builder.rs`](../../crates/cyrup-agent/src/agent/builder.rs)`:216`; `reduce` is called at `agent/lifecycle.rs:42` and `agent/run/mod.rs:172` (plus `src/tests/area02_backlog.rs`, unaffected by `pub(crate)`). Publishing the agent's live mutable interior alongside a mutator advertises a state-machine seam the crate neither supports nor tests as public, and makes any future field change semver-breaking for a type nothing outside uses.

**(2) The third-party re-export block contradicts its own comment.** `lib.rs:41-43` claims to re-export "the load-bearing provider/core types the agent's public API exposes, so downstream crates can drive the agent without depending on cyrup-provider directly", then re-exports `Context, StreamEvent, StreamOptions, ToolDef`. Three ways it fails:
- `ToolDef` appears in **no** public signature. `grep -rn ToolDef crates/cyrup-agent/src` outside `src/tests` returns only `lib.rs:43` plus two private construction sites (`agent/run/stream.rs:72`, `:75`).
- Eight `cyrup_provider` types **do** appear in public signatures and are not re-exported: `CacheRetention` (builder.rs:121), `HeaderMap` (builder.rs:127, [`agent/facade.rs`](../../crates/cyrup-agent/src/agent/facade.rs)`:79`, [`agent/mod.rs`](../../crates/cyrup-agent/src/agent/mod.rs)`:101` `pub type HeaderFn`, state.rs:103/:147), `Transport` (builder.rs:133, facade.rs:96), `ThinkingBudgets` (builder.rs:153), `ProviderEnv` (builder.rs:168), `OnPayload` (builder.rs:202), `OnResponseHook` (builder.rs:209), `Provider` ([`stream_fn.rs`](../../crates/cyrup-agent/src/stream_fn.rs)`:33`).
- The comment says "provider/core" but `grep -n cyrup_core src/lib.rs` returns nothing, while `ModelRef`, `CancelToken`, `EventStream`, `ProviderId`, `AssistantMessage`, `StopReason`, `Content`, `ToolCallId`, `SessionId` are pervasive in public signatures (`agent/facade.rs:44/:72`, `subscriber.rs:25`, `stream_fn.rs:12/:24/:39`, `event.rs:6`).

A half-facade is worse than none: a downstream crate still cannot call `AgentBuilder::transport/headers/thinking_budgets/provider_env/on_payload/on_response`, `Agent::set_headers/set_transport`, or build a `ProviderStreamFn` without depending on cyrup-provider directly — while `ToolDef` occupies a slot as pure noise, giving a false signal that the list is curated.

## Scope
In scope: visibility of `StateInner`, `StateInner::snapshot`, `reduce` in `state.rs`; the module-visibility policy in `lib.rs:11-20`; the third-party re-export block at `lib.rs:41-43`; the one intra-doc link that a `pub(crate)` demotion breaks (`state.rs:27-28`).

Out of scope: any behavior change, field additions/removals on `StateInner`, renaming public items, and touching other modules' internals. **Must not overlap `CARGO_DOC_WARNINGS`** — that task owns the crate's existing 6 rustdoc warnings workspace-wide; this task's only rustdoc obligation is to not *add* one. Do not fix unrelated doc warnings here.

## Approach
1. In `state.rs`, change `pub struct StateInner` (:88) to `pub(crate) struct StateInner`, `impl StateInner { pub fn snapshot` (:117) to `pub(crate) fn snapshot`, and `pub fn reduce` (:158) to `pub(crate) fn reduce`. Leave the 12 fields `pub` — they are inert once the type is crate-private. Keep `AgentStateSnapshot` and `GenerationConfig` public; they are the blessed surface.
2. Fix the fallout link: `state.rs:27-28` inside the **public** `GenerationConfig::transport` doc writes ``[`StateInner::transport`]``. Replace it with the plain code span `` `StateInner::transport` `` so rustdoc does not gain a "public documentation links to private item" warning.
3. Keep `pub mod` at `lib.rs:11-20` rather than demoting to `pub(crate) mod` — `proxy`, `hooks`, `queue` and `event` are legitimately browsed by path and demotion would be a breaking change across cyrup-session-svc. Instead, add a short comment above the block stating the rule: anything `pub` inside these modules is public API and must appear in the `pub use` list below, which is the audit checklist.
4. Complete the facade rather than delete it — eight builder/facade methods already force a direct cyrup-provider dependency, so completing is the smaller behavior-preserving edit. Drop `ToolDef` from `lib.rs:43`; add `CacheRetention, HeaderMap, OnPayload, OnResponseHook, Provider, ProviderEnv, ThinkingBudgets, Transport` to that `pub use cyrup_provider::{…}`; add `pub use cyrup_core::{AssistantMessage, CancelToken, Content, EventStream, ModelRef, ModelThinkingLevel, ProviderId, SessionId, StopReason, Tool, ToolCallId};`. Rewrite the `lib.rs:41-42` comment to state the actual rule: every third-party type appearing in a public signature of this crate is re-exported here.
5. Resolve any name collisions the new re-exports introduce against `lib.rs:22-39` at compile time; if one appears, keep the cyrup-agent name and drop the third-party one, noting it in the comment.

## Acceptance Criteria
- [ ] `grep -n "pub struct StateInner\|pub fn reduce\|pub fn snapshot" crates/cyrup-agent/src/state.rs` returns no matches; the three items read `pub(crate)`.
- [ ] `grep -n "StateInner::transport" crates/cyrup-agent/src/state.rs` shows a plain code span, not a `[...]` intra-doc link.
- [ ] `cargo check --workspace` is clean (no crate outside cyrup-agent references `StateInner`/`reduce` in code).
- [ ] `cargo test -p cyrup-agent` is 140/140 green.
- [ ] `cargo clippy -p cyrup-agent --all-targets` emits no more than the 3 diagnostics it emits today.
- [ ] `cargo doc -p cyrup-agent --no-deps 2>&1 | grep -c "^warning"` reports 7 lines (6 warnings + summary), unchanged from baseline — no new rustdoc warning is introduced.
- [ ] `grep -n ToolDef crates/cyrup-agent/src/lib.rs` returns nothing.
- [ ] Every `cyrup_provider`/`cyrup_core` type named in the Description appears in a `pub use` in `lib.rs`; spot-check by confirming `grep -n "CacheRetention\|ThinkingBudgets\|ProviderEnv\|OnPayload\|OnResponseHook\|Transport\|HeaderMap\|Provider\b" crates/cyrup-agent/src/lib.rs` matches all eight.
- [ ] `lib.rs` carries a comment above `lib.rs:11-20` stating that `pub` items inside these modules are public API and must be listed in the root re-exports.
