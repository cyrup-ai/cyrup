export const meta = {
  name: 'parity-batch10-closeout',
  description: 'Batch 10 closeout — 5 blockers, the $HOME mission-index leak, ~40 dead-code items, 54 untested behaviours',
  phases: [
    { title: 'Fix', detail: '4 agents on disjoint areas' },
    { title: 'Verify', detail: 'ONE serial mutation verifier, exclusive tree' },
  ],
}

const BASE = `
Repo: /home/d0m17bw/workspace/cyrup   Crate: crates/cyrup-ext-subagents
Upstream: /home/d0m17bw/workspace/pi-subagents at **v0.43.0** (tag e76a256).
Read with: git -C /home/d0m17bw/workspace/pi-subagents show v0.43.0:src/<path>
NEVER infer behaviour from a name, from this brief, or from cyrup's doc comments. Open the upstream
file, read the whole function, COUNT the lines before citing them.

THE GATE — the UNQUALIFIED feature flag:
  CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
  CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features test-fixtures
Baseline: 6431 passed / 0 failed / 8 ignored / 342 suites, clippy exit 0.

**THE DEFINING PROBLEM OF THIS BATCH IS DEAD CODE.** Batch 10 ported three subtrees and left ~40
items with ZERO production callers, including an entire 772-line module and the whole interactive
half of the fleet UI. The suite is green because new tests call the new code directly. A test
calling a function does NOT make it live.

So before you claim ANY item done: grep for a NON-TEST caller and confirm it sits above the
'mod tests' boundary. Report the caller's file:line. If there is none, WIRE IT — do not test it, do
not document it as pending, and do not delete it unless upstream genuinely has no such surface at
v0.43.0 (check, then say which and why).

OTHER RULES, each of which cost real time earlier:
- No-panic: clippy DENIES unwrap_used/expect_used/panic/indexing_slicing in non-test code and fires
  ONLY under clippy. Opt-outs are PER-LINT — the usual trio does NOT cover 'panic'. Check clippy's
  exit code directly, never through a pipe.
- NEVER weaken, loosen or delete an assertion. Print BEFORE/AFTER for any test you touch.
- Temp state: use tempfile::TempDir. NEVER write to \$HOME or any real user directory from a test.
  TempDir does not Deref to Path, and 'tmp().join("x")' drops the guard into a temporary, deleting
  the dir before the test uses it.
- A subagent run is ALWAYS a real OS subprocess over NDJSON. Never make it in-process.
- CLAUDE.md is a search index, NOT an oracle — it has been stale repeatedly. Verify against code.
- Your scratchpad is yours alone: <session scratchpad>/<YOUR-LABEL>/. Surgical reverts only. Never
  git checkout. Do NOT run heavy I/O while a suite runs — that produced two phantom failures.
`

phase('Fix')

