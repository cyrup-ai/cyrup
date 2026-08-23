---
stage: exec
status: done
updated: 2026-08-22 20:02
---

# Flux Command Loose Ends

## Description

Four mechanical edits across the 14 command files in `.claude/commands/`. Every claim below was
re-verified against the files on 2026-08-22; two of the six items originally listed here turned out
to be **already correct or misstated** and have been struck (see *Struck items*). Nothing here needs
`cargo`, a network call, or a new file.

Environment facts established while verifying, because three of the original items asserted the
opposite:

| claim | actual |
|---|---|
| `bun` is not installed in this container | **false** — `bun` 1.3.11 is on `PATH` at `/root/.bun/bin/bun` |
| `/tests` shells out to `bun` | **false** — [tests.md:36-41](../../.claude/commands/tests.md) uses plain `[ -f ... ]` tests and checks `Cargo.toml` **first**; the string `bun` does not appear in the file |
| `gh` is available | **false** — no `gh` binary; every `GH_PATH=cli` branch resolves to `mcp` here |
| this repo has a PR template | **false** — no `.github/` directory exists at all |

---

## Edit 1 — `auto-pilot.md` is the only command with no valid-commands list

**Confirmed.** All 13 other files carry the line exactly once;
[auto-pilot.md](../../.claude/commands/auto-pilot.md) carries it zero times.
Verified with `grep -c 'Valid \`//flux\` commands' .claude/commands/*.md` → every file `1` except
`auto-pilot.md` → `0`.

The list constrains what a command may propose next. `/auto-pilot` does not *propose* — it *invokes*,
sequencing `/task → /ask → /split → /aug → /exec → /qa → /tests → /commit`
([auto-pilot.md:22-52](../../.claude/commands/auto-pilot.md)) — so the list belongs against the
`HARD CONSTRAINT` at [auto-pilot.md:58-60](../../.claude/commands/auto-pilot.md), where it binds
behaviour, and it needs one extra sentence to speak to invocation rather than suggestion.

**File:** `.claude/commands/auto-pilot.md`

Before (lines 58-60):

```markdown
## HARD CONSTRAINT

`/auto-pilot` orchestrates other `//flux` commands — it does not implement logic of its own beyond sequencing. Each step's own HARD CONSTRAINTs apply in full. Do not skip steps, do not merge steps, do not take shortcuts.
```

After:

```markdown
## HARD CONSTRAINT

`/auto-pilot` orchestrates other `//flux` commands — it does not implement logic of its own beyond sequencing. Each step's own HARD CONSTRAINTs apply in full. Do not skip steps, do not merge steps, do not take shortcuts.

`/auto-pilot` may only invoke commands from the list below. It MUST NOT invent, rename, or improvise a pipeline step.

Valid `//flux` commands: `/task`, `/ask`, `/split`, `/aug`, `/exec`, `/qa`, `/tests`, `/commit`, `/create-pr`, `/code-review`, `/address-feedback`, `/auto-pilot`, `/rebase`, `/squash-commits`. Do NOT suggest any command not on this list.
```

Keep the sentence byte-identical to the other 13 files — it is the greppable invariant the definition
of done checks.

---

## Edit 2 — the one `bash -n` failure in the whole command set

**Confirmed, and it is exactly one.** Extracting every ` ```bash ` fence from all 14 files and running
`bash -n` on each gives **75 blocks, 1 failure**:

```text
code-review.md lines 235-236: syntax error near unexpected token `newline'
  git diff "$MERGE_BASE" -- <filepath>
