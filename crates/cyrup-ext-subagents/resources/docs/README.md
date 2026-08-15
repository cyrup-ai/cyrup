# Subagents

Subagents delegate work to child `cyrup` processes, each with its own persona, model, tool set and
depth budget. A running subagent is a real OS process: cyrup re-executes its own binary as a child,
hands it a persona and a task, and streams the result back. Because the child is a separate process
with a separate context window, its exploration does not fill yours.

This is the packaged guide that ships inside the extension binary. Every page is embedded at compile
time, so what you are reading always describes the build you are running.

## Topics

Call `subagent({ action: "guide", topic: "<name>" })`, or `/subagents-guide <name>`.

| Topic | What it covers |
|---|---|
| `overview` | This page |
| `workflows` | Single, parallel, chain, prompt workflows, worktrees, forking |
| `agents` | Writing an agent file, every frontmatter key, where files live |
| `missions` | The durable mission store and the six `mission.*` verbs |
| `observability` | Fleet view, doctor, cost, run artifacts, the events log |
| `tool-reference` | Every `subagent` action and every tool parameter |
| `configuration` | `config.json`, `settings.json`, environment variables |
| `models` | Model resolution, fallback ladders, model scope policy |
| `watchdog` | The default-off supervision loop and its four verbs |
| `extension-api` | Host seams, the child protocol, and the control channel |

## Turning it on

```sh
CYRUP_SUBAGENTS=1 cyrup
```

Alternatively, create either config file and the extension arms itself with no environment variable:

- `~/.cyrup/agent/subagents/config.json` — for every project
- `<project>/.cyrup/subagents/config.json` — for one repository

An empty `{}` in either file is enough.

## The shape of a run

Children run in the **foreground** (you wait) or in the **background** (you keep working and collect
the result later). They can be strung into a **chain** where each step feeds the next, or fanned out
in **parallel** across a batch of tasks. A run can be isolated in its own git worktree so a child
editing files does not collide with your working tree.

Each child has a depth budget, so a subagent that spawns its own subagent eventually runs out of
room. The default ceiling is two levels, and a per-session spawn cap sits above it.

## The three surfaces

- The **`subagent` tool**, which the orchestrating model calls on its own.
- The **`wait` tool**, which blocks on a background run until it finishes.
- The **slash commands**, which are for you — `/run`, `/chain`, `/parallel`, `/run-chain`,
  `/subagents-fleet`, `/subagents-doctor` and the rest. See `workflows` and `observability`.

## Turning it off

Unset `CYRUP_SUBAGENTS` and remove both `config.json` files — either one is enough to keep the
extension armed. `cyrup --no-extensions` disables it for a single run along with everything else.
