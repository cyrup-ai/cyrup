---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/path.rs:161"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 02:11
---

# Capability gap: `crates/cyrup-tools/src/path.rs:161`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

fd 10.5.0 (walk.rs:371-375) resolves its global ignore file under `etcetera::choose_base_strategy().config_dir()`. On Windows etcetera's `dir_inner` is `env_var("APPDATA") -> dir_crt("APPDATA") -> home\\AppData\\Roaming`, where `dir_crt` is a real `SHGetKnownFolderPath(FOLDERID_RoamingAppData, KF_FLAG_DONT_VERIFY)` call (verified in /root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/etcetera-0.11.0/src/base_strategy/windows.rs:123-127 and :190-196). pi shells out to that fd binary (find.ts:225-269), so it gets the known-folder answer.

## What cyrup does

`fd_config_dir()` (path.rs:167-176) implements only two of the three steps: `%APPDATA%` when set and non-empty, else `home_dir()\\AppData\\Roaming`. The win32 known-folder lookup between them is absent. cyrup's `home_dir()` is also not `std::env::home_dir()` (which etcetera uses), so the fallback leg diverges twice.

## What a caller sees

CONFIRMED capability gap (this is the first item you asked about — refuting it is not available on the evidence). Precondition: a Windows session with `%APPDATA%` unset or empty and a redirected/roaming AppData folder. pi/fd then reads `<known-folder>\\fd\\ignore` and excludes those patterns; cyrup reads `<home>\\AppData\\Roaming\\fd\\ignore` (usually absent) and excludes nothing. The user sees `find` return files pi omits — a silent over-inclusion, never an error. Verified by source on both sides; the runtime precondition is Windows-only and cannot be produced in this container.

## Decision required

One of:

1. **Close it** — bring cyrup to pi's behaviour.
2. **Accept it** — David explicitly accepts the divergence; the marker stays and is
   annotated as authorized, with the reason.
3. **Reshape it** — the divergence is right but the current form is wrong.

Do not silently keep option 2 by leaving the marker as-is; that is how this became a
backlog in the first place.

## Definition of done

1. The gap is closed, or the marker records an explicit authorized acceptance.
2. If closed, a test fails without the change.
3. No behaviour regression in the owning crate.

---

# AUG PASS — 2026-08-28

Research-only. No source touched. Every API named below was read at its pinned version
in this container; the exact file:line is given for each so the exec pass can re-verify
without re-deriving.

## R1. What `etcetera` 0.11.0 actually does — read, not remembered

Pinned source: `/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/etcetera-0.11.0/src/`.
fd 10.5.0 depends on `etcetera = "0.11"` exactly (`tmp/fdsrc/fd-find-10.5.0/Cargo.toml:106-107`),
so 0.11.0 IS the code pi's fd runs. It is **four** mechanisms, not one, and cyrup
reproduces only part of two of them.

**(a) Strategy selection** — `base_strategy.rs:53-61`, `cfg_if!`: Windows ⇒ `Windows`;
macOS/iOS ⇒ `Xdg` for the *base* strategy (`Apple` is only the *native* one); everything
else ⇒ `Xdg`. cyrup's existing doc comment on `fd_config_dir` states this correctly.

**(b) Home is resolved EAGERLY, and its failure kills the whole strategy.**
`Windows::new()` (`windows.rs:117-121`) and `Xdg::new()` (`xdg.rs:169-173`) both do
`home_dir: crate::home_dir()?`. `etcetera::home_dir()` (`lib.rs:112-114`) is
`std::env::home_dir().ok_or(HomeDirError)` — the **std** function, nothing else. fd's
call site is `if let Ok(basedirs) = etcetera::choose_base_strategy()`
(`fd-find-10.5.0/src/walk.rs:371-372`), so when home is unresolvable fd registers **no**
global ignore file **even if `%APPDATA%` / `$XDG_CONFIG_HOME` is set**.

