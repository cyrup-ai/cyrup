export const meta = {
  name: 'cyrup-tui-batch-04-dialog-envelopes',
  description: 'TUI batch 4 — SYS-3 dialog half: the Spacer(1) rows six dialogs never draw (L4,S20,E5,E6,E7)',
  phases: [
    { title: 'Fix', detail: 'one crate, sequential; extract the shared envelope helper' },
    { title: 'Verify', detail: 'ONE reviewer; row-count arithmetic against pi component by component' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are fixing PRESENTATION FIDELITY defects in \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against
pi (${WS}/pi) at **v0.84.1**. The audit is \`${WS}/cyrup/TUI-FIDELITY.md\` — read §SYS-3 and rows
L4, S20, E5, E6, E7 before touching anything.

## WHAT THIS BATCH IS

**SYS-3: pi's layout language is \`Spacer(1)\` between every structural element. cyrup's
\`Layout::vertical\` regions are adjacent by default and nobody ever added the blank rows back.**
This is the single largest contributor to "cyrup looks cramped" — pi breathes, cyrup is a wall.

This batch does the DIALOG half: six dialog envelopes that are each missing 2-5 blank rows. The
transcript half (L1, L3, X2, X3) is TUI batch 5 and the loader/startup rows (C7, C13) already
landed in batch 2 — do NOT touch those here, or batch 5's diff becomes unreadable.

The audit suggests extracting a shared envelope helper while you are in here. Do it if the six sites
genuinely share a shape — but verify that against pi first: if pi's six components differ in their
spacer pattern, a helper that flattens them into one shape is a NEW divergence dressed as cleanup.
Report which you found.

## THE MISTAKE THIS BATCH MUST NOT REPEAT

The last batch applied a hint row and a 1-column inset to a SHARED engine, giving ~10 dialogs chrome
that pi draws on 4 and 6 respectively. **A property of one pi COMPONENT is not a property of every
dialog.** Before you add a spacer to a shared code path, enumerate which pi components have it and
gate accordingly. State the enumeration in your report.

That same batch also worked from an audit row that was WRONG (it claimed pi never emits a glyph pi
does emit, from a misread \`!showsFoldInConnector\` guard). The audit has since been corrected, but the
lesson stands: **the audit is a lead, the source is the truth.**

## WHAT PREVIOUS BATCHES ESTABLISHED — do not re-litigate

- The palette DATA is clean; no hardcoded RGB in renderers; NO snapshot machinery in this crate.
- Batch 1 fixed the colour accessors; batch 2 the footer/status band; batch 3 the \`selectedBg\`
  inversion, the hint-row/inset gating, and the tree connector. Read the current code rather than
  assuming what an accessor or a selector does.

## HARD RULES

- **READ pi's source for every change. Never infer from a name, this brief, or the audit.** Quote the
  \`file:line\` you read. If the source disagrees, FOLLOW THE SOURCE and say so.
- **Verify every citation**: \`git -C ${WS}/pi show v0.84.1:packages/<path>\` and COUNT the lines.
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.**
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` files are separate crates needing their
  own file-level \`#![allow(...)]\`.
- **Never weaken an assertion.** Report every edited test BEFORE/AFTER, and whether it fails under
  the matching revert.
- **Height arithmetic is this batch's hazard.** Adding rows to a fixed-height region either pushes
  content off the bottom or panics on a small terminal. Every dialog must degrade gracefully: test
  each at heights 1, 3, 5 and its natural height. Batch 2 shipped a hint block that rendered a blank
  row instead of its content on a one-row terminal — do not repeat that shape.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\`. **NEVER \`git checkout\`.**
**If a revert leaves your test green, the test is wrong** — that has happened twice now, once because
the test drove a width where the branch never ran, once because the revert was applied at the
dispatch layer while the defect lived in the render layer. Revert where the defect lives.

## BUILD DISCIPLINE

\`CARGO_INCREMENTAL=0\`; \`cargo check/test/clippy -p cyrup-tui --all-targets\` only.
**NEVER \`--workspace\`.** Finish with \`cargo clippy -p cyrup-tui --all-targets; echo "exit=$?"\`.
`

const ITEM_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['items', 'spacer_inventory'],
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
          rows_before_after: { type: 'string', description: 'Row count before, after, and pi\'s — with the pi children counted' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string' },
        },
      },
    },
    spacer_inventory: {
      type: 'string',
      description: 'Per pi component: its exact child list with Spacer positions, and cyrup\'s before/after. This is how completeness is judged.',
    },
    shared_helper: { type: 'string', description: 'Did the six share a shape? If you extracted a helper, what does it assume, and which components opted out?' },
    tests_changed: { type: 'string' },
    files_changed: { type: 'array', items: { type: 'string' } },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Fix')

const fixed = await agent(
  `${COMMON}

## Your items: L4, S20, E5, E6, E7. Read those rows, then read each pi component IN FULL.

**L4 — dialog envelope \`Spacer(1)\` rows.** \`trust-selector.ts:52-87\` has FIVE; \`extension-selector.ts\`
has its own count. Six dialogs, 2-5 rows each. Count the children of each pi component yourself and
write the child list into \`spacer_inventory\` — that inventory is how I will judge whether this is
complete rather than approximately right.

**S20 — \`/trust\` saved-decision checkmark + header spacing** (\`trust-selector.ts:110\`).

**E5 — extension \`ui.editor\` bottom border** (\`extension-editor.ts:62\` opens with a
\`DynamicBorder\`; establish what closes it).

**E6 — extension \`ui.input\` hint row** (\`extension-input.ts:66-68\`, a \`Text\` with
\`keyHint("tui.select…")\`). Batch 3 made hint rows opt-in per kind — check whether this one is
already covered by that gating before adding a second mechanism.

**E7 — extension dialog envelope spacers** (\`extension-input.ts:47-70\`: border / Spacer / title / …).

For each: read pi, count rows, fix, add a test that FAILS without the fix plus a MIRROR that stays
green, run the revert proof against a cp-backup of the FIXED state, and test the height ladder
(1, 3, 5, natural). Then clippy.

Fill \`rows_before_after\` per item with the arithmetic, and \`shared_helper\` with whether the six
genuinely share a shape.`,
  { label: 'fix:envelopes', phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
)

log(`Fix phase: ${(fixed?.items || []).length} items`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'row_audit'],
  properties: {
    tree_restored: { type: 'boolean' },
    row_audit: {
      type: 'string',
      description: 'Per dialog: cyrup\'s rendered row list vs pi\'s child list, position by position. Name every extra and missing blank.',
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
ROWS: ${a.rows_before_after || '(not counted)'}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 600)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. Do NOT fix; report.

**Lens 1 — render every dialog and count rows against pi.** For each of the six, render it and list
the rows in order; beside it, list the pi component's children from the source. Fill \`row_audit\`
position by position. A blank in the wrong POSITION is as wrong as a missing one, and it will not
show up in a row-count check — compare positions, not totals.

**Lens 2 — over-application.** The previous batch gave ~10 dialogs chrome pi draws on 4. Check every
spacer added here is gated to the components pi actually spaces. If a shared helper was extracted,
verify it did not flatten six different pi shapes into one — read all six pi components and say
whether they truly agree.

**Lens 3 — height degradation.** Drive each dialog at heights 1, 2, 3, 5 and natural. Confirm none
panics, none renders a blank where its content should be (batch 2 shipped exactly that), and none
pushes its primary content off-screen. Added rows make this strictly more likely.

**Lens 4 — weakened assertions.** \`git diff\` every edited pre-existing test and rule on each. Check
renames and shrunken bodies too.

**Lens 5 — revert proofs that bite.** Re-run each with cp-backup/cp-restore, never \`git checkout\`.
**Revert where the defect lives**, not at a dispatch layer above it — an earlier reviewer's revert
left every test green for exactly that reason and had to be redone.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-tui\` is green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`.

## What the author reported

${digest}

SPACER INVENTORY: ${fixed?.spacer_inventory || '(none reported)'}
SHARED HELPER: ${fixed?.shared_helper || '(none reported)'}
TESTS CHANGED: ${fixed?.tests_changed || '(none reported)'}
FILES: ${(fixed?.files_changed || []).join(', ')}
NOTES: ${fixed?.notes || '(none)'}`,
  { label: 'verify:envelopes', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-4',
  items: (fixed?.items || []).map((i) => ({ id: i.id, status: i.status })),
  row_audit: review?.row_audit,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
