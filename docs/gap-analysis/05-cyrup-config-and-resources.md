# 05 — cyrup-config + cyrup-resources

Covers `cyrup/crates/cyrup-config` (settings, auth store, trust, model resolution, config values, login) and `cyrup/crates/cyrup-resources` (packages, discovery, skills/prompts/themes), plus the launch-path glue in `cyrup/crates/cyrup/src/main.rs`, `migrations.rs`, `cli.rs` and `cyrup-session-svc/src/builder.rs` that consumes them. Measured against `pi/packages/coding-agent/src/core/{settings-manager,model-resolver,model-runtime,models-store,model-config,auth-storage,trust-manager,project-trust,package-manager,provider-composer,resource-loader,prompt-templates,skills,slash-commands,keybindings,resolve-config-value}.ts`, `src/{config,migrations,main}.ts`, `src/utils/paths.ts` and `modes/interactive/theme/theme.ts` — read at the explicit tags **v0.83.0** (the ported baseline) and **v0.84.1** (upstream latest) rather than a floating HEAD.

> **Re-audited 2026-08-12, cyrup HEAD `04c1ba2`** (working tree clean; `a9000b1` is docs-only), against
> **pi v0.83.0** for parity and **pi v0.84.1** for version lag. **16 items left the open set**
> (15 closed outright — CFG-002, CFG-010, CFG-011, CFG-022, CFG-024, CFG-029, CFG-031, CFG-032,
> CFG-033, CFG-034, CFG-S01, CFG-S02, CFG-S03, CFG-S04, plus CFG-001 re-confirmed closed — and
> **CFG-012 superseded**, because pi *adopted cyrup's* recursive merge at v0.84.1). **13 newly filed**
> (CFG-035 … CFG-047), **0 reopened**. Three severities were corrected DOWN against the auditor by the
> refuter: CFG-028 medium→low, CFG-030 medium→low, and CFG-034 closed with its **kind corrected to
> `upstream-drift`** (its v0.83.0 upstream cite was false — `scrollbarThumb` is a v0.84.1 addition
> cyrup had already anticipated). CFG-021 was **misdescribed twice** and is corrected in place: the
> key is `tuiMode`, not `uiMode`, and both it and `fullscreenScrollbar` are v0.84.1 additions, so the
> kind moves from `not-ported` to `upstream-drift`. CFG-004's cyrup cite was wrong and is repointed.
> Open set now **38 items: 0 critical, 1 high, 19 medium, 18 low** — the single high is CFG-035, and
> it is new. Read-only pass: nothing was compiled or executed.
>
> **Version-lag basis.** All `upstream-drift` items in this file were measured with
> `git diff v0.83.0..v0.84.1` scoped to this area's paths (nine files moved, 650+/201−). pi HEAD is
> `581d75a89` = **v0.84.1-117-g581d75a89**, so 117 commits past the diffed tag are unanalysed — see
> blind spot 5 in `## Coverage` for the one concrete item that window is known to hold.
>
> ### Repair pass 2026-08-12 (post-critique)
>
> Applied after the completeness critique of the twelve finished area files. The critique's finding 11
> observed that `packages/coding-agent/src/migrations.ts` appears in the whole gap-analysis directory
> exactly once — as an incidental `{mode: 0o600}` citation in this file — and that pi's `runMigrations`
> makes more calls than cyrup's does. A dedicated **migrations + keybindings surface sweep** was then
> run over `pi/packages/coding-agent/src/migrations.ts` and `src/core/keybindings.ts` (both
> byte-identical at v0.83.0 and v0.84.1: `git diff v0.83.0 v0.84.1 --` on both paths is empty) against
> `crates/cyrup/src/migrations.rs` and its read-time consumers. Its findings are absorbed here as
> **CFG-048 … CFG-051** (+2 medium, +2 low; 34 → 38 open). No item was renumbered, merged or deleted.
>
> The sweep's pairing table, reproduced because it is the evidence for CFG-048: pi's `runMigrations`
> (`migrations.ts:305-315`) makes five top-level calls, the fifth fanning out into two more — six
> migration behaviours. cyrup's `run_migrations` (`migrations.rs:26-33`, re-read at HEAD in this pass)
> makes **four**: `migrate_auth_to_auth_json` (`:27`), `migrate_sessions_from_agent_root` (`:28`),
> `migrate_tools_to_bin` (`:29`), `migrate_extension_system` (`:30`) — with commands→prompts and the
> deprecated-dir scan nested inside the fourth. The missing one is `migrateKeybindingsConfigFile()`
> (`migrations.ts:312`). **cyrup runs no migration pi does not**: `rg 'fn migrate|migrate_' crates
> --include='*.rs'` resolves only to `crates/cyrup/src/migrations.rs`, `crates/cyrup-config/src/
> settings.rs` (a faithful port of `migrateSettings`) and its `lib.rs:62` re-export.

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
> **Area 05 — recount: 38 rows → 18 open (0 critical · 0 high · 11 medium · 7 low).** The area's only
> high, `CFG-035`, is CLOSED: sweep 2 landed the discovery half here
> (`crates/cyrup-resources/src/discovery.rs`, `discover_system_prompt_file` /
> `discover_append_system_prompt_file` riding out on `DiscoveryReport`) and area 08 landed the wiring
> half **concurrently and independently** (`cyrup-session-svc/src/builder.rs:1219-1258`), including
> pi's REPLACE-not-accumulate rule for the append leg. Its one residual — a doc line at
> `cyrup-session/src/prompt/overrides.rs:15-16` — is re-filed against area 03 rather than holding a
> high open.
>
> **`CFG-048`'s mechanism landed in a new file the item did not anticipate**, `crates/cyrup-config/src/keybindings.rs`, and the reason is worth keeping: pi applies the migration table **twice** — write
> time from `runMigrations` and read time from `loadFromFile` — and cyrup's two consumers
> (`crates/cyrup/src/migrations.rs`, `crates/cyrup-tui/src/keymap.rs`) have no other common ancestor.
> Same argument that put `migrate_settings` there. Two facts in the item are corrected in its row: the
> instruction to respell 25 targets as `editor.*` is stale and was NOT followed, and `KEYBINDINGS` has
> **42** app ids, not 41 (73 total).
>
> **`CFG-003`'s premise was wrong in the record and is corrected before anyone schedules it.** Sweep
> 1's handoff says auto-install is "gated on an opt-in setting that does not exist"; at the tag,
> `resolvePackageSources` (package-manager.ts:1260-1271 @v0.83.0) installs UNCONDITIONALLY unless
> `isOfflineModeEnabled()` or the optional `onMissing` callback says otherwise. The real blocker is
> structural — a three-crate async restructuring — which is what makes it L.
>
> **CROSS-CRATE BREAKAGE deliberately introduced by `CFG-027`, for the seam phase:**
> `crates/cyrup-session-svc/src/tests/settings_resolve.rs::missing_settings_declared_package_is_reported_not_fatal`
> declares `"packages": ["./nope-not-here"]` — a LOCAL path — and asserts a startup diagnostic, but pi
> is SILENT for a missing local path (package-manager.ts:1324-1326) and cyrup now is too. Its own doc
> cites `:1244-1283`, the npm/git install arm, not the arm its fixture hits. **The fix is to repoint
> the fixture at `"github:org/nope-not-here"`, the arm cyrup's no-network `[CYRUP-DELTA]` actually
> covers — NOT to weaken the assertion.** The in-crate twin was corrected the same way
> (`cfg003_missing_settings_declared_package_is_an_error_diagnostic` →
> `cfg003_uninstallable_settings_declared_package_is_an_error_diagnostic`).
>
> **Blind spots 2 and 3 are unchanged and must not be read as narrowing.** `login.rs` is untouched by
> sweep 2; of blind spot 3's six files, sweep 2 read only `policy.rs:21-48` (`NetworkPolicy::resolve`,
> for `DRIFT-050`) and `package/source.rs:60-118` (`PackageSource::parse`, for `CFG-027`).
> `cyrup-resources/src/{theme.rs, scope.rs, key.rs}`, `cyrup-config/src/env_keys.rs` and
> `cyrup-resources/src/package/git_url.rs` are still unread end to end.
>
> **STALE CITATION, workspace-wide:** `first_env` no longer exists in `cyrup-config/src/env.rs` — it
> was folded into a closure inside the new `EnvVars::from_lookup(get)` seam (see `DRIFT-050`). Any
> area file citing `cyrup-config/src/env.rs:50-53 first_env` is stale.


## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| CFG-001 | **closed** | Re-confirmed at HEAD. `ensure_scope_writable` (`settings.rs:1329-1337`) latches per scope, is called first by `set` (`:1349`) and `set_nested` (`:1404`), and both re-check inside `with_lock`, returning `SettingsWriteRefused` on mid-write corruption (`:1367-1377`, `:1414-1418`). pi `settings-manager.ts` `save()` early-returns on `globalSettingsLoadError` @v0.83.0. Residual hole is CFG-030 (now low). |
| CFG-002 | **closed** | `pub oauth: Option<ModelsJsonOauth>` at `model.rs:1481-1487`, enum `Radius` only at `:1453-1458` matching pi's `Type.Literal("radius")` (`model-config.ts:194` @v0.83.0); oauth-without-baseUrl rejected at `:1843-1848` with pi's exact string; `config.oauth.is_none()` added to the empty-block guard (~`:1859`); the `oauth === "radius"` baseUrl special-case honoured (~`:1876-1880`). Launch-predicate half closed separately as CFG-022. |
| CFG-003 | **closed 2026-08-15** | The `[CYRUP-DELTA]` "no network install during session assembly" is gone from `resolve_configured_package`'s doc; the git arm now installs behind `DiscoveryConfig::install_missing_packages`. See the Open-items row for the full disposition, including the two deltas that remain and the struck "three-crate async restructuring" blocker. |
| CFG-004 | **superseded** (by CFG-025) | Residual is entirely CFG-025, still open. **Cite corrected:** the extension push is `add_local_entries` at `discovery.rs:1373-1379`, NOT `:1242-1246` (that is the SKILL.md walk). Id kept, never reused. |
| CFG-005 | partially closed | `login.rs` (1721 lines, new) ports login/logout/env-key login/status/selectors; refresh at `cyrup-provider/src/auth/resolve.rs:146-239`. Residual: the two **multi-prompt** api-key logins (cloudflare, google-vertex). Maintainer-deprioritised — filed, not scheduled. Remains open below at medium. |
| CFG-006 | partially closed | `retry.provider.*` now threaded (`builder.rs:1223-1234`). `websocketConnectTimeoutMs` still inert. Remains open below at medium. |
| CFG-007 | still open | `AuthStore` re-reads `auth.json` per query; errors coerce to "not configured". Upstream cites corrected to the ported tag. |
| CFG-008 | still open | Model-scope resolution drops every diagnostic. |
| CFG-009 | still open | `npm:` source reports "unsupported source (OCI deferred)". |
| CFG-010 | **closed** | `autoload: Option<bool>` on `PackageSource::Detailed` (`settings.rs:99-103`, accessor `:132-137`) → `PackageFilter` (`builder.rs:1756-1758`) → `retain_by_package_filter` delta branch (`discovery.rs:261-283`), `subtract_delta_shadow` (`:296-322`) and the dedupe delta (`builder.rs:1779-1783`). Verified BELOW the entry condition this time: `apply_autoload_disabled_patterns` (`manifest.rs:401-454`) reproduces pi's `+`/`-`, bare/`!` glob and last-write-wins Map semantics arm for arm, and `delta_shadow` is live (assigned at `discovery.rs:680-696`). |
| CFG-011 | **closed** | `resolve.rs:145-147` is now `now_millis().saturating_add(minimum_validity_ms) >= expires` with `now_millis` at `:239-245`; regression tests at `:661-664`, `:683`, `:699`, `:775`. Units agree with pi's `Date.now() + expires_in * 1000 - 5*60*1000` (`ai/src/auth/oauth/anthropic.ts:225`/`:338` @v0.83.0). |
| CFG-012 | **superseded** | **Upstream moved to cyrup's behaviour.** pi v0.84.1 `settings-manager.ts:139-160` replaces the single-level spread with `deepMergeObjects` + `isMergeableObject` (arrays and null excluded), which is behaviourally identical to cyrup's `deep_merge` (`settings.rs:475-491`). Do NOT "fix" cyrup toward the retired v0.83.0 shape — that would be a regression. |
| CFG-013 | still open | `TrustStore::nearest` reads without the file lock. |
| CFG-014 | still open | `showCacheMissNotices` absent — one of only three Settings keys with zero occurrences in `settings.rs` on the 47-key sweep. |
| CFG-015 | still open | Four unconsumed accessors, **plus a fifth key folded in this pass**: `collapseChangelog`, whose only consumer is the `/settings` display row. |
| CFG-016 | still open | `${0:-default}` emitted literally. |
| CFG-017 | still open | `${@:-default}` / `${ARGUMENTS:-default}` unsupported. |
| CFG-018 | still open | Glob scope no longer short-circuits on an exact reference. |
| CFG-019 | partially closed | The two `qwen-token-plan` arms landed at pi's insertion position (`model.rs:973-974`, `KNOWN_PROVIDERS` `:1022-1023`). Still open: the stale `xai` id and the missing `radius` arm. New v0.84.1 entries are CFG-041, not this item. |
| CFG-020 | still open | No `ModelRuntime` type at HEAD (`grep -rn 'struct ModelRuntime' crates/` is empty); registry recomposed per call. Upstream target GREW at v0.84.1 (+356 lines). |
| CFG-021 | **misdescribed → corrected in place** | The key is `tuiMode`, not `uiMode` (`uiMode` exists nowhere in pi). Both `tuiMode` and `fullscreenScrollbar` are **v0.84.1 additions**, so kind moves `not-ported` → `upstream-drift`. Still zero occurrences in cyrup. Remains open below at low. |
| CFG-022 | **closed** | ONE shared predicate: `provider_is_configured(auth, models_json, provider, env)` at `model.rs:1796-1804`, called from `main.rs:388-392` and `session.rs:2659-2666`. Step-4 regression test at `crates/cyrup/src/tests/models_json_resolution.rs:120-160`. |
| CFG-023 | still open | **Not** closed by CFG-022's landing: `find_initial_model` step 3 (`model.rs:1341-1354`) still returns the saved default unconditionally. |
| CFG-024 | **closed** | `models_json_provider_is_configured` (`model.rs:1817-1832`) now tests `is_command_config_value(raw) \|\| is_config_value_configured(raw, env)` and returns false for an oauth-only block. Purity preserved — no resolution on the status path (`:1806-1815`). |
| CFG-025 | still open | No `~` / `file://` expansion on settings paths or local package sources. Absorbs CFG-004's residual; CFG-031 closed by expanding only its own site, so the shared util is still owed here. |
| CFG-026 | still open | Settings packages deduped by raw source string; the in-code note at `builder.rs:1772-1774` names this item. |
| CFG-027 | still open | Bare-extension-directory local package contributes nothing. |
| CFG-028 | still open, **severity corrected medium → low** | Evidence accurate, rating was not: pi's `execSync` blocks its ONE event loop, cyrup's blocking call occupies one worker of N. cyrup is strictly *less* blocking than upstream — a robustness note, not a parity gap. |
| CFG-029 | **closed** | The always-true closure is gone: `models_json_resolution.rs:91-98` builds the real `provider_is_configured` predicate over a temp `AuthStore`, with a step-4 case (`:120-160`) and a negative case (`:165-179`). |
| CFG-030 | still open, **severity corrected medium → low** | Both sides mangle a non-object top level; pi merely preserves array elements as meaningless indexed keys. Trigger is pathological and load behaviour is identical. |
| CFG-031 | **closed** | `settings.rs:732-734` is `self.merged.get_str("shellPath").map(|s| expand_tilde(&s))` with provenance at `:724-731` and tests at `:1595-1616`, `:1625-1635`. |
| CFG-032 | **closed** | `migrations.rs:106` now calls `cyrup_config::lock::write_atomic(&auth_path, …, true)`; `write_atomic(secret=true)` sets `.mode(0o600)` and `set_permissions(0o600)` before rename (`lock.rs:93-112`). Matches pi's `{ mode: 0o600 }` (`migrations.ts:67-70` @v0.83.0). |
| CFG-033 | **closed** | `cyrup-test-support/src/auth.rs:70-75` is `now_millis()`; decision sites `:132`/`:147`; fixtures re-expressed in ms (`:195`, `:233`, `:276`) with a dedicated millisecond test at `:239-256`. |
| CFG-034 | **closed**, **kind corrected → `upstream-drift`** | cyrup side real: `theme.rs:1036` `pub scrollbar_thumb: Option<Color>`, `g("scrollbarThumb").or(selected)` at `:483`, tests at `theme_fidelity.rs:831-880`. **The item's v0.83.0 upstream cite was FALSE** — `git grep scrollbarThumb v0.83.0 -- packages` returns nothing; those are v0.84.1 line numbers. cyrup anticipated a v0.84.1 addition. |
| CFG-035 | **new — open (high)** | `.cyrup/SYSTEM.md` / `APPEND_SYSTEM.md` never discovered; the trust gate prompts about files cyrup never reads. |
| CFG-036 | **new — open (medium)** | `--session-dir` and the three `CYRUP_*_DIR` env vars are not tilde-expanded. |
| CFG-037 | **new — open (medium)** | Project-scope git package install writes no `.gitignore` into the user's repo. |
| CFG-038 | **new — open (medium)** | One bad key spec discards the whole `keybindings.json` — after partially applying it. |
| CFG-039 | **closed 2026-08-15 (REFUTED: already closed at HEAD)** | models.json `samplingParams` silently dropped (v0.84.1 addition). ~~Hard-blocked on a provider-side field.~~ Both halves landed in batch A; the "HARD-BLOCKED" framing was stale. |
| CFG-040 | **new — open (low)** | `markdown.mermaid` key and its getter/setter absent (v0.84.1 addition). |
| CFG-041 | **new — open (low)** | `defaultModelPerProvider` missing v0.84.1's `baseten` and `qwen-token-plan-individual`. |
| CFG-042 | **closed 2026-08-15** | `FileModelsStore` does not normalize its path, cache by revision, or accept cancellation. Final residual — the `signal` parameter — landed in `cyrup-provider`. |
| CFG-043 | **new — open (low)** | Invalid `models.json` reports a serde parse error instead of pi's per-field schema report. |
| CFG-044 | **new — open (low)** | Three `auth-storage.ts` provenance cites resolve to nothing upstream; `get_auth_status` is dead code. |
| CFG-045 | **new — open (medium)** | `doubleEscapeAction` is inert and cyrup's Escape handler drops two of pi's four branches. |
| CFG-046 | **new — open (medium)** | models.json string fields are not length-validated, so `"baseUrl": ""` composes where pi rejects the file. |
| CFG-047 | **new — open (low)** | Three built-in slash-command metadata divergences (`/model`, `/login` argument hints, `/reload` description). |
| CFG-048 | **new (repair pass) — open (medium)** | pi's sixth startup migration, `migrateKeybindingsConfigFile`, is not ported at write time **or** read time, so all 59 legacy keybinding names are silently inert. |
| CFG-049 | **new (repair pass) — open (medium)** | Extension-system deprecation warnings are printed and immediately buried; pi blocks startup on a keypress so they cannot be missed. |
| CFG-050 | **new (repair pass) — open (low)** | `migrate_tools_to_bin` relocates the managed `fd`/`rg` binaries with no completion notice, so the move looks like a disappearance. |
| CFG-051 | **new (repair pass) — open (low)** | The migrated-credentials notice goes to stderr microseconds before the first TUI frame instead of into the transcript, and its provenance cite names unrelated upstream code. |
| CFG-S01 | **closed** | `resolve_prompt_input` ported (`cli.rs:7-9`), applied to `--system-prompt` (`:456-460`) and each `--append-system-prompt` (`:368-380`, `:461-463`), with three named tests (`:1693-1740`). Matches pi's path-vs-literal-by-existence + warn-and-fall-back (`resource-loader.ts:53-68` @v0.83.0). |
| CFG-S02 | **closed** | `image_auto_resize()` (`settings.rs:787-792`) with live consumers (`builder.rs:681`, `main.rs:546`/`:780`) and end-to-end proof at `cyrup-session-svc/tests/read_image_auto_resize.rs:150-170`. |
| CFG-S03 | **closed** | Byte-identical conflict messages at `cyrup-ext/src/registry.rs:222` and `:591-607`, first-registration-wins precedence, tests at `cyrup-ext/tests/extension_name_conflicts.rs:16-17`, `:208`. Matches `detectExtensionConflicts` (`resource-loader.ts:1059-1093` @v0.83.0). |
| CFG-S04 | **closed** for its four named keys | `enable_skill_commands` → `commands.rs:311`/`:324`; `tree_filter_mode` → `app/execute.rs:131`, `tree_selector.rs:255`; `editor_padding_x` → `editor.rs:238`; `show_hardware_cursor` → `editor.rs:179`. Wiring proofs at `cyrup-tui/src/tests/settings_inert_keys.rs`. A FIFTH key of the same class escaped the sweep and is now CFG-045; `collapseChangelog` is folded into CFG-015. |

## Open items

> **⚠ COUNT THIS TABLE ONLY — but do not assume the `-S` ids are gone.** All four surface-sweep items
> (`CFG-S01`…`CFG-S04`) closed this pass, so the second table under `## Surface-sweep findings` is now
> historical; its ids are retained there and in the status table so the closures can be re-audited.
> The 2026-08-07 undercount that structural defect A in `00-residual-ledger.md` describes came from
> reading one table and ignoring the other — check both headings before quoting a count.

> **RECOUNTED 2026-08-14 (sweeps 3-6 reconciliation) — counted set: 0 critical, 0 high, 5 medium, 7 low = 12.** 28 rows are now marked CLOSED, including the area's only high (`CFG-035`). Sweep 6 closed eight (`CFG-026`, `CFG-048`, `CFG-049`, `CFG-008`, `CFG-050`, and — as REFUTED, already closed at HEAD — `CFG-006`, `CFG-047`, `CFG-005`), narrowed three (`CFG-051`, `CFG-042`, `CFG-015`), corrected one (`CFG-014`) and filed two new (`CFG-052`, `CFG-053`). *(Previous edition: 0 / 0 / 11 / 7 = 18, 20 closed.)*
>
> **RECOUNTED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set: 0 critical, 0 high, 4 medium, 5 low = 9.** The table carries **40 rows: 31 fully closed, 9 open (1 partially)**. Sweep 8 closed three: **`CFG-051`** on both halves (live-observed in a running UI *and* pinned by a rendered-transcript assertion), **`CFG-045`** as already-done (landed under `TUI-009`; the row's "unchanged at HEAD" was stale for two sweeps), and **`CFG-052`** as **REFUTED** — its premise about upstream is false and the refutation is the durable finding, not the closure. **`CFG-049` stays closed but its stated COVERAGE GAP is now closed by live observation too**, with two residuals restated rather than dropped. *(Previous edition: 0 / 0 / 5 / 7 = 12, 30 closed.)*
>
> **RECOUNTED 2026-08-15 (batch C — the four area-05 rows whose fix sites land in other crates).**
> This block is authoritative over the file header and over every earlier recount block above it.
> **Seven rows disposed: four FIXED, three REFUTED-or-record.** `CFG-069` (three crates), `CFG-072`
> (cyrup-tools), `CFG-071` and `CFG-075` (cyrup-ext) and `CFG-042`'s final residual (cyrup-provider,
> forcing a mechanical impl update in cyrup-config) are **closed with tests**; `CFG-039` and
> `CFG-058` are closed as **REFUTED**, and in both cases the refutation is the durable finding.
> `CFG-070` was read and deliberately NOT actioned — it is correct as written and its own row says so.
>
> **Two ledger-quality findings this pass, both of the kind the methodology warns about:**
>
> 1. **`CFG-039`'s `HARD-BLOCKED` marker had been discharged for a whole batch.** The row said "Area 01
>    must add the provider-side field FIRST" and was re-verified as blocked by sweep 6; by `831321b`
>    both the provider half AND the config half had landed under `AGENT-026`, with tests. A blocked-on-
>    another-area marker is a claim about a moving target and must be re-checked at HEAD, not inherited.
> 2. **`CFG-069` was HALF DONE and the row did not say so** — one of its three named sites was already
>    annotated in batch B. Same pattern as `SUBA-057` and `PROV-014`. It also spans three crates, where
>    the routing note assigns it to one.
>
> **One `Fix` paragraph would have introduced a defect if followed** (`CFG-058`): applying the 15 000 ms
> default "where the `Option` is consumed" erases the unset-vs-explicitly-15000 distinction pi keeps,
> and bounds a handshake cyrup never performs. **One `Verify` was unrunnable as written** (`CFG-072`) and
> was landed against an extracted pure function instead of a live process environment.

