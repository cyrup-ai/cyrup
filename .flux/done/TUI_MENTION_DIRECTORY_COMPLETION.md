---
stage: done
status: completed
updated: 2026-08-28
---

# Offer Directories In `@`-Mention Completion And Keep The Caret Inside The Token So The User Can Drill Down

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** low · **Kind:** partial-behaviour · **Area:** Editor, input, keys and autocomplete

## Objective

Typing `@` and a folder name never offers the folder — only files — and every accepted mention ends
with a space, so there is no way to accept a directory and keep narrowing inside it. Upstream `@src`
offers `src/` first, and accepting it leaves the caret inside the token so the next characters filter
within `src/`. The path (non-`@`) completion in cyrup already behaves correctly; the mention path was
added later against a flat file list and never carried the behaviour over.

## Upstream reference

[`packages/tui/src/autocomplete.ts`](../../tmp/pi/packages/tui/src/autocomplete.ts):

- `:124-145` — `walkDirectoryWithFd` runs `fd` with **both** `"--type", "f"` and `"--type", "d"`
  (`:137-140`), plus `--follow --hidden --exclude .git`.
- `:205-217` — each result line is tagged from fd's trailing separator:
  `const hasTrailingSeparator = displayLine.endsWith("/")`, `normalizedPath` is the line without it,
  and the entry is pushed as `{ path: displayLine, isDirectory: hasTrailingSeparator }`.
- `:701-720` — `scoreEntry(filePath, query, isDirectory)` ends with
  `if (isDirectory && score > 0) score += 10;` — the comment above it reads "isDirectory adds bonus
  to prioritize folders".
- `:788-806` — the suggestion build in `getFuzzyFileSuggestions`:
  `pathWithoutSlash = isDirectory ? entryPath.slice(0, -1) : entryPath`,
  `completionPath = isDirectory ? \`${displayPath}/\` : displayPath`,
  `label = entryName + (isDirectory ? "/" : "")` (with `entryName = basename(pathWithoutSlash)`),
  `description = displayPath`, and the inserted text through
  `buildCompletionValue(completionPath, { isDirectory, isAtPrefix: true, isQuotedPrefix })`.
- `:100-121` — `buildCompletionValue` prefixes `@` and wraps in `"…"` when the path contains a space
  or the prefix was already quoted.
- `:414-425` — the `@` branch of `applyCompletion`, with the load-bearing comment:

  ```ts
  // Don't add space after directories so user can continue autocompleting
  const isDirectory = item.label.endsWith("/");
  const suffix = isDirectory ? "" : " ";
  ```

  and, two lines down, `const cursorOffset = isDirectory && hasTrailingQuote ? item.value.length - 1
  : item.value.length` (`:423`) — for a quoted directory the caret lands **inside** the closing
  quote, so the next keystrokes stay in the token.
- `:526-554` — `resolveScopedFuzzyQuery(rawQuery)` splits at the LAST `/`, `stat`s the base
  directory and, when it is one, re-roots the fd search there with the remainder as the query.

## Current state in cyrup-tui

The `is_dir` plumbing already exists and is already correct **for the path context** — the mention
context reuses it but pins the flag to `false`.

- [`autocomplete.rs:351`](../../crates/cyrup-tui/src/autocomplete.rs) — every mention candidate is
  built as `Completion { value: path.clone(), is_dir: false }`, a literal `false`. It is the only
  `CompletionContext::Mention` completion constructor in the crate.
- The candidate source cannot supply directories either:
  - `fd_list` ([`:370-388`](../../crates/cyrup-tui/src/autocomplete.rs)) passes
    `["--type", "f", "--color", "never", "--strip-cwd-prefix", "--exclude", ".git"]` at `:372` —
    files only;
  - the no-`fd` fallback `walk_list` ([`:393-425`](../../crates/cyrup-tui/src/autocomplete.rs))
    pushes a path only in the `else` of `if is_dir` (`:416-420`), i.e. it **enqueues** directories
    for traversal instead of emitting them.
- So `apply`'s Mention arm ([`:116-126`](../../crates/cyrup-tui/src/autocomplete.rs)) always appends
  `" "`, closing the token. Contrast the Path arm **two lines above** it
  ([`:113-115`](../../crates/cyrup-tui/src/autocomplete.rs)):
  `(completion.value.clone(), if completion.is_dir { "" } else { " " })`, fed by `read_dir_sorted`
  ([`:303-318`](../../crates/cyrup-tui/src/autocomplete.rs)), which carries `is_dir` from
  `entry.file_type()` and already sorts directories first.
- The mention arm does handle whitespace quoting (`@"…"`, `:118-124`) but not pi's inside-the-quote
  caret placement for a quoted directory (`autocomplete.ts:423`).
