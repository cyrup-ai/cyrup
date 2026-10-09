//! The script output budget, its truncation, the temp-file spill and the saved images (pi
//! `packages/coding-agent/src/extensions/codemode/execute.ts:232-300` @v1.0.1, CODE-012; the saved
//! images and the file mode are `d677d0ee7` @v1.0.3, CODE-019; the item layout,
//! [`format_output`] and [`join_adjacent_text`], is `eb326d265` @v1.1.0, CODE-021).
//!
//! The budget is an *estimate*: [`CHARS_PER_TOKEN`] characters per token over the text's UTF-16
//! length, which is `execute.ts`' own `CHARS_PER_TOKEN = 4` (`:235`). Upstream does not use a
//! tokenizer or pi's estimator here, so there is no cyrup estimator to reuse and none is substituted.
//!
//! # Functional core and shell
//!
//! [`plan_truncation`] is the decision, a pure function: under budget the items pass through, over
//! budget it yields the one text item that keeps the head and tail, plus the full text that must be
//! saved. [`spill_output`] and [`save_image_output`] are the only I/O. [`TruncationPlan::finish`] joins the two, so a test can
//! feed it a spill result without a file system, and [`truncate_output`] is the composition a caller
//! uses. [`label_images`] is the same split for images: it decides the text item that names each
//! image's file and takes the saving as a closure.
//!
//! # Production call path
//!
//! The `codemode` tool's execute path calls [`truncate_output`] with `sourceOptions.maxOutputTokens
//! ?? DEFAULT_MAX_OUTPUT_TOKENS` (`execute.ts:423`) over the script's output items, passing
//! `|text| spill_output(&std::env::temp_dir(), text)`, and reports
//! [`TruncatedOutput::full_output_path`] as `CodemodeToolDetails.fullOutputPath`. Nothing in this
//! crate calls them: they are the crate's public API for that lane.

use std::collections::HashMap;
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
            OutputItem::Text(text) | OutputItem::Console(text) => Some(text.as_str()),
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

/// Lay out the script's output so the model can tell items apart (`formatOutput`,
/// `execute.ts:262-281` @v1.1.0, pi `eb326d265`): providers join adjacent text blocks with a
/// newline or with nothing. With more than one text item (`text()` or the returned value), each
/// starts with a `==> text N/M <==` line. [`OutputItem::Console`] lines follow all other output in
/// one `<console_output>` text item. Images keep their place.
#[must_use]
pub fn format_output(output: Vec<OutputItem>) -> Vec<OutputItem> {
    let total = output
        .iter()
        .filter(|item| matches!(item, OutputItem::Text(_)))
        .count();
    let mut items = Vec::with_capacity(output.len() + 1);
    let mut console_lines = Vec::new();
    let mut index = 0;
    for item in output {
        match item {
            OutputItem::Image { .. } => items.push(item),
            OutputItem::Console(line) => console_lines.push(line),
            OutputItem::Text(text) => {
                index += 1;
                items.push(OutputItem::Text(if total > 1 {
                    format!("==> text {index}/{total} <==\n{text}")
                } else {
                    text
                }));
            }
        }
    }
    if !console_lines.is_empty() {
        items.push(OutputItem::Text(format!(
            "<console_output>\n{}\n</console_output>",
            console_lines.join("\n")
        )));
    }
    items
}

/// Join adjacent text items into one, each part starting on its own line (`joinAdjacentText`,
/// `execute.ts:284-296` @v1.1.0). A part that already ends in a newline, or is empty, gets no
/// separator.
#[must_use]
pub fn join_adjacent_text(items: Vec<OutputItem>) -> Vec<OutputItem> {
    let mut joined: Vec<OutputItem> = Vec::with_capacity(items.len());
    for item in items {
        match (joined.last_mut(), item) {
            (
                Some(OutputItem::Text(last) | OutputItem::Console(last)),
                OutputItem::Text(text) | OutputItem::Console(text),
            ) => {
                if !last.is_empty() && !last.ends_with('\n') {
                    last.push('\n');
                }
                last.push_str(&text);
            }
            (_, item) => joined.push(item),
        }
    }
    joined
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

/// `<prefix>-<16 hex><extension>` for 8 random bytes (`createOutputFilePath`,
/// `utils/output-files.ts:17-19` @v1.0.3; `extension` includes the dot).
#[must_use]
pub fn output_file_name(prefix: &str, token: [u8; 8], extension: &str) -> String {
    let hex: String = token.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{prefix}-{hex}{extension}")
}

/// `pi-codemode-<16 hex>.txt` for 8 random bytes (`execute.ts:263`).
#[must_use]
pub fn spill_file_name(token: [u8; 8]) -> String {
    output_file_name("pi-codemode", token, ".txt")
}

/// Output can carry private data, so only the user may read the files (`OUTPUT_FILE_MODE`,
/// `utils/output-files.ts:14` @v1.0.3). Unix only: Windows files have no such mode bits.
#[cfg(unix)]
const OUTPUT_FILE_MODE: u32 = 0o600;

/// A new output file: created exclusively, so a path someone else placed there (a link, say) is an
/// error and is never followed (pi's `flag: "wx"`), and readable only by the user.
fn create_output_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(OUTPUT_FILE_MODE);
    }
    options.open(path)
}

