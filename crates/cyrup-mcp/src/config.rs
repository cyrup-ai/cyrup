//! `mcp.json` — `config.ts` and `types.ts` (13b; MCP-050…MCP-090).
//!
//! This module owns the **typed** view of the configuration ([`McpConfig`], [`ServerEntry`],
//! [`McpSettings`]) *and* everything `config.ts` does with it: the six-source resolution ladder
//! ([`ConfigContext::sources`], MCP-052), the per-field merge with URL-bound credential stripping
//! ([`merge_entry`], MCP-053 — **critical**), the seven host-config import families
//! ([`ImportKind`], MCP-056/057), discovery, conflicts and the panel fingerprint (MCP-059), and the
//! six atomic writers with their preview twins (MCP-061…MCP-065).
//!
//! Upstream is `pi-mcp-adapter` **v2.25.0** `config.ts` (1226 lines) plus `types.ts`, retargeted
//! to **v2.26.1**; every `file:line` citation below is against `git show v2.25.0:config.ts` unless
//! it names a later version explicitly.
//!
//! **`config.ts` is fully reconciled to v2.26.1 and nothing is outstanding.** The whole delta is
//! `+4/-4` in four one-line hunks: the `getConfigDirName` import, `PROJECT_PI_CONFIG_NAME` losing
//! its `.pi/` prefix, `getProjectPiConfigPath` composing the dir name (all three commit `4ab5a40`,
//! already ported as [`PROJECT_OVERRIDE_DIR`] + [`crate::dirs::MCP_CONFIG_FILE`]), and
//! [`URL_BOUND_AUTH_FIELDS`] gaining `requestHeadersCommand` (commit `2a2db3c`, ported with
//! [`HttpRequestHeadersCommand`]). `types.ts` contributes `warnOnLargeDirectTools` (`76a4ea3`) and
//! the `ServerEntry` field, both ported.
//!
//! An earlier revision of this header listed `bearerTokenStore` and "the rewrite of
//! `mergeServerMaps`' first two rules" as outstanding v2.26.0 work. **Neither exists.**
//! `git grep bearerTokenStore v2.26.1` is empty — the symbol appears at no upstream tag — and
//! `mergeServerMaps`' rules are byte-identical between v2.25.0 and v2.26.1. Both were removed rather
//! than left as phantom work items for the next reader to hunt.
//!
//! One real gap remains, and it is *not* a missing upstream feature: a malformed
//! `requestHeadersCommand` fails **open** here, because [`lenient`] degrades it to `None` and the
//! server then connects unsigned. See [`HttpRequestHeadersCommand`] and plan unit **MCP-069a**.
//!
//! # Four rules the code below encodes, and one it cannot
//!
//! **1 · Defaults are enforced at the read site, never at parse.** Every field is `Option<T>`, and
//! the documented default is applied by whoever reads it — because the predicate is often *not*
//! the documented default. `notifyOnStartupConnect` is `!== false` (so `null` enables it),
//! `idleTimeout` is `typeof === "number" ? v : 10` (so an explicit `0` is honoured and means
//! "never idle out"), `collapsedResultLines` is a **whitelist** of `1|2|3` rather than a clamp, and
//! `exposeResources` is tested `!== false` everywhere. MCP-066 puts each predicate in exactly one
//! place: the accessor on [`McpSettings`] named for its read site.
//!
//! **2 · Optional is not null.** `#[serde(default, skip_serializing_if = "Option::is_none")]`
//! throughout. Writing `null` for an absent field would change the on-disk file the adapter
//! round-trips — and, worse, would change the `computeServerHash` pre-image, whose scalar branch
//! emits the literal 9-character token `undefined` for absent and `null` for an explicit JSON null
//! (MCP-070, §17).
//!
//! **3 · Unknown keys must round-trip.** `writeSharedServerEntry` and its siblings operate on the
//! **raw** parsed object, never on a typed [`ServerEntry`], precisely so an unknown key survives a
//! write. The port keeps that split: this typed struct for reading and merging, and [`RawJson`] —
//! an *insertion-ordered* JSON document — for writing. Do not unify them, and do not add
//! `#[serde(flatten)]`: an unknown key that survived into a merged entry would bypass
//! [`merge_entry`]'s credential-stripping set.
//!
//! **4 · A malformed config must never `Err`.** `validateConfig` is lenient by design: a non-object
//! root yields `{ mcpServers: {} }`, a malformed *entry* is dropped while the file survives, and
//! `settings` is **not validated at all** — it is a bare cast. A strict `Deserialize` would turn a
//! forward-compatible file into a startup failure, and [`crate::extension::McpExtension`]'s
//! `init` must never return `Err` (MCP-003). Every field of [`ServerEntry`] and [`McpSettings`] therefore
//! goes through [`lenient`], which degrades a type mismatch to `None` instead of failing the file.
//! Without it a single `"idleTimeout": "10"` would take every server in the file down with it.
//!
//! The rule the types cannot encode: **validation fires at connect time, not at parse time.** A
//! two-transport entry (`command` *and* `url`) loads fine and fails per-connection with
//! `` `Server ${name} must configure exactly one of command, url` `` (the message loses
//! `, or socket` post-Cut-3). That is why `isServerCacheValid` wraps `computeServerHash` in a
//! `try/catch`.
//!
//! # Two cut values are rejected loudly, not dropped silently
//!
//! `socket` (Cut 3) and `httpTransport: "sse"` (Cut 1) have no runtime in this build.
//! `agent-plugin-loader.ts` sets `httpTransport` straight from a manifest's `type`, so a plugin
//! declaring `sse` is a live, reachable case. Both are detected at the **raw** level in
//! [`to_server_entries`], the entry is dropped from *that source only* (so a lower-precedence
//! definition survives intact rather than being half-overridden), and a named
//! [`ConfigDiagnostic`] is recorded — MCP-054 / MCP-069.
//!
//! # Recorded divergences from v2.25.0
//!
//! * **`.pi` → `.cyrup`** for the project override directory, settled by in-tree precedent
//!   (`cyrup_ext_subagents::exec::mcp_direct_tools::get_config_paths`). The three tool-agnostic
//!   paths (`~/.config/mcp/mcp.json`, `~/.agents/mcp.json`, `~/.agents/mcp/mcp.json`) stay verbatim.
//! * **Project trust (MCP-096, the section's one open decision).** Upstream has no trust gate at
//!   all. This port implements the plan's recommendation (b): with
//!   [`ConfigContext::project_trusted`] `false`, the two project-scoped sources contribute **zero
//!   servers** while still appearing in the discovery summary as present-but-untrusted. The default
//!   is `true`, so a caller that has not wired `HostServices::is_project_trusted` behaves exactly
//!   as upstream does.
//! * **An explicit JSON `null` normalises to absent.** [`lenient`] cannot distinguish
//!   `"command": null` from an absent `command`; upstream can, and the difference is observable in
//!   the `computeServerHash` pre-image alone (§17). Recorded rather than worked around: preserving
//!   it costs an `Option<Option<T>>` on all 27 fields and buys one byte sequence for a config
//!   nobody writes by hand.
//! * **`JSON.stringify` float shape.** `serde_json`'s pretty writer renders a float that parsed
//!   from `1000.0` as `1000.0`; JS renders `1000`. Integers (the only numbers `mcp.json` actually
//!   carries) are byte-identical because they round-trip through `u64`/`i64`.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use indexmap::IndexMap;
use serde::de::{DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};


use crate::dirs::McpDirs;
use crate::errors::{McpError, McpResult};

// ===================================================================================================
// 1 · Path constants — `config.ts` head
// ===================================================================================================

/// `PROJECT_CONFIG_NAME` — `<cwd>/.mcp.json`, the shared project file.
pub const PROJECT_CONFIG_NAME: &str = ".mcp.json";

/// `getConfigDirName()` — upstream `.pi`. Renamed by settled in-tree precedent; see the module
/// header.
pub const PROJECT_OVERRIDE_DIR: &str = ".cyrup";

/// `REPOPROMPT_BINARY_CANDIDATES[1]`. The `[0]` entry is `~/RepoPrompt/repoprompt_cli` and is built
/// from the caller's home directory, so it cannot be a `const`.
pub const REPOPROMPT_APP_BINARY: &str =
    "/Applications/Repo Prompt.app/Contents/MacOS/repoprompt-mcp";

/// The four markers [`find_project_root`] walks **up** for, in `findProjectRoot`'s own test order.
/// Upstream's fourth is `.pi`; it is renamed with the rest of the project override directory.
pub const PROJECT_ROOT_MARKERS: [&str; 4] =
    [".git", "package.json", PROJECT_CONFIG_NAME, PROJECT_OVERRIDE_DIR];

// ===================================================================================================
// 2 · `ImportKind` — the seven host-config families (MCP-056)
// ===================================================================================================

/// `types.ts` `ImportKind` — the seven other agent tools whose MCP config this adapter can read.
///
/// Declaration order **is** `IMPORT_PATHS`' iteration order, and that order is load-bearing three
/// times: `loadDiscoveredHostConfigs` folds later-wins, `getConfigConflicts` records host
/// candidates in it, and `getServerProvenance` lets later kinds overwrite earlier ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportKind {
    /// `~/.cursor/mcp.json`.
    Cursor,
    /// `~/.claude/mcp.json`, `~/.claude.json`, `~/.claude/claude_desktop_config.json`.
    ClaudeCode,
    /// `~/Library/Application Support/Claude/claude_desktop_config.json`.
    ClaudeDesktop,
    /// `~/.codex/config.toml`, `~/.codex/config.json` — the only TOML path in the package, and the
    /// only family besides `opencode` whose entries are **translated** rather than passed through.
    Codex,
    /// `~/.config/opencode/opencode.json` plus a git-root-bounded walk for `./opencode.json`. The
    /// only family whose candidates are *merged* rather than first-wins.
    Opencode,
    /// `~/.windsurf/mcp.json`.
    Windsurf,
    /// `<cwd>/.vscode/mcp.json`.
    Vscode,
}

impl ImportKind {
    /// `Object.keys(IMPORT_PATHS)` — declaration order, which is the iteration order everywhere.
    pub const ALL: [ImportKind; 7] = [
        ImportKind::Cursor,
        ImportKind::ClaudeCode,
        ImportKind::ClaudeDesktop,
        ImportKind::Codex,
        ImportKind::Opencode,
        ImportKind::Windsurf,
        ImportKind::Vscode,
    ];

    /// The on-the-wire spelling, as it appears in an `imports` array.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ImportKind::Cursor => "cursor",
            ImportKind::ClaudeCode => "claude-code",
            ImportKind::ClaudeDesktop => "claude-desktop",
            ImportKind::Codex => "codex",
            ImportKind::Opencode => "opencode",
            ImportKind::Windsurf => "windsurf",
            ImportKind::Vscode => "vscode",
        }
    }

    /// `Object.hasOwn(IMPORT_PATHS, kind)` — the membership test
    /// `writeProjectServerDisabledOverride` uses to reject an unsupported `imports` entry, and the
    /// filter `expandImports` applies implicitly (`IMPORT_PATHS[kind] ?? []` yields no candidates).
    #[must_use]
    pub fn parse(raw: &str) -> Option<ImportKind> {
        ImportKind::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }

    /// `extractServers`' per-family server key. `None` means "the family reads a fixed key that has
    /// no alternative spelling"; the `Vec` is tried in order, first present wins.
    fn server_keys(self) -> &'static [&'static str] {
        match self {
            // `obj.mcpServers` only — no hyphenated alternative for either Claude family.
            ImportKind::ClaudeCode | ImportKind::ClaudeDesktop => &["mcpServers"],
            // `obj.mcp_servers ?? obj.mcpServers` — TOML convention first.
            ImportKind::Codex => &["mcp_servers", "mcpServers"],
            // `obj.mcpServers ?? obj["mcp-servers"]`.
            ImportKind::Cursor | ImportKind::Windsurf | ImportKind::Vscode => {
                &["mcpServers", "mcp-servers"]
            }
            // OpenCode's own schema.
            ImportKind::Opencode => &["mcp"],
        }
    }
}

impl fmt::Display for ImportKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ===================================================================================================
// 3 · `RawJson` — the insertion-ordered document the writers operate on
// ===================================================================================================

/// A JSON document that remembers its key order.
///
/// # Why this exists rather than `serde_json::Value`
///
/// `serde_json::Map` is a `BTreeMap` under this workspace's feature set (`preserve_order` is off),
/// so **every** round-trip through `Value` silently sorts object keys. For `mcp.json` that is not
/// cosmetic: `Object.entries(config.mcpServers)` yields insertion order, and that order decides the
/// startup connect order, the `/mcp` listing order, the tool-name collision tie-break, and — via
/// `writeRawConfigObject` — the bytes of a file the user reads in a diff. `readRawConfigObject`
/// exists upstream precisely so unknown keys survive a write; sorting them would be a different
/// kind of loss but the same bug.
///
/// This type is also the port's leniency substrate: [`lenient`] deserialises a field into a
/// `RawJson` first and only *then* tries the typed shape, so a type mismatch degrades to `None`
/// instead of failing the whole file (rule 4 in the module header).
#[derive(Debug, Clone, PartialEq)]
pub enum RawJson {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// Any JSON number. Kept as a [`serde_json::Number`] so an integer round-trips as an integer.
    Number(serde_json::Number),
    /// A string.
    String(String),
    /// An array.
    Array(Vec<RawJson>),
    /// An object, in document order.
    Object(IndexMap<String, RawJson>),
}

impl RawJson {
    /// An empty object — `readRawConfigObject`'s answer for a missing, unparseable or non-object
    /// file.
    #[must_use]
    pub fn empty_object() -> RawJson {
        RawJson::Object(IndexMap::new())
    }

    /// `isRecord(value)` — `typeof value === "object" && value !== null && !Array.isArray(value)`.
    #[must_use]
    pub fn as_object(&self) -> Option<&IndexMap<String, RawJson>> {
        match self {
            RawJson::Object(entries) => Some(entries),
            _ => None,
        }
    }

    /// Mutable counterpart of [`Self::as_object`].
    pub fn as_object_mut(&mut self) -> Option<&mut IndexMap<String, RawJson>> {
        match self {
            RawJson::Object(entries) => Some(entries),
            _ => None,
        }
    }

    /// `Array.isArray(value)`.
    #[must_use]
    pub fn as_array(&self) -> Option<&[RawJson]> {
        match self {
            RawJson::Array(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    /// `typeof value === "string"`.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            RawJson::String(text) => Some(text.as_str()),
            _ => None,
        }
    }

    /// `obj[key]`, `undefined`-safe.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&RawJson> {
        self.as_object().and_then(|entries| entries.get(key))
    }

    /// The first present key of `keys` — `extractServers`' `a ?? b` chain, which tests
    /// *presence*, not truthiness.
    #[must_use]
    pub fn get_first(&self, keys: &[&str]) -> Option<&RawJson> {
        keys.iter().find_map(|key| self.get(key))
    }

    /// `toStringRecord(value)` (`utils.ts`) — keep only string-valued keys, and yield `None` for an
    /// empty result so the caller's `...(rec ? { env: rec } : {})` spread stays faithful.
    #[must_use]
    pub fn to_string_record(&self) -> Option<BTreeMap<String, String>> {
        let entries = self.as_object()?;
        let record: BTreeMap<String, String> = entries
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_string())))
            .collect();
        if record.is_empty() { None } else { Some(record) }
    }
}

impl Serialize for RawJson {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            RawJson::Null => serializer.serialize_unit(),
            RawJson::Bool(value) => serializer.serialize_bool(*value),
            RawJson::Number(value) => value.serialize(serializer),
            RawJson::String(value) => serializer.serialize_str(value),
            RawJson::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            RawJson::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for RawJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(RawJsonVisitor)
    }
}

/// The `deserialize_any` visitor behind [`RawJson`]. Deliberately deserializer-agnostic: the same
/// type is filled from `serde_json` (every `mcp.json` and every JSON host config) and from `toml`
/// (`~/.codex/config.toml`, the one TOML path in the package), and both feed map entries in
/// document order.
struct RawJsonVisitor;

impl<'de> Visitor<'de> for RawJsonVisitor {
    type Value = RawJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_unit<E>(self) -> Result<RawJson, E> {
        Ok(RawJson::Null)
    }

