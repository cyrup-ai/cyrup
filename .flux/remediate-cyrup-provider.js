export const meta = {
  name: 'cyrup-provider-remediate',
  description: 'Remediate the cyrup-provider hygiene tasks in dependency order, parallel where file sets are disjoint',
  whenToUse: 'After the hygiene tasks exist in .flux/todo/. Pass their filenames as args.',
  phases: [
    { title: 'Map', detail: 'read each task, extract the exact file set it will touch' },
    { title: 'Sweep', detail: 'crate-wide tasks, one at a time, in the main tree' },
    { title: 'Split', detail: 'file-scoped tasks in parallel, each in its own worktree' },
    { title: 'Integrate', detail: 'apply the parallel work back and verify the crate as a whole' },
  ],
}

// ---------------------------------------------------------------------------
// Inputs. args = ["DECOMPOSE_X.md", ...] or omitted to take every task whose
// body names crates/cyrup-provider.
// ---------------------------------------------------------------------------
const BRANCH = 'claude/cyrup-provider-api-decompositions'
const REQUESTED = Array.isArray(args) ? args : (args ? [args] : [])

const BASELINE = `
VERIFICATION BASELINE for crates/cyrup-provider, measured on this branch:
  cargo build -p cyrup-provider --all-targets   -> 0 errors, 0 warnings
  cargo clippy -p cyrup-provider --all-targets  -> 14 warnings
  cargo doc -p cyrup-provider --no-deps         -> 0 warnings (broken intra-doc links are DENIED)
  cargo test -p cyrup-provider --lib            -> 1118 pass, 7 ignored, 0 fail
Any task that changes these numbers must say so and justify it. A task that is
supposed to be behaviour-neutral and moves them has a bug.
`

const HOUSE_RULES = `
HOUSE RULES for crates/cyrup-provider (a 1:1 behavioural port of pi's TypeScript
packages/ai — the dense upstream-citation comments ARE the product):
- Port-fidelity commentary is load-bearing. Never delete or reword a "pi <file>:<line>",
  PROV-0xx, or CYRUP-DELTA comment. Move it with the code it annotates.
- NEVER run cargo fmt. The crate is not rustfmt-clean and a blanket format would rewrite
  moved code, destroying both git blame and the pure-movement property.
- The workspace denies clippy::unwrap_used / expect_used / panic / indexing_slicing.
  Code is written slice-getter style deliberately; do not "simplify" a .get(..)? into an index.
- A decomposition is PURE CODE MOVEMENT: no logic rewrites, no reordering, no tidying,
  bundled in. If you find a bug while splitting, report it, do not fix it in the same change.
- The worked reference is crates/cyrup-provider/src/api/bedrock_converse_stream/ — a 4,721-line
  file split along its own "// ---" section banners into 16 modules + a 10-file tests/ tree,
  with visibility minimized by stripping every pub(super) and restoring only what the compiler
  demanded. Read it before splitting anything else.
`

const FILESET = {
  type: 'object',
  properties: {
    task: { type: 'string' },
    kind: {
      type: 'string',
      enum: ['file-scoped', 'crate-wide'],
      description: 'file-scoped = touches a bounded, nameable set of paths. crate-wide = a sweep across many/most files.',
    },
    writes: {
      type: 'array',
      description: 'every path this task will CREATE, MODIFY or DELETE, repo-relative. For a decomposition that is the source file plus the directory it becomes.',
      items: { type: 'string' },
    },
    reads_only: { type: 'array', items: { type: 'string' }, description: 'paths it must read but will not modify' },
    conflicts_with_everything: { type: 'boolean', description: 'true if it rewrites shared files (Cargo.toml, lib.rs, mod.rs) such that nothing else may run beside it' },
    summary: { type: 'string' },
  },
  required: ['task', 'kind', 'writes', 'reads_only', 'conflicts_with_everything', 'summary'],
}

