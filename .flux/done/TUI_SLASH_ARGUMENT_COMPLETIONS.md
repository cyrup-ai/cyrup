---
stage: done
status: completed
updated: 2026-08-28
---

# Port The `spaceIndex !== -1` Half Of `getSuggestions` So `/model <arg>` And `/login <arg>` Complete

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** high · **Kind:** missing-feature · **Area:** Editor, input, keys and autocomplete

## Objective

Typing `/model g`, `/login an` or `/thinking hi` should pop a ranked list of the things that can
legally follow that command — the available `provider/id` models, the known provider ids, the
reasoning levels — the way it does upstream, with no `Tab` required. Today those keystrokes close
the popup, and pressing `Tab` there offers a directory listing of the working directory instead,
which is a wrong answer rather than a missing one. Extension commands that register
`getArgumentCompletions` are equally invisible.

## Upstream reference

- [`packages/tui/src/autocomplete.ts:308-364`](../../tmp/pi/packages/tui/src/autocomplete.ts) —
  `CombinedAutocompleteProvider.getSuggestions` splits the `/`-line on the first space
  (`:314 const spaceIndex = textBeforeCursor.indexOf(" ")`). The `spaceIndex === -1` branch
  (`:316-340`) is the command-name list. The `spaceIndex !== -1` branch takes
  `commandName = textBeforeCursor.slice(1, spaceIndex)` (`:344`),
  `argumentText = textBeforeCursor.slice(spaceIndex + 1)` (`:345`), looks the command up
  (`:347-350`), `await command.getArgumentCompletions(argumentText)` (`:355`), and returns
  `{ items: argumentSuggestions, prefix: argumentText }` (`:358-363`) — note the prefix is the
  **argument text only**, not the whole line.
- [`packages/coding-agent/src/modes/interactive/interactive-mode.ts:685-753`](../../tmp/pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts)
  installs the three builtin completers — `/model` (`:687`, fuzzy over the scoped models /
  `modelRuntime.getAvailableSnapshot()`, values shaped `provider/id`), `/thinking` (`:713`, the
  available reasoning levels) and `/login` (`:728`, provider ids) — and `:753` forwards every
  extension command's own `getArgumentCompletions`.
- [`packages/tui/src/components/editor.ts:1132-1143`](../../tmp/pi/packages/tui/src/components/editor.ts)
  re-triggers autocomplete on each typed `[a-zA-Z0-9.\-_]` character while
  `isInSlashCommandContext(textBeforeCursor)` (`:2103`) holds — and that predicate is only
  `trimStart().startsWith("/")`; it does **not** require the absence of a space. So `/model g`
  auto-pops with no `Tab`.

## Current state in cyrup-tui

| piece | where | what it does / does not do |
|---|---|---|
| slash context | [`autocomplete.rs:140-165`](../../crates/cyrup-tui/src/autocomplete.rs) | `slash_context` opens with `if !before.starts_with('/') \|\| before.contains(char::is_whitespace) { return None; }` — a faithful port of the `spaceIndex === -1` branch **only**. The module doc says as much at `:6-7` ("the line begins with `/` and has no space yet"). |
| engine entry | [`autocomplete.rs:76-94`](../../crates/cyrup-tui/src/autocomplete.rs) | `Autocomplete::compute` is a pure **sync** fn over `&CommandRegistry`. It tries `slash_context` only when `!force`, then falls through to `path_context` (`:93`). With `force = true` (Tab) it skips the slash arm entirely and completes the trailing token against the CWD (`path_context`, `:184-219`). |
| apply | [`autocomplete.rs:98-136`](../../crates/cyrup-tui/src/autocomplete.rs) | `CompletionContext` (`:28-36`) has three variants — `Slash`, `Path`, `Mention` — and `apply` has a match arm per variant. There is no argument arm. |
| the declared seam | [`commands.rs:60`](../../crates/cyrup-tui/src/commands.rs) | `SlashCommand::has_arg_completion: bool`. Written at `commands.rs:138` (`cmd`, always `false`), `:152` (`arg_cmd`, always `true`) and `:487` (extension rows, hardcoded `false`). **Read only by tests** (`src/tests/commands.rs:17,21,346-347`). Neither `editor/completion.rs` (`update_autocomplete` `:71-92`, `trigger_completion` `:118-140` — the only two `compute` call sites) nor anything under `src/app/` reads it. It carries a `bool`, never a completer. |
| cross-crate blocker | [`commands.rs:483-487`](../../crates/cyrup-tui/src/commands.rs) | documents the extension half from the other end: "EXT-013 / TUI-012: still hardcoded, and it CANNOT be resolved in this crate — `slash_command_catalog()` emits no key saying whether a registered command declared `getArgumentCompletions`". |