    fn visit_none<E>(self) -> Result<RawJson, E> {
        Ok(RawJson::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<RawJson, D::Error> {
        RawJson::deserialize(deserializer)
    }

    fn visit_bool<E>(self, value: bool) -> Result<RawJson, E> {
        Ok(RawJson::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<RawJson, E> {
        Ok(RawJson::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<RawJson, E> {
        Ok(RawJson::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<RawJson, E> {
        // `JSON.stringify` renders a non-finite number as `null`; there is no JSON literal for one,
        // so an input that produced it (only reachable from TOML) degrades the same way.
        Ok(serde_json::Number::from_f64(value).map_or(RawJson::Null, RawJson::Number))
    }

    fn visit_str<E>(self, value: &str) -> Result<RawJson, E> {
        Ok(RawJson::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<RawJson, E> {
        Ok(RawJson::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<RawJson, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(RawJson::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<RawJson, A::Error> {
        let mut entries = IndexMap::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, value)) = map.next_entry::<String, RawJson>()? {
            entries.insert(key, value);
        }
        Ok(RawJson::Object(entries))
    }
}

/// Re-read a [`RawJson`] as a typed value, or `None` if it does not fit.
///
/// The round-trip goes through a compact JSON **string** rather than `serde_json::from_value`
/// because `from_value` would build a `Value` — and sort every object key on the way (see
/// [`RawJson`]).
fn raw_to<T: DeserializeOwned>(raw: &RawJson) -> Option<T> {
    let text = serde_json::to_string(raw).ok()?;
    serde_json::from_str(&text).ok()
}

/// The inverse of [`raw_to`]: a typed value as an insertion-ordered document.
///
/// `serde_json::to_string` writes a struct's fields in **declaration** order (it never builds a
/// `Value`), so the resulting [`RawJson`] carries the port's field order. That is the one place the
/// written file can differ cosmetically from upstream's, which reuses the parsed object's own order
/// — recorded, and invisible to every reader.
fn raw_from<T: Serialize>(value: &T) -> RawJson {
    serde_json::to_string(value)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(RawJson::empty_object)
}

/// A permissive field reader — the whole of §7 rule 4 in one function.
///
/// `validateConfig` does not validate `settings` at all (it is a bare TypeScript cast) and
/// `toServerEntries` only checks that an entry is a non-array object, so upstream *cannot* fail a
/// file on a wrong-typed field. A plain `Option<T>` would: `"idleTimeout": "10"` inside `settings`
/// would abort the whole document and take every server with it. This buffers the field into a
/// [`RawJson`], tries the typed shape, and yields `None` when it does not fit.
///
/// The one thing it cannot preserve is explicit-`null`-vs-absent; see the module header.
pub fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = RawJson::deserialize(deserializer)?;
    Ok(raw_to::<T>(&raw))
}

// ===================================================================================================
// 4 · The typed model — `types.ts` (MCP-066, MCP-069)
// ===================================================================================================

/// A `Record<string, string>` config block — `env`, `headers` and `requestHeadersCommand.env` — held
/// so that a **non-string member is remembered rather than dropped** (MCP-144).
///
/// # Why this is not a bare `BTreeMap<String, String>` behind [`lenient`]
///
/// It was, and that was one of the four hashing divergences from upstream.
/// `interpolateEnvRecord(values)` (`utils.ts:107`) calls
/// `interpolateSecretExpression(value)` → `value.startsWith("!!")` on **every** member
/// unconditionally, so one non-string member makes `computeServerHash`
/// (`metadata-cache.ts:90`, `:93`, `:98`) **throw**, and `isServerCacheValid`
/// (`metadata-cache.ts:114`) catches that to `false`: the entry is simply never cache-valid.
/// Measured by running `utils.ts` on node 22 against `tmp/pi-mcp-adapter` @ tag `v2.26.1`
/// (`fafae21`) — `interpolateEnvRecord({ k: 5 })` throws `value.startsWith is not a function`,
/// `{ k: true }`, `{ k: [1] }` and `{ k: { a: 1 } }` throw the same, and
/// `interpolateEnvRecord({ k: null })` throws
/// `Cannot read properties of null (reading 'startsWith')`.
///
/// `deserialize_with = "lenient"` over a `BTreeMap<String, String>` dropped the **whole map**, so
/// this crate hashed `"env":undefined`, produced a perfectly good digest, and called the entry
/// **valid** — while `cyrup_ext_subagents::exec::mcp_direct_tools`, which holds the raw JSON and
/// reproduces the throw, called the same entry **invalid**. Two crates in one tree returning
/// opposite answers about one cache entry is worse than either answer on its own, and it is the
/// only one of the four divergences that is not merely a digest mismatch.
///
/// So the type keeps three views of the same block:
///
/// * [`std::ops::Deref`] to the **string** members, which is what every consumer wants and is byte-identical
///   to what the old `BTreeMap<String, String>` held for a well-formed map. Read sites move from
///   `entry.env.as_ref()` to `entry.env.as_deref()` and are otherwise untouched.
/// * [`Self::unhashable`] — `Some(message)` when at least one member was not a string, carrying
///   upstream's own TypeError text. [`crate::dirs::ResolvedIdentity::resolve`] turns it into the
///   `Err` that is upstream's throw, so the digest never exists.
/// * The **raw** members, so a write-back through `raw_from` round-trips a member this port cannot
///   use instead of erasing it. The old field erased the entire block on every write.
///
/// # The one named micro-delta
///
/// Upstream throws on the first offending member in `Object.entries` (**insertion**) order; both
/// Rust sides iterate in **key** order, because both hold the block in a [`BTreeMap`]. The two can
/// disagree only about *which* message a block carrying both a `null` member and a non-null
/// non-string member produces — never about *whether* it throws, and the message reaches nothing
/// but a `catch`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StringRecord {
    /// Every member exactly as written, so [`Serialize`] is lossless.
    raw: BTreeMap<String, RawJson>,
    /// The string-valued members — the [`std::ops::Deref`] target, and the only view a consumer sees.
    values: BTreeMap<String, String>,
    /// `Some(upstream's TypeError text)` when [`Self::raw`] holds a member [`Self::values`] could
    /// not take. Derived, never stored independently.
    unhashable: Option<String>,
}

impl StringRecord {
    /// Split a raw block into the string view and the throw, exactly as `interpolateEnvRecord`
    /// would when it walked it.
    fn from_raw(raw: BTreeMap<String, RawJson>) -> Self {
        let mut values = BTreeMap::new();
        let mut unhashable = None;
        for (key, value) in &raw {
            if let RawJson::String(text) = value {
                let _ = values.insert(key.clone(), text.clone());
            } else if unhashable.is_none() {
                // The two messages node actually raises; see the type's doc comment.
                unhashable = Some(
                    if matches!(value, RawJson::Null) {
                        "Cannot read properties of null (reading 'startsWith')"
                    } else {
                        "value.startsWith is not a function"
                    }
                    .to_string(),
                );
            }
        }
        Self { raw, values, unhashable }
    }

    /// The string-valued members. Same map the field used to be.
    #[must_use]
    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.values
    }

    /// `Some(message)` when `interpolateEnvRecord` would throw on this block — upstream's own
    /// TypeError text, which reaches nothing but `isServerCacheValid`'s `catch`.
    #[must_use]
    pub fn unhashable(&self) -> Option<&str> {
        self.unhashable.as_deref()
    }
}

impl std::ops::Deref for StringRecord {
    type Target = BTreeMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl From<BTreeMap<String, String>> for StringRecord {
    fn from(values: BTreeMap<String, String>) -> Self {
        Self {
            raw: values
                .iter()
                .map(|(key, value)| (key.clone(), RawJson::String(value.clone())))
                .collect(),
            values,
            unhashable: None,
        }
    }
}

impl FromIterator<(String, String)> for StringRecord {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<BTreeMap<String, String>>())
    }
}

impl Serialize for StringRecord {
    /// The **raw** members, so an unusable member survives a write instead of being erased.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StringRecord {
    /// A JSON **object** of anything. A non-object still fails, which is what makes [`lenient`]
    /// drop `"env": "abc"` exactly as it did before this type existed. (Upstream would hash
    /// `Object.entries("abc")`, i.e. `{"0":"a","1":"b","2":"c"}` — a fifth, separate divergence
    /// that is not this change's business and is recorded in `13c-mcp-servers.md`'s MCP-144 notes.)
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_raw(BTreeMap::<String, RawJson>::deserialize(deserializer)?))
    }
}

/// The merged view of every `mcp.json` source — upstream `McpConfig`.
///
/// `mcpServers` is an [`IndexMap`] because `Object.entries(...)` yields **insertion order**, and
/// that order is user-visible: it is the order servers connect in during the startup pass, the
/// order they list in `/mcp`, and the tie-break for tool-name collisions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    /// The server table, in file order. Also accepted on the wire as the legacy `mcp-servers`
    /// key, which `cli.js` normalises away on write (MCP-049).
    #[serde(default)]
    pub mcp_servers: IndexMap<String, ServerEntry>,
    /// The `settings` block. Absent means "every default", never "no settings".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<McpSettings>,
    /// The host-config import families this config opted into (`cursor`, `claude-code`, `codex`,
    /// `opencode`, `windsurf`, `vscode`, `claude-desktop`). Non-string elements are filtered out on
    /// read; unknown strings survive the read and are ignored by [`expand_imports`], exactly as
    /// upstream's `IMPORT_PATHS[kind] ?? []` does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
}

/// The all-`None` settings block every accessor falls back to, so
/// [`McpConfig::settings_or_default`] can hand out a reference without allocating.
static EMPTY_SETTINGS: McpSettings = McpSettings {
    tool_prefix: None,
    show_status_icon: None,
    mcp_footer_status: None,
    notify_on_startup_connect: None,
    host_config_discovery: None,
    agent_plugin_paths: None,
    idle_timeout: None,
    request_timeout_ms: None,
    direct_tools: None,
    warn_on_large_direct_tools: None,
    tool_result_rendering: None,
    collapsed_result_lines: None,
    approve_tools: None,
    disable_proxy_tool: None,
    freeze_direct_tools: None,
    auto_auth: None,
    sampling: None,
    sampling_auto_approve: None,
    elicitation: None,
    output_guard: None,
    trace: None,
    auth_required_message: None,
    oauth_dir: None,
};

impl McpConfig {
    /// `Object.entries(config.mcpServers).filter(([, d]) => !isServerDisabled(d))` — the enabled
    /// servers, in file order.
    pub fn enabled_servers(&self) -> impl Iterator<Item = (&String, &ServerEntry)> {
        self.mcp_servers.iter().filter(|(_, entry)| !entry.is_disabled())
    }

    /// The `settings` block, or an all-defaults one. Every accessor on [`McpSettings`] encodes its
    /// own read-site predicate, so `config.settings_or_default().notify_on_startup_connect()` is
    /// the whole of `settings?.notifyOnStartupConnect !== false`.
    #[must_use]
    pub fn settings_or_default(&self) -> &McpSettings {
        self.settings.as_ref().unwrap_or(&EMPTY_SETTINGS)
    }

    /// `config.settings?.toolPrefix ?? "server"` — the one settings read `init.ts` performs before
    /// anything else, because it decides every direct tool's name.
    #[must_use]
    pub fn tool_prefix(&self) -> ToolPrefix {
        self.settings_or_default().tool_prefix()
    }
}

/// `HttpRequestHeadersCommand` (`types.ts:359-369`, upstream **v2.26.0** commit `2a2db3c`) — a
/// trusted executable run for **every** outbound HTTP request on this server.
///
/// `headers` is resolved once at connect; this is resolved per request, because a caller-bound
/// signature (HMAC over the body, DPoP, SigV4) is a function of the exact bytes about to be sent.
/// The engine — the JSON envelope on stdin, the JSON header object on stdout, and the fail-closed
/// process-tree reaping that stops a signing helper outliving its request — is
/// [`crate::request_headers_command`].
///
/// # Why every field is optional when upstream types `command` as required
///
/// Upstream's `command: string` is a TypeScript type, not a check: `validateConfig` never inspects
/// this block, and `resolvedCommand` is what throws
/// `"HTTP request headers command requires a non-empty command"` — at **connect** time, not at
/// parse. Module rule 1 puts the predicate at the read site, and rule 4 forbids a malformed value
/// taking the file down, so the field is `Option<String>` and
/// [`crate::request_headers_command::resolve_request_headers_command`] raises upstream's sentence.
/// `timeoutMs` is `f64` for the same reason [`ServerEntry::idle_timeout`] is: it arrives from
/// `JSON.parse`, and `Number.isInteger` is checked at the read site.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequestHeadersCommand {
    /// The executable, `interpolateEnvVars`'d. Spawned directly — **not** through a shell, unlike
    /// the `!`-prefixed `headers` secret form.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments, each `interpolateEnvVars`'d.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Environment overrides layered over the adapter's own environment, each value
    /// `interpolateEnvVars`'d. No `!`-secret resolution: upstream uses `interpolateEnvVars`, not
    /// `resolveCommandSecret`, so a leading `!` here is literal.
    ///
    /// A [`StringRecord`] for the same reason [`ServerEntry::env`] is: `computeServerHash` runs the
    /// **nested** env through `interpolateEnvRecord` too (`metadata-cache.ts:98`), so a non-string
    /// member here throws exactly as one in the outer map does. The nested block is not a laxer map
    /// than the outer one, and `cyrup_ext_subagents::exec::mcp_direct_tools` already treats it that
    /// way.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub env: Option<StringRecord>,
    /// Per-invocation timeout in **milliseconds**; defaults to `10_000` and must be an integer in
    /// `1..=60000`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<f64>,
}

/// One `mcpServers` entry — upstream `ServerEntry` (`ServerDefinition` is an alias).
///
/// Upstream carries 28 fields; `socket` is **Cut 3** (the raw framed unix-socket transport, which
/// rmcp does not implement), leaving the 27 below, plus `requestHeadersCommand` from the v2.26.1
/// retarget. README documents 26 of them — `httpTransport`, `pluginDataDir` and `literalEnv` are
/// set only by [`crate::agent_plugin`] and are never hand-written.
///
/// Every field goes through [`lenient`]: a wrong-typed field degrades to `None` rather than taking
/// the whole file down (module header, rule 4). There is deliberately **no**
/// `#[serde(flatten)]` catch-all — an unknown key that survived into a merged entry would bypass
/// [`merge_entry`]'s credential-stripping set (MCP-053).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntry {
    /// stdio transport: the executable. Exactly one of `command` / `url` must be set, checked at
    /// **connect** time.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments, each `interpolateEnvVars`'d at connect time.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Environment, layered over the full `process.env` and passed through
    /// `resolveCommandSecretsRecord` — unless [`Self::literal_env`] is set.
    ///
    /// A [`StringRecord`], **not** a `BTreeMap<String, String>`: a single non-string member makes
    /// upstream's `computeServerHash` throw, and dropping the whole map (which is all `lenient`
    /// could do over a plain map) made this crate call an entry cache-valid that the in-tree reader
    /// called invalid. See [`StringRecord`] for the measurement and the consequence.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub env: Option<StringRecord>,
    /// Working directory, `resolveConfigPath`'d (interpolation + `~`); falls back to the session
    /// cwd.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// HTTP transport: the endpoint. Passed through `resolveServerUrl`, which **throws** on a
    /// non-string value, a missing interpolation variable, or an invalid URL after interpolation.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Extra request headers, `!`-secret-resolved and interpolated. Part of the
    /// `computeServerHash` pre-image, and one of `URL_BOUND_AUTH_FIELDS`.
    ///
    /// A [`StringRecord`] for the same reason [`Self::env`] is — `computeServerHash` runs both
    /// through the same `interpolateEnvRecord` (`metadata-cache.ts:90` and `:93`).
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub headers: Option<StringRecord>,
    /// Add or replace HTTP headers by running a trusted command **for each request**
    /// (`types.ts:383`, v2.26.0). Part of the `computeServerHash` pre-image, and one of
    /// `URL_BOUND_AUTH_FIELDS` — it signs requests to *one* endpoint, so it must not follow a
    /// higher-precedence source that only repointed `url`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub request_headers_command: Option<HttpRequestHeadersCommand>,
    /// `"oauth" | "bearer" | false`. **Absent** is not `false`: it means "auto-detect OAuth for a
    /// `url` server".
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthMode>,
    /// A literal bearer token. Prefer [`Self::bearer_token_env`]. One of `URL_BOUND_AUTH_FIELDS`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
    /// The environment variable holding the bearer token. One of `URL_BOUND_AUTH_FIELDS`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub bearer_token_env: Option<String>,
    /// OAuth client configuration, or the literal `false` to disable it. Untagged: only `false` is
    /// legal on the boolean arm, and `oauth: true` is *tolerated* exactly as TypeScript's
    /// structural cast tolerates it — the value simply never satisfies `oauth !== false`, which is
    /// also why `oauth: true` is **not** protected from URL-bound stripping.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthSetting>,
    /// When the server's process is spawned and how long it lives. Defaults to
    /// [`ServerLifecycle::Lazy`].
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<ServerLifecycle>,
    /// Idle timeout in **minutes**, overriding the global. `0` disables the idle close.
    /// `eager` and `lazy-keep-alive` with no explicit value are forced to `0` by `init.ts`'s
    /// `persistsAfterFirstSpawn` (MCP-020).
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<f64>,
    /// Per-request timeout in **milliseconds**. Normalised by `normalizeRequestTimeoutMs`: must be
    /// finite and `> 0`, else it falls through to the global setting.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub request_timeout_ms: Option<f64>,
    /// Whether this server's resources become `read_*` direct tools. Tested `!== false`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub expose_resources: Option<bool>,
    /// `true` / `false` / an explicit tool-name list. A per-server value that is merely *present*
    /// beats `settings.directTools` — the test is `!== undefined`, not truthiness.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub direct_tools: Option<BoolOrList>,
    /// Overrides `settings.toolPrefix` for this server (`resolveToolPrefix`).
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub tool_prefix: Option<ToolPrefix>,
    /// Glob-or-exact allowlist, applied **before** [`Self::exclude_tools`].
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub include_tools: Option<Vec<String>>,
    /// Glob-or-exact denylist, applied after the allowlist.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub exclude_tools: Option<Vec<String>>,
    /// Extra ranking keywords per tool. Ranking-only: they never appear in a schema, in `describe`
    /// output, or in the metadata cache.
    ///
    /// `IndexMap`, NOT `BTreeMap`, and the distinction is a correctness one rather than a taste
    /// one. Upstream's `resolveSearchKeywords` walks `Object.entries(searchKeywords)`, i.e. **key
    /// insertion order**, and unions the values of every glob key that matches the tool. Under a
    /// `BTreeMap` those keys come back sorted, so whenever two globs both match one tool the union
    /// is assembled in a different order than upstream produces — and that order is observable, it
    /// is the order keywords are scored and reported in. Same family as the `mcp_servers` ordering
    /// trap this module already documents.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub search_keywords: Option<IndexMap<String, Vec<String>>>,
    /// Which of this server's tools skip the approval prompt. Present beats the global.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub approve_tools: Option<BoolOrList>,
    /// `true` ⇒ the child's stderr is **inherited** (visible in the terminal); `false`/absent ⇒
    /// piped. rmcp's `TokioChildProcessBuilder` defaults to `Stdio::inherit()`, so the port sets
    /// `.stderr(Stdio::piped())` on the `false` arm rather than the other way round.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
    /// Per-server trace override: `definition.trace ?? settings.trace?.enabled === true`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub trace: Option<bool>,
    /// Which HTTP transport to use. **Cut 1** removes the `sse` value — rmcp 3.1.2 ships no SSE
    /// *client* transport at all — so the field survives with one legal value and an `sse` entry is
    /// rejected at config load with a named diagnostic. Absent means streamable-HTTP with the
    /// upstream SSE fallback, which post-cut is simply streamable-HTTP.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub http_transport: Option<HttpTransport>,
    /// A directory created (`recursive`) before the child spawns. Set only by
    /// [`crate::agent_plugin`], which uses it as `${PLUGIN_DATA}`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub plugin_data_dir: Option<String>,
    /// `true` ⇒ [`Self::env`] values are used **verbatim**: no `!`-secret resolution and no
    /// interpolation. Set only by [`crate::agent_plugin`], and it is half of what keeps a plugin
    /// from reading the user's environment.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub literal_env: Option<bool>,
    /// Protocol-revision negotiation. `undefined` and `"legacy"` are **byte-identical** on the
    /// wire — both send no `versionNegotiation` — and any other value throws
    /// `` `Invalid MCP protocolVersion: ${…}` `` at connect time.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<ProtocolVersionSetting>,
    /// Only the **literal boolean `true`** disables a server; see [`Self::is_disabled`].
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
}

impl ServerEntry {
    /// `isServerDisabled(definition)` — and its doc comment is the specification: *"Only the
    /// literal boolean `true` disables a server."* A truthy string does not.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled == Some(true)
    }

    /// `definition.lifecycle ?? "lazy"`.
    #[must_use]
    pub fn lifecycle_mode(&self) -> ServerLifecycle {
        self.lifecycle.unwrap_or(ServerLifecycle::Lazy)
    }

    /// `definition.exposeResources !== false` — the predicate every read site uses.
    #[must_use]
    pub fn expose_resources(&self) -> bool {
        self.expose_resources != Some(false)
    }
}

/// `settings` — upstream `McpSettings`.
///
/// 23 keys upstream; `scriptMode` is **Cut 4** (the `mcpScript` JavaScript worker; RUST ONLY, there
/// is no JS runtime to host it), leaving the 22 below. `authRequiredMessage` is the one key the
/// published README does not document but which is live.
///
/// **`validateConfig` does not validate this block at all** — it is a bare cast — so every field
/// goes through [`lenient`] and every default lives in an accessor named for its read site
/// (MCP-066), not in a `Default` impl.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSettings {
    /// Default `"server"`. How every direct tool is named.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub tool_prefix: Option<ToolPrefix>,
    /// Default `true`. `=== false` swaps the footer's `"🔌 MCP: "` prefix for `"MCP: "` — the
    /// emoji is `U+1F50C ELECTRIC PLUG` followed by `U+0020`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub show_status_icon: Option<bool>,
    /// Default `"full"`. `"off"` clears the footer segment entirely.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub mcp_footer_status: Option<FooterStatus>,
    /// Default `true`, tested `!== false`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub notify_on_startup_connect: Option<bool>,
    /// Default `"off"`, via an explicit three-way test rather than a `??`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub host_config_discovery: Option<HostConfigDiscovery>,
    /// Directories handed to [`crate::agent_plugin`].
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub agent_plugin_paths: Option<Vec<String>>,
    /// Global idle timeout in **minutes**. Default `10`, via `typeof === "number" ? v : 10` — so an
    /// explicit `0` is honoured and disables the idle close.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<f64>,
    /// Global per-request timeout in **milliseconds**. Must be finite **and `> 0`**, else the SDK
    /// default stands.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub request_timeout_ms: Option<f64>,
    /// Default `false`. Per-server `directTools` that is merely *present* wins over this.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub direct_tools: Option<bool>,
    /// Default `true`, tested `!== false` (`types.ts:509`, `direct-tools.ts:227`; upstream
    /// `76a4ea3`, issue #358). Silences the "75+ direct tools resolved" advisory and **nothing
    /// else** — it is not a cap and never drops a spec, so the user who deliberately registered 75
    /// tools keeps all 75 and just stops being told about it once per resolve.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub warn_on_large_direct_tools: Option<bool>,
    /// Default `"compact"`, via `=== "boxed" ? "boxed" : "compact"`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub tool_result_rendering: Option<ToolResultRendering>,
    /// A **whitelist** of `1 | 2 | 3`, not a clamp: anything else falls back to the shell default
    /// (1 compact, 3 boxed).
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub collapsed_result_lines: Option<u8>,
    /// Global approval policy; a present per-server value wins.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub approve_tools: Option<BoolOrList>,
    /// Default `false`, tested `!== true` — the proxy tool survives unless this is literally
    /// `true`. **If HA-1 (late tool registration) is not built, this must be treated as
    /// unsupported**: on a cold cache the proxy tool is the *only* model-facing surface.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub disable_proxy_tool: Option<bool>,
    /// Default `false`, tested `=== true`. Freezes the direct-tool set after the first sync so a
    /// reconnect never rebuilds the system prompt — which is what keeps the provider's prompt-cache
    /// prefix valid across reconnects.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub freeze_direct_tools: Option<bool>,
    /// Default `false`, tested `!== true` ⇒ skip. Whether an unauthenticated call may start an
    /// OAuth flow by itself.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub auto_auth: Option<bool>,
    /// Sampling is wired only when `!== false && (has_ui || sampling_auto_approve)`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub sampling: Option<bool>,
    /// Default `false`, tested `=== true`. Also the gate that lets sampling work with no UI.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub sampling_auto_approve: Option<bool>,
    /// Elicitation is wired only when `!== false && has_ui`; the URL mode additionally requires
    /// TUI mode.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<bool>,
    /// `true` / `false` / explicit limits. Resolution is
    /// `enabled = envKillSwitch("MCP_OUTPUT_GUARD") ?? configured !== false` — **the env kill
    /// switch outranks the config in both directions**, so `MCP_OUTPUT_GUARD=1` re-enables a config
    /// that said `false`. Defaults: 51200 bytes / 2000 lines / 16384 detail bytes.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub output_guard: Option<OutputGuardSetting>,
    /// Protocol tracing. Disabled by default; the default destination is
    /// `<cwd>/.cyrup/mcp-traces/mcp-<ts>-<rand>.jsonl` (upstream writes `.pi/`).
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub trace: Option<TraceSettings>,
    /// Overrides the built-in "server requires OAuth" text. Formatted with
    /// `template.replaceAll("${server}", serverName)` — **every** occurrence, not the first.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub auth_required_message: Option<String>,
    /// OAuth storage root. `resolveConfiguredOAuthDir` **throws** `settings.oauthDir must be a
    /// string` on a non-string, treats blank as absent, and otherwise resolves it against the
    /// session cwd. `$MCP_OAUTH_DIR` outranks it.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub oauth_dir: Option<String>,
}

/// `DEFAULT_MCP_OUTPUT_MAX_BYTES = 50 * 1024` (`mcp-output-guard.ts`).
pub const DEFAULT_MCP_OUTPUT_MAX_BYTES: u64 = 50 * 1024;
/// `DEFAULT_MCP_OUTPUT_MAX_LINES = 2000`.
pub const DEFAULT_MCP_OUTPUT_MAX_LINES: u64 = 2000;
/// `DEFAULT_MCP_DETAILS_MAX_BYTES = 16 * 1024`.
pub const DEFAULT_MCP_DETAILS_MAX_BYTES: u64 = 16 * 1024;
/// `DEFAULT_MCP_TRACE_MAX_BYTES = 256 * 1024` (`mcp-trace.ts`).
pub const DEFAULT_MCP_TRACE_MAX_BYTES: u64 = 256 * 1024;
/// `DEFAULT_MCP_TRACE_MAX_EVENTS = 10_000`.
pub const DEFAULT_MCP_TRACE_MAX_EVENTS: u64 = 10_000;
/// `settings.idleTimeout`'s documented default, in minutes.
pub const DEFAULT_IDLE_TIMEOUT_MINUTES: f64 = 10.0;

/// `positiveInt(value, fallback)` (`mcp-output-guard.ts`) — a finite number, **floored**, and
/// strictly `> 0`; anything else is the constant default. Also `boundedPositiveInteger` in
/// `mcp-trace.ts`, which is the same predicate.
#[must_use]
pub fn positive_int(value: Option<f64>, fallback: u64) -> u64 {
    match value {
        Some(raw) if raw.is_finite() => {
            let floored = raw.floor();
            if floored > 0.0 && floored <= u64::MAX as f64 {
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                {
                    floored as u64
                }
            } else {
                fallback
            }
        }
        _ => fallback,
    }
}

/// `envKillSwitch(name)` (`mcp-output-guard.ts`) — a **tri-state** read of an environment variable:
/// trimmed and lowercased, `0/false/no/off` ⇒ `Some(false)`, `1/true/yes/on` ⇒ `Some(true)`, and
/// **anything else, including empty, ⇒ `None`** so the config keeps the decision.
///
/// The value is passed in rather than read here because the crate cannot mutate process env under
/// edition 2024, so a test has to be able to inject it (MCP-068).
#[must_use]
pub fn env_kill_switch(raw: Option<&str>) -> Option<bool> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "0" | "false" | "no" | "off" => Some(false),
        "1" | "true" | "yes" | "on" => Some(true),
        _ => None,
    }
}

/// `resolveMcpOutputGuardOptions`' answer: the guard's on/off state and its three limits, all
/// already defaulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedOutputGuard {
    /// `envKillSwitch("MCP_OUTPUT_GUARD") ?? configured !== false`.
    pub enabled: bool,
    /// Inline text bytes before truncation / spill.
    pub max_bytes: u64,
    /// Inline text lines before truncation / spill.
    pub max_lines: u64,
    /// `details.mcpResult` JSON bytes kept raw.
    pub details_max_bytes: u64,
}

impl McpSettings {
    /// `settings?.toolPrefix ?? "server"`.
    #[must_use]
    pub fn tool_prefix(&self) -> ToolPrefix {
        self.tool_prefix.unwrap_or(ToolPrefix::Server)
    }

    /// `showStatusIcon === false ? "MCP: " : "🔌 MCP: "` — so only a literal `false` drops the plug.
    #[must_use]
    pub fn show_status_icon(&self) -> bool {
        self.show_status_icon != Some(false)
    }

    /// `settings?.mcpFooterStatus ?? "full"`.
    #[must_use]
    pub fn mcp_footer_status(&self) -> FooterStatus {
        self.mcp_footer_status.unwrap_or(FooterStatus::Full)
    }

    /// `settings?.notifyOnStartupConnect !== false`.
    #[must_use]
    pub fn notify_on_startup_connect(&self) -> bool {
        self.notify_on_startup_connect != Some(false)
    }

    /// `getHostConfigDiscovery` — an explicit three-way test, defaulting `"off"`. The explicit form
    /// matters: an unknown string is **not** `"on"`, and the [`lenient`] parse has already turned
    /// one into `None`.
    #[must_use]
    pub fn host_config_discovery(&self) -> HostConfigDiscovery {
        self.host_config_discovery.unwrap_or(HostConfigDiscovery::Off)
    }

    /// `settings?.agentPluginPaths ?? []`.
    #[must_use]
    pub fn agent_plugin_paths(&self) -> &[String] {
        self.agent_plugin_paths.as_deref().unwrap_or(&[])
    }

    /// `typeof settings?.idleTimeout === "number" ? settings.idleTimeout : 10` — **`0` is
    /// honoured** and means "never idle out". A clamp here would silently re-enable the sweep.
    #[must_use]
    pub fn idle_timeout_minutes(&self) -> f64 {
        self.idle_timeout.filter(|value| value.is_finite()).unwrap_or(DEFAULT_IDLE_TIMEOUT_MINUTES)
    }

    /// `normalizeRequestTimeoutMs` — finite **and `> 0`**, else `undefined` (fall through to the
    /// SDK default).
    #[must_use]
    pub fn request_timeout_ms(&self) -> Option<f64> {
        self.request_timeout_ms.filter(|value| value.is_finite() && *value > 0.0)
    }

    /// `Boolean(settings?.directTools)` — truthiness, not presence. A per-server value that merely
    /// *exists* outranks this; that test lives at the per-server read site.
    #[must_use]
    pub fn direct_tools(&self) -> bool {
        self.direct_tools == Some(true)
    }

    /// `settings?.warnOnLargeDirectTools !== false` (`direct-tools.ts:227`). A literal `false` is
    /// the *only* value that silences the large-direct-tools advisory: an absent `settings` block,
    /// an absent key, and a non-boolean (already `None` after [`lenient`]) all still warn, which is
    /// what keeps the default loud for the accidental 75-tool set this advisory exists to catch.
    #[must_use]
    pub fn warn_on_large_direct_tools(&self) -> bool {
        self.warn_on_large_direct_tools != Some(false)
    }

    /// `settings?.toolResultRendering === "boxed" ? "boxed" : "compact"`.
    #[must_use]
    pub fn tool_result_rendering(&self) -> ToolResultRendering {
        match self.tool_result_rendering {
            Some(ToolResultRendering::Boxed) => ToolResultRendering::Boxed,
            _ => ToolResultRendering::Compact,
        }
    }

    /// `v === 1 || v === 2 || v === 3 ? v : (boxed ? 3 : 1)` — a **whitelist**, not a clamp, so
    /// `collapsedResultLines: 9` falls back to the shell default rather than to `3`.
    #[must_use]
    pub fn collapsed_result_lines(&self, boxed: bool) -> u8 {
        match self.collapsed_result_lines {
            Some(value @ 1..=3) => value,
            _ if boxed => 3,
            _ => 1,
        }
    }

    /// `settings?.approveTools` — handed out unresolved, because the decision is
    /// `definition.approveTools !== undefined ? definition.approveTools : settings.approveTools`
    /// and *presence* is what wins.
    #[must_use]
    pub fn approve_tools(&self) -> Option<&BoolOrList> {
        self.approve_tools.as_ref()
    }

    /// `settings?.disableProxyTool !== true` — the proxy tool survives anything but a literal
    /// `true`.
    #[must_use]
    pub fn proxy_tool_enabled(&self) -> bool {
        self.disable_proxy_tool != Some(true)
    }

    /// `settings?.freezeDirectTools === true`.
    #[must_use]
    pub fn freeze_direct_tools(&self) -> bool {
        self.freeze_direct_tools == Some(true)
    }

    /// `settings?.autoAuth !== true ⇒ skip`, i.e. the flow only runs on a literal `true`.
    #[must_use]
    pub fn auto_auth(&self) -> bool {
        self.auto_auth == Some(true)
    }

    /// `settings?.sampling !== false && (hasUI || samplingAutoApprove)` — the whole predicate,
    /// including the reason sampling can work headless.
    #[must_use]
    pub fn sampling(&self, has_ui: bool) -> bool {
        self.sampling != Some(false) && (has_ui || self.sampling_auto_approve())
    }

    /// `settings?.samplingAutoApprove === true`.
    #[must_use]
    pub fn sampling_auto_approve(&self) -> bool {
        self.sampling_auto_approve == Some(true)
    }

    /// `settings?.elicitation !== false && hasUI`.
    #[must_use]
    pub fn elicitation(&self, has_ui: bool) -> bool {
        self.elicitation != Some(false) && has_ui
    }

    /// `resolveMcpOutputGuardOptions(settings, env)` — `env` is `$MCP_OUTPUT_GUARD`, passed in so a
    /// test can inject it. **The kill switch outranks the config in both directions.**
    #[must_use]
    pub fn output_guard(&self, env: Option<&str>) -> ResolvedOutputGuard {
        let configured_enabled = !matches!(self.output_guard, Some(OutputGuardSetting::Enabled(false)));
        let limits = match &self.output_guard {
            Some(OutputGuardSetting::Limits(limits)) => Some(limits),
            _ => None,
        };
        ResolvedOutputGuard {
            enabled: env_kill_switch(env).unwrap_or(configured_enabled),
            max_bytes: positive_int(limits.and_then(|l| l.max_bytes), DEFAULT_MCP_OUTPUT_MAX_BYTES),
            max_lines: positive_int(limits.and_then(|l| l.max_lines), DEFAULT_MCP_OUTPUT_MAX_LINES),
            details_max_bytes: positive_int(
                limits.and_then(|l| l.details_max_bytes),
                DEFAULT_MCP_DETAILS_MAX_BYTES,
            ),
        }
    }

    /// `boundedPositiveInteger(settings.trace?.maxBytes, DEFAULT_MCP_TRACE_MAX_BYTES)`.
    ///
    /// Read by [`crate::runtime::initialize_mcp`] when it mints the generation's
    /// [`crate::trace::TraceWriter`].
    #[must_use]
    pub fn trace_max_bytes(&self) -> u64 {
        positive_int(self.trace.as_ref().and_then(|t| t.max_bytes), DEFAULT_MCP_TRACE_MAX_BYTES)
    }

    /// `boundedPositiveInteger(settings.trace?.maxEvents, DEFAULT_MCP_TRACE_MAX_EVENTS)`.
    ///
    /// Read by [`crate::runtime::initialize_mcp`] when it mints the generation's
    /// [`crate::trace::TraceWriter`].
    #[must_use]
    pub fn trace_max_events(&self) -> u64 {
        positive_int(self.trace.as_ref().and_then(|t| t.max_events), DEFAULT_MCP_TRACE_MAX_EVENTS)
    }

    /// `settings?.authRequiredMessage` — the template, not the formatted text. Formatting is
    /// `template.replaceAll("${server}", serverName)` and belongs to `utils.ts`'s
    /// `formatAuthRequiredMessage` (MCP-088).
    #[must_use]
    pub fn auth_required_message(&self) -> Option<&str> {
        self.auth_required_message.as_deref()
    }
}

// ===================================================================================================
// 5 · The enums `ServerEntry` and `McpSettings` are made of
// ===================================================================================================

/// `toolPrefix` — how a server's tools are named in the model-visible tool list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolPrefix {
    /// `<server>_<tool>` — the default.
    Server,
    /// `<tool>`, unprefixed. Collides readily; the collision resolver is what saves it.
    None,
    /// A shortened, sanitised server prefix.
    Short,
    /// A literal `mcp` prefix.
    Mcp,
}

/// `lifecycle` — when a server's process is spawned and how long it lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerLifecycle {
    /// Connected at load, and health-checked back up if it dies.
    KeepAlive,
    /// Connected on the first tool call. The default.
    Lazy,
    /// Connected on the first tool call, then kept alive — marked keep-alive only *after* that
    /// first successful connect (`markKeepAliveAfterConnect`), not at registration.
    LazyKeepAlive,
    /// Connected at load.
    Eager,
}

impl ServerLifecycle {
    /// `init.ts`'s `persistsAfterFirstSpawn` — `eager` or `lazy-keep-alive`. Such a server with no
    /// explicit `idleTimeout` gets **0**, i.e. never idles out (MCP-020).
    #[must_use]
    pub fn persists_after_first_spawn(self) -> bool {
        matches!(self, ServerLifecycle::Eager | ServerLifecycle::LazyKeepAlive)
    }

    /// Whether the startup pass should connect this server before the first prompt.
    #[must_use]
    pub fn is_prewarmed(self) -> bool {
        matches!(self, ServerLifecycle::Eager | ServerLifecycle::KeepAlive)
    }
}

/// `httpTransport`. The `sse` variant is retained **only** so a config carrying it can be rejected
/// with a named diagnostic — Cut 1 means it can never be connected (MCP-069).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HttpTransport {
    /// The only transport rmcp 3.1.2 implements client-side.
    StreamableHttp,
    /// **Cut 1.** Accepted by the parser so the load can say *why* it is refusing, then rejected.
    Sse,
}