const TASKS = [
  {
    key: 'home-leak',
    files: 'src/missions/store.rs and mission test files',
    body: `**URGENT — tests write into the developer's REAL application config directory.**

\`resolve_mission_store_location\` (store.rs:678-684) defaults \`globalIndexDir\` to
\`agent_dir()/missions/index\` = \`$HOME/.cyrup/agent/missions/index\`. Mission tests inherit that
default, so a workspace gate run writes ~78 index pointer files into live user config. 7,942 had
accumulated in one day; every one is a dangling pointer whose \`projectRoot\` is a deleted tempdir.
\`~/.cyrup/agent\` also holds models-store.json, settings.json and sessions/ — real state.

This is the /tmp leak class pointed at \$HOME, which is why the earlier sweep missed it.

Fix: every mission test must point globalIndexDir at a \`tempfile::TempDir\`. Read upstream to see
what pi does for the equivalent default and whether the PRODUCTION default is even right — the
default itself may be faithful while only the tests are wrong; say which.

Then verify by COUNTING, not by a green suite: note the file count in the index dir, run the full
workspace suite, re-count. The delta must be ZERO. Report both numbers.

Audit for the same class while you are here: grep the crate for any test path that resolves to
\`agent_dir()\`, \`home_dir()\`, \`dirs::\`, or a literal '~' without a TempDir override. Report
everything you find, fixed or not.`,
  },
  {
    key: 'watchdog-blockers',
    files: 'src/extension.rs (watchdog wiring), src/prompt_runtime.rs, src/watchdog/{register_main,review,turn_delta,permission_arbiter}.rs',
    body: `Four blockers/majors, all in watchdog:

1. **turn_end tool results are shaped without a \`role\`, so every one is silently discarded from the
   delta the watchdog reviews.** The wiring builds
   \`json!({"type":"turn_end","message":message,"toolResults":tool_results})\` from
   \`Vec<cyrup_agent::ToolResultMessage>\`. Read what upstream's consumer expects and what cyrup's
   struct actually serializes; make the shapes match. Sites: extension.rs HostEvent::TurnEnd arm and
   prompt_runtime.rs:1304.
2. **The CHILD watchdog is registered with no review function** — prompt_runtime.rs:1536 passes
   \`None\` for review, so the child runs \`InertWatchdogReview\`. Read upstream register-child.ts and
   wire the real review.
3. **The production review can never resolve a configured model.** \`register_main_watchdog\` binds
   \`BuiltinWatchdogModelRegistry::new(None)\` (register_main.rs:108) and the review is constructed
   without the live session model or thinking level, so \`has_configured_auth\` is false and the
   DEFAULT config resolves nothing. Wire the live session model/thinking through.
4. **permission_arbiter.rs is 772 lines with no production caller**, and the stated reason for
   leaving it dead cites the wrong crate. Determine where upstream invokes it, wire it, and correct
   the claim. Note cyrup's permission gate comes from the pi-permission-system port — check how the
   two actually meet rather than assuming.

Also fix: \`redact_secret_values\` drops the upstream regex's case-insensitivity
(permissions.ts:15 ends \`/gi\`), under-redacting real credentials. That is a security boundary.

And: tests/watchdog_wiring.rs:496-515 claims in its module doc that "nothing here calls a watchdog
method directly", then does exactly that in \`a_child_...\` — so the one test claiming to prove
turn_end reaches the child bypasses the event path. Make it drive the real path.`,
  },
  {
    key: 'fleet-dead',
    files: 'src/tui/fleet*.rs, src/extension.rs (show_fleet, fleet_state)',
    body: `**The entire interactive half of both fleet components has zero production callers.**
\`show_fleet\` (extension.rs:9665-9731) constructs the component, calls \`render(100, now)\` ONCE,
flattens it with \`lines_text\`, and drops it. Nothing ever handles input. Dead:
\`handle_input\`, \`finish_action\`, \`set_terminal_rows\` (so terminal_rows is permanently 32),
and ~18 more.

Wire the component into the real TUI event loop so input, selection and scrolling work, as upstream
does. If cyrup's TUI genuinely has no seam for an extension-owned interactive component, BUILD the
seam — that is the port, not an obstacle to it.

Two more, both real divergences:
- \`fleet_transcript.rs\`: the [CYRUP-DELTA] claim that "cyrup NDJSON tags map onto identical
  semantics" is a TAG rename only. Against the artifact cyrup actually writes, the transcript pane
  carries no conversation and no tool content. Make it render real content.
- \`fleet.rs:472-509 collect_fleet_history\` omits pi's session filter, and the doc comment's
  justification is factually wrong — pi passes sessionId into listAsyncRuns and async-status.ts:432
  drops every on-disk run whose session differs. Port the filter, fix the comment.
- extension.rs:3442 \`fleet_state\` feeds the model almost entirely empty defaults, so most of what
  was ported renders as absent in production. Supply the real data.

**Every fleet test flattens Vec<Line> to a string via lines_text, so ZERO style assertions exist
across five modules** — characters matched while colors did not, and that shipped a visible bug
earlier in this effort. Add style assertions over painted cells for anything colour-carrying.`,
  },
  {
    key: 'missions-dead',
    files: 'src/missions/*.rs, src/extension.rs (mission wiring)',
    body: `Seven dead-code items plus real defects:

- \`missions::workflow_state::create_mission_workflow_state\` has NO production caller, and
  \`MissionWorkflowState::{get,set,path}\` are transitively dead through it. Wire or delete with a
  reason from upstream.
- \`artifacts_for_result\`/\`has_structured_output\` (lifecycle.rs:375-424) read five details keys
  cyrup's own producer never writes (\`artifactPaths.outputPath\`, \`savedOutputPath\`,
  \`transcriptPath\`, \`structuredOutputPath\`, \`parallelHandoff.path\`) — dead in production,
  reachable only from hand-built test JSON. Either the producer must write them or the reader is
  wrong; read upstream and decide, don't paper over it.
- \`ready_action_from_value\` descends objects in ALPHABETICAL key order because
  \`serde_json::Map\` is a BTreeMap here, where upstream goal-driver.ts:81 uses JS insertion order.
  The precedence it claims to pin is untested. Fix the ordering (preserve_order feature or an
  explicit key sequence) and pin it.
- Both "Mission tracking unavailable" degradation paths (extension.rs:8241 and :8367) have zero
  coverage, and the post-launch one SILENTLY DISCARDS the run's entire result. Cover both; a
  discarded result is a bug regardless of coverage.`,
  },
]

