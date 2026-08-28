---
stage: done
status: completed
updated: 2026-08-28
---

# Implement `/resume`'s `re:<pattern>` Search Instead Of Returning `None` For Every Session

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** medium · **Kind:** partial-behaviour · **Area:** Selectors, settings and dialogs

## Objective

`/resume` advertises regex search in its own header hint, two rows above the search box, and then
returns zero sessions for every `re:` query. That is a silent wrong answer, not a missing feature:
the user sees an empty list and concludes they have no matching sessions. The fix is small and
self-contained — parsing already works, the scoring formula already exists one arm below, and the
`regex` crate is already a first-class dependency of three sibling crates.

## Upstream reference

- [`session-selector-search.ts:44-56`](../../tmp/pi/packages/coding-agent/src/modes/interactive/session-selector-search.ts)
  — `if (trimmed.startsWith("re:")) { … return { mode: "regex", tokens: [], regex: new RegExp(pattern, "i") }; }`
  (case-insensitive).
- `:113-121` — `matchSession` runs `text.search(parsed.regex)` over the assembled
  `${id} ${name} ${allMessagesText} ${cwd}` and scores `idx * 0.1` — the identical formula the
  phrase arm uses.
- [`session-selector.ts:170`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/session-selector.ts)
  — the header hint that advertises it:
  `keyHint("tui.input.tab", "scope") + sep + theme.fg("muted", 're:<pattern> regex · "phrase" exact')`.

## Current state in cyrup-tui

- [`session_search.rs:205-211`](../../crates/cyrup-tui/src/session_search.rs) — `match_text` opens
  with `if parsed.mode == QueryMode::Regex { return None; }` ("No approved regex engine —
  recognized but unsupported"). Every session scores `None`, so `filter_and_sort` yields an empty
  `Vec` for any `re:` query.
- Parsing is **complete**: [`session_search.rs:115-141`](../../crates/cyrup-tui/src/session_search.rs)
  `parse_search_query` produces `QueryMode::Regex` with `regex_pattern: Some(pattern)` (`:138-141`).
  The field is declared at `:77`.
- The scoring formula the regex arm needs already exists one arm below — the phrase arm at
  `:220-229`: `total += norm.find(&phrase)? as f64 * 0.1`.
- Nothing outside `session_search.rs` references `QueryMode::Regex`, so there is no compensating
  surface elsewhere.
- The hint is still rendered: [`session_selector.rs:802`](../../crates/cyrup-tui/src/session_selector.rs)
  emits the literal `re:<pattern> regex · "phrase" exact`, pinned by
  `crates/cyrup-tui/tests/dialog_envelope_spacers.rs:743`.
- The stub is pinned by `session_search.rs:351-358`
  (`regex_mode_is_recognized_but_unsupported`).
- **The stated rationale is false.** The module doc at `session_search.rs:13-15` says "cyrup has no
  approved regex dependency"; the workspace already carries one in three places —
  `crates/cyrup-ext/Cargo.toml:44` and `crates/cyrup-mcp/Cargo.toml:120` (`regex = "1.12.4"`),
  `crates/cyrup-permission-system/Cargo.toml:62` (`regex = "1"`).
- The same doc also **overstates** today's behaviour: it claims the prefix is "surfaced as a
  one-line unsupported error", but `parse_search_query` sets `error` only for an **empty** pattern
  (`:129-136`). A real `re:foo` query produces a silent empty list, not a message.

## Subtasks

1. **`crates/cyrup-tui/Cargo.toml`** — add `regex`, matching the version the workspace already uses
   (`regex = "1.12.4"` per `crates/cyrup-ext/Cargo.toml:44`); prefer a workspace dependency entry if
   the manifest layout supports it.
2. **`crates/cyrup-tui/src/session_search.rs`** — compile `ParsedSearchQuery::regex_pattern` (`:77`)
   with `RegexBuilder::new(pattern).case_insensitive(true)` (pi's `"i"` flag,
   `session-selector-search.ts:52`). Decide once whether the compiled `Regex` is cached on
   `ParsedSearchQuery` or recompiled per call, and document the choice — `match_text` runs once per
   session.
3. **`crates/cyrup-tui/src/session_search.rs:205-211`** — replace the `return None` with
   `find(text).map(|m| m.start() as f64 * 0.1)` over the assembled
   `{id} {name} {allMessagesText} {cwd}` text, mirroring the phrase arm at `:220-229`.
4. **`crates/cyrup-tui/src/session_search.rs:115-141`** — route a **compile failure** into
   `ParsedSearchQuery::error` (the field `match_text` already short-circuits on at `:206-208`), so a
   malformed pattern reports instead of silently matching nothing. Keep the existing empty-pattern
   `"Empty regex"` error.
5. **`crates/cyrup-tui/src/session_search.rs:13-15`** — rewrite the module doc: drop the
   "no approved regex dependency" claim and the incorrect "surfaced as a one-line unsupported error"
   sentence, and state that every branch is now 1:1 with `session-selector-search.ts`.
6. **`crates/cyrup-tui/src/session_search.rs:351-358`** — the `regex_mode_is_recognized_but_unsupported`
   test pins the stub. Retire or retarget it as part of removing the stub (the test suite is another
   team's, but a test asserting the removed behaviour cannot be left asserting it).

## Acceptance criteria

- [ ] `grep -n "^regex" crates/cyrup-tui/Cargo.toml` returns a dependency line.
- [ ] `crates/cyrup-tui/src/session_search.rs` contains no `if parsed.mode == QueryMode::Regex {
      return None; }`.
- [ ] `match_text("foozzzbar", &parse_search_query("re:foo.*bar"))` returns `Some(0.0)`.
- [ ] Case-insensitivity holds: `match_text("FOOZZZBAR", &parse_search_query("re:foo.*bar"))` is
      `Some(_)`.
- [ ] The score is `match start * 0.1` — a pattern matching at offset 10 scores `1.0`, the same rule
      as the phrase arm at `session_search.rs:220-229`.
- [ ] `parse_search_query("re:[")` yields `error: Some(_)` (a compile failure, reported), not a query
      that silently matches nothing.
- [ ] The module doc at `session_search.rs:13-15` no longer claims cyrup has no approved regex
      dependency.
- [ ] `grep -n "regex_mode_is_recognized_but_unsupported" crates/cyrup-tui/src/session_search.rs`
      returns nothing, or returns a test that asserts the new behaviour.
- [ ] The header hint at `session_selector.rs:802` is unchanged — it was always correct.
- [ ] `cargo build -p cyrup-tui` → 0 warnings; `cargo clippy -p cyrup-tui --all-targets` → no new
      diagnostics.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
