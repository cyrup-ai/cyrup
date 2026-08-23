---
stage: new
status: done
updated: 2026-08-22 15:58
---

# Process Launch: npx Pre-Resolution, Env Interpolation, Browser Open

## Description

Seven units, one question: **what binary does this port actually exec, and with what environment?**

The question is asked at three places and answered inconsistently at all three today:

1. **the npx resolver** — `cyrup_ext::caps::proc::npx_resolver` is a direct port of
   `npx-resolver.ts` that rewrites an `npx`/`npm exec` invocation down to the real MCP server
   binary. It is complete enough to be trusted and **unreachable from `cyrup-mcp`**
   (`mod npx_resolver;` is private at [caps/proc.rs:25](../../crates/cyrup-ext/src/caps/proc.rs),
   `resolve_npx_binary` is `pub(super)` at
   [npx_resolver.rs:114](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)), and it carries four
   divergences from v2.25.0 in its cache and its cancellation (MCP-104, MCP-106, MCP-107, MCP-108);
2. **the stdio connection builder** — `ConnectionBuilder::connect_stdio` has the call site marked
   and empty at [runtime.rs:2397-2408](../../crates/cyrup-mcp/src/runtime.rs) (MCP-103);
3. **the browser opener** — `state.open_browser` is a closure that returns `Ok(())` without opening
   anything ([runtime.rs:236](../../crates/cyrup-mcp/src/runtime.rs)), and the string `BROWSER`
   appears nowhere in the workspace outside prose (MCP-086).

Plus the one interpolation function every spawn path is supposed to share (MCP-342).

**These are not seven file-disjoint edits and must not be split by file.**

* MCP-103's fix is a *visibility promotion in `crates/cyrup-ext`* consumed at a *marked insertion
  point in `crates/cyrup-mcp`*. A file-based split puts the resolver and its only production caller
  in different agents' change sets — the PR #30 failure mode.
* MCP-104, MCP-106, MCP-107 and MCP-108 all edit
  [npx_resolver.rs](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs) and conflict textually
  with MCP-103: MCP-107 changes the *signature and return type* of the very function MCP-103
  promotes, and MCP-104/MCP-108 rewrite the cache-loading path MCP-106 keys into.
* MCP-342's consolidation target is `interpolate_env_vars_with`
  ([caps/proc.rs:148](../../crates/cyrup-ext/src/caps/proc.rs)), whose only caller is the env applied
  at [caps/proc.rs:526](../../crates/cyrup-ext/src/caps/proc.rs) to the same `ProcCaps::spawn` whose
  npx branch MCP-103/107 rewrites, twenty lines above.
* MCP-086 is the same spawn discipline for the browser arm: the platform dispatch, `$BROWSER`, and a
  cancel token, in a crate that has already decided (correctly) to spawn its own children.

**Land this before the MCP-131 process-group wave, not after.** `StdioChildConnection`'s own doc
says so at [server_manager.rs:560-567](../../crates/cyrup-mcp/src/server_manager.rs): "Both signal a
single pid, not a process group. The plan argues that is sufficient *because* npx pre-resolution
(MCP-103) removes the `npm` launcher… MCP-103 is **not ported**." Without MCP-103 the tracked child
of an `npx` server is the npm launcher and a single-pid kill orphans the real server.

Upstream source for every citation below is the checkout at `tmp/pi-mcp-adapter`
(`npx-resolver.ts`, `server-manager.ts`, `utils.ts`, `init.ts`).

---

## Ledger corrections — read before touching anything

[13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) is dated 2026-08-21 and says
of itself that it is not rewritten by later work. Five of the rows in this group are wrong or
incomplete as written. Each correction below was checked against the tree today.

