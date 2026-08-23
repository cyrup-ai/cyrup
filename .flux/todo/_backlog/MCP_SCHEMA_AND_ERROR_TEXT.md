---
stage: new
status: done
updated: 2026-08-22 16:12
---

# MCP: schema rendering, error taxonomy and the log channel

## Description

Eight `cyrup-mcp` port units that share one property: **they are pure, state-free vocabularies and
formatters.** No connection, no session, no seam with another crate. Each takes a value — a JSON
Schema, an error, a log site — and produces a string, a code or a target. That is why they can be
finished today without waiting on the spine, and it is why they belong to one agent: they all end
up asserted by fixture tables rather than by live servers, and three of them are literally two lines
apart in the same file.

The obligations, in one sentence each:

* **MCP-091 + MCP-098** — turn a JSON Schema into the TypeScript type literal the model reads,
  including the re-entrant alias loop that MCP-098 exists solely to stop a porter getting wrong.
* **MCP-211** — turn the same schema into the long-form `Parameters:` listing the model reads when
  MCP-091 declines.
* **MCP-093** — teach the JSON Schema validator the `format` keywords `ajv-formats` supplies and
  the `jsonschema` crate does not.
* **MCP-085 + MCP-089** — turn an error into a terminal-safe string, and into a code plus a
  recovery hint.
* **MCP-079** — the tool-approval decision and origin vocabularies, closed so a cut surface cannot
  re-enter through a string.
* **MCP-090** — give every log site an addressable target and one env-var level bootstrap.

**MCP-091 and MCP-098 cannot be split.** MCP-098 is a constraint on MCP-091's alias-emission loop,
and getting it wrong does not yield `None` — it yields a *wrong string* naming an undefined alias,
so the caller's raw-schema fallback at [proxy.rs:2244](../../crates/cyrup-mcp/src/proxy.rs) and
[:2464](../../crates/cyrup-mcp/src/proxy.rs) (which only fires on `None`) never runs and the model
is shown broken TypeScript. Nothing observable fails; the schema just reads wrong.

**MCP-211 is the sibling trait hole two lines away.** `format_schema`
([proxy.rs:1554](../../crates/cyrup-mcp/src/proxy.rs)) and `render_ts_shape`
([proxy.rs:1556](../../crates/cyrup-mcp/src/proxy.rs)) are declared on the same trait, called from
the same three sites, and stubbed only by the test `FakeEnv` at
[proxy.rs:5034/:5037](../../crates/cyrup-mcp/src/proxy.rs).

**Scheduling.** MCP-093 must land **inside MCP-092's module**, which is Wave 4 of
[MCP_HIGH_SEVERITY_BACKLOG.md](MCP_HIGH_SEVERITY_BACKLOG.md). Schedule this agent alongside Wave 4;
if Wave 4 lands `MCP-092` first, this task adds MCP-093's registrations to the module that unit
created. **Do not create a second validator module under any circumstances.** MCP-090 wires
`MCP_UI_DEBUG` into the subscriber; Wave 9's `MCP-068` is the unit that owns the *env-override
family* and will read the same variable — the constant and the predicate defined here are what
MCP-068 consumes, so this unit lands the reader and MCP-068 must not define a second one.

---

## Six corrections to the specs. Read these before writing any code.

The gap-analysis section files are wrong or incomplete in six places that change what gets built.
Every one was verified against the pinned source on disk.

### 1 · MCP-089's `ConsentError` arm does not exist post-cut. Do not add it.

[13b-mcp-config.md:1513-1526](../../docs/gap-analysis/13b-mcp-config.md) says "Post-cut the taxonomy
is the base shape + `ConsentError` + `McpServerError`", and its §10 table at
[:595](../../docs/gap-analysis/13b-mcp-config.md) marks `ConsentError` as **ports**.

The master file overrules it twice. [13-cyrup-mcp.md:418](../../docs/gap-analysis/13-cyrup-mcp.md):
"`consent-manager.ts` | **cut with Cut 2**, correcting the seam map's `A-5` row and its file table.
A grep at v2.25.0 shows `ConsentManager`'s only consumers are `ui-server.ts` and `ui-session.ts`."
And [:419](../../docs/gap-analysis/13-cyrup-mcp.md): "`errors.ts` | five of seven classes go with
Cut 2 … The surviving taxonomy is the base shape plus `McpServerError`." §10 itself records that
`ConsentError`'s only two production call sites were in `consent-manager.ts`
([13b:607](../../docs/gap-analysis/13b-mcp-config.md)) — so with that file cut, `ConsentError` has
**zero** surviving callers, exactly like `McpServerError`.

The Rust already got this right: [errors.rs:15-17](../../crates/cyrup-mcp/src/errors.rs) says "post-cut
that taxonomy is the base shape plus `McpServerError`". Follow the code and the master file, not
13b's stale §10 row. **Ship no `ConsentError` variant, no `CONSENT_DENIED`, no `CONSENT_REQUIRED`.**

### 2 · Do not change `McpError::Server`'s message template to upstream's.

MCP-089 asks for `#[error("…")]` templates reproduced "byte for byte". For `McpServerError` that
template is `` MCP server "${server}" error: ${reason} ``. The port's variant renders
`{server}: {message}` at [errors.rs:183](../../crates/cyrup-mcp/src/errors.rs).

Changing it would be a pure regression. §10 at
[13b:607-609](../../docs/gap-analysis/13b-mcp-config.md) states that `McpServerError` has **zero
upstream production call sites** — it exists only in `__tests__/errors.test.ts`. No upstream user
has ever seen that string, so byte-exactness buys nothing. Meanwhile `McpError::Server` has 14
construction sites in this crate (`runtime.rs:1100`, `:1783`, `:2460`, `:2907`, and ten more) whose
`message` is already the complete user-facing text, and its rendering flows into `record_failure`,
the `/mcp` panel and the gateway's `text_result`. Prefixing all of them with
`MCP server "x" error: ` would corrupt live output to fix nothing.

**Keep the template. Give the variant `code() == "MCP_SERVER_ERROR"` and upstream's recovery hint.**

### 3 · MCP-085's `formatTerminalError` walk is already landed. Only its tail is missing.

The ledger row at [13-cyrup-mcp-STATUS.md:588](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) says
"No function walks an error's children/`source()` chain with a cycle guard … de-duplicates and joins
with `": "`, then sanitises." Three of those four clauses are done:

* the children walk, the head-drop rule and the de-duplication are
  [`render_aggregate_texts`](../../crates/cyrup-mcp/src/errors.rs) at
  [errors.rs:143-161](../../crates/cyrup-mcp/src/errors.rs), and it is the `Display` of all seven
  `McpError` aggregates *and* of `ManagerError`
  ([server_manager.rs:249-274](../../crates/cyrup-mcp/src/server_manager.rs)) — measured against
  node 22, table at [errors.rs:44-51](../../crates/cyrup-mcp/src/errors.rs);
* the "aggregate whose children are all message-less" case is the empty-message skip at
  [errors.rs:150-155](../../crates/cyrup-mcp/src/errors.rs), asserted at
  [errors.rs:571](../../crates/cyrup-mcp/src/errors.rs);
* the **cycle guard is unbuildable and therefore unnecessary**: `CleanupErrors` is
  `CleanupErrors(Vec<McpError>)` ([errors.rs:463](../../crates/cyrup-mcp/src/errors.rs)) built by an
  owning `push`, and `ManagerError::Aggregate.children` is `Vec<Arc<ManagerError>>`
  ([server_manager.rs:186-190](../../crates/cyrup-mcp/src/server_manager.rs)) built by
  `ManagerError::aggregate(head, children)` from already-constructed values. Neither uses
  `Arc::new_cyclic` nor interior mutability, so an error cannot reach itself. Both walks are
  additionally budget-capped anyway ([errors.rs:443](../../crates/cyrup-mcp/src/errors.rs) `0..32`,
  [server_manager.rs:225](../../crates/cyrup-mcp/src/server_manager.rs) `budget = 1024`).

