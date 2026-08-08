export const meta = {
  name: 'cyrup-parity-batch-02-fix',
  description: 'Batch 2 remediation — close the three input-validation holes tolerant decoding opened',
  phases: [
    { title: 'Fix', detail: 'sequential: all three touch protocol.rs / broker/mod.rs' },
    { title: 'Verify', detail: 'ONE reviewer; live probes against a real broker, not unit tests alone' },
  ],
}

const WS = '/home/d0m17bw/workspace'
const CRATE = 'crates/cyrup-intercom'

const COMMON = `
You are working on \`cyrup\` (${WS}/cyrup), porting \`pi-intercom\` (${WS}/pi-intercom). Ported
baseline **v0.7.0**; target **v0.9.2**. (\`lib.rs:2\` says v0.6.0 and is WRONG — the code is at v0.7.0.)

## WHAT JUST HAPPENED, AND WHY THIS REMEDIATION EXISTS

Batch 2 added the v0.9.2 message tags cyrup was missing. A recon phase established something the
plan had backwards: **upstream is NOT tolerant of unknown tags** — it destroys the socket on both
sides (v0.9.2 \`broker/broker.ts:971-972\`, \`broker/client.ts:599-600\`, both reaching
\`socket.destroy\` through \`framing.ts:44-51\`). cyrup's strict decoding was a CORRECT port; the bug
was only that its known-tag set was frozen at v0.7.0.

Adding those tags opened three input-validation holes, all confirmed by live probes against a real
broker subprocess. **This socket is reachable by any process on the box**, so a decoder that accepts
what pi rejects is worse than the disconnect bug we set out to fix. Your job is to close them
WITHOUT reintroducing the disconnects.

## HARD RULES

- \`spec/\` and \`ADR-0001\` do not exist here; neither is authority for anything.
- **Match pi's acceptance set exactly.** Not looser (an input-validation hole on a shared socket),
  not stricter (a disconnect where pi would proceed). When you cannot have both, say so explicitly
  rather than silently choosing.
- **Verify every citation** with \`git -C ${WS}/pi-intercom show v0.9.2:<file>\` and COUNT the lines.
  The last pass shipped a systematic off-by-2 band in \`types.ts\` citations. Write
  VERSION-QUALIFIED citations (\`v0.9.2 broker/broker.ts:151-155\`) — this crate straddles two tags
  and a bare citation is correct at one and wrong at the other.
- **No-panic policy**: \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` DENIED by clippy in
  non-test code; fires ONLY under \`cargo clippy\`. Test files opt out with file-level \`#![allow]\`;
  \`tests/\` are separate crates inheriting nothing.
- Never hand-write large artifacts to \`std::env::temp_dir()\` — use \`tempfile\`.

## BUILD DISCIPLINE

ONE shared \`target/\`, ONE \`.cargo-lock\`; concurrent cargo runs serialize.
- MAY run \`cargo check/test/clippy -p cyrup-intercom --all-targets\`.
- MUST NOT run any \`--workspace\` command; the orchestrator gates once.
- Finish with \`cargo clippy -p cyrup-intercom --all-targets; echo "exit=$?"\`; check the EXIT CODE.

## DEFINITION OF DONE

1. Behaviour matches v0.9.2 with a verified, version-qualified citation.
2. A test that FAILS without your change — run the revert, PASTE the failure, restore.
3. **A LIVE PROBE, not only a unit test.** Every one of these holes was found by driving a real
   broker subprocess; two of them passed the existing unit-level mirrors. Your proof must send the
   hostile frame over a real socket and assert what the broker does (survives / destroys / answers).
4. A named non-test caller for anything you call wired.
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
    live_probe: { type: 'string', description: 'The hostile frame sent over a REAL socket and what the broker did' },
    mirror_case: { type: 'string' },
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
    key: 'R1',
    title: 'Array-shaped payloads are accepted where pi rejects them',
    brief: `serde's derived \`Deserialize\` for a plain struct implements \`visit_seq\`, so a JSON ARRAY
