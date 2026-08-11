export const meta = {
  name: 'parity-batch09-residual',
  description: 'Batch 9 residual — 2 coverage holes, injectOutputPathSystemPrompt wiring, 7 dead builtins, full-crate citation sweep',
  phases: [
    { title: 'Residual', detail: '3 agents on disjoint files' },
    { title: 'Verify', detail: 'serial mutation pass, exclusive tree' },
  ],
}

const BASE = `
Repo: /home/d0m17bw/workspace/cyrup   Crate: crates/cyrup-ext-subagents
Upstream: /home/d0m17bw/workspace/pi-subagents at **v0.43.0** (tag e76a256).
Read it with: git -C /home/d0m17bw/workspace/pi-subagents show v0.43.0:src/<path>
NEVER infer behaviour from a name or from this brief. Open the upstream file, read the whole
function, COUNT the lines before citing them.

THE GATE — the UNQUALIFIED feature flag, always:
  CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
  CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features test-fixtures
A qualified --features cyrup-ext-subagents/test-fixtures compiles OTHER crates' cfg-gated test
files to empty and hid a cross-crate break that had the real gate at exit=101.
Baseline: 5910 passed / 0 failed / 340 suites, clippy exit 0.

TREE DISCIPLINE:
- Your scratchpad is yours alone: <session scratchpad>/<YOUR-LABEL>/ — create it. Never write a
  backup to the scratchpad root.
- SURGICAL reverts only (edit the exact lines back). Never restore a whole file — concurrent agents
  clobbered each other that way earlier in this effort.
- NEVER git checkout. It destroyed 1,300 lines of uncommitted work here.
- Snapshot every file you touch BEFORE you touch it, including files that start clean; a restore
  that throws on an unsnapshotted file leaks a mutation into the tree.

RULES
- Fix, don't file. NEVER weaken, loosen or delete an assertion; print BEFORE/AFTER if you touch one.
- No-panic: clippy DENIES unwrap_used/expect_used/panic/indexing_slicing in non-test code, fires
  ONLY under clippy; tests/ are separate crates needing their own file-level #![allow].
- A subagent run is ALWAYS a real OS subprocess over NDJSON. Never make it in-process.
- Before testing any function, grep for its NON-TEST callers. If there are none it is dead code:
  wire it or delete it, and say which. A test calling it directly does not make it live.
`

phase('Residual')

const TASKS = [
  {
    key: 'holes',
    files: 'src/exec/task_intent.rs, src/background/runner_main.rs',
    body: `Two mutations survived a 46-mutation serial campaign at full crate scope. Both are real
coverage holes; close them.

1. \`strip_patterns\` (src/exec/task_intent.rs). Replacing the loop body with a \`break\` after the
   FIRST pattern that matches leaves the whole suite green. The existing test kills a loop truncated
   to its first *iteration*, but nothing exercises two INDEPENDENT patterns both matching one text.
   The verifier's concrete case:
     strip_patterns("please produce a report and attach the issue template here",
                    READ_ONLY_DELIVERABLE_PATTERNS)
   should strip patterns 1 AND 2. Confirm the expected result against upstream
   runs/shared/task-intent.ts, then pin it. Live callers: classify_task_mutation_intent /
   task_may_mutate (task_intent.rs:1041,1190,1235,1243).

2. \`background/runner_main.rs:2545-2548\`, the \`.filter(|_| self.artifact_config.enabled)\` term.
   Deleting it leaves the suite green because the only covering test passes \`artifacts_dir: None\`,
   which makes the filter unreachable. The foreground twin (extension.rs:1775) IS covered. Write the
   background-side test with a real artifacts_dir so \`artifacts: false\` is proven to suppress verify
   memo artifact writes.`,
  },
  {
    key: 'wiring',
    files: 'src/exec/output.rs, src/exec/mod.rs, src/extension.rs (skill recommendation fns)',
    body: `Two unfinished-wiring items. Neither may be left as a comment describing a gap.

1. \`exec/output.rs:29\` still documents \`injectOutputPathSystemPrompt\` as "ported but NOT yet
   wired". Upstream: runs/foreground/execution.ts:1443 and api/preflight.ts:313 — read BOTH call
   sites and note they are different surfaces. Wire it where upstream wires it. If the two upstream
   sites disagree about which surface carries the instruction, port both faithfully rather than
   picking one.
   Related and in scope: batch 9 found the capability-aware output instruction only reaches an
   \`OutputMode::FileOnly\` run, so a \`file-and-inline\` run with a configured output path gets no
   instruction. Check upstream and fix if it diverges.

2. \`build_proactive_skill_subagent_recommendation_lines\` and its 6 siblings have ZERO non-test
   callers — confirmed by grep against the \`mod tests\` boundary at extension.rs:10495. They were
   honestly reported as not-done. Finish them: find where upstream emits proactive skill
   recommendations and wire them there, or delete all seven if upstream has no such surface at
   v0.43.0. Decide from upstream, not from the code's shape, and say which you did and why.`,
  },
  {
    key: 'citations',
    files: 'doc comments and comments ONLY, crate-wide',
    body: `Finish the citation sweep. A previous pass audited only the 502 citations touched by
batch 9 and fixed 46; the crate carries roughly 5,000 more with the same defect class, and CLAUDE.md
makes this index the parity-audit mechanism — a wrong range sends the next auditor to unrelated code.

Method that worked: extract every \`<file>.ts:<line>\` / \`:<line>-<line>\` citation, resolve each
against the real v0.43.0 tree, and READ the cited lines. Single-line cites have been overwhelmingly
accurate; MULTI-LINE RANGES carry the drift, worst where upstream rewrote a function for v0.43.0 and
cyrup kept the v0.34.0 offsets. Automate the extraction and the bounds check; read the ones that
resolve in-bounds but may still point at the wrong function.

Known-wrong and explicitly NOT yet fixed (start here):
- subagent-executor.ts:1534-1541 / :1534 / :1535-1541 cited for validateExecutionInput's acceptance
  gate — really :1757-1762, function at :1736. Sites: extension.rs:7477, :7553,
  exec/acceptance.rs:735, background/runner_main.rs:2366,
  tests/subagent_persona_and_depth_integration.rs:1196.
- subagent-executor.ts:1684 cited for a ctx.model fallback — :1684 is canonicalizeAgentName.
- The \`@v0.35.0\`-tagged pair at extension.rs:4188 and :8590 (applySingleAgentLaunchDefaults,
  subagent-executor.ts:1585-1602 @v0.35.0; at v0.43.0 that function is at :1930). Check v0.35.0
  itself and say whether the tag rescues the number or disguises a third stale offset.
- The \`run-status.ts:369,375 @v0.34.0\` pair in run_status.rs:276,655 — same question.
- Four citations are bare basenames matching MULTIPLE upstream files (control-channel.ts → api/ vs
  runs/background/; types.ts → shared/ vs missions/ vs watchdog/). Fully qualify every ambiguous
  path crate-wide.

Report: how many you checked, how many were wrong, how many remain unchecked. Touch only comment
text — no assert condition, no code. If a citation turns out to be correct but the Rust beneath it
has DRIFTED from that upstream code, do not silently fix the comment: report it as a parity finding,
because that is a port bug wearing a correct citation.`,
  },
]

