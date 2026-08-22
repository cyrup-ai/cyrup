---
stage: new
status: done
updated: 2026-08-22 19:32
---

# Record The Formatting Policy And Normalise Import Ordering

**Crate:** `crates/cyrup-session-svc` · **Severity:** low · **Effort:** small

## Description

There is no `rustfmt.toml` or `.rustfmt.toml` anywhere in the workspace, no CI, and no script that runs `cargo fmt` — yet the tree is hand-formatted to a wider style than stock rustfmt: `cargo fmt -p cyrup-session-svc -- --check` reports 1,060 diff hunks with not one file clean, while p99 line length in host_services.rs is 105 and only 12 non-test lines exceed 120 characters. A measured best fit (`max_width = 110`, `use_small_heuristics = "Max"`) drops that to 495 hunks. The absence of a recorded decision is the debt: anyone with format-on-save silently produces a 500-1,000 hunk reformat that buries their real change, and nothing tells a reviewer that lines like `src/session/adapters.rs:41` are deliberately hand-kept. The same task settles the crate's second ordering split: everything under `src/session/` uses edition-2024 version-sort (uppercase-first) in mixed-case brace lists, while nine lines elsewhere still use the pre-2024 lowercase-first order — `src/subscriber.rs:17` and `:20`, `src/lib.rs:118` and `:122`, `src/runtime.rs:16`, `src/host_services.rs:26`, plus `src/builder.rs:2733`, `src/bash.rs:716` and `src/tools.rs:313` in inline test modules — and `src/factory.rs:16` sits alone in a blank-line-separated block between the external group at :11-14 and the crate group at :18-21.

## Acceptance Criteria

- [ ] A workspace-root `rustfmt.toml` exists with `max_width = 110` and `use_small_heuristics = "Max"`, plus a header comment stating the tree is hand-formatted and `cargo fmt` is advisory, not a gate.
- [ ] The PR records the measured `cargo fmt -- --check` hunk count for every workspace crate before and after the config, not just cyrup-session-svc (1,060 → 495), since the config is workspace-scoped.
- [ ] All nine lowercase-first brace lists are uppercase-first and `rg -n 'use .*\{[a-z_]+, [A-Z]' crates/cyrup-session-svc/src` returns zero hits.
- [ ] `src/factory.rs:16` is merged into the external-crate group at :11-14 and the stray blank-line block is gone.
- [ ] No blanket reformat is included: `git diff --stat` touches at most 12 files, and `cargo check -p cyrup-session-svc` passes.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Normalise `use`-brace item ordering: the older top-level files still use pre-2024 sort, the new session/ modules use edition-2024 version sort

`OVERSTATED` · severity **low** · effort **small** · dimension `consistency`

**Evidence.** Old lowercase-first order outside src/session/: src/subscriber.rs:17 `use tokio::sync::{mpsc, Mutex as AsyncMutex};`, src/subscriber.rs:20 `use crate::event::{agent_message_to_core, core_message_to_agent, AgentSessionEvent};`, src/lib.rs:118 `use cyrup_session::layout::{encode_cwd, SessionLayout, SessionsRoot};`, src/lib.rs:122 `use cyrup_session::git_paths::{find_git_paths, GitPaths};`, src/runtime.rs:16 `use tokio::sync::{watch, RwLock};`, src/host_services.rs:26 `use serde_json::{json, Value};`, plus src/builder.rs:2733, src/bash.rs:716, src/tools.rs:313 in inline test modules. New edition-2024 version-sort (uppercase-first) in all 10 mixed-case brace lists under src/session/ (forking.rs:18, compaction.rs:19, auto_compaction.rs:17 and :20, retry.rs:9, bash.rs:14 and :18, inject.rs:13, control.rs:17). Workspace is edition 2024 (Cargo.toml:88), so style_edition 2024 version-sort is the rustfmt default. Separately src/factory.rs:16 sits in its own blank-line-separated block between the external group (:11-14) and the crate group (:18-21).

