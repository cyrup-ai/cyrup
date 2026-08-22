---
stage: aug
status: done
updated: 2026-08-22 15:11
---

# Fix The Two Failing Tests

## Where this stands now

The measurement that opened this task, on `claude/project-build-env-space-d0t3jz`, rustc 1.98.0:

```
cargo nextest run --workspace --no-fail-fast
Summary [135.639s] 7859 tests run: 7857 passed, 2 failed, 8 skipped
```

**Four subsequent full-suite runs have been steady at 7859 run / 7858 passed with only
`rpc_cycle_model_spans_the_full_auth_filtered_registry` failing.** The `prompt_runtime` failure has
not recurred. That reclassifies it: it is a **flaky precondition**, not a regression. The product
behaviour it guards (`FlushGuard` releasing the re-entrancy latch on a dropped future) is present in
the tree at
[prompt_runtime.rs:247-258](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs) and armed before
the first `.await` at
[prompt_runtime.rs:563-568](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs).

Neither failure is caused by the branch it was measured on — that branch touches only `.claude/` and
`.flux/`, no Rust.

Both fixes below are test-side except one, and that one is a dependency-injection seam in
`AuthStore`, not a behaviour change.

---

## 1. `cyrup-ext-subagents prompt_runtime::tests::a_dropped_flush_future_does_not_wedge_the_steering_inbox`

### What fails

The test's own **precondition**, not its subject:

```
panicked at crates/cyrup-ext-subagents/src/prompt_runtime.rs:2654:13:
the first poll must reach an await for this to exercise a mid-body drop
```

The test at
[prompt_runtime.rs:2624-2682](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs) polls
`inbox.flush()` once by hand with `Waker::noop()` and asserts `Poll::Pending`
([prompt_runtime.rs:2654-2657](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs)), then drops
the future mid-body to prove `FlushGuard::drop` releases the latch
([prompt_runtime.rs:2664-2667](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs)). When the
first poll returns `Ready`, the drop under test never happens — the test could not set itself up.

### Why the first poll can return `Ready` — the exact mechanism

`SteeringInbox::flush` ([prompt_runtime.rs:554](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs))
does no `.await` of its own until
[prompt_runtime.rs:576-577](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs):

```rust
let requests =
    crate::background::control::consume_steer_requests_from_dir(&self.dir).await;
```

The **intended** await point is the first line of that function,
[control.rs:1021](../../crates/cyrup-ext-subagents/src/background/control.rs):

```rust
let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
```

In tokio 1.52.3 (`Cargo.lock:6787-6789`), `tokio::fs::read_dir` is
`asyncify(|| std::fs::read_dir(path)) .await` (`tokio-1.52.3/src/fs/read_dir.rs:31-41`), and
`asyncify` is `spawn_blocking(f).await` (`tokio-1.52.3/src/fs/mod.rs:312-324`). So the await is a
`JoinHandle` await on a task dispatched to the runtime's **blocking pool**.

That is the whole problem, and `current_thread` does not help:

* `#[tokio::test]` defaults to `Builder::new_current_thread()`, and that builder **still creates a
  full blocking pool** — `blocking::create_blocking_pool(self, self.max_blocking_threads)` at
  `tokio-1.52.3/src/runtime/builder.rs:1676`, cap 512 by default (`builder.rs:300`). The
  `current_thread` flavor constrains where *async tasks* are polled; it places no constraint at all
  on a blocking-pool OS thread running concurrently.
* `spawn_blocking` dispatches at **call** time, not at poll time
  (`tokio-1.52.3/src/runtime/blocking/pool.rs:393-415`). The pool thread can therefore finish
  `std::fs::read_dir` *before* the test thread performs its first poll — in which case
  `JoinHandle::poll` returns `Ready` and nothing yields.
* The preceding `write_steer_request_to_dir(...).await`
  ([prompt_runtime.rs:2641-2643](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs) →
  [control.rs:794-806](../../crates/cyrup-ext-subagents/src/background/control.rs), which itself
  calls `tokio::fs::create_dir_all`) has already forced a pool thread into existence and left it
  parked and idle, so the dispatch is a condvar notify rather than a thread spawn — the fastest
  case for the pool, i.e. the case most likely to beat the poll. Under a loaded box (this container
  is 4 CPUs with many concurrent jobs) the test thread being descheduled for a few microseconds
  after `spawn_blocking` returns is all it takes.

