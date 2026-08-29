---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/path.rs:161"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 21:00
---

# Close the `SHGetKnownFolderPath` gap in `fd_config_dir`

The live marker is [`crates/cyrup-tools/src/path.rs`](../../../crates/cyrup-tools/src/path.rs)
**lines 161-165**, on the doc comment of `fd_config_dir` (fn body **166-183**):

> `**[CYRUP-DELTA — the Windows arm omits etcetera's SHGetKnownFolderPath fallback]**`

Classified a **capability gap** (a caller can observe a difference) by the audit that reviewed
all 87 `CYRUP-DELTA` markers against pi at `e8682309`. It was written by an agent; nobody
authorized the divergence. **Disposition: CLOSE.** The prescription below is the single
required implementation path — not a menu.

---

## 1. What upstream does (verified, not remembered)

pi does not compute a config directory at all. It shells out to the `fd` binary
([`tmp/pi/packages/coding-agent/src/core/tools/find.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts)
`:225` `ensureTool("fd")`, args built `:235-267`, `spawn` `:269`) and passes **no** `env`
option, so the child inherits pi's environment verbatim — the same `%APPDATA%` cyrup would
read. `grep -rn "global-ignore" tmp/pi/packages/` returns **nothing**, and fd's
`read_global_ignore` is `!(no_ignore || rg_alias_ignore() || no_global_ignore_file)`
([`tmp/fdsrc/fd-find-10.5.0/src/main.rs`](../../../tmp/fdsrc/fd-find-10.5.0/src/main.rs)`:318-320`),
so it is **true on every pi call**.

> Nuance worth recording: pi resolves `fd` as a *system* `fd`/`fdfind` if one is on PATH,
> else downloads the **latest** GitHub release
> (`tmp/pi/packages/coding-agent/src/utils/tools-manager.ts:30-46`, `:250`). `fd-find-10.5.0`
> is the vendored *reference*, not a pin. The `etcetera = "0.11"` dependency
> ([`tmp/fdsrc/fd-find-10.5.0/Cargo.toml`](../../../tmp/fdsrc/fd-find-10.5.0/Cargo.toml)`:106-107`)
> is what fixes the mechanism.

fd's global-ignore block
([`tmp/fdsrc/fd-find-10.5.0/src/walk.rs`](../../../tmp/fdsrc/fd-find-10.5.0/src/walk.rs)`:371-385`):

```rust
if config.read_global_ignore
    && let Ok(basedirs) = etcetera::choose_base_strategy()          // :371-372
{
    let global_ignore_file = basedirs.config_dir().join("fd").join("ignore");  // :374
    if global_ignore_file.is_file() { … }                                       // :375
}
```

`etcetera` 0.11.0 — read at
`/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/etcetera-0.11.0/src/` — is **four**
mechanisms, and `path.rs` reproduces part of two:

| # | mechanism | etcetera source | in `path.rs` today |
|---|---|---|---|
| a | strategy pick: `Windows` on windows, **`Xdg` everywhere else incl. macOS** | `base_strategy.rs:53-61` (`cfg_if!`; `create_strategies!(Apple, Xdg)` — the *second* arg is the base strategy) | ✅ correct, and documented at `path.rs:151-154` |
| b | home resolved **eagerly**, and its failure kills the whole strategy | `windows.rs:117-121` / `xdg.rs:169-173` both do `home_dir: crate::home_dir()?`; `etcetera::home_dir()` is `std::env::home_dir().ok_or(HomeDirError)` (`lib.rs:112-114`) | ❌ **missing** — see §2, F1/F2 |
| c | Windows config dir = `dir_inner("APPDATA")` with a **lazy** `SHGetKnownFolderPath` leg | `windows.rs:190-196` (`config_dir` → `data_dir`), `windows.rs:123-130` (`dir_inner`), `windows.rs:132-177` (`dir_crt`), `:179-182` (the `None` stub) | ❌ **the marked gap** |
| d | Xdg config dir: `$XDG_CONFIG_HOME` only if `is_absolute()`, else `home/.config` | `xdg.rs:175-182` + `:194-196` | ✅ matches |

`dir_inner` verbatim (`windows.rs:123-130`):

```rust
std::env::var_os(env)
    .filter(|s| !s.is_empty())
    .map(PathBuf::from)
    .or_else(|| Self::dir_crt(env))     // ← or_else: the syscall is LAZY
```

`dir_crt` (`windows.rs:132-177`, gated `#[cfg(all(windows, not(target_vendor = "uwp")))]`) maps
`"APPDATA" => FOLDERID_RoamingAppData` / `"LOCALAPPDATA" => FOLDERID_LocalAppData`
(`windows.rs:150-154`) and calls `SHGetKnownFolderPath`. Its own comment
(`windows.rs:130`) says it is kept in sync with `home-0.5.11/crates/home/src/windows.rs`.
Only the `APPDATA` arm is reachable from `config_dir`, so cyrup's port is a legitimate
**specialization**, not a truncation.

## 2. What cyrup does, and the three ways it diverges

`fd_config_dir` ([`path.rs`](../../../crates/cyrup-tools/src/path.rs)`:166-183`):

```rust
#[cfg(windows)] { if APPDATA set && non-empty → it; else home_dir()\AppData\Roaming }
#[cfg(not(windows))] { if XDG_CONFIG_HOME absolute → it; else home_dir()/.config }
```

**D1 (the marker, `:161-165`) — the known-folder leg is absent.** Windows session with
`%APPDATA%` unset/empty and a redirected roaming folder: fd reads
`<known-folder>\fd\ignore`, cyrup reads `<home>\AppData\Roaming\fd\ignore` (usually absent)
and excludes nothing. `find` silently over-includes. Windows-only; **not producible in this
container.**

**D2 — `home_dir()` is the wrong home, and this one IS reachable on Linux.** `path.rs:88-100`
resolves home from `HOME` only on non-Windows. `etcetera` uses `std::env::home_dir()`.
**Verified empirically on this host** (standalone `rustc 1.98.0 (88d9e12ae 2026-08-18)` probe,
`#![deny(deprecated)]`, built clean → `std::env::home_dir` is **not deprecated** on the pinned
toolchain):

```
--- with HOME ---     HOME env = Some("/root")   home_dir() = Some("/root")
--- without HOME ---  HOME env = None            home_dir() = Some("/root")   # getent passwd 0 → /root
```

So with `HOME` unset, fd reads `<passwd-home>/.config/fd/ignore` and cyrup reads nothing —
**the identical over-inclusion failure, on a platform where it executes.** The gap was filed
as Windows-only. It is not.

**D3 — the eager-home gate is missing, and diverges the other way.** Per (b), when home is
unresolvable fd's `if let Ok(basedirs)` fails and it registers **no** global ignore at all,
even with `%APPDATA%`/`$XDG_CONFIG_HOME` set. cyrup returns a directory there, so cyrup
**excludes more** than pi. Same function, one line.

D1/D2/D3 are the same ten lines. Fix them together.

## 3. Prescription — exact sites, exact changes

### P1 — `crates/cyrup-tools/src/path.rs`, replace the body of `fd_config_dir` (`:166-183`)

Split into etcetera's own two halves — eager home, then a **pure** config-dir computation with
the syscall **injected** — and add the missing leg. The injection is not stylistic: it is the
only shape that makes the ordering assertable from a Linux runner, and it reproduces the
CFG-072 pattern this same file already established (`windows_home_from`, `path.rs:121-136`,
pinned by the test at `path.rs:885-914`).

```rust
fn fd_config_dir() -> Option<PathBuf> {
    // etcetera resolves home EAGERLY and a failure kills the strategy: `Windows::new()`
    // (`windows.rs:117-121`) / `Xdg::new()` (`xdg.rs:169-173`) both `crate::home_dir()?`,
    // which is `std::env::home_dir()` (`lib.rs:112-114`), and fd's `if let Ok(basedirs)`
    // (fd 10.5.0 `walk.rs:371-372`) then registers NO global ignore file at all.
    // `std::env::home_dir`, NOT this module's `home_dir()`: that one mirrors Node's
    // `os.homedir()` for `expand_home`, a different upstream. Calling the same function
    // etcetera calls is parity by construction on every target.
    let home = std::env::home_dir()?;
    #[cfg(windows)]
    {
        Some(windows_config_dir_from(
            std::env::var_os("APPDATA"),
            known_folder_roaming_appdata,
            home,
        ))
    }
    #[cfg(not(windows))]
    {
        Some(xdg_config_dir_from(std::env::var_os("XDG_CONFIG_HOME"), home))
    }
}

/// `etcetera::base_strategy::Windows::{dir_inner, data_dir}` (`windows.rs:123-130`, `:190-196`)
/// as a pure function of its three inputs. `or_else` keeps the probe LAZY — no syscall
/// whenever `%APPDATA%` is usable.
#[cfg(any(windows, test))]
fn windows_config_dir_from(
    appdata: Option<std::ffi::OsString>,
    known_folder: impl FnOnce() -> Option<PathBuf>,
    home: PathBuf,
) -> PathBuf {
    appdata
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(known_folder)                       // ← THE MISSING LEG
        .unwrap_or_else(|| home.join("AppData").join("Roaming"))
}

/// `etcetera::base_strategy::Xdg::{env_var_or_none, config_dir}` (`xdg.rs:175-182`, `:194-196`).
#[cfg(any(not(windows), test))]
fn xdg_config_dir_from(xdg: Option<std::ffi::OsString>, home: PathBuf) -> PathBuf {
    xdg.map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
}
```

Notes the exec pass must not "improve" away:

* etcetera's default is the literal `".config/"` (`xdg.rs:194-196`). The trailing slash is
  cosmetic — `PathBuf` compares and joins by component — so `".config"` is identical. Do not
  add it back as a "fix".
* `home_dir()` at `path.rs:88-100` **stays**, unchanged and still called by `expand_home`
  (`path.rs:67`, `:76`). It mirrors Node's `os.homedir()`; the two functions mirror different
  upstreams and must not be merged. This also means `windows_home_from` remains reachable, so
  the `:106` sibling marker and its two tests keep their subject.

### P2 — new symbol `known_folder_roaming_appdata`, same file, directly after `fd_config_dir`

Port of `dir_crt("APPDATA")` (`windows.rs:132-177`). Every symbol below was read in
`windows-sys` **0.61.2** at
`/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/windows-sys-0.61.2/`:

| symbol | file:line | exact form |
|---|---|---|
| `SHGetKnownFolderPath` | `src/Windows/Win32/UI/Shell/mod.rs:573` | `fn(rfid: *const core::GUID, dwflags: u32, htoken: Foundation::HANDLE, ppszpath: *mut core::PWSTR) -> core::HRESULT` |
| `FOLDERID_RoamingAppData` | `Shell/mod.rs:3100` | `GUID::from_u128(0x3eb685db_65f9_4cf6_a03a_e3ef65729f3d)` |
| `KF_FLAG_DONT_VERIFY` | `Shell/mod.rs:3695` | `: KNOWN_FOLDER_FLAG = 16384i32` |
| `KNOWN_FOLDER_FLAG` | `Shell/mod.rs:3746` | `= i32` → **`dwflags` needs `as u32`** |
| `CoTaskMemFree` | `src/Windows/Win32/System/Com/mod.rs:84` | `fn(pv: *const core::ffi::c_void)` |
| `S_OK` | `src/Windows/Win32/Foundation/mod.rs:9452` | `HRESULT = 0x0_u32 as _` |
| `HANDLE` | `Foundation/mod.rs:5119` | `= *mut core::ffi::c_void`; `null_mut()` = calling user |
| `PWSTR` / `HRESULT` | `src/core/mod.rs:9` / `:7` | `= *mut u16` / `= i32` |

Feature gates confirmed: `Win32_UI_Shell` guards `pub mod Shell`
(`src/Windows/Win32/UI/mod.rs:15-16`), `Win32_System_Com` guards `pub mod Com`
(`src/Windows/Win32/System/mod.rs:11-12`); the three names exist at
`windows-sys-0.61.2/Cargo.toml:85,184,272`. `SHGetKnownFolderPath` carries **no** extra
per-item `cfg` — the `#[cfg(feature = "Win32_UI_Shell_Common")]` at `:570` belongs to
`SHGetKnownFolderIDList` at `:571`, and the one at `:575` to `SHGetMalloc` at `:576`.

```rust
/// `etcetera::base_strategy::Windows::dir_crt("APPDATA")` (`windows.rs:132-177`), which
/// etcetera in turn keeps in sync with `home-0.5.11/crates/home/src/windows.rs`.
/// Specialized to `FOLDERID_RoamingAppData`: `config_dir` delegates to `data_dir`, which
/// only ever asks for `"APPDATA"` (`windows.rs:190-196`), so the `LOCALAPPDATA` arm of
/// etcetera's match (`windows.rs:150-154`) is unreachable from fd's global-ignore path.
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
        let mut path: *mut u16 = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(
            &FOLDERID_RoamingAppData,
            KF_FLAG_DONT_VERIFY as u32, // KNOWN_FOLDER_FLAG is i32; dwflags is u32
            std::ptr::null_mut(),
            &mut path,
        );
        if hr != S_OK {
            CoTaskMemFree(path.cast());
            return None;
        }
        let mut len = 0usize;
        while *path.add(len) != 0 {
            len += 1;
        }
        let s = OsString::from_wide(std::slice::from_raw_parts(path, len));
        CoTaskMemFree(path.cast());
        Some(PathBuf::from(s))
    }
}

/// UWP has no `SHGetKnownFolderPath`; etcetera returns `None` there (`windows.rs:179-182`).
#[cfg(all(windows, target_vendor = "uwp"))]
fn known_folder_roaming_appdata() -> Option<PathBuf> {
    None
}
```

Two deliberate departures from etcetera, stated so a reviewer does not read them as drift:

* etcetera's stub is `#[cfg(not(all(windows, not(target_vendor = "uwp"))))]` because its
  `Windows` strategy is constructible on unix. cyrup's is reached only from the
  `#[cfg(windows)]` arm of `fd_config_dir`, so the stub narrows to
  `all(windows, target_vendor = "uwp")`; a `not(windows)` stub would be dead code on unix.
* etcetera declares `unsafe extern "C" { fn wcslen(buf: *const u16) -> usize; }`
  (`windows.rs:145-147`). The inline NUL scan avoids that extern, which matters because
  `cyrup-tools` pulls `libc` only under `[target."cfg(unix)".dependencies]`.

### P3 — manifests

**No new package enters the graph.** `windows-sys` **0.61.2** is already resolved at
[`Cargo.lock`](../../../Cargo.lock)**`:8110-8112`** with **28** dependents (tokio, rustix,
tempfile, mio, socket2, wasmtime, …), and its dep `windows-link` 0.2.1 at
**`Cargo.lock:8017-8019`**. `Cargo.lock` has **no** `etcetera` entry
(`grep -c '^name = "etcetera"' Cargo.lock` → `0`).

Root [`Cargo.toml`](../../../Cargo.toml), in `[workspace.dependencies]` (block starts `:116`),
with a rationale comment in the house style used by `ring` (`:212-215`), `url` (`:217-222`) and
`rustix` (`:265-274`) — say explicitly that it adds no crate to the graph and that the feature
list is etcetera 0.11.0's own, verbatim from
`etcetera-0.11.0/Cargo.toml:45-47`:

```toml
windows-sys = { version = "0.61", features = ["Win32_Foundation", "Win32_System_Com", "Win32_UI_Shell"] }
```

[`crates/cyrup-tools/Cargo.toml`](../../../crates/cyrup-tools/Cargo.toml), mirroring the
existing `[target."cfg(unix)".dependencies] libc` block:

```toml
[target."cfg(windows)".dependencies]
windows-sys = { workspace = true }
```

### P4 — retire the marker

Delete the `**[CYRUP-DELTA — …]**` paragraph, `path.rs:161-165`. **Nothing replaces it**;
the divergence is gone. Do not leave a softened marker — a stale `CYRUP-DELTA` on fixed code
gets re-filed as an open gap, which is precisely how this backlog formed (the same reasoning
already written at `path.rs:460-462`). Extend the surviving bullet list at `path.rs:156-159`
to name all three legs of `dir_inner`. **Do not touch the `:106` marker** on
`windows_home_from` — that is the sibling task, and
`cfg072_the_widening_carries_a_delta_naming_what_it_extends` (`path.rs:916-944`) scans
`include_str!("path.rs")` for the last `[CYRUP-DELTA` occurring **before** `fn windows_home_from`
(`:121`). The `:161` marker sits after it, so deleting it cannot affect that test — and no new
marker may be introduced above `:121`.

### P5 — `crates/cyrup-tools/src/lib.rs:17`

The crate doc currently asserts:

> `//! ... The only `unsafe` in the crate is the isolated unix process-group code in [`ops::local`].`

That becomes false. Amend it to name the second site (the Windows known-folder probe in
`path`). `#![deny(unsafe_code)]` at `lib.rs:18` is **deny, not forbid**, and the
`#[allow(unsafe_code)] + // SAFETY:` shape is already established at seven non-test sites —
`ops/local/command.rs:14`, `ops/local/signal.rs:24,64,110,137`, `ops/local/fs.rs:202`,
`ops/local/guard.rs:78` (plus `ops/local/tests/mod.rs:133`). No lint attribute changes.

### Why this path and not `etcetera`

Taking `etcetera = "0.11"` would be the *identical* code fd links and needs no `unsafe`, but it
(i) adds a genuinely new third-party package that this workspace's manifest treats as a
ratification event, and (ii) makes the ordering **untestable** — etcetera's legs are private,
so nothing about the precedence stays assertable from a Linux runner. P1/P2 add no package,
reuse a compilation unit already in the build, and keep the whole precedence pure and pinned.
Take P1-P5 as written.

## 4. The required guard (RED today)

One test, in `path.rs`'s existing `#[cfg(test)] mod tests` (`#[cfg(test)]` at `:529`, the
`#[allow(clippy::unwrap_used, …)]` header at `:530`, `mod tests {` at `:531`). It compiles and
runs **on this Linux host** because the legs are pure and the probe is injected.

```rust
/// The missing middle leg of `etcetera`'s `dir_inner` (`windows.rs:123-130`) and its laziness.
///
/// RED before this change for a mechanical reason that IS the item: neither
/// `windows_config_dir_from` nor the known-folder leg exists — exactly how CFG-072's own test
/// was RED (`path.rs:872-914`).
#[test]
fn fd_config_dir_prefers_the_known_folder_over_the_home_default() {
    use std::cell::Cell;
    use std::ffi::OsString;
    let home = || PathBuf::from(r"C:\Users\u");
    let probe = || Some(PathBuf::from(r"D:\Redirected\Roaming"));

    // Unset %APPDATA% → the known folder wins over `home\AppData\Roaming`.
    assert_eq!(
        windows_config_dir_from(None, probe, home()),
        PathBuf::from(r"D:\Redirected\Roaming")
    );
    // `.filter(|s| !s.is_empty())`: EMPTY is not a config dir; it falls through to the probe.
    assert_eq!(
        windows_config_dir_from(Some(OsString::new()), probe, home()),
        PathBuf::from(r"D:\Redirected\Roaming")
    );
    // Both absent → etcetera's `unwrap_or_else` default.
    assert_eq!(
        windows_config_dir_from(None, || None, home()),
        PathBuf::from(r"C:\Users\u\AppData\Roaming")
    );
    // `or_else` is LAZY: a usable %APPDATA% wins AND the syscall never runs.
    let ran = Cell::new(false);
    assert_eq!(
        windows_config_dir_from(
            Some(OsString::from(r"E:\App")),
            || { ran.set(true); None },
            home()
        ),
        PathBuf::from(r"E:\App")
    );
    assert!(!ran.get(), "the known-folder syscall must not run when %APPDATA% is usable");
}
```

Nothing else is required. (A companion assertion on `xdg_config_dir_from` — absolute wins,
relative and empty fall back to `home/.config` — would be **green today** and is a regression
guard over the P1 extraction, not a proof of the fix; add it or not, but do not present it as
coverage of this gap.)

## 5. What cannot be verified on this host — stated plainly

* **The syscall does not run here.** `known_folder_roaming_appdata` is Windows-only. No test in
  this repo executes `SHGetKnownFolderPath`, and none can. The originating precondition
  (`%APPDATA%` unset **and** a redirected roaming folder) is likewise not producible in this
  container. D1 remains proved by source on both sides only.
* **Compilation of the `cfg(windows)` arm CAN be checked here** — this corrects the previous
  pass, which claimed it could not. `rustup target list --installed` reports
  **`x86_64-pc-windows-gnu`** installed, with a full `rust-std` at
  `~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-pc-windows-gnu/lib/`.
  `cargo check` performs no link step, so:

  ```
  cargo check --target x86_64-pc-windows-gnu -p cyrup-tools
  ```

  type-checks P1/P2 in place. (`target_vendor` there is `"pc"`, so the real `dir_crt` arm — not
  the UWP stub — is the one compiled.) `x86_64-pc-windows-msvc` is **not** installed, and there
  is **no CI workflow in-tree** (`/home/user/cyrup/.github` does not exist), so the gnu check is
  the whole available gate. Scope it with `-p cyrup-tools`: a workspace-wide windows-gnu check
  may surface unrelated pre-existing breakage that is not this task's.
* **`std::env::home_dir()`'s Windows branch was not read** — the `rust-src` component is not
  installed on this toolchain. It does not matter: etcetera calls that exact function
  (`lib.rs:112-114`), so calling it too is parity **by identity of the callee**, whatever its
  Windows implementation is. Do not write a claim about what it does there.

## 6. Definition of Done

1. `fd_config_dir` resolves home via `std::env::home_dir()` and returns `None` when it fails
   (D2 + D3 closed), and its Windows arm consults `known_folder_roaming_appdata` between the
   `%APPDATA%` read and the `home\AppData\Roaming` default (D1 closed).
2. `windows_config_dir_from` and `xdg_config_dir_from` exist as pure functions with the probe
   injected, per P1.
3. The `[CYRUP-DELTA — … SHGetKnownFolderPath …]` paragraph at `path.rs:161-165` is **deleted**,
   with no marker replacing it; the `:106` marker on `windows_home_from` is untouched and
   `cfg072_the_widening_carries_a_delta_naming_what_it_extends` still passes.
4. `fd_config_dir_prefers_the_known_folder_over_the_home_default` exists and passes; it fails to
   compile on the pre-change tree (the function it calls does not exist).
5. `windows-sys` is declared once in `[workspace.dependencies]` with a no-new-crate rationale and
   consumed by `cyrup-tools` under `[target."cfg(windows)".dependencies]`; `Cargo.lock` gains **no
   new package** (`windows-sys 0.61.2` and `windows-link 0.2.1` were already resolved).
6. `crates/cyrup-tools/src/lib.rs:17` no longer claims the unix process-group code is the crate's
   only `unsafe`.
7. `cargo check --target x86_64-pc-windows-gnu -p cyrup-tools` succeeds, and the existing
   `cyrup-tools` suite is unchanged on the host target.

## 7. Corrections to the previous AUG pass (2026-08-28 02:11)

Re-derived; these citations had drifted or were wrong:

| claimed | actual |
|---|---|
| `windows-sys` 0.61.2 at `Cargo.lock:8091-8093`; `windows-link` at `:7998-8000` | **`:8110-8112`** and **`:8017-8019`** |
| `lib.rs:19` `#![deny(unsafe_code)]` | **`lib.rs:18`** |
| `guard.rs:79` | **`guard.rs:78`** (and an 8th site exists: `ops/local/tests/mod.rs:133`) |
| `windows_home_from, path.rs:113-131`, test at `:818-849` | fn **`:121-136`**, test **`:885-914`** |
| `mod tests (path.rs:489)` | **`:531`** (`#[cfg(test)]` at `:529`) |
| source-scan idiom at `path.rs:856-880` | **`path.rs:916-944`** |
| "`x86_64-pc-windows-msvc` target is not installed … no way to compile-check" | true for msvc, but **`x86_64-pc-windows-gnu` IS installed with full std** — the `cfg(windows)` arm is checkable here |
| offered Options A/B and three "open questions for David" | collapsed to one required path; upstream (pi → fd → etcetera) answers the semantics, and the gnu target answers the verification question |
| did not mention `lib.rs:17` | that sentence becomes false and must be amended (P5) |
