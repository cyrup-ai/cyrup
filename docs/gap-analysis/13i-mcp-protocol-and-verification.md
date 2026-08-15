# 13i · Sampling, elicitation, tracing and verification

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, which holds the thesis, the seam map, the
> architecture and the one canonical table of every port unit. Method and phasing are in
> **[MCP-PORT-METHODOLOGY.md](MCP-PORT-METHODOLOGY.md)**.

*Upstream is `pi-mcp-adapter` v2.25.0. cyrup is branch `david/cyrup`. rmcp is the checkout at
`/Users/davidmaple/cyrup.ai/rmcp` (`rmcp-v3.1.2-7-gf713ebd`).*

This section covers the two places where an MCP **server** issues a request back at the **client** —
`sampling/createMessage` ("run a model completion on my behalf") and `elicitation/create` ("ask your
human a structured question") — the metadata-only wire tracer that observes both, and the verification
apparatus the port inherits and the one it has to build.

**The shape of the answer: none of it touches cyrup's core.** Both handlers are methods on a trait
`cyrup-mcp` implements (`rmcp::handler::client::ClientHandler`), and everything they reach for is
either a dependency the native crate links directly or a `HostServices` verb that already exists.
Sampling reaches `cyrup-provider` the same way `pi-mcp-adapter` reaches `@earendil-works/pi-ai/compat`
— *directly*, bypassing the host API — which is what upstream does and therefore what fidelity
requires; a host verb here would be a divergence, not a fix. Elicitation lands on
`HostServices::{select, input, confirm, notify}`, which match pi's `ExtensionUIContext` verb for verb,
down to `select` taking a JSON array of option *strings* and returning the chosen *string*. Tracing is
a `rmcp::transport::Transport` decorator written inside `cyrup-mcp`. The one cyrup-side surface either
handler needs and cyrup already has is the human-wait machinery — `HostCtx::begin_human_wait` and
`HostServices::human_interaction_lock` — which exists because the permission gate needed it first.

**rmcp carries far more of this than the first pass assumed, and three of its assumed gaps are not
gaps.** The types are not a thin wire mirror: `ElicitationSchema` deserialises through an `IndexMap`
and keeps a `property_order: Option<Vec<String>>`, so the document order of `requestedSchema.properties`
— which drives the question order, the review-row order and the edit-picker order, four user-visible
orderings from one read — **survives for free**; the first pass filed a `serde_json/preserve_order`
prerequisite for it, and that prerequisite does not exist. `PrimitiveSchemaDefinition` is a closed,
typed decomposition (`StringSchema` with a `StringFormat` enum, `NumberSchema`, `IntegerSchema`,
`BooleanSchema`, and `EnumSchema` in single/multi × titled/untitled flavours), so the schema-sniffing
half of `collectField` and all of `extractMultiSelectOptions` become a `match` — and the `enumNames`
asymmetry the first pass warned about ("do not unify the two choice builders") is now enforced by the
type system rather than by discipline. `ElicitRequestParamsWire::LegacyForm` is an untagged fallback
arm, so a params object with an absent or unrecognised `mode` deserialises as **form**, exactly like
upstream's `params.mode === "url" ? url : form`. `ErrorData::invalid_params` is `-32602` on the nose.
And `ElicitationAction::{Accept, Decline, Cancel}` is native, with the trait's **default**
implementation returning `Decline` — an unimplemented handler is fail-safe by construction.

**The tracer's one genuinely unportable mechanism turns out not to matter.** Upstream's
`wrapTransportWithMcpTrace` deliberately returns the *same object* it was handed, `defineProperty`-ing
a getter/setter over `onmessage` and reassigning `send` in place, because SDK v2 sniffs the concrete
transport type before connect and a wrapper object would make *enabling tracing change protocol
negotiation*. Rust has no monkey-patching, and the first pass filed that as the section's largest open
question. It is settled: rmcp's `serve_client_with_ct_inner` is generic over `T: Transport<RoleClient>`
and never inspects a concrete transport type, and `ClientLifecycleMode::{Initialize, Discover, Auto}`
probes `server/discover` over the *same* transport rather than a disposable sibling process. A
`TracingTransport<T>` newtype is safe. The one real consequence is narrow and nameable:
`DynamicTransportError::{is, downcast}` key on `TypeId::of::<T>()` and `T::name()`, so wrapping changes
the *error* identity and any downcast must target `TracingTransport<T>` or unwrap the inner error.

**On verification, the single most valuable fact in this section is that rmcp already does it.**
`@modelcontextprotocol/conformance` can be pointed at a Rust binary — not as an inference, but because
`/Users/davidmaple/cyrup.ai/rmcp` ships `conformance/src/bin/client.rs` (1 149 lines) and a
`.github/workflows/conformance.yml` that runs `npx @modelcontextprotocol/conformance@0.2.0-alpha.10
client --command "$(pwd)/target/debug/conformance-client" --suite all --spec-version 2025-11-25` and
again for `2026-07-28`, with **no `--expected-failures` file on either versioned suite**. Two
consequences reframe the whole verification plan. First, `cyrup-mcp`'s *protocol* conformance is
largely gated upstream by rmcp's own CI, so cyrup's conformance job is about the **adapter's** wiring —
OAuth credential storage, the multi-tenant callback router, the elicitation UI — not about the wire.
Second, all five entries in upstream's `conformance/baseline-client.yml` are scenarios rmcp's own
conformance client implements and does not baseline; because the CLI **fails a run on a stale baseline
entry** (a listed scenario that starts passing), copying that file is not a safe starting point. The
port starts from an empty baseline and writes it from observed failures.

---

### How it lands

| adapter capability | upstream mechanism | cyrup mechanism | verdict |
|---|---|---|---|
| receive `sampling/createMessage` | `registerSamplingHandler` → `client.setRequestHandler` (`server-manager.ts`) | `ClientHandler::create_message` | **`rmcp`** |
| advertise `sampling: {}` | `buildClientCapabilities` (`server-manager.ts`) | `ClientCapabilities.sampling: Option<SamplingCapability>`; `Default` serialises to `{}` | **`rmcp`** |
| reject sampling *tasks* | `"task" in params` guard (`sampling-handler.ts`) | structural: never declare `io.modelcontextprotocol/tasks` in `ClientCapabilities.extensions` (`ClientCapabilities::supports_tasks`) | **`rmcp`** |
| reject includeContext / tools / toolChoice / stopSequences | four guards (`handleSamplingRequest`) | `match` on the typed `Option` fields of `CreateMessageRequestParams` | **`hand-written`** |
| candidate model list from `modelPreferences.hints` | `resolveSamplingModel` over `ModelRegistry.getAvailable` | `cyrup_provider::catalog::{builtin_catalog, load_catalog}` + `HostServices::{models, scoped_models, current_model}` | **`extension-owned`** + **`host-verb`** |
| per-model auth probe | `ModelRegistry.getApiKeyAndHeaders` | `cyrup-provider`'s `Models::get_auth` | **`extension-owned`** |
| run the completion | `complete()` from `@earendil-works/pi-ai/compat`, imported directly | `cyrup-provider`'s completion path (`Models::complete`), linked directly — **no host verb exists and none is wanted** | **`extension-owned`** |
| two approval dialogs + `samplingAutoApprove` | `confirmSampling` / `formatRequestApproval` / `formatResponseApproval` | `HostServices::confirm` under `HostServices::human_interaction_lock` | **`host-verb`** |
| message + result conversion | `convertSamplingMessage` / `convertAssistantResult` / `mapStopReason` | over `SamplingMessage`, `SamplingContent::{Single, Multiple}`, `SamplingMessageContentBlock`, `CreateMessageResult` | **`hand-written`** |
| abort checkpoints (×3) | `throwIfAborted(getSignal())` | `HostServices::is_run_cancelled` + a `tokio_util::sync::CancellationToken` child | **`host-verb`** + **`hand-written`** |
| receive `elicitation/create` | `registerElicitationHandler` | `ClientHandler::create_elicitation` (default impl returns `Decline`) | **`rmcp`** |
| advertise `elicitation: {form, url?}` | `buildClientCapabilities`; `allowUrl === (mode === "tui")` | `ElicitationCapability::{with_form, with_url}`, `FormElicitationCapability::with_schema_validation` | **`rmcp`** + **`host-verb`** (`HostConfig.mode`) |
| mode dispatch, absent/unknown `mode` → form | `params.mode === "url" ? … : form` | `ElicitRequestParams` via `ElicitRequestParamsWire::LegacyForm` (untagged fallback) | **`rmcp`** |
| document order of `properties` | `Object.entries` insertion order | `ElicitationSchema::property_order`, filled from an `IndexMap` at deserialise | **`rmcp`** |
| per-primitive widget matrix | `collectField`'s five branches | `match` on `PrimitiveSchemaDefinition` → `HostServices::{select, input, confirm}` | **`rmcp`** (shape) + **`host-verb`** (widgets) |
| coercion, limits, re-prompt | `coerceAndValidateFormValues`, `collectValidField` | hand-written over the typed schema fields; JS `Number()` semantics reproduced explicitly | **`hand-written`** |
| final schema assertion incl. `format` | ajv (`AjvJsonSchemaValidator`, `addFormats`, `validateFormats`) | `jsonschema` with `.should_validate_formats(true)` — **rmcp validates nothing client-side** | **`hand-written`** |
| URL mode: gate, parse, scheme allowlist, dialog, open | `handleUrlElicitation` | `ElicitRequestParams::UrlElicitationParams` + `url::Url` + `HostServices::{select, notify}` + `opener` | **`hand-written`** + **`host-verb`** + **`extension-owned`** |
| `-32602` for the three URL rejections | `ProtocolError(ProtocolErrorCode.InvalidParams, …)` | `ErrorData::invalid_params(msg, None)` → `ErrorCode::INVALID_PARAMS` | **`rmcp`** |
| decline / cancel outcomes | `{action:"decline"｜"cancel"}` | `ElicitationAction::{Decline, Cancel}`; `ElicitResult::with_content` | **`rmcp`** |
| `notifications/elicitation/complete` + dedupe | `client.setNotificationHandler` + `acceptedUrlElicitations` | `ClientHandler::on_custom_notification` (rmcp models no first-class variant) + `HashMap<String, HashSet<String>>` + `HostServices::notify` | **`rmcp`** + **`hand-written`** |
| batch `UrlElicitationRequiredError` (`-32042`) | `McpServerManager.handleUrlElicitationRequired` | decode `ErrorData { code: -32042, data }` yourself — rmcp has no such type | **`hand-written`** |
| hold the dispatcher budget across a human answer | *(no upstream analogue)* | `HostCtx::begin_human_wait` + `HostServices::human_interaction_lock` | **`host-verb`** |
| trace event schema v1 (JSONL) | `createMcpTraceEvent` | `#[derive(Serialize)]` struct, declaration order = wire order | **`hand-written`** |
| redaction | `redactTraceText` | `regex` + four `LazyLock<Regex>` | **`hand-written`** |
| bounded, latching, never-throwing writer | `McpTraceWriter` | local `TraceWriter` in `cyrup-mcp`, injectable fs | **`hand-written`** |
| transport instrumentation | `wrapTransportWithMcpTrace` (in-place mutation) | `TracingTransport<T>` newtype over `rmcp::transport::Transport<RoleClient>` | **`hand-written`** |
| protocol conformance gate | `conformance/{run.sh,driver.sh,driver.ts,baseline-client.yml}` | `@modelcontextprotocol/conformance` client suite against a hidden `cyrup` subcommand; rmcp's `conformance/` crate is the working reference | **`hand-written`** |
| the vitest / node:test suites | 96 + 5 `*.test.ts` | in-crate `#[cfg(test)]` plus `cyrup-it` for the four seams | **`hand-written`** |

---

### Behavioural specification

#### 10.1 Sampling — `sampling/createMessage`

##### 10.1.1 Registration, capability advertisement, and what changes under rmcp

Upstream constructs its `Client` in `McpServerManager.createClient` (`server-manager.ts`) on **every**
connect attempt, and the ordering is load-bearing and test-pinned:

1. `buildClientCapabilities()` returns `{ ...(samplingConfig ? {sampling:{}} : {}),
   ...(elicitationConfig ? {elicitation:{form:{}, ...(allowUrl ? {url:{}} : {})}} : {}) }`.
2. The object is spread into the `Client` options **only when non-empty**, so a manager with neither
   config advertises **no `capabilities` key at all** — `server-manager-sampling.test.ts` asserts
   `expect(client.options).not.toHaveProperty("capabilities")`.
3. `registerSamplingHandler` then `registerElicitationHandler`, both inside `createClient`, which
   returns to a caller that has not yet called `connect()`. The same test asserts
   `setRequestHandler.mock.invocationCallOrder[0] < connect.mock.invocationCallOrder[0]`.

Under rmcp the **ordering hazard disappears structurally**: the handler *is* the service, and
`ClientHandler::get_info() -> ClientInfo` supplies `ClientCapabilities` at service construction, so a
sampling request arriving in the first millisecond after `initialize` cannot find an unregistered
handler. Two deltas to record rather than fix:

* **rmcp always emits `"capabilities": {}`.** `ClientCapabilities` is a plain struct on
  `InitializeRequestParam`; every field is `skip_serializing_if = "Option::is_none"`, so an
  all-`None` value serialises as `{}` but the key is present. Upstream omits the key. The MCP spec
  requires `capabilities` in `initialize` params, so cyrup is the more conformant of the two. Accepted
  delta; do not contort rmcp to reproduce the omission.
* **`CreateMessageRequestParams` is `#[deprecated]` in rmcp** (SEP-2577 deprecates sampling), as are
  `SamplingMessageContentBlock` and the sampling capability. The module implementing this handler needs
  `#![expect(deprecated)]`, exactly as `rmcp::handler::client` itself does. That is a lint concern, not
  a functional one — the wire behaviour is unaffected.

Enablement, from `init.ts`:

```
samplingAutoApprove = config.settings?.samplingAutoApprove === true
if (config.settings?.sampling !== false && (hasUI || samplingAutoApprove)) manager.setSamplingConfig({
  autoApprove: samplingAutoApprove,
  ...(ui !== undefined ? { ui } : {}),
  modelRegistry,                                   // ctx.modelRegistry, snapshotted pre-await
  getCurrentModel: () => owner.isActive() ? ctx.model : undefined,
  getSignal:       () => owner.isActive() ? combineAbortSignals(owner.signal, ctx.signal)
                                          : owner.signal,
})
```

The two closures read `ctx` **live**, not the snapshot. `init-elicitation.test.ts` pins that a model
swap mid-session is observed by a later sampling request, that the returned signal tracks the *current*
turn's signal, and that after `owner.stop("reload")` the model reads `undefined` and the signal reads
aborted. Sampling is therefore always cancellable by the turn that is running when the server asks.

`SamplingUIContext` is `Pick<ExtensionUIContext, "confirm">` — sampling touches **only** `confirm`,
never `select`/`input`/`notify`. Keep that surface narrow in the port.

##### 10.1.2 `handleSamplingRequest` — the exact sequence

| # | step |
|---|---|
| 1 | `signal = options.getSignal(); throwIfAborted(signal)` |
| 2 | five parameter rejections, in this order, each `throw new Error(...)` |
| 3 | `messages = params.messages.map(convertSamplingMessage)` — raises the sixth rejection |
| 4 | `{model, apiKey, headers} = await resolveSamplingModel(options, params.modelPreferences)` |
| 5 | `throwIfAborted(signal)` |
| 6 | approval gate #1 — title `"Approve MCP sampling request"` |
| 7 | `throwIfAborted(signal)` |
| 8 | `complete(model, {systemPrompt?, messages}, {apiKey?, headers?, maxTokens, temperature?, metadata?, signal?})` |
| 9 | `converted = convertAssistantResult(result)` |
| 10 | `throwIfAborted(signal)` |
| 11 | approval gate #2 — title `"Return MCP sampling response"` |
| 12 | `return converted` |

The five parameter rejections, byte-exact, checked in this order:

| condition | message |
|---|---|
| `"task" in params && params.task` | `MCP sampling tasks are not supported` |
| `params.includeContext && params.includeContext !== "none"` | `MCP sampling context inclusion is not supported` |
| `params.tools?.length` | `MCP sampling tool use is not supported` |
| `params.toolChoice` | `MCP sampling tool choice is not supported` |
| `params.stopSequences?.length` | `MCP sampling stop sequences are not supported` |

…and a sixth raised from inside message conversion (step 3, which runs *before* model resolution):
`convertUserContent` / `convertAssistantContent` accept only `type === "text"` and otherwise throw
`MCP sampling ${block.type} content is not supported` /
`MCP sampling assistant ${block.type} content is not supported`.
`sampling-handler.test.ts` pins the `image` and `audio` forms and asserts `complete` is never called.

**The `task` guard becomes structural under rmcp and this must be understood, not transcribed.**
`CreateMessageRequestParams` has no `task` field. Task augmentation is the SEP-2663
`io.modelcontextprotocol/tasks` extension, negotiated through `ClientCapabilities.extensions` and
queried with `ClientCapabilities::supports_tasks`. A conformant server cannot task-augment a request to
a client that never declared the extension, and a stray `task` key in params is dropped by serde
(`CreateMessageRequestParams` does not `deny_unknown_fields`). So the port reproduces the *guarantee*
by never declaring the extension, and the error string has no live throw site. Record the string in the
error taxonomy anyway so a later pass that adds the tasks extension knows what the adapter's policy was.

`max_tokens` is a required `u32` on `CreateMessageRequestParams` and is passed through **unmodified and
unclamped** — the adapter neither validates nor caps it. `temperature` (`Option<f32>`) and `metadata`
(`Option<Value>`) are conditionally spread only when defined; `metadata` is handed to the provider
verbatim. Note `cyrup-provider`'s `StreamOptions` has no metadata field; carrying `metadata` through is
an open decision, recorded below.

##### 10.1.3 `resolveSamplingModel` — candidate ordering and the auth probe

The most behaviourally load-bearing function in the file; seven dedicated tests. Candidates are appended
through `addSamplingCandidate`, which dedupes on `(provider, id)` — **first insertion wins**, so
ordering is stable:

1. For each `hint` in `modelPreferences?.hints ?? []`, in array order:
   `normalizedHint = hint.name?.trim().toLowerCase()`; skip if falsy. Then for **each** model in
   `availableModels` **in registry order**, match if any of
   `` [`${model.provider}/${model.id}`, model.id, model.name] `` lowercased **contains** the hint as a
   substring (`.includes` — not equality, not prefix). All matches for hint *n* are appended before any
   match for hint *n+1*; `sampling-handler.test.ts` pins that `hints: [{name:"gemini"},{name:"haiku"}]`
   selects the gemini model.
2. `options.getCurrentModel()`, if any.
3. Every model in `availableModels`, in registry order.

Then, for each candidate in order: `throwIfAborted(signal)`;
`auth = await modelRegistry.getApiKeyAndHeaders(model)`; `throwIfAborted(signal)`. If `auth.ok === false`,
push `` `${model.provider}/${model.id}: ${auth.error}` `` onto `errors` and continue. Otherwise **return
immediately** with `{model, apiKey?, headers?}`, each conditionally spread only when not `undefined`.

**`options.getSignal()` is called a second time inside `resolveSamplingModel`**, independently of the
read at entry. Because the closure resolves `ctx.signal` live, the signal governing the auth probe can
be a *different* composite from the one governing the outer sequence. A port that captures one token at
entry and reuses it diverges when the turn rolls over mid-request; call the accessor twice, as upstream
does.

Exhaustion:

* `errors.length > 0` → `` throw new Error(`No configured auth for MCP sampling model. ${errors.join("; ")}`) ``
* otherwise → `throw new Error("No Pi model is available for MCP sampling")`

`sampling-handler.test.ts` pins that a hinted model whose auth fails is *probed first*
(`getApiKeyAndHeaders` nth-called-with `haiku` then `opus`) and then skipped, and that the next candidate
is used.

**Where the candidate set comes from in cyrup.** Upstream's `getAvailable()` spans **every** configured
provider, not one. The seam map settles this: the model set comes from
`cyrup_provider::catalog::{builtin_catalog, load_catalog}`, read by `cyrup-mcp` directly, with
`HostServices::{models, scoped_models, current_model}` supplying the session's own view so that
`modelPreferences` resolution starts from what the session actually has. `HostServices::models()` alone
is **not** the right source — it is one provider's catalogue, which is the narrower set — and the first
pass was right about that even though it drew the wrong conclusion (that a new host verb was needed).

##### 10.1.4 The two approval gates

`confirmSampling`:

```
if (options.autoApprove) return;
if (!options.ui) throw new Error(
  "MCP sampling requires interactive approval. Set settings.samplingAutoApprove to true to allow it without UI.");
const approved = await options.ui.confirm(title, message);
if (!approved) throw new Error("MCP sampling request was declined");
```

Both gates use the **same** decline message regardless of which gate declined. Because the `ui` here is
the owner-fenced proxy (`createOwnedUi` makes every UI method return `undefined` once the runtime owner
has stopped), a stopped runtime yields `undefined` from `confirm` and therefore the decline throw —
fail-closed. cyrup's `HostServices` defaults have the same polarity (`confirm` → `false`, `input` →
`None`, `select` → `None`), so the fail-closed contract is already in the type system.

