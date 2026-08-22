---
stage: new
status: done
updated: 2026-08-22 19:32
---

# Close The Drift Between Public Signatures And lib.rs Re-exports

**Crate:** `crates/cyrup-session-svc` · **Severity:** medium · **Effort:** small

## Description

`src/lib.rs:90` states the crate's contract — "Load-bearing re-exports so embedders need not depend on every subsystem directly" — and the hand-curated list has drifted in both directions with no compiler check. Types in public signatures that the facade cannot name: `UsageCostBreakdownEntry` (returned by session/stats.rs:34, defined state.rs:159, while every sibling state.rs type is re-exported at lib.rs:81-84), `cyrup_provider::cache_stats::CacheWasteTotals` (session/stats.rs:45), `cyrup_core::Tool` (public field builder.rs:177), and `BeforeSessionInvalidate` (runtime.rs:36, parameter of the public runtime.rs:366 and the target of an intra-doc link at runtime.rs:347). In the other direction, `RUSTFLAGS="-W unreachable_pub" cargo check` names exactly four unreachable pubs (runtime.rs:36, state.rs:170/187/245), and three internal-only names are published for no reachable benefit: `merge_provider_attribution_headers` (lib.rs:42, returns a `cyrup_provider::HeaderMap` the facade does not export), `extension_discovery_roots` (lib.rs:48, returns a `cyrup_ext` type likewise), and `ProviderSwap` (lib.rs:70, appears in no public signature). `pub mod auth_guidance;` (lib.rs:20) publishes 133 lines of message formatters with zero external consumers, and lib.rs:128-131 re-exports four cyrup-agent transport types that nothing in the workspace names through this crate — cyrup-sdk reaches past it to `cyrup_agent` directly, and the facade's own signatures (builder.rs:394/398/487/495) spell the foreign path. These land as one change because the `pub(crate)` demotions would otherwise introduce fresh `unreachable_pub` warnings.

## Acceptance Criteria

- [ ] `RUSTFLAGS="-W unreachable_pub" cargo check -p cyrup-session-svc` reports zero unreachable_pub items for this crate, and `#![warn(unreachable_pub)]` sits beside `#![forbid(unsafe_code)]` at lib.rs:22.
- [ ] lib.rs re-exports `UsageCostBreakdownEntry`, `CacheWasteTotals`, `cyrup_core::Tool` and `BeforeSessionInvalidate`; state.rs:170/187/245 are `pub(crate)`.
- [ ] lib.rs no longer re-exports `merge_provider_attribution_headers`, `extension_discovery_roots`, `ProviderSwap`, or the four cyrup_agent transport types; the underlying items are `pub(crate)`, `src/tests/integration.rs:721` uses `crate::builder::extension_discovery_roots`, and the intra-doc link at guest_providers.rs:14 is re-pointed. `ProviderResolver` stays exported.
- [ ] lib.rs:20 reads `mod auth_guidance;` and its six items are `pub(crate)`; `pub mod export;` at lib.rs:27 is untouched.
- [ ] The `SessionActivity`/`SessionCatalog` visibility question (host_services.rs:277 and :299, `pub` in a private module, neither re-exported) is settled one way in one line — both demoted to `pub(crate)` with the ThemeAccess doc link reworded, or both added to the lib.rs:66-69 group — matching whichever option the queued CARGO_DOC_WARNINGS task takes.
- [ ] `cargo check`, `cargo clippy --all-targets` and `cargo test -p cyrup-session-svc` all pass, and `cargo doc -p cyrup-session-svc --no-deps` gains no new warnings.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Close the drift between public signatures and the hand-maintained re-export list in lib.rs

`CONFIRMED` · severity **medium** · effort **small** · dimension `dead-surface`

