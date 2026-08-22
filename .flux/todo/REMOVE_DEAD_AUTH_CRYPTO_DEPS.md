---
stage: new
status: done
updated: 2026-08-22 19:42
---

# Delete The Dead auth::pkce Module And Its Three Unused Dependencies

## Description

`cyrup-config` carries three direct dependencies that exist only to support one dead module — and a
fourth problem (a hand-pinned `base64`) that disappears with them.

### `oauth2` is entirely unreferenced

[`Cargo.toml:24`](../../crates/cyrup-config/Cargo.toml) declares `oauth2 = "5.0.0"`. Measured:
`grep -rn 'oauth2' crates/cyrup-config/src` returns **0 hits**. `cyrup-config` is the only crate in
the workspace with a direct `oauth2` manifest entry — every other consumer (`cyrup`,
`cyrup-ext-subagents`, `cyrup-mcp`, `cyrup-resources`) reaches it only transitively *through*
`cyrup-config`. `cargo tree -p oauth2 -e normal --prefix none | sort -u | wc -l` measures **179**
crates in that subtree, compiled for zero call sites.

### `base64` and `sha2` are used by exactly one dead module

`grep -rn 'base64\|sha2\|Sha256' crates/cyrup-config/src` returns exactly 6 lines, all inside
`pub mod pkce` at [`src/auth.rs:517-534`](../../crates/cyrup-config/src/auth.rs):

```
src/auth.rs:519:    use base64::Engine;
src/auth.rs:520:    use sha2::{Digest, Sha256};
src/auth.rs:524:        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
src/auth.rs:529:        let mut hasher = Sha256::new();
src/auth.rs:532:        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
```

Two functions, `verifier_from_bytes` and `challenge`.
`grep -rn 'auth::pkce\|config::pkce' crates/ --include=*.rs` returns **0 hits** — nothing in the
workspace calls either. Its only exercise is the in-file test `pkce_challenge_is_stable` at
`auth.rs:945-952`.

It is also redundant.
[`crates/cyrup-provider/src/auth/oauth/pkce.rs`](../../crates/cyrup-provider/src/auth/oauth/pkce.rs)
is the live implementation (`base64url_encode`, `pkce_challenge`, `generate_pkce`), a documented 1:1
port of pi's `pkce.ts` asserted against the same RFC 7636 Appendix B vector, and it is what the real
OAuth flows in `cyrup-provider/src/auth/oauth/{anthropic,xai,openrouter,openai_codex}.rs` use.

### The `base64` pin is drifted — and vanishes with the deletion

`Cargo.toml:18` is the only literal `base64 = "0.22.1"` in the workspace.
`cyrup-ext-subagents:67`, `cyrup-mcp:125` and `cyrup-permission-system:56` all use
`{ workspace = true }` against `base64 = { version = "0.22" }` at the workspace root `Cargo.toml:194`.
`Cargo.lock` already resolves **two** base64 majors (0.22.1 at `Cargo.lock:410`, 0.23.1 at `:416`),
so a workspace bump really would strand cyrup-config on its own copy. Deleting the dep resolves this
outright — no workspace pin needed.

**Accuracy note while editing manifests:** `sha2` is **not** drifted. There is no `sha2` entry in
the workspace table, and all four consumers pin the identical literal `sha2 = "0.11.0"`
(cyrup-config:27, cyrup-ext-subagents:73, cyrup-mcp:123, cyrup-permission-system:66) — removing
cyrup-config's line is a pure deletion, not a de-pinning. The workspace `Cargo.toml:260` comment
still lists `oauth2, sha2, base64` under "add to the table when first used" even though base64 was
since added at `:194`; do not read that comment as a rationale for the literal pin.

## Acceptance Criteria

- [ ] `grep -rni 'pkce' crates/cyrup-config/` returns no matches (module `auth.rs:517-534` and test `auth.rs:945-952` both removed)
- [ ] `crates/cyrup-config/Cargo.toml` no longer contains a `base64`, `sha2`, or `oauth2` dependency line
- [ ] `cargo build -p cyrup-config` and `cargo build --workspace` succeed
- [ ] `cargo tree -e normal -i oauth2 --workspace` no longer lists `cyrup-config` as a direct dependent, or reports oauth2 absent from the graph
- [ ] `cargo test -p cyrup-config` shows no failures beyond those tracked in `TEST_FAILURES.md`
