export const meta = {
  name: 'cyrup-parity-batch-06',
  description: 'Parity batch 6 — pi PRE-BASELINE port bugs: G39, G3(list), G62, G63, G66, G16/G42',
  phases: [
    { title: 'Fix', detail: 'grouped by crate; these are bugs, not version lag' },
    { title: 'Verify', detail: 'ONE reviewer; port-bug classification is the thing to check' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are working on \`cyrup\` (${WS}/cyrup), a Rust port targeting BEHAVIOURAL EQUIVALENCE with pi
(${WS}/pi). Ported baseline **v0.83.0**; target **v0.84.1**. The backlog is
\`${WS}/cyrup/PARITY-GAPS.md\`.

## WHAT MAKES THIS BATCH DIFFERENT

Every item here is a **PORT BUG, not version lag**: the behaviour was already present at v0.83.0 —
the tag cyrup targets — and was never ported. These rank ahead of everything post-baseline because
they are things cyrup was always supposed to do.

**Verify that classification yourself for each item** (\`git -C ${WS}/pi show v0.83.0:packages/<path>\`
AND \`v0.84.1\`). If an item turns out to be version lag after all, say so — the backlog has been
wrong before, and mis-labelling distorts priority for everything after it.

## WHAT THE TUI EFFORT ESTABLISHED — carry it over

Ten TUI batches plus two closeout rounds ran on this repo. The failure modes that recurred:

1. **Ported-but-unwired.** A mechanism ported faithfully and wired to nothing. Batch 9 shipped EIGHT
   state seams whose only callers were tests. **Name the user action that reaches your change and
   drive THAT in the test**, not the setter.
2. **Untested fixes.** Batches shipped six, then eleven, found only by disabling each behaviour and
   watching the suite stay green. Before finishing, revert each change in turn and confirm something
   goes red.
3. **Claims inferred, not read.** Two items were declared "genuinely blocked" because an agent
   grepped ONE SPELLING of a function name and concluded the subsystem was absent. Both were false;
   one was a single discarded error signal. **Search for the CAPABILITY, not the identifier.**
   And: "an API I would have to add" is WORK, not a blocker.
4. **Scheduled is not done.** A completeness check on the plan proved nothing about the work.

## HARD RULES

- **READ pi's source. Never infer from a name, this brief, or the backlog.** Quote the \`file:line\`
  and COUNT. The backlog has been wrong four times in the TUI half alone.
- \`spec/\` and \`ADR-0001\` do not exist in this workspace and are never authority for anything.
- **Fix, do not file.**
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` are separate crates needing their own
  file-level \`#![allow(...)]\`.
- **Never weaken an assertion.** Report each edited test BEFORE/AFTER.
- **\`cargo check -p\` is NOT a gate for a \`pub\` signature change** — cross-crate callers, including
  other crates' \`tests/\`, stay invisible. If you change one, say so explicitly.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\`. **NEVER \`git checkout\`.**

## BUILD DISCIPLINE — AGENTS RUN IN PARALLEL

ONE shared \`target/\`, ONE \`.cargo-lock\`; cargo WILL block. Wait it out.
\`CARGO_INCREMENTAL=0\`; \`-p <your crate>\` only. **NEVER \`--workspace\`** — the orchestrator gates once.
Finish with \`cargo clippy -p <crate> --all-targets; echo "exit=$?"\` for each crate you touch.
`

const ITEM_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['group', 'items'],
  properties: {
    group: { type: 'string' },
    items: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['id', 'status', 'summary', 'classification', 'user_action', 'revert_proof'],
        properties: {
          id: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'already-implemented', 'blocked'] },
          summary: { type: 'string' },
          classification: { type: 'string', enum: ['port-bug', 'version-lag', 'unclear'] },
          classification_evidence: { type: 'string', description: 'What v0.83.0 says, quoted' },
          user_action: { type: 'string', description: 'The user action that reaches this. "A caller exists" is not an answer.' },
          upstream_citation: { type: 'string' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string', description: 'A mirror you actually RAN' },
        },
      },
    },
    public_signature_change: { type: 'boolean' },
    tests_changed: { type: 'string' },
    files_changed: { type: 'array', items: { type: 'string' } },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Fix')

const GROUPS = [
  {
    key: 'session-auth',
    title: 'G39 + G3 — model cycling and the auth trait (cyrup-session-svc, cyrup-provider)',
    brief: `**G39 — \`--model\` cycling walks the whole catalog, not the AUTHENTICATED models.**
v0.83.0 \`coding-agent/src/core/agent-session.ts:1644\` already did
\`await this._modelRuntime.getAvailable()\` (auth-filtered). cyrup cycles the whole composed catalog
(\`cyrup-session-svc/src/session.rs\`), so a user pressing the cycle key lands on providers they
cannot call. Port the filter. The user action is the model-cycle keystroke — drive that.

**G3 — \`CredentialStore::list()\` only.** The full item (cancellation threading + a 15 s OAuth
refresh bound) is v0.84.1 version lag and belongs to batch 10. THIS batch does only the port-bug
half: v0.83.0 \`ai/src/auth/types.ts:71\` already declares \`list()\`, and cyrup's trait
(\`cyrup-provider/src/auth/store.rs\`) has no \`list\` at all. Add it, implement it for every store,
and find what upstream USES it for at v0.83.0 — if nothing in cyrup calls it, that is
ported-but-unwired and you must either wire the consumer or name precisely what is missing.
Adding a trait method is a \`pub\` signature change: say so.`,
  },
  {
    key: 'tui-input',
    title: 'G62 + G63 — editor page actions and the Shift+Enter probe (cyrup-tui)',
    brief: `**G62 — \`ctrl+home\`/\`ctrl+end\` aliases AND editor page actions.** The PAGE ACTIONS are the
port bug: v0.83.0 \`tui/src/keybindings.ts:89\` already has \`tui.editor.pageUp\`/\`pageDown\`, and
cyrup's \`EditorAction\` has no page variant at all, so \`PageUp\` always scrolls the TRANSCRIPT even
when the editor has focus. The ctrl aliases are v0.84.1 lag; do both, but classify them separately.

**G63 — the native modifier probe for Shift+Enter.** The Apple Terminal half is the port bug:
v0.83.0 \`tui/src/terminal.ts:44\` already exports \`normalizeAppleTerminalInput\` and ships the darwin
probe, so on Apple Terminal today Shift+Enter SUBMITS instead of inserting a newline. The win32 half
is new at v0.84.1.
Batch 7 of the TUI effort ported \`word_wrap_line\` and reworked the editor — read the current code
before assuming what is there. NOTE: neither platform can be exercised on this Linux box. Drive the
DECISION FUNCTION with a synthesized environment, and say plainly in the test doc that the platform
path itself is unexercised here — do not imply coverage you do not have.`,
  },
  {
    key: 'progress-providers',
    title: 'G66 + G16/G42 — OSC 9;4 progress and the qwen parent providers',
    brief: `**G66 — OSC 9;4 terminal progress.** The \`/settings\` row and the setting are fully wired
(\`cyrup-config/src/settings.rs\` ← \`cyrup-tui/src/app.rs\`) and the EMITTER does not exist, so a user
can turn on "Terminal progress" and get nothing. v0.83.0 \`tui/src/terminal.ts:11-13\` already defines
the sequences and writes them; v0.84.1 only drops a stray \`;\`. This is the INVERSE of the usual
defect — the switch is wired to a mechanism that was never built.
Port the emitter and drive it from the real progress transitions. Be careful what "progress" means:
read what pi emits and WHEN (start, update, clear), and make sure a crashed/aborted turn clears it —
a stuck progress indicator in the taskbar outlives the process that set it.

**G16/G42 — the qwen parent providers.** v0.83.0 \`ai/src/types.ts:67-68\` already lists
\`qwen-token-plan\` and \`qwen-token-plan-cn\` as \`KnownProvider\`; only \`-individual\` is new at
v0.84.1. Port the two parents (\`cyrup-provider/src/providers/\`). Check whether they are fleet
members or standalone before writing anything, and confirm the catalog set matches the implemented
set — the backlog notes there are no orphan catalogs and that invariant should hold after you land.`,
  },
]

const authored = await parallel(
  GROUPS.map((g) => () =>
    agent(
      `${COMMON}

## Your group: ${g.title}

${g.brief}

For each item: verify the port-bug classification at v0.83.0, read pi, fix, name the user action and
drive it, add a test that FAILS without the fix plus a MIRROR you actually RUN, revert-prove against
a cp-backup of the FIXED state. Then clippy.`,
      { label: `fix:${g.key}`, phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
    )
  )
)

const ok = authored.filter(Boolean)
const allItems = ok.flatMap((r) => (r.items || []).map((i) => ({ ...i, group: r.group })))
log(`Authored ${allItems.length} items across ${ok.length} groups`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'classification_audit'],
  properties: {
    tree_restored: { type: 'boolean' },
    classification_audit: {
      type: 'string',
      description: 'Per item: is the port-bug vs version-lag call correct? Quote v0.83.0.',
    },
    weakened_assertions: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['item', 'problem', 'severity', 'evidence'],
        properties: {
          item: { type: 'string' },
          problem: { type: 'string' },
          severity: { type: 'string', enum: ['blocker', 'major', 'minor'] },
          evidence: { type: 'string' },
          fix: { type: 'string' },
        },
      },
    },
    overall: { type: 'string' },
  },
}

