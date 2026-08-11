export const meta = {
  name: 'cyrup-tui-batch-03-selection',
  description: 'TUI batch 3 — SYS-4 selectedBg inversion + SelectList geometry (S1,S2,S3,S10,S24-S28)',
  phases: [
    { title: 'Fix', detail: 'one crate, sequential — select_list.rs and the row builders' },
    { title: 'Verify', detail: 'ONE reviewer; selection style is visible in every list' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are fixing PRESENTATION FIDELITY defects in \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against
pi (${WS}/pi) at **v0.84.1**. The audit is \`${WS}/cyrup/TUI-FIDELITY.md\` — read §SYS-4 and the S-rows
for your items before touching anything. Every claim there was verified on both sides.

## THE HEADLINE DEFECT: SYS-4, AND IT IS INVERTED

pi paints a selection BACKGROUND in exactly TWO components — \`tree-selector.ts:748-753\` and
\`session-selector.ts:506-508\` — and NEVER in \`SelectList\`.
**cyrup does precisely the opposite**: \`SelectList\` paints a \`selectedBg\` bar on eight dialogs pi
never fills, and the two components pi DOES fill are the two cyrup leaves unfilled.

So this is not "add a background" or "remove a background" — it is a swap, and getting it half-right
is worse than leaving it alone. Establish the true upstream set yourself before editing: read both
cited components AND \`select-list.ts\` and confirm SelectList genuinely never fills. The audit already
corrected one wrong claim here ("selectedBg is used in exactly one place upstream" — it is two), so
do not trust a summary over the source.

## WHAT PREVIOUS BATCHES ESTABLISHED — do not re-litigate

- The palette DATA is clean; all 50 tokens match \`dark.json\`/\`light.json\`.
- No hardcoded RGB colours in the renderers.
- \`cyrup-tui\` has NO snapshot/golden machinery. Nothing to regenerate.
- TUI batch 1 fixed the colour ACCESSORS (\`dim_style()\` resolves the \`dim\` token; \`error_style()\`
  no longer bakes in bold). TUI batch 2 fixed the footer/status band. Read \`theme.rs\` for what an
  accessor does now rather than assuming.

## HARD RULES

- **READ pi's source for every change. Never infer from a name or from this brief.** Briefs here have
  been wrong repeatedly, including mine in the last batch (I asserted pi hides footer segments as the
  terminal narrows — it does not, it truncates in a ladder). If the source disagrees, FOLLOW THE
  SOURCE and say so in your report.
- **Verify every citation**: \`git -C ${WS}/pi show v0.84.1:packages/<path>\` and COUNT the lines.
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.**
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` files are separate crates needing their
  own file-level \`#![allow(...)]\`.
- **Never weaken an assertion.** Report every edited test with the assertion BEFORE and AFTER.
- **Watch for arithmetic that can overflow or index out of range.** The last batch introduced a
  \`pub fn\` that panicked on \`u64\` overflow; this batch is full of width arithmetic
  (\`width - prefixWidth - gutter\`), which is the same hazard in subtraction form. Saturating
  arithmetic, and a test at tiny widths (1, 2, 5 columns).

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\` from it. **NEVER \`git checkout\`**:
the tree holds uncommitted work and an agent wiped its own work doing exactly that.
A revert must remove EVERY guard the test covers. **And check your test actually fails**: in the last
batch an agent's narrow-width test used a terminal wide enough that the truncation path never ran, so
it passed against reverted code. If a revert leaves your test green, the test is wrong.

## BUILD DISCIPLINE

Prefix cargo with \`CARGO_INCREMENTAL=0\`. MAY run \`cargo check/test/clippy -p cyrup-tui --all-targets\`.
**MUST NOT run \`--workspace\`** — the orchestrator gates once. If you must touch a file outside
\`cyrup-tui\`, say so explicitly in \`notes\` so the workspace gate looks for it.
Finish with \`cargo clippy -p cyrup-tui --all-targets; echo "exit=$?"\`.
`

const ITEM_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['items'],
  properties: {
    items: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['id', 'status', 'summary', 'revert_proof'],
        properties: {
          id: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'already-correct', 'blocked'] },
          summary: { type: 'string' },
          upstream_citation: { type: 'string' },
          user_visible: { type: 'string' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string' },
        },
      },
    },
    selectedbg_inventory: {
      type: 'string',
      description: 'The definitive list: which pi components fill a selection bg, which cyrup ones do, before and after.',
    },
    tests_changed: { type: 'string' },
    files_changed: { type: 'array', items: { type: 'string' } },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Fix')

const fixed = await agent(
  `${COMMON}

## Your items: SYS-4 (S1+S2), then S25, S26, S27, S3, S10, S24, S28. Read those rows first.

**SYS-4 FIRST — the inversion.** Two edits per the audit (\`select_list.rs\` around :222-251 and the
two row builders), but verify the line numbers yourself; earlier batches shifted this file.
  - Remove the \`selectedBg\` fill from \`SelectList\` (eight dialogs get a bar pi never draws).
  - ADD it to the two components pi does fill: \`/tree\` (\`tree-selector.ts:748-753\`) and \`/resume\`
    (\`session-selector.ts:506-508\`).
  - Then establish what pi uses INSTEAD in SelectList — \`theme.ts:1293-1294\` \`selectedPrefix\` /
    \`selectedText\` per the S1 row. Removing the bar without adding pi's actual selection styling
    would leave selection invisible, which is a worse bug than the one you are fixing. Prove the
    selected row is still distinguishable in a test.

**S25/S26/S27 — SelectList geometry.** The right safety gutter (\`select-list.ts:169\`), the primary
column reduction over ALL filtered rows (\`:180-184\`), and label truncation width (\`:150-152\`). These
are width arithmetic: use saturating subtraction and test at 1, 2 and 5 columns.

**S3 — the ListSelector hint/keybinding row** (\`extension-selector.ts:63-73\`, \`rawKeyHint\`).
**S10 — the \`/resume\` cursor glyph** (\`session-selector.ts:476\`).
**S24 — the \`/tree\` fold marker and per-role prefixes** (\`tree-selector.ts:734-735\`).
**S28 — the ListSelector row inset** (\`extension-selector.ts:87\`, \`new Text(text, 1, 0)\`).

Fill \`selectedbg_inventory\` with the definitive before/after mapping — that is how I will judge
whether the swap is complete rather than half-done.

For each item: read pi, fix, add a test that FAILS without the fix plus a MIRROR that stays green,
run the revert proof against a cp-backup of the FIXED state. Then clippy.`,
  { label: 'fix:selection', phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
)

log(`Fix phase: ${(fixed?.items || []).length} items`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'selection_render'],
  properties: {
    tree_restored: { type: 'boolean' },
    selection_render: {
      type: 'string',
      description: 'For each dialog: render it with a row selected and state whether a bg is filled, matching pi component by component.',
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

const digest = (fixed?.items || [])
  .map(
    (a) => `### ${a.id} [${a.status}] ${a.summary}
UPSTREAM: ${a.upstream_citation || '(none)'}
USER-VISIBLE: ${a.user_visible || '(not stated)'}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 600)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. Do NOT fix; report.

**Lens 1 — is the swap COMPLETE and correct in both directions?** Enumerate every dialog cyrup
renders with a selectable list. For each, render it with a row selected and record whether a
background is filled. Compare against the pi component it ports. Fill \`selection_render\`.
A dialog that lost its bar but gained no other selection styling is a BLOCKER: selection would be
invisible. A dialog that kept a bar pi does not draw means the swap is half-done.

**Lens 2 — width arithmetic.** S25/S26/S27 are all \`width - something\` expressions. Drive every
changed list at widths 1, 2, 5, 20 and 200. Look for subtraction overflow, slice panics, and rows
that render wider than the terminal. The previous batch shipped a \`pub fn\` that panicked on
overflow, so this is a live failure mode here, not a hypothetical.

**Lens 3 — weakened assertions.** \`git diff\` every edited pre-existing test and rule on each:
updated to pi's actual value with the pi line quoted, or loosened? Name each verdict, and check for
renames and shrunken bodies.

**Lens 4 — revert proofs that actually bite.** Re-run each yourself with cp-backup/cp-restore, never
\`git checkout\`. **Specifically check that each test exercises the path it claims**: an earlier
narrow-width test used a terminal too wide for its own truncation branch and passed against reverted
code. If a revert leaves a test green, report it.

**Lens 5 — citations.** Open every cited pi line at v0.84.1 and COUNT.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-tui\` is green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`.

## What the author reported

${digest}

SELECTEDBG INVENTORY: ${fixed?.selectedbg_inventory || '(none reported)'}
TESTS CHANGED: ${fixed?.tests_changed || '(none reported)'}
FILES: ${(fixed?.files_changed || []).join(', ')}
NOTES: ${fixed?.notes || '(none)'}`,
  { label: 'verify:selection', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-3',
  items: (fixed?.items || []).map((i) => ({ id: i.id, status: i.status })),
  selection_render: review?.selection_render,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
