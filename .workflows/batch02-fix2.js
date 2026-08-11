export const meta = {
  name: 'cyrup-parity-batch-02-fix2',
  description: 'Batch 2 round 2 — unvalidated context fields (blocker) and the JS-number domain mismatch (DoS)',
  phases: [
    { title: 'Fix', detail: 'sequential: both touch transport/protocol.rs' },
    { title: 'Verify', detail: 'ONE reviewer; rebuild the full acceptance-parity table' },
  ],
}

const WS = '/home/d0m17bw/workspace'
const CRATE = 'crates/cyrup-intercom'

const COMMON = `
You are working on \`cyrup\` (${WS}/cyrup), porting \`pi-intercom\` (${WS}/pi-intercom). Ported
baseline **v0.7.0**; target **v0.9.2**. (\`lib.rs:2\` says v0.6.0 and is WRONG — the code is at v0.7.0.)

## CONTEXT

Batch 2 added the v0.9.2 tags cyrup lacked. Round 1 of remediation closed three input-validation
holes that addition opened. A live acceptance-parity audit then found ELEVEN remaining divergences
between what pi accepts and what cyrup accepts. This round closes the two worst classes.

**The intercom socket is reachable by any process on the box, and a cyrup session and a pi session
can share a broker.** So divergence is dangerous in BOTH directions:
- cyrup LOOSER than pi = an input-validation hole.
- cyrup STRICTER than pi = a disconnect pi would never have had, i.e. a denial of service — and one
  peer's hostile frame can take down OTHER clients.

## HARD RULES

- \`spec/\` and \`ADR-0001\` do not exist here; neither is authority for anything.
- **Match pi's acceptance set exactly** — not looser, not stricter. When you genuinely cannot have
  both, say which you chose and why, in the doc comment and in your report.
- **Verify every citation** with \`git -C ${WS}/pi-intercom show v0.9.2:<file>\` and COUNT lines.
  Use VERSION-QUALIFIED citations (\`v0.9.2 broker/client.ts:182-186\`).
- **No-panic policy**: \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` DENIED by clippy in
  non-test code; fires ONLY under \`cargo clippy\`. \`tests/\` files are separate crates that inherit
  no lint config — they need their own file-level \`#![allow(...)]\`.
- Never hand-write large artifacts to \`std::env::temp_dir()\` — use \`tempfile\`.

## BUILD DISCIPLINE

ONE shared \`target/\`, ONE \`.cargo-lock\`; concurrent cargo runs serialize.
- MAY run \`cargo check/test/clippy -p cyrup-intercom --all-targets\`. MUST NOT run \`--workspace\`.
- Finish with \`cargo clippy -p cyrup-intercom --all-targets; echo "exit=$?"\`; check the EXIT CODE.

## DEFINITION OF DONE

1. Behaviour matches v0.9.2, verified version-qualified citation.
2. A test that FAILS without your change — run the revert, PASTE the failure, restore.
3. **A LIVE PROBE over a real socket.** Every hole in this batch was found that way; two of three
   passed unit-level mirrors first. Unit tests over the type are not evidence for a wire protocol.
4. A POSITIVE CONTROL: the well-formed frames a real pi peer sends must still work. A fix that
   restores parity by rejecting more has re-broken the batch.
5. Clippy exit 0, no new warnings.
6. Edit ONLY files under \`${CRATE}\`; report cross-crate needs in \`registration_needed\`.
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
    live_probe: { type: 'string' },
    positive_control: { type: 'string' },
    caller: { type: 'string' },
    public_signature_change: { type: 'boolean' },
    registration_needed: { type: 'string' },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Fix')

const ITEMS = [
  {
    key: 'R4',
    title: 'BLOCKER — SessionInfo / presence context fields are never validated',
    brief: `\`isSessionInfo\` guards SEVEN optional fields, not the four R2 handled. v0.9.2
\`broker/client.ts:182-186\` is a loop over \`["contextPct","contextTokens","contextWindow"]\`:
\`if (session[key] !== undefined && typeof session[key] !== "number") return false;\`

cyrup models NONE of the three, so they fall into the \`#[serde(flatten)] extra\` capture as raw
\`Value\` and ANY type — string, object, explicit null — is accepted where pi returns false and
\`framing.ts:44-51\` destroys the socket. This is the SAME hole class round 1 was convened to close,
in the same struct and the same handler it edited, and it sits behind a test whose name already
certifies coverage. Reachable at four broker tags: \`session_joined\`, \`presence_update\`,
\`sessions[]\`, and \`message.from\`.

The same applies to the \`presence\` CLIENT tag, which has its own throw ladder at v0.9.2
\`broker/broker.ts:918-950\` — and note its null-clears semantics: for presence, an explicit
\`null\` on a context field means CLEAR, which is NOT the same rule as \`isSessionInfo\`'s. Read both
and port each faithfully; do not assume one rule covers both.

Model the three fields properly rather than leaving them in \`extra\`, and make the guard explicit so
a future refactor cannot silently drop it.`,
  },
  {
    key: 'R5',
    title: 'JS-number domain mismatch — cyrup disconnects where pi proceeds (DoS)',
    brief: `cyrup types several wire fields as Rust integers (\`u32\`/\`u64\`) where pi only checks
\`typeof x === "number"\`, i.e. an IEEE-754 double. So values pi accepts destroy a cyrup connection:

  - \`SessionRegistration.pid\` / \`startedAt\` / \`lastActivity\` — \`-1\`, \`1.5\`, \`2^32\` all destroy the
    connection where pi registers the session normally.
  - \`SessionInfo.pid\` / \`peerUid\` — same, client-side. **THIS IS THE AMPLIFYING ONE**: one hostile
    \`register\` on a shared pi broker disconnects EVERY cyrup client attached to it, because they all
    decode that \`SessionInfo\` when it is relayed to them.
  - \`Message.timestamp\` and \`MessageReceipt.timestamp\` — fractional values destroy / \`delivery_failed\`
    where pi proceeds. Batch 2 newly EXTENDED this defect to \`MessageReceipt\`.

Additionally, \`send\` with a numerically-out-of-domain message answers
\`delivery_failed{messageId:"unknown"}\` where pi answers with the real \`message.id\`, so the pi peer's
\`pendingSends\` entry never resolves — a hang, not just a rejection.

Port the ACCEPTANCE DOMAIN, which is "any JSON number". Preserve the value faithfully on relay
(round 1 established the relay must be lossless). Where cyrup's own logic genuinely needs an
integer — a pid it will signal, a timestamp it will render — convert at the POINT OF USE and handle
the out-of-domain case there, rather than rejecting at the wire boundary where pi does not.

Think about what \`-1\` as a pid means for any code that acts on it; the fix must not turn a decode
rejection into a bad syscall. State in your report exactly which consumers you audited.`,
  },
]

