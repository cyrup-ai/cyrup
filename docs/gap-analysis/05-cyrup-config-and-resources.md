# 05 — cyrup-config + cyrup-resources

Covers `cyrup/crates/cyrup-config` (settings, auth store, trust, model resolution, config values) and `cyrup/crates/cyrup-resources` (packages, discovery, skills/prompts/themes), plus the launch-path glue in `cyrup/crates/cyrup/src/main.rs`, `migrations.rs` and `cyrup-session-svc/src/builder.rs` that consumes them. Measured against `pi/packages/coding-agent/src/core/{settings-manager,model-resolver,model-runtime,auth-storage,trust-manager,package-manager,provider-composer,prompt-templates}.ts` and `modes/interactive/theme/theme.ts` at pi v0.83.0. Headline: the 28-commit window closed the settings write-refusal latch outright and landed the read/compose halves of models.json, packages and settings-declared paths — but every remaining launch-path defect is an *auth-predicate* problem (`has_configured_auth` is implemented twice and the two copies disagree), and two previously-unfiled high-severity defects sit outside the refresh window entirely (`shellPath` tilde expansion, migration file mode). Re-baselined against HEAD `1806375` on 2026-08-03; read-only, nothing compiled.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| CFG-001 | **closed** | `record_load_error` latches per scope from both error arms of `load_scope`; all four writers call `ensure_scope_writable` and re-check inside `with_lock`. No bypass: the `SettingsStore` trait exposes no write method and `with_lock` has zero callers outside `settings.rs`. Two scope notes, neither a downgrade: the non-object top-level hole is CFG-030, and `crates/cyrup/src/migrations.rs:91` writes `settings.json` outside the manager (pi does the same at `migrations.ts:59`). |
| CFG-002 | partially closed | Load + composition + `--list-models` + `BuiltinProviderResolver` all landed. Still open: the launch predicate (CFG-022) and the silently-dropped `oauth` key. Remains open below. |
| CFG-003 | partially closed | Read/filter/discover landed; no auto-install of a missing source. Remains open below. |
| CFG-004 | **retired into CFG-025** | All four resource types now load from settings-declared paths (extensions genuinely pushed to `ext_paths` at `discovery.rs:1242-1246`). The only residual is `~`-expansion, which is CFG-025. Id kept, not reused. |
| CFG-005 | still open | No OAuth login/refresh/registry. User deprioritised — filed, not scheduled. |
| CFG-006 | still open | `retry.provider.*` / `websocketConnectTimeoutMs` accessors still inert. |
| CFG-007 | still open | `AuthStore` re-reads `auth.json` per query; errors coerce to "not configured". |
| CFG-008 | still open | Model-scope resolution drops every diagnostic. |
| CFG-009 | still open | `npm:` source reports "unsupported source (OCI deferred)". |
| CFG-010 | partially closed | Include-list filters landed; the `autoload:false` delta model is entirely unported. Remains open below at high. |
| CFG-011 | still open | OAuth `expires` unit split across three sites. |
| CFG-012 | still open | `deep_merge` recurses unbounded; pi merges one level. |
| CFG-013 | still open | `TrustStore::nearest` reads without the file lock. |
| CFG-014 | still open | `showCacheMissNotices` absent. |
| CFG-015 | still open | Four unconsumed settings keys. |
| CFG-016 | still open | `${0:-default}` emitted literally. |
| CFG-017 | still open | `${@:-default}` / `${ARGUMENTS:-default}` unsupported. |
| CFG-018 | still open | Glob scope no longer short-circuits on an exact reference. |
| CFG-019 | still open | `defaultModelPerProvider` stale (xai id retired; radius/qwen-token-plan missing). Mitigation void after 6d29542. |
| CFG-020 | partially closed | `models_store.rs`, `ProviderConfig`, `provider_compose.rs` ported; no `ModelRuntime` type, no availability snapshot. Remains open below. |
| CFG-021 | still open | `uiMode` / `fullscreenScrollbar` unmodelled. |
| CFG-022 | still open | Launch-time `has_configured_auth` ignores models.json. |
| CFG-023 | still open | `find_initial_model` step 3 skips the auth check. |
| CFG-024 | still open | `${env:VAR}` apiKey counts as configured when unset. |
| CFG-025 | still open | No `~` / `file://` expansion on settings paths or local package sources. Absorbs CFG-004's residual. |
| CFG-026 | still open | Settings packages deduped by raw source string. |
| CFG-027 | still open | Bare-extension-directory local package contributes nothing. |
| CFG-028 | still open | `!command` config values block a tokio worker up to 10 s. |
| CFG-029 | still open | models.json launch test stubs the auth predicate. |
| CFG-030 | still open | Non-object top-level `settings.json` degraded to `{}` with no load error. |

## Open items

> **⚠ THIS TABLE IS NOT THE COMPLETE OPEN SET.** 4 further items from the 2026-08-03
> surface-driven sweep live in their own table under `## Surface-sweep findings` (line ~541), with
> `-S` ids — **including 1 rated critical/high**. Enumerating only this table undercounts the
> area by 4 items, which is exactly how `SEAM-S01` (high) escaped a full audit pass on
> 2026-08-07. Count BOTH tables. See structural defect A in `00-residual-ledger.md`.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| CFG-022 | high | parity-bug | M | Launch-time `has_configured_auth` ignores models.json — custom-provider-only install starts on faux |
| CFG-031 | high | parity-bug | S | Settings `shellPath` is not tilde-expanded, so `~/bin/bash` breaks every bash command |
| CFG-032 | high | parity-bug | S | Startup auth migration writes `auth.json` with default umask instead of 0600 |
| CFG-011 | high | parity-bug | S | OAuth `expires` is epoch-milliseconds on disk but compared against epoch-seconds |
| CFG-010 | high | not-ported | M | Package-source `autoload` delta filters are modelled nowhere |
| CFG-002 | high | not-ported | M | models.json is composed everywhere except the default launch path |
| CFG-005 | high | not-ported | L | No OAuth refresh, login, or provider registry |
| CFG-023 | medium | parity-bug | S | `find_initial_model` step 3 accepts a saved default whose provider has no configured auth |
| CFG-024 | medium | parity-bug | S | `${env:VAR}` apiKey counts as configured even when the variable is unset |
| CFG-025 | medium | parity-bug | S | Settings-declared paths and local package sources do not expand `~` or `file://` |
| CFG-026 | medium | parity-bug | S | Settings packages deduped by raw source string, not resolved identity |
| CFG-030 | medium | parity-bug | S | Non-object top-level `settings.json` degraded to `{}` with no load error |
| CFG-018 | medium | parity-bug | S | Glob scope patterns no longer short-circuit on an exact model reference |
| CFG-019 | medium | upstream-drift | S | `defaultModelPerProvider` stale — xai id retired, radius/qwen-token-plan missing |
| CFG-029 | medium | test-defect | S | models.json launch test stubs the auth predicate and is blind to it entirely |
| CFG-033 | medium | test-defect | S | cyrup-test-support's OAuth helper and fixtures encode `expires` as epoch-seconds |
| CFG-007 | medium | parity-bug | M | `AuthStore` re-reads auth.json per query and coerces errors to "not configured" |
| CFG-008 | medium | not-ported | M | Model-scope resolution drops every diagnostic |
| CFG-006 | medium | not-ported | M | `retry.provider.*` and `websocketConnectTimeoutMs` never reach the provider/HTTP layer |
| CFG-028 | medium | cyrup-original | S | Config-value `!command` resolution blocks a tokio worker for up to 10 s |
| CFG-020 | medium | not-ported | L | No `ModelRuntime` type and no availability snapshot |
| CFG-003 | medium | not-ported | L | Settings `packages` are resolved but never auto-installed |
| CFG-009 | low | parity-bug | S | An `npm:` package source fails with the misleading message "unsupported source (OCI deferred)" |
| CFG-012 | low | parity-bug | S | `deep_merge` recurses to unlimited depth where pi merges one level |
| CFG-013 | low | parity-bug | S | `TrustStore::nearest` reads trust.json without the file lock |
| CFG-016 | low | parity-bug | S | `${0:-default}` emitted literally instead of substituting |
| CFG-017 | low | parity-bug | S | `${@:-default}` / `${ARGUMENTS:-default}` prompt-template forms unsupported |
| CFG-034 | low | not-ported | S | Theme token `scrollbarThumb` is unmodelled, so a pi theme's scrollbar colour is silently dropped |
| CFG-014 | low | not-ported | M | `showCacheMissNotices` and prompt-cache-miss tracking absent |
| CFG-015 | low | not-ported | M | `warnings.anthropicExtraUsage`, `markdown.codeBlockIndent`, `lastChangelogVersion`, `npmCommand` unconsumed |
| CFG-027 | low | not-ported | M | A local package that is a bare extension directory contributes nothing |
| CFG-021 | low | not-ported | L | `uiMode` / `fullscreenScrollbar` not modelled |

