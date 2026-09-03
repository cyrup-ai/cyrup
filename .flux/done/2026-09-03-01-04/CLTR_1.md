---
stage: qa
status: completed
updated: 2026-09-03 23:20
aug_against: main 2cfff0f / branch c9da9fb — every site below re-read; counts are exact, not "grep to confirm"
---

# CLTR_1 — `TerminateHint`: one tri-state for `terminate` (F3)

OBJECTIVE: Replace the four encodings of the tool early-termination hint — `bool`, `Option<bool>`,
`bool`, `Option<bool>` — with one three-valued `cyrup_core::TerminateHint`, so pi's explicit
`terminate: false` becomes representable, the wire key's presence/absence is derived from one
`.wire()`, and the batch fold reads one predicate. Leaf-most step of the plan; unlocks CLTR_4 and
CLTR_7. Source: `.flux/research/CORE_LOOP_TYPE_REVIEW.md` §3 F3, §6 step 1.

> **READ §0 FIRST.** The sweep changed three things the pre-augmentation version of this file got
> wrong: the WASM ABI is a hard boundary the type must not cross; there is a *second* lossy
> conversion; and `cyrup-ext-subagents` has nothing to change.

---

## 0. What the sweep found — corrections to the plan

**0.1 `TerminateHint` stops at the host side of the WASM boundary. WIT stays `bool`.**
`terminate` crosses the guest ABI in two WIT records — `block-result.terminate: bool`
([`world.wit:78-81`](../../crates/cyrup-ext/wit/world.wit)) and `tool-output.terminate: bool`
(`:158-163`) — and [`cyrup-ext-sdk/wit/world.wit`](../../crates/cyrup-ext-sdk/wit/world.wit) is a
copy of the same file (verify with `diff -q`; they must stay identical). A WIT `bool` cannot say
"unspecified", so a guest tool that wants pi's explicit `false` **still cannot** — that is an
ABI limitation this task does not remove (it would be a breaking guest change; record it as a
follow-up in §6, not work here). The host converts at exactly two sites, and **guest `false` must
map to `Unspecified`**, because today guest `false` → host `false` → `finalize.rs:51` → `None` →
key absent, and the wire must stay byte-identical. The SDK's guest-side builder
([`api.rs:245,268`](../../crates/cyrup-ext-sdk/src/api.rs), `guest.rs:199,273`) is the guest half
of that `bool` and is **untouched**.

