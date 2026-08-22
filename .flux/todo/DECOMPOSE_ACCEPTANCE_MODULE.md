---
stage: qa
status: needs-rework
updated: 2026-08-22 23:10
---

# Decompose exec/acceptance.rs — QA Rework

## QA verdict: 9/10

The decomposition is **done and correct**. Verified independently this round:

- **28 files**, largest 1,125 lines, split along the two-API boundary as specified: `lattice/`
  (9 files) and `model/` (17 files, incl. `report/` and `verify/` subtrees).
- **Public surface exactly preserved** — lattice 18 → 18, model 64 → 64, set difference empty in
  both directions. **Zero files modified outside `src/exec/acceptance/`**, which is what actually
  proves it held.
- Every file opens with a `//!` doc; `acceptance/mod.rs` has no `fn`/`struct`/`impl`; the
  collapse-status banner is present in `model/mod.rs`.
- `cargo check --workspace --all-targets` clean · **2,484 tests pass** · clippy at the two
  pre-existing findings · rustdoc warnings under `exec/acceptance` at **3**, below the baseline of 5.
- The `AcceptanceVerifyResult` impl (R1) rejoined its struct; the three shadowed names (R2) resolve
  half-aware; cross-half references are absolute, no globs.

One defect survived, and it is the exact class flagged as the risk of this split.

---

## Item 1 — a corrupted string literal (the only outstanding item)

[`model/report/parse.rs:586` and `:595`](../../crates/cyrup-ext-subagents/src/exec/acceptance/model/report/parse.rs)
sit inside the raw string opened at line 584, and both gained four spaces the original never had:

```rust
            r#"done
```acceptance-report
    {                          // <- line 586; original is `{` with no indent
  "criteria_satisfied": {"id": "C 1", "status": "Done", "evidence": "did it"},
  …
    }                          // <- line 595; original is `}` with no indent
```"#,
```

The original is at `acceptance.rs:8109`/`:8118` (backup: `.flux`-adjacent scratch copy) and reads
`{` and `}` flush left. This is fenced-JSON fixture data fed to the report parser; the parser is
lenient about leading whitespace, so **every test still passes** — the corruption is silent.

**Cause.** The hoist of `pub mod model` dedented every line by four, including string-literal
interiors. The repair pass then restored indentation by matching line *content* against the set of
damaged originals — and `{` / `}` are far too common for content matching to be unambiguous, so
these two were restored from a different literal's damage.

**Fix:** remove the four leading spaces from `model/report/parse.rs:586` and `:595`. Nothing else in
the tree is affected — the block-level comparison below confirms 26 multi-line literals in, 26 out,
with only this one differing.

## Item 2 — add the check that would have caught it

The exec-stage content proof used a whitespace-normalizing comparison
(`' '.join(line.split())`). That is **structurally blind to indentation damage** — it cannot detect
this bug class at all, which is why the split shipped with it. The proof needs a second,
whitespace-sensitive pass restricted to string literals.

Add this to the verification and run it to zero before declaring done:

```python
# Mark ONLY string/raw-string interiors — not comments, whose reflow is legitimate.
# Compare byte-for-byte: indentation inside a literal is semantically significant.
from strmask import string_interior_lines as S      # scratch: strmask.py
import collections, glob
O = collections.Counter(S(open(ORIGINAL).read()))
N = collections.Counter()
for f in glob.glob('**/*.rs', recursive=True):
    N += collections.Counter(S(open(f).read()))
assert not (O - N) and not (N - O), (O - N, N - O)
```

The stronger form — comparing whole multi-line literals as blocks rather than as loose lines — is
what localized this defect to one literal in one file, and is worth keeping for the same reason:
line-level multisets can cancel out an over-restoration against an under-restoration.

## Acceptance criteria

- [ ] `model/report/parse.rs:586` and `:595` match the original byte-for-byte (`{` and `}`, no
      leading spaces).
- [ ] The whitespace-sensitive string-literal comparison reports zero lost and zero gained lines.
- [ ] The block-level literal comparison reports 26 multi-line literals on both sides, none
      differing.
- [ ] `cargo check --workspace --all-targets` clean, `cargo test -p cyrup-ext-subagents` still
      2,484 passing, clippy still 2, rustdoc under `exec/acceptance` still 3.
- [ ] No file outside `crates/cyrup-ext-subagents/src/exec/acceptance/model/report/parse.rs` is
      modified.