> **The routing note below is now stale on three of its eight entries** (`CFG-045`, `CFG-051`'s residual and `CFG-052` are closed); the remaining five stand.
>
> **AMENDED 2026-08-14 (documentation audit) — counted set: 0 critical, 0 high, 5 medium, 6 low = 11.** Two rows filed from a user-documentation audit that read the CLI surface against the code: `CFG-054` (doubled `packages/packages/` path) and `CFG-055` (`cyrup remove` id round trip). Both are `cyrup-original`; neither has an upstream leg established. The registry-path half of `CFG-054` was verified by running the binary, which is rare for this directory — see `REPRO-LOG.md` for the standard that sets.

> ### AMENDED 2026-08-14 (mechanical surface enumeration — settings.json + environment variables)
>
> **Counted set: 0 critical, 0 high, 10 medium, 20 low = 30.** Twenty-one rows added, `CFG-056` …
> `CFG-076`; **two of them (`CFG-056`, `CFG-057`) were FIXED in the same pass and are already closed**,
> so 19 of the 21 are open. No id was renumbered, merged or deleted.
>
> These did **not** come from re-reading the backlog. Two upstream surfaces were enumerated
> MECHANICALLY and diffed in both directions — every `settings.json` key/type/default/precedence
> (66 upstream vs 67 cyrup), and every environment variable read or written (130 upstream vs 233
> cyrup) — and the findings were filed regardless of whether an item already pointed at them. That
> is why the largest single class here is **`cyrup-original`** (11 of the 21): surfaces cyrup has and
> pi does not are the class this directory has had no habit of tracking, and an invented surface is
> how divergence enters while everyone is looking at parity.
>
> **The area's new `high` was found and closed in the same pass.** `CFG-056` — `defaultThinkingLevel`
> fell back to `off` where pi falls back to `medium`, so every user who had never written the key
> started every session with reasoning DISABLED. It was invisible to a backlog re-read because
> nothing was missing: the getter existed, the key was honoured, the `/settings` row agreed with the
> getter, and the wrong constant was `ModelThinkingLevel::default()` — a correct value for the type's
> zero, in the one place it is not the right fallback.
>
> **THE ENVIRONMENT-VARIABLE ENUMERATION IS INCOMPLETE, and these rows must not be read as coverage
> of it.** Three gaps are named by the enumeration itself:
> 1. **`pi-mcp-adapter`'s env surface was extracted but NOT diffed** — `BROWSER`, `GLIMPSE_BINARY`,
>    `MCP_DIRECT_TOOLS`, `MCP_HASH_{CWD,ENV,HEADER,TOKEN,URL}`, `MCP_OAUTH_CALLBACK_PORT`,
>    `MCP_OAUTH_DIR`, `MCP_UI_DEBUG`, `MCP_UI_VIEWER`, `NPM_CONFIG_CACHE`, the five
>    `PI_MCP_ADAPTER_*` test/keyring vars, `PI_PACKAGE_DIR`, `SSH_CONNECTION`, `SSH_TTY`, `HOME`.
>    Spot-checked: `MCP_UI_DEBUG`, `MCP_UI_VIEWER`, `MCP_OAUTH_DIR`, `MCP_OAUTH_CALLBACK_PORT`,
>    `GLIMPSE_BINARY` and all five `MCP_HASH_*` return zero hits in `crates/`. **That area is owned by
>    `MCP-PORT-METHODOLOGY.md` / `13-cyrup-mcp.md` and is deliberately untouched here** — somebody
>    with that ownership should run the same `PI_`→`CYRUP_` diff against it.
> 2. **The cyrup→`pi-subagents` direction is only partly walked.** `pi-subagents`' 48 literal env
>    names were diffed against cyrup (that produced `CFG-067`), but the ~110 `CYRUP_SUBAGENT_*` /
>    `CYRUP_INTERCOM_*` names were **not** all walked back the other way. `CFG-074` names the seven
>    confirmed cyrup-originals in that family; **there may be more.**
> 3. **~110 of cyrup's 233 names come from the three sibling ports** (`pi-subagents`,
>    `pi-intercom`, `pi-permission-system`). Diffing only `pi/packages` would mislabel every one of
>    them as a cyrup-original; the rows below were assigned against the sibling repos, not against
>    `pi/packages`. Anyone re-deriving this must do the same or the `cyrup-original` count is fiction.
>
> **Findings from the same sweep that got no new id, and why** — recorded so nobody re-derives them:
> `markdown.mermaid` → `CFG-040`; the deep-merge recursion depth → `CFG-012` (**superseded**; upstream
> moved TO cyrup's behaviour — do not "fix" it); `PI_TUI_WRITE_LOG` → `TUI-040`;
> `PI_SHARE_VIEWER_URL` → `TUI-063`; `SystemRoot` / `WINDIR` → `12-upstream-drift-pi-core.md:1075`
> (the `ensureTool` N/A); `LLAMA_BASE_URL` / `HF_HOME` / `HF_TOKEN_PATH` → `EXT-027`;
> `PI_CONFIG_DIR` / `PI_SERVER_DIR` / `PI_RADIUS_URL` / `PI_RADIUS_SERVER_URL` →
> `12-upstream-drift-pi-core.md:1073` (`packages/server` is outside the dependency closure);
> `PNPM_HOME` → `SEAM-078`; the process-global `PI_CODING_AGENT` set → `TOOL-031` / `PARITY-GAPS`
> PB-5; the `HTTP_PROXY` mechanism difference → `PROV-047`; the `NO_PROXY` case-folding "gap" →
> **not a defect**, retired in `CFG-060`'s body; `CYRUP_SHELL` → **not a cyrup-original**, it is the
> sentinel of a NEGATIVE test under `TOOL-039` and is the one case where a grep hit means the
> opposite of what it looks like.

> **ROUTING — of the eleven rows still open, EIGHT have their fix site outside `cyrup-config`/`cyrup-resources`:** `CFG-038`, `CFG-045`, `CFG-021`, `CFG-051`'s residual, `CFG-014` and `CFG-015` are **cyrup-tui**; ~~`CFG-042`'s residual and `CFG-039` are **cyrup-provider**~~ — **both closed 2026-08-15; `CFG-039` was a REFUTATION (already done at HEAD, its HARD-BLOCKED marker stale), and `CFG-042`'s residual landed in `cyrup-provider` with a forced mechanical impl update in `cyrup-config`.** Only `CFG-052` is in-area; `CFG-003` and `CFG-020` are three-crate restructurings. **Six of the thirteen rows sweep 6 opened were already closed at HEAD or had stale headline evidence** — re-verify before scheduling.

> ### 2026-08-15 — `cyrup-resources` slice (packages / skills / prompts / themes)
>
> Three rows left the open set from this crate; the `cyrup-config` half of the file was another
> agent's slice in the same pass, so **no global count is restated here** — recount the table.
> **`CFG-003` FIXED** (the area's last `L`, and it was not an `L`: its stated three-crate blocker is
> struck in its row as false). **`CFG-054` and `CFG-055` REFUTED** — both were already closed at
> HEAD `68bbd39` with tests, and the refutation is the finding: two of the three rows this slice was
> given were stale, which is the same ~12% error rate `00-residual-ledger.md` publishes. Nothing was
> "fixed" for either. The ROUTING note above is now stale on `CFG-003` (no longer a restructuring,
> no longer open) and on `CFG-052` (closed as refuted); its cyrup-tui / cyrup-provider entries
> stand.
>
> **One measurement worth keeping for whoever schedules `CFG-020`:** the same "async restructuring"
> phrasing that was wrong for `CFG-003` is attached to `CFG-020`. It was wrong here because the
> install path is synchronous end to end (`install_blocking`, `git_clone`) and only the *wrapper*
> is `async`. Check that before pricing `CFG-020` as an `L`.
>
> ### 2026-08-19 — post-`40821ed` citation repair (this block is authoritative over every block above it)
>
> **RECOUNTED 2026-08-19 — counted set: 0 critical, 0 high, 4 medium, 12 low = 16.** The table
> carries **64 rows: 48 closed, 16 open.** Net movement is −1 medium, +1 low. **`CFG-038` CLOSED as
> LANDED** — its own three-step recipe is in the tree with tests; the row had said "still open;
> VERIFIED at HEAD by sweep 6" for two editions and was found stale only because the `app.rs` split
> forced a re-read of its citations. **`CFG-077` FILED and record-only** — the recursive namespaced
> prompt scan, a `cyrup-original` that `crates/cyrup-flux` depends on outright.
>
> **The pass that produced both was a CITATION repair, and that is the finding.** `40821ed` split
> `crates/cyrup-tui/src/app.rs` into `crates/cyrup-tui/src/app/`, and every `app.rs:NNNN` in this
> file is now re-pointed by SYMBOL and verified by reading the target — not renumbered. Two of the
> repaired citations had been *mechanically* remapped to text that matched but meant something else
> (`app/execute.rs:135` for a `doubleEscapeAction` write that is `app/execute_misc.rs:204-205`;
> `app/tree_nav.rs:137` for a `warnings.anthropicExtraUsage` read that is `app/selectors.rs:290`),
> and six more carried a range whose END was left at the pre-split number
> (an `app/mod.rs` range ending 661 lines past that file's EOF). **A citation that resolves is not a citation that
> is right** — every claim re-stated in this pass was re-derived from the code at HEAD, and that is
> how `CFG-038` was caught.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~CFG-035~~ | ~~high~~ **CLOSED 2026-08-14** | not-ported | M | `.cyrup/SYSTEM.md` and `APPEND_SYSTEM.md` are never discovered — the trust-gated project system-prompt override is inert — **CLOSED 2026-08-14**: sweep 2 — the area's only `high`. The DISCOVERY half landed in `crates/cyrup-resources/src/discovery.rs` (`discover_system_prompt_file` / `discover_append_system_prompt_file` over a shared `discover_prompt_override`, both on `DiscoveryReport`), so `grep -rn 'SYSTEM\.md' crates/` no longer returns five hits none of which read a file; trust gates the PROJECT candidate only, so an untrusted project falls through to `<agent_dir>/SYSTEM.md` rather than to nothing. The WIRING half landed CONCURRENTLY in area 08 (`cyrup-session-svc/src/builder.rs:1219-1258`), including pi's REPLACE-not-accumulate rule for the append leg. **The one residual — the doc line at `cyrup-session/src/prompt/overrides.rs:15-16` — is re-filed against area 03 as SESS-035's residual rather than holding this item open.** |
| ~~CFG-023~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `find_initial_model` step 3 accepts a saved default whose provider has no configured auth — **CLOSED 2026-08-14**: sweep 1. |
| ~~CFG-025~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | Settings-declared paths and local package sources do not expand `~` or `file://` — **CLOSED 2026-08-14**: sweep 1 — closed by one shared util, `crates/cyrup-config/src/paths.rs`, creating a new `cyrup-resources → cyrup-config` dependency edge (a second copy is what the `encode_cwd` handoff warns against). The util does NOT resolve relative paths — it is `normalizePath`, not `resolvePath`. **Sweep 2 amendment:** the module now ALSO carries the v0.84.1 `normalizeWindowsShellPath` step (paths.ts:66-73, applied at :83-85), so it is measured against v0.84.1 for that one function — a future auditor must not read the win32 branch as an invention. See DRIFT-046. |
| ~~CFG-026~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | Settings packages deduped by raw source string, not resolved identity — **CLOSED 2026-08-14**: sweep 6 — ported pi's `getPackageIdentity` (`package-manager.ts:1676-1690`) as `cyrup_resources::package_identity(source, base_dir)` + `npm_spec_name` (`crates/cyrup-resources/src/package/source.rs`, keys `npm:<name>` / `git:<host>/<path>` / `local:<resolved>`), with the path half as `cyrup_config::paths::resolve_path_from_base` / `_with_home` plus a lexical `..`/`.` collapse (`crates/cyrup-config/src/paths.rs`) — the second half of pi's `resolvePath` (`utils/paths.ts:81-85`) that CFG-025's `normalize_path` deliberately stopped short of. `cyrup_resources::scope_base_dir` ports `getBaseDirForScope` (`:2071-2088`). **Wired at BOTH of pi's call sites, each against its OWN scope base exactly as upstream does:** `dedupePackages` (`:1696-1716`) → `cyrup-session-svc/src/builder.rs::configured_packages_from_settings` (now takes cwd + agent_dir), and `findAutoloadDeltaBase` (`:1301-1313`) → `cyrup-resources/src/discovery.rs::resolve_configured_package`. **The item named only the dedupe; the delta-base pairing had the same raw-string key**, and pi compares the project entry's identity against the PROJECT base and the user entry's against the USER base (`:1307` vs `:1311`), so for a relative local path they never match and there is no delta pairing — cyrup was MORE permissive than pi, and `discovery.rs`'s doc comment asserted the opposite of upstream's behaviour. Net behaviour: `"packages": ["./pack"]` in both scopes is now two packages (`<cwd>/.cyrup/pack` and `<agent_dir>/pack`) instead of the global one being silently dropped; `npm:x@1`/`npm:x@2` and SSH/HTTPS URLs for one repo now collide as they should. The in-code `[CYRUP-DELTA]` at `builder.rs` is deleted. Tests: 5 in `package/source.rs`, 3 in `builder.rs` (one RED before the change), 3 in `paths.rs`. |
| ~~CFG-036~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | `--session-dir` and the `CYRUP_*_DIR` env vars are not tilde-expanded — **CLOSED 2026-08-14**: sweep 1 — closed by one shared util, `crates/cyrup-config/src/paths.rs`, creating a new `cyrup-resources → cyrup-config` dependency edge (a second copy is what the `encode_cwd` handoff warns against). The util does NOT resolve relative paths — it is `normalizePath`, not `resolvePath`. **Sweep 2 amendment:** the module now ALSO carries the v0.84.1 `normalizeWindowsShellPath` step (paths.ts:66-73, applied at :83-85), so it is measured against v0.84.1 for that one function — a future auditor must not read the win32 branch as an invention. See DRIFT-046. |
| ~~CFG-037~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | A project-scope git package install writes no `.gitignore`, so the clone lands in the user's working tree — **CLOSED 2026-08-14**: sweep 1 — closed with CFG-025/CFG-036 on the shared `crates/cyrup-config/src/paths.rs` util. |
| ~~CFG-038~~ | ~~medium~~ **CLOSED 2026-08-19 — LANDED** | parity-bug | S | One unparseable key spec discards the whole `keybindings.json` — and applies it partially first — **CLOSED 2026-08-19: re-read at HEAD `4fb5e40` while repairing this row's citations after the `app.rs` split, and the row's recipe has landed verbatim, all three steps.** ~~**2026-08-14, still open; VERIFIED at HEAD by sweep 6 and this is the single highest-value unlanded item in area 05.**~~ **(a) skip-and-continue.** There is no longer a `parse_key_values`: every one of the **seven** `merge_json` bodies is a one-line delegation to a single shared `merge_entries` (`crates/cyrup-tui/src/keymap.rs:118-149`) — `Keymap` `:760`, `SelectKeymap` `:856`, `AutocompleteKeymap` `:954`, `ModelsKeymap` `:1043`, `SessionKeymap` `:1127`, `TreeKeymap` `:1270`, `EditorKeymap` `:1415` — and `merge_entries` reproduces pi's TWO drop shapes separately: an off-shape value skips the whole entry (`:129-135`, `key_specs` at `:87`), while an unparseable spec drops only that key and applies the rest (`:136-146`), which is behaviourally identical to pi's never-matching `KeyId`. **(b) the list is returned.** `merge_json` is `Result<Vec<KeybindingIssue>, TuiError>` (the type is `keymap.rs:58`), `App::load_keybindings_json` (`app/shell.rs:159-172`) CONCATENATES all six maps' issue lists instead of `?`-chaining them, and `InputEditor::merge_keybindings_json` (`editor.rs:418-426`) does the same for its two inner maps. **(c) `main.rs` names the ids.** `crates/cyrup/src/main.rs:1975-1986` now has two arms: `Err` really does mean the whole document was ignored, and `Ok(issues)` prints `warning: {path}: ignoring {issue}` per id. **The whole-document error stayed exactly where the row said it must** — `keybindings_object` (`keymap.rs:167-177`), pi's `loadRawConfig` `undefined` path, which is also where CFG-048's read-time migration sits (`:171`). Pinned by `one_bad_entry_does_not_discard_or_half_apply_the_document` (`crates/cyrup-tui/src/tests/keybindings.rs:165`) and `an_array_with_a_non_string_element_drops_the_whole_entry` (`:214`), plus the amended `malformed_keybindings_json_errors_cleanly` (`:132`), which now asserts a bad SPEC is reported rather than thrown. **DEVIATION, stated not smoothed:** the Verify asked for `crates/cyrup-tui/src/tests/keybindings.rs`; the pins are **in-src** at `crates/cyrup-tui/src/tests/keybindings.rs`, the same crate-privacy deviation CFG-051 records. pi `keybindings.ts:275-288`, `:328-336`, `:350-355` @v0.83.0; `packages/tui/src/keybindings.ts:243-256`. |
| ~~CFG-045~~ | ~~medium~~ **CLOSED 2026-08-14 — already-done** | not-ported | S | `doubleEscapeAction` is inert — the Escape handler has no double-escape and no bash-mode-exit branch — **CLOSED 2026-08-14**: sweep 8 read `crates/cyrup-tui/src/app.rs` (pre-split; that file is the `crates/cyrup-tui/src/app/` module tree since `40821ed`) at HEAD instead of the row. **BOTH halves this row calls missing are present**, landed under `TUI-009` as a port of `interactive-mode.ts:2569-2595` @v0.83.0: the **bash-mode-exit** branch at `app/input.rs:182-186` (cyrup derives the mode from the buffer the way pi's `onChange` does at `interactive-mode.ts:2621-2622`, so clearing the buffer *is* leaving the mode — the in-source comment says so) and the **500 ms double-Escape window** reading `doubleEscapeAction` at `app/input.rs:191-209`, with the `last_escape` field at `app/state.rs:121`, the `tree`/`fork`/`none` dispatch, and the `last_escape = None` reset on fire so a third press starts a new pair. `EffectiveSettings::double_escape_action` (`cyrup-config/src/settings.rs:883`) now has live consumers at `app/input.rs:192` and `app/execute_misc.rs:204-205` — so the "a `/settings` row is not a consumer" finding this row generated is discharged as well as recorded. **The status line below is stale.** — ~~**2026-08-14, still open**: sweeps 2 and 6 — unchanged at HEAD.~~ **FIX SITE: the `Action::Interrupt` arm in `crates/cyrup-tui/src/app/input.rs` plus a `last_escape_time` field** (pi `interactive-mode.ts:2570-2596` @v0.83.0). Outside cyrup-config/cyrup-resources. |
| ~~CFG-046~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | models.json string fields are not length-validated, so `"baseUrl": ""` rewrites every model to an empty endpoint — **CLOSED 2026-08-14**: sweep 1 — `ModelDefinition`/`ModelOverride`'s `context_window`/`max_tokens` are now `Option<i64>` (pi's `Type.Number()` is signed and the per-provider `<= 0` rejection depends on it), and `apply_model_override` carries a new `[CYRUP-DELTA]`: pi stores a negative override verbatim, `Model::context_window` is `u64`, so it saturates to 0. |
| ~~CFG-048~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | pi's sixth startup migration (`migrateKeybindingsConfigFile`, 59 legacy names) is not ported at write time or read time — **CLOSED 2026-08-14**: sweep 6 verified BOTH residual call sites live at HEAD (they landed in sweeps 3-5): write time at `crates/cyrup/src/migrations.rs:38` (`cyrup_config::migrate_keybindings_config_file(&dirs.agent_dir)`, between `migrate_tools_to_bin` and `migrate_extension_system`, pi's `:311`→`:312`→`:313`), read time at `crates/cyrup-tui/src/keymap.rs:91` inside `keybindings_object`; the false claim at `migrations.rs:9-10` is gone. The mechanism lives in `crates/cyrup-config/src/keybindings.rs` (see the prior note for why, and for the two corrections it carries — the `tui.editor.*` respelling instruction was STALE and correctly NOT followed, and there are **73** declared ids, 31 tui + 42 app). **The rename table was diffed pair-for-pair against `keybindings.ts:210-268` @v0.83.0 and is byte-identical at 59 entries in the same order — any surviving "~30-name rename table" phrasing anywhere in this directory or in a sweep assignment is WRONG.** Pinned by `crates/cyrup/src/migrations.rs::migrates_legacy_keybinding_ids_in_the_agent_dir` plus 11 tests in `cyrup-config/src/keybindings.rs`. |
| ~~CFG-049~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | Deprecation warnings are printed and immediately painted over — pi blocks startup on a keypress — **CLOSED 2026-08-14 (one residual carried, stated below)**: sweep 6 — ported `showDeprecationWarnings` in FULL, not only its text (`migrations.ts:277-298` @v0.83.0: prompt `:286`, the blocking promise `:288-296`, the trailing `console.log()` `:297`; awaited at `main.ts:838-840`). New `show_deprecation_warnings` + `deprecation_gate_block` + `wait_for_any_key` in `crates/cyrup/src/migrations.rs`: prints the block, prints `Press any key to continue...`, enters raw mode (crossterm via cyrup_tui's re-export; failure ignored, mirroring pi's optional `setRawMode?.()`), reads ONE byte, restores raw mode, prints the trailing blank line. `crates/cyrup/src/main.rs`'s interactive arm calls it in place of the bare `eprint!`, so startup blocks before TUI init. `grep -rn 'Press any key' crates` returned zero workspace-wide before this. **[CYRUP-DELTA]:** pi's `once("data")` never fires on an already-closed stdin, so upstream HANGS there; cyrup returns on a zero-length read. **RESIDUAL, explicitly NOT settled — a product decision, not a port decision:** the item's second clause asked to settle the rebrand of `MIGRATION_GUIDE_URL` / `EXTENSIONS_DOC_URL`, which still send cyrup users to `github.com/earendil-works/pi-mono`. Both were left pointing upstream rather than inventing a cyrup URL; **this needs the user, not another sweep.** ~~**COVERAGE GAP, stated so it is not mistaken for done:** the keypress BLOCK is not exercised by any test … the gate itself still needs the live-terminal run the item's Verify asks for.~~ **COVERAGE GAP CLOSED BY OBSERVATION 2026-08-14 (sweep 8) — `REPRO-LOG.md` §0c.** Instrument: **tmux** — a real terminal emulator on a real pty; `script -q /dev/null` still dies on the unanswered `ESC[6n` probe, exactly as `REPRO-LOG.md` §1 records. Scratch `HOME` carrying `$HOME/.cyrup/agent/hooks/legacy.sh`, launched as `env -i HOME=… PATH=… TERM=xterm-256color TMPDIR=/tmp target/debug/cyrup` in a scratch cwd. **At T+3 s and again at T+9 s the pane showed pi's warning block plus `Press any key to continue...` and NOTHING ELSE — no TUI, pane alive; after `tmux send-keys x` the TUI took the terminal.** That is the gate at `crates/cyrup/src/migrations.rs:292-303` + `wait_for_any_key` `:327-335`, called from `crates/cyrup/src/main.rs:709`. **NEGATIVE CONTROL, and it was found by the agent's own first fixture being wrong:** with `hooks/` planted at `$HOME/.cyrup/hooks` (the agent dir is `$HOME/.cyrup/agent`, `cyrup-config/src/env.rs:178`) **no warnings were collected and the TUI painted immediately** — so presence AND absence are both observed, and the null fixture that would have produced a false "the gate is broken" is on the record. **TWO RESIDUALS SURVIVE AND STAY STATED.** (a) The `MIGRATION_GUIDE_URL` / `EXTENSIONS_DOC_URL` rebrand — still `github.com/earendil-works/pi-mono`, **visible verbatim in the live transcript** — is untouched and **needs the user, not a sweep**. (b) The **`[CYRUP-DELTA]` zero-length-read path was NOT exercised**: driving it needs a closed stdin on the interactive arm, which the tmux fixture does not construct. **BLOCKED for that pass, not passed.** |
| ~~CFG-018~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | Glob scope patterns no longer short-circuit on an exact model reference — **CLOSED 2026-08-14**: sweep 1. |
| ~~CFG-019~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | `defaultModelPerProvider` still stale — `xai` id retired, `radius` arm missing — **CLOSED 2026-08-14**: sweep 1. |
| ~~CFG-007~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | `AuthStore` re-reads auth.json per query and coerces errors to "not configured" — **CLOSED 2026-08-14**: sweep 1 — behavioural consequence for the verification phase: `AuthStore` no longer re-reads `auth.json` per query, so a test that constructs the store and then writes `auth.json` out of band must call the new `AuthStore::reload()`. This is pi's semantics (`read()` is `this.data[provider]`), not a shortcut. |
| ~~CFG-008~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | Model-scope resolution drops every diagnostic — **CLOSED 2026-08-14**: sweep 6 — the residual (the diagnostic TYPE living in the bin as a replay of the resolver) is gone. `crates/cyrup/src/main.rs` now calls `ModelResolver::resolve_scope_reporting` ONCE through a new `resolve_scoped_models_reporting` returning `(models, diagnostics)`, the way pi's `resolveModelScope` returns `{ scopedModels, diagnostics }` (`model-resolver.ts:269-350` @v0.83.0; render loop `:355-361`; `main.ts:741-743`). ~75 lines of replay deleted: the per-pattern one-element-slice emptiness loop, `is_glob_pattern`, and the hand-rolled `invalid_thinking_level_message` recursion that re-derived `parseModelPattern`'s colon-stripping. **Two notes the deleted replay carried were verified STALE at HEAD and removed rather than migrated:** its `CYRUP-DELTA` about a missing `findExactModelReferenceMatch` short-circuit (`:297-303`) — CFG-018 landed it in `resolve_scope_reporting`'s glob arm — and its claim that cyrup-config abbreviates the invalid-thinking-level sentence (`parse_pattern` mints pi's full text, `:243`). Pinned by `crates/cyrup/src/main.rs::one_scope_pass_returns_pis_models_and_diagnostics_together`. |
| ~~CFG-006~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | M | `websocketConnectTimeoutMs` never reaches the HTTP/stream layer — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD by an earlier sweep; the row was stale. The item's cyrup evidence ("`grep -rn websocket_connect_timeout_ms crates/` returns exactly three lines — nothing assigns it") no longer holds: `crates/cyrup-session-svc/src/builder.rs:1481-1482` reads `eff.websocket_connect_timeout_ms()` onto the agent builder, cyrup-agent threads it to `StreamOptions` (`agent.rs:866`, `:2350-2355`), and `crates/cyrup-session-svc/src/tests/round8_postrun.rs:341` (`websocket_connect_timeout_setting_reaches_the_providers_stream_options`) pins it end to end. **The item's Verify note about residual test debt from the CLOSED retry half — nothing proves `max_retry_delay_ms` reaches the retry loop (blind spot 6) — is untouched and is RE-FILED as `CFG-053` rather than closed with this row.** |
| ~~CFG-039~~ | ~~medium~~ **CLOSED 2026-08-15 — REFUTED (already closed at HEAD; nothing was changed this pass)** | upstream-drift | M | models.json `samplingParams` on model definitions and modelOverrides is silently dropped — **the row's "HARD-BLOCKED, re-verified by sweep 6" framing is VOID at `831321b`. The prerequisite it named landed and so did the rest.** `cyrup_provider::Model::sampling_params` exists (`crates/cyrup-provider/src/model.rs:66-73`, cited to pi `types.ts:801-802` @v0.84.1) and so does `StreamOptions::sampling_params` (`stream.rs:167-178`), merged per key by `utils/simple_options.rs:64-105` (`merge_sampling_params`, pi `simple-options.ts:27-33`) — the area-01 half, landed as `AGENT-026`. The **config** half landed with it and is NOT provider-side work: `ModelDefinition::sampling_params` (`crates/cyrup-config/src/model.rs:1702-1709`, cited to `model-config.ts:167` @v0.84.1) and `ModelOverride::sampling_params` (`:1736-1741`, `:188`); `apply_models_json` copies it verbatim from the definition (`:2473-2477`, pi `provider-composer.ts:158`) and **merges per key** on the override path (`:2526-2535`, pi `:123-125`) — merge, not replace, which is the one behaviour the row said would be got wrong. Tests: `crates/cyrup-config/src/tests/models_json_provider.rs:570-641` (a `models[]` `samplingParams` reaches the wire impl; a `modelOverrides` entry adding `top_k` leaves `min_p` in place) and `:642-660` (no inheritance from a provider block or a same-id built-in — pi's `ModelDefinitionSchema` has no provider-level twin), plus `crates/cyrup-provider/src/tests/sampling_params.rs` for the wire end. **The lesson is the row's, not the code's:** a HARD-BLOCKED marker naming another area's prerequisite must be re-checked at HEAD before it is trusted — this one had been discharged for a whole batch. |
| CFG-020 | medium | not-ported | L | No `ModelRuntime` type and no availability snapshot — **2026-08-14, still open**: sweeps 2 and 6 — not reached; L, three crates, requires reading `model-runtime.ts` at v0.84.1 (+356 lines). Not tail-sized. **Its prerequisite mechanism — the revision-checked snapshot in `FileModelsStore` — is now complete except for CFG-042's cancellation parameter.** |
| ~~CFG-003~~ | ~~medium~~ **CLOSED 2026-08-15** | not-ported | ~~L~~ S | Settings `packages` are resolved but never auto-installed — **CLOSED 2026-08-15**: the git arm is ported. **The "three-crate async restructuring" blocker was FALSE and is struck**: `discover_blocking` is synchronous *inside* `spawn_blocking` and `PackageManager::install`'s body is the synchronous `install_blocking` (`install.rs`) — `git_clone` is blocking gix, not async — so the install runs on the thread discovery is already on, with no future to drop mid-clone and no `.await` between the clone and the walk. And pi has **no prompt on this path at all**: the session path is `packageManager.resolve()` with NO `onMissing` (`resource-loader.ts:403` and `:549` @v0.83.0), so `installMissing` (`package-manager.ts:1260-1271`) installs unconditionally unless `isOfflineModeEnabled()` (`:42-46`); the ONLY `onMissing` caller upstream is the startup-theme pass, which answers `"skip"` (`cli/startup-ui.ts:73`). Ported: `install_declared_git_package` in `crates/cyrup-resources/src/discovery.rs` (pi `installGit`'s fresh-clone path `:1831-1837` — `ensureGitIgnore(gitRoot)` then the clone — reusing `package::install::{ensure_git_ignore, git_clone}`), wired into `resolve_configured_package`'s git arm exactly where pi's `if (!existsSync(installedPath))` sits (`:1287-1291`). The gate is one field, `DiscoveryConfig::install_missing_packages`, carrying BOTH of upstream's halves: `true` = pi's resource-loader caller, `false` = pi's startup-ui caller, and `false` is the default so no caller gets unasked-for network. Threaded `SessionConfig::install_missing_packages` → `builder.rs` → discovery, set by the bin to `!(--offline‖CYRUP_OFFLINE‖PI_OFFLINE)` (`crates/cyrup/src/main.rs`, beside `to_session_config_with_diagnostics`) — pi's only gate, so no settings key was invented. **No registry row is written** (pi has no registry; the declaration IS the record, and a row would make `cyrup remove` fight `settings.json`). **Two `[CYRUP-DELTA]`s remain, both stated in-source:** a declined install is a loud diagnostic where pi `continue`s silently (`:1290`), and a FAILED install is a diagnostic where pi's `throw` (`:1849`) takes the whole session build down. Note also that pi puts **no timeout on `git clone`** (`runCommand`, `:2628-2638`, has no `timeoutMs` — `NETWORK_TIMEOUT_MS` is applied only to the capture variants), so cyrup is not the more blocking of the two. Tests: `cfg003_an_open_gate_attempts_the_install_and_reports_its_failure` (RED at HEAD — the message was "run `cyrup install`", proving the arm was never entered), `cfg003_install_declared_git_package_materializes_the_tree` (real gix clone over `file://`; **coverage, not red-before** — new API) and the closed-gate half folded into `cfg003_uninstallable_settings_declared_package_is_an_error_diagnostic`, plus `cfg003_the_install_gate_reaches_discovery_in_both_directions` in `cyrup-session-svc/src/tests/settings_resolve.rs` for the threading. **HERMETICITY CONSTRAINT worth keeping:** a settings string can never name a LOCAL git repo, because `file://` is local to `isLocalPath` (paths.ts:41-55) on both sides — the only loopback spelling that reaches the git arm is `git:localhost/<u>/<r>` (→ `https://localhost/...`), which is why the end-to-end tests assert a FAILED clone and the successful clone is driven one layer down. **RESIDUAL, explicitly not closed:** the bin's one-line `config.install_missing_packages = !overrides.offline` has no test — there is no seam to drive it without spawning the binary — and the npm/OCI arms stay unsupported (R-09-021, CFG-009). |
| ~~CFG-005~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | L | Two multi-prompt api-key login flows unported (`ApiKeyAuth` has no `login` member) — **REFUTED, CLOSED 2026-08-14**: sweep 1 closed the flows; sweep 6 verified the recorded residual is FALSE at HEAD on both counts. `EnvKeyAuth` has `supports_login` (unconditionally true, per `envApiKeyAuth` always defining `login`, pi `auth/helpers.ts:12-15` @v0.83.0) and a verbatim `login` at `crates/cyrup-provider/src/auth/helpers.rs:50-75`, and `api_key_strategy_supports_login` is **deleted** — the two things ADR-0010 step 2 was blocking on. Pinned by `crates/cyrup-provider/src/tests/api_key_login.rs`. |
| ~~CFG-009~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | An `npm:` package source fails with the misleading message "unsupported source (OCI deferred)" — **CLOSED 2026-08-14**: sweep 1 — the message half is fixed and the variant is `ResourceError::UnsupportedNpm`; any other area file quoting "unsupported source (OCI deferred)" for an npm source is stale. The "Dangling consequence" line (`EffectiveSettings::npm_command()` has zero consumers) is unchanged and points at CFG-015. |
| ~~CFG-013~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `TrustStore::nearest` reads trust.json without the file lock — **CLOSED 2026-08-14**: sweep 1. |
| ~~CFG-016~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `${0:-default}` emitted literally instead of substituting — **CLOSED 2026-08-14**: sweep 1 — `match_brace_form`'s signature now takes `all_args`, which is what CFG-017's Fix asked for; the `${@:N}` / `${@:N:L}` slice family was re-verified unaffected (`${@:1:-2}` matches neither of pi's alternatives and stays literal on both sides). |
| ~~CFG-017~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `${@:-default}` / `${ARGUMENTS:-default}` prompt-template forms unsupported — **CLOSED 2026-08-14**: sweep 1 — `match_brace_form`'s signature now takes `all_args`, which is what CFG-017's Fix asked for; the `${@:N}` / `${@:N:L}` slice family was re-verified unaffected (`${@:1:-2}` matches neither of pi's alternatives and stays literal on both sides). |
| ~~CFG-028~~ | ~~low~~ **CLOSED 2026-08-14** | cyrup-original | S | Config-value `!command` resolution blocks a tokio worker for up to 10 s — **CLOSED 2026-08-14**: sweep 1. |
| ~~CFG-030~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Non-object top-level `settings.json` degraded to `{}` with no load error — **CLOSED 2026-08-14**: sweep 1. |
| ~~CFG-040~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | `markdown.mermaid` settings key and its getter/setter are absent — **CLOSED 2026-08-14**: sweep 1. |
| ~~CFG-041~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | `default_model_per_provider` missing v0.84.1's `baseten` and `qwen-token-plan-individual` — **CLOSED 2026-08-14**: sweep 1. |
| ~~CFG-043~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | An invalid `models.json` reports a serde parse error instead of pi's per-field schema report — **CLOSED 2026-08-14**: sweep 1 — the one honest gap in the new schema validator is recorded: `compat` is left to serde with a `[CYRUP-DELTA]`, because upstream's `ProviderCompatSchema` is a three-arm union of ~40 optional keys whose cyrup definition lives in `cyrup_provider::api::compat`. The typebox message strings come from the LIBRARY and could not be verified against pi source at the tag — only the surrounding format (`  - <path>: <message>`) is cited, from model-config.ts:274-277. |
| ~~CFG-044~~ | ~~low~~ **CLOSED 2026-08-14** | cyrup-original | S | Three `auth-storage.ts` provenance cites resolve to nothing upstream, and `get_auth_status` is dead — **CLOSED 2026-08-14**: sweep 1 — closed in cyrup-config. Its last clause (updating the dangling doc at `crates/cyrup-tui/src/auth_select.rs:39-42`, which still names `auth.rs::get_auth_status`) is handed to area 07 explicitly rather than left as a silent residual. |
| ~~CFG-047~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | S | Three built-in slash-command metadata divergences (`/model`, `/login`, `/reload`) — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD under `TUI-025`; the row was stale. `crates/cyrup-tui/src/commands.rs:49-72` carries `arg_cmd("model", …, "<provider/model>")`, `arg_cmd("login", "Configure provider authentication", "<provider>")` and `/reload`'s full description ending "themes, and context files" — all three, each with an in-code note. **The behaviour-vs-string question the item asked to resolve first is answered in-code: `/reload` genuinely does reload context files, so the full description is honest** (pi `slash-commands.ts:18-41` @v0.83.0). |
| ~~CFG-050~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `migrate_tools_to_bin` moves the managed `fd`/`rg` binaries with no completion notice — **CLOSED 2026-08-14**: sweep 6 — `migrate_tools_to_bin` (`crates/cyrup/src/migrations.rs`) now tracks pi's `movedAny` and emits `Migrated managed binaries tools/ → bin/` through `crate::output_guard::emit_stray_line`, the same guard the sibling `commands/ → prompts/` line uses, so the notice is rerouted to stderr under PRINT/JSON/RPC and cannot corrupt a machine-readable stdout. The flag is set only on a successful `rename`, never on the collision-delete path, matching pi's `:203-208`. **Cites corrected:** at v0.83.0 the function is `migrations.ts:177-216`, `let movedAny = false` is `:185`, the assignment is `:198` (the item said `:203`) and the notice is `:213-215`. Pinned by `announces_a_managed_binary_move_exactly_once` (real move → true + file moved; second run → false; collision-only pass deletes the stale source WITHOUT claiming a move). |
| ~~CFG-051~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | The migrated-credentials notice is written to stderr pre-TUI instead of into the transcript, on a wrong cite — **CLOSED 2026-08-14 (sweep 8) — the residual is closed on BOTH halves.** **(1) LIVE.** A scratch `HOME` with a legacy `oauth.json` (anthropic) plus `settings.json.apiKeys` (openai), run `--offline`: the running UI rendered `Warning: Migrated credentials to auth.json: anthropic, openai`, `auth.json` was written, `oauth.json.migrated` renamed and `apiKeys` stripped. **Proved TUI-owned rather than pre-TUI stderr residue by resizing mid-session** (120x40 → 90x28): the line re-laid-out from screen row 29 to row 4, and a following Ctrl+O grew the transcript beneath it — stderr residue cannot reflow. The single-provider case renders the singular form. Transcript in `REPRO-LOG.md` §0c. **(2) PIN.** `the_migrated_credential_notice_renders_first_and_verbatim_in_the_transcript` (`crates/cyrup-tui/src/transcript.rs:3466`) pushes both production warnings via `TranscriptView::push_warning` in `run_interactive` order (`main.rs:1940` then `:1946`), renders through `entry_lines` — the production path `app/draw.rs:166` uses — and asserts the notice renders BEFORE `modelFallbackMessage`, carries exactly ONE `Warning: ` prefix (the renderer must not re-prefix a verbatim `Entry::Warning`, `TUI-062`), and lands in the warning colour. Mutation-verified: breaking `Entry::Warning`'s renderer fails it. **DEVIATION, stated not smoothed:** the Verify asked for `crates/cyrup-tui/tests/`; `entry_lines`/`TranscriptView::lines` are crate-private, so the pin is **in-src** rather than widening the public API for a test. **Superseded sweep-6 text follows.** — ~~**PARTIALLY CLOSED 2026-08-14**~~: sweep 6 — the notice moved off the pre-TUI stderr path into the transcript. New `migrated_credentials_warning` in `crates/cyrup/src/main.rs`; `run_interactive` gained a `migrated_providers: Vec<String>` parameter (pi's `InteractiveModeOptions.migratedProviders`, threaded from `main.ts:607`) and pushes the line as a transcript warning FIRST in pi's startup-warning block, ahead of `modelFallbackMessage` (`interactive-mode.ts:874-876`, `:308`, `:883-885`). The `eprintln!` and its wrong cite (`interactive-mode.ts:797`, which is `await this.rebindCurrentSession()` at the tag) are deleted; **the item's own cite `:872-875` is also wrong — that is the destructuring line — the warning is `:874-876`.** Pinned by `the_migrated_credential_notice_is_pis_line_and_is_absent_when_nothing_moved` (exact string, the `, ` join, the empty case). **RESIDUAL, unambiguous: the item's Verify wants a rendered-transcript assertion in `crates/cyrup-tui/tests/` plus a live-terminal confirmation, and NEITHER exists** — the push is verified structurally (it sits beside the existing `modelFallbackMessage` push inside `run_interactive`) and the string is unit-tested, but nothing asserts a RENDERED transcript line. **FIX SITE of the residual: `crates/cyrup-tui`.** |
| CFG-014 | low | not-ported | M | `showCacheMissNotices` and prompt-cache-miss tracking absent — **EVIDENCE CORRECTED 2026-08-14**: sweep 6 — the headline "grep returns ZERO at HEAD" is STALE: the accessor exists, `EffectiveSettings::show_cache_miss_notices` at `crates/cyrup-config/src/settings.rs:572` (declared `:561`; pi `settings-manager.ts:96`, getter `:850-852`, setter `:872-875`). **The gap is now the CONSUMER half only, and `crates/cyrup-provider/src/cache_stats.rs:38` already names it as waiting on cyrup-tui.** Both halves are outside cyrup-config/cyrup-resources. *The same correction applies to the "47-key sweep found three keys with zero occurrences" claim repeated in `CFG-021`'s row.* |
| CFG-015 | low | not-ported | M | Five unconsumed settings accessors, incl. `lastChangelogVersion` and `collapseChangelog` — **NARROWED 2026-08-14**: sweep 6 — `warnings().anthropic_extra_usage` now HAS real consumers (`cyrup-tui/src/app/selectors.rs:290`, `app/execute_misc.rs:209`, `app/run_arms.rs:76-77` and `:253`) and comes off the unconsumed list. **FOUR remain: `code_block_indent`, `last_changelog_version`, `collapse_changelog` (a `/settings` display row only) and `npm_command`.** **FIX SITE: every remaining consumption site is in `crates/cyrup-tui` — NOT cyrup-config.** Landing the accessors alone is the exact "a /settings row is not a consumer" failure this area's Coverage section records. (pi `settings-manager.ts:55`, `:59`, `:84`, `:99`, `:102` @v0.83.0.) |
| ~~CFG-027~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | M | A local package that is a bare extension directory contributes nothing — **CLOSED 2026-08-14**: sweep 1 (CFG-004's residual) + sweep 2 — closed in full. `resolve_configured_package` grew the third outcome the sweep-1 note predicted, as a `ConfiguredPackageResolution` enum (Tree / ExtensionFile / Skip): (1) a settings-declared LOCAL path that does not exist — or cannot be stat'ed at all, pi's `try/catch` — is now a SILENT SKIP, where cyrup emitted `"is not installed at this path — run \`cyrup install\`"`, a message doubly wrong for a local path; (2) a local entry that is a regular file registers directly into `ext_crate_paths` as an extension, never walked for a manifest and never filtered. Citations corrected to the re-derived v0.83.0 offsets: the function is package-manager.ts:1316-1345 (the in-code comment said :1301-1327), the missing-path guard :1324-1326, the FILE case :1330-1334 (item says :1331-1335), the bare-directory fallback :1338-1340 (item says :1338-1341). |
| ~~CFG-042~~ | ~~low~~ **CLOSED 2026-08-15** | upstream-drift | M | `FileModelsStore` does not normalize its path, cache by file revision, or accept cancellation — **CLOSED 2026-08-15: the last residual, the `signal` parameter, landed.** Sweeps 1 + 2 landed path normalization and the revision-checked snapshot; sweep 6 landed the insertion-ordered-map half (see the detail section, kept verbatim). **This pass landed pi's `ModelsStoreOperationOptions`** — re-derived from `git show v0.84.1:packages/ai/src/models-store.ts`, which is 45 lines and diffs cleanly against v0.83.0: the bag is `:16-18`, the three interface methods gain `options?:` at `:22-24`, and each implementation opens with `options?.signal?.throwIfAborted()` (`:31`, `:37`, `:42`). `crates/cyrup-provider/src/models_store.rs` now carries `ModelsStoreOperationOptions { signal: Option<CancelToken> }` with an associated `throw_if_aborted(Option<&Self>)` returning `ProviderError::Aborted` (code `aborted`, the `AbortError` counterpart), and all three `ModelsStore` methods take `options: Option<&ModelsStoreOperationOptions>` — `Option<&_>` is the port of `options?:`, so `None` is pi's `undefined` and every existing call site reads exactly as pi's do. `ProviderModelsStore` forwards it. `FileModelsStore` (`crates/cyrup-config/src/models_store.rs`) honours pi's PLACEMENT, which is the part with behaviour in it: `read` checks twice (`models-store.ts:85` at the head of `readLatest`, `:121` after it returns), and `write`/`delete` check **before** taking the `FileLock`, because pi hands `options` to `storage.withLockAsync(…, options)` (`:132`, `:143`) — a check after acquisition would block every other process for the duration of a write nobody wants. Tests: `cfg042_an_aborted_signal_rejects_the_operation_without_mutating`, `cfg042_absent_signal_and_live_signal_are_both_no_ops`, `cfg042_the_scoped_wrapper_forwards_the_signal` (cyrup-provider) and `cfg042_an_aborted_signal_is_refused_before_the_file_is_touched` (cyrup-config, which proves the placement by reopening the file and finding the pre-abort value). All four were RED by non-compilation before the change. **Two facts recorded rather than acted on:** (1) pi's own production caller of the signal is `packages/ai/src/models.ts:350`, `:352`, `:375` — the per-provider refresh **generation** machinery (`beginProviderRefresh`/`supersedeProviderRefresh`/`publicationChains`), which cyrup does not have in that shape, so every cyrup call site correctly passes `None`, exactly as pi's own `remote-catalog-provider.ts` sites do; the constant lands with that port. (2) **`ProviderModelsStore` was DELETED upstream at v0.84.1** (`git -C pi grep -n ProviderModelsStore v0.84.1 -- packages/` → nothing; the narrowing moved into the `refreshModels` context object). cyrup keeps it. That is a separate shape divergence, is NOT part of CFG-042, and is noted on the type itself so the next reader does not close it by finding the type. |
| CFG-053 | low | test-defect | S | `max_retry_delay_ms` is verified structurally only — nothing proves it reaches the retry loop (blind spot 6) — **FILED 2026-08-14 (sweep 6), open.** Re-filed from `CFG-006`'s Verify note when that row closed, rather than being closed with it: the `retry.provider.*` assignment at `crates/cyrup-session-svc/src/builder.rs` is asserted at the assignment site, and no test drives a retrying provider and observes the delay ceiling actually taking effect. Sibling of the `websocketConnectTimeoutMs` half, which IS pinned end to end (`round8_postrun.rs:341`). |
| ~~CFG-052~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED, and its premise is struck** | parity-bug | S | `parse_git_url` rejects the `github:user/repo` shorthand that `is_local_path` has already classified as NON-local — **CLOSED AS REFUTED 2026-08-14 (sweep 8), which is a correction to the ANALYSIS and must survive: this is not a defect, it is a faithful port of upstream's own inconsistency.** The row asserts "Upstream's `parseGitUrl` reaches `hostedGitInfo.fromUrl`, which resolves the `github:`/`gitlab:`/`bitbucket:` shorthands". **False at v0.83.0.** `parseGitUrl` opens with `if (!hasGitPrefix && !/^(https?\|ssh\|git):\/\//i.test(url)) return null;` (`packages/coding-agent/src/utils/git.ts:172-179`) and its **own doc comment** says verbatim: *"Without git: prefix, only accept explicit protocol URLs."* (`:165-171`). `github:owner/repo` has no `git:` prefix and no `://`, so **upstream returns null BEFORE reaching `fromUrl`**. pi's `parseSource` then takes `isLocalPath` → false (`utils/paths.ts:36-55` lists `github:`) → `parseGitUrl` → null → `return { type: "local", path: source }` (`core/package-manager.ts:1435-1459`). **So upstream ALSO classifies it non-local and then stores it as a local path.** cyrup's `git_url.rs:285-287` + `has_protocol_prefix` at `:367-373` and `source.rs:59-67` are verbatim ports. The "internally inconsistent state" the row describes is **upstream's**, and it was ALREADY pinned before this sweep by `cfg052_a_github_shorthand_is_a_local_path_exactly_as_upstream_leaves_it` (`crates/cyrup-resources/src/tests/resources.rs:2241`), which does presence-before-absence — `git:owner/repo` MUST still resolve through the hosted-git-info table, so the `None` below it is about the missing prefix and not a dead parser — covers all three shorthands, and cites `git.ts:177-179` and `package-manager.ts:1459`. **Superseded original text follows.** — ~~**FILED 2026-08-14 (sweep 6), open.**~~ `crates/cyrup-resources/src/package/git_url.rs::parse_git_url` returns `None` for `github:user/repo`: `has_git_prefix` is false (the prefix is `github:`, not `git:`) and `has_protocol_prefix` accepts only https/http/ssh/git schemes — so the source falls through to `PackageSource::Path` and to `package_identity`'s **local** arm. Upstream's `parseGitUrl` reaches `hostedGitInfo.fromUrl`, which resolves the `github:`/`gitlab:`/`bitbucket:` shorthands. `is_local_path` already treats `github:` as non-local (`paths.ts:41-55`), so cyrup is in an internally inconsistent state: not a local path, and stored as one. **Two functions ported from two different upstream files whose domains no longer meet.** Found while closing CFG-026 and deliberately filed rather than fixed (out of that item's scope); CFG-026's tests were kept off the shorthand rather than encode the current behaviour. |
| CFG-021 | low | upstream-drift | L | `tuiMode` / `fullscreenScrollbar` not modelled — **2026-08-14, still open**: sweeps 2 and 6 — unchanged; waits on the alt-screen renderer (ADR-0005 / area 07). `grep -rni 'tuiMode\|fullscreenScrollbar' crates` is still zero. The settings half alone would be another inert key — the exact failure this area's Coverage section records. *(Its restatement of the "three keys with zero occurrences" claim is stale for `showCacheMissNotices` — see CFG-014.)* |
| ~~CFG-054~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already closed at HEAD)** | cyrup-original | S | Installed package working tree lands under a doubled `packages/packages/` segment — **the row was STALE; verified fixed at HEAD `68bbd39` before any work was attempted.** `PackageStore::packages_root(Global)` is `self.global_dir.clone()` (`crates/cyrup-resources/src/package/store.rs`), carrying a CFG-054 note that names the fix and the reason the `.join("packages")` was dropped rather than re-rooting the store at `agent_dir` (keeping `CYRUP_PACKAGE_DIR` meaningful), and citing pi's own flat roots (`join(this.agentDir,"git")` `package-manager.ts:2050`, npm `:1970`). The migration the item did not ask for is there too — `migrate_legacy_doubled_packages_root` (same file) with a completion notice through `output_guard::emit_stray_line`, called from `run_migrations` (`crates/cyrup/src/migrations.rs`) **and** from `subcommands::run`, because the package verbs are dispatched before migrations (`main.rs`). Pinned by `a_global_package_tree_and_its_registry_sit_at_the_same_level` and `the_legacy_doubled_root_is_migrated_once_and_never_clobbers`. |
| ~~CFG-055~~ | ~~medium~~ **CLOSED 2026-08-15 — REFUTED (already closed at HEAD)** | cyrup-original | S | `cyrup remove` may not match the `PackageId` that `cyrup install` stored — **the row was STALE; verified fixed at HEAD `68bbd39`.** `remove_candidate_ids` (`crates/cyrup/src/subcommands.rs`) routes the argument through `PackageSource::parse` → `package_id()` FIRST and keeps `PackageId::from(raw)` as a fallback for rows an older build wrote, and `update`'s positional target uses the same normalization. **The upstream leg the row said was "not established" IS established in-source:** pi matches on a normalized key too — `packageSourcesMatch` (`package-manager.ts:1418-1422` @v0.83.0) compares `getSourceMatchKeyForSettings` against `getSourceMatchKeyForInput` (`:1362-1383`), both reducing to `git:<host>/<path>` / `local:<resolved>`, and `update` goes through `getPackageIdentity` (`:1051`). Pinned by `remove_matches_the_normalized_id_install_wrote_with_a_raw_fallback`. |
| ~~CFG-056~~ | ~~high~~ **CLOSED 2026-08-14 — FIXED THIS PASS** | parity-bug | S | `defaultThinkingLevel`'s unset-fallback was `off`; pi's is `medium`, so every default session started with reasoning disabled — **FIXED 2026-08-14** (surface-enumeration sweep, settings.json surface). `crates/cyrup-config/src/defaults.rs` is new and ports pi's one-export `core/defaults.ts`; `EffectiveSettings::default_thinking_level()` now returns `Option<ModelThinkingLevel>` as pi's getter returns `ThinkingLevel \| undefined`, and the three `builder.rs` sites plus `model.rs`'s `default_level` name the fallback explicitly. |
| ~~CFG-057~~ | ~~medium~~ **CLOSED 2026-08-14 — FIXED THIS PASS** | parity-bug | S | `httpProxy` was read from the MERGED view, so a project `.cyrup/settings.json` could rewrite the session's egress; pi reads it off the global document only — **FIXED 2026-08-14**: one entry in `GLOBAL_ONLY_KEYS` plus a red-before test. |
| ~~CFG-058~~ | ~~medium~~ **CLOSED 2026-08-15 — REFUTED (the premise does not hold at HEAD)** | not-ported | S | `websocketConnectTimeoutMs` has no 15 000 ms default at the connect site — **REFUTED 2026-08-15: cyrup has no connect site.** The row's upstream leg is entirely correct and was re-derived at v0.83.0 this pass: `const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS = 15_000;` (`packages/ai/src/api/openai-codex-responses.ts:64`) applied as the parameter default at `:1039`, with `if (connectTimeoutMs > 0)` at `:1102` making an explicit `0` mean *disabled*; the getter returns `undefined` unset (`settings-manager.ts:842-844`) and `sdk.ts:309-315` threads that `undefined` through, so the default genuinely lives at the socket; `docs/settings.md:172` documents `15000`. **The IMPACT clause is what fails.** "An unset key means an unbounded WebSocket handshake" requires cyrup to open a handshake, and it never does: the port has no WebSocket client at all — `crates/cyrup-provider/src/api/openai_codex_responses.rs`'s "Mechanism deltas" header records that every transport resolves to SSE, which is **pi's own documented behaviour in a runtime that exposes no WebSocket constructor** (`connectWebSocket` throws at `:1043-1045`, `stream` records the failure and breaks to the SSE path at `:358-377`). So there is no unbounded handshake, and there is nowhere to apply a 15 000 ms floor. **Landing the constant anyway would have been the defect:** defaulting the `Option` to `Some(15_000)` — in `Default`, in `build_base_options`, or in the settings thread — erases the unset-vs-explicitly-15000 distinction pi's `??` chain depends on, and applies a handshake bound to a path that performs no handshake. The residual RISK (a value threaded from settings to a consumer that does not exist) is recorded where the next person will hit it: `StreamOptions::websocket_connect_timeout_ms` (`crates/cyrup-provider/src/stream.rs:212-241`) now states that the constant lands **with** the WebSocket transport, in the same change, or nowhere. **`CFG-006`'s closure was correct**, not premature: the threading it verified is all there is to verify until a transport exists. |
| ~~CFG-059~~ | ~~medium~~ **CLOSED 2026-08-15 — REFUTED (already closed at HEAD)** | cyrup-original | M | A third, persistent `cli` settings layer sits above project in the precedence chain; pi has no CLI settings tier at all — **CLOSED AS REFUTED 2026-08-15** (batch B, cyrup-config slice): the row was stale, the layer is gone. `SettingsManager::load(store, project_trusted)` takes TWO arguments at HEAD (`crates/cyrup-config/src/settings.rs`), the struct has exactly `global` + `project` + `effective`, `recompute` merges `global ◁ project` only, and `grep -rn 'cli_settings' crates/` no longer resolves to a settings LAYER: the surviving `SessionBuilder::cli_settings` / `SessionFactory::cli_settings` setters route to the transient `apply_overrides` at `cyrup-session-svc/src/builder.rs:677-678` (`if !self.cli_settings.is_empty() { settings.apply_overrides(&self.cli_settings) }`), which is pi's `applyOverrides` (`settings-manager.ts:508-510` @v0.83.0) — merged onto the already-merged view and discarded by the next recompute. The removal is documented in place on the `SettingsManager` doc comment and on `apply_overrides`, both of which name CFG-059. Nothing was changed this pass. |
| ~~CFG-060~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `EffectiveSettings::http_proxy`'s env fallback inverts pi's `??=` precedence — **CLOSED 2026-08-15** (batch B): confirmed at HEAD (`.or_else(\|\| env.http_proxy.clone())`) and closed by taking the item's SECOND option — **the env leg and the `&EnvVars` parameter are deleted**, so the accessor is now pi's `getGlobalSettings().httpProxy` and nothing else (`main.ts:537`/`:801` @v0.83.0). Inverting the `or_else`, the item's first option, would have been a NEW divergence and the argument is kept in-source: `applyHttpProxySettings` fills `HTTP_PROXY` and `HTTPS_PROXY` **independently** (`http-dispatcher.ts:43-48`), so with an ambient `HTTP_PROXY` and a set `httpProxy` upstream still routes https targets through the SETTING; returning the ambient value from this accessor would have installed it as `configure_http_proxy`'s value for both names and lost the setting for https entirely. The ambient-wins half of `??=` stays where it was already ported once — `get_proxy_env` (`cyrup-provider/src/utils/node_http_proxy.rs`) consults `configured_http_proxy()` only after all four ambient lookups miss. Both call sites dropped their `EnvVars::default()` argument (`crates/cyrup/src/main.rs`, `crates/cyrup-session-svc/src/builder.rs`) and the two comments that explained why it was passed are rewritten rather than left stale. Test: `http_proxy_is_the_setting_alone_and_takes_no_environment` — **labelled in-file as COVERAGE, not proof**: the fix is a signature removal, so no test can be written against the pre-fix API. |
| ~~CFG-061~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `EffectiveSettings::packages()` discards the whole array on one malformed entry — **CLOSED 2026-08-15** (batch B): confirmed at HEAD and routed through the per-entry port. `EffectiveSettings::packages()` is now `self.merged.packages()` and a new `EffectiveSettings::packages_with_errors()` exposes the diagnostics, matching pi's `getPackages` (`settings-manager.ts:969-971` @v0.83.0 — `[...(this.settings.packages ?? [])]`, a verbatim copy that never parses, so a bad entry travels downstream and is rejected alone). RED-before test `one_malformed_package_entry_does_not_discard_the_other_nine` (ten entries, entry 3 a number → 9 + 1 diagnostic; pre-fix 0 + silence). |
| ~~CFG-062~~ | ~~low~~ **CLOSED 2026-08-15 — one clause of its Impact REFUTED** | parity-bug | S | Clearing a string/array settings key writes JSON `null`; pi's `JSON.stringify` drops the key — **CLOSED 2026-08-15** (batch B). **Write half — real, confirmed, fixed on BOTH paths.** `SettingsManager::set` now removes the key when the serialized value is `Value::Null`, and `set_value_at_path` (shared by `set_nested` and `persist_nested`) does the same at a nested LEAF — the item named only `set`, but `persistScopedSettings` writes the nested object through the same `JSON.stringify(mergedSettings, null, 2)` (`settings-manager.ts:605` @v0.83.0), which omits undefined-valued properties at every depth, so the nested path had the identical defect. Upstream clearing setters: `setShellPath` `:883-887`, `setShellCommandPrefix` `:914-918`, `setNpmCommand` `:924-928`. RED-before test `clearing_a_key_removes_it_rather_than_writing_json_null` (also asserts the parent object survives an emptied leaf). No production caller passes `Null` today (`persist_nested`'s two callers write an array and a bool), so this closes the latency before a clear path exists, as the item's Fix asked. **Merge half — REFUTED, and the refutation is the durable finding.** The Impact's "cyrup has no such [undefined] skip" is unrepresentable: `serde_json` has no `undefined`, so a key absent from the project map is structurally skipped and pi's `:139-141` guard has no Rust counterpart to be missing. And "pi has no way to express that state at all" is false — a hand-written `"npmCommand": null` in a project file parses to `null`, `overrideValue === undefined` is false, so pi's `deepMergeSettings` takes the null too and `getNpmCommand`'s `this.settings.npmCommand ? … : undefined` reads it as unset. cyrup's `deep_merge` `(_, over) => over.clone()` and `npm_command`'s `as_array` do exactly the same. Pinned as a NEGATIVE test, `a_project_null_blanks_a_global_value_on_both_sides`, so nobody "fixes" the merge toward a divergence. |
| CFG-063 | low | not-ported | S | `PI_TUI_DEBUG` and `PI_DEBUG_REDRAW` — the two upstream render-debug env vars — have no counterpart, so the cursor/viewport bug class has no instrument — **filed 2026-08-14** (env-var surface). **FIX SITE: `crates/cyrup-tui` (area 07).** Sibling of `TUI-040`. |
| CFG-064 | low | not-ported | S | `isWindowsTerminalSession()` is unported — `SSH_CLIENT` / `SSH_CONNECTION` / `SSH_TTY` are read nowhere — so Ctrl+Backspace degrades to Backspace on Windows Terminal, and the bug direction flips over SSH — **filed 2026-08-14** (env-var surface). **FIX SITE: `crates/cyrup-tui` (area 07).** |
| CFG-065 | low | not-ported | S | `isWslEnvironment()` (`WSL_DISTRO_NAME` / `WSL_INTEROP`) and its git-HEAD polling fallback are unported, so the footer branch indicator goes stale on `/mnt/<drive>` repos where inotify never fires — **filed 2026-08-14** (env-var surface). **FIX SITE: `crates/cyrup-tui` (area 07).** |
| CFG-066 | low | not-ported | S | The clipboard backend's two load gates — `TERMUX_VERSION` and `hasDisplay` (`DISPLAY` / `WAYLAND_DISPLAY`) — are unported, so the backend is attempted unconditionally on headless Linux and under Termux — **filed 2026-08-14** (env-var surface). Distinct from the known clipboard-TEXT gap at `12-upstream-drift-pi-core.md:820-828`. |
| CFG-067 | medium | not-ported | M | Twelve `pi-subagents` env vars have no `CYRUP_` counterpart — three of them are budget/ceiling caps and one is a security kill switch — **filed 2026-08-14** (env-var surface). **FIX SITE: `crates/cyrup-ext-subagents` (area 09); this row exists so the enumeration is not lost while area 09 has no item for any of them.** |
| CFG-068 | medium | cyrup-original | S | `CYRUP_HOME` is invented, live in shipped builds, and takes precedence over `$HOME` at four sites at once — undocumented in `--help` and described in-source as a test knob — **filed 2026-08-14** (env-var surface). Needs an owner decision: promote it or confine it to test builds. |
| ~~CFG-069~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `AI_AGENT` is written into every bash and subagent child; the KEY does not exist at the ported tag (it is a v0.84.1 addition) and the `[CYRUP-DELTA]` lines flag only its VALUE — **CLOSED 2026-08-15, and the row was HALF DONE when it reached this pass.** Upstream re-derived at both tags: `git -C pi grep -n 'AI_AGENT' v0.83.0 -- packages/` → 0 hits, and `git show v0.83.0:.../cli.ts` line 13 is `process.env.PI_CODING_AGENT = "true";` with nothing after it, while v0.84.1's `cli.ts:14` adds `process.env.AI_AGENT = "pi";`. Of the three sites the row names, **`crates/cyrup-session-svc/src/bash.rs` was already fixed in batch B** (delta at `:164-172`, test `the_forward_ported_ai_agent_marker_names_its_key_and_its_tag`) and the row did not say so. The other two landed here: `crates/cyrup-tools/src/tools/bash.rs` (delta above the `env.push`, test `cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag` in `src/tests/bash_session_env.rs`) and `crates/cyrup-ext-subagents/src/exec/mod.rs` (delta above the `env_overlay.insert`, test `cfg069_the_spawn_overlay_delta_names_the_forward_ported_key_and_its_tag`). All three deltas now name the KEY, the tag it comes from (`@v0.84.1`) and its ABSENCE at `v0.83.0`; each test slices the source between the last `[CYRUP-DELTA` marker and the write itself, so prose elsewhere in the file cannot satisfy it — `CYRUP-DELTA` is the grep the parity sweeps run. Each also asserts `PI_CODING_AGENT` is still written beside it, so the test cannot be satisfied by deleting the forward-ported marker. **Taken deliberately as a recorded forward-port, not pinned to a v0.84.1 uplift item** — the marker is how a hook or script tells an agent shell from a human one, and removing it would leave the uplift with a hole. **Site count corrected: the fix spans three crates, not the one this slice was routed for (`cyrup-tools`).** |
| CFG-070 | low | cyrup-original | S | `AWS_CONFIG_FILE`, `AWS_SHARED_CREDENTIALS_FILE` and `APPDATA` are read by cyrup's hand-rolled credential resolvers and appear nowhere in pi's source, which inherits them from `@aws-sdk` / `google-auth-library` — **filed 2026-08-14** (env-var surface). **Correct as written — do NOT "fix" it by removing the reads.** Recorded so a later fidelity pass does not read them as unexplained branches. |
| ~~CFG-071~~ | ~~low~~ **CLOSED 2026-08-15 for the cyrup-original half; the upstream half stays with `EXT-027`** | cyrup-original | S | `XDG_CACHE_HOME` is a false name-match: cyrup reads it to site the WASM build cache, pi reads it to find the HuggingFace token file — **the double meaning is now recorded in the source, 2026-08-15.** Both legs re-derived: cyrup at `crates/cyrup-ext/src/build/cache.rs` (`ArtifactCache::default_location`), pi at `packages/coding-agent/src/extensions/llama/huggingface.ts:53` inside `findHuggingFaceToken` (declared `:46`), byte-identical at v0.83.0 and v0.84.1 — it is the third of four token-file candidates, after `HF_TOKEN` and `HF_TOKEN_PATH`/`HF_HOME`. `default_location` now carries a `[CYRUP-DELTA]` naming both directions, the owning item for pi's half (`EXT-027`, tracker `DRIFT-032`) and the adjacent grep trap (`HF_TOKEN` is a literal in cyrup, but only as a provider-catalog name, never as a token-file search path). Test `cfg071_the_build_cache_read_records_both_directions_of_the_name_match` (`crates/cyrup-ext/src/tests/env_surface_records.rs`) asserts each of those five facts is in the annotation AND that the read itself is still present — a record must not be "fixed" by deleting what it documents. RED before the pass: the entire doc was one line naming neither pi nor `EXT-027`. **Not fully closed:** the row closes only when `EXT-027` lands pi's half; the `EXT-027` assertion in the test is what keeps one direction from being read as closing the other. |
| ~~CFG-072~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `HOMEDRIVE` / `HOMEPATH` widen home resolution past pi, which reads neither — **CLOSED 2026-08-15, kept and stated, per the row's own preferred call.** Upstream re-derived: `git -C pi grep -c HOMEDRIVE v0.83.0 -- packages/` → 0 (same for `HOMEPATH`); pi resolves a home two ways and neither reads the pair — `normalizePath` calls Node's `homedir()` (`utils/paths.ts:67` @v0.83.0, `:88` @v0.84.1) and the display paths read `process.env.HOME \|\| process.env.USERPROFILE` (`modes/interactive/components/footer.ts:114`, `components/tree-selector.ts:940`). **One correction to the in-code rationale that was there:** the old comment claimed the pair is what libuv's `uv_os_homedir` falls back to. It is not — libuv checks `USERPROFILE` and then makes a *syscall* (`GetUserProfileDirectoryW`); the pair is the environment-visible spelling of the same directory, which is a weaker and more honest claim, and the delta now says so. The three-line branch inside `home_dir`'s `#[cfg(windows)]` block was extracted to a pure `windows_home_from(userprofile, homedrive, homepath)` under `#[cfg(any(windows, test))]` — **that extraction is the fix's load-bearing part**: the row's `Verify` ("HOME/USERPROFILE unset, HOMEDRIVE+HOMEPATH set") was otherwise unassertable from a unix host and unassertable anywhere without `unsafe` env mutation that races every other test in the binary. Tests `cfg072_homedrive_homepath_is_the_documented_fallback_after_userprofile` (precedence, the divergence case, and that a HALF-set pair widens nothing) and `cfg072_the_widening_carries_a_delta_naming_what_it_extends` (`crates/cyrup-tools/src/path.rs`). Both RED before the pass — the first did not compile, the second found no `CYRUP-DELTA`. |
| CFG-073 | low | cyrup-original | S | `NO_COLOR` and `CI` are read where pi reads neither — behaviour that changes under CI and not locally is the divergence class that hides — **filed 2026-08-14** (env-var surface). **FIX SITE: `crates/cyrup-ext-subagents` (area 09).** |
| CFG-074 | medium | cyrup-original | M | Nine invented env vars across the three sibling ports — `CYRUP_PERMISSION_SYSTEM` (an opt-in over a SECURITY gate), two permission-forwarding knobs upstream keeps as compile-time constants, three `CYRUP_INTERCOM_*` transport/broker vars, `CYRUP_SUBAGENT_AGENT_NAME`, `CYRUP_HOOK_WARMUP` and `CYRUP_SUBAGENTS_TEMP_ROOT` — **filed 2026-08-14** (env-var surface). Each is defensible; none is currently KNOWN, and each wants a `[CYRUP-DELTA]` naming the upstream file:line it replaces. |
| ~~CFG-075~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `CYRUP_EXT_ABI_FINGERPRINT` is the surface's only BUILD-time env dependency (`env!`, not `env::var`), so a missing value is a compile error rather than a runtime fallback — **CLOSED 2026-08-15: documented at the consumer, which is all the row asks for.** `crates/cyrup-ext/src/build/mod.rs`'s `ABI_FINGERPRINT` now states, next to the `env!`: that the value is substituted by `rustc` so a missing one fails the build with no branch to reach; that the supplier is this package's own `build.rs`, by the exact directive it emits (`cargo:rustc-env=CYRUP_EXT_ABI_FINGERPRINT=…` at `build.rs:37`) and the `unknown` sentinel at `:24` deliberately chosen over failing the build; that no upstream counterpart is possible (pi has no WASM component ABI); and — **verified this pass, not assumed** — that **neither cargo feature arm removes the dependency**, because `build/` is compiled through `lib.rs`'s bare `pub mod build;` and is therefore present in the `--no-default-features` build too, not only under `wasm-host`. Tests `cfg075_the_build_time_env_dependency_is_documented_at_its_consumer` (RED before the pass: the doc named neither the directive nor the compile-vs-runtime distinction) and `cfg075_the_build_script_still_emits_the_key_and_its_sentinel` (**labelled in-file as COVERAGE, not proof** — it could not go red, since `build.rs` already emitted both lines; it exists so a rename breaks a test in the same crate as the `env!`). `crates/cyrup-it/tests/ext/abi_fingerprint_invalidation.rs` continues to pin the end of the chain. |
| CFG-076 | low | cyrup-original | S | Three `PI_`→`CYRUP_` rename exceptions, one of which is a live inconsistency INSIDE cyrup: `CYRUP_AGENT_DIR` (short) in `cyrup-config` vs `CYRUP_CODING_AGENT_DIR` (long) in `cyrup-ext-subagents` — **filed 2026-08-14** (env-var surface). The other two are deliberate and are recorded so the mechanical diff stops scoring them as missing vars. |
| CFG-077 | low | cyrup-original | S | Prompt roots are scanned **recursively** and named by root-relative path, where pi's `loadTemplatesFromDir` is non-recursive and names by basename — **filed 2026-08-19 as a RECORD, not a defect.** `/flux/*` exists only because of this (`cyrup-flux/src/extension.rs:128-131` says so in as many words), so a parity sweep that "restores" pi's flat scan deletes fifteen shipped commands and renames every remaining template. **Correct as written — do NOT close it by removing the recursion.** Already pinned by nine `npt_*` tests. |