## CFG-022 — Launch-time `has_configured_auth` ignores models.json

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup/src/main.rs:352-353` builds the sole availability filter as `AuthStore::at(dirs.agent_dir.join("auth.json"))` + `move |m| auth.has_auth(&m.provider, None)`, passed to `default_launch_model` at `:359-363`. `AuthStore::has_auth` (`cyrup/crates/cyrup-config/src/auth.rs:215-223`) consults only the runtime `--api-key`, `auth.json` map-key presence and `env_keys::get_env_api_key` (a table keyed on KNOWN provider ids) — a user-declared models.json provider matches none. `AgentSession::has_configured_auth` (`cyrup/crates/cyrup-session-svc/src/session.rs:2296-2337`) DOES count it (models.json branch `:2322-2330`), so the two predicates disagree.

**upstream** — `pi/packages/coding-agent/src/core/model-resolver.ts:632-647` filters on `modelRuntime.getAvailable()`; `hasConfiguredAuth` (`pi/packages/coding-agent/src/core/model-runtime.ts:371-373`) reads `snapshot.configuredProviders`, built from `models.checkAuth` over every composed provider including models.json-only ones (`pi/packages/coding-agent/src/core/provider-composer.ts:314-331`).

**Impact** — a fresh install whose only credentials live in a custom `models.json` provider finds no available model at launch and falls back to the offline faux provider. Masked for existing users by CFG-023 (step 3 returns the saved default unchecked), so it bites new installs specifically.

**Fix** — extract ONE predicate into `cyrup-config`, e.g. `provider_is_configured(&AuthStore, &ModelFile, &ProviderId, env)`, and call it from both `main.rs:353` and `session.rs:2296-2337`. CFG-024's refinement belongs in that same function.

**Verify** — extend `cyrup/crates/cyrup/tests/models_json_resolution.rs` with a step-4 case: empty `auth.json`, no `defaultModel`, a models.json provider with a literal apiKey — launch must select that provider's model, not faux. Fails at HEAD.

## CFG-031 — Settings `shellPath` is not tilde-expanded, so `~/bin/bash` breaks every bash command

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/settings.rs:699-702`: `pub fn shell_path(&self) -> Option<String> { self.merged.get_str("shellPath") }` returns the RAW string, its own doc comment at `:699` citing pi `getShellPath`. It flows unmodified through `cyrup/crates/cyrup-session-svc/src/builder.rs:605` → `BashOpts.shell_path` (`builder.rs:625`, `:1198`; `session.rs:267/376/479`) → `cyrup/crates/cyrup-tools/src/ops/shell.rs:89-97` `ShellConfig::resolve`, which tests `Path::new(p).exists()` at `:91` and otherwise returns `Err(ToolError::new(format!("Custom shell path not found: {p}")))` at `:94`. The crate already has `expand_tilde` (`settings.rs:1004-1019`) and wires it to the sibling `session_dir` at `settings.rs:577-579`.

**upstream** — `pi/packages/coding-agent/src/core/settings-manager.ts:883-886`: `getShellPath()` returns `normalizePath(shellPath)`. The key declaration at `:101` states the contract — "supports leading ~ expansion". `normalizePath` is `pi/packages/coding-agent/src/utils/paths.ts:57-78` (tilde `:65-71`, `file://` `:73-76`). pi applies it to exactly two getters — `getSessionDir` (`:676-679`) and `getShellPath` (`:883-886`); cyrup ported the first and missed the second.

**Impact** — `"shellPath": "~/bin/bash"`, the spelling pi's own key comment advertises, produces `Custom shell path not found: ~/bin/bash` on every bash tool invocation and every user `!` command for the whole session. Loud rather than silent, but a documented setting is 100% broken on its documented spelling, on the most-used tool.

**Fix** — `self.merged.get_str("shellPath").map(|s| expand_tilde(&s))` at `settings.rs:701`, implemented inside the shared path util CFG-025 introduces so `file://` and `~\` are handled once rather than three times.

**Verify** — unit test in `cyrup-config/src/settings.rs` beside the sessionDir test: with HOME overridden, `{"shellPath":"~/bin/bash"}` yields `Some("<home>/bin/bash")` — fails at HEAD. Integration beside `bash_missing_shell_path_errors` (`cyrup/crates/cyrup-tools/tests/tools.rs:1196`): a real shell at `$HOME/myshell` declared as `~/myshell` must execute.

## CFG-032 — Startup auth migration writes `auth.json` with default umask instead of 0600

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup/src/migrations.rs:95-100`: `let _ = std::fs::write(&auth_path, serialized);` (the write is `:98`), creating the file 0o666 & !umask — typically 0644. The file is guaranteed NEW: the function early-returns at `:39-42` when `auth.json` exists. It runs on every startup via `migrations::run_migrations(&dirs)` at `cyrup/crates/cyrup/src/main.rs:175` → `migrations.rs:27-28`. The migrated map holds OAuth refresh/access tokens (built `:51-67`) and plaintext API keys lifted from `settings.json` (`:69-92`). cyrup's own `AuthStore` is correct by contrast: `write_file` → `crate::lock::write_atomic` (`cyrup-config/src/auth.rs:199`), which opens the temp with `.mode(0o600)` (`lock.rs:99`) and `set_permissions(0o600)` (`lock.rs:107`) before rename, asserted at `auth.rs:632-648`. `git diff --stat c8bd2ab..HEAD` shows `migrations.rs` untouched by the 28 commits, so this predates the refresh window.

**upstream** — `pi/packages/coding-agent/src/migrations.ts:67-70`: `writeFileSync(authPath, JSON.stringify(migrated, null, 2), { mode: 0o600 })` — the mode is at `:69`. cyrup's port dropped it. pi's non-atomic `settings.json` rewrite at `:59` IS matched by cyrup at `migrations.rs:87-92`, so that half is faithful and is not part of this finding.

**Impact** — on any multi-user or shared host, every credential a user migrates off the legacy `oauth.json` / `settings.json.apiKeys` layout lands group- and world-readable in `~/.cyrup/agent/auth.json`. Silent — the migration prints only the provider list. The exposure window is "until the next `AuthStore` write" (`write_atomic` always renames a fresh 0600 temp over the target, `lock.rs:93-112`, so a later write does restore the mode) — but since cyrup has no OAuth refresh and no login flow at all (CFG-005), that is indefinite for exactly the population this migration serves.

**Fix** — replace `std::fs::write(&auth_path, serialized)` at `migrations.rs:98` with `cyrup_config::lock::write_atomic(&auth_path, serialized.as_bytes(), true)` so one code path decides `auth.json`'s permissions. Belt-and-braces: chmod 0600 an already-existing `auth.json` at `AuthStore::at` time.

**Verify** — extend the existing migration test in `cyrup/crates/cyrup/src/migrations.rs` (~`:300-340`, which asserts only content): `metadata(agent_dir.join("auth.json")).permissions().mode() & 0o777 == 0o600`. Fails at HEAD under any normal umask. There is currently zero permission coverage on this path — the only 0600 assertion in the workspace is `cyrup-config/src/auth.rs:641`, which covers the writer, not the migrator.

## CFG-011 — OAuth `expires` is epoch-milliseconds on disk but compared against epoch-seconds

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed

**cyrup** — three sites, two wrong. `cyrup/crates/cyrup-config/src/auth.rs:296-303` uses `let now = unix_millis(); if now < expires` (`unix_millis` at `:316-321`) — MILLIS, and correct. `cyrup/crates/cyrup-provider/src/auth/resolve.rs:137` `if now_secs() >= expires`, re-checked at `:152`, `now_secs` at `:183-187` — SECONDS; the field is documented `/// Unix seconds.` at `cyrup/crates/cyrup-provider/src/auth/types.rs:23`. Third site: `cyrup/crates/cyrup-test-support/src/auth.rs:129` and `:144`, both `if now_secs() < *expires` (`now_secs` `:70`).

**upstream** — `pi/packages/ai/src/auth/oauth/anthropic.ts:225` and `:338` both write `expires: Date.now() + expires_in * 1000 - 5 * 60 * 1000` — epoch MILLISECONDS is the on-disk contract.

**Impact** — a pi-written credential (~1.7e12) read by `cyrup-provider` compares as a far-future seconds value, so an expired token is used verbatim and never refreshed; conversely a seconds-encoded value written by cyrup-provider reads to `cyrup-config` as expired-in-1970 and yields no key. Silent auth failures and silent use of dead tokens depending on which path runs.

**Fix** — move all three sites plus the doc comment in one changeset: `resolve.rs:137/:152` onto a millis clock, `cyrup-test-support/src/auth.rs:70` `now_secs` → `now_millis` with `:129`/`:144` updated, `types.rs:23` re-documented as Unix milliseconds.

**Verify** — asymmetric test the old code cannot pass: seed `expires = now_millis() - 1` and assert the refresh/None branch is taken (under the seconds reading that value is ~1.7e12 "seconds" in the future). Pair with `expires = now_millis() + 60_000` asserting verbatim return. See CFG-033 for the paired fixture work.

## CFG-010 — Package-source `autoload` delta filters are modelled nowhere

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed

