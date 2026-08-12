export const meta = {
  name: 'gap-refresh',
  description: 'Regenerate PARITY-GAPS.md from scratch against cyrup HEAD 04c1ba2 and all four upstreams at latest tags',
  phases: [
    { title: 'Survey', detail: '5 agents: four upstreams + a reachability sweep' },
    { title: 'Verify', detail: 'adversarial re-check of every claimed gap' },
    { title: 'Write', detail: 'serial: produce the new PARITY-GAPS.md' },
  ],
}

const BASE = `
Repo: /home/d0m17bw/workspace/cyrup at HEAD **04c1ba2** (clean; 6537 tests / 342 suites; clippy 0).
Upstreams (all read-only oracles — never edit them):
  /home/d0m17bw/workspace/pi                    latest **v0.84.1**   (cyrup ported v0.83.0)
  /home/d0m17bw/workspace/pi-subagents          latest **v0.43.0**   (batches 8-10 ported most of it)
  /home/d0m17bw/workspace/pi-permission-system  latest **v0.8.0**    (cyrup ported v0.7.1)
  /home/d0m17bw/workspace/pi-intercom           latest **v0.9.2**    (cyrup is at v0.7.0, NOT the
                                                v0.6.0 its lib.rs claims — diff from v0.7.0)
Read upstream with: git -C <repo> show <tag>:<path>   and   git -C <repo> diff <old>..<new>

**THE STANDARD OF EVIDENCE IS THE WHOLE POINT OF THIS TASK.**
The document you are helping replace was generated 2026-08-08 and is now wrong in both directions:
it lists items that batches 8-10 have since closed, and it missed things those batches found. Worse,
its predecessor framed unported work as "accepted divergence" and that framing held for as long as
nobody checked.

So: NEVER report a gap you have not confirmed against BOTH sides right now. Every entry needs
- upstream \`file:line\` at the named tag, read and counted (not inferred, not shifted);
- cyrup \`file:line\` as it exists at 04c1ba2;
- a one-line statement of the OBSERVABLE behavioural difference.
If you cannot produce all three, it is not an entry — say you could not confirm it.

Line citations have been a systematic problem here: a previous audit of 502 citations found 46
wrong, and a "fix" pass that renumbered by uniform shift INTRODUCED errors at 15% while looking
verified. Resolve every citation by reading the target. A range check is not enough — a shifted
citation still resolves in-range while naming the wrong line.

CLASSIFY each gap:
  **port bug**   — upstream had it at the tag cyrup ported; cyrup does not. Ranks first.
  **version lag** — upstream added it after cyrup's ported baseline.
  **unwired**    — the code EXISTS in cyrup but has no production caller. Cheapest to fix and the
                   single most common defect this project has: batches 8-10 shipped ~40 such items,
                   all with green tests, because tests called them directly.
There is no "accepted divergence" category. Mechanism may differ where the language forces it (WASM
guests vs jiti; ratatui vs a hand-rolled renderer) — port the BEHAVIOUR and state the mechanism
difference with its reason. That is not a gap; anything else is.

DO NOT EDIT ANY CODE. This is analysis only. Do not run the test suite — it is not needed and
several agents building at once contend on one target lock. Read, grep, and count.
`

phase('Survey')

