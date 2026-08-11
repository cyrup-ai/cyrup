export const meta = {
  name: 'parity-batch10',
  description: 'Parity batch 10 — the three unported pi-subagents subtrees: watchdog/ (17 files), missions/ (6), tui/fleet* (3)',
  phases: [
    { title: 'Port', detail: '4 groups on disjoint new modules' },
    { title: 'Verify', detail: 'adversarial pass per group' },
    { title: 'Mutate', detail: 'ONE serial verifier, exclusive tree' },
    { title: 'Sweep', detail: 'integration + full gate' },
  ],
}

const BASE = `
Repo: /home/d0m17bw/workspace/cyrup   Crate: crates/cyrup-ext-subagents
Upstream: /home/d0m17bw/workspace/pi-subagents at **v0.43.0** (tag e76a256).
Read it with: git -C /home/d0m17bw/workspace/pi-subagents show v0.43.0:src/<path>
NEVER infer behaviour from a name, from this brief, or from cyrup's doc comments. Open the upstream
file, read the whole function, and COUNT the lines before citing them.

These three subtrees are **entirely unported** — the strings "watchdog", "mission" and "fleet"
appear nowhere in the crate. You are writing new modules, not editing existing ones. That makes the
usual failure mode WORSE, not better: new code with new tests can be fully green and still be
unreachable from any production path.

THE GATE — the UNQUALIFIED feature flag, always:
  CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
  CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features test-fixtures
A qualified '--features cyrup-ext-subagents/test-fixtures' compiles OTHER crates' cfg-gated test
files to empty and hid a cross-crate break that had the real gate at exit=101.
Baseline: 5941 passed / 0 failed / 8 ignored / 340 suites, clippy exit 0, HEAD 097bdde.

HARD-WON RULES — every one of these cost real time in batches 8-9:
- **Wire it or it does not count.** Before claiming an item done, grep for a NON-TEST caller and
  confirm it sits above the crate's 'mod tests' boundary. Three separate "fixes" were reported
  closed while landing on code with no production caller.
- **Fix, don't file.** "Blocked" has been wrong every time it was claimed. Search for the
  CAPABILITY, not the identifier you first guessed.
- No-panic policy: clippy DENIES unwrap_used/expect_used/panic/indexing_slicing in non-test code and
  fires ONLY under clippy — cargo test stays green with a violation present. Opt-outs are PER-LINT:
  the usual unwrap/expect/indexing trio does NOT cover 'panic'. Check clippy's exit code directly,
  never through a pipe.
- New modules need '//!' provenance doc comments citing the upstream file — that index is how parity
  is audited here. Cite by READING the target and counting; a uniform line-shift looks verified and
  is not.
- NEVER weaken, loosen or delete an assertion. Print BEFORE/AFTER for any existing test you touch.
- A subagent run is ALWAYS a real OS subprocess over NDJSON with SIGINT->SIGTERM->SIGKILL. Never
  make it in-process.
- Temp dirs: use tempfile::TempDir, never a hand-rolled env::temp_dir() path — five helper families
  leaked 19,874 directories that way. And note TempDir does NOT Deref to Path, so 'tmp().join("x")'
  drops the guard into a temporary and deletes the dir before the test uses it.
- CLAUDE.md is a search index, NOT an oracle. It has been stale on target size (~35x), crate count,
  the all.rs port-status table, the HEAD SHA, and a no-panic carve-out. Verify against the code.

TREE DISCIPLINE — several sessions share this checkout:
- Your scratchpad is yours alone: <session scratchpad>/<YOUR-LABEL>/ — create it. Never write to the
  scratchpad root; two sessions independently chose the same filename there and each could have read
  the other's output as its own.
- SURGICAL reverts only. Snapshot every file you touch BEFORE touching it, including files that
  start clean. NEVER git checkout.
- Do NOT run heavy I/O (mass rm, find over /tmp) while a build or suite runs — that produced two
  spurious failures in a socket-backoff test and a file-watcher test, both of which read as ordinary
  logic bugs.
- Check df -h / and df -h /tmp before any long build. /tmp is a separate 16 GB tmpfs; when it fills,
  ld dies with SIGBUS and the suite reports it as test failures.
`