**cyrup** — include-list half only: `PackageSource::filters()` at `cyrup/crates/cyrup-config/src/settings.rs:107-143`, the `Detailed` variant declared `:94-104` with ONLY `source/extensions/skills/prompts/themes` — no `autoload`. `grep -rn autoload crates/ --include=*.rs` is empty workspace-wide at HEAD. Filters reach `ConfiguredPackage.filter` (`cyrup/crates/cyrup-session-svc/src/builder.rs:1578-1588`) and are applied by `retain_by_package_filter` (`cyrup/crates/cyrup-resources/src/discovery.rs:247-263`), which returns early keeping EVERYTHING when all fields are None (`:252-254`).

**upstream** — `pi/packages/coding-agent/src/core/package-manager.ts:2073-2092` `collectPackageResources` branches on `filter.autoload === false` at `:2084` into `applyPackageDeltaFilter` (`:2085`, body `:2173-2189`), and `return true` whenever a filter object is present is at `:2091`. `applyPackageDeltaFilter` early-returns at `:2180-2182` when `userPatterns` is empty, adding nothing to the target map. Also unported: the `dedupePackages` delta branch (`:1694-1696`, which KEEPS both entries when the project one is `autoload=false`), and the fact that pi never reads a `Detailed` entry's manifest (the `if (filter)` branch at `:2079` returns before `readPiManifest` at `:2094`) while cyrup always calls `resolve_manifest` first (`discovery.rs:649`).

**Impact** — for a bare `{"source":"github:org/pack","autoload":false}` pi contributes ZERO resources; cyrup keeps EVERYTHING. A package the user explicitly opted out of loads in full — every skill, prompt, theme and extension it ships.

**Fix** — add `autoload: Option<bool>` to `PackageSource::Detailed` (`settings.rs:94-104`), carry it into `PackageFilter`, and in `discovery.rs:247-263` branch: when `autoload == Some(false)`, start from an EMPTY set and add back only explicitly-listed patterns, mirroring `applyPackageDeltaFilter`; skip `resolve_manifest` when a filter object is present, matching `package-manager.ts:2079`.

**Verify** — test in `cyrup/crates/cyrup-resources/tests/resources.rs`: a package tree with two skills declared `{"source": …, "autoload": false}` → zero skills; with `{"autoload": false, "skills": ["a"]}` → exactly one. Both fail at HEAD.

## CFG-002 — models.json is composed everywhere except the default launch path

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed

**cyrup** — landed: `load_models_file_reporting` before `--list-models` and provider selection at `cyrup/crates/cyrup/src/main.rs:214-235` (composition errors surfaced `:219-223`); composed as the top layer of `full_model_registry` at `cyrup/crates/cyrup-session-svc/src/session.rs:2359-2394`; `BuiltinProviderResolver::new(models_json.clone())` at `main.rs:381`. `ProviderConfig` (`cyrup/crates/cyrup-config/src/model.rs:1433-1465`) carries name/base_url/api_key/api/headers/auth_header/`compat`(`:1449`)/`models`(`:1452`)/`model_overrides`(`:1457`); `apply_models_json` at `model.rs:1745-1800`. Still missing: `grep -n oauth crates/cyrup-config/src/model.rs` returns ZERO hits, so under `#[derive(Deserialize)]` with no `deny_unknown_fields` the `oauth` key is silently dropped; and the empty-block guard at `model.rs:1751-1758` omits pi's `!config.oauth` clause.

**upstream** — `pi/packages/coding-agent/src/core/provider-composer.ts:167-169` throws `Provider ${id}: "baseUrl" is required when "oauth" is set.`; the empty-block message is `:181-183`; the `config.oauth === "radius"` baseUrl special-case is `:188`; the `!config.oauth` clause in the empty-block guard is `:178`.

**Impact** — a models.json provider block using `oauth` is either silently stripped of its auth mode, or (for a block of only `{"oauth":"radius"}`) rejected with the misleading "must specify baseUrl, headers, compat, modelOverrides, or models" that pi accepts. Plus the launch-predicate half, filed separately as CFG-022.

**Fix** — add `oauth: Option<String>` to `ProviderConfig` (`model.rs:1433-1465`); in `apply_models_json` (`:1745-1800`) reject a block with `oauth` and no `base_url` unless `oauth == "radius"`, and add `oauth.is_none()` to the empty-block guard at `:1751-1758`.

**Verify** — table test in `cyrup-config/src/model.rs`: `{"oauth":"anthropic"}` with no baseUrl → pi's error text; `{"oauth":"radius"}` alone → accepted; `{}` → the existing empty-block error. First two fail at HEAD.

## CFG-005 — No OAuth refresh, login, or provider registry

**Kind** not-ported · **Severity** high · **Effort** L · **Confidence** confirmed

**cyrup** — `grep -rn 'impl OAuthAuth|OAuthAuth for' crates/ --include=*.rs` returns exactly two hits, both inside `#[cfg(test)]`: `cyrup/crates/cyrup-provider/src/auth/resolve.rs:445` and `cyrup/crates/cyrup-test-support/src/auth.rs:184`. `cyrup/crates/cyrup-provider/src/auth/` is helpers/mod/resolve/store/types — no oauth module. `AuthStore::get_api_key` returns `Ok(None)` for an expired `Credential::Oauth` at `cyrup/crates/cyrup-config/src/auth.rs:296-303`. `AuthStore` (`auth.rs:72-313`) has no `login`/`logout`/`oauth_providers`.

**upstream** — `pi/packages/coding-agent/src/core/model-runtime.ts:505` (`ModelRuntime.login`), `pi/packages/ai/src/models.ts:431`, eleven flows under `pi/packages/ai/src/auth/oauth/`.

**Impact** — stored OAuth credentials can be used until they expire, then the provider goes dead with no in-product path to re-authenticate.

**Fix** — port `pi/packages/ai/src/auth/oauth/` as a `cyrup-provider/src/auth/oauth/` module (PKCE + local callback first, per-provider flows after), and add `login`/`logout` to `AuthStore`.

**Verify** — refresh path first: an expired `Credential::Oauth` with a valid refresh token must yield a working key and a rewritten `auth.json`. Land CFG-011 first — its three-site unit bug bites the moment any of this lands. User has DEPRIORITISED this item: keep filed, do not schedule.

## CFG-023 — `find_initial_model` step 3 accepts a saved default whose provider has no configured auth

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/model.rs:1328-1341`: step 3 finds the saved `(provider, model)` in `all` (`:1330-1333`) and returns it unconditionally (`:1335-1340`). `has_configured_auth` is in scope (parameter `:1289`, used by step 1 via `resolve_cli_model` at `:1295`) but step 3 never calls it. Contrast `restore_model_from_session` (`:1373-1393`), which does check `restored_has_auth`.

**upstream** — `pi/packages/coding-agent/src/core/model-resolver.ts:620` `// 3. Try saved default from settings if auth is configured.`, `:621` the guard, `:623` `if (found && modelRuntime.hasConfiguredAuth(found.provider))`; on a failed check pi falls through to step 4 at `:631`.

**Impact** — a user who removes a provider's credentials keeps launching into that provider's model and gets an auth error per turn instead of falling back to a working model. Also masks CFG-022 for existing users, which is why CFG-022 only bites fresh installs.

**Fix** — add `&& has_configured_auth(found)` to the step-3 condition at `model.rs:1330-1333`. Land together with CFG-022's unified predicate, or step 3 will start rejecting models.json providers.

**Verify** — unit test in `model.rs`: a saved default naming a provider the predicate rejects yields step 4's result, not the saved one. Fails at HEAD.

## CFG-024 — `${env:VAR}` apiKey counts as configured even when the variable is unset

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:2322-2330` returns true whenever the models.json block has `cfg.api_key.is_some()` (`:2328`) — the whole test is presence; the config-value language is never inspected. The security comment at `:2310-2321` is correct and still holds: no status-query path resolves a config value (`AuthStore::has_auth` `auth.rs:215-223` and `get_auth_status` `:227-260` test map-key presence only; the three sites that EXECUTE a config value — `auth.rs:289`, `cyrup-config/src/provider_compose.rs:206-212`, `model.rs:1475-1478` — are all request-path).

**upstream** — `pi/packages/coding-agent/src/core/provider-composer.ts:321-327`: for a raw key pi returns configured immediately if `isCommandConfigValue(rawKey)` (`:322`), otherwise gets `envNames` (`:323`) and requires EVERY named var to exist, returning undefined on the first missing one (`:324-326`).

**Impact** — a provider whose apiKey is `${env:MYCORP_KEY}` with the variable unset is reported as configured, is selectable at launch, and fails on the first request instead of being filtered out.

**Fix** — one line at `session.rs:2328`: replace the presence test with `is_config_value_configured(&raw)`. cyrup already has the exact pure machinery — `is_command_config_value` (`cyrup-config/src/config_value.rs:221`), `missing_config_value_env_var_names` (`:210`), `is_config_value_configured` (`:226-227`) — with ZERO non-test callers today (definition, the `lib.rs:41` re-export, and one `#[cfg(test)]` assertion at `config_value.rs:639`). No process spawns, so 51bb11a's security property is preserved. Belongs inside CFG-022's shared predicate.

**Verify** — test in `cyrup-session-svc`: models.json provider with `"apiKey": "${env:UNSET_VAR}"` → `has_configured_auth` false; with the var set → true; with `"!echo k"` → true without executing. First case fails at HEAD.

