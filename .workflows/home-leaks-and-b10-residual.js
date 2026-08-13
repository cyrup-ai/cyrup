export const meta = {
  name: 'home-leaks-and-b10-residual',
  description: 'Kill both $HOME leak producers for real, then finish batch 10 residual dead code and untested behaviours',
  phases: [
    { title: 'Fix', detail: '3 agents: two leak producers + b10 residual' },
    { title: 'Verify', detail: 'serial verifier, delete-then-count on a quiet box' },
  ],
}

const BASE = `
Repo: /home/d0m17bw/workspace/cyrup   Crate: crates/cyrup-ext-subagents
Upstream: /home/d0m17bw/workspace/pi-subagents at **v0.43.0** (tag e76a256).
Read with: git -C /home/d0m17bw/workspace/pi-subagents show v0.43.0:src/<path>
Never infer behaviour from a name or this brief. Open the upstream file and COUNT lines before
citing them.

THE GATE — unqualified feature flag:
  CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
  CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features test-fixtures
Baseline: 6516 passed / 0 failed / 8 ignored / 342 suites, clippy exit 0, HEAD eed28c9.

**MEASUREMENT DISCIPLINE — this is the point of the whole workflow.** A previous pass reported the
$HOME mission leak fixed with "isolated per-target delta 0". That was true and useless: after the
directory was deleted entirely, ONE full workspace gate regrew it to 386 files. Isolated targets do
not exercise the producer. The ONLY measurement that counts:

    rm -rf ~/.cyrup/agent/missions ~/.cyrup/subagents
    CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
    find ~/.cyrup/agent/missions ~/.cyrup/subagents -type f 2>/dev/null | wc -l    # MUST be 0

Run it on a QUIET box — check 'pgrep -af "bin/cargo"' first and note it matches its own command
line. If another session is building, your number is meaningless; say so rather than reporting it.

RULES
- Grep for a NON-TEST caller before claiming anything wired; report its file:line. Batch 10 shipped
  ~40 items with no production caller and a green suite, because tests called them directly.
- No-panic: clippy DENIES unwrap_used/expect_used/panic/indexing_slicing in non-test code, fires
  ONLY under clippy, and opt-outs are PER-LINT (the usual trio does NOT cover 'panic'). Read
  clippy's exit code directly, never through a pipe.
- NEVER weaken, loosen or delete an assertion. BEFORE/AFTER for any test you touch.
- NEVER write to $HOME or any real user directory from a test. Use tempfile::TempDir. Note TempDir
  does not Deref to Path, and tmp().join("x") drops the guard into a temporary.
- Your scratchpad is yours alone. Surgical reverts. Never git checkout. No heavy I/O during a suite.
`

phase('Fix')