The **no-UI** case is not `confirm` returning false: upstream distinguishes "no `ui` at all" (a different,
actionable message naming the setting) from "user said no". In cyrup the analogue of "no ui" is having no
UI sink attached, which is unobservable through `confirm`'s return. Carry an explicit `has_ui: bool` in
the sampling options, sourced from `HostConfig.has_ui`, to preserve the two distinct messages.

`formatRequestApproval` — `lines` joined with `"\n\n"`:

* line 0: `` `${serverName} wants to sample ${n} message${n === 1 ? "" : "s"} with ${provider}/${id}.` ``
* if `systemPrompt` truthy: `` `System: ${truncateAtWord(systemPrompt, 400)}` ``
* for each message, 1-indexed: `` `${i+1}. ${message.role}: ${truncateAtWord(messageText(message), 400)}` ``

`messageText`: a string content passes through; an array maps per block and joins with `"\n"` —
`text` → `block.text`, `image` → `` `[image: ${block.mimeType}]` ``, `thinking` → `"[thinking]"`,
`toolCall` → `` `[tool call: ${block.name}]` ``, anything else → `"[content]"`.

`formatResponseApproval`:
`` `${serverName} will receive this response from ${response.model}:\n\n${truncateAtWord(text, 1000)}` ``
where `text` is `response.content.text` for text content, else `` `[${type} content]` ``.

`truncateAtWord(text, target)` (`utils.ts`) — the exact algorithm:

```
if (!text || text.length <= target) return text;
truncated = text.slice(0, target);
lastSpace = truncated.lastIndexOf(" ");
if (lastSpace > target * 0.6) return truncated.slice(0, lastSpace) + "...";
return truncated + "...";
```

`text.length` is **UTF-16 code units**, not chars or bytes, and `" "` is a literal ASCII space (no other
whitespace). Verified against node: `truncateAtWord("aaaa bbbb cccc dddd", 10)` → `"aaaa bbbb..."`;
`truncateAtWord("aaaaaaaaaaaaaaa b", 10)` → `"aaaaaaaaaa..."`. A Rust port operating on `char` counts
diverges on astral-plane content; one operating on bytes diverges on any non-ASCII. Note the terminator
is three ASCII periods here, and U+2026 in `redactTraceText` — the two are different on purpose.

##### 10.1.5 Result conversion

`convertSamplingMessage`. `blocks = Array.isArray(content) ? content : [content]` — the MCP wire allows
a single block **or** an array; **rmcp already models this** as `SamplingContent<T>::{Single, Multiple}`,
so the normalisation is a `match`, not a shape test. A `user` message becomes
`{role:"user", content: blocks.map(convertUserContent), timestamp: Date.now()}`. An `assistant` message
becomes a full synthetic pi `AssistantMessage` with the **literal** sentinel fields
`api: "mcp-sampling"`, `provider: "mcp"`, `model: "sampling-request"`, `usage: zeroUsage()`,
`stopReason: "stop"`. `zeroUsage()` is all-zero
`{input, output, cacheRead, cacheWrite, totalTokens, cost:{input, output, cacheRead, cacheWrite, total}}`.

`convertAssistantResult`:

1. `stopReason === "error"` → throw `message.errorMessage ?? "MCP sampling model call failed"`.
2. `stopReason === "aborted"` → throw `message.errorMessage ?? "MCP sampling model call was aborted"`.
3. Map content: `text` → its text; `thinking` → **dropped**; anything else → throw
   `` `MCP sampling result ${block.type} content is not supported` ``. Survivors joined with `"\n\n"`,
   then `.trim()`.
4. Empty result → throw `"MCP sampling result did not contain text content"`.
5. Return `` {role:"assistant", content:{type:"text", text}, model: `${provider}/${model}`,
   stopReason: mapStopReason(...)} ``.

`mapStopReason`: `stop`→`endTurn`, `length`→`maxTokens`, `toolUse`→`toolUse`, everything else passed
through unchanged. rmcp's `CreateMessageResult` carries `stop_reason: Option<String>` with named
constants `STOP_REASON_END_TURN` / `STOP_REASON_END_MAX_TOKEN` / `STOP_REASON_TOOL_USE` /
`STOP_REASON_END_SEQUENCE`; the first three are exactly the mapping's outputs, and
`CreateMessageResult::validate()` enforces SEP-1577's "role must be assistant".

#### 10.2 Elicitation — `elicitation/create`

##### 10.2.1 Enablement, capability, and the URL gate

From `init.ts`:

```
elicitationEnabled = config.settings?.elicitation !== false && hasUI
if (elicitationEnabled && ui) manager.setElicitationConfig({ ui, allowUrl: mode === "tui" })
```

`allowUrl` is **exactly** `mode === "tui"` — not `hasUI`. `init-elicitation.test.ts` pins that RPC mode
gets `allowUrl: false` so "the backend never opens a browser", that TUI gets `allowUrl: true`, and that
no UI *or* `settings.elicitation === false` means `setElicitationConfig` is never called at all and the
`elicitation` capability is therefore never advertised. `elicitation-sdk-integration.test.ts` proves the
shapes over a real SDK server: TUI → `{form:{}, url:{}}`, RPC → `{form:{}}`, read back from a live
fixture via `server.getClientCapabilities()?.elicitation`.

In cyrup, `has_ui` and `mode == Tui` are **different** predicates on `HostConfig` and must stay
different. The capability object is `ElicitationCapability::new().with_form(FormElicitationCapability::new())`
plus `.with_url(UrlElicitationCapability::new())` when `allowUrl` — same wire shape, same conditionality.

`FormElicitationCapability` additionally carries `schema_validation: Option<bool>`
(`with_schema_validation`), which advertises "this client validates responses against
`requested_schema` before sending them". The adapter *does* do that (its ajv pass), so setting it `true`
is a truthful upgrade over upstream's bare `{form:{}}` — and it is what rmcp's own conformance client
sets. Treat it as a deliberate, disclosed addition rather than a silent one.

