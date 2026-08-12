# 06 — cyrup-ext (the extension host)

This area covers the extension host itself: the event catalog and dispatch reduction (`cyrup-ext/src/{event,dispatch,facade,registry}.rs`), the WIT world and its wasmtime runtime (`cyrup-ext/wit/world.wit`, `cyrup-ext/src/host/`), the native built-in extension path (`cyrup-ext/src/native.rs`), the guest SDK (`cyrup-ext-sdk/`), and the `cyrup-session-svc` and `cyrup-tui` wiring that is the only production consumer of any of it. It is measured against `pi/packages/coding-agent/src/core/extensions/` at pi v0.83.0 — `types.ts`, `runner.ts`, `loader.ts` — with the standing caveat that cyrup's WASM Component Model host is a deliberate divergence from pi's jiti/TypeScript loader, so only the *semantics* of the event and registration surfaces are in scope, not the mechanism. Headline: the extension seam went from largely inert to genuinely live — six items closed outright — and what remains is a long tail of unported event fields, unconsulted registration hooks, a registration surface that reaches only WASM guests while every extension cyrup ships is native, and one ABI-hygiene defect the live path cannot catch. Re-baselined against HEAD `1806375` on 2026-08-03; every closure below was re-derived by reading the code at HEAD, not by trusting a commit message.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| EXT-001 | **Closed** | `EventKind::fails_closed` (`cyrup-ext/src/event.rs:155-157`) → `dispatch_block_mutate` `Err` arm (`dispatch.rs:248-257`) → `hooks.rs:44` → `cyrup-agent/src/agent.rs:922-925`. Both fault funnels (native `catch_unwind` `native.rs:468-486`, wasm `map_wasm_error` `host/live.rs:1389-1410`) confirmed to yield `Err`. Converse asserted at `tests/ext_fail_closed.rs:352`. The abort-race side effect is filed separately as EXT-029 and must **not** be fixed by weakening `fails_closed`. |
| EXT-002 | **Closed** | Both `from_agent` fns return `None` for `MessageEnd` (`event.rs:175`, `event.rs:437`). One production caller of `emit_message_end`: `cyrup-session-svc/src/subscriber.rs:156`. Same-role rule intact at `contract.rs:93-97`. |
| EXT-003 | **Partially closed — still open** | The extension *vote* is live (`builder.rs:1462-1494`, invoked at `:467-471`); the trust *store* is still absent (`builder.rs:472-481` is a `tracing::warn!`, `:488` passes `saved: None`). Open item below. |
| EXT-004 | **Closed** | Dirty flag (`registry.rs:169-177`) → `refresh_tools` (`facade.rs:376-381`) → `materialize_guest_tools` (`:386-410`) → `AgentSession::refresh_extension_tools` (`session.rs:4156-4178`) → `prepare_next_turn` (`hooks.rs:170-181`). Also called from `emit_session_start` (`session.rs:2224`), so the first turn sees the tool. Residuals filed as EXT-030 and EXT-031. |
| EXT-005 | **Closed** | `ctx-state` (`wit/world.wit:504-513`), `control.abort` (`:537`), `control.shutdown` (`:541`); host impls at `host/live.rs:829-840` deliberately ungated with pi citations. Audited pi's full `ExtensionContext` (`types.ts:320-346`): `getContextUsage`/`compact`/`model`/`thinkingLevel` are all present on `models`/`control`. Two CYRUP-DELTAs should be recorded in `cyrup-ext/src/lib.rs`: `ctx.signal` is inexpressible across the component boundary, and `scopedModels` (`types.ts:325-327`) has no WIT counterpart. |
| EXT-006 | Open | Production wiring closed (`cyrup-tui/src/app.rs:3025-3078`); signature, one-shot and replay residuals survive. |
| EXT-007 | Open | Unchanged. Prompt built at `builder.rs:951`, six lines before `active_tools` at `:957`. |
| EXT-008 | Open | Unchanged; upstream evidence strengthened — pi is first-wins on execution too (`runner.ts:463-471`). |
| EXT-009 | Open | Unchanged. |
| EXT-010 | **Closed** | `EventKind::AgentSettled = 30` (`event.rs:48-53`), WIT export `world.wit:210`, SDK hook `cyrup-ext-sdk/src/api.rs:610`, emitted at `subscriber.rs:226` before the session fan-out at `:228`. Both `world.wit` copies verified byte-identical by `diff`. |
| EXT-011 | Open | Unchanged. Upstream ref corrected to `types.ts:571-575`, subscribed `:1201`. |
| EXT-012 | Open | Unchanged. |
| EXT-013 | Open | Unchanged. |
| EXT-014 | Open | Unchanged. |
| EXT-015 | Open | Sharpened — `previous_session_file` is fanned out intact at `session.rs:2210-2214` then dropped at `:2219`. |
| EXT-016 | Open | Unchanged. |
| EXT-017 | Open | Unchanged; root cause pinned to `commands: HashMap` at `registry.rs:111`. |
| EXT-018 | Open | Unchanged. Fold with EXT-034 — the same edit touches both. |
| EXT-019 | Open | Unchanged. |
| EXT-020 | **Closed** | `usage` on `HostEvent::ToolResult` (`event.rs:271-275`) ← `hooks.rs:69-79`; wire `world.wit:172`, marshalled `host/live.rs:1426-1442`; `EventPatch::ToolResult` applied `contract.rs:73-90`; outbound change-only diff `hooks.rs:100-104` matches pi's `usage: afterResult.usage ?? result.usage`. ABI/coverage residuals folded into EXT-028. |
| EXT-021 | Open | **Count corrected upward**: six → at least eight missing `ctx.ui` capabilities. |
| EXT-022 | Open | Unchanged. |
| EXT-023 | Open | Unchanged. |
| EXT-024 | Open | Unchanged. |
| EXT-025 | Open | Unchanged; drift confirmed still present (`session.rs:4007` vs `facade.rs:585`). |
| EXT-026 | Open | Unchanged; static analysis only, caveat carried. |
| EXT-027 | Open | Unchanged. |
| ~~EXT-028~~ **CLOSED** `513e45a` | Open | **Strengthened with git proof** — `world.wit` changed in f777e44, `manifest.rs` did not. Now the item of record for the deliberate f777e44 ABI break. |
| EXT-029 | Open | Unchanged; batch-wide framing confirmed wrong, confined race-window framing confirmed right. |
| EXT-030 | Open | Unchanged; exact trigger condition pinned (`changed == true` **and** a concurrent mark). |
| EXT-031 | Open | Unchanged; the deferral rationale (`session.rs:4196-4205`) read in full and judged sound. |
| EXT-032 | Open | Unchanged; redundancy argument re-derived independently. |

## Open items

> **⚠ THIS TABLE IS NOT THE COMPLETE OPEN SET.** 6 further items from the 2026-08-03
> surface-driven sweep live in their own table under `## Surface-sweep findings` (line ~514), with
> `-S` ids — **including 2 rated critical/high**. Enumerating only this table undercounts the
> area by 6 items, which is exactly how `SEAM-S01` (high) escaped a full audit pass on
> 2026-08-07. Count BOTH tables. See structural defect A in `00-residual-ledger.md`.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| EXT-028 | high | parity-bug | M | WIT world version not bumped for a re-signed export, and the build cache cannot see SDK/WIT changes |
| EXT-003 | medium | not-ported | M | Project-trust store is unwired at the extension seam |
| EXT-006 | medium | parity-bug | L | Renderers run without display options or theme, once, and never on replay |
| EXT-007 | medium | parity-bug | M | The first system prompt is built from built-ins only; guest promptGuidelines are dropped |
| EXT-008 | medium | parity-bug | S | Extension tools resolve last-registered-wins at execution while the getter is first-wins |
| EXT-009 | medium | not-ported | M | before_provider_headers event is missing entirely |
| EXT-011 | medium | not-ported | M | session_info_changed absent from the extension catalog |
| EXT-012 | medium | not-ported | M | registerEntryRenderer entirely absent |
| EXT-013 | medium | parity-bug | M | Slash-command argument completions and autocomplete providers are dead |
| EXT-014 | medium | parity-bug | M | tool_execution_update / tool_execution_end drop toolName (and args) |
| EXT-015 | medium | parity-bug | M | Session-lifecycle events lose their discriminating fields at the extension boundary |
| EXT-017 | medium | parity-bug | S | Command listing is non-deterministic, drops name:N, and a colliding command is unexecutable |
| EXT-018 | medium | parity-bug | M | The inter-extension event bus is wasm-only — natives have no pi.events |
| EXT-021 | medium | not-ported | L | ctx.ui capabilities with no WIT representation (at least eight) |
| EXT-023 | medium | parity-bug | M | prepareArguments is unreachable for WASM guest tools, and the SDK drops the field silently |
| EXT-024 | medium | parity-bug | M | renderShell/constrainedSampling unexpressible, and render_kind has zero consumers |
| EXT-029 | medium | parity-bug | S | An abort landing during a gated tool-call dispatch reports as an extension failure |
| EXT-030 | medium | parity-bug | S | materialize_guest_tools unconditionally clears the tools-dirty flag, swallowing its own re-arm |
| EXT-031 | medium | parity-bug | L | Turn-boundary refresh propagates tools but not the rebuilt system prompt |
| EXT-033 | medium | parity-bug | S | An `--extension`/`-e` path that is a FILE (or does not exist) is silently ignored |
| EXT-034 | medium | parity-bug | M | Bus events emitted from an event handler are never delivered — the drain runs only after run_command/run_shortcut |
| EXT-035 | medium | parity-bug | M | NativeExtension can register only 5 of the 10 WIT registration surfaces — no shortcuts, flags or providers |
| EXT-016 | low | parity-bug | S | resources_discover carries neither cwd nor reason |
| EXT-019 | low | not-ported | M | registerMarkdownTransformer has no counterpart |
| EXT-022 | low | not-ported | M | ProviderConfig.refreshModels is not represented |
| EXT-025 | low | cyrup-original | S | reload() and four emit_* facade methods are dead code that has drifted |
| EXT-026 | low | cyrup-original | M | A wasmtime-free cyrup-session-svc build cannot be produced |
| EXT-027 | low | not-ported | L | pi's bundled llama.cpp router extension has no counterpart |
| EXT-032 | low | test-defect | S | p3_no_human_wait_is_still_budget_contained asserts an uncontrollable wall-clock bound |
| EXT-036 | low | stale-port | S | `EventKind::COUNT`'s doc claims 1:1 parity with a 31-event pi catalog; pi has 33 — and world.wit still says 30 |

## EXT-028 — WIT world version not bumped for a re-signed export, and the build cache cannot see SDK/WIT changes

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** high (version half directly readable; cache half reasoned from composition)