If `read_dir` loses the race the rest of the body can also complete inside that same poll, which is
what produces `Ready(())`:

* `entries.next_entry().await` ([control.rs:1024](../../crates/cyrup-ext-subagents/src/background/control.rs))
  does **not** touch the pool here — `read_dir`'s blocking closure already buffered up to
  `CHUNK_SIZE = 32` entries (`tokio-1.52.3/src/fs/read_dir.rs:22,35-38`), and `poll_next_entry`
  pops from that `VecDeque` and returns `Poll::Ready` (`read_dir.rs:101-111`). With one file in the
  directory both iterations are synchronous.
* `tokio::fs::read(&path).await` ([control.rs:1035-1036](../../crates/cyrup-ext-subagents/src/background/control.rs))
  and `tokio::fs::remove_file(&path).await` ([control.rs:1042](../../crates/cyrup-ext-subagents/src/background/control.rs))
  are two more identical races.
* Back in the loop, `services` is `None` (nothing ever called `set_host_services`), so the body
  takes the `let Some(services) = services.clone() else { … }` arm at
  [prompt_runtime.rs:587-596](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs) and calls
  `acknowledge`, which returns at its `ack_dir` guard
  ([prompt_runtime.rs:312-314](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs)) **before any
  await** because the test constructs the inbox with `ack_dir: None`
  ([prompt_runtime.rs:2645](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs)).

So `Ready` on the first poll requires the pool to win **three** races in a row. Rare — which is
exactly the observed profile: one failure, then four clean runs.

### Confirm the product behaviour before touching the test

Do this first, and keep it cheap — no new test files.

1. Run the test as it stands, repeatedly:
   `cargo nextest run -p cyrup-ext-subagents a_dropped_flush_future_does_not_wedge_the_steering_inbox --no-capture` in a loop
   (say 50 iterations). Expect it to pass; a failure must be the `:2654` precondition message and
   nothing else. **A failure at
   [prompt_runtime.rs:2664-2667](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs)
   (`a dropped flush must release the re-entrancy latch`) would mean the latch really is broken —
   stop and fix the product, not the test.**
2. Ablation, after the fix below is in: temporarily replace
   `let _flush_guard = FlushGuard { state: &self.state };`
   ([prompt_runtime.rs:568](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs)) with a trailing
   `state.flushing = false;` on the fall-through path and confirm the test goes red at the latch
   assertion, then revert. That is the evidence that the deterministic version still pins what the
   test exists to pin.

### The fix: make the await point unreachable-past, don't hope for it

Cap the runtime's blocking pool at **one** thread and occupy that thread before polling. Then
`spawn_blocking` provably **cannot** have run the `read_dir` closure — `BlockingPool::spawn_task`
pushes to the queue and, with `num_idle_threads() == 0 && num_threads() == thread_cap`, does not
spawn a thread (`tokio-1.52.3/src/runtime/blocking/pool.rs:406-415`). `JoinHandle::poll` therefore
returns `Pending`, by construction rather than by luck. The latch assertions are untouched.

`#[tokio::test]` cannot express this — `tokio_macros` accepts only `flavor`, `worker_threads`,
`start_paused`, `crate` and `unhandled_panic` — so the test builds its own runtime. Note
`max_blocking_threads` is the exact pool cap for the `current_thread` flavor
(`builder.rs:1676`; the multi-thread flavor adds `worker_threads`, `builder.rs:1865`).

Replace [prompt_runtime.rs:2608-2682](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs) with:

```rust
    /// SUBA-049 (review) — a DROPPED `flush` future must not latch the re-entrancy guard.
    ///
    /// Upstream cannot have this bug: its `flush` is a synchronous `(): void` whose body is wrapped
    /// in `try { … } finally { flushing = false; }` (`subagent-prompt-runtime.ts:381-413`). cyrup's
    /// port is `async` and awaits at `consume_steer_requests_from_dir`, at every `acknowledge` and
    /// at the write-back, and it is driven both from the poll task and from the turn-lifecycle event
    /// handlers — so the future genuinely can be dropped mid-body.
    ///
    /// **Red before the fix:** `flush` set `state.flushing = true` before its first `.await` and
    /// cleared it only with a trailing assignment on the fall-through path, reproducing upstream's
    /// `try` but NOT its `finally`. This test parks `flush` at its first await and drops it; pre-fix
    /// `flushing` is still `true` afterwards, the second assertion fails, and — the consequence that
    /// matters — the next flush takes the `disposed || flushing` early return, so the queued request
    /// is never consumed and `remaining_after` stays 1 forever. Post-fix `FlushGuard::drop` clears
    /// the latch and the second flush drains the inbox.
    ///
    /// **Why this builds its own runtime.** `flush`'s first await is
    /// `tokio::fs::read_dir` (`background/control.rs:1021`), which is `asyncify` →
    /// `spawn_blocking(..).await` (tokio `fs/read_dir.rs:31-41`, `fs/mod.rs:312-324`). The blocking
    /// pool is real OS threads under EVERY runtime flavor (`runtime/builder.rs:1676` builds one for
    /// `new_current_thread` too) and `spawn_blocking` dispatches at call time, so awaiting that
    /// `JoinHandle` is a RACE, not a yield point: when the pool thread finishes first the poll
    /// returns `Ready`, and — since `next_entry` is served from `read_dir`'s own 32-entry buffer and
    /// `acknowledge` returns at its `ack_dir: None` guard before any await — the whole body can
    /// complete in one poll, leaving no mid-body drop to test. That is the intermittent this shape
    /// removes: with the pool capped at one thread and that thread occupied,
    /// `BlockingPool::spawn_task` can only QUEUE the read (`runtime/blocking/pool.rs:406-415`), so
    /// the first poll MUST park at the intended await.
    #[test]
    fn a_dropped_flush_future_does_not_wedge_the_steering_inbox() {
        use std::sync::mpsc;
        use std::task::{Context, Poll, Waker};

        let rt = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .expect("runtime");

        rt.block_on(async {
            let temp = tempfile::tempdir().expect("tempdir");
            let dir = temp.path().join("steer-inbox");
            std::fs::create_dir_all(&dir).expect("create inbox");

            let request = crate::background::control::SteerRequest {
                kind: "steer".to_string(),
                id: "req-1".to_string(),
                ts: 1,
                message: "look at the retry path".to_string(),
                mode: None,
                target_index: None,
                source: None,
            };
            // Written BEFORE the gate goes up: this itself needs the blocking pool
            // (`control.rs:799` → `tokio::fs::create_dir_all`), and it is what forces the pool's
            // single thread into existence.
            crate::background::control::write_steer_request_to_dir(&dir, &request)
                .await
                .expect("write request");

            let inbox = Arc::new(SteeringInbox::new(dir.clone(), None, None, 0));
            // `can_steer` is what the turn-lifecycle events set; without it `flush` returns before
            // the latch is even reached and this test would prove nothing.
            inbox.state.lock().expect("not poisoned").can_steer = true;

            // ---- the gate: occupy the pool's only thread ------------------------------------
            // `spawn_blocking` dispatches at CALL time, so once `started_rx.recv()` has returned
            // the single pool thread is provably inside this closure and every later blocking task
            // is queued, never run.
            let (started_tx, started_rx) = mpsc::channel::<()>();
            let (release_tx, release_rx) = mpsc::channel::<()>();
            let gate = tokio::task::spawn_blocking(move || {
                started_tx.send(()).expect("gate handshake");
                let _ = release_rx.recv();
            });
            started_rx.recv().expect("the blocking pool's only thread is occupied");

            {
                let fut = inbox.flush();
                let mut fut = std::pin::pin!(fut);
                let mut cx = Context::from_waker(Waker::noop());
                assert!(
                    matches!(fut.as_mut().poll(&mut cx), Poll::Pending),
                    "with the one-thread blocking pool gated, `flush`'s first await \
                     (`tokio::fs::read_dir` → `asyncify` → `spawn_blocking`) cannot have \
                     completed, so this poll must park mid-body"
                );
                assert!(
                    inbox.state.lock().expect("not poisoned").flushing,
                    "the latch must be held at the drop point, or the test is not testing anything"
                );
            } // `fut` dropped here, mid-body, parked at `read_dir`.

            assert!(
                !inbox.state.lock().expect("not poisoned").flushing,
                "a dropped flush must release the re-entrancy latch (pi's `finally`)"
            );

            // Reopen the gate. The `read_dir` task the dropped future left queued runs first and is
            // inert: it only opens the directory and buffers entries — removal happens later, at
            // `control.rs:1042`, which that future never reached.
            drop(release_tx);
            gate.await.expect("gate task");

            // The consequence: a later flush still drains the inbox. No services are bound, so each
            // request is acknowledged `failed` and consumed rather than injected — either way it
            // must LEAVE the directory, which a wedged latch would prevent.
            inbox.flush().await;
            let remaining_after = std::fs::read_dir(&dir)
                .expect("read inbox")
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                .count();
            assert_eq!(
                remaining_after, 0,
                "the request must be consumed by the flush that follows a dropped one"
            );
        });
    }
```

