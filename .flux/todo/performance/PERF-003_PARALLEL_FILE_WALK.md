---
stage: aug
status: done
updated: 2026-08-29 04:22
---

# Parallelise the file walk in `grep` and `find`

> **This is a parity regression, not an optimisation.** pi's `grep` *is* ripgrep — it
> spawns the real binary ([`grep.ts:177,226`](../../../../pi/packages/coding-agent/src/core/tools/grep.ts)),
> which walks and searches across every core. cyrup reimplements the search in-process with
> `ignore::WalkBuilder` + `grep-searcher` and uses the **serial** `Walk` iterator, one file
> at a time, on one thread. On a large tree cyrup's `grep` is slower than pi's.

---

## 0. READ THIS FIRST — the premise of the original brief is wrong, and that makes this EASIER

The brief said `ignore` "exposes no skip handle to a consumer", so the override prune had to be
reproduced downstream, and that a parallel walk would therefore need `WalkState::Skip` inside the
visitor. **`ignore` 0.4.26 does expose a skip handle: `WalkBuilder::filter_entry`.** Verified in
the vendored source at `~/.cargo/registry/src/*/ignore-0.4.26/src/walk.rs`:

| site | what it proves |
| --- | --- |
| `walk.rs:973-979` | `pub fn filter_entry<P>(&mut self, filter: P)` where `P: Fn(&DirEntry) -> bool + Send + Sync + 'static` |
| `walk.rs:516` | stored as `struct Filter(Arc<dyn Fn(&DirEntry) -> bool + Send + Sync + 'static>)` — already the exact `Arc<dyn Fn>` shape the brief wanted to invent |
| `walk.rs:1083-1088` | **serial**: `Walk::skip_entry` consults the filter last, after the ignore matchers, the stdout `skip` and `max_filesize` |
| `walk.rs:1131-1145` | a `false` verdict on a `WalkEvent::Dir` makes `Walk::next` call `it.skip_current_dir()` — *the private prune the brief said was unreachable is exactly what `filter_entry` drives* |
| `walk.rs:1791-1797` | **parallel**: `Worker::generate_work` consults the same predicate and simply never enqueues the `Work`, so the subtree is never descended and the entry is never yielded |
| `walk.rs:1057-1059` | serial root exemption: `skip_entry` returns `Ok(false)` at `ent.depth() == 0` |
| `walk.rs:1355-1400` | parallel root exemption, structurally: roots are pushed onto the stack in `visit()` before any filter runs |
| `walk.rs:616,640,1423` | `self.filter` is cloned into `Walk` *and* into `WalkParallel` — one registration covers both |

> **Read the RIGHT vendored copy — versions are pinned and this is a live trap.** `Cargo.lock`
> resolves **`ignore 0.4.26`** and **`grep-searcher 0.1.16`** — the exact versions every citation in
> this file names. But the registry ALSO holds `ignore-0.4.33` and `grep-searcher-0.1.17` (present,
> unused), and `fs.rs`'s own comments inconsistently cite `0.4.33` for a few `walk.rs`/`dir.rs`
> lines — those numbers do **not** line up with the resolved crate. Verify against
> `~/.cargo/registry/src/*/ignore-0.4.26/`, never `0.4.33`, or you will read shifted lines and
> "disprove" a citation that is correct. Every `ignore 0.4.26` and `grep-searcher 0.1.16` line
> number below was re-checked against the resolved copies at HEAD `8f49433` on 2026-08-29 and is
> exact.

**Consequence:** ONE predicate, registered once on the builder, gives byte-identical prune
semantics on the serial and the parallel walker. There is no `WalkState::Skip` in this task, no
visitor-side prune, and no risk of the two walk modes drifting apart — which matters because
§3b below keeps *both* modes alive.

It also makes the safety argument in §4 real rather than aspirational: step 1 moves the prune
into the walker **while still serial**, and because the same predicate is what the parallel
walker will consult, a step-1 match set that is identical to today's is direct evidence for
step 2.

---

## 1. Three constraints the original brief did not have

### 1a. `--sort` / `--sortr` MUST fall back to the serial walk

`WalkBuilder::sort_by_file_path` and `sort_by_file_name` carry the same doc line at
`walk.rs:899` and `walk.rs:918`: **"Note that this is not used in the parallel iterator."**
`WalkParallel` has no `sort_by` field at all (`walk.rs:1314-1325`). Switching unconditionally to
`build_parallel()` would make [`fs.rs:376-384`](../../../crates/cyrup-tools/src/ops/local/fs.rs)
a silent no-op.

