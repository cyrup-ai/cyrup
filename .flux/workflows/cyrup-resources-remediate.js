export const meta = {
  name: 'cyrup-resources-remediate',
  description: 'Remediate the queued cyrup-resources hygiene tasks in dependency order with file-partitioned parallelism',
  whenToUse: 'After the cyrup-resources hygiene audit has queued its flux tasks and they have been reviewed.',
  phases: [
    { title: 'Wave1-Edits', detail: '8 file-disjoint small fixes in parallel' },
    { title: 'Wave1-Gate', detail: 'single authoritative build+test gate' },
    { title: 'Wave2-Decompose', detail: 'discovery.rs and git_url.rs splits in parallel' },
    { title: 'Wave2-Gate', detail: 'build+test gate on the new layout' },
    { title: 'Wave3-Format', detail: 'absorb rustfmt drift last' },
    { title: 'Final-Verify', detail: 'roster, clippy, rustdoc, fmt' },
  ],
}

const C = 'crates/cyrup-resources'

const RULES = `
Repo: /home/user/cyrup. Crate: ${C}.

BASELINE that must never regress:
  cargo test -p cyrup-resources  =>  103 passed; 0 failed; 1 ignored

HARD RULES
- Edit ONLY the files listed in YOUR FILES below. Another agent owns every other file RIGHT NOW.
  Touching one outside your list corrupts their work. If your task seems to need a file you do not
  own, STOP and report it in blocked_on instead of editing it.
- No behavior changes. These are hygiene tasks: docs, lints, visibility, dependency wiring,
  formatting, and pure code motion. If a "fix" would change what the code DOES, do not make it.
- Do NOT run 'cargo fmt' (without --check). Formatting is wave 3's job alone.
- Do NOT run git commands.
- Re-derive every line number from the file before editing. Numbers written in a task file may be
  stale if an earlier wave shifted them. Read, locate, then edit.
- You MAY run 'cargo check -p cyrup-resources --all-targets' to sanity-check your own edit. Do NOT
  run the full test suite -- a shared gate does that after the wave.
`

const EDIT_SCHEMA = {
  type: 'object',
  properties: {
    done: { type: 'boolean', description: 'true only if every change in your scope was applied' },
    changes: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          file: { type: 'string' },
          what: { type: 'string', description: 'what changed, with the line numbers you actually edited' },
        },
        required: ['file', 'what'],
      },
    },
    skipped: { type: 'array', items: { type: 'string' }, description: 'sub-items deliberately NOT done, each with the reason' },
    blocked_on: { type: 'array', items: { type: 'string' }, description: 'files you needed but do not own' },
    check_output: { type: 'string', description: 'result of cargo check on your edit' },
  },
  required: ['done', 'changes', 'skipped', 'blocked_on', 'check_output'],
}