Net: the `spaceIndex !== -1` branch was never ported and there is no seam a completer could hang
off. `has_arg_completion` is a declared-but-unwired flag, not an implementation.

## Subtasks

1. **`crates/cyrup-tui/src/autocomplete.rs`** — add a `CompletionContext::SlashArgument` variant to
   the enum at `:28-36`, and an `apply` arm at `:109-127` that replaces **only** the argument span
   (upstream's `prefix` is `argumentText`, `autocomplete.ts:362`) — no `/` re-prepended, trailing
   space per upstream's behaviour for a complete token.
2. **`crates/cyrup-tui/src/autocomplete.rs`** — rewrite `slash_context` (`:140-165`) to split
   `before` on the first space instead of rejecting it: no space → today's command-name list
   (unchanged); space present → resolve `commandName` in the registry and, when it has a completer,
   build the argument popup with `prefix = argumentText`.
3. **`crates/cyrup-tui/src/commands.rs`** — replace `has_arg_completion: bool` (`:60`) with a real
   completer seam on `SlashCommand` (e.g. an enum naming the builtin source, or a boxed sync
   closure — the engine is sync; do not make `compute` async for this). Keep `cmd`/`arg_cmd`
   (`:130-155`) compiling and update the two builtins that need it.
4. **`crates/cyrup-tui/src/autocomplete.rs` + the app layer that owns the model snapshot** — feed
   the `/model` completer from the available-models snapshot (values shaped `provider/id`, fuzzy
   ranked via `crate::fuzzy`), the `/login` completer from the provider ids, and the `/thinking`
   completer from the reasoning-level set. The snapshot has to reach `Autocomplete::compute`; thread
   it in beside `&CommandRegistry` rather than reaching for a global.
5. **`crates/cyrup-tui/src/autocomplete.rs`** — `Autocomplete::compute` (`:76-94`) must try the slash
   arm on the **forced** (`Tab`) path too when `before` starts with `/`, so `/model <Tab>` does not
   fall through to `path_context` and list the CWD.
6. **`crates/cyrup-tui/src/editor/completion.rs`** — `update_autocomplete` (`:71-92`) keeps the popup
   open for `CompletionContext::SlashArgument` on every typed character, matching
   `isInSlashCommandContext` (`editor.ts:2103`), which does not require the absence of a space.
7. **`crates/cyrup-session-svc` `slash_command_catalog()` + `crates/cyrup-tui/src/commands.rs:483-487`**
   — add the one catalog key saying whether a registered extension command declared
   `getArgumentCompletions`, and consume it in place of the hardcoded `false`. This is the
   cross-crate half; it can land after 1-6.

## Acceptance criteria

- [ ] `crates/cyrup-tui/src/autocomplete.rs` `slash_context` no longer contains
      `before.contains(char::is_whitespace)` as an early `return None`.
- [ ] `CompletionContext` has a fourth variant for slash arguments, and `Autocomplete::apply` has a
      match arm for it that replaces only the argument text (no leading `/` in the inserted value).
- [ ] `grep -rn "has_arg_completion" crates/cyrup-tui/src` shows at least one **non-test**,
      non-declaration reader on the completion path.
- [ ] With a `/model ` line and `force = false`, `Autocomplete::compute` returns a popup whose
      `context` is the slash-argument variant and whose items are `provider/id` strings — not a
      directory listing.
- [ ] With a `/model g` line and `force = true` (Tab), `compute` does **not** reach `path_context`:
      no CWD entry appears among the completions.
- [ ] `/login ` completes provider ids and `/thinking ` completes the reasoning levels, each from the
      live source rather than a literal table in `autocomplete.rs`.
- [ ] `crates/cyrup-tui/src/commands.rs:483-487`'s `has_arg_completion: false` is replaced by a value
      read from `slash_command_catalog()`, and the EXT-013/TUI-012 comment is updated or removed.
- [ ] `cargo build -p cyrup-tui` → 0 warnings; `cargo clippy -p cyrup-tui --all-targets` → no new
      diagnostics.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
