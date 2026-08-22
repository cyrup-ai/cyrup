---
stage: qa
status: completed
updated: 2026-08-23 00:55
---

# Decompose exec/acceptance.rs — QA Rework

## Where this stands

The decomposition is **done and QA-verified at 9/10**. 28 files under `exec/acceptance/`, largest
1,125 lines, split along the two-API boundary. Public surface preserved exactly (lattice 18 → 18,
model 64 → 64, set difference empty both ways), zero files touched outside `src/exec/acceptance/`,
workspace check clean, 2,484 tests passing, clippy at its two pre-existing findings, rustdoc under
`exec/acceptance` at 3 (below the baseline of 5).

**One defect outstanding: two corrupted bytes-worth of indentation inside a string literal.**
Everything else on this task is complete.

---

## Item 1 — restore two literal lines (the entire code change)

### Exactly what is wrong

[`model/report/parse.rs`](../../crates/cyrup-ext-subagents/src/exec/acceptance/model/report/parse.rs)
lines **586** and **595**, inside the raw string opened at line 584:

```rust
584            r#"done
585 ```acceptance-report
586     {                      ← has 4 leading spaces; must have none
587   "criteria_satisfied": {"id": "C 1", "status": "Done", "evidence": "did it"},
        …
594   "manual_notes": "nothing else"
595     }                      ← has 4 leading spaces; must have none
596 ```"#,
```

The pre-split original (`acceptance.rs:8109` and `:8118`) has `{` and `}` flush left. Every other
line of this literal already matches byte-for-byte, and the `r#"` opener's own indentation
legitimately changed — that line's first character is *code*, not literal content.

### The fix

Delete the four leading spaces on lines 586 and 595. Nothing else. No other literal in the tree
differs — see the block-level check below, which reports 26 multi-line literals on both sides with
exactly this one differing.

### Why no test catches it, and why it is still worth fixing

The fixture is fed to `parse_acceptance_report`, and the fence pipeline trims before parsing:
[`model/report/fences.rs:100`](../../crates/cyrup-ext-subagents/src/exec/acceptance/model/report/fences.rs)
does `.map(|m| m.body.trim().to_string())` and `:142` does `let trimmed = body.trim();`. Leading
whitespace on the `{` is discarded before `serde_json` ever sees it, so
`normalizes_status_aliases_field_aliases_and_singleton_shapes`
([`parse.rs:578`](../../crates/cyrup-ext-subagents/src/exec/acceptance/model/report/parse.rs))
passes either way.

That is precisely the argument for fixing it by hand rather than waiting for a test to demand it:
this is a faithful port of upstream fixture data, the drift is invisible to the suite, and an
invisible drift in ported data is how a port stops being faithful. The change is behaviour-neutral
by construction.

## Item 2 — the check, inlined

The QA task pointed at a `strmask.py` in the session scratchpad. **That file does not exist under
the repo and the scratchpad does not survive this container**, so the check as cited is not
runnable. It is reproduced here in full; write it to a scratch path and run it.

Two things it must do that the exec-stage proof did not:

- **Compare byte-for-byte.** The exec-stage proof normalized with `' '.join(line.split())`, which
  makes it structurally incapable of detecting indentation damage — it could not fail on this bug.
- **Mark string literals only, not comments.** A first attempt at this check flagged doc comments
  too, drowning the two real lines in ~600 legitimately reflowed doc lines. Comments are reflowed by
  the split on purpose; literals must never be.

```python
import re, glob, collections

ORIGINAL = '<path to the pre-split acceptance.rs>'

def string_interior_lines(text):
    """Lines whose first character sits inside a string/raw-string literal. Comments excluded:
    their reflow is legitimate, a literal's is not."""
    n, i = len(text), 0
    inlit = bytearray(n)
    while i < n:
        c = text[i]
        if c == '/' and i + 1 < n and text[i + 1] == '/':
            j = text.find('\n', i); i = n if j == -1 else j; continue
        if c == '/' and i + 1 < n and text[i + 1] == '*':
            lvl, j = 1, i + 2
            while j < n and lvl:
                if text[j] == '/' and j + 1 < n and text[j + 1] == '*': lvl += 1; j += 2; continue
                if text[j] == '*' and j + 1 < n and text[j + 1] == '/': lvl -= 1; j += 2; continue
                j += 1
            i = j; continue
        m = re.compile(r'r(#*)"').match(text, i)
        if c == 'r' and m:
            term = '"' + m.group(1)
            j = text.find(term, m.end()); j = n if j == -1 else j
            for k in range(m.end(), j): inlit[k] = 1
            i = (j + len(term)) if j < n else n; continue
        if c == '"':
            j = i + 1
            while j < n:
                if text[j] == '\\': j += 2; continue
                if text[j] == '"': break
                j += 1
            for k in range(i + 1, min(j, n)): inlit[k] = 1
            i = j + 1; continue
        if c == "'":
            m2 = re.compile(r"'(\\.|[^\\'\n])'").match(text, i)
            i = m2.end() if m2 else i + 1; continue
        i += 1
    out, off = [], 0
    for l in text.split('\n'):
        if l and off < n and inlit[off]:
            out.append(l)                      # NO normalization — bytes as they are
        off += len(l) + 1
    return out

O = collections.Counter(string_interior_lines(open(ORIGINAL).read()))
N = collections.Counter()
for f in sorted(glob.glob('**/*.rs', recursive=True)):   # run from exec/acceptance/
    N += collections.Counter(string_interior_lines(open(f).read()))
print('lost  ', sum((O - N).values()), list(O - N))
print('gained', sum((N - O).values()), list(N - O))
```

Run it from `crates/cyrup-ext-subagents/src/exec/acceptance/`. **Verified: it prints
`lost 2 ['{', '}']` / `gained 2 ['    {', '    }']` before the fix and `lost 0` / `gained 0` after
it** — the two-line change closes the gap completely and nothing further is required.

Keep the block-level variant too, which compares each multi-line literal as one unit rather than as
loose lines. It is what localized this defect to a single literal in a single file; a line-level
multiset can cancel an over-restoration in one literal against an under-restoration in another and
report nothing.

## Definition of done

- `model/report/parse.rs:586` and `:595` are `{` and `}` with no leading whitespace, matching the
  original byte-for-byte.
- The inlined check prints `lost 0` / `gained 0`.
- `cargo check --workspace --all-targets` clean; `cargo test -p cyrup-ext-subagents` still 2,484
  passing; clippy still exactly 2 findings; rustdoc under `exec/acceptance` still 3.
- `crates/cyrup-ext-subagents/src/exec/acceptance/model/report/parse.rs` is the only file modified.
