export const meta = {
  name: 'cyrup-parity-batch-01',
  description: 'Batch 1 — permission gate: security, audit, policy surface (G129-G133, G135)',
  phases: [
    { title: 'Resolve', detail: 'answer the three open questions the items depend on' },
    { title: 'Author', detail: 'sequential: all items live in one crate, two share extension.rs' },
    { title: 'Verify', detail: 'adversarial review of the applied diff' },
  ],
}

const WS = '/home/d0m17bw/workspace'
const CRATE = 'crates/cyrup-permission-system'

const COMMON = `
You are working on \`cyrup\` (${WS}/cyrup), a Rust port whose goal is BEHAVIOURAL EQUIVALENCE with
four read-only TypeScript upstreams at ${WS}/{pi,pi-subagents,pi-permission-system,pi-intercom}.

This batch is entirely inside \`cyrup-permission-system\`, which ports **pi-permission-system**.
Its ported baseline is **v0.7.1**; the target is **v0.8.0** (latest). pi CORE has no permission
system at all — never cite \`pi/\` for permission behaviour, cite \`pi-permission-system/\`.

Module map: manager.rs<-permission-manager.ts, evaluate.rs<-evaluate-permission.ts,
wildcard.rs<-wildcard-matcher.ts, ask.rs<-permission-dialog.ts, ext_config.rs<-extension-config.ts,
jsonc.rs<-jsonc-config.ts, forwarding.rs<-permission-forwarding.ts, common.rs<-common.ts,
stores.rs<-session-approval-store.ts (+ the deleted permanent-approval-store.ts),
logging.rs<-logging.ts. Upstream \`index.ts\` is large and cyrup spreads it across extension.rs /
gate.rs / forwarding.rs / dedup.rs / ask.rs / skill.rs.

## HARD RULES

- \`spec/\` and \`ADR-0001\` DO NOT EXIST in this workspace despite being cited thousands of times in
  cyrup's own doc comments. A citation to either is NEVER authority for leaving a difference in
  place. There are no accepted divergences.
- **Rust is not JavaScript.** Several upstream behaviours are defences against JS-specific hazards
  (prototype pollution, \`Object.assign\` key order). Port the OBSERVABLE behaviour — which keys end
  up in the map, what the matcher matches — and say plainly in the doc comment when the underlying
  hazard does not exist in Rust. Do not describe such a port as a security fix if it is not one.
- **No-panic policy**: \`unwrap_used\`, \`expect_used\`, \`panic\`, \`indexing_slicing\` are DENIED by
  clippy in non-test code. They do NOT fire under \`cargo build\` or \`cargo test\` — only
  \`cargo clippy\`. Test files opt out with a file-level \`#![allow(...)]\`. Integration tests under
  \`tests/\` are SEPARATE CRATES and inherit nothing from the lib.
- **Never write a large artifact to \`std::env::temp_dir()\` by hand.** Use \`tempfile\`, so cleanup is
  tied to the test's lifetime. A test that leaked 213 MB per run filled this box's 16 GB \`/tmp\`
  tmpfs today and made \`ld\` die with SIGBUS in unrelated crates.

## BUILD DISCIPLINE — READ THIS

There is ONE shared \`target/\` and ONE \`.cargo-lock\`. Concurrent cargo invocations SERIALIZE.
- You MAY run: \`cargo check -p cyrup-permission-system --all-targets\`,
  \`cargo test -p cyrup-permission-system\`, \`cargo clippy -p cyrup-permission-system --all-targets\`.
- You MUST NOT run \`cargo test --workspace\` or \`cargo clippy --workspace\`. The orchestrator runs
  the workspace gate once for the whole batch. \`cargo check -p\` is NOT a sufficient gate for a
  \`pub\` signature change — cross-crate callers stay invisible — so if you change a public
  signature, SAY SO in your return value so the orchestrator watches for it.
- Always run \`cargo clippy\` (not just check) before declaring done, and check the EXIT CODE, not
  the grep output: \`cargo clippy -p cyrup-permission-system --all-targets; echo "exit=$?"\`.
  Expected steady state is ZERO warnings. Any warning in code you touched is yours.

## DEFINITION OF DONE — all six, or the item is not done

1. The behaviour matches pi-permission-system v0.8.0, with the upstream \`file:line\` in a doc
   comment. VERIFY the citation resolves — fabricated citations have shipped in this repo before.
2. A test that FAILS without your change. You must actually run the revert: undo the source edit,
   run the test, PASTE THE FAILURE OUTPUT, restore. A test you merely believe would fail does not
   count. Beware the trap: a regex revert that renames a function's DEFINITION along with its call
   sites is a no-op and leaves everything green — if a "removed" feature's suite stays green,
   your revert did not revert anything.
3. A MIRROR case that stays green, proving the fix is not over-broad (e.g. for a length cap: a
   pattern one char under the cap still matches).
4. If you claim something is wired, NAME the non-test caller. Code whose only callers are in
   \`#[cfg(test)]\` or \`tests/\` is NOT wired — this port has repeatedly shipped mechanisms nothing
   calls.
5. \`cargo clippy -p cyrup-permission-system --all-targets\` exits 0 with no new warnings.
6. You edited ONLY files under \`${CRATE}\`. If your item needs a registration or call site in
   another crate, DO NOT make that edit — report it in \`registration_needed\` and the orchestrator
   will apply it. Cross-file wiring is the orchestrator's job; agents editing shared files in
   parallel have corrupted each other's work in this repo before.

Report honestly. "Partially done, here is exactly what is missing" is far more useful than a
confident claim that does not survive review. If an item turns out to be already implemented, say
so with the file:line and move on — do not invent work.
`

