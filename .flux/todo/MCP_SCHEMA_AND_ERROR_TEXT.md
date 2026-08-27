---
stage: aug
status: done
updated: 2026-08-27 06:00
---

# MCP: schema rendering, error taxonomy and the log channel

## Objective

Eight `cyrup-mcp` port units that share one property: **they are pure, state-free vocabularies and
formatters.** No connection, no session, no seam with another crate. Each takes a value — a JSON
Schema, an error, a log site — and produces a string, a code or a target.

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

Upstream is pinned at **`pi-mcp-adapter` v2.26.1 (`fafae21`)**, checked out at
[`tmp/pi-mcp-adapter`](../../tmp/pi-mcp-adapter). Every citation below is `file:line` at that tag and
was read off disk during this augmentation pass.

---

## 0 · Verification pass — what this task got wrong

Everything in this section was re-derived from the tree as it stands today. **Read it before writing
any code; the previous revision's file:line citations are all stale.**

### 0.1 · `proxy.rs` no longer exists. Every `proxy.rs:NNNN` citation is dead.

The 7,594-line `proxy.rs` was split into the directory module
[`crates/cyrup-mcp/src/proxy/`](../../crates/cyrup-mcp/src/proxy) (14 files).
[`proxy/mod.rs`](../../crates/cyrup-mcp/src/proxy/mod.rs) glob re-exports every submodule, so
`crate::proxy::X` paths still resolve — but every line citation in the previous revision points at a
file that is gone. The corrected map for everything this task touches:

| what | old citation | **actual location today** |
|---|---|---|
| `ProxyEnv::format_schema` / `render_ts_shape` declarations | `proxy.rs:1554/:1556` | [proxy/env.rs:360-365](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `execute_describe`'s shape/parameters fork | `proxy.rs:2244-2247` | [proxy/discovery.rs:356-364](../../crates/cyrup-mcp/src/proxy/discovery.rs) |
| `execute_search`'s fork (indent `"    "`) | `proxy.rs:2464-2467` | [proxy/discovery.rs:576-591](../../crates/cyrup-mcp/src/proxy/discovery.rs) |
| the `Expected parameters:` suffix | `proxy.rs:3574` | [proxy/call.rs:701-705](../../crates/cyrup-mcp/src/proxy/call.rs) |
| `FakeEnv`'s two stubs | `proxy.rs:5034/:5037` | [proxy/testsupport.rs:193-198](../../crates/cyrup-mcp/src/proxy/testsupport.rs) |
| `find_tool_by_name` | `proxy.rs:758-764` | [proxy/tool_metadata.rs:404-413](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) |
| `truncate_at_word` de-duplication note | `proxy.rs:766-770` | [proxy/tool_metadata.rs:415-420](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) |
| `ApprovalOrigin` + its three methods | `proxy.rs:1332-1377` | [proxy/env.rs:132-187](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `ApprovalOutcome` | `proxy.rs:1380-1388` | [proxy/env.rs:189-198](../../crates/cyrup-mcp/src/proxy/env.rs) |
| the dialog-label match | `proxy.rs:4842-4855` | [proxy/approval.rs:327-340](../../crates/cyrup-mcp/src/proxy/approval.rs) |
| the MCP-233 / `abstain` rationale | `proxy.rs:4768-4774` | [proxy/approval.rs:251-259](../../crates/cyrup-mcp/src/proxy/approval.rs) |
| `APPROVE_ONCE_OPTION` / `APPROVE_FOR_SESSION_OPTION` / `DENY_OPTION` | `proxy.rs:119/:122` | [proxy/constants.rs:39/:42/:47](../../crates/cyrup-mcp/src/proxy/constants.rs) |
| `McpErrorCode` (32 arms) | `proxy.rs:203-271` | [proxy/error_vocab.rs:30](../../crates/cyrup-mcp/src/proxy/error_vocab.rs) |
| `disabled_call_result` | `proxy.rs:3025-3039` | [proxy/call.rs:155](../../crates/cyrup-mcp/src/proxy/call.rs) |
| `Failed to connect to "{server}": {message}` | `proxy.rs:2847/:3486` | [proxy/auth.rs:355](../../crates/cyrup-mcp/src/proxy/auth.rs), [proxy/call.rs:624](../../crates/cyrup-mcp/src/proxy/call.rs) |
| `ToolMetadata::input_schema` | `proxy.rs:391` | [proxy/tool_metadata.rs:55](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) |
| the crate-level `#![deny(...)]` | `lib.rs:118-124` | [lib.rs:124-130](../../crates/cyrup-mcp/src/lib.rs) |
| the `pub mod` block | `lib.rs:126-146` | [lib.rs:132-151](../../crates/cyrup-mcp/src/lib.rs) |
| the module-map tables | `lib.rs:88-102` | [lib.rs:90-104](../../crates/cyrup-mcp/src/lib.rs) (13a) and [:109-115](../../crates/cyrup-mcp/src/lib.rs) (Cut 2) |
| the proxy re-export line | `lib.rs:161` | [lib.rs:181](../../crates/cyrup-mcp/src/lib.rs) |
| `init_tracing` | `crates/cyrup/src/main.rs:2344-2352` | **[crates/cyrup/src/bootstrap.rs:279-289](../../crates/cyrup/src/bootstrap.rs)** (`main.rs:172` only calls it) |
| `aggregate_children` (insertion anchor) | `errors.rs:495` | [errors.rs:336-349](../../crates/cyrup-mcp/src/errors.rs); `impl McpError` closes at [:454](../../crates/cyrup-mcp/src/errors.rs) |
| `jsonschema` crate edge | `cyrup-mcp/Cargo.toml:118` | [cyrup-mcp/Cargo.toml:125-131](../../crates/cyrup-mcp/Cargo.toml) |
| `jsonschema` workspace pin | `Cargo.toml:176` | [Cargo.toml:189](../../Cargo.toml) |
| `url` / `regex` / `base64` crate edges | `:104` / `:107` / `:126` | [:115](../../crates/cyrup-mcp/Cargo.toml) / [:120](../../crates/cyrup-mcp/Cargo.toml) / [:138](../../crates/cyrup-mcp/Cargo.toml) |
| `serde_json` workspace decl | `Cargo.toml:133` | [Cargo.toml:146](../../Cargo.toml) |

Unchanged and re-verified: [ui.rs:309](../../crates/cyrup-mcp/src/ui.rs) `strip_osc_sequences`,
[ui.rs:376](../../crates/cyrup-mcp/src/ui.rs) `sanitize_terminal_text`,
[ui.rs:422-424](../../crates/cyrup-mcp/src/ui.rs) `sanitize_display_text`,
[lifecycle.rs:1328](../../crates/cyrup-mcp/src/lifecycle.rs) and
[:1413](../../crates/cyrup-mcp/src/lifecycle.rs) `TODO(MCP-235)`, all eight config-load `warn` sites,
[registration.rs:571](../../crates/cyrup-mcp/src/registration.rs) `truncate_at_word`,
[agent_plugin.rs:146-149](../../crates/cyrup-mcp/src/agent_plugin.rs) the `LazyLock<Option<Regex>>`
house pattern.

### 0.2 · Every "unmet" claim re-checked. All eight units are still open.

* `render_ts_shape` / `ts_shape` — five hits, all plumbing (trait decl, two call sites, one stub).
  No implementation, no helper, nowhere in the workspace.
* `format_schema` — trait decl, three call sites, one stub. `format_property`, `format_variants`,
  `format_nested_schema`, `format_type`, `append_schema_annotations`: **zero hits** in `crates/`.
* `format_terminal_error` — one hit, and it is a *test name*
  ([errors.rs:571](../../crates/cyrup-mcp/src/errors.rs)). No function.
* `McpError::code` / `recovery_hint` / `context` — no hits in `cyrup-mcp` beyond the promise in the
  module header at [errors.rs:13-17](../../crates/cyrup-mcp/src/errors.rs).
* `jsonschema` — **zero** `.rs` references in `crates/cyrup-mcp/`. The only workspace user is
  [cyrup-ext-subagents/src/exec/structured.rs:72](../../crates/cyrup-ext-subagents/src/exec/structured.rs).
* `MCP_UI_DEBUG` — **zero** hits in `crates/`.
* `ToolApprovalDecision` — does not exist; the decision is still a bare label match at
  [proxy/approval.rs:327-340](../../crates/cyrup-mcp/src/proxy/approval.rs).
* Only one type implements `ProxyEnv` in the whole workspace: `FakeEnv` at
  [proxy/testsupport.rs:91](../../crates/cyrup-mcp/src/proxy/testsupport.rs).

### 0.3 · FALSE PREMISE — two existing tests depend on the `render_ts_shape` stub, not one.

The previous revision named only `describe_forks_between_shape_and_parameters`. There is a second:

* [proxy/discovery.rs:907](../../crates/cyrup-mcp/src/proxy/discovery.rs) —
  `assert!(… .ends_with("\nShape:\n{ a: string }"))`
* [proxy/discovery.rs:1047](../../crates/cyrup-mcp/src/proxy/discovery.rs) —
  `assert!(text.contains("srv_run\n  Run it\n\n  Shape:\n    { a: string }"), "{text}")` in
  `search_with_schemas_indents_the_shape_block_by_four`

Both feed `{"type": "object"}`. Under the real renderer that is precedence rule 5 with no
`properties`, so both expectations become `{}`. Nothing asserts the `format_schema` stub's
`"(schema)"` literal — a grep for `Parameters:` / `Expected parameters` over `crates/` returns only
production sites.

### 0.4 · FALSE PREMISE — `ApprovalOrigin::as_str` has no production caller and no `details.origin` key exists.

The previous revision said the origin's "only use is the write-side `as_str` for a `details.origin`
key". A grep for `"origin"` over `crates/cyrup-mcp/src/proxy/` returns **nothing**, and `as_str`'s
only caller in the entire workspace is the assertion at
[proxy/approval.rs:1041](../../crates/cyrup-mcp/src/proxy/approval.rs). `origin` reaches
`ensure_tool_call_approved` and is immediately discarded by `let _ = origin;` at
[proxy/approval.rs:286](../../crates/cyrup-mcp/src/proxy/approval.rs).

This makes the *ruling* stronger, not weaker: `ApprovalOrigin` is preserved vocabulary with zero
readers, exactly like MCP-089's taxonomy methods. **Adding `Deserialize` would invent a wire format
with no producer AND no consumer.** Keep the ruling; correct the justification.

### 0.5 · FALSE PREMISE — the spec's `decodePointerToken`-at-collection is faithful to upstream.

The previous revision said §12 step 1 "cannot be right". It is exactly right:
[ts-shape.ts:17](../../tmp/pi-mcp-adapter/ts-shape.ts) is
``definitions.set(`${key}/${decodePointerToken(name)}`, definition)`` and
[:45](../../tmp/pi-mcp-adapter/ts-shape.ts) decodes again on the `$ref` side. Upstream decodes on
**both** sides.

The ruling still stands, but as a **deliberate divergence with a correctness argument**, not as a
spec correction. Under RFC 6901 a `$defs` object's keys are literal names and only the pointer token
is escaped, so upstream resolves `#/$defs/a~1b` to a member literally named `a~1b` (wrong) and fails
to resolve `#/$defs/a~01b` to it (also wrong). Storing keys raw and decoding only the token is
RFC-correct, and its failure mode is `None` — which §12 licenses as always-safe — where upstream's
failure mode is silently aliasing a *different* definition. The two agree on every schema in which no
`$defs` member name contains `~`, i.e. all of them. **Record it as a divergence in the module doc.**

### 0.6 · CONFIRMED — the `ConsentError` and `McpServerError` rulings hold.

Both re-verified against [errors.ts](../../tmp/pi-mcp-adapter/errors.ts):

* `ConsentError` is [errors.ts:116-137](../../tmp/pi-mcp-adapter/errors.ts) with codes
  `CONSENT_DENIED` / `CONSENT_REQUIRED` at [:128](../../tmp/pi-mcp-adapter/errors.ts). It goes with
  `consent-manager.ts` under Cut 2 ([13-cyrup-mcp.md:418-419](../../docs/gap-analysis/13-cyrup-mcp.md)),
  and [errors.rs:15-17](../../crates/cyrup-mcp/src/errors.rs) already records the surviving taxonomy
  as "the base shape plus `McpServerError`". **Ship no `ConsentError`, no `CONSENT_DENIED`, no
  `CONSENT_REQUIRED`.** 13b §10's `ports` row is stale; the master file and the code overrule it.
* `McpServerError`'s upstream template is `` MCP server "${server}" error: ${reason} ``
  ([errors.ts:184](../../tmp/pi-mcp-adapter/errors.ts)) with **zero upstream production call sites**.
  `McpError::Server` renders `{server}: {message}`
  ([errors.rs:183](../../crates/cyrup-mcp/src/errors.rs)) and its `message` is already the complete
  user-facing text at every one of its construction sites. **Keep the template.** Take only the code
  (`"MCP_SERVER_ERROR"`, [errors.ts:185](../../tmp/pi-mcp-adapter/errors.ts)) and the recovery hint
  (`"Check that the MCP server is running and responsive."`,
  [errors.ts:187](../../tmp/pi-mcp-adapter/errors.ts)).

### 0.7 · FALSE PREMISE — the `ajv-formats` table was wrong in five places.

Read off [node_modules/ajv-formats/dist/formats.js](../../tmp/pi-mcp-adapter/node_modules/ajv-formats/dist/formats.js)
(v3.0.1) and [dist/index.js](../../tmp/pi-mcp-adapter/node_modules/ajv-formats/dist/index.js):

1. **`addFormats(ajv)` installs `fullFormats`, not `fastFormats`** (`index.js:13`:
   `opts.mode === "fast" ? fastFormats : fullFormats`, and `addFormats` is called with no `opts`).
   So `iso-time` and `iso-date-time` are the leap-second-aware **functions** `getTime()` /
   `getDateTime()` (`formats.js:106-153`), *not* the `fastFormats` regexes. The previous revision
   prescribed the fast regexes.
2. **`password` and `binary` are literally `true`** (`formats.js:59`, `:61`), not `/[\s\S]*/`. Same
   conclusion — no-op — but state it correctly.
3. **`duration`'s last alternative is `(\d+W)?`, optional** — `formats.js:16`:
   `/^P(?!$)((\d+Y)?(\d+M)?(\d+D)?(T(?=\d)(\d+H)?(\d+M)?(\d+S)?)?|(\d+W)?)$/`. The previous revision
   wrote `|\d+W)$` and dropped the second lookahead `(T(?=\d)` entirely.
4. **`uuid` accepts an optional `urn:uuid:` prefix** — `formats.js:41`:
   `/^(?:urn:uuid:)?[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i`. The previous revision omitted it.
5. **`byte` is `/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/gm`**
   (`formats.js:179-183`) — the `m` flag makes `^`/`$` *line* anchors, so ajv accepts PEM-wrapped
   base64 when **any one line** conforms. `base64::STANDARD.decode` (the previous prescription)
   rejects embedded newlines and so is strictly stricter.

Re-verified and unchanged: `float`/`double` are `validateNumber()` = `return true`
(`formats.js:193-195`); `int64` is `Number.isInteger(value)` (`:189-192`); `int32` adds the
`±2^31` range (`:184-188`); `json-pointer-uri-fragment` is `formats.js:44`.

### 0.8 · `jsonschema 0.46.9` claims re-verified, and one API correction.

Read from `/root/.cargo/registry/src/index.crates.io-*/jsonschema-0.46.9/`:

* `CustomFormatValidator::is_valid` returns `true` for every non-string instance —
  `src/keywords/format.rs:1287-1293`. Confirmed: `with_format` is unreachable for numeric formats.
* `with_format<N: Into<String>, F: Fn(&str) -> bool + Send + Sync + 'static>` —
  `src/options.rs:323-330`.
* A user-registered format is consulted **before** the built-ins — `src/keywords/format.rs:1420-1431`
  (`ctx.get_format(format)` precedes `builtin_format(draft, format)`), and the whole block is gated
  on `ctx.validates_formats_by_default()` at `:1421`. **Without
  `should_validate_formats(true)` every format — built-in and registered — is an inert annotation.**
* Draft gating — `src/keywords/format.rs:1384-1412`: `duration` and `uuid` require
  `draft >= Draft::Draft201909` (`:1388`, `:1409`); `json-pointer` / `uri-reference` /
  `uri-template` require `>= Draft6`; `idn-hostname` / `iri` / `iri-reference` /
  `relative-json-pointer` require `>= Draft7`; `date`, `date-time`, `email`, `hostname`,
  `idn-email`, `ipv4`, `ipv6`, `regex`, `time`, `uri` are unconditional.
* `should_ignore_unknown_formats` default is `true` — `src/options.rs:51`, `:72`. Leave it; that is
  ajv's `strict: false` behaviour.
* **API correction:** `Draft` is re-exported by `jsonschema` itself —
  `jsonschema/src/lib.rs:899-901` (`pub use referencing::{uri, Draft, …}`). The previous revision's
  `use referencing::Draft;` would require adding a dependency that is not in
  [cyrup-mcp/Cargo.toml](../../crates/cyrup-mcp/Cargo.toml). Use
  `use jsonschema::{Draft, ValidationOptions};`.
* `Draft` derives `PartialOrd, Ord` (`referencing-0.46.9/src/draft.rs:13`) with variant order
  `Draft4 < Draft6 < Draft7 < Draft201909 < Draft202012 < Unknown`. Note `Unknown` sorts **above**
  `Draft201909`; the function below is only ever handed the two concrete drafts its own builders set,
  so this cannot bite, but do not expose a `>= Draft201909` test for arbitrary input.

### 0.9 · MCP-092 is still missing, and MCP-093 cannot dangle.

[13-cyrup-mcp-STATUS.md:595](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) still says `missing`, and
[MCP_HIGH_SEVERITY_BACKLOG.md:333](MCP_HIGH_SEVERITY_BACKLOG.md) still holds it as unstarted Wave 4.
There is no validator module to add to.

**Ruling: this task creates the one module MCP-092 will own** —
`crates/cyrup-mcp/src/schema_validator.rs` — containing the two `ValidationOptions` builders and the
format registration. MCP-092's owner then adds `schema_dialect`, the memoised `get_validator` and the
`Unsupported JSON Schema dialect: {uri}` error **to that same file**. That honours "do not create a
second validator module" literally — there is exactly one, and it is the one MCP-092 extends — and it
is the only way `register_ajv_formats` is anything but dead code the day it lands. Do not ship a bare
registration function with no builder to attach it to.

### 0.10 · Property order is lexicographic, not document order. Confirmed; do not chase it.

`grep -rn preserve_order` over [Cargo.toml](../../Cargo.toml) and every `crates/*/Cargo.toml`:
**zero hits**. `serde_json` is declared `{ version = "1" }` at
[Cargo.toml:146](../../Cargo.toml), so `serde_json::Map` is a `BTreeMap` and object keys iterate in
lexicographic byte order where upstream's `Object.entries` iterates document order.

Not fixable at this seam: the schema arrives as `ToolMetadata::input_schema: Option<Value>`
([proxy/tool_metadata.rs:55](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs)), already
deserialised. It is deterministic, and neither output is a prompt-cache key (that is MCP-213's
`buildProxyDescription`, a different string). **Do not enable `preserve_order`** — it is a
workspace-wide feature flip and the ordered-reader question for `mcpServers` is owned by
MCP-212/MCP-094. Record the divergence in both module docs.

---

## 1 · MCP-091 + MCP-098 — `render_ts_shape`

Upstream: [ts-shape.ts](../../tmp/pi-mcp-adapter/ts-shape.ts), 149 lines, whole file.
Spec: [13b-mcp-config.md §12, :649-694](../../docs/gap-analysis/13b-mcp-config.md).

**These two cannot be split.** MCP-098 is a constraint on MCP-091's alias-emission loop, and getting
it wrong does not yield `None` — it yields a *wrong string* naming an undefined alias, so the
caller's raw-schema fallback at
[proxy/discovery.rs:359](../../crates/cyrup-mcp/src/proxy/discovery.rs) and
[:578](../../crates/cyrup-mcp/src/proxy/discovery.rs) (which forks on `None` only) never runs and the
model is shown broken TypeScript.

### 1.1 · Five upstream behaviours the prose flattens

Read straight off the source; each is a place a plausible Rust port silently diverges.

1. **`anyOf` wins over `oneOf` by presence, not by being an array.**
   [ts-shape.ts:56-57](../../tmp/pi-mcp-adapter/ts-shape.ts) guards on
   `Array.isArray(schema.anyOf) || Array.isArray(schema.oneOf)` and then selects
   `(schema.anyOf ?? schema.oneOf)`. A schema with a non-array `anyOf` *and* an array `oneOf` enters
   the branch, selects the non-array `anyOf`, and throws on `.map` — caught by the outer `try`, so
   `null`. `map.get("anyOf").or_else(|| map.get("oneOf")).and_then(Value::as_array)` gets this wrong:
   it falls through to the next precedence rule instead of returning `None`.
2. **A present-but-non-object `properties` is `null`, not `{}`.**
   [:63-65](../../tmp/pi-mcp-adapter/ts-shape.ts): the branch is entered on
   `schema.properties !== undefined`, `"{}"` is returned only when it is `undefined`, and
   `!isSchema(schema.properties)` then returns `null`. So `{"type":"object","properties":null}` is
   `None` — `properties: null` is *present*.
3. **`renderLiteral` uses `String(value)` for numbers, `JSON.stringify` for the rest**
   ([:138-141](../../tmp/pi-mcp-adapter/ts-shape.ts)). JS has one numeric type: `String(1.0)` is
   `"1"`, while `serde_json` preserves the `1.0` spelling it parsed. Fold integral floats.
4. **`aliasFor` slices at the FIRST `/`** ([:27](../../tmp/pi-mcp-adapter/ts-shape.ts)):
   `definitionKey.slice(definitionKey.indexOf("/") + 1)`. A decoded name containing `/` keeps its
   slashes in the bare-name candidate, which then fails the identifier test and falls to
   `Definition${n}`. `split_once('/')` is the exact equivalent.
5. **The array branch precedes the `type`-array branch** ([:78](../../tmp/pi-mcp-adapter/ts-shape.ts)
   before [:84](../../tmp/pi-mcp-adapter/ts-shape.ts)), and `needsParentheses` is a plain
   `includes(" | ")` ([:147-149](../../tmp/pi-mcp-adapter/ts-shape.ts)) — a substring test on the
   *rendered* item, not a structural one.

**The one obligation the spec does not name.** §12 wraps the body in `try {} catch { return null }`
([:7](../../tmp/pi-mcp-adapter/ts-shape.ts), [:104-106](../../tmp/pi-mcp-adapter/ts-shape.ts)). In
Rust every documented failure is already an `Option`, so the catch has exactly one residual meaning:
**stack exhaustion on a deeply nested schema**. JS throws `RangeError` and returns `null`; Rust
aborts the process, and neither `#![forbid(unsafe_code)]` nor `#![deny(clippy::panic)]`
([lib.rs:124-130](../../crates/cyrup-mcp/src/lib.rs)) helps. A depth cap returning `None` is the
faithful port, and it is the discipline [errors.rs:381](../../crates/cyrup-mcp/src/errors.rs)
(`budget = 1024`), [errors.rs:443](../../crates/cyrup-mcp/src/errors.rs) (`0..32`) and
[server_manager.rs:226](../../crates/cyrup-mcp/src/server_manager.rs) already apply.

### 1.2 · New module `crates/cyrup-mcp/src/ts_shape.rs`

Register as `pub mod ts_shape;` in the alphabetical block at
[lib.rs:132-151](../../crates/cyrup-mcp/src/lib.rs), between `state` and `ui`. Add a row to the Cut 2
module-map table at [lib.rs:109-115](../../crates/cyrup-mcp/src/lib.rs):

```text
//! | [`ts_shape`] | `ts-shape.ts` | JSON Schema → a TypeScript type literal, for the model (13b §12) |
```

`indexmap` is already a crate dependency ([Cargo.toml:145](../../crates/cyrup-mcp/Cargo.toml)).
`clippy::unwrap_used`, `expect_used`, `panic` and `indexing_slicing` are `deny` crate-wide, so use
`.get()`, `.get_index()` and `unwrap_or_*` throughout.

```rust
//! `ts-shape.ts` — the useful JSON Schema subset as a TypeScript type literal (MCP-091, MCP-098).
//!
//! `None` is upstream's `null` and means **"fall back to the raw schema"**: both callers
//! ([`crate::proxy::execute_describe`] and [`crate::proxy::execute_search`]) have a fallback beside
//! the call, so returning `None` more often is a verbosity regression, never a correctness one.
//! Returning a *wrong string* is caught nowhere — which is the whole of MCP-098.
//!
//! # Two deliberate divergences
//!
//! 1. **Pointer tokens are decoded on the `$ref` side only.** Upstream decodes on both
//!    (`ts-shape.ts:17` and `:45`). Under RFC 6901 a `$defs` object's keys are literal names and
//!    only the pointer token is escaped, so upstream resolves `#/$defs/a~1b` to a member literally
//!    named `a~1b` and fails to resolve `#/$defs/a~01b` to it — both backwards. Decoding only the
//!    token is RFC-correct, and where the two disagree this one returns `None` (the always-safe
//!    fallback) while upstream aliases a *different* definition. They agree on every schema with no
//!    `~` in a `$defs` member name.
//! 2. **Property order is lexicographic.** `serde_json` is declared without `preserve_order`, so
//!    `properties` renders in key order where upstream renders document order. The schema arrives
//!    already deserialised, so document order is unrecoverable here. Deterministic, and this is not
//!    a prompt-cache key.

use std::collections::HashSet;

use indexmap::IndexMap;
use serde_json::{Map as JsonMap, Number, Value};

/// `UNSUPPORTED_KEYWORDS` (`ts-shape.ts:3`) — re-tested at EVERY node, not only the root.
const UNSUPPORTED_KEYWORDS: [&str; 7] =
    ["if", "then", "else", "allOf", "not", "patternProperties", "additionalProperties"];

/// The Rust half of upstream's `try {} catch { return null }` (`ts-shape.ts:7`, `:104`).
///
/// Every documented failure below is already an `Option`, so the catch has exactly one residual
/// meaning in Rust: stack exhaustion on a pathological schema, which JS turns into a `RangeError`
/// and this turns into `None`. Same budget discipline as
/// [`crate::errors::McpError::is_cleanup_failure`].
const MAX_RENDER_DEPTH: u32 = 64;

/// `renderTsShape(schema)` (`ts-shape.ts:6-107`) — the whole of `ts-shape.ts`.
#[must_use]
pub fn render_ts_shape(schema: &Value) -> Option<String> {
    // `if (!isSchema(inputSchema)) return null` — object, not null, not an array.
    let root = schema.as_object()?;

    let mut defs: IndexMap<String, &Value> = IndexMap::new();
    collect_definitions(root, "$defs", &mut defs)?;
    collect_definitions(root, "definitions", &mut defs)?;

    let mut aliases = Aliases::default();
    // `const root = render(inputSchema)` — the root renders BEFORE the alias loop, which is what
    // seeds `aliases` with the first entries the loop then walks.
    let rendered_root = render(schema, &defs, &mut aliases, 0)?;

    // MCP-098 — the re-entrant emission loop (`ts-shape.ts:96-102`). `render` below can INSERT into
    // `aliases.map`, and a `$ref` registered inside a `$defs` member must itself be visited and
    // emitted. JS `Map` iterators are live; `for (k, v) in &aliases` and any pre-collected snapshot
    // are BOTH wrong, and both fail SILENTLY by emitting a shape that names an undefined alias.
    let mut lines: Vec<String> = Vec::new();
    let mut index = 0usize;
    while index < aliases.map.len() {
        let Some((key, alias)) = aliases.map.get_index(index).map(|(k, a)| (k.clone(), a.clone()))
        else {
            break;
        };
        // `const definition = definitions.get(key); if (!definition) return null;`
        let definition = *defs.get(&key)?;
        let body = render(definition, &defs, &mut aliases, 0)?;
        lines.push(format!("type {alias} = {body};"));
        index = index.saturating_add(1);
    }

    if lines.is_empty() {
        return Some(rendered_root);
    }
    Some(format!("{}\n\n{rendered_root}", lines.join("\n")))
}

/// §12 step 1 (`ts-shape.ts:11-19`). A non-object group, or a non-object member, aborts the whole
/// render. An **absent** group is skipped; a group present as `null` is a non-object and aborts.
///
/// Keys are stored RAW — see the module doc's divergence 1.
fn collect_definitions<'a>(
    root: &'a JsonMap<String, Value>,
    group: &str,
    out: &mut IndexMap<String, &'a Value>,
) -> Option<()> {
    // `if (rawDefinitions === undefined) continue;`
    let Some(raw) = root.get(group) else { return Some(()) };
    // `if (!isSchema(rawDefinitions)) return null;`
    let members = raw.as_object()?;
    for (name, member) in members {
        // `if (!isSchema(definition)) return null;`
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
    /// `usedAliases` — every alias handed out, so `alias_for` never collides.
    used: HashSet<String>,
    /// `aliasIndex` — the `Definition{n}` counter.
    next: u32,
}

impl Aliases {
    /// `aliasFor(definitionKey)` (`ts-shape.ts:24-33`): reuse the bare name when it is an identifier
    /// and unused, else `Definition${++aliasIndex}`, incrementing until unique.
    fn alias_for(&mut self, key: &str) -> String {
        if let Some(existing) = self.map.get(key) {
            return existing.clone();
        }
        // `definitionKey.slice(definitionKey.indexOf("/") + 1)` — the FIRST slash.
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

/// `render(schema)` (`ts-shape.ts:35-90`) — §12 step 3's precedence order, exactly.
fn render(
    schema: &Value,
    defs: &IndexMap<String, &Value>,
    aliases: &mut Aliases,
    depth: u32,
) -> Option<String> {
    if depth > MAX_RENDER_DEPTH {
        return None;
    }
    // `if (!isSchema(schema) || hasUnsupportedKeyword(schema)) return null;`
    let map = schema.as_object()?;
    if has_unsupported_keyword(map) {
        return None;
    }

    // 1 · `$ref` — `/^#\/(\$defs|definitions)\/([^/]+)$/`, and it must resolve.
    if let Some(reference) = map.get("$ref") {
        // `if (typeof schema.$ref !== "string") return null;`
        let rest = reference.as_str()?.strip_prefix("#/")?;
        let (group, token) = rest.split_once('/')?;
        // `[^/]+` — the token cannot contain a slash and cannot be empty; `$` anchors the end.
        if (group != "$defs" && group != "definitions") || token.contains('/') || token.is_empty() {
            return None;
        }
        let key = format!("{group}/{}", decode_pointer_token(token));
        if !defs.contains_key(&key) {
            return None;
        }
        return Some(aliases.alias_for(&key));
    }

    // 2 · `enum` — every member through `renderLiteral`, or `None`.
    if let Some(members) = map.get("enum").and_then(Value::as_array) {
        let rendered = members.iter().map(render_literal).collect::<Option<Vec<_>>>()?;
        return Some(rendered.join(" | "));
    }

    // 3 · `const` — `Object.hasOwn`, so `const: null` takes this branch and renders `null`.
    if map.contains_key("const") {
        return render_literal(map.get("const")?);
    }

    // 4 · `anyOf` / `oneOf`. The GUARD is "either is an array"; the SELECTION is `anyOf ?? oneOf`,
    // so a non-array `anyOf` beside an array `oneOf` throws upstream and is `None` here.
    let any_of = map.get("anyOf").filter(|value| !value.is_null());
    let one_of = map.get("oneOf").filter(|value| !value.is_null());
    if any_of.is_some_and(Value::is_array) || one_of.is_some_and(Value::is_array) {
        let variants = any_of.or(one_of)?.as_array()?;
        if variants.is_empty() {
            return None;
        }
        let rendered = variants
            .iter()
            .map(|variant| render(variant, defs, aliases, depth.saturating_add(1)))
            .collect::<Option<Vec<_>>>()?;
        return Some(rendered.join(" | "));
    }

    let type_field = map.get("type");

    // 5 · object. `properties !== undefined` is key PRESENCE; a present non-object is `None`.
    if type_field.and_then(Value::as_str) == Some("object") || map.contains_key("properties") {
        let Some(raw) = map.get("properties") else { return Some("{}".to_string()) };
        let properties = raw.as_object()?;
        if properties.is_empty() {
            return Some("{}".to_string());
        }
        let required = required_names(map);
        let mut parts = Vec::with_capacity(properties.len());
        for (name, property) in properties {
            let rendered = render(property, defs, aliases, depth.saturating_add(1))?;
            let optional = if required.contains(name.as_str()) { "" } else { "?" };
            parts.push(format!("{}{optional}: {rendered};", format_property_name(name)));
        }
        return Some(format!("{{ {} }}", parts.join(" ")));
    }

    // 6 · array — BEFORE the `type`-array rule. The item is parenthesised when it is itself a union.
    if type_field.and_then(Value::as_str) == Some("array") {
        let Some(items) = map.get("items") else { return Some("unknown[]".to_string()) };
        let item = render(items, defs, aliases, depth.saturating_add(1))?;
        // `needsParentheses` is a plain `includes(" | ")` on the RENDERED item.
        return Some(if item.contains(" | ") { format!("({item})[]") } else { format!("{item}[]") });
    }

    // 7 · `type: [..]` — `renderType` switches on the raw value, so a non-string member is `None`.
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

/// `hasUnsupportedKeyword` (`ts-shape.ts:113-119`).
///
/// `additionalProperties: false` is a closed-object CONSTRAINT, not a shape — it is exempt, the
/// comparison is strict (`!== false`, so `additionalProperties: 0` is NOT exempt), and the test is
/// repeated at every node.
fn has_unsupported_keyword(map: &JsonMap<String, Value>) -> bool {
    UNSUPPORTED_KEYWORDS.iter().any(|keyword| match map.get(*keyword) {
        None => false,
        Some(value) if *keyword == "additionalProperties" => *value != Value::Bool(false),
        Some(_) => true,
    })
}

/// `renderType` (`ts-shape.ts:125-136`) — the six-way map. Anything else is `None`.
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

/// `renderLiteral` (`ts-shape.ts:138-141`).
///
/// `Number.isFinite` is satisfied by construction: `serde_json` cannot hold NaN or Infinity, so a
/// [`Value::Number`] is always finite.
fn render_literal(value: &Value) -> Option<String> {
    match value {
        Value::Null | Value::String(_) | Value::Bool(_) => serde_json::to_string(value).ok(),
        Value::Number(number) => Some(js_number(number)),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// JS `String(n)` / `JSON.stringify(n)` for a JSON number.
///
/// JS has one numeric type, so `JSON.parse("1.0")` is the integer `1` and `String(1)` is `"1"`;
/// `serde_json` preserves the `1.0` spelling it parsed. Fold an integral float back to the integer
/// JS would print. Shared with [`crate::proxy::format_schema`]'s annotation rendering, which faces
/// the identical problem through `JSON.stringify`.
#[must_use]
pub(crate) fn js_number(number: &Number) -> String {
    if number.as_i64().is_none()
        && number.as_u64().is_none()
        && let Some(float) = number.as_f64()
        && float.fract() == 0.0
        && float.abs() <= 9_007_199_254_740_992.0
    {
        // Exact: |float| <= 2^53 with a zero fractional part.
        return format!("{}", float as i64);
    }
    number.to_string()
}

/// `JSON.stringify(value)` with [`js_number`]'s numeric spelling at the top level.
///
/// A float nested inside a composite `const` / `default` keeps `serde_json`'s spelling (`1.0` where
/// JS prints `1`); that is a bounded, recorded difference inside an annotation string no consumer
/// parses, and it buys a bounded-depth implementation.
#[must_use]
pub(crate) fn js_json(value: &Value) -> String {
    match value {
        Value::Number(number) => js_number(number),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// `formatPropertyName` (`ts-shape.ts:143-145`).
fn format_property_name(name: &str) -> String {
    if is_identifier(name) {
        name.to_string()
    } else {
        serde_json::to_string(name).unwrap_or_else(|_| format!("{name:?}"))
    }
}

/// `/^[A-Za-z_$][\w$]*$/`, where JS `\w` is ASCII `[A-Za-z0-9_]`. The empty string is not an
/// identifier.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// `decodePointerToken` (`ts-shape.ts:121-123`). RFC 6901 order: `~1` BEFORE `~0`, or `~01` decodes
/// to `/` instead of `~1`.
fn decode_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// The string members of `required`, or the empty set (`ts-shape.ts:66-68`).
fn required_names(map: &JsonMap<String, Value>) -> HashSet<&str> {
    map.get("required")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}
```

---

## 2 · MCP-211 — `format_schema` and its four helpers

Upstream: [tool-metadata.ts:162-296](../../tmp/pi-mcp-adapter/tool-metadata.ts).
Spec: [13e-mcp-tools.md §4, :226-267](../../docs/gap-analysis/13e-mcp-tools.md) and
[:1009-1023](../../docs/gap-analysis/13e-mcp-tools.md).

### 2.1 · Where it lands

[proxy/tool_metadata.rs](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs), **immediately after
`find_tool_by_name`** (its body closes at `:413`) and before the `truncate_at_word` de-duplication
comment at `:415`. That module *is* the crate's `tool-metadata.ts`, its sibling `findToolByName`
([tool-metadata.ts:154](../../tmp/pi-mcp-adapter/tool-metadata.ts)) is already there, and the
module's own header records that the whole file moves to `crate::renderers` as a delete when 13e
lands — so `format_schema` travels with `find_tool_by_name`.

Add to that file's imports: `use std::collections::HashSet;`, and widen the `serde_json` line to
`use serde_json::{Map as JsonMap, Value};`.

### 2.2 · Six upstream behaviours, four of which a plausible port gets wrong

1. **`formatProperty` returns a `Vec<String>`, not a `String`**
   ([tool-metadata.ts:197](../../tmp/pi-mcp-adapter/tool-metadata.ts), spread with `...` at
   [:179](../../tmp/pi-mcp-adapter/tool-metadata.ts),
   [:222](../../tmp/pi-mcp-adapter/tool-metadata.ts),
   [:227](../../tmp/pi-mcp-adapter/tool-metadata.ts)). Port the shape, not just the joined output —
   the joins are then unambiguous.
2. **`appendSchemaAnnotations` skips an EMPTY description.**
   [:283](../../tmp/pi-mcp-adapter/tool-metadata.ts) is
   `if (schema.description && typeof schema.description === "string")` — truthiness *and* a type
   test, so `description: ""` contributes nothing. `map.get("description").and_then(Value::as_str)`
   alone emits a bare `- `. **This is the single most likely silent divergence in the unit.**
3. **The eight annotation keys use `!== undefined`**
   ([:288](../../tmp/pi-mcp-adapter/tool-metadata.ts)), which is key *presence* — an explicit `null`
   still renders `[format: null]`. Use `map.get(key)`, never `.filter(|v| !v.is_null())`.
4. **`formatType` rule 1 is `Object.hasOwn`**
   ([:255](../../tmp/pi-mcp-adapter/tool-metadata.ts)), so `{"const": null}` renders `const null`.
   Use `JsonMap::contains_key`, never `get(..).is_some_and(..)`.
5. **`formatType` rule 4 is gated on TRUTHINESS**, not presence
   ([:267](../../tmp/pi-mcp-adapter/tool-metadata.ts)): `"type": ""` and `"type": 0` are falsy and
   fall through to rules 5 and 6.
6. **`formatVariants` nests at `indent + "    "`**
   ([:248](../../tmp/pi-mcp-adapter/tool-metadata.ts)) while `formatProperty` nests at
   `indent + "  "` ([:209](../../tmp/pi-mcp-adapter/tool-metadata.ts)). Four spaces vs two.

**The depth cap.** Upstream has no `try`/`catch` on this path at all, so a pathological schema is an
uncaught `RangeError`; Rust would abort. The cap degrades to the function's own `(complex schema)`
sentinel rather than inventing a new string.

### 2.3 · The code

```rust
/// The annotation keys, in `appendSchemaAnnotations`' exact order (`tool-metadata.ts:287`).
/// `default` is appended AFTER these and is not part of the list.
const SCHEMA_ANNOTATION_KEYS: [&str; 8] =
    ["minLength", "maxLength", "minimum", "maximum", "minItems", "maxItems", "format", "pattern"];

/// Same rationale as [`crate::ts_shape`]'s cap: `formatSchema` has no `null` channel, so an
/// over-deep schema degrades to the function's own `(complex schema)` sentinel rather than
/// exhausting the stack. Upstream has no `try`/`catch` on this path.
const MAX_SCHEMA_DEPTH: u32 = 64;

/// `tool-metadata.ts:162` `formatSchema(schema, indent = "  ")` (MCP-211).
///
/// Model-facing text: it is the `Parameters:` body of `mcp({describe})` and `mcp({search})`, and the
/// `Expected parameters:` suffix on the failure results — **those only**. Drift changes retry
/// behaviour. Note [`crate::proxy::execute_describe`] passes `"  "` and
/// [`crate::proxy::execute_search`] passes `"    "`.
///
/// **Divergence (deliberate).** `serde_json` has no `preserve_order`, so `properties` renders in
/// lexicographic key order where upstream renders document order. Unrecoverable at this seam — the
/// schema is already deserialised — and deterministic.
#[must_use]
pub fn format_schema(schema: &Value, indent: &str) -> String {
    // `!schema || typeof schema !== "object" || Array.isArray(schema)`
    let Some(map) = schema.as_object() else { return format!("{indent}(no schema)") };

    // `s.type === "object" && s.properties && typeof s.properties === "object" && !isArray`
    if map.get("type").and_then(Value::as_str) == Some("object")
        && let Some(properties) = map.get("properties").and_then(Value::as_object)
    {
        if properties.is_empty() {
            return format!("{indent}(no parameters)");
        }
        let required = schema_required_names(map);
        let mut lines: Vec<String> = Vec::new();
        for (name, property) in properties {
            lines.extend(format_property(
                name,
                property,
                required.contains(name.as_str()),
                indent,
                0,
            ));
        }
        return lines.join("\n");
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

/// `tool-metadata.ts:197` `formatProperty(name, schema, required, indent)` — parts joined by ONE
/// space, then the nested block at `indent + "  "`.
fn format_property(
    name: &str,
    schema: &Value,
    required: bool,
    indent: &str,
    depth: u32,
) -> Vec<String> {
    // The non-object early return: no `(type)` part, no annotations, one line.
    let Some(map) = schema.as_object() else {
        let marker = if required { " *required*" } else { "" };
        return vec![format!("{indent}{name}{marker}")];
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
    lines
}

/// `tool-metadata.ts:254` `formatType(schema)` — first match wins, six rules, `""` for none.
fn format_type(schema: &JsonMap<String, Value>) -> String {
    // 1 · `Object.hasOwn(schema, "const")` — PRESENCE, so `const: null` reads `const null`.
    if schema.contains_key("const") {
        let value = schema.get("const").unwrap_or(&Value::Null);
        return format!("const {}", crate::ts_shape::js_json(value));
    }
    // 2 · `enum`
    if let Some(members) = schema.get("enum").and_then(Value::as_array) {
        let rendered =
            members.iter().map(crate::ts_shape::js_json).collect::<Vec<_>>().join(", ");
        return format!("enum: {rendered}");
    }
    // 3 · `Array.isArray(schema.type)` — `String(type)` per member.
    if let Some(list) = schema.get("type").and_then(Value::as_array) {
        return list.iter().map(js_string_of).collect::<Vec<_>>().join(" | ");
    }
    // 4 · TRUTHY `schema.type` — `""` and `0` are falsy and fall through to rule 5.
    if let Some(value) = schema.get("type").filter(|value| is_js_truthy(value)) {
        return js_string_of(value);
    }
    // 5 · an object, non-array `properties`
    if schema.get("properties").is_some_and(Value::is_object) {
        return "object".to_string();
    }
    // 6 · `schema.items !== undefined`
    if schema.contains_key("items") {
        return "array".to_string();
    }
    String::new()
}

/// `tool-metadata.ts:282` `appendSchemaAnnotations(parts, schema)` — description, then the eight
/// keys IN ORDER, then `default`.
///
/// Two presence rules that differ: the description is gated on JS **truthiness** *and* being a
/// string, so `description: ""` contributes nothing; the eight keys and `default` are gated on
/// `!== undefined`, so an explicit `null` still renders.
fn append_schema_annotations(parts: &mut Vec<String>, schema: &JsonMap<String, Value>) {
    if let Some(description) = schema.get("description").and_then(Value::as_str)
        && !description.is_empty()
    {
        parts.push(format!("- {description}"));
    }
    for key in SCHEMA_ANNOTATION_KEYS {
        if let Some(value) = schema.get(key) {
            parts.push(format!("[{key}: {}]", crate::ts_shape::js_json(value)));
        }
    }
    if let Some(value) = schema.get("default") {
        parts.push(format!("[default: {}]", crate::ts_shape::js_json(value)));
    }
}

/// `tool-metadata.ts:212` `formatNestedSchema(schema, indent)` — `anyOf`, `oneOf`, `items`,
/// `properties`, in that order.
///
/// The single depth choke point: both recursion paths ([`format_property`] and [`format_variants`])
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
    // `schema.items !== undefined` — renders as a property literally named `items`, never required.
    if let Some(items) = schema.get("items") {
        lines.extend(format_property("items", items, false, indent, depth.saturating_add(1)));
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let required = schema_required_names(schema);
        for (name, property) in properties {
            lines.extend(format_property(
                name,
                property,
                required.contains(name.as_str()),
                indent,
                depth.saturating_add(1),
            ));
        }
    }
    lines
}

/// `tool-metadata.ts:234` `formatVariants(keyword, variants, indent)` — the variant body nests at
/// `indent + "    "`, four spaces, not two.
fn format_variants(
    keyword: &str,
    variants: &[Value],
    indent: &str,
    depth: u32,
) -> Vec<String> {
    let mut lines = vec![format!("{indent}{keyword}:")];
    for variant in variants {
        let Some(map) = variant.as_object() else {
            lines.push(format!("{indent}  - {}", crate::ts_shape::js_json(variant)));
            continue;
        };
        // `formatType(s) || "schema"`
        let type_str = format_type(map);
        let label = if type_str.is_empty() { "schema".to_string() } else { type_str };
        let mut parts = vec![format!("{indent}  - {label}")];
        append_schema_annotations(&mut parts, map);
        lines.push(parts.join(" "));
        lines.extend(format_nested_schema(map, &format!("{indent}    "), depth.saturating_add(1)));
    }
    lines
}

/// JS `String(value)` for the shapes `type` can legally hold.
///
/// A non-string is a schema bug; its compact JSON is strictly more informative than JS's
/// `"[object Object]"` for an object or `"1,2"` for an array, and that is the one deliberate
/// divergence in this function. Numbers agree exactly, via [`crate::ts_shape::js_number`].
fn js_string_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => crate::ts_shape::js_json(other),
    }
}

/// JS truthiness, for [`format_type`]'s rule 4. An empty string, `0` and `false` are falsy; an empty
/// object and an empty array are truthy.
fn is_js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `Array.isArray(s.required) ? s.required.filter(isString) : []`.
fn schema_required_names(map: &JsonMap<String, Value>) -> HashSet<&str> {
    map.get("required")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}
```

---

## 3 · Wiring MCP-091 / MCP-098 / MCP-211 through the trait

Replace the two constant stubs at
[proxy/testsupport.rs:193-198](../../crates/cyrup-mcp/src/proxy/testsupport.rs) with delegations, so
the existing 13d conformance suite exercises the real renderers:

```rust
    fn format_schema(&self, schema: &Value, indent: &str) -> String {
        crate::proxy::format_schema(schema, indent)
    }
    fn render_ts_shape(&self, schema: &Value) -> Option<String> {
        crate::ts_shape::render_ts_shape(schema)
    }
```

`crate::proxy::format_schema` resolves through
[proxy/mod.rs](../../crates/cyrup-mcp/src/proxy/mod.rs)'s `pub use tool_metadata::*;`.

**Two existing assertions then need their expected literal corrected** — both took it from the stub,
and both feed `{"type": "object"}`, which the real renderer takes down precedence rule 5 with no
`properties` to `{}`:

* [proxy/discovery.rs:907](../../crates/cyrup-mcp/src/proxy/discovery.rs) in
  `describe_forks_between_shape_and_parameters` →
  `assert!(text_of(&execute_describe(&ctx, "srv_run")).ends_with("\nShape:\n{}"));`
* [proxy/discovery.rs:1047](../../crates/cyrup-mcp/src/proxy/discovery.rs) in
  `search_with_schemas_indents_the_shape_block_by_four` →
  `assert!(text.contains("srv_run\n  Run it\n\n  Shape:\n    {}"), "{text}");`

Nothing asserts the `format_schema` stub's `"(schema)"` literal, so no third edit is needed.

---

## 4 · MCP-093 — the `ajv-formats` delta, inside the module MCP-092 will own

Upstream: [json-schema-validator.ts:47](../../tmp/pi-mcp-adapter/json-schema-validator.ts) and
[:63](../../tmp/pi-mcp-adapter/json-schema-validator.ts) — `addFormats(ajv)` on both instances.
Spec: [13b-mcp-config.md:1575-1585](../../docs/gap-analysis/13b-mcp-config.md).

### 4.1 · What actually gets registered, measured against ajv-formats v3.0.1

| ajv format | what ajv asserts | jsonschema 0.46.9 | disposition |
|---|---|---|---|
| `float`, `double` | `validateNumber()` = `return true` (`formats.js:193`) | — | **do not register.** Registering a no-op is noise. |
| `password`, `binary` | literally `true` (`formats.js:59`, `:61`) | — | **do not register.** Same reason. |
| `int64` | `Number.isInteger(x)` (`formats.js:189`) | — | **unreachable** via `with_format`; on the OpenAPI shape `{"type":"integer","format":"int64"}` it is fully subsumed by `type`. |
| `int32` | `Number.isInteger(x) && -2^31 <= x <= 2^31-1` (`formats.js:186`) | — | **unreachable**; residual is the range check alone. |
| `url` | dperini regex, http/https/ftp only (`formats.js:29`) | absent | **register on both** |
| `byte` | line-anchored base64 (`formats.js:179`) | absent | **register on both** |
| `iso-time` | `getTime(false)` (`formats.js:106`) | absent | **register on both** |
| `iso-date-time` | `getDateTime(false)` (`formats.js:146`) | absent | **register on both** |
| `json-pointer-uri-fragment` | `formats.js:44` | absent | **register on both** |
| `duration` | `formats.js:16` | built in at `draft >= 2019-09` only | **draft-07 builder ONLY** |
| `uuid` | `formats.js:41` | built in at `draft >= 2019-09` only | **draft-07 builder ONLY** |
| everything else ajv ships | — | built in unconditionally | do not register |

`duration` and `uuid` on the 2020-12 builder would **shadow jsonschema's better built-in**, because a
user-registered format is consulted first (`src/keywords/format.rs:1420-1431`). That is why the
registration list is draft-dependent: 5 on 2020-12, 7 on draft-07.

Recovering `int32`/`int64` would need `with_keyword("format", …)`, which overrides the whole `format`
compile path and forfeits every built-in string format unless they are reimplemented. **Not worth an
`i32` range check.** Record the reason in the doc comment so no future maintainer "fixes" it by
adding an inert `with_format("int64", …)` back.

### 4.2 · New module `crates/cyrup-mcp/src/schema_validator.rs`

Register as `pub mod schema_validator;` in [lib.rs:132-151](../../crates/cyrup-mcp/src/lib.rs) and
add a row to the Cut 2 module-map table. `url`, `regex` and `base64` are already crate dependencies
([Cargo.toml:115](../../crates/cyrup-mcp/Cargo.toml), [:120](../../crates/cyrup-mcp/Cargo.toml),
[:138](../../crates/cyrup-mcp/Cargo.toml)); `base64` turns out not to be needed. **Nothing is added
to any manifest.**

```rust
//! `json-schema-validator.ts` — the dual-dialect JSON Schema validator (MCP-092, MCP-093).
//!
//! # Scope landed here
//!
//! **MCP-093** — [`register_ajv_formats`], `ajv-formats`' `addFormats(ajv)` minus everything
//! `jsonschema 0.46.9` already ships — plus the two builders it attaches to, because a registration
//! function with no builder is dead code. **MCP-092** adds `schema_dialect`, the memoised
//! `get_validator` and the `Unsupported JSON Schema dialect: {uri}` error TO THIS FILE. There is
//! exactly one validator module in this crate and this is it; do not create a second.
//!
//! # Why formats matter here at all
//!
//! `rmcp` has no validator hook on `Peer<RoleClient>`, unlike the TS SDK's `jsonSchemaValidator`
//! client option — see the manifest comment on the `jsonschema` edge. This validator checks a
//! server's own `structuredContent` against its own `outputSchema`. A **false rejection breaks a
//! working server**; a false acceptance passes data the model reads anyway. Every predicate below
//! therefore errs lenient, and that direction is deliberate, not accidental.

use std::sync::LazyLock;

use jsonschema::{Draft, ValidationOptions};
use regex::Regex;

/// The 2020-12 builder — `new Ajv2020({strict: false, allErrors: true})` plus `addFormats`
/// (`json-schema-validator.ts:46-47`).
///
/// `should_validate_formats(true)` is not optional: `format` compilation is gated on
/// `ctx.validates_formats_by_default()` (`jsonschema-0.46.9/src/keywords/format.rs:1421`), and
/// without it every format — built-in and registered — is an inert annotation.
#[must_use]
pub fn draft_2020_options() -> ValidationOptions {
    register_ajv_formats(
        jsonschema::options().with_draft(Draft::Draft202012).should_validate_formats(true),
        Draft::Draft202012,
    )
}

/// The draft-07 builder — `new Ajv({strict: false, validateFormats: true, validateSchema: false,
/// allErrors: true})` plus `addFormats` (`json-schema-validator.ts:57-63`).
#[must_use]
pub fn draft_07_options() -> ValidationOptions {
    register_ajv_formats(
        jsonschema::options().with_draft(Draft::Draft7).should_validate_formats(true),
        Draft::Draft7,
    )
}

/// `ajv-formats`' `addFormats(ajv)`, minus everything `jsonschema 0.46.9` already ships (MCP-093).
///
/// `addFormats` with no options installs **`fullFormats`**, not `fastFormats`
/// (`ajv-formats/dist/index.js:13`) — which is why `iso-time` and `iso-date-time` below are ports of
/// ajv's leap-second-aware *functions*, not of its fast regexes.
///
/// The delta is **per-draft**: `duration` and `uuid` are built in only at `draft >= 2019-09`
/// (`jsonschema-0.46.9/src/keywords/format.rs:1388`, `:1409`), and a user-registered format is
/// consulted FIRST (`:1420-1431`), so registering them on the 2020-12 builder would shadow the
/// crate's better implementation with these.
///
/// **Four ajv formats are deliberately absent and must stay absent:**
///
/// * `float`, `double`, `password`, `binary` — ajv asserts nothing for any of them
///   (`ajv-formats/dist/formats.js:193` is `return true`; `password` and `binary` are the literal
///   `true`). Registering a no-op is noise.
/// * `int32`, `int64` — these are NUMERIC formats (`{type: "number", validate}`), and `with_format`
///   is unreachable for them: `CustomFormatValidator::is_valid`
///   (`jsonschema-0.46.9/src/keywords/format.rs:1287-1293`) returns `true` for every non-string
///   instance. A `with_format("int64", …)` here would be dead code — exactly the silent pass this
///   unit exists to prevent. Recovering them needs `with_keyword("format", …)`, which overrides the
///   whole `format` compile path and forfeits every built-in string format. Recorded, not done.
///
/// `should_ignore_unknown_formats` stays at its default `true`
/// (`jsonschema-0.46.9/src/options.rs:51`) — that is ajv's `strict: false` behaviour.
fn register_ajv_formats(options: ValidationOptions, draft: Draft) -> ValidationOptions {
    let options = options
        .with_format("url", is_ajv_url)
        .with_format("byte", is_ajv_byte)
        .with_format("iso-time", is_ajv_time)
        .with_format("iso-date-time", is_ajv_date_time)
        .with_format("json-pointer-uri-fragment", |value: &str| {
            matches_static(&JSON_POINTER_URI_FRAGMENT, value)
        });

    if draft >= Draft::Draft201909 {
        return options;
    }
    // draft-07 only — jsonschema ships both from 2019-09 up. See the doc comment.
    options
        .with_format("duration", |value: &str| matches_static(&DURATION, value))
        .with_format("uuid", |value: &str| matches_static(&UUID, value))
}

/// A `LazyLock` regex that failed to compile matches nothing.
///
/// A compile failure is unreachable on the literals below; the `Option` exists so the impossible
/// branch is a **refusal** rather than an `expect`, which the crate lints deny. Same discipline as
/// [`crate::agent_plugin`]'s `PLUGIN_NAME_CLASS`.
fn matches_static(pattern: &LazyLock<Option<Regex>>, value: &str) -> bool {
    pattern.as_ref().is_some_and(|regex| regex.is_match(value))
}

/// `ajv-formats`' `url` (`formats.js:29`), which is dperini's regex: an **http / https / ftp** URL
/// with a host, excluding private and link-local IPv4 ranges.
///
/// The four `(?!…)` private-range lookaheads are not portable to Rust's `regex` (no lookaround) and
/// **must not be reproduced by other means**: a local MCP server on `http://127.0.0.1:8931/mcp` is
/// exactly the shape this port has to accept, and rejecting it would break a working server to
/// satisfy a check dperini wrote for public-URL forms. What survives is the part that carries
/// meaning — the scheme allowlist and the host requirement — which is still far stricter than a bare
/// `Url::parse` (that accepts `mailto:`, `data:` and every other scheme).
fn is_ajv_url(value: &str) -> bool {
    let Ok(parsed) = url::Url::parse(value) else { return false };
    matches!(parsed.scheme(), "http" | "https" | "ftp") && parsed.host().is_some()
}

/// `ajv-formats`' `byte` (`formats.js:179-183`):
/// `/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/gm`.
///
/// **The `m` flag is load-bearing**: `^` and `$` are line anchors, so ajv accepts PEM-style wrapped
/// base64 as long as ONE line conforms. `base64::Engine::decode` would reject every wrapped payload,
/// which is the false-rejection direction this module refuses. JS line terminators are `\n`, `\r`,
/// U+2028 and U+2029.
fn is_ajv_byte(value: &str) -> bool {
    value.split(['\n', '\r', '\u{2028}', '\u{2029}']).any(is_base64_line)
}

/// One line of the `byte` pattern. The alphabet is `[A-Za-z0-9+/]`; the total length is always a
/// multiple of four, since each alternative of the padded tail is itself four characters. An empty
/// line matches, exactly as the regex does.
fn is_base64_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    let len = bytes.len();
    if !len.is_multiple_of(4) {
        return false;
    }
    let alphabet = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/';
    let at = |offset: usize| len.checked_sub(offset).and_then(|i| bytes.get(i)).copied();
    let core_len = match (at(2), at(1)) {
        // `[A-Za-z0-9+/]{2}==`
        (Some(b'='), Some(b'=')) => len.saturating_sub(2),
        // `[A-Za-z0-9+/]{3}=`
        (Some(third), Some(b'=')) if alphabet(third) => len.saturating_sub(1),
        _ => len,
    };
    bytes.get(..core_len).is_some_and(|core| core.iter().copied().all(alphabet))
}

/// `ajv-formats`' `iso-time` = `getTime(false)` (`formats.js:106-127`) — RFC 3339 `full-time` with
/// an **optional** timezone, plus the leap-second rule.
///
/// The leap-second arm is why this is a function and not the `fastFormats` regex: `23:59:60Z` is
/// valid, and so is any local time that maps to UTC `23:59:60`. The optional timezone
/// (`strictTimeZone === false`) is the entire difference from jsonschema's built-in `time`.
fn is_ajv_time(value: &str) -> bool {
    // `/^(\d\d):(\d\d):(\d\d(?:\.\d+)?)(z|([+-])(\d\d)(?::?(\d\d))?)?$/i`
    let Some(groups) = captures_static(&ISO_TIME, value) else { return false };
    let (Some(hour), Some(minute), Some(second)) =
        (groups.number(1), groups.number(2), groups.float(3))
    else {
        return false;
    };
    let sign = if groups.text(5) == Some("-") { -1i64 } else { 1i64 };
    let tz_hour = groups.number(6).unwrap_or(0);
    let tz_minute = groups.number(7).unwrap_or(0);
    // `if (tzH > 23 || tzM > 59 || (strictTimeZone && !tz)) return false`
    if tz_hour > 23 || tz_minute > 59 {
        return false;
    }
    if hour <= 23 && minute <= 59 && second < 60.0 {
        return true;
    }
    let utc_minute = minute - tz_minute * sign;
    let utc_hour = hour - tz_hour * sign - i64::from(utc_minute < 0);
    (utc_hour == 23 || utc_hour == -1) && (utc_minute == 59 || utc_minute == -1) && second < 61.0
}

/// `ajv-formats`' `iso-date-time` = `getDateTime(false)` (`formats.js:145-153`).
///
/// `str.split(/t|\s/i)` splits on EVERY separator regardless of the missing `g` flag, and the result
/// must have exactly two parts — so a second `T` rejects. The date half is ajv's own `date`
/// (`formats.js:62-76`), which is calendar-aware including the leap-year rule for February.
fn is_ajv_date_time(value: &str) -> bool {
    let parts: Vec<&str> =
        value.split(|c: char| c == 't' || c == 'T' || c.is_whitespace()).collect();
    let [date, time] = parts.as_slice() else { return false };
    is_ajv_date(date) && is_ajv_time(time)
}

/// `ajv-formats`' `date` (`formats.js:62-76`) — `/^(\d\d\d\d)-(\d\d)-(\d\d)$/` plus a real
/// month-length table and the proleptic Gregorian leap rule.
fn is_ajv_date(value: &str) -> bool {
    const DAYS: [i64; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let Some(groups) = captures_static(&ISO_DATE, value) else { return false };
    let (Some(year), Some(month), Some(day)) =
        (groups.number(1), groups.number(2), groups.number(3))
    else {
        return false;
    };
    if !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let limit = if month == 2 && leap {
        29
    } else {
        DAYS.get(usize::try_from(month).unwrap_or(0)).copied().unwrap_or(0)
    };
    (1..=limit).contains(&day)
}

/// `LazyLock` capture helper — `None` when the pattern failed to compile or did not match.
fn captures_static<'t>(
    pattern: &LazyLock<Option<Regex>>,
    value: &'t str,
) -> Option<Groups<'t>> {
    pattern.as_ref().and_then(|regex| regex.captures(value)).map(Groups)
}

/// Non-panicking capture-group access. Every accessor is `Option`, so an absent optional group and
/// an out-of-range index are the same, safe answer — which is what keeps `clippy::indexing_slicing`
/// and `clippy::unwrap_used` satisfied on a regex with seven groups, four of them optional.
struct Groups<'t>(regex::Captures<'t>);

impl<'t> Groups<'t> {
    fn text(&self, index: usize) -> Option<&'t str> {
        self.0.get(index).map(|found| found.as_str())
    }
    fn number(&self, index: usize) -> Option<i64> {
        self.text(index).and_then(|text| text.parse().ok())
    }
    fn float(&self, index: usize) -> Option<f64> {
        self.text(index).and_then(|text| text.parse().ok())
    }
}

// `ajv-formats`' own patterns, with lookaround rewritten as alternation (Rust's `regex` compiles to
// a finite automaton and has none).
static ISO_DATE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^(\d{4})-(\d{2})-(\d{2})$").ok());
static ISO_TIME: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)^(\d{2}):(\d{2}):(\d{2}(?:\.\d+)?)(z|([+-])(\d{2})(?::?(\d{2}))?)?$").ok()
});
static JSON_POINTER_URI_FRAGMENT: LazyLock<Option<Regex>> = LazyLock::new(|| {
    // `formats.js:44` verbatim.
    Regex::new(r"(?i)^#(?:/(?:[a-z0-9_\-.!$&'()*+,;:=@]|%[0-9a-f]{2}|~0|~1)*)*$").ok()
});
static DURATION: LazyLock<Option<Regex>> = LazyLock::new(|| {
    // `formats.js:16` is `/^P(?!$)((\d+Y)?(\d+M)?(\d+D)?(T(?=\d)(\d+H)?(\d+M)?(\d+S)?)?|(\d+W)?)$/`.
    // Two lookaheads, both rewritten by enumeration:
    //   `(?!$)`   — "P alone is not a duration"     ⇒ a non-empty date part, OR a bare time part,
    //                                                 OR the week form.
    //   `T(?=\d)` — "T must be followed by a digit" ⇒ the time part carries at least one component.
    Regex::new(concat!(
        r"^P(?:",
        r"(?:\d+Y(?:\d+M)?(?:\d+D)?|\d+M(?:\d+D)?|\d+D)",
        r"(?:T(?:\d+H(?:\d+M)?(?:\d+S)?|\d+M(?:\d+S)?|\d+S))?",
        r"|T(?:\d+H(?:\d+M)?(?:\d+S)?|\d+M(?:\d+S)?|\d+S)",
        r"|\d+W",
        r")$",
    ))
    .ok()
});
static UUID: LazyLock<Option<Regex>> = LazyLock::new(|| {
    // `formats.js:41` — note the OPTIONAL `urn:uuid:` prefix.
    Regex::new(r"(?i)^(?:urn:uuid:)?[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$").ok()
});
```

---

## 5 · MCP-085 — the sanitising tail

Upstream: [utils.ts:238-262](../../tmp/pi-mcp-adapter/utils.ts) `formatTerminalError`.
Spec: [13b-mcp-config.md §9, :537-548](../../docs/gap-analysis/13b-mcp-config.md).

### 5.1 · Three of the four clauses are already landed. Do not write them again.

The STATUS row at [13-cyrup-mcp-STATUS.md:588](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) says
"No function walks an error's children/`source()` chain with a cycle guard … de-duplicates and joins
with `": "`, then sanitises." Measured against the tree:

* **the children walk, the head-drop rule and the de-duplication** are
  [`render_aggregate_texts`](../../crates/cyrup-mcp/src/errors.rs) at
  [errors.rs:144-161](../../crates/cyrup-mcp/src/errors.rs) — upstream's
  `if (messages.length === countBefore && value.message)`
  ([utils.ts:249](../../tmp/pi-mcp-adapter/utils.ts)) and its two dedupe layers
  ([:242](../../tmp/pi-mcp-adapter/utils.ts) `seen`, [:261](../../tmp/pi-mcp-adapter/utils.ts)
  `[...new Set(messages)]`). It is the `Display` of all seven `McpError` aggregates *and* of
  `ManagerError` ([server_manager.rs:266-267](../../crates/cyrup-mcp/src/server_manager.rs)), and it
  was measured against node 22 — table at [errors.rs:44-52](../../crates/cyrup-mcp/src/errors.rs).
* **the "all children message-less" case** is the empty-message skip at
  [errors.rs:150-156](../../crates/cyrup-mcp/src/errors.rs), asserted at
  [errors.rs:571](../../crates/cyrup-mcp/src/errors.rs).
* **the cycle guard is unbuildable and therefore unnecessary.** `CleanupErrors(Vec<McpError>)`
  ([errors.rs:463](../../crates/cyrup-mcp/src/errors.rs)) is built by an owning `push`;
  `ManagerError::Aggregate.children` is `Vec<Arc<ManagerError>>`
  ([server_manager.rs:184-188](../../crates/cyrup-mcp/src/server_manager.rs)) built by
  `ManagerError::aggregate(head, children)`
  ([server_manager.rs:207](../../crates/cyrup-mcp/src/server_manager.rs)) from already-constructed
  values. Neither uses `Arc::new_cyclic` nor interior mutability, so an error cannot reach itself.
  Both walks are budget-capped anyway ([errors.rs:381](../../crates/cyrup-mcp/src/errors.rs),
  [errors.rs:443](../../crates/cyrup-mcp/src/errors.rs),
  [server_manager.rs:226](../../crates/cyrup-mcp/src/server_manager.rs)).

**Do not write a second `source()` walk.** `McpError::Io`'s template is `{path}: {source}`
([errors.rs:195](../../crates/cyrup-mcp/src/errors.rs)), so re-walking its source yields
`/p: denied: denied`.

What is genuinely missing is what the file itself names at
[errors.rs:58-61](../../crates/cyrup-mcp/src/errors.rs): the `sanitizeTerminalText` tail
([utils.ts:261](../../tmp/pi-mcp-adapter/utils.ts)). Its dependency has landed —
`sanitize_terminal_text` is at [ui.rs:376](../../crates/cyrup-mcp/src/ui.rs) and MCP-235 is
`implemented` ([13-cyrup-mcp-STATUS.md:749](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) — and
two live `TODO(MCP-235)` comments still say "once it lands".

### 5.2 · The function

Add to [ui.rs](../../crates/cyrup-mcp/src/ui.rs) directly after `sanitize_display_text` (`:424`), so
the three `utils.ts` terminal functions sit together.

```rust
/// `utils.ts:238` `formatTerminalError(error)` (MCP-085) — the projection a USER reads.
///
/// The walk is already done. [`crate::errors::render_aggregate_texts`] is the `Display` of all seven
/// [`crate::errors::McpError`] aggregates and of [`crate::server_manager::ManagerError`], and it
/// implements the head-drop (`utils.ts:249`), the empty-message skip and the two de-duplication
/// layers (`:242`, `:261`), measured against node 22 — table at the top of `errors.rs`. This is the
/// tail that file records as the residual: `sanitizeTerminalText` (`utils.ts:261`).
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

* [lifecycle.rs:1328-1329](../../crates/cyrup-mcp/src/lifecycle.rs) →
  `tracing::debug!("MCP: auth-required callback failed for {name}: {}", crate::ui::format_terminal_error(&error));`
* [lifecycle.rs:1413-1414](../../crates/cyrup-mcp/src/lifecycle.rs) — `error` is `&McpError` here,
  per the signature at [:1370-1377](../../crates/cyrup-mcp/src/lifecycle.rs) →
  `tracing::error!("MCP: Failed to {target}: {}", crate::ui::format_terminal_error(error));`

Finally, replace the "**Residual:**" sentence at
[errors.rs:58-61](../../crates/cyrup-mcp/src/errors.rs) with a pointer to
`crate::ui::format_terminal_error` — the residual is closed and the header must not keep claiming
otherwise.

### 5.3 · Where the sanitiser must NOT be applied

The gateway's model-facing text at [proxy/auth.rs:355](../../crates/cyrup-mcp/src/proxy/auth.rs) and
[proxy/call.rs:624](../../crates/cyrup-mcp/src/proxy/call.rs)
(`Failed to connect to "{server}": {message}`) is **upstream-faithful as-is** — upstream does not
sanitise the tool-result path. Sanitising it would be a silent divergence. The `/mcp` panel already
sanitises at ingest ([ui.rs:2330](../../crates/cyrup-mcp/src/ui.rs),
[:2386](../../crates/cyrup-mcp/src/ui.rs), [:2435](../../crates/cyrup-mcp/src/ui.rs),
[:2446](../../crates/cyrup-mcp/src/ui.rs)), and the approval dialog at
[proxy/approval.rs:316-317](../../crates/cyrup-mcp/src/proxy/approval.rs).

The two stderr log sites are the gap, and sanitising them is a **justified cyrup divergence**: pi's
`console.error` goes to pi's own log, while cyrup's `tracing` writer is `io::stderr`
([bootstrap.rs:287](../../crates/cyrup/src/bootstrap.rs)) — the same terminal the TUI paints.

---

## 6 · MCP-089 — the taxonomy methods

Upstream: [errors.ts](../../tmp/pi-mcp-adapter/errors.ts).
Spec: [13b-mcp-config.md §10, :582-616](../../docs/gap-analysis/13b-mcp-config.md).

**This is vocabulary with no production consumer, deliberately, and that must be said in the code.**
§10 records that `wrapError` "survives as taxonomy with no caller until another subsystem needs it",
and every message the proxy emits is already byte-exact upstream text built from `McpErrorCode` — see
`disabled_call_result` at [proxy/call.rs:155](../../crates/cyrup-mcp/src/proxy/call.rs). Injecting
`recovery_hint()` into any of those strings would *break* parity. The methods exist so the taxonomy
cannot rot silently, and the exhaustive match — **no `_` arm** — is what enforces that.
`#[non_exhaustive]` ([errors.rs:166](../../crates/cyrup-mcp/src/errors.rs)) does not restrict matches
inside the defining crate.

**Do not conflate the two code vocabularies.**
[`crate::proxy::McpErrorCode`](../../crates/cyrup-mcp/src/proxy/error_vocab.rs) (32 arms) is
`proxy-modes.ts`'s `details.error` value for the `mcp` gateway tool's JSON result.
`McpError::code()` is `errors.ts`'s `McpUiError.code`
([errors.ts:18](../../tmp/pi-mcp-adapter/errors.ts)). Different things, different consumers.

Add to the `impl McpError` block in [errors.rs](../../crates/cyrup-mcp/src/errors.rs), after
`is_credential_store_failure` (which closes at `:453`; the block closes at `:454`). Add
`use serde_json::{Map as JsonMap, Value};` to the file's imports. `McpError` has **thirteen**
variants; every match below names all thirteen.

```rust
    /// `errors.ts:18` `McpUiError.code` (MCP-089) — the machine-readable class.
    ///
    /// **Not [`crate::proxy::McpErrorCode`].** That is `proxy-modes.ts`'s `details.error` value for
    /// the `mcp` gateway tool's JSON result — a model-facing vocabulary with 32 arms and a
    /// different consumer. Conflating the two is the one mistake this doc exists to prevent.
    ///
    /// Post-cut only two of upstream's eight codes survive: `MCP_SERVER_ERROR` (`errors.ts:185`)
    /// and `wrapError`'s `UNKNOWN_ERROR` (`errors.ts:212`). The five MCP Apps codes went with
    /// Cut 2, and `CONSENT_DENIED` / `CONSENT_REQUIRED` (`errors.ts:128`) went with
    /// `consent-manager.ts` (`13-cyrup-mcp.md:418-419`). The rest are cyrup-owned; the two
    /// aggregate codes mirror upstream's own `/cleanup failed|setup failed/` discriminator, which is
    /// the only place those heads are ever classified.
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

    /// `errors.ts:20` `McpUiError.recoveryHint`.
    ///
    /// `Option`, not `&'static str`: the field is `recoveryHint?` and `toJSON` (`errors.ts:45-54`)
    /// lets `JSON.stringify` drop it when `undefined`, so an empty string would serialise a key
    /// upstream omits. [`McpError::Other`] is upstream's `wrapError` fallback, which passes no hint
    /// at all (`errors.ts:211-215`). `Server`'s string is upstream's own, byte for byte
    /// (`errors.ts:187`).
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

    /// `errors.ts:6-12` `McpUiErrorContext` — `{ server?, tool?, … }`, absent keys OMITTED, never
    /// `null` (every upstream constructor spreads conditionally, e.g. `errors.ts:186`).
    ///
    /// `uri` and `session` went with Cut 2. `path` is cyrup's, and the reason is on
    /// [`McpError::Io`]: every adapter-owned path is relocatable through `CYRUP_AGENT_DIR`, so a
    /// bare "permission denied" is unactionable.
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

Update the module header at [errors.rs:13-17](../../crates/cyrup-mcp/src/errors.rs): MCP-089 is
landed, `recovery_hint` returns `Option<&'static str>` (not `&'static str`, which is what the header
currently promises), and the surviving taxonomy is the base shape plus `McpServerError` with
`ConsentError` explicitly cut alongside `consent-manager.ts` — so 13b §10's stale `ports` row cannot
lure the next reader.

---

## 7 · MCP-079 — close the two vocabularies

Spec: [13b-mcp-config.md:1346-1362](../../docs/gap-analysis/13b-mcp-config.md).

**Half landed.** `ApprovalOrigin` is at
[proxy/env.rs:132-187](../../crates/cyrup-mcp/src/proxy/env.rs) with the three surviving arms, both
derivations (`for_proxy_call` at [:156](../../crates/cyrup-mcp/src/proxy/env.rs), `for_direct_tool`
at [:171](../../crates/cyrup-mcp/src/proxy/env.rs) — they differ **only** in their fallback, which is
why both are written out) and `as_str` at [:180](../../crates/cyrup-mcp/src/proxy/env.rs).
`ApprovalOutcome` (upstream `ToolCallApprovalResult`) is at
[proxy/env.rs:189-198](../../crates/cyrup-mcp/src/proxy/env.rs).

**Unmet:** `McpToolApprovalDecision`. Today the decision is a bare string match on dialog labels at
[proxy/approval.rs:327-340](../../crates/cyrup-mcp/src/proxy/approval.rs) against
`APPROVE_ONCE_OPTION` ([proxy/constants.rs:39](../../crates/cyrup-mcp/src/proxy/constants.rs),
`"Allow once"`) and `APPROVE_FOR_SESSION_OPTION`
([:42](../../crates/cyrup-mcp/src/proxy/constants.rs), `"Allow for session"`).

**The unsettled verify, and the ruling.** The spec's verify is "a `script`/`iframe` origin string is
rejected by the deserializer rather than silently accepted." `ApprovalOrigin` derives
`Debug, Clone, Copy, PartialEq, Eq` and **nothing from serde**
([proxy/env.rs:141](../../crates/cyrup-mcp/src/proxy/env.rs)) — there is no deserializer, so the
clause has no surface.

**Ruling: do not add serde.** §0.4 above establishes that `as_str` has *zero* production callers —
`origin` is discarded by `let _ = origin;` at
[proxy/approval.rs:286](../../crates/cyrup-mcp/src/proxy/approval.rs), no `details.origin` key exists
anywhere in the crate, and the broker event that once carried these strings is MCP-233's cut. A
`Deserialize` derive would invent a wire format with neither a producer nor a consumer and then
become a compatibility surface someone must keep stable forever. The obligation the verify is
*reaching for* — that a cut arm cannot re-enter through a string — is discharged by making the
string→enum direction **total and explicit**: a `parse` that returns `None` for `"script"`,
`"iframe"` and `"abstain"`, and exhaustive `as_str` matches with no `_` arm so the vocabulary cannot
gain an arm without a compile error.

`abstain` is correctly absent and the reason is already recorded at
[proxy/approval.rs:251-259](../../crates/cyrup-mcp/src/proxy/approval.rs): "a permission extension
that declines to decide simply does not block, which lands in the same place".

### 7.1 · `ApprovalOrigin::parse`

Inside the existing `impl ApprovalOrigin` block in
[proxy/env.rs](../../crates/cyrup-mcp/src/proxy/env.rs), after `as_str` (which closes at `:186`; the
block closes at `:187`):

```rust
    /// The total inverse of [`ApprovalOrigin::as_str`] (MCP-079).
    ///
    /// **`None` for `"script"` and `"iframe"`** — upstream's other two arms, cut with Cut 4 and
    /// Cut 2. This is the whole of the unit's "rejected rather than silently accepted" obligation.
    ///
    /// It is a `parse`, not a `Deserialize`: nothing in the port ever puts an origin on a wire. The
    /// broker event that did is MCP-233's cut, `SharedBus` is JSON-only and deferred, and
    /// [`ApprovalOrigin::as_str`] has no production caller at all — `origin` is discarded by
    /// `let _ = origin;` in [`ensure_tool_call_approved`]. A serde derive would invent a wire format
    /// with neither producer nor consumer and then owe it stability forever.
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

### 7.2 · `ToolApprovalDecision`

After the `ApprovalOutcome` enum ([proxy/env.rs:198](../../crates/cyrup-mcp/src/proxy/env.rs)). Add
`use crate::proxy::constants::{APPROVE_FOR_SESSION_OPTION, APPROVE_ONCE_OPTION};` to `env.rs` — they
are `const`s, so they are valid match patterns.

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
    /// `"deny"` — [`crate::proxy::DENY_OPTION`], and every non-answer.
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

    /// [`crate::owner`]'s `select` answer → the decision. **Fails closed**: the literal deny option,
    /// an unknown label and a dismissal (`None`) all land on [`Self::Deny`].
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

### 7.3 · Dispatch on the type

Replace the label match at
[proxy/approval.rs:327-340](../../crates/cyrup-mcp/src/proxy/approval.rs) — behaviour-identical, and
the cache rule becomes a property of the type rather than of a match arm:

```rust
    match ToolApprovalDecision::from_dialog_label(decision.as_deref()) {
        ToolApprovalDecision::AllowOnce => ApprovalOutcome::Approved,
        ToolApprovalDecision::AllowForSession => {
            if let Ok(mut approved) = state.approved_tool_calls.lock() {
                approved.insert(cache_key);
            }
            // The insert is best-effort for the same reason the lookup is: a poisoned lock costs a
            // repeat prompt, never an ungated call. The approval itself still stands for THIS call.
            ApprovalOutcome::Approved
        }
        // `return {ok: false, reason: "denied"}` — the literal `Deny`, an unknown label, a
        // dismissal, a timeout, and a fenced (stopped-generation) handle all land here.
        ToolApprovalDecision::Deny => ApprovalOutcome::Denied,
    }
```

Export `ToolApprovalDecision` alongside the other proxy items at
[lib.rs:181](../../crates/cyrup-mcp/src/lib.rs).

---

## 8 · MCP-090 — the log channel

Upstream: [logger.ts](../../tmp/pi-mcp-adapter/logger.ts).
Spec: [13b-mcp-config.md §11, :618-647](../../docs/gap-analysis/13b-mcp-config.md).

### 8.1 · Two of the row's three claims are wrong. One is right.

[13-cyrup-mcp-STATUS.md:593](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) says there is "no stable
`tracing` target". There is: `tracing`'s default target **is the module path**, so all **65** call
sites in this crate (`tracing::debug!` ×22, `info!` ×4, `warn!` ×29, `error!` ×10 — a grep for
`target:` over `crates/cyrup-mcp/src/` returns zero) are already addressable as `cyrup_mcp`,
`cyrup_mcp::oauth`, `cyrup_mcp::lifecycle`, … through `RUST_LOG`. **Adding an explicit
`target: "MCP-UI"` to all 65 would make them strictly less addressable** by flattening that
granularity. Do not do it. The workspace precedent confirms the rule: `cyrup-ext-subagents` uses an
explicit target *only* to carve out a sub-channel, spelled as a module path —
[spawn/mod.rs:961](../../crates/cyrup-ext-subagents/src/spawn/mod.rs)
(`target: "cyrup_ext_subagents::child_stderr"`) and
[tui/fleet_overlay.rs:227](../../crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs)
(`target: "cyrup_ext_subagents::fleet"`).

**The one channel that does need carving out** is §11's second one: config-load warnings that
upstream emits as bare `console.warn`, bypassing the logger entirely. Eight sites, all re-verified:
[config.rs:1862](../../crates/cyrup-mcp/src/config.rs),
[:1876](../../crates/cyrup-mcp/src/config.rs), [:1899](../../crates/cyrup-mcp/src/config.rs),
[:1946](../../crates/cyrup-mcp/src/config.rs) (`push_load_warning`),
[:2331](../../crates/cyrup-mcp/src/config.rs),
[agent_plugin.rs:493](../../crates/cyrup-mcp/src/agent_plugin.rs),
[:524](../../crates/cyrup-mcp/src/agent_plugin.rs),
[:1446](../../crates/cyrup-mcp/src/agent_plugin.rs).

**The level bootstrap is genuinely absent:** zero `MCP_UI_DEBUG` hits in `crates/`. Upstream is
[logger.ts:167-169](../../tmp/pi-mcp-adapter/logger.ts) —
`process.env.MCP_UI_DEBUG === "1" || process.env.MCP_UI_DEBUG === "true"` ⇒
`logger.setLevel("debug")`. Exactly two accepted spellings.

**What "unfiltered" means here, stated rather than silently changed.** Upstream's `console.warn`
ignores the logger's `minLevel` ([logger.ts:66](../../tmp/pi-mcp-adapter/logger.ts) is skipped
entirely for those sites). `tracing` has no bypass, and writing to stderr directly would corrupt the
TUI. It does not matter: `MCP_UI_DEBUG` only ever *raises* verbosity, so the two channels can never
disagree in the direction upstream cares about, and a user who sets `RUST_LOG=cyrup_mcp=error` has
asked to suppress warnings. Distinctness is delivered by the target.

**Dropped with a reason, per the spec:** the pluggable handler list
([logger.ts:53-59](../../tmp/pi-mcp-adapter/logger.ts),
[:92-98](../../tmp/pi-mcp-adapter/logger.ts)). No analogue, no consumer, and the `try {} catch {}`
that swallowed handler errors has nothing to swallow.

### 8.2 · New module `crates/cyrup-mcp/src/log.rs`

`log` is safe as a module name here: it is **not** a dependency of `cyrup-mcp`
([Cargo.toml](../../crates/cyrup-mcp/Cargo.toml) has no `log` edge), so it is not in the extern
prelude and `crate::log` cannot be ambiguous. Register as `pub mod log;` in
[lib.rs:132-151](../../crates/cyrup-mcp/src/lib.rs) and add a row to the Cut 2 module-map table.

```rust
//! `logger.ts` — the log channel, as `tracing` targets plus one env bootstrap (MCP-090).
//!
//! # Three deliberate rulings
//!
//! 1. **No explicit `target:` on the crate's own 65 call sites.** `tracing`'s default target IS the
//!    module path, so every site is already addressable as `cyrup_mcp`, `cyrup_mcp::oauth`,
//!    `cyrup_mcp::lifecycle`, … through `RUST_LOG`. Stamping one flat `"MCP-UI"` target across them
//!    would DESTROY that granularity, not add it. Upstream's `[MCP-UI…]` prefix (`logger.ts:33-38`)
//!    is the package's historical name and carries no scope meaning (13b §11).
//! 2. **One explicit target, for the second channel.** Config-load warnings do not go through
//!    upstream's logger at all — they are bare `console.warn` in `config.ts` and
//!    `agent-plugin-loader.ts`, unfiltered diagnostics that predate the logger. `tracing` has no
//!    bypass and writing to stderr directly would corrupt the TUI, so the channels are separated by
//!    TARGET instead, spelled as a module path so it composes with `RUST_LOG` — the same shape
//!    `cyrup_ext_subagents::child_stderr` already uses. This costs nothing in the direction
//!    upstream cares about: [`UI_DEBUG_DIRECTIVE`] only ever RAISES verbosity.
//! 3. **The pluggable handler list is dropped** (`logger.ts:53-59`, `:92-98`). No analogue, no
//!    production consumer, and the `try {} catch {}` that swallowed handler errors has nothing to
//!    swallow. Stated, not silent.

/// `logger.ts:167` — the module bootstrap variable. Not pi-branded, so it is preserved verbatim
/// (13b §16). MCP-068 owns the env-override family and must consume this constant rather than
/// spelling the name a second time.
pub const UI_DEBUG_ENV_VAR: &str = "MCP_UI_DEBUG";

/// The unfiltered config-load channel — `config.ts`'s and `agent-plugin-loader.ts`'s bare
/// `console.warn` sites, kept distinct from the level-gated logger channel.
pub const CONFIG_LOAD_TARGET: &str = "cyrup_mcp::config_load";

/// The `EnvFilter` directive `MCP_UI_DEBUG` adds. It is ADDITIVE — layered on top of whatever
/// `RUST_LOG` asked for — and it never lowers a floor.
pub const UI_DEBUG_DIRECTIVE: &str = "cyrup_mcp=debug";

/// `logger.ts:167-169` — `MCP_UI_DEBUG === "1" || === "true"` ⇒ raise this crate's floor to `debug`.
#[must_use]
pub fn ui_debug_enabled() -> bool {
    ui_debug_from(std::env::var(UI_DEBUG_ENV_VAR).ok().as_deref())
}

/// The predicate alone, so it is decidable without touching process env — edition 2024 makes
/// `std::env::set_var` `unsafe`, which is why MCP-082 splits `interpolate_env_vars` the same way.
///
/// Exactly two accepted spellings. `"0"`, `"TRUE"`, `""` and an unset variable are all `false`.
#[must_use]
pub fn ui_debug_from(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true"))
}
```

### 8.3 · Give the eight config-load sites their target

At each of the eight sites named above:

```rust
tracing::warn!(target: crate::log::CONFIG_LOAD_TARGET, "{message}");
```

`tracing`'s macros take `target: $target:expr` and a `const &'static str` is const-evaluable in the
callsite `static`, so the path form compiles. Note
[agent_plugin.rs:493](../../crates/cyrup-mcp/src/agent_plugin.rs) is a multi-argument `warn!` and
[:524](../../crates/cyrup-mcp/src/agent_plugin.rs) is a formatted literal — the `target:` prefix goes
first in both, ahead of the format string.

### 8.4 · Make `MCP_UI_DEBUG` observable

In `init_tracing` at [crates/cyrup/src/bootstrap.rs:279-289](../../crates/cyrup/src/bootstrap.rs)
(**not** `main.rs` — [main.rs:172](../../crates/cyrup/src/main.rs) only calls it). `cyrup` already
depends on `cyrup-mcp` ([crates/cyrup/Cargo.toml:82](../../crates/cyrup/Cargo.toml)) and on
`tracing-subscriber` with `env-filter` ([:105](../../crates/cyrup/Cargo.toml)), which is what
supplies `tracing_subscriber::filter::Directive`.

```rust
/// Initialise `tracing` to **stderr**, honouring `RUST_LOG`. Off by default; `--verbose` raises the
/// floor to `debug`. Idempotent and never fatal.
pub fn init_tracing(verbose: bool) {
    use tracing_subscriber::{EnvFilter, filter::Directive, fmt};
    let default = if verbose { "debug" } else { "warn" };
    let mut filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    // `logger.ts:167-169`'s module bootstrap (MCP-090): `MCP_UI_DEBUG=1|true` raises ONLY the MCP
    // adapter's floor, layered on top of whatever `RUST_LOG` asked for. It never lowers one.
    if cyrup_mcp::log::ui_debug_enabled()
        && let Ok(directive) = cyrup_mcp::log::UI_DEBUG_DIRECTIVE.parse::<Directive>()
    {
        filter = filter.add_directive(directive);
    }
    let _ = fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
}
```

---

## 9 · Reference outputs

Computed against the implementations above, including the lexicographic property order of §0.10.
These are the shapes to eyeball when the code first runs; write any input literal in a **different**
key order from the output so the ordering divergence stays visible.

**`format_schema(schema, "  ")`** over
`{"type":"object","required":["mode"],"properties":{"target":{"anyOf":[{"type":"string"},{"type":"number","minimum":0}],"description":"where"},"mode":{"const":null},"kind":{"enum":["a",1]},"tags":{"type":"array","items":{"type":"string","maxLength":8}}}}`:

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

**`render_ts_shape`** — MCP-098's case,
`{"$ref":"#/$defs/A","$defs":{"A":{"$ref":"#/$defs/B"},"B":{"type":"string"}}}`. `B` is referenced
only from *inside* `A`, so it is registered during the emission loop and must still be emitted:

```text
type A = B;
type B = string;

A
```

**`render_ts_shape`** — the alias-plus-object case,
`{"type":"object","properties":{"b":{"type":"string"},"a":{"$ref":"#/$defs/Thing"}},"required":["a"],"$defs":{"Thing":{"type":"object","properties":{"n":{"type":"integer"}}}}}`:

```text
type Thing = { n?: number; };

{ a: Thing; b?: string; }
```

**`render_ts_shape`** — the remaining rules:

| input | output |
|---|---|
| `{"type":"array","items":{"anyOf":[{"type":"string"},{"type":"number"}]}}` | `(string \| number)[]` |
| `{"type":"object","additionalProperties":false,"properties":{}}` | `{}` |
| `{"type":"object","additionalProperties":{"type":"string"}}` | `None` |
| `{"type":"object","properties":null}` | `None` — present, non-object |
| `{"anyOf":"x","oneOf":[{"type":"string"}]}` | `None` — non-array `anyOf` wins the selection |
| any of `if`/`then`/`else`/`allOf`/`not`/`patternProperties` at **any** node | `None` |
| `{"type":"object"}` | `{}` — the value both discovery assertions now expect |
| `{"const":1.0}` | `1`, not `1.0` |

**`format_schema`** — the truthiness rules:

| property schema | rendered part |
|---|---|
| `{"description":""}` | no `- ` part at all |
| `{"type":""}` or `{"type":0}` | falls to rule 5/6; no empty `()` |
| `{"format":null}` | `[format: null]` — presence, not truthiness |
| `"not an object"` | `  name` alone, or `  name *required*` |

**`register_ajv_formats`** — the predicates:

| format | accepts | rejects |
|---|---|---|
| `url` | `http://127.0.0.1:8931/mcp`, `https://x.dev/a` | `mailto:a@b.c`, `x.dev`, `data:,` |
| `byte` | `""`, `aGk=`, `"!!\naGk="` (the `m` flag) | `"aG"`, `"!!!"` |
| `iso-time` | `12:00:00`, `12:00:00.5+01:00`, `23:59:60Z` | `24:00:00`, `12:00:00+24:00` |
| `iso-date-time` | `2020-02-29T12:00:00` | `2021-02-29T12:00:00`, `2020-01-01T00:00:00T` |
| `json-pointer-uri-fragment` | `#`, `#/a~0b` | `/a`, `#a` |
| `duration` (draft-07) | `P1Y2M3DT4H5M6S`, `PT5M`, `P1W` | `P`, `PT`, `P2W3D` |
| `uuid` (draft-07) | a bare UUID, and one with the `urn:uuid:` prefix | a 31-hex string |

---

## 10 · Definition of Done

Checkable by reading the tree. `cargo check --workspace --all-targets`,
`cargo doc --workspace --no-deps --bins` and `cargo nextest run --workspace` must each still exit 0
with no new failures (baseline: 7862 passing).

**MCP-091 + MCP-098**

- [ ] `crates/cyrup-mcp/src/ts_shape.rs` exists, is `pub mod ts_shape;` in `lib.rs`'s alphabetical block, and has a row in a `lib.rs` module-map table.
- [ ] The emission loop is an index-based `while index < aliases.map.len() { … }` over an `IndexMap` grown by `render` **inside** the loop. No `for (k, v) in &aliases`, no pre-collected `Vec`, no `BTreeMap`. The MCP-098 case emits **both** `type A = B;` and `type B = string;`.
- [ ] The `anyOf`/`oneOf` branch guards on "either is an array" and selects `anyOf ?? oneOf`, so a non-array `anyOf` beside an array `oneOf` yields `None`.
- [ ] A present, non-object `properties` yields `None`; an absent one yields `{}`.
- [ ] `$ref` resolution decodes `~1`→`/` before `~0`→`~` on the **token only**, `$defs` keys are stored raw, and the module doc records that as divergence 1 with its reason.
- [ ] `const: null` renders `null` via `contains_key`, distinct from an absent `const`.
- [ ] Each of `if`, `then`, `else`, `allOf`, `not`, `patternProperties` returns `None` at a **nested** node, not only the root; `additionalProperties: false` returns a shape and `additionalProperties: {…}` returns `None`.
- [ ] A schema nested past `MAX_RENDER_DEPTH` returns `None` rather than exhausting the stack.
- [ ] `js_number` folds an integral float, so `{"const":1.0}` renders `1`.

**MCP-211**

- [ ] `format_schema` and its four helpers live in `proxy/tool_metadata.rs` immediately after `find_tool_by_name`, and `format_property` / `format_nested_schema` / `format_variants` return `Vec<String>`.
- [ ] `append_schema_annotations` skips an **empty** description, emits the eight keys in the order `minLength, maxLength, minimum, maximum, minItems, maxItems, format, pattern` on key **presence** (an explicit `null` renders), then `default`.
- [ ] `format_type` rule 1 uses `contains_key("const")`; rule 4 is gated on JS truthiness, so `{"type":""}` and `{"type":0}` fall through.
- [ ] `format_property` joins its parts with exactly one space and recurses at `indent + "  "`; `format_variants` nests at `indent + "    "`.
- [ ] A non-object property schema takes the early return: one line, no `(type)` part, no annotations.
- [ ] The `format_schema` reference output in §9 is reproduced byte for byte.

**Wiring**

- [ ] `FakeEnv::format_schema` and `FakeEnv::render_ts_shape` in `proxy/testsupport.rs` delegate to the real functions; both constant stubs are gone.
- [ ] `discovery.rs:907` expects `"\nShape:\n{}"` and `discovery.rs:1047` expects `"\n  Shape:\n    {}"`; both tests are otherwise unmodified and pass.
- [ ] The `Parameters:` fork at `discovery.rs:359` is still reached for a schema `render_ts_shape` declines.

**MCP-093**

- [ ] `crates/cyrup-mcp/src/schema_validator.rs` is the **only** validator module in the crate, and its header states that MCP-092 extends this file rather than adding another.
- [ ] `draft_2020_options()` and `draft_07_options()` both set `should_validate_formats(true)` and leave `should_ignore_unknown_formats` at its default.
- [ ] `url`, `byte`, `iso-time`, `iso-date-time` and `json-pointer-uri-fragment` are registered on both builders; `duration` and `uuid` **only** on draft-07, and the doc says why registering them on 2020-12 would shadow a better built-in.
- [ ] `float`, `double`, `password`, `binary`, `int32` and `int64` are **not** registered, and the doc records both reasons (ajv asserts nothing / `with_format` is unreachable for numeric instances) so the absence cannot be "fixed" back into dead code.
- [ ] `DURATION` contains no lookaround and rejects `P`, `PT` and `P2W3D`; `UUID` accepts the `urn:uuid:` prefix; `is_ajv_byte` accepts a conforming line inside a wrapped payload.
- [ ] `use jsonschema::{Draft, ValidationOptions};` — no `referencing` edge — and no dependency was added to any `Cargo.toml`.

**MCP-085**

- [ ] `crate::ui::format_terminal_error` exists beside `sanitize_terminal_text`, applies it to the `Display` rendering, and adds **no** `source()` walk and **no** cycle guard.
- [ ] Both `TODO(MCP-235)` comments (`lifecycle.rs:1328`, `:1413`) are deleted and both sites route their error text through it.
- [ ] The `Failed to connect to "{server}": {message}` text at `proxy/auth.rs:355` and `proxy/call.rs:624` is **unchanged**.
- [ ] The "**Residual:**" sentence at `errors.rs:58-61` now points at the new function.

**MCP-089**

- [ ] `McpError::code()`, `recovery_hint()` and `context()` exist, each an exhaustive match over all thirteen variants with **no `_` arm**.
- [ ] `McpError::Server`'s `#[error]` template is still `"{server}: {message}"`; `code()` is `"MCP_SERVER_ERROR"` and `recovery_hint()` is upstream's `"Check that the MCP server is running and responsive."` byte for byte.
- [ ] `recovery_hint()` returns `None` for `McpError::Other` and `Some` for the other twelve.
- [ ] `context()` omits absent keys entirely rather than emitting `null`.
- [ ] No `ConsentError`, `CONSENT_DENIED` or `CONSENT_REQUIRED` anywhere in the crate.
- [ ] `code()`'s doc names `crate::proxy::McpErrorCode` as the other, different vocabulary.
- [ ] The `errors.rs` header no longer lists MCP-089 as pending, spells `recovery_hint`'s return type as `Option<&'static str>`, and records `ConsentError` as cut with `consent-manager.ts`.

**MCP-079**

- [ ] `ApprovalOrigin::parse` returns `None` for `"script"`, `"iframe"` and every other string; `parse(as_str(x)) == Some(x)` for all three arms.
- [ ] `ToolApprovalDecision { AllowOnce, AllowForSession, Deny }` exists with `as_str`, a `parse` returning `None` for `"abstain"`, and `from_dialog_label`, every match exhaustive with no `_` arm on the enum side.
- [ ] `ensure_tool_call_approved`'s tail dispatches on `ToolApprovalDecision` with identical behaviour: `AllowOnce` approves without caching, `AllowForSession` approves and inserts the argument-keyed entry, everything else denies. `allow_for_session_caches_per_argument_payload` and `allow_once_approves_without_caching` pass unmodified.
- [ ] No serde derive on `ApprovalOrigin` or `ToolApprovalDecision`, and `ApprovalOrigin::parse`'s doc records that `as_str` has no production caller.
- [ ] `ToolApprovalDecision` is re-exported from `lib.rs:181`.

**MCP-090**

- [ ] `crates/cyrup-mcp/src/log.rs` exists, is declared in `lib.rs`, and defines `UI_DEBUG_ENV_VAR`, `CONFIG_LOAD_TARGET`, `UI_DEBUG_DIRECTIVE`, `ui_debug_enabled` and `ui_debug_from`.
- [ ] `ui_debug_from` is `true` for exactly `Some("1")` and `Some("true")`, `false` for `None`, `Some("0")`, `Some("TRUE")` and `Some("")`.
- [ ] All eight config-load `warn` sites carry `target: crate::log::CONFIG_LOAD_TARGET`; the crate's other 57 `tracing::*!` sites carry **no** explicit target.
- [ ] `init_tracing` in `crates/cyrup/src/bootstrap.rs` adds `UI_DEBUG_DIRECTIVE` when `ui_debug_enabled()`, additively on top of `RUST_LOG`, never lowering a floor.
- [ ] The module doc states that the pluggable handler list is dropped and why the two channels are separated by target rather than by bypassing the filter.
