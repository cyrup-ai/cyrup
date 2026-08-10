export const meta = {
  name: 'parity-batch09-closeout',
  description: 'Batch 9 closeout — unify the dual acceptance shapes, cover 33 untested behaviours, wire 5 dead-code "fixes", fix citations',
  phases: [
    { title: 'Unify', detail: 'one agent, exclusive tree: collapse model::* vs lattice' },
    { title: 'Cover', detail: '4 agents on disjoint files: tests + live wiring' },
    { title: 'Mutate', detail: 'ONE serial verifier with exclusive tree access' },
    { title: 'Close', detail: 'citation audit + full gate' },
  ],
}

const BASE = `
Repo: /home/d0m17bw/workspace/cyrup   Crate: crates/cyrup-ext-subagents
Upstream: /home/d0m17bw/workspace/pi-subagents at **v0.43.0**.
Read it with: git -C /home/d0m17bw/workspace/pi-subagents show v0.43.0:src/<path>
NEVER infer behaviour from a name, from this brief, or from cyrup's doc comments. Open the upstream
file, read the whole function, and COUNT the lines before citing them.

THE GATE — the feature flag is NOT optional:
  CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
  CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features test-fixtures
Use the UNQUALIFIED --features test-fixtures. A qualified form like
--features cyrup-ext-subagents/test-fixtures compiles other crates' cfg-gated test files to EMPTY
and hid a cross-crate break that had the real gate at exit=101 this batch.
Current baseline: 5855 passed / 0 failed / 336 suites, clippy exit 0.

TREE DISCIPLINE — read this, it went wrong this batch:
- Five agents ran mutation tests concurrently in this ONE working tree and restored whole files from
  .SAFE backups, clobbering each other's edits. Some results were unattributable.
- Your scratchpad is YOURS ALONE: /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<YOUR-AGENT-LABEL>/
  Create it. Never write a backup to the scratchpad root; 354 colliding .SAFE files are there now.
- Prefer a SURGICAL revert (edit the exact lines back) over a whole-file restore.
- NEVER git checkout. It destroyed 1,300 lines of uncommitted work earlier in this effort.

RULES
- Fix, don't file. "Blocked" has been wrong 3/3 times here; search for the CAPABILITY, not the
  identifier you first guessed.
- No-panic policy: clippy DENIES unwrap_used/expect_used/panic/indexing_slicing in non-test code and
  fires ONLY under clippy. tests/ are separate crates needing their own file-level #![allow].
- NEVER weaken, loosen or delete an assertion. Print BEFORE/AFTER for any existing test you touch.
- A subagent run is ALWAYS a real OS subprocess over NDJSON. Never make it in-process.
- Advertise-vs-dispatch: every schema value shown to the model needs a dispatch arm in the SAME
  change. Assert it by WALKING the schema, not by hardcoding a list.
- A pub signature change is invisible to per-package builds. Say so LOUDLY and run --workspace.
`

phase('Unify')

const unify = await agent(
  `You have EXCLUSIVE access to the working tree. No other agent is running. Do the structural fix
this batch deferred.

**The defect.** cyrup carries TWO parallel acceptance implementations: the \`model::*\` shapes and
the "lattice" shapes (\`AcceptanceLedger\` with status/detail/verify_results, and
\`VerifyCommandResult\`). Batch 9 added G78's \`evidence_status\` and G80's seven memoization evidence
fields (artifactPath, cacheKey, memoized, envKeys, envHash, workspaceState, artifactError) ONLY to
\`model::*\`. But \`model::evaluate_acceptance\` has exactly ONE production caller —
\`spawn/chain_graph.rs:1741\`, the dynamic-group gate — which passes \`file_output: None\` and
\`memo: None\`. The LIVE single-run path uses the lattice shapes, which carry none of those fields.

So in production, upstream's acceptance evidence is UNREACHABLE and \`artifactError\` is downgraded
to a \`tracing::debug!\`. Two gaps were reported closed while the user-visible behaviour did not
change. Upstream stamps these fields onto every entry of \`ledger.verifyRuns[]\` on BOTH its live
gates — read \`runs/shared/acceptance.ts\` around :1045-1078 and :3710-3752's cyrup counterpart, and
find upstream's actual single implementation.

**Your job.** Make upstream's evidence reach production. Upstream has ONE acceptance
implementation; the dual shape is cyrup's own accretion, not a port of anything. Collapse them, or
if a full collapse is genuinely too large for one change, make the LIVE path carry and emit the
full evidence and leave the two shapes converging rather than diverging — but say plainly which you
did and what remains.

Watch for public-serialization consequences: these shapes are serialized into session JSONL and
intercom result frames. Check cyrup-test-support's golden snapshots
(UPDATE_GOLDEN=1 regenerates) and say whether any wire shape changed.

Also fix, while you are in here:
- G78: \`model::evaluate_acceptance\` is missing two branches of upstream's report ladder
  (acceptance.ts:1251-1266) — \`needsReport = acceptanceRequiresChildReport(acceptance)\`, the
  \`reportOptional\` guard, and the non-rejecting arm.
- G78: two v0.43.0 additions inside \`validateAcceptanceInput\` were not ported — the
  duplicate-normalized-criterion-id check (upstream builds a \`criterionIds\` set over
  \`normalizedToken(gate.id)\`), and the second one you will find beside it.

${BASE}

Report: what you unified, what still diverges, every wire-shape change, and the full gate.`,
  { label: 'unify', phase: 'Unify', effort: 'high' },
)

