export const meta = {
  name: 'cyrup-tui-batch-08-wrap-rewrite',
  description: 'TUI batch 8 — SYS-2: move wrapping INSIDE the padded render (L2, M5, M9, M10)',
  phases: [
    { title: 'Study', detail: 'read pi\'s wrap pipeline end to end before editing anything' },
    { title: 'Fix', detail: 'the rewrite, in one crate' },
    { title: 'Verify', detail: 'ONE reviewer; the left edge is the whole point' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are performing the largest single change in the TUI fidelity backlog for \`cyrup-tui\`
(${WS}/cyrup/crates/cyrup-tui), against pi (${WS}/pi) at **v0.84.1**. The audit is
\`${WS}/cyrup/TUI-FIDELITY.md\` — read §SYS-2 and rows L2, M5, M9, M10 first.

## THE DEFECT, AND WHY IT IS THE HEADLINE ITEM

**cyrup wraps OUTSIDE its own padding.**
\`markdown::render\` never wraps (its \`width\` argument only reaches \`Event::Rule\` and \`emit_table\`);
\`pad_lines\` inserts an indent into the LOGICAL line; then ratatui's \`Paragraph::wrap\` reflows the
result at FULL FRAME WIDTH.

Consequence: on any message longer than one terminal row — which is most of them — **row 1 starts at
column 1 and rows 2..N start at column 0**, and no row has a right margin. A ragged left edge on
nearly every turn is the single most unfinished-looking thing in the UI.

pi does the opposite: it wraps at \`contentWidth\` FIRST and then prefixes the margin to every produced
row (\`markdown.ts:322-348\` — \`leftMargin + line + rightMargin\`; \`box.ts:85-88\` renders the child at
\`contentWidth = width - paddingX*2\`).

**Fix shape:** wrap inside \`markdown::render\` at \`width - 2*pad\`, margin every produced row.

## WHAT THIS BATCH ALSO OWES

- **M5 — the list-item hanging indent.** pi builds TWO prefixes per item and picks per row
  (\`markdown.ts:774-775\`, \`:789\`): \`firstPrefix = indent + marker\`, \`continuationPrefix = indent +
  " ".repeat(visibleWidth(marker))\`. Batch 6 landed the PREFIX half for soft breaks only and
  deliberately left \`itemWidth = Math.max(1, width - visibleWidth(firstPrefix))\` (\`:776\`) and the
  wrap loop (\`:788\`) to you. Finish it.
- **M9 — content width** (\`assistant-message.ts:111\`, \`new Markdown(text, this.outputPad, 0, …)\`).
- **M10 — the right margin** (\`:330\`/\`:340\`).
- The right-margin halves of **L5** (tool block) and **X3** (bash \`$ cmd\` row), both explicitly
  deferred to this batch by earlier ones.

## WHAT PREVIOUS BATCHES ESTABLISHED — do not re-derive, and do not regress

- pi has NO "does it fit" gate: a component's \`render(width)\` takes no height
  (\`tui/src/tui.ts:211-245\`); visible rows are a strict PREFIX of the natural render.
- \`Text.render\` wraps at \`width - paddingX*2\` and prefixes \`leftMargin\` to every row
  (\`text.ts:60-87\`). Batch 5 ported an equivalent (\`transcript::text_lines_of\`) — REUSE it rather
  than writing a third wrapper.
- **THREE separate width measurements in this crate carried the same char-vs-grapheme defect** and
  were fixed one batch at a time: \`wrap_line\` (batch 5), \`wrap_cell\` (batch 6), \`word_wrap_line\`
  (batch 7). This batch is ENTIRELY width arithmetic. Every measurement you write or touch must be
  grapheme-aware (\`unicode-segmentation\` + display width), and \`word_wrap_line\` in \`editor.rs\` is
  now a faithful port of pi's \`wordWrapLine\` — read it before writing a fourth wrapper.

## HARD RULES

- **READ pi's source. Never infer from a name, this brief, or the audit.** The audit has been WRONG
  FOUR times in this effort (S24's fold glyph, X17's exit row, S3/S28's scope, \`sliceByColumn\`'s
  \`strict\` flag) and each time the agent was right to follow the source. Quote the \`file:line\` and
  COUNT (\`git -C ${WS}/pi show v0.84.1:packages/<path>\`).
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.**
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` need their own file-level \`#![allow]\`.
- **Never weaken an assertion.** This rewrite will churn many row-vector tests. Every changed
  expectation must be justified against a quoted pi line. Report each BEFORE/AFTER.
- **EVERY fix needs a test that fails without it.** Before finishing, revert each change in turn and
  confirm something goes red. Batch 7 shipped SIX untested fixes and only found out by doing this.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\`. **NEVER \`git checkout\`.**

## BUILD DISCIPLINE

\`CARGO_INCREMENTAL=0\`; \`cargo check/test/clippy -p cyrup-tui --all-targets\` only.
**NEVER \`--workspace\`.** Finish with \`cargo clippy -p cyrup-tui --all-targets; echo "exit=$?"\`.
`

phase('Study')

const STUDY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['pipeline', 'cyrup_pipeline', 'plan'],
  properties: {
    pipeline: { type: 'string', description: 'pi\'s wrap pipeline end to end, with file:line at each step' },
    cyrup_pipeline: { type: 'string', description: 'cyrup\'s current path, and every place a width is computed or a row is padded' },
    plan: { type: 'string', description: 'The concrete edit sequence, and what each existing test will do' },
    risks: { type: 'string', description: 'What could regress from batches 1-7, named specifically' },
  },
}

const study = await agent(
  `${COMMON}

## Your job: STUDY ONLY. Do not edit a single file. This is the largest change in the backlog and a
## wrong mental model would be expensive.

1. **pi's pipeline, end to end.** From the component that owns the text
   (\`assistant-message.ts\`, \`box.ts\`, \`text.ts\`) down through \`markdown.ts:322-348\` and whatever
   wrapping primitive it calls in \`tui/src/utils.ts\`. Give the \`file:line\` at every step and state
   exactly WHERE the margin is applied relative to WHERE the wrap happens.
2. **cyrup's current path.** \`markdown::render\`, \`pad_lines\`, and every \`Paragraph::wrap\` in the
   crate. List every place a width is computed and every place a row is padded or indented.
3. **The concrete plan.** The edit sequence, and for each existing row-vector test, whether it will
   churn and why.
4. **Risks.** Name specifically what could regress from batches 1-7 — the grapheme discipline in
   \`wrap_line\`/\`wrap_cell\`/\`word_wrap_line\`, batch 5's transcript rhythm, batch 6's markdown rows,
   batch 4's dialog envelopes.

Be concrete and quote source. Everything downstream depends on this being right.`,
  { label: 'study:wrap-pipeline', phase: 'Study', schema: STUDY_SCHEMA, effort: 'high' }
)

const studyDigest = study
  ? `### pi's pipeline
${study.pipeline}

### cyrup's current path
${study.cyrup_pipeline}

### Plan
${study.plan}

### Risks
${study.risks || '(none named)'}`
  : '(study returned nothing — establish the pipeline yourself before editing, and say so)'

log('Study complete')

phase('Fix')

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
          rendered_before_after: { type: 'string' },
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

const fixed = await agent(
  `${COMMON}

## The pipeline study, done for this batch — use it

${studyDigest}

## Your job: perform the rewrite. L2, M5, M9, M10, plus the right-margin halves of L5 and X3.

Work in the order the study recommends. After EACH step, render a long assistant message, a long
user message, a long tool block, a long bash line and a wrapped list item, and LOOK at the left edge
of every row — that is the whole point of the batch, and a fix that leaves row 2 at column 0 is
inert regardless of what any test says.

Then run the full ladder: widths 1, 2, 5, 20, 40, 200; content containing CJK, a ZWJ family emoji, a
combining mark, and a single unbroken 500-character token.

Fill \`rendered_before_after\` with ACTUAL ROWS — before and after — for at least the assistant
message and the wrapped list item.`,
  { label: 'fix:wrap', phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
)

log(`Fix phase: ${(fixed?.items || []).length} items`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'left_edge_audit'],
  properties: {
    tree_restored: { type: 'boolean' },
    left_edge_audit: {
      type: 'string',
      description: 'For every block type, render content spanning 3+ rows and report the starting column of EVERY row, plus the right margin. Name any row that starts at a different column from its siblings.',
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
RENDERED: ${(a.rendered_before_after || '(not shown)').slice(0, 600)}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 500)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review the wrap rewrite. Do NOT fix; report.

**Lens 1 — THE LEFT EDGE. This is the batch.** For every block type (assistant message, user
message, tool block, bash block, [skill] block, blockquote, nested list item, table) render content
that spans at least three rows and report the STARTING COLUMN OF EVERY ROW plus the right margin.
Fill \`left_edge_audit\`. Any row starting at a different column from its siblings is a failure of the
batch's one job. Compare each against pi's \`leftMargin + line + rightMargin\`.

**Lens 2 — is it INERT anywhere?** A wrap that still happens in \`Paragraph::wrap\` afterwards, or a
margin applied before the wrap, would leave the defect intact while every new test passes. Grep for
remaining \`Paragraph::wrap\` calls on paths this batch was supposed to own and say what each is for.

**Lens 3 — grapheme discipline, for the FOURTH time.** Batches 5, 6 and 7 each found a width
measurement using chars instead of graphemes. This batch is entirely width arithmetic. Check every
measurement it wrote or touched. Test CJK, a ZWJ family emoji, a combining mark, and a 500-character
unbroken token at widths 1, 2, 5, 20, 40 — nothing may panic, slice mid-cluster, or exceed the width.

**Lens 4 — regressions from batches 1-7.** Re-verify: batch 5's transcript blank rows are still in
the right POSITIONS; batch 6's markdown rows (table frame, list indents, fence) are unchanged; batch
4's dialog envelopes still have their spacers; batch 7's editor still wraps CJK correctly.

**Lens 5 — weakened assertions and revert proofs.** This rewrite churns row-vector tests. \`git diff\`
every changed expectation and rule: justified against a quoted pi line, or loosened? Re-run each
revert with cp-backup/cp-restore, never \`git checkout\`. **Then revert each fix in turn and confirm
something goes red** — batch 7 shipped six untested fixes and only found out this way.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-tui\` is green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`.

## What the author reported

${digest}

TESTS CHANGED: ${fixed?.tests_changed || '(none reported)'}
FILES: ${(fixed?.files_changed || []).join(', ')}
NOTES: ${fixed?.notes || '(none)'}`,
  { label: 'verify:left-edge', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-8',
  items: (fixed?.items || []).map((i) => ({ id: i.id, status: i.status })),
  left_edge_audit: review?.left_edge_audit,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
