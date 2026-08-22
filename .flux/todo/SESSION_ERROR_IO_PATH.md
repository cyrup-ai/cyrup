---
stage: new
status: done
updated: 2026-08-22 23:52
---

# SessionError::Io drops the filename on all 14 filesystem `?` sites — users see bare `io: No such file or directory (os error 2)`

> Found by a six-lens hygiene audit of `crates/cyrup-session`, run after the `manager/`
> decomposition landed in PR #53. Every claim below was reproduced against the tree.
> **Priority:** medium · **Effort:** medium

`src/error.rs:12-13` defines the catch-all

```rust
#[error("io: {0}")]
Io(#[from] std::io::Error),
```

Every other path-bearing variant in the same enum does carry a path — `NotASession { path }`, `EmptyFork(PathBuf)`, `AlreadyExists(PathBuf)` — but the blanket `#[from]` means **14** `?`-propagated filesystem failures surface with no filename at all:

```
src/store.rs:54   OpenOptions::new().create(true).append(true).open(&self.path)?
src/store.rs:55   f.write_all(line.as_bytes())?
src/store.rs:56   f.write_all(b"\n")?
src/store.rs:64   std::fs::create_dir_all(parent)?
src/store.rs:86   File::create(&tmp)?
src/store.rs:87   f.write_all(buf.as_bytes())?
src/store.rs:91   std::fs::rename(&tmp, &self.path)?
src/store.rs:101  std::fs::create_dir_all(parent)?
src/store.rs:121  f.write_all(buf.as_bytes())?
src/manager/load.rs:32        std::fs::File::open(path)?
src/manager/accessors.rs:23   w.write_all(serde_json::to_string(&header)?.as_bytes())?
src/manager/accessors.rs:24   w.write_all(b"\n")?
src/manager/accessors.rs:26   w.write_all(e.to_line()?.as_bytes())?
src/manager/accessors.rs:27   w.write_all(b"\n")?
```

These are exactly the paths a user hits when a session cannot be appended to, rewritten, or opened. `store.rs:86-91` is the worst case: it writes a temp file and then renames it, so a failure can be on either of two different paths and the message identifies neither.

## Fix

Add a path-carrying variant, e.g.

```rust
#[error("{op} {path}: {source}")]
Io { op: &'static str, path: PathBuf, #[source] source: std::io::Error },
```

and `map_err` each site. Every `DiskStore` op already holds `self.path`, and `load.rs:32` / `accessors.rs` have the path in scope, so no plumbing is needed. Keep the bare `#[from]` variant only if some caller genuinely has no path; prefer removing it so the compiler forces every new fs site to name its file.

`SessionError` is public API (re-exported at `lib.rs:41`), so this is a breaking change to the variant shape — check `cyrup-session-svc` and `cyrup-ext-subagents` for matches on `SessionError::Io`.

## Acceptance Criteria

- [ ] `SessionError` has an IO variant carrying a `PathBuf`, and its `#[error(...)]` format string includes the path
- [ ] All 14 sites listed above attach a path; `grep -n 'fs::\|File::open\|File::create\|OpenOptions\|write_all(\|create_dir_all\|fs::rename' src/store.rs src/manager/load.rs src/manager/accessors.rs | grep -c '?$'` shows no bare `?` on an fs call that discards the path
- [ ] A test asserts that opening a nonexistent session file produces an error string containing that file's path
- [ ] `cargo test -p cyrup-session` passes (157+) and `cargo test -p cyrup-session-svc` passes
- [ ] `cargo clippy --all-targets -p cyrup-session -p cyrup-session-svc` reports 0 findings

## Verifying command

```bash
cd /home/user/cyrup/crates/cyrup-session && sed -n '10,33p' src/error.rs && grep -rn 'std::fs::\|File::open\|File::create\|OpenOptions\|write_all(\|create_dir_all\|fs::rename' --include='*.rs' src/store.rs src/manager/load.rs src/manager/accessors.rs | grep '?'
```
