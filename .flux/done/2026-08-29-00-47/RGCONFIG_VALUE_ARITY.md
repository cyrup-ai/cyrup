---
stage: qa
status: completed
updated: 2026-08-29 16:45
---

# Close the `$RIPGREP_CONFIG_PATH` value-leak for every known value-taking flag

## 1. The defect

[`RgFlags::parse`](../../crates/cyrup-tools/src/tools/rgconfig.rs) advances past a long flag itself,
but only a match arm that calls `take()` advances past that flag's **value**. A value-taking flag
that lands on the catch-all `_ => {}` therefore hands its argument back to the top of the loop,
where a leading `-` makes it parse as a flag in its own right and apply.

Fixed for three flags (`--engine`, `--pre`, `--pre-glob`) in
`MEDIUM-delta-cyrup-tools-grep-pcre2-and-preprocessor.md`, archived under
`.flux/done/2026-08-23-00-08/`. That task was scoped to the pcre2/preprocessor group and QA fenced
the rest off as separate work. This is that work.

## 2. The measurement — both halves now derived

Walked ripgrep 14.1.0's [`crates/core/flags/defs.rs`](../../tmp/ripgrep-14.1.0/crates/core/flags/defs.rs)
for every flag whose `is_switch()` returns `false` — the authoritative arity method on the `Flag`
trait — and cross-referenced against cyrup's `apply_long` arms and its short-flag arity guard.

### 2.1 Long flags — 20 leak

35 value-taking long flags exist; cyrup handles 15. These 20 reach the catch-all carrying a value:

`--after-context`, `--before-context`, `--color`, `--colors`, `--context`, `--context-separator`,
`--dfa-size-limit`, `--field-context-separator`, `--field-match-separator`, `--file`, `--generate`,
`--hostname-bin`, `--hyperlink-format`, `--max-columns`, `--path-separator`, `--regex-size-limit`,
`--regexp`, `--replace`, `--threads`, `--type-clear`

### 2.2 Short flags — 9 leak (this was NOT measured when the task was filed)

`parse`'s cluster loop hard-codes the value-taking shorts:

```rust
if matches!(ch, 'm' | 'E' | 'g' | 't' | 'T') {
```

ripgrep has **14** value-taking shorts. cyrup's guard covers 5. The other 9 are treated as switches,
so their value line leaks exactly as the long forms do:

| short | long | today |
|---|---|---|
| `-A` | `--after-context` | leaks |
| `-B` | `--before-context` | leaks |
| `-C` | `--context` | leaks |
| `-M` | `--max-columns` | leaks |
| `-e` | `--regexp` | leaks |
| `-f` | `--file` | leaks |
| `-j` | `--threads` | leaks |
| `-r` | `--replace` | leaks |
| **`-d`** | **`--max-depth`** | **leaks AND drops a setting cyrup honours — see §3** |

### 2.3 The failure shape

In ripgrep's documented one-argument-per-line config format:

```
--replace
-i
```

`--replace` is ignored, `-i` is then read as a top-level flag, and **the search silently becomes
case-insensitive**. `--max-columns` then `-v` inverts results; `--context` then `-F` makes the
pattern a literal; `-r` then `-i` does the same as the first example. A value that does *not* begin
with `-` is harmlessly ignored as a path argument, so the trigger is a value starting with `-`, or a
flag written with no value before another flag.

## 3. `-d` is a second, worse defect — a behaviour gap, not just a leak

Every flag cyrup honours by long name also has its short name honoured — **except one**. Checked
exhaustively over the 15 long-honoured flags:

```
OK  --case-sensitive -s   OK  --encoding -E   OK  --fixed-strings -F   OK  --follow -L
OK  --glob -g            OK  --ignore-case -i OK  --invert-match -v   OK  --line-regexp -x
OK  --max-count -m       GAP --max-depth -d   OK  --smart-case -S      OK  --text -a
OK  --type -t            OK  --type-not -T    OK  --word-regexp -w
```