deserializes positionally into \`MessageReceipt\` / \`MessageControl\` / \`SessionInfo\` /
\`ExtensionCapability\`. pi's type guards open with an explicit \`Array.isArray(value)\` bail
(v0.9.2 \`broker/client.ts:57\` for \`isMessageReceipt\`), so pi destroys the socket on exactly these
frames while cyrup accepts them.

Confirmed live: after register, \`{"type":"message_receipt","receipt":["m1","queued",1,null]}\`
left the connection alive.

Make these types MAP-ONLY. Note the trap: \`Message\` and \`Attachment\` are immune **only by
accident** — their \`#[serde(flatten)] extra\` capture happens to disable \`visit_seq\`. Do not leave a
security property resting on an accident. Give every one of these types an EXPLICIT map-only
guarantee (a \`flatten\` capture, which also matches pi's object-spread tolerance for unknown keys, or
a \`deserialize_with\` that rejects a sequence) and add a doc line saying the guarantee is deliberate,
so a later refactor that drops \`flatten\` cannot silently reopen this.

Your live probe must send the array-shaped frame for EACH affected type.`,
  },
  {
    key: 'R2',
    title: 'Explicit JSON `null` on optional Message fields is accepted and silently dropped',
    brief: `pi's \`isMessage()\` guard is \`message[key] !== undefined && typeof message[key] !== "number"\`
(v0.9.2 \`broker/broker.ts:151-155\`). Since \`typeof null === "object"\`, pi REJECTS an explicit null
on the nine optional fields; cyrup ACCEPTS it and erases the key. Same for \`content.attachments\`
(\`:182-183\`).

This breaks two claims the batch made at once: that cyrup now matches pi's reject behaviour, and
that the relay is lossless — the null keys vanish from the delivered envelope. Confirmed live:
alpha sent \`senderSequence:null, replyTo:null\`, broker answered \`delivered\`, and beta received an
envelope with both keys gone. pi would have answered \`delivery_failed\` / "Invalid message format".

Distinguish ABSENT from NULL, matching \`undefined\` vs \`null\` in the guard. Then fix the mirror:
\`message_still_rejects_wrong_typed_known_fields\` (\`protocol.rs:670\`) sweeps all nine fields with
wrong-typed values but never with \`null\` — as written it CERTIFIES a guarantee the code does not
hold, which is worse than no test. Add \`null\` to the sweep.

If you conclude the looser read is actually correct, then DELETE the lossless-relay and
matches-pi claims from the doc comments instead — but say which you chose and why.`,
  },
  {
    key: 'R3',
    title: 'Extension-bus client tags accepted with zero validation and no answer',
    brief: `\`extension_capabilities_update\`, \`extension_publish\` and \`extension_state_commit\` are
accepted at \`${CRATE}/src/broker/mod.rs:309-315\` with NO payload validation and NO response.
Upstream is neither silent nor tolerant:
  - non-array \`extensions\` THROWS -> socket.destroy (v0.9.2 \`broker/broker.ts:560-562\`)
  - \`extension_publish\` from a session that never advertised -> \`error\`
    \`{"Session has not advertised extension capability"}\` (\`:1277-1280\`)
  - \`extension_state_commit\` -> always an \`extension_state_result{committed:false, reason:...}\`
    (\`:1379-1388\`)

Confirmed live: \`{"extensions":"not-an-array","type":"extension_capabilities_update"}\`,
\`{"type":"extension_publish"}\` and \`{"namespace":42,"type":"extension_state_commit"}\` each left the
connection alive with no answer at all.

Port upstream's MISS branches. None of them needs the extension bus itself — they are the
not-advertised / malformed paths. This also resolves a contradiction the batch left in place: it
answered \`cancel_message\` on the reasoning that "a silent drop would hang the caller", then
silently dropped \`extension_state_commit\`, whose commit promise hangs in exactly the same way.

Do NOT implement the extension bus (that is later-batch work). Only the miss branches.
Your live probe must send each malformed frame and assert the broker's answer.`,
  },
]

const fixed = []
for (const [i, item] of ITEMS.entries()) {
  const prior = fixed.length
    ? `\n\n## Already landed in this remediation (do not redo, do not revert)\n\n${fixed
        .map((a) => `- ${a.item}: ${a.status} — ${a.summary}`)
        .join('\n')}`
    : ''
  const res = await agent(
    `${COMMON}

## Your item (${i + 1} of ${ITEMS.length}): ${item.key} — ${item.title}

${item.brief}

Work in ${WS}/cyrup. Edit, test, run the revert proof AND a live probe, run clippy, return the
structured result.${prior}`,
    { label: `fix:${item.key}`, phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
  )
  if (res) fixed.push(res)
}

log(`Fixed ${fixed.length}/${ITEMS.length}`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored'],
  properties: {
    tree_restored: { type: 'boolean' },
    acceptance_parity: { type: 'string', description: 'Your own verdict: does cyrup now accept exactly what pi accepts? Name any remaining delta.' },
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
REVERT PROOF: ${(a.revert_proof || '(none)').slice(0, 900)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this remediation. You are the ONLY reviewer. Do NOT fix; report.

**Lens 1 — acceptance parity, by live probe.** Build a table: for each hostile frame shape (array
payload per type, explicit null per optional field, malformed extension frames, plus any shape YOU
think of that the fixers did not), what does pi do and what does cyrup do? Drive a REAL broker
subprocess — two of the three holes passed unit-level mirrors and were only caught this way. Report
every remaining divergence in either direction: cyrup looser than pi is a validation hole, cyrup
stricter is a disconnect pi would not have.

**Lens 2 — did the fix reintroduce disconnects?** The original bug was cyrup tearing down the
connection on tags a modern pi peer legitimately sends. Confirm the well-formed v0.9.2 frames a real
pi peer sends (\`message_receipt\`, \`message_control\`, and the additive \`features\` on \`registered\`)
STILL survive. A fix that closes the holes by rejecting more than pi has re-broken the batch.

**Lens 3 — revert proofs and test honesty.** Re-run each revert yourself. Check specifically that
the R2 mirror now actually sweeps \`null\` — before this remediation it swept nine fields with
wrong-typed values and certified a guarantee the code did not hold, which is the failure mode to
hunt for elsewhere too: a test whose NAME claims more than its BODY checks.

**Lens 4 — citations.** Open every cited line at v0.9.2 and count. The previous pass had a
systematic off-by-2 band in \`types.ts\`. Flag any bare citation ambiguous between v0.7.0 and v0.9.2.

## CRITICAL PROCESS RULE

You may temporarily edit source to run a revert, but you MUST restore it, confirm
\`cargo test -p cyrup-intercom\` is green, and set \`tree_restored: true\`. Run \`git status --porcelain\`
at start and end and confirm the file list matches. Nothing else is touching this tree; if it
changes under you, STOP and report a blocker.

${digest}`,
  { label: 'verify:all-lenses', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: '2-fix',
  items: fixed.map((a) => ({ item: a.item, status: a.status, files: a.files_changed, caller: a.caller })),
  acceptance_parity: review?.acceptance_parity,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