const fixed = []
for (const [i, item] of ITEMS.entries()) {
  const prior = fixed.length
    ? `\n\n## Already landed in this round (do not redo, do not revert)\n\n${fixed
        .map((a) => `- ${a.item}: ${a.status} — ${a.summary}`)
        .join('\n')}`
    : ''
  const res = await agent(
    `${COMMON}

## Your item (${i + 1} of ${ITEMS.length}): ${item.key} — ${item.title}

${item.brief}

Work in ${WS}/cyrup. Edit, test, run the revert proof AND a live probe AND a positive control, run
clippy, return the structured result.${prior}`,
    { label: `fix:${item.key}`, phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
  )
  if (res) fixed.push(res)
}

log(`Fixed ${fixed.length}/${ITEMS.length}`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'parity_table'],
  properties: {
    tree_restored: { type: 'boolean' },
    parity_table: {
      type: 'array',
      description: 'One row per probed frame shape. Rebuild it fully; do not carry forward round-1 rows on trust.',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['shape', 'pi', 'cyrup', 'verdict'],
        properties: {
          shape: { type: 'string' },
          pi: { type: 'string' },
          cyrup: { type: 'string' },
          verdict: { type: 'string', enum: ['match', 'cyrup-looser', 'cyrup-stricter'] },
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

const digest = fixed
  .map(
    (a) => `### ${a.item} [${a.status}] ${a.summary}
FILES: ${(a.files_changed || []).join(', ')}
TESTS: ${(a.tests_added || []).join(', ')}
UPSTREAM: ${a.upstream_citation || '(none)'}
LIVE PROBE: ${(a.live_probe || '(none)').slice(0, 900)}
POSITIVE CONTROL: ${(a.positive_control || '(none)').slice(0, 600)}
REVERT PROOF: ${(a.revert_proof || '(none)').slice(0, 900)}
NOTES: ${(a.notes || '').slice(0, 600)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: rebuild the acceptance-parity table from scratch and review. Do NOT fix; report.

**Lens 1 — the parity table.** Drive a REAL broker subprocess and probe every shape you can think
of, per field and per tag. Do NOT carry forward the previous round's rows on trust; re-probe them.
Fill \`parity_table\` with one row per shape: what pi does, what cyrup does, and the verdict.
Include at minimum: array-shaped payloads per type; explicit null per optional field; the three
context fields on both \`SessionInfo\` and the \`presence\` tag (their rules DIFFER — presence-null
means CLEAR); negative / fractional / >2^32 / >MAX_SAFE_INTEGER numbers on every numeric field;
malformed extension-bus frames.

**Lens 2 — did R5 create a worse bug than it fixed?** It widened numeric acceptance so cyrup stops
disconnecting. Now audit every CONSUMER of those values: a \`pid\` of \`-1\` or \`1.5\` that reaches a
kill/signal path, a timestamp that reaches a duration or a renderer, a \`peerUid\` that reaches a
trust check. Accepting a value pi accepts is correct; ACTING on it unsafely is a new bug. Name every
consumer you checked and what it does with an out-of-domain value.

**Lens 3 — positive controls.** Confirm the well-formed frames a real pi peer sends still work end
to end: \`register\`, \`send\`, \`message_receipt\`, \`message_control\`, \`presence\` with real context
values, and the additive \`features\` on \`registered\`. A round that restores parity by rejecting more
has re-broken the batch.

**Lens 4 — revert proofs and test honesty.** Re-run each revert. Hunt specifically for tests whose
NAME claims more than their BODY checks — round 1 found one that swept nine fields with wrong-typed
values while certifying null-coverage it did not have.

## CRITICAL PROCESS RULE

You may temporarily edit source to run a revert, but you MUST restore it, confirm
\`cargo test -p cyrup-intercom\` is green, and set \`tree_restored: true\`. \`git status --porcelain\`
at start and end must match. If the tree changes under you, STOP and report a blocker.

${digest}`,
  { label: 'verify:parity-table', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
const table = review?.parity_table || []
return {
  batch: '2-fix2',
  items: fixed.map((a) => ({ item: a.item, status: a.status, files: a.files_changed, public_signature_change: a.public_signature_change })),
  parity_rows: table.length,
  looser: table.filter((r) => r.verdict === 'cyrup-looser'),
  stricter: table.filter((r) => r.verdict === 'cyrup-stricter'),
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
