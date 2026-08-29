---
stage: qa
status: completed
updated: 2026-08-30 07:40
qa_rating: 10/10
result: "grep no-match 172.0ms -> 18.9ms (9.1x); grep \"fn \" limit 100000 218.9ms -> 53.4ms (4.1x);
  walk only 18.0ms -> 6.0ms; capped common case 1.6ms -> 1.9ms (no regression)."
gates: "clippy --workspace --all-targets --features test-fixtures (0 errors, 0 new warnings);
  test --workspace --features test-fixtures --no-fail-fast (8331 passed, 0 failed);
  doc --workspace --no-deps (0 warnings)."
---

# Parallelise the file walk in `grep` and `find` — DONE

The walk is parallel, the search is batched, and the per-file scheduler round-trip that dominated
the runtime is gone. Measured on this host (4 cores, `--release`, `crates/` = 1,885 entries,
median of 5, seven repetitions) with the probe at [`tmp/perf003/`](../../../tmp/perf003/src/main.rs),
which drives the REAL `GrepTool::execute` and the REAL `LocalFs::walk`:

| case | before | after | |
| --- | --- | --- | --- |
| `grep` no-match, default limit | 172.0 ms | **18.9 ms** | **9.1×** |
| `grep "fn "` limit 100 000 | 218.9 ms | **53.4 ms** | 4.1× |
| walk only | 18.0 ms | **6.0 ms** | 3.0× |
| `grep "fn "` default limit 100 | 1.6 ms | 1.9 ms | no regression |

## What shipped

- **`WalkOpts::prune` + `WalkBuilder::filter_entry`.** The override prune moved INSIDE the walker.
  It had been a post-hoc `Vec<PathBuf>` scan in `grep.rs`, sound only because `ignore::Walk` is
  pre-order; a parallel walk gives no such ordering. Landed on the serial walk first and proven
  behaviour-identical, which was the whole safety argument for the next step.
- **`LocalFs::walk` splits serial/parallel** on `sort_by_path`, behind ONE shared builder and ONE
  shared `to_walk_item`, so the two branches cannot drift in which paths they visit.
  `--sort`/`--sortr` stay serial because `sort_by_file_path` "is not used in the parallel
  iterator" and, with a cap in force, direction decides WHICH matches come back.
- **`FsOps::read_streams`** — a batched open on the seam. Its default implementation loops
  `self.read_stream`, which on a decorator is that decorator's own method, so an isolation wrapper
  that never overrides it still confines every path: overriding buys speed, forgetting costs speed
  and nothing else. `TraversalFs` confines every path then delegates; `ProtectedFs` forwards
  unguarded, matching its `read_stream`.
- **`search_batch`** — one seam call, one `spawn_blocking`, one `Searcher` reused across the batch.
  Batch size is derived from observed match density and bounded by a doubling, so a dense pattern
  holds at 1 and behaves exactly like the unbatched shape while a sparse one reaches the cap size
  within five completions.
- **Deterministic output** — rows in path order (walk order under `--sortr`), trimmed to the cap by
  MATCHES; the walk error is the lexicographically smallest rather than the first.

## The measurements that changed the design

1. **The win was in the SEARCH, not the walk.** The walk was 18 ms of a 172 ms search, so §4b alone
   was worth 8%. The original brief's ~2.2× came from `rg -j1` vs `rg -j8` as a stand-in for
   cyrup's own code.
2. **Two `spawn_blocking` round-trips per file, not one.** `LocalFs::read_stream` did one just to
   reach `File::open`, and it had to finish before the search could start. 3,770 round-trips at
   ~31 µs = ~30 ms at concurrency, against 2.9 ms of real `open(2)` and 21 ms of real searching —
   41% of runtime. Running several per-file futures at once does not remove them; only batching
   does.
3. **mmap is a red herring.** `search_path` (18.1 ms) is no faster than `search_reader` (17.9 ms)
   here, so routing every read through the `FsOps` seam costs nothing and the isolation constraint
   was never in tension with the performance.
4. **Over-dispatch is asymmetrically expensive.** Sizing batches from the density estimate alone
   sent the common capped search 1.9 ms → 3.0 ms: one sparse file among the first few predicts
   thousands still needed and jumps straight to the maximum. Bounding growth to a doubling, with
   the estimate acting only as a ceiling, fixed it.

## Residuals, recorded rather than hidden

- The absolute floor for this tree is ~10 ms (walk + search with no channel at all). The remaining
  gap from 18.9 ms is the walk stream and its `mpsc` plumbing, not the search. Worth re-measuring
  before deciding it is worth chasing.
- Above `FD_MAX_BUFFER_LENGTH` results, `find` emits in arrival order and that order is now
  nondeterministic — exactly as fd's own is, since fd's walk is `ignore::WalkParallel` too. Noted
  in `find.rs`.
- The pre-existing `question_mark` warning at `cyrup-tui/src/markdown/highlight.rs:363` is
  untouched: a different crate, out of scope for this task.
