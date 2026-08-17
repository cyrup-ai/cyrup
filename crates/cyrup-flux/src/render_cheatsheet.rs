//! `/flux/cheatsheet` panel — a function-for-function Rust port of
//! [`flux_cheatsheet.py`](../../../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_cheatsheet.py).
//!
//! SINGLE SOURCE OF TRUTH: the pipeline definitions are parsed at compile time from the vendored
//! `_docs/pipeline.md` — the same file the resource registry walks and skips (the `_`-prefix
//! rule, FLUX_01 Fact 5). Editing that doc changes this output automatically; nothing here is
//! hardcoded except presentation.
//!
//! No ANSI (port doc §5.8): the TUI strips escape sequences from externally supplied text, so
//! the Python's colour layer is dropped entirely. `render_flow_line`'s only remaining
//! non-presentational behaviour is `strip_slashes`, which stays.

const PIPELINE_MD: &str = include_str!("../resources/prompts/flux/_docs/pipeline.md");

/// One parsed pipeline: `(LETTER, description, flow_lines)`, in document order.
type Pipeline = (String, String, Vec<String>);

/// Match a `## PIPELINE <letter>:` heading line, mirroring
/// `PIPELINE_HEADING = re.compile(r"^##\s+PIPELINE\s+([A-Za-z0-9]+)\s*:")`. Returns the
/// upper-cased letter on a match. Implemented with `.get()` range slicing only (no raw
/// indexing), so it can never panic on a non-ASCII heading.
fn match_pipeline_heading(line: &str) -> Option<String> {
    let after_hashes = line.strip_prefix("##")?;
    let after_hashes_trimmed = after_hashes.trim_start();
    if after_hashes_trimmed.len() == after_hashes.len() {
        return None; // `\s+` requires at least one whitespace char after `##`
    }
    let after_pipeline = after_hashes_trimmed.strip_prefix("PIPELINE")?;
    let after_pipeline_trimmed = after_pipeline.trim_start();
    if after_pipeline_trimmed.len() == after_pipeline.len() {
        return None; // `\s+` requires at least one whitespace char after `PIPELINE`
    }
    let letter_end = after_pipeline_trimmed
        .char_indices()
        .find(|(_, c)| !c.is_ascii_alphanumeric())
        .map_or(after_pipeline_trimmed.len(), |(idx, _)| idx);
    if letter_end == 0 {
        return None; // `[A-Za-z0-9]+` requires at least one char
    }
    let letter = after_pipeline_trimmed.get(..letter_end)?;
    let after_letter = after_pipeline_trimmed.get(letter_end..)?;
    if !after_letter.trim_start().starts_with(':') {
        return None; // `\s*:`
    }
    Some(letter.to_ascii_uppercase())
}

/// Collapse a run of leading slashes immediately followed by `flux/` down to the single-slash
/// namespaced form, mirroring `SLASH_CMD = re.compile(r"/+flux/")` /
/// `SLASH_CMD.sub("/flux/", line)`. Implemented with `.get()` only (no raw indexing).
fn strip_slashes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    loop {
        let Some(idx) = rest.find('/') else {
            out.push_str(rest);
            break;
        };
        let prefix = rest.get(..idx).unwrap_or("");
        out.push_str(prefix);
        let after_prefix = rest.get(idx..).unwrap_or("");
        let run_len: usize = after_prefix.chars().take_while(|&c| c == '/').count();
        let run = after_prefix.get(..run_len).unwrap_or("");
        let tail = after_prefix.get(run_len..).unwrap_or("");
        if let Some(after_flux) = tail.strip_prefix("flux/") {
            out.push_str("/flux/");
            rest = after_flux;
        } else {
            out.push_str(run);
            rest = tail;
        }
    }
    out
}

/// `render_flow_line` (`flux_cheatsheet.py:132-138`) with the colour layer removed: an
/// all-whitespace line renders empty, everything else is `strip_slashes`d.
fn render_flow_line(raw: &str) -> String {
    if raw.trim().is_empty() {
        String::new()
    } else {
        strip_slashes(raw)
    }
}

