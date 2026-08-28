---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:236"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 02:09
---

# Capability gap: `crates/cyrup-tools/src/tools/bash.rs:236`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

The bash and powershell prompt guideline reads `"You can inspect PI_* environment variables for current model and session details."` (bash.ts:49, powershell.ts:20 @e8682309), and the tool actually injects `PI_SESSION_ID` / `PI_SESSION_FILE` / `PI_PROVIDER` / `PI_MODEL` / `PI_REASONING_LEVEL` into the child (bash.ts:171-181).

## What cyrup does

Emits `"You can inspect CYRUP_* environment variables ..."` and injects `CYRUP_SESSION_ID` / `CYRUP_SESSION_FILE` / `CYRUP_PROVIDER` / `CYRUP_MODEL` / `CYRUP_REASONING_LEVEL`, while `config::session_env_scrub_keys()` DELETES the five `PI_*` names from every child unconditionally.

## What a caller sees

System-prompt text differs, and — more consequentially — any user script, hook, or `.bashrc` that reads `PI_SESSION_ID` (or the other four) gets nothing under cyrup; the variables are actively scrubbed, not merely absent. A pi user's existing shell tooling silently stops working. Deliberate and self-consistent, but squarely caller-visible.

---

# AUGMENTATION — 2026-08-28

Reference tree `./tmp/pi` pinned at **`e8682309`** (`packages/coding-agent/package.json:3` →
`"version": "0.84.3"`). Every upstream line number below is the number **at that commit**, read out
of the working tree, not carried from the ledger. Where a cyrup source comment cites a different
number, that is recorded in §7 as citation drift rather than silently corrected.

Scope discipline. This row is about **the variable-family NAME** — the `PI_*` → `CYRUP_*` token.
Two siblings own the neighbouring deltas in the same file and are cross-referenced, not re-decided
here:

* `MEDIUM-delta-cyrup-tools-src-tools-bash-rs-214.md` — the `"You can inspect "` **wording**
  (v0.84.x-ahead-of-baseline). At `e8682309` cyrup and pi **agree** on the wording.
* `MEDIUM-delta-cyrup-tools-src-tools-bash-rs-312.md` — `PI_CODING_AGENT` / `AI_AGENT`, the
  identity markers, which are **not** session keys and sit outside the exposure gate.

---

## 1. Exactly what pi exports @`e8682309` — names, values, conditions

The whole mechanism is one function: **`resolveSpawnContext`**,
`packages/coding-agent/src/core/tools/bash.ts:170-196`. It is called from the tool's `execute` at
`bash.ts:363`:

```ts
const spawnContext = resolveSpawnContext(resolvedCommand, cwd, spawnHook, exposeSessionEnvironment, ctx);
```

Verbatim body, `bash.ts:177-195`:

```ts
const env = { ...getShellEnv() };                                    // :177
delete env.PI_SESSION_ID;                                            // :178
delete env.PI_SESSION_FILE;                                          // :179
delete env.PI_PROVIDER;                                              // :180
delete env.PI_MODEL;                                                 // :181
delete env.PI_REASONING_LEVEL;                                       // :182
if (exposeSessionEnvironment && ctx) {                               // :183
    const model = ctx.model;                                         // :184
    env.PI_SESSION_ID = ctx.sessionManager.getSessionId();           // :185
    const sessionFile = ctx.sessionManager.getSessionFile();         // :186
    if (sessionFile) env.PI_SESSION_FILE = sessionFile;              // :187
    if (model) {                                                     // :188
        env.PI_PROVIDER = model.provider;                            // :189
        env.PI_MODEL = model.id;                                     // :190
    }                                                                // :191
    if (ctx.thinkingLevel) env.PI_REASONING_LEVEL = ctx.thinkingLevel; // :192
}                                                                    // :193
const baseContext: BashSpawnContext = { command, cwd, env };         // :194
return spawnHook ? spawnHook(baseContext) : baseContext;             // :195
```

### 1.1 The enumerated surface

| # | Name | Value expression | Condition it is SET under | Condition it is UNSET under |
|---|---|---|---|---|
| 1 | `PI_SESSION_ID` | `ctx.sessionManager.getSessionId()` (`:185`) | `exposeSessionEnvironment && ctx` — **unguarded inside the branch** | flag off, or no `ctx` |
| 2 | `PI_SESSION_FILE` | `ctx.sessionManager.getSessionFile()` (`:186-187`) | branch taken **and** the value is truthy | branch not taken, **or** the session is ephemeral (falsy path) |
| 3 | `PI_PROVIDER` | `model.provider` (`:189`) | branch taken **and** `ctx.model` truthy | branch not taken, or no model selected |
| 4 | `PI_MODEL` | `model.id` (`:190`) | same `if (model)` block as #3 — the **pair** is set together or not at all | as #3 |
| 5 | `PI_REASONING_LEVEL` | `ctx.thinkingLevel` (`:192`) | branch taken **and** `ctx.thinkingLevel` truthy | branch not taken, or the level is falsy |

