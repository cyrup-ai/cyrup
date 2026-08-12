# cyrup gap analysis

A verified ledger of every behavioral difference between the **cyrup** Rust port and its four
TypeScript upstreams, written to be used as a work-item backlog.

**Start at [`00-residual-ledger.md`](00-residual-ledger.md)** — it ranks everything and suggests an
order. The numbered files hold the evidence.

> **Re-baselined 2026-08-03 against cyrup `1806375`**, 28 commits past the original `c8bd2ab`
> analysis. 54 items closed, 323 open (2 critical, 30 high, 156 medium, 135 low). Closed items keep
> their IDs in each area file's status table so a closure can be re-audited later.
>
> **Addendum 2026-08-11, HEAD `097bdde`** — the `github-copilot` provider was read end to end
> against pi. `PROV-005` closed; **three new highs** filed (`PROV-027`/`028`/`029`, area 01),
> raising the actionable-high count from 4 to 7. They live in code that did not exist when this
> analysis was written, which is the general lesson: **closing a "not implemented" item means the
> subsystem now exists, not that it is correct.** See the update block at the top of
> `00-residual-ledger.md`.

## Contents

| file | area | items |
|---|---|---|
| [`00-residual-ledger.md`](00-residual-ledger.md) | **ranked cross-cutting view — read first** | — |
| [`01-cyrup-core-and-provider.md`](01-cyrup-core-and-provider.md) | wire APIs, providers, auth, streaming, catalogs, cost | 23 |
| [`02-cyrup-agent.md`](02-cyrup-agent.md) | the turn loop, tool dispatch, hooks, abort | 14 |
| [`03-cyrup-session.md`](03-cyrup-session.md) | JSONL session tree, compaction, system prompt | 24 |
| [`04-cyrup-tools.md`](04-cyrup-tools.md) | the seven built-in tools | 26 |
| [`05-cyrup-config-and-resources.md`](05-cyrup-config-and-resources.md) | settings, model resolution, trust, skills, packages | 32 |
| [`06-cyrup-ext.md`](06-cyrup-ext.md) | extension host, WIT world, event catalog | 30 |
| [`07-cyrup-tui.md`](07-cyrup-tui.md) | terminal UI application layer | 33 |
| [`08-cyrup-session-svc-and-modes.md`](08-cyrup-session-svc-and-modes.md) | the integration seam, RPC, CLI, print/json modes | 28 |
| [`09-cyrup-ext-subagents.md`](09-cyrup-ext-subagents.md) | subagent delegation | 38 |
| [`10-cyrup-permission-system.md`](10-cyrup-permission-system.md) | allow / ask / deny gate | 22 |
| [`11-cyrup-intercom.md`](11-cyrup-intercom.md) | supervisor↔subagent broker | 23 |
| [`12-upstream-drift-pi-core.md`](12-upstream-drift-pi-core.md) | the 397 pi commits since cyrup HEAD | 32 |

Numbering follows the convention already referenced in cyrup's source
(`spec/gap-analysis/03-cyrup-agent.md`, `12-cyrup-tui.md`, `00-residual-ledger.md`). That `spec/`
tree is not in this workspace, so exact alignment with it is unverified.

## Baselines measured against

| repo | HEAD | cyrup ported |
|---|---|---|
| `cyrup/` | **`1806375`** 2026-08-03 (branch `david/cyrup`) — 28 commits past the `c8bd2ab` baseline the port was measured from | — |
| `pi/` | `a0bb4a48` 2026-07-31 | v0.83.0; **397 commits** since cyrup HEAD |
| `pi-subagents/` | `bc40535` 2026-07-31 | ~v0.33.x–v0.34.0; **170 commits**, +46k/−7.3k |
| `pi-permission-system/` | `9affcc9` 2026-07-03 | v0.7.1; **9 commits**, +4.0k/−1.9k |
| `pi-intercom/` | `cbf977b` 2026-07-30 | **v0.7.0** — its `lib.rs` says v0.6.0 but the code is v0.7.0; **14 commits** |

The intercom baseline is the one that bites: diffing from `v0.6.0` reports a pile of already-done
work as debt. Diff `v0.7.0..HEAD`.

## Item format

Every item is a `##` section with a stable id (`AREA-NNN`):

```
**Kind** parity-bug · **Severity** critical · **Effort** S · **Confidence** confirmed
**cyrup**    — cyrup/crates/…:LINE — what the code actually does
**upstream** — pi/packages/…:LINE — what upstream does
**Impact**   — the user-visible consequence
**Fix**      — concrete sketch naming files and functions
**Verify**   — how to prove it is fixed
```

**Kind** — `parity-bug` (ported but drifted) · `not-ported` (predates the baseline, never built) ·
`upstream-drift` (landed after the baseline; expected lag) · `stale-port` (cyrup carries behavior
upstream changed or deleted) · `cyrup-original` (no upstream basis) · **`test-defect`** (a test
pinning wrong behavior, or asserting a timing/scheduling outcome it cannot control).

