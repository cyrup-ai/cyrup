export const meta = {
  name: 'cyrup-tui-batch-06-markdown',
  description: 'TUI batch 6 — markdown rendering (M1-M6, M8, M11, M14, M16, M17; M5/M9/M10 belong to batch 8)',
  phases: [
    { title: 'Fix', detail: 'one crate, one file mostly — markdown.rs' },
    { title: 'Verify', detail: 'ONE reviewer; render real documents and diff' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are fixing PRESENTATION FIDELITY defects in \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against
pi (${WS}/pi) at **v0.84.1**. The audit is \`${WS}/cyrup/TUI-FIDELITY.md\` — read the M rows for your
items before touching anything. pi's renderer is
\`packages/tui/src/components/markdown.ts\`; cyrup's is \`crates/cyrup-tui/src/markdown.rs\`.

## SCOPE — what is NOT in this batch

**M5, M9, M10 are the WRAP REWRITE and belong to batch 8.** M9 is content width, M10 the right
margin, M5 the wrapped list-item hanging indent — all three are the same root cause: cyrup wraps
OUTSIDE its own padding, so line 1 starts at column 1 and lines 2..N at column 0. Do not touch
wrapping here; batch 8 moves it inside the padded render and needs a clean base.
**M7, M12, M13, M15** are \`large\` items scheduled for batch 10 (inline formatting in table cells,
LaTeX, extension transformers/mermaid, table minimum column width). Not here.

So your items are: **M1, M2, M3, M4, M6, M8, M11, M14, M16, M17.**

## WHAT PREVIOUS BATCHES ESTABLISHED — do not re-derive

- pi has NO "does it fit" gate: a component is a \`Container\` whose \`render(width)\` takes no height
  (\`tui/src/tui.ts:211-245\`); visible rows are a strict PREFIX of the natural render
  (\`layout.ts:113\`).
- Batch 1 fixed the colour accessors; batch 5 made \`wrap_line\` and \`apply_bg\` both GRAPHEME-aware
  (\`unicode-segmentation\`, matching \`utils.ts:775-798\`). Do not reintroduce per-\`char\` measurement —
  M1/M11 are full of column arithmetic and that is exactly where it creeps back in.
- The unclassified-code-span green fallback (T5) is already fixed: pi pushes unclassified spans with
  NO style (\`markdown.ts:526-527\`). Read \`markdown.rs\` for what it does now.

## THE MISTAKES THIS EFFORT HAS MADE — do not repeat them

1. **The audit has been WRONG three times** (S24's fold glyph, X17's signalled-exit row, S3/S28's
   scope). It is a LEAD; the source is the truth. If they disagree, follow the source and SAY SO.
2. **A property of one component is not a property of all.** Batch 3 gave ~10 dialogs chrome pi draws
   on 4; batch 5 nearly generalised a \`Spacer\` across four label blocks that differ.
3. **A fix can be INERT** — batch 4 shipped spacers behind a condition that never held. After each
   change, render a real document and LOOK at it.
4. **A revert that leaves tests green means the test is wrong.** Revert where the defect lives.

## HARD RULES

- **READ pi's source for every change. Never infer from a name, this brief, or the audit.** Quote the
  \`file:line\` you read and COUNT the lines
  (\`git -C ${WS}/pi show v0.84.1:packages/<path>\`).
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.**
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` files are separate crates needing their
  own file-level \`#![allow(...)]\`.
- **Never weaken an assertion.** Report every edited test BEFORE/AFTER and whether it fails under the
  matching revert.
- **Column arithmetic is this batch's hazard** (tables especially). Use saturating arithmetic and
  grapheme-aware width. Test tables containing CJK, emoji and combining marks, and at widths too
  narrow for the grid.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\`. **NEVER \`git checkout\`.**

## BUILD DISCIPLINE

\`CARGO_INCREMENTAL=0\`; \`cargo check/test/clippy -p cyrup-tui --all-targets\` only.
**NEVER \`--workspace\`.** Finish with \`cargo clippy -p cyrup-tui --all-targets; echo "exit=$?"\`.
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
          rendered_before_after: { type: 'string', description: 'The actual rendered rows before and after' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string' },
        },
      },
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

## Your items: M1, M2, M3, M4, M6, M8, M11, M14, M16, M17. Read those rows, then read pi's renderer.

**M1 — table border glyphs and \`│\` separators** (\`markdown.ts:956\` onward). Quote pi's exact glyph
set and match it character for character.
**M2 — table header cells** (\`:966-970\`, \`this.theme.bold(padded)\` — pure SGR, note what it does NOT
add).
**M3 — nested list indent** (\`:758\`, \`"    ".repeat(depth)\` — FOUR spaces; check what cyrup uses).
**M4 — task-list item marker** (\`:770-774\`, bullet then \`[${'{'}checked${'}'}]\`).
**M6 — the blank row after a \`---\` rule** (\`:605-610\`; note the condition on the following token).
**M8 — soft line break inside a paragraph** (marked keeps the \`\\n\` in the text token; establish what
pi does with it).
**M11 — a table narrower than its grid** (\`:852-861\`, the \`availableForCells < numCols\` branch).
This is the one most likely to panic on narrow widths — saturating arithmetic, and test it.
**M14 — links when the terminal supports OSC-8** (\`:692-696\`, gated on \`getCapabilities().hyperlinks\`).
Note cyrup already had an OSC-8 escaping bug fixed earlier in this project — check how hyperlinks are
emitted safely before adding another emitter.
**M16 — single-tilde strikethrough** (\`:7-24\`, \`STRICT_STRIKETHROUGH_REGEX\`; port the regex's intent,
not a loose approximation).
**M17 — the code-fence info string** (\`:522\`).

For each: read pi, fix, RENDER a real document and look at the rows, add a test that FAILS without
the fix plus a MIRROR that stays green, run the revert proof against a cp-backup of the FIXED state.
Fill \`rendered_before_after\` with actual rows — not a description of them.`,
  { label: 'fix:markdown', phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
)

log(`Fix phase: ${(fixed?.items || []).length} items`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'document_render'],
  properties: {
    tree_restored: { type: 'boolean' },
    document_render: {
      type: 'string',
      description: 'Render a document exercising every item (table, nested+task lists, hr, soft break, links, strikethrough, fenced code) and diff row by row against pi.',
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
RENDERED: ${(a.rendered_before_after || '(not shown)').slice(0, 500)}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 500)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. Do NOT fix; report.

**Lens 1 — render a document that exercises every item and diff it row by row.** Include: a table
(with a CJK cell and an emoji cell), nested lists three deep, a task list, an \`---\` rule, a paragraph
with a soft line break, a link, single- and double-tilde strikethrough, and a fenced block with an
info string. Beside each row write what pi's \`markdown.ts\` emits, read from source. Fill
\`document_render\`.

**Lens 2 — column arithmetic.** Batch 5 had to fix a helper that measured per-\`char\` while its
partner measured per-grapheme. Check every width computation added here is grapheme-aware and
saturating. Drive tables at widths 1, 5, 20 and narrower than the column count. Nothing may panic,
slice mid-grapheme, or render wider than the terminal.

**Lens 3 — did anything from batches 1-5 regress?** This file was touched by the T5 syntax-fallback
fix and the batch-5 grapheme work. Confirm unclassified code spans still carry NO foreground, and
that \`wrap_line\`/\`apply_bg\` still agree.

**Lens 4 — scope discipline.** M5, M9, M10 belong to batch 8 (the wrap rewrite) and M7, M12, M13, M15
to batch 10. Confirm none of them was partially done here — a half-landed wrap change would make
batch 8 unreadable. If the author touched wrapping at all, report it.

**Lens 5 — weakened assertions, reverts, inertness.** \`git diff\` every edited test and rule on each.
Re-run each revert with cp-backup/cp-restore, never \`git checkout\`; revert where the defect lives.
Then check the opposite: is any fix INERT — behind a condition that never holds in a real render?

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-tui\` is green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`.

## What the author reported

${digest}

TESTS CHANGED: ${fixed?.tests_changed || '(none reported)'}
FILES: ${(fixed?.files_changed || []).join(', ')}
NOTES: ${fixed?.notes || '(none)'}`,
  { label: 'verify:markdown', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-6',
  items: (fixed?.items || []).map((i) => ({ id: i.id, status: i.status })),
  document_render: review?.document_render,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
