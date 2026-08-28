---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/find.rs:1"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 02:23
---

# Capability gap: `crates/cyrup-tools/src/tools/find.rs:1`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi spawns the real fd binary (find.ts:225 `ensureTool("fd")`, :269 `spawn(fdPath, args)`), downloading it if missing, and exposes `FindOperations { exists, glob }` (find.ts:55-71) so an extension can supply server-side globbing (`if custom operations provide glob(), use that instead of fd`, :168).

## What cyrup does

In-process `ignore::WalkBuilder` + `globset`, driven through `FsOps::walk`.

## What a caller sees

(a) fd's own glob dialect and traversal rules are replaced by globset/ignore — divergence here is version-dependent and unbounded rather than pinned. (b) pi's `fd is not available and could not be downloaded` / `Failed to run fd: ...` / `fd exited with code N` errors never occur. (c) pi's `FindOperations.glob` seam lets a remote backend do the glob remotely and return paths; cyrup's `FsOps::walk` forces enumeration-then-match, so a remote backend transfers the whole listing. See also path.rs:161 — the fd global-ignore file cyrup reproduces by hand is where these two diverge concretely on Windows.

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

---

# AUG PASS — 2026-08-28

Research-only. No source touched. **fd is no longer un-vendored for this question**: five
fd source tarballs were fetched into `./tmp/fdsrc/` (`fd-find-{8.7.1,9.0.0,10.2.0,10.3.0,10.5.0}`,
from `static.crates.io`), plus five `globset` and eight `ignore` versions into
`./tmp/fdsrc/gs/` and `./tmp/fdsrc/ig/`. Every claim below is read out of fd's own code at
a named file:line, not inferred from pi's in-source comments. That closes the standing
caveat at `docs/gap-analysis/04-cyrup-tools.md:1084` (*"Neither fd nor ripgrep is vendored
as a binary. TOOL-011 and TOOL-023 still rest on pi's in-source comment … rather than on
fd's matching code."*) for the fd half.

Headline: **the audit's framing is half right and half wrong, and the wrong half is the
scary one.** The glob dialect is *not* unbounded — fd's glob dialect **is** `globset` with
one option set, unchanged across fd 8.7.1 → 10.5.0, and cyrup already links the same crate.
What *is* divergent, and what the audit does not mention, is **result ordering**: fd sorts,
cyrup does not, and cyrup's current no-sort behaviour was landed on a premise fd's source
refutes.

---

## R1. What pi's `fd` actually is — the version story, stated honestly

`ensureTool("fd")` (`find.ts:225`) → `utils/tools-manager.ts:336-341`:

```ts
const existingPath = getToolPath(tool);
if (existingPath) return existingPath;
```

`getToolPath` (`tools-manager.ts:82-101`) checks pi's own `bin` dir first, then
**`systemBinaryNames: ["fd", "fdfind"]`** (`:33`) on `PATH`. Only if neither is found does
`downloadTool` run, and it resolves **`getLatestVersion(repo)`** — a live
`api.github.com/repos/sharkdp/fd/releases/latest` call (`:104-117`) — with exactly one pin:
`if (tool === "fd" && plat === "darwin" && architecture === "x64") version = "10.3.0";`
(`tools-manager.ts:250-252`).

So pi's fd is **whatever the host has** (Debian bookworm ships `fdfind` 8.7.0, Ubuntu 24.04
ships 9.0.0) or **the newest release on the day of first launch**. The audit calls cyrup's
divergence "version-dependent and unbounded"; the honest statement is that **pi's own
`find` semantics are version-dependent and unpinned too.** There is no single fd behaviour
to converge on — only a behaviour *band*. R2 measures that band, and it turns out to be
narrow.

## R2. The glob dialect is NOT unbounded — it is `globset`, and cyrup already links it

**fd's entire glob→pattern path is three lines, and they are byte-identical across every
fd release pi can reach:**

| fd version | `build_pattern_regex` glob arm | file:line |
|---|---|---|
| 8.7.1 | `GlobBuilder::new(pattern).literal_separator(true).build()?` then `glob.regex().to_owned()` | `src/main.rs:166-168` |
| 9.0.0 | identical | `src/main.rs:166-168` |
| 10.2.0 | identical | `src/main.rs:168-170` |
| 10.3.0 | identical | `src/main.rs:170-172` |
| 10.5.0 | identical | `src/main.rs:219-221` |

fd sets **no other `GlobBuilder` option** — no `case_insensitive`, no `backslash_escape`,
no `empty_alternates`, no `allow_unclosed_class`. It then compiles the emitted regex string
itself: `RegexBuilder::new(&pattern_regex).case_insensitive(!config.case_sensitive).dot_matches_new_line(true).build()`
(`fd-find-10.5.0/src/main.rs:541-545`).

**And the `globset` versions in that band are behaviourally identical.** Locked versions,
read from each crate's shipped `Cargo.lock`:

| fd | globset | ignore |
|---|---|---|
| 8.7.1 | 0.4.13 | 0.4.20 |
| 9.0.0 | 0.4.14 | (0.4.21 req) |
| 10.2.0 | 0.4.14 | — |
| 10.3.0 | 0.4.16 | — |
| 10.5.0 | **0.4.19** | **0.4.31** |
| **cyrup** | **0.4.18** (`Cargo.lock:3432-3434`) | **0.4.26** (`Cargo.lock:3882-3884`) |

Diffed `src/glob.rs` across the whole chain 0.4.13 → 0.4.20:

* **0.4.13 → 0.4.14** — regex engine swapped `regex::bytes::Regex` → `regex_automata::meta::Regex`,
  plus `let-else` / field-init-shorthand refactors. The **token emitter is byte-identical**:
  a targeted diff of `tokens_to_regex` between 0.4.13 and 0.4.18 yields exactly one line,
  `for tok in tokens` → `for tok in tokens.iter()`.
* **0.4.14 → 0.4.15** — `char_to_escaped_literal` / `bytes_to_escaped_literal` rewritten to
  avoid allocation; same output string.
