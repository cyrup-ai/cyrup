# 11 — cyrup-intercom

This area covers `crates/cyrup-intercom` — the Unix-socket supervisor↔subagent broker, its client transport, inbound-message delivery, presence/lifecycle reporting, the `intercom` / `contact_supervisor` tool surface, and the broker binary.

> ### RE-BASELINE 2026-08-27 — verified against cyrup HEAD `9962e0f`, tree clean
>
> **This file contradicted itself in four places.** Its header said **44 open**, `:63` said **14**,
> `:190` said **9**, and SWEEP 9 (`:165`, 2026-08-15) said **6**. The `## Status table` below still
> carried `still-open` for **36 items the newer `## Open items` table already recorded as CLOSED**.
> Anyone planning from the top of this file planned to re-implement work that already exists — which
> is what this pass was called to fix.
>
> **The lower table was right.** Every one of its dispositions was re-verified in the Rust at HEAD by
> reading the code, not commit messages, and excluding doc-comment matches — a comment citing an
> upstream symbol is not an implementation, and treating it as one is how a file like this goes stale.
> **Counted set against v0.10.1: 0 critical, 0 high, 3 medium, 1 low, plus 2 partially-closed = 6
> open**, unchanged from SWEEP 9 twelve days earlier — but see the next block: v0.10.1 is not
> upstream's current version, and the true counted set is **11**.
>
> **The suite now runs and is green — which no prior pass in this file could say.** Every earlier
> sweep was static (`cargo check` only) and said so. This one executed:
> `cargo nextest run -p cyrup-intercom` → **279/279 passed**, and
> `cargo nextest run -p cyrup-it --features it -E 'binary(intercom)'` → **79/79 passed, 0 skipped**.
> Both were re-measured at `9962e0f` itself, not carried over from an earlier commit. The integration
> run covers the real-socket paths
> (`broker_roundtrip::child_to_broker_to_supervisor_round_trip_over_the_real_socket`,
> `child_bridge_activation::production_spawned_child_registers_on_the_broker_and_round_trips_with_its_supervisor`).
> The `cyrup-it` compile-error backlog this crate carried is also discharged: `cargo check -p cyrup-it
> --features it --all-targets` returns **0 errors** at `9962e0f`, and the `.flux` task tracking it is
> already `status: done`.
>
> **The six that are open, each re-verified at HEAD:**
>
> | ID | Sev | Verified at HEAD `9962e0f` |
> |---|---|---|
> | ICOM-016 | low | `broker/extensions.rs:136,:171` — `handle_extension_publish` and `handle_extension_state_commit` only ever emit refusals (`committed: false`, `"Session has not advertised extension capability"`). Protocol modelled, effects genuinely absent. Unchanged. |
> | ICOM-024 | medium | Blocked two-deep, as sweep 9 recorded: the seam `render_call(&self, _key, _call)` (`cyrup-ext/src/native.rs:617`) carries no `options`/`theme`, **and** ICOM-029's `details` is still absent. Note the renderers themselves exist and are wired (`cyrup-intercom/src/tools/render.rs` × 4, dispatched at `extension.rs:700-711`) — the gap is the seam, not the renderer. |
> | ICOM-029 | medium | `cyrup-ext/src/host/services.rs:437` `inject_message` takes four parameters (`_content`, `_custom_type`, `_display`, `_trigger_turn`) — no `details`. **Citation drift corrected: the row said `:417`.** |
> | ICOM-042 | medium · partial | The cwd half **is** ported — `cwd.rs` (`normalize_cwd`, `same_cwd`, `resolve_path`) and `project_target.rs` (`resolve_target_in_cwd`, `format_session_refs`). `openProjectPaneIfMissing` appears in comments only; that half stays unported. |
> | ICOM-052 | low · partial | The SUN_LEN **diagnostic** is present — `broker/lifecycle.rs` names the endpoint and its byte length on bind failure. The **guard** is deliberately absent, recorded there as a CYRUP-DELTA because upstream has none either. |
> | ICOM-053 | medium | Confirmed unchanged, and now measured: `cyrup-it` is `required-features = ["it"]`, there is **no `.github/` in this repo**, and `xtask` does not invoke the suite. The 79 tests above execute only when a human types `--features it`. This is the largest live risk in the area — 6,777 lines of integration proof that nothing runs by default. |
>
> ### The baseline itself is measured against the wrong upstream version
>
> **This was found by cloning `pi-intercom` and reading it — the first pass in this file's history to
> do so.** Every prior pass, and the first edition of this re-baseline, took upstream's state from
> this document's own citations. `tmp/` is gitignored, so no checkout was present; the claims were
> second-hand, and one of them was wrong in a way no amount of re-reading the Rust could reveal.
>
> **This file measures parity against `pi-intercom` v0.10.1. Upstream is at v0.12.0** (`ef95f19`,
> 2026-08-22), clone at `tmp/pi-intercom`. The window **`v0.10.1..v0.12.0` is 8 commits,
> +2006/−261 across 15 files (`broker/broker.ts` +713, `index.ts` +370,
> `intercom.integration.test.ts` +790) and has never been audited by any pass.** The doc's stated
> drift window `v0.9.2..v0.10.1` is closed; this one is not open — it is unopened.
>
> Three new wire fields land in `broker/protocol.ts` at v0.12.0 — `provenance`
> (`MessageProvenance`), `endpointEpoch`, `tmuxPane` — and cyrup models **none** of them (0
> occurrences each in `transport/protocol.rs`). cyrup's own `epoch` (`connect.rs:152`) is an
> unrelated local reconnect counter, not upstream's broker-owned endpoint epoch; do not read the
> name as coverage. **Interop does not break:** the v0.9.2 envelope's `#[serde(flatten)] extra`
> capture (`transport/protocol.rs:298`) round-trips the unknown keys, so a v0.12.0 peer is tolerated
> rather than fatal. The features behind them are simply inert.
>
> Filed as **ICOM-054…ICOM-059** below. **Counted set is therefore 6 + 6 = 12 open.** Five are unported
> features from the v0.10.1..v0.12.0 window. **ICOM-056 straddles the boundary, and the halves have
> different origins** (pinned 2026-08-27 with `git log -S` against the clone): its *registry* pair
> (`intercom:extension-register`, `:extension-registry-ready`) landed at **v0.8.0** (`db22c07`) and is
> present at v0.9.2 and v0.10.1 — older than this port's own baseline and never filed by any pass,
> which is the surface-sweep blind spot `## Coverage` names. Its *outbox* pair, both V1 interfaces,
> the ten result codes and `MessageProvenance` landed at **v0.12.0** (0 hits at v0.10.1) and are
> inside the unaudited window. Every one of the twelve now has a task file under `.flux/todo/`.

> **Everything above this block, and the whole `## Status table` below, is historical.** The table's
> per-item notes are kept because they record how each finding was originally derived, but its
> **Status column is superseded by `## Open items`**. Where the two disagree, `## Open items` wins.


> **Re-audited 2026-08-12, cyrup HEAD `04c1ba2`** (last code commit; docs HEAD `a9000b1`, tree clean), against **`pi-intercom` v0.10.1** with the version-lag window measured over **`v0.9.2..v0.10.1`** (24 files, +2495/−700, 14 commits, each accounted for below).
>
> **The headline correction of this pass is the baseline itself.** Every prior version of this file — and `PARITY-GAPS.md:19` — records the ported baseline as **v0.7.0**. That is wrong. A citation census over `crates/cyrup-intercom/src` returns **v0.9.2 × 272**, v0.7.0 × 14, v0.8.0 × 3, v0.6.0 × 1 (the `lib.rs` banner), v0.10.x × 0. Load-bearing v0.8.0/v0.9.x code is present *and tested*: `broker/runtime_claim.rs` + `tests/broker_runtime_claim.rs` (v0.8.0), `/intercom-id` + `tests/intercom_id_command.rs` (v0.8.0), `format_context.rs` + `tests/presence_context_usage.rs` + `tests/session_info_context_fields.rs` (v0.9.x), the full 16-tag `BrokerMessage` union and the v0.9.2 envelope with `#[serde(flatten)] extra` (`transport/protocol.rs`), and `transport/target.rs` + `transport/stream.rs`. **The true ported baseline is v0.9.2**, and the real drift window is v0.9.2..v0.10.1 — which is what ICOM-035…ICOM-047 cover. ICOM-012 carries the banner fix.
>
> **This pass: 3 closed** (ICOM-007, ICOM-019, ICOM-020 — all verified in the Rust at HEAD, not from commit messages), **0 reopened**, **24 newly filed** (ICOM-027…ICOM-050). Three prior closures (ICOM-001, ICOM-002, ICOM-022) were adversarially re-checked and survive; three items moved to `partially-closed` (ICOM-009, ICOM-015, ICOM-026) and stay open on their remaining half. Two closures produced new defects sitting *inside* the closing code (ICOM-022 → ICOM-027, ICOM-043; ICOM-002 → ICOM-035).
>
> ~~Open now: **0 critical, 0 high, 22 medium, 22 low** (44 items).~~ **STALE — superseded by RE-BASELINE 2026-08-27 above: the verified count at HEAD `9962e0f` is 12 (6 against v0.10.1 + 6 newly filed).** Treat that as a floor — see `## Coverage`.

> ### REPAIR PASS — 2026-08-12 (second pass, same day), applying the completeness critique
>
> Read-only static repair. No Rust or TypeScript touched. **No item was added, removed, re-rated or
> reclassified: the counts above are unchanged at 44 — 0 critical, 0 high, 22 medium, 22 low.** The
> critique's finding against this file was about **method disclosure**, not about any item, and every
> item was re-checked for the classes it named (commit-hash-only evidence; items proposing no work;
> duplicate IDs) before concluding that none applies here. That check is recorded in
> `## Coverage → Repair-pass verification` so it is not redone.
>
> **What changed: the sweep axis is now stated, and the axis that was *not* run is named as a blind
> spot.** This file's 24 new items came from exactly two sources — a **commit-driven version-lag
> sweep** over the 14 commits in `v0.9.2..v0.10.1` (13 items) and an **adversarial re-audit of the
> code that closed three prior items** (11 items). Both are legitimate axes and both are now labelled
> as such. Neither is the **surface-driven sweep** README structural blind spot 1 prescribes, and
> this file had never used the term. Because a commit-driven sweep is bounded by the drift window by
> construction, it is structurally incapable of finding anything that predates **v0.9.2** and was
> never ported — which, given that the ported baseline was itself only corrected to v0.9.2 in this
> same pass, is a live risk rather than a theoretical one. See `## Coverage → Sweep axes` and
> **blind spot 10**.
>
> Two cheap surface axes *were* spot-checked during the repair (config keys and env vars, below);
> both came back clean, which is recorded so the next pass spends its budget on the axes that are
> still open rather than re-running these.

