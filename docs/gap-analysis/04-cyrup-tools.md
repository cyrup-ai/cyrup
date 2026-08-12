# 04 — cyrup-tools (the built-in tool set)

This area covers `crates/cyrup-tools` — the seven built-in tools (`read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`), their registry, the `ops` filesystem/process seam, the glob matcher, the file-mutation lock and the `isolation/` decorators — plus the places tool metadata crosses into `cyrup-core`, `cyrup-ext` and `cyrup-session-svc`, which are the tool *surface* and are routed explicitly per item. It is measured against `pi/packages/coding-agent/src/core/tools/` at pi v0.83.0 (the coding-agent copy is pi's live path; the thinner `packages/agent/src/harness/tools/` fork is not the reference). Headline: the metadata plumbing repaired by `9ccc8ff` and `67bf079` genuinely closed the large descriptive gaps for built-ins and survives adversarial re-verification to the provider request, but the same surface is still dead for guest tools, the write path still loses file mode and symlink identity, three glob/limit semantics diverge silently, and five tests assert either nothing or an outcome they cannot control. Re-baselined against HEAD `1806375` on 2026-08-03; every item below was re-read at source on both sides.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| TOOL-001 | closed | 9ccc8ff. Re-attacked at three levels — `ToolMeta` residue, vtable override, and reachability to the model. `crates/cyrup-agent/src/agent.rs:656` reads `t.description()` inside the `Vec<cyrup_provider::ToolDef>` build at `:651-659`, so the string genuinely reaches the provider request. All seven pi description constants byte-compared. The `label` half was never part of this item and lives in TOOL-022. |
| TOOL-002 | closed | 67bf079. `EditTool::prepare_arguments` at `crates/cyrup-tools/src/tools/edit.rs:132-134`; the agent runs it before validation (`agent.rs:893` then `:896`). The obvious refutation — a second batch path skipping the preflight — was checked by reading BOTH `execute_parallel` (`agent.rs:1087`, prepares at `:1114`) and `execute_sequential` (`:1251`, prepares at `:1269`). No such path exists. Guest half stays open under TOOL-022. |
| TOOL-003 | closed | 9ccc8ff. `prompt_snippet`/`prompt_guidelines` on the vtable (`crates/cyrup-core/src/tool.rs:112-114`, `:120-122`), read with no name table at `crates/cyrup-session-svc/src/builder.rs:1424-1430`. Evidence correction: the render sites are `crates/cyrup-session/src/prompt/builder.rs:166-180` and `:200-208`, not `:210,239`. cyrup declares guidelines for 3 of pi's 4 guideline-bearing built-ins; `bash` is TOOL-008. |
| TOOL-004 | open | Atomic-write metadata loss. Unchanged at HEAD. |
| TOOL-005 | closed | 67bf079. Verified at the mechanism level in the vendored `grep-searcher-0.1.16` (`src/searcher/glue.rs:116-128`, `src/searcher/core.rs:215-238`), not at the builder flag. |
| TOOL-006 | open | `write`/`edit` still declare `ExecMode::Sequential`. |
| TOOL-007 | open | `protect_paths: true` still hardcoded, still bypassed by `bash`. |
| TOOL-008 | open | `bash` session env still absent; a third stale comment found this pass. |
| TOOL-009 | open | `supports_images` still unwired in production. |
| TOOL-010 | open | Glob negation still unsupported; the rg-side claim is now source-verified. |
| TOOL-011 | open | `find` relative-vs-absolute glob target. |
| TOOL-012 | open | Registration order still `read,write,edit,bash,…`. |
| TOOL-013 | open | `read` errno text still collapsed to one message. |
| TOOL-014 | open | `edit` access-error body still diverges; the wrong comment is still present. |
| TOOL-015 | open | `render_kind` still has zero consumers. |
| TOOL-016 | open | `constrainedSampling` unrepresented (post-baseline drift). |
| TOOL-017 | open | Compact `read` rendering — owner routed to cyrup-tui, kept here for traceability. |
| TOOL-018 | open | Fuzzy empty-needle divergence. |
| TOOL-019 | open | Mutation queue still per-registry. |
| TOOL-020 | open | Forced-SIGKILL drain test still asserts scheduling. |
| TOOL-021 | open | `prompt_guidelines` return type still `&[&str]`. |
| TOOL-022 | open | Guest `renderShell`/`prepareArguments`/`label` still unreachable. |
| TOOL-023 | open | `find` full-tree walk then sort-truncate. |
| TOOL-024 | open | `every_surface_method_delegates` still vacuous for 9 of 11. |
| TOOL-025 | open | `write_creates_dirs_and_serializes` concurrency half still vacuous. |
| TOOL-026 | open | `bash_timeout_fractional_seconds` wall-clock ceiling. |

Four items closed. Nothing was overturned; no previously-closed item reopened. TOOL-027 through TOOL-030 are new this pass.

## Open items

> **⚠ THIS TABLE IS NOT THE COMPLETE OPEN SET.** 6 further items from the 2026-08-03
> surface-driven sweep live in their own table under `## Surface-sweep findings` (line ~444), with
> `-S` ids — **including 0 rated critical/high**. Enumerating only this table undercounts the
> area by 6 items, which is exactly how `SEAM-S01` (high) escaped a full audit pass on
> 2026-08-07. Count BOTH tables. See structural defect A in `00-residual-ledger.md`.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| TOOL-004 | high | parity-bug | M | `write`/`edit` temp-file+rename drops mode, ownership, symlink and hard-link identity |
| TOOL-008 | high | not-ported | M | `bash` exposes no session env, scrubs no stale keys, and three in-repo comments state upstream backwards |
| TOOL-006 | medium | parity-bug | S | `write`/`edit` declare `Sequential`, serializing the whole batch |
| TOOL-007 | medium | cyrup-original | M | Protected-path write block is on by default, has no pi analog, and `bash` bypasses it |
| TOOL-009 | medium | parity-bug | M | `read`'s non-vision image warning can never fire in the production wiring |
| TOOL-010 | medium | parity-bug | S | `grep`'s `glob` does not support negation (`!pattern`) |
| TOOL-015 | medium | not-ported | M | `edit` does not declare `renderShell: "self"`; nothing reads `render_kind` |
| TOOL-019 | medium | parity-bug | S | File mutation queue is per-`ToolRegistry`, not process-global |
| TOOL-020 | medium | test-defect | M | Forced-SIGKILL drain test asserts scheduling and shell-buffering outcomes |
| TOOL-021 | medium | parity-bug | S | `Tool::prompt_guidelines` returns `&[&str]`, so guest tools lose their guidelines |
| TOOL-022 | medium | not-ported | L | `renderShell`, `prepareArguments` and `label` never reach a guest tool's behavior |
| TOOL-023 | medium | parity-bug | S | `find` walks the whole tree then sorts and truncates; pi passes `--max-results` |
| TOOL-024 | medium | test-defect | S | `every_surface_method_delegates` proves nothing for 9 of its 11 assertions |
| TOOL-025 | medium | test-defect | S | `write_creates_dirs_and_serializes`'s concurrency half is vacuous |
| TOOL-027 | medium | parity-bug | S | `grep`'s glob inherits fd's `**/` auto-prepend, which ripgrep applies in reverse |
| TOOL-028 | medium | not-ported | M | `BashSpawnContext.env` is override-only, so env-key deletion is unrepresentable |
| TOOL-011 | low | parity-bug | S | `find` path-globs match the relative path; pi/fd match the absolute path |
| TOOL-012 | low | parity-bug | S | Built-in registration order diverges from pi (`write`/`bash` swapped) |
| TOOL-013 | low | parity-bug | S | `read`'s missing/unreadable error replaces pi's Node errno text |
| TOOL-014 | low | parity-bug | S | `edit`'s access-failure body diverges from pi's `Error code: <ERRNO>` |
| TOOL-016 | low | upstream-drift | M | `constrainedSampling` has no representation in cyrup's tool model |
| TOOL-017 | low | not-ported | M | `read`'s compact call rendering (SKILL.md / docs / AGENTS.md) not ported |
| TOOL-018 | low | parity-bug | S | `edit` fuzzy matcher returns not-found where pi returns duplicate-occurrences |
| TOOL-026 | low | test-defect | S | `bash_timeout_fractional_seconds` asserts a wall-clock upper bound |
| TOOL-029 | low | parity-bug | S | `ls` swallows pi's `Cannot read directory: <message>` |
| TOOL-030 | low | test-defect | S | `exec_pre_cancelled_never_spawns` carries a 200ms wall-clock ceiling |

## TOOL-004 — `write`/`edit` temp-file+rename drops file mode, ownership, symlink and hard-link identity

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/ops/local.rs:58-81` `LocalFs::write_atomic`: `create_dir_all` at `:59-64`, tmp name `<name>.cyrup-tmp-<id>` at `:65-72`, `tokio::fs::write(&tmp, bytes)` at `:73-75`, `tokio::fs::rename(&tmp, path)` at `:76-80`. Read in full at HEAD: it never reads the target's `Permissions`, never chowns, never canonicalizes a symlink. Both mutators route through it — `crates/cyrup-tools/src/tools/write.rs:89`, `crates/cyrup-tools/src/tools/edit.rs:221`.

**upstream** — `pi/packages/coding-agent/src/core/tools/edit.ts:83-86` `defaultEditOperations.writeFile: (path, content) => fsWriteFile(path, content, "utf-8")` — Node `O_WRONLY|O_CREAT|O_TRUNC`, preserving inode, mode, owner, hard links and the symlink target. `write.ts` uses the same `fsWriteFile`.

**Impact** — The tmp file is created with the process default `0666 & ~umask`, so editing a mode-`0600` secrets file (`~/.ssh/config`, a private key, a `.netrc`) silently WIDENS it to world-readable — a confidentiality regression, not merely a lost `+x` bit. Separately, a symlink is replaced by a regular file and hard links are broken.

**Fix** — Canonicalize the target first, then `set_permissions` from the existing target's metadata onto the tmp file before the rename (plus a unix chown when privileged). Alternatively add a `write_in_place` seam to `crates/cyrup-tools/src/ops/mod.rs` and truncate-in-place when the target exists, keeping create-then-rename only for new files.

**Verify** — In `crates/cyrup-tools/tests/tools.rs`: `chmod 0700` a file, `edit` it, assert `0700` survives; symlink `a -> b`, `write` to `a`, assert `a` is still a symlink and `b` changed.

## TOOL-008 — `bash` does not expose session metadata env vars, does not scrub stale ones, and three in-repo comments state the upstream fact backwards

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/bash.rs:109` builds the child env as `shell_env(self.opts.bin_dir.as_deref())` — only an optional PATH override (`crates/cyrup-tools/src/ops/shell.rs:159-180`), and `bin_dir` is `None` in production (`crates/cyrup-tools/src/config.rs:73`; `crates/cyrup-session-svc/src/builder.rs:619-628` sets only `command_prefix`/`shell_path` then spreads `..BashOpts::default()`). `BashOpts` (`config.rs:49-64`, read in full) has no `expose_session_environment` field, and `grep -rn 'SESSION_ID|SESSION_FILE|REASONING_LEVEL' crates/cyrup-tools/src/` is empty — neither the injection nor the scrub exists. Three wrong comments: `bash.rs:82-83` claims "Pi defines no promptGuidelines for bash, so the trait default (`&[]`) is used"; the same line cites the description as `bash.ts:284-285` when at HEAD it is `bash.ts:327`; `crates/cyrup-tools/tests/pi_schema.rs:116-117` reads as though `exposeSessionEnvironment` were opt-in, then pins bash's guideline set to `&[]` at `:182-188`.

**upstream** — `pi/packages/coding-agent/src/core/tools/bash.ts:322` `const exposeSessionEnvironment = options?.exposeSessionEnvironment ?? true;` — DEFAULT TRUE. `:329-331` ships `promptGuidelines: exposeSessionEnvironment ? ["Inspect PI_* environment variables for current model and session details."] : undefined`. `resolveSpawnContext` (`bash.ts:158-184`) UNCONDITIONALLY deletes `PI_SESSION_ID`/`PI_SESSION_FILE`/`PI_PROVIDER`/`PI_MODEL`/`PI_REASONING_LEVEL` at `:166-170` before repopulating from `ctx` at `:171-181`.

**Impact** — A model running `bash` under cyrup cannot discover its own session id, session file, provider, model or reasoning level — a capability pi grants by default and advertises in the prompt. In the negative direction: a cyrup process launched from a pi-flavoured or subagent parent leaks the parent's stale `PI_SESSION_ID`/`PI_MODEL` into every `bash` child, so scripts that read them act on the wrong session.

**Fix** — Add `expose_session_environment: bool` (default true) to `BashOpts`; thread session id / session file / provider / model / reasoning level through `ToolsOptions` from `crates/cyrup-session-svc/src/builder.rs:619-628`; add the guideline to `BashTool::prompt_guidelines`. The scrub half cannot be written until TOOL-028 gives the spawn seam a deletion channel — land the two together. Fix `pi_schema.rs:182-188` and all three comments in the same commit.

**Verify** — Assert a `bash` child sees `CYRUP_SESSION_ID` matching the live session; assert an injected stale value is removed even when `expose_session_environment` is off; assert `BashTool::prompt_guidelines()` is non-empty by default.

## TOOL-006 — `write`/`edit` declare `ExecMode::Sequential`, serializing the whole tool batch

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/write.rs:56-58` and `edit.rs:119-121` both return `cyrup_core::ExecMode::Sequential` with no `[CYRUP-DELTA]` justification. `crates/cyrup-agent/src/agent.rs:813-816` computes `any_seq` by scanning every call in the batch, `:817` sets `sequential = any_seq || matches!(self.tool_execution, ToolExecution::Sequential)`, then `:821-822` routes the WHOLE batch to `execute_sequential` (`:1251`). Per-file serialization is already provided independently by `FileMutationLocks` (`write.rs:83`, `edit.rs:189`).

**upstream** — `grep -rn executionMode pi/packages/coding-agent/src/core/tools/` hits only `tool-definition-wrapper.ts:16,44` (the plumbing). No pi built-in sets it; pi relies solely on `withFileMutationQueue`.

**Impact** — A batch containing one `edit` plus several `read`s or `grep`s runs entirely serially, adding latency proportional to batch size on the most common multi-tool turn shape.

**Fix** — Delete both `execution_mode` overrides.

**Verify** — Assert `ExecMode::Parallel` for both in `crates/cyrup-tools/tests/pi_schema.rs`, plus an agent-level test that a batch of one `edit` and two `read`s yields `sequential == false` at `agent.rs:813-817`.

## TOOL-007 — `write`/`edit` to `.env`, `.git/`, `node_modules/` blocked by default, no pi analog, and `bash` bypasses it

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-session-svc/src/builder.rs:208` sets `protect_paths: true` in `SessionConfig::new`; `:615-616` wraps the fs with `ProtectedFs::with_defaults(fs)`. `grep -rn protect_paths crates/ --include=*.rs` returns four lines — `builder.rs:152` (doc), `:153` (decl), `:208` (the `true`), `:615` (the application) — so there is no CLI flag, no setting, no override. `ProtectedPaths::defaults()` is `[".env", ".git", "node_modules"]` (`crates/cyrup-tools/src/isolation/protected.rs:29-32`), matched by path-COMPONENT equality (`:51-59` `is_protected`). `builder.rs:617` builds `Backend { fs, proc: base.proc.clone() }` — only `fs` is decorated, so `bash 'echo K=v >> .env'` bypasses the guard entirely. The module doc at `crates/cyrup-tools/src/isolation/mod.rs:3-7` asserts the opposite of the wiring: "by default nothing here is in the call path".

**upstream** — No protected-path concept exists anywhere under `pi/packages/coding-agent/src/core/tools/`; `write.ts` and `edit.ts` write whatever path they are given.

**Impact** — A silent, undocumented, unoverridable refusal on three common paths. The model is told nothing about the restriction (no description text, no guideline), so it retries or routes around it via `bash` — which succeeds, making the guard security theatre while still costing a failed turn.

**Fix** — Decide deliberately: either flip `builder.rs:208` to `false` and expose a flag/setting, or keep it on and (a) surface it in the `write`/`edit` descriptions plus a `prompt_guidelines` entry, (b) decorate `ProcOps` so `bash` is covered, (c) correct `isolation/mod.rs:3-7`. Sibling `confine_to_cwd` (`builder.rs:155,209,612`) is correctly `false` and needs no change. Note `default_bash_rm_rf_runs_without_any_gate` (`crates/cyrup-tools/tests/isolation.rs:53-71`) is NOT a test defect — it builds `Backend::default()` directly and is accurate for `bash`.

**Verify** — With the guard on: assert `write` to `.env` is refused AND `bash 'echo x >> .env'` is refused. With it off: assert both succeed and no `ProtectedFs` is in the chain.

## TOOL-009 — `read`'s non-vision-model image warning can never fire in the production wiring

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/read.rs:227` gates the note on `if self.opts.supports_images`; `ReadOpts::default()` sets `supports_images: true` (`crates/cyrup-tools/src/config.rs:28,38`). Those are the only three sites in the crate — no production caller sets it false. `crates/cyrup-session-svc/src/builder.rs:617-628` customizes only `bash` and spreads `..ToolsOptions::default()`. The field is per-tool-INSTANCE, so it could not track a mid-session `/model` switch even if wired.

**upstream** — `pi/packages/coding-agent/src/core/tools/read.ts:87-92` `getNonVisionImageNote(model)` returns undefined when `model.input.includes("image")`, and is computed PER CALL from `ctx?.model` at `read.ts:246`.

**Impact** — A text-only model asked to `read` an image gets an image block and no warning, so it either hallucinates content or errors at the provider instead of being told the file is unreadable to it.

**Fix** — Drop `supports_images` from `ReadOpts` and derive it per call from the tool context's model — the same channel `bash` needs for TOOL-008.

**Verify** — `read_image_non_vision_keeps_block_and_warns` (`crates/cyrup-tools/tests/tools.rs:134-159`) is green today ONLY because it constructs `ReadOpts { supports_images: false, .. }` at `:126-130`, a configuration production never produces; it is false assurance. Rewrite it to drive the model through the tool context and assert the note fires under a text-only model in the production wiring.

## TOOL-010 — `grep`'s `glob` argument does not support negation (`!pattern`)

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/grep.rs:128-131` compiles the argument via `PatternMatcher::build(g)`. `crates/cyrup-tools/src/tools/globmatch.rs:17-33` hands the (possibly `**/`-prefixed) string straight to `globset::GlobBuilder` with no `!` handling, so a leading `!` compiles as a literal character; the matcher is applied purely as an include-filter at `grep.rs:161-164`. A negated pattern therefore matches nothing and excludes everything.

**upstream** — `pi/packages/coding-agent/src/core/tools/grep.ts:218` `if (glob) args.push("--glob", glob);` passes it verbatim to real ripgrep. Negation verified in-workspace rather than from docs: rg's `--glob` routes through `ignore-0.4.26/src/overrides.rs:142-144` (`OverrideBuilder::add` → `GitignoreBuilder::add_line`), and `ignore-0.4.26/src/gitignore.rs:475-478` sets `glob.is_whitelist = true` and strips the leading `!`.

**Impact** — `grep(glob="!*.test.ts")`, the natural way to exclude tests, silently returns zero results instead of everything-but-tests. The model reads that as "no matches" and draws the wrong conclusion about the codebase.

**Fix** — In `globmatch.rs`, strip a leading `!`, record the polarity, expose include/exclude sets, and apply exclusion at `grep.rs:161-164`. Do NOT apply the same treatment to `find` — fd's `--glob` has no `!` semantics (`pi/packages/coding-agent/src/core/tools/find.ts:246-252`). Land with TOOL-027, same function.

**Verify** — A tree containing `a.ts` and `a.test.ts` both matching the pattern; `glob="!*.test.ts"` must return only `a.ts`. Red today (returns nothing).

## TOOL-015 — `edit` does not declare `renderShell: "self"`; nothing in the workspace reads `render_kind`

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `render_kind`/`ToolRenderKind` outside `cyrup-core/src/tool.rs` resolves to exactly the re-export at `crates/cyrup-core/src/lib.rs:33`, the forwarder at `crates/cyrup-ext/src/wrapper.rs:24,110-111` and its assertion at `:287`, plus the guest-only `crates/cyrup-ext-sdk/src/descriptor.rs`. ZERO consumers — no site branches on the value. `impl Tool for EditTool` (`crates/cyrup-tools/src/tools/edit.rs:111-238`) does not override it. `crates/cyrup-tui/src/transcript.rs` dispatches on the run name (`render_read` at `:1041`, `render_write` at `:1069`) and reads no render kind.

**upstream** — `pi/packages/coding-agent/src/core/tools/edit.ts:306` `renderShell: "self"` — the only built-in that sets it; the field is declared at `pi/packages/coding-agent/src/core/extensions/types.ts:465`.

**Impact** — `edit`'s self-rendered diff is wrapped in the standard tool frame, so the transcript shows a redundant outer header/box around a view that already presents itself — a persistent divergence from pi on the most-used mutating tool, and the mechanism a guest tool would need is entirely absent.

**Fix** — Two halves. (1) `EditTool::render_kind` returns `ToolRenderKind::SelfRendered` (S). (2) `cyrup-tui` must honour it: stamp the kind onto `ToolRun` at execution start and skip the standard frame in `transcript.rs` (M — the real cost). Guest half is TOOL-022.

**Verify** — A TUI snapshot over an `edit` run showing no outer frame, plus a unit assertion that `EditTool::render_kind()` is `SelfRendered` and every other built-in is `Default`.

## TOOL-019 — File mutation queue is per-`ToolRegistry`, not process-global

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/registry.rs:47` constructs `let locks = Arc::new(FileMutationLocks::new());` INSIDE `with_builtins`, shared only between that registry's `WriteTool` (`:51-56`) and `EditTool` (`:57-62`); there is no `LazyLock`/`static` instance anywhere in the crate. Secondary defect: `crates/cyrup-tools/src/lock.rs:50-52` `key()` calls the BLOCKING `std::fs::canonicalize` and is called from inside the async `guard()` at `:62`.

**upstream** — `pi/packages/coding-agent/src/core/tools/file-mutation-queue.ts:4-5` keeps `fileMutationQueues` at MODULE level, so serialization by realpath spans every tool instance in the process.

**Impact** — Two sessions or two registries in one process (subagent orchestration, the SDK embedder) can interleave read-modify-write on the same file, silently losing one edit. The blocking canonicalize additionally stalls a runtime thread on a slow or network filesystem.

**Fix** — Hoist the map to a process-global `static LazyLock<FileMutationLocks>` (or inject one shared instance from `cyrup-session-svc`), and switch to `tokio::fs::canonicalize` inside the async `guard()`.

**Verify** — The lock primitive itself IS correctly tested for mutual exclusion (`crates/cyrup-tools/src/lock.rs:78-109` `same_path_serializes`, concurrency counter with `assert_eq!(max, 1)`); what is missing is a cross-registry case. Add it as part of the TOOL-025 rewrite: two separate `ToolRegistry` instances editing the same path concurrently, both edits must survive. Red today.

## TOOL-020 — Forced-SIGKILL stdout-drain test asserts scheduling and shell-buffering outcomes it cannot control

**Kind** test-defect · **Severity** medium · **Effort** M · **Confidence** high (analytic)

**cyrup** — `crates/cyrup-tools/src/ops/local.rs:991-1038`, `exec_argv_forced_sigkill_does_not_drop_buffered_stdout_already_sitting_in_the_pipe`. Eight trials (`:992`); each builds `LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_millis(15))` (`:996`) and runs a trapped-TERM `sh -c` loop under a 15ms timeout (`:1009-1015`). Two assertions lie outside its control. (a) `:1024-1029` `assert!(gt_last >= 0, …)` requires fork+exec of `/bin/sh`, the `exec 3>>` redirect and one full loop iteration inside roughly 30ms (15ms to SIGTERM plus a 15ms grace to SIGKILL); under a loaded `cargo test --workspace` a fork/exec alone can exceed that. (b) `:1030-1036` `assert!(gt_last - stdout_last <= 1, …)` assumes the shell flushes stdout once per iteration, but the shell comes from `ShellConfig::detect()` — under a `/bin/sh` whose `printf` block-buffers a pipe-connected stdout, a whole buffer never reaches the pipe before SIGKILL. The doc comment at `:971-988` CONCEDES the timing is "inherently racy" and defends the test with per-host trial counts — exactly the pattern commit `1806375` removed from `crates/cyrup-ext/src/caps/proc.rs`.

**upstream** — Not a parity question: the behavior under test is a correct port of `pi/packages/coding-agent/src/core/tools/exec.ts:52-63` `killProcess`. Only the assertion strategy is defective.

**Impact** — Intermittent failure in the repo's only gate — cyrup has no CI, so `cargo test` is all there is. Once a file is known to flake, a genuine regression in it gets dismissed.

**Fix** — Make the invariant deterministic: (1) replace the fixed 15ms timeout with a barrier — the child writes a `ready` marker after its first iteration and the timeout starts only once the marker exists, so `gt_last >= 0` holds by construction; (2) force line-buffered stdout independently of the host shell (`stdbuf -oL`, or a small Rust helper instead of a shell) so the lag bound stops depending on `ShellConfig::detect()`; (3) failing both, delete the trial loop as `1806375` did — drain-to-EOF can be pinned by feeding a known byte count through a pipe with no signal timing involved. The house technique already exists: `bash_trailing_edge_flush_emits_midstream` (`crates/cyrup-tools/tests/tools.rs:1029-1030`) uses `#[tokio::test(start_paused = true)]`.

