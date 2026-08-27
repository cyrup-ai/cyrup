---
stage: todo
status: pending
updated: 2026-08-27
---

# Cut The Consumer Seam For `ExtensionHost::transform_markdown` Onto The Message Render Path

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** medium · **Kind:** missing-feature · **Area:** Markdown, latex, images, diffs and message rendering

## Objective

An extension that calls `registerMarkdownTransformer` currently loads, registers and is folded over
by a fully-implemented host method — and then nothing on screen changes, because no renderer ever
calls it. Wiring the consumer half makes registered transformers actually rewrite assistant,
thinking and user markdown before it is laid out, which is also the seam pi's built-in mermaid
transformer plugs into.

## Upstream reference

- [`packages/tui/src/components/markdown.ts:226`](../../tmp/pi/packages/tui/src/components/markdown.ts)
  declares `transform?: (markdown: string, availableWidth: number) => string` on `MarkdownOptions`,
  and `:285` applies it as the **first** thing `render()` does, with the exact content width:
  `const text = this.options.transform?.(this.text, contentWidth) ?? this.text`.
- Every message component supplies one:
  [`assistant-message.ts:111-113`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/assistant-message.ts)
  (`createMarkdownTransform("assistant", this.isStreaming, this.markdownTransformers)`), `:156-162`
  (`"assistant-thinking"`), and
  [`user-message.ts:53`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/user-message.ts)
  (`"user"`).
- [`markdown-transform.ts:12-28`](../../tmp/pi/packages/coding-agent/src/modes/interactive/markdown-transform.ts)
  folds the registered transformers in order, containing a throw per transformer and keeping the
  current text.

## Current state in cyrup-tui

The **producer** side is complete and documented; the **consumer** side does not exist.

| piece | where | state |
|---|---|---|
| host fold | [`crates/cyrup-ext/src/facade.rs:1201-1229`](../../crates/cyrup-ext/src/facade.rs) | `ExtensionHost::transform_markdown(&self, markdown, message_type, is_streaming, available_width) -> String`. Builds the `{messageType,isStreaming,availableWidth}` ctx and folds owners in load order, containing a faulting transformer (its doc: "A faulting transformer is CONTAINED and SKIPPED"). Backed by `registry.rs:519-531` and `native.rs:376,638`. **Done — do not re-implement the fold.** |
| callers | — | `grep -rn "transform_markdown\|markdown_transformers" crates/cyrup-modes crates/cyrup-agent crates/cyrup-tui crates/cyrup-session crates/cyrup-core crates/cyrup` returns **zero**. |
| render entry points | [`crates/cyrup-tui/src/markdown/mod.rs:100,117,143,155`](../../crates/cyrup-tui/src/markdown/mod.rs) | `render`, `render_with_text_color`, `render_with_default_style`, `render_with_hyperlink_support` — all funnel into `render_inner` (`:164-190`). None takes a callback; none has a `messageType`/`isStreaming` notion. `render_inner:171` does its own `text.replace('\t', "   ")` (`markdown.ts:171`) but no `transform`. |
| call sites, raw text | [`transcript/render.rs:25`](../../crates/cyrup-tui/src/transcript/render.rs) (user, `render_with_text_color`), [`:62`](../../crates/cyrup-tui/src/transcript/render.rs) (assistant, `render`), plus `transcript/cache.rs` (streaming partial) and `transcript/message.rs` (thinking / labeled body) | every one passes the untransformed text at `width - output_pad * 2`. |

The likely reason the seam was never cut: `ExtensionHost::transform_markdown` is `async` and
`markdown::render_inner` is sync.

## Subtasks

1. Pick the seam and record the choice in a doc comment: either (a) an optional
   `transform: Option<&dyn Fn(&str, usize) -> String>` threaded into
   `crates/cyrup-tui/src/markdown/mod.rs::render_inner` and its four public wrappers, driven by a
   pre-resolved result, or (b) — preferred, given the async host — apply the transform at
   **entry push/commit time** in the transcript, so the sync renderer keeps its signature.
2. **`crates/cyrup-tui/src/transcript/render.rs`** (and `cache.rs`, `message.rs`) — supply the
   `messageType` string per call site, matching upstream exactly: `"user"` (`render.rs:25`),
   `"assistant"` (`render.rs:62`), `"assistant-thinking"` (`message.rs`).
3. Supply `isStreaming` truthfully: the streaming-partial path in
   `crates/cyrup-tui/src/transcript/cache.rs` passes `true`; committed entries pass `false`
   (`assistant-message.ts:111`).
4. Supply `availableWidth` as **the same content width the renderer uses** —
   `width.saturating_sub(output_pad * 2).max(1)`, i.e. `markdown.ts:285`'s `contentWidth`, not the
   full container width.
5. Call `cyrup_ext::ExtensionHost::transform_markdown` (facade.rs:1201) from wherever the host handle
   already lives on the app side; when no host is present or no transformer is registered, the path
   must be a zero-cost passthrough (the host already early-returns on an empty owner list,
   `facade.rs:1209-1211`).

## Acceptance criteria

- [ ] `grep -rn "transform_markdown" crates/cyrup-tui crates/cyrup-modes crates/cyrup-agent` returns
      at least one **call site**, not just the definition.
- [ ] The user path passes `messageType == "user"`, the assistant path `"assistant"`, and the
      thinking path `"assistant-thinking"` — verifiable by reading `transcript/render.rs` and
      `transcript/message.rs`.
- [ ] The streaming-partial path passes `is_streaming == true` and the committed path `false`.
- [ ] The `available_width` argument at every call site is the same expression fed to
      `crate::markdown::render*` as `width` — `width.saturating_sub(output_pad * 2).max(1)` — not the
      untrimmed width.
- [ ] With no extension host, or with a host that has no markdown transformers, the rendered lines
      are byte-identical to today's.
- [ ] No new fold/containment logic is added in `cyrup-tui`; the existing
      `facade.rs:1201-1229` fold is the only one.
- [ ] `cargo build -p cyrup-tui` → 0 warnings; `cargo clippy -p cyrup-tui --all-targets` → no new
      diagnostics.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
