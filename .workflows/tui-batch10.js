export const meta = {
  name: 'cyrup-tui-batch-10-subsystems',
  description: 'TUI batch 10 — the last: table cells + minimums, LaTeX, transformers/mermaid, /hotkeys, first-time-setup, fullscreen scrollbar',
  phases: [
    { title: 'Fix', detail: 'grouped: markdown-internal vs new subsystems' },
    { title: 'Verify', detail: 'ONE reviewer; and a FINAL sweep of all 150 audit items' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are completing the TUI fidelity backlog for \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against
pi (${WS}/pi) at **v0.84.1**. The audit is \`${WS}/cyrup/TUI-FIDELITY.md\`.

**This is the LAST of ten batches.** Nine are landed: colour accessors, footer/status, the
\`selectedBg\` swap, dialog envelopes, transcript rhythm, markdown, editor, the wrap rewrite, and
per-selector completeness. What remains are the \`large\` rows — genuinely new subsystems, not
one-line corrections.

## WHAT PREVIOUS BATCHES ESTABLISHED — do not re-derive, do not regress

- pi has NO "does it fit" gate: a component's \`render(width)\` takes no height
  (\`tui/src/tui.ts:211-245\`); visible rows are a strict PREFIX of the natural render, and a
  \`Paragraph\` keeps \`lines[0..height]\` and drops the TRAILING rows (settled empirically in batch 9).
- \`Text.render\` wraps at \`width - paddingX*2\` then prefixes \`leftMargin\` to every row
  (\`text.ts:60-87\`). cyrup's port is \`transcript::text_lines_of\` — REUSE it. Do not write a sixth
  wrapper.
- Batch 8 moved wrapping INSIDE the padded render and made the outer \`Paragraph::wrap\` inert on
  those paths. Do not reintroduce an outer wrap there.
- **FIVE separate width measurements in this crate carried a char-vs-grapheme defect** and were
  fixed one batch at a time. Every measurement must be \`Span::width()\`/\`Line::width()\`, never
  \`chars().count()\`.

## THE TWO FAILURE MODES THIS EFFORT KEEPS PRODUCING

1. **Ported-but-unwired.** Batch 9 shipped EIGHT state seams whose only callers were tests, even
   with "name the user action" in the brief. For every new subsystem here, name the user action that
   reaches it and drive THAT in the test. A function whose only caller is a test is not done.
2. **Untested fixes.** Batch 7 shipped six, batch 9 eleven — each found by disabling the behaviour
   and watching the suite stay green. Before finishing, revert each change in turn and confirm
   something goes red. Any MIRROR you claim must be one you RAN; batch 8 asserted one in prose that
   was false.

## HARD RULES

- **READ pi's source. Never infer from a name, this brief, or the audit.** The audit has been WRONG
  FOUR times (S24's fold glyph, X17's exit row, S3/S28's scope, \`sliceByColumn\`'s \`strict\` flag),
  and batch 9 found its file attributions wrong again. Quote the \`file:line\` and COUNT
  (\`git -C ${WS}/pi show v0.84.1:packages/<path>\`).
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.** If a subsystem is genuinely blocked on a seam cyrup lacks, say exactly what
  and why — batch 9 did that correctly for /model's refresh status.
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` need their own file-level \`#![allow]\`.
- **Never weaken an assertion.** Report each edited test BEFORE/AFTER.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\`. **NEVER \`git checkout\`**, not even
a bare one: an agent typed \`git checkout --quiet\` last batch and had to verify 25 files by hand.

## BUILD DISCIPLINE — AGENTS RUN IN PARALLEL

ONE shared \`target/\`, ONE \`.cargo-lock\`; cargo WILL block. Wait it out.
\`CARGO_INCREMENTAL=0\`; \`cargo check/test/clippy -p cyrup-tui --all-targets\` only.
**NEVER \`--workspace\`.** Do not edit files outside your group; report cross-file needs in \`notes\`.
Finish with \`cargo clippy -p cyrup-tui --all-targets; echo "exit=$?"\`.
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
        required: ['id', 'status', 'summary', 'user_action', 'revert_proof'],
        properties: {
          id: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'already-correct', 'blocked'] },
          summary: { type: 'string' },
          user_action: { type: 'string', description: 'The concrete user action that reaches this, and the call chain. "A caller exists" is not an answer.' },
          upstream_citation: { type: 'string' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string', description: 'A mirror you actually RAN, with its result' },
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

const GROUPS = [
  {
    key: 'markdown',
    title: 'Markdown internals — M7, M15, and the LaTeX/transformer chain (M12, M13)',
    brief: `All in \`markdown.rs\` and its neighbours. Do them in this order.

**M15 — table minimum column width.** \`markdown.ts:863\` \`const maxUnbrokenWordWidth = 30\` and the
\`:871\`ff logic: pi floors each column at \`min(longestWord, 30)\` and only collapses to all-1s when
that exceeds \`availableForCells\`. cyrup floors at 1 unconditionally. Batch 6 found this while
working M11 and correctly deferred it here.

**M7 — inline formatting inside table cells.** \`markdown.ts:960\` and \`:983\` both call
\`renderInline\` on the cell text; cyrup renders cells as plain text, so bold/code/links inside a
table are lost. Note the interaction with M15 and with batch 6's column arithmetic — a styled cell
still has to be MEASURED at its visible width.

**M12 — LaTeX math.** \`markdown.ts:123-144\` registers \`LATEX_MARKDOWN_EXTENSIONS\` (block and
inline). \`tui/src/latex.ts\` is 1373 lines. Read it and decide honestly what is portable: if the full
renderer is out of reach, port the tokenizer + the fallback path (what does pi render when it cannot
typeset?) and say precisely what is left. Do NOT half-port silently.

**M13 — extension markdown transformers, and therefore mermaid.**
\`markdown-transform.ts:3-10\` creates the transformer chain; \`components/mermaid.ts\` is one consumer.
Establish the seam first: does cyrup's extension host have anywhere a transformer could register?
If not, that is the blocker and it belongs in \`notes\` with the specific missing API — do not invent
one.`,
  },
  {
    key: 'subsystems',
    title: 'New surfaces — /hotkeys (S36), first-time-setup, the fullscreen scrollbar',
    brief: `**S36 — \`/hotkeys\`.** \`interactive-mode.ts:6198-6203\` appends to the TRANSCRIPT (not a
dialog): a \`Spacer\` then the keybinding list. Read what it actually renders and where. cyrup's
\`/hotkeys\` — check whether it exists at all before assuming it needs correcting.

**First-time-setup.** \`components/first-time-setup.ts\`. NOTE the parity backlog records that
cyrup's rebrand made pi's \`shouldRunFirstTimeSetup\` predicate a compile-time \`false\`
(\`crates/cyrup/src/startup.rs:17-30\`), so the wizard is ported faithfully and can never fire. If
that is still true, the presentation work here is unreachable until the predicate answers for
cyrup's own identity — say so plainly rather than polishing something nobody can see, and check
whether that has changed since it was written.

**The fullscreen scrollbar.** Part of the alternate-screen TUI mode. Establish whether cyrup has
alternate-screen mode at all (the parity backlog lists G46+G54 "fullscreen TUI mode" as unported,
large). If the mode does not exist, its scrollbar cannot; report that and stop rather than building
half a mode.

For each item: read pi, establish reachability FIRST, then fix. If a thing is genuinely unreachable
in cyrup today, that is a legitimate outcome — say exactly what blocks it and what would unblock it.`,
  },
]

const authored = await parallel(
  GROUPS.map((g) => () =>
    agent(
      `${COMMON}

## Your group: ${g.title}

${g.brief}

For each: read the pi source IN FULL, establish what a user does to see it, fix, add a test that
FAILS without the fix plus a MIRROR you actually RUN, revert-prove against a cp-backup of the FIXED
state. Then clippy.`,
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
  required: ['findings', 'tree_restored', 'backlog_sweep'],
  properties: {
    tree_restored: { type: 'boolean' },
    backlog_sweep: {
      type: 'string',
      description: 'THE FINAL SWEEP: for every one of the audit\'s 150 items, its current state — fixed / genuinely blocked (with what blocks it) / still open. This is the closing record of the whole TUI effort.',
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
    (a) => `### [${a.group}] ${a.id} [${a.status}] ${a.summary}
USER ACTION: ${a.user_action || '(none given)'}
UPSTREAM: ${a.upstream_citation || '(none)'}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 400)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: review this batch AND close out the whole TUI effort. Do NOT fix; report.

**Lens 1 — reachability.** For each item, verify the claimed user action actually reaches the code.
Batch 9 shipped eight seams whose only callers were tests. A "blocked" verdict is legitimate — check
that the stated blocker is real by trying to find the seam yourself.

**Lens 2 — untested fixes.** Disable each behaviour in turn and confirm something goes red. Batches
7 and 9 shipped six and eleven untested fixes respectively, each found exactly this way.

**Lens 3 — regressions across all nine previous batches.** This is the last chance to catch one.
Spot-check: colour accessors resolve the right roles; the footer's context segment renders; the
selection background is on /tree and /resume only; dialog envelopes keep their spacers; transcript
blanks are in position; markdown tables/lists/fences are unchanged; the editor wraps CJK; batch 8's
left edge holds — no row of any block starts at a different column from its siblings.

**Lens 4 — THE FINAL SWEEP.** Go through the audit's full item list (T*, C*, S*, L*, M*, E*, X*) and
record each item's true state: fixed, genuinely blocked (naming the blocker), or still open. Fill
\`backlog_sweep\`. Be exact — this is the closing record, and an item quietly marked fixed that is not
would be the worst possible outcome of ten batches.

**Lens 5 — weakened assertions and citations.** \`git diff\` every edited test and rule on each. Open
every cited pi line at v0.84.1 and COUNT.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-tui\` is green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`; NEVER \`git checkout\`.

## What the agents reported

${digest}

TESTS CHANGED:
${ok.map((r) => `[${r.group}] ${r.tests_changed || '(none)'}`).join('\n')}

NOTES:
${ok.map((r) => `[${r.group}] ${r.notes || '(none)'}`).join('\n')}`,
  { label: 'verify:final-sweep', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-10',
  items: allItems.map((i) => ({ group: i.group, id: i.id, status: i.status })),
  backlog_sweep: review?.backlog_sweep,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
