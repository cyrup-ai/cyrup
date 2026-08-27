---
stage: new
status: done
updated: 2026-08-27 05:30
severity: high
effort: small
subsystem: exec / deadlines
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-077
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-077 — A foreground subagent run with no explicit timeout has NO wall-clock deadline, and there is no global `config.timeoutMs`

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** foreground execution / deadlines
**Window** in-baseline (≤ v0.43.0) for the 30-minute foreground default; **v0.47.1..v0.57.0** for the `config.timeoutMs` rung.

**upstream** — `git show v0.57.0:src/runs/foreground/subagent-executor.ts` **`:2656`**
`export const DEFAULT_FOREGROUND_TIMEOUT_MS = 30 * 60 * 1000;`; **`:2684`**
`resolveConfigDefaultTimeoutMs` validates `config.timeoutMs` as a positive integer; **`:2719`**
`resolveSingleAgentLaunchTimeout(params, async, configDefaultTimeoutMs)` computes **`:2721`**
`const foregroundDefault = configDefaultTimeoutMs ?? DEFAULT_FOREGROUND_TIMEOUT_MS` and applies it to
every non-async launch. Its `!async` arm does not test `isComposite`, so the backstop applies to
single, `tasks: []` and chain launches alike at the one shared call site **`:5914-5917`**; the same
resolution appears again at **`:4440`**. The `config.timeoutMs` rung is a window addition, and commit
`0ed0afee` states the defect it fixes verbatim: agent frontmatter `timeoutMs` was applied only to
single-agent launches, so parallel and chain launches "never adopt it and fall back to the built-in
30-minute foreground default … with no global knob to raise the default."

**cyrup** — `crates/cyrup-ext-subagents/src/extension/tool/params.rs:264-280`
`resolve_foreground_timeout` validates `0` and the `timeoutMs`/`maxRuntimeMs` alias mismatch and then
returns `Ok(p.timeout_ms.or(p.max_runtime_ms))` — **no default at all**. Its caller
`src/extension/tool/routing.rs:370-372` does `resolve_foreground_timeout(p)…?.or(launch_defaults.1)`,
where `launch_defaults.1` is only the agent's own frontmatter `timeoutMs`
(`src/extension/executor/nested_control.rs:148-172`).
`grep -rn '1_800_000\|1800000\|30 \* 60' --include=*.rs` hits only `src/background/wait.rs:86`,
`src/background/mod.rs:43,57` (`DEFAULT_ASYNC_CHILD_TIMEOUT_MS`) and `src/extension/wait_tool.rs:65`
— the async side has its default, the foreground side has none. `SubagentExtensionConfig`
(`src/registration/mod.rs:79-245`) has no `timeout_ms` field; grepping `timeout_ms` in that file
returns only `worktree_setup_hook_timeout_ms`.

**Impact** — `subagent({agent:"x", task:"…"})` run in the foreground against an agent with no
frontmatter `timeoutMs` has **no wall-clock deadline** in cyrup: a child whose bash tool blocks
forever hangs the orchestrator's turn indefinitely with no signal, where upstream terminates it at 30
minutes with `Subagent timed out after 1800000ms.` Separately, an operator who sets
`subagents.timeoutMs` gets nothing — upstream uses it to replace the backstop for single, parallel,
chain and plain single-agent async launches alike, which is the only way to raise a long fan-out's
ceiling without passing `timeoutMs` on every call. `high` not `critical`: an unbounded hang is none
of the four `critical` conditions — but it sits at the top of `high`, because the failure is silent
and open-ended.

**Fix** — Give `resolve_foreground_timeout` a `config_default: Option<u64>` parameter and have it
return `p.timeout_ms.or(p.max_runtime_ms).or(agent_default).or(config_default).or(Some(DEFAULT_FOREGROUND_TIMEOUT_MS))`
for every non-async launch, mirroring `:2719-2725`'s precedence, and add `timeout_ms` to
`SubagentExtensionConfig` with upstream's positive-integer validation. Apply it at **all** foreground
call sites in `routing.rs`, not just the single-agent one — the parallel path drops an explicit
`timeoutMs` today.

**Verify** — A foreground agent with no frontmatter `timeoutMs` whose child sleeps must be terminated
at the default with pi's message; `subagents.timeoutMs: 60000` must replace that default on single,
`tasks: []` and chain launches; an explicit call-site `timeoutMs` must still win over both.

**Relation to corpus** — **REVISION-adjacent to `SUBA-051`**, which covers the ASYNC child default
and whose Fix line explicitly instructs *"Do not apply it to foreground runs, which already have
their own default."* **That instruction is wrong at HEAD and following it would leave the foreground
path unbounded forever.** Distinct item because the fix site (`extension/tool/params.rs` + a new
config key) differs from `SUBA-051`'s (`background` step construction).

---

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-077](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

Add the missing 30-minute foreground default deadline and the `config.timeoutMs` rung, matching the
upstream ladder. Note SUBA-051's claim that foreground already has its own default is false — correct
that row when this lands.

## Acceptance Criteria

- [ ] A foreground run with no explicit timeout is bounded by the 30-minute default
- [ ] `config.timeoutMs` is parsed and applied at the right precedence
- [ ] A test asserts the ladder order
- [ ] `cargo test -p cyrup-ext-subagents` passes