const SURVEYS = [
  {
    key: 'pi-core',
    body: `**pi v0.83.0 -> v0.84.1** (627 files, +52291/-17556). This is the largest surface and the
least recently reviewed. Work the diff, not your memory.
  git -C /home/d0m17bw/workspace/pi diff v0.83.0..v0.84.1 --stat
Focus where cyrup actually ports from (see the crate<->package table in
/home/d0m17bw/workspace/CLAUDE.md): ai/, agent/, coding-agent/{core,modes}, tui/.
Known-unported and worth confirming precisely: providers \`baseten\` and
\`qwen-token-plan-individual\` (cyrup registers 38 of upstream's 40 at
cyrup-provider/src/providers/all.rs — note that file's own port-status doc table is STALE, read
\`builtin_providers_with\` instead); and the catalog shortfall (cyrup embeds 35 JSON catalogs under
providers/catalog/ against upstream's 39 \`*.models.ts\`, with Together hand-written in Rust).
pi's packages/{client,server,protocol,storage,evals} are OUT of the port's dependency closure —
confirm that is still true at v0.84.1 rather than assuming it.`,
  },
  {
    key: 'pi-subagents',
    body: `**pi-subagents v0.43.0** vs cyrup-ext-subagents at 04c1ba2. Batches 8, 9 and 10 just
ported a great deal of this — watchdog/, missions/, tui/fleet*, the acceptance model, the run-state
model — so the PRIMARY job here is establishing what is ACTUALLY left, not re-listing old findings.
Verify each remaining claim against the current tree.
Specifically resolve these, which are currently stated with uncertainty and must end as fact:
  - the fleet interactive half (handle_input, finish_action, set_terminal_rows) — wired or not?
  - permission_arbiter.rs — does it have a production caller now?
  - fleet_transcript.rs — does the transcript pane render real conversation/tool content against
    the artifact cyrup actually writes?
Also confirm the baseline: cyrup-ext-subagents records NO version string, so the ported baseline was
inferred as v0.33.x-v0.34.0 from commit dates. Say whether the current code justifies a different
figure now.`,
  },
  {
    key: 'permission-intercom',
    body: `Two smaller upstreams, both with a version caveat that has misled people:
**pi-permission-system v0.7.1 -> v0.8.0** (28 files, +4023/-1851). Note \`permanent-approval-store.ts\`
was DELETED upstream in v0.8.0 while cyrup still ports and wires \`PermanentApprovalStore\` — confirm
that is still true and size the removal.
**pi-intercom**: cyrup-intercom/src/lib.rs:2 SAYS v0.6.0 and is WRONG — the code is really at v0.7.0
(health probe / INTERCOM_PROTOCOL_NAME, rate limiting, ask edges, trust controls, the ask-timeout env
var are all present). Diff **v0.7.0..v0.9.2** or you will "find" a pile of already-done work.
Known: cyrup implements the Unix-socket transport only; upstream also has Windows named pipes and
opt-in TCP loopback. Confirm and size.`,
  },
  {
    key: 'unwired',
    body: `**The reachability sweep — this is the highest-value survey and the one no previous
analysis has done.**
Across ALL of crates/, find every \`pub\` item with ZERO non-test callers. Method: for each candidate,
grep its identifier across crates/ and check whether any hit sits ABOVE its file's \`mod tests\`
boundary (find that boundary per file; do not assume a line number).
This project's dominant defect is code that exists, has passing tests, and is never called in
production — batches 8-10 alone shipped ~40 such items, and several were reported as completed
features. A previous audit found \`resolve_single_result_status\` promoted to \`pub\`, rewritten and
given a test in the same change that deleted its only production caller.
Prioritise cyrup-ext-subagents (watchdog/, missions/, tui/fleet*), but sweep every crate.
For each, say whether upstream HAS a corresponding call site at its tag — if yes it is an unwired
gap; if upstream has no such surface either, it is dead code to delete.
Report the COMPLETE list. Partial lists are what let this accumulate.`,
  },
]

const SURVEY_SCHEMA = {
  type: 'object',
  required: ['gaps', 'confirmedClosed', 'couldNotConfirm'],
  properties: {
    gaps: {
      type: 'array',
      items: {
        type: 'object',
        required: ['title', 'kind', 'upstream', 'cyrup', 'behaviouralDifference', 'size'],
        properties: {
          title: { type: 'string' },
          kind: { type: 'string', enum: ['port bug', 'version lag', 'unwired'] },
          upstream: { type: 'string', description: 'file:line at the named tag, read and counted' },
          cyrup: { type: 'string', description: 'file:line at 04c1ba2' },
          behaviouralDifference: { type: 'string' },
          size: { type: 'string', enum: ['small', 'medium', 'large'] },
        },
      },
    },
    confirmedClosed: {
      type: 'array',
      description: 'items the OLD PARITY-GAPS.md listed that are now genuinely done, with evidence',
      items: { type: 'string' },
    },
    couldNotConfirm: {
      type: 'array',
      description: 'suspected gaps you could NOT evidence on both sides — reported, not asserted',
      items: { type: 'string' },
    },
  },
}