**cyrup** — `cyrup/crates/cyrup-ext/src/manifest.rs:46` pins `HOST_WORLD = "cyrup:ext@0.2"`, with the bump contract at `:38-45` (bumped 0.1→0.2 for the *added* export `events.on-agent-settled`; correctly notes the `ctx-state`/`control.abort`/`control.shutdown` additions were imports and needed no bump), enforced by `check_world` at `:62-87`, which rejects `gmin < hmin` with a typed `ExtError::WorldVersion` before instantiation. f777e44 then **re-signed** an existing export — `on-tool-result` gained a trailing `usage-json: option<string>` at `cyrup/crates/cyrup-ext/wit/world.wit:172` in both copies. `git log --oneline c8bd2ab..HEAD -- crates/cyrup-ext/wit/world.wit` returns f777e44 **and** 1d87913; the same query on `crates/cyrup-ext/src/manifest.rs` returns **only** 1d87913. A guest declaring `cyrup:ext@0.2` therefore passes `check_world` (called on the real load path at `facade.rs:984` inside `load_discovered`) and then dies inside wasmtime, because `bindings::Extension::instantiate_async` (`host/live.rs:938`) resolves world exports eagerly. Second half: `cache_key(&hash_source_tree(crate_dir)?, &toolchain.id(), HOST_WORLD)` at `cyrup/crates/cyrup-ext/src/build/mod.rs:39-42` *does* include `HOST_WORLD` — which is precisely why a missed bump poisons the cache as well as the gate — and `hash_source_tree` (`build/cache.rs:32-46`) walks only the extension crate directory, so a guest linking `cyrup-ext-sdk` from outside that tree gets a stale artifact. Third drift: the package line is still `package cyrup:ext@0.1.0;` at `world.wit:10`, and the header at `world.wit:8` claims a "30-event catalog". Nothing catches any of it — `cyrup/crates/cyrup-ext/tests/wit_world_sync.rs` only compares the two on-disk copies to each other (verified byte-identical by `diff`, so it passes and proves nothing about versions). Aggravating: `cyrup/crates/cyrup-ext/src/loader.rs:181` synthesizes `world: HOST_WORLD` for a manifest-less discovered `.wasm`, making the gate toothless for prebuilt artifacts.

**upstream** — no direct analog; pi has no compiled ABI (`pi/packages/coding-agent/src/core/extensions/loader.ts` loads TypeScript via jiti). The invariant being violated is cyrup's own, stated at `manifest.rs:38-45`, and the failure mode it exists to prevent is asserted for an *added* export by `cyrup/crates/cyrup-ext/tests/manifest_cache.rs:44-59`.

**Impact** — a component built against the pre-f777e44 world declares a version the host accepts and then fails to instantiate with a raw wasmtime link error instead of the typed `ExtError::WorldVersion` the gate exists to produce. Cached artifacts compound it: an SDK or WIT edit does not invalidate the cache key, so a rebuild can silently serve a component built against the old world.

**Fix** — bump `HOST_WORLD` to `cyrup:ext@0.3` and restate the rule at `manifest.rs:38-45` as "any change to an EXPORT — added, removed, or **re-signed** — bumps the minor". Fold the SDK tree and both `world.wit` files into `cache_key` (`build/mod.rs:39-42`). Bring `world.wit:1,:8,:10` in step in both copies. Extend `tests/wit_world_sync.rs` to tie `HOST_WORLD` to the package line. Update fixture manifests (`tests/discover_load.rs:57`, `tests/loader.rs:24`, `tests/manifest_cache.rs:13`). Separately add live-component coverage for `usage-json`: `cyrup/crates/cyrup-ext-sdk/src/example.rs` registers only `on_tool_call` (`:59`) and `on_agent_settled` (`:103`), so the widened export has never crossed a real boundary.

**Verify** — build the SDK demo guest declaring `@0.2` and confirm `check_world` rejects it with `ExtError::WorldVersion` before wasmtime is reached; touch a `world.wit` line and confirm `cache_key` changes; add an `on_tool_result` handler to `example.rs` and assert a non-null `usage-json` arrives across the live boundary.

## EXT-003 — Project-trust store is unwired at the extension seam

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — the vote is live: `pre_trust_extension_verdict` at `cyrup/crates/cyrup-session-svc/src/builder.rs:1462-1494` (natives filtered on `decides_project_trust()` at `:1474` with an idempotence rationale at `:1452-1461`; `discover_and_load(&roots, false, DenyServices)` at `:1487`; `host.aggregate_project_trust` at `:1490`), invoked at `builder.rs:467-471` behind the `shouldResolveProjectTrust` guard (`cfg.trust_override.is_none() && has_resources`) and fed to `decide_trust_with_extension` at `:484-494`. The store is absent: `builder.rs:472-481` is a bare `tracing::warn!` whose own text admits "no trust store is wired into the session builder — the verdict applies to this session only", and `builder.rs:488` passes `saved: None` literally.

**upstream** — `pi/packages/coding-agent/src/core/project-trust.ts:63-65` persists (`options.trustStore.set(options.cwd, trusted)` guarded by `result.remember === true`) and `:71-74` reads back (`const decision = options.trustStore.get(options.cwd); if (decision !== null) return decision;`).

**Impact** — a directory the user already trusted re-prompts on every launch; "remember" is accepted and discarded. Additionally `builder.rs:1470` is `ExtensionHost::with_wasm(host_config).ok()?` sitting *before* the native-loading loop, so a wasm-runtime construction failure returns `None` and silently discards the native votes too.

**Fix** — persist `remember` at `builder.rs:472-481` and read the store, passing the hit as `saved:` at `:488`. Change `builder.rs:1470` to fall back to `ExtensionHost::new(host_config)`, which the `not(wasm-host)` arm at `:1472` already does.

**Verify** — trust a directory with remember set, restart, assert no prompt and that `decide_trust_with_extension` receives `saved: Some(true)`. Force `with_wasm` to fail and assert native votes still aggregate.

## EXT-006 — Renderers run without display options or theme, once, and never on replay

**Kind** parity-bug · **Severity** medium · **Effort** L · **Confidence** high

**cyrup** — production wiring is live: `extension_render` at `cyrup/crates/cyrup-tui/src/app.rs:3025-3078` (sync `has_message_renderer`/`has_tool_renderer` pre-check, spawn, `EXTENSION_RENDER_TIMEOUT` race, `abort()` on a wedged task rather than detaching it), called from `ingest_event_with_extensions` at `app.rs:2555-2561`, reached from the run loop at `app.rs:4124`; routing at `cyrup/crates/cyrup-ext/src/facade.rs:674-705`, owner tables at `registry.rs:191-197` and `:240-243`, fault containment in `render_via` at `facade.rs:716-758`. Three residuals: (1) signature — `render-call: func(custom-type: string, call-json: string) -> option<string>` at `cyrup/crates/cyrup-ext/wit/world.wit:161-162` carries no display options and no theme; (2) one-shot — `app.rs:2560` computes `rendered` once at ingest and hands it to `ingest_event_rendered` (`app.rs:2564`), baking the text so an expansion or theme change never re-runs the renderer; (3) replay — `replay_session` at `app.rs:759` is a sync walk whose own doc at `app.rs:754-758` calls the renderer bypass a "Known gap".

**upstream** — `MessageRenderer = (message, options, theme) => Component | undefined` at `pi/packages/coding-agent/src/core/extensions/types.ts:1154-1158`, with `EntryRenderOptions {expanded}` at `:1150-1152`.

**Impact** — an extension renderer cannot respond to expand/collapse or to the active theme, and a resumed session draws the built-in `[type] body` framing where the live session drew the extension's output.

**Fix** — widen `render-call`/`render-result` in both `world.wit` copies to carry display options + theme (ABI break ⇒ bump `HOST_WORLD`, EXT-028); move the render out of ingest into the draw path, or cache keyed by `(entry_id, expanded, theme)`; give `replay_session` (`app.rs:759`) the async extension pass.

**Verify** — toggle expansion and theme on a rendered entry and assert the renderer is re-invoked with the new options; resume a session containing a custom-rendered message and assert the extension output, not the built-in framing.

## EXT-007 — The first system prompt is built from built-ins only; guest promptGuidelines are dropped

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/builder.rs:934-937` derives `selected_tools` and `tool_contributions` from `base_tools`; `PromptInputs` is assembled at `:939-950` and the prompt built at `:951` (`SystemPromptBuilder::new().build(&prompt_inputs)`); `ext_host.active_tools(&base_tools)` runs six lines later at `:957` — its own comment there notes it was "moved ahead of the registry", which is true but still leaves it behind the prompt. Because `cyrup/crates/cyrup-session/src/prompt/builder.rs` gates each contribution on `is_selected`, an init-registered extension tool contributes nothing to the first prompt and an extension overriding a built-in gets the built-in's snippet. `builder.rs:943` also passes `prompt_guidelines: Vec::new()` outright. Guidelines do cross the WIT (`world.wit:38`) and are stored (`host/live.rs:84` writes them into the `ToolDescriptor`, `registry.rs:14-30`), but `impl Tool for WasmTool` at `host/live.rs:1341-1375` has no `prompt_guidelines` override, so `tool_contribution` (`builder.rs:1428`, `tool.prompt_guidelines().iter().copied()`) reads the trait default. Root cause is the signature `fn prompt_guidelines(&self) -> &[&str]` at `cyrup/crates/cyrup-core/src/tool.rs:120` against a descriptor owning `Vec<String>`.

**upstream** — pi builds its prompt maps from the merged registry at `pi/packages/coding-agent/src/core/agent-session.ts:2489-2500`.

**Impact** — extension tools are callable but undescribed in the system prompt of the session's first turn, and a guest's `promptGuidelines` never reach the model at all.

**Fix** — move `ext_host.active_tools(&base_tools)` above `builder.rs:934` and derive `selected_tools`/`tool_contributions` from the merged set; widen `Tool::prompt_guidelines` (`cyrup-core/src/tool.rs:120`) to an owned/`Cow` slice and implement the override on `WasmTool` at `host/live.rs:1341-1375`.

**Verify** — an init-registered guest tool with a snippet and guidelines: assert both appear in the first assembled system prompt.

## EXT-008 — Extension tools resolve last-registered-wins at execution while the getter is first-wins

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `register_tool` at `cyrup/crates/cyrup-ext/src/registry.rs:155-166`: `if !g.tools.contains_key(&name) { g.tool_order.push(name.clone()) }` at `:158-160`, then unconditionally `g.tools.insert(name, tool)` at `:162`. `register_guest_tool` (`:182-204`) does the same at `:201`. The getter `all_registered_tool_names` (`:250-260`) walks first-insert `tool_order`, and the divergence is admitted verbatim in its own doc at `:243-248`: "(Execution still resolves last-wins via the `tools` map; only the *getter* is first-wins.)" Execution reads the last-wins map: `ExtensionRegistry::tool` (`:482-484`) and `active_tools` (`:527-547`). Second half: `registry.rs:198` does `g.tool_renderer_owner.remove(&name)` whenever a later `register_guest_tool` declares `has_renderer: false`, stripping the first extension's renderer off a tool that still executes as the first extension's.

**upstream** — pi is first-wins on **both** sides: `getAllRegisteredTools` guards with `if (!toolsByName.has(tool.definition.name))` at `pi/packages/coding-agent/src/core/extensions/runner.ts:450-461`, and `getToolDefinition` at `:463-471` iterates `this.extensions` in load order and returns the first match. That map feeds the executable registry at `pi/packages/coding-agent/src/core/agent-session.ts:2463-2488`.

**Impact** — with two extensions declaring the same tool name, the listing shows the first and the model runs the second. Order-dependent, silent, and diagnosable only by reading the registry.

**Fix** — make both registrars first-wins (`if !g.tools.contains_key(&name)` around the insert) and guard `registry.rs:198` so a later `has_renderer:false` cannot clear an earlier owner. Note the normal wasm load path is *accidentally* first-wins because `materialize_guest_tools` skips a name that already has an executable handle (`facade.rs:387-389`) and `load_wasm` materializes after each load (`facade.rs:943`); it reverts to last-wins when two descriptors land between refreshes, and native `register_tool` is unconditionally last-wins. Also clean the comment at `cyrup/crates/cyrup-ext/tests/aggregation.rs:189`, which normalizes "last-wins for execution" as if intended — it is evidence for this item, not a decision.

**Verify** — two extensions registering `deploy`: assert the listing and the executed implementation are both the first-loaded one, for natives and guests alike.

## EXT-009 — before_provider_headers event is missing entirely

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/src/event.rs:13-53` lists 31 `EventKind` variants with `COUNT: u8 = 31` at `:59`; the provider pair is `BeforeProviderRequest`/`AfterProviderResponse` and nothing else (full `name()` match at `:102-134`). Grep for `BeforeProviderHeaders`/`before_provider_headers`/`before-provider-headers` across `crates/` returns nothing at HEAD.

