export const meta = {
  name: 'cyrup-parity-batch-02',
  description: 'Batch 2 — intercom wire survivability: tolerant frame decoding, metadata preservation, live-broker claim',
  phases: [
    { title: 'Recon', detail: 'map the v0.9.2 protocol surface and every strict-decode site' },
    { title: 'Author', detail: 'sequential: both items touch broker/mod.rs' },
    { title: 'Verify', detail: 'ONE reviewer, all lenses — parallel reviewers corrupt the shared tree' },
  ],
}

const WS = '/home/d0m17bw/workspace'
const CRATE = 'crates/cyrup-intercom'

const COMMON = `
You are working on \`cyrup\` (${WS}/cyrup), a Rust port targeting BEHAVIOURAL EQUIVALENCE with
\`pi-intercom\` (${WS}/pi-intercom). Ported baseline **v0.7.0**; target **v0.9.2** (latest).

CAUTION on the baseline: \`cyrup-intercom/src/lib.rs:2\` SAYS v0.6.0 and that is WRONG — the health
probe, \`INTERCOM_PROTOCOL_NAME\`, rate limiting, ask edges, trust controls and the ask-timeout env
var are all present in cyrup and all absent at v0.6.0. Diff from **v0.7.0** or you will "find" a
pile of already-done work.

Upstream has NO \`src/\` — entrypoint \`index.ts\` at the repo root, sources split across the root plus
\`broker/\`, \`ui/\`, \`skills/\`. Map: transport/framing.rs<-broker/framing.ts,
transport/client.rs<-broker/client.ts, transport/spawn.rs<-broker/spawn.ts,
transport/protocol.rs<-types.ts, broker/<-broker/broker.ts, paths.rs<-broker/paths.ts,
reply_tracker.rs<-reply-tracker.ts, config.rs<-config.ts, ui/<-ui/. \`index.ts\` is cited 118 times
and fans out across inbound.rs, identity.rs, extension.rs, tools/ and session_state.rs.

## WHY THIS BATCH EXISTS

cyrup and pi sessions can share a broker socket, so this is a LIVE INTEROP surface, not an internal
detail. Today an unknown \`ClientMessage\`/\`BrokerMessage\` tag is FATAL: it sets \`close_reason\` and
\`break 'outer\` (\`${CRATE}/src/transport/client.rs:505-517\`) or returns \`FrameOutcome::ProtocolError\`
which ends the connection loop (\`broker/mod.rs:288-295,695\`). A pi >=0.9.0 client sends
\`message_receipt\` on its FIRST inbound message and a pi >=0.9.0 broker sends \`message_control\` on
any peer cancel — so the very first interop message disconnects. Unknown FIELDS are already fine
(serde default); it is unknown TAGS that kill the connection.

## HARD RULES

- \`spec/\` and \`ADR-0001\` do not exist here. Neither is ever authority for leaving a difference.
- **Check CONSUMERS, not just primitives.** Batch 1 shipped ~800 lines of correct code that nothing
  could reach, because the analysis listed a primitive and never asked whether its callers were
  ported. Before you call anything done, name the non-test caller.
- **Rust is not TypeScript.** Where a mechanism must differ (serde vs structural typing), port the
  BEHAVIOUR and state the mechanism difference in the doc comment with its reason.
- **Verify every citation you write** against the right tag:
  \`git -C ${WS}/pi-intercom show v0.9.2:<file>\`. Batch 1 shipped three doc comments citing
  baseline-era line numbers that land on entirely different functions at the target tag. Prefer
  VERSION-QUALIFIED citations — \`v0.9.2 broker/broker.ts:231\` — because a bare citation is correct
  at one tag and wrong at another, and this crate now straddles two.
- **No-panic policy**: \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` are DENIED by clippy
  in non-test code, and fire ONLY under \`cargo clippy\`. Test files opt out with a file-level
  \`#![allow(...)]\`; \`tests/\` files are separate crates inheriting nothing.
- Never write a large artifact to \`std::env::temp_dir()\` by hand — use \`tempfile\`.

## BUILD DISCIPLINE

ONE shared \`target/\`, ONE \`.cargo-lock\`; concurrent cargo invocations serialize.
- MAY run: \`cargo check/test/clippy -p cyrup-intercom --all-targets\`.
- MUST NOT run any \`--workspace\` command; the orchestrator gates the batch once.
- Finish with \`cargo clippy -p cyrup-intercom --all-targets; echo "exit=$?"\` and check the EXIT
  CODE, not grep output. Expected steady state is ZERO warnings.

## DEFINITION OF DONE

1. Behaviour matches v0.9.2, with a VERIFIED version-qualified upstream citation.
2. A test that FAILS without your change — actually run the revert, PASTE the failure output,
   restore. If a "removed" feature's suite stays green, your revert did not revert anything.
3. A MIRROR case that stays green, proving the change is not over-broad. For tolerant decoding the
   mirror is essential: a KNOWN-BAD frame (malformed, wrong type for a known tag) must STILL be
   rejected. "Tolerant" must not become "accepts anything".
4. A NAMED non-test caller for anything you call wired.
5. Clippy exits 0 with no new warnings.
6. Edit ONLY files under \`${CRATE}\`. Report cross-crate needs in \`registration_needed\`.
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
  required: ['client_tags', 'broker_tags', 'strict_sites', 'message_fields'],
  properties: {
    client_tags: { type: 'array', items: { type: 'string' }, description: 'Every ClientMessage tag at v0.9.2' },
    broker_tags: { type: 'array', items: { type: 'string' }, description: 'Every BrokerMessage tag at v0.9.2' },
    cyrup_client_tags: { type: 'array', items: { type: 'string' } },
    cyrup_broker_tags: { type: 'array', items: { type: 'string' } },
    strict_sites: { type: 'array', items: { type: 'string' }, description: 'file:line of every place an unknown tag is fatal' },
    message_fields: { type: 'array', items: { type: 'string' }, description: 'Every field on the v0.9.2 Message type, and which cyrup keeps' },
    forward_path: { type: 'string', description: 'How the broker re-forwards a message, and exactly where fields are lost' },
    upstream_tolerance: { type: 'string', description: 'What upstream actually DOES with a tag it does not know — ignore, log, close? Cite it.' },
    notes: { type: 'string' },
  },
}

const recon = await agent(
  `${COMMON}

