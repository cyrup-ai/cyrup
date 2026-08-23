---
description: Create a GitHub PR for the current branch, or show the existing PR URL and status
---

# CREATE A GITHUB PULL REQUEST

## STEP 1: Validate branch

```bash
BRANCH=$(git branch --show-current)
echo "CURRENT_BRANCH: $BRANCH"
```

If `BRANCH` is `main` or `master`, print and stop:

```
Error: You are on the default branch ("$BRANCH").
Switch to a feature branch before opening a PR.
```

## STEP 2: Check for existing PR

First establish which GitHub path is available — this decides STEP 2 and STEP 7:

```bash
REPO_SLUG=$(git remote get-url origin | sed -E 's|^git@github\.com:|https://github.com/|; s|^https://[^/]*/||; s|\.git$||')
OWNER="${REPO_SLUG%%/*}"; REPO="${REPO_SLUG##*/}"
command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1 && echo "GH_PATH=cli" || echo "GH_PATH=mcp"
echo "OWNER=$OWNER REPO=$REPO BRANCH=$BRANCH"
```

<!-- COVERAGE 2026-08-22: the GH_PATH dispatch line above IS exercised — no `gh` on
     PATH -> mcp; stub `gh` whose `auth status` exits 0 -> cli; stub whose `auth status` exits 1
     -> mcp. The `gh` command bodies in the GH_PATH=cli branches of this file are NOT exercised:
     there is no `gh` binary in this container. They are written against gh's documented
     behaviour and still need one pass on a machine with `gh` authenticated. -->

**If `GH_PATH=cli`** (a local machine with `gh` authenticated):

```bash
EXISTING=$(gh pr view --json number,url,state,title 2>/dev/null)
echo "$EXISTING"
```

**If `GH_PATH=mcp`** (a web/cloud container — no `gh` binary, GitHub reached through MCP tools):

Call `mcp__github__list_pull_requests` with `owner: $OWNER`, `repo: $REPO`, `head: "$OWNER:$BRANCH"`,
`state: "all"`, `fields: ["number","title","state","html_url"]`. Treat a non-empty result array as
the existing PR; read `html_url` where the CLI path reads `url`. An empty array means no PR yet.

If output is non-empty JSON, print and stop:

```
A PR already exists for this branch:
  Title:  <title>
  URL:    <url>
  State:  <state>
```

## STEP 3: Check for commits ahead of default branch

```bash
# Default branch WITHOUT `gh`: the clone's remote HEAD, then ask the remote, then fall back.
DEFAULT_BRANCH=$(git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null | sed 's|^origin/||')
[ -z "$DEFAULT_BRANCH" ] && DEFAULT_BRANCH=$(git remote show origin 2>/dev/null | sed -n 's|.*HEAD branch: ||p')
[ -z "$DEFAULT_BRANCH" ] && DEFAULT_BRANCH=main
# A fresh container clone often has ONLY the checked-out branch, so materialise the base ref.
git fetch origin "$DEFAULT_BRANCH" --quiet 2>/dev/null || true
echo "DEFAULT_BRANCH: $DEFAULT_BRANCH"
COMMIT_COUNT=$(git rev-list HEAD ^"origin/${DEFAULT_BRANCH}" --count 2>/dev/null || echo "0")
echo "COMMIT_COUNT: $COMMIT_COUNT"
```

If `COMMIT_COUNT` is `0`, print and stop:

```
Warning: No commits ahead of origin/$DEFAULT_BRANCH.
Push at least one commit before opening a PR.
```

## STEP 4: Derive PR title from branch name

```bash
BRANCH=$(git branch --show-current)
SLUG=$(echo "$BRANCH" | sed 's|.*/||')
JIRA_ID=$(echo "$SLUG" | grep -oE '^[A-Z]+-[0-9]+' || true)
if [ -n "$JIRA_ID" ]; then
  DESC=$(echo "$SLUG" | sed "s/^${JIRA_ID}-//")
else
  DESC="$SLUG"
fi
TITLE_DESC=$(echo "$DESC" | tr '-' ' ' | awk '{for(i=1;i<=NF;i++) $i=toupper(substr($i,1,1)) tolower(substr($i,2)); print}')
if [ -n "$JIRA_ID" ]; then
  PR_TITLE="${JIRA_ID}: ${TITLE_DESC}"
else
  PR_TITLE="${TITLE_DESC}"
fi
echo "PR_TITLE: $PR_TITLE"
```

Examples: `rio/MYPROJ-745-add-create-pr-command` → `MYPROJ-745: Add Create Pr Command` | `feat/dark-mode` → `Dark Mode`

## STEP 5: Detect PR template

