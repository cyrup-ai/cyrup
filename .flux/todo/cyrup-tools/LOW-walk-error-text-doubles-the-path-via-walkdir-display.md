---
title: Walk error text doubles the path via walkdir's Display
priority: LOW
tool: grep
source: exec follow-up from the find-partial-results task
stage: exec
status: in-progress
updated: 2026-08-27 18:00
---

# `rg:`-prefixed walk errors double the path and interpose walkdir's wording

## What was found

The find-partial-results task prescribed `e.to_string()` in `LocalFs::walk`, on the
stated grounds that `ignore::Error::WithPath` renders `{path}: {io error}`. At the
version this workspace actually pins — **ignore 0.4.26** — it does not.

`Error::from_walkdir` builds `WithPath { path, err: Io(io::Error::from(walkdir_err)) }`,
and `io::Error::from(walkdir::Error)` carries walkdir's own `Display`. Observed:

```
rg: /proc/1/task/1/fdinfo: IO error for operation on /proc/1/task/1/fdinfo: Permission denied (os error 13)
```

The path appears twice and walkdir's wording is interposed. `rg` 14.1.0 prints the
clean form (re-verified in this container, see [Ground truth](#ground-truth-rg-1410-stderr-verbatim)):

```
rg: /proc/1/task/1/fdinfo: Permission denied (os error 13)
```

So this is a genuine parity gap, introduced upstream, not by the port.

## Why it was not fixed in place

The executing agent implemented `e.to_string()` exactly as its brief prescribed and
flagged the divergence rather than improvising. Reaching the clean form needs
structural formatting in `LocalFs::walk`, which that brief did not authorize —
correctly refused as scope creep.

---

# Research (aug)

## The mechanism, read out of the pinned sources

`Cargo.lock` pins `ignore` **0.4.26** and `walkdir` **2.5.0** (single copy of each;
no duplicate-version hazard for the fix below).

**1. `ignore` hands walkdir's error to `io::Error::from`.**
[`ignore-0.4.26/src/lib.rs`](/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ignore-0.4.26/src/lib.rs)
`fn from_walkdir` (`:285-303`), anchor `fn from_walkdir(err: walkdir::Error) -> Error`:

```rust
let path = err.path().map(|p| p.to_path_buf());
let mut ig_err = Error::Io(std::io::Error::from(err));
if let Some(path) = path {
    ig_err = Error::WithPath { path, err: Box::new(ig_err) };
}
```

Note the loop case is handled *before* this and never reaches it: it returns
`WithDepth { depth, err: Loop { ancestor, child } }` (`:286-295`) — **no `WithPath`**.

**2. walkdir 2.5.0's `From` is not a passthrough.**
[`walkdir-2.5.0/src/error.rs`](/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/walkdir-2.5.0/src/error.rs)
`impl From<Error> for io::Error` (`:241-262`), anchor
`fn from(walk_err: Error) -> io::Error`, ends in `io::Error::new(kind, walk_err)` — a
**custom-repr** `io::Error` that *owns the walkdir error as its payload*. Its own doc
comment says so: "*preserving the original `Error` as the 'inner error'. Note that this
also makes the display of the error include the context.*"

That "context" is walkdir's `Display` (`error.rs:220-239`, anchor
`impl fmt::Display for Error`):

```rust
ErrorInner::Io { path: Some(ref path), ref err } =>
    write!(f, "IO error for operation on {}: {}", path.display(), err),
```

**3. `ignore`'s `WithPath` then prefixes the path a second time.**
`lib.rs:333-335` (anchor `Error::WithPath { ref path, ref err } =>`):
`write!(f, "{}: {}", path.display(), err)`.

Composition of 2 + 3 is exactly the doubled string in the report. This was a
`walkdir` change (2.3.x returned the inner `io::Error` unchanged from `From`), which is
why `rg` 14.1.0's build produces the clean form and ours does not.

## The `ignore` 0.4.26 error API — what actually exists

**The parity action drafted in the original filing does not compile.** `ignore::Error`
has **no `path()` accessor** at 0.4.26 *or* 0.4.33. The complete public surface on
`impl Error` (`lib.rs:150-257`) is:

| accessor | line | returns |
| --- | --- | --- |
| `is_partial(&self) -> bool` | `:160` | recurses through `WithLineNumber`/`WithPath`/`WithDepth` |
| `is_io(&self) -> bool` | `:171` | same recursion; `Partial` only when it holds exactly one io error |
| `io_error(&self) -> Option<&std::io::Error>` | `:205` | **see caveat below** |
| `into_io_error(self) -> Option<std::io::Error>` | `:230` | owned variant of the same |
| `depth(&self) -> Option<usize>` | `:250` | `WithDepth`, or through `WithPath` |

(The `pub fn path` that greps up at `lib.rs:540` is `tests::TempDir::path`, not `Error`.)

**The caveat that kills the drafted fix.** `io_error()` (`:205-222`) unwraps only
*`ignore`'s own* wrappers — `Partial` (single-element only), `WithLineNumber`,
`WithPath`, `WithDepth` — and then returns the `Error::Io` payload verbatim. It does
**not** look inside walkdir's wrapper, because from `ignore`'s point of view that
wrapper *is* the `io::Error`. So:

```rust
// what the original filing proposed:
format!("{}: {}", path.display(), e.io_error().unwrap())
// => "/proc/1/task/1/fdinfo: IO error for operation on /proc/1/task/1/fdinfo: Permission denied (os error 13)"
```

— **byte-for-byte the same doubled string as `e.to_string()`.** The drafted parity
action must be replaced, not merely implemented.

## Where the original `io::Error` actually lives

Two documented, stable hops, no new dependency and no `downcast_ref`:

1. `std::io::Error::get_ref()` → `Option<&(dyn Error + Send + Sync + 'static)>` —
   `Some` **only** for the custom repr, i.e. only when something was boxed into the
   error. Here that payload is the `walkdir::Error`.
2. `std::error::Error::source()` on that payload — walkdir's impl
   ([`error.rs:212-217`](/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/walkdir-2.5.0/src/error.rs),
   anchor `fn source(&self) -> Option<&(dyn error::Error + 'static)>`) returns
   `Some(err)` for `ErrorInner::Io`, the **original** `io::Error`, whose `Display` is
   `Permission denied (os error 13)`.

Do **not** substitute `io.source()` for step 1+2 and hope std forwards it. Relying on
`get_ref()` then `source()` is the documented path and is what the prescription below
uses.

**Is the peel safe for non-walkdir errors?** Yes. Every other `io::Error` `ignore`
0.4.26 constructs on this seam is either (a) straight from `std::fs`, i.e. OS repr —
`get_ref()` is `None`
([`walk.rs:290-303`](/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ignore-0.4.26/src/walk.rs)
`metadata_internal`, `:1375`, `:1995`, `:2020`, `:390`, `:409`) — or (b)
`io::Error::new(kind, "<string literal>")` (`walk.rs:175-179`, `:377`, `:427`), whose
boxed payload is std's private string-error type with a `source()` of `None`. Both fall
through the guard untouched. The `walkdir::Error` produced by `from_walkdir` is the
**only** payload on this seam that has a `source()`.

## Ground truth: `rg` 14.1.0 stderr, verbatim

Captured in this container (`ripgrep 14.1.0`, running as uid 0; `/proc/1/task/1/fdinfo`
passes `metadata()` but refuses `read_dir` even for root, so it is a stable fixture):

```console
$ rg --no-config -n zzzz /proc/1/task/1/fdinfo
rg: /proc/1/task/1/fdinfo: Permission denied (os error 13)

$ rg --no-config -n zzzz /nope/does/not/exist
rg: /nope/does/not/exist: No such file or directory (os error 2)

$ rg --no-config -nL zzzz <dir containing a symlink back to itself>
rg: File system loop found: <dir>/a/back points to an ancestor <dir>
```

Three shapes to hit, and note the third: for a **loop** rg emits **no path prefix at
all** — because `from_walkdir` returns `WithDepth { Loop }`, which carries no `WithPath`,
and `WithDepth`'s `Display` is transparent (`lib.rs:336`). Any fix that unconditionally
prepends a path would regress this case.

## Blast radius

`FsOps::walk` has exactly two real consumers; the isolation layers
([`isolation/traversal.rs:132-142`](../../../crates/cyrup-tools/src/isolation/traversal.rs),
[`isolation/protected.rs:150-156`](../../../crates/cyrup-tools/src/isolation/protected.rs))
are pure pass-throughs that never touch `ToolError::message`.

* [`tools/find.rs:257`](../../../crates/cyrup-tools/src/tools/find.rs) — `Some(Err(_)) => continue,`.
  Discards the value unconditionally, mirroring fd. **`find` output cannot change.**
* [`tools/grep.rs:655-665`](../../../crates/cyrup-tools/src/tools/grep.rs) — anchor
  `Some(Err(e)) => {` … `Some(ToolError::new(format!("rg: {}", e.message)))`. The single
  behavioural consumer. This is the only text the fix moves.

---

# Required implementation

Single path. No new dependency, no `downcast_ref`, no `unsafe`, no string surgery.

## 1. `crates/cyrup-tools/src/ops/local/fs.rs` — add two module-private helpers

File: [`crates/cyrup-tools/src/ops/local/fs.rs`](../../../crates/cyrup-tools/src/ops/local/fs.rs).
Place them immediately **above** `impl FsOps for LocalFs`'s `fn walk` (currently `:219`,
anchor `fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>>`),
or beside `windows_access_result` at the top of the file — either is fine, but keep them
in this file: it is the crate's only `ignore::Walk` site.