Each area file also opens with a **status table** covering every item from the original analysis:
`closed` · `partially-closed` · `still-open` · `misdescribed` · `superseded`. Closed items keep
their IDs rather than being deleted, so a closure can be re-audited.

**Severity** is judged by user-visible consequence, not code size: `critical` = data loss, silent
wrong output, a permission bypass, or a crash on a normal path.

**Effort** — `S` under a day · `M` a few days · `L` a week+ or needs design.

## How this was produced

Twelve areas, each run through three independent passes: an analyst enumerating gaps with two-sided
evidence, an adversarial verifier instructed to **refute** every item and to default to rejection
when it could not personally re-read both sides, then a writer rendering only the survivors. Each
file's `## Coverage` section lists what was read, the blind spots, and every rejected item with its
reason — so a later reader can see what was already considered and dismissed rather than re-deriving
it.

On the **2026-08-03 re-baseline** the verifier's primary duty was inverted: rather than confirming
findings, it was told to **refute every `closed` claim**, on the grounds that a wrongly-closed item
deletes a real defect from the backlog and nobody looks again. Closure required reading the code at
HEAD; a commit message asserting a fix was explicitly treated as a hypothesis, not evidence. That
scepticism was warranted — several commits closed only the easy half (`DRIFT-001` shipped in two
halves, `SUBA-005` landed 4 of 9 actions, `TUI-002` shipped with a documented limitation).

The refresh also mined `git log` for debt that existed **only in commit messages** (deferred
subsystems, a deliberate WIT ABI break, known limitations), and ran a systematic hunt for the
`test-defect` class after three instances were found by accident. That hunt returned 27 more.

Known traps were fed to every pass so they would not be re-reported as discoveries: the
`loop_fn.rs` facade, pi's two forked compaction implementations, the provider `fleet!` macro hiding
20 registrations, `wasm-host` being default-on, the out-of-scope pi packages, and the deliberately
unreachable first-run wizard.

## Two structural blind spots, both found the hard way

Both were found because a user hit a live bug the analysis had looked straight at and blessed. They
are properties of the *method*, so they will keep producing misses until the method changes.

**1. An item-driven analysis cannot see behaviour nobody wrote an item for.** Every pass above starts
from a list and asks "is this item real?". A pi function with no corresponding item is invisible to
all three passes, including the adversarial one — the verifier refutes claims, and there is no claim
to refute. The fix is the **surface-driven sweep** (Update 2 in `00-residual-ledger.md`): walk pi
itself, and for each exported symbol / event / config key / CLI flag ask "what in cyrup consumes
this?". One such sweep added 58 items, 6 of them high. One sweep is unlikely to have exhausted the
class; **treat the open count as a floor, not a total.**

**2. The ADR-0001 substrate carve-out was applied far too broadly.** "cyrup delegates rendering to
ratatui + crossterm, so pi's hand-rolled `render(width): string[]` framework is out of scope" is
correct — for the *drawing* layer. It was silently extended to everything living in pi's
`packages/tui/src/tui.ts`, including behaviour that draws nothing: input sanitation, terminal-reply
handling, mode negotiation, paste and focus semantics. Those are portable and in scope. **Before
invoking ADR-0001 on a `tui.ts` line, check whether it actually draws anything.** The `07-cyrup-tui`
count of zero highs was partly an artifact of this — see the re-rated `TUI-004`.

A corollary worth stating separately, because it generalises past the TUI: **not enabling a feature
does not make its hazards moot.** `TUI-004` reasoned that mode 2031 is off, so unsolicited terminal
pushes cannot arrive — ignoring that cyrup *does* issue an OSC-11 query and therefore must handle
its reply, including a reply that arrives late. Ask what the code *sends*, not only what it *enables*.

## Caveats

- This is a **static** analysis. Nothing here was reproduced by running cyrup or pi. Items are
  evidenced by reading both sources, not by observing behavior.
- Severity and effort are judgements, not measurements. Treat the ordering in
  `00-residual-ledger.md` as a starting proposal.
- Deliberate divergences are documented in the workspace `CLAUDE.md` and were excluded by design.
  If an item looks like it contradicts one, the divergence list wins until someone decides otherwise.
- The upstreams keep moving. Re-run the version diffs before trusting the `upstream-drift` counts.
- Four items in the original analysis were **wrong about the mechanism**, not merely stale, and are
  corrected in place: `DRIFT-005` was already fixed before anyone worked it; `DRIFT-001`'s
  `addedToolNames` is a cache-*placement* record and does not change the active tool set;
  `TUI-002`'s claimed `thinkingText` palette never existed; `PROV-005` named xAI/Groq/DeepSeek as
  missing when they were always implemented. Expect a residue of similar errors — treat every item
  as a lead to verify, not a fact.