`elicitation-handler.ts` exports **five** functions: `registerElicitationHandler`,
`handleElicitationRequest`, `handleFormElicitation`, `coerceAndValidateFormValues` and
`handleUrlElicitation`. Only the last two are consumed outside the file (`coerceAndValidateFormValues`
by tests; `handleUrlElicitation` by the manager's `handleUrlElicitationRequired`).

##### 10.2.2 Dispatch

`handleElicitationRequest` is a one-line split: `params.mode === "url" ? handleUrlElicitation(...) :
handleFormElicitation(...)`, so an absent or unknown `mode` is treated as **form**.

**rmcp reproduces this exactly, contrary to the first pass's assumption.** `ElicitRequestParams` is
`#[serde(tag = "mode", try_from = "ElicitRequestParamsWire")]`, and the wire enum's third variant is
`#[serde(untagged)] LegacyForm { meta, message, requested_schema }` — the fallback serde reaches for when
the `mode` tag matches neither `"form"` nor `"url"`, including when it is absent. Both cases land on
`FormElicitationParams`. No leniency needs adding; the earlier "rmcp's typed enum is stricter" note is
withdrawn.

##### 10.2.3 Form mode — the outer loop

`handleFormElicitation`:

1. `properties = Object.entries(params.requestedSchema.properties)` — **JS insertion order**, i.e.
   document order of the JSON object. Every subsequent step indexes off this array; the review rows and
   the edit-picker labels both consume it rather than re-deriving it. In cyrup this is
   `ElicitationSchema::property_order` zipped against `ElicitationSchema::properties`; **iterating the
   `BTreeMap` directly is the bug**, and it is silent.
2. One gate dialog:
   `` ui.select(`MCP Input Request\nServer: ${serverName}\n\n${params.message}`, ["Continue","Decline"]) ``.
   `undefined` → `{action:"cancel"}`; `"Decline"` → `{action:"decline"}`.
3. `properties.length === 0` → `{action:"accept", content:{}}` **immediately**, without a review screen.
   `elicitation-handler.test.ts` pins all three outcomes of the empty form with
   `expect(select).toHaveBeenCalledOnce()`.
4. Collect every field in order via `collectValidField`; any `cancelled` → `{action:"cancel"}`.
5. Review loop, `while (true)`:
   a. `content = coerceAndValidateFormValues(params, values)` — **this can throw and is not caught
      here**; the throw escapes `handleFormElicitation` and becomes a JSON-RPC error response to the
      server instead of an `ElicitResult`. Per-field validation already ran in step 4, so this only
      fires on a cross-field constraint the per-field pass could not see.
   b. `action = ui.select(formatReview(...), ["Submit","Edit","Decline"])`; `undefined`→cancel,
      `"Decline"`→decline, `"Submit"`→`{action:"accept", content}`.
   c. Otherwise (Edit): `` labels = properties.map(([name, schema]) =>
      `${schema.title ?? humanizeName(name)} (${name})`) `` — **not** uniquified, unlike option labels.
      `ui.select("Choose a field to edit", labels)`; `undefined`→cancel;
      `property = properties[labels.indexOf(selected)]` (first match wins on duplicate labels); falsy →
      `continue`, back to the review screen.
   d. Re-collect that one field with `current = values[name]` as the seed, then loop.

`formatReview`: `` [`Review input for ${serverName}`, "", ...rows].join("\n") `` where each row is
`` `${schema.title ?? humanizeName(name)}: ${content[name] === undefined ? "(omitted)" : String(content[name])}` ``.
`String(["Red","Blue"])` is `"Red,Blue"`; `String(true)` is `"true"`.

##### 10.2.4 `collectValidField` — the re-prompt loop

`required = params.requestedSchema.required?.includes(name) === true`. Loop: `collectField(...)`;
`cancelled` propagates. Otherwise re-run `coerceAndValidateFormValues` against a **single-property
synthetic schema**:

```
{ ...params, requestedSchema: { type:"object", properties: { [name]: schema },
                                ...(required ? { required:[name] } : {}) } }
```

On throw: `ui.notify(error.message, "error")`, set `current = result.value`, loop. On success, return.
This is an **unbounded** loop — the only exit other than success is the user dismissing the dialog. Two
consequences for the port: the synthetic schema must copy `params` and replace only `requestedSchema` so
sibling fields (e.g. `message`) survive; and the human-wait guard must wrap the **loop**, not each dialog.

##### 10.2.5 `collectField` — the per-type widget matrix

The dialog title is built once:

```
[schema.title ?? humanizeName(name), required ? "(required)" : "", schema.description]
  .filter(Boolean).join(" ")
```

`filter(Boolean)` drops the empty string, so a non-required field with no description yields just the
label. `elicitation-handler.test.ts` pins `ui.input` being called with `("GitHub username (required)", undefined)`.

| upstream branch | upstream condition | rmcp variant | actions offered, in order | result |
|---|---|---|---|---|
| **enum/oneOf string** | `type === "string" && ("enum" in schema ‖ "oneOf" in schema)` | `PrimitiveSchemaDefinition::Enum(EnumSchema::{Legacy, Single(Untitled｜Titled)})` | `uniqueLabels(choices.map(display))`, then `"Use default"` if `default !== undefined`, then `"Omit"` if not required — the latter two through `uniqueAction` against the accumulating list | chosen → `choices[displays.indexOf(action)]?.value`; default → `schema.default`; omit → `undefined` |
| **boolean** | `type === "boolean"` | `PrimitiveSchemaDefinition::Boolean(BooleanSchema)` | `["Yes","No"]`, `+"Use default"` if default, `+"Omit"` if not required — **not** uniquified (collisions impossible) | `action === "Yes"` |
| **array** | `type === "array"` | `PrimitiveSchemaDefinition::Enum(EnumSchema::Multi(Untitled｜Titled))` | outer: `["Choose values"]`, `+"Use default"`, `+"Omit"`. Then an inner multi-select loop | see below |
| **fallback** | string / number / integer / anything else | `PrimitiveSchemaDefinition::{String, Number, Integer}` | `["Enter value"]`, `+"Use default"`, `+"Omit"` → then `ui.input(title, current === undefined ? undefined : String(current))` | the raw string; `undefined` from `input` → `cancelled` |

`oneOf` choices are `{value: option.const, display: formatChoice(option.const, option.title)}`; `enum`
choices pair `schema.enum[i]` with `schema.enumNames?.[i]` when `"enumNames" in schema`.
`formatChoice(value, title)` is `` title && title !== value ? `${title} (${value})` : value ``.

**The `enumNames` asymmetry, now enforced by rmcp's types.** Upstream's string-enum branch honours
`enumNames`, but `extractMultiSelectOptions` — which serves the *array* branch — does not: for an array
it is `items.anyOf` → `{const, title}` pairs, else `(items.enum ?? []).map(value => ({value, display: value}))`,
i.e. an array of plain `enum` strings has **no** display-name mechanism at all. rmcp encodes exactly this
split: `LegacyEnumSchema` carries `enum_names: Option<Vec<String>>`; `TitledSingleSelectEnumSchema`
carries `one_of: Vec<ConstTitle>`; `UntitledMultiSelectEnumSchema.items` is `UntitledItems { enum_ }`
with no titles, while `TitledMultiSelectEnumSchema.items` is `TitledItems { any_of: Vec<ConstTitle> }`.
A port that unifies the two choice-builders would have to actively defeat the type system to introduce
`enumNames` support the wire never had.

The **array inner loop** is the subtle one:

```
choices  = extractMultiSelectOptions(schema)
selected = new Set(Array.isArray(current) ? current : [])
loop {
  displays = uniqueLabels(choices.map(c => selected.has(c.value) ? `✓ ${c.display}` : c.display))
  done     = uniqueAction("Done", displays)
  picked   = ui.select(title, [...displays, done])
  undefined -> cancelled
  picked === done -> collected [...selected]      // insertion order of the Set
  choice = choices[displays.indexOf(picked)]; !choice -> continue
  toggle choice.value in selected
}
```

`✓` is U+2713 followed by a space. The returned array is the `Set` iteration order — **the order the
user toggled them on**, not schema order.

`uniqueLabels` appends U+2026 `…` repeatedly (a `while`, not an `if`) until unique, tracking a `used`
set. `uniqueAction(label, choices)` appends `…` while `choices.includes(unique)`. Both exist so
`ui.select`'s string-valued return can be inverted back to an index unambiguously — cyrup's
`HostServices::select` has exactly the same string-return contract, so this machinery must be ported,
not simplified away.

`humanizeName`: `name.replace(/[_-]+/g, " ").replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/^./, c => c.toUpperCase())`.
No lookaround; the `regex` crate handles all three with a manual capture-replace.

##### 10.2.6 `coerceAndValidateFormValues` — the validation core

Exported, and iterated over `Object.entries(params.requestedSchema.properties)` (document order again),
writing into `output`. `required` is a `Set`.

* `value === undefined`: throw `` `Missing required elicitation field: ${name}` `` if required, else
  `continue` (the key is simply absent from `output`).
* **string**: `stringValue = String(value)`.
  * `minLength` → `` `Elicitation field ${name} is shorter than minimum length ${limits.minLength}` ``
  * `maxLength` → `` `Elicitation field ${name} is longer than maximum length ${limits.maxLength}` ``
  * `"enum" in schema && !schema.enum.includes(stringValue)` → `` `Elicitation field ${name} is not an allowed value` ``
  * `"oneOf" in schema && !schema.oneOf.some(o => o.const === stringValue)` → same message
* **number / integer**:
  * `typeof value === "string" && value.trim() === ""` → `` `Elicitation field ${name} must be a number` ``
  * `numberValue = typeof value === "number" ? value : Number(value)`; `!Number.isFinite` → same message
  * `type === "integer" && !Number.isInteger` → `` `Elicitation field ${name} must be an integer` ``
  * `minimum` → `` `Elicitation field ${name} is below minimum ${schema.minimum}` ``
  * `maximum` → `` `Elicitation field ${name} is above maximum ${schema.maximum}` ``
* **boolean**: `typeof value === "boolean" ? value : value === "true"` — any other string is `false`,
  silently.
* **array**:
  * not an array → `` `Elicitation field ${name} must be a list` ``
  * `allowed` set from `extractMultiSelectOptions`; `arrayValue = value.map(String)`
  * `minItems` → `` `Elicitation field ${name} has fewer than ${schema.minItems} selections` ``
  * `maxItems` → `` `Elicitation field ${name} has more than ${schema.maxItems} selections` ``
  * any item not in `allowed` → `` `Elicitation field ${name} contains an invalid selection` ``
* **any other `type`** falls through all branches and contributes **nothing** to `output`.

That is **13 distinct message templates across 15 throw sites** (`is not an allowed value` and
`must be a number` each fire from two places), counting the schema-validator wrapper below. An
implementer porting "the 11 templates" will come up two short.

Two rmcp effects on this function. (i) The limits are now **typed fields** —
`StringSchema::{min_length, max_length, format}`, `NumberSchema::{minimum, maximum}` (`f64`),
`IntegerSchema::{minimum, maximum}` (`i64`), `UntitledMultiSelectEnumSchema::{min_items, max_items}` —
so there is no JSON poking and no "is this key present" ambiguity. (ii) The final `else` arm has no rmcp
analogue: `PrimitiveSchemaDefinition` is a closed untagged enum, so a property whose `type` matches none
of the five variants **fails deserialisation of the whole request** rather than being silently dropped.
That is a real behavioural delta — upstream would have accepted the form and omitted that one field —
and it should be recorded, not papered over. It is also arguably the better behaviour, and it is what
every rmcp client does.

Then the whole `output` object is validated against the **original** `requestedSchema` with
`new AjvJsonSchemaValidator().getValidator(schema)(output)`; a failure throws
`` `Invalid elicitation response: ${validation.errorMessage}` ``.

`Number(value)` is JS numeric coercion, and it is not `str::parse::<f64>()`. Verified against node:
`Number("0x1f")` → `31`, `Number("1e3")` → `1000`, `Number("Infinity")` → `Infinity`, `Number(" 7 ")` → `7`,
`Number("7abc")` → `NaN`, `Number("")` → `0` — which is why the blank guard exists. `str::parse::<f64>()`
accepts `"inf"`/`"NaN"` and rejects `"0x1f"`, i.e. it diverges in both directions. Reproduce JS `Number()`
explicitly.

Note the **double validation**: the per-field pass in `collectValidField` runs this same function on a
one-property schema, so the schema-validator call also runs per field.
`elicitation-handler.test.ts` pins that a `format: "email"` violation is caught by that pass (the
coercion pass has no format handling) and re-prompted, so **format must be a validation assertion, not
an annotation**. The SDK's `AjvJsonSchemaValidator` is constructed with `addFormats` and
`validateFormats: true`; the Rust `jsonschema` crate treats `format` as annotation-only unless
`.should_validate_formats(true)` is set. rmcp does **no** client-side JSON-Schema validation anywhere —
there is no validator hook on `Peer<RoleClient>`, unlike the TS SDK's `jsonSchemaValidator` client
option — so this is hand-written, and it is the same `jsonschema` dependency the tool-argument validator
needs. Budget for the compiled-schema construction cost or cache it: the validator runs twice per field.

##### 10.2.7 URL mode

`handleUrlElicitation`, also exported and reused by the manager for `UrlElicitationRequiredError` fan-out.

1. `!options.allowUrl` → `throw new ProtocolError(ProtocolErrorCode.InvalidParams, "URL elicitation is not supported")` — JSON-RPC code **-32602**.
2. `new URL(params.url)` failure → `ProtocolError(-32602, "URL elicitation supplied an invalid URL")`.
3. `protocol !== "http:" && protocol !== "https:"` → `ProtocolError(-32602, "URL elicitation only supports HTTP and HTTPS URLs")`. `file:` is pinned as rejected before any dialog.
4. Confirmation dialog — the exact 9 lines joined with `"\n"`: `"MCP Browser Request"`,
   `` `Server: ${serverName}` ``, `""`, `params.message`, `""`, `` `Host: ${parsed.host}` ``,
   `` `Full URL: ${params.url}` ``, `""`, `"Open this URL in your browser?"`; options `["Open","Decline"]`.
   `undefined`→cancel, `"Decline"`→decline.
   `Host` is `URL.host` (host **plus port**, no userinfo); `Full URL` is the **raw** `params.url` string,
   not the re-serialised `parsed.href` — the test uses `?state=a%2Fb` to pin that no re-encoding happens.
5. `await open(params.url)`; on throw:
   `` ui.notify(`Could not open MCP elicitation URL: ${message}`, "error") `` and return `{action:"cancel"}`.
6. `options.onUrlAccepted?.(params.elicitationId)`, then
   `ui.notify("Opened browser for MCP elicitation.", "info")`, return `{action:"accept"}`.

The accept callback fires **before** the notice, and both fire only after `open` resolved.

Upstream has **two** browser-open mechanisms and they are not interchangeable: the `open` npm package
here, and a hand-rolled platform dispatch through `pi.exec` with an `AbortSignal` (`utils.ts`'s
`execOpen` / `openUrl`), which is what the OAuth flow uses. This call site is the `open` one, so `opener`
is the faithful port; do **not** collapse the OAuth site into it.

##### 10.2.8 The completion-notification dedupe

When `allowUrl`, the manager installs a handler for `notifications/elicitation/complete`. `onUrlAccepted`
calls `rememberUrlElicitation`, which records `elicitationId` in a per-server `Set`
(`acceptedUrlElicitations: Map<string, Set<string>>`), itself gated on `this.runtimeSignal?.aborted`.
The notification handler:

```
if (this.runtimeSignal?.aborted) return;
const accepted = this.acceptedUrlElicitations.get(serverName);
if (!accepted?.delete(notification.params.elicitationId)) return;   // unknown OR already consumed
this.elicitationConfig?.ui.notify(
  `MCP browser interaction for ${serverName} completed. You can retry the tool now.`, "info");
```

`Set.delete` returns `false` for an id that was never accepted **and** for a repeat, so the notice fires
exactly once per accepted elicitation. `server-manager-sampling.test.ts` drives `unknown-id`, `known-id`,
`known-id` and asserts exactly two notifies total — the "Opened browser" one plus this one. The set is
cleared per server on `close` and wholesale on `closeAll`.

**rmcp models no first-class variant for this notification.** Its `is_standard_notification` list is
`cancelled`, `initialized`, `message`, `progress`, `prompts/list_changed`, `resources/list_changed`,
`resources/updated`, `roots/list_changed`, `subscriptions/acknowledged`, `tools/list_changed` — and
`notifications/elicitation/complete` is not among them, so it arrives at
`ClientHandler::on_custom_notification` as `CustomNotification { method, params: Option<Value>, extensions }`.
Match on `method`, decode `params.elicitationId`, and run the same dedupe. `HashSet::remove` returns
`bool` with the same semantics as `Set.delete`. Gate on the runtime cancel token in **both** places, as
upstream gates on `runtimeSignal.aborted` in the handler and in the recorder.

`handleUrlElicitationRequired(serverName, error)` handles the SDK's `UrlElicitationRequiredError`
(JSON-RPC **-32042**), which carries an **array**: returns `"cancel"` immediately if the runtime is
aborted or `allowUrl` is false; otherwise iterates `error.elicitations` **sequentially**,
short-circuiting on the first non-`accept`, and returns `"accept"` only if all accepted.
`server-manager-sampling.test.ts` pins both URLs being opened in order.
**rmcp has no `-32042` type and no `UrlElicitationRequiredError`** — the code appears nowhere in its
source. The port decodes `ErrorData { code: ErrorCode(-32042), data }` and reads the elicitation array
out of `data` itself. That decode is adapter policy either way; the loop was always going to be
hand-written.

#### 10.3 Tracing — `mcp-trace.ts`

##### 10.3.1 Enablement — settings only, no environment variables

`mcp-trace.ts` reads **no environment variable**: grepping the file for `process.env` and `process[` in
any form at v2.25.0 returns zero hits. Tracing is entirely settings-driven, wired from `init.ts`
(`manager.setTraceConfig?.(config.settings?.trace)`) and stored on the manager.

For the record, since a "which env vars does tracing use" question recurs: the complete list of
environment variables read by **non-test** adapter source at v2.25.0 is **20** —
`BROWSER`, `GLIMPSE_BINARY`, `HOME`, `MCP_DIRECT_TOOLS`, `MCP_OAUTH_CALLBACK_PORT`, `MCP_OAUTH_DIR`,
`MCP_UI_DEBUG`, `MCP_UI_VIEWER`, `NPM_CONFIG_CACHE`, `PI_CODING_AGENT_DIR`,
`PI_MCP_ADAPTER_DISABLE_AUTH_CACHE`, `PI_MCP_ADAPTER_DISABLE_KEYRING_RECOVERY`,
`PI_MCP_ADAPTER_KEYRING_RECOVERY_HELPER`, `PI_MCP_ADAPTER_KEYRING_RECOVERY_KEYCTL`,
`PI_MCP_ADAPTER_KEYRING_RECOVERY_NODE`, `PI_MCP_ADAPTER_TEST_AUTH_STORE`,
`PI_MCP_ADAPTER_TEST_LINUX_KEYRING_RECOVERY`, `PI_PACKAGE_DIR`, `SSH_CONNECTION`, `SSH_TTY`.
The seven `PI_MCP_ADAPTER_*` ones are read through **bracket access** (`process.env[TEST_AUTH_STORE_ENV]`
in `mcp-auth.ts`) and are invisible to a `process.env.NAME` grep — a methodology trap worth recording,
because the same trap hides them from any later pass. `GLIMPSE_BINARY`, `MCP_UI_VIEWER` and `MCP_UI_DEBUG`
belong to cut surfaces (`MCP_UI_DEBUG` only lowers `logger.ts`'s singleton level to `debug`).

```
isMcpTraceEnabled(definition, settings) = definition.trace ?? settings?.enabled === true
```

Note `??`, not `||`: a per-server `trace: false` **wins** over a global `enabled: true`.
`mcp-trace.test.ts` pins all five cases, including `isMcpTraceEnabled({debug:true}, undefined) === false`
and `isMcpTraceEnabled({trace:false}, {enabled:true}) === false`. `Option<bool>` +
`unwrap_or(settings.enabled == Some(true))` reproduces `??` exactly; `||` does not.

Settings shape — declared **twice with identical fields** (the doc comments differ) in `mcp-trace.ts`
and `types.ts`: `{ enabled?: boolean; file?: string; maxBytes?: number; maxEvents?: number }`, reached
as `settings.trace`, with the per-server boolean `definition.trace`. Declare it **once** in the port;
the duplication is a TS import-cycle artefact. `settings.trace` is an object while `definition.trace` is
a bare bool — do not unify them.

##### 10.3.2 The event schema — version 1

`MCP_TRACE_SCHEMA_VERSION = 1`. `createMcpTraceEvent` builds fields in **this insertion order**, which is
the JSONL key order:

| # | key | always? | value |
|---|---|---|---|
| 1 | `version` | yes | `1` |
| 2 | `timestamp` | yes | `new Date().toISOString()` — UTC, ms precision, `Z` suffix |
| 3 | `direction` | yes | `"outbound"` ｜ `"inbound"` |
| 4 | `server` | yes | `redactTraceText(server, 120)` |
| 5 | `transport` | yes | `"stdio"｜"streamable-http"｜"unknown"` *(see the cut note below)* |
| 6 | `kind` | yes | `"request"｜"response"｜"notification"` |
| 7 | `status` | yes | `"sent"｜"received"｜"error"` |
| 8 | `bytes` | when computable | `Buffer.byteLength(JSON.stringify(message), "utf8")` — **length only**; the payload is never written |
| 9 | `method` | when `"method" in message` | `redactTraceText(message.method, 120)` |
| 10 | `id` | when `"id" in message` | `traceId(message.id) ?? null` |
| 11 | `relatedRequestId` | when neither `undefined` nor `null` | `traceId(options?.relatedRequestId)` |
| 12 | `errorCode` | when `"error" in message && message.error && typeof message.error.code === "number"` | the numeric code |
| 13 | `durationMs` | when defined and finite | `Math.max(0, Math.round(ms * 100) / 100)` — 2 decimal places |

**The trap:** the `McpTraceEvent` **interface** declares these in a *different* order — `bytes` is 12th
there, 8th on the wire. A Rust `#[derive(Serialize)]` struct transcribed from the interface emits the
wrong key order. Transcribe the *insertion* order in `createMcpTraceEvent`. serde emits struct fields in
declaration order, so with `#[serde(skip_serializing_if = "Option::is_none")]` on the optional fields the
order is preserved without any `preserve_order` feature.

`messageKind`: `"method" in message` → `"id" in message ? "request" : "notification"`; otherwise
`"response"`.

`traceId(value)`: `null` → `null`; a **finite** number → the number; a **string** → the literal
`"[REDACTED_ID]"`; anything else → `undefined`. Because `event.id = traceId(...) ?? null`, a non-finite
number id collapses to `null`. `relatedRequestId` is only set when the result is neither `undefined` nor
`null`, so a null related id is *omitted* rather than written as null — keep that asymmetry.
**In Rust the non-finite case cannot arise**: rmcp's request id is
`NumberOrString::{Number(i64), String(Arc<str>)}`, so the `?? null` collapse is unreachable and the match
is total with two arms.

`messageBytes` is wrapped in try/catch; a circular message yields `undefined` and the key is omitted.
Rust's serializer cannot produce a cycle from rmcp's owned types, so that failure path degenerates to
"serialisation error" and is effectively dead — keep the `Option` so the key stays omissible.

##### 10.3.3 `redactTraceText` — and its one dead branch

```
if (/\b(?:token|secret|password|passwd|api[_-]?key|authorization|cookie)\b/i.test(value))
  return "[REDACTED]";
redacted = value
  .replace(/\b[a-z][a-z\d+.-]*:\/\/[^\s"'<>]+/gi,                        "[REDACTED_URL]")
  .replace(/\b(?:bearer|basic)\s+[A-Za-z0-9._~+/=-]+/gi,                 "[REDACTED_AUTH]")
  .replace(/\b(?:token|secret|password|passwd|api[_-]?key|authorization|cookie)\s*[:=]\s*[^\s,;]+/gi,
           "$1=[REDACTED]");
if (redacted.length > maxLength) redacted = `${redacted.slice(0, maxLength - 1)}…`;
return redacted;
```

Default `maxLength` is **160**; both call sites override it to **120**. Truncation takes `maxLength - 1`
code units and appends U+2026, so the result is exactly `maxLength` code units.

Node-verified observable behaviour: `"server https://user:secret@example.test/mcp"` → `"[REDACTED]"` (the
keyword guard, not the URL rule); `"hello Bearer abc.def"` → `"hello [REDACTED_AUTH]"`;
`"server ftp://example.test/mcp file:///private/path"` → `"server [REDACTED_URL] [REDACTED_URL]"`;
`"tools/call"` → unchanged; `"notify: cookie"` → `"[REDACTED]"`; a 200-`x` string → 159 `x` plus `…`
(length exactly 160).

**The third `.replace()` is unreachable.** Its pattern requires `\bKW` followed by `\s*[:=]`, i.e. the
character after the keyword is whitespace, `:` or `=` — all non-word — so the guard's `\bKW\b` must also
have matched and the function already returned `"[REDACTED]"`. The first two replacements cannot
manufacture a keyword either (`[REDACTED_URL]`/`[REDACTED_AUTH]` contain none). This matters because the
replacement string is `"$1=[REDACTED]"` against a **non-capturing** `(?:...)` group: if it ever fired, JS
would emit the two literal characters `$1`. Port it verbatim (it costs nothing) but do not "fix" the `$1`
into a capture group — that would change behaviour if the guard is ever loosened. In Rust that means the
replacement is the literal string `"$1=[REDACTED]"`, not `"${1}=[REDACTED]"`.

All four patterns are lookaround-free, so the `regex` crate's linear-time engine suffices — no
`fancy-regex`.

##### 10.3.4 `McpTraceWriter`

Defaults: `DEFAULT_MCP_TRACE_MAX_BYTES = 256 * 1024`, `DEFAULT_MCP_TRACE_MAX_EVENTS = 10_000`, both
filtered through `boundedPositiveInteger` (non-finite, `undefined`, or `<= 0` → fallback; otherwise
`Math.floor`).

**`McpTraceWriterOptions` takes injectable `appendFile`, `writeFile` and `mkdir`.** This is not
incidental: it is the seam that lets `mcp-trace.test.ts` assert the `["reset","append"]` operation order
and the `maxBytes` latch entirely in-process, with no disk. A Rust port that hard-codes `tokio::fs`
cannot run those two tests as unit tests and will silently push them into `cyrup-it`, which is not in the
default merge gate. Carry the injection point.

Construction fires an async chain immediately, stored as `fileReady`:
`mkdir(dirname(filePath), {recursive:true})` → `writeFile(filePath, "", {encoding:"utf8"})` →
`.catch(() => { initializationFailed = true; disabled = true; })`. The destination is **truncated on
open**: a re-used trace file is reset, never appended to. `mcp-trace.test.ts` pins the operation order as
exactly `["reset","append"]`.

Accessors `filePath`, `isDisabled` and `stats` are all part of the observable API the tests drive.

`write(event)` is synchronous and never throws:

1. `if (this.disabled || this.eventsWritten >= this.maxEvents) return;`
2. `line = JSON.stringify(event) + "\n"`; a stringify throw sets `disabled = true` and returns.
3. `bytes = Buffer.byteLength(line, "utf8")`.
4. **`if (bytes > this.maxBytes - this.bytesWritten) { this.disabled = true; return; }`** — this
   permanently *latches* the writer off. It does not skip one oversized line and keep going.
   `mcp-trace.test.ts` pins `maxBytes: 20` → `append` never called, `stats.events === 0`,
   `isDisabled === true`.
5. Counters incremented **before** the write is enqueued, so `stats` reports accepted, not flushed.
6. The append is chained onto a single serialized `queue` promise, which first awaits `fileReady` and
   bails if `initializationFailed`. Any rejection sets `disabled = true`. Line ordering is guaranteed by
   the chain, not by the filesystem.

`flush()` awaits `fileReady` then `queue`. The header comment on the two catch blocks states the
invariant twice: *"Tracing must never change MCP request/response behavior."* Every failure path degrades
to silence.

Note for the port: `cyrup_ext_subagents::jsonl::BoundedJsonlWriter` is the workspace's one size-capped
append-only JSONL primitive and it **also latches** on the first over-budget line — but it *appends* to
an existing file where upstream truncates on open, it is `async fn write_line(...) -> io::Result<()>`
awaiting a per-line flush where upstream is a sync never-throwing `write` onto a serialized chain, and a
genuine I/O failure surfaces as `Err` rather than latching silently. Those three differences are the
whole point of upstream's design. Write a local `TraceWriter` in `cyrup-mcp` and record the duplication
deliberately, so a later consolidation pass sees it was a choice.

##### 10.3.5 Trace file path

`createMcpTraceWriter(sessionCwd, settings = {}, randomSuffix = Math.random().toString(36).slice(2,10))`:

* `timestamp = new Date().toISOString().replace(/[:.]/g, "-")`
* if `settings.file`: `isAbsolute(file) ? file : resolve(sessionCwd ?? process.cwd(), file)`
* else: `` resolve(sessionCwd ?? process.cwd(), ".pi", "mcp-traces", `mcp-${timestamp}-${randomSuffix}.jsonl`) ``

`randomSuffix` is up to 8 chars of base-36. The default directory is **project-local `.pi/mcp-traces/`**,
not the agent dir. The `.pi` → `.cyrup` rename is a decision, not a free choice — see below.

##### 10.3.6 Lifecycle inside the manager

* One writer per manager, created **lazily on the first traced connect**:
  `this.traceWriter ??= createMcpTraceWriter(this.defaultCwd, this.traceSettings ?? {})`. All traced
  servers share it, so `maxBytes`/`maxEvents` are **session-global budgets**, not per-server.
* `traceObserver = { record: event => traceWriter.write(event) }`.
* stdio transports are instrumented after construction using `traceTransportKind(definition, transport)`;
  HTTP transports are instrumented **inside** `connectHttpClient` per attempt with the kind passed
  explicitly, and a `transportAlreadyTraced` flag stops the outer block double-wrapping.
* `disposeConnection` flushes the writer **concurrently with** `client.close()` under
  `Promise.allSettled`, aggregating failures into `AggregateError(failures, "MCP connection cleanup failed")`.
* `closeAll` clears `acceptedUrlElicitations`, drops the sampling and elicitation configs, then awaits a
  final flush.

`traceTransportKind` upstream is, in order: `definition.command` → `"stdio"`; `definition.socket` →
`"unix-socket"`; else `transport.constructor?.name.toLowerCase()` containing `"sse"` → `"sse"`, containing
`"streamable"` → `"streamable-http"`; else `definition.url ? "streamable-http" : "unknown"`. **Under the
scope cuts the enum is `{Stdio, StreamableHttp, Unknown}`** and the constructor-name sniffing goes with
it: it exists only to distinguish the two HTTP transports, one of which no longer exists, and it is
already redundant with the kind the HTTP path passes explicitly. cyrup carries the kind as an enum from
the transport factory and never inspects a type name. The observable output is identical for every case
that survives.

##### 10.3.7 The instrumentation mechanism, and why the Rust answer is now settled

`wrapTransportWithMcpTrace` returns **the same object it was given** (`mcp-trace.test.ts` asserts
`expect(wrapped).toBe(underlying)`), and mutates it:

1. `Object.defineProperty(transport, "onmessage", { configurable: true, enumerable: true, get, set })`
   where the setter stores the caller's handler and installs a wrapper that records an `"inbound"` event
   **before** invoking it. Then `transport.onmessage = messageHandler` re-drives the setter for whatever
   was already assigned.
2. The whole `defineProperty` is inside `try/catch` — a non-configurable `onmessage` leaves tracing
   partially off rather than failing the connection. `mcp-trace.test.ts` pins that case.
3. `transport.send` is replaced: `started = performance.now()`;
   `messages = Array.isArray(message) ? message : [message]` (JSON-RPC batch);
   `await originalSend.call(transport, message, options)`; on success record one `"sent"` event per
   member with the same `durationMs`; on failure record one `"error"` event per member and **rethrow**.
   Outbound events are therefore written **after** the send completes, never before.
4. `record()` swallows observer throws — a throwing observer does not break `onmessage` or `send`, and a
   *writer* failure (append rejecting) leaves `send`/`onmessage` intact and only sets `isDisabled`.

The documented reason for in-place mutation is that SDK v2 detects its base stdio transport before
connect so it can run `server/discover` on a disposable sibling process, and a wrapper object hides that
identity — making *enabling tracing change protocol negotiation*.

**That hazard does not exist in rmcp, and this is verified.** `serve_client_with_ct_inner` is generic over
`T: Transport<RoleClient>` and never inspects a concrete transport type; `ClientLifecycleMode::{Initialize,
Discover{preferred_versions}, Auto{preferred_versions, legacy_version}}` probes `server/discover` over
**the same transport**, with a 10-second `DEFAULT_AUTO_DISCOVER_TIMEOUT` before falling back to
`legacy_startup` — no sibling process, no type sniffing. The only `TypeId::of::<T>()` in the transport
layer is in `DynamicTransportError`, which records `transport_name: T::name()` and `transport_type_id` so
a caller can `is::<T, R>()` / `downcast::<T, R>()` the error to the concrete transport's error type.

So the Rust mechanism is a `TracingTransport<T>` newtype implementing
`rmcp::transport::Transport<RoleClient>` — `send(&mut self, TxJsonRpcMessage<RoleClient>)`,
`receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>>`, `close(&mut self)` — forwarding to the inner
transport and recording around each. `receive` replaces the `onmessage` interception (rmcp pulls rather
than pushes, so there is no property to define and the `try/catch` around `defineProperty` has no
analogue); `send` keeps the timing and the batch-per-member event fan-out. **The one consequence to
handle:** any code that downcasts a `DynamicTransportError` must target `TracingTransport<T>` or unwrap
the inner error, because wrapping changes `transport_type_id` and `T::name()`. Name it in the type's doc
comment and in the connection error-mapping path.

---

### Port units

Verdicts: **`rmcp`** · **`host-verb`** · **`extension-owned`** · **`hand-written`** ·
**`host-addition`** · **`open-decision`** · **`cut`**.

#### Sampling

**MCP-450 — `handleSamplingRequest` as a pure function of an options bag** · high · M · `hand-written`
**upstream** — `sampling-handler.ts`: `registerSamplingHandler(client, options)` installs one closure;
`handleSamplingRequest(options, request)` runs the 12-step sequence of §10.1.2 and is separately exported
so it is unit-testable without a `Client`.
**behavior** — an MCP server can ask the agent to run a completion; the user approves the request and the
response; the server receives `{role:"assistant", content:{type:"text",text}, model:"provider/id", stopReason}`.
**cyrup** — `cyrup_mcp::sampling::handle_sampling_request(&SamplingOptions, CreateMessageRequestParams)
-> Result<CreateMessageResult, ErrorData>`, called from `ClientHandler::create_message`. Keep the
free-function shape: rmcp's handler trait is the adapter, not the logic. Keep the capability surface
narrow — upstream's `SamplingUIContext` is `Pick<ExtensionUIContext, "confirm">`, so sampling may **only**
call `HostServices::confirm`.
**verify** — unit tests mirroring `__tests__/sampling-handler.test.ts`'s 11 cases 1:1, in-crate.

**MCP-451 — the six unsupported-feature rejections, in order** · medium · S · `hand-written` (+ `rmcp` for `task`)
**upstream** — `sampling-handler.ts`: five parameter guards checked in a fixed order, plus the
per-content-block guard raised during `convertUserContent`/`convertAssistantContent`, which runs **before**
model resolution.
**behavior** — a server asking for tasks, context inclusion, tool use, tool choice, stop sequences, or
non-text content gets a specific error naming the feature, and no model call and no approval dialog happen.
**cyrup** — a `match` producing the exact strings of §10.1.2 over the typed `Option` fields of
`CreateMessageRequestParams` (`include_context: Option<ContextInclusion>`, `tools: Option<Vec<Tool>>`,
`tool_choice: Option<ToolChoice>`, `stop_sequences: Option<Vec<String>>`). Keep the evaluation order so the
*first* violated guard is reported, and keep the content guard after the five parameter guards but before
model resolution. **The `task` guard is structural:** `CreateMessageRequestParams` has no `task` field;
task augmentation is the `io.modelcontextprotocol/tasks` extension (`ClientCapabilities::supports_tasks`),
which `cyrup-mcp` never declares. Record the string in the error taxonomy with a comment saying why it has
no throw site.
**verify** — unit; assert the exact message strings, and assert the completion seam is never invoked.

**MCP-452 — `resolveSamplingModel` candidate ordering and the sequential auth probe** · high · M · `extension-owned`
**upstream** — `sampling-handler.ts`, helper `addSamplingCandidate`: hints (substring, case-insensitive,
over `provider/id` ‖ `id` ‖ `name`) → current model → whole registry, deduped first-wins; then probe auth
in order, collecting `provider/id: error` strings; two exhaustion messages.
**behavior** — a server's `modelPreferences.hints` steer which of the user's models runs the completion; a
model with no configured auth is skipped, not failed on; exhaustion produces one of two exact messages.
**cyrup** — a pure `fn sampling_candidates(available: &[Model], hints: &[String], current: Option<&Model>)
-> Vec<Model>` plus an async probe loop over `cyrup-provider`'s `Models::get_auth`. Substring matching is
`str::to_lowercase().contains()`; do **not** substitute fuzzy matching. Dedupe key is `(provider, id)`.
**Call the signal accessor twice**, as upstream does — the probe loop reads a freshly-resolved signal, so a
token captured once at entry diverges when the turn rolls over mid-request.
**verify** — unit, the seven hint-ordering cases from `__tests__/sampling-handler.test.ts` transcribed.

**MCP-453 — run the nested completion** · high · M · `extension-owned`
**upstream** — `sampling-handler.ts` imports `complete` from `@earendil-works/pi-ai/compat` **directly**,
bypassing the host API entirely, and drives a model with credentials it obtained from `ctx.modelRegistry`.
**behavior** — the extension can run one model completion, with the session's credentials, while a turn is
in flight, and have it cancelled by the turn's signal.
**cyrup** — `cyrup-mcp` depends on `cyrup-provider` and calls its completion path (`Models::complete`)
directly. This is the *literal* upstream mechanism; `cyrup-ext-subagents` establishes the precedent (it
uses `cyrup_provider::Model`, `ModelCost` and `builtin_catalog()` throughout), and `cyrup-ext` itself
already depends on `cyrup-provider`, so no new layering appears. Compose a child
`tokio_util::sync::CancellationToken` cancelled by either the run cancellation or the connection's own
token, and pass it as `StreamOptions.cancel`.
**Correction to the first pass:** this was filed as a `critical` "add a nested-completion verb to
`HostServices` and decide which tier may call it". There is no such need. `HostServices` is the capability
surface a *WASM guest* is confined to; a native built-in links the provider crate. Adding a host verb here
would **diverge** from upstream, not match it.
**verify** — unit with a stubbed provider; plus a `cyrup-it` test that a completion in flight is cancelled
when the turn is.

**MCP-454 — source the candidate set from the whole configured catalogue** · medium · S · `extension-owned` + `host-verb`
**upstream** — `SamplingModelRegistry = Pick<ModelRegistry, "getAvailable" | "getApiKeyAndHeaders">`;
`getAvailable()` spans **every** configured provider.
**behavior** — hint matching and auth fallback see the user's whole model catalogue, not one provider's.
**cyrup** — `cyrup_provider::catalog::{builtin_catalog, load_catalog}` read directly by `cyrup-mcp`, with
`HostServices::{models, scoped_models, current_model}` supplying the session's own view so resolution
starts from what the session actually has. **`HostServices::models()` alone is the wrong source** — it is
one installed provider's catalogue, which is narrower than pi's `getAvailable()`; the tree has already
fixed that class of bug twice elsewhere. Reading the catalogue directly sidesteps it without changing any
existing `models()` consumer.
**Correction to the first pass:** filed as an open question about widening or adding a `HostServices` verb.
Neither is needed.
**verify** — unit: a catalogue with two providers must produce candidates from both.

**MCP-455 — the two approval gates and their formatters** · critical · M · `host-verb`
**upstream** — `sampling-handler.ts`: `confirmSampling` (auto-approve short-circuit, no-UI throw, decline
throw) plus `formatRequestApproval` / `formatResponseApproval` / `messageText`.
**behavior** — without `settings.samplingAutoApprove`, a server cannot cause a model call the user did not
see, and cannot receive a response the user did not see. Both dialogs show truncated but *inspectable*
content.
**cyrup** — `HostServices::confirm(prompt, message, &DialogOptions)`; `false` → the decline error, which is
the same fail-closed polarity as upstream's owner-fenced `undefined`. Hold both dialogs under
`HostServices::human_interaction_lock` and a `HostCtx::begin_human_wait` guard (MCP-471). The no-UI case is
*not* `confirm` returning false: carry an explicit `has_ui: bool` sourced from `HostConfig.has_ui` so the
two distinct messages survive.
**severity note** — this is the consent gate for spending the user's model credentials on server-directed
input. Inverting the polarity, or treating a dismissed dialog as approval, is a permission bypass under the
house scale's third clause. That is why it is `critical` and MCP-450 is not.
**verify** — unit on the two title strings and the body substrings; plus a live-pty check that both dialogs
render (MCP-496).

**MCP-456 — `convertSamplingMessage`, `convertAssistantResult`, `mapStopReason`** · medium · M · `hand-written`
**upstream** — `sampling-handler.ts`: single-block-or-array normalisation; the synthetic assistant
sentinels `api:"mcp-sampling"`, `provider:"mcp"`, `model:"sampling-request"`, `zeroUsage()`,
`stopReason:"stop"`; error/aborted rethrow; thinking blocks dropped; `"\n\n"` join then `trim`; empty →
throw; stop-reason mapping.
**behavior** — the server's message history is faithfully replayed to the provider, and the provider's
answer is reduced to the single text block the MCP result type allows.
**cyrup** — direct translation over rmcp's `SamplingMessage`, `SamplingContent::{Single, Multiple}` (which
**already** models the single-block-or-array case) and `SamplingMessageContentBlock::{Text, Image, Audio,
ToolUse, ToolResult}`, into `cyrup-provider`'s context/message types and back into `CreateMessageResult`
(whose `STOP_REASON_*` constants are exactly `mapStopReason`'s outputs). The sentinels are *literal strings
in persisted-adjacent structures* and must not be "improved"; a session that records a sampling round-trip
would otherwise diverge from pi's.
**verify** — unit; round-trip a two-message history and assert the exact synthetic assistant record.

**MCP-457 — capability advertisement and handler-before-connect** · low · S · `rmcp`
**upstream** — `server-manager.ts`: capabilities built from which configs are set; the whole `capabilities`
key omitted when empty; handlers registered inside `createClient`, which returns before any `connect()`.
**behavior** — a server sees `sampling` in `initialize` iff the adapter can service it, and a request
arriving in the first millisecond after `initialize` has a handler.
**cyrup** — `ClientHandler::get_info() -> ClientInfo` supplies `ClientCapabilities` at service
construction and the handler *is* the service, so the ordering hazard is structural rather than
disciplinary. Set `ClientCapabilities.sampling = Some(SamplingCapability::default())` when sampling is
configured, `None` otherwise; leave `SamplingCapability::{tools, context}` `None` so it serialises as `{}`,
matching upstream.
**accepted delta** — rmcp always emits `"capabilities": {}` where upstream omits the key. The MCP spec
requires the field, so cyrup is the more conformant; the upstream test that asserts the omission does not
port.
**verify** — a `cyrup-it` test against a fixture server that echoes `getClientCapabilities()`, the pattern
`__tests__/fixtures/elicitation-server.mjs` already uses.

**MCP-458 — bind sampling's model and cancellation to the live runtime owner** · high · M · `host-verb` + `hand-written`
**upstream** — `init.ts`: `getCurrentModel: () => owner.isActive() ? ctx.model : undefined` and
`getSignal: () => owner.isActive() ? combineAbortSignals(owner.signal, ctx.signal) : owner.signal`;
`combineAbortSignals` is `AbortSignal.any`; the fence is `createOwnedUi`'s proxy in `runtime-owner.ts`.
**behavior** — a request issued after the user switched models uses the *new* model; a request in flight
when the turn is cancelled or the session reloads is aborted; after teardown the handler is permanently
aborted rather than running against a dead session.
**cyrup** — two closures over the `Arc<dyn HostServices>` stashed by `NativeExtension::set_host_services`:
`HostServices::current_model()` is already a live read, and `HostServices::is_run_cancelled()` is the
poll-based analogue of `ctx.signal`. **Mechanism divergence to record:** upstream composes two
`AbortSignal`s with `AbortSignal.any`; cyrup composes a child `tokio_util::sync::CancellationToken`
cancelled by either source and passes it down, rather than polling inside the completion. Reproduce the
*two independent* `getSignal()` reads (§10.1.3).
**verify** — unit mirroring `__tests__/init-elicitation.test.ts` (model swap observed; signal aborts;
post-`stop` state).

**MCP-459 — `truncateAtWord` with UTF-16 length semantics** · low · S · `hand-written`
**upstream** — `utils.ts`'s `truncateAtWord`, the 5-branch algorithm of §10.1.4 with the
`lastSpace > target * 0.6` heuristic.
**behavior** — approval dialogs show a word-boundary-truncated prefix ending in `...` (three ASCII periods,
not U+2026 — which is what `redactTraceText` uses).
**cyrup** — `fn truncate_at_word(text: &str, target: usize) -> Cow<str>`. `text.length` in JS is UTF-16
code units; use `text.encode_utf16().count()` and slice on a UTF-16-safe boundary, or accept and record a
divergence for non-BMP input. `lastIndexOf(" ")` is a literal ASCII space only.
**verify** — unit with an emoji-bearing string; the two node-verified vectors from §10.1.4 are the floor.

#### Elicitation

**MCP-460 — dispatch, and absent/unknown `mode` → form** · low · S · `rmcp`
**upstream** — `elicitation-handler.ts`'s `handleElicitationRequest`: `params.mode === "url" ? url : form`,
so absent/unknown mode is **form**.
**behavior** — a server's structured question reaches the user through the right UI shape.
**cyrup** — `ClientHandler::create_elicitation(ElicitRequestParams, ctx)`. `ElicitRequestParams` is
`#[serde(tag = "mode", try_from = "ElicitRequestParamsWire")]` and the wire enum's `#[serde(untagged)]
LegacyForm` arm catches an absent or unrecognised `mode` and maps it to `FormElicitationParams` —
upstream's behaviour, for free.
**Correction to the first pass:** it claimed rmcp's typed enum is stricter than upstream's string compare
and proposed adding `#[serde(other)]`-style leniency. Withdrawn; the leniency is already there.
**verify** — unit; a params object with no `mode` must take the form path.

**MCP-461 — `handleFormElicitation`'s gate, review loop and edit picker** · high · M · `hand-written` + `host-verb`
**upstream** — `elicitation-handler.ts`, §10.2.3, including the empty-form short-circuit and the
un-uniquified edit labels.
**behavior** — the user sees one gate, then one dialog per field, then a review screen they can submit,
decline, or bounce back into an editor from; dismissal at any point is `{action:"cancel"}` and never a
partial submit.
**cyrup** — `HostServices::select` returns `Option<String>`; `None` is upstream's `undefined`, produced on
dialog cancel in both the TUI and RPC hosts. Preserve two behaviours a Rust engineer would "fix":
(i) the review-loop `coerceAndValidateFormValues` call is **not** caught, so a cross-field failure escapes
as a JSON-RPC error rather than an `ElicitResult`; (ii) duplicate edit labels resolve first-wins via
`indexOf`. Return `ElicitResult::new(ElicitationAction::Accept).with_content(value)` /
`::new(Decline)` / `::new(Cancel)` — all three actions are native.
**verify** — unit, the four form cases from `__tests__/elicitation-handler.test.ts` plus the empty-form
triple.

**MCP-462 — iterate `requestedSchema.properties` in document order** · low · S · `rmcp`
**upstream** — `Object.entries(properties)` appears exactly **twice**: once in `handleFormElicitation` and
once in `coerceAndValidateFormValues`. The array derived in the former drives the question order, the
review-row order and the edit-picker order — one insertion-ordered read governing four user-visible
orderings.
**behavior** — the user is asked the server's questions in the order the server wrote them.
**cyrup** — `ElicitationSchema` deserialises through `ElicitationSchemaWire`, whose `properties` is an
`IndexMap`, and the `From` impl fills `property_order: Some(wire.properties.keys().cloned().collect())`.
Iterate `property_order` and look up in `properties`; **iterating the `BTreeMap` directly is the bug**, it
sorts lexicographically, and it is silent.
**Correction to the first pass:** filed as a `high` hand-written item requiring a custom `MapAccess`
`Deserialize` impl, with an explicit warning not to enable `serde_json/preserve_order` workspace-wide. None
of that is needed; rmcp already did it.
**verify** — unit: a schema with keys `z`, `a`, `m` must produce dialogs in that order.

**MCP-463 — `collectValidField`'s per-field re-prompt loop** · medium · S · `hand-written` + `host-verb`
**upstream** — `elicitation-handler.ts`: validate each field against a synthetic one-property schema
(carrying `required` only when the field is required), `notify(error, "error")` on failure, re-ask seeded
with the rejected value, loop forever.
**behavior** — a bad value is rejected immediately with the server's own constraint text, and the user's
typing is not lost.
**cyrup** — `HostServices::notify(&str, NotifyKind::Error)`. The synthetic schema must copy the params and
replace only `requested_schema` so sibling fields survive. `notify` is fire-and-forget in both
implementations, so the re-prompt dialog can open before the toast renders — same as upstream. The loop is
unbounded, so the human-wait guard wraps the **loop** (MCP-471).
**verify** — unit, the email case and the four blank-number parameterisations, asserting the exact notify
text `Elicitation field quantity must be a number`.

**MCP-464 — `coerceAndValidateFormValues`, including JS `Number()` semantics** · high · M · `hand-written`
**upstream** — `elicitation-handler.ts`: the full coercion + limit table of §10.2.6 — **13 distinct message
templates across 15 throw sites**.
**behavior** — a server receives correctly typed values within its declared constraints, and the user sees
the specific constraint they violated.
**cyrup** — a `match` over `PrimitiveSchemaDefinition`, reading the **typed** limit fields
(`StringSchema::{min_length, max_length}`, `NumberSchema::{minimum, maximum}`,
`IntegerSchema::{minimum, maximum}`, `*MultiSelectEnumSchema::{min_items, max_items}`) rather than poking
JSON. Three named hazards. (i) `Number(value)` is **not** `str::parse::<f64>()`: node-verified, it accepts
`"0x1f"`→31, `"1e3"`→1000, `"Infinity"`, `" 7 "`→7, and yields `NaN` for `"7abc"` and `0` for `""` —
implement it explicitly. (ii) boolean coercion is `value === "true"`, so every other string is `false`
silently; do not substitute a `bool::from_str` that errors. (iii) upstream's "any other `type`" arm
silently drops the field; rmcp's closed untagged `PrimitiveSchemaDefinition` instead **fails deserialisation
of the whole request**. Record that as a behavioural delta rather than trying to reproduce the drop.
**verify** — unit over all 13 message templates plus a JS-parity table for `Number()` inputs.

**MCP-465 — final schema assertion with `format` as an assertion** · high · M · `hand-written`
**upstream** — `elicitation-handler.ts`: `new AjvJsonSchemaValidator().getValidator(requestedSchema)(output)`;
failure → `` `Invalid elicitation response: ${errorMessage}` ``. The validator is built with `addFormats`
and `validateFormats: true`.
**behavior** — constraints the coercion pass cannot express — notably `format: "email"` — still reject, and
the user is re-prompted rather than the server receiving garbage.
**cyrup** — `jsonschema` (already in tree, needs a version bump; keep `default-features = false` for the
same remote/file `$ref` reason the workspace already documents), built with `.should_validate_formats(true)`
— the crate treats `format` as annotation-only by default, which would silently disable the very behaviour
the upstream test pins. rmcp validates **nothing** client-side, so this is not optional. `StringFormat` in
rmcp is the closed set `{Email, Uri, Date, DateTime}`, which is narrower than `ajv-formats`; register only
what the schema type can express and drop the OpenAPI-derived extras. The error *text* will differ from
ajv's; the prefix `Invalid elicitation response: ` must not. Cache the compiled schema — it runs twice per
field.
**note** — advertising `FormElicitationCapability::with_schema_validation(true)` is truthful once this is
built, and is a disclosed improvement on upstream's bare `{form:{}}`.
**verify** — unit: `format:"email"` with `not-an-email` must fail; a valid address must pass.

**MCP-466 — the label-uniquifying and humanising helpers** · medium · S · `hand-written`
**upstream** — `elicitation-handler.ts`: `formatChoice`, `uniqueLabels`, `uniqueAction`,
`extractMultiSelectOptions`, `humanizeName`.
**behavior** — two enum members with the same display text remain individually selectable; the synthetic
`Use default` / `Omit` / `Done` actions never collide with a real option; a schema-less field name renders
as a sentence-cased label.
**cyrup** — direct. The disambiguator is U+2026 appended repeatedly (a `while`, not an `if`), and it exists
because `HostServices::select` returns the chosen *string* — the same contract `ui.select` has — so
inverting back to an index needs uniqueness. `extractMultiSelectOptions` becomes a `match` on
`MultiSelectEnumSchema::{Untitled(items.enum_), Titled(items.any_of)}`; **do not unify it with the
single-select builder** — rmcp's types encode the `enumNames` asymmetry (`LegacyEnumSchema::enum_names` and
`TitledSingleSelectEnumSchema::one_of` carry titles; `UntitledItems` does not), so merging would add a
feature the wire never had. `humanizeName`'s three regexes need no lookaround and compile on `regex`.
**verify** — unit: two `oneOf` members with identical titles produce distinct selectable labels; an option
literally named `Done` forces `Done…`.

**MCP-467 — `handleUrlElicitation`, including the three `-32602` rejections** · high · M · `hand-written` + `host-verb` + `extension-owned`
**upstream** — `elicitation-handler.ts`: the `allowUrl` gate, URL parse, scheme allowlist, the exact 9-line
dialog, `open()`, the failure→cancel path, the accept callback then notice.
**behavior** — a server can send the user to a web page only in TUI mode, only over http/https, only after
the user reads the full URL and host, and a browser that fails to launch cancels rather than silently
accepting.
**cyrup** — `url::Url::parse` for parse + scheme check (`url` is already a workspace dependency);
`HostServices::select` for the dialog and `HostServices::notify` for both notices; `opener::open` for the
launch. The `Host:` line is host **plus port** (JS `URL.host`), and `Full URL:` is the **raw input string**,
not `Url::to_string()` — the test pins that `?state=a%2Fb` is not re-encoded. This call site is upstream's
`open` one; do **not** collapse it into the OAuth flow's `execOpen` path.
**severity note** — the scheme allowlist is defence in depth behind a dialog that shows the full URL, which
is why this is `high` and not `critical`; dropping the allowlist alone does not bypass consent.
**verify** — unit for the three rejection paths and the exact dialog text; `cyrup-it` for the open path
with a stub opener.

**MCP-468 — advertise `elicitation:{form, url?}` with `allowUrl == (mode == tui)`** · medium · S · `rmcp` + `host-verb`
**upstream** — `init.ts`: enablement is `settings.elicitation !== false && hasUI`; `allowUrl` is exactly
`mode === "tui"`. `server-manager.ts` builds the capability object.
**behavior** — an RPC or headless host never advertises `url` and therefore never gets asked to open a
browser; the SDK rejects a URL elicitation server-side with "does not support URL-mode elicitation".
**cyrup** — `ElicitationCapability::new().with_form(FormElicitationCapability::new())`, plus
`.with_url(UrlElicitationCapability::new())` when `allowUrl`, returned from `ClientHandler::get_info`.
`HostConfig { mode, has_ui, cwd }` supplies both predicates — `has_ui` and `mode == Tui` are **different**
upstream and must stay different here.
**verify** — `cyrup-it`, mirroring `__tests__/elicitation-sdk-integration.test.ts`: connect a fixture server
in each mode and assert the capability object it observes.

**MCP-469 — the `notifications/elicitation/complete` dedupe and its notice** · medium · S · `rmcp` + `hand-written`
**upstream** — `server-manager.ts`: a per-server `Set<elicitationId>` populated by `onUrlAccepted` →
`rememberUrlElicitation`; the handler notifies only when `Set.delete` returns `true`, so unknown ids and
repeats are silent; cleared per-server on close and wholesale on `closeAll`.
**behavior** — after the user returns from the browser, they get exactly one
`MCP browser interaction for <server> completed. You can retry the tool now.` notice — not zero, not three.
**cyrup** — `ClientHandler::on_custom_notification(CustomNotification { method, params, .. }, ctx)`: rmcp's
`is_standard_notification` list does not include `notifications/elicitation/complete`, so it arrives here.
`HashMap<String, HashSet<String>>` behind the manager's lock; `HashSet::remove` returns `bool` with the same
semantics as `Set.delete`. Gate on the runtime cancel token in **both** places, as upstream gates on
`runtimeSignal.aborted` in the handler and in the recorder.
**verify** — unit: three notifications (`unknown-id`, `known-id`, `known-id`) → exactly two notifies total.

**MCP-470 — `handleUrlElicitationRequired` for the `-32042` elicitation array** · medium · S · `hand-written`
**upstream** — `server-manager.ts`: cancel immediately if aborted or `!allowUrl`; otherwise iterate
`error.elicitations` sequentially, short-circuit on the first non-accept, return `"accept"` only if all
accept.
**behavior** — a tool call that fails with `-32042` because the user must visit N pages walks the user
through all N in order and reports one aggregate outcome the caller turns into
`details.error = "url_elicitation_required"`.
**cyrup** — **rmcp has no `UrlElicitationRequiredError` and `-32042` appears nowhere in its source**
(verified). Decode `ErrorData { code: ErrorCode(-32042), data }` and read the elicitation array out of
`data` in `cyrup-mcp`, then run the sequential loop. The loop was always adapter policy.
**verify** — unit on the sequential-open assertion; plus the six-shape integration matrix from
`__tests__/elicitation-sdk-integration.test.ts` as a `cyrup-it` test.

**MCP-471 — hold the dispatcher budget and the interaction lock across every dialog** · high · S · `host-verb`
**upstream** — no analogue: pi has no invocation-budget watchdog on an extension handler.
**behavior** — a user who takes two minutes to answer a server's question does not have their answer thrown
away by a budget timeout, and an MCP dialog and a permission prompt are never on screen at once.
**cyrup** — hold a `HostCtx::begin_human_wait()` guard across every `select`/`input`/`confirm` in
`cyrup-mcp` — it is `#[must_use]` precisely because dropping it immediately does nothing — and take
`HostServices::human_interaction_lock`, the one session-scoped `HumanInteractionLock`, so the MCP dialog
and the permission gate's own `ask` serialise. This is a cyrup-only requirement created by a cyrup-only
mechanism; it must be in the spec or it will be omitted, because there is no upstream line to port it from.
Elicitation's per-field re-prompt loop is unbounded, so the guard wraps the **loop**, not each dialog.
**severity note** — demoted from the first pass's `critical`. The budget firing kills the handler, which
returns an error to the server; it is fail-closed, not a bypass.
**verify** — `cyrup-it`: an extension handler blocked on a scripted 5-second dialog must not be
budget-killed, and a concurrent permission prompt must queue behind it.

**MCP-472 — the three URL rejections carry JSON-RPC `-32602`** · low · S · `rmcp`
**upstream** — `elicitation-handler.ts`: three `ProtocolError(ProtocolErrorCode.InvalidParams, msg)` throws;
the tests assert `{ code: -32602 }` on the wire.
**behavior** — a server that asks for something the client cannot do gets a *protocol* error with the
standard code, not an opaque internal error.
**cyrup** — `ErrorData::invalid_params(message, None)`, which is `ErrorCode::INVALID_PARAMS = -32602`.
Every **other** throw in these two handlers is a plain `Error` and becomes `-32603` — `ErrorData::internal_error`;
do not uniformly promote them.
**Correction to the first pass:** filed as `medium`/`likely` with rmcp unread. Confirmed and demoted.
**verify** — `cyrup-it` against a fixture server that inspects the JSON-RPC error code.

#### Tracing

**MCP-473 — the `McpTraceEvent` schema v1, exact key set and order** · medium · S · `hand-written`
**upstream** — `mcp-trace.ts`: `MCP_TRACE_SCHEMA_VERSION = 1` and `createMcpTraceEvent`'s 13-field
insertion order (§10.3.2).
**behavior** — a trace file is a stable, machine-readable JSONL format a tool can parse by version;
payloads never appear in it.
**cyrup** — a `#[derive(Serialize)]` struct with `#[serde(skip_serializing_if = "Option::is_none")]` on the
optional fields; serde emits fields in declaration order, so key order is preserved with no `preserve_order`
feature. **Transcribe the insertion order in `createMcpTraceEvent`, NOT the interface declaration order** —
they differ: `bytes` is 8th on the wire and 12th in the interface. `version` is a literal `1`. The
`errorCode` guard includes the truthiness check on `message.error`, not just the `typeof` check on its code.
The `transport` field's value set is reduced by the scope cuts — see MCP-478.
**verify** — unit; golden-compare one serialised line per `kind`, asserting key *order*, not just presence.

**MCP-474 — `redactTraceText`, dead third branch and all** · high · S · `hand-written`
**upstream** — `mcp-trace.ts`: the keyword guard, three replacements, and the `maxLength`/U+2026 truncation
of §10.3.3. Default `maxLength` 160; both call sites pass 120.
**behavior** — a server name or method name that contains a credential-ish token never reaches disk; URLs
and bearer/basic strings are replaced wholesale.
**cyrup** — `regex` with four `LazyLock<Regex>`; all four patterns are lookaround-free so the linear-time
engine suffices and `fancy-regex` is not needed. Two exactness traps: (i) the third replacement's
`"$1=[REDACTED]"` targets a **non-capturing** group and would emit the literal `$1` — port it as the literal
string, not as `"${1}=[REDACTED]"`; (ii) truncation is `slice(0, maxLength - 1) + "…"` in **UTF-16 code
units**.
**severity note** — `high` because this is the only thing standing between a credential-bearing server name
or method and a plaintext file on disk. It is not `critical` under the four clauses.
**verify** — unit, the upstream redaction cases plus the six node-verified vectors in §10.3.3.

**MCP-475 — `traceId`, `messageKind`, `messageBytes`** · low · S · `hand-written`
**upstream** — `mcp-trace.ts`: `messageKind`; `traceId` turns strings into `"[REDACTED_ID]"`, passes finite
numbers through, drops anything else; `messageBytes` is the serialised length of the whole message,
computed in a try/catch.
**behavior** — a correlation id that could itself be a secret is never written, but numeric ids stay useful
for correlating a request with its response.
**cyrup** — a two-arm `match` on rmcp's `NumberOrString::{Number(i64), String(Arc<str>)}`. **The non-finite
case cannot occur in Rust**, so `traceId`'s `?? null` collapse is unreachable — but keep the *asymmetry*
that `id` is written as `null` when absent while `relatedRequestId` is **omitted**.
**verify** — unit; a string id must produce `"[REDACTED_ID]"`, an absent id must produce `null`, an absent
related id must be omitted.

**MCP-476 — `McpTraceWriter`: latching caps, injectable fs, serialized append queue** · medium · M · `hand-written`
**upstream** — `mcp-trace.ts`: `McpTraceWriterOptions` with **injectable `appendFile`/`writeFile`/`mkdir`**;
`boundedPositiveInteger`; truncate-on-open; the `disabled` latch on init failure, stringify failure,
over-budget line, or any append failure; counters incremented before enqueue; single-chain ordering;
`flush()`; the `filePath`/`isDisabled`/`stats` accessors.
**behavior** — tracing is strictly bounded (256 KiB / 10 000 events by default), never reorders lines, and
**never** changes MCP request/response behaviour on any failure path.
**cyrup** — a local `TraceWriter` in `cyrup-mcp`. `cyrup_ext_subagents::jsonl::BoundedJsonlWriter` is the
closest workspace primitive and it **does** latch like upstream — but it appends where upstream truncates on
open, it is async-and-fallible where upstream is sync-and-silent, and it has no injectable-fs seam. Widening
it is the wrong trade: the two contracts differ on exactly the axes upstream designed around, and lifting a
`cyrup-ext-subagents` internal into a shared crate is a bigger change than the code it saves. **Carry the
injectable-fs seam** — without it the `["reset","append"]` ordering test and the `maxBytes` latch test
cannot be unit tests and get pushed into `cyrup-it`, which is not in the default merge gate. Record the
duplication deliberately.
**verify** — unit, all four upstream writer cases including the `["reset","append"]` ordering and the
`maxBytes: 20` latch, all with injected fs functions.

**MCP-477 — trace file path derivation, and `.pi` → `.cyrup`** · low · S · `open-decision`
**upstream** — `mcp-trace.ts`'s `createMcpTraceWriter`: `settings.file` absolute-or-resolved-against-cwd,
else `<cwd>/.pi/mcp-traces/mcp-<ISO with :. → ->-<≤8 base36 chars>.jsonl`.
**behavior** — traces land in a predictable, project-local, per-session file the user can `tail`.
**cyrup** — the directory name is a user-visible path, so it is a decision, not a free choice. Options:
(a) `.cyrup/mcp-traces/` — consistent with `cyrup-config`'s `project_config_dir()` and with the same rename
already decided for `.pi/mcp.json` → `.cyrup/mcp.json`; (b) `.pi/mcp-traces/` — byte-compatible with any
tooling that reads pi traces. **Recommendation: (a)**, settled by the same one line that settles the config
path.
**verify** — unit on the derived path for `file` absent / relative / absolute.

**MCP-478 — `isMcpTraceEnabled` and the transport-kind enum** · low · S · `hand-written`
**upstream** — `mcp-trace.ts`: `definition.trace ?? settings?.enabled === true` (per-server `false` beats
global `true`); `traceTransportKind` falling back to `transport.constructor.name`.
**behavior** — tracing is opt-in globally or per server, and every event carries the right transport label.
**cyrup** — `Option<bool>` + `unwrap_or(settings.enabled == Some(true))` reproduces `??` exactly; `||` does
not. **Under the scope cuts the kind enum is `{Stdio, StreamableHttp, Unknown}`**: the `"sse"` and
`"unix-socket"` values have no producer left, and the constructor-name sniffing existed only to tell the two
HTTP transports apart. Carry the kind as an enum from the transport factory and never inspect a type name.
The observable output is identical for every case that survives.
**verify** — unit, all five enablement cases including `{debug:true}` → false.

**MCP-479 — `TracingTransport<T>` over `rmcp::transport::Transport`** · medium · M · `hand-written`
**upstream** — `mcp-trace.ts`'s `wrapTransportWithMcpTrace` returns the **same object** (test-pinned with
`expect(wrapped).toBe(underlying)`), installing an `onmessage` getter/setter pair and reassigning `send`.
Its comment states the reason: SDK v2 detects its base stdio transport before connect so it can run
`server/discover` on a disposable sibling process, and a wrapper hides that identity.
**behavior** — turning tracing on must not change a byte on the wire, must not change which protocol
revision is negotiated, and must not change what happens when the transport itself fails.
**cyrup** — a `TracingTransport<T>` newtype implementing `Transport<RoleClient>`: `send` records timing and
one event per batch member after the inner send resolves (rethrowing on failure with `status: "error"`);
`receive` records an `"inbound"` event before returning; `close` forwards. `receive` replaces the
`onmessage` interception — rmcp pulls rather than pushes, so there is no property to define and the
`defineProperty` try/catch has no analogue.
**Correction to the first pass:** this was the section's largest open question ("prove negotiation is
unchanged"), with three options and rmcp unread. It is settled. `serve_client_with_ct_inner` is generic over
`T: Transport<RoleClient>` and inspects no concrete type; `ClientLifecycleMode::{Initialize, Discover, Auto}`
probes `server/discover` over the **same** transport with a 10 s timeout before falling back to
`legacy_startup`. There is no disposable sibling process and no type sniffing, so the newtype is safe.
**the one real consequence** — `DynamicTransportError` records `transport_name: T::name()` and
`transport_type_id: TypeId::of::<T>()`, and `is::<T, R>()` / `downcast::<T, R>()` key on both. Wrapping
changes the error identity, so any downcast must target `TracingTransport<T>` or unwrap the inner error.
Name this in the type's doc comment and in the connection error-mapping path.
**verify** — a `cyrup-it` differential test: connect the same fixture server twice, once with tracing off
and once on, and assert the negotiated protocol version and the full request/response sequence are
identical.

