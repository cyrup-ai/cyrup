---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the tool access-extractor registry seam

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | absent |
| **Upstream area** | prompts/presentation — tool-access-extractor-registry |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream lets a sibling extension register a per-tool access-intent extractor so the cross-cutting
`path` and `external_directory` gates can see the filesystem path an extension/MCP tool will touch
when it is not under the default `input.path` key; the port has no such registry and only
recognizes a literal `path`/`file_path` key.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/tool-access-extractor-
registry.ts:29-31 (ToolAccessExtractor type), :39-56 (ToolAccessExtractorRegistry.register with
duplicate-throw and identity-guarded disposer); /home/user/cyrup/tmp/pi-packages/packages/pi-
permission-system/src/access-intent/tool-input-path.ts:29-51 (getToolInputPath consults the
registered extractor for `skill`/`extension` tools, and `input.arguments.path` for mcp); consumed
by /home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/handlers/gates/path.ts:23
and /home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/handlers/gates/external-
directory.ts:25; exposed publicly at /home/user/cyrup/tmp/pi-packages/packages/pi-permission-
system/src/service.ts:173 and permissions-service.ts:87-90

**Port** (`crates/cyrup-permission-system`):

`rg -ni "extractor|register_tool_access|ToolAccessExtractor|access_extractor"
/home/user/cyrup/crates/cyrup-permission-system/src` → 0 matches. The port's only access-path
derivation is /home/user/cyrup/crates/cyrup-permission-system/src/gate.rs:110-125
(`get_path_bearing_tool_path`), which returns None unless the input carries a literal
`path`/`file_path` key, and there is no `arguments.path` arm (`rg -n '"arguments"' src -g
'!tests'` → 0 matches).

## Why it matters

An extension or MCP tool whose input names its target under any other key (e.g.
`{"arguments":{"path":"/etc/shadow"}}`, `{"target_file":...}`, `{"dest":...}`) is invisible to the
path and external_directory gates, so it reads/writes outside the working directory with no ask
and no deny. There is also no public seam for a sibling extension to close that hole for its own
tools.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