const surveyed = await pipeline(
  SURVEYS,
  (s) =>
    agent(
      `Survey your assigned upstream surface and report EVIDENCED gaps against cyrup at 04c1ba2.

${s.body}

${BASE}

Read the existing /home/d0m17bw/workspace/cyrup/PARITY-GAPS.md FIRST to see what was claimed on
2026-08-08 — then verify each relevant claim against today's tree rather than copying it. Report
what is now closed (with evidence) as well as what remains.`,
      { label: `survey:${s.key}`, phase: 'Survey', schema: SURVEY_SCHEMA },
    ),
  (res, s) =>
    agent(
      `ADVERSARIAL RE-CHECK of a gap survey. Your job is to find entries that are WRONG.

Surface: ${s.key}
${JSON.stringify(res, null, 2)}

${BASE}

For a sample of at least 15 entries (all of them if fewer), and ALL entries marked "port bug":
1. Open the upstream file at the named tag and COUNT — is the citation right, and does it name the
   code the entry claims?
2. Open the cyrup file at 04c1ba2 — is the gap REAL, or has it already been closed by batches 8-10?
   A stale "gap" is the specific failure mode that made the previous document misleading.
3. Is the stated behavioural difference actually OBSERVABLE, or is it a mechanism difference the
   language forces (WASM vs jiti, ratatui vs hand-rolled render, Arc<dyn Fn> vs a JS closure)?
4. For "unwired" entries: verify the zero-caller claim yourself against the mod tests boundary.

Report every entry you found wrong and why, and confirm the ones that survive. Be specific: an entry
you cannot verify either way is itself a finding.`,
      { label: `check:${s.key}`, phase: 'Verify', schema: {
        type: 'object',
        required: ['wrong', 'confirmed', 'verdict'],
        properties: {
          wrong: { type: 'array', items: { type: 'object', required: ['title', 'why'], properties: { title: { type: 'string' }, why: { type: 'string' } } } },
          confirmed: { type: 'array', items: { type: 'string' } },
          unverifiable: { type: 'array', items: { type: 'string' } },
          verdict: { type: 'string' },
        },
      }, effort: 'high' },
    ).then((v) => ({ surface: s.key, survey: res, check: v })),
)

phase('Write')

const written = await agent(
  `Write the new /home/d0m17bw/workspace/cyrup/PARITY-GAPS.md, replacing the 2026-08-08 version
entirely. You have exclusive access; no other agent is running.

Surveys and their adversarial re-checks:
${JSON.stringify(surveyed.filter(Boolean), null, 2)}

${BASE}

**Drop every entry the re-check found wrong.** Where a re-check and a survey disagree, verify it
yourself and say which you took. An entry that survives into this document is one a reader should be
able to act on without re-deriving it.

Structure it to be USED, not admired:
1. A header stating cyrup HEAD, each upstream's ported baseline vs latest tag, the generation date,
   and the total counts by kind.
2. **Port bugs first** — upstream had it at the ported tag, cyrup does not. These rank above
   everything.
3. **Unwired items** — code that exists with no production caller. State plainly that this is the
   project's most common defect class and the cheapest to fix.
4. **Version lag**, grouped by upstream.
5. A short section on what batches 8-10 CLOSED, so the next reader can see the doc is current and
   trust it.
6. Open questions — things that need a human decision, not more analysis.

Every entry: upstream file:line at its tag, cyrup file:line at 04c1ba2, the observable behavioural
difference, and a size. No entry without all four.

Two things the old document got badly wrong, which you must not repeat:
- It framed unported work as "accepted divergence … don't fix them without asking", which converted
  an unfinished port into a list nobody was allowed to finish. There is no such category. The
  project's goal is behavioural equivalence.
- It cited \`spec/\` paths and \`ADR-0001\`, neither of which exists in this workspace. Do not cite
  authority that cannot be checked from here.

Also state the method and its limits honestly at the end — what was sampled vs exhaustive, and what
could not be confirmed. A reader should know how much to trust each section.`,
  { label: 'write', phase: 'Write', effort: 'high' },
)

return { surveyed: surveyed.filter(Boolean), written }