**MCP-480 — wire the trace writer lifecycle into the server manager** · medium · S · `hand-written`
**upstream** — `server-manager.ts`: one lazily-created writer per manager shared across servers; HTTP
transports traced inside the connect path with an explicit kind and a `transportAlreadyTraced` flag
preventing double-wrapping; flush concurrent with `client.close()` under `allSettled`; a final flush in
`closeAll` after the configs are dropped.
**behavior** — the byte and event caps are session-global; a trace file is complete when the session ends; a
flush failure aggregates into the connection-cleanup error rather than being lost.
**cyrup** — `OnceCell<Arc<TraceWriter>>` on the manager; `tokio::join!` for the concurrent close+flush; the
aggregate error mirrors `AggregateError(failures, "MCP connection cleanup failed")`. With Cut 1 and Cut 3
there is exactly one instrumentation point per transport kind, so `transportAlreadyTraced` collapses to a
single decision at construction.
**verify** — `cyrup-it`: trace two servers in one session, assert one file, assert both servers' events are
in it, assert it is flushed at shutdown.

**MCP-481 — the trace settings surface** · low · S · `hand-written`
**upstream** — `types.ts` and `mcp-trace.ts`: `McpTraceSettings { enabled?, file?, maxBytes?, maxEvents? }`
declared with identical fields in two files; `ServerEntry.trace?: boolean`; wired from `init.ts`.
**behavior** — a user turns tracing on globally or for one server, and can redirect and re-budget the file.
**cyrup** — one `#[derive(Deserialize)]` struct, declared **once** — the upstream duplication is a TS
import-cycle artefact. `settings.trace` is an object while `definition.trace` is a bare bool; do not unify
them.
**verify** — unit on config parsing; assert per-server `false` beats global `true`.