## CFG-035 — `.cyrup/SYSTEM.md` and `APPEND_SYSTEM.md` are never discovered — the trust-gated project system-prompt override is inert

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed

**cyrup** — `grep -rn 'SYSTEM\.md' crates/` returns FIVE hits and not one reads a file: `cyrup/crates/cyrup-session/src/prompt/overrides.rs:12-16` (a doc comment describing the intended precedence), `cyrup/crates/cyrup-config/src/trust.rs:194` and `:203-204` (the two filenames as trust-gate MARKERS inside `has_trust_requiring_resources`'s `CYRUP_MARKERS` list), and `cyrup/crates/cyrup-session/src/prompt/tests.rs:116`. The only producers of the two override fields are the CLI flags: `cyrup/crates/cyrup-session-svc/src/builder.rs:1051` `custom_prompt: cfg.system_prompt.clone().map(Arc::from)` and `:1055` `append_system_prompt: …`, fed from `cyrup/crates/cyrup/src/cli.rs:456-463`. No code path joins `cwd/.cyrup` or `agent_dir` with either filename; `grep -rn 'system_prompt|SystemPrompt' crates/cyrup-resources/src/` returns nothing at all. The existing test at `prompt/tests.rs:116-138` injects `append_system_prompt` directly, so it proves the JOIN and not the DISCOVERY.

**upstream** — `pi/packages/coding-agent/src/core/resource-loader.ts:1022-1034` @v0.83.0 `discoverSystemPromptFile()`: `join(this.cwd, CONFIG_DIR_NAME, "SYSTEM.md")` when `settingsManager.isProjectTrusted()` (`:1023-1026`), else `join(this.agentDir, "SYSTEM.md")` (`:1028-1031`); `:1036-1048` `discoverAppendSystemPromptFile()` is the identical pair for `APPEND_SYSTEM.md`. Consumed in `reload()` at `:525` (`this.systemPromptSource ?? this.discoverSystemPromptFile()`) and `:533-535` (the discovered append file becomes the sole `appendSources` entry). Unchanged at v0.84.1. The trust list cyrup DID port is `trust-manager.ts:29-37` @v0.83.0, whose `TRUST_REQUIRING_PROJECT_CONFIG_RESOURCES` includes both filenames — cyrup ported the gate and not the thing it gates.

**Impact** — a project shipping `.cyrup/SYSTEM.md` gets the DEFAULT system prompt: every project-specific instruction, house style and safety framing the repo intended is absent from the model's context, with no diagnostic anywhere. Same for `~/.cyrup/agent/SYSTEM.md` (the user's global override) and both `APPEND_SYSTEM.md` tiers. Silent wrong output on a normal path, made worse by the half-port: `has_trust_requiring_resources` PROMPTS the user to trust the project *because* `.cyrup/SYSTEM.md` exists, then loads nothing from it — the user answers a security question about a file cyrup will never read.

