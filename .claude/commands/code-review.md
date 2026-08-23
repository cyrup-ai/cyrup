---
argument-hint: pr_number(optional) | additional_instructions
description: Code review with smart parent branch detection and optional PR posting
---

# CODE REVIEW WORKFLOW

**Argument:** `$ARGUMENTS` (optional PR number)

## STEP 1: Branch setup

### 1a. If PR number provided:

```bash
CURRENT_BRANCH=$(git branch --show-current)
PR_NUMBER="${ARGUMENTS}"
REPO_SLUG=$(git remote get-url origin | sed -E 's|^git@github\.com:|https://github.com/|; s|^https://[^/]*/||; s|\.git$||')
OWNER="${REPO_SLUG%%/*}"; REPO="${REPO_SLUG##*/}"
command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1 && echo "GH_PATH=cli" || echo "GH_PATH=mcp"
```

Resolve the PR's head and base branch by whichever path is available.

<!-- COVERAGE 2026-08-22: the GH_PATH dispatch line above IS exercised — no `gh` on
     PATH -> mcp; stub `gh` whose `auth status` exits 0 -> cli; stub whose `auth status` exits 1
     -> mcp. The `gh` command bodies in the GH_PATH=cli branches of this file are NOT exercised:
     there is no `gh` binary in this container. They are written against gh's documented
     behaviour and still need one pass on a machine with `gh` authenticated. -->

**If `GH_PATH=cli`:**

```bash
PR_BRANCH=$(gh pr view $PR_NUMBER --json headRefName -q '.headRefName')
PARENT_BRANCH=$(gh pr view $PR_NUMBER --json baseRefName -q '.baseRefName')
```

**If `GH_PATH=mcp`:** call `mcp__github__pull_request_read` with `method: "get"`, `owner: $OWNER`,
`repo: $REPO`, `pullNumber: $PR_NUMBER`. Take `PR_BRANCH` from `head.ref` and `PARENT_BRANCH` from
`base.ref`, then set them in the shell for the checkout below.

Then, on either path:

```bash
if [ "$PR_BRANCH" != "$CURRENT_BRANCH" ]; then
  git fetch origin "$PR_BRANCH" && git checkout "$PR_BRANCH"
fi
git fetch origin "$PARENT_BRANCH"
echo "PARENT_BRANCH: $PARENT_BRANCH"
```

### 1b. If no PR number:

```bash
CURRENT_BRANCH=$(git branch --show-current)
PARENT_BRANCH=""
for branch in main master develop; do
  if git rev-parse --verify "origin/$branch" >/dev/null 2>&1; then
    MERGE_BASE=$(git merge-base HEAD "origin/$branch" 2>/dev/null)
    if [ -n "$MERGE_BASE" ] && [ "$branch" != "$CURRENT_BRANCH" ]; then
      PARENT_BRANCH="$branch"; break
    fi
  fi
done
PARENT_BRANCH="${PARENT_BRANCH:-main}"
echo "PARENT_BRANCH: $PARENT_BRANCH"
```

## STEP 2: Detect stack

```bash
# Project-local and checked in: the task queue travels with the repo, so it is visible in
# review, survives a fresh clone, and is the same for everyone. Resolves from the repo root,
# so it is identical no matter which subdirectory the command runs from.
FLUX_BASE="${FLUX_BASE:-$(git rev-parse --show-toplevel 2>/dev/null || pwd -P)/.flux}"
echo "FLUX_BASE=$FLUX_BASE"
```

If a file called `stack.env` exists at `$FLUX_BASE/stack.env`, read it and set `$STACK` from its contents. Otherwise, run the following detection script to determine `$STACK` and save it (this only ever runs the first time for this directory):

