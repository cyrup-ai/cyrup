---
stage: todo
status: pending
updated: 2026-08-28
---

# Finish The Extension Half Of Slash-Argument Completions

> Follow-up to `TUI_SLASH_ARGUMENT_COMPLETIONS` (shipped 2026-08-28), which landed the engine, the
> `SlashArgument` context and the `/model` and `/login` builtins.
> **Priority:** medium · **Effort:** small-medium · Area: Editor, input, keys and autocomplete

## What shipped, so this is not re-done

`CompletionContext::SlashArgument`, the first-space split in `slash_context`, the completer seam on
`SlashCommand` replacing the unwired `has_arg_completion: bool`, the forced-(`Tab`)-path fix, and
the `/model` and `/login` completers. Read the done task before starting.

## What did NOT land, and why

### 1. `/thinking <arg>` — RESOLVED during the rebase, 2026-08-28

At the time this was filed, `ArgumentCompleter::ThinkingLevels` was built and fed but unreachable:
cyrup had no `/thinking` builtin. Main's `82f40d3` ("persist model and thinking level via pi's
Ctrl+S set-as-default") added one, and rebasing onto it surfaced the seam as a compile error —
`arg_cmd` now takes a completer, and main's new row supplied none. It is wired to
`ArgumentCompleter::ThinkingLevels` at `commands.rs:131`, and the variant's "inert" doc comment was
corrected. Nothing left to do here; kept for the record.

### 2. Extension / prompt / skill commands still hardcode `ArgumentCompleter::None`

Two independent blockers, both real:

- **No catalog key.** `commands.rs:483-487` documents it from the other end: `slash_command_catalog()`
  emits no key saying whether a registered command declared `getArgumentCompletions`. This cannot be
  fixed inside `cyrup-tui`.
- **No sync call path.** Guest completers are async; `Autocomplete::compute` is sync, and the shipped
  task deliberately kept it that way. pi awaits `command.getArgumentCompletions(argumentText)`
  (`autocomplete.ts:355`). Bridging this needs a decision: pre-fetch and cache the guest's
  completions at registration, or make the popup path async. Pick one and write down why.

## Also worth a review pass

Two behaviour changes shipped with the parent task that a reviewer should look at deliberately:

- With a `/model ` popup open, `Enter` now accepts the highlighted model instead of submitting the
  bare `/model` line.
- `Tab`-accepting `/model` immediately reopens as an argument popup, where pi shows nothing until
  the next keystroke.

Both are defensible; neither was explicitly specified. Confirm or correct them.

## Acceptance criteria

- [x] `/thinking ` completes reasoning levels — done at `commands.rs:131` during the 2026-08-28 rebase
- [ ] Extension commands declaring `getArgumentCompletions` either complete, or the blocker is
      resolved with a recorded decision rather than a hardcoded `None`
- [ ] `cargo clippy -p cyrup-tui --all-targets` — denied-lint count not increased
- [ ] `cargo test --workspace` — no regression

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