## CFG-025 — Settings-declared paths and local package sources do not expand `~` or `file://`

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-resources/src/package/manifest.rs:359-365`: `let trimmed = entry.trim(); let p = PathBuf::from(trimmed); plain.push(if p.is_absolute() { p } else { base.join(trimmed) });` — so `~/team-skills` becomes `<base>/~/team-skills`. Identical shape at `cyrup/crates/cyrup-resources/src/discovery.rs:294-303` for a settings-declared local PACKAGE path, which then trips the `:323-333` diagnostic reporting a misleading cause ("is not installed at this path — run cyrup install …"). cyrup has `expand_tilde` at `cyrup/crates/cyrup-config/src/settings.rs:1004-1019` but wired to exactly ONE caller (`session_dir`, `settings.rs:577-579`), handling only `~` and `~/` — no `file://`, no `~\`.

**upstream** — `pi/packages/coding-agent/src/core/package-manager.ts:2069-2071` `resolvePathFromBase = resolvePath(input, baseDir, { homeDir: getHomeDir(), trim: true })`; `pi/packages/coding-agent/src/utils/paths.ts:57-78` `normalizePath` expands `~` (`:65-71`) and converts `file://` (`:73-76`).

**Impact** — `"skills": ["~/team-skills"]` silently loads nothing; `"packages": ["~/pack"]` produces a diagnostic naming the wrong cause. Absorbs CFG-004's residual: the regression test at `cyrup/crates/cyrup-session-svc/tests/settings_resolve.rs:173-192` uses the RELATIVE path `"extra"`, so nothing in the suite exercises `~`.

**Fix** — move `expand_tilde` into a shared util that also handles `file://` and `~\`; apply at `manifest.rs:360` (BEFORE the `is_absolute` test) and `discovery.rs:296`, taking the home dir from `DiscoveryConfig` rather than the ambient env, mirroring pi's `options.homeDir`. CFG-031 wants the same util — do them together.

**Verify** — test in `cyrup-resources/tests/resources.rs` with a `DiscoveryConfig` home override: `"skills": ["~/team-skills"]` loads the skill; `"packages": ["file:///abs/pack"]` resolves. Both fail at HEAD.

## CFG-026 — Settings packages deduped by raw source string, not resolved identity

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-session-svc/src/builder.rs:1568` `let source = entry.source().trim().to_string();` then `:1575` `if out.iter().any(|p| p.source == source) { continue; }` — the key is the literal settings string before any resolution, and project entries are pushed first (`:1562-1565`). The two scopes resolve to DIFFERENT bases (`cyrup/crates/cyrup-resources/src/discovery.rs:279-282`: `<cwd>/.cyrup` for project, `cfg.global_dir` for global).

**upstream** — `pi/packages/coding-agent/src/core/package-manager.ts:1660-1675` `getPackageIdentity` returns `npm:<name>`, normalized `git:<host>/<path>`, or `local:${resolvePathFromBase(parsed.path, baseDir)}` — a SCOPE-RESOLVED absolute path (`:1670-1672`) — and `dedupePackages` keys on that (`:1681-1701`).

**Impact** — `"packages": ["./pack"]` declared in both scopes means two different directories to pi (both loaded); cyrup drops the global one and its resources never appear.

**Fix** — compute the identity in `builder.rs:1568` as pi does: resolve local paths against the scope base before using them as the dedupe key, normalize git specs. Land with CFG-010's autoload delta rule, which lives in the same dedupe upstream (`package-manager.ts:1694-1696`).

**Verify** — test in `cyrup-session-svc/tests/settings_resolve.rs`: `"./pack"` in both global and project settings, each pointing at a distinct on-disk tree with distinct skills → both skills present. Fails at HEAD.

## CFG-030 — Non-object top-level `settings.json` degraded to `{}` with no load error

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `Settings::parse` at `cyrup/crates/cyrup-config/src/settings.rs:162-175` deserializes into `serde_json::Value` at `:166` — not a Map — then `match value { Value::Object(mut obj) => …, _ => Ok(Self::default()) }` at `:167-174`, commented "// A non-object top-level is treated as empty (degraded), never a panic." at `:173`. So `[1,2,3]`, `"str"`, `123`, `true`, `null` parse SUCCESSFULLY, produce no `ScopedError`, never reach `record_load_error` (`:1179-1185`, reached only from the two error arms of `load_scope` at `:1192`/`:1198`), leave `ensure_scope_writable` (`:1297-1305`) satisfied, and the next `/config` toggle rewrites the file from an empty document.

**upstream** — `pi/packages/coding-agent/src/core/settings-manager.ts` does a bare `JSON.parse` then `migrateSettings`, so pi does not latch either — but pi's writer spreads the parsed current document (`const mergedSettings: Settings = { ...currentFileSettings };` at `:593`), which for an array preserves the elements as indexed keys. Both mangle the file; only cyrup discards its contents entirely.

**Impact** — a settings.json that is valid JSON but not an object is silently emptied on the next write, losing whatever the user had there, with no diagnostic and no write refusal. CFG-001's protections all apply to malformed TEXT and none to this case.

**Fix** — one line: make `Settings::parse` return `Err` for a non-object top level at `settings.rs:174`; the latch, the diagnostic and all four write refusals then apply unchanged.

**Verify** — add a `[1,2,3]` case to CFG-001's byte-equality suite (`settings.rs:2210-2365`, which today seeds only malformed text): a `/config` write must be refused and the file byte-identical afterwards. Fails at HEAD.

## CFG-018 — Glob scope patterns no longer short-circuit on an exact model reference

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — the glob branch of `ModelResolver::resolve_scope` (`cyrup/crates/cyrup-config/src/model.rs:257-273`) strips an optional `:level` suffix (`:260-267`) then goes straight to the glob_match filter over `self.available` (`:268-273`) — no `match_reference` attempt on the stripped pattern.

**upstream** — `findExactModelReferenceMatch` is declared at `pi/packages/coding-agent/src/core/model-resolver.ts:79` and called INSIDE the glob branch at `:297`, before the minimatch filter (it is also used on the non-glob path at `:128`).

**Impact** — a scope pattern that is an exact model reference but happens to contain a glob metacharacter resolves through the filter instead of matching directly, so it can silently resolve to nothing or to the wrong set.

**Fix** — insert a `match_reference` attempt at `model.rs:268`, returning early on a hit.

**Verify** — unit test in `model.rs`: a pattern that is an exact model reference containing a metacharacter resolves to exactly that model. Fails at HEAD.

## CFG-019 — `defaultModelPerProvider` stale — xai id retired, radius/qwen-token-plan missing

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `default_model_per_provider` at `cyrup/crates/cyrup-config/src/model.rs:936-976` has `"xai" => "grok-4.20-0309-reasoning"` at `:951` and no radius / qwen-token-plan / qwen-token-plan-cn arms; 35 arms total. `KNOWN_PROVIDERS` at `:979-1014` is likewise 35 entries.

**upstream** — `pi/packages/coding-agent/src/core/model-resolver.ts:13-52`, 38 entries: `radius: "auto"` at `:20`, `xai: "grok-4.5"` at `:28`, `"qwen-token-plan"` / `"qwen-token-plan-cn": "qwen3.7-max"` at `:46-47`. Every other arm matches cyrup exactly.

**Impact** — on identical catalogs, a user with only xAI configured and no saved `defaultModel` launches a different model in cyrup than in pi. The old "the catalog doesn't have grok-4.5 anyway" mitigation is void: `cyrup/crates/cyrup-provider/src/providers/catalog/xai.json` carries BOTH ids after 6d29542.

**Fix** — correct the xai arm at `model.rs:951` to `grok-4.5` and add the three missing arms plus matching `KNOWN_PROVIDERS` entries.

**Verify** — table test asserting cyrup's map equals pi's 38 entries. No test pins the stale id today — `grep -rn grok-4.20-0309-reasoning crates/ --include=*.rs` returns only `model.rs:951` itself.

