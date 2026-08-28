---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:236"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 21:00
---

# Capability gap: `crates/cyrup-tools/src/tools/bash.rs:236`

Classified a **capability gap** — a caller can observe a difference — by the audit that reviewed all
87 `CYRUP-DELTA` markers against pi at `e8682309`.

The marker was written by an agent and was never authorized as an accepted divergence. This row
decides it.

---

# AUGMENTATION — 2026-08-28 (re-derived pass)

Reference tree [`./tmp/pi`](../../../tmp/pi) pinned at **`e8682309`**
(`packages/coding-agent/package.json:3` → `"version": "0.84.3"`). Every upstream line below was read
out of that working tree this pass; every cyrup line was re-grepped this pass. Anchor on the
**symbol**, treat the number as a hint.

**Scope.** This row owns the **variable-family name** — the `PI_*` → `CYRUP_*` token and the
environment it describes. Two siblings own the neighbouring deltas in the same doc block and are not
re-decided here:

* [`MEDIUM-delta-cyrup-tools-src-tools-bash-rs-214.md`](./MEDIUM-delta-cyrup-tools-src-tools-bash-rs-214.md)
  — the `"You can inspect …"` **wording** (version-lag-ahead). At `e8682309` cyrup and pi agree on
  the wording.
* [`MEDIUM-delta-cyrup-tools-src-tools-bash-rs-312.md`](./MEDIUM-delta-cyrup-tools-src-tools-bash-rs-312.md)
  — `PI_CODING_AGENT` / `AI_AGENT`, which are identity markers, not session keys, and sit outside
  the exposure gate.

---

## 1. Upstream, verified at `e8682309`

