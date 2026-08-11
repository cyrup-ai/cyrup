export const meta = {
  name: 'cyrup-parity-batch-08',
  description: 'pi-subagents to v0.43.0, part 2 — run-loop correctness (G74, G75, G76, G81, G84, G88, G100, G102, G103)',
  phases: [
    { title: 'Fix', detail: 'three groups inside cyrup-ext-subagents' },
    { title: 'Verify', detail: 'ONE reviewer; mutation testing is the primary lens' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are moving \`cyrup-ext-subagents\` (${WS}/cyrup/crates/cyrup-ext-subagents) toward the CURRENT
state of \`pi-subagents\` (${WS}/pi-subagents) — **v0.43.0**. cyrup ported from ~**v0.34.0**. The
backlog is \`${WS}/cyrup/PARITY-GAPS.md\`. This is part 2 of a contiguous run; parts 3-4 take
acceptance/state and the three new subtrees.

## THE GATE — USE THE RIGHT ONE

\`cyrup-ext-subagents\` has two \`[[bin]]\` targets with \`required-features = ["test-fixtures"]\`.
**Without that flag the tests that drive a real subagent subprocess DO NOT RUN.** Two tests sat red
for three days behind it while every gate reported clean. Always:

    CARGO_INCREMENTAL=0 cargo test -p cyrup-ext-subagents --features test-fixtures --no-fail-fast
    CARGO_INCREMENTAL=0 cargo clippy -p cyrup-ext-subagents --all-targets --features test-fixtures

## THE FAILURE MODES THIS EFFORT KEEPS PRODUCING — all four have recurred

1. **Ported-but-unwired.** Five batches shipped mechanisms whose only callers were tests. Last batch,
   \`steer\` wrote accepted requests to a directory NOTHING EVER READ — a dead letter with a green
   suite. **Name the user action that reaches your change and drive THAT.**
2. **Untested fixes.** Found only by MUTATION: break the behaviour, see if anything goes red. Last
   batch three behaviours were dropped-safe that way. Before you finish, revert each change in turn.
3. **"Blocked" has been WRONG THREE TIMES OUT OF THREE**, each from grepping one spelling of a name.
   The last one was blocked on a capability THIS CRATE ALREADY CALLED IN TWO PLACES. **Search for
   the CAPABILITY, not the identifier.** "An API I would have to add" is WORK, not a blocker.
4. **Wrong scope of check.** Rows instead of sequences; one package instead of the workspace. If your
   item has a LIFECYCLE (a spawn, a stream, a retry ladder), drive the whole thing.

## THE CRATE'S OWN INVARIANTS

- \`extension.rs:10036-10041\` — every advertised schema value needs a real dispatch arm in the SAME
  change; every advertised property needs a real reader outside \`provided_keys\` (the guard now
  excises every such body).
- A subagent run is ALWAYS a real OS subprocess re-exec of the \`cyrup\` binary over NDJSON with real
  SIGINT→SIGTERM→SIGKILL escalation. **Never simplify it into in-process calls.**

## HARD RULES

- **READ pi-subagents' source. Never infer from a name, this brief, or the backlog.** Quote the
  \`file:line\` and COUNT (\`git -C ${WS}/pi-subagents show v0.43.0:src/<path>\`; compare v0.34.0 where
  classification matters). The backlog has been wrong repeatedly.
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for anything.
- **Fix, do not file.**
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code, fires ONLY under \`cargo clippy\`. \`tests/\` need their own file-level \`#![allow]\`.
- **Never weaken an assertion.** Report each edited test BEFORE/AFTER.
- **A \`pub\` signature change is invisible to \`-p\` builds.** Last batch broke struct literals in two
  OTHER crates' \`tests/\` that way. If you change one, say so loudly in \`notes\`.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
— back up the FIXED state BEFORE reverting, restore with \`cp\`. **NEVER \`git checkout\`.**

## BUILD DISCIPLINE — AGENTS RUN IN PARALLEL

ONE shared \`target/\`, ONE \`.cargo-lock\`; cargo WILL block. Wait it out.
\`CARGO_INCREMENTAL=0\`; \`-p cyrup-ext-subagents --features test-fixtures\` only. **NEVER \`--workspace\`.**
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
          user_action: { type: 'string' },
          upstream_citation: { type: 'string' },
          mutation_proof: { type: 'string', description: 'Break the behaviour; name the test that goes red' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string' },
        },
      },
    },
    public_signature_change: { type: 'boolean' },
    tests_changed: { type: 'string' },
    files_changed: { type: 'array', items: { type: 'string' } },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Fix')

const GROUPS = [
  {
    key: 'stream',
    title: 'The NDJSON stream — G75 bounded reader, G76 drain start/cancel, G74 startup retry',
    brief: `**G75 — the reader is UNBOUNDED.** No 16 MiB line cap, no \`protocol_output_limit\`
diagnostic. A child emitting one enormous line can exhaust the parent's memory — this is a real
robustness hole on a path that reads from a subprocess. Find pi's cap and its diagnostic, and port
both. Test with a line over the limit.

**G76 — drain start/cancel.** \`ndjson.rs:156\` already PARSES \`will_retry\` and then does nothing
with it. Consume it. Read what upstream does on a drain start and on a cancel — this is a
lifecycle, so drive the whole sequence against a real subprocess, not a parser unit test.

**G74 — startup retry.** Read pi's retry ladder for a child that fails to start: how many attempts,
what backoff, and what distinguishes "failed to start" from "started and exited". Getting that
boundary wrong either masks a broken binary or retries a legitimate non-zero exit.`,
  },
  {
    key: 'resolution',
    title: 'Resolution and discovery — G88 model + TOOL_FAILURE guard, G81 $ref rewrite, G102 pruning, G100 profile merge',
    brief: `**G88 — model resolution and the \`TOOL_FAILURE\` prefix guard.** The backlog notes this is
live TODAY: cyrup's own formatter emits the guarded prefix at \`exec/output.rs:352-359\`. Establish
what pi guards against and why — a child whose output legitimately begins with that prefix must not
be misclassified as a tool failure.

**G81 — \`$ref\` rewrite.** In the structured-output schema path. Read what upstream rewrites and
when; a \`$ref\` left unresolved produces a schema the model cannot satisfy.

**G102 — discovery pruning.** What upstream prunes from the discovered agent set, and why.

**G100 — profile merge.** \`src/profiles/profiles.ts\` changed only +30/-6 between v0.34.0 and
v0.43.0, so read both and port the delta precisely; the merge ORDER is the thing to get right —
a profile that silently overrides an explicit call-site argument is worse than no profile.`,
  },
  {
    key: 'edges',
    title: 'Edge cases — G103 empty tools list, G84 mutation detection',
    brief: `**G103 — an empty tools list.** Establish what upstream does when an agent declares
\`tools: []\` versus omitting \`tools:\` entirely. These are different: one is "no tools", the other is
"inherit". If cyrup collapses them, an agent asking for no tools silently gets all of them — check
which way round it is before writing.

**G84 — mutation detection.** Read what upstream detects as a mutation and what it does about it.
Name the user-visible consequence, because this one is easy to port as a no-op.

Both are small; spend the time on establishing the exact upstream semantics rather than on the code.
If either turns out to be already-implemented, say so with the file:line and move on — that is a
legitimate outcome and better than inventing a difference.`,
  },
]

const authored = await parallel(
  GROUPS.map((g) => () =>
    agent(
      `${COMMON}

## Your group: ${g.title}

${g.brief}

For each item: read pi-subagents at v0.43.0, fix, name the user action and drive it, add a test that
FAILS without the fix plus a MIRROR you actually RUN, and give a MUTATION PROOF — break the
behaviour and name the test that goes red. Then clippy with the feature flag.`,
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
  required: ['findings', 'tree_restored', 'mutation_audit'],
  properties: {
    tree_restored: { type: 'boolean' },
    mutation_audit: {
      type: 'string',
      description: 'Per item: break the behaviour yourself and name what goes red. Report every one that leaves the suite green.',
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
UPSTREAM: ${a.upstream_citation || '(none)'}
MUTATION: ${a.mutation_proof || '(none given)'}
MIRROR: ${a.mirror_case || '(none)'}
REVERT: ${(a.revert_proof || '(none)').slice(0, 350)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. Do NOT fix; report.

**Lens 1 — MUTATION, the primary lens.** For EVERY item, break the behaviour yourself and run the
full crate suite WITH \`--features test-fixtures\`. Report every item that leaves the suite green —
that is an untested fix, and it has caught six, then eleven, then three in consecutive batches.

**Lens 2 — reachability.** Verify each claimed user action reaches the code in production. Last
batch \`steer\` wrote to a directory nothing read. Trace from the entry point to the effect.

**Lens 3 — lifecycles, driven whole.** G74/G75/G76 are stream and retry behaviours. A parser unit
test proves nothing about a real subprocess. Confirm the tests spawn one, and check the boundary
cases: an oversized line, a cancel mid-drain, a child that dies before its first byte.

**Lens 4 — cross-crate.** Any public signature change is invisible to per-package builds; last batch
broke two other crates' test files that way. Grep the workspace for constructors of anything whose
shape changed.

**Lens 5 — weakened assertions and citations.** \`git diff\` every edited test and rule on each. Open
every cited line at v0.43.0 and COUNT.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm the crate is green WITH the feature flag, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match.
\`CARGO_INCREMENTAL=0\`; never \`--workspace\`; NEVER \`git checkout\`.

## What the agents reported

${digest}

PUBLIC SIGNATURE CHANGES:
${ok.map((r) => `[${r.group}] ${r.public_signature_change ? 'YES' : 'no'} — ${(r.notes || '').slice(0, 200)}`).join('\n')}

TESTS CHANGED:
${ok.map((r) => `[${r.group}] ${r.tests_changed || '(none)'}`).join('\n')}`,
  { label: 'verify:runloop', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 'parity-8',
  items: allItems.map((i) => ({ group: i.group, id: i.id, status: i.status })),
  mutation_audit: review?.mutation_audit,
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