`walk_error_message` must be `pub(crate)` so the test in step 4 can reach it, exactly as
`windows_access_result` is (`fs.rs:41`).

```rust
/// Render an [`ignore::Error`] the way ripgrep does — `{path}: {io error}`, with the path
/// stated **exactly once**.
///
/// `Display` cannot be used directly. `Error::from_walkdir` stores
/// `Error::Io(io::Error::from(walkdir_err))` (ignore 0.4.26 `src/lib.rs:296-301`), and
/// walkdir 2.5.0's `From<Error> for io::Error` is `io::Error::new(kind, walk_err)`
/// (`walkdir-2.5.0/src/error.rs:253-261`) — a CUSTOM io error whose own `Display` re-states
/// the path as `"IO error for operation on {path}: {err}"` (`:224-229`). Printed under
/// `WithPath` (`lib.rs:333-335`) that yields the path twice. walkdir 2.3.x returned the
/// inner `io::Error` unchanged, which is why `rg` 14.1.0 prints the clean form and a
/// straight `to_string()` here does not.
///
/// `Error::io_error()` is NOT the escape hatch: it unwraps only `ignore`'s own
/// `Partial`/`WithLineNumber`/`WithPath`/`WithDepth` nesting and then returns the
/// `Error::Io` payload verbatim (`lib.rs:205-222`), i.e. walkdir's wrapper. And there is
/// no `Error::path()` accessor at 0.4.26 — the path is only reachable by matching the
/// public `WithPath` variant (`lib.rs:78-84`), which is what this does.
///
/// Every arm reproduces the corresponding arm of `impl Display for Error`
/// (`lib.rs:322-359`) unchanged except for the io leaf, so `Loop` (which carries no
/// `WithPath` — `lib.rs:286-295`) keeps ripgrep's un-prefixed
/// `"File system loop found: … points to an ancestor …"`.
pub(crate) fn walk_error_message(err: &ignore::Error) -> String {
    match err {
        // The one variant carrying a path. Recurse, so the tail is the PEELED io text
        // rather than walkdir's restatement of this same path.
        ignore::Error::WithPath { path, err } => {
            format!("{}: {}", path.display(), walk_error_message(err))
        }
        // Transparent in `Display` (`lib.rs:336`); stay transparent.
        ignore::Error::WithDepth { err, .. } => walk_error_message(err),
        // `Display` is `line {n}: {err}` (`lib.rs:330-332`).
        ignore::Error::WithLineNumber { line, err } => {
            format!("line {line}: {}", walk_error_message(err))
        }
        // `Display` joins with `\n` (`lib.rs:325-329`).
        ignore::Error::Partial(errs) => errs
            .iter()
            .map(walk_error_message)
            .collect::<Vec<_>>()
            .join("\n"),
        ignore::Error::Io(io) => io_error_message(io),
        // `Loop`, `Glob`, `UnrecognizedFileType`, `InvalidDefinition`: no nested io error,
        // no doubled path — their own `Display` is already what ripgrep prints.
        other => other.to_string(),
    }
}

/// Peel walkdir's wrapper off an [`std::io::Error`] so it prints as the OS did.
///
/// `io::Error::get_ref` is `Some` only for the custom repr, where it hands back the boxed
/// payload — here the `walkdir::Error`. walkdir's `source()` is the ORIGINAL `io::Error`
/// (`walkdir-2.5.0/src/error.rs:212-217`), which renders as
/// `Permission denied (os error 13)`.
///
/// Both hops are required. An `io::Error` straight from `std::fs` is the OS repr, so
/// `get_ref()` is `None`; `ignore`'s handful of `io::Error::new(kind, "<literal>")` sites
/// (`walk.rs:175-179`, `:377`, `:427`) DO have a payload but its `source()` is `None`.
/// Both therefore fall through to their own `Display`, unchanged. walkdir's error is the
/// only payload on this seam that carries a `source()`.
fn io_error_message(err: &std::io::Error) -> String {
    if let Some(payload) = err.get_ref()
        && let Some(original) = std::error::Error::source(payload)
    {
        return original.to_string();
    }
    err.to_string()
}
```