### Rejected alternatives, and why

* **Poll in a loop until `Pending`.** Does not help: the failure mode is the future *completing*, and
  a completed future has no drop point. Looping converts a panic into a silent no-op test.
* **`tokio::task::yield_now()` / `sleep` before the poll.** Changes the odds, not the guarantee — the
  race is with a separate OS thread, so no amount of yielding on the test thread makes the pool lose
  it.
* **`flavor = "multi_thread"`.** Irrelevant. The blocking pool is orthogonal to the worker pool.
* **Weakening `Poll::Pending` to "Pending or Ready".** Deletes the test. Not an option.

---

## 2. `cyrup-modes tests::modes::rpc_cycle_model_spans_the_full_auth_filtered_registry`

Long-standing and documented
([13-cyrup-mcp-STATUS.md:106-108](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md), also `:205`,
`:331`). Fails on any host with ambient AWS credentials — **including this container**, where both
`AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` are exported.

### The exact failure path, traced

The test ([modes.rs:1669-1717](../../crates/cyrup-modes/src/tests/modes.rs)) writes an `auth.json`
credentialing `anthropic`, launches on a one-model `FauxProvider`
([faux.rs:217-233](../../crates/cyrup-provider/src/faux.rs) — `FauxConfig::default().models` is a
single definition), then asserts that `cycle_model` steps onto `anthropic`.

1. `cycle_model` → `cycle_available_model`
   ([session.rs:4443-4452](../../crates/cyrup-session-svc/src/session.rs),
   [session.rs:4503-4527](../../crates/cyrup-session-svc/src/session.rs)) walks
   `available_model_catalog()`.
2. `available_model_catalog` is `full_model_registry().filter(has_configured_auth)`
   ([session.rs:3191-3193](../../crates/cyrup-session-svc/src/session.rs)).
3. `has_configured_auth` → `provider_has_configured_auth`
   ([session.rs:3055-3071](../../crates/cyrup-session-svc/src/session.rs),
   [session.rs:3085-3092](../../crates/cyrup-session-svc/src/session.rs)) →
   `cyrup_config::provider_is_configured`
   ([model.rs:2298-2306](../../crates/cyrup-config/src/model.rs)) → `AuthStore::has_auth`
   ([auth.rs:319-327](../../crates/cyrup-config/src/auth.rs)).
4. `has_auth`'s third tier is `crate::env_keys::get_env_api_key(provider, env)`
   ([auth.rs:326](../../crates/cyrup-config/src/auth.rs)) with `env == None`
   ([session.rs:3086-3091](../../crates/cyrup-session-svc/src/session.rs)), which resolves through
   `Ambient::Process` ([env_keys.rs:168-169](../../crates/cyrup-config/src/env_keys.rs)) — i.e. the
   **real process environment**.
5. `get_env_api_key_in`'s Bedrock arm
   ([env_keys.rs:193-205](../../crates/cyrup-config/src/env_keys.rs)) returns `"<authenticated>"`
   when `AWS_ACCESS_KEY_ID` **and** `AWS_SECRET_ACCESS_KEY` are both present. On this host they are.
   So `amazon-bedrock` is "configured" and its whole catalog joins the candidate list.
6. Ordering makes that fatal rather than merely noisy. `full_model_registry`
   ([session.rs:3144-3183](../../crates/cyrup-session-svc/src/session.rs)) puts the installed
   provider's models first (one faux model), then `default_models(..).get_models(None)`, which
   iterates `Models::providers` — a `BTreeMap<String, Arc<dyn Provider>>`
   ([collection.rs:135-139](../../crates/cyrup-provider/src/collection.rs),
   [collection.rs:188-201](../../crates/cyrup-provider/src/collection.rs)), i.e. **lexicographic by
   provider id**. `"amazon-bedrock"` sorts before `"anthropic"`.