phase('Cover')

const AREAS = [
  {
    key: 'acceptance',
    files: 'src/exec/acceptance.rs, src/discovery/chains.rs',
    body: `Cover the untested behaviours in the acceptance parser and verify-memoization. Each of
these survived a mutation with the whole suite GREEN, so each is currently unproven:
- G78: the checked rung APPENDING to runtimeChecks and not early-returning on failure — the
  headline behavioural change of the evaluateAcceptance rewrite, with no test at all.
- G78: \`review.required\` defaulting (acceptance.ts:1325, \`!== false\`) — a gate authored without
  \`required\` must still park the ledger at review-required.
- G78: resolveEffectiveAcceptance (acceptance.ts:389) for the \`review: false\` case — an explicit
  false must override an inferred required gate.
- G78: \`AcceptanceContract::to_resolved_config\`'s \`Reviewed|Rejected -> Checked\` mapping.
- G79: normalizedToken's SECOND \`/-+/g\` pass (acceptance.ts:514); the
  \`not-run|not-executed|skip|skipped\` and \`not-applicable|n-a|na|skip|skipped\` alias groups; and
  validateStringArrayField's non-blank tightening (acceptance.ts:827 added \`|| !item.trim()\`) —
  currently asserted only through a message string, not the rule.
- G80: the cache-key terms \`cwdRelative\`, \`timeoutMs\`, \`allowFailure\` can each be deleted with the
  suite green. \`allowFailure\` is behaviour-changing, not cosmetic.
- G80: the memo-hit re-stamp of id/command/cwd (acceptance.ts:1106) so a renamed criterion reports
  under its new name.
- G80: the LEFT \`(?:^|_)\` boundary of SENSITIVE_ENV_KEY_PATTERN — every negative probe in the
  existing test fails on the RIGHT boundary, so the left one is unproven. This is a secret-redaction
  boundary; treat it as security-relevant.
- G80 LIVE WIRING: every memo test builds a \`VerifyMemoContext\` by hand. The live wiring at
  exec/mod.rs:3417-3423, extension.rs:1773 and background/runner_main.rs:2545-2548 is untested.
  Test it at the live seam.`,
  },
  {
    key: 'output-intent',
    files: 'src/exec/output.rs, src/exec/completion_guard.rs, src/exec/mod.rs (build_task_text)',
    body: `- G82 DEAD CODE REPORTED AS A FIX: \`inject_single_output_instruction\` was reported as the
  live wiring for build_task_text, but it has no production caller. Either wire it to the live path
  or delete it — it must not stand as a reported fix. Read upstream
  runs/shared/single-output.ts:14-108 and match where pi actually injects.
- G82: \`select_acceptance_report_source\`'s two upstream rules (the authoritative primary/secondary
  swap, and "a primary defect is decisive, only a genuine miss falls through") — two independent
  mutations survive.
- G82: \`extract_child_written_output\`'s \`tool_name != "write"\` guard — deleting it leaves the
  suite green. The existing test cannot isolate it because its \`edit\` call carries a shape that
  fails for a different reason. Write one that isolates the guard.
- G82: the capability-aware instruction only reaches an \`OutputMode::FileOnly\` run, so a
  \`file-and-inline\` run with a configured output path gets no instruction. Check upstream and fix.
- G83: the sentence-initial \`implement\` mandate at offset 0 was reported FIXED — re-verify it
  against upstream REVIEWER_REQUIRED_EDIT_PATTERNS[2] and pin it.
- G83: \`then\` as a prohibition-object terminator in NO_EDIT_PROHIBITION_PATTERN; \`investigate\` and
  \`scout\` in RESEARCH_AGENT_PATTERNS — all deletable with the suite green.
- G83: \`strip_patterns\`' sequential-vs-merged application is not actually proven by the test
  written for it. Make the test discriminate.`,
  },
  {
    key: 'run-state',
    files: 'src/background/run_status.rs, src/background/runner_main.rs, src/tui/intercom.rs, src/exec/mod.rs',
    body: `- G104 DEAD CODE REPORTED AS A FIX: \`resolve_single_result_status\` was promoted to pub,
  rewritten and given a test, while its production caller was deleted in the same change. Wire it or
  delete it; say which and why.
- G104: \`SingleResult::process_signal\` is never asserted on the live path — replacing
  \`signal.startup.process_signal.clone()\` with \`None\` at src/exec/mod.rs:3178 leaves the suite
  green, and the field exists specifically to make that observable.
- G77: three claimed 'stopped' widenings have zero coverage, including \`finish_run\`'s synthesized
  child \`stopped: terminal_state == RunState::Stopped\` (runner_main.rs:3105). Find and cover all
  three.
- G77: STOP_MESSAGE's verbatim upstream text is pinned by no test.
- G77: \`control_stop\` was claimed to reproduce all four upstream refusal texts, but upstream's
  \`action === "stop"\` block (subagent-executor.ts:4771-4815) emits SEVEN distinct strings. Port the
  missing ones and pin them.`,
  },
  {
    key: 'discovery',
    files: 'src/discovery/*, src/registration/*, resources/skills/pi-subagents/SKILL.md',
    body: `- G97 LIVE WIRING: deleting the \`canonicalize_execution_params(...)\` call from
  \`SubagentTool::call\` — its ONLY production caller — leaves the crate suite fully green. Cover the
  live seam.
- G99: removing \`"oracle"\` from the read-only-agent alternation, and the FALLBACK_AGENT_ORDER
  change, both ship with zero coverage. Removing oracle is a real behaviour change (an \`oracle\`
  child stops inferring read-only acceptance).
- G101: BOTH halves of the project-root wiring are untested. The stated reason for changing
  \`discovery_dirs_config\`/\`discovery_config\` was that keying settings on \`cwd\` made
  projectRootResolution structurally unobservable — so make it observable and assert it.
- G101/G97: \`find_nearest_git_root\`'s linked-worktree fidelity (pi probes \`.git\` with
  \`fs.existsSync\`, true for a \`.git\` FILE, so a linked worktree counts; Rust's \`Path::exists\`
  differs in a way the implementer documented) — untested.
- Stale names/prose: \`src/registration/profiles.rs:1169\` is still
  \`build_profile_file_assigns_all_eight_builtins_to_their_tier\` while asserting 6.
  \`find_nearest_project_root\`'s doc still cites agents.ts:511-522 after a rewrite.
- SKILL.md's replacement prose was newly authored rather than ported. That file is injected into the
  MODEL'S PROMPT, so its wording is behaviour. Port upstream's actual text.`,
  },
]

