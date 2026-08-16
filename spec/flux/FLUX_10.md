---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_10 — The `ask_user_question` native tool

## OBJECTIVE

Close the only real capability gap between code-puppy flux and cyrup flux (spec
[§5.1](../flux.md)): an agent-callable tool that asks structured questions mid-turn, bridging
to `HostServices::select` / `confirm` / `input` under the `HumanInteractionLock`. Full design:
spec [§3.4.4](../flux.md). This task lands the tool; FLUX_12 sweeps the 25 `FLUX-GAP` prompt
sites to use it.

## SUBTASKS

### SUBTASK 1: `ask_tool.rs` — `AskUserQuestionTool`

Implement [`cyrup_core::Tool`](../../crates/cyrup-core/src/tool.rs) (:89) following the spec
§3.4.4 skeleton verbatim in shape:

- **Struct**: `host: Arc<OnceLock<Arc<dyn HostServices>>>` (shared with the extension —
  constructed in `init` from the same `OnceLock`, which `set_host_services` late-binds;
  FLUX_07 already added the field) + `params: serde_json::Value` (the schema).
- **Schema** (`parameters()`), mirroring code-puppy's tool:
  ```json
  {
    "type": "object",
    "properties": {
      "question": { "type": "string", "description": "The question to ask the user" },
      "header":   { "type": "string", "description": "Short category label shown with the question" },
      "options":  {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "label":       { "type": "string" },
            "description": { "type": "string" }
          },
          "required": ["label"]
        },
        "minItems": 2,
        "maxItems": 4
      },
      "multiple": { "type": "boolean", "description": "Allow selecting several options (default false)" }
    },
    "required": ["question", "options"]
  }
  ```
- **`execute`** (the load-bearing mechanics, all verified in spec §0.4):
  1. `host.get()` → `ToolError("ask_user_question: no interactive host")` when unbound.
  2. Deserialize params; validate 2–4 options.
  3. `host.human_interaction_lock()` → acquire the guard across the dialog
     ([`services.rs`](../../crates/cyrup-ext/src/host/services.rs) :153-187, :395) — a second
     question (or a permission dialog) must wait, never overlap.
  4. Hop off the async executor: `select` is a blocking sync host call →
     `tokio::task::spawn_blocking`.
  5. **Label projection** (the `UiKind::Select` flat-string-array constraint —
     `cyrup-session-svc/src/host_services.rs:1696-1702`): display rows are
     `"label — description"` (bare `label` when no description); the reply string maps back to
     the bare label (`back_to_label`, the `oauth_select` pattern at :1703-1730). Two options
     with identical labels resolve to the first, matching the upstream caveat.
  6. `multiple: true` → loop the same select with a synthetic `✔ Done` first row, accumulating
     labels until Done or cancel; result joins labels with `, `.
  7. Cancel (`None`, i.e. Esc) → the string `"(cancelled — no selection made)"` — a Result,
     not an Err, so the agent sees the cancellation as information.
  8. Return `ToolResult { content: vec![Content::text(answer)], ..Default::default() }` (the
     built-in construction, [`bash.rs`](../../crates/cyrup-tools/src/tools/bash.rs) :454).
- **`description()` / `label()` / `prompt_snippet()`**: give the model real text — e.g.
  description "Ask the user a structured multiple-choice question mid-task and return their
  selection; prefer this over plain-text questions when options are known". These feed the
  system prompt's tool section (trait defaults exist; override all three).

### SUBTASK 2: Register in `init`

```rust
api.register_tool(Arc::new(crate::ask_tool::AskUserQuestionTool::new(
    Arc::clone(&self.host_services),
)));
```

`register_tool` overrides a same-named built-in ([`native.rs`](../../crates/cyrup-ext/src/native.rs)
:313) — if cyrup-tools ever grows this tool natively, delete this impl (spec §3.4.4).

### SUBTASK 3: Build + behavioral check

```bash
cargo build -p cyrup-ext-flux && cargo build -p cyrup
```

- The tool appears in the session's tool list (model-visible).
- Drive it from the TUI: ask the agent to "use the ask_user_question tool to ask me which of
  three colors to use" — the select dialog opens with the three `label — description` rows;
  picking one returns the bare label as the tool result; Esc returns the cancelled string.
- Concurrency: trigger the tool while a second prompt is pending — the second waits on the
  lock rather than overlapping dialogs.
- Headless (`cyrup -p` with a scripted provider or a plain print run): `select` is
  default-denied → `None` → the cancelled string (no hang, no panic).

## RESEARCH NOTES

- `HostServices::select` signature: `select(&self, prompt, options: &Value, opts) ->
  Option<String>` ([`services.rs`](../../crates/cyrup-ext/src/host/services.rs) :203); options
  is a JSON array of strings (spec §0.4).
- `Tool::execute` receives no HostServices — capture via the `OnceLock` at construction (spec
  §3.4.4; the subagents `OnceLock` pattern,
  [`extension.rs`](../../crates/cyrup-ext-subagents/src/extension.rs) :139, :751-757).
- The 25 prompt sites that will use this tool are enumerated in spec §0.3; FLUX_12 performs
  the sweep. Do not edit prompts in this task.

## DEFINITION OF DONE

- [ ] Crate + binary build cleanly; the tool is model-visible.
- [ ] Interactive run: dialog opens, selection returns the bare label, Esc returns the
      cancelled string, `multiple: true` accumulates until `✔ Done`.
- [ ] The interaction lock serializes overlapping prompts.
- [ ] Headless run degrades to the cancelled string without hanging.

No tests to be written. No benchmarks to be written.
