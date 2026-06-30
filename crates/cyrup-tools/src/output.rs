//! Bounded streaming output with temp-file spill (R-03-026/044, arch-03 §6.5).
//!
//! `OutputAccumulator` keeps only a rolling decoded **tail** for the live preview while writing the
//! **full** raw output to a temp file, so arbitrarily large `bash` output cannot grow RSS.
//!
//! Pi (output-accumulator.ts:72-77,205-221) only opens the temp file once a limit is actually
//! exceeded — until then the raw chunks are buffered in memory and, on first overflow, replayed
//! into the freshly-created file. If the whole output fit, no temp file is ever created.

use crate::ops::local::unique_suffix;
use std::io::Write;
use std::path::PathBuf;

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
    temp_path: Option<PathBuf>,
    temp_file: Option<std::fs::File>,
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
            temp_path: None,
            temp_file: None,
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
        if self.temp_file.is_some() {
            return;
        }
        let name = format!("{}-{}.log", self.prefix, unique_suffix());
        let path = std::env::temp_dir().join(name);
        if let Ok(mut file) = std::fs::File::create(&path) {
            for chunk in self.raw_chunks.drain(..) {
                let _ = file.write_all(&chunk);
            }
            self.temp_file = Some(file);
            self.temp_path = Some(path);
        }
    }

    /// Append a raw chunk (called from the `ProcOps::exec` data callback).
    pub fn append(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        self.total_raw_bytes += chunk.len();
        // Decode through the streaming UTF-8 decoder so totals/line-counts/last-line bytes reflect
        // the DECODED text (Pi parity, UM-8). For valid UTF-8 this equals the raw counts.
        self.decode_into_counters(chunk);

        // Rolling tail for the preview.
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > self.cap {
            let start = self.buf.len() - self.cap;
            self.buf.drain(..start);
        }

        // Full output: buffer in memory until a limit is exceeded, then spill (and replay).
        if self.temp_file.is_some() || self.should_use_temp_file() {
            self.ensure_temp_replay();
            if let Some(file) = self.temp_file.as_mut() {
                let _ = file.write_all(chunk);
            }
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
        if let Some(file) = self.temp_file.as_mut() {
            let _ = file.flush();
        }
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
        if let Some(file) = self.temp_file.as_mut() {
            let _ = file.flush();
        }
        self.temp_file = None;
        self.temp_path.clone()
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

    #[test]
    fn temp_file_is_lazy_when_under_limit() {
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        acc.append(b"small output\n");
        // No limit exceeded yet ⇒ no temp file created.
        assert!(acc.temp_path.is_none(), "temp file must not be created before a limit is hit");
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
        assert_eq!(acc.total_bytes(), 12, "decoded length = 4 × U+FFFD(3 bytes)");
        assert!(acc.is_truncated(), "decoded 12B > 8B max must read as truncated like Pi");
        let path = acc.finalize(2000, 8);
        assert!(path.is_some(), "Pi spills the full output to a temp file when decoded > max");
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
}
