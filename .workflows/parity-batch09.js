export const meta = {
  name: 'parity-batch09',
  description: 'Parity batch 9 — subagents acceptance, output and state model (G78,G79,G80,G82,G83,G77+G104,G97,G99,G101)',
  phases: [
    { title: 'Implement', detail: '5 groups across acceptance / run-state / discovery' },
    { title: 'Verify', detail: 'adversarial mutation testing per group' },
    { title: 'Sweep', detail: 'cross-cutting reviewer over the whole batch' },
  ],
}

const UPSTREAM = `
Crate: /home/d0m17bw/workspace/cyrup/crates/cyrup-ext-subagents
Upstream: /home/d0m17bw/workspace/pi-subagents at tag **v0.43.0**.
Read upstream with: git -C /home/d0m17bw/workspace/pi-subagents show v0.43.0:src/<path>
NEVER infer behaviour from a name, from this brief, or from cyrup's own doc comments. Open the
upstream file, read the surrounding function, and quote file:line for every claim you make.

THE GATE (both required, always with the feature flag):
  CARGO_INCREMENTAL=0 cargo test -p cyrup-ext-subagents --features test-fixtures --no-fail-fast
  CARGO_INCREMENTAL=0 cargo clippy -p cyrup-ext-subagents --all-targets --features test-fixtures
Two [[bin]] targets carry required-features=["test-fixtures"]; without the flag every test that
drives a real subagent subprocess silently does not build. A "0 failed" without it is meaningless.

RULES
- Fix, don't file. "Blocked" has been wrong 3 times out of 3 in this effort. Search for the
  CAPABILITY, not the identifier you first guessed — the seam usually exists under another name.
- No-panic policy: clippy DENIES unwrap_used/expect_used/panic/indexing_slicing in non-test code
  and fires ONLY under clippy. tests/ are separate crates needing their own file-level #![allow].
- NEVER weaken, delete or loosen an assertion. If you touch an existing test, print BEFORE/AFTER.
- A subagent run is ALWAYS a real OS subprocess re-exec over NDJSON with SIGINT->SIGTERM->SIGKILL.
  Never simplify it to in-process calls.
- Advertise-vs-dispatch invariant (extension.rs:10036-10041): every schema value you advertise to
  the model needs a dispatch arm in the SAME change.
- A pub signature change is invisible to per-package builds — say so LOUDLY and run --workspace.
- Backups: cp <file> to the scratchpad as <unique>.SAFE. NEVER git checkout.
`

const GROUPS = [
  {
    key: 'acceptance-status',
    title: 'G78 + G79 — acceptance status model and report parsing',
    body: `G78 (partial->done): 'reviewed' is no longer requestable, and status is distinct from
evidenceStatus. cyrup: src/exec/acceptance.rs:82-123,5318-5350. Upstream:
runs/shared/acceptance.ts:28-36,181-196,1302-1352. NOTE the advertise-vs-dispatch invariant: the
acceptance enum advertised to the model still offers none/verified/reviewed, and v0.43 REJECTS
'reviewed'. G78 must land before G105 narrows the enum, so get the status model right here.

G79 (partial->done): acceptance report parsing — status aliases, file-output as a report source,
and fenced-block recovery. cyrup: src/exec/acceptance.rs:4018-4043,4390-4470. Upstream:
runs/shared/acceptance.ts:484-770,972-978.`,
  },
  {
    key: 'verify-memo',
    title: 'G80 — verify-command workspace memoization + secret redaction',
    body: `Absent. cyrup: src/exec/acceptance.rs:3299-3312. Upstream:
runs/shared/acceptance.ts:974-1130. Port both halves: the per-workspace memoization of verify
command results AND the secret redaction applied to captured command output. Redaction is a
security boundary — verify command output can carry tokens into a transcript. Get the redaction
patterns from upstream verbatim; do not invent your own set.`,
  },
  {
    key: 'stopped-state',
    title: 'G77 + G104 — `stopped` as a first-class terminal state, and result framing',
    body: `Absent, large, spans runs + registration. cyrup: src/background/run_status.rs:46-53 and
src/tui/intercom.rs:117-125. Upstream: runs/foreground/async-stop-action.ts and
intercom/result-intercom.ts:20-52.

'stopped' must be terminal in its own right — NOT an alias for failed or cancelled. Trace every
match over the status enum after you add the variant; a non-exhaustive match that compiles because
of a catch-all arm is exactly how this gap survives. Grep for every site that maps status to a
user-visible string, to an exit code, and to an intercom result frame.`,
  },
  {
    key: 'authorship-intent',
    title: 'G82 + G83 — authorship from the child, and task mutation-intent',
    body: `G82 (absent): authorship derived from the child's OWN successful write, plus the
read-only instruction. cyrup: src/exec/output.rs:781-810,964-989. Upstream:
runs/shared/single-output.ts:14-108.

G83 (partial): the task mutation-intent classifier — scoped vs blanket intent, taskMayMutate.
cyrup: src/exec/completion_guard.rs:530-568. Upstream: runs/shared/task-intent.ts:39-178. This is
a classifier: port its ACTUAL rules and their order, and table-test them against upstream's own
cases. A classifier that is 90% right is a bug generator.`,
  },
  {
    key: 'discovery',
    title: 'G97 + G99 + G101 — aliases, builtin roster, agent defaults',
    body: `G97 (absent): agent aliases plus alias-aware resolution AND the ambiguity error when an
alias is not unique. cyrup: src/discovery/types.rs:609-687, frontmatter.rs:68-90. Upstream:
agents/agents.ts:495,511-521.

G99 (absent): builtin roster changes — drop planner and context-builder, add advisor, re-tier the
profiles. cyrup: src/discovery/management.rs:1210, registration/profiles.rs:354. Upstream:
agents/agents.ts:38-45 and profiles.ts:263,405-409.

G101 (absent): defaultThinking, defaultExtensions, projectRootResolution. cyrup:
src/discovery/types.rs:505-528, discovery/mod.rs:181. Upstream: agents/agents.ts:161-169,640-673,
945-1000.`,
  },
]

