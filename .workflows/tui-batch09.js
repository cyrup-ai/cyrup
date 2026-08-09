export const meta = {
  name: 'cyrup-tui-batch-09-selectors',
  description: 'TUI batch 9 — per-selector completeness across /resume, /config, /settings, /model, /scoped-models, /login, /fork, /trust',
  phases: [
    { title: 'Fix', detail: 'grouped by dialog so each agent owns one component family' },
    { title: 'Verify', detail: 'ONE reviewer; render every dialog and diff against its pi component' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are fixing PRESENTATION FIDELITY defects in \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against
pi (${WS}/pi) at **v0.84.1**. The audit is \`${WS}/cyrup/TUI-FIDELITY.md\` — read the S rows for your
items first.

## WHAT THIS BATCH IS

Per-dialog completeness. Earlier batches fixed what these selectors SHARE (the \`selectedBg\`
inversion, \`SelectList\` geometry, envelope spacers, hint-row and inset gating). What is left is what
each dialog does on its OWN: \`/resume\`'s tree connectors and metadata column, \`/config\`'s two-row
header and inherited-global states, \`/settings\`'s search and description block, \`/model\`'s
refresh-status rows, \`/scoped-models\`'s badges and footer, \`/login\`'s status colours, \`/fork\`'s
three-line rows, \`/trust\`'s separators.

## THE MISTAKE THIS BATCH IS MOST LIKELY TO MAKE

**A property of one pi component is not a property of the shared engine.** Batch 3 gave ~10 dialogs
a hint row pi draws on 4 and an inset pi applies to 6, by putting both on \`ListSelector\`. These items
are per-component BY CONSTRUCTION — if you find yourself editing a shared helper, stop and check
which components upstream actually gives that behaviour to, and gate accordingly. State the
enumeration in your report.

## WHAT PREVIOUS BATCHES ESTABLISHED — do not re-derive, do not regress

- pi has NO "does it fit" gate; visible rows are a strict PREFIX of the natural render
  (\`tui/src/tui.ts:211-245\`, \`layout.ts:113\`).
- \`Text.render\` wraps at \`width - paddingX*2\` then prefixes \`leftMargin\` to every row
  (\`text.ts:60-87\`). cyrup's port is \`transcript::text_lines_of\` — REUSE it; do not write a fifth
  wrapper.
- Batch 8 moved wrapping INSIDE the padded render. Do not reintroduce a \`Paragraph::wrap\` on a path
  that now wraps internally.
- **FOUR separate width measurements in this crate carried a char-vs-grapheme defect**
  (\`wrap_line\`, \`wrap_cell\`, \`word_wrap_line\`, \`truncate_to_visual_lines\`). Every measurement you
  write must be \`Span::width()\`/\`Line::width()\`, never \`chars().count()\`.

## HARD RULES

- **READ pi's source. Never infer from a name, this brief, or the audit.** The audit has been WRONG
  FOUR times (S24's fold glyph, X17's exit row, S3/S28's scope, \`sliceByColumn\`'s \`strict\` flag) and
  each time the agent was right to follow the source. Quote the \`file:line\` and COUNT
  (\`git -C ${WS}/pi show v0.84.1:packages/<path>\`).
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.**
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` need their own file-level \`#![allow]\`.
- **Never weaken an assertion.** Report each edited test BEFORE/AFTER.
- **EVERY fix needs a test that fails without it, and any MIRROR you claim must be one you RAN.**
  Batch 8 asserted a mirror in prose that was false; batch 7 shipped six untested fixes. Before
  finishing, revert each change in turn and confirm something goes red.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\`. **NEVER \`git checkout\`.**

## BUILD DISCIPLINE — OTHER AGENTS RUN IN PARALLEL

ONE shared \`target/\`, ONE \`.cargo-lock\`; cargo WILL block on the lock. Wait it out, never kill it.
\`CARGO_INCREMENTAL=0\`; \`cargo check/test/clippy -p cyrup-tui --all-targets\` only.
**NEVER \`--workspace\`.** MUST NOT edit files another agent owns — report cross-file needs in
\`notes\`. Finish with \`cargo clippy -p cyrup-tui --all-targets; echo "exit=$?"\`.
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
        required: ['id', 'status', 'summary', 'revert_proof'],
        properties: {
          id: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'already-correct', 'blocked'] },
          summary: { type: 'string' },
          upstream_citation: { type: 'string' },
          rendered_before_after: { type: 'string' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string', description: 'A mirror you actually RAN, with its result' },
        },
      },
    },
    shared_helpers_touched: { type: 'string', description: 'Any shared code you edited, and which components upstream gives that behaviour to' },
    tests_changed: { type: 'string' },
    files_changed: { type: 'array', items: { type: 'string' } },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Fix')

const GROUPS = [
  {
    key: 'resume',
    title: '/resume — the deepest single dialog (S8, S9, S11, S12, S13, S14, S15)',
    brief: `\`session-selector.ts\`, and cyrup's \`session_selector.rs\`. Seven items, all in one component:
  **S8** tree connectors (\`:522-530 buildTreePrefix\`)
  **S9** the right-hand metadata column (\`:502-505\`, \`spacing = …\`)
  **S11** per-row state colours (\`:486-497\`, \`messageColour\`…)
  **S12** the header (\`:131\`, \`theme.bold(title)\`)
  **S13** border rules (\`:738\`, \`:746\`)
  **S14** the path toggle (\`:471-473\`, folds the path into the row)
  **S15** the \`(i/N)\` scroll row (\`:512-516\`)
Batch 3 already ported the \`› \` cursor and the \`selectedBg\` fill here, and batch 4 added the
header/search blank — read the current code before assuming anything is missing.`,
  },
  {
    key: 'config-settings',
    title: '/config and /settings (S16, S17, S18, S19, S33)',
    brief: `\`config-selector.ts\` + \`settings-selector.ts\` / \`settings-list.ts\`, and cyrup's
\`config_selector.rs\` + \`settings_selector.rs\`.
  **S17** \`/config\`'s header renders TWO rows upstream (\`:202-218\`); cyrup has one.
  **S18** \`inherited global\` dim states (\`:418-419\`)
  **S19** the scroll row, \`[+]\`/\`[-]\` states and ellipsis (\`:448\`)
  **S16** \`/settings\` search, description block, hint (\`settings-selector.ts:765-874\`)
  **S33** the settings label column bounds (\`settings-list.ts:121\`,
  \`Math.min(30, Math.max(…))\` — port the exact clamp)
Batch 4 windowed \`ConfigSelector\` and batch 7 wired per-frame terminal height — build on those.`,
  },
  {
    key: 'model-scoped',
    title: '/model and /scoped-models (S6, S7, S23, S29, S30, S32)',
    brief: `\`model-selector.ts\` + \`scoped-models-selector.ts\`, and cyrup's \`model_selector.rs\` +
the \`CheckboxSelector\` in \`selector.rs\`.
  **S6** the scoped-models enable marker — note it is appended AFTER something (\`:252-259\`); read
  the order carefully.
  **S7** title, subtitle, provider badge and \`Model Name:\` (\`scoped-models-selector.ts\`)
  **S29** the footer hint (\`:197-208\`, \`{enter} toggle\`…)
  **S23** \`/model\` refresh-status and error rows (\`model-selector.ts:299-317\`)
  **S30** the scope hint row, which upstream adds as its OWN \`Text\` (\`:99-100\`)
  **S32** the scope/warning row indent (\`:97\`)`,
  },
  {
    key: 'misc',
    title: '/login, /fork, /trust, search boxes and the slash popup (S4, S5, S21, S22, S31, S34, S35)',
    brief: `  **S5** search inputs are missing on THREE dialogs (\`scoped-models-selector.ts\` et al) —
  batch 4 deferred these deliberately when it counted envelope spacers, so this closes that gap.
  **S31** the search-box prompt glyph (\`input.ts:380\`, \`const prompt = "> "\` — ONE shared
  definition; check every dialog that has a search box uses it).
  **S21** \`/login\` and \`/logout\` status and badge colours (\`oauth-selector.ts:132\`)
  **S22** \`/fork\` renders THREE lines per entry upstream (\`user-message-selector.ts:57-69\`)
  **S34** \`/trust\` hint separators (\`trust-selector.ts:74-83\`, a literal string — quote it)
  **S4** hint rows: per-pair dim key + muted description (\`keybinding-hints.ts:42-44\`)
  **S35** multi-line descriptions in the slash popup (\`select-list.ts:9\` \`normalizeToSingleLine\` —
  batch 6 ported this for one call path; check whether the popup shares it)`,
  },
]

const authored = await parallel(
  GROUPS.map((g) => () =>
    agent(
      `${COMMON}

## Your group: ${g.title}

${g.brief}

For each item: read the pi component IN FULL, fix, RENDER the dialog and look at it, add a test that
FAILS without the fix plus a MIRROR you actually RUN, and revert-prove against a cp-backup of the
FIXED state. Then clippy.

You own only the files your group names. If a fix needs a shared helper, report it in
\`shared_helpers_touched\` with the enumeration of which pi components have that behaviour.`,
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
  required: ['findings', 'tree_restored', 'dialog_audit'],
  properties: {
    tree_restored: { type: 'boolean' },
    dialog_audit: {
      type: 'string',
      description: 'Render each dialog and list its rows beside the pi component\'s output, read from source.',
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
UPSTREAM: ${a.upstream_citation || '(none)'}
RENDERED: ${(a.rendered_before_after || '(not shown)').slice(0, 350)}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 350)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. Do NOT fix; report.

**Lens 1 — render every dialog and diff it.** For each of /resume, /config, /settings, /model,
/scoped-models, /login, /fork, /trust and the slash popup, render it with realistic content and list
the rows beside what the pi component emits, read from source. Fill \`dialog_audit\`.

**Lens 2 — over-application.** Four agents worked in parallel and any of them may have reached for a
shared helper. Check every shared edit: does upstream give that behaviour to ALL the components now
receiving it, or only some? This is the batch-3 mistake and it is the most likely failure here.

**Lens 3 — regressions from batches 1-8.** Confirm the \`selectedBg\` swap still holds (fill only on
/tree and /resume), envelope spacers are still in position, and batch 8's wrap is intact — no row of
any dialog starts at a different column from its siblings.

**Lens 4 — untested and inert fixes.** Revert each change in turn and confirm something goes red;
report any that leaves the suite green. Then check the opposite: any fix behind a condition that
never holds in a real render.

**Lens 5 — weakened assertions and citations.** \`git diff\` every edited test and rule on each. Open
every cited pi line at v0.84.1 and COUNT.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-tui\` is green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`.

## What the agents reported

${digest}

SHARED HELPERS TOUCHED:
${ok.map((r) => `[${r.group}] ${r.shared_helpers_touched || '(none)'}`).join('\n')}

TESTS CHANGED:
${ok.map((r) => `[${r.group}] ${r.tests_changed || '(none)'}`).join('\n')}

NOTES:
${ok.map((r) => `[${r.group}] ${r.notes || '(none)'}`).join('\n')}`,
  { label: 'verify:dialogs', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-9',
  items: allItems.map((i) => ({ group: i.group, id: i.id, status: i.status })),
  dialog_audit: review?.dialog_audit,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