That is not a hypothetical: `$RIPGREP_CONFIG_PATH` can carry `--sort=path` / `--sortr=path`
([`rgconfig.rs:287-297`](../../../crates/cyrup-tools/src/tools/rgconfig.rs)), and with a match cap
in force the direction decides **which** matches come back, not merely their order — the point
made at [`ops/mod.rs:301-306`](../../../crates/cyrup-tools/src/ops/mod.rs) and pinned by
`sortr_reverses_which_match_the_cap_returns`
([`grep.rs:2113-2132`](../../../crates/cyrup-tools/src/tools/grep.rs)).

ripgrep resolves this the same way. `rg --help` (rg 15.1.0, on this box):

> Note that sorting results currently always forces ripgrep to abandon parallelism and run in a
> single thread.

So: **`opts.sort_by_path.is_some()` ⇒ serial `.build()`; otherwise `.build_parallel()`.** Both
branches share the prune predicate and every other builder knob.

### 1b. The SEARCH must NOT move into `LocalFs::walk` — it would bypass the isolation seam

The brief's §3c asked for the search to run inside the walker's visitor. That is a containment
regression, not just a layering one:

- [`TraversalFs::walk`](../../../crates/cyrup-tools/src/isolation/traversal.rs) confines the
  **root** (`traversal.rs:133-145`) — one path, once.
- [`TraversalFs::read_stream`](../../../crates/cyrup-tools/src/isolation/traversal.rs) confines
  **every file it opens** (`traversal.rs:104-108`), and its doc comment records that forgetting
  to forward it is a silent defect.
- `grep` passes `follow_links: rg.follow` ([`grep.rs:780`](../../../crates/cyrup-tools/src/tools/grep.rs)),
  so with `--follow` in the user's config a symlink inside the confined root resolves to a target
  **outside** it. `read_stream`'s per-path `confine` is the only thing that stops those bytes
  being searched.

Searching inside `LocalFs` would read through `LocalFs` directly, with the decorator stack
(`TraversalFs`, `ProtectedFs`, and any future one) sitting uselessly above it. `GrepTool` holds
`fs: Arc<dyn FsOps>` ([`grep.rs:169-174`](../../../crates/cyrup-tools/src/tools/grep.rs)) — the
*wrapped* seam — and must keep reading through it.

**Therefore: parallel WALK in `fs.rs`; parallel SEARCH in `grep.rs`, as a bounded-concurrency
pipeline over the `FsOps::walk` stream.** Walk and search still overlap, which is the actual win;
nothing crosses the seam that does not cross it today.

### 1c. `WalkOpts` derives `Debug`, and `Arc<dyn Fn>` does not implement it

[`ops/mod.rs:314`](../../../crates/cyrup-tools/src/ops/mod.rs) is
`#[derive(Clone, Debug, Default)] pub struct WalkOpts`. A bare
`Option<Arc<dyn Fn(&Path) -> bool + Send + Sync>>` field breaks the `Debug` derive. Wrap it in a
newtype that implements `Debug` by hand (§3a) rather than hand-writing `Debug` for all twelve
fields, which would drift the first time a knob is added.

### Also worth knowing before you start

- `grep_searcher::Searcher` holds three `RefCell`s (`grep-searcher-0.1.16 searcher/mod.rs:597-624`
  — `decode_buffer`, `line_buffer`, `multi_line_buffer`) ⇒ `Send`, `!Sync`. One searcher per
  in-flight file. This is **already** what the code does —
  it is built inside the per-file `spawn_blocking`
  ([`grep.rs:301-334`](../../../crates/cyrup-tools/src/tools/grep.rs)) — so the brief's
  "build one per walker thread" is a no-op. Searcher *construction* is not the bottleneck; the
  one-file-at-a-time pipeline is.
- `WalkParallel` defaults to `available_parallelism().min(12)` threads when `threads == 0`
  (`walk.rs:1434-1440`), which is the default. Do not call `.threads()`.
- `WalkParallel::visit` runs its workers under `std::thread::scope` and joins them all before
  returning (`walk.rs:1406-1430`), so no thread outlives the `spawn_blocking` task.
