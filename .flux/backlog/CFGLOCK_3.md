---
stage: exec
status: done
updated: 2026-08-23 02:35
---

# CFGLOCK_3 — Carry The Async Config API Through Its Three Downstream Crates

**Part 3 of 3.** `CFGLOCK_2` landed as `c3d0cdf` and deliberately left `cargo check --workspace`
failing. This task is what makes it build again.

OBJECTIVE: propagate `cyrup-config`'s now-async trust/settings/lock API outward, without
introducing a single `block_on`.

## Scope — now measured from the compiler, not estimated

The pre-`CFGLOCK_2` table in this file was written from greps and was **wrong in three ways**. With
part 2 landed the compiler is ground truth, and it says:

- **Three crates are affected, not two.** `cyrup-ext-subagents` was missing entirely — it calls
  `cyrup_config::lock::FileLock::acquire` directly.
- **`crates/cyrup` does not appear in `cargo check --workspace` output at all**, because it depends
  on `cyrup-session-svc`, which fails first — cargo never reaches it. Its breakage is real but
  *masked*. Fix the two library crates first, then re-run to reveal it. Its sites are listed below
  from grep so they are not a surprise.
- Several sites were missed: a third accessor, `trust::set`, and two more `cyrup` call sites.

### The full set

**`cyrup-ext-subagents` — the crate the old table missed**

| Site | Now | Note |
| --- | --- | --- |
| `discovery/settings_write.rs:82` `lock_settings_file` | → `async fn` | calls `FileLock::acquire(path)`; add the second arg `None` and `.await` |
| `discovery/settings_write.rs:150` `merge_builtin_agent_override` | → `async fn` | `pub` |
| `discovery/settings_write.rs:181` `remove_builtin_agent_override` | → `async fn` | `pub` |
| `discovery/settings_write.rs:211` `remove_builtin_agent_override_fields` | → `async fn` | `pub` |
| `discovery/management.rs:3459` `handle_disable`, `:3523` `handle_enable`, `:3606` `handle_reset` | → `async fn` | private |
| `discovery/management.rs:1421` `handle_management_action` | → `pub async fn` | the dispatch |
| `extension/tool/routing.rs:1094` | add `.await` | enclosing `route_management_action` (`:1024`) is **already `async`** and already awaits `discover_available_skills` — no restructuring needed |

**`cyrup-session-svc`**

| Site | Now |
| --- | --- |
| `session/accessors.rs:102` `saved_trust_decision` | → `pub async fn` |
| `session/accessors.rs:114` `write_project_trust` | → `pub async fn` |
| `session/accessors.rs:130` `persist_setting` | → `pub async fn` — **missed by the old table** |
| `builder.rs:623` | `.and_then(\|store\| store.nearest(&cwd).ok().flatten())` — restructure |
| `builder.rs:636` | `store.set(&cwd, Some(decision))` — `trust::set` is async now too, **missed by the old table** |

**`crates/cyrup` — masked until the two above compile**

| Site | Enclosing fn | Note |
| --- | --- | --- |
| `subcommands.rs:441` | `fn saved_trusted(dirs) -> bool` (`:438`) | sync — the shape most likely to tempt a `block_on` |
| `subcommands.rs:887` | `async fn run_config` (`:809`) | already async, just `.await` — **missed by the old table** |
| `main.rs:1996` | `async fn run_interactive` (`:1957`) | already async, just `.await` — **missed by the old table** |
| `startup_ui.rs:391` | `fn persist_trust_choice` (`:386`) | sync |
| `startup_ui.rs:864`, `:907` | test fns | sync |

## SUBTASK1 — `cyrup-session-svc` first

`builder.rs:623` is the only genuinely structural change in the task; do it first, while everything
else is still untouched.

```rust
// before — an async fn cannot be used inside `and_then`
let saved = self.trust_store.as_ref()
    .and_then(|store| store.nearest(&cwd).ok().flatten());

// after — `build` is already `async fn`, so the await is free
let saved = match self.trust_store.as_ref() {
    Some(store) => store.nearest(&cwd).await.ok().flatten(),
    None => None,
};
```

`:636`'s `store.set(&cwd, Some(decision))` sits inside `if let Err(e) = …` — that stays, it just
becomes `store.set(&cwd, Some(decision)).await`.

Then the three accessors become `pub async fn` with **signatures otherwise identical** — same
parameters, same return types — so the blast radius on the session service's consumers is the
`async` keyword alone.

## SUBTASK2 — `cyrup-ext-subagents`

Convert bottom-up: `lock_settings_file`, then the three `settings_write` pub fns, then the three
`management` handlers, then `handle_management_action`, then `.await` at `routing.rs:1094`.

`lock_settings_file` gains the new argument:

