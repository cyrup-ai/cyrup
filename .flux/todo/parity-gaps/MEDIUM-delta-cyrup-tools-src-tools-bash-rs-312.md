---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:312"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 21:00
---

# The `AI_AGENT` marker in `<ShellTool as Tool>::execute` — retag, record, close

**Outcome of this pass: the filed premise is half wrong and half out of scope, and one real
defect is underneath it.**

- The **value** difference (`AI_AGENT="cyrup"` vs pi's `"pi"`) is not a gap. It is the correct port
  of pi's *documented* semantics, and it is the same naming policy that produced `CYRUP_SESSION_ID`
  nineteen lines below it. → **ACCEPT**, record the reason on the delta line.
- The **scope** difference (per-child at three sites vs pi's process-global) is a **decision this
  project already took and recorded** — `PARITY-GAPS` **PB-5**, with the competing proposal
  (`DRIFT-044`) explicitly rejected as a duplicate. It is not this site's, not this crate's, and not
  this item's. → **do not re-file**; cite it in-source so the next audit stops re-deriving it.
- What *is* wrong here is a **factual citation error**: the annotation says `AI_AGENT` arrives
  `@v0.84.1`. It arrives at **v0.84.0**. The wrong tag is written into three source annotations and
  **asserted as a string literal by three source-scan tests**, so a future `v0.84.x` uplift routes by
  it. → **fix the tag**. That is the code change.

---

## 0. Anchors — every one re-derived at HEAD this pass

`:312` in the title is mid-comment. Anchor by symbol.

| symbol | file:line |
| --- | --- |
| `<ShellTool as Tool>::execute` | [`crates/cyrup-tools/src/tools/bash.rs:266`](../../../crates/cyrup-tools/src/tools/bash.rs) |
| prose block (identity markers, non-delta) | `bash.rs:291-302` |
| `env.push(("PI_CODING_AGENT", "true"))` | `bash.rs:303` |
| `[CYRUP-DELTA` annotation opens | `bash.rs:304` |
| `env.push(("AI_AGENT", "cyrup"))` | `bash.rs:315` |
| `let env_remove = crate::config::session_env_scrub_keys();` | `bash.rs:316` |
| `run_bash` / its `AI_AGENT` push | [`crates/cyrup-session-svc/src/bash.rs:148`](../../../crates/cyrup-session-svc/src/bash.rs) / `:183` (delta opens `:174`) |
| `env_identity_and_depth` / its `AI_AGENT` insert | [`crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:913`](../../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) / `:972` (delta opens `:963`) |
| `crates/cyrup/src/main.rs` — `#[tokio::main]` `:48`, `set_process_name` `:80`, `run()` `:100`, the declined-`set_var` comment `:104-110`, `predispatch::classify_internal` `:136` | [`crates/cyrup/src/main.rs`](../../../crates/cyrup/src/main.rs) |

> **Citation drift corrected from the previous revision of this file.** It gave the immediate-bash
> push as `cyrup-session-svc/src/bash.rs:173` (it is **`:183`**; `:173` is the `PI_CODING_AGENT`
> push) and the subagent insert as `spawn_plan.rs:967` (it is **`:972`**), and
> `env_identity_and_depth` as `:908` (it is **`:913`**). Re-verify by grep, not by memory:
> `grep -n 'AI_AGENT' crates/cyrup-session-svc/src/bash.rs crates/cyrup-ext-subagents/src/exec/spawn_plan.rs`.

## 1. Reference tree, pinned

[`./tmp/pi`](../../../tmp/pi) at **`e8682309`** = `packages/coding-agent` **0.84.3**
([`package.json:3`](../../../tmp/pi/packages/coding-agent/package.json)). Every pi citation below was
read in that tree at that commit; `git` was not run.

`AI_AGENT` occurs in six places tree-wide
(`grep -rn AI_AGENT ./tmp/pi --exclude-dir=node_modules`): two writes
([`src/cli.ts:14`](../../../tmp/pi/packages/coding-agent/src/cli.ts),
[`src/rpc-entry.ts:8`](../../../tmp/pi/packages/coding-agent/src/rpc-entry.ts), both
`process.env.AI_AGENT = "pi";` at module top level), and four doc lines
([`docs/environment-variables.md:15`](../../../tmp/pi/packages/coding-agent/docs/environment-variables.md),
`:18`; [`README.md:675`](../../../tmp/pi/packages/coding-agent/README.md);
[`CHANGELOG.md:288`](../../../tmp/pi/packages/coding-agent/CHANGELOG.md), `:123`). **Nothing in pi
reads it back.** Nothing in cyrup reads it back either (`grep -rn AI_AGENT crates/ --include=*.rs`
returns three writes plus tests). The variable is outward-facing only.

Relevant shape of the shell tool at that commit — the code this whole item is a footnote to:

- [`core/tools/bash.ts:170-195`](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)
  `resolveSpawnContext`: `:177` `const env = { ...getShellEnv() };` → `:178-182` unconditionally
  `delete env.PI_SESSION_ID / PI_SESSION_FILE / PI_PROVIDER / PI_MODEL / PI_REASONING_LEVEL` →
  `:183-193` repopulate them when `exposeSessionEnvironment && ctx` → `:194-195` `spawnHook` last.
- `getShellEnv()` ([`utils/shell.ts:138-150`](../../../tmp/pi/packages/coding-agent/src/utils/shell.ts))
  returns `{ ...process.env, [pathKey]: updatedPath }` — which is *why* `AI_AGENT` reaches the bash
  child upstream at all: it is **inherited**, never pushed.
- `resolveSpawnContext` is called from `bash.ts:363`, inside `createShellToolDefinition`, which
  `powershell.ts:53` also calls — the shared engine, mirrored by cyrup's shared `ShellTool`
  ([`crates/cyrup-tools/src/tools/powershell.rs:49-51`](../../../crates/cyrup-tools/src/tools/powershell.rs)).

The five session keys and their `PI_*` → `CYRUP_*` rename are **not this item** — see
[`MEDIUM-delta-cyrup-tools-src-tools-bash-rs-236.md`](./MEDIUM-delta-cyrup-tools-src-tools-bash-rs-236.md).
That rename is settled policy and is not reopened here; it is cited below only as the precedent that
settles the `AI_AGENT` value.

---

## 2. Disposition of the two filed differences

### 2.1 VALUE — **ACCEPT.** `"cyrup"` is the port; `"pi"` would be the bug.

pi's own documentation defines the split, verbatim:

> `AI_AGENT=pi` is a generic marker that lets tooling identify Pi as **the agent that launched the
> process**. (`docs/environment-variables.md:15`)

and `README.md:675`: "Set to `pi` by the CLI and RPC entry points **so generic tooling can attribute
child processes to Pi**". The KEY is vendor-neutral; the VALUE is the running agent's own name, and
the whole point of the variable is attribution. Porting the *semantics* into a binary called `cyrup`
yields `"cyrup"`. Porting the *literal* would make every cyrup child claim to have been launched by a
program that is not running — a falsehood in the one field whose stated purpose is truthful
attribution. The key arrived upstream from an outside contributor (`CHANGELOG.md:288`, #7493 by
`@renaudhartert-db`), i.e. it is a cross-vendor convention, which sharpens the point rather than
softening it.

This is the same policy that already governs `CYRUP_SESSION_ID` nineteen lines below the push
(`bash.rs:334`) and the read-side `CYRUP_*`-then-`PI_*` fallback chain in
[`crates/cyrup-config/src/env.rs:68-91`](../../../crates/cyrup-config/src/env.rs). **There is no
decision to escalate.** The previous revision of this file asked for a human ruling between
`"cyrup"` and `"pi"`; pi's own docs answer it, and the project's naming convention answers it twice.
Delete the question, record the answer in-source.

*(For a hook asking "is a pi-family agent running?", the answer is already correct today:
`PI_CODING_AGENT="true"` is pushed verbatim one line above, `bash.rs:303`. A hook branching on
`AI_AGENT == "pi"` is asking **which** agent, and `cyrup` is the honest answer.)*

### 2.2 SCOPE — **already decided, elsewhere. Do not re-file here.**

The difference is real: pi writes to `process.env` before `main()`, so *every* descendant inherits
the marker; cyrup writes it per child at exactly three sites, so an MCP stdio server
([`crates/cyrup-mcp/src/runtime.rs:1386-1400`](../../../crates/cyrup-mcp/src/runtime.rs)), an
extension process, the intercom broker, an editor, a clipboard helper or the update check sees
nothing.

**But the placement question was raised, contested and resolved, and this item is downstream of that
resolution:**

- [`docs/gap-analysis/PARITY-GAPS.md:397-401`](../../../docs/gap-analysis/PARITY-GAPS.md) — **PB-5**,
  which owns exactly this: "The unsafe-`set_var` rationale covers *process-global* mutation only —
  `bash.rs` already builds a per-child env vector, so both keys can be added there with **no
  `unsafe`**."
- [`docs/gap-analysis/12-upstream-drift-pi-core.md:1225`](../../../docs/gap-analysis/12-upstream-drift-pi-core.md)
  — area 12 proposed `DRIFT-044` with the **opposite** fix ("explicitly do **not** do that") and
  **rejected it as a duplicate**, ruling: "**Resolve inside PB-5**, and decide the placement question
  there."
- [`docs/gap-analysis/04-cyrup-tools.md:197`](../../../docs/gap-analysis/04-cyrup-tools.md) —
  `TOOL-031` **CLOSED 2026-08-15**, all three per-child halves landed. `PARITY-GAPS.md:170-174`
  records the same closure path.
- The in-source rationale at `crates/cyrup/src/main.rs:104-110` is *specifically* scoped:
  `std::env::set_var` is `unsafe` under edition 2024; the process-**name** half is a syscall and was
  ported (`set_process_name`, `:80`, called `:110`), the env half was routed to PB-5.

So the narrowing is the **accepted consequence of a recorded decision**, not an unauthorized agent
artifact. Two consequences for this task:

1. **Do not prescribe making the markers process-global from here.** The previous revision of this
   file did exactly that — replace `#[tokio::main]` (`main.rs:48`) with a hand-built runtime and put
   two `unsafe std::env::set_var` calls in a single-threaded prologue above
   `predispatch::classify_internal` (`:136`). That is technically sound Rust, and it is still the
   wrong move *from this file*: it reverses PB-5's ruling, re-files the position area 12 already
   rejected, and lands a binary-entry refactor out of a `cyrup-tools` comment marker. If the scope is
   ever revisited, it is revisited in PB-5, by whoever owns `crates/cyrup/src/main.rs`.
2. **Record the decision at the site**, so the next `CYRUP-DELTA` sweep reads a citation instead of
   re-deriving the whole analysis for the third time. That is §4.1.

---

## 3. The one real defect — the tag is off by one minor (`@v0.84.1` → `@v0.84.0`)

All three write-site annotations and all three source-scan tests assert `AI_AGENT` arrives at
`@v0.84.1`. It arrives at **v0.84.0**. Evidence, in the pinned tree:

- `CHANGELOG.md:288` — "Added `AI_AGENT=pi` to CLI and RPC child-process environments for generic
  agent attribution … (#7493)".
- Section headings: `## [0.84.0] - 2026-08-06` at `:188`; the next heading below it is
  `## [0.83.0] - 2026-07-29` at `:384`. **188 < 288 < 384**, so `:288` is inside **0.84.0**.
  (`## [0.84.1] - 2026-08-07` is at `:157`, i.e. *above* — 0.84.1's block is `:157-187` and contains
  no `AI_AGENT` line; the tree-wide grep in §1 finds only `:123` and `:288` in the whole file.)
- `CHANGELOG.md:123` — "Documented the generic `AI_AGENT=pi` process marker … (#7747)" — sits
  between `## [0.84.2]` (`:99`) and `## [0.84.1]` (`:157`), so **0.84.2 merely documented** what
  0.84.0 shipped.

The load-bearing half of the annotation — *absence at the ported v0.83.0 baseline* — is unaffected
and stays. Only the arrival tag moves. It matters because the tests assert the literal string, so the
wrong tag is now **enforced**, and a `v0.84.x` uplift asking "what did 0.84.1 bring?" would find this
site listed under a release that did not touch it.

---

## 4. Required implementation path — the only one

Four edits and one guard. No new files, no new test crate, no runtime behaviour change: every
`AI_AGENT` push stays exactly as it is, with exactly the value it has.

### 4.1 Edit A — rewrite the annotation at `crates/cyrup-tools/src/tools/bash.rs:304-314`

Replace the eleven comment lines between the `PI_CODING_AGENT` push (`:303`) and the `AI_AGENT` push
(`:315`) with the block below. **Leave `:303` and `:315` byte-identical** — both are asserted as
literals by `cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag`.

```rust
        // [CYRUP-DELTA — KEY *and* value; the key is a FORWARD-PORT from `cli.ts:14` @v0.84.0, which
        // is AHEAD of the ported tag] `AI_AGENT` does not exist anywhere in pi @v0.83.0
        // (`git -C pi grep -n AI_AGENT v0.83.0 -- packages/` → 0 hits; `cli.ts:13` @v0.83.0 sets
        // only `PI_CODING_AGENT`), so cyrup writes a variable into every bash child that the ported
        // baseline never wrote.
        //
        // TAG (corrected): `CHANGELOG.md:288` — "Added `AI_AGENT=pi` to CLI and RPC child-process
        // environments" (#7493) — sits under `## [0.84.0]` (`:188`), above `## [0.83.0]` (`:384`).
        // v0.84.2 only DOCUMENTED it (`CHANGELOG.md:123`, #7747). This line read `@v0.84.1` until it
        // was re-derived against the pinned tree; it was off by one minor.
        //
        // VALUE — `"cyrup"`, not `"pi"`: AUTHORIZED, not incidental. pi defines the KEY as generic
        // and the VALUE as the agent that launched the process ("a generic marker that lets tooling
        // identify Pi as the agent that launched the process", `docs/environment-variables.md:15`;
        // `README.md:675`). Porting the SEMANTICS into a binary named `cyrup` yields `"cyrup"`;
        // porting the LITERAL would make every cyrup child claim it was launched by a program that
        // is not running, in the one field whose stated purpose is attribution. Same policy as the
        // `CYRUP_SESSION_*` keys below and as `cyrup-config/src/env.rs`'s read-side ordering. A hook
        // asking "is a pi-family agent running?" is answered by `PI_CODING_AGENT` on the line above.
        // Settled — do not re-open per-variable.
        //
        // SCOPE — pi sets this on `process.env` (`cli.ts:14`, `rpc-entry.ts:8`) so EVERY descendant
        // inherits it via `getShellEnv()`'s `{...process.env}` (`utils/shell.ts:138-150`, consumed at
        // `bash.ts:177`). cyrup writes it per child at three sites — here,
        // `cyrup-session-svc/src/bash.rs:183`, `cyrup-ext-subagents/src/exec/spawn_plan.rs:972` — so
        // children spawned elsewhere (MCP servers, extension processes, the intercom broker) see no
        // marker. That is the ACCEPTED CONSEQUENCE of PARITY-GAPS PB-5's placement ruling — per-child
        // env vector rather than `unsafe std::env::set_var` (`crates/cyrup/src/main.rs:104-110`) —
        // taken after area 12 rejected the opposite proposal (`DRIFT-044`) as a duplicate and routed
        // it into PB-5; `TOOL-031` closed on that basis. Revisit it in PB-5, not here.
        //
        // Stated on the delta line itself rather than only in the prose above (CFG-069) so a later
        // v0.84.x uplift reads this as ALREADY-PORTED-EARLY and not as already-done-at-tag. Same
        // class as the `working-start`/`working-stop` precedent.
```

Also fix the prose line **`bash.rs:293`**: `(added at v0.84.1, mirrored in rpc-entry.ts:7-8)` →
`(added at v0.84.0, mirrored in rpc-entry.ts:7-8)`.

### 4.2 Edit B — `crates/cyrup-session-svc/src/bash.rs`

Same correction, same shape, in `run_bash` (`:148`): the prose at `:164` and the delta block
`:174-182`. Every `v0.84.1` that refers to `AI_AGENT`'s arrival becomes `v0.84.0` (`:164`, `:174`,
`:180`, `:181`), and the block carries the same TAG / VALUE / SCOPE paragraphs. Leave `:173` and
`:183` byte-identical.

### 4.3 Edit C — `crates/cyrup-ext-subagents/src/exec/spawn_plan.rs`

Same, in `env_identity_and_depth` (`:913`): the prose at `:950` and the delta block `:963-971`
(`v0.84.1` at `:950`, `:963`, `:969`). Leave `:962` and `:972` byte-identical. Also the test-doc
occurrences at `:3761`, `:3797`, `:3803`.

### 4.4 Edit D — the guard

**The guard is a one-token flip of an existing assertion, and it is RED until Edit A lands:**

[`crates/cyrup-tools/src/tests/bash_session_env.rs:334`](../../../crates/cyrup-tools/src/tests/bash_session_env.rs)

```rust
        annotation.contains("@v0.84.0"),
```

Update that test's doc prose at `:296` and `:304` to match. With `:334` flipped and `bash.rs:304`
untouched, `cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag` fails on *"the delta
line must state the TAG the key comes from"* — that is the whole proof.

The two sibling source-scan tests move in lockstep or they go red for the same reason:
`crates/cyrup-session-svc/src/tests/summarization_retry_events.rs:756` (doc `:697`, `:734`) and
`crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:3822` (doc `:3761`, `:3797`, `:3803`). These are
not additional deliverables; they are the same literal in three places.

### 4.5 Invariants the rewrite MUST NOT break

`cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag`
(`bash_session_env.rs:315-345`) slices `src[..find(AI_AGENT push)]` and then takes everything from the
**last** `rfind("[CYRUP-DELTA")` in that slice. Therefore:

1. **Do not introduce a second `[CYRUP-DELTA` token between `:304` and the push.** It would move
   `rfind` forward and silently drop the tag / key / `v0.83.0` facts out of the asserted window.
2. The window must still contain, literally: **`@v0.84.0`**, **`AI_AGENT`**, **`v0.83.0`**. The block
   in §4.1 contains all three (`v0.83.0` appears twice in its first paragraph).
3. `env.push(("PI_CODING_AGENT".to_string(), "true".to_string()));` must remain byte-identical —
   asserted at `:318-321` as *presence before absence*, so the test cannot be satisfied by deleting
   the forward-ported marker.
4. `env.push(("AI_AGENT".to_string(), "cyrup".to_string()));` must remain byte-identical — it is the
   `find()` anchor at `:323-326` and the literal the two runtime tests observe as `[true][cyrup]`
   (`:240`, `:265`).
5. Nothing moves into or out of the `expose_session_environment` gate (`:317`), and nothing is added
   to `session_env_scrub_keys()` (`:316`) — pi does not scrub these two.
6. `bash_prompt_guideline_deltas_are_tagged_cyrup_delta`
   ([`crates/cyrup-tools/src/tests/pi_tool_semantics.rs:778`](../../../crates/cyrup-tools/src/tests/pi_tool_semantics.rs))
   scans only the doc block between `prompt_snippet` and `prompt_guidelines` and is unaffected —
   provided Edit A stays inside `execute` and does not touch `bash.rs:195-249`.

### 4.6 Explicitly NOT in this change

- The `v0.84.1` citations at `bash.rs:195`, `:207`, `:225`, `:229` and the test
  `the_guideline_uses_pi_v0_84_1_softened_phrasing` (`bash_session_env.rs:206`). Those are
  **TOOL-043**, a different upstream change (the prompt-guideline wording + const hoist), whose tag
  this pass did **not** re-derive. Do not retag them on the strength of §3.
- `crates/cyrup/src/main.rs`. See §2.2.
- The `PI_*` → `CYRUP_*` session-key rename and `session_env_scrub_keys`. See
  [`MEDIUM-delta-cyrup-tools-src-tools-bash-rs-236.md`](./MEDIUM-delta-cyrup-tools-src-tools-bash-rs-236.md).

---

## 5. Found this pass, routed elsewhere — not to be fixed here

- **`bash.rs:279` cites `docs/environment-variables.md:27`** for "The values are resolved when each
  command starts…". In the pinned tree that sentence is at **`:32`**. It sits in the session-key
  branch, which belongs to the `bash.rs:236` item; fixing it here would collide. Route it there.
- **The SDK-embedder inversion.** pi states its markers are "not set automatically when Pi is
  embedded through the SDK" (`docs/environment-variables.md:18`), so an SDK embedder gets them in
  *no* child. cyrup's per-child writes mean an embedder of `cyrup-tools` / `cyrup-session-svc` /
  `cyrup-ext-subagents` **does** get them. cyrup is behind pi outside the three sites and ahead of it
  inside them. This is a *surplus*, costs nothing, and keeps the tools self-contained — every test in
  §4.4 exercises the tools in-process, never through the binary, so the pushes are load-bearing for
  the crate regardless. Recorded, not actioned.
- **The audit's headline illustration is unverifiable against pi.** pi at `e8682309` has no MCP
  subsystem (`grep -rli mcp ./tmp/pi/packages/*/src` → two unrelated files; no `StdioClientTransport`,
  no client, no server manager), so "a detection script inside an MCP server sees no agent marker"
  describes a real cyrup consequence with **no upstream counterpart to measure against**. The §2.2
  finding stands on the other children. When the INDEX row is closed out, re-word it.
- **`docs/gap-analysis/04-cyrup-tools.md:197`'s citation of `exec/mod.rs:1961-1963`** is stale — the
  subagent write now lives at `exec/spawn_plan.rs:962`/`:972`. Cosmetic; note it if that file is
  touched for another reason.

---

## 6. Definition of done

1. `bash.rs:293` and the `[CYRUP-DELTA` block at `bash.rs:304-314` are rewritten per §4.1: the
   arrival tag reads `@v0.84.0`, the VALUE paragraph records `"cyrup"` as authorized-by-semantics
   with the `docs/environment-variables.md:15` / `README.md:675` citation, and the SCOPE paragraph
   cites PB-5 / `DRIFT-044` / `TOOL-031` as the recorded placement decision.
2. The same corrections land at `cyrup-session-svc/src/bash.rs:164`, `:174-182` and
   `cyrup-ext-subagents/src/exec/spawn_plan.rs:950`, `:963-971`, `:3761`, `:3797`, `:3803`.
   Afterwards `grep -rn 'v0\.84\.1' crates/cyrup-session-svc/src/bash.rs
   crates/cyrup-ext-subagents/src/exec/spawn_plan.rs` returns nothing.
3. `bash_session_env.rs:334` asserts `"@v0.84.0"`; `summarization_retry_events.rs:756` and
   `spawn_plan.rs:3822` likewise. Reverting any one annotation turns its test red — that is the
   guard, and it is the only new assertion state this item introduces.
4. **Zero runtime change.** `bash.rs:303`/`:315`, `cyrup-session-svc/src/bash.rs:173`/`:183` and
   `spawn_plan.rs:962`/`:972` are byte-identical before and after. The four runtime assertions
   (`bash_session_env.rs:240`, `:265`; `summarization_retry_events.rs:707`; `spawn_plan.rs:3772`)
   still observe `[true][cyrup]` / `Some("cyrup")` unchanged, and `round9_l5res.rs:613` (assertion
   `:669`, key-presence only) is untouched.
5. `cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag` and its two siblings pass,
   with the §4.5 invariants intact — in particular exactly **one** `[CYRUP-DELTA` token between
   `bash.rs:303` and `bash.rs:315`.
6. The INDEX row for `crates/cyrup-tools/src/tools/bash.rs:312` is closed as **ACCEPT** (option 2 of
   the three in [`INDEX.md`](./INDEX.md)) — divergence authorized, reason annotated — **not** as
   "close it". Nothing about this item needs a human ruling: pi's own documentation fixes the value,
   and PB-5 already fixed the placement.
