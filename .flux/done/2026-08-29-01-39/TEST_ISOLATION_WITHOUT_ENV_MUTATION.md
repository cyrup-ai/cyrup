---
stage: qa
status: completed
updated: 2026-09-01 18:00
---

# Test isolation without mutating process-global state — two item docs in `paths.rs`

Every item the previous review named is closed, and the end-to-end pass it mandated caught
two more that the review itself had missed. That worked.

It worked only as far as it was scoped. The DoD said *"read `paths.rs` lines 1–55"*, and
lines 1–55 are now correct. **Lines 76 and 100 carry the same defect** — a doc naming keys
and rungs the code does not have — and one of them contradicts the item immediately below
it.

That scope was mine, and writing it that way is what let these through. This round's check
is the whole file.

---

## 1. `resolve_agent_dir`'s doc names two keys; it reads three

[`crates/cyrup-ext-subagents/src/paths.rs`](../../crates/cyrup-ext-subagents/src/paths.rs),
~line 100:

> *"pi `getAgentDir()` … against an explicitly supplied home:
> `$CYRUP_AGENT_DIR`/`$PI_CODING_AGENT_DIR` (with `~`/`~/` expansion against `home`) if set
> and non-empty, else `<home>/.cyrup/agent`."*

`resolve_agent_dir_from` — the **next item in the file** — says the opposite, correctly:

> *"Reads `CYRUP_CODING_AGENT_DIR` as well as `CYRUP_AGENT_DIR` and `PI_CODING_AGENT_DIR`.
> That middle key is the fix, not a widening…"*

The key this doc omits is MCP-139 gap 1. The doc still describes the defect as the
behaviour, one line above the doc that describes the fix.

Two smaller things in the same sentence, worth correcting while it is being rewritten:

- **"if set and non-empty"** — the shared ladder filters with `non_blank`, which trims, so a
  whitespace-only value is treated as unset too.
- The key list should name [`cyrup_config::paths::ENV_AGENT_DIR_KEYS`] rather than restate
  the spellings, so this doc cannot drift from the ladder again.

## 2. `home_dir`'s doc omits a rung

Same file, ~line 76:

> *"`os.homedir()` as this crate resolves it: `CYRUP_HOME` -> `HOME` -> [`std::env::temp_dir`]."*

The ladder is `CYRUP_HOME` → `HOME` → `ambient_home` → `temp_dir`, where `ambient_home` is
`directories::BaseDirs`, then `HOME`:

```rust
// cyrup_config::paths
pub fn cyrup_home_dir_from(env: EnvLookup<'_>) -> Option<PathBuf> {
    cyrup_home_override_from(env).or_else(ambient_home)   // <- the missing rung
}
```

Not cosmetic. On unix `BaseDirs` reads `$HOME`, so rungs 2 and 3 agree and the omission is
invisible. **On Windows `HOME` is usually unset and `BaseDirs` answers with the real user
profile** — so the doc promises a temp dir exactly where the code returns the user's home.

The paragraph below it ("Never returns an empty path: with neither variable set the process
temp dir answers") inherits the same gap: with neither variable set, `BaseDirs` normally
answers, not the temp dir. The *conclusion* — never empty, always absolute — still holds and
should stay.

---

## Definition of done for this rework

1. Both docs above name the keys, rungs and terminals the code actually has.
2. **Every `///` and `//!` in `paths.rs` — the whole file, not a line range — has been read
   and checked against the code it documents.** The previous two rounds each fixed the
   region under discussion and left an adjacent one wrong; a line-scoped pass is what this
   file keeps defeating.
3. `cargo test -p cyrup-ext-subagents --lib paths::`
4. `cargo clippy -p cyrup-ext-subagents --all-targets --all-features -- -D warnings`
5. `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc -p cyrup-ext-subagents --no-deps --all-features`
   reports only the two pre-existing `spawn_detached_runner` links in `background.rs`.

---

## Known-good — do not redo, do not re-verify

Checked against the code this round, independently of the exec's account:

- **`paths.rs`'s module doc (lines 1–68)** is accurate throughout: the five `CYRUP_HOME`
  places; five agent-dir resolvers across three key sets; the byte-identical quote and the
  `cyrup-intercom → cyrup-ext-subagents` edge that made the copy necessary (verified at
  `cyrup-intercom/Cargo.toml:50`); `cyrup-config` depending only on `cyrup-core` and
  `cyrup-provider`; four crates consuming the ladders with two — this crate and `cyrup-mcp` —
  already holding the edge; both deliberate exceptions; both routes and the
  `String`-vs-`OsString` reason; the `missions/store.rs` history and its conclusion now
  pointing at `cyrup_config::paths::cyrup_home_dir_from`.
- **"One resolver goes straight to `cyrup-config`"** — verified: exactly one *ladder* call
  bypasses this module (`native_supervisor:1846`); the other direct hits are the shared
  consts, one of which is the documented run-scratch exception and one a child-env writer.
- **`Roots::from_env`'s doc**, the two probe-verified agent-dir tests, all eight `unsafe`
  env-mutation blocks, and steps 0–8 of the original plan.
- **Verification**: 22/22 in `paths::`; clippy clean; rustdoc at the two pre-existing links;
  and from earlier rounds 2593 / 226 / 213 and 448/448 under nextest.

## Not this task's defects — recorded so they are not re-diagnosed

- `background.rs:32` and `:349` link to `spawn_detached_runner` and do not resolve.
  Pre-existing `///` lines this branch never edited.
- `cyrup-tui/src/markdown/highlight.rs:363` fails `clippy -D warnings` (`question_mark`).
- `cyrup-it`'s `mcp` and `ext` binaries are unsound under plain `cargo test` by their own
  documented design and pass under nextest; `cyrup-it` is feature-gated out of the
  `cargo test --workspace` merge gate.