/// `protocolVersion`. `undefined` and [`Legacy`](Self::Legacy) are byte-identical on the wire.
///
/// # The fourth variant, and why the enum is no longer closed
///
/// `resolveVersionNegotiation` (`server-manager.ts:82-95`) is a `switch` with a `default:` arm that
/// **throws** `` `Invalid MCP protocolVersion: ${String(definition.protocolVersion)}` `` — at
/// CONNECT time. `config.ts` validates the field not at all, and `computeServerHash`
/// (`metadata-cache.ts:104`) hashes whatever the file said, verbatim, long before any of that.
///
/// A three-variant closed enum behind [`lenient`] therefore got this wrong twice over. A real
/// revision like `"2025-06-18"` — exactly the shape of value a user pins — was silently discarded
/// by the deserialiser, so this crate hashed `"protocolVersion":undefined` while
/// `cyrup_ext_subagents::exec::mcp_direct_tools` (which holds the field raw, as upstream does)
/// hashed `"protocolVersion":"2025-06-18"`. The digests could never match, so every cache entry for
/// such a server was rejected and the server re-discovered its whole surface every session —
/// silently, and forever. And because the value was gone by parse time, the connect-time throw
/// upstream *does* perform never happened either: the server quietly negotiated as `legacy`.
///
/// [`Other`](Self::Other) fixes both halves. The value survives to the digest, and
/// [`crate::runtime::version_negotiation`] raises upstream's sentence on it — at connect, which is
/// where upstream raises it. **The deserialiser validates nothing**, which is the whole constraint:
/// a value it rejected could neither be hashed nor be reported.
///
/// `Copy` is gone with the payload; the two read sites take it by reference.
#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolVersionSetting {
    /// `"legacy"` — send no `versionNegotiation` at all. The default.
    Legacy,
    /// `"auto"` — rmcp `ClientLifecycleMode::Auto`: negotiate, with a legacy fallback.
    Auto,
    /// `"2026-07-28"` — pin the revision. Maps to rmcp `ClientLifecycleMode::Discover`.
    V20260728,
    /// Anything else the file contained, held **verbatim** for the digest.
    ///
    /// `switch` has no case for it, so it is `resolveVersionNegotiation`'s `default:` — a connect
    /// error, never a parse error.
    Other(RawJson),
}

impl ProtocolVersionSetting {
    /// `String(definition.protocolVersion)` — the interpolation in upstream's throw.
    ///
    /// Every form below was produced by running `` `Invalid MCP protocolVersion: ${String(v)}` ``
    /// on node 22, not reasoned about: `"2025-06-18"` → `2025-06-18`, `5` → `5`, `1.5` → `1.5`,
    /// `true` → `true`, `null` → `null`, `[1,2]` → `1,2`, `{a:1}` → `[object Object]`, `""` → the
    /// empty string. `Array.prototype.join` renders a `null` element as the empty string, which is
    /// why the array arm maps `Null` to `""` rather than to `"null"`.
    #[must_use]
    pub fn as_js_string(&self) -> String {
        match self {
            ProtocolVersionSetting::Legacy => "legacy".to_string(),
            ProtocolVersionSetting::Auto => "auto".to_string(),
            ProtocolVersionSetting::V20260728 => "2026-07-28".to_string(),
            ProtocolVersionSetting::Other(raw) => js_string(raw),
        }
    }
}

/// JS `String(value)` for a parsed JSON value — see [`ProtocolVersionSetting::as_js_string`], whose
/// throw is its only caller.
fn js_string(value: &RawJson) -> String {
    match value {
        RawJson::Null => "null".to_string(),
        RawJson::Bool(true) => "true".to_string(),
        RawJson::Bool(false) => "false".to_string(),
        RawJson::Number(number) => number.to_string(),
        RawJson::String(text) => text.clone(),
        // `Array.prototype.join(",")`, in which `null` and `undefined` render as the empty string.
        RawJson::Array(items) => items
            .iter()
            .map(|item| if matches!(item, RawJson::Null) { String::new() } else { js_string(item) })
            .collect::<Vec<_>>()
            .join(","),
        RawJson::Object(_) => "[object Object]".to_string(),
    }
}

impl Serialize for ProtocolVersionSetting {
    /// Round-trips the file: a known revision as its own token, anything else as it was written.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ProtocolVersionSetting::Legacy => serializer.serialize_str("legacy"),
            ProtocolVersionSetting::Auto => serializer.serialize_str("auto"),
            ProtocolVersionSetting::V20260728 => serializer.serialize_str("2026-07-28"),
            ProtocolVersionSetting::Other(raw) => raw.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ProtocolVersionSetting {
    /// **Total** — it accepts every JSON value there is, because upstream's `config.ts` accepts
    /// every JSON value there is. Hand-written rather than `#[serde(untagged)]` so the three known
    /// variants keep their names and their unit shape at every call site.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawJson::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            Some("legacy") => ProtocolVersionSetting::Legacy,
            Some("auto") => ProtocolVersionSetting::Auto,
            Some("2026-07-28") => ProtocolVersionSetting::V20260728,
            _ => ProtocolVersionSetting::Other(raw),
        })
    }
}

/// `auth: "oauth" | "bearer" | false`. Untagged with a `bool` arm because only the literal `false`
/// is legal — `true` is not a variant, it is a value the entry simply never satisfies.
///
/// # The third arm exists for the digest, and for nothing else
///
/// `computeServerHash` (`metadata-cache.ts:103`) folds `definition.auth` into the identity object
/// **verbatim**, and `config.ts` validates it not at all — every consumer is a `===` comparison
/// that an unknown value simply fails. Behind [`lenient`], a two-variant enum turned `auth:
/// "basic"` (or `5`, or an object) into `None`, so this crate hashed `"auth":undefined` where
/// `cyrup_ext_subagents::exec::mcp_direct_tools`, which holds the field raw, hashed the value —
/// and the two digests could never match for such a server.
///
/// [`Other`](Self::Other) is last in the untagged list, so `"oauth"`, `"bearer"` and every boolean
/// still land on the two arms that have meaning; it catches only what those reject. Every consumer
/// tests equality against a named variant ([`crate::oauth`], [`crate::secrets`]), and
/// `Other` equals none of them — which is exactly what a TypeScript `===` against an unknown
/// string does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AuthMode {
    /// `"oauth"` / `"bearer"`.
    Named(AuthKind),
    /// The literal `false` — and `true`, which is tolerated exactly as TypeScript's structural cast
    /// tolerates it and satisfies no read site.
    Disabled(bool),
    /// Anything else the file contained, held **verbatim** for the digest and matched by nothing.
    Other(RawJson),
}

/// The two named `auth` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    /// Full OAuth 2.1, through rmcp's `AuthorizationManager`.
    Oauth,
    /// A static bearer token from [`ServerEntry::bearer_token`] / `bearer_token_env`.
    Bearer,
}

/// `oauth: OAuthConfig | false`.
///
/// `large_enum_variant` is allowed rather than boxed: the enum lives inside a [`ServerEntry`] that
/// is itself `Option`-wrapped and cloned once per merge, so a `Box` would trade ~150 stack bytes on
/// a config struct for a heap allocation on every entry that carries OAuth — and the unboxed shape
/// is the published one, named in the crate's public contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum OAuthSetting {
    /// The literal `false`, which is the only boolean with meaning here. Listed **first** so an
    /// untagged match tries it before the all-optional-fields struct, which would otherwise
    /// swallow… nothing (a bool is not a map), but the order documents the intent.
    Disabled(bool),
    /// An explicit client configuration.
    Config(OAuthConfig),
}

/// Per-server OAuth client configuration — upstream `types.ts` `OAuthConfig`, **ten** fields
/// (MCP-069).
///
/// Every field is optional, so an empty object is a legal (and meaningless) value. The two the
/// merge cares about are none of them: `oauth` is stripped or kept **whole** by [`merge_entry`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthConfig {
    /// `"authorization_code"` (the default) or `"client_credentials"`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub grant_type: Option<OAuthGrantType>,
    /// A pre-registered client id. Absent ⇒ dynamic client registration.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// The confidential-client secret. Reaches `resolveCommandSecret` with the context
    /// `` `MCP server "${serverName}" OAuth clientSecret` ``, so a `!command` form is live here.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Requested scopes, space-separated in one string (upstream's shape, not an array).
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Extra authorization-URL parameters for provider-specific extensions. Flow-owned parameters
    /// cannot be overridden.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub authorization_params: Option<BTreeMap<String, String>>,
    /// The exact redirect URI a pre-registered client is registered with.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    /// Client display name for dynamic registration; defaults to [`crate::dirs::APP_NAME`].
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    /// Client homepage for dynamic registration; defaults to [`crate::dirs::APP_CLIENT_URI`].
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub client_uri: Option<String>,
    /// Logo shown on the provider's consent screen.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<String>,
    /// A security-weakening escape hatch for known-misconfigured authorization servers.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub skip_issuer_metadata_validation: Option<bool>,
}

/// `OAuthConfig.grantType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthGrantType {
    /// The default: browser round-trip with PKCE.
    AuthorizationCode,
    /// Machine-to-machine; no browser, no callback listener.
    ClientCredentials,
}

/// `boolean | string[]` — `directTools` and `approveTools`. The distinction that matters is
/// *presence*, not truthiness: a per-server value that exists at all overrides the global.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BoolOrList {
    /// `true` / `false`.
    All(bool),
    /// An explicit name list.
    Named(Vec<String>),
}

/// `outputGuard: boolean | {maxBytes, maxLines, detailsMaxBytes}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutputGuardSetting {
    /// `true` (defaults) / `false` (off — unless `MCP_OUTPUT_GUARD=1` overrides it).
    Enabled(bool),
    /// Explicit limits. Each knob goes through [`positive_int`].
    Limits(OutputGuardLimits),
}

/// The three `outputGuard` knobs. Defaults: 51200 / 2000 / 16384.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputGuardLimits {
    /// [`DEFAULT_MCP_OUTPUT_MAX_BYTES`].
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<f64>,
    /// [`DEFAULT_MCP_OUTPUT_MAX_LINES`].
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub max_lines: Option<f64>,
    /// [`DEFAULT_MCP_DETAILS_MAX_BYTES`].
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub details_max_bytes: Option<f64>,
}

/// `settings.trace` — the protocol trace writer's configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSettings {
    /// Tested `=== true`; a per-server `trace` overrides it.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Destination file. Default `<cwd>/.cyrup/mcp-traces/mcp-<ts>-<rand>.jsonl`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// [`DEFAULT_MCP_TRACE_MAX_BYTES`], via `boundedPositiveInteger`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<f64>,
    /// [`DEFAULT_MCP_TRACE_MAX_EVENTS`], via `boundedPositiveInteger`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub max_events: Option<f64>,
}

/// `hostConfigDiscovery` — whether to look for other agent tools' MCP configs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostConfigDiscovery {
    /// The default.
    Off,
    /// Discover, then ask before importing.
    Prompt,
    /// Discover and import.
    On,
}

/// `mcpFooterStatus` — the footer segment's verbosity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FooterStatus {
    /// Server names and counts. The default.
    Full,
    /// Counts only.
    Compact,
    /// No footer segment at all (`formatMcpStatus` returns `undefined`).
    Off,
}

/// `toolResultRendering` — which result shell the renderer draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolResultRendering {
    /// One-line summary, 1 collapsed line by default. The default.
    Compact,
    /// A bordered block, 3 collapsed lines by default.
    Boxed,
}

// ===================================================================================================
// 6 · Path helpers — Node's `path.resolve` and `os.homedir`
// ===================================================================================================

/// Node's `path.resolve(base, candidate)`: an absolute `candidate` wins outright, otherwise it is
/// joined onto `base`, and the result is **lexically normalised** (`.` dropped, `..` popped) —
/// which is why this is not simply `Path::join`.
#[must_use]
pub fn resolve_from(base: &Path, candidate: &str) -> PathBuf {
    let candidate_path = Path::new(candidate);
    let joined =
        if candidate_path.is_absolute() { candidate_path.to_path_buf() } else { base.join(candidate_path) };
    normalize_lexical(&joined)
}

/// Lexical `..`/`.` collapse, **without** touching the filesystem — Node's `resolve` does not
/// resolve symlinks either, and a port that used `canonicalize` would fail on a path that does not
/// exist yet (which is every path a writer is about to create).
#[must_use]
pub fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() { PathBuf::from(".") } else { out }
}