**1 — MCP-342's row is stale, and its own instruction would make things worse.** The row
([:861](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) says "a THIRD implementation was added
instead of one shared implementation, and **the two pre-existing copies still carry the two-form
parity defect**." That is no longer true. There are three copies and **two of the three are already
correct three-form implementations**:

| copy | forms | evidence |
|---|---|---|
| `cyrup_mcp::credentials::interpolate_env_vars_with` | **three** | [credentials.rs:3323-3345](../../crates/cyrup-mcp/src/credentials.rs) — `\$\{(…)\}`, `\$env:(…)`, `\{env:(…)\}` as three chained `replace_all` passes |
| `cyrup_ext_subagents::exec::mcp_direct_tools::interpolate_env_vars` | **three** | [mcp_direct_tools.rs:1300-1304](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs) — MCP-143 landed; the module header records it at [:45-46](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs) |
| `cyrup_ext::caps::proc::interpolate_env_vars_with` | **two** | [caps/proc.rs:143-150](../../crates/cyrup-ext/src/caps/proc.rs) — `interpolate_dollar_env(&interpolate_braces(value, lookup), lookup)`, no `{env:VAR}` pass |

So the remaining parity defect is in exactly **one** copy, and the consolidation is still owed.

**2 — MCP-104's `std::sync::Once` is unnecessary and must not be written.** The row asks for
`clear_legacy_cache` "invoked once at module load via `std::sync::Once` inside `load_cache` **and**
on every `load_cache()`". Upstream's module-load call (`npx-resolver.ts:501`) exists only because ES
modules have a load hook; Rust has none, and `load_cache` is the *only* reader of that file
([npx_resolver.rs:701-713](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)). A head-of-
`load_cache` call fully subsumes it. A `Once` would additionally be *wrong*: it would suppress the
eviction for every call after the first, which is precisely the arm upstream runs unconditionally.

**3 — MCP-104's eviction predicate is `version == 1`, not "version mismatch".** Upstream
`clearLegacyCache` is `if (raw?.version !== 1) return false;` — it evicts a v1 file and **leaves a
v3 or a garbage-version file alone** (`toNpxCache`'s `raw.version !== CACHE_VERSION` check then
rejects it without deleting). A port that deletes on any mismatch would let a future version's file
be destroyed by an older binary.

**4 — MCP-107's row understates the change: the return type must become fallible.** The row asks for
"a `cancel` parameter with `throw_if_aborted`-equivalent checks". But upstream `resolveNpxBinary`
**throws** on abort (`throwIfAborted(signal)` at `npx-resolver.ts:46`, and `forceNpxCache` rejects
with `signal.reason ?? new Error("MCP request aborted")` at `:266`); it does not return `null`. The
distinction is load-bearing at the connect site: `None` means "not an npx invocation, run
`command`/`args` verbatim", so a cancel folded into `None` would make an aborted connect **spawn
`npx` during teardown** — the opposite of the unit's purpose. `resolve_npx_binary` must return
`Result<Option<NpxResolution>, NpxAborted>`.

**5 — MCP-086's row names the wrong gap, and 13b's prescribed mechanism is wrong for this crate.**
The row says "`openUrl`/`execOpen`'s browser arm is missing". The sharper and more damaging fact is
that the consumer **already exists and silently lies**: `state.open_browser` is built at
[runtime.rs:229-240](../../crates/cyrup-mcp/src/runtime.rs) with a body of
`owner.throw_if_inactive()?; let result = Ok(()); owner.throw_if_inactive()?; result` — the `url`
parameter is bound as `_url` and discarded. Separately,
[13b-mcp-config.md:1477-1479](../../docs/gap-analysis/13b-mcp-config.md) prescribes
`HostServices::exec` as "the faithful landing spot". **Do not use it.**
[13g-mcp-oauth.md:1683-1686](../../docs/gap-analysis/13g-mcp-oauth.md) already refuted that on the
grounds that `HostServices::exec` is the WASM-guest capability model and `cyrup-mcp` is a native
built-in crate, and the tree has settled the other way: the sibling
[`open_path`](../../crates/cyrup-mcp/src/ui.rs) at `ui.rs:3070` spawns
`tokio::process::Command` directly. The cancel token that 13b wanted `HostServices::exec` for comes
from the caller (`owner.token()`), not from the host trait.

**Context, not work: MCP-105 has landed.** The ledger lists it `missing`
([:615](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) and
[MCP_HIGH_SEVERITY_BACKLOG.md:154](MCP_HIGH_SEVERITY_BACKLOG.md) correctly takes it off the list.
`EXACT_PACKAGE_VERSION` is at
[npx_resolver.rs:62-71](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs), `parse_package_spec`
at `:499`, `cache_entry_is_usable`'s version arm at `:461-476`, and `find_cached_package_dir`'s
`exact_version` filter at `:566-604`. **Do not re-derive any of it** — MCP-106 edits the function
directly above `parse_package_spec`'s call and must leave that call in place.

---

## Per-unit breakdown

### MCP-103 — wire npx/npm resolution into the connection builder · medium · `extension-owned`

Spec: [13c-mcp-servers.md:1161-1175](../../docs/gap-analysis/13c-mcp-servers.md).

**Unmet obligation.** `resolve_npx_binary` has no caller in `cyrup-mcp`. The site is marked
`// Step 3 — MCP-103, NOT PORTED` at
[runtime.rs:2397-2408](../../crates/cyrup-mcp/src/runtime.rs), between step 2's arg interpolation
(`:2389-2395`) and step 4's `throw_if_aborted` (`:2410`) — which is exactly upstream's order
(`server-manager.ts:478-485`). Visibility blocks it: `mod npx_resolver;` at
[caps/proc.rs:25](../../crates/cyrup-ext/src/caps/proc.rs), `pub(super) fn resolve_npx_binary` at
[npx_resolver.rs:114](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs), `pub(super) struct
NpxResolution` with `pub(super)` fields at `:76-80`.

**Reachability is not a blocker.** `pub mod caps` is `#[cfg(feature = "wasm-host")]`
([lib.rs:147-148](../../crates/cyrup-ext/src/lib.rs)) and that feature is default-on
([Cargo.toml `default = ["wasm-host"]`](../../crates/cyrup-ext/Cargo.toml)). `cyrup-mcp` already
depends on it unconditionally and already names `wasm-host`-gated items unqualified — its
`Cargo.toml` says so in the comment above `cyrup-ext = { workspace = true }`
([Cargo.toml:17-19](../../crates/cyrup-mcp/Cargo.toml)).

### MCP-104 — `CACHE_VERSION = 2` and `clearLegacyCache` · medium · `hand-written`

Spec: [13c-mcp-servers.md:1176-1186](../../docs/gap-analysis/13c-mcp-servers.md).

**Unmet obligation.** `const CACHE_VERSION: u32 = 1;` at
[npx_resolver.rs:38](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs) against upstream's
`const CACHE_VERSION = 2` (`npx-resolver.ts:8`). `load_cache_at` at `:707-713` rejects a version
mismatch and never deletes; `grep clear_legacy` over the crate returns nothing. Consequence: a v1
file this port writes is deleted on sight by any co-installed pi adapter and vice versa, forever.

### MCP-106 — cache key must be `[command, packageSpec, binName]` · low · `hand-written`

Spec: [13c-mcp-servers.md:1208-1217](../../docs/gap-analysis/13c-mcp-servers.md).

**Unmet obligation.** `let cache_key = cache_key(command, args);` at
[npx_resolver.rs:124](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs), and `cache_key` at
`:767-772` serialises `[command, ...args]` — the whole argv. Upstream is
`JSON.stringify([command, parsed.packageSpec, parsed.binName ?? ""])` (`npx-resolver.ts:56`),
computed after the parse. Note it keys on `parsed.packageSpec` — the **raw spec string**
(`"pkg@1.2.3"`), not `parse_package_spec`'s extracted name. Consequence: `npx -y srv --port 3000`
and `npx -y srv --port 3001` occupy different slots and each pays its own cold `_npx` scan;
`npx pkg bin` and `npx --package pkg bin` never share.

### MCP-107 — no cancellation path · medium · `hand-written`

Spec: [13c-mcp-servers.md:1218-1230](../../docs/gap-analysis/13c-mcp-servers.md), corrected by
finding 4 above.

**Unmet obligation.** `pub(super) fn resolve_npx_binary(command: &str, args: &[String])` at
[npx_resolver.rs:114](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs) takes no signal, and
`force_npx_cache` at `:395-419` is a 50 ms `std::thread::sleep` poll loop bounded only by
`FORCE_CACHE_TIMEOUT` (30 s, `:42`). Nothing can interrupt it. Once MCP-103 lands, that loop runs
inside the manager's single-flight connect future's blocking task, so `close`/`close_all` cannot
preempt an attempt for up to 30 s — the exact guarantee `connect_stdio`'s own comment at
[runtime.rs:2420-2445](../../crates/cyrup-mcp/src/runtime.rs) says it moved
`StdioTransportSpec::resolve` off the async worker to protect.

### MCP-108 — per-entry cache validation and Windows `npm` · low · `hand-written`

Spec: [13c-mcp-servers.md:1231-1240](../../docs/gap-analysis/13c-mcp-servers.md).

**Unmet obligation, half one.** `struct NpxCache { version: u32, entries: HashMap<String,
NpxCacheEntry> }` at [npx_resolver.rs:96-100](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)
is deserialised whole by `serde_json::from_str::<NpxCache>` at `:709`, so one malformed entry
discards every cached resolution. Upstream's `toNpxCacheEntry` (`npx-resolver.ts:456-468`) validates
per entry and drops only the bad ones, checking `typeof resolvedBin === "string"`,
`typeof resolvedAt === "number" && Number.isFinite(resolvedAt)`, `typeof isJs === "boolean"`, and
`packageVersion === undefined || typeof packageVersion === "string"`.

**Half two.** `Command::new("npm")` at `:396` (`force_npx_cache`) and `:659` (`get_npm_cache_dir`)
against upstream's `crossSpawn` / `crossSpawn.sync` (`npx-resolver.ts:255`, `:419`). On Windows
`npm` is `npm.cmd` and `CreateProcess` will not find it, so npx resolution is a silent no-op there.
The in-tree idiom for this is already established:
[secrets.rs:197-200](../../crates/cyrup-mcp/src/secrets.rs) uses
`#[cfg(windows)] let (shell, flag) = ("cmd", "/C");` with `CREATE_NO_WINDOW` at `:140`/`:213`.

### MCP-342 — a reachable, three-form `interpolate_env_vars` · medium · `hand-written`

Spec: [13g-mcp-oauth.md:1401-1416](../../docs/gap-analysis/13g-mcp-oauth.md), the three forms
tabulated at [13g-mcp-oauth.md:344-348](../../docs/gap-analysis/13g-mcp-oauth.md). Row corrected by
finding 1 above.

**Unmet obligation.** `interpolate_env_vars_with` at
[caps/proc.rs:143-150](../../crates/cyrup-ext/src/caps/proc.rs) runs two passes;
`interpolate_braces` (`:156-176`) and `interpolate_dollar_env` (`:178-200`) are its halves and there
is no `{env:VAR}` third. Its one production caller is
`cmd.env(k, interpolate_env_vars(v))` at [caps/proc.rs:526](../../crates/cyrup-ext/src/caps/proc.rs)
— every environment variable of every WASM-guest-spawned child, including the npx-resolved one this
task rewrites. A guest config carrying `{env:GITHUB_TOKEN}` hands the child an 18-character literal.

**The consolidation direction is forced by the dependency graph.** `cyrup-mcp` and
`cyrup-ext-subagents` both depend on `cyrup-ext`
([cyrup-mcp/Cargo.toml:19](../../crates/cyrup-mcp/Cargo.toml),
[cyrup-ext-subagents/Cargo.toml](../../crates/cyrup-ext-subagents/Cargo.toml)) and `cyrup-ext`
depends on neither. `cyrup_ext::caps::proc` is the only module all three can share, which is what
13g means by "promoting the existing one to `pub`".

### MCP-086 — port the browser/path open dispatch · medium · `extension-owned`

Spec: [13b-mcp-config.md:1468-1482](../../docs/gap-analysis/13b-mcp-config.md), platform table at
[13b-mcp-config.md:563-575](../../docs/gap-analysis/13b-mcp-config.md) and again at
[13a-mcp-activation.md:352-364](../../docs/gap-analysis/13a-mcp-activation.md). Row and mechanism
corrected by finding 5 above.

**Unmet obligation.** Three things:

* `execOpen`'s **browser arm** does not exist. `open_path` at
  [ui.rs:3070-3097](../../crates/cyrup-mcp/src/ui.rs) hard-codes the no-`browser` column of the
  table and takes no cancel; its own doc at `:3067` says the `$BROWSER` override "is shared with
  `openUrl`" and defers it.
* `$BROWSER` is never read. Grep for `BROWSER` over `crates/cyrup-mcp/src` returns three hits, all
  prose: [state.rs:55](../../crates/cyrup-mcp/src/state.rs),
  [ui.rs:3067](../../crates/cyrup-mcp/src/ui.rs), [oauth.rs:2392](../../crates/cyrup-mcp/src/oauth.rs).
* `state.open_browser`'s production body is a stub that discards its URL —
  [runtime.rs:229-240](../../crates/cyrup-mcp/src/runtime.rs). Upstream's is
  `owner.throwIfInactive(); await openUrl(pi, url, process.env.BROWSER, owner.signal);
  owner.throwIfInactive();` (`init.ts:175-179`). The two guards are already there; the middle line
  is not.

**Not in scope, and do not "unify" them.** [`OpenerLauncher`](../../crates/cyrup-mcp/src/oauth.rs) at
`oauth.rs:2394-2400` calls `opener::open` and is *correct*: it ports the direct npm `open` import in
`mcp-auth-flow.ts`, a genuinely different upstream mechanism (MCP-338, settled —
[13b-mcp-config.md:578-581](../../docs/gap-analysis/13b-mcp-config.md)). Both mechanisms exist
upstream and both stay.

---

## Implementation

Order matters: 104 + 108 restructure the cache-loading path, 106 keys into it, 107 changes the
signature, 103 writes the caller against the final signature, then 342 and 086 are independent.

### Step 1 — `npx_resolver.rs`: the cache path (MCP-104 + MCP-108 half one)

Replace `load_cache_at` ([:707-713](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)) with
upstream's four-function split. Keep `NpxCacheEntry`'s `serde` derives — they are still the shape
`save_cache_entry_at` serialises — but stop using them to parse the whole file at once.

```rust
/// `npx-resolver.ts:8` `CACHE_VERSION` (MCP-104).
const CACHE_VERSION: u32 = 2;

/// The version `clear_legacy_cache_at` evicts. Upstream's predicate is `raw?.version !== 1`
/// (`npx-resolver.ts:488`) — it deletes a v1 file and deliberately leaves a *newer*-than-current
/// or garbage-version file alone, which `to_npx_cache`'s own check then rejects without deleting.
/// Evicting on any mismatch would let an older binary destroy a newer one's cache.
const LEGACY_CACHE_VERSION: u64 = 1;

/// `npx-resolver.ts:437-444` `readNpxCachePayload`.
fn read_npx_cache_payload(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// `npx-resolver.ts:456-468` `toNpxCacheEntry` (MCP-108) — per-entry validation, so one corrupt
/// entry drops itself instead of the whole file.
fn to_npx_cache_entry(value: &serde_json::Value) -> Option<NpxCacheEntry> {
    let raw = value.as_object()?;
    let resolved_bin = raw.get("resolvedBin")?.as_str()?.to_string();
    // `typeof raw.resolvedAt !== "number" || !Number.isFinite(raw.resolvedAt)`. `as_f64` is the
    // `typeof === "number"` half (it rejects a string and a bool); `is_finite` is the second, and
    // it is not redundant — `serde_json` parses `1e400` to an f64 `inf`.
    let resolved_at = raw.get("resolvedAt")?.as_f64()?;
    if !resolved_at.is_finite() || resolved_at < 0.0 {
        return None;
    }
    let is_js = raw.get("isJs")?.as_bool()?;
    // `raw.packageVersion !== undefined && typeof raw.packageVersion !== "string"` — absent is
    // fine, present-and-not-a-string drops the entry. A JSON `null` is `undefined`'s nearest
    // neighbour here and upstream's `!== undefined` would keep it; `as_str()` on `Null` is `None`,
    // so treat `Null` as absent explicitly rather than as a type failure.
    let package_version = match raw.get("packageVersion") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(value.as_str()?.to_string()),
    };
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Some(NpxCacheEntry { resolved_bin, resolved_at: resolved_at as u64, package_version, is_js })
}

/// `npx-resolver.ts:471-482` `toNpxCache` (MCP-108).
fn to_npx_cache(value: &serde_json::Value) -> Option<NpxCache> {
    let raw = value.as_object()?;
    if raw.get("version").and_then(serde_json::Value::as_u64) != Some(u64::from(CACHE_VERSION)) {
        return None;
    }
    let entries = raw
        .get("entries")?
        .as_object()?
        .iter()
        .filter_map(|(key, raw_entry)| Some((key.clone(), to_npx_cache_entry(raw_entry)?)))
        .collect();
    Some(NpxCache { version: CACHE_VERSION, entries })
}

/// `npx-resolver.ts:485-499` `clearLegacyCache` (MCP-104). Returns whether it evicted.
///
/// Upstream also calls this once at module load (`:501`). There is no Rust module-load hook, and
/// none is needed: `load_cache_at` below is the only reader of this file, and it calls this on
/// every load — which is upstream's *other*, unconditional call site. A `std::sync::Once` here
/// would be strictly worse than nothing, suppressing every eviction after the first.
fn clear_legacy_cache_at(path: &Path) -> bool {
    let Some(payload) = read_npx_cache_payload(path) else { return false };
    if payload.get("version").and_then(serde_json::Value::as_u64) != Some(LEGACY_CACHE_VERSION) {
        return false;
    }
    if fs::remove_file(path).is_err() {
        // `catch { writeFileSync(cachePath, "") }` — a read-only directory can refuse the unlink
        // but still allow a truncate. Both failing is upstream's silent third arm.
        let _ = fs::write(path, "");
    }
    true
}

/// `npx-resolver.ts:503-507` `loadCache`.
fn load_cache_at(path: &Path) -> Option<NpxCache> {
    if clear_legacy_cache_at(path) {
        return None;
    }
    to_npx_cache(&read_npx_cache_payload(path)?)
}
```

Then change `save_cache_entry_at`'s merge read
([:756](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)) from `load_cache_at(path)` to
`read_npx_cache_payload(path).as_ref().and_then(to_npx_cache)`. This is not cosmetic: upstream's
`saveCacheEntry` calls `toNpxCache(readNpxCachePayload(cachePath))` (`npx-resolver.ts:515`), **not**
`loadCache`, so a save must not run the eviction. Leave the `SAVE_CACHE_LOCK` and the
tmp-file-rename cycle exactly as they are.

### Step 2 — `npx_resolver.rs`: the cache key (MCP-106)

```rust
/// `npx-resolver.ts:56` — `JSON.stringify([command, parsed.packageSpec, parsed.binName ?? ""])`,
/// computed AFTER the parse (MCP-106). `parsed.packageSpec` is the raw spec (`"pkg@1.2.3"`), not
/// `parse_package_spec`'s extracted name: two invocations of the same package/bin that differ only
/// in trailing arguments must share one entry.
fn cache_key(command: &str, package_spec: &str, bin_name: &str) -> String {
    serde_json::to_string(&[command, package_spec, bin_name]).unwrap_or_default()
}
```

and at the call site (currently
[:124](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)):

```rust
let cache_key = cache_key(
    command,
    &parsed.package_spec,
    parsed.bin_name.as_deref().unwrap_or(""),
);
```

Leave `let package_spec = parse_package_spec(&parsed.package_spec);` on the line above untouched —
that is MCP-105's, already landed, and `cache_entry_is_usable`'s third argument still needs it.

### Step 3 — `npx_resolver.rs`: cancellation (MCP-107)

```rust
use cyrup_core::CancelToken;

/// `throwIfAborted(signal)`'s rejection, as a value (`npx-resolver.ts:46`, `:266`, `:285`).
///
/// Deliberately NOT folded into the `None` return. `None` means "this is not an npx-shaped
/// invocation, run `command`/`args` verbatim", and `server-manager.ts:480`'s `if (resolved)` acts
/// on it by spawning the original command — so a cancel reported as `None` would spawn `npx`
/// during teardown, which is the exact orphan this module exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NpxAborted;

pub fn resolve_npx_binary(
    command: &str,
    args: &[String],
    cancel: &CancelToken,
) -> Result<Option<NpxResolution>, NpxAborted> {
    // `throwIfAborted(signal)` — before the parse, `npx-resolver.ts:46`.
    if cancel.is_cancelled() {
        return Err(NpxAborted);
    }
    let parsed = match command {
        "npx" => parse_npx_args(args),
        "npm" => parse_npm_exec_args(args),
        _ => return Ok(None),
    };
    let Some(parsed) = parsed else { return Ok(None) };
    // ... unchanged through the cached-hit and warm `resolve_from_npm_cache` arms, each `return`
    // becoming `Ok(Some(..))` and the trailing `?` on `resolve_from_npm_cache` becoming an
    // explicit `else { return Ok(None) }`.

    force_npx_cache(&parsed.package_spec, cancel)?;
    let Some(resolved_after_install) =
        resolve_from_npm_cache(&parsed.package_spec, parsed.bin_name.as_deref())
    else {
        return Ok(None);
    };
    save_cache_entry(&cache_key, &resolved_after_install);
    Ok(Some(NpxResolution {
        bin_path: resolved_after_install.resolved_bin,
        extra_args: parsed.extra_args,
        is_js: resolved_after_install.is_js,
    }))
}
```

`force_npx_cache` grows the same token. Its three upstream abort points are the entry
`throwIfAborted` (`:252`), the `abort` listener that kills the child (`:264-267`), and the trailing
`throwIfAborted` after the swallow-everything catch (`:285`):

```rust
fn force_npx_cache(package_spec: &str, cancel: &CancelToken) -> Result<(), NpxAborted> {
    if cancel.is_cancelled() {
        return Err(NpxAborted);
    }
    let spawned = npm_command()
        .args(["exec", "--yes", "--package", package_spec, "--", "node", "-e", "1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        // `catch { /* Ignore failures */ }` then the trailing `throwIfAborted(signal)`.
        return if cancel.is_cancelled() { Err(NpxAborted) } else { Ok(()) };
    };

    let deadline = Instant::now() + FORCE_CACHE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                // The `abort` listener's `proc.kill()` (`npx-resolver.ts:265`), on this port's
                // 50 ms tick instead of Node's event loop. Reap, so the poll never leaves a
                // zombie behind on a teardown.
                if cancel.is_cancelled() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(NpxAborted);
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }
    // `npx-resolver.ts:285` — the re-check on exit.
    if cancel.is_cancelled() { Err(NpxAborted) } else { Ok(()) }
}
```

### Step 4 — `npx_resolver.rs`: Windows `npm` (MCP-108 half two)

One helper, used at both `Command::new("npm")` sites
([:396](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs) and `:659`). Take the `cmd /C` arm the
unit offers, not the PATH+PATHEXT walk: it needs no new dependency and it is the shape
[secrets.rs:197-214](../../crates/cyrup-mcp/src/secrets.rs) already uses in this workspace for the
identical problem.

```rust
/// `crossSpawn("npm", …)` / `crossSpawn.sync("npm", …)` (`npx-resolver.ts:255`, `:419`) — MCP-108.
///
/// On Windows `npm` is `npm.cmd`, a batch file `CreateProcess` will not resolve, so every npx
/// resolution silently no-ops there. Routed through `cmd /C`, the same arm
/// `cyrup_mcp::secrets::resolve_command_secret` takes for `!command` secrets, with the same
/// console suppression.
fn npm_command() -> Command {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        use std::os::windows::process::CommandExt as _;
        let mut command = Command::new("cmd");
        command.arg("/C").arg("npm").creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    Command::new("npm")
}
```

### Step 5 — `caps/proc.rs`: the promotion and the shared rewrite (MCP-103, `cyrup-ext` half)

In [npx_resolver.rs](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs), make `NpxResolution` and
its three fields `pub` (`:76-80`), and give it the rewrite as an inherent method so the two crates
cannot drift:

```rust
impl NpxResolution {
    /// `server-manager.ts:481-482` — `command = resolved.isJs ? "node" : resolved.binPath;
    /// args = resolved.isJs ? [resolved.binPath, ...resolved.extraArgs] : resolved.extraArgs;`
    ///
    /// The `resolved === null` arm is the caller's: upstream's `if (resolved)` simply never
    /// reassigns, and what "the original" is differs between the two call sites.
    #[must_use]
    pub fn rewrite(self) -> (String, Vec<String>) {
        if self.is_js {
            let mut args = vec![self.bin_path];
            args.extend(self.extra_args);
            ("node".to_string(), args)
        } else {
            (self.bin_path, self.extra_args)
        }
    }
}
```

In [caps/proc.rs](../../crates/cyrup-ext/src/caps/proc.rs): `mod npx_resolver;` (`:25`) becomes
`pub mod npx_resolver;` plus
`pub use npx_resolver::{resolve_npx_binary, NpxAborted, NpxResolution};`. Reduce
`apply_npx_resolution` (`:430-443`) to a delegation so its existing test
`apply_npx_resolution_matches_pi_exactly` (`:947`) keeps passing against one implementation:

```rust
fn apply_npx_resolution(
    resolved: Option<npx_resolver::NpxResolution>,
    spec: &ProcSpawnSpec,
) -> (String, Vec<String>) {
    resolved.map_or_else(
        || (spec.cmd.clone(), spec.args.clone()),
        npx_resolver::NpxResolution::rewrite,
    )
}
```

And at `ProcCaps::spawn`'s call site ([:508-512](../../crates/cyrup-ext/src/caps/proc.rs)):

```rust
// The WIT `proc.spawn` handler has no signal to thread: a `CancelToken` is not a WIT value, and
// the guest-side poll (`HostServices::is_run_cancelled`) is not overridden by `LiveHostServices`
// (see `cyrup_mcp::abort`'s module doc). A never-cancelled token keeps this path's behaviour
// exactly as it is today; the interrupt belongs to the `cyrup-mcp` caller, which has a real one.
let never_cancelled = cyrup_core::CancelToken::new();
let resolved = if spec.cmd == "npx" || spec.cmd == "npm" {
    tokio::task::block_in_place(|| {
        npx_resolver::resolve_npx_binary(&spec.cmd, &spec.args, &never_cancelled)
    })
    .unwrap_or(None)
} else {
    None
};
```

### Step 6 — `runtime.rs`: the connection builder call (MCP-103, `cyrup-mcp` half)

Replace the marker comment at
[runtime.rs:2397-2408](../../crates/cyrup-mcp/src/runtime.rs). `command` and `args` (`:2389-2395`)
become `let mut`.

```rust
// Step 3 — MCP-103. `server-manager.ts:478-485`.
//
// Run on a blocking task for the same reason `StdioTransportSpec::resolve` is, twenty lines
// below: `resolve_npx_binary` is `std::process::Command` + `std::thread::sleep`, bounded by
// `FORCE_CACHE_TIMEOUT` (30 s), and this body is polled inside the manager's single-flight
// connect future. Inline it would hold a tokio worker for the whole cold-cache budget. The
// attempt token goes in with it, so `close`/`close_all` interrupts it at the next 50 ms tick
// instead of at the 30 s ceiling.
if command == "npx" || command == "npm" {
    let resolve_command = command.clone();
    let resolve_args = args.clone();
    let attempt = request.attempt.clone();
    let resolved = match tokio::task::spawn_blocking(move || {
        cyrup_ext::caps::proc::resolve_npx_binary(&resolve_command, &resolve_args, &attempt)
    })
    .await
    {
        // `throwIfAborted` rejects; it does not resolve to `null`. Surfacing this as
        // `McpError::Aborted` is what keeps `server_manager`'s failure backoff from treating a
        // user teardown as a connection failure (`crate::abort::is_abort_error`).
        Ok(Err(cyrup_ext::caps::proc::NpxAborted)) => {
            return Err(McpError::Aborted(crate::abort::ABORTED_FALLBACK_REASON.to_string()));
        }
        Ok(Ok(resolved)) => resolved,
        Err(_join) => None,
    };
    // `if (resolved) { ... }` — a `None` leaves `command`/`args` verbatim.
    if let Some(resolved) = resolved {
        let bin_path = resolved.bin_path.clone();
        (command, args) = resolved.rewrite();
        tracing::debug!("{name} resolved to {bin_path} (skipping npm parent)");
    }
}
```

The debug string is upstream's verbatim (`server-manager.ts:483`). `McpError::Aborted` and
`ABORTED_FALLBACK_REASON` are [abort.rs:94-103](../../crates/cyrup-mcp/src/abort.rs); the fallback
text is `"MCP request aborted"`, which is also `forceNpxCache`'s own rejection text when
`signal.reason` is not an `Error` (`npx-resolver.ts:266`), so the two agree.

### Step 7 — one `interpolate_env_vars` (MCP-342)

Replace `interpolate_braces` and `interpolate_dollar_env`
([caps/proc.rs:156-200](../../crates/cyrup-ext/src/caps/proc.rs)) with the generalised scanner
`cyrup-ext-subagents` already proved at
[mcp_direct_tools.rs:1306-1349](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs),
because it is the only one of the three that is parameterised by the delimiter pair and therefore
the only one that can serve all three forms:

```rust
/// `interpolateEnvVars(value)` (`utils.ts:74-79`) — **three** chained passes, each running over
/// the previous pass's output, each falling back to the empty string on a missing variable.
///
/// Chaining is observable and is why this is not one alternation: with `A="$env:B"` and `B="2"`,
/// `"${A}"` resolves to `"2"`. (The single-alternation form belongs to `getMissingEnvVars`, which
/// *scans* rather than substitutes — transposing the two is how `{env:VAR}` went missing here.)
pub fn interpolate_env_vars_with(
    value: &str,
    lookup: impl Fn(&str) -> Option<String> + Copy,
) -> String {
    let after_braces = expand_pattern(value, "${", Some("}"), lookup);
    let after_dollar_env = expand_pattern(&after_braces, "$env:", None, lookup);
    expand_pattern(&after_dollar_env, "{env:", Some("}"), lookup)
}

/// Expand `<open><NAME><close?>` where `NAME` is `[A-Za-z0-9_]+`. `close: Some` is the
/// delimited form (`${NAME}`, `{env:NAME}`); `close: None` runs the name to the first non-word
/// character (`$env:NAME`). A malformed or empty reference is emitted verbatim.
fn expand_pattern(
    input: &str,
    open: &str,
    close: Option<&str>,
    lookup: impl Fn(&str) -> Option<String> + Copy,
) -> String { /* body of mcp_direct_tools.rs:1309-1349, with `env(name)` → `lookup(name)` */ }
```

**Write the third pass as the delimited form, not as a copy of `interpolate_dollar_env`.** The
`$env:` scanner stops at the first non-word byte and does not require a terminator; applying that
shape to `{env:` would expand `{env:café}` as `caf` where the JS regex leaves it literal. The
delimited arm finds the `}` first and validates the *whole* name, which is what
`("{env:café}", "{env:café}")` at
[mcp_direct_tools.rs:2635](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs) pins.

Promote `interpolate_env_vars` (`:139`) from `pub(crate)` to `pub`. Then delete the two duplicate
bodies and delegate:

* [credentials.rs:3323-3351](../../crates/cyrup-mcp/src/credentials.rs) —
  `pub fn interpolate_env_vars_with<F>(value: &str, lookup: F) -> String where F: Fn(&str) -> Option<String> + Copy
  { cyrup_ext::caps::proc::interpolate_env_vars_with(value, lookup) }`. The `+ Copy` is the only
  signature change and every caller already satisfies it: `|name| env(name)` at `:3307` captures
  `&EnvFn` by reference, and the test closure at
  [oauth.rs:4002-4008](../../crates/cyrup-mcp/src/oauth.rs) captures nothing. The re-exports at
  [secrets.rs:83](../../crates/cyrup-mcp/src/secrets.rs) and
  [oauth.rs:105](../../crates/cyrup-mcp/src/oauth.rs) keep resolving unchanged. Keep the `LazyLock`
  regex only if something else in the file still uses it — `interpolate_env_vars_with` was its sole
  consumer.
* [mcp_direct_tools.rs:1300-1349](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs) —
  `fn interpolate_env_vars(value: &str, env: &dyn Fn(&str) -> Option<String>) -> String
  { cyrup_ext::caps::proc::interpolate_env_vars_with(value, |name| env(name)) }`, deleting
  `expand_pattern`. **Keep `match_placeholder` (`:1042-1058`) and `missing_env_vars`** — those are
  `getMissingEnvVars`, a scanner with a deliberately different pass structure, and the file's own
  doc at `:1006-1019` explains why they must not be merged.

Note what MCP-342 does *not* touch: the stdio env of an MCP server already goes through the
three-form `cyrup-mcp` implementation via `secrets::resolve_stdio_env`
([secrets.rs:380-391](../../crates/cyrup-mcp/src/secrets.rs)). The two-form copy affects the
WASM-guest `ProcCaps` env only. Do not conflate them while editing.

### Step 8 — `execOpen` / `openUrl` and the `$BROWSER` wiring (MCP-086)

In [ui.rs](../../crates/cyrup-mcp/src/ui.rs), beside `open_path` (`:3070`), add the shared dispatch
and refactor `open_path` onto it.

```rust
/// `execOpen(pi, target, browser?, signal?)` (`utils.ts:7-28`) — the platform dispatch shared by
/// [`open_path`] and [`open_url`].
///
/// Spawned with `tokio::process::Command` rather than `HostServices::exec`, which
/// `13b-mcp-config.md:1477` names: that trait is the WASM-guest capability model, `cyrup-mcp` is a
/// native built-in crate (13g's own refutation, and `resolve_command_secret`/`open_path` both
/// already spawn directly), and the cancel token 13b wanted the trait for arrives from the caller.
fn exec_open_argv(target: &str, browser: Option<&str>) -> (String, Vec<String>) {
    #[cfg(target_os = "macos")]
    {
        // `isAbsolute(browser) && extname(browser).toLowerCase() !== ".app"` — an absolute
        // executable path names a launcher binary; app bundles still go through `open -a`.
        if let Some(browser) = browser {
            let path = Path::new(browser);
            let is_app_bundle = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("app"));
            return if path.is_absolute() && !is_app_bundle {
                (browser.to_string(), vec![target.to_string()])
            } else {
                (
                    "open".to_string(),
                    vec!["-a".to_string(), browser.to_string(), target.to_string()],
                )
            };
        }
        ("open".to_string(), vec![target.to_string()])
    }
    #[cfg(target_os = "windows")]
    {
        let mut args =
            vec!["/c".to_string(), "start".to_string(), String::new()];
        if let Some(browser) = browser {
            args.push(browser.to_string());
        }
        args.push(target.to_string());
        ("cmd".to_string(), args)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    match browser {
        Some(browser) => (browser.to_string(), vec![target.to_string()]),
        None => ("xdg-open".to_string(), vec![target.to_string()]),
    }
}

async fn exec_open(
    target: &str,
    browser: Option<&str>,
    cancel: Option<&cyrup_core::CancelToken>,
) -> Result<std::process::Output, String> {
    let (program, args) = exec_open_argv(target, browser);
    let child = tokio::process::Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // `proc.kill()` in `execOpen`'s abort listener, as a drop-net: `run_until_cancelled`
        // drops the `wait_with_output` future on cancel, which drops the `Child`, which kills it.
        // Without this tokio leaves the launcher running past the teardown that cancelled it.
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("{program}: {error}"))?;

    let wait = child.wait_with_output();
    let output = match cancel {
        Some(token) => token
            .run_until_cancelled(wait)
            .await
            .ok_or_else(|| crate::abort::ABORTED_FALLBACK_REASON.to_string())?,
        None => wait.await,
    };
    output.map_err(|error| format!("{program}: {error}"))
}

