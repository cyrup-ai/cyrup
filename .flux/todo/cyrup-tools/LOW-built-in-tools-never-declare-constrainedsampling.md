---
title: Built-in tools never declare constrainedSampling
priority: LOW
tool: read/bash/edit/write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: done
updated: 2026-08-27 14:40
---

# Outstanding: DoD 8 — two stale doc-comments, and the previously-proposed fix for one of them is itself wrong

QA rating **9/10 → needs-rework**. Clauses 1–7 were each read and independently confirmed by the
reviewer (including a line-by-line diff of `make_json_schema_node_strict` against the vendored pi
checkout). **They are settled — see [§5](#5-verified--do-not-revisit) — do not re-litigate them.**

Clause 8 is the only failure:

> 8. No doc-comment anywhere in the workspace still asserts that no pi built-in declares
>    `constrainedSampling`.

Two sites fail it. This is a **doc-only** change: no behaviour, no test, no schema, no signature.

> **Augmentation finding — read before editing.** The replacement wording sketched in the previous
> revision of this task ended with *"(Built-ins do not pass through here …)"*. **That is false**
> and would have planted a third stale claim in the same doc-comment being fixed. Built-ins DO pass
> through `RegisteredTool`, in cyrup and upstream alike — proof in [§1.2](#12-the-fact-that-kills-the-previously-proposed-wording).
> Use the prescribed text in [§1.3](#13-prescribed-replacement--required-verbatim), not the old sketch.

Root cause worth recording so it is not repeated: §6 of the original task said "nothing in
`cyrup-ext/src/wrapper.rs` changes", which directly contradicted its own DoD 8. The implementer
followed §6 and inherited the error. **The DoD is authoritative; the prose was wrong.**

---

## 1. Required — `crates/cyrup-ext/src/wrapper.rs`

### 1.1 The offending block

File: [`crates/cyrup-ext/src/wrapper.rs`](../../../crates/cyrup-ext/src/wrapper.rs)
Currently **lines 113–125**. Lines shift under concurrent work — anchor on the symbol instead:
the doc-comment immediately above

```rust
    fn constrained_sampling(&self) -> Option<&cyrup_core::ConstrainedSampling> {
        self.inner.constrained_sampling()
    }
```

inside `impl Tool for RegisteredTool` (the block starts at `/// PROV-011 — upstream
`wrapRegisteredTool` is a SPREAD`). Verbatim as it stands today:

```rust
    /// PROV-011 — upstream `wrapRegisteredTool` is a SPREAD of the already-wrapped tool
    /// (`return { ...tool, execute }`, `core/extensions/wrapper.ts:21-22` @v0.83.0), so every field
    /// `wrapToolDefinition` copied — `constrainedSampling` among them
    /// (`core/tools/tool-definition-wrapper.ts:14`) — survives this wrapper by construction.
    /// Rust has no spread: each surface method must be delegated by hand, and this one was the
    /// method the hand-written list missed. Extension-registered and WASM-guest tools are the ONLY
    /// tools that can declare `constrainedSampling` (no pi built-in does), and every one of them
    /// reaches the loop through this wrapper — so without this override the whole opt-in path was
    /// dead on arrival: `WasmTool::constrained_sampling` read the guest's declaration off the
    /// descriptor and this wrapper dropped it one frame later, silently.
```

Two false claims in one sentence:

* **"no pi built-in does"** — verbatim the assertion DoD 8 exists to eliminate. False since pi
  `7915cdac` (*"feat(ai): add strict tool schema conversion"*, first tagged **v0.84.2**). Verified
  against the vendored checkout — `rg -n constrainedSampling tmp/pi/packages/coding-agent/src`
  returns [`read.ts:222`](../../../tmp/pi/packages/coding-agent/src/core/tools/read.ts),
  [`bash.ts:354`](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts),
  [`edit.ts:329`](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts),
  [`write.ts:200`](../../../tmp/pi/packages/coding-agent/src/core/tools/write.ts) and
  [`server/create-harness.ts:34`](../../../tmp/pi/packages/coding-agent/src/server/create-harness.ts),
  each `constrainedSampling: getExperimentalToolSampling()`. `bash.ts:354` sits inside
  `createShellToolDefinition` (opens at `bash.ts:338`), so upstream `powershell` inherits the
  declaration from that same line. cyrup mirrors all four.
* **"the ONLY tools that can declare `constrainedSampling`"** — strictly stronger and equally
  false. Any `impl Tool` may override the accessor; four built-ins in `cyrup-tools` do.

### 1.2 The fact that kills the previously-proposed wording

**Built-ins pass through `RegisteredTool`.** Confirmed on both sides:

* cyrup — [`crates/cyrup-ext/src/facade.rs`](../../../crates/cyrup-ext/src/facade.rs) `active_tools`
  (**line 610**; doc at **604–609**) maps `wrap_registered_tool` over the *merged* set and its own
  doc says so: *"the built-ins in `base` as well as the extension-contributed ones"*. `wrap_tool`
  (**line 333**) is the single-tool form used by the session builder.
* pi — [`agent-session.ts`](../../../tmp/pi/packages/coding-agent/src/core/agent-session.ts)
  **2694–2702**: `wrappedExtensionTools = wrapRegisteredTools(allCustomTools, runner)` *and*
  `wrappedBuiltInTools = wrapRegisteredTools(Array.from(this._baseToolDefinitions.values())…)`.
  Both halves of `_toolRegistry` are wrapped.

So the override is **more** load-bearing than the old comment claimed, not less: since v0.84.2 the
four built-ins' declarations ride through this same wrapper, and in Rust they survive only because
the delegation is written out by hand. That is the corrected rationale, and it must be what the new
comment says.

The only genuine distinction left for extension/WASM tools is that they have **no other route** —
a WASM guest tool reaches the loop exclusively as `WasmTool` behind this wrapper
([`crates/cyrup-ext/src/host/live.rs`](../../../crates/cyrup-ext/src/host/live.rs), `fn
constrained_sampling` at **line 1960**, reading `self.descriptor.constrained_sampling`), whereas a
built-in is also reachable un-wrapped when no live active-tool source is attached (`wrap_tool`
returns the tool as-is on `None`).

### 1.3 Prescribed replacement — required, verbatim

Replace the whole doc block above with exactly this:

```rust
    /// PROV-011 — upstream `wrapRegisteredTool` is a SPREAD of the already-wrapped tool
    /// (`return { ...tool, execute }`, `core/extensions/wrapper.ts:21-22` @v0.83.0), so every field
    /// `wrapToolDefinition` copied — `constrainedSampling` among them
    /// (`core/tools/tool-definition-wrapper.ts:14`) — survives this wrapper by construction.
    /// Rust has no spread: each surface method must be delegated by hand, and this one was the
    /// method the hand-written list missed.
    ///
    /// Everything the agent runs reaches it through this wrapper — the built-ins in `base` as well
    /// as the extension-registered and WASM-guest tools ([`crate::ExtFacade::active_tools`], and
    /// upstream `wrapRegisteredTools(allCustomTools…)` + `wrapRegisteredTools(baseToolDefinitions…)`
    /// at agent-session.ts:2694-2702 @v0.84.2) — so the missing delegation silently dropped EVERY
    /// declaration one frame after it was read. For a guest tool that was the whole opt-in path,
    /// dead on arrival: `WasmTool::constrained_sampling` (host/live.rs) lifted the declaration off
    /// the descriptor and this wrapper discarded it. Since pi `7915cdac` @v0.84.2 it is also the
    /// path the four coding built-ins depend on: `read`, `edit`, `write` and the shared `ShellTool`
    /// engine — pi's `createShellToolDefinition`, so `powershell` inherits it — each return
    /// [`cyrup_core::experimental_tool_sampling`], which is `Some` only under
    /// `CYRUP_EXPERIMENTAL=1`/`PI_EXPERIMENTAL=1` and `None` otherwise.
```

Constraints this text satisfies, and which any deviation must also satisfy:

- no surviving claim that built-ins do not declare the field;
- no surviving "the ONLY tools that can declare";
- **no new claim that built-ins bypass this wrapper** — they do not;
- the dead-on-arrival rationale for the override is preserved and now correctly generalised;
- `powershell` is named as inheriting from the shared `ShellTool` factory, matching pi's
  `createShellToolDefinition`;
- the opt-in is named as gated on `CYRUP_EXPERIMENTAL`/`PI_EXPERIMENTAL`.

Ground truth for each claim, all read during augmentation:

| Claim | Source |
| --- | --- |
| `read` declares it | [`crates/cyrup-tools/src/tools/read.rs`](../../../crates/cyrup-tools/src/tools/read.rs) `fn constrained_sampling` (line 98) |
| shared shell engine declares it; `powershell` inherits | [`crates/cyrup-tools/src/tools/bash.rs`](../../../crates/cyrup-tools/src/tools/bash.rs) `impl Tool for ShellTool` `fn constrained_sampling` (line 270); `ShellTool` = pi `createShellToolDefinition`, `bash`/`powershell` are two `ShellToolConfig` instantiations (module header lines 6–8) |
| `edit` declares it | [`crates/cyrup-tools/src/tools/edit.rs`](../../../crates/cyrup-tools/src/tools/edit.rs) line 241 |
| `write` declares it | [`crates/cyrup-tools/src/tools/write.rs`](../../../crates/cyrup-tools/src/tools/write.rs) line 93 |
| flag gate, both names, `prefer` value | [`crates/cyrup-core/src/constrained_sampling.rs`](../../../crates/cyrup-core/src/constrained_sampling.rs) `experimental_tool_sampling_from` (line 100) / `experimental_tool_sampling` (line 120), `static PREFER_STRICT_TOOL_SAMPLING` (line 92) |
| pi's gate is `PI_EXPERIMENTAL` only | [`tmp/pi/packages/coding-agent/src/core/experimental.ts`](../../../tmp/pi/packages/coding-agent/src/core/experimental.ts) lines 1–9 |
| wrapper delegation is already covered by a test | `wrapper.rs` `every_surface_method_delegates` (line 365) asserts presence on the inner fixture *then* equality — not vacuous |

`crates/cyrup-core/src/tool.rs` **lines 154–161** already carries the corrected, accurate paragraph
for the same facts. Read it before writing; keep the two consistent in substance. **Do not edit
`tool.rs`** — it already passes.

## 2. Required — `crates/cyrup-core/src/constrained_sampling.rs`, stale `@v0.83.0` cross-reference

File: [`crates/cyrup-core/src/constrained_sampling.rs`](../../../crates/cyrup-core/src/constrained_sampling.rs),
module header, currently **lines 4–6**. Anchor on the phrase *"The resolvers that consume them live
provider-side in"*.

```rust
//! constrained sampling. The resolvers that consume them live provider-side in
//! `cyrup-provider/src/utils/constrained_sampling.rs` (a port of pi
//! `packages/ai/src/api/constrained-sampling.ts` @v0.83.0).
```

Change `@v0.83.0` → `@v0.84.2` on that line, and nothing else on it. Justification: the pointed-at
file's own citations are uniformly `@v0.84.2` — `constrained-sampling.ts:12-29`, `:35-44`, `:46-51`,
`:53-115`, `:117-127`, `:129-131`, `:208-227` at
[`crates/cyrup-provider/src/utils/constrained_sampling.rs`](../../../crates/cyrup-provider/src/utils/constrained_sampling.rs)
lines 62, 83, 99, 126, 215, 229, 389. This one cross-reference was missed in the bump; the two must
agree.

### Explicitly out of scope — do not churn

- `constrained_sampling.rs` **lines 11, 36, 52, 61, 69, 79** and `tool.rs` **line 142**: each
  `@v0.83.0` there is qualified to the tag the cited line was true at, which is correct practice.
- `constrained_sampling.rs` **line 22** — *"At v0.83.0 no pi built-in declared the field"* — is
  **true as written** (it is tag-qualified and immediately followed by the v0.84.2 correction at
  lines 20–33). It is the one permitted survivor of the verification sweep in [§4](#4-definition-of-done). Leave it.
- The `agent-session.ts:2506-2515` citations in `wrapper.rs`'s module header (**lines 6–7**) and
  `facade.rs` (**line 606**) point at a pre-v0.84.2 offset; the vendored 0.84.2 offset is
  2694–2702. Pre-existing drift in a different sentence, unrelated to DoD 8. **Not this task.**

## 3. Sweep — every candidate hit, adjudicated

Run during augmentation across `crates/`, so the rework pass closes all of them at once instead of
leaving a third:

```
rg -n -i "no (pi )?built-?in|built-?ins? (do|does) not|only tools that can declare|does not declare|never declares?" crates/ --glob '*.rs'
```

**Must change (2):**

| Site | Why |
| --- | --- |
| `crates/cyrup-ext/src/wrapper.rs:119` | §1 — "ONLY tools that can declare … (no pi built-in does)" |
| `crates/cyrup-core/src/constrained_sampling.rs:6` | §2 — stale `@v0.83.0` cross-reference |

**False positives — verified unrelated, must NOT be touched (do not widen the diff):**

| Site | Subject |
| --- | --- |
| `crates/cyrup-core/src/constrained_sampling.rs:22` | tag-qualified and correct (§2) |
| `crates/cyrup-tools/src/tools/write.rs:72` | `executionMode`, not `constrainedSampling` (TOOL-006) |
| `crates/cyrup-tools/src/tests/pi_tool_semantics.rs:62` | `executionMode` (TOOL-006) |
| `crates/cyrup-tools/src/tests/pi_schema.rs:22` | a JSON-schema keyword, unrelated |
| `crates/cyrup-sdk/src/{error.rs:27,client.rs:40,49,264}`, `cyrup-session-svc/src/tests/modelless_launch.rs:55`, `cyrup-it/tests/{bin/embedder_seams.rs:307,session_svc/model_registry.rs:82}`, `cyrup-config/src/{provider_compose.rs:401,tests/models_json_provider.rs:150,model/compose.rs:487}`, `cyrup-provider/src/{collection.rs:541,providers/opencode.rs:104,providers/builtin_oauth.rs:17}`, `cyrup-tui/src/tests/autocomplete.rs:387`, `cyrup-session-svc/src/session/tools.rs:81`, `cyrup-tools/src/isolation/mod.rs:3`, `cyrup-ext-subagents/**`, `cyrup-ext/src/host/services.rs:{47,50,53}` | "built-in **provider**", "built-in agent", capability strings — different noun entirely |

Also read and confirmed clean (no stale exclusivity claim): `cyrup-ext/src/host/live.rs:1953-1961`,
`cyrup-ext/src/registry.rs:127-143`, `cyrup-ext-sdk/src/descriptor.rs:64-105`,
`cyrup-core/src/tool.rs:141-162`.

## 4. Definition of Done

1. `crates/cyrup-ext/src/wrapper.rs` — doc block above `RegisteredTool::constrained_sampling`
   replaced with §1.3 verbatim. No claim that built-ins do not declare the field; no "ONLY tools
   that can declare"; **no claim that built-ins bypass the wrapper**; the dead-on-arrival rationale
   retained.
2. `crates/cyrup-core/src/constrained_sampling.rs` module header line 6 reads `@v0.84.2`.
3. Nothing else in the workspace is modified. No `git`. No new files.
4. **Verification sweep returns exactly one line.**

   ```
   rg -n -i "no (pi )?built-?in tool|no pi built-in|only tools that can declare|built-?ins? (do|does) not (pass|declare)" crates/ --glob '*.rs'
   ```

   Expected output — one hit, and it is the tag-qualified sentence that is correct:

   ```
   crates/cyrup-core/src/constrained_sampling.rs:22://! At v0.83.0 no pi built-in declared the field: `git grep -n constrainedSampling v0.83.0 --
   ```

   Any other line means a stale claim survives (or a new one was introduced) — fix and re-run.
5. **Bypass check returns nothing:**

   ```
   rg -n -i "built-?ins? .{0,40}(do not|don't|never) pass through" crates/ --glob '*.rs'
   ```

   This exists specifically to catch the discarded wording from the previous revision.
6. **Version-agreement check:** `rg -n "constrained-sampling.ts. @v0.83.0" crates/cyrup-core/src/constrained_sampling.rs`
   returns nothing (line 6 was the only such cross-reference; `:52`'s `:85`/`:105` citation is
   line-qualified and stays).
7. Doc-only. No behaviour, signature, schema or test change. `cargo doc` links used above —
   `[`crate::ExtFacade::active_tools`]`, `[`cyrup_core::experimental_tool_sampling`]` — must resolve;
   if a path does not, degrade it to plain backticks rather than inventing a new one.

## 5. Verified — do not revisit

Each read and confirmed by the QA pass; the reviewer independently diffed
`make_json_schema_node_strict` against vendored pi line-by-line and confirmed
`resolve_json_schema_strict_sampling`, `normalize_optional_nulls`, the `openai_responses.rs`
ordering subtlety, and the `bash.rs`-hosts-the-declaration parity. **Out of scope for the rework.**

| DoD | Status |
| --- | --- |
| 1 — flag unset ⇒ `None`, requests byte-identical | PASS (`constrained_sampling.rs` tests `a_tool_that_did_not_opt_in_is_never_converted`, `arguments_without_nulls_are_unchanged_by_the_new_stage`, `openai_completions.rs` DoD-1 arm) |
| 2 — either flag ⇒ `prefer`; `grep`/`find`/`ls` still `None` | PASS (`experimental_tool_sampling_reads_both_flags_and_nothing_else`; only the four tool files contain `constrained_sampling`) |
| 3 — declaration survives the agent loop | PASS (`cyrup-agent/src/agent/run/stream.rs:94`) |
| 4 — strict route ⇒ `strict: true` + converted schema | PASS (all six adapters serialize `json_schema_tool_parameters`; `strict_conversion_requires_every_key_and_makes_optionals_nullable`, `strict_conversion_recurses_through_array_items`) |
| 5 — non-strict route degrades silently, raw schema | PASS (`a_non_strict_route_keeps_the_raw_schema_and_does_not_fail`) |
| 6 — optional `null` executes as ABSENT, never `0` | PASS (`validate.rs` stage 0 **deletes** the key; `an_optional_null_is_deleted_rather_than_coerced_to_zero`) |
| 7 — `require` fails with pi's message shape | PASS (`an_unconvertible_schema_degrades_under_prefer_and_fails_under_require`) |
| 8 — no stale doc-comment | **FAIL** — §1, §2 above |

`bash.rs` hosting the declaration on the shared `ShellTool` engine (so `powershell` inherits) is
**correct**, not a miss: pi puts it on `createShellToolDefinition` at `bash.ts:354`, verified in the
vendored 0.84.2 checkout at `tmp/pi`.