One function: **`resolveSpawnContext`**,
[`tmp/pi/packages/coding-agent/src/core/tools/bash.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)
`:170-196`, called from `execute` at `:363`.

```ts
const env = { ...getShellEnv() };                                      // :177
delete env.PI_SESSION_ID;                                              // :178
delete env.PI_SESSION_FILE;                                            // :179
delete env.PI_PROVIDER;                                                // :180
delete env.PI_MODEL;                                                   // :181
delete env.PI_REASONING_LEVEL;                                         // :182
if (exposeSessionEnvironment && ctx) {                                 // :183
    const model = ctx.model;                                           // :184
    env.PI_SESSION_ID = ctx.sessionManager.getSessionId();             // :185
    const sessionFile = ctx.sessionManager.getSessionFile();           // :186
    if (sessionFile) env.PI_SESSION_FILE = sessionFile;                // :187
    if (model) {                                                       // :188
        env.PI_PROVIDER = model.provider;                              // :189
        env.PI_MODEL = model.id;                                       // :190
    }                                                                  // :191
    if (ctx.thinkingLevel) env.PI_REASONING_LEVEL = ctx.thinkingLevel; // :192
}                                                                      // :193
const baseContext: BashSpawnContext = { command, cwd, env };           // :194
return spawnHook ? spawnHook(baseContext) : baseContext;               // :195
```

Four load-bearing properties, all verified in the block above:

* the five deletes at `:178-182` are **unconditional** — they run before the flag is consulted at
  `:183`, so a stale inherited value can never survive;
* **delete-then-set**, so a name in both lists ends up **set**;
* the spawn hook runs **last** (`:195`) and therefore sees the populated `env`
  (`docs/extensions.md:2140`: "Injection happens before `spawnHook`, so hooks receive these values
  in `env`");
* the flag **defaults on** — `bash.ts:345`, `options?.exposeSessionEnvironment ?? true`, declared at
  `:206` as `/** Expose current Pi session metadata as PI_* environment variables. Default: true */`.

The guideline sentence that this row's marker sits on is `bash.ts:47-49`
(`bashToolSystemPromptContribution`), byte-identical at `powershell.ts:18-20`, and reads
`"You can inspect PI_* environment variables for current model and session details."` Both are
consumed through the shared `createShellToolDefinition` (`bash.ts:352`, gated on the same flag).

User-facing contract: `docs/environment-variables.md:20-30` (table), `:32` (values resolved per
command), `:49` (the negative — *"not injected into user-entered `!` or `!!` commands"*).
`docs/extensions.md:2140` pins the hook ordering.

A **second, different** upstream seam sets the same five names: `src/server/create-harness.ts:114-115`
(SDK harness `prepare`), which writes `PI_SESSION_FILE = sessionFile ?? ""` — empty string, not
unset. cyrup has no port of `create-harness.ts`; out of scope, recorded so a future SDK port does not
read the tool path as the only answer.

---

## 2. cyrup today, verified

**`ShellTool::execute`**, [`crates/cyrup-tools/src/tools/bash.rs`](../../../crates/cyrup-tools/src/tools/bash.rs)
`:316-343` — scrub at `:316`, gate at `:317-318`, then five pushes: `CYRUP_SESSION_ID` `:325`,
`CYRUP_SESSION_FILE` `:330-333`, `CYRUP_PROVIDER`/`CYRUP_MODEL` `:338-339`,
`CYRUP_REASONING_LEVEL` `:342`.

**`config::session_env_scrub_keys`**, [`crates/cyrup-tools/src/config.rs`](../../../crates/cyrup-tools/src/config.rs)
`:46-53`, driven by `SESSION_ENV_SUFFIXES` (`config.rs:31-37`), emits **both** `CYRUP_{suffix}` and
`PI_{suffix}` for all five suffixes.

**`ops::local::command::build_command`**,
[`crates/cyrup-tools/src/ops/local/command.rs`](../../../crates/cyrup-tools/src/ops/local/command.rs)
`:22-29` — `env_remove` first, then `env`. pi's delete-then-set, already correct.

Every other mechanism property matches pi exactly, checked one by one:

| Property | pi | cyrup | Same? |
|---|---|---|---|
| unconditional scrub before the flag | `bash.ts:178-182` | `bash.rs:316`, outside the `if` | yes (cyrup's is wider — ten names) |
| session-file guard | `if (sessionFile)` `:187` | `if let Some(file)` `bash.rs:329` | yes |
| provider/model set as a **pair** | `if (model)` `:188-191` | `if let (Some(p), Some(m))` `bash.rs:337` | yes |
| reasoning level guard | `if (ctx.thinkingLevel)` `:192` | `if let Some(level)` `bash.rs:341` | yes |
| flag default on | `bash.ts:345` | `config.rs:242`, `:300` | yes |
| hook last | `bash.ts:195` | `bash.rs:345-353` | yes |
| `!` / `!!` seam not injected | `environment-variables.md:49` | `crates/cyrup-session-svc/src/bash.rs:158-160` states and implements it | yes |

### 2.1 The gap, stated exactly

| Family | Scrubbed from the child? | Re-published into the child? |
|---|---|---|
| `CYRUP_SESSION_ID` … `CYRUP_REASONING_LEVEL` | yes (`config.rs:49`) | **yes** (`bash.rs:325-342`) |
| `PI_SESSION_ID` … `PI_REASONING_LEVEL` | yes (`config.rs:50`) | **no** |

cyrup dual-**scrubs** and single-**writes**. `PI_SESSION_ID` in a user's `.bashrc`, hook, or
`shellCommandPrefix` is not merely absent under cyrup — it is *guaranteed* absent, because the scrub
destroys whatever a parent exported and nothing puts it back. Resolves to the empty string, no
diagnostic. That single asymmetry is the whole gap; the port is otherwise faithful, so closing it
needs **no structural change**.

---

## 3. Why the marker's own justification does not hold

`bash.rs:228-236` argues the divergence is **forced**:

> … `config::session_env_scrub_keys` DELETES the five `PI_*` session names from that child
> unconditionally … so the upstream literal would point the model at variables cyrup guarantees are
> absent.

The premise is a consequence of the conclusion. `PI_*` is absent only because cyrup chose not to
publish it; the scrub list already carries both families. Nothing external forces it. The project
already rejected this exact shape of argument once — TOOL-044 limb 3, in
[`docs/gap-analysis/04-cyrup-tools.md`](../../../docs/gap-analysis/04-cyrup-tools.md) `:953-954`:
*"divergence was never \*forced\* — … — and the port rule permits only forced divergences, so 'no
reachable consumer' is not a ground to keep one."* Here there **is** a reachable consumer, and the
divergence is not forced.

**Precedent that decides the naming.** cyrup has already settled this three times, and the rule is
in the source:

* **Class A — internal plumbing, both ends cyrup. Rename freely.**
  `crates/cyrup-ext-subagents/src/prompt_runtime.rs:719`, `:727`;
  `crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:90`.
* **Class B — configuration INPUT read by cyrup. Dual-read, `CYRUP_*` first, `PI_*` as a documented
  migration alias.** `crates/cyrup-config/src/env.rs:96-128` — **nine** `PI_*` fallbacks; policy
  stated at `env.rs:25-26`, documented at `docs/guide/reference/environment.md:9`.
* **Class C — an interop token a THIRD PARTY reads. Keep pi's spelling verbatim.**
  `bash.rs:303` `PI_CODING_AGENT` (mirrored `crates/cyrup-session-svc/src/bash.rs:173`,
  `spawn_plan.rs:962`); `crates/cyrup-provider/src/api/openai_codex_responses/headers.rs:50`/`:94`
  (`originator: "pi"` sent verbatim, *"NOT rebranded"*); `spawn_plan.rs:79` `MCP_DIRECT_TOOLS`
  (*"pi keeps this un-namespaced"*).

The five session variables are **Class C**: their only consumers are scripts cyrup did not write.
That is precisely the property that made `PI_CODING_AGENT` keep its prefix — pushed onto the *same*
`env` vector, into the *same* child, thirteen lines above the pushes under audit.

The **prompt guideline sentence** is a different class: brand-facing text in cyrup's own system
prompt, where cyrup rebrands consistently (`crates/cyrup-tui/src/terminal_title.rs:24-26`,
`crates/cyrup-tui/src/app/share.rs:1`, `crates/cyrup-tui/src/chrome.rs:113`). The two do not get the
same answer, and §4 does not give them one.

---

## 4. PRESCRIPTION — the required implementation

**Publish all five session values under BOTH spellings. Keep the guideline sentence saying
`CYRUP_*`.** This is the path; there is no alternative to weigh.

### 4.1 `crates/cyrup-tools/src/config.rs` — one new helper, after `session_env_scrub_keys` (`:53`)

`SESSION_ENV_SUFFIXES` (`:31-37`) exists to hold the family list once, but only the scrub side
derives from it — that is how the publish list and the scrub list came to disagree. Give the publish
side the same source:

```rust
/// Every fully-qualified key `bash` PUBLISHES for one session-metadata suffix: the canonical
/// `CYRUP_*` spelling first, then the `PI_*` spelling pi writes (bash.ts:185-192 @e8682309),
/// which cyrup keeps live as a migration alias so a pi-era user script keeps working.
///
/// Both are already in [`session_env_scrub_keys`], and `build_command` removes before it sets
/// (`ops/local/command.rs:22-29`), so a stale inherited value is deleted and then replaced —
/// exactly pi's `:178-182` then `:185-192`.
pub fn session_env_keys(suffix: &str) -> [String; 2] {
    debug_assert!(
        SESSION_ENV_SUFFIXES.contains(&suffix),
        "`{suffix}` is published but not scrubbed"
    );
    [format!("CYRUP_{suffix}"), format!("PI_{suffix}")]
}
```

The `debug_assert!` is the coupling: a suffix that is published but not in `SESSION_ENV_SUFFIXES`
(and therefore not scrubbed) fails loudly instead of silently drifting.

### 4.2 `crates/cyrup-tools/src/tools/bash.rs` — replace the five literal pushes

Add a free function beside `resolve_timeout_ms` (i.e. above `ShellToolConfig`, `bash.rs:~57`):

```rust
/// Push one session-metadata value into the child env under every spelling cyrup publishes.
fn push_session_env(env: &mut Vec<(String, String)>, suffix: &str, value: String) {
    for key in crate::config::session_env_keys(suffix) {
        env.push((key, value.clone()));
    }
}
```

Then, inside the existing `if self.opts.expose_session_environment && let Some(handle) = …` block
(`bash.rs:317-343`), leaving **every guard, every condition and the block itself unchanged**:

| Site | today | after |
|---|---|---|
| `bash.rs:325` | `env.push(("CYRUP_SESSION_ID".to_string(), id));` | `push_session_env(&mut env, "SESSION_ID", id);` |
| `bash.rs:330-333` | `env.push(("CYRUP_SESSION_FILE".to_string(), file.to_string_lossy().into_owned()));` | `push_session_env(&mut env, "SESSION_FILE", file.to_string_lossy().into_owned());` |
| `bash.rs:338-339` | two pushes | `push_session_env(&mut env, "PROVIDER", provider);` then `push_session_env(&mut env, "MODEL", model);` |
| `bash.rs:342` | `env.push(("CYRUP_REASONING_LEVEL".to_string(), level));` | `push_session_env(&mut env, "REASONING_LEVEL", level);` |

Borrow note: `push_session_env` takes `&mut env` only inside the block; NLL ends that borrow before
`env` is moved into `BashSpawnContext` at `bash.rs:345-350`. No restructuring needed.

Cost: five extra entries on the child's env vector per `bash`/`powershell` call.

**`if let Some(id)` at `bash.rs:324` stays as it is.** pi sets `PI_SESSION_ID` unconditionally inside
the branch (`:185`), so an absent id would export an *empty* variable there; cyrup leaves it unset.
The divergence is unreachable — the only production writer,
`crates/cyrup-session-svc/src/builder.rs:895`, is `session_id: Some(session_id.to_string())` — and
unset is strictly safer for a consumer than an empty string that reads as a real id. Do not change it.

### 4.3 `crates/cyrup-tools/src/tools/bash.rs:228-236` — rewrite the marker's reasoning

The delta survives but shrinks: from *"forced, because the `PI_*` names are guaranteed absent"* to
*"a brand token in model-facing prose, while both spellings are live in the child environment."*
Rewrite the body of the second tag to say that, and delete the now-false clauses at `bash.rs:243-250`
(the paragraph beginning *"`PI_*` -> `CYRUP_*` is deliberate and is NOT a blind rebrand"*), replacing
them with the true statement: cyrup publishes both families; `CYRUP_*` is the canonical, documented
spelling (`docs/guide/reference/environment.md:278-292`,
`docs/guide/guides/tools-and-permissions.md:185-201`), and pointing the model at a migration alias
rather than the canonical name would be the wrong instruction.

**Constraints the rewrite must respect** — these are asserted by source-scanning tests:

* `crates/cyrup-tools/src/tests/pi_tool_semantics.rs:778-839` scans the doc text **between**
  `fn prompt_snippet(` and `fn prompt_guidelines(` and asserts **exactly two** `[CYRUP-DELTA`
  occurrences. Do not add or remove a tag in that window.
* The two tag literals must stay byte-identical:
  `[CYRUP-DELTA — version lag, AHEAD of the ported tag; wording only]` and
  `[CYRUP-DELTA — deliberate, value only; the variable-family name inside the string]`. The second
  is still accurate — it is still a value-only, variable-family delta; only its *reasoning* changes.
* The window must still contain the substrings `CYRUP_*`, `bash.ts:47` and `bash.ts:330`
  (`pi_tool_semantics.rs:830-838` — those belong to sibling row `…-214.md`; leave them).
* `crates/cyrup-tools/src/tests/bash_session_env.rs:315-345`
  (`cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag`) does
  `src[..at].rfind("[CYRUP-DELTA")` where `at` is the index of the `AI_AGENT` push. It must keep
  resolving to the `AI_AGENT` marker at `bash.rs:304`. **Do not add a `[CYRUP-DELTA` tag at the new
  injection sites** — publishing both families removes a divergence, it does not create one — and do
  not add one anywhere between `bash.rs:304` and the `AI_AGENT` push.

### 4.4 What does not move

* `session_env_scrub_keys` (`config.rs:46-53`) — already covers both families.
* `build_command` (`ops/local/command.rs:22-29`) — already removes before setting.
* The exposure gate, pair-atomicity, ephemeral-file guard, hook-last ordering, the `!`/`!!`
  exclusion — all already correct.
* `powershell.rs:30-45` — instantiates the same `ShellTool` engine, so the injection change lands on
  both tools at once, matching upstream's shared factory. `powershell.rs:36-41`'s guideline literal
  is unchanged.
* Every existing assertion in `bash_session_env.rs`, `tests/bash_env_scrub.rs`,
  `bash_session_env_wiring.rs` and `pi_schema.rs` — checked one by one: each probes only `CYRUP_*`
  names, or runs with `BashOpts::default()` (`config.rs:243`, `session_env: None`, so no injection at
  all). The change is purely additive and breaks none of them.
* `docs/guide/reference/environment.md:281-283` currently claims *"both the `CYRUP_*` and `PI_*`
  spellings are stripped from the child environment **and repopulated**"* — false today, **true after
  this change**. No doc edit is required to land it.

---

## 5. The guard

One new test, in [`crates/cyrup-tools/src/tests/bash_session_env.rs`](../../../crates/cyrup-tools/src/tests/bash_session_env.rs),
beside the existing ones:

```rust
/// The `PI_*` spelling is a live migration alias, not merely unscrubbed: pi publishes these five
/// names (bash.ts:185-192 @e8682309) and a pi-era user script, hook or `.bashrc` reads them. RED
/// before the fix — `bash.rs:316` scrubs them and nothing re-published them, so every slot was
/// empty.
#[tokio::test]
async fn pi_named_session_variables_reach_the_child() {
    let out = run(BashOpts {
        session_env: Some(seeded_handle()),
        ..BashOpts::default()
    })
    .await;
    // same probe shape as PROBE, over the PI_* half of the family
    assert!(
        out.contains("[sess-abc123][/sessions/sess-abc123.jsonl][anthropic][claude-opus-5][medium]"),
        "got: {out}"
    );
}
```

with a `PI_*` sibling of `PROBE` (`bash_session_env.rs:43-45`) — `printf '[%s][%s][%s][%s][%s]\n'`
over `PI_SESSION_ID` / `PI_SESSION_FILE` / `PI_PROVIDER` / `PI_MODEL` / `PI_REASONING_LEVEL`. This
binary never mutates the process environment, so it stays in `src/tests/`, not
`tests/bash_env_scrub.rs` (whose module doc pins it to a single `#[test]` because it calls
`set_var`).

It fails today for the exact reason this row exists, and passes after §4.2 alone.

---

## 6. Facts corrected while establishing the above

1. **`bash.rs:249-250` miscounts and mis-ranges.** It claims *"the twelve live `PI_*`
   lower-precedence fallbacks in `cyrup-config/src/env.rs:68-91`"*. There are **nine**, at
   `env.rs:96-128` (plus a tenth, `PI_EXPERIMENTAL`, at `crates/cyrup-tui/src/status.rs:482`). The
   load-bearing claim — none of the five session names is among them — is true. Correct the count and
   the range while rewriting the block per §4.3.
2. **Citation drift against `e8682309`** across the bash/session-env sites. All resolve to real
   upstream code, none at the number cited. The ones touched by §4:
   `bash.ts:165-170` → **`:178-182`**; `bash.ts:171-181` → **`:183-193`**; `bash.ts:158-184` →
   **`:170-196`**; `bash.ts:173-174` → **`:186-187`**; `bash.ts:150-154` → **`:162-166`**;
   `bash.ts:164` → **`:177`**; `bash.ts:322`/`:327` → **`:345`**; `bash.ts:329-331`/`:330` →
   **`:47-49`** (const hoist); `bash.ts:156` → **`:168`**; `bash.ts:194`/`:198` → **`:206`**/**`:209`**;
   `docs/environment-variables.md:27` → **`:32`**; `:22` → **`:27`**; `:19-27` → **`:20-30`**;
   `docs/extensions.md:2122` → **`:2140`**. Re-anchor the ones inside the lines §4 edits
   (`bash.rs`, `config.rs`); the rest are a tree-wide sweep, not this row.
3. **`create-harness.ts:114-115` sets `PI_SESSION_FILE` to `""`** where the tool path leaves it
   unset. `config.rs:61-63` records only the unset rule as "pi's" behaviour. No conflict today
   (cyrup has no harness port); noted so a future one does not inherit the wrong rule.

---

## Definition of done

1. `crates/cyrup-tools/src/config.rs` grows `session_env_keys` (§4.1) and
   `crates/cyrup-tools/src/tools/bash.rs:324-343` publishes every session value under both the
   `CYRUP_*` and `PI_*` spellings through it (§4.2), with no change to any guard, the exposure gate,
   the scrub, or the hook ordering.
2. `crates/cyrup-tools/src/tests/bash_session_env.rs::pi_named_session_variables_reach_the_child`
   (§5) exists and passes; it is RED without step 1.
3. `bash.rs:228-236`'s second `[CYRUP-DELTA` block no longer claims the divergence is *forced* and
   instead records it as a brand token in model-facing prose, with both spellings live in the
   environment; the two tag literals, the tag count of two in the
   `prompt_snippet`…`prompt_guidelines` window, and the `rfind` target at `bash.rs:304` are all
   unchanged (§4.3), so
   `pi_tool_semantics.rs::bash_prompt_guideline_deltas_are_tagged_cyrup_delta` and
   `bash_session_env.rs::cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag` stay
   green.
4. `bash.rs:249-250`'s "twelve … `env.rs:68-91`" reads **nine … `env.rs:96-128`** (§6.1).
5. No existing assertion in `bash_session_env.rs`, `tests/bash_env_scrub.rs`,
   `bash_session_env_wiring.rs` or `pi_schema.rs` is edited — the change is additive and none of them
   observes the `PI_*` half (§4.4).
