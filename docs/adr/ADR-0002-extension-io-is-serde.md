# ADR-0002 — Extension I/O crosses the extension boundary as serialized data

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** the ADR-citation half of **OQ-6** (`PARITY-PLAN.md:1453-1467`) as it applies to the six
in-source `ADR-0002` citations, plus batch 2's standing item "Write ADR-0001 into this workspace or
delete every reference to it" (`PARITY-PLAN.md:248-250`) applied to ADR-0002. There is no OQ that
asks this question directly — it was decided in code and never written down, which is why the six
citation sites resolve to nothing.
**Blocks released** batch 19 (the single `cyrup:ext@0.5.0` bump, 34 items — its member list cannot
be sized until the ADR-consequence items below are split from the plain ones), batch 20
(cyrup-ext dispatch/events/renderers), the WIT-shaped branch condition of batch 18
(`PARITY-PLAN.md:901-903`), and batch 17's manifest→`GuestState` seeding, which must not widen the
value-semantics contract while narrowing the capability one.

---

## Context

### The question

Six sites in `crates/` cite `ADR-0002` as the authority for how extension payloads are shaped. The
document has never existed in this workspace. `rg 'ADR-0002' /Users/davidmaple/cyrup.ai/cyrup/crates`
at HEAD `72cd292` returns exactly six, and no more:

| site | what it asserts on ADR-0002's authority |
|---|---|
| `crates/cyrup-ext/src/lib.rs:1` | the crate as a whole "binds ADR-0002" |
| `crates/cyrup-ext-sdk/src/lib.rs:1` | the guest SDK "binds ADR-0002" |
| `crates/cyrup-ext/src/manifest.rs:1` | `extension.json` is JSON "consistent with cyrup-config (JSON-only); there is no `toml` dep in the host" |
| `crates/cyrup-session/src/compaction/hooks.rs:2` | compaction hook payloads "are plain serde structs (they cross the WASM boundary as serialized events per ADR-0002)" |
| `crates/cyrup-session/src/prompt/hook.rs:8` | "The payload is expressed as serializable data (ADR-0002: extension I/O crosses as serde, not host pointers)" |
| `crates/cyrup-ext/Cargo.toml:70` | `cyrup-ext` "does NOT depend on cyrup-tui — extension UI crosses as serializable commands (arch-00 §2.1, ADR-0001 R-ARCH-TUI-014 / ADR-0002 R-ARCH-EXT-010)" |

`R-ARCH-EXT-010` occurs nowhere else in the tree (`rg 'R-ARCH-EXT-010' crates/` → one hit, that
Cargo.toml line). `arch-08`, which the WIT world and half of `cyrup-ext` cite as the normative
architecture, does not exist here either: the repository root contains `crates docs README.md
TUI-FIDELITY.md Cargo.toml rust-toolchain.toml` and no `spec/`. This is the same unreadable-citation
condition OQ-6 raises for `ADR-0001` and the `R-NN-NNN` ids.

**Scope correction — the title is narrower than the rule.** Two of the six sites are not about WASM
at all. `crates/cyrup-session/src/prompt/hook.rs` is in a crate that depends on neither `cyrup-ext`
nor `cyrup-agent` (`grep 'cyrup-ext\|cyrup-agent' crates/cyrup-session/Cargo.toml` → no hits) and
its dispatcher is injected precisely so it does not (`compaction/hooks.rs:2-3`); `Cargo.toml:70` is
about the `cyrup-ext` → `cyrup-tui` edge. The rule is therefore about **the extension boundary**, in
all three of its forms — the WASM Component Model seam, the native built-in seam, and the
crate-dependency seams that feed both — not about WASM specifically. It is restated below in that
scope.

### What is actually true in cyrup at HEAD `72cd292`

The rule is already implemented, consistently, and stated in one place — the top of the world:

> "Open-shaped values (tool params, message lists, patches, custom payloads) cross as `string`
> carrying serde_json; fixed-shape control values (ids, enums, exec results) are real WIT records
> for type safety" — `crates/cyrup-ext/wit/world.wit:4-6`