const SCHEMA = {
  type: 'object',
  required: ['items', 'testResult', 'clippyExit', 'wiring'],
  properties: {
    items: {
      type: 'array',
      items: {
        type: 'object',
        required: ['id', 'status', 'whatChanged'],
        properties: {
          id: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'not-done'] },
          whatChanged: { type: 'string' },
          testsAdded: { type: 'array', items: { type: 'string' } },
          notDoneReason: { type: 'string' },
        },
      },
    },
    wiring: { type: 'array', description: 'each formerly-dead symbol with its NEW non-test caller file:line', items: { type: 'string' } },
    deletedInstead: { type: 'array', items: { type: 'string' } },
    testResult: { type: 'string' },
    clippyExit: { type: 'string' },
    leakCounts: { type: 'string', description: 'home-leak group only: index file count before and after a full suite run' },
  },
}

const done = (
  await parallel(
    TASKS.map((t) => () =>
      agent(
        `Close your assigned batch-10 defects. AREA: ${t.key}
YOUR FILES (other agents own the rest — stay inside these):
  ${t.files}

${t.body}

${BASE}

Do NOT run mutation tests; a serial verifier does that afterwards with exclusive tree access.`,
        { label: `fix:${t.key}`, phase: 'Fix', schema: SCHEMA },
      ),
    ),
  )
).filter(Boolean)

phase('Verify')

const verify = await agent(
  `You have EXCLUSIVE tree access — all other agents have finished.

Claims to verify:
${JSON.stringify(done, null, 2)}

${BASE}

1. **Reachability audit FIRST.** Batch 10's defining defect was ~40 items with no production caller.
   For every symbol claimed wired, grep for its non-test caller and confirm it is above the
   'mod tests' boundary. Then sweep the three new subtrees (watchdog/, missions/, tui/fleet*) for
   ANY remaining pub item with zero non-test callers and list them all — the fix agents were given a
   partial list, and a complete one is the deliverable.
2. **Verify the \$HOME leak is actually fixed by counting**: record the file count in
   ~/.cyrup/agent/missions/index, run the FULL workspace suite, re-count. Delta must be ZERO. Also
   confirm no test writes anywhere else under \$HOME.
3. MUTATION TEST serially, one at a time, restoring surgically and byte-comparing against a private
   snapshot after each. At least 25, weighted toward the 5 blockers and every newly wired path. For
   each survivor decide EQUIVALENT (prove it) vs real gap.
4. Run the suite twice, once under CPU contention.
5. Confirm no assertion was weakened or deleted.

RESTORE the tree and say so. Then state plainly what remains unported or unwired, with evidence.`,
  { label: 'verify', phase: 'Verify', effort: 'high' },
)

return { done, verify }