// ---------------------------------------------------------------------------
// Wave 1: partitioned by FILE OWNERSHIP, not by concern. Every group below owns
// a disjoint file set, so all eight can edit concurrently with zero write
// conflict. Concern-partitioning would have put three tasks in discovery.rs.
// ---------------------------------------------------------------------------
const WAVE1 = [
  {
    key: 'discovery-docs-lints',
    files: [`${C}/src/discovery.rs`],
    task: `From .flux/todo/RESOURCES_RUSTDOC_WARNINGS.md and .flux/todo/RESOURCES_LINT_SUPPRESSIONS.md,
apply ONLY the discovery.rs parts:
(a) Fix the unresolved intra-doc link near line 154: qualify [\`PackageStore::packages_root\`] as
    [\`crate::package::PackageStore::packages_root\`].
(b) Split the fused doc run. An unbroken /// run starts around 367 ("Resolve a settings-declared
    package entry...") and runs to ~397, landing on 'pub fn scope_base_dir'. Lines 367-394 describe
    resolve_configured_package (~444), NOT scope_base_dir. Move that block down to sit directly above
    'fn resolve_configured_package', leaving only the final ~3 lines on scope_base_dir. Reword
    nothing. This also clears the two "links to private item" warnings, because those lines land on
    a private fn.
(c) Delete the #[allow(clippy::too_many_arguments)] directly above 'fn scan_prompt_dir' -- that fn
    takes exactly 7 params and clippy fires only above 7, so the allow is dead. KEEP the allows on
    scan_skill_dir (8 params) and add_local_entries (10 params).
Verify with: cargo doc -p cyrup-resources --no-deps 2>&1 | grep -c warning  (expect 4 -> 1 remaining,
the manifest.rs one, which another agent owns).`,
  },
  {
    key: 'manifest-docs-visibility',
    files: [`${C}/src/package/manifest.rs`],
    task: `From RESOURCES_RUSTDOC_WARNINGS.md, RESOURCES_API_SURFACE.md and RESOURCES_LINT_SUPPRESSIONS.md,
apply ONLY the package/manifest.rs parts:
(a) Around line 37, demote the intra-doc link [\`PiPackageJson\`] to a plain code span \`PiPackageJson\`.
    It is a private serde shape; linking it from public docs is the defect.
(b) Drop 'pub' from 'struct CyrupManifest' (~61) and 'struct PackageMeta' (~15) -- workspace-wide grep
    shows 4 references, all inside this file. KEEP the 'package: PackageMeta' field; removing it would
    make [package] optional in cyrup.toml, which is a behavior change.
(c) Normalize the #[cfg(test)] mod allow list (~687) to the four-lint multi-line form in this order:
    clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing.`,
  },
  {
    key: 'store-giturl-allows',
    files: [`${C}/src/package/store.rs`, `${C}/src/package/git_url.rs`],
    task: `From RESOURCES_LINT_SUPPRESSIONS.md, normalize the #[cfg(test)] mod allow lists in BOTH files
to the identical four-lint multi-line form, in this order: clippy::unwrap_used, clippy::expect_used,
clippy::panic, clippy::indexing_slicing.
store.rs (~126) currently lists only three and is missing indexing_slicing -- add it.
git_url.rs (~937) is already four lints; make its spelling and order match exactly.
The task file explicitly chose UNIFY over NARROW: do not trim any list to "only the lints this module
trips". Uniformity is the goal. Change nothing else in either file.`,
  },
  {
    key: 'lib-surface',
    files: [`${C}/src/lib.rs`],
    task: `From RESOURCES_LINT_SUPPRESSIONS.md and RESOURCES_API_SURFACE.md, apply ONLY the lib.rs parts:
(a) Delete the whole #![cfg_attr(not(test), deny(clippy::unwrap_used, ...))] attribute (~20-28). Those
    four lints are already denied unconditionally by [lints] workspace = true, test code included, so
    the attribute is a no-op that falsely implies tests are exempt. KEEP #![forbid(unsafe_code)].
    Replace it with a one-line //! note naming [lints] workspace = true as the single source.
(b) Add ManifestKind to the 'pub use package::{...}' list so a root-exported struct's field type is
    nameable at the root.
(c) Soften the two ResourceHandle doc claims (~12-13 and ~73-74) to say it is the R-09-023 swap
    primitive OFFERED to embedders, and that in-tree consumers currently hold Arc<ResourceRegistry>
    directly. Do NOT delete ResourceHandle -- removing a documented public primitive is a design
    decision, not hygiene.`,
  },
  {
    key: 'theme-docs',
    files: [`${C}/src/theme.rs`],
    task: `From RESOURCES_API_SURFACE.md: extend the doc comment on Theme::resolve_export (~375-376) to
state that the arch-12 HTML-export consumer is not yet in tree and that the only current caller is
src/tests/resources/themes.rs, so a reader does not hunt for a caller that does not exist.
Docs only. Do NOT change ExportColors, resolve_export, or any other item in this file.`,
  },
  {
    key: 'prompt-dead-binding',
    files: [`${C}/src/prompt.rs`],
    task: `From RESOURCES_HYGIENE_UNVERIFIED.md ("Ready" section -- this one IS verified):
Around lines 367-371 replace

    if let Some(after) = rest.strip_prefix("ARGUMENTS") {
        let consumed = 1 + "ARGUMENTS".len();
        let _ = after;
        return Some((all_args.to_string(), consumed));
    }

with

    if rest.starts_with("ARGUMENTS") {
        return Some((all_args.to_string(), 1 + "ARGUMENTS".len()));
    }

Identical behavior; matches the rest.starts_with('@') branch just below. Nothing else in this file.`,
  },
  {
    key: 'deps',
    files: ['Cargo.toml', `${C}/Cargo.toml`, 'crates/cyrup-mcp/Cargo.toml'],
    task: `Apply .flux/todo/RESOURCES_DEP_HYGIENE.md in full:
(a) ${C}/Cargo.toml: 'notify = "8.2.0"' becomes 'notify = { workspace = true }'. The root already
    declares notify = { version = "8.2.0" } and both other consumers take the workspace edge.
(b) Root Cargo.toml: add 'toml = { version = "1.1.2" }' to [workspace.dependencies] near the other
    ratified externals, with a one-line rationale comment naming both consumers, matching the
    commenting style of its neighbours. Then switch ${C}/Cargo.toml and crates/cyrup-mcp/Cargo.toml
    to 'toml = { workspace = true }'.
Leave gix and serde_yml alone -- single-consumer, no drift risk.
Cargo.lock MUST NOT move: the versions are identical. Run 'cargo check --workspace' and confirm
Cargo.lock is unchanged; if it moved, you changed a version and must revert.`,
  },
  {
    key: 'fixtures-dedup',
    files: [`${C}/src/tests/resources/fixtures.rs`],
    task: `Apply .flux/todo/RESOURCES_TEST_FIXTURE_DEDUP.md in full: the file holds three copies of the
same git-subprocess runner -- a closure inside make_local_git_repo (~102), a byte-identical closure
inside make_local_git_repo_two_commits (~131), and the real fn git_in (~159).
Delete both closures and both 'use std::process::Command;' lines that serve them, then rewrite each
call site from git(&[...]) to git_in(&dir, &[...]). git_in stays where it is.
Both fixtures must still return None when the git CLI is unavailable. Change no assertion.`,
  },
]

