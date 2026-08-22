---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the `shellTools` shell-alias registration

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | absent |
| **Upstream area** | config schema / bash enforcement reach |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream lets a config map non-bash tools that carry shell semantics (e.g. `exec_command` with a
`cmd` argument) so they are gated through the full bash enforcement stack; the port has no
`shellTools` key, so such a tool is gated only as a generic extension tool.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

config-schema.ts:113-146 (shellToolsSchema and its markdownDescription: "Recording the alias lets
the permission system gate that tool through the same bash enforcement stack as native `bash`");
config-loader.ts:245-256 (shallow-merge by tool name across global → project); extension-
config.ts:277-278 and :340-342 (`shellTools` on the runtime config)

**Port** (`crates/cyrup-permission-system`):

`rg -in 'shell_tools|shellTools|commandArgument|command_argument|workdir'
/home/user/cyrup/crates/cyrup-permission-system/src` returns nothing.
/home/user/cyrup/crates/cyrup-permission-system/src/manager.rs:220 gates the bash surface on the
literal tool name `"bash"` only; ext_config.rs:39-77 `ExtensionConfig` carries only `enabled`,
`debug`, `yolo_mode`, `forwarded_prompt_timeout_seconds`.

## Why it matters

An extension that replaces bash under another tool name executes arbitrary shell commands while
bypassing every `bash:` rule — its command string is never matched against bash patterns and falls
through to the generic `tools` category, where a single `allow` on the tool name grants
unrestricted shell.

## Also reported independently

Other area agents found this same gap from a different angle:

- **Route aliased shell tools through the bash enforcement stack** (access intent: bash parsing) — Upstream's resolveShellInvocation maps any tool named in the `shellTools` config to a
{command, workdir} pair so it is gated on the `bash` surface; the port recognizes only the
literal tool name `bash`.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
