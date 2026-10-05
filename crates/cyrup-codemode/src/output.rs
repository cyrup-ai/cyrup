//! The script output budget, its truncation and the temp-file spill (pi
//! `packages/coding-agent/src/extensions/codemode/execute.ts:232-300` @v1.0.1, CODE-012).
//!
//! The budget is an *estimate*: [`CHARS_PER_TOKEN`] characters per token over the text's UTF-16
//! length, which is `execute.ts`' own `CHARS_PER_TOKEN = 4` (`:235`). Upstream does not use a
//! tokenizer or pi's estimator here, so there is no cyrup estimator to reuse and none is substituted.
//!
//! # Functional core and shell
//!
//! [`plan_truncation`] is the decision, a pure function: under budget the items pass through, over
//! budget it yields the one text item that keeps the head and tail, plus the full text that must be
//! saved. [`spill_output`] is the only I/O. [`TruncationPlan::finish`] joins the two, so a test can
//! feed it a spill result without a file system, and [`truncate_output`] is the composition a caller
//! uses.
//!
//! # Production call path
//!
//! The `codemode` tool's execute path calls [`truncate_output`] with `sourceOptions.maxOutputTokens
//! ?? DEFAULT_MAX_OUTPUT_TOKENS` (`execute.ts:423`) over the script's output items, passing
//! `|text| spill_output(&std::env::temp_dir(), text)`, and reports
//! [`TruncatedOutput::full_output_path`] as `CodemodeToolDetails.fullOutputPath`. Nothing in this
//! crate calls them: they are the crate's public API for that lane.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::types::OutputItem;

/// Default token budget for script output (`execute.ts:233`).
pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 10_000;
/// Characters per token when estimating (`execute.ts:235`).
pub const CHARS_PER_TOKEN: u64 = 4;

/// Why the full output could not be saved. The call still succeeds: the footer says so
/// (`[Could not save the full output: <error>]`) rather than failing the script's result.
#[derive(Debug, thiserror::Error)]
pub enum SpillError {
    /// The random part of the file name could not be drawn.
    #[error("{0}")]
    Entropy(#[source] getrandom::Error),
    #[error("{0}")]
    Write(#[source] io::Error),
}

/// An over-budget output, decided but not yet joined with the result of saving the full text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruncationPlan {
    /// The single text item's text, without the spill footer.
    text: String,
    /// What the spill file must contain: every text item joined with `\n`.
    full_text: String,
    images: Vec<OutputItem>,
}

/// The decision of [`plan_truncation`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputPlan {
    /// No text, or text within budget: the items as they were.
    Untouched(Vec<OutputItem>),
    Truncated(TruncationPlan),
}

/// The items to show, and where the full text went when it was saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruncatedOutput {
    pub items: Vec<OutputItem>,
    /// Set only when the output was truncated and saving succeeded.
    pub full_output_path: Option<PathBuf>,
}

impl TruncationPlan {
    /// The text the spill file must hold.
    #[must_use]
    pub fn full_text(&self) -> &str {
        &self.full_text
    }

    /// Append the footer for `spilled` and place the images after the text item
    /// (`execute.ts:291-299`).
    #[must_use]
    pub fn finish(self, spilled: Result<PathBuf, SpillError>) -> TruncatedOutput {
        let (footer, full_output_path) = match spilled {
            Ok(path) => (
                format!(
                    "\n\n[Full output: {} (read with offset/limit)]",
                    path.display()
                ),
                Some(path),
            ),
            Err(error) => (
                format!("\n\n[Could not save the full output: {error}]"),
                None,
            ),
        };
        let mut items = vec![OutputItem::Text(format!("{}{footer}", self.text))];
        items.extend(self.images);
        TruncatedOutput {
            items,
            full_output_path,
        }
    }
}

