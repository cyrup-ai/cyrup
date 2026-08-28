---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:312"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 02:10
---

# Capability gap: the `AI_AGENT` marker — `<ShellTool as Tool>::execute`

**Anchor by symbol, not by line.** The title says `bash.rs:312`. At HEAD the `[CYRUP-DELTA`
annotation opens at `crates/cyrup-tools/src/tools/bash.rs:304` and the write itself is `:315`,
inside `<ShellTool as Tool>::execute` (declared `:266`). `:312` is mid-comment. Every anchor below
is a symbol plus a line as of this pass.

Classified a **capability gap** — a caller can observe a difference — by the audit that reviewed
all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an agent. Nobody
decided it was acceptable. It is filed here so it is a decision rather than an artifact.

---

## 0. Reference tree, pinned

`./tmp/pi` at **`e8682309`** is `packages/coding-agent` **0.84.3** (`packages/coding-agent/package.json:3`),
with an `[Unreleased]` section above `## [0.84.3] - 2026-08-24` (`packages/coding-agent/CHANGELOG.md:3`, `:18`).
Every pi citation below was read in that tree at that commit. `git` was not run.

`AI_AGENT` occurs in exactly **six** places in the whole reference tree
(`grep -rn "AI_AGENT" ./tmp/pi --exclude-dir=node_modules`): two source writes and four documentation
lines. There is no third write site, no conditional, no settings key, and nothing reads it back.

| where | what |
| --- | --- |
| `packages/coding-agent/src/cli.ts:14` | `process.env.AI_AGENT = "pi";` |
| `packages/coding-agent/src/rpc-entry.ts:8` | `process.env.AI_AGENT = "pi";` |
| `packages/coding-agent/docs/environment-variables.md:15` | semantics |
| `packages/coding-agent/docs/environment-variables.md:18` | inheritance + the SDK carve-out |
| `packages/coding-agent/README.md:675` | table row |
| `packages/coding-agent/CHANGELOG.md:288`, `:123` | added; documented |

---

## 1. What pi does — the VALUE half

Both write sites are **module-top-level statements**, executed at import time, before `main()` is
called:

```
// packages/coding-agent/src/cli.ts:12-14
process.title = APP_NAME;
process.env.PI_CODING_AGENT = "true";
process.env.AI_AGENT = "pi";
```

`packages/coding-agent/src/rpc-entry.ts:6-8` is the identical trio with `process.title =
`${APP_NAME}-rpc``. The value is the literal `"pi"`, unconditionally, with no settings override and
no env-respecting fallback (it *overwrites* an inherited `AI_AGENT`).

Documented semantics, verbatim (`docs/environment-variables.md:15`):

> `AI_AGENT=pi` is a generic marker that lets tooling identify Pi as the agent that launched the process.

and (`README.md:675`) "Set to `pi` by the CLI and RPC entry points so generic tooling can attribute
child processes to Pi". The KEY is deliberately vendor-neutral; the VALUE is the vendor's own name.
That reading matters for §7.

### Correction to the tag every cyrup site cites (divergence **D1**)

All three cyrup write sites and three source-scan tests assert the key arrives at **`@v0.84.1`**.
That is off by one minor version. At `e8682309`:

- `CHANGELOG.md:288` — "Added `AI_AGENT=pi` to CLI and RPC child-process environments for generic
  agent attribution … ([#7493] by @renaudhartert-db)".
- Section headings: `## [0.84.0] - 2026-08-06` at `:188`, and the next heading below it is
  `## [0.83.0] - 2026-07-29` at `:384`. Line 288 is inside **0.84.0**.
- `CHANGELOG.md:123` — "Documented the generic `AI_AGENT=pi` process marker …" ([#7747]) — sits
  between `## [0.84.2]` (`:99`) and `## [0.84.1]` (`:157`), i.e. **0.84.2** merely documented it.

So the forward-port is from **v0.84.0**, not v0.84.1. The *absence* at the ported v0.83.0 baseline —
the load-bearing half of the annotation — is unaffected and still correct.

---

## 2. What pi does — the SCOPE half

Because the write lands on `process.env`, the scope is **every descendant process, whichever code
spawns it**. Node inherits the parent environment by default, and pi's two explicit-`env` sites
re-spread `process.env` rather than replacing it. Verified spawn sites at `e8682309`:

| spawn site | env handling | inherits `AI_AGENT` |
| --- | --- | --- |
| `utils/shell.ts:138-150` `getShellEnv()` | returns `{...process.env, [pathKey]: updatedPath}` | yes |
| `core/tools/bash.ts` (`env: env ?? getShellEnv()`) — `bash` + `powershell` | via `getShellEnv()` | yes |
| `core/exec.ts:41` `spawn(command, args, {...})` | no `env` option → inherit | yes |
| `core/tools/find.ts:269` (`fd`) | `{ stdio: [...] }` only | yes |
| `core/tools/grep.ts:226` (`rg`) | `{ stdio: [...] }` only | yes |
| `modes/rpc/rpc-client.ts:94-96` | `env: { ...process.env, ...this.options.env }` | yes |
| `modes/interactive/external-editor.ts:26` | inherit | yes |
| `modes/interactive/session-share.ts:168` (`gh gist create`) | inherit | yes |
| `modes/interactive/interactive-mode.ts:1213` (`tmux show -gv`) | inherit | yes |
| `utils/clipboard.ts:135` (`wl-copy`), and its `execFileSync`/`execSync` calls | inherit | yes |
| `utils/open-browser.ts:21` | inherit, detached | yes |

**The one condition under which pi does NOT set it**, stated by pi itself
(`docs/environment-variables.md:18`):

> Child processes inherit both markers. They are not session-specific and are not set automatically
> when Pi is embedded through the SDK.

i.e. an embedder that imports the agent without going through `cli.ts` / `rpc-entry.ts` gets **no
markers at all**, in any child, including bash-tool children. Remember this for §4.

**pi at `e8682309` has no MCP subsystem.** `grep -rli mcp ./tmp/pi/packages/*/src` returns two
unrelated files (`packages/ai/src/auth/oauth/anthropic.ts`,
`packages/coding-agent/src/utils/tool-result-images.ts`); there is no `StdioClientTransport`, no MCP
client, no server manager. The audit's headline example — "a detection script inside an MCP server
sees no agent marker" — is therefore **true of cyrup but has no pi counterpart to be measured
against** (divergence **D4**, below). The scope gap is real; that particular illustration is not a
pi-parity observation.

---

## 3. What cyrup does — the VALUE half

`crates/cyrup/src/main.rs` declines the process-global write outright. `run()` (`:100`) says so at
`:104-110`: `std::env::set_var` is `unsafe` under edition 2024, so only `process.title`'s analogue
(`set_process_name`, `:80`, called `:110`) was ported and "the env half is TOOL-031 / PARITY-GAPS
PB-5". There is **no global `AI_AGENT` anywhere in cyrup**.

Instead there are exactly **three** per-child writes, all with the literal value `"cyrup"`:

| # | symbol | file:line | which children |
| --- | --- | --- | --- |
| 1 | `<ShellTool as Tool>::execute` (`:266`) | `crates/cyrup-tools/src/tools/bash.rs:315` | the `bash` **and** `powershell` tool children — one shared engine, `ShellTool::powershell` at `crates/cyrup-tools/src/tools/powershell.rs:47-51` reuses it with `POWERSHELL_CONFIG` (`:30`) |
| 2 | `run_bash` (`:138`) | `crates/cyrup-session-svc/src/bash.rs:173` | the immediate-bash seam — TUI `!!cmd` and RPC `executeBash` |
| 3 | `env_identity_and_depth` (`:908`) | `crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:967` | the re-exec'd subagent child, via `env_overlay` |

Each is paired with `PI_CODING_AGENT = "true"` written verbatim one line above (`bash.rs:303`,
`cyrup-session-svc/src/bash.rs:163`, `spawn_plan.rs:957`).

**Nothing in cyrup reads `AI_AGENT` back.** The only readers are the tests in §9. The divergence is
purely outward-facing: it is observable by user hooks and scripts and by nothing else.

---

## 4. What cyrup does — the SCOPE half (the part that is easy to get wrong)

The three sites in §3 are the **entire** reach. Every other child cyrup spawns runs with no agent
marker at all. Verified spawn sites carrying neither key
(`grep -rn "AI_AGENT" crates/*/src` returns only the three writes plus tests):

- **MCP stdio servers** — `spawn_stdio_transport`, `crates/cyrup-mcp/src/runtime.rs:1386-1400`.
  This is the **only** cyrup spawn path that calls `env_clear()` (`:1391`); it then sets
  `StdioTransportSpec::env`, which `StdioTransportSpec::resolve` (`:1337`) builds from
  `crate::secrets::resolve_stdio_env(entry, server_name, base)` over
  `base = crate::secrets::process_env_snapshot()` (`crates/cyrup-mcp/src/secrets.rs:327-331`,
  `std::env::vars_os()`). So it does not *inherit* — but it does copy the live process env, which
  is precisely the hook a global fix needs (§6).
- Extension process capability — `crates/cyrup-ext/src/caps/proc.rs:514-526` (overlay via `.env()`).
- Detached background subagent — `crates/cyrup-ext-subagents/src/background/spawn_detached.rs:224`
  (overlay, "never `env_clear`", `:195`).
- `crates/cyrup-intercom/src/transport/spawn.rs`;
  `crates/cyrup-ext-subagents/src/watchdog/lsp_diagnostics.rs`;
  `crates/cyrup-resources/src/package/install.rs`;
  `crates/cyrup-tui/src/{tmux.rs,clipboard.rs,open_browser.rs,image.rs}`;
  `crates/cyrup/src/update_check.rs`;
  `crates/cyrup-session-svc/src/{host_services.rs,session/files.rs}`.

**Composition note.** The uncovered set is not the same shape as pi's inheriting set: cyrup's `grep`
and `find` are **in-process** (`crates/cyrup-tools/src/tools/grep.rs:215-230` drives the `ignore`/
`grep` crates from `spawn_blocking`), where pi spawns `rg`/`fd`. Two of pi's marker-inheriting
children simply have no cyrup analogue. Conversely cyrup has whole subsystems (MCP, intercom,
LSP watchdog, package install) that pi has no counterpart for — so "which children see it differs"
cannot be reduced to a diff of two lists; it is *global vs. three sites*.

**The direction of the gap INVERTS for the embedded case (divergence D2).** pi documents that an SDK
embedder gets the markers in **no** child (`docs/environment-variables.md:18`). cyrup's per-child
push means an embedder of `cyrup-tools` / `cyrup-session-svc` / `cyrup-ext-subagents` **does** get
them in bash, immediate-bash and subagent children. So cyrup is *behind* pi outside the three sites
and *ahead* of pi inside them. None of the three annotations says this.

---

## 5. The two differences, stated exactly

| | pi @ `e8682309` | cyrup @ HEAD | observable to |
| --- | --- | --- | --- |
| **value** | `AI_AGENT="pi"`, unconditional literal (`cli.ts:14`, `rpc-entry.ts:8`) | `AI_AGENT="cyrup"`, unconditional literal (three sites) | any hook/script that branches on `"$AI_AGENT" = pi` |
| **scope** | process-global → **every** descendant, from any spawn site, whenever the process started via the CLI or RPC entry; **nothing** when embedded via SDK | three spawn sites only (shell tools, immediate-bash, subagent re-exec) — regardless of how the process started; MCP servers, extension processes, the intercom broker transport, editors, browsers, clipboard, npm installs and the update check see nothing | a detection script anywhere other than a shell-tool/subagent child |

The two are independent: closing the scope half does not decide the value, and vice versa.

---

## 6. Prescription — CLOSE the scope half. It is a parity bug, not a product choice.

Nothing about the scope difference is a branding decision. pi's contract is "child processes inherit
both markers"; cyrup silently narrows it to three call sites, and the narrowing is an artifact of
`std::env::set_var` being `unsafe`, which is a *how*, not a *whether*. Close it.

**6.1 — Set both markers process-globally at the binary entry.**
`crates/cyrup/src/main.rs` is the only legal home: the `cyrup` lib root is `#![forbid(unsafe_code)]`
and this file already carries `unsafe` for `set_process_name` (`:80-96`), for exactly the same
"pi does it at `cli.ts`, we need a syscall" reason.

The `unsafe`-ness of `set_var` is a **thread** hazard, not an absolute bar. `#[tokio::main]`
(`:48-49`) builds the multi-thread runtime *around* the body, so today there is no single-threaded
window inside `run()`. Replace the attribute with a hand-built runtime so there is one:

- `fn main() -> ExitCode` (no attribute), whose first statements are the two `set_var` calls in a
  provably single-threaded prologue — before any `Runtime::new`, before `predispatch`, before
  anything spawns a thread — with a `// SAFETY:` note saying so.
- then `tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run())`.

It must sit **above** `predispatch::classify_internal` (`:128`) so the `__subagent-runner` and
`__intercom-broker` re-execs are covered too — and note each of those is itself a `cyrup` binary, so
it re-asserts the pair at its own startup exactly as pi's `rpc-entry.ts:7-8` re-asserts what
`cli.ts:13-14` already set.

This single change covers everything in §4, including MCP: `process_env_snapshot()`
(`crates/cyrup-mcp/src/secrets.rs:327`) reads `std::env::vars_os()`, so the marker lands in the
`env_clear()`ed child through the snapshot; every other site is an overlay over an inherited env
(`crates/cyrup-tools/src/ops/local/command.rs:24-28` is the shell backend's `.env_remove()` /
`.env()` pattern) and picks it up for free.

**6.2 — Keep all three per-child writes.** They become belt-and-braces, not redundancy:
- they are `.env()` overlays over an inherited env, so they are idempotent with the global;
- they are what makes the tools correct for an SDK embedder whose process never runs
  `crates/cyrup/src/main.rs` — and every test in §9 exercises the tools **in-process**, never
  through the binary, so deleting them turns all four runtime assertions red for the wrong reason;
- the subagent overlay is documented assert-not-assume for a separate reason
  (`spawn_plan.rs:930-940`: an overlay entry that is merely omitted lets a parent's value leak).
This leaves a *residual, narrower* divergence — the SDK-case surplus of D2 — which should be stated
on the annotations rather than left implicit.

**6.3 — Retag.** `@v0.84.1` → `@v0.84.0` at all three write sites and in the three source-scan
tests that assert the string (§9). The v0.84.2 doc entry can be cited separately if useful.

**6.4 — Rewrite the three annotations** so they carry *scope* as well as *value*: after 6.1 the
scope delta is gone at the process level, and what remains to record is (a) whatever David decides
in §7 and (b) the SDK-case surplus.

**6.5 — Failing test first** (cannot be run here: cargo is off-limits this pass, 10 siblings /
7.7 G disk — these are prescriptions, not verifications):
- an integration test in `crates/cyrup-it` that runs the real `cyrup` binary and observes a
  **non-shell-tool** child's environment — the cheapest honest RED is an MCP stdio server whose
  command prints its own env, asserting `AI_AGENT` is present. RED today (only the three sites
  write it), GREEN after 6.1.
- a source scan on `crates/cyrup/src/main.rs` pinning that the prologue sets both keys *before* the
  runtime is built, so a later refactor back to `#[tokio::main]` fails loudly rather than silently
  re-narrowing the scope.

---

## 7. The VALUE half is **David's call** — options, with a recommendation

Presented, not pre-decided. The scope half (§6) closes regardless of which of these is chosen.

**V1 — keep `"cyrup"`, and record it as an explicitly authorized divergence.** *(recommended)*
pi's own documentation defines the KEY as vendor-neutral ("generic marker … identify **Pi** as the
agent that launched the process", `docs/environment-variables.md:15`) and the VALUE as the running
agent's own name. Porting the *semantics* of that line to a binary called `cyrup` yields `"cyrup"`;
porting the *literal* yields a marker that misattributes cyrup's children to a program that is not
running. `AI_AGENT` looks like a cross-vendor attribution convention (contributed upstream by an
outside contributor, `CHANGELOG.md:288`), which makes truthfulness the point of the variable.
Cost: a third-party hook hard-coded to `"pi"` takes the other branch.

**V2 — write `"pi"`.** Byte-exact parity; any pi-targeted hook works unmodified. Cost: cyrup claims
to be pi in a variable whose stated purpose is attribution, and it does so to every descendant once
§6 lands — the blast radius of the lie grows with the fix. Not recommended, but it is a coherent
position if cyrup is meant to be a drop-in under existing pi tooling.

**V3 — keep `"cyrup"` and lean on the key that already answers the family question.** *(recommended
alongside V1)* cyrup already writes `PI_CODING_AGENT="true"` verbatim at all three sites. A hook
asking "is a pi-family agent running?" is answered correctly today by `PI_CODING_AGENT`; a hook
branching on `AI_AGENT == "pi"` is asking "*which* agent", and the honest answer is `cyrup`. If V1 is
taken, this is the migration note to publish for hook authors — and it should be written into the
annotations so the next agent does not re-derive it.

**Recommended disposition:** close the scope half per §6; take **V1 + V3** for the value half as an
*explicitly authorized* divergence with the reason recorded on the annotation line — David's
signature, not an agent's.

---

## 8. Additional divergences found this pass (filed, not descoped)

- **D1 — wrong upstream tag, three sites and three tests.** `@v0.84.1` should be `@v0.84.0`
  (`CHANGELOG.md:288` inside `## [0.84.0]`, `:188`). Cheap to fix, and it matters because the tests
  assert the literal string and a future v0.84.x uplift will route by it. See §6.3.
- **D2 — the SDK-case inversion is undocumented.** pi sets nothing when embedded
  (`docs/environment-variables.md:18`); cyrup's per-child writes mean an embedder's bash children DO
  see the markers. cyrup is behind pi outside the three sites and ahead of it inside them, and no
  annotation says so. See §4, §6.2.
- **D3 — cyrup gives three different answers to one naming question.** It writes `PI_CODING_AGENT`
  **verbatim** into children (impersonating pi on the *pi-specific* key), refuses pi's value on the
  *generic* key (`AI_AGENT="cyrup"`), renames the five session keys `PI_*` → `CYRUP_*` and
  additionally *scrubs* the `PI_*` spellings from the child
  (`crate::config::session_env_scrub_keys`, applied at `bash.rs:316`), while on the READ side it
  accepts `CYRUP_*` first and `PI_*` last as a migration fallback
  (`crates/cyrup-config/src/env.rs:71-97`, e.g. `agent_dir: ["CYRUP_AGENT_DIR",
  "CYRUP_CODING_AGENT_DIR", "PI_CODING_AGENT_DIR"]`). Whatever David decides in §7 should be decided
  as a *policy* across these, not per-variable — otherwise the next agent re-litigates it at the
  next site.
- **D4 — the audit's own illustration is unverifiable against pi.** pi at `e8682309` has no MCP
  subsystem at all, so "a detection script inside an MCP server sees no agent marker" describes a
  real cyrup hole with no upstream counterpart. The scope gap stands on the other children
  (extension processes, intercom broker, editors, npm installs, update check); the MCP example
  should be re-worded when the marker is rewritten.

---

## 9. Tests — exactly which assertions move

Existing coverage, all verified present at HEAD:

| test | file:line | asserts | scope-close only (§6) | if V2 (`"pi"`) | if D1 retag |
| --- | --- | --- | --- | --- | --- |
| `bash_child_sees_the_agent_identity_markers` | `crates/cyrup-tools/src/tests/bash_session_env.rs:240` | child prints `[true][cyrup]` | unchanged | → `[true][pi]` | — |
| `identity_markers_survive_expose_session_environment_off` | `crates/cyrup-tools/src/tests/bash_session_env.rs:265` | child prints `[true][cyrup][]` | unchanged | → `[true][pi][]` | — |
| `cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag` | `crates/cyrup-tools/src/tests/bash_session_env.rs:315` | source contains `env.push(("AI_AGENT".to_string(), "cyrup".to_string()));`; annotation contains `@v0.84.1`, `AI_AGENT`, `v0.83.0` | annotation text changes (§6.4) — the three `contains` still hold if the rewrite keeps the tag + key + absence facts | the searched literal changes | **`@v0.84.1` → `@v0.84.0`** |
| `immediate_bash_carries_the_agent_identity_markers` | `crates/cyrup-session-svc/src/tests/summarization_retry_events.rs:707` | `execute_bash` output contains `[true][cyrup]` | unchanged | → `[true][pi]` | — |
| `the_forward_ported_ai_agent_marker_names_its_key_and_its_tag` | `crates/cyrup-session-svc/src/tests/summarization_retry_events.rs:741` | same shape as the `cyrup-tools` one, over `../bash.rs` | annotation text | literal | **retag** |
| `the_spawn_overlay_carries_the_agent_identity_markers` | `crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:3766` | `env_overlay["AI_AGENT"] == Some("cyrup")` | unchanged | → `Some("pi")` | — |
| `cfg069_the_spawn_overlay_delta_names_the_forward_ported_key_and_its_tag` | `crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:3802` | same shape, over `spawn_plan.rs` | annotation text | literal | **retag** |
| `execute_bash_routes_through_an_operations_override_instead_of_the_local_shell` | `crates/cyrup-session-svc/src/tests/round9_l5res.rs:610` (assertion `:666`) | env **key** `AI_AGENT` present | unchanged | unchanged (key only) | — |
| `bash_prompt_guideline_deltas_are_tagged_cyrup_delta` | `crates/cyrup-tools/src/tests/pi_tool_semantics.rs:778` | scans only the doc block between `prompt_snippet` and `prompt_guidelines` (`:786-793`) | **unaffected** — it does not see the `execute`-site delta | unaffected | unaffected |

**Summary of movement.** Under the recommended disposition (close scope, V1+V3 value, D1 retag) the
only assertions that move are the three `@v0.84.1` strings and whatever the rewritten annotations
must still contain; every runtime `[true][cyrup]` assertion stays exactly as it is, because §6.2
keeps the per-child writes. Under V2 six literals change instead.

**New tests required** (RED before the change, per §6.5):
1. `crates/cyrup-it` — real-binary integration: a **non-shell-tool** child (MCP stdio server) sees
   `AI_AGENT`. This is the test that fails without §6.1 and is the whole point of the item.
2. source scan on `crates/cyrup/src/main.rs`: the markers are set in the pre-runtime prologue,
   above `predispatch::classify_internal`.
3. optional, pins D2 honestly: an in-process test asserting the tool still stamps the markers with
   no global set — i.e. the SDK surplus is deliberate, so a later cleanup cannot delete the pushes
   as "redundant".

---

## 10. Definition of done

1. §6.1 lands: the markers are process-global at the `cyrup` binary entry, above the internal-
   subcommand dispatch, and the three per-child writes are kept (§6.2).
2. New test 1 (§9) fails without the change and passes with it.
3. D1 retag applied at all three write sites and the three source-scan tests (§6.3).
4. The three annotations are rewritten to record scope as well as value, including the SDK-case
   surplus (D2) and a corrected illustration (D4).
5. The value half carries **David's** recorded decision (§7) — V1+V3 recommended — written on the
   annotation line as an authorized acceptance, not an agent's assertion.
6. No behaviour regression in `cyrup-tools`, `cyrup-session-svc`, `cyrup-ext-subagents`, `cyrup-mcp`.

---

## 11. Open questions for David

1. **Value: V1+V3 (recommended), or V2?** Do we want cyrup's children to say `AI_AGENT=cyrup` (true,
   breaks hooks hard-coded to `pi`) or `AI_AGENT=pi` (drop-in, untrue)? This is the only genuinely
   product-shaped half of the item.
2. **D3 — is there one naming policy?** `PI_CODING_AGENT` verbatim + `AI_AGENT=cyrup` +
   `CYRUP_SESSION_*` (with `PI_SESSION_*` scrubbed) + read-side `CYRUP_*`-then-`PI_*` are four
   different answers. Should cyrup also write `CYRUP_CODING_AGENT="true"` alongside
   `PI_CODING_AGENT`, the way `crates/cyrup-config/src/env.rs` reads both spellings? Deciding this
   once settles this item and the sibling below.
3. **Reconciliation with the `PI_SESSION_ID` sibling** (`bash.rs:236`, being augmented in parallel —
   that file was not touched by this pass). The two items meet at the same `execute` body and at the
   same `[CYRUP-DELTA` grep: `:228-236` is the `PI_*` → `CYRUP_*` prompt-string/session-key
   divergence, `:304-315` is this one. Q2's answer governs both. Note for that item's author: this
   pass establishes that `AI_AGENT` arrived at **v0.84.0**, not v0.84.1 (D1), and that the *scope*
   of the identity markers — not just their spelling — diverges; if the `PI_SESSION_ID` item
   prescribes anything about `session_env_scrub_keys` or the guideline string, it should assume
   §6.1 makes the identity pair process-global while the five session keys stay tool-local (pi's
   own split: `resolveSpawnContext`, `bash.ts:158-184`, vs. `cli.ts:13-14`).
4. **Is the SDK surplus (D2) wanted?** Keeping the per-child writes after §6.1 makes cyrup strictly
   more informative than pi for embedders. Recommended (it costs nothing and keeps the tools
   self-contained), but it is a deliberate step past parity and should be authorized as such.