/// `utils.ts:30-35` `openUrl(pi, url, browser?, signal?)` (MCP-086).
pub async fn open_url(
    url: &str,
    browser: Option<&str>,
    cancel: Option<&cyrup_core::CancelToken>,
) -> Result<(), String> {
    let output = exec_open(url, browser, cancel).await?;
    if output.status.success() {
        return Ok(());
    }
    // `result.stderr || `Failed to open browser (exit code ${result.code})``
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        format!(
            "Failed to open browser (exit code {})",
            output.status.code().map_or_else(|| "unknown".to_string(), |code| code.to_string())
        )
    } else {
        stderr
    })
}
```

`open_path` keeps its signature, its `Failed to open path (exit code …)` text and its
stderr-wins rule, and becomes `exec_open(&target.display().to_string(), None, None)` — upstream's
`openPath` passes neither a browser nor a signal (`utils.ts:37-42`), which is exactly the
`(None, None)` pair.

Then replace the stub at [runtime.rs:229-240](../../crates/cyrup-mcp/src/runtime.rs):

```rust
// Steps 8-9. `openBrowser: async (url) => { owner.throwIfInactive();
// await openUrl(pi, url, process.env.BROWSER, owner.signal); owner.throwIfInactive(); }`
// (`init.ts:175-179`). `process.env.BROWSER` is read inside the closure upstream, so it is read
// per call here too; an empty value is falsy to `execOpen`'s `if (browser)` and must be `None`.
let open_browser: OpenBrowser = {
    let owner = Arc::clone(&owner);
    Arc::new(move |url: String| {
        let owner = Arc::clone(&owner);
        Box::pin(async move {
            owner.throw_if_inactive()?;
            let browser = std::env::var("BROWSER").ok().filter(|value| !value.is_empty());
            let result = crate::ui::open_url(&url, browser.as_deref(), Some(&owner.token()))
                .await
                .map_err(McpError::other);
            owner.throw_if_inactive()?;
            result
        })
    })
};
```

The both-sides `throw_if_inactive` guards are already correct and stay; only the middle line
changes. Leave [`OpenerLauncher`](../../crates/cyrup-mcp/src/oauth.rs) (`oauth.rs:2394-2400`)
untouched.

---

## Acceptance Criteria

**npx resolver cache — MCP-104 / MCP-108**

- [ ] `CACHE_VERSION` is `2` in `crates/cyrup-ext/src/caps/proc/npx_resolver.rs`, and
      `CACHE_TTL_MS` (24 h) and `FORCE_CACHE_TIMEOUT` (30 s) are unchanged.
- [ ] `clear_legacy_cache_at` evicts a cache file whose `version` is exactly `1`, by `remove_file`
      falling back to `fs::write(path, "")`, and returns `true`; `load_cache_at` returns `None`
      without further reads when it did.
- [ ] A cache file whose `version` is `3`, or is a non-number, is **not** deleted — `load_cache_at`
      returns `None` and the file survives on disk.
- [ ] `load_cache_at` returns the usable entries of a file containing one valid entry and one
      malformed one (missing `isJs`; a string `resolvedAt`; a non-finite `resolvedAt`; a numeric
      `packageVersion`), rather than `None`.
- [ ] `save_cache_entry_at` reads through the non-evicting path, so saving into a v1 file does not
      route through `clear_legacy_cache_at`, and the resulting file is a valid v2 cache.
- [ ] Both `npm` invocations go through one helper that resolves to `cmd /C npm` on Windows and
      bare `npm` elsewhere; `Command::new("npm")` appears nowhere in the module.

**npx resolver key and cancellation — MCP-106 / MCP-107**

- [ ] `cache_key` takes `(command, package_spec, bin_name)` and is called after `parse_npx_args` /
      `parse_npm_exec_args`; two resolutions differing only in trailing args land on one entry, and
      `npx pkg bin` and `npx --package pkg bin` produce the same key.
- [ ] `resolve_npx_binary` takes `cancel: &cyrup_core::CancelToken` and returns
      `Result<Option<NpxResolution>, NpxAborted>`; an already-cancelled token yields `Err` before
      any parse, filesystem read or spawn.
- [ ] `force_npx_cache` returns `Err(NpxAborted)` on a token cancelled during its poll, after
      killing **and reaping** the child, and again on exit if the token fired while it waited; a
      cancel 100 ms into a long-running stand-in returns promptly rather than at the 30 s ceiling
      and leaves no surviving child.
- [ ] A spawn failure or a non-zero exit from `npm exec` still yields `Ok(())` (upstream's swallow),
      not `Err`.

**Wiring — MCP-103**

- [ ] `cyrup_ext::caps::proc::{resolve_npx_binary, NpxResolution, NpxAborted}` are `pub` and
      `NpxResolution`'s three fields are `pub`; `NpxResolution::rewrite` is the single
      implementation of the `isJs` rewrite and `apply_npx_resolution` delegates to it.
- [ ] `ConnectionBuilder::connect_stdio` calls `resolve_npx_binary` when and only when the
      configured command is exactly `npx` or `npm`, after args interpolation and **before** step
      4's `throw_if_aborted`, on a blocking task, with `request.attempt` as the token.
- [ ] On a hit the spawned command is `node` with `[bin_path, ...extra_args]` when `is_js`, and
      `bin_path` with `extra_args` otherwise; on a miss `command`/`args` are unchanged.
- [ ] `tracing::debug!` emits `<name> resolved to <bin_path> (skipping npm parent)` on a hit and
      nothing on a miss.
- [ ] `Err(NpxAborted)` from the resolver surfaces as `McpError::Aborted` from `connect_stdio` —
      so `crate::abort::is_abort_error` classifies it as a cancellation — and **no child is
      spawned** on that path.
- [ ] The `// Step 3 — MCP-103, NOT PORTED` marker and the "MCP-103 is **not ported**" sentence in
      `StdioChildConnection`'s residual note are both removed or re-stated to the new truth.

