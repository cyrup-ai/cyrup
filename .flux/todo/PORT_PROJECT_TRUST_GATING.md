---
stage: exec
status: done
updated: 2026-08-27 05:13
---
# Gate project-scoped policy on project trust

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.1** and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages). Reference checkout:
> [`tmp/pi-packages/packages/pi-permission-system`](../../tmp/pi-packages/packages/pi-permission-system).
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./_backlog/UPSTREAM_PARITY_INDEX.md).

| | |
| --- | --- |
| **Severity** | critical |
| **Kind** | absent |
| **Upstream area** | handlers: session lifecycle |
| **Verification** | Adversarially confirmed; re-verified during this augmentation against upstream v27.0.1 and the live port |

---

## The gap

Opening an **untrusted** repository lets that repository's checked-in
`<cwd>/.cyrup/agent/cyrup-permissions.jsonc` take effect immediately. The project layer is
untrusted in the merge, so a trusted global **deny** still floors it
([`manager.rs:739-748`](../../crates/cyrup-permission-system/src/manager.rs)) — but the project file
can still **ADD allow rules** for anything the global policy does not explicitly deny, turning an
`ask` into a silent auto-allow before the human has granted trust. Nothing announces the reduced or
widened scope either way.

[`extension/paths.rs:51-69`](../../crates/cyrup-permission-system/src/extension/paths.rs) sets
`project_global_config_path` / `project_agents_dir` from `cwd` unconditionally, and
[`extension/config.rs:55-62`](../../crates/cyrup-permission-system/src/extension/config.rs)
(`refresh_config_and_manager`) rebuilds from `ctx.cwd` with no trust argument. `rg -n
"is_project_trusted|project_trusted"` over the crate returns nothing.

---

## Research findings (this augmentation)

### Upstream shape — verified at v27.0.1

| file | role |
| --- | --- |
| [`handlers/lifecycle.ts:54-60`](../../tmp/pi-packages/packages/pi-permission-system/src/handlers/lifecycle.ts) | `handleSessionStart` reads `ctx.isProjectTrusted()` once, passes it to `refreshConfig` + `resetForNewSession`, then warns |
| `handlers/lifecycle.ts:92-96` | `handleResourcesDiscover` reload path does the same via `session.reload(projectTrusted)` |
| `handlers/lifecycle.ts:24-27` | `UNTRUSTED_PROJECT_MESSAGE` |
| `handlers/lifecycle.ts:109-115` | `warnProjectUntrusted` — `logger.review("project_trust.skipped", { cwd, phase })` **then** `logger.warn(MESSAGE)` |
| [`permission-session.ts:106-110, :132-136`](../../tmp/pi-packages/packages/pi-permission-system/src/permission-session.ts) | `configureForCwd(projectTrusted ? ctx.cwd : undefined)` |

**The mechanism upstream uses is withholding the cwd, not a flag on the manager.**
`configureForCwd(undefined)` derives no project paths. That maps exactly onto this port, because
`manager_paths_for`'s `cwd` parameter is used for *nothing but* the project paths
(`paths.rs:52`) — so `Option<&Path>` is the precise Rust analog of `projectTrusted ? cwd :
undefined`, not an approximation.

`config-store.ts:99-113` (`includeProjectScope`) is the **config** half of upstream's #644 and does
**not** apply here: `rg -n project src/ext_config.rs` returns nothing, so this port has no
project-scope merge for the extension config at all and `yolo_mode` is global-only — safe by
accident. Scope this task to the **policy** half only.

### The `HostCtxRich::default()` trap — resolved, and the resolution is exact

The index warns that gating naively trades a silent widening for a silent narrowing.
[`cyrup-ext/src/native.rs:708-725`](../../crates/cyrup-ext/src/native.rs) documents why:
`ctx_source: Option<Arc<dyn HostCtxSource>>`, and `dispatch_ctx()` (`:822-827`) falls back to a
ctx carrying `HostCtxRich::default()` when it is `None` — where `is_project_trusted = false`. So
`ctx.is_project_trusted()` reads `false` both for a genuinely untrusted project **and** for a host
that attached no source. Gating on it alone would withhold project policy from every such host.

`HostCtx` exposes no liveness accessor, so the flag cannot be disambiguated on its own. **It does
not need to be**, for this crate specifically:

1. `NativeExtension::set_host_services` is `#[cfg(feature = "wasm-host")]`
   ([`native.rs:682-683`](../../crates/cyrup-ext/src/native.rs)) and `pub mod host` is gated the
   same way (`cyrup-ext/src/lib.rs:149-150`). This crate holds
   `host_services: Arc<OnceLock<Arc<dyn HostServices>>>`
   ([`extension/mod.rs:152`](../../crates/cyrup-permission-system/src/extension/mod.rs))
   **unconditionally**, so it cannot compile without `cyrup-ext/wasm-host`. The
   `--no-default-features` configuration the trap describes is unreachable *here*; it is a hazard
   for cyrup-ext's other consumers.
2. On the arm this crate can be built on,
   [`facade.rs:354-366`](../../crates/cyrup-ext/src/facade.rs) `load_native_with_services` sets
   **both** in one body: `ext.set_host_services(services.clone())` then
   `self.set_ctx_source(Arc::new(ServicesCtxSource(services)))`.
3. That is the production path — [`cyrup-session-svc/src/builder.rs:1010`](../../crates/cyrup-session-svc/src/builder.rs)
   loads every native built-in through it.

So **`host_services.get().is_some()` ⟺ a `HostCtxSource` is attached ⟺ `ctx.is_project_trusted()`
is a live answer.** Exact for this crate, not a heuristic.

This is also already the crate's documented convention for exactly this question.
[`extension/warnings.rs:39-46`](../../crates/cyrup-permission-system/src/extension/warnings.rs)
spells it out: *"'is a host backend attached at all' is `host_services.get()`, which is the direct
analog of pi's `runtimeContext != null`"*. `sync_status_when_possible`
(`extension/config.rs:165-169`) uses the same test. Follow it; do not invent a second one.

### What already exists and must be reused

- **`ManagerPaths` project fields are already `Option<PathBuf>`** (`manager.rs:105-106`), and the
  load paths already handle `None`: `manager.rs:541` (`let Some(path) = … else`) and `:566`
  (`project_agents_dir.as_deref()`). Passing `None` is a supported existing state — `manager.rs:1155-1156`
  already constructs that way. **No manager change is needed.**
- **The review stream exists**: `write_review_entry`
  ([`extension/audit.rs:48-50`](../../crates/cyrup-permission-system/src/extension/audit.rs)) is the
  port of pi `writeReviewEntry`. Upstream's `logger.review("project_trust.skipped", …)` ports 1:1.
- **The warn stream exists**: `self.warnings.notify(&str)` (`WarningSink::notify`), the port of pi
  `notifyWarning`, already dedups per session and resets on session start / reload.

---

## Implementation

### 1. `src/extension/paths.rs` — withhold the project scope at the source

Change `manager_paths_for`'s `cwd` to `Option<&Path>`. This is upstream's `configureForCwd(trusted
? cwd : undefined)` expressed in the type, so no call site can forget the decision.

```rust
    /// Derive the [`ManagerPaths`] for `agent_dir` + `project_cwd` (pi
    /// `createPermissionManagerForCwd`'s path derivation, `index.ts:1536-1573`).
    ///
    /// `project_cwd` is `None` when the project scope must be withheld — pi
    /// `permissionManager.configureForCwd(projectTrusted ? ctx.cwd : undefined)`
    /// (`permission-session.ts:106-110`, `:132-136`, #644). The parameter is an `Option` rather
    /// than a companion `bool` because `cwd` is read for NOTHING ELSE here: withholding it IS
    /// withholding the project scope, so the two cannot drift apart.
    pub(super) fn manager_paths_for(agent_dir: &Path, project_cwd: Option<&Path>) -> ManagerPaths {
        let project_dir = project_cwd
            .map(|cwd| PROJECT_AGENT_SUBDIR.iter().fold(cwd.to_path_buf(), |acc, seg| acc.join(seg)));
        let policy_dir = policy_agent_dir(agent_dir);
        ManagerPaths {
            global_config_path: policy_dir.join(POLICY_FILE),
            agents_dir: policy_dir.join("agents"),
            project_global_config_path: project_dir.as_ref().map(|d| d.join(POLICY_FILE)),
            project_agents_dir: project_dir.map(|d| d.join("agents")),
            legacy_global_settings_path: policy_dir.join("settings.json"),
            global_mcp_config_path: policy_dir.join("mcp.json"),
            mcp_server_names_override: None,
        }
    }
```

Keep the existing PERM-025 comment about the four global artifacts hanging off
`policy_agent_dir()` — it is still true and still load-bearing.

### 2. `src/extension/mod.rs` (or wherever the impl block sits) — the one trust decision

Add a single private helper next to `refresh_config_and_manager`, so both handler arms and any
future one ask the same question the same way.

