---
argument-hint: task_file | additional_instructions
description: Decompose a large task into smaller, single-session tasks.
---

# DECOMPOSE A LARGE TASK

**Argument:** `$ARGUMENTS`

## STEP 1: Mark as in-progress

Before anything else, update the input task file's frontmatter (replace existing values, or prepend if no frontmatter exists):

```yaml
---
stage: split
status: in-progress
updated: <YYYY-MM-DD HH:MM>
---
```

## STEP 2: Resolve path & setup

If `$ARGUMENTS` has no `/`, prepend `$FLUX_BASE/todo/`. If no `.md`, append `.md`. (e.g. `NOTIFS` → `$FLUX_BASE/todo/NOTIFS.md`)

```bash
# Project-local and checked in: the task queue travels with the repo, so it is visible in
# review, survives a fresh clone, and is the same for everyone. Resolves from the repo root,
# so it is identical no matter which subdirectory the command runs from.
FLUX_BASE="${FLUX_BASE:-$(git rev-parse --show-toplevel 2>/dev/null || pwd -P)/.flux}"
mkdir -p "$FLUX_BASE/todo" "$FLUX_BASE/done" "$FLUX_BASE/review" "$FLUX_BASE/research"
echo "FLUX_BASE=$FLUX_BASE"
```

## STEP 3: Decompose into subtask files

### 3.1 Choose a prefix

Short, uppercase, human-meaningful, fewer than 8 characters. Example: tasks about "in production" stubs → `IN_PROD_1.md`, `IN_PROD_2.md`, etc.

### 3.2 Write subtask files

Output files:

- `$FLUX_BASE/todo/PREFIX_1.md`
- `$FLUX_BASE/todo/PREFIX_2.md`
- `$FLUX_BASE/todo/PREFIX_3.md`
- ... etc.

Each task must be:

- Focused on a single area of concern
- Achievable in **one Claude session** — after writing, ask yourself "is this really doable in one session?" If no, split into A/B/C variants
- Free of research tasks (all research is done prior to execution)
- Free of tests and benchmarks (see 3.3 and 3.4)

**Frontmatter (required at top of every subtask file):**

```yaml
---
stage: new
status: done
updated: <YYYY-MM-DD HH:MM>
---
```

**Body must include:**

- `OBJECTIVE:` — what this task accomplishes
- Numbered subtasks: SUBTASK1, SUBTASK2, SUBTASK3...
- For each: what changes, where it changes, why it changes
- Clear definition of done
- Research notes and locations of relevant docs/source code

### 3.3 Tests are in scope

There is no separate team that owns tests. Every task that changes behaviour must name the tests
that pin it — each one failing before the change and passing after — and the task's definition of
done includes those tests passing. Each task file must state explicitly which tests it adds or
extends. (Deliberate divergence from the upstream template, which delegated tests to another team.)

### 3.4 Benchmarks

Write a benchmark only when the task is performance-scoped; otherwise say "no benchmarks" and why.

## STEP 4: Preserve the original task file

Update the original task file's frontmatter to mark it as split, then move it to the session done directory.

1. Update the original file's frontmatter (replace `stage`, `status`, `updated` — keep body unchanged):

```yaml
---
stage: split
status: complete
updated: <YYYY-MM-DD HH:MM>
---
```

2. Read the session timestamp and move to the session done subdirectory:

```bash
SESSION_TS=$(grep '^SESSION_TS=' "$FLUX_BASE/session.env" 2>/dev/null | cut -d= -f2)
SESSION_TS="${SESSION_TS:-$(date +%Y-%m-%d-%H-%M)}"
mkdir -p "$FLUX_BASE/done/$SESSION_TS"
mv "<original_task_file_path>" "$FLUX_BASE/done/$SESSION_TS/"
```

The original file is preserved in `done/$SESSION_TS/` as an audit trail of the pre-split specification.

## HARD CONSTRAINTS

- **Path**: All `write` file paths MUST use the exact `FLUX_BASE` value printed by the STEP 2 bash output (e.g. `FLUX_BASE=/Users/...`). Copy it character-for-character — never reconstruct it from `cwd` or memory.
- `/split` is a **decompose-only** command. You MUST NOT modify any source files, run any tests, or touch any file outside of `$FLUX_BASE/todo/`. The only file operations allowed are: writing the new subtask files and deleting the original task file. If you find yourself about to do anything else, stop immediately.

## PROPOSE NEXT STEP

Then propose the next step: `/aug` (include arguments if needed).

Valid `//flux` commands: `/task`, `/ask`, `/split`, `/aug`, `/exec`, `/qa`, `/tests`, `/commit`, `/create-pr`, `/code-review`, `/address-feedback`, `/auto-pilot`, `/rebase`, `/squash-commits`. Do NOT suggest any command not on this list.

=================
$ARGUMENTS