/// `writeOutputFile` (`utils/output-files.ts:22-28` @v1.0.3): write `bytes` to a new output file
/// and return its path.
fn write_output_file(
    dir: &Path,
    prefix: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<PathBuf, SpillError> {
    let mut token = [0u8; 8];
    getrandom::fill(&mut token).map_err(SpillError::Entropy)?;
    write_output_file_named(dir, prefix, extension, token, bytes)
}

/// [`write_output_file`] with the random part given, so a test can collide two names.
fn write_output_file_named(
    dir: &Path,
    prefix: &str,
    extension: &str,
    token: [u8; 8],
    bytes: &[u8],
) -> Result<PathBuf, SpillError> {
    let path = dir.join(output_file_name(prefix, token, extension));
    let mut file = create_output_file(&path).map_err(SpillError::Write)?;
    file.write_all(bytes).map_err(SpillError::Write)?;
    Ok(path)
}

#[cfg(test)]
fn write_spill(dir: &Path, token: [u8; 8], text: &str) -> Result<PathBuf, SpillError> {
    write_output_file_named(dir, "pi-codemode", ".txt", token, text.as_bytes())
}

/// Write the full text output to `<dir>/pi-codemode-<16 hex>.txt`, like `bash` does for truncated
/// output (`execute.ts:262-270`). The file is created exclusively, so an existing file of the same
/// name is an error rather than overwritten, and it is readable only by the user. `dir` is the
/// caller's temp-directory policy: `std::env::temp_dir()` in production, a scratch directory in a
/// test.
///
/// # Errors
///
/// [`SpillError`] when the name cannot be drawn or the file cannot be written.
pub fn spill_output(dir: &Path, text: &str) -> Result<PathBuf, SpillError> {
    write_output_file(dir, "pi-codemode", ".txt", text.as_bytes())
}

/// File extensions of the image types `image()` accepts (`IMAGE_EXTENSIONS`, `execute.ts`
/// @v1.0.3). Must list every type the sandbox's `image()` detects.
#[must_use]
pub fn image_extension(mime_type: &str) -> Option<&'static str> {
    match mime_type {
        "image/png" => Some(".png"),
        "image/jpeg" => Some(".jpg"),
        "image/gif" => Some(".gif"),
        "image/webp" => Some(".webp"),
        _ => None,
    }
}

/// Write an image's bytes to `<dir>/pi-codemode-<16 hex><.png|.jpg|.gif|.webp>`, exclusively and
/// readable only by the user (`saveImages`' `writeOutputFile("pi-codemode", extension, bytes)`).
///
/// # Errors
///
/// [`SpillError`] when the type has no extension, the name cannot be drawn or the file cannot be
/// written.
pub fn save_image_output(dir: &Path, mime_type: &str, bytes: &[u8]) -> Result<PathBuf, SpillError> {
    let Some(extension) = image_extension(mime_type) else {
        return Err(SpillError::Write(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("No file extension for image type {mime_type}"),
        )));
    };
    write_output_file(dir, "pi-codemode", extension, bytes)
}

/// `formatSize` (`core/tools/truncate.ts:61-69`): `123B`, `1.5KB`, `2.0MB`, one decimal rounded as
/// `toFixed(1)` rounds it (ties away from zero, where Rust's `{:.1}` rounds ties to even).
#[must_use]
pub fn format_size(bytes: usize) -> String {
    let to_fixed_1 = |value: f64| {
        let scaled = value * 10.0;
        let floor = scaled.floor();
        let rounded = if scaled - floor >= 0.5 {
            floor + 1.0
        } else {
            floor
        };
        format!("{:.1}", rounded / 10.0)
    };
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{}KB", to_fixed_1(bytes as f64 / 1024.0))
    } else {
        format!("{}MB", to_fixed_1(bytes as f64 / (1024.0 * 1024.0)))
    }
}

/// An image whose type `image()` cannot have produced: no file extension to save it under.
#[derive(Debug, thiserror::Error)]
pub enum ImageLabelError {
    #[error("No file extension for image type {0}")]
    NoExtension(String),
}

/// Put a text item naming the saved file before each image (`saveImages`, `execute.ts` @v1.0.3).
///
/// The model sees the image but has no other way to reach its bytes: scripts cannot write files,
/// and `write` only takes text. An image shown more than once is saved once and every copy gets the
/// same label. `save` writes the decoded bytes and returns the path; a failed write (disk full,
/// unwritable temp directory) must not discard the result of a script whose tool calls already ran,
/// so it becomes the label: `[Image (<mime>, <size>) could not be saved: <error>]`.
///
/// # Errors
///
/// [`ImageLabelError::NoExtension`] for an image type with no extension (upstream throws there too);
/// nothing is saved for it.
pub fn label_images(
    items: Vec<OutputItem>,
    mut save: impl FnMut(&str, &[u8]) -> Result<PathBuf, SpillError>,
) -> Result<Vec<OutputItem>, ImageLabelError> {
    use base64::Engine as _;
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let OutputItem::Image { data, mime_type } = &item else {
            out.push(item);
            continue;
        };
        let label = match labels.get(data) {
            Some(label) => label.clone(),
            None => {
                if image_extension(mime_type).is_none() {
                    return Err(ImageLabelError::NoExtension(mime_type.clone()));
                }
                let label = match base64::engine::general_purpose::STANDARD.decode(data) {
                    Ok(bytes) => {
                        let kind = format!("{mime_type}, {}", format_size(bytes.len()));
                        match save(mime_type, &bytes) {
                            Ok(path) => format!("[Image saved to {} ({kind})]", path.display()),
                            Err(error) => format!("[Image ({kind}) could not be saved: {error}]"),
                        }
                    }
                    Err(error) => {
                        format!("[Image ({mime_type}) could not be saved: {error}]")
                    }
                };
                labels.insert(data.clone(), label.clone());
                label
            }
        };
        out.push(OutputItem::Text(label));
        out.push(item);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
