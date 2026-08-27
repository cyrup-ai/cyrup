---
title: Grepping a binary file by explicit path returns no matches in cyrup; pi returns the matching lines
priority: MEDIUM
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: done
updated: 2026-08-27
---

# Grepping a binary file by explicit path returns no matches in cyrup; pi returns the matching lines

## Core objective

When `grep`'s `path` argument names a **regular file**, that file must be searched and its matching
lines returned — even if it contains NUL bytes. Suppression by binary detection is a *filter*, and
ripgrep's rule is that a path the user named is never filtered out. Only files discovered by
**directory traversal** keep the "stop at the first NUL" behaviour.

The whole gap lives in **one builder call**:
[grep.rs:120-123](../../../crates/cyrup-tools/src/tools/grep.rs) hard-codes
`BinaryDetection::quit(b'\x00')` for every candidate, and `search_one` is shared by both the
explicit-file branch ([grep.rs:342-360](../../../crates/cyrup-tools/src/tools/grep.rs)) and the walk
branch ([grep.rs:367-433](../../../crates/cyrup-tools/src/tools/grep.rs)) — so the explicit path
inherits the walk's suppression. The fix is to make the mode a **per-candidate** input, exactly as
ripgrep makes it, and hand the explicit branch the mode that reproduces what pi observes.

---

## ripgrep's actual rule — verified against source and against the binary

### 1. Two modes, chosen per haystack

ripgrep stores **two** `grep::searcher::BinaryDetection` values and hands both to the search worker,
which picks one per file:

```rust
// ripgrep 14.1.0 crates/core/flags/hiargs.rs:1124-1157
/// ripgrep actually uses two different binary detection heuristics depending
/// on whether a file is explicitly being searched (e.g., via a CLI argument)
/// or implicitly searched (e.g., via directory traversal). In general, the
/// former can never use a heuristic that lets it "quit" seaching before
/// either getting EOF or finding a match. (Because doing otherwise would be
/// considered a filter, and ripgrep follows the rule that an explicitly given
/// file is always searched.)
struct BinaryDetection {
    explicit: grep::searcher::BinaryDetection,
    implicit: grep::searcher::BinaryDetection,
}

fn from_low_args(_: &State, low: &LowArgs) -> BinaryDetection {
    let none = matches!(low.binary, BinaryMode::AsText) || low.null_data;
    let convert = matches!(low.binary, BinaryMode::SearchAndSuppress);
    let explicit = if none {
        grep::searcher::BinaryDetection::none()
    } else {
        grep::searcher::BinaryDetection::convert(b'\x00')
    };
    let implicit = if none {
        grep::searcher::BinaryDetection::none()
    } else if convert {
        grep::searcher::BinaryDetection::convert(b'\x00')
    } else {
        grep::searcher::BinaryDetection::quit(b'\x00')
    };
    BinaryDetection { explicit, implicit }
}
```

Both are wired into the worker at `hiargs.rs:696-697`
(`.binary_detection_explicit(...)` / `.binary_detection_implicit(...)`), and the worker selects on
`Haystack::is_explicit()`, which is *depth 0 and not a directory* — i.e. a path handed to `rg` on
the command line. A file reached one level down inside a named directory is **implicit**.

pi passes no `--text`, no `--binary`, no `--null-data`
([grep.ts:220-224](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts)), so `none` and
`convert` above are both `false`: **explicit = `convert(b'\x00')`, implicit = `quit(b'\x00')`.**

### 2. The trap: `convert` is INERT on the path ripgrep actually takes for an explicit file

`convert` is not "quit, but nicer". Its documented contract in `grep-searcher-0.1.16`
(`src/searcher/mod.rs:83-97`) is:

> Binary detection is performed by looking for the given byte, and **replacing it with the line
> terminator** configured on the searcher. […] When searching is performed with the entire contents
> **mapped into memory, then this setting has no effect and is ignored**.