**Verify** — After the rewrite the test must pass under `cargo test --workspace` with artificial load and `--test-threads=32`. Do NOT sweep in the three sibling `< Duration::from_secs(2)` bounds at `local.rs:757`, `:782`, `:844` — each guards a ~100-200ms expected path against a 5s grace-period alternative (~3s slack) and is load-bearing.

## TOOL-021 — `Tool::prompt_guidelines` returns `&[&str]`, so guest tools silently lose their declared guidelines

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-core/src/tool.rs:120-122` declares `fn prompt_guidelines(&self) -> &[&str] { &[] }`. A `&[&str]` cannot be produced from a `Vec<String>` without an allocation the borrow cannot outlive, so `WasmTool` does not implement it: `crates/cyrup-ext/src/host/live.rs:1341-1375`, read in full, implements `name` (:1342), `parameters` (:1345), `execution_mode` (:1348), `description` (:1354), `prompt_snippet` (:1361) and `execute` (:1364), then stops. The data DOES reach the host — WIT carries `prompt-guidelines: list<string>` (`crates/cyrup-ext/wit/world.wit:38`), `register_tool` copies it (`live.rs:84`), it is stored at `crates/cyrup-ext/src/registry.rs:27` — but that field has no reader anywhere in the workspace, and the vtable is the only surface `tool_contribution` reads (`crates/cyrup-session-svc/src/builder.rs:1428`).

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:459` `promptGuidelines?: string[]` on `ToolDefinition`; `tool-definition-wrapper.ts:13,41` copies it onto every AgentTool.