7. The session's current model is the faux one, so `cur_idx == 0` and forward cycling picks
   `candidates[1]` — an `amazon-bedrock` model. The assertion at
   [modes.rs:1706-1710](../../crates/cyrup-modes/src/tests/modes.rs)
   (`cycled["data"]["model"]["provider"] == "anthropic"`) fails.

The fixture's own doc comment already admits this ("these fixtures are NOT hermetic against the
ambient environment", [modes.rs:58-60](../../crates/cyrup-modes/src/tests/modes.rs)).

### The pattern to follow — already in the tree

[env_keys.rs](../../crates/cyrup-config/src/env_keys.rs) solved exactly this for the seven
host-dependent tests fixed in PR #30: an injectable `Ambient` tier
([env_keys.rs:27-44](../../crates/cyrup-config/src/env_keys.rs)) — `Process` vs `Fixed(&HashMap)` —
behind an **unchanged** public API, with `find_env_keys` / `get_env_api_key`
([env_keys.rs:130-131](../../crates/cyrup-config/src/env_keys.rs),
[env_keys.rs:168-169](../../crates/cyrup-config/src/env_keys.rs)) delegating to `_in` variants.

`AuthStore` needs the same shape, with one difference forced by the call site: the consumer here is
a **different crate**, so the fixed tier cannot be a `cfg(test)`-only enum variant constructed
in-crate. It has to be state on the store instance, and the store already has a public injection
seam for exactly this kind of thing (`set_runtime_api_key`,
[auth.rs:139](../../crates/cyrup-config/src/auth.rs)).

### The change

**(a) [env_keys.rs](../../crates/cyrup-config/src/env_keys.rs) — widen visibility only.** No logic
changes. `Ambient` is now constructed in production builds (by `AuthStore`), so the `cfg(test)`
dead-code shim on `Fixed` goes away:

```rust
#[derive(Clone, Copy)]
pub(crate) enum Ambient<'a> {
    /// Production: read the real process environment.
    Process,
    /// An injected fixture, so "unset" is a property of the fixture and not of the host. Reached in
    /// production builds via [`crate::auth::AuthStore::with_ambient_env`], which is how a consumer
    /// in ANOTHER crate — `cyrup-modes`' `rpc_cycle_model_spans_the_full_auth_filtered_registry` —
    /// gets the same hermeticity this module's own unit tests get.
    Fixed(&'a HashMap<String, String>),
}
```

and both `_in` helpers become crate-visible:

```rust
pub(crate) fn find_env_keys_in(
    provider: &str,
    env: Option<&HashMap<String, String>>,
    ambient: Ambient<'_>,
) -> Option<Vec<String>> { /* unchanged */ }

pub(crate) fn get_env_api_key_in(
    provider: &str,
    env: Option<&HashMap<String, String>>,
    ambient: Ambient<'_>,
) -> Option<String> { /* unchanged */ }
```

**(b) [auth.rs](../../crates/cyrup-config/src/auth.rs) — the store owns its ambient tier.** Add the
field to the struct at [auth.rs:65-79](../../crates/cyrup-config/src/auth.rs):

