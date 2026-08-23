---
title: Spawn Blocking Join Error Reported As Lock Contention
priority: LOW
stage: qa
status: completed
updated: 2026-08-23 07:49
---

# The `spawn_blocking` join failures in `FileLock::acquire` report contention that never happened

One rule, two files, **four exact edits**. Give `ConfigError` a variant for "the lock-acquisition
job never produced a result", add one private constructor for it in `lock.rs`, and route **both**
`spawn_blocking` joins in `FileLock::acquire` through it instead of `ConfigError::Lock`. No
`resume_unwind`, no reuse of `Cancelled`, no other file.

**Every anchor string below was matched against the working tree at 2026-08-23 05:48 and occurs
exactly once in its file.** Every replacement was run through `rustfmt --edition 2024` and comes
back byte-identical.

---

## Queue state — the coordination question from the previous pass is CLOSED

The earlier revision of this spec branched on two in-flight siblings. Both have since landed and
neither is a variable any more:

| task | where it is now | effect on this task |
| --- | --- | --- |
| `HIGH-dropped-acquire-future-detaches-blocking-flock-task.md` | `.flux/done/2026-08-23-00-08/` — **landed in the tree** | `FileLock::acquire` now runs a bounded `open_and_try_lock` prologue plus one `try_lock` job per retry tick, so there are **two** join sites, not one |
| `MEDIUM-lock-cancellation-reported-as-model-source-not-aborted.md` | `.flux/done/2026-08-23-00-08/` — **landed in the tree** | `store_err` in `models_store.rs` already reads `ConfigError::Cancelled => ProviderError::Aborted`, with catch-all `other => ProviderError::ModelSource(Box::new(other))` |
| `MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md` | still in `.flux/todo/`, frontmatter `status: done`, body opens with **"This spec is VOID as written. Do NOT implement the design below."** | not a factor |

Consequences, all re-verified against the current source:

- `open_and_lock` **no longer exists**. The blocking body is now `open_and_try_lock` (prologue) and
  `try_lock` (one non-blocking attempt), both private fns in `lock.rs`.
- The `ConfigError::Lock` mapping this task must fix appears **twice** — `lock.rs:118` and
  `lock.rs:140` — and the *correct* `ConfigError::Lock` (a real `flock` error) is at `lock.rs:187`,
  inside `try_lock`'s `Err(TryLockError::Error(_))` arm.
- `store_err`'s catch-all already absorbs the new variant with the right identity ("the store
  operation failed", not "the user aborted"). **No `models_store.rs` edit is needed or permitted.**

---

## Research — re-verified against the working tree

### 1. The two sites, and what a `JoinError` can be here

Both live in `FileLock::acquire` (`crates/cyrup-config/src/lock.rs`). Prologue join, `:114-120`:

```rust
        let owned_target = target.to_path_buf();
        let (mut file, mut held) =
            tokio::task::spawn_blocking(move || open_and_try_lock(&owned_target, &lock_path))
                .await
                .map_err(|_| ConfigError::Lock {
                    path: target.to_path_buf(),
                })??;
```

Retry-tick join, inside `while !held`, `:137-142`:

```rust
            let owned_target = target.to_path_buf();
            let (f, h) = tokio::task::spawn_blocking(move || try_lock(file, &owned_target))
                .await
                .map_err(|_| ConfigError::Lock {
                    path: target.to_path_buf(),
                })??;
```

In each, the first `?` is on the join and the second on the closure's own `Result`. Only the join
mapping is wrong: `ConfigError::Lock` renders as `"lock contention on {path}"`
(`error.rs`, the `Lock` variant), and a `JoinError` is never contention.
`tokio-1.52.3/src/runtime/task/error.rs:15-18` has exactly two representations:

```rust
enum Repr {
    Cancelled,
    Panic(SyncWrapper<Box<dyn Any + Send + 'static>>),
}
```

`Cancelled` here can only be the runtime dropping a queued blocking task on shutdown: this code
never calls `JoinHandle::abort`, and an already-started `spawn_blocking` task cannot be aborted at
all. `Panic` is a panic inside the closure.

### 2. Release builds abort on panic — the panic arm is unreachable in a shipped binary

Root `Cargo.toml`, `[profile.release]` (`:293-296`):

```toml
[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
```

