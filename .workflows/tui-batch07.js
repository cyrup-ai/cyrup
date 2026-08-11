export const meta = {
  name: 'cyrup-tui-batch-07-editor',
  description: 'TUI batch 7 — editor and input dialogs (E1-E4, E8-E15; E5/E6/E7 landed in batch 4)',
  phases: [
    { title: 'Fix', detail: 'one crate, sequential — editor.rs, text_input.rs, extension_editor.rs, autocomplete.rs' },
    { title: 'Verify', detail: 'ONE reviewer; drive real keystrokes, not just render' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are fixing PRESENTATION FIDELITY defects in \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against
pi (${WS}/pi) at **v0.84.1**. The audit is \`${WS}/cyrup/TUI-FIDELITY.md\` — read the E rows for your
items first. pi's editor is \`packages/tui/src/components/editor.ts\`; its input is
\`components/input.ts\`; the extension dialogs are
\`packages/coding-agent/src/modes/interactive/components/extension-{input,editor}.ts\`.

## SCOPE

Your items: **E1, E2, E3, E4, E8, E9, E10, E11, E12, E13, E14, E15.**
E5, E6 and E7 already landed in batch 4 — read the current code before assuming they are missing.

**Do NOT touch prose wrapping.** M5/M9/M10 are batch 8's wrap rewrite (cyrup wraps OUTSIDE its own
padding, so line 1 starts at column 1 and lines 2..N at column 0). E2 and E15 are ADJACENT to that —
E2 is the editor's own \`contentWidth\`, E15 is the editor slot being MEASURED at a different width
than it RENDERS at — so fix those two as measurement bugs in the editor, and leave the transcript's
prose wrap alone. Say in your report exactly where you drew the line.

## WHAT PREVIOUS BATCHES ESTABLISHED — do not re-derive

- pi has NO "does it fit" gate: a component is a \`Container\` whose \`render(width)\` takes no height
  (\`tui/src/tui.ts:211-245\`); visible rows are a strict PREFIX of the natural render
  (\`layout.ts:113\`).
- \`Text.render\` wraps at \`width - paddingX*2\` and prefixes \`leftMargin\` to every produced row
  (\`components/text.ts:60-87\`).
- Batch 5 made \`wrap_line\`/\`apply_bg\` grapheme-aware; batch 6 did the same for \`wrap_cell\`. The
  editor has its OWN width arithmetic — check it is grapheme-aware too, since a caret positioned by
  char index in a string containing an emoji lands in the wrong column.
- Batch 4 established the dialog envelope shape and made hint rows/insets opt-in PER KIND.

## THE MISTAKES THIS EFFORT HAS MADE — do not repeat them

1. **The audit has been WRONG three times.** It is a LEAD; the source is the truth. Say so if they
   disagree.
2. **A property of one component is not a property of all** — batch 3 gave ~10 dialogs chrome pi
   draws on 4.
3. **A fix can be INERT** — batch 4 shipped spacers behind a condition that never held. Render it and
   LOOK.
4. **A revert that leaves tests green means the test is wrong** — batch 6 proved one of its own tests
   inert by running the OLD test against the bug and watching it pass. Do that when you suspect it.

## HARD RULES

- **READ pi's source for every change. Never infer from a name, this brief, or the audit.** Quote the
  \`file:line\` and COUNT (\`git -C ${WS}/pi show v0.84.1:packages/<path>\`).
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.**
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` files need their own file-level
  \`#![allow(...)]\`.
- **Never weaken an assertion.** Report every edited test BEFORE/AFTER and whether it fails under the
  matching revert.
- **The editor is INTERACTIVE.** A render-only test proves little here: drive real keystrokes
  (typing, newlines, arrow keys, home/end, backspace at a boundary) and assert on the resulting
  buffer AND caret column. E13 (caret on focus loss) and E4 (scroll-to-cursor) cannot be tested any
  other way.

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
          rendered_before_after: { type: 'string' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string' },
        },
      },
    },
    wrap_boundary: { type: 'string', description: 'Exactly where you drew the line between E2/E15 and batch 8\'s wrap rewrite.' },
    tests_changed: { type: 'string' },
    files_changed: { type: 'array', items: { type: 'string' } },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Fix')

const fixed = await agent(
  `${COMMON}

## Your items. Read each E row, then read the pi component in full.

**E1 — the main editor prompt glyph.** \`editor.ts:482-601 render()\` contains NO prompt glyph; cyrup
draws \`› \`. Verify before removing — batch 3 deleted a glyph on exactly this reasoning and was WRONG
(pi emitted it from a different line). Grep pi for where the chat editor's leading glyph comes from,
if anywhere, before you delete cyrup's.
**E2 — the editor wrap width ignores the prompt** (\`editor.ts:485-497\`, \`contentWidth\`).
**E15 — the editor slot is MEASURED at a different width than it RENDERS at** (same lines). These two
are one bug seen from two ends; fix them together.
**E3 — maximum height** (\`:499-501\`, "30% of terminal height, minimum 5 lines" — read the exact
formula).
**E4 — scroll rules and scroll-to-cursor** (\`:259-268 createScrollBorder\`). Needs keystroke-driven
tests.
**E8 — the \`ui.input\` placeholder replaces the caret** (\`extension-input.ts:36\`).
**E9 — \`ui.editor\` hint row colour and indent** (\`extension-editor.ts:82-90\`).
**E10 — the \`ui.input\` prompt** (\`input.ts:380\`, \`const prompt = "> "\` — two chars at column 0;
check what cyrup draws and at what column).
**E11 — extension dialog title weight** (\`extension-input.ts:50\`, \`extension-editor.ts:66\`).
**E12 — \`ui.editor\` body height** (\`extension-editor.ts:65\` reuses the same \`Editor\`, so E3's
formula applies there too — check whether fixing E3 fixes this or whether the dialog overrides it).
**E13 — the caret on focus loss** (\`editor.ts:545-564\`, emitted whenever \`layout\`… — read the exact
condition). Keystroke-driven.
**E14 — the autocomplete popup indent under \`editorPaddingX\`** (\`editor.ts:591-597\`).

For each: read pi, fix, drive KEYSTROKES where the item is interactive, add a test that FAILS without
the fix plus a MIRROR that stays green, revert-prove against a cp-backup of the FIXED state.
Fill \`wrap_boundary\` with exactly where you stopped relative to batch 8.`,
  { label: 'fix:editor', phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
)

log(`Fix phase: ${(fixed?.items || []).length} items`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'keystroke_audit'],
  properties: {
    tree_restored: { type: 'boolean' },
    keystroke_audit: {
      type: 'string',
      description: 'Drive a real editing session (type, newline, arrows, home/end, backspace at a boundary, paste a wide-char string) and report buffer + caret column at each step vs pi.',
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
RENDERED: ${(a.rendered_before_after || '(not shown)').slice(0, 400)}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 500)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. Do NOT fix; report.

**Lens 1 — drive a real editing session.** Type text, insert a newline, arrow around, home/end,
backspace across a line boundary, and paste a string containing CJK and a ZWJ emoji. At each step
report the buffer and the CARET COLUMN, and compare against what pi's editor would do, read from
source. Fill \`keystroke_audit\`. A caret positioned by char index in a string containing wide
characters lands in the wrong column — that is the bug class to hunt.

**Lens 2 — E1, the prompt glyph.** Batch 3 deleted a glyph on the reasoning "pi never emits it" and
was WRONG, because pi emitted it from a line nobody had read. Independently establish whether pi's
chat editor has a leading glyph, searching beyond \`editor.ts\` itself.

**Lens 3 — E2/E15 vs batch 8.** Confirm no prose-wrapping change leaked in. A half-landed wrap
rewrite would make batch 8 unreadable. Report exactly what was touched.

**Lens 4 — height and scroll.** E3/E4/E12: check the max-height formula against pi's exact arithmetic
and that scroll-to-cursor works at the top and bottom edges and after a resize. Drive it, do not
reason about it.

**Lens 5 — weakened assertions, reverts, inertness.** \`git diff\` every edited test and rule on each.
Re-run each revert with cp-backup/cp-restore, never \`git checkout\`, reverting where the defect
lives. Then check whether any fix is INERT — behind a condition that never holds in a real session.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-tui\` is green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`.

## What the author reported

${digest}

WRAP BOUNDARY: ${fixed?.wrap_boundary || '(none reported)'}
TESTS CHANGED: ${fixed?.tests_changed || '(none reported)'}
FILES: ${(fixed?.files_changed || []).join(', ')}
NOTES: ${fixed?.notes || '(none)'}`,
  { label: 'verify:editor', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-7',
  items: (fixed?.items || []).map((i) => ({ id: i.id, status: i.status })),
  keystroke_audit: review?.keystroke_audit,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
