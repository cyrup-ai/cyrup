//! `grep` — in-process ripgrep-parity search (R-03-029…031, arch-03 §6.6). [CYRUP-DELTA]: uses the
//! `ignore`/`grep` crates instead of an external `rg` binary; output format, gitignore semantics,
//! and truncation preserve Pi's observable behavior.

use crate::config::GrepOpts;
use crate::ops::cancel_read::{CancelReader, Cancelled};
use crate::ops::{FsOps, PathSort, WalkFlavor, WalkOpts};
use crate::tools::globmatch::{RgGlob, to_posix};
use crate::tools::rgconfig::{self, CaseMode, RgFlags};
use crate::truncate::{GREP_MAX_LINE_LENGTH, TruncOpts, format_size, truncate_head, truncate_line};
use crate::{error, path};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Encoding, Searcher, SearcherBuilder, Sink, SinkMatch};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrepInput {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    ignore_case: Option<bool>,
    literal: Option<bool>,
    // Pi's TypeBox `Type.Number` (grep.ts:31-34) carries no `integer` and no `minimum`, and Pi
    // never validates tool arguments at runtime — it clamps them at the point of use instead
    // (grep.ts:188-189). Modeling these as `usize` rejected `context: 0.0` / `limit: -1` at
    // deserialization, where Pi returns a normal result. See [`crate::jsnum`].
    context: Option<f64>,
    limit: Option<f64>,
}

/// Render a `grep-regex` build failure as the bytes Pi's caller actually receives.
///
/// Pi never builds a matcher itself. It spawns `rg`, buffers the child's stderr whole
/// (grep.ts:228, :251-253) and, for any exit code other than 0 or 1, rejects with `stderr.trim()`
/// (grep.ts:309-312) — a rejection nothing catches, so rg's stderr text IS the model-observed tool
/// error. That text has three layers, and dropping any one of them diverges:
///
/// 1. The inner message is `grep_regex::Error`'s own `Display`. cyrup links the same crate, so it
///    is already byte-identical: `the literal "\n" is not allowed in a regex` for
///    `ErrorKind::NotAllowed`, `pattern contains "\0" but it is impossible to match` for
///    `ErrorKind::Banned` (grep-regex-0.1.14 `src/error.rs:69-93`), and the same
///    `regex parse error: …` block — `(?:…)` wrapper included, since grep-regex wraps every
///    pattern that way (`src/config.rs:183-188`) — for a syntax error.
/// 2. ripgrep runs the message through THREE hint filters, each gated on the message text itself
///    and on nothing else, composed as `suggest_other_engine(suggest_text(suggest_multiline(msg)))`
///    (ripgrep 14.1.0 `crates/core/flags/hiargs.rs:505-510` for the inner two, `:366-373` for the
///    outer one). The three predicates below are those verbatim
///    (`hiargs.rs:1412-1462`). `suggest_other_engine` forwards to `suggest_pcre2`, whose first
///    statement is `if !cfg!(feature = "pcre2") { return None; }` (`hiargs.rs:1416-1419`) — so the
///    hint is a property of the BUILD, and that gate is TRUE for the binary Pi runs. Pi downloads
///    the official `BurntSushi/ripgrep` release asset (tools-manager.ts:50-70), and release CI
///    builds EVERY matrix target with `--features pcre2` under `PCRE2_SYS_STATIC=1`
///    (`.github/workflows/release.yml:177`, env at `:60-61`), including
///    `x86_64-unknown-linux-musl` — the asset Pi names for Linux (`release.yml:69`). With the
///    verbatim stderr passthrough above (grep.ts:309-310), the suggestion block IS part of Pi's
///    tool output, so cyrup emits it too. cyrup cannot honour the `--pcre2` it advises; see
///    [`RgFlags::PCRE2_IS_DECLINED`] for that divergence and why it is recorded, not erased.
/// 3. ripgrep prefixes every top-level error with `rg: ` (`crates/core/messages.rs:50`, reached
///    from `eprintln_locked!("{:#}", err)` at `crates/core/main.rs:62`).
fn rg_pattern_error(err: &grep_regex::Error) -> String {
    let msg = suggest_other_engine(suggest_text(suggest_multiline(err.to_string())));
    format!("rg: {msg}")
}

/// The refusal for a pattern carrying a RAW NUL byte, byte-identical to `ban::check`'s.
///
/// Routed through the same three suggestion helpers as [`rg_pattern_error`] rather than spelling
/// the final text out, so the two paths cannot drift apart: `ban::check`'s own wording is the seed,
/// `suggest_multiline` does not fire (no "the literal … not allowed"), `suggest_text` appends the
/// `--text` advice, and `suggest_other_engine` does not fire.
///
/// See `build_matcher` for why a raw NUL is refused ahead of the builder at all.
fn rg_nul_literal_error() -> String {
    let seed = "pattern contains \"\\0\" but it is impossible to match".to_string();
    format!(
        "rg: {}",
        suggest_other_engine(suggest_text(suggest_multiline(seed)))
    )
}

/// ripgrep `hiargs.rs:1437-1448`, verbatim.
fn suggest_multiline(msg: String) -> String {
    if msg.contains("the literal") && msg.contains("not allowed") {
        format!(
            "{msg}\n\n\
             Consider enabling multiline mode with the --multiline flag (or -U for short).\n\
             When multiline mode is enabled, new line characters can be matched."
        )
    } else {
        msg
    }
}

/// ripgrep `hiargs.rs:1451-1462`, verbatim. Reachable only because the matcher below sets
/// `ban_byte(Some(b'\x00'))`, exactly as ripgrep does for Pi's flag set.
fn suggest_text(msg: String) -> String {
    if msg.contains("pattern contains \"\\0\"") {
        format!(
            "{msg}\n\n\
             Consider enabling text mode with the --text flag (or -a for short). Otherwise,\n\
             binary detection is enabled and matching a NUL byte is impossible."
        )
    } else {
        msg
    }
}

/// ripgrep `suggest_other_engine` + `suggest_pcre2` (`hiargs.rs:1412-1431`), flattened: the outer
/// function only forwards, and the `cfg!(feature = "pcre2")` guard is always true for the binaries
/// Pi runs (see [`rg_pattern_error`]). Fires on a Rust-engine refusal of a backreference or a
/// look-around — reachable today, with no other setting required.
fn suggest_other_engine(msg: String) -> String {
    if msg.contains("backreferences") || msg.contains("look-around") {
        format!(
            "{msg}\n\n\
             Consider enabling PCRE2 with the --pcre2 flag, which can handle backreferences\n\
             and look-around."
        )
    } else {
        msg
    }
}

/// Build `grep`'s matcher.
///
/// Extracted from `execute` so a guard can drive the PRODUCTION construction rather than a copy of
/// the builder. `matcher.line_terminator()` is an exact discriminator for `.multi_line(true)` — it
/// is `Some(LineTerminator(Byte(10)))` for an anchored pattern with the flag and `None` without —
/// but a test can only use it if it can reach the matcher this function returns.
fn build_matcher(
    input: &GrepInput,
    case: CaseMode,
    rg: &RgFlags,
) -> Result<grep_regex::RegexMatcher, ToolError> {
    // A raw NUL in the pattern is refused HERE, before the builder, because the builder refuses it
    // on only one of its two branches.
    //
    // `ban_byte(Some(b'\x00'))` runs inside `ban::check`, which the parsed branch reaches but the
    // fixed-strings branch does not (grep-regex `config.rs:195-212`): `is_fixed_strings`
    // (`:102-124`) bails on the LINE TERMINATOR and on nothing else, so under `literal: true` a
    // pattern holding a real NUL byte compiles happily and the tool answers `No matches found` —
    // a confident false negative — while the same input under `literal: false` correctly errors.
    //
    // This input is unreachable for pi: Node's `spawn` rejects a NUL in argv, so `rg` never sees
    // one and there is no upstream behaviour to match. cyrup takes the pattern from JSON and can
    // therefore reach a state pi cannot. The choice to refuse it is CYRUP'S OWN, made because one
    // branch erroring while the other silently answers wrong is an inconsistency rather than a
    // considered position — not because upstream does this.
    //
    // The message is `ban::check`'s own, so the two branches now agree byte-for-byte.
    if input.pattern.as_bytes().contains(&b'\x00') {
        return Err(error::invalid(rg_nul_literal_error()));
    }

    let mut builder = RegexMatcherBuilder::new();
    builder.multi_line(true);
    if rg.crlf {
        builder.crlf(true);
    } else {
        builder.line_terminator(Some(b'\n'));
    }
    builder
        .case_insensitive(case == CaseMode::Insensitive)
        .case_smart(case == CaseMode::Smart)
        .word(rg.word)
        .whole_line(rg.whole_line)
        .unicode(!rg.no_unicode)
        .fixed_strings(input.literal.unwrap_or(rg.fixed_strings))
        .ban_byte(Some(b'\x00'))
        .build(&input.pattern)
        .map_err(|e| error::invalid(rg_pattern_error(&e)))
}

pub struct GrepTool {
    fs: Arc<dyn FsOps>,
    cwd: PathBuf,
    opts: GrepOpts,
    params: serde_json::Value,
}

impl GrepTool {
    pub fn new(fs: Arc<dyn FsOps>, cwd: PathBuf, opts: GrepOpts) -> Self {
        // Byte-for-byte Pi's TypeBox emission (grep.ts:24-36): verbatim descriptions,
        // `type:"number"`, no `minimum`, no `additionalProperties`.
        let params = serde_json::json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": { "type": "string", "description": "Search pattern (regex or literal string)" },
                "path": { "type": "string", "description": "Directory or file to search (default: current directory)" },
                "glob": { "type": "string", "description": "Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'" },
                "ignoreCase": { "type": "boolean", "description": "Case-insensitive search (default: false)" },
                "literal": { "type": "boolean", "description": "Treat pattern as literal string instead of regex (default: false)" },
                "context": { "type": "number", "description": "Number of lines to show before and after each match (default: 0)" },
                "limit": { "type": "number", "description": "Maximum number of matches to return (default: 100)" }
            }
        });
        Self {
            fs,
            cwd,
            opts,
            params,
        }
    }

    /// Search ONE candidate and append its formatted rows to `out`, advancing the GLOBAL match
    /// `count`. Extracted from `execute` so the search can be fused into the walk (see the loop
    /// there) instead of running as a second pass over a fully-drained, sorted file list.
    ///
    /// Pi gathers raw matches first (no context in the searcher), then formats each match as an
    /// independent re-read block; mirror that so overlapping windows duplicate shared context
    /// lines exactly as Pi does (`formatBlock`, grep.ts:255-273).
    /// Search a BATCH of candidates: one seam call to open them all, one `spawn_blocking`, one
    /// `Searcher` reused across every file in the batch.
    ///
    /// Batching is the point of this function, and it is a measured one. Per file the old shape
    /// paid TWO `spawn_blocking` round-trips — [`FsOps::read_stream`] does one just to reach
    /// `File::open`, then the search did its own — strictly ordered, because the open has to
    /// finish before the search starts. On a 1,885-entry tree that is ~3,770 round-trips at ~31µs,
    /// ~30ms of scheduler time against ~2.9ms of real `open(2)` work and ~21ms of real searching.
    /// Running several of those per-file futures at once does not remove the round-trips; only
    /// batching does. Measured end to end on `crates/`: ~75ms → ~18ms.
    ///
    /// The `Searcher` is built ONCE here rather than per file. It is `!Sync` (three `RefCell`s,
    /// grep-searcher 0.1.16 `searcher/mod.rs:597-624`) so it can never be shared across threads,
    /// but it is perfectly reusable within one blocking task, and `search_reader` takes `&mut
    /// self`. That is worth ~3ms per full-tree scan on its own.
    ///
    /// Every candidate in a batch shares one `binary` mode, which is what makes one searcher
    /// legal: the caller batches walk-discovered files, and those are all implicit. The explicit
    /// path arrives as a batch of one.
    #[allow(clippy::too_many_arguments)]
    async fn search_batch(
        &self,
        // Owned `(path, rel)`, not borrowed. This future is pushed into a `FuturesUnordered` and
        // outlives the loop iteration that built the batch, so it cannot hold a `&` to that
        // iteration's `WalkItem`. Everything else it borrows (`self`, `matcher`, `cancel`, `rg`,
        // `encoding`) is declared ABOVE the loop and outlives every future, so those stay borrows
        // and no `Arc` is needed.
        files: Vec<(PathBuf, String)>,
        matcher: &grep_regex::RegexMatcher,
        // Observed at every `await` below and, via `CancelReader` and `MatchSink`, inside the
        // blocking search itself — and now also BETWEEN files of a batch, which is the one new
        // abort edge batching introduces.
        cancel: &CancelToken,
        // ripgrep chooses binary detection PER HAYSTACK, not per invocation (`hiargs.rs:1124-1157`).
        // The caller classifies the candidates; a batch is uniform by construction.
        binary: BinaryDetection,
        rg: &RgFlags,
        encoding: Option<&Encoding>,
        context: usize,
        // The GLOBAL match cap, applied per file. With searches in flight concurrently there is no
        // exact remaining budget at dispatch time, so each file is capped at the most any single
        // file could contribute and the excess is trimmed deterministically at render.
        // `-m/--max-count` stays on the searcher and stays independent — see `max_matches` below.
        limit: usize,
    ) -> Result<Vec<(PathBuf, Vec<MatchBlock>)>, ToolError> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        // Pi never materializes a candidate in the agent process: the search runs in a separate
        // ripgrep child (grep.ts:226) with rg's own bounded read buffer, so file size is decoupled
        // from the agent's heap. cyrup's search is in-process, so the same property comes from
        // [`FsOps::read_streams`] + `search_reader`. A full `FsOps::read` here allocated the whole
        // file BEFORE binary detection could reject it — one multi-GB log or vendored tarball in
        // the tree was an RSS spike or an OOM kill, on a file that need not match at all.
        //
        // A cancel landing while the opens are in flight must not wait for them — a remote/RPC
        // `FsOps` can park here. `run_until_cancelled` returns `None` immediately when the token is
        // ALREADY cancelled (tokio-util 0.7.18 `sync/cancellation_token.rs:280-293`), so this
        // doubles as the entry guard.
        let paths: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
        let Some(opened) = cancel
            .run_until_cancelled(self.fs.read_streams(&paths))
            .await
        else {
            return Err(error::aborted());
        };

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
        // line numbers stay the file's raw `\n` numbering. cyrup always uses `search_reader`, the
        // branch where `convert` DOES fire and rewrites every NUL to the line terminator
        // (`line_buffer.rs:448-460`) — which shifts every line number after the first NUL.
        // `none()` is the reader-side mode that reproduces what Pi observes, byte for byte, and it
        // keeps the searcher's numbering in agreement with the raw `\n`-split re-read that the
        // context / non-UTF-8 blocks below are cut from.
        //
        // `grep-searcher`'s own default is also `none()`, but it must not be relied on implicitly:
        // it would apply to the walk too, dumping raw bytes of every PNG/wasm/font/sqlite hit in
        // the tree into the model-facing result.
        let matcher_owned = matcher.clone();
        // The token is moved INTO the blocking task rather than polled from outside it: a
        // `spawn_blocking` task owns an OS thread and cannot be aborted by dropping its
        // `JoinHandle`, so the only way out of `search_reader` is for the work itself to fail.
        // Cloning is an `Arc` refcount bump (`CancelToken` is `tokio_util::sync::CancellationToken`,
        // cyrup-core `cancel.rs:9`).
        let cancel_task = cancel.clone();
        let invert = rg.invert_match;
        let max_count = rg.max_count;
        let encoding = encoding.cloned();
        let searched: Result<Vec<FileMatches>, Aborted> = tokio::task::spawn_blocking(move || {
            let mut searcher: Searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(binary)
                .invert_match(invert)
                // `-m/--max-count` is ripgrep's PER-FILE cap, a different axis from pi's
                // GLOBAL `limit`: rg stops reading each haystack after N matches and moves on
                // to the next file, whereas `limit` ends the whole search. So it belongs on
                // the searcher and not folded into the sink's budget — both caps apply
                // independently, and reusing one searcher across the batch does not change
                // that: `max_matches` is re-applied per `search_reader` call.
                .max_matches(max_count)
                // `None` is grep-searcher's "sniff the BOM, else assume UTF-8" default, so an
                // absent `-E` leaves the existing behaviour exactly as it was.
                .encoding(encoding)
                .build();
            let mut per_file: Vec<FileMatches> = Vec::with_capacity(opened.len());
            for reader in opened {
                // The abort edge batching adds. Within a file `CancelReader` and `MatchSink`
                // still observe the token; without this check a cancelled search would keep
                // grinding through the REST of the batch before anything noticed.
                if cancel_task.is_cancelled() {
                    return Err(Aborted);
                }
                // A file that could not be opened contributes nothing and does not stop the
                // batch: rg emits no match events for a file it cannot read. Its slot is kept
                // so `per_file` stays index-aligned with `files`.
                let Ok(reader) = reader else {
                    per_file.push(Vec::new());
                    continue;
                };
                let mut matches: FileMatches = Vec::new();
                let mut local = 0usize;
                let outcome = {
                    let sink = MatchSink {
                        matches: &mut matches,
                        count: &mut local,
                        limit,
                        cancel: cancel_task.clone(),
                    };
                    searcher.search_reader(
                        &matcher_owned,
                        CancelReader::new(reader, cancel_task.clone()),
                        sink,
                    )
                };
                // A cancel marker is an abort for the whole batch; EVERY other `io::Error`
                // keeps the previous per-file semantics — whatever was collected before the
                // failure stands and the walk moves on.
                if let Err(e) = &outcome
                    && Cancelled::is(e)
                {
                    return Err(Aborted);
                }
                per_file.push(matches);
            }
            Ok(per_file)
        })
        .await
        .map_err(|e| error::invalid(format!("grep: {e}")))?;

        let Ok(per_file) = searched else {
            return Err(error::aborted());
        };

        let mut out: Vec<(PathBuf, Vec<MatchBlock>)> = Vec::new();
        for ((path, rel), matches) in files.into_iter().zip(per_file) {
            // A file that matched nothing contributes no rows and cannot affect the order of the
            // ones that did, so it is dropped HERE rather than carried to the caller's sort. On a
            // search whose pattern is rare or absent — the case this task targets — that is very
            // nearly every file in the tree.
            if matches.is_empty() {
                continue;
            }
            // Which matches take Pi's `formatBlock` path (grep.ts:333-335) rather than the direct
            // `match.lineText` path (grep.ts:323-331)? Pi's condition is
            // `contextValue === 0 && match.lineText !== undefined`, and `lineText` is
            // `event.data.lines.text` — ABSENT whenever ripgrep could not encode the line as UTF-8
            // (it serialises `lines.bytes` instead). So: context>0, or a non-UTF-8 matched line.
            let takes_block = |raw: &[u8]| context > 0 || std::str::from_utf8(raw).is_err();
            // `formatBlock` reads through `getFileLines`, a **second, independent** read of the
            // file (grep.ts:206-218), cached for the rest of the invocation by `fileCache`. Do it
            // at most once per file, and ONLY if some match actually needs it, so that at
            // context==0 the file is never re-read (Pi does not) yet the read-your-latest-writes
            // semantics and the failure path below stay reachable exactly where Pi has them. Pi's
            // own `fileCache` is moot here: each candidate is visited exactly once.
            //
            // This is a WHOLE-FILE read of a file that already matched — on a multi-hundred-MB
            // candidate it is the second place a cancel could be stranded, so it is raced too.
            // `Err(_)` stays Pi's `catch { lines = [] }` (grep.ts:212-214); only a `None` — the
            // token firing — aborts.
            let src_lines: Option<Vec<String>> = if matches.iter().any(|(_, r)| takes_block(r)) {
                let Some(read) = cancel.run_until_cancelled(self.fs.read(&path)).await else {
                    return Err(error::aborted());
                };
                match read {
                    // Pi `getFileLines` folds `\r\n`→`\n` AND lone `\r`→`\n` BEFORE splitting
                    // (grep.ts:211). The matcher numbered lines on raw `\n`, so a file using
                    // lone-`\r` separators yields context blocks that key off these folded
                    // segments — matching Pi even where that diverges from the matcher's numbering.
                    Ok(b) => {
                        let content = String::from_utf8_lossy(&b);
                        let folded = content.replace("\r\n", "\n").replace('\r', "\n");
                        Some(folded.split('\n').map(str::to_owned).collect())
                    }
                    Err(_) => None,
                }
            } else {
                None
            };
            out.push((
                path,
                render_blocks(&rel, &matches, src_lines.as_ref(), context),
            ));
        }
        Ok(out)
    }
}