**Impact** — Every WASM extension tool's declared usage guidance is dropped between the guest and the system prompt, with no warning. Extension authors get a field that appears to work and does nothing.

**Fix** — Widen the trait return type rather than working around it: `fn prompt_guidelines(&self) -> Vec<&str>` (or `Cow<'_, [&str]>`). The three built-ins that declare guidelines keep their const arrays with a trivial `.to_vec()`; `RegisteredTool` (`crates/cyrup-ext/src/wrapper.rs:107-109`) forwards unchanged; `WasmTool` becomes `self.descriptor.prompt_guidelines.iter().map(String::as_str).collect()`. `tool_contribution` (`builder.rs:1428`) already maps into owned `Arc<str>`, so nothing downstream changes.

**Verify** — Register a guest tool declaring two guidelines, assert both appear in the built system prompt. `crates/cyrup-tools/tests/pi_schema.rs:135-157` must stay green unchanged.

## TOOL-022 — `renderShell`, `prepareArguments` and `label` never reach a guest tool's behavior

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** high

**cyrup** — There are two `ToolDescriptor` types and they disagree. Guest-facing (`crates/cyrup-ext-sdk/src/descriptor.rs:29-50`) carries `render_shell: RenderShell` (`:45`) and `prepare_arguments` (`:46-49`). Host-facing (`crates/cyrup-ext/src/registry.rs:16-30`, read in full) has neither, and neither does the WIT record it is built from: `crates/cyrup-ext/wit/world.wit:31-40` and `crates/cyrup-ext-sdk/wit/world.wit:31-40` are byte-identical 8-field records (name / label / description / parameters-json / exec-mode / prompt-snippet / prompt-guidelines / has-renderer). `register_tool` (`crates/cyrup-ext/src/host/live.rs:71-86`) constructs the host descriptor field-by-field from that record, so there is no path by which a guest's `renderShell`/`prepareArguments` could arrive; `WasmTool` (`live.rs:1341-1375`) overrides neither, so the `cyrup_core::Tool` defaults apply. `label` DOES reach the host descriptor (`registry.rs:18`, populated at `live.rs:76`) but `WasmTool` never maps it onto `Tool::label`, and `Tool::label` has no consumer anywhere (only the forwarder `crates/cyrup-ext/src/wrapper.rs:100-102` and its assertion at `:283`).

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:452` (`label: string`, REQUIRED), `:465` (`renderShell`), `:468` (`prepareArguments`); `tool-definition-wrapper.ts:11,15,39,43` copies `label` and `prepareArguments` onto every AgentTool, extension tools included.

**Impact** — Three losses. (1) A guest tool declaring an argument-normalizing shim never gets it run — precisely TOOL-002, un-fixed, for every WASM tool. (2) A `renderShell: "self"` guest tool is double-framed with no diagnostic. (3) A guest's distinct display name is unreachable, harmless today only because `Tool::label` has no consumer at all.

**Fix** — Add `render-shell: option<render-shell>` and `prepare-arguments: bool` to the WIT `tool-descriptor` record in BOTH copies (`f777e44` established that both must move together and that this breaks the guest ABI), mirror them onto `crates/cyrup-ext/src/registry.rs:16-30`, map them in `register_tool` (`live.rs:74-86`), then implement `render_kind` and `prepare_arguments` on `WasmTool` — the latter via a guest-side `prepare-arguments` export the host calls only when the flag is set. Add `fn label` to `WasmTool` at the same time.

**Verify** — A fixture guest tool whose `prepare_arguments` renames a key: assert the renamed key reaches `execute` and passes schema validation. A second declaring `renderShell: "self"`: assert no outer frame in the TUI. Note the ABI break — any component built against the old world must be rebuilt.

## TOOL-023 — `find` walks the whole tree then sorts and truncates; pi passes `--max-results` to fd

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/find.rs:112-143` drains the entire walk into `results` with no early exit (the `loop { tokio::select! { … } }` breaks only on `None` from the walker), then `:154-157` `results.sort(); if results.len() > limit { results.truncate(limit); }`. The limit selects the alphabetically-first N of the COMPLETE match set, and traversal cost is the full tree regardless of `limit`. `limit_reached` is computed off the truncated vector (`:159`).