- `Sender::blocking_send` from the walker threads is legal: those threads are created by
  `std::thread::scope` inside the existing `spawn_blocking` and carry no tokio runtime context.
  It returns `Err` — it does not hang — once the receiver is dropped.
- `ignore::DirEntry::is_dir` is **private** (`pub(crate) fn is_dir`, ignore 0.4.26 `walk.rs:102`),
  so read the type through `e.file_type()`. The existing per-entry code does exactly this at
  [`fs.rs:465-467`](../../../crates/cyrup-tools/src/ops/local/fs.rs) as
  `ty.map(|t| t.is_dir()).unwrap_or(false)`; the §4a `filter_entry` sample below writes the
  equivalent `e.file_type().is_some_and(|t| t.is_dir())`. Both are clippy-clean — `is_some_and`
  currently appears nowhere in the crate, so this introduces it; either form is fine, just match
  the sample within one closure.

---

## 2. Where it is (line numbers re-verified at HEAD `8f49433`, 2026-08-29)

| site | what |
| --- | --- |
| [`ops/local/fs.rs:288-481`](../../../crates/cyrup-tools/src/ops/local/fs.rs) | `fn walk` — the single implementation. `mpsc::channel(256)` at `:289` + `spawn_blocking` at `:291`, builder knobs `:297-435`, `let walker = builder.build()` at **`:437`**, yield loop `:452-479` |
| [`ops/mod.rs:314-352`](../../../crates/cyrup-tools/src/ops/mod.rs) | `WalkOpts` |
| [`ops/mod.rs:501`](../../../crates/cyrup-tools/src/ops/mod.rs) | the trait declaration |
| [`tools/grep.rs:761`](../../../crates/cyrup-tools/src/tools/grep.rs) | `let mut pruned: Vec<PathBuf>` — the post-hoc filter to delete |
| [`tools/grep.rs:762-816`](../../../crates/cyrup-tools/src/tools/grep.rs) | `self.fs.walk(...)` with the full `WalkOpts` |
| [`tools/grep.rs:826-970`](../../../crates/cyrup-tools/src/tools/grep.rs) | the fused walk+search loop; prune check `:879`, prune push `:914` |
| [`tools/grep.rs:209-443`](../../../crates/cyrup-tools/src/tools/grep.rs) | `search_one` — one file per `spawn_blocking`, writing into `&mut out` / `&mut count` |
| [`tools/find.rs:186-273`](../../../crates/cyrup-tools/src/tools/find.rs) | `find`'s walk loop; its cap breaks at `:192`, `biased;` at `:210` |
| [`tools/find.rs:274-303`](../../../crates/cyrup-tools/src/tools/find.rs) | fd's buffered sort — the ordering precedent this task follows |
| [`isolation/protected.rs:150`](../../../crates/cyrup-tools/src/isolation/protected.rs), [`isolation/traversal.rs:133`](../../../crates/cyrup-tools/src/isolation/traversal.rs) | delegating wrappers — unchanged; they forward `WalkOpts` by move |
| `grep.rs:1080` (`FailSecondRead`), `grep.rs:1593` (`RecordingFs`), `tests/*.rs` | mock `FsOps` impls — unaffected, `WalkOpts` gains a field with a `Default` |

The walk is already off the reactor (`spawn_blocking`) and already back-pressured
(`mpsc::channel(256)`). **The async plumbing is fine.** Only the walker's own concurrency and the
consumer's one-file-at-a-time search are missing.

## 3. The measurement

`rg` on this repo (`crates/`, 1691 files, ~250k LOC of code under ~700k total lines — cyrup's doc
comments are large), warm page cache, 8 cores — re-measured 2026-08-29 (`rg 15.1.0`, median of 5,
`rg "fn " crates`):

