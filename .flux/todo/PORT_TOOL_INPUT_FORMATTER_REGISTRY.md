---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the tool-input formatter registry and built-in MCP formatter

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | absent |
| **Upstream area** | previews — tool-input-formatter-registry / builtin formatters |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream exposes a public per-tool preview-formatter registry (consulted before the built-in
dispatch) and registers a built-in MCP formatter that renders `arguments` as a readable summary,
with the `mcp` arm falling back to no preview rather than dumping raw JSON; the port has no
registry and routes `mcp` through the generic JSON preview.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/tool-input-formatter-
registry.ts:37-66 (registry, duplicate-throw, identity-guarded disposer); /home/user/cyrup/tmp/pi-
packages/packages/pi-permission-system/src/builtin-tool-input-
formatters.ts:16-19,30-44,55-70,78-82 (formatMcpInputForPrompt, value truncation at 60/160 chars,
registered as "mcp"); /home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/tool-
preview-formatter.ts:110-116 (custom formatter consulted first), :129-133 (`case "mcp": return ""`
— "produce no additional preview rather than leaking the raw event JSON"); public seam at
service.ts and permissions-service.ts (registerToolInputFormatter)

**Port** (`crates/cyrup-permission-system`):

`rg -ni "ToolInputFormatter|register_tool_input_formatter|input_formatter|formatter_registry|forma
t_mcp_input" /home/user/cyrup/crates/cyrup-permission-system/src` → 0 matches.
/home/user/cyrup/crates/cyrup-permission-system/src/gate.rs:605-614
(`format_tool_input_for_prompt`) has no custom-formatter lookup and no `"mcp"` arm — an mcp call
without a resolvable target falls through to `format_json_input_for_prompt` at :592-598.

## Why it matters

An operator approving an MCP or extension tool call sees a 200-character slice of raw JSON — with
the actual arguments frequently pushed past the cut — instead of the readable `key: value` summary
upstream renders, and a sibling extension has no way to make its own tool's ask legible. Approving
what you cannot read is the failure mode the preview exists to prevent.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
