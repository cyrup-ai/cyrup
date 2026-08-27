---
stage: aug
status: done
priority: MEDIUM
tool: all
source: exec follow-up — self-reported by the mutation-lock executor
updated: 2026-08-27 20:16
---

# The `guard()`-is-first-`.await` guard degrades instead of failing

## What is wrong

[`crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs`](../../../crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs)
exists to fail if anyone inserts an `.await` above `guard()` in
[`write.rs:108`](../../../crates/cyrup-tools/src/tools/write.rs) /
[`edit.rs:273`](../../../crates/cyrup-tools/src/tools/edit.rs). Its assertion (3),
at `mutation_lock_is_first_await.rs:121-126`, calls `lock::registration_is_held()`
([`lock.rs:49-52`](../../../crates/cyrup-tools/src/lock.rs)), which is a `try_lock`
on the **process-global** `MUTATION_REGISTRATION` static (`lock.rs:43-44`).

The sibling test `edit_takes_the_mutation_lock_before_any_other_await`
(`mutation_lock_is_first_await.rs:146`) *holds* that same chain while parked on the
hogged blocking thread. So the observer can read "held" for a reason that has
nothing to do with the code under test.

Measured by the executor, with a `yield_now()` inserted above `write.rs`'s
`guard()` as the RED lever:

| condition | detection |
|---|---|
| test run alone | 20/20 |
| run with its sibling | 28/30 |
| full `cyrup-tools` lib suite — **the merge-gate condition** | **2/3** |

So under the condition that actually matters, the guard **degrades** rather than
fails. The brief's own coverage-map criterion was "fails — not degrade", and the
brief had already flagged exactly this `try_lock`-against-a-process-global hazard
for a different step, then used it here anyway.

**GREEN is unaffected** — the test passes 20/20 deterministically. This is purely
about how reliably it catches the regression it exists to catch.

Assertion (2) (`calls == 0`, `mutation_lock_is_first_await.rs:113-120`) does not
compensate: a bare `yield_now()` makes no `FsOps` call either, so it is invisible
to the counter. Nor could any `FsOps` recorder ever compensate —
`FileMutationLocks::key` calls `tokio::fs::canonicalize` **directly**
(`lock.rs:182`), not through the `FsOps` seam, so the lock step is structurally
invisible to that observer.

## Why it degrades: the writers you are actually competing with

`MUTATION_REGISTRATION` is global **by design** (pi parity — the doc at
`lock.rs:30-42` is explicit that a per-path chain would be the wrong port). Every
`write`, every `edit`, and every direct `FileMutationLocks::guard` call in the
`cyrup-tools` lib binary takes it, and holds it across a `tokio::fs::canonicalize`
(`lock.rs:218` → `:224`). The sibling is one writer among many:

| file | test fns |
|---|---|
| `src/tests/tools.rs` | 69 |
| `src/tests/pi_tool_semantics.rs` | 17 |
| `src/lock.rs` (`mod tests`) | 11 |
| `src/tests/write_semantics.rs` | 9 |
| `src/tests/isolation.rs` | 7 |
| `src/tests/cross_registry_mutation_lock.rs` | 3 |
| `src/tests/edit_preview_diff.rs` | 2 |

One of them is decisive: `the_registration_chain_spans_key_resolution`
(`lock.rs:278`) uses the *same* hog-the-blocking-thread trick and deliberately
parks inside `Self::key` **holding the chain**, then asserts `registration_is_held()`
itself (`lock.rs:301`). Its own doc already concedes the hazard, at `lock.rs:313-315`:

> Stated as behaviour rather than as `try_lock`, because a sibling test in this
> binary may legitimately hold the global chain for the length of its own
> `canonicalize`.

That concession is the defect, written down, one module over.

Note the direction of the numbers. Detection with the sibling alone is 28/30
(93%); adding the rest of the suite drops it to 2/3 (67%). Detection got *worse*
when the writers outside this module joined, which means the sibling is **not**
the dominant interferer. This is the fact that decides the parity action below.

## Required path — one non-global observer

Replace assertion (3)'s process-global `try_lock` with a **per-instance witness
that `guard`'s body began executing**. Nothing else. Do not touch assertions (1)
or (2), and do not delete `registration_is_held` — `lock.rs:301` still uses it.

### Why this is the right property

The property the test must prove is *"no `.await` **suspends** before `guard()`"*.
That is precisely, and only, the harmful case: `cyrup-agent`'s `execute_parallel`
hands each body on once it has been driven to its **first suspension point**
(`cyrup-agent/src/agent/run/tools/exec.rs:177-181`). An `.await` above `guard()`
that resolves without suspending moves nothing and is harmless; an `.await` that
suspends moves the handoff and reopens the ordering bug.

