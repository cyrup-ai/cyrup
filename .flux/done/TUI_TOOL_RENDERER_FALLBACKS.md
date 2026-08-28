---
stage: done
status: completed
updated: 2026-08-28
---

# Port Pi's Tool Call/Result Fallbacks: A Defined-But-Unrendered Tool Gets A Bold Name And A 10-Line Preview, Not Full Args JSON

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** medium · **Kind:** divergent-behaviour · **Area:** Interactive mode shell, footer, status and execution views

## Objective

A verbose extension-registered, SDK-registered or MCP-proxied tool currently floods the transcript:
cyrup commits its **entire** argument JSON and its **entire** output — possibly hundreds of lines —
straight to native scrollback, with no `Ctrl+O to expand` affordance. Upstream the same tool shows
its bold name and ten lines of output with an expand hint. Conversely, an extension tool that
renders only its call line shows **no result at all** in cyrup, where upstream falls through to the
built-in result renderer and then to the text fallback.

## Upstream reference

[`modes/interactive/components/tool-execution.ts`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/tool-execution.ts).
Upstream has **four** shapes where cyrup has two, and the branch that decides between them is
"is there a tool DEFINITION", not "is there a renderer":

- `:104-106` — `hasRendererDefinition()` is
  `this.builtInToolDefinition !== undefined || this.toolDefinition !== undefined` — true for every
  tool that has a definition at all, renderer or not.
- `:84-92` / `:94-102` — `getCallRenderer()` and `getResultRenderer()` merge the two tiers
  **per renderer and independently**: `this.toolDefinition.renderCall ?? this.builtInToolDefinition.renderCall`,
  and the same separately for `renderResult`. An extension supplying only `renderCall` still gets the
  built-in's `renderResult`.
- `:138-140` — `createCallFallback()` returns **only**
  `new Text(theme.fg("toolTitle", theme.bold(this.toolName)), 0, 0)` — the bold name, **no args**.
- `:142-155` — `createResultFallback()`:

  ```ts
  const output = this.getTextOutput();
  if (!output) return undefined;
  const lines = output.split("\n");
  const displayLines = this.expanded ? lines : lines.slice(0, FALLBACK_PREVIEW_LINES);
  const remaining = lines.length - displayLines.length;
  let text = displayLines.map((line) => theme.fg("toolOutput", line)).join("\n");
  if (remaining > 0) {
      text += `${theme.fg("muted", `\n... (${remaining} more lines,`)} ${keyHint("app.tools.expand", "to expand")}${theme.fg("muted", ")")}`;
  }
  ```

  with `FALLBACK_PREVIEW_LINES = 10` at `:9`. Note it honours `expanded` — Ctrl+O shows everything.
- `:273-326` — `updateDisplay`'s definition branch resolves the two sides **separately**: no
  `callRenderer` → `createCallFallback()` (`:281-283`), a THROWING one → the same fallback
  (`:290-294`); then, if a result exists, no `resultRenderer` → `createResultFallback()` (`:298-304`),
  a throwing one → the same (`:316-323`).
- `:327-330` / `:376-387` — the unbounded `formatToolExecution()` (bold name +
  `JSON.stringify(args, null, 2)` + the full output) is the `else` of `hasRendererDefinition()`,
  i.e. it is reached **only when the tool has no definition at all**.

The definition registry covers every non-builtin tier, which is why the fallback path — not
`formatToolExecution` — is the normal one for extension/SDK/MCP tools:
[`agent-session.ts:2659-2676`](../../tmp/pi/packages/coding-agent/src/core/agent-session.ts) builds
`definitionRegistry` over the builtins plus the registered and custom tools and stores it as
`this._toolDefinitions` (`:2676`); `getToolDefinition(name)` is at `:940` and is handed to the
render context at `:3413`.

## Current state in cyrup-tui

**The dispatch is a single boolean.**
[`transcript/tool_render.rs:33-46`](../../crates/cyrup-tui/src/transcript/tool_render.rs):