**(c) The Windows config dir** — `windows.rs:190-196`:
`config_dir()` delegates to `data_dir()`, which is
`dir_inner("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"))`.
`dir_inner` (`windows.rs:123-130`) is
`env::var_os(env).filter(|s| !s.is_empty()).map(PathBuf::from).or_else(|| Self::dir_crt(env))`
— note `or_else`: the syscall is **lazy**, skipped whenever `%APPDATA%` is usable.
`dir_crt` (`windows.rs:132-177`, gated `#[cfg(all(windows, not(target_vendor = "uwp")))]`,
with a `None` stub at `:179-182`) is the `SHGetKnownFolderPath` call, and its own comment
says it is kept in sync with `home-0.5.11/crates/home/src/windows.rs`.

**(d) The Xdg config dir** — `xdg.rs:194-196` → `env_var_or_default("XDG_CONFIG_HOME", ".config/")`
→ `env_var_or_none` (`xdg.rs:175-182`) reads the var and **discards it unless
`PathBuf::is_absolute()`**, else `home.join(".config/")`. cyrup's non-Windows arm already
matches this. (The `".config/"` trailing slash is cosmetic — `PathBuf` compares by
component, so `~/.config/` and `~/.config` are equal and join identically. Do not "fix" it.)

## R2. The Win32 API, verified symbol-by-symbol at the version that would be used

`windows-sys` **0.61.2** is already resolved in `/home/user/cyrup/Cargo.lock:8091-8093`
(28 dependents incl. tokio, rustix, tempfile), and its dep `windows-link` 0.2.1 is at
`Cargo.lock:7998-8000`. etcetera 0.11.0 itself pins
`[target."cfg(windows)".dependencies.windows-sys] version = "0.61", features = ["Win32_Foundation", "Win32_System_Com", "Win32_UI_Shell"]`.
Read at `/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/windows-sys-0.61.2/`:

| symbol | file:line | exact form |
|---|---|---|
| `SHGetKnownFolderPath` | `src/Windows/Win32/UI/Shell/mod.rs:573` | `fn(rfid: *const core::GUID, dwflags: u32, htoken: Foundation::HANDLE, ppszpath: *mut core::PWSTR) -> core::HRESULT` |
| `FOLDERID_RoamingAppData` | `Shell/mod.rs:3100` | `core::GUID` const, `3eb685db-65f9-4cf6-a03a-e3ef65729f3d` |
| `KF_FLAG_DONT_VERIFY` | `Shell/mod.rs:3695` | `KNOWN_FOLDER_FLAG = 16384i32` |
| `KNOWN_FOLDER_FLAG` | `Shell/mod.rs:3746` | `= i32` — **so `dwflags` needs an `as u32` cast** |
| `CoTaskMemFree` | `Win32/System/Com/mod.rs:84` | `fn(pv: *const core::ffi::c_void)` |
| `S_OK` | `Win32/Foundation/mod.rs:9452` | `core::HRESULT` (`= i32`) |
| `HANDLE` | `Win32/Foundation/mod.rs:5119` | `= *mut core::ffi::c_void` — `null_mut()` is the "current user" token |
| `PWSTR` | `src/core/mod.rs:9` | `= *mut u16` |

Feature gating confirmed: `Win32_UI_Shell` guards `pub mod Shell` (`Win32/UI/mod.rs:15-16`),
`Win32_System_Com` guards `pub mod Com` (`Win32/System/mod.rs:11-12`); the three feature
names exist in `windows-sys-0.61.2/Cargo.toml:56,85,184,272`. `SHGetKnownFolderPath` itself
carries no additional per-item `cfg` (the `#[cfg(feature = "Win32_UI_Shell_Common")]` at
`:570` and `:575` belong to its neighbours, not to it).

`std::env::home_dir()` is **not deprecated** on the pinned toolchain — compiled a probe
under `#![deny(deprecated)]` with `rustc 1.98.0 (88d9e12ae 2026-08-18)`; it built clean.
Workspace `rust-version = "1.96"` (`Cargo.toml:89`) clears etcetera's own 1.87 floor.

## R3. Findings — additional divergences this research turned up

