---
stage: new
status: done
updated: 2026-09-06 03:29
---

# SCOPE_12 — inspect RPC reads through the session index and replay

OBJECTIVE: port `background/inspect-rpc.ts` (443 LOC) — the RPC surface that reads a run's output.
It is the last consumer of the session-partitioned index, and it consumes the replay record too.

**Depends on SCOPE_3** (`wait_completions`) and **SCOPE_4** (`completion_replay`).

## SUBTASK1 — `background/inspect_rpc/`

**Ports:** `pi-subagents/src/runs/background/inspect-rpc.ts`

The session-bearing core:

```ts
function readResultOutput(resultsDir, sessionId, runId, stepIndex, trustedRoots,
                          stepAgent, now, trustedSessionFileRoot?) {          // :233
    const resultPath = resultPayloadPathForSessionRun(resultsDir, sessionId, runId);  // :234
    …
    const replay = readCompletionReplay(resultsDir, runId, { sessionId, now: nowMs }); // :247
    …
    && rawReplay.sessionId === sessionId                                       // :255
}
```

Three session uses in one function: resolving the payload through the index (`:234`), reading the
replay with a session filter (`:247`), and re-verifying the raw replay's session (`:255`). `:255`
is belt-and-braces over `:247`'s own OPTIONAL filter — port both, they guard different failure
modes (a filter not supplied vs a record that disagrees).

**`trustedRoots` / `trustedSessionFileRoot`:** a recorded `sessionFile` is data a *child* wrote, so
it is never dereferenced outside the trusted roots. cyrup has this discipline already in
`extension/executor/status.rs`'s `transcript_session_roots` — reuse it, do not invent a second
containment gate.

**Layout:** `inspect_rpc/{mod,request,read_output,respond}.rs`.

## SUBTASK2 — wire the RPC surface

**Where:** the extension's tool/RPC routing (`extension/tool/routing.rs`)

**Change:** register the inspect action so it routes to the new module.

## Tests

| test | pins |
|---|---|
| `inspect_resolves_output_through_the_session_index` | `:234` |
| `inspect_of_a_foreign_sessions_run_returns_nothing` | the partition holds at the RPC boundary |
| `inspect_falls_back_to_the_replay_record_after_cleanup` | `:247` — the SCOPE_4 dependency |
| `a_replay_record_with_a_mismatched_session_is_rejected` | `:255`, distinct from `:247` |
| `a_session_file_outside_the_trusted_roots_is_not_dereferenced` | the containment gate |
| `a_step_index_out_of_range_is_reported_not_panicked` | `stepIndex` bounds |

## Benchmarks

None. Inspect is a user-triggered read.

## Definition of done

- Inspect resolves output through `result_payload_path_for_session_run`, never by joining the
  public path directly.
- After cleanup, inspect still returns output from the replay record.
- Both replay session checks (`:247` filter, `:255` re-verification) are present.
- A child-recorded `sessionFile` outside the trusted roots is refused, reusing the existing gate.
- Tests pass; workspace 0 failed; clippy exit 0.

## Research notes

* Upstream: `pi-subagents/src/runs/background/inspect-rpc.ts` (HEAD `7fe9dee1`).
* Consumes: `result_index::result_payload_path_for_session_run` (landed),
  `completion_replay::read_completion_replay` (SCOPE_4).
* Existing containment gate to reuse: `extension/executor/status.rs`'s `transcript_session_roots`
  (`:431-438`) and `background/fleet_view.rs`'s containment check.