## Your job: map the protocol surface. Do NOT edit any file.

1. Enumerate EVERY \`ClientMessage\` and \`BrokerMessage\` tag at v0.9.2 (\`types.ts:78-131\` is the
   claimed location — verify it) and the corresponding sets cyrup knows. The delta is what currently
   kills a connection.
2. Find EVERY site in \`${CRATE}\` where an unknown tag is fatal. The two known ones are
   \`transport/client.rs:505-517\` and \`broker/mod.rs:288-295,695\`; there may be more. Give file:line.
3. **Establish what upstream actually does with an unrecognised tag.** This is the crux: does it
   ignore it, log it, or close? Cite the code. Do NOT assume "tolerant" — if pi also closes, then
   the fix is to ADD the missing tags rather than to loosen decoding, and this batch changes shape.
4. List every field on the v0.9.2 \`Message\` type, and which of them cyrup's 5-field
   \`transport/protocol.rs:51-64\` keeps. Trace the broker's re-forward path (\`broker/mod.rs:427\` is
   claimed to re-parse into the typed struct and drop the rest on re-serialize) and say exactly
   where data is lost.

Be concrete and cite file:line on both sides. If the claims in this prompt are wrong, say so — they
came from a survey, not from the code.`,
  { label: 'recon:protocol-surface', phase: 'Recon', schema: RECON_SCHEMA, effort: 'high' }
)

const reconDigest = recon
  ? `### v0.9.2 client tags (${(recon.client_tags || []).length})
${(recon.client_tags || []).join(', ')}

### v0.9.2 broker tags (${(recon.broker_tags || []).length})
${(recon.broker_tags || []).join(', ')}

### cyrup knows — client (${(recon.cyrup_client_tags || []).length}): ${(recon.cyrup_client_tags || []).join(', ')}
### cyrup knows — broker (${(recon.cyrup_broker_tags || []).length}): ${(recon.cyrup_broker_tags || []).join(', ')}

### Sites where an unknown tag is fatal
${(recon.strict_sites || []).join('\n')}

### WHAT UPSTREAM DOES WITH AN UNKNOWN TAG
${recon.upstream_tolerance || '(unresolved)'}

### Message fields
${(recon.message_fields || []).join('\n')}

### Re-forward path / where fields are lost
${recon.forward_path || '(unresolved)'}

### Notes
${recon.notes || '(none)'}`
  : '(recon returned nothing — establish these facts yourself before editing, and say so)'

log(`Recon: ${(recon?.client_tags || []).length} client / ${(recon?.broker_tags || []).length} broker tags upstream; ${(recon?.strict_sites || []).length} strict sites`)

phase('Author')