`FileMutationLocks::guard` is an `async fn`. Its body does not run until the
returned future is polled. So:

> **`guard`'s body was entered during `execute`'s first poll**
> ⟺ **nothing above `guard()` suspended.**

That biconditional is the whole design. It is an exact statement of the property,
not a proxy for it — which is what `registration_is_held()` was.

### Why it detects a bare `yield_now()` deterministically

With `tokio::task::yield_now().await` inserted above `write.rs:108`, the first
poll of `execute` returns `Pending` at the yield. `guard`'s future is never
constructed, let alone polled. The counter reads `0`, the `assert_eq!` fails.
There is no clock, no scheduler decision, and no shared state in the path: the
counter lives on the `FileMutationLocks` instance the test itself constructed and
handed to exactly one tool. **No other test in the binary can reach it.** 20/20 by
construction, under any degree of suite parallelism.

It is also strictly more general than the lever it is verified against — a
`sleep`, a channel `recv`, a `spawn(..).await`, or any other suspending `.await`
above `guard()` fails it identically, because none of them let `guard`'s body run.

The observation is likewise immune in the GREEN direction. If a sibling happens to
hold `MUTATION_REGISTRATION` at the moment this test polls, `guard` parks on
`MUTATION_REGISTRATION.lock()` at `lock.rs:218` instead of inside `canonicalize` —
the increment has *already* happened, one line above it. Assertions (1) and (2)
still hold, and the tail (`body.await`) still completes once the sibling releases.

### Step 1 — the instrument, in [`src/lock.rs`](../../../crates/cyrup-tools/src/lock.rs)

Field, immediately after `inner` (`lock.rs:119`):

```rust
    inner: KeyedLocks<PathBuf>,
    /// Test-only, PER-INSTANCE witness that [`Self::guard`]'s body has begun executing.
    ///
    /// [`registration_is_held`] above reads `MUTATION_REGISTRATION`, which is process-global by
    /// design (pi parity, see its docs) and is taken by EVERY `write`/`edit` in the lib test
    /// binary — including `the_registration_chain_spans_key_resolution` below, which parks inside
    /// `Self::key` holding it. A test that samples that static therefore observes other tests, and
    /// `crate::tests::mutation_lock_is_first_await` degraded to 2/3 detection under the full suite
    /// because of it. This counter is reachable only through the `FileMutationLocks` the observing
    /// test constructed and handed to its own tool, so nothing else in the binary can move it.
    #[cfg(test)]
    guard_entries: std::sync::atomic::AtomicUsize,
}
```

Constructor, `FileMutationLocks::new` (`lock.rs:150-156`):

```rust
    pub fn new() -> Self {
        let map = FILE_MUTATION_LOCKS.clone();
        Self {
            inner: KeyedLocks::new(map.clone()),
            map,
            #[cfg(test)]
            guard_entries: std::sync::atomic::AtomicUsize::new(0),
        }
    }
```

`new` is the only struct-literal construction site in the workspace (verified:
`Default` at `lock.rs:126-128` delegates to it, and every other call site uses
`::new()` / `::default()`), so no other file needs a change.

Accessor, inside `impl FileMutationLocks`, directly above `guard`:

```rust
    /// How many times [`Self::guard`]'s body has begun executing on THIS instance.
    ///
    /// `1` after a single poll of a `write`/`edit` body means the caller reached `guard()` without
    /// suspending first — which is exactly the property
    /// `crate::tests::mutation_lock_is_first_await` exists to pin. `0` means an `.await` above
    /// `guard()` suspended and took the `execute_parallel` handoff with it.
    #[cfg(test)]
    pub(crate) fn guard_entries(&self) -> usize {
        self.guard_entries
            .load(std::sync::atomic::Ordering::SeqCst)
    }
```

The increment, as the **first statement** of `guard`'s body (`lock.rs:215`, above
the `MUTATION_REGISTRATION.lock().await` at `:218`):

```rust
    ) -> Result<MutationGuard, ToolError> {
        // Test-only witness, deliberately ABOVE every `.await` in this body: an async fn's body
        // does not run until its future is polled, so reaching this line at all means the caller
        // got here without suspending. Compiles to nothing outside `cfg(test)`.
        #[cfg(test)]
        self.guard_entries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Pi `:33`: the registration slot is claimed in call order and the body below runs
        // serialized.
        let registration = MUTATION_REGISTRATION.lock().await;
```

Use fully-qualified `std::sync::atomic::` paths rather than adding a top-of-file
`use`: `mod tests` at `lock.rs:244` does `use super::*` **and** imports
`AtomicUsize`/`Ordering` itself at `:247`, and there is no reason to perturb that.