/// `os.homedir()`, with the same `CYRUP_HOME`-first override every other in-tree copy uses
/// (`cyrup_ext_subagents::background::home_dir`). The override is what makes the six-source ladder
/// testable: a test that points `CYRUP_HOME` at a `TempDir` moves `~/.config/mcp/mcp.json`,
/// `~/.agents/…` and all seven import families with it.
#[must_use]
pub fn home_dir() -> PathBuf {
    std::env::var_os("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

// ===================================================================================================
// 7 · Parsing and lenient validation (MCP-051, MCP-054, MCP-069)
// ===================================================================================================

/// One thing the load noticed and carried on past.
///
/// Upstream these are bare `console.warn`s in `config.ts` — **not** routed through `logger.ts`, so
/// they are unfiltered and are not level-gated (13b §11). The port keeps both channels distinct:
/// these are values a panel can render, and they are also emitted through `tracing::warn!`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiagnostic {
    /// The file the problem was found in.
    pub path: PathBuf,
    /// The `mcpServers` key, when the problem is about one entry.
    pub server: Option<String>,
    /// User-facing text.
    pub message: String,
}

impl fmt::Display for ConfigDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// What one source contributed, plus everything the load had to complain about on the way.
#[derive(Debug, Clone, Default)]
pub struct LoadedConfig {
    /// The merged configuration. Never an `Err`, ever (MCP-003).
    pub config: McpConfig,
    /// Named diagnostics, in discovery order.
    pub diagnostics: Vec<ConfigDiagnostic>,
}

/// `getConfigPathFromArgv(argv)` — `utils.ts`.
///
/// Scans for the **exact token** `--mcp-config` and takes the following element. Reproduced
/// including its limitation: `--mcp-config=path` is **not** supported and yields `None`, and a
/// trailing `--mcp-config` with nothing after it likewise yields `None`.
///
/// This is read from argv directly rather than through the flag store, and that is not a
/// workaround: `ExtensionHost::apply_extension_flag_values` runs *after* the native-load loop on
/// both sides, so `init` cannot see the flag store on either. The flag is still *registered*
/// through `InitApi::register_flag` — an unreconciled `--flag` is itself a startup diagnostic
/// (MCP-002).
#[must_use]
pub fn config_path_from_argv<I, S>(argv: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut iter = argv.into_iter();
    while let Some(arg) = iter.next() {
        if arg.as_ref() == "--mcp-config" {
            return iter.next().map(|v| v.as_ref().to_string());
        }
    }
    None
}

/// `parseJsonConfig(raw)` = `JSON.parse(stripJsonComments(raw, { trailingCommas: true }))`
/// (MCP-051).
///
/// JSONC comes from `cyrup_permission_system::jsonc` — the same parser
/// `cyrup_permission_system::manager::read_configured_mcp_server_names` runs over this exact file,
/// so the permission gate and the adapter agree about which servers exist **by construction**.
/// Re-porting `strip-json-comments` here would let the two disagree, silently.
pub fn parse_json_config(raw: &str, path: &str) -> Result<RawJson, String> {
    cyrup_permission_system::jsonc::parse_config_into::<RawJson>(raw, path, "MCP config")
}

/// The typed read of one JSONC config document, split out so the source ladder can apply it per
/// source and so a test can drive it without a file.
///
/// Accepts **both** `mcpServers` and the legacy `mcp-servers` key, exactly as `cli.js` does; the
/// legacy key is normalised away on write, never on read. Non-string `imports` entries are filtered
/// out rather than rejecting the file.
///
/// # Do not route this through `serde_json::Value`
///
/// It is the obvious refactor and it is silently wrong. `serde_json::Map` is a `BTreeMap` under this
/// workspace's feature set (`preserve_order` is off), so parsing to a `Value` and then
/// deserialising out of it **sorts the server keys** — losing the file order that decides connect
/// order, `/mcp` listing order and the tool-name collision tie-break. [`RawJson`] and [`IndexMap`]
/// keep document order end to end.
///
/// **No production caller.** The load ladder reads files through [`read_validated_config`], which
/// swallows a parse failure into a diagnostic and carries on; this is the variant that *reports* it
/// to the caller, which is what an editor of a single document wants — and that caller is the `/mcp`
/// panel, `TODO(MCP-394)`.
pub fn parse_config_document(raw: &str, path: &str) -> Result<McpConfig, String> {
    let document = parse_json_config(raw, path)?;
    Ok(validate_config(&document, Path::new(path), &mut Vec::new()))
}

/// `validateConfig(raw)` — **lenient by design**.
///
/// A non-object root yields `{ mcpServers: {} }`; servers come from `raw.mcpServers ?? raw["mcp-servers"]`;
/// `imports` is kept only if `Array.isArray`; `settings` is kept with **no validation at all**.
/// Nothing here can fail, which is the load-bearing half of MCP-003.
#[must_use]
pub fn validate_config(
    raw: &RawJson,
    path: &Path,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> McpConfig {
    let Some(root) = raw.as_object() else {
        return McpConfig::default();
    };
    let servers = root
        .get("mcpServers")
        .or_else(|| root.get("mcp-servers"))
        .map_or_else(IndexMap::new, |value| to_server_entries(value, path, diagnostics));
    let imports = root
        .get("imports")
        .and_then(RawJson::as_array)
        .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let settings = root.get("settings").and_then(raw_to::<McpSettings>);
    McpConfig { mcp_servers: servers, settings, imports }
}

/// `toServerEntries(servers)` — keep an entry **iff** it is a non-array object; a malformed entry is
/// dropped and the file survives.
///
/// Two additions the port owes the cuts (MCP-054 / MCP-069): an entry carrying `socket` (Cut 3) or
/// `httpTransport: "sse"` (Cut 1) is rejected here with a **named diagnostic** rather than silently
/// losing the key. Rejecting the whole entry — rather than dropping just the field — is deliberate:
/// it leaves any lower-precedence definition of that server intact instead of half-overriding it
/// with a definition whose transport this build cannot honour.
#[must_use]
pub fn to_server_entries(
    servers: &RawJson,
    path: &Path,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> IndexMap<String, ServerEntry> {
    let Some(entries) = servers.as_object() else {
        return IndexMap::new();
    };
    let mut out = IndexMap::with_capacity(entries.len());
    for (name, raw_entry) in entries {
        if raw_entry.as_object().is_none() {
            continue;
        }
        if raw_entry.get("socket").is_some() {
            let message = format!(
                "MCP server \"{name}\" in {} configures `socket`, the raw unix-socket transport, \
                 which this build does not implement (Cut 3); the entry is ignored.",
                path.display()
            );
            tracing::warn!("{message}");
            diagnostics.push(ConfigDiagnostic {
                path: path.to_path_buf(),
                server: Some(name.clone()),
                message,
            });
            continue;
        }
        if raw_entry.get("httpTransport").and_then(RawJson::as_str) == Some("sse") {
            let message = format!(
                "MCP server \"{name}\" in {} requests `httpTransport: \"sse\"`; rmcp ships no SSE \
                 client transport (Cut 1), so the entry is ignored.",
                path.display()
            );
            tracing::warn!("{message}");
            diagnostics.push(ConfigDiagnostic {
                path: path.to_path_buf(),
                server: Some(name.clone()),
                message,
            });
            continue;
        }
        // MCP-302 — the twelve `extractOAuthConfig` guards, run over the **raw** block.
        //
        // This has to happen here, before `raw_to::<ServerEntry>`, because every `ServerEntry`
        // field is `lenient`: a `clientId: 42` is silently *dropped* by the deserializer, so the
        // server would go on to authenticate as an anonymous client instead of reporting
        // `OAuth clientId must be a string`. Nine of the twelve messages are unreachable any other
        // way. The whole entry is rejected, matching the `socket` and `httpTransport` arms above
        // and for the same reason: a half-honoured override is worse than none.
        if let Some(raw_oauth) = raw_entry.get("oauth")
            && let Err(error) = crate::oauth::validate_oauth_block(raw_oauth)
        {
            let message = format!(
                "MCP server \"{name}\" in {}: {error}; the entry is ignored.",
                path.display()
            );
            tracing::warn!("{message}");
            diagnostics.push(ConfigDiagnostic {
                path: path.to_path_buf(),
                server: Some(name.clone()),
                message,
            });
            continue;
        }
        // Cannot fail: the root is an object and every field is `lenient`.
        if let Some(entry) = raw_to::<ServerEntry>(raw_entry) {
            out.insert(name.clone(), entry);
        }
    }
    out
}

/// `readValidatedConfig(path, label)` — non-existent ⇒ `None`; a parse throw ⇒
/// `` console.warn(`Failed to load ${label}:`, error) `` and `None`. `label` is always
/// `` `MCP config from ${path}` ``.
#[must_use]
pub fn read_validated_config(
    path: &Path,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Option<McpConfig> {
    if !path.exists() {
        return None;
    }
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            push_load_warning(path, &error.to_string(), diagnostics);
            return None;
        }
    };
    match parse_json_config(&raw, &path.to_string_lossy()) {
        Ok(document) => Some(validate_config(&document, path, diagnostics)),
        Err(error) => {
            push_load_warning(path, &error, diagnostics);
            None
        }
    }
}

/// `` console.warn(`Failed to load MCP config from ${path}:`, error) `` — the exact wording of the
/// one warning every ladder source shares.
fn push_load_warning(path: &Path, error: &str, diagnostics: &mut Vec<ConfigDiagnostic>) {
    let message = format!("Failed to load MCP config from {}: {error}", path.display());
    tracing::warn!("{message}");
    diagnostics.push(ConfigDiagnostic { path: path.to_path_buf(), server: None, message });
}

// ===================================================================================================
// 8 · The merge — MCP-053 (critical), MCP-055, MCP-067
// ===================================================================================================

/// `URL_BOUND_AUTH_FIELDS` — the credential-bearing fields whose value is bound to a specific
/// server `url`.
///
/// **This constant is the checklist, not the enforcement.** [`merge_entry`] clears these fields by
/// name on the typed struct and never reads this list; what actually stops a fifth URL-bound field
/// from quietly bypassing the strip is `merge_entry`'s destructure of `over`, written deliberately
/// **without** a `..` rest pattern so adding a field to [`ServerEntry`] is a compile error there.
/// The compile error tells you to handle the new field; this list is the second place you must then
/// add it, and the exhaustiveness test in this module is what checks you did.
pub const URL_BOUND_AUTH_FIELDS: [&str; 4] =
    ["headers", "bearerToken", "bearerTokenEnv", "requestHeadersCommand"];

/// `mergeServerMaps`' per-entry half — **the security core of this module** (MCP-053).
///
/// Upstream's comment block names the threat verbatim: without the URL-binding rule, *"a
/// higher-precedence source that supplies only a new `url` … would otherwise retain the
/// lower-precedence entry's auth material … and send it to the new url — a credential-exfiltration
/// vector when the higher-precedence source is less trusted than the one that first defined the
/// server."* A port that models the server table as `HashMap<String, ServerEntry>` and calls
/// `.extend()` loses this while passing every functional test.
///
/// The algorithm, in order:
///
/// 1. *(Cut 3, MCP-054)* the two `socket` transport-swap rules. `socket` is not a field, so neither
///    rule has an input; a `socket` entry never reaches here because [`to_server_entries`] rejected
///    it with a named diagnostic.
/// 2. **URL-bound credential stripping.** When `existing` is present, `over.url` is a string, and it
///    **differs** from `existing.url`: clear [`URL_BOUND_AUTH_FIELDS`], and clear `oauth` **unless
///    it is the literal `false`** — an explicit disable is not credential material, so it survives a
///    URL change. Note that `oauth: true` does *not* survive: upstream's test is
///    `baseEntry.oauth !== false`, and `true` fails it.
/// 3. `{ ...baseEntry, ...definition }` — a **shallow, per-field** spread. A partial override
///    inherits every field it does not mention; credentials re-supplied by `definition` still apply
///    because `definition` spreads last.
///
/// The `command` ⇄ `url` case is deliberately *not* handled: upstream v2.25.0 does not handle it
/// either, so a base `{command}` overridden by `{url}` produces a two-transport entry that parses
/// fine and fails at connect with [`crate::runtime::select_transport`]'s message.
///
/// # Exhaustiveness
///
/// The body destructures `over` **without** a `..` rest pattern. Adding a field to [`ServerEntry`]
/// is therefore a compile error here, which is the only way to guarantee a new credential-bearing
/// field cannot quietly bypass step 2.
#[must_use]
pub fn merge_entry(base: Option<&ServerEntry>, over: &ServerEntry) -> ServerEntry {
    let mut base_entry = base.cloned().unwrap_or_default();

    if base.is_some()
        && let Some(next_url) = over.url.as_ref()
        && base_entry.url.as_ref() != Some(next_url)
    {
        base_entry.headers = None;
        base_entry.bearer_token = None;
        base_entry.bearer_token_env = None;
        // v2.26.0 (`config.ts:474`): a per-request signing command is bound to the endpoint it was
        // configured for just as tightly as a static header is, so it goes with them.
        base_entry.request_headers_command = None;
        if base_entry.oauth != Some(OAuthSetting::Disabled(false)) {
            base_entry.oauth = None;
        }
    }

    let ServerEntry {
        command,
        args,
        env,
        cwd,
        url,
        headers,
        request_headers_command,
        auth,
        bearer_token,
        bearer_token_env,
        oauth,
        lifecycle,
        idle_timeout,
        request_timeout_ms,
        expose_resources,
        direct_tools,
        tool_prefix,
        include_tools,
        exclude_tools,
        search_keywords,
        approve_tools,
        debug,
        trace,
        http_transport,
        plugin_data_dir,
        literal_env,
        protocol_version,
        disabled,
    } = over;

    ServerEntry {
        command: command.clone().or(base_entry.command),
        args: args.clone().or(base_entry.args),
        env: env.clone().or(base_entry.env),
        cwd: cwd.clone().or(base_entry.cwd),
        url: url.clone().or(base_entry.url),
        headers: headers.clone().or(base_entry.headers),
        request_headers_command: request_headers_command
            .clone()
            .or(base_entry.request_headers_command),
        auth: auth.clone().or(base_entry.auth),
        bearer_token: bearer_token.clone().or(base_entry.bearer_token),
        bearer_token_env: bearer_token_env.clone().or(base_entry.bearer_token_env),
        oauth: oauth.clone().or(base_entry.oauth),
        lifecycle: lifecycle.or(base_entry.lifecycle),
        idle_timeout: idle_timeout.or(base_entry.idle_timeout),
        request_timeout_ms: request_timeout_ms.or(base_entry.request_timeout_ms),
        expose_resources: expose_resources.or(base_entry.expose_resources),
        direct_tools: direct_tools.clone().or(base_entry.direct_tools),
        tool_prefix: tool_prefix.or(base_entry.tool_prefix),
        include_tools: include_tools.clone().or(base_entry.include_tools),
        exclude_tools: exclude_tools.clone().or(base_entry.exclude_tools),
        search_keywords: search_keywords.clone().or(base_entry.search_keywords),
        approve_tools: approve_tools.clone().or(base_entry.approve_tools),
        debug: debug.or(base_entry.debug),
        trace: trace.or(base_entry.trace),
        http_transport: http_transport.or(base_entry.http_transport),
        plugin_data_dir: plugin_data_dir.clone().or(base_entry.plugin_data_dir),
        literal_env: literal_env.or(base_entry.literal_env),
        protocol_version: protocol_version.clone().or(base_entry.protocol_version),
        disabled: disabled.or(base_entry.disabled),
    }
}

/// `mergeServerMaps(base, next)` — fold `next` over `base` through [`merge_entry`].
///
/// `IndexMap::insert` on an existing key keeps that key's original **position**, which is exactly
/// what `merged[name] = …` does to a JS object; a new key appends. That is why the merged table's
/// order is "first source that mentioned each server", not "last".
#[must_use]
pub fn merge_server_maps(
    base: &IndexMap<String, ServerEntry>,
    next: &IndexMap<String, ServerEntry>,
) -> IndexMap<String, ServerEntry> {
    let mut merged = base.clone();
    for (name, definition) in next {
        let entry = merge_entry(merged.get(name), definition);
        merged.insert(name.clone(), entry);
    }
    merged
}

/// `mergeImports(left, right)` — concat then `Set` dedup, preserving first-seen order.
#[must_use]
pub fn merge_imports(left: &[String], right: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(left.len() + right.len());
    for kind in left.iter().chain(right.iter()) {
        if !out.iter().any(|seen| seen == kind) {
            out.push(kind.clone());
        }
    }
    out
}

/// `mergeSettings` — the **one-level** key merge inside `mergeConfigs` (MCP-067).
///
/// `{ ...base.settings, ...next.settings }`. One level, so `settings.trace` and
/// `settings.outputGuard` **objects are replaced wholesale**, never deep-merged: a project file
/// setting `trace: { enabled: false }` discards the global's `maxBytes`. Explicitly *not*
/// `cyrup_config::settings`'s `deep_merge`, which recurses into objects, and explicitly not
/// `next.settings.or(base.settings)` — the in-tree `mcp_direct_tools::merge_configs` does that, and
/// a project file setting only `toolPrefix` would discard every global setting (MCP-094).
#[must_use]
pub fn merge_settings(base: Option<&McpSettings>, next: Option<&McpSettings>) -> Option<McpSettings> {
    let Some(next) = next else {
        return base.cloned();
    };
    let base = base.cloned().unwrap_or_default();
    Some(McpSettings {
        tool_prefix: next.tool_prefix.or(base.tool_prefix),
        show_status_icon: next.show_status_icon.or(base.show_status_icon),
        mcp_footer_status: next.mcp_footer_status.or(base.mcp_footer_status),
        notify_on_startup_connect: next.notify_on_startup_connect.or(base.notify_on_startup_connect),
        host_config_discovery: next.host_config_discovery.or(base.host_config_discovery),
        agent_plugin_paths: next.agent_plugin_paths.clone().or(base.agent_plugin_paths),
        idle_timeout: next.idle_timeout.or(base.idle_timeout),
        request_timeout_ms: next.request_timeout_ms.or(base.request_timeout_ms),
        direct_tools: next.direct_tools.or(base.direct_tools),
        warn_on_large_direct_tools: next
            .warn_on_large_direct_tools
            .or(base.warn_on_large_direct_tools),
        tool_result_rendering: next.tool_result_rendering.or(base.tool_result_rendering),
        collapsed_result_lines: next.collapsed_result_lines.or(base.collapsed_result_lines),
        approve_tools: next.approve_tools.clone().or(base.approve_tools),
        disable_proxy_tool: next.disable_proxy_tool.or(base.disable_proxy_tool),
        freeze_direct_tools: next.freeze_direct_tools.or(base.freeze_direct_tools),
        auto_auth: next.auto_auth.or(base.auto_auth),
        sampling: next.sampling.or(base.sampling),
        sampling_auto_approve: next.sampling_auto_approve.or(base.sampling_auto_approve),
        elicitation: next.elicitation.or(base.elicitation),
        output_guard: next.output_guard.clone().or(base.output_guard),
        trace: next.trace.clone().or(base.trace),
        auth_required_message: next.auth_required_message.clone().or(base.auth_required_message),
        oauth_dir: next.oauth_dir.clone().or(base.oauth_dir),
    })
}

/// `mergeConfigs(base, next)` — imports, then settings, then the server table.
#[must_use]
pub fn merge_configs(base: &McpConfig, next: &McpConfig) -> McpConfig {
    McpConfig {
        mcp_servers: merge_server_maps(&base.mcp_servers, &next.mcp_servers),
        settings: merge_settings(base.settings.as_ref(), next.settings.as_ref()),
        imports: merge_imports(&base.imports, &next.imports),
    }
}

// ===================================================================================================
// 9 · The seven host-config import families (MCP-056, MCP-057)
// ===================================================================================================

/// One imported host config that actually exists on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedDocument {
    /// The candidate that won — for `opencode` this is `highestPrecedencePath`, the *last* existing
    /// candidate rather than the first.
    pub path: PathBuf,
    /// The parsed document.
    pub value: RawJson,
}

/// `IMPORT_PATHS[kind]`, resolved against a home directory and a cwd (`resolveImportCandidates`).
///
/// A candidate starting with `.` is `resolve(cwd, candidate)`; every other candidate is a
/// home-relative path built here. `./opencode.json` is the one special case — see
/// [`resolve_opencode_project_candidate`].
#[must_use]
pub fn resolve_import_candidates(kind: ImportKind, home: &Path, cwd: &Path) -> Vec<PathBuf> {
    match kind {
        ImportKind::Cursor => vec![home.join(".cursor").join("mcp.json")],
        ImportKind::ClaudeCode => vec![
            home.join(".claude").join("mcp.json"),
            home.join(".claude.json"),
            home.join(".claude").join("claude_desktop_config.json"),
        ],
        ImportKind::ClaudeDesktop => vec![
            home.join("Library")
                .join("Application Support")
                .join("Claude")
                .join("claude_desktop_config.json"),
        ],
        ImportKind::Codex => {
            vec![home.join(".codex").join("config.toml"), home.join(".codex").join("config.json")]
        }
        ImportKind::Opencode => vec![
            home.join(".config").join("opencode").join("opencode.json"),
            resolve_opencode_project_candidate(cwd),
        ],
        ImportKind::Windsurf => vec![home.join(".windsurf").join("mcp.json")],
        ImportKind::Vscode => vec![resolve_from(cwd, ".vscode/mcp.json")],
    }
}

/// OpenCode's `./opencode.json`: walk **up** from `cwd` for a `.git` directory to find the git root,
/// then walk **down-to-up** from `cwd` returning the first existing `opencode.json`, stopping at the
/// git root. With no git root at all, `join(start, "opencode.json")`.
///
/// The two-phase shape matters: without a git root the walk never happens, so a home directory full
/// of unrelated `opencode.json` files is never picked up from a non-repo cwd.
#[must_use]
pub fn resolve_opencode_project_candidate(cwd: &Path) -> PathBuf {
    let start = normalize_lexical(cwd);
    let mut git_root: Option<PathBuf> = None;
    let mut current = start.clone();
    loop {
        if current.join(".git").exists() {
            git_root = Some(current.clone());
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }

    let Some(git_root) = git_root else {
        return start.join("opencode.json");
    };

    let mut current = start;
    loop {
        let project_config = current.join("opencode.json");
        if project_config.exists() || current == git_root {
            return project_config;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            // Defensive: `git_root` is an ancestor of `start`, so this is unreachable. Returning
            // the git-root candidate keeps the loop total rather than trusting that argument.
            _ => return git_root.join("opencode.json"),
        }
    }
}

/// `readImportedConfig(path)` = `path.endsWith(".toml") ? parseToml(raw) : parseJsonConfig(raw)`.
/// The only TOML path in the whole package is `~/.codex/config.toml`.
fn read_imported_config(path: &Path) -> Result<RawJson, String> {
    let raw = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("toml")) {
        // `toml`'s deserializer feeds map entries in document order, so `RawJson` keeps it.
        toml::from_str::<RawJson>(&raw).map_err(|error| error.to_string())
    } else {
        parse_json_config(&raw, &path.to_string_lossy())
    }
}

/// `loadImportedConfig(kind, cwd, warningPrefix)`.
///
/// Every family takes the **first existing** candidate — except `opencode`, which merges **all**
/// existing candidates through [`merge_opencode_configs`] and reports the *last* one as its path.
#[must_use]
pub fn load_imported_config(
    kind: ImportKind,
    home: &Path,
    cwd: &Path,
    warning_prefix: &str,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Option<ImportedDocument> {
    let candidates = resolve_import_candidates(kind, home, cwd);

    if kind == ImportKind::Opencode {
        let mut merged = IndexMap::new();
        let mut highest: Option<PathBuf> = None;
        for path in candidates {
            if !path.exists() {
                continue;
            }
            match read_imported_config(&path) {
                Ok(value) => {
                    if let Some(entries) = value.as_object() {
                        merged = merge_opencode_configs(&merged, entries);
                        highest = Some(path);
                    }
                }
                Err(error) => push_import_warning(&path, warning_prefix, &error, diagnostics),
            }
        }
        return highest.map(|path| ImportedDocument { path, value: RawJson::Object(merged) });
    }

    for path in candidates {
        if !path.exists() {
            continue;
        }
        match read_imported_config(&path) {
            Ok(value) => return Some(ImportedDocument { path, value }),
            Err(error) => push_import_warning(&path, warning_prefix, &error, diagnostics),
        }
    }
    None
}

/// `resolveImportPath(kind, cwd)` — the path only, for the setup panel's "detected" list.
#[must_use]
pub fn resolve_import_path(
    kind: ImportKind,
    home: &Path,
    cwd: &Path,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Option<PathBuf> {
    load_imported_config(
        kind,
        home,
        cwd,
        &format!("Failed to discover imported MCP config from {kind}:"),
        diagnostics,
    )
    .map(|imported| imported.path)
}

fn push_import_warning(
    path: &Path,
    prefix: &str,
    error: &str,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) {
    let message = format!("{prefix} {error}");
    tracing::warn!("{message}");
    diagnostics.push(ConfigDiagnostic { path: path.to_path_buf(), server: None, message });
}

/// `mergeOpenCodeConfigs(base, next)` — MCP-053's credential-unbinding discipline, repeated on a
/// **different schema** (MCP-057).
///
/// Per `mcp` entry, in order: a changed `type` deletes `command, environment, cwd, url, headers,
/// oauth`; a changed `url` deletes `headers` and `oauth`; a changed `command` **array** deletes
/// `environment` and `cwd`. Then the override spreads over what survives, and
/// `environment`/`headers`/`oauth` are **one-level object-merged** on top of that — which is why
/// this cannot be a generic JSON deep-merge, and why it cannot be [`merge_entry`] either.
#[must_use]
pub fn merge_opencode_configs(
    base: &IndexMap<String, RawJson>,
    next: &IndexMap<String, RawJson>,
) -> IndexMap<String, RawJson> {
    let base_mcp = base.get("mcp").and_then(RawJson::as_object).cloned().unwrap_or_default();
    let mut merged_mcp = base_mcp;

    if let Some(next_mcp) = next.get("mcp").and_then(RawJson::as_object) {
        for (name, next_entry) in next_mcp {
            let (Some(base_fields), Some(override_fields)) =
                (merged_mcp.get(name).and_then(RawJson::as_object), next_entry.as_object())
            else {
                merged_mcp.insert(name.clone(), next_entry.clone());
                continue;
            };
            let mut safe_base = base_fields.clone();

            if let Some(next_type) = override_fields.get("type").and_then(RawJson::as_str)
                && safe_base.get("type").and_then(RawJson::as_str) != Some(next_type)
            {
                for field in ["command", "environment", "cwd", "url", "headers", "oauth"] {
                    safe_base.shift_remove(field);
                }
            }
            if let Some(next_url) = override_fields.get("url").and_then(RawJson::as_str)
                && safe_base.get("url").and_then(RawJson::as_str) != Some(next_url)
            {
                safe_base.shift_remove("headers");
                safe_base.shift_remove("oauth");
            }
            if let Some(next_command) = override_fields.get("command").and_then(RawJson::as_array) {
                let changed = safe_base
                    .get("command")
                    .and_then(RawJson::as_array)
                    .is_none_or(|base_command| base_command != next_command);
                if changed {
                    safe_base.shift_remove("environment");
                    safe_base.shift_remove("cwd");
                }
            }

            let mut merged_entry = safe_base.clone();
            for (key, value) in override_fields {
                merged_entry.insert(key.clone(), value.clone());
            }
            for field in ["environment", "headers", "oauth"] {
                let (Some(base_field), Some(next_field)) = (
                    safe_base.get(field).and_then(RawJson::as_object),
                    override_fields.get(field).and_then(RawJson::as_object),
                ) else {
                    continue;
                };
                let mut object_merged = base_field.clone();
                for (key, value) in next_field {
                    object_merged.insert(key.clone(), value.clone());
                }
                merged_entry.insert(field.to_string(), RawJson::Object(object_merged));
            }
            merged_mcp.insert(name.clone(), RawJson::Object(merged_entry));
        }
    }

    let mut out = base.clone();
    for (key, value) in next {
        out.insert(key.clone(), value.clone());
    }
    out.insert("mcp".to_string(), RawJson::Object(merged_mcp));
    out
}

/// `extractServers(config, kind)` — the per-family server key plus the two translators.
///
/// `codex` and `opencode` are **translated**; the other five pass their raw record through
/// unchanged, which is why an unknown key in a Cursor config survives into the merged entry (and
/// why [`to_server_entries`]' cut checks run over the translated result, not before it).
#[must_use]
pub fn extract_servers(
    document: &RawJson,
    kind: ImportKind,
    path: &Path,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> IndexMap<String, ServerEntry> {
    let Some(servers) = document.get_first(kind.server_keys()).and_then(RawJson::as_object) else {
        return IndexMap::new();
    };

    let mut translated: IndexMap<String, RawJson> = IndexMap::with_capacity(servers.len());
    for (name, entry) in servers {
        let Some(fields) = entry.as_object() else {
            continue;
        };
        match kind {
            ImportKind::Opencode => {
                if let Some(mapped) = translate_opencode_server(fields) {
                    translated.insert(name.clone(), mapped);
                }
            }
            ImportKind::Codex => {
                translated.insert(name.clone(), translate_codex_server(fields));
            }
            _ => {
                translated.insert(name.clone(), entry.clone());
            }
        }
    }

    to_server_entries(&RawJson::Object(translated), path, diagnostics)
}

/// `translateCodexServer(entry)` — Codex's three snake_case keys, remapped then **deleted**.
///
/// `bearer_token_env_var` → `bearerTokenEnv` with `auth ??= "bearer"`; `http_headers` merged **over**
/// `headers`; `env_http_headers` `{ header: ENVVAR }` → `` headers[header] ??= `$env:${ENVVAR}` ``
/// (note `??=`: an existing header wins). The remap order is load-bearing — `http_headers` lands
/// before `env_http_headers` reads `headers`.
#[must_use]
pub fn translate_codex_server(entry: &IndexMap<String, RawJson>) -> RawJson {
    let mut mapped = entry.clone();

    if let Some(bearer_env) = entry.get("bearer_token_env_var").and_then(RawJson::as_str) {
        mapped.insert("bearerTokenEnv".to_string(), RawJson::String(bearer_env.to_string()));
        if !mapped.contains_key("auth") {
            mapped.insert("auth".to_string(), RawJson::String("bearer".to_string()));
        }
    }
    if let Some(http_headers) = entry.get("http_headers").and_then(RawJson::as_object) {
        let mut headers =
            mapped.get("headers").and_then(RawJson::as_object).cloned().unwrap_or_default();
        for (key, value) in http_headers {
            headers.insert(key.clone(), value.clone());
        }
        mapped.insert("headers".to_string(), RawJson::Object(headers));
    }
    if let Some(env_headers) = entry.get("env_http_headers").and_then(RawJson::as_object) {
        let mut headers =
            mapped.get("headers").and_then(RawJson::as_object).cloned().unwrap_or_default();
        for (header, env_var) in env_headers {
            if let Some(name) = env_var.as_str()
                && !headers.contains_key(header)
            {
                headers.insert(header.clone(), RawJson::String(format!("$env:{name}")));
            }
        }
        mapped.insert("headers".to_string(), RawJson::Object(headers));
    }

    mapped.shift_remove("bearer_token_env_var");
    mapped.shift_remove("http_headers");
    mapped.shift_remove("env_http_headers");
    RawJson::Object(mapped)
}

/// The OpenCode entry translator (`extractServers`' `kind === "opencode"` branch).
///
/// `enabled === false` skips the entry entirely. `type: "local"` needs a **non-empty, all-string**
/// `command` array, which becomes `command` + `args`. `type: "remote"` needs a string `url`; an
/// `oauth === false` passes through, while an `oauth` **object** additionally sets `auth: "oauth"`
/// and is **projected** down to four fields — anything else OpenCode carries there is dropped
/// rather than handed to the OAuth flow.
#[must_use]
pub fn translate_opencode_server(entry: &IndexMap<String, RawJson>) -> Option<RawJson> {
    if entry.get("enabled") == Some(&RawJson::Bool(false)) {
        return None;
    }
    let kind = entry.get("type").and_then(RawJson::as_str);
    let mut mapped: IndexMap<String, RawJson> = IndexMap::new();

    if kind == Some("local") {
        let command = entry.get("command").and_then(RawJson::as_array)?;
        let parts: Vec<&str> = command.iter().filter_map(RawJson::as_str).collect();
        if parts.len() != command.len() {
            return None;
        }
        let (head, tail) = parts.split_first()?;
        mapped.insert("command".to_string(), RawJson::String((*head).to_string()));
        mapped.insert(
            "args".to_string(),
            RawJson::Array(tail.iter().map(|arg| RawJson::String((*arg).to_string())).collect()),
        );
        if let Some(env) = entry.get("environment").and_then(RawJson::to_string_record) {
            mapped.insert(
                "env".to_string(),
                RawJson::Object(
                    env.into_iter().map(|(k, v)| (k, RawJson::String(v))).collect(),
                ),
            );
        }
        if let Some(cwd) = entry.get("cwd").and_then(RawJson::as_str) {
            mapped.insert("cwd".to_string(), RawJson::String(cwd.to_string()));
        }
        return Some(RawJson::Object(mapped));
    }

    if kind == Some("remote") {
        let url = entry.get("url").and_then(RawJson::as_str)?;
        mapped.insert("url".to_string(), RawJson::String(url.to_string()));
        if let Some(headers) = entry.get("headers").and_then(RawJson::to_string_record) {
            mapped.insert(
                "headers".to_string(),
                RawJson::Object(headers.into_iter().map(|(k, v)| (k, RawJson::String(v))).collect()),
            );
        }
        match entry.get("oauth") {
            Some(RawJson::Bool(false)) => {
                mapped.insert("oauth".to_string(), RawJson::Bool(false));
            }
            Some(RawJson::Object(oauth)) => {
                mapped.insert("auth".to_string(), RawJson::String("oauth".to_string()));
                let mut projected: IndexMap<String, RawJson> = IndexMap::new();
                for key in ["clientId", "clientSecret", "scope"] {
                    if let Some(value) = oauth.get(key).and_then(RawJson::as_str) {
                        projected.insert(key.to_string(), RawJson::String(value.to_string()));
                    }
                }
                if let Some(RawJson::Bool(skip)) = oauth.get("skipIssuerMetadataValidation") {
                    projected
                        .insert("skipIssuerMetadataValidation".to_string(), RawJson::Bool(*skip));
                }
                mapped.insert("oauth".to_string(), RawJson::Object(projected));
            }
            _ => {}
        }
        return Some(RawJson::Object(mapped));
    }

    None
}

/// `expandImports(config, cwd)` — fold one source's own `imports` array **underneath** its own
/// `mcpServers` (MCP-055).
///
/// Imported servers are **first-wins across kinds** (`if (!importedServers[name])`), then the
/// source's own servers merge over the lot. So `imports: ["cursor", "claude-code"]` where both
/// define `foo` resolves to Cursor's `foo`, and a local `foo` beats both.
#[must_use]
pub fn expand_imports(
    config: &McpConfig,
    home: &Path,
    cwd: &Path,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> McpConfig {
    if config.imports.is_empty() {
        return config.clone();
    }
    let mut imported: IndexMap<String, ServerEntry> = IndexMap::new();
    for raw_kind in &config.imports {
        let Some(kind) = ImportKind::parse(raw_kind) else {
            continue;
        };
        let Some(document) = load_imported_config(
            kind,
            home,
            cwd,
            &format!("Failed to import MCP config from {kind}:"),
            diagnostics,
        ) else {
            continue;
        };
        for (name, entry) in extract_servers(&document.value, kind, &document.path, diagnostics) {
            imported.entry(name).or_insert(entry);
        }
    }
    McpConfig {
        mcp_servers: merge_server_maps(&imported, &config.mcp_servers),
        settings: config.settings.clone(),
        imports: config.imports.clone(),
    }
}

/// `loadDiscoveredHostConfigs(cwd)` — fold **all seven** families in [`ImportKind::ALL`] order,
/// later wins (MCP-058).
///
/// Reached only when the merged `settings.hostConfigDiscovery` is `"on"`, and it is the **base**
/// layer: `config.ts`'s own comment states the intent — *"an opt-in discovery cannot override a
/// shared or Pi-owned definition"*. Note the asymmetry with Agent-Plugin servers, which are also a
/// base but are folded *after* the ladder.
#[must_use]
pub fn load_discovered_host_configs(
    home: &Path,
    cwd: &Path,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> McpConfig {
    let mut config = McpConfig::default();
    for kind in ImportKind::ALL {
        let Some(document) = load_imported_config(
            kind,
            home,
            cwd,
            &format!("Failed to discover imported MCP config from {kind}:"),
            diagnostics,
        ) else {
            continue;
        };
        let servers = extract_servers(&document.value, kind, &document.path, diagnostics);
        config = merge_configs(&config, &McpConfig { mcp_servers: servers, ..McpConfig::default() });
    }
    config
}

// ===================================================================================================
// 10 · The six-source precedence ladder (MCP-052, MCP-096)
// ===================================================================================================

/// `ConfigSourceSpec["id"]` — the stable identifier the panel and the fingerprint key off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceId {
    /// `~/.config/mcp/mcp.json`.
    SharedGlobal,
    /// `~/.agents/mcp.json`.
    AgentsGlobal,
    /// `~/.agents/mcp/mcp.json`.
    AgentsNestedGlobal,
    /// `<agent_dir>/mcp.json`, or `--mcp-config`'s target. Upstream calls this `pi-global`; the id
    /// string is kept verbatim because it is a panel/fingerprint key, not a brand.
    PiGlobal,
    /// `<cwd>/.mcp.json`.
    SharedProject,
    /// `<cwd>/.cyrup/mcp.json` (upstream `.pi`).
    PiProject,
}

impl SourceId {
    /// The wire spelling used in the fingerprint's `sources` tuples.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SourceId::SharedGlobal => "shared-global",
            SourceId::AgentsGlobal => "agents-global",
            SourceId::AgentsNestedGlobal => "agents-nested-global",
            SourceId::PiGlobal => "pi-global",
            SourceId::SharedProject => "shared-project",
            SourceId::PiProject => "pi-project",
        }
    }
}

/// `ConfigSourceSpec["kind"]` — what `getServerProvenance` reports, and what decides where a write
/// lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// The adapter's own global file.
    User,
    /// A project-scoped file.
    Project,
    /// A shared file the adapter reads but never writes — its writes are redirected to `userPath`.
    Import,
}

/// `ConfigSourceSpec["scope"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceScope {
    /// Applies to every project.
    Global,
    /// Applies to this `cwd` only — the two sources MCP-096's trust gate covers.
    Project,
}

/// One rung of the ladder — upstream `ConfigSourceSpec`.
///
/// The `kind`/`import_kind`/`shared`/`scope`/`write_path` quintuple is not decoration: the panel
/// renders `label` and `shared`, `getServerProvenance` reports `kind`/`import_kind`, and `write_path`
/// is where a `/mcp` toggle's write actually lands — which is why a *shared* global server's writes
/// go to `<agent_dir>/mcp.json` and not to `~/.config/mcp/mcp.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigSourceSpec {
    /// Stable id.
    pub id: SourceId,
    /// Human label, verbatim from upstream (it is rendered).
    pub label: &'static str,
    /// Where this source is read from.
    pub read_path: PathBuf,
    /// Where a write for a server owned by this source lands.
    pub write_path: PathBuf,
    /// Provenance kind.
    pub kind: SourceKind,
    /// The provenance sub-label for an `Import` source — `"global MCP config"`,
    /// `".agents MCP config"`, `".agents/mcp MCP config"`.
    pub import_kind: Option<&'static str>,
    /// Whether the file is shared with other MCP-speaking tools (drives the panel's `shared`/`pi`
    /// split and `detectRepoPrompt`'s scan).
    pub shared: bool,
    /// Global or project.
    pub scope: SourceScope,
}

/// Everything the ladder needs that is not in the ladder: the resolved directories, the home
/// directory the three tool-agnostic paths hang off, the `--mcp-config` override, and the project
/// trust verdict.
///
/// Home is a **field**, not a call to [`home_dir`], so the whole ladder is testable against a
/// `TempDir` without mutating process env (edition 2024 makes `set_var` `unsafe`).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigContext {
    dirs: McpDirs,
    home: PathBuf,
    override_path: Option<PathBuf>,
    project_trusted: bool,
}

impl ConfigContext {
    /// Build from resolved directories and an optional `--mcp-config` path.
    ///
    /// `project_trusted` defaults to **`true`**, which is upstream's behaviour exactly (upstream has
    /// no trust gate at all); a caller holding `HostServices::is_project_trusted` opts into
    /// MCP-096's recommendation (b) with [`Self::with_project_trusted`].
    #[must_use]
    pub fn new(dirs: McpDirs, override_path: Option<&Path>) -> Self {
        Self {
            dirs,
            home: home_dir(),
            override_path: override_path.map(Path::to_path_buf),
            project_trusted: true,
        }
    }

    /// Override the home directory the three tool-agnostic global paths and all seven import
    /// families resolve against.
    #[must_use]
    pub fn with_home(mut self, home: PathBuf) -> Self {
        self.home = home;
        self
    }

    /// MCP-096: when `false`, `<cwd>/.mcp.json` and `<cwd>/.cyrup/mcp.json` contribute **zero
    /// servers** while still appearing in the discovery summary as present-but-untrusted.
    ///
    /// This is the port's one deliberate divergence from upstream on the config path. The security
    /// delta is one-sided and real — a project file can define a stdio server with an arbitrary
    /// `command`, an `env` value beginning with `!` (a shell command), and a `cwd`, and upstream
    /// connects `eager` servers at load — and `cyrup_config::settings`' `SettingsManager` already
    /// draws exactly this line for every other project-scoped config.
    ///
    /// **No production caller yet.** The caller is whoever holds `HostServices::is_project_trusted`
    /// — the `/mcp` dispatcher (`TODO(MCP-394)`) and the setup panel; until one of them lands the
    /// ladder runs at the upstream-identical default of `true`.
    #[must_use]
    pub fn with_project_trusted(mut self, trusted: bool) -> Self {
        self.project_trusted = trusted;
        self
    }

    /// The resolved directories this context was built from.
    #[must_use]
    pub fn dirs(&self) -> &McpDirs {
        &self.dirs
    }

    /// The home directory the global and import paths hang off.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Whether the project layer is trusted — see [`Self::with_project_trusted`].
    ///
    /// **No production reader yet.** The trust verdict is consumed inside this module by
    /// [`Self::source_contributes`]; the accessor exists for the `/mcp` panel, which renders a
    /// present-but-untrusted project source and whose dispatcher is `TODO(MCP-394)`.
    #[must_use]
    pub fn project_trusted(&self) -> bool {
        self.project_trusted
    }

    /// `getPiGlobalConfigPath(overridePath)` — config source **4**; [`McpDirs::user_config`] is the
    /// resolver, and it carries the `config.ts:168` citation and the source numbering.
    ///
    /// `<agent_dir>/mcp.json` is **not negotiable**: it is fixed by
    /// `cyrup_permission_system::manager::read_configured_mcp_server_names`, which reads the same
    /// file as its global MCP config path.
    #[must_use]
    pub fn user_path(&self) -> PathBuf {
        let raw = self.override_path.as_ref().map(|path| path.to_string_lossy());
        self.dirs.user_config(raw.as_deref())
    }

    /// `getGenericGlobalConfigPath()` — config source **1**, `~/.config/mcp/mcp.json`, resolved by
    /// [`crate::dirs::shared_global_config`] against this context's [`Self::home`].
    #[must_use]
    pub fn generic_global_path(&self) -> PathBuf {
        crate::dirs::shared_global_config(&self.home)
    }

    /// `getProjectConfigPath(cwd)` — config source **5**, `<cwd>/.mcp.json`, resolved by
    /// [`McpDirs::project_shared_config`].
    #[must_use]
    pub fn project_path(&self) -> PathBuf {
        self.dirs.project_shared_config()
    }

    /// `getProjectPiConfigPath(cwd)` — config source **6**, `<cwd>/.cyrup/mcp.json` (upstream
    /// `.pi`), resolved by [`McpDirs::project_agent_config`].
    #[must_use]
    pub fn project_override_path(&self) -> PathBuf {
        self.dirs.project_agent_config()
    }