**upstream** — `BeforeProviderHeadersEvent` declared at `pi/packages/coding-agent/src/core/extensions/types.ts:686-689`, subscribed at `:1220`; its doc at `:682-684` specifies that a `null` value **deletes** that header. Diffing pi's 33 events against cyrup's 31 leaves exactly this and EXT-011.

**Impact** — extensions cannot add, override or delete provider request headers. Any auth-shim, proxy-tagging or telemetry extension that works under pi is impossible under cyrup.

**Fix** — add `EventKind::BeforeProviderHeaders = 31` (bump `COUNT` at `event.rs:59`, add the `from_u8` and `name()` arms), a guest export in `interface events` in both `world.wit` copies with `option<string>` values so `none` means delete, an `EventPatch::Headers` in `contract.rs`, SDK hooks, and a dispatch point in provider request assembly. Guest export ⇒ bump `HOST_WORLD` (EXT-028).

**Verify** — a guest that adds one header and nulls another; assert the outbound request carries the addition and lacks the deletion.

## EXT-011 — session_info_changed absent from the extension catalog

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — no `SessionInfoChanged` among the 31 variants at `cyrup/crates/cyrup-ext/src/event.rs:13-53`; the WIT session block (`wit/world.wit:225-233`) has `on-session-start`/`on-session-shutdown`/`on-resources-discover`/`on-project-trust` and no counterpart. The layer below exists and fires: `AgentSessionEvent::SessionInfoChanged` at `cyrup/crates/cyrup-session-svc/src/event.rs:243-264`, so the signal is available and simply never crosses the extension boundary.

**upstream** — `SessionInfoChangedEvent` at `pi/packages/coding-agent/src/core/extensions/types.ts:571-575` (`name: string | undefined` at `:574`), subscribed at `:1201`.

**Impact** — extensions cannot react to a session being renamed or its metadata changing; a status-line or external-sync extension goes stale.

**Fix** — add the kind (bump `COUNT`/`from_u8`/`name()`), a WIT export `on-session-info-changed: func(name: option<string>)` in both copies, an SDK hook, and dispatch it beside the existing `AgentSessionEvent::SessionInfoChanged` emit in `cyrup-session-svc/src/session.rs`. Guest export ⇒ bump `HOST_WORLD` (EXT-028).

**Verify** — rename a session; assert a subscribed guest receives the event with the new name.

## EXT-012 — registerEntryRenderer entirely absent

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — grep for `entry_renderer|entryrenderer|register-entry|render-entry` across `crates/` returns zero hits at HEAD. `interface registration` (`cyrup/crates/cyrup-ext/wit/world.wit:262-278`) has `register-message-renderer: func(custom-type: string);` at `:271` and no entry counterpart; `interface events` has no `render-entry`. Meanwhile `session.append-entry` still exists at `world.wit:337`.

**upstream** — `registerEntryRenderer<T>(customType: string, renderer: EntryRenderer<T>): void;` at `pi/packages/coding-agent/src/core/extensions/types.ts:1290` (doc at `:1289`: "Custom entries do not participate in LLM context"), `EntryRenderer` at `:1160-1164`, `EntryRenderOptions {expanded}` at `:1150-1152`, resolved by `getEntryRenderer` at `pi/packages/coding-agent/src/core/extensions/runner.ts:593-600`.

**Impact** — a guest can create custom transcript entries it has no way to render; they draw as the built-in fallback.

**Fix** — mirror the message-renderer trio: owner map in `registry.rs` (template at `:240-243`), `render_entry` on the facade (template at `facade.rs:696-705`), and a TUI consumer via `extension_render` (`cyrup-tui/src/app.rs:3025`). Must also work on the replay path, which bypasses renderers entirely today (`app.rs:754-759`, EXT-006).

**Verify** — a guest appending a custom entry and registering its renderer; assert the extension output both live and after resume.

