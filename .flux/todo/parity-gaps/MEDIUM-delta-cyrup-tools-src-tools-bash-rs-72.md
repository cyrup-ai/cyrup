---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:72"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 02:11
---

# Capability gap: `crates/cyrup-tools/src/tools/bash.rs:72`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

---

## AUGMENTATION VERDICT (2026-08-28)

**PRESCRIPTION: CLOSE — and it is already closed in the working tree.**

The divergence this task describes **does not exist at the tree under audit today**.
`crates/cyrup-tools/src/tools/bash.rs` emits pi's `"Shell command to execute"` from a
single literal shared by both shell tools, and `crates/cyrup-tools/src/tests/pi_schema.rs`
pins that literal for `bash` AND `powershell` against one ground-truth constant. Both the
`command_description` field and its `[CYRUP-DELTA]` marker have been removed from the
source. There is nothing left to change.

This is a **finding, not a descope**. Proving there is no divergence is a legitimate
close. What follows is the evidence, the reason the audit saw a marker that is no longer
there, the (empty) blast radius, the exact test assertions that hold the line, and two
live divergences of the *same class* that this task does not cover and that nobody has
authorized.

---

## 1. Ground truth at `e8682309` — pi

Reference tree `./tmp/pi`. Commit confirmed without running `git`:
`tmp/pi/.git/HEAD` → `ref: refs/heads/main`; `tmp/pi/.git/packed-refs:2` →
`e86823096c5bad39e1ca282ec24bc5eb9bec745b refs/heads/main`; `tmp/pi/.git/logs/HEAD` (last
line) → `reset: moving to origin/main` at `e8682309…`. `packages/coding-agent/package.json:3`
→ `"version": "0.84.3"`, so `e8682309` **is** v0.84.3.

`tmp/pi/packages/coding-agent/src/core/tools/bash.ts:42-45`, verbatim bytes:

```ts
const bashSchema = Type.Object({
	command: Type.String({ description: "Shell command to execute" }),
	timeout: Type.Optional(Type.Number({ description: "Timeout in seconds (optional, no default timeout)" })),
});
```

ONE schema, shared. `bash.ts:328-336` — the upstream `ShellToolConfig` **has no
command-description field at all**:

```ts
export interface ShellToolConfig {
	name: string;
	label: string;
	shellName: string;
	prompt: string;
	promptSnippet: string;
	promptGuidelines?: readonly string[];
	tempFilePrefix: string;
}
```

The factory hands the module-level schema straight through — `bash.ts:353`
`parameters: bashSchema,` — and `powershell.ts` defines no schema of its own
(`grep -n "command:" powershell.ts` → 0 hits); `powershell.ts:49-57` calls
`createShellToolDefinition(cwd, powershellToolConfig, {…})`. So at `e8682309`
`bash` and `powershell` emit **byte-identical** `input_schema`.

### The `"Bash command to execute"` bytes DO still exist upstream — in a tool cyrup does not port

`tmp/pi/packages/agent/src/harness/tools/bash.ts:11-14`:

```ts
const bashSchema = Type.Object({
	command: Type.String({ description: "Bash command to execute" }),
	timeout: Type.Optional(Type.Number({ description: "Timeout in seconds (optional, no default timeout)" })),
});
```

That is `packages/agent`'s harness tool (`AgentHarnessTool`, `../types.ts`), a different
package with its own `executeShellWithCapture` engine. `cyrup-tools` ports
`packages/coding-agent/src/core/tools`, not `packages/agent/src/harness/tools`, and no
cyrup crate mirrors `AgentHarnessTool`. So the old wording is not a fabrication and not
only a v0.83.0 fossil — it is live upstream, on a **different tool**. It is simply the
wrong string for the tool cyrup actually ports. (Recorded because a future port of pi's
harness tools would legitimately reintroduce that byte sequence, and the next parity
sweep should not read it as a regression.)