`line_buffer.rs:448-460` confirms the reader path: every NUL is rewritten to `\n` in place, and the
search continues to EOF. **Rewriting NULs to `\n` renumbers every subsequent line.**

And ripgrep memory-maps precisely the explicit-single-file case (`hiargs.rs:221-248`):

```rust
match low.mmap {
    MmapMode::Auto => {
        if paths.paths.len() <= 10 && paths.paths.iter().all(|p| p.is_file()) {
            // If we're only searching a few paths and all of them
            // are files, then memory maps are probably faster.
            maybe
        } else {
            never
        }
    }
    MmapMode::AlwaysTryMmap => maybe,
    MmapMode::Never => never,
}
```

pi always passes exactly one path (`args.push("--", pattern, searchPath)`, grep.ts:224). So:

| pi's `path` | rg paths | mmap? | detection | effect |
| --- | --- | --- | --- | --- |
| a **file** | 1, `is_file()` | **yes** (`search_slice`) | `convert` | **ignored** — bytes untouched, raw `\n` numbering; a `binary_offset` is reported on the `end` event and nothing else changes |
| a **directory** | 1, not a file | **no** (`search_reader`) | `quit` | search ends at the first NUL |

### 3. Measured — ripgrep 14.1.0

Fixture `bin.dat` = `hello NEEDLE\n` + `NUL SOH STX` + `binary NEEDLE\n` + `tail NEEDLE\n`
(the NUL sits at byte offset 13).

Explicit path, exactly pi's argv:

```
$ rg --json --line-number --color=never --hidden -- NEEDLE bin.dat
{"type":"match","data":{"lines":{"text":"hello NEEDLE\n"},"line_number":1,"absolute_offset":0,...}}
{"type":"match","data":{"lines":{"text":"\u0000\u0001\u0002binary NEEDLE\n"},"line_number":2,"absolute_offset":13,...}}
{"type":"match","data":{"lines":{"text":"tail NEEDLE\n"},"line_number":3,"absolute_offset":30,...}}
{"type":"end","data":{"path":{"text":"bin.dat"},"binary_offset":13,...}}
```

Three matches. Line numbers 1/2/3 are the file's **raw** `\n` numbering, and the NUL bytes survive
verbatim in `lines.text`. (`rg` without `--json` prints only
`binary file matches (found "\0" byte around offset 13)` — that is the *printer's* summary line, not
the search result; pi never sees it, because pi reads the JSON `match` events.)

The same file reached by traversal emits nothing at all — not even the pre-NUL line 1:

```
$ rg --json --line-number --color=never --hidden -- NEEDLE sub/     # sub/bin.dat is identical
{"type":"summary","data":{"stats":{"matches":0,"searches_with_match":0,...}}}
$ echo $?
1
```

Force the reader path and `convert` becomes visible — and wrong for our purposes:

```
$ rg --no-mmap --json --line-number --color=never --hidden -- NEEDLE bin.dat
... "lines":{"text":"hello NEEDLE\n"},"line_number":1
... "lines":{"text":"\u0001\u0002binary NEEDLE\n"},"line_number":3    <-- the NUL became a line break
... "lines":{"text":"tail NEEDLE\n"},"line_number":4
```

Lines **1, 3, 4** instead of 1, 2, 3, with the NUL eaten. Whereas `none()` over the same reader is
byte-identical to the mmap run:

```
$ rg --no-mmap --text --json --line-number --color=never -- NEEDLE bin.dat
... "lines":{"text":"hello NEEDLE\n"},"line_number":1
... "lines":{"text":"\u0000\u0001\u0002binary NEEDLE\n"},"line_number":2
... "lines":{"text":"tail NEEDLE\n"},"line_number":3
```

Only `binary_offset` differs (`null` vs `13`), and pi reads no field other than `data.path.text`,
`data.line_number` and `data.lines.text` (grep.ts:287-291).

