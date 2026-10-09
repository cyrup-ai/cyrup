//! Bounded streaming output with temp-file spill (R-03-026/044, arch-03 §6.5).
//!
//! `OutputAccumulator` keeps only a rolling decoded **tail** for the live preview while writing the
//! **full** raw output to a temp file, so arbitrarily large `bash` output cannot grow RSS.
//!
//! Pi (output-accumulator.ts:72-77,205-221) only opens the temp file once a limit is actually
//! exceeded — until then the raw chunks are buffered in memory and, on first overflow, replayed
//! into the freshly-created file. If the whole output fit, no temp file is ever created.

use crate::ops::local::unique_suffix;
use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Output can carry private data, so only the user may read the files (pi `OUTPUT_FILE_MODE`,
/// `utils/output-files.ts:14` @v1.0.4). Unix only: Windows files have no such mode bits.
#[cfg(unix)]
const OUTPUT_FILE_MODE: u32 = 0o600;

/// A new output file (pi `createOutputFileStream`, `utils/output-files.ts:31-35` @v1.0.4): created
/// exclusively, so a path someone else placed there (a link, say) is an error and is never followed
/// (pi's `flags: "wx"`), and readable only by the user.
///
/// # Errors
///
/// The file could not be created, including when something already exists at `path`.
pub fn create_output_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(OUTPUT_FILE_MODE);
    }
    options.open(path)
}

/// `U+FEFF` encoded as UTF-8 — the byte-order mark `TextDecoder` removes at the head of a stream
/// when `ignoreBOM` is false (its default, output-accumulator.ts:40).
const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// The complete output, for callers that can take more than the display snapshot (pi `FullOutput`,
/// `output-accumulator.ts:18-22` @v1.1.0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullOutput {
    pub content: String,
    /// Whether `content` omits part of the output.
    pub truncated: bool,
}

/// `new TextDecoder().decode(bytes, { stream })` over a standalone buffer: one leading BOM is
/// removed, each invalid subsequence becomes U+FFFD, and an incomplete trailing sequence is
/// dropped when `stream` is set (held for a next call that never comes) or becomes one U+FFFD
/// when it is not. Rust's lossy decoding and the WHATWG decoder agree on maximal-subpart
/// replacement, so only the BOM and the held-back tail need handling here.
fn text_decode(bytes: &[u8], stream: bool) -> String {
    let bytes = bytes.strip_prefix(&UTF8_BOM[..]).unwrap_or(bytes);
    if !stream {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                out.push_str(&String::from_utf8_lossy(valid));
                match e.error_len() {
                    Some(bad) => {
                        out.push('\u{FFFD}');
                        rest = after.get(bad..).unwrap_or_default();
                    }
                    // A valid prefix of a sequence that the buffer cut off.
                    None => return out,
                }
            }
        }
    }
}

/// Stream-head BOM filter, mirroring `TextDecoder`'s default `ignoreBOM: false`
/// (output-accumulator.ts:40,70).
///
/// The BOM is removed **only** at the very start of the byte stream, so the state machine is
/// one-shot: it withholds a strict prefix of `EF BB BF` until the next byte decides, then latches
/// to [`BomFilter::Done`] and every subsequent byte passes through untouched (a second BOM, or a
/// BOM in the middle of the output, stays as a real `U+FEFF` — exactly like `TextDecoder`).
#[derive(Clone, Copy)]
enum BomFilter {
    /// The stream so far is exactly `UTF8_BOM[..n]` for `n < 3`; those `n` bytes are withheld from
    /// the decoded counters and from the preview tail. Since the withheld bytes are by definition
    /// a prefix of `UTF8_BOM`, `n` alone reconstructs them — nothing else needs storing.
    Matching(usize),
    /// The head has been decided (BOM consumed, or the first byte proved it was not a BOM).
    Done,
}

/// Streaming accumulator for `bash` output.
pub struct OutputAccumulator {
    /// Rolling raw tail (bounded to `cap`) for the live preview.
    buf: Vec<u8>,
    cap: usize,
    /// Buffered full output, held in memory until a limit is exceeded (Pi `rawChunks`).
    raw_chunks: Vec<Vec<u8>>,
    max_lines: usize,
    max_bytes: usize,
    /// Raw byte length of everything appended (Pi `totalRawBytes`).
    total_raw_bytes: usize,
    /// DECODED (UTF-8) byte length — Pi `totalDecodedBytes`. Differs from raw only when the stream
    /// contains invalid UTF-8 (each bad subsequence decodes to U+FFFD = 3 bytes). The truncation
    /// decision and the reported `totalBytes` key off THIS, not the raw count (output-accumulator.ts
    /// :96,154,205-209).
    total_decoded_bytes: usize,
    /// Newlines in the DECODED text (Pi `completedLines`).
    total_newlines: usize,
    ends_with_newline: bool,
    /// Decoded bytes since the last newline (Pi `getLastLineBytes`, used for the partial-line footer).
    current_line_bytes: usize,
    /// Streaming UTF-8 decoder carry: trailing bytes of an INCOMPLETE multibyte sequence held for
    /// the next chunk (mirrors `TextDecoder.decode(..., { stream: true })`).
    pending: Vec<u8>,
    /// Stream-head BOM removal state (mirrors `TextDecoder`'s default `ignoreBOM: false`). Applies
    /// to the DECODED path and the preview tail only — `total_raw_bytes` and the spill file keep
    /// the BOM, exactly like Pi (output-accumulator.ts:69,74-77).
    bom: BomFilter,
    temp_path: Option<PathBuf>,
    temp_file: Option<std::fs::File>,
    /// The first failure to create or write the spill file; [`Self::take_spill_error`] hands it to
    /// the caller, so the tool fails instead of returning an output it silently lost.
    /// [CYRUP-DELTA] pi attaches its stream's `error` listener only inside `closeTempFile`
    /// (output-accumulator.ts:121-142 @v1.1.0; `createOutputFileStream` adds none), so it rejects
    /// only for a failure seen after `end()`; one raised mid-run is an uncaught stream `error` that
    /// leaves `closeTempFile` unsettled. cyrup always takes pi's rejecting outcome. Once set, the
    /// spill is not retried and later chunks go nowhere, like pi's errored stream.
    spill_error: Option<(PathBuf, std::io::Error)>,
    prefix: &'static str,
}

