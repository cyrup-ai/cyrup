# Workflows

Four execution shapes reach the same machinery: single, parallel, chain, and prompt workflows.

## Single

One agent, one task.

```
subagent({ agent: "reviewer", task: "review the changes on this branch" })
```

```sh
/run reviewer review the changes on this branch
```

Add `async: true` (or `--bg`) to detach the run and collect it later with the `wait` tool or
`action: "status"`.

## Parallel

An array of `{agent, task}` pairs fanned out at once.

```
subagent({ tasks: [
  { agent: "reviewer", task: "review src/" },
  { agent: "scout",    task: "map the call graph" }
] })
```

```sh
/parallel reviewer "review src/" -> scout "map the call graph"
```

`parallel.maxTasks` caps how many tasks one fan-out may carry and `parallel.concurrency` caps how
many of them run at once. `globalConcurrencyLimit` sits above both.

## Chain

Steps run in order and each step's output is available to the next.

```
subagent({ chain: [
  { agent: "scout",  task: "find every caller", output: "callers.md" },
  { agent: "worker", task: "update them", reads: ["callers.md"] }
] })
```

```sh
/chain scout "find every caller" -> worker "update them"
```

A step may declare `reads` (files handed to the child as a `[Read from: …]` instruction) and
`output` (a `[Write to: …]` instruction). Both resolve against the chain's run directory unless the
path is absolute; `~` and `~/x` expand to your home directory, and `~user/` deliberately does not.
A `reads` path that does not exist is filtered out rather than being handed to the child as a
missing file.

Chain runs are addressed by run id. `append-step` adds a step to a chain that is still running.

Named chains live in `~/.cyrup/chains` and `<project>/.cyrup/chains` and run with
`/run-chain <chainName> -- <task>`.

## Prompt workflows

A prompt workflow is a saved prompt (or a sequence of them) run through a subagent.

```sh
/prompt-workflow <name> [args] [--fork|--fresh] [--worktree] [--bg] [--subagent <agent>]
/chain-prompts prompt-a -> prompt-b -- args
```

The bundled prompts are `gather-context-and-clarify`, `parallel-cleanup`, `parallel-research`,
`parallel-review` and `review-loop`.

## Fresh vs forked context

`defaultContext: fresh` starts the child with no inherited conversation — the default, and the
reason a subagent's exploration does not cost you context. `fork` branches your current session into
the child, so it starts with everything you have said so far. `--fork` on a slash command forces the
fork for that one call.

A forked child gets boundary instructions telling it that it is a child and that the parent owns
orchestration, so it does not read your prior delegation instructions as its own.

## Worktrees

`worktree: true` runs the children in a dedicated git worktree so file edits do not collide with
your working tree. `worktreeBaseDir` chooses where those worktrees are created, and
`worktreeSetupHook` names a script run once per worktree group before any child starts, bounded by
`worktreeSetupHookTimeoutMs` (30000 ms by default).

## Depth and spawn budgets

`maxSubagentDepth` (default 2) is the recursion ceiling: a child at the ceiling cannot spawn its own
children. `maxSubagentSpawnsPerSession` (default 40) caps total launches per session. An exhausted
spawn cap is not terminal — `action: "grant-spawn-budget"` adds launches from the root interactive
session, behind an explicit confirmation, and can never exceed the originally configured cap.

## Steering a live run

`action: "steer"` queues non-terminal guidance for a still-live background child without
interrupting it. `mode` selects delivery: `steer` interrupts at the next safe point (the default),
`follow_up` waits for the next turn boundary, and `auto` follows up mid-turn but delivers
immediately between turns. The tool answers with the child's own acknowledgment where one arrives in
time, so a dropped steer is distinguishable from a delivered one.

`action: "resume"` is the heavier sibling: it interrupts the child and delivers a follow-up, or
revives a finished run from its persisted transcript.

`action: "interrupt"` and `action: "stop"` end a run. `action: "dismiss"` clears a recovered
workflow whose runner process is gone but whose status still says running.