```rust
if run.rendered_call.is_some() || run.rendered_result.is_some() {
    render_extension(run, expanded, theme, &mut block);
} else {
    match run.name.as_str() {
        "read" => …, "write" => …, "edit" => …, "bash" => …, "grep" => …, "find" => …, "ls" => …,
        _ => render_generic(run, theme, &mut block),
    }
}
```

- **`render_generic`** ([`transcript/tool_builtin.rs:409-426`](../../crates/cyrup-tui/src/transcript/tool_builtin.rs))
  is the sink for every tool name that is not one of those seven and has no extension renderer. It
  pushes the name, a blank line, the FULL `serde_json::to_string_pretty(&run.args)` and the FULL
  `result_text(result)` ([`transcript/tool_result.rs:59`](../../crates/cyrup-tui/src/transcript/tool_result.rs)).
  It does not even take `expanded` or an expand key as a parameter, so no cap and no hint are
  possible. That is pi's `formatToolExecution` — used where pi would use the two fallbacks.
- **`render_extension`** ([`transcript/tool_builtin.rs:390-406`](../../crates/cyrup-tui/src/transcript/tool_builtin.rs))
  gates its whole body on `if let Some(result) = &run.rendered_result` (`:399`), so a renderer that
  supplies only `renderCall` produces a header and nothing else. Its own doc comment even says "a
  missing result text simply omits the body" — which is the divergence.
- The two sides **are** resolved independently one tier down —
  [`cyrup-ext/src/facade.rs:1069`](../../crates/cyrup-ext/src/facade.rs)
  `render_tool_call_outcome` and `:1089` `render_tool_result_outcome`, called from
  [`app/extension_render.rs:162-163`](../../crates/cyrup-tui/src/app/extension_render.rs) — and
  `ToolRun` keeps them in separate fields
  ([`transcript/entry.rs:190-198`](../../crates/cyrup-tui/src/transcript/entry.rs)
  `rendered_call` / `rendered_result`). Only the renderer's dispatch collapses them.
- **The hint string builder already exists and is correct.**
  [`transcript/tool_args.rs:99-116`](../../crates/cyrup-tui/src/transcript/tool_args.rs)
  `more_lines_hint(remaining, total, key, theme)` emits pi's exact three-colour
  `... (N more lines, <key> to expand)` line, with the live `app.tools.expand` label. **Reuse it —
  do not rewrite it.**
- **There is no "has a definition" signal.** `ToolRun`
  ([`transcript/entry.rs:170-215`](../../crates/cyrup-tui/src/transcript/entry.rs)) is constructed by
  `push_tool_start_rendered` ([`transcript/tool_state.rs:22-44`](../../crates/cyrup-tui/src/transcript/tool_state.rs))
  from `(name, call_id, args, rendered)` only. `ExtensionHost::has_tool_renderer`
  ([`cyrup-ext/src/facade.rs:1449`](../../crates/cyrup-ext/src/facade.rs)) is the **wrong**
  predicate: its own doc says it answers "can anything outside the built-in table render this tool
  name", i.e. has a RENDERER, not has a DEFINITION.
- No test exercises `render_generic` — `src/tests/tool_render.rs` and `src/transcript/tests/` do not
  reach it.

## Subtasks

1. **Plumb `expanded` and the expand key into `render_generic`.**
   [`tool_builtin.rs:409`](../../crates/cyrup-tui/src/transcript/tool_builtin.rs) currently takes
   neither; every sibling already does (`render_write`, `render_bash`, `render_grep`, `render_find`,
   `render_ls` all take `expanded` and `images.expand_key` at
   [`tool_render.rs:37-44`](../../crates/cyrup-tui/src/transcript/tool_render.rs)). Pass
   `ImageOpts::expand_key` ([`tool_render.rs:103-104`](../../crates/cyrup-tui/src/transcript/tool_render.rs))
   the same way they do.
