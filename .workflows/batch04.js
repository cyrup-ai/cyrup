export const meta = {
  name: 'cyrup-parity-batch-04',
  description: 'Batch 4 — eleven small corrections across four crates, parallel by crate (disjoint files)',
  phases: [
    { title: 'Author', detail: 'one agent per crate, in parallel — files are disjoint' },
    { title: 'Verify', detail: 'ONE reviewer; the risk here is weakened assertions, not broken code' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are working on \`cyrup\` (${WS}/cyrup), a Rust port targeting BEHAVIOURAL EQUIVALENCE with pi
(${WS}/pi). Ported baseline **v0.83.0**; target **v0.84.1** (latest).

Crate map: cyrup-provider <- ai/ · cyrup-tui <- tui/ + coding-agent/src/modes/interactive/ ·
cyrup-session <- agent/src/harness/session|compaction + coding-agent/.../session-manager.ts ·
cyrup-tools <- coding-agent/src/core/tools/ (pi's LIVE copy; agent/src/harness/tools/ is a thinner
second copy — check which one your item's citation points at).

## THE DEFINING RISK OF THIS BATCH

These are small corrections, and **several have existing tests that pin the CURRENT (wrong) value**.
When a test fails after your fix, there are two responses and only one is acceptable:
  - CORRECT: update the expectation to pi's actual behaviour, and quote the pi \`file:line\` that
    justifies the new value in the test or its comment.
  - UNACCEPTABLE: weaken the assertion, delete it, loosen a comparison, or add a tolerance so the
    failure goes away.
Report every test you changed with a before/after of the assertion. A reviewer will diff them and
judge which of the two you did. A silently weakened assertion is worse than an unported item,
because it converts a known gap into a false guarantee.

## HARD RULES

- **READ pi's SOURCE. NEVER infer its behaviour from a name, a type, or this brief.** The item
  descriptions below came from a survey and have been WRONG before — an entire batch was planned
  around "upstream is tolerant of unknown message tags" when it destroys the socket, and in the last
  batch my own worklist missed a fifth affected file and had two line numbers and one field name
  wrong. If what you read contradicts the brief, follow the source and say so.
- **Verify every citation**: \`git -C ${WS}/pi show v0.84.1:packages/<path>\` and COUNT the lines.
  Write version-qualified citations (\`v0.84.1 ai/src/api/google-shared.ts:71-79\`).
- **Classify**: if the behaviour existed at v0.83.0 it is a PORT BUG, not version lag. Say which in
  the doc comment. Check with \`git -C ${WS}/pi show v0.83.0:packages/<path>\`.
- **Fix, do not file.** If you find another defect in the same file, fix it or explain precisely why
  it is a genuinely separate change.
- \`spec/\` and \`ADR-0001\` do not exist in this workspace; neither is authority for anything.
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, and fires ONLY under \`cargo clippy\`. \`tests/\` files are separate crates needing
  their own file-level \`#![allow(...)]\`.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
first, edit, run, PASTE the failure, restore with \`cp\` FROM THE BACKUP.
**NEVER \`git checkout\`** — the tree holds uncommitted work and a stray \`git checkout\` destroyed a
1300-line file yesterday. A revert must remove EVERY guard the test covers; a partial revert proves
only what it removed.

## BUILD DISCIPLINE — IMPORTANT, OTHER AGENTS ARE RUNNING

ONE shared \`target/\`, ONE \`.cargo-lock\`. Other crate-agents are working in parallel right now, so
cargo WILL block on the lock — that is expected, wait it out, do not kill it.
- Prefix every cargo command with \`CARGO_INCREMENTAL=0\`. Disk is at ~91%.
- MAY run \`cargo check/test/clippy -p <your crate> --all-targets\`.
- **MUST NOT run any \`--workspace\` command**, and MUST NOT touch files outside your own crate —
  another agent owns those right now. Report cross-crate needs in \`registration_needed\`.
- Finish with \`cargo clippy -p <your crate> --all-targets; echo "exit=$?"\` and check the EXIT CODE.

## DEFINITION OF DONE, per item

1. Behaviour matches v0.84.1, verified version-qualified citation, port-bug/version-lag stated.
2. A test that FAILS without the change (full revert, pasted failure, cp-restore).
3. A MIRROR case that stays green, proving the change is not over-broad.
4. A named non-test caller.
5. Clippy exit 0, no new warnings.
`

const ITEM_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['crate', 'items'],
  properties: {
    crate: { type: 'string' },
    items: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['item', 'status', 'summary', 'revert_proof'],
        properties: {
          item: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'already-implemented', 'blocked'] },
          summary: { type: 'string' },
          classification: { type: 'string', enum: ['port-bug', 'version-lag', 'unclear'] },
          upstream_citation: { type: 'string' },
          files_changed: { type: 'array', items: { type: 'string' } },
          tests_added: { type: 'array', items: { type: 'string' } },
          tests_changed: { type: 'string', description: 'Every pre-existing test you edited, with the assertion BEFORE and AFTER and the pi line justifying the new value' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string' },
          caller: { type: 'string' },
        },
      },
    },
    clippy_exit: { type: 'string' },
    public_signature_change: { type: 'boolean' },
    registration_needed: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Author')

const CRATES = [
  {
    key: 'provider',
    crate: 'cyrup-provider',
    brief: `Five items, all in DIFFERENT files — no two touch the same one.

**G9 — Gemini 3 must echo tool-call ids.** \`requiresToolCallId\` needs a third arm
(v0.84.1 \`ai/src/api/google-shared.ts:71-79\`). cyrup: \`api/google_generative_ai.rs:522-526\`. A
helper \`gemini_major_version\` reportedly already exists at \`:536-540\` with the same \`>= 3\` test its
neighbour uses — \`requires_tool_call_id\` simply does not consult it. Verify that before assuming it.

**G6 — \`compat.supportsFinishReason\`.** When a provider does not report a finish reason, pi INFERS
stop vs toolUse (v0.84.1 \`ai/src/api/openai-completions.ts:578-584\`). cyrup:
\`api/openai_completions.rs:1519-1537\`. This is the fleet wire api behind 16 built-in providers, so
get the inference exactly right and test both branches.

**G11 — structured Bedrock failure diagnostics.** v0.84.1
\`ai/src/api/bedrock-converse-stream.ts:225,318-320\`. cyrup: \`api/bedrock_converse_stream.rs:454-462\`.

**G15 — OpenCode Go display name.** v0.84.1 \`ai/src/providers/opencode-go.ts:11\`; cyrup
\`providers/opencode_go.rs:44\`. A one-string change — but confirm nothing keys off the old string
(grep the workspace) before you change it.

**G13 (JSON-ONLY HALF) — catalog corrections.** GPT-5.6 pricing, Fireworks GLM-5.2/Kimi K3, Groq
Qwen, Copilot Grok 4.5, under \`providers/catalog/*.json\`. pi's catalogs are GENERATED, so the oracle
is \`ai/scripts/generate-models.ts\` (\`:389-393\`, \`:2308-2313\`) and the \`*.models.ts\` files — read the
generated values, do not invent them. Scope: correcting values in EXISTING catalog entries. If you
find models missing from a catalog entirely, that is a different change — report it, do not add them
here. Note the compat keys \`supportsLongCacheRetention\`/\`sendSessionAffinityHeaders\` reportedly
already exist at \`api/compat.rs:107-110\` and are already set on \`glm-5p1\` rows; the \`glm-5p2\` rows
reportedly just lack them. Verify.`,
  },
  {
    key: 'tui',
    crate: 'cyrup-tui',
    brief: `Three items, all in DIFFERENT files.

**G65 — batched colour-scheme reports: LAST wins, not first.** v0.84.1
\`tui/src/terminal-colors.ts:29\`; cyrup \`src/terminal_query.rs:265-270\`. When a terminal emits
several reports in one burst, pi keeps the last. cyrup keeps the first, so it can settle on a stale
colour. Read what pi does with a partial/truncated report in the same burst.

**G64 — assume truecolor on Windows consoles without \`WT_SESSION\`.** v0.84.1
\`tui/src/terminal-image.ts:74,122-129\`; cyrup \`src/image.rs:478-485\`. Marked \`partial\`, so
establish what IS already there before changing anything. This is Windows-only behaviour and cannot
be exercised on this Linux box: write the test so it drives the DECISION FUNCTION with a synthesized
environment rather than the real one, and say plainly in your report that the platform path itself
is unexercised here.

**G50 — shorter length-stop notice.** v0.84.1
\`coding-agent/src/modes/interactive/components/assistant-message.ts:177-181\`; cyrup
\`src/app.rs:4097-4106\`. A user-visible string: quote pi's exact text and match it character for
character, including punctuation and capitalisation.`,
  },
  {
    key: 'session',
    crate: 'cyrup-session',
    brief: `Two items.

**G30 — \`AGENTS.override.md\` as the FIRST context-file candidate.** v0.84.1
\`coding-agent/src/core/resource-loader.ts:71\`; cyrup \`src/prompt/context_files.rs:63\`. Order matters:
read pi's full candidate list and confirm both the new entry AND the relative order of the existing
ones. Test that the override actually WINS over the file it precedes, not merely that it is present
in the list.

**G21 — \`fromHook\` still suppresses file-list inheritance.** v0.84.1
\`agent/src/harness/compaction/compaction.ts:52\`; cyrup \`src/compaction/prepare.rs:92\` and
\`branch.rs:176-179\`.

  CAREFUL — pi has TWO forked compaction implementations:
  \`agent/src/harness/compaction/compaction.ts\` and
  \`coding-agent/src/core/compaction/compaction.ts\`. They diverged on 2026-07-09; the coding-agent
  one is pi's LIVE path, and cyrup ported the HARNESS copy. Your citation points at the harness
  copy. Read BOTH, state whether they agree on this behaviour, and if they disagree say so
  explicitly in your report rather than silently picking one.`,
  },
  {
    key: 'tools',
    crate: 'cyrup-tools',
    brief: `One item.

**G41 — softened \`PI_*\` env guideline in the bash prompt contribution.** v0.84.1
\`coding-agent/src/core/tools/bash.ts:45-48\`; cyrup \`src/tools/bash.rs:96-103\`.

Two things to get right:
  - This text goes into the SYSTEM PROMPT, so it changes model behaviour. Match pi's wording
    exactly, then consider the rebrand: cyrup renamed \`PI_*\` env vars to \`CYRUP_*\` but the \`PI_*\`
    names remain LIVE as lower-precedence fallbacks (twelve of them, \`cyrup-config/src/env.rs\`).
    Decide what the ported text should say for cyrup and justify it — a blind find-and-replace of
    \`PI_\` to \`CYRUP_\` may be wrong if the guidance is about the user's environment rather than
    cyrup's own variables. Say what you chose and why.
  - Confirm you are reading pi's LIVE tool copy (\`coding-agent/src/core/tools/bash.ts\`), not the
    thinner harness copy at \`agent/src/harness/tools/bash.ts\`.

There is likely an existing snapshot/golden test pinning the current prompt text. Update it to the
new expected text — do not weaken it — and show the before/after.`,
  },
]

const authored = await parallel(
  CRATES.map((c) => () =>
    agent(
      `${COMMON}

## Your crate: \`${c.crate}\` — you own ONLY files under \`crates/${c.crate}/\`

${c.brief}

Work through your items one at a time in ${WS}/cyrup. For each: read pi, edit, test, full revert
proof with cp-backup, then move on. Finish with clippy for your crate and report everything.`,
      { label: `author:${c.key}`, phase: 'Author', schema: ITEM_SCHEMA, effort: 'high' }
    )
  )
)

const ok = authored.filter(Boolean)
const allItems = ok.flatMap((r) => (r.items || []).map((i) => ({ ...i, crate: r.crate })))
log(`Authored ${allItems.length} items across ${ok.length} crates`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'weakened_assertions'],
  properties: {
    tree_restored: { type: 'boolean' },
    weakened_assertions: {
      type: 'string',
      description: 'Your verdict on EVERY changed test: updated-to-pi-correct, or weakened. Name each.',
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

const digest = allItems
  .map(
    (a) => `### [${a.crate}] ${a.item} [${a.status}] (${a.classification || 'unclassified'})
${a.summary}
UPSTREAM: ${a.upstream_citation || '(none)'}
FILES: ${(a.files_changed || []).join(', ')}
TESTS ADDED: ${(a.tests_added || []).join(', ')}
TESTS CHANGED: ${(a.tests_changed || '(none reported)').slice(0, 900)}
MIRROR: ${a.mirror_case || '(none)'}
CALLER: ${a.caller || '(none)'}
REVERT PROOF: ${(a.revert_proof || '(none)').slice(0, 700)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review all eleven items. Do NOT fix anything; report.

**Lens 1 — WEAKENED ASSERTIONS. This is the point of this review.** Several of these items had
tests pinning the OLD, wrong value. For EVERY pre-existing test that was edited, run
\`git diff -- <test file>\` yourself and judge: was the expectation updated to pi's actual behaviour
(with a pi line justifying it), or was the assertion weakened, loosened, given a tolerance, or
deleted so a failure would go away? Fill \`weakened_assertions\` with a verdict per test, naming each.
A weakened assertion is worse than an unported item: it converts a known gap into a false guarantee.
Also look for tests that were RENAMED or whose body shrank — those hide the same thing.

**Lens 2 — citations and classification.** Open every cited line at v0.84.1 and COUNT. Then check
each port-bug/version-lag call against v0.83.0. My own worklist for the previous batch had two
drifted line numbers, one wrong field name, and missed an entire affected file — so verify, do not
trust. For G13, confirm the catalog values came from pi's generated \`*.models.ts\` and were not
invented; spot-check at least three numbers against upstream.

**Lens 3 — over-broad changes.** Each item is meant to be small. For each, check the change did not
alter a neighbouring behaviour: G6 is the wire api behind 16 providers, G9 must not change
behaviour for Gemini < 3, G30 must not reorder the EXISTING context-file candidates, G50 and G41
are user- and model-facing STRINGS that must match pi character for character.

**Lens 4 — revert proofs.** Re-run each yourself with cp-backup/cp-restore; never \`git checkout\`.
Confirm each mirror stays green while the fix-specific test fails.

**Lens 5 — G21's forked upstream.** pi has two compaction implementations that diverged on
2026-07-09; cyrup ported the harness copy while coding-agent's is pi's live path. Confirm the author
actually read both and reported whether they agree, rather than silently citing one.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm the affected crates are green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
Prefix cargo with \`CARGO_INCREMENTAL=0\`; never run \`--workspace\`.

${digest}`,
  { label: 'verify:all-lenses', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 4,
  items: allItems.map((a) => ({ crate: a.crate, item: a.item, status: a.status, classification: a.classification })),
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  registration_needed: ok.map((r) => r.registration_needed).filter(Boolean),
  overall: review?.overall,
}