**Evidence.** (1) `pub async fn usage_cost_breakdown(&self) -> Vec<crate::state::UsageCostBreakdownEntry>` at session/stats.rs:34 returns the struct defined at state.rs:159, which lib.rs never re-exports — while every sibling return type from state.rs is re-exported in the `state::{…}` group at lib.rs:81-84 (`CompactionResult, ContextUsage, SessionStateView, SessionStats, StatsContextUsage, StatsTokens`). (2) `pub async fn cache_waste(&self) -> cyrup_provider::cache_stats::CacheWasteTotals` at session/stats.rs:45; lib.rs re-exports only `StreamEvent` from cyrup_provider (lib.rs:62). (3) `pub custom_tools: Vec<Arc<dyn cyrup_core::Tool>>` at builder.rs:177; `grep -n '\bTool\b' src/lib.rs` returns nothing — the cyrup_core re-exports are `EventStream` (lib.rs:91) and `Content, EntryId, ModelThinkingLevel` (lib.rs:139). (4) I ran `RUSTFLAGS="-W unreachable_pub" cargo check -p cyrup-session-svc`: exactly four unreachable `pub` items in this crate — runtime.rs:36 (`pub type BeforeSessionInvalidate`), state.rs:170 (`TOOLS_SUMMARIES_KEY`), state.rs:187 (`pub fn usage_cost_breakdown`), state.rs:245 (`pub fn cache_scan_entries`). `BeforeSessionInvalidate` is the parameter type of the public `AgentSessionRuntime::set_before_session_invalidate` (runtime.rs:366) and the target of an intra-doc link at runtime.rs:347; the three state.rs items are called only from session/stats.rs:36 and :49 (plus TOOLS_SUMMARIES_KEY from state.rs:214/:219/:449).

**Why it matters.** lib.rs's own banner at :90 ("Load-bearing re-exports so embedders need not depend on every subsystem directly") is the crate's stated contract, and these are the places it silently fails: three public methods and one public field whose types an embedder cannot name through the facade. The list is hand-curated with no compiler check, so every new public method is another chance to drift, and the four `unreachable_pub` items read as API while being unreachable.

**Fix.** Add `UsageCostBreakdownEntry` to the `state::{…}` group at lib.rs:81-84; add `CacheWasteTotals` beside the `cyrup_provider::StreamEvent` re-export at lib.rs:62; add `Tool` to the `cyrup_core::{…}` group at lib.rs:139; re-export `BeforeSessionInvalidate` in the `runtime::{…}` group at lib.rs:71-74. Make state.rs:170/:187/:245 `pub(crate)`. Then add `#![warn(unreachable_pub)]` beside `#![forbid(unsafe_code)]` at lib.rs:22 so the next drift is caught by the compiler — landing it together with the `pub(crate)` demotions from the auth_guidance and leaked-re-export findings, or those fixes will introduce fresh warnings.

### Three internal-only names leaked through lib.rs's public re-exports (`merge_provider_attribution_headers`, `extension_discovery_roots`, `ProviderSwap`)

`OVERSTATED` · severity **low** · effort **small** · dimension `dead-surface`

**Evidence.** (1) lib.rs:42 `pub use attribution::merge_provider_attribution_headers;` — `rg -n 'merge_provider_attribution' crates/` shows hits only inside cyrup-session-svc; both call sites spell the private path (builder.rs:1476, session/model.rs:244), and its return type is `Option<cyrup_provider::HeaderMap>` (attribution.rs:117-122), which lib.rs does not re-export. (2) lib.rs:48 `extension_discovery_roots` inside the `builder::{…}` group — no external hits; in-crate callers builder.rs:1127, builder.rs:2075 and src/tests/integration.rs:721-742; `pub fn extension_discovery_roots(cfg: &SessionConfig) -> cyrup_ext::DiscoveryRoots` (builder.rs:2165) returns a `cyrup_ext` type of which lib.rs re-exports only `NotifyKind` (lib.rs:65). (3) lib.rs:70 `pub use provider_swap::{ProviderResolver, ProviderSwap};` — `ProviderSwap` appears in no public signature: private field session/mod.rs:149 and the `pub(crate) fn from_parts` parameter at session/mod.rs:311-315. `rg -c 'allow\(dead_code\)' crates/cyrup-session-svc/src/` returns nothing — the crate has zero dead-code allows.

**Why it matters.** Three names in the crate's documented API that nothing outside can usefully call; two of them return types an external caller cannot even spell through the facade. They inflate the rustdoc surface a reviewer must reason about for no reachable benefit.