Four properties of the mechanism that any port has to preserve, all visible in the block above:

* **The five deletes at `:178-182` are UNCONDITIONAL** — they run before `exposeSessionEnvironment`
  is consulted at `:183`. A stale value inherited from a parent process can never survive, with the
  flag on or off. This is the anti-staleness guarantee, and it is separate from the injection.
* **Delete-then-set ordering** (`:178-182` then `:185-192`): a name in both lists ends up **set**.
* **The spawn hook runs LAST** (`:195`), so a hook observes the fully-populated `env`
  (`docs/extensions.md:2140`: "Injection happens before `spawnHook`, so hooks receive these values
  in `env`").
* **The flag defaults ON** — `bash.ts:345`: `const exposeSessionEnvironment = options?.exposeSessionEnvironment ?? true;`
  declared at `bash.ts:205-206` as `/** Expose current Pi session metadata as PI_* environment variables. Default: true */`.

### 1.2 Where else pi names the same five

* `packages/coding-agent/src/core/tools/powershell.ts:20` — `powershellToolSystemPromptContribution.guidelines`
  is the **byte-identical** sentence to bash's (`bash.ts:47-50`, `bashToolSystemPromptContribution`).
  PowerShell reaches the same `resolveSpawnContext` through the shared `createShellToolDefinition`.
* `packages/coding-agent/docs/environment-variables.md:20-49` — the user-facing contract. The table
  is `:26-30`; `:32` pins the timing ("The values are resolved when each command starts…"); `:49`
  pins the **negative**: "These variables are injected into the LLM-callable `bash` and `powershell`
  tools. **They are not injected into user-entered `!` or `!!` commands.**"
* `packages/coding-agent/README.md:690-694` — the same table.
* `packages/coding-agent/docs/extensions.md:2140` — the `spawnHook` ordering guarantee.
* `packages/coding-agent/CHANGELOG.md:470` — the release note that introduced the five.
* `packages/coding-agent/src/server/create-harness.ts:114-118` — a **second, different** seam (the
  SDK harness `prepare` callback) which sets the same five, with one divergence from the tool path:
  `execution.env.PI_SESSION_FILE = sessionFile ?? ""` sets it to the **empty string** rather than
  leaving it unset. Pinned upstream at `test/server/create-harness.test.ts:204-205`
  (`Object.hasOwn(..., "PI_SESSION_FILE")` is `true`, value `""`). **cyrup has no port of
  `create-harness.ts`** — `grep -rn 'create_harness' crates/ --include=*.rs` returns only unrelated
  identifiers — so this seam is out of scope here; it is noted so a future SDK-surface port does not
  read the tool path's `unset` as the only correct answer.

Upstream's own coverage of the five, for reference when writing cyrup's:
`test/agent-session-dynamic-tools.test.ts:81-94` (set when on, all five absent when opted out),
`test/sdk-session-manager.test.ts:114`, `test/server/create-harness.test.ts:137-205`,
`packages/agent/test/harness/nodejs-env.test.ts:294-314`.

**Unrelated use of two of the names, so nobody conflates them:** `packages/evals/src/pi-harness.ts:48-53`
reads `PI_PROVIDER` / `PI_MODEL` as **inputs** to select an eval harness model
(`packages/evals/README.md:18`). That is a different package with a different direction of flow and
has no bearing on this row.

---

## 2. Exactly what cyrup exports today

**`ShellTool::execute`**, `crates/cyrup-tools/src/tools/bash.rs:316-343`:

```rust
let env_remove = crate::config::session_env_scrub_keys();                       // :316
if self.opts.expose_session_environment
    && let Some(handle) = &self.opts.session_env {                              // :317-318
    let info = handle.get();                                                    // :322
    if let Some(id) = info.session_id {
        env.push(("CYRUP_SESSION_ID".to_string(), id));                         // :325
    }
    if let Some(file) = info.session_file {
        env.push(("CYRUP_SESSION_FILE".to_string(), file.to_string_lossy().into_owned())); // :330-333
    }
    if let (Some(provider), Some(model)) = (info.provider, info.model) {
        env.push(("CYRUP_PROVIDER".to_string(), provider));                     // :338
        env.push(("CYRUP_MODEL".to_string(), model));                           // :339
    }
    if let Some(level) = info.reasoning_level {
        env.push(("CYRUP_REASONING_LEVEL".to_string(), level));                 // :342
    }
}
```

**`config::session_env_scrub_keys`**, `crates/cyrup-tools/src/config.rs:46-53`, driven by
**`config::SESSION_ENV_SUFFIXES`** (`config.rs:31-37`):

```rust
pub const SESSION_ENV_SUFFIXES: [&str; 5] =
    ["SESSION_ID", "SESSION_FILE", "PROVIDER", "MODEL", "REASONING_LEVEL"];

pub fn session_env_scrub_keys() -> Vec<String> {
    let mut keys = Vec::with_capacity(SESSION_ENV_SUFFIXES.len() * 2);
    for suffix in SESSION_ENV_SUFFIXES {
        keys.push(format!("CYRUP_{suffix}"));
        keys.push(format!("PI_{suffix}"));
    }
    keys
}
```

Applied by **`ops::local::command::build_command`**,
`crates/cyrup-tools/src/ops/local/command.rs:24-29`, removals **before** overrides — pi's
delete-then-set ordering, already correct.

### 2.1 The asymmetry, stated precisely

| Family | Scrubbed from the child? | Re-published into the child? |
|---|---|---|
| `CYRUP_SESSION_ID` … `CYRUP_REASONING_LEVEL` | yes (`config.rs:49`) | **yes** (`bash.rs:325-342`) |
| `PI_SESSION_ID` … `PI_REASONING_LEVEL` | yes (`config.rs:50`) | **no** |

**That single asymmetry is the whole gap.** cyrup dual-*scrubs* and single-*writes*. The `PI_*`
family is not merely absent from a cyrup child — it is guaranteed absent, because the scrub deletes
whatever a parent exported and nothing puts it back. `PI_SESSION_ID` in a user's `.bashrc`, hook, or
`shellCommandPrefix` script resolves to the empty string with no diagnostic.

### 2.2 Per-variable condition mapping — cyrup vs pi

| Name | pi condition | cyrup condition | Equivalent? |
|---|---|---|---|
| `*_SESSION_ID` | unguarded inside the branch (`bash.ts:185`) | `if let Some(id)` (`bash.rs:324`) | **Yes in practice** — the only production writer, `builder.rs:888`, is `session_id: Some(session_id.to_string())`, never `None`. See Q3. |
| `*_SESSION_FILE` | `if (sessionFile)` (`:187`) | `if let Some(file)` (`bash.rs:329`) | Yes. Seeded from `manager.session_file()` at `builder.rs:891`; `None` ⇒ unset, matching "unset for ephemeral sessions". |
| `*_PROVIDER` + `*_MODEL` | `if (model)` sets the **pair** (`:188-191`) | `if let (Some(p), Some(m))` sets the pair (`bash.rs:337-340`) | Yes — the pair-atomicity is preserved. |
| `*_REASONING_LEVEL` | `if (ctx.thinkingLevel)` (`:192`) | `if let Some(level)` (`bash.rs:341`) | Yes. `builder.rs:896` is always `Some(thinking_level_to_str(thinking))`, and pi's `"off"` is truthy, so both always set it. |
| the five deletes | unconditional, before the flag (`:178-182`) | unconditional, `bash.rs:316` is outside the `if` | **Yes** — and cyrup's is *wider*, covering ten names. |
| flag default | `?? true` (`bash.ts:345`) | `expose_session_environment: true` (`config.rs:242`, `:300`) | Yes. |
| hook ordering | hook last (`bash.ts:195`) | hook last (`bash.rs:345-353`) | Yes. |
| `!` / `!!` seam | **not** injected (`environment-variables.md:49`) | **not** injected — `crates/cyrup-session-svc/src/bash.rs:148-151` states and implements exactly this | Yes. |

**Everything about the mechanism is a faithful port. The names are the only divergence.** That
matters for the prescription: closing this needs no structural change, only additional pushes.

---

## 3. The precedent already in this tree — three classes of env name

cyrup has already, independently, settled this question three times. The rule it settled on is
visible in the source and it decides this row:

**Class A — internal plumbing (cyrup writes it, cyrup reads it). Rename freely.**
`crates/cyrup-ext-subagents/src/prompt_runtime.rs:719` `CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT`
(pi `PI_SUBAGENT_INHERIT_PROJECT_CONTEXT`), `:727` `CYRUP_SUBAGENT_INHERIT_SKILLS`,
`crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:90` `CYRUP_SUBAGENT_PARENT_SESSION`
(pi `PI_AGENT_ROUTER_PARENT_SESSION_ID`). Both ends are cyrup, so nothing third-party can break.

**Class B — configuration INPUT read by cyrup. Dual-read, `CYRUP_*` first, `PI_*` as a documented
migration alias.** `crates/cyrup-config/src/env.rs:96-128` — nine `PI_*` fallbacks
(`PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`, `PI_PACKAGE_DIR`, `PI_OFFLINE`,
`PI_SKIP_VERSION_CHECK`, `PI_TELEMETRY`, `PI_CACHE_RETENTION`, `PI_CLEAR_ON_SHRINK`,
`PI_HARDWARE_CURSOR`), plus a tenth at `crates/cyrup-tui/src/status.rs:482` (`PI_EXPERIMENTAL`).
`env.rs:25-26` states the policy: *"Typed view over the `CYRUP_*` environment surface, with `PI_*`
accepted as a migration fallback (documented; R-07-028)."* It is documented to users at
`docs/guide/reference/environment.md:9` — *"Each core variable has a `PI_*` migration alias. Both
spellings are checked, `CYRUP_*` first"* — and tabulated at `:21-31`.

**Class C — an interop token a THIRD PARTY reads. Keep pi's spelling verbatim, even though the
product is not pi.** Three in-tree instances, each with the reasoning written at the site:

* `crates/cyrup-tools/src/tools/bash.rs:303` — `env.push(("PI_CODING_AGENT".to_string(), "true".to_string()));`
  The `PI_` prefix is **kept** because it is what a script tests for. Mirrored at
  `crates/cyrup-session-svc/src/bash.rs:163` and `crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:957`.
* `crates/cyrup-provider/src/api/openai_codex_responses/headers.rs:50` — `originator: "pi"` and the
  `pi (...)` User-Agent are *"sent verbatim, NOT rebranded"* because a remote party reads them.
* `crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:79` — `MCP_DIRECT_TOOLS` kept un-namespaced,
  *"pi keeps this un-namespaced"*.

**The five session variables are Class C.** Their only consumer is code cyrup did not write: a user
script, a hook, a `shellCommandPrefix`, a `.bashrc`. That is precisely the property that made
`PI_CODING_AGENT` keep its prefix in the very same function, eleven lines above the pushes under
audit. cyrup currently exports `PI_CODING_AGENT` and `CYRUP_SESSION_ID` from the same `env` vector,
into the same child, on the same call — one honouring the Class C rule and one not.

The *prompt guideline sentence* is a different class again: it is **brand-facing text in cyrup's own
system prompt**, and cyrup rebrands those consistently and deliberately —
`crates/cyrup-tui/src/terminal_title.rs:24-26` (`APP_TITLE`), `crates/cyrup-tui/src/app/share.rs:1`
(the trust banner), `crates/cyrup-tui/src/chrome.rs:113`. So the sentence and the variable names
should NOT be forced to the same answer, and §5 does not force them.

---

## 4. Why the marker's stated justification no longer holds

`bash.rs:228-236` (the marker this row anchors on) argues:

> This is a **forced** divergence, not a rebrand: … `config::session_env_scrub_keys` DELETES the
> five `PI_*` session names from that child unconditionally … so the upstream literal would point
> the model at variables cyrup guarantees are absent.

The argument is **valid but circular**. `PI_*` is guaranteed absent *because cyrup chose not to
publish it*, and the scrub list already contains both families. The premise is a consequence of the
conclusion; nothing external forces it. The project has already rejected exactly this shape of
reasoning once, in the closing note on **TOOL-044 limb 3** (`docs/gap-analysis/04-cyrup-tools.md`):

> The divergence was never *forced* — … the port rule permits only forced divergences, so "no
> reachable consumer" is not a ground to keep one.

Here there IS a reachable consumer (every pi user's shell tooling), and the divergence is not
forced. Both halves of that precedent point the same way.

Note also `bash.rs:248-250`'s supporting claim — *"not one of the twelve live `PI_*` lower-precedence
fallbacks in `cyrup-config/src/env.rs:68-91`"*. The **load-bearing part is true** (none of the five
appears in `env.rs`), but the count and the range are both wrong: there are **nine** at
`env.rs:96-128`. Recorded in §7.

---

## 5. Prescription — CLOSE, by exporting BOTH families

### 5.1 The change

**Publish each of the five under BOTH spellings; leave the guideline sentence saying `CYRUP_*`.**

In `crates/cyrup-tools/src/tools/bash.rs`, inside the existing
`if self.opts.expose_session_environment && let Some(handle) = …` block (`:317-343`), pair every
`CYRUP_*` push with a `PI_*` push of the identical value, under the identical condition. Drive both
off `config::SESSION_ENV_SUFFIXES` (`config.rs:31-37`) rather than ten literals, so the publish list
and the scrub list `session_env_scrub_keys` builds cannot drift — that shared constant already
exists for exactly this reason and is currently used by only one of the two sides.

Nothing else in the mechanism moves:

* `session_env_scrub_keys` (`config.rs:46-53`) **already** covers both families — no change.
* `build_command` (`ops/local/command.rs:24-29`) **already** removes before setting, so
  `PI_SESSION_ID` inherited-stale → deleted → re-set to the live value, exactly pi's `:178-182`
  then `:185-192` — no change.
* The exposure gate, the pair-atomicity, the ephemeral-file guard, the hook-last ordering, the
  `!`/`!!` exclusion — all already correct, all unchanged.

Cost: five additional entries on the child's env vector per `bash`/`powershell` call.

### 5.2 Why the guideline sentence stays `CYRUP_*`

After the change `PI_*` is no longer "guaranteed absent", so pi's literal would become *accurate* —
but it should still not be shipped. The guideline is model-facing prose in cyrup's system prompt,
and `CYRUP_*` is the canonical, documented spelling
(`docs/guide/reference/environment.md:285-292`, `docs/guide/guides/tools-and-permissions.md:185-201`).
Pointing the model at the migration alias instead of the canonical name is the wrong instruction to
give it. Keeping `CYRUP_*` here is the same brand-token rebrand as `APP_TITLE` and the trust banner
(§3), which the project already accepts.

The delta therefore **survives, but shrinks and changes class**: from *"forced, because the
variables cyrup names are the only ones that exist"* to *"a brand token in model-facing text, while
both spellings are live in the environment."* That is a materially weaker and more honest claim, and
it is the claim the rewritten `[CYRUP-DELTA]` annotation must make. The caller-visible harm — a pi
user's script breaking — is gone either way.

### 5.3 Powershell moves with bash for free

`crates/cyrup-tools/src/tools/powershell.rs:30-45` (`POWERSHELL_CONFIG`) instantiates the same
`ShellTool` engine, so the injection change lands on both tools at once — matching upstream, where
`powershell.ts` re-exports `createShellToolDefinition` from `bash.ts`. Only `powershell.rs:39-41`'s
guideline literal is separate, and under this prescription it does not change.

### 5.4 Docs that must move with the code

* `docs/guide/reference/environment.md:278-292` — the "Variables cyrup sets for you — outputs"
  section. `:282-283` currently says *"both the `CYRUP_*` and `PI_*` spellings are stripped from the
  child environment and repopulated"*, which is **already wrong today** (only `CYRUP_*` is
  repopulated) and becomes correct after the change. The table at `:285-291` gains the `PI_*` alias
  column, mirroring the input table's alias column at `:21-31`.