const GROUPS = [
  {
    key: 'watchdog-core',
    title: 'watchdog/ — runtime, scope, settings, registration',
    body: `Port src/watchdog/{runtime,scope,settings,types,register-main,register-child}.ts.
This is the spine the rest of the subtree hangs off, so establish the module layout, the config
surface and the registration seams first. Registration is the part most likely to end up unwired:
find where upstream actually calls register-main and register-child, and wire cyrup's equivalents at
the same points rather than exposing a function nobody calls.`,
  },
  {
    key: 'watchdog-signals',
    title: 'watchdog/ — signal collection and arbitration',
    body: `Port src/watchdog/{change-signature,child-status,turn-delta,emission-guard,
lsp-diagnostics,model-selection,permission-arbiter,review,tool-actions,render}.ts.
permission-arbiter interacts with cyrup's permission gate, which comes from the separate
pi-permission-system port — check how the two actually meet before assuming a seam exists.
emission-guard and turn-delta are ordering-sensitive; port their sequencing, not just their outputs.`,
  },
  {
    key: 'missions',
    title: 'missions/ — the mission subsystem (6 files)',
    body: `Port all of src/missions/. Read every file before designing the Rust module layout;
6 files is small enough to hold at once, and a layout chosen from filenames alone will fight the
port. Note there is a src/missions/types.ts distinct from src/shared/types.ts — cite paths fully,
because a bare 'types.ts' citation is ambiguous across three upstream directories.`,
  },
  {
    key: 'fleet',
    title: 'tui/fleet* — FleetView (3 files, +1856 LOC)',
    body: `Port src/tui/{fleet,fleet-status,fleet-transcript}.ts into cyrup's TUI layer.
Mechanism differs by necessity and that is fine: pi hand-rolls a render(width) -> string[] framework
with its own line-diff renderer, while cyrup delegates to ratatui + crossterm (reached via
cyrup_tui::crossterm, never a direct dep) and ports only the application layer. Port the BEHAVIOUR
and state model; state the transport difference in the doc comment with its reason.
Related and in scope: '/subagents-fleet' is version-lagged — v0.43.0 slash-commands.ts:714-716 is
"Open the live subagent fleet inspector" + showFleet(ctx), while cyrup still ships v0.34.0's
runSlashSubagent({action:"status",view:"fleet"}). That was blocked on this port; unblock it.`,
  },
]

const PORT_SCHEMA = {
  type: 'object',
  required: ['items', 'testResult', 'clippyExit', 'wiring'],
  properties: {
    items: {
      type: 'array',
      items: {
        type: 'object',
        required: ['upstreamFile', 'status', 'cyrupPath', 'whatChanged'],
        properties: {
          upstreamFile: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'not-done'] },
          cyrupPath: { type: 'string' },
          whatChanged: { type: 'string' },
          testsAdded: { type: 'array', items: { type: 'string' } },
          notDoneReason: { type: 'string' },
        },
      },
    },
    wiring: {
      type: 'array',
      description: 'For each new entry point: its non-test production caller, with file:line',
      items: { type: 'string' },
    },
    testResult: { type: 'string' },
    clippyExit: { type: 'string' },
    publicApiChanges: { type: 'array', items: { type: 'string' } },
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
    deadCode: { type: 'array', items: { type: 'string' } },
    treeRestored: { type: 'boolean' },
    verdict: { type: 'string' },
  },
}

phase('Port')

