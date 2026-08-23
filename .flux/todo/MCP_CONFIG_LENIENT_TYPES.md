---
stage: aug
status: done
updated: 2026-08-22 15:12
---

# Decide The Config Type Model — Replace `lenient` With `Lenient<T>`

## The decision

**cyrup adopts upstream's model: the parse is total and lossless, and every type judgement moves to
the read site. `lenient`'s "degrade to `None`" survives as a *view*, and is deleted as a *storage*
rule.**

Concretely: a config field stops being `Option<T>` behind
[`deserialize_with = "lenient"`](../../crates/cyrup-mcp/src/config.rs) (`config.rs:479-486`) and
becomes `Lenient<T>` — one value that always keeps the raw JSON and additionally offers the typed
view when the raw fits. Absent stays distinguishable from present-but-unusable, which is exactly the
distinction upstream has (`undefined` vs. a value of the wrong type) and the one `lenient` destroys.

This is **not** a new rule. It is [`config.rs`](../../crates/cyrup-mcp/src/config.rs)'s own rule 1 —
"Defaults are enforced at the read site, never at parse" (`config.rs:34-40`) — applied to types as
well as to defaults, and rule 3 — "Unknown keys must round-trip" (`config.rs:48-53`) — extended from
unknown *keys* to unusable *values*. Rule 4 (`config.rs:55-62`) currently states the mechanism
(`lenient` → `None`) where it should state the requirement ("a malformed config must never `Err`");
`Lenient<T>` satisfies the requirement without the collateral damage, and rule 4's last sentence
(`config.rs:60`) is the one line of the header that changes.

The rejected alternative — keep typed reader fields and let `lenient` drop what does not fit — is
rejected because it is what is deployed, and it is measurably wrong on both sides of the tree in
opposite directions. See the evidence below.

## Why this side, from the measurements

**1. Upstream never validates a config field, so a type judgement at parse has no upstream
counterpart.** `validateConfig` is a bare cast, `toServerEntries` checks only "non-array object", and
`mergeServerMaps` spreads objects (`config.ts:476-518`). Every judgement that exists happens inside a
resolver at a read site: `resolveServerUrl` throws on a non-string `url` (`utils.ts:167-171`),
`interpolateEnvRecord` throws when `value.startsWith` is called on a non-string member
(`utils.ts:107-114`), `resolveSearchKeywords` skips only the offending key and only the offending
element (`search-ranking.ts:43`, `:46`). A parse that removes the value removes the input those
judgements are defined over. (Upstream working copy: `tmp/pi-mcp-adapter` @ `v2.26.1`, `fafae21` —
gitignored, so cited by `file:line` rather than linked.)

**2. The writer is wrong.** `computeServerHash({command:"x",env:"abc"})` = `01ed7340…` upstream; the
writer produces `f0211144…`, which is upstream's digest for the same definition with `env` **absent**
(`13-cyrup-mcp-STATUS.md:281-287`). `env: []`, `env: 5`, `env: true` hash as `{}` upstream
(`1d224401…`, already pinned at `mcp_direct_tools.rs:2300`) and as absent here. The mechanism is
visible in the source: `interpolateEnvRecord` is `if (!values) return undefined;` then
`Object.entries(values)` (`utils.ts:107-114`), and `Object.entries("abc")` is
`{"0":"a","1":"b","2":"c"}` while `Object.entries(5)`, `Object.entries(true)` and `Object.entries([])`
are all `{}`. `lenient` collapses six distinct upstream answers into one.

**3. The reader is worse, and in the other direction.**
[`extract_server_map`](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs)
(`mcp_direct_tools.rs:506-517`) keeps an entry only when `serde_json::from_value::<ServerEntry>`
succeeds, and that struct's fields are bare `Option<T>` with no lenient equivalent
(`mcp_direct_tools.rs:229-276`) — `env: Option<BTreeMap<String, Value>>` (`:239`) rejects a string,
array, number or bool outright. **Measured over six definitions, the reader keeps three where
upstream keeps six** (`13-cyrup-mcp-STATUS.md:289-295`). `args: [1,"b"]` and `command: 5` behave
identically, which is the proof that this is one root cause and not three items.

**4. The two crates therefore answer opposite questions about one cache entry.** That is the failure
mode already named for `env` at `config.rs:513-518` and closed there by `StringRecord` — for one
field, by hand. `StringRecord` is the correct design applied once. The decision is to make it the
design rather than the exception.