#### Verification

**MCP-482 — tracker: the upstream verification surface** · n/a · S · `hand-written`
**upstream** — `__tests__/**` (96 `*.test.ts`, 25 125 lines, 1 061 cases; 11 fixtures, 517 lines), root
`*.test.ts` (5 files, 2 250 lines, 137 cases), `conformance/**` (5 files), `vitest.config.ts`,
`package.json`'s scripts block, `.github/workflows/ci.yml` — 101 test files, 1 198 cases, 27 375 lines, **two**
runners and one external referee.
**behavior** — n/a (index).
**cyrup** — indexes MCP-483..MCP-499. Note the counts in circulation are wrong in two directions: the brief's
"23 files under `__tests__/`" and "27 test files" are both low, and "26" is the number of *conformance
scenarios* in upstream's driver allowlist, not of test files.
**cut adjustment** — **12 of the 96 `__tests__` files pin cut surfaces only** and do not port:
`host-html-template`, `interactive-visualizer-server`, `proxy-modes-ui-messages`, `ui-integration`,
`ui-resource-handler`, `ui-server`, `ui-server-session-recovery`, `ui-session-messages`, `ui-streaming`,
`ui-viewer-none` (Cut 2), `server-manager-unix-socket` (Cut 3), `mcp-code` (Cut 4).
`ui-tool-visibility.test.ts` (47 lines, 3 cases) **splits** — the two cases covering
`extractUiToolVisibility` / `isUiToolVisibleToModel` port, the one covering `isUiToolCallableByApp` does not.
`consent-manager.test.ts`, `errors.test.ts` and `logger.test.ts` are **not** MCP-UI despite being grouped
that way in the first pass, and all three port. That leaves **84 vitest files plus 5 node:test files** in
scope.
**verify** — n/a.

