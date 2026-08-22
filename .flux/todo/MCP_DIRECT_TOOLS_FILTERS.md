---
stage: new
status: done
updated: 2026-08-22 06:00
---

# MCP-370: Port includeTools And Glob excludeTools Into The In-Tree Reader

## Description

`MCP-370` is still `partial` after wave 1 (`docs/gap-analysis/13-cyrup-mcp-STATUS.md:47`). The
`ToolPrefix` half landed — the reader now carries upstream's four prefix modes — but the filter
half did not: `includeTools` and glob `excludeTools` are unported in
`crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`.

Consequence: **the reader over-approximates what the adapter actually registers.** It believes a
server exposes tools that `cyrup-mcp` filters out, so the two sides disagree about the tool
surface — the same class of reader/writer drift that wave 1 existed to close.

Wave 1 also added `cyrup-mcp` to that crate's **`[dev-dependencies]` only**, so conformance tests
assert against the writer itself rather than against constants copied out of it. Use that seam:
a test that compares the reader's resolved tool set to the writer's for the same definition makes
this class of drift impossible rather than merely detectable.

## Acceptance Criteria

- [ ] `includeTools` is honoured by the reader
- [ ] `excludeTools` supports globs, matching the writer's semantics
- [ ] A conformance test asserts reader and writer resolve the SAME tool set for a definition using both filters
- [ ] `MCP-370`'s row in `13-cyrup-mcp-STATUS.md` is updated
- [ ] `cargo nextest run --workspace` and `cargo clippy --workspace --all-targets` are clean