**Fix.** lib.rs:42 — delete the line and change attribution.rs:117 to `pub(crate) fn`. lib.rs:48 — drop `extension_discovery_roots` from the `builder::{…}` group, change builder.rs:2165 to `pub(crate) fn`, and update src/tests/integration.rs:721 to `use crate::builder::extension_discovery_roots;`. lib.rs:70 — change to `pub use provider_swap::ProviderResolver;` (`ProviderResolver` has real external users and must stay), make provider_swap.rs:33 `pub(crate) struct ProviderSwap`, and re-point the intra-doc link at guest_providers.rs:14 to `crate::provider_swap::ProviderSwap`.

**Verifier correction.** All three specific claims verified exactly. Two corrections. (1) The 'permanently exempt from rustc's dead-code analysis' argument does not hold and should be dropped: all three have live in-crate call sites (attribution.rs:117 from builder.rs:1476 and session/model.rs:244; builder.rs:2165 from builder.rs:1127 and :2075; ProviderSwap from builder.rs:1488 and session/mod.rs:149/:315), so dead-code would never fire either way. The real benefit is a smaller documented API, not rot detection — severity medium -> low. (2) The fix is incomplete: src/tests/integration.rs:721 does `use crate::extension_discovery_roots;` (called at :725, :732, :742) and will not compile once the lib.rs re-export is dropped; and guest_providers.rs:14 carries an intra-doc link `[`crate::ProviderSwap`]` that needs re-pointing.

### Make `auth_guidance` a private module

`CONFIRMED` · severity **low** · effort **small** · dimension `dead-surface`

**Evidence.** crates/cyrup-session-svc/src/lib.rs:20 `pub mod auth_guidance;` — one of only two `pub mod`s (the other is `pub mod export;` at lib.rs:27). The file is 133 lines exposing `pub fn get_provider_login_help` (:15), `format_no_models_available_message` (:25), `format_no_model_selected_message` (:33), `pub const UNKNOWN_PROVIDER` (:39), `format_no_api_key_found_message` (:50), `format_oauth_reauthenticate_message` (:62). `rg -n 'auth_guidance' crates/ --glob '*.rs'` outside this crate returns exactly one hit — an unrelated test name at crates/cyrup-provider/src/unconfigured.rs:181. Every real call site is in-crate via the private path: error.rs:63, builder.rs:1915, session/run.rs:447, session/run.rs:449, src/tests/modelless_launch.rs:104.

**Why it matters.** 133 lines of message-string formatters published as a documented public module with no consumer; anything added there silently joins the crate's API and its signatures must stay stable for callers that do not exist.

**Fix.** Change lib.rs:20 to `mod auth_guidance;`; no call site changes needed (all five already use `crate::auth_guidance::`). If `unreachable_pub` is enabled in the same pass, demote auth_guidance.rs:15/:25/:33/:39/:50/:62 to `pub(crate)`. Keep `pub mod export;` at lib.rs:27 — crates/cyrup-tui/src/export.rs:4 documents that seam by path.

### Duplicated cyrup-agent transport re-export at lib.rs:131 that neither the crate's own signatures nor any consumer uses

`OVERSTATED` · severity **low** · effort **small** · dimension `dead-surface`

**Evidence.** crates/cyrup-session-svc/src/lib.rs:128-130 is the doc comment ("so an embedder can name the custom-transport seam types ([`SessionBuilder::stream_fn`]/[`SessionBuilder::key_resolver`]) ... without a direct `cyrup-agent` dependency") and :131 is `pub use cyrup_agent::{ApiKeyResolver, ProxyStreamFn, ProxyStreamOptions, StreamFn};`. `rg -n 'session_svc::(ApiKeyResolver|ProxyStreamFn|ProxyStreamOptions|StreamFn)|cyrup_session_svc::(ApiKeyResolver|ProxyStreamFn|ProxyStreamOptions|StreamFn)' crates/ --glob '*.rs'` returns zero hits workspace-wide. crates/cyrup-sdk/Cargo.toml:18 takes `cyrup-agent = { workspace = true }` directly and crates/cyrup-sdk/src/lib.rs:72 re-exports the same names from the source: `pub use cyrup_agent::{stream_proxy, ApiKeyResolver, ProxyStreamFn, ProxyStreamOptions, StreamFn};`. The facade's own signatures spell the foreign path: builder.rs:394 `stream_fn: Option<Arc<dyn cyrup_agent::StreamFn>>`, builder.rs:398 `key_resolver: Option<Arc<dyn cyrup_agent::ApiKeyResolver>>`, builder.rs:487 `pub fn stream_fn(mut self, stream_fn: Arc<dyn cyrup_agent::StreamFn>)`, builder.rs:495 `pub fn key_resolver(mut self, resolver: Arc<dyn cyrup_agent::ApiKeyResolver>)`.