### 4. Therefore: the required mode for cyrup's explicit branch is `none()`, not `convert()`

cyrup never memory-maps — every candidate goes through `FsOps::read_stream` +
`Searcher::search_reader` ([grep.rs:93-95](../../../crates/cyrup-tools/src/tools/grep.rs),
[grep.rs:132](../../../crates/cyrup-tools/src/tools/grep.rs)), which is the branch where `convert`
*does* take effect. Copying ripgrep's `explicit` **mode name** would therefore copy the one
configuration ripgrep never actually executes for this case, and would ship the renumbered 1/3/4
output. Copying ripgrep's **observable behaviour** means `none()`.

> The original *Parity action* on this task said `convert(b'\x00')`. That is corrected here: it
> reproduces the flag, not the behaviour.

This also dissolves the objection recorded in the existing `[CYRUP-DELTA]` comment at
[grep.rs:104-108](../../../crates/cyrup-tools/src/tools/grep.rs) — "`convert` renumbers lines at
every NUL, while the output blocks below are cut from a separate raw re-read of the file that splits
on `\n` only". Under `none()` the searcher's numbering **is** raw-`\n` numbering, so it agrees with
the re-read at [grep.rs:162-177](../../../crates/cyrup-tools/src/tools/grep.rs) by construction.
Nothing downstream of the searcher needs to change.

---

## What pi does

[pi grep.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts) (vendored pi 0.84.3)
stats the path once and branches only on how it *formats* results, never on whether it searches:

```ts
// grep.ts:185-191
let isDirectory: boolean;
try {
    isDirectory = await ops.isDirectory(searchPath);
} catch {
    settle(() => reject(new Error(`Path not found: ${searchPath}`)));
    return;
}
```

```ts
// grep.ts:220-226
const args: string[] = ["--json", "--line-number", "--color=never", "--hidden"];
if (ignoreCase) args.push("--ignore-case");
if (literal) args.push("--fixed-strings");
if (glob) args.push("--glob", glob);
args.push("--", pattern, searchPath);

const child = spawn(rgPath, args, { stdio: ["ignore", "pipe", "pipe"] });
```

`isDirectory === false` makes `formatPath` return `path.basename(filePath)` (grep.ts:195-203) —
which cyrup's explicit branch already matches via `search_root.file_name()`
([grep.rs:346-349](../../../crates/cyrup-tools/src/tools/grep.rs)).

For the fixture above, pi's rendered result at `context: 0` is three rows, the middle one carrying
the raw NUL and control bytes, because `match.lineText !== undefined` sends it down the direct path
at grep.ts:323-331.

A **non-UTF-8** matched line behaves differently, and cyrup already models it. `rg` serialises
`lines.bytes` (base64) instead of `lines.text` for such a line — measured on
`ok NEEDLE\n` + `0xFF 0xFE` + ` NEEDLE bad\n` + `NUL` + `x NEEDLE\n`:

```
... "lines":{"text":"ok NEEDLE\n"},"line_number":1
... "lines":{"bytes":"//4gTkVFRExFIGJhZAo="},"line_number":2
... "lines":{"text":"\u0000x NEEDLE\n"},"line_number":3
```

so `match.lineText` is `undefined` for line 2 and pi falls to `formatBlock` → `getFileLines`
(grep.ts:206-218), whose `defaultGrepOperations.readFile` is `fsReadFile(p, "utf-8")`
(grep.ts:63-66) — a lossy decode producing U+FFFD. cyrup's `takes_block` closure
([grep.rs:149](../../../crates/cyrup-tools/src/tools/grep.rs)) is exactly
`context > 0 || std::str::from_utf8(raw).is_err()`, and the re-read at
[grep.rs:168-172](../../../crates/cyrup-tools/src/tools/grep.rs) uses `String::from_utf8_lossy`.
Both already line up — again, **only once the numbering agrees**, which is what `none()` delivers.

---