const ITEM_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['item', 'status', 'summary', 'files_changed', 'revert_proof'],
  properties: {
    item: { type: 'string' },
    status: { type: 'string', enum: ['done', 'partial', 'already-implemented', 'blocked'] },
    summary: { type: 'string', description: 'What behaviour changed, concretely' },
    upstream_citation: { type: 'string', description: 'The verified upstream file:line' },
    files_changed: { type: 'array', items: { type: 'string' } },
    tests_added: { type: 'array', items: { type: 'string' } },
    revert_proof: { type: 'string', description: 'The ACTUAL pasted failure output from running the test against reverted source, or an explanation of why none exists' },
    mirror_case: { type: 'string', description: 'The case that stays green and what it rules out' },
    caller: { type: 'string', description: 'The non-test call site that makes this reachable' },
    public_signature_change: { type: 'boolean' },
    registration_needed: { type: 'string', description: 'Edits required OUTSIDE this crate, for the orchestrator to apply. Empty if none.' },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Resolve')

const RESOLVE_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['answers'],
  properties: {
    answers: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['question', 'answer', 'evidence'],
        properties: {
          question: { type: 'string' },
          answer: { type: 'string' },
          evidence: { type: 'string', description: 'file:line on both sides' },
          confidence: { type: 'string', enum: ['certain', 'likely', 'unresolved'] },
        },
      },
    },
  },
}

const resolved = await agent(
  `${COMMON}

## Your job: answer three questions the authoring agents depend on. Do not edit any file.

**Q-A (blocks G129).** \`permanent-approval-store.ts\` was DELETED in pi-permission-system v0.8.0,
but cyrup still ports AND WIRES \`PermanentApprovalStore\` (\`${CRATE}/src/stores.rs:85-180\`, read at
\`gate.rs:161-179\` and \`extension.rs:645,813,1216\`). Establish CONCRETELY:
  - What does v0.8.0 do instead? Does an approval still persist across sessions at all?
  - \`git -C ${WS}/pi-permission-system show v0.7.1:src/permanent-approval-store.ts\` and
    \`git -C ${WS}/pi-permission-system diff v0.7.1..v0.8.0 -- src/index.ts\` are your primary sources.
  - Is there a replacement file/mechanism, or was the capability removed outright?
  - Deleting a \`pub\` type from this crate may break other crates. Grep the WHOLE workspace for
    \`PermanentApprovalStore\` and report every use site outside this crate.

**Q-B (blocks G130).** v0.8.0 adds an \`enabled\` master switch (\`extension-config.ts:11-12,88\`,
\`index.ts:1473-1477\`). cyrup's \`ExtensionConfig::is_pristine_default_file\`
(\`${CRATE}/src/ext_config.rs:205-207\`) does an EXACT STRING COMPARE against
\`default_config_content()\`, and \`is_installed()\` (\`${CRATE}/src/extension.rs:947\`) depends on it.
Adding a fourth key would stop an existing three-key config file reading as pristine. Determine:
  - What exactly does \`is_pristine_default_file\` gate — what changes for a user if a file stops
    reading as pristine?
  - What does upstream's equivalent do at v0.8.0?
  - What is the correct way to add \`enabled\` without breaking existing on-disk configs?

**Q-C (blocks G132).** v0.8.0 caps wildcard patterns at 500 chars and substitutes a never-match
regex (\`wildcard-matcher.ts:15-27\`). cyrup's \`CompiledWildcard\` (\`${CRATE}/src/wildcard.rs:32-66\`)
holds an \`Option<Regex>\`. Read every consumer of that Option and determine whether "never match"
is correctly expressed as \`None\` or as \`Some(<a regex that cannot match>)\` — i.e. do the consumers
treat \`None\` as never-match or as match-everything? Answer with the consuming file:line.

Be concrete. If something is genuinely undetermined, mark it \`unresolved\` and say what you would
need — a guess presented as fact is worse than an admission.`,
  { label: 'resolve:open-questions', phase: 'Resolve', schema: RESOLVE_SCHEMA, effort: 'high' }
)

