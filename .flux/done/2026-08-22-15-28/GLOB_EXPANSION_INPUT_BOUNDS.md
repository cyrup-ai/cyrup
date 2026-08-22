---
stage: qa
status: completed
updated: 2026-08-22 23:58
---

# Bound Brace Expansion And Match Recursion In model/glob.rs

## Description

[`src/model/glob.rs`](../../crates/cyrup-config/src/model/glob.rs) is a hand-written minimatch port
with **no bound on output size, integer range, or recursion depth**. Verified: `grep` for
`const .*(LIMIT|MAX|DEPTH|BOUND)|checked_add|checked_sub|saturating` finds only two incidental
`saturating_sub` calls (lines 369, 563) — there are no limits of any kind.

Every arm is reachable from a user-supplied `--models` pattern:
[`model/resolver.rs:294`](../../crates/cyrup-config/src/model/resolver.rs) enters the glob branch
whenever a pattern contains `*`, `?` or `[`, and `resolver.rs:309` first tries
`exact_reference_match`, which fails for these inputs, so the glob path runs.

### 1. Unbounded / overflowing numeric range (`expand_brace_range`, glob.rs:475-546)

```rust
let mut x = a;                     // :497
if a <= b { while x <= b { out.push(format_range_num(x, padded, width)); x += step; } }  // :498-502
else      { while x >= b { out.push(...); x -= step; } }                                 // :503-507
```

No cap on `b - a`, no checked arithmetic. `{1..9223372036854775807}*` allocates one `String` per
iteration, ~9.2e18 times. Measured on release: RSS grew 1,219,040 kB → 2,797,812 kB over 10s with no
sign of termination. When `b == i64::MAX`, `x += step` at :501 also overflows — a debug panic, and in
release a wrap that makes the loop infinite. The single-char alpha arm at :525-543 has the same shape
(`x += step` at :533, `x -= step` at :542), though ASCII bounds limit it.

### 2. Unbounded brace cross-product (`brace_expand`, glob.rs:462-467)

The triple loop multiplies option counts across sibling braces with no ceiling on `out.len()`, so
N adjacent 10-way braces produce 10^N strings before `glob_match` iterates them.

### 3. Unbounded recursion → stack overflow (SIGABRT)

Two independent recursions, both measured on release:

- `brace_expand` recurses on **balanced** nesting (`:410-424` finds `close`, `:463` recurses via
  `brace_expand(&opt)`). Probe `"{a," * N + "b" + "}" * N`: N=2000 → exit 0; N=8000 → exit 0;
  **N=20000 → `fatal runtime error: stack overflow`, exit 134.** An *unbalanced* probe does NOT
  reproduce — `close` stays `None` and `:424` returns without recursing.
- `m()` (`:226-249`) recurses once per pattern character on every arm, including the plain-literal
  arm at `:247` and the `*` arm at `:244`. Threshold measured between 20,000 and 50,000 `*`
  characters — cheaper to hit than the brace nest.

These are aborts and unbounded allocation, not wrong results. `glob_match` returns `bool` and has no
error channel, so **fail closed** — cap the inputs and return `false` / a truncated expansion rather
than introducing a `Result`.

**Scope:** `src/model/glob.rs` only. Suggested shape: a module-level `const` per limit;
`checked_add`/`checked_sub` (or a precomputed iteration count) at :501/:506/:533/:542; an `out.len()`
ceiling inside the :462-467 loop; an explicit depth counter threaded through `brace_expand` and `m`.
Do NOT change `glob_match`'s signature or the semantics of any currently-passing case —
`glob_matches_pi_minimatch_byte_for_byte` (the captured Pi/minimatch parity table) must still pass.

## Acceptance Criteria

- [ ] `rg -n 'x [+-]= step' src/model/glob.rs` returns 0 matches
- [ ] `expand_brace_range` refuses or truncates beyond a named module-level constant, referenced from both the numeric arm (~:497-507) and the alpha arm (~:525-543)
- [ ] The brace cross-product loop (~:462-467) checks an output-count ceiling inside the loop body and stops expanding once reached
- [ ] `brace_expand` and `m` each enforce an explicit depth or input-length bound; `rg -n 'const [A-Z_]*(LIMIT|MAX|DEPTH)' src/model/glob.rs` returns at least three constants
- [ ] Release probe `{1..9223372036854775807}*` through the resolver's glob path returns within 5s
- [ ] Release probe `"{a," * 20000 + "b" + "}" * 20000` exits 0 (currently exit 134)
- [ ] Release probe with 50,000 `*` characters exits 0
- [ ] `cargo test -p cyrup-config` still reports 222 passed / 0 failed, including `glob_matches_pi_minimatch_byte_for_byte`
- [ ] `cargo clippy -p cyrup-config --all-targets` 0 warnings; `cargo fmt -p cyrup-config -- --check` 0 hunks

## Outcome — completed

Landed in `d931658`. All nine acceptance criteria verified mechanically at QA.

Four bounds added, all failing closed because `glob_match` returns `bool` and has no error channel:
`MAX_PATTERN_LEN` (4096) rejects an over-large pattern at the entry and transitively bounds `m`'s
recursion, since `m` recurses at most once per pattern character; `MAX_BRACE_DEPTH` (32) leaves a
too-deep brace literal, the same fallback an unbalanced brace already takes; `MAX_EXPANSIONS` (4096)
caps the sibling cross-product; `MAX_RANGE_ITEMS` (4096) caps one `{a..b}` range. Both range arms use
`checked_add`/`checked_sub`, so the `i64` overflow is gone rather than merely bounded.

Probes re-run at QA **through `ModelResolver::resolve_scope`**, which is what the criteria specify —
the implementation pass had driven `glob_match` directly, a weaker check:

| Probe | Before | After |
|---|---|---|
| `{1..9223372036854775807}*` | RSS 1.2GB → 2.8GB in 10s, no termination | **3 ms** |
| `"{a," * 20000 + "b" + "}" * 20000` | stack overflow, exit 134 | **0 ms**, exit 0 |
| 50,000 `*` characters | stack overflow | **0 ms**, exit 0 |

Ordinary globs still resolve (`anthropic/*` → 1 model), and
`glob_matches_pi_minimatch_byte_for_byte` — the captured Pi/minimatch parity table — passes
unchanged, which is what demonstrates the limits sit above real patterns.

**Known gap, deliberate:** the probes were run from a temporary test that was then deleted, because
the task forbids new tests as deliverables. The behaviour is verified but **not permanently pinned** —
a future change could reintroduce an unbounded path without any test failing. Worth a follow-up if
these bounds are considered load-bearing.