const IMPL_SCHEMA = {
  type: 'object',
  required: ['items', 'testResult', 'clippyExit', 'publicApiChanges'],
  properties: {
    items: {
      type: 'array',
      items: {
        type: 'object',
        required: ['id', 'status', 'whatChanged', 'upstreamCitations', 'testsAdded'],
        properties: {
          id: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'not-done'] },
          whatChanged: { type: 'string' },
          upstreamCitations: { type: 'array', items: { type: 'string' } },
          testsAdded: { type: 'array', items: { type: 'string' } },
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

const VERIFY_SCHEMA = {
  type: 'object',
  required: ['mutations', 'findings', 'verdict'],
  properties: {
    mutations: {
      type: 'array',
      items: {
        type: 'object',
        required: ['description', 'result'],
        properties: {
          description: { type: 'string' },
          result: { type: 'string', enum: ['RED', 'GREEN'] },
          meaning: { type: 'string' },
        },
      },
    },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        required: ['severity', 'item', 'problem', 'evidence'],
        properties: {
          severity: { type: 'string', enum: ['blocker', 'major', 'minor'] },
          item: { type: 'string' },
          problem: { type: 'string' },
          evidence: { type: 'string' },
        },
      },
    },
    treeRestored: { type: 'boolean' },
    verdict: { type: 'string' },
  },
}

phase('Implement')

const results = await pipeline(
  GROUPS,
  (g) =>
    agent(
      `Port these gaps to behavioural parity with pi-subagents v0.43.0.

${g.title}

${g.body}

${UPSTREAM}

Deliver a real, complete port of every item — not the easy half. For each item, add tests that
drive the LIVE path (the one with production callers), not a convenient sibling. If an item turns
out to touch a dead-code path with no non-test callers, say so explicitly rather than reporting it
as a fix.

Report the full test result line WITH --features test-fixtures, and the clippy exit code.`,
      { label: `impl:${g.key}`, phase: 'Implement', schema: IMPL_SCHEMA },
    ),
  (impl, g) =>
    agent(
      `ADVERSARIAL VERIFICATION of a just-completed port. Your job is to prove the work WRONG.

Group: ${g.title}
${g.body}

The implementer reported:
${JSON.stringify(impl, null, 2)}

${UPSTREAM}

Do all of this:

1. MUTATION TEST every claimed fix. Break the behaviour at its source — invert a condition, drop a
   branch, neuter a constant, reorder a priority — then run the gate. A mutation that leaves the
   suite GREEN means the fix is UNTESTED; report it as a finding. Design mutations the implementer
   would not have anticipated. At least 10.
2. Verify each upstream citation by opening the file at v0.43.0 and COUNTING. A miscited line range
   is a finding.
3. Check the tests drive the LIVE path. A test against a dead sibling with no production callers
   proves nothing — grep for non-test callers of whatever the test calls.
4. Check for weakened assertions: diff the existing tests and confirm none was loosened or deleted.
5. Check the advertise-vs-dispatch invariant for any schema the model sees.
6. Run the suite 3+ times to catch flakiness. A flaky suite is how an untested fix hides.
7. Cross-crate lens: if a pub signature changed, run cargo check --workspace --all-targets.

RESTORE THE TREE to the fixed state when done and set treeRestored. Back up with cp to the
scratchpad BEFORE reverting anything; restore with cp. NEVER git checkout — it has destroyed
1,300 lines of uncommitted work in this effort already.`,
      { label: `verify:${g.key}`, phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' },
    ).then((v) => ({ group: g.key, title: g.title, impl, verify: v })),
)

const done = results.filter(Boolean)

phase('Sweep')

const sweep = await agent(
  `Final cross-cutting review of parity batch 9 in /home/d0m17bw/workspace/cyrup.

Five groups were ported and each verified in isolation. Isolated verification has repeatedly missed
INTERACTION defects in this effort — the last one shipped a hang because every check compared parts
against upstream separately and none drove a whole sequence end to end.

Per-group results:
${JSON.stringify(done, null, 2)}

${UPSTREAM}

Look for what per-group review structurally cannot see:
- The status enum grew a 'stopped' variant (G77). Find EVERY match over it across the whole crate,
  including catch-all arms that silently absorb the new variant, and every place status becomes a
  user-visible string, an exit code, or an intercom frame.
- G78 changed the acceptance status model while G79 changed the parser that produces it. Drive a
  real subprocess run end to end and assert the status the parser yields is the one the state model
  expects.
- G97's aliases and G99's roster change both feed resolution. Assert an alias cannot collide with a
  renamed builtin.
- Any two groups touching the same file: check the second did not undo the first.

Then run the FULL gate and report it verbatim:
  CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
  CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features test-fixtures
Check disk with df -h / first; the volume has hit 100% mid-run before and ENOSPC silently truncates
files. rm -rf target/debug/incremental is the safe thing to free.

Fix what you find. Report anything you could not fix, with evidence, rather than leaving it silent.`,
  { label: 'sweep', phase: 'Sweep', effort: 'high' },
)

return { groups: done, sweep }
