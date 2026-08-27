---
stage: todo
status: pending
updated: 2026-08-27
---

# Wire The Ported Cache-Miss Detector Into Transcript Notices And Add The Compaction Cost Line

> Identified by the `cyrup-tui` <-> pi port audit (fan-out + adversarial verification).
> **Priority:** medium · **Effort:** medium · Area: Interactive mode shell, footer, status and execution views

## Objective

pi warns in-transcript when a turn silently re-billed a large prompt prefix — after a model switch,
or after an idle gap longer than the prompt-cache TTL — and prints what each compaction and branch
summary cost. In cyrup those costs only ever surface later inside the footer's cumulative `$`
figure, with no attribution, and a user who wants the notices cannot even find the toggle: there is
no `/settings` row for it.

## Scope — consumer side only

**Do NOT re-implement detection, and do NOT re-implement the settings getter.** Both are already
ported, tested, and have zero callers. This task is entirely the TUI consumer half.

- Detection: [`cyrup-provider/src/cache_stats.rs:284-303`](../../crates/cyrup-provider/src/cache_stats.rs)
  — `collect_cache_misses`, `detect_cache_miss`, with `CacheMiss { missed_tokens, missed_cost,
  idle_ms, model_changed }` at `:53-67` and `CACHE_TTL_MS` at `:47`. Re-exported at
  [`cyrup-provider/src/lib.rs:61-63`](../../crates/cyrup-provider/src/lib.rs). A workspace-wide grep
  for both names outside that file returns only the re-export line.
- Setting: [`cyrup-config/src/settings/effective.rs:64-77`](../../crates/cyrup-config/src/settings/effective.rs)
  `show_cache_miss_notices()`, whose only readers are its own tests in `settings/tests/getters.rs:53-66`.

[`cache_stats.rs:37-38`](../../crates/cyrup-provider/src/cache_stats.rs) names this task's owners in
its own module doc: *"Those are `crates/cyrup-tui` and a new `showCacheMissNotices` setting."*

## Upstream reference

In [`interactive-mode.ts`](../../tmp/pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts):

- `:3781-3796` `renderSessionEntries` — after every `compaction` / `branch_summary` entry that
  carries `usage` **and** produced messages, it synthesises
  `{ type: "compaction_cost", kind: entry.type, usage: entry.usage }`; `renderSessionItems`
  (`:3705-3709`) dispatches it via `isCompactionCostNotice` (`:223-225`).
- `:3802-3813` `addCompactionCostNotice` — gated on `getShowCacheMissNotices()`; `tokens = input +
  output + cacheRead + cacheWrite`; `cost = usage.cost.total >= 0.01 ? " (~$x.xx)" : ""`; label is
  `Compaction` or `Branch summary`; emits `Spacer(1)` then warning-coloured
  `` `${label}: ${formatTokens(tokens)} tokens billed${cost}` ``.
- `:3819-3826` `maybeShowCacheMissNotice(message)` — gated on the same setting, then
  `detectCacheMiss(this.sessionManager.getEntries(), message, this.session.modelRuntime)`. The
  comment matters: *"Entries don't contain `message` yet: message_end fires before persistence."*
- `:3828-3843` `addCacheMissNotice` — **suppression floor** `if (miss.missedTokens < 20_000 &&
  miss.missedCost < 0.1) return;` (i.e. shown when *either* threshold is met); `cost` suffix at
  `>= 0.01`; label is `Cache miss`, or `Cache miss after model switch` when `miss.modelChanged`,
  or `` `Cache miss after ${Math.round(miss.idleMs / 60_000)}m idle` `` when
  **`miss.idleMs >= CACHE_TTL_MS`** — note `>=`, not `>`; emits `Spacer(1)` then warning-coloured
  `` `${label}: ${formatTokens(miss.missedTokens)} tokens re-billed${cost}` ``.
- `:3694-3697` — the whole set is re-derived with `collectCacheMisses(...)` on replay/rebuild, so a
  resumed session keeps its notices.
- `settings-selector.ts:509` — the `/settings` row `label: "Cache miss notices"`, wired at
  `interactive-mode.ts:4560` and `:4655-4657`.

## Current state in cyrup-tui

- **No cache-miss notice anywhere.** Nothing calls `detect_cache_miss` or `collect_cache_misses`.
- **No compaction-cost notice.** The `CompactionEnd` arm at
  [`app/events_fold.rs:286-296`](../../crates/cyrup-tui/src/app/events_fold.rs) branches only
  aborted / `error_message` / `push_compaction_summary(res.tokens_before, res.summary)` (`:290-292`)
  and never reads the entry's `usage`. Nothing in `cyrup-tui` formats the string "tokens billed".