* `docs/guide/guides/tools-and-permissions.md:185-201` — "What the bash tool exports". Same table
  treatment; `:198-201` already says "cyrup scrubs all five from the child environment before
  writing its own values" and needs the alias sentence.
* `docs/guide/guides/models.md:220-222` — mentions `CYRUP_REASONING_LEVEL` only; add the alias in
  passing or leave it pointing at the tools-and-permissions section (it already does).

---

## 6. Tests — exactly which assertions change

### 6.1 `crates/cyrup-tools/src/tests/bash_session_env.rs` (the injection half)

* **`PROBE` (`:43-45`)** — widen from five `CYRUP_*` slots to ten, appending the five `PI_*` names in
  the same order. Consequential edits to the four call sites that match against it:
  * `bash_child_sees_the_live_session_metadata` (`:73-87`) — expected literal at `:83` becomes the
    ten-slot form, the `PI_*` half repeating the same five values.
  * `switching_model_affects_the_next_command` (`:91-128`) — `:116` and `:125` each gain the
    mirrored `PI_*` triple, proving the spawn-time re-read applies to both families.
  * `an_ephemeral_session_publishes_no_session_file` (`:133-151`) — `:148` becomes the ten-slot form
    with **both** `*_SESSION_FILE` slots empty. This is the assertion that pins pi's
    `if (sessionFile)` guard (`bash.ts:187`) across the alias.
  * `the_exposure_flag_suppresses_the_injection` (`:154-163`) — `:162`'s `"[][][][][]"` becomes ten
    empty slots. This is the assertion that pins the gate covering both families.