**5. One consequence the record missed: `lenient` silently loses merge precedence.** Upstream's merge
is `{ ...baseEntry, ...definition }` (`config.ts:515`) — a key **present** in the higher-precedence
source wins whatever its type. In [`merge_entry`](../../crates/cyrup-mcp/src/config.rs)
(`config.rs:1995-2075`) every field is `over.field.clone().or(base_entry.field)`, so a
present-but-wrong-typed override arrives as `None` and **loses**, leaving the lower-precedence
definition in force. A user-level `"env": 5` silently keeps a project-level `env`. `Lenient::or`
keyed on *presence* restores upstream's spread. Upstream's URL-bound credential strip is separately
keyed on `typeof definition.url === "string"` (`config.ts:506`), so that guard wants the *typed*
view — which is what makes carrying both views mandatory rather than merely convenient.

## The type

Add to [`config.rs`](../../crates/cyrup-mcp/src/config.rs), beside `RawJson` (`config.rs:254`) and
`raw_to` (`config.rs:452`):

```rust
/// One config field held the way upstream holds it: the raw JSON always, the typed view when the
/// raw fits it. The replacement for `deserialize_with = "lenient"`, which kept only the second and
/// so could not tell "absent" from "present and unusable".
///
/// Three states, matching upstream's three:
///
/// | file | `raw()` | `get()` | upstream |
/// |---|---|---|---|
/// | key absent | `None` | `None` | `undefined` |
/// | `"command": "x"` | `Some(String("x"))` | `Some("x")` | a value of the declared type |
/// | `"command": 5` | `Some(Number(5))` | `None` | a value the TS type lied about |
///
/// The third row is the one `lenient` could not express, and it is the whole of this change.
#[derive(Debug, Clone)]
pub struct Lenient<T> {
    /// The value exactly as the file wrote it. `None` — and only `None` — means the key was ABSENT.
    raw: Option<RawJson>,
    /// The typed view, or `None` when the raw does not fit `T`. Never an error: rule 4.
    typed: Option<T>,
}

impl<T> Default for Lenient<T> {
    /// What `#[serde(default)]` supplies for an ABSENT key. A manual impl, not a derive, so `T`
    /// need not be `Default` — `AuthMode` and `ToolPrefix` are not.
    fn default() -> Self {
        Self { raw: None, typed: None }
    }
}

impl<T> PartialEq for Lenient<T> {
    /// Over the raw only: `typed` is a pure function of `raw`, so comparing it as well would be
    /// redundant and would force a `T: PartialEq` bound. `RawJson`'s object arm is an `IndexMap`,
    /// whose `PartialEq` is key-set equality rather than order equality, so this stays
    /// insertion-order insensitive exactly as `computeServerHash` is (`stableStringify` sorts keys,
    /// `metadata-cache.ts:353`).
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T> Lenient<T> {
    /// An absent key. What `#[serde(default)]` produces, and what `merge_entry`'s URL-bound
    /// credential strip assigns.
    #[must_use]
    pub const fn absent() -> Self {
        Self { raw: None, typed: None }
    }

    /// The key was not in the file. The `skip_serializing_if` predicate — rule 2, unchanged.
    #[must_use]
    pub const fn is_absent(&self) -> bool {
        self.raw.is_none()
    }

    /// The typed view. `None` for an absent key AND for one whose value does not fit `T`; a read
    /// site that must tell those apart asks [`Self::raw`]. Named `get` rather than `as_ref` so it
    /// does not shadow `AsRef::as_ref`.
    #[must_use]
    pub const fn get(&self) -> Option<&T> {
        self.typed.as_ref()
    }

    /// The value as written. The hasher's input, and the only view that can reproduce upstream's
    /// coercions.
    #[must_use]
    pub const fn raw(&self) -> Option<&RawJson> {
        self.raw.as_ref()
    }

    /// `Option::as_deref`'s counterpart, so `entry.command.as_deref()` and `entry.env.as_deref()`
    /// keep compiling untouched at every read site that already encodes `typeof x === "string"`.
    #[must_use]
    pub fn as_deref(&self) -> Option<&T::Target>
    where
        T: std::ops::Deref,
    {
        self.typed.as_deref()
    }

    /// `{ ...base, ...over }` for one field (`config.ts:515`): the override wins whenever the key is
    /// PRESENT, whatever its type. `Option::or` over the typed view — which is what `merge_entry`
    /// did — made a present-but-unusable override lose.
    #[must_use]
    pub fn or(self, base: Self) -> Self {
        if self.is_absent() { base } else { self }
    }

    /// A value this crate constructed rather than parsed — fixtures, `write_direct_tools_config`'s
    /// materialised import entry, and the `From`/`FromIterator` impls on [`StringRecord`].
    #[must_use]
    pub fn present(value: T) -> Self
    where
        T: Serialize,
    {
        Self { raw: Some(raw_from(&value)), typed: Some(value) }
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Lenient<T> {
    /// The body of the old `lenient`, plus the one line that keeps the evidence. serde calls this
    /// only when the key is PRESENT — `#[serde(default)]` covers the absent case — which is the same
    /// mechanism `present_or_absent` already relies on (`mcp_direct_tools.rs:280-292`).
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawJson::deserialize(deserializer)?;
        let typed = raw_to::<T>(&raw);
        Ok(Self { raw: Some(raw), typed })
    }
}

impl<T> Serialize for Lenient<T> {
    /// The **raw**, so a value this port cannot use survives a write-back instead of being erased —
    /// rule 3, now at field granularity. Note the absence of a `T: Serialize` bound: the typed view
    /// is never the thing written.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.raw {
            Some(raw) => raw.serialize(serializer),
            // Unreachable behind `skip_serializing_if = "Lenient::is_absent"`; `null` is the only
            // honest rendering of "no value" should a caller ever bypass that guard.
            None => serializer.serialize_none(),
        }
    }
}
```

The free function `lenient` (`config.rs:470-486`) is **deleted**, and with it every
`deserialize_with = "lenient"` attribute on [`ServerEntry`](../../crates/cyrup-mcp/src/config.rs)
(`config.rs:771` onward), `McpSettings` and `HttpRequestHeadersCommand` (`config.rs:733-755`). The
attribute set shrinks to `#[serde(default, skip_serializing_if = "Lenient::is_absent")]`.