* **0.4.15 → 0.4.16** — `glob.rs` unchanged (empty diff).
* **0.4.16 → 0.4.18** — adds `allow_unclosed_class` (`glob.rs:236`, `:663`), **default
  `false`** (`glob.rs:246`), i.e. the pre-0.4.18 error behaviour (`0.4.16 glob.rs:942`
  `ErrorKind::UnclosedClass`) is preserved unless opted in; plus `Debug`/`arbitrary` derives.
* **0.4.18 → 0.4.19 → 0.4.20** — `glob.rs` diff is a single line
  (`self.chars.peek().map(|&ch| ch)` → `.copied()`), then empty.

**Verdict on audit claim (a), glob half: REFUTED.** fd's glob dialect is `globset`'s, it has
not moved in the reachable version band, and cyrup compiles the same crate. "Vendoring fd's
matcher" is not required, and the closure surface is not open-ended — it is the finite list
in R3.

## R3. The dialect differences that DO exist — case by case

### D1 — smart case: fd runs its predicate over the **regex**, cyrup over the **glob string**

fd (`main.rs:252-257`) computes `case_sensitive` from
`pattern_regexps.iter().any(|pat| pattern_has_uppercase_char(pat))` — i.e. over the string
`globset` **emitted**, not over the user's glob. `pattern_has_uppercase_char`
(`fd-find-10.5.0/src/regex_helper.rs:5-37`) parses that string with
`regex_syntax::ParserBuilder::new().utf8(false)` and walks the HIR:

```rust
HirKind::Literal(Literal(bytes)) => match std::str::from_utf8(bytes) {
    Ok(s)  => s.chars().any(|c| c.is_uppercase()),
    Err(_) => bytes.iter().any(|b| char::from(*b).is_uppercase()),
},
HirKind::Class(Class::Unicode(ranges)) => ranges.iter().any(|r| r.start().is_uppercase() || r.end().is_uppercase()),
HirKind::Class(Class::Bytes(ranges))   => ranges.iter().any(|r| char::from(r.start()).is_uppercase() || char::from(r.end()).is_uppercase()),
```

cyrup instead scans the glob string: `pattern.chars().any(char::is_uppercase)`
(`globmatch.rs:38-40`), and its doc comment at `globmatch.rs:29-35` asserts the two are
equivalent because *"globset's emitter introduces no uppercase letter — non-ASCII bytes
become lowercase `\xNN` escapes"*.

**That equivalence argument is false, and the counterexample is a character class.**
For a `Literal` token the argument holds — `char_to_escaped_literal('é')` emits `\xc3\xa9`,
regex-syntax re-joins the bytes into one `Literal`, `from_utf8` recovers `"é"`, not
uppercase. But `Token::Class` emits its range endpoints through the *same* escape
(`globset-0.4.18/src/glob.rs:726-741`) **inside `[...]`**, where regex-syntax cannot rejoin
them: under `(?-u)` they become a `Class::Bytes`, and its endpoints are inspected as
*individual bytes* through `char::from(u8)`. Every UTF-8 lead byte in `0xC2..=0xDE`
(excluding `0xD7`) maps to `Â..Þ`, which **is** uppercase.

**Concrete, unit-testable case — `[a-ÿ]bc.ts`:**

| | |
|---|---|
| emitted regex | `(?-u)^[a-\xc3\xbf]bc\.ts$` (globset `glob.rs:676-741`) |
| fd's verdict | class normalizes to bytes `0x61..=0xC3`; `char::from(0xC3) == 'Ã'` is uppercase ⇒ **case-SENSITIVE** |
| cyrup's verdict | `"[a-ÿ]bc.ts"` has no `char::is_uppercase` ⇒ **case-INSENSITIVE** |
| caller-visible | a file `Abc.ts`: cyrup's `(?i)` ASCII-folds `a-z ⊂ [0x61-0xC3]` and so admits `A-Z` ⇒ **cyrup returns it, fd/pi does not** |

General rule: **any 2-byte UTF-8 character inside a `[...]` class flips fd to
case-sensitive while cyrup stays case-insensitive.** `[à-ÿ]*.md` is the realistic shape.
Direction of the error is over-inclusion, so it is silent.

### D2 — `literal_separator`: fd sets it unconditionally, cyrup conditionally — **not observable, but load-bearing**

fd: `.literal_separator(true)` always (`main.rs:220`). cyrup:
`.literal_separator(full_path)` (`globmatch.rs:59`), where `full_path = pattern.contains('/')`.

