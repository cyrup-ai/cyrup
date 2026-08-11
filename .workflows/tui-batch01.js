export const meta = {
  name: 'cyrup-tui-batch-01-colour-accessors',
  description: 'TUI batch 1 — the eight theme.rs colour-accessor defects (T1-T9), highest visible payoff per line',
  phases: [
    { title: 'Fix', detail: 'one crate, one file — sequential' },
    { title: 'Verify', detail: 'ONE reviewer; snapshot churn is where a regression hides' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are fixing PRESENTATION FIDELITY defects in \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against
pi (${WS}/pi) at **v0.84.1**. These come from a read-only audit whose findings are in
\`${WS}/cyrup/TUI-FIDELITY.md\` — read §2 before you start; every claim there was verified on both
sides, and the IDs (T1..T9) are the ones to cite in doc comments and commit text.

## THE STATE OF PLAY — read this, it will save you a wrong turn

The audit established two things that contradict the obvious assumptions:

  - **The palette DATA is clean.** \`cyrup-resources/src/theme.rs:518-600\` (dark) and \`:602-684\`
    (light) match pi's \`dark.json\`/\`light.json\` token for token, all 50 of them. The light theme is
    fully present and \`ThemeController\` faithfully ports \`resolveThemeSetting\`. Do NOT go
    "fixing" the palette.
  - **There are NO hardcoded RGB colours in any renderer.** \`grep "Color::"\` over
    \`crates/cyrup-tui/src/*.rs\` finds only \`Color::Reset\` equality tests and \`unwrap_or\` fallbacks.

The damage is entirely in **style CONSTRUCTION** in \`cyrup-tui/src/theme.rs\`: an accessor resolving
the wrong role, or adding an SGR attribute pi never emits. That is why one file fixes 29 call sites.

## HARD RULES

- **READ pi's source for every change. Never infer from a name or from this brief.** Briefs in this
  project have been wrong repeatedly — a cited line containing nothing of the sort, an item filed
  under the wrong crate, a whole batch planned on a false premise. If the source disagrees with the
  audit or with me, FOLLOW THE SOURCE and say so in your report.
- **Verify every citation**: \`git -C ${WS}/pi show v0.84.1:packages/<path>\` and COUNT the lines.
  Write version-qualified citations.
- \`spec/\` and \`ADR-0001\` do not exist in this workspace and are never authority for anything.
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code and fires ONLY under \`cargo clippy\`. \`tests/\` files are separate crates needing
  their own file-level \`#![allow(...)]\`.
- **Fix, do not file.** If you find another defect in \`theme.rs\`, fix it or say precisely why it is
  a separate change.

## SNAPSHOTS — the specific hazard of this batch

\`crates/cyrup-tui/tests/\` holds render-snapshot tests (\`render.rs\`, \`assembled_render.rs\`, others).
Changing a colour accessor WILL churn them. For each one that changes:
  - Read the diff and confirm the new bytes are what pi would produce. Quote the pi line.
  - Regenerate deliberately, and list in \`snapshots_changed\` every snapshot you touched with a
    one-line justification.
  - **Never** loosen an assertion, drop a colour check, or widen a comparison to make a snapshot
    pass. If a snapshot cannot be justified against pi, STOP and report it — that means the fix is
    wrong, not the snapshot.
A regenerated snapshot is the easiest place in this repo to hide a regression, so it gets the
scrutiny.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
BEFORE editing, restore with \`cp\` FROM THAT BACKUP. **NEVER \`git checkout\`** — the tree holds
uncommitted work and a stray \`git checkout\` destroyed a 1300-line file. Take the backup of the
FIXED state before reverting, or you will restore a pre-fix copy and wipe your own work (that
happened yesterday). A revert must remove EVERY guard the test covers.

## BUILD DISCIPLINE

ONE shared \`target/\`, ONE \`.cargo-lock\`; prefix cargo with \`CARGO_INCREMENTAL=0\`. Disk is tight.
MAY run \`cargo check/test/clippy -p cyrup-tui --all-targets\` (and \`-p cyrup-resources\` if a token
definition genuinely needs reading). **MUST NOT run any \`--workspace\` command** — the orchestrator
gates once. Finish with \`cargo clippy -p cyrup-tui --all-targets; echo "exit=$?"\`.
`

const ITEM_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['items', 'snapshots_changed'],
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
          call_sites_affected: { type: 'string', description: 'How many render sites this accessor feeds, and how you counted' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string' },
        },
      },
    },
    snapshots_changed: { type: 'string', description: 'Every snapshot/fixture regenerated, with the pi line justifying the new bytes' },
    tests_changed: { type: 'string', description: 'Every pre-existing test edited: assertion BEFORE and AFTER' },
    files_changed: { type: 'array', items: { type: 'string' } },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Fix')

const fixed = await agent(
  `${COMMON}

## Your items: T1-T9 from TUI-FIDELITY.md §2. Read that section in full first.

Work them in this order — T1 first because it is the single highest-payoff line in the file.

**T1 — \`dim_style()\` resolves the wrong role AND adds an attribute pi never uses.**
\`theme.rs:292-299\` builds \`Style::default().add_modifier(Modifier::DIM).fg(self.foreground)\` — the
\`text\` role. pi's \`fg("dim")\` (\`theme.ts:372-376\`) emits a plain foreground escape with NO SGR
attribute, resolving the \`dim\` TOKEN (\`dark.json:34\` = \`#666666\`, \`light.json:33\` = \`#767676\`).
\`grep '"dim"' crates/cyrup-tui/src/*.rs\` finds it only in doc comments — the role is never read.
Two consequences: on terminals that drop SGR 2 (Terminal.app, much of tmux, Windows consoles) every
hint renders at full body brightness; and in the LIGHT theme hints come out near-black \`#1f2328\`
where pi draws grey. 29 call sites. Count them yourself and report the number.

**T4 — \`error_style()\` bakes in bold.** \`theme.rs:287-290\` adds \`Modifier::BOLD\`. pi's \`fg()\` is
colour-only and \`bold()\` is a separate combinator (\`theme.ts:390\`); every pi error site is
unbolded — verify with \`git grep -c 'bold(theme.fg("error"' \` at v0.84.1, which the audit reports
as 0.

**T2 — truecolor detection.** \`theme.rs:35-49 ColorMode::detect()\` reads only \`COLORTERM\` and
\`TERM\`. pi's \`detectCapabilities\` (\`tui/src/terminal-image.ts:68-135\`) has a terminal-PROGRAM
table — kitty, ghostty, wezterm, warp, iTerm2 (\`:105\`), Windows Terminal via \`WT_SESSION\` (\`:109\`),
vscode, alacritty, jediterm, win32 — and treats \`COLORTERM\` as a strict-equality FALLBACK hint
(\`:72\`), not the gate. Consequence: on iTerm2 / Windows Terminal / JetBrains (none set
\`COLORTERM\`) the whole palette is quantised through \`rgb_to_256\` and the three tool background
tints collapse into near-identical cube cells.
  NOTE: another agent recently touched this area for a Windows-console item and introduced
  \`detect_capabilities_on_platform\`, a parameterised core. Read what is already there before adding
  a second detection path.

**T3 — the monochrome arm.** \`theme.rs:41-44\` returns \`ColorMode::None\` for \`TERM=dumb\` or empty,
stripping every colour. pi has no such mode: \`type ColorMode = "truecolor" | "256color"\`
(\`theme.ts:167\`). Check whether T2's port subsumes this before writing a separate fix.

**T5 — the syntax-highlight green fallback.** \`markdown.rs:690\` takes \`md_code_block_style()\` as
the flat default and \`:726\` does \`scope_style(...).unwrap_or(flat)\`, so every scope outside the
13-prefix table (\`theme.rs:578-621\`) renders as \`mdCodeBlock\` = \`#b5bd68\` GREEN. pi pushes
unclassified code with NO style (\`markdown.ts:526-527\`), leaving it at terminal default. Roughly
half a typical code block is the wrong colour. Note this one is in \`markdown.rs\`, not \`theme.rs\`.

**T6 — \`meta\` scopes.** \`theme.rs:578-621\` maps only \`meta.attribute\`; pi maps all \`meta\` to
\`muted\` (\`theme.ts:1128\`). Rust attributes, Python decorators and C preprocessor lines are wrong.

**T7, T8, T9** — the remaining §2 rows: the markdown link-URL style, \`tool_title_style()\` /
\`user_message_bg_style()\` reading \`self.foreground\` instead of their own roles, and the six dead
theme tokens. Read the table rows for the exact literals.

For each item: read pi, fix, add a test that FAILS without the fix and a MIRROR that stays green,
run the revert proof with a cp-backup of the FIXED state, and move on. Then run clippy.

Report \`call_sites_affected\` for T1 and T4 specifically — those two are the ones whose blast radius
justifies the batch.`,
  { label: 'fix:T1-T9', phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
)

log(`Fix phase: ${(fixed?.items || []).length} items`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'snapshot_verdict'],
  properties: {
    tree_restored: { type: 'boolean' },
    snapshot_verdict: {
      type: 'string',
      description: 'Per regenerated snapshot: are the new bytes what pi produces? Name each and cite the pi line.',
    },
    colour_table: {
      type: 'array',
      description: 'One row per accessor changed: the resolved colour before, after, and pi\'s value, per theme.',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['accessor', 'before', 'after', 'pi'],
        properties: {
          accessor: { type: 'string' },
          before: { type: 'string' },
          after: { type: 'string' },
          pi: { type: 'string' },
          matches: { type: 'boolean' },
        },
      },
    },
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
CALL SITES: ${a.call_sites_affected || '(not counted)'}
MIRROR: ${a.mirror_case || '(none)'}
REVERT PROOF: ${(a.revert_proof || '(none)').slice(0, 700)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. Do NOT fix anything; report.

**Lens 1 — resolved colours, per theme.** For every accessor changed, compute what it now resolves
to in BOTH the dark and light themes, and compare against pi's token value in
\`dark.json\`/\`light.json\`. Fill \`colour_table\`. A fix that corrects dark and leaves light wrong is
half a fix — the light theme is fully present in cyrup, so there is no excuse for skipping it.

**Lens 2 — snapshots.** This is where a regression hides. For EVERY regenerated snapshot, read the
diff and decide: do the new bytes match what pi would emit? Cite the pi line. Then check nothing was
loosened — no dropped colour assertion, no widened comparison, no deleted case. Report per snapshot
in \`snapshot_verdict\`.

**Lens 3 — SGR attributes.** T1 and T4 are both about an attribute pi never emits. Verify NO
remaining accessor adds \`Modifier::DIM\` or \`Modifier::BOLD\` where pi's equivalent is a plain
\`fg()\`. Grep the whole file for \`add_modifier\` and check each against pi's construction.

**Lens 4 — blast radius.** T1 claims 29 call sites. Count them yourself. Then spot-check three that
the author did NOT mention and confirm each now renders pi's colour — an accessor fix that misses a
consumer which builds its own style inline is not done.

**Lens 5 — revert proofs and citations.** Re-run each revert with cp-backup / cp-restore, never
\`git checkout\`. Open every cited pi line at v0.84.1 and COUNT. Confirm each mirror stays green while
the fix-specific test fails.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-tui\` is green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`.

## What the author reported

${digest}

SNAPSHOTS CHANGED: ${fixed?.snapshots_changed || '(none reported)'}
TESTS CHANGED: ${fixed?.tests_changed || '(none reported)'}
FILES: ${(fixed?.files_changed || []).join(', ')}
NOTES: ${fixed?.notes || '(none)'}`,
  { label: 'verify:colour', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'tui-1',
  items: (fixed?.items || []).map((i) => ({ id: i.id, status: i.status })),
  colour_table: review?.colour_table || [],
  mismatches: (review?.colour_table || []).filter((r) => r.matches === false),
  snapshot_verdict: review?.snapshot_verdict,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