/// Turn one file's raw matches into one [`MatchBlock`] per match.
///
/// Split out of the search so the batch loop above reads as open → search → render, and so the
/// per-match formatting stays one thing rather than being interleaved with I/O.
///
/// **One block per match, always** — including the one-row `(unable to read file)` marker, which
/// pi counts as an emitted match — so `blocks.len()` IS the file's match count and the caller's
/// cap keeps counting the axis pi counts on.
fn render_blocks(
    rel: &str,
    matches: &[(u64, Vec<u8>)],
    src_lines: Option<&Vec<String>>,
    context: usize,
) -> Vec<MatchBlock> {
    let takes_block = |raw: &[u8]| context > 0 || std::str::from_utf8(raw).is_err();
    let mut blocks: Vec<MatchBlock> = Vec::with_capacity(matches.len());
    for (ln, raw) in matches {
        let l = *ln as usize;
        // `rows` is what this match renders to — one row at `context == 0`, or pi's
        // `2·context+1`-line window — and `line_truncated` describes only these rows, so a block
        // the cap later drops cannot raise the notice.
        let mut rows: Vec<String> = Vec::new();
        let mut line_truncated = false;
        if !takes_block(raw) {
            // Pi grep.ts:325-331: format straight from the captured line text — `\r\n`→`\n`, then
            // DROP every remaining `\r` (not fold it to `\n`, which is what `getFileLines` does),
            // then strip ONE trailing `\n`.
            let stripped = String::from_utf8_lossy(raw)
                .replace("\r\n", "\n")
                .replace('\r', "");
            let text = stripped.strip_suffix('\n').unwrap_or(&stripped);
            let (capped, tr) = truncate_line(text, GREP_MAX_LINE_LENGTH);
            if tr {
                line_truncated = true;
            }
            rows.push(format!("{rel}:{l}: {capped}"));
            blocks.push(MatchBlock {
                rows,
                line_truncated,
            });
            continue;
        }
        // Pi `formatBlock`: an unreadable file collapses the whole block to ONE marker row per
        // match (grep.ts:258). The rows still count as output, so they participate in byte
        // truncation like any other line.
        let Some(src_lines) = src_lines else {
            rows.push(format!("{rel}:{l}: (unable to read file)"));
            blocks.push(MatchBlock {
                rows,
                line_truncated,
            });
            continue;
        };
        // Pi: `start = max(1, n - context)`, `end = min(lines.length, n + context)` when
        // context > 0, else just the single match line (grep.ts:260-261).
        let (start, end) = if context > 0 {
            (
                l.saturating_sub(context).max(1),
                (l + context).min(src_lines.len()),
            )
        } else {
            (l, l)
        };
        for current in start..=end {
            let raw = src_lines
                .get(current.saturating_sub(1))
                .map_or("", String::as_str);
            // Pi's per-line `replace(/\r/g,"")` (grep.ts:264).
            let text = raw.replace('\r', "");
            let (capped, tr) = truncate_line(&text, GREP_MAX_LINE_LENGTH);
            if tr {
                line_truncated = true;
            }
            if current == l {
                rows.push(format!("{rel}:{current}: {capped}"));
            } else {
                rows.push(format!("{rel}-{current}- {capped}"));
            }
        }
        blocks.push(MatchBlock {
            rows,
            line_truncated,
        });
    }
    blocks
}

/// The rows one MATCH renders to: exactly one at `context == 0`, or the `2·context+1`-line block
/// pi's `formatBlock` emits (grep.ts:250-268).
///
/// Grouped per match rather than flattened because the global cap counts MATCHES, so a file that
/// overshoots must be trimmed at a match boundary — and with searches running concurrently a file
/// CAN overshoot: each is capped at the global `limit` because no exact remaining budget exists at
/// dispatch time.
struct MatchBlock {
    rows: Vec<String>,
    /// Whether any row in this block hit `GREP_MAX_LINE_LENGTH`. Per-block so the
    /// `Some lines truncated` notice describes only rows that were actually EMITTED — a block
    /// dropped by the cap must not raise it.
    line_truncated: bool,
}

/// Appends blocks until the GLOBAL match cap is reached, counting MATCHES (not rows) — the axis
/// pi counts on (grep.ts:278-292), and the axis `count >= limit` is tested on at every consumer.
fn take_into(
    out: &mut Vec<String>,
    count: &mut usize,
    any_line_truncated: &mut bool,
    limit: usize,
    blocks: Vec<MatchBlock>,
) {
    for b in blocks {
        if *count >= limit {
            return;
        }
        *count += 1;
        *any_line_truncated |= b.line_truncated;
        out.extend(b.rows);
    }
}

/// One file's raw matches: the 1-based line number and the raw bytes of each matching line, in
/// the order the searcher produced them. Named because a batch carries one of these per file and
/// the nested shape is otherwise unreadable at the call site.
type FileMatches = Vec<(u64, Vec<u8>)>;

/// The blocking search stopped because the token fired, not because the file ended.
struct Aborted;

/// Collects the 1-based line number AND the raw bytes of every match in a file, capping the GLOBAL
/// match count at `limit`. Pi counts each rg `match` event (one per matching line) and stops the
/// child once `matchCount >= effectiveLimit` (grep.ts:280-292).
///
/// The bytes are Pi's `event.data.lines.text` (grep.ts:307, kept on the match record at :310): at
/// `context == 0` Pi formats the row straight from that captured text and never re-reads the file
/// (grep.ts:318-326). Only the context>0 path — and the non-UTF-8 fallback, where ripgrep emits
/// `lines.bytes` instead of `lines.text` so `match.lineText` is `undefined` — goes through
/// `formatBlock` → `getFileLines` (grep.ts:250-268), which re-reads and formats an INDEPENDENT
/// block per match, so overlapping context windows DUPLICATE shared lines rather than merging.
struct MatchSink<'a> {
    matches: &'a mut Vec<(u64, Vec<u8>)>,
    count: &'a mut usize,
    limit: usize,
    /// The SECOND abort hook, covering the match-DENSE case `CancelReader` cannot: one 64 KiB
    /// refill can fire `matched` thousands of times, and every one of those callbacks runs before
    /// the searcher asks the reader for another byte. Owned rather than borrowed — the token is an
    /// `Arc` handle, so the clone is a refcount bump and it keeps the `'a` lifetime tied to the
    /// caller's buffers alone.
    cancel: CancelToken,
}