### Step 2 — the assertion, in [`src/tests/mutation_lock_is_first_await.rs`](../../../crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs)

Import (`:19`) — drop `registration_is_held`:

```rust
use crate::lock::FileMutationLocks;
```

Keep a handle on the instance in `assert_first_await_is_the_mutation_lock`
(replacing `:93`):

```rust
        let locks = Arc::new(FileMutationLocks::new());
        let (tool, args) = build(fs, Arc::clone(&locks), dir.path().to_path_buf());
```

`Arc::clone` is the same object, so the counter the test reads is the counter the
tool's `guard()` increments.

Assertion (3) (replacing `:121-126`):

```rust
        assert_eq!(
            locks.guard_entries(),
            1,
            "the first `.await` of `execute` is NOT `FileMutationLocks::guard` — some other await \
             was inserted above it and SUSPENDED, so `guard`'s body never ran in this poll. \
             `execute_parallel` hands the batch on at the first suspension point \
             (cyrup-agent/src/agent/run/tools/exec.rs:177-181), so same-path mutations are no \
             longer granted in the order the model issued them (DoD 1/2/3); nothing else in the \
             suite observes this. This counter is per-`FileMutationLocks`-instance on purpose — \
             it replaced a `try_lock` on the process-global `MUTATION_REGISTRATION`, which other \
             tests in this binary hold and which made this assertion degrade to 2/3 detection \
             instead of failing"
        );
```

Also update the module doc (`mutation_lock_is_first_await.rs:12-14`), which still
describes the old mechanism, to say that the witness is the per-instance
`guard_entries` counter and that the hogged blocking thread is what keeps the first
poll *parked* (so the counter is sampled mid-`guard`) rather than what is observed.

### Why not the other candidates

- **A counting/recording `FsOps`.** Already present as assertion (2), and it
  cannot be strengthened into this. A bare `yield_now()` makes no seam call, and
  `FileMutationLocks::key` bypasses the seam entirely (`lock.rs:182` calls
  `tokio::fs::canonicalize`), so no `FsOps` observer can ever see the lock step.
- **Runtime metrics (`blocking_queue_depth`).** Genuinely non-global — the test
  owns its runtime — and it would witness "`canonicalize`'s blocking job was
  queued". Rejected on availability: tokio 1.52.3 puts it inside
  `cfg_unstable_metrics!` (`src/runtime/metrics/runtime.rs:249-574`; the macro at
  `src/macros/cfg.rs:278-286` expands to `#[cfg(tokio_unstable)]`), and this
  workspace's `.cargo/config.toml` sets no such flag, so the method does not exist
  under `cargo test`. It would also need a "the hog is actually running" handshake,
  since a hog still sitting in the queue is itself queue depth.
- **An instrumented waker ("was the waker woken during the poll?").** Rejected as
  both fragile and incomplete. tokio 1.52.3's `yield_now` (`src/task/yield_now.rs`)
  calls `context::defer(cx.waker())`, which wakes immediately *only* when no
  scheduler context is set (`src/runtime/context.rs:168-178`); inside a worker it
  goes on the scheduler's defer list and the waker is never touched. So the signal
  is a tokio implementation detail on both sides. Worse, it detects only
  *self-waking* awaits: `tokio::time::sleep(..)` above `guard()` would pass.
- **A poll-count observer.** The test performs exactly one poll. A poll count says
  nothing about *where* that poll parked.
- **Threading a test-only observer through the call.** The signature is
  `cyrup_core::Tool::execute`, implemented by every tool in the workspace. A
  test-only parameter there is a production API change to serve one assertion.

## The fallback (parity option 2), plainly

Serialising the two tests behind a module-local mutex:

- **What it buys.** It removes exactly one interferer —
  `edit_takes_the_mutation_lock_before_any_other_await` can no longer be parked in
  the chain while `write_takes_…` samples it. On the executor's own numbers that
  turns the *pair* condition from 28/30 into 30/30.
- **What it does not buy.** It does **not** make detection 20/20 under the full
  suite. It cannot: a mutex private to
  `src/tests/mutation_lock_is_first_await.rs` is invisible to the ~118 test
  functions in the seven other files that drive `WriteTool`/`EditTool`/
  `FileMutationLocks`, and invisible in particular to
  `the_registration_chain_spans_key_resolution` (`lock.rs:278`), which parks inside
  `Self::key` holding the global chain by design. The measurements already prove
  the sibling is not the dominant term: detection *fell* from 28/30 to 2/3 when the
  rest of the suite was added. Removing a non-dominant interferer reduces
  interference; it does not eliminate it, and it leaves a probabilistic assertion
  probabilistic.