**Interpolation — MCP-342**

- [ ] `cyrup_ext::caps::proc::interpolate_env_vars_with` is `pub`, runs three chained passes, and
      is the workspace's only implementation: neither
      `cyrup_mcp::credentials::interpolate_env_vars_with` nor
      `cyrup_ext_subagents::exec::mcp_direct_tools::interpolate_env_vars` contains a scanner or a
      regex of its own any more.
- [ ] All three forms expand from the `cyrup-ext` engine, in upstream's order, with a missing
      variable expanding to the empty string in each: `${A}`, `$env:A`, `{env:A}`; and chaining is
      observable — `A="$env:B"`, `B="2"` makes `"${A}"` yield `"2"`.
- [ ] The delimited third pass leaves `{env:café}`, `{env:}`, `{env:-}` and `{env:A` byte-for-byte
      untouched, while `$env:Bc` still consumes `Bc` as its name.
- [ ] The env applied at `ProcCaps::spawn` expands a `{env:VAR}` value.
- [ ] `match_placeholder` / `missing_env_vars` in `mcp_direct_tools.rs` are untouched.

**Browser — MCP-086**

- [ ] `open_url(url, browser, cancel)` exists in `cyrup-mcp` and `open_path` shares its dispatch;
      neither is routed through `HostServices::exec` or `opener`.
- [ ] The argv matches upstream's table for all seven cases: darwin absolute-non-`.app` browser →
      `[browser, target]`; darwin `.app` or non-absolute browser → `open -a <browser> <target>`;
      darwin unset → `open <target>`; win32 set → `cmd /c start "" <browser> <target>`; win32 unset
      → `cmd /c start "" <target>`; other set → `[browser, target]`; other unset →
      `xdg-open <target>`.
- [ ] A non-zero exit yields the trimmed stderr when non-empty, else
      `Failed to open browser (exit code N)` from `open_url` and
      `Failed to open path (exit code N)` from `open_path`.
- [ ] A cancelled token returns before the child exits and does not leave the launcher running.
- [ ] `state.open_browser` passes its `url` to `open_url` with `$BROWSER` (empty string treated as
      unset) and `owner.token()`, keeps `throw_if_inactive` on both sides of the await, and no
      longer discards its argument.
- [ ] `OpenerLauncher` still calls `opener::open` and is unchanged.
