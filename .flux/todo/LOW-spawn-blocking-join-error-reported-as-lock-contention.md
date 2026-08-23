---
title: Spawn Blocking Join Error Reported As Lock Contention
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:39
---

# The `spawn_blocking` join failure in `FileLock::acquire` reports contention that never happened

One rule, two files. Give [`ConfigError`](../../crates/cyrup-config/src/error.rs) a variant for
"the lock-acquisition task never produced a result", and return it from every `spawn_blocking` join
in [`lock.rs`](../../crates/cyrup-config/src/lock.rs) instead of `ConfigError::Lock`. No
`resume_unwind`, no reuse of `Cancelled`, no other file. Two sibling tasks are rewriting the code
around that join right now, in mutually exclusive ways; the rule is stated so it survives either —
see [Coordinating](#coordinating-with-the-sibling-tasks-on-this-function).

---

## Research — what was verified

### 1. The site, and what a `JoinError` can be here

[`lock.rs:64-67`](../../crates/cyrup-config/src/lock.rs), inside `FileLock::acquire`:

```rust
let target_owned = target.to_path_buf();
let file = tokio::task::spawn_blocking(move || open_and_lock(&target_owned, &lock_path))
    .await
    .map_err(|_| ConfigError::Lock { path: target.to_path_buf() })??;
```

The first `?` is on the join, the second on `open_and_lock`'s own `Result`. Only the join mapping is
wrong: `ConfigError::Lock` renders as `"lock contention on {path}"`
([`error.rs:53-54`](../../crates/cyrup-config/src/error.rs)), and a `JoinError` is never contention.
`tokio-1.52.3/src/runtime/task/error.rs:15-18` has exactly two representations:

```rust
enum Repr {
    Cancelled,
    Panic(SyncWrapper<Box<dyn Any + Send + 'static>>),
}
```

`Cancelled` here can only be the runtime dropping a queued blocking task on shutdown: this code
never calls `JoinHandle::abort`, and an already-started `spawn_blocking` task cannot be aborted at
all. (The sibling `AcquireTask` design does call `abort`, but only from `Drop`, when no one is left
to observe the join.) `Panic` is a panic inside the closure.

Also confirmed new on this branch: at merge base `4902cddf` `acquire` was
`pub fn acquire(target: &Path) -> Result<Self, ConfigError>` running `ensure_dir`/`open`/`FileExt::lock`
inline, so no join existed and a panic propagated to the caller normally.

### 2. Release builds abort on panic — the panic arm is unreachable in a shipped binary

Root [`Cargo.toml:281-286`](../../Cargo.toml):

```toml
[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
```

With `panic = "abort"` there is no unwind for tokio's `catch_unwind` to catch: a panic inside the
closure kills the process. `JoinError::is_panic()` is therefore reachable only in unwinding profiles
— `[profile.dev]` (`:274-275`) sets no `panic` key, so dev and test builds unwind and the arm is
live there. This is the fact that decides the shape of the fix: an `is_panic()` branch calling
`std::panic::resume_unwind` would be dead code in every binary users run.

`crates/cyrup-tui/src/panic_hook.rs:17-26` states the same profile fact from the other side, and
notes the corollary that matters for the "silent panic" half of this finding: a panic **hook** still
runs. `std::panic::set_hook`'s hook executes at panic time, before any `catch_unwind`, so in the
unwinding builds where a `JoinError::Panic` can be produced at all, the message, location and any
`RUST_BACKTRACE` output have **already** been printed by the default hook (or by the chained TUI
hook, `panic_hook.rs:96-106`). What is genuinely lost today is not the stderr output — it is the
returned error value, which asserts a cause it does not have.

### 3. `JoinError`'s own `Display` carries the panic payload

`tokio-1.52.3/src/runtime/task/error.rs:135-153`:

```rust
Repr::Cancelled => write!(fmt, "task {} was cancelled", self.id),
Repr::Panic(p) => match panic_payload_as_str(p) {
    Some(panic_str) => write!(fmt, "task {} panicked with message {:?}", self.id, panic_str),
    None => write!(fmt, "task {} panicked", self.id),
},
```

So `join.to_string()` recovers the payload for every `&str`/`String` panic — `panic!`, `unwrap`,
`expect`, slice bounds — and distinguishes the cancelled case in the same string. Stringifying the
`JoinError` therefore addresses both losses at once without a second variant and without
`into_panic`.

### 4. House convention: collapse a `JoinError` into an accurate domain error, never re-panic

`grep -rn 'resume_unwind\|into_panic\|\.is_panic()' --include=*.rs crates/` returns **zero** hits
workspace-wide. Every join site instead maps to a domain error whose text is true:

| site | mapping |
| --- | --- |
| [`cyrup-mcp/src/runtime.rs:2456-2464`](../../crates/cyrup-mcp/src/runtime.rs) | `Err(_join) => McpError::Server { message: "MCP stdio environment resolution failed to run" }`, with a comment that the arm is "defensive" under the crate's lint policy |
| [`cyrup-mcp/src/request_headers_command.rs:871-880`](../../crates/cyrup-mcp/src/request_headers_command.rs) | `Err(_) => command_error("HTTP request headers command failed to start")` |
| [`cyrup-resources/src/package/install.rs:80-82`](../../crates/cyrup-resources/src/package/install.rs) | `.map_err(\|e\| ResourceError::Git(e.to_string()))?` — keeps the `JoinError` text |
| [`cyrup-resources/src/discovery/mod.rs:415-419`](../../crates/cyrup-resources/src/discovery/mod.rs) | `ResourceError::Core(CoreError::Io(io::Error::other(e.to_string())))` — keeps the text |
| [`cyrup-config/src/config_value.rs:598-600`](../../crates/cyrup-config/src/config_value.rs) | `.unwrap_or_default()`, with "A panic in the blocking body is *unresolvable*, the same answer the sync path gives" |
| [`cyrup-config/src/config_value.rs:616-622`](../../crates/cyrup-config/src/config_value.rs) | `Err(_) => Err(format!("Failed to resolve {description}"))` |
| [`cyrup-ext-subagents/src/background/watch.rs:672-686`](../../crates/cyrup-ext-subagents/src/background/watch.rs) | `.unwrap_or(false)` — "degrades to *not delivered*", stated in the comment |

The two in-crate precedents (`config_value.rs`) collapse to a value, but the value they collapse to
is *true* for a join failure: "unresolvable" / "failed to resolve" describe the outcome without
inventing a cause. This site is the only one in the workspace whose collapsed value names a cause it
cannot know. The fix is to rejoin the convention, not to invent a new posture.

The lint policy those comments lean on is real: `[workspace.lints.clippy]`
([`Cargo.toml:97-102`](../../Cargo.toml)) denies `unwrap_used`, `expect_used`, `panic` and
`indexing_slicing`, and `cyrup-config` inherits it (`crates/cyrup-config/Cargo.toml:11-12`). So a
panic in `open_and_lock` can only come from `std`/`fs4`, or from future maintenance — which is
exactly why the arm must stay defensive rather than disappear.

### 5. `ConfigError::Cancelled` is spoken for, and taking it would break a sibling task

`Cancelled` has exactly one meaning on this path today: layer 1's `KeyedLocks::guard` lost its
`select!` to the caller's token ([`lock.rs:60-63`](../../crates/cyrup-config/src/lock.rs)).
[`MEDIUM-lock-cancellation-reported-as-model-source-not-aborted.md`](./MEDIUM-lock-cancellation-reported-as-model-source-not-aborted.md)
is in flight to make `store_err` map that variant to `ProviderError::Aborted` — `code() ==
"aborted"`, `is_aborted() == true`, `StopReason::Aborted` on the wire
(`cyrup-provider/src/error.rs:180-186`). Routing a runtime-shutdown join failure into `Cancelled`
would report a *runtime fault* as a *user-initiated abort*, and would falsify the invariant that
task is about to write into `store_err`'s doc comment. The finding's own suggested
`Err(_) => ConfigError::Cancelled` arm is therefore rejected below.

### 6. `ConfigError::Io` is spoken for too

`io_err` ([`lock.rs:176-183`](../../crates/cyrup-config/src/lock.rs)) is documented as tagging "an
`io::Error` with the path whose syscall produced it". A join failure produced no syscall and no
`io::Error`; wrapping one in `io::Error::other` to reuse the variant would falsify that contract in
the same file that states it.

### 7. A new variant costs nothing

`grep -rn 'ConfigError::' --include=*.rs crates/ | grep -v crates/cyrup-config/src` → **0 hits**.
The enum is constructed only inside its own crate (37 uses) and is `match`ed nowhere at all today;
the only `match` on it in flight is the sibling `store_err` above, whose catch-all
`other => ProviderError::ModelSource(Box::new(other))` arm absorbs the new variant with the correct
meaning ("the store operation failed", not "the user aborted"). `ConfigError` is re-exported at
`crates/cyrup-config/src/lib.rs:53`, so this is an additive public-API change on a `0.0.0`
workspace-internal crate; no `#[non_exhaustive]` is involved and no architecture doc enumerates the
variants (`docs/` mentions `ConfigError` only in `gap-analysis/05-…:828`, about `models.json`
messages).

No test anywhere constructs or asserts on `ConfigError::Lock`, and no test references `FileLock`
(`grep -rn 'FileLock' --include=*.rs crates/ | grep -i test` → nothing), so nothing pins the current
wording.

---

## Required change — two files, three hunks

### Hunk 1 — [`crates/cyrup-config/src/error.rs`](../../crates/cyrup-config/src/error.rs), insert between `Lock` (`:53-54`) and `Cancelled` (`:55-56`)

```rust
    #[error("lock contention on {path}")]
    Lock { path: PathBuf },
    /// The blocking half of [`crate::lock::FileLock::acquire`] never produced a result: the task
    /// panicked (unwinding builds only — the release profile is `panic = "abort"`), or the runtime
    /// dropped it while shutting down.
    ///
    /// Deliberately NOT [`Self::Lock`]: nothing was contended, and "lock contention on …" sends an
    /// operator looking for a competing process that does not exist. Deliberately not
    /// [`Self::Cancelled`] either — that one means the caller's own `CancelToken` fired
    /// (`lock.rs:60-63`), which is a user-initiated abort rather than a failure. `message` is the
    /// `JoinError`'s own `Display`, which carries the panic payload when there is one.
    #[error("lock acquisition for {path} failed to run to completion: {message}")]
    LockTaskFailed { path: PathBuf, message: String },
    #[error("cancelled")]
    Cancelled,
```

`String`, not `#[source] JoinError`: no error type in this workspace carries a `JoinError` (verified
in §4), the `Lock`/`Trust`/`Dir` neighbours are all string- or path-shaped, and it keeps `tokio` out
of the error vocabulary.

### Hunk 2 — [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs), the join arm (`:64-67` on today's branch tip)

Before:

```rust
        let target_owned = target.to_path_buf();
        let file = tokio::task::spawn_blocking(move || open_and_lock(&target_owned, &lock_path))
            .await
            .map_err(|_| ConfigError::Lock { path: target.to_path_buf() })??;
```

After:

```rust
        let target_owned = target.to_path_buf();
        // The one outcome here that is NOT about the lock: a `JoinError` means the closure
        // panicked, or the runtime dropped the task while shutting down. Reported as
        // `ConfigError::Lock` it sends an operator hunting for a peer process that never existed.
        // Not re-panicked either: the release profile is `panic = "abort"` (root `Cargo.toml`), so
        // the panic arm is unreachable in a shipped binary, and in the unwinding builds where it is
        // reachable the panic hook has already printed message and location before tokio caught it.
        // Defensive, not expected — `clippy::panic`/`unwrap_used`/`expect_used`/`indexing_slicing`
        // are all denied workspace-wide, so nothing this closure calls in-tree panics by
        // construction.
        let joined =
            tokio::task::spawn_blocking(move || open_and_lock(&target_owned, &lock_path)).await;
        let file = match joined {
            Ok(result) => result?,
            Err(join) => return Err(join_failed(target, &join)),
        };
```

`Ok(result) => result?` is the second `?` of the old `??`: `open_and_lock`'s own errors — including
the `ConfigError::Cancelled` the sibling designs return from inside it — pass through unchanged.

### Hunk 3 — same file, new private helper immediately after `io_err` (`:176-183`)

```rust
/// A `spawn_blocking` acquisition that never produced a result: the task panicked, or the runtime
/// dropped it while shutting down. Names the TARGET for the same reason `open_and_lock` does — the
/// `<path>.lock` sidecar is not a file any operator opens — and carries the `JoinError`'s own text,
/// which includes the panic payload when there is one.
fn join_failed(target: &Path, join: &tokio::task::JoinError) -> ConfigError {
    ConfigError::LockTaskFailed {
        path: target.to_path_buf(),
        message: join.to_string(),
    }
}
```

A helper rather than an inline struct literal for two reasons: it is the exact shape `io_err`
already has in this file (a private, path-tagging `ConfigError` constructor sitting at the bottom),
and the HIGH sibling below turns one join site into two — with the helper, the second site is
`Err(join) => return Err(join_failed(target, &join))` and the rule cannot drift between them.

All three hunks were run through `rustfmt --edition 2024` verbatim and come back unchanged (longest
line 100 chars). Do **not** let a stray `cargo fmt` reflow the neighbouring
`Ok(Self { _in_process: in_process, file })` — default rustfmt wants that expanded, and it belongs to
[`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md),
not here.

Nothing else changes: `open_and_lock`'s signature, the contention mapping at `:87`, `Drop`, and the
happy path are all untouched.

---

## Coordinating with the sibling tasks on this function

Three augmented tasks now rewrite parts of `FileLock::acquire`. This one owns **only** the mapping
of a `JoinError`; it never decides what runs on the blocking pool. State of play as of
2026-08-23 03:39, both sibling specs read in full:

| task | status | what it does to the join | join sites after it |
| --- | --- | --- | --- |
| [`HIGH-dropped-acquire-future-detaches-blocking-flock-task.md`](./HIGH-dropped-acquire-future-detaches-blocking-flock-task.md) | `done` (spec) | replaces the blocking `flock` with `FileExt::try_lock` retried from async land: a bounded `open_and_try_lock` prologue plus one `try_lock` job per retry tick | **two** |
| [`MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md`](./MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md) | `done` (spec) | keeps the blocking `flock`, moves the layer-1 guard into the closure, wraps the handle in an aborting `AcquireTask`, races `token.cancelled()` against the join in a `biased` `select!` | **one**, inside a select arm |

The two are **mutually exclusive by their own words** — the HIGH spec rejects the MEDIUM's shape as
its option (B) and says "do not also apply MEDIUM's suggested fix"; the MEDIUM spec says its own
design is void if the HIGH lands with polling. That contest is theirs to settle, and this task does
not depend on the outcome. The invariant to hold either way:

> Every `spawn_blocking` join inside `FileLock::acquire` returns `join_failed(target, &join)`.
> `ConfigError::Lock` survives only where a lock genuinely could not be taken.

**If the HIGH design lands** (its Interactions section already names this task: "after this change
there are **two** `map_err(|_| ConfigError::Lock { … })` on `JoinError`, not one … That task must fix
both, with the same variant"). Prologue:

```rust
        let owned_target = target.to_path_buf();
        let joined =
            tokio::task::spawn_blocking(move || open_and_try_lock(&owned_target, &lock_path)).await;
        let (mut file, mut held) = match joined {
            Ok(result) => result?,
            Err(join) => return Err(join_failed(target, &join)),
        };
```

and the retry attempt inside the `while !held` loop:

```rust
            let owned_target = target.to_path_buf();
            let joined = tokio::task::spawn_blocking(move || try_lock(file, &owned_target)).await;
            let (f, h) = match joined {
                Ok(result) => result?,
                Err(join) => return Err(join_failed(target, &join)),
            };
```

Hunk 2's comment then belongs above the prologue only; the loop site needs no second copy.

**If the MEDIUM design lands**, hunk 2 becomes a replacement of its join arm — the arm's value is
already `Result<Self, ConfigError>` there, so no `?` and no `return`:

```rust
            joined = &mut task.0 => match joined {
                Ok(result) => result,
                Err(join) => Err(join_failed(target, &join)),
            }
```

with hunk 2's comment moved above the `select!`. Note that the MEDIUM's DoD line "the `JoinError`
arm is byte-identical to the pre-change mapping" means *carry whatever mapping is in the file
through unchanged* — if this task landed first, that is `join_failed`, **not** a restored
`ConfigError::Lock`. Its scope fence already assigns this arm here: "Discriminating panic from
runtime-shutdown belongs to [this task]; this restructure leaves that a one-arm edit."

Neither sibling changes hunk 1 or hunk 3, and neither can be broken by them.

## Paths considered and rejected

- **`Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic())`** (the finding's first
  suggestion). Dead code in every shipped binary — `panic = "abort"` means no `JoinError::Panic` is
  ever constructed in release (§2) — and zero precedent workspace-wide (§4). It would also make a
  public `async fn` in a library crate unwind through the caller's `select!`/`Drop` path in test
  builds only, i.e. a panic-propagation behaviour that differs by profile.
- **`Err(_) => ConfigError::Cancelled`** for the `is_cancelled()` half. Collides with the sibling
  `store_err` arm that turns `Cancelled` into `ProviderError::Aborted`, relabelling a runtime fault
  as a user abort (§5).
- **Two variants, one per `JoinError` kind.** Both mean "the acquisition task produced no result",
  the caller can act on neither differently, and `JoinError`'s `Display` already distinguishes them
  inside the message (§3).
- **`ConfigError::Io { path, source: io::Error::other(join.to_string()) }`.** Falsifies `io_err`'s
  stated contract in the same file (§6), even though `cyrup-resources` reaches for that shape — it
  has no better-fitting variant, and this crate would be *adding* the misfit deliberately.
- **`.unwrap_or_default()` / degrade to a value**, as `config_value.rs` does twice. There is no
  meaningful default for a lock guard: failing to acquire must reach the caller (`models_store.rs:245-247`
  is explicit that an untaken lock "is not a degraded read — it is an unserialized one").
- **Leaving it and only widening `Lock`'s message.** `Lock` is still the right variant for `:87`,
  where contention is exactly what happened; blurring its text to cover both would degrade the
  accurate case to fix the inaccurate one.

## Do not touch

- The lock-refusal mapping inside `open_and_lock` — `FileExt::lock(&file).map_err(|_| ConfigError::Lock { … })`
  at [`lock.rs:87`](../../crates/cyrup-config/src/lock.rs), or whatever the winning sibling design
  leaves in its place (the HIGH spec's `try_lock` helper keeps the same variant for a real
  `TryLockError::Error`). `Lock` is the right answer there; it is only the *join* that is wrong.
  That the mapping is also imprecise for non-contention errno values (`EINTR`, `ENOLCK`) predates
  this branch (merge base `4902cddf`, `lock.rs:30`) and is a separate finding if anyone wants it.
- The layer-1 `map_err(|_| ConfigError::Cancelled)` at `:60-63` — correct as it stands.
- Every `FileLock::acquire` call site: `auth.rs:277`, `settings/store.rs:69`, `trust.rs:150`/`:174`,
  `models_store.rs:248`/`:311`/`:338`, `cyrup-ext-subagents/src/discovery/settings_write.rs:84`. All
  of them already render whatever `ConfigError` they get through `Display` (`{e}` / `e.to_string()`
  / `Box`ed source), so the new variant surfaces with no call-site change.
- `crates/cyrup-config/src/models_store.rs` — `store_err` belongs to the sibling MEDIUM task, whose
  catch-all `other =>` arm already gives `LockTaskFailed` the right identity
  (`ProviderError::ModelSource`: the operation failed, the user did not abort it).
- Whatever the two siblings do to the *closure*, the retry structure, the type doc, or `acquire`'s
  doc comment. This task adds no sentence to either doc block.
- `crates/cyrup-tui/src/panic_hook.rs` and root `Cargo.toml` — cited only as evidence.

## Definition of done

1. `ConfigError::LockTaskFailed { path: PathBuf, message: String }` exists in
   `crates/cyrup-config/src/error.rs`, sited between `Lock` and `Cancelled`, with the doc comment
   above and the `#[error]` text `"lock acquisition for {path} failed to run to completion: {message}"`.
   No other variant's text, fields or ordering changes.
2. `join_failed(target: &Path, join: &tokio::task::JoinError) -> ConfigError` exists in
   `crates/cyrup-config/src/lock.rs` next to `io_err`, and is the ONLY constructor of
   `LockTaskFailed`.
3. **Every** `spawn_blocking` join in `FileLock::acquire` — one on today's branch tip, two if the
   HIGH sibling has landed — is a two-arm `match`: `Ok(result) => result?` (or `=> result` inside a
   `select!` arm) and `Err(join) => … join_failed(target, &join)`. `grep -n 'map_err(|_|' crates/cyrup-config/src/lock.rs`
   shows no match on a join result, and the `JoinError` value is used rather than discarded.
4. `grep -n 'ConfigError::Lock' crates/cyrup-config/src/lock.rs` names only lock-refusal sites — the
   `FileExt::lock`/`FileExt::try_lock` mapping — and no join site.
5. A join failure renders as `lock acquisition for /…/settings.json failed to run to completion:
   task 12 panicked with message "boom"` (or `… task 12 was cancelled`); the string "lock contention"
   is no longer reachable from a `JoinError`.
6. `git diff --stat` against the branch tip lists exactly two files —
   `crates/cyrup-config/src/error.rs` and `crates/cyrup-config/src/lock.rs` — with the `lock.rs`
   change confined to the join statement(s), their comment, and the new helper. No signature change,
   no new `use`, no new dependency, no call-site change.
7. `cargo check -p cyrup-config` is clean and the edit introduces no new clippy or rustdoc warning;
   the three intra-doc links used (`crate::lock::FileLock::acquire`, `Self::Lock`, `Self::Cancelled`)
   all resolve.
8. `rustfmt --check --edition 2024 crates/cyrup-config/src/lock.rs crates/cyrup-config/src/error.rs`
   reports no *new* violation attributable to these hunks (all three were verified rustfmt-stable;
   pre-existing branch violations on neighbouring lines stay as they are). No workspace-wide
   `cargo fmt`.