/// Decide what the token budget does to `items` (`execute.ts:277-300`, the part before the spill).
///
/// When the combined text (text items joined with `\n`) is longer than `max_tokens *
/// CHARS_PER_TOKEN` UTF-16 units, it becomes one item: a `Warning: truncated output (original
/// token count: N)` line, the total line count, the first half of the budget, an `…N tokens
/// truncated…` marker, and the last half. A cut that lands inside a surrogate pair leaves U+FFFD
/// where upstream leaves a lone surrogate, which UTF-8 encodes as U+FFFD too.
#[must_use]
pub fn plan_truncation(items: Vec<OutputItem>, max_tokens: u64) -> OutputPlan {
    let texts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            OutputItem::Text(text) => Some(text.as_str()),
            OutputItem::Image { .. } => None,
        })
        .collect();
    let combined = texts.join("\n");
    let units: Vec<u16> = combined.encode_utf16().collect();
    let total = units.len() as u64;
    let budget = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    if total <= budget {
        return OutputPlan::Untouched(items);
    }
    let head_chars = budget / 2;
    let tail_chars = budget - head_chars;
    let removed = total - head_chars - tail_chars;
    let head = units
        .get(..usize::try_from(head_chars).unwrap_or(0))
        .map(String::from_utf16_lossy)
        .unwrap_or_default();
    let tail = if tail_chars > 0 {
        let from = usize::try_from(total - tail_chars).unwrap_or(0);
        units
            .get(from..)
            .map(String::from_utf16_lossy)
            .unwrap_or_default()
    } else {
        String::new()
    };
    let lines = combined.matches('\n').count() + 1;
    let text = format!(
        "Warning: truncated output (original token count: {})\nTotal output lines: {lines}\n\n{head}…{} tokens truncated…{tail}",
        total.div_ceil(CHARS_PER_TOKEN),
        removed.div_ceil(CHARS_PER_TOKEN),
    );
    let images = items
        .into_iter()
        .filter(|item| matches!(item, OutputItem::Image { .. }))
        .collect();
    OutputPlan::Truncated(TruncationPlan {
        text,
        full_text: combined,
        images,
    })
}

/// [`plan_truncation`], saving the full text with `spill` when the output is over budget
/// (`execute.ts:277-300`).
#[must_use]
pub fn truncate_output(
    items: Vec<OutputItem>,
    max_tokens: u64,
    spill: impl FnOnce(&str) -> Result<PathBuf, SpillError>,
) -> TruncatedOutput {
    match plan_truncation(items, max_tokens) {
        OutputPlan::Untouched(items) => TruncatedOutput {
            items,
            full_output_path: None,
        },
        OutputPlan::Truncated(plan) => {
            let spilled = spill(plan.full_text());
            plan.finish(spilled)
        }
    }
}

/// `pi-codemode-<16 hex>.txt` for 8 random bytes (`execute.ts:263`).
#[must_use]
pub fn spill_file_name(token: [u8; 8]) -> String {
    let hex: String = token.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("pi-codemode-{hex}.txt")
}

fn write_spill(dir: &Path, token: [u8; 8], text: &str) -> Result<PathBuf, SpillError> {
    let path = dir.join(spill_file_name(token));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(SpillError::Write)?;
    file.write_all(text.as_bytes()).map_err(SpillError::Write)?;
    Ok(path)
}

/// Write the full text output to `<dir>/pi-codemode-<16 hex>.txt`, like `bash` does for truncated
/// output (`execute.ts:262-270`). The file is created exclusively, so an existing file of the same
/// name is an error rather than overwritten. `dir` is the caller's temp-directory policy:
/// `std::env::temp_dir()` in production, a scratch directory in a test.
///
/// # Errors
///
/// [`SpillError`] when the name cannot be drawn or the file cannot be written.
pub fn spill_output(dir: &Path, text: &str) -> Result<PathBuf, SpillError> {
    let mut token = [0u8; 8];
    getrandom::fill(&mut token).map_err(SpillError::Entropy)?;
    write_spill(dir, token, text)
}

#[cfg(test)]
mod tests;