`config.rs:478`'s standing caveat — "The one thing it cannot preserve is explicit-`null`-vs-absent" —
is resolved as a side effect: explicit `null` is `Some(RawJson::Null)`, absent is `None`. Delete the
caveat and the module-header sentence it points to.

## `StringRecord` becomes `interpolateEnvRecord`'s input handling, whole

[`StringRecord`](../../crates/cyrup-mcp/src/config.rs) (`config.rs:535-542`) already keeps three
views. It loses its own `raw` field — `Lenient` owns that now, so `impl Serialize for StringRecord`
(`config.rs:610-615`) is deleted — and gains the coercion arm its `Deserialize` (`config.rs:617-623`)
currently refuses:

```rust
pub struct StringRecord {
    /// `Object.entries(raw)`'s STRING members — the `Deref` target, and every consumer's view.
    values: BTreeMap<String, String>,
    /// Every member as written, keyed the way `Object.entries` keyed it. The `literalEnv` spread's
    /// input; see `secrets.rs`'s literal arm.
    members: BTreeMap<String, RawJson>,
    /// `Some(upstream's TypeError text)` when a member is not a string — `interpolateEnvRecord`
    /// would have thrown. Unchanged.
    unhashable: Option<String>,
    /// The raw was JS-**falsy**, so `interpolateEnvRecord` returned `undefined` and the key hashes
    /// as `undefined` even though it is PRESENT in the file (`utils.ts:108`).
    falsy: bool,
}

impl<'de> Deserialize<'de> for StringRecord {
    /// ANY JSON, because `interpolateEnvRecord` receives any JSON: `validateConfig` never checks
    /// this block and `Record<string, string>` is a TypeScript type, not a runtime check.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_raw(&RawJson::deserialize(deserializer)?))
    }
}