**0.2 There are TWO lossy `if x { Some(true) } else { None }` conversions, not one.**
[`finalize.rs:51`](../../crates/cyrup-agent/src/agent/run/tools/finalize.rs) (the tool's result)
**and** [`preflight.rs:153`](../../crates/cyrup-agent/src/agent/run/tools/preflight.rs)
(`immediate_error`, from `BeforeOutcome::Block.terminate: bool`). Both go.

**0.3 `cyrup-ext-subagents` has zero `ToolResult.terminate` sites.** Every `terminate` hit there is
process-signal (`child.terminate()`, `SignalKind::terminate`) or the string "unterminated". The
former SUBTASK5 is deleted.

**0.4 `cyrup-tools` has 17 literals; all are `false`/`None`; none is `true`.** Files:
`find.rs:317,365`, `grep.rs:1257,1312`, `edit.rs:363`, `read.rs:267,324,460,474,503`,
`write.rs:140`, `ls.rs:216,261` (`ToolResult`, `terminate: false`); `bash.rs:370,580,700`
(`ToolUpdate`, `terminate: None`) and `bash.rs:596` (`ToolResult`, `false`). Purely mechanical.

**0.5 `Batch.terminate: bool` is a DIFFERENT fact and stays `bool`.**
[`tools/mod.rs:48`](../../crates/cyrup-agent/src/agent/run/tools/mod.rs) is the *aggregate*
"every finalized result requested termination" — the output of the fold, consumed at
[`turn.rs:79`](../../crates/cyrup-agent/src/agent/run/turn.rs) as `has_more_tools = !batch.terminate`
and constructed at `mod.rs:115`, `exec.rs:253,374`. It is a genuine boolean, not a hint. Do not
touch it.

**0.6 `AfterOverride.terminate` is dead-in-practice today, and the research's "wire it to the
guest" addition is what makes it live.** `cyrup-ext/src/hooks.rs` `after_tool_call` builds
`HostEvent::ToolResult { content, details, is_error, usage, .. }` — **no `terminate`** — and its
override diff reads back only those four (`:14-39`); `EventPatch::ToolResult`
([`contract.rs:52-57`](../../crates/cyrup-ext/src/contract.rs)) has no `terminate` either. So no
producer sets `AfterOverride.terminate`. Both `HostEvent::ToolResult` and `EventPatch::ToolResult`
travel as **JSON** (`world.wit:84`, `live.rs:2058-2068`, `:2284-2292`), so adding an optional
`terminate` key to each is **additive and backward-compatible** — a guest that never sends it sees
no change. This task adds it (§3.4); it is the prescriptive, feature-complete path and the only
reason `AfterOverride.terminate` exists.

---

## 1. The type (SUBTASK1 — `cyrup-core`)

What: in [`cyrup-core/src/tool.rs`](../../crates/cyrup-core/src/tool.rs), add:
```rust
/// Pi's `AgentToolResult.terminate?: boolean` (types.ts:354-368) as the three values it
/// actually has. The wire key is emitted iff `wire()` is `Some`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminateHint {
    /// pi `undefined` — key ABSENT on the wire; contributes nothing to the batch fold.
    #[default]
    Unspecified,
    /// pi `true` — key present as `true`; the batch terminates iff every finalized result says this.
    Terminate,
    /// pi explicit `false` — key PRESENT as `false`. Representable now; was not before.
    Continue,
}
impl TerminateHint {
    pub const fn requested(self) -> bool { matches!(self, Self::Terminate) }
    pub const fn wire(self) -> Option<bool> {
        match self { Self::Unspecified => None, Self::Terminate => Some(true), Self::Continue => Some(false) }
    }
    /// The ONLY sanctioned `bool` → hint mapping, for the two WASM host-side sites (§0.1):
    /// a guest `false` is "nothing said", never an explicit `Continue`.
    pub const fn from_guest_bool(b: bool) -> Self { if b { Self::Terminate } else { Self::Unspecified } }
    /// JSON `Option<bool>` (a tool-update chunk, §3.5) maps 1:1 — here `Some(false)` IS `Continue`.
    pub const fn from_wire(o: Option<bool>) -> Self {
        match o { None => Self::Unspecified, Some(true) => Self::Terminate, Some(false) => Self::Continue }
    }
}
```
**No `impl From<bool>`** — an implicit `false → Continue` is exactly the ambiguity being removed;
the two explicit constructors name which mapping a call site means.
Change `ToolResult.terminate: bool` (`:42`) and `ToolUpdate.terminate: Option<bool>` (`:56`) to
`TerminateHint`; update their doc comments (`:47-55`) to point at the enum.
Why: `finalize.rs:43-45`'s `[CYRUP-DELTA]` says it: explicit `false` is unrepresentable.

## 2. `cyrup-agent` (SUBTASK2)

Every site, verified at HEAD. Change the type, then let the compiler walk the list.

| file:line | today | after |
|---|---|---|
| `hooks.rs:62` `BeforeOutcome::Block.terminate` | `bool` | `TerminateHint` |
| `hooks.rs:84` `AfterToolCall.terminate` | `Option<bool>` | `TerminateHint` |
| `hooks.rs:109` `AfterOverride.terminate` | `Option<bool>` | **`Option<TerminateHint>`** — `None` = hook has no opinion (pi `afterResult.terminate ?? result.terminate`, `finalize.rs:103-104`); `Some(Unspecified)` = hook clears it; the distinction `Option<bool>` collapses |
| `tools/mod.rs:35` `Finalized.terminate` | `Option<bool>` | `TerminateHint` |
| `tools/mod.rs:48` `Batch.terminate` | `bool` | **unchanged** (§0.5) |
| `message.rs:74-99` `result_value_of(.., terminate: Option<bool>)` | emits key iff `Some` (`:95-96`) | takes `TerminateHint`; `if let Some(t) = terminate.wire()` |
| `message.rs:105-115` `update_value` | `if let Some(t) = u.terminate` (`:111`) | `u.terminate.wire()` |
| `finalize.rs:37` destructure `mut terminate` | `Option<bool>` | `TerminateHint` |
| `finalize.rs:51` | `if r.terminate { Some(true) } else { None }` | `r.terminate` — **delete the conversion**; delete the `[CYRUP-DELTA]` at `:43-45` (resolved) |
| `finalize.rs:75` | passes `terminate` | unchanged |
| `finalize.rs:105-107` | `if let Some(t) = ov.terminate { terminate = Some(t) }` | `if let Some(t) = ov.terminate { terminate = t }` |
| `finalize.rs:122` (hook `Err`) | `terminate = None` | `terminate = TerminateHint::Unspecified` |
| `finalize.rs:147,150` | passes `terminate` | unchanged |
| `preflight.rs:89` `Block { reason, terminate }` | `bool` | `TerminateHint` (from `hooks.rs:62`) |
| `preflight.rs:104` | passes it to `immediate_error` | unchanged |
| `preflight.rs:132` `immediate_error(.., terminate: bool)` | `bool` | `TerminateHint` |
| `preflight.rs:153` | `if terminate { Some(true) } else { None }` | **delete** (§0.2) |
| `preflight.rs:158,160` | passes `terminate` | unchanged |
| `exec.rs:244-245` parallel fold | `.all(\|f\| f.terminate == Some(true))` | `.all(\|f\| f.terminate.requested())` |
| `exec.rs:358` sequential fold | `if fin.terminate != Some(true)` | `if !fin.terminate.requested()` |
| `exec.rs:253,264,372,374`, `mod.rs:115`, `turn.rs:79` | `Batch.terminate` / `all_terminate: bool` | **unchanged** (§0.5) |

## 3. `cyrup-ext` (SUBTASK3) — boundary B6, and where the type ends

**3.1 The two WIT→host conversions (the boundary, §0.1).**
- [`host/live.rs:1480`](../../crates/cyrup-ext/src/host/live.rs) `ToolResult { .., terminate: out.terminate, .. }`
  where `out` is the WIT `tool-output` → `terminate: TerminateHint::from_guest_bool(out.terminate)`.
- [`host/live.rs:2256`](../../crates/cyrup-ext/src/host/live.rs) `HookOutcome::Block { reason: b.reason, terminate: b.terminate }`
  where `b` is the WIT `block-result` → `terminate: TerminateHint::from_guest_bool(b.terminate)`.
WIT unchanged. SDK unchanged.

**3.2 The JSON tool-update chunk.** [`host/live.rs:1467`](../../crates/cyrup-ext/src/host/live.rs)
`terminate: chunk.get("terminate").and_then(Value::as_bool)` → `TerminateHint::from_wire(chunk.get("terminate").and_then(Value::as_bool))`.
This path CAN express `Continue` (it is JSON, not WIT) — the one guest-originated `Continue` today.

**3.3 The duplicated contract (B6 — both sides together).**
- [`contract.rs:31`](../../crates/cyrup-ext/src/contract.rs) `HookOutcome::Block.terminate: bool` → `TerminateHint`; doc at `:19-31` unchanged in substance.
- `contract.rs:196` `Reduced::Blocked.terminate: bool` → `TerminateHint`.
- [`dispatch.rs:469-470`](../../crates/cyrup-ext/src/dispatch.rs) pass-through: unchanged text, new type.
- `dispatch.rs:461` synthesised `Reduced::Blocked { .., terminate: false, .. }` → `TerminateHint::Unspecified`.
- [`cyrup-ext/src/hooks.rs:56-57`](../../crates/cyrup-ext/src/hooks.rs) `Reduced::Blocked { reason, terminate, .. } => Ok(BeforeOutcome::Block { reason, terminate })`: unchanged text, new type.

**3.4 Make `AfterOverride.terminate` reachable (§0.6) — additive JSON.**
- `contract.rs:52-57` `EventPatch::ToolResult` gains `terminate: Option<TerminateHint>`.
- `contract.rs:94-114` `apply_patch`'s `ToolResult` arm: add `if let Some(t) = t { *terminate = t }`
  beside `is_error` (replace-not-merge, same shape as `:106-108`).
- `live.rs:2058` `HostEvent::ToolResult { .. }` gains `terminate: TerminateHint`, serialised to the
  guest as the JSON key iff `.wire()` is `Some` (mirror `result_value_of`) — so the guest sees
  exactly what the model will see.
- `live.rs:2284-2292` the patch parser: `terminate: TerminateHint` parsed leniently as
  `v.get("terminate").and_then(Value::as_bool)` → `Option<TerminateHint>` via `from_wire`,
  wrapped so a missing key is `None` (no opinion), present `false` is `Some(Continue)`.
- `cyrup-ext/src/hooks.rs` `after_tool_call`: seed `HostEvent::ToolResult.terminate` from
  `ctx.terminate` (now `TerminateHint`), and in the diff at `:26-39` add
  `if terminate != orig_terminate { over.terminate = Some(terminate) }`.
Why: `AfterOverride.terminate` has no producer today; this is the only path that gives it one,
and it is additive — a guest that omits the key changes nothing.

## 4. `cyrup-tools` (SUBTASK4) — 17 mechanical edits (§0.4)

For each `ToolResult { .., terminate: false, .. }`: if the literal already ends in
`..Default::default()`, **delete the `terminate: false` line**; otherwise replace with
`terminate: TerminateHint::Unspecified`. For `bash.rs:370,580,700` `ToolUpdate { terminate: None }`
→ `TerminateHint::Unspecified` (or delete under a `..Default::default()`). None means `Continue`
— all 17 were the default before and are the default after.

## 5. Definition of done

- `cyrup_core::TerminateHint` exists with `requested`, `wire`, `from_guest_bool`, `from_wire`;
  **no `From<bool>`**.
- No `terminate: bool` / `Option<bool>` field remains on `ToolResult`, `ToolUpdate`,
  `BeforeOutcome::Block`, `AfterToolCall`, `Finalized`, `HookOutcome::Block`, `Reduced::Blocked`;
  `AfterOverride.terminate` and `EventPatch::ToolResult.terminate` are `Option<TerminateHint>`.
- `Batch.terminate` is still `bool` (§0.5).
- Both `if … { Some(true) } else { None }` conversions (`finalize.rs:51`, `preflight.rs:153`) are
  gone; the `[CYRUP-DELTA]` at `finalize.rs:43-45` is gone.
- `crates/cyrup-ext/wit/world.wit` and `crates/cyrup-ext-sdk/wit/world.wit` are byte-identical to
  HEAD (`diff` against the pre-change file). `cyrup-ext-sdk/src` untouched.
- `cyrup-ext-subagents` untouched (§0.3).
- `tool_result_model.rs` (11 tests on the `terminate` key's presence/absence) passes **unedited**
  — they are now the `.wire()` parser tests.
- `cargo check --workspace` green; `cargo test -p cyrup-core -p cyrup-agent -p cyrup-ext -p cyrup-tools`
  green; `cargo clippy --workspace --all-targets --features test-fixtures` exits 0.
- Wire byte-identical for every existing path: the key was absent for `false` before and is absent
  for `Unspecified` now; a guest `tool-output.terminate: false` still yields an absent key.

## 6. Recorded follow-up (not this task)

A guest tool cannot express pi's explicit `terminate: false` because `tool-output.terminate` is a
WIT `bool` (§0.1). Lifting that means `option<bool>` in both WIT files plus the SDK builder — a
breaking guest ABI change, out of scope here and for CLTR as a whole.

## Research notes

Research doc §3 F3 (tendrils, boundary), §2 boundary B6. pi's fold rule
(`finalizedCalls.length > 0 && every(f => f.result.terminate)`) is at `exec.rs:237-245` and stays a
runtime rule. pi's override rule (`afterResult.terminate ?? result.terminate`, agent-loop.ts:739)
is what `Option<TerminateHint>` on `AfterOverride` encodes exactly.

No tests to be written — another team owns tests. No benchmarks to be written.