```bash
ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd -P)
STACK="software"
# Cargo.toml is checked first: a Rust project never needs `bun`, and the package.json
# branch below is the only part of detection that shells out to a runtime that may not
# be installed.
if [ -f "$ROOT/Cargo.toml" ]; then STACK="Rust"
elif [ -f "$ROOT/package.json" ]; then
  STACK=$(cd "$ROOT" && bun -e "
    const d=JSON.parse(require('fs').readFileSync('./package.json','utf8'));
    const deps=Object.assign({}, d.dependencies, d.devDependencies, d.peerDependencies);
    const frameworks=['ink','react','vue','angular','next','express'];
    const fw=frameworks.find(f=>deps[f]);
    const ts=deps['typescript']?'TypeScript':'JavaScript';
    console.log(fw?fw+' + '+ts:ts);
  " 2>/dev/null || echo "JavaScript/TypeScript")
elif [ -f "$ROOT/go.mod" ]; then STACK="Go"
elif [ -f "$ROOT/requirements.txt" ] || [ -f "$ROOT/pyproject.toml" ]; then STACK="Python"
elif [ -f "$ROOT/pom.xml" ] || [ -f "$ROOT/build.gradle" ]; then STACK="Java/Kotlin"
fi
mkdir -p "$FLUX_BASE"
echo "$STACK" > "$FLUX_BASE/stack.env"
echo "Detected stack: $STACK (saved)"
```

## STEP 3: Branch ownership & merge base

```bash
MERGE_BASE=$(git merge-base HEAD "origin/$PARENT_BRANCH")
BRANCH_AUTHOR=$(git log --format='%an' --reverse "$MERGE_BASE..HEAD" 2>/dev/null | head -1)
CURRENT_USER=$(git config user.name)
[ "$BRANCH_AUTHOR" = "$CURRENT_USER" ] && IS_MY_BRANCH="true" || IS_MY_BRANCH="false"
echo "MERGE_BASE: $MERGE_BASE  IS_MY_BRANCH: $IS_MY_BRANCH"
```

## SCOPE CONSTRAINTS (CRITICAL)

**ONLY** flag issues **newly introduced** by changes vs `$MERGE_BASE` in files **directly modified**.

**DO NOT** flag: pre-existing issues in unchanged code, files/lines not touched, missing tests/benchmarks, style preferences.

> Why `$MERGE_BASE` not `origin/$PARENT_BRANCH`: upstream commits that landed after this branch was created would pollute the diff. Merge-base is the exact divergence point.

## STEP 4: Identify changed files

```bash
git diff --name-only "$MERGE_BASE"
```

## STEP 5: Analyze diffs

```bash
git diff "$MERGE_BASE"
```

Group files by: key modules, logical areas, high-risk changes (security, performance, data handling).

## STEP 6: Spawn code review sub-agents

Launch with the `subagent` tool — foreground calls only; NEVER background.

**Agent count:**
| Changed Files | Agents | Strategy |
|---|---|---|
| 1-5 | 1-3 | Group related files |
| 6-15 | 4-8 | One per module/feature area |
| 16-50 | 8-12 | Group by directory |
| 50+ | 12-15 max | By directory, prioritize high-risk |

Per agent, provide: file paths + line ranges, relevant diff snippets, module/area name, PR context.

### Sub-agent prompt template

````
# CODE REVIEW: {{MODULE_NAME}}

## Files: {{FILE_LIST_WITH_LINE_RANGES}}

## Diffs
```diff
{{RELEVANT_DIFF_SNIPPETS}}
```

## Checklist — flag only issues in code CHANGED by this PR vs `$MERGE_BASE`:
- [ ] Stubs/Placeholders: non-functional required code (TODOs, NotImplemented)
- [ ] Race Conditions: concurrency, async/await problems
- [ ] Performance: O(n²) loops, memory leaks, excessive allocations, blocking calls
- [ ] Logic Errors: off-by-one, null checks, edge cases, wrong conditionals
- [ ] Dead Code: unwired code, unreachable paths
- [ ] Security: input validation, injection risks, exposed secrets
- [ ] Requirements Mismatch: code doesn't fulfill intended purpose
- [ ] Complexity: functions >50 lines, cyclomatic complexity >10 branches, nesting >4 levels
- [ ] Duplication: repeated code blocks >5 lines, duplicate constants/magic strings used 3+ times
- [ ] Maintainability: unused variables/imports, dead code paths, hardcoded magic numbers
- [ ] Design: commented-out code blocks >3 lines, inconsistent naming, TODO/FIXME in changed code
- [ ] Language-specific (TypeScript): missing async/await handling, unhandled promise rejections, `var` usage, `==` vs `===` in changed lines

