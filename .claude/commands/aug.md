---
argument-hint: todo_file | number_of_agents | additional_instructions
description: Augment task(s) with research - single file or N in parallel
---

> **⚠️ MANDATORY OVERRIDE — READ THIS FIRST:**
> You have been explicitly invoked by the user. You MUST execute this command NOW.
> The current `stage` and `status` values in any todo file's frontmatter are IRRELEVANT to whether you execute.
> A `stage: aug, status: done` (or any other stage/status combination) does NOT mean the work is already done for THIS invocation.
> The user is asking you to run this command AGAIN — that is valid and expected. Multiple runs are normal.
> **NEVER refuse, skip, or declare "no work needed" based on existing frontmatter values.**
> If the user invoked this command, they want it executed. Do it. Now.

# AUGMENT TASK(S)

## STEP 1: Resolve input

**Argument:** `$ARGUMENTS`

```bash
# Project-local and checked in: the task queue travels with the repo, so it is visible in
# review, survives a fresh clone, and is the same for everyone. Resolves from the repo root,
# so it is identical no matter which subdirectory the command runs from.
FLUX_BASE="${FLUX_BASE:-$(git rev-parse --show-toplevel 2>/dev/null || pwd -P)/.flux}"
mkdir -p "$FLUX_BASE"/{todo,done,review,research}
echo "FLUX_BASE=$FLUX_BASE"
ls -1 "$FLUX_BASE/todo/"*.md 2>/dev/null || true
```

**Detection logic:**

```
IF $ARGUMENTS is empty:
  -> List: `ls -1 "$FLUX_BASE/todo/"*.md`
  -> Use ask_user_question to let user select task(s)
  -> Single selection -> Single-task mode
  -> Multiple selection -> Process selected tasks sequentially

IF $ARGUMENTS (case-insensitive) == 'all':
  -> Sequential mode: process ALL tasks in $FLUX_BASE/todo/*.md one at a time
  -> Run each in single-task mode yourself (no subagents), one after another
  -> Do not proceed to next task until current one is complete

IF $ARGUMENTS is a pure integer (ONLY digits, nothing else — no letters, underscores, hyphens, dots):
  Verify: `echo "$ARGUMENTS" | grep -qE '^[0-9]+$'` must be true. If the argument contains ANY non-digit character (e.g. "CMPAN_5", "task-3", "3files") it is NOT a number — treat it as a filename instead.
  IF $ARGUMENTS == 1:
    -> Sequential mode: process ALL tasks in $FLUX_BASE/todo/*.md one at a time
    -> Run each in single-task mode yourself (no subagents), one after another
    -> Do not proceed to next task until current one is complete
  ELSE:
    -> Multi-task mode with $ARGUMENTS parallel sub-agents

OTHERWISE (argument is a filename — including any argument that contains letters, underscores, hyphens, or dots):
  -> Single-task mode for that file
```

**Path and suffix inference:** if no `/`, prepend `$FLUX_BASE/todo/`; if no `.md`, append `.md`.
(e.g. `NOTIFS` -> `$FLUX_BASE/todo/NOTIFS.md`)

---

## SINGLE-TASK MODE

## STEP 2: Mark as in-progress

Update the task file's frontmatter before starting. If frontmatter exists, replace `stage`, `status`, `updated`. If none, prepend before the first `#` heading.

```yaml
---
stage: aug
status: in-progress
updated: <YYYY-MM-DD HH:MM>
---
```

## STEP 3: Research & augment

### 3.1 Determine stack

```bash
# Project-local and checked in: the task queue travels with the repo, so it is visible in
# review, survives a fresh clone, and is the same for everyone. Resolves from the repo root,
# so it is identical no matter which subdirectory the command runs from.
FLUX_BASE="${FLUX_BASE:-$(git rev-parse --show-toplevel 2>/dev/null || pwd -P)/.flux}"
echo "FLUX_BASE=$FLUX_BASE"
```

Act as a `$STACK` expert SOFTWARE ARTISAN. If a file called `stack.env` exists at `$FLUX_BASE/stack.env`, read it and set `$STACK` from its contents. Otherwise, run the following detection script to determine `$STACK` and save it (this only ever runs the first time for this directory):