* **NEW, and this is the RED test the Definition of Done asks for** —
  `pi_named_session_variables_reach_the_child`: seeded handle, default opts, probe only the five
  `PI_*` names, assert the live values. **Fails today** (five empty slots, because `bash.rs:316`
  scrubs them and nothing re-publishes), passes after. Keep it as a standalone test rather than
  relying on the widened `PROBE` alone, so the intent survives a future refactor of the probe.
* **NEW** — `both_spellings_carry_identical_values`: assert slot-for-slot equality between the two
  halves of the widened probe, so a partial port (e.g. `PI_SESSION_ID` published but
  `PI_REASONING_LEVEL` forgotten) fails loudly.
* **UNCHANGED, deliberately** — `the_prompt_guideline_tracks_the_exposure_flag` (`:172-190`,
  asserting the `CYRUP_*` sentence at `:178`) and `the_guideline_uses_pi_v0_84_1_softened_phrasing`
  (`:205-227`, `:218` and `:225`). Under §5.2 the sentence does not move. **If David chooses
  Option B in §8, these two are the tests that change**, along with `pi_schema.rs:141` / `:151`,
  `bash.rs:95-97` (`BASH_CONFIG.prompt_guidelines`) and `powershell.rs:39-41`.
* **UNCHANGED** — `bash_child_sees_the_agent_identity_markers` (`:239-259`),
  `identity_markers_survive_expose_session_environment_off` (`:264-292`),
  `cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag` (`:314-345`). All three
  belong to sibling row `…-bash-rs-312.md`.
