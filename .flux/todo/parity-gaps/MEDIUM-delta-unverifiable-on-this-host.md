---
title: CYRUP-DELTA claims that cannot be verified from Linux
priority: MEDIUM
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Deltas unverifiable on this host

These rest on a platform this container cannot exercise. They are neither confirmed
nor refuted — they need a Windows runner or an explicit accepted-unverified decision.

- `crates/cyrup-tools/src/path.rs:106`
  - The `HOMEDRIVE`/`HOMEPATH` widening in `home_dir`. What I CAN verify here: the cyrup side. `windows_home_from` is extracted as a pure function with `#[cfg(any(windows, test))]` and its precedence is pinned by `cfg072_homedrive_homepath_is_the_documented_fallback_after_userprofile` (path.rs:618-646), so on Windows with USERPROFILE and HOME unset but the pair set, cyrup returns `D:\Users\hd` and expands `~`. What I CANNOT verify in this container: the pi side of the claim. The marker asserts pi resolves NOTHING in that state, which rests on Node's `os.homedir()` -> libuv `uv_os_homedir` checking `USERPROFILE` and then falling back to the `GetUserProfileDirectoryW` SYSCALL rather than to the HOMEDRIVE/HOMEPATH pair. That is a Win32/libuv behaviour with no libuv source in this tree and no Windows host to run it on; the reference tree only shows that pi's own TypeScript never reads either variable (`grep -c HOMEDRIVE` over packages/ is 0, which I confirmed at e8682309). So the DIRECTION of the divergence — 'cyrup widens, pi resolves none' vs 'both resolve, possibly to different directories' — cannot be settled here. If the marker is right it is a capability gap of the same shape as path.rs:161 (cyrup expands `~` where pi leaves it literal, so a `read`/`write`/`bash` cwd path silently resolves differently); if libuv's syscall fallback happens to return the same directory the pair spells, it is mechanism-only. It needs one run on a Windows host with USERPROFILE and HOME cleared to decide, and I am not willing to record a verdict I could not test. Note also that this same `home_dir` feeds `fd_config_dir`'s fallback leg, so it compounds with path.rs:161; etcetera uses `std::env::home_dir()` there, which is a third resolution rule again.