What is genuinely missing is named by the file itself at
[errors.rs:58-61](../../crates/cyrup-mcp/src/errors.rs): "**Residual:** `formatTerminalError`
finishes with `sanitizeTerminalText` … which is a terminal-rendering concern and is not applied by
`Display`". Two live `TODO(MCP-235)` comments at
[lifecycle.rs:1328](../../crates/cyrup-mcp/src/lifecycle.rs) and
[:1413](../../crates/cyrup-mcp/src/lifecycle.rs) say "route `error` through `sanitize_terminal_text`
once it lands" — **it landed**: MCP-235 is `implemented`
([13-cyrup-mcp-STATUS.md:749](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) and
`sanitize_terminal_text` is at [ui.rs:376](../../crates/cyrup-mcp/src/ui.rs).

**Do not write a second `source()` walk.** It would double-print: `McpError::Io`'s template is
`{path}: {source}` ([errors.rs:196](../../crates/cyrup-mcp/src/errors.rs)), so re-walking its source
yields `/p: denied: denied`. Write the sanitising tail and wire the two TODO sites.

### 4 · MCP-093's prescribed mechanism is a proven no-op for four of its eleven formats.

MCP-093 says to call `ValidationOptions::with_format(name, fn)` for each of
`url, int32, int64, float, double, byte, binary, password, iso-time, iso-date-time,
json-pointer-uri-fragment`.

`jsonschema` is pinned at `0.46.9` ([Cargo.lock:4183-4185](../../Cargo.lock), workspace declaration
at [Cargo.toml:176](../../Cargo.toml), crate edge at
[cyrup-mcp/Cargo.toml:118](../../crates/cyrup-mcp/Cargo.toml) with zero `.rs` references anywhere in
the crate). In that version, `CustomFormatValidator::is_valid` is:

```rust
// jsonschema-0.46.9/src/keywords/format.rs:1287-1293
fn is_valid(&self, instance: &Value, _ctx: &mut ValidationContext) -> bool {
    if let Value::String(item) = instance {
        self.check.is_valid(item)
    } else {
        true          // <-- every non-string instance passes, unconditionally
    }
}
```

`with_format` takes `F: Fn(&str) -> bool` (`src/options.rs:323-329`) and **only ever runs on string
instances**. `ajv-formats`' `int32`, `int64`, `float` and `double` are *numeric* formats
(`{type: "number", validate: …}`). Registering them through `with_format` is guaranteed dead code —
precisely the silent pass this unit exists to prevent.

Measured against `ajv-formats`' own definitions, the delta is smaller than the unit claims:

| ajv format | what ajv actually asserts | disposition |
|---|---|---|
| `float`, `double` | `() => true` — nothing | **do not register.** Registering a no-op is noise. |
| `password`, `binary` | `/[\s\S]*/` — matches anything | **do not register.** Same reason. |
| `int64` | `Number.isInteger(x)` | unreachable via `with_format`; and on the OpenAPI shape `{"type":"integer","format":"int64"}` it is fully subsumed by `type` |
| `int32` | `Number.isInteger(x) && -2^31 <= x <= 2^31-1` | unreachable via `with_format`; residual is the range check alone |
| `url`, `iso-time`, `iso-date-time`, `json-pointer-uri-fragment`, `byte` | string predicates | **register on both builders** |
| `duration`, `uuid` | string predicates | **register on the draft-07 builder ONLY** — see correction 5 |

Recovering `int32`/`int64` would require `with_keyword("format", …)`, which takes precedence over
the built-in (`src/compiler.rs:997-998`, comment: *"Check if this keyword is overridden, then check
the standard definitions"*) but replaces the whole `format` compile path — forfeiting all nineteen
built-in string formats unless they are reimplemented. That trade is not worth an `i32` range check.
**Ruling: register the string formats, do not chase the numeric ones, and lock the reason down with
a test** so no future maintainer "fixes" it by adding an inert `with_format("int64", …)` back.

### 5 · The format delta is per-draft, not shared. The unit's "both builders" is wrong.

`jsonschema 0.46.9`'s built-in dispatch gates several formats on draft
(`src/keywords/format.rs:1384-1412`):

* always: `date`, `date-time`, `email`, `hostname`, `idn-email`, `ipv4`, `ipv6`, `regex`, `time`, `uri`
* `draft >= 6`: `json-pointer`, `uri-reference`, `uri-template`
* `draft >= 7`: `idn-hostname`, `iri`, `iri-reference`, `relative-json-pointer`
* **`draft >= 2019-09`: `duration`, `uuid`**

So `duration` and `uuid` are built in on 2020-12 and **absent on draft-07**, where `ajv-formats`
supplies both. And because a user-registered format is consulted *first*
(`src/keywords/format.rs:1422-1428`), registering them on the 2020-12 builder would **shadow
jsonschema's better built-in with a hand-rolled one**. The registration list is therefore
draft-dependent: 5 formats on 2020-12, 7 on draft-07.

### 6 · Property order in both renderers is alphabetical, not document order. Lock it in.

Upstream iterates `Object.entries(properties)` — JSON document order. `serde_json` is declared
`{ version = "1" }` at [Cargo.toml:133](../../Cargo.toml) with **`preserve_order` enabled nowhere in
the workspace** (`grep preserve_order` over every `Cargo.toml`: zero hits), so `serde_json::Map` is a
`BTreeMap` and object keys iterate in lexicographic byte order.

Both `render_ts_shape` and `format_schema` walk `properties`, so both will render property lists
alphabetically where upstream renders them in document order. This is **not fixable here**: the
schema arrives as `ToolMetadata::input_schema: Option<Value>`
([proxy.rs:391](../../crates/cyrup-mcp/src/proxy.rs)), already deserialised, so document order is
gone before either function is called. It is *deterministic*, which is what actually matters — and
neither output is a prompt-cache key (that is MCP-213's `buildProxyDescription`, a different
string).

**Do not enable `preserve_order` to chase this.** It is a workspace-wide feature flip touching every
crate, and the ordered-reader question for `mcpServers` is already owned by MCP-212/MCP-094. Record
the divergence in the module doc and **write the golden tests to expect alphabetical order**, with
the fixture literal deliberately written in a different order so the test documents the behaviour.

---

## Per-unit breakdown

### MCP-091 — Port `renderTsShape` · medium · M · hand-written

Spec: [13b-mcp-config.md:1545-1556](../../docs/gap-analysis/13b-mcp-config.md), algorithm at
[§12, :649-695](../../docs/gap-analysis/13b-mcp-config.md).

**Unmet.** `grep -rn 'render_ts_shape\|renderTsShape\|ts_shape' --include='*.rs' crates/` returns
five hits, all of them plumbing: the trait declaration at
[proxy.rs:1556](../../crates/cyrup-mcp/src/proxy.rs), two call sites at
[:2244](../../crates/cyrup-mcp/src/proxy.rs) and [:2464](../../crates/cyrup-mcp/src/proxy.rs), and
the test stub at [:5037](../../crates/cyrup-mcp/src/proxy.rs) which returns the literal
`"{ a: string }"`. There is no implementation and no helper — no `UNSUPPORTED_KEYWORDS`, no
`aliasFor`, no `decodePointerToken`, no `renderLiteral`, nowhere in the workspace. There is no
production `impl ProxyEnv` at all: `grep 'ProxyEnv for'` finds exactly one, `FakeEnv` at
[proxy.rs:4932](../../crates/cyrup-mcp/src/proxy.rs).

**Extra obligation the spec does not name.** §12 wraps the whole body in `try {} catch { return
null }`. In Rust every documented failure is already an `Option`, so the catch has exactly one
residual meaning: **stack exhaustion on a deeply nested schema**. JS throws `RangeError` and the
catch returns `null`; Rust aborts the process. The crate is `#![forbid(unsafe_code)]` and
`#![deny(clippy::panic)]` ([lib.rs:118-124](../../crates/cyrup-mcp/src/lib.rs)) and neither helps
here. A depth cap returning `None` is the faithful port of the catch, and it is the same discipline
[errors.rs:443](../../crates/cyrup-mcp/src/errors.rs) and
[server_manager.rs:225](../../crates/cyrup-mcp/src/server_manager.rs) already apply.

**One place §12's prose does not implement.** Step 1 says the `$defs`/`definitions` collection
applies `decodePointerToken` to member names. That cannot be right: a `$defs` object's own keys are
literal names, never pointer-escaped, while the `$ref` token `#/$defs/a~1b` *is* escaped. Decoding
at collection and comparing against a raw `$ref` token can only ever fail to resolve. **Store keys
raw and decode the token on the `$ref` lookup side.** For every schema without a `~` in a `$defs`
member name — i.e. all of them — the two readings agree, and where they disagree this one resolves
and the other returns `None`, which §12 licenses as always-safe.

### MCP-098 — Preserve `renderTsShape`'s re-entrant alias emission · medium · S · hand-written

Spec: [13b-mcp-config.md:1652-1668](../../docs/gap-analysis/13b-mcp-config.md).

**Unmet, and unmeetable separately** — there is no loop to constrain until MCP-091 exists. The trap
is that JS `Map` iterators are live: `for (const [key, alias] of aliases)` visits entries that
`render(definition)` inserts *during* the loop. A Rust port that snapshots into a `Vec` emits a type
literal referencing a name it never defined. Because that is a *string*, not a `None`, the fallback
at [proxy.rs:2244-2247](../../crates/cyrup-mcp/src/proxy.rs) — which forks on `None` only — cannot
catch it.

### MCP-211 — `formatSchema` and its four helpers · medium · M · hand-written

Spec: [13e-mcp-tools.md:1009-1023](../../docs/gap-analysis/13e-mcp-tools.md), full algorithm at
[§4, :226-267](../../docs/gap-analysis/13e-mcp-tools.md).

**Unmet.** `grep -rn 'format_property\|format_variants\|format_nested_schema\|format_type\|
append_schema_annotations' --include='*.rs' crates/` returns **nothing**. `format_schema` itself is
the trait declaration at [proxy.rs:1554](../../crates/cyrup-mcp/src/proxy.rs), three call sites at
[:2246](../../crates/cyrup-mcp/src/proxy.rs), [:2467](../../crates/cyrup-mcp/src/proxy.rs) and
[:3574](../../crates/cyrup-mcp/src/proxy.rs), and the stub at
[:5034](../../crates/cyrup-mcp/src/proxy.rs) returning `format!("{indent}(schema)")`.

Its sibling `findToolByName` (MCP-210) **is** landed, at
[proxy.rs:758-764](../../crates/cyrup-mcp/src/proxy.rs), inside section 2 (`ToolMetadata` and the
tool-name grammar, [proxy.rs:373-772](../../crates/cyrup-mcp/src/proxy.rs)) — that is where the rest
of `tool-metadata.ts` lands in this port, and it is where `format_schema` goes.

Two JS behaviours the spec flags and one it does not:

* `Object.hasOwn(schema, "const")` is key **presence**, so `const: null` renders `const null` — use
  `JsonMap::contains_key`, never `get(..).is_some_and(..)`.
* `JSON.stringify` of a JSON value is `serde_json::to_string` — compact, no spaces.
* **Not flagged:** rule 3/4 of `formatType` uses `String(schema.type)` and rule 4 is gated on
  *truthiness*, not presence. `"type": ""` and `"type": 0` are falsy and fall through to rule 5.

### MCP-093 — Register the `ajv-formats` formats `jsonschema` does not ship · medium · S

Spec: [13b-mcp-config.md:1575-1585](../../docs/gap-analysis/13b-mcp-config.md).

**Unmet, and blocked on MCP-092.** `jsonschema` is declared at
[cyrup-mcp/Cargo.toml:118](../../crates/cyrup-mcp/Cargo.toml) with a 6-line comment stating exactly
what it is for, and `grep -rn jsonschema --include='*.rs' crates/cyrup-mcp/src/` returns **zero
hits**. The only user in the workspace is
[cyrup-ext-subagents/src/exec/structured.rs:72](../../crates/cyrup-ext-subagents/src/exec/structured.rs)
(`jsonschema::Validator::new`), a different mechanism. There is no `outputSchema` /
`structuredContent` validation anywhere: `renderers.rs` reads `structuredContent`
([:711](../../crates/cyrup-mcp/src/renderers.rs), [:1335](../../crates/cyrup-mcp/src/renderers.rs))
only to render and summarise it.

MCP-092 (Wave 4) builds the two `ValidationOptions` builders this unit registers onto. See
corrections 4 and 5 for what actually gets registered.

### MCP-085 — Port terminal sanitisation and error flattening · medium · M · hand-written

Spec: [13b-mcp-config.md:1450-1465](../../docs/gap-analysis/13b-mcp-config.md), behaviour at
[§9, :537-548](../../docs/gap-analysis/13b-mcp-config.md).

**Mostly landed.** `strip_osc_sequences` is at [ui.rs:309](../../crates/cyrup-mcp/src/ui.rs) (a
hand-written scanner over both `ESC ]` and C1 `U+009D`, consuming to end-of-string when
unterminated), `sanitize_terminal_text` at [ui.rs:376](../../crates/cyrup-mcp/src/ui.rs),
`sanitize_display_text` at [:422](../../crates/cyrup-mcp/src/ui.rs), and `truncate_at_word` at
[registration.rs:571](../../crates/cyrup-mcp/src/registration.rs) (measuring **UTF-16 code units**,
per the de-duplication note at [proxy.rs:766-770](../../crates/cyrup-mcp/src/proxy.rs)).

**Unmet:** the sanitising tail — see correction 3. Concretely, `grep format_terminal_error` over
`crates/` returns only doc-comment references; there is no function. And two error strings still
reach stderr raw, flagged in-tree: [lifecycle.rs:1328-1329](../../crates/cyrup-mcp/src/lifecycle.rs)
and [:1413-1414](../../crates/cyrup-mcp/src/lifecycle.rs).

**Where the sanitiser must NOT be applied.** The gateway's model-facing text at
[proxy.rs:2847](../../crates/cyrup-mcp/src/proxy.rs) and
[:3486](../../crates/cyrup-mcp/src/proxy.rs) (`let message = error.to_string();` →
`Failed to connect to "{server}": {message}`) is **upstream-faithful as-is** — upstream does not
sanitise the tool-result path either. Sanitising it would be a silent divergence. The `/mcp` panel
already sanitises at ingest ([ui.rs:2330](../../crates/cyrup-mcp/src/ui.rs),
[:2386](../../crates/cyrup-mcp/src/ui.rs), [:2435](../../crates/cyrup-mcp/src/ui.rs),
[:2446](../../crates/cyrup-mcp/src/ui.rs)). The stderr log sites are the gap, and sanitising them is
a *justified cyrup divergence*: pi's `console.error` goes to pi's own log, while cyrup's `tracing`
writer is `io::stderr` ([main.rs:2350](../../crates/cyrup/src/main.rs)) — the same terminal the TUI
paints.

### MCP-089 — Port the error taxonomy · medium · S · hand-written

Spec: [13b-mcp-config.md:1513-1526](../../docs/gap-analysis/13b-mcp-config.md), table at
[§10, :582-616](../../docs/gap-analysis/13b-mcp-config.md).

**Unmet.** `grep -rn 'fn code(\|recovery_hint' --include='*.rs' crates/` finds `code()` on
`cyrup_provider::error` (`:44`, `:125`) and `cyrup_config::login` (`:181`), and in `cyrup-mcp`
finds **only the doc comment at [errors.rs:14](../../crates/cyrup-mcp/src/errors.rs)** that promises
them. `McpError` ([errors.rs:167](../../crates/cyrup-mcp/src/errors.rs)) has `is_cleanup_aggregate`,
`aggregate_head`, `aggregate_children` and `is_cleanup_failure` — no `code`, no `recovery_hint`, no
`context`.

**This is vocabulary with no production consumer, deliberately, and that must be said in the code.**
The spec's own §10 records that `wrapError` "survives as taxonomy with no caller until another
subsystem needs it", and every message the proxy emits is already byte-exact upstream text built
from `McpErrorCode` — see `disabled_call_result` at
[proxy.rs:3025-3039](../../crates/cyrup-mcp/src/proxy.rs). Injecting `recovery_hint()` into any of
those strings would *break* parity. The methods exist so the taxonomy cannot rot silently, and the
exhaustive match (no `_` arm) is what enforces that.

**Do not conflate the two code vocabularies.** `crate::proxy::McpErrorCode`
([proxy.rs:203-271](../../crates/cyrup-mcp/src/proxy.rs), 32 arms) is `proxy-modes.ts`'s
`details.error` value for the `mcp` gateway tool's JSON result. `McpError::code()` is
`errors.ts`'s `McpUiError.code`. They are different things with different consumers.

### MCP-079 — Port the tool-approval decision and origin types · medium · S · hand-written

Spec: [13b-mcp-config.md:1346-1362](../../docs/gap-analysis/13b-mcp-config.md).

**Half landed.** `ApprovalOrigin` is at [proxy.rs:1333-1341](../../crates/cyrup-mcp/src/proxy.rs)
with the three surviving arms, both derivations (`for_proxy_call` at
[:1347](../../crates/cyrup-mcp/src/proxy.rs), `for_direct_tool` at
[:1362](../../crates/cyrup-mcp/src/proxy.rs) — they differ **only** in their fallback, which is why
both are written out) and `as_str` at [:1369](../../crates/cyrup-mcp/src/proxy.rs). `ApprovalOutcome`
(upstream `ToolCallApprovalResult`) is at
[proxy.rs:1380-1388](../../crates/cyrup-mcp/src/proxy.rs).

**Unmet:** `McpToolApprovalDecision`. `grep -rn 'allow_once\|allow_for_session\|AllowOnce\|abstain'`
over `crates/cyrup-mcp` finds only prose at [proxy.rs:4772-4774](../../crates/cyrup-mcp/src/proxy.rs)
and two test names at [:7198](../../crates/cyrup-mcp/src/proxy.rs), [:7231](../../crates/cyrup-mcp/src/proxy.rs).
Today the decision is a bare string match on dialog labels at
[proxy.rs:4842-4855](../../crates/cyrup-mcp/src/proxy.rs) against `APPROVE_ONCE_OPTION`
([:119](../../crates/cyrup-mcp/src/proxy.rs)) and `APPROVE_FOR_SESSION_OPTION`
([:122](../../crates/cyrup-mcp/src/proxy.rs)).

**The unsettled verify, and the ruling.** The spec's verify is "a `script`/`iframe` origin string is
rejected by the deserializer rather than silently accepted." `ApprovalOrigin` derives
`Debug, Clone, Copy, PartialEq, Eq` and **nothing from serde**
([proxy.rs:1332](../../crates/cyrup-mcp/src/proxy.rs)) — there is no deserializer, so the clause has
no surface.

**Ruling: do not add serde.** The broker event that carried these strings is MCP-233's cut
([proxy.rs:4768-4774](../../crates/cyrup-mcp/src/proxy.rs)), cyrup's `SharedBus` is JSON-only and
deferred, and the origin's *only* use is the write-side `as_str` for a `details.origin` key
(the only reader is the assertion at [proxy.rs:7588-7591](../../crates/cyrup-mcp/src/proxy.rs)). A `Deserialize` derive would invent a wire
format with no producer and then become a compatibility surface someone must keep stable. The
obligation the verify is *reaching for* — that a cut arm cannot re-enter through a string — is
discharged by making the string→enum direction **total and explicit**: a `parse` that returns `None`
for `"script"`, `"iframe"` and `"abstain"`, and exhaustive `as_str` matches with no `_` arm so the
vocabulary cannot gain an arm without a compile error.

`abstain` is correctly absent and the reason is already recorded at
[proxy.rs:4772-4774](../../crates/cyrup-mcp/src/proxy.rs): "a permission extension that declines to
decide simply does not block, which lands in the same place".

### MCP-090 — Port the logger as a `tracing` adapter · low · S · extension-owned

Spec: [13b-mcp-config.md:1530-1543](../../docs/gap-analysis/13b-mcp-config.md), §11 at
[:618-647](../../docs/gap-analysis/13b-mcp-config.md).

**Unmet, in one of its three halves.** `grep -rn 'MCP_UI_DEBUG\|MCP-UI' --include='*.rs'
crates/cyrup-mcp/src/` returns **zero hits**. There is no level bootstrap.

**Two halves the ledger row calls missing are not.** The row at
[13-cyrup-mcp-STATUS.md:593](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) says there is "no stable
`tracing` target". There is: `tracing`'s default target **is the module path**, so all 65 call sites
in this crate (`tracing::debug!` ×22, `info!` ×4, `warn!` ×29, `error!` ×10 — zero use `target:`)
are already addressable as `cyrup_mcp`, `cyrup_mcp::oauth`, `cyrup_mcp::lifecycle`, … through
`RUST_LOG`. **Adding an explicit `target: "MCP-UI"` to all 65 would make them strictly less
addressable** by flattening that granularity. Do not do it. The workspace precedent confirms the
rule: `cyrup-ext-subagents` uses an explicit target *only* to carve out a sub-channel, spelled as a
module path — [spawn/mod.rs:1050](../../crates/cyrup-ext-subagents/src/spawn/mod.rs)
(`target: "cyrup_ext_subagents::child_stderr"`) and `extension.rs:3940`.

**The one channel that does need carving out** is §11's second one: config-load warnings that
upstream emits as bare `console.warn`, bypassing the logger. Eight sites:
[config.rs:1862](../../crates/cyrup-mcp/src/config.rs), [:1876](../../crates/cyrup-mcp/src/config.rs),
[:1899](../../crates/cyrup-mcp/src/config.rs), [:1946](../../crates/cyrup-mcp/src/config.rs)
(`push_load_warning`), [:2331](../../crates/cyrup-mcp/src/config.rs), and
[agent_plugin.rs:493](../../crates/cyrup-mcp/src/agent_plugin.rs),
[:524](../../crates/cyrup-mcp/src/agent_plugin.rs), [:1446](../../crates/cyrup-mcp/src/agent_plugin.rs).

**What "unfiltered" means here, stated rather than silently changed.** Upstream's `console.warn`
ignores the logger's `minLevel`. `tracing` has no bypass, and writing to stderr directly would
corrupt the TUI. It does not matter: `MCP_UI_DEBUG` only ever *raises* verbosity, so the two channels
can never disagree in the direction upstream cares about. A user who sets `RUST_LOG=cyrup_mcp=error`
has asked to suppress warnings. Distinctness is delivered by the target.

**Dropped with a reason, per the spec:** the pluggable handler list. No analogue, no consumer.

---

## Implementation

Everything below is prescriptive. Types and idioms match this crate:
`serde_json::{Map as JsonMap, Value}` and `indexmap::IndexMap` are already imported at
[proxy.rs:75-77](../../crates/cyrup-mcp/src/proxy.rs); `LazyLock<Option<Regex>>` with `.ok()` is the
house pattern for a static regex under the no-panic policy
([agent_plugin.rs:148-149](../../crates/cyrup-mcp/src/agent_plugin.rs)); `clippy::unwrap_used`,
`expect_used`, `panic` and `indexing_slicing` are `deny` crate-wide
([lib.rs:118-124](../../crates/cyrup-mcp/src/lib.rs)), so use `.get()`, `.get_index()` and
`unwrap_or_*` throughout.

### A · New module `crates/cyrup-mcp/src/ts_shape.rs` (MCP-091 + MCP-098)

Register it in [lib.rs](../../crates/cyrup-mcp/src/lib.rs) as `pub mod ts_shape;` in the
alphabetical block at `:126-146`, and add a row to the module map table at `:88-102`:
`| [`ts_shape`] | `ts-shape.ts` | JSON Schema → a TypeScript type literal, for the model |`.

```rust
//! `ts-shape.ts` — the useful JSON Schema subset as a TypeScript type literal (MCP-091, MCP-098).
//!
//! `None` is upstream's `null` and means **"fall back to the raw schema"**: both callers
//! ([`crate::proxy::execute_describe`], [`crate::proxy::execute_search`]) have a fallback beside
//! the call, so returning `None` more often is a verbosity regression, never a correctness one.
//! Returning a *wrong string* is caught nowhere — which is the whole of MCP-098.
//!
//! **Divergence (deliberate).** `serde_json` is declared without `preserve_order`, so
//! `properties` renders in lexicographic key order where upstream renders document order. The
//! schema arrives already deserialised, so document order is unrecoverable here. Deterministic,
//! and neither output is a prompt-cache key.

use std::collections::HashSet;

use indexmap::IndexMap;
use serde_json::{Map as JsonMap, Value};

/// `UNSUPPORTED_KEYWORDS` — re-tested at EVERY node, not only the root.
const UNSUPPORTED_KEYWORDS: [&str; 7] =
    ["if", "then", "else", "allOf", "not", "patternProperties", "additionalProperties"];

/// The Rust half of upstream's `try {} catch { return null }`.
///
/// Every documented failure below is already an `Option`, so the catch has exactly one residual
/// meaning in Rust: stack exhaustion on a pathological schema, which JS turns into a `RangeError`
/// and this turns into `None`. Same budget discipline as [`crate::errors::McpError::is_cleanup_failure`].
const MAX_RENDER_DEPTH: u32 = 64;

/// `renderTsShape(schema)` — the whole of `ts-shape.ts`.
#[must_use]
pub fn render_ts_shape(schema: &Value) -> Option<String> {
    let root = schema.as_object()?;

    let mut defs: IndexMap<String, &Value> = IndexMap::new();
    collect_definitions(root, "$defs", &mut defs)?;
    collect_definitions(root, "definitions", &mut defs)?;

    let mut aliases = Aliases::default();
    let rendered_root = render(schema, &defs, &mut aliases, 0)?;

    // MCP-098 — the re-entrant emission loop. `render` below can INSERT into `aliases.map`, and a
    // `$ref` registered inside a `$defs` member must itself be visited and emitted. JS `Map`
    // iterators are live; `for (k, v) in &aliases` and any pre-collected snapshot are BOTH wrong
    // and both fail silently, emitting a shape that names an undefined alias.
    let mut lines: Vec<String> = Vec::new();
    let mut index = 0usize;
    while index < aliases.map.len() {
        let Some((key, alias)) = aliases.map.get_index(index).map(|(k, a)| (k.clone(), a.clone()))
        else {
            break;
        };
        let definition = *defs.get(&key)?;
        let body = render(definition, &defs, &mut aliases, 0)?;
        lines.push(format!("type {alias} = {body};"));
        index += 1;
    }

    if lines.is_empty() {
        return Some(rendered_root);
    }
    Some(format!("{}\n\n{rendered_root}", lines.join("\n")))
}

/// §12 step 1. A non-object group, or a non-object member, aborts the whole render.
///
/// Keys are stored RAW. `decodePointerToken` belongs on the `$ref` LOOKUP side — a `$defs` object's
/// own keys are literal names, never pointer-escaped, so decoding here could only ever make a
/// `~`-bearing name unresolvable. See the task file's correction 6 for the reasoning.
fn collect_definitions<'a>(
    root: &'a JsonMap<String, Value>,
    group: &str,
    out: &mut IndexMap<String, &'a Value>,
) -> Option<()> {
    let Some(raw) = root.get(group) else { return Some(()) };
    let members = raw.as_object()?;
    for (name, member) in members {
        if !member.is_object() {
            return None;
        }
        out.insert(format!("{group}/{name}"), member);
    }
    Some(())
}

/// The alias table — insertion-ordered, index-addressable, and grown DURING the emission loop.
#[derive(Default)]
struct Aliases {
    /// `"$defs/<name>"` → the emitted alias, first-referenced-first. Never a `BTreeMap`.
    map: IndexMap<String, String>,
    /// Every alias handed out, so `alias_for` never collides.
    used: HashSet<String>,
    /// `aliasIndex` — the `Definition{n}` counter.
    next: u32,
}

impl Aliases {
    /// `aliasFor(key)`: reuse the bare name when it is an identifier and unused, else
    /// `Definition${++aliasIndex}`, incrementing until unique.
    fn alias_for(&mut self, key: &str) -> String {
        if let Some(existing) = self.map.get(key) {
            return existing.clone();
        }
        let bare = key.split_once('/').map_or(key, |(_, name)| name);
        let alias = if is_identifier(bare) && !self.used.contains(bare) {
            bare.to_string()
        } else {
            loop {
                self.next = self.next.saturating_add(1);
                let candidate = format!("Definition{}", self.next);
                if !self.used.contains(&candidate) {
                    break candidate;
                }
            }
        };
        self.used.insert(alias.clone());
        self.map.insert(key.to_string(), alias.clone());
        alias
    }
}

/// `render(schema)` — §12 step 3's precedence order, exactly.
fn render(
    schema: &Value,
    defs: &IndexMap<String, &Value>,
    aliases: &mut Aliases,
    depth: u32,
) -> Option<String> {
    if depth > MAX_RENDER_DEPTH {
        return None;
    }
    let map = schema.as_object()?;
    if has_unsupported_keyword(map) {
        return None;
    }

    // 1 · `$ref` — `/^#\/(\$defs|definitions)\/([^/]+)$/`, and it must resolve.
    if let Some(reference) = map.get("$ref") {
        let rest = reference.as_str()?.strip_prefix("#/")?;
        let (group, token) = rest.split_once('/')?;
        if (group != "$defs" && group != "definitions") || token.contains('/') {
            return None;
        }
        let key = format!("{group}/{}", decode_pointer_token(token));
        if !defs.contains_key(&key) {
            return None;
        }
        return Some(aliases.alias_for(&key));
    }

    // 2 · `enum`
    if let Some(members) = map.get("enum").and_then(Value::as_array) {
        let rendered = members.iter().map(render_literal).collect::<Option<Vec<_>>>()?;
        return Some(rendered.join(" | "));
    }

    // 3 · `const` — `Object.hasOwn`, so `const: null` takes this branch.
    if map.contains_key("const") {
        return render_literal(map.get("const")?);
    }

    // 4 · `anyOf` / `oneOf`, `anyOf` preferred; empty ⇒ `None`.
    if let Some(variants) =
        map.get("anyOf").or_else(|| map.get("oneOf")).and_then(Value::as_array)
    {
        if variants.is_empty() {
            return None;
        }
        let rendered = variants
            .iter()
            .map(|variant| render(variant, defs, aliases, depth + 1))
            .collect::<Option<Vec<_>>>()?;
        return Some(rendered.join(" | "));
    }

    let type_field = map.get("type");

    // 5 · object
    if type_field.and_then(Value::as_str) == Some("object") || map.contains_key("properties") {
        let Some(properties) = map.get("properties").and_then(Value::as_object) else {
            return Some("{}".to_string());
        };
        if properties.is_empty() {
            return Some("{}".to_string());
        }
        let required = required_names(map);
        let mut parts = Vec::with_capacity(properties.len());
        for (name, property) in properties {
            let rendered = render(property, defs, aliases, depth + 1)?;
            let optional = if required.contains(name.as_str()) { "" } else { "?" };
            parts.push(format!("{}{optional}: {rendered};", format_property_name(name)));
        }
        return Some(format!("{{ {} }}", parts.join(" ")));
    }

    // 6 · array — the item is parenthesised when it is itself a union.
    if type_field.and_then(Value::as_str) == Some("array") {
        let Some(items) = map.get("items") else { return Some("unknown[]".to_string()) };
        let item = render(items, defs, aliases, depth + 1)?;
        return Some(if item.contains(" | ") { format!("({item})[]") } else { format!("{item}[]") });
    }

    // 7 · `type: [..]`
    if let Some(list) = type_field.and_then(Value::as_array) {
        let rendered = list
            .iter()
            .map(|entry| entry.as_str().and_then(render_type))
            .collect::<Option<Vec<_>>>()?;
        return Some(rendered.join(" | "));
    }

    // 8 · `type: "..."`
    if let Some(name) = type_field.and_then(Value::as_str) {
        return render_type(name);
    }

    // 9 · fallback
    Some("unknown".to_string())
}

/// `additionalProperties: false` is a closed-object CONSTRAINT, not a shape — it is exempt, and the
/// test is repeated at every node.
fn has_unsupported_keyword(map: &JsonMap<String, Value>) -> bool {
    UNSUPPORTED_KEYWORDS.iter().any(|keyword| match map.get(*keyword) {
        None => false,
        Some(value) if *keyword == "additionalProperties" => *value != Value::Bool(false),
        Some(_) => true,
    })
}

/// `renderType` — the six-way map. Anything else is `None`.
fn render_type(name: &str) -> Option<String> {
    Some(
        match name {
            "string" => "string",
            "number" | "integer" => "number",
            "boolean" => "boolean",
            "null" => "null",
            "object" => "{}",
            "array" => "unknown[]",
            _ => return None,
        }
        .to_string(),
    )
}

/// `renderLiteral`. `Number.isFinite` is satisfied by construction: `serde_json` cannot hold NaN or
/// Infinity, so a `Value::Number` is always finite.
fn render_literal(value: &Value) -> Option<String> {
    match value {
        Value::Null | Value::String(_) | Value::Bool(_) => serde_json::to_string(value).ok(),
        Value::Number(number) => Some(number.to_string()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// `formatPropertyName`.
fn format_property_name(name: &str) -> String {
    if is_identifier(name) {
        name.to_string()
    } else {
        serde_json::to_string(name).unwrap_or_else(|_| format!("{name:?}"))
    }
}

/// `/^[A-Za-z_$][\w$]*$/`, where `\w` is `[A-Za-z0-9_]`.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// `decodePointerToken`. RFC 6901 order: `~1` BEFORE `~0`, or `~01` decodes to `/` instead of `~1`.
fn decode_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// The string members of `required`, or the empty set.
fn required_names(map: &JsonMap<String, Value>) -> HashSet<&str> {
    map.get("required")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}
```

### B · `format_schema` and its four helpers (MCP-211)

Land in [proxy.rs](../../crates/cyrup-mcp/src/proxy.rs) **immediately after `find_tool_by_name`**
(its body closes at `:764`) and before the `truncate_at_word` re-export at `:771`, inside section 2. `HashSet` needs
adding to the `use std::collections::…` line at the top of the file.

```rust
/// The annotation keys, in `appendSchemaAnnotations`' exact order. `default` is appended after
/// these and is NOT part of the list.
const SCHEMA_ANNOTATION_KEYS: [&str; 8] =
    ["minLength", "maxLength", "minimum", "maximum", "minItems", "maxItems", "format", "pattern"];

/// Same rationale as [`crate::ts_shape`]'s cap: `formatSchema` has no `null` channel, so an
/// over-deep schema degrades to `(complex schema)` rather than exhausting the stack.
const MAX_SCHEMA_DEPTH: u32 = 64;

/// `tool-metadata.ts` `formatSchema(schema, indent = "  ")` (MCP-211).
///
/// Model-facing text: it is the body of `mcp({describe})` and the `Expected parameters:` suffix on
/// the direct-tool `tool_error` / `call_failed` results — **those two only**. Drift changes retry
/// behaviour. Note [`execute_describe`] passes `"  "` and [`execute_search`] passes `"    "`.
///
/// **Divergence (deliberate).** `serde_json` has no `preserve_order`, so `properties` renders in
/// lexicographic key order where upstream renders document order. Unrecoverable at this seam — the
/// schema is already deserialised — and deterministic.
#[must_use]
pub fn format_schema(schema: &Value, indent: &str) -> String {
    // `typeof schema !== "object" || schema === null || Array.isArray(schema)`
    let Some(map) = schema.as_object() else { return format!("{indent}(no schema)") };

    if map.get("type").and_then(Value::as_str) == Some("object")
        && let Some(properties) = map.get("properties").and_then(Value::as_object)
    {
        if properties.is_empty() {
            return format!("{indent}(no parameters)");
        }
        let required = schema_required_names(map);
        return properties
            .iter()
            .map(|(name, property)| {
                format_property(name, property, required.contains(name.as_str()), indent, 0)
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    let nested = format_nested_schema(map, indent, 0);
    if !nested.is_empty() {
        return nested.join("\n");
    }
    let type_str = format_type(map);
    if !type_str.is_empty() {
        return format!("{indent}({type_str})");
    }
    format!("{indent}(complex schema)")
}

/// `formatProperty(name, schema, required, indent)` — parts joined by ONE space, then the nested
/// block at `indent + "  "`.
fn format_property(
    name: &str,
    schema: &Value,
    required: bool,
    indent: &str,
    depth: u32,
) -> String {
    // The non-object early return.
    let Some(map) = schema.as_object() else {
        let marker = if required { " *required*" } else { "" };
        return format!("{indent}{name}{marker}");
    };
    let mut parts = vec![format!("{indent}{name}")];
    let type_str = format_type(map);
    if !type_str.is_empty() {
        parts.push(format!("({type_str})"));
    }
    if required {
        parts.push("*required*".to_string());
    }
    append_schema_annotations(&mut parts, map);

    let mut lines = vec![parts.join(" ")];
    lines.extend(format_nested_schema(map, &format!("{indent}  "), depth));
    lines.join("\n")
}

/// `formatType(schema)` — first match wins, six rules.
fn format_type(schema: &JsonMap<String, Value>) -> String {
    // 1 · `Object.hasOwn(schema, "const")` — PRESENCE, so `const: null` reads `const null`.
    if schema.contains_key("const") {
        let value = schema.get("const").unwrap_or(&Value::Null);
        return format!("const {}", serde_json::to_string(value).unwrap_or_default());
    }
    // 2 · `enum`
    if let Some(members) = schema.get("enum").and_then(Value::as_array) {
        let rendered = members
            .iter()
            .map(|member| serde_json::to_string(member).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(", ");
        return format!("enum: {rendered}");
    }
    // 3 · `Array.isArray(schema.type)`
    if let Some(list) = schema.get("type").and_then(Value::as_array) {
        return list.iter().map(js_string_of).collect::<Vec<_>>().join(" | ");
    }
    // 4 · TRUTHY `schema.type` — `""` and `0` are falsy and fall through to rule 5.
    if let Some(value) = schema.get("type").filter(|value| is_js_truthy(value)) {
        return js_string_of(value);
    }
    // 5 · an object non-array `properties`
    if schema.get("properties").is_some_and(Value::is_object) {
        return "object".to_string();
    }
    // 6 · `schema.items !== undefined`
    if schema.contains_key("items") {
        return "array".to_string();
    }
    String::new()
}

/// `appendSchemaAnnotations(parts, schema)` — description, then the eight keys IN ORDER, then
/// `default`. `!== undefined` is key presence, so an explicit `null` still renders.
fn append_schema_annotations(parts: &mut Vec<String>, schema: &JsonMap<String, Value>) {
    if let Some(description) = schema.get("description").and_then(Value::as_str) {
        parts.push(format!("- {description}"));
    }
    for key in SCHEMA_ANNOTATION_KEYS {
        if let Some(value) = schema.get(key) {
            parts.push(format!("[{key}: {}]", serde_json::to_string(value).unwrap_or_default()));
        }
    }
    if let Some(value) = schema.get("default") {
        parts.push(format!("[default: {}]", serde_json::to_string(value).unwrap_or_default()));
    }
}

/// `formatNestedSchema(schema, indent)` — `anyOf`, `oneOf`, `items`, `properties`, in that order.
///
/// The single depth choke point: both recursion paths (`format_property` and `format_variants`)
/// re-enter here.
fn format_nested_schema(
    schema: &JsonMap<String, Value>,
    indent: &str,
    depth: u32,
) -> Vec<String> {
    if depth > MAX_SCHEMA_DEPTH {
        return vec![format!("{indent}(complex schema)")];
    }
    let mut lines = Vec::new();
    if let Some(variants) = schema.get("anyOf").and_then(Value::as_array) {
        lines.extend(format_variants("anyOf", variants, indent, depth));
    }
    if let Some(variants) = schema.get("oneOf").and_then(Value::as_array) {
        lines.extend(format_variants("oneOf", variants, indent, depth));
    }
    // `items` renders as a property literally named `items`, never required.
    if let Some(items) = schema.get("items") {
        lines.push(format_property("items", items, false, indent, depth + 1));
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let required = schema_required_names(schema);
        for (name, property) in properties {
            lines.push(format_property(
                name,
                property,
                required.contains(name.as_str()),
                indent,
                depth + 1,
            ));
        }
    }
    lines
}

/// `formatVariants(keyword, variants, indent)`.
fn format_variants(
    keyword: &str,
    variants: &[Value],
    indent: &str,
    depth: u32,
) -> Vec<String> {
    let mut lines = vec![format!("{indent}{keyword}:")];
    for variant in variants {
        let Some(map) = variant.as_object() else {
            lines.push(format!(
                "{indent}  - {}",
                serde_json::to_string(variant).unwrap_or_default()
            ));
            continue;
        };
        let type_str = format_type(map);
        let label = if type_str.is_empty() { "schema".to_string() } else { type_str };
        let mut parts = vec![format!("{indent}  - {label}")];
        append_schema_annotations(&mut parts, map);
        lines.push(parts.join(" "));
        lines.extend(format_nested_schema(map, &format!("{indent}    "), depth + 1));
    }
    lines
}

/// JS `String(value)` for the shapes `type` can legally hold. A non-string is a schema bug; its
/// compact JSON is strictly more informative than JS's `"[object Object]"`, and that is the one
/// deliberate divergence in this function.
fn js_string_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// JS truthiness, for `formatType`'s rule 4.
fn is_js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn schema_required_names(map: &JsonMap<String, Value>) -> std::collections::HashSet<&str> {
    map.get("required")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}
```

### C · Wire the two through the trait stub

Change the `FakeEnv` stubs at [proxy.rs:5034-5039](../../crates/cyrup-mcp/src/proxy.rs) to delegate
to the real functions, so the existing proxy conformance tests exercise the renderers:

```rust
fn format_schema(&self, schema: &Value, indent: &str) -> String {
    super::format_schema(schema, indent)
}
fn render_ts_shape(&self, schema: &Value) -> Option<String> {
    crate::ts_shape::render_ts_shape(schema)
}
```

This changes exactly one existing assertion. `describe_forks_between_shape_and_parameters`
([proxy.rs:6370-6384](../../crates/cyrup-mcp/src/proxy.rs)) feeds `{"type": "object"}`, which the
stub rendered as the literal `"{ a: string }"`. The real renderer takes precedence rule 5 with no
`properties`, so the expectation at `:6377` becomes:

```rust
assert!(text_of(&execute_describe(&ctx, "srv_run")).ends_with("\nShape:\n{}"));
```

### D · MCP-093 — format registrations, inside MCP-092's module

Land as one function in whatever module MCP-092 creates. **Do not create a second validator
module.** The contract is the function, not the path.

```rust
use jsonschema::ValidationOptions;
use referencing::Draft;

/// `ajv-formats`' `addFormats(ajv)`, minus everything `jsonschema 0.46.9` already ships (MCP-093).
///
/// The delta is **per-draft**: `duration` and `uuid` are built in only at `draft >= 2019-09`
/// (`jsonschema-0.46.9/src/keywords/format.rs:1388`, `:1410`), and a user-registered format is
/// consulted FIRST (`:1422-1428`), so registering them on the 2020-12 builder would shadow the
/// crate's better implementation with this one.
///
/// **Four ajv formats are deliberately absent and must stay absent:**
///
/// * `float`, `double`, `password`, `binary` — ajv asserts nothing for any of them
///   (`() => true` / `/[\s\S]*/`). Registering a no-op is noise.
/// * `int32`, `int64` — these are NUMERIC formats, and `with_format` is unreachable for them:
///   `CustomFormatValidator::is_valid` (`src/keywords/format.rs:1287-1293`) returns `true` for
///   every non-string instance. A `with_format("int64", …)` here would be dead code, which is
///   exactly the silent pass this unit exists to prevent. Recovering them needs
///   `with_keyword("format", …)`, which overrides the whole `format` compile path and forfeits all
///   nineteen built-in string formats. Not worth an `i32` range check; recorded rather than done.
pub(crate) fn register_ajv_formats(
    options: ValidationOptions,
    draft: Draft,
) -> ValidationOptions {
    let options = options
        .with_format("url", |value: &str| url::Url::parse(value).is_ok())
        .with_format("byte", |value: &str| {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.decode(value).is_ok()
        })
        .with_format("iso-time", |value: &str| matches_static(&ISO_TIME, value))
        .with_format("iso-date-time", |value: &str| matches_static(&ISO_DATE_TIME, value))
        .with_format("json-pointer-uri-fragment", |value: &str| {
            matches_static(&JSON_POINTER_URI_FRAGMENT, value)
        });

    if draft >= Draft::Draft201909 {
        return options;
    }
    // draft-07 only — see the doc comment.
    options
        .with_format("duration", |value: &str| matches_static(&DURATION, value))
        .with_format("uuid", |value: &str| matches_static(&UUID, value))
}

/// A `LazyLock` regex that failed to compile matches nothing, which is the conservative direction
/// for a format assertion. Same `Regex::new(..).ok()` discipline as
/// [`crate::agent_plugin`]'s `PLUGIN_NAME_CLASS`.
fn matches_static(pattern: &LazyLock<Option<Regex>>, value: &str) -> bool {
    pattern.as_ref().is_some_and(|regex| regex.is_match(value))
}

// `ajv-formats`' own patterns. `iso-time` and `iso-date-time` differ from RFC 3339 `time` /
// `date-time` in exactly one way: the timezone is OPTIONAL.
static ISO_TIME: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)^([01]\d|2[0-3]):[0-5]\d:([0-5]\d|60)(\.\d+)?(z|[+-]([01]\d|2[0-3]):?[0-5]\d)?$").ok()
});
static ISO_DATE_TIME: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)^\d{4}-[01]\d-[0-3]\d[t ]([01]\d|2[0-3]):[0-5]\d:([0-5]\d|60)(\.\d+)?(z|[+-]([01]\d|2[0-3]):?[0-5]\d)?$").ok()
});
static JSON_POINTER_URI_FRAGMENT: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)^#(/([a-z0-9_\-.!$&'()*+,;:=@]|%[0-9a-f]{2}|~[01])*)*$").ok()
});
static DURATION: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"^P(?!$)((\d+Y)?(\d+M)?(\d+D)?(T(?=\d)(\d+H)?(\d+M)?(\d+S)?)?|\d+W)$").ok()
});
static UUID: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").ok()
});
```

Rust's `regex` has no lookahead, so the `(?!$)` in `DURATION` must be rewritten as an alternation of
the legal shapes; do that rather than reaching for a different engine. `url::Url::parse` accepts more
schemes than ajv's `url` regex — **intentional**: this validator checks a server's own
`structuredContent` against its own `outputSchema`, so a false rejection breaks a working server
while a false acceptance passes data the model reads anyway. Lenient is the safe direction, and it is
the direction every predicate above takes. Keep `should_ignore_unknown_formats` at its default
(`true`, `src/options.rs:51`) — that is ajv's `strict: false` behaviour.

Both `url` ([Cargo.toml:104](../../crates/cyrup-mcp/Cargo.toml)), `base64`
([:126](../../crates/cyrup-mcp/Cargo.toml)) and `regex` ([:107](../../crates/cyrup-mcp/Cargo.toml))
are already crate dependencies. Nothing new is added to `Cargo.toml`.

### E · MCP-085 — the sanitising tail

Add to [ui.rs](../../crates/cyrup-mcp/src/ui.rs) directly after `sanitize_display_text` (`:424`), so
the three `utils.ts` terminal functions sit together:

```rust
/// `utils.ts` `formatTerminalError(error)` (MCP-085) — the projection a USER reads.
///
/// The walk is already done. [`crate::errors::render_aggregate_texts`] is the `Display` of all
/// seven [`crate::errors::McpError`] aggregates and of
/// [`crate::server_manager::ManagerError`], and it implements the head-drop, the empty-message skip
/// and the de-duplication, measured against node 22 (table at the top of `errors.rs`). This is the
/// tail `errors.rs` records as the residual: `sanitizeTerminalText`.
///
/// **Do not add a `source()` walk here.** [`crate::errors::McpError::Io`]'s template is already
/// `{path}: {source}`, so re-walking it would print `"/p: denied: denied"`. And a cycle guard has
/// nothing to guard: `CleanupErrors(Vec<McpError>)` and
/// `ManagerError::Aggregate { children: Vec<Arc<ManagerError>> }` are both built by owning
/// constructors with no `Arc::new_cyclic` and no interior mutability, so an error cannot reach
/// itself.
///
/// **Single-line output.** [`sanitize_terminal_text`] collapses every whitespace run, newlines
/// included. That is correct for a status line and for a log record; do not route multi-line panel
/// text through here.
#[must_use]
pub fn format_terminal_error<E: std::fmt::Display + ?Sized>(error: &E) -> String {
    sanitize_terminal_text(&error.to_string())
}
```

Then delete both `TODO(MCP-235)` comments and wire the sites:

* [lifecycle.rs:1328-1330](../../crates/cyrup-mcp/src/lifecycle.rs) →
  `tracing::debug!("MCP: auth-required callback failed for {name}: {}", crate::ui::format_terminal_error(&error));`
* [lifecycle.rs:1413-1414](../../crates/cyrup-mcp/src/lifecycle.rs) →
  `tracing::error!("MCP: Failed to {target}: {}", crate::ui::format_terminal_error(error));`

Finally, replace the "**Residual:**" paragraph at
[errors.rs:58-61](../../crates/cyrup-mcp/src/errors.rs) with a pointer to
`crate::ui::format_terminal_error` — the residual is closed and the header must not keep claiming
otherwise.

### F · MCP-089 — the taxonomy methods

Add to the `impl McpError` block in [errors.rs](../../crates/cyrup-mcp/src/errors.rs) (after
`aggregate_children`, `:495`). Add `use serde_json::{Map as JsonMap, Value};` to the file's imports.
Every match is exhaustive with **no `_` arm** — `#[non_exhaustive]` does not restrict matches inside
the defining crate, and that is what stops the taxonomy rotting.