- The world is `package cyrup:ext@0.4.0;` (`world.wit:18`), `HOST_WORLD = "cyrup:ext@0.4"`
  (`crates/cyrup-ext/src/manifest.rs:69`). The two copies are byte-identical
  (`diff crates/cyrup-ext/wit/world.wit crates/cyrup-ext-sdk/wit/world.wit` → empty). The line-1
  header still says `0.3.0`, which is EXT-028's open residual.
- **Host → guest** is serialization at the call site: `invoke` (`host/live.rs:1460-1615`) turns each
  `HostEvent` into strings — `input.to_string()` at `:1469`, `serde_json::to_string(content)` at
  `:1472`, `serde_json::to_string(messages)` at `:1490`, `:1551`, and so on.
- **Guest → host** is a serialized patch, not a mutation: `variant hook-outcome { noop, block(…),
  mutate(string), handled(string) }` (`world.wit:30-36`), lifted by `decode_outcome`
  (`host/live.rs:1637-1656`) into `HookOutcome` and then by `decode_patch` (`:1659`) into the typed
  `EventPatch` (`crates/cyrup-ext/src/contract.rs:29-65`), which the host folds left-to-right in
  load order (`contract.rs:1-3`).
- **The native tier is held to the same contract.** `NativeExtension::on_event(&self, ev: &HostEvent,
  ctx: &HostCtx) -> HookOutcome` (`crates/cyrup-ext/src/native.rs:324`) hands a native an owned-data
  event and takes back the same `HookOutcome` — not a live agent, session or TUI handle. Registration
  descriptors are serde structs on both paths (`ToolDescriptor`, `registry.rs:12-30`,
  `#[serde(rename_all = "camelCase")]`).
- **Long-lived host-owned things cross as opaque `u32` handles the guest polls**, never as
  references: `http-client.request-stream`/`poll-stream-chunk`/`close-stream` (`world.wit:457-459`)
  and the whole `interface proc` (`:392-413`), with the reason written at `:389-392` — "the guest
  polls … rather than holding a live process handle across the wasm boundary" — and again in the SDK
  at `crates/cyrup-ext-sdk/src/ctx.rs:309`.
- **Where pi passes a function, cyrup already added a round-trip export.** `with-session:
  func(callback-id: string)` (`world.wit:121`) exists because "wasm single-instance reentrancy
  forbids calling it synchronously inside the `control.*` import" (`:117-120`). Renderers registered
  by key (`register-message-renderer(custom-type)`, `:279`) are dispatched back through
  `render-call`/`render-result` returning a **serialized widget tree** (`:169-170`), with the reason
  at `:151-155`: "a WASM guest cannot hand back an object, so cyrup's wire analog is that component
  tree SERIALIZED".

### What is actually true in pi at v0.83.0

pi does not have a boundary here at all. Extensions are TypeScript modules loaded **into the host
process** by jiti and handed live objects:

- `createJiti(import.meta.url, …)` at `packages/coding-agent/src/core/extensions/loader.ts:411`,
  `const module = await jiti.import(extensionPath, { default: true })` at `:419`, `await factory(api)`
  at `:472`. `VIRTUAL_MODULES` (`:47-75`) hands the extension the host's **own live module
  instances** of `pi-tui`, `pi-ai`, `pi-agent-core` and `pi-coding-agent`, so an extension's
  `Component` is the same class the renderer instantiates.
- The base `ExtensionContext` (`extensions/types.ts:307-347` @v0.83.0) carries live host objects, not
  data: `ui: ExtensionUIContext` (`:309`), `sessionManager: ReadonlySessionManager` (`:317`),
  `modelRegistry: ModelRegistry` (`:319`), `model: Model<any> | undefined` (`:321`),
  `signal: AbortSignal | undefined` (`:334`), plus methods (`isIdle()` `:330`, `abort()` `:336`,
  `compact(options?)` `:344`).