`"max-depth" | "maxdepth" => self.max_depth = take().and_then(|v| v.parse().ok())` honours the long
form, but `'d'` is in neither the arity guard nor `apply_short_with_value`. So a config containing

```
-d
2
```

sets **no depth limit at all** in cyrup while real ripgrep limits the walk to two levels — a
different result set, not merely a dropped flag. `-d` must therefore be **honoured**, not
consume-and-dropped, so the two spellings of one flag stop disagreeing.

## 4. The design tension to respect, not reverse

[`rgconfig.rs`](../../crates/cyrup-tools/src/tools/rgconfig.rs)'s module header argues deliberately
against reproducing ripgrep's flag table, because pinning to one ripgrep version would turn a newer
rg's flag into a hard error instead of a search. **That argument stands. Do not reverse it.**

It does not conflict with this fix. For a flag from a *newer* ripgrep, cyrup genuinely cannot know
the arity, and the leak there is unavoidable and stays best-effort behind the catch-all. For the
flags in §2 the arity is known — they exist in the pinned 14.1.0 reference and are knowingly
ignored. Closing those is recognise-and-decline, exactly the distinction `"quiet" => {}`,
`PCRE2_IS_DECLINED` and `PREPROCESSOR_IS_DECLINED` already draw.

**Do not attempt a heuristic** such as "if the next line starts with `-`, it is not a value" — a
legitimate value can start with `-` (`--context-separator`, `--replace`), so the heuristic swaps one
silent wrong answer for another.

## 5. Required implementation path

### 5.1 `apply_long` — one grouped arm above the catch-all

Insert immediately before `// Everything else — inert through the JSON pipeline…`:

```rust
// Recognised, value CONSUMED, semantics dropped. These are ripgrep 14.1.0 flags that take a
// value and that this module does not act on. They must not reach the catch-all: `parse`
// advances past the flag but only `take()` advances past the VALUE, so an unconsumed
// value-taking flag hands its argument back to the top of the loop, where a leading `-` makes
// it parse as a flag and apply — `--replace` followed by `-i` silently turns the search
// case-insensitive. The catch-all stays for flags this module does not KNOW: a newer
// ripgrep's flag must still be ignored rather than become an error.
"after-context" | "before-context" | "color" | "colors" | "context" | "context-separator"
| "dfa-size-limit" | "field-context-separator" | "field-match-separator" | "file"
| "generate" | "hostname-bin" | "hyperlink-format" | "max-columns" | "path-separator"
| "regex-size-limit" | "regexp" | "replace" | "threads" | "type-clear" => {
    take();
}
```

### 5.2 `parse`'s cluster loop — widen the arity guard

```rust
// Value-taking shorts consume the rest of the cluster, else the next line. The set is
// ripgrep 14.1.0's `is_switch() == false` shorts; a short missing from here is treated as a
// switch and leaks its value line into the next loop iteration.
if matches!(
    ch,
    'm' | 'E' | 'g' | 't' | 'T' | 'd' | 'A' | 'B' | 'C' | 'M' | 'e' | 'f' | 'j' | 'r'
) {
```

### 5.3 `apply_short_with_value` — honour `-d`, drop the other eight

```rust
'm' => self.max_count = v.and_then(|v| v.parse().ok()),
'E' => self.encoding = v,
'g' => self.globs.extend(v.map(|g| (g, false))),
't' => self.types.extend(v),
'T' => self.types_not.extend(v),
// `-d` is `--max-depth`, which this module ALREADY honours by its long names. Leaving the
// short form on the catch-all made the two spellings of one flag disagree: `--max-depth 2`
// limited the walk and `-d 2` did nothing at all.
'd' => self.max_depth = v.and_then(|v| v.parse().ok()),
// Recognised, value consumed, semantics dropped — the short spellings of the §5.1 group.
'A' | 'B' | 'C' | 'M' | 'e' | 'f' | 'j' | 'r' => {}
_ => {}
```

### 5.4 Correct the module header's flag inventory