const answers = (resolved?.answers || [])
  .map((a) => `### ${a.question}\n${a.answer}\nEVIDENCE: ${a.evidence}\nCONFIDENCE: ${a.confidence || '?'}`)
  .join('\n\n') || '(the resolve phase returned nothing — treat every question as unresolved and say so)'

log(`Resolve phase answered ${(resolved?.answers || []).length} questions`)

phase('Author')

// Sequential on purpose: every item is in ONE crate, and G129/G130 both touch extension.rs.
// Parallel agents in a shared file have corrupted each other's work in this repo.
// Order: disjoint low-risk files first, then the ext_config pair, then the extension.rs pair.
const ITEMS = [
  {
    key: 'G131',
    title: 'Un-gate the security-review audit stream from `debug`',
    brief: `cyrup gates its security-review audit logging behind the \`debug\` config flag
(\`${CRATE}/src/logging.rs:147-164\`); upstream emits it unconditionally (\`logging.ts:98-100\`).
Six live call sites are no-ops today for any user who has not turned on debug — an audit trail that
is off by default is not an audit trail. Remove the gate, keeping \`debug\` for whatever upstream
still gates on it. Name the six call sites in your summary and confirm each becomes live.`,
  },
  {
    key: 'G132',
    title: 'Cap wildcard patterns at 500 chars → never-match',
    brief: `Upstream caps a wildcard pattern at 500 characters and compiles an over-long pattern to a
regex that can never match (\`wildcard-matcher.ts:15-27\`), bounding regex-compilation blowup from a
hostile config. cyrup has no cap (\`${CRATE}/src/wildcard.rs:32-66\`). Use the Q-C answer above to
decide how "never match" must be represented so that CONSUMERS see never-match and not
match-everything — getting this backwards turns a DoS guard into a permission bypass, so state
explicitly which way the consumers read it and prove it with a test. Mirror case: a 499/500-char
pattern still matches normally.`,
  },
  {
    key: 'G135',
    title: 'Drop `__proto__`/`constructor`/`prototype` in the frontmatter YAML parser',
    brief: `Upstream's frontmatter parser skips the three keys \`__proto__\`, \`constructor\` and
\`prototype\` (\`common.ts:111-113,132-135\`); cyrup's (\`${CRATE}/src/common.rs:149-187\`) does not.
In JS this is a prototype-pollution defence. **In Rust it is not a security issue** — a HashMap key
named \`__proto__\` is an ordinary string key. Port it anyway, because it is an observable parity
difference: upstream yields a map WITHOUT those keys and cyrup yields one WITH them. Say exactly
that in the doc comment. Do NOT describe this as a security fix.`,
  },
  {
    key: 'G133',
    title: 'Config save: preserve non-extension keys, refuse corrupt, follow symlinks',
    brief: `Upstream has a config-save path (\`extension-config.ts:140-268\`) that (a) preserves keys
it does not own rather than rewriting the whole document, (b) refuses to write over a file it could
not parse, and (c) resolves symlinks before writing. cyrup has NO save function at all in
\`${CRATE}/src/ext_config.rs\`. Port the behaviour. Read the upstream carefully for the corrupt-file
and symlink semantics — silently clobbering a user's hand-edited config with unknown keys dropped
is the failure this prevents. If nothing in cyrup calls a save path today, say so in \`caller\` and
report what would have to call it in \`registration_needed\` rather than inventing a caller.`,
  },
  {
    key: 'G129',
    title: 'Delete `PermanentApprovalStore` — approvals must not cross sessions',
    brief: `\`permanent-approval-store.ts\` was deleted upstream in v0.8.0. cyrup still ports it
(\`${CRATE}/src/stores.rs:85-180\`) and READS it at \`gate.rs:161-179\` and
\`extension.rs:645,813,1216\`, so a hand-written approvals file remains a live, last-match-wins
policy-override channel. Use the Q-A answer above: apply exactly what v0.8.0 does, and if v0.8.0
retains cross-session persistence by some other means, port THAT rather than simply deleting.
Removing a \`pub\` type may break other crates — set \`public_signature_change\` and list every
out-of-crate use site in \`registration_needed\`. Your test must prove the OBSERVABLE change: an
approval written to the permanent file no longer influences a decision.`,
  },
  {
    key: 'G130',
    title: '`enabled` master switch (early return before every registration)',
    brief: `v0.8.0 adds an \`enabled\` master switch that early-returns before every registration
(\`extension-config.ts:11-12,88\`, \`index.ts:1473-1477\`). cyrup has no such key
(\`${CRATE}/src/ext_config.rs:29-45,178-187\`). Use the Q-B answer above — adding a fourth key
naively breaks \`is_pristine_default_file\`'s exact-string compare, which \`is_installed()\` depends
on, so an existing user's config could stop reading as pristine. Land \`enabled\` WITHOUT that
regression and test both: (a) \`enabled: false\` suppresses registration, (b) an existing three-key
config file still behaves exactly as it did before. This item goes last because it shares
\`extension.rs\` with G129.`,
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

## Resolved open questions — USE THESE, they were established for this batch

${answers}

## Your item (${i + 1} of ${ITEMS.length}): ${item.key} — ${item.title}

${item.brief}

Work in ${WS}/cyrup. Make the edit, add the test, run the revert proof, run clippy, and return the
structured result. Remember: only files under \`${CRATE}\`.${prior}`,
    { label: `author:${item.key}`, phase: 'Author', schema: ITEM_SCHEMA, effort: 'high' }
  )
  if (res) authored.push(res)
}