**Fix** — add a discovery step in `cyrup-session-svc/src/builder.rs` beside the existing wiring (~`:1045-1060`), mirroring `resource-loader.ts:1022-1048`: `custom_prompt` = `cfg.system_prompt` (CLI) else `<cwd>/.cyrup/SYSTEM.md` when `settings.project_trusted()` else `<agent_dir>/SYSTEM.md`; `append_system_prompt` = `cfg.append_system_prompt` (CLI, which REPLACES per pi's `this.appendSystemPromptSource ?? …`) else the single discovered `APPEND_SYSTEM.md` under the same trust rule. Route the discovered path through the same `resolve_prompt_input` shape used at `cli.rs:456`. **While fixing:** `cyrup-session/src/prompt/overrides.rs:15-16` documents ACCUMULATION of global + project `APPEND_SYSTEM.md`; pi picks exactly ONE — correct the doc to match upstream in the same change.

**Verify** — integration test in `cyrup-session-svc/tests`: (a) trusted project with a sentinel `.cyrup/SYSTEM.md` → the built system prompt equals that sentinel and does NOT contain the default tool guidance; (b) same tree UNtrusted → default prompt, project file ignored; (c) `<agent_dir>/SYSTEM.md` present, no project file → global sentinel used; (d) `.cyrup/APPEND_SYSTEM.md` present → its text appended, and with `--append-system-prompt X` also given, ONLY `X` is appended. All four fail at HEAD.

## CFG-023 — `find_initial_model` step 3 accepts a saved default whose provider has no configured auth

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/model.rs:1341-1354`: step 3 is `// 3. Saved default from settings.` then `if let (Some(dp), Some(dm)) = (default_provider, default_model_id) && let Some(found) = all.iter().find(…)` returning `found.clone()` unconditionally. `has_configured_auth` is a parameter at `:1301` and is used by step 1 (`:1307`) but never by step 3. **CFG-022's landing did not close this** — the shared predicate now exists and step 3 still does not call it.

**upstream** — `pi/packages/coding-agent/src/core/model-resolver.ts:621` @v0.83.0 `// 3. Try saved default from settings if auth is configured.`, `:623` `if (found && modelRuntime.hasConfiguredAuth(found.provider))`, falling through to step 4 at `:632` on a failed check.

**Impact** — a user who removes a provider's credentials keeps launching into that provider's model and gets an auth error per turn instead of falling back to a working model.

**Fix** — add the `has_configured_auth(found)` guard to the step-3 condition at `model.rs:1341-1354`, calling the same `provider_is_configured` predicate CFG-022 landed (`model.rs:1796-1804`) so models.json-only providers are not newly rejected.

**Verify** — unit test in `model.rs`: a saved default naming a provider the predicate rejects yields step 4's result, not the saved one. Fails at HEAD.

## CFG-025 — Settings-declared paths and local package sources do not expand `~` or `file://`

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-resources/src/package/manifest.rs:358-366` is still `let trimmed = entry.trim(); let p = PathBuf::from(trimmed); plain.push(if p.is_absolute() { p } else { base.join(trimmed) })` — `~/team-skills` becomes `<base>/~/team-skills`. Same shape at `cyrup/crates/cyrup-resources/src/discovery.rs:371-380` for a settings-declared local PACKAGE path, which then trips the misleading `run `cyrup install …`` diagnostic at `:401-413`. This is also CFG-004's residual: settings-declared extension paths reach the same join via `add_local_entries` (`discovery.rs:1373-1379`). `expand_tilde` (`cyrup/crates/cyrup-config/src/settings.rs:1036-1051`) still has exactly TWO callers — `session_dir` (`:603`) and `shell_path` (`:733`, added by CFG-031's fix) — and still handles only `~` and `~/`: no `file://`, no win32 `~\`.

**upstream** — `pi/packages/coding-agent/src/utils/paths.ts:57-78` @v0.83.0 `normalizePath` (tilde `:65-71` including win32 `~\`, `file://` `:73-76`), applied by `resolvePathFromBase` at `package-manager.ts:2069-2071`.

**Impact** — `"skills": ["~/team-skills"]` silently loads nothing; `"packages": ["~/pack"]` produces a diagnostic naming the wrong cause ("not installed — run cyrup install"). CFG-031 closed by tilde-expanding its own getter, so the shared util is still owed here and CFG-036 now wants it too.

**Fix** — promote `expand_tilde` into a shared `cyrup-config` path util handling `~`, `~/`, `~\` and `file://`; apply it at `manifest.rs:360` BEFORE the `is_absolute` test and at `discovery.rs:372`, taking the home dir from `DiscoveryConfig` rather than the ambient env, mirroring pi's `options.homeDir`. Land as one changeset with CFG-036, which needs the identical util on the env/CLI dir tiers.

**Verify** — test in `cyrup-resources/tests/resources.rs` with a `DiscoveryConfig` home override: `"skills": ["~/team-skills"]` loads the skill; `"packages": ["file:///abs/pack"]` resolves. Both fail at HEAD. The existing regression at `cyrup-session-svc/tests/settings_resolve.rs:173-192` uses the RELATIVE path `"extra"`, so nothing in the suite exercises `~` today.

## CFG-026 — Settings packages deduped by raw source string, not resolved identity

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-session-svc/src/builder.rs:1746` `let source = entry.source().trim().to_string();` and the dedupe key at `:1775` `out.iter().position(|p| p.source == built.source)`. The in-code note at `:1772-1774` states it explicitly: `[CYRUP-DELTA] the identity is the trimmed source STRING … Tracked separately as CFG-026`. The two scopes genuinely resolve to different bases (`cyrup/crates/cyrup-resources/src/discovery.rs:376-380` vs the `base` computed at `:355-358`).

**upstream** — `pi/packages/coding-agent/src/core/package-manager.ts:1676-1695` @v0.83.0 `getPackageIdentity` returns `npm:<name>`, normalized `git:<host>/<path>`, or `local:${resolvePathFromBase(parsed.path, baseDir)}` — a SCOPE-RESOLVED absolute path — and `dedupePackages` (`:1697`) keys on that.

**Impact** — `"packages": ["./pack"]` declared in both scopes means two different directories to pi (both loaded); cyrup drops the global one and its resources never appear.

**Fix** — compute the identity in `builder.rs:1746` as pi does: resolve local paths against the scope base before using them as the dedupe key, normalize git specs. The delta branch of the same dedupe (`builder.rs:1779-1783`) already landed with CFG-010 and must keep working.

**Verify** — test in `cyrup-session-svc/tests/settings_resolve.rs`: `"./pack"` in both global and project settings, each pointing at a distinct on-disk tree with distinct skills → both skills present. Fails at HEAD.

## CFG-036 — `--session-dir` and the `CYRUP_AGENT_DIR` / `CYRUP_SESSION_DIR` / `CYRUP_PACKAGE_DIR` env vars are not tilde-expanded

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/env.rs:66-70`: `let path = |keys: &[&str]| first_env(keys).map(PathBuf::from);` feeding `agent_dir`, `session_dir` and `package_dir` (with their `PI_*` aliases) raw — no expansion. `ConfigDirs::resolve` (`:139-181`) consumes both the env values and `cli.agent_dir` / `cli.session_dir` / `cli.package_dir` verbatim at `:145-163`, with only `.unwrap_or_else` defaults. The CLI flag is equally raw: `cyrup/crates/cyrup/src/cli.rs:206-207` `#[arg(long = "session-dir")] pub session_dir: Option<PathBuf>`, threaded through `cli.rs:430-431`. The crate already owns `expand_tilde` (`cyrup-config/src/settings.rs:1036-1051`) and wires it only to `session_dir` READ FROM SETTINGS (`:603`) and `shell_path` (`:733`) — never to the flag or env tiers, which take precedence over the settings tier.

**upstream** — `pi/packages/coding-agent/src/config.ts:515-521` @v0.83.0 `getAgentDir()`: `const envDir = process.env[ENV_AGENT_DIR]; if (envDir) { return expandTildePath(envDir); }`, where `expandTildePath` is `normalizePath` (`:498-500` → `utils/paths.ts:57-78`). `config.ts:367-372` `getPackageDir()` does the same with `PI_PACKAGE_DIR`. `main.ts:625-628` @v0.83.0 resolves the session dir as `(parsed.sessionDir ? normalizePath(parsed.sessionDir) : undefined) ?? (envSessionDir ? expandTildePath(envSessionDir) : undefined) ?? startupSettingsManager.getSessionDir()` — pi normalizes ALL THREE tiers; cyrup normalizes only the lowest.

**Impact** — `cyrup --session-dir ~/sessions` (quoted, or set from a config file or CI variable where the shell does not expand) writes sessions into a directory literally named `~` under the cwd, and a later `--resume` from a different cwd cannot find them: the user's transcripts appear lost. `CYRUP_AGENT_DIR=~/alt-agent` silently starts a fresh, empty agent dir at `./~/alt-agent` — no credentials, no settings, no trust decisions, no sessions — and the first-run path re-prompts for everything. Silent, and the symptom points nowhere near the cause. The exact spellings come from pi's documented behaviour, so a migrating user hits it immediately.

**Fix** — move `expand_tilde` into the shared `cyrup-config` path util CFG-025 also wants (handling `file://` and win32 `~\`), then apply it inside the `path` closure at `env.rs:66-70` and to the three CLI overrides in `ConfigDirs::resolve` (`env.rs:145-163`) — one call site each, mirroring pi's `normalizePath` on every tier.

**Verify** — unit tests in `cyrup-config/src/env.rs` beside `settings_session_dir_overrides_the_default_and_is_explicit` (`:296-310`), with HOME overridden: `EnvVars { agent_dir: Some("~/alt".into()), .. }` resolves to `<home>/alt`; `CliConfigOverrides { session_dir: Some("~/sessions".into()), .. }` resolves to `<home>/sessions`; `file:///abs/dir` resolves to `/abs/dir`. All fail at HEAD.

## CFG-037 — A project-scope git package install writes no `.gitignore`, so the clone lands in the user's working tree

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `grep -rn gitignore crates/cyrup-resources/src crates/cyrup/src` returns only the SKILL-WALK ignore READER (`cyrup/crates/cyrup-resources/src/discovery.rs:1299` `for filename in [".gitignore", ".ignore", ".fdignore"]`, applied `:1344-1352`) — nothing ever WRITES a `.gitignore`. `PackageInstaller::install` (`cyrup/crates/cyrup-resources/src/package/install.rs:36-101`) resolves the target with `self.store.package_dir(scope, &id)` (`:69-73`) and goes straight into `spawn_blocking(git_clone(...))` (`:77-80`); there is no install-root preparation step. The project-scope root is `<project_root>/.cyrup/packages` (`cyrup/crates/cyrup-resources/src/package/store.rs:26-33`) — inside the user's repository. `grep -rn 'cloud_sync|xattr|setfattr' crates/` is likewise empty.

**upstream** — `pi/packages/coding-agent/src/core/package-manager.ts:1829-1834` @v0.83.0 (`installGit`) opens with `const gitRoot = this.getGitInstallRoot(scope); if (gitRoot) { this.ensureGitIgnore(gitRoot); }` before the clone. `ensureGitIgnore` is `:1952-1960` — `if (!existsSync(ignorePath)) { writeFileSync(ignorePath, "*\n!.gitignore\n", "utf-8"); }`. The same pair guards the npm root at `:1943-1944` alongside `markPathIgnoredByCloudSync` (`utils/paths.ts:124-139`). Unchanged at v0.84.1 (`:1815-1818`, `:1988-1996`).

**Impact** — `cyrup install github:org/pack` at project scope drops an entire cloned repository (including its own `.git`) into `<repo>/.cyrup/packages/…` with nothing telling git to ignore it. The next `git status` shows hundreds of untracked files, `git add -A` commits a vendored third-party tree into the user's history, and the nested `.git` turns it into a broken embedded repo. pi's users never see this because the install root self-ignores. It is a one-line write cyrup dropped.

**Fix** — add an `ensure_git_ignore(root)` helper in `cyrup-resources/src/package/install.rs`, called from `install` (`:36`) immediately before the `PackageSource::Git` clone at `:69-80`, against `self.store.packages_root(scope)` (`store.rs:26-33`): create the root if absent and, when `<root>/.gitignore` does not exist, write exactly `*\n!.gitignore\n`. Optionally port `markPathIgnoredByCloudSync` for the same root; the npm half of pi's call site is moot while the npm channel is dropped (CFG-009).

**Verify** — test in `cyrup-resources/tests/resources.rs`: install from a local bare git remote at `InstallScope::Project` into a tempdir project root, then assert `<root>/.cyrup/packages/.gitignore` exists with byte content `*\n!.gitignore\n`, and that a pre-existing `.gitignore` there is left untouched. Fails at HEAD.

## CFG-038 — One unparseable key spec discards the whole `keybindings.json` — and applies it partially first — **CLOSED 2026-08-19 (landed)**

> **CLOSED 2026-08-19. Everything below is the filing text and is now wrong about the code** — the
> row's own three-step recipe landed in full; see the open-table row for the per-step evidence.
> `parse_key_values` no longer exists; all seven `merge_json` bodies delegate to one shared
> `merge_entries` (`crates/cyrup-tui/src/keymap.rs:118-149`) that skips-and-continues and returns a
> `Vec<KeybindingIssue>`; `App::load_keybindings_json` (`app/shell.rs:159-172`) concatenates the six
> maps' issue lists rather than `?`-chaining them; and `crates/cyrup/src/main.rs:1975-1986` reports
> the two outcomes differently, naming the rejected ids. **The one citation below that was repaired
> rather than left verbatim is the `load_keybindings_json` range** — the `40821ed` split remap
> pointed it at `app/mod.rs` with an end line 661 past that file's EOF, which is a defect of the
> remap and not of the original filing.

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-tui/src/keymap.rs:13-28` `parse_key_values` returns `Err(TuiError::Keybindings(..))` for any value that is not a string or array-of-strings, and propagates `Key::parse(s)?` for an unrecognised key spec. `Keymap::merge_json` (`:487-493`) does `self.set_action(action, parse_key_values(&value)?)` inside the entry loop, so the FIRST bad entry aborts *after* earlier entries have already been applied via `set_action`. `App::load_keybindings_json` (`cyrup/crates/cyrup-tui/src/app/shell.rs:159-172` at HEAD; the pre-split file cited this as lines 951-962) then chained six merges with `?` — `keymap` (`:160`), `select_keymap` (`:166`), `tree_keymap` (`:167`), `session_keymap` (`:168`), `models_keymap` (`:169`), `editor` (`:170`) — so an error in the first returned before the other five ever ran. The bin's handler at `cyrup/crates/cyrup/src/main.rs:1624-1629` prints `warning: ignoring {path}: {e}` and continues — which is false, the file was half-applied.

**upstream** — pi never parses key strings at load. `pi/packages/coding-agent/src/core/keybindings.ts:350-355` @v0.83.0 `loadFromFile` → `loadRawConfig` (`:328-336`, `catch { return undefined; }` — the only whole-document failure) → `toKeybindingsConfig` (`:275-288`), which SKIPS any entry whose value is neither a string nor a string[] and keeps every other entry. `pi/packages/tui/src/keybindings.ts:243-256` @v0.83.0 `rebuild()` only does `if (!(keybinding in this.definitions)) continue;` and stores the raw `KeyId` strings — an unresolvable spec simply never matches at dispatch and costs nothing else.

**Impact** — a user with a dozen rebinds and one typo (`"app.tools.expand": "ctrl+"`, or a value accidentally written as a number) loses EVERY editor, selector, tree, session and model-picker rebind while keeping whichever app-level bindings happened to parse before the bad one — a half-configured keymap with a warning claiming the file was ignored. In pi the same file loses exactly the one broken entry. **Refuter's caveat:** cyrup iterates a `serde_json::Map` (BTreeMap, alphabetical), so the amount that applies is decided by *key order*, not document order — it still reads as flaky rather than as a config error.

**Fix** — in `cyrup-tui/src/keymap.rs` make every `merge_json` skip-and-continue instead of propagating: `if let Ok(keys) = parse_key_values(&value) { self.set_action(action, keys) }`, collecting per-entry failures — matching `toKeybindingsConfig`'s drop semantics. Keep the whole-document error only for the `keybindings_object` case (`:32-40`, malformed JSON / non-object top level), which is pi's `loadRawConfig` `undefined` path. Return the dropped-entry list from `App::load_keybindings_json` so `main.rs:1624-1629` can name the offending ids instead of claiming the file was ignored.

**Verify** — test in `cyrup-tui/tests/keybindings.rs` beside `malformed_keybindings_json_errors_cleanly` (`:101`): a document with a valid `"tui.select.confirm"` rebind, a valid `"app.tools.expand"` rebind and one bogus `"app.interrupt": "ctrl+"` must apply BOTH valid rebinds to their respective keymaps. Fails at HEAD regardless of key order.

## CFG-045 — `doubleEscapeAction` is inert — the Escape handler has no double-escape and no bash-mode-exit branch — **CLOSED 2026-08-14 (already-done)**

> **CLOSED 2026-08-14 (sweep 8) as already-done. Everything below is the filing text and is now
> wrong about the code.** `crates/cyrup-tui/src/app/input.rs:147-209` is a faithful port of pi's four
> mutually-exclusive `else if` arms (`interactive-mode.ts:2569-2595` @v0.83.0), and it carries **both**
> halves this item calls missing: the bash-mode-exit branch (`:182-186`) and the 500 ms
> double-Escape window reading `doubleEscapeAction` (`:191-209`, `last_escape` field at
> `app/state.rs:121`, tree/fork/none dispatch, `last_escape` reset on fire). Landed under `TUI-009`.
> `EffectiveSettings::double_escape_action` (`cyrup-config/src/settings.rs:883`) has live consumers at
> `app/input.rs:192` and `app/execute_misc.rs:204-205`. **The row's status line said "sweeps 2 and 6 — unchanged at
> HEAD" for two editions; it was doc staleness, the failure class the ledger names as the more
> expensive of the two.**


**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — same inert-key class as CFG-S04, which the sweep passed because this key *does* have a consumer outside `cyrup-config` — one that only DISPLAYS it. `grep -rn '\bdouble_escape_action\b' crates/ --include='*.rs'` outside `settings.rs` returns exactly ONE line: `cyrup/crates/cyrup-tui/src/app/settings_rows.rs:166-172` (the id at `:167`, the accessor at `:169`), the `SettingRow::choice("doubleEscapeAction", "Double-escape action", eff.double_escape_action(), choices(&["fork","tree","none"]))` row. `grep -rn 'double_esc|DoubleEsc|doubleEscape' crates/cyrup-tui/src crates/cyrup/src` finds no handler, no `last_escape_time`, no 500 ms window. cyrup's `Action::Interrupt` arm (`app/input.rs:130-214`) implements only three branches — branch-summary abort, bash-running abort, streaming/idle teardown — then falls out.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2570-2596` @v0.83.0, `this.defaultEditor.onEscape`, is a FOUR-branch chain: `isStreaming` → restore-queued + abort; `isBashRunning` → abortBash; `isBashMode` → `this.editor.setText(""); this.isBashMode = false; this.updateEditorBorderColor();`; else empty editor → `const action = this.settingsManager.getDoubleEscapeAction(); if (action !== "none") { if (now - this.lastEscapeTime < 500) { action === "tree" ? this.showTreeSelector() : this.showUserMessageSelector(); this.lastEscapeTime = 0; } else { this.lastEscapeTime = now; } }`. cyrup ports branches 1–2 and drops 3–4.

**Impact** — the `/settings` row lets a user pick `fork` / `tree` / `none` and nothing whatsoever changes, and Esc-Esc on an empty editor never opens `/tree` (pi's default) or `/fork`. Both destinations exist in cyrup as slash commands (`cyrup/crates/cyrup-tui/src/commands.rs:60-63`), so the reachable-feature loss is the shortcut — but the setting is fully dead and pi's documented default behaviour is absent. The dropped bash-mode-exit branch is a second, separate loss: Escape does not leave bash mode.

**Fix** — extend the `Action::Interrupt` arm at `app/input.rs:130-214` with pi's remaining two branches: a bash-mode exit (clear editor text, clear the bash flag, restore the border colour) and, on an empty editor, a `last_escape_time` field with a 500 ms window dispatching to the tree selector or the user-message (fork) selector per `eff.double_escape_action()`, with `"none"` disabling it. Land beside CFG-S04's wiring work.

**Verify** — test in `cyrup-tui/tests` that sends two Esc events within 500 ms on an empty editor and asserts the tree selector opens; a second with `doubleEscapeAction = "none"` asserting it does not; a third asserting Esc in bash mode clears the editor and leaves bash mode. All fail at HEAD.

## CFG-046 — models.json string fields are not length-validated, so `"baseUrl": ""` rewrites every model to an empty endpoint

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/model.rs:1465-1477` declares `pub base_url: Option<String>`, `name`, `api_key`, `api` with no length or emptiness check anywhere. `apply_models_json` step 1 (`:1874-1880`) is `if let Some(base_url) = &config.base_url && config.oauth.is_none() { m.base_url = base_url.clone(); }` — `Some("")` is `Some`, so EVERY built-in model of that provider gets `base_url = ""`. `model_from_json` (`:1931-1938`) has the same hole: `definition.base_url.clone().or_else(|| provider_config.base_url.clone()).or_else(…).ok_or_else(…)` treats `Some("")` as present, so pi's `"baseUrl" is required when defining custom models.` never fires. `load_models_file_reporting` (`:1700-1717`) reports no error. The numeric fields are the mirror image: `Option<u64>` makes `-1` a whole-file serde failure where pi rejects only that provider, and cyrup checks only `== Some(0)` at `:1939`/`:1945`.

**upstream** — `pi/packages/coding-agent/src/core/model-config.ts` @v0.83.0 types `name`, `baseUrl`, `apiKey` and `api` on `ProviderConfigSchema` as `Type.Optional(Type.String({ minLength: 1 }))`, so `{"providers":{"x":{"baseUrl":""}}}` fails `validateModelsConfig.Check(parsed)` and `ModelConfig.load` returns an EMPTY provider map plus `Invalid models.json schema:` — the agent starts on built-ins with a loud diagnostic. Belt-and-braces, `modelFromJson` also has `if (!baseUrl) throw …` and `""` is falsy in JS. `contextWindow` / `maxTokens` are `Type.Number()` with a `<= 0` runtime check in `modelFromJson` that rejects only the offending provider.

**Impact** — a `models.json` pi refuses outright is accepted and composed by cyrup: every request for that provider goes to an empty URL while the file is reported as valid. This is a different lens from CFG-043 (which is about the error *message* for a wrong-typed field) — here the file is not diagnosed at all.

**Fix** — validate the four string fields as non-empty after trimming, in `apply_models_json` / `model_from_json` (`model.rs:1465-1477`, `:1874-1880`, `:1931-1945`), and widen the numeric guard from `== Some(0)` to "not strictly positive". Land with CFG-043's per-provider deserialization so one bad provider block is rejected while the rest load, matching pi.

**Verify** — table test in `model.rs`: `{"providers":{"x":{"baseUrl":""}}}` produces a schema diagnostic and composes NO provider override (built-in base URLs intact); a custom model with `"baseUrl": ""` yields pi's `"baseUrl" is required when defining custom models.`; `"contextWindow": -1` rejects only that provider. All fail at HEAD.

## CFG-048 — pi's sixth startup migration, `migrateKeybindingsConfigFile`, is not ported at write time or read time

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup/src/migrations.rs:26-36` `run_migrations` makes exactly four calls — `migrate_auth_to_auth_json` (`:27`), `migrate_sessions_from_agent_root` (`:28`), `migrate_tools_to_bin` (`:29`), `migrate_extension_system` (`:30`). Re-read at HEAD in this pass; there is no keybindings step. The only justification is the in-source comment at `migrations.rs:9-10` — "The keybindings-config migration is intentionally NOT ported here: cyrup's keybindings store (`cyrup-tui`) has no legacy on-disk shape to migrate from" — which is the self-certifying kind `docs/gap-analysis/README.md:208-212` says must not be treated as a decision of record, and which is **factually wrong about the read path as well**. The read path is `crates/cyrup/src/main.rs:1622-1629` (reads `<agent_dir>/keybindings.json`) → `crates/cyrup-tui/src/app/shell.rs:159-172` (the pre-split file cited this as lines 951-963) `load_keybindings_json` → six `merge_json` calls; `crates/cyrup-tui/src/keymap.rs:487-493` is `for (id, value) in keybindings_object(json)? { if let Some(action) = Action::from_id(&id) { … } }`, so an unrecognised id is dropped with **no diagnostic**. `grep -n migrat crates/cyrup-tui/src/{keymap,app,editor}.rs` returns zero; no alias table exists anywhere in the crate.

**upstream** — `pi/packages/coding-agent/src/migrations.ts:312` @v0.83.0 — `migrateKeybindingsConfigFile();`, the fourth of five calls in `runMigrations` (`:305-315`). Body at `:157-174`: read `<agentDir>/keybindings.json`, call `migrateKeybindingsConfig`, and if `migrated` write it back as `${JSON.stringify(config, null, 2)}\n`. `pi/packages/coding-agent/src/core/keybindings.ts:209-269` holds `KEYBINDING_NAME_MIGRATIONS` — **59** legacy→modern entries (the critique's "~30" undercounts): 21 → `tui.editor.*` (`cursorUp` → `tui.editor.cursorUp`, `:210`), 4 → `tui.input.*`, 6 → `tui.select.*`, 28 → `app.*` (`interrupt` → `app.interrupt` `:241`, `deleteSessionNoninvasive` → `app.session.deleteNoninvasive` `:268`). `migrateKeybindingsConfig` (`:289-309`) also **drops** a legacy key when its modern twin is already present (`:301-304`) and reorders through `orderKeybindingsConfig` (`:311-327`). It is applied a **second** time on every read at `keybindings.ts:366` inside `loadFromFile` (`:363-367`), which both `KeybindingsManager.create` (`:348-352`) and `reload()` (`:354-357`) go through. Both files are byte-identical at v0.83.0 and v0.84.1, so this is a baseline miss, not drift.

**Impact** — a user carrying a pre-rename `keybindings.json` — the pi-migrant population this port exists to serve — gets a file that pi repairs on first launch and honours forever, and that cyrup ignores entry by entry in silence: `main.rs:1626-1629` prints only on a hard parse error, so a fully legacy file produces no output at all and stock defaults. Cross-checking the 59 targets against what cyrup's maps actually resolve, **27 of 59** (the six `tui.select.*` plus 21 of the 28 `app.*`) would work today if the table alone were ported; the other 32 are inert for the separate reasons already filed as **TUI-028** (the 21 `tui.editor.*` + 4 `tui.input.*` land in a namespace cyrup spells `editor.*`) and **TUI-008** (7 unbound `app.*` ids). So this recovers real user-visible behaviour on its own **and** is a hard prerequisite for TUI-028 being safe to land.

**Fix** — port `migrateKeybindingsConfigFile` as `migrate_keybindings_config_file(&dirs.agent_dir)` in `crates/cyrup/src/migrations.rs`, called from `run_migrations` between `migrate_tools_to_bin` and `migrate_extension_system` (pi's position, `:311`→`:312`→`:313`). Port `KEYBINDING_NAME_MIGRATIONS` as a 59-entry const table plus `migrate_keybindings_config(&mut Map) -> bool` reproducing pi's three behaviours: rename, drop-legacy-when-modern-present (`keybindings.ts:301-304`), and `orderKeybindingsConfig` ordering (`:311-327`) against cyrup's own id list; write back only when `migrated`, with pi's trailing newline. **Map the 21 `tui.editor.*` / 4 `tui.input.*` targets to cyrup's CURRENT `editor.*` spelling so the migration is correct at HEAD, and add `editor.* → tui.editor.*` rows when TUI-028 renames the namespace** — otherwise TUI-028 silently breaks every `editor.*` config users have written against shipped cyrup. Also apply the table at read time in `keymap.rs`'s `keybindings_object` (`:32-40`) so the alias works before the migration has ever run and after a hand-edit, matching pi's double application. **Delete the false claim at `migrations.rs:9-10`.** While in the area, correct TUI-028's upstream cites in `07-cyrup-tui.md`: it says `keybindings.ts:208-270` and `migrateKeybindingsConfig (:294-311)`; the true offsets are `:209-269` and `:289-309`, identical at both tags.

**Verify** — new test in `migrations.rs`'s test module: write `{"cursorUp":"ctrl+p","interrupt":"ctrl+q","app.clear":"ctrl+k"}` to `<agent_dir>/keybindings.json`, run `run_migrations`, assert the file reads `editor.cursorUp` / `app.interrupt` / `app.clear` in cyrup's declaration order with a trailing newline, and that a second run is a no-op. Collision case: `{"interrupt":"ctrl+q","app.interrupt":"ctrl+e"}` must keep only `app.interrupt: ctrl+e`. Plus a read-time test in `crates/cyrup-tui/src/tests/keybindings.rs`: `App::load_keybindings_json(r#"{"interrupt":"ctrl+q"}"#)` binds `Action::Interrupt` to ctrl+q without the file ever being migrated on disk. **Ships with TUI-051** (`/reload` is pi's second application site for this table) and must not land after TUI-028.

## CFG-049 — Extension-system deprecation warnings are printed and immediately buried; pi blocks startup on a keypress

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup/src/migrations.rs:263-277` `format_deprecation_warnings` builds pi's text (per-warning `Warning: …` lines, the `Move your extensions to the extensions/ directory.` line, both URLs) but its own doc comment concedes it is "minus the keypress wait (handled by the interactive front-end)". **The front-end does not handle it:** `crates/cyrup/src/main.rs:529-533` is `let warnings = migrations::format_deprecation_warnings(&deprecation_warnings); if !warnings.is_empty() { eprint!("{warnings}"); }` and execution continues straight into TUI init. `grep -rn 'Press any key' crates` returns **zero hits workspace-wide**, so neither the pause nor pi's `Press any key to continue...` line exists anywhere.

**upstream** — `pi/packages/coding-agent/src/migrations.ts:277-296` @v0.83.0 `showDeprecationWarnings` — after the warnings, the guide URL and the docs URL it prints `chalk.dim('\nPress any key to continue...')` (`:286`) and then blocks: `await new Promise<void>(resolve => { process.stdin.setRawMode?.(true); process.stdin.resume(); process.stdin.once('data', () => { … resolve(); }); })` (`:288-295`). Awaited from `main.ts:838-840` — `if (appMode === 'interactive' && deprecationWarnings.length > 0) await showDeprecationWarnings(deprecationWarnings);` — i.e. startup is deliberately gated on acknowledgement, **before** the interactive UI takes the terminal.

**Impact** — these warnings are the only signal a user gets that a legacy `hooks/` directory or a custom `tools/` directory has stopped working, meaning every extension in them is now silently doing nothing. In pi the user cannot proceed without seeing it. In cyrup it is one of several stderr lines emitted microseconds before the first TUI frame paints over the same region; on a busy startup (settings diagnostics, migrated-credential notice, model-fallback line) it is realistically never read, and the user concludes their extensions broke for no reason.

**Fix** — port the gate literally. After `eprint!("{warnings}")` at `main.rs:529-533`, print pi's `Press any key to continue...` line and block on a single key read before TUI init, in interactive mode only (pi's `appMode === 'interactive'` guard at `main.ts:838`; cyrup's call site is already inside the interactive branch). Use the raw-mode read cyrup already owns rather than an `App` dependency, so it stays on the pre-TUI path exactly as upstream. **While editing this function, settle the rebrand question on `MIGRATION_GUIDE_URL` / `EXTENSIONS_DOC_URL` (`migrations.rs:270-272`)**, which currently send cyrup users to `github.com/earendil-works/pi-mono`.

**Verify** — integration test in `crates/cyrup/tests/` driving the binary with a temp `agent_dir` containing a `hooks/` dir and a scripted stdin: assert the process does not reach TUI init until a byte is written to stdin, and that `Press any key to continue...` appears on stderr. Plus a **live terminal run** with `~/.cyrup/hooks/` present, confirming the notice is readable and the session waits — this one is pre-TUI output, and a captured-stderr assertion cannot show that the first frame does not paint over it.

## CFG-018 — Glob scope patterns no longer short-circuit on an exact model reference

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — the glob branch of `ModelResolver::resolve_scope` (`cyrup/crates/cyrup-config/src/model.rs:258-275`) strips an optional `:level` suffix (`:261-268`) then goes straight to the `glob_match` filter over `self.available` (`:269-274`) — no `match_reference` / exact attempt first.

**upstream** — `pi/packages/coding-agent/src/core/model-resolver.ts:297` @v0.83.0 calls `findExactModelReferenceMatch(globPattern, availableModels)` INSIDE the glob branch, before the minimatch filter (declared `:79`; also used on the non-glob path at `:128`).

**Impact** — a scope pattern that is an exact model reference but happens to contain a glob metacharacter (`[`, `?`) resolves through the filter instead of matching directly, so it silently resolves to nothing or to the wrong set.

**Fix** — insert a `match_reference` attempt at `model.rs:269`, returning early on a hit.

**Verify** — unit test in `model.rs`: a pattern that is an exact model reference containing a metacharacter resolves to exactly that model. Fails at HEAD.

## CFG-019 — `defaultModelPerProvider` still stale — `xai` id retired, `radius` arm missing

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — partially fixed since the last pass: the two qwen arms landed at `cyrup/crates/cyrup-config/src/model.rs:973-974` (`qwen-token-plan` / `qwen-token-plan-cn` → `qwen3.7-max`) with matching `KNOWN_PROVIDERS` entries at `:1022-1023` in pi's insertion position. **Still wrong:** `model.rs:951` is `"xai" => "grok-4.20-0309-reasoning"`, and there is NO `radius` arm anywhere in `default_model_per_provider` (`:936-982`) or `KNOWN_PROVIDERS` (`:985-1031`). cyrup's map is 37 entries; pi's is 38 at v0.83.0.

**upstream** — `pi/packages/coding-agent/src/core/model-resolver.ts:28` @v0.83.0 (`:35` @v0.84.1) `xai: "grok-4.5"`; `:20` @v0.83.0 (`:27` @v0.84.1) `radius: "auto"`. Both unchanged across the two tags, so this is inherited debt rather than new drift — the v0.84.1 *additions* are CFG-041.

**Impact** — on identical catalogs, a user with only xAI configured and no saved `defaultModel` launches a different model in cyrup than in pi. `cyrup/crates/cyrup-provider/src/providers/catalog/xai.json` carries BOTH ids, so the old "the catalog doesn't have grok-4.5 anyway" mitigation is void.

**Fix** — correct the `xai` arm at `model.rs:951` to `grok-4.5` and add the `radius` arm plus its `KNOWN_PROVIDERS` entry at pi's position. Do it in one changeset with CFG-041's two v0.84.1 additions, since map ORDER is load-bearing for `first_default_or_first` (`:1034-…`).

**Verify** — table test asserting cyrup's map equals pi v0.84.1's 40 entries key-for-key AND in order. `grep -rn grok-4.20-0309-reasoning crates/ --include='*.rs'` returns only `model.rs:951` itself, so nothing pins the stale id today and the fix is unguarded in both directions.

## CFG-007 — `AuthStore` re-reads auth.json per query and coerces errors to "not configured"

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/auth.rs:120-126` `read_file` does `read_to_string` + `parse_auth` on every call; there is no cache field on `AuthStore`. `has_auth` (`:261-271`) is `matches!(self.read_file(), Ok(map) if …)` at `:265`, so any `Err` reads as not-configured, and `get_auth_status` (`:273-306`) uses the same idiom at `:278`. No cached `AuthFile`, no `reload()`. The RwLock covers only the runtime `--api-key` tier.

**upstream** — `pi/packages/coding-agent/src/core/auth-storage.ts:172-178` @v0.83.0 (the constructor calls `reload()` once, `:177`), `:204-215` (`reload()` ending in `catch { /* Preserve the last valid in-memory snapshot. */ }`), `:217-222` (`read()` answers from the cached `this.data`). **Cite correction:** the item's original upstream lines (`:188-204` / `:236-247` / `:260-273`) were taken from pi HEAD, not the ported tag — `auth-storage.ts` is 271 lines at v0.83.0.

**Impact** — a transient read error, or a mid-write window, makes every configured provider read as unauthenticated; pi keeps the last good snapshot. Plus one syscall per auth query on hot TUI paths.

**Fix** — add a cached `AuthFile` + revision behind the existing RwLock, an explicit `reload()` that preserves the prior snapshot on error, and route `has_auth` / `get_api_key` through it. Note CFG-044 proposes deleting `get_auth_status` outright — sequence the two so this item does not cache a function that is about to be removed.

**Verify** — test: populate `auth.json`, query once, make it unreadable, query again → still configured. Fails at HEAD.

## CFG-008 — Model-scope resolution drops every diagnostic

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `ModelResolver::resolve_scope` (`cyrup/crates/cyrup-config/src/model.rs:236-282`) returns a bare `Vec<ScopedModel>`: the glob branch (`:258-275`) falls through silently when the filter matches nothing, and the non-glob branch (`:276-281`) keeps only `parsed.model` and DISCARDS `parsed.warning`. No `ModelScopeDiagnostic` type exists; `grep -rn 'No models match' crates/` is empty.

**upstream** — `pi/packages/coding-agent/src/core/model-resolver.ts:261` @v0.83.0 declares `ModelScopeDiagnostic` (codes `no-match` / `invalid-thinking-level`), `:270` returns `diagnostics` on the result, `No models match pattern "…"` is pushed at `:316` (glob) and `:340` (reference), and `Invalid thinking level "…" in pattern` is minted at `:243`.

**Impact** — a typo'd `--models 'anthorpic/*'` resolves to nothing with no explanation, as does an invalid thinking-level suffix.

**Fix** — introduce `ModelScopeDiagnostic` plus a result struct in `model.rs`, accumulate in both branches (`:258-275`, `:276-281`), and surface at the CLI/session call sites.

**Verify** — unit test: `anthorpic/*` yields one diagnostic with pi's exact text; `anthropic/x:bogus` yields the invalid-level warning. Both fail at HEAD.

## CFG-006 — `websocketConnectTimeoutMs` never reaches the HTTP/stream layer

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — the `retry.provider.*` half CLOSED: `cyrup/crates/cyrup-session-svc/src/builder.rs:1223-1234` reads `eff.provider_retry_settings()` and threads `timeout_ms` / `max_retries` / `max_retry_delay_ms` onto the agent builder, citing pi `sdk.ts:303-317`. The websocket half is still inert: `grep -rn websocket_connect_timeout_ms crates/ --include='*.rs'` returns exactly three lines — the accessor `cyrup/crates/cyrup-config/src/settings.rs:705`, the field declaration `cyrup/crates/cyrup-provider/src/stream.rs:179`, and the copy-forward `cyrup/crates/cyrup-provider/src/utils/simple_options.rs:84`. Nothing assigns it.

**upstream** — `pi/packages/coding-agent/src/core/settings-manager.ts:131` @v0.83.0 declares the key (`websocketConnectTimeoutMs?: number`, still present at v0.84.1), consumed through `getWebSocketConnectTimeoutMs` in `sdk.ts`.

**Impact** — a user tuning the websocket connect timeout in settings gets no effect at all; the provider layer always uses its built-in default.

**Fix** — thread `websocket_connect_timeout_ms()` from `builder.rs` (beside the retry block at `:1223-1234`) into `cyrup-provider`'s stream options at the existing field site (`stream.rs:179`).

**Verify** — test asserting the constructed provider options carry the settings value. **Note the residual test debt from the closed half:** the retry assignment is verified structurally only — nothing proves `max_retry_delay_ms` reaches the retry loop (blind spot 6).

## CFG-039 — models.json `samplingParams` on model definitions and modelOverrides is silently dropped — **CLOSED 2026-08-15 (REFUTED: already closed at HEAD)**

> **CLOSED 2026-08-15 as REFUTED — nothing was changed this pass; everything below is the filing text
> and is now wrong about the code.** The `cyrup` paragraph's headline grep is stale in both directions:
> `grep -rn 'sampling_params' crates/ --include='*.rs'` returns hits in `cyrup-config`, `cyrup-provider`
> and `cyrup-agent`, and both `ModelDefinition` and `ModelOverride` carry the field. **The `Fix`
> paragraph is an accurate description of what landed**, including the one clause that mattered — the
> override path MERGES key-wise rather than replacing — and the `Verify` table test exists and passes.
> The disposition, and the reason a HARD-BLOCKED marker must be re-checked at HEAD, are in the
> Open-items row.


**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `grep -rn 'sampling_params|samplingParams' crates/ --include='*.rs'` returns ZERO at HEAD. `ModelDefinition` (`cyrup/crates/cyrup-config/src/model.rs:1542-1565`) carries id/name/api/base_url/reasoning/thinking_level_map/input/cost/context_window/max_tokens/headers/compat and no sampling field; `ModelOverride` (`:1572-1591`) likewise. Both derive `Deserialize` WITHOUT `deny_unknown_fields`, so a declared `samplingParams` block deserializes successfully and is discarded — no error, no warning, no diagnostic through `load_models_file_reporting` (`:1700-1717`).

**upstream** — `pi/packages/coding-agent/src/core/model-config.ts:167` @v0.84.1 adds `samplingParams: Type.Optional(Type.Record(Type.String(), Type.Unknown()))` to `ModelDefinitionSchema` and `:188` the same to `ModelOverrideSchema` (neither exists at v0.83.0). Consumed in `provider-composer.ts` @v0.84.1: `modelFromJson` sets `samplingParams: definition.samplingParams` (`:162`), and `applyModelOverride` MERGES rather than replaces — `samplingParams: override.samplingParams ? { ...model.samplingParams, ...override.samplingParams } : model.samplingParams` (`:123-125`). It reaches the wire via `ai/src/api/simple-options.ts:27-33` and `openai-completions.ts:885-886`.

**Impact** — a user who pins `top_p`, `top_k` or `repetition_penalty` on a custom or overridden model gets the provider's defaults instead, with the file reported as valid. Silent wrong request parameters on exactly the models people hand-tune. This is the models.json half of a two-part gap: `PARITY-GAPS` already records the provider-layer half (`StreamOptions` / `Model` carry no sampling field). Neither half is useful alone — schedule them together.

**Fix** — add `sampling_params: Option<serde_json::Map<String, Value>>` to `ModelDefinition` (`model.rs:1542-1565`) and `ModelOverride` (`:1572-1591`); in `apply_models_json` (`:1838-…`) set it from the definition when building a custom model and MERGE it key-wise on the override path rather than replacing. Then carry it onto `cyrup_provider::Model` and into the request builders (the PARITY-GAPS provider-layer item).

**Verify** — table test in `model.rs`: a provider block declaring `"models": [{"id":"m","samplingParams":{"top_p":0.5}}]` composes a model carrying `top_p = 0.5`; a `modelOverrides` entry adding `{"top_k": 40}` yields BOTH keys (merge, not replace). Both fail at HEAD.

## CFG-020 — No `ModelRuntime` type and no availability snapshot

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed

**cyrup** — `grep -rn 'struct ModelRuntime' crates/ --include='*.rs'` returns ZERO — no such type at HEAD. `full_model_registry()` (`cyrup/crates/cyrup-session-svc/src/session.rs:2680-2720`) recomposes base + guest + built-ins + models.json on EVERY call; `available_model_catalog` (`:2727`) calls it once and then filters, and `provider_has_configured_auth` (`:2659-2666`) re-reads through `provider_is_configured` each time. There is no snapshot and no invalidation queue. `cyrup_provider::auth::ApiKeyAuth` (`cyrup/crates/cyrup-provider/src/auth/mod.rs`) still exposes only `name` + `resolve` — pi's status-query half (`check`) has no counterpart.

**upstream** — `pi/packages/coding-agent/src/core/model-runtime.ts` @v0.83.0 holds a `snapshot` with `configuredProviders` (`:372-374`) rebuilt on invalidation (`queueAvailabilityRefresh`, `:270-289`). **The target grew substantially at v0.84.1** (+356 lines): per-provider `refreshProviderAvailability`, `getAvailableSnapshot()`, `enqueueCredentialOperation`, `CredentialSynchronizationError`.

**Impact** — repeated per-call recomposition of the whole registry; more importantly, the absence of a single snapshot is what let the two `has_configured_auth` implementations drift in the first place (CFG-022, CFG-024 — both now closed by a shared *function*, which is the cheap half of the fix, not the snapshot).

**Fix** — introduce a `ModelRuntime` in `cyrup-config` owning the composed registry plus a `configured_providers` set, invalidated on settings/auth/models.json change; add a `check` method to `ApiKeyAuth`; have `session.rs:2680-2720` and `main.rs` both read from it. CFG-042's revision-checked `FileModelsStore` cache is the mechanism this snapshot would sit on.

**Verify** — assert the registry is composed once per invalidation rather than once per query, and that `main.rs` and `AgentSession` return identical `configured_providers` for a models.json-only provider. **Whoever schedules this must read `model-runtime.ts` at v0.84.1, not v0.83.0** — the port target is materially larger than this item was originally written against, and `CredentialSynchronizationError` / `enqueueCredentialOperation` are area-01 PARITY-GAPS items that interlock with it.

## CFG-003 — Settings `packages` are resolved but never auto-installed — **CLOSED 2026-08-15**

**Kind** not-ported · **Severity** medium · **Effort** ~~L~~ S · **Confidence** confirmed

> **CLOSED 2026-08-15.** The full disposition is in the Open-items row. Three things below are
> corrected rather than deleted, because each one misdirected an earlier pass:
>
> 1. **The `Fix`'s "gated on an explicit opt-in setting" is WRONG and was not followed.** Upstream's
>    only gate is `isOfflineModeEnabled()` (`package-manager.ts:42-46`, `PI_OFFLINE`) plus an
>    optional `onMissing` callback the session path does not pass (`resource-loader.ts:403`, `:549`
>    @v0.83.0). Inventing a settings key would have been the divergence, not the safety.
> 2. **The `Fix`'s "git/oci" is WRONG on the OCI half.** pi's `installParsedSource` handles npm and
>    git only (`:1347-1356`); there is no OCI arm upstream to port, and cyrup has no OCI fetcher
>    (R-09-021).
> 3. **The `Verify` is unrunnable as written** — "a local bare git remote declared in settings"
>    cannot reach the git arm at all: `isLocalPath` (paths.ts:41-55) calls a `file://` URL LOCAL on
>    both sides, so a settings entry naming a local repo is resolved as a path, never cloned. The
>    landed tests split that: `git:localhost/...` end to end for the arm, a direct `file://` clone
>    one layer down for the mechanism.

**cyrup** — ~~`cyrup/crates/cyrup-resources/src/discovery.rs:325-340` still carries the `[CYRUP-DELTA]` doc "cyrup performs no network install during session assembly". `resolve_configured_package` (`:341-419`) resolves git/oci ONLY through an already-materialized `cyrup install` tree via `installed_dir` (`:384-399`); anything else becomes the loud diagnostic at `:403-413`.~~ **Stale — that delta is deleted and the git arm installs (2026-08-15).** The read/filter/discover half landed and is fine.

**upstream** — `pi/packages/coding-agent/src/core/package-manager.ts:1240-1283` @v0.83.0 `resolvePackageSources` defines `installMissing` (`:1244-1251`) and calls it for both the npm branch and the git branch.

**Impact** — a fresh clone whose `.cyrup/settings.json` lists `github:org/pack` gets zero resources from it and a diagnostic telling the user to run `cyrup install` manually.

**Fix** — implement the git/oci fetch path behind `resolve_configured_package` (`discovery.rs:384-399`), reusing what `cyrup install` already does, gated on an explicit opt-in setting since this is a network operation at session start. Land CFG-037 first or alongside — auto-install at project scope is exactly the path that would drop an un-ignored clone into the user's repo.

**Verify** — integration test with a local bare git remote declared in settings: the first session materializes the tree and loads its skills.

## CFG-005 — Two multi-prompt api-key login flows unported (`ApiKeyAuth` has no `login` member)

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed

**cyrup** — most of this item CLOSED: `cyrup/crates/cyrup-config/src/login.rs` (1721 lines, new since `1806375`) ports `login` (`:760`), `logout` (`:818`), `env_api_key_login` (`:343`), `provider_auth_status` (`:360`) and the selector/option builders (`:422`, `:491`, `:547`, `:589`, `:641`, `:686`); OAuth refresh lives at `cyrup/crates/cyrup-provider/src/auth/resolve.rs:146-239`. The RESIDUAL is stated in-tree at `login.rs:33-42`: the two MULTI-PROMPT api-key logins are unported because `cyrup_provider::auth::ApiKeyAuth` (`cyrup/crates/cyrup-provider/src/auth/mod.rs:60-71`) exposes only `name` + `resolve` — there is no `login` member for a flow that needs more than one field.

**upstream** — `pi/packages/ai/src/providers/cloudflare-auth.ts:48-53` @v0.83.0 (`cloudflareWorkersAIAuth()` prompts for key THEN account id) and `pi/packages/ai/src/providers/google-vertex.ts:15-45` @v0.83.0 (`vertexAuth` prompts select → key / project / location). Both are `login` members on pi's `ApiKeyAuth` shape.

**Impact** — Cloudflare Workers AI and Google Vertex cannot be configured interactively; a user must hand-edit `auth.json` or set env vars, with no in-product path.

**Fix** — add an optional `login` member to the `ApiKeyAuth` trait (`cyrup-provider/src/auth/mod.rs:60-71`) carrying a multi-field prompt description, and implement it for the two providers. Interlocks with `PROV-029` (area 01), which records the Copilot/Codex flows as unreachable for a related reason.

**Verify** — a scripted login for each provider writes the full multi-field credential and a subsequent `resolve` returns a usable key. **Maintainer has DEPRIORITISED this item: keep filed, do not schedule.**

## CFG-009 — An `npm:` package source fails with the misleading message "unsupported source (OCI deferred)"

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `PackageSource::parse` returns `Err(ResourceError::Unsupported)` for an `npm:` prefix at `cyrup/crates/cyrup-resources/src/package/source.rs:78-81` (the documented R-09-021 drop is at `:70-71`), whose Display is `#[error("unsupported source (OCI deferred)")]` at `cyrup/crates/cyrup-resources/src/error.rs:40-41`. Settings entries route through the same parse (`cyrup/crates/cyrup-resources/src/discovery.rs:360-369`), so it appears on a normal session start.

**upstream** — `pi/packages/coding-agent/src/core/package-manager.ts:1419-1445` @v0.83.0 (`parseSource`'s npm branch), consumed by `resolvePackageSources` at `:1257-1268`.

**Impact** — a user declaring an npm package is told the problem is OCI. The npm channel drop itself is a documented decision; only the message is wrong.

**Fix** — split `ResourceError::Unsupported` into `UnsupportedNpm` / `UnsupportedOci` in `error.rs:40-41` with accurate text. Dangling consequence: `EffectiveSettings::npm_command()` (`cyrup-config/src/settings.rs:742`) has zero consumers for the same root cause (CFG-015).

**Verify** — assert the message text for an `npm:` source. The existing test at `cyrup-resources/tests/resources.rs:1987-1991` asserts the VARIANT, not the text, so it neither pins nor guards the message.

## CFG-013 — `TrustStore::nearest` reads trust.json without the file lock

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `nearest()` at `cyrup/crates/cyrup-config/src/trust.rs:140-141` calls `self.read_map()` with no `crate::lock::FileLock::acquire`, while `set_many` (`:160`) acquires it at `:163` before its own `read_map()` at `:164`.

**upstream** — `pi/packages/coding-agent/src/core/trust-manager.ts:219-222` @v0.83.0 wraps `getEntry`'s read in `withTrustFileLock` (defined `:168`); `get()` at `:216` routes through `getEntry`.

**Impact** — negligible on POSIX, since the writer uses rename-based `write_atomic`. A consistency-posture divergence that matters if the writer ever stops being atomic or a non-POSIX target appears.

**Fix** — acquire the lock around `read_map()` at `trust.rs:141`.

**Verify** — code review; no behavioural test is meaningful on POSIX.

## CFG-016 — `${0:-default}` emitted literally instead of substituting

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `match_brace_form` at `cyrup/crates/cyrup-resources/src/prompt.rs:236`; line `:248` is still `let idx = num.parse::<usize>().ok()?.checked_sub(1)?;` — for `num == "0"`, `checked_sub(1)` is None and the `?` aborts the WHOLE form, so `substitute_args` falls to the unrecognized-`${…}` path and emits the token verbatim.

**upstream** — `pi/packages/coding-agent/src/core/prompt-templates.ts:74` @v0.83.0 — the regex alternative `\$\{(\d+|ARGUMENTS|@):-([^}]*)\}` matches, and the handler at `:78` indexes `args[0-1] = args[-1] = undefined`, which is falsy, so `:79` returns the default.

**Impact** — a prompt template using `${0:-default}` renders the literal `${0:-default}` into the model's context instead of the default text.

**Fix** — one line at `prompt.rs:248`: treat index 0 as "no such arg" and take the default branch instead of aborting the form.

**Verify** — unit test in `prompt.rs`: `${0:-fallback}` with no args renders `fallback`. No test pins the current behaviour, so the fix is unguarded in both directions.

## CFG-017 — `${@:-default}` / `${ARGUMENTS:-default}` prompt-template forms unsupported

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — the `:-` guard at `cyrup/crates/cyrup-resources/src/prompt.rs:244-246` still requires `num.bytes().all(|b| b.is_ascii_digit())`, so inner `@:-x` / `ARGUMENTS:-x` fails it; the `@:` arm at `:257-263` then rejects `-x` at `:262` and returns None, and the token falls out to the literal-`$` path. `match_brace_form`'s signature (`:236`) still takes only `args`, never `all_args`.

**upstream** — `pi/packages/coding-agent/src/core/prompt-templates.ts:74` @v0.83.0 accepts `(\d+|ARGUMENTS|@)` and resolves `@` / `ARGUMENTS` to `allArgs` at `:78`.

**Impact** — `${@:-default}` and `${ARGUMENTS:-default}` render literally into the prompt.

**Fix** — extend the guard at `prompt.rs:246` and thread `all_args` into `match_brace_form` (`:236`). The `${@:N}` / `${@:N:L}` slice family at `:256-310` is CORRECT and unaffected — re-verified this pass against `prompt-templates.ts:74-96`.

**Verify** — unit tests for `${@:-fallback}` and `${ARGUMENTS:-fallback}`, with and without args.

## CFG-028 — Config-value `!command` resolution blocks a tokio worker for up to 10 s

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

> **Severity corrected medium → low by the refuter.** The evidence is accurate, the rating was not: pi's
> `execSync` blocks its ONE event loop for the same 10 s, while cyrup runs
> `#[tokio::main(flavor = "multi_thread")]` (`crates/cyrup/src/main.rs:40`) and blocks one worker of N.
> cyrup is strictly *less* blocking than upstream, so there is no behavioural divergence to close —
> this is a robustness note kept on the list, not a parity gap.

**cyrup** — `cyrup/crates/cyrup-config/src/config_value.rs:306-355` `run_with_timeout` is fully synchronous: `Instant::now()` (`:330`), `Duration::from_millis(10_000)` (`:331`), elapsed check (`:345`), `std::thread::sleep(Duration::from_millis(10))` (`:349`). `grep -rn spawn_blocking crates/cyrup-config/src/` returns nothing, and it is reached from two async fns: `AuthStore::get_api_key` (`cyrup-config/src/auth.rs`) and `ConfiguredApiKeyAuth::resolve` (`cyrup-config/src/provider_compose.rs:206-212`).

**upstream** — `pi/packages/coding-agent/src/core/resolve-config-value.ts:186-196` @v0.83.0 uses `execSync(command, { timeout: 10000 })` inside its async resolve.

**Impact** — a slow `!command` credential helper occupies one tokio worker thread for up to 10 s, degrading unrelated concurrent work. No user-visible parity difference.

**Fix** — add an async entry point in `config_value.rs` wrapping the blocking body in `tokio::task::spawn_blocking`, called from `auth.rs` and `provider_compose.rs:206-212`. Do NOT change the 10 s ceiling — that is pi's number.

**Verify** — test that a 1 s `!sleep 1` credential resolve does not delay a concurrently-spawned task by more than a few milliseconds.

## CFG-030 — Non-object top-level `settings.json` degraded to `{}` with no load error

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

> **Severity corrected medium → low by the refuter.** Both sides mangle the document; the only delta is
> that pi preserves array elements as meaningless indexed keys (`{0:1,1:2,2:3}`) while cyrup drops them.
> Neither preserves anything the user intended, load behaviour is identical (all defaults), and the
> trigger — a `settings.json` whose top level is an array / string / number / null — is pathological.

**cyrup** — `Settings::parse` at `cyrup/crates/cyrup-config/src/settings.rs:186-200` deserializes into `serde_json::Value` (`:191`) then `match value { Value::Object(mut obj) => …, _ => Ok(Self::default()) }` (`:192-199`), commented "// A non-object top-level is treated as empty (degraded), never a panic." at `:198`. So `[1,2,3]` parses Ok, produces no `ScopedError`, never reaches `record_load_error`, leaves `ensure_scope_writable` (`:1329-1337`) satisfied, and the next `/config` write takes the `Some(Ok(default))` branch at `:1360` and rewrites the file from an empty document.

**upstream** — `pi/packages/coding-agent/src/core/settings-manager.ts:389` @v0.83.0 does a bare `JSON.parse` then `migrateSettings`, and `persistScopedSettings` (`:585-593`) spreads the parsed document (`{ ...currentFileSettings }`), preserving array elements as indexed keys.

**Impact** — a `settings.json` that is valid JSON but not an object is silently emptied on the next write, with no diagnostic and no write refusal. CFG-001's protections all apply to malformed TEXT and none to this case.

**Fix** — one line: make `Settings::parse` return `Err` for a non-object top level at `settings.rs:198`; the latch, the diagnostic and all write refusals then apply unchanged.

**Verify** — add a `[1,2,3]` case to CFG-001's byte-equality suite (which today seeds only malformed text): a `/config` write must be refused and the file byte-identical afterwards. Fails at HEAD.

## CFG-040 — `markdown.mermaid` settings key and its getter/setter are absent

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `grep -rni 'mermaid' crates/cyrup-config/src` returns ZERO. `EffectiveSettings`' only markdown accessor is `code_block_indent()` (`cyrup/crates/cyrup-config/src/settings.rs:840-844`, reading `["markdown","codeBlockIndent"]`); no sibling reads `markdown.mermaid` and there is no setter for it. The only `mermaid` mentions in the workspace are two comment lines in `cyrup/crates/cyrup-tui/src/markdown.rs:964-965` quoting upstream's fence predicate.

**upstream** — `pi/packages/coding-agent/src/core/settings-manager.ts:57` @v0.84.1 declares `export type MermaidRenderingMode = "off" | "final" | "streaming"` and `:61` adds `mermaid?: MermaidRenderingMode; // default: "streaming"` to `MarkdownSettings` (absent at v0.83.0). Getter `getMermaidRenderingMode()` at `:1251-1254` (validated, defaulting to `"streaming"`), setter at `:1257-1262` writing through `markModified("markdown", "mermaid")`.

**Impact** — a user cannot turn mermaid rendering off or restrict it to final output; the key is inert and `/settings` has no row for it. Masked today because cyrup renders no mermaid at all (PARITY-GAPS records the renderer half) — the moment that lands, the off-switch is missing and there is no way back to a plain code fence.

**Fix** — add `MermaidRenderingMode` plus `mermaid_rendering_mode()` beside `code_block_indent()` (`settings.rs:840-844`), validating against the three-member list with `"streaming"` as the fallback exactly as `settings-manager.ts:1251-1254` does, and a `set_mermaid_rendering_mode` using `set_nested(&["markdown","mermaid"], …)` so sibling markdown keys survive. Land with the renderer work.

**Verify** — unit test beside the `code_block_indent` assertion (`settings.rs:1878`): `{"markdown":{"mermaid":"off"}}` → `Off`; an unknown value and an absent key both → `Streaming`; a `set_mermaid_rendering_mode` round-trip preserves a sibling `markdown.codeBlockIndent`.

## CFG-041 — `default_model_per_provider` is missing v0.84.1's `baseten` and `qwen-token-plan-individual`

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/model.rs:936-982` `default_model_per_provider` has 37 arms and neither `baseten` nor `qwen-token-plan-individual`; `KNOWN_PROVIDERS` (`:985-1031`) matches it arm-for-arm and likewise has neither. `grep -rni 'baseten|qwen-token-plan-individual' crates/ --include='*.rs'` returns ZERO. Because `first_default_or_first` (`:1034-…`) scans `KNOWN_PROVIDERS` in order, a provider absent from the list can never contribute a curated default at launch step 4.

**upstream** — `pi/packages/coding-agent/src/core/model-resolver.ts:48` @v0.84.1 adds `baseten: "zai-org/GLM-5.2"` and `:56` adds `"qwen-token-plan-individual": "qwen3.8-max"` (neither present at v0.83.0; the map is 38 entries there and 40 at v0.84.1). The map's `Object.keys` order is the launch scan order at `:683-692` @v0.84.1.

**Impact** — a user whose only credential is a Baseten or Qwen-Individual key launches on no curated default: step 4 falls through the whole known-provider scan and takes `availableModels[0]`, a different model from pi's on identical inputs. Small blast radius today (PARITY-GAPS records that neither provider is registered at all), but the map is the launch contract and will be wrong the moment they are.

**Fix** — add both arms to `default_model_per_provider` and the matching ids to `KNOWN_PROVIDERS` at pi's insertion positions — `baseten` between `together` and `opencode`, `qwen-token-plan-individual` immediately after `qwen-token-plan-cn` — since position is load-bearing. One changeset with CFG-019's `xai`/`radius` corrections.

**Verify** — extend CFG-019's proposed table test to assert cyrup's map equals pi v0.84.1's 40 entries key-for-key AND in the same order, so the next upstream addition fails loudly. No test pins the current 37 entries.

## CFG-043 — An invalid `models.json` reports a serde parse error instead of pi's per-field schema report

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/model.rs:1700-1717` `load_models_file_reporting` has exactly two messages — `Failed to parse models.json: {e}\n\nFile: {path}` for `ConfigError::Serde` (`:1704-1710`) and `Failed to load models.json: {e}\n\nFile: {path}` otherwise (`:1711-1716`). `load_models_file` (`:1680-1690`) is a single `serde_json::from_str::<ModelFile>` at `:1689`, so a wrong-typed field (e.g. `"contextWindow": "big"`) surfaces as serde's `invalid type: string "big", expected u64 at line N column M` under the PARSE heading, first error only. `grep -n 'Invalid models.json' crates/` returns nothing.

**upstream** — `pi/packages/coding-agent/src/core/model-config.ts` @v0.83.0 keeps three DISTINCT messages: `Failed to load models.json: …` for the read error (`:253-256`), `Failed to parse models.json: …` for the JSON syntax error (`:265-270`), and `Invalid models.json schema:\n{errors}\n\nFile: {path}` for a schema failure (`:272-279`), where `errors` is EVERY validation error rendered as `  - ${formatValidationPath(error)}: ${error.message}` (`:274-277`) with the dotted instance path from `formatValidationPath` (`:217-228`). Unchanged at v0.84.1.

**Impact** — a user with a typo'd field type is told the file failed to PARSE — which points at JSON syntax, not at the field — and is shown only the first offender, by byte offset rather than key path. pi names every bad field as `providers.mycorp.models.0.contextWindow: Expected number`, so the fix is one read; cyrup's message sends the user hunting for a missing comma that does not exist.

**Fix** — split the error in `model.rs`: keep `Failed to parse models.json` for a `serde_json::from_str::<Value>` step, then deserialize the `Value` into `ModelFile` with `serde_path_to_error` (or hand-walk the providers map, deserializing each block independently) and render `Invalid models.json schema:\n` followed by one `  - <dotted.path>: <message>` line per failure. Per-provider deserialization also lets one bad block be rejected while the rest load — the shape `ModelFile::compose` already uses (`:1727-1740`) — and is what CFG-046 needs.

**Verify** — test beside the existing `Failed to parse models.json` assertion (`model.rs:2840`): `{"providers":{"mycorp":{"models":[{"id":"m","contextWindow":"big"}]}}}` must produce a message starting `Invalid models.json schema:` and containing `providers.mycorp.models.0.contextWindow`. Fails at HEAD.

## CFG-044 — Three `auth-storage.ts` provenance cites resolve to nothing upstream, and `get_auth_status` is dead

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — three provenance comments name upstream symbols that do not exist at the ported tag: `cyrup/crates/cyrup-config/src/auth.rs:258-260` ("Pi `AuthStorage.hasAuth`, auth-storage.ts:344-349"), `:271-272` ("Pi `getAuthStatus`, auth-storage.ts:354-369"), `:133-134` ("Pi `AuthStorage.getProviderEnv`, auth-storage.ts:305-308"). `auth-storage.ts` is **271 lines at v0.83.0**, so all three ranges are past end-of-file. `get_auth_status` (`:273-306`) additionally has ZERO production callers: `grep -rn get_auth_status crates/ --include='*.rs'` returns the definition, two `#[cfg(test)]` assertions (`:793`, `:801`) and two doc mentions (`cyrup-config/src/login.rs:356-359`, `cyrup-tui/src/auth_select.rs:42`). The live status function is `cyrup_config::login::provider_auth_status` (`login.rs:360-395`), whose own doc already records that `AuthStore::get_auth_status` "reports `configured: false` for the runtime and environment tiers" and cites "a function that no longer exists at this tag".

**upstream** — `git grep -n 'hasAuth\b' v0.83.0 -- packages` returns NOTHING (only a test helper `hasAuthForProvider` at pi HEAD); `git grep -n getAuthStatus v0.83.0 -- packages` returns only prose in `packages/agent/docs/models.md:874` recording that `AuthStorage` was deleted and its `getAuthStatus` moved to a ModelRegistry facade; `getProviderEnv` returns nothing. The real equivalents at v0.83.0 are `model-runtime.ts:372-374` (`hasConfiguredAuth`) and `model-runtime.ts:428-437` (`getProviderAuthStatus`) — exactly what `login.rs:22` already cites.

**Impact** — CLAUDE.md makes these comments the provenance record, and three assert an upstream that never existed at the ported tag. A maintainer "restoring parity" against them will re-derive the wrong semantics — specifically `get_auth_status`'s `configured: false` for the runtime and environment tiers, the OPPOSITE of `getProviderAuthStatus`'s `{ configured: true, source: "runtime" | "environment" }` — and a dead function with a plausible-looking cite is exactly what a later reader wires up by mistake. Same class as the `subagent-executor.ts:3022` false precedent already recorded in the residual ledger's Corrections.

**Fix** — repoint the three comments at real code: `has_auth` (`:258-260`) → `ModelRuntime.hasConfiguredAuth`, `model-runtime.ts:372-374` @v0.83.0 (noting the models.json tier lives in `provider_is_configured`); `provider_env` (`:133-134`) → the `resolution.env` construction in `model-runtime.ts` `prepareRequest`; then DELETE `AuthStore::get_auth_status` (`:273-306`) with its two tests, since `login::provider_auth_status` is the ported function and the only one with a caller. Update the dangling doc at `cyrup-tui/src/auth_select.rs:39-42` in the same change. Sequence against CFG-007, which otherwise caches a function slated for deletion.

**Verify** — after the change, `grep -rn 'auth-storage.ts:' crates/` must not name `hasAuth`, `getAuthStatus` or `getProviderEnv`, and `grep -rn get_auth_status crates/` must return nothing. Cross-check each surviving cite by opening the named line at `git show v0.83.0:<path>`.

## CFG-047 — Three built-in slash-command metadata divergences (`/model`, `/login`, `/reload`)

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-tui/src/commands.rs:49-72`. Comparing entry by entry against pi's `BUILTIN_SLASH_COMMANDS`, the 22 names and their order match exactly and 19 of 22 descriptions are byte-identical. Three do not: (a) `/model` is `arg_cmd("model", …, "<model>")` where pi's hint is `"<provider/model>"`; (b) `/login` is `cmd("login", "Configure provider authentication", None)` — the hint is dropped entirely, even though `resolve_login_command` (`cyrup/crates/cyrup-config/src/login.rs:589`) does accept a provider argument; (c) `/reload`'s description is "Reload keybindings, extensions, skills, prompts, and themes".

**upstream** — `pi/packages/coding-agent/src/core/slash-commands.ts:19-42` @v0.83.0: `/model` carries `argumentHint: "<provider/model>"`; `/login` is `{ name: "login", description: "Configure provider authentication", argumentHint: "<provider>" }`; `/reload`'s description is "Reload keybindings, extensions, skills, prompts, themes, **and context files**".

**Impact** — autocomplete never tells the user that `/model anthropic/claude-x` and `/login anthropic` are accepted, so a documented affordance is invisible. **Worth a second look while fixing (c):** if cyrup's shortened `/reload` string is HONEST, then `/reload` also fails to re-read `AGENTS.md` / `CLAUDE.md` context files — a behaviour gap rather than a string gap, belonging to whoever owns the reload path (area 03/07). Determine which before editing the string.

**Fix** — correct the two argument hints at `commands.rs:49-72`. For `/reload`, first check whether the reload path re-reads context files; restore pi's full description if it does, or file the missing reload step against the owning area if it does not.

**Verify** — a table test asserting cyrup's 22 built-in commands equal pi's name-for-name including `argumentHint` and `description`, so the next upstream string change fails loudly. Nothing pins these strings today.

## CFG-050 — `migrate_tools_to_bin` moves the managed `fd`/`rg` binaries with no completion notice

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup/src/migrations.rs:177-199` `migrate_tools_to_bin` reproduces pi's loop faithfully — the four managed names `fd`/`rg`/`fd.exe`/`rg.exe`, `bin/` created on demand, rename when the destination is free, delete the stale source when the destination already exists — but tracks no `moved_any` flag and **emits nothing**. Contrast the sibling migration in the same file: `migrate_commands_to_prompts` (`:212-234`) does route its success line through `crate::output_guard::emit_stray_line("Migrated {label} commands/ → prompts/")` at `:220`, so the omission is inconsistent within cyrup rather than a deliberate house style.

**upstream** — `pi/packages/coding-agent/src/migrations.ts:185` `let movedAny = false;`, set true on each successful `renameSync` (`:203`), and `:213-215` — `if (movedAny) { console.log(chalk.green('Migrated managed binaries tools/ → bin/')); }`. Identical at v0.84.1.

**Impact** — a user who has been pointing scripts, settings or a `PATH` entry at `~/.cyrup/agent/tools/rg` finds the binaries gone after an upgrade with nothing in the output saying where they went. pi tells them. Low because the tools themselves keep working through cyrup's own resolver — the cost is a confusing silent filesystem change, not broken function.

**Fix** — add a `moved_any` bool to `migrate_tools_to_bin` (`migrations.rs:177-199`), set it on each successful `std::fs::rename`, and after the loop emit `crate::output_guard::emit_stray_line("Migrated managed binaries tools/ → bin/")` when true — the same guard `migrate_commands_to_prompts` already uses at `:220`, so the line is rerouted to stderr under PRINT/JSON/RPC and cannot corrupt a machine-readable stdout.

**Verify** — extend the `migrates_commands_dir_and_warns_on_legacy_dirs` neighbourhood in `migrations.rs`'s test module: create `<agent_dir>/tools/rg`, run `run_migrations`, assert the file is now at `<agent_dir>/bin/rg` and the notice was emitted (via the `output_guard` seam or a returned flag). A second run must emit nothing.

## CFG-051 — The migrated-credentials notice is written to stderr pre-TUI instead of into the transcript, on a cite that points at unrelated upstream code — **CLOSED 2026-08-14**

> **CLOSED 2026-08-14 (sweep 8) on both halves of the residual — see the table row above for the
> live transcript and the pin. Everything below is the filing text.** Note the deliberate deviation
> recorded there: the pin is **in-src** (`crates/cyrup-tui/src/transcript.rs:3466`) rather than in
> `crates/cyrup-tui/tests/` as the Verify asked, because `entry_lines`/`TranscriptView::lines` are
> crate-private and widening the public API for a test is the worse trade.


**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup/src/main.rs:520-527`: the comment reads "Migrated-credential notice (Pi `InteractiveMode` startup warning, interactive-mode.ts:797)" and the code is `if !migration.migrated_auth_providers.is_empty() { eprintln!("Warning: Migrated credentials to auth.json: {}", migration.migrated_auth_providers.join(", ")); }` — a raw stderr write on the pre-TUI path, immediately followed by the deprecation-warning block (`:529-533`, CFG-049) and TUI init.

**upstream** — pi does not print this before the UI. `pi/packages/coding-agent/src/main.ts:607` threads `migratedAuthProviders` into the interactive mode's options (`migratedProviders?: string[]`, `interactive-mode.ts:308`), and `interactive-mode.ts:872-875` @v0.83.0 renders it **inside** the running UI: `const { migratedProviders, … } = this.options; if (migratedProviders && migratedProviders.length > 0) { this.showWarning(\`Migrated credentials to auth.json: ${migratedProviders.join(', ')}\`); }` — a styled warning entry in the transcript, scrollable for the whole session. **The in-tree cite is simply wrong:** `interactive-mode.ts:797` at v0.83.0 is `await this.rebindCurrentSession();`, unrelated to credentials, and v0.84.1 does not carry it either. Same class as the three false `auth-storage.ts` cites already filed as CFG-044.

**Impact** — the one notice telling a user their OAuth tokens and API keys were relocated out of `oauth.json`/`settings.json` into `auth.json` — a change that silently invalidates any tooling or backup pointing at the old files — is emitted exactly where the first TUI frame overwrites it, instead of into the transcript where pi keeps it. Low because the migration itself is correct and idempotent and the credentials keep working; the cost is a one-shot notice about a filesystem change that the user very likely never sees.

**Fix** — thread `migration.migrated_auth_providers` into the TUI the way pi threads `migratedProviders` (`main.ts:607` → `interactive-mode.ts:308` → `:872-875`) and render it through the transcript's warning path after the first frame, rather than `eprintln!` at `main.rs:522-527`. Keep the stderr form for non-interactive modes only. Correct the cite in the comment from `interactive-mode.ts:797` to `interactive-mode.ts:872-875` @v0.83.0 — and note the ordering interaction with CFG-049: once the deprecation gate blocks on a keypress, this line becomes readable by accident, which is not the same as being ported.

**Verify** — app test in `crates/cyrup-tui/tests/`: seed a legacy `oauth.json`, boot, and assert the **rendered transcript** contains `Warning: Migrated credentials to auth.json: anthropic` after the first draw — not merely that stderr received it. Confirm in a live terminal run that the line is still visible and scrollable after the UI has painted.

## CFG-014 — `showCacheMissNotices` and prompt-cache-miss tracking absent

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `grep -rn 'showCacheMissNotices|show_cache_miss' crates/ --include='*.rs'` returns ZERO at HEAD; not among `EffectiveSettings`' accessors (`cyrup/crates/cyrup-config/src/settings.rs:509-1000`). A full 47-key sweep of pi's v0.84.1 Settings interface found this as one of only THREE keys with zero occurrences in `settings.rs` — the other two are `tuiMode` / `fullscreenScrollbar` (CFG-021).

**upstream** — `pi/packages/coding-agent/src/core/settings-manager.ts:96` @v0.83.0 declares the key (default false), getter `:850-852`, setter `:872-875`.

**Impact** — no way to surface prompt-cache misses; a user debugging cache behaviour has no signal.

**Fix** — add the accessor and setter in `settings.rs`, detect the miss off the usage block in `cyrup-provider`, and add the `/config` row.

**Verify** — a faux-provider run with a cache-miss usage block emits the notice when the setting is on and nothing when off.

## CFG-015 — Five unconsumed settings accessors, incl. `lastChangelogVersion` and `collapseChangelog`

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — four accessors have zero consumers anywhere outside `cyrup/crates/cyrup-config/src`: `code_block_indent` (`settings.rs:840-844`), `last_changelog_version` (`:994-996`), `npm_command` (`:742`), `warnings().anthropic_extra_usage` (`:884-890`, field `:52`). **A fifth key is folded in this pass** — `collapse_changelog`, whose only consumer outside `settings.rs` is the display row `cyrup/crates/cyrup-tui/src/app/settings_rows.rs:115` (`SettingRow::toggle("collapseChangelog", "Collapse changelog", eff.collapse_changelog())`), i.e. a `/settings` row over a value nothing reads. This item is also `PARITY-GAPS` PB-6's home: `/changelog` is still hardcoded in `cyrup-tui/src/app/submit.rs:111-112` (`push_block("What's New", "No changelog entries found.")`) and `grep -rn 'parse_changelog|CHANGELOG' crates/ --include='*.rs'` finds no parser anywhere.

**upstream** — `pi/packages/coding-agent/src/core/settings-manager.ts` @v0.83.0: `:55` `codeBlockIndent`, `:59` `anthropicExtraUsage`, `:84` `lastChangelogVersion` (getter `:660-662`, setter `:664-667`), `:99` `collapseChangelog` ("Show condensed changelog after update (use /changelog for full)"), `:102` `npmCommand`.

**Impact** — five documented settings do nothing when set; two of them (`lastChangelogVersion`, `collapseChangelog`) are the whole state model for a changelog feature cyrup does not have.

**Fix** — wire `code_block_indent` into `cyrup-tui`'s markdown renderer and `anthropicExtraUsage` into the Anthropic provider's warning path. Land `lastChangelogVersion` + `collapseChangelog` together with the changelog parser (PB-6) so all of that feature's state closes at once rather than in halves. `npm_command()` stays blocked behind the unported npm channel (CFG-009) — a consequence, not an independent gap.

**Verify** — one assertion per key at its consumption site; for the changelog pair, a test that a version bump renders the condensed form when `collapseChangelog` is true and the full form when false.

## CFG-027 — A local package that is a bare extension directory contributes nothing

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-resources/src/discovery.rs:813-916`: after `resolve_manifest(&dir)` (`:815`) every resource comes from the manifest lists, and the extension loop at `:887-916` pushes only `manifest.extensions` entries — nothing ever pushes `dir` itself onto `ext_paths`. `resolve_configured_package` additionally hard-errors on a non-directory at `:401-413`.

**upstream** — `pi/packages/coding-agent/src/core/package-manager.ts:1316-1344` @v0.83.0 `resolveLocalExtensionSource`: a missing path is a silent skip (`if (!existsSync(resolved)) return;`, not an error); a FILE entry goes straight to `accumulator.extensions`; a DIRECTORY whose `collectPackageResources` returns false falls back to `this.addResource(accumulator.extensions, resolved, metadata, true)`.

**Impact** — `"packages": ["./my-ext"]` where `./my-ext` is a bare extension with no manifest loads nothing. Narrow: `--extension`/`-e` and the settings `extensions` array both cover the need.

**Fix** — in `discovery.rs:813-916`, when `resolve_manifest` yields nothing, push `dir` onto `ext_paths`; relax the non-directory error at `:401-413` to accept a file entry as an extension and a missing path as a silent skip.

**Verify** — test: a settings-declared local package that is a manifest-less extension directory registers as an extension.

## CFG-042 — `FileModelsStore` does not normalize its path, cache by file revision, or accept cancellation — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15.** The full disposition is in the Open-items row. Two corrections to the text
> below, because each one would misdirect a re-reader:
>
> 1. **The `Fix`'s "Add a `CancelToken` parameter to the `ModelsStore` trait" understates the shape.**
>    Upstream's addition is a one-field OPTIONS BAG (`ModelsStoreOperationOptions`,
>    `packages/ai/src/models-store.ts:16-18` @v0.84.1), not a bare signal, and that is what landed —
>    it is the shape pi chose so the surface can grow without re-breaking three signatures.
> 2. **The `upstream` paragraph's "`read`/`write`/`delete` all take `ModelsStoreOperationOptions`"
>    is right about the fact and silent about the PLACEMENT, which is where the behaviour is.** pi
>    checks `read` twice (`:85` and `:121`) and hands `options` to `withLockAsync` for `write`/`delete`
>    (`:132`, `:143`) — i.e. **before** the lock. A check inside the lock would pass a naive test and
>    still block every other process for the duration of a write nobody wants.


**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/models_store.rs:49-51` `FileModelsStore::new(path: impl Into<PathBuf>)` stores the path raw — no tilde / `file://` normalization. `read` (`:89-97`) takes the cross-process lock and calls `read_all` (`:67-72`), which does a full `read_to_string` + `serde_json::from_str` on EVERY call; there is no revision field, no shared read state, no cached snapshot. The trait methods (`:89-116`) take no options argument, so an in-flight read cannot be cancelled. Secondary: `read_all`/`write_all` use `BTreeMap` (`:67`, `:74`), so the file is rewritten with keys SORTED where pi preserves insertion order.

**upstream** — `pi/packages/coding-agent/src/core/models-store.ts` @v0.84.1: the constructor normalizes (`this.path = normalizePath(path)`, `:53`) and adopts a process-wide `sharedModelsFileReadState` (`:23`, `:55-58`); `readLatest` (`:81-108`) short-circuits on `getFileRevision(this.path) === readState.revision` (`:86-87`) and otherwise coalesces concurrent readers onto one in-flight reload; `read`/`write`/`delete` all take `ModelsStoreOperationOptions` and call `options?.signal?.throwIfAborted()` (`:120-122`, `:127-137`, `:139-149`). None of this exists at v0.83.0, where `read` was a bare `storage.withLock(...)`. `InMemoryCodingAgentModelsStore.read`/`write` also gained `structuredClone`.

**Impact** — every catalog-overlay lookup re-reads and re-parses `models-store.json` under a cross-process lock instead of answering from a revision-checked snapshot, paying a syscall + a JSON parse each time and serializing against any concurrent `cyrup update --models`. `FileModelsStore::new("~/alt/models-store.json")` silently targets a literal `~` directory. A hung read cannot be aborted. Low today (cyrup builds the overlay once at session start) — it matters as the mechanism CFG-020's snapshot would need.

**Fix** — give `FileModelsStore` a `RwLock<{ data, revision }>` alongside `path`, normalize the path in `new` (`:49-51`) through the shared util CFG-025/CFG-036 introduce, add a `file_revision(&self)` from mtime+size, and have `read` (`:89`) answer from the cached map when the revision is unchanged, refreshing under the existing `FileLock` otherwise; update the cache in `write`/`delete` (`:99-116`) from the map they already computed. Add a `CancelToken` parameter to the `ModelsStore` trait (`cyrup-provider/src/models_store.rs`) to carry pi's `signal`. If byte-interop with pi matters, swap `BTreeMap` for an insertion-ordered map.

**Verify** — test beside `round_trips_through_the_file_and_survives_a_restart` (`models_store.rs:139`): after one `read`, make the file unreadable and assert a second `read` still returns the cached entry; then rewrite the file and assert the next `read` observes it. Plus a path test: `FileModelsStore::new("~/x/models-store.json")` with HOME overridden resolves under the home dir. Both fail at HEAD.

## CFG-021 — `tuiMode` / `fullscreenScrollbar` not modelled

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed

> **Misdescribed twice, corrected in place (id retained).** (1) The key name `uiMode` does not exist
> anywhere in pi — the real key is `tuiMode`. (2) BOTH keys are **v0.84.1 additions**
> (`git show v0.83.0:…/settings-manager.ts` has neither), so the kind moves from `not-ported` to
> `upstream-drift`. The finding itself survives: cyrup models neither key.

**cyrup** — `grep -rni 'uiMode|tuiMode|fullscreenScrollbar|fullscreen_scrollbar' crates/ --include='*.rs'` returns ZERO at HEAD. Confirmed by the independent 47-key sweep of pi's v0.84.1 Settings interface against `cyrup/crates/cyrup-config/src/settings.rs`: these two and `showCacheMissNotices` (CFG-014) are the only keys with a zero count.

**upstream** — `pi/packages/coding-agent/src/core/settings-manager.ts:135` @v0.84.1 declares `tuiMode?: TuiMode` (getter `:1128-1133`, setter `:1135-1140`) and the sibling `fullscreenScrollbar?: ScrollViewScrollbar`.

**Impact** — none today; deferred with the fullscreen TUI mode itself. Interlocks with `PARITY-GAPS` VL-P19 (the alt-screen renderer) and with CFG-040, which also waits on renderer work. The companion theme token `scrollbarThumb` already landed (CFG-034, closed).

**Fix** — land with the fullscreen viewport work: add both accessors and setters in `settings.rs` and the `/settings` rows, then consume them in the alt-screen renderer.

**Verify** — settings round-trip test once the mode exists, plus a `/settings` row assertion.

## CFG-054 — installed package working tree lands under a doubled `packages/packages/` segment — **CLOSED 2026-08-15 (REFUTED: already fixed at HEAD)**

> The evidence below described HEAD `04c1ba2`-era code and is stale at `68bbd39`. `packages_root`
> no longer doubles, and a startup migration moves trees an older build wrote. See the Open-items
> row. **The `Fix` paragraph's two options were both live when written; the first was taken.**


**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-resources/src/package/store.rs:26-40` — `PackageStore` is constructed with `global_dir = package_dir`, which already defaults to `<agent_dir>/packages` (`crates/cyrup-config/src/env.rs:191-196`). `packages_root(Global)` then returns `global_dir.join("packages")` and `package_dir(scope, id)` appends the sanitized id, so a cloned package's working tree is `<agent_dir>/packages/packages/<id>`. `registry_path(Global)` does **not** double — it is `global_dir.join("packages.json")` = `<agent_dir>/packages/packages.json`, **verified empirically** by installing a local package under an isolated `CYRUP_AGENT_DIR`. Project scope does not double either (`<cwd>/.cyrup/packages/<id>`), so the two scopes disagree in shape.

**upstream** — no upstream basis; pi has no equivalent two-level join. Filed `cyrup-original`.

**Impact** — the path a user must open to inspect, patch or delete an installed package is not one any document would naturally state, and differs in shape between global and project scope. Cosmetic, but it is the path documentation has to name.

**Fix** — either drop the `.join("packages")` in `packages_root(Global)` or construct `PackageStore` from `agent_dir` rather than `package_dir`. `CYRUP_PACKAGE_DIR` / `PI_PACKAGE_DIR` override `package_dir` directly, so whichever is chosen must keep that override meaningful; either change is a migration for existing installs.

**Verify** — `CYRUP_AGENT_DIR=$(mktemp -d) cyrup install git:github.com/<u>/<r>`, then assert the clone directory and `packages.json` sit at the same level.

## CFG-055 — `cyrup remove` may not match the `PackageId` that `cyrup install` stored — **CLOSED 2026-08-15 (REFUTED: already fixed at HEAD)**

> Stale at `68bbd39`: `remove_candidate_ids` normalizes first and falls back to the raw id, and
> `update`'s positional target does the same. **The "upstream leg not established" caveat is
> discharged, not inherited** — pi normalizes too (`packageSourcesMatch`,
> `package-manager.ts:1418-1422` @v0.83.0 over `getSourceMatchKeyFor{Settings,Input}`
> `:1362-1383`). See the Open-items row.


**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** confirmed on the cyrup side; upstream leg not established

**cyrup** — `crates/cyrup/src/subcommands.rs:445-447` builds `PackageId::from(source_str)` from the raw argument, while the install path records the normalized form produced by `PackageSource::parse` → `PackageId` (`crates/cyrup-resources/src/package/source.rs:100-111`, `:180-190`) — `git:<host>/<user>/<repo>` or `path:<canonical-abs-path>`, with every non-`[A-Za-z0-9._-]` character replaced by `-`. `CFG-026` ported `getPackageIdentity` and wired pi's two call sites (`dedupePackages`, `findAutoloadDeltaBase`); the `remove` round trip is not among them, and no test covers it.

**upstream** — pi's remove path @v0.83.0 was **not re-read this pass**. Establish it before writing a fix.

**Impact** — `cyrup remove <source>` can report success, or "not installed", while leaving the registry row in place — for any spelling that normalizes: an `https://` URL, a relative path, a `@ref` suffix, an scp-style `git@host:u/r`. The user's remedy is undiscoverable because `cyrup list` prints the source display, not the id.

**Fix** — route `remove` through the same `PackageSource::parse` → `PackageId` pipeline `install` uses and match on the normalized id, keeping an exact-string fallback for legacy rows.

**Verify** — install by each accepted source spelling, then remove by the same string and by a differing-but-equivalent one; assert the row is gone in the first case and a diagnostic names the mismatch in the second.

## CFG-056 — `defaultThinkingLevel`'s unset-fallback was `off` where pi's is `medium` — **CLOSED 2026-08-14 (FIXED THIS PASS)**

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed

**upstream** — `packages/coding-agent/src/core/defaults.ts:3` @v0.83.0 — `export const DEFAULT_THINKING_LEVEL: ThinkingLevel = "medium";`. It is the ONLY export of defaults.ts. Applied at `core/sdk.ts:230` and `:235` (`settingsManager.getDefaultThinkingLevel() ?? DEFAULT_THINKING_LEVEL`), `core/agent-session.ts:1738` (same expression), and `core/model-resolver.ts:594` (`let thinkingLevel: ThinkingLevel = DEFAULT_THINKING_LEVEL`), `:608`, `:616`, `:642`, `:647`, `:651`. The getter itself, `settings-manager.ts:740-742`, deliberately returns `ThinkingLevel | undefined` so each site names this fallback.

**cyrup (as filed)** — `EffectiveSettings::default_thinking_level()` ended `.unwrap_or_default()` on `ModelThinkingLevel`, whose `#[default]` is `Off` (`crates/cyrup-core/src/message.rs:44-48`). The same wrong constant appeared a second time as `let default_level = ModelThinkingLevel::default();` in `find_initial_model` (`crates/cyrup-config/src/model.rs`), the terminal fallback in all five arms — pi's `DEFAULT_THINKING_LEVEL` at `model-resolver.ts:594/608/616/642/647/651`. Consumed at `crates/cyrup-session-svc/src/builder.rs` (pi's `sdk.ts:223-236` rung, arm for arm), after which `clamp_thinking_level(model, Off)` is still `Off`.

**Impact** — every user who had never written `defaultThinkingLevel` into `settings.json` started every session with reasoning DISABLED where pi starts at `medium`. Silent (no warning; the `/settings` row reports what the getter says), survives session restore, and on reasoning models it changes output quality and cost on the very first turn.

**Fix — LANDED 2026-08-14.** (a) New `crates/cyrup-config/src/defaults.rs`, a one-constant module mirroring pi's one-export `defaults.ts`, exporting `DEFAULT_THINKING_LEVEL = ModelThinkingLevel::Medium` and stating in its doc why `ModelThinkingLevel::default()` is *not* it. (b) `EffectiveSettings::default_thinking_level()` now returns `Option<ModelThinkingLevel>`, matching pi's `ThinkingLevel | undefined` — this closes the `differingShape` half of the same finding, and it is the mechanism that keeps the value correct: the fallback is now spelled in the source at each site instead of hiding inside a `Default` impl. (c) `model.rs`'s `default_level` is `crate::DEFAULT_THINKING_LEVEL`. (d) `builder.rs`'s three sites go through one `settings_default` closure that is literally `…default_thinking_level().unwrap_or(cyrup_config::DEFAULT_THINKING_LEVEL)`. **`ModelThinkingLevel::default()` was deliberately NOT changed** — `Off` is correct as the type's zero and `builder.rs` relies on it for the modelless branch (pi `sdk.ts:238-240`).

**Verify** — `unset_default_thinking_level_is_none_and_falls_back_to_medium` in `crates/cyrup-config/src/settings.rs` asserts the getter returns `None` for `{}`, that the named fallback is `Medium`, and that `DEFAULT_THINKING_LEVEL != ModelThinkingLevel::default()` — all three RED before the change. The two pre-existing `default_thinking_level_*` tests were re-expressed against the `Option`. `cargo check -p cyrup-config -p cyrup-session-svc -p cyrup --all-targets` clean.

## CFG-057 — `httpProxy` was read from the merged view; pi reads it from the GLOBAL layer only — **CLOSED 2026-08-14 (FIXED THIS PASS)**

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — `packages/coding-agent/src/main.ts:537` (`applyHttpProxySettings(bootstrapSettingsManager.getGlobalSettings().httpProxy)`) and `:801` (`applyHttpProxySettings(settingsManager.getGlobalSettings().httpProxy)`) @v0.83.0 — both go through `getGlobalSettings()` (`settings-manager.ts:442-444`), which returns the raw GLOBAL document, NOT `this.settings` (the merged view every other getter reads). Confirmed as intentional by `packages/coding-agent/docs/settings.md:87` — "| `httpProxy` | string | - | HTTP proxy URL applied as `HTTP_PROXY` and `HTTPS_PROXY`. **Global setting only.** |". `git grep -nE 'get(Global|Project)Settings\(\)\s*[.\[]' v0.83.0` returns exactly two production keys read this way: `httpProxy` and `npmCommand` (`package-manager-cli.ts:754`, the self-update path only).

**cyrup (as filed)** — `GLOBAL_ONLY_KEYS` was `&["defaultProjectTrust"]`; `EffectiveSettings::http_proxy` reads `self.merged`, and its one production consumer (`crates/cyrup-session-svc/src/builder.rs`) passes the merged `eff`.

**Impact** — a project `.cyrup/settings.json` containing `{"httpProxy": "http://attacker:8080"}` routed that session's provider traffic through the named proxy, where pi ignores the key entirely. The trust gate limits this to an approved project, but approving a project is not approving an egress rewrite. Note the neighbouring `httpIdleTimeoutMs` IS merged upstream, so this is a per-key upstream decision cyrup had flattened, not a category.

**Fix — LANDED 2026-08-14.** `httpProxy` added to `GLOBAL_ONLY_KEYS` in `crates/cyrup-config/src/settings.rs`, with the two upstream call sites and the docs line quoted in the constant's doc comment so the *reason* survives. `strip_global_only` already ran over both the project and the CLI layer, so no other change was needed.

**Verify** — `http_proxy_is_global_only` in `crates/cyrup-config/src/settings.rs`: a global `http://global:8080` beats a project `http://project:9090` even with `project_trusted = true`, and a project-only value yields `None`. RED before the change (it returned the project value). **`PROV-047` is independent and is NOT closed by this** — it is about which egress PATHS see the proxy, not which SCOPE supplies it.

## CFG-058 — `websocketConnectTimeoutMs` has no 15 s default at the connect site — **CLOSED 2026-08-15 (REFUTED: the premise does not hold)**

> **CLOSED 2026-08-15 as REFUTED.** The `upstream` and `cyrup` paragraphs below are both CORRECT and
> were re-derived at v0.83.0 this pass — keep them. **The `Impact` paragraph is what fails**, and with
> it the `Fix` and `Verify`: cyrup opens no WebSocket handshake at all, so an unset key cannot produce
> an unbounded one and there is no connect site to host the constant. Following the `Fix` as written
> ("apply it where the `Option` is consumed") would have introduced a defect — it erases the
> unset-vs-explicitly-15000 distinction pi's `??` chain depends on. See the Open-items row; the
> residual risk is now recorded at `crates/cyrup-provider/src/stream.rs`'s field itself.


**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — `packages/ai/src/api/openai-codex-responses.ts:64` @v0.83.0 — `const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS = 15_000;`, applied at `:1039` as the parameter default `connectTimeoutMs = DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS`. The settings getter returns `undefined` when unset (`settings-manager.ts:842-844`) and `sdk.ts:309-315` threads that `undefined` straight through, so the default lives at the connect site. Documented as the user-visible default in `packages/coding-agent/docs/settings.md:172` — `| websocketConnectTimeoutMs | number | 15000 | ... Set to 0 to disable. |`.

**cyrup** — the settings half is faithful: `EffectiveSettings::websocket_connect_timeout_ms` (`crates/cyrup-config/src/settings.rs`) returns `Ok(None)` when unset and `crates/cyrup-session-svc/src/builder.rs:1510-1512` threads `Some(ms)` onto the builder. The 15 000 ms floor at the other end is absent: `grep -rn --include='*.rs' '15_000' crates/cyrup-provider/` returns nothing, and `StreamOptions.websocket_connect_timeout_ms` (`crates/cyrup-provider/src/stream.rs:214`) is an `Option<u64>` that no connect path defaults.

**Impact** — a user who has not set the key gets an unbounded WebSocket handshake where pi gives up after 15 s and falls back to SSE. **PARTIALLY KNOWN:** `CFG-006` covered this key and closed once `builder.rs:1510` landed; that closure verified the THREADING, not the DEFAULT, so this residual is new — the same "closed one half, named the other" pattern the ledger records.

**Fix** — **FIX SITE: `crates/cyrup-provider`, NOT cyrup-config.** Port the constant next to the WebSocket connect path and apply it where the `Option` is consumed, preserving `0` as "disabled" per the docs line.

**Verify** — a test that constructs `StreamOptions` with `websocket_connect_timeout_ms: None` and asserts the connect deadline is 15 s, plus one asserting `Some(0)` disables it rather than meaning "immediately".

## CFG-059 — A third, persistent `cli` settings layer that pi does not have — **CLOSED 2026-08-15 (REFUTED: already closed at HEAD)**

> **CLOSED 2026-08-15 (batch B, cyrup-config slice) as REFUTED — already closed at HEAD, nothing was
> changed this pass. Everything below is the filing text and is now wrong about the code.**
> `SettingsManager::load` takes `(store, project_trusted)` — two arguments, no `cli` — the struct
> holds exactly `global` + `project` + `effective`, and `recompute()` merges `global ◁ project`
> only, applying `strip_global_only` to the project layer alone. The row's seam citations
> (`builder.rs:369/435/538/593`, `factory.rs:30/49/95/154/185`) still resolve, but they are no
> longer a settings LAYER: `SessionBuilder::cli_settings` now feeds the transient
> `apply_overrides` at `crates/cyrup-session-svc/src/builder.rs:677-678`
> (`if !self.cli_settings.is_empty() { settings.apply_overrides(&self.cli_settings) }`), which is
> pi's `applyOverrides` (`settings-manager.ts:508-510` @v0.83.0) — merged onto the already-merged
> view and discarded by the next `recompute()`. The item's own preferred remedy ("deleting is the
> smaller change today") is what landed; both the `SettingsManager` doc comment and the
> `apply_overrides` doc comment name CFG-059 as the reason.


**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-config/src/settings.rs` carries a `cli: Settings` field on `SettingsManager`, taken by `SettingsManager::load(store, cli, project_trusted)`, and `recompute` merges `global ◁ project ◁ cli`, applying `strip_global_only` to BOTH project and cli. Seams at `crates/cyrup-session-svc/src/builder.rs:369/435/538/593` and `crates/cyrup-session-svc/src/factory.rs:30/49/95/154/185`.

**upstream** — pi v0.83.0 has NO CLI settings tier. `git -C pi grep -n 'applyOverrides' v0.83.0 -- 'packages/**/*.ts'` returns exactly two hits: the method definition (`settings-manager.ts:508-510`) and one SDK example (`examples/sdk/10-settings.ts:17`) — zero production callers.

**Impact** — cyrup carries BOTH a faithful `apply_overrides` (correctly documented as transient) AND this persistent third merge layer. It is currently inert — `grep -rn 'cli_settings(' crates/` finds only the builder/factory setters and their own chaining, never a binary call site — so nothing is mis-merged today. But it diverges in the precedence MODEL, not just in an unused setter: it is stripped by `strip_global_only`, it participates in every `recompute()` including after `reload()` / `set_project_trusted()`, and it outranks project settings — three properties pi's transient `applyOverrides` does not have.

**Fix** — either delete it in favour of `apply_overrides`, or keep it and write a `[CYRUP-DELTA]` on the field naming `settings-manager.ts:508-510` as what it replaces and stating the three property differences. Deleting is the smaller change today and the larger one after a binary starts calling it.

**Verify** — if kept: a test pinning that a `cli` value outranks a project value and survives `reload()`. If deleted: assert `SettingsManager::load` no longer takes the layer and that `apply_overrides` is the only override path.

## CFG-060 — `EffectiveSettings::http_proxy`'s env fallback inverts pi's `??=` precedence — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15 (batch B).** Confirmed at HEAD exactly as filed — the body ended
> `.or_else(|| env.http_proxy.clone())` and both production callers passed `EnvVars::default()` to
> defeat it. Closed by the item's SECOND option: **the env leg and the `&EnvVars` parameter are
> deleted**, leaving `EffectiveSettings::http_proxy(&self)` as pi's `getGlobalSettings().httpProxy`
> and nothing more (`main.ts:537`, `:801` @v0.83.0), with the trim/empty filter kept because
> `applyHttpProxySettings` opens `const proxy = httpProxy?.trim(); if (!proxy) return;`
> (`http-dispatcher.ts:43-48`).
>
> **Why the item's FIRST option — inverting the `or_else` so env wins — would have been a new
> divergence, recorded so it is not re-proposed.** `??=` fills `HTTP_PROXY` and `HTTPS_PROXY`
> *independently*. With an ambient `HTTP_PROXY=A` and `"httpProxy": "S"`, upstream leaves
> `HTTP_PROXY` at `A` and sets `HTTPS_PROXY` to `S`, so an **https** target proxies through the
> SETTING. An inverted accessor would have returned `A` and handed it to `configure_http_proxy`,
> making `A` the configured proxy for both names and losing `S` for https targets entirely. The
> ambient-wins half of `??=` is already ported, once, in `get_proxy_env`
> (`crates/cyrup-provider/src/utils/node_http_proxy.rs`), which consults `configured_http_proxy()`
> only after all four ambient lookups miss — including that function's own recorded corner about an
> ambient empty string.
>
> Call sites updated: `crates/cyrup/src/main.rs` (the bootstrap `configure_http_proxy`) and
> `crates/cyrup-session-svc/src/builder.rs` (`apply_http_proxy_settings`); the comments at both
> sites explaining why `EnvVars::default()` was passed are rewritten, not left stale.
>
> **Test honesty:** `http_proxy_is_the_setting_alone_and_takes_no_environment`
> (`crates/cyrup-config/src/settings.rs`) is labelled IN-FILE as coverage, not proof. The fix is a
> signature removal, so no test can be written against the pre-fix API — the pre-fix behaviour it
> replaces (`http_proxy(&EnvVars { http_proxy: Some("http://ambient:3128"), .. })` returning
> `Some("http://ambient:3128")` with no `httpProxy` key anywhere) is stated in the test's doc
> comment instead of implied by a red run that cannot exist.
>
> The `NO_PROXY` case-folding note below stands and is still a retired false positive.


**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-config/src/settings.rs` — `http_proxy` reads the setting first (`get_str("httpProxy")`) and falls back to `env.http_proxy` second.

**upstream** — pi has no `getHttpProxy()` at all; the interaction between the setting and the ambient env happens in `applyHttpProxySettings` (`core/http-dispatcher.ts:43-46` @v0.83.0), which is `process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy` — the `??=` means an ambient `HTTP_PROXY` WINS and the setting only fills a gap.

**Impact** — inert today: the only production caller passes an empty `EnvVars::default()` (`crates/cyrup-session-svc/src/builder.rs:1462`), which is why the in-source comment says it "mirrors Pi reading the setting value" — true of that call, not of the accessor. The moment any caller passes real `EnvVars` the precedence flips against upstream. Different axis from `PROV-047`'s recorded `??=`/empty-string corner in `node_http_proxy::get_proxy_env`; that one is in the provider resolver, this one is in the settings accessor.

**Fix** — either invert the `or_else` so env wins, or delete the env leg and let the caller compose — and say which in a `[CYRUP-DELTA]` citing `http-dispatcher.ts:43-46`.

**Verify** — a test passing a non-empty `EnvVars` alongside a set `httpProxy` and asserting the ambient value wins.

**Note — a retired false positive, recorded so nobody re-derives it.** `NO_PROXY`, `HTTP_PROXY`, `FTP_PROXY` etc. never appear as string literals on EITHER side: pi's `getProxyEnv(key)` lowercases and uppercases the key at runtime (`packages/ai/src/utils/node-http-proxy.ts:13-23`, consumed at `:38` and `:103-106`) and cyrup does the same (`crates/cyrup-provider/src/utils/node_http_proxy.rs:50-56`). A literal-grep diff reports `NO_PROXY` as missing in cyrup; it is fully handled. The same trap will fire for any future case-folded lookup.

## CFG-061 — `EffectiveSettings::packages()` discards the whole array on one malformed entry — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15 (batch B).** Confirmed at HEAD. `EffectiveSettings::packages()` is now
> `self.merged.packages()` — delegating to the per-entry `Settings::packages_with_errors` the item
> names as the correct live path — and a new `EffectiveSettings::packages_with_errors()` exposes the
> diagnostics so a future caller inherits the error channel rather than the silence. Upstream cite
> **re-derived at v0.83.0**: `getPackages` is `settings-manager.ts:969-971`, not `:953-955` as the
> old in-code comment said (`:953-955` at that tag is `getTrackingId()`); it is
> `[...(this.settings.packages ?? [])]`, a verbatim copy that never parses, which is why a malformed
> entry survives to be rejected individually downstream. RED before:
> `one_malformed_package_entry_does_not_discard_the_other_nine` asserts 9 packages + 1 indexed
> diagnostic from a ten-entry array whose fourth entry is the number `42`; the pre-fix blanket
> `from_value::<Vec<PackageSource>>(v.clone()).ok().unwrap_or_default()` returned zero and no error.


**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-config/src/settings.rs` — `serde_json::from_value::<Vec<PackageSource>>(v.clone()).ok().unwrap_or_default()`.

**upstream** — pi's `getPackages` (`settings-manager.ts:969-971` @v0.83.0) is `[...(this.settings.packages ?? [])]` — a verbatim copy with no parsing, so a malformed entry is carried forward and rejected individually downstream.

**Impact** — cyrup's per-layer `Settings::packages_with_errors` reproduces upstream correctly and is what production actually calls; this `EffectiveSettings` twin has no production caller (`grep -rn '\.packages()' crates/` finds only three assertions inside `settings.rs`'s own tests) and silently returns an EMPTY list — "no packages configured" — when one of ten entries has a typo. Dead but wrong-shaped, which is exactly how a future caller inherits a defect.

**Fix** — delete it, or route it through `packages_with_errors` and surface the errors.

**Verify** — a ten-entry array with one malformed row must yield nine packages and one diagnostic, not zero and silence.

## CFG-062 — Clearing a string/array settings key writes JSON `null`; pi drops the key — **CLOSED 2026-08-15 (write half fixed; the Impact's merge clause REFUTED)**

> **CLOSED 2026-08-15 (batch B). The write half was real and is fixed on BOTH paths; the Impact's
> merge clause is REFUTED and that refutation is the durable finding.**
>
> **Write half — fixed.** `SettingsManager::set` removes the key when `serde_json::to_value(value)`
> is `Value::Null`, instead of inserting it. **The item named only `set`; `set_value_at_path` — the
> shared leaf writer behind `set_nested` AND `persist_nested` — had the identical defect** and now
> removes a `Null` leaf too, because `persistScopedSettings` serializes the nested object through
> the same `JSON.stringify(mergedSettings, null, 2)` (`settings-manager.ts:605` @v0.83.0) that omits
> undefined-valued properties at every depth. RED before:
> `clearing_a_key_removes_it_rather_than_writing_json_null` — pre-fix the written document contained
> `"shellPath": null` and `"terminal": { "showImages": null }`; it also asserts the parent object
> survives an emptied leaf. Still LATENT as filed: `persist_nested`'s two production callers write
> an array (`crates/cyrup/src/subcommands.rs`) and a selector value
> (`crates/cyrup-session-svc/src/session.rs`), neither `Null`.
>
> **Merge half — REFUTED on both of its clauses.** (a) *"cyrup has no such skip"* — pi's
> `overrideValue === undefined` guard (`settings-manager.ts:139-141` @v0.83.0; the same guard at
> `:149-152` of v0.84.1's `deepMergeObjects`) has no Rust counterpart to be missing: `serde_json`
> cannot represent `undefined`, so a key absent from the project map is structurally skipped by
> iterating the map at all. (b) *"a project `npmCommand: null` blanks the global value where pi has
> no way to express that state"* — a hand-written `"npmCommand": null` in a project settings file
> parses to `null`, `overrideValue === undefined` is false, so pi's merge takes the null as well,
> and `getNpmCommand`'s `this.settings.npmCommand ? [...] : undefined` (`:920-922`) then reads it as
> unset. cyrup's `deep_merge` `(_, over) => over.clone()` and `npm_command`'s `as_array` do exactly
> the same thing. Pinned as a NEGATIVE test — `a_project_null_blanks_a_global_value_on_both_sides` —
> so a later pass does not "fix" the merge into a divergence.


**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `packages/coding-agent/src/core/settings-manager.ts:883-887` (`setShellPath(path: string | undefined)`), `:914-918` (`setShellCommandPrefix`), `:924-928` (`setNpmCommand`) @v0.83.0 — each assigns `undefined`, and `JSON.stringify` at `:605` OMITS undefined-valued properties, so "clear" means the key is gone from the file.

**cyrup** — the generic `SettingsManager::set` (`crates/cyrup-config/src/settings.rs`) does `serde_json::to_value(value)` then `doc.obj.insert(key, json)`, so `None::<String>` persists as `"shellPath": null`.

**Impact** — reads coincide (`get_str` on null yields `None`), but the merge does not: a `null` in the GLOBAL file is a present value that cyrup's `deep_merge` lets a project layer override, and in the other direction pi's `deepMergeSettings` skips `undefined` overrides at `:139-141` while cyrup has no such skip — so a project `"npmCommand": null` blanks the global value where pi has no way to express that state at all. **LATENT, not live:** no production caller passes `None` to `set` for any of the three keys.

**Fix** — when the serialized value is `Value::Null`, remove the key instead of inserting it. Do this before any clear-path is added, not after.

**Verify** — set then clear `shellPath`; assert the key is ABSENT from the written document, and that a project `null` does not blank a global value.

## CFG-063 — `PI_TUI_DEBUG` and `PI_DEBUG_REDRAW` have no counterpart, so the cursor/viewport bug class has no instrument

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/tui/src/tui.ts:1577` — `if (process.env.PI_TUI_DEBUG === "1")` dumps firstChanged/viewportTop/cursorRow/height/lineDiff/hardwareCursorRow/renderEnd/finalCursorRow/cursorPos to `/tmp/tui/render-<ts>-<rand>.log` at the end of every synchronized-output frame. `pi/packages/tui/src/tui.ts:1331` — `const debugRedraw = process.env.PI_DEBUG_REDRAW === "1"`; `logRedraw(reason)` appends `fullRender: <reason> (prev=…, new=…, height=…)` to `<logDirectory>/pi-debug.log`.

**cyrup** — neither exists: `grep -rl 'DEBUG_REDRAW\|debug_redraw' crates --include='*.rs'` → 0.

**Impact** — `PI_TUI_DEBUG` records the per-frame decision state and `PI_DEBUG_REDRAW` records WHY a full redraw fired. Together they are the instrument for the cursor/viewport class of TUI bugs — the class the project's own live-render note says `TestBackend` unit tests cannot see. `TUI-040` already files the byte-stream sibling (`PI_TUI_WRITE_LOG`); filing these together is what makes any of the three useful, because a byte stream without the decision state that produced it is not diagnosable.

**Fix** — **FIX SITE: `crates/cyrup-tui` (area 07), alongside `TUI-040`.** Port both under `CYRUP_`-prefixed names, matching the `== "1"` gate and the two output paths.

**Verify** — set each var, drive one frame and one forced full redraw, assert the log files exist and carry the named fields.

## CFG-064 — `isWindowsTerminalSession()` is unported, so Ctrl+Backspace degrades on Windows Terminal — and flips over SSH

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/tui/src/keys.ts:715-718` — `isWindowsTerminalSession()` = `Boolean(process.env.WT_SESSION) && !process.env.SSH_CONNECTION && !process.env.SSH_CLIENT && !process.env.SSH_TTY` (`SSH_CONNECTION` and `SSH_TTY` both at `:717`).

**cyrup** — `grep -rn 'SSH_TTY\|SSH_CLIENT\|SSH_CONNECTION' crates --include='*.rs'` → 0. cyrup reads `WT_SESSION` for terminal capabilities (`crates/cyrup-tui/src/image.rs:640`) but never for this predicate.

**Impact** — the predicate gates two decisions upstream: raw `0x08` → `ctrl+backspace` vs `backspace` (`keys.ts:1287`) and the modifier expected by `matchesRawBackspace` (`keys.ts:733`). Without it Ctrl+Backspace degrades to plain Backspace on Windows Terminal — and because the predicate is a NEGATION of the three SSH vars, the bug direction flips when you ssh INTO or OUT OF a WT session, which is why all three names are filed rather than one representative.

**Fix** — **FIX SITE: `crates/cyrup-tui` (area 07).** Port the predicate verbatim, including all three SSH negations, and route both `keys.ts` decisions through it.

**Verify** — a key-decode test parameterized over `WT_SESSION` set/unset × each SSH var set/unset, asserting the `0x08` mapping flips exactly where upstream's predicate does.

## CFG-065 — `isWslEnvironment()` and its git-HEAD polling fallback are unported

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/coding-agent/src/core/footer-data-provider.ts:84` — `isWslEnvironment()` = `process.platform === "linux" && !!(process.env.WSL_DISTRO_NAME || process.env.WSL_INTEROP)`, consumed by `shouldPollGitHead()` at `:91` together with `isWindowsMountedRepoPath` (`/^\/mnt\/[a-z](?:\/|$)/i`).

**cyrup** — no `isWslEnvironment` and no `WSL_*` read anywhere.

**Impact** — pi POLLS git HEAD instead of relying on filesystem watch events specifically when a repo sits on a 9p `/mnt/<drive>` mount under WSL, because inotify does not fire there. Without it the footer's branch indicator goes stale after a checkout for exactly the users the fallback was added for. `WSL_INTEROP` is filed alongside `WSL_DISTRO_NAME` because WSL2 sets it even when `WSL_DISTRO_NAME` is scrubbed. `12-upstream-drift-pi-core.md:1061` records `footer-data-provider.ts` as read first-hand — this predicate was not carried across.

**Fix** — **FIX SITE: `crates/cyrup-tui` (area 07), in the footer data provider.** Port both halves — the env predicate and the `/mnt/<drive>` path test — and switch the branch source to polling when both hold.

**Verify** — with `WSL_DISTRO_NAME` set and a cwd under `/mnt/c`, assert the footer picks the polling path; assert it does not on a native Linux path.

## CFG-066 — The clipboard backend's two load gates (`TERMUX_VERSION`, `DISPLAY`/`WAYLAND_DISPLAY`) are unported

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/coding-agent/src/utils/clipboard-native.ts:31` — `const clipboard = !process.env.TERMUX_VERSION && hasDisplay ? loadClipboardNative() : null;` and `:16` — `const hasDisplay = process.platform !== "linux" || Boolean(process.env.DISPLAY || process.env.WAYLAND_DISPLAY);`.

**cyrup** — no `TERMUX_*` read anywhere; `grep -rn '"DISPLAY"' crates --include='*.rs'` → 0.

**Impact** — upstream refuses to even LOAD the native clipboard module under Termux (where the prebuilt `.node` cannot load and the require throws at import time) or on headless Linux (CI, ssh without X forwarding, a container). cyrup attempts the backend unconditionally. **Adjacent to but distinct from** the known clipboard-text gap at `12-upstream-drift-pi-core.md:820-828`, which is about READING text; this is about whether the backend is loaded at all. Note `12-upstream-drift-pi-core.md:822` already cites `WAYLAND_DISPLAY` for the v0.84.1 `wl-paste` TEXT-read branch (`clipboard.ts:54`) — a different call site at a different tag; the v0.83.0 `hasDisplay` gate is unfiled.

**Fix** — gate the clipboard backend's construction on the same two predicates before any platform call is attempted.

**Verify** — with `TERMUX_VERSION` set, or on Linux with neither `DISPLAY` nor `WAYLAND_DISPLAY`, assert no clipboard backend is constructed and the caller degrades rather than erroring.

## CFG-067 — Twelve `pi-subagents` env vars have no `CYRUP_` counterpart

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**upstream** (`pi-subagents` v0.10.1 checkout at `/Users/davidmaple/cyrup.ai/pi-subagents`) — each name with the file:line it is declared at:

| var | pi citation | what is lost |
|---|---|---|
| `PI_SUBAGENT_TOOL_TIMEOUT_MS` | `src/runs/shared/tool-timeout.ts:1` | per-tool timeout override for children; cyrup ports `CYRUP_SUBAGENT_TOOL_BUDGET` but not the time dimension (`grep -rn 'TOOL_TIMEOUT_MS' crates --include='*.rs'` → 0) |
| `PI_SUBAGENT_TASK_DELIVERY` | `src/runs/shared/pi-args.ts:76` | selects how the task body reaches the child (argv vs stdin vs file); the delivery mechanism is fixed and unswitchable in cyrup |
| `PI_SUBAGENT_STEER_CAPABILITY` | `src/runs/shared/pi-args.ts:129` | the capability token authorizing a child to be steered; cyrup has `CYRUP_SUBAGENT_STEER_INBOX` and `..._PARENT_CAPABILITY_TOKEN` but not this |
| `PI_SUBAGENT_STEER_ACK_DIR` | `src/runs/shared/pi-args.ts:130` | steer acknowledgement directory; without it a steer is fire-and-forget |
| `PI_SUBAGENT_CAPABILITY_CEILING_V1` | `src/runs/shared/capability-ceiling.ts:5` | versioned ceiling capping what a nested child may inherit |
| `PI_SUBAGENT_RUN_FANOUT_BUDGET` | `src/runs/shared/run-fanout-budget.ts:12` | per-run fanout budget; cyrup has the `CYRUP_SUBAGENT_FANOUT_CHILD` marker but no cap |
| `PI_SUBAGENT_MAX_SPAWNS_PER_RUN` | `src/shared/types.ts:2184`, `src/extension/doctor.ts:192` (`normalizeMaxSubagentSpawnsPerRun(process.env.PI_SUBAGENT_MAX_SPAWNS_PER_RUN)`) | cyrup ports only the PER_SESSION sibling; the per-RUN cap is the tighter one and the one `doctor` reports on |
| `PI_SUBAGENT_TOOL_BUDGET_ZERO_AUTH` | `src/runs/shared/tool-budget.ts:5` | the escape hatch authorizing a zero tool budget; the `budget == 0` case has no gate |
| `PI_SUBAGENT_ASYNC_EVENTS_MAX_BYTES` | `src/runs/background/subagent-runner.ts:281` | cap on the async event stream → no backpressure limit on the NDJSON sink |
| `PI_SUBAGENT_RUNTIME_ACKNOWLEDGED_EXTENSIONS` | `src/runs/shared/runtime-acknowledged-extensions.ts:6` | the acknowledged-extension list passed down at spawn |
| `PI_SUBAGENTS_LLM_INTENT_ARBITER` | `src/runs/shared/llm-intent-arbiter.ts:229` — `if (process.env.PI_SUBAGENTS_LLM_INTENT_ARBITER === "0") return undefined;` (doc at `:20`: "Enabled by default; set …=0 to disable") | the operator kill switch for the LLM intent arbiter |
| `PI_SUBAGENTS_PI_CODING_AGENT_PACKAGE_ROOT` | `src/shared/utils.ts:19` | package-root override used to locate the agent package from a child |

**Impact** — four of the twelve are ceilings or budgets (`CAPABILITY_CEILING_V1`, `RUN_FANOUT_BUDGET`, `MAX_SPAWNS_PER_RUN`, `TOOL_BUDGET_ZERO_AUTH`), so their absence is a missing bound rather than a missing convenience, and one is a security-adjacent off switch. Per the port-mechanism-fidelity rule these are knobs to port literally, not to choose. `09-cyrup-ext-subagents.md` mentions `ASYNC_EVENTS_MAX_BYTES` and the acknowledged-extensions concept in prose, but **no item owns any of the twelve env vars**.

**Fix** — **FIX SITE: `crates/cyrup-ext-subagents` (area 09).** This row exists so the enumeration survives until area 09 files per-var items; close it by reference when it does, do not delete it.

**Verify** — per var: set it, assert the ported behaviour changes, and assert `doctor` reports the two that upstream's doctor reports.

## CFG-068 — `CYRUP_HOME` is invented, live in shipped builds, and outranks `$HOME` at four sites

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:5168` (`dirs_home`), `native_supervisor.rs:1784` (`agent_dir_from`), `background/mod.rs:1408` (`temp_root_dir_from`) and `:2580`.

**upstream** — there is no `PI_HOME` or equivalent anywhere in pi @v0.83.0 or in the three sibling repos (`git -C pi grep -n 'PI_HOME' v0.83.0 -- packages/` → 0).

**Impact** — it takes PRECEDENCE over `$HOME` at all four sites, so setting it relocates the agent dir, the tilde expansion used by `expand_tilde`, and the subagent temp root simultaneously. Its comments describe it as a test sandbox knob, but **none of the four sites is behind `#[cfg(test)]`** — it is live in shipped builds and absent from `cyrup --help`, i.e. supported by accident. Two concrete risks: an operator who sets it for one purpose silently moves three unrelated trees, and its precedence against `CYRUP_AGENT_DIR` is undefined anywhere.

**Fix** — an owner decision, taken deliberately either way: **promote it** (document it, define its precedence against `CYRUP_AGENT_DIR`, list it in `--help`) or **confine it** (`#[cfg(test)]` / a test-only lookup seam).

**Verify** — whichever is chosen, a test asserting the precedence against both `$HOME` and `CYRUP_AGENT_DIR`, plus a `--help` assertion if promoted.

## CFG-069 — `AI_AGENT` is written into every bash and subagent child, and the KEY is a v0.84.1 forward-port — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15.** The filing text below is accurate and was re-derived at both tags. Two things
> it does not say: **one of its three sites was already fixed** (`cyrup-session-svc/src/bash.rs`, batch
> B) — the row was HALF DONE when it was scheduled; and the fix therefore spans **three crates**, not
> the one (`cyrup-tools`) it was routed to. Its `Fix` offered two options and the second was taken:
> recorded as a deliberate forward-port with the TAG stated in the delta line itself. Its `Verify` is
> what landed, one test per site.


**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tools/src/tools/bash.rs:167` (`env.push(("AI_AGENT".to_string(), "cyrup".to_string()))`), `crates/cyrup-session-svc/src/bash.rs:151`, `crates/cyrup-ext-subagents/src/exec/mod.rs:1885`.

**upstream** — `git -C pi grep -n 'AI_AGENT' v0.83.0 -- packages/` returns NOTHING. The var does not exist at the ported tag; `cli.ts:13` @v0.83.0 is a one-line statement setting only `PI_CODING_AGENT`. The three cyrup sites cite `cli.ts:14 @v0.84.1`, which is honest — and which means cyrup writes an env var into every bash and subagent child that the ported baseline never wrote.

**Impact** — the `[CYRUP-DELTA]` annotations at those sites cover the VALUE (`"cyrup"` vs `"pi"`); they do not flag that the KEY itself is a forward-port. This is the same class as the `working-start`/`working-stop` precedent — a real upstream citation attached to a tag the port is not at — and it is exactly how a v0.84.1 uplift later reads as already-done.

**Fix** — either pin it to a v0.84.1 uplift item, or record it as a deliberate forward-port with the TAG stated in the delta line itself rather than only in surrounding prose. Do not remove it silently.

**Verify** — assert the delta line at each of the three sites names `@v0.84.1` and the key, not only the value.

## CFG-070 — Three credential-resolver env names cyrup must read because it does not inherit an SDK

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/api/bedrock_converse_stream.rs:1006` (`AWS_CONFIG_FILE`, with the doc line at `:996-997`: "Honors AWS_SHARED_CREDENTIALS_FILE and AWS_CONFIG_FILE, exactly as the SDK does"), `:1002` (`AWS_SHARED_CREDENTIALS_FILE`), and `crates/cyrup-provider/src/auth/google_adc.rs:223` (`APPDATA`, the Windows ADC well-known path branch in `resolve_source`).

**upstream** — all three are absent from pi's source by NAME. pi delegates Bedrock profile resolution to `@aws-sdk`'s `fromNodeProviderChain`, which reads the AWS pair itself, and `env-api-keys.ts:61` hardcodes only the POSIX ADC well-known path (`_homedir()/.config/gcloud/application_default_credentials.json`), letting `google-auth-library` handle Windows.

**Impact** — **not a divergence in observable behaviour, and the reads are correct as written.** The `APPDATA` branch makes cyrup MORE correct on Windows than the literal pi source. Filed because a name-level parity diff flags all three, and because a later fidelity pass comparing `resolve_source` against `env-api-keys.ts:54-63` will otherwise read the `%APPDATA%\gcloud\…` branch as an unexplained extra. **Do NOT "fix" this by removing the reads.**

**Fix** — none. If anything, extend the existing in-file boundary comment (which already states that role assumption / SSO / IMDS are deliberately not ported) to say that the SDK-inherited env names are reimplemented on purpose.

**Verify** — n/a; this row is a record, not work.

## CFG-071 — `XDG_CACHE_HOME` is a false name-match: both directions, one name — **CLOSED 2026-08-15 for the cyrup half**

> **The cyrup-original half is recorded in the source as of 2026-08-15**; the row still closes only
> when `EXT-027` lands pi's half, exactly as the `Verify` below states. Both citations were re-derived
> this pass and resolve: `huggingface.ts:53` is inside `findHuggingFaceToken` (declared `:46`) and is
> byte-identical at v0.83.0 and v0.84.1. See the Open-items row.


**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-ext/src/build/cache.rs:84` reads it to site the WASM extension build cache.

**upstream** — `pi/packages/coding-agent/src/extensions/llama/huggingface.ts:53` reads it to locate `$XDG_CACHE_HOME/huggingface/token`.

**Impact** — the NAME exists on both sides for unrelated purposes, so a name-only diff scores it as parity while cyrup simultaneously (a) has a cyrup-original use of it and (b) is missing pi's use of it. pi's use belongs to the llama.cpp gap already owned by `EXT-027` (`06-cyrup-ext.md:601`) with `DRIFT-032` (`12-upstream-drift-pi-core.md:712`) as tracker; the cyrup-original use is filed here.

**Fix** — none required for the cyrup side; record the double meaning so neither direction is closed by the other. Note the adjacent grep trap `DRIFT-032` already warns about at `12-…:725`: `HF_TOKEN` appears as a literal in cyrup, but only as a provider-catalog name, never as a token-file search path.

**Verify** — n/a; this row is a record. It closes only when `EXT-027` closes AND the build-cache read is documented as unrelated.

## CFG-072 — `HOMEDRIVE` / `HOMEPATH` widen home resolution past upstream — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15**, taking the `Fix`'s stated better call: kept, with a `[CYRUP-DELTA]` naming
> what it extends. One correction the row could not have known: the branch's own in-code comment
> claimed the pair is libuv's `uv_os_homedir` fallback. **It is not** — libuv checks `USERPROFILE` and
> then makes a syscall (`GetUserProfileDirectoryW`). The delta now makes the weaker, true claim. The
> `Verify` as written is unrunnable from a unix host and needs `unsafe` env mutation anywhere; it was
> landed instead against an extracted pure function. See the Open-items row.


**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tools/src/path.rs:105` reads both as a further fallback.

**upstream** — neither appears anywhere in pi @v0.83.0 (`git -C pi grep -c HOMEDRIVE v0.83.0 -- packages/` → 0). pi uses `process.env.HOME || homedir()` and, on the footer path, `USERPROFILE`.

**Impact** — harmless widening, but it means cyrup resolves a home on Windows configurations where pi resolves none — a behavioural difference in the direction nobody looks, and the kind that makes a "same input, different output" report unreproducible upstream.

**Fix** — keep it and add a `[CYRUP-DELTA]` naming pi's `HOME || homedir()` as what it extends, or drop the pair for strict parity. Keeping it is the better call; stating it is the point.

**Verify** — a path-resolution test with `HOME`/`USERPROFILE` unset and `HOMEDRIVE`+`HOMEPATH` set, asserting the documented outcome.

## CFG-073 — `NO_COLOR` and `CI` are read where pi reads neither

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-ext-subagents/src/watchdog/lsp_diagnostics.rs:903` (`NO_COLOR`, honoured when spawning LSP diagnostics) and `crates/cyrup-ext-subagents/src/exec/acceptance.rs:2702` (`CI`).

**upstream** — neither is read in pi @v0.83.0 product code; pi's only CI-ish read is `GITHUB_ACTIONS` in `packages/agent/vitest.config.ts`, a build config that does not ship.

**Impact** — both are defensible conventions, and `NO_COLOR` especially so. `CI` is the one worth naming: behaviour that changes under CI and not locally is precisely the divergence class that hides, because the environment where it differs is the environment nobody attaches a debugger to.

**Fix** — **FIX SITE: `crates/cyrup-ext-subagents` (area 09).** Add a `[CYRUP-DELTA]` at each site stating that upstream reads neither and what the read changes.

**Verify** — assert the acceptance path's behaviour under `CI=1` is stated in the delta line and covered by one test.

## CFG-074 — Nine invented env vars across the three sibling ports

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup**, each with the upstream fact that makes it an invention:

| var | cyrup citation | upstream |
|---|---|---|
| `CYRUP_PERMISSION_SYSTEM` | `crates/cyrup-permission-system/src/extension.rs:183` — `pub const INSTALL_ENV_VAR` ("the explicit opt-in flag (DI-5)") | `pi-permission-system` reads exactly two env vars — `PI_PERMISSION_SYSTEM_CONFIG_PATH` (`src/permission-manager.ts`) and `PI_PERMISSION_SYSTEM_LOGS_DIR`. There is no install/opt-in flag; the extension installs on the presence of a policy file. |
| `CYRUP_PERMISSION_SYSTEM_FORWARDING_AGENT_DIR` | `crates/cyrup-permission-system/src/forwarding.rs:89` | `pi-permission-system/src/permission-forwarding.ts:6-8` declares only numeric constants (`POLL_INTERVAL_MS`, `WATCH_DEBOUNCE_MS`, `TIMEOUT_MS`) — all compile-time, none env-overridable. |
| `CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS` | `crates/cyrup-permission-system/src/forwarding.rs:63` — `CHILD_WAIT_TIMEOUT_ENV` | upstream's `PERMISSION_FORWARDING_TIMEOUT_MS = 10 * 60 * 1000` (`src/permission-forwarding.ts:8`) is a fixed constant. |
| `CYRUP_INTERCOM_TRANSPORT` | `crates/cyrup-intercom/src/transport/target.rs:36` | `pi-intercom` has no transport-selection var; its full set is `PI_BIN`, `PI_INTERCOM_PI_BIN`, `PI_INTERCOM_ASK_TIMEOUT_MS`, `PI_INTERCOM_LIVENESS_INTERVAL_MS`, `PI_INTERCOM_LIVENESS_TIMEOUT_MS`, `PI_INTERCOM_NAME_POLL_MS`, `PI_INTERCOM_SESSION_ID`, `PI_INTERCOM_STABLE_ID`, plus the `PI_SUBAGENT_*` it reads and `HERDR_BIN`. |
| `CYRUP_INTERCOM_TCP` | `crates/cyrup-intercom/src/transport/target.rs:39` | same — the TCP variant selector has no counterpart. |
| `CYRUP_INTERCOM_BROKER_BINARY` | `crates/cyrup-intercom/src/transport/spawn.rs:42`, overriding the `current_exe()` re-exec at `:140` | the closest analogue is `PI_INTERCOM_PI_BIN` / `PI_BIN` (`pi-intercom/project-agent.ts:245`), but those name the PI binary to launch in a Herdr pane, not the broker to spawn — a different object. |
| `CYRUP_SUBAGENT_AGENT_NAME` | `crates/cyrup-ext-subagents/src/exec/mod.rs:1319` — `AGENT_NAME_ENV_VAR`, set by the spawn overlay in `build_attempt_spawn_plan` | no `PI_SUBAGENT_AGENT_NAME` in `pi-subagents` (checked against the 48-name literal set extracted from `pi-subagents/src`). |
| `CYRUP_HOOK_WARMUP` | `crates/cyrup-ext-subagents/src/spawn/worktree.rs:1312` — `HOOK_WARMUP_ENV` | no counterpart in `pi-subagents`' `worktree.ts`. |
| `CYRUP_SUBAGENTS_TEMP_ROOT` | `crates/cyrup-ext-subagents/src/background/mod.rs`, alongside `temp_root_dir_from` at `:1408` | no `PI_SUBAGENTS_TEMP_ROOT` upstream. Pairs with the documented `[CYRUP-DELTA]` at `background/mod.rs:1418-1423`, where cyrup interposes a cwd key that pi's flat `ASYNC_DIR`/`RESULTS_DIR` do not have — so the root that key hangs off is itself an invention, and the delta comment covers the layout but not the var. |

**Impact** — each is individually defensible; several follow structurally from the port mechanism (a re-exec'd Rust broker instead of a Node module has to be able to name the broker). The problem is that none of them is KNOWN: an extension, hook or operator written against pi's contract sees names in the child environment that upstream children never carry, and `CYRUP_PERMISSION_SYSTEM` in particular is an invented control over whether a SECURITY gate is installed — the one category worth naming explicitly. **Note for the record:** `PI_PERMISSION_SYSTEM_POLICY_AGENT_DIR`, which cyrup carries as a legacy alias, is NOT fabricated — it is real at `pi-permission-system/src/permission-manager.ts:29` (bracket-form access, which a `process.env.X` dot-grep misses).

**Fix** — one `[CYRUP-DELTA]` per var naming the upstream file:line it replaces or the mechanism that forced it, and a decision on `CYRUP_PERMISSION_SYSTEM` specifically: does presence of a policy file install the gate (upstream's rule), with the flag as an override, or is the flag load-bearing? **FIX SITES: areas 09, 10 and 11.**

**Verify** — a test per crate asserting the documented install/transport precedence, and a doc assertion that each const carries a delta line.

## CFG-075 — `CYRUP_EXT_ABI_FINGERPRINT` is the surface's only build-time env dependency — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15.** The `Fix` is done as written — the supplying build script is documented next
> to the `env!`, by the exact `cargo:rustc-env` directive it emits. One fact added that the row did not
> state and that was verified rather than assumed: **neither cargo feature arm removes the
> dependency** — `build/` is compiled through `lib.rs`'s bare `pub mod build;`, so a
> `--no-default-features` build needs the value too. See the Open-items row.


**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-ext/src/build/mod.rs:21` — `pub const ABI_FINGERPRINT: &str = env!("CYRUP_EXT_ABI_FINGERPRINT");`.

**upstream** — none possible: pi has no WASM component ABI to fingerprint.

**Impact** — it is a compile-time `env!`, not a runtime `env::var`, so a missing value is a **compile error**, not a runtime fallback. That is worth knowing before anyone reorganizes the build scripts, and it is why this one is filed separately from `CFG-074` rather than folded into it.

**Fix** — **FIX SITE: `crates/cyrup-ext` (area 06).** No behavioural work implied; ensure the build script that supplies it is documented next to the `env!` so the dependency is discoverable from the consumer.

**Verify** — n/a beyond a build-script comment; the compiler already enforces it.

## CFG-076 — Three `PI_`→`CYRUP_` rename exceptions, one of them a live inconsistency inside cyrup

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi-subagents/src/runs/shared/pi-spawn.ts:6` — `export const PI_SUBAGENT_PI_BINARY_ENV = "PI_SUBAGENT_PI_BINARY"`; and `pi/packages/coding-agent/src/config.ts:515-521` (`getAgentDir()`) with `main.ts:625-628` (the session-dir env tier).

**cyrup** — three departures from the mechanical `PI_` → `CYRUP_` substitution that the whole env diff relies on:

1. **`PI_SUBAGENT_PI_BINARY` → `CYRUP_SUBAGENT_BINARY`.** The upstream name carries TWO `PI` tokens (the prefix and the `PI_BINARY` noun) and cyrup collapses them, giving `CYRUP_SUBAGENT_BINARY` rather than `CYRUP_SUBAGENT_CYRUP_BINARY`. Sensible; recorded because the mechanical diff scores it as a MISSING upstream var and anyone re-running the enumeration hits the same false positive. Sites: `crates/cyrup-ext-subagents/src/registration/doctor.rs:311`, `:338`, `background/runner_main.rs:3793`.
2. **`PI_CODING_AGENT_DIR` / `PI_CODING_AGENT_SESSION_DIR` → `CYRUP_AGENT_DIR` / `CYRUP_SESSION_DIR`.** cyrup shortens the noun as well as swapping the prefix. Both original spellings are retained as lower-precedence fallbacks (`crates/cyrup-config/src/env.rs:81-82`, `first(&["CYRUP_AGENT_DIR", "PI_CODING_AGENT_DIR"])`), so nothing breaks for a migrating user. Worth stating because the surrounding twelve aliases in that file DO preserve the noun exactly (`CYRUP_CACHE_RETENTION` ← `PI_CACHE_RETENTION`, `CYRUP_CLEAR_ON_SHRINK` ← `PI_CLEAR_ON_SHRINK`, …), making these two the odd pair.
3. **The real defect: `CYRUP_CODING_AGENT_DIR` also exists**, at `crates/cyrup-ext-subagents/src/native_supervisor.rs:1772`. The long form is live in one crate and the short form in another, so the same concept has two `CYRUP_` names — an inconsistency internal to cyrup, independent of pi.

**Impact** — items 1 and 2 cost nothing but re-derivation; item 3 means an operator who sets `CYRUP_AGENT_DIR` does not necessarily move the subagent supervisor's notion of the agent dir, and neither name is documented in `--help`.

**Fix** — route `native_supervisor.rs:1772` through the same `first(&[…])` alias list `cyrup-config` uses, so one name wins everywhere and the `PI_` fallbacks stay honoured. Leave items 1 and 2 alone; this row is their record.

**Verify** — a test asserting `CYRUP_AGENT_DIR` is honoured by the subagent supervisor, and that the `PI_CODING_AGENT_DIR` fallback still resolves when the short form is unset.

## CFG-077 — Prompt roots are scanned recursively and named by root-relative path, where pi scans one level and names by basename

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/coding-agent/src/core/prompt-templates.ts:135-137` @v0.83.0 is the whole of it, and it is the docstring itself: *"Scan a directory for .md files **(non-recursive)** and load them as prompt templates."* `loadTemplatesFromDir` (`:138`) reads one `readdirSync` level and stops; the name is derived in `loadTemplateFromFile` (`:104`) as `basename(filePath).replace(/\.md$/, "")` (`:109`). A subdirectory under a prompt root contributes **nothing** upstream, and two files with the same basename in different subdirectories are indistinguishable.

**cyrup** — `crates/cyrup-resources/src/discovery.rs:1764` `scan_prompt_root` hands off to the recursive `scan_prompt_dir` (`:1780`), and the divergence is declared where it happens: the `[CYRUP-DELTA]` at `:1758-1763` names pi's non-recursive `prompt-templates.ts:136-174` and cites code-puppy's `_is_in_skipped_namespace` (`customizable_commands/register_callbacks.py`) plus the skills walker's `node_modules` carve-out as the model for the skip rules. The rules, all read at HEAD: children are sorted per directory so first-wins tie-breaking is deterministic (`:1793`); a directory named with a leading `.` or `_`, or `node_modules`, is not descended into (`:1810`); directory symlinks are never followed, which makes the walk cycle-proof by construction, while a FILE symlink whose target is a regular `.md` still loads (`:1802-1807`, pi's own symlink handling); and nesting past `MAX_PROMPT_NAMESPACE_DEPTH = 8` (`:1756`) is refused with a `namespace depth exceeds 8` warning per refused dir rather than silently (`:1813-1819`). Name derivation moved with it: `PromptTemplate::load_with_root` (`prompt.rs:60`) joins the root-relative components with `/`, strips `.md` and **preserves case** (the field doc at `prompt.rs:33-35` carries its own `[CYRUP-DELTA]` against `prompt-templates.ts:108`), while the single-file `load` (`:129`) relativizes against the file's own parent (`:134-135`) and therefore reproduces pi's basename behaviour exactly for every caller that has no meaningful root. Applied at all four directory-scan call sites: the global `<global_dir>/prompts` root (`discovery.rs:857`), each package-manifest prompt dir (`:1102`, inside `for pdir in &manifest.prompts` at `:1086`), the project `.cyrup/prompts` root (`:1296`), and `add_prompt_path`'s directory arm (`:1945`, in `add_prompt_path` at `:1929`) — which is the one extensions reach.

**Impact** — **maximally reachable, and load-bearing for a shipped surface.** It changes the name of every prompt template on every session start for every user, and `crates/cyrup-flux` depends on it outright: flux contributes its prompts as a **directory** rather than as files precisely so the namespace survives, and the in-source comment at `crates/cyrup-flux/src/extension.rs:128-131` states the dependency verbatim — *"This one line is why `/flux/new` is `/flux/new` and not `/new`."* **Under pi's rule the fifteen `/flux/*` commands do not flatten — they VANISH**: `bundled_prompts_dir()` (`crates/cyrup-flux/src/resources.rs:27-29`) contributes `<resources>/prompts`, whose only child is the directory `flux/`, so a non-recursive `readdirSync` of it finds no `.md` at all and registers nothing. The five `_docs/*.md` files one level deeper are the skip rule earning its keep in the same tree: without the `_`-prefix refusal cyrup would register `/flux/_docs/README`, `/flux/_docs/pipeline`, `/flux/_docs/synopsis`, `/flux/_docs/about` and `/flux/_docs/cheatsheet` as commands. Left untracked, the failure mode is not a missed defect but an *inflicted* one: a surface sweep reading `prompt-templates.ts:136` sees "(non-recursive)" in the docstring, files the recursion as drift, and removes it.

**Fix** — none. This row IS the fix; it exists so the divergence is a decision of record rather than an unexplained branch, which is the same job `CFG-070` does for the credential-resolver env names. **One real residual, and it is documentation-only:** six source sites cite `spec/namespaced-prompt-templates.md` as the governing spec (`discovery.rs:1753`, `:1759`; `prompt.rs:15`, `:34`, `:57`; `tests/resources.rs:4022`) and **that file is not in `spec/`** — `spec/flux.md:62-63` already notes the absence in passing. Either write it or re-point the six citations at this row; a `[CYRUP-DELTA]` whose authority is a file nobody can read is the shape `docs/gap-analysis/README.md:719-721` says to treat as an unverifiable claim rather than a decision of record.

**Verify** — already pinned, and no test-coverage sub-gap should be filed against this: nine `npt_*` cases in `crates/cyrup-resources/src/tests/resources.rs` cover name derivation and expansion (`:4033`), the dir-names-only skip rules (`:4089`), the depth cap warning once per refused dir (`:4123`), symlink policy (`:4175`), `load_with_root` derivation edges (`:4225`) and its non-UTF-8 component error (`:4293`), load-error-becomes-warning (`:4320`), precedence shadowing and case collision (`:4352`), and — the one that guards the four call sites this row enumerates — `npt_all_directory_and_single_file_call_sites` (`:4375`).

## Coverage

**Read first-hand at cyrup HEAD `04c1ba2`** (tree clean at `a9000b1`, docs-only): `crates/cyrup-config/src/{settings,auth,trust,model,models_store,provider_compose,config_value,env,lib}.rs` (`models_store.rs` and `env.rs` in full), plus `login.rs`'s module doc, full public-fn index and the `provider_auth_status` / `login_provider_options` bodies; `crates/cyrup-resources/src/{discovery,skill,prompt,error}.rs` (discovery's settings / package / project / skill-scan regions) and `src/package/{manifest,source,install,store}.rs`; `crates/cyrup/src/{main,migrations,cli}.rs` (launch predicate, migrations call, keybindings load, dir flags, prompt-input region); `crates/cyrup-session-svc/src/{builder,session}.rs` (packages / retry / prompt regions; auth predicate + `full_model_registry`); `crates/cyrup-test-support/src/auth.rs`; `crates/cyrup-tui/src/{keymap,auth_select,commands,app,theme}.rs` (keymap's `merge_json` + `parse_key_values`; app's keybindings, login-selector and `/settings` regions); tests `crates/cyrup/src/tests/models_json_resolution.rs`, `crates/cyrup-session-svc/src/tests/read_image_auto_resize.rs`, `crates/cyrup-tui/src/tests/settings_inert_keys.rs`, `crates/cyrup-ext/src/tests/extension_name_conflicts.rs`.

**Read first-hand upstream, at explicit tags** (`git show v0.83.0:<path>` / `git show v0.84.1:<path>`, never the floating HEAD unless stated): `coding-agent/src/core/{settings-manager,model-resolver,model-runtime,models-store,model-config,model-registry,provider-composer,resource-loader,package-manager,pi-manifest,project-trust,trust-manager,source-info,experimental,defaults,resolve-config-value,prompt-templates,skills,slash-commands,keybindings,auth-storage}.ts`, `coding-agent/src/{config,migrations,main,package-manager-cli}.ts`, `coding-agent/src/utils/paths.ts`, `coding-agent/src/modes/interactive/{interactive-mode.ts,components/oauth-selector.ts}`, `packages/tui/src/keybindings.ts`, `packages/ai/src/auth/oauth/anthropic.ts`, `packages/ai/src/providers/{cloudflare-auth,google-vertex}.ts`.

**Version-lag sweep** run as `git diff v0.83.0..v0.84.1` scoped to every area-05 path: nine files moved, 650+/201−. Everything it produced is either filed (CFG-039 `samplingParams`, CFG-040 `markdown.mermaid`, CFG-041 `defaultModelPerProvider`, CFG-042 models-store) or folded into a re-audit (CFG-012 superseded; CFG-021 corrected; CFG-034's kind corrected). Drift deliberately NOT filed here because another item or area owns it: `AGENTS.override.md` (`resource-loader.ts:71`) is already ported at `crates/cyrup-session/src/prompt/context_files.rs:81` (area 03); `chatTemplateArgs` + the `baseten` `thinkingFormat` literal (`model-config.ts:87`, `:98`) are the defect PARITY-GAPS already records against `crates/cyrup-provider/src/api/compat.rs`; `CredentialSynchronizationError` / `enqueueCredentialOperation` / deferred responses / `AuthOperationOptions` cancellation are area-01 PARITY-GAPS items interlocking with CFG-020; the git-update `.pi-update-incomplete` marker and `repairMissingGitDependencies` (`package-manager.ts:1854-1902`) are npm/node_modules machinery downstream of the dropped npm channel (CFG-009).

**Surface-driven sweep method** (the counter to structural blind spot 1). Walked pi's Settings interface key by key — `settings-manager.ts:88-140` @v0.84.1, 47 keys — against `grep -n 'merged.get' crates/cyrup-config/src/settings.rs` and then against consumers OUTSIDE `cyrup-config`; that re-confirmed CFG-014 / CFG-015 open and CFG-S04 closed, and it is what caught `collapseChangelog` (folded into CFG-015) and `doubleEscapeAction` (CFG-045) — the latter had a consumer, but only a `/settings` display row, which is the shape the previous sweep's "has a consumer" test let through. **Record that refinement: a `/settings` row is not a consumer.** Then walked every exported symbol of `resource-loader.ts`, `package-manager.ts`, `trust-manager.ts`, `project-trust.ts`, `skills.ts`, `prompt-templates.ts`, `resolve-config-value.ts`, `keybindings.ts`, `config.ts`, `source-info.ts`, `experimental.ts`, `defaults.ts`, `slash-commands.ts` and `pi-manifest.ts` asking "what in `crates/` consumes this?". That produced CFG-035 (`discoverSystemPromptFile` had no cyrup counterpart at all), CFG-036 (`expandTildePath` on the env/CLI dir tiers), CFG-037 (`ensureGitIgnore`), CFG-038 (`toKeybindingsConfig`'s drop-one semantics), CFG-044 (three cites resolving to nothing upstream) and CFG-047 (`BUILTIN_SLASH_COMMANDS` metadata).

**Migrations + keybindings surface sweep (repair pass 2026-08-12).** Added because the critique found that `pi/packages/coding-agent/src/migrations.ts` was named exactly once in the entire fifteen-file directory, as an incidental `{mode: 0o600}` citation in this file. Axis: **enumerate every call `runMigrations` makes and pair each with its cyrup counterpart, then follow the result to its consumer.** Both upstream files are byte-identical at v0.83.0 and v0.84.1 (`git diff v0.83.0 v0.84.1 -- packages/coding-agent/src/migrations.ts packages/coding-agent/src/core/keybindings.ts` → empty), so every line number below holds at either tag. Six upstream behaviours, six pairings:

| # | pi @v0.83.0 | cyrup @`04c1ba2` | verdict |
|---|---|---|---|
| 1 | `migrateAuthToAuthJson()` `:309` | `migrate_auth_to_auth_json` `migrations.rs:27`/`:39` | ported (CFG-032 closed) |
| 2 | `migrateSessionsFromAgentRoot()` `:310` | `migrate_sessions_from_agent_root` `:28`/`:114` | ported; one edge divergence → area 03 |
| 3 | `migrateToolsToBin()` `:311` | `migrate_tools_to_bin` `:29`/`:177` | ported **minus its completion notice** → **CFG-050** |
| 4 | `migrateKeybindingsConfigFile()` `:312` | **nothing** | **not ported** → **CFG-048** |
| 5 | `migrateExtensionSystem(cwd)` `:313` | `migrate_extension_system` `:30`/`:201` | ported |
| 5a | `migrateCommandsToPrompts` `:137-155` | `migrate_commands_to_prompts` `:212` | ported, notice included |
| 5b | `checkDeprecatedExtensionDirs` `:222-255` | `check_deprecated_extension_dirs` `:236` | ported |
| — | `showDeprecationWarnings` `:277-296` | `format_deprecation_warnings` `:263` | text ported, **keypress gate dropped** → **CFG-049** |

Verified faithful arm-for-arm and **deliberately not filed**: (1)'s skip-if-`auth.json`-exists, the `oauth.json` → `{type:"oauth",…}` wrap, the `oauth.json.migrated` rename, the `settings.json.apiKeys` lift skipping oauth-claimed providers, and the 0600 write; (2)'s non-recursive `*.jsonl` scan, the first-line `{type:"session", cwd}` gate, skip-if-target-exists and the swallow-everything rename; (3)'s four managed names and stale-source delete; (5a)'s Global/Project double run and both message strings routed through `output_guard::emit_stray_line`; (5b)'s always-warn `hooks/` rule and the `tools/`-holds-a-non-managed-entry rule with its leading-dot and case handling; and `settings.rs:362-421` `migrate_settings` against `settings-manager.ts:381-440` (queueMode→steeringMode, websockets→transport, the legacy `skills` **object** with arrays correctly excluded, `retry.maxDelayMs`→`retry.provider.maxRetryDelayMs`, applied in-memory on every parse and never written back — matching pi).

**Rejected with reason — do not re-derive.** Nothing filed by the auditor was refuted outright this pass, but four re-audits were **corrected against the auditor** and those corrections are the record: CFG-028's medium rating was rejected (pi's `execSync` blocks its single event loop for the same 10 s; cyrup blocks one worker of N and is therefore *less* blocking than upstream — robustness note, not a parity gap); CFG-030's medium was rejected (both sides mangle a non-object top level; pi's "preservation" is meaningless indexed keys, and load behaviour is identical); CFG-034's `not-ported` kind and its v0.83.0 upstream cite were rejected (`git grep scrollbarThumb v0.83.0 -- packages` is empty — the cited lines are v0.84.1's, so cyrup anticipated an addition rather than closing a parity gap); CFG-004's cyrup cite `discovery.rs:1242-1246` was rejected (that is the SKILL.md walk; the extension push is `add_local_entries` at `:1373-1379`). One `missedByAuditor` entry was deliberately NOT given its own id: `collapseChangelog` is folded into CFG-015 so the whole changelog feature — `lastChangelogVersion`, `collapseChangelog`, the parser, `/changelog` — closes in one changeset instead of four.

**Checked and deliberately not filed — verified faithful.** `trust.rs`'s `TRUST_REQUIRING_PROJECT_CONFIG_RESOURCES` list, the ancestor `.agents/skills` walk with its `$HOME` exclusion, the 4–6 option `trust_options`, `read_map`'s hard errors on a non-object `trust.json` and on a non-bool value, and the `decide_trust_with_extension` hook ordering (`trust.rs:196-424` vs `trust-manager.ts:29-244` + `project-trust.ts:46-95`); `migrate_settings`' four legacy shapes (`settings.rs:357-420` vs `settings-manager.ts:389-448`); `set`/`set_nested`'s locked read-modify-write with nested-sibling preservation (`settings.rs:1349-1420` vs `persistScopedSettings` `:586-616`); the whole of `config_value.rs` (parse / template / command / cache / shell-selection) against `resolve-config-value.ts`; skill frontmatter validation and the ignore-aware SKILL.md walk (`skill.rs:15-95` + `discovery.rs:1191-1280` vs `skills.ts:91-270`); the `${@:N}` / `${@:N:L}` slice family and `$0` (`prompt.rs:256-310` vs `prompt-templates.ts:74-96`); theme dir precedence; `status_indicator_runs` (`auth_select.rs:171-199`), a faithful 4-state port of `formatStatusIndicator` (`oauth-selector.ts:164-181`) — the 3-state `AuthState` / `provider_rows` beside it is legacy scaffolding, not the render path, so nothing was filed against it. `InMemoryCredentialStore::modify`'s `Ok(None)` early return was chased against pi's `post?.type !== "oauth"` guard and matches.

**Handoffs (repair pass).**

- **`encode_cwd` belongs to area 03, and a duplicate copy lives here.** The sweep found that `crates/cyrup/src/migrations.rs:160-173` and `crates/cyrup-session/src/layout.rs:97-105` both do `trim_start_matches(['/', '\\'])`, stripping **all** leading separators, while pi's `migrations.ts:112` and `session-manager.ts:479` both use `/^[/\\]/` with no `g` flag — exactly **one**. For `\\server\share\proj` pi yields `---srv-share-proj--` and cyrup yields `--srv-share-proj--`. Both cyrup copies agree with each other, so no cyrup-written session is lost by cyrup; the costs are cross-tool session-tree interop under UNC/double-slash cwds, and a doc comment at `layout.rs:95` claiming "Pi-compatible encoding: strip a leading separator" that is untrue. **No CFG id is filed for it**: the defect is one behaviour with two copies, `layout.rs` is the primary and belongs to area 03, and filing a second id here would double-count. The fix must delete the `migrations.rs` copy in favour of `cyrup_session::encode_cwd` — the duplication is what let two drift-free copies both be wrong — and area 03's item should say so.
- **CFG-048 ships with TUI-051 and must precede TUI-028.** `/reload` is pi's second application site for the keybinding name migration (`keybindings.ts:366` via `loadFromFile`, driven from `interactive-mode.ts:5386`), so the config half and the reload half are one behaviour split across two areas. And TUI-028's namespace rename will break every `editor.*` config users have written against shipped cyrup unless CFG-048's table lands first with the `editor.* → tui.editor.*` rows added in the same change.
- **CFG-049 and CFG-051 are both launch-glue ordering in `crates/cyrup/src/main.rs:520-533`** — one blocks, one relocates into the transcript. They touch adjacent lines and should land together.

**Handoffs.** PARITY-GAPS PB-6 (`lastChangelogVersion` / `/changelog`) is folded into CFG-015 rather than duplicated. CFG-005 is confirmed partially closed and left UNSCHEDULED per the maintainer's deprioritisation; its Copilot/Codex half belongs to PROV-029 (area 01). CFG-039 is half of a two-part gap whose other half is PARITY-GAPS' provider-layer `samplingParams` item (area 01). CFG-047's `/reload` clause may be a behaviour gap owned by the reload path (area 03/07), not a string fix.

**Blind spots for the next pass.**

1. **Static only; nothing compiled or run.** No closure here is observed-passing — every `closed` verdict is a two-sided code read. CFG-035's claim that no code path reads `SYSTEM.md` rests on an exhaustive `grep -rn 'SYSTEM\.md' crates/` returning five hits that are all comments, markers or test inputs, plus a `format!("{…}.md"` sweep for a dynamically-constructed filename. None was found, but absence cannot be proved by grep.
2. **`login.rs` was NOT audited for correctness.** 1721 lines landed since the last pass; the module doc, the public-fn index and two bodies were read. The remaining ~1200 lines — `login`, `logout`, `resolve_login_command`, `start_provider_login`, `resolve_auth_type_selector`, `ProviderCredentialSink` — were not read against upstream. This is exactly the shape that produced PROV-027/028/029. The same caveat applies to the code that closed CFG-002, CFG-010, CFG-011, CFG-022 and CFG-024: the specific defect each item named is verified gone, not that the new code is correct in the large. **`crates/cyrup-config/src/login.rs` is the single highest-value unread surface in this area.**
3. **Five in-scope files were only grepped, not read end to end:** `crates/cyrup-resources/src/{theme.rs (828 lines), scope.rs, key.rs}`, `crates/cyrup-config/src/{policy.rs, env_keys.rs}`, and `crates/cyrup-resources/src/package/git_url.rs` (985 lines). `git_url.rs` carries the `hasUnsafeGitInstallPart` security validator, confirmed referenced from `source.rs:75-77` but never compared line-by-line against pi's `parseGitUrl` (`coding-agent/src/utils/git.ts`). A validator gap there would be a security finding this pass could not have seen.
4. **Upstream `package-manager.ts` is ~2500 lines and roughly a third was read.** Read: `resolve`, `resolvePackageSources`, `collectPackageResources`, `dedupePackages`/`getPackageIdentity`, `installGit`/`updateGit`/`ensureGitRef`, `ensureGitIgnore`/`ensureNpmProject`, `checkForAvailableUpdates`, the install-path resolvers, and — new this pass, closing the previous pass's blind spot 4 — `applyAutoloadDisabledPatterns` against cyrup's `manifest.rs:401-454`. Still NOT read: `applyPatterns`, `collectAutoThemeEntries`, `collectAncestorAgentsSkillDirs`, `resolveExtensionSources`, `getTemporaryDir`, and the whole progress-callback surface.
5. **The version-lag window stops at v0.84.1.** pi HEAD is `581d75a89` = v0.84.1-117-g581d75a89, so 117 commits over these paths are unanalysed. One concrete item was hit and deliberately NOT filed for that reason: `getExperimentalToolSampling()` in `core/experimental.ts`, which makes `PI_EXPERIMENTAL=1` request `{type:"json_schema", strict:"prefer"}` constrained sampling on the four built-in tools (`read.ts:222`, `bash.ts:337`, `edit.ts:311`, `write.ts:200` at pi HEAD). It is absent at BOTH v0.83.0 and v0.84.1 — `git show v0.84.1:…/experimental.ts` is three lines — so it is post-tag drift outside the swept window. cyrup has no `constrained_sampling` anywhere (only a note at `crates/cyrup-provider/src/api/bedrock_converse_stream.rs:44`). **File it when the window moves.**
6. **Two closures are verified structurally, not end to end.** CFG-006's retry settings are confirmed assigned onto the agent builder (`builder.rs:1223-1234`) but not confirmed to reach the retry loop — `max_retry_delay_ms` in particular. CFG-022's shared predicate is verified identical at both call sites, but it was not confirmed that a models.json-only provider actually STREAMS (the same caveat the previous pass recorded).
7. **Area boundaries not crossed:** `remote_catalog.rs` / `spawn_model_catalog_refresh` (area 01); the alt-screen / TUI-mode renderer that CFG-021 and CFG-040 both wait on (area 07); pi's keybinding CONFLICT detector (`KeybindingConflict`, `packages/tui/src/keybindings.ts:235-256`), which has no cyrup counterpart but lives in the TUI substrate rather than `core/keybindings.ts` — worth a look from area 07 alongside CFG-038.
8. **The `spec/` tree behind the `R-07-*` / `R-09-*` ids is still absent** from this workspace, so those ids were used only as a grep index; no requirement text was quoted or checked.
9. **NEW (repair pass) — the launch-glue path in `crates/cyrup/src/main.rs` has never had its own sweep.** CFG-049 and CFG-051 were both found by following one upstream function (`showDeprecationWarnings`) to its cyrup call site and noticing what happened *around* it. `main.rs` is the place where migration results, settings diagnostics, model-fallback notices, the first-run gate and the trust prompt all compete for the same few pre-TUI stderr lines, and pi routes several of them into the running UI instead. Nobody has enumerated pi's `main.ts:600-860` startup block against `main.rs`'s statement by statement. Two of two items found there in a partial look is a high enough base rate to run it properly.
10. **NEW (repair pass) — in-source "intentionally NOT ported" comments were never enumerated as a class.** `migrations.rs:9-10` was one, and it was wrong on both halves of its claim. `grep -rn 'NOT ported\|not ported\|intentionally\|CYRUP-DELTA' crates/cyrup-config crates/cyrup-resources crates/cyrup` would produce the full list, and README:208-212 says none of them is a decision of record. Each is either a gap or a documented mechanism difference, and this file currently adjudicates them only where an item happened to land on one.

---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`) — ALL FOUR CLOSED 2026-08-12

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at all, rather
than checking a list of known items. That inversion exists because the item-driven method missed pi's
stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`) — a real, user-reported bug — and by
construction cannot see behaviour nobody wrote an item for. IDs use an `-SNN` suffix to mark their
provenance.

**All four closed at HEAD `04c1ba2`.** The section and its ids are retained so each closure can be
re-audited; the evidence is in the status table above. One follow-on: CFG-S04's sweep tested "does this
key have a consumer outside `cyrup-config`?", which passes for a key whose only consumer is a
`/settings` display row — `doubleEscapeAction` escaped exactly that way and is now **CFG-045**, and
`collapseChangelog` the same way and is folded into **CFG-015**.

| ID | Severity | Kind | Effort | Status | Title |
|---|---|---|---|---|---|
| CFG-S01 | high | not-ported | S | **closed** | `--system-prompt` / `--append-system-prompt` never read file contents — a path becomes the literal system prompt |
| CFG-S02 | medium | not-ported | S | **closed** | `images.autoResize: false` is inert — the read tool always downsamples to 2000px |
| CFG-S03 | medium | not-ported | M | **closed** | Extension tool-name and flag-name conflicts are never detected, and precedence on collision is inverted |
| CFG-S04 | low | not-ported | M | **closed** | Four more settings keys inert beyond CFG-015's list — `enableSkillCommands`, `treeFilterMode`, `editorPaddingX`, `showHardwareCursor` |