```rust
pub struct AuthStore {
    path: PathBuf,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    runtime: RwLock<HashMap<String, String>>,
    cached: RwLock<AuthFile>,
    /// The AMBIENT environment tier this store's env fallback reads (Pi
    /// `getProviderEnvValue`'s `process.env` half). `None` — the default, and the only value
    /// production ever uses — is the real process environment.
    ///
    /// This exists because without it the `env` argument threaded through [`Self::has_auth`] /
    /// [`Self::get_api_key`] is only half a seam: `provider_env_value` is
    /// `env?.[name] || process.env[name]`, so an overlay can ADD a value but cannot express
    /// "this variable is unset". Every caller passing `None` — including
    /// `AgentSession::provider_has_configured_auth` (`cyrup-session-svc/src/session.rs:3085-3092`),
    /// which is the whole `/model` and `cycle_model` candidate filter — therefore answered from
    /// the machine running the process. That is not hypothetical: a host with ambient
    /// `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` makes `amazon-bedrock` configured and puts
    /// its 109-model catalog into every available-model set.
    ///
    /// It cannot be worked around by scrubbing the environment: this crate is
    /// `#![forbid(unsafe_code)]` and `std::env::remove_var` is unsafe in Rust 2024, and a scrub
    /// would be process-global — wrong under any parallel test runner.
    ambient: Option<HashMap<String, String>>,
}
```

initialise it in `at` ([auth.rs:88-97](../../crates/cyrup-config/src/auth.rs)) with `ambient: None`,
and add the builder plus the tier accessor:

```rust
    /// Pin the AMBIENT environment tier to a fixed map instead of the process environment.
    ///
    /// The seam `cyrup_config::provider_is_configured` needs to be answerable from a fixture rather
    /// than from the host. Pass `HashMap::new()` for "no ambient credentials at all", which is what
    /// a hermetic fixture wants; the scoped `env` argument on [`Self::has_auth`] /
    /// [`Self::get_api_key`] keeps its usual precedence over this tier.
    #[must_use]
    pub fn with_ambient_env(mut self, env: HashMap<String, String>) -> Self {
        self.ambient = Some(env);
        self
    }

    /// This store's ambient tier, as [`crate::env_keys`] wants it.
    pub(crate) fn ambient_tier(&self) -> crate::env_keys::Ambient<'_> {
        match self.ambient.as_ref() {
            Some(map) => crate::env_keys::Ambient::Fixed(map),
            None => crate::env_keys::Ambient::Process,
        }
    }
```

Then route the store's two env reads through it. `has_auth`
([auth.rs:326](../../crates/cyrup-config/src/auth.rs)) — signature **unchanged**:

```rust
        crate::env_keys::get_env_api_key_in(provider.as_str(), env, self.ambient_tier()).is_some()
```

and `get_api_key`'s fallback ([auth.rs:389](../../crates/cyrup-config/src/auth.rs)) — signature
**unchanged**:

```rust
        Ok(crate::env_keys::get_env_api_key_in(provider.as_str(), env, self.ambient_tier()))
```

**(c) [login.rs:331](../../crates/cyrup-config/src/login.rs) — one line, for consistency.**
`provider_auth_status` already takes `store: &AuthStore`
([login.rs:309-313](../../crates/cyrup-config/src/login.rs)) and reports the `environment` tier; if
it kept reading `Ambient::Process` the same store would give two different answers about what is
set:

```rust
    if let Some(name) = crate::env_keys::find_env_keys_in(provider.as_str(), env, store.ambient_tier())
        .and_then(|keys| keys.into_iter().next())
```

The free functions `find_env_keys` / `get_env_api_key` keep their `Ambient::Process` bodies, so
every caller that does not hold an `AuthStore` is untouched.

**(d) [modes.rs](../../crates/cyrup-modes/src/tests/modes.rs) — hand the test's store to the
factory.** The plumbing already exists: `SessionFactory::auth`
([factory.rs:86-91](../../crates/cyrup-session-svc/src/factory.rs)) is forwarded to the builder at
[factory.rs:162-164](../../crates/cyrup-session-svc/src/factory.rs), and the builder only falls back
to `AuthStore::at(cfg.agent_dir.join("auth.json"))` when none was supplied
([builder.rs:540-545](../../crates/cyrup-session-svc/src/builder.rs),
[builder.rs:692-695](../../crates/cyrup-session-svc/src/builder.rs)). `cyrup-config` is already a
dev-dependency of `cyrup-modes` (`Cargo.toml:35-39`).

In `rpc_cycle_model_spans_the_full_auth_filtered_registry`, replace the factory construction at
[modes.rs:1680-1687](../../crates/cyrup-modes/src/tests/modes.rs):

```rust
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let cfg = base_config(&fx);
    let target = cfg.target.clone();
    // The env tier of `provider_is_configured` is pinned to an EMPTY ambient map, so "amazon-bedrock
    // has no credential" is a property of this fixture rather than of the machine. Without it, a
    // host exporting `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` completes Bedrock's IAM pair
    // (`cyrup-config/src/env_keys.rs:193-205`), `has_auth` reports it configured
    // (`cyrup-config/src/auth.rs:319-327`), and its whole catalog enters
    // `available_model_catalog` — AHEAD of anthropic, because `Models::providers` is a `BTreeMap`
    // keyed by provider id (`cyrup-provider/src/collection.rs:135-139,188-201`) and
    // `"amazon-bedrock" < "anthropic"`. `cycle_model` then steps onto a Bedrock model and the
    // assertion below fails on a host that has nothing to do with what is under test.
    let auth = Arc::new(
        cyrup_config::AuthStore::at(fx.agent_dir.join("auth.json"))
            .with_ambient_env(std::collections::HashMap::new()),
    );
    let factory = Arc::new(
        SessionFactory::new(provider, cfg)
            .auth(auth)
            .provider_resolver(Arc::new(AnyFauxResolver) as Arc<dyn cyrup_session_svc::ProviderResolver>),
    );
    let runtime = AgentSessionRuntime::create(factory, target).await.expect("build runtime");