```rust
    /// `errors.ts`'s `McpUiError.code` (MCP-089) — the machine-readable class.
    ///
    /// **Not [`crate::proxy::McpErrorCode`].** That is `proxy-modes.ts`'s `details.error` value for
    /// the `mcp` gateway tool's JSON result — a model-facing vocabulary with 32 arms and a
    /// different consumer. Conflating the two is the one mistake this doc exists to prevent.
    ///
    /// Post-cut only two of upstream's eight codes survive: `MCP_SERVER_ERROR` and `wrapError`'s
    /// `UNKNOWN_ERROR`. The five MCP Apps codes went with Cut 2 and `CONSENT_DENIED` /
    /// `CONSENT_REQUIRED` went with `consent-manager.ts` (`13-cyrup-mcp.md:418-419`). The rest are
    /// cyrup-owned; the two aggregate codes mirror upstream's own
    /// `/cleanup failed|setup failed/` discriminator, which is the only place those heads are ever
    /// classified.
    ///
    /// No production consumer, deliberately: every message the proxy emits is byte-exact upstream
    /// text built from `McpErrorCode`, so wiring this into user-facing output would BREAK parity.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            McpError::Aborted(_) => "ABORTED",
            McpError::Config(_) => "CONFIG_ERROR",
            McpError::Server { .. } => "MCP_SERVER_ERROR",
            McpError::Io { .. } => "IO_ERROR",
            McpError::CredentialStore(_) => "CREDENTIAL_STORE_ERROR",
            McpError::SetupFailed(_) => "SETUP_FAILED",
            McpError::RuntimeCleanupFailed(_)
            | McpError::OAuthAggregate { .. }
            | McpError::AbortCleanupFailed(_)
            | McpError::HttpCleanupFailed(_)
            | McpError::ConnectionCleanupFailed(_)
            | McpError::ManagerCleanupFailed(_) => "CLEANUP_FAILED",
            McpError::Other(_) => "UNKNOWN_ERROR",
        }
    }

    /// `errors.ts`'s `recoveryHint`.
    ///
    /// `Option`, not `&'static str`: the field is `recoveryHint?` on `McpUiError` and `toJSON`
    /// drops it when absent, so an empty string would serialise a key upstream omits.
    /// [`McpError::Other`] is upstream's `wrapError` fallback and has no hint by construction.
    /// `Server`'s string is upstream's own, byte for byte.
    #[must_use]
    pub const fn recovery_hint(&self) -> Option<&'static str> {
        match self {
            McpError::Server { .. } => Some("Check that the MCP server is running and responsive."),
            McpError::Aborted(_) => Some("The run was cancelled. Retry the request."),
            McpError::Config(_) => Some("Check mcp.json for syntax errors, then run /reload."),
            McpError::Io { .. } => Some("Check the path exists and is readable, then run /reload."),
            McpError::CredentialStore(_) => {
                Some("The OS credential store is unavailable. Unlock the keychain and retry.")
            }
            McpError::SetupFailed(_) => Some(
                "The server failed to start and could not be cleaned up. Check the server command \
                 and retry.",
            ),
            McpError::RuntimeCleanupFailed(_)
            | McpError::OAuthAggregate { .. }
            | McpError::AbortCleanupFailed(_)
            | McpError::HttpCleanupFailed(_)
            | McpError::ConnectionCleanupFailed(_)
            | McpError::ManagerCleanupFailed(_) => Some(
                "A teardown step failed. Restart the session if MCP servers become unresponsive.",
            ),
            McpError::Other(_) => None,
        }
    }

    /// `errors.ts`'s `McpUiErrorContext` — `{ server?, tool?, … }`, absent keys OMITTED, never
    /// `null`. `uri` and `session` went with Cut 2; `path` is cyrup's, because every adapter-owned
    /// path is relocatable through `CYRUP_AGENT_DIR` and a bare "permission denied" is unactionable.
    #[must_use]
    pub fn context(&self) -> JsonMap<String, Value> {
        let mut context = JsonMap::new();
        match self {
            McpError::Server { server, .. } => {
                context.insert("server".to_string(), Value::String(server.clone()));
            }
            McpError::Io { path, .. } => {
                context.insert("path".to_string(), Value::String(path.display().to_string()));
            }
            McpError::Aborted(_)
            | McpError::Config(_)
            | McpError::CredentialStore(_)
            | McpError::RuntimeCleanupFailed(_)
            | McpError::OAuthAggregate { .. }
            | McpError::AbortCleanupFailed(_)
            | McpError::SetupFailed(_)
            | McpError::HttpCleanupFailed(_)
            | McpError::ConnectionCleanupFailed(_)
            | McpError::ManagerCleanupFailed(_)
            | McpError::Other(_) => {}
        }
        context
    }