- Function values are ordinary members of the API surface: `prepareArguments?: (args) => Static<TParams>`
  (`:468`); `execute(toolCallId, params, signal, onUpdate, ctx)` (`:480-486`);
  `MessageRenderer = (message, options, theme) => Component | undefined` (`:1145-1149`);
  `registerMessageRenderer(customType, renderer)` (`:1276`); `registerEntryRenderer` (`:1279`);
  `AutocompleteProviderFactory = (current: AutocompleteProvider) => AutocompleteProvider` (`:124`);
  `onTerminalInput(handler): () => void` (`:145`); `setEditorComponent(factory)` / `getEditorComponent()`
  (`:260`, `:263`); `setWidget`'s component-factory overload (`:171`);
  `streamSimple?: (model, context, options) => AssistantMessageEventStream` (`:1437`);
  `refreshModels?(context): Promise<ProviderModelConfig[]>` (`:1448`); `events: EventBus` (`:1419`)
  whose `on(channel, handler): () => void` returns an unsubscribe closure
  (`core/event-bus.ts:5`, returned at `:27`).
- And pi mutates shared objects in place. `emitBeforeProviderHeaders(headers: ProviderHeaders)`
  (`core/extensions/runner.ts:1045-1071` @v0.83.0) passes the live headers object to every handler
  under the comment at `:1054`: *"Handlers mutate `headers` in place; the return value is ignored."*
  It then returns the same object it was given (`:1071`).

That is the whole of the mechanism gap: **pi's extension surface is aliasing; cyrup's cannot be.** A
WASM Component Model instance has no shared address space with the host, no shared allocator, and no
way to hold a Rust `&mut` across a call. Nothing about this is a preference.

---

## Decision

**Every value that crosses the extension boundary crosses as a value, not as a reference — on both
the WASM guest tier and the native built-in tier.** Implement it by these rules; do not re-derive
them per item.

1. **Encoding.** Fixed-shape control values (ids, enums, exec results, http request/response,
   tool descriptors) are real WIT records and enums. Open-shaped values (tool params, message lists,
   patches, custom payloads, provider request bodies, option bags) cross as `string` carrying
   serde_json. This is `world.wit:4-6` and it stays. Tool `parameters` stays JSON-Schema for
   pi-interop (`registry.rs:13`).

2. **Field naming.** `#[serde(rename_all = "camelCase")]` on every wire struct. pi's field names are
   the interop contract; a Rust-idiomatic rename is a parity bug, not a style choice.