- **The `Re-billed` string that does exist is a different pi feature**: the `/session` stats table
  row at [`app/execute_session.rs:204-208`](../../crates/cyrup-tui/src/app/execute_session.rs), fed
  by `compute_cache_waste` from
  [`cyrup-session-svc/src/session/stats.rs:50`](../../crates/cyrup-session-svc/src/session/stats.rs)
  — pi's separate `computeCacheWaste`. Leave it alone.
- **No `/settings` row**: read the complete list at
  [`app/settings_rows.rs:47-198`](../../crates/cyrup-tui/src/app/settings_rows.rs).
- **The primitives are all present.**
  [`transcript/notices.rs:34`](../../crates/cyrup-tui/src/transcript/notices.rs) `push_warning` is
  exactly the warning-styled row both notices need (and `push_compaction_summary` at `:131` is the
  neighbouring shape to copy), and [`status.rs:420`](../../crates/cyrup-tui/src/status.rs)
  `format_tokens` is pi's `formatTokens`, half-up rounding included.

## Subtasks

1. **`TranscriptView::push_cache_miss_notice`** in
   [`transcript/notices.rs`](../../crates/cyrup-tui/src/transcript/notices.rs): take a `CacheMiss`,
   apply pi's suppression floor (`missed_tokens < 20_000 && missed_cost < 0.1` -> return), build the
   three labels, and emit the leading blank + warning row. Use `>= CACHE_TTL_MS` for the idle branch
   — do **not** use `CacheMiss::exceeded_ttl()`, which is `>` (`cache_stats.rs:70-72`) and belongs to
   a different call site.
2. **`TranscriptView::push_compaction_cost_notice`** in the same file: `Compaction` /
   `Branch summary` label, `format_tokens(input + output + cache_read + cache_write)`, the
   `>= $0.01` cost suffix, leading blank + warning row.
3. **Call the cache-miss path** from the finished-assistant-message handler with
   `cyrup_provider::detect_cache_miss`, passing the entries **without** the just-finished message
   (pi's `message_end`-before-persistence ordering, `interactive-mode.ts:3822`).
4. **Call the compaction-cost path** from the `CompactionEnd` arm at
   [`app/events_fold.rs:286-296`](../../crates/cyrup-tui/src/app/events_fold.rs) and from the
   branch-summary path, reading the entry's `usage` — which the arm does not touch today.
5. **Re-derive on replay/rebuild** with `cyrup_provider::collect_cache_misses`, keyed by the assistant
   entry index it returns, so a resumed or post-compaction-rebuilt transcript still carries the
   notices (`interactive-mode.ts:3694-3697`).
6. **Gate every emission** on
   [`EffectiveSettings::show_cache_miss_notices()`](../../crates/cyrup-config/src/settings/effective.rs).
7. **Add the `/settings` row** `SettingRow::toggle("showCacheMissNotices", "Cache miss notices", …)`
   in [`app/settings_rows.rs`](../../crates/cyrup-tui/src/app/settings_rows.rs) at pi's position
   (`settings-selector.ts:509`), persisting through the existing `AppCommand::ApplySetting` path.

## Acceptance criteria

- [ ] `grep -rn "detect_cache_miss\|collect_cache_misses" crates/cyrup-tui/src` returns production
      call sites (today the workspace has none outside `cyrup-provider`'s own re-export)
- [ ] `grep -rn "show_cache_miss_notices" crates/cyrup-tui/src` returns a gate on both notice paths
- [ ] A miss with `missed_tokens = 19_999` and `missed_cost = 0.05` emits nothing; either
      `missed_tokens >= 20_000` **or** `missed_cost >= 0.1` emits the notice
- [ ] The three labels render exactly as `Cache miss`, `Cache miss after model switch` and
      `Cache miss after {N}m idle`, with `N = round(idle_ms / 60_000)` and the branch taken at
      `idle_ms >= CACHE_TTL_MS`
- [ ] The cost suffix ` (~$x.xx)` appears only at `>= 0.01`, two decimal places, on both notice types
- [ ] `grep -rn "tokens billed" crates/cyrup-tui/src` returns the compaction-cost formatter, emitting
      `Compaction:` for a compaction and `Branch summary:` for a branch summary
- [ ] Both notices are preceded by one blank line and rendered in the warning colour, via the same
      `push_warning`-shaped path
- [ ] A resumed session re-renders its cache-miss notices (the `collect_cache_misses` replay path is
      reached, not only the live `detect_cache_miss` path)
- [ ] `grep -n 'Cache miss notices' crates/cyrup-tui/src/app/settings_rows.rs` returns one toggle row
      writing `showCacheMissNotices`
- [ ] The `/session` stats `Cache Re-billed` row at `app/execute_session.rs:204-208` is unchanged
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