Recorded, not descoped. Each is a *finding*; the disposition is David's (see Open questions).

**F1 — the same gap is reachable on Linux, and this host can prove it.**
`etcetera` resolves home with `std::env::home_dir()`, which on unix falls back to the
passwd entry when `HOME` is unset. **Verified empirically here**, not from memory:
a `rustc`-built probe printed `HOME env = None` / `home_dir() = Some("/root")` under
`env -u HOME`, with `getent passwd 0` giving `/root`. cyrup's `home_dir()` (path.rs:88-100)
is `HOME`-only on non-Windows, so on any Linux/macOS session with `HOME` unset, fd reads
`<passwd-home>/.config/fd/ignore` and cyrup reads nothing — the identical
"cyrup excludes fewer paths, silently" failure the marker describes, on a platform where
it is executable. The gap was filed as Windows-only; it is not.

**F2 — the eager-home gate diverges in the opposite direction.**
Per R1(b), when home is unresolvable fd registers no global ignore at all, even with
`%APPDATA%`/`$XDG_CONFIG_HOME` set. cyrup returns a directory in that case, so cyrup
**excludes more** than pi. Low-population, but it is the same function and one line to fix.

**F3 — bears on the sibling marker at `path.rs:106`** (`HOMEDRIVE`/`HOMEPATH` widening,
filed separately as unverifiable-on-this-host — *not touched by this task*). Two things
that pass are worth recording there: (i) once the known-folder leg exists, the home leg of
`fd_config_dir` is reached only when `%APPDATA%` is unset **and** `SHGetKnownFolderPath`
fails, which with `KF_FLAG_DONT_VERIFY` is near-unreachable — so if the prescription below
is taken, `HOMEDRIVE`/`HOMEPATH` stops being reachable through `fd_config_dir` entirely and
survives only through `expand_home`; (ii) `expand_home` must **keep** calling cyrup's own
`home_dir()`, because there the target is Node's `os.homedir()`, not std's — the two
functions mirror different upstreams and must not be merged.

## R4. The dependency decision — tradeoff, and the pick

The task asks for one of: call Win32 directly, or take the `etcetera` dependency the
original work deliberately avoided.

