---
stage: qa
status: completed
updated: 2026-08-29 12:20
---

# Make `SpawnCommand` Propagate Whole Across Spawn Hops — remaining work

## QA verdict: 7/10 — needs-rework

**Accepted and not to be re-opened.** `SUBAGENT_BINARY_ARGS_ENV_VAR`
([`spawn/mod.rs:96`](../../crates/cyrup-ext-subagents/src/spawn/mod.rs)) is well documented — it
states the JSON choice, why no delimiter is safe, why NUL is impossible, and why it is read only
alongside a set, non-blank binary variable. The tier-1 branch decodes it and degrades absent/empty/
unparseable to an empty `base_args`, so the never-fails contract holds; tiers 2 and 3 keep
`base_args: Vec::new()` verbatim. Gating the args variable on the binary variable was **not** in the
filing and is a genuine improvement: without it the environment could inject argv into an ordinary
run. Six tests cover the decode side properly, including `[1,2,3]` — valid JSON, invalid
`Vec<String>` — which a naive "is it JSON?" check would wave through.

**One real defect, and it reintroduces this task's own bug class.**

## The defect — a stale args value pairs with a freshly injected binary

[`exec/spawn_plan.rs:496-509`](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) seeds the
binary unconditionally but **skips the args insert when `base_args` is empty**:

```rust
if !command.base_args.is_empty()
    && let Ok(encoded) = serde_json::to_string(&command.base_args)
{
    env_overlay.insert(crate::spawn::SUBAGENT_BINARY_ARGS_ENV_VAR.to_string(), encoded);
}
```

`env_overlay` is **strictly additive**: `spawn/mod.rs:522` layers it over the inherited
environment, and `env_clear()` is deliberately never called anywhere in the crate — stated at
`:198`, `:445`, `:523` and enforced by the test at `:1656`. A variable the overlay omits therefore
**passes through from the parent's own environment**.

So when a run injects a `SpawnCommand` whose `base_args` is empty, while the parent process itself
already has `CYRUP_SUBAGENT_BINARY_ARGS` set:

| variable | value the child sees | source |
|---|---|---|
| `CYRUP_SUBAGENT_BINARY` | the **injected** binary | overlay |
| `CYRUP_SUBAGENT_BINARY_ARGS` | the **parent's stale** args | inherited, because the overlay skipped it |

The child's `resolve_spawn_command()` then reconstructs a command that never existed: one
command's binary wearing another command's leading argv. That is precisely the half-a-command
failure this task was filed to eliminate, reintroduced one layer up.

**It is reachable, not theoretical.** Any hop-2+ process spawned by an injecting parent has
`CYRUP_SUBAGENT_BINARY_ARGS` in its environment — that is the mechanism this task adds. A nested
run that injects a binary with no `base_args` inherits the outer hop's args. A developer with the
variable exported in a shell hits it on the first injected run.

**It also fails the DoD as written**, which said *"seeds both variables when a command is injected
and neither when it is not"*. The skip-when-empty optimization was not requested; it bought one
absent variable in the common case and paid for it with a correctness hole.

### The fix

Always insert the args variable whenever a command is injected — the two halves must travel
together or not at all:

```rust
if opts.spawn_command.is_some() {
    env_overlay.insert(
        crate::spawn::SUBAGENT_BINARY_ENV_VAR.to_string(),
        command.binary.display().to_string(),
    );
    // BOTH halves, always. `env_overlay` is additive and `env_clear()` is never called, so an
    // omitted variable is not "unset" — it is whatever this process inherited. Writing `[]` for
    // an empty `base_args` is what makes the injected command authoritative; skipping the insert
    // would let a stale inherited value pair with the injected binary.
    if let Ok(encoded) = serde_json::to_string(&command.base_args) {
        env_overlay.insert(crate::spawn::SUBAGENT_BINARY_ARGS_ENV_VAR.to_string(), encoded);
    }
}
```

`serde_json::to_string(&Vec::<String>::new())` is `"[]"`, which the tier-1 branch already decodes
to an empty `base_args` — no resolver change is needed. Keep `if let Ok` over `expect`: the
workspace forbids `unwrap`/`expect`, and a `Vec<String>` always serializes.

### The coverage gap that let it through

`an_injected_spawn_command_seeds_both_halves_into_the_child_env` (`spawn_plan.rs:1817`) is the only
injected-path test and it uses a **non-empty** `base_args` (`:1822`). The empty-while-injected case
— the one that breaks — has no test.

Add one that pins the invariant directly: inject a command with `base_args: vec![]` and assert the
overlay carries `CYRUP_SUBAGENT_BINARY_ARGS` as `"[]"`, with a message saying an omitted variable
would be inherited rather than unset. Assert on the overlay, not on a spawned process — the
existing tests already establish that reading `plan.spec.env_overlay` is how this file tests
seeding.

## Definition of done

- [ ] The args variable is inserted on **every** injected path, empty `base_args` included, so an
      injected command is always authoritative over both halves.
- [ ] A test covers injection with empty `base_args` and asserts the overlay carries `"[]"`.
- [ ] The existing six tests still pass unchanged — the decode side is correct and needs no edit.
- [ ] `cargo test -p cyrup-ext-subagents` and
      `cargo test -p cyrup-it --features it --test subagents -- --test-threads=1` stay green
      (2586 and 196/196 at review time), with clippy and doc clean.