> ### Reconciliation 2026-08-14 — sweeps 1 and 2 applied, counts re-derived
>
> **cyrup HEAD `380c713`** (this file was written against `04c1ba2`), tree clean. Two whole-backlog
> parity sweeps have landed since this file was last edited: **sweep 1 — 232 items across 11 crates**,
> and **sweep 2**, run under the same rules. Area agents were forbidden from editing documentation so
> that a single writer could reconcile all sixteen files in one pass; this block, and the dispositions
> written into the `## Open items` rows below, are that reconciliation. **Every status in this file
> that predates this block is stale — including the header notes above it and the
> `## Status of every item…` table.**
>
> **No ID was renumbered, merged or deleted.** A refuted item keeps its ID with the refutation
> recorded in its row, so nobody re-derives it. Refutations are corrections to *this analysis*, not
> failures of the sweep — see `00-residual-ledger.md`, which now publishes the measured error rate.
>
> **The test architecture changed underneath every path citation in this file.** The integration
> tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` / `d973906`), taking the
> suite from **310 integration binaries to 6 + 8 gated** behind a new **`cyrup-it`** harness crate.
> The gate is now **6440 tests / 6440 passed / 8 skipped in 16.4 s**. Any citation of the form
> `crates/<crate>/tests/<x>.rs` in this file is stale unless it names `cyrup-it`, and note that
> `cyrup-it` is `required-features = ["it"]`, so **the gate does not build or run it**.
>
> **Still a static analysis.** Neither sweep executed the suite: area agents were restricted to
> `cargo check -p <crate> [--all-targets]` and the orchestrator ran the gate once over the combined
> work. Every red-before/green-after claim below is a reasoned argument plus a type-check, and every
> `Verify` line in this file remains a design, not an observation.
>
> **Area 11 — recount: 45 rows → 14 open (0 critical · 0 high · 6 medium · 8 low).** 31 rows are
> closed; the area has no critical and no high and never did.
>
> **`ICOM-013` is REOPENED IN PART, deliberately.** Sweep 1 closed four of its five sites and found a
> fifth divergence while porting; sweep 2 fixed the `NON_INTERACTIVE_BUSY_NOTICE` half and **refuted
> three more of the item's cyrup sites** (the "Cannot send an intercom message to yourself." /
> "Cannot ask yourself." pair does not exist — all three arms already use pi's single "Cannot message
> the current session"). One live instance remains and was NOT half-fixed on purpose:
> `crates/cyrup-intercom/src/extension.rs:398` returns `format!("Message sent to {target}.")` where
> v0.10.1 index.ts:2429 is `notifyIfLive(ctx, \`Message sent to ${targetLabel}\`, "info", overlayGeneration)`,
> differing on BOTH the trailing period and the label (`formatSessionLabel`'s duplicate-aware
> `name (id8)` form vs cyrup's raw caller token). Fixing only the period would leave the item open AND
> red an off-gate `cyrup-it` assertion.
>
> **A STRUCTURAL FINDING ABOUT THE GATE, not just about `ICOM-026`.** Four `cyrup-it` assertions
> (`tests/intercom/tool_actions.rs:319`, `:372`, `:502` and
> `tests/intercom/intercom_command_transcript.rs:142`) pin a trailing period production stopped
> emitting when `ICOM-013`'s closed half landed. **They are green only because `cyrup-it` is
> `required-features = ["it"]` and is therefore not built or run by `cargo test --workspace` — its own
> Cargo.toml states this at `:26-34`. The 6440-test gate gives NO coverage of the broker-socket seam
> tests at all.** Filed as structural defect J in `00-residual-ledger.md`.
>
> **`ICOM-033` cannot be closed by the renderer half alone, and that blocker is not currently in the
> item**: `tools/mod.rs::text_result` sets `details: None` and every arm of the `intercom` tool returns
> through it, so `render_result`'s load-bearing `details.messageId` would have nothing to read. The
> details-emission half must be sequenced first, and it is not purely mechanical — cyrup returns
> `ToolError` where upstream returns a non-error result with `details.delivered === false`.
>
> **NEW BLIND SPOT, added to Coverage:** the crate's `#![forbid(unsafe_code)]` means no test can
> `set_var`, so ANY env-driven behaviour here is untestable unless the code takes the value as a
> parameter. `ICOM-038` had to add `IntercomClient::connect_target_with_liveness` for exactly this
> reason, and `paths.rs:46` already records the same constraint. **A future env-reading port that does
> not carry an injectable form will ship untested.**
>
> **`crates/cyrup-intercom/src/project_target.rs` is a NEW module** (sweep 2, `ICOM-042`) and should be
> added to the Coverage file list; it carries that pass's regressions. `ICOM-012`'s baseline
> correction (v0.7.0 → **v0.9.2**) is now applied to `PARITY-GAPS.md:19` and `README.md`'s baselines
> table, which had both been carrying the wrong figure.

> ### RE-AUDIT 2026-09-04 — cyrup HEAD `2571969` (210 commits past the `4fb5e40` re-audit baseline),
> upstream re-checked at `pi-intercom` **v0.13.0** (`199279a`)
>
> **The RE-BASELINE 2026-08-27 block's "6 + 6 = 12 open" is STALE — superseded by `## Open items`
> below, where the true count is now 5 open + 2 newly filed = 7.** `git log --oneline
> 4fb5e40..HEAD -- crates/cyrup-intercom` returns 11 commits, and `.flux/todo/ICOM-*.md` /
> `.flux/done/**/ICOM-*.md` were both walked: **ICOM-024, ICOM-029, ICOM-016, ICOM-042 and ICOM-056**
> each have a `.flux/done/…/ICOM-NNN_*.md` task file marked `status: completed`, and **all five were
> independently re-verified by reading the Rust at HEAD**, not taken from the task file's own claim
> (a completed task file is a hypothesis, not evidence, per this directory's rule) — the closures
> below cite the actual code read. **ICOM-058** likewise has a done task file and was verified the
> same way. **ICOM-054, ICOM-055, ICOM-057 and ICOM-059 have NO done task file** (054/055/057 sit in
> `.flux/todo/`, unstarted), and a `grep` for their subject matter (`endpoint_epoch`, `scope_id`,
> a persisted pending-ask record, `tool_visibility`) across `crates/cyrup-intercom/src/**/*.rs`
> confirms none of the four is implemented — so they were re-verified as **genuinely still open**,
> except **ICOM-059**, whose upstream feature was itself withdrawn (see its row): that one closes,
> but as a refutation of the item's premise, not as a port.
>
> **Six closures, one refutation, five still open, two new:**
>
> | ID | Disposition | Verified at HEAD `2571969` |
> |---|---|---|
> | ICOM-024 | **CLOSED** | `render_live` seam lands (`cyrup-ext/src/native.rs:745-751`, dispatched at `cyrup-ext/src/facade.rs:1365`/`:2295`); `IntercomExtension::render_live` (`extension.rs:796-807`) rebuilds the card live, at width/theme. |
> | ICOM-029 | **CLOSED** | `HostServices::inject_message` (`cyrup-ext/src/host/services.rs:446-455`) carries `_details: Option<&Value>`; `inbound.rs:247-256`/`:284-292` build and pass it, and hardcode `display: true`. |
> | ICOM-016 | **CLOSED** | `broker/extensions.rs` (owner election), `broker/extension_state.rs` (sha256, capped, revisioned store), `broker/session.rs:157-164` (`features: Some(vec![EXTENSION_BUS_FEATURE...])`). |
> | ICOM-042 | **CLOSED** | `project_pane.rs` (777 lines: `HerdrClient`, `HerdrErrorCode`, `ProjectPaneLaunch`); `tools/intercom/mod.rs` `IntercomParams` carries `cwd`/`open_project_pane_if_missing`/`focus`; `send.rs`/`ask.rs` relax `to` to `to \|\| cwd`. |
> | ICOM-056 | **CLOSED** | `outbox.rs` (671 lines: both V1 envelopes, `parse_outbox_request`, generation-fenced delivery); `MessageProvenance`/`ProvenanceKind` (`transport/protocol.rs:318-403`). The one residual the closing task file flagged (`delivery_failed` emitting an empty `messageId`) is already handled at `outbox.rs:594-600`. |
> | ICOM-058 | **CLOSED** | `identity.rs:89-116` (`ENV_TMUX_PANE`, `current_tmux_pane`), `connect.rs:601`, `broker/session.rs:105`, `tools/intercom/mod.rs:358-361`. |
> | ICOM-059 | **CLOSED — REFUTED (upstream reverted)** | `toolVisibility` was added at v0.12.0 (`12f4b6c`) and deleted two commits later at v0.12.1 (`ee0d74c`, "fix: remove lazy intercom tool visibility (#120)"); absent from `config.ts` at v0.13.0. cyrup's non-port of a withdrawn feature is now correct. |
> | ICOM-052 | still open | Unchanged — no `SUN_LEN` length guard or hashed fallback in `paths.rs`/`transport/target.rs::unix_socket_path`; fix (b), the stderr capture, is confirmed landed (`transport/spawn.rs:168`,`:243-249`) but is not this item's residual. **Left open — not re-reproduced on this host** (the original finding was `observed`, and I did not re-run the long-`HOME` repro). |
> | ICOM-053 | still open | Unchanged — no `.github/` directory exists anywhere in the repo (`find … -iname "*.yml" -o -iname "*.yaml"` over the tree is empty outside `tmp/`/`target/`), and `xtask/src/features.rs`'s `cyrup-it` matrix row is a `check` (type-check), not a test run, and is `slow: true` / skippable with `--fast`. The process decision this item asks for is still not made. |
> | ICOM-054 | still open | Unchanged — `grep -rn 'endpoint_epoch\|endpointEpoch' crates/cyrup-intercom/src/` is empty; `.flux/todo/ICOM-054_ENDPOINT_EPOCHS.md` unstarted. |
> | ICOM-055 | still open | Unchanged — `grep -rn 'scope_id\|scopeId\|SCOPE_ID' crates/cyrup-intercom/src/` is empty; `.flux/todo/ICOM-055_SCOPED_ROUTING.md` unstarted. |
> | ICOM-057 | still open | Unchanged — no persisted pending-ask record in `crates/cyrup-intercom/src/`; `.flux/todo/ICOM-057_PERSIST_PENDING_ASKS.md` unstarted. |
>
> **Upstream moved v0.10.1 → v0.13.0** (`v0.12.0` `ef95f19` → `v0.12.1` → `v0.13.0` `199279a`) since
> this file's last upstream read. `git -C tmp/pi-intercom log --oneline v0.10.1..v0.13.0` = 14
> commits; 11 were already accounted for (the ICOM-054…059 filing plus the six above). **Three were
> not previously seen and were each read with `git -C tmp/pi-intercom show <sha>`:**
>
> - `5fe0ee3` (v0.12.1, #119, "fix: guard active intercom replies") — a genuine new gap, filed as
>   **ICOM-060**.
> - `90e6ad4` (v0.13.0, #126, "feat: add intercom alias command") — a genuine new gap, filed as
>   **ICOM-061**.
> - `dd1b36b` (v0.13.0, #125, "fix: harden Windows broker launcher") — **not filed.** Purely a
>   `wscript.exe`/VBScript encoding fix inside upstream's own Windows launch mechanism
>   (`broker/spawn.ts`'s `writeWindowsHiddenLauncher`); cyrup has no VBScript launcher at all — the
>   broker is a native re-exec (`current_exe __intercom-broker`, `transport/spawn.rs:130-140`), the
>   same mechanism-forced carve-out already recorded for ICOM-047's tsx half. No cyrup counterpart
>   exists to be out of parity with.
>
> **Not walked this pass, so not claimed clean:** `broker/mod.rs`'s un-swept ranges (blind spot 1),
> `transport/client.rs`/`framing.rs` (blind spot 2), the `ui/compose.rs`/`session_list.rs` overlays
> (blind spot 3), and the axis-4 surface-driven sweep generally (blind spot 10) — none of those were
> re-read this pass; this was a targeted verification of items already on the table plus a
> diff-stat/commit skim of the upstream window, not a fresh surface sweep. `cargo test -p
> cyrup-intercom` was **not** re-run this pass (static read only, per the file's own standing
> caveat) — the 293/280/etc. pass counts cited above are the closing task files' own numbers, not
> reproduced here; the code-level verification is what closes each item, independent of those counts.

## Status table

> **SUPERSEDED 2026-08-27 — historical.** The Status column below is stale: 36 rows
> marked `still-open` are closed at HEAD `9962e0f`, each re-verified in the Rust. The
> notes are kept for how the findings were derived. **`## Open items` is authoritative.**

| ID | Status | Note |
|---|---|---|
| ICOM-001 | **closed** (6f667c5) · low | Adversarially re-checked at HEAD. `reply_tracker.rs:98-145` `resolve_reply_target` reproduces v0.9.2 `reply-tracker.ts:52-90` statement-for-statement: `reply_to` branch + the `to`-mismatch cross-check (:106-118), the terminal explicit-`to` filter (:121-129), `current_turn_context` (:131-133), single-pending (:135-140), both error strings (:142, :144). `HashMap`-vs-`Map` iteration order is immaterial — every order-sensitive branch is gated on `len == 1`. Closure holds at the v0.9.2 baseline; upstream **replaced** this function at v0.9.3 (`c3543d6`/`fd30948`), filed separately as **ICOM-036**, not as a reopening. |
| ICOM-002 | **closed** (7c3862b) · medium | `inbound.rs:99-116` `decide_inbound_policy` reproduces the `!isIdle → !hasUI → !message.replyTo` tree of v0.7.0 `index.ts:745-765`, including that the IDLE case is delivered regardless of `hasUI`; cyrup's extra `SurfaceOnly` variant is upstream's bare `return`. Dispatch reads `state.is_idle()` → `session_state.rs:119-121` → `AgentSession::is_idle` (`cyrup-session-svc/src/session.rs:598-600`). Upstream **deleted this entire branch** at v0.9.3 (`25ffb96`) — filed as **ICOM-035**. |
| ICOM-003 | still-open · low | `seams.rs:250-253` `IntercomClarifyChannel::ask` still reads `self.state.client()` directly; its two siblings (`seams.rs:144`, `:186`) use `ensure_connected(ConnectReason::Background)`. |
| ICOM-004 | **closed 2026-08-15** · medium | Ported to `resources/skills/pi-intercom/SKILL.md` + `src/resources.rs` + the `ResourcesDiscover` arm; two forced `[CYRUP-DELTA]`s (env-var spelling, the Herdr passages). |
| ICOM-005 | still-open · low | Register inside the pending window neither clears `shutdown_scheduled` nor aborts the task; the re-arm is lost. |
| ICOM-006 | still-open · medium | `sync_presence` hard-codes `name: None`; `ENV_INTERCOM_NAME_POLL_MS` has exactly one occurrence — its declaration. |
| ICOM-007 | **closed** · medium | **Closed this pass.** `tools/intercom.rs:259-300` emits `**Pending asks:**` + `- {who} · {message.id} · {elapsed}s ago · {preview}` with whitespace collapsed and `.chars().take(80)`; empty case `"No unresolved inbound asks."` (:268). Matches v0.7.0 `index.ts:1733-1755`. Regression test at `tools/intercom.rs:294-330` asserts the **message** id is the printed column. Residual nit, not enough to reopen: `split_whitespace().join(" ")` also trims where JS `replace(/\s+/g," ")` does not, and `.chars().take(80)` differs from `.slice(0,80)` on astral input. |
| ICOM-008 | still-open · medium | The `"ask"` arm never passes `params.reply_to`; `session_state.rs:249` hard-codes `reply_to: None`. The audit entry records a `replyTo` that was never sent. |
| ICOM-009 | **partially-closed** · medium | Context-usage half landed (`extension.rs:261-284`). Active-tool map and `model_select` subscription still absent. |
| ICOM-010 | still-open · medium | No mailbox; `grep -n mailbox src/broker/mod.rs` returns only the doc note at :590. |
| ICOM-011 | still-open · medium | `grep -rni 'stable_id\|stableId'` over `src/` and `tests/` returns zero. `/intercom-id` itself **is** now ported (`extension.rs:474-481`, `tests/intercom_id_command.rs`) — only the stable-id half remains. |
| ICOM-012 | **misdescribed**, still-open · low | Banner still says v0.6.0 — but the item's *fix* was wrong: the target is **v0.9.2**, not v0.7.0. Corrected in place. |
| ICOM-013 | still-open, **corrected** · low (was medium) | Five halves closed, four still open, one downgraded — and the item was **under-scoped** on the ambiguity message (upstream has two distinct errors, not one). See the item. |
| ICOM-014 | still-open · low | Presence type-checks at `broker/mod.rs:906-926` still precede the ownership filter at :928. |
| ICOM-015 | **closed 2026-08-15** · low | Named-pipe listen arm had already landed; sweep 9 added the loopback-TCP arm, `broker.port.json` publication/unlink, the `stateId` endpoint-auth gate and endpoint-derived `trustedLocal`. |
| ICOM-016 | still-open · low | Protocol half modelled (`protocol.rs:89` `EXTENSION_BUS_FEATURE`, `:767` `features`); effects half (owner election, fan-out, state store) absent. |
| ICOM-017 | still-open · low | Envelope fields modelled (`protocol.rs:310/:317/:320/:324/:328/:332`); receipts, dedupe, `cancel` lifecycle absent. |
| ICOM-018 | still-open · low | No `cwd.rs`; action enum has no `list-cwd`; raw byte compare at `tools/intercom.rs:364`. Now also blocks ICOM-042. |
| ICOM-019 | **closed** · low | **Closed this pass.** Producer `extension.rs:261-284` `current_context_usage` reproduces pi's two-tier omit/null/number semantics; wire tri-state at `protocol.rs:266-285` + `:709-717`; broker apply at `broker/mod.rs:915-926` + `:952-955`; renderer `format_context.rs:69-82` is a statement-for-statement port of v0.10.1 `format-context.ts:19-32`, wired at `tools/intercom.rs:352`. Tests: `tests/presence_context_usage.rs`, `tests/session_info_context_fields.rs`. |
| ICOM-020 | **closed** · medium | **Closed this pass.** `broker/mod.rs:1238` calls `runtime_claim::assert_no_live_broker(&pid_path)?` **before** the stale-socket unlink (:1242) and the bind (:1243) — the same ordering as upstream `broker/broker.ts:230-238`. Implementation `broker/runtime_claim.rs` (256 lines, three-way ESRCH/EPERM/alive probe) with `tests/broker_runtime_claim.rs` and `tests/broker_startup_fail_fast.rs`. |
| ICOM-021 | still-open · low | `connect.rs:455` sets `status: state.config.status.clone()` — the raw configured suffix, never `currentStatus()`. |
| ICOM-022 | **closed** (513e45a) · high | Adversarially re-checked and holds. `inbound.rs:248` and `:284` both build `build_inline_message(...).content_markdown()`, which produces `**📨 From {sender}** ({cwd}){replyInstruction}\n\n{body}` (`ui/inline_message.rs:70-84`), matching v0.7.0 `index.ts:665`. The third site (local subagent relay, `seams.rs:137-138`) routes through the same function. Tests at `inbound.rs:795`, `:837-841`, `:867`. **Two adjacent defects filed out of the closing code: ICOM-027 (`display=false`) and ICOM-043 (the v0.10.0 deslop removed the `📨`).** A third — the missing `_deliveryMetadata_` segment in the very same template — is ICOM-048. |
| ICOM-023 | still-open · medium | Spawn-then-install race intact at `inbound.rs:179-188`; both `delay_ms == 0` call sites confirmed (`extension.rs:587`, `:609`). Subsumed by ICOM-035 if that lands. |
| ICOM-024 | still-open · medium | No `register_message_renderer`; card frozen at width 80, `collapsed: true`, `PlainTheme`. Also blocked by ICOM-029. |
| ICOM-025 | **closed 2026-08-15 — refuted** · low | No test in this crate sleeps at HEAD; the `connect.rs` test is on a paused clock and the `inbound.rs` half went with ICOM-035's deleted queue. |
| ICOM-026 | **partially-closed** · low | The id-vs-original-target half is closed (`tools/intercom.rs:845-880` registers a peer named `reviewer` with id `peer-session` and asserts the echoed target is `reviewer`). The trailing-period half is pinned at **three** sites now: `:820`, `:875`, `:1016`. |
| ICOM-027 | **new**, open · low | Inbound messages injected with `display=false`. Impact is narrower than first filed — see the item. |
| ICOM-028 | **new**, open · low | `append_entry("intercom_message", …)` has no entry renderer. |
| ICOM-029 | **new**, open · medium | `HostServices::inject_message` carries no `details`; blocks ICOM-024. |
| ICOM-030 | **new**, open · medium | `contact_supervisor` registered even when a native supervisor channel is active. |
| ICOM-031 | **new**, open · medium | No presence identity re-sync on `turn_start` or at the head of a tool call. |
| ICOM-032 | **new**, open · low | `SessionShutdown` never drains `pending_idle`. |
| ICOM-033 | **new**, open · medium | Neither tool registers a tool renderer. |
| ICOM-034 | **new**, open · low | Subagent relay has no self-target guard. |
| ICOM-035 | **new**, open · medium | Busy inbound parked until idle; upstream deleted the queue at v0.9.3 and steers instead. |
| ICOM-036 | **new**, open · medium | No sender-ID-prefix reply targeting; four v0.9.3 disambiguation errors absent. |
| ICOM-037 | **new**, open · medium | A plain `send` to the sole pending asker is not treated as its reply. |
| ICOM-038 | **new**, open · medium | No client liveness heartbeat; half-open broker socket never detected. |
| ICOM-039 | **new**, open · medium | `list` prints a fixed 8-char id instead of a distinguishing prefix. |
| ICOM-040 | **new**, open · medium | Unnamed-session fallback alias uses 8 id chars, not 18. |
| ICOM-041 | **new**, open · low | `runtimeFallbackAlias` neither modelled nor applied. |
| ICOM-042 | **new**, open · medium | cwd-scoped `send`/`ask` and `openProjectPaneIfMissing` unported. |
| ICOM-043 | **new**, open · low | v0.10.0 copy revision unported (`📨`/`📎`/`↩`/`↳` still present). Incomplete on its own — pair with ICOM-048. |
| ICOM-044 | **new**, open · medium | Malformed config fails closed silently instead of erroring with the path. |
| ICOM-045 | **new**, open · low | Blocking `ask` / supervisor decision not refused up front when the target is offline. |
| ICOM-046 | **new**, open · medium | `intercom({action:"reply"})` silently drops attachments. |
| ICOM-047 | **new**, open · low | Broker startup failures discard the broker's stderr. |
| ICOM-048 | **new**, open · medium | Injected content omits v0.9.2's `_deliveryMetadata_` line, so the model never sees the inbound message id. |
| ICOM-049 | **new**, open · low | Inbound delivery and flush carry no runtime-generation / liveness guard. |
| ICOM-050 | **new**, open · low | `intercom_received` audit entry drops `messageId` and `attachments` and re-timestamps. |
| ICOM-051 | **new**, **closed** (fixed) · high | Ambient `CYRUP_INTERCOM` satisfied `is_installed()` in four hermetic binary-seam fixtures; 13 immortal brokers per `-p cyrup` run, now 0. Carries the refutation of the proposed `CLOEXEC` code defect. |

Closed this pass: **3** (ICOM-007, ICOM-019, ICOM-020). Newly filed: **24** (ICOM-027…ICOM-050). Reopened: **0**. Filed by the later suite-verification pass: **1** (ICOM-051, closed on arrival).

## Open items

> ### BATCH 2 — 2026-09-04 (ledger audit): **2 closed, 5 open** — counted set **0 critical, 0 high, 2 medium, 3 low = 5**, 49 closed (`scripts/count_open_items.py`); authoritative over every block below.
>
> **`ICOM-053`** CLOSED at `1d2b4418` (the feature-matrix gate now RUNS the `cyrup-it` seam suite instead of type-checking it; two seam targets that had stopped compiling were fixed on the way) and **`ICOM-060`** CLOSED at `a91e3c41` (pi-intercom v0.12.1 `5fe0ee3`'s misdirected-reply guard, line for line). The two remaining mediums are `ICOM-054` (broker-owned endpoint epochs, v0.11.0) and `ICOM-055` (scoped routing, v0.12.0). **`ICOM-055` CLOSED later the same day at `c440c038`** (see its row), leaving `ICOM-054` as the only open medium in this area. **`ICOM-054` CLOSED 2026-09-05 at `87421cd7`** (see its row), which leaves **0 medium** open here: `docs/gap-analysis/scripts/count_open_items.py` at that sha reads area 11 as `0 critical, 0 high, 0 medium, 3 low = 3` open (`ICOM-016`, `ICOM-052`, `ICOM-057`) and **51 closed**. Note for the next pass: no `cyrup-it --features it` run happened after `ICOM-053`'s own; `SUBA-072`/`088`/`090` edited ~30 `cyrup-it` test files later the same day and were `cargo check`ed only (disk).
>
> ### SWEEP 9 — 2026-08-15, area 11 worked in full: **3 closed (2 ported, 1 refuted), 6 open**
>
> Counted set now **0 critical, 0 high, 4 medium, 2 low = 6** (was 5 medium / 4 low = 9).
> Closed this pass: **ICOM-004** (SKILL.md ported + declared through `ResourcesDiscover`),
> **ICOM-015** (the loopback-TCP broker half — the last piece of pi-intercom this port had never
> carried — plus a real `trustedLocal` defect found inside it), and **ICOM-025** (REFUTED: the
> residual does not exist at HEAD; all eight `sleep(` sites in this crate are production code).
> Verified against `pi-intercom` **v0.10.1** (`30dcbdd`), every citation re-derived at that tag.
>
> **Two of the closures discharge blockers this file recorded as needing a human decision, without
> taking that decision.** ICOM-004 was blocked on the Herdr yes/no because upstream's SKILL.md
> documents `openProjectPaneIfMissing`; the resolution is that a skill must describe the tool THIS
> build advertises, so the two Herdr passages are adapted under a `[CYRUP-DELTA]` naming the lines
> to restore, and ICOM-042's product question is untouched. ICOM-015 was blocked on "a Windows
> target available to `cargo check`"; the resolution is that the named-pipe arm had already landed
> and what remained — the TCP arm — is platform-independent in upstream too, so it binds, accepts
> and is tested on this macOS host.
>
> **The four still-open rows were each re-measured and their blockers restated (see the rows).** The
> one substantive correction: **ICOM-024 has a second blocker it has never named** — cyrup's
> renderer seam passes no `options`/`theme` and returns a `String`, so the live-width card cannot be
> drawn there even once ICOM-029's `details` land. Sequencing ICOM-029 first therefore buys nothing
> on its own, and ICOM-029 itself needs a field on `cyrup_agent::AgentMessage::Custom` (area 03)
> that its Fix does not mention. ICOM-016 was NOT attempted, deliberately: re-measured at ~450
> upstream lines and a first hashing dependency, with a half-landing (advertising
> `extension-bus-v1` without owner election) strictly worse than none.

> **RE-DERIVED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set UNCHANGED at 0 critical, 0 high, 5 medium, 4 low = 9** (46 rows: 37 fully closed, 9 open — 3 of them partially). No sweep-8 agent owned this area. **`ICOM-053` — the gate does not build or run `cyrup-it` — is unchanged and is now partially answered by measurement rather than by argument:** the integration suite does run, at **473 tests in 92 s behind `--features it`**, and it carries **guard tests that FAIL when ambient credentials leak in**. That is the harness the row asks for; what `ICOM-053` still names is that the 17.9 s merge gate does not invoke it.

> **SUPERSEDED — RECOUNTED 2026-08-14 (sweeps 3-6 reconciliation) — counted set: 0 critical, 0 high, 5 medium, 4 low = 9.** 37 rows are now marked CLOSED. **Sweep 6 closed six** — `ICOM-017` by porting the session/client/tool halves, and `ICOM-013`, `ICOM-026`, `ICOM-010`, `ICOM-028`, `ICOM-033` as REFUTED (all five were already closed at HEAD by sweeps between 3 and 5) — and filed one new (`ICOM-053`, re-filed out of `ICOM-026`'s structural half). *(Previous edition: 0 / 0 / 6 / 8 = 14, 31 closed.)*
>
> **THIS AREA'S TAIL IS GENUINELY BLOCKED RATHER THAN MIS-FILED, AND EACH BLOCKER IS A DIFFERENT KIND — three of five are DECISIONS, not work, and three sweeps have now correctly declined to make them on an agent's own judgement:** `ICOM-015` needs a **Windows build target**; `ICOM-024`/`ICOM-029` need one agent owning `crates/cyrup-ext` (the `HostServices` trait) **plus** `crates/cyrup-session-svc` (its impl) to widen `inject_message` with a `details` payload — the in-source note at `ui/mod.rs:19` states it, and until then a live `intercom_message` renderer would receive a bare markdown string and hit upstream's `return undefined`; `ICOM-004` and `ICOM-042` need **one yes/no on the Herdr pane launcher**; `ICOM-052`'s residual needs **one yes/no on relocating the broker socket** (the hashed temp-dir fallback moves it out of the agent dir and therefore changes where every peer looks for it — the item itself gates it on "if the fallback path is acceptable to the design"). `ICOM-016` is the one genuine L left (see its row).

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ICOM-052 | low — **PARTIALLY CLOSED 2026-08-14** | cyrup-original | S | The broker socket path has no `SUN_LEN` guard — **PARTIALLY CLOSED 2026-08-14** (fix (b), the named bind error + captured stderr); **residual re-confirmed open 2026-08-15 (sweep 9), unchanged and still a design decision**: fix (a) is a hashed temp-dir fallback socket path, which relocates the socket out of the agent dir and therefore changes where every peer looks for it — and pi has no such fallback either (`broker/paths.ts:65-74` has no length guard), so this is a deliberate divergence to authorize, not a parity gap to close. Reconfirmed at HEAD: `paths.rs:89` is still `intercom_dir.join("broker.sock")` with no length check. **NEEDS one yes/no on relocating the socket.** **Re-confirmed 2026-09-04: `transport/target.rs::unix_socket_path` still has no length guard or fallback; fix (b)'s stderr capture is confirmed still landed (`transport/spawn.rs:168`, `:243-249`). Not re-reproduced on this host — the original finding was `observed` via a long-`HOME` repro that was not re-run this pass.** |
| ~~ICOM-004~~ | ~~medium~~ **CLOSED 2026-08-15** | not-ported | M | `skills/pi-intercom/SKILL.md` not ported — **CLOSED 2026-08-15**: sweep 9 — ported as `crates/cyrup-intercom/resources/skills/pi-intercom/SKILL.md` (452 lines, v0.10.1) and declared to cyrup's discovery through the SAME `cyrup_resources::resolve_manifest` plumbing `cyrup-ext-subagents` uses: new `crates/cyrup-intercom/src/resources.rs` (`bundled_skill_files`, pi's `SKILL.md`-then-`.md` walk) + `EventKind::ResourcesDiscover` in `init` + the `HookOutcome::Handled({"skillPaths":[…]})` arm in `on_event`. **The Herdr blocker was DISCHARGED WITHOUT a Herdr decision, not deferred:** the two `openProjectPaneIfMissing` passages (upstream `:145-157`, `:241-248`, the bullet at `:26` and a clause at `:228`) are adapted rather than carried, because that parameter is deliberately absent from the tool schema (ICOM-042's residual) and documenting it would instruct the model into an unknown-field rejection; the cwd-TARGETING examples, which cyrup DOES advertise, are carried verbatim and one `list-cwd` example was added. Second forced delta: `PI_INTERCOM_ASK_TIMEOUT_MS` → `CYRUP_INTERCOM_ASK_TIMEOUT_MS` (5 sites) — pi's spelling names a variable this build ignores. Both deltas are `[CYRUP-DELTA] ICOM-004` YAML comments in the file's own front matter (the `researcher.md` precedent), each naming the exact upstream lines to restore. Tests: `extension.rs::the_bundled_skill_is_declared_to_resource_discovery` (red pre-fix on the subscription assertion) + two in `resources.rs`, one of which cross-checks every action the skill tells the model to call against `tools::intercom::parameters_schema()`'s enum, so a skill that documents an unadvertised action is red. — **2026-08-14, still open; BLOCKER LIST REWRITTEN 2026-08-14 (sweep 6)**: the `intercom({action:"cancel"})` blocker is **DISCHARGED** — `cancel` is a real advertised action as of sweep 6 (`ICOM-017`), so porting the skill no longer instructs the model to call it into a rejection. **The ONLY remaining blocker is `ICOM-042`'s Herdr half:** SKILL.md `:154` and `:244` document `openProjectPaneIfMissing`, which is gated on the open design question of whether cyrup ships a Herdr pane launcher at all. The port is otherwise mechanical (452 lines). **NEEDS an explicit yes/no on the Herdr pane launcher, or explicit sign-off to ship the skill with the two Herdr passages removed under a `CYRUP-DELTA`.** Sweep 6 deliberately did not port 452 lines minus two sections on its own judgement: which skills root, how the resource loader discovers it, and whether those passages are deleted or carried, are shipped-resource shape decisions. |
| ~~ICOM-006~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | No name poll; presence name fixed at registration — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-008~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `ask` silently drops `replyTo` — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-009~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | Lifecycle status has no active-tool map and no `model_select` hook — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-010~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | L | Broker mailbox for briefly-disconnected sessions absent — **REFUTED, CLOSED 2026-08-14**: sweep 6 — ported at HEAD by a sweep between 3 and 5: `mailbox_messages` (`broker/mod.rs:248-252`, an ARRAY with FIFO front-eviction at the cap), `queue_mailbox_message`, `flush_mailbox_for_session`, `prune_mailbox_messages`, `find_live_sessions_sharing_mailbox_identity` (`:472-500`, `:573-625`), with the superseded no-mailbox invariant note rewritten in place at `connect.rs:48` ("Superseded 2026-08-14 (ICOM-010)"). vs pi-intercom v0.10.1 `broker/broker.ts:890-953`. **The broker halves of `ICOM-017` landed with it, which is why that item's cyrup evidence read stale.** |
| ~~ICOM-011~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | Restart-stable session IDs absent; `stableId` silently ignored — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-023~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `schedule_inbound_flush(state, 0)` races its own task and aborts the retry — **CLOSED 2026-08-14**: sweep 1 — closed as SUBSUMED by ICOM-035, not as independently fixed: the machinery they described (`pending_idle`, `flush_timer`, `schedule_inbound_flush`, `flush_idle_messages`, `PendingInbound`, `InboundPolicy::Queue`) no longer exists. |
| ~~ICOM-024~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | M | No `intercom_message` renderer; card frozen at width 80 — **CLOSED 2026-09-04**: both named blockers are cleared. `InitApi::register_message_renderer(crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE)` is called from `IntercomExtension::init` (`extension.rs:468`). The renderer seam now carries live width/theme/expansion: `NativeExtension::render_live(&self, key, payload) -> Option<Arc<dyn RenderedComponent>>` (`cyrup-ext/src/native.rs:745-751`) is a new trait tier, dispatched via `crate::facade::MessageRenderOutcome::Live(Arc<dyn RenderedComponent>)` (`cyrup-ext/src/facade.rs:1365`, `:2278-2295`, doc: "the current width, theme and expansion"). `IntercomExtension::render_live` (`extension.rs:796-807`) reads `payload.get("details").or_else(|| payload.get("data"))`, builds `InlineMessage::from_details`, and returns `InlineMessageComponent`, re-resolved per frame exactly as upstream's `(message, options, theme) => Component` is. ICOM-029's `details` blocker (below) is what feeds it. The stale half-sentence at `ui/mod.rs:16-19` is now fully wrong, not half — both `register_message_renderer` and the live-component tier exist. |
| ~~ICOM-029~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | M | `inject_message` carried no `details` — **CLOSED 2026-09-04**: `HostServices::inject_message` (`cyrup-ext/src/host/services.rs:446-455`) now takes `_details: Option<&serde_json::Value>` as its fourth parameter (between `_display` and `_trigger_turn`, per the fix's own note on avoiding a silent bool-swap). `crates/cyrup-intercom/src/inbound.rs:238-256` (`send_incoming_message`) and `:281-292` (`trigger_turn_over_inbound`) both build `let details = serde_json::to_value(&card).ok();` and pass `details.as_ref()`, and both now pass `true` unconditionally for `display` (the ICOM-027 fix, already closed, landed alongside). This unblocks ICOM-024, closed above. |
| ~~ICOM-030~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | `contact_supervisor` registered alongside an active native supervisor channel — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-031~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | Presence identity never re-synced on `turn_start` or at a tool call — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-033~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | M | No tool renderers for `intercom` / `contact_supervisor` — **REFUTED, CLOSED 2026-08-14**: sweep 6 — `crates/cyrup-intercom/src/tools/render.rs` is the full `renderCall`/`renderResult` port for both tools (pi-intercom v0.10.1 `index.ts:1743-1774`, `:2298-2331`). **The sequencing blocker the 2026-08-14 note added is DISCHARGED:** `tools/mod.rs::text_result` no longer sets `details: None`, it sets pi's `{}`, and `detailed_result` carries the load-bearing `details.messageId`. The three unreachable-branch carve-outs that note demanded (no theme, no `isPartial`, no `context.isError`/expanded) are documented at the head of `render.rs`. |
| ~~ICOM-035~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | M | Busy inbound parked until idle instead of steered — **CLOSED 2026-08-14**: sweep 1 — the queue is DELETED, not merely fixed. Blind spot 7 ("confirm pi's `deliverAs:'steer'` means what `AgentSession::steer` means") is answered: `cyrup-session-svc/src/session.rs:3926-3928` routes any custom message to `agent.steer` whenever `is_streaming()`, so no HostServices change was needed. |
| ~~ICOM-036~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | No reply targeting by sender-ID prefix; four disambiguation errors absent — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-037~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | A `send` to the sole pending asker is not treated as its reply — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-038~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | M | No client liveness heartbeat — **CLOSED 2026-08-14**: sweep 2 — pi's client liveness heartbeat ported end to end: `CYRUP_INTERCOM_LIVENESS_INTERVAL_MS`/`_TIMEOUT_MS` with defaults 30 s/5 s and the `Math.min(raw, interval)` clamp applied in the CONFIGURED branch only (an unset timeout is a flat 5000 and is NOT clamped by a shorter interval — upstream's asymmetry, reproduced); a real `Number.parseInt(x,10)` port (`js_parse_int_base10`), NOT the `Number()` that `getNamePollMs` uses; `LivenessConfig`, `ClientInner.liveness_abort` (pi's `livenessTimer`) started at the connect success arm (pi's `onRegistered`) and stopped in `teardown` (pi's `onClose`) and `disconnect()`; `liveness_task` as an `interval_at` tick loop under `MissedTickBehavior::Skip`; `force_close` (pi's `socket.destroy()`) feeding the shared onClose tail; and `list_sessions_inner(inner, timeout)` driving the existing `list` round trip under `getLivenessTimeoutMs()`. The env names live in `identity.rs` (the crate's single env inventory), as the Fix directed. Two mechanism notes recorded so nobody "restores" them: pi's `livenessInFlight` boolean has no counterpart because the probe is awaited inline and `MissedTickBehavior::Skip` reproduces the same observable schedule; and `tokio::time::interval` fires its first tick IMMEDIATELY where `setInterval` waits one period, hence `interval_at(now + interval, …)`. |
| ~~ICOM-039~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | `list` prints a fixed 8-char id, not a distinguishing prefix — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-040~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | Unnamed-session alias uses 8 id characters, not 18 — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-042~~ | ~~medium~~ **CLOSED 2026-09-04** | upstream-drift | L | cwd-scoped `send`/`ask` and `openProjectPaneIfMissing` — **CLOSED 2026-09-04**: both halves are landed. The cwd-targeting half (`cwd.rs`, shared with ICOM-018; `project_target.rs::resolve_target_in_cwd`) had already closed. The Herdr half — the product decision this row was blocked on — is now resolved and ported in full: `crates/cyrup-intercom/src/project_pane.rs` (777 lines) carries `HerdrErrorCode`/`HerdrResult`/`HerdrClient`/`ProjectPaneLaunch`, `parse_herdr_version` (the leftmost-greedy-triple parse, tested), and `open_project_pane`, reading `HERDR_BIN` exactly as upstream's `process.env.HERDR_BIN ?? "herdr"` does (`project_pane.rs:349-374`). `IntercomParams` (`tools/intercom/mod.rs:48-105`) carries `cwd`/`open_project_pane_if_missing`/`focus`; `tools/intercom/send.rs:37-52` and `ask.rs:37-52` both relax the `to` requirement to `to \|\| cwd` and thread `open_project_pane_if_missing` through `CwdDeliveryOptions`. Landed via `.flux/done/2026-08-28-02-56/ICOM-042_OPEN_PROJECT_PANE.md`, whose own QA (9/10, `cargo doc -p cyrup-intercom --no-deps` exits 0, `cargo test -p cyrup-intercom` 293/0 failed) is corroborated by the code read above, not taken on the task file's word alone. |
| ~~ICOM-044~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | Malformed config fails closed silently — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-046~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | `reply` silently drops attachments — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-048~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | Injected content omits the `_deliveryMetadata_` line (message id) — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-003~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `IntercomClarifyChannel::ask` bypasses `ensure_connected` — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-005~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `register` inside the pending window leaves an idle broker alive forever — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-012~~ | ~~low~~ **CLOSED 2026-08-14** | stale-port | S | `lib.rs` claims v0.6.0 when the code is at v0.9.2 — **CLOSED 2026-08-14**: sweep 1 — the ported baseline correction: `lib.rs:2` now records **v0.9.2**, not v0.7.0 (the area's own citation census is v0.9.2 × 272 against v0.7.0 × 14). PARITY-GAPS.md:19 and README.md's baselines table are corrected in this reconciliation. |
| ~~ICOM-013~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | M | User-visible message strings still diverge at four sites — **REFUTED, CLOSED 2026-08-14**: sweep 6 — the named residual does not exist at HEAD, so the `PARTIALLY CLOSED` status was stale. `extension.rs:398` returning `Message sent to {target}.` (trailing period, caller's raw token) is gone: `extension.rs:410-419` computes `target_label` via `crate::identity::format_session_label(name, id, &duplicates)` and returns `format!("Message sent to {target_label}")` with **no** period, and `identity.rs:320` is the ported `formatSessionLabel`. vs pi-intercom v0.10.1 `index.ts:2429`. Landed after the sweeps 1-2 reconciliation. |
| ~~ICOM-014~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Broker `presence` validation runs before the socket-ownership check — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-015~~ | ~~low~~ **CLOSED 2026-08-15** | not-ported | M | Broker listen half of the named-pipe / TCP transport absent — **CLOSED 2026-08-15**: sweep 9 — **and the row's own framing was half stale: the named-pipe listen arm had already landed** (`broker/listener.rs::BrokerListener`, bound from `broker/mod.rs`'s `broker_listen_target` call), so the Windows-target unblock condition never applied to what was actually left. What was left is the piece the crate's module docs called "the one piece of pi-intercom this port has never carried" — the **loopback-TCP broker half**, which needs no Windows target at all because upstream's own branch is platform-independent (`getBrokerListenTarget` only SELECTS it on Windows). Landed: `BrokerListener::Tcp` binding pi's port-`0` listen target (`broker.ts:150-151`) with `local_addr()` for `this.server.address()` (`:132`) and the `noDelay` the client side already sets; `run` generating this process's `BROKER_STATE_ID` (`:29`) and publishing `{transport,host,port,stateId}` into `broker.port.json` at 0600 via the already-written-but-uncalled `BrokerTcpEndpoint::to_port_file_body` (`:132-141`), with the unconditional `unlinkSync(PORT_PATH)` at shutdown (`:1423-1427`); `BrokerState::{trusted_local, endpoint_state_id}` fed by `with_listen_endpoint`, giving pi's `requiresEndpointAuth`/`hasEndpointAuth` gate on `health` (`:290-292`) and `register` (`:303-305`). **A live defect was found and fixed while porting: `trustedLocal` was the compile-time `cfg!(unix)`, so a TCP-bound broker on a unix host stamped `trustedLocal: true` on every session — the inverse of `broker.ts:365`, on the one field a peer reads to decide a connection came from a process that could open the socket.** The refusal stub and its test are gone. Tests: `listener.rs::the_tcp_listen_target_binds_accepts_and_publishes_a_readable_endpoint` (binds, accepts a real client, and round-trips the published body back through `broker_connect_target`'s own validation ladder — a port read from the listen target instead of `local_addr()` publishes `0` and fails the client's `port <= 0` rung), plus two in `broker/mod.rs` covering all three credential-gate groups and the `trustedLocal` regression. Both were red pre-fix.
| ~~ICOM-016~~ | ~~low~~ **CLOSED 2026-09-04** | not-ported | L | Silent namespaced extension bus: effects absent — **CLOSED 2026-09-04**: the effects are implemented, including the sha2 dependency the row flagged as needing a decision. `crates/cyrup-intercom/src/broker/extensions.rs` carries owner election (`NamespaceOwner`, `recompute_namespace_owners`) and the shared `extensions`-field validators (`extensions_field_is_valid`, `namespace_is_valid`); `crates/cyrup-intercom/src/broker/extension_state.rs` is the checksummed, capped, revisioned store (`use sha2::{Digest as _, Sha256}`, `payload_sha256`, `MAX_EXTENSION_STATE_BYTES`, `read_envelope` rejecting a hash mismatch); `broker/session.rs:157-164` now sets `features: Some(vec![EXTENSION_BUS_FEATURE.to_string()])` on `Registered`, so the broker advertises the bus rather than silently refusing it. Landed via `.flux/done/2026-08-28-02-56/ICOM-016_EXTENSION_STATE_EFFECTS.md`; independently confirmed by the code read above, not the task file's own "port is complete" claim. |
| ~~ICOM-017~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | L | Delivery diagnostics (receipts, dedupe, cancel/supersede) absent — **CLOSED 2026-08-14**: sweep 6 — **and the item's cyrup evidence was STALE in a way that would mislead the next reader: the BROKER halves had already landed with `ICOM-010`'s mailbox.** `broker/mod.rs:595-607` does NOT always answer `Message cannot be cancelled by this session` (the full `handle_cancel_message`, with both the parked-mail and delivered-route arms, is `:922-990`), and `:447-456` does NOT validate-and-return (`handle_message_receipt` forwards, at `:745-779`). Sweep 6 landed the SESSION / CLIENT / TOOL halves, which is all that was left: **(1) `transport/client.rs`** — `InboundEvent::MessageReceipt`/`MessageControl` variants, the read loop RE-EMITS both instead of dropping them (`broker/client.ts:402-419`), `cancel_message()` correlated through the SAME `pending_sends` table keyed by the CANCELLED id under its own `CANCEL_TIMEOUT` with pi's `Cancel timeout` text (`:666-699`), `send_message_receipt()` fire-and-forget under pi's disconnecting/registered/socket guards (`:701-712`), and `SendOptions` gained `supersedes`/`retry_of` threaded onto the envelope. **(2) `session_state.rs`** — `SeenInboundMessages` (`index.ts:527`) with pi's NUL-joined `${from.id}\0${message.id}` key, the whole-map retention sweep, and a `VecDeque` carrying insertion order so the 1000-entry cap evicts the OLDEST (a bare `HashMap` would evict at random); `emit_message_receipt` with pi's truthiness `detail` filter and pi's swallow-everything failure mode; `latest_outbound_receipts` + `latest_delivery_state`; `handle_message_control` with the UNCONDITIONAL `dismissPendingAsk` ahead of the action branch. **(3) `inbound.rs`** — ONE `receiver_received_at` timestamp reused as both the dedupe clock and the envelope stamp (a second `now_ms()` would be a divergence), the duplicate-suppression early return, and `receiver_received`/`acknowledged`(×3)/`injected` receipts at pi's five emission points. **(4) `tools/intercom.rs`** — the `cancel` action, **which is what gives the broker's already-ported `handle_cancel_message` a caller at all (it was dead code)**, plus `messageId`/`supersedes`/`retryOf` params with pi's verbatim descriptions. **(5)** the ask-timeout message is now pi's full text including `for message {id}`, `Last known delivery state: {state}` (the ONLY reader of the receipt map) and the "this waiter timeout is not cancellation" sentence — cyrup's short form told a supervisor nothing it could act on. vs pi-intercom v0.10.1 `index.ts:32-33`, `:527-528`, `:532-548`, `:550-559`, `:562-569`, `:570-576`, `:594`, `:880-881`, `:902-934`, `:1018-1027`, `:1810-1830`, `:1943-1969`, `:2144-2145`; `broker/client.ts:402-419`, `:666-699`, `:701-712`. Tests: 4 in `session_state.rs` (incl. `inbound_dedupe_cap_evicts_the_oldest_insertion_not_an_arbitrary_one`) + `the_schema_advertises_cancel_and_the_three_message_id_parameters`. |
| ~~ICOM-018~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | M | `list-cwd` and cwd normalization absent — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-021~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Registration `status` is the raw config suffix, not the lifecycle status — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-025~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED** | test-defect | S | Two tests assert fixed wall-clock sleeps instead of polled conditions — **REFUTED (residual does not exist), CLOSED 2026-08-15**: sweep 9 — re-derived at HEAD rather than carried: `connect.rs:750` is `#[tokio::test(start_paused = true)]` and `grep -rn 'sleep(' crates/cyrup-intercom/src` returns **eight** hits, every one of them in PRODUCTION code (`connect.rs:359` the backoff itself, `broker/mod.rs:1640` the 5 s shutdown delay, `:1735` the registration deadline, `transport/{stream,client,spawn}.rs`) and **none** in a test. The `inbound.rs` half went with the queue ICOM-035 deleted. The row survived only as a standing rule, which belongs in the crate's test conventions, not in an open-items table.
| ~~ICOM-026~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED** | test-defect | S | Three tests pin ICOM-013's trailing period — **REFUTED, CLOSED 2026-08-14**: sweep 6 — the four assertions the item names carry NO trailing period at HEAD and match production: `cyrup-it/tests/intercom/tool_actions.rs:319` `"Message sent to target-session"`, `:372` `"Message sent to reviewer"`, `:502` `"Reply sent to reviewer"`, `intercom_command_transcript.rs:144` `"Message sent to reviewer"`. **The item's STRUCTURAL finding is NOT closed with it — it is re-filed as `ICOM-053`, because it is a statement about the GATE, not about these tests.** |
| ~~ICOM-053~~ | ~~medium~~ **CLOSED 2026-09-04** | test-gap | M | The workspace gate builds and runs NONE of the broker-socket seam tests, because `cyrup-it` is `required-features = ["it"]` — **CLOSED 2026-09-04 at `1d2b4418`**: of the row's three options (schedule / CI / required pre-merge step) the only one this repo can hold is the third — there is no CI and no scheduler (README "Build": *"There is no CI in this repository"*) — and the repo's pre-merge gate IS the README Build block, whose `cargo run -p xtask -- feature-matrix` line was the one step that reached `cyrup-it` at all, as a `check`. That row is now `cargo nextest run -p cyrup-it --features it,wasm-host` (`xtask/src/features.rs` `MATRIX`, the `cyrup-it` `Combo` — exactly `docs/TEST-ARCHITECTURE.md` §2.4's "everything" command; `verb` doc, module doc, `xtask/src/main.rs` summary and README L161 updated with it). Pinned by `xtask/src/features.rs` `tests::the_matrix_runs_the_gated_seam_suite_rather_than_type_checking_it` (exactly one row selects `cyrup-it`; verb `nextest run`; arms `it,wasm-host`; `slow`) — FAILED on the old row at `assert_eq!(row.verb, "nextest")` before the change, passes after. Upstream parity: pi-intercom v0.13.0 `package.json:21` runs `intercom.integration.test.ts` (the real-socket broker suite, 4 351 lines) in the single `npm test` script beside every unit test, with no CI workflow at the tag; identical from the ported baseline v0.9.2 (`package.json:20`) through v0.10.1, v0.12.0, v0.12.1 — one gate, socket suite included. **The item's thesis was demonstrated on the way to closing it:** the first full run found two `cyrup-it` targets that no longer COMPILED at HEAD — `permission` (`tests/permission/forwarding_spawn_env.rs:149`: the `AgentConfig` literal lacked the four fields SUBA-082 `5a4ae4ed` and SUBA-092 `247ff97b` added; those commits updated the five sibling `base_agent_config` sites in the `subagents`/`intercom` targets and missed this one) and `subagents` (`tests/subagents/subagent_tool_renderer_integration.rs:417`: serialised `ToolResult.terminate` after `4b7097be` retyped it to `cyrup_core::TerminateHint`, deliberately not `Serialize`) — both fixed in the same commit; the renderer test now mirrors `cyrup-agent/src/agent/message.rs::result_value_of` (key emitted iff `TerminateHint::wire()` is `Some`). Run record at `1d2b4418`: after every fix, at the tree of `1d2b4418` (a concurrent track's uncommitted `cyrup-ext-subagents` work was also present in the shared tree), target by target: `intercom` 79/79, `permission` 48/48, `subagents` 195/195, `ext` 41/41 (1 slow), `mcp` 33/33, `session_svc` 29/29, `bin` 63/63, `verify_redaction` 1/1, `misc` 0 tests (see residual) — 489 tests, 0 failed, 0 leaked, 0 skipped; before the fixes the first run had `permission` and `subagents` failing to compile and `perm034` losing its race once. A third rot surfaced as a flake and is fixed at its source: `tests/permission/human_dialog.rs` `perm034_a_reload_wipes_always_grants_exactly_as_upstream_does` failed once under load with `Session(StreamingNeedsBehavior)` on its second `harness.run` (3/3 green re-run alone) because `cyrup-test-support` `Harness::run` returned when the run stream ended — which on an unbound session happens inside the agent's `agent_end` emit (`cyrup-session-svc/src/subscriber.rs::end_run`), before `SettlementGuard::drop` (`cyrup-agent/src/agent/lifecycle.rs`) releases the `running` latch that `is_run_active()` reads; pi's multi-turn tests always follow `prompt` with `await session.waitForIdle()` (`core/agent-session.ts:1626` @v0.84.4), so `Harness::run` now awaits `session.wait_for_idle()` after draining, pinned by `cyrup-test-support/src/lib.rs` `smoke::run_returns_only_once_the_session_is_idle` (failed 3/3 before at turn 2-3, passes 3/3 after). The clippy half of the same gate (`cargo clippy -p cyrup-it --features it,wasm-host --all-targets -- -D warnings`, README "Build") had rotted too: two deny-level lints in the `mcp` target — `tests/mcp/http_oauth.rs:1952` `manual_split_once`, `tests/mcp/protocol_13i.rs:222` `collapsible_if` — fixed mechanically in the same commit; the surface is now clean except one lint in the DEPENDENCY `cyrup-tui` (`app/input_reader.rs:443` `redundant_closure`, committed in `3e69ea2a`, already recorded by area 07 `TUI-089`). **Residuals (low):** `tests/misc/main.rs` is an EMPTY `[[test]]` target (`mod support;` only; nextest reports 0 tests for `--test misc`) whose header still describes ten files — stale, and removing a target is a `docs/TEST-ARCHITECTURE.md` §9.1 decision, so it is recorded, not acted on; `--fast` still skips the row — by design, documented at README L161 and pinned as `slow: true`; and nothing runs the gate for anyone, because there is no CI or schedule — the README states it and it stays the process decision this row always said was not an agent's to make. |
| ~~ICOM-054~~ | ~~medium~~ **CLOSED 2026-09-05** | not-ported | L | **Broker-owned endpoint epochs are absent** — upstream `636f61e` (#109, v0.11.0) adds "broker-owned endpoint epochs, exact-send retry handling, and bounded delivery records **so stale endpoints are refused**", +422/−107 over `broker/broker.ts`, `broker/client.ts`, `broker/protocol.ts`, `index.ts`, `types.ts`. **Correctness, not cosmetics:** without it a send can be accepted against an endpoint the broker has already replaced. — **CLOSED 2026-09-05 at `87421cd7`**, ported in full at **v0.13.0** per ADR-0006. Tag-to-tag note: `636f61e`'s exact-target block has ONE combined missing-or-rebound arm; at v0.13.0 `089b631` (ICOM-055, closed at `c440c038`) had already split it into `E_TARGET_NOT_FOUND` + `E_TARGET_REBOUND` and made the lookup `sessions.get(scopedSessionKey(fromSession.scopeId, targetId))` — **the v0.13.0 shape is what landed**, on top of the `ScopeId`/`SessionKey` types that item introduced. **(1) The broker owns the epoch.** `SessionInfo::endpoint_epoch` (`v0.13.0 types.ts:14-16`, `[NON-NULL]` per `broker/protocol.ts:142-144`) is stamped `Some(Uuid::new_v4())` in `broker/session.rs` on EVERY register, a stable-id takeover included (`v0.13.0 broker/broker.ts:466`) — the id names the identity, the epoch names this socket binding of it. Never read from the registration (`SessionRegistration` omits it upstream, `types.ts:102`). `registered` now advertises `["extension-bus-v1", "exact-send-v1"]` (`:498-502`). **(2) A superseded endpoint is refused.** `ClientMessage::Send.target` carries the flattened `targetId`/`targetEpoch` pair (`types.ts:111`); `broker/send.rs`'s exact-target block runs BEFORE name resolution (`:624-654`) because a valid exact target REPLACES `to` rather than filtering it. Half-set / non-string / empty is `delivery_failed` + `E_INVALID_TARGET` — not a connection kill, and never a silent degrade to name routing. Every other refusal in both send handlers gains its upstream `code` with the reason string byte-identical (`E_INVALID_MESSAGE`, `E_SENDER_NOT_FOUND`, `E_REPLY_TARGET`, `E_SUPERSEDE_TARGET`, `E_MUTUAL_ASK`, `E_AMBIGUOUS_TARGET`, `E_TARGET_DISCONNECTED`, `E_TARGET_NOT_FOUND`); `cancel_message`'s two acks and its "cannot be cancelled" refusal stay **BARE**, exactly as `636f61e` left them (`:829,835-839,856`), which is why all four ack detail fields are optional on the wire. **(3) Bounded delivery records.** New `broker/delivery.rs` (`:44-46,65-74,1043-1110`): one record per `(sender SessionKey, message id)`, 1 h TTL, 4096-entry FIFO cap, with `delivery_record_order` supplying the JS `Map` insertion order a `HashMap` lacks. `replay_or_reject` runs on **all three** send arms, so any resend of an id replays its recorded ack without re-injecting the message and any resend with different authored content is `E_MESSAGE_ID_REUSE`; the one hole is the retryable `E_TARGET_REBOUND` record, which is what lets the client's single retry through under the same id. Six lifecycle events update records in place (`:709,823,856,1005,1021,1162`), including the `queued` → `socket_delivered` flip on a mailbox flush. **Client** (`transport/client.rs`): `Registered.features` is stored instead of discarded, `supports_feature` ports `supportsFeature` (`broker/client.ts:817-819`), and `send` becomes gate → resolve → `send_once` → **at most one** re-resolve on `E_TARGET_REBOUND`, under the same message id (`:645-690`). A reply is never exact-sent and a broker that never advertised `exact-send-v1` gets the v0.9.2 frame byte-for-byte; `resolve_exact_target` reuses `broker::routing::find_session_ids` rather than restating `findSessions`, and a peer with no `endpointEpoch` degrades to a plain send. `SendResult` grows `delivery`/`code`/`retryable`/`outcome_known` with upstream's absent-field defaults; the two disconnect drains are the crate's only producers of `DeliveryState::Unknown`. **Surfaces:** `tools::delivery_details` ports `deliveryDetails` (`v0.13.0 index.ts:116-126`) into `intercom{send}`/`{reply}`'s existing details maps, and the ask-timeout message reports the send's real delivery state (`index.ts:2464`) instead of hard-coding `socket_delivered`, so an offline peer now reads `queued`. **Deleted:** `BrokerState::clear_ask_edges_for_session` and its only call site — `636f61e` removed that call from the register-time takeover, leaving the method dead upstream at `broker/broker.ts:1176`; once the epoch refuses stale sends, wiping a replaced endpoint's ask edges is unnecessary AND harmful (it refuses a reply arriving after a stable-id replacement). Mailbox recovery is untouched. **Renamed, no behaviour change:** `ConnectSupervisor::epoch` → `reconnect_generation`, a purely local reconnect counter that shared nothing but a name — `grep epoch` now means what it says. **Design decisions (four, recorded in the commit body with the rejected alternatives):** (a) `DeliveredState`/`FailedState` beside the wide `DeliveryState` encode pi's **asymmetric** ack acceptance sets (`broker/client.ts:375,392`), so a `delivered` frame typed as failed — or the reverse — is unrepresentable; rejected one wide enum on both variants (looser than pi one way, nonsense the other) and a `String`. (b) `ExactTarget` has private fields, two total constructors and one `as_pair` reader, so a cyrup client cannot EMIT a half-set pair and no caller can use one half — but the rule is deliberately **not** in `Deserialize`, because pi answers a half-set frame with `E_INVALID_TARGET` rather than a type-guard throw (contrast `ExtensionOwnerRef`, whose pairing rule IS in `Deserialize`); enforcing it there would make cyrup fatally stricter than pi on a frame pi merely refuses. (c) `RecordedOutcome` collapses the record's `state`/`reason`/`code`/`retryable` quartet into a sum type: a delivered record cannot carry a failure code and a failed record cannot lack one, which makes upstream's `record.reason ?? …` / `record.code ?? …` defaults (`:1074`) unreachable by construction rather than merely unused. (d) `ReplayVerdict` replaces `replayOrReject`'s bare `bool` at the one point where inverting it delivers a message twice; `#[must_use]`. **Not done on purpose:** no newtype for message ids or epochs (pi compares them as opaque strings; this item establishes no rule about their shape) and no typestate (a send's state is decided per frame by received data, not by a legal sequence). **Tests: 15 added, 313 → 328, red before / green after.** RED was established by neutering the implementation in place and restoring — the epoch/feature pair, the exact-target block, `replay_or_reject`, the flush record flip, the client's feature gate, the eviction order, the order-vector prune, the fingerprint field set and the record key each in turn: 10 of the 15 failed under those neuters; `a_live_identity_takeover_preserves_the_ask_edge` was made red by restoring the deleted `clear_ask_edges_for_session` call; the remaining four (the three `transport/protocol.rs` wire tests and the client's plain-frame test) are red against the pre-change tree by non-compilation, since `ExactTarget`, `DeliveredState`/`FailedState` and `ClientInner::features` did not exist. Measured at `87421cd7`: `cargo nextest run -p cyrup-intercom` **328/328**, `CYRUP_IT_BIN_DIR=… cargo nextest run -p cyrup-it --features it -E 'binary(intercom)'` **80/80** (includes the real-socket broker round-trip; two `cyrup-it` helpers updated for the new `SessionInfo` field), `cargo clippy -p cyrup-intercom --all-targets -- -D warnings` clean, `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-intercom --no-deps` clean, `cargo fmt -p cyrup-intercom -- --check` / `-p cyrup-it -- --check` clean. **Residuals (low, none filed as new ids):** upstream's `writePendingAskRecord` / `removePendingAskRecord` calls inside the ported send arms are still absent — those records are exactly what **ICOM-057** adds, and it should write them where the citations here mark them; and the tool layer's FAILURE arms still return a `ToolError` where upstream returns a details-bearing result, so `deliveryDetails` reaches only the two success sites — converting that channel would change every failure string and belongs to a gap of its own, not to `636f61e`. |
| ~~ICOM-055~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | M | **Scoped routing is absent** — upstream `089b631` (v0.12.0) adds opt-in broker-enforced `PI_INTERCOM_SCOPE_ID` routing scopes (issue #112). cyrup has no scope concept on the wire or in the broker. Without it every registered session is mutually addressable with no isolation boundary. — **CLOSED 2026-09-04 at `c440c038`**, ported in full at `pi-intercom` **v0.13.0** per ADR-0006. **Tag-to-tag evidence, CORRECTED 2026-09-05 (review pass).** The sentence this replaces — and the identical sentence in `c440c038`'s commit body, which cannot be amended without rewriting a landed commit and is therefore superseded here — said `git diff v0.12.0 v0.13.0` *touches `config.ts` only, −15 lines*. That does not reproduce: `git -C tmp/pi-intercom diff --stat v0.12.0 v0.13.0` is **13 files, +356 / −171** (`CHANGELOG.md`, `README.md`, `broker/spawn.ts`, `broker/spawn.test.ts`, `config.ts`, `config.test.ts`, `index.ts` at +124, `intercom.integration.test.ts` at +227, `package.json`, `package-lock.json`, `reply-tracker.ts`, `reply-tracker.test.ts`, `skills/pi-intercom/SKILL.md`). The CONCLUSION is unchanged and is re-derived the right way: the feature lives in four production files — `git -C tmp/pi-intercom grep -c scopeId v0.13.0` gives `broker/broker.ts` 71, `broker/client.ts` 2, `config.ts` 2, `types.ts` 1 (plus two test files) — and of those only `config.ts` appears in the tag-to-tag diff, where the whole change is the −15-line removal of ICOM-059's withdrawn `toolVisibility` and `getIntercomScopeId` (`config.ts:21-24`) is byte-identical at both tags. **No file the feature lives in changed between the tags**, so what landed is `089b631` unchanged at v0.13.0. **Env + wire:** `ENV_INTERCOM_SCOPE_ID = "CYRUP_INTERCOM_SCOPE_ID"` (`identity.rs`) and `config::intercom_scope_id{,_from}` port `getIntercomScopeId` (`v0.13.0 config.ts:6,21-24`) in the crate's established const-in-`identity`/resolver-in-`config` split; `ClientMessage::Register.scope_id` (`transport/protocol.rs`) carries it with `skip_serializing_if`, so an **unscoped register frame is byte-identical to the pre-scope one** (`...(scopeId ? { scopeId } : {})`, `v0.13.0 broker/client.ts:286-292`; `types.ts:107`), and `IntercomClient::connect_target` resolves the scope where `client.ts` does. Deliberately NOT an `IntercomConfig` key — upstream reads it from the environment only, because `config.json` is machine-global. **Broker enforcement** (`crates/cyrup-intercom/src/broker/`, never the tool layer): the scoped `findSessions` ladder in all three tiers (`routing.rs::find_session_keys`, `v0.13.0 broker/broker.ts:1247-1262`), the scope-relative `list` roster (`state.rs::session_infos_in_scope`, `:596-597`), scoped `broadcast` for `session_joined`/`session_left`/`presence_update` (`state.rs`, `:1312-1318` called at `:327,:504,:540,:957`), the scoped disconnected-session and mailbox ladders including both `targetScopeId` and `fromScopeId` (`mailbox.rs`, `:1120-1131`, `:1264-1279`, `:1299-1310`), scoped ask edges and receipt routes, and the **extension bus** — one owner elected per scope (`NamespaceKey`, `scopedExtensionKey`, `:152-154`, `:1346-1424`), publish and state-commit fan-out filtered (`:1504-1506`, `:1668-1670`), and `scoped_extension_state_namespace` (`:156-161`) whose UNSCOPED arm returns the **bare** namespace so every file already under `<intercomDir>/extension-state/` keeps its name. `handle_send`'s sender lookup is hoisted to the top as upstream hoisted its own (`:614-618`), which also deletes the duplicated `"Sender session not found"` block. **Amended 2026-09-05 (review pass), no behaviour change:** `handle_send_to_disconnected`'s reply-edge guard read `edge.to != *current_key || edge.from.id != target.id` — comparing the bare ID on the second conjunct where upstream compares the whole composite key (`replyEdge.from !== disconnectedTarget.key`, `v0.13.0 broker/broker.ts:747`). It was unreachable in practice (an edge whose `from` sits in another scope has its `to` there too, so the first conjunct rejects it first) but it was the one hand-written place that stepped around the `SessionKey` discipline this row advertises as *a scope-forgetting lookup is a compile error*. The resolved key now travels with the info out of the `find_disconnected_session_keys` match and the guard reads `edge.from != target_key`; `cargo nextest run -p cyrup-intercom` **328/328** after (313 at closure; the rest are ICOM-054's). **Design decision (two types, recorded in the commit body with the rejected alternatives):** `ScopeId` is a parse-don't-validate newtype whose only constructor trims and maps blank to unscoped — an untrimmed scope (which would split one boundary in two) and a blank scope (an isolation class no unscoped session could reach) become unrepresentable; it has no `Deserialize` derive, so serde cannot bypass the parser. `SessionKey { scope, id }` (plus `NamespaceKey`) replaces upstream's stringified composite as the key of `sessions`, `disconnected_sessions`, `AskEdge`, `MessageReceiptRoute` and `MailboxMessage`, so a scope-forgetting lookup is a **compile** error rather than upstream's silent miss, and the derived `Eq` IS `sameScope` — `None` is a scope, never a wildcard. Rejected: a parallel `scope_id` field beside a `String` key (leaves `sessions.get(&id)` compiling); upstream's stringified key (opaque, re-parses on read); a derived `Deserialize` on `ScopeId` (bypasses the parser); `Option<String>` inside `SessionKey` (scatters the trim/blank rule); typestate (wrong shape — a scope is selected by an external event, not a legal sequence). **Unchanged by design:** `tools/intercom/list.rs` and `project_target.rs` (they render a roster the broker already filtered — a client-side filter is the anti-pattern the feature exists to avoid), `IntercomConfig`, the global `MAX_SESSIONS` cap, and `BrokerMessage::DeliveryFailed`'s v0.9.2 shape (upstream's `E_SENDER_NOT_FOUND` code belongs to a different gap). **One behaviour change for unscoped sessions, upstream's own** (`:592-595`): a `list` arriving on a SUPERSEDED socket is now a protocol error instead of a full-roster reply. A `list` before registration was already a protocol error via `dispatch`, so that is not a change. **Tests: 17 added, red before / green after.** `broker/scoped.rs` (new, 12) drives the whole definition of done — roster isolation with unscoped proved not to be a wildcard; a cross-scope `send` by full id, name and id prefix answered exactly like a never-registered name, with a same-scope positive control; join/presence/leave never crossing; parked mail never flushing into another scope; the same id coexisting in two scopes; a non-string `scopeId` fatal and a blank one unscoped; the superseded-socket `list`; per-scope owner election and publish isolation; the bare-when-unscoped state namespace; and the unscoped register frame asserted byte-for-byte. `routing.rs` +2 pins every resolution tier; `config.rs` +1 and `transport/protocol.rs` +2 pin the trim/blank/non-string trio on the env and wire paths. RED was established by neutering the enforcement in place (`SessionKey::in_scope` to `true`, the register-frame guard to silently-unscoped, the `list` requester check removed, the mailbox target-scope skip off): 7/12 in `scoped.rs` and 2/2 in `routing.rs` failed, then passed on restore; the other five are red against the pre-change tree by non-compilation. Measured at `c440c038`: `cargo nextest run -p cyrup-intercom` **313/313** (was 296/296), `CYRUP_IT_BIN_DIR=… cargo nextest run -p cyrup-it --features it -E 'binary(intercom)'` **80/80**, `cargo clippy -p cyrup-intercom --all-targets -- -D warnings` clean, `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-intercom --no-deps` clean, `cargo fmt --all -- --check` clean. **Residual (low, folded into ICOM-057 rather than filed as a new id):** `scopedPendingAskRecordPath` (`v0.13.0 broker/broker.ts:163-169`) is not ported because this crate has **no on-disk pending-ask records to scope** (no `PENDING_ASKS_DIR`, 0 hits) — those records are exactly what ICOM-057 adds, and it should now write them scoped from birth, inheriting `ScopeId`/`SessionKey` as things that already exist. |
| ~~ICOM-056~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | L | The extension outbox API and message provenance — **CLOSED 2026-09-04**: `crates/cyrup-intercom/src/outbox.rs` (671 lines) implements `IntercomOutboxRequestV1`/`IntercomOutboxResultV1` (`parse_outbox_request`, `resolve_outbox_target`, the generation-fenced delivery leg, build/emit/settle/fail), and `MessageProvenance`/`ProvenanceKind` are modelled on the envelope with `present_non_null` (`transport/protocol.rs:318-403`). The one edge-case the closing task file (`.flux/done/2026-08-28-02-56/ICOM-056_EXTENSION_OUTBOX_AND_PROVENANCE.md`) flagged as outstanding — `delivery_failed` emitting an empty `messageId` when a send tears down in flight — is already fixed in the code read: `outbox.rs:592-607` falls back to the request id (`rid`) when `result.id.is_empty()`, with an in-source comment explaining why an empty id would be meaningless to an extension correlating results. |
| ICOM-057 | low | not-ported | S | **Pending blocking-ask records are not persisted** — upstream `d69854d` (v0.11.0, issue #104) persists local pending-ask observability records for delivered blocking asks. cyrup has no equivalent (0 non-comment hits for a persisted ask record). Observability only; no delivery-path effect. **Re-confirmed 2026-09-04: `.flux/todo/ICOM-057_PERSIST_PENDING_ASKS.md` is still unstarted (todo, not done) and `grep` for a persisted-record shape in `crates/cyrup-intercom/src/` is still empty.** |
| ~~ICOM-058~~ | ~~low~~ **CLOSED 2026-09-04** | not-ported | S | `tmuxPane` neither modelled nor surfaced — **CLOSED 2026-09-04**: `identity.rs:89-116` (`ENV_TMUX_PANE = "TMUX_PANE"`, `current_tmux_pane_from`/`current_tmux_pane`, trimmed and empty-filtered exactly as upstream's `process.env.TMUX_PANE?.trim()`); `connect.rs:601` sets `tmux_pane` in the live registration; `broker/session.rs:105` copies `registration.tmux_pane` onto the stored `SessionInfo`; `tools/intercom/mod.rs:358-361` surfaces it in the `list` roster row. Landed via `.flux/done/2026-08-28-02-56/ICOM-058_TMUX_PANE_ROSTER.md`, corroborated by the code read above. |
| ~~ICOM-059~~ | ~~low~~ **CLOSED 2026-09-04 — REFUTED, upstream withdrew the feature** | not-ported | S | `toolVisibility` config key absent — **CLOSED 2026-09-04, not by porting.** Upstream added `IntercomToolVisibility`/`"after-first-use"` at v0.12.0 (`12f4b6c`) and **removed it two commits later** at v0.12.1, `ee0d74c` ("fix: remove lazy intercom tool visibility (#120)"): `config.ts`, `index.ts` and the tests all drop every `toolVisibility` reference, and CHANGELOG 0.12.1 states why — "The generic `intercom` tool now stays in the active tool set, which avoids a late-session prompt-cache reset when intercom first becomes useful." `git -C tmp/pi-intercom show v0.13.0:config.ts \| grep toolVisibility` returns nothing (confirmed this pass). cyrup never having ported the withdrawn feature is now the CORRECT state, not a gap — there is no upstream behaviour left to match. No `.flux` task file was ever started for this item, consistent with it having always been dead by the time anyone got to it. |
| ~~ICOM-060~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | M | No guard against a misdirected reply during an ask-triggered turn — upstream `5fe0ee3` (v0.12.1, #119, "fix: guard active intercom replies") — **CLOSED 2026-09-04 at `a91e3c41`**: `ReplyTracker::find_active_reply_target_mismatch(&mut self, to, now)` (`crates/cyrup-intercom/src/reply_tracker.rs`, beside `find_unique_pending_ask_from`) ports `findActiveReplyTargetMismatch` (v0.13.0 `reply-tracker.ts:124-130`, unchanged since `5fe0ee3`): prune, then `None` unless the current turn context is an ask whose `from.id` differs from the RESOLVED target — id only, no name or prefix arm. `tools/intercom/send.rs::action_send` consults it between the self-target guard and the ICOM-037 inferred-reply lookup, only when `replyTo` is absent, keyed on the resolved id exactly like that lookup (v0.13.0 `index.ts:2320-2328`), and refuses with the upstream string verbatim (`This turn is responding to an intercom ask from "<name || id>". Use intercom({ action: "reply", message: "..." }) or set replyTo: "<ask id>". Refusing non-reply send to "<targetDisplay>" to avoid a misdirected reply.`) as a `ToolError`. Pinned by `reply_tracker.rs` `tests::active_ask_context_flags_non_reply_sends_to_a_different_target` / `…does_not_trust_sender_names_as_destination_identity` (the two upstream `reply-tracker.test.ts` cases) and `…is_silent_without_a_live_ask_turn` (did not compile before — method absent), and by the seam test `crates/cyrup-it/tests/intercom/tool_actions.rs` `send_refuses_a_different_target_during_an_active_inbound_ask_turn` (the port of `intercom.integration.test.ts` "intercom send refuses a different target during an active inbound ask turn", real broker: planner asks worker through the broker, worker's turn adopts it, `send` to `orchestrator` is refused naming `planner` and `cwd-hierarchy-ask`, the orchestrator receives nothing, the ask stays pending; then the same turn's `send` to `planner-id` is the inferred reply and lands with `replyTo`). Run with `send.rs` at the pre-fix HEAD it FAILED with `Ok(Message sent to orchestrator)` — the misdirected delivery itself; passes at `a91e3c41`; `intercom` seam target 80/80, `cargo nextest run -p cyrup-intercom` 296/296. **Residual (low):** upstream's refusal also carries `details: { error: true, replyTo }`; cyrup's `ToolError` is message-only (the crate's standing convention for every upstream `details.error` result), so the ask id reaches the model in the text, not as a structured field. |
| ICOM-061 | low | not-ported | S | **New 2026-09-04.** No `/alias` command — upstream `90e6ad4` (v0.13.0, #126, "feat: add intercom alias command") adds `pi.registerCommand("alias", …)` (`index.ts:2812-2815`): `/alias <name>` (or bare `/alias` / `/alias menu`, which opens an interactive input when a UI is present) calls `pi.setSessionName(alias)` then immediately `syncPresenceIdentity(sessionId)`, so the new name reaches intercom peers at once rather than waiting for the idle name poll (ICOM-006) or the next `turn_start`/tool-call sync (ICOM-031). `crates/cyrup-intercom/src/extension.rs:472-479` (`init`) registers exactly two commands — `intercom-id` and the overlay-open command — `grep -rn '"alias"' crates/cyrup-intercom/src/extension.rs` finds neither a registration nor a handler. **Impact bounded and not fully characterised**: whatever mechanism cyrup already exposes for renaming a session outright (if any — not checked here) is unaffected; the gap is specifically the intercom-side command that renames AND immediately pushes the new identity to connected peers in one step. **Confidence: medium** — the upstream contract was read from the commit diff, not the full `index.ts` command-registration context. **Fix** — register an `alias` command in `IntercomExtension::init` that resolves `HostServices::session_name()`/`set_session_name` (if such a seam exists; if not, this item's Fix needs a cross-area handoff) and then calls the existing `sync_presence_identity` helper ICOM-031 introduced. **Verify** — dispatch `alias` with a bare name in a non-UI context; assert the session's advertised presence name changes without waiting for a poll tick. |
| ~~ICOM-027~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Non-trigger inbound messages persisted with `display=false` — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-028~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED** | cyrup-original | M | `intercom_message` entry surface has no renderer — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD as **the item's own option (a)**: `IntercomExtension::render_entry` draws the durable `intercom_message` entry (`extension.rs:690-707`, `:832`), with the in-source note recording that option (b) still depends on `ICOM-024`/`ICOM-029`. |
| ~~ICOM-032~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Session shutdown leaves the pending-idle queue populated — **CLOSED 2026-08-14**: sweep 1 — closed as SUBSUMED by ICOM-035, not as independently fixed: the machinery they described (`pending_idle`, `flush_timer`, `schedule_inbound_flush`, `flush_idle_messages`, `PendingInbound`, `InboundPolicy::Queue`) no longer exists. |
| ~~ICOM-034~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Subagent relay has no self-target guard — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-041~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | `runtimeFallbackAlias` neither modelled nor applied — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-043~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | v0.10.0 copy revision unported (emoji still present) — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-045~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | Blocking `ask` not refused up front when the target is offline — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-047~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | Broker startup failures discard the broker's stderr — **CLOSED 2026-08-14**: sweep 1. |
| ~~ICOM-049~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | M | Inbound delivery carries no runtime-generation guard — **CLOSED 2026-08-14**: sweep 2 — pi's `getLiveContext` fence ported into the inbound delivery path: `ConnectSupervisor.runtime_session_id` (pi's `currentSessionId`) captured by `begin_runtime` and cleared by `shutdown`, a new `runtime_ever_started` latch, `generation()`/`runtime_ever_started()` accessors and `is_live_at(state, generation)`; the inbound loop head stamps `message_generation` and re-checks liveness BEFORE the waiter match / record / surface, again at the head of the delivery decision, and a third time in the Trigger arm; the busy auto-reply's `dismissIncomingAsk` is gated on liveness AFTER its `await`. The FLUSH half is moot — ICOM-035 deleted the queue, exactly as the item's last line predicted. **A NEW LATCH WAS REQUIRED and the reason is on the record: pi's `runtimeStarted` (index.ts:522, set at :1253) is never cleared, whereas cyrup's `started` is cleared by `shutdown` because the reconnect ladder needs "is a runtime active right now". Reusing `started` makes `runtimeStarted &&` false after shutdown, which SKIPS the fence and lets a stale delivery through — the inverse of the guard. The pre-existing doc comment on `started` asserting it WAS pi's `runtimeStarted` is what made the wrong mapping look right, and is corrected.** |
| ~~ICOM-050~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `intercom_received` audit entry drops `messageId` and `attachments` — **CLOSED 2026-08-14**: sweep 1. |

---

## ICOM-003 — `IntercomClarifyChannel::ask` bypasses `ensure_connected`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/seams.rs:250-253` — `IntercomClarifyChannel::ask` does `self.state.client().ok_or_else(|| "intercom not connected: cannot route the human answer to the child")`. `grep -n 'ensure_connected' src/seams.rs` returns only `:144` and `:186` — the delivery and steer channels, both `ConnectReason::Background`.

**upstream** — `pi-intercom` v0.7.0 `index.ts:1000` (`ensureConnected("background")`); upstream has exactly five `ensureConnected(` call sites (`:805`, `:959`, `:1000`, `:1231`, `:1477`, `:1827`, `:1864`) and **no** bare client read on any send path. The ClarifyChannel itself is cyrup-original (it bridges pi-subagents), so the counterpart is the contract, not a line.

**Impact** — Bounded: `handle_disconnect` has already armed the ladder, so intercom is not dead. But a clarify whose human answer becomes ready inside a backoff gap fails once with a misleading "not connected" instead of waiting out the reconnect, and the human's answer is lost from the child's perspective.

**Fix** — Route the acquisition at `seams.rs:250` through `connect::ensure_connected(&self.state, ConnectReason::Background)` like its siblings at `seams.rs:144` and `:186`. ~3 lines.

**Verify** — Kill the broker mid-clarify; assert `ask` succeeds after the ladder reconnects rather than returning "intercom not connected".

## ICOM-004 — `skills/pi-intercom/SKILL.md` is not ported

> **CLOSED 2026-08-15 (sweep 9).** Ported; see the open-items row. The Fix below is superseded on one point: the skill ships as `resources/skills/pi-intercom/SKILL.md` (upstream's own name, matching the ported `pi-subagents` skill), not `skills/cyrup-intercom/`.

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `find crates/cyrup-intercom -type f ! -name '*.rs'` returns only `./Cargo.toml` — no `SKILL.md`, no `skills/` directory. `crates/cyrup-intercom/src/extension.rs:457-497` (`init`) registers 2 tools, 2 commands and 8 event kinds; it never registers a skill and never subscribes `EventKind::ResourcesDiscover`.

**upstream** — `pi-intercom` v0.7.0 `skills/pi-intercom/SKILL.md` (513 lines), declared at `package.json:26-28` (`"pi": { "skills": ["./skills"] }`). Rewritten at v0.10.0 — 164 changed lines (`git diff v0.9.2..v0.10.1 -- skills/`).

**Impact** — The agent gets no skill describing how to use the intercom tool — no worked examples of list/send/ask/reply, no guidance on addressing peers. This is the primary documentation of a subsystem whose tool errors (ICOM-013, ICOM-036, ICOM-045) are already opaque.

**Fix** — Add `crates/cyrup-intercom/resources/skills/cyrup-intercom/SKILL.md` and register it the way `crates/cyrup-ext-subagents/resources/skills/pi-subagents/SKILL.md` is registered, subscribing `ResourcesDiscover` in `init` (`extension.rs:483-495`). Port the **v0.10.0** text, not v0.9.2 — the rewrite is the copy ICOM-043's deslop aligns with.

**Verify** — Assert the skill is discoverable from the extension's registration and that its documented actions match the tool's action enum (`tools/intercom.rs:388`).

## ICOM-005 — `register` inside the pending window leaves an idle broker alive forever

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/broker/mod.rs:1005-1027` — `schedule_shutdown_check` early-returns on `g.shutdown_scheduled` and clears the flag **only inside the spawned task** (:1020). `handle_register` bumps `self.shutdown_gen` at `:693` and never clears `shutdown_scheduled` nor aborts the pending task, though the comment at `:692` claims "A register cancels any pending auto-shutdown".

**upstream** — `pi-intercom` v0.7.0 `broker/broker.ts:377-381` does `if (this.shutdownTimer) { clearTimeout(this.shutdownTimer); this.shutdownTimer = null; }`. Upstream's `scheduleShutdownCheck` has the same `if (this.shutdownTimer) return;` guard (`:286-296`), so the guard is not the divergence — **nulling the handle is what re-arms a later check**.

**Impact** — Not a premature kill; the generation stamp prevents that. What is lost is the re-arm: t=0 last session leaves (scheduled, gen G) → t=1 register (gen G+1, still scheduled) → t=2 that session disconnects, `schedule_shutdown_check` early-returns → t=5 the pending task sees `G+1 != G`, no shutdown, `scheduled = false`. The broker then idles indefinitely with zero sessions until an unrelated connect/disconnect cycle re-arms it. An idle-broker leak.

**Fix** — Have `handle_register` (`broker/mod.rs:693`) set `shutdown_scheduled = false` and abort the stored `JoinHandle`, the way `set_flush_timer` / `ConnectSupervisor::set_timer` already do in this crate. Keep the generation bump as belt-and-braces.

**Verify** — Register-then-disconnect inside one 5 s window with no other sessions; assert the broker process exits.

## ICOM-006 — No name poll; presence name is fixed at registration

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/extension.rs:199-213` — `sync_presence` calls `client.update_presence_with_context(None, Some(status), None, …)`; the `name` argument is hard-coded `None`. `grep -rn ENV_INTERCOM_NAME_POLL_MS crates/` returns exactly one hit — the declaration at `crates/cyrup-intercom/src/identity.rs:24`. No poll task exists anywhere.

**upstream** — `pi-intercom` v0.7.0 `index.ts:597-611` `startNamePoll` — `setInterval(getNamePollMs())` re-deriving `buildPresenceIdentity`, diffing against `lastPresenceName` and calling `syncPresenceIdentity` on a change. Same shape at v0.10.1 `index.ts:818-831`.

**Impact** — A session renamed mid-run (branch switch, `/name`, title change) keeps advertising its startup name to every peer's `intercom{list}` and `/intercom` picker, so operators address the wrong worker.

**Fix** — Land ICOM-031 first (it supplies `sync_presence_identity`, which recomputes the name from `connect.rs:437-457`'s helper), then add the poll task alongside `connect::begin_runtime` (`extension.rs`'s `SessionStart` arm) and cancel it in the `SessionShutdown` arm (`extension.rs:562-576`). The poll then becomes just a third caller.

**Verify** — Unit test: change the identity source, advance the poll interval, assert a `Presence` frame carrying the new name is sent exactly once, not once per tick.

## ICOM-008 — `ask` silently drops `replyTo`

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/tools/intercom.rs:159-171` — the `"ask"` arm calls `ask_and_wait(&client, &target, question_id, message, params.attachments.clone(), cancel)`; `params.reply_to` is never passed. `session_state.rs:249` hard-codes `reply_to: None` in the `SendOptions` while setting `expects_reply: Some(true)` and `message_id`. The parameter is declared at `tools/intercom.rs:34` and schema-advertised as `replyTo` at `:387`. The audit entry at `:183` even records the `replyTo` that was never sent.

**upstream** — `pi-intercom` v0.7.0 `index.ts:1626-1632` — `connectedClient.send(sendTo, { messageId: questionId, text: message, attachments, replyTo, expectsReply: true })`.

**Impact** — Asking a clarifying question back at a peer's pending ask is rejected by cyrup's own broker: `handle_send` returns `"Reply target does not match a pending ask"` when `message.reply_to.is_some() && reply_edge.is_none()`. The tool advertises a parameter it discards, so the failure reads as a broker bug.

**Fix** — Thread `params.reply_to` through `ask_and_wait`'s signature into `SendOptions.reply_to` (`session_state.rs:249`).

**Verify** — Integration test against the real broker: peer A asks, B counter-asks with `replyTo` set to A's message id; assert delivery instead of `Reply target does not match a pending ask`.

## ICOM-009 — Lifecycle status has no active-tool map and no `model_select` hook

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high · *partially closed*

**cyrup** — `crates/cyrup-intercom/src/extension.rs:590-597` maps `ToolExecStart{name} → "tool:{name}"` and `ToolExecEnd → "thinking"` with no per-`ToolCallId` map; `SharedIntercomState` (`session_state.rs:20-61`) has no `active_tools`. `EventKind::ModelSelect` exists (`crates/cyrup-ext/src/event.rs:39`, `= 22`; `HostEvent::ModelSelect` at `:329`) but is absent from the subscription list at `extension.rs:483-495`, and `sync_presence` passes `None` for model (`extension.rs:205-212`).

**upstream** — `pi-intercom` v0.7.0 `index.ts:1096-1108` maintains `activeTools.set/delete(event.toolCallId, …)` plus `activeTools.clear()` on `agent_start`/`agent_end`, and `currentStatus()` reads `activeTools.values().next().value`; `index.ts:1135-1147` is `pi.on("model_select")` → `updatePresence({…identity, model, status})`.

**Impact** — With two overlapping tool calls, the first `ToolExecEnd` resets presence to `thinking` while a tool is still running, so peers see the wrong activity. The advertised model is always empty in `intercom{list}`, so a supervisor cannot tell which worker is on which model.

**Fix** — Add `active_tools: Mutex<IndexMap<ToolCallId, String>>` to `SharedIntercomState`, derive status in a `current_status()` helper (also required by ICOM-021 — port them together), and add `EventKind::ModelSelect` to the subscription list at `extension.rs:483-495` with a `sync_presence` overload carrying the model. No new seam is needed.

**Verify** — Unit test: start tool A, start tool B, end A → status still `tool:…`; end B → `thinking`. Separately assert a `ModelSelect` event produces a `Presence` frame with `model` set.

**Partial closure** — The sibling context-usage half of `sync_presence` landed (`extension.rs:261-284`); that half is ICOM-019 and is closed.

## ICOM-010 — Broker mailbox for briefly-disconnected sessions is absent

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/broker/mod.rs:791-797` answers an unresolvable target with `DeliveryFailed { reason: "Session not found" }` and drops the message. `BrokerState` (`:190-260`) has no mailbox or disconnected-session map; `grep -n mailbox src/broker/mod.rs` returns only the doc note at `:590` ("pi searches its mailbox and then `messageReceiptRoutes`; cyrup has neither table").

**upstream** — `pi-intercom` v0.10.1 `broker/broker.ts:219` (`mailboxMessages`), `:775`/`:1002` (`queueMailboxMessage`), `:1020` (`flushMailboxForSession`, called from register at `:510`), `:1110` (`findDisconnectedSessions`), retention constants at `:40-41`, plus the `liveMailboxTarget` path via `findUniqueLiveSessionForDisconnectedSession` around `:640`.

**Impact** — Any message sent during a peer's reconnect gap is lost with a misleading "Session not found", now more likely because ICOM-003's ladder makes reconnect gaps a routine state rather than a terminal one.

**Fix** — Port the disconnected-session map and redelivery-on-register into `broker/mod.rs`. `connect.rs:44-51` documents "there is no mailbox, no queue, no redelivery" as an invariant the reconnect design relies on; a mailbox port must revisit that reasoning and the `fail_pending`-on-disconnect decision. **Sequence ICOM-045 first** so a mailbox never swallows a blocking ask, and ICOM-041 alongside it (the mailbox identity guard reads `runtimeFallbackAlias`).

**Verify** — Integration test: register A and B, kill B's socket, send from A, restart B with the same session id, assert delivery.

## ICOM-011 — Restart-stable session IDs absent; `stableId` silently ignored

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `grep -rni 'stable_id\|stableId' src/ tests/` returns **zero** hits. `crates/cyrup-intercom/src/config.rs:33-48` has 7 fields and `parse_config` (`:95-143`) reads only brokerCommand/brokerArgs/confirmSend/enabled/inboundTrigger/replyHint/status, silently ignoring an unknown `stableId`. The register id comes from `HostServices::session_id()` with a `last_session_id()` fallback (`connect.rs:377-382`) — stable across a reconnect, not across a process restart. No `CYRUP_INTERCOM_STABLE_ID` exists in the crate's env inventory.

**upstream** — `pi-intercom` v0.10.1 `config.ts:38-39` declares `stableId` with fail-closed validation at `:141-150` (non-string / empty rejected); `index.ts:38` `STABLE_INTERCOM_SESSION_ID_ENV = "PI_INTERCOM_STABLE_ID"`, resolved at `:435` `resolveConfiguredIntercomSessionId`, consumed at `:1264`.

**Impact** — A restarted worker gets a fresh id, so every peer's stored target breaks and long-lived supervisor scripts must re-list after each restart. The config key is accepted and ignored, which reads as a working feature.

**Fix** — Add `stable_id` to `config.rs`'s struct and `parse_config` with upstream's fail-closed validation, add `CYRUP_INTERCOM_STABLE_ID` beside the other env names in `identity.rs:11-36`, and prefer both in `connect.rs:377-382`. The wire already carries it (`ClientMessage::Register { session_id: Option<String> }`), and `/intercom-id` itself is already ported (`extension.rs:474-481`, `tests/intercom_id_command.rs`) — this is a config+env gap, not a protocol or command gap.

**Verify** — Set `stableId`, connect, drop the process, reconnect; assert the broker sees the same session id and prior peers' targets still resolve. Assert a non-string `stableId` is a hard error, not a silent default.

## ICOM-012 — `lib.rs` claims v0.6.0 when the code is at v0.9.2

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/lib.rs:1-2`: "cyrup-intercom — out-of-band supervisor coordination companion (a 1:1 source port of `pi-intercom` v0.6.0)."

**upstream** — Citation census over `crates/cyrup-intercom/src`: **v0.9.2 × 272**, v0.7.0 × 14, v0.8.0 × 3, v0.6.0 × 1 (the banner), v0.10.x × 0. Load-bearing v0.8.0/v0.9.x code is present and tested: `transport/protocol.rs` models the full 16-tag `BrokerMessage` union and the v0.9.2 envelope; `format_context.rs` ports `format-context.ts`; `broker/runtime_claim.rs` ports v0.8.0 `runtime-claim.ts`; `extension.rs:474` registers v0.8.0's `/intercom-id`.

**Impact** — Every agent that diffs from v0.6.0 "finds" a pile of already-done work; every agent that trusts this file's former "v0.7.0" header under-counts the drift window by two minor versions. It has cost at least four agents a correction.

**Fix** — Edit the banner at `lib.rs:2` to **v0.9.2**. Correct the same claim in `docs/gap-analysis/PARITY-GAPS.md:19` and in `docs/gap-analysis/README.md`'s baselines table. *(Note for the next pass: `v0.9.0` has zero citations in the crate — `format_context.rs` cites v0.9.2, not v0.9.0.)*

**Verify** — `grep -n 'v0\.[0-9]' crates/cyrup-intercom/src/lib.rs` reads v0.9.2, and no doc asserts a v0.7.0 baseline.

## ICOM-013 — User-visible message strings still diverge at four sites

**Kind** parity-bug · **Severity** low *(corrected down from medium)* · **Effort** M · **Confidence** high

**Corrected in this pass.** Five halves are now closed, one is downgraded rather than open, and the item was **under-scoped** on the ambiguity message — upstream has two distinct errors there, not one.

**cyrup** — Still open: `tools/intercom.rs:94` `"Cannot send an intercom message to yourself."` and `:164` `"Cannot ask yourself."`; the trailing period cyrup adds at `:157`, `:254` and `extension.rs:447`; `session_state.rs:186-190` `"Multiple sessions match \"{x}\". Use the session ID instead."`; `inbound.rs:39-40` `NON_INTERACTIVE_BUSY_NOTICE` = "This session is running in non-interactive mode and cannot respond to messages right now."

**upstream** — `pi-intercom` v0.7.0 `index.ts:1532` and `:1615` both use a single `"Cannot message the current session"`; `:1571` is `` `Message sent to ${to}` `` and `:1726` `` `Reply sent to ${target.from.name || target.from.id}` `` — **no period**; `:751` is the full notice "This agent is running in non-interactive mode and cannot respond to intercom messages while it is working. It will continue its current task and exit when done." And `resolveSessionTarget` raises **two** errors, not one: `:872` `` `Multiple sessions named "X" are connected. Address one by the id shown in parentheses by "list" (${ids}).` `` **and** `:883` `` `Multiple sessions match ID prefix "X". Use a longer session ID prefix.` `` — cyrup collapses both into one generic string naming no candidates.

**Impact** — Prompt-visible drift: agents and skill docs written against pi pattern-match on these strings. The ambiguity error is the material one — pi tells the caller which ids to choose between and which *kind* of ambiguity it hit; cyrup does neither, leaving no path forward.

**Fix** — Normalize each string at the cited lines; echo `name || id` consistently; port **both** upstream ambiguity errors into `session_state.rs:186-190`, printing candidate ids. Land with ICOM-039 so the printed candidates are distinguishing prefixes rather than colliding 8-char slices, and with ICOM-026 (three tests pin the period).

**Verify** — String-equality tests per site against the upstream text, including a two-name collision and a two-prefix collision producing the two different errors.

**Closed halves (verified at HEAD, do not re-report)** — `Message sent to {to}` now echoes the **caller-supplied** target (`tools/intercom.rs:157`, test at `:841-880`); `Reply sent to {name||id}` (`:254` via `display_name`, `:186-190`); the empty pending case is pi's `"No unresolved inbound asks."` (`:268`); `status` is pi's four-line `**Intercom Status:**` block (`:311-313`); the `reply` self-guard already uses pi's exact `"Cannot message the current session"` (`:218`).

**Downgraded, not open** — The `ToolError`-vs-text-result half is **not** a divergence: pi's `pi.on("tool_result")` handler (`index.ts:1163-1166`) maps `details.error === true || details.delivered === false` to `{ isError: true }`, which is exactly what cyrup's `ToolError` produces. Only the failure *text* differs — upstream `Message to "${to}" was not delivered: ${reason}` (`index.ts:1547`) vs cyrup's bare reason (`tools/intercom.rs:126-128`). Fix the text; leave the mechanism.

## ICOM-014 — Broker `presence` validation runs before the socket-ownership check

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/broker/mod.rs:906-926` runs `for key in ["name","status","model"]` and `for key in ["contextPct","contextTokens","contextWindow"]` type checks, each returning `FrameResult::protocol_error()`, **before** the ownership check at `:928` (`self.sessions.get_mut(&current_id).filter(|s| s.conn_id == conn_id)`, whose miss is a benign `FrameResult::cont()`).

**upstream** — `pi-intercom` v0.7.0 `broker/broker.ts:516-556`: every `throw new Error("Invalid presence …")` is nested **inside** `if (session?.socket === socket) { … }` (`:521`), so a non-owning socket's malformed presence is ignored, not fatal. Same nesting at v0.10.1 `broker/broker.ts:763-805`.

**Impact** — A superseded socket sending a late malformed presence frame gets its connection killed as a protocol error rather than ignored. ICOM-003's ladder makes this a live path: `connect.rs` deliberately re-offers the previous session id, so takeover races are real.

**Fix** — Move both type-check loops inside the ownership `let Some(session) = …` block at `broker/mod.rs:928`. Do the same for ICOM-041's new `runtimeFallbackAlias` check when it lands.

**Verify** — Connect two sockets claiming the same session id; from the losing one send a presence frame with `name: 5`; assert the connection survives.

## ICOM-015 — Broker listen half of the named-pipe / TCP transport absent

> **CLOSED 2026-08-15 (sweep 9).** The evidence below is stale in both halves: the named-pipe listen arm had already landed (`broker/listener.rs`), and the remaining TCP arm needed no Windows target. See the open-items row.

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high · *partially closed*

**cyrup** — The **client** half is now fully ported and live: `transport/target.rs:254-256` `broker_connect_target` is called from `transport/spawn.rs:226` and `:299`; the three-arm `BrokerConnectTarget` plus the named-pipe/TCP dialer live at `transport/stream.rs:64-90`; the `broker.port.json` validation ladder is at `target.rs:282-330`. The **broker** half is still absent: `broker/mod.rs:1243` is an unconditional `let listener = UnixListener::bind(&socket_path)?;`, `:24` imports only `tokio::net::UnixListener`, no `broker.port.json` is ever written, and `grep -rn broker_listen_target src/` returns hits **only inside `transport/target.rs` itself** (definition + tests at `:430`, `:442`) — zero production callers.

**upstream** — `pi-intercom` v0.7.0 `broker/broker.ts:21` `const LISTEN_TARGET = getBrokerListenTarget()` with the two-branch listen at `:176-179`; helper `broker/paths.ts:107-116`.

**Impact** — The broker binary does not build or run on Windows, so the ported client transport has nothing to connect to there; and there is no opt-in TCP-loopback fallback for environments where Unix sockets are unavailable (some container/WSL and network-filesystem setups). Effort drops from L to M because the resolver, the enum and the discovery-file format are already written and tested.

**Fix** — Call the already-ported `broker_listen_target` (`transport/target.rs:278`) from `broker/mod.rs:1243`, cfg-gate the `UnixListener` import at `:24`, add the named-pipe and TCP listener arms, and write `broker.port.json` on the TCP arm to match what `target.rs:282-330` already validates.

**Verify** — `broker_listen_target` has a production caller; a TCP-mode broker writes a `broker.port.json` that `broker_connect_target` accepts; a Windows-target `cargo check -p cyrup-intercom` compiles.

## ICOM-016 — Silent namespaced extension bus: protocol modelled, effects absent

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/broker/mod.rs:419-425` routes `extension_publish` / `extension_state_commit` / `extension_capabilities_update` to validation-only handlers (`:480-583`) that answer pi's not-advertised miss branch and never implement owner election, fan-out or the state store; the in-tree comment at `:413` concedes the bus is unimplemented. `BrokerMessage::Registered { session_id, features: None }` at `:695` never advertises the feature, even though `protocol.rs:89` defines `EXTENSION_BUS_FEATURE = "extension-bus-v1"` and `:767` models the optional `features` field.

**upstream** — `pi-intercom` v0.10.1 `types.ts:1` (`EXTENSION_BUS_FEATURE`), `extension-api.ts` (44 lines), `broker/extension-state.ts` (186 lines — sha256-checksummed, 64 KiB-capped, optimistic revisions), `broker/broker.ts:505` (`features: [EXTENSION_BUS_FEATURE]`), `:509` (owner election).

**Impact** — Extensions built on the upstream bus have no cyrup equivalent, and there is no way to carry coordination metadata between sessions without it appearing as a user-visible intercom message. The protocol half being present makes this look supported from the wire.

**Fix** — Implement the effects behind the already-modelled frames: owner election and fan-out in `broker/mod.rs:480-583`, a checksummed capped state store mirroring `broker/extension-state.ts`, and `features: Some(vec![EXTENSION_BUS_FEATURE])` on `Registered` at `:695`. Read `extension-state.ts` in full first — the fix sketch here is directional, not a design (see Coverage blind spot 5).

**Verify** — Two sessions exchange a namespaced bus message; assert neither session's conversation receives an injected message, and that a state commit at a stale revision is rejected.

## ICOM-017 — Delivery diagnostics (receipts, dedupe, cancel/supersede) absent

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/broker/mod.rs:595-607` `handle_cancel_message` **always** answers `DeliveryFailed { reason: "Message cannot be cancelled by this session" }` (no mailbox, no `messageReceiptRoutes`); `handle_message_receipt` (`:447-456`) validates and returns without forwarding; the inbound dispatch (`inbound.rs:355-388`) stamps no `receiverReceivedAt`/`injectedAt` and does no `(from.id, message.id)` dedupe; `tools/intercom.rs:388` lists six actions with no `cancel`, and `IntercomParams` (`:24-36`) has no `supersedes`/`retryOf`/`messageId`.

**upstream** — `pi-intercom` v0.10.1 `types.ts:49-58` (`MessageReceipt`), `index.ts:900-912` (`hasSeenInboundMessage` dedupe + `emitMessageReceipt`), `:1927` (the `cancel` case), `broker/broker.ts:822-866`.

**Impact** — A sender cannot tell whether a message was delivered, read or superseded, and cannot withdraw an ask that is no longer relevant, so a stale ask blocks the peer's `pending` list until it times out. A duplicate delivery (which the reconnect ladder makes reachable) is injected twice.

**Fix** — The **envelope is already modelled** — `protocol.rs:310`/`:317`/`:320`/`:324`/`:328`/`:332` carry `sender_sequence`/`broker_delivered_at`/`receiver_received_at`/`injected_at`/`supersedes`/`retry_of` — so this is a lifecycle port, not a wire port: stamp the timestamps in `inbound.rs:355-388`, add the `(from.id, message.id)` dedupe set, forward receipts in `broker/mod.rs:447-456`, implement `handle_cancel_message`, and add the `cancel` action plus `supersedes`/`retryOf`/`messageId` to `tools/intercom.rs`. Port ICOM-010 first or keep consistent with the at-most-once no-mailbox invariant at `connect.rs:44-51`.

**Verify** — Send an ask, cancel it, assert it disappears from the peer's `pending` and the sender receives a receipt; deliver the same `(from, id)` twice and assert one injection.

## ICOM-018 — `list-cwd` and cwd normalization absent

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — No `cwd.rs` exists in `crates/cyrup-intercom/src` (`ls src/` confirms). `tools/intercom.rs:388` action enum is `["list","send","ask","reply","pending","status"]` and the schema (`:382-410`) has no `cwd` parameter. `format_session_list_row` does the raw byte comparison `session.cwd == current_cwd` at `tools/intercom.rs:364`.

**upstream** — `pi-intercom` v0.10.1 `cwd.ts:13-27` (`normalizeCwd`) and `:29-31` (`sameCwd`); `index.ts:1895-1945` (the `list-cwd` case), `:1832-1835` (the `cwd` param).

**Impact** — `/w` and `/w/`, or a symlinked vs realpath'd cwd, read as different directories, so the "same project" marker in `list` output is wrong for any session started through a symlink. There is no way to list only the peers working in this repo, which is the common supervisor query.

**Fix** — Port `cwd.ts` as `crates/cyrup-intercom/src/cwd.rs`, use it at `tools/intercom.rs:364`, and add the `list-cwd` action plus its `cwd` parameter to the schema. **This now also blocks ICOM-042**, whose v0.10.0 cwd-scoped send/ask reuses the same helper — port `cwd.rs` once, for both.

**Verify** — Register two sessions whose cwds differ only by a trailing slash and a symlink; assert both are marked same-project and both appear under `list-cwd`.

## ICOM-021 — Registration `status` is the raw config suffix, not the lifecycle status

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/connect.rs:455` — `build_registration` sets `status: state.config.status.clone()`, the raw configured suffix, never the lifecycle-derived status.

**upstream** — `pi-intercom` v0.7.0 `index.ts:570-587` — `buildRegistration` sets `status: currentStatus()`, where `currentStatus()` (`:562-567`) is `tool:<name> | thinking | idle` optionally suffixed ` · ${config.status}`.

**Impact** — Because `build_registration` is rebuilt on **every** reconnect rung (`connect.rs:351`), a session that reconnects mid-run re-registers as having no lifecycle status at all. Bounded: the next `AgentStart`/`ToolExecStart` `sync_presence` self-heals it within a turn.

**Fix** — Have `build_registration` (`connect.rs:437-457`) call the `current_status()` helper introduced by ICOM-009 rather than reading `config.status` directly. Port the two together.

**Verify** — Force a reconnect mid-tool-call; assert the re-registration frame carries `tool:<name> · <config.status>`, not the bare suffix.

## ICOM-023 — `schedule_inbound_flush(state, 0)` races its own task and aborts the retry

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/inbound.rs:179-188` spawns **first** and installs the handle **after**: `let handle = tokio::spawn(async move { if delay_ms > 0 { sleep(..).await; } flush_idle_messages(&flush_state); }); state.set_flush_timer(Some(handle));`. With `delay_ms == 0` the task never awaits, so on a multi-thread runtime it can run `flush_idle_messages` → `release_flush_timer()` (`:197`) → `schedule_inbound_flush(state, INBOUND_IDLE_RETRY_MS)` (`:202`) before the caller reaches `:187`, whose `set_flush_timer` then `.abort()`s the retry (`session_state.rs:167-176`). Both `delay_ms == 0` call sites remain: `extension.rs:587` (`AgentEnd`) and `extension.rs:609` (`TurnEnd`).

**upstream** — `pi-intercom` v0.7.0 `index.ts:674-683` assigns `inboundFlushTimer = setTimeout(...)` synchronously before any callback can run (single-threaded; `setTimeout(…,0)` is a macrotask), so `flushIdleMessages`'s own retry scheduling is always the last writer.

**Impact** — A message parked by `InboundPolicy::Queue` can lose its retry and sit in `pending_idle` until an unrelated later event re-arms the flush. Mid-run `TurnEnd`s are self-correcting; the real loss window is the last event of a run — if the final `AgentEnd` is dispatched while the session still reads busy and its retry is aborted, no further events fire and the queued peer message is never delivered while the session sits idle. Exactly the failure ICOM-002's fix removed, reintroduced as a scheduling race.

**Fix** — Install the slot before the work can run: hold the flush-timer lock across spawn+store; or have the task await a `Notify` signalled right after `set_flush_timer`; or give `set_flush_timer` a monotonic schedule id and refuse to install a handle older than the one in the slot. **Note: fixing ICOM-035 subsumes this** — upstream deleted the whole machine at v0.9.3. Do not invest in a careful fix here if ICOM-035 is scheduled.

**Verify** — `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]`: bind an `IdleControlledHost` reporting busy, `queue_idle_message` once, call `schedule_inbound_flush(&s, 0)` in a 50-iteration loop to widen the window, then `set_idle(true)` and **poll** for delivery within ~1 s.

## ICOM-024 — No `intercom_message` renderer; card frozen at width 80

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/extension.rs:457-497` (`init`) calls `register_tool` ×2, `register_command` ×2 and `subscribe` — no `api.register_message_renderer(..)`. `grep -rn 'register_message_renderer' src/` returns only two doc comments (`ui/mod.rs:13`, `ui/inline_message.rs:14`). The card is pre-rendered **once** at a hard-coded `SURFACE_CARD_WIDTH: usize = 80` (`inbound.rs:30`) with `collapsed: true` frozen at `inbound.rs:445`, against `PlainTheme` (`inbound.rs:471`). The seam is live: `crates/cyrup-ext/src/native.rs:270` `InitApi::register_message_renderer`, trait hooks at `native.rs:409`/`:415`, dispatched by `crates/cyrup-tui/src/app/extension_render.rs:161-164` (`render_message_call_outcome` at `:161`).

**upstream** — `pi-intercom` v0.7.0 `index.ts:1149-1153` — `pi.registerMessageRenderer("intercom_message", (message, options, theme) => { const details = message.details as {...}; if (!details) return undefined; return new InlineMessageComponent(details.from, details.message, theme, details.replyCommand, details.bodyText, !options.expanded); })` — re-invoked per frame with the live theme and `options.expanded`.

**Impact** — A resized terminal shows an 80-column card in a 120- or 60-column pane with misaligned borders and truncated text. Collapse/expand is unreachable — `collapsed` is baked in at receive time, so a long message can never be expanded while the card still renders the literal hint "Ctrl+O expands", which does nothing. The card also ignores the active theme.

**Fix** — Add `api.register_message_renderer("intercom_message")` in `IntercomExtension::init` and implement `NativeExtension::render_call` for that key, rebuilding an `InlineMessage` at the live width from the payload's `from`/`message`/`replyCommand`/`bodyText`. **Blocked by ICOM-029** — `inject_message` carries no `details`, so the renderer would receive a bare markdown string and be forced into upstream's `return undefined` branch. Land ICOM-029 first, then this, then ICOM-043's copy revision. Correct the now half-stale rationale at `ui/mod.rs:12-19` ("cyrup's native `InitApi` has no `register_message_renderer` / `register_shortcut`") — the message-renderer half of that claim is false.

**Verify** — Unit-test `render_call("intercom_message", &payload)` returning `Some(..)` for the exact payload `surface_incoming_message` produces and `None` for one missing `from`/`message`; then assert `ExtensionHost::has_message_renderer("intercom_message")` is true after loading the extension.

## ICOM-025 — Two tests assert fixed wall-clock sleeps instead of polled conditions

> **CLOSED 2026-08-15 (sweep 9) — REFUTED at HEAD.** Neither test exists in the form described: `connect.rs` is on a paused clock and the `inbound.rs` half went with ICOM-035's deleted queue. No `sleep(` in this crate is in a test.

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/connect.rs:557-575` `a_failing_rung_waits_its_backoff_instead_of_busy_looping` is a bare `#[tokio::test]` doing `sleep(300ms)` → `assert_eq!(attempt(), 0)`, `sleep(1000ms)` → `assert_eq!(attempt(), 1)`, `sleep(700ms)` → `assert_eq!(attempt(), 1)` — 300 ms of slack over a 1000 ms timer plus a spawn hop plus a filesystem-failing `ensure_broker`, with no `start_paused`. `crates/cyrup-intercom/src/inbound.rs:722-740` does `sleep(INBOUND_FLUSH_DELAY_MS + 100)` then `sleep(INBOUND_IDLE_RETRY_MS + 200)` then `assert_eq!(injected.len(), 2)` across threads.

**upstream** — Not an upstream-behavior question: the behaviors under test are correct ports. Only the assertion technique is wrong. The correct technique already exists in this crate — `tests/reconnect.rs:85-96` `within(budget, predicate)`, used at `:147`, `:163`, `:287`.

**Impact** — Intermittent red on a clean tree. The standard reaction is to widen the sleep, which slows the suite and hides genuine regressions in the backoff/retry timing these two tests are the only guard for.

**Fix** — For `connect.rs:557-575` switch to `#[tokio::test(start_paused = true)]` + `tokio::time::advance(..)`. For `inbound.rs:722-740` keep the real clock (it drives `IdleControlledHost` across threads) but replace the **final** fixed sleep with a `within`-style poll on `host.injected().len() == 2`, keeping the mid-test negative assertion as a fixed sleep. Do **not** "fix" `tests/reconnect.rs:299` or `tests/shared_human_lock.rs:265` — both are negative assertions after a fixed wait, which is sound.

**Verify** — Run both under `--test-threads=1` alongside a CPU load generator and confirm they still pass; confirm the paused-clock connect test completes in milliseconds rather than ~2 s.

## ICOM-026 — Three tests pin ICOM-013's trailing period

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high · *partially closed*

**cyrup** — The trailing-period assertions are now pinned at **three** sites: `crates/cyrup-intercom/src/tools/intercom.rs:820` `assert_eq!(result_text(&result), "Message sent to target-session.");`, `:875` `assert_eq!(result_text(&result), "Message sent to reviewer.", …)`, and `:1016` `"Reply sent to reviewer."`.

**upstream** — `pi-intercom` v0.7.0 `index.ts:1571` is `` `Message sent to ${to}` `` and `:1726` `` `Reply sent to ${target.from.name || target.from.id}` `` — no period at either site.

**Impact** — Anyone fixing ICOM-013's period must edit all three, and their phrasing — bare `assert_eq!`s with no parity citation — reads as an intentional contract rather than an accident.

**Fix** — Land with ICOM-013's normalization: drop the periods from the expectations at `tools/intercom.rs:820`, `:875`, `:1016` once `:157`, `:254` and `extension.rs:447` stop emitting them.

**Verify** — All three assertions expect the period-free upstream strings and pass.

**Partial closure** — The id-vs-original-target half is closed: a second test registers a peer under **name** `reviewer` with **session id** `peer-session` and asserts the echoed target is `reviewer` (`tools/intercom.rs:845-880`), which is exactly the conversion the prior pass prescribed.

## ICOM-027 — Non-trigger inbound messages are persisted with `display=false` and vanish on replay

**Kind** parity-bug · **Severity** low *(corrected down from medium)* · **Effort** S · **Confidence** high

**Corrected in this pass.** The originally-filed headline — "every inbound intercom message is written to the session tree hidden" — is **false** for the default path and is not claimed here.

**cyrup** — `crates/cyrup-intercom/src/inbound.rs:250` passes `false` for `display` to `services.inject_message(&content, Some(INBOUND_MESSAGE_CUSTOM_TYPE), false, trigger)`, and the follow-up path repeats it at `:286`. `AgentSession::inject_message` (`crates/cyrup-session-svc/src/session.rs:3732-3767`) forwards the caller's flag to `append_custom_message` **only on the not-streaming, not-trigger branch** (`:3762`); the trigger branch takes `spawn_run` and the streaming branch takes `agent.steer`, and in both the Custom message is re-emitted as MessageStart/MessageEnd (`cyrup-agent/src/agent.rs:454-455`, `:491-492`) and persisted by `cyrup-session-svc/src/subscriber.rs:172-183` with `display` **hard-coded true**. So the defect is narrow: it bites `inboundTrigger: "replies"` / `"never"`, and every FollowUp after the first in a flushed backlog. The replay gate is `crates/cyrup-tui/src/app/session_bind.rs:266-269` (`AgentMessage::Custom(c) => { … if c.display { … } }`), a **correct** port of Pi's `if (message.display)` (`interactive-mode.ts:3470`) that must not be changed.

**upstream** — `pi-intercom` v0.7.0 `index.ts:662-668` — `pi.sendMessage({ customType: "intercom_message", content: …, display: true, details: entry }, …)`; unchanged at v0.10.1 `index.ts:892`. Upstream has exactly one surface and it is always displayed.

**Impact** — With a non-default `inboundTrigger`, or for the 2nd..Nth message of a flushed backlog, the inbound message is written to the session tree hidden: it shows live (the TUI's live path at `app/events_fold.rs:111-123` ignores the flag) and then disappears completely from `cyrup --resume` and any transcript replay — sender, body and reply hint alike. A supervisor who resumes such a session cannot see what a peer told it.

**Fix** — Pass `true` for `display` at `crates/cyrup-intercom/src/inbound.rs:250` and `:286`, matching upstream unconditionally. Delete the stale rationale at `inbound.rs:220-222` ("`display = false` because the durable card was ALREADY surfaced via `surface_incoming_message`") — that card has no renderer either (ICOM-028). The parity resolution is to make the single displayed custom message the surface, as upstream does.

**Verify** — Extend `idle_headless_session_delivers_instead_of_auto_replying` (`inbound.rs:753-800`) with a recording host that captures the `display` argument and assert `true`; then a `cyrup-session-svc` integration test with `inboundTrigger: "never"`: inject, reload with `SessionManager`, assert the `CustomMessage` entry replays rather than being skipped at `app/session_bind.rs:269`.

## ICOM-028 — `append_entry("intercom_message", …)` has no entry renderer

**Kind** cyrup-original · **Severity** low *(corrected down from medium)* · **Effort** M · **Confidence** high

**Corrected in this pass.** The impact was overstated: the injected custom **message** is persisted and drawn by the TUI's built-in `[type] body` framing on the default trigger path, so the human does see header, cwd, reply hint and body. The card is dead weight plus one noise status line — not "nothing at all".

**cyrup** — `crates/cyrup-intercom/src/inbound.rs:471` `services.append_entry("intercom_message", &payload)` writes a rich payload (`content`, the 80-column pre-rendered `card`, `from`, `message`, `replyCommand`, `bodyText`, `collapsed` — `inbound.rs:461-469`). `crates/cyrup-intercom/src/extension.rs:457-497` (`init`) never calls `InitApi::register_entry_renderer`, and `IntercomExtension` implements no `render_entry`, so `crates/cyrup-tui/src/app/events_fold.rs:415-422` falls through to `push_status(format!("entry appended → {ty}"))`. The seam exists and is unused: `crates/cyrup-ext/src/native.rs:295` `register_entry_renderer`, hook at `native.rs:429-435`, dispatched at `app/extension_render.rs:164`.

**upstream** — `pi-intercom` has **no** entry surface at any tag: `grep -n 'pi.register' index.ts` @v0.7.0 returns only `registerMessageRenderer`, `registerTool` ×2, `registerCommand`, `registerShortcut`. The one displayed custom message (`index.ts:662-668`, `display: true`) is both the model's context and the human's card, drawn by `registerMessageRenderer("intercom_message", …)` at `:1149-1153`. cyrup's `append_entry` split is a cyrup-original addition documented as "the port doc §4.2/§7.2 human surface" (`inbound.rs:450-453`).

**Impact** — The bordered card cyrup renders at `inbound.rs:471` is never drawn by anything; the durable surface it produces is one grey status line reading `entry appended → intercom_message`. Every consumer of the structured `from`/`message`/`replyCommand` payload is likewise absent. Pure waste plus transcript noise.

**Fix** — Two options. **(b), the parity answer:** delete the split — make the displayed message the single surface (ICOM-027 + ICOM-024) and drop `surface_incoming_message`'s `append_entry`, which is what upstream does. **(a), the port-doc-compatible answer:** register `api.register_entry_renderer("intercom_message")` in `init` and implement `NativeExtension::render_entry` to deserialize the payload and return the `card` lines, re-rendered at the live width once ICOM-024 lands. Pick one and correct the stale rationale at `ui/mod.rs:12-19` either way.

**Verify** — For (a): assert `host.render_entry("intercom_message", &payload)` returns `RenderOutcome::Rendered` for the exact payload `surface_incoming_message` produces, and a TUI test asserting the transcript contains the card border rather than the literal `entry appended → intercom_message`. For (b): assert `append_entry` is no longer called on the inbound path and the transcript still contains the message.

## ICOM-029 — `HostServices::inject_message` carries no `details`

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-ext/src/host/services.rs:330-338` — `fn inject_message(&self, _content: &str, _custom_type: Option<&str>, _display: bool, _trigger_turn: bool) -> Result<(), String>`. Four parameters, no `details`. The live impl (`crates/cyrup-session-svc/src/host_services.rs:801-822`) builds `InjectMessage { content, custom_type, display, trigger_turn }` (struct at `:242`) and the sink calls `AgentSession::inject_message` (`cyrup-session-svc/src/session.rs:3732-3767`), which passes `None` for details at `:3762`; the subscriber path passes `None` at `subscriber.rs:183`. **The capability exists one layer down and is unreachable from the seam:** `AgentSession::send_custom_message` (`session.rs:3686-3716`) takes `details: Option<serde_json::Value>` and forwards it at `:3711`.

**upstream** — `pi-intercom` v0.7.0 `index.ts:667` passes `details: entry` alongside `content`, and the renderer at `:1150-1152` reads exactly that — `const details = message.details as { from: SessionInfo; message: Message; replyCommand?: string; bodyText?: string } | undefined; if (!details) return undefined;`. Same at v0.10.1 `index.ts:885` (`details: deliveredEntry`).

**Impact** — **Blocks ICOM-024.** Even after `register_message_renderer("intercom_message")` is added, `render_call` would receive a message whose payload is a bare markdown string, so it could not rebuild the `InlineMessage` (no `from`, no `replyCommand`, no attachment list) and would fall back to the default `[type] body` box — i.e. exactly today's behaviour. Details are lost on the trigger path too, before the `display` question arises. Any other native extension wanting a rendered custom message hits the same wall.

**Fix** — Add a `details: Option<&serde_json::Value>` parameter to `HostServices::inject_message` (`crates/cyrup-ext/src/host/services.rs:330`), carry it on `InjectMessage` (`cyrup-session-svc/src/host_services.rs:242`), and thread it into `AgentSession::inject_message`'s non-trigger arm (`session.rs:3762`) **and** the run-input/steer arms' persistence (`subscriber.rs:183`). Then pass the structured entry from `crates/cyrup-intercom/src/inbound.rs:250`/`:286` — the same object `inbound.rs:461-469` already builds for `append_entry`. **Cross-area handoff: areas 06 (cyrup-ext) and 08 (session-svc).**

**Verify** — Unit-test that a message injected with `details` round-trips through `AgentSession` into the `CustomMessage` entry's `details` field, and that `render_message_call_outcome("intercom_message", …)` receives it.

## ICOM-030 — `contact_supervisor` is registered even when a native supervisor channel is active

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/extension.rs:461-463` — `if let Some(metadata) = &self.metadata { api.register_tool(Arc::new(ContactSupervisorTool::new(self.state.clone(), metadata.clone()))); }`. The gate is metadata-only. `grep -rn 'SUPERVISOR_CHANNEL_DIR' crates/cyrup-intercom/` returns **zero** hits. The variable is real and is written on the production spawn path: `crates/cyrup-ext-subagents/src/spawn/intercom_target.rs:50` defines `ENV_SUPERVISOR_CHANNEL_DIR` (`CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR`), `crates/cyrup-ext-subagents/src/exec/mod.rs:1811` inserts it into every child's env overlay, and `crates/cyrup-ext-subagents/src/native_supervisor.rs:343` consumes it.

**upstream** — `pi-intercom` v0.7.0 `index.ts:29` (`SUBAGENT_SUPERVISOR_CHANNEL_DIR_ENV = "PI_SUBAGENT_SUPERVISOR_CHANNEL_DIR"`), `:1170` (`const nativeSupervisorChannelAvailable = Boolean(process.env[…]?.trim())`), `:1171` (`if (childOrchestratorMetadata && !nativeSupervisorChannelAvailable) { pi.registerTool({ name: "contact_supervisor", … }) }`). Identical at v0.10.1 `index.ts:1505-1507`. The v0.7.0 CHANGELOG names it: "suppressing legacy supervisor tools when native supervisor channels are present".

**Impact** — A cyrup subagent child launched through the native supervisor channel is handed **both** the native channel and the legacy broker-routed `contact_supervisor` tool. The model picks one, so the same decision can be requested through two mechanisms and the parent may be polling only one of them. *(Caveat: the further claim that a mis-picked ask is lost until the 10-minute ask timeout is **not** established — `native_supervisor.rs`'s polling loop was not read, and the intercom clarify seam may still correlate it. The certain harm is two competing supervisor mechanisms in a `!has_ui` child.)*

**Fix** — Gate the registration at `crates/cyrup-intercom/src/extension.rs:461` on `self.metadata.is_some() && env var absent-or-blank`. Capture the probe at construction (`IntercomExtension::new`) rather than reading process env inside `init`, matching how `read_child_orchestrator_metadata` is captured, and add the constant beside `ENV_ORCH_TARGET` in `identity.rs:26-36`.

**Verify** — Unit-test `init` twice against a stubbed env lookup: with the channel dir set, assert `InitApi` holds only the `intercom` tool; without it, assert both tools register.

## ICOM-031 — Presence identity never re-synced on `turn_start` or at the head of a tool call

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/extension.rs:598-604` — the `HostEvent::TurnStart` arm calls only `tracker.begin_turn(now_ms())`; no presence sync. `crates/cyrup-intercom/src/tools/intercom.rs:49-53` — `dispatch` opens with `crate::connect::ensure_connected(&self.state, ConnectReason::Tool)` and goes straight into the action `match`; no presence sync. The only presence writer is `sync_presence` (`extension.rs:199-213`), driven from four lifecycle arms, and it hard-codes `name: None`.

**upstream** — `pi-intercom` v0.7.0 `index.ts:1132` — the `turn_start` handler calls `syncPresenceIdentity(sessionId)` **before** `replyTracker.beginTurn()`; `index.ts:1487` — the `intercom` tool's `execute` calls `syncPresenceIdentity(ctx.sessionManager.getSessionId())` immediately after `ensureConnected("tool")`. `syncPresenceIdentity` (`:589-596`) sends `{ ...buildPresenceIdentity(pi, sessionId), status: currentStatus() }` — i.e. the **name**. Both sites unchanged at v0.10.1 (`index.ts:1445`, `:1853`).

**Impact** — Upstream has three independent name-sync points (the poll, `turn_start`, and every intercom tool call); cyrup has none. A session renamed by `/name`, a branch switch or a title change keeps advertising its startup label to every peer's `intercom{list}` and `/intercom` picker forever, so operators address the wrong worker. These two sites are the cheap 80% of ICOM-006 and need no timer at all.

**Fix** — Add a `sync_presence_identity(&self)` on `IntercomExtension` that recomputes the name the way `connect::build_registration` does (`connect.rs:437-457` — `presence_name(services.session_name(), services.session_id())`) and calls `client.update_presence_with_context(Some(name), Some(self.presence_status(base)), None, …)`. Call it from the `TurnStart` arm (`extension.rs:598`) and expose it to `IntercomTool::dispatch` (`tools/intercom.rs:51`) through `SharedIntercomState`. Land before ICOM-006, which then reuses it.

**Verify** — Unit test: connect with session name `alpha`, change the stub `HostServices::session_name()` to `beta`, dispatch `intercom({action:"status"})`, and assert a `Presence` frame carrying `name: "beta"` was sent.

## ICOM-032 — Session shutdown leaves the pending-idle queue populated

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/extension.rs:562-576` — the `HostEvent::SessionShutdown` arm calls `set_flush_timer(None)`, `connect::shutdown`, `waiter.fail_pending`, `client.disconnect()`, `set_client(None)` and `tracker.reset()`. It never drains `pending_idle`. The queue lives on the extension-lifetime `Arc<SharedIntercomState>` (`session_state.rs:39-41`) and the only drain is `take_pending_inbound` (`:131-134`) inside `flush_idle_messages`.

**upstream** — `pi-intercom` v0.7.0 `index.ts:1069-1071` — the `session_shutdown` handler does `replyTracker.reset(); pendingIdleMessages.length = 0; clearInboundFlushTimer();`. At v0.9.x the same site additionally emits `expired` receipts for each dropped entry. That one extension instance genuinely sees more than one session is upstream-proven, not inferred: `index.ts:1118-1124`'s `turn_start` handler has an explicit `if (!currentSessionId || sessionId !== currentSessionId) { startSessionRuntime(ctx); … }` branch.

**Impact** — Messages parked while a session was busy survive its shutdown. If the same `IntercomExtension` sees a second `SessionStart` (runtime replacement, an RPC re-attach), the first `AgentEnd`/`TurnEnd` fires `schedule_inbound_flush(state, 0)` and the new session is handed the previous session's peer messages as live inbound turns, attributed to peers it never talked to. Even without a rebuild, the entries and their attachment bodies are retained for the process lifetime.

**Fix** — In `crates/cyrup-intercom/src/extension.rs:562`, add `let _ = self.state.take_pending_inbound();` beside the existing `set_flush_timer(None)`. Do the same wherever a session-replace path is added. Subsumed if ICOM-035 lands and the queue is deleted outright.

**Verify** — Unit test: queue two messages, dispatch `HostEvent::SessionShutdown`, assert `pending_inbound_len() == 0`; then re-run `begin_runtime` + `schedule_inbound_flush(&s, 0)` against an idle host and assert nothing is injected.

## ICOM-033 — No tool renderers for `intercom` / `contact_supervisor`

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `grep -rn 'register_tool_renderer\|fn render_call\|fn render_result' crates/cyrup-intercom/src/` returns **zero** hits. `crates/cyrup-intercom/src/extension.rs:457-497` (`init`) registers the two tools and declares no renderer for either; `impl NativeExtension for IntercomExtension` (`extension.rs:452-615`) implements `id`/`init`/`set_host_services`/`execute_command`/`on_event` only, so the trait defaults at `crates/cyrup-ext/src/native.rs:409` and `:415` (`render_call`/`render_result` → `None`) apply. The seam is live and used in production by a sibling crate: `InitApi::register_tool_renderer` at `native.rs:279`, dispatched at `crates/cyrup-tui/src/app/extension_render.rs:162-163`, consumed by `crates/cyrup-ext-subagents/src/extension.rs:9616`/`:9646`.

**upstream** — `pi-intercom` v0.7.0 `index.ts:1783-1801` (`intercom` `renderCall` — bold `intercom `, the action colour-coded warning/success/accent, `→ target`, an `(N attachments)` badge and a 96-char message preview) and `:1802-1817` (`renderResult` — a `✓`/`✗` prefix derived from `context.isError || details.error === true || details.delivered === false`, the short message id when collapsed, a `Reason: …` line when expanded); `index.ts:1397-1410` and `:1411-1429` for `contact_supervisor`.

**Impact** — Every intercom and `contact_supervisor` call renders as an undifferentiated tool row: no action colour, no `→ peer` target, no attachment count, no message preview, no ✓/✗, and — the load-bearing one — **no message id**, which is the value `intercom({action:"reply", replyTo})` needs. A transcript with several peer conversations in it is unreadable, and a failed send does not visually read as failed.

**Fix** — Call `api.register_tool_renderer("intercom")` and, inside the metadata branch, `api.register_tool_renderer("contact_supervisor")` in `IntercomExtension::init` (`extension.rs:460-463`); implement `NativeExtension::render_call`/`render_result` keyed on the tool name, porting the four upstream bodies. `previewText(value, maxLength = 72|96)` (`index.ts:434-440`) needs porting too. Reuse `crate::ui::Theme` / `truncate_to_width` for width handling.

**Verify** — Unit-test `render_call("intercom", &json!({"action":"ask","to":"reviewer","message":"…"}))` against the upstream shape and `render_result("intercom", …)` for both the delivered and `delivered:false` cases; assert `ExtensionHost::render_tool_call_outcome("intercom", …)` is `Rendered` after loading the extension.

## ICOM-034 — The subagent relay has no self-target guard

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/seams.rs:79-155` — `IntercomDeliveryChannel::send` branches on `self.supervisor_target` being `None` (top-level orchestrator → local delivery) and otherwise goes straight to `ensure_connected` → `resolve_target` → `client.send(&resolved, …)` (`:147-153`). There is no check that the resolved target is this session. `grep -rn 'current_session_target_matches\|session_target_matches' crates/cyrup-intercom/src/` returns **zero** hits. The broker does not backstop it either: `handle_send` (`broker/mod.rs:740-866`) never compares `target_id` to `current_id`. Same hole in `IntercomSteerChannel::steer` (`seams.rs:180-202`).

**upstream** — `pi-intercom` v0.7.0 `index.ts:628-640` `currentSessionTargetMatches(to, resolvedTo?, activeClient?)` builds a lower-cased target set from `currentSessionId`, `activeClient?.sessionId`, `pi.getSessionName()` and `buildPresenceIdentity(pi, currentSessionId).name`, and also returns true when `resolvedTo === activeClient.sessionId`. `relaySubagentIntercomPayload` consults it **twice** — `index.ts:991` before resolving and `:1012` after — and on a match calls `deliverLocalSubagentRelayMessage` instead of `activeClient.send`. Unchanged at v0.10.1.

**Impact** — A child orchestrator whose `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET` resolves to itself (a name collision between two same-named sessions — which ICOM-040 makes routine — or a re-exec that inherited its own env) relays its subagent result through the broker to its own session id. cyrup's broker delivers it, so the session receives its own result as an inbound peer message with a synthetic sender, and the local-delivery path that surfaces the result with correct attribution is skipped.

**Fix** — Port `currentSessionTargetMatches` as a helper on `SharedIntercomState` (it needs `self_session_id()`, already at `session_state.rs:212-215`, plus `HostServices::session_name()` and `presence_name(..)` from `identity.rs:94-106`) and consult it at both points in `IntercomDeliveryChannel::send` (`seams.rs:83` pre-resolution and `:148` post-resolution), routing a match to the existing local-delivery block at `seams.rs:85-139`. Apply the post-resolution check to `IntercomSteerChannel::steer` (`seams.rs:192-196`) as well.

**Verify** — Integration test against the real broker: register one session, give the delivery channel a `supervisor_target` equal to that session's own name, call `send`, and assert the message is surfaced locally (`append_entry` + `inject_message`) and that no broker `send` frame is emitted.

## ICOM-035 — Busy inbound messages are parked until idle instead of steered onto the live run

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — The whole queue machine is present and is the only busy-path behaviour: `crates/cyrup-intercom/src/inbound.rs:42-46` (`INBOUND_FLUSH_DELAY_MS = 200`, `INBOUND_IDLE_RETRY_MS = 500`), `:141-146` (`queue_idle_message`), `:179-188` (`schedule_inbound_flush`), `:195-210` (`flush_idle_messages`), `InboundPolicy::Queue` at `:67-70` returned by `decide_inbound_policy` at `:113`; storage at `session_state.rs:39-41` + `:123-176`; drivers at `extension.rs:587` and `:609`.

**upstream** — `pi-intercom` v0.9.3 `25ffb96` ("fix: steer busy inbound messages promptly") **deleted** `pendingIdleMessages`, `queueIdleMessage`, `scheduleInboundFlush`, `flushIdleMessages`, `clearInboundFlushTimer`, `expirePendingIdleMessages`, `INBOUND_FLUSH_DELAY_MS` and `INBOUND_IDLE_RETRY_MS` outright. At v0.10.1 the busy-interactive branch is one line — `index.ts:956` `sendIncomingMessage(entry, "steer")` — and `sendIncomingMessage` (`:876-899`) delivers with `{ deliverAs: "steer" }` (`:898`) and rewrites the reply hint to carry the explicit message id (`:882-884`). The `agent_end`/`turn_end` `scheduleInboundFlush(0)` calls are gone (`index.ts:1421`, `:1451`). CHANGELOG 0.9.3: "Hand busy interactive inbound messages directly to Pi's safe steering queue instead of waiting for aggregate idle, preventing stale coordination from appearing hours after it was received."

**Impact** — A message that arrives while a cyrup session is working is invisible to the running agent and is replayed only after the run ends — upstream's own words, "stale coordination appearing hours after it was received". A supervisor cannot redirect a worker mid-task, which is the primary reason intercom exists. It also keeps alive ICOM-023's scheduling race and ICOM-032's shutdown leak, both of which are defects in code upstream no longer has.

**Fix** — **A seam change is not strictly required**: `AgentSession::inject_message` already routes to `self.agent.steer(msg)` whenever `is_streaming()` (`cyrup-session-svc/src/session.rs:3752-3754`), so replacing `InboundPolicy::Queue` (`inbound.rs:67-70`, `:113`) with an immediate `send_incoming_message(entry, …)` would steer today. Then delete `inbound.rs:42-46`/`:141-146`/`:179-210` and `session_state.rs:39-41`/`:123-176`, and drop the `schedule_inbound_flush(&self.state, 0)` calls at `extension.rs:587` and `:609`. Port the steer-mode reply-hint rewrite (upstream `index.ts:882-884`) into `build_inline_message` (`inbound.rs:426-447`). If an *explicit* delivery selector is wanted rather than relying on `is_streaming()`, extend `HostServices::inject_message` (`crates/cyrup-ext/src/host/services.rs:330`) with a `DeliverAs` argument reaching `AgentSession::send_custom_message`'s existing `DeliverAs::{NextTurn,FollowUp}` fan-out — **cross-area handoff: areas 06 and 08** — but confirm first that pi's `deliverAs: "steer"` means what `AgentSession::steer` means (Coverage blind spot 7).

**Verify** — Integration test with a busy `IdleControlledHost`: deliver an inbound message and assert it reaches the host within one scheduler tick with a steer delivery, rather than after `INBOUND_FLUSH_DELAY_MS + INBOUND_IDLE_RETRY_MS` and only once idle.

## ICOM-036 — No reply targeting by sender-ID prefix; the four v0.9.3 disambiguation errors are absent

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/reply_tracker.rs:25-30` — `matches_pending_sender` matches only an exact `context.from.id == to` or a case-insensitive exact name. `reply_tracker.rs:122-129` — the `to` branch filters with that predicate and answers with two strings only: `Multiple pending asks from "{to}" — use the sender session ID instead.` (`:126`) and `No pending ask from "{to}"` (`:128`).

**upstream** — `pi-intercom` v0.9.3 `c3543d6` / v0.10.1 `reply-tracker.ts:10-16` — `matchesPendingSender` now also accepts `context.from.id.startsWith(to)`; `:18-45` adds `resolvePendingSender(pending, to)`, a four-tier ladder (exact id → exact name → id prefix → miss) with four distinct messages: `` Multiple pending asks from session ID "${to}" — specify `replyTo` ``, `` Multiple pending asks match sender name "${to}" — specify a full session ID or `replyTo` ``, `` Multiple pending asks match ID prefix "${to}" — use a longer session ID prefix or specify `replyTo` ``, and `No pending ask from "${to}"`; wired at `:96-97`.

**Impact** — `intercom{list}` prints an 8-char `short_session_id` as the addressable column (`tools/intercom.rs:374`) and `intercom{pending}` prints message ids, but `intercom({action:"reply", to:"<prefix>"})` fails with `No pending ask from "<prefix>"` — the agent is handed an id form the reply path rejects, so replies to a pending ask can only be addressed by full UUID or by name. The three-way ambiguity messages that tell the caller *how* to disambiguate are also missing. *(Caveat: cyrup's own schema at `tools/intercom.rs:391` reads only "Target session name or id (send/ask/reply)." and never promises prefix resolution — the promise is upstream's description text at v0.7.0 `index.ts:1459-1460`. The impact chain runs through the printed `list` column, not through a broken schema promise.)*

**Fix** — Port `resolvePendingSender` into `crates/cyrup-intercom/src/reply_tracker.rs` as a free function beside `matches_pending_sender` (`:25`), add the `starts_with` arm to `matches_pending_sender` for the `reply_to` cross-check at `:112-117`, and replace the `to` branch at `:122-129` with a call to it. Four string-equality tests, one per tier.

**Verify** — Unit test with two pending asks from distinct UUIDv7 senders sharing a prefix: assert a unique-prefix `to` resolves, a shared-prefix `to` raises the `use a longer session ID prefix` message, and two asks from one sender raise the `` specify `replyTo` `` message.

## ICOM-037 — A plain `send` to the sole pending asker is not treated as its reply

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/tools/intercom.rs:89-157` — the `"send"` arm passes `reply_to: params.reply_to.clone()` straight through (`:120`) and only calls `crate::inbound::dismiss_incoming_ask` when the caller supplied `reply_to` explicitly (`:146-151`). `grep -rn 'find_unique_pending_ask_from\|findUniquePendingAsk' crates/cyrup-intercom/src/` returns **zero** hits — `ReplyTracker` (`reply_tracker.rs:33-190`) has no such method.

**upstream** — `pi-intercom` v0.9.3 `5d76146` ("Resolve asks answered with send"), v0.10.1 `reply-tracker.ts:114-123` `findUniquePendingAskFrom(to, now)` — filters unexpired pending asks by `from.id === to || from.name?.toLowerCase() === to.toLowerCase()` and returns the single match or null; consumed at `index.ts:2035-2062` for the wire `replyTo`, the audit entry, `dismissIncomingAsk(effectiveReplyTo)` and the result text `Reply sent to ${targetDisplay} (inferred from pending ask)`. CHANGELOG 0.9.3: "Treat a public send to the sole pending asker as its reply."

**Impact** — When a peer asks and the agent answers with `intercom({action:"send", to:"peer", message:"…"})` — the natural phrasing, and the one a model reaches for when it has a target but no message id — the ask is never dismissed. It stays in `pending`, it stays in the pending-idle queue, and `flush_idle_messages` re-injects it once the run ends (the exact re-injection `dismiss_incoming_ask` exists to prevent, `inbound.rs:148-172`). The asking peer's blocking waiter also never resolves, because the send carries no `replyTo`, so it hangs to the 10-minute ask timeout.

**Fix** — Add `find_unique_pending_ask_from(&mut self, to: &str, now: u64) -> Option<IntercomContext>` to `ReplyTracker` (beside `list_pending` at `reply_tracker.rs:171`) honouring `ask_timeout_ms`. In the `"send"` arm compute `effective_reply_to = params.reply_to.or_else(|| tracker.find_unique_pending_ask_from(&target, now).map(|c| c.message.id))` after the self-guard at `:94`, use it in `SendOptions.reply_to` (`:120`), in the audit entry (`:141`), in the dismissal at `:146-151`, and switch the result text at `:157` to `Reply sent to {to} (inferred from pending ask)` when it was inferred.

**Verify** — Integration test against the real broker: peer A asks B; B calls `intercom({action:"send", to:"A", message:"…"})` with no `replyTo`; assert A's blocking `ask` resolves, B's `pending` is empty, and B's result text carries `(inferred from pending ask)`.

## ICOM-038 — No client liveness heartbeat, so a half-open broker socket is never detected

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `grep -rni 'heartbeat\|liveness' crates/cyrup-intercom/src/` returns only doc prose (`transport/client.rs:114`, `:356`, `:383`) plus the unrelated broker-side `PRESENCE_HEARTBEAT_MS` — no timer, no probe. `IntercomClient` has no periodic task; the only disconnect detection is the reader task observing EOF/error, which routes to `InboundEvent::Disconnected` → `crate::connect::handle_disconnect` (`inbound.rs:396-402`, `connect.rs:403-419`). Neither `CYRUP_INTERCOM_LIVENESS_INTERVAL_MS` nor `..._TIMEOUT_MS` exists, while every other `PI_INTERCOM_*` var has a `CYRUP_INTERCOM_*` counterpart in `identity.rs:11-36` and `transport/target.rs:34-39`.

**upstream** — `pi-intercom` v0.9.3 `f260df0` ("fix(client): liveness heartbeat detects half-open broker sockets"), v0.10.1 `broker/client.ts:39-55` — the doc comment states the failure mode verbatim: "A half-open socket (peer killed with SIGKILL or crashed without sending a FIN) stays 'writable' indefinitely, so passive close-event detection never fires and the client silently drops out of the roster." Implementation at `:72-73` (`livenessTimer`/`livenessInFlight`), `:106-118` (`startLivenessHeartbeat`/`stopLivenessHeartbeat`, `setInterval(getLivenessIntervalMs()).unref()`), plus `runLivenessProbe`, which round-trips a `list` and destroys the socket if no reply arrives inside `getLivenessTimeoutMs()`. Defaults 30 s / 5 s from `PI_INTERCOM_LIVENESS_INTERVAL_MS` / `_TIMEOUT_MS`, the timeout clamped to `Math.min(raw, interval)` (`:47-55`). New test file `broker/client-liveness.test.ts` (173 lines).

**Impact** — If the broker is SIGKILLed, panics without closing, or the socket half-opens, a cyrup session keeps a dead `IntercomClient` in `SharedIntercomState` indefinitely. `is_connected()` stays true, so `ensure_connected` hands the dead client back, the reconnect ladder is never armed, and every send silently fails or hangs. The session also stays in other peers' `intercom{list}` as a live participant nobody can reach — the failure mode ICOM-003's ladder was built to eliminate, reachable through a path the ladder cannot see.

**Fix** — Add `CYRUP_INTERCOM_LIVENESS_INTERVAL_MS` / `_TIMEOUT_MS` beside `ENV_INTERCOM_NAME_POLL_MS` (`identity.rs:24`) with the 30 s / 5 s defaults and the `min(timeout, interval)` clamp. Spawn a probe task from `IntercomClient::connect` (`transport/client.rs:181-205`) that every interval issues the existing `list_sessions()` round trip under a `tokio::time::timeout`, guarded by an in-flight flag; on timeout, close the stream so the reader task emits `InboundEvent::Disconnected` and `connect::handle_disconnect` arms the ladder. Cancel the task in `disconnect()`.

**Verify** — Integration test: connect to a real broker, `SIGKILL` the broker process without letting it close, and assert that within `interval + timeout` the client reports disconnected and the ladder is armed. Use a short override interval so the test runs in under a second.

## ICOM-039 — `intercom{list}` prints a fixed 8-char session id instead of a distinguishing prefix

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/identity.rs:108-112` — `pub fn short_session_id(session_id: &str) -> String { session_id.chars().take(8).collect() }`, whose doc still cites `index.ts:365-367`. Consumed at `crates/cyrup-intercom/src/tools/intercom.rs:374` inside `format_session_list_row`, and by the session picker in `ui/session_list.rs`. The ambiguity error at `session_state.rs:186-190` names no candidate ids at all.

**upstream** — `pi-intercom` v0.9.3 `72309e0` ("fix: show unique session ID prefixes"), v0.10.1 `index.ts:387-406` — `shortSessionId` is **replaced** by `sessionIdPrefixes(sessions): Map<string,string>`, which computes each session's longest shared prefix with every other session, takes `Math.max(8, longestSharedPrefix + 1)`, then extends to the next `-` group boundary (`session.id.indexOf("-", minimumLength)`). Threaded as a fourth `idPrefix` argument into `formatSessionListRow` (`:447-453`) and used at `:1872`/`:1876` (`list`), `:1926`/`:1930` (`list-cwd`) and in the `Multiple sessions named …` error. `formatSessionLabel` (`:441-446`) deliberately keeps the raw 8-char slice.

**Impact** — UUIDv7 session ids started in the same millisecond share far more than 8 leading characters. cyrup's `list` prints an identical `(abcdef12)` for two different peers, and that string is exactly what the agent is told to address them by — `resolve_target` (`session_state.rs:180-190`) then finds two matches and errors `Multiple sessions match "abcdef12". Use the session ID instead.` without saying which ids to pick from. Addressing a peer becomes impossible from `list` output alone.

**Fix** — Port `sessionIdPrefixes` into `crates/cyrup-intercom/src/identity.rs` beside `short_session_id` (`:108`), taking the full `&[SessionInfo]`. Thread the per-session prefix into `format_session_list_row` (`tools/intercom.rs:334-374`) and compute the map once per `list` call. Land with ICOM-013's ambiguity-message fix so `session_state.rs:186-190` can print the candidate prefixes. Leave `short_session_id` for the label path, which upstream deliberately kept at 8.

**Verify** — Unit test `session_id_prefixes` against two UUIDv7s sharing 20 characters: assert the returned prefixes differ, are at least 8 chars, and end on a `-` group boundary; then assert `format_session_list_row` prints them.

## ICOM-040 — The unnamed-session fallback alias uses 8 id characters, not 18

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/identity.rs:103-105` — `presence_name` returns `format!("{DEFAULT_UNNAMED_SESSION_ALIAS_PREFIX}-{short}")` where `short` is `normalized.chars().take(8)`. The test at `identity.rs:216-217` pins the 8-char form. This alias is the presence **name** the session registers under (`connect.rs:437-457`) and therefore the string peers address it by, so a collision is an addressing collision, not a display one.

**upstream** — `pi-intercom` v0.10.0 `126875e`; v0.9.2 `index.ts:402` is `slice(0, 8)`, v0.10.1 `index.ts:425` is `` `${DEFAULT_UNNAMED_SESSION_ALIAS_PREFIX}-${normalizedSessionId.slice(0, 18)}` ``. CHANGELOG 0.10.0: "Extend unnamed-session fallback aliases with enough session-ID characters to distinguish UUIDv7 sessions started close together."

**Impact** — Two unnamed cyrup sessions whose UUIDv7 ids were minted in the same millisecond register under the **same** presence name. `find_session_ids` (`broker/routing.rs:18-38`) then returns both for that name, and the broker answers every send to it with `Multiple sessions named "subagent-chat-abcdef12" are connected. Use the session ID instead.` (`broker/mod.rs:786-790`) — neither peer is reachable by its advertised alias. Subagent children are exactly the population that gets an alias rather than a name, and fan-out spawns them milliseconds apart. Also makes ICOM-034's self-relay hole reachable.

**Fix** — Change `.take(8)` to `.take(18)` at `crates/cyrup-intercom/src/identity.rs:105` and update the pinned expectations at `:216-217`. **Check the other side of the coupling:** `cyrup-ext-subagents`' `orchestrator_presence_target` must produce the identical string or the child's `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET` stops matching (the coupling is documented at `connect.rs:443-448`). **Cross-area handoff: area 09.**

**Verify** — Unit test two UUIDv7s differing only after character 12: assert `presence_name(None, a) != presence_name(None, b)`. Add a cross-crate test that `orchestrator_presence_target` and `presence_name` agree on the same `(name, id)` pair.

## ICOM-041 — `runtimeFallbackAlias` is neither modelled nor applied

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `grep -rn 'runtime_fallback_alias\|runtimeFallbackAlias' crates/cyrup-intercom/src/` returns **zero** hits. `transport/protocol.rs:237-290` — `SessionInfo` models id/name/cwd/model/pid/startedAt/lastActivity/status/peerUid/trustedLocal/context×3 and no `runtime_fallback_alias`; `:699-717` — `ClientMessage::Presence` models name/status/model/context×3 only. `broker/mod.rs:667-685` builds the stored `SessionInfo` field-by-field from the registration and sets `extra: Default::default()`, discarding any additive registration key; `handle_presence` (`:893-960`) has no such arm; `connect.rs:437-457` `build_registration` never sets it.

**upstream** — `pi-intercom` v0.10.0 `126875e`, v0.10.1 `types.ts:6-7` (`/** True only when the extension synthesized name for an unnamed runtime. */ runtimeFallbackAlias?: boolean;` on `SessionInfo`) and `types.ts:88` (on the `presence` client frame); produced at `index.ts:427-433` (`buildPresenceIdentity` returns `{ name, runtimeFallbackAlias: !sessionName?.trim() }`), tracked across the name poll at `:814-831`; the broker applies it at `broker/broker.ts:779-787` (presence, **inside** the ownership block) and `:358` (register relay), and gates mailbox identity on it at `:1039-1047` (`if (!lowerName || info.runtimeFallbackAlias) return [];`).

**Impact** — A cyrup broker relaying between two pi v0.10 sessions strips the flag, so a pi peer cannot tell a chosen name from a synthesized alias; and if the mailbox is ever ported (ICOM-010) the guard that stops one unnamed session inheriting another's queued mail has no input. A cyrup client also never advertises the flag, so a pi broker treats every cyrup session as durably named. Inert today, load-bearing the moment ICOM-010 lands.

**Fix** — Add `runtime_fallback_alias: Option<bool>` to `SessionInfo` (`transport/protocol.rs:237-290`) and to `ClientMessage::Presence` (`:699-717`) with the crate's existing `present_non_null` guard; set it in `connect::build_registration` (`connect.rs:437-457`) from whether a real session name was found; carry it through `handle_register`'s `SessionInfo` construction (`broker/mod.rs:667-685`) and add the presence arm in `handle_presence` (`:930-950`) — with the boolean type check placed **inside** the ownership block, per ICOM-014's fix.

**Verify** — Extend `tests/session_info_context_fields.rs` with a round-trip asserting the field survives a cyrup-broker hop, and `tests/presence_context_usage.rs` with a presence frame that flips it.

## ICOM-042 — cwd-scoped `send`/`ask` and `openProjectPaneIfMissing` are unported

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** medium (upstream contract characterised from exports + CHANGELOG, not a complete read — see Coverage blind spot 4)

**cyrup** — `crates/cyrup-intercom/src/tools/intercom.rs:24-36` — `IntercomParams` has action/to/message/attachments/replyTo; no `cwd`, `openProjectPaneIfMissing` or `focus`. The schema at `:382-410` declares the same five plus `action`. The `"send"` arm (`:89-157`) and `"ask"` arm (`:159-198`) both hard-require `to` via `require(params.to, …)`. No `project_agent.rs` or `cwd.rs` exists in `crates/cyrup-intercom/src`.

**upstream** — `pi-intercom` v0.10.0 `c7987b3` ("feat: open project panes for cwd messages"), v0.10.1 `project-agent.ts` (324 lines — a `HerdrClient` spawning the `herdr` binary from `HERDR_BIN`, a six-variant `HerdrErrorCode` union at `:10-16`, `openProjectPane`, `waitForProjectSession`, `resolveTargetInCwd`); `index.ts:27` (import), `:62-66` (`DeliveryTarget`), `:1174-1217` (`resolveCwdDeliveryTarget`), `:1832-1841` (the three new schema params), `:1973-2066` (the `send` case), `:2071-2186` (the `ask` case), `:2100-2113` (the `openProjectPaneIfMissing requires a target cwd.` guard), `:2052-2060` (the `Opened Herdr project pane … and sent message to …` result plus the `openedProjectPane`/`paneId`/`projectRoot` details).

**Impact** — A cyrup agent cannot address a peer by working directory — it must know the peer's name or id in advance — and cannot start a peer session in another repo at all. Cross-codebase coordination, which is what the feature exists for, is unavailable, along with the confirm-send variant for pane launches and the structured pane result details.

**Fix** — Port `cwd.ts` first as `crates/cyrup-intercom/src/cwd.rs` (**shared with ICOM-018 — do it once**), then `resolveTargetInCwd` and `resolveCwdDeliveryTarget`. Add `cwd` / `open_project_pane_if_missing` / `focus` to `IntercomParams` (`tools/intercom.rs:24-36`) and the schema (`:382-410`), relax the `to` requirement in the `send`/`ask` arms to `to || cwd`, and thread the resolved `DeliveryTarget { id, label, project_pane }` through the result strings. **The Herdr half needs a decision first (OQ): whether cyrup shells out to a `herdr` binary at all.** Note this is not a green field — `cyrup-ext-subagents` already references Herdr (`tui/fleet.rs:58-60`, `tui/fleet_overlay.rs:37`/`:263`/`:529`, `extension.rs:9861`) where the crate documents a deliberate Herdr-inspector divergence; align with that decision rather than making a fresh one. Land the cwd-targeting half independently — it needs no external binary and carries the user-visible benefit.

**Verify** — Register two sessions in different cwds; assert `intercom({action:"send", cwd:"<dir>", message:"…"})` with no `to` reaches the sole peer there, and that `openProjectPaneIfMissing` without `cwd` returns `openProjectPaneIfMissing requires a target cwd.`

## ICOM-043 — The v0.10.0 copy revision is unported: `📨`/`📎`/`↩`/`↳` still present

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/ui/inline_message.rs:78` — `"**📨 From {}** ({}){}\n\n{}"` in `content_markdown`, the string that reaches the **model** as well as the human; `:98` the card header `" 📨 From: {sender_name} ({cwd}) "`; `:117` `" ↩ To reply: {rc}"`; `:129` `" 📎 {}"` per attachment; `:139` `" ↳ Reply to {short}"`; the collapsed meta rows at `:156`, `:162`, `:168`. `crates/cyrup-intercom/src/inbound.rs:412` and `:415` — `format_attachments` emits `"\n\n---\n📎 {name}…"`. Pinned by tests at `inline_message.rs:295` and `inbound.rs:546`, `:795`, `:837`, `:841`, `:867`.

**upstream** — `pi-intercom` v0.10.0 `633e782` ("refactor: deslop intercom protocol cleanup"). `git diff v0.9.2..v0.10.1 -- ui/inline-message.ts index.ts`: header ` 📨 From: ` → ` From: ` (`ui/inline-message.ts:45`); `↩ To reply:` → `To reply:` (`:65`, `:90`); `📎 ${count} attachment…` → `${count} attachment…` (`:69`); `↳ Reply to` → `Reply to` (`:71`, `:105`); `📎 ${att.name}` → `Attachment: ${att.name}` (`:99`); `formatAttachments` `\n📎 ${att.name}` → `\nAttachment: ${att.name}` in both the language and non-language arms (`index.ts:99`, `:101`); and the injected content `**📨 From ${senderDisplay}**` → `**From ${senderDisplay}**` (`index.ts:893`).

**Impact** — The string the model receives on every inbound message differs from upstream's at the first token. Agents and skill docs written against pi's `**From …**` prefix will not match, and the attachment separator `---\n📎 name` differs from `---\nAttachment: name`, which is the delimiter a model is expected to parse when a peer sends files. Cosmetically, the card also disagrees with pi's terminal-safe rendering on fonts without emoji coverage.

**Fix** — Apply the eight replacements at `ui/inline_message.rs:78`, `:98`, `:117`, `:129`, `:139`, `:156`, `:162`, `:168` and `inbound.rs:412`, `:415`, then update the pinned assertions at `inline_message.rs:295` and `inbound.rs:546`, `:795`, `:837`, `:841`, `:867`. **This fix is incomplete on its own: `content_markdown` is also missing v0.9.2's `_deliveryMetadata_` segment — see ICOM-048. Land the two together** so `content_markdown` ends up matching v0.10.1 exactly rather than being emoji-free but still structurally divergent. Also re-read the v0.10.0 `SKILL.md` rewrite before porting it under ICOM-004 — it is the copy this change aligns with.

**Verify** — String-equality test on `content_markdown()` against the full v0.10.1 template (with ICOM-048's metadata line), and on `format_attachments` against the `Attachment: ` form.

## ICOM-044 — A malformed intercom config fails closed silently instead of erroring with the path

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/config.rs:74-90` — `load_config` maps any `parse_config` error to a `tracing::warn!` plus `IntercomConfig { inbound_trigger: InboundTrigger::Never, ..Default::default() }`. The module doc at `config.rs:3-5` calls this "the load-bearing behavior … fails CLOSED to `inbound_trigger = Never` on ANY parse/validation error (`config.ts:139-142`)" and `config.rs:209-215` pins it as a test. The single caller, `intercom_extension_for_env_concrete` (`extension.rs:661-675`), calls it infallibly and cannot distinguish a corrupt config from a valid restrictive one.

**upstream** — `pi-intercom` v0.10.0, `config.ts:153-155` — the `catch` no longer returns a fallback: `const message = error instanceof Error ? error.message : String(error); throw new Error(\`Failed to load intercom config at ${configPath}: ${message}\`, { cause: error });`. `git diff v0.9.2..v0.10.1 -- config.ts` shows the removed `console.error(...); return { ...defaults, inboundTrigger: "never" }`. CHANGELOG 0.10.0: "Surface malformed intercom config errors with path context instead of silently falling back to defaults."

**Impact** — A user who typos `~/.cyrup/intercom/config.json` gets an intercom that connects, lists, sends and asks normally but never auto-triggers on an inbound message — the exact symptom of a working install with `inboundTrigger: "never"` set on purpose. Nothing on screen says the config failed to parse (the diagnostic is a `tracing::warn!`, invisible in the TUI), and the config path is never named. Upstream refuses to construct the extension and says which file and why.

**Fix** — Change `load_config` (`config.rs:74-90`) to return `Result<IntercomConfig, String>` carrying `format!("Failed to load intercom config at {}: {err}", path.display())`, and propagate it from `intercom_extension_for_env_concrete` (`extension.rs:661-675`), which **already** returns `Result<_, String>` for the analogous `ask_timeout_ms()` hard error — the precedent is in place. Replace the fail-closed test at `config.rs:209-215` with one asserting the error message names the path.

**Verify** — Write a corrupt `config.json`, call `intercom_extension_for_env`, and assert the `Err` string contains the absolute config path and the underlying parse message.

## ICOM-045 — A blocking `ask` (and a supervisor decision) is not refused up front when the target is offline

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/tools/intercom.rs:159-165` — the `"ask"` arm resolves through `resolve_or_err`, whose miss answers the bare `Session not found: "{to}"` (`:319-325`) and says nothing about queueing or retrying, regardless of `expects_reply`. `broker/mod.rs:791-797` likewise answers an unresolvable target with a bare `DeliveryFailed { reason: "Session not found" }` with no `expects_reply` branch. `crates/cyrup-intercom/src/tools/contact_supervisor.rs:42-52` `resolve_supervisor` falls back to `preferred_supervisor_target(&self.metadata)` on a miss and sends anyway, with no reason-specific refusal for the blocking `need_decision` / `interview_request` paths.

**upstream** — `pi-intercom` v0.10.0/v0.10.1 `index.ts:2108-2110` — `` `Session "${to}" is not currently connected. Blocking asks are not queued; use send for a non-blocking mailbox delivery or retry after the session reconnects.` `` with `details: { error: true }`; `contact_supervisor` at `index.ts:1590-1595` (`` `Supervisor "${metadata.orchestratorTarget}" is not currently connected. Blocking requests are not queued; use a progress update or retry after the supervisor reconnects.` ``) with `resolveSupervisorTarget` now returning `string | null` (`:1166-1174`); broker side at `broker/broker.ts:632-638` (`if (message.expectsReply)` → `Target session is not currently connected; blocking asks are not queued`). CHANGELOG 0.10.0 names it.

**Impact** — Mostly message quality today, because cyrup has no mailbox (ICOM-010) so the delivery fails either way. But the failure text tells the agent nothing actionable — no "blocking asks are not queued", no "use send instead", no "retry after the peer reconnects" — so the model retries the same blocking ask. It becomes a real hang the moment ICOM-010 lands: a mailbox would accept the ask and the caller would block to the 10-minute timeout.

**Fix** — Port the three refusal strings verbatim: the `"ask"` miss branch at `tools/intercom.rs:162`; the blocking arms of `tools/contact_supervisor.rs` (make `resolve_supervisor` return `Option<String>` and refuse for `need_decision`/`interview_request` while letting `progress_update` fall through to the raw target); and the broker's `expects_reply` branch in `handle_send` (`broker/mod.rs:791`). **Sequence this before ICOM-010** so the mailbox never swallows a blocking ask.

**Verify** — Call `intercom({action:"ask", to:"ghost"})` against a live broker with no such session and assert the exact upstream string; same for `contact_supervisor({reason:"need_decision"})` with an unregistered supervisor.

## ICOM-046 — `intercom({action:"reply"})` silently drops attachments

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/tools/intercom.rs:220-227` — the `"reply"` arm calls `client.send(&target.from.id, SendOptions { text: message.clone(), attachments: None, reply_to: Some(target.message.id.clone()), expects_reply: None, message_id: None })`. `attachments: None` is hard-coded even though `params.attachments` is parsed (`:33`) and the schema advertises `attachments` for the whole tool (`:393-405`). The audit entry at `:246` likewise records `{ "text": message, "replyTo": … }` with no attachments key.

**upstream** — `pi-intercom` v0.10.1 `2ba9f53` ("fix: preserve reply attachments (#100)"), `index.ts:2217-2221` — `const result = await connectedClient.send(target.from.id, { text: message, attachments, replyTo: target.message.id });` — and `:2235` records it in the audit entry. CHANGELOG 0.10.1: "Preserve attachments when replying through `intercom({ action: \"reply\" })`."

**Impact** — An agent answering a peer's ask with a file, snippet or context attachment sends the prose and silently loses the payload. The tool reports `Reply sent to …` with no indication anything was dropped, so the asking peer receives an answer referencing content it never got — and the audit entry records the same lie, making the loss undiscoverable after the fact. This is the exact bug upstream shipped a point release for. *(Genuine drift, not a porting miss: v0.7.0 `index.ts:1697-1700` also omitted attachments, so cyrup was correct at its baseline.)*

**Fix** — One line: pass `attachments: params.attachments.clone()` at `tools/intercom.rs:222`, and add the `"attachments"` key to the audit payload at `:246` to match upstream's shape.

**Verify** — Integration test against the real broker: A asks B; B replies with one `{type:"snippet"}` attachment; assert the inbound `Message` A receives carries `content.attachments` with that entry, and that the `intercom_sent` audit entry records it.

## ICOM-047 — Broker startup failures discard the broker's stderr

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/transport/spawn.rs:147-149` — the spawn sets `.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())`, so the child's diagnostics are discarded at the OS level and cannot be recovered later. `broker_wait_error` (`transport/spawn.rs:178-196`) can therefore only produce `intercom broker exited before startup with signal {name}` / `… with code {code}` / `… with code unknown`, with no room for a cause.

**upstream** — `pi-intercom` v0.10.1 `c9675a5`, `broker/spawn.ts:25` (`const BROKER_STARTUP_STDERR_LIMIT = 4_000;`), `:29`/`:40` (`captureStartupStderr` on both `BrokerLaunchSpec` arms), `:156-176` (`getBrokerSpawnOptions(extensionDir, env, captureStderr)` switching `stdio` to `["ignore","ignore","pipe"]`), `:216-232` (`rememberBrokerStderr` keeping the last 4 KB and `brokerStartupError(message, cause)` appending `\nBroker stderr:\n${stderr}`), applied to all four rejection paths (`:243`, `:254`, `:258`, `:267-268`). It also switched the exit listener from `exit` to `close` (`:262`, `:271`) so the pipe has drained before the error is built. CHANGELOG 0.10.1: "…and include broker stderr when startup exits early."

**Impact** — When the detached broker dies during startup — a bind failure, a panic, an `assert_no_live_broker` refusal (`broker/mod.rs:1238`), a permissions error under `ensure_intercom_runtime_dir` — the user is told only `intercom broker exited before startup with code 1`. The broker is detached with all stdio nulled, so there is no log file and no terminal output to consult; the actual reason is unrecoverable. Every "intercom won't connect" report becomes unactionable.

**Fix** — In `transport/spawn.rs:149` use `Stdio::piped()` for stderr, drain the child's stderr into a bounded 4 KB tail buffer on the wait path (`:160-176`), and append `\nBroker stderr:\n{tail}` in `broker_wait_error` (`:178-196`) when the tail is non-empty. Keep stdout null. Take the child's stderr handle before detaching so the pipe is not inherited by the long-lived process. *(The same upstream commit's tsx-resolution half is mechanism-forced and out of scope: cyrup re-execs `current_exe __intercom-broker`, `transport/spawn.rs:130-140`.)*

**Verify** — Point the spawn at a stub that writes a known line to stderr and exits 1; assert the returned `IntercomError::Broker` message contains both `exited before startup with code 1` and that line.

## ICOM-048 — Injected content omits the `_deliveryMetadata_` line, so the model never sees the inbound message id

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

This is a **baseline** gap at the crate's own targeted tag (v0.9.2), not v0.9.3+ drift, and no prior item covers it. Found by the refuter while checking ICOM-043.

**cyrup** — `crates/cyrup-intercom/src/ui/inline_message.rs:70-84` — `content_markdown` emits `**📨 From {}** ({}){}\n\n{}` — sender, cwd, reply instruction, body — with **no metadata segment at all**, and `inbound.rs:248`/`:284` feed exactly that string to `inject_message`. cyrup also queues the raw message as turn context (`inbound.rs:239-243`) rather than an `injectedAt`-stamped copy. The envelope fields are already modelled (`transport/protocol.rs:310-332`: `sender_sequence`/`broker_delivered_at`/`receiver_received_at`/`injected_at`/`supersedes`/`retry_of`), so this is a **renderer** gap, not a wire gap.

**upstream** — `pi-intercom` v0.9.2 `index.ts:891` injects `` `**📨 From ${senderDisplay}** (${entry.from.cwd})${replyInstruction}\n\n_${deliveryMetadata}_\n\n${entry.bodyText}` ``, where `formatInboundDeliveryMetadata` (v0.9.2 `index.ts:446-460`, identical at v0.10.1 `:471-485`) **always** emits at least `id ${message.id}` and then optionally `seq N`, `supersedes …`, `retry of …`, `sent …`, `broker delivered …`, `receiver received …`, `injected …`, joined by ` · `. Upstream also stamps a per-delivery copy — `const injectedMessage = { ...entry.message, injectedAt: Date.now() }` (v0.10.1 `index.ts:878`) — and queues **that** as the turn context.

**Impact** — The model is handed an intercom message with **no id**, so `intercom({action:"reply", replyTo:"<id>"})` is unreachable without a separate `pending` call — and `pending` is the only place in cyrup's whole surface that prints a message id. It also loses the ordering/supersede/latency signals upstream deliberately surfaces to the model.

**Fix** — Port `formatInboundDeliveryMetadata` as a helper beside `content_markdown` in `crates/cyrup-intercom/src/ui/inline_message.rs`, reading the already-modelled envelope fields from `transport/protocol.rs:310-332`, and insert `\n\n_{metadata}_\n\n` between the reply instruction and the body at `:78`. Stamp `injected_at` on a per-delivery copy at `inbound.rs:239-243` and queue that copy as the turn context. **Land with ICOM-043** so `content_markdown` reaches the v0.10.1 shape in one edit rather than two.

**Verify** — String-equality test on `content_markdown()` against `**From sender** (/tmp/project)\n\n_id msg-1 · seq 3_\n\nbody`; assert the id is present for a message carrying no optional envelope fields at all.

## ICOM-049 — Inbound delivery and the pending-idle flush carry no runtime-generation guard

**Kind** parity-bug · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/inbound.rs:355-388` — the dispatch goes `record_incoming_message` → `surface_incoming_message` → `decide_inbound_policy` → deliver, with no generation read and no liveness check. `schedule_inbound_flush` (`:179-188`) captures no generation and checks no liveness; `flush_idle_messages` (`:195-210`) checks only `pending_inbound_len()` and `is_idle()`. The machinery exists and is used elsewhere: `connect::begin_runtime` bumps a generation on every `SessionStart` (`connect.rs:201-218`) and `connect_once` consults it before publishing the client — the omission is on the **delivery** half only.

**upstream** — `pi-intercom` v0.7.0 re-checks `getLiveContext(ctx, generation)` at **six** points around one inbound message: `index.ts:712-715` (entry to `handleIncomingMessage`), `:737-740` (inside the async IIFE), `:756` (before `dismissIncomingAsk` on the busy auto-reply), `:765` (before `sendIncomingMessage`), `:653-655` (the top of `sendIncomingMessage` itself), plus `scheduleInboundFlush` (`:674-677`), which early-returns when there is no live context and captures `scheduledGeneration`, and `flushIdleMessages` (`:686-695`), which re-resolves the context for that generation and returns rather than delivering through a stale one.

**Impact** — An inbound message whose delivery task is already in flight when the session runtime is replaced (an RPC re-attach, a runtime rebuild) is delivered into the **new** session, attributed to a peer the new session never talked to — and a flush scheduled against the old runtime fires against the new one. Same blast radius as ICOM-032, but a different mechanism: ICOM-032 is the queue not being drained, this is the in-flight task not being fenced. **Fixing ICOM-032 alone does not close it.**

**Fix** — Thread the runtime generation (`connect.rs:201-218`) into the inbound delivery path: read it at the top of the dispatch (`inbound.rs:355`), re-check it before each of the four delivery decisions, capture it in `schedule_inbound_flush` (`:179`) and re-check it in `flush_idle_messages` (`:195`). Port upstream's `getLiveContext` shape rather than inventing one. If ICOM-035 lands and the queue is deleted, the flush half disappears but the dispatch half remains required.

**Verify** — Unit test: begin a delivery against generation G with a host that blocks mid-`inject_message`, call `begin_runtime` to bump to G+1, release the block, and assert nothing is injected.

## ICOM-050 — `intercom_received` audit entry drops `messageId` and `attachments` and re-timestamps

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-intercom/src/tools/intercom.rs:189-196` — `append_entry("intercom_received", json!({ "from": to, "message": { "text": reply }, "timestamp": now_ms() }))`.

**upstream** — `pi-intercom` v0.7.0 `index.ts:1650-1655` — `pi.appendEntry("intercom_received", { from: to, message: { text: replyText, attachments: replyMessage.content.attachments }, messageId: replyMessage.id, timestamp: replyMessage.timestamp })`. Unchanged at v0.10.1, so this is a porting miss at the baseline, not drift.

**Impact** — Three losses in the durable audit record of a peer exchange: the reply's own message id, its attachment list, and the **sender's** timestamp (replaced by local receipt time). Matters for the same reason ICOM-046 does — the record no longer matches what was exchanged, so the loss is undiscoverable after the fact. cyrup's sibling `intercom_sent` entries (`tools/intercom.rs:132-144`, `:176-188`, `:242-250`) **do** carry `messageId`, which makes the omission look like an oversight rather than a decision.

**Fix** — At `tools/intercom.rs:189-196` add `"messageId": reply_message.id`, add `"attachments"` inside the `message` object, and use the reply message's own timestamp instead of `now_ms()`.

**Verify** — Integration test: A asks B, B replies with an attachment; assert A's `intercom_received` entry carries the reply's message id, its attachment list, and the sender's timestamp rather than the receipt time.

---

## ICOM-051 — An ambient `CYRUP_INTERCOM` opted every hermetic binary-seam test into intercom, leaving an immortal broker per test process

**Kind** test-defect · **Severity** high · **Effort** S · **Confidence** confirmed · **Status** fixed this pass

**cyrup** — `is_installed()` (`crates/cyrup-intercom/src/extension.rs:630-631`) is `env_truthy(INSTALL_ENV_VAR) || config_path(intercom_dir).exists()`, with the var named at `:87` (`CYRUP_INTERCOM`). The four `crates/cyrup/tests/*.rs` fixtures that exercise the real binary — `one_shot_parity.rs`, `piped_stdin_trim.rs`, `unknown_flag_exit.rs`, `extension_load_failure_exit.rs` — scrubbed provider keys and proxies from the child environment and declared themselves hermetic, but did not scrub the three built-in opt-ins. On any machine that exports `CYRUP_INTERCOM=1` (this one does) every child therefore attached intercom and spawned a broker, and a broker that no session ever registered with never reaches `schedule_shutdown_check` (`broker/mod.rs:1005-1027`) and so never exits. Measured A/B on the four targets with the ambient vars still exported: **13 surviving `cyrup __intercom-broker` processes** with the fixtures at HEAD, **0** with the scrub. Two full `cargo test --workspace --no-fail-fast` runs after the fix ended with 0.

**upstream** — the broker's immortality is correct parity and must NOT be "fixed" with a startup idle-exit: `pi-intercom` v0.10.1 `broker/broker.ts:221`/`:429` reach `scheduleShutdownCheck()` only from a *registered* session's teardown, exactly as `broker/mod.rs:1005-1027` does. What pi does not have is the ambient attachment: `pi` v0.83.0 `packages/coding-agent/src/core/resource-loader.ts:451-452` (`const extensionPaths = this.noExtensions ? cliEnabledExtensions : this.mergePaths(...)`) means an upstream `--no-extensions` run loads only explicit `-e` paths, and pi-intercom is an ordinary discovered extension — so the equivalent pi run has no intercom and no broker at all.

**Impact** — This was the mechanism behind two of the three symptoms the suite showed: the orphaned-broker count after a full run, and the `one_shot_parity` stall (`wait_with_output()`/`output()` read to EOF, not to child exit, so any surviving grandchild in the pipe group can hold the harness open indefinitely — a target that finishes 4/4 in 4.19 s alone was reported "running for over 60 seconds" in the full run). It also silently widened the extension set of `extension_load_failure_exit.rs`, whose entire contract is that the set is the one it *plants*.

**Fix** — *applied.* `.env_remove("CYRUP_INTERCOM") / ("CYRUP_SUBAGENTS") / ("CYRUP_PERMISSION_SYSTEM")` in the shared child-command builder of each of the four fixtures, beside the existing key/proxy scrub. No assertion was touched. In-repo precedent for the stronger form is `crates/cyrup/tests/auth_credential_print.rs`, which already uses `env_clear` + an allowlist.

**Verify** — done. All 16 tests across the four targets green; `ps -axo pid,command | grep -cE '[/]cyrup __intercom-broker'` returns 0 immediately after and 12 s later (past the 5 s shutdown window). **Measurement note, because it corrupted the original figure:** `pgrep -f '__intercom-broker'` matches *its own* pattern inside other shells' command lines and over-counts — the "22 orphaned brokers" in the original report is not trustworthy. Use `ps -axo pid,command | grep -cE '[/]cyrup __intercom-broker'`.

**Investigated and refuted — do not re-file as a code defect.** A prior diagnosis proposed that `spawn_detached_broker` (`crates/cyrup-intercom/src/transport/spawn.rs:137-172`) leaks non-`CLOEXEC` descriptors into the broker, and prescribed a `pre_exec` FD sweep. That is wrong on three independent counts: (a) the function already sets `Stdio::null()` on 0/1/2 (`:147-149`), so it cannot hold a harness pipe through stdio; (b) Rust std on macOS spawns via `posix_spawn` and marks its own pipes `FD_CLOEXEC`, and a controlled experiment — 40 detached children spawned while 8 threads hammered `Command::output()` — found **0** inherited stray PIPE fds (the earlier apparent confirmation used a descriptor created raw with `libc::pipe`, proving only that a deliberately-non-`CLOEXEC` fd is inherited); (c) the crate is `#![forbid(unsafe_code)]` (`crates/cyrup-intercom/src/lib.rs:14`), a deliberate policy the prescribed patch does not compile against. Anyone reopening this needs a fresh reproduction *and* sign-off to relax that policy. The product-side residue is not here at all — it is `SEAM-071` (area 08): `--no-extensions` does not gate native built-ins.

---

## ICOM-052 — The broker socket path has no `SUN_LEN` guard, so a long agent-dir path silently and permanently degrades intercom

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** **confirmed — reproduced first-hand, and pi's spawn read at v0.9.2** · **observed 2026-08-13** (headless-binary; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Read the upstream note before rating this.** pi has the **same shape** — `broker/spawn.ts` also
> spawns with `stdio: "ignore"` and `broker/paths.ts:65-74` builds the socket path with no length
> check — so this is **not** a parity bug and must not be filed as one. It is a robustness gap cyrup
> shares with upstream, filed because it was *measured here* and because the diagnostic half is
> cheap and entirely cyrup's to fix.

**cyrup** — `crates/cyrup-intercom/src/paths.rs:81-85` `broker_socket_path` is `intercom_dir.join("broker.sock")` with **no length validation**, and `crates/cyrup-intercom/src/broker/mod.rs:1243` binds it with `UnixListener::bind(&socket_path)?`. On macOS `sockaddr_un.sun_path` is 104 bytes, so a sufficiently deep `HOME`/`CYRUP_AGENT_DIR` makes the bind fail. The parent cannot see why: `crates/cyrup-intercom/src/transport/spawn.rs:147-149` sets `Stdio::null()` on all three descriptors, so the child's error text is discarded, and `:189-194` synthesises the parent-visible message from the exit status alone.

**upstream** — `pi-intercom` v0.9.2, read this pass: `broker/spawn.ts:168`/`:175` spawn with `stdio: "ignore"`, and `:229`/`:232` reject with `Intercom broker exited before startup with signal/code …` — the identical shape, including the identical loss of the child's reason. `broker/paths.ts:65-74` `getBrokerSocketPath` has no length guard either. Cyrup's port is faithful; both fail the same way.

**Impact** — Every cyrup invocation in a scratch `HOME` during this exercise logged:

```
WARN cyrup_intercom::extension: intercom: startup connect failed; scheduling reconnect
  error=intercom broker error: intercom broker exited before startup with code 1
```

Running the broker subcommand directly gives the real reason, which the parent **never** surfaces:

```
$ HOME=<long path> CYRUP_AGENT_DIR=<long path>/.cyrup ./target/debug/cyrup __intercom-broker
__intercom-broker: path must be shorter than SUN_LEN
EXIT=1
```

With a short `HOME` the broker starts and stays up, so this is path-length dependent rather than universal. But the failure is **permanent for the session** and the only user-visible trace is a WARN whose text names neither the cause nor the path — so anyone with a deep home or project path silently loses intercom, and subagent messaging with it, having been told only that something exited with code 1. Note the diagnostic gap is broader than this one cause: **every** broker startup failure is reported as "code 1", so a config error, a permissions error and this are indistinguishable.

**Fix** — Two independent halves; the second is worth doing regardless of the first. **(a)** Guard the path: in `broker_socket_path` (`paths.rs:81-85`), check the byte length against the platform's `sun_path` limit (104 on macOS, 108 on Linux) and fall back to a short hashed path under `std::env::temp_dir()` when it would overflow, as other Unix daemons do. **(b)** Surface the reason: capture the child's stderr on the startup path (`spawn.rs:147-149` currently nulls it) — pipe it, read it with a bounded timeout, and fold the text into the `IntercomError::Broker` message at `:189-194`, so *any* broker startup failure names itself. (b) is a strict improvement over upstream and is where the value is; take (a) only if the fallback path is acceptable to the design.

**Verify** — Construct an agent dir whose `intercom/broker.sock` exceeds 104 bytes and assert the broker either binds successfully via the fallback (fix a) or that the parent's error message contains `SUN_LEN` and the offending path (fix b). Then assert the short-path case is unchanged and the broker still shuts down cleanly, per the harness check recorded above.


## ~~ICOM-053~~ — ~~medium~~ **CLOSED 2026-09-04** — The workspace gate runs none of the broker-socket seam tests

**Kind** test-gap · **Severity** ~~medium~~ · **Effort** M · **Confidence** confirmed (both sides read: cyrup at `1d2b4418`, pi-intercom at `v0.13.0` and every tag back to the ported baseline `v0.9.2`)

**Verdict** — CLOSED by making the documented gate RUN the suite. `cyrup-it`'s `[[test]]` targets stay behind `required-features = ["it"]` (`docs/TEST-ARCHITECTURE.md` §2.3 rejects every alternative, and this closure does not reopen that design), so `cargo nextest run --workspace` still builds none of them by design; what changed is the one line of README "Build" that reached the suite. `cargo run -p xtask -- feature-matrix`'s `cyrup-it` row was `check -p cyrup-it --features it,wasm-host --all-targets` — a type-check of 486 seam tests that executed none. It is now `nextest run -p cyrup-it --features it,wasm-host`, the literal "everything" command of §2.4, on the precedent the matrix already carried for MCP-037a (*"a type-check would not catch a wrong answer, only a wrong type"*).

**upstream** — pi-intercom `v0.13.0` `package.json:21`: the single `"test"` script is `tsx --test … intercom.integration.test.ts …` — the real-socket broker suite (4 351 lines at the tag) runs in the same invocation as every unit test, and the tag has no `.github/` workflow. Byte-for-byte the same shape at `v0.9.2` (`package.json:20`), `v0.10.1`, `v0.12.0`, `v0.12.1`: upstream has never gated its socket suite separately.

**cyrup** — `xtask/src/features.rs` `MATRIX`, the `cyrup-it` `Combo` (verb `nextest`, args `run -p cyrup-it --features it,wasm-host`, `slow: true`), its `verb` field doc and the module doc; `xtask/src/main.rs` command summary; README "Build" L161. The everyday gate and `--fast` are unchanged.

**What the first run found** — the item's premise, observed rather than argued: two targets had rotted at HEAD with nothing to notice. `tests/permission/forwarding_spawn_env.rs:149` no longer compiled (`E0063`: the `AgentConfig` literal lacked `acceptance_role`/`default_acceptance` from SUBA-082 `5a4ae4ed` and `exclude_tools`/`allow_nested_subagents` from SUBA-092 `247ff97b` — those commits fixed the five sibling sites in the `subagents`/`intercom` targets and missed the `permission` one), and `tests/subagents/subagent_tool_renderer_integration.rs:417` no longer compiled (`E0277`: `json!` over `ToolResult.terminate` after `4b7097be` retyped it to `cyrup_core::TerminateHint`, which deliberately has no `Serialize`). Both fixed in `1d2b4418`; the renderer test's wire value now mirrors `cyrup-agent/src/agent/message.rs::result_value_of` exactly (the `terminate` key emitted iff `TerminateHint::wire()` is `Some`, never as a null).

**What the first full run also found — a flake, fixed at its source** — `tests/permission/human_dialog.rs` `perm034_a_reload_wipes_always_grants_exactly_as_upstream_does` failed once under load (`Session(StreamingNeedsBehavior)` on its second `harness.run`) and passed 3/3 re-run alone. `cyrup-test-support` `Harness::run` returned when the run-scoped stream ended; on an unbound session that stream is closed inside the agent's `agent_end` emit (`cyrup-session-svc/src/subscriber.rs::end_run`), BEFORE `SettlementGuard::drop` (`cyrup-agent/src/agent/lifecycle.rs`) releases the `running` latch that `is_run_active()` (`session/run.rs` `prompt`) reads — the window `cyrup-agent/src/tests/settlement_latch.rs` documents as "milliseconds when the machine is loaded enough to preempt". pi's multi-turn tests never prompt inside it: each follows `await session.prompt(...)` with `await session.waitForIdle()` (`core/agent-session.ts:1626` @v0.84.4). `Harness::run` now awaits `session.wait_for_idle()` after draining (`crates/cyrup-test-support/src/harness.rs`), and its doc no longer claims the stream end is race-free for a caller. Only `cyrup-it` calls `Harness::run` (0 in-crate callers in the four other dev-dependents), so the seam suite re-run after the fix is the whole blast radius.

**Test** — `xtask/src/features.rs` `tests::the_matrix_runs_the_gated_seam_suite_rather_than_type_checking_it`: exactly one matrix row selects `cyrup-it`, its verb is `nextest` with `run` first, it arms `it,wasm-host`, and it is `slow`. Run against the old row before the change it FAILED at `assert_eq!(row.verb, "nextest")` (left `"check"`); it passes at `1d2b4418`. The two restored targets are their own proof: they did not compile before and run green after. `crates/cyrup-test-support/src/lib.rs` `smoke::run_returns_only_once_the_session_is_idle` (40 turns on the 4-thread runtime, `session().is_idle()` asserted the instant `run` returns): FAILED 3/3 runs before the harness fix (turn 2-3 each time), passes 3/3 after.

**Run record** — after every fix, at the tree of `1d2b4418` (a concurrent track's uncommitted `cyrup-ext-subagents` work was also present in the shared tree), target by target: `intercom` 79/79, `permission` 48/48, `subagents` 195/195, `ext` 41/41 (1 slow), `mcp` 33/33, `session_svc` 29/29, `bin` 63/63, `verify_redaction` 1/1, `misc` 0 tests (see residual) — 489 tests, 0 failed, 0 leaked, 0 skipped; before the fixes the first run had `permission` and `subagents` failing to compile and `perm034` losing its race once. Disk on this host forced the run to go target-by-target through the documented `CYRUP_IT_BIN_DIR` / `CYRUP_EXT_FIXTURE_COMPONENT` shortcut (`crates/cyrup-it/build.rs` header; §2.4 "skip the second link") rather than as one invocation; the tests executed are the same eight targets the matrix row runs.

**Checks** — `cargo fmt --all -- --check` clean; `cargo clippy -p xtask --all-targets -- -D warnings` clean; `cargo clippy -p cyrup-it --features it,wasm-host --all-targets -- -D warnings -A clippy::redundant_closure` clean (the single allow is for the dependency `cyrup-tui`'s `app/input_reader.rs:443`, committed in `3e69ea2a` and recorded by area 07 `TUI-089`; without it the run fails in that crate before reaching this one — two further deny-level lints that were `cyrup-it`'s own, `tests/mcp/http_oauth.rs:1952` and `tests/mcp/protocol_13i.rs:222`, are fixed in this commit); `cargo nextest run -p xtask` 15 passed; `cargo clippy -p cyrup-test-support --all-targets -- -D warnings` clean, `cargo nextest run -p cyrup-test-support` 41 passed, `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-test-support --no-deps` clean; `RUSTDOCFLAGS='-D warnings' cargo doc -p xtask --no-deps --bins` clean.

**Residuals (low)** — (0) `crates/cyrup-it/tests/misc/main.rs` is an empty `[[test]]` target: `mod support;` and nothing else, while its header still describes ten files across five crates; `cargo nextest run … --test misc` reports 0 tests. Stale doc plus a target that proves nothing — recorded here, not deleted, because removing a target is a §9.1 decision. (1) `--fast` skips the row: deliberate, documented at README L161, and pinned (`slow: true`) so the flag and the sentence cannot diverge silently. (2) No CI and no schedule run the gate for anyone — README "Build" says so, and adding a workflow is the process decision this row always recorded as not an agent's to make. Falsification condition for this closure: a `cyrup-it` target that fails to compile or fails at HEAD while `cargo run -p xtask -- feature-matrix` (without `--fast`) reports green.

---

## ~~ICOM-060~~ — ~~medium~~ **CLOSED 2026-09-04** — No guard against a misdirected reply during an ask-triggered turn

**Kind** not-ported (upstream drift past the v0.10.1 baseline) · **Severity** ~~medium~~ · **Effort** M · **Confidence** confirmed (both sides read: cyrup at `a91e3c41`, pi-intercom at `v0.13.0` `199279a` and the introducing commit `5fe0ee3` at v0.12.1)

**Verdict** — CLOSED at `a91e3c41` by a line-for-line port. No design decision was required: cyrup's `ReplyTracker` already carried the one piece of state the guard reads (`current_turn_context`, set by `turn_start` → `begin_turn` in `extension.rs`, cleared by `turn_end` → `end_turn`), so the port is one method and one guard.

**upstream** — pi-intercom `v0.12.1` `5fe0ee3` (#119, issue #117; CHANGELOG 0.12.1 "Refuse non-reply `send` calls to a different target during a turn triggered by an inbound ask, preventing CWD hierarchy or roster guesses from misdirecting replies"), byte-identical at `v0.13.0`:
- `reply-tracker.ts:124-130` `findActiveReplyTargetMismatch(to, now = Date.now())`: `pruneExpired(now)`; `null` unless `currentTurnContext?.message.expectsReply`; then `currentTurnContext.from.id === to ? null : currentTurnContext`. ID only — `reply-tracker.test.ts` "active ask context does not trust sender names as destination identity" pins that the asker's own NAME is still a mismatch.
- `index.ts:2320-2328` (`send` arm): `const activeReplyMismatch = replyTo ? null : replyTracker.findActiveReplyTargetMismatch(sendTo)` — after the self-target guard (`:2314-2319`), before `findUniquePendingAskFrom` (`:2329`) — returning `This turn is responding to an intercom ask from "${from.name || from.id}". Use intercom({ action: "reply", message: "..." }) or set replyTo: "${message.id}". Refusing non-reply send to "${targetDisplay}" to avoid a misdirected reply.` with `details: { error: true, replyTo }`.
- `intercom.integration.test.ts:3225-3260` "intercom send refuses a different target during an active inbound ask turn"; `reply-tracker.test.ts:96-114` the two tracker cases.
- The `ask` arm carries no such guard upstream, and none was added here.

**cyrup** — `crates/cyrup-intercom/src/reply_tracker.rs` `ReplyTracker::find_active_reply_target_mismatch(&mut self, to: &str, now: u64) -> Option<IntercomContext>` (`&mut self` because upstream prunes here, unlike `find_unique_pending_ask_from`, whose non-pruning `&self` is deliberate and documented on that method); `crates/cyrup-intercom/src/tools/intercom/send.rs` `action_send`, the guard between `Cannot message the current session` and the ICOM-037 inferred-reply lookup, keyed on the RESOLVED `target` like that lookup, skipped when `params.reply_to` is set, `from.name || from.id` ported as name-if-non-empty (JS `||` treats `""` as falsy).

**Tests** — `reply_tracker.rs` `tests::active_ask_context_flags_non_reply_sends_to_a_different_target`, `tests::active_ask_context_does_not_trust_sender_names_as_destination_identity` (the two upstream cases, same fixtures and expectations) and `tests::active_ask_context_is_silent_without_a_live_ask_turn` (no context / non-ask context / expired ask pruned before the comparison / `end_turn`); before the change these do not compile (the method did not exist). `crates/cyrup-it/tests/intercom/tool_actions.rs` `send_refuses_a_different_target_during_an_active_inbound_ask_turn` — the upstream integration test against the real broker, with the ask routed THROUGH the broker (so its reply edge exists) and the worker's tracker fed exactly as the inbound loop + `turn_start` would feed it: the refusal text is asserted whole (naming `planner` by name and `cwd-hierarchy-ask`), the orchestrator's channel is drained and must carry presence but no `Message`, the ask is still pending, and the positive half — the same turn's `send` to `planner-id` — returns `Reply sent to planner-id (inferred from pending ask)` and reaches the planner with `replyTo` set. Established fail-before by reverting only `send.rs` to the pre-fix HEAD and re-running: FAILED with `a non-reply send to a different target during an ask turn is refused: ToolResult { … "Message sent to orchestrator" … delivered: true }` — the misdirected delivery the item names; passes at `a91e3c41`.

**Checks** — `cargo fmt --all -- --check` clean on the committed files; `cargo clippy -p cyrup-intercom --all-targets -- -D warnings` clean; `cargo nextest run -p cyrup-intercom` 296/296; `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-intercom --no-deps` clean; `cargo nextest run -p cyrup-it --features it --test intercom` 80/80 (run through the documented `CYRUP_IT_BIN_DIR` / `CYRUP_EXT_FIXTURE_COMPONENT` shortcut, `crates/cyrup-it/build.rs` header, because the shared host was at 100% disk and could not relink the other seam targets — the `intercom` target is the only one this change touches).

**Residuals (low)** — (0) `details: { error: true, replyTo }`: cyrup's `ToolError` is message-only, the crate's standing convention for every upstream `details.error` result (the undelivered-send and self-target results are shaped the same way), so the ask id is in the refusal text, not a structured field; a model that wants it re-reads the string. (1) The `intercom{send}` description in `tools/intercom/mod.rs` and the bundled `SKILL.md` do not mention the guard — upstream changed only its README (`5fe0ee3` README.md:221, :369), not `SKILL.md` (v0.13.0 `skills/pi-intercom/SKILL.md:270` still describes only the sole-pending-ask inference), so there is nothing to port there. Falsification condition: a turn whose `current_turn_context` is an ask, and a `send` with no `replyTo` whose resolved target is not `from.id`, that returns `Message sent to …`.

---

## Coverage

### Read first-hand (cyrup, HEAD `04c1ba2`, tree clean)

`crates/cyrup-intercom/src/{lib.rs (full), config.rs (full, 241 lines), identity.rs (full, 238 lines), inbound.rs (full, 869 lines), session_state.rs (full, 376 lines), reply_tracker.rs:1-190, connect.rs:59-160 + 344-460 + tests 490-590, extension.rs:1-330 + 440-700, seams.rs:55-270, tools/intercom.rs:1-470 + 620-1058, tools/contact_supervisor.rs:1-60, relay.rs (full), format_context.rs:55-95, ui/inline_message.rs:1-175, ui/mod.rs:1-40, transport/protocol.rs:230-350 + 625-770, transport/spawn.rs:140-210, transport/target.rs:250-330}`; `broker/mod.rs` in ranges `:190-260`, `:370-610`, `:609-700`, `:700-880`, `:893-1040`, `:1200-1250`; `broker/routing.rs` (full).

**Cross-crate, read rather than grepped:** `crates/cyrup-ext/src/native.rs:240-300 + 318-435` (the `InitApi` surface and all four renderer hooks), `crates/cyrup-ext/src/host/services.rs:224-340`, `crates/cyrup-ext/src/event.rs:39/:329/:403`, `crates/cyrup-session-svc/src/session.rs:570-600 + 3640-3770 + 4610-4672`, `crates/cyrup-session-svc/src/host_services.rs:242-260 + 801-822`, `crates/cyrup-session-svc/src/subscriber.rs:170-195`, `crates/cyrup-agent/src/agent.rs:450-495`, `crates/cyrup-tui/src/app/session_bind.rs:250-290`, `app/events_fold.rs:90-124 + 392-424`, `app/extension_render.rs:146-189`, `crates/cyrup-ext-subagents/src/exec/mod.rs:1754/:1811`, `crates/cyrup-ext-subagents/src/spawn/intercom_target.rs:50`, `crates/cyrup-ext-subagents/src/native_supervisor.rs:343`.

### Read first-hand (upstream)

Both tags materialised to disk with `git -C pi-intercom show <tag>:<path>` (never the working tree): **v0.7.0** (19 files) and **v0.10.1** (26 files), plus **v0.9.2** for the baseline comparisons (`reply-tracker.ts`, `index.ts`, `config.ts`, `ui/inline-message.ts`). Read in full at v0.7.0: `index.ts` regions 560-700, 709-772, 950-1180, 1390-1830; `broker/broker.ts` regions 283-300, 374-385, 420-470, 516-600; `reply-tracker.ts`; `config.ts`; `package.json`. Read at v0.10.1: `types.ts` (full), `format-context.ts` (full), `reply-tracker.ts` (full), `project-agent.ts:1-90`, `index.ts:873-1000 + 2000-2260`, `broker/client.ts:36-120`, `broker/broker.ts` (presence, send, mailbox, register), `broker/spawn.ts:20-280`, `CHANGELOG.md:1-90`, `package.json`.

### Sweep axes — what was walked, and what the walk cannot see

Stated explicitly because the prior edition of this file never named an axis and never used the term
*surface-driven sweep*, which made it impossible for a reader to tell which misses were possible.

**Axis 1 — commit-driven version-lag sweep (ran; 13 items).** `git log --oneline v0.9.2..v0.10.1` =
14 commits, every one dispositioned in the table below. Produced ICOM-035…ICOM-047.

**Axis 2 — adversarial re-audit of closing code (ran; 11 items).** Every prior `closed` claim was
re-read at HEAD against the tag, on the standing rule that a wrongly-closed item deletes a real defect
from the backlog. Produced ICOM-027…ICOM-034 and ICOM-048…ICOM-050 — including three defects sitting
*inside* the code that closed ICOM-022 and ICOM-002.

**Axis 3 — baseline census (ran; the headline correction).** In-tree `vX.Y.Z` citations counted per
crate rather than inherited, which moved the ported baseline from v0.7.0 to v0.9.2 and reclassified
six items out of version lag. This is README structural blind spot 3's prescribed counter, and it is
the reason the drift window above is `v0.9.2..v0.10.1` rather than a whole minor version wider.

**Axis 4 — surface-driven sweep over the ported baseline: NOT RUN.** This is the axis README
structural blind spot 1 prescribes — walk upstream itself and, for each exported symbol / event /
config key / protocol tag / tool action, ask "what in cyrup consumes this?". **Axes 1 and 3 are both
bounded by the drift window by construction, and axis 2 is bounded by what a prior pass happened to
close** — so nothing in this pass could have found behaviour that predates **v0.9.2** and was simply
never ported. That is not a theoretical concern here: the ported baseline was itself only established
as v0.9.2 *in this same pass*, so no pass has ever walked the v0.9.2 surface against `crates/`. Sized
and recorded as **blind spot 10**.

**Two cheap sub-axes of axis 4 were spot-checked during the 2026-08-12 repair pass; both came back
clean.** Recorded so the next pass does not re-run them:

- **Config keys.** `config.ts` @v0.9.2 declares 9 `IntercomConfig` members (`brokerCommand`,
  `brokerArgs`, `confirmSend`, `inboundTrigger`, `status`, `stableId`, `enabled`, `replyHint`, plus
  `DEFAULT_ASK_TIMEOUT_MS`/`getAskTimeoutMs`). `crates/cyrup-intercom/src/config.rs:33-48` models 7.
  The one behavioural absence is **`stableId`**, which is already **ICOM-011**. `brokerCommand`/
  `brokerArgs` are parsed for wire parity and documented as informational (cyrup re-execs
  `current_exe __intercom-broker` instead of shelling `npx tsx`) — mechanism, correctly stated
  in-tree at `config.rs:29-32`. **No new item.**
- **Env vars.** pi's six runtime variables at v0.9.2 are `PI_INTERCOM_{ASK_TIMEOUT_MS, NAME_POLL_MS,
  SESSION_ID, STABLE_ID, TCP, TRANSPORT}`. cyrup carries five under both the `CYRUP_` and `PI_`
  spellings (`identity.rs:20-24` plus `transport/`); the only absentee is `PI_INTERCOM_STABLE_ID`,
  again **ICOM-011**. (The other `INTERCOM_*` identifiers in pi's tree — `INTERCOM_DIR`,
  `INTERCOM_DIR_MODE`, `INTERCOM_RUNTIME_FILE_MODE`, `INTERCOM_PROTOCOL_VERSION`, `INTERCOM_TCP_HOST`
  and the four `INTERCOM_*_EVENT` names — are internal constants in `broker/paths.ts` and
  `extension-api.ts`, not environment variables; a name-grep that treats them as env vars will report
  a false gap.) **No new item.**

That both came back clean is mild evidence that axis 4 is not a large open seam — but it is two axes
out of six, and they are the two smallest.

### Version-lag sweep: `v0.9.2..v0.10.1`, commit by commit

`git -C pi-intercom diff --stat v0.9.2..v0.10.1` = 24 files, +2495/−700; `git log --oneline v0.9.2..v0.10.1` = 14 commits. **Every commit is accounted for:**

| commit | disposition |
|---|---|
| `c3543d6` + `fd30948` | → ICOM-036 |
| `f260df0` | → ICOM-038 |
| `25ffb96` | → ICOM-035 |
| `5d76146` | → ICOM-037 |
| `72309e0` | → ICOM-039 |
| `126875e` | → ICOM-040 + ICOM-041 (its mailbox-routing half is moot until ICOM-010) |
| `c7987b3` | → ICOM-042 |
| `633e782` | → ICOM-043; the `broker/protocol.ts` extraction in the same commit is a **pure refactor with no behavioural delta** — the validators moved, `isMessage`/`isSessionRegistration`/`isMessageReceipt`/`isSessionId` are byte-identical, and cyrup already ports them as serde guards |
| `c9675a5` | → ICOM-047; its tsx-resolution half is **mechanism-forced** (cyrup re-execs `current_exe __intercom-broker`, `transport/spawn.rs:130-140`) |
| `2ba9f53` | → ICOM-046 |
| v0.10.0 config `throw` | → ICOM-044 |
| v0.10.0 blocking-ask refusal | → ICOM-045 |
| `0685e19`, `8b189e8`, `30dcbdd` | release/version bumps, no behaviour |

### Repair-pass verification, 2026-08-12 — three defect classes checked, none found here

The completeness critique found three classes of defect in the sibling area file and named them as
classes rather than instances, so all three were checked against this file before concluding it
needed no item-level change. Recorded so the check is not redone.

- **Evidence resting on a commit hash instead of a two-sided read — none.** Every open item's
  `**upstream**` line cites a path and a line at a named tag. The three that do not cite a `.ts:`
  line are legitimate and were each inspected: **ICOM-004** cites a 513-line `SKILL.md` and its
  `package.json:26-28` declaration (a whole-file item; there is no line to cite); **ICOM-012**'s
  upstream *is* the citation census, which is the item; **ICOM-025**'s upstream is explicitly "not an
  upstream-behaviour question — only the assertion technique is wrong", and it names the correct
  in-crate technique at `tests/reconnect.rs:85-96`. Where commit hashes appear in this file they are
  in the version-lag **sweep table**, mapping a commit to the item that owns it, with the item
  carrying the file:line — which is the correct use of a hash.
- **Items that propose no work (trackers) — none.** Every open item has a concrete `**Fix**` naming
  files and functions and a `**Verify**` describing a test. No item's Fix is "defer", "track, do not
  build" or "n/a while tracking". The closest is **ICOM-025**, whose Fix is partly a *negative*
  instruction ("do **not** 'fix' `tests/reconnect.rs:299` or `tests/shared_human_lock.rs:265` — both
  are sound"), but it also names two concrete rewrites, so it is work. **This area's 44 are 44 items
  of backlog**, unlike area 12's 33, which contained 4 trackers.
- **IDs duplicating another area's item — none.** No `ICOM-` id appears in another area file. Every
  external reference is from `PARITY-GAPS.md` (`PB-19`/`PB-20`/`PB-21`, `VL-I1`…`VL-I6`, `UW-10`)
  routing *into* this area, which is the correct direction, or from `00-residual-ledger.md`'s
  test-defect cluster listing `ICOM-025`/`ICOM-026` as members. The cross-area *couplings* recorded
  under `### Cross-area handoffs` are genuine dependencies (a signature change in area 06/08, an
  alias length shared with area 09), not duplicate filings — the item stays here and the other area
  supplies a seam.

### Rejected with reason (do not re-derive)

No finding was fully refuted this pass, but **five were corrected** and one **evidence claim in each of two items was found false**. Recording both so the next pass does not restate the wrong version:

- **ICOM-027's headline is wrong and must not be restored.** "Every inbound intercom message is written to the session tree as a hidden custom message" is false: `AgentSession::inject_message` (`cyrup-session-svc/src/session.rs:3732-3767`) honours the caller's `display` only on the not-streaming, not-trigger branch (`:3762`); the trigger branch (`spawn_run`) and the streaming branch (`agent.steer`) both re-emit the Custom message through `cyrup-agent/src/agent.rs:454-455`/`:491-492` and it is persisted by `cyrup-session-svc/src/subscriber.rs:172-183` with `display` **hard-coded true**. With the default `inboundTrigger: "always"` an idle session replays fine. The item survives only for `inboundTrigger: "replies"`/`"never"` and for FollowUps in a flushed backlog — hence medium → low. The clause "even the live view is driven by a flag that says do not show me" is also false: the live path (`cyrup-tui/src/app/events_fold.rs:111-123`) ignores the flag.
- **ICOM-028's "nothing at all" impact is wrong.** The injected custom **message** is persisted and drawn by the TUI's built-in `[type] body` framing on the default path, so the human sees header, cwd, reply hint and body. The card is dead weight plus a noise status line. Medium → low.
- **ICOM-036 must not claim cyrup's schema promises prefix resolution.** "the short id shown in parentheses by 'list' (a leading ID prefix resolves)" is **upstream's** description text (v0.7.0 `index.ts:1459-1460`); cyrup's own schema (`tools/intercom.rs:391`) reads only "Target session name or id (send/ask/reply)." The impact chain runs through the printed `list` column instead.
- **ICOM-042 must not claim `grep -rni herdr crates/` returns zero.** It returns ~10 hits in `cyrup-ext-subagents` (`tui/fleet.rs:58-60`, `tui/fleet_overlay.rs:37`/`:263`/`:529`, `extension.rs:9861`, plus a test), where a deliberate Herdr-inspector divergence is already documented. The Herdr decision is not green-field.
- **ICOM-043 is incomplete as originally scoped.** Its "mechanical eight replacements" would leave `content_markdown` still divergent from the crate's own v0.9.2 baseline, which carries a `_deliveryMetadata_` segment cyrup omits entirely. Filed as ICOM-048; land the two together.
- **ICOM-035's fix sketch overstated the blocker.** A `HostServices` seam change is **not** strictly required — `AgentSession::inject_message` already routes to `agent.steer` when `is_streaming()` (`session.rs:3752-3754`).
- **ICOM-030's impact tail is not established.** "the parent may never see it / stalls to the 10-minute ask timeout" was not verified — `native_supervisor.rs`'s polling loop was not read. The certain harm (two competing supervisor mechanisms in a headless child) is what the item claims.
- **ICOM-007's "byte-for-byte" was an overstatement**, not a defect: `split_whitespace().join(" ")` trims where JS `replace(/\s+/g," ")` does not, and `.chars().take(80)` differs from `.slice(0,80)` on astral input. Not enough to reopen; noted so a future pass does not file it as new.
- **ICOM-013's `ToolError`-vs-text-result half is not a divergence** — pi's `tool_result` handler maps `details.error`/`delivered:false` to `isError: true`, which is what `ToolError` produces. Only the failure text differs. Do not re-file the mechanism.

### Corrections propagated to other docs

- `PARITY-GAPS.md:19` and this file's former header both record the ported baseline as **v0.7.0**. Both are stale — the crate targets **v0.9.2** (ICOM-012 carries the fix).
- `PARITY-GAPS.md` §1d **PB-21** ("session-name poll timer unported") = ICOM-006, confirmed. **PB-20** = ICOM-004, confirmed. **PB-19** = ICOM-015, now correctly **partial** (client half live, broker listen half absent).
- §2 **UW-10** ("intercom compose/session-picker overlays are render-only") is confirmed at HEAD — `handle_input` in `ui/compose.rs:86` and `ui/session_list.rs:75` have zero non-test callers, and `open_overlay` (`cyrup-ext/src/host/services.rs:224`) is never called from this crate. But the in-tree rationale at `ui/mod.rs:12-19` ("cyrup's native `InitApi` has no `register_message_renderer` / `register_shortcut`") is now **half stale**: `register_message_renderer` exists at `cyrup-ext/src/native.rs:270`. Correct it with ICOM-024 or ICOM-028.
- §3c **VL-I1…VL-I6** map to ICOM-010/017/017/016/011/018; all six confirmed still open.
- §4's "pi-intercom v0.9.2 interop" closure list was re-verified line by line and every claim in it holds.

### Cross-area handoffs

- **ICOM-029** needs a signature change in **area 06 (cyrup-ext)** and **area 08 (session-svc)**: `HostServices::inject_message` gains a `details` argument.
- **ICOM-035** may need the same two areas to expose an explicit steer delivery mode (optional — see its Fix).
- **ICOM-030**'s env var is written by **area 09 (cyrup-ext-subagents)**, `exec/mod.rs:1811`; the fix itself is entirely inside area 11.
- **ICOM-027** depends on **area 07 (cyrup-tui)**'s replay gate at `app/session_bind.rs:269`, which is a **correct** port of Pi and must NOT be changed — the fix belongs on the intercom side.
- **ICOM-040**'s alias length is coupled to **area 09**'s `orchestrator_presence_target`; changing one without the other breaks child→supervisor addressing.
- **ICOM-018** and **ICOM-042** share `cwd.rs`; port it once.

### Blind spots — what a divergence could still hide behind

1. **`broker/mod.rs` is 1559 lines and was read in ranges only.** Not compared against `broker/broker.ts`: the rate-limit path (`broker/ratelimit.rs`, grep only), `handle_list`, `handle_unregister`, `handle_cancel_ask`, the eviction cap, the trust metadata, the connection reader/writer tasks, and the signal handling. The v0.7.0 CHANGELOG names four hardening changes in this area — broker-owned local trust metadata, per-connection rate limiting, no-op presence coalescing, inbound broker frame-size cap — that **no item covers either way**. This is the single largest unswept surface in the crate.
2. **`transport/client.rs` (1268 lines) and `transport/framing.rs` (367 lines) were grepped, not read.** The framing state machine was rewritten upstream at v0.9.1 as a bounded reader ("removing quadratic `Buffer.concat` accumulation … up to ~28× faster") and cyrup's `framing.rs` was not compared against it — a correctness divergence on fragmented reads would be missed. `client.rs`'s `handleBrokerMessage` arm-for-arm coverage of the 16-tag union was taken on the strength of PARITY-GAPS §4 plus `tests/protocol_*.rs`, not re-derived.
3. **`ui/compose.rs`, `ui/session_list.rs` and `ui/mod.rs`'s width helpers were not diffed against `ui/compose.ts` / `ui/session-list.ts`.** UW-10 caps the blast radius (both overlays are unreachable in production), but a rendering or key-handling divergence inside them is unverified. The v0.9.1 inline-message render caching ("cached the collapsed preview and width-keyed wrapped body lines … ~2–3×") was not checked against `ui/inline_message.rs` at all.
4. **`project-agent.ts` was read only to line 90 of 324.** ICOM-042's cyrup-side absence is certain, but the upstream contract — the Herdr version gate, `waitForProjectSession`'s polling and abort semantics, the six `HerdrErrorCode` mappings — is characterised from exports and the CHANGELOG. Its **L** effort estimate is correspondingly soft.
5. **`broker/extension-state.ts` (186 lines) and `extension-api.ts` (44 lines) were not read.** ICOM-016 rests on cyrup's verified absence plus upstream's file inventory; its fix sketch is directional, not a design.
6. **No build, no test run, no clippy, no npm.** Every claim is static. ICOM-023's race, ICOM-025's flakiness, ICOM-038's half-open-socket scenario and ICOM-049's replacement window are argued from code shape and cannot be observed here; ICOM-027's replay consequence is derived from `app/session_bind.rs:269` and `session.rs:3762`, not from a resumed session.
7. **The `pi` core side of the seam contracts was not re-read.** ICOM-027/ICOM-029/ICOM-035 assert what cyrup's host seam can and cannot carry, verified against `cyrup-ext` and `cyrup-session-svc`; pi's `sendMessage` / `deliverAs` contract in `pi/packages/coding-agent` was **not** checked to confirm that upstream's `"steer"` means what `AgentSession::steer` means. If it does not, ICOM-035's fix sketch is wrong even though the gap is real.
8. **`tools/contact_supervisor.rs` was read only to line 60.** ICOM-030 and ICOM-045 both touch it; the rest of the file (validation, structured-reply parsing, its `:586-664` assertions) was not diffed against upstream's `contact_supervisor` handler at `index.ts:1390-1600`.
9. **Resolved this pass, recorded so it is not re-opened as a question:** whether one `IntercomExtension` instance can see two `SessionStart` events. It can — upstream's `turn_start` handler (v0.7.0 `index.ts:1118-1124`) has an explicit `if (!currentSessionId || sessionId !== currentSessionId) { startSessionRuntime(ctx); … }` branch. ICOM-032 and ICOM-049 both rest on that and keep their severities.

10. **NEW (2026-08-12 repair pass) — no surface-driven sweep has ever been run against the ported
   baseline, and the two axes that *were* run cannot see past the drift window.** All 24 items filed
   in this pass came from a commit-driven sweep of `v0.9.2..v0.10.1` (13) or from re-auditing the
   code that closed three prior items (11). Both are bounded: the first by the 14 commits in the
   window, the second by what a previous pass happened to close. **Neither can surface a symbol that
   existed at v0.9.2 and was never ported** — and because the ported baseline was only *established*
   as v0.9.2 in this same pass, no pass has ever walked that surface. The un-run enumeration, sized:
   **68 top-level `export` declarations across the 17 non-test `.ts` files at v0.9.2** (`types.ts`
   16 of them — 12 exports whose unions carry the 16-tag `BrokerMessage` vocabulary — `broker/paths.ts`
   16, `extension-api.ts` 7, `config.ts` 6, `broker/spawn.ts` 8, the rest 1–3 each), **plus** the
   `intercom` tool's action enum, the 8 subscribed event kinds, and the actions `SKILL.md` documents.
   Two of the cheapest sub-axes (config keys, env vars) were spot-checked in the repair pass and both
   reduce to the already-filed **ICOM-011**, which is mild evidence the seam is not large — but they
   are the two smallest, and the largest, `broker/broker.ts` and `broker/client.ts`, coincide exactly
   with blind spots 1 and 2 (read in ranges / grepped only). **The highest-yield next action for this
   area is axis 4 restricted to `broker/`,** because that is where the un-swept surface and the
   un-read code overlap.

11. **NEW (2026-08-12 repair pass) — "0 critical, 0 high" has never been tested against the
   severity definition, only inherited.** README:106-107 defines `critical` as data loss, silent
   wrong output, a permission bypass, or a crash on a normal path. Several items here describe silent
   loss on their own text — **ICOM-008** (`ask` discards an advertised `replyTo`, so the send is
   rejected by cyrup's own broker), **ICOM-046** (`reply` silently drops attachments), **ICOM-050**
   (the audit entry drops `messageId` and `attachments`), **ICOM-027** (content persisted with
   `display=false` vanishes on replay under two of three `inboundTrigger` settings). Each was rated on
   *blast radius* — all four are bounded to the intercom subsystem and none corrupts a session
   transcript or bypasses a permission — and the repair pass did **not** re-rate them, because
   re-rating on a definition the whole ledger applies unevenly would make this file inconsistent with
   its siblings rather than more correct. **Recorded as a blind spot rather than acted on:** if the
   ledger tightens the severity rule (the critique's finding 3 proposes exactly that), these four are
   the rows in this file that must be re-examined first, and `ICOM-046`/`ICOM-050` are the two whose
   text most directly matches "data loss".