phase('Wave1-Edits')
const w1 = await parallel(
  WAVE1.map((g) => () =>
    agent(
      `${RULES}\n\nYOUR FILES (edit these and nothing else):\n${g.files.map((f) => '  - ' + f).join('\n')}\n\nTASK:\n${g.task}`,
      { label: `w1:${g.key}`, phase: 'Wave1-Edits', schema: EDIT_SCHEMA }
    )
  )
)

const w1ok = w1.filter(Boolean)
log(`wave 1: ${w1ok.filter((r) => r.done).length}/${WAVE1.length} groups reported done`)
for (const r of w1ok) {
  if (r.blocked_on?.length) log(`  BLOCKED: ${JSON.stringify(r.blocked_on)}`)
  if (r.skipped?.length) log(`  SKIPPED: ${JSON.stringify(r.skipped)}`)
}

// Single authoritative gate. Per-agent test runs would race on one target dir and, worse, one
// agent's half-applied edit would fail another's check and send it chasing a phantom.
const GATE_SCHEMA = {
  type: 'object',
  properties: {
    green: { type: 'boolean' },
    test_line: { type: 'string', description: 'the verbatim "test result:" line' },
    clippy_new_findings: { type: 'array', items: { type: 'string' } },
    rustdoc_warning_count: { type: 'number' },
    problems: { type: 'array', items: { type: 'string' }, description: 'what broke and which file group most likely caused it' },
  },
  required: ['green', 'test_line', 'clippy_new_findings', 'rustdoc_warning_count', 'problems'],
}

const GATE = `${RULES}

You are the WAVE GATE. Edit nothing. Run, in /home/user/cyrup:
  cargo test -p cyrup-resources 2>&1 | grep -E '^test result:'
  cargo clippy -p cyrup-resources --all-targets 2>&1 | grep 'cyrup-resources/src'
  cargo doc -p cyrup-resources --no-deps 2>&1 | grep -c '^warning'
green = the test line reads exactly "test result: ok. 103 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out"
(ignore the timing suffix). If anything is off, say which file group most plausibly caused it, citing
the specific error. Do not fix anything.`

phase('Wave1-Gate')
const g1 = await agent(GATE, { label: 'gate:wave1', phase: 'Wave1-Gate', schema: GATE_SCHEMA })

if (!g1 || !g1.green) {
  log(`WAVE 1 GATE FAILED -- stopping before the decompositions. ${JSON.stringify(g1?.problems)}`)
  return { stopped_at: 'wave1-gate', gate: g1, wave1: w1ok }
}
log(`wave 1 gate green (rustdoc warnings now ${g1.rustdoc_warning_count}, was 4)`)

// ---------------------------------------------------------------------------
// Wave 2: the two decompositions. Different files, so parallel is safe -- but
// they run AFTER wave 1 because a pure code move carries wave 1's small edits
// along for free, whereas the reverse order would strand every line number
// wave 1 depends on.
// ---------------------------------------------------------------------------
phase('Wave2-Decompose')
const w2 = await parallel([
  () =>
    agent(
      `${RULES}\n\nYOUR FILES: ${C}/src/discovery.rs and the new ${C}/src/discovery/ directory you create.\n\n` +
        `Apply .flux/todo/DISCOVERY_RS_DECOMPOSE.md in full.\n\n` +
        `CRITICAL: wave 1 edited discovery.rs, so EVERY line number in that task file is now stale. ` +
        `Re-derive all ranges from the current file before cutting -- locate the eight '// --- ' banners ` +
        `inside discover_blocking and the fn boundaries yourself.\n` +
        `Extract by whole-line range copy so the diff is a verifiable pure move. Do not reword or ` +
        `restructure any body. src/lib.rs must remain unmodified -- preserve the public surface through ` +
        `re-exports in the new mod.rs.\n` +
        `Watch the scan.rs boundary: start it at the '///' doc comment that precedes emit_collisions, ` +
        `NOT at the fn line, or you orphan the doc comment.`,
      { label: 'w2:discovery-split', phase: 'Wave2-Decompose', schema: EDIT_SCHEMA }
    ),
  () =>
    agent(
      `${RULES}\n\nYOUR FILES: ${C}/src/package/git_url.rs and the new ${C}/src/package/git_url/ directory.\n\n` +
        `Apply .flux/todo/GIT_URL_HOSTED_EXTRACT.md in full.\n\n` +
        `CRITICAL: this finding was NOT line-mapped by the audit. FIRST map the seam between cyrup's own ` +
        `URL parsing/security validation and the ported hosted-git-info shorthand table. If the two turn ` +
        `out to be interleaved rather than contiguous, STOP: make no edit, set done=false, and report ` +
        `that in skipped. Forcing a split through interleaved code is worse than leaving the file alone.\n` +
        `Wave 1 edited this file's test-module allow list, so re-derive line numbers.\n` +
        `src/package/mod.rs and src/lib.rs must remain unmodified.`,
      { label: 'w2:git-url-split', phase: 'Wave2-Decompose', schema: EDIT_SCHEMA }
    ),
])

