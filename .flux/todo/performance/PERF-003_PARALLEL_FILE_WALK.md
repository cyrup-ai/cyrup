---
stage: new
status: pending
updated: 2026-08-29 02:33
---

# Parallelise the file walk in `grep` and `find`

> **This is a parity regression, not an optimisation.** pi's `grep` *is* ripgrep — it
> spawns the real binary ([`grep.ts:177,226`](../../../../pi/packages/coding-agent/src/core/tools/grep.ts)),
> which walks and searches across every core. cyrup reimplements the search in-process with
> `ignore::WalkBuilder` + `grep-searcher` and uses the **serial** `Walk` iterator, one file
> at a time, on one thread. On a large tree cyrup's `grep` is slower than pi's.

---

## 0. READ THIS FIRST — the ordering guarantee that makes this non-trivial

`WalkBuilder::build_parallel()` is not a drop-in for `.build()`. The serial walk is
**pre-order**, and [`grep.rs:756-761`](../../../crates/cyrup-tools/src/tools/grep.rs)
depends on that:

> Directory roots the override PRUNED. `ignore::Walk` prunes internally with
> `skip_current_dir()` on a private iterator and exposes no skip handle to a consumer
> (walk.rs:1131-1145), so the prune is reproduced here: **the walk is pre-order, so a
> directory always arrives before its contents**, and every later item beneath a pruned
> root is dropped.

`WalkParallel` gives no such ordering — a file can be yielded before the directory that
would have pruned it. Porting the prune verbatim onto a parallel walk **silently changes
the match set**, and no existing test would catch it because the current tests run against
a deterministic serial order.

This is the whole difficulty of the task. Everything else is mechanical.

**The prune must move into the walker, not stay downstream of it.** `WalkParallel`'s
visitor returns `ignore::WalkState`, and `WalkState::Skip` from a directory entry prunes
that subtree inside the walker — which is what `Walk` was doing privately all along. The
fix is to express the override prune as a `WalkState::Skip` decision rather than as a
post-hoc `pruned: Vec<PathBuf>` filter. Done that way the parallel walk is *more* faithful
to ripgrep than the serial one, because it is what ripgrep itself does.

---

## 1. Where it is

| site | what |
| --- | --- |
| [`ops/local/fs.rs:288-…`](../../../crates/cyrup-tools/src/ops/local/fs.rs) | `fn walk(&self, root, opts) -> EventStream<Result<WalkItem, ToolError>>` — the single implementation. `spawn_blocking` + `mpsc::channel(256)`, then `WalkBuilder…build()` (serial) |
| [`ops/mod.rs:501`](../../../crates/cyrup-tools/src/ops/mod.rs) | the trait declaration |
| [`tools/grep.rs:762`](../../../crates/cyrup-tools/src/tools/grep.rs) | `self.fs.walk(...)` — the pre-order-dependent consumer |
| [`tools/grep.rs:302-316`](../../../crates/cyrup-tools/src/tools/grep.rs) | per-file `SearcherBuilder…build()` inside `spawn_blocking`, one file at a time, fused into the walk |
| [`tools/find.rs:165`](../../../crates/cyrup-tools/src/tools/find.rs) | the other direct consumer |
| [`isolation/protected.rs:155`](../../../crates/cyrup-tools/src/isolation/protected.rs), [`isolation/traversal.rs:139`](../../../crates/cyrup-tools/src/isolation/traversal.rs), [`grep.rs:1101,1614`](../../../crates/cyrup-tools/src/tools/grep.rs) | delegating wrappers — they forward `walk` and need no change beyond the signature |

The walk is already off the reactor (`spawn_blocking`) and already back-pressured
(`mpsc::channel(256)`). **The async plumbing is fine.** Only the walker's own concurrency
is missing.

## 2. The measurement

`rg` on this repo (`crates/`, ~250k LOC), warm page cache, 8 cores:

| | wall |
| --- | --- |
| `rg --threads 1` (cyrup's shape) | **31 ms** |
| `rg --threads 8` (pi's shape) | **11 ms** |

**2.8× floor.** It is a floor for three reasons: this repo is small, the cache was warm,
and `rg -j1` still overlaps its own walk and search where cyrup's fused loop does not. On
a cold monorepo the gap is wider.

Reproduce:

```bash
rg -c impl crates/ >/dev/null 2>&1   # warm the cache
for t in 1 8; do echo "--- -j$t ---"; ( time rg --threads $t -c "impl" crates/ >/dev/null ) 2>&1 | grep real; done
```

---

## 3. Required implementation

### 3a. Move the prune inside the walker

In [`grep.rs`](../../../crates/cyrup-tools/src/tools/grep.rs), replace the downstream
`pruned: Vec<PathBuf>` filter with a prune *predicate* passed through `WalkOpts`:

```rust
    /// Directory subtrees the caller's ignore override excludes. Applied INSIDE the
    /// walker (`WalkState::Skip` on a matching directory), not downstream of it.
    ///
    /// It used to be a post-hoc `Vec<PathBuf>` filter in `grep.rs`, which was only sound
    /// because `ignore::Walk` is pre-order — a directory always arrived before its
    /// contents. `WalkParallel` gives no such ordering, so the same filter would drop a
    /// different set of files depending on thread interleaving. Expressing it as a skip
    /// decision is both order-independent and what ripgrep itself does.
    pub prune: Option<Arc<dyn Fn(&Path) -> bool + Send + Sync>>,
```

`Arc<dyn Fn>` rather than a closure generic because `WalkOpts` crosses a `dyn`-object trait
method (`ops/mod.rs:501`) and is cloned into each walker thread.

### 3b. Switch `fs.rs`'s walk to `build_parallel()`

Keep every builder knob in
[`fs.rs:296-334`](../../../crates/cyrup-tools/src/ops/local/fs.rs) exactly as it is — the
`$RIPGREP_CONFIG_PATH` handling, `require_git`, `parents(!opts.no_ignore)`, the custom
ignore-file registration. They are all `WalkBuilder` settings and apply identically to a
parallel walk. Only the terminal call and the yield loop change:

```rust
            builder.build_parallel().run(|| {
                let tx = tx.clone();
                let prune = opts.prune.clone();
                Box::new(move |entry| {
                    match entry {
                        Ok(e) if e.file_type().is_some_and(|t| t.is_dir()) => {
                            if prune.as_ref().is_some_and(|p| p(e.path())) {
                                // Prunes the subtree INSIDE the walker — the ordering-safe
                                // replacement for grep.rs's old post-hoc filter.
                                return ignore::WalkState::Skip;
                            }
                        }
                        _ => {}
                    }
                    // A closed receiver means the consumer hit its limit or was cancelled:
                    // `Quit`, not `Continue`, so the remaining threads stop immediately.
                    match tx.blocking_send(/* WalkItem or ToolError */) {
                        Ok(()) => ignore::WalkState::Continue,
                        Err(_) => ignore::WalkState::Quit,
                    }
                })
            });
```

Note `blocking_send`, not `send`: the visitor runs on `WalkParallel`'s own threads, which
are not tokio workers, and the bounded channel must keep applying back-pressure.

### 3c. Parallelise the search too

Today [`grep.rs:302`](../../../crates/cyrup-tools/src/tools/grep.rs) builds one `Searcher`
and searches one file per `spawn_blocking`. A `Searcher` is cheap but not `Sync`; build one
**per walker thread** (`thread_local!` or one per visitor closure, which `build_parallel`'s
`||` factory already gives you naturally) and search inside the visitor, so walk and search
overlap the way ripgrep's do.

The global match `limit` then becomes shared state: an `AtomicUsize` the visitor
decrements, returning `WalkState::Quit` when it reaches zero.

### 3d. Preserve output determinism

ripgrep's parallel walk yields in nondeterministic order; `rg` sorts only under `--sort`.
Two constraints:

- **cyrup must keep whatever ordering it emits today**, because a tool result the model
  reads should not vary run to run for the same tree. Collect results and sort by path
  before rendering, in `grep.rs`/`find.rs`, not in the walker.
- **The global `limit` interacts with ordering.** With a serial pre-order walk, "first N
  matches" is deterministic. With a parallel walk it is not. Decide explicitly: either
  collect-then-truncate in sorted order (deterministic, but pays for matches it discards)
  or keep first-N-arrived (fast, nondeterministic). The comment at
  [`grep.rs:747-755`](../../../crates/cyrup-tools/src/tools/grep.rs) — about the limit and
  rejection happening at close, "after rg has walked the whole tree" — is the relevant
  precedent and suggests collect-then-truncate matches pi. **Choose, and write the choice
  into the doc comment.**

---

## 4. Order of work

1. `WalkOpts::prune` + move `grep.rs`'s prune into it, still on the **serial** walk. Match
   set must be identical before any parallelism lands — that is the whole safety argument.
2. `build_parallel()` in `fs.rs` (§3b).
3. Per-thread searcher + atomic limit (§3c).
4. Ordering decision + `find.rs` (§3d).

Step 1 alone is a behaviour-preserving refactor that can be verified independently. Do not
merge 1 and 2.

---

## 5. Definition of Done

1. **The match set is byte-identical to the serial walk**, for the same tree and query,
   including with an ignore override that prunes a directory whose children would
   otherwise match — run repeatedly, since a wrong answer here is interleaving-dependent
   and may not reproduce on the first try.
2. **Results are deterministic across runs.** The same query on the same tree returns the
   same matches in the same order, every time.
3. **All of ripgrep's config surface still applies.** `$RIPGREP_CONFIG_PATH`'s
   `--no-ignore`, `--no-ignore-vcs`, `--max-depth`, `--follow`, `--one-file-system`,
   `--max-filesize` and `--ignore-file` behave exactly as they do today, including
   `--ignore-file` surviving `--no-ignore` and the parent-traversal interaction at
   [`fs.rs:310-334`](../../../crates/cyrup-tools/src/ops/local/fs.rs).
4. **Cancellation is immediate.** A cancelled or limit-reached search stops every walker
   thread promptly (`WalkState::Quit`), does not block on a full channel, and leaves no
   detached threads.
5. **It is actually faster.** Wall-clock on a cold-cache tree of ≥100k files improves by
   ≥2× against the current serial implementation on an 8-core host.
6. **`find` gets the same treatment**, or a doc comment saying why it does not.
7. **The suite is green under the real gate:**
   `cargo test --workspace --features test-fixtures --no-fail-fast`, and
   `cargo clippy --workspace --all-targets --features test-fixtures` exits **0**.