```rust
    /// pi `ctx.isProjectTrusted()` (`handlers/lifecycle.ts:54`, `:92`), guarded by whether the
    /// answer is real.
    ///
    /// **\[CYRUP-DELTA]** pi's `ExtensionContext` always carries a populated `isProjectTrusted`.
    /// cyrup's [`HostCtx`] carries [`HostCtxRich::default()`] — `is_project_trusted = false` —
    /// when no [`HostCtxSource`] is attached (`cyrup-ext/src/native.rs:708-725`,
    /// `dispatch_ctx` `:822-827`), and exposes no accessor to tell that apart from a genuine
    /// `false`. Gating on the raw flag would withhold project policy from every host that never
    /// attached one, trading upstream's silent widening for a silent narrowing.
    ///
    /// `host_services.get()` resolves it exactly for THIS crate, and is the same test
    /// [`WarningSink::notify`] and [`Self::sync_status_when_possible`] already use for "is a host
    /// backend attached at all" (pi's `runtimeContext != null`). It is exact rather than a
    /// heuristic because: this crate holds `Arc<dyn HostServices>` unconditionally while
    /// `cyrup-ext`'s `host` module is `cfg(feature = "wasm-host")`, so it cannot build on the arm
    /// where the two could diverge; and on the arm it does build,
    /// `ExtensionHost::load_native_with_services` (`cyrup-ext/src/facade.rs:354-366`) sets the
    /// backend and the ctx source in one body — the path `cyrup-session-svc/src/builder.rs:1010`
    /// takes for every native built-in.
    ///
    /// With no backend attached the project scope is KEPT, preserving today's behaviour: a host
    /// that supplies no trust signal has not said the project is untrusted.
    fn project_trusted(&self, ctx: &HostCtx) -> bool {
        match self.host_services.get() {
            Some(_) => ctx.is_project_trusted(),
            None => true,
        }
    }
```

### 3. `src/extension/config.rs` — thread the decision through the refresh

```rust
    pub(super) fn refresh_config_and_manager(&self, project_cwd: Option<&Path>) {
        // pi order (`refreshSessionRuntimeState`, v0.8.0 `index.ts:1819-1826`): config first,
        // manager second, agent-start cache invalidated third.
        self.refresh_extension_config();
        *guard(&self.manager) = manager_with_warnings(
            Self::manager_paths_for(&self.agent_dir, project_cwd),
            &self.warnings,
        );
        self.invalidate_agent_start_cache();
    }
```

