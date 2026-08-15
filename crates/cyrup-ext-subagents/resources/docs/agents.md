# Agents

An agent is one Markdown file. The frontmatter configures the child; the body is its system prompt.

```markdown
---
name: reviewer
description: Versatile review specialist for code diffs, plans, proposed solutions, codebase health, and PR/issue validation
tools: read, grep, find, ls, intercom
thinking: high
systemPromptMode: replace
inheritProjectContext: true
inheritSkills: false
---

You are a disciplined review subagent. Your job is to inspect, evaluate, and report findings with
evidence. You do not guess; you verify from the code, tests, docs, or requirements.
```

`name` and `description` are required. **A file missing either is skipped silently** — no warning, no
diagnostic, the agent never appears. If an agent you wrote is not listed by `/subagents-doctor`,
check those two keys first.

The frontmatter parser is a YAML subset: line-oriented, with one level of block nesting. It does not
understand two-level nesting. Folded (`>`, `>-`) and literal (`|`, `|-`) block scalars are both
supported for multi-line values.

## Frontmatter keys

| Key | Meaning |
|---|---|
| `name` | The agent's name; how you select it. Required |
| `description` | One-line summary shown in listings and to the orchestrating model. Required |
| `package` | Package namespace; makes the runtime name `<package>.<name>` |
| `alias`, `aliases` | Alternate names the agent also answers to |
| `tools` | Tool allowlist. Omit for all built-ins; an empty list means no tools |
| `model` | Model this agent runs on |
| `fallbackModels` | Models to try when the primary is unavailable |
| `thinking` | Thinking level for the child, for example `high` or `off` |
| `systemPromptMode` | `replace` (the body is the whole prompt) or `append` |
| `inheritProjectContext` | Whether the child inherits your project context |
| `inheritSkills` | Whether the child sees your skills |
| `defaultContext` | `fresh` (no inherited conversation) or `fork` (branch your session) |
| `skill`, `skills` | Skill names injected into the child's prompt at spawn |
| `extensions` | Extension allowlist for the child |
| `subagentOnlyExtensions` | Extensions visible to the child but not to you |
| `output` | Default output path and mode (`inline`, `fileAndInline`, `fileOnly`) |
| `defaultReads` | Files handed to the child when the call names none |
| `defaultProgress` | Whether progress is visible by default |
| `interactive` | Parsed and round-tripped; not enforced |
| `maxSubagentDepth` | Per-agent recursion ceiling |
| `completionGuard` | `false` disables the completion-mutation guard for this agent |
| `toolBudget` | Tool-call budget enforced inside the child |
| `turnBudget` | Assistant-turn budget applied when the call site omits one |
| `memory` | Persistent-memory scope folded into the child's prompt |
| `async` | Run in the background by default when the call site does not say |
| `timeoutMs` | Default run timeout when the call site does not say |

**Unrecognised keys are preserved, not dropped.** They round-trip through cyrup's own agent editing
intact. That is how a `permission:` block rides along in an agent file and gets enforced by the
permission system as its own policy layer.

## Turn budgets

`turnBudget` caps how many assistant turns a child may take. It is two numbers, written as **inline
JSON on one line** — the frontmatter parser hands the value to a JSON reader, so a nested YAML block
is not accepted here:

```markdown
turnBudget: {"maxTurns": 12, "graceTurns": 1}
```

`maxTurns` is a soft limit — reaching it earns the child a one-time note asking it to wrap up, and
kills nothing. `graceTurns` is how many further assistant turns are tolerated after that before the
child is aborted and its partial output returned. Omitting `graceTurns` means `1`.

Three layers supply it, in this order: the `subagent` tool call's own `turnBudget` parameter, then
this frontmatter key, then `subagents.turnBudget` in the extension `config.json`. The first one
present wins.

Validation is strict. `maxTurns` must be an integer at least 1, `graceTurns` an integer at least 0,
and any other key inside the object is rejected by name — including `hard`, which is not supported.
An invalid `turnBudget:` in an agent file **skips that whole agent file** rather than degrading to
unbudgeted. An invalid one in `config.json` is carried raw and surfaces at the call that would have
used it, not at load.

Enforcement is parent-side: the supervising process counts the child's assistant turns off its event
stream. It will not abort mid-tool-call — a run that hits the hard limit while tool work is in flight
ends as *termination-deferred* with a note saying so, rather than throwing the work away.

## Tool budgets

`toolBudget` caps tool calls rather than turns, and is enforced inside the child. It accepts a soft
threshold, a hard threshold, and is supplied by the same three layers as `turnBudget` plus the
`toolBudget` tool parameter.

## Where agent files live

Agents are collected from several directories and merged. In precedence order, lowest first:

| Tier | Directory |
|---|---|
| Builtin | the personas bundled in the binary |
| Package | the `agents` directories contributed by installed packages |
| Extra dirs | every path in `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` |
| User | `~/.cyrup/agents`, plus legacy `~/.agents` if it exists |
| Project | `<project>/.agents` if it exists, then `<project>/.cyrup/agents` |

Chains live alongside them in `~/.cyrup/chains` and `<project>/.cyrup/chains`.

**Watch the path.** Agent files go in `~/.cyrup/agents` — the `~/.cyrup` root, *not* the
`~/.cyrup/agent` agent directory that holds `settings.json` and the subagents `config.json`. The two
paths differ by one directory segment and by an `s`.

Set `CYRUP_HOME` to relocate the `~/.cyrup` root that subagents uses.

The project root is the nearest ancestor directory containing `.cyrup/` or `.agents/`, unless
`subagents.projectRootResolution` moves it to the git root. Anything under a path segment named
`skills` is excluded from agent discovery.

**Discovery re-runs on every call.** Drop a new agent file in and the next `/run` picks it up — no
restart, no reload command.

## Managing agents from the tool

`list`, `get`, `create`, `update`, `delete`, `eject`, `disable`, `enable` and `reset` are all
`subagent` actions. `eject` copies a builtin persona into your user directory so you can edit it;
`disable`/`enable`/`reset` write per-agent overrides into the scope's `settings.json` rather than
touching the agent file.

## Bundled personas

`delegate`, `oracle`, `researcher`, `reviewer`, `scout` and `worker` ship inside the binary. Eject
any of them to take ownership of the text.