    /// `getConfigSources(overridePath, cwd)` — 4 to 6 specs, in precedence order, **later wins**.
    ///
    /// The dedup guards are the reason the count varies: a source whose read path is already the
    /// `userPath` (or, for the two `.agents` paths, already the generic global path) is dropped
    /// rather than read twice. With `--mcp-config ~/.config/mcp/mcp.json`, rung 1 disappears.
    #[must_use]
    pub fn sources(&self) -> Vec<ConfigSourceSpec> {
        let user_path = self.user_path();
        let generic = self.generic_global_path();
        let project_path = self.project_path();
        let project_override = self.project_override_path();
        let mut sources = Vec::with_capacity(6);

        if generic != user_path {
            sources.push(ConfigSourceSpec {
                id: SourceId::SharedGlobal,
                label: "user-global standard MCP",
                read_path: generic.clone(),
                write_path: user_path.clone(),
                kind: SourceKind::Import,
                import_kind: Some("global MCP config"),
                shared: true,
                scope: SourceScope::Global,
            });
        }

        // Sources 2 and 3, in `AGENTS_GLOBAL_CONFIG_PATHS` order — the paths themselves come from
        // `dirs::agents_global_configs`, which owns the `config.ts:14` grammar.
        let [agents_global, agents_nested] = crate::dirs::agents_global_configs(&self.home);
        let agents_paths = [
            (SourceId::AgentsGlobal, "user-global .agents MCP", ".agents MCP config", agents_global),
            (
                SourceId::AgentsNestedGlobal,
                "user-global .agents nested MCP",
                ".agents/mcp MCP config",
                agents_nested,
            ),
        ];
        for (id, label, import_kind, agents_path) in agents_paths {
            if agents_path == user_path || agents_path == generic {
                continue;
            }
            sources.push(ConfigSourceSpec {
                id,
                label,
                read_path: agents_path,
                write_path: user_path.clone(),
                kind: SourceKind::Import,
                import_kind: Some(import_kind),
                shared: true,
                scope: SourceScope::Global,
            });
        }

        sources.push(ConfigSourceSpec {
            id: SourceId::PiGlobal,
            label: "Pi global override",
            read_path: user_path.clone(),
            write_path: user_path.clone(),
            kind: SourceKind::User,
            import_kind: None,
            shared: false,
            scope: SourceScope::Global,
        });

        if project_path != user_path {
            sources.push(ConfigSourceSpec {
                id: SourceId::SharedProject,
                label: "project standard MCP",
                read_path: project_path.clone(),
                write_path: project_path.clone(),
                kind: SourceKind::Project,
                import_kind: None,
                shared: true,
                scope: SourceScope::Project,
            });
        }

        if project_override != user_path && project_override != project_path {
            sources.push(ConfigSourceSpec {
                id: SourceId::PiProject,
                label: "project Pi override",
                read_path: project_override.clone(),
                write_path: project_override,
                kind: SourceKind::Project,
                import_kind: None,
                shared: false,
                scope: SourceScope::Project,
            });
        }

        sources
    }

    /// Whether this source contributes servers. Always `true` upstream; `false` for the two
    /// project-scoped rungs when the project is untrusted (MCP-096).
    #[must_use]
    pub fn source_contributes(&self, source: &ConfigSourceSpec) -> bool {
        self.project_trusted || source.scope != SourceScope::Project
    }

    /// `getMergedSettings(overridePath, cwd)` — walk the ladder and **one-level** merge each
    /// source's `settings`, ignoring its servers and its `imports` entirely.
    ///
    /// Separate from [`Self::load`] because `getConfiguredHostConfigDiscovery` has to know the
    /// answer *before* the load can decide whether to build a host-config base layer.
    #[must_use]
    pub fn merged_settings(&self, diagnostics: &mut Vec<ConfigDiagnostic>) -> Option<McpSettings> {
        let mut settings: Option<McpSettings> = None;
        for source in self.sources() {
            if !self.source_contributes(&source) {
                continue;
            }
            if let Some(loaded) = read_validated_config(&source.read_path, diagnostics)
                && loaded.settings.is_some()
            {
                settings = merge_settings(settings.as_ref(), loaded.settings.as_ref());
            }
        }
        settings
    }

    /// `getConfiguredHostConfigDiscovery` — the merged `settings.hostConfigDiscovery`, defaulting
    /// `"off"` through an explicit three-way test.
    #[must_use]
    pub fn host_config_discovery(&self, diagnostics: &mut Vec<ConfigDiagnostic>) -> HostConfigDiscovery {
        self.merged_settings(diagnostics)
            .as_ref()
            .map_or(HostConfigDiscovery::Off, McpSettings::host_config_discovery)
    }

    /// `loadMcpConfig(overridePath, cwd)` — the whole ladder, and **this function cannot fail**.
    ///
    /// Order, and every step's rationale:
    ///
    /// 1. Host-discovered configs, when `hostConfigDiscovery === "on"`, as the **base** — so an
    ///    opt-in discovery can never override a shared or adapter-owned definition.
    /// 2. Each ladder rung, left to right, later wins. Each rung is first passed through
    ///    [`expand_imports`], so that rung's own `imports` sit *underneath* its own `mcpServers`.
    /// 3. Agent-Plugin servers folded in as a **second base** (`mergeConfigs(pluginConfig, config)`),
    ///    so every file source outranks them. Note the asymmetry with step 1: host configs are the
    ///    base *before* the ladder, plugins *after* — both end up lowest-precedence, by different
    ///    routes.
    ///
    /// Every read is defensive: a missing file is skipped, a malformed file warns and is skipped, a
    /// malformed entry is dropped while the file survives. That is what lets
    /// [`crate::extension::McpExtension`]'s `init` be infallible (MCP-003).
    #[must_use]
    pub fn load(&self) -> LoadedConfig {
        let mut diagnostics = Vec::new();
        let discovery = self.host_config_discovery(&mut diagnostics);
        let mut config = if discovery == HostConfigDiscovery::On {
            load_discovered_host_configs(&self.home, self.dirs.cwd(), &mut diagnostics)
        } else {
            McpConfig::default()
        };

        for source in self.sources() {
            if !self.source_contributes(&source) {
                continue;
            }
            let Some(loaded) = read_validated_config(&source.read_path, &mut diagnostics) else {
                continue;
            };
            let expanded =
                expand_imports(&loaded, &self.home, self.dirs.cwd(), &mut diagnostics);
            config = merge_configs(&config, &expanded);
        }

        let plugin_config = self.load_plugin_config(&config, &mut diagnostics);
        config = merge_configs(&plugin_config, &config);

        LoadedConfig { config, diagnostics }
    }

    /// `loadAgentPluginConfigs(config.settings?.agentPluginPaths, cwd)`.
    ///
    /// MCP-047 has landed: this really does load, through
    /// [`crate::agent_plugin::load_agent_plugins_in`] (see the comment on the call below for why it
    /// is the three-argument form and not [`crate::agent_plugin::load_agent_plugins`]). It
    /// contributes an empty base only when `agentPluginPaths` is absent, which is the overwhelmingly
    /// common case.
    fn load_plugin_config(
        &self,
        config: &McpConfig,
        diagnostics: &mut Vec<ConfigDiagnostic>,
    ) -> McpConfig {
        let paths = config.settings_or_default().agent_plugin_paths();
        if paths.is_empty() {
            return McpConfig::default();
        }
        // `load_agent_plugins_in`, NOT `load_agent_plugins`: a plugin path is resolved against the
        // **session cwd** while `${PLUGIN_DATA}` hangs off `<agent_dir>`, and the two-argument form
        // can only supply the latter — it fills cwd from `std::env::current_dir()`, which is the
        // process's, not this context's. Those differ for an SDK embedder and for any caller that
        // built `McpDirs` with an explicit cwd, and the symptom would be a relative
        // `agentPluginPaths` entry silently resolving somewhere else (MCP-047 care point 4).
        let (loaded, skipped) = crate::agent_plugin::load_agent_plugins_in(paths, &self.dirs);
        for (message, reason) in skipped {
            // `message` is already the VERBATIM upstream `console.warn` line, and
            // `agent_plugin::warn_skip` has already emitted it through `tracing::warn!` — every
            // rejection in that file goes through one function precisely so this is true. Re-wrapping
            // it as `Skipping Agent Plugin server "{message}": {reason:?}` is what this used to do,
            // which put a whole English sentence in the `server` field and printed a second,
            // upstream-less warning for every skip. The reason enum stays machine-readable and is
            // what the `/mcp setup` panel matches on; only the names it carries are server keys.
            let server = match &reason {
                crate::agent_plugin::SkipReason::DuplicateServer(name)
                | crate::agent_plugin::SkipReason::DuplicateHeader(name) => Some(name.clone()),
                _ => None,
            };
            diagnostics.push(ConfigDiagnostic {
                path: self.dirs.agent_dir().to_path_buf(),
                server,
                message,
            });
        }
        McpConfig {
            mcp_servers: loaded.into_iter().map(|server| (server.name, server.entry)).collect(),
            ..McpConfig::default()
        }
    }
}

/// `loadMcpConfig(configPath, cwd)` — the crate-facing entry point.
///
/// Delegates to [`ConfigContext::load`] and discards the diagnostics, which have already been
/// emitted through `tracing::warn!`. Callers that render them (the `/mcp setup` panel) build a
/// [`ConfigContext`] and keep the [`LoadedConfig`].
///
/// **This function cannot fail** — see [`ConfigContext::load`] and MCP-003.
#[must_use]
pub fn load_mcp_config(dirs: &McpDirs, explicit_path: Option<&Path>) -> McpConfig {
    ConfigContext::new(dirs.clone(), explicit_path).load().config
}

// ===================================================================================================
// 11 · Raw-config I/O — the writer every write funnels through (MCP-061)
// ===================================================================================================

/// `Record<string, unknown>` — a parsed config **document**, in file order.
///
/// The writers deal in this and never in [`ServerEntry`], which is the whole of §7 rule 3: an
/// unknown top-level key (`"$schema"`, a future `"experimental"` block, a comment-adjacent key some
/// other MCP tool wrote) survives a write because nothing on the write path ever round-trips
/// through the typed model.
pub type RawObject = IndexMap<String, RawJson>;

/// `mcpServers` — the key `setServersObject` always writes.
pub const SERVERS_KEY: &str = "mcpServers";

/// `mcp-servers` — the legacy spelling `getServersObject` still *reads* and `setServersObject`
/// **deletes**. A hyphenated file is silently normalised by any write.
pub const LEGACY_SERVERS_KEY: &str = "mcp-servers";

/// `serializeRawConfig(raw)` for a document that is already known to be an object — the only
/// serialiser, because every writer in this module holds a [`RawObject`].
///
/// `` `${JSON.stringify(raw, null, 2)}\n` `` — 2-space indent and a **trailing newline**, both part
/// of the on-disk contract because [`build_config_write_preview`] diffs against exactly this text
/// (MCP-099) and a missing newline would show up as a one-line change on every write.
#[must_use]
pub fn serialize_raw_object(raw: &RawObject) -> String {
    serde_json::to_string_pretty(raw).map_or_else(|_| "{}\n".to_string(), |text| format!("{text}\n"))
}

/// A compact `JSON.stringify(value)` — no indent, no newline — used only for the structural
/// equality tests upstream writes as `JSON.stringify(a) === JSON.stringify(b)`.
///
/// It has to be a *string* comparison and not `IndexMap`'s [`PartialEq`], because `IndexMap` compares
/// as a map (order-insensitive) while `JSON.stringify` is order-**sensitive**, and key order is
/// exactly what these writers preserve. Serialisation of a [`RawJson`] cannot fail — the keys are
/// strings and [`RawJsonVisitor::visit_f64`] already rejected the non-finite numbers — so the
/// fallback is unreachable rather than lossy.
fn compact_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// `readRawConfigObject(filePath)` — missing ⇒ `{}`, unparseable ⇒ `{}`, non-object root ⇒ `{}`.
///
/// **Silently.** This is a *writer* helper, and upstream's bare `catch {}` here is deliberate:
/// clobbering an unparseable file is the accepted cost of being able to write one at all. The user
/// still sees what is about to happen, because [`build_config_write_preview`] renders the diff from
/// `{}` — it announces the clobber rather than hiding it (MCP-099).
///
/// Note this is *not* [`read_validated_config`]: no typing, no diagnostics, no `mcp-servers`
/// normalisation. The two exist side by side on purpose.
#[must_use]
pub fn read_raw_config_object(path: &Path) -> RawObject {
    if !path.exists() {
        return RawObject::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return RawObject::new();
    };
    match parse_json_config(&text, &path.to_string_lossy()) {
        Ok(RawJson::Object(entries)) => entries,
        _ => RawObject::new(),
    }
}

/// `writeRawConfigObject(filePath, raw)` — `mkdirSync(recursive)`, write `<path>.<pid>.tmp`, then
/// `renameSync` (MCP-061).
///
/// The literal mechanism, including what it does **not** do: there is no file lock. Concurrency
/// safety comes only from the rename being atomic within a directory, and that is the contract the
/// rest of the tree's MCP writers were written against. `cyrup-config`'s `FileSettingsStore` takes a
/// cross-process advisory `FileLock`; adopting it here would serialise this writer against every
/// other holder of that convention and is explicitly out of scope without sign-off.
///
/// One addition upstream does not make: when the rename fails the temp file is removed. Upstream
/// leaves it, which litters `<agent_dir>` with `mcp.json.4821.tmp` after a full disk; nothing
/// observes the difference because the rename failure is reported either way.
pub fn write_raw_config_object(path: &Path, raw: &RawObject) -> McpResult<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|source| McpError::Io { path: parent.to_path_buf(), source })?;
    }

    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);

    std::fs::write(&tmp, serialize_raw_object(raw))
        .map_err(|source| McpError::Io { path: tmp.clone(), source })?;
    if let Err(source) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(McpError::Io { path: path.to_path_buf(), source });
    }
    Ok(())
}

/// `getServersObject(raw)` — `raw.mcpServers ?? raw["mcp-servers"] ?? {}`, and `{}` again for any
/// non-object value.
///
/// Returns an owned map where upstream returns the live reference. Every caller's next move is
/// `setServersObject(raw, servers)`, so the end state is identical; the clone is what makes the
/// mutation explicit instead of spooky.
#[must_use]
pub fn get_servers_object(raw: &RawObject) -> RawObject {
    match raw.get(SERVERS_KEY).or_else(|| raw.get(LEGACY_SERVERS_KEY)) {
        Some(RawJson::Object(servers)) => servers.clone(),
        _ => RawObject::new(),
    }
}

/// `setServersObject(raw, servers)` — `delete raw["mcp-servers"]; raw.mcpServers = servers;`.
///
/// So **any** write normalises a hyphenated file. The removal is a `shift_remove`, not a
/// `swap_remove`: JS `delete` preserves the order of the surviving keys and a `swap_remove` would
/// silently reshuffle the document. Likewise `insert` keeps `mcpServers` in its original position
/// when the key already existed and appends it otherwise — which is exactly what `raw.mcpServers =`
/// does to a JS object.
pub fn set_servers_object(raw: &mut RawObject, servers: RawObject) {
    raw.shift_remove(LEGACY_SERVERS_KEY);
    raw.insert(SERVERS_KEY.to_string(), RawJson::Object(servers));
}

// ===================================================================================================
// 12 · The unified diff and the write preview (MCP-062, MCP-099)
// ===================================================================================================

/// `ConfigWritePreview` — what `/mcp setup` shows before it touches a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWritePreview {
    /// The file the write would land in.
    pub path: PathBuf,
    /// Whether the file exists **now**.
    pub existed: bool,
    /// `beforeText !== afterText` — computed against the *reserialised* before text, not the file's
    /// bytes (MCP-099).
    pub changed: bool,
    /// `existed ? serializeRawConfig(readRawConfigObject(path)) : ""`.
    pub before_text: String,
    /// `serializeRawConfig(nextRaw)`.
    pub after_text: String,
    /// [`build_unified_diff`]'s output, rendered verbatim by the panel.
    pub diff_text: String,
}

/// `buildUnifiedDiff(beforeText, afterText)` — a hand-rolled LCS, ported as the DP it is (MCP-062).
///
/// # Why not `similar`
///
/// `similar` *is* a workspace dependency (`cyrup-tools`, `cyrup-test-support` both use it) and would
/// produce a correct diff. It would not produce **this** diff. The tie-break below —
/// `lcs[i][j+1] >= lcs[i+1][j]` prefers the addition — decides which side a changed line is
/// attributed to, the panel renders the resulting text verbatim, and a differently-shaped hunk is
/// therefore a user-visible divergence. Substituting a diff library here is a mechanism
/// substitution, not a dependency saving.
///
/// The table is filled bottom-up over `(rows+1) × (cols+1)` and the walk is forward, so the emitted
/// order matches upstream line for line. Equal texts short-circuit to the literal `"(no changes)"`,
/// which the panel special-cases.
#[must_use]
pub fn build_unified_diff(before_text: &str, after_text: &str) -> String {
    if before_text == after_text {
        return "(no changes)".to_string();
    }

    // `String.prototype.split("\n")` keeps the trailing empty field a text ending in `\n` produces,
    // and Rust's `split('\n')` does the same — which matters because every config text ends in one.
    let before: Vec<&str> = before_text.split('\n').collect();
    let after: Vec<&str> = after_text.split('\n').collect();
    let rows = before.len();
    let cols = after.len();

    let mut lcs = vec![vec![0usize; cols.saturating_add(1)]; rows.saturating_add(1)];
    for i in (0..rows).rev() {
        for j in (0..cols).rev() {
            let value = if before.get(i) == after.get(j) {
                lcs.get(i + 1).and_then(|row| row.get(j + 1)).copied().unwrap_or(0).saturating_add(1)
            } else {
                let down = lcs.get(i + 1).and_then(|row| row.get(j)).copied().unwrap_or(0);
                let right = lcs.get(i).and_then(|row| row.get(j + 1)).copied().unwrap_or(0);
                down.max(right)
            };
            if let Some(cell) = lcs.get_mut(i).and_then(|row| row.get_mut(j)) {
                *cell = value;
            }
        }
    }

    let mut lines: Vec<String> = vec!["--- before".to_string(), "+++ after".to_string()];
    let mut i = 0usize;
    let mut j = 0usize;
    while i < rows || j < cols {
        if i < rows && j < cols && before.get(i) == after.get(j) {
            if let Some(line) = before.get(i) {
                lines.push(format!("  {line}"));
            }
            i += 1;
            j += 1;
            continue;
        }
        // The tie-break: on an equal-length LCS, prefer the addition. This is the whole reason the
        // DP is ported rather than delegated.
        let prefer_addition = j < cols
            && (i == rows || {
                let right = lcs.get(i).and_then(|row| row.get(j + 1)).copied().unwrap_or(0);
                let down = lcs.get(i + 1).and_then(|row| row.get(j)).copied().unwrap_or(0);
                right >= down
            });
        if prefer_addition {
            if let Some(line) = after.get(j) {
                lines.push(format!("+ {line}"));
            }
            j += 1;
            continue;
        }
        if i < rows {
            if let Some(line) = before.get(i) {
                lines.push(format!("- {line}"));
            }
            i += 1;
            continue;
        }
        // Unreachable: `i == rows` implies `j < cols` from the loop condition, which makes
        // `prefer_addition` true. Present so a future edit to the guards cannot hang the panel.
        break;
    }

    lines.join("\n")
}

/// `buildConfigWritePreview(filePath, nextRaw)` — and its one surprising property (MCP-099).
///
/// **`before_text` is not the file's bytes.** It is `serializeRawConfig(readRawConfigObject(path))`
/// — the *reserialised parse*. So comments, 4-space indent and a hyphenated `mcp-servers` key are
/// all stripped from the "before" side, an unparseable file previews as a diff from `{}`, and
/// `changed` is computed against that normalised text. A commented `mcp.json` therefore previews as
/// a whole-file rewrite even when the semantic content is unchanged.
///
/// That is not a bug to fix in the port: the writer really does normalise the file, and a
/// byte-accurate "before" would under-report what the write is about to do.
#[must_use]
pub fn build_config_write_preview(path: &Path, next_raw: &RawObject) -> ConfigWritePreview {
    let existed = path.exists();
    let before_raw = read_raw_config_object(path);
    let before_text = if existed { serialize_raw_object(&before_raw) } else { String::new() };
    let after_text = serialize_raw_object(next_raw);
    ConfigWritePreview {
        path: path.to_path_buf(),
        existed,
        changed: before_text != after_text,
        diff_text: build_unified_diff(&before_text, &after_text),
        before_text,
        after_text,
    }
}

// ===================================================================================================
// 13 · The typed writers (MCP-063, MCP-064, MCP-065)
// ===================================================================================================

/// `ServerDisabledOverrideResult` — where the toggle landed, and whether anything was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDisabledOverrideResult {
    /// Always `<cwd>/.cyrup/mcp.json` (upstream `.pi`), whether or not it changed.
    pub path: PathBuf,
    /// `false` for a no-op — the file's mtime is not touched.
    pub changed: bool,
}

/// `ServerProvenance` (`types.ts`) — which file a server *came from*, and which file a write for it
/// goes **to**.
///
/// The two are not the same file, and that is the point: `path` is the source's `write_path`, so a
/// server first defined in the shared `~/.config/mcp/mcp.json` has its toggles written to
/// `<agent_dir>/mcp.json` instead of being edited in a file other MCP tools own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerProvenance {
    /// The **write** target, i.e. [`ConfigSourceSpec::write_path`].
    pub path: PathBuf,
    /// `"user" | "project" | "import"`.
    pub kind: SourceKind,
    /// For a host-discovered server the [`ImportKind`] spelling (`"cursor"`); for a shared ladder
    /// source the human sub-label (`"global MCP config"`). Upstream overloads one optional string
    /// for both and so does this.
    pub import_kind: Option<String>,
}

/// `ensureCompatibilityImports`' return — the target file and what it actually added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityImportsResult {
    /// The adapter-owned global file (`userPath`).
    pub path: PathBuf,
    /// Empty when every requested kind was already listed — and then **nothing was written**.
    pub added: Vec<ImportKind>,
}

/// `buildStarterProjectConfig()` — literally `{ "mcpServers": {} }`.
///
/// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
#[must_use]
pub fn build_starter_project_config() -> McpConfig {
    McpConfig::default()
}

/// The raw document `writeStarterProjectConfig` writes: one key, an empty object.
fn starter_raw() -> RawObject {
    let mut raw = RawObject::new();
    raw.insert(SERVERS_KEY.to_string(), RawJson::Object(RawObject::new()));
    raw
}

/// `previewSharedServerEntry(filePath, serverName, entry)` — the write preview for adding one
/// server to an arbitrary shared file (a preset, or the RepoPrompt proposal).
///
/// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
#[must_use]
pub fn preview_shared_server_entry(
    path: &Path,
    server_name: &str,
    entry: &ServerEntry,
) -> ConfigWritePreview {
    let mut next_raw = read_raw_config_object(path);
    let mut servers = get_servers_object(&next_raw);
    servers.insert(server_name.to_string(), raw_from(entry));
    set_servers_object(&mut next_raw, servers);
    build_config_write_preview(path, &next_raw)
}

/// `writeSharedServerEntry(filePath, serverName, entry)` — MCP-065.
///
/// The entry is serialised from the typed [`ServerEntry`], so only the fields it actually sets are
/// written (`skip_serializing_if = "Option::is_none"` throughout); every *other* key in the file,
/// known or not, survives because the surrounding document is the raw one.
///
/// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
pub fn write_shared_server_entry(
    path: &Path,
    server_name: &str,
    entry: &ServerEntry,
) -> McpResult<PathBuf> {
    let mut raw = read_raw_config_object(path);
    let mut servers = get_servers_object(&raw);
    servers.insert(server_name.to_string(), raw_from(entry));
    set_servers_object(&mut raw, servers);
    write_raw_config_object(path, &raw)?;
    Ok(path.to_path_buf())
}

/// `writeDirectToolsConfig(changes, provenance, fullConfig)` — MCP-064.
///
/// Groups the changes by their provenance's **write** path and rewrites one file per group. The
/// `import` arm is the interesting one: an imported server has no adapter-owned definition to patch,
/// so the *fully merged* definition is materialised into the adapter's own file with `directTools`
/// attached — toggling a Cursor-defined server writes a copy into `<agent_dir>/mcp.json` rather than
/// editing Cursor's config, which is both the safe thing and what the panel promised.
///
/// The non-import arm patches the **raw** entry in place, so unknown keys on that entry survive
/// (§7 rule 3). An entry that is present but is not an object is skipped: upstream's
/// `{ ...servers[name] }` would spread a string into `{0:"x"}`, which is not a behaviour worth
/// reproducing and which no config can reach through [`to_server_entries`] anyway.
///
/// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
pub fn write_direct_tools_config(
    changes: &IndexMap<String, BoolOrList>,
    provenance: &IndexMap<String, ServerProvenance>,
    full_config: &McpConfig,
) -> McpResult<()> {
    let mut by_path: IndexMap<PathBuf, Vec<(String, BoolOrList, ServerProvenance)>> = IndexMap::new();
    for (server_name, value) in changes {
        let Some(prov) = provenance.get(server_name) else {
            continue;
        };
        by_path
            .entry(prov.path.clone())
            .or_default()
            .push((server_name.clone(), value.clone(), prov.clone()));
    }

    for (path, entries) in by_path {
        let mut raw = read_raw_config_object(&path);
        let mut servers = get_servers_object(&raw);

        for (name, value, prov) in entries {
            if prov.kind == SourceKind::Import {
                if let Some(full) = full_config.mcp_servers.get(&name) {
                    let mut materialised = full.clone();
                    materialised.direct_tools = Some(value);
                    servers.insert(name, raw_from(&materialised));
                }
            } else if let Some(RawJson::Object(existing)) = servers.get(&name) {
                let mut patched = existing.clone();
                patched.insert("directTools".to_string(), raw_from(&value));
                servers.insert(name, RawJson::Object(patched));
            }
        }

        set_servers_object(&mut raw, servers);
        write_raw_config_object(&path, &raw)?;
    }
    Ok(())
}

