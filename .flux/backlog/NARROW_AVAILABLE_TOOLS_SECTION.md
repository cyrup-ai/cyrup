---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Narrow the Available tools section instead of deleting it wholesale

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | behaviour-drift |
| **Upstream area** | sanitizers — system-prompt-sanitizer |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream filters the `Available tools:` section down to the still-exposed tools (keeping the
header and allowed bullets, dropping only denied ones) and bounds an unterminated section at the
first non-body line; the port deletes the entire section whenever it exists and extends an
unterminated section to the end of the prompt.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/system-prompt-
sanitizer.ts:148-193 (narrowAvailableToolsSection — keeps allowed bullets and non-tool prose,
removes the header only when no tool bullet survives, sets `removed` only when a bullet was
actually filtered), :123-135 (findSection: "No subsequent section header — stop at the first non-
body line so that content after the section (e.g. custom user notes) is not silently deleted"),
:97-103 (isSectionBodyLine)

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/sanitize/tools.rs:121-130
(`remove_line_section` drops `[start,end)` unconditionally) called at :199-200;
/home/user/cyrup/crates/cyrup-permission-system/src/sanitize/tools.rs:107-117 (`find_section` sets
`end = lines.len()` when no later top-level header is found — no body-line boundary); doc comment
at :183-186 states the intent: "remove the 'Available tools:' section entirely". `rg -n
"narrow_available_tools|is_section_body_line|extract_tool_bullet_name" src` → 0 matches.

## Why it matters

The model is left with no listing of the tools it is actually permitted to use, so it guesses
names and burns turns on calls the gate then blocks — the opposite of the sanitizer's stated
purpose. Worse, a prompt whose `Available tools:` section is last silently loses every line after
it, including operator- or user-authored instructions that were never about tools.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