3. **Where pi mutates a shared object, port it as an explicit round-trip.** The host serializes the
   current value in; the guest returns `hook-outcome::mutate(json)`; the host decodes it into a typed
   `EventPatch` and folds it. **Never** model an in-place mutation as a notify-only event — that
   silently drops the behaviour. `before_provider_headers` is the outstanding case (EXT-009): port it
   as an export taking the header map as JSON and returning a patch whose values are
   `option<string>`, `none` meaning *delete* (pi's `null`, documented at `types.ts:681-685`).

4. **Where pi passes or returns a function, port it as a WIT export plus, where the function is
   invoked with host-owned state, a matching import.** Registration splits into `register-X(key)`
   (import) + a keyed dispatch export. Copy the worked example already in the world:
   `register-message-renderer(custom-type)` + `render-call`/`render-result`, and `with-session`.
   A callback that fires *during* a guest call becomes a host import the guest calls (the
   `oauth` interface, `world.wit:492-500`, is the existing example).

5. **Where pi hands over a live object with methods, decompose it into imports.** Fields become
   getters, because WIT imports are functions — the `ctx-state` block (`world.wit:523-540`) already
   says so at `:527`. A getter is not optional sugar: it is the only representation available.

6. **Where pi hands over a live stream, handle or signal, the host owns it and the guest polls by
   opaque handle.** Never block a guest `Store` on an unbounded await. `http-client` and `proc` are
   the pattern; `ctx.signal` (`types.ts:334`) must become a poll (`is-run-cancelled()`), not be
   dropped.

7. **The encoding is never a licence to drop a field.** Any pi argument, field or return that is
   serde-representable **is in scope and must be carried**. Only a genuinely non-representable thing
   (a live object, a closure, a stream) may be re-shaped — and re-shaped, not omitted. Where
   re-shaping is deferred, a `CYRUP-DELTA` note is **mandatory** in the owning crate's `lib.rs`, and
   it carries **two** things: (a) the upstream `file:line` **with its tag** (`pi v0.83.0
   types.ts:334`) and the reason, and (b) the owning `docs/adr/…` path or `docs/gap-analysis/` item
   id. Half (b) is not decoration — `docs/adr/ADR-0008-requirement-ids-and-sdk-surface.md` §A.3 lints
   `CYRUP-DELTA` as a divergence marker and fails any note that names no owning document, on the
   ground that an unowned delta is precisely the unverifiable claim the project does not accept. The
   two checks ship as one `cargo xtask lint-citations` pass. This rule currently has zero compliance:
   `grep -n 'CYRUP-DELTA' crates/cyrup-ext/src/lib.rs` returns nothing (EXT-045).

8. **An encoding failure is an error, not an empty payload.** Today it is not: `serde_json::to_string(…)
   .unwrap_or_else(|_| "[]".into())` (`host/live.rs:1472`, `:1490`, `:1494`, `:1551`, `:1562-1563`,
   `:744`, `:748`, `:765`) makes a failed encode indistinguishable from an empty list, and
   `decode_outcome`'s unparseable-patch arm returns `Noop` (`:1645-1651`), making a malformed patch
   indistinguishable from a handler that declined. pi has no such failure mode, because passing an
   object cannot fail. Encode/decode failures must reach the extension error channel
   (`App::install_error_listener`, `cyrup-tui/src/app.rs:3140-3149`). **No gap item covers this** —
   see *New work implied*.

9. **ABI versioning follows from the encoding and does not change.** A guest bakes its import list and
   export signatures into its component, so any export added, removed or re-signed bumps the minor
   (`manifest.rs:41-69`); added imports are additive. Keep both `world.wit` copies byte-identical.

10. **Both tiers, one contract.** Do not give native built-ins a reference-holding escape hatch. The
    WIT world is the contract for what an extension can observe and do; a native that could hold a
    live handle would be able to do things no guest could ever do, and the world would stop being the
    specification of extension behaviour.

---

## Consequences

### The classification area 06 is missing

Area 06's opening caveat says the mechanism divergence is "deliberate" and that "the *semantics* …
are fully in scope" (`docs/gap-analysis/06-cyrup-ext.md:3`) — but no item records **which** of its
50 open rows exist *because* of that mechanism. The distinction matters operationally: an
ADR-0002-consequence item needs a round-trip **designed** (an export, a bump, a guest fixture) before
it can be scheduled, while a plain unported item is a field to carry. That is the difference between
an S and an M, and between "batch 19 member" and "batch 20 member".

Add a **`adr-0002`** column to area 06's open-items table with these values. This is the ledger
change; nothing is re-severitied by it except where noted.

**Consequence of ADR-0002 — the gap exists because a live reference cannot cross, and closing it
requires designing a round-trip:**

| ID | the pi thing that cannot cross | the ported shape this ADR mandates |
|---|---|---|
| EXT-009 | `emitBeforeProviderHeaders` mutates `headers` in place (`runner.ts:1045-1071`, comment `:1054`) | export + `option<string>` patch, `none` = delete (rule 3). **This is why the event is absent at all**, and `world.wit:9-11` admits the omission without giving the reason. |
| EXT-023 | `prepareArguments?: (args) => Static<TParams>` (`types.ts:468`) | `prepare-arguments` guest export + descriptor flag (rule 4) |
| EXT-006 | `MessageRenderer = (message, options, theme) => Component` (`types.ts:1145-1149`); pi re-invokes it from the draw path | options+theme as explicit export params, render moved out of ingest or cached on `(entry_id, expanded, theme)` (rules 4, 5). Its **`L` effort is this ADR's cost**, not the item's. |
| EXT-019 | `registerMarkdownTransformer(transformer)` (`types.ts:1292` @v0.84.1) | registration import + `transform-markdown` export + host fold (rule 4) |
| EXT-022 | `refreshModels?(context)` (`types.ts:1448` @v0.83.0 / `:1469` @v0.84.1), whose `context.publish({persist})` is a method on a passed object | export + a `publish` import (rules 4, 5) |
| EXT-052 | `streamSimple?: (model, context, options) => AssistantMessageEventStream` (`types.ts:1437`); `options.onPayload`/`onResponse` are callbacks | the stream half is already ported (`provider-stream.emit-event`); the **callback half** must become `on-payload`/`on-response` imports (rule 4) |
| EXT-S04 | `ctx.compact`'s `onComplete`/`onError` (`interactive-mode.ts:1819-1829`) | already acknowledged in-world at `world.wit:554-557` — "cannot cross the component boundary as function values". Rule 7 makes the *substitution* mandatory to complete: `control.compact`'s `result<_, string>` must distinguish vetoed / errored / produced-nothing. |
| EXT-034, EXT-057 | not the encoding — the **sibling** constraint of the same mechanism: single-instance reentrancy forbids re-entering the emitting guest (`world.wit:469-474`) | deferred fan-out is legal under this ADR; **silent loss at the round bound is not**. Rule 8 applies. |

**Mixed — split the ID; one half is an ADR-0002 consequence, the other is plain.** The last row is
here because it is routinely mis-attributed, not because it has an ADR half:

| ID | ADR-0002 half | plain half |
|---|---|---|
| EXT-021 | `onTerminalInput(handler): () => void` (`types.ts:145`) and `setEditorComponent`/`getEditorComponent` (`:260`, `:263`) — a live `EditorFactory`, and in the *getter* direction the host returning a function **to** the guest, which has no representation at all and must be re-specified (e.g. "is an editor component installed, and by whom") or delta'd under rule 7 | `setWorkingVisible` `:154`, `setWorkingIndicator` `:164`, `setHiddenThinkingLabel` `:167`, `setWorkingMessage` `:151`, `getAllThemes` `:269`, `getTheme(name)` `:272` — six plain data verbs, all imports, **no bump** |
| EXT-045 | `ctx.signal: AbortSignal` (`types.ts:334`) → `is-run-cancelled(): bool` poll (rule 6) | `scopedModels` (`types.ts:326`) is a plain read-only snapshot; **and the missing `CYRUP-DELTA` notes are now a rule-7 compliance defect, not a nicety** |
| EXT-050 | `EventBus.on(channel, handler): () => void` returns an unsubscribe closure (`event-bus.ts:5`, `:27`) — hence `unsubscribe(topic)` as an explicit import | the `assertActive` stale-context guard (`loader.ts:413-421` @v0.84.1) is plain upstream-drift |
| EXT-013 | `AutocompleteProviderFactory = (current) => provider` (`types.ts:124`) is a live wrapping chain; cyrup's `autocomplete-suggest(base-json, query-json)` fold (`world.wit:112`) is **this ADR's answer, already designed** | the deadness is plain wiring — `has_arg_completion: false` hardcoded at `cyrup-tui/src/commands.rs:348`, and neither `command_completions` nor `argument_completions` has a production caller |
| EXT-047 | only `setWidget`'s component-factory overload (`types.ts:171`) | `key`, `content: string[] \| undefined` and `placement` (`types.ts:170`, `:104-110`) are **plain data that was collapsed into one opaque blob for no mechanism reason** — the item is a mis-shaped port, not an ADR cost |
| EXT-024 | **none — this row belongs in the unrelated list** | both halves plain: `renderShell?: "default" \| "self"` (`types.ts:465`) is an enum; `constrainedSampling` (`:463`) is provider-side request config; `render_kind`'s zero consumers is cyrup-original |

**Unrelated to ADR-0002 — plain unported behaviour, serde-representable, the encoding is not the
reason:** EXT-003, EXT-007, EXT-011, EXT-014, EXT-015, EXT-016, EXT-017, EXT-018, EXT-025, EXT-026,
EXT-027, EXT-028, EXT-029, EXT-030, EXT-031, EXT-032, EXT-033, EXT-035, EXT-036, EXT-037, EXT-038,
EXT-039, EXT-040, EXT-041, EXT-042, EXT-043, EXT-044, EXT-046, EXT-048, EXT-049, EXT-051, EXT-053,
EXT-054, EXT-055, EXT-056 — **and EXT-024**, per the last row above. 35 + 1 unrelated, 9 pure
consequences, 5 genuinely mixed: 50 rows, the whole of area 06's open table.

Three of these deserve a note because they *look* like ADR costs and are not:

- **EXT-044 / EXT-043 / EXT-016** (`ctx.cwd`, `project_trust` cwd, `resources_discover` payload).
  `cwd` is a `string` on pi's base context (`types.ts:315`). The field→getter transformation is this
  ADR's form (rule 5), but the **omission** is plain: the host holds the value four lines from the
  dispatch (`cyrup-session-svc/src/builder.rs:1646`) and does not pass it. Do not let "it's a WASM
  thing" explain these away.
- **EXT-035** (a native reaches 5 of 11 registration verbs). This is rule 10 being violated in the
  *native* direction — the tier that ships is the weaker one. It is plain missing code in `InitApi`
  (`native.rs:230-297`), not an encoding cost.
- **EXT-054 / EXT-055** (the inert capability grant). Nothing to do with encoding. `06-cyrup-ext.md:178`
  lists ADR-0002 among the three places that document the capability-scoped sandbox; that citation is
  **wrong** and should be re-pointed at `manifest.rs:2` and `host/store_state.rs:1-3` alone. This ADR
  makes no claim about capability scoping.

### Batch by batch

- **Batch 2.** ADR-0002 now resolves to a readable file. Of the six citations, five are accurate as
  written; `Cargo.toml:70`'s `R-ARCH-EXT-010` still resolves to nothing and should be replaced with a
  pointer to this file (rule 1/rule 10), or deleted. Area 06's ADR-0002 citation at `:178` is
  mis-pointed (above). This closes ADR-0002's share of OQ-6 and leaves `arch-08`, `arch-00 §2.1`,
  `R-ARCH-EXT-003/008/011/012/014/015/016/017` and `R-08-NNN` still unreadable — they are OQ-6's
  remainder, not this ADR's.
- **Batch 17.** Unaffected in scope. One instruction: seeding `GuestState` from the manifest must not
  introduce a host reference into `GuestState` that the guest can reach — the grant is data, the
  enforcement is host-side.
- **Batch 18.** Its branch condition "if it yields WIT-shaped items, batch 19 is not safe to close"
  gains a test: a finding is WIT-shaped if closing it needs a **new export** under rule 3 or 4. A
  finding that only needs a new import is additive and can land after 19 without a second bump.
- **Batch 19 (the `0.5.0` bump).** Every rule-3 and rule-4 item in the tables above is a **member by
  construction**, because each needs an export: EXT-009, EXT-023, EXT-006, EXT-S04, EXT-021's
  `onTerminalInput` half. The plain halves of EXT-021, EXT-045 and EXT-050 are **imports** and need no
  bump — splitting those three IDs is what lets batch 19 shed work rather than absorb it. Batch 19's
  own note already anticipates this for `on_terminal_input` (`PARITY-PLAN.md:918-919`); this ADR
  settles it: `onTerminalInput` is a guest export and belongs in 19, or it needs a written
  `CYRUP-DELTA` under rule 7.
- **Batch 20.** EXT-019, EXT-022, EXT-052 and EXT-050's unsubscribe half all need round-trips
  designed before they are estimated; EXT-006 and EXT-013's provider-stacking half are already
  designed and need wiring only.
- **Batch 3 (the lying-control detector).** Gains a rule-7 conformance check: a two-part
  `CYRUP-DELTA` note must exist for every knowingly-unported pi symbol. It is **not a separate lint**
  — it is a check inside ADR-0008's `cargo xtask lint-citations`, which already scans for the same
  marker; shipping two passes over the same tokens is how they drift apart. Today there are zero
  notes in `crates/cyrup-ext/src/lib.rs`, so its first run reports EXT-045 and EXT-021's expensive
  half. Batch 3 collects deliverables from five ADRs in this batch; the consolidated list is in
  `docs/adr/README.md`.

### New work implied — file these; no existing item covers them

1. **Silent encode/decode degradation (rule 8).** `host/live.rs:1472/:1490/:1494/:1551/:1562-1563`
   substitute `"[]"`/`"null"` for a failed encode; `decode_outcome` (`:1645-1651`) turns an
   unparseable patch into `Noop`. A guest whose patch fails to parse is indistinguishable from one
   that declined to act — a silent wrong-output class that pi structurally cannot have. Suggested
   severity **medium**, effort **S**, kind `cyrup-original`, owner batch 20.
2. **Splitting EXT-021, EXT-045, EXT-050 and EXT-013** into their ADR and plain halves, so batch 19's
   member list and batch 20's estimates are derived from the right rows.
3. **Re-pointing the ADR-0002 citation at `06-cyrup-ext.md:178`** away from the capability-sandbox
   claim it does not support.

---

## Rejected alternatives

**A. Give the world WIT `resource` types so a guest can hold a host object.** The Component Model
does support it, so this is not impossible — it is wrong. It does not recover pi's semantics: pi's
`before_provider_headers` works because the host *observes the same object* the handler wrote; a
resource method is still a host call, so you write the round-trip anyway and pay for it twice. Cost of
taking it: a per-resource lifetime contract in the ABI, explicit drops, host state leaked by any
guest that traps before dropping, and a `HOST_WORLD` surface that grows with every host type. Note the
**degenerate** form is already adopted where it is genuinely right — `u32` handles for `proc` and
`http-client` streams (`world.wit:392-413`, `:423-460`), for things that are host-owned and long-lived by nature.
That is the correct scope for handles, and widening it is what is rejected.

**B. Embed a JS/TS runtime and run pi's extensions unmodified.** This is the only option that would
give true mechanism parity, and it is rejected on a **stated project constraint**, not on effort: the
port is Rust-only and does not take a JavaScript/TypeScript runtime dependency. It also destroys the
property the WASM tier exists for — a jiti-style in-process guest with live host references is not
sandboxed, not capability-scoped, and not preemptible, so batch 17's entire subject would become
unimplementable. Cost of taking it: every extension becomes trusted code in the host process.

**C. Typed WIT records for every payload instead of JSON strings.** Cost: pi's payloads are
genuinely open-shaped — `details` blobs, tool `parameters` (JSON-Schema, pinned for interop at
`registry.rs:13`), custom message payloads, provider request bodies, option bags — so closing them
would (i) break interop with pi-authored schemas and (ii) make every upstream field addition an ABI
break and a `HOST_WORLD` bump, against an upstream that ships a minor every few weeks. Partially
adopted: it is exactly what the world already does for **fixed-shape** values (`exec-result`,
`tool-descriptor`, `http-request`/`http-response`, `hook-outcome`'s discriminant), and rule 1 keeps
that split.

**D. Accept the omissions the encoding makes awkward ("accepted divergence").** This is the category
the project does not have, and the concrete cost is legible: EXT-009, EXT-021's expensive half,
EXT-S04, EXT-052 and EXT-045's `signal` half would close by fiat, and with them an auth-shim or
proxy-tagging extension, a terminal-input extension, a compaction-driving extension, and every
third-party provider's visibility to every other extension. Each fails **silently**, keyed on user
configuration. Rejected under the parity rule; the behavioural cost stays on the backlog as work.

**E. A native-only escape hatch — let built-ins hold live references, hold only guests to serde.**
Superficially cheap: `cyrup-ext-subagents` is in-process and could just take an `Arc<AgentSession>`.
Cost: two extension semantics, and a behaviour class no guest could ever have, so the WIT would stop
being the specification of what an extension can do and would drift unobserved. EXT-035 is already
that drift in the opposite direction (natives weaker than the world). Rejected — `native.rs:324`
already implements rule 10 and should not be relaxed.

---

## How to reverse this

> *"Extensions may hold live host references — give the world WIT resource handles (or run guests
> in-process with shared memory), and stop paying for round-trips."*

What would have to change: `world.wit` gains `resource` types with an explicit drop/lifetime
contract and `HOST_WORLD` goes to `cyrup:ext@1.0`; `NativeExtension::on_event`'s owned-data payload
(`native.rs:324`) becomes a borrow of live host state; `contract.rs`'s `EventPatch` fold is replaced
by in-place mutation; and every row classified above as an **ADR-0002 consequence** must be
re-derived — most of them stop being round-trip design work and become plain field-carrying, which
would shrink batch 19 and grow the host's trust surface by exactly the same amount. Reversal is
cheapest **before** batch 19 fixes the `0.5.0` ABI; after it, every already-built guest is
invalidated.
