---
stage: new
status: done
updated: 2026-08-22 22:40
---

# Decompose api/openai_completions.rs Into Submodules

## Description

`crates/cyrup-provider/src/api/openai_completions.rs` is 3,657 lines (2,224 production +
1,433 test) — **the largest production half in the crate**. One `mod tests` at L2225 with 34 test
fns. This module is the shared OpenAI-compatible path used by many providers, so its size is felt
by anyone touching any OpenAI-compatible wire behaviour.

Proposed modules, drawn from the item map (verifier reproduced the structure; the only drift was
~6 lines on the production/test cut and an off-by-one on item counts, neither load-bearing):

```
driver.rs     L44-179     impl ApiImpl
headers.rs    L180-270    resolve_url, chat_completions_url, build_headers
params.rs     L271-690    reasoning_effort, provider_env_value, resolve_cache_retention,
                          build_body_with_env, apply_sampling_params, apply_reasoning,
                          build_chat_template_kwargs
cache.rs      L691-790
tools.rs      L791-861    message_has_tool_use, deferred_tool_names, tools_by_name, convert_tools
transform.rs  L862-1190
convert.rs    L1191-1530  convert_messages, build_assistant, user_content, join_text
blocks.rs     L1531-...   Block, Decoder, blocks_to_content, REASONING_FIELDS
```

`params.rs` at ~420 lines is the one section that may want sub-splitting; decide from the item
boundaries rather than by line count alone.

**Coordination note:** `provider_env_value` and `resolve_cache_retention` in this file are also
targets of PROVIDER_SHARED_PLUMBING.md. Do not run both tasks concurrently on this file. If the
dedup lands first, the split simply carries the shared call sites along.

## Reference

Follow `crates/cyrup-provider/src/api/bedrock_converse_stream/` exactly. Read it first. Required
properties: pure code movement; port-fidelity comments move verbatim; **never run `cargo fmt`**;
visibility minimized by strip-and-restore under the compiler; banner titles become `//!` headers.

## Acceptance Criteria

- [ ] `openai_completions.rs` is replaced by `openai_completions/` (~9 modules + `tests/`)
- [ ] No module exceeds ~350 lines
- [ ] Item-name multiset identical before and after (sort, diff -> empty)
- [ ] Every file differs from its pre-split content by nothing but the `pub(super)` token, module headers and `use` blocks
- [ ] `api/mod.rs` unedited; other api impls that import from this module still compile untouched
- [ ] `cargo build -p cyrup-provider --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-provider --all-targets` — still 37 warnings, none new here
- [ ] `cargo doc -p cyrup-provider --no-deps` — still 76 warnings, none new here
- [ ] `cargo test -p cyrup-provider --lib` — 1118 pass, 34 tests still run in this module