const COVER_SCHEMA = {
  type: 'object',
  required: ['items', 'testResult', 'clippyExit'],
  properties: {
    items: {
      type: 'array',
      items: {
        type: 'object',
        required: ['id', 'status', 'whatChanged', 'testsAdded'],
        properties: {
          id: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'not-done'] },
          whatChanged: { type: 'string' },
          testsAdded: { type: 'array', items: { type: 'string' } },
          deadCodeVerdict: { type: 'string' },
          notDoneReason: { type: 'string' },
        },
      },
    },
    testResult: { type: 'string' },
    clippyExit: { type: 'string' },
    publicApiChanges: { type: 'array', items: { type: 'string' } },
    editedExistingTests: { type: 'array', items: { type: 'string' } },
  },
}

const covered = await parallel(
  AREAS.map((a) => () =>
    agent(
      `Close the untested behaviours and dead-code items in YOUR AREA ONLY.

AREA: ${a.key}
YOUR FILES (do not edit outside these; other agents own the rest):
  ${a.files}

${a.body}

${BASE}

DO NOT run mutation tests — a later serial agent does that with exclusive tree access. Your job is
to WRITE THE COVERING TESTS and fix the dead-code items. Every test must drive the LIVE path: before
writing one, grep for non-test callers of whatever you are calling, and if there are none, say so
and fix the wiring instead of testing a dead sibling.

Other agents are editing OTHER files in this same tree concurrently. Never restore a whole file.
Never run a workspace-wide revert. Stay in your files.`,
      { label: `cover:${a.key}`, phase: 'Cover', schema: COVER_SCHEMA },
    ),
  ),
)