impl ConfigContext {
    /// `writeProjectServerDisabledOverride(overridePath, cwd, serverName, disabled)` — MCP-063.
    ///
    /// Writes **only** the `disabled` field into `<cwd>/.cyrup/mcp.json`. It never copies a server
    /// definition, and therefore never copies a global `bearerToken` or `headers` into a
    /// repo-visible file — that property is the entire reason this writer exists instead of a
    /// generic "write the merged entry" one.
    ///
    /// Disabling is `{ ...existing, disabled: true }`. **Enabling** is the subtle half: it deletes
    /// the key, then re-merges every *other* ladder source plus this file's own `imports`, and
    /// writes an explicit `disabled: false` only when that lower-precedence merge is itself
    /// disabled. Otherwise the key simply disappears, and an entry left empty is removed entirely so
    /// the file does not accumulate `{"foo": {}}` husks.
    ///
    /// Unlike every other function in this module this one **can** fail, with four exact messages
    /// (§14). It is a user-initiated write on a file the user can hand-edit, so a structural problem
    /// is reported rather than silently normalised — the opposite of the load path's contract, and
    /// deliberately so.
    ///
    /// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
    pub fn write_project_server_disabled_override(
        &self,
        server_name: &str,
        disabled: bool,
    ) -> McpResult<ServerDisabledOverrideResult> {
        let file_path = self.project_override_path();
        let shown = file_path.display().to_string();
        let mut raw = RawObject::new();

        if file_path.exists() {
            let text = std::fs::read_to_string(&file_path).map_err(|error| {
                McpError::Config(format!(
                    "Failed to read project MCP override at {shown}: {error}"
                ))
            })?;
            let parsed = parse_json_config(&text, &shown).map_err(|error| {
                McpError::Config(format!(
                    "Failed to read project MCP override at {shown}: {error}"
                ))
            })?;
            let Some(entries) = parsed.as_object() else {
                return Err(McpError::Config(format!(
                    "Failed to read project MCP override at {shown}: root value must be an object"
                )));
            };
            raw = entries.clone();
        }

        // `raw.mcpServers !== undefined ? "mcpServers" : raw["mcp-servers"] !== undefined ? … `.
        // Presence, not truthiness: an explicit `"mcpServers": null` selects that key and then fails
        // the object test below, which is the reported error rather than a silent overwrite.
        let server_key = if raw.contains_key(SERVERS_KEY) {
            SERVERS_KEY
        } else if raw.contains_key(LEGACY_SERVERS_KEY) {
            LEGACY_SERVERS_KEY
        } else {
            SERVERS_KEY
        };

        let mut servers = match raw.get(server_key) {
            None => RawObject::new(),
            Some(RawJson::Object(existing)) => existing.clone(),
            Some(_) => {
                return Err(McpError::Config(format!(
                    "Failed to update project MCP override at {shown}: {server_key} must be an object"
                )));
            }
        };

        let existing: Option<RawObject> = match servers.get(server_name) {
            None => None,
            Some(RawJson::Object(entry)) => Some(entry.clone()),
            Some(_) => {
                return Err(McpError::Config(format!(
                    "Failed to update project MCP override at {shown}: server \"{server_name}\" must be an object"
                )));
            }
        };

        let mut next = existing.clone().unwrap_or_default();
        if disabled {
            next.insert("disabled".to_string(), RawJson::Bool(true));
        } else {
            next.shift_remove("disabled");
            if self.lower_precedence_disabled(&file_path, &raw, server_name)? {
                next.insert("disabled".to_string(), RawJson::Bool(false));
            }
        }

        let unchanged = match &existing {
            None => next.is_empty(),
            Some(current) => compact_json(current) == compact_json(&next),
        };
        if unchanged {
            return Ok(ServerDisabledOverrideResult { path: file_path, changed: false });
        }

        if next.is_empty() {
            servers.shift_remove(server_name);
        } else {
            servers.insert(server_name.to_string(), RawJson::Object(next));
        }

        // Note: `raw[serverKey] = servers`, **not** `setServersObject` — this writer keeps whichever
        // spelling the file already used instead of normalising it.
        raw.insert(server_key.to_string(), RawJson::Object(servers));
        write_raw_config_object(&file_path, &raw)?;
        Ok(ServerDisabledOverrideResult { path: file_path, changed: true })
    }

    /// The enabling half of [`Self::write_project_server_disabled_override`]: is the server still
    /// disabled once **this** file is taken out of the ladder?
    ///
    /// Every other source is re-merged from scratch (this file skipped by read path), and this
    /// file's own `imports` are folded in afterwards — an unsupported `imports` entry is the fourth
    /// error string, because silently ignoring it would compute the answer from a different config
    /// than the one the file describes. The MCP-096 trust gate applies, so the answer matches what
    /// [`Self::load`] would actually produce rather than what the files say in the abstract.
    fn lower_precedence_disabled(
        &self,
        file_path: &Path,
        raw: &RawObject,
        server_name: &str,
    ) -> McpResult<bool> {
        let mut diagnostics = Vec::new();
        let mut lower = McpConfig::default();
        for source in self.sources() {
            if source.read_path == file_path || !self.source_contributes(&source) {
                continue;
            }
            if let Some(loaded) = read_validated_config(&source.read_path, &mut diagnostics) {
                let expanded =
                    expand_imports(&loaded, &self.home, self.dirs.cwd(), &mut diagnostics);
                lower = merge_configs(&lower, &expanded);
            }
        }

        if let Some(imports) = raw.get("imports") {
            let shown = file_path.display().to_string();
            let unsupported = McpError::Config(format!(
                "Failed to update project MCP override at {shown}: imports contains an unsupported config kind"
            ));
            let Some(items) = imports.as_array() else {
                return Err(unsupported);
            };
            let mut kinds = Vec::with_capacity(items.len());
            for item in items {
                let Some(text) = item.as_str() else {
                    return Err(unsupported);
                };
                if ImportKind::parse(text).is_none() {
                    return Err(unsupported);
                }
                kinds.push(text.to_string());
            }
            let own_imports = McpConfig { imports: kinds, ..McpConfig::default() };
            let expanded =
                expand_imports(&own_imports, &self.home, self.dirs.cwd(), &mut diagnostics);
            lower = merge_configs(&lower, &expanded);
        }

        Ok(lower.mcp_servers.get(server_name).is_some_and(ServerEntry::is_disabled))
    }

    /// `getServerProvenance(overridePath, cwd)` — MCP-064.
    ///
    /// Three passes, and the precedence rule differs between them:
    ///
    /// 1. Host-discovered families, only when `hostConfigDiscovery === "on"`, in [`ImportKind::ALL`]
    ///    order with **later kinds winning** — the same deterministic order
    ///    [`load_discovered_host_configs`] folds in.
    /// 2. Per ladder source, that source's own `imports`, **first-wins** (`if (!provenance.has)`).
    /// 3. That source's own `mcpServers`, later sources overwriting earlier ones.
    ///
    /// Every import-derived name maps to `userPath` with `kind: "import"`, which is what keeps
    /// writes inside adapter-owned storage even though the definition came from somewhere else.
    ///
    /// The MCP-096 trust gate applies here as it does in [`Self::load`]: an untrusted project's
    /// sources contribute no servers, so they contribute no provenance either. Anything else would
    /// hand out a write target for a server that is not loaded.
    ///
    /// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
    #[must_use]
    pub fn server_provenance(
        &self,
        diagnostics: &mut Vec<ConfigDiagnostic>,
    ) -> IndexMap<String, ServerProvenance> {
        let mut provenance: IndexMap<String, ServerProvenance> = IndexMap::new();
        let user_path = self.user_path();

        if self.host_config_discovery(diagnostics) == HostConfigDiscovery::On {
            for kind in ImportKind::ALL {
                let Some(document) = load_imported_config(
                    kind,
                    &self.home,
                    self.dirs.cwd(),
                    &format!("Failed to inspect imported MCP config from {kind}:"),
                    diagnostics,
                ) else {
                    continue;
                };
                for name in
                    extract_servers(&document.value, kind, &document.path, diagnostics).keys()
                {
                    provenance.insert(
                        name.clone(),
                        ServerProvenance {
                            path: user_path.clone(),
                            kind: SourceKind::Import,
                            import_kind: Some(kind.as_str().to_string()),
                        },
                    );
                }
            }
        }

        for source in self.sources() {
            if !self.source_contributes(&source) {
                continue;
            }
            let Some(loaded) = read_validated_config(&source.read_path, diagnostics) else {
                continue;
            };

            for raw_kind in &loaded.imports {
                let Some(kind) = ImportKind::parse(raw_kind) else {
                    continue;
                };
                let Some(document) = load_imported_config(
                    kind,
                    &self.home,
                    self.dirs.cwd(),
                    &format!("Failed to inspect imported MCP config from {kind}:"),
                    diagnostics,
                ) else {
                    continue;
                };
                for name in
                    extract_servers(&document.value, kind, &document.path, diagnostics).keys()
                {
                    if !provenance.contains_key(name) {
                        provenance.insert(
                            name.clone(),
                            ServerProvenance {
                                path: user_path.clone(),
                                kind: SourceKind::Import,
                                import_kind: Some(kind.as_str().to_string()),
                            },
                        );
                    }
                }
            }

            for name in loaded.mcp_servers.keys() {
                provenance.insert(
                    name.clone(),
                    ServerProvenance {
                        path: source.write_path.clone(),
                        kind: source.kind,
                        import_kind: source.import_kind.map(str::to_string),
                    },
                );
            }
        }

        provenance
    }

    /// `previewCompatibilityImports(importKinds, overridePath)` — MCP-065.
    ///
    /// Note what the preview does that a naive reading would not: `setServersObject` runs
    /// unconditionally, so a file with no server table at all gains `"mcpServers": {}` in the
    /// preview *and* in the write. That is upstream, and it is why the preview of a first-time
    /// import shows two changes rather than one.
    ///
    /// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
    #[must_use]
    pub fn preview_compatibility_imports(&self, import_kinds: &[ImportKind]) -> ConfigWritePreview {
        let target = self.user_path();
        let mut next_raw = read_raw_config_object(&target);
        let merged = merged_import_list(&next_raw, import_kinds);
        next_raw.insert(
            "imports".to_string(),
            RawJson::Array(merged.into_iter().map(RawJson::String).collect()),
        );
        let servers = get_servers_object(&next_raw);
        set_servers_object(&mut next_raw, servers);
        build_config_write_preview(&target, &next_raw)
    }

    /// `ensureCompatibilityImports(importKinds, overridePath)` — idempotent by construction.
    ///
    /// When nothing is added the function returns `added: []` and **does not write**, so a second
    /// call does not touch the file's mtime. That matters because the onboarding path calls it on
    /// every start.
    ///
    /// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
    pub fn ensure_compatibility_imports(
        &self,
        import_kinds: &[ImportKind],
    ) -> McpResult<CompatibilityImportsResult> {
        let target = self.user_path();
        let mut raw = read_raw_config_object(&target);
        let current = current_import_list(&raw);
        let merged = merged_import_list(&raw, import_kinds);
        // `merged.filter(kind => !currentImports.includes(kind))` — computed off the **deduped**
        // merged list, so a caller passing the same kind twice gets one entry back, not two.
        let mut added: Vec<ImportKind> = Vec::new();
        for kind in import_kinds {
            if current.iter().any(|seen| seen == kind.as_str()) || added.contains(kind) {
                continue;
            }
            added.push(*kind);
        }
        if added.is_empty() {
            return Ok(CompatibilityImportsResult { path: target, added: Vec::new() });
        }

        raw.insert(
            "imports".to_string(),
            RawJson::Array(merged.into_iter().map(RawJson::String).collect()),
        );
        let servers = get_servers_object(&raw);
        set_servers_object(&mut raw, servers);
        write_raw_config_object(&target, &raw)?;
        Ok(CompatibilityImportsResult { path: target, added })
    }

    /// `previewStarterProjectConfig(cwd)` — the `{ "mcpServers": {} }` scaffold's preview.
    ///
    /// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
    #[must_use]
    pub fn preview_starter_project_config(&self) -> ConfigWritePreview {
        build_config_write_preview(&self.project_path(), &starter_raw())
    }

    /// `writeStarterProjectConfig(cwd)` — writes `<cwd>/.mcp.json`, **clobbering** whatever was
    /// there. Upstream does not merge here and neither does this: the caller is the setup panel,
    /// which only offers the action when the file does not exist.
    ///
    /// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
    pub fn write_starter_project_config(&self) -> McpResult<PathBuf> {
        let target = self.project_path();
        write_raw_config_object(&target, &starter_raw())?;
        Ok(target)
    }
}

/// `Array.isArray(raw.imports) ? raw.imports.filter(isString) : []` — the file's current list,
/// unvalidated (an unknown kind is preserved, exactly as upstream preserves it).
fn current_import_list(raw: &RawObject) -> Vec<String> {
    raw.get("imports")
        .and_then(RawJson::as_array)
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// `[...new Set([...currentImports, ...importKinds])]` — first-seen order, deduplicated.
fn merged_import_list(raw: &RawObject, import_kinds: &[ImportKind]) -> Vec<String> {
    let requested: Vec<String> =
        import_kinds.iter().map(|kind| kind.as_str().to_string()).collect();
    merge_imports(&current_import_list(raw), &requested)
}

// ===================================================================================================
// 14 · Discovery, conflicts and the fingerprint (MCP-059, MCP-097)
// ===================================================================================================

/// `ConfigDiscoveryPath` — one ladder rung, with nothing read off disk but its existence.
///
/// [`ConfigContext::config_discovery_paths`] is the render-time accessor and deliberately does
/// **not** parse (MCP-097); [`ConfigDiscoverySource`] is the parsed form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiscoveryPath {
    /// Upstream's `label`, rendered verbatim.
    pub label: &'static str,
    /// The read path.
    pub path: PathBuf,
    /// `existsSync(readPath)`.
    pub exists: bool,
}

/// `DiscoveredImportConfig` — a host-config family that resolved to a real file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredImportConfig {
    /// Which of the seven families.
    pub kind: ImportKind,
    /// The candidate that won (for `opencode`, the highest-precedence one).
    pub path: PathBuf,
}

/// `ConfigDiscoverySource["kind"]` — the panel's shared-vs-adapter split, which is
/// [`ConfigSourceSpec::shared`] rendered as a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryKind {
    /// A file other MCP-speaking tools also read.
    Shared,
    /// A file only this adapter owns. Upstream's spelling is `"pi"`; it is a fingerprint token, so
    /// it is not rebranded.
    Pi,
}

impl DiscoveryKind {
    /// The wire spelling used in conflicts and the fingerprint.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            DiscoveryKind::Shared => "shared",
            DiscoveryKind::Pi => "pi",
        }
    }
}

/// `ConfigDiscoverySource` — a ladder rung the panel has actually read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiscoverySource {
    /// Stable id; the first element of the fingerprint's `sources` tuple.
    pub id: SourceId,
    /// Rendered label.
    pub label: &'static str,
    /// The **read** path.
    pub path: PathBuf,
    /// `existsSync(path)`.
    pub exists: bool,
    /// Global or project.
    pub scope: SourceScope,
    /// Shared or adapter-owned.
    pub kind: DiscoveryKind,
    /// `Object.keys(loaded.mcpServers).length` — what the file actually declares, **before** the
    /// trust gate.
    pub server_count: usize,
    /// MCP-096, and not an upstream field: `false` for a project-scoped rung when the project is
    /// untrusted. The rung still appears, with its real [`Self::server_count`], so the panel can say
    /// *"present, not loaded"* instead of pretending the file is empty.
    ///
    /// **Not read in production yet**: the renderer is the `/mcp` setup panel (`TODO(MCP-394)`).
    pub contributes: bool,
}

/// `ImportConfigSummary` — a detected host config plus how many servers it declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportConfigSummary {
    /// Which family.
    pub kind: ImportKind,
    /// The winning candidate path.
    pub path: PathBuf,
    /// Servers after that family's own translation (`extractServers`).
    pub server_count: usize,
}

/// `HostConfigSummary` — an [`ImportConfigSummary`] plus whether it is actually being merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfigSummary {
    /// Which family.
    pub kind: ImportKind,
    /// The winning candidate path.
    pub path: PathBuf,
    /// Servers this family declares.
    pub server_count: usize,
    /// `hostConfigDiscovery === "on"` — detected-but-inactive is the `"prompt"` state's whole point.
    pub active: bool,
}

/// `McpConfigConflict["sources"][n]["kind"]` — three arms, because a host-discovered file is
/// neither shared nor adapter-owned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// A shared ladder file.
    Shared,
    /// An adapter-owned ladder file.
    Pi,
    /// A host-config import.
    Host,
}

impl ConflictKind {
    /// The wire spelling — part of the fingerprint string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictKind::Shared => "shared",
            ConflictKind::Pi => "pi",
            ConflictKind::Host => "host",
        }
    }
}

/// One `(kind, path)` pair a server name was seen in. Recorded at most once per pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSource {
    /// Shared / adapter-owned / host.
    pub kind: ConflictKind,
    /// The file it was seen in.
    pub path: PathBuf,
}

/// `McpConfigConflict` — a server name defined in more than one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfigConflict {
    /// The contested name.
    pub server_name: String,
    /// Every `(kind, path)` that defines it, in **record order**: host candidates first, then the
    /// ladder in precedence order.
    pub sources: Vec<ConflictSource>,
    /// `sources[sources.length - 1]` — last recorded wins, which is exactly what the merge does.
    pub winner: ConflictSource,
}

/// `RepoPromptDiscovery` — either "already configured, here" or "not configured, here is the
/// proposal".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RepoPromptDiscovery {
    /// A RepoPrompt server was found in a **shared** source.
    pub configured: bool,
    /// Where it was found.
    ///
    /// **Not read in production yet**: the renderer is the `/mcp` setup panel (`TODO(MCP-394)`).
    pub configured_path: Option<PathBuf>,
    /// The binary that was probed for, when not configured.
    pub executable_path: Option<PathBuf>,
    /// Where a one-key add would write: the nearest project root's `.mcp.json`, else
    /// `~/.config/mcp/mcp.json`.
    pub target_path: Option<PathBuf>,
    /// Always `"repoprompt"` when a proposal exists.
    pub server_name: Option<String>,
    /// The proposed entry.
    pub entry: Option<ServerEntry>,
}

/// `McpStandardConfigSummary` — the narrow summary, with its own fingerprint over `sources` alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStandardConfigSummary {
    /// Every ladder rung, read.
    pub sources: Vec<ConfigDiscoverySource>,
    /// Any shared rung with servers.
    pub has_shared_servers: bool,
    /// `JSON.stringify({ sources })` — note the wrapping object, which is *not* the same string as
    /// the full summary's `sources` field.
    pub fingerprint: String,
}

/// `McpDiscoverySummary` — the whole `/mcp setup` panel model (MCP-059).
#[derive(Debug, Clone)]
pub struct McpDiscoverySummary {
    /// The ladder, read.
    pub sources: Vec<ConfigDiscoverySource>,
    /// Detected host configs (empty when `include_host_configs` is `false`).
    pub imports: Vec<ImportConfigSummary>,
    /// The same list, tagged with whether discovery is `"on"`.
    pub host_configs: Vec<HostConfigSummary>,
    /// The merged `settings.hostConfigDiscovery`.
    pub host_config_discovery: HostConfigDiscovery,
    /// Agent Plugin roots and their server counts.
    pub agent_plugins: Vec<crate::agent_plugin::AgentPluginSummary>,
    /// Names defined more than once, sorted by [`locale_compare`].
    pub conflicts: Vec<McpConfigConflict>,
    /// `totalServerCount > 0 || imports.some(…) || hasAnyDetectedPaths`.
    pub has_any_config: bool,
    /// Any source exists, or any import or plugin was detected.
    pub has_any_detected_paths: bool,
    /// Any shared source — or any plugin — declares a server.
    pub has_shared_servers: bool,
    /// Any adapter-owned source declares a server.
    pub has_pi_owned_servers: bool,
    /// Ladder servers **plus** plugin servers. Counted per source, so a name defined twice counts
    /// twice — upstream's arithmetic, and the number the panel shows.
    pub total_server_count: usize,
    /// The change-detection key the panel polls; see [`ConfigContext::mcp_discovery_summary`].
    pub fingerprint: String,
    /// RepoPrompt's state.
    pub repo_prompt: RepoPromptDiscovery,
}

/// `String.prototype.localeCompare` — real ICU root collation, not code-point order (MCP-059).
///
/// Upstream sorts the conflict list with `localeCompare`, which is Node's default ICU root
/// collation: a primary case-blind comparison with lowercase ordering *before* uppercase as the
/// tertiary tie-break. Rust's `str::cmp` is code-point order and puts every uppercase letter before
/// every lowercase one — `["A", "a"]` under `cmp`, `["a", "A"]` under ICU — and the delta is not
/// cosmetic here: the conflict list is stringified into
/// [`ConfigContext::mcp_discovery_summary`]'s fingerprint, so a different order is a different
/// fingerprint and the onboarding panel re-fires on a config that did not change.
///
/// `feruca` with `Collator::new(Tailoring::default() /* CLDR root */, false /* non-ignorable
/// variable weighting */, true /* byte-value tie-break */)` is the configuration this workspace
/// already proved against Node twice — `cyrup-tools/src/tools/ls.rs:214` (13 mixed-case/accented/
/// dotted names) and `cyrup-config/src/model.rs:3123` (a captured `locale_compare.json` vector).
/// Reused verbatim rather than re-derived, because a third approximation is how the two existing
/// ones would drift apart.
///
/// Note the deliberate absence of a `.to_lowercase()` pre-step: `ls.rs` lowercases both keys
/// because *pi's `ls`* does (`ls.ts:150`), while `config.ts`'s conflict sort calls `localeCompare`
/// on the raw names. Collating raw is what reproduces the lowercase-first tertiary rule.
///
/// The collator is built per call. `feruca::Collator::collate` takes `&mut self` (it memoises), so
/// a shared one would need a lock; conflict lists are a handful of server names read once per
/// discovery-summary poll, so the allocation is not on any hot path.
#[must_use]
pub fn locale_compare(left: &str, right: &str) -> Ordering {
    let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
    collator.collate(left, right)
}

/// One `[id, exists, serverCount]` tuple of the fingerprint's `sources` array.
fn source_fingerprint_tuple(source: &ConfigDiscoverySource) -> RawJson {
    RawJson::Array(vec![
        RawJson::String(source.id.as_str().to_string()),
        RawJson::Bool(source.exists),
        RawJson::Number(source.server_count.into()),
    ])
}

/// `{ kind, path }` — the field order is load-bearing, because this object is stringified into the
/// fingerprint.
fn conflict_source_json(source: &ConflictSource) -> RawJson {
    let mut object = RawObject::new();
    object.insert("kind".to_string(), RawJson::String(source.kind.as_str().to_string()));
    object.insert("path".to_string(), RawJson::String(source.path.display().to_string()));
    RawJson::Object(object)
}

/// `{ serverName, sources, winner }`, in that order.
fn conflict_json(conflict: &McpConfigConflict) -> RawJson {
    let mut object = RawObject::new();
    object.insert("serverName".to_string(), RawJson::String(conflict.server_name.clone()));
    object.insert(
        "sources".to_string(),
        RawJson::Array(conflict.sources.iter().map(conflict_source_json).collect()),
    );
    object.insert("winner".to_string(), conflict_source_json(&conflict.winner));
    RawJson::Object(object)
}

impl ConfigContext {
    /// `getConfigDiscoveryPaths(overridePath, cwd)` — the ladder with an exists marker and
    /// **nothing parsed** (MCP-097).
    ///
    /// That is the whole contract: it is cheap enough to call on every render, where
    /// [`Self::config_source_summaries`] reads and validates every file and emits every warning.
    /// A port that implemented this in terms of the summary would change both the cost and the
    /// warning output of the setup panel.
    ///
    /// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
    #[must_use]
    pub fn config_discovery_paths(&self) -> Vec<ConfigDiscoveryPath> {
        self.sources()
            .into_iter()
            .map(|source| ConfigDiscoveryPath {
                label: source.label,
                exists: source.read_path.exists(),
                path: source.read_path,
            })
            .collect()
    }

    /// `findAvailableImportConfigs(cwd)` — which of the seven families resolve, in
    /// [`ImportKind::ALL`] order (MCP-097).
    ///
    /// This one **does** parse (through `resolveImportPath`) and warns with
    /// `` `Failed to discover imported MCP config from ${kind}:` ``.
    ///
    /// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
    #[must_use]
    pub fn find_available_import_configs(
        &self,
        diagnostics: &mut Vec<ConfigDiagnostic>,
    ) -> Vec<DiscoveredImportConfig> {
        ImportKind::ALL
            .into_iter()
            .filter_map(|kind| {
                resolve_import_path(kind, &self.home, self.dirs.cwd(), diagnostics)
                    .map(|path| DiscoveredImportConfig { kind, path })
            })
            .collect()
    }

    /// `getConfigSourceSummaries(sourceSpecs)` — read and validate every rung.
    #[must_use]
    pub fn config_source_summaries(
        &self,
        diagnostics: &mut Vec<ConfigDiagnostic>,
    ) -> Vec<ConfigDiscoverySource> {
        self.sources()
            .into_iter()
            .map(|source| {
                let loaded = read_validated_config(&source.read_path, diagnostics);
                ConfigDiscoverySource {
                    id: source.id,
                    label: source.label,
                    exists: source.read_path.exists(),
                    scope: source.scope,
                    kind: if source.shared { DiscoveryKind::Shared } else { DiscoveryKind::Pi },
                    server_count: loaded.map_or(0, |config| config.mcp_servers.len()),
                    contributes: self.source_contributes(&source),
                    path: source.read_path,
                }
            })
            .collect()
    }

    /// `getMcpStandardConfigSummary(overridePath, cwd)` — the narrow summary, with its own
    /// fingerprint over `sources` alone.
    ///
    /// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
    #[must_use]
    pub fn mcp_standard_config_summary(
        &self,
        diagnostics: &mut Vec<ConfigDiagnostic>,
    ) -> McpStandardConfigSummary {
        let sources = self.config_source_summaries(diagnostics);
        let has_shared_servers = sources
            .iter()
            .any(|source| source.kind == DiscoveryKind::Shared && source.server_count > 0);
        let mut root = RawObject::new();
        root.insert(
            "sources".to_string(),
            RawJson::Array(sources.iter().map(source_fingerprint_tuple).collect()),
        );
        McpStandardConfigSummary {
            sources,
            has_shared_servers,
            fingerprint: compact_json(&RawJson::Object(root)),
        }
    }

    /// `getConfigConflicts(sourceSpecs, imports, cwd)` — every name defined more than once.
    ///
    /// Record order **is** precedence order, because the winner is `sources[last]`: host candidates
    /// first (lowest precedence when enabled), then each ladder rung's `imports`-derived names, then
    /// that rung's own `mcpServers`. A `(kind, path)` pair is recorded once, so a name defined twice
    /// in the same file is not a conflict with itself.
    #[must_use]
    pub fn config_conflicts(
        &self,
        imports: &[ImportConfigSummary],
        diagnostics: &mut Vec<ConfigDiagnostic>,
    ) -> Vec<McpConfigConflict> {
        let mut seen: IndexMap<String, Vec<ConflictSource>> = IndexMap::new();

        for entry in imports {
            let Some(document) = load_imported_config(
                entry.kind,
                &self.home,
                self.dirs.cwd(),
                &format!("Failed to inspect imported MCP config from {}:", entry.kind),
                diagnostics,
            ) else {
                continue;
            };
            for name in
                extract_servers(&document.value, entry.kind, &document.path, diagnostics).keys()
            {
                record_conflict_source(
                    &mut seen,
                    name,
                    ConflictSource { kind: ConflictKind::Host, path: document.path.clone() },
                );
            }
        }

        for source in self.sources() {
            let Some(loaded) = read_validated_config(&source.read_path, diagnostics) else {
                continue;
            };
            for raw_kind in &loaded.imports {
                let Some(kind) = ImportKind::parse(raw_kind) else {
                    continue;
                };
                let Some(document) = load_imported_config(
                    kind,
                    &self.home,
                    self.dirs.cwd(),
                    &format!("Failed to inspect imported MCP config from {kind}:"),
                    diagnostics,
                ) else {
                    continue;
                };
                for name in
                    extract_servers(&document.value, kind, &document.path, diagnostics).keys()
                {
                    record_conflict_source(
                        &mut seen,
                        name,
                        ConflictSource { kind: ConflictKind::Host, path: document.path.clone() },
                    );
                }
            }
            let kind = if source.shared { ConflictKind::Shared } else { ConflictKind::Pi };
            for name in loaded.mcp_servers.keys() {
                record_conflict_source(
                    &mut seen,
                    name,
                    ConflictSource { kind, path: source.read_path.clone() },
                );
            }
        }

        let mut conflicts: Vec<McpConfigConflict> = seen
            .into_iter()
            .filter(|(_, sources)| sources.len() > 1)
            .filter_map(|(server_name, sources)| {
                let winner = sources.last()?.clone();
                Some(McpConfigConflict { server_name, sources, winner })
            })
            .collect();
        conflicts.sort_by(|left, right| locale_compare(&left.server_name, &right.server_name));
        conflicts
    }

