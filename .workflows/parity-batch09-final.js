export const meta = {
  name: 'parity-batch09-final',
  description: 'Batch 9 final — 4 surviving mutants, agent-refinements + child-safe tool divergence, executePublic/rememberParentModel, citation re-audit',
  phases: [
    { title: 'Close', detail: '3 agents on disjoint areas' },
    { title: 'Verify', detail: 'serial mutation + citation re-audit, exclusive tree' },
  ],
}

const BASE = `
Repo: /home/d0m17bw/workspace/cyrup   Crate: crates/cyrup-ext-subagents
Upstream: /home/d0m17bw/workspace/pi-subagents at **v0.43.0** (tag e76a256).
Read it with: git -C /home/d0m17bw/workspace/pi-subagents show v0.43.0:src/<path>
NEVER infer behaviour from a name or from this brief. Open the upstream file, read the whole
function, and COUNT the lines before citing them.

THE GATE — the UNQUALIFIED feature flag, always:
  CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
  CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features test-fixtures
Baseline: 5923 passed / 0 failed / 340 suites, clippy exit 0.

TREE DISCIPLINE:
- Your scratchpad is yours alone: <session scratchpad>/<YOUR-LABEL>/ — create it.
- SURGICAL reverts only. Never restore a whole file: concurrent agents clobbered each other that
  way, and one agent's whole-file restore silently reverted another's committed work.
- Snapshot every file you touch BEFORE touching it, INCLUDING files that start clean — a restore
  that throws on an unsnapshotted file leaks a mutation into the tree.
- NEVER git checkout.

RULES
- Fix, don't file. NEVER weaken, loosen or delete an assertion; print BEFORE/AFTER if you touch one.
- No-panic: clippy DENIES unwrap_used/expect_used/panic/indexing_slicing in non-test code, fires
  ONLY under clippy; tests/ are separate crates needing their own file-level #![allow].
- A subagent run is ALWAYS a real OS subprocess over NDJSON. Never make it in-process.
- Before testing a function, grep for its NON-TEST callers and check they sit ABOVE the
  \`mod tests\` boundary. A test calling it directly does not make it live.
`

phase('Close')