* **Module doc (`:1-11`)** — cites `bash.ts:171-181`, `:322`, `:329-331`; at `e8682309` these are
  `:183-193`, `:345`, `bash.ts:47-49`. Correct while editing (§7).

### 6.2 `crates/cyrup-tools/tests/bash_env_scrub.rs` (the scrub half)

* **`session_metadata_is_scrubbed_and_hooks_can_delete` (`:64-133`)** — both existing
  `!out.contains("stale-")` assertions (`:84-87` and `:98-104`) **stay green unchanged**: both runs
  use `BashOpts` with `session_env: None` (`config.rs:243`), so no injection happens in either and
  the scrub is all that is under test. The seeding at `:67-77` and the probe at `:80-82` already
  cover all ten names — no edit needed.
* **NEW, and it is the assertion that actually protects the fix** —
  `a_stale_pi_value_is_replaced_not_merely_deleted`: with `PI_SESSION_ID=stale-pi-session` already
  in the process env (this binary is the only place that may `set_var`) **and** a seeded
  `SessionEnvHandle`, assert the child sees the live id and not `stale-pi-session`. That is the
  delete-then-set ordering of `bash.ts:178`+`:185` under the alias; without it, a future refactor
  that publishes `PI_*` via `env` while dropping it from `session_env_scrub_keys` would still pass
  everything else. It must live in **this** binary because it needs `set_var`, per the file's own
  single-`#[test]`-per-binary rule (`:53-58`).

