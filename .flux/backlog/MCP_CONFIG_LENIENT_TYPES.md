---
stage: aug
status: done
updated: 2026-08-27 06:00
---

# Replace `lenient` With `Lenient<T>` — Keep The Raw, Judge At The Read Site

## Objective

`crates/cyrup-mcp/src/config.rs`'s free function
[`lenient`](../../crates/cyrup-mcp/src/config.rs) (`config.rs:469-486`) reads a field into a
`RawJson`, tries the typed shape, and **throws the raw away** when it does not fit. Seventy-two
fields carry `#[serde(default, deserialize_with = "lenient", …)]`. Every one of them therefore
answers `None` to two different questions — *"the key was absent"* and *"the key was there and I
could not use it"* — and upstream answers those two questions differently in eleven measured places.

Replace it with a type that keeps both answers:

```rust
pub struct Lenient<T> { raw: Option<RawJson>, typed: Option<T> }
```

`raw()` is what `computeServerHash` folds, what a write-back must round-trip, and what upstream's
resolvers are defined over. `get()` is the typed view every existing read site already wants. Absent
is `raw().is_none()`, and nothing else.

This is not a new rule. It is `config.rs`'s own **rule 1** — *"Defaults are enforced at the read
site, never at parse"* (`config.rs:34-40`) — applied to types as well as defaults, and **rule 3** —
*"Unknown keys must round-trip"* (`config.rs:48-53`) — moved from unknown *keys* down to unusable
*values*. **Rule 4** (`config.rs:55-61`) states its mechanism (`lenient` → `None`) where it should
state its requirement (*a malformed config must never `Err`*); `Lenient<T>` meets the requirement
without the collateral damage, and `config.rs:60-61` is the pair of lines in the header that change.

---

## Corrections to the previous augmentation pass

Read these first — three of them invalidate prescriptions the previous revision of this file made.

### 1 · `cyrup-ext-subagents` **cannot** import `cyrup_mcp::config::Lenient`

The previous pass prescribed *"use `cyrup_mcp::config::Lenient` and `cyrup_mcp::config::StringRecord`
directly rather than porting a second copy"*, citing `crates/cyrup-ext-subagents/Cargo.toml:128`.

`cyrup-mcp` is a **`[dev-dependencies]`** entry of that crate
([`Cargo.toml:142`](../../crates/cyrup-ext-subagents/Cargo.toml)), under a comment that says why in
so many words:

> They share an on-disk file rather than a type — on purpose, because resolving a subagent's `mcp:`
> selectors must not drag the whole MCP adapter (rmcp, reqwest, oauth2) into a spawn …
> Dev-only: `[dependencies]` above is unchanged, so no production layering is introduced.

`cyrup-mcp` has no `[features]` table and depends unconditionally on rmcp, reqwest, `cyrup-provider`
and `cyrup-ext`'s `wasm-host`. Importing the type would put all of that on every subagent spawn.
`crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:1838-1841` states the same contract from the
other side.

**The prescription is the opposite: mirror the type locally**, exactly as that module already mirrors
`ServerEntry`, `interpolate_env_record`, `interpolate_env_vars`, `server_identity_pre_image`,
`stable_stringify` and `hex_sha256`. The dev-dependency conformance tests
(`mcp_direct_tools.rs:1857`, `:2264`) are the mechanism that holds the two copies together, and they
already exist.

### 2 · `resolve_search_keywords` is not in `proxy.rs`

`crates/cyrup-mcp/src/proxy.rs` does not exist. The module is a directory. The function is
[`crates/cyrup-mcp/src/proxy/ranking.rs:283-314`](../../crates/cyrup-mcp/src/proxy/ranking.rs), and
its stale two-divergence note is `ranking.rs:273-278`. Every `proxy.rs:10xx` citation in the previous
revision is dead.

### 3 · Throw source 5 is reachable **today**, not only after this change

The previous pass claimed *"Sources 2, 4 and 5 are unreachable today only because `lenient` erases
their inputs."* True for 2 and 4. **False for 5.** `requestHeadersCommand: { "timeoutMs": 2500 }`
parses today into `HttpRequestHeadersCommand { command: None, … }`, and
[`ResolvedIdentity::resolve`](../../crates/cyrup-mcp/src/dirs.rs) (`dirs.rs:1017-1021`) maps that to
`command: None`, hashing `"command":undefined` inside the nested object. Upstream **throws**:
[`metadata-cache.ts:96`](../../tmp/pi-mcp-adapter/metadata-cache.ts) is
`interpolateEnvVars(definition.requestHeadersCommand.command)` with no guard, and `interpolateEnvVars`
is `value.replace(…)` ([`utils.ts:74-78`](../../tmp/pi-mcp-adapter/utils.ts)). Measured:
`THROW Cannot read properties of undefined (reading 'replace')`. A live digest divergence, not a
latent one.

### 4 · Citation fixes carried into the code below

| previous | actual |
|---|---|
| `mcp_direct_tools.rs:1878` (cyrup_mcp import) | `mcp_direct_tools.rs:1847-1850`, inside `#[cfg(test)]` |
| `mcp_direct_tools.rs:2295` / `:2300` (differential table) | `mcp_direct_tools.rs:2263` / `:2269` |
| `runtime.rs:3830` (`ProtocolVersionSetting::Other` fixture) | `runtime.rs:3837` — **and `runtime.rs:3204`, which the previous pass missed** |
| `ProtocolVersionSetting::Other` at `config.rs:1393-1400` | enum at `config.rs:1393-1405`, the `Other` variant at `config.rs:1404` |
| `write_direct_tools_config`'s import arm at `config.rs:3423-3450` | `config.rs:3450-3455` |
| four "Still open" bullets at `13-cyrup-mcp-STATUS.md:276-299` | `:281-309`; `:276` is the heading and `:310` starts a fifth, unrelated bullet |
| rule 4 at `config.rs:55-62` | `config.rs:55-61` |
| `check_exactly_one_transport` at `config.rs:928-939` | `config.rs:927-943` |
| `ResolvedIdentity::resolve` at `dirs.rs:997-1035` | `dirs.rs:997-1032` |
| `resolve_stdio_env` at `secrets.rs:380-391` | `secrets.rs:379-390`; the offending read at `secrets.rs:386` is correct |
| `stableStringify`'s `keys.sort()` at `metadata-cache.ts:353` | correct |

What the previous pass got **right** and this one keeps: the `Lenient<T>` shape itself, the
`StringRecord` coercion arm, the `env` digests, the merge-precedence finding, the `literalEnv`
asymmetry, the five-throw count, and the observation that `13-cyrup-mcp-STATUS.md:690`'s `MCP-174`
row cites `config.rs:715` — a blank line inside `HttpRequestHeadersCommand`'s doc — when
`search_keywords` is at `config.rs:865`.

---

## The measurement

Every number below was produced by transcribing `computeServerHash` and its five resolvers verbatim
out of [`metadata-cache.ts:82-112`](../../tmp/pi-mcp-adapter/metadata-cache.ts),
[`metadata-cache.ts:344-354`](../../tmp/pi-mcp-adapter/metadata-cache.ts) and
[`utils.ts:74-203`](../../tmp/pi-mcp-adapter/utils.ts) onto node 22, run against the checkout at
`v2.26.1` (`fafae21`) with `HOME=/home/u` and no other variable set. The transcription reproduces all
four digests already pinned at `mcp_direct_tools.rs:2266-2292`, which is what makes it trustworthy.

**Digests — every one of these is a value `lenient` currently erases.**

