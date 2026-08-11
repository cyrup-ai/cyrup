export const meta = {
  name: 'cyrup-parity-batch-01-fix',
  description: 'Batch 1 remediation — wire G133 save to real callers, fix truncation and G130 config-load defects',
  phases: [
    { title: 'Fix', detail: 'sequential: all items share extension.rs / ext_config.rs' },
    { title: 'Verify', detail: 'ONE reviewer — parallel reviewers corrupted the shared tree last run' },
  ],
}

const WS = '/home/d0m17bw/workspace'
const CRATE = 'crates/cyrup-permission-system'

const COMMON = `
You are working on \`cyrup\` (${WS}/cyrup), a Rust port targeting BEHAVIOURAL EQUIVALENCE with
pi-permission-system. Ported baseline v0.7.1; target **v0.8.0** (latest). pi CORE has no permission
system — cite \`${WS}/pi-permission-system\`, never \`${WS}/pi\`.

## HARD RULES

- \`spec/\` and \`ADR-0001\` do not exist here. Neither is ever authority for leaving a difference.
- **Rust is not JavaScript.** When an upstream behaviour guards a JS-specific hazard that does not
  exist in Rust, port the observable behaviour and SAY SO plainly. Do not describe a parity port as
  a security fix. (Last run an agent reproduced upstream's ReDoS rationale for a cap that cannot
  matter to Rust's linear-time \`regex\` engine — that is the exact mistake to avoid.)
- **Verify every citation you write.** \`git -C ${WS}/pi-permission-system show v0.8.0:src/<file>\`
  and count the lines. Last run, three doc comments cited v0.7.1 line numbers that land on entirely
  different functions at v0.8.0. Fabricated and stale citations have shipped in this repo.
- **No-panic policy**: \`unwrap_used\`/\`expect_used\`/\`panic\`/\`indexing_slicing\` are DENIED by clippy
  in non-test code, and fire ONLY under \`cargo clippy\`, never build/test. Test files opt out with a
  file-level \`#![allow(...)]\`; \`tests/\` files are separate crates inheriting nothing.
- Never write a large artifact to \`std::env::temp_dir()\` by hand — use \`tempfile\`.

## BUILD DISCIPLINE

ONE shared \`target/\`, ONE \`.cargo-lock\`; concurrent cargo invocations serialize.
- MAY run: \`cargo check/test/clippy -p cyrup-permission-system --all-targets\`.
- MUST NOT run any \`--workspace\` command; the orchestrator gates the batch once.
- Always finish with \`cargo clippy -p cyrup-permission-system --all-targets; echo "exit=$?"\` and
  check the EXIT CODE. Expected steady state is ZERO warnings; any warning you introduce is yours.

## DEFINITION OF DONE

1. Behaviour matches v0.8.0, with a VERIFIED upstream \`file:line\` in a doc comment.
2. A test that FAILS without your change — actually run the revert, PASTE the failure output,
   restore. A revert that renames a definition along with its call sites changes nothing and leaves
   the suite green; if a "removed" feature stays green, your revert did not revert.
3. A MIRROR case that stays green, proving the change is not over-broad.
4. Anything you call wired must have a NAMED non-test caller. Callers only in \`#[cfg(test)]\` or
   \`tests/\` do NOT count. This is the whole point of this remediation — see item F1.
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
    caller: { type: 'string', description: 'The non-test call site making this reachable' },
    public_signature_change: { type: 'boolean' },
    registration_needed: { type: 'string' },
    clippy_exit: { type: 'string' },
    notes: { type: 'string' },
  },
}

phase('Fix')

const ITEMS = [
  {
    key: 'F1',
    title: 'Wire `ExtensionConfig::save` to real callers (completes G133)',
    brief: `G133 landed \`ExtensionConfig::save\` plus ~800 lines of \`OrderedJson\` machinery with
**ZERO non-test callers** — every \`.save(\` call site is inside \`#[cfg(test)] mod tests\`. So the
v0.8.0 delta it exists to close (non-extension keys preserved, corrupt file refused, symlink written
through) is unobservable in cyrup, because cyrup never saves this config at all. That is the
present-but-unwired defect this entire effort exists to eliminate.

Port the two upstream consumers so the primitive is reachable:

  - \`saveExtensionConfig(next, ctx)\` — v0.8.0 \`index.ts:1402-1420\`. Normalizes, saves, and on
    failure notifies through \`ctx.ui\` and returns WITHOUT mutating in-memory state. On success it
    updates the live config, syncs status, clears \`lastConfigWarning\`, and writes a
    \`config.saved\` DEBUG entry. It is registered as \`setConfig\` on the \`permission-system\`
    command (\`index.ts:1502-1508\`).
  - \`setYoloModeFromRuntimeApi(enabled, options)\` — v0.8.0 \`index.ts:1422-1470\`, exposed on the
    extension's runtime API as \`setYoloMode\` and \`toggleYoloMode\` (\`index.ts:1483-1484\`). Note
    its details: non-boolean input returns an error result unchanged; \`options.persist !== false\`
    controls whether it writes; a failed save returns \`persisted: false\` WITHOUT changing the
    in-memory yolo mode and writes \`yolo_mode.update_failed\`; success writes \`yolo_mode.updated\`.

Read both upstream functions in full before writing anything. Match the ERROR paths exactly — the
security-relevant property is that a failed persist must not leave in-memory state claiming yolo
mode changed. If cyrup's command/runtime-API surface cannot express one of these, port what it can
express, say precisely what is missing in \`notes\`, and put the cross-crate need in
\`registration_needed\` rather than inventing a seam.

Your test must drive a REAL caller — not \`save\` directly — and prove the config file changed on
disk. \`caller\` must name a non-test call site.`,
  },
  {
    key: 'F2',
    title: 'Fractional forwarded-timeout is silently truncated on save',
    brief: `\`forwarded_prompt_timeout_seconds\` is \`Option<u64>\` and \`normalize\` does
\`Some(n) if n.is_finite() && n > 0.0 => Some(n as u64)\` (\`${CRATE}/src/ext_config.rs:305-312\`).
Upstream keeps any finite positive number (v0.8.0 \`extension-config.ts:83-84\`), so
\`forwardedPromptTimeoutSeconds: 45.5\` survives upstream at 45.5 but cyrup REWRITES it to 45.
Because that key is one of the EXTENSION_CONFIG_KEYS the new save path writes back
(\`ext_config.rs:388-397\`), this now silently mutates an operator's file, not just the in-memory
value.

Fix the truncation so a fractional timeout round-trips. Check how the value is CONSUMED
(\`forwarding.rs\` builds a \`Duration\` from it) and keep that working — a fractional second must
produce a fractional duration, not a re-truncation at the point of use. If you change the field's
type, that is a public change: set \`public_signature_change\` and grep the workspace for readers.`,
  },
  {
    key: 'F3',
    title: 'G130: duplicate config load, and a config-path split with `is_installed`',
    brief: `Two defects in the \`enabled\` master switch as landed.

(a) DOUBLE LOAD. \`extension.rs\` checks \`ExtensionConfig::load(config_path_for(&agent_dir)).enabled\`
and then the constructor's \`derive_parts\` loads the SAME file again (\`extension.rs:365-369\`).
\`load\` \`eprintln!\`s on a malformed config, so an operator with a corrupt \`config.json\` now sees
the identical warning twice per session build where they saw it once. Upstream loads once
(\`index.ts\` holds one \`extensionConfig\`). Load once and thread it through.

(b) PATH SPLIT. \`is_installed\`'s pristine probe reads the RAW \`config_path_for(agent_dir)\` with no
env consultation, while the new \`enabled\` check goes through \`ExtensionConfig::load\` ->
\`resolve_config_path\`, which honours \`CYRUP_PERMISSION_SYSTEM_CONFIG_PATH\`. With that override set
the two gates inspect DIFFERENT files, so the install decision and the on/off decision can disagree.
Worse, \`load\` materializes the default template at the override path as a side effect of merely
deciding whether to attach. Upstream funnels everything through one \`getPermissionSystemConfigPath()\`.
Make both consult the same resolved path, and add the test neither new test covers: the override set
to a file whose \`enabled\` differs from the on-disk template.

Also fix the locking hazard the review found: \`extension.rs:1748\`
\`not_installed_without_policy_or_env_returns_none\` mutates the process-global \`INSTALL_ENV_VAR\`
with NO lock, and its comment claims no other test touches that variable — which G130 falsified by
adding \`without_install_env\`. A mutex only serializes parties that take it. Make that test take
\`env_lock()\` and correct its comment.`,
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

Work in ${WS}/cyrup. Edit, test, run the revert proof, run clippy, return the structured result.${prior}`,
    { label: `fix:${item.key}`, phase: 'Fix', schema: ITEM_SCHEMA, effort: 'high' }
  )
  if (res) fixed.push(res)
}

log(`Fixed ${fixed.length}/${ITEMS.length} items`)

phase('Verify')

// ONE reviewer, not three. Last run three parallel reviewers each did revert experiments on the
// same working tree; they clobbered each other and for ~30s the tree did not contain G129 at all.
// Adversarial breadth is not worth a corrupted tree — the orchestrator's gate would have committed
// whatever state it happened to observe.
const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings', 'tree_restored'],
  properties: {
    tree_restored: { type: 'boolean', description: 'You MUST confirm the tree is exactly as you found it' },
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
CALLER: ${a.caller || '(none)'}
REVERT PROOF: ${(a.revert_proof || '(none)').slice(0, 1500)}
REGISTRATION NEEDED: ${a.registration_needed || '(none)'}`
  )
  .join('\n\n')

