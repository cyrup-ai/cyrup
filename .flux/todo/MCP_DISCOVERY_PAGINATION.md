---
stage: new
status: done
updated: 2026-08-22 06:00
---

# MCP-119: Paginated Discovery With Capability Gating

## Description

`crates/cyrup-mcp` has **no discovery at all**. Rated `high`, section `13c`, verdict `missing` in
`docs/gap-analysis/13-cyrup-mcp-STATUS.md:630`. This is the larger half of what was scoped as
"wave 6" after the transport work landed in PR #30.

Port upstream's discovery from `pi-mcp-adapter` @ `v2.26.1` (`fafae21`). Four obligations:

1. **Unconditional `list_all_tools`** with errors propagating rather than swallowed.
2. **Capability gating** for `resources` and `prompts`, read from
   `RunningService::peer_info() -> InitializeResult.capabilities` — do not call a list method the
   server never advertised.
3. **Per-list failure policy**, which differs per list and is the part most likely to be
   flattened by mistake: tools abort and re-throw on 401; resources degrade to a silent `[]`;
   prompts have their own arm. Read the unit body in
   `docs/gap-analysis/13c-mcp-servers.md` for the exact matrix rather than inferring it.
4. **Pagination** across all three lists.

Upstream is checked out at `tmp/pi-mcp-adapter` (clone it again if the container was recycled:
`github.com/nicobailon/pi-mcp-adapter`, tag `v2.26.1`). Node 22 can execute the TypeScript
directly with `node --experimental-strip-types`, which is how the hashing divergences in PR #30
were settled — prefer measuring upstream's behaviour to reasoning about it.

## Acceptance Criteria

- [ ] `list_all_tools` paginates and propagates errors
- [ ] `resources`/`prompts` are called only when `peer_info()` advertises the capability
- [ ] The per-list failure policy matches the matrix in `13c-mcp-servers.md`, including the 401 re-throw
- [ ] A test drives each failure arm against a fixture, not just the happy path
- [ ] `MCP-119`'s row in `13-cyrup-mcp-STATUS.md` is updated with what landed
- [ ] `cargo nextest run --workspace` and `cargo clippy --workspace --all-targets` are clean
