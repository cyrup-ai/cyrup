---
stage: new
status: done
updated: 2026-08-22 19:32
---

# Move builder.rs's Free-Function Tail And Test Module Out

**Crate:** `crates/cyrup-session-svc` · **Severity:** medium · **Effort:** medium

## Description

`src/builder.rs` is 2767 lines, the crate's second-largest file, and roughly 990 of them have no dependence on `SessionBuilder`'s state: after `impl SessionBuilder` closes at L1772 there is a 568-line tail of 20 free items (settings parsing L1776-1809; model resolution L1810-2008 with `ResolvedModel`, `resolve_model`, `fallback_model`; `tool_contribution`:2009; `pre_trust_extension_verdict`:2047; natives/packages/extensions L2092-2343) followed by a 424-line `#[cfg(test)] mod tests` (L2344-2767, 15 tests) that exercises exactly those clusters. Related helpers are also split across 1700 lines — `DEFAULT_BUILTIN_TOOLS`:313, `ALL_BUILTIN_TOOLS`:321, `select_active_tools`:326, `apply_http_proxy_settings`:296 and `http_proxy_overlay`:301 sit above `pub struct SessionBuilder`:374 while `tool_contribution` sits at 2009. Six of the tail items are not private: `parse_queue_mode`, `parse_transport`, `thinking_level_to_str`, `thinking_level_from_str` and `tool_contribution` are `pub(crate)` and reached by path from `src/session/thinking.rs:67,71,95,121`, `src/session/mod.rs:328-329`, `src/session/control.rs:268`, `src/tools.rs:54,374` and `src/host_services.rs:2530`, and `extension_discovery_roots`:2165 is `pub`. Unlike the build() decomposition this needs no data-flow analysis, and it is the precondition that makes that refactor reviewable.

## Acceptance Criteria

- [ ] `src/builder.rs` no longer exists; `src/builder/` contains `mod.rs` plus `model.rs`, `natives.rs`, `packages.rs`, `tools.rs` and `settings_parse.rs`, with `builder/mod.rs` under 1800 lines.
- [ ] `builder/mod.rs` carries `pub(crate) use` re-exports for `parse_queue_mode`, `parse_transport`, `thinking_level_to_str`, `thinking_level_from_str` and `tool_contribution` so every existing `crate::builder::<name>` call site compiles unchanged — verified by `git diff` touching none of src/session/thinking.rs, src/session/mod.rs, src/session/control.rs, src/tools.rs or src/host_services.rs call lines.
- [ ] `git diff src/lib.rs` shows the `pub use builder::{…}` group at lines 47-50 byte-identical.
- [ ] Each `#[cfg(test)]` fragment of the old L2344-2767 module travels with the cluster it exercises (the tests for `natives_to_load`, `native_survives_no_extensions`, `fallback_model`, `configured_packages_from_settings` and `apply_http_proxy_settings` stay in-module because those items are private).
- [ ] `cargo test -p cyrup-session-svc` still reports 311 passing and `cargo clippy -p cyrup-session-svc --all-targets` gains no warnings.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Move builder.rs's 568-line free-function tail and its 424-line inline test mod into sibling modules

`CONFIRMED` · severity **medium** · effort **medium** · dimension `large-files`

**Evidence.** /home/user/cyrup/crates/cyrup-session-svc/src/builder.rs — `impl SessionBuilder` closes at L1772, then: settings parsing L1776-1809 (parse_queue_mode:1776, parse_transport:1785, thinking_level_to_str:1795, thinking_level_from_str:1804); model resolution L1810-2008 (ResolvedModel:1810, resolve_model:1838, fallback_model:1964); tool_contribution:2009; pre_trust_extension_verdict:2047; natives/packages/extensions L2092-2343 (SUBAGENT_CHILD_ENV:2092, SUBAGENT_CHILD_RUNTIME_NATIVES:2110, native_survives_no_extensions:2125, natives_to_load:2140, extension_discovery_roots:2165, load_installed_packages:2194, configured_packages_from_settings:2221, ext_mode:2295, read_discovered_prompt:2326, today:2340); then `#[cfg(test)] mod tests` L2344-2767 (424 lines, 15 tests) which opens with `use super::http_proxy_overlay;` and exercises exactly those clusters. Separately, DEFAULT_BUILTIN_TOOLS:313, ALL_BUILTIN_TOOLS:321 and select_active_tools:326 sit ABOVE `pub struct SessionBuilder`:374, ~1700 lines from the tool cluster below; apply_http_proxy_settings:296 and http_proxy_overlay:301 sit there too.

**Why it matters.** Cohesive helper families with no dependency on SessionBuilder's state, sitting in the crate's second-largest file purely because that is where they were written, and split across the file (tool selection at 313-368, tool contribution at 2009). Unlike the build() decomposition this needs no data-flow analysis — they are free functions with explicit signatures — and it removes ~990 lines from builder.rs on its own, which is the precondition that makes attempting build() safe.

**Fix.** Create `src/builder/` (shared with finding #2): `builder/model.rs` <- L1795-2008; `builder/natives.rs` <- L2047-2092 (pre_trust_extension_verdict) + L2092-2193 + L2295-2325; `builder/packages.rs` <- L2194-2294; `builder/tools.rs` <- L313-368 + L2009-2046; `builder/settings_parse.rs` <- L296-312 + L1776-1794. In `builder/mod.rs` add `pub(crate) use` re-exports for parse_queue_mode, parse_transport, thinking_level_to_str, thinking_level_from_str and tool_contribution so the existing `crate::builder::<name>` call sites in src/session/thinking.rs, src/session/mod.rs, src/session/control.rs, src/tools.rs and src/host_services.rs keep compiling, plus `pub use` for extension_discovery_roots (re-exported at src/lib.rs:48). Carry each `#[cfg(test)]` fragment of the L2344-2767 mod along with the cluster it exercises — natives_to_load, native_survives_no_extensions, fallback_model, configured_packages_from_settings and apply_http_proxy_settings are private, so those tests must stay in-module rather than moving to src/tests/.