impl StringRecord {
    /// `interpolateEnvRecord`'s input handling (`utils.ts:107-114`), whole:
    ///
    /// * `if (!values) return undefined` — JS falsy is `null`, `false`, `0`/`-0`, `""`. `NaN`
    ///   cannot arrive from `JSON.parse`.
    /// * otherwise `Object.entries(values)`, which enumerates an object's own keys in insertion
    ///   order, an array's indices as `"0"`, `"1"`, …, a **string**'s indices likewise, and nothing
    ///   at all for a number or a boolean.
    fn from_raw(raw: &RawJson) -> Self {
        let members: BTreeMap<String, RawJson> = match raw {
            RawJson::Null | RawJson::Bool(false) => return Self::falsy(),
            RawJson::Number(n) if n.as_f64() == Some(0.0) => return Self::falsy(),
            RawJson::String(s) if s.is_empty() => return Self::falsy(),
            RawJson::Object(entries) => {
                entries.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
            RawJson::Array(items) => items
                .iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v.clone()))
                .collect(),
            // `Object.entries("abc")` === `{"0":"a","1":"b","2":"c"}`, which is upstream's
            // `01ed7340…` for `{command:"x",env:"abc"}`.
            RawJson::String(s) => s
                .chars()
                .enumerate()
                .map(|(i, c)| (i.to_string(), RawJson::String(c.to_string())))
                .collect(),
            // No own enumerable properties.
            RawJson::Bool(true) | RawJson::Number(_) => BTreeMap::new(),
        };
        Self::split(members)
    }

    /// The hashed view: the string members, or `None` when the raw was falsy and
    /// `interpolateEnvRecord` returned `undefined`.
    #[must_use]
    pub fn hashed(&self) -> Option<&BTreeMap<String, String>> {
        (!self.falsy).then_some(&self.values)
    }

    /// Every member as written — the `literalEnv` spread's input.
    #[must_use]
    pub fn members(&self) -> &BTreeMap<String, RawJson> {
        &self.members
    }
}
```

`split` is today's `from_raw` body (`config.rs:548-567`) unchanged: string members into `values`, the
first non-string member's TypeError text into `unhashable`.

**One named micro-delta, in the family of the one already documented at `config.rs:527-533`.**
`Object.entries` on a string enumerates **UTF-16 code units**, so a string containing a non-BMP
scalar yields two lone surrogates where the `chars()` form above yields one key. A lone surrogate is
a valid JS string and does not throw, so upstream hashes it; Rust's `String` cannot hold one. The
`chars()` form is exact for every BMP string. Record it in the type's doc comment beside the existing
insertion-order-vs-key-order delta; do not engineer a UTF-16 pre-image for it.

## Every `ServerEntry` field, under the decision

| field | new type | why |
|---|---|---|
| `command`, `cwd`, `url`, `bearer_token`, `bearer_token_env`, `plugin_data_dir` | `Lenient<String>` | hashed verbatim or resolved; `as_deref()` unchanged at read sites |
| `args`, `include_tools`, `exclude_tools` | `Lenient<Vec<String>>` | `args: [1,"b"]` hashes verbatim upstream |
| `env`, `headers` | `Lenient<StringRecord>` | `StringRecord` now takes any JSON, so `typed` is always `Some` |
| `request_headers_command` | `Lenient<HttpRequestHeadersCommand>` | its own four fields become `Lenient` likewise |
| `auth` | `Lenient<AuthMode>` | see below |
| `protocol_version` | `Lenient<ProtocolVersionSetting>` | see below |
| `search_keywords` | `Lenient<IndexMap<String, RawJson>>` | per-key and per-element skip belong to `resolve_search_keywords`, not to serde |
| `oauth`, `lifecycle`, `idle_timeout`, `request_timeout_ms`, `expose_resources`, `direct_tools`, `tool_prefix`, `approve_tools`, `debug`, `trace`, `http_transport`, `literal_env`, `disabled` | `Lenient<T>`, `T` unchanged | not hashed, but rule 3 applies to them on `write_direct_tools_config`'s import arm (`config.rs:3423-3450`), which materialises a typed entry through `raw_from` and today erases whatever `lenient` dropped |

`McpSettings` takes the same treatment for the same reason. Leaving half the struct on the old
semantics reproduces exactly the "four different answers to one question" this task exists to
prevent.

**`AuthMode::Other` (`config.rs:1498`) and `ProtocolVersionSetting::Other` (`config.rs:1393-1400`)
are deleted.** They exist only to carry a raw value past a typed field, which is now `Lenient`'s job,
and keeping both would be a second mechanism for one thing. Their read sites:

* `config.rs:1421` (`js_string(raw)` in the pre-image) and `config.rs:1452` (the `Serialize` arm) —
  both subsumed by `opt_raw` and `Lenient::serialize` below.
* [`runtime.rs`](../../crates/cyrup-mcp/src/runtime.rs)`:1166`, `version_negotiation`'s
  `Invalid MCP protocolVersion` arm — becomes
  `entry.protocol_version.raw().is_some() && entry.protocol_version.get().is_none()`, the same
  predicate stated without the variant. Update the doc at `runtime.rs:1128` and the fixture
  construction at `runtime.rs:3830` in the same edit.

**Read sites that already encode `typeof x === "string"` need no change at all.**
`ServerEntry::check_exactly_one_transport` (`config.rs:928-939`) is
`[self.command.as_deref(), self.url.as_deref()]`, and upstream is
`.filter(value => typeof value === "string" && value.length > 0)` (`server-manager.ts:465-466`) —
`Lenient::as_deref` yields `None` for a non-string, so the two agree before and after. That is the
measure of how mechanical this migration is: the sites that change are the sites that were wrong.

## The pre-image, rewritten

[`server_identity_pre_image`](../../crates/cyrup-mcp/src/dirs.rs) (`dirs.rs:1239-1269`) folds nine of
its fifteen keys verbatim, exactly as `computeServerHash` does (`metadata-cache.ts:82-108`), and must
therefore read the raw rather than the typed view:

```rust
/// A field folded into the pre-image VERBATIM — what upstream does for every identity key it runs
/// no resolver over (`metadata-cache.ts:86-108`).
///
/// `serde_json::to_value` sorts object keys, which is harmless here and must not be "fixed":
/// `stable_stringify` sorts keys at render time because `stableStringify` does
/// (`metadata-cache.ts:353`, `keys.sort()`).
fn opt_raw<T>(field: &Lenient<T>) -> HashValue {
    field.raw().map_or(HashValue::Undefined, |raw| {
        serde_json::to_value(raw).map_or(HashValue::Undefined, HashValue::from_json)
    })
}
```

`("command", opt_string(entry.command.as_deref()))` (`dirs.rs:1241`) becomes
`("command", opt_raw(&entry.command))`, and likewise for `args` (`:1242`), `auth` (`:1257`),
`protocolVersion` (`:1258`), `bearerTokenEnv` (`:1260`), `exposeResources` (`:1261-1264`),
`includeTools` (`:1265`) and `excludeTools` (`:1266`). `opt_string_list` (`dirs.rs:1302`) and
`opt_serde` (`dirs.rs:1340`) lose their `entry.*` callers; `opt_string` keeps its `resolved.*` ones.
The `socket` key stays `HashValue::Undefined` (`dirs.rs:1250`) — `to_server_entries` rejects any
entry configuring one (`config.rs:1856-1869`), so the field cannot exist to be raw.

## `ResolvedIdentity::resolve` and the five throws

[`ResolvedIdentity::resolve`](../../crates/cyrup-mcp/src/dirs.rs) (`dirs.rs:997-1035`) reads the raw
for the resolved fields, because the resolvers are where upstream's throws live:

```rust
env: interpolate_env_record(entry.env.get(), env)?,                       // signature unchanged
headers: interpolate_env_record(entry.headers.get(), env)?,
cwd: match entry.cwd.raw() {
    None => None,                                                        // absent
    Some(RawJson::String(raw)) => Some(resolve_config_path(raw, env, home)),
    // `resolveConfigPath` returns early only for literal `undefined` (`utils.ts:188`); anything
    // else reaches `interpolateEnvVars`, which is `value.replace(...)` (`utils.ts:74-78`).
    Some(RawJson::Null) => return Err(McpError::Config(
        "Cannot read properties of null (reading 'replace')".to_string())),
    Some(_) => return Err(McpError::Config("value.replace is not a function".to_string())),
},
url: match entry.url.raw() {
    // `if (definition.url == null) return undefined` — loose equality, so BOTH null and absent
    // (`utils.ts:168`).
    None | Some(RawJson::Null) => None,
    Some(RawJson::String(raw)) => crate::credentials::resolve_server_url(Some(raw), env)?,
    Some(_) => return Err(McpError::Config("MCP server URL must be a string".to_string())),
},
```

and [`interpolate_env_record`](../../crates/cyrup-mcp/src/dirs.rs) (`dirs.rs:1052-1064`) gains the
falsy arm, checked first because upstream checks `!values` first:

```rust
fn interpolate_env_record(
    values: Option<&StringRecord>,
    env: &EnvFn,
) -> McpResult<Option<BTreeMap<String, String>>> {
    if let Some(message) = values.and_then(StringRecord::unhashable) {
        return Err(McpError::Config(message.to_string()));
    }
    Ok(crate::secrets::interpolate_env_record(
        values.and_then(StringRecord::hashed),   // was `.map(StringRecord::values)`
        env,
    ))
}
```

`try_compute_server_hash`'s doc (`dirs.rs:1078-1085`) says the `Err` has "**two**" sources. Under the
decision it has **five**, and the count is the specification:

| # | source | upstream | throws on |
|---|---|---|---|
| 1 | `resolve_server_url` | `utils.ts:167-185` | non-string `url`; unset variable in it; unparseable after interpolation |
| 2 | `resolve_config_path` on `cwd` | `utils.ts:187-196` → `:74` | any non-string `cwd`, `null` included |
| 3 | `interpolate_env_record` on `env` / `headers` / `requestHeadersCommand.env` | `utils.ts:107-114` | a non-string member |
| 4 | `resolve_bearer_token` | `utils.ts:198-202` | a present non-string `bearerToken` (the guard is `!== undefined`, so `null` throws too) |
| 5 | `requestHeadersCommand.command` / `.args` | `metadata-cache.ts:96-97` | a non-string `command` — **absent included**, since `interpolateEnvVars(undefined)` is `undefined.replace`; and any non-string `args` element |

Sources 2, 4 and 5 are unreachable today only because `lenient` erases their inputs. The decision
adds no error machinery — it un-blinds machinery that already exists. Fix the count in
`dirs.rs:1078-1085` and in
[`registration.rs`](../../crates/cyrup-mcp/src/registration.rs)`:792` and `:865-866`, both of which
say "exactly one" and were already recorded as stale at `13-cyrup-mcp-STATUS.md:296-299`.

## The four sites, closed under one decision

### 1 · Non-object `env` hashes wrong (the writer)

Closed by `StringRecord::from_raw`'s coercion arm plus `opt_raw`. `env: "abc"` coerces to
`{"0":"a","1":"b","2":"c"}` and hashes `01ed7340…`; `env: []` / `5` / `true` coerce to `{}` and hash
`1d224401…`; `env: 0` / `""` / `false` / `null` take the falsy arm and hash `undefined`, which is the
digest the writer currently produces for *all* of them.

The parenthetical at `config.rs:618-621` — "a fifth, separate divergence … recorded in
`13c-mcp-servers.md`'s MCP-144 notes" — is **deleted, not repointed**. It dangles because
`13c-mcp-servers.md:1838-1852` records only that `interpolate_env_record` drops non-string values and
says nothing about a non-object `env`; and after this change there is no residual left to point at.
Deleting it also removes the only place in the tree that calls this "a fifth" divergence.

### 2 · The reader drops whole servers

[`mcp_direct_tools.rs`](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs)'s local
`ServerEntry` (`:229-276`) takes the same treatment. `cyrup-ext-subagents` already depends on
`cyrup-mcp` (`crates/cyrup-ext-subagents/Cargo.toml:128`) and already imports from it
(`mcp_direct_tools.rs:1878`), so **use `cyrup_mcp::config::Lenient` and
`cyrup_mcp::config::StringRecord` directly** rather than porting a second copy. One deserializer, two
crates, is the strongest available form of "reader and writer agree".

Fields become `Lenient<String>` / `Lenient<Vec<String>>` / `Lenient<StringRecord>` /
`Lenient<Value>`; `present_or_absent` (`mcp_direct_tools.rs:290-296`) is deleted — `Lenient` is its
generalisation, and `auth` / `protocol_version` stop being special-cased — and `extract_server_map`
(`:506-517`) becomes `toServerEntries`' own rule:

```rust
fn extract_server_map(value: &Value) -> BTreeMap<String, ServerEntry> {
    let Some(map) = value.as_object() else { return BTreeMap::new() };
    map.iter()
        // `toServerEntries` keeps an entry iff it is a non-array object; nothing else drops one.
        .filter(|(_, def)| def.is_object())
        .filter_map(|(name, def)| {
            serde_json::from_value::<ServerEntry>(def.clone())
                .ok()
                .map(|entry| (name.clone(), entry))
        })
        .collect()
}
```

The `from_value` call cannot fail once every field is `Lenient`; the `filter_map` stays as the
type-level statement that a non-object entry is the only droppable thing. The reader's local
`interpolate_env_record` (`:1239-1262`) and `server_identity_pre_image` fold the raw the same way the
writer's now does, and its local `IdentityError` (`:854`) gains the same five sources.

Six of six restored: `env: "abc"`, `env: 5`, `env: true`, `env: []`, `args: [1,"b"]` and `command: 5`
all keep their server and reach the same digest on both sides.

### 3 · `secrets.rs:386` spawns children with a partial env

[`resolve_stdio_env`](../../crates/cyrup-mcp/src/secrets.rs) (`secrets.rs:380-391`) passes
`entry.env.as_deref()` (`:386`), which `Deref`s to the string members only, so
`env: {"GOOD":"1","BAD":5}` spawns with `GOOD=1`. Upstream does neither that nor the
pre-`StringRecord` behaviour of spawning with none: `resolveEnv` (`server-manager.ts:1231-1243`)
calls `resolveCommandSecretsRecord` (`utils.ts:155-165`) → `resolveCommandSecret`, whose first act is
`value.startsWith("!!")` — so a non-string member **throws and the connect fails**. Route the
existing evidence to the existing error:

```rust
pub fn resolve_stdio_env(
    entry: &ServerEntry,
    server_name: &str,
    base: &HashMap<String, String>,
) -> McpResult<HashMap<String, String>> {
    let literal = entry.literal_env.get() == Some(&true);
    // `resolveCommandSecretsRecord` reaches `value.startsWith` on every member
    // (`utils.ts:155-165`), so one non-string member fails the connect rather than shrinking the
    // env. Upstream's own TypeError text; unlike the hasher's, this one is user-visible.
    if !literal
        && let Some(message) = entry.env.get().and_then(StringRecord::unhashable)
    {
        return Err(McpError::Config(message.to_string()));
    }
    resolve_env(entry.env.get(), server_name, literal, base)
}
```

The `literal_env` guard is not a hedge — it is upstream's asymmetry.
`if (literalEnv) return env ? { ...resolved, ...env } : resolved` (`server-manager.ts:1236`) **skips
the resolver entirely**, so no member is ever `startsWith`-ed and nothing throws; the members land in
the child environment stringified by `spawn`. So the literal arm of `resolve_env`
(`secrets.rs:352-372`) must spread `StringRecord::members()` through `js_string` — which already
exists at `config.rs:1428-1443` and is exactly JS `String(x)` — rather than the string members alone.
Make `js_string` `pub(crate)` and reuse it; do not write a second one.

[`resolve_http_secrets`](../../crates/cyrup-mcp/src/secrets.rs) (`secrets.rs:456-521`) takes the
identical guard for `entry.headers` before its step-3 `resolve_command_secrets_record`
(`secrets.rs:470`). There is no `literalEnv` for headers, so it is unconditional.

### 4 · MCP-174 — `search_keywords` drops the whole key

Upstream (`search-ranking.ts:31-54`) rejects the whole field only for a non-object `searchKeywords`
(`:38`), then skips a non-array value (`:43`) and a non-string element (`:46`) individually. With
`search_keywords: Lenient<IndexMap<String, RawJson>>` the field-level rejection and upstream's `:38`
coincide, and the two `continue`s move into
[`resolve_search_keywords`](../../crates/cyrup-mcp/src/proxy.rs) (`proxy.rs:1039-1068`) where rule 1
puts them:

```rust
let Some(map) = definition.and_then(|entry| entry.search_keywords.get()) else {
    return Vec::new();                                               // search-ranking.ts:38
};
...
for (pattern, values) in map {
    let RawJson::Array(items) = values else { continue };            // search-ranking.ts:43
    if !matches_tool_pattern(&candidates, Some(std::slice::from_ref(pattern))) {
        continue;                                                    // search-ranking.ts:44
    }
    for value in items {
        let RawJson::String(text) = value else { continue };         // search-ranking.ts:46
        let trimmed = text.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;                                                // search-ranking.ts:48
        }
        keywords.push(trimmed.to_string());
    }
}
```

`{"a":["x"],"b":"nope"}` now yields `["x"]` for a tool matching `a`, where today the whole field is
`None` and `a` yields nothing.

`proxy.rs:1027-1032` reports **two** divergences forced by the field's type. One of them is already
fixed and the comment is stale: it says `Option<BTreeMap<String, Vec<String>>>`, but the field has
been `Option<IndexMap<String, Vec<String>>>` since the ordering fix, with the reasoning written out
at `config.rs:855-865`. Delete both bullets in the same edit — the first because it is untrue, the
second because this change closes it.

## Bookkeeping the decision settles

**The fifth/sixth numbering.** Canonical, per `13-cyrup-mcp-STATUS.md:83-89`: `auth: null` in
`mcp_direct_tools` is the **fifth** divergence; the non-object `env` family is the **sixth**. The only
place in the *tree* that disagrees is `config.rs:620` ("a fifth, separate divergence"), and §1 above
deletes that whole parenthetical, which settles it at the source. In the docs the conflict is
recorded rather than committed — `13-cyrup-mcp-STATUS.md:287-288` names it explicitly — so closing
the family means striking that ledger bullet and its siblings, not restating the numbering somewhere
new.

**The residual ledger.** `13-cyrup-mcp-STATUS.md:276-299` holds four "Still open" bullets, three of
which are this task (non-object `env`, reader-vs-writer, `secrets.rs:386`) and the fourth of which is
the stale `registration.rs` "exactly one" count that the five-throws table corrects. All four close
together. `MCP-174` (`13-cyrup-mcp-STATUS.md:690`) moves from **partial** to **implemented**, and its
cell's `config.rs:715` citation is wrong in the current text — `search_keywords` is at
`config.rs:865`; `config.rs:715` is inside `HttpRequestHeadersCommand`'s doc comment.

## What the original task got wrong

* `search_keywords` is at `config.rs:865`, not `config.rs:715`. The bad line number came from
  `13-cyrup-mcp-STATUS.md:690` and was carried into the task verbatim.
* It treats "per-key skip" as upstream's uniform semantics. Upstream has three different behaviours —
  verbatim into the hash (`command`, `args`, `auth`, …), **throw** (`env`, `headers`, `cwd`, `url`,
  `bearerToken`), and per-key skip (`searchKeywords`) — with one invariant behind them: the parse
  never judges, the read site always does. Prescribing "per-key skip" everywhere would be a fifth
  wrong answer.
* It misses that `lenient` also loses **merge precedence** (`config.rs:1995-2075` vs
  `config.ts:515`): a present-but-wrong-typed override silently loses to a lower-precedence source.
* It misses the `literalEnv` half of site 3. `literalEnv: true` skips the resolver upstream
  (`server-manager.ts:1236`), so that arm must spread every member stringified — neither throw nor
  drop.
* Its acceptance criteria call for the decision to be "written into `13b-mcp-config.md`". This
  crate's house style is that comments carry the specification (`13-cyrup-mcp-STATUS.md:293`), and
  the model is already stated in exactly one place — `config.rs`'s module-header rule 4
  (`config.rs:55-62`). That sentence is what changes; a parallel statement in a gap-analysis doc
  would be the second source of truth this task exists to prevent.

## Definition of done

* `lenient` (`config.rs:470-486`) no longer exists; no `deserialize_with = "lenient"` attribute
  remains in the workspace.
* `Lenient<T>` is the type of every field of `ServerEntry`, `McpSettings` and
  `HttpRequestHeadersCommand` in `cyrup-mcp`, and of every field of `mcp_direct_tools.rs`'s
  `ServerEntry` and `RequestHeadersCommand` — the same type, imported, not a second copy.
* `config.rs:55-62` (rule 4) states the requirement and the new mechanism; `config.rs:478`'s
  explicit-`null`-vs-absent caveat is gone because it is no longer true.
* The nine verbatim identity keys reach `stable_stringify` through `opt_raw`; `env`, `headers`,
  `cwd`, `url`, `bearerToken` and `requestHeadersCommand` reach it through their resolvers, and each
  of the five throw sources returns `Err`.
* `extract_server_map` (`mcp_direct_tools.rs:506`) drops an entry for exactly one reason: it is not a
  non-array object.
* `resolve_stdio_env` (`secrets.rs:380`) and `resolve_http_secrets` (`secrets.rs:456`) fail the
  connect on a non-string member instead of spawning a shortened env; the `literalEnv` arm spreads
  every member through `js_string`.
* `resolve_search_keywords` (`proxy.rs:1039`) skips per key and per element; `proxy.rs:1027-1032`'s
  two-divergence note is deleted.
* `config.rs:618-621`'s parenthetical is deleted; `AuthMode::Other` and
  `ProtocolVersionSetting::Other` are deleted with their read sites updated (`runtime.rs:1128`,
  `:1166`, `:3830`).
* `dirs.rs:1078-1085`, `registration.rs:792` and `registration.rs:865-866` say **five** sources.
* `13-cyrup-mcp-STATUS.md:276-299`'s four "Still open" bullets are struck, and `MCP-174` (`:690`)
  reads **implemented** with the corrected `config.rs:865` citation.
* The existing differential table `reader_writer_and_upstream_agree_across_the_edge_cases`
  (`mcp_direct_tools.rs:2295`) — the one place reader, writer and upstream digests meet — still
  passes, extended with the six definitions from §2 against the digests already recorded at
  `13-cyrup-mcp-STATUS.md:281-287`.