const RESULT = {
  type: 'object',
  properties: {
    task: { type: 'string' },
    done: { type: 'boolean' },
    worktree: { type: 'string', description: 'absolute path of the worktree you worked in, or "" if you worked in the main tree' },
    patch_file: { type: 'string', description: 'absolute path of the patch you wrote, or "" if you worked in the main tree' },
    files_touched: { type: 'array', items: { type: 'string' } },
    gates: { type: 'string', description: 'the gate commands you ran and their real output numbers' },
    deviations: { type: 'string', description: 'anything you did beyond the task, and why. "" if none.' },
    blocked_by: { type: 'string', description: 'what stopped you, if you did not finish. "" if you finished.' },
  },
  required: ['task', 'done', 'worktree', 'patch_file', 'files_touched', 'gates', 'deviations', 'blocked_by'],
}

// ---------------------------------------------------------------------------
phase('Map')
// ---------------------------------------------------------------------------

const listed = await agent(
  `List the //flux hygiene tasks to remediate.

${REQUESTED.length
    ? `The caller named these explicitly: ${REQUESTED.join(', ')}. Use exactly those.`
    : `No explicit list. Read every file in /home/user/cyrup/.flux/todo/*.md and select ONLY those whose body targets crates/cyrup-provider. Skip MCP_*, TEST_FAILURES, CYRUP_IT_COMPILE_ERRORS, FLUX_COMMANDS_FOLLOWUPS and BUILD_FEATURE_COMBINATIONS — those are other crates or workspace-wide.

EXCLUDE WORKSPACE_RUSTFMT_BASELINE.md unconditionally, even though it touches this crate. It
reformats nearly every file in all 22 crates, so it conflicts with everything here, and its own
task file says it must run strictly before or strictly after these — never beside them. A
reformat landing mid-flight destroys the byte-level movement baseline the decompositions are
verified against. It is run separately, by hand, not by this workflow.`}

Return the bare filenames. If none match, return an empty list — do not invent tasks.`,
  { label: 'list-tasks', phase: 'Map', schema: {
    type: 'object',
    properties: { tasks: { type: 'array', items: { type: 'string' } }, note: { type: 'string' } },
    required: ['tasks', 'note'],
  } }
)

const TASKS = (listed?.tasks || []).filter(Boolean)
if (!TASKS.length) {
  log('No cyrup-provider hygiene tasks found in .flux/todo/ — nothing to remediate.')
  return { remediated: [], note: listed?.note || 'empty queue' }
}
log(`${TASKS.length} task(s) to remediate: ${TASKS.join(', ')}`)

// Read each task and extract the exact set of paths it will write. This is what
// makes safe parallelism decidable rather than guessed.
const filesets = (await parallel(TASKS.map(t => () =>
  agent(`Read /home/user/cyrup/.flux/todo/${t} in full.

Work out the EXACT set of repo-relative paths this task will create, modify or delete. Be precise
and complete — this is used to decide which tasks may run concurrently, so an omission causes two
agents to edit the same file at once and corrupt each other's work.

- A decomposition of foo.rs writes: crates/.../foo.rs (deleted) AND crates/.../foo/** (created).
  It does NOT write the parent mod.rs, because \`pub mod foo;\` resolves to the directory unchanged.
  Verify that claim against the task rather than assuming it.
- A lint or doc sweep writes many scattered files — enumerate them by actually running the tool
  that reports them (e.g. cargo clippy) and collecting the file list. If it is more than ~15 files
  or spans most of the crate, call it crate-wide.
- If the task edits Cargo.toml, lib.rs, or a shared mod.rs, set conflicts_with_everything.

Do NOT start the work. This is reconnaissance only.`,
    { label: `map:${t}`, phase: 'Map', schema: FILESET })
))).filter(Boolean)

// ---------------------------------------------------------------------------
// Deterministic wave assignment. Plain JS, no model judgement: two tasks share a
// wave only if their write-sets are provably disjoint by path prefix.
// ---------------------------------------------------------------------------
const overlaps = (a, b) =>
  a.some(x => b.some(y => x === y || x.startsWith(y.replace(/\/?\*+$/, '')) || y.startsWith(x.replace(/\/?\*+$/, ''))))

const sweeps = filesets.filter(f => f.kind === 'crate-wide' || f.conflicts_with_everything)
const scoped = filesets.filter(f => !(f.kind === 'crate-wide' || f.conflicts_with_everything))

const waves = []
for (const f of scoped) {
  let placed = false
  for (const w of waves) {
    if (!w.some(g => overlaps(f.writes, g.writes))) { w.push(f); placed = true; break }
  }
  if (!placed) waves.push([f])
}
log(`plan: ${sweeps.length} exclusive sweep(s) first, then ${waves.length} parallel wave(s) of ${scoped.length} file-scoped task(s)`)
waves.forEach((w, i) => log(`  wave ${i + 1}: ${w.map(f => f.task).join(' | ')}`))

// ---------------------------------------------------------------------------
phase('Sweep')
// ---------------------------------------------------------------------------
// Crate-wide sweeps run FIRST and ALONE, in the main tree. First because a sweep
// over the monolithic files is smaller and simpler than the same sweep over the
// directories they become, and because a decomposition is pure movement — it
// carries the already-clean code along. Alone because their write-set is the crate.

const sweepResults = []
for (const s of sweeps) {
  const r = await agent(`Execute the //flux task /home/user/cyrup/.flux/todo/${s.task} to completion.

${HOUSE_RULES}
${BASELINE}

You are working IN THE MAIN TREE at /home/user/cyrup, alone — no other agent is running. Do not
create a worktree. Do not use git branch/stash/checkout/reset; commit nothing. Leave your changes
in the working tree.

Do exactly what the task says and nothing else. Meet every acceptance criterion in it, and prove
each one with the command and its real output. If you cannot meet one, say which and why in
blocked_by rather than declaring success.

Set worktree and patch_file to "".`,
    { label: `sweep:${s.task}`, phase: 'Sweep', schema: RESULT })
  sweepResults.push(r)
  log(`sweep ${s.task}: ${r?.done ? 'done' : 'INCOMPLETE — ' + (r?.blocked_by || 'no reason given')}`)
}

// ---------------------------------------------------------------------------
phase('Split')
// ---------------------------------------------------------------------------
// File-scoped tasks run in parallel, each in its OWN git worktree with its OWN
// CARGO_TARGET_DIR. Two reasons, both real:
//   1. Verification. Every one of these tasks is verified by whole-crate compilation
//      (build/clippy/test counts). In a shared tree an agent's `cargo build` sees a
//      sibling's half-finished edits, so its numbers are meaningless and it may try to
//      "fix" a file it does not own. Isolation is what makes the gates trustworthy.
//   2. Throughput. cargo takes an exclusive lock on a target dir, so agents sharing one
//      would serialize on every build anyway — the parallelism would be fictional.
// Each agent leaves a patch; the main tree is not touched until Integrate.

const splitResults = []
for (const [i, wave] of waves.entries()) {
  const done = await parallel(wave.map(f => () =>
    agent(`Execute the //flux task /home/user/cyrup/.flux/todo/${f.task} to completion.

${HOUSE_RULES}
${BASELINE}

${wave.length > 1 ? `ISOLATION — read this before touching anything:
You are ONE OF ${wave.length} AGENTS RUNNING CONCURRENTLY, in your own git worktree.

FIRST, before any other work, check what your worktree is based on:
    git log --oneline -1
A fresh worktree is cut from the repository's DEFAULT branch, which is usually NOT the branch this
work belongs on. If HEAD is not ${BRANCH}, run exactly:
    git merge ${BRANCH} --no-edit
That is the ONLY git write operation you are permitted. Skipping this check is how a whole run gets
thrown away: a patch authored against the wrong base targets files that no longer exist on the
target branch, and will not apply.` : `You are running ALONE, in the main checkout at /home/user/cyrup, which is already on the correct
branch. Do NOT create a worktree. Leave your changes in the working tree — no patch file, no commit.`}

Work ONLY on these paths:
${f.writes.map(w => '  ' + w).join('\n')}

- Do NOT modify any file outside that list. If the task seems to require it, stop and report it in
  blocked_by. Another agent may own that file right now.
- Export a private target dir before any cargo command so you do not contend with your siblings:
    export CARGO_TARGET_DIR=$(pwd)/target-iso
  Expect the first build to be slow — it is a cold cache, not a hang.
- Do NOT run git branch/stash/checkout/reset/commit, and do not touch /home/user/cyrup itself.

When the task is complete and every one of its acceptance criteria is proven by a real command:
${wave.length > 1 ? `1. Write your changes as a patch, from the root of your worktree:
     git add -A && git diff --cached --binary > /tmp/remediate-${f.task.replace(/\W+/g, '_')}.patch
   (git add -A then diff --cached is required so that NEW and DELETED files are in the patch.)
2. Report that absolute patch path in patch_file, your worktree root in worktree, and the real
   gate numbers you measured in gates.` : `simply leave the changes in the working tree and report worktree:"" and patch_file:"". Report the
real gate numbers you measured in gates.`}

If you cannot finish, still report what you did and why you stopped. Do NOT fabricate gate numbers
and do NOT report done:true on a task whose gates you did not actually run.`,
      wave.length > 1
        ? { label: `split:${f.task}`, phase: 'Split', isolation: 'worktree', schema: RESULT }
        : { label: `split:${f.task}`, phase: 'Split', schema: RESULT })
  ))
  const ok = done.filter(Boolean)
  splitResults.push(...ok)
  log(`wave ${i + 1}/${waves.length}: ${ok.filter(r => r.done).length}/${wave.length} completed`)
}

// ---------------------------------------------------------------------------
phase('Integrate')
// ---------------------------------------------------------------------------

const patches = splitResults.filter(r => r?.done && r.patch_file)
const failed = [...sweepResults, ...splitResults].filter(r => r && !r.done)

const anyWork = [...sweepResults, ...splitResults].some(r => r?.done)
const integration = anyWork
  ? await agent(`${patches.length
      ? 'Apply the isolated remediation work back into the main tree, then verify the crate as a whole.'
      : 'All remediation ran in place in the main tree, so there is nothing to apply. Verify the crate as a whole — this is still the check that counts, because the tasks were verified one at a time and this is the first look at the combination.'}

${HOUSE_RULES}
${BASELINE}

${patches.length ? `Patches to apply, in this order:
${patches.map(p => `  ${p.patch_file}   (${p.task}, touches: ${(p.files_touched || []).slice(0, 6).join(', ')}${(p.files_touched || []).length > 6 ? ', …' : ''})`).join('\n')}` : 'No patches — the work is already in the working tree.'}

Work in /home/user/cyrup. If there are patches, for EACH one, in order:
  1. git apply --check <patch>   — if it does not apply cleanly, STOP on that patch, record it,
     and continue with the next one. Do not force it and do not hand-edit to make it fit.
  2. git apply <patch>
  3. cargo build -p cyrup-provider --all-targets   — must be clean before you apply the next.
     If a patch that applied cleanly then fails to build, the two changes interact; record exactly
     how and leave the tree in the last good state (git apply -R the offender).

Then verify the WHOLE crate against the baseline above and report every number:
  cargo build -p cyrup-provider --all-targets
  cargo clippy -p cyrup-provider --all-targets
  cargo doc -p cyrup-provider --no-deps
  cargo test -p cyrup-provider --lib
  cargo build --workspace

These tasks were verified INDIVIDUALLY in isolation. This is the first time they exist together,
so this integration check is the one that actually counts. A per-task gate passing in a worktree
proves nothing about the combination.

Do not commit. Do not run git branch/stash/checkout/reset. Leave the result in the working tree.
Report the final numbers, which patches applied, and which did not.`,
      { label: 'integrate', phase: 'Integrate', schema: {
        type: 'object',
        properties: {
          applied: { type: 'array', items: { type: 'string' } },
          rejected: { type: 'array', items: { type: 'string' } },
          final_gates: { type: 'string' },
          regressions_vs_baseline: { type: 'string' },
          tree_state: { type: 'string' },
        },
        required: ['applied', 'rejected', 'final_gates', 'regressions_vs_baseline', 'tree_state'],
      } })
  : null

return {
  tasks: TASKS,
  plan: { sweeps: sweeps.map(s => s.task), waves: waves.map(w => w.map(f => f.task)) },
  sweeps: sweepResults,
  split: splitResults,
  integration,
  incomplete: failed.map(r => ({ task: r.task, blocked_by: r.blocked_by })),
}