Notes for the implementer, so nothing is second-guessed at the keyboard:

* `std::error::Error::source(payload)` is written **fully qualified on purpose**:
  `fs.rs` does not import `std::error::Error`, and a trait method on a `dyn Trait` object
  still needs the trait in scope. Do not add a `use` for it.
* The `if let … && let …` chain is a let-chain; `fs.rs:262-264` already uses one
  (`if opts.flavor.reads_fd_global_ignore() && let Some(global) = …`), so the edition
  supports it.
* In the `WithPath`/`WithDepth`/`WithLineNumber` arms `err` binds as `&Box<ignore::Error>`;
  deref coercion at the call site makes `walk_error_message(err)` type-check with no
  explicit `&**err`.
* `ignore::Error` is not `#[non_exhaustive]`, but keep the `other =>` catch-all anyway —
  it is what makes an `ignore` bump a no-op here rather than a build break.

## 2. `crates/cyrup-tools/src/ops/local/fs.rs` — the `Err` arm

Anchor: the sole line `Err(e) => Err(ToolError::new(e.to_string())),` (currently `:308`,
inside `for result in walker {`, currently `:286`).

```rust
Err(e) => Err(ToolError::new(walk_error_message(&e))),
```

## 3. `crates/cyrup-tools/src/ops/local/fs.rs` — correct the comment that stated the false premise