With `panic = "abort"` there is no unwind for tokio's `catch_unwind` to catch: a panic inside a
closure kills the process. `JoinError::is_panic()` is therefore reachable only in unwinding
profiles — `[profile.dev]` (`:286-287`) sets no `panic` key, so dev and test builds unwind and the
arm is live there. This decides the shape of the fix: an `is_panic()` branch calling
`std::panic::resume_unwind` would be dead code in every binary users run.

`crates/cyrup-tui/src/panic_hook.rs` states the same profile fact from the other side (module doc,
`:17-26`) and notes the corollary: a panic **hook** still runs. `std::panic::set_hook`'s hook
executes at panic time, before any `catch_unwind`, so in the unwinding builds where a
`JoinError::Panic` can be produced at all, the message, location and any `RUST_BACKTRACE` output
have **already** been printed by the default hook (or by the chained TUI hook — see
`install_panic_hook`, which calls `restore_terminal_best_effort()` then `previous(info)`). What is
genuinely lost today is not the stderr output — it is the returned error value, which asserts a
cause it does not have.

### 3. `JoinError`'s own `Display` carries the panic payload

`tokio-1.52.3/src/runtime/task/error.rs`, `impl fmt::Display for JoinError` (`:136-150`):

```rust
Repr::Cancelled => write!(fmt, "task {} was cancelled", self.id),
Repr::Panic(p) => match panic_payload_as_str(p) {
    Some(panic_str) => write!(fmt, "task {} panicked with message {:?}", self.id, panic_str),
    None => write!(fmt, "task {} panicked", self.id),
},
```

So `join.to_string()` recovers the payload for every `&str`/`String` panic — `panic!`, `unwrap`,
`expect`, slice bounds — and distinguishes the cancelled case in the same string. Stringifying the
`JoinError` addresses both losses at once, without a second variant and without `into_panic`.

### 4. House convention: collapse a `JoinError` into an accurate domain error, never re-panic

`grep -rn 'resume_unwind\|into_panic\|\.is_panic()' --include=*.rs crates/` returns **zero** hits
workspace-wide (re-run, still zero). Every join site instead maps to a domain error whose text is
true:

| site | mapping |
| --- | --- |
| `cyrup-mcp/src/runtime.rs:2462` | `McpError::Server { message: "MCP stdio environment resolution failed to run" }`, with a comment that the arm is "defensive" under the crate's lint policy |
| `cyrup-mcp/src/request_headers_command.rs:878` | `command_error("HTTP request headers command failed to start")` |
| `cyrup-resources/src/package/install.rs:82` | `.map_err(\|e\| ResourceError::Git(e.to_string()))?` — keeps the `JoinError` text |
| `cyrup-resources/src/discovery/mod.rs:415-417` | `ResourceError::Core(CoreError::Io(std::io::Error::other(…)))` — keeps the text |
| `cyrup-config/src/config_value.rs:598-600` | `.unwrap_or_default()`, with "A panic in the blocking body is *unresolvable*, the same answer the sync path gives" |
| `cyrup-config/src/config_value.rs:616-622` | `Err(_) => Err(format!("Failed to resolve {description}"))` |
| `cyrup-ext-subagents/src/background/watch.rs:679-685` | `.unwrap_or(false)` — "degrades to *not delivered*", stated in the comment |

The two in-crate precedents (`config_value.rs`) collapse to a value, but that value is *true* for a
join failure: "unresolvable" / "failed to resolve" describe the outcome without inventing a cause.
`FileLock::acquire` is the only place in the workspace whose collapsed value names a cause it
cannot know. The fix is to rejoin the convention, not to invent a new posture.

The lint policy those comments lean on is real: `[workspace.lints.clippy]` (root `Cargo.toml`,
`:97-101`) denies `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`, and `cyrup-config`
inherits it (`crates/cyrup-config/Cargo.toml:11-12`, `[lints] workspace = true`). So a panic in
`open_and_try_lock`/`try_lock` can only come from `std`/`fs4`, or from future maintenance — which is
exactly why the arms must stay defensive rather than disappear.

### 5. `ConfigError::Cancelled` is spoken for

