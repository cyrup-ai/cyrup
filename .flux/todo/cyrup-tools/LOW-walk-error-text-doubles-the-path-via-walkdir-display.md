---
title: Walk error text doubles the path via walkdir's Display
priority: LOW
tool: grep
source: exec follow-up from the find-partial-results task
stage: new
status: done
updated: 2026-08-27
---

# `rg:`-prefixed walk errors double the path and interpose walkdir's wording

## What was found

The find-partial-results task prescribed `e.to_string()` in `LocalFs::walk`, on the
stated grounds that `ignore::Error::WithPath` renders `{path}: {io error}`. At the
version this workspace actually pins — **ignore 0.4.26** — it does not.

`Error::from_walkdir` (`lib.rs:296-301`) builds `WithPath { path, err: Io(io::Error::from(walkdir_err)) }`,
and `io::Error::from(walkdir::Error)` carries walkdir's own `Display`. Observed:

```
rg: /proc/1/task/1/fdinfo: IO error for operation on /proc/1/task/1/fdinfo: Permission denied (os error 13)
```

The path appears twice and walkdir's wording is interposed. `rg` 14.1.0 — which
builds against ignore 0.4.23 — prints the clean form:

```
rg: /proc/1/task/1/fdinfo: Permission denied (os error 13)
```

So this is a genuine parity gap, introduced by an upstream change between 0.4.23
and 0.4.26, not by the port.

## Why it was not fixed in place

The executing agent implemented `e.to_string()` exactly as its brief prescribed and
flagged the divergence rather than improvising. Reaching the clean form needs
`e.path()` + `e.io_error()` formatting in `LocalFs::walk`, which that brief did not
authorize — correctly refused as scope creep.

## Parity action

In `LocalFs::walk`'s `Err` arm, format from the structured parts rather than
`Display`: take `e.path()` and `e.io_error()` and emit `{path}: {io_error}`,
falling back to `e.to_string()` when either is absent. `find` still discards the
text entirely, so only `grep`'s `rg: `-prefixed output changes.

## Definition of done

1. A walk error surfaced by `grep` reads `rg: <path>: <io error>` with the path
   appearing exactly once.
2. The text matches what `rg` 14.1.0 prints for the same path.
3. `find` output is unchanged — it discards walk errors unconditionally.
4. An error carrying no path still produces its `Display` text rather than an
   empty prefix.