## What cyrup-tools does today

[grep.rs:119-123](../../../crates/cyrup-tools/src/tools/grep.rs):

```rust
        let matches: Vec<(u64, Vec<u8>)> = tokio::task::spawn_blocking(move || {
            let mut searcher: Searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(BinaryDetection::quit(b'\x00'))
                .build();
```

One construction, no per-candidate input, called from both branches. Running the tool with
`{"pattern":"NEEDLE","path":"bin.dat"}` yields `No matches found`.

**User-visible impact.** A user who points `grep` directly at a file containing NUL bytes — a
minified bundle with an embedded binary payload, a `.pack`/`.pyc`/`.class`, a log with control
bytes, a UTF-16 file without a BOM — is told there are no matches although the text is present. The
answer is *wrong*, not merely truncated, and nothing in the output signals that a filter fired.

---

## Required change

One file: [crates/cyrup-tools/src/tools/grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs).
`BinaryDetection` is already imported at
[grep.rs:13](../../../crates/cyrup-tools/src/tools/grep.rs); no import, dependency or schema change.

### 1. `search_one` takes the mode per candidate

Current ([grep.rs:72-83](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
    #[allow(clippy::too_many_arguments)]
    async fn search_one(
        &self,
        file: &std::path::Path,
        rel: &str,
        matcher: &grep_regex::RegexMatcher,
        context: usize,
        limit: usize,
        count: &mut usize,
        out: &mut Vec<String>,
        any_line_truncated: &mut bool,
    ) -> Result<(), ToolError> {
```

Replacement — one new parameter, immediately after `matcher` (the existing `#[allow]` already covers
the arity):

```rust
    #[allow(clippy::too_many_arguments)]
    async fn search_one(
        &self,
        file: &std::path::Path,
        rel: &str,
        matcher: &grep_regex::RegexMatcher,
        // ripgrep chooses binary detection PER HAYSTACK, not per invocation: a path the user named
        // gets one mode, a path found by traversal gets another (`hiargs.rs:1124-1157`, handed to
        // the worker as `binary_detection_explicit` / `binary_detection_implicit` at
        // `hiargs.rs:696-697`). The caller classifies the candidate; this function just uses what
        // it was given.
        binary: BinaryDetection,
        context: usize,
        limit: usize,
        count: &mut usize,
        out: &mut Vec<String>,
        any_line_truncated: &mut bool,
    ) -> Result<(), ToolError> {
```

### 2. Replace the rationale comment and use the passed-in mode

Current ([grep.rs:97-123](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
        // Binary detection. Pi spawns real ripgrep with no `--text`/`-a` (grep.ts:220-224), so
        // ripgrep's default applies: files reached by traversal are searched with
        // `BinaryDetection::quit(b'\x00')` — a NUL ends that file as if EOF, so a binary file
        // contributes no `--json` match lines. `grep-searcher`'s own default is
        // `BinaryDetection::None` ("Data reported by the searcher may contain arbitrary bytes"),
        // which would dump raw bytes of PNG/wasm/font/sqlite hits into the model-facing result.
        //
        // [CYRUP-DELTA] ripgrep uses `convert(b'\x00')` instead of `quit` for a path named
        // EXPLICITLY on the command line (Pi's `path` argument pointing at a single file). cyrup
        // keeps `quit` there too: `convert` renumbers lines at every NUL, while the output blocks
        // below are cut from a separate raw re-read of the file that splits on `\n` only, so the
        // two numberings would disagree and emit the wrong lines.
        //
        // The searcher is built per file rather than hoisted because `search_reader` is a
        // BLOCKING API driven from `spawn_blocking` (see [`FsOps::read_stream`]), so it and the
        // reader must be owned by the blocking task.
        let matcher_owned = matcher.clone();
        // `MatchSink` counts against the REMAINING budget; the caller's global `count` is advanced
        // by however many this file contributed. Pi's cap is global too — its line handler ignores
        // every event once `matchCount >= effectiveLimit` (grep.ts:278) — so a file can only ever
        // fill the gap.
        let remaining = limit.saturating_sub(*count);
        let matches: Vec<(u64, Vec<u8>)> = tokio::task::spawn_blocking(move || {
            let mut searcher: Searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(BinaryDetection::quit(b'\x00'))
                .build();
```

Replacement:

```rust
        // Binary detection is the caller's choice (the `binary` parameter above); this note is only
        // about why the two values it can hold are what they are.
        //
        // Pi spawns real ripgrep with no `--text`/`--binary`/`--null-data` (grep.ts:220-224), so
        // `hiargs.rs:1141-1157` resolves to explicit=`convert(b'\x00')`, implicit=`quit(b'\x00')`.
        // Implicit is copied verbatim: a NUL ends a traversed file as if EOF, so it contributes no
        // `--json` match events at all — not even for lines BEFORE the NUL.
        //
        // Explicit is copied by BEHAVIOR rather than by name, and the mode is `none()`. ripgrep
        // memory-maps exactly this case (one path, `is_file()` — `hiargs.rs:233-244`), and
        // `convert` is documented as having NO EFFECT under a memory map
        // (grep-searcher-0.1.16 `searcher/mod.rs:88-94`): the bytes reach the sink untouched and
        // line numbers stay the file's raw `\n` numbering. cyrup always uses `search_reader`
        // (`FsOps::read_stream` below), the branch where `convert` DOES fire and rewrites every
        // NUL to the line terminator (`line_buffer.rs:448-460`) — which shifts every line number
        // after the first NUL. `none()` is the reader-side mode that reproduces what Pi observes,
        // byte for byte, and it keeps the searcher's numbering in agreement with the raw
        // `\n`-split re-read that the context / non-UTF-8 blocks below are cut from.
        //
        // `grep-searcher`'s own default is also `none()`, but it must not be relied on implicitly:
        // it would apply to the walk too, dumping raw bytes of every PNG/wasm/font/sqlite hit in
        // the tree into the model-facing result.
        //
        // The searcher is built per file rather than hoisted because `search_reader` is a
        // BLOCKING API driven from `spawn_blocking` (see [`FsOps::read_stream`]), so it and the
        // reader must be owned by the blocking task.
        let matcher_owned = matcher.clone();
        // `MatchSink` counts against the REMAINING budget; the caller's global `count` is advanced
        // by however many this file contributed. Pi's cap is global too — its line handler ignores
        // every event once `matchCount >= effectiveLimit` (grep.ts:278) — so a file can only ever
        // fill the gap.
        let remaining = limit.saturating_sub(*count);
        let matches: Vec<(u64, Vec<u8>)> = tokio::task::spawn_blocking(move || {
            let mut searcher: Searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(binary)
                .build();
```

### 3. Build the two modes once in `execute`

Current ([grep.rs:338-340](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
        let mut out: Vec<String> = Vec::new();
        let mut count = 0usize;
        let mut any_line_truncated = false;
```

Replacement:

```rust
        // ripgrep's two detection modes, one per candidate class (`hiargs.rs:1141-1157` with Pi's
        // flag set). The explicit one is `none()` and NOT `convert(b'\x00')` on purpose — see the
        // note in `search_one`.
        let binary_explicit = BinaryDetection::none();
        let binary_implicit = BinaryDetection::quit(b'\x00');

        let mut out: Vec<String> = Vec::new();
        let mut count = 0usize;
        let mut any_line_truncated = false;
```

### 4. Call site 1 — the explicit-file branch

Current ([grep.rs:350-360](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
            self.search_one(
                &search_root,
                &rel,
                &matcher,
                context,
                limit,
                &mut count,
                &mut out,
                &mut any_line_truncated,
            )
            .await?;
```

Replacement:

```rust
            // `meta.is_file` IS ripgrep's explicit rule: `Haystack::is_explicit()` is "depth 0 and
            // not a directory", i.e. a path handed to `rg` on the command line — which is what
            // Pi's `path` argument becomes (grep.ts:224). ripgrep never filters such a file out.
            self.search_one(
                &search_root,
                &rel,
                &matcher,
                binary_explicit,
                context,
                limit,
                &mut count,
                &mut out,
                &mut any_line_truncated,
            )
            .await?;
```

`binary_explicit` is moved here; this branch runs at most once per call, so no clone is needed.

### 5. Call site 2 — the fused walk loop

Current ([grep.rs:415-425](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
                                self.search_one(
                                    &w.path,
                                    &rel,
                                    &matcher,
                                    context,
                                    limit,
                                    &mut count,
                                    &mut out,
                                    &mut any_line_truncated,
                                )
                                .await?;
```

Replacement:

```rust
                                // Traversal-discovered, so implicit: binary files are still cut
                                // off at the first NUL, exactly as before this change.
                                self.search_one(
                                    &w.path,
                                    &rel,
                                    &matcher,
                                    binary_implicit.clone(),
                                    context,
                                    limit,
                                    &mut count,
                                    &mut out,
                                    &mut any_line_truncated,
                                )
                                .await?;
```

`BinaryDetection` is `Clone` (ripgrep clones it per worker at `hiargs.rs:696-697`); the clone is a
one-byte enum copy inside a newtype.

---

## What this costs, stated plainly

With `quit`, the streaming line buffer for a candidate was bounded by the offset of its first NUL.
On the explicit branch it is now bounded by the longest `\n`-delimited run in that one user-named
file, because `grep-searcher`'s `LineBuffer` grows eagerly (`line_buffer.rs:29-32`,
`BufferAllocation::Eager`) — the same bound that already governs every non-binary file cyrup
searches today. This is accepted rather than mitigated: it applies to at most one file per call, the
file was named by the user, each emitted row is still capped at `GREP_MAX_LINE_LENGTH` (500), and
the whole result is still capped by `max_bytes`. No heap-limit knob is added — ripgrep exposes none
either, and adding one would invent behaviour pi does not have.

---

## Coordination with the two sibling briefs on this file

Three briefs edit
[crates/cyrup-tools/src/tools/grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs). They touch
**three different builders** and are independently applicable in any order.

| Brief | Builder / seam it owns | Regions |
| --- | --- | --- |
| [LOW — cancellation is only observed between candidate files](./LOW-cancellation-is-only-observed-between-candidate-files-not-during-a-file.md) | the `io::Read` + `Sink` seam, new `ops/cancel_read.rs` | `:72-83` signature, `:93-95`, `:113-137`, `:162-163`, insert at `:233`, `:243-259`, `:350-360`, `:415-425`, `:388-393` |
| [MEDIUM — a pattern containing a newline](./MEDIUM-a-pattern-containing-a-newline-errors-in-pi-but-silently-yields-no-match.md) | `RegexMatcherBuilder` | `:308-312`, insert at `:32` |
| **this brief** | `SearcherBuilder` / `BinaryDetection` | `:72-83` signature, `:97-123`, `:338-340`, `:350-360`, `:415-425` |

**No overlap with the newline brief.** It edits the `RegexMatcherBuilder` chain in `execute`
(`:308-312`) and inserts a helper near the imports. This brief touches neither, and inserts its two
mode locals *below* that chain, at `:338`. It also leaves `matcher: &grep_regex::RegexMatcher` built
once in `execute` and shared, as that brief requires.

**Overlap with the cancellation brief — reconciled explicitly.** Both add a parameter to
`search_one`, both edit the `spawn_blocking` block, and both edit the two call sites.

- **Signature.** Both new parameters go immediately after `matcher`, `cancel` first and `binary`
  second, whichever change lands first:

  ```rust
      #[allow(clippy::too_many_arguments)]
      async fn search_one(
          &self,
          file: &std::path::Path,
          rel: &str,
          matcher: &grep_regex::RegexMatcher,
          cancel: &CancelToken,          // cancellation brief
          binary: BinaryDetection,       // this brief
          context: usize,
          limit: usize,
          count: &mut usize,
          out: &mut Vec<String>,
          any_line_truncated: &mut bool,
      ) -> Result<(), ToolError> {
  ```

  Both call sites take the same order:
  `(&path, &rel, &matcher, &cancel, binary_…, context, limit, &mut count, &mut out, &mut any_line_truncated)`.

- **`spawn_blocking` block.** The cancellation brief rewrites that block's *outcome handling* — the
  binding becomes `Result<Vec<(u64, Vec<u8>)>, Aborted>`, `reader` is wrapped in `CancelReader`, and
  `MatchSink` gains a `cancel` field. This brief changes exactly one line inside the same block,
  `.binary_detection(BinaryDetection::quit(b'\x00'))` → `.binary_detection(binary)`. Merged:

  ```rust
          let searched: Result<Vec<(u64, Vec<u8>)>, Aborted> = tokio::task::spawn_blocking(move || {
              let mut searcher: Searcher = SearcherBuilder::new()
                  .line_number(true)
                  .binary_detection(binary)      // this brief; everything else is the cancel brief's
                  .build();
  ```

- **The cancellation brief's `binary` interaction note.** That brief observes that under
  `BinaryDetection::quit`, `LineBuffer::fill` returns early once a NUL is seen and so stops pulling
  from the reader. That still holds for the **walk** branch, which keeps `quit`. On the explicit
  branch under `none()` the reader is pulled to EOF, which strictly *improves* that brief's mid-file
  cancellation: there are more reads through `CancelReader`, not fewer.

- **`MatchSink` and its `Sink` impl.** This brief does not touch them. The default
  `Sink::binary_data` is never invoked under `none()`, and is unchanged under `quit`.

---

## Files changed

- [crates/cyrup-tools/src/tools/grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs) — the only
  file. Five edits: the `search_one` signature, the binary-detection comment block plus the
  `SearcherBuilder` call, the two mode locals in `execute`, and the two `search_one` call sites.

## Files that do NOT change

- [crates/cyrup-tools/src/ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs),
  [crates/cyrup-tools/src/ops/local/fs.rs](../../../crates/cyrup-tools/src/ops/local/fs.rs) —
  `read_stream` already hands back a real `std::fs::File` (`fs.rs:73-80`); the streaming seam is
  untouched.
- [crates/cyrup-tools/src/config.rs](../../../crates/cyrup-tools/src/config.rs) — `GrepOpts`
  (`config.rs:272-284`) keeps exactly `limit` and `max_bytes`. Binary handling is not configurable
  in pi, so it must not become configurable here.
- [crates/cyrup-tools/src/registry.rs](../../../crates/cyrup-tools/src/registry.rs) — the single
  registration at `registry.rs:88` is unchanged.
- The tool's JSON schema at [grep.rs:44-56](../../../crates/cyrup-tools/src/tools/grep.rs) — no
  `text` / `binary` / `-a` input field is added. pi has none; cyrup's declaration stays
  byte-identical to pi's.
- [crates/cyrup-tools/src/truncate.rs](../../../crates/cyrup-tools/src/truncate.rs) —
  `GREP_MAX_LINE_LENGTH` and the byte cap already bound how much of a binary line can be emitted.
- [crates/cyrup-tools/src/tools/find.rs](../../../crates/cyrup-tools/src/tools/find.rs) — `find`
  matches file names, never content, and has no searcher.

## Explicitly out of scope

- **`--text` / `-a` as a user-facing option.** pi exposes none.
- **`.ban_byte(Some(b'\x00'))` on the matcher** (`hiargs.rs:502-504`, applied when the pair as a
  whole is non-`none`). Under this change the pair stays non-`none`, so ripgrep's own
  `binary.is_none()` gate is unaffected; that divergence is a separate finding already claimed by
  the newline-pattern brief.
- **`quit` being laxer under a memory map than under a reader** (`searcher/mod.rs:73-78`: only a
  leading region plus matching/context lines are scanned). cyrup never memory-maps, so its walk
  branch is *stricter* than rg's. Distinct finding, distinct surface.
- **macOS.** `grep-searcher` refuses memory maps there (`searcher/mmap.rs:73-76`), so `rg` falls to
  the reader and `convert` renumbers lines — pi's own output for a binary file differs by platform.
  This brief targets the behaviour measured above, which is also the numbering that keeps the
  printed line numbers usable with the `read` tool.
- **Non-regular explicit paths** (FIFOs, devices). `meta.is_file` is false for those today
  (`fs.rs:170-183` uses `std::fs::Metadata::is_file`), so they take the walk branch. Unchanged here.

## Citations corrected during this augmentation

- The original *Parity action*'s `BinaryDetection::convert(b'\x00')` → **`BinaryDetection::none()`**,
  for the reasons measured above.
- The refutation quote's `grep.ts:219-225` → **grep.ts:220-224** (`spawn` is at `:226`).
- The refutation quote's `tests/tools.rs:858-907` → the correct path is
  [crates/cyrup-tools/src/tests/tools.rs](../../../crates/cyrup-tools/src/tests/tools.rs) and the
  correct span is `:854-906`. What is recorded there describes the **walk** branch, which this
  change leaves alone.
- The in-code comment at [grep.rs:303-306](../../../crates/cyrup-tools/src/tools/grep.rs) cites
  `grep.ts:186` for `Path not found`; the current line is **grep.ts:189**. Noted, not edited — it
  lies outside this change's regions.
- Verified exactly as written: `grep.rs:120-123`, `grep.rs:104-108`, `grep.rs:342-360`,
  `grep.rs:367-433`, `grep.rs:44-56`, `config.rs:272-284`, `registry.rs:88`, `grep.ts:224`,
  `grep.ts:278`.

---

## Definition of done

1. `grep` with `path` naming a file that contains NUL bytes returns that file's matching lines
   instead of `No matches found`. For `bin.dat` = `hello NEEDLE\n` + `NUL SOH STX` +
   `binary NEEDLE\n` + `tail NEEDLE\n`, the call `{"pattern":"NEEDLE","path":"bin.dat"}` returns
   three rows — `bin.dat:1:`, `bin.dat:2:`, `bin.dat:3:` — where row 2 carries the NUL and control
   bytes verbatim, and the row numbers are the file's raw `\n` numbering. Same rows, same order,
   same text, same numbers that pi returns for the same call.
2. The same file reached by **traversal** (`{"pattern":"NEEDLE"}` with `bin.dat` inside the searched
   tree, or `path` naming its parent directory) still contributes **no rows at all** — not even the
   pre-NUL line 1 — and a plain text file alongside it still matches normally.
3. On an explicitly named file, a matched line that is **not** valid UTF-8 renders through the
   re-read block with U+FFFD replacement characters, at the same line number the searcher reported,
   while valid-UTF-8 matched lines in the same file render directly and unaltered.
4. With `context > 0` on an explicitly named binary file, the rows printed around a match on line
   *n* are the file's raw lines *n-context* … *n+context* — the context block lines up with the
   match line rather than being offset.
5. A file named explicitly but unreadable still yields no rows and no error, as before.
6. The `limit`, `max_bytes` and 500-character line caps apply to binary rows exactly as to text
   rows, and the notice bracket appears on the same conditions as before.
7. No new input field appears on the `grep` tool; its declared schema is unchanged.
8. `grep` output is unchanged for every non-binary file, for every `glob`, for `path` naming a
   directory, and on every cancellation path.