2. **Add a `has_definition` signal to `ToolRun`.** A new field on
   [`transcript/entry.rs:170-215`](../../crates/cyrup-tui/src/transcript/entry.rs), set through
   `push_tool_start_rendered` ([`transcript/tool_state.rs:22`](../../crates/cyrup-tui/src/transcript/tool_state.rs)),
   answering pi's `hasRendererDefinition()` — "the agent knows a definition for this tool name",
   which for cyrup means: a built-in name, an extension/SDK-registered tool, or an MCP-proxied tool.
   Source it where the tool-start event is turned into a `ToolRun`, from whatever registry the
   session already exposes; do **not** reuse `ExtensionHost::has_tool_renderer`. Default it to the
   conservative value for legacy/test constructors so existing call sites keep their current shape.
3. **Split the generic sink into pi's two shapes**, in
   [`transcript/tool_builtin.rs`](../../crates/cyrup-tui/src/transcript/tool_builtin.rs):
   - *definition, no renderer* (the new common case): bold tool name only — **no args JSON** — then
     the result text capped at **10 lines** when not `expanded`, with `more_lines_hint(remaining,
     None, expand_key, theme)` ([`tool_args.rs:99`](../../crates/cyrup-tui/src/transcript/tool_args.rs))
     appended when lines were dropped. Empty output emits no body at all (pi returns `undefined`,
     `tool-execution.ts:144-146`). Name the constant `10` after
     `FALLBACK_PREVIEW_LINES` (`tool-execution.ts:9`).
   - *no definition at all*: keep today's `render_generic` verbatim — name + full pretty args + full
     output (pi `formatToolExecution`, `:376-387`).
4. **Resolve the call side and the result side independently** at
   [`tool_render.rs:33`](../../crates/cyrup-tui/src/transcript/tool_render.rs), replacing the single
   boolean with pi's `getCallRenderer` / `getResultRenderer` shape (`tool-execution.ts:84-102`):
   - call: `run.rendered_call` if present, else the built-in header for a built-in name, else the
     new bold-name fallback;
   - result: `run.rendered_result` if present, else the built-in result renderer for a built-in
     name, else the new 10-line text fallback.
   This is what fixes `render_extension`'s missing-`rendered_result` hole
   ([`tool_builtin.rs:399`](../../crates/cyrup-tui/src/transcript/tool_builtin.rs)) without a special
   case.
5. **Keep the image tail intact.** The `image` content-block handling after the dispatch
   ([`tool_render.rs:47-90`](../../crates/cyrup-tui/src/transcript/tool_render.rs)) runs for every
   shape and must keep running for all of them, including the new fallback.

## Acceptance criteria

- [ ] `render_generic`'s signature takes `expanded: bool` and an expand-key `&str`
- [ ] `ToolRun` carries a has-a-definition flag distinct from `rendered_call` / `rendered_result`,
      and it is **not** sourced from `ExtensionHost::has_tool_renderer`
- [ ] A tool that HAS a definition and NO renderer renders as: the bold tool name, then at most 10
      lines of result text — and **no** `serde_json::to_string_pretty(&run.args)` anywhere in the
      block
- [ ] When that tool's output exceeds 10 lines, the block ends with the
      `... (N more lines, <key> to expand)` line produced by the existing
      `more_lines_hint` ([`tool_args.rs:99`](../../crates/cyrup-tui/src/transcript/tool_args.rs)) —
      `grep -c 'more lines' crates/cyrup-tui/src/transcript/` shows no second implementation
- [ ] With `expanded == true`, that same tool shows **all** output lines and no hint
      (`tool-execution.ts:149`)
- [ ] A tool with a definition and empty output renders the name line and nothing else
- [ ] A tool with NO definition still renders name + full pretty args + full output, unchanged
- [ ] An extension renderer supplying only `renderCall` now shows a result body: the built-in result
      renderer for a built-in name, otherwise the 10-line text fallback
- [ ] An extension renderer supplying only `renderResult` still shows a header (the built-in's, or
      the bold-name fallback)
- [ ] The image content-block tail at
      [`tool_render.rs:47-90`](../../crates/cyrup-tui/src/transcript/tool_render.rs) still runs for
      every dispatch shape
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test in `src/tests/tool_render.rs`,
      `src/tests/tool_result_images.rs` or `src/transcript/tests/` regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