DO NOT create tasks for: missing tests, benchmarks, style preferences, pre-existing issues.

## Task File Creation

For each issue, create `$FLUX_BASE/review/<issue-slug>.md`:

```markdown
---
severity: critical|high|medium|low
file: <filepath>
lines: <start>-<end>
introduced: true
---

# <Issue Title>

## Problem
<description>

## Evidence
<code snippet or diff>

## Impact
<what could go wrong>

## Suggested Fix
<recommended approach>
```

Before creating any task, verify: issue is in CHANGED code, did NOT exist before these changes, is genuinely problematic.
````

## STEP 7: Categorize by severity

```bash
# Project-local and checked in: the task queue travels with the repo, so it is visible in
# review, survives a fresh clone, and is the same for everyone. Resolves from the repo root,
# so it is identical no matter which subdirectory the command runs from.
FLUX_BASE="${FLUX_BASE:-$(git rev-parse --show-toplevel 2>/dev/null || pwd -P)/.flux}"
mkdir -p "$FLUX_BASE/todo" "$FLUX_BASE/done" "$FLUX_BASE/review" "$FLUX_BASE/research"
mkdir -p "$FLUX_BASE/review/critical" "$FLUX_BASE/review/high" "$FLUX_BASE/review/medium" "$FLUX_BASE/review/low"
echo "FLUX_BASE=$FLUX_BASE"
```

Launch sub-agents (groups of 10) to move `$FLUX_BASE/review/*.md` to the appropriate severity subdirectory.

| Severity | Criteria                                                            |
| -------- | ------------------------------------------------------------------- |
| Critical | Security vulns, data loss, crashes, blocking bugs                   |
| High     | Significant logic errors, perf issues affecting UX, race conditions |
| Medium   | Minor logic issues, clarity problems, non-blocking bugs             |
| Low      | Minor improvements, low-probability edge cases                      |

## PATH REFRESH (after STEP 7)

Flat paths `$FLUX_BASE/review/*.md` no longer exist after categorization. All subsequent steps must locate files via:

```bash
find "$FLUX_BASE/review/" -name "*.md" -type f | sort
```

Steps 8, 10, and 11 sub-agents must use `find` or explicit subdirectory paths — never assume flat paths still exist.

## STEP 8: Verify newly introduced issues

Enumerate all task files with `find "$FLUX_BASE/review/" -name "*.md" -type f | sort`, then launch sub-agents. Each agent diffs the flagged file:

```text
git diff "$MERGE_BASE" -- <filepath>
```

Classify each issue:
| Classification | Definition |
|---|---|
| Introduced | Exists only in new changes; parent branch is clean |
| Pre-existing | Exists in parent branch; these changes didn't create it |
| Aggravated | Pre-existing but made worse by these changes |

## STEP 9: Summarize findings

Enumerate with `find "$FLUX_BASE/review/" -name "*.md" -type f | sort`. Report: introduced issues requiring attention, aggravated issues to consider, pre-existing issues flagged for removal.

## STEP 10: User checkpoint

```bash
find "$FLUX_BASE/review/" -name "*.md" -type f | sort
```

Use `ask_user_question`: present pre-existing issues (full paths), ask which to keep. Default if no response: DELETE pre-existing issue tasks.

## STEP 11: Consolidation & deduplication

```bash
find "$FLUX_BASE/review/" -name "*.md" -type f | sort
```

Divide into groups, assign to sub-agents. Sub-agents use full paths from `find` (e.g. `$FLUX_BASE/review/high/auth-race-condition.md`).

| Classification          | Action                   |
| ----------------------- | ------------------------ |
| Exact Duplicate         | Remove one               |
| Semantic Duplicate      | Merge into one           |
| Consolidation Candidate | Combine into single task |
| Unique                  | Keep as-is               |

## STEP 12: Final summary & post-review actions

```
Total Issues Found: X
+-- Critical: A  +-- High: B  +-- Medium: C  +-- Low: D

Duplicates Removed: Y  Tasks Consolidated: Z -> W  Final Task Count: N
```

**Recommendation:** `APPROVE` / `REQUEST CHANGES` / `NEEDS DISCUSSION`

**If PR provided OR reviewing someone else's branch (`IS_MY_BRANCH=false`):**

