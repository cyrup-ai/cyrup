# Extension API

This page describes the seams the subagents extension uses and exposes. It is for people writing
against it, not for people running it.

## The two extensions

The crate registers **two** extensions that never coexist in one process:

| Id | Where | What it does |
|---|---|---|
| `subagents` | the orchestrator session | Registers the `subagent` and `wait` tools, the slash commands, the fleet widget and the watchdog |
| `subagent-prompt-runtime` | inside a spawned child | Registers the child's structured-output tool, the steering inbox, the tool-budget enforcer and the child watchdog |

A plain child gets only the second; a root orchestrator gets only the first.

## Host seams

The extension is a native extension and reaches the host through `cyrup_ext::host::HostServices`:

| Seam | Used for |
|---|---|
| `inject_message` | Delivering a steer or a completion notice into a live session |
| `set_widget` | The under-editor fleet widget — three arguments; `lines: None` removes it |
| `session_id` | Scoping runs and the `wait` tool to the current session |
| `confirm` | The authority-policy confirmation for gated actions |

`NativeExtension::set_host_services` is how the backend is bound. It is late-bound: code that needs
it must degrade rather than assume it is present.

## The parent/child protocol

A child is a real OS process. Everything the parent tells it at spawn time crosses the boundary as
argv or environment:

| Carried as | Examples |
|---|---|
| argv | the persona (`--system-prompt <path>`), the model, the task |
| env | run ids, depth, capability tokens, inbox paths, budgets |

Persona text is spilled to a `0600` file in the run's scratch directory rather than passed inline, so
it does not appear in `/proc/<pid>/cmdline` and cannot overflow the argv limit.

Encoded env payloads — the tool budget, the capability ceiling, the permission policy — are
monotonic: a child can only ever tighten them, never widen them, across a re-exec.

## The control channel

`<run_dir>/control/` is a filesystem inbox the parent writes and the detached runner drains:

| Path | Meaning |
|---|---|
| `interrupt.json` | Interrupt this run |
| `stop.json` | Stop this run |
| `timeout.json` | Deadline reached |
| `steer-requests/` | Run-level queue of steering requests |
| `steer-targets/<index>/` | Per-child inbox the runner routes an accepted request into |
| `steer-capabilities/<index>.json` | Whether that child can be steered at all, and its pid |
| `steer-acks/<index>/` | Per-request acknowledgment the child writes back |

Every record is written atomically (temp file plus rename) and carries a `protocolVersion`.

Steer requests are a **queue**, not a flag: several may be outstanding, and they are consumed in
`(ts, id)` order so two corrections typed in quick succession arrive the way they were written.

## Run records

| File | Written by | Meaning |
|---|---|---|
| `status.json` | the runner | The authoritative run record |
| `events.jsonl` | the runner | Append-only event log, capped at 50 MB |
| `result.json` | the runner | The terminal result, written once |

The parent never treats an in-memory event as authoritative: every wake re-reads the run tree from
disk through the same reconciliation gate every control action uses.

## Events

The extension consumes host events (`message_start`, `message_update`, `message_end`,
`tool_execution_start`, `tool_execution_end`, `turn_start`, `turn_end`, `session_start`,
`session_shutdown`) and
publishes custom messages back into the session under these types:

- `subagent-notify` — a background run completed
- `subagent-slash-result`, `subagent-slash-text-result` — slash-command output
- `subagent_control_notice`, `subagent-control`, `subagent-control-notice` — control-path notices
- `subagent-orchestration-instructions` — parent-only orchestration text

All of these are **parent-only**: a forked child strips them from inherited history, because a child
that reads them reads itself as the orchestrator.

## Extending discovery

`CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` adds read-only agent directories at the lowest user tier. A
package that declares an `agents = [...]` manifest entry has its personas discovered at package
scope. Project-scope packages are skipped until a project-trust decision is available — this crate
never silently trusts a project's installed packages.
