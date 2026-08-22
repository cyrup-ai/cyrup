---
argument-hint: task_description | JIRA_ticket_id
description: Create a new task file from a description or Jira ticket
---

# Create New Task

**Argument:** `$ARGUMENTS`

## STEP 1: Show the existing queue

```bash
# Project-local and checked in: the task queue travels with the repo, so it is visible in
# review, survives a fresh clone, and is the same for everyone. Resolves from the repo root,
# so it is identical no matter which subdirectory the command runs from.
FLUX_BASE="${FLUX_BASE:-$(git rev-parse --show-toplevel 2>/dev/null || pwd -P)/.flux}"
mkdir -p "$FLUX_BASE/todo" "$FLUX_BASE/done" "$FLUX_BASE/review" "$FLUX_BASE/research"
echo "FLUX_BASE=$FLUX_BASE"
ls -1 "$FLUX_BASE/todo/"*.md 2>/dev/null | wc -l | tr -d ' ' | xargs -I{} echo "Tasks already queued: {}"
ls -1 "$FLUX_BASE/todo/"*.md 2>/dev/null || true
```

**This command ADDS to the queue. It never clears it.** `todo/` is meant to stack — queue as many
tasks as you like and work them in any order. Existing files are listed for context only: do not
offer to delete, move or back them up, and do not treat a non-empty `todo/` as a problem to resolve
before continuing.

Draining the queue is `/qa`'s job — it moves a finished task to `$FLUX_BASE/done/<session>/`.
Nothing else removes a task file.

## STEP 2: Detect argument type

Pattern for Jira tickets: `^[A-Z]+-[0-9]+$` (e.g. `PROJECT-456`). Lowercase or plain descriptions do NOT match.

## STEP 3: Create task file

### If `$ARGUMENTS` matches Jira pattern

#### 3.1 Check if Jira MCP is available

**If `get_issue_by_key_or_link` is NOT available:**

Inform the user:

> "The Jira MCP server is not configured. To fetch ticket details, please install a Jira MCP server (e.g. `mcp-jira`) via `/mcp`. Then re-run this command."

**Stop. Do not proceed further.**

---

**If `get_issue_by_key_or_link` IS available:**

#### 3.2 Fetch the ticket

```
get_issue_by_key_or_link(issue_key_or_link: "$ARGUMENTS")
```

Read the **Summary**, **Description**, and **Acceptance Criteria** fields from the response. Convert any Jira markup to Markdown (see conversion table below).

#### 3.3 Write task file at `$FLUX_BASE/todo/<TICKET-ID>.md`

If that file already exists the ticket is already queued. Do not silently overwrite it — it may
carry local edits from `/ask` or `/aug`. Ask whether to refresh it from Jira or leave it as is.

```markdown
---
stage: new
status: done
updated: <YYYY-MM-DD HH:MM>
---

# <TICKET-ID>: <summary>

## Description

<description — converted from Jira markup to Markdown>

## Acceptance Criteria

<omit this section entirely if not present on the ticket>

## Source

- **Jira ticket:** <TICKET-ID>
- **Status:** <status>
- **Priority:** <priority>
```

### If `$ARGUMENTS` does NOT match Jira pattern

#### 3.1 Generate filename

Take 3-4 significant words, UPPER_SNAKE_CASE, append `.md` (e.g. "add dark mode toggle" → `DARK_MODE.md`)

Because the queue stacks, that name may already be taken. Check before writing:

```bash
ls -1 "$FLUX_BASE/todo/<FILENAME>.md" 2>/dev/null && echo "__TAKEN__" || echo "__FREE__"
```

If taken, do NOT overwrite — an existing queued task is someone's unstarted work. Pick a more
specific name from the request (`DARK_MODE.md` → `DARK_MODE_TOGGLE.md`), and if that collides too,
suffix `_2`, `_3`. Say which name you settled on and that the original was left untouched.

#### 3.2 Clarify if needed

If requirements are unclear, use `ask_user_question` to clarify before writing.

#### 3.3 Write `$FLUX_BASE/todo/<FILENAME>.md`

```markdown
---
stage: new
status: done
updated: <YYYY-MM-DD HH:MM>
---

# <Title derived from $ARGUMENTS — Title Case, human-readable (e.g. "Add Dark Mode Toggle")>

## Description

$ARGUMENTS

## Acceptance Criteria

- [ ] <To be clarified>
```

## HARD CONSTRAINTS

- **Path**: The `write` file path MUST use the exact `FLUX_BASE` value printed by the STEP 1 bash output (e.g. `FLUX_BASE=/Users/...`). Copy it character-for-character — never reconstruct it from `cwd` or memory.
- **Capture-only**: You MUST NOT read, edit, delete or move any file other than the single task file being created in `$FLUX_BASE/todo/`. No source files, no config files, no other flux files — nothing. If you find yourself about to touch anything else, stop immediately and output the task file instead.

## OUTPUT

```
Task created: $FLUX_BASE/todo/<FILENAME>.md
```

After creating the task file, start a new session by writing `$FLUX_BASE/session.env`:

```bash
SESSION_TS=$(date +%Y-%m-%d-%H-%M)
echo "SESSION_TS=$SESSION_TS" > "$FLUX_BASE/session.env"
echo "Session started: $SESSION_TS"
```

Then propose the next step: `/ask` (include arguments if needed).

Valid `//flux` commands: `/task`, `/ask`, `/split`, `/aug`, `/exec`, `/qa`, `/tests`, `/commit`, `/create-pr`, `/code-review`, `/address-feedback`, `/auto-pilot`, `/rebase`, `/squash-commits`. Do NOT suggest any command not on this list.

## JIRA MARKUP → MARKDOWN

| Jira              | Markdown      |
| ----------------- | ------------- |
| `*bold*`          | `**bold**`    |
| `_italic_`        | `*italic*`    |
| `{{code}}`        | `` `code` ``  |
| `{code}...{code}` | ` ```...``` ` |
| `h1. Title`       | `# Title`     |
| `h2. Title`       | `## Title`    |
| `* item`          | `- item`      |
| `# item`          | `1. item`     |
| `[text\|url]`     | `[text](url)` |

=================
$ARGUMENTS
