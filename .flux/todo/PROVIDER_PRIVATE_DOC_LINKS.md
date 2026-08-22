---
stage: new
status: done
updated: 2026-08-22 22:40
---

# Fix The 7 Broken Intra-Doc Links Only --document-private-items Can See

## Description

Three rustdoc invocations on `cyrup-provider`, keyed by `file:line :: warning` rather than compared
by count alone:

```
cargo doc -p cyrup-provider --no-deps                              -> 75 warnings
cargo doc -p cyrup-provider --no-deps --document-private-items     -> 81
cargo doc -p cyrup-provider --no-deps --all-features --document-private-items -> 81 (different set)
```

The private-items delta is **exactly 6**, a strict superset (nothing in the plain run disappears),
and `--all-features` swaps `unresolved link to \`faux\`` for one real warning inside the
feature-gated `faux` module. That gives **7 in-scope sites across 3 areas**:

- **4 in `bedrock_converse_stream/`** — bare sibling-module names that stopped resolving after the
  split: `failure.rs:10` -> `format_bedrock_error`, `options.rs:53` -> `convert_tool_config`,
  `params.rs:22` -> `split_command_input`, `sigv4.rs:90` -> `is_reserved_header`.
- **2 in `api/openai_*.rs`** — docs on production functions linking to `#[cfg(test)]`-only test
  wrappers, which exist in no doc or release build.
- **1 in `faux.rs`** — prose `[text,image]` parsed as a link; invisible because the module sits
  behind the non-default `faux` feature.

**This is the only doc scope not already owned by CARGO_DOC_WARNINGS.md**, and that was proved
rather than assumed: applying all 7 fixes takes the private-items run 81 -> 74 while the plain run
stays at exactly 75, so the other task's scope is untouched.

**One fix needs more than a path.** `is_reserved_header` is module-private, so path-qualifying the
link alone still fails — it needs a visibility bump to `pub(super)`. That is 8 files touched for
7 links.

The fix pattern is the one already applied to `sanitize_surrogates` and `ToolDef` in
`bedrock_converse_stream/mod.rs`: give the link an explicit path, keeping the rendered text
identical. For the two test-wrapper links, drop the link and leave the name in plain backticks —
linking production docs at a test-only item is the actual defect.

These comments are dense port-fidelity citations that engineers navigate by, so fix the links;
never delete the reference to make the warning go away.

## Acceptance Criteria

- [ ] `cargo doc -p cyrup-provider --no-deps --document-private-items` drops 81 -> 74
- [ ] `cargo doc -p cyrup-provider --no-deps` still reports **exactly 75** (CARGO_DOC_WARNINGS scope unmoved)
- [ ] `cargo doc -p cyrup-provider --no-deps --all-features --document-private-items` has no unresolved link in `faux.rs`
- [ ] No doc reference is deleted — every one is repaired
- [ ] `is_reserved_header` visibility bump is the minimum that makes the link resolve
- [ ] `cargo build -p cyrup-provider --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-provider --all-targets` — warning count unchanged from its value when this task starts