- **The only version that would work is unacceptable.** To reach 20/20 with a
  global observer, the mutex would have to cover *every* writer of
  `MUTATION_REGISTRATION` in the binary — i.e. serialise the whole `cyrup-tools`
  lib suite around one assertion. Reject.

**Take option 1.** Option 2 is recorded here as evaluated-and-rejected, not as a
live choice.

## Adjacent, deliberately out of scope

`the_registration_chain_spans_key_resolution` (`lock.rs:278`, assertion at
`:301`) has the same defect: it samples the process-global chain and can read
"held" because a sibling holds it, masking its own RED lever (moving
`drop(registration)` above `Self::key(..).await`). The `guard_entries` counter
does **not** fix it — its property is "the chain is held *across key resolution*",
which is genuinely about the global. Note it; do not expand this task to cover it.

## Verification

The gate is `cargo test`, not `cargo nextest run` (`.config/nextest.toml:15-17`).
**Do not verify with nextest**: it runs each test in its own process, which removes
the sibling-in-process condition entirely and would report a green the gate does
not. For the same reason, do not pass `--test-threads=1`. Run the full lib suite
at default parallelism — that *is* the merge-gate condition.

```bash
cd /home/user/cyrup
```

**RED — insert the lever above `write.rs`'s `guard()`:**

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path("crates/cyrup-tools/src/tools/write.rs")
s = p.read_text()
needle = "        let _guard = self.locks.guard(&abs, &cancel).await?;"
assert s.count(needle) == 1, "write.rs guard() call site moved — re-locate it"
p.write_text(s.replace(needle, "        tokio::task::yield_now().await;\n" + needle))
PY

red=0
for i in $(seq 1 20); do
  cargo test -p cyrup-tools --lib 2>&1 \
    | grep -q '^test tests::mutation_lock_is_first_await::write_takes_the_mutation_lock_before_any_other_await \.\.\. FAILED' \
    && red=$((red+1))
done
echo "RED (write): $red/20"    # MUST be 20/20
```

**Revert the lever:**

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path("crates/cyrup-tools/src/tools/write.rs")
s = p.read_text()
assert s.count("        tokio::task::yield_now().await;\n") == 1
p.write_text(s.replace("        tokio::task::yield_now().await;\n", "", 1))
PY
```

**RED — the `edit` half.** Repeat both blocks with
`crates/cyrup-tools/src/tools/edit.rs` and the same needle
(`        let _guard = self.locks.guard(&abs, &cancel).await?;`, `edit.rs:273`),
grepping for
`^test tests::mutation_lock_is_first_await::edit_takes_the_mutation_lock_before_any_other_await \.\.\. FAILED`.
MUST be 20/20.

**GREEN — with both levers reverted:**

```bash
green=0
for i in $(seq 1 20); do
  cargo test -p cyrup-tools --lib >/dev/null 2>&1 && green=$((green+1))
done
echo "GREEN: $green/20"        # MUST be 20/20
```

**No production behaviour change** — assert it structurally, not by eye. Every
edit in Step 1 is `#[cfg(test)]`, so the non-test lib is unchanged:

```bash
cargo check -p cyrup-tools 2>&1 | tail -3
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
```

Baseline to preserve: workspace check clean, 3947 tests across 9 crates, zero
failures.

## Do not

- Do not weaken or delete the assertion. The property it guards is real and is the
  one thing standing between a future refactor and a silent reintroduction of the
  original bug.
- Do not delete `registration_is_held` (`lock.rs:49-52`) — `lock.rs:301` still
  uses it.
- Do not relax assertion (1) (`is_pending()`) or assertion (2) (`calls == 0`).
  (1) pins the hog mechanism that keeps the poll parked *inside* `guard`;
  (2) still catches an `FsOps` call hoisted above the lock, which the counter
  would not distinguish from a non-suspending one.
- Do not make `MUTATION_REGISTRATION` per-instance or per-path. It is global by
  design (`lock.rs:30-42`) and `cross_registry_mutation_lock.rs` exists to keep it
  that way.

## Related

Same class of defect as
`MEDIUM-permission-system-extension-tests-flake-under-parallel-execution.md`:
process-global state sampled by a test that does not control every writer. Worth
fixing with the same eye.

## Definition of done

1. With a `yield_now()` inserted above `write.rs`'s `guard()`, the test fails
   **20/20** under the full `cyrup-tools` lib suite, not just in isolation.
2. Same, 20/20, for the `edit.rs` half with the lever above `edit.rs:273`.
3. With both levers reverted, both tests pass **20/20** under the same conditions.
4. No production behaviour changes — every instrument added in Step 1 is
   `#[cfg(test)]`.
5. Assertions (1) and (2) are unchanged, and `registration_is_held` still exists
   for `lock.rs:301`.