The header lists `--max-columns`, `-r/--replace` and "the context flags" among those that "parse and
change nothing". That is still true of their *semantics* and must stay — but it now reads as though
they are handled, when until this change they were actively harmful. Add one sentence to that
paragraph recording that these flags are recognised so their VALUE is consumed, and that the
catch-all remains for genuinely unknown flags.

## 6. Explicitly out of scope

- **Do not implement `-e/--regexp` or `-f/--file` semantics.** Consuming their value closes the
  leak; it does **not** reach parity, and the gap is much larger than this task. ripgrep's own doc:
  *"When \flag{file} or \flag{regexp} is used, then ripgrep treats all positional arguments as files
  or directories to search."* Under pi, `args.push("--", pattern, searchPath)` (`grep.ts:224`) means
  a config containing `-e foo` turns pi's **pattern into a path** and searches for `foo` instead.
  That is a real upstream divergence and belongs in its own task.
- **Do not implement `--dfa-size-limit`, `--regex-size-limit` or `--type-clear`**, even though
  `RegexMatcherBuilder` and `TypesBuilder` have the corresponding knobs. They are consume-and-drop
  here, unchanged from today; a later task may honour them.
- **Do not honour the context flags.** pi consumes only `event.type === "match"`
  ([`grep.ts:285`](../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts)) and builds its own
  context by reading the file, so `-A`/`-B`/`-C` from a config are genuinely unobservable in pi's
  output. The module header's existing claim is correct.
- **Do not remove or narrow the catch-all**, and do not make any flag an error. §4.

## 7. Guards (each fails today)

Extend `declined_value_taking_flags_consume_their_value`
([`grep.rs`](../../crates/cyrup-tools/src/tools/grep.rs)) or add a sibling beside it, in the same
register:

```rust
// Long forms: the value no longer escapes into the next iteration.
assert_eq!(RgFlags::parse("--replace\n-i\n"), RgFlags::default());
assert_eq!(RgFlags::parse("--max-columns\n-v\n"), RgFlags::default());
assert_eq!(RgFlags::parse("--context\n-F\n"), RgFlags::default());
// Short forms: same defect, separately guarded because the arity guard is a separate list.
assert_eq!(RgFlags::parse("-r\n-i\n"), RgFlags::default());
assert_eq!(RgFlags::parse("-M\n-v\n"), RgFlags::default());
assert_eq!(RgFlags::parse("-C\n-F\n"), RgFlags::default());

// `-d` is HONOURED, not dropped: the two spellings of `--max-depth` must agree.
assert_eq!(RgFlags::parse("-d\n2\n").max_depth, Some(2));
assert_eq!(RgFlags::parse("-d2\n").max_depth, Some(2));       // cluster-tail form
assert_eq!(RgFlags::parse("--max-depth\n2\n").max_depth, Some(2)); // passes today: the anchor

// Not passing for the wrong reason — the leaked lines are live flags on their own.
assert_eq!(RgFlags::parse("-i\n").case, Some(CaseMode::Insensitive));
assert!(RgFlags::parse("-v\n").invert_match);
assert!(RgFlags::parse("-F\n").fixed_strings);

// The catch-all still ignores a flag from a newer ripgrep rather than failing.
assert_eq!(RgFlags::parse("--some-future-flag\n"), RgFlags::default());
```

## 8. Definition of done

1. All 20 long flags in §2.1 reach an explicit arm that calls `take()` and discards the value.
2. All 9 short flags in §2.2 are in the arity guard in `parse`'s cluster loop.
3. `-d` sets `max_depth` identically to `--max-depth`, in both the next-line and cluster-tail forms;
   the other 8 shorts consume their value and drop it.
4. The catch-all still exists and still ignores unknown flags — a newer ripgrep's flag is not an
   error.
5. The module header records that these flags are recognised to consume their value (§5.4).
6. Every guard in §7 is in the tree, and each `✗` row is proven to fail without the change.
7. `cargo test -p cyrup-tools`, `cargo clippy -p cyrup-tools --all-targets` and
   `cargo doc -p cyrup-tools --no-deps --document-private-items` are clean.