phase('Mutate')

const mutate = await agent(
  `You have EXCLUSIVE access to the working tree — all other agents have finished. Nothing you see
is contaminated by a concurrent edit.

Batch 9 plus its closeout claims a large set of fixes. Earlier verification ran FIVE agents
concurrently in this one tree, restoring whole files from backups and clobbering each other, so a
number of their GREEN/RED results are unattributable. Redo it properly, serially.

What was claimed:
${JSON.stringify({ unify: String(unify).slice(0, 3000), covered: covered.filter(Boolean) }, null, 2)}

${BASE}

Do this:
1. MUTATION TEST every claimed behaviour, one at a time, restoring fully between each. Break it at
   its source — invert a condition, drop a branch, neuter a constant, reorder a priority. A mutation
   that leaves the suite GREEN means the behaviour is UNTESTED: report it. At least 25 mutations,
   weighted toward the items that were reported GREEN before and are now claimed covered.
2. For every "I wired the dead code" claim, verify the production caller EXISTS by grepping for
   non-test callers. A test calling it directly does not make it live.
3. Verify the unify phase actually made upstream's evidence reachable in production: drive a REAL
   subprocess run through the live single-run path and assert the evidence fields appear.
4. Run the suite 3+ times, including once under CPU contention
   (for i in $(seq 1 $(nproc)); do (while :; do :; done) & done ... kill $(jobs -p)), to catch
   flakiness. A flaky suite is how an untested fix hides behind a green run.
5. Confirm no assertion anywhere was weakened or deleted.

RESTORE THE TREE to the fixed state and say so explicitly. Use your own scratchpad subdirectory.
Prefer surgical reverts. NEVER git checkout.`,
  { label: 'mutate', phase: 'Mutate', effort: 'high' },
)

phase('Close')

const close = await agent(
  `Final closeout of parity batch 9. You have exclusive tree access.

**Citations are broken across the whole batch and that is a real defect.** CLAUDE.md: "Doc comments
carry the port provenance … it is how parity is audited." Every group's verifier found miscitations,
including: functions rewritten to v0.43.0 still carrying their v0.34.0 line offsets; roughly a dozen
citations stamped "@ v0.43.0" that do not match v0.43.0; a systematic 1-3 line drift on every
multi-line result-intercom.ts range; a range that runs past the end of its file; and a comment
citing background/subagent-runner.ts:879 for a gate that is actually \`settled = true;\` there.

Audit and FIX every upstream citation added or modified by batch 9 and its closeout. Method: for
each citation, open the upstream file at v0.43.0 and COUNT the lines. Single-line cites have been
accurate; multi-line RANGES are where the drift is. A wrong range sends the next auditor to
unrelated code and is worse than no citation. Report how many you checked and how many were wrong.

Also verify one specific false provenance claim was really corrected: a comment near
\`read_only_agent\` in src/exec/acceptance.rs asserted \`oracle\` "was ALWAYS in" the read-only
alternation "at v0.34.0". Check v0.34.0 yourself.

${BASE}

Then run the FULL gate and report it VERBATIM:
  df -h /   (check FIRST — the volume has hit 100% mid-run and ENOSPC silently truncates files;
             rm -rf target/debug/incremental is the safe thing to free)
  df -h /tmp   (separate 16 GB tmpfs; when it fills, ld dies with SIGBUS and unrelated crates fail
             to LINK, which the suite reports as test failures)
  CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
  CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features test-fixtures

Finally, state honestly what in batch 9 is STILL not at parity, with evidence. Do not smooth it
over — an unfinished item named plainly is worth more than a clean-sounding summary.`,
  { label: 'close', phase: 'Close', effort: 'high' },
)

return { unify: String(unify).slice(0, 8000), covered: covered.filter(Boolean), mutate, close }
