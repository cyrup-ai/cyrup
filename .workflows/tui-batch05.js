export const meta = {
  name: 'cyrup-tui-batch-05-transcript-rhythm',
  description: 'TUI batch 5 — SYS-3 transcript half: the blank rows every message block is missing (L1,L3,L5,L6,X1-X5,X10,X16-X18)',
  phases: [
    { title: 'Fix', detail: 'one crate, sequential — transcript.rs, bash.rs, app.rs' },
    { title: 'Verify', detail: 'ONE reviewer; render a whole conversation and diff row by row' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are fixing PRESENTATION FIDELITY defects in \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against
pi (${WS}/pi) at **v0.84.1**. The audit is \`${WS}/cyrup/TUI-FIDELITY.md\` — read §SYS-3 and rows
L1, L3, L5, L6, X1-X5, X10, X16, X17, X18 before touching anything.

## THIS IS THE BATCH THAT FIXES "CYRUP LOOKS CRAMPED"

The transcript is the screen a user stares at all day, and it is where the vertical-rhythm gap is
worst. Per the audit: every tool block, every user message and every \`[skill]\`/\`[branch]\`/
\`[compaction]\` block loses the tinted blank row above AND below its content; every assistant reply
loses its leading blank. The whole UI reads as a dense wall where pi breathes.

Batch 4 just did the DIALOG half of the same systemic defect and established the governing fact,
which you should not re-derive:

  **pi has NO "does it fit" gate anywhere.** A component is a \`Container\` whose \`render(width)\`
  takes no height at all (\`tui/src/tui.ts:211-245\`); \`Spacer(1)\` is one \`""\` per line
  (\`components/spacer.ts:21-27\`). Every spacer is emitted on every frame. Height is decided one
  level up, where \`layoutComponent\` (\`layout.ts:113\`) renders at natural height and paints a
  window — so **the rendered rows are a strict PREFIX of the natural render**.

Apply that same shape here. Do not invent a fit gate; emit the rows and let the window clamp.

## THE MISTAKES THIS EFFORT HAS ALREADY MADE — do not repeat them

1. **A property of one pi component is not a property of every block.** Batch 3 applied a hint row
   and an inset to a shared engine and gave ~10 dialogs chrome pi draws on 4 and 6. Before adding a
   blank to a shared render path, enumerate which pi components emit it.
2. **The audit has been wrong.** Batch 3's S24 row claimed pi never emits a glyph pi does emit,
   from a misread \`!showsFoldInConnector\` guard. The audit is a LEAD; the source is the truth.
3. **A fix can be inert.** Batch 4's \`/config\` spacers were gated behind a height check that never
   passed, so the fix rendered nothing. After each change, render the thing and LOOK at the rows.
4. **A revert that leaves tests green means the test is wrong** — that has happened three times.
   Revert where the defect lives, not at a dispatch layer above it.

## HARD RULES

- **READ pi's source for every change. Never infer from a name, this brief, or the audit.** Quote the
  \`file:line\` you read. If the source disagrees, FOLLOW THE SOURCE and say so.
- **Verify every citation**: \`git -C ${WS}/pi show v0.84.1:packages/<path>\` and COUNT the lines.
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.**
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` files are separate crates needing their
  own file-level \`#![allow(...)]\`.
- **Never weaken an assertion.** Report every edited test BEFORE/AFTER and whether it fails under the
  matching revert. Many transcript tests pin exact row vectors and WILL churn — each change must be
  justified against a quoted pi line.
- **L6 is a correctness trap, not a spacing one**: the tool-block background is measured in CHARS
  where pi measures COLUMNS (\`box.ts:127-131\`). Any CJK, emoji or combining mark makes the fill the
  wrong width. Use a width-aware measure and test with a wide-character string.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\`. **NEVER \`git checkout\`**: an agent
destroyed a 1300-line uncommitted file that way.

## BUILD DISCIPLINE

\`CARGO_INCREMENTAL=0\`; \`cargo check/test/clippy -p cyrup-tui --all-targets\` only.
**NEVER \`--workspace\`.** Finish with \`cargo clippy -p cyrup-tui --all-targets; echo "exit=$?"\`.
`

const ITEM_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['items', 'block_inventory'],
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
          rows_before_after: { type: 'string' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string' },
        },
      },
    },
    block_inventory: {
      type: 'string',
      description: 'Per transcript block type: pi\'s child list with blank positions, and cyrup before/after. Completeness is judged on this.',
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

## Your items: L1, L3, L5, L6, X1, X2, X3, X4, X5, X10, X16, X17, X18. Read those rows first.

Suggested order — the structural ones first, because the rest sit inside them:

**L1 — \`Box\` paddingY.** The tinted blank row above AND below the content of every tool block, user
message, and \`[skill]\`/\`[branch]\`/\`[compaction]\`/\`[custom]\` block. Read \`box.ts\` and establish
exactly which components pass a non-zero paddingY — do not assume all of them do.
**L3 — the assistant message leading blank** (\`assistant-message.ts:100-102\`, gated on
\`hasVisibleContent\`; port the gate, not just the blank).
**X2 — the label block** for \`[skill]\`/\`[branch]\`/\`[compaction]\`/\`[custom]\`
(\`custom-message.ts:88\` onward), including the \`Spacer(1)\` after the label that batch 1 deliberately
left for this batch.
**X1 — role labels and the streaming caret.** cyrup draws \`you: \`/\`assistant: \` labels and a \`▌\`
caret that pi does not. Verify against \`assistant-message.ts:110-114\` and the user-message component
before removing anything — batch 3 deleted a glyph on exactly this reasoning and was wrong.
**L5 — tool block right inset** (\`box.ts:79-88\`, \`contentWidth = width - paddingX*2\`).
**L6 — the bg fill measured in COLUMNS not chars** (\`box.ts:127-131\`). Correctness trap; see above.
**X3, X4, X10, X16, X17 — the bash block**: \`!\`/\`!!\` indent and blank rows, the \`Running…\` row, the
blanks before warnings and \`Took Ns\`, the expand/collapse wording, and the \`(exit N)\` badge for a
SIGNALLED command (\`bash-execution.ts:105-109\` — check what pi shows when \`exitCode === null\`).
**X5 — the reasoning/thinking block body** (\`assistant-message.ts:146-164\`).
**X18 — compaction/retry status duplicated into scrollback** (\`status-indicator.ts:42-72\`): cyrup
writes a transcript line pi only ever shows in the status band. Verify before deleting.

For each: read pi, fix, render the block and LOOK at the rows, add a test that FAILS without the fix
plus a MIRROR that stays green, run the revert proof against a cp-backup of the FIXED state.

Fill \`block_inventory\` with pi's child list per block type and cyrup's before/after — that is how
completeness will be judged.`,
  { label: 'fix:transcript', phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
)

log(`Fix phase: ${(fixed?.items || []).length} items`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'conversation_render'],
  properties: {
    tree_restored: { type: 'boolean' },
    conversation_render: {
      type: 'string',
      description: 'Render a full conversation (user turn, assistant reply, tool call, bash block, skill block) and list every row beside pi\'s expected rows.',
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

**Lens 1 — render a WHOLE CONVERSATION and diff it row by row.** Build a transcript containing: a
user turn, an assistant reply with text, a tool call, a bash block with output and a non-zero exit, a
\`[skill]\` block, and a reasoning block. Render it. Beside each row, write what pi's components would
emit, read from source. Fill \`conversation_render\`. A blank in the wrong POSITION is as wrong as a
missing one and will not show up in a count — compare positions.

**Lens 2 — did any block get a blank pi does not give it?** Enumerate which pi components pass a
non-zero paddingY and which do not. A shared helper applied to all block types would be the batch-3
mistake repeated.

**Lens 3 — L6, the column-width trap.** Test the tool-block background fill with CJK, an emoji, a
combining mark and a zero-width joiner sequence. Confirm the fill width matches the rendered column
width, not the char count, and that nothing panics or slices mid-grapheme.

**Lens 4 — weakened assertions.** Transcript tests pin exact row vectors, so this batch will have
churned many. \`git diff\` each and rule: updated to pi's actual rows with a pi line quoted, or
loosened? Name every verdict. Watch for a row-vector assertion replaced by a \`contains\` check —
that is a weakening even though it still asserts something.

**Lens 5 — revert proofs and inertness.** Re-run each revert with cp-backup/cp-restore, never
\`git checkout\`. Then check the opposite failure: is any fix INERT — emitted behind a condition that
never holds in a real render? Batch 4 shipped exactly that. Render, do not reason.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-tui\` is green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`.

## What the author reported

${digest}

BLOCK INVENTORY: ${fixed?.block_inventory || '(none reported)'}
TESTS CHANGED: ${fixed?.tests_changed || '(none reported)'}
FILES: ${(fixed?.files_changed || []).join(', ')}
NOTES: ${fixed?.notes || '(none)'}`,
  { label: 'verify:transcript', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-5',
  items: (fixed?.items || []).map((i) => ({ id: i.id, status: i.status })),
  conversation_render: review?.conversation_render,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