log(`Authored ${authored.length}/${ITEMS.length} items`)

phase('Verify')

const digest = authored
  .map(
    (a) => `### ${a.item} [${a.status}] ${a.summary}
FILES: ${(a.files_changed || []).join(', ')}
TESTS: ${(a.tests_added || []).join(', ')}
UPSTREAM: ${a.upstream_citation || '(none given)'}
CALLER: ${a.caller || '(none given)'}
MIRROR: ${a.mirror_case || '(none given)'}
REVERT PROOF (as reported): ${(a.revert_proof || '(none)').slice(0, 1200)}
REGISTRATION NEEDED: ${a.registration_needed || '(none)'}`
  )
  .join('\n\n')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings'],
  properties: {
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

const LENSES = [
  {
    key: 'revert',
    prompt: `Your lens: ARE THE REVERT PROOFS REAL? For each item, independently RE-RUN the revert
yourself: undo the source change (not the test), run the item's test, observe. A proof is fake if
(a) the test still passes with the change removed, (b) the "revert" was a rename that also renamed
the definition so nothing actually changed, (c) the test asserts something true by construction, or
(d) the test hits its own skip/early-return branch and never reaches the assertion. Restore the
source afterwards and confirm the suite is green again. Report any item whose proof does not hold.`,
  },
  {
    key: 'wiring',
    prompt: `Your lens: IS IT ACTUALLY REACHABLE, AND IS THE FIX COMPLETE? For each item, grep the
whole workspace for callers of what changed. A named caller inside \`#[cfg(test)]\` or \`tests/\` does
NOT count. Check that every site the item claimed to cover was actually covered — e.g. G131 claimed
six call sites; verify all six. Check that deletions (G129) removed every read site and left no
dangling config key that still parses. Check that no item edited a file outside
\`${CRATE}\` without declaring it.`,
  },
  {
    key: 'parity',
    prompt: `Your lens: DOES IT MATCH UPSTREAM v0.8.0, AND ARE THE CITATIONS REAL? For each item,
open the cited upstream file at the right tag (\`git -C ${WS}/pi-permission-system show
v0.8.0:src/<file>\`) and confirm the cited lines exist and say what was claimed. This repo has
shipped fabricated citations before. Then check the ported behaviour actually matches — especially
G132, where representing "never match" the wrong way turns a DoS guard into a permission BYPASS,
and G129, where deleting persistence outright is only correct if v0.8.0 really removed the
capability rather than relocating it. Flag any item that overstates what it did.`,
  },
]

const reviews = await parallel(
  LENSES.map((l) => () =>
    agent(
      `${COMMON}

## Your job: adversarially review what this batch just landed. Do NOT fix anything — report.

${l.prompt}

You may run \`cargo test -p cyrup-permission-system\` and \`cargo clippy -p cyrup-permission-system
--all-targets\`. You may temporarily edit source to run a revert, but you MUST restore it and
confirm the crate's tests pass again before you return. Never leave the tree modified.

Assume each claim below is wrong until you have checked it yourself.

${digest}`,
      { label: `verify:${l.key}`, phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
    )
  )
)

const findings = reviews.filter(Boolean).flatMap((r) => r.findings || [])
const blockers = findings.filter((f) => f.severity === 'blocker')

log(`Review: ${findings.length} findings (${blockers.length} blockers)`)

return {
  batch: 1,
  items: authored.map((a) => ({
    item: a.item,
    status: a.status,
    files: a.files_changed,
    tests: a.tests_added,
    caller: a.caller,
    registration_needed: a.registration_needed,
    public_signature_change: a.public_signature_change,
  })),
  open_question_answers: resolved?.answers || [],
  findings,
  blockers,
  overall: reviews.filter(Boolean).map((r) => r.overall).filter(Boolean),
}