const TASKS = [
  {
    key: 'mutants',
    files: 'src/extension.rs (proactive skills region), src/discovery/management.rs',
    body: `Four mutants survived a 20-mutation serial campaign. All four are in code wired during
the last pass, so each is currently undetectable if removed. Close all four.

S1 — extension.rs:6992, \`setting: proactive_setting.as_ref()\` -> \`setting: None\` kills nothing.
   A config.json \`proactiveSkillSubagents: {minReferences, maxRecommendations, preferredAgent}\` is
   silently droppable. The disable works only ACCIDENTALLY (the scan short-circuits and empties
   available_skills). The tuning knobs have zero end-to-end coverage;
   \`the_extension_config_bridge_...\` tests the bridge function in isolation and never the field
   being passed. Cover the knobs end to end, through the real config surface.

S2 — \`handle_list\` passing \`&[]\` instead of \`&chain_inputs\` to the recommender kills nothing.
   \`proactive_chain_input\` / \`collect_chain_step_skills\` have a production caller but no
   end-to-end coverage. Upstream counts chain-step skill references toward \`minReferences\` — verify
   that against upstream and pin it.

S3 — replacing \`if !suggestions.is_empty() { push blank; extend }\` with an unconditional push kills
   nothing. Upstream's \`...(len ? ["", ...s] : [])\` FALSE branch is unpinned: the two "no block"
   tests only assert the header string is absent, never that no spurious blank line is emitted.
   Pi's layout contract makes a stray blank line a real rendering defect.

S4 — \`positive_integer\`'s \`.filter(|v| *v >= 1)\` -> identity kills nothing. Pre-existing code, now
   live. Check upstream's \`positiveInteger\` and pin the rejection of zero/negative.`,
  },
  {
    key: 'divergences',
    files: 'src/exec/mod.rs, src/extension.rs (child-safe tool + fanout-child region)',
    body: `Three real unported items and one FALSE ASSERTION locking in a divergence.

1. \`appendAgentRefinementOverlay\` is ENTIRELY unported. \`git show
   v0.43.0:src/agents/agent-refinements.ts\` exists; grep for
   refinement_overlay/append_agent_refinement/agent-refinements returns nothing crate-wide.
   Upstream applies it at execution.ts:1442 — the line IMMEDIATELY BEFORE the output-path injector
   that was just wired at :1443. So cyrup's composition order is currently
   memory -> [HOLE] -> output-path. Port it into that slot.

2. \`CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION\` diverges from upstream in TWO ways and the code asserts
   otherwise:
   - the eighth verb \`grant-spawn-budget\` is missing;
   - cyrup's allowed list carries \`stop\`, which upstream has at NEITHER v0.34.0
     (fanout-child.ts:161) NOR v0.43.0 (:180).
   Meanwhile extension.rs:4832 claims the list matches "fanout-child.ts:161 verbatim", and
   extension.rs:11451 asserts the whole string with the message "the child-safe tool must advertise
   pi's exact fanout-child.ts:159-163 text" — both false. extension.rs:15466 then PINS \`stop\`'s
   presence, locking the divergence in behind a test.
   Read upstream, make the string actually match, and correct the tests and the claim. Removing
   \`stop\` from a child-safe allowlist is a real behaviour change: check whether any dispatch arm
   depends on it and handle that in the same change (advertise-vs-dispatch).

3. \`executor.execute(...)\` -> \`executor.executePublic(...)\` on the v0.43.0 fanout-child path,
   unported. Read upstream and port it, including whatever difference \`executePublic\` actually
   makes — do not assume it is a rename.

4. \`resolveSubagentModelOverride\` -> \`resolveEffectiveSubagentModel\`, with
   \`normalizeParentModel\` / \`rememberParentModel(deps.state, requestSessionId, ctx.model)\` at
   :4344-4345. cyrup's inheritance still reads \`ctx.model\` directly; the remembered-parent-model
   indirection is unported. Port it and pin what it changes — a remembered parent model differs
   from a live read exactly when the parent's model changes mid-session, so test that case.`,
  },
  {
    key: 'citations',
    files: 'doc comments and comments ONLY, crate-wide',
    body: `The previous citation pass FIXED 46 but also INTRODUCED errors, and a spot-check of 50
found 9 defective (18%). Of 20 citations whose NUMBER was changed by that pass, 3 were wrong (15%).
Renumbering by uniform shift is what caused it — a shift preserves a pre-existing wrong-function
error while making it look freshly verified. Re-audit, and this time resolve each citation by
reading the target, never by shifting.

Known defective RIGHT NOW (fix these first, then sweep):
- agents.ts:701-722 for readSettingsFileStrict (settings_write.rs:38) — true extent :683-704; the
  cited range starts mid-body and spills into writeSettingsFile at :706.
- shared/artifacts.ts:207-223 for writeMetadata (artifacts.rs:231) — true :221; :207-223 is
  formatOutputArtifactContent.
- tool-budget.ts:62-64 for toolBudgetBlockedMessage (tool_budget.rs:177) — true :66-68. A uniform +6
  shift PRESERVED a pre-existing off-by-one-function error, and its sibling
  toolBudgetSoftNudge is still at :58-60, so two adjacent citations now both claim :62-64.
- acceptance.ts:1207 for the status ternary (acceptance.rs:2620, :2638) — true :1193; :1207 is \`});\`.
- acceptance.ts:1336 for evaluateAcceptance's reject test — true :1297. :842 WAS correct at v0.34.0,
  so the remap actively broke this one.
- async-execution.ts:1256-1267 for a per-call model override (extension.rs:418) — that range is the
  systemPrompt/skill/memory composition; the override is ~:1291.
- pi-args.ts:76-81 for applyThinkingSuffix (true :186 — wrong at v0.34.0 too) · pi-args.ts:108-110
  for mkdirSync(sessionDir) (true :525) · pi-args.ts:201-202 for the intercom presence label ·
  shared/types.ts:283-291 for AcceptanceConfig (true :674; that range is ParallelHandoffManifest).
- slash-commands.ts: three of four :1193-1196 sites were moved to :891-894; the fourth
  (extension.rs:8658) was left at :1193-1196 @v0.34.0. Make them consistent.

Then finish the two acknowledged blind spots:
- **25 distinct / 27 sites still point PAST EOF.** These are the highest-value ones: a citation
  pointing past the end of a file usually means upstream REWROTE that code, so each is a candidate
  PARITY GAP, not mere citation rot. For each, find where the code went at v0.43.0 and report
  whether cyrup's Rust beneath it still matches. Report them as parity findings.
- **198 comma-list citations hide ~244 unaudited line references** (e.g. \`foo.ts:12,44,91\`). The
  extractor only ever read the first. Audit them.

Report: how many checked, how many wrong, how many remain. Touch comment text ONLY — no assert
condition, no code. If a citation is correct but the Rust beneath it has DRIFTED, do NOT silently
fix the comment: report it as a parity finding, because that is a port bug wearing a correct
citation.`,
  },
]

