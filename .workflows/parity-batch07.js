export const meta = {
  name: 'cyrup-parity-batch-07',
  description: 'pi-subagents to v0.43.0, part 1 — the behaviour present at the ported baseline and never ported',
  phases: [
    { title: 'Fix', detail: 'three groups inside cyrup-ext-subagents' },
    { title: 'Verify', detail: 'ONE reviewer; advertise-vs-dispatch is the invariant to check' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are moving \`cyrup-ext-subagents\` (${WS}/cyrup/crates/cyrup-ext-subagents) toward the CURRENT
state of \`pi-subagents\` (${WS}/pi-subagents), which is **v0.43.0**. cyrup ported from ~**v0.34.0**
— nine minor versions back. The backlog is \`${WS}/cyrup/PARITY-GAPS.md\`.

**The goal is v0.43.0, not "fix the old debt".** This batch is part 1 of a contiguous run: the
behaviour pi already had at the ported baseline and cyrup never ported. Parts 2-4 take the
run-loop, acceptance/state, and the three entirely-new subtrees (\`watchdog/\`, \`missions/\`,
\`tui/fleet*\`). Do not treat this batch as the finish line for anything.

NOTE the crate records NO version at all (zero \`v0.N.N\` strings). v0.34.0 is INFERRED from commit
dates and is if anything an over-estimate — the crate still ports \`companion-suggestions.ts\`, which
upstream deleted three days BEFORE v0.34.0. Treat the baseline as somewhere in v0.33.x-v0.34.0 and
**verify against the code**, not the tag.

## THE CRATE'S OWN INVARIANT — it will bite you

\`extension.rs:10036-10041\` enforces advertise-vs-dispatch: **every value the tool schema advertises
must have a real dispatch arm in the same change.** Several items here add enum values. A schema
that advertises a verb with no arm is worse than not adding it — the model will emit it and the run
will fail at dispatch.

There is also a guard test asserting every advertised schema property is actually READ (it scans the
source outside \`provided_keys\`). Adding a property without a reader fails it.

## THE FAILURE MODES THIS EFFORT KEEPS PRODUCING

1. **Ported-but-unwired** — a mechanism ported faithfully and wired to nothing. **Name the user
   action that reaches your change and drive THAT**, not the setter. This has recurred in five
   separate batches, most recently eight state seams whose only callers were tests.
2. **Untested fixes** — batches shipped six, then eleven, found only by disabling each behaviour and
   watching the suite stay green. Revert each change in turn before you finish.
3. **Claims inferred, not read** — two items were declared "genuinely blocked" because an agent
   grepped ONE SPELLING of a name. Both were false. **Search for the CAPABILITY, not the
   identifier**, and "an API I would have to add" is WORK, not a blocker.
4. **Row-level checks miss sequence and area bugs.** A user just found two defects ten batches
   missed, because every check compared one rendered block against its upstream counterpart and none
   drove a full interleaved sequence. Where your item has an ORDER or a LIFECYCLE, drive the whole
   thing.

## HARD RULES

- **READ pi-subagents' source. Never infer from a name, this brief, or the backlog.** Quote the
  \`file:line\` and COUNT (\`git -C ${WS}/pi-subagents show v0.43.0:src/<path>\`). Compare against
  v0.34.0 where classification matters. The backlog has been wrong repeatedly.
- \`spec/\` and \`ADR-0001\` do not exist in this workspace and are never authority for anything.
- **Fix, do not file.**
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` are separate crates needing their own
  file-level \`#![allow(...)]\`.
- **Never weaken an assertion.** Report each edited test BEFORE/AFTER.
- A subagent run is ALWAYS a real OS subprocess re-exec of the \`cyrup\` binary over NDJSON with real
  SIGINT→SIGTERM→SIGKILL escalation. **Never "simplify" it into in-process calls.**

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\`. **NEVER \`git checkout\`.**

## BUILD DISCIPLINE — AGENTS RUN IN PARALLEL

ONE shared \`target/\`, ONE \`.cargo-lock\`; cargo WILL block. Wait it out.
\`CARGO_INCREMENTAL=0\`; \`-p cyrup-ext-subagents\` only. **NEVER \`--workspace\`.**
Finish with \`cargo clippy -p cyrup-ext-subagents --all-targets; echo "exit=$?"\`.
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
          status: { type: 'string', enum: ['done', 'partial', 'already-implemented', 'blocked'] },
          summary: { type: 'string' },
          user_action: { type: 'string', description: 'What a user types/does to reach this. "A caller exists" is not an answer.' },
          upstream_citation: { type: 'string' },
          dispatch_arm: { type: 'string', description: 'For any new enum value: the dispatch arm added in the SAME change' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string', description: 'A mirror you actually RAN' },
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
    key: 'verbs',
    title: 'Management verbs — G90 steer, G91 schedule*, G92 view/lines + /subagents-fleet',
    brief: `All add values to the 15-verb management enum at \`src/extension.rs:5259\`. **The
advertise-vs-dispatch invariant applies to every one** — schema value and dispatch arm in the same
change, or the guard test fails and the model emits a verb that dies at dispatch.

**G90 — \`steer\`.** \`runs/foreground/subagent-executor.ts\` at v0.34.0 is literally
\`if (action === "steer") {\`. There is currently NO way to inject non-terminal guidance into a live
child. Read how upstream delivers it to a RUNNING subprocess — this is the control-inbox path, not a
restart.
**G91 — scheduled runs (4 verbs).** v0.34.0 ships \`runs/background/scheduled-runs.ts\`; dispatch at
\`subagent-executor.ts:3224\`. Four verbs; read what each does and what state they share.
**G92 — \`view\` + \`lines\`, and \`/subagents-fleet\`.** \`schemas.ts:233-237\`. Note \`tui/fleet*.ts\` is
a v0.35+ subtree and belongs to a LATER part of this run — port only what v0.34.0 has here, and say
where you drew the line.`,
  },
  {
    key: 'frontmatter',
    title: 'Agent definition — G95 memory:, G89 budgets, G98 launch defaults, G96 the YAML parser',
    brief: `**G96 first — the frontmatter parser is the foundation the others sit on.** Folded
scalars \`>\`/\`>-\` and block lists, plus the indent-anchor BUG: v0.34.0 already used \`/^([ \\t]+)/m\`
where cyrup takes the prefix from the block's first characters
(\`src/discovery/frontmatter.rs:210-230\`). Get the parser right before adding fields that depend on it.

**G95 — \`memory:\` scopes.** v0.34.0 \`agents/agent-memory.ts\`; \`KNOWN_FIELDS\` has \`memory\`.
**G89 — tool and turn budgets.** \`extension/schemas.ts:77-92\`; \`agent-serializer.ts\`'s
\`KNOWN_FIELDS\` has \`toolBudget\`. Today \`toolBudget:\` in an agent file is silently demoted to
\`extra_fields\` — a user writes it, nothing happens, no error.
**G98 — agent-level launch defaults** (async/timeoutMs/turnBudget/acceptance/skillPath/permissions/
runner), \`agents/agents.ts:1509-1584\`. Large; read the precedence rules carefully — a per-agent
default that silently overrides an explicit call-site argument is worse than not having it.

For each field: it must round-trip through the serializer AND actually change behaviour. A field
parsed into a struct that nothing reads is the ported-but-unwired defect.`,
  },
  {
    key: 'channel-removal',
    title: 'G106 native supervisor channel, G94 remove the companions surface',
    brief: `**G106 — the native supervisor channel.** \`intercom/native-supervisor-channel.ts\` exists
at v0.34.0, and \`intercom-bridge.ts:8\` exports \`NATIVE_INTERCOM_EXTENSION_DIR\`. Today, with no
intercom extension installed, a child has NO supervisor channel at all; upstream falls back to a
FILE channel. Marked partial in the backlog — establish what IS already there before writing.
Note cyrup's intercom crate is now at v0.9.2 (batch 2), so read the current cyrup code, not the
backlog's description of it.

**G94 — remove \`/subagents-companions\` and \`companionSuggestions\`.** Upstream DELETED
\`companion-suggestions.ts\` on 2026-07-03 — three days BEFORE v0.34.0 — and cyrup still ports it,
registers a 14th slash command and persists a config key upstream has not had for over a year.
This is a REMOVAL: find every reference (command registration, config key, docs, tests) and take
them all out. A removal that leaves the config key still parsed is half-done. Confirm the deletion
date and that nothing replaced it before removing anything.`,
  },
]

const authored = await parallel(
  GROUPS.map((g) => () =>
    agent(
      `${COMMON}

## Your group: ${g.title}

${g.brief}

For each item: read pi-subagents at v0.43.0 AND v0.34.0, fix, name the user action and drive it, add
a test that FAILS without the fix plus a MIRROR you actually RUN, revert-prove against a cp-backup of
the FIXED state. For any new enum value, state the dispatch arm in \`dispatch_arm\`. Then clippy.`,
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
  required: ['findings', 'tree_restored', 'advertise_dispatch_audit'],
  properties: {
    tree_restored: { type: 'boolean' },
    advertise_dispatch_audit: {
      type: 'string',
      description: 'Every schema value the crate advertises, and its dispatch arm. Name any advertised value with no arm, and any schema property with no reader.',
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
USER ACTION: ${a.user_action || '(none)'}
DISPATCH ARM: ${a.dispatch_arm || '(n/a)'}
UPSTREAM: ${a.upstream_citation || '(none)'}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 400)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. Do NOT fix; report.

**Lens 1 — advertise-vs-dispatch.** Enumerate EVERY value the crate's tool schemas advertise and
find its dispatch arm. Any advertised value without one is a BLOCKER: the model will emit it and the
run dies. Then the converse — every schema property must have a real READER outside
\`provided_keys\`. Fill \`advertise_dispatch_audit\`.

**Lens 2 — reachability.** Verify each claimed user action reaches the code. A frontmatter field
parsed into a struct that nothing reads is the ported-but-unwired defect that has recurred in five
batches. For G89/G95/G98, confirm the field CHANGES BEHAVIOUR, not just that it parses.

**Lens 3 — the removal (G94).** A removal is complete only when the command, the config key, its
parsing, its persistence, the docs and the tests are all gone. Grep for every spelling and report
any remainder.

**Lens 4 — lifecycle, not snapshots.** G90 (steer) and G91 (scheduled runs) have a LIFECYCLE:
a live child, a delivered message, a state change. Drive the whole sequence — a subagent run is a
real subprocess over NDJSON. A test that calls the handler directly proves nothing about delivery.

**Lens 5 — untested fixes, weakened assertions, citations.** Revert each change in turn and confirm
something goes red. \`git diff\` every edited test. Open every cited line at v0.43.0 and COUNT.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm \`cargo test -p cyrup-ext-subagents\` is green,
set \`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`; NEVER \`git checkout\`.

## What the agents reported

${digest}

TESTS CHANGED:
${ok.map((r) => `[${r.group}] ${r.tests_changed || '(none)'}`).join('\n')}

NOTES:
${ok.map((r) => `[${r.group}] ${r.notes || '(none)'}`).join('\n')}`,
  { label: 'verify:subagents', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'parity-7',
  items: allItems.map((i) => ({ group: i.group, id: i.id, status: i.status })),
  advertise_dispatch_audit: review?.advertise_dispatch_audit,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
