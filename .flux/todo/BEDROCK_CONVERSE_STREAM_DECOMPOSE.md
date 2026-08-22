---
stage: new
status: done
updated: 2026-08-22 00:00
---

# Decompose Bedrock Converse Stream Into Submodules

## Description

decompose cyrup-provider	src/api/bedrock_converse_stream.rs — 4,721
into logical decomposed submodules based on separation of concerns

The file `cyrup-provider/src/api/bedrock_converse_stream.rs` is ~4,721 lines. Split it
into a `bedrock_converse_stream/` submodule directory whose children are organized by
separation of concerns (e.g. request construction, wire/event types, SSE or event-stream
frame decoding, delta/chunk accumulation, tool-use handling, error mapping, and the
public streaming driver), with `mod.rs` re-exporting the existing public surface.

## Acceptance Criteria

- [ ] `bedrock_converse_stream.rs` is replaced by a `bedrock_converse_stream/` module directory
- [ ] Each submodule has a single, clearly named concern; no submodule is disproportionately large
- [ ] The crate's public API is unchanged — all previously exported items remain exported from the same paths
- [ ] Pure code movement: no behavior changes, no logic rewrites bundled into the split
- [ ] `cargo build` and `cargo clippy` pass with no new warnings
- [ ] Existing tests pass unchanged