**Why it matters.** Two public spellings of the same four types; the crate never uses its own, and the one embedder-facing crate reached past it. The doc comment documents an intent the code contradicts, so rustdoc points readers at `cyrup_agent` while the facade claims to spare them that dependency.

**Fix.** Delete crates/cyrup-session-svc/src/lib.rs:128-131 and let cyrup-sdk keep its direct `cyrup_agent` re-export — this matches how the workspace actually consumes these types. (The alternative of re-pointing builder.rs:394/:398/:487/:495 and cyrup-sdk/src/lib.rs:72 at the local alias is more churn for no measured benefit; take it only if facade purity is being enforced crate-wide.)

**Verifier correction.** Evidence is exact and fully verified; severity is inflated. Corrected: this is a single `pub use` line plus a three-line doc comment whose stated purpose the codebase contradicts — no maintenance burden beyond that one line, and cyrup-sdk already publishes the direct path so no measurable consumer confusion in-tree. Severity medium -> low. Effort small is right; the fix is a one-line deletion.

### SessionActivity and SessionCatalog are `pub` in a private module and never re-exported — unreachable public API

`OVERSTATED` · severity **low** · effort **small** · dimension `large-files`

**Evidence.** /home/user/cyrup/crates/cyrup-session-svc/src/host_services.rs:277 and :299 declare both traits `pub`; src/lib.rs:30 `mod host_services;` (private); src/lib.rs:66-69 exports `ControlSink, EditorTextMirror, InjectMessage, InjectSink, LiveHostServices, OverlayRequest, OverlaySink, ThemeAccess, UiEffect, UiEffectSink, UiKind, UiReply, UiRequest, UiSink` — ThemeAccess (host_services.rs:341, same three-trait cluster) is in, these two are not. Consequence check: the pub inherent methods `attach_session_activity` and `attach_session_catalog` (host_services.rs:713 / :721) therefore take parameter types no external caller can name. `cargo doc -p cyrup-session-svc --no-deps` emits `warning: public documentation for \`ThemeAccess\` links to private item \`SessionActivity\``.

**Why it matters.** The visibility contradicts itself — `pub` claims external API but nothing outside the crate can name either trait — so a reader cannot tell whether they are an intentional embedder extension point (like the adjacent ThemeAccess) or crate plumbing. It also decides which module the attach-trait cluster lands in when host_services.rs is split.

**Fix.** Fold into the host_services split as a one-line decision on `attach.rs`: if these are internal attach-points, change both to `pub(crate) trait` (host_services.rs:277, :299) — non-breaking, nothing outside the crate names them — and reword the ThemeAccess doc link that references SessionActivity; if they are meant to be embedder-implementable alongside ThemeAccess, add `SessionActivity, SessionCatalog` to src/lib.rs:66-69, which also clears that rustdoc warning. Do not queue as standalone work and check CARGO_DOC_WARNINGS.md first so the two tasks pick the same option.

**Verifier correction.** The facts hold exactly: host_services.rs:277 `pub trait SessionActivity: Send + Sync` and :299 `pub trait SessionCatalog: Send + Sync`, module declared `mod host_services;` at src/lib.rs:30, and the src/lib.rs:66-69 re-export list contains ThemeAccess but neither of these; a workspace-wide rg finds them only in host_services.rs, src/session/adapters.rs, src/session/mod.rs and src/tests/mid_run_tool_anchoring.rs. What is overstated is the scope and the novelty. I ran `cargo doc -p cyrup-session-svc --no-deps`: one of this crate's 19 warnings is literally `public documentation for \`ThemeAccess\` links to private item \`SessionActivity\``, so the SessionActivity half is already surfaced by the queued CARGO_DOC_WARNINGS.md task. Only SessionCatalog is unattested there. Scope corrected to: settle the visibility of the trait pair as a rider on the host_services split, and coordinate with the doc-warnings task so the two do not fight (note that switching them to `pub(crate)` does NOT silence that rustdoc warning — only exporting them, or rewording ThemeAccess's doc link, does).
