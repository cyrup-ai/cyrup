---
stage: exec
status: done
updated: 2026-08-22 23:21
---

# Trim The Manifest And Repair Its Stale Citations

**Crate:** `crates/cyrup-session-svc` · **Severity:** low · **Effort:** small

## Description

`Cargo.toml` declares `futures = { workspace = true }` at line 40 under `[dependencies]` although the only non-test mention of futures in `src/` is the English word in a doc comment at `src/session/control.rs:263` — all 11 real uses are `use futures::StreamExt;` inside `src/tests/`, which the `[dev-dependencies]` twin at line 49 already serves (verified: deleting line 40 leaves `cargo check` clean). The 19-line `[dev-dependencies]` block is then 10/14 noise: tokio, futures, async-trait, serde_json, cyrup-core, cyrup-agent, cyrup-ext, cyrup-session, cyrup-config and cyrup-tools (lines 47-57) byte-duplicate their `[dependencies]` twins, so the genuinely test-only entries (tempfile, cyrup-provider with `features = ["faux"]`, image, base64) are buried and cyrup-ext/cyrup-tools read as test-relevant. Separately, five comment citations point at paths that no longer exist — this crate has no `tests/` directory at all: `Cargo.toml:18` and `src/session/accessors.rs:294` cite `tests/wasm_slash_command.rs` (now `crates/cyrup-it/tests/session_svc/wasm_slash_command.rs`, across a crate boundary), `Cargo.toml:59` cites `tests/read_image_auto_resize.rs`, `src/session/mod.rs:50` cites `tests/delete_session_file_trash.rs`, and `src/builder.rs:2451` cites `tests/build_containment_and_flag_diagnostics.rs` (all three now under `src/tests/`); `Cargo.toml:35` cites `cyrup-ext/Cargo.toml:29`, which is `blake3`, not `tracing`. Two of these comments are the stated justification for a dev-dependency and for a default-on feature flag.

## Acceptance Criteria

- [ ] `[dependencies]` no longer contains `futures`, and `[dev-dependencies]` is exactly five entries: futures, tempfile, cyrup-provider (features = ["faux"]), image, base64.
- [ ] `cargo test -p cyrup-session-svc --no-run` finishes clean and `git diff Cargo.lock` is empty.
- [ ] `Cargo.toml:18` cites `crates/cyrup-it/tests/session_svc/wasm_slash_command.rs`, the read_image comment cites `src/tests/read_image_auto_resize.rs`, and the tracing comment cites the crate without a line number.
- [ ] `src/session/accessors.rs:294`, `src/session/mod.rs:50` and `src/builder.rs:2451` cite paths that exist; `rg -n '(^|[^/])\btests/[a-z0-9_]+\.rs' crates/cyrup-session-svc/src crates/cyrup-session-svc/Cargo.toml` returns zero hits.
- [ ] Every path named in a surviving comment resolves — verified by running `ls` on each cited path in the PR description.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Drop the unused `futures` entry from [dependencies] — it is test-only

`OVERSTATED` · severity **low** · effort **small** · dimension `manifest`

**Evidence.** crates/cyrup-session-svc/Cargo.toml:40 `futures = { workspace = true }` under [dependencies]. Non-test src has no `futures::` use — only the prose at src/session/control.rs:263. Test-only uses are served by the [dev-dependencies] twin at Cargo.toml:49. Verified: `sed -i '40d'` then `cargo check -p cyrup-session-svc` → `Finished dev profile`.

**Why it matters.** The lib's declared dependency surface overstates what the non-test crate links. A reader auditing the facade's real coupling is misled, and a future `futures::` use in src/ would look pre-sanctioned when it never has been.

**Fix.** Delete line 40 (`futures = { workspace = true }`) from [dependencies]. Keep the [dev-dependencies] entry at line 49 — removing both breaks the test build. Land together with the dev-dependencies trim below as a single manifest commit.

**Verifier correction.** Every factual claim reproduced exactly; only the severity is inflated. I re-ran the roll-call: the sole `futures` hit under src/ outside src/tests/ is the English word in the doc comment at src/session/control.rs:263. All 11 real uses are `use futures::StreamExt;` inside src/tests/ (agent_settled.rs:31, round9_l5res.rs:33, compact_refusals.rs:31, fork_non_persisted.rs:34, summarization_retry_events.rs:30, integration.rs:29, native_host_services.rs:31, round8_postrun.rs:21, round2.rs:247, abort_settles.rs:39, base_system_prompt.rs:23), and src/lib.rs:38-39 gates that whole tree behind `#[cfg(test)] mod tests;`. I deleted Cargo.toml:40 and `cargo check -p cyrup-session-svc` finished clean, then restored. Corrected scope: this is a declaration-accuracy fix with zero build-graph impact — `futures` is already in the graph transitively via cyrup-ext and cyrup-agent, so nothing stops being compiled. Severity low, not medium. It is also the same one-line edit as the dev-dependencies finding and should land as one change, not two.

### Delete the 10 [dev-dependencies] entries that byte-duplicate [dependencies]

`OVERSTATED` · severity **low** · effort **small** · dimension `manifest`