| | wall |
| --- | --- |
| `rg --threads 1` (cyrup's shape) | **~41 ms** |
| `rg --threads 8` (pi's shape) | **~18 ms** |

**~2.2–2.3× floor** on a small warm tree — the RATIO is the load-bearing claim and it reproduces
across sessions even as the absolutes drift with machine load (the brief's earlier 31/14 ms run
gave the same ~2.2×); wider cold, and wider again on a real monorepo, because `rg -j1` still
overlaps its own walk and search where cyrup's fused loop does not.

---

## 4. Required implementation

### 4a. `WalkOpts::prune`, applied via `filter_entry` — serial walk unchanged

In [`ops/mod.rs`](../../../crates/cyrup-tools/src/ops/mod.rs), beside `WalkOpts`:

```rust
/// A caller-supplied directory prune, applied INSIDE the walker.
///
/// Takes `&Path` and not `ignore::DirEntry` on purpose: `WalkOpts` is the backend-agnostic
/// seam (`FsOps::walk`), and a non-`ignore` backend must be able to honour a prune without
/// depending on ripgrep's walker types.
///
/// A newtype rather than a bare `Option<Arc<dyn Fn…>>` so `WalkOpts` keeps its `Debug` derive:
/// a trait object is not `Debug`, and hand-writing `Debug` for the whole struct would drift the
/// next time a knob is added.
#[derive(Clone)]
pub struct PruneDirs(Arc<dyn Fn(&Path) -> bool + Send + Sync>);

impl PruneDirs {
    pub fn new(f: impl Fn(&Path) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// `true` ⇒ do not descend into this directory, and do not yield it.
    pub fn prunes(&self, dir: &Path) -> bool {
        (self.0)(dir)
    }
}

impl std::fmt::Debug for PruneDirs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PruneDirs(<predicate>)")
    }
}
```

and the field:

```rust
    /// Directory subtrees the caller's ignore override excludes. Applied INSIDE the walker via
    /// `WalkBuilder::filter_entry` (ignore 0.4.26 `walk.rs:973-979`), which BOTH walkers honour:
    /// the serial one turns a `false` verdict on a directory into `skip_current_dir()`
    /// (`walk.rs:1131-1145`) and the parallel one never enqueues the subtree at all
    /// (`walk.rs:1791-1797`).
    ///
    /// It used to be a post-hoc `Vec<PathBuf>` filter in `grep.rs:761,879`, sound only because
    /// `ignore::Walk` is pre-order — a directory always arrived before its contents. A parallel
    /// walk gives no such ordering, so that filter would drop a different set of files depending
    /// on thread interleaving. Expressed as a walker-side prune it is order-independent, and it
    /// is what ripgrep itself does.
    ///
    /// Never applied at depth 0: the search root is not prunable (`walk.rs:1057-1059`).
    pub prune: Option<PruneDirs>,
```

In [`fs.rs`](../../../crates/cyrup-tools/src/ops/local/fs.rs), register it with the other builder
knobs, **before** the terminal call (anywhere in `:297-435`; every `WalkBuilder` setter is a plain
field assignment and they commute — see the note already at `fs.rs:321-325`):

```rust
            // The caller's directory prune. `filter_entry` is the ONLY skip handle `ignore`
            // exposes, and it is honoured identically by `Walk` and `WalkParallel`, which is what
            // lets the two branches below share one prune. The predicate must be total: it is
            // consulted for FILES too, and a `false` there would drop the file.
            if let Some(prune) = opts.prune.clone() {
                builder.filter_entry(move |e| {
                    // Depth 0 is the search root and is never prunable. `Walk::skip_entry`
                    // short-circuits it (walk.rs:1057-1059) and `WalkParallel` pushes roots
                    // before any filter runs, so this guard is belt-and-braces on both — and
                    // load-bearing for any future walker that is neither.
                    if e.depth() == 0 {
                        return true;
                    }
                    if e.file_type().is_some_and(|t| t.is_dir()) {
                        return !prune.prunes(e.path());
                    }
                    true
                });
            }
```

In [`grep.rs`](../../../crates/cyrup-tools/src/tools/grep.rs), build the predicate from the same
two inputs the downstream filter used, and delete `pruned` entirely:

```rust
        // `RgGlob` is not `Clone`, and both the prune predicate and the per-file filter below
        // need it, so it is shared by `Arc` rather than duplicated.
        let glob = glob.map(Arc::new);
        let cfg_override = cfg_override.map(Arc::new);

        // The walker-side replacement for the old `pruned: Vec<PathBuf>` (grep.rs:761,879).
        // ripgrep evaluates the override for directories too (ignore `dir.rs:416-425`) and an
        // Ignore verdict takes the whole subtree; a plain (non-`!`) glob that merely MISSES does
        // not prune, because `Override::matched` guards its whitelist-miss fallback with
        // `!is_dir` (`overrides.rs:106`). `--type`/`--type-not` never prune: `Types::matched`
        // returns `None` for a directory, so a `-t rust` config must still descend into
        // directories that do not look like Rust.
        let prune = (glob.is_some() || cfg_override.is_some()).then(|| {
            let glob = glob.clone();
            let cfg_override = cfg_override.clone();
            let cwd = self.cwd.clone();
            PruneDirs::new(move |dir: &Path| {
                // Same relativisation as the file filter: the glob is anchored at the OVERRIDE
                // ROOT — ripgrep's own cwd — not at the search root, because pi spawns `rg` with
                // no `cwd` option and passes `searchPath` positionally (grep.ts:224).
                let rel = dir.strip_prefix(&cwd).map_or_else(|_| to_posix(dir), to_posix);
                glob.as_ref().is_some_and(|g| g.prunes_dir(&rel))
                    || cfg_override
                        .as_ref()
                        .is_some_and(|o| o.matched(&rel, true).is_ignore())
            })
        });
```

then `prune,` in the `WalkOpts` literal at `grep.rs:762-816`, and in the loop body: delete the
`pruned.iter().any(...)` check (`:879`), delete the whole `if w.is_dir { … pruned.push(…); continue; }`
block (`:895-916`), and restore the cheap `if w.is_dir { continue; }` — no directory is a search
subject any more, and the walker never yields a pruned one.

**Nothing else changes in this step.** The walk is still `.build()`. The match set must be
identical; the only behavioural deltas are that a pruned directory's own entry is no longer
yielded (grep dropped it anyway) and that the O(items × pruned-roots) `starts_with` scan is gone.

### 4b. `build_parallel()` in `fs.rs`, with the serial branch kept for `--sort`

Replace `fs.rs:437-478` — every builder knob above it stays exactly as it is
(`$RIPGREP_CONFIG_PATH` handling, `require_git`, `parents(!opts.no_ignore)`, the custom
ignore-file registration, `max_filesize`, `follow_links`, `same_file_system`): all of them live on
`ig_builder` or on plain fields that `build_parallel` copies verbatim (`walk.rs:625-645`).

```rust
            // `--sort=path` / `--sortr=path` force the SERIAL walk. `WalkBuilder::sort_by_file_path`
            // "is not used in the parallel iterator" (ignore 0.4.26 `walk.rs:899`) and
            // `WalkParallel` has no sort field at all (`walk.rs:1314-1325`), so a parallel walk
            // would silently drop the ordering — and with a match cap in force the ordering
            // decides WHICH matches the caller gets (`ops/mod.rs:301-306`). ripgrep resolves it
            // the same way: "sorting results currently always forces ripgrep to abandon
            // parallelism and run in a single thread" (`rg --help`, rg 15.1.0).
            //
            // Both branches share every setting above, including the `filter_entry` prune, which
            // `Walk` and `WalkParallel` honour identically — so the two differ in ORDER and
            // THREADS only, never in which paths are visited.
            if opts.sort_by_path.is_some() {
                for result in builder.build() {
                    if tx.blocking_send(to_item(result)).is_err() {
                        break;
                    }
                }
                return;
            }

            // Threads default to `available_parallelism().min(12)` (`walk.rs:1434-1440`) — the
            // same heuristic ripgrep uses — so `.threads()` is deliberately not called.
            // `visit()` joins every worker under `std::thread::scope` before returning
            // (`walk.rs:1406-1430`): no thread outlives this `spawn_blocking` task.
            builder.build_parallel().run(|| {
                let tx = tx.clone();
                Box::new(move |result| {
                    // `blocking_send` — these are `std::thread::scope` threads with no tokio
                    // runtime context, so the bounded channel keeps applying back-pressure
                    // instead of panicking. A closed receiver means the consumer hit its cap or
                    // was cancelled, so `Quit` (not `Continue`) stops every remaining thread at
                    // once; a dropped receiver also makes an already-parked `blocking_send`
                    // return `Err` rather than hang.
                    match tx.blocking_send(to_item(result)) {
                        Ok(()) => ignore::WalkState::Continue,
                        Err(_) => ignore::WalkState::Quit,
                    }
                })
            });
```

Lift today's per-entry conversion (`fs.rs:452-477` — the `file_type()` read that feeds `is_dir`
/`is_file`, and `walk_error_message` for the `Err` arm) into one `fn to_item(result: Result<ignore::DirEntry,
ignore::Error>) -> Result<WalkItem, ToolError>` so both branches use it verbatim. Move the long
comment block at `fs.rs:438-451` (per-entry `Err` is non-fatal, message shape, no tool prefix) onto
that function — it is still true of both branches, and the "the walk CONTINUES past it" clause is
now also what `WalkState::Continue` on the `Err` arm means.

Keep the closure panic-free: no `unwrap`, no indexing. `WalkParallel::visit` joins with
`handle.join().unwrap()`, so a panicking visitor would surface as a panic in the blocking task.

### 4c. Overlap the searches — a bounded pipeline in `grep.rs`

Today the loop searches one file, awaits it, then pulls the next
([`grep.rs:826-970`](../../../crates/cyrup-tools/src/tools/grep.rs)). Keep the loop and the
`FsOps` seam; make it keep N searches in flight.

**Refactor `search_one` to RETURN its rows instead of appending to the caller's buffers.** The
render step needs to reorder and trim whole files, which is impossible once rows are already
concatenated:

```rust
/// The rows one MATCH renders to: exactly one at `context == 0`, or the `2·context+1`-line block
/// pi's `formatBlock` emits (grep.ts:250-268). Grouped per match rather than flattened because
/// the global cap counts MATCHES, so a file that overshoots must be trimmed at a match boundary.
struct MatchBlock {
    rows: Vec<String>,
    /// Whether any row in this block hit `GREP_MAX_LINE_LENGTH`. Per-block so the
    /// `Some lines truncated` notice describes only rows that were actually EMITTED — a block
    /// dropped by the cap must not raise it.
    line_truncated: bool,
}

async fn search_one(…, per_file_cap: usize) -> Result<Vec<MatchBlock>, ToolError>
```

`per_file_cap` replaces the old `remaining = limit - count`: with searches in flight
concurrently there is no exact remaining budget at dispatch time, so each file is capped at the
GLOBAL `limit` — the most any single file could contribute — and the excess is trimmed
deterministically at render. Worst-case retention is `concurrency × limit` blocks (12 × 100 at
the defaults). `-m/--max-count` stays on the searcher and stays independent, exactly as
documented at `grep.rs:307-313`.

Both callers then flatten through one helper:

```rust
/// Appends blocks until the GLOBAL match cap is reached, counting MATCHES (not rows) — the axis
/// pi counts on (grep.ts:278-292), and the axis `count >= limit` is tested on everywhere below.
fn take_into(
    out: &mut Vec<String>,
    count: &mut usize,
    any_line_truncated: &mut bool,
    limit: usize,
    blocks: Vec<MatchBlock>,
) {
    for b in blocks {
        if *count >= limit {
            return;
        }
        *count += 1;
        *any_line_truncated |= b.line_truncated;
        out.extend(b.rows);
    }
}
```

The explicit-file branch (`grep.rs:712-737`) becomes one `search_one` + one `take_into`. The walk
branch becomes:

```rust
            // ripgrep's own default and `WalkParallel`'s (ignore `walk.rs:1434-1440`): keep walk
            // and search at the same width so neither starves the other.
            let concurrency = std::thread::available_parallelism().map_or(1, |n| n.get()).min(12);
            let mut inflight = futures::stream::FuturesUnordered::new();
            // Keyed by path so the render order is decided by `Path::cmp`, never by completion
            // order. `PathBuf` and not the rendered `rel`: `Path::cmp` compares COMPONENTS, which
            // is the only ordering stable across runs and platforms (`fs.rs:373-375`).
            let mut collected: Vec<(PathBuf, Vec<MatchBlock>)> = Vec::new();
            let mut found = 0usize;
```

with a `select!` that (a) polls cancel first — `biased`, for the reason already recorded at
[`find.rs:204-210`](../../../crates/cyrup-tools/src/tools/find.rs) — (b) drains a completed
search, and (c) pulls the next candidate only while `found < limit && inflight.len() < concurrency`.
`found` accumulates the match counts of COMPLETED searches only; no atomics are needed, because the
dispatch loop is a single async task and `FuturesUnordered` is polled from it. When the walk ends,
drain `inflight` to completion. When `found >= limit`, stop dispatching, drain what is already in
flight, and drop `walk` — dropping the receiver is what makes every walker thread return
`WalkState::Quit` (§4b).

Each in-flight search is still one `tokio::task::spawn_blocking`
([`grep.rs:301`](../../../crates/cyrup-tools/src/tools/grep.rs)); 12 of them is nothing against
tokio's 512-thread blocking pool. The `Searcher` stays built inside that task — it holds `RefCell`s
and is `!Sync`, so it must never be shared, and it is cheap enough that per-file construction is not
worth removing.

The `?` on a search result still propagates `error::aborted()` out of `execute`. Drop `inflight`
on that path — the `spawn_blocking` tasks own their `CancelToken` clones and stop themselves
through `CancelReader`/`MatchSink` (`grep.rs:292-300`, `:462-492`).

### 4d. Deterministic output — and the walk error

**Decision, to be written into the doc comment: collect per file, sort by `Path::cmp`, then trim
to the cap. The walk stays bounded by the cap; it is NOT drained.**

```rust
            // Rows are emitted in path order, never in completion order. pi cannot be copied here
            // — pi IS rg's parallel walk, so pi's own row order varies run to run — and a tool
            // result the model reads must not. `find` already resolves this the same way
            // (find.rs:274-302): bound the walk at the cap, then sort the bounded set. fd does
            // exactly that too (`ReceiverBuffer` sorts while buffering, fd 10.5.0
            // `walk.rs:281-285`), and fd's walk is parallel — so sorting a capped set is the
            // upstream behaviour, not a divergence from it.
            //
            // This is NOT the full-tree `sort()`+`truncate()` that TOOL-033 removed
            // (`tests/pi_tool_semantics.rs:482-489`). That version drained the whole walk on
            // every call and then took the 100-match window from the alphabetically-first files —
            // a systematic `a*`-biased sample after a full-tree walk. Here the walk still stops
            // at the cap, so the SET is what discovery produced and only its ORDER moves.
            //
            // Determinism, precisely: the ORDER is always deterministic. The SET is deterministic
            // whenever the cap is not reached, which is every search that returns fewer than
            // `limit` matches. When the cap IS reached the set depends on which files finished
            // first — which is exactly pi's behaviour, since `stopChild(true)` (grep.ts:240-245,
            // :291-295) kills rg mid-parallel-walk.
            collected.sort_by(|(a, _), (b, _)| a.cmp(b));
            for (_, blocks) in collected {
                take_into(&mut out, &mut count, &mut any_line_truncated, limit, blocks);
            }
```

Every downstream test on `count` keeps working unchanged: `count >= limit` still drives the
`N matches limit reached` notice and `match_limit_reached`
([`grep.rs:1005-1029`](../../../crates/cyrup-tools/src/tools/grep.rs)), and it is now reached only
after trimming, so the row count is exact.

**The walk error needs the same treatment.** `grep.rs:958-967` keeps the FIRST error, on the
stated ground that "rg's parallel walk does not order it against ours, so the first is the only
stable choice". Once *our* walk is parallel, "first" is no longer stable either. Collect them and
keep the lexicographically smallest message — the message is `{path}: {os error}`
([`fs.rs:79`](../../../crates/cyrup-tools/src/ops/local/fs.rs) `walk_error_message`), so this is
path order, and it is deterministic. Everything else about that code stays: still deferred to
after the loop, still suppressed when `count >= limit` (pi's `killedDueToLimit`), still prefixed
`rg: ` here and nowhere else.

### 4e. `find`

`find` needs **no** code change beyond inheriting §4b — and that is the answer to "the same
treatment, or a doc comment saying why not":

- It sets no `prune`, so `WalkOpts::prune` defaults to `None` and no filter is registered.
- It has no per-entry I/O — just `PatternMatcher::is_match` on a string — so there is nothing to
  overlap; the walk was the entire cost.
- Its cap already bounds the walk (`find.rs:192-194`) and its
  `results.sort_by(Path::cmp)` at `find.rs:301-303` already restores deterministic order for
  `len() <= FD_MAX_BUFFER_LENGTH`.

Extend the comment at `find.rs:274-303` with the one fact that changes: the residual it already
records ("on a tree slow enough that fd's deadline beats the cap, fd streams in arrival order
where this sorts") now has a second, symmetric half — above `FD_MAX_BUFFER_LENGTH` results
cyrup also emits in arrival order, and that arrival order is now nondeterministic, exactly as
fd's is, because fd's own walk is `ignore::WalkParallel`. Delete the stale clause "cyrup's walk is
single-threaded with no streaming mode".

---

## 5. Order of work

1. **§4a** — `PruneDirs` + `WalkOpts::prune` + `filter_entry` + `grep.rs` prune moved in, still on
   the **serial** walk. Behaviour-preserving; the match set must be identical before any
   parallelism lands.
2. **§4b** — `to_item` extraction, then `build_parallel()` with the `sort_by_path` serial branch.
3. **§4c** — `search_one` returns `Vec<MatchBlock>`; the bounded pipeline.
4. **§4d/§4e** — sorted render, deterministic walk error, `find`'s comment.

Do not merge 1 and 2: step 1 is the whole safety argument for step 2, and it is only an argument
if it lands on its own.

---

## 6. Definition of done

1. `WalkOpts::prune` exists, is registered through `WalkBuilder::filter_entry`, is never consulted
   at depth 0, and returns `true` for non-directories. `grep.rs` has no `pruned: Vec<PathBuf>` and
   no `starts_with` scan.
2. `fs.rs::walk` calls `build_parallel()` when `opts.sort_by_path.is_none()` and `build()`
   otherwise, with **one** shared builder and **one** shared per-entry conversion, so the two
   branches cannot drift in which paths they visit.
3. Every knob at `fs.rs:297-435` is still applied on both branches: `$RIPGREP_CONFIG_PATH`'s
   `--no-ignore` / `--no-ignore-vcs` / `--max-depth` / `--follow` / `--one-file-system` /
   `--max-filesize` / `--ignore-file`, `require_git`, `parents(!opts.no_ignore)`, the custom
   ignore filename, and fd's global ignore file — including `--ignore-file` surviving
   `--no-ignore` and the parent-traversal interaction.
4. The visitor sends with `blocking_send` and returns `WalkState::Quit` on a closed receiver, so a
   cancelled or capped search stops every walker thread and never parks on a full channel. No
   `unwrap`, no indexing, no panic path inside the closure.
5. `grep` keeps up to `available_parallelism().min(12)` searches in flight, dispatching only while
   `found < limit`, and every read still goes through `self.fs` — never through `LocalFs`
   directly — so `TraversalFs`/`ProtectedFs` stay in the path of every byte searched.
6. Rows are emitted in `Path::cmp` order over per-file blocks, trimmed to `limit` **matches** (not
   rows); `any_line_truncated` reflects only emitted blocks; the walk error is the
   lexicographically smallest one and is still suppressed when the cap fired.
7. The doc comments state the ordering decision (§4d), the `--sort` ⇒ serial rule and its `rg
   --help` citation (§1a), and why the search stays on the `FsOps` side (§1b). Provenance comments
   are how parity is audited here — extend them, do not drop them.

**Semantics that already have guards — these must not change behaviour, and each is the fastest
signal if step 4a or 4c is wrong:**

| existing test | what it pins |
| --- | --- |
| `negated_directory_glob_prunes_the_whole_subtree` ([`grep.rs:1343`](../../../crates/cyrup-tools/src/tools/grep.rs)) | `!node_modules` removes the subtree |
| `trailing_slash_glob_prunes_directories_at_any_depth` (`grep.rs:1364`) | `!src/` is directory-only, unanchored |
| `plain_glob_miss_never_prunes_a_directory` (`grep.rs:1382`) | a non-negated miss must still descend |
| `the_search_root_and_an_explicit_file_are_never_pruned` (`grep.rs:1419`) | the depth-0 exemption |
| `a_pruned_subtree_does_not_consume_the_match_cap` (`grep.rs:1445`) | the prune happens before the cap is spent |
| `sortr_reverses_which_match_the_cap_returns` (`grep.rs:2119`) | `--sortr` selects the opposite set — **fails outright if `build_parallel()` swallows the sort** |
| `grep_stops_walking_once_the_match_limit_is_reached` ([`tests/pi_tool_semantics.rs:490`](../../../crates/cyrup-tools/src/tests/pi_tool_semantics.rs)) | exactly `limit` rows and a bounded walk — **fails if the concurrent over-collection is not trimmed** |
| `find_stops_walking_once_the_limit_is_reached` (`tests/pi_tool_semantics.rs:454`) | `find`'s cap still bounds consumer pulls |

Running the suite and measuring the speedup belong to `/flux/tests`, not here.