impl OutputAccumulator {
    /// `max_bytes`/`max_lines` are the preview limits; the rolling tail is kept at `2 * max_bytes`.
    pub fn new(prefix: &'static str, max_lines: usize, max_bytes: usize) -> Self {
        Self {
            buf: Vec::new(),
            cap: max_bytes.saturating_mul(2).max(8192),
            raw_chunks: Vec::new(),
            max_lines,
            max_bytes,
            total_raw_bytes: 0,
            total_decoded_bytes: 0,
            total_newlines: 0,
            ends_with_newline: false,
            current_line_bytes: 0,
            pending: Vec::new(),
            bom: BomFilter::Matching(0),
            temp_path: None,
            temp_file: None,
            spill_error: None,
            prefix,
        }
    }

    /// Whether the full output has already overflowed a limit (Pi `shouldUseTempFile`,
    /// output-accumulator.ts:205-209): raw OR decoded byte count OR decoded line count over limit.
    fn should_use_temp_file(&self) -> bool {
        self.total_raw_bytes > self.max_bytes
            || self.total_decoded_bytes > self.max_bytes
            || self.total_lines() > self.max_lines
    }

    /// Feed a raw chunk through the stream-head BOM filter and return the bytes the DECODED path
    /// and the preview tail should see (Pi: the output of `decoder.decode(chunk, {stream:true})`
    /// minus the leading BOM, output-accumulator.ts:40,70).
    ///
    /// Zero-copy in the only case that matters at runtime — once the head is decided the chunk is
    /// borrowed straight through. The single allocation happens at most once per accumulator, for
    /// the one chunk that ends a partially-matched BOM prefix with a non-BOM byte.
    fn filter_bom<'a>(&mut self, chunk: &'a [u8]) -> Cow<'a, [u8]> {
        let BomFilter::Matching(mut matched) = self.bom else {
            return Cow::Borrowed(chunk);
        };
        let mut rest = chunk;
        while matched < UTF8_BOM.len() {
            let Some((&b, tail)) = rest.split_first() else {
                // Chunk exhausted while the stream head is still a strict BOM prefix: keep
                // withholding, exactly like `TextDecoder` holding an undecided sequence.
                self.bom = BomFilter::Matching(matched);
                return Cow::Borrowed(&[]);
            };
            if UTF8_BOM.get(matched) != Some(&b) {
                self.bom = BomFilter::Done;
                if matched == 0 {
                    // Hot path: the stream simply does not start with a BOM — borrow, never copy.
                    return Cow::Borrowed(rest);
                }
                // A partial match that turned out not to be a BOM: release the withheld prefix
                // (which is, by construction, `UTF8_BOM[..matched]`) ahead of the rest.
                let mut out = Vec::with_capacity(matched + rest.len());
                out.extend_from_slice(UTF8_BOM.get(..matched).unwrap_or_default());
                out.extend_from_slice(rest);
                return Cow::Owned(out);
            }
            matched += 1;
            rest = tail;
        }
        // Full `EF BB BF` matched: drop it and forward the remainder of this chunk.
        self.bom = BomFilter::Done;
        Cow::Borrowed(rest)
    }

    /// Streaming UTF-8 decode of a raw chunk into the decoded counters, mirroring Pi's
    /// `TextDecoder.decode(chunk, { stream: true })` + `appendDecodedText` (output-accumulator.ts:
    /// 70,148-177). Invalid byte subsequences become U+FFFD (3 bytes); an incomplete trailing
    /// sequence is carried in `pending` for the next chunk.
    fn decode_into_counters(&mut self, chunk: &[u8]) {
        self.pending.extend_from_slice(chunk);
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    let owned = s.to_string();
                    self.append_decoded_text(&owned);
                    self.pending.clear();
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if let Some(slice) = self.pending.get(..valid).filter(|s| !s.is_empty()) {
                        let owned = String::from_utf8_lossy(slice).into_owned();
                        self.append_decoded_text(&owned);
                    }
                    match e.error_len() {
                        Some(bad) => {
                            // A complete-but-invalid subsequence → one replacement char now.
                            self.append_decoded_text("\u{FFFD}");
                            self.pending.drain(..valid + bad);
                        }
                        None => {
                            // Incomplete trailing sequence: keep it for the next chunk.
                            self.pending.drain(..valid);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Flush any incomplete trailing sequence as a replacement char (Pi `decoder.decode()` with no
    /// `stream` flag, output-accumulator.ts:85). Idempotent. Call before reading final totals.
    pub fn finish(&mut self) {
        // A stream that ended while still inside a BOM prefix (`EF`, or `EF BB`, and nothing else)
        // never carried a BOM: release the withheld bytes into the decoder and the preview tail so
        // the final no-stream `decode()` renders them as one U+FFFD, exactly like Pi.
        if let BomFilter::Matching(matched) = self.bom {
            self.bom = BomFilter::Done;
            if matched > 0 {
                let held = UTF8_BOM.get(..matched).unwrap_or_default().to_vec();
                self.decode_into_counters(&held);
                self.buf.extend_from_slice(&held);
            }
        }
        if !self.pending.is_empty() {
            self.pending.clear();
            self.append_decoded_text("\u{FFFD}");
        }
    }

    /// Update decoded counters from a decoded text increment (Pi `appendDecodedText`).
    fn append_decoded_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bytes = text.len();
        self.total_decoded_bytes += bytes;
        match text.rfind('\n') {
            None => {
                self.current_line_bytes += bytes;
                self.ends_with_newline = false;
            }
            Some(idx) => {
                self.total_newlines += text.bytes().filter(|&b| b == b'\n').count();
                let tail = text.get(idx + 1..).unwrap_or("");
                self.current_line_bytes = tail.len();
                self.ends_with_newline = tail.is_empty();
            }
        }
    }

    /// Open the temp file (if not already open) and replay any buffered chunks into it.
    fn ensure_temp_replay(&mut self) {
        if self.temp_file.is_some() || self.spill_error.is_some() {
            return;
        }
        let name = format!("{}-{}.log", self.prefix, unique_suffix());
        let path = std::env::temp_dir().join(name);
        match create_output_file(&path) {
            Ok(file) => {
                self.temp_file = Some(file);
                self.temp_path = Some(path);
                for chunk in std::mem::take(&mut self.raw_chunks) {
                    self.write_spill(&chunk);
                }
            }
            Err(e) => {
                // Pi's `rawChunks = []` after the replay (output-accumulator.ts:255 @v1.1.0): the
                // buffered output goes to the failed stream and is gone either way.
                self.raw_chunks.clear();
                self.spill_error = Some((path, e));
            }
        }
    }

    /// Write to the open spill file, recording the first failure (see [`Self::spill_error`]).
    fn write_spill(&mut self, chunk: &[u8]) {
        let Some(file) = self.temp_file.as_mut() else {
            return;
        };
        if let Err(e) = file.write_all(chunk)
            && let Some(path) = self.temp_path.clone()
        {
            self.temp_file = None;
            self.spill_error = Some((path, e));
        }
    }

    /// Flush the spill file, recording a failure like [`Self::write_spill`].
    fn flush_spill(&mut self) {
        if let Some(file) = self.temp_file.as_mut()
            && let Err(e) = file.flush()
            && let Some(path) = self.temp_path.clone()
        {
            self.temp_file = None;
            self.spill_error = Some((path, e));
        }
    }

    /// The failure that cost the spill file part of the output, if any, as the `"{path}: {error}"`
    /// message the tool fails with: pi's outcome when `closeTempFile` rejects with the stream's
    /// error and `finishOutput` propagates it out of `execute` (output-accumulator.ts:121-142
    /// @v1.1.0). See [`Self::spill_error`] for the [CYRUP-DELTA] in when that happens.
    pub fn take_spill_error(&mut self) -> Option<String> {
        self.spill_error
            .take()
            .map(|(path, e)| format!("{}: {e}", path.display()))
    }

    /// Append a raw chunk (called from the `ProcOps::exec` data callback).
    ///
    /// Pi splits this chunk into a RAW path and a DECODED path (output-accumulator.ts:64-78) and a
    /// leading BOM survives on the raw side only: `totalRawBytes` counts it (:69) and the spill
    /// file/`rawChunks` keep it byte-for-byte (:74-77), while `TextDecoder`'s default
    /// `ignoreBOM: false` removes it before `appendDecodedText` ever runs (:40,70). Mirror that
    /// split exactly.
    pub fn append(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        // RAW path: the BOM counts here, and it still gates `should_use_temp_file` (Pi :69,205-208).
        self.total_raw_bytes += chunk.len();

        // DECODED path: everything the model can see goes through the stream-head BOM filter first.
        let visible = self.filter_bom(chunk);
        let visible = visible.as_ref();
        if !visible.is_empty() {
            // Decode through the streaming UTF-8 decoder so totals/line-counts/last-line bytes
            // reflect the DECODED text (Pi parity, UM-8). For valid UTF-8 this equals the raw
            // counts minus any stream-head BOM.
            self.decode_into_counters(visible);

            // Rolling tail for the preview (Pi `tailText`, built from decoded text, :155).
            self.buf.extend_from_slice(visible);
            if self.buf.len() > self.cap {
                let start = self.buf.len() - self.cap;
                self.buf.drain(..start);
            }
        }

        // Full output: buffer in memory until a limit is exceeded, then spill (and replay). The
        // ORIGINAL chunk, BOM included — Pi writes the raw `Buffer` (:74-77).
        if self.temp_file.is_some() || self.should_use_temp_file() {
            self.ensure_temp_replay();
            self.write_spill(chunk);
        } else {
            self.raw_chunks.push(chunk.to_vec());
        }
    }

    /// Total newline-terminated line count (Pi parity: trailing newline does not add a line).
    pub fn total_lines(&self) -> usize {
        if self.total_decoded_bytes == 0 {
            return 0;
        }
        if self.ends_with_newline {
            self.total_newlines
        } else {
            self.total_newlines + 1
        }
    }

    /// Pi reports `truncation.totalBytes = totalDecodedBytes` (output-accumulator.ts:105).
    pub fn total_bytes(&self) -> usize {
        self.total_decoded_bytes
    }

    /// Byte length of the still-open last line (Pi `getLastLineBytes`, output-accumulator.ts:144).
    pub fn last_line_bytes(&self) -> usize {
        self.current_line_bytes
    }

    /// The rolling tail decoded lossily for the live preview.
    pub fn tail_string(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }

    /// Whether the accumulated output has overflowed a limit (Pi `snapshot.truncation.truncated`).
    pub fn is_truncated(&self) -> bool {
        self.should_use_temp_file()
    }

    /// Non-destructive mid-stream snapshot of the full-output path (Pi
    /// `snapshot({ persistIfTruncated: true })`, output-accumulator.ts). If the output has already
    /// overflowed a limit, ensure the temp file exists (replaying buffered chunks) and flush it so a
    /// live `onUpdate` can surface `fullOutputPath`; the file stays OPEN for further appends. Returns
    /// `None` while the whole output still fits in the preview.
    pub fn snapshot_path(&mut self) -> Option<PathBuf> {
        if !self.should_use_temp_file() {
            return None;
        }
        self.ensure_temp_replay();
        self.flush_spill();
        self.temp_path.clone()
    }

    /// Flush the temp file and, if the output did not exceed `max_lines`/`max_bytes`, drop it (the
    /// full output fits in the preview). Returns the path of the retained full-output file, if any.
    pub fn finalize(&mut self, max_lines: usize, max_bytes: usize) -> Option<PathBuf> {
        self.finish();
        let truncated = self.total_raw_bytes > max_bytes
            || self.total_decoded_bytes > max_bytes
            || self.total_lines() > max_lines;
        if !truncated {
            self.temp_file = None;
            if let Some(path) = self.temp_path.take() {
                let _ = std::fs::remove_file(&path);
            }
            return None;
        }
        // Truncated: make sure the full output is on disk (it should already be, but be safe).
        self.ensure_temp_replay();
        self.flush_spill();
        self.temp_file = None;
        self.temp_path.clone()
    }

    /// The complete output, for callers that can take more than the display snapshot (pi
    /// `readFullOutput`, output-accumulator.ts:144-176 @v1.1.0). Call after [`Self::finalize`].
    /// Output longer than `max_bytes` raw bytes keeps its first and last `max_bytes / 2` bytes
    /// around an omission marker.
    ///
    /// Without a spill file the buffered chunks are the whole output, and pi decodes them all
    /// whatever their size. With one, the cut points land on character boundaries: the head is a
    /// streaming decode that holds back an incomplete trailing sequence, and the tail skips leading
    /// continuation bytes.
    ///
    /// # Errors
    ///
    /// The spill file could not be opened or read.
    pub fn read_full_output(&self, max_bytes: usize) -> std::io::Result<FullOutput> {
        let Some(path) = &self.temp_path else {
            return Ok(FullOutput {
                content: text_decode(&self.raw_chunks.concat(), false),
                truncated: false,
            });
        };
        let mut file = std::fs::File::open(path)?;
        let size = usize::try_from(file.metadata()?.len()).unwrap_or(usize::MAX);
        if size <= max_bytes {
            let mut all = Vec::with_capacity(size);
            file.read_to_end(&mut all)?;
            return Ok(FullOutput {
                content: text_decode(&all, false),
                truncated: false,
            });
        }
        let head_bytes = max_bytes / 2;
        let tail_bytes = max_bytes - head_bytes;
        let mut head = vec![0; head_bytes];
        file.read_exact(&mut head)?;
        let mut tail = vec![0; tail_bytes];
        file.seek(SeekFrom::Start((size - tail_bytes) as u64))?;
        file.read_exact(&mut tail)?;
        let head_text = text_decode(&head, true);
        let tail_start = tail
            .iter()
            .position(|b| b & 0xC0 != 0x80)
            .unwrap_or(tail.len());
        let tail_text = text_decode(tail.get(tail_start..).unwrap_or_default(), false);
        let omitted = size - head_bytes - tail_bytes;
        Ok(FullOutput {
            content: format!("{head_text}\n\n[... {omitted} bytes omitted ...]\n\n{tail_text}"),
            truncated: true,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_and_bytes() {
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        acc.append(b"a\nb\nc");
        assert_eq!(acc.total_bytes(), 5);
        assert_eq!(acc.total_lines(), 3);
        // "c" is the open last line.
        assert_eq!(acc.last_line_bytes(), 1);
        let _ = acc.finalize(2000, 50 * 1024);
    }

    /// TOOL-057 — pi `d677d0ee7` (v1.0.3): *"Output files … are now readable only by the user"*
    /// (`utils/output-files.ts` `OUTPUT_FILE_MODE = 0o600`). RED before: the spill was created with
    /// `File::create`, `0o666` minus the umask, measured `644`.
    #[cfg(unix)]
    #[test]
    fn the_spill_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut acc = OutputAccumulator::new("cyrup-test-mode", 2000, 16);
        acc.append(b"0123456789abcdefghij");
        let path = acc.finalize(2000, 16).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_file(&path);
        assert_eq!(mode, 0o600, "the spill must be readable only by the user");
    }

    /// pi creates output files with `flags: "wx"`: a path someone else placed there is an error and
    /// is never followed. RED before: `File::create` truncated and followed a link.
    #[cfg(unix)]
    #[test]
    fn an_output_file_is_never_created_through_a_path_someone_else_placed() {
        let dir = std::env::temp_dir().join(format!("cyrup-output-excl-{}", unique_suffix()));
        std::fs::create_dir(&dir).unwrap();
        let target = dir.join("target");
        std::fs::write(&target, b"keep").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let through_link = create_output_file(&link);
        let over_file = create_output_file(&target);
        let kept = std::fs::read(&target).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(through_link.is_err(), "a link at the path must be refused");
        assert!(over_file.is_err(), "an existing file must be refused");
        assert_eq!(kept, b"keep", "the file behind the link must be untouched");
    }

    #[test]
    fn temp_file_is_lazy_when_under_limit() {
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        acc.append(b"small output\n");
        // No limit exceeded yet ⇒ no temp file created.
        assert!(
            acc.temp_path.is_none(),
            "temp file must not be created before a limit is hit"
        );
        let path = acc.finalize(2000, 50 * 1024);
        assert!(path.is_none());
    }

    #[test]
    fn spills_when_truncated_and_replays_buffered_chunks() {
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 16);
        // First chunk fits under 16 bytes; buffered in memory, no temp yet.
        acc.append(b"0123456789");
        assert!(acc.temp_path.is_none());
        // Second chunk pushes total over 16 ⇒ temp opens and replays the first chunk.
        acc.append(b"abcdefghij");
        assert!(acc.temp_path.is_some());
        let path = acc.finalize(2000, 16);
        assert!(path.is_some());
        let p = path.unwrap();
        let content = std::fs::read_to_string(&p).unwrap();
        // Replayed first chunk + second chunk = full output preserved.
        assert_eq!(content, "0123456789abcdefghij");
        assert!(acc.buf.len() <= acc.cap);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn truncation_decision_uses_decoded_not_raw_bytes() {
        // UM-8: Pi keys `shouldUseTempFile`/`snapshot` off `totalDecodedBytes`, where each invalid
        // UTF-8 subsequence decodes to U+FFFD = 3 bytes (output-accumulator.ts:70,96,205-209).
        // Four 0xFF bytes: raw = 4 (UNDER an 8-byte limit) but decoded = 4*3 = 12 (OVER). Pi
        // truncates and spills; the prior raw-only cyrup did NOT. This is the byte-diff that proves
        // the decoded path.
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 8);
        acc.append(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(
            acc.total_bytes(),
            12,
            "decoded length = 4 × U+FFFD(3 bytes)"
        );
        assert!(
            acc.is_truncated(),
            "decoded 12B > 8B max must read as truncated like Pi"
        );
        let path = acc.finalize(2000, 8);
        assert!(
            path.is_some(),
            "Pi spills the full output to a temp file when decoded > max"
        );
        if let Some(p) = path {
            let _ = std::fs::remove_file(&p);
        }
    }

    #[test]
    fn valid_utf8_decoded_equals_raw() {
        // Sanity: for valid UTF-8 the decoded total equals the raw byte length (no divergence).
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        acc.append("héllo\n".as_bytes()); // 'é' is 2 bytes → 7 raw bytes, 7 decoded bytes.
        acc.finish();
        assert_eq!(acc.total_bytes(), "héllo\n".len());
        assert_eq!(acc.total_lines(), 1);
        let _ = acc.finalize(2000, 1024);
    }

    #[test]
    fn removes_file_when_not_truncated() {
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        acc.append(b"small output\n");
        let path = acc.finalize(2000, 50 * 1024);
        assert!(path.is_none());
    }

    #[test]
    fn stream_head_bom_is_invisible_to_decoded_path() {
        // Pi's `new TextDecoder()` defaults to `ignoreBOM: false` (output-accumulator.ts:40), so the
        // leading U+FEFF never reaches `appendDecodedText` (:70) — while `totalRawBytes` still counts
        // all 3 of its bytes (:69). 6 raw bytes in, 3 decoded bytes out.
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        acc.append(b"\xEF\xBB\xBFhi\n");
        acc.finish();
        assert_eq!(
            acc.total_bytes(),
            3,
            "decoded totals exclude the stream-head BOM"
        );
        assert_eq!(acc.total_lines(), 1);
        assert_eq!(acc.last_line_bytes(), 0, "chunk ends on a newline");
        // The model-visible clause: assert the STRING, not just the length.
        assert_eq!(
            acc.tail_string(),
            "hi\n",
            "preview tail must not contain U+FEFF"
        );
        assert_eq!(acc.total_raw_bytes, 6, "raw path keeps the BOM (pi :69)");
        assert!(acc.finalize(2000, 1024).is_none());

        // A stream that is nothing but a BOM decodes to the empty string.
        let mut only = OutputAccumulator::new("cyrup-test", 2000, 1024);
        only.append(&UTF8_BOM);
        only.finish();
        assert_eq!(only.total_bytes(), 0);
        assert_eq!(only.total_lines(), 0);
        assert_eq!(only.tail_string(), "");
        assert_eq!(only.total_raw_bytes, 3);
        assert!(only.finalize(2000, 1024).is_none());
    }

    #[test]
    fn bom_is_stripped_across_every_chunk_boundary() {
        // `TextDecoder` with `stream: true` (output-accumulator.ts:70) holds an undecided head across
        // chunk boundaries; `BomFilter::Matching(n)` is that carry. Every split of `EF BB BF | "hi"`
        // must produce the identical decoded stream.
        const INPUT: &[u8] = b"\xEF\xBB\xBFhi";
        for split in 0..=INPUT.len() {
            let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
            acc.append(&INPUT[..split]);
            acc.append(&INPUT[split..]);
            acc.finish();
            assert_eq!(acc.tail_string(), "hi", "split at {split}");
            assert_eq!(acc.total_bytes(), 2, "split at {split}");
            assert_eq!(
                acc.total_raw_bytes, 5,
                "split at {split}: raw is split-invariant"
            );
            assert!(acc.finalize(2000, 1024).is_none(), "split at {split}");
        }

        // Degenerate delivery: one byte per callback, the worst case for the carry.
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        for b in INPUT {
            acc.append(&[*b]);
        }
        acc.finish();
        assert_eq!(acc.tail_string(), "hi", "byte-at-a-time delivery");
        assert_eq!(acc.total_bytes(), 2, "byte-at-a-time delivery");
        assert!(acc.finalize(2000, 1024).is_none());
    }

    #[test]
    fn only_the_stream_head_bom_is_stripped() {
        // Pi strips at most one BOM, at offset 0 — every later U+FEFF is ordinary text
        // (output-accumulator.ts:40). `BomFilter::Done` is that one-shot latch.
        let mut mid = OutputAccumulator::new("cyrup-test", 2000, 1024);
        mid.append("\u{feff}a\u{feff}b".as_bytes()); // 8 raw bytes
        mid.finish();
        assert_eq!(mid.tail_string(), "a\u{feff}b");
        assert_eq!(
            mid.total_bytes(),
            5,
            "3 stripped, the interior U+FEFF's 3 bytes kept"
        );
        assert_eq!(mid.total_raw_bytes, 8);
        assert!(mid.finalize(2000, 1024).is_none());

        // Back-to-back BOMs: only the first goes.
        let mut double = OutputAccumulator::new("cyrup-test", 2000, 1024);
        double.append(b"\xEF\xBB\xBF\xEF\xBB\xBFx");
        double.finish();
        assert_eq!(
            double.tail_string(),
            "\u{feff}x",
            "the second BOM is real text"
        );
        assert_eq!(double.total_bytes(), 4);
        assert!(double.finalize(2000, 1024).is_none());
    }

    #[test]
    fn bom_lookalike_prefixes_lose_no_bytes() {
        // `EF BB 41`: two BOM bytes withheld, then the third byte disproves the BOM. `filter_bom`
        // releases `UTF8_BOM[..2]` ahead of the rest, so the decoder sees `EF BB 41` and produces
        // U+FFFD (3 bytes, for the invalid `EF BB`) + "A" — exactly what pi's TextDecoder yields.
        let mut a = OutputAccumulator::new("cyrup-test", 2000, 1024);
        a.append(b"\xEF\xBB\x41");
        a.finish();
        assert_eq!(a.tail_string(), "\u{FFFD}A");
        assert_eq!(a.total_bytes(), 4, "U+FFFD(3) + 'A'(1)");
        assert_eq!(a.total_raw_bytes, 3);
        assert!(a.finalize(2000, 1024).is_none());

        // Lone `EF` at end of stream: never a BOM, never completed. `finish` releases it into the
        // decoder, whose final no-`stream` `decode()` (output-accumulator.ts:85) emits ONE U+FFFD.
        let mut lone = OutputAccumulator::new("cyrup-test", 2000, 1024);
        lone.append(b"\xEF");
        assert_eq!(lone.total_bytes(), 0, "withheld while still undecided");
        lone.finish();
        assert_eq!(lone.tail_string(), "\u{FFFD}");
        assert_eq!(lone.total_bytes(), 3, "exactly one U+FFFD, not two");
        assert!(lone.finalize(2000, 1024).is_none());

        // `EF BB` at end of stream: the `matched == 2` release path in `finish`.
        let mut two = OutputAccumulator::new("cyrup-test", 2000, 1024);
        two.append(b"\xEF\xBB");
        two.finish();
        assert_eq!(two.tail_string(), "\u{FFFD}");
        assert_eq!(
            two.total_bytes(),
            3,
            "one U+FFFD for the incomplete sequence"
        );
        assert_eq!(two.total_raw_bytes, 2);
        assert!(two.finalize(2000, 1024).is_none());

        // `EF BF BD` is a literal U+FFFD whose FIRST byte matches UTF8_BOM[0] and whose second does
        // not. Proves `filter_bom` reassembles the byte it withheld: if the withheld `EF` were
        // dropped, `BF BD` would decode to garbage instead of one clean character.
        let mut fffd = OutputAccumulator::new("cyrup-test", 2000, 1024);
        fffd.append(b"\xEF\xBF\xBD");
        assert!(
            fffd.pending.is_empty(),
            "decoded as one complete char, nothing carried"
        );
        assert_eq!(
            fffd.buf,
            vec![0xEF, 0xBF, 0xBD],
            "withheld byte re-emitted verbatim"
        );
        fffd.finish();
        assert_eq!(fffd.tail_string(), "\u{FFFD}");
        assert_eq!(fffd.total_bytes(), 3);
        assert!(fffd.finalize(2000, 1024).is_none());
    }

    #[test]
    fn spill_file_keeps_the_bom_and_raw_count_still_gates_the_spill() {
        // Pi writes the untouched `Buffer` to the spill (output-accumulator.ts:74,76): the full-output
        // file is a byte-exact copy of the process's stdout, BOM included.
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 16);
        acc.append(b"\xEF\xBB\xBF0123456789"); // 13 raw, under the 16-byte limit ⇒ buffered
        assert!(acc.temp_path.is_none());
        acc.append(b"abcdefghij"); // 23 raw ⇒ spill opens and replays chunk 1 WITH the BOM
        assert!(acc.temp_path.is_some());
        assert_eq!(acc.total_bytes(), 20, "decoded side still excludes the BOM");
        let p = acc.finalize(2000, 16).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(
            &bytes[..3],
            &UTF8_BOM[..],
            "spill file must start with EF BB BF"
        );
        assert_eq!(bytes, b"\xEF\xBB\xBF0123456789abcdefghij".to_vec());
        let _ = std::fs::remove_file(&p);

        // Boundary flavour: 14 payload bytes + 3 BOM bytes = 17 raw > 16 = max, while decoded 14 <= 16.
        // The spill is triggered by `totalRawBytes` ALONE (pi :69,205-207) — the BOM must still count.
        let mut edge = OutputAccumulator::new("cyrup-test", 2000, 16);
        edge.append(b"\xEF\xBB\xBF0123456789abcd");
        assert_eq!(edge.total_bytes(), 14, "decoded is under the limit");
        assert_eq!(
            edge.total_raw_bytes, 17,
            "raw is over it, only because of the BOM"
        );
        assert!(edge.is_truncated(), "raw count alone must trip the spill");
        let p = edge.finalize(2000, 16).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(bytes, b"\xEF\xBB\xBF0123456789abcd".to_vec());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn finish_is_idempotent_after_a_partial_bom() {
        // `finish` latches `bom = Done` BEFORE releasing the withheld prefix (output.rs:183). Without
        // that latch the second call would re-release `EF` and emit a second U+FFFD. `finalize` calls
        // `finish` again internally (:327), so this is a real path, not a hypothetical.
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        acc.append(b"\xEF");
        acc.finish();
        assert_eq!(acc.total_bytes(), 3);
        assert_eq!(acc.buf.len(), 1);
        acc.finish();
        assert_eq!(
            acc.total_bytes(),
            3,
            "second finish must not emit another U+FFFD"
        );
        assert_eq!(
            acc.buf.len(),
            1,
            "second finish must not re-release the prefix"
        );
        assert_eq!(acc.tail_string(), "\u{FFFD}");
        assert!(
            acc.finalize(2000, 1024).is_none(),
            "finalize's internal finish is also a no-op"
        );
        assert_eq!(acc.total_bytes(), 3);
    }

    /// Spill `bytes` and finalize, so `read_full_output` reads them back from the temp file.
    fn spilled(bytes: &[u8]) -> (OutputAccumulator, PathBuf) {
        let mut acc = OutputAccumulator::new("cyrup-test-full", 2000, 16);
        acc.append(bytes);
        let path = acc.finalize(2000, 16).unwrap();
        (acc, path)
    }

    /// TOOL-054 — pi `readFullOutput` (output-accumulator.ts:144-176 @v1.1.0): over the limit, the
    /// first `floor(max / 2)` and the last `max - floor(max / 2)` raw bytes around a marker that
    /// counts the bytes in between.
    #[test]
    fn full_output_keeps_head_and_tail_around_an_omission_marker() {
        let size = 3 * 1024 * 1024;
        let bytes: Vec<u8> = (0..size).map(|i| b'a' + (i % 26) as u8).collect();
        let (acc, path) = spilled(&bytes);
        let full = acc.read_full_output(1024 * 1024).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(full.truncated);
        let half = 512 * 1024;
        let expected = format!(
            "{}\n\n[... {} bytes omitted ...]\n\n{}",
            std::str::from_utf8(&bytes[..half]).unwrap(),
            size - 1024 * 1024,
            std::str::from_utf8(&bytes[size - half..]).unwrap(),
        );
        assert_eq!(full.content, expected);

        // An odd budget gives the extra byte to the tail.
        let (acc, path) = spilled(b"0123456789abcdefghij");
        let full = acc.read_full_output(5).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(full.content, "01\n\n[... 15 bytes omitted ...]\n\nhij");
    }

    /// The cut never splits a character: the head holds back an incomplete trailing sequence and
    /// the tail skips leading continuation bytes, at every phase of a multi-byte stream.
    #[test]
    fn full_output_cuts_on_character_boundaries() {
        for unit in ["é", "€", "😀"] {
            for pad in 0..4 {
                let mut text = "x".repeat(pad);
                text.push_str(&unit.repeat(40));
                let (acc, path) = spilled(text.as_bytes());
                let full = acc.read_full_output(21).unwrap();
                let _ = std::fs::remove_file(&path);
                assert!(full.truncated);
                assert!(
                    !full.content.contains('\u{FFFD}'),
                    "{unit} pad {pad}: {:?}",
                    full.content
                );
                let (head, tail) = full.content.split_once("\n\n[... ").unwrap();
                assert!(text.starts_with(head), "{unit} pad {pad}");
                let tail = tail.split_once("...]\n\n").unwrap().1;
                assert!(text.ends_with(tail), "{unit} pad {pad}");
                assert!(
                    head.len() <= 10 && head.len() > 10 - unit.len(),
                    "{unit} pad {pad}"
                );
                assert!(
                    tail.len() <= 11 && tail.len() > 11 - unit.len(),
                    "{unit} pad {pad}"
                );
            }
        }
    }

    /// Without a spill file the buffered chunks are the whole output, decoded like `TextDecoder`:
    /// the leading BOM goes, an invalid byte becomes U+FFFD, and nothing is cut.
    #[test]
    fn full_output_without_a_spill_is_the_whole_decoded_output() {
        let mut acc = OutputAccumulator::new("cyrup-test-full", 2000, 1024);
        acc.append(b"\xEF\xBB\xBFone\n");
        acc.append(b"two \xFF\n");
        assert!(acc.finalize(2000, 1024).is_none());
        let full = acc.read_full_output(4).unwrap();
        assert_eq!(
            full,
            FullOutput {
                content: "one\ntwo \u{FFFD}\n".to_owned(),
                truncated: false,
            }
        );

        // A spill under the budget is read whole, with its BOM removed the same way.
        let (acc, path) = spilled(b"\xEF\xBB\xBF0123456789abcdefghij");
        let full = acc.read_full_output(1024).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(!full.truncated);
        assert_eq!(full.content, "0123456789abcdefghij");
    }

    /// A spill file that cannot be created loses the output pi's stream would also lose, and the
    /// failure is reported instead of the loss passing silently (pi's rejecting `closeTempFile`,
    /// output-accumulator.ts:121-142 @v1.1.0; [CYRUP-DELTA] in that cyrup reports it every time).
    #[test]
    fn a_spill_file_that_cannot_be_created_is_reported() {
        let mut acc = OutputAccumulator::new("cyrup-missing-dir/cyrup-test", 2000, 16);
        acc.append(b"0123456789");
        acc.append(b"abcdefghij");
        assert!(acc.finalize(2000, 16).is_none());
        let message = acc.take_spill_error().unwrap();
        assert!(message.contains("cyrup-missing-dir"), "{message}");
        assert!(acc.take_spill_error().is_none(), "taken once");
    }

    /// A spill file that was created but then refuses a write is reported the same way, and the
    /// spill stops there: later chunks are not written to it.
    #[test]
    fn a_spill_write_that_fails_is_reported() {
        let mut acc = OutputAccumulator::new("cyrup-test-write-fail", 2000, 16);
        acc.append(b"0123456789");
        acc.append(b"abcdefghij");
        let path = acc.temp_path.clone().unwrap();
        // Swap the writable handle for a read-only one so the next write fails with EBADF.
        acc.temp_file = Some(std::fs::File::open(&path).unwrap());
        acc.append(b"klmnopqrst");
        assert!(acc.temp_file.is_none(), "a failed write closes the spill");
        acc.append(b"uvwxyz");
        let message = acc.take_spill_error().unwrap();
        assert!(
            message.starts_with(&format!("{}: ", path.display())),
            "{message}"
        );
        let kept = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(kept, "0123456789abcdefghij");
    }

    #[test]
    fn no_bom_output_is_untouched() {
        // Hot path: the first byte (0x68) is not UTF8_BOM[0], so `filter_bom` borrows the whole chunk
        // straight through. Cheap insurance that the filter never eats a leading byte.
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        acc.append(b"hello\n");
        acc.finish();
        assert_eq!(acc.tail_string(), "hello\n");
        assert_eq!(acc.total_bytes(), 6);
        assert_eq!(
            acc.total_raw_bytes, 6,
            "raw and decoded agree when there is no BOM"
        );
        assert_eq!(acc.total_lines(), 1);
        assert!(acc.finalize(2000, 1024).is_none());
    }
}