## EXT-013 — Slash-command argument completions and autocomplete providers are dead

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `ExtensionHost::command_completions` at `cyrup/crates/cyrup-ext/src/facade.rs:1016-1024` has exactly one caller workspace-wide: `cyrup/crates/cyrup-ext/tests/discover_load.rs:94`. `LiveExtension::autocomplete_suggest` at `cyrup/crates/cyrup-ext/src/host/live.rs:1282-1298` has exactly one non-SDK caller: `cyrup/crates/cyrup-ext/tests/wasm_provider.rs:115` (the remaining hits are the SDK's own guest-side plumbing at `cyrup-ext-sdk/src/{macros.rs:120,api.rs:816,guest.rs:307}` and its unit test). Registration is record-only (`add-autocomplete` `world.wit:272`, `add-autocomplete-provider` `:275`, pushed by `cyrup-ext-sdk/src/guest.rs:103-109`). Nothing under `crates/cyrup-tui/` calls either.

**upstream** — `getArgumentCompletions?: (argumentPrefix: string) => AutocompleteItem[] | null | Promise<…>` on `RegisteredCommand` at `pi/packages/coding-agent/src/core/extensions/types.ts:1174`, and `addAutocompleteProvider` at `:225`; both are consulted by the interactive editor.

**Impact** — an extension can declare completions that never appear. The registration API succeeds and the feature is inert.

**Fix** — call `command_completions` for the argument position from the TUI completion path and fold `autocomplete_suggest` results in load order, reusing the off-thread pattern at `cyrup-tui/src/app.rs:3025-3078` (sync pre-check, spawn, timeout, abort). Route command resolution through `ExtensionRegistry::resolved_command_owner` (`registry.rs:381`) — itself production-dead, see EXT-017.

**Verify** — type `/deploy <tab>` with a guest declaring argument completions; assert its items appear, and that a registered autocomplete provider contributes to plain-text completion.

## EXT-014 — tool_execution_update / tool_execution_end drop toolName (and args)

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/src/event.rs:304-305`: `ToolExecUpdate { call_id, chunk }` and `ToolExecEnd { call_id, result, is_error }`. `HostEvent::from_agent` discards the rest with `..` — `AgentEvent::ToolExecutionUpdate { tool_call_id, partial_result, .. }` at `event.rs:445` and `AgentEvent::ToolExecutionEnd { tool_call_id, result, is_error, .. }` at `:451` — while `ToolExecStart` immediately above at `:438-444` destructures and keeps `tool_name` and `args`. The source events demonstrably carry the fields and cyrup throws them away. WIT signatures match the loss at `wit/world.wit:222-223`.

**upstream** — `ToolExecutionUpdateEvent { type, toolCallId, toolName, args, partialResult }` at `pi/packages/coding-agent/src/core/extensions/types.ts:770-776`; `ToolExecutionEndEvent { type, toolCallId, toolName, result, isError }` at `:779-785`.

**Impact** — an extension observing tool execution must maintain its own `callId → toolName` map from `tool_execution_start`, and cannot filter by tool at all if it missed the start (late registration, reload).

**Fix** — add `tool_name` to both variants and `args` to `ToolExecUpdate` (`event.rs:304-305`), stop discarding at `event.rs:445`/`:451`, widen the two WIT exports in both copies. ABI break ⇒ bump `HOST_WORLD` (EXT-028).

**Verify** — a guest subscribed only to `tool_execution_end` asserts a non-empty `tool_name`.

## EXT-015 — Session-lifecycle events lose their discriminating fields at the extension boundary

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `AgentSessionEvent::SessionStart` carries `previous_session_file` (`cyrup/crates/cyrup-session-svc/src/event.rs:267-270`); `emit_session_start(&self, reason: &str, previous_session_file: Option<String>)` at `cyrup/crates/cyrup-session-svc/src/session.rs:2206` fans it out intact at `:2210-2214` — and then `:2219` dispatches `&HostEvent::SessionStart { reason: reason.to_string() }` only, one statement before it would cross. The `HostEvent` arms are unchanged: `cyrup/crates/cyrup-ext/src/event.rs:307-308` (`SessionStart { reason }` / `SessionShutdown { reason }`) and `:332-333` (`SessionBeforeSwitch { target_id }` / `SessionBeforeFork { entry_id }`); WIT at `wit/world.wit:226-227` and `:232-233`.

**upstream** — `SessionStartEvent` at `pi/packages/coding-agent/src/core/extensions/types.ts:562-568` with `previousSessionFile?: string` at `:567`; `SessionBeforeSwitchEvent { reason: 'new'|'resume', targetSessionFile? }` at `:578-582`; `SessionBeforeForkEvent { entryId, position: 'before'|'at' }` at `:585-589`; `SessionShutdownEvent { reason, targetSessionFile? }` at `:616-621`.

**Impact** — an extension cannot tell a fresh session from a resume, cannot find the previous session file, and cannot tell a fork *before* an entry from a fork *at* it. Session-lifecycle extensions are limited to a coarse "something happened".

**Fix** — widen the four `HostEvent` variants and their WIT exports in both copies; pass `previous_session_file` at `session.rs:2219`; add pi's `reason` alongside `target_id` in `cyrup/crates/cyrup-session-svc/src/runtime.rs` (the new-session case is papered over with `target_id: String::new()`). ABI break ⇒ bump `HOST_WORLD` (EXT-028).

**Verify** — resume a session and assert the guest sees the previous session file; fork before and at an entry and assert the two are distinguishable.

## EXT-017 — Command listing is non-deterministic, drops name:N, and a colliding command is unexecutable

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `commands: HashMap<String, (ExtensionId, CommandDescriptor)>` at `cyrup/crates/cyrup-ext/src/registry.rs:111`, so `command_descriptions` at `:502-510`, which iterates `.commands.iter()` directly, yields random order and one entry per name. Its only production consumer is `slash_command_catalog` at `cyrup/crates/cyrup-session-svc/src/session.rs:2072-2091`, emitting the bare `name` at `:2081`. The correct implementations exist and are production-dead: `resolved_commands` (`registry.rs:337`) and `resolved_command_owner` (`:381`), reached only from `cyrup/crates/cyrup-ext/tests/aggregation.rs:216-239`, which proves `deploy:1`/`deploy:2` in load order and owner round-trip. Execution shares the flaw: `ExtensionHost::run_command` (`facade.rs:999-1013`) → `live_for_command` (`:1026-1038`) → `ExtensionRegistry::command_owner` (`registry.rs:491-493`), a raw-name lookup, so a second `deploy` is unreachable.

**upstream** — `resolveRegisteredCommands` at `pi/packages/coding-agent/src/core/extensions/runner.ts:603-635` assigns `name:N` in load order with a `takenInvocationNames` collision loop, exposed by `getRegisteredCommands` at `:642-645` and matched by `invocationName` in `getCommand` at `:653-655`.

**Impact** — the slash-command palette reorders between runs, and when two extensions register `deploy` only one is ever listed and only one is ever executable; the other is silently unreachable.

**Fix** — swap `session.rs:2081` to `resolved_commands()` emitting `r.invocation_name`; change `live_for_command` (`facade.rs:1026-1038`) to consult `resolved_command_owner` so `deploy:2` dispatches. `tests/aggregation.rs:216-239` already proves the correct behavior — it is simply unreachable from production.

**Verify** — two extensions registering `deploy`: assert stable load-order listing showing `deploy:1`/`deploy:2` and that invoking each reaches its own owner.

## EXT-018 — The inter-extension event bus is wasm-only — natives have no pi.events

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `ExtensionHost.bus: Arc<crate::host::SharedBus>` is `#[cfg(feature = "wasm-host")]` at `cyrup/crates/cyrup-ext/src/facade.rs:137-138`, constructed at `:171-172`. `deliver_bus_events` (`:848-873`) is cfg-gated and resolves subscribers only out of `self.live` (`:860`), the wasm instance map. `bus.emit`/`bus.subscribe` exist only as WIT imports (`wit/world.wit:449-459`). `grep -c bus crates/cyrup-ext/src/native.rs` is zero; `InitApi` (`native.rs:227-233`) has no bus surface — its five fields are `subs`/`tools`/`commands`/`tool_renderers`/`message_renderers`. All three cyrup-shipped extensions (permission-system, intercom, subagents) are `NativeExtension`s.

**upstream** — pi attaches the one shared bus to every extension regardless of kind: `pi/packages/coding-agent/src/core/extensions/loader.ts:398` (`events: eventBus,` on the returned `ExtensionAPI`), bus impl at `pi/packages/coding-agent/src/core/event-bus.ts:12-32`.

**Impact** — the documented cross-extension coordination channel reaches nothing that ships. The permission system, intercom and subagents extensions cannot signal each other through it.

**Fix** — move `bus` out of the cfg gate, give `NativeExtension` an `on_bus_event` entry point mirroring the renderer registration path added by 1d87913 (`native.rs:262-278`), and extend `deliver_bus_events` to fan out to `self.native` as well as `self.live`. Fold with EXT-034 — the same edit touches both.

**Verify** — a native emitting on `demo:bus` and a second native subscribed to it; assert delivery in a build with and without `wasm-host`.

## EXT-021 — ctx.ui capabilities with no WIT representation (at least eight)

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** high on the WIT surface; medium on completeness (see caveat)

**cyrup** — `interface ui` at `cyrup/crates/cyrup-ext/wit/world.wit:283-329` offers exactly `notify`/`set-status`/`abort-signal`/`confirm`/`input`/`select`/`editor`/`set-widget`/`set-header`/`set-footer`/`set-title`/`custom`/`get-editor-text`/`set-editor-text`/`paste-editor-text`/`theme-get`/`theme-list`/`theme-set`/`working-start`/`working-stop`/`get-tools-expanded`/`set-tools-expanded`, enumerated function by function. It collapses pi's three working-indicator controls into `working-start(label)`/`working-stop()` at `:324-325`.

**upstream** — absent from the WIT: `onTerminalInput` (`pi/packages/coding-agent/src/core/extensions/types.ts:144`), `setWorkingVisible` (`:154`), `setWorkingIndicator(options?: WorkingIndicatorOptions)` (`:164`), `setHiddenThinkingLabel` (`:167`), `setEditorComponent` (`:260`), `getEditorComponent` (`:263`), `getAllThemes(): {name, path|undefined}[]` (`:269` — cyrup's `theme-list` at `world.wit:321` returns bare names), `getTheme(name): Theme | undefined` (`:272`, by-name lookup without switching).

**Impact** — extensions cannot observe raw terminal input, cannot replace or read the editor component, cannot control working-indicator visibility independently of the label, and cannot enumerate themes with their paths or inspect a theme without switching to it.

**Fix** — the cheap half is all *imports* and needs no `HOST_WORLD` bump: add `set-hidden-thinking-label`, `set-working-indicator`, `set-working-visible`, `theme-get-by-name`, and widen `theme-list` (`world.wit:321`) to a `{name, path}` record. The expensive half (`onTerminalInput`, `setEditorComponent`/`getEditorComponent`) needs guest exports and a component-safe renderable-editor representation; if out of scope, record it as an explicit CYRUP-DELTA in `cyrup/crates/cyrup-ext/src/lib.rs`.

**Verify** — a guest calling each new import and asserting the observable TUI effect; for the deltas, assert the lib.rs note exists so they are not re-found as gaps.

*Caveat*: the WIT surface was verified exhaustively and the eight pi declarations line-exactly, but `cyrup/crates/cyrup-ext/src/host/services.rs` (~1885 lines) was not audited method-by-method, so a capability could be implemented host-side while missing from the WIT.

## EXT-023 — prepareArguments is unreachable for WASM guest tools, and the SDK drops the field silently

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — the host mechanism exists and works for natives: `Tool::prepare_arguments` at `cyrup/crates/cyrup-core/src/tool.rs:134` (identity default), forwarded by `RegisteredTool` at `cyrup/crates/cyrup-ext/src/wrapper.rs:113-115`, invoked before validation at `cyrup/crates/cyrup-agent/src/agent.rs:893` (immediately preceding `validate_tool_call(tool.parameters(), prepared)` at `:897`); `cyrup/crates/cyrup-tools/src/tools/edit.rs:132` is a live user. The WASM boundary cannot express it: `grep -n prepare crates/cyrup-ext/wit/world.wit` returns nothing; `ToolDescriptor` (`registry.rs:14-30`) has no flag; `WasmTool` (`host/live.rs:1341-1375`) has no override. The SDK accepts and discards it: `pub prepare_arguments: bool` at `cyrup/crates/cyrup-ext-sdk/src/descriptor.rs:45-49`, documented as "the host coerces args before validation when set", defaulted at `:66` — and `lower_tool_descriptor` at `cyrup/crates/cyrup-ext-sdk/src/guest.rs:54-69` copies name/label/description/parameters_json/exec_mode/prompt_snippet/prompt_guidelines/has_renderer and **not** `prepare_arguments`. Struct-literal construction of a different type means no compile error and no warning.

**upstream** — `prepareArguments` on `ToolDefinition` at `pi/packages/coding-agent/src/core/extensions/types.ts:468`, run before `validateToolArguments` at `pi/packages/agent/src/agent-loop.ts:548-560,578-579`.

**Impact** — a guest tool that needs argument coercion (the most common reason a tool call fails validation) sets a documented SDK field that does nothing, with no diagnostic anywhere.

**Fix** — add `prepare-arguments: bool` to the WIT `tool-descriptor` and a `prepare-arguments: func(args-json: string) -> option<string>` guest export in both copies; carry the flag on `registry.rs:14-30`; implement `Tool::prepare_arguments` on `WasmTool`. Copy the field in `lower_tool_descriptor` (`guest.rs:54-69`) and add a compile-time exhaustiveness guard there so future fields cannot be dropped silently. Guest export ⇒ bump `HOST_WORLD` (EXT-028).

**Verify** — a guest tool declaring `prepare_arguments` that coerces a string to a number; assert the call validates and executes.

## EXT-024 — renderShell/constrainedSampling unexpressible, and render_kind has zero consumers

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `grep -rn 'constrained_sampling|constrainedSampling' crates/` is empty at HEAD. For `renderShell`: the WIT `tool-descriptor` record (`cyrup/crates/cyrup-ext/wit/world.wit:31-40`) and `ToolDescriptor` (`registry.rs:14-30`) carry no field, while cyrup-core models it host-side as `ToolRenderKind` (`cyrup/crates/cyrup-core/src/tool.rs:67`) / `Tool::render_kind` (`:127-129`), forwarded by `cyrup/crates/cyrup-ext/src/wrapper.rs:110-112`. Every occurrence workspace-wide is definition, default, re-export, passthrough or unit test (`cyrup-core/src/tool.rs:67,126-128,198`, `cyrup-core/src/lib.rs:33`, `cyrup-ext/src/wrapper.rs:24,110-111,287`) — **zero consumers** in `cyrup-tui` or `cyrup-agent`, so even a native tool declaring `SelfRendered` is ignored by the TUI. The SDK drops its half too: `RenderShell` at `cyrup/crates/cyrup-ext-sdk/src/descriptor.rs:16-24`, field `render_shell` at `:42-44`, defaulted at `:65`, never copied by `lower_tool_descriptor` (`guest.rs:54-69`).

**upstream** — `constrainedSampling` at `pi/packages/coding-agent/src/core/extensions/types.ts:463`, `renderShell` at `:465`.

**Impact** — a self-rendering tool still draws the default row chrome; the model's structured-output constraint hook does not exist.

**Fix** — three pieces for `renderShell`: the WIT/`registry.rs` field, the copy in `lower_tool_descriptor`, and — the part that matters most — a TUI consumer of `render_kind` in the tool-row draw path, now feasible since renderers are live end-to-end (`cyrup-tui/src/app.rs:3025`). `constrainedSampling` needs provider-side request assembly.

**Verify** — a native tool declaring `SelfRendered`: assert the TUI omits the default shell.

## EXT-029 — An abort landing during a gated tool-call dispatch reports as an extension failure

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-agent/src/agent.rs:900-902` pre-checks `if self.cancel.is_cancelled() { return Prep::Immediate(… "Operation aborted") }` before `before_tool_call` at `:917` and re-checks at `:927-929`, so every not-yet-dispatched call in a batch reports correctly. The defect is the race window: the token handed to the hook is a *child* of the run's (`self.hooks.before_tool_call(ctx, self.cancel.child()).await`, `agent.rs:917`), so a run abort cancels it mid-flight; the biased `tokio::select!` then returns `Err(ExtError::Cancelled)` — native at `cyrup/crates/cyrup-ext/src/native.rs:477-481`, wasm identically at `cyrup/crates/cyrup-ext/src/host/live.rs:1401-1405`. `ExtError::Cancelled` renders as `"cancelled"` (`cyrup/crates/cyrup-ext/src/error.rs:8-9`) and is not excluded by `dispatch_block_mutate`, whose `Err(e)` arm at `dispatch.rs:248-255` reports it to every `onError` listener and returns `Reduced::Blocked { reason: Some("Extension failed, blocking execution: cancelled") }`; `hooks.rs:44` maps that to `Block` and `agent.rs:922-925` persists it as the tool-result text the model sees.

**upstream** — pi has no path by which an abort becomes an extension fault: `emitToolCall` (`pi/packages/coding-agent/src/core/extensions/runner.ts:932-953`) awaits handlers with no cancellation race, and the "Extension failed, blocking execution: …" text is produced only for a genuine throw (`pi/packages/coding-agent/src/core/agent-session.ts:475-487`).

**Impact** — pressing Esc during a tool call writes a transcript entry blaming a healthy extension, and fires every `onError` listener. Reachable whenever an extension subscribes to `tool_call` — the normal state once `cyrup-permissions.jsonc` exists.

**Fix** — in `dispatch_block_mutate` (`dispatch.rs:248`), special-case `ExtError::Cancelled`: do not `report` it and do not synthesize a block reason — return `Blocked { reason: None }` so the agent's own "Operation aborted" text wins; or short-circuit to `Proceed` in `hooks.rs:33-53` when `cancel.is_cancelled()`. **Do not weaken `fails_closed`** — that reopens EXT-001.

**Verify** — abort mid-`before_tool_call` with a subscribed extension; assert the result text is "Operation aborted", that no `onError` fired, and that `tests/ext_fail_closed.rs` still passes unchanged.

## EXT-030 — materialize_guest_tools unconditionally clears the tools-dirty flag, swallowing its own re-arm

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — the not-live branch deliberately re-arms at `cyrup/crates/cyrup-ext/src/facade.rs:390-396` ("Re-arm so the next refresh retries once it is live"), but the tail at `:403-405` is `if changed { self.registry.take_tools_dirty(); }`, and `take_tools_dirty` is a wholesale `swap(false, AcqRel)` (`registry.rs:175-177`) that clears that re-arm along with any mark raised meanwhile by `register_late_tool` (`facade.rs:423-430` → `registry.rs:163`) or a guest `registration.register-tool` import. Trigger condition: at least one tool materialized (`changed == true`) **and** another was skipped or arrived concurrently; when nothing materialized the re-arm survives. `refresh_tools` short-circuits on `!take_tools_dirty()` at `facade.rs:377-379`, so a swallowed mark means the skipped descriptor is never retried and `refresh_extension_tools` returns early on `Ok(false)` (`cyrup-session-svc/src/session.rs:4157-4159`). `load_wasm` calls `materialize_guest_tools` directly at `facade.rs:943`, so the clearing happens on every extension load.

**upstream** — pi has no dirty flag: `registerTool` ends with `runtime.refreshTools()` on every registration (`pi/packages/coding-agent/src/core/extensions/loader.ts:249-256`) and `_refreshToolRegistry` rebuilds the whole registry each time (`pi/packages/coding-agent/src/core/agent-session.ts:2452-2546`), so no signal can be lost.

**Impact** — a tool registered concurrently with a materialization pass, or one whose owner was not yet live, is dropped permanently for the session. Nondeterministic and load-order dependent.

**Fix** — drop the `take_tools_dirty()` at `facade.rs:403-405` and instead stop the materializer's own re-registrations from raising the flag — a `register_tool_quiet` on `ExtensionRegistry`, or a `mark_dirty: bool` parameter on `register_tool` (`registry.rs:155-166`). `refresh_tools` already takes the flag once at entry (`facade.rs:377`), which is the correct scoping.

**Verify** — two guests where one registers a tool while the other's descriptors materialize; assert both tools appear after a single `refresh_tools`, and that a descriptor whose owner is not yet live is retried on the next refresh.

## EXT-031 — Turn-boundary refresh propagates tools but not the rebuilt system prompt

**Kind** parity-bug · **Severity** medium · **Effort** L · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:4180-4216`: `next_turn_tools` (`:4206`) drains the EXT-004 refresh at `:4208` and the pending `setActiveTools` push, then returns `self.agent.tools().await` at `:4216` — the array only, with the pending push's rebuilt prompt explicitly discarded at `:4211-4212` (`if let Some((tools, _rebuilt_prompt)) = …`). The divergence and its reasoning are stated at `:4196-4205`. `TurnUpdate` has the slot (`pub system_prompt: Option<String>` at `cyrup/crates/cyrup-agent/src/hooks.rs:138`) and `PolicyHooks::prepare_next_turn` sets only `update.tools` at `cyrup/crates/cyrup-session-svc/src/hooks.rs:180`. The stated reason is real: cyrup has one prompt slot into which `assemble_run_messages` already wrote a `before_agent_start` handler's sanitized prompt (same refusal at `session.rs:1102`), and overwriting it mid-run would undo the permission companion's `shouldExposeTool` shaping.

**upstream** — pi keeps the override and the base in separate slots: `_installAgentNextTurnRefresh` returns `{...previousContext, systemPrompt, tools}` resolving `_systemPromptOverride ?? _baseSystemPrompt` at `pi/packages/coding-agent/src/core/agent-session.ts:519-540` (`:531`).

**Impact** — a tool registered mid-run becomes callable but undescribed for the remainder of that run. Compounds EXT-007: between them an extension tool's snippet is missing from the first prompt of a session and from the rest of any run it joins mid-flight.

**Fix** — split the prompt into `base_system_prompt` and `system_prompt_override` as pi does, then have `next_turn_tools` return `(tools, resolved_prompt)` and `prepare_next_turn` (`hooks.rs:180`) populate `TurnUpdate.system_prompt`. Until the split lands, keep **both** guards (`session.rs:4196-4205` and `session.rs:1102`) — removing either without the split silently undoes permission-system prompt sanitization.

**Verify** — register a tool mid-run and assert its snippet appears in the next turn's prompt while a `before_agent_start` sanitization applied earlier in the same run survives. The tool-array half is already covered by `cyrup/crates/cyrup-agent/tests/turn_tool_refresh.rs`.

## EXT-033 — An `--extension`/`-e` path that is a FILE (or does not exist) is silently ignored

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/src/loader.rs:109-115`: for each configured path `p`, `if is_extension_dir(p) { push_dir(...) } else { scan_dir(p, ...) }`. `is_extension_dir` (`:120-122`) is `dir.join("extension.json").is_file() || first_wasm(dir).is_some()`; for a file path both are false — `./my-ext.wasm/extension.json` cannot exist, and `first_wasm` does `std::fs::read_dir(dir).ok()?` at `:126`, returning `None` on ENOTDIR. Control falls to `scan_dir` (`:136-150`), whose first statement is `let Ok(rd) = std::fs::read_dir(dir) else { return };` — a silent return, which also swallows a non-existent path. Nothing records a `LoadError`: `discover_and_load` (`cyrup/crates/cyrup-ext/src/facade.rs:960-974`) is `for disc in self.discover(roots)` and only pushes to `result.errors` from `load_discovered`, so a path `discover` dropped produces neither `loaded` nor `errors`. The path reaches loader.rs unmodified: `cyrup/crates/cyrup/src/cli.rs:413` (`config.extra_extension_paths = resolve_cli_paths(&dirs.cwd, &self.extension)`) → `resolve_cli_paths` (`cli.rs:580-591`, joins relatives to cwd, no existence or shape check) → `cyrup/crates/cyrup-session-svc/src/builder.rs:1505`/`:1511` as `DiscoveryRoots.configured`. cyrup's own help at `cli.rs:148` reads "Load an extension file (repeatable)". No test covers a file-shaped configured path — `cyrup/crates/cyrup-ext/tests/loader.rs:34-60` exercises only directory forms.

**upstream** — `pi/packages/coding-agent/src/core/extensions/loader.ts:695-709` checks `if (fs.existsSync(resolved) && fs.statSync(resolved).isDirectory())` and only then does directory discovery (`resolveExtensionEntries`, `discoverExtensionsInDir`); otherwise it falls through to `addPaths([resolved])` at `:708`, loading the path directly. pi's documented rule #1 at `loader.ts:621` is "Direct files: `extensions/*.ts` or `*.js` → load", and its help text at `pi/packages/coding-agent/src/cli/args.ts:281` is the identical "Load an extension file (can be used multiple times)". An unresolvable path is still added and surfaces as a per-path entry in `LoadExtensionsResult.errors`.

**Impact** — `cyrup -e ./my-ext.wasm`, the exact invocation the flag's own help advertises, does nothing at all, and a typo'd `-e` path is indistinguishable from a correct one: no load, no diagnostic, empty `errors`. The author's only symptom is that their extension's tools and commands are absent. This is also the documented escape hatch under `--no-extensions` (`builder.rs:1501-1507` keeps `configured` alive when discovery is off), so it is the path a user is most likely to reach for. Medium rather than high only because cyrup ships zero WASM guests today; the silent-no-diagnostic behavior argues for high once anyone authors a component.

**Fix** — in `crate::loader::discover` (`loader.rs:109-115`), branch on `p.is_file()` first: for a `*.wasm` file synthesize the same minimal manifest `push_dir` builds at `:176-185` (id from the file stem, `world: HOST_WORLD`) with `wasm: Some(p.clone())` and `dir: p.parent()`, pushed as `ExtOrigin::Configured`. For a path that exists but is neither a dir nor a `.wasm`, and for a nonexistent path, return a discovery-level diagnostic so `discover_and_load` records a `LoadError` — either by giving `discover` a `(Vec<DiscoveredExtension>, Vec<LoadError>)` return or by pre-validating `roots.configured` in `facade::discover_and_load`. Do not relax the manifest world gate for the synthesized case beyond what `loader.rs:181` already does (see EXT-028).

**Verify** — point `DiscoveryRoots.configured` at (a) a prebuilt `.wasm` file, (b) a nonexistent path, (c) an existing non-extension file. Assert (a) loads and its tools appear in `all_registered_tool_names`, and that (b) and (c) each produce exactly one `LoadExtensionsResult.errors` entry naming the path. Extend `cyrup/crates/cyrup-ext/tests/loader.rs`.

## EXT-034 — Bus events emitted from an event handler are never delivered — the drain runs only after run_command/run_shortcut

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `SharedBus::emit` merely enqueues into a `pending: Mutex<VecDeque<(String, Value)>>` (`cyrup/crates/cyrup-ext/src/host/services.rs:985-989`; struct at `:961-967`). The fan-out is `ExtensionHost::deliver_bus_events` (`cyrup/crates/cyrup-ext/src/facade.rs:848-873`), and it has exactly **two** production call sites, both at the tail of a command-tier call: `facade.rs:1011` (`run_command`) and `:1065` (`run_shortcut`). Nothing drains after `Dispatcher::dispatch_notify` / `dispatch_block_mutate` / `dispatch_collect_handled` / `dispatch_first_handled` (`cyrup/crates/cyrup-ext/src/dispatch.rs:181-271`), after `LiveExtension::invoke_event` (`host/live.rs:1389-1410`), or anywhere in `cyrup-session-svc`. The deferral itself is deliberate and correct (wasm single-instance reentrancy forbids re-entering the emitting guest inside its own `bus.emit` import — documented at `host/services.rs:952-960`); the defect is that the drain was only wired into the command path. `cyrup/crates/cyrup-ext/tests/wasm_bus_flag.rs` exercises exactly two shapes — a manual `host.deliver_bus_events(&cancel)` at `:82` and the `run_command` tail drain at `:111` — and has no event-handler emit test.

**upstream** — `pi/packages/coding-agent/src/core/event-bus.ts:12-32`: `createEventBus()` returns `emit: (channel, data) => { emitter.emit(channel, data); }` over a node `EventEmitter`, so every listener is invoked synchronously at the emit call regardless of which entry point emitted (the per-listener `try/catch` lives in the `on` wrapper at `:19-27`). `pi/packages/coding-agent/src/core/extensions/loader.ts:398` attaches that one bus to every extension. There is no queue, no drain point, and no entry point from which an emit can go undelivered.

**Impact** — `pi.events` silently works from a slash-command handler and silently does not work from an event handler, which is where cross-extension coordination actually happens (a permission decision, a tool result, a session start). A subscriber either never runs or runs much later against stale state, and the author sees `bus.emit` succeed with no error. Combined with EXT-018 the bus is usable today only for guest→guest signalling initiated by a slash command.

**Fix** — drain at every seam that can have re-entered a guest, not just the command tier. The cheapest correct placement is inside `Dispatcher`'s public entry points, after the subscriber loop completes in `dispatch_notify`/`dispatch_block_mutate`/`dispatch_collect_handled`/`dispatch_first_handled` (`dispatch.rs:181-271`), which is outside every guest store guard. That requires the dispatcher to hold a bus handle (today only `ExtensionHost` does), so either move `SharedBus` ownership onto `Dispatcher` or pass an `Arc<SharedBus>` in at `Dispatcher::new`. Keep the `MAX_ROUNDS = 64` cycle bound at `facade.rs:851`. Fold with EXT-018's un-cfg-gating so the same drain reaches natives.

**Verify** — two guests from the fixture component, B subscribed to `demo:bus`. Drive an *event* (not a command) into A whose handler emits — e.g. `dispatch_block_mutate` with a `HostEvent::ToolCall` — and assert B's `bus-deliver` ran before the dispatch call returned, with no manual `deliver_bus_events`. Add beside the two existing shapes in `tests/wasm_bus_flag.rs`.

## EXT-035 — NativeExtension can register only 5 of the 10 WIT registration surfaces — no shortcuts, flags or providers

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `InitApi` (`cyrup/crates/cyrup-ext/src/native.rs:227-235`) holds exactly `subs`, `tools`, `commands`, `tool_renderers`, `message_renderers`, and its public surface (`:242-278`) is `subscribe`/`register_tool`/`register_command`/`register_message_renderer`/`register_tool_renderer`; `InitParts` (`:218-224`) is the matching 5-tuple. The WIT `interface registration` (`cyrup/crates/cyrup-ext/wit/world.wit:262-278`) gives a guest ten: `register-tool` (`:264`), `register-command` (`:265`), `register-shortcut` (`:266`), `register-flag` (`:267`), `get-flag` (`:268`), `register-provider` (`:269`), `unregister-provider` (`:270`), `register-message-renderer` (`:271`), `add-autocomplete` (`:272`), `add-autocomplete-provider` (`:275`), plus `subscribe` (`:277`). The registry backs all of them (`register_shortcut` `registry.rs:390`, `register_provider` `:415`, `unregister_provider` `:428`) but the only route is the guest import path in `host/live.rs`; `HostCtx` (`native.rs:1-213`) exposes no registry handle. The asymmetry is load-bearing even given a back door: `ExtensionHost::run_shortcut` (`facade.rs:1050-1067`) is `#[cfg(feature = "wasm-host")]` and resolves the owner out of `self.live` only (`:1057-1062`), so a registry entry owned by a native could never fire — even though `shortcut_keys()` (`facade.rs:1043-1045`) is registry-backed and *is* consumed by the TUI (`cyrup/crates/cyrup-tui/src/app.rs:4187`, `cyrup/crates/cyrup/src/main.rs:1330`, dispatch at `app.rs:4066`). All three extensions cyrup ships are `NativeExtension`s, so this is the whole shipped population.

**upstream** — pi has one extension kind and one API object. `ExtensionAPI` (`pi/packages/coding-agent/src/core/extensions/types.ts`) declares `registerTool`, `registerCommand`, `registerShortcut`, `registerFlag`/`getFlag`, `registerProvider`/`unregisterProvider`, `registerMessageRenderer` (`:1284`), `registerMarkdownTransformer` (`:1287`), `registerEntryRenderer` (`:1290`) and `addAutocompleteProvider` (`:225`) on the same object; `pi/packages/coding-agent/src/core/extensions/loader.ts:355-401` builds that single object — including `registerNativeProvider` at `:390`, `unregisterProvider` at `:392-395` and `events: eventBus` at `:398` — and hands it to every extension it loads. There is no upstream notion of an extension that can register tools but not shortcuts, flags or providers.

**Impact** — a shipped first-party extension cannot bind a keyboard shortcut, cannot declare a CLI flag (so it cannot be configured the way pi extensions are), and cannot contribute a provider. With zero WASM guests shipping, five of the ten documented registration capabilities are unreachable by every extension that actually exists — present in the WIT and the SDK, dead for the live population. It also forces natives to re-invent configuration out of band, which is how `CYRUP_*` env-var sprawl grows.

**Fix** — add `register_shortcut(key, desc)`, `register_flag(name, spec)`, `register_provider(id, config)` and `unregister_provider` to `InitApi` (`native.rs:242-278`), extend `InitParts` (`:218-224`) and the `load_native` fold that consumes it to push each into the existing registry methods (`registry.rs:390`/`:415`/`:428`). Then make `ExtensionHost::run_shortcut` (`facade.rs:1050-1067`) try `self.native` before `self.live` — the native-first pattern `render_via` already established at `facade.rs:716-730` — and drop its `wasm-host` cfg gate so a native shortcut fires in a slim build. `get_flag` needs a native-visible read; route it through `HostCtx` alongside the existing rich-state accessors.

**Verify** — a native registering a shortcut, a flag and a provider in `init`: assert `ExtensionHost::shortcut_keys()` lists the key, that `run_shortcut` reaches the native's handler, that `apply_extension_flag_values` + a native `get_flag` round-trips the CLI value (mirroring `cyrup/crates/cyrup-ext/tests/wasm_bus_flag.rs:127-170`, which proves the guest half), and that `registry().provider_ids()` contains the id. Add to `cyrup/crates/cyrup-ext/tests/native_dispatch.rs`.

## EXT-016 — resources_discover carries neither cwd nor reason

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `HostEvent::ResourcesDiscover` is a payload-less unit variant at `cyrup/crates/cyrup-ext/src/event.rs:309`; WIT `on-resources-discover: func() -> hook-outcome;` at `wit/world.wit:228`. Dispatched from `aggregate_resources` (`facade.rs:455-461`), whose only production caller is `cyrup/crates/cyrup-session-svc/src/builder.rs:870`. There is no `cwd` accessor anywhere in the WIT — `ctx-state` (`world.wit:504-513`) exposes `is-idle`/`has-pending-messages`/`is-project-trusted`/`get-system-prompt` and nothing path-shaped, and no other interface offers one.

**upstream** — `ResourcesDiscoverEvent { type, cwd, reason: "startup" | "reload" }` at `pi/packages/coding-agent/src/core/extensions/types.ts:544-548`.

**Impact** — a resource-contributing extension cannot tell which directory it is discovering for, nor distinguish startup from a `/reload`, so it cannot cache or scope its contribution.

**Fix** — add `cwd: String` and `reason: String` to the variant, pass them from `builder.rs:870` and the reload path, widen `world.wit:228` in both copies (ABI break ⇒ bump `HOST_WORLD`). Adding `cwd` to `ctx-state` is a cheaper partial needing no bump — it is an import, and imports are additive.

**Verify** — a guest asserting a non-empty cwd and `reason == "startup"` at launch and `"reload"` after `/reload`.

## EXT-019 — registerMarkdownTransformer has no counterpart

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — grep for `markdown_transformer|markdowntransform|transform-markdown` across `crates/` returns zero hits at HEAD.

**upstream** — `registerMarkdownTransformer(transformer: MarkdownTransformer): void;` at `pi/packages/coding-agent/src/core/extensions/types.ts:1287` (doc at `:1286`: "Register a transformer for user and assistant Markdown before Pi renders it in the interactive transcript"), the `MarkdownTransformer` type at `:1143`, load-order collection by `getMarkdownTransformers` at `pi/packages/coding-agent/src/core/extensions/runner.ts:589`.

**Impact** — extensions cannot post-process transcript Markdown (link rewriting, redaction, custom syntax). Post-baseline upstream addition — expected lag, not a regression.

**Fix** — registration import in `world.wit:262-278`, an owner list in `registry.rs` preserving load order, a `transform_markdown` fold on the facade, and a call site in the TUI markdown render path. Guest export ⇒ bump `HOST_WORLD` (EXT-028).

**Verify** — two guests transforming the same message; assert both applied in load order.

## EXT-022 — ProviderConfig.refreshModels is not represented

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `ProviderConfig` at `cyrup/crates/cyrup-ext/src/provider.rs:17-42` carries `name`/`base_url`/`api`/`api_key`/`auth_header`/`headers`/`models`/`oauth`/`has_stream_simple` and no refresh hook. `interface events` has `provider-login`/`provider-refresh-token`/`provider-get-api-key`/`provider-modify-models`/`provider-stream-simple` at `wit/world.wit:93-97` and no `provider-refresh-models`; grep for `refresh_models|refresh-models` under `crates/cyrup-ext/` returns nothing.

**upstream** — `refreshModels?(context: RefreshModelsContext): Promise<ProviderModelConfig[]>` at `pi/packages/coding-agent/src/core/extensions/types.ts:1459`, doc at `:1455-1458` ("The returned list replaces extension-provided models"). Post-baseline upstream addition.

**Impact** — an extension provider's model list is fixed at registration; a provider that gains models at runtime cannot refresh them.

**Fix** — add a `provider-refresh-models` guest export in both `world.wit` copies plus a marker on `ProviderConfig`, and reuse the collapse-concurrent-calls machinery at `cyrup/crates/cyrup-provider/src/utils/refresh.rs`. Guest export ⇒ bump `HOST_WORLD` (EXT-028).

**Verify** — a guest provider whose `refresh_models` returns a changed list; assert the model picker reflects it and that concurrent refreshes collapse.

## EXT-025 — reload() and four emit_* facade methods are dead code that has drifted

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — per-symbol greps at HEAD: `emit_before_agent_start` (`cyrup/crates/cyrup-ext/src/facade.rs:468`) — callers `tests/wasm_dispatch.rs:68,86` only; `emit_input` (`:505`) — `wasm_dispatch.rs:91,98,104` only; `emit_user_bash` (`:585`) — `wasm_dispatch.rs:133,138` only; `command_completions` (`:1016`) — `tests/discover_load.rs:94` only; `ExtensionHost::reload` (`:1081`) — `tests/discover_load.rs:105` only, while production `/reload` goes through `SessionRuntime::reload` (`cyrup/crates/cyrup-session-svc/src/runtime.rs:177`, reached from `session.rs:2559` and `cyrup-tui/src/app.rs:2439`). `emit_message_end` (`:535`) is the live exception (`subscriber.rs:156`). The drift is real and still present: `cyrup/crates/cyrup-session-svc/src/session.rs:4007` threads `options.exclude_from_context` through the inline copy while the facade copy at `facade.rs:585` takes no such parameter.

**upstream** — `pi/packages/coding-agent/src/core/extensions/runner.ts` has exactly one emitter per event and no parallel unused copy.

**Impact** — two implementations of the same seam, one exercised only by tests and already behind in signature. A future contributor editing the facade copy changes nothing; one editing only the inline copy leaves the tests asserting stale behavior.

**Fix** — either delete the four dead `emit_*` methods and `reload()` and repoint the tests at the live paths, or make the live paths call the facade so there is one implementation — the latter is what `emit_message_end` already demonstrates.

**Verify** — grep confirms each `emit_*` has at least one production caller, or no definition; `tests/wasm_dispatch.rs` and `tests/discover_load.rs` exercise the live paths.

## EXT-026 — A wasmtime-free cyrup-session-svc build cannot be produced

**Kind** cyrup-original · **Severity** low · **Effort** M · **Confidence** medium (static analysis only — no compile was run)

**cyrup** — `cyrup/crates/cyrup-ext/src/lib.rs` gates `caps`/`host`/`host_runtime` on `wasm-host`. `cyrup/crates/cyrup-session-svc/src/lib.rs:29` declares `mod host_services;` unconditionally, and `cyrup/crates/cyrup-session-svc/src/host_services.rs:17-22` imports `cyrup_ext::caps::http::HttpCaps`, `cyrup_ext::caps::proc::ProcCaps` and `cyrup_ext::host::{ControlOp, DialogOptions, ExecOutput, HostServices, HttpRequest, HttpResponse, HttpStreamResponse, HumanInteractionLock, NotifyKind, ProcSpawnSpec}` with zero cfg guards. `cyrup/Cargo.toml:73` declares `cyrup-ext = { path = "crates/cyrup-ext", version = "0.0.0" }` without `default-features = false`, and `cyrup/crates/cyrup-session-svc/Cargo.toml:33` inherits it as `{ workspace = true }`, so cyrup-ext's `default = ["wasm-host"]` stays on regardless of session-svc's own `wasm-host = ["cyrup-ext/wasm-host"]` (`Cargo.toml:23`). Evidence the arms have rotted: 1d87913 added `#[cfg(not(feature = "wasm-host"))] let host = ExtensionHost::new(host_config);` at `cyrup/crates/cyrup-session-svc/src/builder.rs:1472` — code that has never been compiled. Same state for `facade.rs:1069-1073` (`not(wasm-host)` `run_shortcut` fallback) and `materialize_guest_tools`'s `not(wasm-host)` arm at `facade.rs:413-416`.

**upstream** — no analog; pi has no compile-time feature tiers. This is a cyrup-original invariant (`wasm-host` is documented as an opt-*out* slimming switch) that does not hold.

**Impact** — the advertised slim build is unbuildable, and the `not(wasm-host)` code paths accumulate without ever being type-checked, so they will not work when the gate is finally fixed.

**Fix** — gate `mod host_services;` and its `cyrup_ext::caps`/`cyrup_ext::host` imports on `wasm-host`; declare `cyrup-ext` in the workspace with `default-features = false` and re-enable `wasm-host` from the crates that want it; then compile the `not(wasm-host)` arms at least once.

**Verify** — `cargo check -p cyrup-session-svc --no-default-features` succeeds and the resulting dependency graph contains no `wasmtime`.

## EXT-027 — pi's bundled llama.cpp router extension has no counterpart

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — `grep -rni llama crates/ --include=*.rs` returns only incidental hits, all inspected: an auth comment at `cyrup/crates/cyrup-provider/src/auth/helpers.rs:23`, two overflow regexes at `cyrup/crates/cyrup-provider/src/utils/overflow.rs:27,170`, a reasoning-field comment at `cyrup/crates/cyrup-provider/src/api/openai_completions.rs:1207`, plus catalog data. There is no bundled-extension tier in cyrup at all.

**upstream** — `pi/packages/coding-agent/src/extensions/` contains exactly `index.ts` and `llama/`. Entirely post-baseline — expected lag.

**Impact** — no local llama.cpp routing out of the box. Lowest value in this area.

**Fix** — only worth doing if a bundled-extension tier is wanted; it would be the first. Needs a shipped-with-binary extension registration path alongside `NativeExtension`.

**Verify** — n/a until the tier exists.

## EXT-032 — p3_no_human_wait_is_still_budget_contained asserts an uncontrollable wall-clock bound

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/tests/native_dispatch.rs:848-883`, on `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` (`:848`). `SlowNoGateExt` sleeps 400ms with no human-wait guard (`:811`), the dispatcher budget is 80ms (`:861`), and `:875` asserts `elapsed < Duration::from_millis(300)` — 220ms of scheduling headroom under a full parallel `cargo test`. The assertion is strictly redundant: `SlowNoGateExt::on_event` returns `HookOutcome::Block { reason: Some("should never be observed (budget-timed-out)") }` at `:812`, so had the budget not fired the reduction would be `Blocked` with *that* reason — which the deterministic assertion at `:876-881` (reason must contain "Extension failed, blocking execution") already rejects with a panic. The timing assertion can therefore only fail spuriously. `native_dispatch.rs:706-710` is the same shape with a defensive 2s bound for the same 80ms budget; `cyrup/crates/cyrup-ext/tests/ext_fail_closed.rs:256-280` is the correct form (80ms budget vs a 600ms sleep, no wall-clock assertion at all).

**upstream** — no analog; this is a cyrup test-suite defect of the class the project keeps finding (`providers/anthropic.rs`, `round9_l5res.rs`, `caps/proc.rs`).

**Impact** — a flaky failure under load that blames the dispatcher budget, eroding trust in the suite. It proves nothing the surrounding assertions do not already prove deterministically.

**Fix** — delete the assertion at `native_dispatch.rs:875`; the outcome assertion at `:876-881` already proves budget containment completely. If a timing bound is wanted for documentation, use the defensive 2s form already used at `:710`.

**Verify** — the test still **fails** if `fails_closed` or the budget watchdog is reverted, and passes under `cargo test -- --test-threads=1` and under a loaded parallel run alike.

## EXT-036 — `EventKind::COUNT`'s doc claims 1:1 parity with a 31-event pi catalog; pi has 33 — and world.wit still says 30

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/src/event.rs:57-59`: "The number of distinct kinds (must stay <= 64 for the bitset). 1:1 with Pi's 31-event catalog (extensions/types.ts:1133-1171 + `agent_settled` at :1225)." followed by `pub const COUNT: u8 = 31;`. The cited range is stale — pi's `on(event: "…")` overload block now runs `types.ts:1198-1240`, with `agent_settled` at `:1225` *within* it, not appended after. The same stale `:1133-1171` range recurs in the `name()` doc at `event.rs:100`. Staler still: `cyrup/crates/cyrup-ext/wit/world.wit:8` reads "The `events` interface mirrors Pi's 30-event catalog 1:1 (extensions/types.ts:1133-1171)" — a count that was already wrong before `agent_settled` landed — and it appears in both byte-identical copies.

**upstream** — enumerating every `on(event: "…")` overload in `pi/packages/coding-agent/src/core/extensions/types.ts:1198-1240` by reading the block (four overloads are multi-line and defeat a grep) gives 33 distinct events. Diffing against cyrup's 31 `EventKind::name()` values (`event.rs:102-134`) leaves exactly two: `before_provider_headers` (`types.ts:1220`, type at `:686-689`) and `session_info_changed` (`types.ts:1201`, type at `:571-575`). 33 − 2 = 31, so `COUNT` is internally consistent — but the catalog is not 1:1.

**Impact** — the comment is the primary provenance marker a reader consults to decide whether the event catalog is complete, and it asserts a completeness that does not hold, in two places that disagree with each other on the number. Anyone auditing the extension surface from either file concludes there is nothing left to port and stops — which is how EXT-009 and EXT-011 stay invisible. Doc-only; no runtime consequence.

**Fix** — rewrite `event.rs:57-59` to state the real position (31 of pi's 33, `extensions/types.ts:1198-1240`), naming `before_provider_headers` and `session_info_changed` as outstanding and cross-referencing EXT-009 / EXT-011. Refresh the `:1133-1171` range to `:1198-1240` at `event.rs:57` and `:100`. Fix `wit/world.wit:8` in **both** copies — a one-sided edit is caught by `tests/wit_world_sync.rs` byte-equality.

**Verify** — grep the crate for `1133-1171` and for `30-event`; confirm no occurrences remain in either `world.wit` copy or in `event.rs`, and that the stated missing-event list matches a diff of pi's `on`-event names against `EventKind::name()` values.

## Coverage

**Read at HEAD `1806375`** (tree confirmed clean): `cyrup-ext/src/{event,dispatch,facade,registry,native,contract,hooks,error,loader,manifest,provider,wrapper,lib}.rs`, `cyrup-ext/src/host/{live,services}.rs` (targeted regions), `cyrup-ext/src/build/{mod,cache}.rs`, both copies of `wit/world.wit` (enumerated function by function in every interface, and `diff`ed byte-for-byte), `cyrup-ext-sdk/src/{descriptor,guest,api,macros,example}.rs`, and every file under `cyrup-ext/tests/` plus `cyrup-ext-sdk/tests/ergonomic.rs`. Consumer seams read where the chain required it: `cyrup-session-svc/src/{builder,session,hooks,subscriber,runtime,host_services,event}.rs`, `cyrup-agent/src/{agent,hooks}.rs`, `cyrup-core/src/tool.rs`, `cyrup-tui/src/app.rs`, `cyrup/src/{cli,main}.rs`, plus `Cargo.toml` at the workspace root and in `cyrup-session-svc`. Upstream read directly: `pi/packages/coding-agent/src/core/extensions/{types,runner,loader}.ts`, `core/{agent-session,event-bus,project-trust}.ts`, `cli/args.ts`, `packages/agent/src/agent-loop.ts`. Git evidence gathered with `git log --oneline c8bd2ab..HEAD -- <path>` for `wit/world.wit` and `src/manifest.rs`.

**Method** — no closure was accepted on the strength of a commit message. For each previously-open item the cyrup file was opened at HEAD, the chain followed to its consumer, and the upstream file re-read at the cited lines. No cargo, no npm; read and grep only. Six items closed (EXT-001, -002, -004, -005, -010, -020); EXT-003 closed only in part and remains open. Nothing was overturned — every closure survived a deliberate attempt to refute it (EXT-001 tested for `fails_closed` over-breadth via the converse assertion at `tests/ext_fail_closed.rs:352`; EXT-005 audited against pi's entire `ExtensionContext` rather than the four cited methods; EXT-020 checked in the harder outbound direction).

**Evidence corrections carried in, not status changes** — EXT-021's title count raised from six to at least eight after auditing pi's `UIContext` block directly (and `onTerminalInput` corrected from `:145` to `:144`); EXT-008's upstream side strengthened (pi is first-wins on execution too, `runner.ts:463-471`); EXT-017's root cause pinned to the `HashMap` at `registry.rs:111` with refreshed pi refs; EXT-015 sharpened to the exact statement where the field is dropped, with all four pi event refs re-counted; EXT-030's trigger condition narrowed; EXT-028 backed with git rather than assertion, plus a third stale doc found at `world.wit:8`; EXT-011's pi ref corrected to `types.ts:571-575` (the prior `:559-564` was `SessionStartEvent`); EXT-006's pi refs re-counted and confirmed exact.

**Test-defect hunt** — both shapes re-run independently across `cyrup-ext/tests/` and `cyrup-ext-sdk/tests/` (`elapsed|Instant::now|sleep(|yield_now|timeout(`). Shape 2 (uncontrollable timing): exactly one unsafe bound survives, filed as EXT-032; `native_dispatch.rs:706-710` is the same shape with a defensive margin and is folded into EXT-032's fix rather than filed separately; `ext_fail_closed.rs:256-280` is the correct form and is the model to copy; `ergonomic.rs:290` is a serde assertion on a `timeoutMs` field, not a timing bound. Shape 1 (pinning current-but-wrong behavior): none found — `tests/aggregation.rs:184-239` asserts the *correct* first-wins getter order at `:204` and `deploy:1`/`deploy:2` load-order disambiguation at `:227-238`, making it evidence for EXT-008/EXT-017 rather than a defect; `manifest_cache.rs` compares against the `HOST_WORLD` constant so a bump will not spuriously break it; `wasm_bus_flag.rs:97` only notes last-registered command routing in a comment without asserting it. One comment worth cleaning (`aggregation.rs:189` normalizes "last-wins for execution" as if intended) is folded into EXT-008.

**Blind spots and things taken on trust** — (1) EXT-026 is static analysis; every input was verified but no compile was observed. (2) EXT-028's cache-staleness half is reasoned from `cache_key`'s composition and `hash_source_tree`'s crate-dir-only walk, not from a stale artifact being served; the version-gate half is directly readable and not in doubt. (3) EXT-028's "a 0.2 guest dies inside wasmtime" depends on wasmtime resolving world exports eagerly at `instantiate_async` (`host/live.rs:938`) — the call site was read, no pre-f777e44 component was built. (4) `host/services.rs` (~1885 lines) was not audited method-by-method, so a `ctx.ui` capability could be implemented host-side while missing from the WIT (EXT-021's caveat). (5) The capability sandbox (`caps::{http,fs,proc}`) is unaudited beyond the boundaries these items touch; `caps/proc.rs` changed in this window (the 1806375 test fix) and its enforcement logic was not reviewed. (6) Also unaudited: the Tier-1 `cargo build` path (`build/toolchain.rs`), `ExtensionHost::reload`'s cache-bust ordering, and per-extension capability grant parsing in the manifest. (7) `spec/` is absent from this workspace; `R-NN-NNN` ids were used only as a grep index — no requirement text was consulted or invented.


---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at
all, rather than checking a list of known items. That inversion exists because the
item-driven method missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`)
— a real, user-reported bug — and by construction cannot see behaviour nobody wrote an item
for. IDs use an `-SNN` suffix to mark their provenance.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| EXT-S01 | high | not-ported | S | A native extension whose init() fails aborts the whole session build — pi contains a load failure per-extension and starts anyway |
| EXT-S02 | high | not-ported | M | Extension-registered slash commands never appear in the TUI's `/` autocomplete — the registry cannot hold a runtime name |
| EXT-S03 | medium | not-ported | S | Contained extension faults are invisible in the TUI — only RPC mode registers an error listener (print/json half is SEAM-006) |
| EXT-S04 | low | not-ported | M | ctx.compact(options) drops customInstructions and both completion callbacks — the WIT verb takes no arguments at all |
| EXT-S05 | low | not-ported | M | ctx.mode and ctx.hasUI are unreachable from a WASM guest — the ext-mode enum is declared and used by zero functions |
| EXT-S06 | low | not-ported | S | A prebuilt .wasm dropped directly into an extensions discovery root is skipped — scan_dir only descends into directories |

## EXT-S01 — A native extension whose init() fails aborts the whole session build — pi contains a load failure per-extension and starts anyway

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** confirmed

**upstream** — loader.ts:414-440 `loadExtension` wraps `await factory(api)` in try/catch → `{extension:null, error}`; loader.ts:481-500 `loadExtensionsInternal` pushes to `errors` and `continue`s. The closer analog to a cyrup native (an in-process factory, not a disk file) is resource-loader.ts:945-966 `loadExtensionFactories`, which wraps `loadExtensionFromFactory` in its own try/catch and pushes `{path, error}` — so BOTH of pi's load routes contain per-extension. No pi path lets one bad extension prevent the agent from running.

**cyrup** — ABSENT. 

**Impact** — With subagents armed (CYRUP_SUBAGENTS truthy), an unwritable artifacts root means `cyrup` does not start at all — no session, no TUI, no prompt, and the error names a subagents directory rather than telling the user extensions can be disabled. pi in the same state prints one diagnostic line and runs with the extension absent. The crate even carries a comment asserting this matches pi; it does not — pi's equivalent throw is caught one frame up.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## EXT-S02 — Extension-registered slash commands never appear in the TUI's `/` autocomplete — the registry cannot hold a runtime name

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed

**upstream** — interactive-mode.ts:636-645 builds `extensionCommands` from `this.session.extensionRunner.getRegisteredCommands()` (runner.ts:643-646, backed by `resolveRegisteredCommands` at :603-635), filters built-in shadows, and at :662 merges `[...slashCommands, ...templateCommands, ...extensionCommands, ...skillCommandList]` into `CombinedAutocompleteProvider`.

**cyrup** — ABSENT. 

**Impact** — Every command every shipped extension registers is invisible in `/` autocomplete: `/intercom` (`cyrup-intercom/src/extension.rs:234`) and the whole `SLASH_COMMANDS` table registered at `cyrup-ext-subagents/src/extension.rs:5676-5683`. Prompt-template and skill commands are absent for the same reason. Execution is fine — `cyrup-session-svc/src/session.rs:830-838` `prepare` tries `try_execute_extension_command` before anything else — so a user who knows the exact name is served; a user who types `/inter` gets no popup at all (`slash_context` returns `None` on empty matches) and, on Enter, `Dispatch::Prompt` sends the literal text to the model.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## EXT-S03 — Contained extension faults are invisible in the TUI — only RPC mode registers an error listener (print/json half is SEAM-006)

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — interactive-mode.ts:1759-1761 binds `onError: (error) => this.showExtensionError(error.extensionPath, error.error, error.stack)`; `showExtensionError` (:2610-2626) appends `Extension "<path>" error: <msg>` plus a dimmed stack to the chat container and calls `requestRender()`. print-mode.ts:98-100 `console.error(...)`; rpc-mode.ts:347 the wire event. All eleven runner `emit*` reducers funnel their per-handler catch into `emitError` (runner.ts:563-567).

**cyrup** — ABSENT. 

**Impact** — In the primary front-end an extension handler that traps, times out, OOMs or panics produces zero user-visible output. A permission gate faulting on every `tool_call` blocks every tool with a generic reason and never names the extension; a `context` handler that throws silently stops filtering the transcript. pi puts the extension path and stack on screen.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## EXT-S04 — ctx.compact(options) drops customInstructions and both completion callbacks — the WIT verb takes no arguments at all

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**upstream** — types.ts:296-300 `CompactOptions {customInstructions?, onComplete?(CompactionResult), onError?(Error)}` and types.ts:345 `compact(options?)`; runner.ts:740-743 forwards to `contextActions.compact`; agent-session.ts:2424-2434 is the real implementation — `await this.compact(options?.customInstructions)` then `options?.onComplete?.(result)` / `options?.onError?.(err)`.

**cyrup** — ABSENT. 

**Impact** — An extension cannot steer compaction the way `/compact <instructions>` can, and cannot learn whether the compaction it asked for ran, was vetoed by another extension's `session_before_compact`, or errored — so it cannot sequence work after it. Rated LOW rather than the claim's medium purely on today's exposure: cyrup ships zero WASM guests and no shipped native calls `ctx.compact` (`grep -rn 'ControlOp::Compact' crates` finds only the enum, the label table at session.rs:68, the handler at :2681, and a unit test). The seam shape is the defect.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## EXT-S05 — ctx.mode and ctx.hasUI are unreachable from a WASM guest — the ext-mode enum is declared and used by zero functions

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**upstream** — types.ts:309-315 puts `mode: ExtensionMode`, `hasUI: boolean` and `cwd: string` on the BASE `ExtensionContext` (every handler, every tool, not just command handlers); runner.ts:681-693 resolves all three lazily per call. The doc on `mode` states the intended use: guard terminal-only UI so it is not attempted in print/json mode.

**cyrup** — ABSENT. 

**Impact** — A guest cannot branch on run mode, so pi's documented guard is unwritable — the guest either always attempts terminal-only UI (silently no-oped by `DenyServices`, `host/services.rs:169-177`) or never does. Low: cyrup ships zero WASM guests today.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## EXT-S06 — A prebuilt .wasm dropped directly into an extensions discovery root is skipped — scan_dir only descends into directories

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — loader.ts:620-641 `discoverExtensionsInDir` rule 1 is direct files: `if ((entry.isFile() || entry.isSymbolicLink()) && isExtensionFile(entry.name)) { discovered.push(entryPath); continue; }`; the contract comment at :618-625 says "1. Direct files: `extensions/*.ts` or `*.js` → load". Only rules 2/3 descend.

**cyrup** — ABSENT. 

**Impact** — The simplest install — copy a component next to the others in `~/.cyrup/extensions/` — produces nothing: no load, no error, no diagnostic. Low: zero WASM guests ship today.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

