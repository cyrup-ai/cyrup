---
stage: new
status: done
updated: 2026-08-22 19:32
---

# Split host_services.rs Into A Module Directory

**Crate:** `crates/cyrup-session-svc` · **Severity:** medium · **Effort:** medium

## Description

`src/host_services.rs` is the crate's largest file at 3050 lines and 38.5% of it (L1877-3050) is an inline `mod tests` holding 13 tests that `src/tests/mod.rs` cannot see — so the crate runs two test-placement conventions with no stated reason, even though src/tests/mod.rs documents itself as the place tests were "relocated from tests/ so the whole crate's tests build and run as ONE binary". A reader opening the file to change one capability grant scrolls past ~380 lines of wire and attach types (UiKind:68 through UiEffectSink:225, SessionActivity:277/SessionCatalog:299/ThemeAccess:341, EditorTextMirror:403, builtin_tool_source_info:436, tree_node_to_json:453, InjectMessage:479) before reaching `struct LiveHostServices`:500, and the `impl HostServices` block at L955-1869 already carries nine grant banners that the file layout does not honour. The test half is self-partitioned by its own banners at 2463, 2642, 2750 and 2901. Extraction was checked: the only `super::` use in the test module is the bare glob and all 38 distinct receivers are public, with one exception — `with_exec_timeout` at :667-669 is a private `#[cfg(test)] fn` called at :2066 and must become `pub(crate)` for the move to compile. `impl HostServices` must stay in one file; do not delegate-split its 64 mostly-4-to-8-line bodies.

## Acceptance Criteria

- [ ] `src/host_services.rs` is replaced by `src/host_services/` containing `mod.rs`, `ui.rs`, `attach.rs`, `json.rs` and `inject.rs`; no file in the directory exceeds 1500 lines and `rg -c 'mod tests' crates/cyrup-session-svc/src/host_services/mod.rs` returns 0.
- [ ] The old L1877-3050 test module is relocated into `src/tests/` split along its own four banners (core, guest introspection, session read-only view, provider OAuth callbacks, custom seam), each registered in `src/tests/mod.rs`.
- [ ] `with_exec_timeout` is `#[cfg(test)] pub(crate) fn` and the relocated timeout test compiles against it.
- [ ] `git diff src/lib.rs` shows the `pub use host_services::{…}` group at lines 66-69 byte-identical.
- [ ] `cargo test -p cyrup-session-svc` still reports 311 passing (the 13 relocated tests included) and `cargo clippy -p cyrup-session-svc --all-targets` gains no warnings.
- [ ] `impl HostServices for LiveHostServices` remains a single unsplit block in `mod.rs`.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Split src/host_services.rs (3050 lines) into a host_services/ module dir; 38% of it is an inline test mod

`OVERSTATED` · severity **medium** · effort **medium** · dimension `large-files`

**Evidence.** /home/user/cyrup/crates/cyrup-session-svc/src/host_services.rs is 3050 lines (`wc -l`). Top-level items: UI wire types L68-225 (UiKind:68, UiReply:80, UiRequest:91, UiSink:120, OverlayRequest:131, OverlaySink:140, UiEffect:148, UiEffectSink:225); private LiveSnapshot:229; attach traits SessionActivity:277, SessionCatalog:299, ThemeAccess:341; helpers EditorTextMirror:403, builtin_tool_source_info:436, tree_node_to_json:453, InjectMessage:479, InjectSink:497; `struct LiveHostServices`:500; inherent impl L623-954; `impl HostServices for LiveHostServices` L955-1869 (64 fns, 9 grant banners at 956/1094/1165/1217/1362/1477/1508/1589/1642); `impl ActiveToolNames`:1870; `#[cfg(test)]`:1877 + `mod tests {`:1878 running to EOF at 3050 = 1174 lines / 38.5%, holding 13 tests, self-partitioned by banners at 2463 (EXT-037/038 guest introspection), 2642 (session read-only view), 2750 (provider OAuth callbacks), 2901 (TUI-030 custom seam). Destination exists and is conventional: src/tests/mod.rs declares 48 sibling files ("relocated from tests/ so the whole crate's tests build and run as ONE binary"), including native_host_services.rs. Public surface is pinned by src/lib.rs:66-69 `pub use host_services::{ControlSink, EditorTextMirror, InjectMessage, InjectSink, LiveHostServices, OverlayRequest, OverlaySink, ThemeAccess, UiEffect, UiEffectSink, UiKind, UiReply, UiRequest, UiSink};`.

**Why it matters.** It is the crate's largest file and 38.5% of it is tests that are invisible to src/tests/mod.rs, so the crate runs two test-placement conventions with no stated reason. A reader opening it to change one capability grant scrolls past ~380 lines of plain wire/attach types before reaching the struct, and past the whole test half if they scroll from the bottom. The trait impl's own 9 grant banners are documented seams the file layout does not honour. Not urgent — nothing is broken — but it is the single largest legibility win available in this crate now that session.rs is done.

**Fix.** Convert to `src/host_services/mod.rs` + siblings: `ui.rs` <- L68-225 + EditorTextMirror L403-435; `attach.rs` <- L277-402 (SessionActivity/SessionCatalog/ThemeAccess — see the visibility finding, settle it in this move); `json.rs` <- L436-478 (builtin_tool_source_info, tree_node_to_json — keep `pub(crate)`); `inject.rs` <- L479-499. mod.rs keeps LiveSnapshot, the struct, the inherent impl and the single `impl HostServices` block — do NOT try to split one trait impl across files (Rust forbids it) and do not delegate-split its 64 mostly-4-to-8-line bodies. Then move L1877-3050 to src/tests/ cut along its own four banners (host_services_core.rs L1877-2462, host_services_introspection.rs L2463-2641, host_services_session_view.rs L2642-2749, host_services_oauth.rs L2750-2900, host_services_custom_seam.rs L2901-3050) and register each in src/tests/mod.rs. REQUIRED alongside the move: change host_services.rs:668 `fn with_exec_timeout` to `pub(crate) fn` (keep the `#[cfg(test)]`), otherwise host_services_core.rs will not compile. src/lib.rs:66-69 stays byte-identical. Net: mod.rs ~1450 lines.

**Verifier correction.** Every structural measurement holds; only the severity and one line of the fix are wrong. Corrected: severity high -> medium (this is a mechanical relocation of already-cohesive clusters in a crate that currently compiles clean with zero warnings; nothing is broken, no correctness risk). Corrected facts: the test mod is L1877-3050 = 1174 lines (38.5%), 13 tests, banners at exactly 2463/2642/2750/2901 as claimed; `impl HostServices` L955-1869 has 64 `fn` items, not 65. The fix's claim "No privacy changes needed" is FALSE: host_services.rs:667-669 is `#[cfg(test)] fn with_exec_timeout(...)` — a PRIVATE associated fn — and it is called at host_services.rs:2066 by `exec_with_no_timeout_ms_still_gets_killed_by_the_fallback_ceiling`. Moving that test to src/tests/ requires bumping it to `#[cfg(test)] pub(crate) fn`. Everything else the test mod reaches is genuinely public: I extracted L1877-3050 and grepped it — the only `super::` use is the bare `use super::*`, and all 38 distinct `svc.*` receivers are `pub` inherent methods or `HostServices` trait methods.
