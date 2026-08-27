---
title: Rendered tool paths are not OSC-8 hyperlinks
priority: LOW
tool: all
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: in-progress
updated: 2026-08-27
---

# Rendered tool paths are not OSC-8 hyperlinks

## What pi does

`renderToolPath` wraps the styled, `~`-shortened path in `linkPath(...)` (core/tools/render-utils.ts:84), and `linkPath` emits an OSC-8 hyperlink to the `file://` URL of the resolved absolute path whenever the terminal advertises the capability (render-utils.ts:19-23: `if (!getCapabilities().hyperlinks) return styledText; const absolutePath = resolvePath(rawPath, cwd); return hyperlink(styledText, pathToFileURL(absolutePath).href)`). Every built-in tool header path goes through this.

## What cyrup-tools does

`tool_path_span` reproduces the `[invalid arg]` / `emptyFallback` / `...` / accent-styled-shortened-path branches but emits a plain `Span` with no escape sequence, and its own doc records the omission: "Hyperlinks are a terminal escape the cell grid does not carry (tracked residual)" (crates/cyrup-tui/src/transcript/tool_args.rs:26-43). The crate does implement OSC-8 elsewhere — `crates/cyrup-tui/src/markdown/mod.rs:139-161` and `walk.rs:562-575` — so the capability exists but is not applied to tool paths.

## User-visible impact

In a hyperlink-capable terminal, clicking/ctrl-clicking the file path in a `read`/`edit`/`write` tool row opens the file under pi; under cyrup the path is inert text.

## Parity action

Thread the terminal hyperlink capability into `tool_args::tool_path_span` and emit an OSC-8 sequence around the styled path using `resolve_to_cwd(raw_path, cwd)` converted to a `file://` URL, matching `linkPath`'s capability gate.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Searched the whole workspace for OSC-8 emission: `rg '\]8;;' --glob '*.rs'` hits only ANSI-stripping code (crates/cyrup-tui/src/ansi.rs:247,292), sanitizer tests (tests/tool_result_sanitize.rs:63), OAuth URL-injection tests (cyrup-provider/src/auth/oauth/github_copilot.rs:1578) and doc comments. There is no `fn hyperlink`, no `file://` URL construction, and `rg hyperlink crates/cyrup-tools/src crates/cyrup-core/src` returns nothing. `tool_path_span` (tool_args.rs:26-43) does emit a plain accent-styled `Span` with no escape, so the clickable-path capability is genuinely absent — no different-named or differently-shaped implementation exists.

Two corrections to the claim's reasoning, though. (1) Its premise that "the crate does implement OSC-8 elsewhere" is FALSE. markdown/mod.rs:141-161 and markdown/walk.rs:558-576 do not emit any escape; they only consult `hyperlinks_supported()` to decide whether to append the legacy ` (url)` fallback suffix, and the doc there states outright that the hyperlink-capable branch "emits the link text alone… omitting the (unrepresentable) clickable wrapper". So this is not "capability present but unapplied to tool paths"; the crate emits zero OSC-8 in the transcript. (2) The omission is architectural and crate-wide, not a tool-path oversight: transcript `Line`s go through `Paragraph…wrap()` and `Terminal::insert_before`, where a stored escape would be re-wrapped as literal cell text (see the same reasoning at image.rs:342-350 for why images are half-blocks). login_dialog.rs:41-47 records the same crate-wide decision and names the tracked item (TUI-020) that would add paint-time escape injection. So the gap is real but is one residual with a single cause, not an `all`-tools defect.

Severity corrected to low: nothing renders incorrectly and nothing is silently wrong — the `~`-shortened path is fully visible, correctly styled and copy-pasteable, and many terminals ctrl-click plain paths anyway. The only loss is a click affordance.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