### 6.3 `crates/cyrup-session-svc/src/tests/bash_session_env_wiring.rs` (end-to-end)

The only test that proves the values reach a child through the real session wiring rather than a
hand-built handle.

* **Probe loop (`:54`)** — extend the variable list with the five `PI_*` names.
* **`get(&kv, …)` assertions at `:99`, `:106`, `:111`, `:115`, `:127-128`, `:130`, `:146-147`,
  `:154`, `:159`, `:164`** — each gains its `PI_*` counterpart, or is refactored to assert over both
  spellings from one table. `:146-164` is the fork/resume case (`session/forking.rs:464-477`
  republishes id+file), so it must cover the alias too or a fork would silently re-publish only half.

### 6.4 `crates/cyrup-tools/src/tests/pi_tool_semantics.rs`

* **`bash_prompt_guideline_deltas_are_tagged_cyrup_delta` (`:778-839`, doc comment `:759-776`)** — a **source scan** over the
  doc block between `fn prompt_snippet(` and `fn prompt_guidelines(`. It asserts exactly two
  `[CYRUP-DELTA` tags (`:806-816`, the exact-count assertion) and pins their exact opening literals — including
  `"[CYRUP-DELTA — deliberate, value only; the variable-family name inside the string]"` at
  `:823-829`. Rewriting `bash.rs:228-236`'s justification per §5.2 **must keep that literal
  byte-identical** (the tag still describes a value-only, variable-family delta — only its
  *reasoning* changes) or this assertion must be updated in the same commit. The `bash.ts:47` /
  `bash.ts:330` citation assertions (`:830-838`) belong to sibling row `…-bash-rs-214.md`; leave
  them.
* **`:759-776`** — that same doc comment describing the two divergences says `PI_*` → `CYRUP_*` is because
  the `PI_*` names are deleted. That prose becomes false and must be rewritten with the tag literal.

### 6.5 `crates/cyrup-tools/src/tests/pi_schema.rs`

* `PI_BASH_GUIDELINES` (`:140-141`) and `PI_POWERSHELL_GUIDELINES` (`:150-151`) — **unchanged**
  under §5.2. Their explanatory comments at `:134-138` and `:145-147` (both asserting `CYRUP_*` is
  "the family cyrup's `resolveSpawnContext` port actually publishes") need one clause added: both
  families are published; `CYRUP_*` is the canonical one named in the prompt.