**MCP-483 — adopt the MCP conformance harness as the port's protocol gate** · high · S · `hand-written`
**upstream** — `conformance/run.sh`, `driver.sh`, `package.json`'s `test:conformance`, and
`.github/workflows/ci.yml`: `npx conformance client --command "$DRIVER" --scenario X --expected-failures
conformance/baseline-client.yml --timeout 90000 --output-dir conformance/results`, with
`@modelcontextprotocol/conformance` pinned to **0.1.16**.
**behavior** — referee-graded protocol scenarios — initialize, tools/call, SEP-1034 elicitation defaults,
SSE retry, and the OAuth matrix — pass against a server neither implementation controls.
**cyrup** — **the harness is language-agnostic, and this is proved rather than inferred**: the rmcp checkout
ships `conformance/src/bin/client.rs` and a `.github/workflows/conformance.yml` that runs
`npx -y "@modelcontextprotocol/conformance@0.2.0-alpha.10" client --command "$(pwd)/target/debug/conformance-client"
--suite all --spec-version 2025-11-25` (and again for `2026-07-28`, and once more with `--suite extensions
--expected-failures conformance/expected-failures-extensions.yaml`). The client contract, read from rmcp's
own driver: the server URL arrives as **`argv[1]`**; the scenario name arrives in `MCP_CONFORMANCE_SCENARIO`;
per-scenario data arrives as JSON in `MCP_CONFORMANCE_CONTEXT` (`client_id`, `client_secret`,
`private_key_pem`, `signing_algorithm`, `toolCalls`); grading is entirely server-side and the client's only
other obligation is to exit 0. `--command` is split on **spaces**, so the binary path must contain none.
**version delta to decide** — upstream pins 0.1.16; rmcp runs 0.2.0-alpha.10, which is where `--suite` and
`--spec-version` come from. Track rmcp's pin, not the adapter's.
**scope reframing** — because rmcp's own CI runs the full versioned client suites with **no expected-failures
file**, the wire-level conformance of `cyrup-mcp` is largely gated upstream. cyrup's job is the *adapter*
half: real credential storage, the multi-tenant callback router, the real elicitation UI. Say that plainly
in the methodology instead of claiming the protocol matrix as cyrup's own achievement.
**verify** — the harness is the verification; the meta-check is that every non-baselined scenario passes and
no baselined entry has started passing (the CLI fails the run on a stale entry).

**MCP-484 — a hidden `cyrup mcp conformance-driver` subcommand** · high · M · `hand-written`
**upstream** — `conformance/driver.ts`: the 26-scenario allowlist with `process.exit(1)` on anything outside
it, the scripted elicitation UI, the `CONFORMANCE_DRIVER_DEBUG` trace hook, the definition builder, the
headless-browser OAuth round trip, `connectWithAuth`, `isAuthFailure`, `callTool` with mid-session re-auth,
the per-scenario action table, and the swallowing teardown.
**behavior** — the referee drives the *real* client stack — transport, needs-auth detection, elicitation
handler, OAuth discovery/DCR/PKCE/token exchange, real localhost callback server — not a bare protocol client.
**cyrup** — a clap subcommand hidden with `#[command(hide = true)]`, the second instance of the same pattern
as the `--mcp-keyring-helper` re-exec mode; design them together. **rmcp's `conformance/src/bin/client.rs` is
the working reference for everything below the adapter** — scenario dispatch, `ConformanceContext`
deserialisation (note it aliases `toolCalls`), the OAuth helpers, and a self-imposed
`MCP_CONFORMANCE_TIMEOUT_SECS` (default 25 s) so the client exits before the harness's 30 s kill, which rmcp
records as having wedged the harness process in CI. What cyrup adds on top is the adapter layer.
Keep four upstream behaviours: the **explicit allowlist** and its rationale (a scenario the driver does not
know must exit non-zero rather than pass on incidental initialize traffic); the scripted UI's preference
order `["Use default", "Submit", "Continue"]` then `options[0]`, which is what makes
`elicitation-sep1034-client-defaults` traverse the *real* form handler; `CONFORMANCE_DRIVER_DEBUG`, the only
diagnostic the harness offers; and `connectWithAuth`'s `MAX_AUTH_ROUND_TRIPS = 3`, i.e. up to **four** connect
attempts and three OAuth round-trips, not three attempts.
**note on the scripted UI** — reading it against §10.2.5: with `allowUrl:false`, a field with a `default`
picks `"Use default"`, the gate picks `"Continue"`, the review picks `"Submit"`. A field *without* a default
falls to `options[0]` — `"Enter value"` — and then `input` returns `undefined`, which cancels the whole form.
The scenario name ("client-defaults") tells you every field has a default.
**verify** — the conformance run itself; plus a `cyrup-it` `bin`-target test that the subcommand exists, is
hidden from `--help`, and exits non-zero on an unknown scenario.

**MCP-485 — a sequential runner with post-hoc log assertions** · medium · S · `hand-written`
**upstream** — `conformance/run.sh`: env-overridable results dir and 90 s timeout; `is_baselined` as a
literal `grep -Fqx "  - $1"` on the YAML text, not a YAML parse; `allows_client_error` hardcoding
`auth/scope-retry-limit` and `auth/resource-mismatch`; a focused `--scenario` path; refusal of `--suite` with
exit 2; live scenario discovery via `npx conformance list --client` + awk; a three-way outcome per scenario;
the closing line `All MCP client conformance scenarios passed or matched the reviewed baseline.`
**behavior** — the whole matrix runs green-or-red deterministically, and a client that crashes *after*
passing its wire checks still fails the run.
**cyrup** — a script or `xtask`, not a `#[test]`: it shells out to `npx` and must stay outside the cargo
gate. Two behaviours are easy to lose. (i) The **log greps** — conformance's baseline check only evaluates
wire checks and would otherwise hide a client-process failure after those checks ran; keep the
`Client timed out after` / `Client exited with code` post-hoc scans. (ii) **Sequential execution** — but
understand *why*: upstream refuses `--suite` because pre-registered OAuth clients bind an **exact** local
callback port, and the CLI's parallel mode creates false port-contention failures. That constraint is
inherited only if cyrup uses a fixed callback port; rmcp's CI runs `--suite all` in parallel precisely
because its driver does not. Decide which, and note that `docs/TEST-ARCHITECTURE.md`'s R4 ("bind `:0`, read
the assignment back") pushes toward the rmcp shape. Upstream's focused-scenario scan also precedes its
`--suite` refusal, so `--scenario X --suite` takes the focused path — preserve or deliberately fix, but
decide.
**verify** — run it; assert a deliberately-crashing driver fails a non-baselined scenario.

