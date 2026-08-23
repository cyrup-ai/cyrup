---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Expand ~ / $HOME / ${HOME} in rule patterns before matching

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | absent |
| **Upstream area** | policy model / pattern matching |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream expands the three home-directory spellings in every pattern at compile time so a home-
relative rule matches an absolute path value; the port compiles patterns verbatim, while the value
side is already resolved to an absolute path, so home-relative rules never match.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

expand-home.ts:15-42 (`HOME_PREFIXES = ["~", "$HOME", "${HOME}"]`, `expandHomePath`); wildcard-
matcher.ts:50 (`const expanded = foldSeparators(expandHomePath(pattern), options)` inside
`compileWildcardPattern`); config-schema.ts:64 and :73 (pattern keys documented to support `~/`
and `$HOME/`)

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/wildcard.rs:73-113
`compile_with_case_insensitive` escapes and anchors the pattern with no home expansion; `rg -n
'HOME|home_dir|expand' /home/user/cyrup/crates/cyrup-permission-system/src/wildcard.rs` returns
nothing. The value side is expanded and absolutized: manager.rs:873-880 `path_resource_from_input`
→ common.rs:70-95 `normalize_path_resource_for_permission` (expands `~`, resolves against cwd).

## Why it matters

A rule such as `"read:~/.ssh/*": "deny"` or `"$HOME/.aws/*": "deny"` is inert: the compiled
pattern still contains the literal `~`, while the checked resource is `/home/user/.ssh/id_rsa`, so
the deny never matches and the access falls through to a broader allow or the category default.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
