export const meta = {
  name: 'cyrup-parity-batch-03',
  description: 'Batch 3 — session/interop safety: deferred stop reason, Responses error fallback, JSON/RPC delta projection',
  phases: [
    { title: 'Recon', detail: 'read pi at v0.84.1 for all three; do not infer semantics' },
    { title: 'Author', detail: 'sequential — cargo serializes on one target dir anyway' },
    { title: 'Verify', detail: 'ONE reviewer, all lenses, explicit-backup reverts' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are working on \`cyrup\` (${WS}/cyrup), a Rust port targeting BEHAVIOURAL EQUIVALENCE with pi
(${WS}/pi). Ported baseline **v0.83.0**; target **v0.84.1** (latest).

Crate map for this batch:
  cyrup-core        <- ai/src/types.ts            (StopReason, Message)
  cyrup-session     <- agent/src/harness/session/ (session load/save, context)
  cyrup-provider    <- ai/                        (wire APIs, incl. openai-responses)
  cyrup-modes       <- coding-agent/src/modes/    (print-mode, json, rpc)
  cyrup-session-svc <- coding-agent/src/core/agent-session*.ts  (THE integration seam)

## THE RULE THAT MATTERS MOST IN THIS BATCH

**READ pi's SOURCE. DO NOT INFER ITS BEHAVIOUR FROM TYPES OR NAMES.** The previous batch was
planned around "upstream is tolerant of unknown message tags". It is not — it destroys the socket.
That error survived a survey, a plan and a review, and was caught only when someone finally opened
the file. Two further conclusions in that batch were also reversed by reading the source. Every
claim you make about what pi does must come with the \`file:line\` you read it at, quoted.

## HARD RULES

- \`spec/\` and \`ADR-0001\` DO NOT EXIST in this workspace, despite thousands of citations to them in
  cyrup's own doc comments. Neither is ever authority for leaving a difference in place.
- **Fix it, do not file it.** If you find a second defect while working your item, fix that too, or
  say precisely why it is a different change. "Tracked as a follow-up" is not an acceptable
  resolution for something you have already found and understood.
- **Check CONSUMERS, not just primitives.** A type or function nothing calls is not done. Name the
  non-test caller.
- **Verify every citation** at the right tag: \`git -C ${WS}/pi show v0.84.1:packages/<path>\`, and
  COUNT the lines. Prefer version-qualified citations (\`v0.84.1 ai/src/types.ts:391\`).
- **No-panic policy**: \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` are DENIED by clippy
  in non-test code and fire ONLY under \`cargo clippy\`. \`tests/\` files are separate crates that
  inherit nothing — they need their own file-level \`#![allow(...)]\`.
- Never hand-write a large artifact to \`std::env::temp_dir()\` — use \`tempfile\`.

## REVERT PROOFS — READ THIS, IT COST US A FILE

To prove a test fails without its fix:
  1. \`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<name>.SAFE\`
  2. edit, run the test, PASTE the failure
  3. restore with \`cp\` FROM THAT BACKUP.
**NEVER \`git checkout\` a file.** The tree has uncommitted work from this batch; \`git checkout\`
discarded a 1,300-line file that way and it had to be rebuilt from tests.
A revert must remove EVERY guard the test claims to cover. A partial revert that fails 3 of 5 tests
is not proof for the other 2 — that mistake was made and nearly reported as a complete proof.

## BUILD DISCIPLINE

ONE shared \`target/\`, ONE \`.cargo-lock\`; concurrent cargo invocations serialize.
- MAY run \`cargo check/test/clippy -p <crate> --all-targets\`. MUST NOT run any \`--workspace\`
  command — the orchestrator gates the batch once.
- \`cargo check -p\` is NOT a gate for a \`pub\` signature change: cross-crate callers, including
  other crates' \`tests/\`, stay invisible. If you change a public signature, say so explicitly.
- Finish with \`cargo clippy -p <crate> --all-targets; echo "exit=$?"\` and check the EXIT CODE.

## DEFINITION OF DONE

1. Behaviour matches v0.84.1, with a verified version-qualified citation.
2. A test that FAILS without your change — full revert, pasted failure, backup-restore.
3. A MIRROR case that stays green, proving the change is not over-broad.
4. A named non-test caller.
5. Clippy exit 0, no new warnings.
6. Report cross-crate edits you could not make in \`registration_needed\`.
`

const ITEM_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['item', 'status', 'summary', 'files_changed', 'revert_proof'],
  properties: {
    item: { type: 'string' },
    status: { type: 'string', enum: ['done', 'partial', 'already-implemented', 'blocked'] },
    summary: { type: 'string' },
    upstream_citation: { type: 'string' },
    files_changed: { type: 'array', items: { type: 'string' } },
    tests_added: { type: 'array', items: { type: 'string' } },
    revert_proof: { type: 'string' },
    mirror_case: { type: 'string' },
    caller: { type: 'string' },
    public_signature_change: { type: 'boolean' },
    registration_needed: { type: 'string' },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Recon')

const RECON_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['deferred', 'responses_error', 'json_event'],
  properties: {
    deferred: { type: 'string', description: 'What "deferred" IS at v0.84.1, quoted with file:line' },
    deferred_round_trip: { type: 'string', description: 'What a pi session JSONL containing it looks like, and what cyrup does with it today' },
    responses_error: { type: 'string', description: 'The exact v0.84.1 fallback behaviour, quoted' },
    json_event: { type: 'string', description: 'What toJsonEvent projects and what both cyrup serializers emit today' },
    scope_warnings: { type: 'string', description: 'Anything in these three that is larger than the plan assumes, or where the plan is simply wrong' },
  },
}

const recon = await agent(
  `${COMMON}

## Your job: establish the ground truth for three items. Do NOT edit any file. Quote what you read.

**(1) G1+G23 — the deferred contract.** \`cyrup-core/src/message.rs\` enumerates
Pending/Stop/Length/ToolUse/Error/Aborted with no \`Deferred\`, and its own doc says an unknown value
fails \`Deserialize\`. \`cyrup_session::manager::load\` then keeps only the valid PREFIX of a session
file, raises \`recovered\`, and declines to rewrite. So a pi-written session containing
\`"stopReason":"deferred"\` is silently TRUNCATED — user data loss on someone's real session file.
Read at v0.84.1: \`ai/src/types.ts\` (the \`StopReason\` union, \`DeferredHandle\`,
\`AssistantMessage.deferred\`, \`ProviderStreams.fetchDeferred/cancelDeferred\`,
\`SimpleStreamOptions.deferred\`) and \`agent/src/harness/session/context.ts\` (the context-exclusion
filter). Establish:
  - what \`deferred\` MEANS and when it is emitted;
  - exactly what the on-disk JSONL looks like for a deferred turn;
  - what the context-exclusion filter does with one, and why;
  - whether adding the enum variant ALONE makes a pi session round-trip, or whether other fields
    (\`deferred\` on the message?) also fail strict deserialization. Test this concretely — construct
    the JSON pi would write and try to decode it.

**(2) G12 — Responses terminal error with an empty message.** Claim: at v0.84.1
\`ai/src/api/openai-responses.ts:174\` throws \`"An unknown error occurred"\` unconditionally, while
cyrup's \`openai_responses.rs:996-1000\` can emit an error terminal with \`error_message: None\`. The
claim also says this was ALREADY true at v0.83.0, i.e. it is a port bug, not version lag. Verify
both halves by reading v0.83.0 AND v0.84.1. The equivalent correct pattern is said to exist already
at \`cyrup-provider/src/api/bedrock_converse_stream.rs:454-457\` — read it and confirm it is really
the same shape.

**(3) G43 — \`toJsonEvent\`.** Claim: cyrup's JSON and RPC modes both serialize the raw
\`AgentSessionEvent\` (\`cyrup-modes/src/json.rs:59-63\`, \`rpc.rs:1414\`), so \`message_update\` ships a
full message snapshot plus \`assistantMessageEvent.partial\` on EVERY delta — quadratic output where
pi is linear, and a different record shape for any consumer. Find pi's \`toJsonEvent\`
(\`coding-agent/src/modes/json-event.ts\` is claimed) and establish exactly what it projects, for
which event types, and what both cyrup serializers emit today. This is a CONTRACT surface: name
every field that would change shape for a downstream consumer.

For each, end with what you would actually change and roughly how big it is. If any claim above is
wrong, say so plainly — the plan came from a survey, not from the code.`,
  { label: 'recon:three-items', phase: 'Recon', schema: RECON_SCHEMA, effort: 'high' }
)

const reconDigest = recon
  ? `### (1) Deferred contract
${recon.deferred}

### (1b) Round-trip: what pi writes, what cyrup does today
${recon.deferred_round_trip || '(unresolved)'}

### (2) Responses terminal error
${recon.responses_error}

### (3) toJsonEvent projection
${recon.json_event}

### Scope warnings / where the plan is wrong
${recon.scope_warnings || '(none reported)'}`
  : '(recon returned nothing — establish these facts yourself before editing, and say so)'

log('Recon complete')

phase('Author')

const ITEMS = [
  {
    key: 'G1+G23',
    title: 'Deferred stop reason — a pi-written session must round-trip, not truncate',
    brief: `Use the recon. THE USER-VISIBLE HARM is data loss: \`cyrup_session::manager::load\` keeps
only the valid prefix of a session file and then declines to rewrite it, so a pi session containing
a deferred turn loses every entry after it.

Scope: the INTEROP slice. cyrup does not need to PRODUCE a deferred turn or implement
\`fetchDeferred\`/\`cancelDeferred\` — it needs to read, retain and re-emit one without loss, and the
context-exclusion filter must treat it as pi does. R-00-013 (the interop round-trip property) is the
bar: load a pi-shaped JSONL, re-export it, assert equivalence.

If the recon found that other fields ALSO fail strict deserialization, fix those too — a
round-trip that still truncates on the next field is not a fix.

There is existing parity machinery to use rather than reinvent: \`cyrup-test-support\`'s
\`interop.rs\` does exactly this load/re-export/compare, and \`golden.rs\` holds normalized JSONL
snapshots. Prefer extending those.`,
  },
  {
    key: 'G12',
    title: 'Responses terminal error must never carry an empty message',
    brief: `Use the recon's verified reading of v0.84.1 and v0.83.0. If it confirmed this is a PORT
BUG (present at the ported baseline, never ported) rather than version lag, say so in the doc
comment — that distinction determines priority for everything after it.

Apply the pattern that already exists at \`cyrup-provider/src/api/bedrock_converse_stream.rs:454-457\`.
Check whether Azure inherits this through the shared import at \`azure_openai_responses.rs:25\`; if it
does, your test should cover both, because a fix that silently misses a sibling API is half a fix.`,
  },
  {
    key: 'G43',
    title: 'JSON/RPC `message_update` must be delta-only',
    brief: `Use the recon's field-by-field account of \`toJsonEvent\`. Both \`cyrup-modes/src/json.rs\`
and \`rpc.rs\` currently serialize the raw event, so output is quadratic in message length where pi's
is linear.

This is a CONTRACT surface — anything consuming cyrup's \`--json\` or RPC stdout sees the shape
change. Two consequences you must handle rather than discover later:
  - Land the projection ONCE, ahead of both serializers, not copy-pasted into each. Two copies of a
    wire projection will drift.
  - Existing tests almost certainly pin the OLD shape. Update them to the pi-correct shape and say
    exactly which ones you changed and why — do not weaken an assertion to make it pass. If a test
    pins the old shape deliberately (a golden snapshot), regenerate it and note the regeneration.`,
  },
]

const authored = []
for (const [i, item] of ITEMS.entries()) {
  const prior = authored.length
    ? `\n\n## Already landed in this batch (do not redo, do not revert)\n\n${authored
        .map((a) => `- ${a.item}: ${a.status} — ${a.summary} (files: ${(a.files_changed || []).join(', ')})`)
        .join('\n')}`
    : ''
  const res = await agent(
    `${COMMON}

## Recon — established for this batch, use it rather than re-deriving

${reconDigest}

## Your item (${i + 1} of ${ITEMS.length}): ${item.key} — ${item.title}

${item.brief}

Work in ${WS}/cyrup. Edit, test, run the full revert proof (explicit backup, never \`git checkout\`),
run clippy, return the structured result.${prior}`,
    { label: `author:${item.key}`, phase: 'Author', schema: ITEM_SCHEMA, effort: 'high' }
  )
  if (res) authored.push(res)
}

log(`Authored ${authored.length}/${ITEMS.length}`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored'],
  properties: {
    tree_restored: { type: 'boolean' },
    round_trip_verdict: { type: 'string', description: 'Does a pi-shaped session with a deferred turn now round-trip with NO loss? Show the evidence.' },
    contract_diff: { type: 'string', description: 'The exact before/after JSON+RPC record shape, as emitted.' },
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

const digest = authored
  .map(
    (a) => `### ${a.item} [${a.status}] ${a.summary}
FILES: ${(a.files_changed || []).join(', ')}
TESTS: ${(a.tests_added || []).join(', ')}
UPSTREAM: ${a.upstream_citation || '(none)'}
CALLER: ${a.caller || '(none)'}
MIRROR: ${a.mirror_case || '(none)'}
PUBLIC SIGNATURE CHANGE: ${a.public_signature_change}
REVERT PROOF: ${(a.revert_proof || '(none)').slice(0, 1500)}
REGISTRATION NEEDED: ${a.registration_needed || '(none)'}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. You are the ONLY reviewer — run all five lenses
## yourself, in order. Do NOT fix anything; report.

**Lens 1 — does the session actually round-trip?** Construct a pi-shaped session JSONL containing a
deferred turn YOURSELF (from pi's v0.84.1 types, not from the author's test fixture), load it,
re-export it, and diff. Then extend it: put entries AFTER the deferred turn and confirm none are
lost, since silent truncation was the original harm. Check \`cyrup-test-support\`'s \`interop.rs\` was
used rather than a bespoke fixture that only proves its own shape.

**Lens 2 — the contract change.** For G43, capture the ACTUAL stdout of both \`--json\` and RPC mode
before and after, for a multi-delta message. Confirm output is now linear, not quadratic, and record
the exact field-level diff a downstream consumer would see. Then check the projection was landed
ONCE ahead of both serializers rather than duplicated — two copies of a wire projection will drift.

**Lens 3 — tests that were CHANGED.** G43 almost certainly required editing tests that pinned the
old shape. For each edited test, decide: was it updated to the pi-correct expectation, or was the
assertion WEAKENED to make a failure go away? Quote the before and after. This is the highest-risk
place for a silent regression in this batch.

**Lens 4 — revert proofs.** Re-run each yourself, with an explicit backup and \`cp\` restore; never
\`git checkout\` — the tree holds uncommitted work. A revert must remove EVERY guard the test claims
to cover; a partial revert proves only what it removed.

**Lens 5 — citations and port-bug classification.** Open every cited line at v0.84.1 and count. For
G12, independently verify the v0.83.0 claim: if it was already true at the ported baseline it is a
PORT BUG, and mis-labelling that as version lag distorts the whole backlog's priority.

## CRITICAL PROCESS RULE

You may temporarily edit source, but back it up first and restore with \`cp\`. Confirm the affected
crates' tests are green before returning, then set \`tree_restored: true\`. \`git status --porcelain\`
at start and end must match. If the tree changes under you, STOP and report a blocker.

${digest}`,
  { label: 'verify:all-lenses', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 3,
  items: authored.map((a) => ({
    item: a.item,
    status: a.status,
    caller: a.caller,
    files: a.files_changed,
    public_signature_change: a.public_signature_change,
    registration_needed: a.registration_needed,
  })),
  round_trip_verdict: review?.round_trip_verdict,
  contract_diff: review?.contract_diff,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