/// `parse_pipelines` (`flux_cheatsheet.py:85-130`), reimplemented exactly. Every line access
/// goes through `.get(i)` rather than raw indexing.
fn parse_pipelines(md_text: &str) -> Vec<Pipeline> {
    let lines: Vec<&str> = md_text.lines().collect();
    let n = lines.len();
    let mut pipelines = Vec::new();
    let mut i = 0usize;

    while i < n {
        let Some(line) = lines.get(i) else { break };
        let Some(letter) = match_pipeline_heading(line) else {
            i += 1;
            continue;
        };
        i += 1;

        // Description: first meaningful line before the next heading; stop early at a fence.
        let mut description = String::new();
        while i < n {
            let Some(cur) = lines.get(i) else { break };
            if match_pipeline_heading(cur).is_some() {
                break;
            }
            let stripped = cur.trim();
            if stripped.starts_with("```") {
                break;
            }
            if !stripped.is_empty() && stripped != "---" && !stripped.starts_with('#') {
                description = stripped.to_string();
                i += 1;
                break;
            }
            i += 1;
        }

        // Flow: the first fenced block before the next heading.
        let mut flow: Vec<String> = Vec::new();
        while i < n {
            let Some(cur) = lines.get(i) else { break };
            if match_pipeline_heading(cur).is_some() {
                break;
            }
            if cur.trim().starts_with("```") {
                i += 1; // enter the fence
                while i < n {
                    let Some(fence_line) = lines.get(i) else { break };
                    if fence_line.trim().starts_with("```") {
                        break;
                    }
                    flow.push((*fence_line).to_string());
                    i += 1;
                }
                i += 1; // exit the fence
                break;
            }
            i += 1;
        }

        // Trim leading/trailing blank lines inside the flow.
        while flow.first().is_some_and(|l| l.trim().is_empty()) {
            flow.remove(0);
        }
        while flow.last().is_some_and(|l| l.trim().is_empty()) {
            flow.pop();
        }

        pipelines.push((letter, description, flow));
    }
    pipelines
}

/// Fixed panel width (`flux_cheatsheet.py:144`).
const WIDTH: usize = 60;

/// `render` (`flux_cheatsheet.py:144-164`) with the colour layer removed.
fn render_pipelines(pipelines: &[Pipeline]) -> String {
    let mut out: Vec<String> = Vec::new();
    out.push("\u{1D571} FLUX CHEATSHEET".to_string());
    out.push("\u{2550}".repeat(WIDTH));
    for (idx, (letter, desc, flow)) in pipelines.iter().enumerate() {
        if idx > 0 {
            out.push(String::new());
            out.push("\u{2500}".repeat(WIDTH));
        }
        out.push(String::new());
        out.push(format!("PIPELINE {letter}"));
        if !desc.is_empty() {
            out.push(desc.clone());
        }
        out.push(String::new());
        for line in flow {
            out.push(render_flow_line(line));
        }
    }
    out.push(String::new());
    out.push("\u{2550}".repeat(WIDTH));
    out.join("\n")
}

/// Parse the positional pipeline filter, matching the Python's validation (`main`,
/// `flux_cheatsheet.py:186-206`). Empty/whitespace-only args -> no filter (`Ok(None)`). A
/// case-insensitive `A`/`B`/`C`/`D` (surrounding whitespace trimmed) -> that pipeline
/// (`Ok(Some(letter))`). Anything else -> `Err` of the raw (untrimmed) argument text, so the
/// caller can self-issue an Error notification with the Python's exact wording.
pub fn parse_arg(args: &str) -> Result<Option<String>, String> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let normalized = trimmed.to_ascii_uppercase();
    if matches!(normalized.as_str(), "A" | "B" | "C" | "D") {
        Ok(Some(normalized))
    } else {
        Err(args.to_string())
    }
}

/// Render the cheatsheet for an already-validated filter (`None` = all pipelines stacked).
/// Mirrors the Python's own empty-state lines exactly.
#[must_use]
pub fn render(filter: Option<&str>) -> String {
    let mut pipelines = parse_pipelines(PIPELINE_MD);
    if let Some(want) = filter {
        pipelines.retain(|(letter, _, _)| letter == want);
        if pipelines.is_empty() {
            return format!("(no PIPELINE {want} found)");
        }
    }
    if pipelines.is_empty() {
        return "(no pipelines found in pipeline.md)".to_string();
    }
    render_pipelines(&pipelines)
}
