---
stage: qa
status: completed
updated: 2026-08-27 05:00
---

# Decompose api/openai_codex_responses.rs Into Submodules


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

`crates/cyrup-provider/src/api/openai_codex_responses.rs` is 2,266 lines (1,294 production +
973 test). It is the **cleanest-banner file in the crate** — eight `// ---` sections, none of them
oversized, each already a coherent concern. Its banners map 1:1 onto modules with essentially zero
judgement required, which makes it the right one to do first.

Measured (verifier reproduced every number exactly — all 8 banner offsets, all 9 section sizes,
51 items, 17 pub, 34 test fns, single `cfg(test)` at L1294):

```
L123  Typed per-API options      114 lines
L237  ApiImpl                    284
L521  URL / session               45
L566  Auth & headers             119
L685  Request encoding           149
L834  Retry decisions            203
L1037 Codex -> Responses events  210
L1247 Terminals                   47
      preamble (consts, type alias) 122
```

The retry-decision block (`is_terminal_rate_limit_error`, `is_retryable_error`,
`get_retry_after_delay_ms`, `validate_retry_delay_ms`, `backoff_delay_ms`) is self-contained
arithmetic currently reachable only by scrolling a 2,266-line buffer.

`impl ApiImpl for CodexResponsesApi` (262 lines) is a trait impl and moves whole into `driver.rs`.

## Reference

Follow `crates/cyrup-provider/src/api/bedrock_converse_stream/` exactly — the worked example
(4,721 lines -> 16 modules + 10-file `tests/`). Read it first. Required properties:

- **Pure code movement.** No logic rewrites, no reordering, no tidying bundled in. If you find a
  bug while splitting, report it; do not fix it here.
- **Port-fidelity comments are load-bearing.** Every `pi <file>:<line>`, PROV-0xx and CYRUP-DELTA
  comment moves verbatim with the code it annotates.
- **Never run `cargo fmt`** — the crate is not rustfmt-clean and it would rewrite moved code.
- **Visibility minimized**, not uniformly widened: widen to `pub(super)`, then strip every
  `pub(super)` and restore only what the compiler demands. Note that `E0616` carries no declaration
  span (resolve private-field errors through the struct name) and glob imports raise `E0425`, not a
  privacy error, for items the test tree reaches via `use super::*`.
- Each banner title becomes the module's `//!` header, so no new prose is authored.

## Acceptance Criteria

- [ ] `openai_codex_responses.rs` is replaced by `openai_codex_responses/` (~9 modules + `tests/`)
- [ ] No module exceeds ~350 lines
- [ ] Item-name multiset is identical before and after (extract `fn`/`struct`/`enum`/`const` names from both, sort, diff -> empty)
- [ ] Every file differs from its pre-split content by nothing but the `pub(super)` token, module headers and `use` blocks
- [ ] `api/mod.rs` is unedited — `pub mod openai_codex_responses;` resolves to the directory
- [ ] `cargo build -p cyrup-provider --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-provider --all-targets` — still 14 warnings, none new in this module
- [ ] `cargo doc -p cyrup-provider --no-deps` — still **0** warnings (broken links are denied, so any new one is a build error)
- [ ] `cargo test -p cyrup-provider --lib` — 1118 pass; the 34 tests in this module all still run