**MCP-486 — re-derive the expected-failures baseline; do not copy it** · medium · S · `hand-written`
**upstream** — `conformance/baseline-client.yml` has five entries, each with a protocol-level rationale
comment: `auth/basic-cimd`, `auth/scope-step-up`, `auth/2025-03-26-oauth-metadata-backcompat`,
`auth/client-credentials-jwt`, `auth/cross-app-access-complete-flow`. The README states the invariant: "A
failure in that file keeps the suite green; an unexpected failure or a baseline entry that starts passing
fails the run," and "Do not add a failure caused by the driver, callback-port contention, or test setup."
**behavior** — CI is green on known gaps, red on regressions, and red when a gap is fixed but the baseline is
not updated.
**cyrup** — **copying the file is unsafe, and the evidence is direct.** All five upstream entries are
scenarios rmcp's `conformance/src/bin/client.rs` implements: `basic-cimd` via
`AuthorizationRequest::with_client_metadata_url`, `client-credentials-jwt` behind rmcp's
`auth-client-credentials-jwt` feature, `cross-app-access-complete-flow`, `scope-step-up` and
`2025-03-26-oauth-metadata-backcompat` each with their own scenario arm — and **none of the five appears in
rmcp's `conformance/expected-failures-extensions.yaml`**, whose entire client list is four informational
extension scenarios (`auth/enterprise-managed-authorization`, `auth/dpop`, `auth/dpop-nonce`,
`auth/wif-jwt-bearer`). rmcp's versioned suites run with no baseline at all. Since a **passing baselined
entry fails the run**, every copied entry is a landmine. Start from an **empty** baseline, run once, and
write the file from the observed failures with a fresh rationale per entry. Keep the exact
`  - scenario` two-space indentation: upstream's `is_baselined` is a literal `grep -Fqx` on that text.
**Correction to the first pass:** it rated this an `open-decision` and judged three of the five entries
"likely to survive". The evidence says the opposite for at least `basic-cimd` and
`2025-03-26-oauth-metadata-backcompat`, which a Feb-2026 rmcp conformance audit records as PASS with 14/14
and 12/12 checks.
**verify** — the run; plus a check that every surviving baseline entry's comment cites a mechanism, not a
symptom.

**MCP-487 — allocate the ephemeral callback port in Rust** · low · S · `hand-written`
**upstream** — `conformance/driver.sh` per invocation: a fresh `mktemp -d` `MCP_OAUTH_DIR` with a
`trap cleanup EXIT HUP INT TERM`, `PI_MCP_ADAPTER_TEST_AUTH_STORE=memory`, an ephemeral
`MCP_OAUTH_CALLBACK_PORT` allocated by a two-line `node -e` (`createServer().listen(0,"127.0.0.1")`, print,
close), then `node --import tsx conformance/driver.ts "$@"`. The comment explains the port: pre-registered
browser clients require an exact callback port, so one is allocated per process, and the probe-then-release
TOCTOU is only safe because the runner is sequential.
**behavior** — each scenario process owns a distinct, currently-free loopback port.
**cyrup** — `std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port()`, then drop. Same mechanism —
bind `:0`, read back, release — in a different language, inside test scaffolding that never ships.
`docs/TEST-ARCHITECTURE.md`'s R4 says exactly this. rmcp's CI needs no port probe at all because its driver
does not use a fixed callback port; if cyrup's driver follows suit (MCP-485), this item disappears entirely.
**Correction to the first pass:** it filed this as an `open-decision` needing sign-off under the "port the
literal mechanism" rule. The mechanism *is* being ported; only the language of a two-line probe changes, and
the probe is test scaffolding. No decision needed.
**verify** — run two scenarios back to back; assert distinct ports and no bind failure.

**MCP-488 — record what conformance does not cover** · n/a · S · `hand-written`
**upstream** — the driver's per-scenario action table: one client action each, all over HTTP, all
single-server, all in-memory-auth.
**behavior** — n/a (a documentation invariant).
**cyrup** — the methodology must not claim conformance as the port's test strategy. The referee only sees the
wire between one client process and one MCP/OAuth server, so it says nothing about:
* **any adapter behaviour above the protocol** — config discovery and merge, agent plugins, the metadata
  cache and its hash, direct-tool naming/prefixing/exclusion, the `mcp` proxy tool's modes, search ranking,
  output guarding, tool approval, prompts-as-slash-commands, the two panels, onboarding. That is the large
  majority of the 84 in-scope vitest files.
* **sampling** — no client scenario exercises `sampling/createMessage`.
* **most of elicitation** — the only handler scenario is `elicitation-sep1034-client-defaults`, and it takes
  the defaults path only: no enum, no boolean, no array multi-select, no validation re-prompt, no Edit loop,
  no URL mode.
* **stdio** — every scenario serves over HTTP; the referee has no stdio mode, so `npx` resolution, process
  spawn/kill escalation, stderr capture and framing are untouched.
* **tracing** — nothing asserts a trace file exists or is redacted.
* **lifecycle** — lazy/eager/keep-alive, idle shutdown, reconnect, session fork/resume/reload, the
  runtime-owner generation fence.
* **multi-server anything** — every scenario is one server.
* **persistence** — no keychain, no `mcp-cache.json`; the driver forces an in-memory auth store and a
  throwaway OAuth dir.
Conformance is the *protocol* gate. It is necessary and it is nearly free; it is nowhere near sufficient.
**verify** — n/a.

**MCP-489 — the fate of the fixture MCP servers** · medium · M · `open-decision`
**upstream** — `__tests__/fixtures/`: nine stdio MCP servers plus two pi-host harness modules
(`direct-tools-child-harness.ts`, `direct-tools-agent-start-probe.ts`). Six are real SDK servers
(`elicitation-server.mjs` — form/url elicitations and `UrlElicitationRequiredError`; `prompts-server.mjs`;
`output-schema-server.mjs`; `tools-only-server.mjs`; `delayed-mcp-server.mjs`, which writes its PID to
`$MCP_RELOAD_PID_DIR` and traps SIGTERM/SIGINT; and `mcp-code-server.mjs`); three are **hand-rolled NDJSON**
servers that bypass the SDK to script the handshake precisely (`modern-discover-server.mjs`,
`legacy-no-discover-server.mjs`, `legacy-exits-on-discover-server.mjs`).
**behavior** — the integration tests exercise the client against a **real spawned process speaking real MCP
over real pipes**, including negotiation of the `server/discover` lifecycle.
**cyrup** — `mcp-code-server.mjs` goes with Cut 4, leaving eight. Options: (a) keep the `.mjs` fixtures and
spawn `node` in tests only; (b) rewrite them in Rust; (c) split — hand-roll the three NDJSON handshake
servers in Rust and keep the SDK-based ones as `.mjs`. **The calculus changed: rmcp ships a full Rust MCP
server** (`conformance/src/bin/server.rs`, 1 780 lines) plus `examples/servers`, so option (b) costs far less
than it looked, and `rmcp/server` would be a **dev-dependency only** — it never enters the shipped
`cyrup-mcp` feature set. **Recommendation: (c) leaning (b)** — hand-roll the three handshake servers (they
are already hand-rolled JSON writers), and build the SDK-based fixtures on rmcp's server role as
dev-dependencies, keeping one `.mjs` fixture only if a genuinely third-party implementation is wanted for
interop. Settled by: a ruling on whether `node` may be a test-only dependency, and on whether `rmcp/server`
may appear in `[dev-dependencies]`.
**verify** — the ported integration tests pass against whichever fixtures are chosen.

**MCP-490 — port the unit-testable share of the vitest suite** · high · L · `hand-written`
**upstream** — of the 84 in-scope `__tests__` files, roughly 65 are pure in-process unit tests with mocked
SDK/UI (`config` 1347/42, `direct-tools` 1171/50, `session-recovery` 400/30, `prompts` 290/25,
`tool-result-renderer` 325/27, `resolve-server-from-tool-name` 220/24, …).
**behavior** — every algorithm the adapter owns — naming, prefixing, exclusion, ranking, schema shaping,
output guarding, error taxonomy — is pinned by name and by message string.
**cyrup** — module-local `#[cfg(test)]` in `cyrup-mcp`, per `docs/TEST-ARCHITECTURE.md`'s rule that a crate's
`tests/` stays empty and a test earns a place in `cyrup-it` only by crossing one of four seams. `vi.mock` has
no Rust analogue: the tree's substitute is trait injection (`cyrup_test_support::scripted`) plus the faux
provider. Budget for the mocking rewrite, not just the assertion translation — the vitest suite mocks the
MCP client, `open`, `../config.ts` and `../server-manager.ts` heavily.
**verify** — the workspace gate; case-count parity against the in-scope upstream cases is the tracking metric.

**MCP-491 — a home for the MCP seam tests without breaking the 7-target cap** · medium · M · `open-decision`
**upstream** — roughly 18 in-scope files that spawn a process, bind a socket, or drive a real session:
`elicitation-sdk-integration`, `prompts-sdk-integration`, `server-manager-{streamable-http, stderr-capture,
legacy-handshake, auth-cache-recovery-integration}`, `session-recovery-integration`,
`structured-content-fallback-integration`, `output-schema-validation`, `resource-tool-materialization`,
`resources-capability`, `pi-reload-real-path`, `direct-tools-child-startup`,
`mcp-callback-server-{manual, unref}`.
**behavior** — the client is proven against real processes and real sockets, not against mocks.
**cyrup** — `crates/cyrup-it` declares exactly seven `[[test]]` targets with `autotests = false`, and
`docs/TEST-ARCHITECTURE.md` accepts only two justifications for an eighth (a crate-level `#![cfg(...)]` the
rest of the suite must not get, or process isolation for an aborting/global-state-mutating target), enforced
by a CI guardrail that counts test targets. MCP seam tests spawn processes and bind loopback sockets but meet
neither justification. Options: (a) fold them into `misc`; (b) fold the process-spawning ones into `bin` and
the session-driving ones into `session_svc`; (c) write the justification and raise the cap.
**Recommendation: (b)** — it matches the seam each test actually crosses, which is the criterion the rule is
written around, and it needs no doc amendment.
**second, independent hazard** — every `cyrup-it` target is `required-features = ["it"]`, which is what keeps
the suite off the everyday path, so a seam test placed there is **not** in the merge gate unless CI passes
`--features it` explicitly. Do not put a load-bearing MCP invariant there and assume it is gated.
**verify** — the chosen target runs green; the target-count guardrail still passes.

**MCP-492 — port the `node:test` OAuth suite as a serialised group** · high · M · `hand-written`
**upstream** — five root `*.test.ts` (2 250 lines, 137 cases) run only by `test:oauth` with
`--test-concurrency=1` and both auth env vars set inline, and **not** matched by `vitest.config.ts`'s
`include`. Note the deliberate basename collision: `__tests__/mcp-oauth-provider.test.ts` (vitest, 507 lines)
and `mcp-oauth-provider.test.ts` (node:test, 638 lines) are **different files testing the same module under
different runners**; a port that reads only one loses a large block of cases.
**behavior** — the credential store, the chunking manifest, the callback server, the OAuth provider and the
public token API are pinned by cases that must not run concurrently (they share process-global auth state and
bind ports).
**cyrup** — **the surface shrinks a lot.** `mcp-oauth-provider.test.ts` (638/38) tests DCR, protected-resource
metadata and AS-metadata discovery — all now `rmcp::transport::auth`'s job, and all covered by rmcp's own
conformance suite. What survives is the adapter's half: keychain storage and the chunking manifest
(`mcp-auth.test.ts`), the multi-tenant callback router (`mcp-callback-server.test.ts`), the flow wrapper
(`mcp-auth-flow.test.ts`) and the public API (`oauth-public-api.test.ts`). These belong in whichever
`cyrup-it` target MCP-491 settles on (they bind loopback ports). cargo has no per-target concurrency knob, so
serialisation must be explicit in the test code — a shared `Mutex` or `serial_test`, not `--test-threads=1`,
because the gate command is workspace-wide. Bind `:0` per R4 rather than reproducing a fixed port.
**effort note** — down from `L` because the protocol half is gone.
**verify** — the chosen target with the serial guard in place, run twice concurrently to prove it holds.

**MCP-493 — a Cargo/manifest policy test** · low · S · `hand-written`
**upstream** — `__tests__/package-manifest.test.ts` (84 lines, 5 cases) asserts every root runtime `.ts`
appears in `package.json.files`; that no runtime module imports the app-bridge; that the pi host packages are
optional peers with exact dev pins; and that the MCP client/core packages are pinned with no legacy SDK.
**behavior** — the published artifact cannot silently omit a module, and a dependency-policy violation fails
CI rather than shipping.
**cyrup** — a `#[test]` in `cyrup-mcp` that parses its own `Cargo.toml` and asserts the rmcp feature set is
exactly `default-features = false` plus `client`, `transport-child-process`,
`transport-streamable-http-client-reqwest`, `reqwest`, `auth` — in particular that **`server` is absent**
(it pulls `schemars`, `pastey`, `uuid` and `transport-async-rw` for a role the adapter never plays) and that
`elicitation` is absent (it only adds `url` for server-side code paths). Also assert the pinned rmcp version
and that no module references a removed dependency. The "every module is published" half has no analogue —
Cargo publishes by directory — and is dropped with reason.
**verify** — unit; break the manifest and watch it fail.

**MCP-494 — the CI gate's shape, including the conformance step** · medium · S · `open-decision`
**upstream** — `.github/workflows/ci.yml`, in order: `npm ci` → build the example → `typecheck` → `npm test`
→ `npm run test:oauth` → `npm run test:conformance` → `npm pack --dry-run`; Node 22; 60-minute timeout.
**behavior** — all three test surfaces plus a packaging smoke test gate every PR; none is optional.
**cyrup** — the two authoritative statements of cyrup's gate disagree and this must be resolved before the
job is written: the root manifest says the merge gate is `cargo test --workspace`, while
`docs/TEST-ARCHITECTURE.md`'s guardrail G5 says a single command silently drops doctests and specifies
`cargo nextest run --workspace && cargo test --workspace --doc`. Either way the gate **excludes** `cyrup-it`
(`required-features = ["it"]`). The job must run at least: the clippy typecheck analogue, the chosen gate, the
`cyrup-it` target with `--features it`, and the conformance runner. **`--all-features` must not appear** —
a guardrail greps for it and fails, because it silently re-arms the whole seam suite and unifies the faux
provider onto a normal edge. Adding an MCP target must also keep the test-target count guardrail intact
(MCP-491). rmcp's `conformance.yml` is the shape to copy for the conformance job, including the artifact
upload of results.
**verify** — the workflow file itself, plus re-running the documented guardrails.