const w2ok = w2.filter(Boolean)
log(`wave 2: ${w2ok.filter((r) => r.done).length}/2 splits applied`)

phase('Wave2-Gate')
const g2 = await agent(GATE, { label: 'gate:wave2', phase: 'Wave2-Gate', schema: GATE_SCHEMA })
if (!g2 || !g2.green) {
  log(`WAVE 2 GATE FAILED -- not formatting on top of a broken tree. ${JSON.stringify(g2?.problems)}`)
  return { stopped_at: 'wave2-gate', gate: g2, wave1: w1ok, wave2: w2ok }
}

// ---------------------------------------------------------------------------
// Wave 3: formatting LAST and ALONE. It is the one job that rewrites every file,
// so any earlier placement guarantees conflicts and rework.
// ---------------------------------------------------------------------------
phase('Wave3-Format')
const w3 = await agent(
  `${RULES.replace("- Do NOT run 'cargo fmt' (without --check). Formatting is wave 3's job alone.", '- You ARE wave 3, so you MAY run cargo fmt.')}

YOUR FILES: every .rs file under ${C}/src/ EXCEPT everything under ${C}/src/tests/.

Apply .flux/todo/RESOURCES_RUSTFMT_DRIFT.md:
  cargo fmt -p cyrup-resources
then IMMEDIATELY restore the test tree, which must stay byte-identical:
  git checkout -- ${C}/src/tests/
(this is the one git command you are permitted, and only this exact one).

WHY the test tree is excluded: the hunks under src/tests/resources/ sit inside bodies that were moved
byte-for-byte in a recent decomposition and were confirmed pre-existing. They are left alone so that
split stays a verifiable pure move.

Then confirm the change is whitespace-only: 'git diff -w --stat' must come back EMPTY. If it is not
empty, cargo fmt changed something structural -- report it and do not proceed.`,
  { label: 'w3:absorb-fmt', phase: 'Wave3-Format', schema: EDIT_SCHEMA }
)

phase('Final-Verify')
const FINAL_SCHEMA = {
  type: 'object',
  properties: {
    test_line: { type: 'string' },
    roster_identical: { type: 'boolean', description: '94 tests::resources leaf names, unchanged' },
    fmt_clean_outside_tests: { type: 'boolean' },
    tests_tree_untouched: { type: 'boolean', description: 'the 6 known hunks under src/tests/ still present' },
    rustdoc_warnings: { type: 'number' },
    clippy_findings: { type: 'array', items: { type: 'string' } },
    summary: { type: 'string' },
  },
  required: ['test_line', 'roster_identical', 'fmt_clean_outside_tests', 'tests_tree_untouched', 'rustdoc_warnings', 'clippy_findings', 'summary'],
}

const final = await agent(
  `${RULES}\n\nFINAL VERIFICATION. Edit nothing. Establish, with commands:
1. cargo test -p cyrup-resources -- the "test result:" line must still read 103 passed; 0 failed; 1 ignored.
2. The test roster is unchanged: cargo test -p cyrup-resources -- --list | grep -c '^tests::resources::' must be 94.
3. cargo fmt -p cyrup-resources -- --check reports NO diff in any src/ file outside src/tests/.
4. The 6 known hunks under src/tests/resources/ are STILL PRESENT (they must not have been absorbed).
5. cargo doc -p cyrup-resources --no-deps -- warning count (was 4 at the start; expect 0).
6. cargo clippy -p cyrup-resources --all-targets -- list any finding in cyrup-resources/src.
Report exactly what you observed. Do not fix anything.`,
  { label: 'final-verify', phase: 'Final-Verify', schema: FINAL_SCHEMA }
)

return { wave1: w1ok, wave1_gate: g1, wave2: w2ok, wave2_gate: g2, wave3: w3, final }