    /// `getMcpDiscoverySummary(overridePath, cwd, { includeHostConfigs })` — the panel's whole model
    /// (MCP-059).
    ///
    /// # The fingerprint
    ///
    /// `JSON.stringify` over an **object literal**, so it is insertion-ordered:
    /// `sources`, `imports`, `agentPlugins`, `hostConfigDiscovery`, `conflicts`. The panel polls it
    /// as an opaque string, so the key order, the tuple shapes and the `null` a missing plugin name
    /// stringifies to (`JSON.stringify(["p", undefined, 0])` is `["p",null,0]`) are all part of the
    /// contract. It is built through [`RawJson`] rather than `serde_json::Value` for the same reason
    /// everything else in this module is: a `Value` would sort the keys.
    ///
    /// One recorded divergence from upstream, and it is in the *reads*, not the answer: upstream
    /// walks the ladder three times over (`getConfiguredHostConfigDiscovery` → `getMergedSettings`,
    /// then `getMergedSettings` again, then `getConfigSourceSummaries`), so a malformed file warns
    /// three times. Here the settings walk happens once.
    ///
    /// **No production caller yet.** This is the `/mcp` panel's data layer; the dispatcher that would call it is `TODO(MCP-394)` (`crate::ui`'s own note at `open_mcp_panel`, and `crate::extension`'s `/mcp` arm, which keeps the trait's default answer until MCP-394 lands).
    #[must_use]
    pub fn mcp_discovery_summary(
        &self,
        include_host_configs: bool,
        diagnostics: &mut Vec<ConfigDiagnostic>,
    ) -> McpDiscoverySummary {
        let sources = self.config_source_summaries(diagnostics);

        let imports: Vec<ImportConfigSummary> = if include_host_configs {
            ImportKind::ALL
                .into_iter()
                .filter_map(|kind| {
                    let document = load_imported_config(
                        kind,
                        &self.home,
                        self.dirs.cwd(),
                        &format!("Failed to inspect imported MCP config from {kind}:"),
                        diagnostics,
                    )?;
                    let server_count =
                        extract_servers(&document.value, kind, &document.path, diagnostics).len();
                    Some(ImportConfigSummary { kind, path: document.path, server_count })
                })
                .collect()
        } else {
            Vec::new()
        };

        let settings = self.merged_settings(diagnostics);
        let host_config_discovery =
            settings.as_ref().map_or(HostConfigDiscovery::Off, McpSettings::host_config_discovery);
        let host_configs: Vec<HostConfigSummary> = imports
            .iter()
            .map(|entry| HostConfigSummary {
                kind: entry.kind,
                path: entry.path.clone(),
                server_count: entry.server_count,
                active: host_config_discovery == HostConfigDiscovery::On,
            })
            .collect();

        let plugin_paths: Vec<String> = settings
            .as_ref()
            .map(|settings| settings.agent_plugin_paths().to_vec())
            .unwrap_or_default();
        let agent_plugins = crate::agent_plugin::agent_plugin_summaries(&plugin_paths, &self.dirs);

        let total_server_count = sources
            .iter()
            .map(|source| source.server_count)
            .chain(agent_plugins.iter().map(|plugin| plugin.server_count))
            .fold(0usize, usize::saturating_add);
        let has_shared_servers = sources
            .iter()
            .any(|source| source.kind == DiscoveryKind::Shared && source.server_count > 0)
            || agent_plugins.iter().any(|plugin| plugin.server_count > 0);
        let has_pi_owned_servers = sources
            .iter()
            .any(|source| source.kind == DiscoveryKind::Pi && source.server_count > 0);
        let has_any_detected_paths = sources.iter().any(|source| source.exists)
            || !imports.is_empty()
            || !agent_plugins.is_empty();
        let has_any_config = total_server_count > 0
            || imports.iter().any(|entry| entry.server_count > 0)
            || has_any_detected_paths;

        let conflicts = self.config_conflicts(&imports, diagnostics);

        let mut root = RawObject::new();
        root.insert(
            "sources".to_string(),
            RawJson::Array(sources.iter().map(source_fingerprint_tuple).collect()),
        );
        root.insert(
            "imports".to_string(),
            RawJson::Array(
                imports
                    .iter()
                    .map(|entry| {
                        RawJson::Array(vec![
                            RawJson::String(entry.kind.as_str().to_string()),
                            RawJson::String(entry.path.display().to_string()),
                            RawJson::Number(entry.server_count.into()),
                        ])
                    })
                    .collect(),
            ),
        );
        root.insert(
            "agentPlugins".to_string(),
            RawJson::Array(
                agent_plugins
                    .iter()
                    .map(|plugin| {
                        RawJson::Array(vec![
                            RawJson::String(plugin.path.display().to_string()),
                            // `JSON.stringify` renders an `undefined` array element as `null`.
                            plugin.name.clone().map_or(RawJson::Null, RawJson::String),
                            RawJson::Number(plugin.server_count.into()),
                        ])
                    })
                    .collect(),
            ),
        );
        root.insert(
            "hostConfigDiscovery".to_string(),
            RawJson::String(
                match host_config_discovery {
                    HostConfigDiscovery::Off => "off",
                    HostConfigDiscovery::Prompt => "prompt",
                    HostConfigDiscovery::On => "on",
                }
                .to_string(),
            ),
        );
        root.insert(
            "conflicts".to_string(),
            RawJson::Array(conflicts.iter().map(conflict_json).collect()),
        );
        let fingerprint = compact_json(&RawJson::Object(root));

        let repo_prompt = self.detect_repo_prompt(&sources, diagnostics);

        McpDiscoverySummary {
            sources,
            imports,
            host_configs,
            host_config_discovery,
            agent_plugins,
            conflicts,
            has_any_config,
            has_any_detected_paths,
            has_shared_servers,
            has_pi_owned_servers,
            total_server_count,
            fingerprint,
            repo_prompt,
        }
    }
}

/// `record(name, source)` from `getConfigConflicts` — append unless this exact `(kind, path)` pair
/// is already recorded for that name.
fn record_conflict_source(
    seen: &mut IndexMap<String, Vec<ConflictSource>>,
    name: &str,
    source: ConflictSource,
) {
    let entries = seen.entry(name.to_string()).or_default();
    if !entries.contains(&source) {
        entries.push(source);
    }
}

// ===================================================================================================
// 15 · RepoPrompt and the curated presets (MCP-060)
// ===================================================================================================

/// `KnownServerPreset` — one of the five curated servers `/mcp setup` offers.
///
/// **All four fields are user-visible**: the panel renders `name` as the row title and `summary` as
/// its description, `id` is the selection key, and `entry` is what gets written. A port that carried
/// only the `entry` would produce a panel of unlabelled URLs.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownServerPreset {
    /// Stable selection key, and the server name a write uses.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// One-line description, rendered under the name.
    pub summary: &'static str,
    /// The entry written into the chosen file.
    pub entry: ServerEntry,
}

/// `KNOWN_SERVER_PRESETS` — the five, in panel order (§15's table).
///
/// A function rather than a `const` because [`ServerEntry`]'s fields are `Option<String>`, which
/// cannot be built in const context. The strings are `&'static str`; only the entries allocate.
///
/// The `chrome-devtools` preset spawns a third-party Node MCP server over stdio through `npx`. That
/// is inherent to MCP and is exactly what `cyrup_ext::caps::proc::npx_resolver` already exists to
/// pre-resolve — it is an external process, not a JS runtime inside cyrup.
#[must_use]
pub fn known_server_presets() -> Vec<KnownServerPreset> {
    let remote = |url: &str, auth: Option<AuthKind>| ServerEntry {
        url: Some(url.to_string()),
        auth: auth.map(AuthMode::Named),
        protocol_version: Some(ProtocolVersionSetting::Auto),
        ..ServerEntry::default()
    };
    vec![
        KnownServerPreset {
            id: "deepwiki",
            name: "DeepWiki",
            summary: "Ask questions about public GitHub repositories.",
            entry: remote("https://mcp.deepwiki.com/mcp", None),
        },
        KnownServerPreset {
            id: "context7",
            name: "Context7",
            summary: "Look up current library documentation and examples.",
            entry: remote("https://mcp.context7.com/mcp", None),
        },
        KnownServerPreset {
            id: "notion",
            name: "Notion",
            summary: "Search and work with your Notion workspace.",
            entry: remote("https://mcp.notion.com/mcp", Some(AuthKind::Oauth)),
        },
        KnownServerPreset {
            id: "github",
            name: "GitHub",
            summary: "Work with GitHub through your Copilot account.",
            entry: remote("https://api.githubcopilot.com/mcp", Some(AuthKind::Oauth)),
        },
        KnownServerPreset {
            id: "chrome-devtools",
            name: "Chrome DevTools",
            summary: "Inspect and automate a local Chrome browser.",
            entry: ServerEntry {
                command: Some("npx".to_string()),
                args: Some(vec!["-y".to_string(), "chrome-devtools-mcp@1.6.0".to_string()]),
                ..ServerEntry::default()
            },
        },
    ]
}

/// `REPOPROMPT_BINARY_CANDIDATES` — `~/RepoPrompt/repoprompt_cli` then the macOS app bundle's
/// binary. The first is home-relative, which is why the pair is a function and only the second is a
/// `const` ([`REPOPROMPT_APP_BINARY`]).
#[must_use]
pub fn repoprompt_binary_candidates(home: &Path) -> [PathBuf; 2] {
    [home.join("RepoPrompt").join("repoprompt_cli"), PathBuf::from(REPOPROMPT_APP_BINARY)]
}

/// `isRepoPromptServer(name, entry)` — four independent tests, all case-insensitive.
///
/// The name contains `repoprompt` or **is** exactly `rp`; or the command contains `repoprompt` or
/// `rp-mcp` or ends with `repoprompt_cli`; or any argument contains `repoprompt`. Deliberately
/// generous: a false positive costs a hidden "add RepoPrompt" offer, a false negative costs a
/// duplicate server.
#[must_use]
pub fn is_repo_prompt_server(name: &str, entry: &ServerEntry) -> bool {
    let normalized_name = name.to_lowercase();
    if normalized_name.contains("repoprompt") || normalized_name == "rp" {
        return true;
    }

    let command = entry.command.as_deref().unwrap_or_default().to_lowercase();
    if command.contains("repoprompt")
        || command.contains("rp-mcp")
        || command.ends_with("repoprompt_cli")
    {
        return true;
    }

    entry
        .args
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|arg| arg.to_lowercase().contains("repoprompt"))
}

/// `findProjectRoot(cwd)` — walk **up** for the first directory holding any of
/// [`PROJECT_ROOT_MARKERS`] (`.git`, `package.json`, `.mcp.json`, `.cyrup`), or `None` at the
/// filesystem root.
///
/// Upstream's fourth marker is `.pi`; it is renamed with the rest of the project override directory
/// (module header). The walk terminates on `dirname(current) === current`, which in Rust is
/// `Path::parent() == None`.
#[must_use]
pub fn find_project_root(cwd: &Path) -> Option<PathBuf> {
    let mut current = normalize_lexical(cwd);
    loop {
        if PROJECT_ROOT_MARKERS.iter().any(|marker| current.join(marker).exists()) {
            return Some(current);
        }
        let parent = current.parent()?.to_path_buf();
        if parent == current {
            return None;
        }
        current = parent;
    }
}

/// `buildRepoPromptEntry(executablePath)` — `{ command, args: [], lifecycle: "lazy" }`.
///
/// The empty `args` array is written explicitly rather than omitted: it is what upstream writes into
/// the user's file, and it is also the shape `computeServerHash` hashes.
#[must_use]
pub fn build_repo_prompt_entry(executable_path: &Path) -> ServerEntry {
    ServerEntry {
        command: Some(executable_path.display().to_string()),
        args: Some(Vec::new()),
        lifecycle: Some(ServerLifecycle::Lazy),
        ..ServerEntry::default()
    }
}

impl ConfigContext {
    /// `detectRepoPrompt(summary, cwd)` — MCP-060.
    ///
    /// Two phases. First, scan the **shared** sources that actually declare servers (adapter-owned
    /// files are skipped: the offer is about the shared ecosystem) and report the first file
    /// carrying a RepoPrompt-looking server. Otherwise probe the two binary candidates and, if one
    /// exists, propose writing `repoprompt` into `findProjectRoot(cwd)/.mcp.json` — falling back to
    /// `~/.config/mcp/mcp.json` when there is no project root at all.
    #[must_use]
    pub fn detect_repo_prompt(
        &self,
        sources: &[ConfigDiscoverySource],
        diagnostics: &mut Vec<ConfigDiagnostic>,
    ) -> RepoPromptDiscovery {
        for source in sources {
            if source.kind != DiscoveryKind::Shared || source.server_count == 0 {
                continue;
            }
            let Some(config) = read_validated_config(&source.path, diagnostics) else {
                continue;
            };
            if config.mcp_servers.iter().any(|(name, entry)| is_repo_prompt_server(name, entry)) {
                return RepoPromptDiscovery {
                    configured: true,
                    configured_path: Some(source.path.clone()),
                    ..RepoPromptDiscovery::default()
                };
            }
        }

        let Some(executable_path) = repoprompt_binary_candidates(&self.home)
            .into_iter()
            .find(|candidate| candidate.exists())
        else {
            return RepoPromptDiscovery::default();
        };

        let target_path = find_project_root(self.dirs.cwd())
            .map_or_else(|| self.generic_global_path(), |root| root.join(PROJECT_CONFIG_NAME));
        RepoPromptDiscovery {
            configured: false,
            configured_path: None,
            entry: Some(build_repo_prompt_entry(&executable_path)),
            executable_path: Some(executable_path),
            target_path: Some(target_path),
            server_name: Some("repoprompt".to_string()),
        }
    }
}