Note on the archived rationale's "cyrup's baseline is v0.83.0, where the text was
`Bash command to execute`": **unverifiable in this checkout** — `tmp/pi` is at
`main`/`e8682309`, the `v0.83.0` tag is not checked out (`.git/logs/HEAD` shows it was,
then reset away), and the constraints forbid `git`. It is also moot: the pinned reference
for this backlog is `e8682309`, and there the answer is unambiguous.

## 2. Ground truth at HEAD — cyrup

`crates/cyrup-tools/src/tools/bash.rs:58-88` — `ShellToolConfig`'s fields are `name`,
`label`, `shell_name`, `prompt_snippet`, `prompt_guidelines`, `temp_file_prefix`,
`command_preamble`, `resolve_shell`. **There is no `command_description` field.**
`grep -rn command_description --include=*.rs crates/` returns zero hits in `cyrup-tools`
(the only matches are `cyrup-mcp`'s unrelated `prompt_command_description` and
`cyrup-ext`'s `command_descriptions`).

`bash.rs:132-142`, the schema built once in `ShellTool::new`:

```rust
        // Byte-for-byte Pi's TypeBox emission (bash.ts:42-45): verbatim descriptions,
        // `type:"number"`, no `minimum`, no `additionalProperties`. ONE schema for BOTH shells,
        // exactly as upstream hands the single `bashSchema` to `parameters` from the shared
        // factory (bash.ts:353) — which is why the `command` description is a literal here and
        // not a `ShellToolConfig` field.
        let params = serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string", "description": "Shell command to execute" },
                "timeout": { "type": "number", "description": "Timeout in seconds (optional, no default timeout)" }
            }
        });
```

`ShellTool::bash` (`bash.rs:166-167`) and `ShellTool::powershell`
(`powershell.rs:49-51`) both route through that one `Self::new`, and
`Tool::parameters` (`bash.rs:182-184`) returns that `params` unchanged.
`POWERSHELL_CONFIG` (`powershell.rs:30-45`) carries no command-description field either.

Whole-repo sweep, excluding `target/`, `tmp/`, `.flux/`, `.git/`:
`grep -rn "command to execute"` → exactly **two** hits, both the pi string:
`crates/cyrup-tools/src/tests/pi_schema.rs:63` and
`crates/cyrup-tools/src/tools/bash.rs:139`. No second copy of the schema exists in
`docs/`, `spec/`, or any other crate.

**Conclusion: zero bytes of divergence on this property description. The model-facing
JSON schema for `bash` and for `powershell` is pi's, exactly.**

## 3. Why the audit saw a marker at line 72

Not a phantom — the audit read a *pre-fix* revision of the file. Three facts line up:

1. **Uniform line drift of exactly 8.** The three sibling tasks anchor
   `bash.rs:214`, `:236`, `:312`. The three surviving `[CYRUP-DELTA` markers in the live
   file are at `:206`, `:228`, `:304`. Every anchor is high by 8.
2. **The removed block is 8 lines net.** The archived design
   (`.flux/done/2026-08-23-00-08/MEDIUM-the-entire-powershell-built-in-tool-is-missing-from-cyrup.md:755-764,789,832`)
   shows a 10-line struct member (`:755-763` doc + `:764` `pub command_description: &'static str,`)
   plus 1 line in `BASH_CONFIG` (`:789`); `:832` read `"description": config.command_description`
   where the live file has the literal plus 3 explanatory comment lines (`bash.rs:132-135`).
   10 + 1 − 3 = 8. (`:1052` is `POWERSHELL_CONFIG`'s copy — a different file, no bearing on
   `bash.rs` drift.)
3. **Line 72 lands on the marker.** The live file's `pub shell_name` is line 69 and the
   lines above it are untouched, so in the pre-fix file line 70 was
   ``/// The `command` property's schema description.``, line 71 was `///`, and line **72**
   was ``/// [CYRUP-DELTA — version lag, per-tool instead of shared] Pi shares ONE `bashSchema` between``.

The same workflow already noticed the aftermath: `MEDIUM-open-questions-from-gap-closure.md:51`
records that the archived task record "still contains the old 'Bash command to execute'
string and the command_description field. Historical task record, not a live claim", and
`:48` calls `cyrup-ext-sdk`'s `bash_descriptor` "a sibling of **ITEM 1's** exact defect" —
ITEM 1 being this one. So a closure agent in the `wf_12c49023-adf` wave closed it between
the marker inventory and now, and this task file was never refreshed.

`git` is forbidden here, so "who changed it and in which commit" is not established — only
that the live tree is aligned. Mtimes are useless (the entire checkout shares
`2026-08-28 01:53:55`).

## 4. Blast radius of the alignment — measured, and empty

The brief asks what the change breaks. Every candidate was checked against the live tree:

| candidate | result |
| --- | --- |
| `powershell` shares the config struct | **Nothing to do.** The field is gone from `ShellToolConfig`, so `POWERSHELL_CONFIG` (`powershell.rs:30-45`) never named it. `powershell` already asserted `"Shell command to execute"` under the archived design, so its bytes never moved. |
| `pi_schema.rs` pins the string | **Already the pi string.** `pi_schema.rs:59-63` is now a single `PI_SHELL` constant with a comment stating the sharing is the invariant. |
| `pi_tool_semantics.rs` pins the string | **False premise — it does not.** `grep -n "parameters(\|PI_SHELL\|command to execute" crates/cyrup-tools/src/tests/pi_tool_semantics.rs` → **zero hits**. That file carries no schema assertion at all; the description/snippet/guideline constants (`PI_BASH_DESCRIPTION`, `PI_BASH_SNIPPET`, `PI_BASH_GUIDELINES`) live in `pi_schema.rs:142-146`, and none of them contains the `command` property description. Nothing in `pi_tool_semantics.rs` moves. |
| `pi_tool_semantics.rs` source-scan test counts markers in `bash.rs` | **Out of range.** `bash_prompt_guideline_deltas_are_tagged_cyrup_delta` (`:778-840`) scans only `src[find("fn prompt_snippet(")..find("fn prompt_guidelines(")]` — `bash.rs:191..251`. The deleted marker was a **struct field** doc at `bash.rs:~72`, far above the window (and `find("fn prompt_snippet(")` cannot match the struct field `pub prompt_snippet:`). Its `assert_eq!(tags.len(), 2)` is unaffected in both directions. |
| `bash_session_env.rs` source-scan test | **Out of range.** `cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag` (`:314-345`) takes `rfind("[CYRUP-DELTA")` in `src[..AI_AGENT push]` — always the `AI_AGENT` annotation, never an earlier one. |
| any other copy of the schema | **None.** See the whole-repo sweep in §2. |
| pi's `ShellToolConfig.prompt` (`"$"` / `"PS>"`, `bash.ts:522`, `powershell.ts:43`) absent from cyrup's config struct | **Verified not a gap.** cyrup carries it in the TUI instead — `cyrup-tui/src/transcript/tool_render.rs:41` passes `"PS>"`, and `cyrup-tui/src/tests/tool_render.rs:85-111` pins both prompts against `bash.ts:488`. Mechanism-only; no caller-visible difference. |

## 5. Tests — exactly which assertions hold this, and the RED lever

The pin is entirely in `crates/cyrup-tools/src/tests/pi_schema.rs` (module wired at
`crates/cyrup-tools/src/tests/mod.rs:13`):

* **`pi_schema.rs:59-63`** — the ground-truth constant, and its comment IS the invariant:

  ```rust
  // v0.84.3 `bashSchema` (bash.ts:42-45). ONE schema object serves BOTH shell tools: the shared
  // factory hands it to `parameters` (bash.ts:353) and `powershell.ts:49-57` calls that same
  // factory. `bash` and `powershell` therefore emit byte-identical `input_schema`, and asserting
  // both against ONE constant here is what keeps them that way.
  const PI_SHELL: &str = r#"{"type":"object","required":["command"],"properties":{"command":{"type":"string","description":"Shell command to execute"},"timeout":{"type":"number","description":"Timeout in seconds (optional, no default timeout)"}}}"#;
  ```

* **`pi_schema.rs:83-84`** — `all_eight_tool_schemas_match_pi_typebox_bytes`:
  `assert_schema("bash", bash.parameters(), PI_SHELL);`
* **`pi_schema.rs:86-87`** — same test: `assert_schema("powershell", powershell.parameters(), PI_SHELL);`

Under the archived (pre-fix) design these two assertions could not have shared one
constant — `bash` needed a `"Bash command to execute"` expectation and `powershell` a
`"Shell command to execute"` one. **The test delta this gap's closure required was:
collapse the two shell expectations onto the single `PI_SHELL` constant and point both
`assert_schema` calls at it.** That is exactly the state in tree; no assertion remains to
move.

**RED lever (proves DoD #2).** Edit `bash.rs:139` back to
`"description": "Bash command to execute"`. `ShellTool::bash` → `Self::new` → `params` →
`Tool::parameters` (`bash.rs:182-184`) → `assert_schema` (`pi_schema.rs:70-76`, which
compares parsed `serde_json::Value`s) fails at `pi_schema.rs:84` with
`bash parameters() schema diverges from Pi's TypeBox input_schema`. `powershell` at `:87`
stays green, which is precisely the shape of the old divergence.
**Not executed:** `cargo` is forbidden by this pass's constraints (10 sibling worktrees,
7.7G disk). The RED claim is by inspection of the call path above, which is four
unconditional hops with no branching.

## 6. Decision required

1. **Close it.** ✅ **Prescribed.** cyrup's behaviour already equals pi's at `e8682309`;
   the marker is gone; the guard exists at `pi_schema.rs:63,84,87`. Record this as
   *closed by verification* — no source edit, no new test.
2. **Accept it.** Arguing this honestly for David: the only accept case is baseline
   fidelity — "cyrup was ported from v0.83.0, keep v0.83.0's string until a wholesale
   v0.84.x uplift." It fails on three counts. (a) The rest of the file is *already* ahead
   of v0.83.0 on model-facing text — `bash.rs:206-215` documents that the prompt guideline
   ships the v0.84.1 wording deliberately, and `pi_schema.rs:132-143` pins it — so keeping
   v0.83.0's schema string would be an *inconsistent* baseline, not a preserved one.
   (b) It splits a schema upstream shares, forcing two constants where one enforces the
   sharing invariant. (c) The stated benefit (a prompt-cache prefix matching a v0.83.0
   transcript) is unattainable anyway, because the guideline string in the same system
   prompt is already v0.84.1 wording with `CYRUP_*` names. **There is no accept case worth
   his signature, and no action is needed to decline it — the tree is already aligned.**
3. **Reshape it.** N/A.

## 7. Additional divergences found — NOT part of this gap, nobody has authorized them

Surfaced, not decided. Each is the same class of defect (a hand-written model-facing
`command` description that is not pi's) and none has a task.

* **`crates/cyrup-ext-sdk/src/tool_factory.rs:17-33` — LIVE, same defect, different crate.**
  `bash_descriptor(cwd)` hand-rolls a schema with
  `"command": { "type": "string", "description": "The shell command to run." }`, plus an
  extra `"cwd"` property, **no `timeout`**, `label("Bash")` (pi: `"bash"`), and a
  paraphrased tool description `"Run a shell command in the project working directory."`.
  At `e8682309`, `packages/coding-agent/src/core/sdk.ts:117-129` re-exports the **real**
  `createBashTool` (and `createPowerShellTool`), so a pi extension author gets the
  byte-exact `bashSchema`; a cyrup extension author using this builder does not. The
  module doc's claim at `tool_factory.rs:4` that these builders "reproduce the shapes of
  Pi's built-in tools" is false as written. `read_descriptor` (`:36-48`) and
  `write_descriptor` (`:50-62`) carry no property descriptions at all. `tool_factory` is
  `pub` (`cyrup-ext-sdk/src/lib.rs:59`), so this is reachable public API and the strings
  reach a model. **QUESTION FOR DAVID:** file this as its own gap — align to pi's
  re-export model (expose the real `cyrup-tools` descriptors) rather than patching the
  paraphrase? Also noted at `MEDIUM-open-questions-from-gap-closure.md:48`, still
  undecided. It is the only *live* instance of ITEM 1's defect left in the workspace.
* **`crates/cyrup-ext-sdk/src/tool_factory.rs:17` — stale upstream citation.** Cites
  "Pi `createBashTool(cwd)`, bash.ts:451"; at `e8682309` `createBashTool` is
  `bash.ts:536` (`createBashToolDefinition` at `:529`). `bash.ts:451` is the `ops.exec`
  call inside the shared factory.
* **Archived record carries the dead string.**
  `.flux/done/2026-08-23-00-08/MEDIUM-the-entire-powershell-built-in-tool-is-missing-from-cyrup.md:349,726,755-764,789,832`
  still shows `command_description` and `"Bash command to execute"` as live design (verified
  line numbers; the open-questions entry's `:760` is inside the same doc block).
  **QUESTION FOR DAVID:** do completed task records get a superseded-by note when a later
  pass reverses their design, or does the `.flux/done` archive stay immutable? A future
  audit reading `:760` will re-derive this same phantom.
* **Process, not code: line-number anchors go stale.** This task, and its three siblings,
  were filed with line anchors that drifted by 8 within hours. **QUESTION FOR DAVID:**
  should gap tasks anchor by *symbol* (`ShellToolConfig::command_description`) with the
  line as a hint only? Cheap change, would have prevented this whole re-derivation.

## Definition of done — status

1. **Gap closed** ✅ — closed in tree; `bash` and `powershell` both emit pi's
   `"Shell command to execute"` from one shared literal. Not an accepted divergence; no
   marker remains to annotate.
2. **A test fails without the change** ✅ — `pi_schema.rs:84`
   (`all_eight_tool_schemas_match_pi_typebox_bytes`) fails on the bash assertion the
   moment `bash.rs:139` reverts. RED lever spelled out in §5; not executed (`cargo`
   forbidden this pass).
3. **No behaviour regression in the owning crate** ✅ by inspection — the only consumers
   of the removed field were `BASH_CONFIG`/`POWERSHELL_CONFIG` and the `json!` literal;
   the two source-scan tests that read `bash.rs` scan windows that never contained the
   deleted marker (§4). Not re-run under this pass's no-cargo constraint.

## Verification log (no `git`, no `cargo`)

```
cat tmp/pi/.git/HEAD; head -2 tmp/pi/.git/packed-refs; tail -1 tmp/pi/.git/logs/HEAD
grep -n '"version"' tmp/pi/packages/coding-agent/package.json
grep -n "bashSchema\|Shell command to execute" tmp/pi/packages/coding-agent/src/core/tools/bash.ts
sed -n '326,342p;515,528p' tmp/pi/packages/coding-agent/src/core/tools/bash.ts
sed -n '36,58p'          tmp/pi/packages/coding-agent/src/core/tools/powershell.ts
grep -n "command:"       tmp/pi/packages/coding-agent/src/core/tools/powershell.ts     # 0 hits
sed -n '108,132p'        tmp/pi/packages/coding-agent/src/core/sdk.ts
sed -n '11,14p'          tmp/pi/packages/agent/src/harness/tools/bash.ts
sed -n '58,88p;130,145p;166,190p' crates/cyrup-tools/src/tools/bash.rs
sed -n '28,52p'                   crates/cyrup-tools/src/tools/powershell.rs
sed -n '55,95p;128,150p'          crates/cyrup-tools/src/tests/pi_schema.rs
sed -n '750,845p'                 crates/cyrup-tools/src/tests/pi_tool_semantics.rs
sed -n '295,350p'                 crates/cyrup-tools/src/tests/bash_session_env.rs
sed -n '1,70p'                    crates/cyrup-ext-sdk/src/tool_factory.rs
grep -rn "command to execute" --exclude-dir={target,tmp,.flux,.git} .        # 2 hits, both pi's
grep -rn "command_description" --include=*.rs crates/                        # 0 in cyrup-tools
grep -n  "CYRUP-DELTA" crates/cyrup-tools/src/tools/bash.rs                  # 206, 228, 304
```