```

Update the module header at [errors.rs:12-17](../../crates/cyrup-mcp/src/errors.rs): MCP-089 is
landed, and the header must say the taxonomy is base + `McpServerError` with `ConsentError`
explicitly cut alongside `consent-manager.ts`, so the 13b §10 row cannot lure the next reader.

### G · MCP-079 — close the two vocabularies

Add to [proxy.rs](../../crates/cyrup-mcp/src/proxy.rs): the first goes **inside** the existing
`impl ApprovalOrigin` block, after `as_str` (which closes at `:1377`; the block closes at `:1378`);
the second goes after the `ApprovalOutcome` enum (`:1388`).

```rust
    /// The total inverse of [`ApprovalOrigin::as_str`] (MCP-079).
    ///
    /// **`None` for `"script"` and `"iframe"`** — upstream's other two arms, cut with Cut 4 and
    /// Cut 2. This is the whole of the unit's "rejected rather than silently accepted" obligation.
    /// It is a `parse`, not a `Deserialize`: nothing in the port ever puts an origin ON a wire.
    /// The broker event that did is MCP-233's cut, `SharedBus` is JSON-only and deferred, and the
    /// only live use is the write-side `as_str` for `details.origin`. A serde derive would invent a
    /// wire format with no producer and then owe it stability forever.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "proxy" => Some(ApprovalOrigin::Proxy),
            "direct" => Some(ApprovalOrigin::Direct),
            "resource" => Some(ApprovalOrigin::Resource),
            _ => None,
        }
    }