**Disproven as a divergence, with the proof, so it is not re-litigated:** the option changes
only two emitted tokens — `Token::Any` ⇒ `[^/]` vs `.` and `Token::ZeroOrMore` ⇒ `[^/]*` vs
`.*` (`globset-0.4.18/src/glob.rs:703-713`). When `full_path` is false cyrup matches against
the **basename**, which contains no `/` on either platform (globset's `Candidate` normalizes
`\` ⇒ `/` before matching, `pathutil.rs:75-85`). And `.` vs `[^/]` cannot differ on a
newline either: globset's own `new_regex` sets `dot_matches_new_line(true)`
(`globset-0.4.18/src/lib.rs:272-274`), exactly as fd's `RegexBuilder` does (`main.rs:544`).
`GlobMatcher::is_match` goes straight to the regex (`glob.rs:142-148`); the
`MatchStrategy` fast paths — the other place `literal_separator` is consulted — are
`#[cfg(test)]`-only in 0.4.18 (`glob.rs:157-159`), so they never run.

Still prescribed for change (P2) as hardening: the equality holds only because of a chain of
three unrelated facts, any of which a future globset release could break.

### D3 — path bytes: fd matches **raw `OsStr` bytes** on unix, cyrup matches a **lossy `String`**

fd: `pat.is_match(&filesystem::osstr_to_bytes(search_str.as_ref()))` (`walk.rs:517-522`),
and `osstr_to_bytes` on unix is `Cow::Borrowed(input.as_bytes())` — **no loss**
(`filesystem.rs:103-106`). cyrup: `let abs_posix = to_posix(&w.path)` (`find.rs:227`), and
`to_posix` is `path.to_string_lossy()` (`globmatch.rs:225-232`) — every invalid byte becomes
`U+FFFD` (3 bytes, `EF BF BD`).

Caller-visible on unix with a non-UTF-8 filename. Pattern `caf?.ts` against a Latin-1 file
`caf\xE9.ts`: fd's `(?-u)^caf[^/]\.ts$` tests one **byte** ⇒ `\xE9` matches ⇒ **fd lists it**.
cyrup sees `caf\u{FFFD}.ts`, whose replacement char is three bytes ⇒ **no match**. Same
divergence for any single-byte quantifier or literal over invalid UTF-8.

**Windows is not affected**: fd's own `osstr_to_bytes` there is `input.to_string_lossy()`
(`filesystem.rs:108-116`) — the identical loss. So this is a unix-only item, which is why it
survived a Windows-scoped review.

### D4 — Windows: pi's separator-class rewrite degrades fd's recursive star, and cyrup does not reproduce the defect

`find.ts:263-265`:

```ts
if (process.platform === "win32") effectivePattern = effectivePattern.replaceAll("/", String.raw`[/\\]`);
```

This runs **after** the `**/` prepend (`:257-262`), so `src/**/*.ts` reaches fd as
`` `**[/\\]src[/\\]*.ts` ``.

globset's `parse_star` (`globset-0.4.18/src/glob.rs:901-917`) only promotes `**` to
`Token::RecursivePrefix` when the next character `is_separator`; `[` is not, so it takes the
`:909-911` arm and pushes **two `ZeroOrMore`s**. With `literal_separator(true)` that is
`[^/]*[^/]*` — a `**` that **cannot cross a directory boundary**. On Windows, pi's
path-containing `find` patterns are therefore anchored one component below the drive root.

cyrup does no rewrite and posix-normalizes the candidate instead, so its `**` still spans
components. **Direction: cyrup is more useful and pi is arguably broken.** Closing "to pi"
here means porting a defect. Flagged as an open question rather than prescribed — see OQ3.
Reasoned from source; **not executed** (no Windows host, and no `.github/` CI in-tree).

### D5 — Windows: fd 10.5.0's path-separator diagnostic (fd-version-dependent, reachable)

`ensure_search_pattern_is_not_a_path` (`fd-find-10.5.0/src/main.rs:169-217`, added for
sharkdp/fd#1873, **absent in 8.7.1/9.0.0/10.2.0/10.3.0**) hard-errors when `--full-path` is
off and the pattern contains `\` **and** `Path::new(pattern).is_dir()`. pi only adds
`--full-path` for `/` (`find.ts:258`), so a Windows caller passing `src\lib` where that
directory exists gets a two-paragraph fd error and pi rejects the call; cyrup silently
matches basenames. This one **is** genuinely fd-version-dependent — it appears only on
fd ≥ 10.5.0 — and is the sharpest instance of the band described in R1.

### D6 — unreachable by construction, recorded so it is not re-derived

`ensure_use_hidden_option_for_leading_dot_pattern` (`main.rs:521-539`) errors on a
dot-leading pattern without `--hidden`; pi always passes `--hidden` (`find.ts:235`), so it
can never fire. Likewise fd's `--exact` / `--fixed-strings` / `--type` / `--exclude` /
`--ignore-contain` arms: pi's argv is exactly
`["--glob","--color=never","--hidden", (--no-require-git)?, "--max-results", N, (--full-path)?, "--", pattern, searchPath]`
(`find.ts:235-267`), and nothing else is reachable.

---

## R4. Traversal, ordering, depth, hidden files, ignore files — enumerated

### The walker configuration matches, knob for knob

fd `WalkerState::build_walker` (`fd-find-10.5.0/src/walk.rs:346-403`) vs cyrup
`LocalFs::walk` (`ops/local/fs.rs:287-345`), under pi's argv:

| fd knob | fd value under pi's argv | cyrup | verdict |
|---|---|---|---|
| `.hidden(config.ignore_hidden)` `:354` | `false` (`--hidden`) | `.hidden(!opts.include_hidden)` = false `fs.rs:299` | ✅ |
| `.ignore(config.read_fdignore)` `:355` | `true` | not set; `WalkBuilder` default `true` | ✅ |
| `.parents(read_parent_ignore && (read_fdignore \|\| read_vcsignore))` `:356` | `true` | `.parents(true)` `fs.rs:315` | ✅ |
| `.git_ignore/.git_global/.git_exclude(read_vcsignore)` `:357-359` | `true`×3 | `fs.rs:300-304` | ✅ |
| `.require_git(require_git_to_read_vcsignore)` `:360` | per `--no-require-git` | `.require_git(opts.require_git)` `fs.rs:309`, set from `inside_git_repo` `find.rs:160,169` | ✅ |
| `.overrides(overrides)` `:361` | empty (`--exclude` unused) | none | ✅ |
| `.follow_links(config.follow_links)` `:362` | `false` | default `false` | ✅ |
| `.same_file_system(one_file_system)` `:364` | `false` | default `false` | ✅ |
| `.max_depth(config.max_depth)` `:365` | `None` | default `None` | ✅ |
| `add_custom_ignore_filename(".fdignore")` `:367-369` | yes | `WalkFlavor::Fd` `ops/mod.rs:271-277`, `fs.rs:322-324` | ✅ |
| global `<config>/fd/ignore` via `add_ignore` `:371-386` | yes | `fs.rs:331-340` + `path::fd_global_ignore_file` | ✅ (Windows residual is the `path.rs:161` task) |
| root entry `if e.depth() == 0 { Continue }` `:480-483` | skip | `if w.path == search_root { continue }` `find.rs:209-211` | ✅ |
| dir trailing separator `print_trailing_slash` `output.rs:49-66` — `file_type().is_dir()`, i.e. **lstat**, so a symlink-to-dir gets none | | `w.is_dir` from `entry.file_type()` `fs.rs:365-370` | ✅ |
| **`.threads(config.threads).build_parallel()` `:402`** | **parallel, default `min(available_parallelism, 64)`** (`cli.rs:755-757`, `:789-800`) | **`builder.build()` — serial `ignore::Walk`** `fs.rs:341` | ❌ **T1/T2** |

### T1 — **fd SORTS its output. cyrup does not. This is the real gap, and the audit does not mention it.**

fd's receiver starts in `ReceiverMode::Buffering` and **sorts before flushing**:

```rust
const MAX_BUFFER_LENGTH: usize = 1000;                              // walk.rs:125
const DEFAULT_MAX_BUFFER_TIME: Duration = Duration::from_millis(100); // walk.rs:127
...
fn stop(&mut self) -> Result<(), ExitCode> {
    if self.mode == ReceiverMode::Buffering {
        self.buffer.sort();                                          // walk.rs:284
        self.stream()?;
    }
```

and `impl Ord for DirEntry { fn cmp(&self, other) { self.path().cmp(other.path()) } }`
(`fd-find-10.5.0/src/dir_entry.rs:132-137`) — a **`Path::cmp`**, component-wise, over the
absolute paths pi handed fd as the root.

Three regimes, exactly:

1. **Walk finishes < 100 ms and total matches ≤ limit** — the receiver never leaves
   `Buffering` (its only other exits are `RecvTimeoutError::Timeout` at `walk.rs:238-240` and
   `buffer.len() > MAX_BUFFER_LENGTH` at `:211`; note **with pi's `DEFAULT_LIMIT = 1000`
   the length trip can never fire**, because `num_results >= max_results` stops at exactly
   1000 while the overflow needs 1001). Output is **fully path-sorted and deterministic.**
   *This is the overwhelmingly common case for an agent tool call.*
2. **Matches > limit, walk still < 100 ms** — `stop()` fires on the cap; the SET is the first
   N produced by a *parallel* walk (nondeterministic run to run), and that set is then sorted.
3. **Walk > 100 ms** — the buffered prefix is sorted and flushed, everything after streams in
   raw parallel order.

**cyrup emits `ignore::Walk` order in all three regimes** — walkdir readdir order, since
`sorter: None` by default (`ignore-0.4.26/src/walk.rs:556`, consumed at `:586-597`) and
`find.rs` no longer sorts.

**This directly refutes the premise the current code was built on.** `find.rs:178-185` says
*"`grep -n sort find.ts` is empty at v0.84.1 … so the sort is dropped rather than moved"*,
and `docs/gap-analysis/04-cyrup-tools.md:454` says *"fd stops after N results in its own
parallel, unordered traversal … and never sorts."* pi does not sort **because fd already
did**. The TOOL-023 fix bounded the walk (correct, and keep it) and deleted the sort
(incorrect for regime 1, which is most calls).

### T2 — the truncated result SET

Under regime 2 fd's chosen N is genuinely nondeterministic — worker batches of `0x100`
(`walk.rs:451-457`) racing into one receiver. **This part cannot be closed by emulation,
because there is nothing deterministic to close to.** The closest deterministic analogue is
"the path-sorted first N", which P1 delivers.

### T3 — `ignore` version skew is a REAL, enumerable exclusion difference

cyrup pins `ignore 0.4.26`; fd 10.5.0 locks `0.4.31`. Diffing `src/gitignore.rs`,
`src/dir.rs`, `src/walk.rs`, `src/pathutil.rs`, `src/overrides.rs` (0.4.26 → 0.4.31/0.4.33),
**one** change alters which files are excluded — `gitconfig_excludes_path()`:

* **0.4.26** (`gitignore.rs:568-584`): `$HOME/.gitconfig` → `$XDG_CONFIG_HOME/git/config` →
  default `$XDG_CONFIG_HOME/git/ignore`.
* **0.4.28+**: adds **`GIT_CONFIG_GLOBAL`** (highest priority, replacing both of the above,
  per git 2.32) and **`GIT_CONFIG_SYSTEM` / `/etc/gitconfig`** (below the two user configs,
  above the default). Bisected by `grep -c GIT_CONFIG_SYSTEM src/gitignore.rs` across
  0.4.20/0.4.23/0.4.25/**0.4.27 = 0**, **0.4.28/0.4.29/0.4.30/0.4.31 = 3**.

**Caller-visible today:** a host with `core.excludesFile` set in `/etc/gitconfig`, or a
session with `GIT_CONFIG_GLOBAL` exported (common in CI and in test harnesses). fd 10.5.0
honours that ignore file; cyrup does not, and returns files pi omits. Everything else in the
0.4.26→0.4.31 delta is a readdir-based stat optimisation (`dir.rs` `add_child_with_entries`,
`collect_ignore_files`), an `is_hidden` → `is_hidden_entry` rename with identical semantics
(`pathutil.rs`), error values gaining `.with_depth(...)`, and additive builder API
(`WalkBuilder::empty`/`from_iter`/`build_matchers`).

One older-direction note for the R1 band: `ignore 0.4.20` (fd 8.7.1) built its global
gitignore with `GitignoreBuilder::new("")` where 0.4.26 uses `current_dir()`, which changes
how a global-gitignore glob anchors. Recorded, not prescribed.

### T4 — broken symlinks (latent, not live)

fd converts an `ignore::Error::WithPath{ NotFound }` whose path `symlink_metadata()` reports
as a symlink into `DirEntry::broken_symlink(path)` and **emits it as a result**
(`walk.rs:487-500`). cyrup discards every walk `Err` (`find.rs:261`). With
`follow_links(false)` on both sides, `ignore` yields symlinks as `Ok`, so this arm is
reachable only on a race (entry unlinked between readdir and stat). **Latent**; recorded so
a future `--follow` or `FsOps` change does not resurrect it unexamined.

### T5 — verified equal, so it is not re-derived

fd swallows every traversal error and still exits `Success` (`walk.rs:227-231`, `:500-505`,
receiver `stop()` → `ExitCode::Success` at `:286-291`), so pi's `if (code !== 0)` guard
(`find.ts:304`) never fires — `find.rs:233-261`'s existing analysis is correct. fd's
search-root gate is `is_existing_directory` = `path.is_dir() && (file_name().is_some() ||
normalize().is_ok())` (`filesystem.rs:38-42`), matching cyrup's `metadata(...).is_dir` gate
at `find.rs:135-146`. `--color=never` ⇒ `ls_colors: None` ⇒ `print_entry_uncolorized`, and on
a **pipe** that writes raw bytes with no terminal sanitization (`output.rs:167-183`,
`sanitize.rs:47-53`), which pi's `readline` then decodes lossily — so cyrup's lossy *output*
is right even though its lossy *matching* (D3) is not. `strip_cwd_prefix` is false because
pi passes an explicit search path (`cli.rs:765-768` gates on `no_search_paths()`), so fd
prints absolute paths and pi relativizes (`find.ts:321-326`) — cyrup's split of
match-on-absolute / print-relative is correct.

---

## R5. Additional divergences this research turned up

### F1 — `--max-results 0` means **UNLIMITED** in fd, not "no results"

```rust
pub fn max_results(&self) -> Option<usize> {
    self.max_results.filter(|&m| m > 0)          // cli.rs:759-763
        .or_else(|| self.max_one_result.then_some(1))
}
```

`limit: 0` ⇒ pi sends `--max-results 0` ⇒ fd's filter drops it ⇒ **fd returns the entire
tree**, and pi then evaluates `relativized.length >= 0` as true (`find.ts:328`) and appends
`[0 results limit reached. Use limit=0 for more, or refine pattern]`. cyrup folds 0 through
`jsnum::to_count` and returns `"No files found matching pattern"` (`find.rs:154`, `:186-188`,
`:272-279`). Maximal divergence — whole tree vs empty.

### F2 — a negative or fractional `limit` makes pi **reject the call**; cyrup returns success

`max_results: Option<usize>` (`cli.rs:575`) — clap's `usize` value parser. pi stringifies
without validating (`find.ts:252` `String(effectiveLimit)`), so `limit: -1` ⇒ `"-1"` and
`limit: 2.5` ⇒ `"2.5"` both fail to parse: fd exits **2** with empty stdout, and
`find.ts:304-309` rejects with `stderr.trim()`. cyrup returns `"No files found matching
pattern"` for `-1` and two rows for `2.5`.

**Two in-tree comments assert the opposite and must be corrected with the fix:**
`find.rs:149-153` (*"A non-positive count yields no rows from that cap"*) and
`jsnum.rs:33-35` (*"`fd --max-results` in find.ts:241"* listed as a place where a negative
value behaves as zero). The existing test
`find_accepts_float_and_negative_limit` (`src/tests/tools.rs:2596-2648`) **pins the wrong
behaviour** at `:2647` and must be rewritten, not merely extended.

### F3 — audit claim (c) overstates pi's `FindOperations.glob` seam

The audit says the seam *"lets a remote backend do the glob remotely and return paths"*
while cyrup's `FsOps::walk` *"forces enumeration-then-match, so a remote backend transfers
the whole listing."* Two corrections:

* **pi's only in-tree implementation is enumerate-then-match as well.** `createGondolinFindOps`
  (`examples/extensions/gondolin/index.ts:183-203`) calls `walkGuestFiles` over the entire
  guest tree and applies `matchesToolGlob` per entry — and that helper (`:172-181`) uses
  **Node's `path.posix.matchesGlob`**, a *third* dialect that is neither fd's nor globset's,
  hard-skips `.git`/`node_modules` (`:151`) and honours **no** `.gitignore` at all. Taking
  the seam does not preserve fd semantics; it discards them.
* **cyrup has no remote `FsOps` today.** The only implementors are `LocalFs`
  (`ops/local/fs.rs:131`) and the two isolation decorators `TraversalFs`
  (`isolation/traversal.rs:88`) and `ProtectedFs` (`isolation/protected.rs:101`), both of
  which delegate `walk` straight through. Claim (c) is **latent on both sides**.

What survives, and is real: `FsOps::walk` streams **every** walked entry across the seam
while only matches are kept, whereas `FindOperations.glob` would ship only matches. That is
a bandwidth property of the seam shape, not a behaviour difference, and it is unobservable
until a remote backend exists.

### F4 — audit claim (b): pi's fd-specific error strings

`fd is not available and could not be downloaded` (`find.ts:231`), `Failed to run fd: …`
(`:294`), `fd exited with code N` (`:305`) genuinely cannot occur in cyrup — there is no
child. This is correct as filed and it is the one part of the divergence that is **not
closable by any amount of emulation**, because reproducing it would require reproducing the
failure mode (a missing binary) that cyrup structurally does not have. It is also the part
with the least caller value: an agent seeing `fd is not available` learns nothing actionable
about the repository.

---

## R6. Prescription — CLOSE

Everything below is per-difference. **No compatibility layer and no vendored matcher is
warranted** — R2 shows the matcher is already shared code.

### P1 — restore fd's ordering, at the walk, gated per tool

`crates/cyrup-tools/src/ops/mod.rs`, symbol `WalkOpts` (`:302-306`): add

```rust
/// Emit entries in `Path::cmp` order. fd's receiver buffers its results and
/// `self.buffer.sort()`s them before printing (fd 10.5.0 `src/walk.rs:284`), with
/// `impl Ord for DirEntry` = `self.path().cmp(other.path())` (`src/dir_entry.rs:132-137`);
/// ripgrep has no equivalent, so this is `find`-only and must stay a per-caller knob.
pub sorted: bool,
```

`crates/cyrup-tools/src/ops/local/fs.rs`, symbol `LocalFs::walk`: when `opts.sorted`, call

```rust
builder.sort_by_file_path(|a, b| a.cmp(b));
```

**Verified API** — `ignore-0.4.26/src/walk.rs:900-906`:
`pub fn sort_by_file_path<F>(&mut self, cmp: F) -> &mut WalkBuilder where F: Fn(&Path, &Path) -> Ordering + Send + Sync + 'static`,
honoured by `build()` at `:586-597` (its doc note *"not used in the parallel iterator"* is
irrelevant — cyrup uses the serial `Walk`).

**Why the walk and not `results.sort()` — two reasons, both load-bearing:**

1. **Per-directory `Path::cmp` on a pre-order DFS is exactly the global `Path::cmp` order.**
   For any two emitted paths, either one is an ancestor of the other — pre-order emits the
   ancestor first, and `Path::cmp` ranks the shorter component sequence first — or they
   diverge at a lowest common ancestor directory, where the sorter orders the two differing
   sibling components and the DFS emits the smaller sibling's whole subtree first, which is
   what `Path::cmp` decides on. So the matched subsequence is `Path::cmp`-sorted, i.e.
   byte-identical to fd's regime-1 output.
2. **It makes the truncated set deterministic** (T2) instead of readdir-dependent, and it
   sidesteps a trap: `results` holds *relative posix strings with a trailing `/` on
   directories* (`find.rs:229`), and **sorting those as strings is not `Path::cmp`** —
   `"a/b"` vs `"a.txt"` compares `'/'`(0x2F) against `'.'`(0x2E) and gets the **opposite**
   answer to `Path::cmp`, which compares the components `"a"` and `"a.txt"`. A
   `results.sort()` would therefore be a *new* divergence wearing a fix's clothes.

`crates/cyrup-tools/src/tools/find.rs:165-175` sets `sorted: true`;
`crates/cyrup-tools/src/tools/grep.rs:610-622` sets `sorted: false` with a comment saying
ripgrep's own printer was not audited (OQ4). Both are struct literals naming every field, so
the new field forces both call sites to state a choice — that is the point.

**Keep the bounded walk.** TOOL-023's early break was right; only the sort deletion was wrong.

### P2 — port fd's smart-case predicate verbatim (D1), and pin `literal_separator` (D2)

`crates/cyrup-tools/src/tools/globmatch.rs`, symbol `PatternMatcher::build`. fd's shape is
build-then-inspect-then-recompile, and cyrup must adopt it:

1. `GlobBuilder::new(&effective).literal_separator(true).build()?` — **unconditional
   `true`**, matching `fd main.rs:220` exactly (D2).
2. Take `glob.regex()` — **verified**: `pub fn regex(&self) -> &str`,
   `globset-0.4.18/src/glob.rs:326-328`.
3. Run fd's predicate over that string, replacing `pattern_has_uppercase_char`
   (`globmatch.rs:38-40`) with a 1:1 port of `fd-find-10.5.0/src/regex_helper.rs:5-37`.
4. Rebuild with `.case_insensitive(!has_upper)` and compile.

**Verified APIs at the pinned version** (`regex-syntax 0.8.11`, already resolved in
`/home/user/cyrup/Cargo.lock:5780-5783` as a transitive dep — needs a direct edge in
`crates/cyrup-tools/Cargo.toml`):

| symbol | file:line |
|---|---|
| `ParserBuilder` | `regex-syntax-0.8.11/src/parser.rs:25` |
| `ParserBuilder::utf8(&mut self, yes: bool)` | `src/parser.rs:106` |
| `HirKind::{Literal, Class, Look, Repetition, Capture, Concat, Alternation}` | `src/hir/mod.rs:717-751` |
| `Literal(pub Box<[u8]>)` | `src/hir/mod.rs:801` |
| `Class::{Unicode(ClassUnicode), Bytes(ClassBytes)}` | `src/hir/mod.rs:830-836` |
| `ClassUnicodeRange::start(&self) -> char` | `src/hir/mod.rs:1321` |
| `ClassBytesRange::{start,end}(&self) -> u8` | `src/hir/mod.rs:1580`, `:1588` |

Two builds per call is the cost; a `find` call builds one glob, so it is noise. Delete the
false equivalence paragraph at `globmatch.rs:29-35` — do not soften it.

### P3 — match on bytes, not on a lossy string (D3)

`crates/cyrup-tools/src/tools/globmatch.rs`, symbol `PatternMatcher::is_match`: take
`&[u8]` and go through globset's byte API instead of `&str`.

**Verified APIs** — `Candidate::from_bytes<P: AsRef<[u8]> + ?Sized>(path: &'a P) -> Candidate<'a>`
(`globset-0.4.18/src/lib.rs:629-631`) and
`GlobMatcher::is_match_candidate(&self, path: &Candidate<'_>) -> bool`
(`globset-0.4.18/src/glob.rs:147-149`); `Candidate::from_cow` applies `normalize_path`
(`src/pathutil.rs:67-85`), which is a no-op on unix and does the `\` ⇒ `/` rewrite on
Windows — i.e. `to_posix` becomes unnecessary on the *matching* path and stays only for
*output*.

Caller in `crates/cyrup-tools/src/tools/find.rs:212-231` supplies the bytes fd supplies,
platform for platform, mirroring `fd filesystem.rs:103-116`:

* unix — `w.path.as_os_str().as_bytes()` and `file_name().as_bytes()` (raw, lossless);
* windows — `to_string_lossy().into_owned().into_bytes()` (fd is lossy there too, so this is
  parity rather than a compromise).

### P4 — bump `ignore` to `0.4.31` (T3)

Root `Cargo.toml` `[workspace.dependencies]`. 0.4.31 is fd 10.5.0's own lock, and the
0.4.26→0.4.31 delta was read in full above: one behavioural change (`GIT_CONFIG_GLOBAL` /
`GIT_CONFIG_SYSTEM` / `/etc/gitconfig` in `gitconfig_excludes_path`), the rest optimisation
and additive API. `WalkBuilder::sort_by_file_path` (P1) exists unchanged in 0.4.31.
Note `RgGlob`'s doc comments (`globmatch.rs:92-110`) cite `ignore-0.4.26 gitignore.rs` and
`overrides.rs` line numbers; `overrides.rs` is **byte-identical** at 0.4.31 (diff empty) and
`gitignore.rs`'s `add_line` region is unchanged, but the citations should be re-anchored in
the same commit.

### P5 — `limit` semantics (F1, F2)

`crates/cyrup-tools/src/tools/find.rs:148-154`:

* `limit == 0` (after `to_count`) ⇒ **unlimited**, and `limit_reached` is still true, so the
  notice reads `0 results limit reached. Use limit=0 for more, or refine pattern` — pi's
  literal output. Mirrors `fd cli.rs:759-763`.
* a value that clap's `usize` parser would reject — negative after `to_integer`, or
  non-integral before truncation — ⇒ **reject the call**, matching pi's
  `settle(() => reject(new Error(stderr.trim())))` at `find.ts:304-309`. The check must run
  on the raw `f64` (`input.limit`), because `to_count` has already erased both signals by the
  time it returns. Exact clap error text is OQ1.

Correct the two comments that assert the current behaviour is pi's: `find.rs:149-153` and
`jsnum.rs:33-35` (the `find.ts:241` clause only — read/ls clauses there are unaffected).

### P6 — do NOT shell out to `fd`, and say why in the marker

The third option the task names — spawn fd when present — is the one to argue *against*, on
four grounds, each with a citation:

1. It **re-imports the unboundedness** the audit objects to. R1: `getToolPath`
   (`tools-manager.ts:82-101`) takes any `fd`/`fdfind` on `PATH`, so behaviour would become a
   function of the host distro. D5 is a live example of a difference that exists **only** on
   fd ≥ 10.5.0.
2. It **bypasses the isolation seam**. `TraversalFs::walk` confines the root
   (`isolation/traversal.rs:133-142`) and `ProtectedFs` wraps the same seam
   (`isolation/protected.rs:150-156`); a spawned fd walks the real filesystem with the
   session's ambient authority and no decorator applies.
3. It **needs authority `find` does not hold**. Direct-argv exec is a capability-scoped grant
   through `cyrup-session-svc::host_services::exec` (documented on `ArgvSpec`,
   `ops/mod.rs`); `find` receives only `Arc<dyn FsOps>`.
4. `ensureTool`'s fallback is a **live GitHub download** (`tools-manager.ts:240-315`) — a
   network fetch and an executable write inside a tool call. `docs/gap-analysis/04-cyrup-tools.md:1078`
   already records that this *"has no cyrup analog by construction."*

Rewrite the `[CYRUP-DELTA]` at `find.rs:1-2` into a **non-marker**: after P1–P5 the
mechanism is still in-process, but the residual is R7's list, not "unbounded". Do not leave a
softened `CYRUP-DELTA` behind — that is what produced this backlog.

### P7 — correct the record

`docs/gap-analysis/04-cyrup-tools.md` **TOOL-023** (`:448-460`): its *"fd … never sorts"*
premise is refuted by `fd walk.rs:284` / `dir_entry.rs:132-137`. Its **Fix** paragraph forced
a false either/or ("break early **and drop the sort**" vs "sort **and** keep the full walk");
fd does **both** — bounded traversal *and* sorted output. Annotate the row with that, and
with the fact that the fd-not-vendored caveat at `:1084` is now discharged for fd.

---

## R7. Residual after P1–P6 — what David is being asked to accept

1. **Regime 2/3 result SETS** (T2). fd's choice of *which* N under `--max-results`, and its
   ordering past 100 ms, are nondeterministic upstream. Nothing to converge to.
2. **fd's three error strings** (F4). Structurally unreachable without a child process.
3. **fd-version band** (R1, D5). fd ≥ 10.5.0's path-separator diagnostic exists only on newer
   fd; a cyrup that reproduces it diverges from a host running fd 9.0.0, and vice versa.
   Any single choice is wrong for some hosts. Recommend targeting fd 10.5.0 (newest, and the
   version cyrup's existing comments already cite) and **naming it in the module doc** so the
   band is declared rather than implied.
4. **`globset` 0.4.18 vs 0.4.19** (R2). Proven identical today; a future 0.4.x could diverge.
   Mitigation is `Cargo.lock` plus the D-series tests, not a code change.
5. **Windows `**` degradation** (D4) — pending OQ3.
6. **`FindOperations.glob` seam shape** (F3) — latent until a remote `FsOps` exists.

That is the complete list. It is six items, five of which are bounded and one of which
(item 1) is *undefined upstream*. It is not "an entire external tool's semantics".

---

## R8. Tests

Guards that are **RED before the change and GREEN after**. `globmatch.rs`'s existing
`#[cfg(test)] mod tests` (`:234-236`) and `src/tests/tools.rs` both already carry the
clippy allow headers these need.

1. `fd_smart_case_reads_the_emitted_regex_not_the_glob` — **RED for P2.** In `globmatch.rs`:
   `PatternMatcher::build("[a-ÿ]bc.ts")` must **not** match basename `"Abc.ts"`, and must
   match `"abc.ts"`. Fails today because `pattern.chars().any(char::is_uppercase)`
   (`globmatch.rs:39`) sees no uppercase and compiles the glob case-insensitively. Pair it
   with an already-green control (`"*.TS"` stays case-sensitive, `"*.ts"` stays insensitive)
   so a regression in the common path is caught too.
2. `find_results_are_path_sorted_like_fd` — **RED for P1.** In `src/tests/tools.rs`: build a
   tree that makes readdir order differ from `Path::cmp` order and where **string** order
   differs from `Path::cmp` order — the `a/b` vs `a.txt` pair from P1 is the discriminating
   case: create `a.txt`, `a/b.txt`, `ab.txt`. Assert the rows are exactly
   `a/`, `a/b.txt`, `a.txt`, `ab.txt`. Today's output is readdir order, so this is
   nondeterministically red; **make it deterministically red by asserting the full sequence**,
   and note in the test that a naive `results.sort()` on the joined strings would put
   `a.txt` before `a/b.txt` and therefore also fail — the test discriminates the right fix
   from the wrong one.
3. `find_matches_non_utf8_filenames_by_byte` — **RED for P3, unix-only**
   (`#[cfg(unix)]`, `std::os::unix::ffi::OsStrExt::from_bytes`): create `caf\xE9.ts`, assert
   `pattern: "caf?.ts"` returns one row. Today the lossy `to_posix` makes it three bytes and
   the row is absent.
4. `find_limit_zero_is_unlimited_like_fd` — **RED for P5.** Ten files, `limit: 0`; assert all
   ten rows plus the `0 results limit reached. Use limit=0 for more, or refine pattern`
   notice. Today: `"No files found matching pattern"`.
5. `find_rejects_a_limit_fd_would_reject` — **RED for P5.** `limit: -1` and `limit: 2.5`
   must both return `Err`. **This requires editing `find_accepts_float_and_negative_limit`
   (`src/tests/tools.rs:2596-2648`), which asserts the opposite at `:2647`** — that existing
   assertion is wrong, not merely incomplete, and its `2.0` half stays valid.
6. `find_honors_git_config_system_excludes_file` — **RED for P4.** Point `GIT_CONFIG_GLOBAL`
   at a config whose `core.excludesFile` names an ignore file, and assert the excluded file
   is absent. **Caveat stated rather than worked around:** this needs process-environment
   mutation, which is `unsafe` under edition 2024 and races every other test in the binary;
   the honest form is an integration test in `crates/cyrup-tools/tests/` with a scrubbed
   child (the `bash_env_scrub.rs` pattern), not a unit test. If that is judged too heavy,
   the fallback guard is a `Cargo.toml` assertion that `ignore` is `>= 0.4.28` — weaker, and
   it must be labelled as such.

**What cannot be verified here, stated plainly:**

* **Nothing was compiled or run.** `cargo` is barred for this pass (ten sibling agents, 7.7 G
  disk). Every API above was read at its pinned version in `~/.cargo/registry` or in the
  tarballs under `./tmp/fdsrc/`; that is the strongest check available without a compiler.
* **No fd binary was executed.** `fd` is not installed on this host and neither is `fd-find`.
  All fd claims are source claims. Regimes 1–3 (T1) in particular are read off the receiver
  state machine, not timed.
* **D4 and D5 are Windows-only** and there is no Windows host and no `.github/` in-tree.
* **ripgrep was not read.** P1 deliberately leaves `grep` unsorted because rg's printer was
  not audited — see OQ4. Do not generalise T1 to `grep` on this pass.

---

## Open questions for David

1. **F2's exact rejection text.** pi rejects with clap's own stderr, which is
   version-and-locale-shaped (`error: invalid value '-1' for '--max-results <count>': invalid
   digit found in string` on fd 10.5.0 / clap 4). Reproducing it byte-for-byte pins cyrup to a
   clap release. Options: (a) reproduce the fd 10.5.0 text verbatim, (b) emit a stable cyrup
   message and accept the string divergence while matching the *shape* (an `Err`, not a
   result). **Recommend (b)** — the failure classification is what a caller acts on. Needs
   your call because it is the only place in P1–P6 that knowingly leaves a string different.
2. **Which fd do we target?** R1 shows pi has no pinned fd. Recommend declaring **fd 10.5.0**
   in `find.rs`'s module doc as the reference implementation, and treating divergence from
   older hosts as declared rather than as a gap. Alternative: target the *oldest* commonly
   shipped fd (8.7.0, Debian bookworm) for the widest agreement. Cannot be decided from the
   code.
3. **Windows recursive star (D4).** pi's `` `[/\\]` `` rewrite silently demotes fd's `**` to
   a pair of non-separator-crossing stars. Closing "to pi" means porting that defect; leaving cyrup as-is
   means Windows `find` is *better* than pi and therefore divergent. Which?
4. **Does T1 transfer to `grep`?** fd buffers-and-sorts; **ripgrep was not read this pass**
   and its printer may or may not have an equivalent. The `grep.rs:1` sibling task must
   answer this against rg's own source before `sorted:` is set either way there. Flagging so
   the two are not assumed symmetric — TOOL-023 and TOOL-033 were landed as a pair on the
   assumption that they were.
5. **`ignore` bump blast radius.** P4 moves the whole workspace from 0.4.26 to 0.4.31, which
   also affects `grep`. The delta was read and is behaviour-neutral except for the
   git-config discovery, but the bump was not compiled. Confirm it lands as its own commit
   with the full suite green before P1–P3.

## The accept case, argued for David

The honest case for accepting: **fd's output under the result cap is nondeterministic**
(T2), so "exact parity with fd" is not a well-formed target for the regime the audit is most
worried about; **pi's own fd is unpinned** (R1), so there is no single upstream to match;
and one part of the divergence — the three fd-process error strings (F4) — is structurally
unreachable and carries no caller value. On top of that, `find` is a low-stakes tool: its
divergences over-include rather than under-include, and an agent that gets an extra file back
loses nothing.

**That case does not survive contact with T1.** The single largest observable difference is
not exotic at all — **fd's output is path-sorted for essentially every real call** (regime 1:
under 100 ms, under 1000 matches), and cyrup's is readdir order. An agent reading a `find`
result is reading an *ordered list*; two runs of cyrup on the same tree on different
filesystems can order it differently, and neither matches pi. That is a one-field, one-line
fix (P1) whose absence traces to a documented premise — *"fd … never sorts"* — that fd's own
source refutes. Accepting the divergence would mean formally authorising a behaviour that was
adopted by mistake.

**Recommend CLOSE**, in this order: **P1 (ordering) and P5 (`limit` semantics) first** —
they are the two with everyday caller impact and the smallest diffs; then **P4 (`ignore`
bump)** as its own commit; then **P2/P3 (smart case, byte matching)**, which are narrow and
exotic but cheap now that the APIs are pinned. If the effort must be cut, cut P2/P3 —
**never P1** — and record the cut as an explicit acceptance of items D1 and D3 with the
reason, not as a marker.