// ===================================================================================================
// Tests — one per **verify** paragraph of the units this module owns
// ===================================================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A hermetic ladder: a temp `home`, a temp `<agent_dir>` and a temp `cwd`, wired so that no
    /// test can see the developer's real `~/.config/mcp/mcp.json` or any of the seven host-config
    /// families. `home` is a **field** on [`ConfigContext`] precisely so this is possible without
    /// mutating process env, which edition 2024 makes `unsafe`.
    struct Fixture {
        _dir: tempfile::TempDir,
        home: PathBuf,
        agent_dir: PathBuf,
        cwd: PathBuf,
    }

    impl Fixture {
        fn new() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            let agent_dir = dir.path().join("agent");
            let cwd = dir.path().join("project");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&agent_dir).unwrap();
            std::fs::create_dir_all(&cwd).unwrap();
            Fixture { _dir: dir, home, agent_dir, cwd }
        }

        fn context(&self) -> ConfigContext {
            ConfigContext::new(McpDirs::new(self.agent_dir.clone(), self.cwd.clone()), None)
                .with_home(self.home.clone())
        }

        fn write(&self, path: &Path, text: &str) {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, text).unwrap();
        }

        /// `~/.config/mcp/mcp.json` — the lowest, and `shared`, ladder rung.
        fn shared_global(&self) -> PathBuf {
            self.home.join(".config").join("mcp").join("mcp.json")
        }

        /// `<agent_dir>/mcp.json` — `userPath`, the adapter-owned file every shared source's writes
        /// are redirected to.
        fn user_path(&self) -> PathBuf {
            self.agent_dir.join("mcp.json")
        }

        /// `<cwd>/.cyrup/mcp.json` — the project override the disabled writer owns.
        fn project_override(&self) -> PathBuf {
            self.cwd.join(PROJECT_OVERRIDE_DIR).join(crate::dirs::MCP_CONFIG_FILE)
        }
    }

    fn config_message(error: &McpError) -> String {
        match error {
            McpError::Config(message) => message.clone(),
            other => format!("not a config error: {other:?}"),
        }
    }

    // -- MCP-061 -----------------------------------------------------------------------------

    #[test]
    fn write_preserves_unknown_keys_and_normalises_the_legacy_server_key() {
        let fixture = Fixture::new();
        let path = fixture.user_path();
        fixture.write(
            &path,
            "{\n  \"$schema\": \"https://example/schema.json\",\n  \"mcp-servers\": {\"a\": {\"command\": \"x\"}}\n}\n",
        );

        let mut raw = read_raw_config_object(&path);
        let servers = get_servers_object(&raw);
        assert!(servers.contains_key("a"), "the legacy key is still READ");
        set_servers_object(&mut raw, servers);
        write_raw_config_object(&path, &raw).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"$schema\""), "an unknown top-level key must survive a write");
        assert!(text.contains("\"mcpServers\""), "the write normalises the spelling");
        assert!(!text.contains("\"mcp-servers\""), "and deletes the legacy one");
        assert!(text.ends_with("}\n"), "2-space JSON plus a trailing newline");

        let leftovers: Vec<_> = std::fs::read_dir(&fixture.agent_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "the temp file is renamed away, never left behind");
    }

    #[test]
    fn unparseable_and_missing_files_read_as_empty_objects() {
        let fixture = Fixture::new();
        let path = fixture.user_path();
        assert!(read_raw_config_object(&path).is_empty(), "missing ⇒ {{}}");
        fixture.write(&path, "{{{");
        assert!(read_raw_config_object(&path).is_empty(), "unparseable ⇒ {{}}, silently");
        fixture.write(&path, "[1, 2]");
        assert!(read_raw_config_object(&path).is_empty(), "a non-object root ⇒ {{}}");
    }

    // -- MCP-062 -----------------------------------------------------------------------------

    #[test]
    fn unified_diff_matches_upstream_line_for_line() {
        assert_eq!(build_unified_diff("same", "same"), "(no changes)");
        assert_eq!(
            build_unified_diff("a\nb", "a\nx\nb"),
            "--- before\n+++ after\n  a\n+ x\n  b",
            "insert-only"
        );
        assert_eq!(
            build_unified_diff("a\nx\nb", "a\nb"),
            "--- before\n+++ after\n  a\n- x\n  b",
            "delete-only"
        );
        // The tie-break: on an equal-length LCS the ADDITION is emitted first. This is the line
        // `similar` would render differently, and the panel prints it verbatim.
        assert_eq!(
            build_unified_diff("a\nb", "a\nc"),
            "--- before\n+++ after\n  a\n+ c\n- b",
            "replace ⇒ addition before deletion"
        );
    }

    // -- MCP-099 -----------------------------------------------------------------------------

    #[test]
    fn preview_before_text_is_the_reserialised_parse_not_the_file_bytes() {
        let fixture = Fixture::new();
        let path = fixture.user_path();
        fixture.write(
            &path,
            "{\n    // four-space indent and a comment\n    \"mcpServers\": {},\n}\n",
        );

        let mut next = RawObject::new();
        next.insert(SERVERS_KEY.to_string(), RawJson::Object(RawObject::new()));
        let preview = build_config_write_preview(&path, &next);

        assert!(preview.existed);
        assert!(!preview.before_text.contains("//"), "comments are stripped by the parse");
        assert_eq!(preview.before_text, "{\n  \"mcpServers\": {}\n}\n", "reindented to two spaces");
        // `changed` is computed against that NORMALISED text, so a semantically identical file is
        // `changed: false` — while the bytes on disk still differ from what the write would produce.
        assert!(!preview.changed);
        assert_ne!(preview.before_text, std::fs::read_to_string(&path).unwrap());

        // A hyphenated key IS a change, because `setServersObject` normalises it.
        fixture.write(&path, "{\n  \"mcp-servers\": {}\n}\n");
        let preview = build_config_write_preview(&path, &next);
        assert!(preview.changed);
        assert!(preview.diff_text.contains("- ") && preview.diff_text.contains("+ "));
    }

    // -- MCP-063 -----------------------------------------------------------------------------

    #[test]
    fn disabled_override_round_trips_without_copying_credentials() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.shared_global(),
            "{\"mcpServers\":{\"foo\":{\"url\":\"https://foo.example/mcp\",\"bearerToken\":\"s3cret\"}}}",
        );
        let context = fixture.context();

        let disabled = context.write_project_server_disabled_override("foo", true).unwrap();
        assert!(disabled.changed);
        assert_eq!(disabled.path, fixture.project_override());
        let text = std::fs::read_to_string(fixture.project_override()).unwrap();
        assert!(text.contains("\"disabled\": true"));
        assert!(!text.contains("bearerToken"), "a definition is NEVER copied into the project file");
        assert!(!text.contains("https://foo.example/mcp"));

        // Nothing lower is disabled ⇒ enabling deletes the key, and the now-empty entry with it.
        let enabled = context.write_project_server_disabled_override("foo", false).unwrap();
        assert!(enabled.changed);
        let raw = read_raw_config_object(&fixture.project_override());
        let servers = get_servers_object(&raw);
        assert!(!servers.contains_key("foo"), "an empty entry is removed, not left as a husk");

        // A second enable is a no-op: no entry, nothing to write.
        let again = context.write_project_server_disabled_override("foo", false).unwrap();
        assert!(!again.changed);
    }

    #[test]
    fn enabling_writes_an_explicit_false_only_when_a_lower_source_is_disabled() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.shared_global(),
            "{\"mcpServers\":{\"foo\":{\"url\":\"https://foo.example/mcp\",\"disabled\":true}}}",
        );
        let context = fixture.context();

        let result = context.write_project_server_disabled_override("foo", false).unwrap();
        assert!(result.changed);
        let text = std::fs::read_to_string(fixture.project_override()).unwrap();
        assert!(text.contains("\"disabled\": false"), "the override has to out-vote the base");
    }

    #[test]
    fn disabled_override_reports_its_four_exact_errors() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let path = fixture.project_override();
        let shown = path.display().to_string();

        fixture.write(&path, "[]");
        assert_eq!(
            config_message(&context.write_project_server_disabled_override("foo", true).unwrap_err()),
            format!("Failed to read project MCP override at {shown}: root value must be an object")
        );

        fixture.write(&path, "{\"mcpServers\": 5}");
        assert_eq!(
            config_message(&context.write_project_server_disabled_override("foo", true).unwrap_err()),
            format!("Failed to update project MCP override at {shown}: mcpServers must be an object")
        );

        fixture.write(&path, "{\"mcpServers\": {\"foo\": 5}}");
        assert_eq!(
            config_message(&context.write_project_server_disabled_override("foo", true).unwrap_err()),
            format!("Failed to update project MCP override at {shown}: server \"foo\" must be an object")
        );

        fixture.write(&path, "{\"imports\": [\"not-a-host\"]}");
        assert_eq!(
            config_message(
                &context.write_project_server_disabled_override("foo", false).unwrap_err()
            ),
            format!(
                "Failed to update project MCP override at {shown}: imports contains an unsupported config kind"
            )
        );
    }

    // -- MCP-064 -----------------------------------------------------------------------------

    #[test]
    fn provenance_points_a_shared_server_at_the_adapter_owned_file() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.shared_global(),
            "{\"mcpServers\":{\"alpha\":{\"url\":\"https://alpha.example/mcp\"}}}",
        );
        fixture.write(
            &fixture.user_path(),
            "{\"mcpServers\":{\"beta\":{\"command\":\"b\",\"customKey\":1}}}",
        );
        let context = fixture.context();
        let mut diagnostics = Vec::new();
        let provenance = context.server_provenance(&mut diagnostics);

        let alpha = provenance.get("alpha").unwrap();
        assert_eq!(alpha.path, fixture.user_path(), "writes never land in the shared file");
        assert_eq!(alpha.kind, SourceKind::Import);
        assert_eq!(alpha.import_kind.as_deref(), Some("global MCP config"));

        let beta = provenance.get("beta").unwrap();
        assert_eq!(beta.path, fixture.user_path());
        assert_eq!(beta.kind, SourceKind::User);
        assert!(beta.import_kind.is_none());
    }

    #[test]
    fn direct_tools_write_materialises_imports_and_patches_raw_entries() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.shared_global(),
            "{\"mcpServers\":{\"alpha\":{\"url\":\"https://alpha.example/mcp\"}}}",
        );
        fixture.write(
            &fixture.user_path(),
            "{\"mcpServers\":{\"beta\":{\"command\":\"b\",\"customKey\":1}}}",
        );
        let context = fixture.context();
        let mut diagnostics = Vec::new();
        let provenance = context.server_provenance(&mut diagnostics);
        let loaded = context.load().config;

        let mut changes: IndexMap<String, BoolOrList> = IndexMap::new();
        changes.insert("alpha".to_string(), BoolOrList::All(true));
        changes.insert("beta".to_string(), BoolOrList::Named(vec!["read_x".to_string()]));
        write_direct_tools_config(&changes, &provenance, &loaded).unwrap();

        let text = std::fs::read_to_string(fixture.user_path()).unwrap();
        assert!(
            text.contains("https://alpha.example/mcp"),
            "an imported server materialises its MERGED definition into the adapter's file"
        );
        assert!(text.contains("\"directTools\": true"));
        assert!(text.contains("\"customKey\": 1"), "a raw patch keeps unknown keys on the entry");
        assert!(text.contains("\"read_x\""));
    }

    // -- MCP-065 -----------------------------------------------------------------------------

    #[test]
    fn compatibility_imports_are_idempotent() {
        let fixture = Fixture::new();
        let context = fixture.context();

        let first = context.ensure_compatibility_imports(&[ImportKind::Cursor]).unwrap();
        assert_eq!(first.added, vec![ImportKind::Cursor]);
        assert_eq!(first.path, fixture.user_path());
        let after_first = std::fs::read_to_string(fixture.user_path()).unwrap();
        assert!(after_first.contains("\"cursor\""));
        assert!(after_first.contains("\"mcpServers\": {}"), "setServersObject runs unconditionally");

        let second = context.ensure_compatibility_imports(&[ImportKind::Cursor]).unwrap();
        assert!(second.added.is_empty(), "nothing added ⇒ nothing written");
        assert_eq!(std::fs::read_to_string(fixture.user_path()).unwrap(), after_first);
    }

    #[test]
    fn starter_and_shared_entry_writers_produce_the_documented_files() {
        let fixture = Fixture::new();
        let context = fixture.context();

        let starter = context.write_starter_project_config().unwrap();
        assert_eq!(starter, fixture.cwd.join(PROJECT_CONFIG_NAME));
        assert_eq!(std::fs::read_to_string(&starter).unwrap(), "{\n  \"mcpServers\": {}\n}\n");

        let entry = ServerEntry { url: Some("https://x.example/mcp".to_string()), ..ServerEntry::default() };
        let preview = preview_shared_server_entry(&starter, "x", &entry);
        assert!(preview.changed);
        write_shared_server_entry(&starter, "x", &entry).unwrap();
        let text = std::fs::read_to_string(&starter).unwrap();
        assert!(text.contains("\"x\""));
        assert!(text.contains("https://x.example/mcp"));
        assert!(!text.contains("null"), "absent fields are skipped, never written as null");
    }

    // -- MCP-059 -----------------------------------------------------------------------------

    #[test]
    fn discovery_fingerprint_is_insertion_ordered_and_change_detecting() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut diagnostics = Vec::new();

        let empty = context.mcp_discovery_summary(true, &mut diagnostics);
        // A `\` at the end of a line in a Rust string literal eats the newline AND the next line's
        // leading whitespace, so this is one unbroken golden string.
        assert_eq!(
            empty.fingerprint,
            "{\"sources\":[[\"shared-global\",false,0],[\"agents-global\",false,0],\
             [\"agents-nested-global\",false,0],[\"pi-global\",false,0],\
             [\"shared-project\",false,0],[\"pi-project\",false,0]],\"imports\":[],\
             \"agentPlugins\":[],\"hostConfigDiscovery\":\"off\",\"conflicts\":[]}"
        );
        assert!(!empty.has_any_config);

        fixture.write(&fixture.user_path(), "{\"mcpServers\":{\"one\":{\"command\":\"x\"}}}");
        let one = context.mcp_discovery_summary(true, &mut diagnostics);
        assert_ne!(one.fingerprint, empty.fingerprint, "a new server changes the fingerprint");
        assert_eq!(one.total_server_count, 1);
        assert!(one.has_pi_owned_servers && !one.has_shared_servers);

        fixture.write(&fixture.cwd.join("unrelated.txt"), "noise");
        let again = context.mcp_discovery_summary(true, &mut diagnostics);
        assert_eq!(again.fingerprint, one.fingerprint, "an unrelated file does not");
    }

    #[test]
    fn conflicts_record_every_source_and_let_the_last_one_win() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.shared_global(),
            "{\"mcpServers\":{\"dup\":{\"command\":\"shared\"},\"only-here\":{\"command\":\"z\"}}}",
        );
        fixture.write(
            &fixture.user_path(),
            "{\"mcpServers\":{\"dup\":{\"command\":\"owned\"}}}",
        );
        let context = fixture.context();
        let mut diagnostics = Vec::new();
        let conflicts = context.config_conflicts(&[], &mut diagnostics);

        assert_eq!(conflicts.len(), 1);
        let conflict = conflicts.first().unwrap();
        assert_eq!(conflict.server_name, "dup");
        assert_eq!(conflict.sources.len(), 2);
        assert_eq!(conflict.sources.first().unwrap().kind, ConflictKind::Shared);
        assert_eq!(conflict.winner.kind, ConflictKind::Pi);
        assert_eq!(conflict.winner.path, fixture.user_path(), "last recorded wins, as the merge does");
    }

    // -- MCP-047 wiring: the plugin ladder rung ----------------------------------------------

    /// `settings.agentPluginPaths` is resolved against **this context's cwd**, and a rejected
    /// plugin server surfaces upstream's own warning text with no re-wrapping.
    ///
    /// Both halves were wrong before: `load_plugin_config` called the two-argument
    /// `load_agent_plugins`, whose cwd comes from `std::env::current_dir()` — the *process*'s, which
    /// in a test binary is the crate directory and in an SDK embedding is whatever the host was
    /// launched from — so a relative `agentPluginPaths` entry resolved somewhere else entirely. And
    /// it destructured the `(warning, reason)` tuple as if `.0` were a plugin NAME, producing
    /// `Skipping Agent Plugin server "<a whole English sentence>": <Debug of the enum>` and putting
    /// that sentence in `ConfigDiagnostic::server`, a field the `/mcp setup` panel renders as a
    /// server key.
    #[test]
    fn plugin_paths_resolve_against_this_contexts_cwd_and_keep_upstream_warning_text() {
        const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
        const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

        let fixture = Fixture::new();
        // A RELATIVE path, which is the whole point: it can only resolve through `dirs.cwd()`.
        let root = fixture.cwd.join("plugins").join("acme");
        fixture.write(
            &root.join("plugin.json"),
            &format!(r#"{{"$schema":"{PLUGIN_SCHEMA}","name":"acme"}}"#),
        );
        fixture.write(
            &root.join("mcp.json"),
            &format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{
                    "good": {{"type":"stdio","command":"./bin/server"}},
                    "bad": {{"type":"stdio","command":42}}
                }}}}"#
            ),
        );
        fixture.write(
            &fixture.user_path(),
            r#"{"settings":{"agentPluginPaths":["plugins/acme"]},"mcpServers":{}}"#,
        );

        let loaded = fixture.context().load();

        // (1) The plugin was found through the relative path, so its VALID server is in the config
        //     under the namespaced name the loader mints.
        let names: Vec<&str> = loaded.config.mcp_servers.keys().map(String::as_str).collect();
        assert!(
            names.iter().any(|name| name.contains("good")),
            "a relative `agentPluginPaths` entry must resolve against the context cwd; got {names:?}"
        );

        // (2) The invalid server was skipped with upstream's sentence, unwrapped.
        let plugin_diagnostics: Vec<&ConfigDiagnostic> = loaded
            .diagnostics
            .iter()
            .filter(|d| d.message.starts_with("Agent Plugin "))
            .collect();
        assert_eq!(
            plugin_diagnostics.len(),
            1,
            "exactly one rejection, carrying upstream's text: {:?}",
            loaded.diagnostics
        );
        let diagnostic = plugin_diagnostics.first().expect("one rejection, asserted above");
        assert!(
            diagnostic.message.contains("skips invalid MCP server bad"),
            "upstream's `skip_message` verbatim, not a re-wrap: {}",
            diagnostic.message
        );
        assert!(
            !diagnostic.message.starts_with("Skipping Agent Plugin server"),
            "the old re-wrap is gone: {}",
            diagnostic.message
        );
        assert_eq!(
            diagnostic.server, None,
            "`server` is a `mcpServers` KEY or nothing — never a sentence"
        );
    }

    #[test]
    fn locale_compare_orders_case_the_way_icu_does() {
        assert_eq!(locale_compare("apple", "banana"), Ordering::Less);
        assert_eq!(locale_compare("Banana", "apple"), Ordering::Greater, "case-blind primary");
        assert_eq!(locale_compare("a", "A"), Ordering::Less, "lowercase first at the tie-break");
        assert_eq!(locale_compare("same", "same"), Ordering::Equal);
        // The vector that separates real UCA from the case-folded `str::cmp` this used to be:
        // `é` is U+00E9, so ANY code-point comparison sorts it after `z` (U+007A), while ICU root
        // collates it as a variant of `e`. Node agrees — `"é".localeCompare("z") === -1` — and this
        // is the same `feruca` configuration `cyrup-tools/src/tools/ls.rs:219` pins with
        // `[…, "e", "é", "z", "Z", …]`.
        assert_eq!(
            locale_compare("é", "z"),
            Ordering::Less,
            "accents collate by base letter, not by code point"
        );
        // Digits before letters, and `"10"` after `"2"`: UCA is not numeric-aware, so this is the
        // ordering `mcpServers` keys like `srv-10` / `srv-2` actually get.
        assert_eq!(locale_compare("1", "a"), Ordering::Less);
        assert_eq!(locale_compare("10", "2"), Ordering::Less);
    }

    // -- MCP-096 -----------------------------------------------------------------------------

    #[test]
    fn an_untrusted_project_contributes_no_servers_but_still_appears() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.cwd.join(PROJECT_CONFIG_NAME),
            "{\"mcpServers\":{\"proj\":{\"command\":\"x\"}}}",
        );
        let trusted = fixture.context();
        assert!(trusted.load().config.mcp_servers.contains_key("proj"));

        let untrusted = fixture.context().with_project_trusted(false);
        assert!(untrusted.load().config.mcp_servers.is_empty(), "untrusted ⇒ zero servers");

        let mut diagnostics = Vec::new();
        let summary = untrusted.config_source_summaries(&mut diagnostics);
        let project = summary
            .iter()
            .find(|source| source.id == SourceId::SharedProject)
            .expect("the rung is still listed");
        assert!(project.exists && project.server_count == 1 && !project.contributes);
    }

    // -- MCP-097 -----------------------------------------------------------------------------

    #[test]
    fn discovery_paths_list_the_whole_ladder_without_parsing() {
        let fixture = Fixture::new();
        // A file that would warn loudly if this accessor parsed it.
        fixture.write(&fixture.user_path(), "{{{");
        let paths = fixture.context().config_discovery_paths();

        assert_eq!(paths.len(), 6, "four to six rungs; nothing is deduped in this layout");
        assert_eq!(paths.first().unwrap().label, "user-global standard MCP");
        assert!(paths.iter().filter(|entry| entry.exists).count() == 1);
        assert!(
            paths.iter().any(|entry| entry.path == fixture.project_override()),
            "the `.cyrup` rung is present even with no file"
        );
    }

    // -- MCP-060 -----------------------------------------------------------------------------

    #[test]
    fn presets_carry_their_display_strings() {
        let presets = known_server_presets();
        assert_eq!(presets.len(), 5);
        let ids: Vec<&str> = presets.iter().map(|preset| preset.id).collect();
        assert_eq!(ids, ["deepwiki", "context7", "notion", "github", "chrome-devtools"]);

        let notion = presets.iter().find(|preset| preset.id == "notion").unwrap();
        assert_eq!(notion.name, "Notion");
        assert_eq!(notion.summary, "Search and work with your Notion workspace.");
        assert_eq!(notion.entry.auth, Some(AuthMode::Named(AuthKind::Oauth)));
        assert_eq!(notion.entry.protocol_version, Some(ProtocolVersionSetting::Auto));

        let chrome = presets.last().unwrap();
        assert_eq!(chrome.entry.command.as_deref(), Some("npx"));
        assert_eq!(
            chrome.entry.args.as_deref(),
            Some(["-y".to_string(), "chrome-devtools-mcp@1.6.0".to_string()].as_slice())
        );
    }

    #[test]
    fn project_root_walks_up_for_each_marker() {
        for marker in PROJECT_ROOT_MARKERS {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("root");
            let nested = root.join("a").join("b");
            std::fs::create_dir_all(&nested).unwrap();
            std::fs::create_dir_all(root.join(marker)).unwrap();
            assert_eq!(find_project_root(&nested).as_deref(), Some(root.as_path()), "{marker}");
        }
    }

    #[test]
    fn repo_prompt_reports_a_configured_shared_server() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.shared_global(),
            "{\"mcpServers\":{\"rp\":{\"command\":\"/opt/repoprompt_cli\"}}}",
        );
        let context = fixture.context();
        let mut diagnostics = Vec::new();
        let sources = context.config_source_summaries(&mut diagnostics);
        let discovery = context.detect_repo_prompt(&sources, &mut diagnostics);

        assert!(discovery.configured);
        assert_eq!(discovery.configured_path.as_deref(), Some(fixture.shared_global().as_path()));
        assert!(discovery.entry.is_none(), "no proposal when it is already configured");
    }

    #[test]
    fn repo_prompt_proposes_the_nearest_project_root() {
        let fixture = Fixture::new();
        let binary = fixture.home.join("RepoPrompt").join("repoprompt_cli");
        fixture.write(&binary, "#!/bin/sh\n");
        std::fs::create_dir_all(fixture.cwd.join(".git")).unwrap();

        let context = fixture.context();
        let mut diagnostics = Vec::new();
        let discovery = context.detect_repo_prompt(&[], &mut diagnostics);

        assert!(!discovery.configured);
        assert_eq!(discovery.executable_path.as_deref(), Some(binary.as_path()));
        assert_eq!(
            discovery.target_path.as_deref(),
            Some(fixture.cwd.join(PROJECT_CONFIG_NAME).as_path())
        );
        assert_eq!(discovery.server_name.as_deref(), Some("repoprompt"));
        let entry = discovery.entry.unwrap();
        assert_eq!(entry.lifecycle, Some(ServerLifecycle::Lazy));
        assert_eq!(entry.args.as_deref(), Some([].as_slice()));
    }

    // -- MCP-002 -----------------------------------------------------------------------------

    #[test]
    fn the_config_flag_is_read_from_argv_and_only_in_its_space_separated_form() {
        assert_eq!(
            config_path_from_argv(["cyrup", "--mcp-config", "/tmp/a.json", "chat"]).as_deref(),
            Some("/tmp/a.json")
        );
        assert_eq!(
            config_path_from_argv(["cyrup", "--mcp-config=/tmp/a.json"]),
            None,
            "the `=` form is unsupported upstream and stays unsupported here"
        );
        assert_eq!(config_path_from_argv(["cyrup", "--mcp-config"]), None, "nothing follows it");
        assert_eq!(config_path_from_argv(["cyrup", "chat"]), None);
    }

    #[test]
    fn an_override_path_replaces_the_adapter_owned_rung_and_drops_its_duplicate() {
        let fixture = Fixture::new();
        let override_path = fixture.home.join(".config").join("mcp").join("mcp.json");
        fixture.write(&override_path, "{\"mcpServers\":{\"only\":{\"command\":\"x\"}}}");
        let context =
            ConfigContext::new(McpDirs::new(fixture.agent_dir.clone(), fixture.cwd.clone()), Some(&override_path))
                .with_home(fixture.home.clone());

        assert_eq!(context.user_path(), override_path, "`--mcp-config` IS `userPath`");
        let ids: Vec<SourceId> = context.sources().into_iter().map(|source| source.id).collect();
        assert!(
            !ids.contains(&SourceId::SharedGlobal),
            "the generic global rung is deduped away when it IS the override target"
        );
        assert_eq!(ids.len(), 5);
        assert!(context.load().config.mcp_servers.contains_key("only"));
    }

    // -- MCP-003 / the degradation contract ----------------------------------------------------

    #[test]
    fn a_ladder_of_malformed_files_loads_to_an_empty_config_rather_than_an_error() {
        let fixture = Fixture::new();
        let ladder = [
            fixture.shared_global(),
            fixture.home.join(".agents").join("mcp.json"),
            fixture.home.join(".agents").join("mcp").join("mcp.json"),
            fixture.user_path(),
            fixture.cwd.join(PROJECT_CONFIG_NAME),
            fixture.project_override(),
        ];
        for path in &ladder {
            fixture.write(path, "{{{ not json at all");
        }

        let loaded = fixture.context().load();
        assert!(loaded.config.mcp_servers.is_empty());
        assert!(loaded.config.settings.is_none());
        for path in &ladder {
            assert!(
                loaded.diagnostics.iter().any(|diagnostic| diagnostic.path == *path),
                "every unreadable rung is reported, not swallowed: {}",
                path.display()
            );
        }
    }

    #[test]
    fn one_malformed_entry_does_not_take_the_file_with_it() {
        let mut diagnostics = Vec::new();
        let document = parse_json_config(
            "{\"mcpServers\":{\"good\":{\"command\":\"x\"},\"bad\":\"not-an-object\",\
             \"typed\":{\"command\":\"y\",\"idleTimeout\":\"ten\"}}}",
            "test",
        )
        .unwrap();
        let config = validate_config(&document, Path::new("test"), &mut diagnostics);

        assert!(config.mcp_servers.contains_key("good"));
        assert!(!config.mcp_servers.contains_key("bad"), "a non-object entry is dropped");
        let typed = config.mcp_servers.get("typed").unwrap();
        assert_eq!(typed.command.as_deref(), Some("y"));
        assert!(typed.idle_timeout.is_none(), "a wrong-typed FIELD degrades to None, not to an error");
    }

    #[test]
    fn server_order_is_the_file_order_not_a_sorted_one() {
        let config = parse_config_document(
            "{\"mcpServers\":{\"zulu\":{\"command\":\"z\"},\"alpha\":{\"command\":\"a\"},\
             \"mike\":{\"command\":\"m\"}}}",
            "test",
        )
        .unwrap();
        let names: Vec<&str> = config.mcp_servers.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            ["zulu", "alpha", "mike"],
            "a `serde_json::Value` round-trip would sort these, and connect order, `/mcp` listing \
             order and the collision tie-break all read it"
        );
    }

    // -- MCP-055 / MCP-058 ---------------------------------------------------------------------

    #[test]
    fn per_file_imports_are_first_wins_across_kinds_and_lose_to_the_file_itself() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.home.join(".cursor").join("mcp.json"),
            "{\"mcpServers\":{\"foo\":{\"command\":\"from-cursor\"},\"only-cursor\":{\"command\":\"c\"}}}",
        );
        fixture.write(
            &fixture.home.join(".claude").join("mcp.json"),
            "{\"mcpServers\":{\"foo\":{\"command\":\"from-claude\"}}}",
        );
        fixture.write(
            &fixture.user_path(),
            "{\"imports\":[\"cursor\",\"claude-code\"],\"mcpServers\":{\"own\":{\"command\":\"o\"}}}",
        );

        let config = fixture.context().load().config;
        assert_eq!(
            config.mcp_servers.get("foo").and_then(|entry| entry.command.as_deref()),
            Some("from-cursor"),
            "imports are first-wins in `imports` order, not last-wins"
        );
        assert!(config.mcp_servers.contains_key("only-cursor"));
        assert!(config.mcp_servers.contains_key("own"));
    }

    #[test]
    fn host_config_discovery_is_off_until_a_setting_turns_it_on() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.home.join(".cursor").join("mcp.json"),
            "{\"mcpServers\":{\"shared-name\":{\"command\":\"from-cursor\"},\"host-only\":{\"command\":\"h\"}}}",
        );
        fixture.write(
            &fixture.user_path(),
            "{\"mcpServers\":{\"shared-name\":{\"command\":\"from-owned\"}}}",
        );
        let context = fixture.context();

        let off = context.load().config;
        assert!(!off.mcp_servers.contains_key("host-only"), "the default is `off` — nothing is read");

        fixture.write(
            &fixture.user_path(),
            "{\"settings\":{\"hostConfigDiscovery\":\"on\"},\
             \"mcpServers\":{\"shared-name\":{\"command\":\"from-owned\"}}}",
        );
        let on = context.load().config;
        assert!(on.mcp_servers.contains_key("host-only"), "`on` folds all seven families in");
        assert_eq!(
            on.mcp_servers.get("shared-name").and_then(|entry| entry.command.as_deref()),
            Some("from-owned"),
            "host configs are the BASE layer: an opt-in discovery never outranks an owned definition"
        );

        // `prompt` detects without loading.
        fixture.write(
            &fixture.user_path(),
            "{\"settings\":{\"hostConfigDiscovery\":\"prompt\"},\"mcpServers\":{}}",
        );
        assert!(!context.load().config.mcp_servers.contains_key("host-only"));
        let mut diagnostics = Vec::new();
        let summary = context.mcp_discovery_summary(true, &mut diagnostics);
        assert_eq!(summary.host_config_discovery, HostConfigDiscovery::Prompt);
        let cursor = summary
            .host_configs
            .iter()
            .find(|entry| entry.kind == ImportKind::Cursor)
            .expect("detected");
        assert!(!cursor.active, "detected, listed, and NOT merged");
        assert_eq!(cursor.server_count, 2);
    }

    // --- MCP-144/141: what `lenient` is allowed to discard, and what it is not -----------------

    /// The three fields `lenient` used to silently discard, and which `computeServerHash` hashes
    /// verbatim.
    ///
    /// Upstream's `config.ts` validates none of `auth`, `protocolVersion`, `env` or `headers` at
    /// load — `validateConfig` only checks that an entry is a non-array object — and
    /// `computeServerHash` (`metadata-cache.ts:90`, `:93`, `:103`, `:104`) folds all four into the
    /// identity object as written. A closed enum or a `BTreeMap<String, String>` behind [`lenient`]
    /// therefore threw away exactly the values the digest is *supposed* to be sensitive to, and
    /// `cyrup_ext_subagents::exec::mcp_direct_tools` — which holds them raw, as upstream does — could
    /// never agree with this crate about such a server.
    ///
    /// This is the parse-side half of the fix; the digest half is pinned cross-crate in that
    /// module, and the connect-side half in
    /// [`crate::runtime::version_negotiation`]'s
    /// `wire_tests::an_unknown_revision_throws_upstreams_sentence_at_connect`.
    #[test]
    fn the_deserialiser_keeps_what_upstream_hashes_and_validates_none_of_it() {
        let entry: ServerEntry = serde_json::from_str(
            r#"{
                "command": "x",
                "auth": "basic",
                "protocolVersion": "2025-06-18",
                "env": { "GOOD": "1", "BAD": 5 },
                "headers": { "X-Ok": "y", "X-Bad": null }
            }"#,
        )
        .expect("a wrong-typed field never fails the parse");

        // `auth` and `protocolVersion` survive verbatim…
        assert_eq!(entry.auth, Some(AuthMode::Other(RawJson::String("basic".to_string()))));
        assert_eq!(
            entry.protocol_version,
            Some(ProtocolVersionSetting::Other(RawJson::String("2025-06-18".to_string())))
        );
        // …and neither satisfies any read site, which is what a TypeScript `===` against an unknown
        // value does.
        assert_ne!(entry.auth, Some(AuthMode::Named(AuthKind::Oauth)));
        assert_ne!(entry.auth, Some(AuthMode::Disabled(false)));

        // `env`/`headers` keep their usable members AND the throw, instead of vanishing whole.
        let env = entry.env.as_ref().expect("the map is not dropped");
        assert_eq!(env.get("GOOD").map(String::as_str), Some("1"));
        assert_eq!(env.len(), 1);
        assert_eq!(env.unhashable(), Some("value.startsWith is not a function"));
        let headers = entry.headers.as_ref().expect("the map is not dropped");
        assert_eq!(headers.get("X-Ok").map(String::as_str), Some("y"));
        assert_eq!(
            headers.unhashable(),
            Some("Cannot read properties of null (reading 'startsWith')")
        );

        // And a write-back round-trips the file rather than erasing what it cannot use. The old
        // `Option<BTreeMap<String, String>>` dropped both blocks entirely on every write.
        let raw = raw_from(&entry);
        let written = serialize_raw_object(raw.as_object().expect("a serialised entry is an object"));
        assert!(written.contains(r#""BAD": 5"#), "{written}");
        assert!(written.contains(r#""X-Bad": null"#), "{written}");
        assert!(written.contains(r#""auth": "basic""#), "{written}");
        assert!(written.contains(r#""protocolVersion": "2025-06-18""#), "{written}");
    }

    /// A well-formed record is byte-identical to the `BTreeMap<String, String>` it replaced, and
    /// carries no throw — the property that kept every existing digest and every consumer intact.
    #[test]
    fn a_well_formed_record_is_unchanged_by_the_new_type() {
        let entry: ServerEntry =
            serde_json::from_str(r#"{ "command": "x", "env": { "B": "2", "A": "1" } }"#)
                .expect("entry");
        let env = entry.env.as_ref().expect("present");
        assert_eq!(env.unhashable(), None);
        assert_eq!(
            env.values(),
            &BTreeMap::from([("A".to_string(), "1".to_string()), ("B".to_string(), "2".to_string())])
        );
        // `Deref` is the whole compatibility story: `.get`, `.len`, `.iter` all reach the strings.
        assert_eq!(env.get("A").map(String::as_str), Some("1"));
        assert_eq!(entry.env.as_deref(), Some(env.values()));

        // A non-object still degrades to `None`, exactly as it did before — `lenient`'s rule 4.
        let odd: ServerEntry =
            serde_json::from_str(r#"{ "command": "x", "env": "not-a-map" }"#).expect("entry");
        assert_eq!(odd.env, None);
    }

    // --- MCP-302: the twelve `extractOAuthConfig` guards run at CONFIG LOAD --------------------

    /// Every `ServerEntry` field is `lenient`, so a wrong-typed `oauth` member is silently dropped
    /// by the deserializer. Nine of MCP-302's twelve messages are unreachable unless the raw block
    /// is validated *before* that — this pins the hook that `to_server_entries` now performs.
    #[test]
    fn a_wrong_typed_oauth_block_is_rejected_at_load_with_the_field_named() {
        let fixture = Fixture::new();
        let context = fixture.context();

        fixture.write(
            &fixture.user_path(),
            "{\"mcpServers\":{\"bad\":{\"url\":\"https://x.example/mcp\",\"oauth\":{\"clientId\":42}},\
             \"good\":{\"url\":\"https://y.example/mcp\",\"oauth\":{\"clientId\":\"cid\"}}}}",
        );
        let loaded = context.load();

        // The offending entry is dropped — NOT silently half-honoured as an anonymous client.
        assert!(!loaded.config.mcp_servers.contains_key("bad"));
        // Its neighbour is untouched: the rejection is per-entry, like the `socket` and
        // `httpTransport: "sse"` arms.
        assert!(loaded.config.mcp_servers.contains_key("good"));

        let diagnostic = loaded
            .diagnostics
            .iter()
            .find(|d| d.server.as_deref() == Some("bad"))
            .expect("a named diagnostic");
        assert!(
            diagnostic.message.contains("OAuth clientId must be a string"),
            "{}",
            diagnostic.message
        );
    }

    #[test]
    fn oauth_false_and_a_well_typed_block_both_load_clean() {
        let fixture = Fixture::new();
        let context = fixture.context();
        fixture.write(
            &fixture.user_path(),
            "{\"mcpServers\":{\"off\":{\"url\":\"https://x.example/mcp\",\"oauth\":false},\
             \"on\":{\"url\":\"https://y.example/mcp\",\"oauth\":{\"scope\":\"a b\",\
             \"authorizationParams\":{\"audience\":\"api\"}}}}}",
        );
        let loaded = context.load();
        assert!(loaded.config.mcp_servers.contains_key("off"));
        assert!(loaded.config.mcp_servers.contains_key("on"));
        assert!(
            loaded.diagnostics.iter().all(|d| !d.message.contains("OAuth ")),
            "{:?}",
            loaded.diagnostics
        );
    }

    // -- MCP-053 -----------------------------------------------------------------------------

    /// `requestHeadersCommand` is one of [`URL_BOUND_AUTH_FIELDS`] (v2.26.0 `config.ts:474`), so a
    /// higher-precedence source that supplies only a new `url` must **not** inherit it: the command
    /// signs requests to the endpoint it was configured for, and following the server to a new one
    /// would hand the signature — and whatever `${REQUEST_SECRET}` it interpolates — to a different
    /// host. Upstream's
    /// `"drops an inherited request headers command when a url-only override repoints the server"`
    /// (`__tests__/config.test.ts:903-921`).
    #[test]
    fn a_url_only_override_drops_an_inherited_request_headers_command() {
        let base: ServerEntry = serde_json::from_str(
            r#"{ "url": "https://a.example/mcp", "bearerToken": "s3cret",
                 "requestHeadersCommand": { "command": "sign-old-url",
                                            "args": ["${REQUEST_SECRET}"] } }"#,
        )
        .unwrap();

        // Repointed: everything URL-bound goes, the entry is the override alone.
        let repointed: ServerEntry =
            serde_json::from_str(r#"{ "url": "https://b.example/mcp" }"#).unwrap();
        let merged = merge_entry(Some(&base), &repointed);
        assert_eq!(merged.request_headers_command, None);
        assert_eq!(merged.bearer_token, None);
        assert_eq!(merged.url.as_deref(), Some("https://b.example/mcp"));
        let serialized = serde_json::to_string(&merged).unwrap();
        assert!(!serialized.contains("REQUEST_SECRET"), "{serialized}");

        // The SAME url is not a repoint: the command is inherited exactly as `headers` is.
        let same: ServerEntry =
            serde_json::from_str(r#"{ "url": "https://a.example/mcp", "debug": true }"#).unwrap();
        let kept = merge_entry(Some(&base), &same);
        assert_eq!(
            kept.request_headers_command
                .as_ref()
                .and_then(|command| command.command.as_deref()),
            Some("sign-old-url")
        );

        // Rule 4: a wrong-typed block degrades to `None` rather than taking the entry — and the
        // rest of the entry survives, so the server still loads (unsigned, and it will fail closed
        // at connect rather than at parse).
        let malformed: ServerEntry = serde_json::from_str(
            r#"{ "url": "https://c.example/mcp", "requestHeadersCommand": "sign-me" }"#,
        )
        .unwrap();
        assert_eq!(malformed.request_headers_command, None);
        assert_eq!(malformed.url.as_deref(), Some("https://c.example/mcp"));

        // An override that re-supplies the command wins, because `definition` spreads last.
        let resupplied: ServerEntry = serde_json::from_str(
            r#"{ "url": "https://b.example/mcp",
                 "requestHeadersCommand": { "command": "sign-new-url" } }"#,
        )
        .unwrap();
        assert_eq!(
            merge_entry(Some(&base), &resupplied)
                .request_headers_command
                .as_ref()
                .and_then(|command| command.command.as_deref()),
            Some("sign-new-url")
        );
    }

    /// The constant and the fields [`merge_entry`] actually clears must not drift apart — the whole
    /// reason `URL_BOUND_AUTH_FIELDS` exists as data.
    #[test]
    fn url_bound_auth_fields_matches_what_the_merge_clears() {
        let base: ServerEntry = serde_json::from_str(
            r#"{ "url": "https://a.example/mcp", "headers": { "X-Key": "k" },
                 "bearerToken": "t", "bearerTokenEnv": "TOK",
                 "requestHeadersCommand": { "command": "sign" } }"#,
        )
        .unwrap();
        let repointed: ServerEntry =
            serde_json::from_str(r#"{ "url": "https://b.example/mcp" }"#).unwrap();
        let merged = merge_entry(Some(&base), &repointed);

        // Serialising the merged entry names exactly the fields that survived, so a field added to
        // the constant but forgotten in `merge_entry` shows up here as a leftover key.
        let serialized = serde_json::to_string(&merged).unwrap();
        for field in URL_BOUND_AUTH_FIELDS {
            assert!(!serialized.contains(field), "{field} survived a url repoint: {serialized}");
        }
        assert_eq!(URL_BOUND_AUTH_FIELDS.len(), 4);
    }
}