```

```rust
/// `types.ts` `McpToolApprovalDecision` — what the human chose, before it collapses into an
/// [`ApprovalOutcome`] (MCP-079).
///
/// Three arms, not upstream's four. `"abstain"` is cut with the approval broker (MCP-233): the
/// replacement gate, `ExtHooks::before_tool_call`, has no abstain — a permission extension that
/// declines to decide simply does not block, which lands in the same place. See
/// [`ensure_tool_call_approved`]'s "two deltas from upstream".
///
/// The distinction this type carries that a bare label match does not: **`AllowForSession` inserts
/// the argument-keyed cache entry and `AllowOnce` does not.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolApprovalDecision {
    /// `"allow_once"` — [`APPROVE_ONCE_OPTION`].
    AllowOnce,
    /// `"allow_for_session"` — [`APPROVE_FOR_SESSION_OPTION`].
    AllowForSession,
    /// `"deny"` — [`DENY_OPTION`], and every non-answer.
    Deny,
}

impl ToolApprovalDecision {
    /// Upstream's own spellings.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ToolApprovalDecision::AllowOnce => "allow_once",
            ToolApprovalDecision::AllowForSession => "allow_for_session",
            ToolApprovalDecision::Deny => "deny",
        }
    }

    /// The total inverse of [`Self::as_str`]. `None` for `"abstain"` — see the type's docs.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allow_once" => Some(ToolApprovalDecision::AllowOnce),
            "allow_for_session" => Some(ToolApprovalDecision::AllowForSession),
            "deny" => Some(ToolApprovalDecision::Deny),
            _ => None,
        }
    }

    /// [`crate::owner`]'s `select` answer → the decision. **Fails closed**: the literal deny
    /// option, an unknown label and a dismissal (`None`) all land on [`Self::Deny`].
    #[must_use]
    pub fn from_dialog_label(label: Option<&str>) -> Self {
        match label {
            Some(APPROVE_ONCE_OPTION) => ToolApprovalDecision::AllowOnce,
            Some(APPROVE_FOR_SESSION_OPTION) => ToolApprovalDecision::AllowForSession,
            _ => ToolApprovalDecision::Deny,
        }
    }
}
```

Then replace the label match at
[proxy.rs:4842-4855](../../crates/cyrup-mcp/src/proxy.rs) — behaviour-identical, and the cache rule
becomes a property of the type rather than of a match arm:

```rust
    match ToolApprovalDecision::from_dialog_label(decision.as_deref()) {
        ToolApprovalDecision::AllowOnce => ApprovalOutcome::Approved,
        ToolApprovalDecision::AllowForSession => {
            if let Ok(mut approved) = state.approved_tool_calls.lock() {
                approved.insert(cache_key);
            }
            // Best-effort for the same reason the lookup is: a poisoned lock costs a repeat
            // prompt, never an ungated call. The approval still stands for THIS call.
            ApprovalOutcome::Approved
        }
        ToolApprovalDecision::Deny => ApprovalOutcome::Denied,
    }