const results = await pipeline(
  GROUPS,
  (g) =>
    agent(
      `Port this upstream subtree to cyrup, to behavioural parity with pi-subagents v0.43.0.

${g.title}

${g.body}

${BASE}

Read EVERY upstream file in your group before writing Rust. Port all of it — an unported file is not
a smaller batch, it is an incomplete one. If something genuinely cannot be ported without a seam
that does not exist, build the seam; if that is truly out of reach, say so with evidence rather than
quietly narrowing scope.

Report the full test result with --features test-fixtures, clippy's exit code, and for EVERY new
entry point the non-test production caller with file:line.`,
      { label: `port:${g.key}`, phase: 'Port', schema: PORT_SCHEMA },
    ),
  (impl, g) =>
    agent(
      `ADVERSARIAL VERIFICATION of a freshly ported subtree. Prove it wrong.

Group: ${g.title}
${g.body}

The implementer reported:
${JSON.stringify(impl, null, 2)}

${BASE}

1. **Reachability first.** This is new code, so the likeliest defect by far is that it is never
   called. For every claimed entry point, grep for a non-test caller and confirm it is above the
   'mod tests' boundary. List everything with zero production callers as deadCode.
2. MUTATION TEST every claimed behaviour — invert a condition, drop a branch, neuter a constant,
   reorder a sequence. A mutation leaving the suite GREEN means the behaviour is UNTESTED; report
   it. At least 12, designed so the implementer would not have anticipated them.
3. Verify every upstream citation by opening the file at v0.43.0 and COUNTING.
4. Check for weakened assertions and for advertise-vs-dispatch on anything the model sees.
5. Run the suite 3x to catch flakiness.

RESTORE the tree to the fixed state, surgically, and set treeRestored. Back up with cp to YOUR
scratchpad subdir before reverting. NEVER git checkout.`,
      { label: `verify:${g.key}`, phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' },
    ).then((v) => ({ group: g.key, title: g.title, impl, verify: v })),
)

const done = results.filter(Boolean)

phase('Mutate')

const mutate = await agent(
  `You have EXCLUSIVE tree access — all other agents have finished. Nothing you see is contaminated
by a concurrent edit. Earlier in this effort five verifiers ran mutation campaigns concurrently in
one tree and produced results that could not be attributed; you are the serial replacement.

Per-group results:
${JSON.stringify(done, null, 2)}

${BASE}

1. Re-run mutation testing SERIALLY across all four groups, one mutation at a time, restoring
   surgically and byte-comparing against a private snapshot after each. At least 25, weighted toward
   anything a group verifier reported GREEN and toward every newly wired entry point.
2. For each survivor, decide explicitly: is it an EQUIVALENT mutant (prove it — an equivalence
   argument or a fuzz result) or a real coverage gap? Do not report equivalence without proof.
3. Confirm no mutation residue anywhere in src/.
4. Run the suite twice, once under CPU contention
   (for i in $(seq 1 $(nproc)); do (while :; do :; done) & done ... kill $(jobs -p)).

RESTORE the tree and say so explicitly.`,
  { label: 'mutate', phase: 'Mutate', effort: 'high' },
)

phase('Sweep')

const sweep = await agent(
  `Final integration review of parity batch 10 — three previously-unported subtrees now landed.

${JSON.stringify({ groups: done.map((d) => d.group), mutate: String(mutate).slice(0, 4000) }, null, 2)}

${BASE}

Per-group review is structurally blind to interaction defects; that blindness shipped a hang earlier
in this effort. Look for what it cannot see:
- watchdog registration vs the existing extension lifecycle: does a watchdog actually start, observe
  a real subprocess run, and stop, without leaking a task or a file watcher?
- fleet vs the existing TUI event loop: drive a real sequence and assert painted cells, not just row
  strings. Earlier TUI work passed 10 batches of row-level checks while tool calls rendered after
  the assistant response in solid green, because nothing drove an interleaved sequence.
- missions vs chains/subagent spawn: do they contend over the same run state?
- '/subagents-fleet' end to end.
- Any file touched by two groups: confirm the second did not undo the first.

Then run the FULL gate and report VERBATIM:
  df -h / ; df -h /tmp     (check FIRST; / has hit 100% mid-run, ENOSPC silently truncates files,
                            rm -rf target/debug/incremental is the safe reclaim and it refills)
  CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
  CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features test-fixtures

Fix what you find. Then state plainly, with evidence, what remains unported — a named unfinished
item beats a clean-sounding summary.`,
  { label: 'sweep', phase: 'Sweep', effort: 'high' },
)

return { groups: done, mutate, sweep }
