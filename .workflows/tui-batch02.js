export const meta = {
  name: 'cyrup-tui-batch-02-footer-status',
  description: 'TUI batch 2 — footer, status band, loaders and hints (C1-C15); C1 has never rendered once',
  phases: [
    { title: 'Fix', detail: 'one crate, sequential — status.rs / status_indicator.rs / chrome.rs / app.rs' },
    { title: 'Verify', detail: 'ONE reviewer; the footer is on screen every frame' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are fixing PRESENTATION FIDELITY defects in \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against
pi (${WS}/pi) at **v0.84.1**. The audit is \`${WS}/cyrup/TUI-FIDELITY.md\` — read §3's chrome table
(rows C1-C15) in full before you touch anything. Every claim there was verified on both sides.

## WHY THIS BATCH IS HIGH STAKES

**The footer is on screen every frame.** A wrong separator, a wrong colour or a missing segment is
visible in every screenshot anyone ever takes of this tool. There is nowhere to hide a mistake here,
which cuts both ways: get it right and the whole UI reads as finished.

## WHAT THE LAST BATCH ESTABLISHED — do not re-litigate

- The palette DATA is clean; all 50 tokens match \`dark.json\`/\`light.json\`. Do not "fix" it.
- There are NO hardcoded RGB colours in the renderers.
- \`cyrup-tui\` has NO snapshot/golden machinery (no \`.snap\`, no \`insta\`, no \`cyrup-test-support\`
  dep). Nothing to regenerate. Verified independently.
- TUI batch 1 just fixed the colour ACCESSORS in \`theme.rs\` — \`dim_style()\` now resolves the \`dim\`
  token (\`#666666\` dark / \`#767676\` light, no SGR attribute) and \`error_style()\` no longer bakes in
  bold. So when C2 says "swap \`muted_style()\` for the \`dim\` token", the correct accessor already
  exists and behaves correctly. Read \`theme.rs\` before assuming what an accessor does.

## HARD RULES

- **READ pi's source for every change. Never infer from a name or from this brief.** Briefs here have
  been wrong repeatedly — a cited line containing nothing of the sort, an item filed under the wrong
  crate, a whole batch planned on a false premise, and in the last batch my literal wording would
  have undone the very fix it was refining. If the source disagrees, FOLLOW THE SOURCE and say so.
- **Verify every citation**: \`git -C ${WS}/pi show v0.84.1:packages/<path>\` and COUNT the lines.
  Write version-qualified citations.
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.** Another defect in the same function gets fixed, or you state precisely why
  it is a separate change.
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` files are separate crates needing their
  own file-level \`#![allow(...)]\`.
- **Never weaken an assertion.** If a test fails after your fix, update its expectation to pi's
  actual value and quote the pi line, or conclude your fix is wrong. Report every test you edit with
  the assertion BEFORE and AFTER.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— take the backup of the FIXED state BEFORE reverting, then restore with \`cp\` from it.
**NEVER \`git checkout\`**: the tree holds uncommitted work, and an agent that restored a pre-fix
backup wiped its own work doing exactly this. A revert must remove EVERY guard the test covers.

## BUILD DISCIPLINE

ONE shared \`target/\`, ONE \`.cargo-lock\`. Prefix cargo with \`CARGO_INCREMENTAL=0\`. Disk is tight.
MAY run \`cargo check/test/clippy -p cyrup-tui --all-targets\`. **MUST NOT run \`--workspace\`** — the
orchestrator gates once. Finish with \`cargo clippy -p cyrup-tui --all-targets; echo "exit=$?"\`.
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
          user_visible: { type: 'string', description: 'What a user sees differently now' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string' },
        },
      },
    },
    tests_changed: { type: 'string', description: 'Every pre-existing test edited: assertion BEFORE and AFTER' },
    files_changed: { type: 'array', items: { type: 'string' } },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Fix')

const fixed = await agent(
  `${COMMON}

## Your items: C1-C15 from TUI-FIDELITY.md §3. Read those rows first — they carry the exact literals.

Order matters. Do **C1 first**: it is the only BEHAVIOURAL item here and the rest are cosmetic on top
of it.

**C1 — the context-usage segment has NEVER RENDERED.** \`StatusLine::set_context\` exists and nothing
in production calls it, so pi's \`{pct}%/{window} (auto)\` segment (\`footer.ts:161\`) is simply absent
from cyrup's footer. Wire it from the live session, and port the \`?/{window}\` branch for the
not-yet-known case. This is the footer's most-watched segment. Name the user action that makes it
appear, and drive that path in the test — not \`set_context\` directly. Calling the setter proves only
what already worked.

**C14 — cyrup draws a \`{n} queued\` segment pi does not have.** \`footer.ts:129-165\` builds exactly
\`↑ ↓ R W CH% $cost\` and nothing else. Verify that yourself before deleting: if pi has an equivalent
under another name, this becomes a rename, not a removal.

**C15 — the \`• xp\` experimental marker** (\`footer.ts:162-164\`, gated on
\`areExperimentalFeaturesEnabled()\`). Find cyrup's equivalent predicate and wire it.

**C2, C3, C9, C12** — the footer's own text: base colour via the \`dim\` token on lines 1 and 2; line 3
styling only the \`"..."\` ellipsis rather than the whole status text; \`Math.round\` token formatting
rather than integer division; and ASCII \`...\` instead of \`…\` in all four spinner messages. These are
literals — quote pi's exact bytes and match them character for character.

**C4, C5, C6, C7, C8, C13** — the status band and loaders: the 1-column left inset and trailing pad,
the cancel suffix that must NOT appear for \`IndicatorKind::Working\`, the loader message colour
(\`muted\`, not \`accent\`), the loader's row count (pi's is 7/5 where cyrup's is 4/3 — that is
\`Spacer(1)\` rows pi emits and cyrup does not), the retry countdown, and the startup compact hint bar.

**C10, C11** — the escape key label, and resolving the loader's cancel hint from \`tui.select.cancel\`
with ALL bound keys joined by \`/\` rather than hardcoding one.

For each: read pi, fix, add a test that FAILS without the fix plus a MIRROR that stays green, run the
revert proof with a cp-backup of the FIXED state. Then clippy.

Report \`user_visible\` for every item — this batch is judged on what changes on screen.`,
  { label: 'fix:C1-C15', phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
)

log(`Fix phase: ${(fixed?.items || []).length} items`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'footer_render'],
  properties: {
    tree_restored: { type: 'boolean' },
    footer_render: {
      type: 'string',
      description: 'Render cyrup\'s footer for a realistic session and paste it beside pi\'s expected line, segment by segment.',
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

**Lens 1 — render the footer and compare it to pi, segment by segment.** Build a realistic session
state (a model, token counts, a cost, a context percentage, a git branch) and produce cyrup's actual
footer line. Then write out what pi's \`footer.ts\` would produce for the same state, reading the
source. Put them side by side in \`footer_render\` and account for EVERY difference: segment order,
separator literal, padding at each end, and the format of each segment. This is on screen every
frame; a diff here is worth more than any amount of unit testing.

**Lens 2 — C1 reachability.** It claims a segment that has never rendered now renders. Trace the
chain from a real user action to \`set_context\`, and confirm the test drives that path rather than
calling the setter. This port has repeatedly shipped mechanisms wired to nothing — that is the entire
reason this item exists.

**Lens 3 — narrow-width behaviour.** pi hides segments as the terminal narrows. Check cyrup does the
same, in the same order, and that nothing panics or overflows at small widths. Try 40, 60, 80 columns.

**Lens 4 — weakened assertions.** \`git diff\` every pre-existing test that was edited and rule on
each: updated to pi's actual value with a pi line quoted, or loosened so a failure went away? Name
each verdict. Also check for renamed tests and shrunken bodies, which hide the same thing.

**Lens 5 — revert proofs and citations.** Re-run each revert with cp-backup / cp-restore, never
\`git checkout\`. Open every cited pi line at v0.84.1 and COUNT. Confirm each mirror stays green while
the fix-specific test fails.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-tui\` is green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`.

## What the author reported

${digest}

TESTS CHANGED: ${fixed?.tests_changed || '(none reported)'}
FILES: ${(fixed?.files_changed || []).join(', ')}
NOTES: ${fixed?.notes || '(none)'}`,
  { label: 'verify:footer', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-2',
  items: (fixed?.items || []).map((i) => ({ id: i.id, status: i.status })),
  footer_render: review?.footer_render,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
