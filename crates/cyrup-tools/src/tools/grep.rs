//! `grep` — in-process ripgrep-parity search (R-03-029…031, arch-03 §6.6). [CYRUP-DELTA]: uses the
//! `ignore`/`grep` crates instead of an external `rg` binary; output format, gitignore semantics,
//! and truncation preserve Pi's observable behavior.

use crate::config::GrepOpts;
use crate::ops::cancel_read::{CancelReader, Cancelled};
use crate::ops::{FsOps, WalkFlavor, WalkOpts};
use crate::tools::globmatch::{RgGlob, to_posix};
use crate::truncate::{GREP_MAX_LINE_LENGTH, TruncOpts, format_size, truncate_head, truncate_line};
use crate::{error, path};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use futures::StreamExt;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
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
///    (`hiargs.rs:1412-1462`). `suggest_other_engine` is `cfg!(feature = "pcre2")`-gated in
///    ripgrep, and it IS reachable from Pi: Pi downloads the official `BurntSushi/ripgrep` release
///    asset (tools-manager.ts:50-70), which release CI builds with `--features pcre2`
///    (`.github/workflows/release.yml`, `cargo build --release --features pcre2`).
/// 3. ripgrep prefixes every top-level error with `rg: ` (`crates/core/messages.rs:50`, reached
///    from `eprintln_locked!("{:#}", err)` at `crates/core/main.rs:62`).
fn rg_pattern_error(err: &grep_regex::Error) -> String {
    let msg = suggest_other_engine(suggest_text(suggest_multiline(err.to_string())));
    format!("rg: {msg}")
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
    #[allow(clippy::too_many_arguments)]
    async fn search_one(
        &self,
        file: &std::path::Path,
        rel: &str,
        matcher: &grep_regex::RegexMatcher,
        // Observed at BOTH `await` points below and, via `CancelReader` and `MatchSink`, inside
        // the blocking search itself. `Err(error::aborted())` here propagates through the `?` at
        // each call site and out of `execute`, matching Pi's `Operation aborted` rejection.
        cancel: &CancelToken,
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
        // Pi never materializes the candidate in the agent process: the search runs in a separate
        // ripgrep child (`spawn(rgPath, args, …)`, grep.ts:226) with rg's own bounded read buffer,
        // so file size is decoupled from the agent's heap. cyrup's search is in-process (the
        // declared `ignore`/`grep-searcher` delta, module doc above), so the same property comes
        // from [`FsOps::read_stream`] + `search_reader`. A full `FsOps::read` here allocated the
        // whole file BEFORE binary detection could reject it — one multi-GB log, database dump or
        // vendored tarball in the tree was an RSS spike or an OOM kill of the session, on a file
        // that need not match at all. A read failure is skipped, exactly as before: rg simply
        // emits no match events for a file it cannot open.
        //
        // A cancel landing while the open is in flight must not wait for the open to finish — a
        // remote/RPC `FsOps` can park here for a long time. `run_until_cancelled` returns `None`
        // immediately when the token is ALREADY cancelled (tokio-util 0.7.18
        // `sync/cancellation_token.rs:280-293`), so this doubles as the entry guard, and otherwise
        // drops the open future the moment the token fires.
        //
        // `None` is an abort; a `Some(Err(_))` is still the pre-existing skip: rg simply emits no
        // match events for a file it cannot open.
        let Some(opened) = cancel.run_until_cancelled(self.fs.read_stream(file)).await else {
            return Err(error::aborted());
        };
        let Ok(reader) = opened else {
            return Ok(());
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
        // line numbers stay the file's raw `\n` numbering. cyrup always uses `search_reader`
        // (`FsOps::read_stream` above), the branch where `convert` DOES fire and rewrites every
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
        // The token is moved INTO the blocking task rather than polled from outside it: a
        // `spawn_blocking` task owns an OS thread and cannot be aborted by dropping its
        // `JoinHandle`, so the only way out of `search_reader` is for the work itself to fail.
        // Cloning is an `Arc` refcount bump (`CancelToken` is `tokio_util::sync::CancellationToken`,
        // cyrup-core `cancel.rs:9`).
        let cancel_task = cancel.clone();
        let searched: Result<Vec<(u64, Vec<u8>)>, Aborted> =
            tokio::task::spawn_blocking(move || {
                let mut searcher: Searcher = SearcherBuilder::new()
                    .line_number(true)
                    .binary_detection(binary)
                    .build();
                let mut matches: Vec<(u64, Vec<u8>)> = Vec::new();
                let mut local = 0usize;
                let outcome = {
                    let sink = MatchSink {
                        matches: &mut matches,
                        count: &mut local,
                        limit: remaining,
                        cancel: cancel_task.clone(),
                    };
                    searcher.search_reader(
                        &matcher_owned,
                        CancelReader::new(reader, cancel_task),
                        sink,
                    )
                };
                match outcome {
                    // The searcher's error is no longer thrown away wholesale: a cancel marker is an
                    // abort, and EVERY other `io::Error` keeps the previous `let _ = …` semantics —
                    // whatever was collected before the failure stands and the walk moves on, because
                    // rg emits no match events for a file it cannot read.
                    Err(e) if Cancelled::is(&e) => Err(Aborted),
                    _ => Ok(matches),
                }
            })
            .await
            .map_err(|e| error::invalid(format!("grep: {e}")))?;

        let Ok(matches) = searched else {
            return Err(error::aborted());
        };

        if matches.is_empty() {
            return Ok(());
        }
        *count += matches.len();

        // Which matches take Pi's `formatBlock` path (grep.ts:333-335) rather than the direct
        // `match.lineText` path (grep.ts:323-331)? Pi's condition is
        // `contextValue === 0 && match.lineText !== undefined`, and `lineText` is
        // `event.data.lines.text` — ABSENT whenever ripgrep could not encode the line as UTF-8
        // (it serialises `lines.bytes` instead). So: context>0, or a non-UTF-8 matched line.
        let takes_block = |raw: &[u8]| context > 0 || std::str::from_utf8(raw).is_err();
        // `formatBlock` reads through `getFileLines`, a **second, independent** read of the file
        // (grep.ts:206-218) — ripgrep's own read was a different process against the real
        // filesystem — cached for the rest of the invocation by `fileCache`. Do it at most once
        // per file, and ONLY if some match actually needs it, so that at context==0 the file is
        // never re-read (Pi does not) yet the read-your-latest-writes semantics and the failure
        // path below stay reachable exactly where Pi has them. Pi's own `fileCache` is moot here:
        // each candidate is now visited exactly once.
        //
        // `None` is Pi's `catch { lines = [] }` (grep.ts:212-214). A successfully-read EMPTY
        // file is NOT that case — `"".split("\n")` is `[""]`, length 1. This one IS a whole-file
        // read on both sides, and pi pays it too — but only for a file that ACTUALLY MATCHED and
        // only on the `contextValue > 0` / non-UTF-8 path.
        let src_lines: Option<Vec<String>> = if matches.iter().any(|(_, r)| takes_block(r)) {
            // This is a WHOLE-FILE read of a file that already matched — on a multi-hundred-MB
            // candidate it is the second place a cancel could be stranded, so it is raced too.
            // `Err(_)` stays Pi's `catch { lines = [] }` (grep.ts:212-214); only a `None` — the
            // token firing — aborts.
            let Some(read) = cancel.run_until_cancelled(self.fs.read(file)).await else {
                return Err(error::aborted());
            };
            match read {
                // Pi `getFileLines` folds `\r\n`→`\n` AND lone `\r`→`\n` BEFORE splitting
                // (grep.ts:211). The matcher numbered lines on raw `\n`, so a file using
                // lone-`\r` separators yields context blocks that key off these folded segments
                // — matching Pi even where that diverges from the matcher's numbering.
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
        for (ln, raw) in &matches {
            let l = *ln as usize;
            if !takes_block(raw) {
                // Pi grep.ts:325-331: format straight from the captured line text —
                // `\r\n`→`\n`, then DROP every remaining `\r` (not fold it to `\n`, which is
                // what `getFileLines` does), then strip ONE trailing `\n`.
                let stripped = String::from_utf8_lossy(raw)
                    .replace("\r\n", "\n")
                    .replace('\r', "");
                let text = stripped.strip_suffix('\n').unwrap_or(&stripped);
                let (capped, tr) = truncate_line(text, GREP_MAX_LINE_LENGTH);
                if tr {
                    *any_line_truncated = true;
                }
                out.push(format!("{rel}:{l}: {capped}"));
                continue;
            }
            // Pi `formatBlock`: an unreadable file collapses the whole block to ONE marker row
            // per match (grep.ts:258). The rows still count as output, so they participate in
            // byte truncation like any other line.
            let Some(src_lines) = src_lines.as_ref() else {
                out.push(format!("{rel}:{l}: (unable to read file)"));
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
                    *any_line_truncated = true;
                }
                if current == l {
                    out.push(format!("{rel}:{current}: {capped}"));
                } else {
                    out.push(format!("{rel}-{current}- {capped}"));
                }
            }
        }
        Ok(())
    }
}

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
    /// The SECOND abort hook, covering the match-DENSE case [`CancelReader`] cannot: one 64 KiB
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
        let matcher = RegexMatcherBuilder::new()
            .multi_line(true)
            .case_insensitive(input.ignore_case.unwrap_or(false))
            .fixed_strings(input.literal.unwrap_or(false))
            .line_terminator(Some(b'\n'))
            .ban_byte(Some(b'\x00'))
            .build(&input.pattern)
            .map_err(|e| error::invalid(rg_pattern_error(&e)))?;

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

        // ripgrep's two detection modes, one per candidate class (`hiargs.rs:1141-1157` with Pi's
        // flag set). The explicit one is `none()` and NOT `convert(b'\x00')` on purpose — see the
        // note in `search_one`.
        let binary_explicit = BinaryDetection::none();
        let binary_implicit = BinaryDetection::quit(b'\x00');

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
            self.search_one(
                &search_root,
                &rel,
                &matcher,
                &cancel,
                binary_explicit,
                context,
                limit,
                &mut count,
                &mut out,
                &mut any_line_truncated,
            )
            .await?;
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
            // answers successfully. First error only — pi reports rg's whole stderr, but rg's
            // parallel walk does not order it against ours, so the first is the only stable
            // choice.
            let mut walk_error: Option<ToolError> = None;
            // Directory roots the override PRUNED. `ignore::Walk` prunes internally with
            // `skip_current_dir()` on a private iterator and exposes no skip handle to a consumer
            // (walk.rs:1131-1145), so the prune is reproduced here: the walk is pre-order, so a
            // directory always arrives before its contents, and every later item beneath a pruned
            // root is dropped. Same paths searched, same paths excluded, same match cap.
            let mut pruned: Vec<PathBuf> = Vec::new();
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
                },
            );
            loop {
                // The walk and the search are FUSED, and both stop at the limit. Pi's line handler
                // sets `matchLimitReached` and calls `stopChild(true)` (grep.ts:292-295, defined at
                // `:240-245`) the instant `matchCount >= effectiveLimit`, which kills the rg child
                // — so upstream neither finishes the traversal nor reads another file. Staging the
                // whole walk into a `Vec`, sorting it, and only then searching cost a complete
                // tree walk on EVERY call regardless of `limit`, and took the 100-match window
                // from the alphabetically-first files rather than from the first matches
                // discovered — a systematic `a*`-biased sample with nothing in the output to
                // distinguish it. `grep -n sort grep.ts` is empty at v0.84.1, so the sort is
                // dropped rather than moved.
                if count >= limit {
                    break;
                }
                // The `select!` below observes a cancel while parked on `walk.next()`; one that
                // lands mid-candidate is observed INSIDE `search_one`, which threads the token
                // through `CancelReader` and `MatchSink` into the blocking searcher. This check is
                // the cheap fast path: it skips opening the next candidate at all.
                if cancel.is_cancelled() {
                    return Err(error::aborted());
                }
                tokio::select! {
                    _ = cancel.cancelled() => return Err(error::aborted()),
                    item = walk.next() => {
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
                                // Directories are deliberately NOT filtered out here any more:
                                // the override is evaluated for them below, and that is what
                                // prunes a subtree. Everything that is neither a regular file nor
                                // a directory is dropped up front, since it is neither a search
                                // subject nor a prunable root.
                                //
                                // This filter is for WALK-discovered candidates only. A `path`
                                // argument that is a symlink to a file never reaches this loop:
                                // `FsOps::metadata` (fs.rs:170-182) stats through the link, so
                                // `meta.is_file` is true at the explicit-path branch above and the
                                // file is searched directly — which is what ripgrep does for a
                                // depth-0 explicit subject (`Subject::is_file` short-circuits on
                                // `dent.depth() == 0`).
                                if !w.is_file && !w.is_dir {
                                    continue;
                                }
                                if let Some(g) = &glob {
                                    // Anything under a directory the override already pruned is
                                    // gone — files AND nested directories — before any further
                                    // test. This is `skip_current_dir()`'s effect, applied on the
                                    // consumer side (walk.rs:1131-1145).
                                    if pruned.iter().any(|p| w.path.starts_with(p)) {
                                        continue;
                                    }
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
                                    if w.is_dir {
                                        // ripgrep evaluates the override for directories too
                                        // (dir.rs:416-425), and an Ignore verdict takes the whole
                                        // subtree. A plain (non-`!`) glob that simply misses does
                                        // NOT prune — overrides.rs:106 guards that fallback with
                                        // `!is_dir` — so `prunes_dir` is the only test here.
                                        //
                                        // walk.rs:1057-1060: `skip_entry` returns false at depth 0,
                                        // so the search root itself is never prunable.
                                        if w.path != search_root && g.prunes_dir(&glob_rel) {
                                            pruned.push(w.path.clone());
                                        }
                                        continue;
                                    }
                                    if !g.keeps_file(&glob_rel) {
                                        continue;
                                    }
                                } else if w.is_dir {
                                    continue;
                                }
                                let rel_path = w.path.strip_prefix(&search_root).unwrap_or(&w.path);
                                let rel = to_posix(rel_path);
                                // Traversal-discovered, so implicit: binary files are still cut
                                // off at the first NUL, exactly as before this change.
                                self.search_one(
                                    &w.path,
                                    &rel,
                                    &matcher,
                                    &cancel,
                                    binary_implicit.clone(),
                                    context,
                                    limit,
                                    &mut count,
                                    &mut out,
                                    &mut any_line_truncated,
                                )
                                .await?;
                            }
                            Some(Err(e)) => {
                                if walk_error.is_none() {
                                    // pi surfaces rg's stderr verbatim (`stderr.trim()`,
                                    // grep.ts:310) and rg writes `rg: {path}: {os error}`.
                                    // `LocalFs::walk` yields the bare `{path}: {os error}` because
                                    // `find` must carry no prefix at all, so the `rg: ` half is
                                    // added here, at the one consumer that emulates ripgrep.
                                    walk_error =
                                        Some(ToolError::new(format!("rg: {}", e.message)));
                                }
                            }
                            None => break,
                        }
                    }
                }
            }

            // pi: `if (!killedDueToLimit && code !== 0 && code !== 1) { reject(stderr.trim()); }`
            // (grep.ts:309-313). `count >= limit` IS `killedDueToLimit`: it is the condition that
            // breaks the loop above, exactly as it kills the rg child upstream. `limit` is
            // `.max(1)` (grep.ts:189), so this cannot be vacuously true on entry.
            //
            // This check precedes the `out.is_empty()` reply below because pi's does — the
            // exit-code guard is at grep.ts:309 and the `matchCount === 0` reply at `:314` — so an
            // errored walk that found nothing reports the error rather than "No matches found".
            // It also precedes the successful formatting path, because rg exiting 2 makes pi
            // reject even when matches were found.
            if count < limit
                && let Some(e) = walk_error
            {
                return Err(e);
            }
        }

        if out.is_empty() {
            return Ok(ToolResult {
                content: vec![Content::text("No matches found")],
                details: None,
                terminate: false,
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
            terminate: false,
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::GrepTool;
    use crate::config::GrepOpts;
    use crate::ops::local::LocalFs;
    use crate::ops::{Access, DirEntry, FsOps, Meta, WalkItem, WalkOpts};
    use cyrup_core::{CancelToken, Content, EventStream, Tool, ToolCallId, ToolError, ToolUpdate};
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
            Some(Content::Text { text, .. }) => text.clone(),
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
            Some(Content::Text { text, .. }) => text.clone(),
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
            Some(Content::Text { text, .. }) => text.clone(),
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
}