const TASKS = [
  {
    key: 'mission-leak',
    files: 'src/missions/**, and any test that reaches mission code',
    body: `**The mission-index leak is NOT fixed.** \`~/.cyrup/agent/missions/index\` was deleted
outright; one full workspace gate regrew it to **386 files** — all test garbage (titles "do it",
"a", "c"; every projectRoot a deleted tempdir; 11 files per tempdir, so ~35 distinct tempdirs).

The previous pass fixed real things — \`missions/store.rs\`'s \`home_dir()\` was the one of five
crate home resolvers ignoring \`CYRUP_HOME\`, and it restored upstream's \`projectFixture\`
\`globalIndexDir\` scoping across lifecycle/extension and three integration binaries — but a
producer remains.

Find it by MEASUREMENT, not by reading: delete the dir, run ONE test target at a time across the
whole crate (and any other crate that reaches mission code), and re-count after each. The "11 files
per tempdir" shape is your fingerprint — find which target produces 11 per fixture. Then fix it.

Consider that the producer may not be a mission test at all: anything that dispatches a subagent
task through the executor reaches \`extension.rs:8232\`'s mission launch path. A test that never
mentions missions can still produce them.

Deliverable: delete-then-full-workspace-count of ZERO, on a quiet box.`,
  },
  {
    key: 'subagents-leak',
    files: 'src/background/**, src/extension.rs (default_async_root/default_results_dir), src/*/artifacts.rs',
    body: `**A second, larger leak of the same class, entirely unfixed.** \`~/.cyrup/subagents\`
had accumulated **59,321 files / 551 MB** — \`async/\` across 21,076 cwd-keyed dirs, plus
\`artifacts/\`, \`chain-runs/\`, \`results/\`, \`run-history.jsonl\`. Written via
\`background::subagents_home()\` (src/background/mod.rs:1224), reached from \`default_async_root\`
and \`default_results_dir\` (extension.rs:1196, 1209, 3308, 3503, 3534, 3649, 3666) and
artifacts.rs:155,162. Run IDs are synthetic (\`fleetrun0001\`), so it is test residue in real user
config. I have cleared it; it will regrow.

First check whether the PRODUCTION default is faithful to upstream before changing it — for the
mission leak it was, and only the tests were wrong. Read upstream's equivalent
(\`runs/background/\`) and say which side is at fault.

Then make every test that reaches these paths scope its root at a \`tempfile::TempDir\`. Note
\`subagents_home()\` may have the same \`CYRUP_HOME\` bug the mission resolver had — check it against
the other four resolvers (extension.rs:4719, exec/mcp_direct_tools.rs:831,
registration/prompt_workflows.rs:105, watchdog/settings.rs:743).

Deliverable: delete-then-full-workspace-count of ZERO, on a quiet box.

Also audit the whole crate for this class and report everything: any test path resolving to
\`agent_dir()\`, \`home_dir()\`, \`dirs::\`, \`subagents_home()\`, or a literal '~' without a TempDir
override.`,
  },
  {
    key: 'b10-residual',
    files: 'src/watchdog/**, src/tui/fleet*.rs, src/extension.rs (fleet + watchdog wiring)',
    body: `Batch 10's closeout left residual dead code and untested behaviour. Finish it.

Re-derive the list yourself rather than trusting mine — sweep \`src/watchdog/\`, \`src/missions/\`
and \`src/tui/fleet*\` for every \`pub\` item with zero non-test callers (check against the
\`mod tests\` boundary), and either wire it where upstream invokes it or delete it with a
v0.43.0-sourced reason. Report the complete list with a verdict each.

Known-remaining from the closeout's own report, as a starting point only:
- fleet: the interactive half (handle_input, finish_action, set_terminal_rows — terminal_rows is
  permanently 32) may still lack a production path into the TUI event loop.
- \`fleet_transcript.rs\`: verify the transcript pane renders REAL conversation and tool content
  against the artifact cyrup actually writes, not just remapped tags.
- \`permission_arbiter.rs\`: confirm it now has a production caller and that the arbiter actually
  meets cyrup's permission gate (which comes from the pi-permission-system port).
- Style assertions: every fleet test flattened \`Vec<Line>\` via \`lines_text\`, so colours were
  unasserted across five modules and a real repaint bug shipped. Confirm style coverage now exists
  for anything colour-carrying; add it where it does not.

There is an untracked \`src/tui/fleet_overlay.rs\` in the tree that I did not write and a
\`SteerDeliveryMode\` unused-import warning was reported in it. Determine whether it belongs, and
either integrate it properly or report it — do not silently delete another party's work.`,
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
          notDoneReason: { type: 'string' },
        },
      },
    },
    wiring: { type: 'array', items: { type: 'string' } },
    deletedInstead: { type: 'array', items: { type: 'string' } },
    leakMeasurement: { type: 'string', description: 'delete-then-full-workspace-count, and whether the box was quiet' },
    testResult: { type: 'string' },
    clippyExit: { type: 'string' },
  },
}

const done = (
  await parallel(
    TASKS.map((t) => () =>
      agent(
        `AREA: ${t.key}
YOUR FILES (other agents own the rest — stay inside these):
  ${t.files}

${t.body}

${BASE}

Do NOT run mutation tests; a serial verifier follows with exclusive tree access.`,
        { label: `fix:${t.key}`, phase: 'Fix', schema: SCHEMA },
      ),
    ),
  )
).filter(Boolean)

phase('Verify')

const verify = await agent(
  `EXCLUSIVE tree access — all other agents finished.

Claims:
${JSON.stringify(done, null, 2)}

${BASE}

1. **THE LEAK MEASUREMENT IS THE HEADLINE.** Confirm the box is quiet (pgrep), then:
     rm -rf ~/.cyrup/agent/missions ~/.cyrup/subagents
     full workspace gate
     find ~/.cyrup/agent/missions ~/.cyrup/subagents -type f 2>/dev/null | wc -l
   Report the number. If it is not 0, the fix failed regardless of what any agent claimed — say so
   and find the remaining producer. Also confirm nothing else appeared anywhere under ~/.cyrup
   beyond models-store.json, settings.json, sessions/ and their .lock files.
2. **Reachability sweep** across watchdog/, missions/ and tui/fleet*: list EVERY pub item with zero
   non-test callers. This is the complete-list deliverable batch 10 never produced.
3. MUTATION TEST serially, restoring surgically and byte-comparing after each. At least 20, weighted
   toward newly wired paths. For each survivor: EQUIVALENT (prove it) or real gap.
4. Full gate twice, once under CPU contention.
5. Confirm no assertion was weakened or deleted.

RESTORE the tree and say so. State plainly what remains unfixed, with evidence.`,
  { label: 'verify', phase: 'Verify', effort: 'high' },
)

return { done, verify }