Anchor: the comment block opening `// The message is the BARE `ignore::Error` text —`
(currently `:279-285`), directly above `for result in walker {`. Its first sentence is
the premise this task disproved and must not survive. Replace those two sentences —
keep the rest of the block (the "No prefix is added here…" half) verbatim:

```rust
            // The message is `walk_error_message`'s rendering of the `ignore::Error`, i.e.
            // ripgrep's own `{path}: {io error}` with the path stated once, e.g.
            // `/srv/locked: Permission denied (os error 13)`. It is NOT `e.to_string()`:
            // at ignore 0.4.26 + walkdir 2.5.0 `Display` states the path TWICE — see
            // `walk_error_message`. No prefix is added here because the two consumers of
            // this seam want different things from the same value: `find` emulates fd,
            // which discards it (fd 10.5.0 `src/walk.rs:227-231`, `:500-505`), and `grep`
            // emulates ripgrep, which reports it as `rg: {path}: {io error}` on stderr.
            // A `walk: ` prefix matched neither.
```

## 4. New test module `crates/cyrup-tools/src/tests/walk_error_text.rs`

Register it in [`crates/cyrup-tools/src/tests/mod.rs`](../../../crates/cyrup-tools/src/tests/mod.rs)
in alphabetical position (after `mod tools;`, before `mod write_semantics;`).

The test must be **hermetic** — no `/proc`, no root, no `chmod`. `ignore::Error`'s
variants and fields are public, and the walkdir wrapper's *shape* (a custom `io::Error`
whose payload has a `source()`) is reproducible with a five-line local error type. Follow
[`read_access_errno.rs`](../../../crates/cyrup-tools/src/tests/read_access_errno.rs) for
the module-header + `#![allow(clippy::unwrap_used, …)]` convention.

Required cases:

1. **The regression itself.** `WithPath { path: "/srv/locked", err: Io(<wrapper over
   EACCES>) }` renders `/srv/locked: Permission denied (os error 13)` — and assert
   `!msg.contains("IO error for operation on")` and that `"/srv/locked"` occurs exactly
   once, so the doubling cannot silently return.
2. **Plain OS error is untouched.** `WithPath { path, err: Io(io::Error::from_raw_os_error(2)) }`
   renders `{path}: No such file or directory (os error 2)`.
3. **No path, no empty prefix** (DoD 4). Bare `Io(io::Error::from_raw_os_error(13))`
   renders `Permission denied (os error 13)` with no leading `": "`.
