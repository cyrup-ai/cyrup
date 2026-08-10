export const meta = {
  name: 'cyrup-tui-closeout',
  description: 'TUI closeout — the 9 open + 4 partial items the final sweep found, plus batch 10 review findings',
  phases: [
    { title: 'Fix', detail: 'three groups by file family' },
    { title: 'Verify', detail: 'ONE reviewer; re-sweep to confirm the backlog is actually closed' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are CLOSING OUT the TUI fidelity backlog for \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against
pi (${WS}/pi) at **v0.84.1**. The audit is \`${WS}/cyrup/TUI-FIDELITY.md\`.

## WHY THIS EXISTS

Ten batches ran. A final sweep of all 117 audit rows found: **102 fixed, 4 partial, 2 genuinely
blocked, 9 STILL OPEN** — items that were scheduled in every plan and delivered by no batch. Being
scheduled is not being done. Your job is to close the remainder so the backlog is actually complete.

## WHAT PREVIOUS BATCHES ESTABLISHED — do not re-derive, do not regress

- pi has NO "does it fit" gate; a \`Paragraph\` keeps \`lines[0..height]\` and drops the TRAILING rows.
- \`Text.render\` wraps at \`width - paddingX*2\` then prefixes \`leftMargin\` to every row
  (\`text.ts:60-87\`). cyrup's port is \`transcript::text_lines_of\` — REUSE it.
- Batch 8 moved wrapping INSIDE the padded render; the outer \`Paragraph::wrap\` is inert there.
- \`keyHint\` is per-pair: \`fg("dim", keyText) + fg("muted", " desc")\` (\`keybinding-hints.ts:42-44\`),
  and the key must be RESOLVED from the live keymap, never spelled as a literal.
- **SEVEN separate width measurements in this crate have carried a char-vs-grapheme defect**, each
  found only when looked at directly. Two more are open below. Every measurement must be
  \`Span::width()\`/\`Line::width()\` and every truncation grapheme-aware.

## THE FAILURE MODES THIS EFFORT KEEPS PRODUCING

1. **Ported-but-unwired** — batch 9 shipped eight seams whose only callers were tests.
2. **Untested fixes** — batch 7 shipped six, batch 9 eleven, found by disabling each and watching
   the suite stay green.
3. **Claims that were inferred, not read** — batch 10 asserted "cyrup has no extension-registered
   -shortcut registry" to justify omitting a table. That is FALSE. If you are about to justify NOT
   doing something, go and read the thing first.

## HARD RULES

- **READ pi's source. Never infer from a name, this brief, or the audit.** The audit has been WRONG
  four times. Quote the \`file:line\` and COUNT (\`git -C ${WS}/pi show v0.84.1:packages/<path>\`).
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.** "Blocked" is legitimate ONLY with the specific missing seam named, after you
  have looked for it.
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` need their own file-level \`#![allow]\`.
- **Never weaken an assertion.** Report each edited test BEFORE/AFTER.
- **Every fix needs a test that fails without it; any MIRROR you claim must be one you RAN.**
  Before finishing, revert each change in turn and confirm something goes red.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\`. **NEVER \`git checkout\`**, not even a
bare one.

## BUILD DISCIPLINE — AGENTS RUN IN PARALLEL

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
        required: ['id', 'status', 'summary', 'revert_proof'],
        properties: {
          id: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'already-correct', 'blocked'] },
          summary: { type: 'string' },
          user_action: { type: 'string' },
          upstream_citation: { type: 'string' },
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

const GROUPS = [
  {
    key: 'transcript',
    title: 'Transcript items never delivered — X6, X7, X8, X9, X11, X13, X14, X15',
    brief: `All in \`transcript.rs\`, \`bash.rs\`, \`theme.rs\`. Eight items scheduled by every plan and
delivered by none. Read each pi source in full.

**X6** — \`render_read\`/\`render_write\` are a flat \`tool_output_style()\` loop: no syntax highlighting,
no \`replaceTabs\` (\`read.ts:183-190\`, \`write.ts:150-160\`).
**X7** — \`render_read\` always emits \`read <path>\`; pi classifies skill/docs/resource reads
(\`read.ts:145-166\`, \`:336\`). NOTE this is the same defect as G30b in the PARITY backlog
(\`getCompactReadClassification\`) — read that entry, port it once, and say so.
**X8** — \`tool_bg_style\` keys only on \`done\`/\`is_error\`; pi has a preview-state arm
(\`edit.ts:239-253 getEditHeaderBg\`).
**X9** — \`more_lines_hint\` is one flat \`muted_style()\` and \`EXPAND_KEY\` is the literal \`"ctrl+o"\`.
pi splits dim key from muted words and resolves the live binding. **bash.rs already got this right
in X16, so the crate's two hint renderers currently disagree with each other** — make them agree.
**X11** — \`transcript.rs:2307\` splits an extension renderer's output and restyles every line
\`dim\`, stripping the colour the extension chose. pi adds the component as-is
(\`custom-message.ts:76-81\`).
**X13** — \`set_complete\` takes no truncation flag or spool path; \`"Output truncated"\` appears
nowhere. A comment at \`bash.rs:318\` references "X13's truncation-warning part" as if it existed.
pi: \`bash-execution.ts:196-199\`.
**X14** — branch/compaction summaries always render in full; pi has a collapsed form with an
expand hint (\`branch-summary-message.ts:11,46-56\`, \`compaction-summary-message.ts:48-56\`).
**X15** — a throwing extension entry renderer draws NOTHING; pi renders a failure box
(\`custom-entry.ts:47-52\`).`,
  },
  {
    key: 'geometry',
    title: 'The char-vs-grapheme residuals and L7 — S24, S26, S27, T9, L7',
    brief: `**S26 + S27 — REGRESSION-CLASS, open after ten batches.** pi measures every \`SelectList\`
width with \`visibleWidth()\` and truncates with \`truncateToWidth()\`; cyrup's
\`select_list.rs:257-267\` measures with \`chars().count()\` and \`:343-350\` slices chars. This is the
same defect that has now been found SEVEN times in this crate (\`wrap_line\`, \`wrap_cell\`,
\`word_wrap_line\`, \`truncate_to_visual_lines\`, \`apply_bg\`, and two more). Fix both, and test with
CJK, a ZWJ family emoji and a combining mark in both a label and a description.

**S24 (partial)** — two halves left. (a) the \`/tree\` label-timestamp column pads from
\`chars().count()\` over the row's spans — the EIGHTH instance; (b) the row LABEL is one span in
base/accent style where pi colours it PER ROLE and prefixes it (\`tree-selector.ts\`). Batch 3 did the
fold glyph only.

**T9 (partial)** — \`borderAccent\` and \`scrollbarThumb\` still have zero read sites. \`scrollbarThumb\`
is genuinely blocked behind the unported fullscreen mode — confirm and record. \`borderAccent\` is
said to be blocked behind S24's \`[compaction: Nk tokens]\` row; you are fixing S24, so resolve it.

**L7 — never claimed by any batch.** \`region_constraints\` (\`app.rs:5417-5427\`) allocates the footer
FIRST, then \`let slot = want_slot.min(remaining);\` with no floor, so on a short terminal the editor
is squeezed below 3 rows where pi's dock entry holds it at \`{shrink:1, minSize:3}\`
(\`interactive-mode.ts:877-883\`). The audit said "verify before fixing" and nobody did — verify, then
fix or record why not.`,
  },
  {
    key: 'batch10-findings',
    title: 'Batch 10 review findings — M7, S36, Entry::Block, wrap_cell',
    brief: `**S36 — a FALSE claim, and the omitted Extensions table.** The batch omitted \`/hotkeys\`'
Extensions section justifying it with "cyrup has no extension-registered-shortcut registry to read".
The reviewer says that is FALSE. FIND the registry, then port the table. This is the one claim in
batch 10 that was inferred rather than read — treat it as the lesson it is.
**S36 (second half)** — \`hotkeys_markdown\`'s two key-display closures were both corrected but only
the EDITOR half has a test; the GLOBAL half is untested.
**M7 — the central claim is untested.** "Cell wrapping went from a plain-\`str\` wrapper to a
span-aware one so styles survive the break" has NO test: every M7 test uses cells short enough not
to wrap. Add one where a STYLED cell wraps and assert the style survives on both rows.
**M7 (minor)** — \`wrap_cell\` re-inserts word separators taking the PRECEDING word's style, so the
space between two differently-styled words is wrong. Check what pi does.
**Entry::Block** — two untested changes: the \`width - 2\` content reduction, and the empty-body
early return (the port of \`markdown.ts:288-296\`).
**tests/bash_overlay.rs** — \`overlay_captures_navigation_keys\` became
\`hotkeys_does_not_capture_navigation_keys\` with a net −1 assertion and nothing pinning the new
routing. Restore the coverage.`,
  },
]

const authored = await parallel(
  GROUPS.map((g) => () =>
    agent(
      `${COMMON}

## Your group: ${g.title}

${g.brief}

For each: read the pi source IN FULL, fix, add a test that FAILS without the fix plus a MIRROR you
actually RUN, revert-prove against a cp-backup of the FIXED state. Then clippy.`,
      { label: `fix:${g.key}`, phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
    )
  )
)

const ok = authored.filter(Boolean)
const allItems = ok.flatMap((r) => (r.items || []).map((i) => ({ ...i, group: r.group })))
log(`Closeout: ${allItems.length} items across ${ok.length} groups`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'resweep'],
  properties: {
    tree_restored: { type: 'boolean' },
    resweep: {
      type: 'string',
      description: 'Re-sweep EVERY audit row. Per ID: fixed / partial / blocked (with the named blocker) / still open. Give the final tally. This closes the record.',
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
USER ACTION: ${a.user_action || '(n/a)'}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 400)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: verify the closeout AND re-sweep the whole backlog. Do NOT fix; report.

**Lens 1 — THE RE-SWEEP.** Go through every audit row (T*, L*, X*, C*, S*, E*, M*) and record its
TRUE state: fixed, partial (say which half), blocked (name the seam), or still open. Give the final
tally. The previous sweep found nine items every plan had scheduled and no batch had delivered —
verify independently rather than trusting this batch's claims. Fill \`resweep\`.

**Lens 2 — the grapheme sweep, eighth pass.** \`grep\` the entire crate for \`chars().count()\`,
\`.chars().take\`, \`.chars().skip\` and char-indexed slicing on anything that reaches a render. Seven
instances have been found one at a time. Report EVERY remaining site and whether it can reach output.

**Lens 3 — reachability and untested fixes.** For each item, verify the user action reaches the
code, then disable the behaviour and confirm something goes red.

**Lens 4 — the S36 registry claim.** Batch 10 justified omitting a table with a claim the previous
reviewer called false. Establish the truth yourself: does cyrup have an extension-shortcut registry,
and does \`/hotkeys\` now render the Extensions table?

**Lens 5 — regressions and weakened assertions.** Spot-check batches 1-10 still hold, \`git diff\`
every edited test, and open every cited pi line at v0.84.1.

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
  { label: 'verify:resweep', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-closeout',
  items: allItems.map((i) => ({ group: i.group, id: i.id, status: i.status })),
  resweep: review?.resweep,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