```

Export `ToolApprovalDecision` alongside the other proxy items at
[lib.rs:161](../../crates/cyrup-mcp/src/lib.rs).

### H · MCP-090 — new module `crates/cyrup-mcp/src/log.rs`

Register as `pub mod log;` in [lib.rs](../../crates/cyrup-mcp/src/lib.rs)'s module block and add a
row to the map table.

```rust
//! `logger.ts` — the log channel, as `tracing` targets plus one env bootstrap (MCP-090).
//!
//! # Three deliberate rulings
//!
//! 1. **No explicit `target:` on the crate's own 65 call sites.** `tracing`'s default target IS the
//!    module path, so every site is already addressable as `cyrup_mcp`, `cyrup_mcp::oauth`,
//!    `cyrup_mcp::lifecycle`, … through `RUST_LOG`. Stamping one flat `"MCP-UI"` target across them
//!    would DESTROY that granularity, not add it. Upstream's `[MCP-UI…]` prefix is the package's
//!    historical name and carries no scope meaning (13b §11).
//! 2. **One explicit target, for the second channel.** Config-load warnings do not go through
//!    upstream's logger at all — they are bare `console.warn` in `config.ts` and
//!    `agent-plugin-loader.ts`, unfiltered diagnostics that predate the logger. `tracing` has no
//!    bypass and writing to stderr directly would corrupt the TUI, so the channels are separated by
//!    TARGET instead, spelled as a module path so it composes with `RUST_LOG` — the same shape
//!    `cyrup_ext_subagents::child_stderr` already uses. This costs nothing in the direction
//!    upstream cares about: [`UI_DEBUG_DIRECTIVE`] only ever RAISES verbosity.
//! 3. **The pluggable handler list is dropped.** No analogue, no production consumer, and the
//!    `try {} catch {}` that swallowed handler errors has nothing to swallow. Stated, not silent.