**MCP-495 — reconcile the test-time environment contract with cyrup's isolation rules** · medium · S · `hand-written`
**upstream** — `vitest.config.ts` sets `PI_MCP_ADAPTER_TEST_AUTH_STORE=memory` and
`PI_MCP_ADAPTER_DISABLE_AUTH_CACHE=1` **process-wide for the whole suite** ("Cache tests opt in explicitly to
keep existing tests platform-neutral"); both are read through bracket access in `mcp-auth.ts`;
`MCP_OAUTH_DIR` is a per-scenario tempdir in `driver.sh`.
**behavior** — no test touches the developer's real OS keychain or real OAuth cache, and cache behaviour is
tested only where it is opted into.
**cyrup** — `docs/TEST-ARCHITECTURE.md` forbids exactly this mechanism (R2 — no `set_var`/`remove_var` in
test code, enforced by a `clippy.toml` `disallowed-methods` entry), alongside R1 tempdir-per-test, R3 no
`set_current_dir`, R4 bind `:0`, R5 ambient credentials scrubbed *and the scrub asserted*, R6 no
`process::exit`. So this is a forced divergence: inject the credential store as a constructor parameter,
which `cyrup-provider` already supports via its `CredentialStore` trait and in-memory implementation, and
which is also the shape rmcp wants (`AuthorizationManager::set_credential_store` takes a per-server store,
and rmcp ships `InMemoryCredentialStore` for exactly this). **Finding to carry:** the doc's own named escape
hatch, `cyrup_test_support::env::scoped`, does not exist — the crate declares no `env` module — so
constructor injection is the only compliant path, not merely the preferred one. Either write that module or
remove the doc's reference to it.
**verify** — R5's own assertion: a test that fails if a real credential is reachable.

**MCP-496 — live-pty verification for the elicitation dialogs and sampling gates** · high · M · `hand-written`
**upstream** — no analogue: pi's `ExtensionUIContext` is mocked in every adapter test, and the adapter never
renders a dialog itself.
**behavior** — the multi-step elicitation flow (gate → N field dialogs → review → edit → submit) is
*usable*, not merely correct: the select list is scrollable at 20+ options, long titles do not truncate the
question, the `✓ ` prefix renders, and Escape maps to cancel at every step.
**cyrup** — `HostServices::select` routes to the TUI selector, and the cancel-to-`None` path runs through the
default-UI-reply arm in both the TUI and RPC hosts. `cyrup_test_support::tui` provides `TestBackend` helpers,
but the project has been burned by exactly this shape: a `TestBackend` unit test passes while the assembled
app has a layout or empty-state bug. Elicitation is the adapter's only multi-dialog sequence and its worst
case (a 20-option multi-select with toggle state and `✓ ` prefixes) is not reachable from any upstream test.
Note the selector already has a documented rule that an **empty** options list must still open the dialog —
relevant because an array field with an empty `items.enum` produces exactly that.
**verify** — live pty: drive a fixture server that elicits one field of each widget kind plus a 20-option
multi-select, and screenshot each step. Not done until it has actually been run in a real terminal.

**MCP-497 — coverage tracking** · n/a · S · `cut`
**upstream** — `vitest.config.ts`'s v8 coverage over `*.ts`, exposed as `test:coverage` and **not** run in CI.
**cyrup** — out of scope by decision: the config gates nothing, and adding coverage tooling is a
workspace-wide decision unrelated to MCP. Recorded so a later pass knows it was seen and dropped, not missed.

**MCP-498 — the two child-process host harnesses** · medium · M · `hand-written`
**upstream** — `__tests__/direct-tools-child-startup.test.ts` `execFile`s a child pi process with
`MCP_DIRECT_TOOLS=demo/reload_identity` and asserts `demo_reload_identity` is registered **before**
`agent_start` from a cold cache; `__tests__/pi-reload-real-path.test.ts` drives a real session through a
reload and asserts, via PID files the fixture server writes to `$MCP_RELOAD_PID_DIR` and its SIGTERM/SIGINT
traps, that the old child process is actually reaped and a new one started.
**behavior** — the "tools exist before any server connects" trick works in a freshly-spawned agent, and a
session reload does not leak MCP child processes.
**cyrup** — the `cyrup-it` `bin` target (see MCP-491; do not create a new one). **Do not** use
`env!("CARGO_BIN_EXE_cyrup")` — a CI guardrail greps `crates/` for it and fails; the sanctioned convention is
the `CYRUP_IT_BIN_*` variables that `cyrup-it`'s build script resolves. The PID-file + signal-trap mechanism
is the *literal* mechanism for proving reaping and must be preserved (a Rust fixture writing the same
`{pid, toolName}` JSON is equivalent); do not substitute "assert the manager's map is empty", which would not
detect a leaked OS process.
**note** — the first test's premise interacts with `HA-1`: on a cold cache cyrup exposes only the `mcp` proxy
tool, so the "before `agent_start`" assertion is about the *warm* path unless `HA-1` is built. State which
you are testing.
**verify** — `cyrup-it` `bin` target; assert no surviving child PID after reload.

**MCP-499 — a trace-JSONL differential harness against the TS adapter** · medium · M · `open-decision`
**upstream** — the trace event is a *metadata-only, payload-free, deterministic-shaped* record of every
message in both directions, with `timestamp`, `bytes` and `durationMs` as the only non-deterministic fields.
**behavior** — n/a upstream (the tracer is a debugging aid).
**cyrup** — `cyrup_test_support::differential` already implements this pattern for the agent loop
(`run_differential`, `diff_sequences`, `canonicalize_cross_impl`, `diff_normalized`, …). Point both
implementations at the same fixture MCP server with tracing on, then diff the two JSONL files after dropping
`timestamp`, `bytes` and `durationMs`. The remaining fields — `direction`, `transport`, `kind`, `status`,
`method`, `id` shape, `errorCode` — are a complete, ordered description of the protocol exchange, which makes
this a cheap and very high-signal oracle for **exactly what conformance does not cover**: stdio, multi-server,
lifecycle, reconnect. It is a proposal, not a port. Settled by: deciding whether `pi-mcp-adapter` stays
checked out and installable in CI as a reference implementation during the port.
**verify** — bootstrap it on `initialize` + `tools/list` + one `tools/call` against the smallest fixture.

---

### Out of scope

Each of these is a **decision by the project owner**, recorded with its reason so a later pass does not
re-file it as a gap.

* **The legacy HTTP+SSE transport (CUT 1).** Supported transports are exactly `stdio` and `streamable
  HTTP`. rmcp 3.1.2 ships no SSE client transport at all — `crates/rmcp/src/transport.rs` exports
  `TokioChildProcess`, `StreamableHttpClientTransport` and `UnixSocketHttpClient` and nothing else on the
  client side; `client-side-sse` is only the SSE *frame parser* the streamable-HTTP client uses. Supporting
  it would mean hand-writing a protocol transport, which is the one thing the dependency decision exists to
  avoid.
  *In this section:* the trace `transport` field loses its `"sse"` value and the constructor-name sniffing
  that produced it (MCP-478); `traceTransportKind`'s HTTP disambiguation collapses to a single kind carried
  from the factory. **Not affected:** the conformance `sse-retry` scenario, which is about SSE **stream
  resumption inside streamable HTTP** — rmcp's own conformance client runs it with
  `StreamableHttpClientTransport` — and therefore stays in the matrix.
* **MCP Apps / the UI extension (CUT 2).** Entirely out. No `axum`, no local HTTP *server*, no `ui://`
  resource rendering, no iframe bridge.
  *In this section:* ten `__tests__` files pin it and do not port (`host-html-template`,
  `interactive-visualizer-server`, `proxy-modes-ui-messages`, `ui-integration`, `ui-resource-handler`,
  `ui-server`, `ui-server-session-recovery`, `ui-session-messages`, `ui-streaming`, `ui-viewer-none`), and
  `ui-tool-visibility.test.ts` splits — its `isUiToolVisibleToModel` cases port because that filter is kept
  (cutting it would expose to the model tools the server marked app-only), its `isUiToolCallableByApp` case
  does not. `MCP_UI_DEBUG`, `MCP_UI_VIEWER` and `GLIMPSE_BINARY` leave the environment-variable surface.
  Neither sampling, elicitation nor tracing has any Apps entanglement — verified: `elicitation-handler.ts`,
  `sampling-handler.ts` and `mcp-trace.ts` contain no `ui://` code and no `ext-apps` import.
* **The raw unix-socket transport (CUT 3).** rmcp's `UnixSocketHttpClient` is streamable-HTTP-over-UDS, a
  different wire shape from the adapter's raw framed socket; stdio plus streamable HTTP cover the field.
  *In this section:* the trace `transport` field loses `"unix-socket"`, and
  `__tests__/server-manager-unix-socket.test.ts` does not port.
* **`mcpScript` / the JavaScript worker (CUT 4).** This removes the only JS-engine question in the port —
  no `rquickjs`, no vendored C, no `boa`, no JS-in-WASM.
  *In this section:* `__tests__/mcp-code.test.ts` does not port, and `__tests__/fixtures/mcp-code-server.mjs`
  goes with it, leaving eight fixture servers for MCP-489. `node` remains only a *test-environment*
  question (MCP-489) and a *referee* question (the conformance CLI is invoked via `npx`, which is a
  third-party test harness, not a cyrup runtime).
* **Coverage tracking (MCP-497).** Gates nothing upstream; a workspace-wide tooling decision unrelated to
  MCP.

---

### What does not fit cleanly

**No host addition survives in this section.** The first pass filed two — a nested-completion verb on
`HostServices` (rated `critical`) and a widened available-model catalogue — and neither passes the two-part
test. Upstream reaches the provider layer *directly*, bypassing pi's host API, so a native crate doing the
same is the faithful port and a host verb would be the divergence. Everything else lands on
`HostServices::{confirm, select, input, notify, current_model, models, scoped_models, is_run_cancelled,
human_interaction_lock}` and `HostCtx::begin_human_wait`, all of which exist. The section's three
`host-addition` neighbours (`HA-1` late tool registration, `HA-2` argument completions, `HA-3` overlay
geometry) are owned elsewhere and none of them gates sampling, elicitation or tracing.

Four genuine open decisions remain, none blocking:

1. **`metadata` pass-through for sampling.** `CreateMessageRequestParams.metadata: Option<Value>` is handed
   to the provider verbatim upstream, and `cyrup-provider`'s `StreamOptions` has no metadata field. Options:
   (a) add one, which is a `cyrup-provider` change for one consumer; (b) drop `metadata` and record the
   divergence. **Recommendation: (b)** — no upstream test pins it, no MCP server in the wild is known to
   send it, and (a) widens a crate the port otherwise only reads. Settled by one line.
2. **`.pi/mcp-traces/` → `.cyrup/mcp-traces/`** (MCP-477). A user-visible path. **Recommendation:** rename,
   settled by the same decision that settles `mcp.json`'s path.
3. **Fixture strategy** (MCP-489): `.mjs` fixtures spawning `node` in tests, Rust fixtures on
   `rmcp/server` as a dev-dependency, or a split. **Recommendation:** hand-roll the three NDJSON handshake
   servers in Rust; build the rest on `rmcp/server` as a dev-dependency; keep at most one `.mjs` fixture for
   genuine third-party interop. Settled by a ruling on test-only `node` and on `rmcp/server` in
   `[dev-dependencies]`.
4. **Where the MCP seam tests live** (MCP-491) and **what the merge gate actually is** (MCP-494). Both are
   pre-existing workspace ambiguities the port surfaces rather than creates: the test-target cap accepts only
   two justifications and MCP meets neither, and the two authoritative statements of the gate disagree with
   each other. **Recommendation:** fold into `bin` and `session_svc` by the seam each test crosses, and
   adopt the two-command gate. Both need a ruling from whoever owns the test-architecture document.

One decision is *near*-settled and worth flagging as such: the conformance **baseline** (MCP-486). The
evidence says all five upstream entries pass under an rmcp stack, so the file should be written from an
observed empty-baseline run rather than copied. That is not an open question so much as an instruction not to
take the obvious shortcut.

---

### Coverage

**Read — upstream at v2.25.0**

* `sampling-handler.ts`, `elicitation-handler.ts`, `mcp-trace.ts` — every line.
* `conformance/run.sh`, `driver.sh`, `driver.ts`, `baseline-client.yml`, `README.md` — every line.
* `vitest.config.ts`, `package.json`, `.github/workflows/ci.yml` — every line.
* `__tests__/{sampling-handler, elicitation-handler, mcp-trace, elicitation-sdk-integration,
  init-elicitation, server-manager-sampling, package-manifest}.test.ts`;
  `__tests__/fixtures/elicitation-server.mjs`.
* Supporting regions in `server-manager.ts` (client construction, capability building, trace wiring, the
  elicitation-complete handler, `handleUrlElicitationRequired`, connection disposal), `init.ts` (sampling and
  elicitation enablement, trace config), `types.ts` (`McpTraceSettings`, `ServerEntry.trace`), `utils.ts`
  (`truncateAtWord`, `execOpen`/`openUrl`), `runtime-owner.ts` (`createOwnedUi`, `combineAbortSignals`),
  `logger.ts`, `mcp-auth.ts` (only far enough to locate the bracket-accessed env vars).
* All 96 + 5 `*.test.ts` enumerated mechanically at the tag, with line and case counts.
* Behavioural spot-checks executed against node: `redactTraceText` (6 inputs), `truncateAtWord` (2 inputs),
  `Number()` coercion (6 inputs).

**Read — rmcp at `rmcp-v3.1.2-7-gf713ebd`, from the checkout**

* `crates/rmcp/src/handler/client.rs` — the full `ClientHandler` trait and its `Service<RoleClient>` blanket
  impl, including every default body.
* `crates/rmcp/src/model.rs` — `CreateMessageRequestParams` (and its absence of a `task` field),
  `ContextInclusion`, `SamplingMessage`, `SamplingContent`, `SamplingMessageContentBlock`,
  `CreateMessageResult` and its `STOP_REASON_*` constants, `ElicitRequestParams`, `ElicitRequestParamsWire`
  (including the untagged `LegacyForm` arm), `ElicitResult`, `ElicitationAction`, `CustomNotification`,
  `NumberOrString`, `ErrorCode`, `ErrorData` and its `invalid_params`/`internal_error` constructors.
* `crates/rmcp/src/model/elicitation_schema.rs` — `PrimitiveSchemaDefinition`, `StringSchema`/`StringFormat`,
  `NumberSchema`, `IntegerSchema`, `BooleanSchema`, the `EnumSchema` family, `ElicitationSchema` and
  `ElicitationSchemaWire` including the `IndexMap` → `property_order` round trip.
* `crates/rmcp/src/model/capabilities.rs` — `ClientCapabilities`, `SamplingCapability`,
  `ElicitationCapability`, `FormElicitationCapability`, `UrlElicitationCapability`.
* `crates/rmcp/src/model/{task.rs, meta.rs}` — the tasks extension id and `RequestMetaObject`'s key set,
  enough to establish that sampling task augmentation is capability-negotiated, not a params field.
* `crates/rmcp/src/service/client.rs` — `ClientLifecycleMode`, `ClientServiceExt::serve_with_lifecycle`,
  `serve_client_with_lifecycle_and_ct`, `serve_client_with_ct_inner`, `DiscoverOutcome`.
* `crates/rmcp/src/service.rs` — `RequestHandle::cancel`, `PeerRequestOptions`,
  `RunningService::cancellation_token`.
* `crates/rmcp/src/transport.rs` — the `Transport<R>` trait, `IntoTransport`, `DynamicTransportError` and its
  `TypeId`-keyed `is`/`downcast`; the client-side export list.
* `crates/rmcp/src/transport/async_rw.rs` — `is_standard_method` / `is_standard_notification`, to establish
  that `notifications/elicitation/complete` is not first-class.
* `crates/rmcp/Cargo.toml` — the full feature graph.
* `conformance/Cargo.toml`, `conformance/src/bin/client.rs`, `conformance/expected-failures-extensions.yaml`,
  `conformance/results/2026-02-25-rust-sdk-assessment.md`, `.github/workflows/conformance.yml`.

**Excluded**

* **The OAuth subsystem's behaviour** — `mcp-auth.ts`, `mcp-auth-flow.ts`, `mcp-oauth-provider.ts`,
  `mcp-callback-server.ts`, `oauth.ts`, `oauth-handler.ts`. Owned by the auth section; this section covers
  only how they are *verified* (MCP-483, MCP-486, MCP-492).
* **`proxy-modes`, `direct-tools`, `tool-registrar`, `config`, `metadata-cache`, `prompts`, `lifecycle`,
  `npx-resolver`, `session-recovery`** — other sections. Their test files are counted here; their behaviour
  is not specified here.
* **Everything under Cut 2** — `ui-server.ts`, `ui-session.ts`, `host-html-template.ts`,
  `ui-resource-handler.ts`, `ui-stream-types.ts`, `ui-app-bridge-helpers.ts`, `glimpse-ui.ts`,
  `app-bridge.bundle.js`. Out of scope by decision, not deferred.
* **`mcp-code.ts` / `mcp-script-worker.mjs` / `skills/mcp-scripting`** — Cut 4. `__tests__/mcp-code.test.ts`
  is counted only as a test file that does not port.
* **`examples/interactive-visualizer/`** — a demo MCP server, not adapter code; it appears here only because
  upstream CI builds it and one test file touches it, and both go with Cut 2.
* **rmcp's server role and its `server`/`elicitation`/`schemars`/`macros` features** — not enabled by
  `cyrup-mcp`. `rmcp/server` is considered only as a possible *dev*-dependency for Rust test fixtures
  (MCP-489).
* **The MCP-450..MCP-499 id range is exhausted**, so findings from this pass were folded into the items they
  belong to rather than filed as new ids: rmcp's `property_order` into MCP-462, the untagged `LegacyForm` arm
  into MCP-460, `ErrorData::invalid_params` into MCP-472, the generic-transport proof and the
  `DynamicTransportError` identity consequence into MCP-479, the tasks-extension reframing into MCP-451,
  rmcp's own conformance CI into MCP-483/484/486, and the cut test-file census into MCP-482.

**Corrections to the first pass**

* **"Add a nested-completion verb to `HostServices`", rated `critical` (MCP-453)** — dissolved. Upstream
  reaches the provider layer directly and so does the port; a host verb would diverge from upstream, not
  match it. Re-verdicted `extension-owned`, `high`.
* **"Expose the cross-provider catalogue on the seam" (MCP-454)** — dissolved as a host concern.
  `cyrup_provider::catalog::{builtin_catalog, load_catalog}` is read directly; the observation that
  `HostServices::models()` is one provider's catalogue was correct, the conclusion was not.
* **"Preserve JSON document order" as a hand-written `high` item requiring a custom `MapAccess` impl
  (MCP-462)** — dissolved. `ElicitationSchema::property_order` is populated from an `IndexMap` at
  deserialise. The warning about not enabling `serde_json/preserve_order` workspace-wide is moot.
* **"rmcp's typed elicitation enum is stricter than upstream's string compare" (MCP-460)** — refuted.
  `ElicitRequestParamsWire::LegacyForm` is an untagged fallback; absent or unknown `mode` deserialises as
  form, exactly like upstream.
* **"Map `ProtocolError(InvalidParams)` onto rmcp's error type", `likely` (MCP-472)** — confirmed and
  demoted. `ErrorData::invalid_params` is `ErrorCode::INVALID_PARAMS = -32602`.
* **"Replace in-place transport instrumentation … and prove negotiation is unchanged", the section's largest
  open question (MCP-479)** — settled. rmcp's client lifecycle is generic over `T: Transport<RoleClient>`,
  inspects no concrete type, and probes `server/discover` over the same transport. A newtype is safe. The one
  surviving consequence is `DynamicTransportError`'s `TypeId`-keyed downcast, which is new information the
  first pass did not have.
* **"The conformance harness can be pointed at a Rust binary"** — the first pass inferred this from a
  de-minified npm bundle. It is now *demonstrated*: rmcp ships a Rust conformance client and a CI workflow
  that runs it, at harness version 0.2.0-alpha.10 rather than the adapter's 0.1.16, with `--suite` and
  `--spec-version` support the adapter's 0.1.16 runner did not have.
* **"Three of the five baseline entries likely survive" (MCP-486)** — refuted for at least two of them.
  rmcp's conformance client implements all five scenarios and baselines none; a Feb-2026 audit in the
  checkout records `auth/basic-cimd` and `auth/2025-03-26-oauth-metadata-backcompat` as PASS. Start from an
  empty baseline.
* **`node -e` port probe filed as needing mechanism sign-off (MCP-487)** — dissolved. The mechanism (bind
  `:0`, read back, release) is being ported; only the language of a two-line probe inside non-shipping test
  scaffolding changes. rmcp's driver does not need the probe at all.
* **Sampling `task` rejection as a runtime string guard (MCP-451)** — reframed. There is no `task` field on
  `CreateMessageRequestParams`; the guarantee comes from never declaring the
  `io.modelcontextprotocol/tasks` extension.
* **`traceId`'s non-finite-number branch (MCP-475)** — unreachable in Rust; rmcp's request id is
  `NumberOrString::{Number(i64), String(Arc<str>)}`.
* **`BoundedJsonlWriter` "skips over-budget lines and keeps going" (MCP-476)** — already corrected once in
  the first pass and re-confirmed: it latches, like upstream. The three real divergences are
  append-vs-truncate, async-fallible-vs-sync-silent, and the missing injectable-fs seam.
* **Test-file grouping (MCP-482)** — `consent-manager`, `errors` and `logger` were grouped under MCP-UI in
  the first pass. They are not MCP-UI and all three port. Twelve files pin cut surfaces only;
  `ui-tool-visibility.test.ts` splits.
* **Severity inflation** — the first pass rated five of this section's items `critical`
  (MCP-453, 455, 467, 471, 483). Two of those items dissolved entirely, two are re-rated `high` with the
  blocking-ness moved into the body, and one — MCP-455, the sampling consent gate — stays `critical` because
  inverting it is a permission bypass under the house scale's third clause.
