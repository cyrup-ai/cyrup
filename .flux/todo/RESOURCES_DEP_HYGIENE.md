---
stage: new
status: done
updated: 2026-08-22 23:07
---

# Route cyrup-resources Dependencies Through The Workspace

**Owns files:** `Cargo.toml`, `crates/cyrup-resources/Cargo.toml`, `crates/cyrup-mcp/Cargo.toml`

## Description

Two dependency declarations bypass the workspace convention this repo otherwise follows.

### 1. `notify` is pinned locally while a workspace entry already exists

Verified:

```
Cargo.toml:161                          notify = { version = "8.2.0" }
crates/cyrup-ext-subagents/Cargo.toml:51    notify = { workspace = true }
crates/cyrup-permission-system/Cargo.toml:49 notify = { workspace = true }
crates/cyrup-resources/Cargo.toml:29    notify = "8.2.0"     <-- the odd one out
```

Every other consumer takes the workspace edge. The versions happen to agree today, so this is purely
a drift hazard: bumping the workspace entry would silently leave this crate behind.

**Fix:** `crates/cyrup-resources/Cargo.toml:29` becomes `notify = { workspace = true }`. No feature
changes — the workspace entry is plain default features, matching the local pin exactly.

### 2. `toml` is pinned identically in two crates with no workspace entry

```
crates/cyrup-resources/Cargo.toml:35  toml = "1.1.2"
crates/cyrup-mcp/Cargo.toml:111       toml = "1.1.2"
(no [workspace.dependencies] entry)
```

Two independent pins on the same version is the exact situation `[workspace.dependencies]` exists to
prevent.

**Fix:** add `toml = { version = "1.1.2" }` to root `[workspace.dependencies]` alongside the other
ratified externals, with a one-line rationale naming both consumers (cyrup-resources manifest
parsing, cyrup-mcp config), matching the commenting style of the neighbouring entries. Then switch
both crates to `toml = { workspace = true }`.

Leave `gix` and `serde_yml` alone — single-consumer deps, no drift risk, out of scope.

## Acceptance Criteria

- [ ] `grep -n notify crates/cyrup-resources/Cargo.toml` shows `{ workspace = true }`
- [ ] Root `Cargo.toml` declares `toml`; both consuming crates use `{ workspace = true }`
- [ ] `Cargo.lock` is unchanged apart from nothing — versions are identical, so the lock must not move
- [ ] `cargo check --workspace` clean
- [ ] `cargo test -p cyrup-resources -p cyrup-mcp` unchanged