const ITEMS = [
  {
    key: 'G144',
    title: 'Broker must refuse to replace a LIVE broker instead of unlinking its socket',
    brief: `\`${CRATE}/src/broker/mod.rs:835-838\` unlinks an existing socket unconditionally before
binding. Upstream claims a runtime lock first (\`v0.9.2 broker/runtime-claim.ts:3-21\`, used at
\`broker/broker.ts:231\`): a second broker must DETECT a live incumbent and decline, rather than
stealing the socket out from under it and silently orphaning every session already connected.

Port the liveness check. Think about the failure you must NOT introduce: a STALE socket — left by a
crashed broker — must still be reclaimable, or the whole intercom deadlocks until someone deletes a
file by hand. So you need both tests:
  - a live incumbent is refused (the socket keeps working for its existing clients), and
  - a stale socket IS reclaimed.
This item goes first because it is the smaller change to \`broker/mod.rs\`, which G136 also touches.`,
  },
  {
    key: 'G136',
    title: 'Unknown protocol frames must not tear down the connection; preserve unknown Message fields',
    brief: `Use the recon above — especially its finding on what upstream ACTUALLY does with an
unknown tag. Implement whatever that is, not what this brief assumes.

Two halves:

(a) TAG TOLERANCE at every strict site the recon found (at minimum
\`transport/client.rs:505-517\` and \`broker/mod.rs:288-295,695\`). A pi >=0.9.0 client sends
\`message_receipt\` on its first inbound message and a pi >=0.9.0 broker sends \`message_control\` on
any peer cancel; today either one disconnects cyrup immediately.

  CRITICAL: tolerant must not mean credulous. A frame that is genuinely MALFORMED, or that carries a
  KNOWN tag with a wrong-typed payload, must still be rejected exactly as today. Prove both
  directions — an unknown tag survives, a corrupt frame still closes. Getting this wrong turns a
  compatibility fix into an input-validation hole on a socket other sessions can reach.

(b) FIELD PRESERVATION through the broker's re-forward. \`broker/mod.rs:427\` re-parses into the typed
5-field \`Message\` and forwards THAT, so \`senderSequence\`/\`supersedes\`/\`retryOf\`/\`broker*At\` are
silently dropped on re-serialize. A cyrup broker sitting between two pi sessions therefore corrupts
their conversation. Preserve unknown fields across the round trip (serde's
\`#[serde(flatten)]\` capture is the usual Rust answer) and test it with a field cyrup does not model.

Do NOT implement the semantics of the new message types in this batch — G139/G140 own those, and
they are scheduled for batch 12. This batch is survivability only: cyrup must not disconnect, and
must not corrupt what it relays.`,
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

## Protocol recon — established for this batch, use it

${reconDigest}

## Your item (${i + 1} of ${ITEMS.length}): ${item.key} — ${item.title}

${item.brief}

Work in ${WS}/cyrup. Edit, test, run the revert proof, run clippy, return the structured
result.${prior}`,
    { label: `author:${item.key}`, phase: 'Author', schema: ITEM_SCHEMA, effort: 'high' }
  )
  if (res) authored.push(res)
}

log(`Authored ${authored.length}/${ITEMS.length} items`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored'],
  properties: {
    tree_restored: { type: 'boolean' },
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
REVERT PROOF: ${(a.revert_proof || '(none)').slice(0, 1500)}
REGISTRATION NEEDED: ${a.registration_needed || '(none)'}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. You are the ONLY reviewer — run all four lenses
## yourself, in order. Do NOT fix anything; report.

**Lens 1 — did tolerance become credulity?** THE risk of this batch. Verify that a malformed frame,
and a known tag with a wrong-typed payload, are STILL rejected. Write the hostile frames yourself
and check. This socket is reachable by other sessions on the box; a decoder that accepts anything is
worse than one that disconnects.

**Lens 2 — reachability.** For each item, grep the whole workspace for callers and confirm at least
one is outside \`#[cfg(test)]\`/\`tests/\`. Batch 1 shipped ~800 lines reachable only from tests. Trace
G144's liveness check to the real bind path and G136's tolerance to the real connection loop.

**Lens 3 — revert proofs.** Re-run each yourself. Fake if the test passes with the change removed,
if the "revert" renamed a definition along with its call sites, if the assertion is true by
construction, or if the test hits a skip branch. Check G144 BOTH ways: a live incumbent refused AND
a stale socket still reclaimable — a fix that deadlocks on a stale socket is worse than the bug.

**Lens 4 — citations and scope.** Open every cited upstream line
(\`git -C ${WS}/pi-intercom show v0.9.2:<file>\`) and confirm it says what is claimed; flag any bare
citation that is ambiguous between v0.7.0 and v0.9.2. Then confirm this batch did NOT implement
G139/G140 semantics — survivability only. Scope creep here lands untested protocol behaviour.

## CRITICAL PROCESS RULE

You may temporarily edit source to run a revert, but you MUST restore it and confirm
\`cargo test -p cyrup-intercom\` is green before returning, then set \`tree_restored: true\`. Run
\`git status --porcelain\` at the start and again at the end and confirm the file list matches.
Nothing else is touching this tree; if you see it change under you, STOP and report a blocker.

${digest}`,
  { label: 'verify:all-lenses', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: 2,
  recon: {
    client_tags: recon?.client_tags,
    broker_tags: recon?.broker_tags,
    strict_sites: recon?.strict_sites,
    upstream_tolerance: recon?.upstream_tolerance,
  },
  items: authored.map((a) => ({
    item: a.item,
    status: a.status,
    caller: a.caller,
    files: a.files_changed,
    registration_needed: a.registration_needed,
    public_signature_change: a.public_signature_change,
  })),
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  overall: review?.overall,
}