```

The fence at [code-review.md:234](../../.claude/commands/code-review.md) opens as `bash`, but its body
is a documentation placeholder — `<filepath>` is prose standing in for the file each STEP 8 sub-agent
was handed, not a value any shell resolves. Marking the fence `text` costs nothing (no other command
file relies on `bash` highlighting for placeholder blocks) and takes the set to **74 blocks, 0
failures** — verified on a patched copy.

**File:** `.claude/commands/code-review.md`, line 234.

Before (lines 234-236):

````text
```bash
git diff "$MERGE_BASE" -- <filepath>
```
````

After:

````text
```text
git diff "$MERGE_BASE" -- <filepath>
```
````

Change the opening fence only. Do not touch the command text, and do not "fix" the placeholder into
`"$FILEPATH"` — no such variable is ever set, and that would trade a parse error for a silent one.

---

## Edit 3 — stack detection reads the wrong directory

The original item asked to "prefer `Cargo.toml` over shelling out to `bun`". That premise does not
hold: `bun` **is** installed here, the `bun -e` call is already guarded by
`|| echo "JavaScript/TypeScript"` and degrades cleanly when it is not (verified by re-running the
block with `bun` stripped from `PATH`), and it only ever fires when a `package.json` exists — which
this repo does not have. Reordering `Cargo.toml` ahead of `package.json` would also make every
Rust+JS repo (Tauri, napi, wasm-pack) report `Rust` and lose the framework detail. **The reorder is
declined.**

The same block does carry a real, reproducible defect, and it is the reason it was worth looking at:
**every `-f` test is relative to `$PWD`, while the cache it writes is relative to the repo root.**

`FLUX_BASE` is resolved as `$(git rev-parse --show-toplevel)/.flux`, so it is the same from anywhere
in the tree — but `[ -f "package.json" ]` and `[ -f "Cargo.toml" ]` are not. Measured by running the
block verbatim from four directories in this repo:

| cwd | detected |
|---|---|
| `/home/user/cyrup` | `Rust` |
| `/home/user/cyrup/crates` | **`software`** |
| `/home/user/cyrup/.claude/commands` | **`software`** |
| `/home/user/cyrup/crates/cyrup-agent` | `Rust` |

A single `/aug` run started from `crates/` therefore writes `software` into the repo-root
`.flux/stack.env`, and because every command short-circuits on "if `stack.env` exists, read it", that
wrong value sticks for every later run in the repo. `.flux/stack.env` is gitignored
([.flux/.gitignore](../../.flux/.gitignore)) so it never surfaces in review.

The block is **byte-identical in four files** (md5 `44031bbc...` across all four), so this is one
replacement applied four times:

| file | lines to replace |
|---|---|
| [aug.md](../../.claude/commands/aug.md) | 91-105 |
| [exec.md](../../.claude/commands/exec.md) | 93-107 |
| [qa.md](../../.claude/commands/qa.md) | 93-107 |
| [code-review.md](../../.claude/commands/code-review.md) | 75-89 |

Before (all four, identical):

```bash
STACK="software"
if [ -f "package.json" ]; then
  STACK=$(bun -e "
    const d=JSON.parse(require('fs').readFileSync('./package.json','utf8'));
    const deps=Object.assign({}, d.dependencies, d.devDependencies, d.peerDependencies);
    const frameworks=['ink','react','vue','angular','next','express'];
    const fw=frameworks.find(f=>deps[f]);
    const ts=deps['typescript']?'TypeScript':'JavaScript';
    console.log(fw?fw+' + '+ts:ts);
  " 2>/dev/null || echo "JavaScript/TypeScript")
elif [ -f "Cargo.toml" ]; then STACK="Rust"
elif [ -f "go.mod" ]; then STACK="Go"
elif [ -f "requirements.txt" ] || [ -f "pyproject.toml" ]; then STACK="Python"
elif [ -f "pom.xml" ] || [ -f "build.gradle" ]; then STACK="Java/Kotlin"
fi
```

After (all four, identical):

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
```

The three lines that follow the replaced region — `mkdir -p "$FLUX_BASE"`,
`echo "$STACK" > "$FLUX_BASE/stack.env"`, `echo "Detected stack: $STACK (saved)"` — are unchanged and
stay where they are.

`cd "$ROOT" &&` inside the command substitution is what keeps the embedded JavaScript's
`readFileSync('./package.json')` valid without editing the JS. Verified behaviour of the replacement:

| cwd / fixture | result |
|---|---|
| repo root, `crates/`, `.claude/commands/`, `crates/cyrup-agent/`, `.flux/todo/` | `Rust` (all five) |
| fixture with `react` + `typescript` in `package.json` | `react + TypeScript` |
| same fixture, `bun` removed from `PATH` | `JavaScript/TypeScript` |
| fixture with `go.mod` | `Go` |
| fixture with `pyproject.toml` | `Python` |
| empty fixture | `software` |

---

## Edit 4 — `code-review.md` HARD CONSTRAINT points at the wrong subsection

Found while verifying STEP 13. The confirmation gate is **13c**
([code-review.md:338](../../.claude/commands/code-review.md), "### 13c. Confirm before posting");
**13b** is "Build the comment bodies" ([code-review.md:320](../../.claude/commands/code-review.md)).
The HARD CONSTRAINT that makes the gate mandatory names the wrong one, so a reader following the
cross-reference lands on body formatting instead of the `ask_user_question` that guards a public,
irreversible post.

**File:** `.claude/commands/code-review.md`, line 412.

Before:

```markdown
- **Posting is the one outward-facing action this command takes.** It requires the STEP 13b confirmation every time — never post because the review "looks done", because a previous run was approved, or because the user asked for a review. Reviewing and publishing a review are different requests.
```

After:

```markdown
- **Posting is the one outward-facing action this command takes.** It requires the STEP 13c confirmation every time — never post because the review "looks done", because a previous run was approved, or because the user asked for a review. Reviewing and publishing a review are different requests.
```

One character. `13b` → `13c`.

---

## Struck items — verified, no edit needed

### `/create-pr` template handling — **works; exercised during this research**

The original item asked for this path to be tested or declared unexercised. It has now been tested.
The STEP 5 detection script ([create-pr.md:99-152](../../.claude/commands/create-pr.md)) was extracted
verbatim, its `git rev-parse` line pointed at a fixture root, and run against nine layouts. All nine
behave as documented:

| fixture | result |
|---|---|
| no template anywhere | `TEMPLATE_FOUND: no` |
| `.github/pull_request_template.md` | found, contents echoed between the `---TEMPLATE_BEGIN/END---` markers |
| `.github/PULL_REQUEST_TEMPLATE.md` | found |
| `docs/pull_request_template.md` | found |
| root `PULL_REQUEST_TEMPLATE.md` | found |
| `.github/PULL_REQUEST_TEMPLATE/` with one `.md` | found, that file |
| `.github/PULL_REQUEST_TEMPLATE/` with two `.md` | `TEMPLATE_FOUND: no` + `TEMPLATE_NOTE` naming both, as designed |
| `.github/PULL_REQUEST_TEMPLATE/` empty | `TEMPLATE_FOUND: no`, loop continues correctly |
| single-file **and** directory both present | single-file wins, per GitHub's precedence |

What remains unexercised is STEP 6's "fill in EVERY section, do not invent, do not reorder"
([create-pr.md:161-171](../../.claude/commands/create-pr.md)) — that is model behaviour on a template
that only exists downstream, not code with a branch to fix. **No edit.**

### `GH_PATH=cli` branches — unverifiable here, and nothing suggests they are wrong

No `gh` binary exists in this container, so
[create-pr.md:28](../../.claude/commands/create-pr.md) and
[code-review.md:19](../../.claude/commands/code-review.md) both resolve to `mcp` and the `cli` arms
are dead. Every `cli` bash block does parse (`bash -n` clean, part of the 74). Confirming `gh pr
view --json`, `gh pr create`, and `gh api .../pulls/N/reviews` end-to-end needs a machine with `gh`
authenticated; it cannot be done from this task and there is no edit to make in the meantime.
**Struck — no edit, no acceptance criterion.**

### STEP 13 live posting — schemas verified against the real tools

The pending-review flow at [code-review.md:350-372](../../.claude/commands/code-review.md) was checked
against the actual MCP tool definitions available in this session, not against memory:

- `mcp__github__pull_request_review_write` — `method` enum contains `create`, `submit_pending`,
  `delete_pending`; `event` is optional; required set is `method, owner, repo, pullNumber`. The
  file's central claim — *omitting `event` on `create` is what makes it a pending review* — matches
  the tool's own description verbatim.
- `mcp__github__add_comment_to_pending_review` — required `owner, repo, pullNumber, path, body,
  subjectType`; optional `line, side, startLine, startSide`; `subjectType` enum is `FILE | LINE`;
  `side`/`startSide` enum is `LEFT | RIGHT`. Every parameter STEP 13d names exists and is spelled
  correctly, and the `subjectType: "FILE"` fallback at
  [code-review.md:365-368](../../.claude/commands/code-review.md) is a valid call shape.
- `mcp__github__list_pull_requests` — accepts `head`, `state: "all"`, and a `fields` array whose enum
  includes all four values [create-pr.md:42](../../.claude/commands/create-pr.md) passes
  (`number`, `title`, `state`, `html_url`).
- `mcp__github__pull_request_read` `method: "get"` and `mcp__github__create_pull_request`
  (`owner, repo, title, head, base` required, `body` optional) also match their call sites.

Only end-to-end execution against a live PR is unverified, and that is not something this task can
do. The one defect the review of STEP 13 did surface is **Edit 4**. **Struck otherwise.**

---

## Out of scope (recorded, not fixed here)

Shell state does not survive between fenced blocks — each is a separate invocation — yet
[create-pr.md:29](../../.claude/commands/create-pr.md) reads `$BRANCH` set back in the STEP 1 block
(line 10) and [create-pr.md:157-158](../../.claude/commands/create-pr.md) reads `${DEFAULT_BRANCH}`
set in the STEP 3 block, while [create-pr.md:78](../../.claude/commands/create-pr.md) re-derives
`BRANCH` precisely because it cannot rely on that. The files depend on the model carrying echoed
values forward by substitution, which mostly works and is stated explicitly elsewhere (e.g.
[code-review.md:410](../../.claude/commands/code-review.md) on `FLUX_BASE`). Systemic, not a loose
end — raise it as its own task if it ever bites.

---

## Definition of done

Four edits, in four files, all in `.claude/commands/`. No other file is touched, no file is created,
nothing is run that builds or tests Rust.

- [ ] `auto-pilot.md` carries the valid-commands sentence byte-identical to the other 13 files, plus
      the invoke-only sentence, inside `## HARD CONSTRAINT`
- [ ] `code-review.md` line 234 opens ` ```text `, not ` ```bash `
- [ ] The 15-line detection block is replaced in all four of `aug.md`, `exec.md`, `qa.md`,
      `code-review.md`, and the four remain byte-identical to each other
- [ ] `code-review.md` line 412 reads `STEP 13c`

Verify with exactly these three checks:

```bash
cd /home/user/cyrup
# 1. all 14 files carry the list exactly once
grep -c 'Valid `//flux` commands' .claude/commands/*.md | grep -v ':1$' || echo "OK: 14/14"

# 2. every bash fence parses — expect 74 blocks, 0 failures
python3 - <<'PY'
import os,re,glob,subprocess,tempfile
t=tempfile.mkdtemp(); n=0; bad=[]
for f in sorted(glob.glob('.claude/commands/*.md')):
    L=open(f).read().split('\n'); i=0
    while i<len(L):
        m=re.match(r'^```(\w*)\s*$',L[i])
        if m:
            j=i+1
            while j<len(L) and not re.match(r'^```\s*$',L[j]): j+=1
            if m.group(1)=='bash':
                n+=1; p=os.path.join(t,f'{n}.sh'); open(p,'w').write('\n'.join(L[i+1:j])+'\n')
                r=subprocess.run(['bash','-n',p],capture_output=True,text=True)
                if r.returncode: bad.append((f,i+2,r.stderr.strip()))
            i=j+1
        else: i+=1
print('blocks:',n,'failures:',len(bad))
for b in bad: print(' ',b)
PY

# 3. detection is repo-root anchored and the stale cross-reference is gone
grep -c 'ROOT=$(git rev-parse --show-toplevel' .claude/commands/{aug,exec,qa,code-review}.md
grep -n 'STEP 13b confirmation' .claude/commands/code-review.md || echo "OK: no stale 13b reference"
```

Expected: `OK: 14/14`; `blocks: 74 failures: 0`; four `:1` lines; `OK: no stale 13b reference`.

Do not delete `.flux/stack.env` as part of this task — it is per-machine scratch and currently holds
the correct value (`Rust`).