const review = await agent(
  `${COMMON}

## Your job: adversarially review this remediation. You are the ONLY reviewer — cover all three
## lenses yourself, in this order. Do NOT fix anything; report.

**Lens 1 — reachability (the reason this remediation exists).** F1 claims it wired
\`ExtensionConfig::save\` to real callers. Verify by grepping the WHOLE workspace for call sites and
confirming at least one is outside \`#[cfg(test)]\` and \`tests/\`. Trace it back to something a user
can actually trigger (a registered command, a runtime API the host exposes). If \`save\` is still
reachable only from tests, that is a BLOCKER and the remediation failed at its one job.

**Lens 2 — revert proofs.** Re-run each item's revert YOURSELF. A proof is fake if the test passes
with the change removed, if the "revert" was a rename that also moved the definition, if the
assertion is true by construction, or if the test silently hits a skip branch. For F1, note the
proof is necessarily counterfactual for NEW code — judge whether the reconstructed "before" is an
honest v0.7.1-shaped baseline or one cherry-picked to fail.

**Lens 3 — citations and parity.** Open every upstream line cited by the new code
(\`git -C ${WS}/pi-permission-system show v0.8.0:src/<file>\`) and confirm it says what is claimed.
For F1 specifically, check the ERROR paths against \`index.ts:1422-1470\`: a failed persist must
leave in-memory yolo mode UNCHANGED and report \`persisted: false\`. Getting that backwards would
mean the gate believes it is in yolo mode when disk says otherwise.

## CRITICAL PROCESS RULE

You may temporarily edit source to run a revert, but you MUST restore it and confirm
\`cargo test -p cyrup-permission-system\` is green before returning, then set \`tree_restored: true\`.
Run \`git status --porcelain\` at the start and again at the end and confirm the file list matches.
Nothing else is touching this tree; if you see it change under you, STOP and report it as a blocker.

${digest}`,
  { label: 'verify:all-lenses', phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
)

const findings = review?.findings || []
return {
  batch: '1-fix',
  items: fixed.map((a) => ({
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