/// `logger.ts`'s module bootstrap variable. Not pi-branded, so it is preserved verbatim
/// (13b §16). MCP-068 owns the env-override family and must consume this constant rather than
/// spelling the name a second time.
pub const UI_DEBUG_ENV_VAR: &str = "MCP_UI_DEBUG";

/// The unfiltered config-load channel — `config.ts`'s and `agent-plugin-loader.ts`'s bare
/// `console.warn` sites, kept distinct from the level-gated logger channel.
pub const CONFIG_LOAD_TARGET: &str = "cyrup_mcp::config_load";

/// The `EnvFilter` directive `MCP_UI_DEBUG` adds. It is ADDITIVE — layered on top of whatever
/// `RUST_LOG` asked for, and it never lowers a floor.
pub const UI_DEBUG_DIRECTIVE: &str = "cyrup_mcp=debug";

/// `MCP_UI_DEBUG === "1" || "true"` ⇒ raise this crate's floor to `debug`.
#[must_use]
pub fn ui_debug_enabled() -> bool {
    ui_debug_from(std::env::var(UI_DEBUG_ENV_VAR).ok().as_deref())
}

/// The predicate alone, so it is testable without touching process env — edition 2024 makes
/// `std::env::set_var` `unsafe`, which is why MCP-082 splits `interpolate_env_vars` the same way.
///
/// Exactly two accepted spellings. `"0"`, `"TRUE"`, `""` and an unset variable are all `false`.
#[must_use]
pub fn ui_debug_from(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true"))
}
```

Give the eight config-load sites their target — [config.rs:1862](../../crates/cyrup-mcp/src/config.rs),
[:1876](../../crates/cyrup-mcp/src/config.rs), [:1899](../../crates/cyrup-mcp/src/config.rs),
[:1946](../../crates/cyrup-mcp/src/config.rs), [:2331](../../crates/cyrup-mcp/src/config.rs),
[agent_plugin.rs:493](../../crates/cyrup-mcp/src/agent_plugin.rs),
[:524](../../crates/cyrup-mcp/src/agent_plugin.rs),
[:1446](../../crates/cyrup-mcp/src/agent_plugin.rs):

```rust
tracing::warn!(target: crate::log::CONFIG_LOAD_TARGET, "{message}");
```

(`tracing`'s macro takes `target: $target:expr` and a `const &'static str` is const-evaluable in the
callsite `static`, so the path form compiles.)

