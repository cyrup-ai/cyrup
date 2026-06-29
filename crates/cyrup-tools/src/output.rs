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
    total_bytes: usize,
    total_newlines: usize,
    ends_with_newline: bool,
    /// Bytes since the last newline (Pi `getLastLineBytes`, used for the partial-line footer).
    current_line_bytes: usize,
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
            total_bytes: 0,
            total_newlines: 0,
            ends_with_newline: false,
            current_line_bytes: 0,
            temp_path: None,
            temp_file: None,
            prefix,
        }
    }

    /// Whether the full output has already overflowed a limit (Pi `shouldUseTempFile`).
    fn should_use_temp_file(&self) -> bool {
        self.total_bytes > self.max_bytes || self.total_lines() > self.max_lines
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
        self.total_bytes += chunk.len();
        let newlines = chunk.iter().filter(|&&b| b == b'\n').count();
        self.total_newlines += newlines;
        if let Some(&last) = chunk.last() {
            self.ends_with_newline = last == b'\n';
        }
        // Track bytes in the still-open last line (Pi `currentLineBytes`).
        match chunk.iter().rposition(|&b| b == b'\n') {
            Some(idx) => self.current_line_bytes = chunk.len() - (idx + 1),
            None => self.current_line_bytes += chunk.len(),
        }

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
        if self.total_bytes == 0 {
            return 0;
        }
        if self.ends_with_newline {
            self.total_newlines
        } else {
            self.total_newlines + 1
        }
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
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
        let truncated = self.total_bytes > max_bytes || self.total_lines() > max_lines;
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
    fn removes_file_when_not_truncated() {
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        acc.append(b"small output\n");
        let path = acc.finalize(2000, 50 * 1024);
        assert!(path.is_none());
    }
}