| definition | upstream digest | port today |
|---|---|---|
| `{"command":"x"}` | `f0211144c2b2b29b578deb59dd0edc37fe366b5a902d0cf65d3b12d1a56bbda5` | ✓ |
| `{"command":"x","env":{}}` | `1d224401e4ab9a3e11e3490649da48a3fd946b49869464320b97b423c7f2893b` | ✓ |
| `{"command":"x","env":"abc"}` | `01ed73400a8a8e5c123703b3fcfb537bcccf8a98176aea772da6d81023c7bea7` | `f0211144…` |
| `{"command":"x","env":["a","b"]}` | `2e4600309d847c366291efcad4c6e793bc0c3ba552827cf43af6de9828c72b82` | `f0211144…` |
| `{"command":"x","env":[]}` / `5` / `true` | `1d224401…` (all three) | `f0211144…` |
| `{"command":"x","env":0}` / `""` / `false` / `null` | `f0211144…` (all four) | ✓ |
| `{"command":5}` | `c486aafd3166299a278cca7ff8a54398b35d7d88c5b9a0713efee264cd1c39f5` | folds `command:undefined` |
| `{"command":"x","args":[1,"b"]}` | `dcb187bbe79a6b4ca1ba2a693880b5370a41827c47db0d3071d8a8bd0b5212b1` | `f0211144…` |
| `{"command":"x","args":"ab"}` | `8270485a82f019ba65f1c6b7049ab311b40bf35082913c722b4bc1c1a5c5158e` | `f0211144…` |
| `{"command":"x","args":null}` | `6ed03e3d4b6050bb88c0cefef771ae57b9e558999ffe2aeb3991f81615b960da` | `f0211144…` |
| `{"command":"x","headers":"ab"}` | `a11157a0b6fb5597558093ccba45bb81df6e1c48c88323dfb903b23fe770b78d` | `f0211144…` |
| `{"command":"x","includeTools":5}` | `6819b3078703ccf29e75c68b8e07f3ede7cf4e125830dd905dcaa02484a13672` | `f0211144…` |
| `{"command":"x","exposeResources":"yes"}` | `bb3c42a0e14d54534ce28e0fb36cc57fefea70ccc6698f05bfe0dc6e917ec752` | `f0211144…` |
| `{"command":"x","bearerTokenEnv":5}` | `08952f8a37dce520e31414ad98ff5a8a4223983b1df5f96365445f2fae988ac7` | `f0211144…` |
| `{"command":"x","auth":"basic"}` | `1148aefe78b3dd0db4eb0233167a5d6d9f2d075c7e9b4ee429a447ab18c902ff` | ✓ (via `AuthMode::Other`) |
| `{"command":"x","requestHeadersCommand":{"command":"n"}}` | `b4a8f003044e89d5801467027c3a3af0cefb62613cf55960ac8be25d95d1198d` | ✓ |
| `…{"command":"n","args":null}` | `b4a8f003…` — identical: `?.map` on `null` is `undefined` | ✓ |
| `…{"command":"n","timeoutMs":"5"}` | `dab6fff7c8bbbded767f498fe7e9a960820218bc78c382c56c543f5e586b646c` | `b4a8f003…` |
| `…{"command":"n","env":"ab"}` | `2753a02dcd90b099946789fa4cac9a616b1cef305f9948d1e2e4622be2da3112` | `b4a8f003…` |
| `…{"command":"n","env":0}` | `b4a8f003…` — falsy, so `undefined` | ✓ |

**Throws.** `computeServerHash` is wrapped in a `try` at `metadata-cache.ts:114`
(`isServerCacheValid`), so each of these means *"this entry is never cache-valid"*.

| definition | upstream |
|---|---|
| `cwd: 5` (any present non-string) | `value.replace is not a function` |
| `cwd: null` | `Cannot read properties of null (reading 'replace')` |
| `url: 5` | `MCP server URL must be a string` |
| `url: null` | **no throw** — `definition.url == null` is loose (`utils.ts:168`), so `null` and absent give the same `undefined` |
| `bearerToken: 5` | `value.startsWith is not a function` |
| `bearerToken: null` | `Cannot read properties of null (reading 'startsWith')` |
| `env` / `headers` / `requestHeadersCommand.env` with a non-string **member** | `value.startsWith is not a function` (`… of null …` when the member is `null`) |
| `requestHeadersCommand: {}` / `"sign.sh"` / `[]` | `Cannot read properties of undefined (reading 'replace')` |
| `requestHeadersCommand: {command: 5}` | `value.replace is not a function` |
| `requestHeadersCommand: {command:"n", args: 5}` | `definition.requestHeadersCommand.args?.map is not a function` |
| `requestHeadersCommand: {command:"n", args: [1]}` | `value.replace is not a function` |
| `requestHeadersCommand: null` | **no throw** — falsy, so the whole key is `undefined` |

**One measurement that names a delta this port must not silently reproduce.** With `TOK=zzz`,
`{"command":"x","bearerTokenEnv":["TOK"]}` resolves `bearerToken` to `"zzz"`: `resolveBearerToken` is
`definition.bearerTokenEnv ? process.env[definition.bearerTokenEnv] : undefined`
(`utils.ts:198-202`), and JS coerces the property key with `String(x)`, so `["TOK"]` becomes `"TOK"`.
See §7.

**One accepted micro-delta.** `{"command":"x","env":"a😀b"}` hashes over
`{"0":"a","1":"\ud83d","2":"\ude00","3":"b"}` — `Object.entries` on a string enumerates **UTF-16 code
units**, and a non-BMP scalar yields two lone surrogates. Rust's `String` cannot hold one. The
`chars()` form prescribed in §3 is exact for every BMP string and yields one key where upstream
yields two for an astral scalar. Record it in `StringRecord`'s doc beside the existing
insertion-order-vs-key-order delta at `config.rs:527-533`; do **not** engineer a UTF-16 pre-image.

---

## 1 · The type — `crates/cyrup-mcp/src/config.rs`

Add beside [`RawJson`](../../crates/cyrup-mcp/src/config.rs) (`config.rs:254`) and `raw_to`
(`config.rs:452`) / `raw_from` (`config.rs:463`). Delete `lenient` (`config.rs:469-486`) in the same
edit.

```rust
/// One config field held the way upstream holds it: the raw JSON always, and the typed view when
/// the raw fits it.
///
/// The replacement for `deserialize_with = "lenient"`, which kept only the second view and so could
/// not tell "absent" from "present and unusable". Upstream has both — `undefined` versus a value the
/// TypeScript type lied about — and answers them differently in every resolver
/// (`utils.ts:107-203`), in `computeServerHash` (`metadata-cache.ts:86-108`), in `mergeServerMaps`
/// (`config.ts:515`) and in the two `!== undefined` presence tests (`direct-tools.ts:143`, and
/// `approveTools`).
///
/// | file | [`Self::raw`] | [`Self::get`] | upstream |
/// |---|---|---|---|
/// | key absent | `None` | `None` | `undefined` |
/// | `"command": "x"` | `Some(String("x"))` | `Some("x")` | a value of the declared type |
/// | `"command": 5` | `Some(Number(5))` | `None` | a value the TS type lied about |
///
/// The third row is the whole of this type, and the whole of this change.
#[derive(Debug, Clone)]
pub struct Lenient<T> {
    /// The value exactly as the file wrote it. `None` — and only `None` — means the key was ABSENT.
    /// An explicit `null` is `Some(RawJson::Null)`, which is the distinction the standing caveat at
    /// the old `config.rs:477-478` said could not be preserved.
    raw: Option<RawJson>,
    /// The typed view, or `None` when the raw does not fit `T`. Never an error: rule 4.
    typed: Option<T>,
}

impl<T> Default for Lenient<T> {
    /// What `#[serde(default)]` supplies for an ABSENT key. Written by hand, not derived, so `T`
    /// need not be `Default` — `AuthMode`, `ToolPrefix` and `ProtocolVersionSetting` are not.
    fn default() -> Self {
        Self { raw: None, typed: None }
    }
}