```

Ordering matters and is already right: the `auth.json` write at
[modes.rs:1671-1678](../../crates/cyrup-modes/src/tests/modes.rs) precedes this, and `AuthStore::at`
calls `reload()` in its constructor ([auth.rs:88-97](../../crates/cyrup-config/src/auth.rs)), so the
stored `anthropic` credential is in the snapshot.

Do the same in `rpc_model_commands_span_the_full_auth_filtered_registry`
([modes.rs:1588-1657](../../crates/cyrup-modes/src/tests/modes.rs)) and in `build_runtime`
([modes.rs:61-70](../../crates/cyrup-modes/src/tests/modes.rs)) if — and only if — leaving them on
the process tier keeps any of them host-dependent; `build_runtime`'s doc comment at
[modes.rs:58-60](../../crates/cyrup-modes/src/tests/modes.rs) should be updated or deleted to match
whatever is true after the change.

### Explicitly not this

**Do not scrub the environment around the test.** `std::env::remove_var` is `unsafe` in Rust 2024,
`cyrup-config` is `#![forbid(unsafe_code)]`, and a scrub is process-global — it works exactly until
something else runs in the same process, which is precisely the class of bug being fixed.

### Note on blast radius

`AuthStore` is the only store gaining the field, and the ambient default stays `Process`, so nothing
shipped changes behaviour. Two existing in-crate assertions —
[auth.rs:827](../../crates/cyrup-config/src/auth.rs) (`assert!(!s.has_auth(&openai, Some(&empty)))`)
and its siblings at `:854-871` — carry the same latent host dependency (an ambient `OPENAI_API_KEY`
would flip them) and can be pinned by constructing their fixture store with
`.with_ambient_env(HashMap::new())`. That is a one-line change per fixture, not a new suite; make it
while the seam is fresh.

The `models.json` tier of `provider_is_configured`
([model.rs:2319-2335](../../crates/cyrup-config/src/model.rs) →
`config_value::resolve_env_value`, [config_value.rs:146-157](../../crates/cyrup-config/src/config_value.rs))
also falls back to the process env, but it is **inert here**: the fixture writes no `models.json`, so
`models_json.providers.get(id)` is `None` and the tier returns `false` before any env read. Leave it
alone.

---

## Definition of done

- [ ] The `prompt_runtime` classification is recorded as a **flake**, backed by a repeat run of the
      current test (≈50 iterations) showing only the `:2654` precondition can fail, and by the
      `FlushGuard` ablation showing the latch assertion still catches a real regression.
- [ ] `a_dropped_flush_future_does_not_wedge_the_steering_inbox` reaches its await point by
      construction — one-thread blocking pool, gated — with `Poll::Pending`, the held-latch
      assertion, the released-latch assertion and the drained-inbox assertion all intact and
      unweakened.
- [ ] `AuthStore` carries an injectable ambient tier (`with_ambient_env` +
      `ambient_tier`), `has_auth` and `get_api_key` keep their exact public signatures, and
      `env_keys`' free functions still resolve through `Ambient::Process`.
- [ ] `rpc_cycle_model_spans_the_full_auth_filtered_registry` passes **with**
      `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` exported, and still passes with them unset.
- [ ] No `std::env::set_var` / `remove_var` added anywhere, and no `#![forbid(unsafe_code)]` relaxed.
- [ ] `cargo nextest run --workspace` is 7859 run / 7859 passed (8 skipped), repeated twice to
      confirm the flake is gone.
- [ ] `cargo clippy --workspace --all-targets` exit 0 with no new warnings in the touched files.
