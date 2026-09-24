# PARITY-GAPS

---

## Above medium — every open item, ranked (2026-09-24, second pass, cyrup `ea23ca2`)

**This is the current §0a.** Everything under the `⚠` notes below is older and is kept for its
per-entry fix sketches only. Every id and status here was re-checked against its own area file's
`## Open items` row. The count comes from `python3 scripts/count_open_items.py`; re-run it rather than
copying figures. Nothing here was observed at runtime.

| rank | ID | Sev | Area | Gap class (Kind) | Effort | Gap |
|---|---|---|---|---|---|---|
| 1 | ~~`SESS-056`~~ | ~~critical~~ **CLOSED 2026-09-24 (`c625fbc`)** | 03 | Version lag (`upstream-drift`) | S | If a session file's last line has no newline, the next entry is glued onto it and lost, and later entries lose their parent chain (pi v0.84.4 repairs the tail) |
| 2 | `SEAM-122` | **critical** | 08 | Version lag (`upstream-drift`) | S | Importing a session whose file name already exists overwrites the stored session (pi v0.85.0: unique name, `COPYFILE_EXCL`) |
| 3 | `ICOM-068` | **high** | 11 | Port bug (`parity-bug`) | M | An intercom message delivered without a turn is saved and drawn but never reaches the model's transcript. The code is area 08's |
| 4 | `ICOM-035` | **high** | 11 | Port bug (`parity-bug`) | M | **Reopened, a regression from `8de7460`:** a peer message to a busy session waits for idle instead of steering |
| 5 | `ICOM-062` | **high** | 11 | Version lag (`upstream-drift`) | M | A peer message during `/compact` starts a run whose transcript `compact` then replaces (needs `SEAM-125`) |
| 6 | `TUI-104` | **high** | 07 | Version lag (`upstream-drift`) | S | `/tree` during a compaction leaves the compaction's kept range on the abandoned branch, so history drops out of context. Session half `SEAM-124` is rated medium; settle the rating when the pair is fixed together |
| 7 | `TOOL-047` | **high** | 04 | Version lag (`upstream-drift`) | S | A shell command killed by a signal is reported as a success (pi v0.86.0: `128 + signo`, failure) |
| 8 | `SUBA-115` | **high** | 09b | Version lag (`upstream-drift`) | S | Stop/interrupt/timeout of a nested run also hits sibling subtrees it never launched (v0.68.0) |
| 9 | `SUBA-114` | **high** | 09b | Reverse lag (`stale-port`) | M | Child tools are cut to the parent session's start-up tools, so reviewer/scout launches are refused under a narrow `--tools` (removed upstream at v0.70.0) |
| 10 | `SUBA-110` | **high** | 09b | Version lag (`upstream-drift`) | S | `GIT_DIR`/`GIT_INDEX_FILE`/`GIT_CONFIG_*` reach the background runner and allowlist-less external CLIs (v0.71.0) |

**Not in this table by the standing rule**, which keeps areas 13 and 15 out of this file: area 13
has critical `MCP-500` and, among its highs, three filed on 2026-09-24 that a user would feel first:
`MCP-553` (`inheritEnv:false` ignored), `MCP-576` (a rotating `!command` bearer token is never
re-resolved) and `MCP-585` (an Agent Plugin header's `!command` is run through the shell).
`13-cyrup-mcp-STATUS.md` is the authority for the full list. Area 15 has nothing above medium. Area 16
(herdr) is counted here but is **not a port**: `crates/cyrup-herdr` is cyrup's own client of herdr's
socket API (<https://github.com/herdrdev/herdr>). Its highest row is `HERDR-001` (medium).

**Folded in from the 2026-09-24 second pass** (medium and below; bodies in the area files):
`PROV-083`…`100`, `DRIFT-058`/`059`, `AGENT-042`…`044`, `SESS-057`…`063`, `EXT-081`…`087`,
`TOOL-051`, `CFG-086`…`093`, `SEAM-123`…`131`, `SEAM-133`, `TUI-105`…`122`, `SUBA-116`…`143`,
`ICOM-069`/`070`, `HERDR-001`…`003`; trackers `EXT-088`, `SEAM-132`, `TUI-118`, `SUBA-142`, `HARN-001`,
`HARN-002`. **Duplicate:** `DRIFT-056` is `EXT-077` (area 06 is canonical); the script no longer
counts it twice. **Struck or narrowed:** `CFG-067`'s `LLM_INTENT_ARBITER` limb (upstream removed the
variable) and three of `CFG-074`'s nine variables (renames of real upstream variables). No counted
row closed outright. `ICOM-035` reopened.

---

## ⚠ 2026-09-16 — the SCOPE sequence (#137 / #139 / #140) landed; §0, §1b and §2 are reconciled against it, and the rest of this file is NOT

**Why this note exists.** Three PRs merged after the twelfth-edition census was written —
`21d1acc` (#137, `2bd76ac` "SCOPE batch 1"), `b2fdc7e` (#139, `e61ff44` "SCOPE batch 2") and
`cc7818b` (#140, `7e41cf9` "finish the SCOPE sequence") — and this file did not know. Measured with
`git diff --shortstat 0d653d2 cc7818b -- crates/`, the three feature commits are **+45 072 / −1 091
under `crates/`** (181 file-touches; 136 distinct files in `crates/cyrup-ext-subagents`), which is
the largest single movement this document has ever had to absorb. `cargo nextest` at the last of
them reports 10 124 tests run / 10 124 passed / 9 skipped.

**What this pass did, and what it did not.** §0 carries a thirteenth-edition census from
`scripts/count_open_items.py` run at `cc7818b`. §1b's `pi-subagents` entries and §2's unwired
register were **each re-greped against the code at `cc7818b`** and are marked closed, partially
closed or refreshed accordingly; `PB-11`/`SUBA-016` and the four SCOPE-closed entries in the
session-scoping block above are the substantive movements. **§1a, §1c, §1d, §1e, §3, §4, §5 and §6
were NOT re-walked** and keep whatever staleness they already carried.

**Two citation classes this pass found dead, recorded once here rather than repeated per row.**

1. **`crates/cyrup-ext-subagents/src/extension.rs` no longer exists.** It was split into
   `extension/{mod,wait_tool,models,testsupport}.rs` plus `extension/{executor,host,tool}/`. **Every
   `extension.rs:<line>` citation anywhere in this document is therefore dead**, not merely offset —
   `§1b`'s VL-S8 already said so for one line; it is true for all of them. The rows this pass
   touched carry refreshed citations; the rows it did not touch still carry dead ones, and that is
   a known, stated defect rather than a silent one.
2. **The "27-verb enum at `extension.rs:6557`" is wrong twice over.** The action list is now
   `extension/tool/text.rs:215` (`pub(crate) const SUBAGENT_ACTIONS`) and carries **42** verbs, of
   which nine are the `schedule.*` family `PB-11` says are absent. Re-derived this pass against
   `pi-subagents` v0.68.0 `shared/types.ts:2801`, which carries **57**: cyrup is missing seventeen
   (`children.list`, `worktree.discard`, `worktree.cleanup`, `lane.status`, `lane.recordMerge`,
   `lane.recordSupersession`, `refine`, `refine.show`, `refine.rollback`, `inspector.{open,command,status,close}`,
   `project.{open,status,close}`, `debug.run`) and carries two of its own (`append-step`, `inspect`).
   Those seventeen are the live verb-level gap; use this number, not 27.
   **Update 2026-09-19:** `children.list` landed (`background/retained_children.rs`, advertised at
   pi's own index; see the CLOSED row below), and `debug.run` landed the same day
   (`background/run_lifecycle_debug.rs`, its own CLOSED row below) — of that seventeen, only
   `inspector.*` and `project.*` remain: **seven**, every one needing a third-party terminal binary.

**Tag re-measured this pass, not inherited.** `git -C tmp/pi-subagents tag --sort=-v:refname | head -1`
returns **`v0.68.0`**, superseding the `v0.67.0` the 2026-09-14 pin correction below recorded. Every
upstream read in this pass's edits was taken with `git -C tmp/pi-subagents show v0.68.0:<path>`.

---

## ⚠ 2026-09-06 — session scoping landed; this document was generated against STALE upstream tags

**Version correction.** This analysis was generated against pi v0.84.1, pi-subagents v0.43.0 and
pi-intercom v0.9.2. Verified with `git describe --tags` on 2026-09-06, the real upstreams are
**pi v0.85.1**, **pi-subagents v0.65.1** (HEAD `7fe9dee1`, +50 commits) and **pi-intercom v0.10.1**.
The 147 items below are therefore a **floor**, not a total, and every "unported" claim needs
re-checking against the current tag before it is worked.

> **PIN CORRECTION — 2026-09-14. Two of the three tags in the paragraph above are themselves now
> stale, and this file was NOT re-read.** The 2026-09-06 sentence records what `git describe --tags`
> returned on that date and stays as written. Re-measured against `README.md`'s baselines table, the
> current tags are **pi `v0.85.1`** (unchanged), **pi-subagents `v0.67.0`** (*not* v0.65.1 — v0.66.0
> and v0.67.0 have since been cut) and **pi-intercom `v0.13.0`** (*not* v0.10.1 — re-checked
> 2026-09-14 and still the newest). The two upstreams this file counts separately are
> **`pi-mcp-adapter` v2.33.0** and **`pi-acp` v0.0.33**, and `code_puppy_core_plugins` is **v0.0.50**.
> **No item below is re-read, re-classified, re-ranked or re-counted by this note.** Its only effect
> is that "re-check against the current tag" now names a different tag for two upstreams — which is
> exactly the failure mode the `pi-intercom` v0.7.0 error demonstrated: an inherited pin that nobody
> re-measured parked real in-baseline defects in "version lag" for months.
>
> **Line-citation shift from this edit, stated so nobody re-verifies against the wrong line.** This
> note and the baselines-table corrections below it moved lines into this file at three points.
> A `PARITY-GAPS.md:<line>` citation written elsewhere needs **+20** if it named old line 13-96,
> **+24** for old 97-116, and **+25** for old 117 and after — so `:19` → `:39`, `:306` → `:331`,
> `:508` → `:533`, `:931` → `:956`, `:1360` → `:1385`, `:1537` → `:1562`. Old lines 1-12 are
> unaffected. Those citations live in `docs/adr/**`, `docs/PARITY-PLAN.md` and the area files, none
> of which this pass owns; they were **not** edited, and this is the repair instruction.

**Closed by the session-scoping change.** Async subagent results were being delivered to, and
**deleted by**, the wrong cyrup instance. `<results_dir>` is `<temp_root>/results/<cwd_key>`
(`background/artifact_roots.rs:281-284`) — keyed by cwd, never by session — and the watcher listed
it flat, so every concurrent instance in a directory consumed every other instance's results. Now
closed:

| area | what landed |
|---|---|
| result delivery | results partitioned on disk under `result-index/sessions/<enc(session)>/`; `read_dir(results_dir)` removed from the watcher; three-way `DeliveryDisposition` gate (`Unattributed` / `ObserveOnly` / `Deliver`); `delete_after_notify` clears every index |
| identity | `identity::{SessionId, CompletionOwnerId, IndexSegment, ResultFileName}` — `ResultFile`/`RunStatus`/`RunnerConfig` now carry both identities |
| control ops | session gates on `stop`, `interrupt`, `steer` and the async transcript view, with upstream's exact refusal strings and a zero-filesystem-trace guarantee on refusal |
| job tracker | `resume_tracking` no longer adopts other instances' runs — which also stopped it re-widening delivery, since tracked run ids are a candidate source |
| run-id resolver | prefix/exact resolution scoped before it acquires a caller (it is still unwired) |

**`SUBA-031` is re-scoped.** It was filed as "`wait` scopes by cwd not session". The same root cause
covered delivery, control, tracking and resolution; `wait` was simply the only surface that had
already been fixed. One implementation now serves all of them —
`background::delivery::SessionGate` for "may I act on this run?" and
`OwnershipSnapshot::owns` for "may I consume this completion?".

**Still open from that work** (upstream files with no cyrup counterpart, all session-scoped
upstream). **RE-READ 2026-09-16 against cyrup `cc7818b`; four of the seven entries below are now
CLOSED and are struck rather than deleted, per this directory's id-retention rule.**
~~`completion-replay.ts` (287 LOC, `SUBA-056`)~~ — **CLOSED**, ported as
`background/completion_replay/` (`2bd76ac`), ~~`wait-subscriptions.ts` (348)~~ — **CLOSED**, ported as
`background/wait_subscriptions/` (`e61ff44`),
~~`terminal-run-index.ts` (138)~~ — **CLOSED**, ported in full as `background/terminal_run_index/`
(4 files, 948 LOC; its own module doc names the upstream file and LOC), read in production by
`tui/fleet.rs:554` and written by `tui/fleet.rs:2339`; ~~`foreground-history.ts` (162)~~ —
**CLOSED**, ported as `extension/executor/foreground_history/` (mod/persist/record/restore) and
consumed at `extension/executor/mod.rs:214-220`;
`async-{stop,steering,dismiss}-action.ts` (418 — cyrup applies their gates from its own control
layer, but does not port the actions; **still true at `cc7818b`** — `background/delivery/gate.rs:3-4`
cites all three upstream files as the source of the predicate and nothing ports the actions
themselves), ~~`active-async-capacity.ts` (516 — per-session concurrency cap)~~ — **CLOSED**,
ported as `background/active_async_capacity/` (7 files, 3 603 LOC incl. 1 344 of tests) and
**acquired on the live async spawn path** at `extension/executor/background.rs:587`, with the
resume arm taking `transfer` at `:577` instead of a second slot; the doctor surface reads it at
`extension/executor/reports.rs:99-117`. `async-retention.ts` (912 — the async-root reaper; its RUN half is ported in full as
`background/async_retention/` — policy, batched cursor-windowed scan, tombstone markers,
wait-reference reader, the cross-instance lock, the destructive rename-then-delete sweep, the
report and its maintenance log — and runs as the third stage of `spawn_retention_sweep`, 60 s after
a session installs its completion watcher, with `/subagents-doctor`'s `Async retention` block as
its operator-visible surface. STILL OPEN: the results-side half (`:839-896` plus `resultSkipReason`
`:483-511`), which cyrup instead owns through `completion_replay::retention` +
`result_index::retention` rather than through a second sweep over the same directories).

---

> **SECOND REFRESH 2026-09-04, cyrup code HEAD `275c1f85`** (branch `claude/beautiful-feynman-odz1v5`,
> five code commits off `main` = `a4805955`). **Seven of the eight above-medium rows below closed the
> same day** — five `09a` rows on landed, both-sides-read code (`SUBA-085`, `SUBA-092`, `SUBA-082`,
> `SUBA-084`, `SUBA-086`) and the two live-use rows on live runs (`TUI-091` a duplicate of `TUI-090`,
> observed in a real pty; `SEAM-113` refuted as stale under ADR-0006, observed headless). §0's ninth
> edition and §0a's seventh edition are the current census: **125 open = 0/1/49/75, one row above
> medium (`SUBA-074` stage 2)**. The blockquote below it is the sixth edition's, kept as written.
>
> **CENSUS REFRESH 2026-09-04, cyrup HEAD `2571969`.** Every area file (01–12, excluding area 13
> — counted separately for the structural reason in `README.md`, NOT because another team owns it;
> that earlier claim was wrong — plus the `09a` v0.57-drift supplement and `14-cyrup-flux.md`) was
> independently re-audited this pass against cyrup HEAD `2571969` (baseline `4fb5e40`, 210 commits)
> and each upstream's real current tag, personally read on both sides per this ledger's evidence
> rule — not taken from any commit message. **This refresh regenerates §0's census and §0a's
> above-medium table from the fourteen files' current `## Open items` tables, mechanically, for the
> first time with a committed script** (`scripts/count_open_items.py`) — discharging the
> `README.md` "Work this directory owns" bullet asking for one. **§1–§6's per-item bodies below were
> NOT re-walked this pass** (§1 already carried this same caveat since the fourth edition, 2026-08-14
> — see its own blockquote) **except OQ-8, which this pass's closures answer directly.** Read the new
> blockquotes at the top of §0 and §0a before trusting any number in this file; the two counting
> pitfalls the script's own header comment documents — a struck severity that is a **re-rating**, not
> a closure (`~~medium~~ low — **PARTIALLY CLOSED**`, area 08's `SEAM-020`), and area 07's
> ID-unstruck/title-struck `**FIXED <date>**` convention — both cost a naive `grep -c CLOSED` a wrong
> answer and are exactly why a script is now committed instead of hand-counted.
>
> **Headline: the above-medium set is now EIGHT rows and, for the first time in this file's history,
> ZERO of them are critical.** `PERM-009`, `EXT-054`, `TUI-027`, `SEAM-112`, `PERM-034`, `TUI-092` —
> every critical this file has ever carried — are closed. The two reading-pass survivors are
> `TUI-091` and `SEAM-113`, joined by six rows new to this table because they come from `09a`, a file
> this document's census has never previously merged in (see §0's new block). Full detail below.

**Regenerated 2026-08-12 (repair pass) from the twelve repaired area files.** Supersedes the earlier
2026-08-12 edition, whose bookkeeping was exact and whose *content* carried four defects a
completeness critique found and this pass fixed:

1. **The severity scale was not being applied.** The previous edition's headline was "0 critical"
   while six open items met this project's own definition (`README.md` — data loss, silent wrong
   output, a permission bypass, or a crash on a normal path) **on their own text**. The definition
   was applied, not amended: `AGENT-020`, `TUI-027`, `EXT-054`, `PERM-009` were raised to critical
   — **`AGENT-020`'s raise was subsequently REFUTED by measurement on 2026-08-13 and the item is now `low`;**
   `TUI-027`, `EXT-054` and `PERM-009` were all confirmed in the shipped binary (`REPRO-LOG.md`) —
   and the repair pass filed two more (`TUI-042`, `TUI-043`). Four items were re-rated up to high
   (`TOOL-039`, `SEAM-051`, `PERM-023`, `DRIFT-049`) and one down-held rating (`TUI-019`) had its
   unverifiable-ADR justification struck.
2. **Nine items proposed no work.** They are now `tracker` rows — ID and body retained, excluded from
   the count, listed in §0.
3. **Citations were asserted tag-invariant without being read at the tag.** A sweep across the area
   files re-resolved every `@v0.83.0` / `@v0.84.1` / "identical at both tags" claim by opening the
   file at the tag. It found wrong offsets on a critical, on three highs, and on ~25 items overall.
4. **Three files contradicted each other about whether pi's catalog generator exists.** It does, at
   both tags — see §4 and OQ-5.

Two baselines were corrected in the previous edition and still hold: `pi-subagents` latest is
**v0.47.1** (not v0.43.0) and `pi-intercom` latest is **v0.10.1** (not v0.9.2); `pi-intercom`'s
**ported** baseline is **v0.9.2**, not v0.7.0 — see the §1d note. *(**Pin correction 2026-09-14**:
the two **ported** baselines in that sentence stand — `pi-subagents` ≈v0.43.0 and `pi-intercom`
v0.9.2 — but both **latest** figures have moved again. Latest is now `pi-subagents` **v0.67.0** and
`pi-intercom` **v0.13.0**. The sentence is left as written because it records a correction that was
true when it was made; only the currency claim in the word "latest" is superseded.)*

This document is the work-facing companion to `00-residual-ledger.md`. The ledger ranks; the area
files hold the evidence; this file is organised by **gap class**, so someone doing the work sees the
shape of the remaining distance rather than a per-crate walk.

**There is no "accepted divergence" category.** The project's goal is behavioural equivalence with
the four upstreams. Mechanism may differ where the language forces it — WASM Component Model guests
where pi runs TypeScript through `jiti`; ratatui where pi hand-rolls a renderer. Port the BEHAVIOUR
and state the mechanism difference with its reason. That is not an exemption: **where a mechanism
difference costs behaviour, the entry says so and stays on the list as work.**

| | |
|---|---|
| cyrup HEAD | **`824a539e`** — last CODE commit of the 2026-09-04→05 batch-3 (eleventh-edition) pass (`fix(tui,session-svc,ext,intercom): TUI-046 DRIFT-041 EXT-006 ICOM-054 review fixes`, branch `claude/parity-batch3`, 51 commits off `main` = `3e9633c4`, 23 of them touching `crates/`/`xtask`). The docs commit that wrote this row cannot cite its own sha; a status is measured against code. *Superseded: `6cf2cb9f` (batch 2), `275c1f85` (second pass).* **Re-measure before trusting any status.** **Pin correction 2026-09-14:** `824a539e` is itself superseded — `README.md`'s baselines table records the last CODE commit as **`9aeba769`** and the ledger's current cyrup pin as **`b28d3ff`** (`9aeba769..b28d3ff` is docs-only). `824a539e..9aeba769` is **32 code commits / 453 files / +98 509 / −15 880** under `crates/`+`xtask`, and no item in this file has been read against it. |
| `pi` | ported baseline **v0.83.0** → latest **v0.84.4** (HEAD `6aedd1066`) · delta v0.83.0..v0.84.4 = 775 files, +68 885 / −20 827. **Pin correction 2026-09-14: latest is now `v0.85.1`.** `v0.83.0..v0.85.1` = 1 087 files, +142 846 / −23 694; the newly opened `v0.84.4..v0.85.1` = 708 files, +96 348 / −25 254 is unmeasured by this file |
| `pi-subagents` | ported baseline **≈v0.43.0** (inferred — the crate records no version string) → latest **v0.64.0** (HEAD `a5f401e8`) · delta v0.43.0..v0.64.0 = 485 files, +92 664 / −18 069. **Pin correction 2026-09-14: latest is now `v0.67.0`.** `v0.43.0..v0.67.0` = 613 files, +123 871 / −31 254. Two further cautions recorded and not applied: the ≈v0.43.0 ported baseline is contradicted by the crate's own citation census (v0.64.0 × 330), and **`v0.57.0..v0.67.0` is owned by no area file** — area 09 is settled at v0.47.1 and 09a's `## Scope` stops at v0.57.0 |
| `pi-permission-system` | ported baseline **v0.7.1** → latest **v0.8.0** (HEAD `9affcc9`) · delta 28 files, +4 023 / −1 851 — **re-checked 2026-09-04 and again 2026-09-14, unchanged from every prior edition's figure; `v0.8.0` is still the newest tag** |
| `pi-intercom` | ported baseline **v0.9.2** *(prior docs said v0.7.0 — wrong, see §1d)* → latest **v0.13.0** (HEAD `199279a`, re-measured 2026-09-04, superseding the `v0.12.0` figure recorded 2026-08-27) · true window `v0.9.2..v0.13.0` = 26 files, +4 701 / −976 — **re-checked 2026-09-14, `v0.13.0` is still the newest tag and the figures re-measure identical.** Area 11 itself measures parity only to **v0.10.1**, so `v0.10.1..v0.13.0` is unopened there |
| `code_puppy_core_plugins` | ported baseline **v0.0.6** *(not recorded in-crate; see `FLUX-007`)* → latest **v0.0.40** (HEAD `8c6f852`) · 139 files, +11 071 / −3 822 across the whole repo, but the *ported* surface (`flux_bootstrap/`) is byte-identical `v0.0.6..v0.0.40` — area 14's own re-derivation this pass. **Pin correction 2026-09-14: latest is now `v0.0.50`**, 190 files / +16 472 / −4 288 across the whole repo, and the ported surface re-checked tag-by-tag is **byte-identical at all 39 intervening tags** — so the blind window is empty on the surface this directory owns |
| `pi-mcp-adapter` | **counted separately** — area 13 (this repository's own work; the earlier "owned by the MCP team" note was wrong). Clone re-pulled and **area 13 RE-AUDITED against `v2.32.1` on 2026-09-04** (`11b9994a`), superseding the prior "not re-measured here" note: `v2.26.1..v2.32.1` = 147 files, +16 014 / −1 001, **72 commits** (68 `--no-merges`). Its census stays outside §0 by the standing counting rule. **Pin correction 2026-09-14: latest is now `v2.33.0`**, and `v2.32.1..v2.33.0` = **123 files, +9 455 / −1 352, 33 non-merge commits** is measured by no pass — area 13's files carry it as an UNVERIFIED census with no `MCP-` ids assigned. Also recorded: the re-audit sha `11b9994a` cited in this row **does not resolve in this repository** |
| `pi-acp` | **added 2026-09-14 — this table had no row for it at all.** **counted separately** — area 15, excluded from §0 by the same standing rule that excludes area 13. Its file was written as a plan for code that did not exist; **`crates/cyrup-acp` now does exist**, so the "not ported" framing inherited from earlier editions is itself a stale pin. Ported baseline: none recorded. Latest tag **v0.0.33**, re-checked 2026-09-14 and still the newest; `v0.0.33..HEAD` touches no `src/` path, so there is no upstream drift window. Its stale axis is the **cyrup** side — `crates/cyrup-acp` exists now and three commits touching it post-date area 15's last correction |

Read upstream with `git -C <repo> show <tag>:<path>`, never from a working tree — clone-HEAD line
numbers and file existence both mislead. §7 says how much of this was first-hand.

---

## 0. Census — every open item in the fourteen area files, by class

> **FOURTEENTH EDITION 2026-09-24 (second pass), cyrup code `ea23ca2`.** The script changed: it now
> reads areas `16` (the herdr **client**; its `client-bug`/`protocol-drift` kinds fold into Port
> bug/Version lag, which the script documents) and `17` (pi harness/durable, trackers only), and it
> drops a row whose `Dedup` cell names an open canonical row (`DRIFT-056` → `EXT-077`). Its output,
> verbatim:
>
> ```text
> area   open  crit  high   med   low  trackers  closed  dups
> 01       33     0     0    17    16         0      57     0
> 02       10     0     0     0    10         1      28     0
> 03       16     1     0     4    11         1      32     0
> 04        6     0     1     2     3         0      34     0
> 05       24     0     0     6    18         0      56     0
> 06       21     0     0     5    16         1      63     0
> 07       45     0     1     2    42         1      86     0
> 08       17     1     0     7     9         0      72     0
> 09        2     0     0     0     2         0      51     0
> 09b      35     0     3    14    18         2       0     0
> 10        1     0     0     0     1         1      22     0
> 11       13     0     3     0    10         0      50     0
> 12        6     0     0     1     5         3      32     1
> 14        1     0     0     0     1         0       6     0
> 16        3     0     0     1     2         0       0     0
> 17        0     0     0     0     0         2       0     0
> 09a       1     0     0     0     1         0      31     0
> TOTAL   234     2     8    59   165        12     620     1
> (areas 13 and 15 are counted in their own files by the standing rule; area 16 is a
>  client of herdr, not a port -- its client-bug/protocol-drift kinds fold into Port bug /
>  Version lag as documented in KIND_TO_CLASS)
>
> Duplicates not counted (row -> canonical open row):
>   12 DRIFT-056 -> EXT-077
>
> Gap class (open, non-tracker rows only):
>   Port bug               60
>   Version lag           147
>   Reverse lag             6
>   Test defect             0
>   Invented surface       17
>   Tooling                 3
>   TOTAL                 233
>
> UNCLASSIFIED kind values (need a manual look / a KIND_TO_CLASS entry):
>   10 PERM-032: kind='*unclassified — lead*'
>
> Above-medium open rows (10):
>   03   SESS-056     critical  kind=upstream-drift
>   08   SEAM-122     critical  kind=upstream-drift
>   04   TOOL-047     high      kind=upstream-drift
>   07   TUI-104      high      kind=upstream-drift
>   09b  SUBA-110     high      kind=upstream-drift
>   09b  SUBA-114     high      kind=stale-port
>   09b  SUBA-115     high      kind=upstream-drift
>   11   ICOM-035     high      kind=parity-bug
>   11   ICOM-062     high      kind=upstream-drift
>   11   ICOM-068     high      kind=parity-bug
> ```
>
> The thirteenth edition below is superseded.

> **THIRTEENTH EDITION 2026-09-16, cyrup code HEAD `cc7818b` — the same script, unchanged, re-run
> after the SCOPE sequence (#137/#139/#140, +45 072 lines under `crates/`). This is the first
> edition of this census to publish a number it can PROVE is wrong, in a stated direction and by a
> stated amount.** `python3 scripts/count_open_items.py` from `docs/gap-analysis/`, over the fourteen
> files' current `## Open items` tables (and `09a`'s `## Summary — confirmed items` table),
> reproduced verbatim below; `SEAM-058` and `SUBA-005` remain the two hand-counted trackers outside
> any table. **No script change this edition.**
>
> **Open set: 77 work items — 0 critical, 0 high, 12 medium, 65 low**, of which 75 sort into the six
> Kind-derived classes and 2 do not (`EXT-058`, `PERM-032`). **599 closed.**
>
> | area | open | crit | high | med | low | trackers | closed |
> |---|---:|---:|---:|---:|---:|---:|---:|
> | [01 core + provider](01-cyrup-core-and-provider.md) | 4 | 0 | 0 | 1 | 3 | 0 | 57 |
> | [02 agent](02-cyrup-agent.md) | 3 | 0 | 0 | 0 | 3 | 1 | 28 |
> | [03 session](03-cyrup-session.md) | 3 | 0 | 0 | 0 | 3 | 1 | 32 |
> | [04 tools](04-cyrup-tools.md) | 0 | 0 | 0 | 0 | 0 | 0 | 34 |
> | [05 config + resources](05-cyrup-config-and-resources.md) | 12 | 0 | 0 | 4 | 8 | 0 | 55 |
> | [06 ext host](06-cyrup-ext.md) | 10 | 0 | 0 | 1 | 9 | 0 | 63 |
> | [07 tui](07-cyrup-tui.md) | 21 | 0 | 0 | 0 | 21 | 0 | 85 |
> | [08 session-svc + modes](08-cyrup-session-svc-and-modes.md) | 3 | 0 | 0 | 0 | 3 | 0† | 72 |
> | [09 subagents](09-cyrup-ext-subagents.md) | 10 | 0 | 0 | 3 | 7 | 0† | 41 |
> | [09a v0.57 drift](09a-cyrup-ext-subagents-v0.57-drift.md) | 1 | 0 | 0 | 1 | 0 | 0 | 23 |
> | [10 permission system](10-cyrup-permission-system.md) | 1 | 0 | 0 | 0 | 1 | 1 | 22 |
> | [11 intercom](11-cyrup-intercom.md) | 3 | 0 | 0 | 0 | 3 | 0 | 51 |
> | [12 pi core drift](12-upstream-drift-pi-core.md) | 5 | 0 | 0 | 2 | 3 | 3 | 30 |
> | [14 flux](14-cyrup-flux.md) | 1 | 0 | 0 | 0 | 1 | 0 | 6 |
> | **total** | **77** | **0** | **0** | **12** | **65** | **6 + 2‡** | **599** |
>
> † `SEAM-058` and `SUBA-005` sit outside their files' `## Open items` tables (standalone
> `## Trackers` sections) and print as 0 here; ‡ they are the "+ 2" hand-counted in the total.
>
> | class | n |
> |---|---:|
> | **Port bug** (`not-ported` + `parity-bug` + `port-divergence`) | **43** |
> | **Version lag** (`upstream-drift`) | **14** |
> | **Reverse lag** (`stale-port`) | **1** |
> | **Test defect** (`test-defect`) | **0** |
> | **Invented surface** (`cyrup-original`) | **15** |
> | **Tooling** (`tooling`) | **2** |
> | *(unclassified — `EXT-058`, `PERM-032`)* | 2 |
> | | **75 + 2 = 77** |
>
> **⚠ THIS TABLE IS A KNOWN OVERCOUNT BY TWO ROWS, AND BOTH ARE NAMED.** The script reports what the
> area tables SAY; two rows in area `09` say "open" about work that shipped and is tested:
>
> | row | table says | code at `cc7818b` says | landed |
> |---|---|---|---|
> | `SUBA-016` | open, `medium`, XL, BLOCKED, *"zero hits for `scheduled_runs`"* | `background/scheduled_runs/` = 8 files / 6 669 LOC; all **nine** verbs advertised (`extension/tool/text.rs:298-306`) and dispatched (`extension/tool/routing.rs:1243-1250`); 80/80 schedule tests pass | `7e41cf9` (#140) |
> | `SUBA-056` | open, `medium` | `background/completion_replay/` = 5 files / 1 810 LOC, third rung of `collect_wait_completions`, second consumer `background/inspect_rpc/read_output.rs` | `2bd76ac` (#137) |
>
> **The behavioural figure is therefore `75 open / 10 medium`; the mechanical figure is `77 / 12`.**
> Both are published because concealing either would misrepresent something. `09-cyrup-ext-subagents.md`
> is outside this pass's write scope, which is why the rows are not struck — see
> `00-residual-ledger.md`'s eleventh edition, whose first recommendation is to strike them.
> **Do not schedule `SUBA-016` or `SUBA-056`.**
>
> **A second reason not to over-read this number: the directory was being edited while it was
> measured.** The script was run twice by this pass at the same code sha, and moved: **76 open / 11
> medium at the start, 77 / 12 at the end**, because area `09a` went `0 → 1` open in between. Neither
> run is wrong. Quote the run, not the number.
>
> **What moved since the twelfth edition (84 → 77 open; 590 → 599 closed), derived by differencing
> the two per-area tables cell by cell.** `04` 1 open / 33 closed → **0 / 34** (**that file's open set
> is now empty**); `07` 28 / 78 → **21 / 85** (seven closed — the largest single-area movement);
> `09` 11 / 40 → **10 / 41**; `01` 3 / 57 → **4 / 57** (open rose with closed unchanged, so a row was
> **FILED** in area 01, not closed); `09a` 0 / 23 → **1 / 23** (likewise a filing — the row this
> pass watched appear mid-measurement). Every other area is unchanged. **The arithmetic: nine rows
> closed, two filed, 84 − 9 + 2 = 77.**
>
> **None of that movement is this pass's.** This edition is a reconciliation of the navigation layer
> — §0, §1b and §2 of this file, plus `00-residual-ledger.md` — and it wrote no code and touched no
> area file. The tenth edition's 90 → 84 remains the last delta produced by a code batch, and the
> nine closures above were written by passes whose own blocks this census does not carry.
>
> **Above-medium open rows: ZERO**, for the third consecutive edition. The script prints
> `Above-medium open rows (0)`. See §0a for what that does not mean.
>
> **`UW-21` is filed by this pass and is NOT in this table.** It is a §2 entry with no area-file id,
> because the area file that should own it is outside this pass's write scope. **The census counts
> area-file rows; a real defect with no row is invisible to it.** That is a property of the counting
> rule, not of the defect — see §2.
>
> **Not counted, deliberately:** area 13 (`13-cyrup-mcp*.md`) stays outside this census by the
> standing counting rule, unchanged by this pass. Its last counted census is
> **244 implemented / 82 partial / 84 missing / 27 n-a of 437** at `pi-mcp-adapter` v2.32.1 — a floor,
> not an answer; see `13-cyrup-mcp-STATUS.md`. Area 15 (`pi-acp`) is likewise outside it.
>
> Every block below this one is superseded.

> **TWELFTH EDITION 2026-09-05 (batch 4), cyrup code HEAD `f2630a7a` — the same script, unchanged,
> re-run after six closures, three narrowings, and — for the first time in this ledger's history —
> **ZERO new rows filed by the batch that ran**. `python3 scripts/count_open_items.py` from
> `docs/gap-analysis/`, over the fourteen files' current `## Open items` tables (and `09a`'s
> `## Summary — confirmed items` table), reproduced verbatim below; `SEAM-058` and `SUBA-005` remain
> the two hand-counted trackers outside any table. No script change this edition.
>
> **Open set: 84 work items — 0 critical, 0 high, 10 medium, 74 low**, of which 82 sort into the six
> Kind-derived classes and 2 do not (`EXT-058`, `PERM-032`). **590 closed.**
>
> | area | open | crit | high | med | low | trackers | closed |
> |---|---:|---:|---:|---:|---:|---:|---:|
> | [01 core + provider](01-cyrup-core-and-provider.md) | 3 | 0 | 0 | 0 | 3 | 0 | 57 |
> | [02 agent](02-cyrup-agent.md) | 3 | 0 | 0 | 0 | 3 | 1 | 28 |
> | [03 session](03-cyrup-session.md) | 3 | 0 | 0 | 0 | 3 | 1 | 32 |
> | [04 tools](04-cyrup-tools.md) | 1 | 0 | 0 | 0 | 1 | 0 | 33 |
> | [05 config + resources](05-cyrup-config-and-resources.md) | 12 | 0 | 0 | 4 | 8 | 0 | 55 |
> | [06 ext host](06-cyrup-ext.md) | 10 | 0 | 0 | 1 | 9 | 0 | 63 |
> | [07 tui](07-cyrup-tui.md) | 28 | 0 | 0 | 0 | 28 | 0 | 78 |
> | [08 session-svc + modes](08-cyrup-session-svc-and-modes.md) | 3 | 0 | 0 | 0 | 3 | 0† | 72 |
> | [09 subagents](09-cyrup-ext-subagents.md) | 11 | 0 | 0 | 3 | 8 | 0† | 40 |
> | [09a v0.57 drift](09a-cyrup-ext-subagents-v0.57-drift.md) | 0 | 0 | 0 | 0 | 0 | 0 | 23 |
> | [10 permission system](10-cyrup-permission-system.md) | 1 | 0 | 0 | 0 | 1 | 1 | 22 |
> | [11 intercom](11-cyrup-intercom.md) | 3 | 0 | 0 | 0 | 3 | 0 | 51 |
> | [12 pi core drift](12-upstream-drift-pi-core.md) | 5 | 0 | 0 | 2 | 3 | 3 | 30 |
> | [14 flux](14-cyrup-flux.md) | 1 | 0 | 0 | 0 | 1 | 0 | 6 |
> | **total** | **84** | **0** | **0** | **10** | **74** | **6 + 2‡** | **590** |
>
> † `SEAM-058` and `SUBA-005` sit outside their files' `## Open items` tables (standalone
> `## Trackers` sections) and print as 0 here; ‡ they are the "+ 2" hand-counted in the total.
>
> | class | n |
> |---|---:|
> | **Port bug** (`not-ported` + `parity-bug` + `port-divergence`) | **50** |
> | **Version lag** (`upstream-drift`) | **14** |
> | **Reverse lag** (`stale-port`) | **1** |
> | **Test defect** (`test-defect`) | **1** |
> | **Invented surface** (`cyrup-original`) | **15** |
> | **Tooling** (`tooling`) | **1** |
> | *(unclassified — `EXT-058`, `PERM-032`)* | 2 |
> | | **82 + 2 = 84** |
>
> **What moved, measured rather than asserted.** The script was run against `origin/main`'s copy of
> this directory (`git archive origin/main docs/gap-analysis`) as well as HEAD, so the delta below is
> a difference of two script runs, not a difference of two hand-written tables: **90 → 84 open,
> 584 → 590 closed. Six rows closed, none filed.** `01` 4 → 3 (`PROV-042`, the last `medium` in
> that area); `06` 11 → 10 (`EXT-041`); `07` 29 → 28 (`TUI-096`); `08` 4 → 3 (`SEAM-118`);
> `09a` 1 → 0 (`SUBA-095` — **that file's open set is now empty**); `12` 6 → 5 (`DRIFT-054`).
> `DRIFT-009`, `TUI-046` and `CFG-067` were worked and NARROWED but correctly stay open at their
> existing severities. Every other area is untouched.
>
> **Four of the six closures are batch 3's own self-filed rows.** `DRIFT-054`, `SUBA-095`, `TUI-096`
> and `SEAM-118` were opened by the batch-3 ledger pass against code batch 3 had just landed, and
> that batch merged with them open. All four are closed here with landing shas. Of the seven rows
> batch 3 filed, six described its own code; batch 4 filed **zero** rows of any kind. That is the one
> number this edition exists to publish, and `00-residual-ledger.md`'s tenth edition says what it does
> and does not license — in particular that batch 4 still shipped with four unfixed review findings
> against its own code, recorded on the rows that own the code (`SUBA-095`, `CFG-067`) rather than
> filed as new rows.
>
> **AUDIT CORRECTION carried by this edition.** The eleventh edition's per-area TABLE and its own
> prose disagreed: the table totalled **91 open / 583 closed** while the paragraph above it said
> **90 / 584**. The prose was right and the table was wrong, in one cell — area `06` was printed as
> `12 open / 61 closed / 3 medium` where the script said `11 / 62 / 2`. Nothing on this branch caused
> it; it was a hand-transcription slip in the eleventh edition itself, and it is the reason this
> edition quotes both script runs rather than differencing published tables. Anyone re-deriving the
> batch-3 delta should use 90/584 as its end state.
>
> **Above-medium open rows: ZERO**, for the second consecutive edition. The script prints
> `Above-medium open rows (0)`. See §0a for what that does not mean.
>
> **Not counted, deliberately:** area 13 (`13-cyrup-mcp*.md`) stays outside this census by the
> standing counting rule, unchanged by this batch. Its last counted census is
> **244 implemented / 82 partial / 84 missing / 27 n-a of 437** at `pi-mcp-adapter` v2.32.1 — a floor,
> not an answer; see `13-cyrup-mcp-STATUS.md`.
>
> Every block below this one is superseded.

> **ELEVENTH EDITION 2026-09-05 (batch 3), cyrup code HEAD `824a539e` — the same script, unchanged,
> re-run after nineteen closures, two partial closures, one refutation and the seven new rows this
> ledger pass filed against batch-3's own code.** `python3 scripts/count_open_items.py` from
> `docs/gap-analysis/`, over the fourteen files' current `## Open items` tables (and `09a`'s
> `## Summary — confirmed items` table), reproduced verbatim below; `SEAM-058` and `SUBA-005` remain
> the two hand-counted trackers outside any table. **No script change this edition** — both carried
> lists were already empty and the census is entirely table-derived.
>
> **What moved since the tenth edition (103 → 90 open; 564 → 584 closed):** `04` 2 → 1
> (`TOOL-022` REFUTED — all three limbs already reached a guest tool at HEAD, the consumer having
> landed in `75532cee` before this branch was cut); `05` 15 → 12 (`CFG-078`, `CFG-079`, `CFG-080`);
> `06` 14 → 12 (`EXT-003`, `EXT-006`, `EXT-039` closed; **`EXT-076` FILED medium** by this pass);
> `07` 30 → 29 (`TUI-004`, `TUI-N02`, `TUI-N11` closed; **`TUI-096` and `TUI-097` FILED low**);
> `08` 4 → 4 (`SEAM-015` closed; **`SEAM-118` FILED low**); `09a` 3 → 1 (`SUBA-074` — the last
> `high` — plus `SUBA-093` and `SUBA-094` closed; **`SUBA-095` FILED medium**); `11` 5 → 3
> (`ICOM-054`, `ICOM-055`); `12` 7 → 6 (`DRIFT-004`, `DRIFT-041`, `DRIFT-053` closed;
> **`DRIFT-054` FILED medium and `DRIFT-055` FILED low**); `01`, `02`, `03`, `09`, `10`, `14`
> untouched. `DRIFT-009` and `TUI-046` are PARTIAL and correctly stay open with their severities
> intact (`TUI-046` re-rated `medium` → `low`).
>
> **Read the arithmetic before quoting the headline.** 583 − 564 = nineteen rows closed, but open
> fell only 103 → 91, because six of the seven new rows are defects in the code this batch landed —
> raised by reviews that blocked and were not answered before the batch ended. A batch that closes
> nineteen and opens seven has closed twelve.
>
> **Open set: 90 work items — 0 critical, 0 high, 14 medium, 76 low**, of which 88 sort into the
> six Kind-derived classes and 2 do not (`EXT-058`, `PERM-032`).
>
> | area | open | crit | high | med | low | trackers | closed |
> |---|---:|---:|---:|---:|---:|---:|---:|
> | [01 core + provider](01-cyrup-core-and-provider.md) | 4 | 0 | 0 | 1 | 3 | 0 | 56 |
> | [02 agent](02-cyrup-agent.md) | 3 | 0 | 0 | 0 | 3 | 1 | 28 |
> | [03 session](03-cyrup-session.md) | 3 | 0 | 0 | 0 | 3 | 1 | 32 |
> | [04 tools](04-cyrup-tools.md) | 1 | 0 | 0 | 0 | 1 | 0 | 33 |
> | [05 config + resources](05-cyrup-config-and-resources.md) | 12 | 0 | 0 | 4 | 8 | 0 | 55 |
> | [06 ext host](06-cyrup-ext.md) | 12 | 0 | 0 | 3 | 9 | 0 | 61 |
> | [07 tui](07-cyrup-tui.md) | 29 | 0 | 0 | 0 | 29 | 0 | 77 |
> | [08 session-svc + modes](08-cyrup-session-svc-and-modes.md) | 4 | 0 | 0 | 0 | 4 | 0† | 71 |
> | [09 subagents](09-cyrup-ext-subagents.md) | 11 | 0 | 0 | 3 | 8 | 0† | 40 |
> | [09a v0.57 drift](09a-cyrup-ext-subagents-v0.57-drift.md) | 1 | 0 | 0 | 1 | 0 | 0 | 22 |
> | [10 permission system](10-cyrup-permission-system.md) | 1 | 0 | 0 | 0 | 1 | 1 | 22 |
> | [11 intercom](11-cyrup-intercom.md) | 3 | 0 | 0 | 0 | 3 | 0 | 51 |
> | [12 pi core drift](12-upstream-drift-pi-core.md) | 6 | 0 | 0 | 3 | 3 | 3 | 29 |
> | [14 flux](14-cyrup-flux.md) | 1 | 0 | 0 | 0 | 1 | 0 | 6 |
> | **total** | **91** | **0** | **0** | **15** | **76** | **6 + 2‡** | **583** |
>
> † `SEAM-058` and `SUBA-005` sit outside their files' `## Open items` tables (standalone
> `## Trackers` sections) and print as 0 here; ‡ they are the "+ 2" hand-counted in the total.
>
> | class | n |
> |---|---:|
> | **Port bug** (`not-ported` + `parity-bug` + `port-divergence`) | **56** |
> | **Version lag** (`upstream-drift`; the one open `09a` row counts here) | **15** |
> | **Reverse lag** (`stale-port`) | **1** |
> | **Test defect** (`test-defect`) | **1** |
> | **Invented surface** (`cyrup-original`) | **15** |
> | **Tooling** (`tooling`) | **1** |
> | *(unclassified — `EXT-058`, `PERM-032`)* | 2 |
> | | **91** |
>
> **Above-medium open rows: ZERO.** The script prints `Above-medium open rows (0)`. `SUBA-074` was
> the last one and closed at `af1a8a76` / `95a55ea1`. **No prior edition of this census has been able
> to print that line** — see §0a, which is now a statement rather than a table, and
> `00-residual-ledger.md`'s ninth edition for what it does and does not mean.
>
> **Not counted, deliberately:** area 13 (`13-cyrup-mcp*.md`) stays outside this census by the
> standing counting rule. **It HAS now been re-audited** against `tmp/pi-mcp-adapter` v2.32.1
> (`11b9994a`), which the tenth edition recorded as still outstanding; the counted census is
> **244 implemented / 82 partial / 84 missing / 27 n-a of 437**, a floor rather than an answer, with
> the extrapolation and its interval in `00-residual-ledger.md`'s ninth edition and in
> `13-cyrup-mcp-STATUS.md`.
>
> Every block below this one is superseded.
> **TENTH EDITION 2026-09-04 (batch 2), cyrup code HEAD `6cf2cb9f` — the same script, re-run after the
> twenty-five-row medium batch and its two independent reviews.** `python3 scripts/count_open_items.py`
> from `docs/gap-analysis/`, over the fourteen files' current `## Open items` tables (and `09a`'s
> `## Summary — confirmed items` table), reproduced verbatim below; `SEAM-058` and `SUBA-005` are the
> two hand-counted trackers outside any table, as before. **One script change in this commit, and it
> is a correction, not a re-rule:** the hand-enumerated `carried_medium = [SUBA-087..091]` list in
> `parse_09a` is now empty — all five became table rows this batch (closed or partially closed), so
> the interim run counted `09a` as 8 open / 7 medium when 3 / 2 is the truth. Both carried lists are
> now empty and the whole census is table-derived.
>
> **What moved since the ninth edition (125 → 103 open; 539 → 564 closed):** `01` 4 → 4 but 2 → 1
> medium (`PROV-014` PARTIALLY CLOSED at `1471a16f`, re-rated low); `03` 4 → 3 (`SESS-049`,
> `23abca0f`); `04` 3 → 2 (`TOOL-042` closed by measurement — 300 concurrent amplified runs, 0 LEAK);
> `05` 14 → 15 (`CFG-067` PARTIALLY CLOSED at `91ca02e5`, still medium; `CFG-080` filed low from its
> review); `06` 15 → 14 (`EXT-024` closed at `75532cee`; `EXT-041` PARTIALLY CLOSED at `a0134787`, still
> medium); `07` 35 → 30 (`TUI-037`, `TUI-068`, `TUI-081`, `TUI-089` REFUTED, and `TUI-025` whose last
> residual landed with `TUI-037`); `08` 7 → 4 (`SEAM-115`, `SEAM-116`, `SEAM-117`); `09` 12 → 11
> (`SUBA-072`, `7791b26a`); `09a` 6 → 3 (`SUBA-088`, `SUBA-089`, `SUBA-091` closed; `SUBA-087`,
> `SUBA-090` PARTIALLY CLOSED with their medium residuals FILED as `SUBA-093`, `SUBA-094`; only
> `SUBA-074` high + those two remain); `11` 7 → 5 (`ICOM-053`, `ICOM-060`); `14` 7 → 1 (`FLUX-001`…
> `FLUX-004`; `FLUX-005` partial, low); `02`, `10`, `12` untouched. Every closure was written by its
> implementer in the area file and re-read by this edition's ledger audit against HEAD and the named
> tags (`00-residual-ledger.md`, eighth edition, lists the findings).
>
> **Open set: 103 work items — 0 critical, 1 high, 28 medium, 74 low**, of which 101 sort into the
> six Kind-derived classes and 2 do not (`EXT-058`, `PERM-032`; `ICOM-053` closed).
>
> | area | open | crit | high | med | low | trackers | closed |
> |---|---:|---:|---:|---:|---:|---:|---:|
> | [01 core + provider](01-cyrup-core-and-provider.md) | 4 | 0 | 0 | 1 | 3 | 0 | 56 |
> | [02 agent](02-cyrup-agent.md) | 3 | 0 | 0 | 0 | 3 | 1 | 28 |
> | [03 session](03-cyrup-session.md) | 3 | 0 | 0 | 0 | 3 | 1 | 32 |
> | [04 tools](04-cyrup-tools.md) | 2 | 0 | 0 | 1 | 1 | 0 | 32 |
> | [05 config + resources](05-cyrup-config-and-resources.md) | 15 | 0 | 0 | 4 | 11 | 0 | 52 |
> | [06 ext host](06-cyrup-ext.md) | 14 | 0 | 0 | 5 | 9 | 0 | 58 |
> | [07 tui](07-cyrup-tui.md) | 30 | 0 | 0 | 4 | 26 | 0 | 74 |
> | [08 session-svc + modes](08-cyrup-session-svc-and-modes.md) | 4 | 0 | 0 | 1 | 3 | 0† | 70 |
> | [09 subagents](09-cyrup-ext-subagents.md) | 11 | 0 | 0 | 3 | 8 | 0† | 40 |
> | [09a v0.57 drift](09a-cyrup-ext-subagents-v0.57-drift.md) | 3 | 0 | 1 | 2 | 0 | 0 | 19 |
> | [10 permission system](10-cyrup-permission-system.md) | 1 | 0 | 0 | 0 | 1 | 1 | 22 |
> | [11 intercom](11-cyrup-intercom.md) | 5 | 0 | 0 | 2 | 3 | 0 | 49 |
> | [12 pi core drift](12-upstream-drift-pi-core.md) | 7 | 0 | 0 | 5 | 2 | 3 | 26 |
> | [14 flux](14-cyrup-flux.md) | 1 | 0 | 0 | 0 | 1 | 0 | 6 |
> | **total** | **103** | **0** | **1** | **28** | **74** | **6 + 2‡** | **564** |
>
> † `SEAM-058` and `SUBA-005` sit outside their files' `## Open items` tables (standalone
> `## Trackers` sections) and print as 0 here; ‡ they are the "+ 2" hand-counted in the total.
>
> | class | n |
> |---|---:|
> | **Port bug** (`not-ported` + `parity-bug` + `port-divergence`) | **61** |
> | **Version lag** (`upstream-drift`; all 3 open `09a` rows count here) | **21** |
> | **Reverse lag** (`stale-port`) | **1** |
> | **Test defect** (`test-defect`) | **2** |
> | **Invented surface** (`cyrup-original`) | **15** |
> | **Tooling** (`tooling`) | **1** |
> | *(unclassified — `EXT-058`, `PERM-032`)* | 2 |
> | | **103** |
>
> **Above-medium open rows (1):** `09a` **`SUBA-074`** high (`upstream-drift`) — stage 2, the
> external-runner adapter protocol. Unchanged; §0a's seventh-edition table stands, with the batch-2
> note added there.
>
> **Not counted, deliberately, and read the next-batch note before planning:** area 13 (`13-cyrup-mcp*.md`)
> stays outside this census by the standing counting rule, but it has NOT been re-audited against
> `tmp/pi-mcp-adapter` v2.32.1 and is scheduled first in `00-residual-ledger.md`'s eighth edition.
>
> Every block below this one is superseded.
> **NINTH EDITION 2026-09-04 (second pass), cyrup code HEAD `275c1f85` — the eighth edition's
> script, re-run after seven closures.** `python3 scripts/count_open_items.py` from
> `docs/gap-analysis/`, over the fourteen files' current `## Open items` tables (and `09a`'s
> `## Summary — confirmed items` table plus its now five-row carried list), reproduced verbatim
> below; the two hand-counted trackers outside any table are carried as before. **What moved since
> the eighth edition, and nothing else did:** `09a` 11 → 6 open (five closures: `SUBA-085`,
> `SUBA-092`, `SUBA-082`, `SUBA-084`, `SUBA-086` — the last three promoted out of `## Carried` and
> closed, and the script's hand-enumerated `carried_high` list emptied in the same commit so they
> count once); `07` 36 → 35 (`TUI-091`, duplicate of `TUI-090`, live pty); `08` 8 → 7 (`SEAM-113`,
> refuted as stale under ADR-0006, live headless). No new row was filed; six residual leads are
> recorded ownerless in `00-residual-ledger.md`'s seventh edition. The "closed this pass / new this
> pass" columns the eighth edition added by hand are dropped here — the script does not produce
> them, and the delta is fully stated in this paragraph.
>
> **Open set: 125 work items — 0 critical, 1 high, 49 medium, 75 low**, of which 122 sort into the
> six Kind-derived classes and 3 do not (`EXT-058`, `PERM-032`, `ICOM-053`, unchanged).
>
> | area | open | crit | high | med | low | trackers | closed |
> |---|---:|---:|---:|---:|---:|---:|---:|
> | [01 core + provider](01-cyrup-core-and-provider.md) | 4 | 0 | 0 | 2 | 2 | 0 | 56 |
> | [02 agent](02-cyrup-agent.md) | 3 | 0 | 0 | 0 | 3 | 1 | 28 |
> | [03 session](03-cyrup-session.md) | 4 | 0 | 0 | 1 | 3 | 1 | 31 |
> | [04 tools](04-cyrup-tools.md) | 3 | 0 | 0 | 2 | 1 | 0 | 31 |
> | [05 config + resources](05-cyrup-config-and-resources.md) | 14 | 0 | 0 | 4 | 10 | 0 | 52 |
> | [06 ext host](06-cyrup-ext.md) | 15 | 0 | 0 | 6 | 9 | 0 | 57 |
> | [07 tui](07-cyrup-tui.md) | 35 | 0 | 0 | 8 | 27 | 0 | 69 |
> | [08 session-svc + modes](08-cyrup-session-svc-and-modes.md) | 7 | 0 | 0 | 4 | 3 | 0† | 67 |
> | [09 subagents](09-cyrup-ext-subagents.md) | 12 | 0 | 0 | 4 | 8 | 0† | 39 |
> | [09a v0.57 drift](09a-cyrup-ext-subagents-v0.57-drift.md) | 6 | 0 | 1 | 5 | 0 | 0 | 14 |
> | [10 permission system](10-cyrup-permission-system.md) | 1 | 0 | 0 | 0 | 1 | 1 | 22 |
> | [11 intercom](11-cyrup-intercom.md) | 7 | 0 | 0 | 4 | 3 | 0 | 47 |
> | [12 pi core drift](12-upstream-drift-pi-core.md) | 7 | 0 | 0 | 5 | 2 | 3 | 26 |
> | [14 flux](14-cyrup-flux.md) | 7 | 0 | 0 | 4 | 3 | 0 | 0 |
> | **total** | **125** | **0** | **1** | **49** | **75** | **6 + 2‡** | **539** |
>
> † `SEAM-058` and `SUBA-005` sit outside their files' `## Open items` tables (standalone
> `## Trackers` sections) and print as 0 here; ‡ they are the "+ 2" hand-counted in the total, as
> the eighth edition explains.
>
> | class | n |
> |---|---:|
> | **Port bug** (`not-ported` + `parity-bug` + `port-divergence`) | **69** |
> | **Version lag** (`upstream-drift`; all 6 open `09a` rows count here) | **27** |
> | **Reverse lag** (`stale-port`) | **2** |
> | **Test defect** (`test-defect`) | **5** |
> | **Invented surface** (`cyrup-original`) | **18** |
> | **Tooling** (`tooling`) | **1** |
> | *(unclassified — `EXT-058`, `PERM-032`, `ICOM-053`, as before)* | 3 |
> | | **125** |
>
> **Above-medium open rows (1):** `09a` **`SUBA-074`** high (`upstream-drift`) — stage 2, the
> external-runner adapter protocol.
>
> Every block below this one is superseded.

> **EIGHTH EDITION 2026-09-04, cyrup HEAD `2571969` — RECOUNTED IN FULL, MECHANICALLY, FOR THE FIRST
> TIME.** Every block below this one is superseded. What changed and why it is more trustworthy than
> its predecessors:
>
> **The denominator changed from twelve files to fourteen, and this is the first count that says so
> plainly.** This document's title ("every open item in the twelve area files") and its 2026-08-12
> regeneration note both predate `14-cyrup-flux.md` (opened 2026-08-14, and even the fourth edition's
> own count of it was informal) and `09a-cyrup-ext-subagents-v0.57-drift.md` (a same-tier supplement
> to area 09, per `README.md`'s Contents table, that this file's census has **never** merged in before
> now). Both are walked below as their own rows, matching how the area files themselves are already
> organised — `09a` is not folded into area 09's own row, because its own file says its ids are not
> covered by area 09's table and both must be read.
>
> **The count is produced by a committed script, not by hand, for the first time**:
> `scripts/count_open_items.py`, run from `docs/gap-analysis/`. It parses each file's *current*
> `## Open items` table only — never the `## Status of every item from prior analyses` /
> `## Status table` /`## Status since …` table some files also carry, which every file's own
> convention (and `README.md`'s "Reading the area tables") says is not current — and classifies every
> row as OPEN, CLOSED, or excluded (`tracker` / the literal `*(partially-closed)*` marker) by the
> Severity cell's markup. **Getting this right took two false starts, both now documented in the
> script's own header comment so the next pass does not repeat them**: (1) a struck old severity
> followed by a bare NEW severity word is a **re-rating**, not a closure — `~~medium~~ low —
> **PARTIALLY CLOSED 2026-08-14**` (area 08's `SEAM-020`) is an OPEN row at `low`, not a closed one,
> and a naive "any `~~..~~` in the Severity cell means closed" rule miscounted it; (2) area 07 alone
> closes a subset of its rows with a bare `**FIXED <date>**` Severity cell and no strike-through at
> all, striking the Title cell instead — thirteen rows silently vanished from both the open and the
> closed count before this was found and handled. Two rows (`PROV-053` in area 01, `SEAM-074` in area
> 08) strike only one of ID/Severity where the repo's stated convention asks for both; the script
> treats a struck ID as authoritative over an unstruck severity cell for exactly this reason. Three
> Kind values used in the wild (`product-decision` — `EXT-058`; `*unclassified — lead*` — `PERM-032`;
> `test-gap` — `ICOM-053`) are not in `README.md`'s `Kind` enum at all and are reported as
> **Unclassified** rather than forced into one of the six buckets below.
>
> **Open set: 132 work items — 0 critical, 8 high, 49 medium, 75 low**, of which 129 sort cleanly
> into the six Kind-derived classes and 3 do not (`EXT-058`, `PERM-032`, `ICOM-053`, above). Plus
> **8 `tracker` rows** — `PROV-004` and `DRIFT-022` (this pass's two tracker closures) drop out of
> the prior 9-tracker figure, while `SEAM-058` (area 08) and `SUBA-005` (area 09) are **not inside
> any file's `## Open items` table at all** — both live in a standalone `## Trackers` section their
> own file excludes from the table by construction, so they are counted here by hand, the one place
> this census is not purely mechanical; `AGENT-028` (02), `SESS-038` (03), `PERM-017` (10),
> `DRIFT-023`/`DRIFT-032`/`DRIFT-040` (12) are the other six, all inside their tables and all found
> by the script.
>
> | area | open | crit | high | med | low | trackers | closed this pass\* | new this pass\* |
> |---|---:|---:|---:|---:|---:|---:|---:|---:|
> | [01 core + provider](01-cyrup-core-and-provider.md) | 4 | 0 | 0 | 2 | 2 | 0 | 2 | 0 |
> | [02 agent](02-cyrup-agent.md) | 3 | 0 | 0 | 0 | 3 | 1 | 1 | 2 |
> | [03 session](03-cyrup-session.md) | 4 | 0 | 0 | 1 | 3 | 1 | 2 | 2 |
> | [04 tools](04-cyrup-tools.md) | 3 | 0 | 0 | 2 | 1 | 0 | 1 | 0 |
> | [05 config + resources](05-cyrup-config-and-resources.md) | 14 | 0 | 0 | 4 | 10 | 0 | 4 | 2 |
> | [06 ext host](06-cyrup-ext.md) | 15 | 0 | 0 | 6 | 9 | 0 | 5 | 1 |
> | [07 tui](07-cyrup-tui.md) | 36 | 0 | 1 | 8 | 27 | 0 | 23 | 0 |
> | [08 session-svc + modes](08-cyrup-session-svc-and-modes.md) | 8 | 0 | 1 | 4 | 3 | 0† | 0 | 2 |
> | [09 subagents](09-cyrup-ext-subagents.md) | 12 | 0 | 0 | 4 | 8 | 0† | 0 | 0 |
> | [09a v0.57 drift](09a-cyrup-ext-subagents-v0.57-drift.md) | 11 | 0 | 6 | 5 | 0 | 0 | 9 | 1 |
> | [10 permission system](10-cyrup-permission-system.md) | 1 | 0 | 0 | 0 | 1 | 1 | 0 | 0 |
> | [11 intercom](11-cyrup-intercom.md) | 7 | 0 | 0 | 4 | 3 | 0 | 7 | 2 |
> | [12 pi core drift](12-upstream-drift-pi-core.md) | 7 | 0 | 0 | 5 | 2 | 3 | 1 | 1 |
> | [14 flux](14-cyrup-flux.md) | 7 | 0 | 0 | 4 | 3 | 0 | 0 | 0 |
> | **total** | **132** | **0** | **8** | **49** | **75** | **6 + 2‡** | **55** | **13** |
>
> \* Unlike every other column, "closed this pass" / "new this pass" are **not** script-derived — no
> prior-state snapshot exists to diff against — and are taken from each area's own 2026-09-04 pass
> summary, spot-checked against the actual struck rows for at least half the files rather than trusted
> whole. Treat them as approximate. † `SEAM-058` and `SUBA-005` are each area's one tracker but sit
> outside the `## Open items` table (see above), so they print as 0 in this script-derived column;
> both are counted in the `total` row's "6 + 2‡" cell. ‡ 6 trackers found inside `## Open items`
> tables by the script, plus the 2 (`SEAM-058`, `SUBA-005`) that live outside any table.
>
> **Gap class (open, non-tracker rows only) — the mechanical grouping this section's title promises:**
>
> | class | Kind values it covers | n |
> |---|---|---:|
> | **Port bug** | `not-ported` + `parity-bug` + `port-divergence` (+ the `port-bug` typo variant, and `DRIFT-015`/`DRIFT-019`'s in-row reclassification from `upstream-drift` to `not-ported`) | **71** |
> | **Version lag** | `upstream-drift` (all 11 open `09a` rows count here — see its own file's note on why: the whole file is drift against a later `pi-subagents` tag than the one cyrup ported) | **32** |
> | **Reverse lag** | `stale-port` | **2** |
> | **Test defect** | `test-defect` | **5** |
> | **Invented surface** | `cyrup-original` | **18** |
> | **Tooling** | `tooling` | **1** |
> | *(unclassified)* | `product-decision` (`EXT-058`), `*unclassified — lead*` (`PERM-032`), `test-gap` (`ICOM-053`) — no `Kind` enum value in `README.md`'s Item Format covers these three; reported rather than mis-bucketed | **3** |
> | | | **132** |
>
> **Deduplication, re-derived for one area only (area 12), the rest not re-walked this pass**: of
> area 12's 7 open rows, 4 carry `duplicate-of` (`DRIFT-009`→`PROV-018`, `DRIFT-015`→`EXT-019`,
> `DRIFT-019`→`PROV-014`, `DRIFT-047`→`VL-P5`) — this pass's own re-derivation, not carried over from
> the stale "16 of 30" figure two blocks below, which was measured at a much larger prior open set
> and was never recomputed against the current one. **The wider cross-area dedup census ("Work this
> directory owns", `README.md`) was not re-run this pass** — every other area's duplication is
> unmeasured here, so **132 is a floor, not a deduplicated total**, exactly as every prior edition
> has said.
>
> **What this refresh explicitly did NOT do, so it is not silently assumed done**: it did not re-walk
> §1 (port bugs), §2 (unwired), §3 (version lag), §4 (closure record), or §5 (deletion candidates)
> line by line — those sections were already marked incomplete-as-of-2026-08-14 by the fourth edition
> and remain so; **§6 (open questions) got exactly one update, q8**, because this pass's own closures
> (`CFG-021`, `DRIFT-022`, `TUI-019`) directly answer the question it asks — see the note at q8. It
> did not re-run the F4 duplicate-defect reduction, the unwired sweep, or any of the "surfaces left to
> walk" `README.md` lists. Scheduling from this file still means opening the named area file first.

> **CORRECTED 2026-08-19 against cyrup HEAD `4fb5e40`, and NOT recounted — read why.** The
> `0 critical / 5 high` headline below is false: all five of those rows closed on 2026-08-15, and the
> twelve area tables carried **three unstruck `critical` severity cells** when this correction was
> measured (`SEAM-112`, `PERM-034`, `TUI-092`) and three unstruck `high` ones (`PROV-068`, `TUI-091`,
> `SEAM-113`) — **2 + 4** after `TUI-092`'s de-escalation landed later in the same batch. **All three
> of those criticals are now struck: `SEAM-112` fixed 2026-08-29, `TUI-092` closed 2026-08-20, and
> `PERM-034` closed 2026-08-29 as REFUTED (a faithful port, not a gap).** See
> §0a, which tables them. **The 237/606 figures are stale by roughly a hundred rows** — five closing
> batches landed on 2026-08-15 without reconciling this file — and no replacement total is published
> here **because the twelve tables were being edited by other writers in the same batch that produced
> this correction**, which is the same reason `00-residual-ledger.md`'s fourth edition declines to
> restate area 05's count mid-batch ("*deliberately NOT restated by one slice mid-batch; recount the
> table*"). Recount from the tables, with the rule stated below, once the batch settles.
> **A second reason a recount must be run rather than adjusted:** `crates/cyrup-flux` — 9 files,
> 1 513 lines, shipped, with a spec and a rustbook chapter — had **no area file at all** until this
> batch opened [`14-cyrup-flux.md`](14-cyrup-flux.md) with 7 rows, so every figure in this file
> predates a whole shipped surface. The denominator moved, not just the numerator.
>
> **SUPERSEDED — FOURTH EDITION 2026-08-14 (after the surface enumeration). Every census below this
> block, including the third edition's, enumerates a set that no longer exists.**
>
> **Open set: 237 work items — 0 critical, 5 high, 88 medium, 144 low** (was 145 = 0/2/61/82), plus
> the same **10 `tracker` rows**. **606 rows across the twelve tables: 360 carry a full closure marker
> and 36 more a partial one — 396 of 606 (65%).** Derived row by row from the twelve `## Open items`
> tables **in the working tree** — the four surface writers' filings are not committed yet; the last
> code commit is `5990e86`. The counting rule is the third edition's, and it was validated by
> re-running it against the same twelve files at commit `e5c6933`, where it reproduces
> **503 / 349 / 35 / 145 = 0/2/61/82** exactly. **`13-cyrup-mcp.md`, `13a`–`13i` and
> `MCP-PORT-METHODOLOGY.md` are excluded from every figure in this file** — as they always have been,
> though it was never written down. *(Corrected 2026-09-04: the original sentence said "owned by another
> team"; no such team exists. The exclusion is a counting rule, not an ownership boundary.)*
>
> **The ninth pass was not a sweep. It enumerated nine finite pi SURFACES mechanically and diffed
> both directions: 191 findings — 67 missing in cyrup, 66 cyrup-original, 58 differing in shape —
> against the 10-25 a late-stage sweep produces. 93 ids were filed, 11 closed on arrival.** Five of
> the nine surfaces were walked completely; four state exactly what they could not reach. See
> `00-residual-ledger.md`'s fourth-edition block for the re-run recipe, the incomplete-surface list
> and the `cyrup-original` count. **No ID was renumbered, merged or deleted; `SEAM-087`…`SEAM-099` are
> deliberately unallocated — do not "recover" them.**
>
> **THE ABOVE-MEDIUM SET IS NOW FIVE ROWS, NOT TWO. §0a below is stale in a way that costs work.**
> Joining `PROV-047` and `SESS-040`: **`PROV-054`** (xai `grok-4.5` routed over `openai-completions`
> where pi uses `openai-responses` — and it is the xai *default* model), **`PROV-055`** (opencode's
> `sessionAffinityFormat: "openai-nosession"` missing on all 16 `openai-responses` rows, so cyrup
> leaks a `session_id` header pi suppresses) and **`PROV-056`** (kimi-coding's `forceAdaptiveThinking`
> ×3 and `allowEmptySignature` ×1 — two wire divergences per request on every model of the provider).
> **All three are catalog DATA and share ONE fix site with `PROV-057`…`PROV-059`:** they close through
> `PROV-018`/`PROV-060`'s bulk regeneration in the commit that rewrites `catalog_manifest.json`.
> **Do not schedule them individually** — that produces six agents each hand-patching one catalog row
> and each invalidating the manifest.
>
> **§6 q5 (OQ-5) IS REFUTED and is corrected in place below.** Catalog accuracy IS statically
> auditable; the "two-line re-export" premise holds only from `a9f6a3159` onward, and its direct
> parent `b0c2a90e` — cyrup's own stated provenance floor — still carries the full data literals.
> Filed as `PROV-060`.
>
> **§5's "21 `cyrup-original` items in the census" is likewise stale: the class is now 46 open rows
> (68 filed), and it has its own section in the ledger.** 66 of this pass's 191 findings are surfaces
> cyrup has and pi does not — the class through which divergence enters while everyone is looking at
> parity.
>
> *Superseded third-edition census follows.*

> **SUPERSEDED — THIRD EDITION 2026-08-14 (after sweeps 7-8). The census below enumerates a set that
> no longer exists, and so does the second-edition block beneath it.** **Eight** whole-backlog sweeps
> have landed. The current set, re-derived row by row from the twelve `## Open items` tables, is
> **145 open work items — 0 critical, 2 high, 61 medium, 82 low**, plus the same **10 `tracker`
> rows**. **503 rows across the twelve tables: 349 carry a full closure marker and 35 more a partial
> one — 384 of 503 (76%).** *(The second edition's "338 of 500" and its 63/88 medium/low split were
> derived by a different method; this edition states its counting rule in `00-residual-ledger.md` so
> the two can be compared.)*
>
> **THREE rows are new since the second edition and all three are closed: `PROV-M01` (area 01) and
> `TOOL-M01` (area 04), filed and closed in the same pass, plus `EXT-M03` (area 06), filed
> RETROACTIVELY because the ID was cited five times in `crates/cyrup-ext/src/host/live.rs` and had no
> row anywhere** — both produced by one assigned audit of
> hand-written delegating trait impls rather than by the backlog. **One row was REOPENED by
> measurement: `TOOL-042`** (area 04), whose closure rested on an argument that a 286-run experiment
> refuted for the one occurrence it instrumented. **The two highs are unchanged: `SESS-040` and
> `PROV-047`.** **No ID was renumbered, merged or deleted.**
>
> **THREE ENTRIES IN THIS FILE ARE NOW WRONG AND ARE CORRECTED HERE, because a work-facing document
> that mis-sizes an item costs a whole agent-pass:**
>
> - **`PB-10` (`turnBudget` = `SUBA-008`) IS CLOSED (sweep 8), and the second-edition note calling it
>   "the cheapest remaining medium … wiring plus a schema key, not a port" was measurably wrong.**
>   cyrup's `Usage` carried **no `turns` counter at all**, so there was no turn count to wire; the
>   port is ~888 lines of new module plus a drive-loop arm, a signal ladder, three new `SingleResult`
>   fields, a frontmatter field, a serializer arm and a config key. **And the mechanism it prescribed
>   was inverted** — the turn budget has no env handoff and no child-side enforcement, unlike the tool
>   budget it was told to mirror. See the ledger's mechanism register, entry 1.
> - **The `CFG-052` entry's premise about upstream is FALSE and the row is closed as REFUTED.** pi's
>   `parseGitUrl` returns `null` before reaching `hostedGitInfo.fromUrl` unless there is a `git:`
>   prefix or an explicit `://` (`utils/git.ts:172-179` @v0.83.0, and its own doc comment says so).
>   Upstream stores the shorthand as a local path exactly as cyrup does.
> - **`TOOL-042` is not a same-pass win.** It was filed, largely fixed, and reopened.
>
> *Second-edition block, retained for provenance:*

> **SUPERSEDED — SECOND EDITION 2026-08-14 (after sweeps 3-6). The census below enumerates a set that
> no longer exists.** **Six** whole-backlog sweeps have landed. The current set, re-derived from the
> twelve `## Open items` tables, is **153 open work items — 0 critical, 2 high, 63 medium, 88 low**,
> plus **10 `tracker` rows** (`PERM-017` re-classified 2026-08-14). **338 of 500 rows carry a closure
> marker.** Eight rows are new since the first edition, four of them filed AND closed in the same
> pass — `TOOL-042`, `EXT-M01`, `EXT-M02`, `PERM-033`; `TUI-062` was filed and partially closed, and
> `CFG-052`, `CFG-053` and `ICOM-053` were filed open. **The two remaining highs are `SESS-040` and `PROV-047`;
> `SEAM-061` closed as REFUTED (already landed at HEAD in both crates).**
>
> *First-edition figures, superseded: 173 open = 0 / 3 / 75 / 95 after sweeps 1-2, which closed 290
> rows; eight rows new (`PROV-053`, `AGENT-034`, `AGENT-035`, `SESS-045`…`SESS-048`, `EXT-060`).*
> The class *taxonomy* below is unchanged and still the right way to read the backlog; only the
> per-class counts are dead, and they have not been re-derived because the disposition is recorded
> per row in the area files rather than per class. See `00-residual-ledger.md`, top section.
>
> **Two class corrections landed that this file's §3 must absorb:** `DRIFT-013` and `DRIFT-029` were
> filed as **version lag** and are **port omissions inside the ported baseline** (`isZai` is at
> openai-completions.ts:1435 @v0.83.0; `_bashAbortControllers` is present in full at v0.83.0). With
> `DRIFT-014`/`018`/`019`/`030`/`031`/`032`, that is **eight** rows moved out of §3 by re-derivation.
> **Re-derive every remaining §3 entry at `v0.83.0` before scheduling it.**
>
> **`PB-13` is closed** with `SUBA-048`, as its own text instructed. **`PB-5`** is down to the
> subagent re-exec half only — and as of 2026-08-14 (sweep 6) **both** of its non-subagent halves are
> landed: the immediate-bash half in `cyrup-session-svc/src/bash.rs:107-109` **and the bash-TOOL half
> in `cyrup-tools/src/tools/bash.rs:154-165`**, pinned by `cyrup-tools/src/tests/bash_session_env.rs:200-221`.
> **`PB-5`'s remaining fix site is `crates/cyrup-ext-subagents/**` — route it there, not to area 04.**
> ~~**`PB-10`** (`turnBudget`) is `SUBA-008`, re-verified open at HEAD and rated the cheapest remaining
> medium in area 09: the three consumers already exist and read a hard-coded `false`, so it is wiring
> plus a schema key, not a port.~~ **CLOSED 2026-08-14 (sweep 8) with `SUBA-008`; the sizing and the
> mechanism in this sentence were both wrong — see the third-edition block above.**
> **`VL-P22`** is half-addressed: `DiskStore::rewrite`'s temp-sibling-and-rename now carries a
> `[CYRUP-DELTA]` naming pi's `_rewriteFile` (session-manager.ts:979-988) and the reason; the
> torn-tail half is untouched.
>
> **One sweep-1 doc instruction against this file is REFUTED:** it asked for line 19's `pi-intercom`
> ported baseline to be corrected from v0.7.0 to v0.9.2. The repair pass had already done it — `:26`
> and the §1d baseline table at `:44` both say **v0.9.2**. No change was needed.

**448 open work items: 6 critical, 22 high, 197 medium, 223 low.** Plus **9 `tracker` rows**, which
keep their IDs and bodies but propose no schedulable work and are deliberately outside the
arithmetic. Counted mechanically from the single `## Open items` table each area file now carries —
**all twelve have exactly one table as of this pass**; area 03's second table (`SESS-S05`) was the
last one and is gone.

Arithmetic from the previous edition: **426 + 31 filed by the repair pass − 9 reclassified as
trackers = 448.** Nothing was renumbered, merged or deleted to produce it.

| PARITY-GAPS class | area-file `Kind` values it covers | n |
|---|---|---|
| **Port bug** (§1) — upstream had it at the ported tag; cyrup does not | `not-ported` 146 + `parity-bug` 176 + `port-divergence` 1 | **323** |
| **Version lag** (§3) — landed upstream after the ported baseline | `upstream-drift` 66 | **66** |
| **Reverse lag** (§1e) — cyrup carries behaviour upstream changed or deleted | `stale-port` | **14** |
| **Test defect** — a test pinning wrong behaviour or an uncontrollable timing outcome | `test-defect` | **23** |
| **Invented surface** — behaviour with no upstream basis; delete it or justify it | `cyrup-original` | **21** |
| **Tooling** — audit/generation debt, not a user-visible gap | `tooling` | **1** |
| | | **448** |

The `tracking` kind no longer appears in the counted set: every row that carried it is now a tracker.

**§2 (unwired) is a lens, not a bucket.** Every unwired item also carries one of the kinds above; it
is called out separately because it is the project's most common defect shape and by far the cheapest
to fix. Do not add §2 to the census total.

| area | open | crit | high | med | low | trackers | closed this pass | new this pass |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| [01 core + provider](01-cyrup-core-and-provider.md) | 40 | 0 | 6 | 14 | 20 | 1 | 10 | 22 |
| [02 agent](02-cyrup-agent.md) | 26 | 1 | 1 | 6 | 18 | 1 | 3 | 14 |
| [03 session](03-cyrup-session.md) | 29 | 0 | 1 | 13 | 15 | 1 | 9 | 9 |
| [04 tools](04-cyrup-tools.md) | 29 | 0 | 1 | 10 | 18 | 0 | 14 | 11 |
| [05 config + resources](05-cyrup-config-and-resources.md) | 38 | 0 | 1 | 19 | 18 | 0 | 16 | 17 |
| [06 ext host](06-cyrup-ext.md) | 50 | 1 | 0 | 28 | 21 | 0 | 6 | 21 |
| [07 tui](07-cyrup-tui.md) | 56 | 3 | 1 | 26 | 26 | 0 | 13 | 25 |
| [08 session-svc + modes](08-cyrup-session-svc-and-modes.md) | 40 | 0 | 7 | 19 | 14 | 1 | 6 | 24 |
| [09 subagents](09-cyrup-ext-subagents.md) | 45 | 0 | 2 | 23 | 20 | 1 | 22 | 24 |
| [10 permission system](10-cyrup-permission-system.md) | 21 | 1 | 1 | 6 | 13 | 0 | 10 | 7 |
| [11 intercom](11-cyrup-intercom.md) | 44 | 0 | 0 | 22 | 22 | 0 | 3 | 24 |
| [12 pi core drift](12-upstream-drift-pi-core.md) | 30 | 0 | 1 | 11 | 18 | 4 | 5 | 9 |
| **total** | **448** | **6** | **22** | **197** | **223** | **9** | **117** | **207** |

**The nine trackers**, so nobody mistakes them for backlog: `PROV-004` (catalog field-diff coverage —
its whole Fix is `PROV-018`), `AGENT-028` + `SESS-038` (both turn on whether cyrup models pi's
v0.84.1 agent-harness — **answer them together**, OQ-7), `SEAM-058` (pi's experimental server/client
tree; escalate the moment `main()` references `experimentalCli`), `SUBA-005` (the management-verb
census — "this item is the ledger, not the work"), `DRIFT-022`, `DRIFT-023`, `DRIFT-032`,
`DRIFT-040`. Two of those — `DRIFT-023` and `DRIFT-040` — are additionally **leads, not items**:
neither side was ever re-read, and area 12 records the two commands that would settle each.

**Treat 448 as a floor.** Four area files say so explicitly with reasons: area 09 (`workflowScript`
is a whole execution model nobody has decomposed), area 11 (`broker/mod.rs` read in ranges only),
areas 05/10 (the surface sweep is the only counter to structural blind spot 1 and it is not
exhaustive), and area 08 (the inner RPC payload shapes are unswept).

**And 448 contains known duplication, so it is a floor with a soft ceiling.** The only deduplication
anyone has actually computed is area 12's: **16 of its 30 counted rows carry a `duplicate-of`** naming
the area that owns the same defect, so **432 is the largest defensible deduplicated figure today**.
The ledger's cluster F4 lists roughly twenty further defects carrying two-to-four IDs each
(`AGENT-019`/`DRIFT-039` are literally the same test); nobody has reduced that to a number, and until
someone does, no single figure here is both a floor and a total. Do the F4 reduction before any plan
books the same fix twice.

**Stable IDs are load-bearing.** `PB-N` / `UW-N` / `VL-*` ids below are never renumbered or deleted.
Where the re-audit moved an item to a different class, the id moves section and keeps its number.
`PB-32`…`PB-41` and `UW-19`/`UW-20` are new in this edition.

---

## 0a. Everything above medium, in one table

> **ELEVENTH EDITION 2026-09-24 (second pass), same code — ten rows: 2 critical, 8 high.** The
> ranked table is at the top of this file (*Above medium — every open item, ranked*), so it is read
> first. The tenth edition's three rows are all still open and are ranks 2, 7 and 10 there.
>
> Every block below this one is superseded.

> **TENTH EDITION 2026-09-24, cyrup code HEAD `ea23ca2`, upstreams re-pulled (pi v0.87.1,
> pi-subagents v0.71.0, pi-intercom v0.14.0, pi-mcp-adapter v2.37.0). The set is no longer empty.**
> `python3 docs/gap-analysis/scripts/count_open_items.py` prints `Above-medium open rows (3)`, all
> filed by this re-measure against new upstream tags, none against landed cyrup code:
>
> | ID | Sev | Area | Kind | Effort | Gap |
> |---|---|---|---|---|---|
> | `SEAM-122` | **critical** | 08 | upstream-drift | S | Importing a session whose file name already exists in the session dir overwrites the stored session (pi v0.85.0 renames the copy, `COPYFILE_EXCL`) |
> | `TOOL-047` | **high** | 04 | upstream-drift | S | A shell command killed by a signal is reported as a success (pi v0.86.0: `128 + signo`, failure) |
> | `SUBA-110` | **high** | 09b | upstream-drift | S | Git routing variables (`GIT_DIR`, `GIT_INDEX_FILE`, `GIT_CONFIG_*`, …) reach the background runner and allowlist-less external CLIs (pi-subagents v0.71.0) |
>
> Not in this table by the standing counting rule: `MCP-540` (high, area 13) — transport switch in a
> layered config keeps the old transport's fields. Area 15 closed five criticals and five highs this
> pass (`ACP-121`, `145`, `209`, `219`, `291`; `ACP-005`, `056`, `122`, `140`, `221`); none was in
> this table. The caveats of the ninth edition below still apply: severity is a per-row judgement,
> and whatever is open is a floor.
>
> Every block below this one is superseded.

> **NINTH EDITION 2026-09-05 (batch 4), cyrup code HEAD `f2630a7a` — THE SET IS STILL EMPTY, for the
> second consecutive edition. This section remains a statement, not a table.**
>
> `python3 scripts/count_open_items.py` prints `Above-medium open rows (0)` across all fourteen
> tables. Batch 4 neither closed nor filed an above-medium row — it closed six `medium`/`low` rows
> and filed none — so the set is empty for the same reason the eighth edition gave: `SUBA-074`,
> the last one, closed in batch 3.
>
> **What this does NOT license — unchanged from the eighth edition, with the numbers refreshed.**
>
> * **It is not a claim that nothing serious is open.** 84 rows are open, 10 of them `medium`, and
>   §0's twelfth edition has the table. Severity is a per-row judgement made when the row was
>   written; a `medium` that nobody has re-read is not evidence of a small problem.
> * **It is not a claim that the analysis has looked everywhere.** `README.md`'s *Where this analysis
>   is blind* is unchanged: an item-driven pass cannot see behaviour nobody filed an item for, so
>   **whatever is open is a floor, never a total.**
> * **It is not durable.** The fifth edition watched this set turn over completely in one pass and
>   acquire two criticals that had not existed a week before. The next surface-driven sweep should be
>   expected to refill it.
> * **It is not a statement about area 13**, counted separately, whose own re-audit at
>   `pi-mcp-adapter` v2.32.1 filed four criticals/highs against `crates/cyrup-mcp` — `MCP-500`
>   foremost. Excluded by the standing counting rule, not because they are less serious.
> * **NEW THIS EDITION — it is not a claim that this batch's own code is clean.** Batch 4 filed zero
>   rows, which is its target and a real improvement on batch 3's six-against-itself. It nonetheless
>   merged with **four unfixed findings against its own output**, two of them raised as blocking: a
>   false upstream citation at `crates/cyrup-ext-subagents/src/exec/external_cli/run.rs:333-336`, a
>   claim rollback that is not panic-safe at `.../exec/run_fanout_budget.rs:888-902` (with a test
>   pinning the divergent outcome as correct), a ledger citation this batch announced as corrected and
>   did not correct, and dead state in `run.rs`. Three of the four are recorded on `SUBA-095`'s and
>   `CFG-067`'s rows; the ledger one is fixed. **A batch with zero filed rows and four unfixed findings
>   is not the same thing as a batch with nothing wrong in it**, and reading this section as the
>   latter is the specific misreading the tenth-edition residual ledger was written to prevent.
>
> **Six of the ten open mediums predate this batch by more than a week**; none was filed by it. That
> is a statement about how recently anyone looked, not about how good the remainder is.
>
> Every block below this one is superseded.

> **EIGHTH EDITION 2026-09-05 (batch 3), cyrup code HEAD `824a539e` — THE SET IS EMPTY. This section
> is a statement, not a table, because there is nothing to tabulate.**
>
> `python3 scripts/count_open_items.py` prints `Above-medium open rows (0)` across all fourteen
> tables. **No prior edition of this section has been able to say that.** The seventh edition's one
> row, `SUBA-074`, closed in batch 3: stage 1 (the refusal) at `bf8b0f9`, stage 2 — the
> capability/status contract, the hardened external-CLI runner, the generic no-adapter path and the
> `claude-code`/`claude-code-writer` adapter — at `af1a8a76`, review-fixed at `95a55ea1`. The three
> deferred surfaces (`codex-exec`, `cursor-agent`, the whole `external-job` protocol) are refused BY
> NAME through an exhaustive `RunnerDispatch` with no `_` arm, so "did not refuse" can no longer be
> what selects a full-capability native child. `09a`'s row and section carry the landed/deferred
> split and the design decision with its rejected alternatives.
>
> **What this does NOT license, stated here because an empty section invites the wrong reading.**
>
> * **It is not a claim that nothing serious is open.** 91 rows are open, 15 of them `medium`, and
>   §0's eleventh edition has the table. Severity is a per-row judgement made when the row was
>   written; a `medium` that nobody has re-read is not evidence of a small problem.
> * **It is not a claim that the analysis has looked everywhere.** `README.md`'s *Where this analysis
>   is blind* is unchanged: an item-driven pass cannot see behaviour nobody filed an item for, so
>   **whatever is open is a floor, never a total.**
> * **It is not durable, and the record says so.** The fifth edition watched this set turn over
>   completely in a single pass and acquire two criticals that had not existed a week before; the
>   sixth celebrated "zero critical" and still carried eight rows. The next surface-driven sweep
>   should be expected to refill it.
> * **It is not a statement about area 13**, which is counted separately and whose own re-audit at
>   `pi-mcp-adapter` v2.32.1 filed four new criticals/highs against `crates/cyrup-mcp` — `MCP-500`
>   (`URL_BOUND_AUTH_FIELDS` is five fields upstream, four here) foremost. Those are not in this
>   section by the standing counting rule, not because they are less serious.
>
> **Six of the fifteen open mediums were filed in the two days this batch ran**, three of them by the
> ledger pass against batch-3's own landed code (`SUBA-095`, `DRIFT-054`, `EXT-076`). That is a
> statement about how recently anyone looked, not about how good the remainder is, and it is the
> first item on the recommended next batch in `00-residual-ledger.md`'s ninth edition.
>
> Every block below this one is superseded.

> **BATCH-2 NOTE 2026-09-04 (eighth-edition ledger audit), cyrup code HEAD `6cf2cb9f`: the set is
> UNCHANGED — still the one row below (`SUBA-074`, stage 2).** The twenty-five medium rows batch 2
> worked (listed in §0's tenth edition) contained nothing above medium and produced nothing above
> medium: the two residuals it filed as rows (`SUBA-093`, `SUBA-094`) and the one this audit filed
> (`CFG-080`) are medium, medium and low. The seventh-edition table below stands as the table of record.

> **SEVENTH EDITION 2026-09-04 (second pass), cyrup code HEAD `275c1f85`. ONE ROW.** Seven of the
> sixth edition's eight rows closed the same day, each dispositioned in its area file on the evidence
> `README.md` requires — not on a commit subject:
>
> | id | area | disposition |
> |---|---|---|
> | ~~`TUI-091`~~ | 07 | **CLOSED 2026-09-04 — duplicate of `TUI-090`.** Observed in a real pty (tmux 3.4, 120×40, HEAD `a4805955`): the reasoning block rendered live and committed above the answer in seven variants including the owner's exact `together`/Kimi-K3/`high` path. `TUI-091` was filed 16:26 on 2026-08-15; `TUI-090` was fixed 19:50 the same day with a body naming this asymmetry. `REPRO-LOG.md` §0e. |
> | ~~`SEAM-113`~~ | 08 | **CLOSED 2026-09-04 — REFUTED as an open bug, stale under ADR-0006.** Contract settled against v0.83.0; at the target v0.84.4 the contract is opt-in Ctrl+S persist and cyrup matches it path for path. The "rank 4 input permanently empty" claim is false (`crates/cyrup/src/bootstrap.rs:247-275`; read-back proven by seeding in a headless run). The `--default` flag never shipped in a tag (`5133c9284`). `set_thinking_level` dispositioned alongside. |
> | ~~`SUBA-085`~~ | 09a | **CLOSED 2026-09-04** at `5e3aa1c8` — `mission.resolve-decision` ported with upstream's status gate; the goal driver moves past a resolved decision (pinned). |
> | ~~`SUBA-092`~~ | 09a | **CLOSED 2026-09-04** at `247ff97b` — `excludeTools:`/`allowNestedSubagents:` ported end to end (frontmatter, override, serializer, spawn-plan subtraction, nested-fanout grant). |
> | ~~`SUBA-082`~~ | 09a | **PROMOTED from `## Carried` (upstream re-read at v0.57.0 and v0.64.0, confirmed as filed), then CLOSED 2026-09-04** at `5a4ae4ed`. |
> | ~~`SUBA-084`~~ | 09a | **PROMOTED, confirmed (effort L → M), then CLOSED 2026-09-04** at `dee8b9d0`. |
> | ~~`SUBA-086`~~ | 09a | **PROMOTED, confirmed with three corrections to the filed text, then CLOSED 2026-09-04** at `275c1f85`. |
>
> **The above-medium set is now:**
>
> | id | area | sev | one line |
> |---|---|---|---|
> | `SUBA-074` | 09a | high | Agent `runner:` frontmatter — stage 1 (the refusal path) closed 2026-09-04; **stage 2, the external-runner adapter protocol itself, is the open residual under this id.** Effort L, needs design. Unchanged this edition. |
>
> Six residual leads the closures produced (v0.63.0 `inferLevel` and custom-override drift, the
> v0.64.0 runtime-agent event bridge, five refused `RuntimeAgentDefinition` fields, two cosmetic TUI
> observations, the workspace's rustfmt state) are recorded ownerless in `00-residual-ledger.md`'s
> seventh edition and in `09a`'s second 2026-09-04 summary blockquote; none is a row yet. Every
> block below this one is superseded.

> **SIXTH EDITION 2026-09-04, cyrup HEAD `2571969`. EIGHT ROWS, ZERO CRITICAL — the first time this
> table has ever been empty of criticals.** Read this block before planning; every row below it,
> including the fifth edition's, is superseded.
>
> **Every fifth-edition row is dispositioned, all six re-verified this pass by re-reading the current
> code, not by trusting a prior "closed" mark**: `SEAM-112` (area 08) — **CLOSED 2026-08-29**, before
> this pass, re-confirmed unchanged. `PERM-034` (area 10) — **CLOSED 2026-08-29, REFUTED**, and this
> pass's area-10 re-audit specifically re-ran its own falsification condition and found nothing that
> would reopen it. `TUI-092` (area 07) — **CLOSED**, part of area 07's 23 closures this pass; the
> keybinding claims that had already downgraded it from critical to high (`keymap.rs:655`/`:656`
> wiring Ctrl+D/Ctrl+C) hold at HEAD. `PROV-068` (area 01) — **CLOSED 2026-09-04 this pass, REFUTED**:
> re-read at the ported tag `v0.83.0` rather than a later one, `mapped === null` really does mean
> unsupported on both sides; see area 01's `PROV-068` row for the full citation trail. `SEAM-113`
> (area 08) and `TUI-091` (area 07) are the two survivors — both re-confirmed still open this pass,
> with `SEAM-113`'s evidence substantially expanded (below).
>
> **The eight-row set below adds six rows from `09a`, which no prior edition of this table has ever
> drawn from.** `09a-cyrup-ext-subagents-v0.57-drift.md` predates this table's fifth edition, but
> nobody had folded its own severities into this cross-cutting file before — it is a same-tier
> supplement to area 09 per `README.md`'s Contents table, not a subsection of it, and its
> `## Summary — confirmed items` table carries six rows this pass re-confirmed at `high`: three
> closed-elsewhere-in-the-taxonomy-sense but still open here (`SUBA-074` stage-2 residual, `SUBA-085`,
> `SUBA-092`, all re-verified unchanged this pass) and three `## Carried — NOT adversarially verified`
> rows (`SUBA-082`, `SUBA-084`, `SUBA-086`) that this pass re-checked port-side only (every zero-hit
> grep the file recorded for them still returns zero at HEAD `2571969`) — held to the lower evidence
> bar `09a`'s own header states for that section, and flagged as such below rather than silently
> promoted to the same confidence as a fully re-read row.
>
> | id | area | sev | one line |
> |---|---|---|---|
> | `TUI-091` | 07 | high | Reasoning blocks never render although every layer — provider through the renderer — is wired and correct. Owner report, live use 2026-08-15 (`together`/`Kimi-K3`), re-confirmed absent this pass; **zero live hypotheses remain**, its last named candidate refuted in the area file. |
> | `SEAM-113` | 08 | high | A model chosen with `/model` does not survive into the next session. **Evidence substantially expanded this pass, not just re-confirmed**: commit `82f40d3` landed pi's later opt-in Ctrl+S persist mechanism, which is a *different* contract than the unconditional persist in `apply_model_change` this ledger settled on 2026-08-19 as the fix — re-read directly (`session/model.rs`, `session/thinking.rs`, `cyrup-tui/src/app/execute_misc.rs`), not inferred from the landing commit's message. The ordinary `/model` path still writes nothing to settings. |
> | `SUBA-074` | 09a | high | Agent `runner:` frontmatter is ignored entirely, so a sandboxed foreign-CLI profile runs as a full-capability native child. **Stage 1 (the refusal path) closed this pass**; stage 2 (the external-runner adapter protocol itself) is the open residual under this id. |
> | `SUBA-085` | 09a | high | `mission.resolve-decision` unported: a mission decision is write-once and permanently open, so the goal driver proposes the same next action forever. Re-verified open this pass (`resolve_decision`/`ResolveDecision` — 0 hits). |
> | `SUBA-092` | 09a | high | **New this pass.** Agent-level `excludeTools:`/`allowNestedSubagents:` (frontmatter and settings-override) are entirely unported — a declared per-agent tool exclusion has no effect, and a nested-subagent grant can only ever come from an explicit `tools:` allowlist. Both sides read at `pi-subagents` v0.62.0, inside the v0.57.0..v0.64.0 window past this file's original scope. |
> | `SUBA-082` | 09a | high | *(carried, not adversarially verified — port-side zero-hit grep re-confirmed this pass; upstream line numbers not re-read)* |
> | `SUBA-084` | 09a | high | *(carried, not adversarially verified — port-side zero-hit grep re-confirmed this pass; upstream line numbers not re-read)* |
> | `SUBA-086` | 09a | high | *(carried, not adversarially verified — port-side zero-hit grep re-confirmed this pass; upstream line numbers not re-read)* |
>
> **Six of the eight are effort M or smaller** per their area files; none is blocked on a decision the
> way several prior above-medium rows were (`PB-7`'s npm channel, `PB-19`'s Windows question). The
> live-use pair (`TUI-091`, `SEAM-113`) are the two this ledger's own "no `TUI-*`/live item is done
> until observed" rule holds to the highest evidence bar; the six `09a` rows are static-read findings
> at a named tag, per that file's own (lower, for the carried three) standard.

> **SUPERSEDED — FIFTH EDITION 2026-08-19, against cyrup HEAD `4fb5e40`. ALL FIVE ROWS OF THE
> FOURTH EDITION'S ABOVE-MEDIUM SET ARE CLOSED, AND THE SET THAT REPLACED THEM OPENED WITH THREE
> CRITICALS — a class the fourth edition published as empty.** Read this block before planning; the
> two below it name work that no longer exists.
>
> **Closed, each verified in its area table and, for the two the cross-cutting files kept alive, in
> the code:** `PROV-047` (`01-cyrup-core-and-provider.md`, CLOSED 2026-08-15 —
> `cyrup-session-svc/src/builder.rs:296-299`
> calls `cyrup_provider::configure_http_proxy(proxy.clone())` unconditionally, including with `None`,
> reached from `:1516`, and `crates/cyrup/src/main.rs:177` is the bootstrap call deliberately ABOVE
> the package/credential pre-dispatches that can egress before a session exists — so the "inert until
> one line lands" residual is DISCHARGED); `SESS-040` (`03-…`, REFUTED 2026-08-15 — see §2's UW-12
> entry for the dispatch chain); and `PROV-054`/`PROV-055`/`PROV-056`, all three CLOSED 2026-08-15
> in area 01, through exactly the one bulk catalog regeneration this block predicted.
>
> **The current above-medium set is SIX rows — 2 `critical` + 4 `high` — and it is entirely disjoint
> from the old one.** It opened 3 + 3 and became 2 + 4 when `TUI-092` was de-escalated inside this
> batch. Every one of the six was filed from LIVE USE on 2026-08-15 or later — the first cohort in
> this directory's history that no reading pass produced:
>
> | id | area | sev | one line |
> |---|---|---|---|
> | ~~`SEAM-112`~~ | 08 | ~~**crit**~~ **CLOSED 2026-08-29** | `/resume` produced a broken session: nothing rendered and bash tool calls repeated endlessly. Render half closed at `879eb4e`; the repetition was a port divergence — pi guards the overflow-latch clear with `stopReason !== "error" && stopReason !== "length"` and the port kept only the shared arm, so a `Length` message cleared `overflow_recovery_attempted` on `message_end` immediately before `check_compaction` read it, leaving the one-shot brake unreachable and compact-and-retry unbounded. See area 08. |
> | ~~`PERM-034`~~ | 10 | ~~**crit**~~ **CLOSED 2026-08-29 — REFUTED** | *(renumbered from `PERM-033` on 2026-08-19 — id collision; see area 10.)* "Allow Always" does not stick. **Not a gap — the port is faithful and no code changed.** Both sides clear `sessionApprovals` unconditionally from `session_start` AND `session_shutdown` (pi `index.ts:1828-1831`/`:1862-1865` @v0.8.0), so a reload wiping always-grants is upstream behaviour, now pinned by a test rather than filed as a bug. The subject round-trips byte-identically for simple, reported and compound commands; the per-instance-store suspect is structurally impossible (one `Arc` shared process-wide). Falsification condition recorded in area 10 — reopen only on a re-prompt inside one session with no reload and no session switch |
> | `TUI-092` | 07 | ~~crit~~ **high** | The TUI degrades from smooth to a total lockup. **Its severity cell was corrected `critical` → `high` inside this batch**; its own bug file's `**Severity**` header has said `high` since round 2 (`bugs/TUI-092-progressive-lockup.md`); the three clauses that justified `critical` are all false at HEAD — Ctrl+D is bound (`keymap.rs:655` → `app/input.rs:126-129` → `app/run_action.rs:16`), Ctrl+C is bound (`keymap.rs:656` → `app/input.rs:219-231`), and `TUI-088` is CLOSED |
> | `PROV-068` | 01 | high | An explicit `null` in `thinkingLevelMap` reads as UNSUPPORTED, collapsing most reasoning models to two rungs (`cyrup-provider/src/collection.rs:794-807`) |
> | `TUI-091` | 07 | high | Reasoning blocks never render although every layer is wired — and as of 2026-08-19 the row has **zero live hypotheses**; its last named candidate is refuted in the area file |
> | `SEAM-113` | 08 | high | A model chosen with `/model` does not survive into the next session |
>
> **The lesson is the one this directory keeps re-learning from the other side.** Nine reading passes
> and one nine-surface enumeration produced a five-row above-medium set of which every row was a
> *wire or wiring* defect a reader can see; four days of live use produced three rows rated
> `critical` on arrival, none of which any reading pass had a row for. `README.md`'s caveat — "no `TUI-*` item is done until it has been run in a real
> terminal" — generalises past the TUI: **the above-medium set is the part of this backlog a static
> method is worst at populating.**

> **SUPERSEDED — SECOND EDITION 2026-08-14 (after sweeps 3-6). The current above-medium set is TWO
> rows — `SESS-040` and `PROV-047` — tabled at the top of `00-residual-ledger.md`.** `SEAM-061`
> closed as REFUTED: sweep 6 found it already landed at HEAD in **both** crates
> (`cyrup-tui/src/session_selector.rs:154`/`:276`/`:313`/`:1918`/`:1985`; `crates/cyrup/src/main.rs:1354`
> + `startup_ui.rs:191-201`), which also retires the "one agent, both crates" coordination note that
> ranked it #1 for two editions. `PROV-030` (row 7 below) is likewise closed and was re-verified at
> HEAD by sweep 6 — `api/google_vertex.rs` is 717 lines with a real `ApiImpl::run`.
>
> *First edition, retained:* **SUPERSEDED 2026-08-14 — every row in this table is dispositioned.** All six criticals and 31 of
> the 34 highs are closed. Three of the highs (`PROV-027`, `PROV-028`, `PROV-029`) turned out to have
> been fixed before either sweep and were closed by **refutation**; four more (`SEAM-047`, `SEAM-051`,
> `SEAM-064`, `SEAM-072`) plus `DRIFT-049` had been marked fixed in their *kind* cell while their
> *severity* cell still read `high`, which is how this table published phantom highs across two
> recounts. **The current above-medium set is three rows — `SEAM-061`, `SESS-040`, `PROV-047` —
> tabled at the top of `00-residual-ledger.md`. Do not plan from the ranking below**; it is retained
> because each row is still the best one-line statement of what the work was.

A planner should not have to read six sections to find the twenty-eight items that outrank the rest.
Port bugs still rank above everything at equal severity (§1); the two `cyrup-original` highs are here
because a severity is a consequence, not a class.

> **⚠ RECONCILED 2026-09-22 at `14e6c56` — this table no longer names any open work, and it was
> naming plenty until this pass.** Every numbered row whose `entry` column points into §1, §2 or
> §3a has been struck here because that entry is struck in this file, each on its own re-greped
> evidence; **twenty-four of the twenty-eight rows were pointing at finished work.** Two survivors
> (`15` `TOOL-039`, `27` `PERM-023`) point at §5 rather than at an entry and were **not** re-greped
> by this pass — `python3 docs/gap-analysis/scripts/count_open_items.py` reports
> `Above-medium open rows (0)` across all fourteen area files and area 04 carries **0** open rows
> at all, so both are closed in their owning tables, but that is the script's word and not a read
> of the code, and it is recorded as such.
>
> **The table is kept, struck, as the edition's snapshot** — ids are retained in this directory and
> the ranking itself is history worth having. **Do not schedule from it.** The census and the area
> files are the authority on status, exactly as the warning under §1 already says.

| id | sev | area item | entry | effort | one line |
|---|---|---|---|---:|---|
| — | ~~crit~~ **low** | `AGENT-020` (02) | PB-25 | S | **⚠ REFUTED 2026-08-13 — no longer belongs in this table.** `continue_run` does drain before the run-active check, but the predicted loss does not occur on the normal path: typing during a live stream queued and delivered the message **5/5 times** (`REPRO-LOG.md`). Latent race only, reachable via `AGENT-030`. Severity critical → low; **this table's ranking is stale until someone re-ranks it.** |
| ~~2~~ | ~~**crit**~~ **CLOSED** | `TUI-042` (07) | PB-33 | S | The undo snapshot omits the paste registry — one undo turns a `[paste #N …]` marker into 20 literal characters sent to the model — **CLOSED: its `PB-33` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``TUI-042`` is absent from its area's open set.** |
| ~~3~~ | ~~**crit**~~ **CLOSED** | `TUI-043` (07) | PB-34 | S | Word motion / Ctrl+W are not paste-marker atomic — one Ctrl+W after a large paste orphans the marker and drops the paste — **CLOSED: its `PB-34` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``TUI-043`` is absent from its area's open set.** |
| ~~4~~ | ~~**crit**~~ **CLOSED** | `TUI-027` (07) | PB-28 | M | `/tree` has no text search; typing a filter word triggers `e`, and Enter **persists** the typed text as that entry's label in the session JSONL — **CLOSED: its `PB-28` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``TUI-027`` is absent from its area's open set.** |
| ~~5~~ | ~~**crit**~~ | `EXT-054` (06) | UW-13 | M | **FIXED 2026-08-13** — `capabilities` reaches instantiation via `load_wasm_with_caps`; enforced host-side at the import boundary; `EXT-055` (`ext-fs`) closed in the same change. Evidence in `06-cyrup-ext.md` |
| ~~6~~ | ~~**crit**~~ **CLOSED** | `PERM-009` (10) | PB-32 | S | `should_expose_tool`'s cyrup-only bash branch keeps `bash` advertised under a tool-level deny **and the allow-listed command runs** — **CLOSED: its `PB-32` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``PERM-009`` is absent from its area's open set.** |
| ~~7~~ | ~~high~~ **CLOSED** | `PROV-030` (01) | PB-22 | L | `google-vertex` registered with 10 catalog models and **no wire API** — every request dies at dispatch — **CLOSED: its `PB-22` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``PROV-030`` is absent from its area's open set.** |
| ~~8~~ | ~~high~~ **CLOSED** | `PROV-027` (01) | PB-23 | S | Copilot's 9 anthropic-messages models send `x-api-key`; pi sends `Authorization: Bearer` — all unauthenticated — **CLOSED: its `PB-23` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``PROV-027`` is absent from its area's open set.** |
| ~~9~~ | ~~high~~ **CLOSED** | `PROV-028` (01) | PB-24 | S | `github-copilot-headers.ts` unported on all three routes — Copilot image turns are rejected outright — **CLOSED: its `PB-24` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``PROV-028`` is absent from its area's open set.** |
| ~~10~~ | ~~high~~ **CLOSED** | `PROV-029` (01) | UW-11 | S | Copilot + Codex login flows ship complete and unreachable; `/login` dead-ends on `LoginUnsupported` — **CLOSED: its `UW-11` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``PROV-029`` is absent from its area's open set.** |
| ~~11~~ | ~~high~~ **CLOSED** | `PROV-047` (01) | PB-35 | M | `httpProxy` reaches only the streaming wire APIs — five OAuth flows, the agent proxy and extension HTTP all bypass it — **CLOSED: its `PB-35` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``PROV-047`` is absent from its area's open set.** |
| ~~12~~ | ~~high~~ **CLOSED** | `PROV-048` (01) | PB-36 | S | A lone-surrogate `\uXXXX` escape in an SSE frame kills the whole assistant turn (and blocks resuming a pi-written session) — **CLOSED: its `PB-36` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``PROV-048`` is absent from its area's open set.** |
| ~~13~~ | ~~high~~ **CLOSED** | `AGENT-030` (02) | PB-26 | M | `AgentSession::prompt` gates on the agent's per-run flag, so a prompt in the post-run gap starts a **second** run — **CLOSED: its `PB-26` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``AGENT-030`` is absent from its area's open set.** |
| ~~14~~ | ~~high~~ **CLOSED** | `SESS-040` (03) | UW-12 | M | Compaction cannot be cancelled from the shipped binary while the indicator advertises "(esc to cancel)" — **CLOSED: its `UW-12` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``SESS-040`` is absent from its area's open set.** |
| 15 | high | `TOOL-039` (04) | §5 | S | `CYRUP_SHELL` silently redirects every model-issued `bash` call to an arbitrary interpreter; pi has no shell env var |
| ~~16~~ | ~~high~~ **CLOSED** | `CFG-035` (05) | PB-27 | M | `.cyrup/SYSTEM.md` / `APPEND_SYSTEM.md` never discovered — the trust gate prompts about files cyrup never reads — **CLOSED: its `PB-27` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``CFG-035`` is absent from its area's open set.** |
| ~~17~~ | ~~high~~ **CLOSED** | `TUI-031` (07) | PB-29 | M | A prompt typed during compaction is dispatched into a context the compaction is mid-rewrite of — **CLOSED: its `PB-29` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``TUI-031`` is absent from its area's open set.** |
| ~~18~~ | ~~high~~ **CLOSED** | `SEAM-051` (08) | VL-P19 | S | `--tui-mode regular` — the flag's **default** value — makes the binary exit 1 claiming the option is unknown — **CLOSED: its `VL-P19` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``SEAM-051`` is absent from its area's open set.** |
| ~~19~~ | ~~high~~ **CLOSED** | `SEAM-047` (08) | PB-30 | M | First SIGTERM/SIGHUP neither tears down nor exits; `cyrup --mode rpc` cannot be stopped by a supervisor — **CLOSED: its `PB-30` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``SEAM-047`` is absent from its area's open set.** |
| ~~20~~ | ~~high~~ **CLOSED** | `SEAM-065` (08) | PB-41 | M | Trust is resolved pre-launch, inverting pi's tier order — the extension `project_trust` hook is skipped entirely — **CLOSED: its `PB-41` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``SEAM-065`` is absent from its area's open set.** |
| ~~21~~ | ~~high~~ **CLOSED** | `SEAM-064` (08) | PB-40 | S | The pre-launch trust prompt drops both "(this session only)" options — every answer is persisted, including a lockout — **CLOSED: its `PB-40` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``SEAM-064`` is absent from its area's open set.** |
| ~~22~~ | ~~high~~ **CLOSED** | `SEAM-063` (08) | PB-39 | M | Session delete permanently unlinks where pi routes through `trash`, and the failure is swallowed — **CLOSED: its `PB-39` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``SEAM-063`` is absent from its area's open set.** |
| ~~23~~ | ~~high~~ **CLOSED** | `SEAM-061` (08) | PB-37 | M | `--resume` lists every project's sessions under "Current Folder" with no cwd column and a dead `tab scope` hint — **CLOSED: its `PB-37` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``SEAM-061`` is absent from its area's open set.** |
| ~~24~~ | ~~high~~ **CLOSED** | `SEAM-062` (08) | PB-38 | S | Pre-launch rename is invited, accepted, echoed on screen — and discarded — **CLOSED: its `PB-38` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``SEAM-062`` is absent from its area's open set.** |
| ~~25~~ | ~~high~~ **CLOSED** | `SUBA-014` (09) | PB-31 | S | `requireReadTool` unported — a skill-carrying agent is told to `read` a skill it has no `read` tool for — **CLOSED: its `PB-31` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``SUBA-014`` is absent from its area's open set.** |
| ~~26~~ | ~~high~~ **CLOSED** | `SUBA-043` (09) | UW-14 | S | SINGLE-mode `outputSchema` unadvertised and hardcoded `None`, so the structured-output channel is unreachable — **CLOSED: its `UW-14` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``SUBA-043`` is absent from its area's open set.** |
| 27 | high | `PERM-023` (10) | §5 | S | The install probe never consults `agents_dir`, which the manager enforces — an agent-frontmatter deny is silently inert |
| ~~28~~ | ~~high~~ **CLOSED** | `DRIFT-049` (12) | PB-30 | M | **duplicate of `SEAM-047`** — schedule once, in area 08; area 12's body carries the RPC-mode analysis — **CLOSED: its `PB-30` entry is struck in this file (re-greped 2026-09-22 at `14e6c56`); ``DRIFT-049`` is absent from its area's open set.** |

Six of the top ten are effort **S**. Three pairs must ship together or the defect moves rather than
closes: `AGENT-020`+`AGENT-030`, `TUI-042`+`TUI-043`+`TUI-044`, `SEAM-047`+`SEAM-059`(+`SEAM-008`).

---

## 1. Port bugs — upstream had it at the tag cyrup ported; cyrup does not

> **⚠ INCOMPLETE AS OF 2026-08-14 (fourth edition), and stated rather than implied. The class
> sections §1–§4 were NOT regenerated for the 93 ids the surface enumeration filed.** They enumerate
> the pre-enumeration set. The 93 new ids carry no `PB-nn` / `UW-nn` / `VL-nn` entry number and are
> **not** listed below — they are in their area files, with the counts and the cross-cutting reading
> in `00-residual-ledger.md`'s fourth-edition block. Regenerating these sections is a pass of its own
> (67 of the 191 findings are `missingInCyrup`, i.e. §1 material; 58 are `differingShape`, which this
> taxonomy has no home for at all — **that is itself a finding about the taxonomy**). **Do not read
> the absence of an id from §1 as evidence that no port bug was filed for it.**
>
> **AND DO NOT READ AN ENTRY'S PRESENCE AS EVIDENCE THAT IT IS OPEN — added 2026-08-19.** §1–§4 are
> a class-organised RENDERING of the area files at the edition that produced them; the owning area
> file's `## Open items` table is the only authority on status. Repairing this file's citations at
> `4fb5e40` walked past three specimens in a single pass — **PB-39** (`SEAM-063`, session delete via
> `trash`) closed 2026-08-14 and the seam now reports pi's own three strings from
> `app/execute_session.rs:24-32`; **PB-29** (`TUI-031`) closed 2026-08-14 with the compaction guard at
> `app/run_action.rs:68-82`; **UW-12** (`SESS-040`) closed 2026-08-15. Each entry now carries a dated
> `CLOSED` bullet, but they were found incidentally and **the rest of §1–§4 was not audited for
> status**. Check the area file before scheduling any entry here.
>
> **STILL TRUE 2026-09-04 — the census refresh (§0/§0a) did not extend to §1–§6's per-item bodies.**
> This pass re-audited all fourteen area files and regenerated §0's census and §0a's above-medium
> table mechanically from their current `## Open items` tables (see both sections' new top blocks),
> but did not walk §1–§5 entry by entry the way the 2026-08-12 regeneration did — that remains a pass
> of its own. The one exception is §6 q8, which this pass's own closures (`CFG-021`, `DRIFT-022`,
> `TUI-019`) answer directly and which now carries an **ANSWERED** note. **The rule above still
> holds and is now doubly true**: an id's presence or absence in §1 says nothing about whether it is
> open — the owning area file's `## Open items` table is the only authority, and §0's new block names
> exactly which ids are open today.

> **PARTIAL EXCEPTION 2026-09-16 — §1b, and only §1b, IS status-audited.** All 23 of its entries
> (`PB-8`…`PB-14`, `PB-31`, `VL-S1`…`VL-S15`) were re-greped against cyrup `cc7818b` and each now
> carries its current status: ten struck as CLOSED, one marked PARTIALLY CLOSED with the halves
> named, twelve refreshed and still open. ~~**`§1a`, `§1c`, `§1d` and `§1e` were NOT touched and the
> warning above applies to them in full.**~~ Note also that this file's `extension.rs:<line>`
> citations are dead rather than stale — see the ⚠ block at the top of this file.
>
> **EXCEPTION WIDENED 2026-09-22 at `14e6c56` — §1a, §1b, §1c, §1d, §2, §3a and §3b are now ALL
> status-audited**, each row re-greped rather than carried. `§1e` is the only class section that
> still is not, and the warning above applies to it in full. **The open set of the audited sections,
> in one place, so nobody re-derives it:** §1a — `PB-6`, `PB-7`; §1b — none (`PB-9` and `PB-14` closed 2026-09-23); §1c — none;
> §1d — none; §2 — `UW-10` and `UW-18` partially (`UW-7` **closed 2026-09-22**, `UW-3` **closed 2026-09-23**);
> §3a — `VL-P4`, `VL-P5`, `VL-P6`, `VL-P12`, `VL-P15`, `VL-P16`, `VL-P17`, `VL-P22`, `VL-P23`,
> `VL-P24`; §3b — `SUBA-024`'s agent-contract half. **Everything else in those sections is
> struck, with the grep that struck it, and keeps its body as history.**
>
> **Three citations in this file are version-pinned and are NOT stale — do not renumber them.**
> `UW-3` and `UW-7`'s upstream coordinates are correct at **v0.43.0**, the tag those cyrup files are
> pinned to, and `VL-P17`'s are correct at **v0.84.1**, the upper bound of §3a's window (the same
> function is strictly sequential at v0.83.0, so checking it at the wrong tag makes a true row look
> false). Where a later tag's coordinates are useful they are **added beside** the pinned ones. A
> later pass ADDS; it does not substitute.

**These rank above everything else in this document at equal severity.** They are not version lag:
the behaviour was available to be ported and was not.

### 1a. From `pi` v0.83.0

**~~PB-1 · `radius` provider is not registered~~** — ~~*medium*~~ **CLOSED 2026-09-22** · area 01 `PROV-014` (re-confirmed at HEAD)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `crates/cyrup-provider/src/providers/all.rs:260` pushes `radius_provider_with(…)` (the comment at `:257` cites `all.ts:121` @v0.84.4 / `:117` @v0.83.0) and `env_api_keys.rs:71` maps `"radius" => RADIUS_API_KEY`. Upstream re-verified at the pinned tag: `git -C tmp/pi show v0.83.0:packages/ai/src/providers/all.ts` line 117 is `radiusProvider(),`. **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/ai/src/providers/all.ts:117` @v0.83.0 (`radiusProvider()`; :121 @v0.84.1); definition `providers/radius.ts:20`; `env-api-keys.ts` @v0.83.0 already maps `RADIUS_API_KEY`
- cyrup: `crates/cyrup-provider/src/providers/all.rs:140-240` — no radius push; `env_api_keys.rs:34-73` has no such arm; `providers/builtin_oauth.rs:17` documents the hole in-tree
- observable: `--provider radius` resolves to no provider. The Radius OAuth flow is ported (`auth/oauth/radius.rs`, id registered at `auth/oauth/load.rs:59`) and the wire API it streams over exists (`api/pi_messages.rs`), so a working credential can never be attached to a streamable provider.

**~~PB-2 · `qwen-token-plan` and `qwen-token-plan-cn` are not registered, but the resolver advertises them~~** — ~~*medium*~~ **CLOSED 2026-09-22** · area 01 `PROV-014`, area 12 `DRIFT-019` (**kind corrected this pass**: `DRIFT-019` was `upstream-drift`; `git cat-file -e v0.83.0:` proves the upstream files predate the ported tag, so it is a port bug on both sides of the pair)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** Both are registered as fleet providers with dynamic catalogs — `providers/all.rs:39-41` with the `openai-completions` fleet at `:250-251` — and `env_api_keys.rs:53-59` maps all three keys, including the v0.84.x `qwen-token-plan-individual`. The resolver no longer advertises what the registry lacks. **The body below is the original filing and is kept as history.**
- upstream: `providers/all.ts:115-116` @v0.83.0; definition `providers/qwen-token-plan.ts:6-15`; `env-api-keys.ts` @v0.83.0 maps `QWEN_TOKEN_PLAN_API_KEY` / `QWEN_TOKEN_PLAN_CN_API_KEY`
- cyrup: `providers/all.rs:140-240` (no push, no catalog) versus `crates/cyrup-config/src/model.rs:1022-1023` (both in `KNOWN_PROVIDERS`) and `model.rs:973-974` (both given the default model `qwen3.7-max`)
- observable: cyrup accepts `--provider qwen-token-plan` at argument validation and resolves a default model for it, then fails at stream time with no such provider.

**~~PB-3 · `Models::refresh` accepts no options and returns no per-provider result~~** — ~~*low*~~ **CLOSED 2026-09-22** **(severity corrected down)** · area 01 `PROV-S05`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `crates/cyrup-provider/src/collection.rs:431` is `refresh_with(&self, provider, ModelsRefreshOptions) -> ModelsRefreshResult`, with `allow_network`/`force`/`cancel` in and `aborted`/`errors` out, and a clause-by-clause table against `models.ts:276-328` @v0.83.0 in its own doc. `refresh` (`:509`) survives as the compatibility shape and says so. **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/ai/src/models.ts:46-56` @v0.83.0 (`ModelsRefreshOptions{allowNetwork,force,signal}` + `ModelsRefreshResult{aborted,errors}`), refresh at `:276`; v0.84.1 adds `providers?: readonly string[]` (`models.ts:67`) and generation-checked publication (`:320-361`)
- cyrup: `crates/cyrup-provider/src/collection.rs:317-337` — `refresh(&self, provider: Option<&str>)`, `join_all` at `:335` with every result discarded, unconditional `Ok(())`
- **corrected by the re-audit**: most of what pi's options buy is already reproduced by a different mechanism this entry missed. `crates/cyrup/src/provider.rs:71-130` splits pi's `refresh({allowNetwork:false})` restore from the network refresh, gates the network path on mode (mirroring pi's rpc/interactive-only triggers) and restricts the fetch to configured providers exactly as pi's `resolveRefreshCredential` bail does. **What genuinely remains is the `errors`/`aborted` result shape, `force`, and the abort signal** — API-shape and error-reporting residue. The proposed raise to medium was rejected in area 01; it is *low*.
- observable: no force past a freshness check, no cancellation, and no report of which providers failed. Two concurrent refreshes of one provider both publish, last-writer-wins.

**~~PB-4 · Compact-read classification has no `docs` arm~~** — ~~*low*~~ **CLOSED 2026-09-22** · area 04 `TOOL-017`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `crates/cyrup-tui/src/transcript/tool_args.rs:521` (`compact_read_classification`) runs all three arms in upstream's order, with `docs_classification` (`:468`, `CompactReadKind::Docs` at `:446`/`:480`) between `SKILL.md` and `COMPACT_RESOURCE_FILE_NAMES`. The `getReadmePath`-has-no-counterpart blocker is discharged and so is OQ-2. **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/coding-agent/src/core/tools/read.ts:98` @v0.83.0 (`getPiDocsClassification`, resolving against `dirname(getReadmePath())`), called from `:130` — **present at the ported tag**, so this is not lag
- cyrup: `crates/cyrup-tui/src/transcript.rs:2468` (`compact_read_classification` — `skill` and `resource` arms complete, no `docs` arm; the doc at `:2461-2467` states why, and names the missing seam: `getReadmePath` has no counterpart in `crates/`) *(citation re-resolved by symbol 2026-08-19; `:2265` is now a closing brace)*
- observable: reading cyrup's own shipped README/docs/examples renders as an ordinary file read. **Blocked on a decision, not on code** — see OQ-2. Area 04 confirms it is TOOL-017's residual and that it needs a packaged-docs locator to exist first.

**~~PB-5 · `PI_CODING_AGENT` is never stamped into the environment (and `AI_AGENT` is the v0.84.1 half)~~** — ~~*low*~~ **CLOSED 2026-09-22** · area 04 `TOOL-031`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `crates/cyrup-tools/src/tools/bash.rs:309` pushes `CYRUP_CODING_AGENT=true` and `:321` pushes `AI_AGENT=cyrup` into every bash child; `cyrup-ext-subagents/src/exec/spawn_plan.rs:913`/`:923` do the same for every re-exec'd subagent child, with the hard-rename `[CYRUP-DELTA]` recorded at `:911-923`. Both are pinned by `spawn_plan.rs:4193-4243`. **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/coding-agent/src/cli.ts:13` @v0.83.0 (`process.env.PI_CODING_AGENT = "true"`); `AI_AGENT = "pi"` is new at v0.84.1, `cli.ts:14` and `rpc-entry.ts:8`
- cyrup: `crates/cyrup/src/main.rs:53-57` — an explicit comment declines to replicate it because `std::env::set_var` is `unsafe` under edition 2024; `crates/cyrup-tools/src/tools/bash.rs:158-175` assembles the child env explicitly and adds neither key
- observable: a shell hook, npm script or MCP server that branches on `$AI_AGENT` / `$PI_CODING_AGENT` cannot tell it is inside cyrup. The unsafe-`set_var` rationale covers *process-global* mutation only — `bash.rs` already builds a per-child env vector, so both keys can be added there with no `unsafe`. Area 12 rejected a duplicate filing (`DRIFT-044`) because **PB-5 is strictly broader**. Its sibling — `process.title`'s role suffix, which is a syscall against the current process and carries none of the `set_var` hazard — is `DRIFT-051` / `SEAM-070`, filed this pass.

**PB-6 · Changelog-on-upgrade is absent; `lastChangelogVersion` is never read or written** — *medium* · area 05 `CFG-015`, area 07 `TUI-011`
- upstream: `modes/interactive/interactive-mode.ts:997` @v0.83.0 (`getLastChangelogVersion`), `:998-999` (`getChangelogPath` / `parseChangelog`), `:1003` and `:1010` (`setLastChangelogVersion(VERSION)`); getter/setter at `core/settings-manager.ts:660` and `:664`
- cyrup: `crates/cyrup-config/src/settings/effective.rs:705-707` (`last_changelog_version`) has zero callers workspace-wide and no setter exists; `/changelog` is hardcoded at `crates/cyrup-tui/src/app/submit.rs:130-133` to `push_block("What's New", "No changelog entries found.")` *(was `settings.rs:994` / `submit.rs:111-113`; the settings monolith was split into `cyrup-config/src/settings/`, re-resolved by symbol 2026-09-22)*
- observable: after upgrading, pi shows the new entries once and records the version; cyrup shows nothing. The `collapseChangelog` settings row (`app/settings_rows.rs:181`) toggles a value nothing reads. (`enableInstallTelemetry`, the row beside it, **does** have live consumers — `cyrup-config/src/policy.rs:27`, `cyrup-session-svc/src/builder.rs:1779` — so it is not part of this claim.)

**PB-7 · The npm package channel is unported, and `npmCommand` is inert** — *large* · area 05 `CFG-009` / `CFG-015`
- upstream: `core/package-manager.ts:1720` (`getNpmCommand`) with install/update/list through `:1740` (`runNpmCommand`), `:1745` (`getGitDependencyInstallArgs`), `:1753` (`runNpmCommandSync`); manifest kinds at `core/pi-manifest.ts:3-9` — a package may ship `extensions`, **`skills`, `prompts` and `themes`**
- cyrup: `crates/cyrup-resources/src/package/source.rs:79-81` returns `Err(ResourceError::UnsupportedNpm)` for any `npm:` spec **(the misleading "unsupported source (OCI deferred)" message area 05 `CFG-009` recorded is gone — the arm is now typed and its comment names R-09-021; the channel is still absent)**; `crates/cyrup-config/src/settings/effective.rs:373-376` (`npm_command`) has zero production callers anywhere — its only references are `settings/tests/merge_and_scope.rs:463-481` *(citations refreshed 2026-09-22 at `14e6c56`)*
- observable: `cyrup install npm:<pkg>` fails outright, and setting `"npmCommand": ["pnpm","--silent"]` does nothing. The *extension* half is genuinely mechanism-forced (WASM guests cannot load a TypeScript extension); skills, prompts and themes are plain files needing no runtime and are unreachable purely because the channel is gone. See OQ-1. Downstream of this: pi's `.pi-update-incomplete` marker has nowhere to attach (area 05 records it as deliberately not filed for that reason).

**~~PB-22 · `google-vertex` is registered with 10 catalog models and has no wire API — every request dies at dispatch~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 01 `PROV-030`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `crates/cyrup-provider/src/api/google_vertex.rs` exists; `providers/all.rs:88-93` records the residual as closed by it, and the self-contradicting port-status table this row demanded be rewritten now reads `| 103 | google-vertex | ✓ |` (`:24`). **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/ai/src/providers/google-vertex.ts` @v0.83.0 registers the provider **and** its api implementation together
- cyrup: `crates/cyrup-provider/src/providers/all.rs:187` pushes `google-vertex` with a 10-row catalog (`providers/catalog/google-vertex.json`) and resolved auth including the ADC arm, and it appears in `/model`. But `api/mod.rs:130-163` `register_builtins` registers **9** factories and none of them is google-vertex, and there is no `api/google_vertex.rs`. All 10 rows die at `wire.rs:158-166` with `no API implementation for google-vertex`
- observable: selecting any Vertex model fails at stream time with an internal error after the model list has already offered it. **This is the exact failure mode `PROV-005`'s own Fix text warned about for `bedrock-converse-stream`** — it was fixed there and shipped here in the same sweep. `PROV-005` stays closed; this defect is new and carries its own id.
- **Mandatory in the same change (added this pass):** rewrite the port-status doc table at `providers/all.rs:12-47`, which still calls `amazon-bedrock` / `google-vertex` / `openai-codex` "**pending**" and names all four including `github-copilot` at `:46-47` as "Pending (NOT registered)" — while `:176-197` pushes all four and `:21` already marks copilot ported. The table is self-contradictory *and* it flatly denies this item's premise; it is the first thing an engineer opening the file reads. `PROV-038`'s roster rewrite should then assert `all_providers()` matches the set the table marks registered.

**~~PB-23 · GitHub Copilot's Claude models send `x-api-key`; pi sends `Authorization: Bearer`~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 01 `PROV-027`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `api/anthropic_messages/claude_code.rs:56-76` (`resolve_is_oauth`) branches on `model.provider == GITHUB_COPILOT_PROVIDER` **before** the `sk-ant-oat` test, exactly as `anthropic-messages.ts:867-888` does. **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/ai/src/api/anthropic-messages.ts:867-888` @**both** v0.83.0 and v0.84.1 (re-read at both tags this pass, offsets verified equal) branches on `model.provider === "github-copilot"` **before** the OAuth test
- cyrup: `api/anthropic_messages.rs:470-536` `build_headers` has **no provider branch** — the scheme is chosen solely by `is_oauth`, derived at `:434-437` from `api_key.contains("sk-ant-oat")`, and the non-OAuth arm at `:524-531` emits `x-api-key`
- observable: every request on that route arrives unauthenticated. Blast radius measured by parsing the catalog: `github-copilot.json` has 28 rows, exactly **9** on `anthropic-messages`.

**~~PB-24 · `github-copilot-headers.ts` is unported on all three routes~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 01 `PROV-028`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `api/github_copilot_headers.rs` exists (`X_INITIATOR`, `COPILOT_VISION_REQUEST`, `build_copilot_dynamic_headers`, `apply_copilot_dynamic_headers`) and is applied on **all three** routes: `anthropic_messages/headers.rs:151`, `openai_completions/headers.rs:71`, `openai_responses/headers.rs:52`. **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/ai/src/api/github-copilot-headers.ts` @v0.83.0 exports `inferCopilotInitiator` / `hasCopilotVisionInput` / `buildCopilotDynamicHeaders`, applied under the Copilot guard at `anthropic-messages.ts:867-871`, `openai-completions.ts:638-645` and `openai-responses.ts:223-230`. **Citation corrected this pass**: the previously recorded `openai-completions.ts:646-652` is the v0.84.1 offset, not v0.83.0
- cyrup: `rg -i 'X-Initiator|Copilot-Vision|Openai-Intent' crates/cyrup-provider/src` returns only the login flow's unrelated `openai-intent: chat-policy` (`auth/oauth/github_copilot.rs:666`); there is no `api/github_copilot_headers.rs` and no dynamic-header call in any api impl
- observable: Copilot image turns are rejected outright (no `Copilot-Vision-Request` — a loud failure on a normal path) and every agent-loop request is misreported for quota (no `X-Initiator` / `Openai-Intent`).

**~~PB-25 · `continue_run` drains the steering queue before claiming the run latch~~** — ~~***critical***~~ ~~***low***~~ **CLOSED 2026-09-22** **(raised to critical 2026-08-12 on a predicted consequence; LOWERED to low 2026-08-13 after that consequence was measured and refuted — typing during a live stream delivered the message 5/5 times, see `REPRO-LOG.md`)** · area 02 `AGENT-020`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup-agent/src/agent/lifecycle.rs:191-233`: the fast-path guard is hoisted **above** both drains (`:193-196`) and each drain requeues with `push_front` on `Err` (`:212`, `:230`) — both halves this row required, in one function. `agent.rs` no longer exists. **PB-25 + PB-26 "must land in the same change" is moot; both are closed.** **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/agent/src/agent.ts:350` @v0.83.0 — `async continue()`, with the run-active guard at **`:351-353`** running **before both drains** (`:361` steering, `:367` follow-ups). **Citation corrected this pass**: the previous edition cited `:362-364`/`:361-388` and asserted tag-invariance; those are the **v0.84.1** offsets. The bodies are byte-identical across the tags, the line numbers are not
- cyrup: `crates/cyrup-agent/src/agent.rs:1637` `continue_run` drains steering at `:1646` and follow-ups at `:1650`, then `start_run` (`:1659`) claims the latch at `:1672-1682` and returns `Err(AgentError::RunActive)` at `:1681`; `queue.rs:51` `drain()` removes
- observable: an `Err(RunActive)` silently destroys a user-typed steering message — no error, no log, no retry. **Typing while a turn streams is the normal path**, which is why this is critical rather than high. Fix has two halves and needs both: hoist the guard as a fast path, *and* push the drained vec back with a new `PendingQueue::push_front` on `Err` (the fast path is racy in Rust, where pi gets atomicity from single-threaded JS).

**~~PB-26 · `AgentSession::prompt` gates on the agent's per-run flag, so a prompt in the post-run gap starts a SECOND run~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 02 `AGENT-030`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup-session-svc/src/session/mod.rs:493` `is_run_active()` = `!is_idle()` = `driver_tx || agent.is_running()`, and it is what `prompt_run` (`session/run.rs:122`) and `prepare` (`:480`) consult, both citing `AGENT-030`. **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/coding-agent/src/core/agent-session.ts` @v0.83.0 — `_isAgentRunActive` set `:1062`, cleared `:582`, consulted `:876-877` and `:1159`; it spans `_handlePostAgentRun` and every `agent.continue()`, so a submission during the post-run driver loop is routed to steering
- cyrup: `crates/cyrup-session-svc/src/session.rs:627` (and `prepare`, `:854`) gate on `agent.snapshot().is_streaming` (`:3202-3204`), a per-run flag `SettlementGuard::drop` clears at `cyrup-agent/src/agent.rs:1441` the moment each individual run settles. The session already owns the right latch — `driver_tx`, set in `spawn_run` at `:686` and dropped after the whole post-run loop at `:739` — but it is consulted only by `is_idle()` (`:601-603`)
- observable: a prompt landing in the post-run gap (auto-retry, auto-compaction, queued continuation) starts a second run and races `continue_run`. **Must land in the same change as PB-25** or the loss just moves to the other branch.

**~~PB-27 · `.cyrup/SYSTEM.md` and `APPEND_SYSTEM.md` are never discovered — the trust gate prompts about files cyrup will never read~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 05 `CFG-035`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup-resources/src/discovery/mod.rs:387` (`discover_system_prompt_file`) and `:479` (`discover_append_system_prompt_file`), called from production at `cyrup-session-svc/src/builder.rs:1611`/`:1621` and from `discovery/blocking.rs:116`. **The body below is the original filing and is kept as history.**
- upstream: `core/resource-loader.ts:1022-1034` @v0.83.0 `discoverSystemPromptFile()` (project `.pi/SYSTEM.md` when trusted, else `<agentDir>/SYSTEM.md`), `:1036-1048` the identical pair for `APPEND_SYSTEM.md`; consumed in `reload()` at `:525` and `:533-535`. Unchanged at v0.84.1
- cyrup: `grep -rn 'SYSTEM\.md' crates/` returns five hits and **not one reads a file** — a doc comment (`cyrup-session/src/prompt/overrides.rs:12-16`), two trust-gate MARKERS (`cyrup-config/src/trust.rs:194`, `:203-204`) and a test. The only producers of the two override fields are the CLI flags (`cyrup-session-svc/src/builder.rs:1051`, `:1055` ← `cyrup/src/cli.rs:456-463`)
- observable: a project shipping `.cyrup/SYSTEM.md` gets the DEFAULT system prompt with no diagnostic — silent wrong output on a normal path. Made worse by the half-port: `has_trust_requiring_resources` prompts the user to trust the project *because* the file exists, then loads nothing from it. cyrup ported the gate and not the thing it gates. Note pi's `??` semantics: the CLI flag **replaces** the discovered append file, it does not accumulate — `overrides.rs:15-16` documents the opposite and must be corrected in the same change.

**~~PB-28 · `/tree` has no text search, and its four action keys are the characters pi types INTO that search — `e` persists the typed text as a label~~** — ~~***critical***~~ **CLOSED 2026-09-22** **(raised from high this pass)** · area 07 `TUI-027`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `tree_selector.rs:394` carries `search_query` with the `Type to search:` line (`:983-990`) and printable keys appended at `:1231`; `:290` records that `from_digit` was deleted and that the filter modes moved to the seven `app.tree.filter.*` chords. **The body below is the original filing and is kept as history.**
- upstream: `modes/interactive/components/tree-selector.ts:113`, `:1079-1100` — `z`/`x`/`e`/`t` are ordinary characters typed into the tree's text filter
- cyrup: `crates/cyrup-tui/src/tree_selector.rs:850-889` binds them as actions, and `e` opens the inline label editor, which captures all keys. **Corrected trace this pass** — the persistence is two hops past the local star update (`update_node_label`, `:529-533`): the confirm arm returns `SelectorOutcome::Apply(entry_id + FIELD_SEP + label)` (`:540-546`) → `app/selectors.rs:201-208` splits it on `FIELD_SEP` into `AppCommand::SetEntryLabel` → `app/execute.rs:288-298` calls `session.services().host_services.set_label(&entry_id, (!label.is_empty()).then_some(label.as_str()))` → `manager.append_label`, the same live path an extension's `setLabel` uses. *(Trace re-resolved by symbol 2026-08-19: the middle hop was cited as `app.rs:3306-3307` → `app/tree_nav.rs`, and `tree_nav.rs` has no part in it — the `40821ed` remap carried the wrong module forward.)* The seven `app.tree.filter.*` ids are unknown to `TreeAction::from_id` (`:887-895`), so a pi-shaped `keybindings.json` cannot fix it
- observable: a pi user typing a filter word into `/tree` silently renames a session branch **in the session JSONL**. Corruption of persisted user data on a normal path.

**~~PB-29 · A prompt typed during compaction is dispatched immediately instead of queued~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 07 `TUI-031` · **supersedes VL-P11**, which filed the same defect as lag and at *small* — **VL-P11's table row should read "→ PB-29, CLOSED"**
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** The row's in-body closure is confirmed by an independent re-grep: the compaction guard is live at `app/run_action.rs:107`, with pi's follow-up gate at `:154`. **The body below is the original filing and is kept as history.**
- upstream: `modes/interactive/interactive-mode.ts:3023-3033`, `:4230-4236` — `queueCompactionMessage` with a visible status; the session-level throw is `core/agent-session.ts:1133-1137`
- cyrup: the TUI never consults `is_compacting` (the `AppAction::Submit` arm, now `app/run_action.rs:83-103`, branched on `is_streaming` only) **and** `AgentSession::prepare` has no compaction guard either (`session.rs:849-900`); `is_compacting` exists at `session.rs:4110` and its one production consumer is an RPC status field (`cyrup-modes/src/rpc.rs:1428`)
- **CLOSED — verify before scheduling.** Area 07 struck `TUI-031` on 2026-08-14 and the guard is at HEAD (`4fb5e40`): `app/run_action.rs:68-82` is a `Submit` arm guarded `if ctx.session.is_compacting() && !is_extension_command(…)` sitting **above** the streaming arm, routing to `queue_compaction_message(text, false)`, with pi's identical follow-up gate at `:116-117`
- observable: the turn is assembled from a context the compaction is mid-rewrite of. Not "rejected instead of accepted" — **wrong context, silently**. Note `TUI-016`: there is currently no surface that would *show* a queued message, so the queue and its indicator ship together.

**~~PB-30 · First SIGTERM/SIGHUP neither tears down nor exits 143/129 — `--mode rpc` cannot be stopped by a supervisor~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 08 `SEAM-047`, area 12 `DRIFT-049` (**duplicate — schedule once, in area 08**)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup/src/signals.rs` now runs `kill_tracked_detached_children()` (`:287`), `runtime.dispose().await` (`:324`) and `process::exit` with pi's codes on the **first** delivery (`first_delivery_exit_code`, `:224`), with the repeat force-exit at `:304`. **The body below is the original filing and is kept as history.**
- upstream: `modes/rpc-mode.ts:366-383` @v0.83.0 registers SIGTERM plus SIGHUP off-win32 and calls `killTrackedDetachedChildren()` then `shutdown(SIGHUP?129:143, signal)`; `shutdown` at `:724-741` runs `runtimeHost.dispose()` then `process.exit`. `print-mode.ts:50-66` is the same shape
- cyrup: `crates/cyrup/src/signals.rs:88-101` does only `session.abort()` + `cancel.cancel()` on the first delivery, and the token it fires is `main.rs:367`'s **TUI input** `CancelToken`; neither `run_rpc` (`cyrup-modes/src/rpc.rs:575-579`) nor `run_print`/`run_json` takes a cancel token at all, and `rpc_driver`'s `select!` (`rpc.rs:717-842`) has no cancellation arm
- observable: `cyrup --mode rpc` never returns, `runtime.dispose()` never runs, and no `session_shutdown` is ever emitted. Interactive/print/json survive only incidentally, because the mode loop returns and `main.rs:575` / `run.rs:39` / `run.rs:60` then dispose. The repeat force-exit path *is* implemented with pi's exact 130/143/129 codes (`signals.rs:97-100`) — which is why `SEAM-S02` should be re-audited as **closed**. **Ships with `SEAM-059`** (the watcher holds the startup session `Arc` and aborts a disposed session after any `new_session`/switch/fork), because both rewrite the same function.

**~~PB-33 · The undo snapshot omits the paste registry — one undo sends the literal `[paste #N …]` marker to the model~~** — ~~***critical***~~ **CLOSED 2026-09-22** · area 07 `TUI-042` (new this pass)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup-tui/src/editor/mod.rs:110-115` — `Snapshot` carries `pastes` (and `paste_counter`), documented against `editor.ts:218`. **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/tui/src/editor.ts:216-220` — `EditorSnapshot` carries `pastes` **and** `pasteCounter`, restored at `:2012-2030`. Byte-identical at v0.83.0 and v0.84.1 **at the same line numbers** (checked at both tags, not asserted)
- cyrup: `crates/cyrup-tui/src/editor.rs:71-78` — `Snapshot { lines, row, col }`, no pastes. `backspace()` (`:814`) and `delete()` (`:852`) erase `pastes[N]` *after* the snapshot was pushed, and `undo()` (`:748-756`) restores only the visible text, so `marker_at` (`:663-694`, which ends `self.pastes.get(&id)?`) no longer matches. `history_draft` (`:93`, `:1199`, `:1218`) reuses the same struct and inherits the defect
- observable: undo restores the marker text and Enter sends ~20 literal characters instead of the pasted content — silent wrong output, on a keystroke pair every user makes. Ships with `TUI-044` (the same `undo()` discards `Snapshot::col`, a field written and never read).

**~~PB-34 · Word motion and Ctrl+W are not paste-marker atomic — one Ctrl+W after a large paste drops the paste~~** — ~~***critical***~~ **CLOSED 2026-09-22** · area 07 `TUI-043` (new this pass)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `editor/motion.rs:141` (`word_left_target`) and `:179` (`word_right_target`) segment through `word_segments`, which marks a whole paste marker `atomic: true` (`:110`), porting `word-navigation.ts`'s `isAtomicSegment`; `edit.rs:114`/`:126` delete to those targets. **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/tui/src/word-navigation.ts:9-14` declares `isAtomicSegment` with pi's own paste-marker comment; `findWordBackward`/`findWordForward` take the atomic branch at `:44-46` and `:97-99` (present at v0.83.0)
- cyrup: `editor.rs:1074-1128` `word_left_target`/`word_right_target` classify only by `is_word_char` (`:1637-1639`) and never call `marker_covering` (`:697-712`, which has just two callers); `delete_word_backward`/`delete_word_forward` (`:874-892`) never drop the registry entry the way `backspace()` does at `:814`
- observable: one Ctrl+W at the end of `[paste #1 +42 lines]` deletes the single `]`, the marker stops matching, and Enter sends the 19-character fragment. Ships with `TUI-042`; `TUI-049` (`marker_at` accepts text pi's regex rejects) is the same code and should be folded in.

**~~PB-35 · `httpProxy` reaches only the streaming wire APIs — OAuth, the agent proxy transport and extension HTTP all bypass it~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 01 `PROV-047` (new this pass)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup-provider/src/lib.rs:165-166` exports `build_client_for`, `build_client_for_target`, `build_client_with_proxy` and `configure_http_proxy`; all five OAuth flows, `cyrup-agent/src/proxy/transport.rs:58-61` and `cyrup-ext/src/caps/http.rs:139-148` are on the per-target resolver, each citing `PROV-047`. **The body below is the original filing and is kept as history.**
- upstream: `packages/ai/src/utils/http-dispatcher.ts:43-48`, `:79-103` @v0.83.0 — pi installs a **process-global** undici dispatcher, so every `fetch` in the process is proxied
- cyrup: `cyrup-session-svc/src/builder.rs:229-239` turns the setting into a `ProviderEnv` overlay read solely by `sse.rs:181-192` `build_client_for_target`. Every other egress path calls `build_client()` (`sse.rs:140-144`), which has no proxy handling: five OAuth flows (`auth/oauth/{anthropic:443, openai_codex:552, xai:525, openrouter:372, radius:468}`), `cyrup-agent/src/proxy.rs:455`, and `cyrup-provider/src/wire.rs:472`; `cyrup-ext/src/caps/http.rs:599` is a bare `reqwest` builder with reqwest's own competing env detection
- observable: on a proxied network, streaming works and logging in does not — and the failure is a connection error with no mention of the proxy. Fix: `configure_http_proxy()` beside `configure_http_idle_timeout`, `build_client_for(target_url)` running the already-ported resolver, the URL threaded through the seven call sites, and `.no_proxy()` on `caps/http.rs` so reqwest's detection is retired process-wide.

**~~PB-36 · A lone-surrogate `\uXXXX` escape in a provider SSE frame kills the whole assistant turn~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 01 `PROV-048` (new this pass)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup-core/src/json.rs:164-183` — `repair_json`'s `Some('u')` branch now drops an unpaired high or low surrogate escape (and keeps a paired run verbatim), so `repaired != json` and the retry succeeds. **The body below is the original filing and is kept as history.**
- upstream: `packages/ai/src/utils/sanitize-unicode.ts` @v0.83.0 — `JSON.parse` accepts the escape and `sanitizeSurrogates` strips it on the way back out. (The **outbound** half is a correct documented no-op in Rust: a `String` cannot hold a lone surrogate. Only the inbound half is a gap.)
- cyrup: `serde_json` rejects it; `repair_json` re-emits it verbatim (`utils/json_parse.rs:67-75`) so `repaired == json` and `parse_json_with_repair` returns `None`; both SSE callers treat `None` as fatal (`anthropic_messages.rs:1439-1449`, `google_generative_ai.rs:975-985`)
- observable: one malformed escape from a provider ends the turn. The same weakness breaks **resuming a pi-written session JSONL**. Fix is one arm: in `repair_json`'s `Some('u')` valid-hex branch, drop an escape decoding to an unpaired surrogate so `repaired != json` and the retry succeeds. Ships with `PROV-049` / `PROV-050` (the other two defects in the same repair path).

**~~PB-37 · `--resume` lists every project's sessions under "Current Folder", and the `tab scope` hint it prints is dead~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 08 `SEAM-061` (new this pass)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup/src/startup_ui.rs:218` calls `selector.set_all_rows(…)` and `:221-226` `set_session_cwds(…)`; `session_selector.rs:322` (`toggle_scope`) is the live Tab handler. *(The row's own "verification requires a live run in a real terminal with two project dirs" caveat still stands for the rendering; the three code halves are what was re-greped.)* **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/coding-agent/src/cli/session-picker.ts:15-19` @v0.83.0 (byte-identical at v0.84.1) — `selectSession(currentSessionsLoader, allSessionsLoader, settingsManager)` takes **two** loaders and passes both to the component, which starts at `scope: "current"` (`components/session-selector.ts:704`), loads only the current set (`:859`), and toggles on Tab (`:551-556`); `main.ts:419-421` supplies `SessionManager.list(cwd, …)` and `SessionManager.listAll(…)`
- cyrup: `crates/cyrup/src/main.rs:1259-1268` `gather_session_infos` concatenates the cwd listing and the cross-project listing into **one** vector, handed to a `SessionSelector` defaulting to `scope: Current` (`cyrup-tui/src/session_selector.rs:204`); no `SessionAction::ToggleScope` exists, so the advertised hint cannot fire and `show_path` never flips
- observable: on any machine with more than one cyrup project the picker is headed "Current Folder" over every session on disk, with no cwd column and rows labelled only by their first message; picking a foreign row resumes another project's session with no guard (`main.rs:1124`). Both halves must land together or the screen keeps lying. **Verification requires a live run in a real terminal** with two project dirs.

**~~PB-38 · The pre-launch rename is invited, accepted, echoed, and discarded~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 08 `SEAM-062` (new this pass)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `startup_ui.rs:241-245` — `run_resume_picker`'s `on_apply` now matches `SessionSelectorOutcome::Rename` and calls `cyrup_session_svc::rename_session_file_at`. **The body below is the original filing and is kept as history.**
- upstream: `cli/session-picker.ts:48` @v0.83.0 (byte-identical at v0.84.1) passes `showRenameHint: false` and **no** `renameSession` callback, so `canRename` is false (`components/session-selector.ts:771`), the hint is hidden (`:772`) and the handler bails before entering rename mode (`:807-808`). pi's pre-launch picker cannot rename at all
- cyrup: `session_selector.rs:214` defaults `show_show_rename_hint: true` and `startup_ui.rs:126-127` never disables it; `SessionAction::Rename` is ungated (`:833-837`), the row is mutated in place (`:798-801`) and an `Apply(rename_payload)` is returned (`:802`) — but `run_resume_picker`'s `on_apply` (`startup_ui.rs:129-138`) matches only `Delete`
- observable: complete positive feedback for an operation that is never persisted. Same class as PB-28 (typed text accepted and thrown away), on a surface nobody had read. Minimum fix is two lines; the preferred fix reuses `session.rs:3355-3365`'s existing rename sequence.

**~~PB-39 · Session delete permanently unlinks where pi routes through `trash`, and the failure is swallowed~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 08 `SEAM-063`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** The pre-launch half this row flagged as the part to re-check is done too: `startup_ui.rs:233-238` goes through `delete_session_file_at` and prints `method.status_message()` / `Failed to delete: {e}`. **The body below is the original filing and is kept as history.**
- upstream: `modes/interactive/components/session-selector.ts:645-680` @v0.83.0 (identical at v0.84.1) — `deleteSessionFile` runs `spawnSync("trash", …)` **first** with a `["--", path]` guard (`:649`), treats exit-0 **or** the file having vanished as success with `method:"trash"` (`:666-668`), falls back to `unlink` (`:672-674`), and on failure returns `{ok:false,error}` with a `trash: …` hint (`:675-679`); the caller reports which happened
- cyrup: two bare `std::fs::remove_file` sites — `startup_ui.rs:133-137`, which additionally `let _`s the `io::Result`, and `cyrup-session-svc/src/session.rs:3343-3347`, whose caller (the `C::DeleteSession` arm, now `app/execute_session.rs:15-33`) prints "deleted session" either way. `rg -ni 'trash' crates/` returns nothing
- **CLOSED — do not schedule.** Area 08 struck `SEAM-063` on 2026-08-14 and the seam is at HEAD: `delete_session_file` returns a method, and `app/execute_session.rs:24-32` pushes pi's own `"Session moved to trash"` / `"Session deleted"` / `Failed to delete: {e}` strings (`session-selector.ts:846`/`:849` @v0.83.0). The pre-launch `--resume` half — the residual `REPRO-LOG.md` §3 measured — is the part to re-check
- observable: for every user with `trash` installed, pi's delete is recoverable and cyrup's is not — one confirmed keypress destroys a conversation JSONL with no undo, on the same screen whose *reversible* action (PB-38) is the broken one. A failed delete on a read-only volume looks identical to a successful one.

**~~PB-40 · The pre-launch trust prompt drops both "(this session only)" options — every answer is persisted~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 08 `SEAM-064` (new this pass)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup-config/src/trust.rs:387` (`trust_options(cwd, include_session_only)`) carries both ephemeral rows at `:407-424`, and `startup_ui.rs:759` records that the pre-launch path now asks for `includeSessionOnly: true`. **The body below is the original filing and is kept as history.**
- upstream: `core/project-trust.ts:32` @v0.83.0 (identical at v0.84.1) — the **pre-launch** path calls `getProjectTrustOptions(cwd, { includeSessionOnly: true })`; `core/trust-manager.ts:82-84`, `:91-93` append the two ephemeral options with an empty `updates`, which `saveProjectTrustPromptResult` skips writing (`project-trust.ts:40-44`). pi's **in-app** selector passes no flag — so the asymmetry is deliberate upstream
- cyrup: `crates/cyrup/src/main.rs:1155` passes `false`. That flag gates exactly both ephemeral rows (`cyrup-config/src/trust.rs:356-363`, `:370-377`), so the prompt renders three rows, every one with a non-empty `updates`, and `run_trust_prompt` persists unconditionally (`startup_ui.rs:266-268`). cyrup's other call site (`session.rs:3255`) is correct and must be left alone
- observable: a user cannot answer a security prompt about someone else's repository without recording a permanent verdict in `trust.json` — including "Do not trust", which then locks the folder out with no prompt offered to reverse it. One-line production change plus a test rewrite (`startup_ui.rs:504-537`).

**~~PB-41 · Trust is resolved pre-launch, inverting pi's tier order — the extension `project_trust` hook never runs~~** — ~~*high*~~ **CLOSED 2026-09-22** · area 08 `SEAM-065` (new this pass)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup/src/prelaunch.rs:214-226` states in-tree that project trust is **not** resolved pre-launch any more, and `trust_prompt_callback` (`:237`) is handed to the builder, which invokes it only on `TrustOutcome::NeedsPrompt` — i.e. after `pre_trust_extension_verdict` (`cyrup-session-svc/src/builder.rs:783-788`) and the store. pi's tier order is restored. **The body below is the original filing and is kept as history.**
- upstream: `core/project-trust.ts:46-95` @v0.83.0 (identical at v0.84.1) — `resolveProjectTrusted` orders the tiers: `trustOverride` (`:47`), no-trust-requiring-resources (`:50`), then **`emitProjectTrustEvent` at `:54-70`**, which returns before anything else and persists when `remember === true`; only then the store (`:72-75`), the default policy (`:77-84`), `hasUI` (`:86-88`), and last the prompt (`:90-94`)
- cyrup: `main.rs:325-329` calls `resolve_startup_ui` **before** any runtime exists; `main.rs:1142-1162` resolves trust from store + default policy, prompts, and sets `config.trust_override` (`:1159`) — which short-circuits `cyrup-session-svc/src/builder.rs:495-499`, so `pre_trust_extension_verdict` never runs whenever the user answered
- observable: an extension implementing `on-project-trust` (declared at `cyrup-ext-sdk/wit/world.wit:237`) is defeated on the interactive path: the human is asked first and wins, the hook is suppressed, and its `remember` half never fires. **Reachability caveat, stated because it bounds the severity**: no in-tree extension implements the hook today, so the bypass is latent — but the seam the builder was written for is dead on the path that matters. Also retires `builder.rs`'s `saved: None` and its "no trust store is wired" warning.

### 1b. From `pi-subagents` v0.43.0

> **Reclassification, and it is large.** The previous edition filed `VL-S1…VL-S15` as version lag on
> the strength of "baseline ≈v0.34.0 with holes". The recorded baseline is **v0.43.0**, and every one
> of those fifteen has a first tag **at or before v0.43.0** — so by this document's own baseline they
> are in-baseline **port bugs**, not lag. Area 09 proved three of them directly with
> `git cat-file -e v0.43.0:<path>` (`capability-ceiling.ts`, `usage-budget.ts`, `spawn-budget.ts` all
> present at both v0.43.0 and v0.47.1) and re-classified its own `SUBA-021`, `SUBA-017` and
> `SUBA-022` the same way. **The ids do not change; the section does.** The genuine
> `v0.43.0..v0.47.1` lag is §3b, and it is 17 items nobody had looked at.
>
> Area 09 deliberately does **not** restate `PB-8…PB-14`, `UW-3…UW-8` or `VL-S1…VL-S15` as findings —
> it confirmed them still accurate at HEAD by spot-check and left them owned here. **This section is
> therefore the only record for that work; do not compress it away.**
>
> **SWEEP 2026-09-16 at cyrup `cc7818b`, and the reason it mattered: because area 09 left these rows
> owned HERE, nothing in the area files was ever going to correct them.** All **23** entries
> (`PB-8`…`PB-14`, `PB-31`, `VL-S1`…`VL-S15`) were re-greped against the code that pass.
> **TEN closed** — `PB-10`, `PB-11`, `PB-13`, `PB-31`, `VL-S1`, `VL-S2`, `VL-S7`, `VL-S9`, `VL-S14`,
> `VL-S15`. **ONE was partially closed and said which half** — `PB-12`. **TWELVE stayed open with
> refreshed evidence** — `PB-8`, `PB-9`, `PB-14`, `VL-S3`, `VL-S4`, `VL-S5`, `VL-S6`, `VL-S8`,
> `VL-S10`, `VL-S11`, `VL-S12`, `VL-S13`. 10 + 1 + 12 = 23, which is the whole section. Closed
> entries are struck and keep their bodies as history, per this directory's id-retention rule.
>
> **Re-derived from the rows themselves at `14e6c56` (2026-09-22), because the running total above
> had been amended four times and was wrong in both directions.** Of the twelve, **TEN have since
> closed**: `PB-8` (2026-09-18), `VL-S10` and `VL-S13` (2026-09-19), `VL-S5` (2026-09-19),
> `VL-S3` + `VL-S4` (2026-09-20 — one feature, **two rows**; the previous edition counted them as
> one and so said "FOUR" while naming five), and `VL-S6`, `VL-S8`, `VL-S11`, `VL-S12` (2026-09-21).
> `PB-12`'s partial closure became full on 2026-09-19. **The current open set of this section is
> exactly TWO rows — `PB-9` and `PB-14`** — against **TWENTY-ONE** closed. 21 + 2 = 23, which is
> still the whole section. Both survivors were re-greped at `14e6c56`; both are genuinely open and
> both carry drifted cyrup addresses, refreshed in their own rows.
>
> **2026-09-23: both survivors closed, so the section's open set is now EMPTY (23 closed).** `PB-9`
> closed at `076a5f9` by Plan A: the `clarify` key is refused with upstream's text, as upstream does
> at v0.68.0. `PB-14` closed at `f978396`, on both surfaces. Each closing row carries its evidence.
>
> **Read every un-refreshed `extension.rs:<line>` citation in this section as DEAD, not as stale.**
> That file was split into `extension/{mod,wait_tool,models,testsupport}.rs` + `extension/{executor,host,tool}/`
> and no line in it resolves. The rows this pass touched carry current addresses; the rest do not, and
> the ⚠ block at the top of this file states the general repair.

**~~PB-8 · Subagent RPC bridge is entirely absent~~** — ~~*large*~~ **CLOSED**
- **CLOSED 2026-09-18.** `extension/rpc/` (5 files, 2 527 LOC) answers all **EIGHT** upstream methods
  over `subagents:rpc:v1:request`, replying on `subagents:rpc:v1:reply:<requestId>` and announcing
  `subagents:rpc:v1:ready`. Registered from production `init`:
  `extension/host/native_impl.rs:261` (`api.subscribe_bus(SUBAGENT_RPC_REQUEST_EVENT)`, in the
  `RegistrationMode::Full` arm only) with the handler at `:701` (`on_bus_event`)
- **The method list in the body below is WRONG and is kept only as history.** It says seven;
  `src/extension/rpc.ts:34 @v0.68.0` has eight — `ping`, `status`, **`manage`**, `spawn`, `steer`,
  `interrupt`, `stop`, `resume` — and `manage` carries seven `schedule.*` actions (`:65-73`) that map
  onto the verbs SCOPE_11 landed. The file is 848 lines, not the 653 this row cites from v0.43.0
- **Reachability, end to end, through production only:** `crates/cyrup/src/cli/runtime_mode.rs` →
  `session_launch.rs:107`/`:115` → `SubagentsExtension` → `cyrup-session-svc/src/builder.rs:1220`
  `load_native_with_services` (with `attach_event_bus` already run at `:1186`) → `init`'s
  `subscribe_bus` → `cyrup-ext/src/facade.rs:2785` dispatch → reply via
  `LiveHostServices::emit_event` (`cyrup-session-svc/src/host_services.rs`). `BusFanout::drain_bus`
  loops to 64 rounds, so a reply emitted inside `on_bus_event` reaches the caller in the SAME
  `deliver_bus_events` call — no pump, no sleep
- **It is a dispatch layer over code that already existed**, which is what makes it reachable rather
  than a new subsystem. The control verbs route through `SubagentTool::execute` rather than
  re-implementing upstream's inline `stopAsyncRun` (`rpc.ts:561-701`), which keeps the authority
  consult at `extension/tool/routing.rs:1723` in the path — **stricter than upstream, deliberately**,
  because this is now an externally-reachable control surface. The bridge dispatches into the SAME
  `Arc<SubagentTool>` `api.register_tool` received, not a fresh one, so an RPC-driven spawn and a
  model-driven spawn share one `DispatchGuard`
- `crates/cyrup-ext-subagents/src/tests/rpc_bridge_integration.rs` drives a real `ExtensionHost`,
  `load_native_with_services`, `init`, `host.bus().emit` and `deliver_bus_events`. Every method is
  exercised over the bus. 22 mutations were run across the implementation and remediation passes,
  each with an OBSERVED failure


- upstream: `src/extension/rpc.ts:622` @v0.43.0 (`registerSubagentRpcBridge`, 653-line file; method list `:29`; event names `:25-27`), registered from `src/extension/index.ts:529`. First tag **v0.33.0**. **Re-read at v0.68.0 this pass** (`git -C tmp/pi-subagents show v0.68.0:src/extension/rpc.ts`): `registerSubagentRpcBridge` has moved to **`:817`** and the three event constants are `:30-32` (`SUBAGENT_RPC_REQUEST_EVENT` / `…_READY_EVENT` / `…_REPLY_EVENT_PREFIX`). The surface grew; it did not go away
- cyrup: **the old `extension.rs:9313-9352` citation is DEAD — that file no longer exists.** The current registration/subscription block is `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:45-250` (`async fn init`), whose `api.subscribe(&[…])` is `:246` and whose `register_tool`/`register_command` calls are `:48`, `:142`, `:156`, `:171`, `:200`, `:221`, `:234`. **No bridge**, and `grep -rn 'subagents:rpc' crates/` **still returns 0**, re-run at `cc7818b`
- **Do not confuse this row with SCOPE_12.** `background/inspect_rpc/` (4 files, 2 459 LOC, landed `e61ff44`) ports a **different upstream file** — `runs/background/inspect-rpc.ts` (443 LOC @v0.68.0), a read-only artifact inspector reached through `/subagents-inspect-rpc` and cyrup's own `inspect` verb. It registers no event bridge, answers no `ping`/`spawn`/`steer`, and its own module doc says so. PB-8 is untouched by it
- observable: no host, embedder or sibling extension can drive subagents programmatically. Upstream answers `ping`/`status`/`spawn`/`steer`/`interrupt`/`stop`/`resume` over `subagents:rpc:v1:request` with a `subagents:rpc:v1:reply:<id>` envelope; cyrup emits no ready event and answers nothing. *(Area 09 blind spot 3: `src/extension/rpc.ts` was read on the upstream side for the first time in this 2026-09-16 pass, for the offsets above only — the method list was not diffed against v0.43.0.)*

**~~PB-9 · `clarify` is advertised and accepted, but upstream refuses it at v0.68.0~~** — ~~*small (parity)*~~ **CLOSED 2026-09-23 at `076a5f9` (Plan A, v0.68.0 parity)** · plan `.flux/todo/CLARIFY_PREVIEW.md`

> **CLOSED by Plan A.** cyrup now refuses the `clarify` key, whatever its value (`true`, `false`, `null`, a string), with upstream's exact text `"Public workflowScript execution does not support clarify UI."` (`extension/tool/text.rs:550`, `CLARIFY_REFUSAL`):
> * **tool** — `extension/tool/params.rs:64-82` (`normalize_public_subagent_execution`) runs the blank-action refusal first and the clarify refusal second, which is upstream's order (`public-execution.ts:133-135` then `:143-145`). Key presence is taken from the raw params before the typed parse. Pinned by `the_public_boundary_refuses_any_clarify_value_on_both_registrations` (root and child-safe) and by the ordering test after it (`text.rs`).
> * **RPC `spawn`** — `extension/rpc/params.rs` passes `input.contains_key("clarify")` into the same normalizer. The foreground bug below is fixed: `{…, clarify:true}` is refused before it can run. Pinned by `spawn_refuses_clarify`.
> * **slash** — cyrup's slash parser has no way to produce a `clarify` key (`extension/host/slash.rs:899` sets none), so no slash call can reach the key. Nothing to refuse there.
> * **deleted** — the `clarify` field on the tool params, its `is_background` term (pinned by `is_background_ignores_a_clarify_key`), the `[async]`-badge exception in `native_impl.rs` `render_subagent_call`, and the schema property. The bundled `SKILL.md`, `docs/tool-reference.md` and `review-loop.md` no longer advertise the UI (pinned by `registration/guide.rs` `bundled_docs_do_not_advertise_clarify`).
> * **left alone on purpose** — `exec::RunOptions::clarify`, `ClarifyChannel` and `tui::intercom::spawn_clarify`. These belong to the `contact_supervisor` ask (R-SA-037), which shares the word but is a different feature.
>
> Plan B (port the v0.42.1 preview as a `[CYRUP-DELTA]`) was not taken. The corrected row text from plan §8 is kept below as the history of what was open.

- upstream @v0.68.0: `clarify` is **not in the schema** (`schemas.ts`, 0 hits), and the public boundary refuses **any** value: `public-execution.ts:143-145` → `"Public workflowScript execution does not support clarify UI."`, reached from the tool (`index.ts:738`), RPC spawn (`rpc.ts:516`) and slash (`slash-bridge.ts:82`). The preview UI (`ChainClarifyComponent`) was removed from the schema in `39c37184` (first tag v0.43.0) and **deleted** in `ef554d2a` #1166 (first tag v0.51.0). The older `chain-clarify.ts:199` / `subagent-executor.ts:3190,:3572` / `chain-execution.ts:692` cites resolve only at v0.43.0, a tag where the public tool already refused `clarify`.
- cyrup (before `076a5f9`): advertised it at `extension/tool/schema.rs:556` with the v0.42.1 description, parsed it at `params.rs:228`, and used it only in `is_background` (`params.rs:631-637`, which forced foreground) and in the `[async]` badge (`native_impl.rs:1126-1128`). The bundled `SKILL.md:402-416` and `docs/tool-reference.md:105` advertised a UI that did not exist.
- **bug:** RPC `spawn` (`rpc/params.rs:196-223`) never checked `clarify`, so `{…, clarify:true}` ran in the FOREGROUND even though it was forced to `async:true`. Upstream refuses this at v0.42.1 (`rpc.ts:432-433`) and at v0.68.0 (`rpc.ts:516`).
- fix: `.flux/todo/CLARIFY_PREVIEW.md`, Plan A.

**~~PB-10 · `turnBudget` — no soft assistant-turn budget for children~~** — ~~*medium*~~ **CLOSED** · = area 09 `SUBA-008`, closed there 2026-08-14 (sweep 8)
- **CLOSED, re-verified in code 2026-09-16 at `cc7818b`.** `exec/turn_budget.rs` exists (~888 LOC); `"turnBudget"` now has **29** occurrences across the crate including the advertised schema key, and the run-level budget is carried onto the runner at `extension/executor/background.rs:622` (`turn_budget` on `RunnerConfig`, commented "SUBA-008 — the run-level turn budget the orchestrator resolved, carried verbatim"). The three hard-coded `false` consumers this row names are gone. **The body below is the original filing and is kept as history.**
- upstream: `src/runs/shared/turn-budget.ts:5` (`resolveTurnBudgetConfig`) and `:26` (`appendTurnBudgetSystemPrompt`); tool param `src/extension/schemas.ts:328`. First tag **v0.33.0**
- cyrup: the tool schema at `extension.rs:6634` has 45 `props.insert` keys and none is `turnBudget`; the flag has **three** hard-coded `false` consumers, each commented as having no source — `tui/intercom.rs:348-352`, `exec/fallback.rs`, `exec/mod.rs:2354-2360` *(the previous count of two was wrong)*
- observable: no "## Turn budget" wrap-up block in the child's system prompt, no abort past `maxTurns+graceTurns`, and the result always reports `turnBudgetExceeded: false`, so an unexplained process signal is misattributed. (Frontmatter `toolBudget` **is** read, `discovery/frontmatter.rs:850` — this is the turn half only.)

**~~PB-11 · Scheduled subagent runs (`schedule.*`) are unported — and it is NINE verbs, not four~~** — ~~*large*~~ **CLOSED 2026-09-16** · = area 09 `SUBA-016`

> **CLOSED at `7e41cf9` (PR #140, merge `cc7818b`) — "finish the SCOPE sequence — capacity, status, retention, scheduled runs".** All three closure tests this directory requires are met and were run by this pass, not taken from a commit subject.
>
> * **Present.** `background/scheduled_runs/` — 8 files, **6 669 LOC** (`ceiling_gate` 407, `manager` 266, `mod` 158, `schedule` 1 712, `store` 1 419, `test_fixtures` 80, `tool` 1 040, `trigger` 1 587). `tool.rs:35-45` is `SCHEDULED_RUN_ACTIONS`, all **nine** verbs in upstream's own order, with `ScheduledRunAction::{from_wire,as_str,is_mutating}` beside it.
> * **Reachable from production.** Advertised at `extension/tool/text.rs:298-306` (inside `SUBAGENT_ACTIONS` — ~~42~~ **59** verbs at `14e6c56`, `text.rs:234`; see `VL-S13`) and **dispatched** at `extension/tool/routing.rs:1243-1250` through `ScheduledRunAction::from_wire`, in upstream's own dispatch position; `schedule.create` is gated through the existing `registration::authority::AuthorityAction::ScheduleCreate` arm (`registration/authority.rs:77`) rather than a second policy surface, and the six mutating verbs are refused from child-safe fanout. The manager is installed from `extension/executor/scheduled_runs.rs:292` (`install_scheduled_runs`), gated on `cfg.scheduled_runs_enabled()` (`registration/mod.rs:573`).
> * **Pinned by tests.** `cargo nextest run -p cyrup-ext-subagents -E 'test(schedule)'` → **80 run, 80 passed** (this pass, at `cc7818b`), including `every_scheduled_run_action_dispatches` — the advertise-vs-dispatch proof — plus `the_armed_tick_fires_a_due_schedule_with_nobody_asking`, `run_due_reports_what_it_processed`, `deleting_a_schedule_with_a_live_run_is_refused` and `the_max_pending_limit_refuses_the_twenty_first_schedule`.
>
> **The row's own evidence line was false when this pass opened it, and both halves are corrected here rather than deleted.** *"Zero hits for `scheduled_runs`"* — there are now hits in eleven files outside the module. *"The 27-verb enum at `extension.rs:6557` has nothing beginning `schedule.`"* — `extension.rs` does not exist, the enum is `extension/tool/text.rs:265` *(was `:215`)* and has ~~42~~ **59** verbs at `14e6c56`, nine of them `schedule.*`. The `extension.rs:3909` and `extension.rs:12572` in-tree notes this row quoted are likewise gone with the file.
>
> **Upstream re-measured this pass at the real latest tag.** `scheduled-runs.ts` is **1 012 LOC @v0.68.0** (753 at v0.43.0/v0.47.1) and `SCHEDULED_RUN_ACTIONS` is still exactly the nine verbs (`v0.68.0:src/runs/background/scheduled-runs.ts:20-30`); the union is `shared/types.ts:2801`. **The `v0.47.1..v0.68.0` growth in that file (+259 lines) was NOT diffed against the port** — a residual, stated so it is not mistaken for coverage.
>
> **Area 09's own table still carries `SUBA-016` as an OPEN medium.** That file is not this pass's to edit; the discrepancy is recorded in §0's thirteenth edition and in `00-residual-ledger.md`'s eleventh edition, and it is why the script's open count is a known overcount by this row.

- upstream *(original filing, kept as history)*: `src/runs/background/scheduled-runs.ts:14` (`SCHEDULED_RUN_ACTIONS`) and `:358` (`class ScheduledRunManager`), 753-line file, present at v0.43.0 and v0.47.1. **Nine** verbs in `shared/types.ts:1968`. First tag **v0.33.0**
- ~~cyrup: zero hits for `scheduled_runs`; the 27-verb enum at `extension.rs:6557` has nothing beginning `schedule.`; `extension.rs:3909` states "The `schedule.*` family is unported"~~ — **all three claims false at `cc7818b`, see above**
- ~~observable: `subagent({action:"schedule.create", at:…})` is refused as an unknown action **after its schedule parameters are silently discarded**~~ — **no longer observable**; the verb dispatches, and a disabled install answers pi's own `"Scheduled runs are disabled by scheduledRuns.enabled=false."` (`background/scheduled_runs/tool.rs:48-49`)

**~~PB-12 · No live child transcript writer; the `transcriptPath` artifact is missing~~** — ~~*medium*~~ **CLOSED 2026-09-19**
- **CLOSED 2026-09-19.** The WRITER half shipped: `crates/cyrup-ext-subagents/src/exec/child_transcript.rs`
  (`ChildTranscriptWriter`, pi `src/shared/child-transcript.ts` @v0.68.0) is created by `exec::run_sync`
  before the first spawn (`exec/mod.rs:481-498`, sentinel first) and fed from the crate's ONE parse
  point, `exec/drive_attempt.rs:380-382` `handle_child_line`, which both the foreground executor and
  the detached runner reach — so `<base>_transcript.jsonl` grows WHILE the child runs on both paths.
  Gate: `RunOptions::transcript` (`exec/agent_config.rs:663`), produced at
  `extension/executor/foreground.rs:962` and `background/runner_main/executor.rs:800` (the first reader
  of `ArtifactConfig::include_transcript`). Published: `SingleResult::{transcript_path, transcript_error}`
  (`exec/run_result.rs:242,249`), `StepStatus` stamped at declaration (`runner_main/entry.rs:270-281`,
  `flat_index::resolve_async_step_transcript_path`) and post-step (`runner_main/status.rs:387-390,
  428-431`), `ResultFile` via `settle.rs:628-629`. The FleetView pane now opens `paths.transcript_path`
  for a running foreground child and `step.transcript_path` for an async one (`tui/fleet.rs:1156,
  1186-1191`). Proven mid-run by `crates/cyrup-it/tests/subagents/child_transcript_live_integration.rs`
  (a real fixture child parked behind a 30 s sleep, the file read with the run future still pending,
  gutting mutation observed). Not ported, disclosed in the module doc: upstream's stdout/stderr
  transcript records (`child-transcript.ts:249-259`) — the stderr pump is a separate task
- **Previous state, kept as history (2026-09-16):** the ARTIFACT half had shipped, the WRITER half had not.
- ~~**The ARTIFACT half shipped; the WRITER half did not.**~~ `ArtifactPaths` now carries a fifth field — `artifacts.rs:75` `pub transcript_path: PathBuf`, documented as pi's `transcriptPath` and minted at `artifacts.rs:310` as `<base>_transcript.jsonl` — so **the "four fields (input/output/jsonl/metadata)" claim below is false at HEAD** and the field is consumed at `exec/mod.rs:580-582` and rendered by `tui/fleet.rs:916`/`:1162`. **What is still open is the thing the row is named for:** nothing writes that file live. `spawn/mod.rs:1144` states in-tree that there is no `ChildTranscriptWriter` port and that the lines go to `tracing` at debug level; `background/runner_main/status.rs:593` and `executor.rs:1218` both publish `transcript_path: None`; and `tui/fleet.rs:1144-1148` carries the explicit note "When a transcript writer lands, switch this to `paths.transcript_path`". **Do not round this to closed** — the FleetView transcript pane for a RUNNING child still has nothing to read
- upstream: `src/shared/child-transcript.ts:102` (`createChildTranscriptWriter`, per-record `fs.appendFileSync` at `:133`), created at `runs/background/subagent-runner.ts:1200-1201`; the field is the **fourth** `ArtifactPaths` member (`src/shared/types.ts:1048`, interface opens `:1044`); reported by `runs/background/run-status.ts:128`. First tag **v0.33.0**
- cyrup: `crates/cyrup-ext-subagents/src/artifacts.rs:61-70` — `ArtifactPaths` has four fields (input/output/jsonl/metadata) and `:58` says so; the substitute `.jsonl` is written only after the run settles (`extension.rs:4925-4928` foreground, `background/runner_main.rs:2611-2614` background). A live NDJSON stream exists but goes elsewhere: `exec/mod.rs:2113-2118` writes `<cwd>/.cyrup-subagent-scratch/attempt-N.jsonl`
- observable: the FleetView transcript pane for a RUNNING foreground child points at `paths.jsonl_path` (`tui/fleet.rs:1041-1058`), a file that does not exist until the child finishes, so it renders empty where upstream's fills in real time; `status`/`run-status` never print a `Transcript:` line.

**~~PB-13 · Chain-run artifacts default to the temp root, not the project~~** — ~~*small*~~ **CLOSED**
- **CLOSED, re-verified in code 2026-09-16 at `cc7818b`.** `artifacts.rs:274` resolves `ArtifactDirPreference::Project => project_chain_runs_dir(project_cwd)` inside `resolve_chain_runs_dir`, and `artifacts.rs:266` records this row's own history in-tree: *"…unconditionally used the temp root, so `project_chain_runs_dir` had zero references"*. Pinned at `artifacts.rs:704-712`. The `artifactDir` preference key this row said to land with (area 09 `SUBA-048`) is the same resolver and landed with it. **The body below is the original filing and is kept as history.**
- upstream: `runs/foreground/subagent-executor.ts:2022` @v0.34.0 (`chainDir: params.chainDir ?? getProjectChainRunsDir(effectiveCwd)`), helper `src/shared/artifacts.ts:16`. At v0.43.0 the same slot is `subagent-executor.ts:2623` via `getChainRunsDir`, whose "project" default still resolves to `getProjectChainRunsDir` (`shared/artifacts.ts:141-143`)
- cyrup: `artifacts.rs:146` (`project_chain_runs_dir`) has **zero references of any kind**; the live resolver `resolve_chain_dir` (`extension.rs:6539`) falls back to `chain_runs_dir(cwd)` = `temp_root_dir()/chain-runs/<cwd_key>` (`artifacts.rs:164-166`)
- observable: a chain run's artifacts land under `$TMPDIR/.../chain-runs/<cwd_key>/<runId>` instead of `<cwd>/.cyrup-subagents/chain-runs/<runId>` — invisible to the project, not committable, swept by OS tmp cleanup. The `[CYRUP-DELTA]` at `extension.rs:6536-6538` documents only the added per-run subdirectory and is silent on the root change. **Land with area 09 `SUBA-048`** (the `artifactDir` preference key, which is the same resolver and makes `project` the correct default for both).

**~~PB-14 · The "skills not found" warning is unported on BOTH surfaces~~** — ~~*small*~~ **CLOSED 2026-09-23 at `f978396`**
> **CLOSED on both surfaces.**
> * **Run side.** `SingleResult.skills_warning` (`exec/run_result.rs:99`, pi `shared/types.ts:1282`). It is computed in `LadderSetup` (`exec/mod.rs:1168-1205`) from the same `resolve_skills_with_fallback` resolution that feeds the orchestration-skill hard failure, and is set only after that failure's early return, so a missing orchestration skill still fails with exit 1 and carries no warning. `run_sync` copies it onto the result (`exec/mod.rs:725`). The value is upstream's text, `Skills not found: <a, b>` (`execution.ts:1902` @v0.68.0). It is persisted to `_meta.json` as `skillsWarning` (`artifacts.rs:617-621`; the old "`SingleResult` does not carry" confession is gone) and rendered as a `Warning:` line under the settled row (`tui/events.rs:736-740`, pi `tui/render.ts:3484-3486`). Pinned by `run_with_a_typo_skill_sets_skills_warning`, `orchestration_skill_still_hard_fails_and_carries_no_warning`, `metadata_carries_skills_warning` and `inline_result_renders_the_skills_warning_line`.
> * **Management side.** `discovery/management/handlers.rs:667` (`management_skills_warning`, pi `skillsWarning(cwd, agent)` `agent-management.ts:221-230`) appends `Warning: skills not found: <names>.` after the headline on `create`, and on `update` only when the patch touched `skills` (pi `:1240-1243`). Pinned by `create_with_unknown_skill_warns_after_the_headline` and `update_without_skills_in_the_patch_does_not_warn`. `[CYRUP-DELTA]`: the discovery config carries no request cwd, so the project root stands in for pi's `ctx.cwd`. There is no `skillPath` term, because cyrup has no per-agent skill path.
> * **The body below is the original filing and is kept as history.**
- **Re-greped 2026-09-22 at `14e6c56`:** `grep -rn 'skills_warning\|skillsWarning' crates/cyrup-ext-subagents/src/` returns exactly **one** hit and it is a confession, not a port — `artifacts.rs:544` documents `skillsWarning` as one of the fields *"which `SingleResult` does not carry in this crate"*. Both surfaces stay unported. **One sub-claim below has gone false and is corrected in place**: the stale deferral note that said the skills subsystem is "entirely absent today" is gone (`grep -rn 'entirely absent today' crates/cyrup-ext-subagents/src/` → 0), and `discovery/management.rs` no longer exists as a monolith
- upstream, run side: `runs/foreground/execution.ts:1112` @v0.34.0 — `skillsWarning: missingSkills.length > 0 ? …` declared on the shared result shape at `:179` (v0.43.0: `execution.ts:1524`)
- upstream, management side: `agents/agent-management.ts:773` and `:823` @v0.34.0 call `skillsWarning(ctx.cwd, …)`, helper `:190` (v0.43.0: `:971`, `:1023`, helper `:206`)
- cyrup: `exec/mod.rs:1155` calls `resolve_skills_with_fallback` and reads `resolution.missing` at `:1167` **for exactly one purpose** — pi's orchestration-skill hard failure (`execution.ts:938-946`) — then discards it; `SingleResult` has no `skills_warning` field and `artifacts.rs:544` documents omitting it. On the management side `discovery/skills.rs:152` (`resolve_skills`) still has zero callers outside its own module — production reaches the fallback form at `:195` instead *(citations refreshed 2026-09-22; the old `exec/mod.rs:3190-3193`, `artifacts.rs:427`, `discovery/skills.rs:149` and `discovery/management.rs:1276-1277` are all dead)*
- observable: `subagent({action:"create", config:{skills:"typo"}})` reports success with no warning, **and** a run with the same typo produces no warning either. (The `Skills not found:` string at `exec/mod.rs:1172` is a different thing: the hard failure for a missing *orchestration* skill, exit 1.)

**~~PB-31 · `requireReadTool` unported — a skill-carrying agent is told to `read` a skill it has no `read` tool for~~** — ~~*high*~~ **CLOSED** · = area 09 `SUBA-014`, closed there 2026-08-14 (sweep 1)
- **CLOSED, re-verified in code 2026-09-16 at `cc7818b`.** The head-injection lives at `exec/tool_surface.rs:466` and `:484`, driven by the `require_read_tool: bool` parameter threaded at `:384`, `:392` and `:419`; the seam is `exec::build_attempt_spawn_plan_with_read_requirement` and the 7-arg `build_attempt_spawn_plan` survives as pi's `requireReadTool: undefined` form. The `exec/mod.rs:1463-1491` citation below is dead (that allowlist builder moved into `exec/tool_surface.rs`). **The body below is the original filing and is kept as history.**
- upstream: `src/runs/shared/pi-args.ts:355-372` @v0.43.0 — `requireReadTool` head-injects `read` into the allowlist under `requireReadTool && requestedBuiltinTools.length > 0 && !includes("read")`, with **seven** live setters, all deriving it from `Boolean(resolvedSkills.length)`
- cyrup: `exec/mod.rs:1463-1491` builds the tool allowlist with no `read` head-injection, while `discovery/skills.rs:273` tells the child to "use the read tool to load a skill's file"
- observable: an agent with an explicit `tools:` list plus any resolved skill silently cannot load it. The child is instructed to use a tool it does not have and the failure surfaces as a model apology rather than a config error.

**~~VL-S1 · No capability ceiling on child tools/agents/extensions~~** — ~~*medium*~~ **CLOSED** · id retained · area 09 `SUBA-021`, closed there 2026-08-15 (sweep 10)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `exec/capability_ceiling.rs` exists and `CAPABILITY_CEILING_ENV = "CYRUP_SUBAGENT_CAPABILITY_CEILING_V1"` is declared at `:68` and **written into the child env** at `exec/spawn_plan.rs:1221` *(was `:557`)*, with the survive-the-re-exec guard at `:1493`. The `exec/mod.rs:1428` comment this row quotes ("no capability ceiling in this port") no longer exists. A second consumer landed with the SCOPE sequence: `background/scheduled_runs/ceiling_gate.rs:37-39` makes the ceiling a precondition for persisting a schedule. **The body below is the original filing and is kept as history.**
`src/runs/shared/capability-ceiling.ts:5`, `:95`, `:106` (209 lines) — present at **both** v0.43.0 and v0.47.1; env write and the `MCP_DIRECT_TOOLS="__none__"` forcing at `src/runs/shared/pi-args.ts:741-756` — vs `exec/mod.rs:1428`, the single workspace-wide mention, a comment reading "no capability ceiling in this port". `CAPABILITY_CEILING_V1` is one of the six upstream child env names with no cyrup counterpart (area 09 sweep 1). **Observable**: a grandchild inherits its parent's full tool/extension surface; upstream clamps monotonically and stamps the ceiling so the child cannot re-widen.

**~~VL-S2 · `workflowScript` runtime (and `chatProgress`)~~** — ~~*large*~~ **CLOSED** · id retained
- **CLOSED, re-verified in code 2026-09-16 at `cc7818b`.** `"workflowScript"` has **56** occurrences across the crate; the runtime is `workflows/scripted/` (`engine.rs` alone carries `WORKFLOW_DEFAULT_TIMEOUT_MS` at `:2178`) and the `WorkflowStateStore` trait is implemented by `missions/workflow_state.rs:298`, constructed in production at `extension/tool/routing.rs:571`. The `extension.rs:5327-5338` claim this row quotes ("the identifier appears nowhere in this crate") is gone with the file. **This closure is what discharges UW-8** (§2), exactly as UW-8's own text predicted. **NOT closed by it and still owed:** VL-S12's reverse-lag half — the four v0.41.0-deleted slash commands are still registered, see that entry. **The body below is the original filing and is kept as history.**
`src/workflows/scripted-workflow.ts:311` (`runWorkflowScript`, 502 lines) plus `src/workflows/chat-progress.ts` (140); tool params `src/extension/schemas.ts:317`, `:318` — vs `extension.rs:5327-5338` ("the identifier appears nowhere in this crate") and `missions/workflow_state.rs:26-30`. **Observable**: the model cannot express a dynamic workflow (`runs.run`/`runs.all`/`emit`/`state.get`/`state.set`); conversely cyrup still exposes the `tasks`/`chain`/`concurrency`/`chainDir` shapes v0.41.0 removed (that half is VL-S12). **Area 09's blind spot 2 is a warning about this entry specifically:** upstream deleted the entire task/chain execution surface at v0.41.0 and replaced it with `workflowScript`; at v0.43.0 the top-level schema has **no `task` key at all** and the whole model-facing tool description is workflowScript-centric. cyrup implements the v0.34.0-era surface. **This is not one item — it is a different execution model**, and its per-behaviour consequences (`runs.ref`, `emit`, per-child gates, `prompts.render`, `chatProgress`, retained-child `resume`, `children.list`) have never been decomposed by any pass. Treat area 09's count as a floor "by a wide margin" because of it.

**~~VL-S3 · Session lease — two runners can own one session file~~** — ~~*medium*~~ **CLOSED 2026-09-20** · id retained, class corrected (v0.35.0)
- **CLOSED.** `background/session_lease/` (7 modules, 2,343 LOC) ports `src/runs/shared/session-lease.ts` (293 L @v0.68.0) whole: the canonical session id (sha256 of the session file's REALPATH, so a symlink and its target take ONE lease), the strict `parse_owner` with upstream's two cross-field rules (`:141-142`), the four staleness rungs (`:174-181`), and `create_lease_directory`'s rename-claim (`acquire.rs:208`, pi `:183-200`) — a `<leaseDir>.candidate-<token>` at 0700 with `owner.json` at 0600, renamed onto the lease dir, because POSIX `rename(2)` refuses to rename onto a non-empty directory and that refusal IS the claim. The per-owner tombstone (`:278-288`) is reproduced with its reason: named after the token of the record the contender READ, so N contenders that saw one stale owner compute the SAME occupied destination and exactly one wins.
- **Reachable, and the claim is demonstrated by a real process.** The ORCHESTRATOR builds the request on the revival path only (`extension/executor/control.rs:487`, pi `config.revivalLease`); the RUNNER acquires it after config load and before the initial status (`background/runner_main/entry.rs:118`, pi `subagent-runner.ts:5241-5242`), holds it for the whole run, updates its writer state at each dispatch, and releases it in `run_with`'s tail — AFTER the candidate write and BEFORE the proof is finalized (`entry.rs:273`, `:289`, `:299`, pi `:5168-5190` then `:5280-5292`), which is the order that makes `markProcessTerminalCandidateLeaseRelease`'s token guard (`process-terminal.ts:136`) match instead of decline. **Both halves are pinned through production:** the orchestrator's by `extension/executor/background.rs`'s `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`, which drives a real `resume` and reads `revivalLease` back off the revived run's own `runner-config.json` (setting it to `None` there fails that test), and the runner's by the lease integration test below. A refused acquire does not `?` past the terminal write: it funnels to `finish_run` with `RunState::Failed` and upstream's conflict sentence. The integration test `crates/cyrup-it/tests/subagents/session_lease_revival_integration.rs` (572 L, `--features it`) refuses a SECOND revival of one session file against an incumbent whose pid is genuinely alive on this genuine hostname, and reads a live child's pid back out of `owner.json` mid-run.
- **The pid-reuse hole this row's sibling named is closed underneath it.** `processDemonstrablyGone` (`:162-172`) is `background::reconcile::check_pid_identity_with` (`reconcile.rs:186`): `kill(pid, 0)` first, then, on `Alive` with a recorded `ProcessStartIdentity`, a re-read of `/proc/<pid>/stat` field 20 — a RECYCLED pid answers `Dead`. Absence and `Unknown` never downgrade to `Dead`, which is the counter-rung an over-eager identity check would break.
- **What is NOT closed by this, and is a different row:** `03-cyrup-session.md`'s SQLite-backend row keeps its own lease gap — that is `cyrup-session`'s JSONL store, not pi-subagents' async session files. Its cross-reference to this row is corrected there.
- **The body below is the original filing and is kept as history.**
`src/runs/shared/session-lease.ts:9`, `:59`, `:208` (299 lines); acquired `subagent-runner.ts:4618`, released `:4648`; present at v0.47.1 — vs zero lease machinery anywhere in `crates/cyrup-ext-subagents/src` (area 09 `SUBA-023` re-confirms zero-hit). **Observable**: nothing prevents two runner processes writing one async session file concurrently, and there is no dead-owner reclaim on the next revival.

**~~VL-S4 · Process-terminal record — a killed runner leaves an ambiguous run~~** — ~~*medium*~~ **CLOSED 2026-09-20** · id retained, class corrected (v0.37.0)
- **CLOSED.** `background/process_terminal/` (8 modules, 3,569 LOC) ports `src/runs/background/process-terminal.ts` (310 L @v0.68.0): the candidate (`<run_dir>/process-terminal-candidate.json`, 0600 — it can carry a session path), the proof sidecar (`<run_dir>/process-terminal.json`), the `status.processTerminal` / per-step overlays, and `finalizeProcessTerminal`'s whole decision ladder in upstream's order (`finalize.rs:366`, pi `:243-310`) — one distinguishable reason per rung, each pinned by a test, `observed` included. Every `validateProof` refusal sentence is byte-identical to upstream's. **All TEN `ProcessTerminalReason` arms have a producer.** The eight the ladder reaches are the ladder's; the remaining two are the STALE-RUN RECONCILER's, exactly as upstream splits them — `observer-unavailable` on the result-file repair (`background/reconcile.rs::repair_from_result`, pi `stale-run-reconciler.ts:186-188`) and `stale-repair` on the pid-probe failure synthesis (`synthesize_failure`, pi `:248-250`), both behind upstream's own gate of "only over an absent or `pending` overlay", so a repair pass can never overwrite a verdict somebody already reached.
- **The identity is real and cannot collide.** `RunnerProcessInstanceId` (`process_terminal/id.rs`) is a v4 uuid on `RunId::new()`'s idiom, minted by the ORCHESTRATOR before the launch config is written (`extension/executor/background.rs:665`, pi `async-execution.ts:707`) and carried to four places: `RunnerConfig`, the candidate + `pending` sidecar (`background.rs:880`, written BEFORE the spawn because cyrup has no startup barrier — the spawn IS the authorization), the capacity owner (`mark_started`), and `status.processTerminal`.
- **A crash is now DISTINGUISHABLE from a clean close, which is this row's whole observable.** The runner finalizes its own close (`runner_main/entry.rs:299`, after `finish_run`) because cyrup's orchestrator detaches its runner and drops the handle unawaited (R-SA-078) — so a runner that was killed never reaches that line, its sidecar stays `pending` forever, and `status` reports that instead of a reconciled guess. A clean close reads `observed` with `instances: [runner, …writers]`.
- **cyrup's writers are REAL, where upstream's are structurally empty.** pi `subagent-runner.ts:5169` writes `writers[i] = []` for every step because *"children run inside this process"*; every cyrup step spawns a real OS child in its own process group, so the candidate carries one `PiWriterProcessInstanceExit` per launched child with a `ProcessTreeTerminal` saying whether its whole group was observed torn down. The `process-tree-unverified` and `writer-close-unverified` rungs are therefore live verdicts about real processes rather than arms upstream's own shape can never reach.
- **The substitution this row warned about is resolved, not deleted.** `active_async_capacity::inspect::runner_release_verdict` now reads the real proof BEFORE the pid ladder (`inspect.rs:299`, pi `active-async-capacity.ts:227-235`; it is not the ladder's first rung — upstream's five identity/terminality guards and the `not-started` carve-out still answer ahead of it) and keeps the pid ladder BENEATH it — because a runner killed before it could finalize writes no proof, ever, and deleting the fallback would retain such a slot forever. The fallback is no longer a substitute for the proof; it is the answer to a different question, asked only when the first has none. §D3 and `key.rs` are rewritten to say exactly that.
- **Consumers:** the capacity release rung and the per-child workflow loop (`inspect.rs:296`, `:592`); the active-run index's staleness rung, which now releases a marker on an `observed` proof instead of waiting out 24 hours (`active_run_index.rs`, pi `async-status.ts:575`); `debug.run`, which prints upstream's real trio — `Process terminal file:` (`run-status.ts:95`) plus the `Status`/`Sidecar` pair (`:102-103`) — over a file the launch really wrote; and the RPC bridge, whose `capabilities.processTerminalProof` and `events.processTerminal` (`rpc.ts:458`, `:466`) are paid for by `background::watch::ProcessTerminalAnnouncingCompletionObserver` (pi `emitProcessTerminalEvent`, `async-execution.ts:666-672`).
- **The `TerminationOutcome` sentence this row carried was ALREADY FALSE and is deleted rather than carried into the closure.** `spawn/signal.rs:112-127` has carried `signal_name: Option<&'static str>` since sweep 1; `signal_name(i32)` is `:136-158` and `signal_name_of(&ExitStatus)` is `:162-170`, literally `status.signal().and_then(signal_name)`. Area 09 `SUBA-023:617` recorded the observation as REFUTED before this pass began; the sentence below survived in this row only by being re-copied. The SIGNAL-NAME half of that file — `TerminationOutcome` at `:112-127`, `signal_name` at `:136-158`, `signal_name_of` at `:162-170` — needed nothing and got nothing. **The file itself is NOT untouched by this closure**: it gained `PROCESS_GROUP_VERIFY_BUDGET`, `PROCESS_GROUP_VERIFY_INTERVAL`, `pub enum ProcessGroupTerminal`, `pub async fn verify_process_group_terminated` and two tests (+207 lines), consumed on the production child-close path at `exec/attempt_runner.rs:652`. That call is where every candidate writer's `ProcessTreeTerminal` comes from, so it is the input to the `process-tree-unverified` rung above, and `09a-cyrup-ext-subagents-v0.57-drift.md:3414` credits it by name as the reason the owned-process-tree half landed ABOVE upstream. A teardown regression bisected to this closure must start in `spawn/signal.rs`.
- **The body below is the original filing and is kept as history**, including its "cyrup has neither input" consequence note — which was true when filed and is what the rewritten §D3 replaces.
`src/runs/background/process-terminal.ts:52`, `:163`, `:216` (280 lines); present at v0.47.1 — vs zero hits crate-wide; run state comes from `background/run_status.rs` and `background/reconcile.rs`. (The `TerminationOutcome` sentence that stood here was refuted by `SUBA-023` and is deleted, not reworded — see the closure above.) **Observable**: when a runner dies without writing a result, upstream still reports a definite terminal cause; cyrup can only report the reconciled "stale" guess, so `status` cannot distinguish a crash from a slow start.

**~~VL-S5 · Revival does not restore the child's effective config~~** — ~~*small*~~ **CLOSED 2026-09-19**
- **CLOSED.** `background/recovery_descriptor.rs` writes `<run_dir>/recovery-descriptor.json` from
  the RESOLVED launch inside `spawn_background_steps` (`extension/executor/background.rs`), gated
  to single async runs as upstream is, **failing the launch** on a write error (pi
  `async-execution.ts:2053`) and written private (`atomic.rs:148` `write_private_atomic_json`,
  0600 — the file carries a system prompt). `revive_from_transcript`
  (`extension/executor/control.rs`) reads it back: a missing descriptor **refuses** the revive with
  pi's sentence (`subagent-executor.ts:2059-2061`), an agent-mismatched one refuses
  (`async-resume.ts:566`), and the carried fields land on the revived `SingleStepSpec` /
  `BackgroundStepsSpec` and the persona overlay, so `model: None, tools: None, …` is no longer what
  a resumed run gets
- **This row's "*small*" and its three-field framing were both wrong.** Upstream's
  `SteeringRecoveryDescriptor` (`src/shared/types.ts:805-864` @v0.68.0) has **54** field
  declarations — the launch contract, not `model`/`tools`/`toolBudget`. Of those, **13 have no
  cyrup concept** (`fast`, `mcpDirectTools`, `mutationTools`, `inheritGlobalContext`, `skillPath`,
  `intercomBridge`, `maxOutput`, `launchResolvedExtensions`, `modelResponseAliases`,
  `extensionBindings`, `requiredExtensions`, `agentContract`, `baseRef`) and are named per field in
  the source rather than dropped *(2026-09-23: `fast`, `mutationTools` and `inheritGlobalContext`
  are now carried — SUBA-101/102/103 in `09a`; ten remain)*; `runFanoutBudget` and `lane` are carried as `Option` and are
  always `None` on a single run (no async launch allocates a fanout ledger; lanes attach to
  parallel groups). Three run-level fields cyrup has and pi's descriptor does not —
  `turnBudget`, `usageBudget`, `permissionRules` — are an explicit additive `[CYRUP-DELTA]`
- **Two of the restored fields are capability constraints** (`tools`/`excludeTools`,
  `maxSubagentDepth`), so before this the fallback WIDENED: a child launched with a narrowed tool
  set resumed with the agent file's full set. And PR #142's RPC bridge exposes `resume`, so the
  path was reachable programmatically, not only by a human typing a verb
- `has_resumable_contract` (`async_retention/scan.rs`) now finds real files; its no-writer
  `[CYRUP-DELTA]` is deleted. `RunDir::recovery_descriptor()` joins `status()`/`events()`/`handoff()`
- **Visible behaviour change, stated rather than hidden:** a terminal async run launched BEFORE this
  feature has no descriptor and will not revive via the old bare-agent path. That is upstream's
  stance and no grace path was added
- **Pinned** by a production round trip: N explicit overrides in, N asserted out on the revived
  spec, with a per-field writer-drop mutation table recorded in `.flux/done/RECOVERY_DESCRIPTOR.md`
- upstream: `src/shared/types.ts:805` (type), `src/runs/background/async-execution.ts:1993-2053`
  (write), `src/runs/background/async-resume.ts:310-440` and `:560-632` (read + overlay),
  `src/runs/background/async-retention.ts:191-203` (retention). This row's `:1358`/`:1401`/`:276`/
  `:501-524` cite v0.35.0 and are dead
- ~~**Re-greped 2026-09-16, kept as history and now WRONG:** *"`background/async_retention/scan.rs:56` defines `RECOVERY_DESCRIPTOR_FILE` as a reader … `:381` carries the explicit `[CYRUP-DELTA] no cyrup writer produces recovery-descriptor.json today` … the read half now exists and the write half still does not."*~~ — **false at `14e6c56`.** The writer landed 2026-09-19 (`extension/executor/background.rs:808`), and with it both quoted lines were deleted: `grep -n 'RECOVERY_DESCRIPTOR_FILE\|CYRUP-DELTA' background/async_retention/scan.rs` now shows `:56` carrying an unrelated discovery-worker note and nothing at `:381`. Struck rather than deleted so the next reader does not re-derive the contradiction. The `extension.rs:4269-4285` citation below is dead
`runs/background/async-execution.ts:1358` builds a `SteeringRecoveryDescriptor` and `:1401` persists it as `recovery-descriptor.json`; `async-resume.ts:276` reads it back and `:501-524` re-applies model, fallbackModels, thinking, tools, extensions, mcpDirectTools, systemPrompt, skills, completionGuard, memory, output, toolBudget and maxSubagentDepth — vs cyrup, which writes no descriptor and rebuilds the revived step with `model: None, tools: None, extensions: None` at `extension.rs:4269-4285`. **Observable**: a run launched with per-call `model`/`tools`/`toolBudget` overrides revives without them. *(Revival ITSELF is ported and works — `ResumeOutcome::RespawnFromTranscript` at `background/control.rs:1214` → `revive_from_transcript` at `extension.rs:4232`.)*

**~~VL-S6 · Herdr inspector subsystem~~** — ~~*large*~~ **CLOSED 2026-09-21**
- **CLOSED.** Both upstream paths this row names are ported, and the feature works for a human and
  for an agent on the same merge.
- **`crates/cyrup-herdr`** (35 files) is the workspace's ONE herdr client: NDJSON over a Unix
  socket (named pipe on Windows), the typed method surface checked against herdr's own
  checked-in JSON Schema (`tmp/herdr/docs/next/api/herdr-api.schema.json`), `events.subscribe`
  with herdr's documented no-gap `bootstrap()`, reconnect, and a CLI fallback.
  `cyrup-intercom`'s `HerdrLauncher` was MIGRATED onto it in the same PR — its public behaviour
  unchanged, its existing tests unmodified — so the workspace has one herdr transport, not two.
- **The seven verbs** — `inspector.{open,command,status,close}` and `project.{open,status,close}`
  — are advertised in `SUBAGENT_ACTIONS` at pi's own index and dispatched through the typed
  `InspectorAction::from_wire` / `ProjectPaneAction::from_wire` seams
  (`extension/tool/routing.rs:1537`, `:1606`), with a test driving all seven through the real
  `cyrup_core::Tool::execute` and asserting each lands on its own arm rather than the
  `Unknown action:` fallback.
- **The status bridge** (`src/herdr/`) is `src/integrations/herdr-status.ts`'s port and more:
  cyrup reports SEMANTIC state through `pane.report_agent`, which pi never does (`git grep
  'report_agent\|report-agent' v0.68.0 -- src` is empty — pi reports display metadata only and
  rolls up as `unknown`). The `blocked` signal is the process-wide `HumanInteractionLock` the
  permission dialog and MCP's dialog owner both acquire, so a pane goes `blocked` on a real
  prompt. Raw prompts can never reach pane metadata, pinned on the PRODUCTION producer.
- **The `H` key answers.** `tui/fleet.rs`'s `has_inspect` seam and its
  "Herdr inspector controls are unavailable in this context." refusal were already in the tree;
  only the implementation behind them was missing. It is there now, and the deltas saying it
  could not be wired are DELETED.
- **With no herdr and no ghostty installed** — this container, and any CI box — every surface
  degrades with upstream's own sentence and no pretend success: `inspector.command` still returns
  the full launch string (upstream returns before reading its plugin list), `inspector.open`
  refuses byte-exactly and writes nothing.
- **Verification:** three QA lenses; two BLOCKING defects found and closed — the bridge was
  polling a `HumanWaitGate` no production code raises, so the pane never reported `blocked` at
  all, and the privacy rule's only test drove a function with zero production callers. An
  independent verifier re-proved both by tracing the tree. 23 gutting mutations, all RED.
  Gates: fmt clean; clippy `--workspace --all-targets` clean; `nextest --workspace` **11 032
  passed**; `cyrup-it` **590 passed**.
- **[CYRUP-EXCEEDS-UPSTREAM]** `agent.view.set`/`agent.view.clear` are sent, which pi never does:
  eleven types in `cyrup-herdr`'s `schema/agents.rs` mirroring `tmp/herdr/src/api/schema/agents.rs:52-162`,
  both `Method` variants, and a production caller in the status bridge's drain that installs the
  projection before its first report and clears it after release, gated by the same env the
  bridge's own gate reads. It is a SORT WITH NO FILTER — attention desc, then `state_change_seq`
  desc — deliberately: there is exactly ONE view server-wide
  (`tmp/herdr/src/app/api/agent_view.rs:88-89`) and it governs the sidebar, the mobile list,
  mouse targets, indexed focus and next/previous navigation (`socket-api.mdx:421-424`), so a
  cyrup-scoped FILTER would hide other agents' panes from the human. Sorting reorders; filtering
  would conceal.
- The body that follows is the original filing, kept as history.

- ~~**Re-measured 2026-09-16 against the verb set rather than a line citation:** *"of the 57 verbs in `pi-subagents` v0.68.0 `shared/types.ts:2801`, cyrup's 42-verb list (`extension/tool/text.rs:215`) is missing all seven this row owns"*~~ — **stale at `14e6c56`, and it is the same "42-verb list" figure `VL-S13` corrects**: the list is **59** (`extension/tool/text.rs:234`) and all seven `inspector.*`/`project.*` verbs are in it — which is what closed this row. The `extension.rs:6557` / `extension.rs:9863` citations below are dead; `tui/fleet.rs`'s "Herdr inspector controls are unavailable in this context." refusal was not re-resolved to a current line this pass
`src/inspectors/herdr/actions.ts:15` (`HERDR_INSPECTOR_ACTIONS`) and `:158`, plus `client.ts` (130), `inspector-runner.ts` (141), `project-panes.ts` (154), `src/integrations/herdr-status.ts` (330) — vs `tui/fleet.rs:1654` ("Herdr inspector controls are unavailable in this context."), the hard-coded `false` at `extension.rs:9863`, and no `inspector.*` verb in the enum at `extension.rs:6557`. **Observable**: the FleetView's advertised `H` key (footer at `tui/fleet.rs:2025`) always answers "unavailable".

**~~VL-S7 · Authority policy (confirm/forbid gates)~~** — ~~*medium*~~ **CLOSED** · id retained · area 09 `SUBA-064`, closed there 2026-08-14 (sweep 1)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `registration/authority.rs` is the port and names the upstream file in its own header; `validate_authority_policy` exists; the gate is consulted from the live `stop`/`steer` path, from `schedule.create` (`extension/tool/routing.rs:1268-1275` → `AuthorityAction::for_tool_action`, mapping at `registration/authority.rs:113`), and — **since LANES_2 and `VL-S6`** — from `worktree.discard` (`:120`), `inspector.open` (`:136`) and `project.open` (`:137`). **The scope caveat this row used to carry is now FALSE and is struck:** ~~"`worktree.discard` and `worktree.cleanup` are among the seventeen verbs cyrup's action list is still missing, so those arms have nothing to attach to"~~ — `SUBAGENT_ACTIONS` is **59** verbs (`extension/tool/text.rs:234`) and both `worktree.*` verbs are in it. `worktree.discard` arrived already gated; `worktree.cleanup` returns `None` **deliberately**, because upstream gates it on nothing and it is plan-only (`authority.rs:118-119`). **The "becomes critical the day `worktree.discard` lands" clause is therefore spent — that day came and the gate was already there**, which is why this closure holds at *medium* rather than reopening. **The body below is the original filing and is kept as history.**
`src/policy/authority.ts:1-8` (`AUTHORITY_ACTIONS`), `:14-21` (defaults — discardWorktree/destructiveCleanup/spawnBudgetGrant default to `confirm`), `:23`, `:30`; consumed by `inspectors/herdr/actions.ts:205-206` (`allowSteer`/`allowStop`) and validated at `src/extension/config.ts:26` — vs `extension.rs:7574` (a doc line naming upstream's dispatch arm) and no `authorityPolicy` config key anywhere. **Area 09 sharpened this**: the `stop`/`steer` gate it drives is **live-reachable** in cyrup today, so the missing policy is not merely unconfigurable — it is an unguarded live path. Scope caveat: upstream's `discardWorktree` gate hangs off a `worktree.discard` action cyrup does not have, so that arm has nothing to attach to yet. **`SUBA-064` stays medium only because of that caveat — it becomes critical the day `worktree.discard` or `destructiveCleanup` lands**, and its Fix now carries that as a hard prerequisite.

**~~VL-S8 · Wait tool is still `wait`~~** — ~~*medium*~~ **CLOSED 2026-09-21** · id retained, class corrected (v0.35.0/v0.41.0)
- **CLOSED.** `extension/wait_tool.rs`'s `WAIT_TOOL_NAME` is `"bg_wait"`, and that is the name the
  registered tool object carries — an integration test resolves the tool out of the real extension
  and asserts `name() == "bg_wait"` rather than reading the const, so the registration and the
  const cannot drift apart
- **THE ROW'S REAL VALUE WAS NOT THE RENAME.** It was a gating bug the rename exposed:
  `watchdog/permission_arbiter.rs`'s `INTERNAL_TOOLS` — the set a permission policy may not gate,
  because gating one of its members strands a child that cannot then report back — carried the
  literal `"subagent_wait"`, **a name this crate has never registered at any point in its
  history.** So the protection was aimed at a phantom: a parent shipping `{"bg_wait": "deny"}`
  was ACCEPTED by `validate_permission_rules` and could strand a launched child. The set now
  holds `crate::extension::wait_tool::WAIT_TOOL_NAME` itself rather than a copy of it, so the
  two cannot diverge again, and the deny rule is refused with upstream's own sentence
- **NO COMPAT ALIAS SHIPPED, AND NONE SHOULD.** `00-residual-ledger.md`'s prescription for this
  row was *"one const plus a compat alias"*. Upstream registers exactly one tool:
  `wait-tool.ts:37-44` @v0.68.0 builds a single `primaryTool` with `name: "bg_wait"` (`:38`) and
  calls `pi.registerTool(primaryTool)` once (`:44`). There is no second registration and no
  aliasing anywhere in that file. An alias here would be a cyrup invention that widens the tool
  surface a child sees; the ledger row is amended so the next reader does not add one
- **Three other sites moved with the name, and each is now pinned by a literal rather than by the
  const**, so a revert is caught in more than one place: `background/wait.rs`'s no-manager refusal
  (*"…can only use blocking bg_wait calls."*, byte-identical to `subagent-wait.ts:706`),
  `SUBAGENT_SAFETY_GUIDANCE` (pin 1427 → 1430) and `COMPACT_SUBAGENT_TOOL_DESCRIPTION`
- **The evidence line below cited `wait-tool.ts:9` and `name: "subagent_wait"`.** Both are wrong at
  `v0.68.0`: `:9` is the `pi: ExtensionAPI` parameter of `registerWaitTool`, and the name literal
  is `bg_wait` on `:38`. `subagent_wait` was upstream's name at the tag this row was FIRST written
  against and has not been upstream's name for many tags. **The body below is the original filing
  and is kept as history.**
`src/runs/background/wait-tool.ts:9` (`name: "subagent_wait"`), backed by `subagent-wait.ts` (651), `wait-config.ts` (36) and `auto-drain.ts` (67) at v0.35.0 plus `wait-subscriptions.ts` (348 @ `7fe9dee1`; 253 at v0.41.0, the tag this row was first written against) — vs `extension/wait_tool.rs:16` (`WAIT_TOOL_NAME: &str = "wait"`; `extension.rs` no longer exists as a monolith, so the old `extension.rs:6704` citation is stale). **Still observable**: a child prompted by upstream's tool description calls `subagent_wait` — or, at `7fe9dee1`, `bg_wait` — and gets "unknown tool". **Closed since**: the `{id, nonBlocking:true}` wake subscription is ported as `background/wait_subscriptions/` (SCOPE_11) and armed from `background/wait.rs`'s own arming site, rendered by `extension/executor/status.rs`'s no-id branch, and reconciled as the FOURTH member of the completion watcher's composite observer; auto-drain at `agent_end` is ported as `background/auto_drain.rs` and driven from `extension/host/native_impl.rs`'s `AgentEnd` arm. Related residuals now filed in area 09: `SUBA-034` (event-bus wake) and `SUBA-031` (`wait` scoping) are both **CLOSED**. `SUBA-056` (durable completion replay) is **CLOSED** — `background/completion_replay/` is the port, and `collect_wait_completions`' third rung reads it; `wait_subscriptions`' own settle reads through the same three rungs. **What remains in this row is the tool RENAME and nothing else.**

**~~VL-S9 · `usageBudget`~~** — ~~*small*~~ **CLOSED** · id retained · area 09 `SUBA-021`, closed there 2026-08-15 (sweep 10)
- **CLOSED, re-verified in code 2026-09-16 at `cc7818b`.** `exec/usage_budget.rs` exists; `"usageBudget"` has **14** occurrences including the advertised schema key, and the resolved budget is carried onto the runner at `extension/executor/background.rs:620` (`usage_budget` on `RunnerConfig`, commented "SUBA-021 — the run-level usage budget the orchestrator validated, carried verbatim onto hop 2"). The "zero hits in the crate" claim below is false at HEAD. **The body below is the original filing and is kept as history.**
`src/runs/shared/usage-budget.ts:14`, `:44`, `:61` (65 lines) — present at **both** v0.43.0 and v0.47.1; tool param `src/extension/schemas.ts:330` — vs zero hits in the crate and no such key among the 45 schema properties. **Observable**: a run cannot be capped by cost/token spend.

**~~VL-S10 · Parallel worktree handoff manifests~~** — ~~*medium*~~ **CLOSED** · area 09 `SUBA-024`
- **CLOSED 2026-09-19.** `handoff/` (9 modules) writes the manifest the retention reader has been
  looking for since it landed: `background/async_retention/scan.rs`'s `has_unresolved_run_handoff`
  now finds real files, and the `[CYRUP-DELTA]` saying no cyrup writer produces one is gone. The
  writer is reached from `spawn/chain_graph.rs`'s `publish_worktree_handoff`, on every
  `worktree: true` group settle, in two phases around cleanup so the removal ledger is recorded
  with the group rather than guessed afterwards
- **Five verbs, not two, and they are ONE feature.** This row named `worktree.discard`/
  `worktree.cleanup`; `lane.status`/`lane.recordMerge`/`lane.recordSupersession` were filed
  separately inside the seventeen-verb list. All five dispatch through the same manifest upstream
  (`subagent-executor.ts:6270-6290` @v0.68.0). `SUBAGENT_ACTIONS` is 42 → 47
- **`VL-S7`'s authority arms now have something to attach to**, as this row predicted:
  `registration/authority.rs` maps `worktree.discard` → `discardWorktree` (Confirm by default) and
  the in-tree note saying "whoever lands `worktree.discard` … must wire them through" is discharged
- **`worktree: true` did not WORK before this.** It was advertised at
  `extension/tool/schema.rs:430` and returned *"worktree: true group requires
  `ChainRunContext::worktree_base_dir` to be configured"* in a default install. So this closed more
  than the convergence layer it was scoped as
- **A data-loss defect was introduced and closed inside this batch, and it is worth recording.**
  Giving the harvest a cleanup call made it force-remove worktrees holding uncommitted work, with
  nobody asked — upstream refuses at `worktree.ts:1197-1231`. The gate is now ported with all four
  interlocks (both probes; the capture row carries a real error; the manifest must RECORD the patch;
  the patch must still represent the worktree), `git branch -D` runs only after a successful
  removal, and both paths are pinned by mutation-proven tests
- upstream: `src/runs/shared/parallel-handoff.ts` (741 lines @v0.68.0),
  `src/runs/shared/worktree-cleanup-plan.ts` (869), `src/runs/shared/lane-metadata.ts` (126)
- **Re-greped this pass:** `grep -rn '"handoffPath"' crates/cyrup-ext-subagents/src/` returns **0** — the param is still unadvertised and there is still no manifest writer. Adjacent and also still missing: `worktree.discard` / `worktree.cleanup` are two of the seventeen verbs absent from cyrup's action list, which is why VL-S7's authority arms for them have nothing to attach to
`src/runs/shared/parallel-handoff.ts:74`, `:158`, `:162`, `:183` (238 lines), present at v0.47.1; `handoffPath` tool param at `src/extension/schemas.ts:274` — vs `spawn/parallel.rs` (no manifest writer) and three incidental mentions only. **Observable**: after a parallel run with `worktree: true` there is no handoff manifest, no `handoffPath` to hand preserved worktrees to a follow-up, and no `discardPreservedWorktrees` cleanup — the branches are left for the user to find by hand.

**~~VL-S11 · Three slash commands missing: `/subagents`, `/subagents-refine`, `/subagents-detach`~~** — ~~*medium*~~ **CLOSED 2026-09-21** · id retained, class corrected (v0.35.0/v0.43.0/v0.39.0) · area 09 `SUBA-026`
- **CLOSED, not narrowed.** All four remaining commands landed in the VL-S11/S12/S8 batch:
  `/subagents` (the admin surface), `/subagents-detach`, `/subagents-steer` and
  `/subagents-inspect-rpc`. `/subagents-refine` closed earlier with `VL-S13`
- **This row's own count was stale in both directions and is corrected here.** The "17 variants at
  `registration/slash_commands.rs:83-121`" note above was written against a table that has since
  moved and grown: `SlashCommandName` is now `:95-158` and `SLASH_COMMANDS` is `:228`, and the
  table is **18** — `Run`, `SubagentCost`, `SubagentsDoctor`, `SubagentsModels`,
  `SubagentsProfiles`, `SubagentsLoadProfile`, `SubagentsRefreshProviderModels`,
  `SubagentsGenerateProfiles`, `SubagentsCheckProfile`, `PromptWorkflow`, `SubagentsFleet`,
  `SubagentsStop`, `SubagentsGuide`, `SubagentsRefine`, `Subagents`, `SubagentsDetach`,
  `SubagentsSteer`, `SubagentsInspectRpc`. **The "17" was already stale before this batch** — the
  table held 18 at `2647f68`, the batch's own base. It still holds 18 after it, and that is
  arithmetic rather than coincidence: `VL-S12` deleted four (`Chain`, `Parallel`, `RunChain`,
  `ChainPrompts`) and `VL-S11` added four, in the same batch.
  `slash_commands_table_is_exactly_upstreams_eighteen_commands` now asserts the full ordered list,
  and `the_four_commands_upstream_deleted_at_v0_41_0_are_not_registered` asserts the length and
  each deleted name's absence, so neither the count nor the membership can drift again silently
- **`/subagents-detach` is the one that is not a registration over an existing capability.**
  Upstream's detach is cheap — pi's child is an in-process session object, so `detachForeground`
  snapshots a receipt and returns while the session's callbacks keep firing. cyrup's child is a
  REAL OS PROCESS owned by the `drive_foreground_run_sync` future, so the same move would kill the
  thing the feature exists to preserve. `run_foreground_impl` is split: the future is built from
  owned inputs, boxed `Pin<Box<dyn Future + Send>>` so it is movable, raced in a biased `select!`,
  and on an accepted detach the WHOLE VALUE moves into `tokio::spawn`. Proved by an OS-level
  `/proc/<pid>/stat` liveness probe taken immediately after the receipt returns, against a real
  scripted child, with a receipt budget deliberately far shorter than the child's own sleep
- **Both READER halves were dead from production before this**, which is what made the detached
  run unaddressable: `status`-by-id gained a second arm over `foreground_runs` (a detached run is
  by construction absent from `foreground_controls`), and `bg_wait` gained the whole
  detached-foreground candidate set. `WaitTool::execute` never chained
  `.with_detached_foreground(…)` and `DetachedForegroundRunsSource` had no implementation anywhere
  in the tree, so `active_detached_foreground_runs` always returned empty. Found by the
  integration test that asserts `bg_wait` BLOCKS — committed knowingly red and now green
- **`/subagents-steer`'s "no-id opens a selector" hypothesis is REFUTED, not unported.** Upstream's
  `/subagents-stop` opens `ctx.ui.custom(…)` on its no-id branch (`:1044-1047`);
  `/subagents-steer` does not — `:1065-1068` is `sendSlashText(pi, usage)` and nothing else
- **The wrong path this row shares with area 09 `SUBA-026`** — `src/tui/selector.ts` — is still
  wrong; the real file is `src/slash/selector.ts`. Noted rather than silently fixed, because the
  selector itself remains unported and `SUBA-026`'s UI half is what keeps that row open
- **Gates at closure** (`53f0c25`): fmt `--check` clean; clippy `--workspace --all-targets
  --features test-fixtures -- -D warnings` clean; `nextest --workspace` **11 074 passed**, 9
  skipped; `cyrup-it` **594 passed**. Five gutting mutations, all RED, each restored byte-for-byte
  — the table is in `.flux/done/SLASH_SURFACE_AND_WAIT_RENAME.md`. Two flakes seen under parallel
  load on the way there (the scheduled-runs armed tick, and an ACP dispatch test last touched
  before this batch's base) are recorded there too rather than retried away
`src/slash/slash-commands.ts:651`, `:701`, `:724`; the admin surface is `src/slash/subagents-admin.ts` (432 lines) — vs the 16-variant match at `registration/slash_commands.rs:127-145`, which has none of the three. **A fourth is now known**: `/subagents-guide`, filed separately as area 09 `SUBA-066` because it sits outside both this entry and `SUBA-055`. **Observable**: no interactive admin surface for an agent's model/thinking/prompt, no way to detach a live foreground run from a slash command, no refinement overlay generation.

**~~VL-S12 · Four slash commands upstream deleted at v0.41.0 are still registered~~** — ~~*small*~~ **CLOSED 2026-09-21** · **reverse lag**, not a port bug and not lag
- **CLOSED. Four commands deleted**: `SlashCommandName::Chain`, `Parallel`, `RunChain` and
  `ChainPrompts` are gone from the enum, from `SLASH_COMMANDS`, from the parsers and from the
  prose that taught them. `git grep -n 'SlashCommandName::\(Chain\|Parallel\|RunChain\|ChainPrompts\)'`
  is empty
- **The deletion is pinned NEGATIVELY**, which is the only way a removal can be: a test asserts
  each of the four names resolves to nothing in `SLASH_COMMANDS`, so re-adding one is red
- **The blocker was genuinely discharged first.** `VL-S2`'s `workflowScript` runtime landed before
  this, so the capability these four exposed did not disappear with them — it moved, which is
  exactly what upstream did at v0.41.0
- **Collateral, triaged rather than deleted wholesale:** the five pure slash-surface tests over the
  deleted commands are gone, but `tool_parallel_chain`'s inline-group fan-out counting and both
  recipe-chain tests were RE-POINTED onto the surviving surfaces, because the machinery under them
  is real and still reachable. Six retired, nine new
`git grep -oh 'registerCommand("[a-z-]*"' <tag> -- src` gives 19 unique names at v0.40.0 including `chain`, `parallel`, `run-chain`, `chain-prompts`, and 15 at v0.41.0 with all four gone (still gone at v0.43.0 and v0.47.1) — vs `registration/slash_commands.rs:128` (`Chain`), `:129` (`Parallel`), `:130` (`RunChain`), `:142` (`ChainPrompts`). **Observable**: cyrup's palette advertises four commands upstream no longer has, whose function moved into `workflowScript` (VL-S2). Do not delete them before VL-S2 lands or the capability disappears entirely.

**~~`debug.run` · run-lifecycle diagnostic dump~~** — ~~*low*~~ **CLOSED 2026-09-19**
- **CLOSED.** `background/run_lifecycle_debug.rs` ports `run-status.ts:47-108` @v0.68.0
  (`formatCapacityOwner`, `formatWorkflowDebug`, `formatRunLifecycleDebug`) and
  `SubagentExecutor::control_debug_run` (`extension/executor/status.rs`) ports the verb's four
  arms (`:406-410` location without the foreground/nested ladder, `:472-475` the FULL
  reconciler, `:512-519` the dump over the reconciled status with
  `inspect_active_async_capacity_owner`'s three identities, `:714-721`/`:781-785` the two
  refusals); advertised at pi's own index (`shared/types.ts:2801`, directly after `status`) and
  dispatched through the control band's own arm with pi's target-before-view refusal order
  (`subagent-executor.ts:6532-6536`). `SUBAGENT_ACTIONS` 51 → 52.
- **The process-terminal lines, as of VL-S4's closure (2026-09-20).** When this verb landed cyrup
  had no sidecar, no status overlay, no reader and no writer, so the dump printed ONE
  `Process terminal: not recorded — …` line and the `check_pid_liveness` probe beside it. **That
  substitute is DELETED**, not reworded: the dump now prints upstream's real three —
  `Process terminal file:` (`run-status.ts:95`) and the `Status process terminal:` /
  `Sidecar process terminal:` pair (`:102-103`) — fed by `debug_process_terminal`, cyrup's
  `debugProcessTerminal` (`:52-58`), and the file the first line names is one
  `initialize_process_terminal` wrote before the runner was spawned. `Capacity runner:` prints the
  minted `runnerProcessInstanceId` at pi's own `:69`, with `Capacity runner pid:` kept beside it
  because cyrup's detached runner has a pid upstream's in-process child does not.
  `Workflow parent:` / `Workflow key:` / `Lane:` remain absent because `RunStatus` has none of
  those fields (upstream itself omits them when undefined).
- **Reachability:** `cyrup-it/tests/subagents/debug_run_lifecycle_integration.rs` drives the
  PRODUCTION claim path (`spawn_background_steps` at `max_active_async_runs_per_session: 1` →
  `slot-0/owner.json` + `mark_started(pid)`), then `SubagentTool::execute({action:"debug.run"})` by
  id and by dir, asserting the real slot, the minted instance, both process-terminal records and
  the EXISTENCE on disk of the sidecar the first line names.
- **2026-09-20 (remediation round 1):** the `dir` form ports the rest of
  `resolveAsyncRunLocation` (`background::resolve_async_run_dir`): `assertInsideRoot`
  (`async-resume.ts:229`, `Async run directory must be inside <root>.`) and the `id`/`dir`
  mismatch throw (`:231-233`). A run directory WITHOUT `status.json` is refused BEFORE the
  reconciler runs — `:714-721` when a result file exists, `:781-785` otherwise — because
  upstream's reconciler returns `status: null` there (`stale-run-reconciler.ts:369`) while
  cyrup's `reconcile_now` would repair and WRITE one from the result; a diagnostic must not
  create the record it reports.

**~~`children.list` · retained workflow children~~** — ~~*low*~~ **CLOSED 2026-09-19**
- **CLOSED.** `background/retained_children.rs` ports `retained-children.ts` @v0.68.0 (135 lines):
  `listRetainedChildren`, `childResumability` rung by rung, `boundedTaskSummary`,
  `formatRetainedChildren` with the keep-one-resumable window rule; advertised at pi's own index
  (`shared/types.ts:2801`, between `models` and `guide`) and dispatched from `route_action`'s own
  read arm before `doctor` (`subagent-executor.ts:6467`). `SUBAGENT_ACTIONS` 50 → 51.
- **The in-tree premise it replaced was stale three ways, and each note is corrected, not
  reworded:** `text.rs`'s "this build retains nothing" (a workflow's settled children ARE retained,
  as step rows of the workflow's own `status.json`); `goal_driver.rs`'s and `notices.rs`'s "cyrup
  has no `workflowScript` runtime" (it has had one since WORKFLOW_2 — both `[CYRUP-DELTA]`s
  deleted, and `raise_goal_continuation_notices` now passes the real list); `handoff/read.rs`'s
  "upstream's only caller of `resolveParallelHandoffChild` is `retained-children.ts:68`" (that
  line calls `resolveRetainedWorktreeCwd`, which cyrup has and which this verb now calls too).
- **The port is over cyrup's shape, and says so.** A cyrup workflow child is FOREGROUND — no async
  dir, no `parentWorkflowRunId` on disk (`async_retention/policy.rs`'s note stays true) — so one
  upstream retained run is one `(workflow status, step index)` pair, the descriptor is read at the
  workflow's dir (the same file `resume { id, index }` reads), and the resume hint names
  `id: "<workflow>", index: N`. `workflow_step_statuses` now fills `sessionFile`/`model`/`usage`
  from `results[0]`, without which every row answered `no persisted session file`.
- **2026-09-20 (remediation round 1):** the row's `state`/`completedAt` are the CHILD's — the step
  row's own `status` and `ended_at` (upstream's `run` at `retained-children.ts:85-104` is the child
  async run) — so a settled child of a still-`Running` workflow is listed and a `failed` child of a
  workflow that caught the failure prints `failed`; `carry_step_settle_times` stamps `ended_at` on
  the row when the child settles. The candidate source is the async root's DIRECTORY SCAN
  (`tui::fleet::collect_async_runs_by_scan`, pi `repairScan`), not the indexed fleet history: a
  foreground workflow run is in neither run index, so the indexed source hid every workflow status
  file once any async single run of the session had settled. That scan applies the `sessionId`
  filter BEFORE its 100-candidate bound (upstream's `repairScan` arm is unbounded and its
  `entryLimit` reaches only the per-session terminal-index read, `async-status.ts:519`), so a
  hundred newer runs from other sessions cannot push this session's own out of the window. A deleted cwd prints upstream's
  `catch` sentence (`resume dependency unavailable: ENOENT: …`). `details` is
  `{ mode: "management", results: [] }` (`:6471`).
- **Residuals with TRUE premises (not closed here):** R1 — no producer writes a recovery
  descriptor for a workflow launch, so in production every row lists `not resumable (missing
  recovery descriptor)` (pi's own reason sentence; `resume` refuses the same absence with
  `RecoveryDescriptorError::Missing`'s different sentence, "Async child '<id>' is missing its
  required run fan-out recovery identity. Start a new run instead." — same file, same verdict,
  not the same words), until a per-child descriptor location is chosen; R2 — *(closed 2026-09-20)* `StepStatus::runner` now carries the child's
  `SingleResult::runner` (pi `AsyncJobStep.runner`, `shared/types.ts:1315`) off `results[0]` at
  settle, and the external-runner rung (`:54`) refuses `external-cli`/`external-job` before the
  session rung; R3 — no task text on the step row, so `task:` is upstream's own
  `(no task summary)` branch.

**~~VL-S13 · Agent refinement WRITE half~~** — ~~*medium*~~ **CLOSED 2026-09-19**
- **CLOSED.** `exec/agent_refinements/` (evidence, proposal, action) + `exec/refinement_evidence.rs`
  port the three functions the READ half's own module doc named as missing —
  `collectBoundedRefinementEvidence`, `validateRefinementProposal`, `handleRefinementAction` — so
  cyrup can now GENERATE and REVERT the overlay it has always applied at spawn. `SUBAGENT_ACTIONS`
  47 → 50; the remaining gap is **9** verbs (`children.list`, `inspector.*`, `project.*`,
  `debug.run`) — `children.list` and `debug.run` both closed 2026-09-19 (the two rows below),
  leaving **7**.
- **This was the FOURTH reader-without-writer, and the last one known.** The handoff manifest was
  the first (PR #143), the recovery descriptor and the child transcript the second and third
  (PR #144). `exec/agent_refinements.rs` (673 lines) declared its own gap in its module doc —
  *"restricted to the READ half that the spawn path needs"* — and that doc is now true of a
  complete feature instead of half of one
- **Two production surfaces**, sharing one body: the `subagent` tool's `refine`/`refine.show`/
  `refine.rollback` arms (`extension/tool/routing.rs`) and `/subagents-refine <agent>`
  (`extension/host/slash.rs`). `refine` and `refine.rollback` are gated as mutating and refused in
  child-safe fanout; `refine.show` is read-only and stays reachable there, matching pi's
  `MUTATING_MANAGEMENT_ACTIONS` (`subagent-executor.ts:213`), which omits it
- **The writer round-trips the EXISTING parser** — `write_refinement_file` reparses its own bytes
  and compares, so a file this crate writes is one `parse_refinement_file` reads back, and the
  `<!-- pi-subagents-refinement:v1 -->` metadata comment and both fence languages stay
  byte-identical so pi and cyrup still read each other's overlays
- **A SECURITY BYPASS was found and closed inside this batch, by the QA lens written to hunt for
  it.** `validateRefinementProposal` is a privilege boundary, not a lint: the overlay it gates is
  folded into every later spawn's system prompt, so guidance that slips through silently
  re-instructs every future run of that agent. A verbatim port of upstream's `\s+` is NOT
  equivalent in Rust — Rust's `\s` is `\p{White_Space}`, which **excludes U+FEFF**, while
  ECMAScript's `\s` includes it, so `disable\u{FEFF}acceptance` passed cyrup's regex and is
  blocked by pi's. Measured in both engines rather than argued. The class is now
  `[\s\x{FEFF}]+`, every alternative of the pattern has its own test, and the one residual
  divergence (U+0085 NEL, which Rust blocks and ECMAScript does not) is stricter — the safe
  direction — and recorded
- **No overlay is written on ANY failure path** — no evidence, child error, invalid proposal, zero
  edits — and the tests assert the FILE ON DISK is unchanged, not merely that an error was returned
- upstream: `src/agents/agent-refinements.ts` (624 lines @v0.68.0) —
  `collectBoundedRefinementEvidence:349`, `validateRefinementProposal:448`, `proposalSchema:471`,
  `proposalFromChild:502`, `handleRefinementAction:546`
- ~~**Re-measured 2026-09-16 against the verb set:** *"`refine`, `refine.show` and `refine.rollback` are three of the seventeen verbs absent from cyrup's 42-verb action list"*~~ — **false at `14e6c56`, in both halves.** All three verbs are advertised, and the list is **59**, not 42: `extension/tool/text.rs:234` states it "names all **57** verbs of upstream's `SUBAGENT_ACTIONS` (`shared/types.ts:2801` @v0.68.0) plus **2** cyrup dispatches that upstream does not advertise there — `append-step` and `inspect`". **The "42-verb list" figure is stale wherever it appears** — `VL-S6`'s history bullet, `PB-11`'s closure note and §3b's `SUBA-055` clause all carry it; enumerate `SUBAGENT_ACTIONS` rather than quoting the number. The read-half-only statement at `exec/agent_refinements.rs:12-20` was not re-resolved this pass
`src/agents/agent-refinements.ts:349` (`collectBoundedRefinementEvidence`), `:448` (`validateRefinementProposal`), `:546` (`handleRefinementAction`) — vs `exec/agent_refinements.rs:12-20`, which states the port is the read half only, and no `refine*` verb in the enum at `extension.rs:6557` (area 09 counts three such verbs missing). **Observable**: an overlay written by upstream (or by hand) is applied correctly at spawn (`exec/mod.rs:1565`), but cyrup can never generate or roll one back.

**~~VL-S14 · `runner: external-cli` agents unsupported~~** — ~~*medium*~~ **CLOSED** · id retained
- **CLOSED, re-verified in code 2026-09-16 at `cc7818b`.** `exec/external_cli/` is the port — `mod.rs`, `run.rs`, `env.rs`, `framing.rs`, `preflight.rs`, `prompt.rs` and an `adapters/` directory (`claude_code.rs` among them) — and the frontmatter `runner` key round-trips at `discovery/management/frontmatter_write.rs:343`/`:773`/`:779`. `background/runner_main.rs:4020` is no longer "the sole trace". Its residual defects are tracked on area 09 `SUBA-095`, which is closed; `00-residual-ledger.md`'s tenth edition records one unfixed comment-level finding against `external_cli/run.rs` that this pass did not re-check. **The body below is the original filing and is kept as history.**
`src/runs/shared/external-cli-runner.ts:12`, `:26`; `src/api/external-runs.ts` (129 lines); refusal text `runs/foreground/subagent-executor.ts:5023` — vs `discovery/frontmatter.rs` (no `runner` key) and `discovery/types.rs` (no `runner`/`external` field); the sole trace is a doc citation at `background/runner_main.rs:4020`. **Observable**: `runner: {type:'external-cli'}` in frontmatter parses as if absent and the agent is launched as an ordinary cyrup re-exec instead of shelling out to the declared CLI (or being refused, as upstream does for foreground/clarify).

**~~VL-S15 · Native extensions cannot register a keyboard shortcut~~** — ~~*small*~~ **CLOSED** · area 06 `EXT-039`, closed there · see also §2 UW-7, which this does **not** close
- **CLOSED, re-verified in code 2026-09-16 at `cc7818b`.** `InitApi::register_shortcut` exists at `crates/cyrup-ext/src/native.rs:406`, is carried into the registry at `crates/cyrup-ext/src/facade.rs:502`, and is **dispatched** — `native.rs:651` runs the declared shortcut, with `:664` recording in-tree that it was a write-only surface before that landed (EXT-035). The `native.rs:240-297` "no `register_shortcut`" citation below is false at HEAD
- **This did NOT close UW-7.** UW-7 needed `on_terminal_input` — a per-keystroke stream into a widget — not a registered chord. Two different seams, and this row's old "same missing seam family as VL-S15" cross-reference in UW-7 is corrected there. **UW-7 closed separately 2026-09-22**, through `on_terminal_input` exactly as this bullet predicted; `register_shortcut` played no part in it. **The body below is the original filing and is kept as history.**
`src/slash/slash-commands.ts:719-722` (`pi.registerShortcut(Key.ctrlAlt("f"), … showFleet(ctx))`) — vs `crates/cyrup-ext/src/native.rs:240-297` (`InitApi` exposes `subscribe`, `register_tool`, `register_command` and three renderer registrations; no `register_shortcut`). The WASM-guest path HAS one (`cyrup-ext/src/host/live.rs:98`), which proves the seam can carry it. **Observable**: the fleet inspector opens only by typing `/subagents-fleet`; Ctrl+Alt+F has no counterpart, and the same limit blocks every other native-extension shortcut (`crates/cyrup-intercom/src/extension.rs:465` records the identical complaint). Note area 11's correction: `ui/mod.rs:12-19`'s rationale is now **half stale** — `register_message_renderer` DOES exist at `native.rs:270`; only `register_shortcut` is missing.

### 1c. From `pi-permission-system` v0.7.1

**~~PB-32 · `should_expose_tool` keeps `bash` advertised under a tool-level deny — and the allow-listed command executes~~** — ~~***critical***~~ **CLOSED 2026-09-22** · = area 10 `PERM-009` (raised from medium this pass; **now the first row of area 10's table**)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `crates/cyrup-permission-system/src/extension/agent_start.rs:200-222` (`should_expose_tool`) has **one** bypass (`read` + allowed skills, `:217`); the doc at `:183-198` records the bash arm's deletion, names it a live permission bypass, and cites the reproduction in `REPRO-LOG.md §PERM-009`. **The body below is the original filing and is kept as history.**
- upstream: `shouldExposeTool` has a read/skills bypass and **nothing else** at **both** tags — `src/index.ts:2049-2075` @v0.7.1 (the ported baseline, which governs the classification) and `:1790-1816` @v0.8.0. There is no bash branch at either. cyrup's in-tree citation of `index.ts:2049-2075` turns out to be the correct v0.7.1 offset
- cyrup: `crates/cyrup-permission-system/src/extension.rs:1651-1653` adds `if tool_name == "bash" && mgr.get_bash_permissions(agent_name).any_allow() { return true; }`, with a justification comment at `:1624-1631`. And `manager.rs:205-215` resolves a bash **command** rule above the tool-level state — its own comment says "command rules OUTRANK the tool-level bash fallback"
- observable: `tools.bash: deny` + `bash: {"git status": allow}` hides bash in pi; in cyrup it leaves bash **advertised to the model AND executes the command**. A configured deny is defeated silently, in both directions. This is an **in-baseline parity bug**, not drift. No test pins the divergence (`tests/context_hygiene.rs:128-152` denies `write`), so the suite goes green on the deletion. Effort **S**: delete the branch and its comment, refresh the citation.

**~~PB-15 · Model-option compatibility guard (temperature stripping) is entirely unported~~** — ~~*medium*~~ **CLOSED 2026-09-22** · = area 10 `PERM-012`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup-provider/src/api/compat.rs:868` (`unsupported_temperature_reason`) carries pi's three reasons (api / provider / model) and is consumed as the gate on every `temperature` insert (`:825`, `:904`). `grep -rn "does not support temperature" crates/` is no longer 0. **The body below is the original filing and is kept as history.**
- upstream: `src/model-option-compatibility.ts:62` @v0.8.0 (`getUnsupportedTemperatureReason`), `:126` (`ensureModelOptionGuardForApi`), `:164` (`registerModelOptionCompatibilityGuard`); wired as the **first statement** of the `session_start` handler, `src/index.ts:1829`. `git diff v0.7.1..v0.8.0 -- src/model-option-compatibility.ts` is **empty** and v0.7.1 `index.ts:2088` makes the same call — this predates the ported baseline
- cyrup: `crates/cyrup-permission-system/src/extension.rs:1991` (the `HostEvent::SessionStart` arm) registers no guard; `cyrup-provider/src/api/openai_responses.rs:359-361`, `openai_codex_responses.rs:707-708` and `azure_openai_responses.rs:386-387` insert `temperature` unconditionally; `grep -rn "does not support temperature" crates/` = 0
- observable: with the permission system installed, pi strips `temperature` for openai-codex-responses, the openai-codex provider, any `codex`-tokened model id, and any reasoning model on openai-/azure-openai-responses; cyrup sends it (its own test at `openai_codex_responses.rs:1515` asserts `body["temperature"] == 0.25`), so those requests are rejected or silently mis-parameterised. Not blocked: `cyrup-session-svc/src/guest_providers.rs` already exposes `register_provider`/`unregister_provider`.

**~~PB-16 · Permission-request events are never emitted — native extensions cannot reach the event bus~~** — ~~*medium*~~ **CLOSED 2026-09-22** · = area 10 `PERM-011` half B (area 10 merged PB-16 and UW-9 into one item)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup-permission-system/src/extension/consts.rs:31` declares `PERMISSION_REQUEST_EVENT_CHANNEL = "cyrup-permission-system:permission-request"` and `extension/events.rs` is the two event-bus publications; `cyrup-ext/src/native.rs` now carries 15 `bus` references where this row measured zero. **The body below is the original filing and is kept as history.**
- upstream: `src/index.ts:150` @v0.8.0 (`PERMISSION_REQUEST_EVENT_CHANNEL`), `:1518-1529` (`emitPermissionRequestEvent`), `:1531-1548` (`emitPermissionStateEvent`), fired `:1606`, `:1612`, `:1626`. Present at v0.7.1 (`:137`, `:1753-1755`, fired `:1825`/`:1844`/`:1871`)
- cyrup: `grep -rn "events.emit|emit_event|permission-request" crates/cyrup-permission-system/src` = 0. **The bus itself exists** — `SharedBus` at `cyrup-ext/src/host/services.rs:988` (`subscribe` `:1002`, `emit` `:1010`, `take_pending` `:1018`), fanned out by `cyrup-ext/src/facade.rs:1003-1026` — but it is wired to WASM guests only (`host/live.rs:642`, `:650`) and `cyrup-ext/src/native.rs` contains **zero** `bus` references
- observable: in pi any extension can subscribe and observe every waiting/approved/denied transition with its requestId, tool, command, target and agent; in cyrup no such stream exists. The work is a native-extension bus accessor (area 06 owns the seam) plus three emit sites at `extension.rs:1384-1469` — **not** a new subsystem.

**~~PB-17 · The forwarding half of the security-review audit trail is unwritten~~** — ~~*medium*~~ **CLOSED 2026-09-22** · = area 10 `PERM-008`, which sharpened the count to **8 review + 3 debug sites** — **CLOSED; and the census was still short.** `PERM-008` landed the mechanism and **eleven** call sites; upstream has **~28**, and the sixteen missing ones (the fs helpers, both reader diagnostics, and **both response-binding rejections**) were ported by sweep 6 as **`PERM-033`** — filed and closed 2026-08-14. The security-relevant one: a forged or misaddressed forwarded response was discarded leaving only an all-null `response_received` entry, so nothing named it.
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `forwarding.rs` calls `audit.review(…)` at `:736`, `:795`, `:848`, `:1070`, `:1148`, `:1171` and `:1182` — this row's "grep returns zero" is long gone. **The body below is the original filing and is kept as history.**
- upstream: `src/index.ts` @v0.8.0 has the `writeReviewEntry` definition at `:200` plus 17 call sites; the forwarding path is `:735`, `:1032`, `:1058`, `:1080`, `:1173`, `:1184`, `:1187`, `:1228`. All eight exist at v0.7.1 (`:1011`, `:1019`, `:1298`, `:1324`, `:1346`, `:1417`, `:1428`, `:1473`)
- cyrup: `crates/cyrup-permission-system/src/forwarding.rs` is 1125 lines and `grep -n "logger|review|tracing"` returns **zero**; both entry points (`wait_for_forwarded_approval` `:398`, `process_forwarded_requests` `:528`) log nothing. `write_review_entry` is at `extension.rs:930` with 6 calls
- observable: a forwarded child ask that times out, expires, is auto-approved or is denied leaves no audit record, and every forwarding I/O failure is silent where pi writes `permission_forwarding.error`.

**~~PB-18 · `/permission-system` prints text instead of opening the settings modal~~** — ~~*medium*~~ **CLOSED 2026-09-22** · = area 10 `PERM-007`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `cyrup-permission-system/src/config_modal.rs` is the `PERM-007` port of `config-modal.ts:63-122`, with `ConfigController` and `PermissionSystemSettingsOverlay`. **The body below is the original filing and is kept as history.**
- upstream: `src/config-modal.ts:63-122` @v0.8.0 (`openPermissionSystemSettingsModal` — `ctx.ui.custom<void>` overlay at `:66` with a live `onChange`→`setConfig` loop), registered at `src/index.ts:1502-1512`. Same shape at v0.7.1
- cyrup: `extension.rs:1038` (`run_permission_system_command`) parses `<setting> <value>` and returns a `String`. **The in-tree rationale ("HostServices exposes no custom-overlay seam") was STALE and has since been corrected in place — `extension.rs:998` now says "One exists:" and names it.** The seam: `HostServices::open_overlay` is `cyrup-ext/src/host/services.rs:254`, implemented at `cyrup-session-svc/src/host_services.rs:1043`, driven by `App::on_overlay_request` (`cyrup-tui/src/app/run_arms.rs:406-430`, which pushes onto `state.overlays` at `:427`), and already consumed in production by `cyrup-ext-subagents/src/extension.rs:11376` — with no `cyrup-tui` dependency in the consuming crate *(all five citations re-resolved by symbol 2026-08-19; the `app/events_fold.rs:441` one was a bad `40821ed` remap and lands on a doc comment)*
- observable: `/permission-system` opens a live two-row modal in pi; in cyrup it prints a status paragraph and the user must retype `/permission-system debug on`. A straight port onto an existing seam. Area 10 adds the missing companion: `PERM-024` (config not refreshed on `before_agent_start`) is what makes the modal feel instantaneous once it lands.

> **Area 10's second fail-open is not a port bug and so is not in this section**: `PERM-023` (*high*,
> `cyrup-original`) — `is_installed` (`extension.rs:2159-2175`) probes env, policy file and
> `config.json` and never `agents_dir`, which `manager_paths_for` (`:390-401`) wires and
> `manager.rs:500-503` enforces. An operator whose only policy artifact is agent-markdown
> frontmatter gets no extension attached and silently inert deny rules. See §5.

### 1d. From `pi-intercom` v0.9.2

> **The ported baseline was recorded wrong in every prior document.** A citation census over
> `crates/cyrup-intercom/src` returns **v0.9.2 × 272**, v0.7.0 × 14, v0.8.0 × 3, v0.6.0 × 1 (the
> `lib.rs` banner), v0.10.x × 0. Load-bearing v0.8.0/v0.9.x code is present **and tested**:
> `broker/runtime_claim.rs`, `/intercom-id`, `format_context.rs`, the 16-tag `BrokerMessage` union,
> the v0.9.2 envelope with `#[serde(flatten)] extra`, and `transport/target.rs` + `stream.rs`.
> **The true baseline is v0.9.2.** Two consequences: (a) the drift window is two minor versions, not
> three — §3c; (b) **`VL-I1…VL-I6` were never version lag.** They are in-baseline port bugs and move
> here, keeping their ids. Area 11 maps all six and confirms all six still open.

**~~PB-19 · The broker binds a Unix socket unconditionally and never consults its own listen-target resolver~~** — ~~*low*~~ **CLOSED 2026-09-22** · = area 11 `ICOM-015`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `broker/lifecycle.rs:118` calls `transport::target::broker_listen_target(&agent_dir)`, and `broker/listener.rs` binds `Tcp` (`:77`) or `Unix` (`:105`) off it and publishes the chosen port (`:73`, `:288`). **The "zero callers of any kind" claim in the body below is refuted, not merely stale.** **The body below is the original filing and is kept as history.**
- upstream: `broker/broker.ts:21` @v0.7.0 (`const LISTEN_TARGET = getBrokerListenTarget();`) with the two-branch listen at `:176-179`; helper `broker/paths.ts:107`, Windows named pipe `paths.ts:65-74`, TCP predicate `paths.ts:44-59`. At v0.9.2 the broker additionally publishes its endpoint (`broker/broker.ts:252-256`, `stateId: BROKER_STATE_ID`) and enforces it at `:408-409`
- cyrup: `crates/cyrup-intercom/src/broker/mod.rs:1243` — `let listener = UnixListener::bind(&socket_path)?;`, unconditional; `:24` imports only `tokio::net::UnixListener`; `paths.rs:6-8` records the deferral. The ported resolver `broker_listen_target` (`transport/target.rs:278`) has **zero callers of any kind**. **The CLIENT half is fully live**: `broker_connect_target` (`target.rs:254`) is called from `transport/spawn.rs:226` and `:299`, and `transport/client.rs:202` handles all three transports
- observable: with `CYRUP_INTERCOM_TRANSPORT=tcp` (or on Windows) a cyrup client resolves a TCP/pipe endpoint while a cyrup broker only ever listens on a Unix socket and never writes `broker.port.json` — the two halves cannot meet. Severity is *low* because the reachable configuration is Windows or an explicit env override; see OQ-3, which decides how much of this survives.

**~~PB-20 · The bundled `pi-intercom` skill is not shipped or registered~~** — ~~*medium*~~ **CLOSED 2026-09-22** · = area 11 `ICOM-004`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `crates/cyrup-intercom/resources/skills/pi-intercom/SKILL.md` ships — the row's own `find … ! -name '*.rs'` now returns it — discovered through `resources.rs` and answered on `EventKind::ResourcesDiscover` (`resources.rs:17`). The row's instruction was followed: `resources.rs:2` records it as the **v0.10.1** text. **The body below is the original filing and is kept as history.**
- upstream: `pi-intercom/skills/pi-intercom/SKILL.md` — 514 lines at v0.9.2 (513 at v0.7.0; **rewritten by 164 lines at v0.10.0**), declared at `package.json:26-28` (`"pi": { "skills": ["./skills"] }`)
- cyrup: `find crates/cyrup-intercom -type f ! -name '*.rs'` returns only `Cargo.toml`; `init` at `extension.rs:457-495` registers 2 tools and 2 commands, subscribes 8 event kinds, never subscribes `EventKind::ResourcesDiscover`, and never registers a skill
- observable: a pi session with intercom installed gets a coordination-protocol skill the model can load; a cyrup session has none. Not blocked: `EventKind::ResourcesDiscover` exists (`cyrup-ext/src/event.rs:20`), is dispatched via `facade.rs:485`, and `cyrup-permission-system` already subscribes to it (`extension.rs:1900`). **Port the v0.10.0 text, not the v0.9.2 text.**

**~~PB-21 · The session-name poll timer is unported; `CYRUP_INTERCOM_NAME_POLL_MS` is inert~~** — ~~*medium*~~ **CLOSED 2026-09-22** **(severity corrected up)** · = area 11 `ICOM-006`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `identity.rs:153` (`name_poll_ms()`) has a production caller at `session_state.rs:574`, which builds the poll interval; presence goes out through `update_presence_full` (`session_state.rs:720`). **The body below is the original filing and is kept as history.**
- upstream: `index.ts:598-611` @v0.7.0 (`startNamePoll`, `setInterval` `:601`, interval from `getNamePollMs()` `:609`; helper `:421-429`). Same shape at v0.9.2 (`index.ts:461`, used `:829`)
- cyrup: `identity.rs:24` declares `ENV_INTERCOM_NAME_POLL_MS` and **its declaration is its only occurrence**; `transport/client.rs:368` (`update_presence`) has no production caller; the only live presence path is `update_presence_with_context` from `sync_presence` (`extension.rs:205`), which **hard-codes `name: None`**. The label is sent once at connect (`connect.rs:444`)
- observable: renaming a session never updates its presence label for peers — other sessions' `/intercom` listings keep the old name until the client reconnects. Setting the env var does nothing.

**~~VL-I1 · Broker has no mailbox~~** — ~~*medium*~~ **CLOSED 2026-09-22** · id retained, **class corrected to port bug** · = area 11 `ICOM-010`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `broker/mailbox.rs` plus `broker/state.rs:82-84` (`mailbox_messages`, `queue_mailbox_message`, `flush_mailbox_for_session` on re-register). `connect.rs:47-50` records that this row's "there is no mailbox, no queue, no redelivery" was true of the v0.7.0 shape only. **The body below is the original filing and is kept as history.**
`broker/broker.ts:40-41` (24 h retention / 256 messages), `:219` (`mailboxMessages`), `:775`/`:1002` (`queueMailboxMessage`), `:992` (prune), `:1020` (`flushMailboxForSession`, called on register at `:510`), `:1110` (`findDisconnectedSessions`) — vs `broker/mod.rs:792-797`, which replies `DeliveryFailed{reason:"Session not found"}` the moment no LIVE session resolves; absence documented at `:590`, `connect.rs:46`, `reply_tracker.rs:268`. **Observable**: sending to a named session that just restarted is lost; pi queues and delivers on reconnect.

**~~VL-I2 · Message receipts, receiver-side dedupe and delivery metadata~~** — ~~*large*~~ **CLOSED 2026-09-22** · id retained, class corrected · = area 11 `ICOM-017` (+ `ICOM-048`, `ICOM-050`)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `ui/inline_message.rs:295` (`format_inbound_delivery_metadata`) is rendered into the injected body at `:113`; `MessageReceipt` / `MessageReceiptStatus` are live at `session_state.rs:20`; the dedupe window is documented at `connect.rs:344`. **The body below is the original filing and is kept as history.**
`types.ts:49-56` (`MessageReceipt`); `index.ts:446` (`formatInboundDeliveryMetadata`), `:503`, `:515` (dedupe), `:533` (`emitMessageReceipt`), `:564`, `:588`, fired `:880`–`:974`; `broker/client.ts:773`; broker routes `broker/broker.ts:698`, `:773`, `:809`, `:1053` — vs `inbound.rs:347-386` (no timestamps stamped, no `(from.id, message.id)` dedupe, no receipt), `transport/client.rs:635-640` (decodes and `tracing::debug!`s only), `broker/mod.rs:447-456`, `session_state.rs:295-298`. The envelope fields themselves ARE modelled (`transport/protocol.rs:301-347`). **Observable**: a pi peer sending to cyrup gets no receipts, so its `ask` timeout reports an unknown delivery state; a duplicate message id is injected twice; and **cyrup's injected body omits the `_deliveryMetadata_` line entirely, which is why `replyTo` is unreachable without a second tool call** (area 11 `ICOM-048` — land it with `ICOM-043`).

**~~VL-I3 · No cancel / supersede / retry controls~~** — ~~*medium*~~ **CLOSED 2026-09-22** · id retained, class corrected · = area 11 `ICOM-017`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `tools/intercom/mod.rs:431` advertises the eight-action enum including `cancel`, dispatched at `:308`; `supersedes`/`retry_of` ride the envelope (`session_state.rs:976-977`) and render at `inline_message.rs:302-306`; the broker's `handle_cancel_message` (`broker/receipts.rs:88`) resolves a real session key instead of always refusing. **The body below is the original filing and is kept as history.**
`index.ts:1795` (8-action enum including `cancel`), `:1813`/`:1816`/`:1819` (`messageId`/`supersedes`/`retryOf`), `:1927` (the `cancel` case), `:551-562` (`handleMessageControl`); `broker/client.ts:738` (`cancelMessage`); `broker/broker.ts:642` (supersede validation), `:822-866` (cancel) — vs `tools/intercom.rs:388` (six actions), `:26-36` (`IntercomParams`), `:315` (unknown-action error), `transport/client.rs:46-57` (`SendOptions` without supersedes/retry_of/sender_sequence), and `broker/mod.rs:595-607`, where `handle_cancel_message` **always** answers `DeliveryFailed{"Message cannot be cancelled by this session"}`. **Observable**: the model cannot cancel or supersede an in-flight message, and a pi peer's cancel/supersede against a cyrup session is silently discarded.

**~~VL-I4 · Extension bus: frames validated then dropped~~** — ~~*large*~~ **CLOSED 2026-09-22** · id retained, class corrected · = area 11 `ICOM-016`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `broker/extension_state.rs` (`ExtensionStateManager`, held at `broker/state.rs:140`), `EXTENSION_BUS_FEATURE` advertised on `registered`, and `extension_state_commit` dispatched at `broker/dispatch.rs:104-114`. **The body below is the original filing and is kept as history.**
`types.ts:1` (`EXTENSION_BUS_FEATURE`), `:80`/`:88`/`:96`, `:115`-`:131`; `extension-api.ts` (44 lines); `broker/extension-state.ts` (186 lines — persisted, sha256-checksummed, 64 KiB-capped, optimistic revisions); `broker/broker.ts:505` (`features: [EXTENSION_BUS_FEATURE]`), `:509` (owner election); `broker/client.ts:216` (`supportsFeature`) with gates at `:648`/`:817` — vs `broker/mod.rs:419-425`, routing the three frames to validation-only handlers (rationale `:460-463`), and `transport/client.rs:575`, which discards the `features` field the protocol models at `protocol.rs:767`. **Observable**: a pi extension registering an intercom namespace gets owner election, cross-session publish and a durable revisioned state store; against a cyrup broker it is told no feature is supported and any forced frame is dropped with no reply. *(Area 11 blind spot 5: neither upstream file was read this pass, so the fix sketch is directional.)*

**~~VL-I5 · No restart-stable intercom session id~~** — ~~*small*~~ **CLOSED 2026-09-22** · id retained, class corrected · = area 11 `ICOM-011`
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `connect.rs:655-665` resolves `ENV_INTERCOM_STABLE_ID` then `config.stable_id`, with pi's falsy-`||` semantics spelled out. `grep -rni 'stable_id' crates/cyrup-intercom/` is no longer zero. **The body below is the original filing and is kept as history.**
`config.ts:38-39` (`stableId`) with fail-closed validation `:141-150`; `index.ts:39` (`STABLE_INTERCOM_SESSION_ID_ENV`), `:409-411`, consumed `:1264` — vs `config.rs:33-48` (7 fields, no `stable_id`) and `connect.rs:377-382`. `grep -rni 'stable_id|stableId'` over `src/` and `tests/` returns **zero**. `/intercom-id` itself IS ported (`extension.rs:474-481`). **Observable**: a cyrup session's intercom address changes on every restart, so peers holding the old id can no longer reach it. *(Area 11's repair-pass spot-checks confirm this is the **only** behavioural absentee in both the config-key diff — pi's 9 `IntercomConfig` members vs cyrup's 7 — and the env-var diff. Warning recorded there: the nine `INTERCOM_*` identifiers in `broker/paths.ts` and `extension-api.ts` are internal constants, not env vars; a name-grep that treats them as such reports a false gap.)*

**~~VL-I6 · No `list-cwd` action~~** — ~~*medium*~~ **CLOSED 2026-09-22** · id retained, class corrected · = area 11 `ICOM-018` (shares `cwd.rs` with `ICOM-042` — port it once)
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `crates/cyrup-intercom/src/cwd.rs` exists (`normalize_cwd` `:38`, `same_cwd` `:60`) and `"list-cwd"` is advertised (`tools/intercom/mod.rs:431`) and dispatched (`:307`). **The body below is the original filing and is kept as history.**
`cwd.ts:13-27` (`normalizeCwd`) and `:29-31` (`sameCwd`); `index.ts:25`, `:1783-1784`, `:1795`, `:1822-1824`, `:1874-1925` (the case, with the "your session's cwd has N peers" fail-loud note at `:1901-1908`) — vs `tools/intercom.rs:388` and `:26-36`; no `cwd.rs` in the crate. **Observable**: a cyrup agent cannot ask for peers in a given working directory; it must call `list` and eyeball paths. *(Do NOT also claim symlink-normalization breakage: pi's own session list compares raw strings exactly as `ui/session_list.rs:153` does.)*

### 1e. Reverse lag — cyrup carries behaviour upstream changed or deleted (14 items, `stale-port`)

Not port bugs and not lag; a third shape that still costs behaviour. The named instances:
`VL-S12` (four deleted slash commands), area 01 `PROV-033` (`sendSessionIdHeader`, which pi **deleted**
in #6496 with a documented migration to `sessionAffinityFormat: "openai-nosession"` —
`packages/ai/CHANGELOG.md:168` — so `x-session-id` is now unreachable on openai-responses),
`PROV-016` / `PROV-019` (validation and `max_output_tokens` behaviour that drifted from a byte-identical
upstream), `PROV-039` / `PROV-041` (provenance comments asserting things that are no longer true),
`AGENT-017`, `SESS-025` / `SESS-027`, `EXT-028` / `EXT-036`, `TUI-025`, `SEAM-029`, `ICOM-012`, and —
new to this class this pass — area 12 `DRIFT-016` (`Current date:` still injected into the system
prompt; `git grep 'Current date' v0.83.0 -- packages/coding-agent/src` returns **nothing**, so it is
cyrup carrying something upstream does not have, not lag). The recurring sub-shape is **an in-tree
comment that documents a divergence the code no longer has, or a rationale that a later commit
invalidated** — six of the fourteen. Fix the comment in the same change as the code, always.

---

## 2. Unwired — the code exists in cyrup and has no production caller

**This is the project's most common defect class and the cheapest to fix.** Batches 8–10 shipped
roughly forty such items — all with green tests, because the tests called the functions directly. **A
green suite is not evidence that a subsystem runs.** Every entry here is a wiring job, not a port.

A refinement this pass earned, recorded by area 05 and worth generalising: **a `/settings` row is not
a consumer.** The previous sweep's "has a consumer" test let `doubleEscapeAction` through because it
was rendered in the settings list — and nothing else read it.

> **SWEEP 2026-09-16, cyrup `cc7818b` — every row below re-greped, and this register was badly
> stale.** Eleven of the twenty entries are closed or partially closed and are marked in place;
> five that stay open carry refreshed citations; **one new entry, `UW-21`, is filed by this pass
> against code the SCOPE sequence itself landed** — which makes it the FIFTH time this programme has
> shipped tested machinery with no production caller, and the second time the batch that shipped it
> also wrote the test that proves the machinery works in isolation. Ids are never renumbered or
> deleted here, so a closed `UW-` keeps its number and its body.
>
> **The method, so a later reader can re-run it rather than trust it.** For each row: grep the named
> symbol workspace-wide, subtract every hit inside the symbol's own module and every hit under
> `tests/`/`#[cfg(test)]`, and ask whether anything is left. A row closes only when what remains is a
> call on a path a user can reach. A doc-comment mention is not a caller — three rows below were
> kept open on exactly that distinction.

**~~UW-1 · The native modifier probe has no production caller, so the Apple-Terminal Shift+Enter rescue never fires~~** — ~~*medium*~~ **CLOSED**
- **CLOSED, re-greped 2026-09-16 at `cc7818b`.** `crates/cyrup/src/main.rs:241` calls `cyrup_tui::set_native_modifier_probe(native_modifier_probe::probe)` — a production install in the binary's own startup, which is exactly the caller this row said did not exist. The remaining hits are the definition (`cyrup-tui/src/native_modifiers.rs:62`), the re-export (`lib.rs:196`), the module doc (`:36`), one test (`tests/native_shift_enter.rs:155`) and a `cyrup-it` doc note (`tests/misc/main.rs:21`) recording that the probe is first-writer-wins. **Not live-verified on macOS Apple Terminal** — the wiring is proven, the rescue's behaviour on that terminal is not. **The body below is the original filing and is kept as history.**
- upstream: `pi/packages/tui/src/native-modifiers.ts:21-56` (`loadNativeModifiersHelper` loads the prebuilt darwin/win32 addon), consumed at `packages/tui/src/terminal.ts:6` and used at `:324`
- cyrup: `crates/cyrup-tui/src/native_modifiers.rs:62` (`set_native_modifier_probe`) — the only call workspace-wide is `crates/cyrup-tui/src/tests/native_shift_enter.rs:138`. The consumer side IS wired: `app/input_reader.rs:403` calls `is_native_modifier_pressed` on the production `map_event_on` path *(`app/settings_rows.rs:110` was a bad `40821ed` remap — that line is an idle-timeout description)*
- observable: with no probe installed the predicate always answers false, so on macOS Apple Terminal Shift+Enter still submits instead of inserting a newline — the exact defect the ported code exists to fix. Mechanism note: pi `require`s a prebuilt `.node` addon; cyrup needs an OS query (`CGEventSourceKeyState`/`GetKeyState`), which is FFI and cannot live inside `#![forbid(unsafe_code)]` `cyrup-tui` — the injectable seam exists precisely for that and is fed by nothing. *(Area 07 did not restate this item; `native_modifiers.rs` is one of fifteen files that did not exist at the older baseline. The citations above are at `04c1ba2`.)*

**UW-2 · The first-run setup wizard is gated but never invoked — the `if` body is empty** — *small* · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md)) · **FIXED 2026-08-13**
- **FIXED 2026-08-13, per ADR-0011 (which decided `OQ-9` / `PARITY-GAPS` §6 q6 — *not* `OQ-6`; the escalation below is one of the two mis-citations ADR-0011 records).** The call now lives in its own gate function: `crates/cyrup/src/bootstrap.rs:140` (`maybe_run_first_time_setup`) evaluates the condition at `:148-149` and calls `crate::startup::run_first_time_setup(&theme, settings, detected)` at `:166`; `crates/cyrup/src/main.rs:445` invokes the gate at **pi's position** — after `startupSettingsManager` + its diagnostics, **before** `apply_settings_session_dir` — mirroring `main.ts:610 → 615-617 → 625-630` *(citations refreshed 2026-09-22 at `14e6c56`: the call was in `main.rs` when this row was filed and moved to `bootstrap.rs` afterwards — the `--list-models` conjunct the row calls load-bearing is now `bootstrap.rs:148`; `startup.rs:256` is now `startup.rs:273`)* and pi's stated reason ("Runs before any runtime services are created so the chosen settings apply everywhere"). The condition carries pi's `listModels` conjunct (`mode == Interactive && cli.list_models.is_none() && should_run_first_time_setup(...)`); `!parsed.help` needs none because `main.rs` prints help and returns upstream of the gate, and the comment says so. `detected` is `detect_terminal_theme_for_auto(&StdinTerminalProbe, 100ms, $COLORFGBG)` — pi's `detectTerminalThemeForAuto({ ui, timeoutMs: 100 })`, startup-ui.ts:180 — and the theme is that detected polarity (`UiTheme::light()`/`dark()`), not `UiTheme::default()`, because on a first run there is by definition no `settings.json` for pi's `createStartupTui` to resolve. Nothing was deleted. The false comment at the old `main.rs:215-217` is gone.
- **Test** `crates/cyrup/tests/first_time_setup.rs` — its header no longer claims the wizard is unexercised-and-unreachable; it now states that the production caller exists and lists exactly what a live pty run must show (Light + "Don't share" → `"theme":"light"`, `"enableAnalytics":false`, **no** `trackingId`; opt-in → non-empty `trackingId`; Escape at either step → **no** `settings.json`; relaunch → no wizard; `--list-models gpt` on a TTY → the model list, no wizard; `CYRUP_AGENT_DIR` set → no wizard). The one clause a unit test CAN pin is added: `a_list_models_run_is_still_interactive_so_the_gate_needs_the_list_models_conjunct` proves `resolve_app_mode` answers `Interactive` for `--list-models gpt` on a TTY, which is why the conjunct is not optional. **The wizard APPEARING cannot be asserted from `cargo test`** — it is a `CrosstermBackend` surface — so this fix is *not* live-verified, and that live run is the outstanding evidence.
- **Also strike the trap-list entry** at `docs/gap-analysis/README.md` (done in the same pass): a wrong trap is removed, not downgraded.
- upstream: `pi/packages/coding-agent/src/main.ts:615-616` @v0.83.0; unchanged at v0.84.1 `main.ts:663-664`
- cyrup: `crates/cyrup/src/main.rs:218-223` evaluates `should_run_first_time_setup` and the body (`:221-222`) is comment-only; `crates/cyrup/src/startup.rs:256` (`run_first_time_setup`) has zero callers
- observable: on a first run with `CYRUP_EXPERIMENTAL=1`, no `settings.json` and no agent-dir override, pi presents the theme + analytics wizard and persists the answers; cyrup does nothing. **The gate can fire**: `OFFICIAL_PACKAGE_NAME`/`APP_NAME`/`CONFIG_DIR_NAME` (`startup.rs:32-34`) name cyrup itself and match the live values at `:38-43`, so `is_official_distribution()` (`:71-73`) is **true** for this build. This is the one place where the standing "deliberately unreachable first-run wizard" trap and the code disagree — the comment at `main.rs:215-217` and CLAUDE.md's "compile-time constant `false`" are both stale. Escalated to **OQ-6**.
- **observed 2026-08-13, live terminal, and the trap list is now settled on evidence rather than on a read.** With `CYRUP_EXPERIMENTAL=1`, no `settings.json` and the *default* agent dir, the binary goes straight to the interactive TUI: no theme picker, no analytics question, and no `settings.json` written. Two independent corroborations that the gate's inputs were **all true in that very process**: the footer printed the `xp` experimental badge (`crates/cyrup-tui/src/status.rs:356`, pi `footer.ts:162-164`), proving `CYRUP_EXPERIMENTAL=1` was read; and the agent dir ended the run containing only `models-store.json.lock`, proving `settings_path` did not exist. The wizard is also **not** broken-but-invisible: the sibling pre-launch selectors (trust prompt, resume picker) rendered fine on the same pty in the same pass, so a wizard that ran would have been seen. **The standing trap-list entry "the deliberately unreachable first-run wizard" is wrong and should be struck**; OQ-6 is a live product decision, not a documentation cleanup.

**~~UW-3 · Child-watchdog status events are never read by the parent, so an armed child is killed mid-review~~** — ~~*medium*~~ **CLOSED 2026-09-23** (observable PROVEN by trace 2026-09-22; plan `.flux/todo/CHILD_STATUS_EVENTS.md`)
- **CLOSED 2026-09-23.** The parent now folds a child's `subagent.watchdog.status` lines in its ONE drive loop (`exec/drive_attempt.rs`, `DriveState::fold_child_watchdog_line`), which serves foreground and background alike: an active phase (`reviewing`/`autofollow`/`settling`, or `followUpPending`) clears the 1000 ms final-drain window and arms a watchdog TAIL (`watchdogTailTimeoutMs`, pi `armWatchdogTail`); a terminal phase clears the tail and restarts the drain after a clean stop or `agent_settled`; the tail's expiry stamps the parent's own `{phase: stale, reason: "child watchdog tail timeout", timedOut: true, seq+1}` and drains; `agent_end{willRetry}` clears the tail as well as the drain. The line is folded after the transcript write and returned from BEFORE the activity/progress folds (upstream returns before `lastActivityAt`). The armed config travels out of `spawn_plan.rs` on `AttemptSpawnPlan::child_watchdog` → `PreparedAttempt` → `drive_attempt`, and the identity filter is built from THAT config — never upstream's literal `options.index ?? 0`, which would reject every event of an unindexed run (cyrup writes no `childIndex` then). `SingleResult.watchdog` carries the winning attempt's snapshot; `StepStatus.watchdog` is folded live into `status.json` (`background::apply_child_watchdog_line_to_step`, pi `subagent-runner.ts:2711-2722`) and overwritten at settle from the step's result (pi `:3508`), which is how the parent-only `stale`/`timedOut` mark reaches it.
- **The second defect behind it is fixed too.** The child's review is awaited inside an `AgentEnd` extension handler, and `cyrup-ext`'s dispatcher dropped any handler still running after `DEFAULT_INVOKE_BUDGET` = 5 s — so porting the fold alone would only have moved the cut-off from 1 s to 5 s. `cyrup_ext::native::SanctionedWaitGate` (the P-3 `HumanWaitGate` generalised; the old names stay as aliases) lets a handler DECLARE a bounded long wait (`HostCtx::begin_sanctioned_wait(SanctionedWaitKind::ModelReview, ceiling)`); the dispatcher extends that handler's budget for the guard's lifetime, up to its ceiling, and every undeclared handler keeps the 5 s hang protection. Both reviews use it: the child's (`ChildWatchdog::handle_agent_end_in_handler`, from `prompt_runtime.rs`) and the main session's (`MainWatchdogRuntime::handle_agent_end_in_handler`, from `extension/host/native_impl.rs`), with the ceiling `lsp.timeoutMs + agentEndTimeoutMs + 5 s`. **[CYRUP-DELTA]** upstream has no per-handler budget to extend (`runner.ts:805-811`).
- **Proof.** `crates/cyrup-it/tests/subagents/child_watchdog_status_integration.rs` drives real child processes (`run_sync` → spawn plan → `SpawnedChild` → `drive_attempt`) with the child watchdog armed through the run's own project settings, asserting the env var is present and taking the status lines' identity from the config the parent actually encoded. T1 (the row's observable) was run against the code before the fold and FAILED: drained at 1.007 s. `watchdog_review_budget_integration.rs` scripts BOTH reviews with a provider delayed 6.5 s — past the 5 s budget the instant-provider UW-4 test could never reach — and asserts the review's finding arrives.
- upstream: `pi-subagents/src/runs/foreground/execution.ts:846`, `:848`, `:857`, `:585`; `runs/background/subagent-runner.ts:626`, `:628`, `:640`, `:831`, `:2711-2712`; definitions `src/watchdog/child-status.ts:167`, `:181`, `:186`; emitter `src/watchdog/register-child.ts:102-110`. **All @v0.43.0, the tag `child_status.rs` is pinned to, and all re-verified 2026-09-22.** At v0.68.0 they are `child-status.ts:159`/`:182`/`:187`, `execution.ts:692`/`:1015`/`:1017`/`:1082`, `subagent-runner.ts:3016`/`:3017`/`:3033` (status.json half) and `runs/background/run-child-session.ts:315`/`:430`/`:432`/`:444` (drain half). At v0.68.0 the child is in-process and the events arrive through a `writeStatus` sink rather than stdout, and the fold is the same shape. **These are pinned coordinates: add, never renumber.**
- *(Pre-fix record, kept:)* a status line parsed to `SubagentEvent::Unknown` (`exec/ndjson.rs:245-246`) and was folded nowhere; `final_drain_at` was disarmed only by `agent_end{willRetry:true}`; an armed child that edited files had its final `message_end{stop}` arm the 1000 ms window before `AgentEnd` was dispatched, emitted `reviewing`, started a model review, was SIGINT→SIGTERM→SIGKILLed at +1 s, and `forced_drain_after_final_success` reported **exit 0** with the pre-review answer and none of the review's warnings.

**~~UW-3a · The main watchdog's boundary review is dropped by the 5 s extension-handler budget~~** — ~~*medium*~~ **CLOSED 2026-09-23** (found by UW-3's trace; filed here as its own row, as `.flux/todo/CHILD_STATUS_EVENTS.md` §3 asked)
- **Defect.** `extension/host/native_impl.rs`'s `AgentEnd` arm awaits `MainWatchdogRuntime::handle_agent_end` — a model review bounded by `agentEndTimeoutMs` (30 s) — inside a native extension handler, and `cyrup-ext`'s dispatcher dropped any handler still running after `DEFAULT_INVOKE_BUDGET` = 5 s (`dispatch.rs`, `invoke_contained`). A review longer than 5 s was silently cut: no finding, no warning, and every statement after it in the arm (the headless auto-drain, the goal scan, the fleet repaint, the herdr notifier) skipped. UW-4's integration test could not see it: its scripted provider answers instantly. Upstream has no per-handler budget (`coding-agent/src/core/extensions/runner.ts:805-811`).
- **Fix** (`343977d`, wired in UW-3's commit). `cyrup_ext::native::SanctionedWaitGate` generalises P-3's `HumanWaitGate` (whose names stay as aliases): a handler declares a bounded long wait with `HostCtx::begin_sanctioned_wait(SanctionedWaitKind::ModelReview, ceiling)`, and the dispatcher extends that one handler's budget for the guard's lifetime, up to its ceiling; every undeclared handler keeps the 5 s hang protection. `MainWatchdogRuntime::handle_agent_end_in_handler` declares `lsp.timeoutMs + agentEndTimeoutMs + 5 s`. **[CYRUP-DELTA]**: kept the protection upstream lacks, fixed what it breaks; not an `AgentEnd` exemption, not a review moved off the handler.
- **Proof.** `crates/cyrup-it/tests/subagents/watchdog_review_budget_integration.rs::the_main_watchdogs_slow_review_is_not_cut_by_the_dispatch_budget` delays the review response 6.5 s and asserts its scripted finding reaches the runtime; RED with the call site reverted to the unguarded `handle_agent_end`. The same file covers the child's review.

**~~UW-4 · Watchdog review never runs a model turn — every review is silently clean~~** — ~~*medium*~~ **CLOSED**
- **CLOSED 2026-09-18.** `watchdog::review::ModelTurnReviewAgent` runs the real nested `cyrup_agent::Agent` and is bound in BOTH production paths: `watchdog/register_main.rs:177` (the orchestrator's review, reached from `extension/host/mod.rs:259` `SubagentsExtension::new`, driven by `MainWatchdogRuntime::review_delta`, `watchdog/runtime.rs:1301`) and `prompt_runtime.rs:2491`'s `child_watchdog_review` (reached from `crates/cyrup/src/session_launch.rs:130`). The shared construction is the new `watchdog/agent_turn.rs`, pinned to **v0.68.0** — the file it serves stays pinned to v0.43.0 and every new citation says which tag it means
- **Reachability, not machinery.** `crates/cyrup-it/tests/subagents/watchdog_model_turn_integration.rs` drives `create_harness_with_extensions` with the REAL `SubagentsExtension`, arms the watchdog through the real `/subagents-watchdog session on` handler, runs a turn that writes a file, and asserts that a `watchdog_warn` call made by the nested review turn arrives at `MainWatchdogRuntime`'s `last_warning` carrying SCRIPTED summary/evidence/recommendedAction strings — data that can only have crossed a real model turn. It also asserts the nested turn's wire request offers exactly `{read, grep, find, ls, watchdog_warn}`. Both tests were run against a deliberately gutted `run` (`Ok(Vec::new())`) and FAILED
- `/subagents-watchdog status`'s "real model review" (`register_main.rs:191`) is now TRUE, and the integration test asserts the string rather than trusting it
- `NoTurnReviewAgent` survives as a `#[cfg(test)]` fixture for the ~20 unit tests written against it; it is bound nowhere in production and is not `#[allow(dead_code)]`-ed
- upstream: `src/watchdog/review.ts:350 @v0.68.0` — `await agent.prompt(buildReviewPrompt(request, selection))` inside `runWatchdogAttempt` (`:268-363`), which `createMainWatchdogReview` (`:253`) calls. (The seed's `:295`/`:249` are v0.43.0 numbers.)

**~~UW-5 · Watchdog permission arbiter never runs a model turn — every `ask` denies~~** — ~~*medium*~~ **CLOSED**
- **CLOSED 2026-09-18.** `watchdog::permission_arbiter::ModelTurnPermissionAgent` runs the real nested turn and is bound at `prompt_runtime.rs:2445`, inside `with_permission_gate`, reached from `crates/cyrup/src/session_launch.rs:130`. **There was exactly ONE production binding, not two:** this row previously also named `prompt_runtime.rs:2858`, which is inside `#[cfg(test)] mod permission_gate_tests` (`:2710-2866`) — the assertion body of `a_child_with_no_policy_installs_no_gate`, not a production site
- **Two seams had to be built for the outcome to be reachable, and both are load-bearing:**
  - `WatchdogPermissionTurn`/`WatchdogPermissionRequest` now carry the LIVE session context (pi `request.ctx`, `permission-arbiter.ts:96 @v0.68.0`), threaded from `PermissionGate` through `with_permission_gate`. Without it `resolve_watchdog_review_model` has no model in the default configuration and every ask still denies, with a different sentence
  - `HostServices::registered_provider(provider_id)` (`cyrup-ext/src/host/services.rs`, implemented over the live `ProviderSwap` in `cyrup-session-svc/src/host_services.rs`) is pi `ctx.modelRegistry.getRegisteredProviderConfig` (`permission-arbiter.ts:99-104`, `review.ts:276-281` @v0.68.0). It is what lets a subagent-internal call stream through the session's own transport — and what makes both reachability suites run offline
  - each was mutated to its pre-change value and the approve test FAILED in both cases
- **Reachability:** `crates/cyrup-it/tests/subagents/watchdog_permission_arbiter_integration.rs` builds the child runtime through the real `prompt_runtime_extension_from` over an armed policy + child-watchdog config, inside a real session, and asserts (a) an `ask`-tier `write` is APPROVED by a scripted `watchdog_permission_decision` call and actually RUNS, with the audit's decision record carrying the model's own reason; (b) a `deny` blocks with the MODEL's sentence; (c) a provider error still blocks with `approved:false` and `decision:"error"`; (d) a turn that calls no tool still denies as `malformed`. Gutting `decide` to `Ok(None)` fails (a), (b) and (c)
- **Upstream drift adopted, deliberately.** `permission-arbiter.ts`'s `Promise.race` was restructured between v0.43.0 (which the file was ported from) and v0.68.0: the timeout arm now resolves `timeout` WITHOUT the `failed closed:` prefix (`:136`), a mid-turn cancel resolves `cancelled` instead of `error` (`:138`), and a `completed` latch makes `finish` idempotent (`:49,:52-53`). All three still deny. The module doc's fail-closed table and the test that pinned the old timeout string were updated in the same change and say so
- `NoDecisionPermissionAgent` survives as a `#[cfg(test)]` fixture for the arms it honestly models; it is bound nowhere in production
- upstream: `src/watchdog/permission-arbiter.ts:43` (`createWatchdogPermissionArbiter`), `new Agent({…})` at `:111`, exported as `requestWatchdogPermission` at `:156` — all @v0.68.0. (The seed's `:41`/`:102`/`:145` are v0.43.0 numbers.)
- **Still open, named rather than glossed:** `PermissionGate::evaluate` passes `cancel: None` to `request_watchdog_permission`. Upstream races the turn against `request.signal` and `ctx.signal` (`:139-140 @v0.68.0`); cyrup's `tool_call` hook carries neither token, so the arbiter's own `agentEndTimeoutMs` bound is the only stop. It fails closed, so the residual costs latency on an aborted child, never an approval. There is a `[CYRUP-DELTA]` at the call site saying exactly this

**~~UW-6 · Nothing ever ships a permission policy to a child — the child-side gate is inert~~** — ~~*medium*~~ **CLOSED**
- **CLOSED, re-greped 2026-09-16 at `cc7818b`.** `exec/spawn_plan.rs:1178` carries the comment "SUBA-073 — pi ships the resolved permission policy to the child in `PERMISSION_POLICY_ENV`" and `:1202` writes `crate::watchdog::permission_arbiter::PERMISSION_POLICY_ENV` into the child env overlay; `:3202-3252` *(was `:3171-3252`, refreshed 2026-09-22)* is the test that pins it reaching the child as `CYRUP_SUBAGENT_PERMISSION_POLICY`. The run-level, fully-merged policy is carried onto the runner at `extension/executor/background.rs:625` (`RunnerConfig`, "SUBA-073 — the run-level, fully-merged permission policy the orchestrator resolved"), and `exec/external_cli/env.rs:189-205` deliberately withholds it from an external-CLI child. **The "only writer workspace-wide is a test stub" claim below is false at HEAD.**
- **What this closure did NOT do — read it with UW-5.** The child-side gate received a policy and could reach the `ask` tier, but the arbiter that would decide an `ask` was still `NoDecisionPermissionAgent`, so every `ask` still denied. **That escalation is discharged as of 2026-09-18: UW-5 is CLOSED and an `ask` can now be approved.** **The body below is the original filing and is kept as history.**
- upstream: `src/runs/shared/permissions.ts:40` (`resolvePermissionRules`) and `:51` (`encodePermissionRules`), written into the child env at `src/runs/shared/pi-args.ts:730` and `:758`
- cyrup: `exec/mod.rs:1376` (`build_attempt_spawn_plan`) builds the child env overlay through `~:1874` — structured-output vars, `TOOL_BUDGET_ENV` (`:1840-1846`), steer inbox, supervisor channel, required child tools — but never `CYRUP_SUBAGENT_PERMISSION_POLICY` / `…_PERMISSION_AUDIT_PATH` (defined at `watchdog/permission_arbiter.rs:358`, `:361`). The only writer workspace-wide is a test stub at `prompt_runtime.rs:1949-1950`; the reader is `prompt_runtime.rs:1716-1717`
- observable: `with_permission_gate` always receives `None`, so no child tool is ever checked against agent/config permission rules and no `permission.request`/`permission.decision` audit record is ever appended. `permission_arbiter.rs:56-63` admits this in-tree.

**~~UW-7 · The fleet-status widget receives no keystrokes~~** — ~~*medium*~~ **CLOSED 2026-09-22 on `claude/subagents-fleet-input`**
- **CLOSED — and the row's central claim was FALSE WHEN IT WAS WRITTEN, not merely stale.** It said *"The host seam is still absent — `cyrup-ext/src/host/services.rs` has nothing resembling `on_terminal_input`"*. `host/services.rs` is the **`HostServices`** trait — the host's EFFECTS (`notify`, `set_widget`, `open_overlay`, `editor_text`). An inbound per-keystroke callback was never going to live there: it is a method on the **extension**, and the dispatcher is **`ExtensionHost::terminal_input`** (`cyrup-ext/src/facade.rs:1486`). The seam existed end to end and was tested across all four tiers — `InitApi::subscribe_terminal_input` (`native.rs:425`), `NativeExtension::on_terminal_input` (`native.rs:775`), `ExtensionRegistry::terminal_input_subscribers` (`registry.rs:613`), the fold (`facade.rs:1486-1546`), `LiveExtension::on_terminal_input` (`host/live.rs:1980`), `on-terminal-input` in the WIT (`wit/world.wit:388`), the SDK's guest macro (`cyrup-ext-sdk/src/macros.rs:118-128`), and five dispatch tests (`cyrup-ext/src/tests/native_dispatch.rs:1568-1760`). Area 06 recorded `EXT-021`'s closure on **2026-08-14** and closed the row **2026-08-15** (`06-cyrup-ext.md:399`); this row was re-greped **2026-09-16** — a month later — and still reported the seam absent. The grep that would have caught it is `git grep -n 'terminal_input' -- crates/cyrup-ext`, ~50 hits across 9 files
- **What was actually missing was two wires, and they are now in.** (1) `crates/cyrup-tui` never called the dispatcher; it does now, from `App::on_input_event` (`app/run_action.rs:281-305`) through `App::offer_input_to_extensions` (`app/terminal_input.rs`), which is pi's own position — the listener fold completes BEFORE the focused component is offered the key (`packages/tui/src/tui.ts:773-788` @v0.83.0). (2) `cyrup-ext-subagents` never subscribed; `init` now declares `api.subscribe_terminal_input()` under the `fleetView` gate (`extension/host/native_impl.rs:276-284`) and `NativeExtension::on_terminal_input` (`:1040`) routes to `extension/host/terminal_input.rs`
- **Three things the wiring needed, all built:**
  - **One `KeyEvent` ⇄ bytes table**, in `cyrup-ext` beside the seam's own contract types (`contract.rs`: `TerminalKey`, `TerminalKeyEvent`, `TerminalKeyModifiers`, `encode_terminal_key`, `decode_terminal_key`). The seam carries raw terminal bytes because pi's does, and crossterm parses them away at the reader (`cyrup-tui/src/app/input_reader.rs`), so both directions are needed — and both live in ONE table with a round-trip test over every variant, because two hand-written halves in two crates that must agree byte for byte is a defect waiting to happen. The encoder emits one canonical form (the legacy sequences, which is what `native_dispatch.rs` already feeds); the decoder also accepts SS3 application-cursor keys, kitty CSI u (including flag-4 alternate-key fields and event types), xterm `modifyOtherKeys`, and Enter's `\n`/`\x1bOM` — each an alternate pi's own `matchesKey` accepts (`packages/tui/src/keys.ts:820`)
  - **`HostServices::editor_has_focus`** (`cyrup-ext/src/host/services.rs:358`), pi's `editorHasFocus()` guard, backed by an `EditorFocusMirror` on `LiveHostServices` (`cyrup-session-svc/src/host_services.rs`) that `App::publish_extension_readbacks` republishes — symmetric with `editor_text` (SEAM-T02) in every respect. **It reports which COMPONENT the key path routes to, not DEC `?1004` window focus:** upstream's `focusedComponent` is independent of the window, and publishing window focus would leave the roster expanded behind an open selector, which is the exact state pi's guard exists to end
  - **`Enter` spawns.** `FleetStatusKeyOutcome::OpenInspector` reaches `open_overlay`, whose contract is *"BLOCK until the user closes it"* (`host/services.rs:332`), and the handler runs on the only task that can deliver that close. Upstream's detached `void Promise.resolve().then(() => this.openInspector(selectedKey))` (`tui/fleet-status.ts:741-750` @v0.68.0) is load-bearing for the same reason; `spawn_fleet_inspector` is it, and the prohibition is now written on `NativeExtension::on_terminal_input` itself
- **Every line number in the old row had drifted, and is corrected here:** `handle_key` is `tui/fleet_status.rs:922` (row said `:740`), `press` `:1457` (said `:1169`), `is_widget_registered` `:869` (said `:711`). `extension.rs` no longer exists as one file — the publish half is `extension/host/slash.rs:196` (`refresh_fleet_status_widget`) → `set_widget` at `:265`/`:271` (the row's `extension.rs:9489`/`:9889`/`:9978` are unresolvable). The upstream coordinates `fleet-status.ts:282-283` / handler `:352` are correct **at v0.43.0**, the tag the cyrup port targets; at the pinned **v0.68.0** they are `:577-578` and `:696`
- upstream: `src/tui/fleet-status.ts:577-578` @v0.68.0 (`ui.onTerminalInput((data) => this.handleKey(data))`), handler `:696-753`; `editorHasFocus` `:965-976`; `matchesKey` `pi/packages/tui/src/keys.ts:820` @v0.83.0
- cyrup: fold `cyrup-tui/src/app/terminal_input.rs`; handler `cyrup-ext-subagents/src/extension/host/terminal_input.rs`; parser `tui/fleet_status.rs::FleetStatusKey::from_terminal_data`; table `cyrup-ext/src/contract.rs`
- observable: while subagents run, `↓`/`←` on an empty editor now expands the widget into a selectable roster; `j`/`k`/`↑`/`↓` move the selection, `Enter` opens the fleet inspector on the highlighted run, `Esc` (or `↑` past the top) collapses it, and any other key falls through to the editor unchanged
- **Does NOT close `SUBA-026`.** That row's open half is the interactive `/subagents` admin selector, mounted through `ctx.ui.custom` (`src/slash/subagents-admin.ts:216-219`), not `onTerminalInput` — `git -C tmp/pi-subagents show v0.68.0:src/slash/selector.ts | grep onTerminalInput` is empty. Its blocker is porting `SelectorComponent` as an `InteractiveOverlay`, which `open_overlay` already supports

**~~UW-8 · Mission workflow state is never written~~** — ~~*small*~~ **CLOSED**
- **CLOSED, re-greped 2026-09-16 at `cc7818b`, citations refreshed 2026-09-22 at `14e6c56`, and it closed exactly the way this row predicted.** `extension/tool/routing.rs:606` constructs `crate::missions::MissionWorkflowStateStore::create(…)` (`missions/workflow_state.rs:292`) on the production workflow-launch path; that store wraps `create_mission_workflow_state` (`:248`) and implements `workflows::scripted::WorkflowStateStore` (`:300`), so the scripted runtime's `state.get` / `state.set` now reach the real file. **"Closes for free the moment VL-S2 lands" — VL-S2 landed, and it did.** The read half (`mission_state_path` from `missions/goal_driver.rs` and `missions/actions.rs`) was already live and now finds a file that can exist. **The body below is the original filing and is kept as history.**
- upstream: `src/missions/workflow-state.ts:17` (`missionStatePath`); its only consumer is the `workflowScript` runtime at `runs/foreground/subagent-executor.ts:4139`, which exposes `state.get`/`state.set`
- cyrup: `missions/workflow_state.rs:209` (`get`) and `:222` (`set`) have no non-test caller. The **read** half IS live — `mission_state_path` is called from `missions/goal_driver.rs:372` and `missions/actions.rs:908`
- observable: `<missionDir>/<missionId>/state.json` is never written, so `goal_driver`'s production read always finds a missing file and falls through to the decisions list, and `mission.show` advertises a path that never exists. **Closes for free the moment VL-S2 lands.**

**~~UW-9 · The yolo-mode runtime API has no publish seam and no caller~~** — ~~*medium*~~ **CLOSED** · = area 10 `PERM-011` half A, closed there 2026-08-15
- **CLOSED, re-greped 2026-09-16 at `cc7818b` — and the closure is a DECISION, which is why the symbol still has no external caller.** Area 10 established that upstream's "publish seam" is not a host registry at all: `registerPiPermissionSystemRuntimeApi` writes one object into `globalThis.__piPermissionSystem` (`yolo-mode-api.ts:20-43` @v0.8.0), a process-global single slot. The false in-tree doc this row's last sentence calls out is **fixed**: `crates/cyrup-permission-system/src/extension/command.rs:57-60` now states plainly that an earlier revision routed the `/permission-system` yolo row through `set_yolo_mode` *"so that method would have a caller"*, and that this was reverted rather than kept — i.e. the code refuses to change behaviour to satisfy a reachability rule, and says so. `yolo_api.rs:22-24` records the same. **A reader checking this row by grepping for callers will still find none; that is the answer, not the defect.** **The body below is the original filing and is kept as history.**
- upstream: `src/index.ts:1480-1484` @v0.8.0 (`registerPiPermissionSystemRuntimeApi({getYoloMode,setYoloMode,toggleYoloMode})`); `src/yolo-mode-api.ts:23-29` publishes it on `globalThis.__piPermissionSystem`, `:40-43` reads it back. `git diff v0.7.1..v0.8.0 -- src/yolo-mode-api.ts` is empty
- cyrup: `extension.rs:608` (`yolo_mode`), `:628` (`set_yolo_mode`), `:693` (`toggle_yolo_mode`). `set_yolo_mode` has exactly one caller — `:694`, inside `toggle_yolo_mode` — and `toggle_yolo_mode` has none. The `/permission-system` yoloMode row deliberately routes through `save_extension_config` instead (`:783-790`). `crates/cyrup-ext/src/native.rs:318` (`trait NativeExtension`) exposes `id`/`init`/`on_event`/`execute_command` — no way to publish a callable API object
- observable: the three methods compile, are documented and tested, and cannot be invoked in production. **And `yolo_api.rs:16` claims they are "reached through the `/permission-system` command", which `extension.rs:721-728` contradicts** — the same doc-asserts-wiring-that-does-not-exist pattern as `PERM-014`. Unlike PB-16 this needs a new seam: `SharedBus` is an event bus, not a callable-API registry.

**UW-10 · The intercom compose and session-picker overlays are render-only** — ~~*medium*~~ *low* · **STILL OPEN at `14e6c56`; NARROWED, re-rated and citations refreshed 2026-09-22**
- **Re-greped this pass:** `open_overlay` is still never called from `crates/cyrup-intercom/`, and `ui/mod.rs:29` still says so in-tree — *"…is likewise not reachable yet. The overlays' interactive `handle_input` state machines are ported faithfully and…"*. The row stands
- **Cross-reference corrected — this row's Fix pointer was wrong.** It says "Fix the comment with `ICOM-024`/`ICOM-028`". **Both are now CLOSED and neither touched overlay reachability**: `ICOM-024` (closed 2026-09-04) landed `register_message_renderer` for the inbound message card, and `ICOM-028` (closed 2026-08-14, REFUTED) resolved the `intercom_message` entry surface through `render_entry`. Renderers, not overlays. **UW-10 now has no owning id in area 11's open set** (`ICOM-052`, `ICOM-057`, `ICOM-061`) — it needs one filed by that area's owner, and this pass may not file it there
- upstream: `pi-intercom/index.ts:1857` (`new SessionListOverlay(...)`) and `:1874` (`new ComposeOverlay(...)`), both handed to `ctx.ui.custom`; classes at `ui/session-list.ts:44` and `ui/compose.ts:13`
- observable: `/intercom <target>` prints a picture of a compose box and asks the user to retype the whole command with a body.
- **Rationale correction, second pass (2026-09-22) — the previous correction is itself stale.** The row said "only the `alt+m` path stays blocked by VL-S15". **`VL-S15` is CLOSED**: `InitApi::register_shortcut` is `cyrup-ext/src/native.rs:406`, dispatched `facade.rs:501-502`, and `HostServices::open_overlay` is `host/services.rs:332`. **Nothing outside this crate blocks the overlay path any more.** What blocks it is three call sites inside `cyrup-intercom` that were never written: `IntercomExtension::init` (`extension.rs:538-611`) registers tools, renderers, commands and bus subscriptions and never calls `register_shortcut`; nothing calls `open_overlay`; and three in-tree comments still assert the seam is absent — `extension.rs:565` ("cyrup has no `register_shortcut`"), `ui/compose.rs:9-10` and `ui/session_list.rs:7` ("the Phase-6 `register_shortcut`/overlay-host gap"). **All three comments are false at HEAD and must be fixed in the same change as the code.** Re-rate `low`: this is wiring inside one crate, not a seam
- cyrup: `ui/compose.rs:109` (`handle_input`), `:74`, `:80` and `ui/session_list.rs:88` have zero production callers; `open_overlay` is never called from this crate (`grep -rn open_overlay crates/cyrup-intercom/` → nothing). The only production use is a one-shot `render` at `extension.rs:405-420` whose output ends "Type `/intercom {target} <message>` to send." *(citations refreshed 2026-09-22; `register_message_renderer` is now `cyrup-ext/src/native.rs:370`)*

**~~UW-11 · Copilot and Codex login flows are fully written and unreachable~~** — ~~*high*~~ **CLOSED — REFUTED** · = area 01 `PROV-029`, closed there 2026-08-14 (sweep 2)
- **CLOSED as REFUTED-at-HEAD, re-checked 2026-09-16 at `cc7818b`.** Area 01 established that `providers/github_copilot.rs:157` wires `GitHubCopilotLogin` (the flow that HAS `login`) with an explanatory block at `:141-146`, and `providers/openai_codex.rs:137` does the same for the Codex flow — i.e. the one-field-assignment fix this row proposed had already landed when the row was re-read. **The second half of this row's Fix is NOT discharged and is not tracked anywhere**: "either populate the flow registry or delete it". **Re-greped 2026-09-22 at `14e6c56` and the answer is: zero.** `register_bundled_oauth_flow_loaders` (`auth/oauth/load.rs:111`) has no production caller — its only calls are `:239`, `:277` and `:322`, all inside the `#[cfg(test)]` block that opens at `:198` — and the registry is dead on the read side too: `registered_oauth_flows` (`:118`), whose own doc says it exists "for status UIs that list which logins are actually available", has **zero** callers outside the module. The backing `registry()` (`:104`) is written by nobody. **This is a live UW-class defect at `low`, now filed as area 01 `PROV-072`.** **The body below is the original filing and is kept as history.**
- upstream: `providers/github-copilot.ts:16` and `openai-codex.ts:**13**` @v0.83.0 both carry `lazyOAuth({… load: load*OAuth })`. **Citation corrected this pass**: the previous edition cited `openai-codex.ts:15` (that line is `models:` at v0.83.0), and elsewhere quoted an `isSubscription: true` property from `github-copilot.ts:16` that **does not exist at v0.83.0 at all** — it is a v0.84.1 addition. Both sides re-read at the tag
- cyrup: two Copilot OAuth types exist — `GitHubCopilotLogin` (`auth/oauth/github_copilot.rs`, real `login` at `:821`) and `GitHubCopilotOAuth` (`providers/github_copilot.rs:410`, refresh/to_auth only) — and `github_copilot_auth()` (`providers/github_copilot.rs:142-146`) wires the **second**. Same shape for Codex: `openai_codex_auth()` (`providers/openai_codex.rs:129-131`) wires `OpenAiCodexOAuth`, not `OpenAiCodexOAuthFlow` (`auth/oauth/openai_codex.rs:516`). `/login` resolves through `provider.provider_auth().oauth` (`cyrup-config/src/login.rs:784`), so both dead-end on `LoginUnsupported` (`auth/mod.rs:124-131`). `providers/builtin_oauth.rs:37-56` has four arms and a prose exemption at `:14-16`; `register_bundled_oauth_flow_loaders` (`auth/oauth/load.rs:111`) has **zero production callers**
- observable: `/login` advertises both providers — with the subscription marker, since both `is_subscription` returns true — and dead-ends. Two complete, tested login flows ship in the binary and cannot be reached. Fix is one field assignment per provider; separately, either populate the flow registry or delete it.

**~~UW-12 · Compaction cannot be cancelled from the shipped binary, and the indicator advertises the dead key~~** — ~~*high*~~ **CLOSED — REFUTED** · area 03 `SESS-040` struck 2026-08-15; whole chain re-verified at `14e6c56` 2026-09-22 · (+ `SESS-041`, `SESS-042`)
- upstream: `modes/interactive/interactive-mode.ts:3074-3085` @v0.83.0 rebinds Escape on `compaction_start`, restoring it at `:3088-3095`
- cyrup: that rebind was never ported — the `CompactionStart` arm (now `app/events_fold.rs:195-223`) handled it by setting the indicator and nothing else; `AbortCompaction` (`cyrup-session-svc/src/command.rs:32`, `:116-118`) had **zero production callers**, `AgentSession::abort_compaction` likewise; and the indicator band (now `app/render.rs:86-90`) prints "(esc to cancel)"
- **CLOSED — do not schedule.** Area 03 struck `SESS-040` as REFUTED on 2026-08-15 and the whole chain is live at `14e6c56`: `app/events_fold.rs:284` arms `state.compacting` beside the band text at `:266`; `app/input.rs:295` returns `AppAction::AbortCompaction` on Escape ahead of pi's four-branch chain, exactly as `interactive-mode.ts:3080-3086` shadows it; `app/run_action.rs:91-92` calls `ctx.session.abort_compaction()` (`cyrup-session-svc/src/session/compaction.rs:406`), reached from the command enum at `command.rs:34`/`:151-152`. Pinned by `cyrup-tui/src/tests/escape_chain.rs:276`/`:287`; the band's "(escape to cancel)" is pinned at `tests/compaction_status.rs:133` — note the suffix is composed from the live keymap, not a literal in `app/render.rs` (see the comment at `render.rs:98`). `REPRO-LOG.md`'s "one of seventeen still open" is corrected in the same direction *(every citation in this bullet refreshed 2026-09-22; the row's `events_fold.rs:195-223`, `input.rs:144-146`, `run_action.rs:53-54`, `session.rs:1900`, `command.rs:32`/`:116-118`, `render.rs:86-90` and `escape_chain.rs:233`/`:244` are all stale)*
- **`SESS-041`/`SESS-042` were NOT re-verified this pass** and stay exactly as the body leaves them — stated as unknown, not as open.
- observable: the provider call bills and `append_compaction` mutates the session file regardless of what the user presses. Two adjacent defects make it worse and **must ship together**: `abort_compaction()` never cancels an AUTO compaction (`SESS-041`), and there is no abort re-check before `append_compaction`, so even a cancelled compaction is written (`SESS-042`) — both are latent *only* because this has no caller. **Verification must include a live terminal run**, not a driven event loop (standing TUI rule).

**~~UW-13 · `ExtensionManifest.capabilities` is parsed and never read — the per-extension WASM sandbox grant is inert~~** — ~~***critical***~~ **FIXED 2026-08-13** · = area 06 `EXT-054` (+ `EXT-055`) · re-verified at `14e6c56` 2026-09-22

> **FIXED 2026-08-13.** See `06-cyrup-ext.md` `EXT-054`/`EXT-055` for the full evidence block. Summary: `load_discovered` passes `disc.manifest.capabilities` into a new `ExtensionHost::load_wasm_with_caps`; `GuestState` carries the grant as DATA and the host enforces it at the `exec`/`proc`/`http-client`/`ui` import boundary and through `FsCaps` for `ext-fs`; `load_wasm` keeps its signature as the manifest-less host-internal entry. 9 new tests, 5 RED before a one-line revert.
>
> **Re-verified 2026-09-22 at `14e6c56`, and the header is struck with it — this row was the register's only `critical` and it has been finished since August, so anyone scanning headers to choose work was being sent here.** `load_discovered` passes `&disc.manifest.capabilities` into `load_wasm_with_caps` at `facade.rs:2129` (definition `:1938`); `load_wasm` (`:1915`) survives as the manifest-less host-internal entry and delegates with `Capabilities::host_granted()` at `:1921`. **Deny-by-default held**: both loader synthesis sites are now the EMPTY grant — `loader.rs:345` and `:426` are `Capabilities::none()`, not `Default::default()`. The manifest field is `manifest.rs:46`, the shape `:62`. **One citation in the body below names a symbol that no longer exists**: `FsCaps::with_fs_root` is gone workspace-wide (`grep -rn 'with_fs_root' crates/ --include=*.rs` → no output); `ext-fs` roots now come from `FsCaps::single` (`host/services.rs:1782`) and `FsCaps::from_grants` (`:1800`), with the grant refusals at `:1446`, `:1469`, `:1473`. **No lookalike is substituted for the row's symbol — the grep and the absence are what is recorded.** `loader.rs:56-59`'s coarse trust check was not re-read this pass.
- upstream: **none, and that is the point.** pi has no capability model; every TypeScript extension runs with the whole process's authority. This is a divergence from **cyrup's own** security design, which is why a pi-anchored item-driven sweep cannot see it
- cyrup: `manifest.rs:20` declares the field and `:23-35` the `Capabilities { fs, exec, net, ui }` shape; `loader.rs:213`/`:259` synthesise defaults; **there is no consumer anywhere.** Re-verified at HEAD this pass: `load_discovered` (`facade.rs:1166-1184`) holds `disc.manifest` and calls `self.load_wasm(id, &bytes, services)`, whose signature (`facade.rs:1063-1070`) is `(id, bytes, services)` — the manifest provably cannot reach instantiation, so `capabilities.{exec,net,ui}` narrow nothing. `FsCaps::with_fs_root` likewise has zero callers (`EXT-055`), so `ext-fs` is permanently denied for every guest
- observable: every loaded guest gets the full host surface, gated only by the coarse `origin.is_pre_trust() || project_trusted` check (`loader.rs:56-59`) — while `manifest.rs:2`, `host/store_state.rs:1-3`/`:20-22` and ADR-0002 all document a capability-scoped sandbox. **Critical because README's definition of critical carries no reachability qualifier**: a permission bypass is a permission bypass. Blast radius *is* bounded — zero WASM guests ship today — and that is scheduling information, not a rating: **land it before the first third-party component, not after.** Deny-by-default is part of the fix: the loader's two `capabilities: Default::default()` synthesis sites must stay the EMPTY grant.

**~~UW-14 · SINGLE-mode `outputSchema` is unadvertised, so the structured-output channel is unreachable~~** — ~~*high*~~ **CLOSED** · = area 09 `SUBA-043`
- **CLOSED, re-greped 2026-09-22 at `14e6c56`, and both halves are dead.** `outputSchema` is advertised **top-level**: `extension/tool/schema.rs:602` inserts it into the root props with a `[CYRUP-DELTA]` description, and the comment block at `:592-601` narrates this row's own defect in the past tense. Both constructors now carry the real value instead of a pinned `None` — `extension/executor/foreground.rs:1182` (`structured_output_schema: overrides.output_schema.clone()`, inside the production `run_foreground_impl`, `:299`) and `extension/executor/background.rs:238`; the param is parsed at `extension/executor/requests.rs:115`/`:214` and pinned by `background.rs:1563` plus `extension/tool/routing_tests.rs:621-631`. The general fix this row asked area 09 for also landed: `schema.rs:1629` is the advertised-property-has-a-consumer guard, iterating `["outputSchema", "toolBudget"]`. **The `extension.rs:6543-6690` / `:1934` / `:2295` citations below cannot be refreshed — that file no longer exists; the crate was split into `extension/`.** **The body below is the original filing and is kept as history.**
- upstream: `src/extension/schemas.ts:349` @v0.43.0 has `outputSchema` **top-level**
- cyrup: it is not among the 45 props at `extension.rs:6543-6690`, and `structured_output_schema` is hardcoded `None` at `:1934` and `:2295` — the runner already carries the field; only the two constructors pin it
- observable: the schema is silently dropped and the run returns prose. **`SUBA-S01` was closed to deliver exactly this channel** — the transport exists and the surface a model calls does not expose it. Area 09 names the general fix: a schema/dispatch guard asserting every advertised property has a consumer, which would have caught this, `SUBA-047` and `SUBA-N05` as a class.

**~~UW-15 · Concurrent-duplicate ask collapse is implemented in `dedup.rs` and never wired~~** — ~~*medium*~~ **CLOSED** · = area 10 `PERM-014`, closed there 2026-08-14 (sweep 1)
- **CLOSED, re-greped 2026-09-16 at `cc7818b`.** `crate::dedup::DedupCache` is constructed at `crates/cyrup-permission-system/src/extension/construct.rs:198` (imported `:14` — the row's `:14` was the `use`, not the construction; corrected 2026-09-22) and `DedupDetails` is consumed on the live decision, event and audit paths — `extension/decide.rs:12`, `extension/events.rs:6`, `extension/audit.rs:7`. Pinned by `two_concurrent_identical_asks_collapse_to_one_prompt`. The "in-tree docs read as though it were wired" complaint is discharged: `dedup.rs`'s module doc is now true. **The body below is the original filing and is kept as history.**
- cyrup: the module exists and is tested; nothing in the live gate path calls it, and the in-tree docs read as though it were wired
- observable: two concurrent identical asks each prompt the user, where pi collapses them onto one decision. (Examined and deliberately **not** raised this pass: a double-ask is not an unasked approval.)

**~~UW-16 · Implemented-and-unadvertised, subagents edition~~** — ~~*medium*~~ **CLOSED 2026-09-22 — all three shipped**
- **`SUBA-047` CLOSED** (area 09, 2026-08-14): the top-level `toolBudget` is advertised; a per-call budget from an orchestrator is no longer discarded. Its own residual — the PER-ITEM `toolBudget` override on `tasks[]`/`chain[]` — was deliberately left unadvertised because advertising without a consumer is this very defect class
- **`SUBA-046` CLOSED — REFUTED** (area 09, 2026-08-14): `exec/spawn_budget.rs` exists, the `grant-spawn-budget` verb is in the advertised list, and the SUBA-064 authority consult is live
- **`SUBA-054` CLOSED, re-greped 2026-09-22 at `14e6c56`** — the last third, and the whole of what this row still owed. The persona's `defaultReads` now reaches a SINGLE run: `extension/executor/foreground.rs:1178` sets `reads: agent.default_reads.clone()` inside the production `run_foreground_impl` (`:299`), and `exec/spawn_plan.rs:1414-1426` composes pi's `task = readsInstruction + task` (`subagent-executor.ts:3873`) through `build_single_reads_instruction`, so `[Read from: …]` is prepended outside chains. Pinned by `spawn_plan.rs:1991` — whose header records "RED before the fix: `RunOptions` had no `reads` field at all" — and by the missing-file variant at `:2012-2032`. ~~*"This is the whole of what UW-16 still owes, and it is one of only three mediums left in area 09"*~~ — both clauses are now spent; area 09's table row is struck in the same pass
`toolBudget` is fully enforced (`exec/tool_budget.rs`, 388 lines; `TOOL_BUDGET_ENV` written at `exec/mod.rs:1837-1846`) and **not advertised** on the tool schema, so a per-call budget from an orchestrator is silently discarded (`SUBA-047`). `defaultReads` is parsed and rendered and **never reaches a single run**, so `[Read from: …]` never appears outside chains (`SUBA-054`). And the inverse shape: `grant-spawn-budget` is **advertised and unported**, so an exhausted spawn cap is terminal for the session (`SUBA-046`). Area 09 found the first two *by accident*, without running a systematic hunt — which is the argument for doing one.

**~~UW-17 · Extension widgets, headers and footers reach the TUI and are stored where nothing renders them~~** — ~~*medium*~~ **CLOSED** · area 07 `TUI-014` + `TUI-033`, both closed there 2026-08-14
- **CLOSED on the strength of both owning rows.** `TUI-014` (`ui.setWidget`) and `TUI-033` (`ui.setHeader`/`ui.setFooter`) are struck in `07-cyrup-tui.md` and neither appears in that file's current open set. ~~**Not independently re-greped by this pass**~~ — **independently re-greped 2026-09-22 at `14e6c56`, so the area-file-only caveat is discharged for the widget half**: `cyrup-tui/src/app/render.rs:236` (`render_extension_widgets`) draws the stored values in mount order, called for both placements at `:155` and `:158`. The wire-shape companions this row names (`SEAM-011`, `SEAM-028`) are likewise absent from area 08's open set. **The body below is the original filing and is kept as history.**
The delivery half of `ui.setWidget` / `ui.setHeader` / `ui.setFooter` is live all the way into `cyrup-tui`, and the values land in fields no draw path reads. Related on the wire: `SEAM-011` sends `setWidget` with a cyrup-invented `{widget}` blob, and `SEAM-028` is the test pinning it.

**UW-18 · Settings rows that toggle values nothing reads** — *low* · **PARTIALLY CLOSED 2026-09-16 — the headline example shipped, the register did not**
- **`CFG-045` CLOSED, re-greped 2026-09-22 at `14e6c56`:** `doubleEscapeAction` is no longer a settings row nothing reads — `crates/cyrup-tui/src/app/input.rs:342` is the double-Escape window and reads `self.state.double_escape_action`, the field being `app/state.rs:199` with its default at `:491`. **This matters beyond the row**: `doubleEscapeAction` is the example §2's own header paragraph uses to define "a `/settings` row is not a consumer", and that example is now stale as an example while remaining correct as a lesson
- **`CFG-015` STILL OPEN** (area 05, `low`), re-greped 2026-09-22 and the two headline accessors are worse than "unconsumed" in different ways: `last_changelog_version` (`cyrup-config/src/settings/effective.rs:706`) has **zero callers of any kind workspace-wide**, and `collapse_changelog` (`:711`) has exactly **one** — `cyrup-tui/src/app/settings_rows.rs:181`, a `/settings` row, which is the very thing this section's header says does not count. `collapseChangelog` is PB-6's home
- **`CFG-044` CLOSED** (absent from area 05's open set; not independently re-greped this pass)
- The adjacent list in the body below (`AGENT-031`, `CFG-006`, `AGENT-S03`, `SESS-033`, `PROV-032`, `SEAM-048`, `TOOL-015`, `EXT-024`, `EXT-025`) was **not** re-checked row by row; of those ids only `EXT-025` is in any area's current open set
`doubleEscapeAction` is offered in `/settings` and the Escape handler has no double-escape and no bash-mode-exit branch (`CFG-045`, and `TUI-009` is its TUI half). `CFG-015` carries five unconsumed settings accessors including `lastChangelogVersion` and `collapseChangelog` (PB-6's home). `CFG-044`'s `get_auth_status` is dead. Adjacent: `AGENT-031` / `CFG-006` (`websocketConnectTimeoutMs` parsed, never reaching the HTTP layer), `AGENT-S03` (`StreamOptions.metadata` unreachable from the agent loop), `SESS-033` (`inputs_fingerprint` has no caller and its doc claims otherwise), `PROV-032` (`filter_github_copilot_models`, zero production callers), `SEAM-048` (pi's `name:N` command disambiguation is dead code), `TOOL-015` / `EXT-024` (nothing reads `render_kind`), `EXT-025` (`reload()` plus four `emit_*` facade methods).

**~~UW-19 · `keybindings.json` is read exactly once, at boot, and no other surface ever reads it~~** — ~~*medium*~~ **CLOSED** · area 07 `TUI-051`, area 08 `SEAM-067`, area 05 `CFG-048` — **all three closed 2026-08-14**
- **CLOSED on the strength of all three owning rows**, none of which is in its file's current open set: `TUI-051` (`/reload` never re-reads `keybindings.json` while its help text claims it does), `SEAM-067` (pre-launch selectors never load it and print wrong hint rows), `CFG-048` (pi's sixth startup migration, `migrateKeybindingsConfig`, 59 legacy names). ~~**Not independently re-greped by this pass.**~~ **Independently re-greped 2026-09-22 at `14e6c56` for the `TUI-051` half, so that caveat is discharged**: there are now TWO production readers, not one — boot (`crates/cyrup/src/interactive.rs:231`) and `/reload` (`cyrup-tui/src/app/execute_session.rs:316` sets `reload_keybindings_in: Some(agent_dir)`, driving `app/shell.rs:268-281`, which resets all five keymaps plus the editor before calling `load_keybindings_json` at `:281`). The body's "`crates/cyrup/src/main.rs:1626`, at boot" is stale on both counts. `SEAM-067` and `CFG-048` remain on area-file evidence.
- **The ordering constraint this row records is DISCHARGED, and that is the load-bearing part:** "`CFG-048` must precede `TUI-028`, or the `editor.*`→`tui.editor.*` namespace rename breaks every config written against shipped cyrup." `CFG-048` has landed, so `TUI-028` is unblocked. **The body below is the original filing and is kept as history.**
Three findings, one wiring hole. `/reload` (`cyrup-tui/src/app/execute_session.rs:241-268`, `rt.reload(None).await` at `:264`) calls only `rt.reload`; `load_keybindings_json` has exactly **one** non-test caller — `crates/cyrup/src/main.rs:1626`, at boot — while both the command's help text and its in-source comment claim keybindings are re-read (`TUI-051`). The **pre-launch** selectors (`--resume` picker, trust prompt, config selector) never load it at all and print hint rows naming the built-in keys (`SEAM-067`). And pi's sixth startup migration, `migrateKeybindingsConfigFile` → `migrateKeybindingsConfig` (`core/keybindings.ts:289-309`, **59** legacy names, also applied at read time at `keybindings.ts:366`), is not ported at write time or read time, so every legacy name is silently inert (`CFG-048`; `crates/cyrup/src/migrations.rs:26-33` makes four calls, pi's `runMigrations` six). **Ordering matters: `CFG-048` must precede `TUI-028`**, or the `editor.*`→`tui.editor.*` namespace rename breaks every config written against shipped cyrup.

**~~UW-20 · The faithful fuzzy matcher is ported and unused; `--list-models` hand-rolls a lossier one~~** — ~~*low*~~ **CLOSED** · area 08 `SEAM-068`, closed there 2026-08-14
- **CLOSED on the strength of the owning row**, which is struck in `08-cyrup-session-svc-and-modes.md` and absent from that file's open set. ~~**Not independently re-greped by this pass.**~~ **Independently re-greped 2026-09-22 at `14e6c56`, so that caveat is discharged**: `crates/cyrup/src/actions.rs:122` calls `cyrup_tui::fuzzy_filter` on the production `--list-models <search>` path (`list_models`, `:102`, from `list_models_action`, `:83`), pinned by `list_models_search_uses_pis_fuzzy_filter` at `:255`. **`SEAM-020` is STILL OPEN** (area 08, `low`) — the companion this row said to ship with it: `--list-models` still prints the whole compiled catalog rather than the auth-configured one, so its no-models-available branch stays unreachable. **The body below is the original filing and is kept as history.**
The port of pi's `fuzzy.ts` exists and has no caller on the `--list-models <search>` path, which uses a hand-written filter that drops matches pi returns. Same shape as UW-1: a correct port sitting beside the code that should call it. Ships naturally with `SEAM-020` (the same command prints the whole compiled catalog rather than the auth-configured one, and its no-models-available branch is therefore unreachable).

**~~UW-21 · The whole `async_status_snapshot` subsystem has no production caller — 1 928 LOC landed unwired, and the fifth instance of this class~~** — ~~*low*~~ **CLOSED**
- **CLOSED 2026-09-18, by PB-8, exactly as this row predicted** (*"This row closes for free when
  either does"*). Both halves landed, not just the RPC one:
  - `build_async_status_snapshot_for_state` is called from the `status` reply at
    `extension/rpc/mod.rs:429`
  - `encode_async_status_snapshot_widget` is called from `extension/host/slash.rs:248` (imported
    `:214`) *(was `:236`; refreshed 2026-09-22)*, the
    `ExtMode::Rpc` branch of `refresh_fleet_status_widget` — pi `renderWidget`'s RPC arm
    (`tui/render.ts:2999-3002 @v0.68.0`). It publishes under its own
    `ASYNC_STATUS_SNAPSHOT_WIDGET_KEY` (`"subagent-async"`, pi `WIDGET_KEY`,
    `src/shared/types.ts:2789`), which is a DIFFERENT slot from `FLEET_STATUS_WIDGET_KEY`
    (`tui/fleet-status.ts:14`) — upstream runs the two side by side and `renderWidget` never touches
    the fleet slot
- **The `set_widget` blocker this row named was not real.** `HostServices::set_widget` takes the key
  as its first argument (`cyrup-ext/src/host/services.rs:401`; the `LiveHostServices` side is
  `:2193`) *(was `:376`; refreshed 2026-09-22)*, so the capability existed; what was missing was a
  caller
- The module doc at `background/async_status_snapshot/mod.rs:27-48` — the ⚠ block that declared the
  unwiredness and named PB-8 as a blocker — is replaced by the real addresses. **The body below is
  the original filing and is kept as history**


- upstream: `src/runs/background/async-status-snapshot.ts`. Two callers, both read at the tags this pass opened: the RPC bridge's `status` method — `extension/rpc.ts:725` and `:749` @v0.66.0 (`:729`/`:753` @v0.67.0; at v0.68.0 the shape changed to `asyncStatusSnapshot: {kind, version}` at `:449`) — and `ctx.ui.setWidget(WIDGET_KEY, encodeAsyncStatusSnapshotWidget(jobs))` at `tui/render.ts:2863` @v0.66.0 (`:2984` @v0.67.0, `:3000` @v0.68.0)
- cyrup: `crates/cyrup-ext-subagents/src/background/async_status_snapshot/` — 4 files, **1 928 LOC** (`mod` 202, `project` 1 153, `state` 205, `types` 368), landed in `7e41cf9`. Workspace-wide, the ONLY references outside the module are `background/mod.rs:48` (`pub mod`) and one test — `extension/tool/routing_tests.rs:2040` `a_workflow_host_step_reaches_the_async_status_snapshot`, which calls `build_async_status_snapshot` at `:2088`. `grep -rn 'PI_SUBAGENT_ASYNC_JSON' crates/` finds the constant and no emitter
- observable: **nothing**, and that is the point — the `PI_SUBAGENT_ASYNC_JSON:` widget line pi emits while async runs are in flight is never produced by cyrup, so any reader keying on that prefix sees nothing, and the projection's correctness cannot be observed from the shipped binary
- **This is the fifth time this programme has shipped tested machinery with no production caller** (UW-1, UW-8, UW-15 and UW-4/UW-5 are the precedents — UW-4/UW-5 are now CLOSED with production callers and mutation-checked reachability tests; see their rows), and the second time the shipping batch also wrote the test that proves the machinery works in isolation — `routing_tests.rs:2019` is even headed "THE REACHABILITY PROOF", which proves a *projection hop*, not reachability from a user
- **Filed at `low`, and the rating is argued rather than assumed.** Unlike its four precedents this module **declares its own unwiredness in-tree**, at length and correctly: `background/async_status_snapshot/mod.rs:27-49` is headed "⚠ `encode_async_status_snapshot_widget` has NO production caller in cyrup, and that is the decision, not an oversight", names both blockers (`cyrup-ext` has no `set_widget` capability — the same one `tui/events.rs:49` already blocks on — and cyrup has no RPC bridge, which is **PB-8**), and records the two shortcuts it deliberately refused: it does not splice the 32 KiB line into `control_status`'s text output, and it does not invent a `SubagentExecutor::async_status_snapshot` entry point whose only caller would be its own test. **That is the right behaviour under this document's rules and it does not make the code reachable.** It is filed so the register is complete and so the debt is visible when `set_widget` or PB-8 lands
- **Depends on:** nothing any more — both halves landed. ~~*"PB-8 (§1b) for the RPC half, and a `set_widget` capability on `cyrup-ext`'s native-extension seam for the widget half — the same seam family as UW-7 and area 06 `EXT-021`"*~~ — **the seam-family clause is struck 2026-09-22**: `EXT-021` shipped (`InitApi::subscribe_terminal_input` `cyrup-ext/src/native.rs:425`, dispatch `facade.rs:1472-1522`), so naming it as a blocker family pointed at finished work
- **Owning id:** none. Area 09's `## Open items` table is not this pass's to edit, so a `SUBA-` id must be assigned by that area's owner; until then this entry is the only record. Its in-source citations are **correct at v0.66.0**, the tag the sibling capacity module pins, and have drifted by v0.68.0 as noted above

---

## 3. Version lag — upstream added it after cyrup's ported baseline

Still work. An item here is in scope for the next version bump, not out of scope.
**66 items: 36 against `pi`, 17 against `pi-subagents`, 13 against `pi-intercom`, 0 against
`pi-permission-system`.**

> **CORRECTION 2026-09-16 — the `pi-subagents` figure is spent, and the WINDOW is wrong.** §3b was
> re-read against cyrup `cc7818b` this pass: **fourteen of its seventeen items are CLOSED** and
> four remain (`SUBA-054`, `SUBA-023`, `SUBA-024`, `SUBA-026`), every one of which is also carried
> in §1b. (**`SUBA-023` closed 2026-09-20** with VL-S3/VL-S4; three remain, and the arithmetic
> below is the sweep's own and is kept as taken.) So the honest figure is **53 items: 36 `pi`, 4 `pi-subagents`, 13 `pi-intercom`** — and
> §3a and §3c were **not** re-read, so 36 and 13 are themselves unverified.
>
> **The larger problem is the window, not the count.** §3b's range is `v0.43.0..v0.47.1`. Latest,
> re-measured this pass with `git -C tmp/pi-subagents tag --sort=-v:refname | head -1`, is
> **`v0.68.0`**. **`v0.47.1..v0.68.0` is owned by no area file and no item in this document** —
> area 09 is settled at v0.47.1 and `09a`'s scope stops at v0.57.0. Closing §3b's fourteen does not
> shrink the lag; it exhausts the only window anyone has measured.

**Why this fell from 78.** Nothing was closed and nothing was deleted. Area 12 ran
`git cat-file -e v0.83.0:<path>` before trusting any inherited `upstream-drift` kind and **six items
proved to be in-baseline work misfiled as lag** — `DRIFT-018`, `DRIFT-019`, `DRIFT-030`, `DRIFT-031`
and `DRIFT-032` moved to `not-ported`, `DRIFT-016` to `stale-port`. Separately, the `tracking`-kind
rows and the two `upstream-drift` scope questions (`AGENT-028`, `SESS-038`) are now trackers outside
the count. **Lag shrank because the port-bug pile grew**, which is the direction that matters: a
port bug is owed now, a lag item is owed at the next bump.

### 3a. `pi` v0.83.0 → v0.84.1 — 36 items (627 files, +52 291 / −17 556)

The `VL-P` rows below are the previous edition's cross-cutting entries; the re-audit assigned owners
to most of them. Where an area item owns the row, **the area file carries the two-sided evidence at
HEAD and is the place to work from** — the citation here is the load-bearing pair only.

| id | one line | upstream (v0.84.1 unless noted) | cyrup | owner |
|---|---|---|---|---|
| ~~VL-P1~~ | ~~`baseten` provider, `thinkingFormat:"baseten"`, `compat.chatTemplateArgs`~~ | `providers/baseten.ts:6-14`; `api/openai-completions.ts:779-795`; `types.ts:565`, `:574-575` | `api/compat.rs:28-39` (10 variants, no `Baseten`), `:73-168` (no `chat_template_args`); `grep -rni baseten crates/` = 0 | 12 `DRIFT-009`; env half `BASETEN_API_KEY` — **CLOSED 2026-09-22.** `baseten` is registered as a fleet provider with a dynamic catalog (`providers/all.rs:16`, `:60-64`; `providers/fleet.rs:162` maps `BASETEN_API_KEY`), and the `Baseten` thinking format plus `chat_template_args` are ported (`api/compat.rs:49`, `:338-345`). |
| ~~VL-P2~~ | ~~`qwen-token-plan-individual` provider~~ | `providers/all.ts:120` (absent at v0.83.0) | `providers/all.rs:141-243` — cyrup registers **35** built-ins against v0.84.1's **40** | 12 `DRIFT-019` (only this one of its four is genuine lag — the other three predate the tag) — **CLOSED 2026-09-22.** `qwen-token-plan-individual` is registered (`providers/all.rs:41`) with its env key at `env_api_keys.rs:59`. |
| ~~VL-P3~~ | ~~`samplingParams` carried nowhere~~ | `api/simple-options.ts:27-33`; `types.ts:189`, `:802`; applied `openai-completions.ts:885-886`, `openai-responses.ts:331-332`, `azure-openai-responses.ts:325-326`; composed `provider-composer.ts:123-125` | `utils/simple_options.rs:61-98` threads 20 fields, none of them sampling params | 05 `CFG-039` (config tier), 02 `AGENT-026` (proxy body) — **CLOSED 2026-09-22.** `samplingParams` is threaded — `utils/simple_options.rs:58-101` (`merge_sampling_params`), with pi's `if (options?.samplingParams)` empty-map semantics preserved. |
| VL-P4 | vLLM `thinking_token_budget` via `compat.supportsThinkingTokenBudget` | `types.ts:583`; `openai-completions.ts:851-866` with a `MIN_ANSWER_TOKENS` floor | `api/compat.rs:73-168` — no such field; and `PROV-015` means it has nowhere to land | 01 `PROV-015` |
| VL-P5 | `telemetryContext` on request options | `packages/ai/package.json:65` (telemetry became a runtime dep at v0.84.1), `types.ts:122-123`, `api/simple-options.ts:36` | `stream.rs` `StreamOptions` has no telemetry field; zero `TelemetryContext` hits | 12 `DRIFT-047` (which extends it to the whole `packages/telemetry` + `pi.ai.request` span contract), 02 `AGENT-028` *(now a tracker — see OQ-7)* |
| VL-P6 | Auth ops take no cancellation signal; OAuth refresh has no 15 s bound | `auth/types.ts:45-48` threaded onto every `CredentialStore` method `:70`, `:76`, `:86-90`, `:93`; `DEFAULT_OAUTH_REFRESH_TIMEOUT_MS = 15_000` at `auth/resolve.ts:120`, raced `:149-153` | `auth/store.rs:24-53` — `CredentialStore` still takes no options argument on `read`/`list`/`modify`/`delete`; `auth/resolve.rs:150-200` is the refresh path and it is unbounded — `grep -n 'timeout\|cancel' auth/resolve.rs` → 0, and there is no `DEFAULT_OAUTH_REFRESH_TIMEOUT_MS` counterpart anywhere *(was `:198`; refreshed 2026-09-22)* | unowned at item level; 08 confirmed the CLI call sites pass no deadline |
| ~~VL-P7~~ | ~~Copilot Individual: no policy-state fallback~~ | `auth/oauth/github-copilot.ts:92-113`, gate `:115-133` | `providers/github_copilot.rs:310-326`, `:331` — single list, no fallback, no gate | 01 (re-confirmed open) — **CLOSED 2026-09-22.** The Copilot policy-state fallback is at `providers/github_copilot.rs:322-328` (`model_picker_enabled`, `policy.state`), citing `github-copilot.ts:91-96`. |
| ~~VL-P8~~ | ~~A blocked tool call cannot terminate the batch~~ | `packages/agent/src/types.ts:61-69` (`BeforeToolCallResult.terminate`), consumed `agent-loop.ts:636-646` | `cyrup-agent/src/hooks.rs:49-52`; blocked result hardcodes `terminate:false` at `agent.rs:1030` | 02 `AGENT-022`, 06 `EXT-049` (the extension-facing `ToolCallEventResult.terminate`) — **CLOSED 2026-09-22.** `BeforeToolCallResult.terminate` is live: `cyrup-agent/src/hooks.rs:58-68` carries `terminate: TerminateHint`, cited to `AGENT-022`. |
| ~~VL-P9~~ | ~~`Agent::reset()` does not reject mid-run~~ | `agent.ts:333-336` (no guard at v0.83.0 `:326`) | `agent.rs:1604-1616`, whose own doc at `:1601` says it clears "unconditionally, even mid-run" | 02 `AGENT-023` — **CLOSED 2026-09-22.** `Agent::reset()` rejects mid-run — `cyrup-agent/src/agent/lifecycle.rs:113-116` returns `AgentError::RunActive(BusyEntry::Reset)`. (The `agent.rs:1604-1616` citation is dead; that file no longer exists.) |
| ~~VL-P10~~ | ~~No compact-and-retry after a recoverable `length` stop~~ — **CLOSED 2026-09-22; the row's evidence was a false zero-hit.** `rg is_recoverable_length crates/` is still 0 and always would be: cyrup names the predicate differently. `utils/overflow.ts:171-173`'s `isRecoverableLengthStop` **is** `is_context_overflow`'s **case 3** (`cyrup-provider/src/utils/overflow.rs:101-109`: `StopReason::Length && usage.output == 0 && input*100 >= window*99`), which drives overflow recovery; the compact-and-**retry** half is `session/auto_compaction.rs:412-425` (`will_retry` + `pop_trailing_assistant_if(Error\|Length)`, pi's exact predicate) with the one-shot brake at `session/run.rs:444-446` (SEAM-112) | — | — | closed — do not re-file on the symbol name |
| VL-P11 | *(superseded)* prompt during compaction | — | — | **→ PB-29, CLOSED 2026-09-22** (promoted to high; it is a port bug, and the defect is wrong context, not a missing rejection) |
| VL-P12 | Tool-result images never normalized/auto-resized | `utils/tool-result-images.ts:22-62`, called after the `tool_result` hook at `core/agent-session.ts:518-520` | `cyrup-ext/src/hooks.rs:72-82` — `after_tool_call` is a pure field diff over `content`/`is_error`/`details`/`usage`/`terminate`. Primitive exists for the `read` tool only (`cyrup-tools/src/tools/read.rs:425-427`, `image_proc::process_image`), so an image returned by **any other** tool is never normalized; the `images.autoResize` toggle is wired at `cyrup-tui/src/app/settings_rows.rs:101` → `eff.image_auto_resize()` *(was `hooks.rs:58-113` / `read.rs:265`; refreshed 2026-09-22)* | **unowned** — see below |
| ~~VL-P13~~ | ~~Ambiguous bare `--model` silently picks the first catalog match~~ — **PREMISE FALSE, retired 2026-09-22.** The upstream cell claimed `model-resolver.ts:469-503` *"errors `Model "…" is ambiguous across providers: …`"*. `git -C tmp/pi grep -n 'ambiguous across providers' v0.83.0 -- packages/` returns **nothing**: `findExactModelReferenceMatch` returns `undefined` on an ambiguous bare id and `tryMatchModel` falls through to **partial matching**, which always picks one. The doc comment's word "rejected" describes the exact-match helper's `undefined`, not an error. cyrup reproduces upstream exactly and records the same finding independently at `cyrup-config/src/model/resolver.rs:34`, `:163` and `:443` (`ambiguous_bare_id_resolves_via_partial_like_pi`). `cyrup-config/src/model.rs:1139-1143` is dead — the module is now `cyrup-config/src/model/` | — | — | retired; there is no gap |
| ~~VL-P14~~ | ~~`auth check` subcommand unrecognized~~ | `cli/auth-command.ts:51` (usage `:18`, `:42`), impl `cli/auth-check.ts:22-73`, result shape `:16-20` | `crates/cyrup/src/credential_print.rs:149-155` matches only the two print verbs | 08 `SEAM-050`, which widens it to the whole v0.84.1 auth-command surface (per-kind unknown-option errors, tri-state 0/1/2 exit) — **CLOSED 2026-09-22.** `auth check` is ported — `cyrup/src/credential_print.rs:41-89` (`CredentialPrintKind::Check`, usage at `:76`, `AuthCheckResult` at `:89`), with the 0/1/2 exits. |
| VL-P15 | A malformed `pi` block in `package.json` hard-fails the install | `core/pi-manifest.ts:16-34` (whole body in try/catch → `null`; `:26` skips non-string-array fields) | `cyrup-resources/src/package/manifest.rs:114` — `serde_json::from_str(&text)?` (the `pi` block) and `:107` `toml::from_str(&text)?` (the `cyrup.toml` branch); both still hard-fail where pi's try/catch yields `null` *(was `:87`/`:80`; refreshed 2026-09-22)* | **unowned** — see below |
| VL-P16 | Management HTTP fetches are not retried | `utils/management-http.ts:25-68` (2 extra attempts, retryable set `:3`), callers `remote-catalog-provider.ts:81`, `version-check.ts:57`, `tools-manager.ts:109`/`:127` | `cyrup-provider/src/remote_catalog.rs:652-655` — one `send()` with no retry wrapper; any transport error or 5xx is terminal. **`utils/retry.rs` is `retry.ts`, the assistant-call retry loop, NOT `management-http.ts` — do not conflate them**; nothing in the workspace ports `management-http.ts` *(was `:544-547`; refreshed 2026-09-22)* | 01 (re-confirmed open) |
| VL-P17 | Terminal colour-scheme and background probes run sequentially | `modes/interactive/theme/theme.ts:796-810` (both promises started, then awaited) | `cyrup-tui/src/theme.rs:1416-1424` — early return on `query_color_scheme`, then fall through to `detect_terminal_background_theme` *(was `:1334-1343`; refreshed 2026-09-22)* | **unowned** — see below |
| ~~VL-P18~~ | ~~`tui.editor.historyPrevious` / `historyNext` not rebindable~~ | `packages/tui/src/keybindings.ts:68-75` (both `defaultKeys: []`), consumed `components/editor.ts:768-777` | `cyrup-tui/src/keymap.rs:157-186` — 24 ids, neither history id | 07 `TUI-035` (verified absent at v0.83.0 — genuine lag) — **CLOSED 2026-09-22.** Both history ids are rebindable — `cyrup-tui/src/keymap.rs:471` `"tui.editor.historyPrevious" => E::HistoryPrevious` and the forward half at `:384`. |
| ~~VL-P19~~ | ~~The fullscreen (alternate-screen) TUI program is entirely absent~~ | `packages/tui/src/tui-alt-screen.ts` (1047 lines, new at v0.84.1) + `components/scroll-view.ts` (195) + `alt-screen-flash.ts` (51); flag `cli/args.ts:180-193`; settings `settings-manager.ts:135-136`; switch `interactive-mode.ts:345`; 8 `tui.altScreen.*` bindings `keybindings.ts:43-50` | no alt-screen module; `app/input_reader.rs:431` is `Event::Mouse(_) => None`; no `tui_mode` anywhere | 08 **`SEAM-051` — now *high*, effort S, and the most urgent row in this table**: `--tui-mode regular`, the flag's *default* value, is captured by `partition_extension_flags` and every mode exits 1. 07 `TUI-019` (**re-rated low → medium**; the ADR-0001 justification is struck — see OQ-8), 05 `CFG-021` (`tuiMode`/`fullscreenScrollbar` settings), 12 `DRIFT-022` *(now a tracker)*. **`SEAM-051` and `CFG-021` must be fixed under either answer to OQ-8; they do not wait on it.** **Mechanism note**: ratatui supports the alternate screen and mouse capture natively; the gap is the application layer. — **CLOSED 2026-09-22.** The alt-screen program exists — `crates/cyrup-tui/src/altscreen/` (`document/exit/flash/images/keys/mod/mouse/out/prompt_nav/scroll`), the eight `tui.altScreen.*` ids at `keymap.rs:1894-1913`, `--tui-mode` parsed at `cli/args.rs:190` and consumed by `cli/argv.rs:147-177` (so the `SEAM-051` exit-1 is gone), and `app/input_reader.rs:488` routes `Event::Mouse` into `altscreen::mouse::map_reader_event` instead of dropping it. |
| ~~VL-P20~~ | ~~Mermaid fences not rendered; `markdown.mermaid` is not a setting~~ | `modes/interactive/components/mermaid.ts:14-30`; setting `settings-manager.ts:61` (getter `:1251`) | `cyrup-tui/src/markdown.rs:964-965` — the only two "mermaid" occurrences are a comment quoting upstream's predicate | 07 `TUI-034`, 05 `CFG-040` — **CLOSED 2026-09-22.** Mermaid is rendered — `cyrup-tui/src/markdown/mermaid.rs`, with the `markdown.mermaid` setting at `cyrup-config/src/settings/effective.rs:478-485`. The "has nowhere to attach until VL-P21 lands" dependency is discharged: both landed. |
| ~~VL-P21~~ | ~~`registerMarkdownTransformer` and the transform pipeline are absent~~ | `core/extensions/types.ts:1153`, `:1292`, `:1703`; pipeline `components/markdown-transform.ts:3-29` (fail-open per transformer, width-aware) | `cyrup-ext/wit/world.wit` has no transformer import/export; `markdown.rs` has no transform seam | 06 `EXT-019` (re-scoped to the v0.84.1 `MarkdownTransformContext` shape), 07 `TUI-034`, 12 `DRIFT-015`. **VL-P20 has nowhere to attach until this lands** — upstream ships mermaid AS a registered transformer. — **CLOSED 2026-09-22.** The transformer seam exists on both sides — `cyrup-ext/wit/world.wit:340-346`/`:581-584` and `cyrup-ext/src/native.rs:416` `register_markdown_transformer`, with the pipeline documented at `cyrup-tui/src/markdown/mod.rs:184-200`. |
| VL-P22 | A torn session-JSONL tail is never repaired; fork is not published atomically | `packages/agent/src/harness/session/jsonl/storage.ts:33-46`, `:83-90`, `:93-95`, `:99-109` | `cyrup-session/src/manager/load.rs:36-53` (`load` skips malformed lines and returns `recovered`) and `manager/lifecycle.rs:106`, `:121-122`, where the rewrite is gated `if migrated && !recovered` — a recovered file is provably never rewritten. `cyrup-session/src/store.rs:326-362` (`create_exclusive`) writes header+entries straight to the freshly created fd (`:338-341` says so deliberately, mirroring pi's `"wx"`). *(Was `manager.rs:851-888` / `:114-117` / `store.rs:86-116`; the manager monolith was split into `cyrup-session/src/manager/`, refreshed 2026-09-22.)* | **ownerless and growing** — see below |
| VL-P23 | `packages/protocol` + `packages/client` entered the closure — no remote-session wire format | `coding-agent/package.json:48-49` now depends on `pi-client`/`pi-protocol` (neither at v0.83.0), consumed `client/remote-session.ts:7`, `:17` | no `cyrup-protocol`/`cyrup-client` crate; nothing decodes framed CBOR. **Do not conflate** `cyrup-intercom/src/transport/framing.rs` — a line/JSON protocol capped at 1 MiB (`:19`) | 08 `SEAM-058` *(a tracker: its Fix is "track, do not build"; the reachability re-check confirms v0.84.1's `main()` still does not reference `experimentalCli`)* |
| VL-P24 | `CredentialSynchronizationError` and serialized, cancellable credential operations | `core/model-runtime.ts:94-111`, `:494` (`enqueueCredentialOperation`) | zero `CredentialSynchronizationError` hits; `/login`/`/logout` have no per-provider operation queue | 05 `CFG-020` — **note the target grew +356 lines at v0.84.1; read `model-runtime.ts` at v0.84.1, not v0.83.0.** Depends on PB-3's `providers:[id]` refresh scope. Area 12's `DRIFT-023` is a duplicate **and a lead** — its diffstat was never re-derived |
| ~~VL-P25~~ | ~~Catalog set trails the provider set~~ | 39 `*.models.ts` at v0.84.1 (Together's is hand-written Rust) | 35 embedded catalogs; the four with no counterpart are `baseten`, `qwen-token-plan`, `qwen-token-plan-cn`, `qwen-token-plan-individual` — closes with PB-2, VL-P1, VL-P2 | 12 `DRIFT-009` (rewritten this pass), 01 `PROV-018` / `PROV-038` / `PROV-039`, `PROV-004` *(tracker)*. Per-model accuracy IS statically auditable and **has now been audited**: all 35 catalogs are generated from `b0c2a90e` by `cargo run -p xtask -- gen-catalogs` (**CLOSED 2026-08-15**, `PROV-018`/`PROV-060`), which needs no generator run and no network. See OQ-5's refutation above — **CLOSED 2026-09-22.** Every registered provider now has a catalog source: 35 embedded JSON catalogs plus the five deliberate dynamic ones (`radius`, the three `qwen-token-plan*`, `baseten` — `providers/all.rs:57`, "NO embedded catalog by design"). This row's own closure condition ("closes with PB-2, VL-P1, VL-P2") is met — all three are closed. |

**Four rows no area file has claimed across two passes** — nobody has re-derived them at `04c1ba2`,
and nobody owns the fix. They are the highest-risk rows in this table because they read as verified:
**VL-P12** (tool-result image normalization — the `images.autoResize` *toggle* is now wired at
`app/settings_rows.rs:101`, which is a different thing from running pi's normalizer after the
`tool_result` hook), ~~**VL-P13** (ambiguous `--model`; `--model glm-4.7` is offered by six
providers in cyrup's own catalogs)~~ — **retired 2026-09-22, premise false: upstream never errors
on an ambiguous bare id either, and cyrup already matches it** — **VL-P15**
(`"pi": {"extensions":[1,2]}` aborting a whole install — and its blast radius was already flagged
unconfirmed), **VL-P17** (double-timeout `auto` theme detection on a terminal answering neither
DSR ?996 nor OSC 11; the remedy is larger than the symptom because cyrup's `TerminalProbe` is a
synchronous `&dyn` trait). **Three, not four, after VL-P13's retirement.**

> **Rule-4 note on `VL-P17`, so a later pass does not "correct" a correct citation.** §3a is the
> `v0.83.0 → v0.84.1` window and the upstream cell is pinned at **v0.84.1**, where
> `detectTerminalThemeForAuto` (`theme/theme.ts:791-811`) really does start both promises before
> awaiting either. At **v0.83.0** the same function is strictly sequential, so anyone re-checking
> this row at the wrong tag will wrongly call it false. Re-verified at v0.84.1 on 2026-09-22; the
> cyrup half is what had drifted, and only the cyrup half was renumbered. *(Do not mistake area 08's new `SEAM-066` for an
owner of VL-P17: that item is the pre-launch surfaces hardwiring the dark palette, a different
defect in a different file.)*

**VL-P22 is still the single largest ownerless surface in the port.** Three area files hand the same
mass to it and none owns it: area 03 (`packages/agent/src/harness/session` retree — 21 files,
+3070/−1147, adding seq/lanes/records/retainedTail plus a 993-line conformance suite), area 02
(`AGENT-028` — `packages/agent/src/harness/**` is ~11.4k insertions / ~10.9k deletions in this window,
including the `agent-harness.ts` rewrite, a new 667-line `reducer.ts` and a new typed telemetry layer,
**not measured and owned by no area file**), and area 12 (`DRIFT-040`, a tracker **and** a lead —
area 12 states plainly which of its claims were confirmed first-hand, the `packages/storage` →
`packages/session-backends` rename and `harness/session/types.ts:44-58`, and which were never read).
The *torn-tail* half of VL-P22 is a concrete, small, provable bug and should be fixed now; the
*harness-v2* half is a scope decision — **OQ-7**.

**Additional pi-drift items filed since the VL-P rows were written**: area 12's `DRIFT-041`
(session HTML export is a 131-line text dump against pi's 5 021-line templated document across 8
files — tool-result *text* survives, call arguments and result metadata do not), `DRIFT-045`
(Ctrl+V with text on the clipboard inserts nothing — the `wl-paste` text branch), `DRIFT-046`
(`normalizeWindowsShellPath` — **duplicate of `TOOL-036`**, which owns the same body plus the
`~`/`os.homedir()` half; note area 04's correction that the `~` half is a v0.83.0 parity bug while
`normalizeWindowsShellPath` itself landed *inside* the drift window), `DRIFT-048` (Google converter
picks the tool-call-id rule off the SOURCE message's model — a defect inside the code that closed
`DRIFT-026`), `DRIFT-050` (`CYRUP_TELEMETRY=` empty is an explicit OFF upstream and a silent no-op
here), `DRIFT-051` (`process.title`'s role suffix — RPC, `__subagent-runner` and `__intercom-broker`
children are all bare `cyrup` in `ps`; the base title is satisfied by accident in Rust, which is why
the item claims only the suffix); area 01's `PROV-040` (`fetchDeferred`/`cancelDeferred` — cyrup
ports the deferred data model but no handle can be redeemed); area 05's `CFG-041` / `CFG-042`;
area 06's `EXT-049`…`EXT-052`; area 07's `TUI-046` (Kitty keyboard flag 1 vs pi's 7).

**Systematic delta sweeps, so nobody re-runs them.** Across the whole v0.83.0..v0.84.1 delta: exactly
**1** CLI flag added (`--tui-mode` → VL-P19), exactly **3** settings keys added (all area 07), **7**
env vars added (`AI_AGENT` → PB-5, `BASETEN_API_KEY` → VL-P1, `CC`/`STY`/`ZELLIJ` → area 07). Areas
03 and 04 both returned near-empty: `core/compaction/*`, `system-prompt.ts`, `skills.ts`,
`session-cwd.ts` and `prompt-templates.ts` are **byte-unchanged** across the two tags (all five
re-verified this pass), and `packages/coding-agent/src/core/tools/` is 7 files / +68 −35 with all
four changes already carried or provably no-ops. `bash-executor.ts` and `output-guard.ts` are
byte-identical. That is a real, narrowing result — it means this delta's risk is concentrated in
`packages/agent/src/harness/**` (VL-P22 / OQ-7), `packages/tui` (VL-P19) and the provider tier.
**One caution the repair pass earned**: area 04's version-lag sweep was scoped to
`packages/coding-agent/src/core/tools/`, and `utils/paths.ts` is not under that path — which is how
`normalizeWindowsShellPath` was missed. A "near-empty diff" result is only as wide as the paths it
walked, so state them.

**Window limit.** pi HEAD is `581d75a89` = `v0.84.1-117-g581d75a89`, so **117 commits past the tag are
unanalysed**. One concrete item is known to sit in there and was deliberately not filed because the
hard rules require a named tag: `getExperimentalToolSampling()`'s constrained-sampling request on the
four built-in tools, absent at both diffed tags.

### 3b. `pi-subagents` v0.43.0 → v0.47.1 — 17 items (151 files, +10 254 / −1 333)

**This range was unanalysed territory until the 2026-08-12 re-baseline** — the previous edition
recorded v0.43.0 as "latest". The src-only sweep covered **96 non-merge commits, 67 files,
+4 696/−769 and 12 net-new source files, all 12 read**; fourteen commits were diffed line by line.
All 17 items live in area 09 with two-sided evidence:

> **RE-READ 2026-09-16 at `cc7818b`: fourteen of the seventeen are CLOSED, and the strikes below are
> this pass's.** Each was checked against area 09's current `## Open items` table AND spot-verified in
> code; the three that remain are `SUBA-054`, ~~`SUBA-023`~~ (**CLOSED 2026-09-20**) and
> `SUBA-024`, plus `SUBA-026`. **Re-greped 2026-09-22 at `14e6c56`: `SUBA-054` and `SUBA-026` are
> both CLOSED and `SUBA-024` is NARROWED to half of itself, so this section's only remaining open
> item is `SUBA-024`'s agent-contract half.** **The
> upstream window this section names is itself obsolete** — `v0.47.1` was latest when it was written;
> re-measured this pass, latest is **`v0.68.0`**. `v0.47.1..v0.68.0` is owned by no area file and no
> item here, which is a larger hole than the seventeen items this section counts.

- **medium** — ~~`SUBA-044`~~ **CLOSED** (the bundled `reviewer` agent's tool grant), ~~`SUBA-050` (`subagents.modelScope.strict`)~~ — **CLOSED**, ported as `exec/model_scope.rs` and enforced over the whole fallback ladder at `exec/fallback.rs:279`, `:417`, `:437`, `:457` (landed `2bd76ac`, SCOPE batch 1), ~~`SUBA-051`~~ **CLOSED** (default wall-clock bound; `exec/mod.rs:215` `DEFAULT_FOREGROUND_TIMEOUT_MS = 30 * 60 * 1000` and `workflows/scripted/engine.rs:2178` the workflow twin), ~~`SUBA-052`~~ **CLOSED** (YAML block scalars — `discovery/frontmatter.rs:337` folds `>`/`>-` and `:507` takes the dedented literal block verbatim), ~~`SUBA-053`~~ **CLOSED** (`~` expansion — `extension/executor/paths.rs:152` `expand_tilde`, `spawn/chain_graph.rs:710` `expand_home_path`, both with live callers), ~~**`SUBA-054` (`defaultReads` never reaches a single run — also UW-16)**~~ — **CLOSED 2026-09-22.** `RunOptions::reads` (`exec/agent_config.rs:440-453`) is produced from the persona at `extension/executor/foreground.rs:1178` (`reads: agent.default_reads.clone()`) and consumed in `build_task_text` at `exec/spawn_plan.rs:1414-1420`, which prepends `build_single_reads_instruction` ahead of every other injected block — pi's `task = readsInstruction + task` (`subagent-executor.ts:3873`, BEFORE `injectSingleOutputInstruction` at `:3874`). `agent_config.rs:449-452` records the closed hole in-tree: *"Before this existed, `defaultReads` was parsed off frontmatter and rendered in agent listings but never reached a run"*. **Check UW-16 separately — this closure does not automatically discharge it**, ~~`SUBA-055` (the `guide` action)~~ — **CLOSED**, `guide` is in `SUBAGENT_ACTIONS` — ~~the 42-verb list at `extension/tool/text.rs:215`~~, **59 verbs at `text.rs:265` as of `14e6c56`; enumerate the const rather than quoting a number (see `VL-S13`)** — and `/subagents-guide` is `registration/slash_commands.rs:121`, ~~`SUBA-056` (durable completion replay and output archives)~~ — **CLOSED**, ported as `background/completion_replay/` (5 files, 1 810 LOC, landed `2bd76ac`) and read as the THIRD rung of `collect_wait_completions`; `background/inspect_rpc/read_output.rs` is a second consumer. **Area 09's table still carries this row as an open medium — see §0's thirteenth edition**, ~~`SUBA-057` (`dismiss`)~~ — **CLOSED**, `dismiss` is in the advertised verb list
- **low** — ~~**`SUBA-023` (async lifecycle hardening; no signal-name attribution)**~~ — **CLOSED 2026-09-20**; §1b's VL-S3/VL-S4 were its two halves and both closed that day (`background/session_lease/` and `background/process_terminal/`, 5,912 LOC across 15 modules, both reachable from production callers). Its signal-name half closed in sweep 1 and the "no `ExitStatus::signal()` name mapping" observation was REFUTED long before that, **`SUBA-024` (`parallel-handoff` / `agent-contract`) — NARROWED 2026-09-22, still open for its agent-contract half only**: ~~`"handoffPath"` is still zero-hit (§1b VL-S10)~~ — **false**, and self-refuting: `VL-S10` closed 2026-09-19 and `handoff/` (9 modules) is the writer, so `handoffPath` now has production hits (`background/records.rs:423-426`, `background/runner_main/status.rs:341-344`, `spawn/cleanup_plan/{mod,model,metadata}.rs`). What remains is `agent-contract.ts`, unported: `spawn/chain_graph.rs:2316-2321` records that "this crate has no agent-contract concept at all yet … the predicate is `false` for every run", so `reportOptional` is a hard-coded `false`. That is the whole residual and it keeps the row at *low*, ~~**`SUBA-026` (interactive admin UI and selector)**~~ — **CLOSED 2026-09-22.** The three slash commands landed with §1b `VL-S11` and the table is **18** at `registration/slash_commands.rs:228` (the "17-variant match at `:83-121`" was stale on count AND line). **The narrowing this row carried was stale in the same commit that wrote it (`cbdb27a`)**: `/subagents` — the interactive admin surface — is `extension/host/slash_admin.rs:26` over `registration/subagents_admin.rs`, which ports `openSubagentsAdmin` (`subagents-admin.ts:396`) including `selectAgent` (`:1206`) and the `chooseModel`/`chooseThinking`/`editSystemPrompt` loop, with the no-UI `metadataFor` branch. The remaining half — `src/slash/selector.ts`, 147 L @v0.68.0 (**NOT** `src/tui/selector.ts`, which `git cat-file -e <tag>:src/tui/selector.ts` proves exists at no tag; keep that correction) — is **not a gap but a recorded `[CYRUP-DELTA]`**: `subagents_admin.rs:31-36` states that `selectFromList` deliberately takes upstream's own documented `ctx.ui.select` fallback (`:224-227`) because `HostServices::custom` is always present and its `None` cannot be told apart from a dismissal. Preferring the overlay would turn "this host paints no custom overlay" into a spurious cancel, ~~`SUBA-058`~~ **CLOSED**, ~~`SUBA-059`~~ **CLOSED**, ~~`SUBA-060`~~ **CLOSED**, ~~`SUBA-065` (`unknownSubagentActionMessage`)~~ — **CLOSED**, and its `DESTRUCTIVE_MANAGEMENT_ACTIONS` gate (`extension/tool/text.rs:317`) had carried `schedule.delete` since before that verb dispatched — deliberately, so the stricter did-you-mean rule applied from the first call, ~~`SUBA-066` (`/subagents-guide`)~~ — **CLOSED**, `registration/slash_commands.rs:121`
- ~~**The still-open rows are ALL in §1b as well** (VL-S10, VL-S11, UW-16 — VL-S3 and VL-S4 were on this list until they closed on 2026-09-20)~~ — **re-derived 2026-09-22: every row that sentence named has since closed** (`VL-S10` 2026-09-19, `VL-S11` 2026-09-21, `UW-16` 2026-09-22), and `SUBA-054`, `SUBA-026` and `SUBA-024`'s handoff half went with them. **§3b's only remaining open item is `SUBA-024`'s agent-contract half** (`low`, and §1b carries no row for it — this section is its only home). §3b still records no work §1b does not carry a twin for elsewhere, which is a sign the section has been absorbed, not a sign it should be deleted: ids are retained.

**Not filed by rule**: `run-fanout-budget.ts` (257 lines — a whole per-run logical fan-out cap with
config, doctor check and status surface) landed on `main` at `17b4078`/`668c587` **after v0.47.1** and
has no named tag to cite. Pick it up on the next tag.

**Unread on the upstream side in this range**, so anything inside is invisible: `workflows/scripted-workflow.ts`
(+231), `inspectors/herdr/project-panes.ts` (+524), `runs/background/async-job-tracker.ts` (+426 — the
v0.47.0 event-driven rewrite), `foreground-history.ts` / `workflow-foreground-steering.ts` (new),
`shared/display-text.ts` (new), `tui/{render,fleet}.ts` (+343), `missions/workflow-state.ts` (+209),
`extension/rpc.ts` (PB-8's file). Specifically unfiled and likely drifted: the **steering-recovery
hardening across v0.44–v0.47** in `subagent-prompt-runtime.ts` was read but not compared line by line
against cyrup's `prompt_runtime.rs` `SteeringInbox`, which targets the v0.43.0 shape. Diff it when
`SUBA-049` is scheduled.

### 3c. `pi-intercom` v0.9.2 → v0.10.1 — 13 items (24 files, +2 495 / −700, 14 commits)

The window is **two** minor versions, not three, because the ported baseline is v0.9.2 (§1d). All 14
commits are accounted for commit-by-commit in area 11. The 13 items are `ICOM-035`…`ICOM-047`:

- **medium** — `ICOM-035` (busy inbound **parked until idle instead of steered**; upstream deleted the
  entire park branch at v0.9.3 — this one also keeps `ICOM-023`/`ICOM-032` alive, and its fix does
  **not** need a `HostServices` change: `AgentSession::inject_message` already routes to `agent.steer`
  when `is_streaming()` at `session.rs:3752-3754` — **no longer true: `ICOM-035` was REOPENED at high on 2026-09-24, because `8de7460`'s injection pump waits for idle; see the ranked table at the top of this file**), `ICOM-036` (reply targeting by sender-ID prefix +
  four disambiguation errors; upstream **replaced** the function `ICOM-001` closed against, at
  v0.9.3), `ICOM-037` (a `send` to the sole pending asker is not treated as its reply), `ICOM-038`
  (no client liveness heartbeat — a half-open broker socket strands a session invisibly), `ICOM-039`
  (`list` prints a fixed 8-char id, not a distinguishing prefix), `ICOM-040` (unnamed-session alias
  uses 8 id characters, not 18 — **collides for UUIDv7 subagents spawned milliseconds apart, making
  peers unaddressable**; coupled to area 09's `orchestrator_presence_target`, change both or neither),
  `ICOM-042` (cwd-scoped `send`/`ask` and `openProjectPaneIfMissing`), `ICOM-044` (malformed config
  fails closed silently), `ICOM-046` (`reply` silently drops attachments — a one-line fix upstream
  shipped a point release for)
- **low** — `ICOM-041` (`runtimeFallbackAlias`), `ICOM-043` (v0.10.0 copy revision; emoji still
  present — land with `ICOM-048`), `ICOM-045` (blocking `ask` not refused up front when the target is
  offline), `ICOM-047` (broker startup failures discard the broker's stderr)

### 3d. `pi-permission-system` v0.7.1 → v0.8.0 — **zero**

Independently corroborated by area 10, which read the diff per shipping file: of 28 changed files, 11
are non-shipping, and `permission-manager.ts`, `jsonc-config.ts`, `permission-dialog.ts`,
`system-prompt-sanitizer.ts`, `permission-forwarding.ts` and `before-agent-start-cache.ts` are
extract/export refactors with no behavioural delta. The five genuinely behavioural v0.8.0 changes are
all ported and were closed or confirmed this pass — see §4. The five permission-system entries in
this document (PB-32, PB-15…PB-18) are all port bugs against **v0.7.1**, and PB-32 was verified at
**both** tags precisely so it could not be re-litigated as drift.

---

## 4. Closure record — what has actually landed, verified at `04c1ba2`

**117 items closed across the twelve area files in the re-baseline**, every one re-derived from code
on both sides rather than from a commit message. The largest single blocks: area 09 (22), area 05
(16), area 04 (14), area 07 (13), areas 01 and 10 (10 each). **207 new items were filed in the same
pass** (176 in the audit, 31 more in the repair pass that followed the completeness critique), and
most of them come out of auditing what the *closing* code does — which is the standing lesson:
**closing a "not implemented" item means the subsystem now exists, not that it is correct.**

**Subsystems that were "absent" and are now ported and wired**
- `watchdog/` — 18 modules under `crates/cyrup-ext-subagents/src/watchdog/` (~18k lines), including a real stdio LSP client. Wiring: `register_main_watchdog` at `extension.rs:9055`, nine subscriptions `:9338-9352`, `/subagents-watchdog` `:9326`, four `watchdog.*` verbs `:7567-7570`. *(Both no-op agents were replaced by real nested-`Agent` turns — UW-4 and UW-5 are CLOSED.)*
- `missions/` — 7 modules; six `mission.*` verbs, params parsed `:5423-5432`, goal-continuation notices from the `agent_end` handler. *(Write half of workflow state remains — UW-8.)*
- The interactive fleet inspector — `tui/fleet_overlay.rs:177` implements `InteractiveOverlay`; `extension.rs:11366`/`:11376` construct it and call `services.open_overlay(Box::new(overlay))`, driven by `App::on_overlay_request` (`cyrup-tui/src/app/run_arms.rs:406-430`). **This is the seam PB-9 and PB-18 should now use.**
- The persistent fleet status widget (`set_widget` at `extension.rs:9489`, `:9889`, `:9978`); subagent tool renderers (consumed through the ONE dispatch site, `run_renderer` at `app/extension_render.rs:153-166`; `Which::ToolCall`/`Which::ToolResult` at `:162`/`:163`); terminal-run **revival** (`background/control.rs:1214` → `extension.rs:4232`); agent **aliases** end to end with the verbatim `Ambiguous agent alias '…': …` message.
- The child-side prompt runtime, the structured-output capture channel, the control/activity pipeline, the async deadline + cascade, agent memory, prompt workflows, the **native supervisor channel** (`native_supervisor.rs`, 2251 lines) and the tool-budget enforcer (`exec/tool_budget.rs`, 388 lines).
- All 11 OAuth login flows (`crates/cyrup-provider/src/auth/oauth/`), driven from `/login`. *(Two of them are unreachable — UW-11. And nine of the eleven were not audited against upstream at all; area 01 flags `PROV-003`'s closure as deliberately weak.)*
- The 2-model `seed.json` stub is **physically gone** (`PROV-007`): the six production sites now go through `cyrup-provider/src/catalog.rs:38-44` `builtin_catalog()`, guarded at `:52-75`.
- Provider retry + idle timeout (`utils/provider_retry.rs`, consumed by **seven** api impls); provider error-body truncation (`utils/error_body.rs`); the full 25-pattern `OVERFLOW_PATTERNS` set, byte-compared; `StopReason::{Pending,Deferred}` + `raw_stop_reason` verified by **producer sweep** across all six wire APIs; `supports_finish_reason` including its consumer; the WIT world at `@0.4.0` with a real cross-copy test.
- `crates/cyrup-modes/src/json_event.rs` — `modes/json-event.ts` is **new at v0.84.1 and already ported**, wired at `json.rs:79` and `rpc.rs:300`.

**`pi-permission-system` v0.8.0, fully absorbed** — `PermanentApprovalStore` removed (regression test `tests/permanent_approvals_file_is_inert.rs`) · review stream un-gated from `debug` (`logging.rs:168-170`) · wildcard 500-char cap → never-match (`wildcard.rs:24`, `:81-83`, `:57`) · forwarded-request id path containment, the critical one (`forwarding.rs:592`, `:596-597`) · `enabled` master switch (`ext_config.rs:51`, `:324`; early return `extension.rs:2221-2223`) · merge-preserving config save (`ext_config.rs:408`) · prototype-pollution key skip (`common.rs:173-175`).

**`pi-intercom` v0.9.2 interop** — re-verified line by line by area 11, and **every claim holds**: the full 16-tag `BrokerMessage` union decodes instead of tearing down the connection (`transport/protocol.rs:857-875`, `client.rs:635-651`) · the v0.9.2 envelope round-trips with a `#[serde(flatten)] extra` capture (`protocol.rs:301-347`) · the broker refuses to replace a live broker (`broker/runtime_claim.rs`, called at `broker/mod.rs:1238` before the stale-socket unlink) · live context-window usage in presence/session lists (`format_context.rs:70` → `tools/intercom.rs:377`).

**`pi` core** — Gemini 3 tool-call ids · Responses terminal-error fallback and length-stop mapping · `OAuthAuth::isSubscription` and the footer `(sub)` marker · `--model` cycling filtered to authenticated models · LaTeX rendering (`markdown/latex.rs`, 2242 lines) · `ctrl+home`/`ctrl+end` and editor page actions · batched colour-scheme reports settling on the last frame · OSC 9;4 terminal progress (tracking the **newer** v0.84.1 spelling) · searchable settings list · `AGENTS.override.md` as first context-file candidate · the `scrollbarThumb` theme token with pi's optional-with-fallback semantics · the bash session-env scrub in pi's exact delete→repopulate→hook order · the Google signed-empty-block hoist.

**Corrections this pass forced, all of them to claims that read as settled**
- **pi's model-catalog generator EXISTS, at both tags.** `git cat-file -e` passes at v0.83.0 and v0.84.1 for `packages/ai/scripts/{generate-models.ts (2733 lines), model-data.ts, models-dev-reasoning-options.ts, check-model-data.ts}` and `scripts/{diff-model-catalog.mjs, publish-model-catalog.mjs}`; pi's root `package.json:24-30` exposes `generate:models` / `hydrate:model-data` / `generate:model-catalog` / `diff:model-catalog` / `check:model-catalog`, and `packages/ai/package.json:52-54` gives the concrete invocations. **Only the OUTPUT is gitignored** (`.gitignore:11` → `packages/ai/src/providers/data/`). `DRIFT-009`'s "no in-tree regeneration source" sentence and the ledger's "catalog lag is unresolvable from this workspace" bullet were both **false** and are struck; `DRIFT-009` now defers to `PROV-018`, because seeding from the published pi.dev artifact is strictly lossier — the artifact *is* the published catalog, so it cannot reproduce what the generator computes from `models-dev-reasoning-options.ts` or the per-provider compat overrides, and it yields no reproducible build step. **The lesson generalises**: "no runtime effect" licenses skipping a directory's *behaviour*, never its *provenance* — a gitignored path is evidence that an artifact is generated, hence that a generator exists.
- **`SEAM-035`…`SEAM-046` never existed.** `git show a9000b1:docs/gap-analysis/08-…md | grep -o 'SEAM-0[34][0-9]'` returns 030-034 only; the 2026-08-12 pass simply started its new ids at `SEAM-047`. Recorded in area 08 so the twelve-wide hole is not read as a deletion under a hard rule about deletion. Honest caveat: `docs/gap-analysis/` has exactly **one** commit in cyrup's history, so an id dropped before the directory came under source control is invisible to that check — to this pass and to every pass.
- **`PROV-030` is contradicted by the file it points at.** `providers/all.rs:176-197` pushes `amazon_bedrock`, `openai_codex`, `google_vertex` and `github_copilot`, while the same file's status table at `:12-47` still calls three of them "pending" and its summary at `:46-47` names all four — including one the table's own row at `:21` marks ported. The doc fix is now mandatory inside PB-22 rather than a separate id, because a separate id lets the code fix land without it.
- **Nine `upstream-drift` claims in area 12 rested on a commit hash rather than a two-sided read.** Seven were re-derived cheaply and six proved misclassified (§3 preamble). The two expensive ones are now leads outside the count. The rule that falls out: **a commit hash answers "when did this land upstream", but classification turns on "before or after the tag cyrup ported from" — run `git cat-file -e <tag>:<path>` before assigning `upstream-drift`.**

**Corrections to CLAUDE.md that fall out of this pass** — worth folding back:
- "first-run predicate is a compile-time constant `false`" is **wrong**; it is `true` for this build (UW-2, OQ-6).
- "cyrup registers the 38 built-ins pi shipped at v0.83.0" is **wrong**; it registers **35** of upstream's **40**, and three of the five missing are port bugs (PB-1, PB-2).
- `packages/telemetry` is **inside** the port's dependency closure as of v0.84.1 (VL-P5 / `DRIFT-047`); `packages/{server,storage,evals}` remain outside it — **though area 08 could not re-verify that against v0.84.1's `package.json` and flags it**; `packages/{protocol,client}` newly entered (VL-P23).
- The `pi-subagents` baseline is not v0.33.x–v0.34.0; it is **≈v0.43.0**.
- The `pi-intercom` baseline is **v0.9.2**, not v0.7.0 (§1d). `crates/cyrup-intercom/src/lib.rs:2` still says v0.6.0 — `ICOM-012` carries the fix.
- **There is no `CLAUDE.md` in this workspace.** The file this document and the README repeatedly cite for the deliberate-divergence list and the out-of-scope pi package list does not exist here. Every claim sourced from it is unverifiable — which is why `SEAM-058` is a tracker and why OQ-7 exists.

**Bookkeeping actions for the ledger owner**: the promoted `PROV-010`/`AGENT-014`/`DRIFT-012` high row
is **stale** (`DRIFT-012` is closed); `SEAM-S02` (`08-…:520`) should be re-audited as **closed** —
`signals.rs:97-100` implements the repeat force-exit with pi's exact 130/143/129 codes — with
`DRIFT-049` replacing it; and the ledger's own "catalog lag is unresolvable from this workspace"
bullet is false (above). None of these changes an item count.

---

## 5. Deletion candidates — dead code, no behavioural difference

Not gaps. Recorded so nobody mistakes them for unimplemented features and "finishes" them.

- `crates/cyrup-ext/src/facade.rs:766` (`render_message_result`) and `:772` (`render_message_result_outcome`) — zero references outside their definitions and two doc-links (`native.rs:264`, `registry.rs:179`). pi has no message-RESULT renderer surface at all: `MessageRenderer` (`core/extensions/types.ts:1153`) is a single call-side function consumed only by `getMessageRenderer` (`runner.ts:579`) at `interactive-mode.ts:3471`. This is invented surface wider than pi's. Scope check before deleting: `RenderKind::Result` and the routing behind it also exist on the extension trait (and area 06 `EXT-024` records that `render_kind` has zero consumers, so the two should be settled together).
- `crates/cyrup-provider/src/session_resources.rs:48` / `:67` — a faithful port of `packages/ai/src/session-resources.ts` with no registrant and no dispose caller. pi's only registrant is the codex WebSocket cleanup (`api/openai-codex-responses.ts:927`), and cyrup has no WS transport by design-note (`api/openai_codex_responses.rs:39-46`). It is pre-wiring for a transport that does not exist; the real item is that transport.
- `crates/cyrup-permission-system/src/jsonc.rs:33` (`parse_ordered`) — zero references including tests; every caller goes through `parse_ordered_config`. Upstream has no counterpart behaviour to lose.

Also dead on **both** sides, so purely cosmetic: `crates/cyrup-provider/src/legacy_api_aliases.rs`
(mirrors pi's own deprecated shim) and `nested_events.rs`'s `is_top_level_async_dir` /
`nested_artifact_env` (upstream's `nested-events.ts` definitions have no call sites at v0.43.0 either).

**Do not confuse this section with the `cyrup-original` items in the census.** Those are invented
*behaviour*, not dead code — they run, they diverge from pi, and each needs a decision (delete, or
justify with a `[CYRUP-DELTA]` note).

> **UPDATED 2026-08-14 (fourth edition): the class is no longer 21 items. It is 46 open rows out of
> 68 filed, and it now has its own section and its own count in `00-residual-ledger.md`.** 66 of the
> surface enumeration's 191 findings are surfaces cyrup has and pi does not — 31 of them newly filed
> and open. **Rate them by REACHABILITY**: an advertised-but-dead surface (`TUI-063` —
> `CYRUP_SHARE_VIEWER_URL` is in `--help` and read by nothing; `EXT-071` — a WIT comment advertising a
> `ToolInfo.source` field that was already removed from the emitted object) outranks an internal
> helper, and a mechanism port the language forces (a WASM guest cannot hand a function back, so
> pi's callback values necessarily become WIT exports) is not divergence at all. **The most dangerous
> members are the ones other analysis has already built on**: `SEAM-080`'s cyrup-invented
> `model_changed` RPC line is reasoned about as upstream by two existing backlog items, exactly as
> `SEAM-100`'s missing `cyrup update --models` was. By area the open set is 05 → 14, 08 → 11,
> 06 → 8, 01 → 5, 07 → 5, 04 → 1, 09 → 1, 11 → 1.

Three of them are the highest-severity invented surface in
the port, and two of those are the whole reason this class needs a decision rather than a backlog
slot:
- **`EXT-054` (UW-13) — *critical***: an advertised per-extension sandbox that grants nothing.
- **`TOOL-039` — *high***: `CYRUP_SHELL` is the FIRST arm of `ShellConfig::detect()` (`ops/shell.rs:101-105`, ahead of the `/bin/bash` probe at `:108-110`), which is the default path for both `ToolRegistry::with_builtins` (`registry.rs:54`) and `Backend::default()` (`ops/mod.rs:359-361`); it is structurally excluded from `session_env_scrub_keys()` (built from `SESSION_ENV_SUFFIXES` × `{CYRUP_,PI_}`, `config.rs:31-48`), so it survives into subagent re-execs; nothing records the resolved interpreter anywhere; and the substitute need not be a shell, since the value goes straight to `get_bash_shell_config`. pi's `getShellConfig` (`utils/shell.ts:67-120`, byte-identical at both tags) reads **no** env var as a shell selector. **Decide it in ONE change with `TOOL-007`** (protected-path write block, on by default, no pi analog, bypassed by `bash`) — see OQ-9.
- **`PERM-023` — *high***: the permission extension's install probe never consults `agents_dir`, which the manager enforces, so an operator whose only policy artifact is agent-markdown frontmatter gets no gate attached and silently inert deny rules.

Also in this class: `TOOL-038` (on Windows with no bash, `bash` silently falls back to `cmd.exe /C` where pi refuses to run), `AGENT-016` / `AGENT-033` (panic handling that diverges from pi's failure semantics — both bounded by `[profile.release] panic="abort"`, which is why they stay medium/low), `SUBA-039` (`SpawnedChild` has no `Drop` guard, so a dropped drive future orphans a process group).

---

## 6. Open questions — need a human decision, not more analysis

1. **The npm package channel (PB-7).** The extension half is genuinely blocked — WASM guests cannot load a TypeScript extension. The skills/prompts/themes half is not blocked. Do we (a) support `npm:` for resource-only packages, (b) support it fully by treating `extensions` entries as unsupported-but-skipped, or (c) delete `npmCommand` from settings so it stops advertising a capability? Today it is (d): the setting exists and does nothing, and the error message blames OCI. The code comment at `package/source.rs:81` cites a requirement id (`R-09-021`) that **cannot be checked from this workspace** — not a decision of record.
2. **The compact-read `docs` arm (PB-4).** Upstream resolves against `dirname(getReadmePath())` — the shipped npm package tree. A Rust binary ships no such tree beside it. Porting the behaviour requires deciding what cyrup's "shipped docs root" is (embedded? install-relative? nothing?). Area 04 states the blocker precisely: **a packaged-docs locator has to exist before the arm can be written.**
3. **Windows (PB-19).** The broker's Unix-only bind is unambiguous, but `crates/` carries 161 `cfg(unix)` sites against 6 `cfg(windows)`, so this may be a property of the whole port. If Windows is out of scope for the binary, PB-19 reduces to its second half (the client resolves TCP targets a cyrup broker never serves, and the ported listen resolver is dead) — still real, but smaller. **This question also governs `DRIFT-046` (`normalizeWindowsShellPath`), `TOOL-036` (the win32 leg of `normalizePath`) and `TOOL-038`** (the `cmd.exe` fallback), so answer it once for all four. Note that `TOOL-036`'s `~`/`os.homedir()` half is a v0.83.0 parity bug on **every** platform and does not wait on this answer.
4. **SDK-surface parity vs behavioural parity.** `VL-P5`/`DRIFT-047` (telemetry), `SESS-038` (`session-backends/sqlite-node`), `SEAM-058` (`packages/{server,protocol,client}`) and `PROV-031` are all embedder-facing with no user-visible symptom in the cyrup binary. Is SDK-surface parity in scope, or do we track it separately? **Four separate area files asked this independently**, which is the signal that it needs deciding rather than re-litigating per item. `SESS-038` and `SEAM-058` are now trackers, as is `AGENT-028` (the other owner of the telemetry half), precisely because the answer — not more analysis — is what unblocks them.
5. **REFUTED 2026-08-14 (fourth edition) — CATALOG ACCURACY *IS* STATICALLY AUDITABLE, and this question is no longer open on its stated premise.** Filed as **`PROV-060`**. The "every `*.models.ts` is a two-line re-export" premise below is true only from **`a9f6a3159`** onward — the single commit that both gitignored `providers/data/` and converted the files to re-exports. Its **direct parent `b0c2a90e` still carries the full data literals, and `b0c2a90e` is the revision cyrup's own `catalog_manifest.json` names as its provenance floor.** `git log --oneline b0c2a90e..a9f6a3159` returns exactly one commit. So the whole catalog is checkable with `git show b0c2a90e:packages/ai/src/providers/<name>.models.ts` plus a short parser — no generator run, no `npm install`, no network. **It was done in this pass: 35/35 catalogs parsed, 1072 upstream models vs 1078 cyrup, 1027 compared field-by-field, yielding 25 missing models (`PROV-057`), 16 extra (`PROV-058`), 28 compat-flag differences and 119 non-compat field differences (`PROV-059`) — including the wire-API mismatch that is now the high `PROV-054`.** Nine sweeps inherited the "unverifiable" verdict; the data was one `git show` away the whole time. **`PROV-018`'s `xtask` generator is still the right fix site** — the same recipe is the drift check it needs — but it is no longer blocked on an unanswerable question. **Two caveats travel with every catalog claim derived this way:** `b0c2a90e` is **13 days earlier than v0.83.0**, so a clean refresh still leaves an unmeasurable residue and *"catalog parity at v0.83.0" is a claim about `b0c2a90e` plus an unbounded delta*; and a regeneration will **re-introduce** groq `qwen/qwen3-32b`'s deliberately-removed `thinkingLevelMap` (`PROV-064`) unless the generator carries a named exception list. *The original question is retained below for provenance; its factual premise is dead.* ~~**Catalog accuracy (VL-P25 / `PROV-004` / `DRIFT-009`) is not *statically* auditable — but it IS auditable.**~~ pi generates `providers/data/*.json` and gitignores the output (`pi/.gitignore:11`); every `*.models.ts` at v0.84.1 is a two-line re-export, so no pricing, context-window, `maxTokens` or compat-flag claim about the 35 embedded catalogs can be checked by reading this workspace. **The generator itself is committed at both tags** (§4), so the honest options are: accept structural parity only, or run pi's `generate:models` / `diff:model-catalog` and diff the result. **`PROV-018`'s `xtask` generator is the highest-leverage tooling item in the port** for exactly this reason, and `DRIFT-009` now defers to it rather than proposing the lossier pi.dev-artifact path.
6. **The first-run wizard (UW-2).** The standing trap list says it is "deliberately unreachable"; the code says `is_official_distribution()` is **true** for this build and the gate can fire into an empty `if` body. One of the two is wrong — and **as of the 2026-08-13 live run it is the trap list**: the gate was measured firing (the `xp` badge proves `CYRUP_EXPERIMENTAL=1` was read, the empty agent dir proves `settings_path` did not exist) straight into the empty body, on a pty where the sibling selectors rendered fine. Treat this as a product decision awaiting an answer, not as a documentation discrepancy. Decide whether the wizard ships (wire `startup.rs:256`) or does not (delete the predicate and the dead function, and correct the trap list) — it cannot stay in this state, because the current shape is the worst of both. **ANSWERED 2026-08-13 by ADR-0011: it ships.** `startup.rs`'s wizard is wired at pi's call position with pi's full condition, nothing was deleted, and the trap-list entry was struck from `README.md`. See `UW-2` above for the code, the tests, and the live run that is still owed. This question is closed; note for the record that the escalation token in `UW-2` used to read "OQ-6", which is wrong under both namespaces — this is `PARITY-PLAN` §7 **OQ-9** / `PARITY-GAPS` §6 q6.
7. **pi's agent-harness v2 (VL-P22 / `AGENT-028` / `DRIFT-040` / `SESS-038`).** ~11.4k insertions / ~10.9k deletions in `packages/agent/src/harness/**` in this delta — an `agent-harness.ts` rewrite, a new 667-line `reducer.ts`, a new `session/` subtree with its own JSONL codec/repo/storage/state and a 993-line conformance suite, and a new typed telemetry layer. **No area file owns it and nobody has measured it.** Do we (a) absorb it, (b) track interop only (read harness-v2-written sessions, keep writing the coding-agent format), or (c) declare it out of scope until pi's own `coding-agent` migrates? The torn-tail bug inside VL-P22 is small and should be fixed regardless of the answer. **`AGENT-028` and `SESS-038` are the same question in two files and must be answered together.**
8. **NEW — the alt-screen/TUI-mode scope decision (VL-P19 / `TUI-019`, filed in area 07 as `OQ-07-1`).** The previous edition held `TUI-019` at *low* "as a deliberate ADR-0001 divergence". That justification is dead twice over: ADR-0001 is **unreadable in this workspace** (§7), and even a real ADR would not hold it down, because a mechanism difference that costs behaviour stays as work. It is now rated on consequence — *medium*, for no fullscreen mode, no mouse scroll, no scrollbar and no jump-to-prompt, four normal-path features — and the underlying question is what needs a human: **does cyrup ship a fullscreen TUI mode at all?** (a) port it (effort **L+**, an application layer on ratatui's native alt-screen/mouse support), (b) support the flag and settings key as accepted no-ops with an explicit not-supported message, or (c) declare it out of scope and say so in the flag's error text. **`SEAM-051` and `CFG-021` must be fixed under every one of those answers and must not wait on it** — today the flag's default value makes the binary refuse to start.
   **ANSWERED 2026-09-04 by commit `dbcf59a` ("ship the fullscreen (alternate-screen) TUI mode, ADR-0005 B-1..B-14"), independently re-verified this pass, not taken from the commit message: it is (a).** `--tui-mode` is parsed and wired end to end — `crates/cyrup/src/cli/args.rs:184-185` through `App::switch_tui_mode` (`crates/cyrup/src/interactive.rs:207-217`) with flag-then-setting-then-regular precedence — into a real 13-module renderer under `crates/cyrup-tui/src/altscreen/`. `SEAM-051` (the flag rejected with exit 1) and `CFG-021` (`tuiMode`/`fullscreenScrollbar` settings keys) are both closed as a direct consequence, confirmed in their own area files (08, 05) this pass. `TUI-019` itself is closed in area 07. **Residual, stated so the next pass does not treat this as total fidelity**: this pass did not line-for-line diff cyrup's alt-screen renderer against pi's 1378-line `tui-alt-screen.ts`, and area 12's `DRIFT-022` tracker closure explicitly leaves that comparison to `TUI-019`'s own row rather than claiming it. Two settings keys landed too late to be in scope even for this residual (`fullscreenExitOutput`, `fullscreenCopyOnSelect` — pi v0.84.4, filed as area 05's `CFG-078`) and one more surface (`defaultTools`, `CFG-079`) is unrelated to alt-screen but was found in the same version-lag skim.
9. **NEW — what is `bash` allowed to be? (`TOOL-039` + `TOOL-007`, one decision).** Both are `cyrup-original` behaviour on the same surface and taking them separately produces an incoherent shell: either (i) delete the `CYRUP_SHELL` arm and require the `shellPath` setting — a three-line deletion, pi's shape, **recommended** — or (ii) keep it and do **all four** of stamp a `[CYRUP-DELTA]`, report the resolved interpreter at session start and in bash result details, add `CYRUP_SHELL` to the scrub set (it does not fit the `{CYRUP,PI}_<SUFFIX>` shape, so a second explicitly-named group is needed), and validate the path exists and is executable per `shell.ts:73`. **Half of (ii) is not an option.** The same decision governs whether `TOOL-007`'s protected-path block — on by default, no pi analog, and bypassed by `bash` — stays, is made configurable, or goes.

---

## 7. Method, and what to trust

**How this edition was produced.** Two stages. First, twelve areas were independently re-audited at
cyrup `04c1ba2` against a **named upstream tag** on both sides — never a working tree, never a
floating HEAD. Then a completeness critique read all fifteen finished files and returned 17 findings;
five repair agents applied them area by area, and this document and the README were regenerated from
the result. The class counts in §0 are mechanical: every row of every `## Open items` table tallied
by `Kind` and `Severity`, with `tracker` rows excluded by construction. Where an area file and this
document disagree, **the area file wins** — it is the one that re-read the code.

**Closure required reading the Rust at HEAD and the TypeScript at the tag.** A commit message
asserting a fix was treated as a hypothesis. That scepticism paid twice: area 01 found four follow-on
defects (`PROV-027`/`028`/`029`/`030`) *inside* the code that closed `PROV-005`, and area 11 found two
(`ICOM-027`/`043` inside `ICOM-022`'s fix, `ICOM-035` inside `ICOM-002`'s).

**Severity is now applied, not narrated.** The definition (README — data loss, silent wrong output, a
permission bypass, or a crash on a normal path) carries **no reachability qualifier**, and this pass
stopped adding one: `EXT-054` is critical although no WASM guest ships today, and its blast radius is
recorded inside the item as *scheduling* information. Two counter-cases are recorded with reasons so
they are not re-opened: `SEAM-051` is **high, not critical** — the failure is deterministic, loud,
diagnosed and one token from working, so it is a launch refusal rather than the silent/unrecoverable
class; and six items in areas 02/03/04 were tested against the definition and deliberately left where
they were (`AGENT-030` loss is race-conditional, `AGENT-016`/`AGENT-033` are bounded by
`panic="abort"`, `SESS-004` needs a fork/extension-written key, `SESS-042` is latent until `SESS-040`
lands a caller, `TOOL-035` needs a PNG with >4100 bytes of pre-`acTL` chunks). **A severity is a
consequence, not a class**, which is why two `cyrup-original` items sit in the high list.

**Citations, and the failure mode that made this pass necessary.** Every "identical at both tags"
claim in the fifteen files was re-resolved by opening the file at the named tag. The class defect —
quoting the **v0.84.1** offset while asserting it holds at **v0.83.0** — was found on the
number-one-ranked item in the backlog (`AGENT-020`: `agent.ts:350`/`:351-353` at v0.83.0, not
`:361`/`:362-364`) and on roughly twenty-five others: nine wrong-at-the-named-tag citations in area 01
alone, including a `high` that quoted a property (`isSubscription: true`) that **does not exist at
v0.83.0 at all**; thirteen more in area 02, where the `agent-loop.ts` shift is **not uniform** (0
through `:636`, +4 from `:642` on, because the block arm was rewritten); `TUI-028`'s two keybindings
offsets; `SEAM-006`'s `print-mode.ts` set; `PROV-024`'s four cites, which matched **neither** tag.
The rules that follow, and they are cheap:
- **Never write "identical at both tags".** Give the offsets per tag, each labelled with the tag it
  was read at. Byte-identical bodies do not imply identical line numbers.
- **Do not "fix" a citation by shifting it.** A previous renumber-by-uniform-shift pass introduced
  errors at 15% while looking verified.
- Area 01 proposes widening `PROV-041`'s in-tree citation lint to cover `docs/gap-analysis/*.md`,
  which would make this class machine-checkable instead of pass-dependent.

**Trackers, and why the count is cleaner for having them.** Nine rows propose no schedulable work —
they index other items, or they ask a scope question. They keep their IDs, their status rows and
their full bodies, and they are excluded from the severity census, because a number that mixes work
with bookkeeping cannot be planned against. Each records what would escalate it back (for
`SEAM-058`: the moment pi's `main()` references `experimentalCli`). Two of them are additionally
**leads** — `DRIFT-023` and `DRIFT-040`, where neither side was ever re-read — and area 12 lists the
exact commands that would settle each. **Items are held to a two-sided read; leads are not, which is
precisely why they sit outside the count.**

**Exhaustive vs sampled.**
- **Exhaustive**: the `v0.7.1..v0.8.0` permission-system diff, per shipping file; the `v0.9.2..v0.10.1` intercom window, commit by commit (14 of 14); the `v0.43.0..v0.47.1` subagents src sweep (96 commits, 12 net-new files, all read); the `v0.83.0..v0.84.1` diffs over `packages/ai/src` (52 files), `packages/coding-agent/src/core/tools/` (7 files, whole diff read), the area-03 session scope (37 files) and `packages/{server,protocol,client}` + the coding-agent CLI/modes tree (81+39 files); the CLI flag set, RPC verb set, RPC payload field set, settings key set and env var set, each diffed as a **set** rather than spot-checked.
- **Newly swept in the repair pass, because the critique proved nobody had read them**: `packages/ai/src/utils/` (`sanitize-unicode.ts`, `http-dispatcher.ts`, `json-parse.ts`, `abort-signals.ts`, `provider-env.ts`, `event-stream.ts`, `hash.ts`, `typebox-helpers.ts`) → `PROV-047`…`PROV-051` plus 9 confirmed-covered symbols and 6 mechanism-N/A carve-outs, each with its reason; `packages/coding-agent/src/bun/` (`cli.ts`, `register-bedrock.ts`, `restore-sandbox-env.ts`) → `DRIFT-051`, the rest carved out explicitly; `packages/tui/src`'s **input pipeline** (`stdin-buffer.ts`, `word-navigation.ts`, `undo-stack.ts`, `editor-component.ts`, `terminal-colors.ts`, `fuzzy.ts`) → `TUI-042`…`TUI-050`, including two criticals; `packages/coding-agent/src/cli/` (`session-picker.ts`, `startup-ui.ts`, `list-models.ts`, `config-selector.ts`, `initial-message.ts`, `file-processor.ts`) → `SEAM-061`…`SEAM-070`, **five of them high**; `packages/coding-agent/src/migrations.ts` → `CFG-048`…`CFG-051`. That five of ten items from one previously-unread directory came back `high` is the evidence for the point: **the axis, not the diligence, was the variable.**
- **Still sampled**: everything else in the 627-file pi delta. `packages/agent/src/harness/**` is **not measured at all** (OQ-7). Nine of eleven OAuth flows unread against upstream. The four large SSE decoders (~8k lines) read only along the paths each item required. `broker/mod.rs` (1559 lines) read in ranges; `transport/client.rs` (1268) and `framing.rs` (367) grepped, not read — and upstream rewrote the framing state machine at v0.9.1. Area 11's **surface-driven axis has never been run at all**: 68 top-level export declarations across the 17 non-test `.ts` files at v0.9.2, plus the tool action enum, the 8 subscribed event kinds and the 16-tag `BrokerMessage` vocabulary. Its commit-driven and citation-census axes are bounded by the drift window, so **none of them can see a symbol that existed at v0.9.2 and was never ported** — run it against `broker/` first.
- **The unwired sweep is incomplete by construction**: it indexed bare identifiers, not resolved paths, so any method whose name collides with a live method elsewhere was silently excluded (`SubagentFleetStatus::handle_key` was found only by reading). Of ~7 087 pub items it flagged 385, of which ~120 remain untriaged. **The true unwired set is larger than §2.** Closing it properly needs a type-resolved pass (`cargo +nightly rustdoc --output-format json` or rust-analyzer), not grep.

**Record what you EXCLUDED.** This is the method lesson of the repair pass and it is now README blind
spot 6. A surface dismissed as out of scope becomes invisible to every later pass, and the dismissal
is never re-examined: area 12's one-line rejection of pi's root `scripts/` as "dev/release tooling
with no runtime effect" is how the catalog generator came to be declared non-existent, and four whole
upstream subtrees went unread by anybody until this workflow. **Every sweep must publish its
exclusion list with a reason per entry**, so exclusions are auditable rather than silent. The repair
pass did this in each area's Coverage section — including the negative results, so nobody redoes
them.

**Static only.** Nothing here was built, run, tested or reproduced. Every `Verify` line in every area
file is a design, not an observation. **For TUI items this is not a formality**: ratatui `TestBackend`
unit tests pass while the assembled application has layout and empty-state bugs, so `TUI-*`,
`SEAM-061`/`062`/`063`/`066`/`067` and `SESS-040` are not done until they have been **run in a real
terminal**.

**Finally: `spec/`, `ADR-0001` and `CLAUDE.md` do not exist in this workspace.** Doc comments across
the codebase cite `spec/architecture/arch-NN-*.md`, `R-NN-NNN` ids and ADR numbers thousands of times.
Those are a useful search index and nothing more. **No entry in this document rests on one, and none
should.** Where a code comment invokes one to justify a divergence — as `package/source.rs:81` does
for the npm channel, `spawn/mod.rs:428` does for a 0600 mode the code never sets, and `TUI-019` did
for the whole alt-screen cluster until this pass — treat it as an unverifiable claim, not as a
decision of record.