**Why it matters.** Two orderings in one crate mean 'is this import list sorted?' stops being a usable review signal, and whoever next runs `cargo fmt` on a touched file gets an unrelated reordering in the diff. The std / external / crate:: / super:: grouping is otherwise a genuine crate-wide norm, so these nine lines plus factory.rs:16 are the only exceptions.

**Fix.** Reorder all nine brace lists to uppercase-first (`{Mutex as AsyncMutex, mpsc}`, `{AgentSessionEvent, agent_message_to_core, core_message_to_agent}`, `{SessionLayout, SessionsRoot, encode_cwd}`, `{GitPaths, find_git_paths}`, `{RwLock, watch}`, `{Value, json}`, `{SessionConfig, fallback_model}`, `{BashResult, bash_message_payload}`, `{Value, json}`) — note src/lib.rs and src/subscriber.rs:20 were missed by the original finding — and merge src/factory.rs:16 into the external-crate group at :11-14.

**Verifier correction.** The direction is right but the count is wrong, and the fix as written is incomplete. The old/new split IS clean — I grepped src/session/ for lowercase-first mixed-case brace lists and got zero hits, and all 10 new-style lines cited are indeed under src/session/. But there are NOT 'exactly four' old-style lines outside src/session/. There are six in non-test code — src/subscriber.rs:17 AND src/subscriber.rs:20 `{agent_message_to_core, core_message_to_agent, AgentSessionEvent}`, src/lib.rs:118 `{encode_cwd, SessionLayout, SessionsRoot}`, src/lib.rs:122 `{find_git_paths, GitPaths}`, src/runtime.rs:16, src/host_services.rs:26 — plus three more inside inline `mod tests` blocks (src/builder.rs:2733 `{fallback_model, SessionConfig}`, src/bash.rs:716 `{bash_message_payload, BashResult}`, src/tools.rs:313 `{json, Value}`). The finding missed src/lib.rs entirely, which is the crate's public facade. Nine lines, not four, so the listed five-edit fix leaves the inconsistency in place. The src/factory.rs grouping defect is real as described (:11-14 external group, blank line, then a lone `use cyrup_config::trust::TrustStore;` at :16 before the crate:: group at :18).

### Record the formatting decision: there is no rustfmt.toml and stock `cargo fmt` would rewrite all 86 files

`CONFIRMED` · severity **low** · effort **small** · dimension `consistency`

**Evidence.** No rustfmt.toml/.rustfmt.toml anywhere in the workspace; no .github/workflows; no `cargo fmt` invocation in xtask or any script. `cargo fmt -p cyrup-session-svc -- --check` = 1,060 diff hunks, not one file clean. Best-fit search reproduced: max_width=110 + use_small_heuristics="Max" = 495 hunks. p99 line length 105 in host_services.rs; 12 non-test lines over 120 chars. src/session/adapters.rs:41 keeps `self.0.upgrade().map(|s| s.slash_command_catalog()).unwrap_or_default()` on one line where stock rustfmt breaks the chain.

**Why it matters.** The absence of a recorded decision is the debt, not the formatting. Any contributor with format-on-save, or anyone who runs `cargo fmt` once, silently produces a 500-1,000 hunk reformat that buries the real change in review, and nothing in the repo warns them off. Conversely nothing tells a reviewer that the hand formatting is deliberate, so it reads as neglect.

**Fix.** Do NOT blanket-reformat. Add a workspace-root rustfmt.toml with `max_width = 110`, `use_small_heuristics = "Max"`, and a header comment stating that the tree is hand-formatted and `cargo fmt` is advisory rather than a gate. Before landing, measure the hunk delta for the other workspace crates too, since the config is workspace-scoped and only cyrup-session-svc was measured. If the team would rather converge, the same config makes a one-time `cargo fmt --workspace` the smallest such commit — land it alone.
