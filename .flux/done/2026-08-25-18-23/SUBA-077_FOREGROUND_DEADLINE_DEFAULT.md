---
stage: qa
status: completed
updated: 2026-08-28 11:15
severity: high
effort: small
subsystem: exec / deadlines
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-077
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-077 — A foreground subagent run with no explicit timeout has NO wall-clock deadline, and there is no global `config.timeoutMs`

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** foreground execution / deadlines
**Window** in-baseline (≤ v0.43.0) for the 30-minute foreground default; **v0.47.1..v0.57.0** for the `config.timeoutMs` rung.

**upstream** — `git show v0.57.0:src/runs/foreground/subagent-executor.ts`:

```js
export const DEFAULT_FOREGROUND_TIMEOUT_MS = 30 * 60 * 1000;                       // :2656
const MAX_TIMER_DELAY_MS = 2_147_483_647;                                          // :2675

export function resolveConfigDefaultTimeoutMs(raw: unknown): number | undefined {  // :2684
    if (typeof raw !== "number" || !Number.isInteger(raw) || raw <= 0 || raw > MAX_TIMER_DELAY_MS) return undefined;
    return raw;
}

export function resolveForegroundTimeout(params, defaultTimeoutMs?) {              // :2689
    const rawTimeout = params.timeoutMs, rawMaxRuntime = params.maxRuntimeMs;
    if (rawTimeout === undefined && rawMaxRuntime === undefined) {                 // the default is an
        return defaultTimeoutMs === undefined ? {} : { timeoutMs: defaultTimeoutMs };  // EARLY RETURN
    }
    /* positivity + alias-agreement validation on whichever was supplied */
}

export function resolveSingleAgentLaunchTimeout(params, async, configDefaultTimeoutMs?) {  // :2719
    const isComposite = (params.chain?.length ?? 0) > 0 || (params.tasks?.length ?? 0) > 0 || params.workflowScript !== undefined;
    const foregroundDefault   = configDefaultTimeoutMs ?? DEFAULT_FOREGROUND_TIMEOUT_MS;
    const asyncSingleDefault  = configDefaultTimeoutMs ?? DEFAULT_ASYNC_TIMEOUT_MS;
    const defaultTimeoutMs = !async ? foregroundDefault : isComposite ? undefined : asyncSingleDefault;
    return resolveForegroundTimeout(params, defaultTimeoutMs);
}
```

The agent's own frontmatter `defaultTimeoutMs` never reaches `resolveForegroundTimeout` as a
separate rung upstream — `applySingleAgentLaunchDefaults` folds it into `params.timeoutMs` first, so
by the time the ladder runs the precedence is already **explicit(+agent) > config > built-in**.

**cyrup** — [`extension/tool/params.rs:264`](../../../crates/cyrup-ext-subagents/src/extension/tool/params.rs)
`resolve_foreground_timeout` validates and returns `Ok(p.timeout_ms.or(p.max_runtime_ms))` — no
default. `SubagentExtensionConfig` ([`registration/mod.rs:79`](../../../crates/cyrup-ext-subagents/src/registration/mod.rs))
has no `timeout_ms`. The foreground message itself is already correct:
`format_timeout_message` (`exec/mod.rs:184`) is `Subagent timed out after {ms}ms.`

---

## Corrections to the item as filed

Six things this pass found. The first three change the shape of the fix; the item as written would
introduce a new dead-code bug and leave two of the four affected surfaces untouched.

**(1) The Fix line's ladder would make the agent rung DEAD.** It prescribes
`p.timeout_ms.or(p.max_runtime_ms).or(agent_default).or(config_default).or(Some(DEFAULT))` *inside*
`resolve_foreground_timeout`. But `routing.rs:320` already applies the agent rung at the CALL SITE:

```rust
        let timeout_ms = resolve_foreground_timeout(p)
            .map_err(ToolError::new)?
            .or(launch_defaults.1);          // <- the agent's frontmatter timeoutMs
```

Put a default inside the function and this `.or()` can never fire again — the function will have
already returned the config/built-in value, and an agent's `timeoutMs:` frontmatter becomes
unreachable. **The existing `.or(launch_defaults.1)` must be REMOVED and folded into the default
argument**, not left beside it.

**(2) There are FOUR foreground surfaces, not "all call sites in `routing.rs`".**

| surface | today |
|---|---|
| `routing.rs:320` single | `explicit.or(agent)` — no default |
| `routing.rs:1567` chain | `explicit` only |
| `routing.rs:1443-1445` parallel | hard-coded `None`, with a comment calling timeout wiring "a separate unit" — **this task is that unit**; an explicit call-site `timeoutMs` is dropped outright today |
| [`host/slash.rs:389`](../../../crates/cyrup-ext-subagents/src/extension/host/slash.rs) `/run` | `agent` only — the same unbounded hole, reached by a different entry point |