**upstream** — `pi/packages/coding-agent/src/core/tools/find.ts:241` `args.push("--max-results", String(effectiveLimit))` — fd stops after N results in its own parallel, unordered traversal; pi relativizes only the lines it received (`find.ts:307-320`), never sorts, and computes `resultLimitReached` off that same set (`find.ts:322`).

**Impact** — On a large repo, `find` with a small `limit` still pays the full-tree walk (seconds where pi returns in milliseconds) and returns a different result SET — the alphabetically-first N rather than the first N discovered. The `effectiveLimit` computation itself matches (cyrup `find.rs:103`, no `.max(1)`, unlike grep's at `grep.rs:126`); only the selection strategy diverges.

**Fix** — Decide and document: either (i) break out of the walk at `find.rs:141` once `results.len() == limit` and drop the `sort()`, matching fd's early-exit semantics and removing the full-tree cost; or (ii) keep sort-then-truncate for determinism and add a `[CYRUP-DELTA]` at `find.rs:154`. Option (ii) plus an early break at some multiple of `limit` is not equivalent and should not be used.

**Verify** — Build a tree of 10k files, run `find` with `limit=5`, assert wall time is not proportional to tree size (option i) or assert the documented determinism (option ii). Assert `limit_reached` matches the chosen semantics.

## TOOL-024 — `every_surface_method_delegates` proves nothing for 9 of its 11 assertions

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-ext/src/wrapper.rs:277-294`, read in full at HEAD. The inner tool is `Fixed` (`wrapper.rs:180-207`), which overrides only `name`, `parameters` and `execute`; every other method falls to the `cyrup_core::Tool` default. Of exactly eleven assertions, nine compare a default against itself — `description()` `""` vs `""`, `label()` `None` vs `None`, `prompt_snippet()` `None`, `prompt_guidelines()` `[]`, `render_kind()` `Default`, `execution_mode()` `Parallel`, `render_call`/`render_result` against literal `None`, and `prepare_arguments` asserted to be the identity. Only `name` and `parameters` discriminate. Nine of eleven pass identically whether `RegisteredTool` forwards to `self.inner` (`wrapper.rs:87-121`) or omits the override entirely.

**upstream** — `pi/packages/coding-agent/src/core/extensions/wrapper.ts` `wrapRegisteredTool` spreads the whole definition (`{...tool, execute: instrumented}`), so forwarding is structural and cannot regress. cyrup's hand-written per-method delegation can, which is exactly why the guard matters here and not upstream.

**Impact** — The wrapper is the only path by which the built-ins' newly restored descriptions, snippets and guidelines (TOOL-001 / TOOL-003) reach the agent and the prompt builder — `crates/cyrup-session-svc/src/builder.rs:957,973-975` puts it on that path. A regression that drops a forwarder silently re-opens TOOL-001 with a green suite.

**Fix** — Give `Fixed` non-default metadata: distinct `description`, `label`, `prompt_snippet`, `prompt_guidelines`, `render_kind: SelfRendered`, `execution_mode: Sequential`, `render_call`/`render_result` returning `Some(..)`, and a `prepare_arguments` that mutates the value.

**Verify** — After the change, comment out any single forwarder in `wrapper.rs:87-121` and confirm the test goes red; today deleting nine of them leaves it green.

## TOOL-025 — `write_creates_dirs_and_serializes`'s concurrency half is vacuous

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/tests/tools.rs:356-403`, read in full at HEAD. The test spawns two concurrent `WriteTool::execute` calls to the same `race.txt` sharing one `FileMutationLocks` (`:375-399`), then asserts at `:402` `final_content == "AAAA" || final_content == "BBBB"`. That assertion cannot fail whether or not the lock is held: `LocalFs::write_atomic` (`crates/cyrup-tools/src/ops/local.rs:65-80`) writes a uniquely named tmp file and `rename`s it over the target, and rename is atomic, so the surviving content is always exactly one writer's full payload. Removing `self.locks.guard(&abs, &cancel)` from `crates/cyrup-tools/src/tools/write.rs:83` leaves the test green. The name claims "serializes"; nothing in it observes ordering, mutual exclusion or the queue.

**upstream** — `pi/packages/coding-agent/src/core/tools/file-mutation-queue.ts:4-5` keeps `fileMutationQueues` at module level and `write.ts`/`edit.ts` wrap the whole read-modify-write in `withFileMutationQueue`. The property that actually needs guarding is the EDIT read-modify-write interleaving, which cyrup's atomic-rename write masks entirely.

**Impact** — The only tool-level guard on the mutation lock asserts nothing. The lock could be removed from both `write` and `edit` and the suite would stay green, while concurrent `edit`s silently lose changes.

**Fix** — Rewrite the concurrency half around `EditTool`, not `WriteTool`, so the read-modify-write is what races: seed a file with `"1\n2\n"`, fire two concurrent edits replacing different lines, assert both replacements survive. Add a second case constructing two separate `ToolRegistry` instances over the same path to pin TOOL-019.

**Verify** — The new test must fail with the `guard()` call removed from `edit.rs:189` and pass with it; the cross-registry case is red today. The lock primitive remains separately and correctly covered at `crates/cyrup-tools/src/lock.rs:78-109`.

## TOOL-027 — `grep`'s glob inherits `find`/fd's `**/` auto-prepend, which ripgrep applies in reverse

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/grep.rs:128-131` compiles the user's `glob` with the SHARED `PatternMatcher::build(g)`. `crates/cyrup-tools/src/tools/globmatch.rs:18-27`: `full_path = pattern.contains('/')`, and any slash-containing pattern that does not start with `/` or `**/` and is not exactly `**` is rewritten to `format!("**/{pattern}")`. That rule is fd's, declared as such in the module header (`globmatch.rs:1-4`, "Shared glob semantics for `find` and `grep`") and derived from `find.ts`. It is then applied as an include-filter against the search-root-relative path at `grep.rs:161-164`.

**upstream** — `pi/packages/coding-agent/src/core/tools/grep.ts:215` builds rg's argv as `["--json","--line-number","--color=never","--hidden"]` and `:218` `if (glob) args.push("--glob", glob);` — verbatim, with no `**/` prepend anywhere in the file. Verified in-workspace against the crate rg actually runs: `ignore-0.4.26/src/overrides.rs:142-144` routes `OverrideBuilder::add` into `GitignoreBuilder::add_line`, and `ignore-0.4.26/src/gitignore.rs:499-508` prepends `**/` ONLY when the pattern contains no literal slash. The two rules are exact opposites. Contrast `find.ts:243-252`, which prepends `**/` explicitly and only because fd's `--full-path` matches the ABSOLUTE candidate.

**Impact** — `grep(pattern=…, glob="src/**/*.ts")` — a call shape the schema's own description invites (`grep.rs:46`) — matches `vendor/foo/src/a.ts`, `node_modules/pkg/src/b.ts` and every other nested `src/`, while pi/rg match only the search root's own `src/`. On a monorepo that is an order of magnitude more hits, consuming the 100-match limit (`grep.rs:126`) on files the user excluded by construction. The divergence is silent — nothing in the output distinguishes the two selection rules.

**Fix** — Split the two semantics instead of sharing one `PatternMatcher`. Give `PatternMatcher::build` an explicit mode (`build_anchored` for grep, `build_fd` for find); grep compiles the pattern with NO `**/` prepend, `literal_separator(true)` when it contains `/`, matched against the search-root-relative posix path already computed at `grep.rs:155-160`. Keep `find.rs:102`'s call on the fd-shaped path. Land with TOOL-010 (same function); TOOL-011's absolute-path change must NOT be applied to the grep side.

**Verify** — Tree with `src/a.ts` and `vendor/src/b.ts` both containing the pattern; assert `grep(glob="src/**/*.ts")` returns only `src/a.ts`. Red today (returns both). Add the mirror case asserting `find(pattern="src/**/*.ts")` still matches at any depth, so the fd path is not regressed.

## TOOL-028 — `BashSpawnContext.env` is override-only, so pi's env-key deletion is unrepresentable through the spawn seam

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/config.rs:13-18` defines `BashSpawnContext { command, cwd, env: Vec<(String, String)> }`, documented at `:9-12` as the set of variable OVERRIDES layered on top of the inherited parent environment. `crates/cyrup-tools/src/tools/bash.rs:109-114` builds it from `shell_env(…)` and hands it to the optional hook. `crates/cyrup-tools/src/ops/local.rs:236-237` (`build_command`) and `:286-287` (`build_argv_command`) apply it as `for (k, v) in &spec.env { std_cmd.env(k, v); }` — `env_remove`/`env_clear` appear nowhere in `crates/cyrup-tools/src/`. A hook can add or replace a variable but never remove one the parent process exported.

**upstream** — `pi/packages/coding-agent/src/core/tools/bash.ts:164-183` `resolveSpawnContext` builds `const env = { ...getShellEnv() }` — the FULL materialized environment (`pi/packages/coding-agent/src/utils/shell.ts:122-134` returns `{ ...process.env, [pathKey]: updatedPath }`) — then performs five unconditional `delete env.PI_*` at `:166-170` before repopulating. `BashSpawnHook` (`bash.ts:156`) receives and returns that whole object, so an extension hook can delete keys too.

**Impact** — Two consequences. (1) It is the structural blocker for TOOL-008's scrub half: a cyrup process launched from a pi-flavoured or subagent parent leaks a stale `PI_SESSION_ID`/`PI_MODEL` into every `bash` child, and the fix cannot be written against today's seam. (2) An extension `spawnHook` that redacts a secret from the child env — a legitimate pi capability — is silently impossible; the variable is inherited regardless of what the hook returns, with no error. (2) is theoretical today: the only `spawn_hook` consumer in the workspace is a test (`crates/cyrup-tools/tests/tools.rs:1144-1156`); production never sets one.

**Fix** — Extend `BashSpawnContext` with an explicit removal channel — either `env_remove: Vec<String>` alongside the overrides, or switch `env` to a full materialized map plus an `inherit: bool` so a hook can return a complete environment the way pi's does. Apply removals via `std_cmd.env_remove(k)` before the overrides at `ops/local.rs:236` and `:286`. Ship in the same commit as TOOL-008 so the unconditional session-key scrub has somewhere to live.

**Verify** — Set a variable in the parent via the test harness, install a `spawn_hook` that removes it, run `bash 'echo ${VAR:-ABSENT}'`, assert `ABSENT`. Red today (echoes the value). Second case after TOOL-008: inject a stale `CYRUP_SESSION_ID` and assert the child sees it empty even with `expose_session_environment` off.

## TOOL-011 — `find` path-globs match the relative path; pi/fd match the absolute path

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** medium

**cyrup** — `crates/cyrup-tools/src/tools/find.rs:126-133` computes `rel = to_posix(w.path.strip_prefix(&search_root))` and calls `matcher.is_match(&rel, &basename)` at `:131`; `PatternMatcher::is_match` (`crates/cyrup-tools/src/tools/globmatch.rs:38-44`) tests `rel_posix` in full-path mode. The `**/` auto-prepend guard at `globmatch.rs:19-27` has a dead `pattern.starts_with('/')` arm, since a relative candidate can never begin with `/`.

**upstream** — `pi/packages/coding-agent/src/core/tools/find.ts:243-245` documents in-source that fd's `--full-path` "matches against the absolute candidate path, so a path-containing pattern like 'src/**/*.spec.ts' needs a leading '**/'"; the prepend block is `:246-252`; `find.ts:253` passes the absolute `searchPath` as fd's root and pi relativizes only for OUTPUT (`find.ts:307-320`).

**Impact** — Because both sides prepend `**/`, the two agree for the common case. The divergence is confined to (a) patterns naming an ancestor directory ABOVE the search root and (b) leading-`/` patterns such as `/src/**/*.ts`, which silently return an empty set in cyrup where fd would match.

**Fix** — Match against the absolute POSIX path in `find.rs:126-133` (keeping relativization for output only) and revisit `globmatch.rs:19-27` so the `starts_with('/')` arm becomes live.

**Verify** — Under a search root `<tmp>/repo`, assert `pattern="/src/**/*.ts"` returns the same set as `pattern="src/**/*.ts"`; red today. Confidence is medium because fd is not vendored in this workspace — fd's actual matching target is taken from pi's own in-source comment plus its argv construction. This is the only glob-family claim still resting on an external binary.

## TOOL-012 — Built-in registration order diverges from pi (`write`/`bash` swapped)

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/registry.rs:14` `BUILTIN_NAMES: [&str; 7] = ["read", "write", "edit", "bash", "grep", "find", "ls"]`, and the inserts at `:50-66` use the same order; `all()`/`visible()` walk `self.order`. `coding_tools()` (`:109-114`) filters that order through a `HashSet` allowlist at `:112`, so it yields read, write, edit, bash.

**upstream** — `pi/packages/coding-agent/src/core/tools/index.ts:156-166` `createAllToolDefinitions` returns `{read, bash, edit, write, grep, find, ls}`; `:138-145` `createCodingToolDefinitions` returns `[read, bash, edit, write]`.

**Impact** — The order propagates into the system prompt: `crates/cyrup-session-svc/src/builder.rs:936-937` maps over `base_tools` in registry order into `tool_contributions`, which `crates/cyrup-session/src/prompt/builder.rs:166-180` (snippets) and `:200-208` (guidelines) emit in vector order. Prompt text therefore differs from pi's byte-for-byte, defeating any golden prompt comparison and perturbing cache prefixes.

**Fix** — Reorder the inserts at `registry.rs:50-66`, `BUILTIN_NAMES` at `:14`, and the allowlist at `:112` to read, bash, edit, write, grep, find, ls. The adjacent divergence — cyrup activating all seven where pi's default active set is `["read","bash","edit","write"]` — belongs to the cyrup-session-svc owner.

**Verify** — Assert `BUILTIN_NAMES` and `registry.all()` order match pi's; add a prompt snapshot showing the tool section in pi order.

## TOOL-013 — `read`'s missing/unreadable error replaces pi's Node errno text

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/read.rs:95-100`: `if self.fs.access(&abs, crate::ops::Access::Read).await.is_err() { return Err(error::not_found(format!("File not found or unreadable: {}", input.path))); }` — one message for every failure mode, echoing the raw user-supplied path and DISCARDING the errno `LocalFs::access` already produced at `crates/cyrup-tools/src/ops/local.rs:104` (`error::io(&error::show(path), &std::io::Error::last_os_error())`, inside the fn spanning `:84-115`).

**upstream** — `pi/packages/coding-agent/src/core/tools/read.ts:241` `await ops.access(absolutePath)` with no local catch (the surrounding try at `:237` rejects with the raw error), so Node's `ENOENT: no such file or directory, access '<abs>'` / `EACCES: permission denied, access '<abs>'` reaches the model verbatim.

**Impact** — The model cannot distinguish "file does not exist" from "file exists but is unreadable", so it retries a permission failure as if it were a typo, and gets a relative path back where pi returns the absolute one.

**Fix** — Propagate the `ToolError` from `FsOps::access` and format pi-shaped `<ERRNO>: <text>, access '<abs>'`. The errno→name mapping is shared with TOOL-014 — implement it once in `ops/local.rs`.

**Verify** — Assert the message for a missing file contains `ENOENT` and the absolute path, and for a `chmod 000` file contains `EACCES`. `read_missing_file_errors` (`crates/cyrup-tools/tests/tools.rs:119-128`) asserts `contains("not found") || contains("unreadable")` at `:127` and would NOT be satisfied by the pi-shaped message — update it in the same commit.

## TOOL-014 — `edit`'s access-failure body diverges from pi's `Error code: <ERRNO>`; the in-source comment is wrong

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/edit.rs:194-196` formats `Could not edit file: {}. {e}.` where `e` is the whole `ToolError` from `FsOps::access` (`"<path>: <io::Error Display>"`, `crates/cyrup-tools/src/error.rs:22-24`). The comment at `edit.rs:192-193` asserts that "The `${errorMessage}` body itself (a Node errno string) is irreducible" — factually wrong about upstream.

**upstream** — `pi/packages/coding-agent/src/core/tools/edit.ts:323-330`: `try { await ops.access(absolutePath); } catch (error: unknown) { throwIfAborted(); const errorMessage = error instanceof Error && "code" in error ? \`Error code: ${error.code}\` : String(error); throw new Error(\`Could not edit file: ${path}. ${errorMessage}.\`); }` — the throw is at `:329`, the bare `Error code: EACCES` form, never the full Node message. Access mode is identical on both sides: pi `constants.R_OK | constants.W_OK` at `edit.ts:86`; cyrup `Access::ReadWrite => libc::R_OK | libc::W_OK` at `ops/local.rs:98`.

**Impact** — A read-only or missing file yields a Rust-flavoured body instead of `Error code: EACCES` / `Error code: ENOENT`. Models trained on the pi phrasing lose the machine-readable errno token, and the wrong comment will cause the next reader to close this item as unfixable.

**Fix** — Map the `ToolError` to an errno name (shared with TOOL-013) and emit `Could not edit file: {path}. Error code: {ERRNO}.`; DELETE the incorrect comment at `edit.rs:192-193`.

**Verify** — `edit_access_error_has_trailing_period` (`crates/cyrup-tools/tests/tools.rs:1432-1448`) checks only the `starts_with("Could not edit file: missing.txt. ")` prefix at `:1446` and the trailing period at `:1447`, so it survives unchanged; add an assertion for the `Error code: ENOENT` body.

## TOOL-016 — `constrainedSampling` has no representation in cyrup's tool model

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `grep -rni constrained crates/ --include=*.rs --include=*.wit` returns exactly one hit workspace-wide, an unrelated comment at `crates/cyrup-ext/src/host/engine.rs:30`. `cyrup_core::Tool` (`crates/cyrup-core/src/tool.rs:89-159`, read in full) has slots for execution_mode / description / label / prompt_snippet / prompt_guidelines / render_kind / prepare_arguments / render_call / render_result / execute — none for constrained sampling. Neither WIT `tool-descriptor` record has a field (`crates/cyrup-ext/wit/world.wit:31-40` and `crates/cyrup-ext-sdk/wit/world.wit:31-40`, byte-identical 8-field records).

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:463` `constrainedSampling?: false | ConstrainedSamplingConfig`, plumbed at `tool-definition-wrapper.ts:14,42`. Introduced by pi `24bace27` on 2026-07-23, thirteen days after cyrup HEAD's baseline.

**Impact** — Zero built-in behavior gap today — no pi built-in sets `constrainedSampling`. It becomes real only when an extension wants grammar-constrained tool output, which cyrup then cannot express at all.

**Fix** — Add the config type to `cyrup-core/src/tool.rs`, a `constrained-sampling` field to BOTH WIT copies and both descriptors, and honour it where tool schemas reach the provider (`crates/cyrup-agent/src/agent.rs:651-659`).

**Verify** — A guest tool declaring a constrained-sampling config; assert the config appears in the emitted `ToolDef` reaching the provider. Correctly deprioritized until an extension needs it.

## TOOL-017 — `read`'s compact call rendering (SKILL.md / docs / AGENTS.md) not ported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-tui/src/transcript.rs:1041-1066` `render_read` unconditionally emits `read <path>[:range]` (`:1042-1047`) with no classification of the target and no expand affordance; the collapsed flag only caps how many body lines follow (`:1056` `total.min(10)`). `grep -rn 'SKILL.md' crates/cyrup-tui/src/` finds only skills-selector and config-selector code, never a read-call classification.

**upstream** — `pi/packages/coding-agent/src/core/tools/read.ts:117` `getCompactReadClassification`, `:140` `formatCompactReadCall`, selected at `:329-334` inside `renderCall` by `const classification = !context.expanded ? getCompactReadClassification(args, context.cwd) : undefined;`.

**Impact** — Collapsed transcripts show a raw path where pi shows a semantic label (skill, docs, AGENTS.md), making skill and instruction loads indistinguishable from ordinary file reads in a long scrollback. Cosmetic only.

**Fix** — OWNER IS cyrup-tui, not cyrup-tools — routed out of this area but retained here for traceability. Port `getCompactReadClassification`/`formatCompactReadCall` next to `render_read`, keyed off the collapsed flag. Fold in the post-baseline drift while there: pi `a2c5ee33` (2026-07-17) changed `formatReadResult` to stop highlighting read errors.

**Verify** — TUI snapshot over a collapsed `read` of `.cyrup/skills/foo/SKILL.md` showing the classified label, and of an ordinary source file showing the plain path.

## TOOL-018 — `edit` fuzzy matcher returns not-found where pi returns duplicate-occurrences

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/edit_diff.rs:298-303` guards with `match (fuzzy_old.is_empty(), fuzzy_content.find(&fuzzy_old))`, falling to `FuzzyMatch { found: false, .. }` when the normalized needle is empty (`:296-297` are the two `normalize_for_fuzzy` calls). `count_occurrences` (`:307-314`) returns 0 for an empty normalized needle. Neither carries a `[CYRUP-DELTA]`.

**upstream** — `pi/packages/coding-agent/src/core/tools/edit-diff.ts:222` `const fuzzyIndex = fuzzyContent.indexOf(fuzzyOldText);` — `indexOf("")` returns 0 (found); `countOccurrences` at `:250-254` is `fuzzyContent.split(fuzzyOldText).length - 1`, far above 1 for an empty needle, so pi raises the DUPLICATE-occurrences error.

**Impact** — Both sides reject a literally-empty `oldText` up front, so this is reachable only when `oldText` is non-empty but NORMALIZES to empty — i.e. entirely trailing whitespace. There cyrup returns `Could not find the exact text in {path}…` where pi returns the duplicate-occurrences error, giving the model different remediation advice in a rare case.

**Fix** — The cheapest resolution is documentation, not code: keep the guards and mark them an intentional `[CYRUP-DELTA]` in the `edit_diff.rs` module comment, naming the reachability condition, so this is not re-filed a fourth time. Otherwise mirror pi's semantics exactly at `:298-314`.

**Verify** — `edit` with `oldText = "   "` on a file containing whitespace: assert the chosen error (documented delta, or the pi-shaped duplicate message).

## TOOL-026 — `bash_timeout_fractional_seconds` asserts a wall-clock upper bound it cannot control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** medium (analytic)

**cyrup** — `crates/cyrup-tools/tests/tools.rs:752-774`, read in full. Runs `sleep 30` with `timeout: 2.5` and asserts at `:769-772` `elapsed >= Duration::from_millis(2300) && elapsed < Duration::from_millis(4000)`. The lower bound is safe. The upper bound leaves only ~1.5s of slack for scheduling, the SIGTERM→grace→SIGKILL escalation and process reaping, under an arbitrarily loaded `cargo test --workspace`. The load-independent part — `assert!(msg.contains("Command timed out after 2.5 seconds"))` at `:768` — is what actually pins the float-seconds parsing (`resolve_timeout_ms`, `crates/cyrup-tools/src/tools/bash.rs:36-47`) and needs no timing at all.

**upstream** — Not a parity question: the ported behavior is correct (`pi/packages/coding-agent/src/core/tools/bash.ts:42` `Type.Number` timeout, message at `:414-415`). Same class as commit `1806375` and TOOL-020.

**Impact** — A low-probability intermittent failure in the repo's only gate. Materially less likely to trip than TOOL-020 (1.5s slack versus ~15ms), which is why it is low — do not let it displace TOOL-020 or TOOL-024/025 in priority.

**Fix** — Keep the message assertion and the `>= 2300ms` lower bound (which proves the 2.5s value was honoured rather than a default), and either drop the upper bound or widen it to something no realistic load can exceed (e.g. 15s, still far below the 30s sleep, so it still proves the timeout fired).

**Verify** — Run the file under `--test-threads=32` alongside a loaded workspace build; analytic only this pass, since cargo was forbidden.

## TOOL-029 — `ls` swallows pi's `Cannot read directory: <message>` and emits the raw io-error wrapper instead

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/ls.rs:83` is `let mut entries = self.fs.read_dir(&abs).await?;` — the `?` propagates whatever `FsOps::read_dir` produced. For `LocalFs` (`crates/cyrup-tools/src/ops/local.rs:135-151`) both failure arms are `error::io(&error::show(path), &e)`, which formats as `"<path>: <io::Error Display>"` (`crates/cyrup-tools/src/error.rs:22-24`). Every other failure mode in the same function is carefully pi-shaped: `Path not found: {}` at `ls.rs:78`, `Not a directory: {}` at `:80`. The readdir branch is the only one that is not.

**upstream** — `pi/packages/coding-agent/src/core/tools/ls.ts:141-147`: `try { entries = await ops.readdir(dirPath); } catch (e: any) { reject(new Error(\`Cannot read directory: ${e.message}\`)); return; }` — the reject is at `:145`, a distinct stable prefix the model can pattern-match on, separate from `Path not found:` and `Not a directory:`.

**Impact** — An `ls` of a directory that exists and is a directory but cannot be enumerated (mode `0300`, an unmounted/EIO path, a permissions-stripped `.git/objects`) returns a Rust-flavoured message with no `Cannot read directory:` prefix. The model reads it as an unclassified failure rather than a permissions problem, and it is one more place a golden transcript comparison against pi diverges for a trivially fixable reason. Narrow reachability keeps this low.

**Fix** — Wrap the call at `ls.rs:83`: `self.fs.read_dir(&abs).await.map_err(|e| error::invalid(format!("Cannot read directory: {e}")))?`, matching the surrounding style — the other two branches already `map_err` into pi-shaped literals. If TOOL-013's errno mapping lands first, reuse it so `{e}` renders Node-shaped rather than as the Rust wrapper.

**Verify** — Create a directory, `chmod 0300` it (readdir denied, traversal allowed), run `LsTool`, assert the error contains `Cannot read directory:` and not the bare path-prefixed form. Skip on non-unix and when running as root, where the mode is not enforced.

## TOOL-030 — `exec_pre_cancelled_never_spawns` carries a 200ms wall-clock upper bound it does not control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** medium (analytic)

**cyrup** — `crates/cyrup-tools/src/ops/local.rs:794-817`. The attribute at `:794` is a plain `#[tokio::test]` (single-threaded current-thread runtime), so the test task shares one thread with the runtime — if anything more exposed, not less. After asserting the pre-cancelled `exec` returns `ExitStatus::Killed` (`:805`), it asserts at `:806-810` `started.elapsed() < Duration::from_millis(200)` with the message "must short-circuit before spawning, not pay real process start/teardown latency" — a wall-clock ceiling on work the test cannot schedule, in a suite with no CI and an arbitrarily loaded `cargo test --workspace`.

**upstream** — Not a parity question: the ported behavior (pi's pre-spawn `signal?.aborted` check, `pi/packages/coding-agent/src/core/tools/bash.ts:86-88`) is correct and cyrup's `exec` mirrors it. Filed purely under the defect class the project keeps finding — `1806375` deleted an unassertable scheduling outcome from `crates/cyrup-ext/src/caps/proc.rs`, and `9b3afd7`'s message records that a second unrelated intermittent test remains somewhere in the workspace.

**Impact** — A low-probability intermittent failure in the repo's only gate, and one more candidate for `9b3afd7`'s unfound second flake. Kept low deliberately (200ms for two awaits is far more slack than TOOL-020's ~15ms for a fork+exec).

**Fix** — Prove no-spawn deterministically rather than by latency. Note that simply deleting the timing assertion and keeping `!marker.exists()` is NOT a sufficient substitute: a run that DID spawn `sh -c 'touch …'` and was killed before the `touch` completed also leaves the marker absent, so that would strictly weaken the test. Instead (a) instrument the path — have `exec` return a distinguishable pre-spawn short-circuit the test can assert on, or use a `ProcOps` test double that records spawn attempts and assert zero; or (b) if a latency guard is still wanted alongside, widen it to a figure no realistic load can reach (e.g. 5s — still below the 5s kill grace this `LocalProc` was built with at `:795`, and matching the ~3s-of-slack margin the defensible sibling bounds at `local.rs:757`/`:782`/`:844` use).

**Verify** — With the test double in place the no-spawn invariant is assertable with zero timing. Until then, only running the suite under artificial load can observe whether the 200ms bound trips; analytic only, since cargo was forbidden this pass.

## Coverage

Read at HEAD `1806375` (tree clean): all seven tool sources under `crates/cyrup-tools/src/tools/`, plus `registry.rs`, `config.rs`, `lock.rs`, `error.rs`, `globmatch.rs`, `edit_diff.rs`, `isolation/{mod,protected}.rs`, `ops/{local,shell,mod}.rs` including their inline `#[cfg(test)]` blocks, and `crates/cyrup-tools/tests/{tools,pi_schema,isolation}.rs`. Off-crate tool surface: `crates/cyrup-core/src/tool.rs`, `crates/cyrup-ext/src/{registry,wrapper}.rs` and `src/host/live.rs`, both `wit/world.wit` copies, `crates/cyrup-ext-sdk/src/descriptor.rs`, and the tool-facing parts of `crates/cyrup-session-svc/src/builder.rs` and `crates/cyrup-session/src/prompt/builder.rs`. Upstream: all seven `pi/packages/coding-agent/src/core/tools/*.ts` plus `index.ts`, `edit-diff.ts`, `file-mutation-queue.ts`, `exec.ts`, `tool-definition-wrapper.ts`, `extensions/types.ts`, `extensions/wrapper.ts` and `utils/shell.ts`.

Two external-tool claims were resolved in-workspace rather than from documentation: ripgrep's `--glob` semantics — both the `**/`-prepend rule and `!` negation — verified against the vendored `ignore-0.4.26` (`src/overrides.rs:142-144`, `src/gitignore.rs:475-478` and `:499-508`), and grep-searcher's binary-quit mechanism against `grep-searcher-0.1.16/src/searcher/{glue,core}.rs`. That same read settled a question left open by the previous pass — whether rg applies override globs to an explicitly-named path — with a definitive no (`ignore-0.4.26/src/walk.rs:1057-1060`: `skip_entry` returns `Ok(false)` unconditionally at depth 0), matching cyrup's behavior at `grep.rs:135-141`. No item needed; do not re-open it next pass.

Blind spots and things taken on trust. (1) No tests were run — cargo was forbidden — so TOOL-020, TOOL-026, TOOL-030 and every "red today" claim about a proposed test are ANALYTIC: high-confidence defective design, not confirmed failures. (2) fd is not vendored, so TOOL-011 and TOOL-023 rest on pi's own in-source comment at `find.ts:243-245` plus its argv construction; that is the only remaining claim in this area resting on an external binary. (3) `spec/` is absent from this workspace; `R-NN-NNN` ids were used only as a grep index, and no requirement text is quoted or inferred. (4) Not re-audited, unchanged since the c8bd2ab pass and untouched by any of the 28 commits: `crates/cyrup-tools/src/{truncate,output,path,details}.rs`, `isolation/{policy,sandbox,traversal}.rs`, and the image resize/encode path in `read.rs:297-501` beyond the `supports_images` gate. (5) Debt-mining: of the 28 commits only `9ccc8ff`, `67bf079` and `f777e44` touch this area. `9ccc8ff`'s sole deferral ("extension tools get snippets but not guidelines") is fully captured as TOOL-021; `67bf079` defers nothing; `f777e44`'s deferrals (WIT ABI break, turn-boundary tool refresh, `getUsageCostBreakdown` bucket) land in cyrup-agent/cyrup-ext, though its ABI-break precedent is folded into TOOL-022's fix cost. No unrecorded tools-area debt was found in the commit messages. (6) Defect-class sweep: `elapsed()` has nine assertion sites in this crate. `local.rs:935` and `:965` are lower bounds (safe); `:757`, `:782`, `:844` are `< 2s` ceilings guarding ~100-200ms paths against a 5s grace alternative (~3s slack) and are load-bearing, NOT defects — do not sweep them into TOOL-020's fix; same verdict for `:1146`. Only `:807` and `tests/tools.rs:770-771` are genuinely tight. No test in this area was found PINNING outright wrong behavior (`pi_schema.rs:182-188` pins bash's empty guideline set, but that value is correct-for-cyrup-today and the deferral is recorded under TOOL-008); three give FALSE ASSURANCE without pinning anything wrong — TOOL-009's, TOOL-024 and TOOL-025.


---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at
all, rather than checking a list of known items. That inversion exists because the
item-driven method missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`)
— a real, user-reported bug — and by construction cannot see behaviour nobody wrote an item
for. IDs use an `-SNN` suffix to mark their provenance.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| TOOL-S01 | medium | not-ported | S | Six of seven built-ins model numeric parameters as `usize`, so pi-legal JSON floats/negatives hard-error at deserialization instead of being coerced |
| TOOL-S02 | low | not-ported | S | Tool-result text is rendered with no `stripAnsi` and no `sanitizeBinaryOutput` — escape sequences survive as literal garbage in the transcript |
| TOOL-S03 | low | not-ported | M | `computeEditsDiff` — pi's pre-execution edit preview — has no counterpart; cyrup renders a diff only after the write has landed |
| TOOL-S04 | low | not-ported | S | `images.autoResize` is a live toggle in cyrup's settings UI with no consumer — `read` always downsizes to 2000px |
| TOOL-S05 | low | not-ported | S | `bash` shows no live elapsed timer — pi ticks `Elapsed 12.3s` while running and only switches to `Took` on settle |
| TOOL-S06 | low | not-ported | S | `grep` with context silently drops a file it cannot read; pi emits an explicit `(unable to read file)` marker |

## TOOL-S01 — Six of seven built-ins model numeric parameters as `usize`, so pi-legal JSON floats/negatives hard-error at deserialization instead of being coerced

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — All numeric tool params are TypeBox `Type.Number`, no `minimum`, no integer constraint: read.ts:22-23, ls.ts:16, find.ts:25, grep.ts:33-35, bash.ts:42. JS coerces rather than rejects — read.ts:271 `Math.max(0, offset - 1)`, grep.ts:188-189 `context > 0 ? context : 0` / `Math.max(1, limit ?? DEFAULT)`, ls.ts:125 and find.ts:151 take the value unvalidated. No pi built-in can fail on a numeric argument's shape.

**cyrup** — ABSENT. 

**Impact** — `{"path":"f.rs","limit":100.0}` or `{"pattern":"x","context":2.0}` — legal JSON the tool's own schema invites, and which several providers emit when a number round-trips through a float — returns `read: invalid type: floating point \`100.0\`, expected usize` where pi returns the file; `offset: -1` errors where pi reads from line 1. Wasted turn plus a retry, with a raw serde message as the only guidance. Not tracked in 04-cyrup-tools.md (TOOL-026 concerns the bash timeout *test*, not the other params).

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## TOOL-S02 — Tool-result text is rendered with no `stripAnsi` and no `sanitizeBinaryOutput` — escape sequences survive as literal garbage in the transcript

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/tools/render-utils.ts:39-64 `getTextOutput`, line 48: `sanitizeBinaryOutput(stripAnsi(c.text || "")).replace(/\r/g,"")`. Called from bash.ts:248, read.ts:178, ls.ts:71, find.ts:85, grep.ts:97. `sanitizeBinaryOutput` = pi/packages/coding-agent/src/utils/shell.ts:144-174.

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## TOOL-S03 — `computeEditsDiff` — pi's pre-execution edit preview — has no counterpart; cyrup renders a diff only after the write has landed

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/tools/edit-diff.ts:518-547 `computeEditsDiff` (resolve → access(R_OK) → read → strip BOM → normalize LF → real `applyEditsToNormalizedContent` → `generateDiffString`, or a `{error}` shaped like the execute-time failure). Fired from edit.ts:377-386 inside `renderCall` gated on `context.argsComplete`, driving `buildEditCallComponent` (edit.ts:246-265) and `getEditHeaderBg` (edit.ts:229-244); `formatEditResult` (edit.ts:200-227, esp. :222) suppresses the post-hoc diff when it equals the preview.

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## TOOL-S04 — `images.autoResize` is a live toggle in cyrup's settings UI with no consumer — `read` always downsizes to 2000px

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/tools/read.ts:59-63 `ReadToolOptions.autoResizeImages`, read at :207, passed at :250 into `processImage(buffer, mimeType, { autoResizeImages })`; pi/packages/coding-agent/src/utils/image-process.ts:77 reads the flag and :86-116 forks — when false it returns the original bytes base64 at :112. Wired at pi/packages/coding-agent/src/core/agent-session.ts:2553/:2564 from settings-manager.ts:1150 (`this.settings.images?.autoResize ?? true`).

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## TOOL-S05 — `bash` shows no live elapsed timer — pi ticks `Elapsed 12.3s` while running and only switches to `Took` on settle

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/tools/bash.ts:309-313 (`const label = options.isPartial ? "Elapsed" : "Took"`, footer present from call start), `startedAt` stamped in `renderCall` at :461-464, per-second repaint via `setInterval(() => context.invalidate(), 1000)` at :471-473, torn down at :474-480.

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## TOOL-S06 — `grep` with context silently drops a file it cannot read; pi emits an explicit `(unable to read file)` marker

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/tools/grep.ts:250-253 inside `formatBlock`: `const lines = await getFileLines(filePath); if (!lines.length) return [\`${relativePath}:${lineNumber}: (unable to read file)\`];`; `getFileLines` (:201-213) swallows the read error into `lines = []` at :207-209.

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