`Cancelled` has exactly two producers on this path, and both mean the caller's own token fired:
`KeyedLocks::guard` losing its `select!` (`lock.rs:106-109`) and the retry loop's biased cancel arm
(`lock.rs:133`). `store_err` (`models_store.rs:209-214`) — already landed — maps that variant to
`ProviderError::Aborted`, which is `code() == "aborted"`, `is_aborted() == true`, and
`StopReason::Aborted` on the wire (`cyrup-provider/src/error.rs`, `into_error_message`, `:181-185`).
Routing a runtime-shutdown join failure into `Cancelled` would report a *runtime fault* as a
*user-initiated abort*, and would falsify the invariant `store_err`'s doc comment now states in the
tree ("A lock that genuinely could not be taken is a different variant … and still lands on
`ProviderError::ModelSource`"). Rejected below.

### 6. `ConfigError::Io` is spoken for too

`io_err` (`lock.rs:279-286`) is documented as tagging "an `io::Error` with the path whose syscall
produced it". A join failure produced no syscall and no `io::Error`; wrapping one in
`io::Error::other` to reuse the variant would falsify that contract in the same file that states it.

### 7. A new variant costs nothing

`grep -rn 'ConfigError::' --include=*.rs crates/ | grep -v crates/cyrup-config/src` → **0 hits**.
The enum is constructed only inside its own crate (43 uses) and is `match`ed in exactly one place,
`store_err`, whose catch-all `other => ProviderError::ModelSource(Box::new(other))` absorbs the new
variant with the correct meaning. `ConfigError` is re-exported at
`crates/cyrup-config/src/lib.rs:53`, so this is an additive change on a `0.0.0` workspace-internal
crate; no `#[non_exhaustive]` is involved and no architecture doc enumerates the variants (`docs/`
mentions `ConfigError` only in `gap-analysis/05-cyrup-config-and-resources.md:828`, about
`models.json` messages).

No test anywhere constructs or asserts on `ConfigError::Lock`, and no test references `FileLock`
(`grep -rn 'FileLock' --include=*.rs crates/ | grep -i test` → nothing), so nothing pins the current
wording.

---

## Required change — four exact edits

Apply them in order. Each `MATCH` block is the literal current text; each `REPLACE` block is what
must stand in its place. Before each edit, confirm the `MATCH` text occurs **exactly once** in the
file; if a count is not 1, stop and re-read the file rather than guessing.

### Edit 1 — `crates/cyrup-config/src/error.rs`, insert the variant between `Lock` and `Cancelled`

MATCH (must occur exactly 1×):

```rust
    #[error("lock contention on {path}")]
    Lock { path: PathBuf },
    #[error("cancelled")]
    Cancelled,
```

REPLACE:

```rust
    #[error("lock contention on {path}")]
    Lock { path: PathBuf },
    /// A `spawn_blocking` job inside [`crate::lock::FileLock::acquire`] never produced a result:
    /// the task panicked (unwinding builds only — the release profile is `panic = "abort"`), or
    /// the runtime dropped it while shutting down.
    ///
    /// Deliberately NOT [`Self::Lock`]: nothing was contended, and "lock contention on …" sends an
    /// operator looking for a competing process that does not exist. Deliberately not
    /// [`Self::Cancelled`] either — that one means the caller's own `CancelToken` fired, which is a
    /// user-initiated abort rather than a failure, and `models_store::store_err` turns it into
    /// `ProviderError::Aborted`. `message` is the `JoinError`'s own `Display`, which carries the
    /// panic payload when there is one.
    #[error("lock acquisition for {path} failed to run to completion: {message}")]
    LockTaskFailed { path: PathBuf, message: String },
    #[error("cancelled")]
    Cancelled,
```

`message: String`, not `#[source] JoinError`: no error type in this workspace carries a `JoinError`
(§4), the `Lock`/`Trust`/`Dir` neighbours are all string- or path-shaped, and it keeps `tokio` out of
`cyrup-config`'s error vocabulary. No new `use` is needed — `PathBuf` is already imported at
`error.rs:3`.

### Edit 2 — `crates/cyrup-config/src/lock.rs`, the prologue join

MATCH (must occur exactly 1×):

```rust
        let owned_target = target.to_path_buf();
        let (mut file, mut held) =
            tokio::task::spawn_blocking(move || open_and_try_lock(&owned_target, &lock_path))
                .await
                .map_err(|_| ConfigError::Lock {
                    path: target.to_path_buf(),
                })??;
```

REPLACE:

```rust
        let owned_target = target.to_path_buf();
        // The one outcome here that is NOT about the lock: a `JoinError` means the closure
        // panicked, or the runtime dropped the task while shutting down. Reported as
        // `ConfigError::Lock` it sends an operator hunting for a peer process that never existed.
        // Not re-panicked either: the release profile is `panic = "abort"` (root `Cargo.toml`), so
        // the panic arm is unreachable in a shipped binary, and in the unwinding builds where it is
        // reachable the panic hook has already printed message and location before tokio caught it.
        // Defensive, not expected — `clippy::panic`/`unwrap_used`/`expect_used`/`indexing_slicing`
        // are all denied workspace-wide, so nothing these closures call in-tree panics by
        // construction. The retry attempt below maps its join the same way.
        let joined =
            tokio::task::spawn_blocking(move || open_and_try_lock(&owned_target, &lock_path)).await;
        let (mut file, mut held) = match joined {
            Ok(result) => result?,
            Err(join) => return Err(join_failed(target, &join)),
        };
```

`Ok(result) => result?` is the second `?` of the old `??`: `open_and_try_lock`'s own errors — the
`ConfigError::Io` from `ensure_dir`/`open`, and the `ConfigError::Lock` from a real `flock` error —
pass through unchanged.

### Edit 3 — `crates/cyrup-config/src/lock.rs`, the retry-tick join

MATCH (must occur exactly 1×):

```rust
            let owned_target = target.to_path_buf();
            let (f, h) = tokio::task::spawn_blocking(move || try_lock(file, &owned_target))
                .await
                .map_err(|_| ConfigError::Lock {
                    path: target.to_path_buf(),
                })??;
```

REPLACE:

```rust
            let owned_target = target.to_path_buf();
            let joined = tokio::task::spawn_blocking(move || try_lock(file, &owned_target)).await;
            let (f, h) = match joined {
                Ok(result) => result?,
                Err(join) => return Err(join_failed(target, &join)),
            };
```

No second copy of Edit 2's comment: its last sentence already covers this site.

### Edit 4 — `crates/cyrup-config/src/lock.rs`, the helper, immediately after `io_err`

`io_err` is the **last item in the file**; the file ends with its closing `}` and a trailing
newline. Append the helper after it.

MATCH (must occur exactly 1×, and is the file's tail):

```rust
fn io_err(path: &Path, source: std::io::Error) -> ConfigError {
    ConfigError::Io {
        path: path.to_path_buf(),
        source,
    }
}
```

REPLACE:

```rust
fn io_err(path: &Path, source: std::io::Error) -> ConfigError {
    ConfigError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// A `spawn_blocking` acquisition attempt that never produced a result: the task panicked, or the
/// runtime dropped it while shutting down. Names the TARGET for the same reason `try_lock` does —
/// the `<path>.lock` sidecar is not a file any operator opens — and carries the `JoinError`'s own
/// text, which includes the panic payload when there is one.
fn join_failed(target: &Path, join: &tokio::task::JoinError) -> ConfigError {
    ConfigError::LockTaskFailed {
        path: target.to_path_buf(),
        message: join.to_string(),
    }
}
```

A helper rather than two inline struct literals, and this is the **required** shape, not the
convenient one: two sites must not be able to drift apart, and it is exactly the form `io_err`
already has in this file — a private, path-tagging `ConfigError` constructor at the bottom.

No new `use` in `lock.rs`: `tokio::task::JoinError` is written out in full, and `Path`/`PathBuf` are
already imported at `lock.rs:6`. `JoinError` is reachable with the workspace tokio feature set
(`rt-multi-thread` implies `rt`).

### Formatting

All four replacements were applied to scratch copies and run through
`rustfmt --edition 2024`; both patched files come back **byte-identical**, and both files are
rustfmt-clean before the edit too, so this introduces no `rustfmt` delta at all. Longest new line is
100 characters (the `spawn_blocking(... open_and_try_lock ...).await;` continuation), exactly at the
default `max_width`. Do **not** run a workspace-wide `cargo fmt` — unrelated drift belongs to
`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`.

Nothing else changes: `open_and_try_lock`, `try_lock`, `Drop`, `lock_path_for`, `ensure_dir`,
`write_atomic` and the happy path are all untouched.

## Paths considered and rejected

- **`Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic())`** (the finding's first
  suggestion). Dead code in every shipped binary — `panic = "abort"` means no `JoinError::Panic` is
  ever constructed in release (§2) — and zero precedent workspace-wide (§4). It would also make a
  public `async fn` in a library crate unwind through the caller's `select!`/`Drop` path in test
  builds only, i.e. panic-propagation behaviour that differs by profile.
- **`Err(_) => ConfigError::Cancelled`** for the `is_cancelled()` half. Collides with the landed
  `store_err` arm that turns `Cancelled` into `ProviderError::Aborted`, relabelling a runtime fault
  as a user abort (§5).
- **Two variants, one per `JoinError` kind.** Both mean "the acquisition job produced no result",
  the caller can act on neither differently, and `JoinError`'s `Display` already distinguishes them
  inside the message (§3).
- **Two inline struct literals instead of `join_failed`.** Two sites, one rule; an inline literal
  lets them diverge on the next edit, and `io_err` already establishes the helper shape here.
- **`ConfigError::Io { path, source: io::Error::other(join.to_string()) }`.** Falsifies `io_err`'s
  stated contract in the same file (§6), even though `cyrup-resources` reaches for that shape — it
  has no better-fitting variant, and this crate would be *adding* the misfit deliberately.
- **`.unwrap_or_default()` / degrade to a value**, as `config_value.rs` does twice. There is no
  meaningful default for a lock guard: failing to acquire must reach the caller (`models_store.rs`
  is explicit at `:263-265` that an untaken lock "is not a degraded read — it is an unserialized
  one, so it reaches the caller rather than being `.ok()`-ed").
- **Leaving it and only widening `Lock`'s message.** `Lock` is still the right variant at
  `lock.rs:187`, where a real `flock` failure happened; blurring its text to cover both would
  degrade the accurate case to fix the inaccurate one.

## Do not touch

- The lock-refusal mapping inside `try_lock` — `Err(TryLockError::Error(_)) => Err(ConfigError::Lock { … })`
  at `lock.rs:187-189`. `Lock` is the right answer there; it is only the *joins* that are wrong.
  That this mapping is also imprecise for non-contention errno values (`ENOLCK`) is a separate
  finding if anyone wants it.
- The `EINTR` retry arm at `lock.rs:184`, the `WouldBlock` arm at `:180`, and `try_lock`'s
  move-the-`File`-through signature.
- The layer-1 `.map_err(|_| ConfigError::Cancelled)` at `lock.rs:109` and the retry loop's cancel
  arm at `:133` — both correct as they stand.
- Every `FileLock::acquire` call site: `auth.rs:316`, `settings/store.rs:69`, `trust.rs:150`/`:174`,
  `models_store.rs:267`/`:330`/`:357`,
  `cyrup-ext-subagents/src/discovery/settings_write.rs:84`. All of them already render whatever
  `ConfigError` they get through `Display` (`{e}` / `e.to_string()` / `Box`ed source), so the new
  variant surfaces with no call-site change.
- `crates/cyrup-config/src/models_store.rs` in its entirety — `store_err` (`:209-214`) already
  gives `LockTaskFailed` the right identity through its catch-all `other =>` arm
  (`ProviderError::ModelSource`: the operation failed, the user did not abort it).
- The `FileLock` type doc (`lock.rs:50-73`) and `acquire`'s doc comment (`:84-102`). This task adds
  no sentence to either.
- `crates/cyrup-tui/src/panic_hook.rs` and root `Cargo.toml` — cited only as evidence.

## Definition of done

Verify by reading the two edited files and by running the read-only commands named below. No test is
written or run, no benchmark, no new documentation page, and no `git` command is used.

1. `crates/cyrup-config/src/error.rs` contains
   `LockTaskFailed { path: PathBuf, message: String }`, sited between `Lock` and `Cancelled`, with
   the doc comment from Edit 1 and the `#[error]` text
   `"lock acquisition for {path} failed to run to completion: {message}"`. No other variant's text,
   fields or ordering changed.
2. `crates/cyrup-config/src/lock.rs` ends with `join_failed(target: &Path, join: &tokio::task::JoinError) -> ConfigError`
   directly after `io_err`, and that function is the **only** constructor of `LockTaskFailed`:
   `grep -rn 'LockTaskFailed' crates/` returns exactly two lines — the variant declaration in
   `error.rs` and the construction inside `join_failed`.
3. `grep -c 'ConfigError::Lock {' crates/cyrup-config/src/lock.rs` returns **1**, and
   `grep -n 'ConfigError::Lock {' crates/cyrup-config/src/lock.rs` points at the
   `Err(TryLockError::Error(_))` arm inside `try_lock` — no join site.
4. `grep -n 'map_err(|_|' crates/cyrup-config/src/lock.rs` returns exactly one line, the layer-1
   `ConfigError::Cancelled` mapping in `acquire`. Neither `spawn_blocking` result is mapped with a
   discarding closure any more.
5. `grep -c 'join_failed(target, &join)' crates/cyrup-config/src/lock.rs` returns **2** — both
   joins are a two-arm `match` on a `let joined = …await;` binding, `Ok(result) => result?` and
   `Err(join) => return Err(join_failed(target, &join))`, with the `JoinError` value used rather
   than discarded.
6. `cargo check -p cyrup-config` completes with no error and no new warning.
7. `cargo clippy -p cyrup-config --all-targets` reports no new lint attributable to these edits, and
   `cargo doc -p cyrup-config --no-deps` emits no `broken_intra_doc_links` error — the three
   intra-doc links introduced (`crate::lock::FileLock::acquire`, `Self::Lock`, `Self::Cancelled`)
   all resolve. (`lock` is `pub mod` at `lib.rs:27`, and `.cargo/config.toml` sets
   `rustdocflags = ["--document-private-items"]`.)
8. `rustfmt --check --edition 2024 crates/cyrup-config/src/lock.rs crates/cyrup-config/src/error.rs`
   is silent. Both files were rustfmt-clean before the change and must stay so; no workspace-wide
   `cargo fmt`.
9. Exactly two files differ from the pre-edit tree — `crates/cyrup-config/src/error.rs` and
   `crates/cyrup-config/src/lock.rs`. Confirm by inspection: no signature changed, no `use` was
   added or removed in either file, no dependency was added to any `Cargo.toml`, and no call site of
   `FileLock::acquire` was touched.
10. Reading the resulting code, a join failure renders as
    `lock acquisition for /…/models-store.json failed to run to completion: task 12 panicked with message "boom"`
    (or `… task 12 was cancelled`), and the string `lock contention` is no longer reachable from a
    `JoinError` anywhere in the crate.

---

## QA verdict — 2026-08-23 07:49 — 9/10, PASS

All four edits are present in the tree and every factual claim in the new text was re-verified
against the real source:

- `error.rs:55-66` carries `LockTaskFailed { path: PathBuf, message: String }` between `Lock` and
  `Cancelled`, with the specified `#[error]` text and doc comment. No other variant changed.
- `lock.rs:144-149` and `:167-171` are the two `let joined = …await;` + two-arm `match` sites;
  `grep -c 'join_failed(target, &join)'` = 2.
- `grep -c 'ConfigError::Lock {' lock.rs` = 1, at `:216` inside `try_lock`'s
  `Err(TryLockError::Error(_))` arm — the one place contention is a true statement.
- `grep -n 'map_err(|_|' lock.rs` = 1 line, the layer-1 `ConfigError::Cancelled` at `:129`.
- `grep -rn 'LockTaskFailed' crates/` = exactly 2 lines (declaration + `join_failed`); `join_failed`
  is the sole constructor, at the tail of `lock.rs` directly after `io_err`.
- Truth of the new comments: root `Cargo.toml` `[profile.release]` really sets `panic = "abort"`;
  `[workspace.lints.clippy]` really denies `unwrap_used`/`expect_used`/`panic`/`indexing_slicing`
  and `cyrup-config` inherits via `[lints] workspace = true`; tokio 1.52.3 (Cargo.lock) really has
  `Repr::{Cancelled, Panic}` and a `Display` that emits `task N was cancelled` /
  `task N panicked with message "…"`; `cyrup-tui::install_panic_hook` really chains `previous(info)`
  so the default hook still prints; `store_err` really absorbs the new variant through its
  `other => ProviderError::ModelSource(...)` catch-all with no edit. Zero `resume_unwind`/
  `into_panic`/`is_panic()` workspace-wide, and zero `ConfigError::` construction outside the crate.
- `cargo check -p cyrup-config` clean; `cargo clippy -p cyrup-config --all-targets` emits no
  cyrup-config diagnostic (only pre-existing `cyrup-provider` `return_self_not_must_use` warnings);
  `cargo doc -p cyrup-config --no-deps` clean, so all three new intra-doc links resolve;
  `rustfmt --check --edition 2024` silent on both files.

Non-blocking nits, deliberately not reworked:
1. `models_store.rs` `store_err`'s doc still names only "`ConfigError::Lock` or `ConfigError::Io`"
   as the non-cancel lock failures. Not false (it makes no exhaustiveness claim) and that file was
   explicitly out of scope, but a future pass could add `LockTaskFailed` to the list.
2. "carries the panic payload when there is one" is exact only for `String`/`&'static str`
   payloads; a non-string payload renders as `task N panicked` (tokio
   `panic_payload_as_str`, `error.rs:183-196`). Cosmetic.
