//! Bounded streaming output with temp-file spill (R-03-026/044, arch-03 §6.5).
//!
//! `OutputAccumulator` keeps only a rolling decoded **tail** for the live preview while writing the
//! **full** raw output to a temp file, so arbitrarily large `bash` output cannot grow RSS. If the
//! final output turns out to fit within the limits, the temp file is removed.

use crate::ops::local::unique_suffix;
use std::io::Write;
use std::path::PathBuf;

/// Streaming accumulator for `bash` output.
pub struct OutputAccumulator {
    /// Rolling raw tail (bounded to `cap`).
    buf: Vec<u8>,
    cap: usize,
    total_bytes: usize,
    total_newlines: usize,
    ends_with_newline: bool,
    temp_path: Option<PathBuf>,
    temp_file: Option<std::fs::File>,
    prefix: &'static str,
}

impl OutputAccumulator {
    /// `max_bytes` is the preview byte limit; the rolling tail is kept at `2 * max_bytes`.
    pub fn new(prefix: &'static str, max_bytes: usize) -> Self {
        Self {
            buf: Vec::new(),
            cap: max_bytes.saturating_mul(2).max(8192),
            total_bytes: 0,
            total_newlines: 0,
            ends_with_newline: false,
            temp_path: None,
            temp_file: None,
            prefix,
        }
    }

    fn ensure_temp(&mut self) {
        if self.temp_file.is_some() {
            return;
        }
        let name = format!("{}-{}.log", self.prefix, unique_suffix());
        let path = std::env::temp_dir().join(name);
        if let Ok(file) = std::fs::File::create(&path) {
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
        self.total_newlines += chunk.iter().filter(|&&b| b == b'\n').count();
        if let Some(&last) = chunk.last() {
            self.ends_with_newline = last == b'\n';
        }

        self.ensure_temp();
        if let Some(file) = self.temp_file.as_mut() {
            let _ = file.write_all(chunk);
        }

        self.buf.extend_from_slice(chunk);
        if self.buf.len() > self.cap {
            let start = self.buf.len() - self.cap;
            self.buf.drain(..start);
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

    /// The rolling tail decoded lossily for the live preview.
    pub fn tail_string(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }

    /// Flush the temp file and, if the output did not exceed `max_lines`/`max_bytes`, remove it
    /// (full output fits in the preview). Returns the path of the retained full-output file, if any.
    pub fn finalize(&mut self, max_lines: usize, max_bytes: usize) -> Option<PathBuf> {
        if let Some(file) = self.temp_file.as_mut() {
            let _ = file.flush();
        }
        let truncated = self.total_bytes > max_bytes || self.total_lines() > max_lines;
        if !truncated {
            self.temp_file = None;
            if let Some(path) = self.temp_path.take() {
                let _ = std::fs::remove_file(&path);
            }
            return None;
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
        let mut acc = OutputAccumulator::new("cyrup-test", 1024);
        acc.append(b"a\nb\nc");
        assert_eq!(acc.total_bytes(), 5);
        assert_eq!(acc.total_lines(), 3);
        let _ = acc.finalize(2000, 50 * 1024);
    }

    #[test]
    fn spills_when_truncated_and_keeps_file() {
        let mut acc = OutputAccumulator::new("cyrup-test", 16);
        let big = "x".repeat(1000);
        acc.append(big.as_bytes());
        let path = acc.finalize(2000, 16);
        assert!(path.is_some());
        let p = path.unwrap();
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content.len(), 1000);
        // Rolling tail is bounded.
        assert!(acc.buf.len() <= acc.cap);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn removes_file_when_not_truncated() {
        let mut acc = OutputAccumulator::new("cyrup-test", 1024);
        acc.append(b"small output\n");
        let path = acc.finalize(2000, 50 * 1024);
        assert!(path.is_none());
    }
}
