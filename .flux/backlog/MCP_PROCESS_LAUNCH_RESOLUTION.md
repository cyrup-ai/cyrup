---
stage: aug
status: done
updated: 2026-08-27 06:00
---

# Process Launch: npx Pre-Resolution, Env Interpolation, Browser Open

## Description

Seven units, one question: **what binary does this port actually exec, and with what environment?**

The question is asked at three places and answered inconsistently at all three today:

1. **the npx resolver** — [`cyrup_ext::caps::proc::npx_resolver`](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)
   is a 1219-line direct port of [`npx-resolver.ts`](../../tmp/pi-mcp-adapter/npx-resolver.ts) that
   rewrites an `npx`/`npm exec` invocation down to the real MCP server binary. It is complete enough
   to be trusted and **unreachable from `cyrup-mcp`** (`mod npx_resolver;` is private at
   [caps/proc.rs:25](../../crates/cyrup-ext/src/caps/proc.rs), `resolve_npx_binary` is `pub(super)`
   at [npx_resolver.rs:114](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)), and it carries
   four divergences from v2.26.1 in its cache and its cancellation (MCP-104, MCP-106, MCP-107,
   MCP-108);
2. **the stdio connection builder** — `ConnectionBuilder::connect_stdio` has the call site marked and
   empty at [runtime.rs:2404-2414](../../crates/cyrup-mcp/src/runtime.rs) (MCP-103);
3. **the browser opener** — `state.open_browser` is a closure that returns `Ok(())` without opening
   anything ([runtime.rs:229-241](../../crates/cyrup-mcp/src/runtime.rs)), and the string `BROWSER`
   appears nowhere in the workspace outside prose (MCP-086).

Plus the one interpolation function every spawn path is supposed to share (MCP-342).

**These are not seven file-disjoint edits and must not be split by file.**

* MCP-103's fix is a *visibility promotion in `crates/cyrup-ext`* consumed at a *marked insertion
  point in `crates/cyrup-mcp`*. A file-based split puts the resolver and its only production caller
  in different agents' change sets — the PR #30 failure mode.
* MCP-104, MCP-106, MCP-107 and MCP-108 all edit
  [npx_resolver.rs](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs) and conflict textually with
  MCP-103: MCP-107 changes the *signature and return type* of the very function MCP-103 promotes, and
  MCP-104/MCP-108 rewrite the cache-loading path MCP-106 keys into.
* MCP-342's consolidation target is `interpolate_env_vars_with`
  ([caps/proc.rs:148](../../crates/cyrup-ext/src/caps/proc.rs)), whose production caller is the env
  applied at [caps/proc.rs:526](../../crates/cyrup-ext/src/caps/proc.rs) to the same `ProcCaps::spawn`
  whose npx branch MCP-103/107 rewrites, eighteen lines above.
* MCP-086 is the same spawn discipline for the browser arm: the platform dispatch, `$BROWSER`, and a
  cancel token, in a crate that has already decided (correctly) to spawn its own children.

Upstream source for every citation below is the checkout at
[`tmp/pi-mcp-adapter`](../../tmp/pi-mcp-adapter) at tag `v2.26.1` (`fafae21`) — verified with
`git describe --tags` in that tree.

---

## Finding 0 — the reason all of this is one task, and the reason MCP-106 is not `low`

**Landing MCP-103 on its own writes live credentials to a plaintext file on disk.** This is the
central finding of this pass and it hard-orders the work below.

`cache_key` today is the whole argv:

```rust
// crates/cyrup-ext/src/caps/proc/npx_resolver.rs:767-772  — CURRENT
fn cache_key(command: &str, args: &[String]) -> String {
    let mut all = Vec::with_capacity(args.len() + 1);
    all.push(command.to_string());
    all.extend(args.iter().cloned());
    serde_json::to_string(&all).unwrap_or_default()
}
```

That string becomes a JSON **object key** in `<agent_dir>/mcp-npx-cache.json`
([npx_resolver.rs:744-765](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)). And the `args`
MCP-103 would hand it are the **post-interpolation** ones — `runtime.rs:2397-2402` already runs
`crate::credentials::interpolate_env_vars(arg, &self.env)` over every argument, so `${GITHUB_TOKEN}`
has already been expanded into the real token by the time step 3 runs. An `mcp.json` entry of
`"args": ["-y", "srv", "--token=${GITHUB_TOKEN}"]` therefore persists the token, verbatim, forever.

Upstream found and fixed exactly this, and the fix is why MCP-104 and MCP-106 exist. `git show 547fab4`
in [`tmp/pi-mcp-adapter`](../../tmp/pi-mcp-adapter) is one commit, `fix: interpolate stdio server args
(#337)`, whose body lists six sub-changes:

```
* fix: interpolate stdio server args        <- server-manager.ts:476, `args.map(interpolateEnvVars)`
* fix: keep npx cache keys secret-free      <- npx-resolver.ts:56,  the [command, packageSpec, binName] key
* fix: delete legacy npx resolver cache     <- npx-resolver.ts:8/485, CACHE_VERSION = 2 + clearLegacyCache
* fix: keep npx cache writes best effort
* fix: clear legacy npx cache contents
* fix: clear legacy npx cache on load
```

The diff of that one commit changes `let args = definition.args ?? []` to
`let args = (definition.args ?? []).map(interpolateEnvVars)` **and** `JSON.stringify([command, ...args])`
to `JSON.stringify([command, parsed.packageSpec, parsed.binName ?? ""])` **and** `CACHE_VERSION = 1`
to `= 2` with a new `clearLegacyCache`. Its own test fixtures name the payload: the legacy-cache tests
at [`__tests__/npx-resolver.test.ts:66`](../../tmp/pi-mcp-adapter/__tests__/npx-resolver.test.ts) and
[`:119`](../../tmp/pi-mcp-adapter/__tests__/npx-resolver.test.ts) write a v1 key of
`["npx","-y","demo-pkg","--token=secret-value"]` and the test at `:107` is literally titled
*"clears version-1 secrets and returns a cached package when cache save fails"*.

Three consequences, all load-bearing:

* **MCP-106 is a credential-exposure fix, not a cache-efficiency nicety.** The ledger's `low` severity
  ([13-cyrup-mcp-STATUS.md:616](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) and 13c's
  "two invocations … share one cache entry" framing
  ([13c-mcp-servers.md:1208-1216](../../docs/gap-analysis/13c-mcp-servers.md)) both describe the
  *side effect*, not the *purpose*.
* **It is already leaking on the guest path.** `ProcCaps::spawn` calls `resolve_npx_binary` today
  ([caps/proc.rs:508-512](../../crates/cyrup-ext/src/caps/proc.rs)) with the guest's own argv. Every
  WASM-guest `npx` spawn already writes that argv to the cache file. This is not hypothetical and it
  is not gated on MCP-103.
* **`CACHE_VERSION = 2` is the migration for it, which is why the eviction `unlink`s rather than
  ignores.** A v1 file is *definitionally* one whose keys are full argv. Ignoring it (what
  `load_cache_at` does today for a version mismatch) leaves the secrets on disk forever. That is why
  MCP-104 and MCP-106 are one edit and why both must precede MCP-103.

---

## Ledger corrections — read before touching anything

[13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) is dated 2026-08-21 and says
of itself ([:17-27](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) that a single row is "a lead
rather than a verdict". Eight statements across the ledger and the section specs are wrong or stale as
written. Each correction below was checked against the tree today.

**1 — MCP-342's row is stale, and its own instruction would make things worse.** The row
([:861](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) says "a THIRD implementation was added instead
of one shared implementation, and **the two pre-existing copies still carry the two-form parity
defect**." That is no longer true. There are three copies and **two of the three are already correct
three-form implementations**:

| copy | forms | evidence |
|---|---|---|
| `cyrup_mcp::credentials::interpolate_env_vars_with` | **three** | [credentials.rs:3323-3351](../../crates/cyrup-mcp/src/credentials.rs) — `\$\{(…)\}`, `\$env:(…)`, `\{env:(…)\}` as three chained `replace_all` passes |
| `cyrup_ext_subagents::exec::mcp_direct_tools::interpolate_env_vars` | **three** | [mcp_direct_tools.rs:1300-1304](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs) — MCP-143 landed; the module header records it at [:45-46](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs) |
| `cyrup_ext::caps::proc::interpolate_env_vars_with` | **two** | [caps/proc.rs:148-150](../../crates/cyrup-ext/src/caps/proc.rs) — `interpolate_dollar_env(&interpolate_braces(value, lookup), lookup)`, no `{env:VAR}` pass |

So the remaining parity defect is in exactly **one** copy, and the consolidation is still owed.

**2 — MCP-104's `std::sync::Once` is unnecessary and must not be written.** The row
([:614](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) and
[13c-mcp-servers.md:1182-1183](../../docs/gap-analysis/13c-mcp-servers.md) ask for `clear_legacy_cache`
"invoked once at module load via `std::sync::Once` inside `load_cache` **and** on every `load_cache()`".
Upstream's module-load call
([npx-resolver.ts:501](../../tmp/pi-mcp-adapter/npx-resolver.ts)) exists only because an ES module has
a load hook; Rust has none, and `load_cache_at` is the *only* reader of that file
([npx_resolver.rs:701-714](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)). A head-of-
`load_cache_at` call fully subsumes it. A `Once` would additionally be *wrong*: it would suppress the
eviction for every call after the first, which is precisely the arm upstream runs unconditionally at
[:504](../../tmp/pi-mcp-adapter/npx-resolver.ts).

**3 — MCP-104's eviction predicate is `version == 1`, not "version mismatch".** Upstream
`clearLegacyCache` is `if (raw?.version !== 1) return false;`
([npx-resolver.ts:488](../../tmp/pi-mcp-adapter/npx-resolver.ts)) — it evicts a v1 file and **leaves a
v3 or a garbage-version file alone**; `toNpxCache`'s own `raw.version !== CACHE_VERSION` check
([:473](../../tmp/pi-mcp-adapter/npx-resolver.ts)) then rejects it without deleting. A port that
deleted on any mismatch would let a future version's file be destroyed by an older binary.

**4 — MCP-107's row understates the change: the return type must become fallible.** The row
([:617](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) asks for "a `cancel` parameter with
`throw_if_aborted`-equivalent checks". But upstream `resolveNpxBinary` **throws** on abort
(`throwIfAborted(signal)` at [npx-resolver.ts:46](../../tmp/pi-mcp-adapter/npx-resolver.ts), and
`forceNpxCache` rejecting with `signal.reason ?? new Error("MCP request aborted")` at
[:266](../../tmp/pi-mcp-adapter/npx-resolver.ts)); it does not return `null`. The distinction is
load-bearing at the connect site: `None` means "not an npx invocation, run `command`/`args` verbatim",
and [server-manager.ts:480](../../tmp/pi-mcp-adapter/server-manager.ts)'s `if (resolved)` acts on it by
spawning the original command — so a cancel folded into `None` would make an aborted connect **spawn
`npx` during teardown**, the exact orphan this module exists to prevent.
`resolve_npx_binary` must return `Result<Option<NpxResolution>, NpxAborted>`.

**5 — MCP-086's row names the wrong gap, cites stale lines, and 13b's prescribed mechanism is wrong
for this crate.** The row ([:589](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) says
"`openUrl`/`execOpen`'s browser arm is missing" and cites `src/oauth.rs:2534` / `src/ui.rs:3062`.
Neither line number is current (the real hits are `oauth.rs:2392`, `ui.rs:3067`, and a third the row
misses entirely, `state.rs:55`), and the sharper and more damaging fact is that the consumer **already
exists and silently lies**: `state.open_browser` is built at
[runtime.rs:229-241](../../crates/cyrup-mcp/src/runtime.rs) with a body of
`owner.throw_if_inactive()?; let result = Ok(()); owner.throw_if_inactive()?; result` — the `url`
parameter is bound as `_url` and discarded. Separately,
[13b-mcp-config.md:1476-1477](../../docs/gap-analysis/13b-mcp-config.md) (repeated at
[:575-576](../../docs/gap-analysis/13b-mcp-config.md)) prescribes `HostServices::exec` as "the faithful
landing spot". **Do not use it.** [13g-mcp-oauth.md:1684-1687](../../docs/gap-analysis/13g-mcp-oauth.md)
already refuted that on the grounds that `HostServices::exec` is the WASM-guest capability model and
`cyrup-mcp` is a native built-in crate, and the tree has settled the other way twice: the sibling
[`open_path`](../../crates/cyrup-mcp/src/ui.rs) at `ui.rs:3080` and
[`resolve_command_secret`](../../crates/cyrup-mcp/src/secrets.rs) at `secrets.rs:202` both spawn a
`Command` directly. The cancel token that 13b wanted `HostServices::exec` for comes from the caller
(`owner.token()`), not from the host trait.

**6 — `13-cyrup-mcp.md:1244-1245` claims the port has "the same cache version" as upstream. It does
not.** The port is `const CACHE_VERSION: u32 = 1;`
([npx_resolver.rs:38](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)) against upstream's `2`
([npx-resolver.ts:8](../../tmp/pi-mcp-adapter/npx-resolver.ts)). The same paragraph also puts the file
at "892 lines"; it is 1219. Neither figure is load-bearing, but the cache-version claim is exactly the
one a reader would trust and skip MCP-104 over.

**7 — MCP-108's "PATH + PATHEXT walk" option is not viable and must not be attempted.**
[13c-mcp-servers.md:1236-1239](../../docs/gap-analysis/13c-mcp-servers.md) offers "either resolve
`npm.cmd`/`npm.exe` via a PATH + PATHEXT walk before `Command::new` or invoke through `cmd /c npm`".
npm ships no `npm.exe` — on Windows it is `npm.cmd` (plus `npm.ps1` and a POSIX shell script), and a
`.cmd` file cannot be handed to `CreateProcess` at all, resolved path or not. Only the `cmd /c` arm is
correct, which is why the single path below is prescribed and no choice is offered.

**8 — assorted stale citations, corrected here so the exec agent does not chase them.**
`crates/cyrup-mcp/Cargo.toml`'s `cyrup-ext` edge is at **:32** (comment `:15-31`), not `:17-19`.
The `("{env:café}", "{env:café}")` vector is at
[mcp_direct_tools.rs:2604](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs), not `:2635`.
`McpError::Aborted` / `ABORTED_FALLBACK_REASON` are
[abort.rs:95-104](../../crates/cyrup-mcp/src/abort.rs). `connect_stdio`'s step-2 args live at
`runtime.rs:2395-2402`, the MCP-103 marker at `:2404-2414`, step 4 at `:2417`, and the
`spawn_blocking` rationale MCP-103 reuses at `:2427-2445`.

**Context, not work: MCP-105 has landed.** The ledger lists it `missing`
([:615](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) and
[MCP_HIGH_SEVERITY_BACKLOG.md:154](MCP_HIGH_SEVERITY_BACKLOG.md) correctly takes it off the list.
`EXACT_PACKAGE_VERSION` is at
[npx_resolver.rs:62-72](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs), `parse_package_spec` at
`:499`, `cache_entry_is_usable`'s version arm at `:461-472`, and `find_cached_package_dir`'s
`exact_version` filter at `:566-617`. **Do not re-derive any of it** — MCP-106 edits the line directly
below `parse_package_spec`'s call and must leave that call in place.

**Context, not work: MCP-131 was overturned, but its residual stands and this task is its premise.**
[MCP_HIGH_SEVERITY_BACKLOG.md:60](MCP_HIGH_SEVERITY_BACKLOG.md) took MCP-131 off the high list
(`ManagerSupervisor::close`/`close_all` do delegate; `spawn_stdio_transport` has a production caller at
[runtime.rs:2474](../../crates/cyrup-mcp/src/runtime.rs)). What survives is the *process-group*
residual, and `StdioChildConnection`'s own doc at
[server_manager.rs:560-568](../../crates/cyrup-mcp/src/server_manager.rs) rests it on this task:
"Both signal a single pid, not a process group. The plan argues that is sufficient *because* npx
pre-resolution (MCP-103) removes the `npm` launcher… MCP-103 is **not ported**." Until MCP-103 lands,
the tracked child of an `npx` server is the npm launcher and a single-pid kill orphans the real server.

---

## Per-unit breakdown

### MCP-103 — wire npx/npm resolution into the connection builder · medium · `extension-owned`

Spec: [13c-mcp-servers.md:1161-1174](../../docs/gap-analysis/13c-mcp-servers.md).

**Unmet obligation.** `resolve_npx_binary` has no caller in `cyrup-mcp`. The site is marked
`// Step 3 — MCP-103, NOT PORTED` at
[runtime.rs:2404-2414](../../crates/cyrup-mcp/src/runtime.rs), between step 2's arg interpolation
(`:2395-2402`) and step 4's `throw_if_aborted` (`:2417`) — which is exactly upstream's order
([server-manager.ts:476-486](../../tmp/pi-mcp-adapter/server-manager.ts)). Visibility blocks it:
`mod npx_resolver;` at [caps/proc.rs:25](../../crates/cyrup-ext/src/caps/proc.rs),
`pub(super) fn resolve_npx_binary` at
[npx_resolver.rs:114](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs), `pub(super) struct
NpxResolution` with `pub(super)` fields at `:75-80`.

**Reachability is not a blocker.** `pub mod caps` is `#[cfg(feature = "wasm-host")]`
([lib.rs:147-148](../../crates/cyrup-ext/src/lib.rs)), `pub mod proc` is unconditional inside it
([caps/mod.rs:14](../../crates/cyrup-ext/src/caps/mod.rs)), and `cyrup-mcp` already states
`cyrup-ext = { workspace = true, features = ["wasm-host"] }` on its own edge
([Cargo.toml:32](../../crates/cyrup-mcp/Cargo.toml)). `cyrup-ext` already depends on `cyrup-core`
([Cargo.toml:21](../../crates/cyrup-ext/Cargo.toml)), so `CancelToken` needs no new edge either.

### MCP-104 — `CACHE_VERSION = 2` and `clearLegacyCache` · medium · `hand-written`

Spec: [13c-mcp-servers.md:1176-1185](../../docs/gap-analysis/13c-mcp-servers.md), corrected by findings
0, 2 and 3.

**Unmet obligation.** `const CACHE_VERSION: u32 = 1;` at
[npx_resolver.rs:38](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs) against upstream's
`const CACHE_VERSION = 2` ([npx-resolver.ts:8](../../tmp/pi-mcp-adapter/npx-resolver.ts)).
`load_cache_at` at `:707-714` rejects a version mismatch and never deletes; `grep clear_legacy` over
the crate returns nothing. Consequence, per finding 0: a v1 file's keys are full argv, so the secrets
already written there are never purged.

### MCP-106 — cache key must be `[command, packageSpec, binName]` · **credential exposure** · `hand-written`

Spec: [13c-mcp-servers.md:1208-1216](../../docs/gap-analysis/13c-mcp-servers.md), re-severitied by
finding 0.

**Unmet obligation.** `let cache_key = cache_key(command, args);` at
[npx_resolver.rs:124](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs), and `cache_key` at
`:767-772` serialises `[command, ...args]` — the whole argv, post-interpolation at the MCP-103 call
site. Upstream is `JSON.stringify([command, parsed.packageSpec, parsed.binName ?? ""])`
([npx-resolver.ts:56](../../tmp/pi-mcp-adapter/npx-resolver.ts)), computed after the parse. Note it
keys on `parsed.packageSpec` — the **raw spec string** (`"pkg@1.2.3"`), not `parse_package_spec`'s
extracted name. The upstream fixture that pins the exact key shape is
[`__tests__/npx-resolver.test.ts:157`](../../tmp/pi-mcp-adapter/__tests__/npx-resolver.test.ts):
`JSON.stringify(["npx", "demo-pkg", ""])`.

### MCP-107 — no cancellation path · medium · `hand-written`

Spec: [13c-mcp-servers.md:1218-1229](../../docs/gap-analysis/13c-mcp-servers.md), corrected by
finding 4.

**Unmet obligation.** `pub(super) fn resolve_npx_binary(command: &str, args: &[String])` at
[npx_resolver.rs:114](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs) takes no signal, and
`force_npx_cache` at `:395-419` is a 50 ms `std::thread::sleep` poll loop bounded only by
`FORCE_CACHE_TIMEOUT` (30 s, `:42`). Nothing can interrupt it. Once MCP-103 lands, that loop runs
inside the manager's single-flight connect future's blocking task, so `close`/`close_all` cannot
preempt an attempt for up to 30 s — the exact guarantee `connect_stdio`'s own comment at
[runtime.rs:2427-2445](../../crates/cyrup-mcp/src/runtime.rs) says it moved
`StdioTransportSpec::resolve` off the async worker to protect.

### MCP-108 — per-entry cache validation and Windows `npm` · low · `hand-written`

Spec: [13c-mcp-servers.md:1231-1240](../../docs/gap-analysis/13c-mcp-servers.md), corrected by
finding 7.

**Unmet obligation, half one.** `struct NpxCache { version: u32, entries: HashMap<String,
NpxCacheEntry> }` at [npx_resolver.rs:96-100](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs) is
deserialised whole by `serde_json::from_str::<NpxCache>` at `:709`, so one malformed entry discards
every cached resolution. Upstream's `toNpxCacheEntry`
([npx-resolver.ts:456-469](../../tmp/pi-mcp-adapter/npx-resolver.ts)) validates per entry and drops
only the bad ones, checking `typeof resolvedBin === "string"`,
`typeof resolvedAt === "number" && Number.isFinite(resolvedAt)`, `typeof isJs === "boolean"`, and
`packageVersion === undefined || typeof packageVersion === "string"`. The upstream fixture that pins
"one bad entry, whole file still usable" is
[`__tests__/npx-resolver.test.ts:144-171`](../../tmp/pi-mcp-adapter/__tests__/npx-resolver.test.ts).

**Half two.** `Command::new("npm")` at `:396` (`force_npx_cache`) and `:659` (`get_npm_cache_dir`)
against upstream's `crossSpawn` / `crossSpawn.sync`
([npx-resolver.ts:255](../../tmp/pi-mcp-adapter/npx-resolver.ts),
[:419](../../tmp/pi-mcp-adapter/npx-resolver.ts)). On Windows `npm` is `npm.cmd`, `CreateProcess` will
not run a batch file, and there is no `npm.exe` to find — so npx resolution is a silent no-op there
today and a PATH walk cannot fix it (finding 7). The in-tree idiom for routing through `cmd` is already
established: [secrets.rs:197-214](../../crates/cyrup-mcp/src/secrets.rs) uses
`#[cfg(windows)] let (shell, flag) = ("cmd", "/C");` with `CREATE_NO_WINDOW` at `:140`/`:213`.

### MCP-342 — a reachable, three-form `interpolate_env_vars` · medium · `hand-written`

Spec: [13g-mcp-oauth.md:1401-1417](../../docs/gap-analysis/13g-mcp-oauth.md), the three forms tabulated
at [13g-mcp-oauth.md:342-349](../../docs/gap-analysis/13g-mcp-oauth.md). Row corrected by finding 1.

**Unmet obligation.** `interpolate_env_vars_with` at
[caps/proc.rs:148-150](../../crates/cyrup-ext/src/caps/proc.rs) runs two passes; `interpolate_braces`
(`:157-176`) and `interpolate_dollar_env` (`:180-200`) are its halves and there is no `{env:VAR}` third.
Its production caller is `cmd.env(k, interpolate_env_vars(v))` at
[caps/proc.rs:526](../../crates/cyrup-ext/src/caps/proc.rs) — every environment variable of every
WASM-guest-spawned child, including the npx-resolved one this task rewrites — and it is also reached
by `resolve_config_path_with` at `:225-240`, so a guest `cwd` of `{env:HOME}/x` is equally literal
today. Upstream's three passes are [utils.ts:74-79](../../tmp/pi-mcp-adapter/utils.ts).

**The consolidation direction is forced by the dependency graph.** `cyrup-mcp` and
`cyrup-ext-subagents` both depend on `cyrup-ext` with `wasm-host`
([cyrup-mcp/Cargo.toml:32](../../crates/cyrup-mcp/Cargo.toml),
[cyrup-ext-subagents/Cargo.toml:39](../../crates/cyrup-ext-subagents/Cargo.toml)) and `cyrup-ext`
depends on neither. `cyrup_ext::caps::proc` is the only module all three can share, which is what 13g
means by "promoting the existing one to `pub`".

### MCP-086 — port the browser/path open dispatch · medium · `extension-owned`

Spec: [13b-mcp-config.md:1468-1482](../../docs/gap-analysis/13b-mcp-config.md), platform table at
[13b-mcp-config.md:563-576](../../docs/gap-analysis/13b-mcp-config.md) and again at
[13a-mcp-activation.md:352-364](../../docs/gap-analysis/13a-mcp-activation.md). Row and mechanism
corrected by finding 5.

**Unmet obligation.** Three things:

* `execOpen`'s **browser arm** does not exist. `open_path` at
  [ui.rs:3070-3097](../../crates/cyrup-mcp/src/ui.rs) hard-codes the no-`browser` column of the table
  and takes no cancel; its own doc at `:3067` says the `$BROWSER` override "is shared with `openUrl`"
  and defers it.
* `$BROWSER` is never read. Grep for `BROWSER` over `crates/cyrup-mcp/src` and `crates/cyrup-ext/src`
  returns three hits, all prose: [state.rs:55](../../crates/cyrup-mcp/src/state.rs),
  [ui.rs:3067](../../crates/cyrup-mcp/src/ui.rs),
  [oauth.rs:2392](../../crates/cyrup-mcp/src/oauth.rs).
* `state.open_browser`'s production body is a stub that discards its URL —
  [runtime.rs:229-241](../../crates/cyrup-mcp/src/runtime.rs). Upstream's is
  `owner.throwIfInactive(); await openUrl(pi, url, process.env.BROWSER, owner.signal);
  owner.throwIfInactive();` ([init.ts:175-179](../../tmp/pi-mcp-adapter/init.ts)). The two guards are
  already there; the middle line is not.

**Not in scope, and do not "unify" them.** [`OpenerLauncher`](../../crates/cyrup-mcp/src/oauth.rs) at
`oauth.rs:2393-2400` calls `opener::open` and is *correct*: it ports the direct npm `open` import in
`mcp-auth-flow.ts`, a genuinely different upstream mechanism (MCP-338, settled —
[13b-mcp-config.md:578-580](../../docs/gap-analysis/13b-mcp-config.md)). Both mechanisms exist upstream
and both stay.

---

## Implementation

Order is forced, not stylistic: 104 + 106 are one credential-exposure edit and its migration (finding
0); 108 half one restructures the loader they both key into; 107 changes the signature; 103 writes the
caller against the final signature; 342 and 086 are independent of the four and of each other.

### Step 1 — `npx_resolver.rs`: the cache path (MCP-104 + MCP-108 half one)

Replace `load_cache_at` ([:707-714](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)) with
upstream's four-function split. Keep `NpxCacheEntry`'s `serde` derives — they are still the shape
`save_cache_entry_at` serialises — but stop using them to parse the whole file at once.
`asRecord`/`createCacheEntries` ([npx-resolver.ts:446-454](../../tmp/pi-mcp-adapter/npx-resolver.ts))
have no port: `Value::as_object` is `asRecord` (it rejects an array and a null the same way), and a
`HashMap` has no prototype to poison, which is the whole of what `Object.create(null)` bought — the
`__proto__` fixture at
[`__tests__/npx-resolver.test.ts:173`](../../tmp/pi-mcp-adapter/__tests__/npx-resolver.test.ts) is
satisfied by construction here.

```rust
/// `npx-resolver.ts:8` `CACHE_VERSION` (MCP-104).
///
/// Bumped from 1 with `clear_legacy_cache_at` below, and the pair is a MIGRATION, not a
/// housekeeping chore: a v1 file is by definition one whose entry keys are the full argv
/// (`cache_key`'s old shape), which for an interpolated `mcp.json` argument list means real
/// credentials sitting in a plaintext JSON key. Upstream bumped the version in the same commit
/// that narrowed the key, `547fab4` "keep npx cache keys secret-free". Never change one without
/// the other.
const CACHE_VERSION: u32 = 2;

/// The version `clear_legacy_cache_at` evicts. Upstream's predicate is `raw?.version !== 1`
/// (`npx-resolver.ts:488`) — it deletes a v1 file and deliberately leaves a *newer*-than-current
/// or garbage-version file alone, which `to_npx_cache`'s own check then rejects without deleting.
/// Evicting on any mismatch would let an older binary destroy a newer one's cache.
const LEGACY_CACHE_VERSION: f64 = 1.0;

/// `npx-resolver.ts:437-444` `readNpxCachePayload`. The `existsSync` guard has no port: a missing
/// file and an unreadable one both land on the same `None` here as they do on the same `null` there.
fn read_npx_cache_payload(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// `raw.version === n`, compared as a JS number rather than as a `u64` (MCP-104).
///
/// JSON `1.0` is `1` to `JSON.parse`, so upstream's `!== 1` is false for it; `Value::as_u64` on a
/// `1.0` returns `None` and would silently skip an eviction upstream performs. Comparing small
/// integral `f64`s for exact equality is well defined — both sides are exactly representable.
fn json_version_is(raw: &serde_json::Map<String, serde_json::Value>, expected: f64) -> bool {
    raw.get("version").and_then(serde_json::Value::as_f64) == Some(expected)
}

/// `npx-resolver.ts:456-469` `toNpxCacheEntry` (MCP-108) — per-entry validation, so one corrupt
/// entry drops itself instead of taking the whole file with it.
fn to_npx_cache_entry(value: &serde_json::Value) -> Option<NpxCacheEntry> {
    let raw = value.as_object()?;
    // `typeof raw.resolvedBin !== "string"`.
    let resolved_bin = raw.get("resolvedBin")?.as_str()?.to_string();
    // `typeof raw.resolvedAt !== "number" || !Number.isFinite(raw.resolvedAt)`. `as_f64` is the
    // `typeof === "number"` half — it rejects a string and a bool — and `is_finite` is upstream's
    // second predicate, kept verbatim rather than reasoned away.
    let resolved_at = raw.get("resolvedAt")?.as_f64()?;
    // NAMED DELTA, and unobservable. `resolvedAt` is a `Date.now()` millisecond count; upstream's
    // field is a JS `number` and it KEEPS a negative or absurd one, letting the TTL arithmetic
    // reject the entry at use time. This port's field is a `u64`, so a value outside the safe
    // integer range is dropped here instead. `cache_entry_is_usable` computes
    // `now.saturating_sub(resolved_at) >= CACHE_TTL_MS`, which is true for every such value, so no
    // dropped entry could ever have produced a hit; what changes is only that `save_cache_entry`'s
    // merge stops carrying the junk forward.
    const MAX_SAFE_INTEGER_MS: f64 = 9_007_199_254_740_991.0;
    if !resolved_at.is_finite() || !(0.0..=MAX_SAFE_INTEGER_MS).contains(&resolved_at) {
        return None;
    }
    // `typeof raw.isJs !== "boolean"`.
    let is_js = raw.get("isJs")?.as_bool()?;
    // `raw.packageVersion !== undefined && typeof raw.packageVersion !== "string"` — absent is
    // fine, present-and-not-a-string drops the entry. A JSON `null` is `undefined`'s nearest
    // neighbour and upstream's `!== undefined` keeps it, so `Null` is treated as absent rather
    // than as a type failure.
    let package_version = match raw.get("packageVersion") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(value.as_str()?.to_string()),
    };
    // The bound check above makes this cast exact; the `allow` records that it was reasoned about
    // rather than reached for. (Attributes go on the `let`, not on a struct-expression field.)
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let resolved_at = resolved_at as u64;
    Some(NpxCacheEntry { resolved_bin, resolved_at, package_version, is_js })
}

/// `npx-resolver.ts:471-483` `toNpxCache` (MCP-108).
fn to_npx_cache(value: &serde_json::Value) -> Option<NpxCache> {
    let raw = value.as_object()?;
    if !json_version_is(raw, f64::from(CACHE_VERSION)) {
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
/// none is needed: [`load_cache_at`] below is the only reader of this file and it calls this on
/// every load — which is upstream's *other*, unconditional call site (`:504`). A `std::sync::Once`
/// here would be strictly worse than nothing, suppressing every eviction after the first.
fn clear_legacy_cache_at(path: &Path) -> bool {
    let Some(payload) = read_npx_cache_payload(path) else { return false };
    let Some(raw) = payload.as_object() else { return false };
    if !json_version_is(raw, LEGACY_CACHE_VERSION) {
        return false;
    }
    if fs::remove_file(path).is_err() {
        // `catch { writeFileSync(cachePath, "") }` — a read-only directory can refuse the unlink
        // and still allow a truncate, and truncating is what actually removes the secrets. Both
        // failing is upstream's silent third arm: cleanup is best effort, resolution proceeds.
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
([:755-756](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)) from `load_cache_at(path)` to the
non-evicting path:

```rust
let mut merged = read_npx_cache_payload(path)
    .as_ref()
    .and_then(to_npx_cache)
    .unwrap_or_else(|| NpxCache { version: CACHE_VERSION, entries: HashMap::new() });
```

This is not cosmetic: upstream's `saveCacheEntry` calls `toNpxCache(readNpxCachePayload(cachePath))`
([npx-resolver.ts:515](../../tmp/pi-mcp-adapter/npx-resolver.ts)), **not** `loadCache`, so a save must
not run the eviction. Overwriting a v1 file with a fresh v2 one is exactly what
[`__tests__/npx-resolver.test.ts:107-142`](../../tmp/pi-mcp-adapter/__tests__/npx-resolver.test.ts)
pins. Leave the `SAVE_CACHE_LOCK` (`:731`) and the tmp-file-rename cycle exactly as they are.

### Step 2 — `npx_resolver.rs`: the cache key (MCP-106)

```rust
/// `npx-resolver.ts:56` — `JSON.stringify([command, parsed.packageSpec, parsed.binName ?? ""])`,
/// computed AFTER the parse (MCP-106).
///
/// Three elements, never the argv. The argv form this replaces put every trailing argument into a
/// JSON object key on disk, and at the `cyrup-mcp` call site those arguments have already been
/// through `interpolate_env_vars` — so `--token=${GITHUB_TOKEN}` was persisted expanded. That is
/// what upstream's `547fab4` "keep npx cache keys secret-free" closed, and why `CACHE_VERSION` is
/// bumped in the same edit.
///
/// `parsed.packageSpec` is the RAW spec (`"pkg@1.2.3"`), not `parse_package_spec`'s extracted name:
/// two invocations of the same package/bin that differ only in trailing arguments must share one
/// entry, and two that differ in requested version must not.
///
/// `serde_json::to_string` over a `[&str; 3]` is byte-identical to `JSON.stringify` for every input
/// this sees — same array punctuation with no spaces, same `"`/`\`/control-character escapes, and
/// neither escapes non-ASCII — which matters because the file is shared with a co-installed pi
/// adapter whenever `PI_CODING_AGENT_DIR` is the resolved agent dir (`agent_dir()`, `:675-694`).
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
that is MCP-105's, already landed, and `cache_entry_is_usable`'s second argument still needs it.

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

    // ... unchanged through the cached-hit and warm `resolve_from_npm_cache` arms, each `return
    // Some(..)` becoming `return Ok(Some(..))`.

    // Slow path: force npx cache population (`npx-resolver.ts:75-83`).
    force_npx_cache(&parsed.package_spec, cancel)?;
    let Some(resolved_after_install) =
        resolve_from_npm_cache(&parsed.package_spec, parsed.bin_name.as_deref())
    else {
        // The trailing `?` this replaces would now short-circuit the WRONG type.
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

`force_npx_cache` grows the same token. Its three upstream abort points are the entry `throwIfAborted`
([:252](../../tmp/pi-mcp-adapter/npx-resolver.ts)), the `abort` listener that kills the child
([:264-267](../../tmp/pi-mcp-adapter/npx-resolver.ts)), and the trailing `throwIfAborted` after the
swallow-everything catch ([:282-285](../../tmp/pi-mcp-adapter/npx-resolver.ts)):

```rust
/// `npx-resolver.ts:251-286` `forceNpxCache` — see the module doc for why this blocks via
/// `std::process::Command` rather than `tokio::process`. Every failure mode (spawn error, timeout,
/// non-zero exit) is still swallowed exactly like the TS `catch { /* Ignore failures */ }`; the
/// ONLY new exit is the abort, which upstream also rejects with rather than swallowing.
fn force_npx_cache(package_spec: &str, cancel: &CancelToken) -> Result<(), NpxAborted> {
    if cancel.is_cancelled() {
        return Err(NpxAborted);
    }
    let spawned = npm_command(&["exec", "--yes", "--package", package_spec, "--", "node", "-e", "1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        // `proc.on("error")` rejects, the `catch` swallows it, then `throwIfAborted(signal)` runs.
        return if cancel.is_cancelled() { Err(NpxAborted) } else { Ok(()) };
    };

    let deadline = Instant::now() + FORCE_CACHE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                // The `abort` listener's `proc.kill()` (`npx-resolver.ts:265`), on this port's
                // 50 ms tick instead of Node's event loop. Reap as well as kill, so a teardown
                // never leaves a zombie behind.
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
    // `npx-resolver.ts:285` — the re-check on exit, which catches a token that fired between the
    // last tick and the child's own exit.
    if cancel.is_cancelled() { Err(NpxAborted) } else { Ok(()) }
}
```

The two existing tests that call the old arity —
[npx_resolver.rs:1029](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs) and `:1034` — take
`&CancelToken::new()` and assert `Ok(None)`.

### Step 4 — `npx_resolver.rs`: Windows `npm` (MCP-108 half two)

One helper, replacing both `Command::new("npm")` sites
([:396](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs) and `:659`). The `cmd /C` arm is the only
correct one (finding 7) and it is the shape
[secrets.rs:197-214](../../crates/cyrup-mcp/src/secrets.rs) already uses in this workspace for the
identical problem, down to the console suppression.

```rust
/// `cross-spawn`'s `escapeArgument` (`lib/util/escape.js`), narrowed to this module's argv.
///
/// Every argument except the package spec is a fixed ASCII flag. A spec CAN carry `>`/`<`/`|`/`&`
/// (`pkg@>=1.0.0`, `pkg@1.x||2.x`) — each one a `cmd.exe` metacharacter that would otherwise be
/// read as a redirect or a pipe — and it never carries a `"` or a `\`, so the backslash-doubling
/// half of cross-spawn's escape has nothing to do here and is omitted rather than reproduced
/// blind. Quote first, then caret-escape, including the quotes themselves: that is cross-spawn's
/// order and it is what `cmd /d /s /c` unwinds.
#[cfg(windows)]
fn escape_cmd_argument(arg: &str) -> String {
    const META: &[char] = &[
        '(', ')', '[', ']', '%', '!', '^', '"', '`', '<', '>', '&', '|', ';', ',', ' ', '*', '?',
    ];
    let mut out = String::with_capacity(arg.len() + 4);
    for ch in std::iter::once('"').chain(arg.chars()).chain(std::iter::once('"')) {
        if META.contains(&ch) {
            out.push('^');
        }
        out.push(ch);
    }
    out
}

/// `crossSpawn("npm", …)` / `crossSpawn.sync("npm", …)` (`npx-resolver.ts:255`, `:419`) — MCP-108.
///
/// On Windows `npm` is `npm.cmd`, a batch file `CreateProcess` will not run, and npm ships no
/// `npm.exe` for a PATH+PATHEXT walk to find — so every npx resolution silently no-ops there
/// unless it goes through the command interpreter. `%COMSPEC%` before the literal, `/d` to skip
/// AutoRun, `/s` so the outer quotes are stripped as one string, and `CREATE_NO_WINDOW` so a
/// resolution never flashes a console at the user (the same flag, for the same reason, as
/// `cyrup_mcp::secrets::resolve_command_secret`'s `windowsHide` arm).
fn npm_command(args: &[&str]) -> Command {
    #[cfg(windows)]
    let command = {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        use std::os::windows::process::CommandExt as _;
        let mut line = String::from("/d /s /c \"");
        line.push_str(&escape_cmd_argument("npm"));
        for arg in args {
            line.push(' ');
            line.push_str(&escape_cmd_argument(arg));
        }
        line.push('"');
        let comspec = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
        let mut command = Command::new(comspec);
        command.raw_arg(line).creation_flags(CREATE_NO_WINDOW);
        command
    };
    #[cfg(not(windows))]
    let command = {
        let mut command = Command::new("npm");
        command.args(args);
        command
    };
    command
}
```

`get_npm_cache_dir` ([:650-667](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs)) becomes
`npm_command(&["config", "get", "cache"]).output().ok()?`, leaving its `NPM_CONFIG_CACHE` short-circuit
and its `OnceLock` memoization untouched.

### Step 5 — `caps/proc.rs`: the promotion and the shared rewrite (MCP-103, `cyrup-ext` half)

In [npx_resolver.rs](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs), make `NpxResolution` and
its three fields `pub` (`:75-80`), and give it the rewrite as an inherent method so the two crates
cannot drift:

```rust
impl NpxResolution {
    /// `server-manager.ts:481-482` — `command = resolved.isJs ? "node" : resolved.binPath;
    /// args = resolved.isJs ? [resolved.binPath, ...resolved.extraArgs] : resolved.extraArgs;`
    ///
    /// The `resolved === null` arm is the caller's: upstream's `if (resolved)` simply never
    /// reassigns, and what "the original" is differs between the two call sites — `spec.cmd`/
    /// `spec.args` on the guest path, the interpolated `command`/`args` locals on the MCP one.
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
`pub mod npx_resolver;` plus `pub use npx_resolver::{resolve_npx_binary, NpxAborted, NpxResolution};`.
Both paths are then valid, which matters because
[13c-mcp-servers.md:1167](../../docs/gap-analysis/13c-mcp-servers.md) names the module path and
[13-cyrup-mcp-STATUS.md:613](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) names the re-export.
Reduce `apply_npx_resolution` (`:430-443`) to a delegation so its existing test
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
let resolved = if spec.cmd == "npx" || spec.cmd == "npm" {
    // The WIT `proc.spawn` handler has no signal to thread: a `CancelToken` is not a WIT value,
    // and `ctx-state.is-run-cancelled` is a poll the GUEST makes, not a token the host holds
    // (see this crate's `lib.rs` CYRUP-DELTA register). A never-cancelled token keeps this path's
    // behaviour exactly as it is today; the interrupt belongs to the `cyrup-mcp` caller, which has
    // a real one. `ok().flatten()` is upstream's `null` fallback for an arm that cannot fire.
    let never_cancelled = cyrup_core::CancelToken::new();
    tokio::task::block_in_place(|| {
        npx_resolver::resolve_npx_binary(&spec.cmd, &spec.args, &never_cancelled)
    })
    .ok()
    .flatten()
} else {
    None
};
```

### Step 6 — `runtime.rs`: the connection builder call (MCP-103, `cyrup-mcp` half)

Replace the marker comment at
[runtime.rs:2404-2414](../../crates/cyrup-mcp/src/runtime.rs). `command` (`:2396`) and `args`
(`:2397-2402`) become `let mut`; both are moved into `StdioTransportSpec::resolve`'s `spawn_blocking`
at `:2454-2455`, so nothing else has to change.

```rust
// Step 3 — MCP-103. `server-manager.ts:478-485`.
//
// Run on a blocking task for the same reason `StdioTransportSpec::resolve` is, thirty lines
// below: `resolve_npx_binary` is `std::process::Command` + `std::thread::sleep`, bounded by
// `FORCE_CACHE_TIMEOUT` (30 s), and this body is polled inside the manager's single-flight
// connect future. Inline it would hold a tokio worker for the whole cold-cache budget. The
// attempt token goes in with it, so `close`/`close_all` interrupts it at the next 50 ms tick
// instead of at the 30 s ceiling (MCP-107) — which is more than `resolve` below can promise.
if command == "npx" || command == "npm" {
    let resolve_command = command.clone();
    let resolve_args = args.clone();
    let attempt = request.attempt.clone();
    let resolved = match tokio::task::spawn_blocking(move || {
        cyrup_ext::caps::proc::resolve_npx_binary(&resolve_command, &resolve_args, &attempt)
    })
    .await
    {
        Ok(Ok(resolved)) => resolved,
        // `throwIfAborted` REJECTS; it does not resolve to `null`. Surfacing it as
        // `McpError::Aborted` is what keeps the manager's failure backoff from counting a user
        // teardown as a connection failure — `crate::abort::is_abort_error` classifies this
        // variant, and no other, as a cancellation. No child has been spawned on this path.
        Ok(Err(cyrup_ext::caps::proc::NpxAborted)) => {
            return Err(McpError::Aborted(crate::abort::ABORTED_FALLBACK_REASON.to_string()));
        }
        // Defensive, exactly like `StdioTransportSpec::resolve`'s own join arm below: the closure
        // cannot panic under this crate's lint policy and the runtime is alive. Reported as "not
        // an npx invocation", which runs `command`/`args` verbatim — the same fallback every
        // `null` arm of `resolveNpxBinary` takes.
        Err(_join) => None,
    };
    // `if (resolved) { ... }` — a `None` leaves `command`/`args` exactly as configured.
    if let Some(resolved) = resolved {
        let bin_path = resolved.bin_path.clone();
        (command, args) = resolved.rewrite();
        tracing::debug!("{name} resolved to {bin_path} (skipping npm parent)");
    }
}
```

The debug string is upstream's verbatim
([server-manager.ts:483](../../tmp/pi-mcp-adapter/server-manager.ts)). `McpError::Aborted` and
`ABORTED_FALLBACK_REASON` are [abort.rs:95-104](../../crates/cyrup-mcp/src/abort.rs); the fallback text
is `"MCP request aborted"`, which is also `throwIfAborted`'s own text when `signal.reason` is not an
`Error` ([abort.ts:3](../../tmp/pi-mcp-adapter/abort.ts)) and `forceNpxCache`'s rejection text
([npx-resolver.ts:266](../../tmp/pi-mcp-adapter/npx-resolver.ts)), so all three agree.

Three stale prose blocks in `cyrup-mcp` become false the moment this lands and must be restated in the
same change: the `// Step 3 — MCP-103, NOT PORTED` marker itself, the doc bullet at
[runtime.rs:1859-1861](../../crates/cyrup-mcp/src/runtime.rs) ("the call site is marked below so
landing MCP-103 is three lines"), and `StdioChildConnection`'s residual at
[server_manager.rs:560-568](../../crates/cyrup-mcp/src/server_manager.rs) ("MCP-103 is **not
ported**"). The residual's *second* half — a server that forks its own worker still leaves it behind,
and the fix is `process_wrap::tokio::ProcessGroup::leader()` — stays, and the test that pins it
(`a_forking_child_leaves_its_grandchild_behind`, `server_manager.rs:4525`) is unaffected.

### Step 7 — one `interpolate_env_vars` (MCP-342)

Replace `interpolate_braces` and `interpolate_dollar_env`
([caps/proc.rs:156-200](../../crates/cyrup-ext/src/caps/proc.rs)) with the generalised scanner
`cyrup-ext-subagents` already proved at
[mcp_direct_tools.rs:1306-1348](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs). It is
the only one of the three implementations parameterised by the delimiter pair and therefore the only
one that can serve all three forms from one body.

```rust
/// `interpolateEnvVars(value)` (`utils.ts:74-79`) — **three** chained passes, each running over
/// the previous pass's output, each falling back to the empty string on a missing variable.
///
/// Chaining is observable and is why this is not one alternation: with `A="$env:B"` and `B="2"`,
/// `"${A}"` resolves to `"2"`. (The single-alternation form belongs to `getMissingEnvVars`
/// (`utils.ts:83`), which *scans* rather than substitutes — transposing the two is how `{env:VAR}`
/// went missing here.)
pub fn interpolate_env_vars_with(
    value: &str,
    lookup: impl Fn(&str) -> Option<String> + Copy,
) -> String {
    let after_braces = expand_pattern(value, "${", Some("}"), lookup);
    let after_dollar_env = expand_pattern(&after_braces, "$env:", None, lookup);
    expand_pattern(&after_dollar_env, "{env:", Some("}"), lookup)
}

/// Expand `<open><NAME><close?>` where `NAME` is `[A-Za-z0-9_]+`. `close: Some` is the delimited
/// form (`${NAME}`, `{env:NAME}`); `close: None` runs the name to the first non-word character
/// (`$env:NAME`). A malformed or empty reference is emitted verbatim.
///
/// The class is `[A-Za-z0-9_]` rather than `\w` because JavaScript's `\w` is ASCII-only and Rust's
/// `regex` makes it Unicode-aware: `${café}` must stay literal and `$env:café` must expand `caf`
/// and leave `é`.
fn expand_pattern(
    input: &str,
    open: &str,
    close: Option<&str>,
    lookup: impl Fn(&str) -> Option<String> + Copy,
) -> String {
    /* body of mcp_direct_tools.rs:1315-1347, with `env(name)` -> `lookup(name)` */
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
```

**Write the third pass as the delimited form, not as a copy of `interpolate_dollar_env`.** The `$env:`
scanner stops at the first non-word byte and does not require a terminator; applying that shape to
`{env:` would expand `{env:café}` as `caf` where the JS regex leaves it literal. The delimited arm
finds the `}` first and validates the *whole* name, which is what `("{env:café}", "{env:café}")` at
[mcp_direct_tools.rs:2604](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs) pins.

Delete `is_word_byte` (`:152-154`) — `expand_pattern` is `char`-based and nothing else uses it.
Promote `interpolate_env_vars` (`:139`) from `pub(crate)` to `pub`. `resolve_config_path_with`
(`:225-240`) already passes an `impl Fn(&str) -> Option<String> + Copy` and needs no change.

Then delete the two duplicate bodies and delegate:

* [credentials.rs:3323-3351](../../crates/cyrup-mcp/src/credentials.rs) —
  ```rust
  pub fn interpolate_env_vars_with<F>(value: &str, lookup: F) -> String
  where
      F: Fn(&str) -> Option<String>,
  {
      // `&F` is itself `Fn(&str) -> Option<String>` AND `Copy`, so the engine's `+ Copy` bound is
      // satisfied by reference and this signature does not change at all — no caller, no test and
      // neither re-export moves.
      cyrup_ext::caps::proc::interpolate_env_vars_with(value, &lookup)
  }
  ```
  The re-exports at [secrets.rs:83](../../crates/cyrup-mcp/src/secrets.rs) and
  [oauth.rs:104-107](../../crates/cyrup-mcp/src/oauth.rs) keep resolving unchanged, as do the callers
  at `credentials.rs:3307` and the vectors at
  [oauth.rs:4001-4015](../../crates/cyrup-mcp/src/oauth.rs). **Keep the `regex`/`LazyLock` imports** —
  `KEY_REVOKED_PATTERN` (`credentials.rs:1262`) and `missing_env_vars` (`:3428`) still use both. Update
  `secrets.rs:56-69`'s doc, which still says both cyrup copies are missing the third form.
* [mcp_direct_tools.rs:1300-1348](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs) —
  ```rust
  fn interpolate_env_vars(value: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
      cyrup_ext::caps::proc::interpolate_env_vars_with(value, env)
  }
  ```
  deleting `expand_pattern`. `&dyn Fn(&str) -> Option<String>` is `Copy` and implements the trait, so
  it satisfies the bound directly. **Keep `match_placeholder` (`:1043-1059`), `missing_env_vars`
  (`:1020-1035`) and `is_word_char` (`:1350-1352`)** — those are `getMissingEnvVars`, a scanner with a
  deliberately different pass structure, and the file's own doc at `:1005-1019` explains why they must
  not be merged.

**Why the swap is behaviour-preserving in `cyrup-mcp`.** The regex `\$\{([A-Za-z0-9_]+)\}` requires the
`}` immediately after a maximal word run; the scanner finds the first `}` and requires everything up to
it to be a word char. `\w` never matches `}`, so the two describe the same language — `${A B}`,
`${}`, `${FOO`, `${A}B}` and `${${A}}` all resolve identically under both. That equivalence is what
makes replacing the regex implementation with the scanner a consolidation rather than a change.

Note what MCP-342 does *not* touch: the stdio env of an MCP server already goes through the three-form
`cyrup-mcp` implementation via `secrets::resolve_stdio_env`
([secrets.rs:380-391](../../crates/cyrup-mcp/src/secrets.rs)). The two-form copy affects the WASM-guest
`ProcCaps` env and `cwd` only. Do not conflate them while editing.

### Step 8 — `execOpen` / `openUrl` and the `$BROWSER` wiring (MCP-086)

In [ui.rs](../../crates/cyrup-mcp/src/ui.rs), beside `open_path` (`:3070`), add the shared dispatch and
refactor `open_path` onto it. The dispatch is parameterised by an explicit platform value rather than
by `#[cfg]` alone, so all seven rows of the table are one function of its inputs on any host — the same
shape upstream's own test takes by mocking `platform()`
([`__tests__/utils-exec-open.test.ts:6-9`](../../tmp/pi-mcp-adapter/__tests__/utils-exec-open.test.ts)).

```rust
/// `platform()`'s three arms (`utils.ts:8`), lifted to a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenPlatform {
    /// `os === "darwin"`.
    Darwin,
    /// `os === "win32"`.
    Win32,
    /// Everything `execOpen`'s trailing `return` covers.
    Other,
}

impl OpenPlatform {
    /// The platform this binary was compiled for.
    #[must_use]
    pub const fn host() -> Self {
        if cfg!(target_os = "macos") {
            Self::Darwin
        } else if cfg!(target_os = "windows") {
            Self::Win32
        } else {
            Self::Other
        }
    }
}

/// `extname(browser).toLowerCase() === ".app"` (`utils.ts:14`).
fn has_app_extension(browser: &str) -> bool {
    Path::new(browser)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

/// `execOpen`'s argv table (`utils.ts:8-27`), as a pure function.
fn exec_open_argv(
    platform: OpenPlatform,
    target: &str,
    browser: Option<&str>,
) -> (String, Vec<String>) {
    let target = target.to_string();
    match (platform, browser) {
        // `isAbsolute(browser) && extname(browser).toLowerCase() !== ".app"` — an absolute
        // executable path names a launcher binary; an app bundle still goes through `open -a`,
        // which knows how to open one. `isAbsolute` is node's POSIX one on darwin, i.e. a leading
        // `/`, spelled literally rather than as `Path::is_absolute` so this row answers the same
        // on every host.
        (OpenPlatform::Darwin, Some(browser))
            if browser.starts_with('/') && !has_app_extension(browser) =>
        {
            (browser.to_string(), vec![target])
        }
        (OpenPlatform::Darwin, Some(browser)) => {
            ("open".to_string(), vec!["-a".to_string(), browser.to_string(), target])
        }
        (OpenPlatform::Darwin, None) => ("open".to_string(), vec![target]),
        // The empty string is `start`'s window-title argument and is NOT optional: without it
        // `start` reads the next quoted argument as the title and opens nothing.
        (OpenPlatform::Win32, browser) => {
            let mut args = vec!["/c".to_string(), "start".to_string(), String::new()];
            args.extend(browser.map(str::to_string));
            args.push(target);
            ("cmd".to_string(), args)
        }
        (OpenPlatform::Other, Some(browser)) => (browser.to_string(), vec![target]),
        (OpenPlatform::Other, None) => ("xdg-open".to_string(), vec![target]),
    }
}

/// `execOpen(pi, target, browser?, signal?)` (`utils.ts:7-28`) — the spawn shared by [`open_path`]
/// and [`open_url`].
///
/// Spawned with `tokio::process::Command` rather than `HostServices::exec`, which
/// `13b-mcp-config.md:1476` names: that trait is the WASM-guest capability model, `cyrup-mcp` is a
/// native built-in crate (13g's own refutation at `13g-mcp-oauth.md:1684`), `open_path` and
/// `secrets::resolve_command_secret` both already spawn directly, and the cancel token 13b wanted
/// the trait for arrives from the caller.
///
/// `kill_on_drop` is the port of `pi.exec`'s `{ signal }` teardown: `run_until_cancelled` drops the
/// `wait_with_output` future on cancel, which drops the `Child`, which kills it. Without it tokio
/// detaches the launcher and it outlives the teardown that cancelled it. NAMED DELTA: that is a
/// SIGKILL with no graceful window, where pi's own `killProcess` escalates.
///
/// `stdin` is `null` rather than inherited — a launcher must never take the TUI's stdin. That is
/// the one behavioural change to `open_path`'s existing spawn, and it is a fix.
async fn exec_open(
    target: &str,
    browser: Option<&str>,
    cancel: Option<&cyrup_core::CancelToken>,
) -> McpResult<std::process::Output> {
    let (program, args) = exec_open_argv(OpenPlatform::host(), target, browser);
    let fail = |error: std::io::Error| McpError::other(format!("{program}: {error}"));
    let child = tokio::process::Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(fail)?;

    let wait = child.wait_with_output();
    match cancel {
        Some(token) => token
            .run_until_cancelled(wait)
            .await
            .ok_or_else(|| McpError::Aborted(crate::abort::ABORTED_FALLBACK_REASON.to_string()))?
            .map_err(fail),
        None => wait.await.map_err(fail),
    }
}

/// `` result.stderr || `Failed to open {what} (exit code ${result.code})` `` — stderr wins whenever
/// it is non-empty, which is why the exit code is only ever formatted on the empty-stderr arm.
fn open_failure_text(what: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let code = output.status.code().map_or_else(|| "unknown".to_string(), |code| code.to_string());
    format!("Failed to open {what} (exit code {code})")
}

/// `openUrl(pi, url, browser?, signal?)` (`utils.ts:30-35`) — MCP-086.
///
/// # Errors
///
/// [`McpError::Aborted`] when `cancel` fires before the launcher exits; otherwise
/// [`McpError::Other`] carrying the launcher's stderr, or `Failed to open browser (exit code N)`.
pub async fn open_url(
    url: &str,
    browser: Option<&str>,
    cancel: Option<&cyrup_core::CancelToken>,
) -> McpResult<()> {
    let output = exec_open(url, browser, cancel).await?;
    if output.status.success() {
        return Ok(());
    }
    Err(McpError::other(open_failure_text("browser", &output)))
}
```

`open_path` keeps its `Result<(), String>` signature and its `Failed to open path (exit code …)` text,
and becomes upstream's "neither a browser nor a signal"
([utils.ts:37-42](../../tmp/pi-mcp-adapter/utils.ts)) — which is exactly the `(None, None)` pair:

```rust
pub async fn open_path(target: &Path) -> Result<(), String> {
    // `McpError::Other`/`Aborted` are both `#[error("{0}")]`, so this round-trips the same text
    // the old body produced.
    let output = exec_open(&target.display().to_string(), None, None)
        .await
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    Err(open_failure_text("path", &output))
}
```

Then replace the stub at [runtime.rs:229-241](../../crates/cyrup-mcp/src/runtime.rs):

```rust
// Steps 8-9. `openBrowser: async (url) => { owner.throwIfInactive();
// await openUrl(pi, url, process.env.BROWSER, owner.signal); owner.throwIfInactive(); }`
// (`init.ts:175-179`).
let open_browser: OpenBrowser = {
    let owner = Arc::clone(&owner);
    Arc::new(move |url: String| {
        let owner = Arc::clone(&owner);
        Box::pin(async move {
            // Guarded on BOTH sides of the await, as upstream is (13a §8 step 9).
            owner.throw_if_inactive()?;
            // `process.env.BROWSER` is read INSIDE the closure upstream, so it is read per call
            // here too — a user who exports it after the runtime started still gets it. An empty
            // value is falsy to `execOpen`'s `if (browser)` and must arrive as `None`.
            let browser = std::env::var("BROWSER").ok().filter(|value| !value.is_empty());
            let token = owner.token();
            let result = crate::ui::open_url(&url, browser.as_deref(), Some(&token)).await;
            owner.throw_if_inactive()?;
            result
        })
    })
};
```

The both-sides `throw_if_inactive` guards are already correct and stay; only the middle line changes.
Update `state.rs:53-55`'s doc, which says the closure "closes over the owner, the host `exec` handle
and `$BROWSER`" — there is no host `exec` handle. Leave
[`OpenerLauncher`](../../crates/cyrup-mcp/src/oauth.rs) (`oauth.rs:2393-2400`) untouched.

---

## Definition of Done

Every line below is checkable by reading the tree or by a single grep; none requires running a suite.
`cargo check --workspace --all-targets` and `cargo doc --workspace --no-deps --bins` must both still
exit 0, and `cargo nextest run --workspace` must not lose any of the 7862 passing tests.

**Cache identity and migration — MCP-104 + MCP-106 (land together)**

- [ ] `CACHE_VERSION` is `2` in [npx_resolver.rs](../../crates/cyrup-ext/src/caps/proc/npx_resolver.rs);
      `CACHE_TTL_MS` (24 h) and `FORCE_CACHE_TIMEOUT` (30 s) are unchanged.
- [ ] `cache_key` takes `(command: &str, package_spec: &str, bin_name: &str)`, serialises exactly those
      three, and its only call site passes `&parsed.package_spec` and
      `parsed.bin_name.as_deref().unwrap_or("")` — no `args` reaches it. `grep -n 'cache_key(' `
      shows one definition and one call, both three-argument.
- [ ] `clear_legacy_cache_at` exists, evicts only when `version` compares equal to `1` **as a JSON
      number**, does `remove_file` falling back to `fs::write(path, "")`, returns `true`, and is called
      at the head of `load_cache_at` — with **no** `std::sync::Once` anywhere in the module.
- [ ] `load_cache_at` returns `None` without further reads when the eviction fired, and a file whose
      `version` is `3` or non-numeric is left on disk.
- [ ] `save_cache_entry_at` reads through `read_npx_cache_payload` + `to_npx_cache`, never through
      `load_cache_at`; `SAVE_CACHE_LOCK` and the tmp-file-rename cycle are untouched.

**Per-entry validation and Windows — MCP-108**

- [ ] `read_npx_cache_payload`, `to_npx_cache_entry` and `to_npx_cache` exist; `NpxCache` is no longer
      produced by a whole-file `serde_json::from_str::<NpxCache>`, so a file with one valid and one
      malformed entry yields the valid one rather than `None`.
- [ ] `to_npx_cache_entry` rejects a non-string `resolvedBin`, a non-number or non-finite `resolvedAt`,
      a non-boolean `isJs` and a present-but-non-string `packageVersion`, and treats an absent or
      `null` `packageVersion` as absent.
- [ ] `Command::new("npm")` appears nowhere in the module; both former sites go through `npm_command`,
      which is bare `npm` off Windows and `%COMSPEC% /d /s /c "npm …"` with `CREATE_NO_WINDOW` and
      caret-escaped arguments on Windows. No PATH/PATHEXT walk was written.

**Cancellation — MCP-107**

- [ ] `resolve_npx_binary(command, args, cancel: &cyrup_core::CancelToken) ->
      Result<Option<NpxResolution>, NpxAborted>`; an already-cancelled token returns `Err` before any
      parse, filesystem read or spawn.
- [ ] `force_npx_cache(package_spec, cancel) -> Result<(), NpxAborted>` checks the token on entry, on
      every 50 ms tick (killing **and** reaping before returning `Err`), and once more on exit.
- [ ] A spawn failure, a timeout or a non-zero exit from `npm exec` still yields `Ok(())` — the abort
      is the only new `Err`.

**Wiring — MCP-103**

- [ ] `cyrup_ext::caps::proc::{resolve_npx_binary, NpxResolution, NpxAborted}` are `pub`,
      `npx_resolver` is a `pub mod`, and `NpxResolution`'s three fields are `pub`.
- [ ] `NpxResolution::rewrite` is the single implementation of the `isJs` rewrite; `apply_npx_resolution`
      delegates to it and `apply_npx_resolution_matches_pi_exactly` still passes.
- [ ] `ConnectionBuilder::connect_stdio` calls `resolve_npx_binary` when and only when the configured
      command is exactly `npx` or `npm`, **after** step 2's arg interpolation and **before** step 4's
      `throw_if_aborted`, on a `spawn_blocking` task, with `request.attempt` as the token.
- [ ] On a hit the built spec's command is `node` with `[bin_path, ...extra_args]` when `is_js` and
      `bin_path` with `extra_args` otherwise; on a miss `command`/`args` are byte-identical to step 2's.
- [ ] `tracing::debug!` emits `<name> resolved to <bin_path> (skipping npm parent)` on a hit only.
- [ ] `Err(NpxAborted)` surfaces from `connect_stdio` as `McpError::Aborted`, and no child is spawned
      on that path.
- [ ] `grep -rn 'MCP-103' crates/` shows no remaining "NOT PORTED" / "not ported" / "the call site is
      marked below" prose: `runtime.rs:2404`, `runtime.rs:1859-1861` and `server_manager.rs:560-568` are
      all restated. `server_manager.rs`'s forking-grandchild residual and its test survive.

**Interpolation — MCP-342**

- [ ] `cyrup_ext::caps::proc::interpolate_env_vars_with` is `pub`, runs three chained `expand_pattern`
      passes in `${…}` → `$env:…` → `{env:…}` order, and `interpolate_env_vars` is `pub`.
- [ ] `interpolate_braces`, `interpolate_dollar_env` and `is_word_byte` are gone from
      `caps/proc.rs`; `expand_pattern` is gone from `mcp_direct_tools.rs`.
- [ ] Neither `cyrup_mcp::credentials::interpolate_env_vars_with` nor
      `cyrup_ext_subagents::exec::mcp_direct_tools::interpolate_env_vars` contains a scanner or a regex
      of its own — each is a one-line delegation — and neither one's public signature changed, so no
      caller, re-export or existing vector was edited.
- [ ] `match_placeholder`, `missing_env_vars` and `is_word_char` in `mcp_direct_tools.rs`, and
      `missing_env_vars` and `KEY_REVOKED_PATTERN` in `credentials.rs`, are untouched, and
      `credentials.rs` still imports `Regex`/`LazyLock`.
- [ ] `ProcCaps::spawn`'s `cmd.env(k, interpolate_env_vars(v))` and `resolve_config_path_with` now both
      reach all three forms; `secrets.rs:56-69`'s "both existing cyrup copies are missing" paragraph is
      restated.

**Browser — MCP-086**

- [ ] `OpenPlatform`, `exec_open_argv`, `exec_open`, `open_failure_text` and `open_url` exist in
      `ui.rs`; `open_path` shares the dispatch and keeps its signature and its two error strings.
- [ ] `exec_open_argv` reproduces all seven rows: darwin `/`-prefixed non-`.app` browser →
      `[browser, target]`; darwin `.app` (case-insensitive) or non-`/` browser →
      `open -a <browser> <target>`; darwin unset → `open <target>`; win32 set →
      `cmd /c start "" <browser> <target>`; win32 unset → `cmd /c start "" <target>`; other set →
      `[browser, target]`; other unset → `xdg-open <target>`.
- [ ] Neither `open_url` nor `open_path` routes through `HostServices::exec` or `opener`, and
      `kill_on_drop(true)` is set so a cancelled open leaves no launcher running.
- [ ] `state.open_browser` passes its `url` to `open_url` with `$BROWSER` (empty string treated as
      unset, read per call) and `owner.token()`, keeps `throw_if_inactive` on both sides of the await,
      and no longer binds its argument as `_url`.
- [ ] `grep -rn 'BROWSER' crates/cyrup-mcp/src` now finds a real `std::env::var("BROWSER")` read, and
      `state.rs:53-55`'s "host `exec` handle" doc is restated.
- [ ] `OpenerLauncher` still calls `opener::open` and is unchanged.
