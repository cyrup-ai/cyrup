export const meta = {
  name: 'cyrup-parity-batch-05',
  description: 'Batch 5 — wire the present-but-unwired: subscription flag, footer marker, bundled resources, renderers, intercom seams',
  phases: [
    { title: 'Unblock', detail: 'G14 first — the footer cannot read a flag that does not exist' },
    { title: 'Wire', detail: 'three crates in parallel; files are disjoint across them' },
    { title: 'Verify', detail: 'ONE reviewer; the risk is claiming wired when it is not' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are working on \`cyrup\` (${WS}/cyrup), a Rust port targeting BEHAVIOURAL EQUIVALENCE with four
read-only TypeScript upstreams at ${WS}/{pi,pi-subagents,pi-permission-system,pi-intercom}.
Ported baselines: pi **v0.83.0** -> target v0.84.1 · pi-subagents **~v0.34.0** -> v0.43.0 ·
pi-intercom **v0.7.0** -> v0.9.2 (its \`lib.rs:2\` says v0.6.0 and is WRONG).

## WHAT THIS BATCH IS, AND WHY IT IS DIFFERENT

Every item here is code that ALREADY EXISTS, compiles, and is tested — but whose only callers are in
\`#[cfg(test)]\` or \`tests/\`. A user cannot reach any of it. This is the single most common defect in
this port: a mechanism ported faithfully and then wired to nothing. G133 shipped ~800 lines that way
last week; the 11 OAuth flows sat unreachable for weeks.

So the bar for this batch is NOT "a non-test caller exists". It is: **name the user action that
reaches this code.** A keystroke, a slash command, a session event, a config value. If you cannot
name one, you have not finished the item, and saying so plainly is the correct outcome.

Corollary: a test that calls the newly-wired function directly proves nothing here — that is what
already existed. Your test must drive the REAL entry point (the registered command, the event
dispatch, the render path) and observe the effect.

## HARD RULES

- **READ the upstream source. NEVER infer behaviour from a name, a type, or this brief.** These
  briefs come from a survey and have been wrong repeatedly: an entire batch was planned around
  "upstream is tolerant of unknown message tags" when it destroys the socket; a recent brief of mine
  cited a length check at a line that contains no such thing; another put an item in the wrong crate
  entirely. If the source disagrees with this brief, FOLLOW THE SOURCE and say so.
- **Verify every citation** at the right tag and COUNT the lines
  (\`git -C ${WS}/<repo> show <tag>:<path>\`). Write version-qualified citations.
- **Classify** port-bug (present at the ported baseline) vs version-lag, and say which.
- **Fix, do not file.** If you find a second defect in the same file, fix it or explain precisely
  why it is a genuinely separate change.
- \`spec/\` and \`ADR-0001\` do not exist in this workspace; neither is authority for anything.
- **No-panic policy**: clippy DENIES \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` in
  non-test code and fires ONLY under \`cargo clippy\`. \`tests/\` files are separate crates that
  inherit nothing and need their own file-level \`#![allow(...)]\`.
- **Do not weaken an assertion.** If a test fails after your change, update its expectation to the
  upstream-correct value and quote the line justifying it, or conclude your change is wrong.

## REVERT PROOFS

\`cp <file> /tmp/claude-1004/-home-d0m17bw-workspace/caf64888-9e1e-4177-beb4-6b77db16a1ee/scratchpad/<uniquename>.SAFE\`
first, edit, run, PASTE the failure, restore with \`cp\` FROM THE BACKUP.
**NEVER \`git checkout\`** — the tree holds uncommitted work; a stray \`git checkout\` destroyed a
1300-line file and it had to be rebuilt from its tests. A revert must remove EVERY guard the test
covers; a partial revert proves only what it removed.

## BUILD DISCIPLINE — OTHER AGENTS ARE RUNNING IN PARALLEL

ONE shared \`target/\`, ONE \`.cargo-lock\`, so cargo WILL block on the lock. That is expected — wait
it out, never kill it.
- Prefix every cargo command with \`CARGO_INCREMENTAL=0\`.
- MAY run \`cargo check/test/clippy -p <your crate> --all-targets\`.
- **MUST NOT run any \`--workspace\` command**, and MUST NOT edit files outside the crates you are
  told you own — another agent owns those right now. Report cross-crate needs in
  \`registration_needed\` and the orchestrator will apply them.
- Finish with \`cargo clippy -p <crate> --all-targets; echo "exit=$?"\` per crate; check the EXIT CODE.
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
        required: ['item', 'status', 'summary', 'user_action', 'revert_proof'],
        properties: {
          item: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'already-implemented', 'blocked'] },
          summary: { type: 'string' },
          user_action: {
            type: 'string',
            description: 'THE key field: the concrete user action that now reaches this code — a keystroke, slash command, config value or session event — and the call chain from it. "A non-test caller exists" is NOT an answer.',
          },
          classification: { type: 'string', enum: ['port-bug', 'version-lag', 'unclear'] },
          upstream_citation: { type: 'string' },
          files_changed: { type: 'array', items: { type: 'string' } },
          tests_added: { type: 'array', items: { type: 'string' } },
          tests_changed: { type: 'string' },
          revert_proof: { type: 'string' },
          mirror_case: { type: 'string' },
        },
      },
    },
    clippy_exit: { type: 'string' },
    public_signature_change: { type: 'boolean' },
    registration_needed: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Unblock')

const g14 = await agent(
  `${COMMON}

## Your crate: \`cyrup-provider\` — you own ONLY files under \`crates/cyrup-provider/\`

**G14 — \`OAuthAuth.isSubscription\` metadata.** Upstream v0.84.1 \`ai/src/auth/types.ts:210-211\` and
\`ai/src/auth/helpers.ts:40-52\`; cyrup \`src/auth/mod.rs:74-110\`.

This runs FIRST and alone because the next item in this batch (the footer's \` (sub)\` marker) reads
the flag you are adding — it cannot be wired until this exists.

Read pi's source and establish precisely:
  - what \`isSubscription\` MEANS (which credentials set it, and on what evidence);
  - the helper in \`helpers.ts:40-52\` that derives it, and whether it is a stored field or computed;
  - **the crucial distinction**: the survey notes the footer needs a NARROWED predicate
    (\`is_subscription\`), NOT the old OAuth-wide one. So establish which OAuth credentials are
    subscriptions and which are not. Getting this backwards mislabels every OAuth user as a
    subscriber, which is a user-visible lie about their billing.

Land the flag with whatever cyrup needs to expose it to a consumer in another crate. The consumer
itself is another agent's job — but say EXACTLY what you exposed and how to read it, in
\`registration_needed\`, because someone is about to depend on it.

For \`user_action\`: this item's own user-facing effect may be indirect (it enables the footer). Say
so honestly rather than inventing one.`,
  { label: 'unblock:G14', phase: 'Unblock', schema: ITEM_SCHEMA, effort: 'high' }
)

const g14Digest = g14
  ? `### G14 landed — what it exposed (the footer item depends on this)
${(g14.items || []).map((i) => `- ${i.item} [${i.status}]: ${i.summary}\n  CITATION: ${i.upstream_citation || '(none)'}`).join('\n')}
REGISTRATION / HOW TO READ THE FLAG: ${g14.registration_needed || '(none reported)'}
NOTES: ${g14.notes || '(none)'}`
  : '(G14 returned nothing — establish the flag yourself before wiring the footer, and say so)'

log('G14 phase complete')

phase('Wire')

const CRATES = [
  {
    key: 'tui',
    crate: 'cyrup-tui',
    brief: `**G35+G49 — the footer \` (sub)\` marker.**
\`crates/cyrup-tui/src/status.rs:100/185/186/277/278\` define the field, the setter and the render.
The ONLY caller of \`set_using_subscription\` workspace-wide is \`tests/render.rs:387\` — a test. The
marker therefore can never appear in a real session.

Wire it to the flag G14 just landed (see above). Note the survey's warning: use the NARROWED
subscription predicate, not the old OAuth-wide one, or every OAuth user is mislabelled as a
subscriber — a user-visible falsehood about billing.

Find pi's own footer source and match when the marker shows and exactly what it renders (string,
spacing, position relative to the other footer segments). Do not guess the text.

\`user_action\` must be concrete: what does a user do, with what credential, to see \` (sub)\` appear —
and your test must drive that path, not call \`set_using_subscription\` directly.`,
  },
  {
    key: 'subagents',
    crate: 'cyrup-ext-subagents',
    brief: `Three items. G93 and G147 both live in \`src/registration/resources.rs\` — do them one
after the other, not concurrently.

**G93 — bundled prompt templates.** \`bundled_prompt_files()\` at \`src/registration/resources.rs:61\`;
its only caller is \`resources.rs:204\`, inside its own \`#[cfg(test)]\`. Seven \`.md\` recipes ship at
\`crates/cyrup-ext-subagents/resources/prompts/\` and nothing registers the directory. Upstream:
\`pi-subagents/src/slash/prompt-workflows.ts:269,303\` and the \`/prompt-workflow\` + \`/chain-prompts\`
commands. Establish how upstream registers them and what a user types to reach one.

**G147 — the bundled skill.** \`bundled_skill_files()\` at \`resources.rs:77\`; only caller \`:226\`, a
test. A 58 KB \`resources/skills/pi-subagents/SKILL.md\` ships and is never registered.
ALSO: \`src/prompt_runtime.rs:69-70\` contains a comment claiming "this crate has no skills/
directory". That is factually false — the directory exists. Fix the comment; a false comment that
retires a question is worse than the gap it hides.

**G128 — subagent renderers.** \`tui/events.rs:612\` (\`render_inline_result\`) and \`:738\`
(\`render_async_jobs_widget\`) are called only from \`events.rs:854,867\`, their own test module.
\`SubagentsExtension\` (\`extension.rs:7078\`) implements neither \`render_call\` nor \`render_result\` and
never calls \`register_tool_renderer\`. The host substrate is proven live by
\`crates/cyrup-tui/tests/extension_renderers.rs:46\` — read that test to learn the seam's real shape
before wiring. The already-written spinner animation is dead code today.

For each: \`user_action\` must name what a user does to see the effect.`,
  },
  {
    key: 'intercom',
    crate: 'cyrup-intercom',
    brief: `Two items, both small, both about seams that already exist on the host side.

**G145 — the editor-text seam / \`/intercom-id\`.** \`HostServices::editor_text\` and
\`set_editor_text\` (\`cyrup-ext/src/host/services.rs:209,250\`) are implemented at
\`cyrup-session-svc/src/host_services.rs:667-669\` and consumed at \`cyrup-tui/src/app.rs:2441,2643\`.
\`cyrup-intercom\` registers exactly ONE command (\`src/extension.rs:266\`). Upstream has an
\`/intercom-id\` command; per the survey this is a second \`register_command\` plus a match arm, needing
no new seam. Verify that against \`pi-intercom\`'s v0.9.2 source — find what \`/intercom-id\` actually
does there (what it inserts, and where) rather than assuming.

**G143 — context usage.** \`HostServices::context_usage\` is live at \`host_services.rs:690-702\`, and
\`cyrup-intercom\` already holds the \`Arc<dyn HostServices>\` (it uses it at \`src/inbound.rs:229\`).
Nothing in the crate reads it. Find what upstream does with context usage at v0.9.2 — the
\`presence\` message carries \`contextPct\`/\`contextTokens\`/\`contextWindow\`, which this crate now models,
so check whether this is the producer half of that. If so, wiring it makes cyrup's presence frames
carry real numbers where they currently carry nothing.

For each: \`user_action\` must name what a user does, or what a peer observes, as a result.`,
  },
]

const wired = await parallel(
  CRATES.map((c) => () =>
    agent(
      `${COMMON}

${g14Digest}

## Your crate: \`${c.crate}\` — you own ONLY files under \`crates/${c.crate}/\`

${c.brief}

Work your items one at a time in ${WS}/cyrup: read upstream, wire, test through the REAL entry
point, full revert proof with cp-backup. Finish with clippy for your crate.`,
      { label: `wire:${c.key}`, phase: 'Wire', schema: ITEM_SCHEMA, effort: 'high' }
    )
  )
)

const ok = [g14, ...wired].filter(Boolean)
const allItems = ok.flatMap((r) => (r.items || []).map((i) => ({ ...i, crate: r.crate })))
log(`Wired ${allItems.length} items across ${ok.length} crates`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored', 'reachability_table'],
  properties: {
    tree_restored: { type: 'boolean' },
    reachability_table: {
      type: 'array',
      description: 'One row per item. This is the point of the review.',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['item', 'user_action', 'verdict'],
        properties: {
          item: { type: 'string' },
          user_action: { type: 'string', description: 'The action you VERIFIED reaches the code, with the call chain' },
          verdict: { type: 'string', enum: ['reachable', 'still-unwired', 'partially-reachable'] },
          evidence: { type: 'string' },
        },
      },
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
    (a) => `### [${a.crate}] ${a.item} [${a.status}] (${a.classification || 'unclassified'})
${a.summary}
CLAIMED USER ACTION: ${a.user_action || '(none given)'}
UPSTREAM: ${a.upstream_citation || '(none)'}
FILES: ${(a.files_changed || []).join(', ')}
TESTS ADDED: ${(a.tests_added || []).join(', ')}
TESTS CHANGED: ${(a.tests_changed || '(none)').slice(0, 600)}
MIRROR: ${a.mirror_case || '(none)'}
REVERT PROOF: ${(a.revert_proof || '(none)').slice(0, 700)}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this batch. Do NOT fix anything; report.

**Lens 1 — REACHABILITY. This is the entire point of the batch.** For EVERY item, independently
verify the claimed user action actually reaches the code. Trace the full chain yourself, from the
entry point (keystroke handler, registered command table, event dispatch, render path) down to the
function. Fill \`reachability_table\` with a verdict per item.
  - "a non-test caller exists" is NOT reachable — that was true of several of these BEFORE the batch.
  - A caller that exists but sits behind a condition nothing satisfies is \`still-unwired\`.
  - If the test drives the function directly rather than the entry point, say so: it proves nothing
    this batch was meant to establish.
This port has repeatedly shipped mechanisms wired to nothing, including ~800 lines in one item, so
assume every claim is wrong until you have traced it.

**Lens 2 — did the wiring change behaviour it should not have?** These are additive wirings. Check
each for collateral: G35+G49 must show the marker ONLY for genuine subscription credentials (a
narrowed predicate — mislabelling every OAuth user as a subscriber is a user-visible falsehood about
billing); G93/G147 must not shadow or reorder user-authored prompts/skills with bundled ones;
G128 must not change rendering for tools it does not own.

**Lens 3 — weakened assertions.** For every pre-existing test edited, \`git diff\` it and judge:
updated to the upstream-correct expectation, or loosened so a failure went away? Name each verdict.

**Lens 4 — revert proofs.** Re-run each yourself with cp-backup / cp-restore; never \`git checkout\`.
Confirm each mirror stays green while the fix-specific test fails.

**Lens 5 — citations and classification.** Open every cited line at the stated tag and COUNT. Check
each port-bug/version-lag call against the ported baseline. Recent briefs of mine contained a
fabricated line reference and an item filed under the wrong crate, so verify rather than trust.
Also confirm the stale comment at \`cyrup-ext-subagents/src/prompt_runtime.rs:69-70\` ("this crate has
no skills/ directory") was actually corrected.

## PROCESS

Back up before any edit, restore with \`cp\`, confirm affected crates are green, set
\`tree_restored: true\`. \`git status --porcelain\` at start and end must match. Prefix cargo with
\`CARGO_INCREMENTAL=0\`; never run \`--workspace\`.

${digest}`,
  { label: 'verify:reachability', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
const table = review?.reachability_table || []
return {
  batch: 5,
  items: allItems.map((a) => ({ crate: a.crate, item: a.item, status: a.status })),
  reachability_table: table,
  still_unwired: table.filter((r) => r.verdict !== 'reachable'),
  weakened_assertions: review?.weakened_assertions,
  tree_restored: review?.tree_restored,
  findings,
  blockers: findings.filter((f) => f.severity === 'blocker'),
  registration_needed: ok.map((r) => r.registration_needed).filter(Boolean),
  overall: review?.overall,
}
