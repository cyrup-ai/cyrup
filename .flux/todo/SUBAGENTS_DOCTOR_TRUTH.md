---
stage: new
status: done
updated: 2026-08-23 00:06
---

# Stop cyrup-ext-subagents shipping false capability claims: /subagents-doctor reports 0 skills for code it already calls, and 94 'later phase' notes name modules that now exist

> Found by an eight-lens workspace hygiene sweep. Every count below was reproduced
> against the tree before this task was filed.
> **Priority:** high · **Effort:** medium
> **Crates:** `cyrup-ext-subagents`

Two related problems, both verified in-tree, both making the crate lie about itself — one to operators, one to the next implementer.

**1. `/subagents-doctor` under-reports its own installation.** `registration/doctor.rs:993` emits the user-facing line `"- skills: total 0 (full skill discovery not implemented in this build — Tier 5)"` (verified), justified at `doctor.rs:955-964` with "no separate discoverAvailableSkills call exists". But `discovery/skills.rs:239` defines `pub async fn discover_available_skills` in a **1,198-line** module in the same crate (verified), and `extension/tool/routing.rs:1076` **already calls it in production** (verified). Separately, `doctor.rs:30-33` and `:693-695` state the live model-probe checks and the `/subagents-refresh-provider-models`, `/subagents-generate-profiles`, `/subagents-check-profile` algorithms are "NOT implemented here or anywhere else in this crate as of this phase" — while `extension/host/profiles.rs:19-21` says those exact three commands are implemented "with REAL live-probe subprocess classification" and exposes `refresh_provider_catalog_cache` / `generate_provider_profiles`, backed by `extension/models/probe.rs`. The doctor therefore ships a false capability statement to operators.

**2. 94 stale "later phase" deferral notes across 30 files** (verified: `grep -ric 'later[- ]phase'` sums to 94). Every module path they name exists today, several at multi-thousand-line size: `exec/mod.rs` (×6), `background/tracker.rs` (×3), `control.rs` (×2), `background/control.rs`, `background/runner_main.rs`, `spawn_detached.rs`, `runner_main.rs`, `registration/slash_commands.rs`, `tui/render.rs`, `spawn/worktree.rs`, `exec/fallback.rs`. Three are now self-refuting inside a single file: `registration/mod.rs:15` says "this file does not declare a `pub mod` item for it" while `registration/mod.rs:81` is literally `pub mod slash_commands;` (**2,720 lines**); `exec/fallback.rs:22-23` says `exec/mod.rs` is "currently only `pub mod ndjson;`" when `exec/mod.rs` declares **19** `pub mod`s over **7,926 lines** (verified); `background/control.rs:36` and `spawn/chain_graph.rs:45` both say modules "do not exist yet". Several notes also justify plain module-path prose instead of intra-doc links on that false premise (`spawn_detached.rs:11`), so the stale text is additionally suppressing working rustdoc links.

A related smaller contradiction sits in cyrup-intercom and can ride along or be handled separately: `ui/mod.rs:16-21` says the `intercom_message` renderer is "blocked outside this crate on TWO counts (ICOM-024/ICOM-029)" while `ui/inline_message.rs:15-19` says "the reason for that degradation is gone". The mod.rs blocker still holds — `cyrup-session-svc/src/host_services.rs:1312-1318` shows `inject_message(content, custom_type, display, trigger_turn)` with no `details` channel — and `ui/mod.rs`'s pointer to `cyrup-ext/src/native.rs:270` is off by 60 lines (`register_message_renderer` is at `native.rs:330`).

## Acceptance Criteria

- [ ] `/subagents-doctor` reports real skill counts by calling `crate::discovery::skills::discover_available_skills`; the string "full skill discovery not implemented in this build" no longer appears anywhere in registration/doctor.rs
- [ ] The doctor's model-probe/profile claims at doctor.rs:30-33 and :693-695 are either wired to `extension/host/profiles.rs` (`refresh_provider_catalog_cache` / `generate_provider_profiles`) or corrected to describe what actually ships — no remaining text asserting those commands are unimplemented "anywhere else in this crate"
- [ ] `grep -ric 'later[- ]phase' crates/cyrup-ext-subagents/src` returns a total that is 0, or is a small documented remainder where every surviving note names a module that genuinely does not exist (checked by resolving each named path)
- [ ] The three self-refuting notes are fixed: registration/mod.rs:15 (vs the `pub mod slash_commands;` at :81), exec/fallback.rs:22-23 (`exec/mod.rs` has 19 pub mods / 7,926 lines), and background/control.rs:36 + spawn/chain_graph.rs:45
- [ ] Deferral prose that avoided intra-doc links on the false premise (e.g. spawn_detached.rs:11) is converted to real rustdoc links, and `cargo doc -p cyrup-ext-subagents` introduces no new broken-link warnings
- [ ] Either the cyrup-intercom ui/mod.rs vs ui/inline_message.rs contradiction is resolved to a single accurate statement (with the native.rs:270 → :330 pointer corrected), or the task explicitly records it as out of scope

## Verifying command

```bash
cd /home/user/cyrup/crates/cyrup-ext-subagents/src && grep -n 'not implemented in this build' registration/doctor.rs && grep -n 'pub async fn discover_available_skills' discovery/skills.rs && grep -n 'discover_available_skills' extension/tool/routing.rs && grep -ric 'later[- ]phase' . | awk -F: '{s+=$2} END{print s}' && sed -n '15p;81p' registration/mod.rs && wc -l registration/slash_commands.rs exec/mod.rs
```
