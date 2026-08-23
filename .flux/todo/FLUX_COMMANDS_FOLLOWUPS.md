---
stage: exec
status: done
updated: 2026-08-22 20:02
---

# Flux Command Loose Ends

## Description

Small follow-ups on `.claude/commands/`, none blocking, all known and none yet done. Grouped
because each is a few minutes and they share a test cycle.

1. **`auto-pilot.md` has no "valid commands" list.** Every other command ends with one
   constraining what it may propose next. `auto-pilot` orchestrates the whole pipeline and has
   none, so nothing stops it inventing a step.

2. **`/create-pr`'s template handling is untested.** It looks for a PR template and mirrors its
   headings; this repo has none, so that path has never run. Either test it against a fixture
   template or state that it is unexercised.

3. **The `<filepath>` placeholder in `code-review.md` STEP 8** sits inside a ```bash fence, so it
   is the one block of 75 that fails `bash -n`. It is documentation, not code. Either move it out
   of the bash fence or mark the fence as `text`, so a future lint over the command files can
   demand a clean pass instead of "clean except one known case".

4. **`/tests` and `/code-review` shell out to `bun`** in stack detection. Not installed in the web
   container. Both have `|| echo` fallbacks so they degrade rather than fail, but the Rust path
   never needs `bun` at all — the `Cargo.toml` branch could be checked first.

5. **The `GH_PATH=cli` branches have never run here** — no `gh` binary in the container. They are
   written against `gh`'s documented behaviour. Worth one pass on a machine that has it.

6. **No review has been posted to a live PR.** `code-review.md` STEP 13 is written against the
   GitHub MCP tools' schemas but never executed end-to-end. The pending-review flow
   (`create` without `event` → `add_comment_to_pending_review` → `submit_pending`) is the part
   most worth confirming, plus the file-level fallback when a line is not in the diff.

## Acceptance Criteria

- [ ] `auto-pilot.md` carries the valid-commands list
- [ ] The `<filepath>` fence no longer breaks `bash -n`, and all command bash blocks parse clean
- [ ] Stack detection prefers `Cargo.toml` over shelling out to `bun`
- [ ] The `cli` and live-posting paths are either exercised once or explicitly recorded as unexercised
