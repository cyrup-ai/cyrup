---
stage: new
status: done
updated: 2026-08-22 22:40
---

# Decompose api/anthropic_messages.rs Into Submodules


## Base has changed — read before starting

This branch is based on main, which now sets `rustdocflags = ["--document-private-items"]` in
`.cargo/config.toml` and `rustdoc::broken_intra_doc_links = "deny"` workspace-wide. **A broken
intra-doc link is now a build error, not a warning.**

That directly affects this task. When a file is split, doc comments that referenced a sibling item
by bare name (`[`some_helper`]`) stop resolving, because the item is no longer in the same module —
this is exactly what happened to four links in the bedrock split. Repair them as part of the split
by giving the link an explicit path, e.g. ``[`format_bedrock_error`](super::errors::format_bedrock_error)``.
Never delete the reference to silence the error; these are port-fidelity cross-references.

Measured baseline on this branch (use these, not any older numbers):

```
cargo build -p cyrup-provider --all-targets   -> 0 errors, 0 warnings
cargo clippy -p cyrup-provider --all-targets  -> 14 warnings
cargo doc -p cyrup-provider --no-deps         -> 0 warnings
cargo test -p cyrup-provider --lib            -> 1118 pass, 7 ignored, 0 fail
```

Of the 14 clippy warnings, 13 are `return_self_not_must_use` from main's lint policy in files this
task does not touch, and 1 is the deliberately-accepted `result_large_err` on `run_inner`. Do not
"fix" either; just do not add to them.

## Description

`crates/cyrup-provider/src/api/anthropic_messages.rs` is 3,968 lines (1,970 production +
1,999 test) — **the largest file left in the crate** and the closest structural twin of the
bedrock original: same banner discipline, same ~50/50 production-to-test ratio, largest single
production item only 190 lines (no giant inherent impl to unpick).

Measured banners:

```
L231  Compat resolution (Pi getAnthropicCompat, anthropic-messages.ts:170-181)   134 lines
L365  Cache retention (Pi resolveCacheRetention / getCacheControl)                50
L415  Claude Code tool-name mapping (Pi anthropic-messages.ts:98-109)             57
L472  Request encoding                                                           876
L1355 Response decoding                                                          623
      preamble: API_ID, AnthropicThinkingDisplay, AnthropicOptions,
      CLAUDE_CODE_TOOLS, AnthropicMessagesApi, impl ApiImpl                       230
```

Two sections need sub-splitting; both divide along item boundaries that are already grouped in
file order:

- **Request encoding (876)** -> `headers` (`is_oauth_token`, `is_github_copilot`, `resolve_is_oauth`,
  `resolve_url`, `messages_url`, `should_use_fine_grained_beta`, `build_headers`), then `params`
  (`map_thinking_level_to_effort`, `build_params`, `tool_choice_wire`, `system_text`,
  `normalize_tool_call_id`), then `convert` (`convert_content_blocks`, `ToolAnchors`,
  `convert_tool_result*`).
- **Response decoding (623)** -> block/decoder state, then the stream driver, then usage accounting.

Note there are **two** `#[cfg(test)]` blocks (L642 and L1977), not one — the L642 one is an inline
fixture inside the production half and must be handled, not assumed away.

This is the crate's most-touched API module, so it pays back the split fastest.

## Reference

Follow `crates/cyrup-provider/src/api/bedrock_converse_stream/` exactly. Read it first. Required
properties: pure code movement (no logic rewrites, no reordering, no tidying); port-fidelity
comments move verbatim with their code; **never run `cargo fmt`**; visibility minimized by
stripping every `pub(super)` and restoring only what the compiler demands; each banner title
becomes the module's `//!` header.

## Acceptance Criteria

- [x] `anthropic_messages.rs` is replaced by `anthropic_messages/` (14 production modules + `mod.rs`
      + a 10-file `tests/` tree)
- [x] No module exceeds ~350 lines (largest: `tests/decode.rs` 311, `tests/headers.rs` 304,
      `params.rs` 287)
- [x] Both `#[cfg(test)]` regions are accounted for — the inline `build_body` fixture now sits in
      `params.rs`, the `mod tests` block became `tests/`
- [x] Item-name multiset identical before and after (127 items each, sorted diff empty)
- [x] Every file differs from its pre-split content by nothing but the `pub(super)` token, module
      headers and `use` blocks (plus five intra-doc links repaired with explicit paths)
- [x] `api/mod.rs` unedited
- [x] `cargo build -p cyrup-provider --all-targets` — 0 errors, 0 warnings
- [x] `cargo clippy -p cyrup-provider --all-targets` — 14 warnings, none in this module
- [x] `cargo doc -p cyrup-provider --no-deps --features faux` — 0 warnings, 0 errors
- [x] `cargo test -p cyrup-provider --lib` — 1118 pass, 7 ignored, 0 fail; 52 tests in this module
      before and after

### Note on the `cargo doc` gate

`cargo doc -p cyrup-provider --no-deps` **without** `--features faux` fails on a pre-existing
`unresolved link to `crate::faux`` in `src/unconfigured.rs:3` — `faux` is `#[cfg(any(test, feature =
"faux"))]`, so the link only resolves when the feature is on. That failure is untouched by this task
and cannot have been introduced by it — neither `unconfigured.rs` nor `lib.rs` is in this diff. The
baseline number is the `--features faux` run.