```rust
async fn lock_settings_file(path: &Path) -> Result<cyrup_config::lock::FileLock, SubagentError> {
    cyrup_config::lock::FileLock::acquire(path, None).await.map_err(|e| { … })
}
```

`None` for the token, consistent with every other non-`models_store` caller — this crate has no
`CancelToken` in scope at these sites, and fabricating one would defeat the point.

**One comment goes stale and must be corrected**, `routing.rs:1056`:

> "cyrup's skill scan is `async` and `handle_management_action` is sync, so the laziness lives here
> instead: …"

The clause "and `handle_management_action` is sync" becomes false. Fix that clause only — the
surrounding rationale about *where the laziness lives* is still correct and still worth keeping,
because the config is still resolved before the scan is awaited. Do not rewrite the paragraph.

## SUBTASK3 — `crates/cyrup`, once the libraries compile

`subcommands.rs:887` and `main.rs:1996` are already inside `async fn` — a bare `.await`.

`saved_trusted(dirs) -> bool` and `persist_trust_choice` are sync and must become `async fn`, then
await outward. **If the chain reaches a sync `main`, take the async up to where the runtime already
exists — do not block inside.** `startup_ui.rs` carries extensive doc comments about `set_many`
being unconditional once entered (`:380`, `:763`, `:804`, `:841`, `:903`); those stay accurate
because only the call becomes awaited. Do not rewrite that prose.

## Method — this worked in `CFGLOCK_2`, reuse it

Hand-editing ~100 call sites invites transcription errors. Drive the mechanical part off the
compiler's own spans instead:

1. `cargo check -p <crate> --all-targets --message-format=json`
2. For `E0599` where the message contains `opaque type` and `Future`: insert `.await` before the
   `.method` at the primary span (`byte_start`, scan back to the preceding `.`).
3. For `E0308` where the **primary span's `label`** contains `found future`: append `.await` at
   `byte_end`. (The label, not the message — the message is only "mismatched types".)
4. Then promote any `fn` whose body now contains `.await` to `async fn`, and any `#[test]`
   immediately above it to `#[tokio::test]`.
5. Repeat to a fixpoint, then move to the next crate.

Apply edits back-to-front within a file so earlier byte offsets stay valid.

## Expected test churn

Mechanical, and concentrated in one file:

| File | Scale |
| --- | --- |
| `cyrup-ext-subagents/src/discovery/management.rs` | 51 `handle_management_action(` call sites; **30 of its 72 sync `#[test]` fns** call it and must become `#[tokio::test]` |
| `cyrup-ext-subagents/src/tests/management_actions_integration.rs:128` | 1 call site |
| `cyrup/src/startup_ui.rs:864`, `:907` | 2 test fns |

Match each file's existing async-test convention; do not introduce a new one.

## Definition of done

- [ ] `cargo check --workspace --all-targets` is clean — the workspace builds again
- [ ] **Zero** `block_on` / `futures::executor::block_on` / `Handle::block_on` anywhere in the diff
      (`git diff | grep -c block_on` returns 0)
- [ ] `builder.rs:623`'s `.and_then(…)` is restructured to a `match`, not worked around
- [ ] The three `cyrup-session-svc` accessors differ from before by the `async` keyword only
- [ ] Every `FileLock::acquire` call outside `models_store` passes `None`, not a fabricated token
- [ ] `routing.rs:1056`'s "and `handle_management_action` is sync" clause is corrected, and the rest
      of that paragraph is untouched
- [ ] `startup_ui.rs`'s `set_many` doc comments are unchanged
- [ ] `cargo test --workspace` shows no new failures against the pre-`CFGLOCK_2` baseline
      (`cyrup-config` 222, `cyrup-tools` 253+1+2, `cyrup-core` 36)
- [ ] `cargo clippy --workspace --all-targets` adds no new warnings

## Research notes

- Part 2's API: `FileLock::acquire(target, cancel: Option<&CancelToken>) -> Result<Self, ConfigError>`,
  async. See [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs).
- Async in `cyrup-config` now: `trust::{nearest, set_many, set}`,
  `settings::store::with_lock`, ten `settings::manager` setters, `models_store::read_latest`.
- `route_management_action` (`routing.rs:1024`) is already `async` — verified, no restructure.
- Do **not** run `cargo fmt`: no crate here is rustfmt-clean at HEAD, so it reformats whole packages
  and would bury this diff entirely.

## No tests

Tests are in scope (the earlier "another team owns tests" line was wrong). Add tests for any behaviour this task changes. The `.await` and `#[tokio::test]`
migration above is mechanical and in scope; anything requiring a logic change is not — stop and
flag it.

## No benchmarks

No benchmarks: this task is not performance-scoped.