```bash
REPO_ROOT=$(git rev-parse --show-toplevel)
TEMPLATE_PATH=""

# Check single-file locations in GitHub-documented priority order
for candidate in \
  ".github/pull_request_template.md" \
  ".github/PULL_REQUEST_TEMPLATE.md" \
  "docs/pull_request_template.md" \
  "docs/PULL_REQUEST_TEMPLATE.md" \
  "pull_request_template.md" \
  "PULL_REQUEST_TEMPLATE.md"; do
  if [ -f "$REPO_ROOT/$candidate" ]; then
    TEMPLATE_PATH="$REPO_ROOT/$candidate"
    break
  fi
done

# Check directory locations if no single-file match found
TEMPLATE_NOTE=""
TEMPLATE_DIR=""
if [ -z "$TEMPLATE_PATH" ]; then
  for dir in \
    ".github/PULL_REQUEST_TEMPLATE" \
    "docs/PULL_REQUEST_TEMPLATE" \
    "PULL_REQUEST_TEMPLATE"; do
    if [ -d "$REPO_ROOT/$dir" ]; then
      TEMPLATE_COUNT=$(ls "$REPO_ROOT/$dir/"*.md 2>/dev/null | wc -l | tr -d ' ')
      if [ "$TEMPLATE_COUNT" -gt 1 ]; then
        TEMPLATE_NOTE="Multiple templates found in $dir — automatic selection is ambiguous. Falling back to default body."
        TEMPLATE_DIR="$REPO_ROOT/$dir"
        break
      elif [ "$TEMPLATE_COUNT" -eq 1 ]; then
        TEMPLATE_PATH=$(ls "$REPO_ROOT/$dir/"*.md 2>/dev/null | head -1)
        break
      fi
    fi
  done
fi

if [ -n "$TEMPLATE_PATH" ]; then
  echo "TEMPLATE_FOUND: yes"
  echo "TEMPLATE_PATH: $TEMPLATE_PATH"
  echo "---TEMPLATE_BEGIN---"
  cat "$TEMPLATE_PATH"
  echo "---TEMPLATE_END---"
else
  echo "TEMPLATE_FOUND: no"
  if [ -n "$TEMPLATE_NOTE" ]; then
    echo "TEMPLATE_NOTE: $TEMPLATE_NOTE"
    ls "$TEMPLATE_DIR/"*.md 2>/dev/null | sed 's|.*/||' | while read f; do echo "  - $f"; done
  fi
fi
```

## STEP 6: Generate PR body from diff

```bash
git log HEAD ^origin/${DEFAULT_BRANCH} --oneline
git diff origin/${DEFAULT_BRANCH}...HEAD --stat
```

**If a template was found in STEP 5:**

Parse every section heading from the template file (e.g., `## 📝 Problem`, `## 💡 Solution`, `### 📚 References`, `### CCM:`, `### 🖼️ Screenshots`, `### 📖 Build History`).

Fill in **EVERY section** from the template using context from the current implementation:

- Do not invent your own sections
- Do not skip any sections
- Do not reorder sections
- Match the exact heading text, emoji, and formatting from the template
- Preserve verbatim any HTML comments, unchecked checkboxes (`- [ ] ...`), and static non-heading text from the template; only replace designated placeholder text (e.g., italic or angle-bracket placeholders like `*describe here*` or `<your text>`)

**If no template was found:**

Write a PR body with:

- `## Summary` — 1–3 bullets: what changed and why (specific to actual commits/files, no placeholders)
- `## Test plan` — concrete manual verification steps specific to the changes

In both cases, the body is written as literal text inline in the heredoc in STEP 7. Do NOT use a shell variable for the body.

## STEP 7: Create the PR

**If `GH_PATH=cli`:** do NOT pass `--web`, `--fill`, `--draft`, `--reviewer`, `--label`,
`--milestone`, or `--assignee`. Write the body inline in the heredoc — do NOT use shell variables
for it:

```bash
PR_URL=$(gh pr create \
  --title "$PR_TITLE" \
  --body "$(cat <<'EOF'
<generated body — either template-filled or fallback Summary+Test plan>
EOF
)")
echo "PR_URL: $PR_URL"
```

On non-zero exit, surface the raw `gh` error verbatim. Common causes: not authenticated (`gh auth login`), no remote tracking branch (`git push -u origin <branch>`), network/API error.

**If `GH_PATH=mcp`:** call `mcp__github__create_pull_request` with `owner: $OWNER`, `repo: $REPO`,
`title: $PR_TITLE`, `head: $BRANCH`, `base: $DEFAULT_BRANCH`, and `body` set to the generated body.
Do not pass `draft` or `reviewers`. The PR URL is `html_url` on the result.

Pass the body as the `body` argument directly — the heredoc rule above exists to stop the shell from
mangling backticks and `$` in the body, and a tool argument is not shell-interpreted, so that hazard
does not apply here.

On error, surface the tool's message verbatim. Common causes: the head branch was never pushed
(`git push -u origin <branch>`), a PR already exists for this head, or the repository is outside the
session's granted scope — the last one is an access grant to raise with the user, not something to
retry or work around.

## STEP 8: Print result

```
PR created successfully:
  Title:  <PR_TITLE>
  URL:    <PR_URL>
```

Do NOT open a browser window.

## HARD CONSTRAINT

`/create-pr` MUST NOT modify any source files, task files, or config files. The only permitted operations are reading git history, calling GitHub (via `gh` or the `mcp__github__*` tools), and pushing the branch if needed. Write the PR body inline — never via shell variables. Do NOT open a browser window.

## Propose next step

Then propose the next step: `/code-review`

Valid `//flux` commands: `/task`, `/ask`, `/split`, `/aug`, `/exec`, `/qa`, `/tests`, `/commit`, `/create-pr`, `/code-review`, `/address-feedback`, `/auto-pilot`, `/rebase`, `/squash-commits`. Do NOT suggest any command not on this list.