4. **Loop keeps ripgrep's un-prefixed wording.** `WithDepth { depth: 1, err: Loop { ancestor, child } }`
   renders `File system loop found: {child} points to an ancestor {ancestor}` —
   the exact string captured from `rg` above.
5. **Ignore-file parse errors keep `Display`'s shape.** `Partial([WithPath { path, err:
   WithLineNumber { line: 3, err: Glob { glob: Some(..), err: .. } } }])` renders
   `{path}: line 3: error parsing glob '…': …`, i.e. identical to `err.to_string()`.
   Assert equality against `err.to_string()` directly — for every non-io-leaf shape the
   helper is required to be a faithful reimplementation of `Display`.
6. **String-payload io errors are not over-peeled.** `Io(io::Error::new(ErrorKind::Other, "boom"))`
   renders `boom`, proving the `source()` half of the guard is load-bearing.

## Definition of done

1. A walk error surfaced by `grep` reads `rg: <path>: <io error>` with the path
   appearing exactly once.
2. The text matches what `rg` 14.1.0 prints for the same path — the three captures under
   [Ground truth](#ground-truth-rg-1410-stderr-verbatim) are the target strings.
3. `find` output is unchanged — `find.rs:257` discards walk errors unconditionally.
4. An error carrying no path still produces its `Display` text rather than an empty
   prefix; a `Loop` error still prints with no path prefix at all.
5. `fs.rs`'s comment above `for result in walker` no longer claims `WithPath` renders
   `{path}: {io error}` at 0.4.26.
6. Every non-io-leaf `ignore::Error` shape renders byte-identically to `Display`.

---

## Second, unrelated fragment folded in: a stale comment citation

Anchor: [`crates/cyrup-tools/src/tools/find.rs`](../../../crates/cyrup-tools/src/tools/find.rs),
the walk-loop error-swallowing comment, sentence beginning
`// Swallowing here cannot hide a bad search root:` (currently `:250-252`, immediately
above `Some(Err(_)) => continue,` at `:257`). Current text:

> Swallowing here cannot hide a bad search root: a root that does not
> exist is already rejected by the `metadata(&search_root)` probe above
> (pi's `Path not found`, find.ts:158).

Two problems, both confirmed:

* **The coordinate is wrong.** `find.ts:158` is inside pi's `onAbort` handler
  (`:156-159`, the body being `settle(() => reject(new Error("Operation aborted")))`),
  registered at `:160` — the range `find.rs:202` *correctly* cites, for a different
  purpose. pi's `Path not found:` literal is at
  [`find.ts:171`](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts), inside
  the `if (customOps?.glob) {` branch opened at `:169`.
* **Attributing it to this gate is wrong, not just imprecise.** That branch is pi's
  injected-operations path, **not** the fd path `find` implements — and the guard block
  20 lines up (`find.rs:123-134`, anchor
  `// Pi's fd branch has NO pre-check:`) already says so in as many words, ending
  "*the `Path not found:` literal … does NOT belong on this path*". The stale comment
  contradicts its own file.
* **The description is understated.** Since the find-path-guard task landed, the probe is
  `!self.fs.metadata(&search_root).await.is_ok_and(|meta| meta.is_dir)` (`find.rs:135-140`)
  — it rejects a root that exists but is **not a directory** as well as a missing one, and
  emits fd's two-line `[fd error]: Search path '…' is not a directory.` /
  `[fd error]: No valid search paths given.` (`find.rs:141-144`), never pi's `Path not found`.

### Required replacement

Replace exactly the two-and-a-bit lines cited above (through
`… (pi's `Path not found`, find.ts:158).`) with:

```rust
                        // Swallowing here cannot hide a bad search root: the
                        // `metadata(&search_root)` probe above already rejects BOTH a
                        // missing root and one that exists but is not a directory, with
                        // fd's own two-line `[fd error]: …` message — see the note on
                        // that gate for why pi's `Path not found:` (find.ts:171, the
                        // `customOps.glob` branch) is NOT this tool's message.
```

Leave the rest of the paragraph (`A root that exists but cannot be opened yields zero
rows …`) untouched. This comment emits nothing; the change is documentation-only and
carries no test.

### Definition of done

7. The `find.rs` comment cites `find.ts:171` (or no pi coordinate at all) and never
   `find.ts:158` for `Path not found`, describes the gate as rejecting missing **and**
   non-directory roots, and does not attribute pi's `Path not found` to this tool.