- No alternate mention path exists: the only other `mention` hits crate-wide are the cache
  (`set_mention_files` / [`editor/mod.rs:140-143`](../../crates/cyrup-tui/src/editor/mod.rs),
  [`editor/config.rs:174-183`](../../crates/cyrup-tui/src/editor/config.rs)) and image attachment
  ([`app/shell.rs:293-299`](../../crates/cyrup-tui/src/app/shell.rs)). The single mention test
  ([`src/tests/autocomplete.rs:230-238`](../../crates/cyrup-tui/src/tests/autocomplete.rs)) injects a
  flat file list, so it never exercises a directory.

## Subtasks

The first four are the core; the last two are the ranking/scoping increment and can land separately.

1. **Emit directories from `fd`.** Add `"--type", "d"` to the arg list in `fd_list`
   ([`autocomplete.rs:372`](../../crates/cyrup-tui/src/autocomplete.rs)) and tag each line from its
   trailing `/` (`autocomplete.ts:205-217`) — `fd` prints one for directories. The candidate list
   type has to grow from `String` to a `(path, is_dir)` pair (or equivalent) through `list_files`
   ([`:360-366`](../../crates/cyrup-tui/src/autocomplete.rs)) and the `mention_files` cache it feeds
   ([`editor/mod.rs:140-143`](../../crates/cyrup-tui/src/editor/mod.rs),
   [`editor/config.rs:174-183`](../../crates/cyrup-tui/src/editor/config.rs)).
2. **Emit directories from the fallback walk.** In `walk_list`
   ([`:393-425`](../../crates/cyrup-tui/src/autocomplete.rs)) push the directory itself *as well as*
   enqueueing it, keeping the existing `SKIP` set and the `visit_cap` bound.
3. **Carry the flag into the completion and the label.** At
   [`:351`](../../crates/cyrup-tui/src/autocomplete.rs) set `is_dir` from the candidate instead of
   `false`, and render the `SelectItem` label with a trailing `/` for a directory
   (`autocomplete.ts:803`).
4. **Withhold the trailing space for a directory.** Give the Mention arm of `apply`
   ([`:116-126`](../../crates/cyrup-tui/src/autocomplete.rs)) the same
   `if completion.is_dir { "" } else { " " }` suffix the Path arm at
   [`:113-115`](../../crates/cyrup-tui/src/autocomplete.rs) already has, and — for a quoted
   (whitespace-containing) directory — place the caret **before** the closing quote, per
   `autocomplete.ts:423`. Also append the trailing `/` to the inserted path itself
   (`completionPath`, `autocomplete.ts:794`), so the accepted token is `@src/` and the next
   keystroke narrows inside it.
5. *(increment)* **Rank directories first.** Apply pi's `+10` bonus to a directory's fuzzy score
   when the score is positive (`autocomplete.ts:719`) in `mention_autocomplete`'s filter at
   [`:342`](../../crates/cyrup-tui/src/autocomplete.rs).
6. *(increment)* **Re-root at the typed directory.** Port `resolveScopedFuzzyQuery`
   (`autocomplete.ts:526-554`): split the mention query at the last `/`, and when the prefix is an
   existing directory, list from there with the remainder as the query. Without it drill-down still
   works but ranks against the whole cached tree.

## Acceptance criteria

- [ ] `grep -n 'is_dir: false' crates/cyrup-tui/src/autocomplete.rs` no longer matches the mention
      constructor at `:351` (the `/`-command constructor at `:161` legitimately stays `false`)
- [ ] `fd_list` passes both `--type f` and `--type d`, and tags a result as a directory from fd's
      trailing `/`
- [ ] `walk_list` emits directory paths in addition to descending into them, still honouring `SKIP`
      and `visit_cap`
- [ ] Typing `@` plus a folder-name prefix lists the folder, labelled with a trailing `/`
- [ ] Accepting a directory suggestion inserts `@<path>/` with **no** trailing space and leaves the
      caret at the end of the token; the next typed character narrows within that folder
- [ ] Accepting a FILE suggestion still inserts a trailing space, and a whitespace-containing path is
      still quoted as `@"…"`
- [ ] For a quoted directory the caret lands before the closing quote (`autocomplete.ts:423`)
- [ ] *(increment)* A directory and a file with equal fuzzy scores rank directory-first
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test in `src/tests/autocomplete.rs` regresses

## Constraints

- Tests ARE in scope. (A prior revision of this file claimed "another team owns the test suite"; that was unfounded — `git log` over `crates/cyrup-tui/src/tests/` shows only the two authors already working here. It cost the alt-screen renderer its entire suite.)
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