### 6.6 Regression surface

Nothing outside `cyrup-tools` and `cyrup-session-svc` reads these names.
`grep -rn 'CYRUP_SESSION_ID\|CYRUP_SESSION_FILE\|CYRUP_PROVIDER\|CYRUP_MODEL\|CYRUP_REASONING_LEVEL' --include=*.rs crates/`
returns only `tools/bash.rs`, `config.rs`, the three test files above, and the four
`cyrup-session-svc` republish sites (`session/model.rs:489`, `session/forking.rs:464`,
`session/thinking.rs:63`, `builder.rs:888-897`) — all of which write into `SessionEnvHandle` and are
name-agnostic. No settings key, no session-file field, no RPC surface carries these names.

---

## 7. Findings recorded, not descoped

Each of these is a defect found while establishing the above. None is closed here; each is stated so
it is not re-derived.

1. **`docs/guide/reference/environment.md:282-283` is factually wrong today.** It tells users *"both
   the `CYRUP_*` and `PI_*` spellings are stripped from the child environment **and repopulated**"*.
   Only `CYRUP_*` is repopulated. The prescription in §5 makes the sentence true; if David chooses
   otherwise, the sentence must be corrected instead.
2. **`bash.rs:248-250` miscounts and mis-ranges the `PI_*` fallbacks** — claims "twelve … in
   `cyrup-config/src/env.rs:68-91`"; there are **nine**, at `env.rs:96-128` (plus a tenth,
   `PI_EXPERIMENTAL`, in `cyrup-tui/src/status.rs:482`). The load-bearing claim — none of the five
   session names is among them — is true.
3. **Citation drift against `e8682309` across the bash/session-env sites.** Every one of these
   resolves to real upstream code, but not at the line cited. Mapping:

   | Cited in cyrup | Sites | Actual @`e8682309` |
   |---|---|---|
   | `bash.ts:165-170` (the deletes) | `config.rs:16`, `config.rs:29`, `bash.rs:294`, `bash_env_scrub.rs:4` | `bash.ts:178-182` |
   | `bash.ts:171-181` (the injection) | `bash.rs:245`, `bash.rs:262`, `config.rs:56`, `builder.rs:870`, `builder.rs:894`, `bash_session_env.rs:3` | `bash.ts:183-193` |
   | `bash.ts:158-184` (`resolveSpawnContext`) | `bash.rs:293`, `session-svc/bash.rs:150` | `bash.ts:170-196` |
   | `bash.ts:173-174` / `:174` (`if (sessionFile)`) | `builder.rs:890`, `bash.rs:328`, `bash_session_env.rs:131` | `bash.ts:186-187` |
   | `bash.ts:150-154` (`BashSpawnContext`) | `config.rs:10` | `bash.ts:162-166` |
   | `bash.ts:322` / `:327` (flag default) | `bash_session_env.rs:6`, `:166`, `pi_schema.rs:135` | `bash.ts:345` |
   | `bash.ts:329-331` / `:330` (guideline) | `bash_session_env.rs:6` | `bash.ts:47-49` (const hoist) |
   | `bash.ts:164` (`getShellEnv()` spread) | `config.rs:12`, `bash.rs:295` | `bash.ts:177` |
   | `bash.ts:156` (hook receives env) | `bash_env_scrub.rs:60` | `bash.ts:168` (`BashSpawnHook`) |
   | `docs/environment-variables.md:27` (timing) | `bash.rs:321`, `config.rs:73-75` | `:32` |
   | `docs/environment-variables.md:22` ("unset for ephemeral") | `config.rs:62`, `bash_session_env.rs:131` | `:27` |
   | `docs/environment-variables.md:19-27` (the table) | `config.rs:30` | `:20-30` |
   | `docs/extensions.md:2122` (hook can delete) | `config.rs:17`, `bash_env_scrub.rs:61` | `:2140` |

   These are v0.83.0/v0.84.1-era offsets that the `e8682309` tree has moved. **Open question Q4**
   asks whether re-anchoring is in scope for this row or belongs to a tree-wide sweep; the audit
   reference commit is `e8682309`, so a reader following any of these citations today lands in the
   wrong place.
4. **Two seams set `*_SESSION_FILE` differently upstream and cyrup ports only one.** The tool path
   leaves it **unset** for an ephemeral session (`bash.ts:187`); the SDK harness path sets it to the
   **empty string** (`create-harness.ts:115`, pinned at `create-harness.test.ts:204-205`). cyrup has
   no harness port, so today there is no conflict — but `config.rs:61-63`'s doc records only the
   unset rule as "pi's" behaviour, which a future harness port would inherit wrongly.