## CFG-029 — models.json launch test stubs the auth predicate and is blind to it entirely

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup/tests/models_json_resolution.rs`: `a_settings_default_model_can_name_a_models_json_provider` (`:93`) builds `let configured = |_: &cyrup_provider::Model| true;` at `:95` and drives `default_launch_model(Some("mycorp"), Some("mycorp-large"), …)`, which lands on `find_initial_model` STEP 3 (`cyrup-config/src/model.rs:1328-1341`) — and cyrup's step 3 never consults `has_configured_auth` at all (CFG-023), so the test passes identically with `|_| false`. Nothing in the 154-line file exercises step 4, the only place the predicate decides anything and the only place CFG-022 lives. The doc comment at `:90-91` cites "pi findInitialModel step 3, model-resolver.ts:600-609".

**upstream** — `pi/packages/coding-agent/src/core/model-resolver.ts:600-609` is the tail of pi's STEP 1 (`resolveCliModel`); pi's step 3 is `:620-630` and REQUIRES `hasConfiguredAuth` at `:623`. The comment documents a divergence as if it were the port.

**Impact** — the only test covering models.json at launch cannot fail on either CFG-022 or CFG-023, and its doc comment will lead the next reader to believe cyrup's step 3 matches pi.

**Fix** — replace the always-true closure at `:95` with the real predicate the binary installs (`AuthStore` over a temp empty `auth.json` PLUS the loaded `ModelFile`), factored into CFG-022's shared function; add a step-4 case with no `defaultModel` at all; fix the doc comment's line cite to `:620-630`.

**Verify** — with the real predicate the step-3 test must FAIL at HEAD (proving it now exercises the check) and pass after CFG-023.

## CFG-033 — cyrup-test-support's OAuth helper and fixtures encode `expires` as epoch-seconds

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-test-support/src/auth.rs`: the NON-test public helper `resolve_api_key_refreshing_in` (`:116-168`, re-exported through `resolve_api_key_refreshing` `:112-114`, which runs against `get_real_auth_store()`) reads a `cyrup_config::Credential` from the REAL `AuthStore` and decides expiry with `if now_secs() < *expires` at `:129`, and again in the modify double-check closure at `:144` (`now_secs` `:70-75`). cyrup-config decides the SAME on-disk field with `unix_millis()` at `cyrup-config/src/auth.rs:296-303`. The fixtures bake the seconds reading in: `FreshOAuth::refresh` mints `expires: now_secs() + 3600` at `:192`, `valid_oauth_returns_verbatim_without_refresh` seeds the same at `:244`, and `expired_oauth_is_refreshed_and_written_back` asserts `expires > now_secs()` at `:230`. All three pass today only because the helper shares the bug.

**upstream** — `pi/packages/ai/src/auth/oauth/anthropic.ts:225` and `:338` both write `expires: Date.now() + expires_in * 1000 - 5 * 60 * 1000` — epoch MILLISECONDS is the on-disk contract, so a fixture whose `expires` is ~1.7e9 represents a credential that expired in 1970.

**Impact** — the harness meant to prove OAuth parity models the on-disk format wrongly and reports green while a real pi-written `auth.json` is misread. A credential this helper writes back after refresh is unreadable by `AuthStore::get_api_key`, which sees ~1.7e9 < now_millis and returns None — the refresh "succeeds" and the key is still unusable. It also makes CFG-011 look like a single-site typo in cyrup-provider when it is a three-site convention split, which is how the wrong half gets "fixed".

**Fix** — one changeset with CFG-011: switch `auth.rs:70`'s `now_secs` to `now_millis`, update `:129` and `:144`, re-express the fixtures as `now_millis() + 3_600_000` (`:192`, `:244`) and assert `expires > now_millis()` (`:230`). Keep `expires: 0` at `:215` — unit-agnostic. Correct `/// Unix seconds.` at `cyrup-provider/src/auth/types.rs:23` in the same commit so no fourth site is written from the wrong doc.

**Verify** — add the asymmetric pair from CFG-011 (`now_millis() - 1` must take the refresh branch; `now_millis() + 60_000` must return verbatim), then grep `crates/ --include=*.rs` for any remaining `now_secs` touching an oauth `expires` (today: `cyrup-provider/src/auth/resolve.rs:137` and `:152`).