```bash
REVIEW_COUNT=$(find "$FLUX_BASE/review/" -name "*.md" -type f 2>/dev/null | wc -l | tr -d ' ')
if [ "$REVIEW_COUNT" -eq 0 ]; then
  echo "No issues found — no zip created."
else
  FLAT_BRANCH=$(echo "$(git branch --show-current)" | tr '/' '-')
  ZIP_NAME="${FLAT_BRANCH}-review.zip"
  cd "$FLUX_BASE" && zip -r "$ZIP_NAME" review/
  echo "Review package: $FLUX_BASE/$ZIP_NAME"
fi
```

Report zip location. If a PR number was provided, STEP 13 can post the findings to the PR directly — offer that rather than telling the user to attach the zip by hand. If no PR and it is someone else's branch: "share the zip with the branch author."

**If `IS_MY_BRANCH=true`:** Leave files in `$FLUX_BASE/review/`. Report: "Issues saved to `$FLUX_BASE/review/` organized by severity."

## STEP 13: Post the review to the PR (optional)

Run this step **only when a PR number was provided** in STEP 1a. With no PR there is nothing to post
to — hand over the zip from STEP 12 instead and skip to NEXT STEP.

### 13a. Decide the review event

STEP 12 produced a recommendation. Map it to `$EVENT`, then apply the self-review rule:

| STEP 12 recommendation | GitHub `event` |
| ---------------------- | -------------- |
| `APPROVE`              | `APPROVE`      |
| `REQUEST CHANGES`      | `REQUEST_CHANGES` |
| `NEEDS DISCUSSION`     | `COMMENT`      |

**If `IS_MY_BRANCH=true`, force the event to `COMMENT`.** GitHub rejects `APPROVE` and
`REQUEST_CHANGES` on your own pull request — the whole post fails, not just the event — so a
self-review posts as a plain comment regardless of the recommendation.

### 13b. Build the comment bodies

Enumerate the issue files with `find "$FLUX_BASE/review/" -name "*.md" -type f | sort` — their
count is `$N`, the number of inline comments the review will carry. Each file's frontmatter supplies
`file`, `lines: <start>-<end>` and `severity`.

One inline comment per issue file. Body: the issue title, then its Problem / Impact / Suggested Fix
sections, prefixed with the severity — `**critical**`, `**high**`, `**medium**`, `**low**`. Drop the
Evidence section: it quotes the very lines the comment is anchored to, so GitHub already shows it.

Every comment body and the summary body MUST end with the attribution footer, so a reader can tell
the review was machine-generated:

```
---
_Generated by [Claude Code](https://claude.ai/code)_
```

### 13c. Confirm before posting

Posting is public and visible to everyone on the PR, and a submitted review cannot be edited back
into a draft. Never post without asking.

Use `ask_user_question` — question: "Post this review to PR #$PR_NUMBER as $EVENT, with $N inline
comments?" / header: "Post review" / options:

- `Post it` → continue to 13d
- `Summary only` → continue, but skip every inline comment; post only the STEP 12 summary
- `Don't post` → skip to NEXT STEP, leaving the zip as the hand-off

<!-- COVERAGE 2026-08-22: STEP 13 has never been run against a live PR. The pending-review
     flow below (`create` with no `event` -> add_comment_to_pending_review -> `submit_pending`),
     the FILE-level fallback for a line that is not in the diff, and `delete_pending` are written
     against the MCP tools' schemas and are UNVERIFIED end-to-end. 13e is doubly unexercised —
     no live PR and no `gh` binary. Nothing here has posted a byte to GitHub. -->

### 13d. Post — `GH_PATH=mcp`

Three calls, in order:

1. `mcp__github__pull_request_review_write` with `method: "create"`, `owner`, `repo`, `pullNumber`,
   and **no `event`** — omitting `event` is what makes it a *pending* review that comments can be
   attached to. Passing `event` here submits an empty review immediately and the inline comments
   have nothing to attach to.