const digest = allItems
  .map(
    (a) => `### [${a.group}] ${a.id} [${a.status}] (${a.classification}) ${a.summary}
CLASSIFICATION EVIDENCE: ${(a.classification_evidence || '(none)').slice(0, 300)}
USER ACTION: ${a.user_action || '(none)'}
UPSTREAM: ${a.upstream_citation || '(none)'}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 400)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. Do NOT fix; report.

**Lens 1 — the classification.** For EVERY item, open v0.83.0 yourself and confirm port-bug vs
version-lag. A wrong call here distorts the priority of every remaining batch. Fill
\`classification_audit\`.

**Lens 2 — reachability.** Verify each claimed user action actually reaches the code. G66 is the
acid test: the setting was ALREADY wired to a mechanism that did not exist, so check the emitter now
fires on the real transitions and, critically, that it CLEARS on an aborted or crashed turn.

**Lens 3 — untested fixes.** Disable each behaviour in turn and confirm something goes red. Report
any that leaves the suite green — that has caught six and then eleven untested fixes in this repo.

**Lens 4 — platform honesty.** G63 covers Apple Terminal and win32, neither exercisable here.
Confirm the tests drive a decision function with a synthesized environment and that the docs say
plainly the platform path is unexercised — not implied coverage.

**Lens 5 — weakened assertions, citations, public signatures.** \`git diff\` every edited test and
rule on each. Open every cited pi line and COUNT. G3 adds a trait method — check every implementor
and every cross-crate caller, since \`-p\` builds cannot see them.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm the affected crates are green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`; NEVER \`git checkout\`.

## What the agents reported

${digest}

PUBLIC SIGNATURE CHANGES:
${ok.map((r) => `[${r.group}] ${r.public_signature_change ? 'YES' : 'no'} — ${r.notes || ''}`).join('\n')}

TESTS CHANGED:
${ok.map((r) => `[${r.group}] ${r.tests_changed || '(none)'}`).join('\n')}`,
  { label: 'verify:port-bugs', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'parity-6',
  items: allItems.map((i) => ({ group: i.group, id: i.id, status: i.status, classification: i.classification })),
  classification_audit: review?.classification_audit,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