impl<T> PartialEq for Lenient<T> {
    /// Over the raw only. `typed` is a pure function of `raw`, so comparing it too would be
    /// redundant and would force a `T: PartialEq` bound this struct does not need.
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T> Lenient<T> {
    /// An absent key — what `#[serde(default)]` produces, and what [`merge_entry`]'s URL-bound
    /// credential strip assigns.
    #[must_use]
    pub const fn absent() -> Self {
        Self { raw: None, typed: None }
    }

    /// The key was not in the file. The `skip_serializing_if` predicate — rule 2, unchanged — and
    /// the `!== undefined` half of `direct-tools.ts:143`.
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

    /// The value as written — the hasher's input, and the only view that can reproduce upstream's
    /// coercions.
    #[must_use]
    pub const fn raw(&self) -> Option<&RawJson> {
        self.raw.as_ref()
    }

    /// `Option::as_deref`'s counterpart, so `entry.command.as_deref()`, `entry.env.as_deref()` and
    /// `config.args.as_deref()` keep compiling untouched at every read site that already encodes
    /// `typeof x === "string"`.
    #[must_use]
    pub fn as_deref(&self) -> Option<&T::Target>
    where
        T: std::ops::Deref,
    {
        self.typed.as_deref()
    }

    /// `{ ...base, ...over }` for one field (`config.ts:515`): the override wins whenever the key is
    /// PRESENT, whatever its type. `Option::or` over the typed view — which is what [`merge_entry`]
    /// did — made a present-but-unusable override LOSE to a lower-precedence source.
    #[must_use]
    pub fn or(self, base: Self) -> Self {
        if self.is_absent() { base } else { self }
    }

    /// A value this crate constructed rather than parsed — fixtures, [`crate::agent_plugin`]'s
    /// manifest entries, `write_direct_tools_config`'s materialised import entry, and the
    /// `From`/`FromIterator` impls on [`StringRecord`].
    #[must_use]
    pub fn present(value: T) -> Self
    where
        T: Serialize,
    {
        Self { raw: Some(raw_from(&value)), typed: Some(value) }
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Lenient<T> {
    /// The body of the deleted `lenient`, plus the one line that keeps the evidence. serde calls
    /// this only when the key is PRESENT — `#[serde(default)]` covers the absent case, which is the
    /// same mechanism `mcp_direct_tools`'s `present_or_absent` already relies on.
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

Then, in the same file:

* Delete the standing caveat at `config.rs:477-478` (*"The one thing it cannot preserve is
  explicit-`null`-vs-absent; see the module header"*) and the module-header sentence it points at —
  `Lenient` preserves it.
* Rewrite `config.rs:60-61` so **rule 4 states the requirement, not the mechanism**: a malformed
  config must never `Err`; `Lenient<T>` meets that by keeping the raw and offering the typed view
  only when the raw fits, so a wrong-typed field no longer costs the file *or* the value.
* Delete the "One real gap remains" paragraph at `config.rs:27-30` — §6 closes it.
* Every attribute becomes `#[serde(default, skip_serializing_if = "Lenient::is_absent")]`.

---

## 2 · The JS-truthiness helper

Three upstream guards are the same falsy test and must not be written three times:
`if (!values) return undefined` (`utils.ts:108`, `utils.ts:159`),
`definition.bearerTokenEnv ? … : undefined` (`utils.ts:201`), and
`env ? { ...resolved, ...env } : resolved` (`server-manager.ts:1236`). Add one, next to `js_string`
(`config.rs:1428-1443`):

```rust
/// JS truthiness for a parsed JSON value. The falsy set reachable from `JSON.parse` is `null`,
/// `false`, `0`/`-0` and `""`; `NaN` and `undefined` cannot arrive. Every upstream `!value` /
/// `value ? …` guard over a config field is this predicate.
#[must_use]
pub(crate) fn is_js_truthy(value: &RawJson) -> bool {
    match value {
        RawJson::Null | RawJson::Bool(false) => false,
        RawJson::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        RawJson::String(text) => !text.is_empty(),
        RawJson::Bool(true) | RawJson::Array(_) | RawJson::Object(_) => true,
    }
}
```

Make `js_string` (`config.rs:1428`) `pub(crate)` in the same edit — §5, §7 and §8 all need it, and a
second hand-rolled `String(x)` is exactly the drift this crate's house style exists to prevent.

---

## 3 · `StringRecord` becomes `interpolateEnvRecord`'s input handling, whole

[`StringRecord`](../../crates/cyrup-mcp/src/config.rs) (`config.rs:535-542`) already keeps three
views. It **loses its own `raw` field** — `Lenient` owns the raw now, so `impl Serialize for
StringRecord` (`config.rs:610-615`) is deleted — and **gains the coercion arm** its `Deserialize`
(`config.rs:617-623`) currently refuses. Today's `from_raw` body (`config.rs:548-567`) survives
unchanged under the name `split`.

```rust
pub struct StringRecord {
    /// `Object.entries(raw)`'s STRING members — the [`std::ops::Deref`] target, and every
    /// consumer's view.
    values: BTreeMap<String, String>,
    /// Every member as written, keyed the way `Object.entries` keyed it. The `literalEnv` spread's
    /// input (`server-manager.ts:1236`); see [`crate::secrets::resolve_env`].
    members: BTreeMap<String, RawJson>,
    /// `Some(upstream's TypeError text)` when a member is not a string — `interpolateEnvRecord`
    /// would have thrown on it (`utils.ts:110-112`). Unchanged.
    unhashable: Option<String>,
    /// The raw was JS-**falsy**, so `interpolateEnvRecord` returned `undefined` and the key hashes
    /// as `undefined` even though it is PRESENT in the file (`utils.ts:108`).
    falsy: bool,
}

impl<'de> Deserialize<'de> for StringRecord {
    /// ANY JSON, because `interpolateEnvRecord` receives any JSON: `validateConfig` never inspects
    /// this block (`config.ts:640-650`) and `Record<string, string>` is a TypeScript type, not a
    /// runtime check. Total, so `Lenient<StringRecord>::get()` is always `Some` for a present key —
    /// the raw is still what `Lenient` hands the hasher.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_raw(&RawJson::deserialize(deserializer)?))
    }
}

impl StringRecord {
    /// `interpolateEnvRecord`'s input handling (`utils.ts:107-114`), whole:
    ///
    /// * `if (!values) return undefined` — see [`is_js_truthy`].
    /// * otherwise `Object.entries(values)`, which enumerates an object's own keys in insertion
    ///   order, an array's indices as `"0"`, `"1"`, …, a **string**'s indices likewise, and nothing
    ///   at all for a number or a boolean.
    ///
    /// Measured on node 22 @ v2.26.1: `{command:"x",env:"abc"}` hashes `01ed7340…` over
    /// `{"0":"a","1":"b","2":"c"}`; `env:[]`, `env:5` and `env:true` all hash `1d224401…` over `{}`;
    /// `env:0`, `env:""`, `env:false` and `env:null` hash `f0211144…`, i.e. `"env":undefined`.
    ///
    /// The string arm enumerates `chars()` where `Object.entries` enumerates UTF-16 code units —
    /// exact for every BMP string, one key short for an astral scalar, and Rust's `String` cannot
    /// hold the lone surrogate that would make it exact. Named, not engineered around.
    fn from_raw(raw: &RawJson) -> Self {
        if !is_js_truthy(raw) {
            return Self {
                values: BTreeMap::new(),
                members: BTreeMap::new(),
                unhashable: None,
                falsy: true,
            };
        }
        let members: BTreeMap<String, RawJson> = match raw {
            RawJson::Object(entries) => {
                entries.iter().map(|(key, value)| (key.clone(), value.clone())).collect()
            }
            RawJson::Array(items) => items
                .iter()
                .enumerate()
                .map(|(index, value)| (index.to_string(), value.clone()))
                .collect(),
            RawJson::String(text) => text
                .chars()
                .enumerate()
                .map(|(index, ch)| (index.to_string(), RawJson::String(ch.to_string())))
                .collect(),
            // No own enumerable properties. `is_js_truthy` has already taken `false`, `0` and `""`.
            RawJson::Bool(_) | RawJson::Number(_) | RawJson::Null => BTreeMap::new(),
        };
        Self::split(members)
    }

    /// The hashed view: the string members, or `None` when the raw was falsy and
    /// `interpolateEnvRecord` returned `undefined`.
    #[must_use]
    pub fn hashed(&self) -> Option<&BTreeMap<String, String>> {
        (!self.falsy).then_some(&self.values)
    }

    /// Every member as written — the `literalEnv` spread's input, and the only view that can
    /// reproduce a non-string member reaching a child process.
    #[must_use]
    pub fn members(&self) -> &BTreeMap<String, RawJson> {
        &self.members
    }
}
```

`Deref`, `values()`, `unhashable()`, `From<BTreeMap<String, String>>` and `FromIterator` all stay; the
two constructors set `members` from the strings and `falsy: false`.

**Delete the parenthetical at `config.rs:618-621`** (*"a fifth, separate divergence … recorded in
`13c-mcp-servers.md`'s MCP-144 notes"*). It dangles — `13c-mcp-servers.md:1838-1852` records only
that `interpolate_env_record` drops non-string values — and after this change there is no residual
left to point at. Deleting it also removes the only place in the tree that calls the non-object-`env`
family "a fifth" divergence, which `13-cyrup-mcp-STATUS.md:83-89` calls the sixth.

---

## 4 · Every field, under the decision

### `ServerEntry` (`config.rs:771-905`)

| field | new type | why |
|---|---|---|
| `command`, `cwd`, `url`, `bearer_token`, `bearer_token_env`, `plugin_data_dir` | `Lenient<String>` | hashed verbatim or resolved; `as_deref()` unchanged at read sites |
| `args`, `include_tools`, `exclude_tools` | `Lenient<Vec<String>>` | `args:[1,"b"]` → `dcb187bb…`, `args:"ab"` → `8270485a…`, `includeTools:5` → `6819b307…`, all folded **verbatim** with no coercion |
| `env`, `headers` | `Lenient<StringRecord>` | `StringRecord` now takes any JSON, so `typed` is always `Some`; `raw()` is what the resolver and the `literalEnv` spread need |
| `request_headers_command` | `Lenient<HttpRequestHeadersCommand>` | its own four fields become `Lenient` likewise — see §6 |
| `auth` | `Lenient<AuthMode>` | `AuthMode::Other` is deleted — see §8 |
| `protocol_version` | `Lenient<ProtocolVersionSetting>` | `ProtocolVersionSetting::Other` is deleted — see §8 |
| `search_keywords` | `Lenient<IndexMap<String, RawJson>>` | field-level rejection then coincides **exactly** with `search-ranking.ts:38`; the per-key and per-element skips move to the read site — see §9 |
| `expose_resources`, `debug`, `trace`, `literal_env`, `disabled` | `Lenient<bool>` | `exposeResources:"yes"` → `bb3c42a0…`, folded verbatim |
| `oauth`, `lifecycle`, `idle_timeout`, `request_timeout_ms`, `direct_tools`, `tool_prefix`, `approve_tools`, `http_transport` | `Lenient<T>`, `T` unchanged | not hashed, but rule 3 applies on `write_direct_tools_config`'s import arm (`config.rs:3450-3455`), which materialises a typed entry through `raw_from` and today erases whatever `lenient` dropped; `direct_tools` and `approve_tools` additionally need presence — see §10 |

`merge_entry` (`config.rs:1994-2075`): every `field.clone().or(base_entry.field)` now resolves to
`Lenient::or`, which keys on presence — `{ ...baseEntry, ...definition }` (`config.ts:515`) — instead
of on the typed view. The five credential-strip assignments at `config.rs:2003-2011` become
`Lenient::absent()`. The URL guard at `config.rs:1998-2000` becomes

```rust
if base.is_some()
    && let Some(next_url) = over.url.get()
    && base_entry.url.get() != Some(next_url)
```

which is upstream's `typeof definition.url === "string" && definition.url !== existing.url`
(`config.ts:506`) stated exactly: `get()` is `None` for a non-string on both sides, and
`None != Some(x)`. `base_entry.oauth != Some(OAuthSetting::Disabled(false))` becomes
`base_entry.oauth.get() != Some(&OAuthSetting::Disabled(false))` — `baseEntry.oauth !== false`
(`config.ts:511`). The destructure at `config.rs:2013-2043` keeps its missing `..` rest pattern; that
exhaustiveness is the only thing guaranteeing a new credential-bearing field cannot bypass the strip.

`check_exactly_one_transport` (`config.rs:927-943`) needs **no change**: it is
`[self.command.as_deref(), self.url.as_deref()]`, upstream is
`.filter(value => typeof value === "string" && value.length > 0)` (`server-manager.ts:465-466`), and
`Lenient::as_deref` yields `None` for a non-string. That is the measure of how mechanical this
migration is — the sites that change are the sites that were wrong.

### `McpSettings` (`config.rs:953-1050`)

Same treatment, mechanically. **No `McpSettings` read site changes behaviour**: every accessor is
already a `typeof` / `===` / `!==` value test (`config.rs:1100-1230`), which is what upstream does
over an unvalidated bare cast, so `lenient`'s `None` and `Lenient::get()`'s `None` land on the same
branch. It converts for one reason, and that reason is checkable: the free function `lenient` is the
thing that cannot express present-but-unusable, and leaving one struct on it keeps two parse models
alive in one module. The gain is rule 3 — `"idleTimeout": "10"` now round-trips instead of vanishing.

### `HttpRequestHeadersCommand` (`config.rs:733-755`)

All four fields become `Lenient<String>` / `Lenient<Vec<String>>` / `Lenient<StringRecord>` /
`Lenient<f64>`. `timeoutMs:"5"` is folded verbatim upstream (`dab6fff7…`), which only `raw()` can do.

---

## 5 · The pre-image — `crates/cyrup-mcp/src/dirs.rs`

[`server_identity_pre_image`](../../crates/cyrup-mcp/src/dirs.rs) (`dirs.rs:1239-1269`) folds eight
of its fifteen keys straight off `entry`, exactly as `computeServerHash` does
(`metadata-cache.ts:87-88`, `:102-103`, `:105-108`), and must therefore read the **raw**. Add beside
`opt_string` (`dirs.rs:1296`):

```rust
/// A [`crate::config::RawJson`] as a `stableStringify` node.
///
/// A direct match rather than `serde_json::to_value` + [`HashValue::from_json`]: `serde_json::Map`
/// is a `BTreeMap` under this workspace's feature set, so the detour would sort object keys on the
/// way through. Harmless in the end — [`stable_stringify`] sorts them at render time because
/// `stableStringify` does (`metadata-cache.ts:353`, `keys.sort()`) — but a detour that is only
/// harmless by coincidence is not one to take.
fn hash_raw(raw: &RawJson) -> HashValue {
    match raw {
        RawJson::Null => HashValue::Null,
        RawJson::Bool(value) => HashValue::Bool(*value),
        RawJson::Number(number) => number.as_f64().map_or(HashValue::Null, HashValue::Number),
        RawJson::String(text) => HashValue::String(text.clone()),
        RawJson::Array(items) => HashValue::Array(items.iter().map(hash_raw).collect()),
        RawJson::Object(entries) => HashValue::Object(
            entries.iter().map(|(key, value)| (key.clone(), hash_raw(value))).collect(),
        ),
    }
}

/// A field folded into the pre-image VERBATIM — what upstream does for every identity key it runs no
/// resolver over. `undefined` for an ABSENT key; the value as written for a present one, whatever
/// its type.
fn opt_raw<T>(field: &Lenient<T>) -> HashValue {
    field.raw().map_or(HashValue::Undefined, hash_raw)
}
```

Rewire, one line each:

| line | from | to |
|---|---|---|
| `dirs.rs:1241` | `opt_string(entry.command.as_deref())` | `opt_raw(&entry.command)` |
| `dirs.rs:1242` | `opt_string_list(entry.args.as_ref())` | `opt_raw(&entry.args)` |
| `dirs.rs:1257` | `opt_serde(entry.auth.as_ref())` | `opt_raw(&entry.auth)` |
| `dirs.rs:1258` | `opt_serde(entry.protocol_version.as_ref())` | `opt_raw(&entry.protocol_version)` |
| `dirs.rs:1260` | `opt_string(entry.bearer_token_env.as_deref())` | `opt_raw(&entry.bearer_token_env)` |
| `dirs.rs:1261-1264` | `entry.expose_resources.map_or(…, HashValue::Bool)` | `opt_raw(&entry.expose_resources)` |
| `dirs.rs:1265` | `opt_string_list(entry.include_tools.as_ref())` | `opt_raw(&entry.include_tools)` |
| `dirs.rs:1266` | `opt_string_list(entry.exclude_tools.as_ref())` | `opt_raw(&entry.exclude_tools)` |

`opt_string_list` (`dirs.rs:1302`) and `opt_serde` (`dirs.rs:1340`) lose every caller and are
deleted. `opt_string` keeps its `resolved.*` callers (`dirs.rs:1244`, `:1251`, `:1259`).
`("socket", HashValue::Undefined)` (`dirs.rs:1250`) is untouched — `to_server_entries` rejects any
entry configuring one (`config.rs:1856-1869`), so the field cannot exist to be raw.

### `ResolvedIdentity` and the five throws

`ResolvedIdentity` currently reuses `HttpRequestHeadersCommand` as its *resolved* carrier
(`dirs.rs:1013-1031`). It cannot once that struct is `Lenient` throughout. Give it its own:

```rust
/// `computeServerHash`'s nested `requestHeadersCommand` object, already resolved
/// (`metadata-cache.ts:94-101`). A separate type from [`crate::config::HttpRequestHeadersCommand`]
/// because the config block is `Lenient` throughout and this one is what survived the resolvers —
/// which is why `command` is a bare `String`: upstream's `interpolateEnvVars` threw unless it was
/// one.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedHeadersCommand {
    /// `interpolateEnvVars(definition.requestHeadersCommand.command)` (`metadata-cache.ts:96`).
    pub command: String,
    /// `definition.requestHeadersCommand.args?.map(interpolateEnvVars)` (`:97`). `None` covers both
    /// ABSENT and explicit `null` — optional chaining collapses them, measured identical
    /// (`b4a8f003…`).
    pub args: Option<Vec<String>>,
    /// `interpolateEnvRecord(definition.requestHeadersCommand.env)` (`:98`).
    pub env: Option<BTreeMap<String, String>>,
    /// `definition.requestHeadersCommand.timeoutMs` (`:99`) — copied through untouched, so a
    /// non-number is folded verbatim (`timeoutMs:"5"` → `dab6fff7…`).
    pub timeout_ms: Option<RawJson>,
}
```

`ResolvedIdentity::resolve` (`dirs.rs:997-1032`) reads the raw wherever upstream's throw lives:

```rust
pub fn resolve(entry: &ServerEntry, env: &EnvFn, home: &Path) -> McpResult<Self> {
    Ok(Self {
        env: interpolate_env_record(entry.env.get(), env)?,
        headers: interpolate_env_record(entry.headers.get(), env)?,
        // `resolveConfigPath` returns early only for literal `undefined` (`utils.ts:188`); anything
        // else reaches `interpolateEnvVars`, which is `value.replace(…)` (`utils.ts:74-78`).
        cwd: match entry.cwd.raw() {
            None => None,
            Some(RawJson::String(raw)) => Some(resolve_config_path(raw, env, home)),
            Some(RawJson::Null) => {
                return Err(McpError::Config(
                    "Cannot read properties of null (reading 'replace')".to_string(),
                ));
            }
            Some(_) => return Err(McpError::Config("value.replace is not a function".to_string())),
        },
        // `if (definition.url == null) return undefined` — LOOSE equality, so `null` and absent are
        // the same answer (`utils.ts:168`). Only a present non-null non-string throws (`:169-171`).
        url: match entry.url.raw() {
            None | Some(RawJson::Null) => None,
            Some(RawJson::String(raw)) => crate::credentials::resolve_server_url(Some(raw), env)?,
            Some(_) => return Err(McpError::Config("MCP server URL must be a string".to_string())),
        },
        bearer_token: resolve_bearer_token_raw(&entry.bearer_token, &entry.bearer_token_env, env)?,
        request_headers_command: resolve_headers_command(&entry.request_headers_command, env)?,
    })
}
```

`interpolate_env_record` (`dirs.rs:1052-1064`) gains the falsy arm, checked first because upstream
checks `!values` first:

```rust
fn interpolate_env_record(
    values: Option<&StringRecord>,
    env: &EnvFn,
) -> McpResult<Option<BTreeMap<String, String>>> {
    if let Some(message) = values.and_then(StringRecord::unhashable) {
        return Err(McpError::Config(message.to_string()));
    }
    // `hashed()`, not `values()`: a JS-falsy block hashes as `undefined` even though the key is
    // present in the file (`utils.ts:108`).
    Ok(crate::secrets::interpolate_env_record(values.and_then(StringRecord::hashed), env))
}
```

`resolve_headers_command` is the fifth throw, and it is the one nothing in this crate expresses today:

```rust
/// `metadata-cache.ts:94-101`. Upstream takes the `?` branch for any JS-**truthy** block and then
/// calls `interpolateEnvVars(block.command)` with no guard, so a block without a string `command` —
/// an ABSENT one included — throws. Measured: `{}`, `"sign.sh"` and `[]` all give
/// `Cannot read properties of undefined (reading 'replace')`; `{command:5}` gives
/// `value.replace is not a function`; `requestHeadersCommand: null` is falsy and gives `undefined`.
fn resolve_headers_command(
    field: &Lenient<HttpRequestHeadersCommand>,
    env: &EnvFn,
) -> McpResult<Option<ResolvedHeadersCommand>> {
    if !field.raw().is_some_and(crate::config::is_js_truthy) {
        return Ok(None);
    }
    // Truthy but not an object at all — `block.command` is `undefined`.
    let Some(block) = field.get() else {
        return Err(McpError::Config(
            "Cannot read properties of undefined (reading 'replace')".to_string(),
        ));
    };
    let command = match block.command.raw() {
        None => {
            return Err(McpError::Config(
                "Cannot read properties of undefined (reading 'replace')".to_string(),
            ));
        }
        Some(RawJson::String(text)) => crate::credentials::interpolate_env_vars(text, env),
        Some(RawJson::Null) => {
            return Err(McpError::Config(
                "Cannot read properties of null (reading 'replace')".to_string(),
            ));
        }
        Some(_) => return Err(McpError::Config("value.replace is not a function".to_string())),
    };
    // `args?.map(…)` — absent and `null` both short-circuit to `undefined`; anything else that is
    // not an array has no `.map`, and a non-string element has no `.replace`.
    let args = match block.args.raw() {
        None | Some(RawJson::Null) => None,
        Some(RawJson::Array(items)) => Some(
            items
                .iter()
                .map(|item| match item {
                    RawJson::String(text) => {
                        Ok(crate::credentials::interpolate_env_vars(text, env))
                    }
                    RawJson::Null => Err(McpError::Config(
                        "Cannot read properties of null (reading 'replace')".to_string(),
                    )),
                    _ => Err(McpError::Config("value.replace is not a function".to_string())),
                })
                .collect::<McpResult<Vec<String>>>()?,
        ),
        Some(_) => {
            return Err(McpError::Config(
                "definition.requestHeadersCommand.args?.map is not a function".to_string(),
            ));
        }
    };
    Ok(Some(ResolvedHeadersCommand {
        command,
        args,
        env: interpolate_env_record(block.env.get(), env)?,
        timeout_ms: block.timeout_ms.raw().cloned(),
    }))
}
```

`opt_request_headers_command` (`dirs.rs:1311-1325`) takes `Option<&ResolvedHeadersCommand>` and
renders `command` through `HashValue::String`, `args` as an array of `HashValue::String` (or
`Undefined`), `env` through `opt_string_map`, and `timeout_ms` through `hash_raw`.

`try_compute_server_hash`'s doc (`dirs.rs:1078-1085`) says the `Err` has "**two**" sources. Under
this change it has **five**, and the count is the specification:

| # | source | upstream | throws on |
|---|---|---|---|
| 1 | `resolve_server_url` | `utils.ts:167-185` | a present non-`null` non-string `url`; an unset variable in it; unparseable after interpolation |
| 2 | `resolve_config_path` on `cwd` | `utils.ts:187-196` → `:74` | any present non-string `cwd`, `null` included |
| 3 | `interpolate_env_record` on `env` / `headers` / `requestHeadersCommand.env` | `utils.ts:107-114` | a non-string **member** |
| 4 | `resolve_bearer_token_raw` | `utils.ts:198-202` | a present non-string `bearerToken` — the guard is `!== undefined`, so `null` throws too |
| 5 | `resolve_headers_command` | `metadata-cache.ts:96-97` | a truthy block whose `command` is not a string (**absent included**); a non-array non-`null` `args`; a non-string `args` element |

Sources 2 and 4 are unreachable today because `lenient` erases their inputs. **Source 5 is reachable
today** and is producing wrong digests — see the corrections above. Fix the count in the doc at
`dirs.rs:1078-1085`, in [`registration.rs:792`](../../crates/cyrup-mcp/src/registration.rs) and in
`registration.rs:865-866`, both of which say "exactly one".

---

## 6 · `requestHeadersCommand` stops failing open

`config.rs:27-30` says *"One real gap remains … a malformed `requestHeadersCommand` fails **open**
here, because [`lenient`] degrades it to `None` and the server then connects unsigned. See plan unit
**MCP-069a**."* `Lenient` supplies the input that gap was missing, and settles both rulings
[`13b-mcp-config.md:1202-1206`](../../docs/gap-analysis/13b-mcp-config.md) says must be made first:
the field's raw *is* the record of the defect, so no `defect: Option<&'static str>` is needed and
rule 4 gets no named exception; and the pre-image folds the resolved value or throws, so ruling 2's
premise — that `stableStringify` emits `"sign.sh"` — is simply wrong. Upstream throws, as §5's source
5 measures.

`runtime.rs:2607-2612` is `match entry.request_headers_command.clone() { Some(config) => … }`. It
becomes:

```rust
let signing_client = if entry.request_headers_command.is_absent() {
    None
} else {
    let config = entry.request_headers_command.get().ok_or_else(|| {
        // `request-headers-command.ts:159-161`. Present-but-not-an-object: upstream refuses the
        // CONNECTION, so the server never exists to send an unsigned request.
        McpError::Config("HTTP request headers command must be an object".to_string())
    })?;
    Some(crate::request_headers_command::RequestHeadersCommandClient::new(/* … */))
};
```

`resolve_request_headers_command` (`request_headers_command.rs:216-254`) then raises the remaining
sentences from [`request-headers-command.ts:162-178`](../../tmp/pi-mcp-adapter/request-headers-command.ts),
each keyed on the raw:

* `config.command.as_deref()` (`request_headers_command.rs:219`) is unchanged — a present non-string
  gives `None`, which the existing trim-empty check turns into *"HTTP request headers command
  requires a non-empty command"* (`:162-164`).
* `config.args.raw().is_some() && config.args.get().is_none()` ⇒ *"HTTP request headers command args
  must be strings"* (`:165-167`).
* `config.env` ⇒ *"HTTP request headers command env values must be strings"* (`:168-174`) when the
  raw is present and is **not** an all-string object. This check reads the raw, **not**
  `StringRecord`: `:169-171` rejects a string and an array outright where `interpolateEnvRecord`
  coerces them, so the validator and the hasher genuinely disagree upstream and must here too.
* `config.timeout_ms.raw().is_some() && config.timeout_ms.get().is_none()` ⇒ *"HTTP request headers
  command timeoutMs must be an integer between 1 and 60000"* (`:175-178`), joining the existing
  finite / integral / range test at `request_headers_command.rs:230-239`.

Rename the crate test `the_two_reachable_configuration_throws_carry_upstreams_sentences` to match its
new arity.

---

## 7 · `crates/cyrup-mcp/src/secrets.rs` — the connect path

### `resolve_stdio_env` (`secrets.rs:379-390`)

`secrets.rs:386` passes `entry.env.as_deref()`, which `Deref`s to the string members only, so
`env: {"GOOD":"1","BAD":5}` spawns the child with `GOOD=1`. Upstream does neither that nor the
pre-`StringRecord` behaviour of spawning with none: `resolveEnv` (`server-manager.ts:1231-1242`)
calls `resolveCommandSecretsRecord` (`utils.ts:155-165`) → `resolveCommandSecret`, whose first act is
`value.startsWith("!!")` — so a non-string member **fails the connect**.

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
    resolve_env(&entry.env, server_name, literal, base)
}
```

The `literal` guard is not a hedge, it is upstream's asymmetry:
`if (literalEnv) return env ? { ...resolved, ...env } : resolved` (`server-manager.ts:1236`) **skips
the resolver entirely**, so no member is ever `startsWith`-ed and nothing throws. `resolve_env`
(`secrets.rs:352-372`) therefore takes `&Lenient<StringRecord>`, and its literal arm
(`secrets.rs:359-364`) spreads `StringRecord::members()` through `js_string` — JS `String(x)`, which
already exists at `config.rs:1428-1443`. Reuse it; do not write a second one. The non-literal arm
takes `.get().and_then(StringRecord::hashed)` and is otherwise unchanged.

### `resolve_http_secrets` (`secrets.rs:456-521`)

The identical guard on `entry.headers`, **unconditional** — there is no `literalEnv` for headers.
Upstream throws one line earlier than the previous pass claimed: `hasCommandHeader` at
`server-manager.ts:840-841` is
`Object.values(definition.headers ?? {}).some(value => value.startsWith("!") …)`, which reaches
`startsWith` before `resolveCommandSecretsRecord` at `:842` does. Place the guard **before** the
step-2 scan at `secrets.rs:461-467`, not before step 3 at `secrets.rs:470`.

`secrets.rs:485`'s `entry.auth == Some(AuthMode::Named(AuthKind::Bearer))` becomes
`entry.auth.get() == Some(&AuthMode::Named(AuthKind::Bearer))`.

### `resolve_bearer_token_raw`

`credentials::resolve_bearer_token` (`credentials.rs:3386-3397`) takes `Option<&str>` twice and
cannot say two things upstream says. Add the raw-taking form in `dirs.rs` and route the hash path
(source 4) and `resolve_http_secrets`' step-5 fallback (`secrets.rs:491-495`) through it:

```rust
/// `resolveBearerToken(definition)` (`utils.ts:198-202`) over the raw fields.
///
/// Two things `Option<&str>` cannot say. `if (definition.bearerToken !== undefined)` is a PRESENCE
/// test, so a present non-string reaches `value.startsWith` and **throws**. And
/// `process.env[definition.bearerTokenEnv]` coerces the property key with `String(x)`, so a
/// non-string name still performs a lookup — measured, `bearerTokenEnv: ["TOK"]` with `TOK=zzz`
/// resolves to `"zzz"`, where a `typeof`-guarded port resolves nothing.
fn resolve_bearer_token_raw(
    token: &Lenient<String>,
    token_env: &Lenient<String>,
    env: &EnvFn,
) -> McpResult<Option<String>> {
    match token.raw() {
        None => {}
        Some(RawJson::String(text)) => {
            return Ok(Some(crate::credentials::interpolate_secret_expression(text, env)));
        }
        Some(RawJson::Null) => {
            return Err(McpError::Config(
                "Cannot read properties of null (reading 'startsWith')".to_string(),
            ));
        }
        Some(_) => return Err(McpError::Config("value.startsWith is not a function".to_string())),
    }
    Ok(token_env
        .raw()
        .filter(|raw| crate::config::is_js_truthy(raw))
        .and_then(|raw| env(&crate::config::js_string(raw))))
}
```

---

## 8 · `AuthMode::Other` and `ProtocolVersionSetting::Other` are deleted

Both variants exist for one job — carry a raw value past a typed field — which is now `Lenient`'s
job. Keeping both is a second mechanism for one thing, and `raw_to::<T>` over an enum with a
catch-all arm always succeeds, so `Lenient` would carry a duplicate of a value the enum already held.

* **`ProtocolVersionSetting::Other(RawJson)`** — `config.rs:1404`. Removing it makes the enum's
  hand-written `Deserialize` (`config.rs:1461-1470`) fallible for an unknown value, which is exactly
  what `Lenient` wants. Delete the `Other` arms at `config.rs:1421` (`as_js_string`) and
  `config.rs:1452` (`Serialize`).
* **`AuthMode::Other(RawJson)`** — `config.rs:1498`. `AuthMode` is `#[serde(untagged)]`, so removing
  the arm makes `"basic"` fail `raw_to` and land in `Lenient::typed = None`. Every consumer already
  compares against a named variant and `Other` matched none of them, so `get()` returning `None` is
  behaviourally identical.

**The hazard:** `oauth.rs:369` and `oauth.rs:3927` are `definition.auth.is_none()`, which today is
`false` for `auth: "basic"` because `Other` carried it. They must become
`definition.auth.is_absent()`, **not** `definition.auth.get().is_none()` — upstream's predicate is
`definition.auth === undefined` (`supports_oauth`'s sixth row, `oauth.rs:342`), and the `.get()`
form would silently turn every unknown `auth` value into implicit OAuth. `oauth.rs:353` and
`oauth.rs:359` become `get()` comparisons; `oauth.rs:362-368`'s `headers` non-empty test becomes
`definition.headers.get().is_some_and(|headers| !headers.is_empty())`.

Read sites, all of which change in the same edit:

| site | today | becomes |
|---|---|---|
| `runtime.rs:1166` | `Some(ProtocolVersionSetting::Other(_)) => Err(…)` | the `None` arm of a `match … .get()`, guarded by `!is_absent()` |
| `runtime.rs:1171` | `.map(ProtocolVersionSetting::as_js_string)` | `entry.protocol_version.raw().map_or_else(String::new, crate::config::js_string)` |
| `runtime.rs:1122-1132` | the doc paragraph naming `Other` | restate over `Lenient`: the deserialiser still validates nothing, the digest still folds the value, the throw still happens at connect |
| `runtime.rs:3204` | `matches!(entry.protocol_version, Some(ProtocolVersionSetting::Other(_)))` | `!entry.protocol_version.is_absent() && entry.protocol_version.get().is_none()` |
| `runtime.rs:3837` | `Some(ProtocolVersionSetting::Other(RawJson::String(…)))` | `serde_json::from_str::<Lenient<ProtocolVersionSetting>>("\"2025-06-18\"")` |
| `runtime.rs:3179` | test doc naming `Other` | restate over `Lenient` |
| `config.rs:5442`, `:5445` | assertions on the two `Other` variants | assert on `raw()` and on `get().is_none()` |
| `mcp_direct_tools.rs:76`, `:2103`, `:2146` | prose naming the two variants | restate over `Lenient` |

`version_negotiation` (`runtime.rs:1151-1176`) in full:

```rust
pub fn version_negotiation(entry: &ServerEntry) -> McpResult<ClientLifecycleMode> {
    Ok(match entry.protocol_version.get() {
        // Byte-identical arms: `undefined` and `"legacy"` both send a plain `initialize`.
        None if entry.protocol_version.is_absent() => ClientLifecycleMode::Initialize,
        Some(ProtocolVersionSetting::Legacy) => ClientLifecycleMode::Initialize,
        Some(ProtocolVersionSetting::Auto) => ClientLifecycleMode::Auto { /* unchanged */ },
        Some(ProtocolVersionSetting::V20260728) => ClientLifecycleMode::Discover { /* unchanged */ },
        // `default:` — present, and not one of the three. Upstream's throw, at the moment upstream
        // throws it (`server-manager.ts:82-95`).
        None => {
            return Err(McpError::Config(invalid_protocol_version_message(
                &entry.protocol_version.raw().map_or_else(String::new, crate::config::js_string),
            )));
        }
    })
}
```

The eight `String(v)` forms pinned at `runtime.rs:3193-3200` are unaffected: `js_string`
(`config.rs:1428-1443`) is unchanged and is what produced them.

---

## 9 · `search_keywords` — MCP-174

Upstream ([`search-ranking.ts:31-54`](../../tmp/pi-mcp-adapter/search-ranking.ts)) rejects the whole
field only for a missing / falsy / non-object / array `searchKeywords` (`:38`), then skips a non-array
value (`:43`) and a non-string element (`:46`) **individually**. With
`search_keywords: Lenient<IndexMap<String, RawJson>>` the field-level rejection and `:38` coincide
exactly — `IndexMap<String, RawJson>` accepts a JSON object and nothing else — and the two
`continue`s move into
[`resolve_search_keywords`](../../crates/cyrup-mcp/src/proxy/ranking.rs) (`ranking.rs:283-314`),
where rule 1 puts them:

```rust
let Some(map) = definition.and_then(|entry| entry.search_keywords.get()) else {
    return Vec::new();                                                // search-ranking.ts:38
};
// … candidates / keywords / seen unchanged (ranking.rs:292-299) …
for (pattern, values) in map {
    // Upstream tests the value's SHAPE before the pattern (`:43` then `:44`); keep that order.
    let RawJson::Array(items) = values else { continue };             // search-ranking.ts:43
    if !matches_tool_pattern(&candidates, Some(std::slice::from_ref(pattern))) {
        continue;                                                     // search-ranking.ts:44
    }
    for value in items {
        let RawJson::String(text) = value else { continue };          // search-ranking.ts:46
        let trimmed = text.trim();
        if trimmed.is_empty() || seen.contains(trimmed) {
            continue;                                                 // search-ranking.ts:48
        }
        seen.insert(trimmed.to_string());
        keywords.push(trimmed.to_string());
    }
}
```

`{"a":["x"],"b":"nope"}` now yields `["x"]` for a tool matching `a`, where today the whole field is
`None` and `a` yields nothing.

**Delete both bullets of the note at `ranking.rs:273-278`.** The first is untrue — it says the field
is `Option<BTreeMap<String, Vec<String>>>`, but it has been `Option<IndexMap<String, Vec<String>>>`
since the ordering fix, with the reasoning written out at `config.rs:855-865`. The second is what
this section closes.

---

## 10 · The two presence tests

`lenient` erased *presence*, and two read sites are defined over it. Both stop compiling under
`Lenient` and both must be re-keyed, not merely re-typed.

**`resolve_tool_filter` (`registration.rs:968-979`)** — upstream is
`if (definition.directTools !== undefined) { toolFilter = definition.directTools } else if (globalDirect) { … }`
(`direct-tools.ts:143-147`):

```rust
match definition.direct_tools.get() {
    Some(BoolOrList::All(true)) => ToolFilter::All,
    Some(BoolOrList::All(false)) => ToolFilter::Off,
    Some(BoolOrList::Named(names)) => ToolFilter::Named(names.clone()),
    // PRESENT but not `true | false | string[]`. Upstream's `!== undefined` takes this branch and
    // then reaches `toolFilter.includes(name)` (`direct-tools.ts:178`, `:204`), which is a
    // TypeError on a number and an accidental SUBSTRING test on a string. Neither is worth
    // reproducing; what IS worth reproducing is that presence beats the global, and a value that
    // names no tool selects none. Named delta.
    None if !definition.direct_tools.is_absent() => ToolFilter::Off,
    None => {
        if settings.is_some_and(|s| s.direct_tools.get() == Some(&true)) {
            ToolFilter::All
        } else {
            ToolFilter::Off
        }
    }
}
```

**`proxy/approval.rs:84-89`** — `definition.approveTools !== undefined ? definition.approveTools :
settings.approveTools` (`config.rs:1198`). `definition.and_then(|entry| entry.approve_tools.as_ref())`
becomes a presence test: a per-server `approve_tools` that is present but not a `BoolOrList` must
**not** fall through to the global. Deny for that case, with the same named-delta comment.

---

## 11 · The reader — `crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`

**Mirror, do not import** — see the correction in §1. The reader gets its own `Lenient<T>` over
`serde_json::Value` rather than over `RawJson`, because it never writes a config file and every raw
it holds goes straight to `HashValue`, which `stable_stringify` sorts anyway:

```rust
/// The twin of `cyrup_mcp::config::Lenient` — the raw always, the typed view when the raw fits.
///
/// A second copy, not an import: `cyrup-mcp` is this crate's DEV-dependency on purpose
/// (`Cargo.toml:130-142`), because resolving a subagent's `mcp:` selectors must not drag rmcp,
/// reqwest and oauth2 into a spawn. `serde_json::Value` rather than `RawJson` because this module
/// has no write path — the only consumer of the raw is [`HashValue::from_optional_json`], and
/// [`stable_stringify`] sorts object keys at render time regardless.
///
/// The generalisation of `present_or_absent`, which is deleted with it: `auth` and
/// `protocolVersion` stop being special-cased because every field now keeps its raw.
#[derive(Clone, Debug)]
pub struct Lenient<T> {
    raw: Option<Value>,
    typed: Option<T>,
}
```

with the same `Default` / `is_absent` / `get` / `raw` / `as_deref` / `Deserialize` bodies as §1 — and
no `Serialize`, no `or`, no `present`: this module never writes and never merges per field
(`merge_configs`, `mcp_direct_tools.rs:519-536`, replaces whole entries).

Field types on `ServerEntry` (`mcp_direct_tools.rs:229-275`) and `RequestHeadersCommand`
(`:207-221`): `Lenient<String>`, `Lenient<Vec<String>>`, `Lenient<Value>` for `env` / `headers` /
`auth` / `protocol_version`, `Lenient<RequestHeadersCommand>`, `Lenient<bool>` for
`expose_resources`, `Lenient<f64>` for `timeout_ms`. `present_or_absent` (`:277-296`) is deleted.

`extract_server_map` (`:502-518`) becomes `toServerEntries`' own rule (`config.ts:652-667`):

```rust
fn extract_server_map(value: &Value) -> BTreeMap<String, ServerEntry> {
    let Some(map) = value.as_object() else { return BTreeMap::new() };
    map.iter()
        // `isServerEntry` is `isRecord`, i.e. a non-array object — the ONLY thing that drops an
        // entry upstream (`config.ts:661-667`). Today's bare `from_value::<ServerEntry>` also
        // accepts a JSON **array**, because serde's derived visitor implements `visit_seq` and
        // every field is `#[serde(default)]`: `"srv": []` currently becomes an empty entry where
        // upstream drops it.
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
type-level statement that a non-object entry is the only droppable thing.

The rest of the module follows §5 and §7 with the reader's own helpers:

* `server_identity_pre_image_with` (`:800-843`) folds `command` (`:806`), `args` (`:807`),
  `bearerTokenEnv` (`:835`), `exposeResources` (`:836-839`), `includeTools` (`:840`) and
  `excludeTools` (`:841`) through `HashValue::from_optional_json` over the raw, which is already
  exactly `opt_raw`; `auth` (`:826`) and `protocolVersion` (`:828-830`) already do it that way.
* `interpolate_env_record` (`:1239-1259`) gains the falsy arm and the `Object.entries` coercion:
  `HashValue::Undefined` for a falsy raw, and an object built from the coerced members otherwise,
  with the same per-member `startsWith` throw it already raises.
* `resolve_config_path`, `resolve_server_url`, `resolve_bearer_token` (`:1423`) and
  `request_headers_command_value` take the raw and raise the five sources into `IdentityError`
  (`:854`), whose doc gains the same five-source table.
* Typed read sites: `definition.exclude_tools.as_deref()` (`:621`, `:654`) is unchanged;
  `definition.expose_resources == Some(false)` (`:632`) becomes
  `definition.expose_resources.get() == Some(&false)` — upstream is `exposeResources !== false`
  (`direct-tools.ts:201`), so a present non-bool must **not** disable resources.

Six of six restored: `env:"abc"`, `env:5`, `env:true`, `env:[]`, `args:[1,"b"]` and `command:5` all
keep their server and reach the digests in the measurement table on both sides of the tree.

---

## 12 · Not in scope

State these as out of scope rather than re-deriving them:

* **Tests.** `mcp_direct_tools.rs:2264`'s differential table, `:1857`'s fifteen-field conformance
  vector and `runtime.rs:3190-3208`'s eight `String(v)` rows are **gates**, not deliverables: they
  must still pass. Do not add rows; do not add a new table.
* **`13-cyrup-mcp-STATUS.md` and `13b-mcp-config.md`.** The four "Still open" bullets at
  `13-cyrup-mcp-STATUS.md:281-309` and the `MCP-174` row at `:690` (whose `config.rs:715` citation is
  wrong — `search_keywords` is at `config.rs:865`) are ledger entries, regenerated from the tree, not
  hand-edited as part of this change. This crate's house style is that **comments carry the
  specification** (`13-cyrup-mcp-STATUS.md:293`), and the model is stated in exactly one place:
  `config.rs`'s rule 4. A parallel statement in a gap-analysis doc would be the second source of
  truth this task exists to prevent.
* **`lenient_epoch_ms`.** `dirs.rs:608-616` (`-> i64`, absent ⇒ `0`) and its twin in `registration.rs`
  (`-> Option<f64>`) disagree over the same bytes. That is the **metadata cache**'s type model, not
  the **config**'s, and `ServerCacheEntry` is not a `ServerEntry`. Leave both alone, and leave the
  cross-reference at `dirs.rs:602-603` standing.
* **The rest of MCP-069a.** §6 raises all five `request-headers-command.ts` sentences because
  `Lenient` makes each a one-line predicate. The per-request engine and the fail-closed process-tree
  reaping in `request_headers_command.rs` are untouched.

---

## Definition of done

1. `lenient` no longer exists in `config.rs`, and `grep -rn 'deserialize_with = "lenient"' crates/`
   returns nothing. (Today: 72 attributes in `config.rs`, plus five prose mentions — `config.rs:509`,
   `runtime.rs:1123`, `:3180`, `mcp_direct_tools.rs:2019`, `:2094` — that must be restated.)
2. `Lenient<T>` is the type of every field of `ServerEntry`, `McpSettings` and
   `HttpRequestHeadersCommand` in `cyrup-mcp`, and a **mirrored** `Lenient<T>` is the type of every
   field of `mcp_direct_tools.rs`'s `ServerEntry` and `RequestHeadersCommand`.
   `crates/cyrup-ext-subagents/Cargo.toml`'s `[dependencies]` section is **unchanged** — `cyrup-mcp`
   stays a dev-dependency.
3. `config.rs:60-61` (rule 4) states the requirement and the new mechanism; the
   explicit-`null`-vs-absent caveat at `config.rs:477-478` and the module-header sentence it points
   at are gone because they are no longer true; the "One real gap remains" paragraph at
   `config.rs:27-30` is gone because §6 closes it.
4. `StringRecord` deserialises **any** JSON, and for `{"command":"x", …}` the port's digest equals
   upstream's in every row of the measurement table — in particular `env:"abc"` → `01ed7340…`,
   `env:[]`/`5`/`true` → `1d224401…`, `env:0`/`""`/`false`/`null` → `f0211144…`,
   `args:[1,"b"]` → `dcb187bb…`, `args:"ab"` → `8270485a…`, `command:5` → `c486aafd…`,
   `includeTools:5` → `6819b307…`, `exposeResources:"yes"` → `bb3c42a0…`,
   `bearerTokenEnv:5` → `08952f8a…`, `requestHeadersCommand.timeoutMs:"5"` → `dab6fff7…`.
5. The eight verbatim identity keys reach `stable_stringify` through `opt_raw`; `opt_string_list` and
   `opt_serde` are deleted; `("socket", HashValue::Undefined)` is untouched.
6. `try_compute_server_hash` returns `Err` for each of the five sources with upstream's byte-exact
   message, and returns `Ok` for `url: null` and for `requestHeadersCommand: null`.
   `dirs.rs:1078-1085`, `registration.rs:792` and `registration.rs:865-866` say **five**.
7. `merge_entry` uses `Lenient::or`, so a present-but-wrong-typed override **wins** over a
   lower-precedence definition; its URL guard and its `oauth !== false` guard both read `get()`.
8. `extract_server_map` drops an entry for exactly one reason: it is not a non-array object.
   `"srv": []` is dropped where it is currently kept.
9. `resolve_stdio_env` and `resolve_http_secrets` fail the connect on a non-string member instead of
   spawning a shortened env or sending shortened headers; the `literalEnv` arm spreads every member
   through `js_string`; the HTTP guard sits before the step-2 `hasCommandHeader` scan.
10. `resolve_search_keywords` skips per key and per element; `ranking.rs:273-278`'s two-bullet note
    is deleted.
11. `AuthMode::Other` and `ProtocolVersionSetting::Other` no longer exist; every read site in §8's
    table is updated; `oauth.rs:369` and `:3927` read `is_absent()`, **not** `get().is_none()`;
    `version_negotiation` raises `Invalid MCP protocolVersion: …` for a present-and-unusable value
    and `ClientLifecycleMode::Initialize` for an absent one.
12. `runtime.rs`'s HTTP connect refuses a present-but-non-object `requestHeadersCommand` with
    *"HTTP request headers command must be an object"*, and `resolve_request_headers_command` raises
    the other four sentences of `request-headers-command.ts:162-178`.
13. `direct_tools` and `approve_tools` are decided on **presence**, not on the typed view.
14. `cargo nextest run --workspace` is at **7862** passing with no new failures, and the workspace
    lints (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`,
    `rustdoc::broken_intra_doc_links`, all `deny`) are clean.