**Option B — depend on `etcetera` 0.11 and call `choose_base_strategy()`.**
*For:* it is not an equivalent, it is the identical code fd links, same major version;
zero `unsafe` in cyrup; closes R1(a)-(d) and F1 and F2 in one edge; future upstream fixes
arrive free; nothing to compile-verify on a host we do not have.
*Against:* `etcetera` is **not** in `Cargo.lock` (grepped: no entry) — a genuinely new
third-party crate. This workspace's root `Cargo.toml` treats that as a ratification event
(every dep block carries a written justification; several say "User-ratified for the L6
round"). It also dissolves the pure, host-independent unit tests: etcetera's legs are
private, so nothing in the ordering stays assertable from a Linux runner.

**Option A — a direct `windows-sys` edge plus a ~30-line `#[allow(unsafe_code)]` probe. ← PICKED.**
*For:*
1. **Adds no package to the graph.** `windows-sys` 0.61.2 and `windows-link` 0.2.1 are
   already resolved. The workspace has explicit precedent language for exactly this move —
   `ring` (`Cargo.toml:212-217`: "ADDS NO NEW CRATE TO THE GRAPH … a direct edge onto
   something already compiled, not a new third-party surface"), and `url` and `rustix`
   are justified the same way. Nothing new is built on non-Windows targets either.
2. **The shape is already sanctioned in this exact crate.** `cyrup-tools` is
   `#![deny(unsafe_code)]` (lib.rs:19) — deny, not forbid — with eight established
   `#[allow(unsafe_code)]` + `// SAFETY:` sites (`ops/local/signal.rs:24,64,110,137`,
   `command.rs:14`, `fs.rs:202`, `guard.rs:79`). It already carries a target-gated
   `[target."cfg(unix)".dependencies] libc` block, so `[target."cfg(windows)".dependencies]`
   is the mirror of a pattern the manifest already uses.
3. **It is the only option that yields a RED test on this host.** Keeping the legs as pure
   functions with the syscall injected reproduces the CFG-072 pattern this very file
   already established (`windows_home_from`, path.rs:113-131 + its test at :818-849),
   whose whole stated purpose was "so the precedence is testable on every platform".
4. It matches `path.rs`'s design intent throughout: reproduce the upstream mechanism with
   citations (`normalize_windows_shell_path`, `windows_home_from`), which the module doc
   already declares for etcetera.
*Against:* ~30 lines of FFI cyrup owns, whose bug class is memory-unsafety, and which
**cannot be compiled on this host** (see "What cannot be verified").

If David rejects the `unsafe`, Option B is the fallback and the exec pass should take it
wholesale (`fd_config_dir` collapses to `choose_base_strategy().ok().map(|s| s.config_dir())`),
accepting the loss of the pure tests.

## R5. Prescription — CLOSE

Anchors are symbols, not line numbers.

### P1 — `crates/cyrup-tools/src/path.rs`, symbol `fd_config_dir`

Split into etcetera's own two halves — eager home, then a pure config-dir computation —
and add the missing middle leg. Sketch (verified against the signatures in R2; laziness of
`or_else` preserved so the syscall is skipped whenever `%APPDATA%` is usable):

```rust
fn fd_config_dir() -> Option<PathBuf> {
    // R1(b): `Windows::new()`/`Xdg::new()` resolve home EAGERLY (`windows.rs:117-121`,
    // `xdg.rs:169-173`) via `etcetera::home_dir()` = `std::env::home_dir()` (`lib.rs:112-114`),
    // and fd's `if let Ok(basedirs)` (fd 10.5.0 `walk.rs:371-372`) then registers NO global
    // ignore file at all. `std::env::home_dir`, not this module's `home_dir()`: that one
    // mirrors Node's `os.homedir()` for `expand_home`, a different upstream. F2/F1.
    let home = std::env::home_dir()?;
    #[cfg(windows)]
    { Some(windows_config_dir_from(std::env::var_os("APPDATA"), known_folder_roaming_appdata, home)) }
    #[cfg(not(windows))]
    { Some(xdg_config_dir_from(std::env::var_os("XDG_CONFIG_HOME"), home)) }
}

/// `etcetera::base_strategy::Windows::{dir_inner, data_dir}` (`windows.rs:123-130`, `:190-196`)
/// as a pure function of its three inputs, with the known-folder probe INJECTED so the
/// precedence is assertable on a non-Windows runner — the `windows_home_from` pattern (CFG-072).
#[cfg(any(windows, test))]
fn windows_config_dir_from(
    appdata: Option<std::ffi::OsString>,
    known_folder: impl FnOnce() -> Option<PathBuf>,
    home: PathBuf,
) -> PathBuf {
    appdata
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(known_folder)                      // THE MISSING LEG
        .unwrap_or_else(|| home.join("AppData").join("Roaming"))
}

/// `etcetera::base_strategy::Xdg::{env_var_or_none, config_dir}` (`xdg.rs:175-182`, `:194-196`).
fn xdg_config_dir_from(xdg: Option<std::ffi::OsString>, home: PathBuf) -> PathBuf {
    xdg.map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
}
```

### P2 — new symbol `known_folder_roaming_appdata` in the same file

Port of `etcetera`'s `dir_crt("APPDATA")` (`windows.rs:132-177`). Every name below is the
verified one from the R2 table.

```rust
/// `etcetera::base_strategy::Windows::dir_crt("APPDATA")` (`windows.rs:132-177`), which
/// etcetera in turn keeps in sync with `home-0.5.11/crates/home/src/windows.rs`.
#[cfg(all(windows, not(target_vendor = "uwp")))]
#[allow(unsafe_code)]
fn known_folder_roaming_appdata() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::Foundation::S_OK;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_RoamingAppData, KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath,
    };
    // SAFETY: on `S_OK` the API writes one COM-allocated, NUL-terminated UTF-16 string
    // through `ppszpath`; on any other HRESULT it leaves the out-pointer null.
    // `CoTaskMemFree` is documented as a no-op on null, so both arms free exactly once and
    // the pointer is never read after its free. The scan stops at the first NUL, which the
    // API guarantees on success. A null `htoken` selects the calling user.
    unsafe {
        let mut path: windows_sys::core::PWSTR = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(
            &FOLDERID_RoamingAppData,
            KF_FLAG_DONT_VERIFY as u32,   // KNOWN_FOLDER_FLAG is i32; dwflags is u32
            std::ptr::null_mut(),
            &mut path,
        );
        if hr != S_OK {
            CoTaskMemFree(path.cast());
            return None;
        }
        let mut len = 0usize;
        while *path.add(len) != 0 { len += 1; }
        let s = OsString::from_wide(std::slice::from_raw_parts(path, len));
        CoTaskMemFree(path.cast());
        Some(PathBuf::from(s))
    }
}

/// UWP has no `SHGetKnownFolderPath`; etcetera returns `None` there (`windows.rs:179-182`).
#[cfg(all(windows, target_vendor = "uwp"))]
fn known_folder_roaming_appdata() -> Option<PathBuf> { None }
```

Two deliberate departures from etcetera, both stated so a reviewer does not read them as drift:
* etcetera's stub is `#[cfg(not(all(windows, not(target_vendor = "uwp"))))]` because its
  `Windows` strategy is constructible on unix. cyrup's is reachable only from the
  `#[cfg(windows)]` arm of `fd_config_dir`, so the stub is narrowed to
  `all(windows, target_vendor = "uwp")` — a `not(windows)` stub would be dead code on unix.
* etcetera declares `unsafe extern "C" { fn wcslen(buf: *const u16) -> usize; }`. The inline
  NUL scan above avoids that extern, which matters because `cyrup-tools` pulls `libc` only
  under `[target."cfg(unix)".dependencies]`.

### P3 — manifests

Root `/home/user/cyrup/Cargo.toml`, `[workspace.dependencies]`, with a rationale comment in
the house style used by `ring`/`url`/`rustix` — say explicitly that it is already in
`Cargo.lock` and that the feature list is etcetera 0.11.0's own, verbatim:

```toml
windows-sys = { version = "0.61", features = ["Win32_Foundation", "Win32_System_Com", "Win32_UI_Shell"] }
```

`crates/cyrup-tools/Cargo.toml`, alongside the existing `[target."cfg(unix)".dependencies]`:

```toml
[target."cfg(windows)".dependencies]
windows-sys = { workspace = true }
```

### P4 — the marker

Delete the `[CYRUP-DELTA — the Windows arm omits etcetera's SHGetKnownFolderPath fallback]`
paragraph from `fd_config_dir`'s doc comment. Nothing replaces it: the divergence is gone.
Do **not** leave a softened marker behind — a stale `CYRUP-DELTA` is what produced this
backlog. Do not touch the `windows_home_from` marker; that is the `:106` task.

## R6. Tests

**What CAN be guarded, and runs on this Linux host** — in `path.rs`'s existing
`#[cfg(test)] mod tests` (path.rs:489), which already carries the
`#[allow(clippy::unwrap_used, …)]` header these need:

1. `fd_config_dir_prefers_the_known_folder_over_the_home_default` — **THE RED TEST**.
   `windows_config_dir_from(None, || Some(PathBuf::from(r"D:\Redirected\Roaming")), PathBuf::from(r"C:\Users\u"))`
   must equal `D:\Redirected\Roaming`, not `C:\Users\u\AppData\Roaming`. Fails today for the
   mechanical reason that is the point of the item: the leg does not exist, and neither does
   the function — exactly how CFG-072's test was RED (see its doc comment, path.rs:818-830).
   Add the empty-string case: `Some(OsString::new())` must fall through to the probe, not
   be taken as a config dir (`dir_inner`'s `.filter(|s| !s.is_empty())`).
2. `fd_config_dir_appdata_outranks_the_known_folder` — a set, non-empty `%APPDATA%` wins,
   and the probe closure must **not** run (assert with a `Cell<bool>` the closure sets;
   this pins `or_else`'s laziness, i.e. no syscall on the common path).
3. `fd_config_dir_falls_back_to_home_roaming_when_both_are_absent` —
   `(None, || None, C:\Users\u)` ⇒ `C:\Users\u\AppData\Roaming`.
4. `fd_config_dir_xdg_requires_an_absolute_value` — `xdg_config_dir_from`: absolute wins,
   relative and empty fall back to `home/.config`. **GREEN today** — stated plainly: this is
   a regression guard over the extraction in P1, not a proof of the fix.
5. `fd_config_dir_resolves_home_with_std_env_home_dir` — the F1/F2 half. The eager-home gate
   is not purely testable without mutating the process environment (`unsafe` under edition
   2024, and it races every other test in the binary — the reason CFG-072 used extraction in
   the first place). Use this file's own established idiom instead: the `include_str!("path.rs")`
   source assertion at path.rs:856-880 — assert `fd_config_dir`'s body contains
   `std::env::home_dir()` and does not call this module's `home_dir()`. Weaker than a
   behavioural test; it is what is available, and it is precedented in-file.

**What CANNOT be verified here — stated, not worked around:**

* That the P2 body **compiles or links at all.** `#[cfg(windows)]` code is not type-checked
  on a Linux host, `cargo` is barred for this pass, the `x86_64-pc-windows-msvc` target is
  not installed, and there is **no CI workflow in-tree** (`/home/user/cyrup/.github` does not
  exist). The names are verified against the windows-sys 0.61.2 source (R2), which is the
  strongest check available without a compiler; it is not a compile.
  Exec-pass gate: `cargo check --target x86_64-pc-windows-msvc -p cyrup-tools` on a host
  with that target, or a Windows box.
* That `SHGetKnownFolderPath` returns `S_OK` and the expected path at runtime, and that the
  free/decode pair is sound — Windows only.
* The originating precondition itself (`%APPDATA%` unset **and** a redirected roaming
  folder) cannot be produced in this container. Unchanged from the audit's own note.
* F1's Linux-observable case (`HOME` unset ⇒ passwd fallback) — the *std* behaviour is
  verified (R2/F1, run here); the resulting cyrup-vs-fd difference is not executed as a test,
  because doing so needs a subprocess with a scrubbed environment (the `cyrup-it` integration
  pattern), not a unit test.

## Open questions for David

1. **Dependency call.** Option A (direct `windows-sys` edge, no new package, ~30 lines of
   `unsafe` FFI cyrup owns and cannot compile-check in-repo) is prescribed. Option B (take
   `etcetera` 0.11 — the literal crate fd links; zero `unsafe`; one new package; loses the
   pure tests) is the fallback. Confirm A, or say the word and exec takes B.
2. **Scope of F1/F2.** The prescription folds them in — they are the same ten lines, and F1
   is the *same* gap on a platform where it is executable. Say if you want them split into
   their own task instead; they should not be dropped either way.
3. **Verification gate.** With no Windows CI and no `.github/`, how does the `cfg(windows)`
   arm get checked before merge — a local `cargo check --target x86_64-pc-windows-msvc`, or
   is adding that target to a CI job the real prerequisite here?

## The accept case, argued for David

Honestly: on a normally provisioned Windows account the session manager sets `%APPDATA%`,
so the Windows precondition reaches a small population — service accounts, scrubbed or
sanitized environments, some CI containers. The cost of closing is real: FFI this project
cannot compile-check on its own infrastructure today. That is the whole case, and it is not
strong enough — because **F1 shows the identical failure is reachable on Linux with zero
`unsafe`**, so an "accept" scoped to Windows leaves a live, executable gap open behind it.
**Recommend CLOSE.** If David wants the `unsafe` deferred regardless, the coherent partial
is: take Option B (etcetera) — it closes everything with no `unsafe` at all — rather than
accepting the divergence.
