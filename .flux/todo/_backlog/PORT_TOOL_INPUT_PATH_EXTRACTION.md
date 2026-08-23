---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Extract the gated path from MCP and extension tool inputs

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | partial |
| **Upstream area** | access intent: path surfaces |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream's getToolInputPath dispatches on tool kind — `mcp` reads `input.arguments.path`, and
extension/skill tools consult a registered ToolAccessExtractor before falling back to `input.path`
— so those tools are no longer exempt from path gating; the port reads only a top-level
`path`/`file_path`.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/access-intent/tool-input-path.ts:29-52 (getToolInputPath: `case "mcp": return
getNonEmptyString(toRecord(record.arguments).path)`, `case "skill"/"extension":
extractors?.get(toolName)` then `record.path`; the doc states it "recognizes extension and MCP
tools so they are no longer exempt from path gating"); src/handlers/gates/external-directory.ts:27
and src/handlers/gates/path.ts:25 both call it

**Port** (`crates/cyrup-permission-system`):

crates/cyrup-permission-system/src/gate.rs:110-125 `get_path_bearing_tool_path` reads only
`record.get("path")`/`record.get("file_path")`, so an `mcp` input `{tool: "fs:read", arguments:
{path: "/etc/shadow"}}` returns `None`; extension/decide.rs:117-121 therefore skips the external-
directory guard. Negative grep: `rg -n "arguments\").*path|extractor"
/home/user/cyrup/crates/cyrup-permission-system/src` → 0 matches for any `arguments.path` read or
extractor registry.

## Why it matters

MCP filesystem servers and any extension tool whose path argument is not a top-level
`path`/`file_path` reach paths outside the working directory without the external_directory
ask/deny — the exact exemption upstream closed. The port's own gate.rs:100-108 doc calls this
guard "an ENFORCEMENT input, not merely cosmetic".

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute the MCP half. gate.rs:110-113 does
`record.get("path").or(record.get("file_path"))?` BEFORE any tool-kind test, so an mcp input
`{tool:"fs:read", arguments:{path:"/etc/shadow"}}` returns None at the `?` and
extension/decide.rs:115-121 skips the guard. Negatives: `rg -n '"arguments"'` over src/ -> 0 hits
anywhere in the crate; `rg -ni "extractor|tool_access|access_extractor"` -> 0. Upstream verified
at src/access-intent/tool-input-path.ts:29-52. Partial-credit correction the finder understated:
the EXTENSION half is largely covered already. gate.rs:114-121 ORs three recognizers —
PATH_BEARING_TOOLS, `has_structured_edit_payload` (gate.rs:88-98, which upstream only reaches via
a registered extractor), and `is_likely_filesystem_tool_name` (gate.rs:49-61) — and that heuristic
matches `read_file`, `grep_files`, `fs_search`, `list-dir`, any `*_read`/`*_write`. So the port
already gates extension tools that use the top-level `path` convention plus a filesystem-ish name;
gate.rs:73-87 documents this as a deliberate closed fail-open. What is genuinely missing is (a)
`mcp` -> `arguments.path` and (b) a ToolAccessExtractor registry for a non-conventional argument
name. Keep medium: the mcp category default is Ask (types.rs:55), so exploitation needs a
permissive mcp rule or an MCP baseline allow (manager.rs:262-274).

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
