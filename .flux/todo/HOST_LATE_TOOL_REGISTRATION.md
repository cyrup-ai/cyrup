---
stage: new
status: done
updated: 2026-08-22 06:00
---

# HA-1 / MCP-037: Give Native Extensions A Handle To register_late_tool

## Description

A native extension cannot register a tool after `init`. The machinery exists and is live — what
is missing is the **handle**. This is the single item blocking the most other work: `MCP-394`
(the last open `critical`), plus `MCP-039`, `MCP-152`, `MCP-193` and `MCP-395` are the same seam
seen from five subsystems.

Everything downstream already works: `ExtensionHost::register_late_tool`
(`crates/cyrup-ext/src/facade.rs:645`) writes the registry and raises the dirty flag;
`refresh_tools` (`facade.rs:569`) consumes it; `AgentSession::refresh_extension_tools`
(`crates/cyrup-session-svc/src/session.rs:5678`) merges and calls `push_active_tools`, which
rewrites `Agent::set_tools` **and** the system prompt at every turn boundary in a live run.

The break is that a native extension's only host handles are the `Arc<dyn HostServices>` from
`set_host_services` and the per-dispatch `HostCtx`, and `HostServices` exposes five tool-shaped
verbs (`active_tools`, `all_tool_names`, `set_active_tools`, `all_tools`, `commands` —
`crates/cyrup-ext/src/host/services.rs:641-702`), all read-or-restrict. **None adds.** The WASM
tier reaches the same registry through `register-tool` in `crates/cyrup-ext/wit/world.wit:504`,
so this is a two-tier asymmetry in one verb, not an absent capability.

Two acceptable shapes, per `docs/gap-analysis/13a-mcp-activation.md:1817`:

- **(i)** a defaulted `NativeExtension::set_ext_host(&self, host: Weak<ExtensionHost>)` called
  from `load_native_with_services` beside the existing `set_host_services` — one method, one call
  site, `Weak` avoiding the cycle; or
- **(ii)** defaulted `HostServices::{register_late_tool, register_late_command}` backed by a
  late-attach sink, the shape `set_overlay_sink` / `set_inject_sink` already use.

Take `register_late_command` with it (`MCP-039`) and MCP-036's renderer declaration — a tool
registered mid-session currently has no way to declare a renderer.

**MCP-037a, the defect that made the handle worthless, is already fixed** (PR #30): `refresh_tools`
returned the guest materializer's verdict rather than the flag's, so a native late registration
always reported "nothing changed" while `take_tools_dirty`'s `swap(false)` destroyed the signal.
Test at `crates/cyrup-ext/src/tests/seam_liveness.rs:242`.

## Consequence today

On a cold `mcp-cache.json` the first session exposes only the `mcp` proxy tool; direct tools and
prompt commands appear next session. `mcp({connect:"x"})` cannot surface tools mid-session, the
proxy tool's description is frozen for the session, and `settings.disableProxyTool` must be
treated as unsupported — hiding a tool you cannot re-register is one-way.

## Acceptance Criteria

- [ ] A native extension can register a tool and a command after `init`
- [ ] `cyrup-mcp` uses it: connecting a server mid-session surfaces its prefixed tools in the next turn's tool list
- [ ] A renderer can be declared for a tool registered mid-session, or the loss is recorded
- [ ] The verify test runs **twice** — `--features wasm-host` and `--no-default-features`; both must pass
- [ ] `settings.disableProxyTool` is honoured, or its remaining limits are stated
- [ ] `MCP-037`, `MCP-039`, `MCP-152`, `MCP-193`, `MCP-394`, `MCP-395` rows updated in `13-cyrup-mcp-STATUS.md`
