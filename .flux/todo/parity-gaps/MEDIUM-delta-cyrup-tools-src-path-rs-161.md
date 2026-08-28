---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/path.rs:161"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
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