2. For each issue, `mcp__github__add_comment_to_pending_review` with `owner`, `repo`, `pullNumber`,
   `path` (the issue's `file`), `body`, `subjectType: "LINE"`, `side: "RIGHT"`, and `line` set to the
   **end** of the issue's line range. Add `startLine` (the range start) and `startSide: "RIGHT"` only
   when the range spans more than one line — GitHub rejects `startLine` equal to `line`.
3. `mcp__github__pull_request_review_write` with `method: "submit_pending"`, the STEP 12 summary as
   `body`, and the `event` from 13a.

**If a comment is rejected because its line is not in the diff** — the flagged line is context rather
than a changed line — retry that one comment with `subjectType: "FILE"` and no `line`/`startLine`.
That anchors it to the file as a whole, which always succeeds for a file in the diff. Report how many
comments landed on a file rather than a line.

**If the pending review must be abandoned** (repeated failures, or the user interrupts), call
`pull_request_review_write` with `method: "delete_pending"` — a pending review left behind blocks the
next `create` and is invisible to everyone but its author.

### 13e. Post — `GH_PATH=cli`

`gh` posts a review and its inline comments in one API call. Write the payload to a file under
`$FLUX_BASE` (never to the repo), then post:

```bash
# review.json: {"body": "<summary>", "event": "<EVENT>", "comments": [...]}
# each comment: {"path": "<file>", "line": <end>, "side": "RIGHT", "body": "<text>"}
# multi-line: add "start_line": <start> and "start_side": "RIGHT"
gh api "repos/$OWNER/$REPO/pulls/$PR_NUMBER/reviews" --method POST --input "$FLUX_BASE/review.json"
```

A `422` here almost always means a comment's `line` is not part of the diff. Drop that comment's
`line`/`side` keys and resend it — a comment with only `path` and `body` anchors to the file.

### 13f. Report

Print the review URL from the result, the event posted, how many comments anchored to lines, and how
many fell back to the file. Say plainly if the event was downgraded to `COMMENT` by the
`IS_MY_BRANCH` rule.

The zip from STEP 12 is still written — posting supplements the hand-off, it does not replace it.

## ERROR HANDLING

- Git command fails → report and stop
- Sub-agent fails → log, continue with remaining agents
- No changes found → report "No files changed vs parent branch" and exit
- 100+ issues found → pause and ask user how to proceed before categorization
- GitHub is unreachable (no `gh`, and the `mcp__github__*` tools absent or the repo out of scope) → report it and continue with the local review against the detected parent branch
- Posting rejected because a comment's line is not in the diff → re-anchor that comment to the file (STEP 13d/13e), never drop the finding
- Posting rejected for the whole review → keep the pending review out of the way with `delete_pending`, report the error verbatim, and fall back to the zip hand-off
- Repository outside the session's granted scope → report it as an access grant the user must widen; do NOT retry or route around it

## HARD CONSTRAINTS

- **Path**: All `write`/`edit`/`mv`/`cp` file paths MUST use the exact `FLUX_BASE` value printed by STEP 2 or STEP 7 bash output (e.g. `FLUX_BASE=/Users/...`). Copy it character-for-character — never reconstruct it from `cwd` or memory.
- `/code-review` MUST NOT modify any source files. The only permitted file operations are: creating issue task files in `$FLUX_BASE/review/`, moving/deleting those files during deduplication, creating the zip archive, and writing the STEP 13 review payload under `$FLUX_BASE`. No changes to `./src/`, no git commits, no pushes.
- **Posting is the one outward-facing action this command takes.** It requires the STEP 13c confirmation every time — never post because the review "looks done", because a previous run was approved, or because the user asked for a review. Reviewing and publishing a review are different requests.
- Never post `APPROVE` or `REQUEST_CHANGES` on the user's own PR (`IS_MY_BRANCH=true`); GitHub rejects it and the whole submission fails.

## NEXT STEP

Then propose the next step:

- if user is reviewing their own branch: `/address-feedback`
- if the review was posted in STEP 13: nothing further — the PR now carries the findings
- if user is reviewing someone else's PR and did not post: `share the zip with the author`

Valid `//flux` commands: `/task`, `/ask`, `/split`, `/aug`, `/exec`, `/qa`, `/tests`, `/commit`, `/create-pr`, `/code-review`, `/address-feedback`, `/auto-pilot`, `/rebase`, `/squash-commits`. Do NOT suggest any command not on this list.

=================
$ARGUMENTS