const SCHEMA = {
  type: 'object',
  required: ['items', 'testResult', 'clippyExit'],
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
          verdict: { type: 'string' },
          notDoneReason: { type: 'string' },
        },
      },
    },
    parityFindings: { type: 'array', items: { type: 'string' } },
    testResult: { type: 'string' },
    clippyExit: { type: 'string' },
    publicApiChanges: { type: 'array', items: { type: 'string' } },
  },
}

const done = (
  await parallel(
    TASKS.map((t) => () =>
      agent(
        `Close your assigned residual items from parity batch 9.

AREA: ${t.key}
YOUR FILES (other agents own the rest of the tree — stay inside these):
  ${t.files}

${t.body}

${BASE}

Do NOT run mutation tests; a serial verifier does that afterwards with exclusive tree access.`,
        { label: `residual:${t.key}`, phase: 'Residual', schema: SCHEMA },
      ),
    ),
  )
).filter(Boolean)

phase('Verify')

const verify = await agent(
  `You have EXCLUSIVE tree access — every other agent has finished.

Claims to verify:
${JSON.stringify(done, null, 2)}

${BASE}

1. MUTATION TEST every claimed behaviour, one at a time, restoring surgically and byte-comparing
   against a private snapshot after each. At least 15 mutations, weighted toward the two holes that
   survived the last campaign (strip_patterns' multi-pattern application, and the background
   artifact_config.enabled filter) and toward any newly wired code path.
2. For every "I wired it" claim, grep for the NON-TEST caller and check it sits above the
   \`mod tests\` boundary. For every "I deleted it" claim, confirm upstream really has no such
   surface at v0.43.0.
3. Spot-check 30 of the citation fixes by reading v0.43.0 yourself. Report the error rate. If the
   citation agent reported parity findings (correct citation, drifted Rust), verify each one.
4. Run the full gate 2x, once under CPU contention:
     for i in $(seq 1 $(nproc)); do (while :; do :; done) & done
     CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
     kill $(jobs -p)
   Check df -h / and df -h /tmp FIRST — / has hit 100% mid-run (ENOSPC silently truncates files;
   rm -rf target/debug/incremental is safe to free) and /tmp is a separate 16 GB tmpfs whose
   exhaustion makes ld die with SIGBUS, which the suite reports as test failures.
5. Confirm no assertion was weakened or deleted anywhere.

RESTORE the tree to the fixed state and say so explicitly. Then state plainly what in batch 9 is
STILL not at parity, with evidence — an unfinished item named plainly beats a clean-sounding
summary.`,
  { label: 'verify', phase: 'Verify', effort: 'high' },
)

return { done, verify }