Add to its doc comment: *"`project_cwd` is `None` when project trust is withheld — pi
`configureForCwd(projectTrusted ? ctx.cwd : undefined)` (#644). A later reload that finds the
project trusted re-includes the scope, and one that finds it untrusted drops it again; the manager
is rebuilt from scratch each time, so trust is re-evaluated per session start and per reload exactly
as upstream does."*

### 4. `src/extension/native.rs` — both handler arms

Upstream reads trust **once per handler** and uses it for both the refresh and the warning
(`lifecycle.ts:54-60`, `:92-96`). Mirror that; do not call `project_trusted` twice.

`SessionStart` arm, at the existing `self.refresh_config_and_manager(&ctx.cwd);` (`:195`):

```rust
                // pi `handleSessionStart` (`handlers/gates/../lifecycle.ts:54-60`): read trust ONCE,
                // use it for the refresh and the warning both.
                let project_trusted = self.project_trusted(ctx);
                self.refresh_config_and_manager(project_trusted.then(|| ctx.cwd.as_path()));
                if !project_trusted {
                    self.warn_project_untrusted(&ctx.cwd, "session_start");
                }
```

`ResourcesDiscover` reload arm, at `:249`:

```rust
                let project_trusted = self.project_trusted(ctx);
                self.refresh_config_and_manager(project_trusted.then(|| ctx.cwd.as_path()));
                if !project_trusted {
                    self.warn_project_untrusted(&ctx.cwd, "resources_discover");
                }
```

Upstream orders the warning **after** the refresh in both handlers; keep that order so the review
entry cannot claim a scope the manager has not actually been rebuilt with.

### 5. The announcement — port `warnProjectUntrusted` verbatim

Beside `write_review_entry` in `src/extension/audit.rs`, or next to the new `project_trusted`
helper:

```rust
/// pi `UNTRUSTED_PROJECT_MESSAGE` (`handlers/lifecycle.ts:24-27`), with pi's package name
/// swapped for cyrup's — the string is operator-facing, and naming the wrong extension in it
/// would send them to the wrong place to grant trust.
const UNTRUSTED_PROJECT_MESSAGE: &str = "cyrup-permission-system: project is not trusted — \
    skipping project-scoped permission configuration. Only global policy applies. Grant project \
    trust to load this project's permission rules.";

    /// pi `warnProjectUntrusted` (`handlers/lifecycle.ts:109-115`): record the skip in the review
    /// stream and surface a loud warning, so the reduced (global-only) scope is never silent
    /// (#644). Review entry FIRST, then the notification — pi's order, and the useful one: the
    /// durable trail is written even if the notify sink drops it.
    fn warn_project_untrusted(&self, cwd: &Path, phase: &str) {
        self.write_review_entry(
            "project_trust.skipped",
            &json!({ "cwd": cwd.to_string_lossy(), "phase": phase }),
        );
        self.warnings.notify(UNTRUSTED_PROJECT_MESSAGE);
    }
```

`phase` is `"session_start"` or `"resources_discover"`, matching upstream's union exactly.
`WarningSink::notify` already dedups per session and is reset by the `self.warnings.reset()` both
arms already call, so a reload in a still-untrusted project re-announces — which is upstream's
behaviour, since `resetShownWarnings()` runs first in its reload branch too.

### 6. Constructors — `src/extension/construct.rs:45, :73, :103`

All three take a `cwd` and run **before** any host backend or ctx exists, so there is no trust
signal to consult. Pass `Some(&cwd)` — preserving today's behaviour — and let the first
`session_start` apply the gate. This mirrors upstream, where the manager is constructed and only
then `resetForNewSession` calls `configureForCwd`.

Give one of them a short comment saying so, so the next reader does not think the gate was
forgotten here:

```rust
        // Construction precedes any attached backend or ctx, so there is no trust answer yet
        // (pi constructs the manager, then `resetForNewSession` calls `configureForCwd` —
        // `permission-session.ts:106-110`). The first `session_start` applies the gate.
        let paths = Self::manager_paths_for(&agent_dir, Some(cwd.as_path()));
```

Also update the two `manager_paths_for` call sites in
`src/extension/tests/install.rs:117, :125` to the new signature — they are existing tests, not new
ones, and will not compile otherwise.

---

## Out of scope — do not absorb

| Deferred | Why |
| --- | --- |
| The **config** half of upstream #644 (`config-store.ts:99-113`, `includeProjectScope`) | This port has no project-scope merge for the extension config at all (`rg -n project src/ext_config.rs` → nothing); `yolo_mode` is global-only and safe by accident. There is nothing to gate. |
| Adding a liveness accessor to `cyrup-ext`'s `HostCtx` | Would be the cleaner fix in the abstract, but it is a change to another crate for a hazard this crate cannot reach (it cannot build without `wasm-host`). If cyrup-ext ever grows one, this helper collapses to using it — say so in its doc, which the code above does. |
| `PORT_RESOLVED_CONFIG_PATH_AUDIT` (`logResolvedConfigPaths`, `lifecycle.ts:57`) | A separate backlog task; it happens to sit between the two calls upstream, but it is its own finding. |

---

## Definition of done

1. `manager_paths_for` takes `Option<&Path>` and derives **no** project paths from `None`.
2. Both the `SessionStart` and the `ResourcesDiscover`-reload arms read trust once via
   `project_trusted(ctx)`, pass it into `refresh_config_and_manager`, and warn when false.
3. With a host backend attached and `is_project_trusted() == false`, a `ManagerPaths` built for the
   session has `project_global_config_path == None` **and** `project_agents_dir == None`, so a
   `<cwd>/.cyrup/agent/cyrup-permissions.jsonc` granting `{"bash": {"curl *": "allow"}}` does **not**
   take effect; with trust true it does.
4. With **no** backend attached the project scope is still loaded — the raw
   `is_project_trusted = false` default must not narrow anything.
5. An untrusted session writes a `project_trust.skipped` review entry carrying `cwd` and `phase`,
   and notifies `UNTRUSTED_PROJECT_MESSAGE` once per session.
6. Deliberate divergences carry `[CYRUP-DELTA]` with reasoning — at minimum the
   `host_services.get()` liveness key, which is this task's whole design decision.
7. `cargo check -p cyrup-permission-system --all-targets` and
   `cargo clippy -p cyrup-permission-system --all-targets` are clean, and the existing suite still
   passes. `unwrap_used`, `expect_used`, `panic` and `indexing_slicing` are `deny` at the workspace
   root (`Cargo.toml:97-101`).

/home/user/cyrup/.flux/todo/PORT_PROJECT_TRUST_GATING.md