## CFG-007 — `AuthStore` re-reads auth.json per query and coerces errors to "not configured"

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/auth.rs:215-222`: `has_auth` is `matches!(self.read_file(), Ok(map) if map.contains_key(..))` at `:219` — any `Err` reads as not-configured. `get_auth_status` (`:227-260`) uses the same idiom at `:232`. `read_file` hits the filesystem on every call; the RwLock covers only the runtime `--api-key` tier. No cached `AuthFile`, no `reload()`. Live callers: `cyrup/crates/cyrup-tui/src/app.rs:1941`, `cyrup/crates/cyrup-session-svc/src/session.rs:2344`, `cyrup/crates/cyrup/src/main.rs:325` and `:353`.

**upstream** — `pi/packages/coding-agent/src/core/auth-storage.ts:188-204` (`readState` seeded from a process-wide `sharedAuthFileReadState`), `:236-247` (`reload()` ending in `catch { /* Preserve the last valid in-memory snapshot. */ }`), `:260-273` (`readLatestData` short-circuits on a file-revision match).

**Impact** — a transient read error, or a mid-write window, makes every configured provider read as unauthenticated; pi keeps the last good snapshot. Plus one syscall per auth query on hot TUI paths.

**Fix** — add a cached `AuthFile` + revision behind the existing RwLock, an explicit `reload()` that preserves the prior snapshot on error, and route `has_auth`/`get_auth_status`/`get_api_key` through it.

**Verify** — test: populate `auth.json`, query once, make it unreadable, query again → still configured. Fails at HEAD.

## CFG-008 — Model-scope resolution drops every diagnostic

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `ModelResolver::resolve_scope` at `cyrup/crates/cyrup-config/src/model.rs:236-283` returns a bare `Vec<ScopedModel>`: the glob branch `:257-273` falls through silently when the filter matches nothing, and the non-glob branch `:274-280` keeps only `parsed.model` (`:276`) and DISCARDS `parsed.warning`. No `ModelScopeDiagnostic` type exists; `grep -rn 'No models match' crates/` is empty.

**upstream** — `pi/packages/coding-agent/src/core/model-resolver.ts:261` declares `ModelScopeDiagnostic`, `:268-271` `ResolveModelScopeResult`, the `diagnostics` accumulator at `:279`, `No models match pattern "${pattern}"` pushed at `:316` (glob) and `:340` (reference), and the `Invalid thinking level "${suffix}" in pattern` warning minted at `:243`.

**Impact** — a typo'd `--models 'anthorpic/*'` resolves to nothing with no explanation, as does an invalid thinking-level suffix.

**Fix** — introduce `ModelScopeDiagnostic` plus a result struct in `model.rs`, accumulate at `:268-273` and `:274-280`, and surface at the CLI/session call sites.

**Verify** — unit test: `anthorpic/*` yields one diagnostic with pi's exact text; `anthropic/x:bogus` yields the invalid-level warning. Both fail at HEAD.

## CFG-006 — `retry.provider.*` and `websocketConnectTimeoutMs` never reach the provider/HTTP layer

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — accessors present and inert: `provider_max_retry_delay_ms` (`cyrup/crates/cyrup-config/src/settings.rs:661`), `websocket_connect_timeout_ms` (`:680`), `provider_retry_timeout_ms` (`:862`), `provider_retry_settings` (`:875-881`). Grepping all four names across `crates/` returns ONLY `settings.rs` (incl. its own tests `:1791`/`:1940`/`:1989`) plus two inert hits: `cyrup/crates/cyrup-provider/src/stream.rs:179` (field declaration) and `cyrup/crates/cyrup-provider/src/utils/simple_options.rs:84` (copies the field forward). Nothing assigns either from settings.

**upstream** — `getProviderRetrySettings` / `getWebSocketConnectTimeoutMs` in `pi/packages/coding-agent/src/core/settings-manager.ts` (keys declared `:24-33` and `:131`), consumed in `pi/packages/coding-agent/src/core/sdk.ts`.

**Impact** — a user tuning retry behaviour or websocket connect timeout in settings gets no effect at all; the provider layer always uses built-in defaults.

**Fix** — thread `provider_retry_settings()` and `websocket_connect_timeout_ms()` from `cyrup-session-svc/src/builder.rs` into `cyrup-provider`'s stream/simple options at the two existing field sites.

**Verify** — test asserting the constructed provider options carry the settings values. Land CFG-012 alongside — `deep_merge`'s unlimited recursion is unobservable only because `retry.provider` is the sole two-level key and it is unconsumed.

## CFG-028 — Config-value `!command` resolution blocks a tokio worker for up to 10 s

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-config/src/config_value.rs:306-354`: `run_with_timeout` is fully synchronous — `let start = Instant::now()` `:328`, `let timeout = Duration::from_millis(10_000)` `:329`, elapsed check `:344`, `std::thread::sleep(Duration::from_millis(10))` `:349`. Reached from two async fns with no `spawn_blocking`: `AuthStore::get_api_key` (`cyrup-config/src/auth.rs:271-292`, resolve at `:289`) and `ConfiguredApiKeyAuth::resolve` (`cyrup-config/src/provider_compose.rs:183-214`, resolve at `:206-212`).

**upstream** — pi calls `resolveConfigValueOrThrow` synchronously inside its async resolve, implemented with `execSync` — pi blocks its single event loop identically.

**Impact** — a slow `!command` credential helper stalls a tokio worker thread for up to 10 s, degrading unrelated concurrent work. Faithful to pi's semantics; it earns a filing only because cyrup's runtime is multi-task.

**Fix** — add an async entry point in `config_value.rs` wrapping the blocking body in `tokio::task::spawn_blocking`, and call it from `auth.rs:289` and `provider_compose.rs:206-212`. Do NOT change the 10 s ceiling — that is pi's number.

**Verify** — test that a 1 s `!sleep 1` credential resolve does not delay a concurrently-spawned task by more than a few milliseconds.

## CFG-020 — No `ModelRuntime` type and no availability snapshot

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed

**cyrup** — ported: `cyrup/crates/cyrup-config/src/models_store.rs`, the `ProviderConfig` shape (`model.rs:1433-1465`), and `rebuildProviders`/`composeModelProvider` semantics in `cyrup/crates/cyrup-config/src/provider_compose.rs`. Not ported: `grep -rn ModelRuntime crates/ --include=*.rs` returns ONLY doc comments (`session.rs:2356`, `provider_compose.rs:2/9/113/170/269/333/396`, `provider.rs:14/101/120/219`, `services.rs:42/77/85`, test headers) — no type. `full_model_registry()` (`cyrup/crates/cyrup-session-svc/src/session.rs:2359-2394`) is recomposed on EVERY call (`available_model_catalog` at `:2406` calls it once and then filters; `has_configured_auth` at `:2296-2337` does no recomposition — the cost is per-call, not per-model). cyrup's `ApiKeyAuth` trait (`cyrup/crates/cyrup-provider/src/auth/mod.rs:51-62`) has only `resolve` — pi's `ProviderAuthMethod.check` (`pi/packages/coding-agent/src/core/provider-composer.ts:314-331`), the status-query half, has no counterpart.

**upstream** — `pi/packages/coding-agent/src/core/model-runtime.ts` holds a snapshot with `configuredProviders` and `getAvailable()`, rebuilt on explicit invalidation rather than per query.

**Impact** — repeated per-call recomposition of the whole registry, and, more importantly, the absence of a single snapshot is what let the two `has_configured_auth` implementations drift (CFG-022, CFG-024).

**Fix** — introduce a `ModelRuntime` in `cyrup-config` owning the composed registry plus a `configured_providers` set, invalidated on settings/auth/models.json change; add a `check` method to `ApiKeyAuth`; have `session.rs:2359-2394` and `main.rs` both read from it.

**Verify** — assert the registry is composed once per invalidation rather than once per query, and that `main.rs` and `AgentSession` return identical `configured_providers` for a models.json-only provider.

## CFG-003 — Settings `packages` are resolved but never auto-installed

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed

**cyrup** — read/filter/discover half landed: `configured_packages_from_settings` (`cyrup/crates/cyrup-session-svc/src/builder.rs:1556-1592`) reads the two raw layers project-then-global (`:1562-1565`), skips an empty source with a diagnostic (`:1569-1572`), carries include filters into `PackageFilter` (`:1578-1588`); discovery consumes them at `cyrup/crates/cyrup-resources/src/discovery.rs:590-604` via `resolve_configured_package` (`:273-341`). That function resolves a local Path against the scope base (`:294-303`) and git/oci ONLY through an already-materialized `cyrup install` tree via `installed_dir` (`:305-321`); anything else becomes the loud diagnostic at `:323-333`. The `[CYRUP-DELTA]` no-network-install note is at `discovery.rs:265-271`.

**upstream** — `pi/packages/coding-agent/src/core/package-manager.ts:1224-1283` `resolvePackageSources` installs the missing source (`:1244-1274`).

**Impact** — a fresh clone whose `.cyrup/settings.json` lists `github:org/pack` gets zero resources from it and a diagnostic telling the user to run `cyrup install` manually.

**Fix** — implement the git/npm/oci fetch path behind `resolve_configured_package` (`discovery.rs:305-321`), reusing whatever `cyrup install` already does, gated on an explicit opt-in setting since this is a network operation at session start.

**Verify** — integration test with a local bare git remote declared in settings: the first session materializes the tree and loads its skills.

## CFG-009 — An `npm:` package source fails with the misleading message "unsupported source (OCI deferred)"

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `PackageSource::parse` returns `Err(ResourceError::Unsupported)` for an `npm:` prefix at `cyrup/crates/cyrup-resources/src/package/source.rs:79-81` (the documented R-09-021 drop is at `:70-71`), whose Display is `#[error("unsupported source (OCI deferred)")]` at `cyrup/crates/cyrup-resources/src/error.rs:40-41`. CFG-003's wiring routes settings-declared entries through the same parse (`discovery.rs:282`, wrapped into a diagnostic at `:283-290`), so it now appears on a normal session start.

**upstream** — pi's `parseSource` returns a full npm source, consumed by `resolvePackageSources`' npm branch at `pi/packages/coding-agent/src/core/package-manager.ts:1257-1268`.

**Impact** — a user declaring an npm package is told the problem is OCI. The npm channel drop itself is a documented decision; only the message is wrong.

**Fix** — split `ResourceError::Unsupported` into `UnsupportedNpm` / `UnsupportedOci` in `error.rs:40-41` with accurate text. Dangling consequence: `EffectiveSettings::npm_command()` (`cyrup-config/src/settings.rs:709-711`) has zero consumers outside `cyrup-config/src` for the same root cause (CFG-015).

**Verify** — assert the message text for an `npm:` source. The existing test at `cyrup/crates/cyrup-resources/tests/resources.rs:1987-1991` asserts the VARIANT, not the text, so it neither pins nor guards the message.

## CFG-012 — `deep_merge` recurses to unlimited depth where pi merges one level

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `deep_merge` at `cyrup/crates/cyrup-config/src/settings.rs:450-466` recurses at every level (`Some(bv) => deep_merge(bv, ov)` at `:456`).

**upstream** — `deepMergeSettings` at `pi/packages/coding-agent/src/core/settings-manager.ts:137-165`: the nested branch is a SINGLE-LEVEL object spread `{ ...baseValue, ...overrideValue }` at `:157`, so at depth 2 pi REPLACES the base's object wholesale where cyrup merges it key-by-key.

**Impact** — none observable today: `retry.provider` is pi's only two-level key and it is unconsumed (CFG-006). Becomes a real divergence the moment CFG-006 lands or any other nested key appears.

**Fix** — replace the recursive call at `settings.rs:456` with a one-level key-wise override at depth ≥ 2.

**Verify** — a depth-2 test asserting project `retry.provider` REPLACES global `retry.provider` rather than merging. The existing `deep_merge_precedence_and_nested` (`settings.rs:1600-1624`) exercises depth 1 only (`retry.{enabled,maxRetries}`) and passes under either implementation — silent about the divergence, not asserting it.

## CFG-013 — `TrustStore::nearest` reads trust.json without the file lock

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `nearest()` at `cyrup/crates/cyrup-config/src/trust.rs:140-156` calls `self.read_map()` at `:141` with no `crate::lock::FileLock::acquire`, while `set_many` (`:158-…`) acquires it at `:163` before its own `read_map` at `:164`.

**upstream** — `pi/packages/coding-agent/src/core/trust-manager.ts:219-220` wraps `getEntry(cwd)`'s read in `withTrustFileLock` (defined `:168`); `isTrusted` at `:216` goes through `getEntry`.

**Impact** — negligible on POSIX, since the writer uses rename-based `write_atomic`. A consistency-posture divergence that matters if the writer ever stops being atomic or a non-POSIX target appears.

**Fix** — acquire the lock around `read_map()` at `trust.rs:141`.

**Verify** — code review; no behavioural test is meaningful on POSIX.

## CFG-016 — `${0:-default}` emitted literally instead of substituting

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `match_brace_form` at `cyrup/crates/cyrup-resources/src/prompt.rs:236-283`. The `:-` guard is `:244-247`; line `:248` is `let idx = num.parse::<usize>().ok()?.checked_sub(1)?;` — for `num == "0"`, `checked_sub(1)` is None and the `?` aborts the WHOLE form, so `substitute_args` falls to the unrecognized-`${…}` path and emits the token verbatim.

**upstream** — `pi/packages/coding-agent/src/core/prompt-templates.ts:74-80` — the regex alternative `\$\{(\d+|ARGUMENTS|@):-([^}]*)\}` matches, and the handler indexes `args[0-1] = args[-1] = undefined`, which is falsy, so it returns the default.

**Impact** — a prompt template using `${0:-default}` renders the literal `${0:-default}` into the model's context instead of the default text.

**Fix** — one line at `prompt.rs:248`: treat index 0 as "no such arg" and take the default branch instead of aborting the form.

**Verify** — unit test in `prompt.rs`: `${0:-fallback}` with no args renders `fallback`. No test pins the current behaviour (grepping `cyrup-resources` for `${0` and `${@:-` finds nothing), so the fix is unguarded in both directions.

## CFG-017 — `${@:-default}` / `${ARGUMENTS:-default}` prompt-template forms unsupported

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `match_brace_form`'s `:-` guard at `cyrup/crates/cyrup-resources/src/prompt.rs:244-247` requires `num.bytes().all(is_ascii_digit)` (`:246`). For inner `"@:-default"`: `split_once(":-")` yields `num="@"` → guard fails; the next arm `strip_prefix("@:")` (`:257`) yields `"-default"` → `start_str` fails the all-digits test at `:262-264` → returns None; the token falls out to the literal-`$` path.

**upstream** — `pi/packages/coding-agent/src/core/prompt-templates.ts:74` regex accepts `(\d+|ARGUMENTS|@)` and the handler resolves `@`/`ARGUMENTS` to `allArgs` at `:78-79`.

**Impact** — `${@:-default}` and `${ARGUMENTS:-default}` render literally into the prompt.

**Fix** — extend the guard at `prompt.rs:246` and thread `all_args` into `match_brace_form` (signature `:236` currently takes only `args`). The `${@:N}` / `${@:N:L}` slice family at `:256-280` is CORRECT and unaffected — `saturating_sub(1)` at `:267` matches pi's bash-0-means-1 rule (`prompt-templates.ts:82-84`).

**Verify** — unit tests for `${@:-fallback}` and `${ARGUMENTS:-fallback}`, with and without args.

## CFG-034 — Theme token `scrollbarThumb` is unmodelled, so a pi theme's scrollbar colour is silently dropped

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `grep -rni 'scrollbarThumb|scrollbar_thumb' crates/ --include=*.rs` returns ZERO hits at HEAD. `cyrup/crates/cyrup-resources/src/theme.rs:205-264` `REQUIRED_COLOR_TOKENS` is 51 entries including `selectedBg` but no scrollbar entry; the two built-ins (`BUILTIN_DARK_JSON`, `BUILTIN_LIGHT_JSON`) define `selectedBg` and no `scrollbarThumb`. Validation only reports MISSING required tokens (`build_theme_error`, `theme.rs:148-165`), so an extra key is accepted and discarded rather than rejected.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/theme/theme.ts`: `scrollbarThumb: Type.Optional(ColorValueSchema)` at `:50`, the ThemeBg union member at `:160`, `withThemeColorFallbacks` returning `scrollbarThumb: colors.scrollbarThumb ?? colors.selectedBg` at `:330`, the bgColors fallback at `:365`, the runtime key-list entry at `:617`.

**Impact** — a theme authored for pi that sets `scrollbarThumb` loads without complaint and the token is discarded. Dormant until CFG-021's fullscreen scrollbar ships, at which point the scrollbar renders unthemed and the omission looks like a rendering bug. Distinct from CFG-021: that item is two settings keys, this is a theme-schema token; either can land without the other.

**Fix** — land with CFG-021. Add `scrollbarThumb` as an OPTIONAL token (NOT in `REQUIRED_COLOR_TOKENS`, exactly like `thinkingMax` — see the NOTE at `theme.rs:262-264`) with a `?? selectedBg` fallback at the resolution site, and add it to both built-in JSON blobs.

**Verify** — test in `cyrup/crates/cyrup-tui/src/theme.rs` mirroring `legacy_theme_without_thinking_max_falls_back_to_xhigh` (`:1210`): a theme without `scrollbarThumb` resolves it to its own `selectedBg`; a theme declaring it keeps the declared value.

## CFG-014 — `showCacheMissNotices` and prompt-cache-miss tracking absent

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `grep -rn 'showCacheMissNotices|show_cache_miss' crates/ --include=*.rs` returns ZERO hits at HEAD; not among `EffectiveSettings`' accessors in `cyrup/crates/cyrup-config/src/settings.rs`.

**upstream** — key declared at `pi/packages/coding-agent/src/core/settings-manager.ts:99` with default false, plus the setter `setShowCacheMissNotices` at `:878-882`.

**Impact** — no way to surface prompt-cache misses; a user debugging cache behaviour has no signal.

**Fix** — add the accessor and setter in `settings.rs`, detect the miss off the usage block in `cyrup-provider`, and add the `/config` row.

**Verify** — a faux-provider run with a cache-miss usage block emits the notice when the setting is on and nothing when off.

## CFG-015 — `warnings.anthropicExtraUsage`, `markdown.codeBlockIndent`, `lastChangelogVersion`, `npmCommand` unconsumed

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — accessors exist — `code_block_indent()` at `cyrup/crates/cyrup-config/src/settings.rs:808`, `warnings()` at `:851`, `last_changelog_version()` at `:962`, `npm_command()` at `:709-711` — and grepping those names across `crates/` outside `cyrup-config/src` returns ZERO hits.

**upstream** — `pi/packages/coding-agent/src/core/settings-manager.ts:58` (`codeBlockIndent`), `:62` (`anthropicExtraUsage`), `:87` (`lastChangelogVersion`), plus `getNpmCommand`.

**Impact** — four documented settings do nothing when set.

**Fix** — wire `code_block_indent` into `cyrup-tui`'s markdown renderer, `anthropicExtraUsage` into the Anthropic provider's warning path, `lastChangelogVersion` into startup. `npm_command()` is blocked behind the unported npm channel (CFG-009) — a consequence rather than an independent gap.

**Verify** — one assertion per key at its consumption site.

## CFG-027 — A local package that is a bare extension directory contributes nothing

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-resources/src/discovery.rs:632-660` — after `resolve_configured_package` yields a `PackageTree`, the loop calls `resolve_manifest(&dir)` at `:649` and every resource comes from the returned manifest lists; nothing ever pushes `dir` itself onto `ext_paths`. `resolve_configured_package` additionally hard-errors on a non-directory at `:322-333`.

**upstream** — `pi/packages/coding-agent/src/core/package-manager.ts:1300-1328` `resolveLocalExtensionSource`: a FILE entry goes straight to `accumulator.extensions` (`:1314-1318`); a DIRECTORY calls `collectPackageResources` and then `if (!resources) { this.addResource(accumulator.extensions, resolved, metadata, true); }` (`:1321-1324`).

**Impact** — `"packages": ["./my-ext"]` where `./my-ext` is a bare extension with no manifest loads nothing. Narrow: `--extension`/`-e` and the settings `extensions` array both cover the need.

**Fix** — in `discovery.rs:632-660`, when `resolve_manifest` yields nothing, push `dir` onto `ext_paths`; relax the non-directory error at `:322-333` to accept a file entry as an extension.

**Verify** — test: a settings-declared local package that is a manifest-less extension directory registers as an extension.

## CFG-021 — `uiMode` / `fullscreenScrollbar` not modelled

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** confirmed

**cyrup** — `grep -rni 'uiMode|fullscreenScrollbar|ui_mode|fullscreen_scrollbar' crates/ --include=*.rs` returns ZERO at HEAD.

**upstream** — keys at `pi/packages/coding-agent/src/core/settings-manager.ts:132-133`. A key-by-key sweep of pi's Settings interface (`:87-133`) against cyrup's accessor surface found these two plus `showCacheMissNotices` (CFG-014) as the only entirely-unmodelled keys.

**Impact** — none today; deferred with the fullscreen TUI mode itself.

**Fix** — land with the fullscreen viewport work. The companion theme token is CFG-034.

**Verify** — settings round-trip test once the mode exists.

## Coverage

Read at HEAD `1806375`: `cyrup-config/src/{settings,auth,trust,model,models_store,provider_compose,config_value,env,lock}.rs`; `cyrup-resources/src/{discovery,theme,prompt,error}.rs` and `src/package/{manifest,source}.rs`; `crates/cyrup/src/{main,migrations}.rs`; `cyrup-session-svc/src/{builder,session}.rs` (auth/registry/package regions); `cyrup-test-support/src/auth.rs`; `cyrup-tools/src/ops/shell.rs`; tests `crates/cyrup/tests/models_json_resolution.rs` (all 154 lines), `cyrup-session-svc/tests/settings_resolve.rs`, plus the inline suites at `settings.rs:1600-2365` and `auth.rs:585-648`. Upstream read: `pi/packages/coding-agent/src/core/{settings-manager,model-resolver,model-runtime,auth-storage,trust-manager,package-manager,provider-composer,prompt-templates}.ts`, `src/migrations.ts`, `src/utils/paths.ts`, `src/modes/interactive/theme/theme.ts`, `pi/packages/ai/src/auth/oauth/anthropic.ts`.

CFG-001 was attacked rather than accepted: `with_lock` has zero callers outside `settings.rs`, the `SettingsStore` trait (`settings.rs:1022-1033`) exposes only `read` + `with_lock`, all four writer bodies were read to confirm the in-lock re-check, and a `settings.json` grep across all crates surfaced exactly one out-of-manager writer (`crates/cyrup/src/migrations.rs:91`), which pi matches at `migrations.ts:59`. The closure stands.

Test-defect hunt run independently rather than inherited. Bug-pinning shape: nothing pins CFG-016 (`${0` → no hits), CFG-017 (`${@:-`/`ARGUMENTS:-` → no hits), CFG-019 (`grok-4.20-0309-reasoning` → only the production arm), or CFG-012 (`deep_merge_precedence_and_nested`, `settings.rs:1600-1624`, depth-1 only and silent). The only bug-pinning tests in this area are CFG-029 and the new CFG-033 fixtures. Timing shape: four sleep/`Instant` hits judged, none filed — `config_value.rs:328/:344/:349` is production code (CFG-028); `resources.rs:518`'s 120 ms pre-sleep sits under a 5 s `tokio::time::timeout` on `rx.changed()`, i.e. it waits on the real outcome with ~40x margin; `cyrup-config/src/auth.rs:585-606`'s `yield_now` is inside the critical section, so a missing lock drives `max_seen` to 2 even single-threaded — a genuine mutual-exclusion property; `theme.rs:761` is a production task spawn.

Blind spots and things taken on trust: (1) nothing was compiled or executed, so no closure is observed-passing — CFG-001's assertions were reasoned about statically only. (2) Not verified that a models.json-only provider STREAMS end to end, only that it is composed, listed, selectable and resolvable. (3) The `spec/` tree behind the `R-07-*`/`R-09-*` ids is absent from this workspace; ids were used only as a grep index and no requirement text is quoted. (4) pi's `applyPatterns` / `applyAutoloadDisabledPatterns` bodies were not line-diffed — only `applyPackageDeltaFilter`'s entry condition and early return — so CFG-010's remaining-work description is at the semantics level below that point. (5) `remote_catalog.rs` / `spawn_model_catalog_refresh` (DRIFT-007) belongs to area 01 and was not audited here.

Four factual corrections were folded into pre-existing items without changing any status: CFG-020's "recomposed once per model" claim was false (`available_model_catalog` at `session.rs:2406` calls `full_model_registry()` exactly once; the cost is per-call); CFG-010's upstream mechanism is an early return on empty patterns, not an empty-start set, with cites corrected to `:2084`/`:2085`/`:2091`; CFG-002's upstream cites corrected to `:167-169`, `:181-183`, `:188`, plus the newly-noticed missing `!config.oauth` clause; CFG-019's cites corrected to `:20`, `:28`, `:46-47`. CFG-032's original "nothing later re-chmods it" rationale was refuted (`lock.rs:93-112` always renames a fresh 0600 temp) and replaced with the CFG-005-dependent permanence argument; severity unchanged.



---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at
all, rather than checking a list of known items. That inversion exists because the
item-driven method missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`)
— a real, user-reported bug — and by construction cannot see behaviour nobody wrote an item
for. IDs use an `-SNN` suffix to mark their provenance.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| CFG-S01 | high | not-ported | S | `--system-prompt` / `--append-system-prompt` never read file contents — a path becomes the literal system prompt |
| CFG-S02 | medium | not-ported | S | `images.autoResize: false` is inert — the read tool always downsamples to 2000px |
| CFG-S03 | medium | not-ported | M | Extension tool-name and flag-name conflicts are never detected — and cyrup's precedence on collision is INVERTED vs pi (last-wins, not first-wins) |
| CFG-S04 | low | not-ported | M | Four more settings keys are inert beyond CFG-015's list — `enableSkillCommands`, `treeFilterMode`, `editorPaddingX`, `showHardwareCursor` |

## CFG-S01 — `--system-prompt` / `--append-system-prompt` never read file contents — a path becomes the literal system prompt

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** confirmed

**upstream** — /home/d0m17bw/workspace/pi/packages/coding-agent/src/core/resource-loader.ts:53-68 `resolvePromptInput(input, description)` — `if (existsSync(input)) { try { return readFileSync(input, "utf-8") } catch { warn; return input } }`, else returns the literal. Applied to BOTH inputs at :526 (`const baseSystemPrompt = resolvePromptInput(systemPromptSource, "system prompt")`) and :536-538 (`appendSources.map((s) => resolvePromptInput(s, "append system prompt"))`). The same block also derives `systemPromptSourcePath` (:528-529, `existsSync(...) ? resolvePath(...) : undefined`) and `appendSystemPromptSourcePaths` (:542-544). Advertised in `--help`: /home/d0m17bw/workspace/pi/packages/coding-agent/src/cli/args.ts:261 `--append-system-prompt <text>  Append text or file contents to the system prompt (can be used multiple times)`.

**cyrup** — ABSENT. 

**Impact** — `cyrup --system-prompt ./prompts/reviewer.md` sets the model's ENTIRE system prompt to the 22-character string `./prompts/reviewer.md` — `custom_prompt` is a full replacement (prompt/builder.rs:134), so the agent runs with no tool guidance and no project framing, silently, with no diagnostic. `--append-system-prompt ./house-style.md` is the milder same failure. Both spellings come straight from pi's `--help`, so a migrating user hits it on the first command and the symptom ("model ignores my instructions") points nowhere near the cause.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## CFG-S02 — `images.autoResize: false` is inert — the read tool always downsamples to 2000px

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — /home/d0m17bw/workspace/pi/packages/coding-agent/src/core/settings-manager.ts:1149-1151 `getImageAutoResize(): boolean { return this.settings.images?.autoResize ?? true; }`. Consumers: /home/d0m17bw/workspace/pi/packages/coding-agent/src/core/agent-session.ts:2553 -> :2564 `createAllToolDefinitions(this._cwd, { read: { autoResizeImages }, ... })`, and /home/d0m17bw/workspace/pi/packages/coding-agent/src/main.ts:830 `prepareInitialMessage(parsed, settingsManager.getImageAutoResize(), stdinContent)`. Branch condition at /home/d0m17bw/workspace/pi/packages/coding-agent/src/utils/image-process.ts:86 `if (autoResizeImages) { ... resizeImage ... }` — false returns the original normalized bytes.

**cyrup** — ABSENT. 

**Impact** — A user who sets `"images": {"autoResize": false}` still gets every image downscaled to a 2000px bound before it reaches the model, with no note in the tool result. Silent wrong input on exactly the tasks where the setting exists (small text in screenshots, dense diagrams, OCR-shaped work). The `/config` toggle makes it worse than a plain unported key: the user flips it, watches it persist, and gets an identical result. Same defect on the CLI image path (`prepareInitialMessage`), where cyrup has no counterpart at all.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## CFG-S03 — Extension tool-name and flag-name conflicts are never detected — and cyrup's precedence on collision is INVERTED vs pi (last-wins, not first-wins)

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**upstream** — /home/d0m17bw/workspace/pi/packages/coding-agent/src/core/resource-loader.ts:1059-1092 `detectExtensionConflicts(extensions)` walks `ext.tools.keys()` / `ext.flags.keys()` against `toolOwners`/`flagOwners`, emitting `Tool "${toolName}" conflicts with ${existingOwner}` (:1071-1074) and `Flag "--${flagName}" conflicts with ${existingOwner}` (:1082-1085); `addExtensionConflictDiagnostics` (:626-633) pushes each into `extensionsResult.errors` — the one diagnostic source cyrup already renders. **New evidence the original claim did not have**: pi's runtime resolution is explicitly FIRST-registration-wins — /home/d0m17bw/workspace/pi/packages/coding-agent/src/core/extensions/runner.ts:450-460 `getAllRegisteredTools()` ("first registration per name wins", `if (!toolsByName.has(...))`), :463-471 `getToolDefinition` returns on the first extension holding the name, and :473-480 `getFlags()` does the same `if (!allFlags.has(name))` for flags.

**cyrup** — ABSENT. 

**Impact** — Two extensions registering a tool named `search`/`deploy`/`query` both load clean, both look healthy in `[Extension issues]`, and one silently never executes. Worse than the claim stated: pi routes the call to the FIRST-loaded extension and cyrup to the LAST-loaded one, so the same two extensions installed in the same order behave differently under pi and cyrup — a user porting a working pi setup gets a different tool silently. The shadowed extension's other tools keep working, so it reads as a bug in one tool rather than a name collision. Same on both halves for `--flag` names. pi's design is stated in code ("Keep all extensions loaded. Conflicts are reported as diagnostics, and precedence is handled by load order") and cyrup has neither half of it right.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## CFG-S04 — Four more settings keys are inert beyond CFG-015's list — `enableSkillCommands`, `treeFilterMode`, `editorPaddingX`, `showHardwareCursor`

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**upstream** — Verified every upstream consumer by grep at pi HEAD: `getEnableSkillCommands` (/home/d0m17bw/workspace/pi/packages/coding-agent/src/core/settings-manager.ts:1054) gates registration at modes/interactive/interactive-mode.ts:652 and is surfaced at :4221. `getTreeFilterMode` (settings-manager.ts:1195-1199, validated against the five-member list, falls back to `"default"`) seeds the tree at interactive-mode.ts:4725 and is surfaced at :4235. `getEditorPaddingX` (settings-manager.ts:1217-1219, clamped 0-3 on write at :1222) is read at interactive-mode.ts:507 (construction) and :1791 (live change), surfaced at :4239. `getShowHardwareCursor` (settings-manager.ts:1207-1209, `?? process.env.PI_HARDWARE_CURSOR === "1"`) is a TUI ctor arg at cli/startup-ui.ts:82 and interactive-mode.ts:490, re-applied live at :1785, surfaced at :4236; the TUI side is packages/tui/src/tui.ts:333,352-357,378,382-383.

**cyrup** — ABSENT. 

**Impact** — Four documented settings a user can flip in `/config`, watch persist to `settings.json`, and observe do nothing. `enableSkillCommands: false` still leaves every `/skill:name` registered and expanding. `treeFilterMode: "user-only"` never applies — `/tree` always opens on `default`, so the preference must be re-entered by hand every time, which is the exact repetition the key exists to remove. `editorPaddingX` does nothing. `showHardwareCursor` does nothing and cyrup's default is the opposite of pi's, which matters for IME/CJK users — and it strands cyrup's own faithful `CYRUP_HARDWARE_CURSOR`/`PI_HARDWARE_CURSOR` env-fallback port, wired end-to-end into a getter nothing calls. Low individually; aggregate matters because `/config` becomes partly a placebo surface.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