impl Sink for MatchSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, m: &SinkMatch<'_>) -> Result<bool, std::io::Error> {
        // NOT `Ok(false)`. That is `search_reader`'s ordinary "stop, you have enough" signal — the
        // very thing the limit line below uses — and it ends the search SUCCESSFULLY, so the caller
        // could not distinguish a cancel from a satisfied match budget and would emit a normal
        // partial result. Pi rejects with `Operation aborted` instead (grep.ts:305-307), so the
        // cancel has to leave as an `Err` the caller can recognise.
        if self.cancel.is_cancelled() {
            return Err(Cancelled::err());
        }
        // `SinkMatch::bytes` is the matched line INCLUDING its terminator, which is exactly what
        // ripgrep serialises into `data.lines.text` — hence Pi's `.replace(/\n$/,"")` below.
        self.matches
            .push((m.line_number().unwrap_or(0), m.bytes().to_vec()));
        *self.count += 1;
        Ok(*self.count < self.limit)
    }
}

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    /// TOOL-045 — pi declares `label` explicitly beside `name` on every built-in
    /// `ToolDefinition` and the two are equal for all seven (`grep.ts:129-130` @v0.83.0). See
    /// [`super::ReadTool::label`] for why the trait default was not left to stand in.
    fn label(&self) -> Option<&str> {
        Some("grep")
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }

    // Verbatim from Pi (grep.ts:131-132). DEFAULT_LIMIT=100, DEFAULT_MAX_BYTES/1024=50,
    // GREP_MAX_LINE_LENGTH=500. Pi defines no promptGuidelines for grep.
    fn description(&self) -> &str {
        "Search file contents for a pattern. Returns matching lines with file paths and line \
         numbers. Respects .gitignore. Output is truncated to 100 matches or 50KB (whichever is \
         hit first). Long lines are truncated to 500 chars."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Search file contents for patterns (respects .gitignore)")
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let input: GrepInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("grep: {e}")))?;

        let search_root = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);

        // Pi's grep inherits `$RIPGREP_CONFIG_PATH` because it spawns the real binary
        // (`grep.ts:226`, no `env` key, no `--no-config`), so a user who set `--smart-case` or
        // `-g '!vendor/**'` in their config gets it applied to every agent search. cyrup searches
        // in-process, so nothing reads that config unless this does. Read once per call, not per
        // candidate. A missing or unreadable file yields the default (all-off) set — ripgrep only
        // warns on stderr there and searches anyway, and pi discards rg's stderr on a zero exit.
        let rg = RgFlags::read(self.opts.rg_config_path.as_deref());
        let meta = self
            .fs
            .metadata(&search_root)
            .await
            // Pi: `Path not found: ${searchPath}` (grep.ts:186).
            .map_err(|_| {
                error::not_found(format!("Path not found: {}", error::show(&search_root)))
            })?;

        // `line_terminator(Some(b'\n'))` is ripgrep's non-multiline default, and Pi never passes
        // `-U/--multiline` (grep.ts:220-224 carries no such flag), so rg always takes the
        // else-branch of `matcher_rust`:
        //     builder.line_terminator(Some(b'\n')).dot_matches_new_line(false);
        // (ripgrep 14.1.0 `crates/core/flags/hiargs.rs:482-484`).
        //
        // Setting it makes grep-regex GUARANTEE the matcher can never produce a match containing
        // the terminator. `\n` is transparently subtracted from classes like `\s`, so `a\sb` keeps
        // building; a BARE `\n` literal cannot be removed without changing the pattern's intent, so
        // `build` fails with `ErrorKind::NotAllowed("\n")` (grep-regex-0.1.14 `src/config.rs:222-225`
        // → `src/strip.rs:60`). Leaving it at the builder default of `None` (`src/config.rs:61`)
        // skips that guard entirely — the pattern compiled fine and then matched nothing, because
        // the searcher below is line-oriented and no match can span a line break.
        //
        // `fixed_strings` stays wired to `input.literal` and needs no special casing: grep-regex
        // drops out of its literal-alternation fast path when a pattern contains the terminator
        // (`src/config.rs:113-124`), so a REAL newline still errors under `literal: true` while the
        // two-character `\n` escape stays an ordinary literal — exactly what rg does.
        //
        // Consistency with the searcher is required, not optional: `Searcher::check_config` errors
        // with `MismatchedLineTerminators` unless the two agree (grep-searcher-0.1.16
        // `src/searcher/mod.rs:805-821`). The searcher's default is `\n`
        // (`mod.rs:190` → grep-matcher-0.1.8 `src/lib.rs:268-273`) and is not overridden below.
        //
        // `multi_line(true)` is grep-regex's `(?m)` flag, NOT ripgrep's `-U/--multiline`: ripgrep
        // sets it UNCONDITIONALLY on the Rust-engine matcher (`hiargs.rs:461-465`), while `-U`
        // drives a different pair of knobs (`line_terminator(None)` / `dot_matches_new_line`,
        // `hiargs.rs:477-495`) and the SEARCHER's `multi_line` (`hiargs.rs:715`). Pi passes no
        // `-U` (grep.ts:220-224), so the searcher built in `search_one` keeps grep-searcher's
        // `multi_line: false` default and stays line-oriented — matcher and searcher are not the
        // same setting and do not need to agree here.
        //
        // What it buys: with `(?m)` OFF, `^`/`$` translate to the HAYSTACK anchors
        // `Look::Start`/`Look::End`, and `ConfiguredHIR::line_terminator` then reports `None`
        // rather than `\n` for any such pattern — `contains_anchor_haystack()`, grep-regex
        // `src/config.rs:296-302`. A matcher that reports no line terminator fails
        // `Core::is_line_by_line_fast` (grep-searcher-0.1.16 `src/searcher/core.rs:673-706`), so
        // every `^…`/`…$` search silently fell off the fast line searcher onto the slow one and
        // ran the full regex against every line of every candidate instead of letting the literal
        // prefilter skip whole buffers. Output was identical either way — the slow path strips the
        // terminator before matching (`core.rs:99-107`, `:347-351`), which makes `Look::End`
        // behave like `(?m:$)` on a line — so this is ripgrep's engine path, restored, not a
        // change to what the caller sees.
        //
        // `ban_byte(Some(b'\x00'))` is ripgrep's `hiargs.rs:502-504`, gated on
        // `!BinaryDetection::is_none()`. That predicate is `explicit == none && implicit == none`
        // (`hiargs.rs:1159-1164`), so cyrup's deliberately-`none()` EXPLICIT mode (see
        // `search_one`) does not switch it off: the implicit mode is `quit(b'\x00')`, so the ban
        // is on, exactly as it is for Pi. It refuses at BUILD time any pattern whose HIR must
        // match a NUL (`ban::check`, grep-regex `src/ban.rs:8-53`, called from
        // `config.rs:210-212` — before the line-terminator strip, so a pattern holding both a NUL
        // and a `\n` reports the NUL). Without it `grep pattern:"\\x00"` compiled happily and
        // then reported "No matches found", because binary detection had already ended the file at
        // the first NUL — a confident false negative where rg exits 2 with an actionable error.
        //
        // Note the asymmetry `ban::check` inherits from its call site: it runs only on the parsed
        // branch, and only for a class of exactly one element. So `[\x00-\x01]` still builds, and
        // under `literal: true` the pattern is `regex_syntax::escape`d into ordinary backslash-x-0-0
        // characters that hold no NUL at all — rg exits 1 there, and so does this.
        //
        // The config layer sits UNDER the caller's arguments, which is ripgrep's own precedence:
        // it splices the config in as `config_args ++ cli_args` and lets the later occurrence win
        // (`flags/parse.rs`). So an explicit `ignoreCase` on the tool call overrides `-i`/`-s`/`-S`
        // from the config, and `-S/--smart-case` only gets a say when the caller expressed no
        // preference at all — otherwise a config `--smart-case` would quietly re-case a search the
        // model asked to be case-sensitive.
        //
        // `-i`, `-s` and `-S` are ONE mutually-exclusive group resolved to whichever came last
        // (`CaseMode`), and the caller's own `ignoreCase` outranks all three — ripgrep's
        // `config_args ++ cli_args` precedence, where the later occurrence wins
        // (`flags/parse.rs`). An absent config and an absent argument leave the pre-existing
        // case-sensitive default.
        let case = match input.ignore_case {
            Some(true) => CaseMode::Insensitive,
            Some(false) => CaseMode::Sensitive,
            None => rg.case.unwrap_or(CaseMode::Sensitive),
        };

        // `crlf` and `line_terminator` are the SAME setting reached two ways, so exactly one of
        // them may be called — which is why this is an `if` and not two chained calls.
        //
        // In grep-regex (`matcher.rs`), `crlf(true)` sets both `config.crlf` and
        // `config.line_terminator = LineTerminator::crlf()` (`:322-328`); `line_terminator` sets
        // only the terminator (`:281-285`). Both orders of two unconditional calls are wrong, in
        // opposite directions:
        //
        //   * `crlf` then `line_terminator` puts the terminator back to plain LF, so `^`/`$` used
        //     the CRLF anchors from `config.crlf` while `strip_from_match` (`config.rs:222-229`)
        //     and `has_line_terminator` (`:350-357`) still treated `\r` as an ordinary byte;
        //   * `line_terminator` then `crlf` is worse, because `crlf(FALSE)` does not leave the
        //     terminator alone — it sets it to `None` — which silently disarmed the `\n` guard for
        //     every caller who has no config at all.
        //
        // So: the CRLF flag installs its own terminator, and otherwise the plain `\n` default
        // stands exactly as it did before any of this existed.
        let matcher = build_matcher(&input, case, &rg)?;

        // Pi: `const contextValue = context && context > 0 ? context : 0` (grep.ts:188) — a
        // negative, zero or NaN `context` all collapse to 0 instead of failing. `to_count` folds
        // the fraction the way `lineNumber ± contextValue` indexing would (grep.ts:254-255).
        let context = input.context.map_or(0, crate::jsnum::to_count);
        // Pi: `effectiveLimit = Math.max(1, limit ?? DEFAULT_LIMIT)` (grep.ts:189). The JSON-schema
        // `minimum:1` is advisory only, so an explicit `limit: 0` — or a negative one, which the
        // same `Math.max` absorbs — must still yield up to one match rather than short-circuiting
        // to "No matches found". `??` is null/undefined-only, so a JSON `null` also takes the
        // default.
        let limit = input
            .limit
            .map_or(self.opts.limit, crate::jsnum::to_count)
            .max(1);

        // Pi hands `glob` to ripgrep verbatim (grep.ts:218), so it parses as ONE gitignore-style
        // override line — anchored when it contains a `/`, basename-matched when it does not. That
        // is the opposite of the `**/`-prefix rule fd needs, which `find` uses
        // (find.ts:243-252 / [`PatternMatcher`]); wiring `grep` through fd's rule un-anchored every
        // path glob, so `glob: "src/**/*.ts"` also matched `vendor/src/a.ts`.
        let glob = match input.glob.as_deref() {
            Some(g) => RgGlob::build(g)?,
            None => None,
        };

        // `-E/--encoding` names a transcoding source (`Encoding::new`, grep-searcher 0.1.16
        // `searcher/mod.rs:137`). An unknown label is DROPPED rather than made fatal, for the same
        // reason an unknown flag is: a config written against a different build must not turn every
        // search into an error. Declaring a real encoding the files do not use and then finding
        // nothing is correct ripgrep behaviour, and is honoured as such.
        let encoding = rg
            .encoding
            .as_deref()
            .and_then(|label| Encoding::new(label).ok());

        // The config's own glob and type filters. Both are `None` unless the config actually named
        // one, so a caller with no config takes exactly the path it took before.
        //
        // These are the real `ignore` matchers rather than the hand-rolled single-line
        // [`RgGlob`]: a config can carry many `-g` lines whose verdicts interact (last match wins,
        // `!` re-includes), which is precisely what `Override` implements and what a per-line
        // matcher cannot express. `RgGlob` stays where it is — it models the ONE `glob` tool
        // argument pi forwards (grep.ts:218), which has different anchoring rules.
        //
        // The override root is `self.cwd`, matching ripgrep's: pi spawns `rg` with no `cwd` option
        // and passes the search path positionally, so a `path` argument narrows the walk without
        // re-anchoring the globs.
        let cfg_override = rgconfig::build_override(&self.cwd, &rg);
        let cfg_types = rgconfig::build_types(&rg);

        // `RgGlob` is not `Clone`, and both the prune predicate below and the per-file filter in
        // the walk loop need it, so it is shared by `Arc` rather than duplicated.
        let glob = glob.map(Arc::new);
        let cfg_override = cfg_override.map(Arc::new);

        // The walker-side replacement for the old `pruned: Vec<PathBuf>` post-hoc filter. That
        // filter was sound only because `ignore::Walk` is pre-order — a directory always arrived
        // before its contents — which a parallel walk does not guarantee. Registered through
        // `WalkBuilder::filter_entry` it is order-independent, and it is what ripgrep does.
        //
        // ripgrep evaluates the override for directories too (ignore `dir.rs:416-425`) and an
        // Ignore verdict takes the whole subtree; a plain (non-`!`) glob that merely MISSES does
        // not prune, because `Override::matched` guards its whitelist-miss fallback with `!is_dir`
        // (`overrides.rs:106`). `--type`/`--type-not` never prune: `Types::matched` returns `None`
        // for a directory, so a `-t rust` config must still descend into directories that do not
        // look like Rust to reach the `.rs` files inside them.
        let prune = (glob.is_some() || cfg_override.is_some()).then(|| {
            let glob = glob.clone();
            let cfg_override = cfg_override.clone();
            let cwd = self.cwd.clone();
            crate::ops::PruneDirs::new(move |dir: &std::path::Path| {
                // Same relativisation as the file filter below: the glob is anchored at the
                // OVERRIDE ROOT — ripgrep's own cwd — not at the search root, because pi spawns
                // `rg` with no `cwd` option and passes `searchPath` positionally (grep.ts:224).
                let rel = dir
                    .strip_prefix(&cwd)
                    .map_or_else(|_| to_posix(dir), to_posix);
                glob.as_ref().is_some_and(|g| g.prunes_dir(&rel))
                    || cfg_override
                        .as_ref()
                        .is_some_and(|o| o.matched(&rel, true).is_ignore())
            })
        });

        // ripgrep's two detection modes, one per candidate class (`hiargs.rs:1141-1157` with Pi's
        // flag set). The explicit one is `none()` and NOT `convert(b'\x00')` on purpose — see the
        // note in `search_one`.
        let binary_explicit = BinaryDetection::none();
        // `-a/--text` (and `--binary`) turn the implicit `quit` off, so a traversed file keeps
        // producing match events past its first NUL instead of ending as if at EOF. The explicit
        // mode is already `none()` for the reasons in `search_one`, so `-a` cannot change it.
        let binary_implicit = if rg.text {
            BinaryDetection::none()
        } else {
            BinaryDetection::quit(b'\x00')
        };

        let mut out: Vec<String> = Vec::new();
        let mut count = 0usize;
        let mut any_line_truncated = false;

        if meta.is_file {
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            let rel = search_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| to_posix(&search_root));
            // `meta.is_file` IS ripgrep's explicit rule: `Haystack::is_explicit()` is "depth 0 and
            // not a directory", i.e. a path handed to `rg` on the command line — which is what
            // Pi's `path` argument becomes (grep.ts:224). ripgrep never filters such a file out.
            let searched = self
                .search_batch(
                    vec![(search_root.clone(), rel)],
                    &matcher,
                    &cancel,
                    binary_explicit,
                    &rg,
                    encoding.as_ref(),
                    context,
                    limit,
                )
                .await?;
            for (_, blocks) in searched {
                take_into(&mut out, &mut count, &mut any_line_truncated, limit, blocks);
            }
        } else {
            // Pi runs plain `rg --hidden` with NO `--no-require-git` flag (grep.ts:215-219): search
            // dotfiles/dot-dirs, but honor `.gitignore` only *inside* a git repo — ripgrep's default,
            // which is `ignore`'s `require_git:true` (the crate does the in-repo detection internally).
            // Unlike Pi's `find` (fd `--no-require-git` outside a repo, find.ts:226-240), Pi's grep
            // never disables require-git, so outside any repo a stray `.gitignore` is NOT applied.
            // ripgrep does not abort a search on a traversal error, but it does REPORT one: it
            // prints `rg: {path}: {os error}` to stderr and exits 2 (verified, rg 14.1.0), and
            // pi's grep rejects on any code that is neither 0 nor 1 (grep.ts:309-313). The
            // rejection happens at CLOSE, though, after rg has walked the whole tree — and does
            // not happen at all if the match limit was hit first, because `stopChild(true)` sets
            // `killedDueToLimit` (grep.ts:240-245, :291-295) and that flag short-circuits the
            // guard. So the error is REMEMBERED here and decided after the loop; returning at the
            // first one would stop the walk and make the limit unreachable, failing calls pi
            // answers successfully. Which one is reported used to be "the first", on the ground
            // that rg's parallel walk does not order its stderr against ours — but OUR walk is
            // parallel now too, so arrival order is no longer stable on either side. They are
            // collected and the lexicographically smallest is taken; the message is
            // `{path}: {os error}`, so that is path order, and it is the same on every run.
            let mut walk = self.fs.walk(
                &search_root,
                WalkOpts {
                    include_hidden: true,
                    require_git: true,
                    // Pi's `grep` IS ripgrep (grep.ts:177 `ensureTool("rg")`, spawned at
                    // `:226`), invoked with no `--no-ignore`/`--no-ignore-dot`/`--ignore-file`
                    // (grep.ts:220-224), so ripgrep's full default ignore set is in force:
                    // `.rgignore` on top of `.ignore` and the gitignore family. ripgrep has no
                    // global ignore file, so unlike `find` nothing else attaches. This also
                    // keeps `find`'s `.fdignore` out of grep.
                    flavor: WalkFlavor::Rg,

                    // The user's `$RIPGREP_CONFIG_PATH`, reaching the walk. Pi gets all of this
                    // for free by spawning the real `rg`; cyrup has to carry it across.
                    no_ignore: rg.no_ignore,
                    no_ignore_vcs: rg.no_ignore_vcs,
                    max_depth: rg.max_depth,
                    follow_links: rg.follow,
                    one_file_system: rg.one_file_system,
                    max_filesize: rg.max_filesize,
                    // Resolved against the TOOL's cwd, not the process cwd: `add_ignore` takes the
                    // path as given, so a relative `--ignore-file` would otherwise be looked up
                    // wherever the host process happens to be — never the directory the caller
                    // named.
                    //
                    // Deliberately NOT `path::resolve_to_cwd`. That is pi's resolver for a
                    // user-supplied TOOL ARGUMENT (`paths.ts:80-93`) and carries semantics that are
                    // right there and wrong here: it strips a leading `@` unconditionally, so a
                    // config naming a real file called `@vendorignore` would silently open
                    // `vendorignore` instead — a different existing file, with nothing reporting
                    // the substitution. It also rewrites `file://` URLs and Windows shell paths,
                    // neither of which appears in a ripgrep config.
                    //
                    // Tilde expansion is the one piece kept, and it is a deliberate improvement
                    // rather than an oversight: real ripgrep never expands `~` here, because a
                    // config file is not shell-expanded, so `--ignore-file ~/.rgignore` simply
                    // fails to open for it. Expanding cannot open the WRONG file — it can only
                    // find one ripgrep would have missed.
                    ignore_files: rg
                        .ignore_files
                        .iter()
                        .map(|raw| {
                            let expanded = path::expand_home(raw);
                            let candidate = std::path::Path::new(&expanded);
                            if candidate.is_absolute() {
                                candidate.to_path_buf()
                            } else {
                                self.cwd.join(candidate)
                            }
                        })
                        .collect(),
                    sort_by_path: rg.sort_path,
                    prune,
                },
            );
            // ripgrep's own default and `WalkParallel`'s (ignore `walk.rs:1434-1440`): keep walk
            // and search at the same width so neither starves the other.
            // ripgrep's own default and `WalkParallel`'s (ignore `walk.rs:1434-1440`): keep walk
            // and search at the same width so neither starves the other.
            let concurrency = std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .min(12);
            // Candidates waiting to be dispatched as one batch. Batching is what removes the
            // per-file `spawn_blocking` round-trips — see [`Self::search_batch`].
            let mut pending: Vec<(PathBuf, String)> = Vec::new();
            // Batch size is derived from the OBSERVED match density, not ramped on a fixed
            // schedule, and that is a correctness property of the common case rather than a tuning
            // nicety. Over-dispatching costs real work: every file in a batch is opened and
            // searched even if the cap was already reachable without it. A `grep` for a frequent
            // pattern at the default `limit: 100` finishes inside the first file or two, so a
            // fixed ramp to 32 turned a ~1.6ms search into ~2.7ms — a regression on the single
            // most common call this tool gets.
            //
            // So: once anything has matched, estimate how many files are still needed to fill the
            // remaining budget (`remaining ÷ matches-per-file`), spread that across the in-flight
            // width, and batch that much. A dense pattern computes 1 and behaves exactly like the
            // unbatched shape; a sparse one computes thousands and clamps to `MAX_BATCH`, which is
            // where the hop amortisation lives. Before anything has matched there is no density to
            // measure and no budget being spent, so go straight to the maximum.
            const MAX_BATCH: usize = 32;
            let mut batch_size = 1usize;
            // Candidates handed to `search_batch`, the denominator of that density. Counted at
            // DISPATCH rather than completion, so it includes files still in flight; that
            // understates density slightly and errs toward larger batches, which is the safe
            // direction — the estimate only has to be right to an order of magnitude.
            let mut dispatched = 0usize;
            let mut inflight = FuturesUnordered::new();
            // Keyed by path so the render order is decided by the path comparator below, never by
            // completion order. `PathBuf` and not the rendered `rel`: `Path::cmp` compares
            // COMPONENTS, which is the only ordering stable across runs and platforms.
            let mut collected: Vec<(PathBuf, Vec<MatchBlock>)> = Vec::new();
            // EVERY walk error, not the first. `grep` used to keep the first on the ground that
            // "rg's parallel walk does not order it against ours, so the first is the only stable
            // choice" — but now OUR walk is parallel too, so "first" is no longer stable either.
            // The smallest is taken after the loop; the message is `{path}: {os error}`, so that
            // is path order. Small — one per unreadable directory.
            let mut walk_errors: Vec<String> = Vec::new();
            // Matches from COMPLETED batches only. No atomic and no lock: the dispatch loop is a
            // single async task and `FuturesUnordered` is polled from it, so this is plain
            // single-threaded state even though the searches themselves are not.
            let mut found = 0usize;
            let mut walk_done = false;

            loop {
                // Dispatch when a batch is full, or when the walk has ended and a tail remains.
                let batch_ready = pending.len() >= batch_size || (walk_done && !pending.is_empty());
                if batch_ready && found < limit && inflight.len() < concurrency {
                    let batch = std::mem::take(&mut pending);
                    dispatched += batch.len();
                    inflight.push(self.search_batch(
                        batch,
                        &matcher,
                        &cancel,
                        binary_implicit.clone(),
                        &rg,
                        encoding.as_ref(),
                        context,
                        limit,
                    ));
                    continue;
                }
                // TERMINATION, and it is load-bearing in a way the old `if count >= limit { break }`
                // was not. The cancel arm below is ALWAYS enabled, so a turn with nothing left to
                // do does NOT hit `select!`'s "all branches disabled" panic — it would park
                // forever on a token that may never fire. This condition is the only thing that
                // ends the loop, and it must be tested BEFORE the `select!`. Reaching the cap ends
                // it even with candidates still pending: those are exactly the files the cap says
                // we no longer need.
                if inflight.is_empty() && (found >= limit || (walk_done && pending.is_empty())) {
                    break;
                }
                // The cheap fast path, kept from the fused loop: skip opening the next candidate
                // at all. A cancel that lands mid-search is observed INSIDE `search_batch`, which
                // threads the token through `CancelReader`, `MatchSink`, and the between-files
                // check in the blocking task.
                if cancel.is_cancelled() {
                    return Err(error::aborted());
                }
                // The walk and the search are still FUSED to the cap, exactly as pi is: its line
                // handler sets `matchLimitReached` and calls `stopChild(true)` (grep.ts:292-295,
                // defined at `:240-245`) the instant `matchCount >= effectiveLimit`, killing the
                // rg child so upstream neither finishes the traversal nor reads another file.
                let can_pull = !walk_done
                    && found < limit
                    && pending.len() < batch_size
                    && inflight.len() < concurrency;

                tokio::select! {
                    // `biased;` — without it `select!` polls in RANDOM order, so with the token
                    // already cancelled AND a walk item already buffered the walk arm would win
                    // half the time and the tool would keep consuming entries after Esc: bounded
                    // in expectation, unbounded in the worst case. `find.rs` has carried this
                    // since its own loop was written; grep's did not, and this is where it lands.
                    biased;
                    _ = cancel.cancelled() => return Err(error::aborted()),

                    // Completions are drained ahead of new dispatch, which is what bounds
                    // `collected`'s growth rather than letting the walk run ahead of the searches.
                    //
                    // `Some(done)` is a PATTERN, and it is load-bearing: an EMPTY
                    // `FuturesUnordered` returns `Poll::Ready(None)` immediately, and `select!`
                    // DISABLES a branch whose pattern does not match, letting the walk arm park
                    // normally. Written as a bare `done = inflight.next()` this arm would instead
                    // complete instantly on every turn whenever nothing is in flight and spin the
                    // loop hot against the walker.
                    Some(done) = inflight.next() => {
                        // `?` still propagates `error::aborted()` out of `execute`. Dropping
                        // `inflight` on that path is enough: each `spawn_blocking` task owns its
                        // own `CancelToken` clone and stops itself through `CancelReader` /
                        // `MatchSink`.
                        let batch: Vec<(PathBuf, Vec<MatchBlock>)> = done?;
                        // `search_batch` already dropped every file that matched nothing, so each
                        // entry here contributes at least one row.
                        for (path, blocks) in batch {
                            found += blocks.len();
                            collected.push((path, blocks));
                        }
                        // Re-derive the batch size from what the search has actually seen. All
                        // integer: `need` is "files still required at the density observed so
                        // far", divided across the in-flight width. `found == 0` means nothing has
                        // matched yet, so no budget is being spent and there is nothing to
                        // over-dispatch against.
                        let estimate = if found == 0 {
                            MAX_BATCH
                        } else {
                            let remaining = limit.saturating_sub(found);
                            let need = remaining.saturating_mul(dispatched) / found;
                            (need / concurrency).clamp(1, MAX_BATCH)
                        };
                        // Bounded by a DOUBLING, never jumped to. The estimate is derived from a
                        // handful of files and is wildly noisy early: one sparse file among the
                        // first few predicts thousands still needed and sends the batch straight
                        // to `MAX_BATCH`, putting `concurrency × 32` files in flight for a cap
                        // that four more files would have filled. Measured, that cost the common
                        // capped search 1.9ms → 3.0ms.
                        //
                        // The two errors are not symmetric: over-dispatching opens and searches
                        // files whose results are then discarded, while under-dispatching only
                        // costs one more turn of a loop that is already running. So growth is
                        // geometric and the estimate acts purely as a CEILING on it — a dense
                        // pattern holds the batch at 1 and behaves exactly like the unbatched
                        // shape, a sparse one reaches `MAX_BATCH` within five completions, which
                        // on any tree worth batching is immediately.
                        batch_size = estimate.min(batch_size.saturating_mul(2)).max(1);
                    }

                    // Guarded, so a full pipeline stops consuming the walker and the bounded
                    // channel back-pressures into `WalkParallel`'s `blocking_send`.
                    item = walk.next(), if can_pull => {
                        match item {
                            Some(Ok(w)) => {
                                // ripgrep's subject filter. A traversal-discovered entry is
                                // searched only when its OWN file type is a regular file
                                // (`SubjectBuilder::build` → `Subject::is_file`); pi passes no
                                // `--follow`/`-L` (grep.ts:220-224), so a symlink inside the tree
                                // is not followed and not searched. `!w.is_dir` admitted symlinks
                                // — and FIFOs, sockets and device nodes — then handed the path to
                                // `read_stream`'s `File::open`, which has no `O_NOFOLLOW` and DOES
                                // follow, searching the target's bytes under the link's name.
                                //
                                // Directories are dropped here again, as they were before the
                                // consumer-side prune existed: the override prune now runs INSIDE
                                // the walker ([`WalkOpts::prune`]), so a pruned subtree never
                                // reaches this loop and a directory is neither a search subject
                                // nor a prunable root on this side any more.
                                //
                                // This filter is for WALK-discovered candidates only. A `path`
                                // argument that is a symlink to a file never reaches this loop:
                                // `FsOps::metadata` (fs.rs:170-182) stats through the link, so
                                // `meta.is_file` is true at the explicit-path branch above and the
                                // file is searched directly — which is what ripgrep does for a
                                // depth-0 explicit subject (`Subject::is_file` short-circuits on
                                // `dent.depth() == 0`).
                                if !w.is_file {
                                    continue;
                                }
                                {
                                    // The glob is matched against the path relative to the OVERRIDE
                                    // ROOT, which for ripgrep is its own cwd — Pi spawns `rg` with
                                    // no `cwd` option and passes `searchPath` positionally, so a
                                    // `path` argument narrows the walk but does NOT re-anchor the
                                    // glob (ignore-0.4.26 gitignore.rs:275-304 `strip`). A
                                    // candidate outside the root keeps its full path, as `strip`
                                    // leaves it.
                                    let glob_rel = w
                                        .path
                                        .strip_prefix(&self.cwd)
                                        .map_or_else(|_| to_posix(&w.path), to_posix);
                                    if let Some(g) = &glob
                                        && !g.keeps_file(&glob_rel)
                                    {
                                        continue;
                                    }
                                    if let Some(o) = &cfg_override
                                        && o.matched(&glob_rel, false).is_ignore()
                                    {
                                        continue;
                                    }
                                    if let Some(t) = &cfg_types
                                        && t.matched(&glob_rel, false).is_ignore()
                                    {
                                        continue;
                                    }
                                }
                                let rel_path = w.path.strip_prefix(&search_root).unwrap_or(&w.path);
                                let rel = to_posix(rel_path);
                                // Queued, not awaited, and not yet dispatched: the batch goes out
                                // when it is full. Traversal-discovered, so implicit binary
                                // detection — unchanged.
                                pending.push((w.path.clone(), rel));
                            }
                            // pi surfaces rg's stderr verbatim (`stderr.trim()`, grep.ts:310) and
                            // rg writes `rg: {path}: {os error}`. `LocalFs::walk` yields the bare
                            // `{path}: {os error}` because `find` must carry no prefix at all, so
                            // the `rg: ` half is added below, at the one consumer that emulates
                            // ripgrep.
                            Some(Err(e)) => walk_errors.push(e.message),
                            // NOT a `break`: batches are still in flight and a tail may still be
                            // pending. The termination test at the top of the loop ends it.
                            None => walk_done = true,
                        }
                    }
                }
            }
            // Dropping `walk` here is what makes every remaining walker thread observe a closed
            // receiver and return `WalkState::Quit` — the parallel-walk equivalent of pi's
            // `stopChild(true)`. On the `found >= limit` exit the walk is mid-tree, so this is the
            // path that stops it; letting `walk` live to the end of the scope would leave up to 12
            // threads walking a tree whose results are already discarded.
            drop(walk);

            // Rows are emitted in path order, never in completion order. pi cannot be copied here
            // — pi IS rg's parallel walk, so pi's own row order varies run to run — and a tool
            // result the model reads must not. `find` already resolves this the same way: bound
            // the walk at the cap, then sort the bounded set. fd does exactly that too
            // (`ReceiverBuffer` sorts while buffering, fd 10.5.0 `walk.rs:281-285`), and fd's walk
            // is parallel — so sorting a capped set is the upstream behaviour, not a divergence.
            //
            // This is NOT the full-tree `sort()`+`truncate()` that TOOL-033 removed. That version
            // drained the whole walk on EVERY call and then took the 100-match window from the
            // alphabetically-first files — a systematic `a*`-biased sample. Here the walk still
            // stops at the cap, so the SET is what discovery produced and only its ORDER moves.
            //
            // Determinism, precisely: the ORDER is always deterministic. The SET is deterministic
            // whenever the cap is not reached, which is every search returning fewer than `limit`
            // matches. When the cap IS reached the set depends on which files finished first —
            // which is exactly pi's behaviour, since `stopChild(true)` kills rg mid-parallel-walk.
            // The comparator follows the WALK's ordering contract, it is not unconditionally
            // ascending. `--sortr=path` shares no comparator with `--sort=path`, and with a match
            // cap in force the direction decides WHICH matches come back, not merely their order
            // — which is why that walk stays serial at all (`fs.rs`, the `sort_by_path` branch).
            // Re-sorting its output ascending here would throw away the very thing that branch
            // exists to preserve. Pinned by `sortr_reverses_which_match_the_cap_returns`.
            //
            // Trimming after this sort is still correct for a sorted walk: dispatch happens in
            // walk order and only stops once `found >= limit`, so every file ahead of the cutoff
            // in sort order was already dispatched.
            match rg.sort_path {
                Some(PathSort::Descending) => collected.sort_by(|(a, _), (b, _)| b.cmp(a)),
                Some(PathSort::Ascending) | None => collected.sort_by(|(a, _), (b, _)| a.cmp(b)),
            }
            for (_, blocks) in collected {
                take_into(&mut out, &mut count, &mut any_line_truncated, limit, blocks);
            }

            // pi: `if (!killedDueToLimit && code !== 0 && code !== 1) { reject(stderr.trim()); }`
            // (grep.ts:309-313). `count >= limit` IS `killedDueToLimit`, but note it is no longer
            // what BREAKS the loop: the pipeline stops dispatching on `found >= limit` (matches
            // from completed batches) and `count` is derived afterwards, by `take_into`, while
            // trimming the sorted blocks. The two agree exactly where it matters — the collected
            // blocks are a superset of the completed ones, so `found >= limit` guarantees
            // `count == limit` — which is why testing `count` here still means "the cap fired",
            // exactly as `stopChild(true)` kills the rg child upstream. `limit` is `.max(1)`
            // (grep.ts:189), so this cannot be vacuously true on entry.
            //
            // This check precedes the `out.is_empty()` reply below because pi's does — the
            // exit-code guard is at grep.ts:309 and the `matchCount === 0` reply at `:314` — so an
            // errored walk that found nothing reports the error rather than "No matches found".
            // It also precedes the successful formatting path, because rg exiting 2 makes pi
            // reject even when matches were found.
            if count < limit
                && let Some(message) = walk_errors.into_iter().min()
            {
                return Err(ToolError::new(format!("rg: {message}")));
            }
        }

        if out.is_empty() {
            return Ok(ToolResult {
                content: vec![Content::text("No matches found")],
                details: None,
                ..Default::default()
            });
        }

        let joined = out.join("\n");
        let t = truncate_head(&joined, TruncOpts::bytes_only(self.opts.max_bytes));

        // Pi joins every notice into ONE bracket (grep.ts:338-356), e.g.
        // `[100 matches limit reached. Use limit=200 for more, or refine pattern. 50.0KB limit
        //   reached. Some lines truncated to 500 chars. Use read tool to see full lines]`.
        let mut text = t.content.clone();
        let mut notices: Vec<String> = Vec::new();
        if count >= limit {
            notices.push(format!(
                "{limit} matches limit reached. Use limit={} for more, or refine pattern",
                limit.saturating_mul(2)
            ));
        }
        if t.info.truncated {
            // Pi hardcodes `formatSize(DEFAULT_MAX_BYTES)` (grep.ts:347).
            notices.push(format!(
                "{} limit reached",
                format_size(crate::truncate::DEFAULT_MAX_BYTES)
            ));
        }
        if any_line_truncated {
            notices.push(format!(
                "Some lines truncated to {GREP_MAX_LINE_LENGTH} chars. Use read tool to see full lines"
            ));
        }
        if !notices.is_empty() {
            text.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }

        // Pi only adds `truncation` to details when the byte cap actually fired, and emits
        // `details: undefined` when no key is set (grep.ts:337-360). Mirror that exactly.
        let match_limit_reached = if count >= limit { Some(limit) } else { None };
        let lines_truncated = if any_line_truncated { Some(true) } else { None };
        let truncation = if t.info.truncated { Some(t.info) } else { None };
        let details =
            if truncation.is_some() || match_limit_reached.is_some() || lines_truncated.is_some() {
                serde_json::to_value(crate::details::GrepDetails {
                    truncation,
                    match_limit_reached,
                    lines_truncated,
                })
                .ok()
            } else {
                None
            };

        Ok(ToolResult {
            content: vec![Content::text(text)],
            details,
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::{CaseMode, GrepInput, GrepTool, build_matcher};
    // `line_terminator()` lives on the `Matcher` trait, not on `RegexMatcher` itself.
    use crate::config::GrepOpts;
    use crate::ops::local::LocalFs;
    use crate::ops::{Access, DirEntry, FsOps, Meta, WalkItem, WalkOpts};
    use crate::tools::rgconfig::RgFlags;
    use cyrup_core::{CancelToken, Content, EventStream, Tool, ToolCallId, ToolError, ToolUpdate};
    use grep_matcher::Matcher;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// `LocalFs` that serves the first `read()` of any path normally and fails every later one.
    ///
    /// This models Pi's two-reader shape exactly: ripgrep matches the file from its own process
    /// while the format pass re-reads it through the injectable `ops.readFile` (grep.ts:200-213),
    /// so the format read can fail on a file that demonstrably matched — the file is unlinked, its
    /// mode is dropped to `0000`, or a custom `readFile` backend goes away between the two reads.
    struct FailSecondRead {
        inner: LocalFs,
        reads: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl FsOps for FailSecondRead {
        async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
            if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                self.inner.read(path).await
            } else {
                Err(ToolError::new("EACCES"))
            }
        }
        async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
            self.inner.write_in_place(path, bytes).await
        }
        async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError> {
            self.inner.access(path, mode).await
        }
        async fn metadata(&self, path: &Path) -> Result<Meta, ToolError> {
            self.inner.metadata(path).await
        }
        async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
            self.inner.read_dir(path).await
        }
        fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
            self.inner.walk(root, opts)
        }
    }

    /// Pi `formatBlock` (grep.ts:250-253): when `getFileLines` cannot produce lines it emits ONE
    /// `<path>:<line>: (unable to read file)` row per match in place of the context block, rather
    /// than dropping the file from the output. cyrup previously `continue`d on a read failure, so
    /// the file vanished silently and the marker string appeared nowhere in the workspace.
    #[tokio::test]
    async fn context_block_emits_unable_to_read_marker_when_reread_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::write(cwd.join("a.txt"), "one\nNEEDLE\nthree\n").unwrap();

        let fs = Arc::new(FailSecondRead {
            inner: LocalFs,
            reads: AtomicUsize::new(0),
        });
        let grep = GrepTool::new(fs, cwd.clone(), GrepOpts::default());
        let r = grep
            .execute(
                ToolCallId::from("tc-test"),
                serde_json::json!({ "pattern": "NEEDLE", "context": 1 }),
                CancelToken::new(),
                Box::new(|_u: ToolUpdate| {}),
            )
            .await
            .unwrap();

        let text = match r.content.first() {
            Some(Content::Text { text, .. }) => text.to_string(),
            _ => String::new(),
        };
        // Exactly Pi's row: match separator `:`, then the parenthesized marker. No context rows
        // (`a.txt-1-` / `a.txt-3-`) accompany it, because the block was replaced, not augmented.
        assert_eq!(text, "a.txt:2: (unable to read file)");
    }

    /// The context==0 path must NOT re-read: Pi formats it from ripgrep's own captured
    /// `data.lines.text` and never calls `getFileLines` (grep.ts:316-326), so a backend that would
    /// fail a second read still renders the real line.
    #[tokio::test]
    async fn zero_context_never_rereads_and_so_never_marks() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::write(cwd.join("a.txt"), "one\nNEEDLE\nthree\n").unwrap();

        let fs = Arc::new(FailSecondRead {
            inner: LocalFs,
            reads: AtomicUsize::new(0),
        });
        let grep = GrepTool::new(fs, cwd.clone(), GrepOpts::default());
        let r = grep
            .execute(
                ToolCallId::from("tc-test"),
                serde_json::json!({ "pattern": "NEEDLE" }),
                CancelToken::new(),
                Box::new(|_u: ToolUpdate| {}),
            )
            .await
            .unwrap();

        let text = match r.content.first() {
            Some(Content::Text { text, .. }) => text.to_string(),
            _ => String::new(),
        };
        assert_eq!(text, "a.txt:2: NEEDLE");
    }

    async fn grep_text(cwd: &Path, args: serde_json::Value) -> String {
        let grep = GrepTool::new(Arc::new(LocalFs), cwd.to_path_buf(), GrepOpts::default());
        let r = grep
            .execute(
                ToolCallId::from("tc-test"),
                args,
                CancelToken::new(),
                Box::new(|_u: ToolUpdate| {}),
            )
            .await
            .unwrap();
        match r.content.first() {
            Some(Content::Text { text, .. }) => text.to_string(),
            _ => String::new(),
        }
    }

    /// `glob` goes to ripgrep verbatim (grep.ts:218), so a pattern containing `/` is ANCHORED at
    /// the override root (ignore-0.4.33 gitignore.rs:513-522 prefixes `**/` only when the pattern
    /// has no `/`). `grep` used to compile the pattern with fd's opposite rule — the one `find`
    /// needs (find.ts:243-252) — which un-anchored it, so `src/**/*.ts` also matched
    /// `vendor/src/*.ts` and those extra hits crowd out real ones against the 100-match cap.
    #[tokio::test]
    async fn path_glob_is_anchored_at_the_override_root() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        std::fs::create_dir_all(cwd.join("vendor/src")).unwrap();
        std::fs::write(cwd.join("src/a.ts"), "NEEDLE\n").unwrap();
        std::fs::write(cwd.join("vendor/src/b.ts"), "NEEDLE\n").unwrap();

        let text = grep_text(
            &cwd,
            serde_json::json!({ "pattern": "NEEDLE", "glob": "src/**/*.ts" }),
        )
        .await;
        assert_eq!(text, "src/a.ts:1: NEEDLE");
    }

    /// gitignore.rs:492-498 strips a leading `/` and anchors on it. Passing it through to globset
    /// untouched matched it against a root-relative path that never starts with `/`, so the whole
    /// query silently returned "No matches found".
    #[tokio::test]
    async fn leading_slash_glob_anchors_instead_of_matching_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        std::fs::create_dir_all(cwd.join("vendor/src")).unwrap();
        std::fs::write(cwd.join("src/a.ts"), "NEEDLE\n").unwrap();
        std::fs::write(cwd.join("vendor/src/b.ts"), "NEEDLE\n").unwrap();

        let text = grep_text(
            &cwd,
            serde_json::json!({ "pattern": "NEEDLE", "glob": "/src/*.ts" }),
        )
        .await;
        assert_eq!(text, "src/a.ts:1: NEEDLE");
    }

    /// A `path` argument narrows the walk but does NOT re-anchor the glob: Pi spawns `rg` with no
    /// `cwd` option, so ripgrep's override root stays the agent's cwd and the glob is matched
    /// against the cwd-relative path (gitignore.rs:286-315 `strip`). Output paths remain relative
    /// to the search root, as `rg` prints what it was given.
    #[tokio::test]
    async fn glob_is_matched_against_the_cwd_relative_path_not_the_search_root() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        std::fs::write(cwd.join("src/a.ts"), "NEEDLE\n").unwrap();

        let text = grep_text(
            &cwd,
            serde_json::json!({ "pattern": "NEEDLE", "path": "src", "glob": "src/*.ts" }),
        )
        .await;
        assert_eq!(text, "a.ts:1: NEEDLE");
    }

    /// ripgrep picks binary detection PER HAYSTACK: a path the user NAMED is never filtered out
    /// (`hiargs.rs:1124-1157`, selected on `Haystack::is_explicit()`), while a traversal-discovered
    /// one still quits at the first NUL. cyrup shared ONE `quit(b'\x00')` searcher across both
    /// branches, so `{"pattern":"NEEDLE","path":"bin.dat"}` answered "No matches found" for a file
    /// whose text is plainly there — wrong, not merely truncated.
    ///
    /// The explicit mode is `none()`, not ripgrep's `convert(b'\x00')`: measured against rg 14.1.0,
    /// `convert` is inert under the memory map rg uses for exactly this case, and on the reader
    /// path cyrup always takes it rewrites each NUL to the line terminator and emits lines 1/3/4
    /// instead of 1/2/3. `none()` reproduces rg's observable output — same rows, same raw `\n`
    /// numbering, control bytes verbatim.
    #[tokio::test]
    async fn explicit_binary_path_is_searched_while_traversal_still_quits_at_the_nul() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        // rg 14.1.0, `rg --json --line-number --color=never --hidden -- NEEDLE bin.dat`:
        //   line 1 "hello NEEDLE\n" / line 2 "\u0000\u0001\u0002binary NEEDLE\n" / line 3 "tail NEEDLE\n"
        std::fs::write(
            cwd.join("bin.dat"),
            b"hello NEEDLE\n\x00\x01\x02binary NEEDLE\ntail NEEDLE\n".as_slice(),
        )
        .unwrap();
        std::fs::write(cwd.join("plain.txt"), "plain NEEDLE\n").unwrap();

        let text = grep_text(
            &cwd,
            serde_json::json!({ "pattern": "NEEDLE", "path": "bin.dat" }),
        )
        .await;
        let want = concat!(
            "bin.dat:1: hello NEEDLE\n",
            "bin.dat:2: \u{0}\u{1}\u{2}binary NEEDLE\n",
            "bin.dat:3: tail NEEDLE",
        );
        assert_eq!(text, want, "explicitly named file must be searched to EOF");

        // The SAME file reached by traversal contributes nothing — not even the pre-NUL line 1 —
        // and the text file beside it is unaffected.
        let text = grep_text(&cwd, serde_json::json!({ "pattern": "NEEDLE" })).await;
        assert_eq!(text, "plain.txt:1: plain NEEDLE", "got: {text}");
    }

    /// A bare pattern (no `/`) is basename-matched at any depth in BOTH rules — the schema's own
    /// first example (`'*.ts'`, grep.rs description) must keep working.
    #[tokio::test]
    async fn bare_glob_still_matches_basenames_at_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::create_dir_all(cwd.join("src/deep")).unwrap();
        std::fs::write(cwd.join("src/deep/a.ts"), "NEEDLE\n").unwrap();
        std::fs::write(cwd.join("b.js"), "NEEDLE\n").unwrap();

        let text = grep_text(
            &cwd,
            serde_json::json!({ "pattern": "NEEDLE", "glob": "*.ts" }),
        )
        .await;
        assert_eq!(text, "src/deep/a.ts:1: NEEDLE");
    }

    /// Build the tree the pruning contract is stated over: three files holding NEEDLE, one of them
    /// under a `src` nested inside `vendor` so an unanchored directory glob can be seen to reach it.
    fn prune_tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        std::fs::create_dir_all(cwd.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        std::fs::create_dir_all(cwd.join("vendor/src")).unwrap();
        std::fs::write(cwd.join("node_modules/pkg/a.js"), "NEEDLE\n").unwrap();
        std::fs::write(cwd.join("src/b.js"), "NEEDLE\n").unwrap();
        std::fs::write(cwd.join("vendor/src/c.js"), "NEEDLE\n").unwrap();
        dir
    }

    /// `grep_text` with the rows sorted: the walk is not ordered, and neither is ripgrep's.
    async fn grep_rows(cwd: &Path, args: serde_json::Value) -> Vec<String> {
        let text = grep_text(cwd, args).await;
        if text == "No matches found" {
            return Vec::new();
        }
        let mut rows: Vec<String> = text.lines().map(str::to_string).collect();
        rows.sort();
        rows
    }

    /// TOOL — a negated glob naming a DIRECTORY must remove the whole subtree, which is what
    /// ripgrep does: the override is evaluated for directories too (dir.rs:401-425) and an Ignore
    /// verdict reaches `skip_current_dir()` (walk.rs:1131-1145). The port of `Override::matched`
    /// dropped `is_dir`, so `!node_modules` compiled to `**/node_modules` — a pattern that with
    /// `literal_separator(true)` never matches `node_modules/pkg/a.js` — and every file in the
    /// subtree was KEPT.
    ///
    /// Verified against rg 14.1.0:
    /// `rg --json --hidden --glob '!node_modules' -- NEEDLE .` → `./src/b.js`, `./vendor/src/c.js`.
    #[tokio::test]
    async fn negated_directory_glob_prunes_the_whole_subtree() {
        let dir = prune_tree();
        let rows = grep_rows(
            dir.path(),
            serde_json::json!({ "pattern": "NEEDLE", "glob": "!node_modules" }),
        )
        .await;
        assert_eq!(
            rows,
            vec!["src/b.js:1: NEEDLE", "vendor/src/c.js:1: NEEDLE"]
        );
    }

    /// A trailing `/` is DIRECTORY-ONLY, not "matches nothing": `is_only_dir` removes the glob from
    /// consideration unless the candidate is a directory (gitignore.rs:262), so `!src/` prunes
    /// every `src` directory — the unanchored `**/src` reaches `vendor/src` too — and no file.
    /// With `only_dir` used in one direction only, this pattern was a complete no-op.
    ///
    /// Verified against rg 14.1.0:
    /// `rg --json --hidden --glob '!src/' -- NEEDLE .` → `./node_modules/pkg/a.js` alone.
    #[tokio::test]
    async fn trailing_slash_glob_prunes_directories_at_any_depth() {
        let dir = prune_tree();
        let rows = grep_rows(
            dir.path(),
            serde_json::json!({ "pattern": "NEEDLE", "glob": "!src/" }),
        )
        .await;
        assert_eq!(rows, vec!["node_modules/pkg/a.js:1: NEEDLE"]);
    }

    /// The other half of the rule, and the one a "prune whatever the glob does not match" shortcut
    /// would break: a PLAIN glob that misses a directory returns `Match::None`, never Ignore —
    /// `Override::matched` guards its whitelist-miss fallback with `!is_dir` (overrides.rs:106) —
    /// so rg still descends and finds the files inside. An anchored path glob keeps its anchor.
    ///
    /// Verified against rg 14.1.0: `--glob '*.js'` → all three files; `--glob 'src/**/*.js'` →
    /// `./src/b.js` only. The no-glob walk is unchanged and returns all three.
    #[tokio::test]
    async fn plain_glob_miss_never_prunes_a_directory() {
        let dir = prune_tree();
        let all = vec![
            "node_modules/pkg/a.js:1: NEEDLE",
            "src/b.js:1: NEEDLE",
            "vendor/src/c.js:1: NEEDLE",
        ];

        let rows = grep_rows(
            dir.path(),
            serde_json::json!({ "pattern": "NEEDLE", "glob": "*.js" }),
        )
        .await;
        assert_eq!(
            rows, all,
            "a directory is not a `*.js` candidate, yet it must be descended"
        );

        let rows = grep_rows(
            dir.path(),
            serde_json::json!({ "pattern": "NEEDLE", "glob": "src/**/*.js" }),
        )
        .await;
        assert_eq!(rows, vec!["src/b.js:1: NEEDLE"]);

        let rows = grep_rows(dir.path(), serde_json::json!({ "pattern": "NEEDLE" })).await;
        assert_eq!(rows, all, "the no-glob walk is byte-for-byte what it was");
    }

    /// `skip_entry` returns `Ok(false)` unconditionally at `ent.depth() == 0` (walk.rs:1057-1060):
    /// the search root itself is never pruned, whatever the glob says. Nor is an explicitly named
    /// file, which never reaches the walk at all.
    ///
    /// Verified against rg 14.1.0: both
    /// `rg --glob '!node_modules' -- NEEDLE node_modules` and
    /// `rg --glob '!node_modules' -- NEEDLE node_modules/pkg/a.js` print the match.
    #[tokio::test]
    async fn the_search_root_and_an_explicit_file_are_never_pruned() {
        let dir = prune_tree();
        let rows = grep_rows(
            dir.path(),
            serde_json::json!({
                "pattern": "NEEDLE", "path": "node_modules", "glob": "!node_modules"
            }),
        )
        .await;
        assert_eq!(rows, vec!["pkg/a.js:1: NEEDLE"]);

        let rows = grep_rows(
            dir.path(),
            serde_json::json!({
                "pattern": "NEEDLE", "path": "node_modules/pkg/a.js", "glob": "!node_modules"
            }),
        )
        .await;
        assert_eq!(rows, vec!["a.js:1: NEEDLE"]);
    }

    /// The point of pruning rather than post-filtering: the walk and the search are FUSED and both
    /// stop at the match cap, so an excluded subtree that holds more than `limit` matches used to
    /// spend the entire 100-match window before the walk ever reached the file the caller wanted.
    /// Pruning it means the cap is spent where ripgrep spends it.
    #[tokio::test]
    async fn a_pruned_subtree_does_not_consume_the_match_cap() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        std::fs::create_dir_all(cwd.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        for i in 0..150 {
            std::fs::write(cwd.join(format!("node_modules/pkg/f{i}.js")), "NEEDLE\n").unwrap();
        }
        std::fs::write(cwd.join("src/b.js"), "NEEDLE\n").unwrap();

        let rows = grep_rows(
            cwd,
            serde_json::json!({ "pattern": "NEEDLE", "glob": "!node_modules" }),
        )
        .await;
        assert_eq!(rows, vec!["src/b.js:1: NEEDLE"]);
    }

    /// The four-line block `rg` writes to stderr for a pattern that can match a line terminator,
    /// captured from ripgrep 14.1.0 driven with Pi's own argv
    /// (`rg --json --line-number --color=never --hidden -- $'a\nimport' file.txt`, exit 2). Pi
    /// rejects with `stderr.trim()` (grep.ts:309-312) and nothing catches it, so this string IS the
    /// model-observed tool error.
    const RG_NEWLINE_ERROR: &str = concat!(
        "rg: the literal \"\\n\" is not allowed in a regex\n",
        "\n",
        "Consider enabling multiline mode with the --multiline flag (or -U for short).\n",
        "When multiline mode is enabled, new line characters can be matched.",
    );

    async fn grep_err(cwd: &Path, args: serde_json::Value) -> String {
        let grep = GrepTool::new(Arc::new(LocalFs), cwd.to_path_buf(), GrepOpts::default());
        let r = grep
            .execute(
                ToolCallId::from("tc-test"),
                args,
                CancelToken::new(),
                Box::new(|_u: ToolUpdate| {}),
            )
            .await;
        r.expect_err("pattern should have been refused at matcher build")
            .message
    }

    fn newline_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // The audit's fixture: `a` alone on line 1, `import c` on line 2, so a pattern spanning the
        // break would "obviously" match if line-oriented search allowed it.
        std::fs::write(dir.path().join("file.txt"), "a\nimport c\n").unwrap();
        dir
    }

    /// A REAL `\n` byte in the pattern. Without `line_terminator(Some(b'\n'))` grep-regex's
    /// `strip_from_match` guard is off entirely (`config.rs:222-225`), the matcher builds, and the
    /// line-oriented searcher then reports "No matches found" — a confident false negative where rg
    /// exits 2 with an actionable error.
    #[tokio::test]
    async fn real_newline_in_pattern_is_refused_with_ripgreps_message() {
        let dir = newline_fixture();
        let msg = grep_err(dir.path(), serde_json::json!({ "pattern": "a\nimport" })).await;
        assert_eq!(msg, RG_NEWLINE_ERROR);
    }

    /// The two-character regex escape parses to the same `\n` literal in the HIR, so it is refused
    /// identically — rg 14.1.0 exits 2 for `-- 'a\nimport'` too.
    #[tokio::test]
    async fn newline_escape_in_pattern_is_refused_with_ripgreps_message() {
        let dir = newline_fixture();
        let msg = grep_err(dir.path(), serde_json::json!({ "pattern": "a\\nimport" })).await;
        assert_eq!(msg, RG_NEWLINE_ERROR);
    }

    /// `--fixed-strings` does not exempt a real newline: `Config::is_fixed_strings`
    /// (grep-regex `config.rs:113-124`) bails out of the literal-alternation fast path when the
    /// pattern contains the terminator, routing it through the parse path where the guard runs.
    #[tokio::test]
    async fn real_newline_is_refused_under_fixed_strings_too() {
        let dir = newline_fixture();
        let msg = grep_err(
            dir.path(),
            serde_json::json!({ "pattern": "a\nimport", "literal": true }),
        )
        .await;
        assert_eq!(msg, RG_NEWLINE_ERROR);
    }

    /// The other half of that asymmetry, and the reason `fixed_strings` needs no special casing:
    /// under `--fixed-strings` the two-character escape is an ordinary backslash-`n` literal, holds
    /// no `\n` byte, and rg exits 1 with no match rather than erroring.
    #[tokio::test]
    async fn newline_escape_under_fixed_strings_is_an_ordinary_literal() {
        let dir = newline_fixture();
        let text = grep_text(
            dir.path(),
            serde_json::json!({ "pattern": "a\\nimport", "literal": true }),
        )
        .await;
        assert_eq!(text, "No matches found");
    }

    /// Only `\n` is banned — the terminator is a single byte and `crlf` is not enabled — so a
    /// pattern carrying `\r` still builds. rg 14.1.0 exits 1 here.
    #[tokio::test]
    async fn carriage_return_in_pattern_is_not_banned() {
        let dir = newline_fixture();
        let text = grep_text(dir.path(), serde_json::json!({ "pattern": "a\\rb" })).await;
        assert_eq!(text, "No matches found");
    }

    /// `\n` inside a CLASS is subtracted transparently rather than refused (`strip.rs`), so `a\sb`
    /// keeps building and keeps matching within a line — it simply cannot span a line break.
    #[tokio::test]
    async fn newline_inside_a_class_is_stripped_not_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::write(cwd.join("same.txt"), "a b\n").unwrap();
        std::fs::write(cwd.join("split.txt"), "a\nb\n").unwrap();

        let text = grep_text(&cwd, serde_json::json!({ "pattern": "a\\sb" })).await;
        assert_eq!(text, "same.txt:1: a b");
    }

    /// A syntax error keeps ripgrep's own `regex parse error:` caret block — grep-regex wraps every
    /// pattern in `(?:…)` (`config.rs:183-188`), which is why rg prints `(?:a()` — and gains only
    /// the `rg: ` prefix. The old `grep: invalid pattern:` wrapper has no counterpart in Pi and
    /// must appear nowhere.
    #[tokio::test]
    async fn syntax_error_keeps_ripgreps_caret_block_under_the_rg_prefix() {
        let dir = newline_fixture();
        let msg = grep_err(dir.path(), serde_json::json!({ "pattern": "a(" })).await;
        // Captured from rg 14.1.0: `rg: regex parse error:\n    (?:a()\n    ^\nerror: unclosed group`.
        assert_eq!(
            msg,
            "rg: regex parse error:\n    (?:a()\n    ^\nerror: unclosed group"
        );
        assert!(!msg.contains("grep: invalid pattern:"), "got: {msg}");
        // And the multiline hint is gated on the message text, so it stays off here.
        assert!(!msg.contains("--multiline"), "got: {msg}");
    }

    /// `FsOps` that records any traversal or read, proving the refusal is returned from the matcher
    /// build — before the first `walk`/`read_stream` — so no file in the tree is ever opened.
    struct RecordingFs {
        inner: LocalFs,
        touched: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl FsOps for RecordingFs {
        // `read_stream`'s default impl funnels through `read` (ops/mod.rs:412-414), so this one
        // override covers the search path too.
        async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
            self.touched.store(true, Ordering::SeqCst);
            self.inner.read(path).await
        }
        async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
            self.inner.write_in_place(path, bytes).await
        }
        async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError> {
            self.inner.access(path, mode).await
        }
        async fn metadata(&self, path: &Path) -> Result<Meta, ToolError> {
            self.inner.metadata(path).await
        }
        async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
            self.inner.read_dir(path).await
        }
        fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
            self.touched.store(true, Ordering::SeqCst);
            self.inner.walk(root, opts)
        }
    }

    #[tokio::test]
    async fn refusal_precedes_any_traversal_or_read() {
        let dir = newline_fixture();
        let touched = Arc::new(AtomicBool::new(false));
        let grep = GrepTool::new(
            Arc::new(RecordingFs {
                inner: LocalFs,
                touched: Arc::clone(&touched),
            }),
            dir.path().to_path_buf(),
            GrepOpts::default(),
        );
        let r = grep
            .execute(
                ToolCallId::from("tc-test"),
                serde_json::json!({ "pattern": "a\nimport" }),
                CancelToken::new(),
                Box::new(|_u: ToolUpdate| {}),
            )
            .await;
        assert_eq!(
            r.err().map(|e| e.message).as_deref(),
            Some(RG_NEWLINE_ERROR)
        );
        assert!(
            !touched.load(Ordering::SeqCst),
            "matcher refusal must precede the first walk/read"
        );
    }

    /// `foo$` — the plainest thing a caller writes, and the one the matcher's `(?m)` flag exists
    /// for. Verified against ripgrep 14.1.0 driven with Pi's own argv on this exact fixture
    /// (`rg --json --line-number --color=never --hidden -- 'foo$' .` reports a.txt:2 and a.txt:3).
    #[tokio::test]
    async fn dollar_anchors_at_the_end_of_every_line() {
        let dir = anchor_fixture();
        let rows = grep_rows(dir.path(), serde_json::json!({ "pattern": "foo$" })).await;
        assert_eq!(rows, vec!["a.txt:2: foo", "a.txt:3: bar foo"]);
    }

    /// The `^` half. rg 14.1.0 on this fixture: a.txt:2 and a.txt:4.
    #[tokio::test]
    async fn caret_anchors_at_the_start_of_every_line() {
        let dir = anchor_fixture();
        let rows = grep_rows(dir.path(), serde_json::json!({ "pattern": "^foo" })).await;
        assert_eq!(rows, vec!["a.txt:2: foo", "a.txt:4: foobar"]);
    }

    /// An anchor-only pattern with no extractable literal, over an empty line — the case that
    /// separates "anchored to the haystack" from "anchored to the line" most visibly. rg 14.1.0
    /// reports every non-empty line for `.$` and only the empty one for `^$`.
    #[tokio::test]
    async fn anchors_without_literals_stay_line_relative() {
        let dir = anchor_fixture();
        let dollar = grep_rows(dir.path(), serde_json::json!({ "pattern": ".$" })).await;
        assert_eq!(
            dollar,
            vec![
                "a.txt:1: alpha",
                "a.txt:2: foo",
                "a.txt:3: bar foo",
                "a.txt:4: foobar",
                "e.txt:1: x",
                "e.txt:3: y",
            ]
        );
        let empty = grep_rows(dir.path(), serde_json::json!({ "pattern": "^$" })).await;
        assert_eq!(empty, vec!["e.txt:2: "]);
    }

    fn anchor_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nfoo\nbar foo\nfoobar\n").unwrap();
        std::fs::write(dir.path().join("e.txt"), "x\n\ny\n").unwrap();
        dir
    }

    /// The four-line block `rg` writes to stderr for a pattern that must match a NUL, captured
    /// from ripgrep 14.1.0 driven with Pi's own argv (`rg --json --line-number --color=never
    /// --hidden -- '\x00' .`, exit 2). Same rejection path as [`RG_NEWLINE_ERROR`].
    const RG_NUL_ERROR: &str = concat!(
        "rg: pattern contains \"\\0\" but it is impossible to match\n",
        "\n",
        "Consider enabling text mode with the --text flag (or -a for short). Otherwise,\n",
        "binary detection is enabled and matching a NUL byte is impossible.",
    );

    /// Every spelling of a NUL that survives into the HIR is refused at build time. Without
    /// `ban_byte` the pattern compiled and then reported "No matches found", because binary
    /// detection had already ended the file at the first NUL.
    #[tokio::test]
    async fn nul_in_pattern_is_refused_with_ripgreps_message() {
        let dir = anchor_fixture();
        for pattern in ["\\x00", "[\\x00]", "\\x{0}"] {
            let msg = grep_err(dir.path(), serde_json::json!({ "pattern": pattern })).await;
            assert_eq!(msg, RG_NUL_ERROR, "pattern {pattern}");

            // The same refusal under `ignoreCase`, which the original assertion never covered.
            //
            // The note that asked for this explained it as `case_insensitive` forcing grep-regex
            // off the `is_fixed_strings` fast path. That reasoning is wrong and is recorded here so
            // it is not re-derived: `fixed_strings` is not set on this call, so the fast path was
            // never in play, and setting it would not help either — under `literal` the pattern is
            // `regex_syntax::escape`d BEFORE parsing, so `\x00` becomes ordinary backslash-x-0-0
            // holding no NUL for `ban::check` to find (pinned by
            // `nul_escape_under_fixed_strings_is_an_ordinary_literal`).
            //
            // What it does pin is real: `-i` changes the HIR translation path (case folding is
            // applied during translation), and a NUL must not survive that. rg 14.1.0 exits 2 for
            // `-i -- '\x00'`.
            let msg = grep_err(
                dir.path(),
                serde_json::json!({ "pattern": pattern, "ignoreCase": true }),
            )
            .await;
            assert_eq!(msg, RG_NUL_ERROR, "pattern {pattern} under ignoreCase");
        }
    }

    /// ITEM 7 — a RAW NUL byte, which pi cannot deliver at all.
    ///
    /// `literal: true` took the fixed-strings branch, which `ban::check` never sees, so the pattern
    /// compiled and the tool answered "No matches found" — a confident false negative — while
    /// `literal: false` correctly errored. Both branches now refuse it with the same bytes.
    #[tokio::test]
    async fn a_raw_nul_byte_is_refused_on_both_branches() {
        let dir = anchor_fixture();
        for literal in [false, true] {
            let msg = grep_err(
                dir.path(),
                serde_json::json!({ "pattern": "a\u{0}b", "literal": literal }),
            )
            .await;
            assert_eq!(msg, RG_NUL_ERROR, "literal {literal}");
        }
    }

    /// ITEM 5 — the `.multi_line(true)` guard, driving the PRODUCTION construction.
    ///
    /// With `multi_line` on, `^`/`$` translate to the LINE anchors and `ConfiguredHIR` reports the
    /// line terminator; with it off they become haystack anchors and `line_terminator()` answers
    /// `None` (grep-regex `config.rs:296-302`). That is an exact discriminator, and it is only
    /// usable because `build_matcher` is a function the test can call — asserting against a copy of
    /// the builder would pass no matter what `execute` did.
    #[test]
    fn multi_line_is_set_on_the_production_matcher() {
        let flags = RgFlags::default();
        let input = |pattern: &str| GrepInput {
            pattern: pattern.to_string(),
            path: None,
            glob: None,
            ignore_case: None,
            literal: None,
            context: None,
            limit: None,
        };
        for anchored in ["foo$", "^foo", "^$"] {
            let m = build_matcher(&input(anchored), CaseMode::Sensitive, &flags).unwrap();
            assert_eq!(
                m.line_terminator(),
                Some(grep_matcher::LineTerminator::byte(b'\n')),
                "{anchored} must stay line-anchored"
            );
        }
    }

    /// `ban::check` only refuses a class of exactly ONE element (grep-regex `src/ban.rs:19-37`),
    /// so widening it past the NUL builds and simply does not match. rg 14.1.0 exits 1 here.
    #[tokio::test]
    async fn nul_inside_a_wider_class_is_not_banned() {
        let dir = anchor_fixture();
        let text = grep_text(
            dir.path(),
            serde_json::json!({ "pattern": "a[\\x00-\\x01]" }),
        )
        .await;
        assert_eq!(text, "No matches found");
    }

    /// Under `literal: true` the pattern is `regex_syntax::escape`d (grep-regex
    /// `src/config.rs:184-185`), so `\x00` is four ordinary characters holding no NUL and the ban
    /// never sees one. rg 14.1.0 exits 1 for `-F -- '\x00'` too.
    #[tokio::test]
    async fn nul_escape_under_fixed_strings_is_an_ordinary_literal() {
        let dir = anchor_fixture();
        let text = grep_text(
            dir.path(),
            serde_json::json!({ "pattern": "\\x00", "literal": true }),
        )
        .await;
        assert_eq!(text, "No matches found");
    }

    /// The third hint filter. Pi runs an official ripgrep build, which is compiled with
    /// `--features pcre2`, so a Rust-engine refusal of a look-around or a backreference carries
    /// ripgrep's PCRE2 suggestion. Both blocks captured from rg 14.1.0, exit 2.
    #[tokio::test]
    async fn lookaround_and_backreferences_carry_ripgreps_pcre2_hint() {
        let dir = anchor_fixture();
        let hint = concat!(
            "\n\nConsider enabling PCRE2 with the --pcre2 flag, which can handle backreferences\n",
            "and look-around.",
        );

        let msg = grep_err(dir.path(), serde_json::json!({ "pattern": "(?=foo)" })).await;
        assert_eq!(
            msg,
            format!(
                concat!(
                    "rg: regex parse error:\n",
                    "    (?:(?=foo))\n",
                    "       ^^^\n",
                    "error: look-around, including look-ahead and look-behind, is not supported",
                    "{}"
                ),
                hint
            )
        );

        let msg = grep_err(dir.path(), serde_json::json!({ "pattern": "\\0" })).await;
        assert_eq!(
            msg,
            format!(
                concat!(
                    "rg: regex parse error:\n",
                    "    (?:\\0)\n",
                    "       ^^\n",
                    "error: backreferences are not supported",
                    "{}"
                ),
                hint
            )
        );
    }

    // ---------------------------------------------------------------------------------------
    // $RIPGREP_CONFIG_PATH
    //
    // Pi's grep spawns the real `rg` with no `env` key and no `--no-config` (grep.ts:226), so the
    // child inherits the variable and applies the user's config on every search. cyrup searches
    // in-process, so none of that happens unless it is done deliberately.
    //
    // Every test below drives the config through `GrepOpts::rg_config_path` rather than through
    // `std::env::set_var`: mutating the process environment is UB under edition 2024 (the setter
    // is `unsafe` for exactly that reason) and would race every other test in the binary.
    // ---------------------------------------------------------------------------------------

    /// Run `grep` against `cwd` with `config` (if any) as the user's ripgrep config file.
    async fn grep_with_config(
        cwd: &Path,
        config: Option<&Path>,
        args: serde_json::Value,
    ) -> String {
        let opts = GrepOpts {
            rg_config_path: config.map(std::path::Path::to_path_buf),
            ..GrepOpts::default()
        };
        let grep = GrepTool::new(Arc::new(LocalFs), cwd.to_path_buf(), opts);
        let r = grep
            .execute(
                ToolCallId::from("tc-test"),
                args,
                CancelToken::new(),
                Box::new(|_u: ToolUpdate| {}),
            )
            .await
            .unwrap();
        match r.content.first() {
            Some(Content::Text { text, .. }) => text.to_string(),
            _ => String::new(),
        }
    }

    /// A tree with one lowercase and one uppercase occurrence, plus a config file.
    fn tree_with_config(config: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "NEEDLE\n").unwrap();
        std::fs::write(dir.path().join("rc"), config).unwrap();
        dir
    }

    /// THE CANARY. Before the config was read, this returned only the exact-case hit.
    ///
    /// `--smart-case` makes an all-lowercase pattern case-insensitive, so `needle` must now also
    /// find `NEEDLE`. Delete the `.case_smart(…)` call from the matcher and this fails — which is
    /// the point: it proves the config reaches `RegexMatcherBuilder` and changes the MATCH SET,
    /// not merely that a file was parsed.
    #[tokio::test]
    async fn smart_case_in_config_changes_the_match_set() {
        let dir = tree_with_config("--smart-case\n");
        let rc = dir.path().join("rc");

        let without =
            grep_with_config(dir.path(), None, serde_json::json!({"pattern": "needle"})).await;
        assert!(
            without.contains("a.txt"),
            "baseline should match the lowercase file: {without}"
        );
        assert!(
            !without.contains("b.txt"),
            "without the config the search is case-SENSITIVE: {without}"
        );

        let with = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            with.contains("a.txt") && with.contains("b.txt"),
            "--smart-case must fold the uppercase file in: {with}"
        );
    }

    /// `-q` is parsed and then DROPPED. See [`RgFlags::QUIET_IS_REFUSED`].
    ///
    /// Honouring it would reproduce pi's own defect: `-q` short-circuits ripgrep's printer to
    /// `Summary(Quiet)` before the JSON arm is reached (`flags/hiargs.rs:565` @14.1.0), so rg exits
    /// 0 emitting no `type:"match"` events and pi answers `No matches found` with matches present.
    /// Parity is the floor, not the target.
    #[tokio::test]
    async fn quiet_in_config_is_ignored_and_matches_are_still_returned() {
        assert!(
            RgFlags::QUIET_IS_REFUSED.contains("hiargs.rs:565"),
            "the refusal must stay cited to the ripgrep line that causes it"
        );
        let dir = tree_with_config("-q\n");
        let rc = dir.path().join("rc");
        let out = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            out.contains("a.txt"),
            "-q must not silence the result set: {out}"
        );
        assert!(!out.contains("No matches found"), "{out}");
    }

    /// `--engine`, `--pre` and `--pre-glob` are recognised, declined, and CONSUME THEIR VALUE.
    ///
    /// `parse` advances past the flag itself, but only a match arm that calls `take()` advances
    /// past the value. A value-taking flag left on the catch-all therefore hands its argument back
    /// to the top of the loop, where a leading `-` makes it parse as a flag in its own right — so
    /// a config written in ripgrep's canonical one-argument-per-line form silently applied its
    /// NEXT line. See [`RgFlags::PCRE2_IS_DECLINED`] and [`RgFlags::PREPROCESSOR_IS_DECLINED`].
    #[test]
    fn declined_value_taking_flags_consume_their_value() {
        assert!(
            RgFlags::PCRE2_IS_DECLINED.contains("grep-pcre2"),
            "the refusal must stay cited to the crate cyrup does not link"
        );
        assert!(
            RgFlags::PREPROCESSOR_IS_DECLINED.contains("ops/mod.rs:437"),
            "the refusal must stay cited to the seam that has no exec"
        );
        assert!(
            RgFlags::PREPROCESSOR_IS_DECLINED.contains("--no-pre"),
            "the negations belong to the group the const explains"
        );

        // Each of these silently applied its second line before the flags were recognised:
        // case-insensitive, inverted, and literal respectively.
        assert_eq!(RgFlags::parse("--engine\n-i\n"), RgFlags::default());
        assert_eq!(RgFlags::parse("--pre\n-v\n"), RgFlags::default());
        assert_eq!(RgFlags::parse("--pre-glob\n-F\n"), RgFlags::default());

        // The inline form has no trailing line to leak, and the switches take no value at all.
        assert_eq!(RgFlags::parse("--engine=pcre2\n"), RgFlags::default());
        assert_eq!(
            RgFlags::parse(
                "-P\n-z\n--pcre2\n--no-pcre2\n--pre\n--no-pre\n--search-zip\n--no-search-zip\n"
            ),
            RgFlags::default()
        );
        // `--no-pre` is a SWITCH (`Pre::update` asserts there is no affirmative switch for
        // `--pre`, defs.rs:5431-5435), so it must not call `take()`. If it ever did, it would
        // swallow the following line and this would read `None`.
        assert_eq!(
            RgFlags::parse("--no-pre\n-i\n").case,
            Some(CaseMode::Insensitive)
        );
        assert_eq!(
            RgFlags::parse("--auto-hybrid-regex\n--no-auto-hybrid-regex\n"),
            RgFlags::default()
        );

        // Not passing for the wrong reason: the leaked lines are live flags on their own.
        assert_eq!(RgFlags::parse("-i\n").case, Some(CaseMode::Insensitive));
        assert!(RgFlags::parse("-v\n").invert_match);
        assert!(RgFlags::parse("-F\n").fixed_strings);
    }

    /// Every ripgrep 14.1.0 flag that TAKES a value consumes it, in both spellings.
    ///
    /// `parse` advances past the flag but only a `take()` arm advances past the value, so a
    /// value-taking flag left on the catch-all handed its argument back to the top of the loop —
    /// where a leading `-` made it parse as a flag and apply. 20 long forms and 9 short forms were
    /// in that state. `-d` was the worst of them: `--max-depth` is honoured, `-d` was not, so the
    /// two spellings of one flag produced different result sets.
    #[test]
    fn every_value_taking_flag_consumes_its_value() {
        // Long forms.
        assert_eq!(RgFlags::parse("--replace\n-i\n"), RgFlags::default());
        assert_eq!(RgFlags::parse("--max-columns\n-v\n"), RgFlags::default());
        assert_eq!(RgFlags::parse("--context\n-F\n"), RgFlags::default());
        // Short forms — a separate list, so separately guarded.
        assert_eq!(RgFlags::parse("-r\n-i\n"), RgFlags::default());
        assert_eq!(RgFlags::parse("-M\n-v\n"), RgFlags::default());
        assert_eq!(RgFlags::parse("-C\n-F\n"), RgFlags::default());

        // `-d` is HONOURED, not dropped: the two spellings of `--max-depth` must agree, in both
        // the next-line and cluster-tail forms.
        assert_eq!(RgFlags::parse("-d\n2\n").max_depth, Some(2));
        assert_eq!(RgFlags::parse("-d2\n").max_depth, Some(2));
        assert_eq!(RgFlags::parse("--max-depth\n2\n").max_depth, Some(2));

        // Not passing for the wrong reason — the leaked lines are live flags on their own.
        assert_eq!(RgFlags::parse("-i\n").case, Some(CaseMode::Insensitive));
        assert!(RgFlags::parse("-v\n").invert_match);
        assert!(RgFlags::parse("-F\n").fixed_strings);

        // The catch-all still IGNORES a flag from a newer ripgrep rather than failing. Its value
        // can still leak — the arity of an unknown flag is unknowable here — and that is the
        // deliberate limit of this fix, not an oversight.
        assert_eq!(RgFlags::parse("--some-future-flag\n"), RgFlags::default());
    }

    /// All four unicode names write the same engine-independent bool, last occurrence winning.
    ///
    /// `NoPcre2Unicode::update` is one line — `args.no_unicode = v.unwrap_switch();`
    /// (`defs.rs:4711-4714`) — and `--no-pcre2-unicode` / `--pcre2-unicode` are DEPRECATED aliases
    /// of `--no-unicode` / `--unicode` (`defs.rs:4692-4707`). Nothing about them is pcre2-gated,
    /// so the Rust engine honours all four; ripgrep proves the cross pairs at `defs.rs:4851-4860`.
    #[test]
    fn all_four_unicode_names_write_the_same_flag() {
        assert!(RgFlags::parse("--no-unicode\n").no_unicode);
        assert!(RgFlags::parse("--no-pcre2-unicode\n").no_unicode);
        // The cross pairs ripgrep tests by name — the later occurrence wins, both directions.
        assert!(!RgFlags::parse("--no-unicode\n--pcre2-unicode\n").no_unicode);
        assert!(!RgFlags::parse("--no-pcre2-unicode\n--unicode\n").no_unicode);
        assert!(RgFlags::parse("--unicode\n--no-pcre2-unicode\n").no_unicode);
    }

    /// The config layer sits UNDER the caller's arguments — ripgrep's own `config_args ++ cli_args`
    /// precedence, where the later occurrence wins (`flags/parse.rs`).
    #[tokio::test]
    async fn caller_argument_beats_config() {
        let dir = tree_with_config("--ignore-case\n");
        let rc = dir.path().join("rc");

        // Config alone: case-insensitive, so both files match.
        let cfg_only = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            cfg_only.contains("b.txt"),
            "config -i should fold in NEEDLE: {cfg_only}"
        );

        // The caller says otherwise, and the caller wins.
        let overridden = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle", "ignoreCase": false}),
        )
        .await;
        assert!(
            overridden.contains("a.txt") && !overridden.contains("b.txt"),
            "an explicit ignoreCase:false must override the config's -i: {overridden}"
        );
    }

    /// An unrecognised flag is IGNORED, and the flags around it still apply.
    ///
    /// ripgrep would exit 2 with `unrecognized flag`. Reproducing that would pin cyrup to one
    /// ripgrep version — a user on a newer rg with a newer flag would get an error instead of a
    /// search — and cyrup is not a ripgrep CLI. The neighbouring `--smart-case` still landing is
    /// what makes this a real assertion rather than "nothing crashed".
    #[tokio::test]
    async fn unknown_flag_is_ignored_not_fatal() {
        let dir = tree_with_config("--this-flag-does-not-exist\n--smart-case\n");
        let rc = dir.path().join("rc");
        let out = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            out.contains("a.txt") && out.contains("b.txt"),
            "the unknown flag must be skipped and --smart-case must still apply: {out}"
        );
    }

    /// A missing or unreadable config is not an error.
    ///
    /// ripgrep only warns on stderr and searches anyway, and pi discards rg's stderr on a zero
    /// exit — so upstream this is invisible. A hard failure here would break every search for a
    /// user whose `$RIPGREP_CONFIG_PATH` points at a file they deleted.
    #[tokio::test]
    async fn missing_config_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
        let absent = dir.path().join("no-such-file");

        let out = grep_with_config(
            dir.path(),
            Some(&absent),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            out.contains("a.txt"),
            "a missing config must not fail the search: {out}"
        );

        // A directory stands in for the unreadable case: `read_to_string` fails on it on every
        // platform, with no need to depend on running as a non-root user.
        let out = grep_with_config(
            dir.path(),
            Some(dir.path()),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            out.contains("a.txt"),
            "an unreadable config must not fail the search: {out}"
        );
    }

    /// Comments, blank lines and a value on its own line — ripgrep's config FORMAT, not a shell
    /// grammar (`flags/config.rs` @14.1.0). `-g` prunes the excluded directory out of the walk.
    #[tokio::test]
    async fn config_globs_filter_the_walk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("vendor")).unwrap();
        std::fs::write(dir.path().join("keep.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("vendor/skip.txt"), "needle\n").unwrap();
        std::fs::write(
            dir.path().join("rc"),
            "# a comment line is ignored\n\n--glob\n!vendor/**\n",
        )
        .unwrap();
        let rc = dir.path().join("rc");

        let without =
            grep_with_config(dir.path(), None, serde_json::json!({"pattern": "needle"})).await;
        assert!(
            without.contains("keep.txt") && without.contains("skip.txt"),
            "baseline should reach both files: {without}"
        );

        let with = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(with.contains("keep.txt"), "{with}");
        assert!(
            !with.contains("skip.txt"),
            "the config's !vendor/** must prune the directory: {with}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // QA rework guards. Each of the three below was a flag §6.1 lists as HONOUR that did not
    // actually work; each test fails without its fix.
    // ---------------------------------------------------------------------------------------

    /// Same as [`grep_with_config`] but for the refusal path.
    async fn grep_err_with_config(cwd: &Path, config: &Path, args: serde_json::Value) -> String {
        let opts = GrepOpts {
            rg_config_path: Some(config.to_path_buf()),
            ..GrepOpts::default()
        };
        let grep = GrepTool::new(Arc::new(LocalFs), cwd.to_path_buf(), opts);
        grep.execute(
            ToolCallId::from("tc-test"),
            args,
            CancelToken::new(),
            Box::new(|_u: ToolUpdate| {}),
        )
        .await
        .unwrap_err()
        .to_string()
    }

    /// An overflowing `--max-filesize` is dropped, not propagated.
    ///
    /// `18446744073709551615K` parses as `u64::MAX` and then multiplies by 1024. Unchecked that
    /// PANICS under the workspace dev profile and WRAPS in release into a tiny cap that silently
    /// drops files from the search — the worse of the two, because nothing in the output says so.
    /// The module's contract is that a bad config never makes the tool fail, so the value is
    /// discarded like any other unparseable one.
    #[tokio::test]
    async fn overflowing_max_filesize_is_dropped_not_a_panic() {
        assert_eq!(
            RgFlags::parse("--max-filesize\n18446744073709551615K\n").max_filesize,
            None,
            "an overflowing size must be dropped, not wrapped"
        );
        // A sane value still resolves, so the guard above is not passing for the wrong reason.
        assert_eq!(
            RgFlags::parse("--max-filesize\n2K\n").max_filesize,
            Some(2048)
        );

        let dir = tree_with_config("--max-filesize\n18446744073709551615K\n");
        let rc = dir.path().join("rc");
        let out = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            out.contains("a.txt"),
            "the search must still run with the bad value dropped: {out}"
        );
    }

    /// `--crlf` must leave the matcher internally consistent about what ends a line.
    ///
    /// `crlf(true)` sets BOTH `config.crlf` and a CRLF line terminator (grep-regex
    /// `matcher.rs:322-328`); `line_terminator` sets only the latter. With `line_terminator`
    /// called last it overwrote the CRLF terminator, so `^`/`$` used the CRLF anchors while
    /// `strip_from_match` and `has_line_terminator` still treated `\r` as an ordinary byte.
    ///
    /// The observable is the existing line-terminator guard, extended: under `--crlf`, `\r` IS a
    /// line terminator and a pattern demanding one must be refused exactly as `\n` already is.
    /// Without the fix the pattern builds and searches instead.
    #[tokio::test]
    async fn crlf_in_config_makes_carriage_return_a_line_terminator() {
        let dir = tree_with_config("--crlf\n");
        let rc = dir.path().join("rc");

        let msg =
            grep_err_with_config(dir.path(), &rc, serde_json::json!({"pattern": "a\\rb"})).await;
        assert!(
            msg.starts_with(r#"rg: the literal "\r" is not allowed in a regex"#),
            "under --crlf, \\r is a line terminator and must be refused exactly as \\n is: {msg}"
        );

        // Without the flag the same pattern is ordinary and simply finds nothing, which is what
        // makes the assertion above about `--crlf` rather than about the pattern.
        let out = grep_with_config(dir.path(), None, serde_json::json!({"pattern": "a\\rb"})).await;
        assert!(out.contains("No matches found"), "{out}");
    }

    /// `--sortr=path` descends. It shares no comparator with `--sort=path`.
    ///
    /// This is result-visible rather than cosmetic: `grep` is fused to its match cap, so with
    /// `limit: 1` the traversal direction decides WHICH match the caller receives. Collapsing both
    /// flags onto one ascending sort returned the opposite file.
    #[tokio::test]
    async fn sortr_reverses_which_match_the_cap_returns() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("z.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("asc"), "--sort\npath\n").unwrap();
        std::fs::write(dir.path().join("desc"), "--sortr\npath\n").unwrap();

        let args = serde_json::json!({"pattern": "needle", "limit": 1, "glob": "*.txt"});
        let asc = grep_with_config(dir.path(), Some(&dir.path().join("asc")), args.clone()).await;
        let desc = grep_with_config(dir.path(), Some(&dir.path().join("desc")), args).await;

        assert!(
            asc.contains("a.txt") && !asc.contains("z.txt"),
            "--sort=path: {asc}"
        );
        assert!(
            desc.contains("z.txt") && !desc.contains("a.txt"),
            "--sortr=path: {desc}"
        );
    }

    /// `-i`, `-s` and `-S` are one group, and the LAST one written wins.
    ///
    /// As three independent fields, `-S` followed by `-s` kept smart-case on and the later `-s`
    /// did nothing.
    #[tokio::test]
    async fn last_case_flag_in_config_wins() {
        let dir = tree_with_config("-S\n-s\n");
        let rc = dir.path().join("rc");
        let out = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            out.contains("a.txt") && !out.contains("b.txt"),
            "-s came last, so the search is case-sensitive: {out}"
        );

        // The same two flags the other way round, to prove the assertion tracks ORDER and not
        // merely the presence of `-s`.
        let dir = tree_with_config("-s\n-S\n");
        let rc = dir.path().join("rc");
        let out = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            out.contains("a.txt") && out.contains("b.txt"),
            "-S came last, so smart-case folds NEEDLE in: {out}"
        );
    }

    /// A relative `--ignore-file` resolves against the TOOL's cwd, not the process cwd.
    ///
    /// `WalkBuilder::add_ignore` takes the path verbatim, so an unresolved relative path was looked
    /// up wherever the host process happened to be — never the directory the caller named — and
    /// the ignore file silently did nothing.
    #[tokio::test]
    async fn relative_ignore_file_resolves_against_the_tools_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("skip.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("myignore"), "skip.txt\n").unwrap();
        std::fs::write(dir.path().join("rc"), "--ignore-file\nmyignore\n").unwrap();
        let rc = dir.path().join("rc");

        let out = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(out.contains("keep.txt"), "{out}");
        assert!(
            !out.contains("skip.txt"),
            "the relative ignore file must be found and applied: {out}"
        );
    }

    /// An `--ignore-file` whose filename really does begin with `@` must be opened, not rewritten.
    ///
    /// `path::resolve_to_cwd` is pi's resolver for a user-supplied tool ARGUMENT and strips a
    /// leading `@` unconditionally (`paths.ts:80-93`). Pointed at a config value it silently
    /// substituted a different existing file. The decoy below is what makes this a real assertion:
    /// the two candidate files exclude OPPOSITE things, so reading the wrong one is visible in the
    /// result rather than merely absent from it.
    #[tokio::test]
    async fn ignore_file_named_with_a_leading_at_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("skip.txt"), "needle\n").unwrap();
        // The file the config actually names.
        std::fs::write(dir.path().join("@vendorignore"), "skip.txt\n").unwrap();
        // The file an `@`-stripping resolver would reach for instead — it excludes the other one.
        std::fs::write(dir.path().join("vendorignore"), "keep.txt\n").unwrap();
        std::fs::write(dir.path().join("rc"), "--ignore-file\n@vendorignore\n").unwrap();
        let rc = dir.path().join("rc");

        let out = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            out.contains("keep.txt"),
            "the @-named ignore file excludes skip.txt, so keep.txt must survive: {out}"
        );
        assert!(
            !out.contains("skip.txt"),
            "the file the config named must actually be applied: {out}"
        );
    }

    /// One malformed `--type-add` must not disable the type filtering that IS valid.
    ///
    /// `TypesBuilder::build` fails with `unrecognized file type` if any selection names a type that
    /// does not exist, and that failure took the whole matcher down — so a dropped `--type-add`
    /// plus a `-t` for the name it tried to define also discarded the `-t rust` beside it, and the
    /// search silently stopped filtering by type at all.
    #[tokio::test]
    async fn a_malformed_type_add_does_not_disable_valid_type_filters() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("top.rs"), "needle\n").unwrap();
        std::fs::write(dir.path().join("top.txt"), "needle\n").unwrap();
        std::fs::write(
            dir.path().join("rc"),
            // `thisIsMalformed` has no `:`, so `add_def` rejects it and the type is never defined.
            "--type-add\nthisIsMalformed\n--type\nthisIsMalformed\n--type\nrust\n",
        )
        .unwrap();
        let rc = dir.path().join("rc");

        let out = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            out.contains("top.rs"),
            "the valid -t rust must still select: {out}"
        );
        assert!(
            !out.contains("top.txt"),
            "type filtering must survive the malformed --type-add: {out}"
        );
    }

    /// `--no-ignore` must clear EVERY ignore source, including the strongest one.
    ///
    /// `ignore(false)` drops `.ignore` and the git switches drop the gitignore family, but the
    /// custom ignore file (`.rgignore` for the rg flavor) is a separate source — and by
    /// `dir.rs:580-585` it outranks all of them. Leaving it registered meant `--no-ignore` changed
    /// nothing whatsoever in any tree that had one.
    ///
    /// The fixture carries all three kinds deliberately: with only `.gitignore` and `.ignore` this
    /// test passes with or without the fix and proves nothing.
    #[tokio::test]
    async fn no_ignore_in_config_clears_every_ignore_source() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.path().join(f), "needle\n").unwrap();
        }
        // A real `.git` directory, because `grep` walks with `require_git: true` — ripgrep's
        // default. Outside a repository a stray `.gitignore` is deliberately NOT applied, so
        // without this the `.gitignore` arm of the fixture would be inert and the test would be
        // asserting over two sources while claiming three.
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "a.txt\n").unwrap();
        std::fs::write(dir.path().join(".ignore"), "b.txt\n").unwrap();
        std::fs::write(dir.path().join(".rgignore"), "c.txt\n").unwrap();
        std::fs::write(dir.path().join("rc"), "--no-ignore\n").unwrap();
        let rc = dir.path().join("rc");

        // Baseline: each of the three files is hidden by a different ignore source, so a search
        // with no config finds none of them. This is what makes the assertion below meaningful.
        let without =
            grep_with_config(dir.path(), None, serde_json::json!({"pattern": "needle"})).await;
        for f in ["a.txt", "b.txt", "c.txt"] {
            assert!(
                !without.contains(f),
                "{f} should be ignored by default: {without}"
            );
        }

        let with = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        for f in ["a.txt", "b.txt", "c.txt"] {
            assert!(with.contains(f), "--no-ignore must surface {f}: {with}");
        }
    }

    /// `--no-ignore-vcs` is the NARROWER switch and must stay that way.
    ///
    /// It implies neither `no_ignore_dot` nor `no_ignore_parent`, so `.ignore` and `.rgignore` keep
    /// applying while only the gitignore family is dropped. Folding the two flags together would
    /// have made this indistinguishable from `--no-ignore`.
    #[tokio::test]
    async fn no_ignore_vcs_leaves_the_non_git_sources_in_force() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.path().join(f), "needle\n").unwrap();
        }
        // A real `.git` directory, because `grep` walks with `require_git: true` — ripgrep's
        // default. Outside a repository a stray `.gitignore` is deliberately NOT applied, so
        // without this the `.gitignore` arm of the fixture would be inert and the test would be
        // asserting over two sources while claiming three.
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "a.txt\n").unwrap();
        std::fs::write(dir.path().join(".ignore"), "b.txt\n").unwrap();
        std::fs::write(dir.path().join(".rgignore"), "c.txt\n").unwrap();
        std::fs::write(dir.path().join("rc"), "--no-ignore-vcs\n").unwrap();
        let rc = dir.path().join("rc");

        let out = grep_with_config(
            dir.path(),
            Some(&rc),
            serde_json::json!({"pattern": "needle"}),
        )
        .await;
        assert!(
            out.contains("a.txt"),
            "the gitignore'd file must surface: {out}"
        );
        assert!(!out.contains("b.txt"), ".ignore must still apply: {out}");
        assert!(!out.contains("c.txt"), ".rgignore must still apply: {out}");
    }
}
