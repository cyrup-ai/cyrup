---
stage: new
status: done
updated: 2026-08-22 22:40
---

# Decompose google_generative_ai.rs And mistral_conversations.rs

## Description

Two mid-sized api impls, grouped into one task because verification found each too small to
justify a task of its own but both worth doing while the pattern is fresh. **They are independent
files — split them in either order.**

### `api/google_generative_ai.rs` — 2,300 lines (1,434 production + 866 test)

The coarsest-grained file of the set: only **2 section banners for 1,434 production lines** —
`// Request encoding` at L181 (764 lines) and `// Response decoding`. Both sections are far too
large to become single modules, so unlike the other decompositions this one **requires real
judgement about where the sub-seams go** — the banners alone will not draw them. One `mod tests`
at L1435 with 24 test fns (21 `#[test]` + 3 `#[tokio::test]`), plus a small inline `#[cfg(test)]`
fixture `build_body` at L289-295 inside the production half.

### `api/mistral_conversations.rs` — 1,776 lines (1,305 production + 471 test)

Three banners: L183 Request encoding, L641 Tool-call-id normalization, L701 Response decoding.
45 top-level production items, 12 already `pub`/`pub(crate)`. Largest production item is
`to_chat_messages` at 115 lines. Two `#[cfg(test)]` regions: a 7-line `build_body` fixture at
L314-320 and the 464-line `mod tests` at L1313.

Because both files carry an inline `#[cfg(test)]` fixture inside the production half, neither can
be split by the naive "everything after the last `#[cfg(test)]` is tests" rule.

## Reference

Follow `crates/cyrup-provider/src/api/bedrock_converse_stream/` exactly. Read it first. Required
properties: pure code movement; port-fidelity comments move verbatim; **never run `cargo fmt`**;
visibility minimized by strip-and-restore under the compiler.

For `google_generative_ai.rs` specifically: where you invent a seam the banners do not draw, say
in the module's `//!` header what concern it holds, and keep the split along existing item
boundaries — do not reorder items to make a tidier module.

## Acceptance Criteria

- [ ] Both files are replaced by directories; no module exceeds ~350 lines
- [ ] Both inline `#[cfg(test)]` fixtures are handled, not lost
- [ ] Item-name multiset identical before and after, per file (sort, diff -> empty)
- [ ] Every file differs from its pre-split content by nothing but the `pub(super)` token, module headers and `use` blocks
- [ ] `api/mod.rs` unedited; `google_vertex.rs` still compiles untouched
- [ ] `cargo build -p cyrup-provider --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-provider --all-targets` — still 37 warnings, none new here
- [ ] `cargo doc -p cyrup-provider --no-deps` — still 75 warnings, none new here
- [ ] `cargo test -p cyrup-provider --lib` — 1118 pass; 24 + 15 tests still run in these modules