```bash
ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd -P)
STACK="software"
if [ -f "$ROOT/package.json" ]; then
  STACK=$(cd "$ROOT" && bun -e "
    const d=JSON.parse(require('fs').readFileSync('./package.json','utf8'));
    const deps=Object.assign({}, d.dependencies, d.devDependencies, d.peerDependencies);
    const frameworks=['ink','react','vue','angular','next','express'];
    const fw=frameworks.find(f=>deps[f]);
    const ts=deps['typescript']?'TypeScript':'JavaScript';
    console.log(fw?fw+' + '+ts:ts);
  " 2>/dev/null || echo "JavaScript/TypeScript")
elif [ -f "$ROOT/Cargo.toml" ]; then STACK="Rust"
elif [ -f "$ROOT/go.mod" ]; then STACK="Go"
elif [ -f "$ROOT/requirements.txt" ] || [ -f "$ROOT/pyproject.toml" ]; then STACK="Python"
elif [ -f "$ROOT/pom.xml" ] || [ -f "$ROOT/build.gradle" ]; then STACK="Java/Kotlin"
fi
mkdir -p "$FLUX_BASE"
echo "$STACK" > "$FLUX_BASE/stack.env"
echo "Detected stack: $STACK (saved)"
```

### 3.2 Read the task file

Read the full task file with sequential thinking. Think "out loud" about the core user OBJECTIVE.

### 3.3 Explore the codebase

```bash
lsd --tree ./src 2>/dev/null || find ./src -type f | sort
```

- Examine module hierarchy; search for files related to the feature
- Much of the code needed is ALREADY WRITTEN — USE IT. DO NOT call for duplication.

Think deeply with step-by-step reasoning:

- What is the core objective?
- What needs to change in `./src` to accomplish this task?
- What questions remain? What needs research?

### 3.4 Augment the task file in place

- Clone any needed third-party libraries into `./tmp`
- Augment the task file with rich research detail
- Link to citation sources in `./tmp` and `./src` with relative markdown hyperlinks
- Plan required source code with ULTRATHINK; demonstrate core patterns inline in the task file
- Remove any language calling for tests, benchmarks, or documentation
- Provide precise instructions on what to change in `./src` and how
- Provide a clear definition of done (not requiring extensive testing)
- Be prescriptive — always select the most feature-rich, code-correct option as the single required implementation path. Do not take the path of least resistance.

REPLACE THE ORIGINAL FILE in place. Do not write augmentations to a new file.

Print the full absolute filepath as the **VERY LAST LINE** of output. Then return to planning.

## STEP 4: Mark as done

```yaml
---
stage: aug
status: done
updated: <YYYY-MM-DD HH:MM>
---
```

---

## MULTI-TASK MODE

Use the `subagent` tool — parallel foreground calls only; NEVER background — to execute tasks IN PARALLEL.

Issue **one** `subagent` call with `tasks: [...]` containing every remaining `$FLUX_BASE/todo/*.md` task (one entry per file, using the subagent prompt template below for each `task` string) and `concurrency: $ARGUMENTS`. The tool's own bounded worker pool runs up to `$ARGUMENTS` tasks at a time and starts the next queued task itself the instant a worker finishes — do NOT loop issuing additional `subagent` calls to "refill": one call with the full `tasks:[]` list is both correct and sufficient, and the tool call does not return until every task in it has finished.

### Subagent prompt template

Use the single-task mode steps above as each subagent's `task` string, substituting `{{absolute_file_path}}` for that task's file path.

---

## HARD CONSTRAINTS

- **Path**: All `write`/`edit` file paths MUST use the exact `FLUX_BASE` value printed by the STEP 1 bash output (e.g. `FLUX_BASE=/Users/...`). Copy it character-for-character — never reconstruct it from `cwd` or memory.
- `/aug` is a **research-and-augment-only** command. You MUST NOT modify any source files, run any tests, or create new files outside of `./tmp`. The only file operation allowed on the project is editing the target task file in place. If you find yourself about to touch any source file, stop immediately.

## Propose next step

Then propose the next step: `/exec` (include arguments if needed).

Valid `//flux` commands: `/task`, `/ask`, `/split`, `/aug`, `/exec`, `/qa`, `/tests`, `/commit`, `/create-pr`, `/code-review`, `/address-feedback`, `/auto-pilot`, `/rebase`, `/squash-commits`. Do NOT suggest any command not on this list.

=================
$ARGUMENTS