5. **`SESSION_ENV_SUFFIXES` is used by only one of its two natural consumers.** `config.rs:31-37`
   exists to keep the family list in one place, but `bash.rs:325-342` open-codes five `CYRUP_*`
   literals instead of deriving them. That is how the publish list and the scrub list came to
   disagree in the first place. §5.1 folds the fix for this into the change.

---

## 8. Decision — CLOSE. The accept case, argued for David, alongside it.

**Prescribed: option 1, close it,** in the shape of §5 — export both families, keep the guideline
saying `CYRUP_*`. Rationale in one line: the five variables are a **Class C interop token** (§3),
the same class as `PI_CODING_AGENT`, which cyrup already exports **with its `PI_` prefix intact from
the identical `env` vector eleven lines above the pushes under audit**; the marker's
"forced divergence" claim is circular (§4); and the fix is additive, costs five env entries, and
touches no mechanism.

**The accept case, stated at its strongest, because it is not empty:**

> A product named cyrup exporting `PI_SESSION_ID` is exporting a competitor's brand into every shell
> its users run, permanently — env-var names are the hardest surface in software to ever remove,
> because the removal is silent. cyrup's `PI_*` migration aliases in `env.rs` are **inputs**, which
> a user can stop setting on their own timetable; an **output** alias is a promise cyrup keeps
> forever on the user's behalf, and every script written against it deepens the promise. Cyrup
> already ships a coherent, documented, self-consistent `CYRUP_*` output surface
> (`docs/guide/reference/environment.md:278-292`), and a migrating user's fix is one line in their
> script. If cyrup is a product rather than a pi-compatible runtime, the clean break belongs here —
> at the naming layer, once, deliberately — rather than being deferred into an alias nobody will
> ever be able to delete.

That case turns entirely on **whether cyrup is positioning as a pi-compatible drop-in or as its own
product** — which is David's call, not a parity finding. If the answer is "own product", the right
disposition is **option 3, reshape**, not option 2: keep `CYRUP_*` as the only exported family, but
**stop scrubbing `PI_*`** so a pi-flavoured parent's variables at least pass through instead of
being destroyed, and add a one-line note to `docs/guide/reference/environment.md` telling migrating
users the five names changed. That is strictly better than today's silent-scrub, whatever the naming
decision. The default recommendation stays option 1.

### Open questions for David

* **Q1 — naming.** `CYRUP_*` only / `PI_*` only / **both** (recommended)? This is the product
  decision the whole row reduces to.
* **Q2 — the guideline sentence.** §5.2 keeps it `CYRUP_*`. Reverting it to pi's `PI_*` literal
  would zero the last of TOOL-043's limb (b), at the cost of naming the alias rather than the
  canonical spelling to the model. Cheap either way once Q1 is answered — the affected sites are
  `bash.rs:95-97`, `powershell.rs:39-41`, `pi_schema.rs:141`/`:151`, and the two guideline tests in
  `bash_session_env.rs:172-227`. Interacts with sibling row `…-bash-rs-214.md`.
* **Q3 — `*_SESSION_ID` when the id is absent.** pi sets it unconditionally inside the branch
  (`bash.ts:185`), so an empty id would export an **empty** variable; cyrup's `if let Some(id)`
  (`bash.rs:324`) would leave it **unset**. Unreachable today (`builder.rs:888` is always `Some`),
  so this is a latent, not live, divergence. Tighten to match pi, or leave it and document the
  invariant at `config.rs:59`?
* **Q4 — citation drift (finding 3).** Re-anchor the thirteen drifted `bash.ts` / docs citations to
  `e8682309` as part of this row's commit, or file a tree-wide re-anchoring sweep? They are wrong
  either way, and the audit's reference commit is `e8682309`.
* **Q5 — finding 1.** `docs/guide/reference/environment.md:282-283` is wrong **today** regardless of
  Q1's answer. Fix it in this commit?

## Definition of done

1. The gap is closed, or the marker records an explicit authorized acceptance.
2. If closed, a test fails without the change.
   → **`pi_named_session_variables_reach_the_child`** in
   `crates/cyrup-tools/src/tests/bash_session_env.rs` (§6.1) is that test: it is RED today because
   `bash.rs:316` scrubs the five `PI_*` names and nothing re-publishes them.
3. No behaviour regression in the owning crate.
   → §6.6 bounds the surface: only `cyrup-tools` and `cyrup-session-svc` name these variables, and
   every `cyrup-session-svc` site writes through the name-agnostic `SessionEnvHandle`.