And make `MCP_UI_DEBUG` observable, in `init_tracing`
([main.rs:2344-2352](../../crates/cyrup/src/main.rs)) — `cyrup` already depends on `cyrup-mcp`
([crates/cyrup/Cargo.toml:82](../../crates/cyrup/Cargo.toml)):

```rust
fn init_tracing(verbose: bool) {
    use tracing_subscriber::{EnvFilter, fmt};
    let default = if verbose { "debug" } else { "warn" };
    let mut filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    // `logger.ts`'s module bootstrap (MCP-090): `MCP_UI_DEBUG=1|true` raises ONLY the MCP
    // adapter's floor, layered on top of whatever `RUST_LOG` asked for. It never lowers one.
    if cyrup_mcp::log::ui_debug_enabled()
        && let Ok(directive) = cyrup_mcp::log::UI_DEBUG_DIRECTIVE.parse()
    {
        filter = filter.add_directive(directive);
    }
    let _ = fmt().with_env_filter(filter).with_writer(io::stderr).try_init();
}
```

---

## Fixtures the goldens must reproduce

These are computed against the implementations above, including the alphabetical property order of
correction 6. Write the input literals in a **different** key order from the output so the test
documents the divergence.

**`format_schema(schema, "  ")`** over
`{"target": {"anyOf":[{"type":"string"},{"type":"number","minimum":0}],"description":"where"},
"mode":{"const":null},"kind":{"enum":["a",1]},"tags":{"type":"array","items":{"type":"string","maxLength":8}}}`
wrapped in `{"type":"object","properties":{…},"required":["mode"]}`:

```text
  kind (enum: "a", 1)
  mode (const null) *required*
  tags (array)
    items (string) [maxLength: 8]
  target - where
    anyOf:
      - string
      - number [minimum: 0]
```

**`render_ts_shape`** — MCP-098's fixture,
`{"$ref":"#/$defs/A","$defs":{"A":{"$ref":"#/$defs/B"},"B":{"type":"string"}}}`:

```text
type A = B;
type B = string;

A
```

**`render_ts_shape`** — the alias-plus-object case,
`{"type":"object","properties":{"b":{"type":"string"},"a":{"$ref":"#/$defs/Thing"}},
"required":["a"],"$defs":{"Thing":{"type":"object","properties":{"n":{"type":"integer"}}}}}`:

```text
type Thing = { n?: number; };

{ a: Thing; b?: string; }
```

**`render_ts_shape`** — the parenthesisation rule,
`{"type":"array","items":{"anyOf":[{"type":"string"},{"type":"number"}]}}` ⇒
`(string | number)[]`.

**`render_ts_shape`** — the `additionalProperties` exemption:
`{"type":"object","additionalProperties":false,"properties":{}}` ⇒ `{}` (supported), while
`{"type":"object","additionalProperties":{"type":"string"}}` ⇒ `None`, and each of `if`, `then`,
`else`, `allOf`, `not`, `patternProperties` present at ANY node ⇒ `None`.

---

## Acceptance Criteria

**MCP-091 + MCP-098**

- [ ] `crates/cyrup-mcp/src/ts_shape.rs` exists, is declared `pub mod ts_shape;` in `lib.rs`, and has a row in `lib.rs`'s module-map table.
- [ ] `render_ts_shape` reproduces all four fixtures above byte for byte, including `type A = B;` **and** `type B = string;` for the MCP-098 fixture where `B` is referenced only from inside `A`.
- [ ] The emission loop is an index-based `while index < aliases.map.len() { … index += 1 }` over an `IndexMap` grown by `render` inside the loop. No `for (k, v) in &aliases`, no pre-collected `Vec`, no `BTreeMap`.
- [ ] Each of `if`, `then`, `else`, `allOf`, `not`, `patternProperties` returns `None` when present at a nested node, not only at the root; `additionalProperties: false` returns a shape and `additionalProperties: {…}` returns `None`.
- [ ] A schema nested past `MAX_RENDER_DEPTH` returns `None` rather than exhausting the stack.
- [ ] `$ref` resolution decodes `~1`→`/` before `~0`→`~` on the ref token, with `$defs` keys stored raw; a `$ref` that does not resolve returns `None`.
- [ ] `const: null` renders `null` (key presence, via `contains_key`), distinct from an absent `const`.

**MCP-211**

- [ ] `format_schema` and the four helpers live in `proxy.rs` section 2, immediately after `find_tool_by_name`, and reproduce the `formatSchema` fixture above byte for byte.
- [ ] `formatType` rule 1 uses `JsonMap::contains_key("const")`, so `{"const": null}` renders `(const null)`.
- [ ] `{"type": ""}` and `{"type": 0}` fall through rule 4 to rule 5/6 rather than rendering an empty `()` part.
- [ ] `appendSchemaAnnotations` emits the eight keys in the order `minLength, maxLength, minimum, maximum, minItems, maxItems, format, pattern`, then `default`.
- [ ] `formatProperty` joins its parts with exactly one space and recurses at `indent + "  "`; `formatVariants` nests at `indent + "    "`.
- [ ] A non-object property schema takes the early return and emits the single `<indent><name>[ *required*]` line with no `(type)` part.

**Wiring (MCP-091/098/211)**

- [ ] `FakeEnv::format_schema` and `FakeEnv::render_ts_shape` at `proxy.rs:5034/:5037` delegate to the real functions; the constant stubs are gone.
- [ ] `describe_forks_between_shape_and_parameters` asserts `ends_with("\nShape:\n{}")`, and the `Parameters:` fork at `proxy.rs:2246` is still reached for a schema `render_ts_shape` declines.

**MCP-093**

- [ ] `register_ajv_formats(options, draft)` lives in MCP-092's module. No second validator module exists anywhere in the crate.
- [ ] `url`, `byte`, `iso-time`, `iso-date-time` and `json-pointer-uri-fragment` are registered on both builders; `duration` and `uuid` are registered on the **draft-07 builder only** and shadow nothing on 2020-12.
- [ ] `float`, `double`, `password` and `binary` are not registered, and the module doc states that ajv asserts nothing for any of them.
- [ ] A test proves the numeric mechanism is inert — a deliberately-false `with_format("int64", |_| false)` does **not** reject a numeric instance — so the absence of `int32`/`int64` cannot be "fixed" back into dead code.
- [ ] One accept/reject pair per registered format, and a test enumerating `jsonschema`'s built-ins per draft so a version bump that adds `url` or `iso-time` fails loudly instead of silently double-registering.
- [ ] `should_ignore_unknown_formats` is left at its default.

**MCP-085**

- [ ] `crate::ui::format_terminal_error` exists beside `sanitize_terminal_text`, applies `sanitize_terminal_text` to the `Display` rendering, and adds no `source()` walk and no cycle guard.
- [ ] Both `TODO(MCP-235)` comments at `lifecycle.rs:1328` and `:1413` are deleted and both sites route their error text through it.
- [ ] An unterminated `ESC ]` payload, a C1 `0x9d` introducer and an OSC 8 hyperlink are all absent from what those two sites emit.
- [ ] The gateway's model-facing `Failed to connect to "{server}": {message}` at `proxy.rs:2847` and `:3486` is **unchanged** — upstream does not sanitise that path.
- [ ] The "**Residual:**" paragraph at `errors.rs:58-61` is replaced by a pointer to the new function.

**MCP-089**

- [ ] `McpError::code()`, `recovery_hint()` and `context()` exist, each an exhaustive match with **no `_` arm**.
- [ ] `McpError::Server`'s `#[error]` template is still `"{server}: {message}"`; `code()` returns `"MCP_SERVER_ERROR"` and `recovery_hint()` returns upstream's `"Check that the MCP server is running and responsive."`.
- [ ] No `ConsentError` variant, no `CONSENT_DENIED`, no `CONSENT_REQUIRED` anywhere in the crate.
- [ ] `context()` omits absent keys entirely rather than emitting `null`.
- [ ] `recovery_hint()` returns `None` for `McpError::Other` and `Some` for every other variant.
- [ ] The `errors.rs` module header no longer lists MCP-089 as pending and records that `ConsentError` is cut with `consent-manager.ts`.
- [ ] `code()`'s doc names `crate::proxy::McpErrorCode` as the other, different vocabulary.

**MCP-079**

- [ ] `ApprovalOrigin::parse` exists and returns `None` for `"script"`, `"iframe"` and any other string; `parse(as_str(x)) == Some(x)` for all three arms.
- [ ] `ToolApprovalDecision { AllowOnce, AllowForSession, Deny }` exists with `as_str`, a `parse` returning `None` for `"abstain"`, and `from_dialog_label`.
- [ ] `ensure_tool_call_approved`'s tail dispatches on `ToolApprovalDecision` instead of matching raw labels, with identical behaviour: `AllowOnce` approves without caching, `AllowForSession` approves and inserts the argument-keyed entry, everything else denies. The existing tests `allow_for_session_caches_per_argument_payload` and `allow_once_approves_without_caching` still pass unmodified.
- [ ] No serde derive is added to `ApprovalOrigin` or `ToolApprovalDecision`, and the reason is recorded on `ApprovalOrigin::parse`.
- [ ] `ToolApprovalDecision` is re-exported from `lib.rs`.

**MCP-090**

- [ ] `crates/cyrup-mcp/src/log.rs` exists, is declared in `lib.rs`, and defines `UI_DEBUG_ENV_VAR`, `CONFIG_LOAD_TARGET`, `UI_DEBUG_DIRECTIVE`, `ui_debug_enabled` and `ui_debug_from`.
- [ ] `ui_debug_from` is `true` for exactly `Some("1")` and `Some("true")` and `false` for `None`, `Some("0")`, `Some("TRUE")` and `Some("")`.
- [ ] All eight config-load warn sites carry `target: crate::log::CONFIG_LOAD_TARGET`; the crate's other 57 `tracing::*!` sites carry **no** explicit target.
- [ ] `init_tracing` in `crates/cyrup/src/main.rs` adds `UI_DEBUG_DIRECTIVE` when `ui_debug_enabled()`, additively on top of `RUST_LOG`, and never lowers a floor.
- [ ] The module doc states that the pluggable handler list is dropped, and why the two channels are separated by target rather than by bypassing the filter.