`resolve.rs:442` is a test, not a surface.

**(3) `route_single` serves BOTH foreground and async, so the backstop must be gated.** It decides
`p.is_background(&cfg, depth)` *after* the timeout is resolved. The async single path ALREADY applies
its own default downstream —
[`executor/background.rs:314`](../../../crates/cyrup-ext-subagents/src/extension/executor/background.rs)
`timeout_ms.unwrap_or(DEFAULT_ASYNC_CHILD_TIMEOUT_MS)`. Applying an ungated foreground default in
`route_single` would hand that `unwrap_or` a `Some` on every async run, silently retiring
`DEFAULT_ASYNC_CHILD_TIMEOUT_MS`: harmless today (both constants are `30 * 60 * 1000`) and a live
trap the moment either moves. Gate on `!background`, which is upstream's own `!async` arm.

**(4) The item omits upstream's `MAX_TIMER_DELAY_MS` ceiling, and it is load-bearing HERE too.**
`resolveConfigDefaultTimeoutMs` rejects `> 2_147_483_647` as well as non-positive. Upstream's stated
reason is a Node `setTimeout` overflow, which Rust does not share — but cyrup arms its deadline as
`Instant::now() + Duration::from_millis(ms)` (`foreground.rs:509`, `chain.rs:168`), and
`Instant + Duration` **panics** on overflow. So the bound both keeps the same settings file behaving
identically in both ports and stops a config value from panicking a run. Port it.

**(5) The config key must be carried RAW, not typed.** `resolveConfigDefaultTimeoutMs` takes
`unknown` and returns `undefined` for anything invalid — it never errors. A typed
`timeout_ms: Option<u64>` field would make `"timeoutMs": -5` (or `1.5`, or `"abc"`) fail
deserialization of the WHOLE `SubagentExtensionConfig`, taking every other setting down with it.
That is the opposite of upstream, and the crate already has the pattern and the rationale written
down for `turn_budget` (`registration/mod.rs:224-230`): *"Carried RAW rather than pre-resolved
because upstream validates it at USE time … does not take the whole extension down at load."*
Follow it: `Option<serde_json::Value>`.

**(6) `route_single` binds `cfg` AFTER the timeout resolution** (`let cfg = …config_snapshot()` sits
below line 320). Both (3) and the config rung need it earlier. Moving that binding up is safe — it
is an unconditional snapshot with no dependency on anything between.

---

## What already exists — REUSE, do not re-port

| need | already present |
|---|---|
| the timeout message | `format_timeout_message` (`exec/mod.rs:184`) — `Subagent timed out after {ms}ms.`, already 1:1 |
| the deadline arming | `foreground.rs:509` and `chain.rs:168`, both `timeout_ms.map(\|ms\| Instant::now() + Duration::from_millis(ms))` — a `Some` is all either needs |
| the agent frontmatter rung | `SubagentExecutor::single_agent_launch_defaults` returns `(default_async, default_timeout_ms, default_turn_budget)`; `.1` is the rung, already resolved at `routing.rs:312` and `slash.rs` |
| the raw-config pattern | `turn_budget` / `permissions` are `Option<serde_json::Value>` on `SubagentExtensionConfig`, validated at use — copy the shape AND the doc rationale |
| the async twin | `background::DEFAULT_ASYNC_CHILD_TIMEOUT_MS` (`background/mod.rs:57`), same `30 * 60 * 1000`, applied at `background.rs:314`. Leave it alone. |
| config wire naming | the struct is `#[serde(rename_all = "camelCase", default)]` with no `deny_unknown_fields`, so a `timeout_ms` field is `subagents.timeoutMs` on the wire with no registry to update |

---

## Required implementation

### 1. `src/exec/mod.rs` — the constant

Beside `format_timeout_message`, the foreground twin of `background/mod.rs`'s async constant:

```rust
/// pi `DEFAULT_FOREGROUND_TIMEOUT_MS` (`runs/foreground/subagent-executor.ts:2656` @v0.57.0) — the
/// wall-clock backstop every foreground launch gets when neither the caller, the agent, nor
/// `subagents.timeoutMs` set one. Without it a child whose bash tool blocks forever hangs the
/// orchestrator's turn with no signal at all.
///
/// Deliberately a SEPARATE constant from [`crate::background::DEFAULT_ASYNC_CHILD_TIMEOUT_MS`]
/// rather than an alias of it. They coincide today (upstream calls the async one "same generous
/// default as foreground") but upstream keeps them independent, and the two paths apply them at
/// different seams.
pub const DEFAULT_FOREGROUND_TIMEOUT_MS: u64 = 30 * 60 * 1000;
```