**Evidence.** crates/cyrup-session-svc/Cargo.toml:47-57 vs 26-44 — 10 identical `{ workspace = true }` restatements. Cargo already merges [dependencies] into test targets. Verified by deleting all 10 (keeping `futures` once) and running `cargo test -p cyrup-session-svc --no-run` → `Finished test profile`.

**Why it matters.** A 19-line dev-dep block that is 10/14 noise defeats the block's only purpose: telling a reader which deps exist solely for tests. Today cyrup-ext and cyrup-tools read as test-relevant when they are core deps, while the genuinely test-only tempfile/image/base64/faux are buried among them.

**Fix.** Remove lines 47 and 49-57 from [dev-dependencies], moving `futures` down from [dependencies] so it appears there exactly once. Resulting block: futures, tempfile, cyrup-provider {features=["faux"]}, image, base64 — five entries, each with a reason to exist. Re-verify with `cargo test -p cyrup-session-svc --no-run`.

**Verifier correction.** Evidence reproduced verbatim; severity is one notch high. Cargo.toml:47-57 does list tokio(47), futures(49), async-trait(50), serde_json(51), cyrup-core(52), cyrup-agent(53), cyrup-ext(54), cyrup-session(55), cyrup-config(56), cyrup-tools(57), each `{ workspace = true }` and byte-identical to its [dependencies] twin in lines 26-44. I deleted line 46(tokio) and 49-56 in the post-futures-removal numbering — i.e. the 9 net duplicates, keeping `futures` once — and `cargo test -p cyrup-session-svc --no-run` finished clean, then restored. The four informative entries are confirmed: tempfile(48) has no normal twin, cyrup-provider(59) adds `features = ["faux"]`, image(63) and base64(64) each carry a justifying comment. Corrected scope: pure manifest readability, no build or lockfile effect whatsoever — severity low. Also note this is the same edit as the `futures` finding; treat them as one task.

### Fix three stale file/line citations inside the manifest's own rationale comments

`CONFIRMED` · severity **low** · effort **small** · dimension `manifest`

**Evidence.** crates/cyrup-session-svc/Cargo.toml:35 cites `cyrup-ext/Cargo.toml:29` (actually blake3; tracing is at :35). Cargo.toml:18 cites `tests/wasm_slash_command.rs` (no tests/ dir in this crate; real path crates/cyrup-it/tests/session_svc/wasm_slash_command.rs). Cargo.toml:60 cites `tests/read_image_auto_resize.rs` (real path crates/cyrup-session-svc/src/tests/read_image_auto_resize.rs, base64 at :24, image at :56).

**Why it matters.** These comments are the crate's rationale record and are unusually load-bearing; a reader who follows any of the three lands on the wrong line or a nonexistent path and has to re-derive the reasoning the comment exists to preserve. The two `tests/` references in particular imply an integration-test layout this crate deliberately abandoned.

**Fix.** Cargo.toml:35 — cite the crate without a line number (`matching cyrup-ext`), which cannot drift again; if the tracing hoist lands, this comment disappears entirely. Cargo.toml:18 — change to `crates/cyrup-it/tests/session_svc/wasm_slash_command.rs`. Cargo.toml:60 — change `tests/read_image_auto_resize.rs` to `src/tests/read_image_auto_resize.rs`. Bundle with the other manifest edits.

### Fix five stale `tests/…` path references left by the migration into `src/tests/`

`CONFIRMED` · severity **low** · effort **small** · dimension `test-organisation`

**Evidence.** `crates/cyrup-session-svc/Cargo.toml:18` — "the full end-to-end (see tests/wasm_slash_command.rs)", justifying the default-on `wasm-host` feature; file is now `crates/cyrup-it/tests/session_svc/wasm_slash_command.rs`. `crates/cyrup-session-svc/Cargo.toml:59` — "`tests/read_image_auto_resize.rs` builds a real >2000px PNG fixture…", justifying the `image`/`base64` dev-dependencies; file is now `src/tests/read_image_auto_resize.rs`. `crates/cyrup-session-svc/src/session/accessors.rs:294` — "`tests/wasm_slash_command.rs`: `prompt("/greet …")` → …" (now in `cyrup-it`). `crates/cyrup-session-svc/src/session/mod.rs:50` — "`tests/delete_session_file_trash.rs` names this through `crate::session::trash_args`" (now `src/tests/`, and the claim itself still holds at `src/tests/delete_session_file_trash.rs:34`). `crates/cyrup-session-svc/src/builder.rs:2451` — "`tests/build_containment_and_flag_diagnostics.rs`'s …" (now `src/tests/`).

**Why it matters.** These comments exist specifically so a reader can jump to the test that proves the claim — two of them are the stated justification for a dev-dependency and for a default-on feature flag. A path that resolves to nothing costs a grep and teaches the reader that this crate's comments are unmaintained, which discounts the many that are accurate. Two of the five now cross a crate boundary, which a reader will not guess from `tests/`.

**Fix.** Rewrite the three intra-crate references to `src/tests/…` (`Cargo.toml:59`, `src/session/mod.rs:50`, `src/builder.rs:2451`) and the two that migrated out to the workspace-relative `crates/cyrup-it/tests/session_svc/wasm_slash_command.rs` (`Cargo.toml:18`, `src/session/accessors.rs:294`). Fold into the finding-2 refile pass, which will invalidate more of these paths anyway.