const SCHEMA = {
  type: 'object',
  required: ['items', 'testResult', 'clippyExit'],
  properties: {
    items: {
      type: 'array',
      items: {
        type: 'object',
        required: ['id', 'status', 'whatChanged'],
        properties: {
          id: { type: 'string' },
          status: { type: 'string', enum: ['done', 'partial', 'not-done'] },
          whatChanged: { type: 'string' },
          testsAdded: { type: 'array', items: { type: 'string' } },
          notDoneReason: { type: 'string' },
        },
      },
    },
    parityFindings: { type: 'array', items: { type: 'string' } },
    testResult: { type: 'string' },
    clippyExit: { type: 'string' },
    publicApiChanges: { type: 'array', items: { type: 'string' } },
    editedExistingTests: { type: 'array', items: { type: 'string' } },
  },
}

const done = (
  await parallel(
    TASKS.map((t) => () =>
      agent(
        `Close your assigned items from parity batch 9's final pass.

AREA: ${t.key}
YOUR FILES (other agents own the rest — stay inside these):
  ${t.files}

${t.body}

${BASE}

Do NOT run mutation tests; a serial verifier does that afterwards with exclusive tree access.`,
        { label: `final:${t.key}`, phase: 'Close', schema: SCHEMA },
      ),
    ),
  )
).filter(Boolean)

phase('Verify')

const verify = await agent(
  `You have EXCLUSIVE tree access — every other agent has finished.

Claims to verify:
${JSON.stringify(done, null, 2)}

${BASE}

1. MUTATION TEST every claimed behaviour, one at a time, restoring surgically and byte-comparing
   against a private snapshot after each. At least 18 mutations, weighted toward the four mutants
   that survived last time (S1 proactive setting passthrough, S2 chain_inputs, S3 the empty-branch
   blank line, S4 positive_integer) and toward the newly ported agent-refinements overlay,
   executePublic and rememberParentModel.
2. Verify the CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION fix by diffing cyrup's string against upstream's
   at v0.43.0 CHARACTER BY CHARACTER, and confirm no test still asserts a false "verbatim" claim.
   Confirm nothing dispatches on the removed \`stop\` verb.
3. Spot-check 40 citation fixes by reading v0.43.0 yourself; report the error rate. The last pass
   INTRODUCED errors at 15% while claiming success, so measure, do not trust. Verify the
   past-EOF citations were resolved into real parity findings rather than just renumbered.
4. Run the full gate 2x, once under CPU contention:
     for i in $(seq 1 $(nproc)); do (while :; do :; done) & done
     CARGO_INCREMENTAL=0 cargo test --workspace --features test-fixtures --no-fail-fast
     kill $(jobs -p)
   Check df -h / and df -h /tmp FIRST — / has hit 100% mid-run (ENOSPC silently truncates files;
   rm -rf target/debug/incremental is safe to free) and /tmp is a separate 16 GB tmpfs whose
   exhaustion makes ld die with SIGBUS, reported by the suite as test failures.
5. Confirm no assertion was weakened or deleted: count removed assert*!/#[test] lines in the diff.

RESTORE the tree and say so explicitly. Then state plainly, with evidence, what in batch 9 remains
un-ported — naming an unfinished item beats a clean summary.`,
  { label: 'verify', phase: 'Verify', effort: 'high' },
)

return { done, verify }