### 2. `src/registration/mod.rs` — the config key

```rust
    /// pi `config.timeoutMs` (`resolveConfigDefaultTimeoutMs`,
    /// `runs/foreground/subagent-executor.ts:2684` @v0.57.0): the global default wall-clock
    /// deadline, replacing the built-in backstop wherever a concrete default is applied. The only
    /// way to raise a long fan-out's ceiling without passing `timeoutMs` on every call.
    ///
    /// Carried RAW, exactly like [`Self::turn_budget`] and for the SAME reason: upstream's
    /// validator returns `undefined` for anything invalid and NEVER errors, so a malformed value
    /// must degrade to the built-in default rather than fail this whole struct's deserialization
    /// and take every other setting with it. A typed `Option<u64>` here would do exactly that for
    /// `"timeoutMs": -5`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<serde_json::Value>,
```

…and `timeout_ms: None` in the `Default` impl beside `turn_budget: None`.

### 3. `src/extension/tool/params.rs` — the validator, the ladder, the default parameter

```rust
/// pi `MAX_TIMER_DELAY_MS` (`subagent-executor.ts:2675` @v0.57.0). Upstream's reason is a Node
/// `setTimeout` overflow; cyrup keeps the same bound because `Instant + Duration` PANICS on
/// overflow (`foreground.rs`/`chain.rs` arm the deadline that way) and because the same settings
/// file must behave identically in both ports.
const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;

/// pi `resolveConfigDefaultTimeoutMs` (`subagent-executor.ts:2684` @v0.57.0). Silently yields
/// `None` for ANY invalid value — absent, non-integer, non-positive, or above the timer ceiling —
/// so the caller falls back to the built-in default. Upstream never errors here, and neither does
/// this: a bad `subagents.timeoutMs` must not fail a run that would otherwise have a sane deadline.
pub(crate) fn resolve_config_default_timeout_ms(raw: Option<&serde_json::Value>) -> Option<u64> {
    let value = raw?.as_u64()?;
    (value > 0 && value <= MAX_TIMER_DELAY_MS).then_some(value)
}
```

> `Value::as_u64` already rejects strings, booleans, floats with a fractional part, and negatives —
> upstream's `typeof !== "number" || !Number.isInteger || <= 0` in one call. Note `as_u64` returns
> `None` for `1.0` too, where upstream's `Number.isInteger(1.0)` is `true`; a JSON `1.0` for a
> millisecond count is not a shape worth widening the port for, and the degrade is to the built-in
> default, not an error.

The ladder, as one helper so four call sites do not each re-spell it:

```rust
/// The foreground default rung of pi `resolveSingleAgentLaunchTimeout`
/// (`subagent-executor.ts:2719-2725` @v0.57.0): `configDefaultTimeoutMs ?? DEFAULT_FOREGROUND_TIMEOUT_MS`,
/// with the agent's own frontmatter `timeoutMs` ahead of both (upstream folds that into `params`
/// before the ladder runs; this port resolves it separately, so it is the first rung here).
///
/// Returns what [`resolve_foreground_timeout`] should be given as its default — NOT the final
/// timeout: an explicit call-site `timeoutMs`/`maxRuntimeMs` still outranks all three.
#[must_use]
pub(crate) fn foreground_timeout_default(
    agent_default_ms: Option<u64>,
    config_timeout_ms: Option<&serde_json::Value>,
) -> Option<u64> {
    agent_default_ms
        .or_else(|| resolve_config_default_timeout_ms(config_timeout_ms))
        .or(Some(crate::exec::DEFAULT_FOREGROUND_TIMEOUT_MS))
}
```

And the default parameter on the existing validator — appended last, so it is reached only when
BOTH params are absent, which is exactly upstream's early return:

```rust
pub(crate) fn resolve_foreground_timeout(
    p: &SubagentToolParams,
    default_timeout_ms: Option<u64>,
) -> Result<Option<u64>, String> {
    // ...the existing positivity and alias-agreement checks, unchanged: they run BEFORE the
    // default so an invalid explicit value still errors rather than being silently replaced...
    Ok(p.timeout_ms.or(p.max_runtime_ms).or(default_timeout_ms))
}
```

### 4. `src/extension/tool/routing.rs` — the three tool surfaces

**`route_single`** — move the `let cfg = self.executor.config_snapshot().await;` binding (and the
`depth`/`background` derivation it feeds) ABOVE the timeout resolution, then:

```rust
        // SUBA-077 / pi `resolveSingleAgentLaunchTimeout` (`subagent-executor.ts:2719-2725`
        // @v0.57.0). The backstop is gated on this launch being FOREGROUND, which is upstream's
        // own `!async` arm: an async single launch already picks up
        // `DEFAULT_ASYNC_CHILD_TIMEOUT_MS` downstream at `executor/background.rs`'s
        // `timeout_ms.unwrap_or(...)`, and handing that `unwrap_or` a `Some` on every run would
        // silently retire it.
        //
        // NOTE the agent rung moved INTO the default argument. It cannot stay as a trailing
        // `.or(launch_defaults.1)`: the default would already have filled the value, leaving an
        // agent's `timeoutMs:` frontmatter permanently unreachable.
        let timeout_default = if background {
            launch_defaults.1
        } else {
            foreground_timeout_default(launch_defaults.1, cfg.timeout_ms.as_ref())
        };
        let timeout_ms =
            resolve_foreground_timeout(p, timeout_default).map_err(ToolError::new)?;
```

**`route_chain`** (`:1567`) — a chain names many agents, so there is no single agent rung; pass
`None` for it:

```rust
        let timeout_ms = resolve_foreground_timeout(
            p,
            foreground_timeout_default(None, cfg.timeout_ms.as_ref()),
        )
        .map_err(ToolError::new)?;
```

**`route_parallel_mode`** — resolve the same way and replace the hard-coded `None` argument to
`run_or_background_graph` with it, retiring the "carries no timeout param yet" comment. This is the
half of the item that fixes a dropped EXPLICIT value, not just a missing default.

> Both graph paths reach `run_or_background_graph`, which serves foreground and background. Match
> `route_single`: derive `background` first and pass `foreground_timeout_default(...)` only on the
> foreground arm.

### 5. `src/extension/host/slash.rs` — the `/run` surface

The foreground branch (`:389`) passes `default_timeout_ms` — the agent rung alone. Wrap it:

```rust
                    // SUBA-077: `/run` parses no timeout token, so the ladder here is agent
                    // frontmatter > `subagents.timeoutMs` > the built-in backstop. Without the
                    // last rung this surface has no wall-clock deadline at all.
                    foreground_timeout_default(default_timeout_ms, cfg.timeout_ms.as_ref()),
```

Leave the BACKGROUND branch above it (`:382`) untouched — that is `background.rs`'s default and
SUBA-051's item.

---

## Definition of done

1. A foreground `subagent({agent, task})` against an agent with no frontmatter `timeoutMs`, and no
   `subagents.timeoutMs`, resolves a timeout of `1_800_000` — where it resolves `None` today.
2. `subagents.timeoutMs: 60000` replaces that built-in on all four foreground surfaces: tool single,
   tool `tasks: []`, tool `chain`, and `/run`.
3. An explicit call-site `timeoutMs` still outranks both, and an agent's frontmatter `timeoutMs`
   still outranks the config value and the built-in — the agent rung must be demonstrably live, not
   shadowed by the new default.
4. An invalid `subagents.timeoutMs` — `0`, `-1`, `"abc"`, `true`, or `2_147_483_648` — is IGNORED,
   falling back to the built-in `1_800_000`, and never errors a run or fails config load.
5. A top-level `tasks: []` call carrying an explicit `timeoutMs` now propagates it instead of
   dropping it on the floor.
6. An ASYNC single launch is unchanged: `route_single` hands the background path no foreground
   backstop, so `DEFAULT_ASYNC_CHILD_TIMEOUT_MS` still decides.
7. `cargo test -p cyrup-ext-subagents`, `cargo clippy -p cyrup-ext-subagents --all-targets` and
   `cargo doc -p cyrup-ext-subagents --no-deps --lib` stay as clean as they are now (2540 passing,
   no new clippy finding, no doc warning).

## Notes for whoever executes

- `resolve_foreground_timeout` has exactly two call sites today; adding a parameter surfaces both
  plus any test callers at `cargo check --all-targets`.
- **Out of scope, and worth its own item:** upstream applies the SAME `configDefaultTimeoutMs` to
  the plain single-agent ASYNC default (`asyncSingleDefault` at `:2723`). This task deliberately
  leaves `background.rs:314` alone — that is SUBA-051's site. After this lands, `subagents.timeoutMs`
  will govern the four foreground surfaces but not the async one.
- The item's "Relation to corpus" note is correct and should be acted on: `SUBA-051`'s Fix line says
  *"Do not apply it to foreground runs, which already have their own default"* — false at HEAD, and
  the reason this hole survived. Flag it for `SUBA-CORPUS-HEALTH`.
- The call-site `timeoutMs` param is NOT bounded by `MAX_TIMER_DELAY_MS`, because upstream's
  `resolveForegroundTimeout` does not bound it either — only the config validator does. Leave that
  asymmetry as upstream has it.
