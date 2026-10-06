//! Presentation for the `codemode` tool (pi `extensions/codemode/renderer.ts` @v1.0.1).
//!
//! The call shows the script; the result lists the nested tool calls with their status as they
//! run and the cost of its model calls, followed by the script output without the "Script
//! completed" header. Nested calls are not separate tool rows because they never reach the model
//! as tool calls (`renderer.ts:1-8`).
//!
//! The tool declares no `renderShell`, so it keeps the default shell (`tool.ts:384` spreads only
//! `renderCall` and `renderResult`).
//!
//! # Production call path
//!
//! [`crate::CodemodeExtension`] declares a renderer for the tool in `init`
//! (`InitApi::register_tool_renderer`) and answers [`cyrup_ext::NativeExtension::render_call_tree`] /
//! `render_result_tree` with [`render_call`] / [`render_result`]. The TUI asks for the call tree at
//! `tool_execution_start`, for a result tree at every `tool_execution_update` (the partial result the
//! [`crate::tool::recorder::Recorder`] publishes on each nested-call change) and again at
//! `tool_execution_end`, then lays the tree out on every frame.

use std::sync::Arc;

use cyrup_ext::{PreviewKeep, RenderNode, RenderOptions, RenderTheme, RenderedTree, TreeCtx};
use serde_json::Value;

use crate::tool::{CodemodeNestedCall, CodemodeNestedCallStatus, CodemodeToolDetails};

/// `CODE_PREVIEW_LINES` (`renderer.ts:17`).
const CODE_PREVIEW_LINES: usize = 10;
/// `CALL_PREVIEW_COUNT` (`renderer.ts:18`).
const CALL_PREVIEW_COUNT: usize = 8;
/// `OUTPUT_PREVIEW_LINES` (`renderer.ts:19`).
const OUTPUT_PREVIEW_LINES: usize = 5;
/// `COLLAPSED_ARGS_CHARS` (`renderer.ts:20`).
const COLLAPSED_ARGS_CHARS: usize = 80;

/// `expandHint` (`renderer.ts:23-25`).
fn expand_hint(theme: &dyn RenderTheme, hidden: usize, noun: &str) -> String {
    format!(
        "{} {}{}",
        theme.fg("muted", &format!("... ({hidden} more {noun},")),
        theme.key_hint("app.tools.expand", "to expand"),
        theme.fg("muted", ")"),
    )
}

/// `Math.round`: halves go toward positive infinity, which is not what [`f64::round`] does for a
/// negative half.
fn js_round(x: f64) -> f64 {
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// `String(x)` for an integral-valued number.
fn js_integer_string(x: f64) -> String {
    // `-0` prints as `0` in JS; Rust prints `-0`.
    let x = if x == 0.0 { 0.0 } else { x };
    format!("{x}")
}

/// `formatDuration` (`renderer.ts:27-30`): `Math.round(ms)ms` under a second, `(ms / 1000).toFixed(1)s`
/// from a second on, nothing when the duration is unknown.
#[must_use]
pub fn format_duration(ms: Option<f64>) -> String {
    let Some(ms) = ms else { return String::new() };
    if ms < 1000.0 {
        format!("{}ms", js_integer_string(js_round(ms)))
    } else {
        format!("{}s", js_to_fixed(ms / 1000.0, 1))
    }
}

/// `formatCost` (`renderer.ts:32-35`): cents to two decimals for amounts of a cent or more, two
/// significant digits for the fractions of a cent classifier calls cost.
#[must_use]
pub fn format_cost(cost: f64) -> String {
    if cost >= 0.01 {
        format!("${}", js_to_fixed(cost, 2))
    } else {
        format!("${}", js_to_precision(cost, 2))
    }
}

/// `String(x)` for a number that is too large for `toFixed`, or not finite.
fn js_number_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let shortest = format!("{x:e}");
    match shortest.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => {
            format!("{mantissa}e+{exponent}")
        }
        _ => shortest,
    }
}

/// The exact decimal digits of a finite, non-negative double: the integer digits, then the
/// fractional digits (a double is a dyadic rational, so the expansion terminates inside 1074
/// places).
fn exact_digits(x: f64) -> (Vec<u8>, Vec<u8>) {
    let text = format!("{x:.1100}");
    let (int, frac) = text.split_once('.').unwrap_or((text.as_str(), ""));
    (
        int.bytes().map(|b| b - b'0').collect(),
        frac.bytes().map(|b| b - b'0').collect(),
    )
}

/// Adds one unit in the last place of `digits`, growing it on overflow (`999` → `1000`).
fn increment(digits: &mut Vec<u8>) {
    for digit in digits.iter_mut().rev() {
        if *digit == 9 {
            *digit = 0;
        } else {
            *digit += 1;
            return;
        }
    }
    digits.insert(0, 1);
}

/// `Number.prototype.toFixed(digits)` for a finite non-negative `x`: the exact value rounded half up
/// (the spec picks the larger `n` on a tie, which [`format!`]'s round-half-even does not, so
/// `1.25` is `"1.3"` here and `"1.2"` there).
fn js_to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() || x >= 1e21 {
        return js_number_string(x);
    }
    if x < 0.0 {
        return format!("-{}", js_to_fixed(-x, digits));
    }
    let (int, frac) = exact_digits(x);
    let mut kept: Vec<u8> = int
        .iter()
        .copied()
        .chain(frac.iter().copied().take(digits))
        .collect();
    // Pad: the exact expansion can be shorter than `digits` places.
    while kept.len() < int.len() + digits {
        kept.push(0);
    }
    if frac.get(digits).copied().unwrap_or(0) >= 5 {
        increment(&mut kept);
    }
    let split = kept.len() - digits;
    let (whole, fraction) = kept.split_at(split);
    let mut out: String = whole.iter().map(|d| char::from(b'0' + d)).collect();
    if out.is_empty() {
        out.push('0');
    }
    if digits > 0 {
        out.push('.');
        out.extend(fraction.iter().map(|d| char::from(b'0' + d)));
    }
    out
}

/// `Number.prototype.toPrecision(precision)` for a finite `x` (`precision >= 1`): `precision`
/// significant digits, rounded half up, in exponential form when the exponent is below -6 or at
/// least `precision`.
fn js_to_precision(x: f64, precision: usize) -> String {
    if !x.is_finite() {
        return js_number_string(x);
    }
    if x < 0.0 {
        return format!("-{}", js_to_precision(-x, precision));
    }
    if x == 0.0 {
        return if precision > 1 {
            format!("0.{}", "0".repeat(precision - 1))
        } else {
            "0".to_owned()
        };
    }
    let (int, frac) = exact_digits(x);
    let all: Vec<u8> = int.iter().copied().chain(frac.iter().copied()).collect();
    let Some(first) = all.iter().position(|d| *d != 0) else {
        return "0".to_owned();
    };
    // Decimal exponent of the leading digit: the digit at index `i` of `all` has weight
    // `10^(int.len() - 1 - i)`.
    let mut exponent =
        i64::try_from(int.len()).unwrap_or(i64::MAX) - 1 - i64::try_from(first).unwrap_or(i64::MAX);
    let mut n: Vec<u8> = all.iter().copied().skip(first).take(precision).collect();
    while n.len() < precision {
        n.push(0);
    }
    if all.get(first + precision).copied().unwrap_or(0) >= 5 {
        increment(&mut n);
        if n.len() > precision {
            // `9.96` at two digits is `10`, one order larger.
            n.truncate(precision);
            exponent += 1;
        }
    }
    let digits: String = n.iter().map(|d| char::from(b'0' + d)).collect();
    let precision_i = i64::try_from(precision).unwrap_or(i64::MAX);
    if exponent < -6 || exponent >= precision_i {
        let (lead, rest) = digits.split_at(1);
        let mantissa = if rest.is_empty() {
            lead.to_owned()
        } else {
            format!("{lead}.{rest}")
        };
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{}", exponent.abs());
    }
    if exponent == precision_i - 1 {
        return digits;
    }
    if exponent >= 0 {
        let split = usize::try_from(exponent + 1).unwrap_or(0);
        let (whole, fraction) = digits.split_at(split);
        return format!("{whole}.{fraction}");
    }
    let zeros = usize::try_from(-(exponent + 1)).unwrap_or(0);
    format!("0.{}{digits}", "0".repeat(zeros))
}

/// The first `max` UTF-16 units of `text` (`String.prototype.slice(0, max)`), without splitting a
/// surrogate pair.
fn utf16_prefix(text: &str, max: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in text.chars() {
        units += c.len_utf16();
        if units > max {
            break;
        }
        out.push(c);
    }
    out
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `statusIcon` (`renderer.ts:37-49`).
fn status_icon(call: &CodemodeNestedCall, theme: &dyn RenderTheme) -> String {
    match call.status {
        CodemodeNestedCallStatus::Running => theme.fg("warning", "…"),
        CodemodeNestedCallStatus::Ok => theme.fg("success", "✓"),
        CodemodeNestedCallStatus::Error => theme.fg("error", "✗"),
        CodemodeNestedCallStatus::Cancelled => theme.fg("muted", "⊘"),
    }
}

/// A JS-truthy `cost`: present, not `0`, not `NaN`.
fn truthy_cost(call: &CodemodeNestedCall) -> Option<f64> {
    call.cost.filter(|cost| *cost != 0.0 && !cost.is_nan())
}

/// `formatCall` (`renderer.ts:51-64`).
fn format_call(call: &CodemodeNestedCall, theme: &dyn RenderTheme, expanded: bool) -> String {
    let args = if !expanded && utf16_len(&call.args) > COLLAPSED_ARGS_CHARS {
        format!("{}...", utf16_prefix(&call.args, COLLAPSED_ARGS_CHARS - 3))
    } else {
        call.args.clone()
    };
    let duration = format_duration(call.duration_ms);
    let mut line = format!(
        "{} {}",
        status_icon(call, theme),
        theme.fg("toolTitle", &call.name)
    );
    if !args.is_empty() {
        line.push(' ');
        line.push_str(&theme.fg("muted", &args));
    }
    if !duration.is_empty() {
        line.push(' ');
        line.push_str(&theme.fg("dim", &duration));
    }
    if let Some(cost) = truthy_cost(call) {
        line.push(' ');
        line.push_str(&theme.fg("dim", &format_cost(cost)));
    }
    if expanded && let Some(error) = call.error.as_deref().filter(|error| !error.is_empty()) {
        line.push_str("\n    ");
        line.push_str(&theme.fg("error", &error.replace('\n', "\n    ")));
    }
    line
}

/// JS `String.prototype.trim`: Unicode white space plus the byte-order mark.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}

/// `SCRIPT_HEADER` (`renderer.ts:21`): `^Script (completed|failed)\nWall time [\d.]+ seconds\nOutput:\n$`.
fn is_script_header(text: &str) -> bool {
    let Some(rest) = text
        .strip_prefix("Script completed\n")
        .or_else(|| text.strip_prefix("Script failed\n"))
    else {
        return false;
    };
    let Some(rest) = rest.strip_prefix("Wall time ") else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(" seconds\nOutput:\n") else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The arguments of a `codemode` call as `renderCall` reads them: `str(args.code)`
/// (`render-utils.ts`), where a missing or `null` value is the empty script and any other
/// non-string is an invalid argument.
#[derive(Debug)]
enum CallArgs {
    Script(String),
    Invalid,
}

/// The component `renderCall` returns (`renderer.ts:66-92`).
#[derive(Debug)]
struct CallComponent {
    args: CallArgs,
}

impl RenderedTree for CallComponent {
    fn tree<'a>(&self, ctx: &TreeCtx<'a>) -> RenderNode<'a> {
        let theme = ctx.theme;
        let title = theme.fg("toolTitle", &theme.bold("codemode"));
        let code = match &self.args {
            CallArgs::Invalid => {
                return RenderNode::Container(vec![RenderNode::Text(format!(
                    "{title} {}",
                    theme.fg("error", "[invalid arg]")
                ))]);
            }
            CallArgs::Script(code) => code,
        };
        let mut children = vec![RenderNode::Text(title)];
        if !code.is_empty() {
            // The code includes the `// @options:` line, so options show as part of the script.
            let source = code.replace('\r', "");
            let source = source.trim_end().replace('\t', "   ");
            let highlighted = theme.highlight_code(&source, "javascript").join("\n");
            children.push(if ctx.expanded {
                RenderNode::Text(highlighted)
            } else {
                RenderNode::VisualLinePreview {
                    text: highlighted,
                    max_visual_lines: CODE_PREVIEW_LINES,
                    keep: PreviewKeep::Start,
                    hint: Box::new(move |hidden| expand_hint(theme, hidden, "lines")),
                }
            });
        }
        RenderNode::Container(children)
    }
}

/// `renderCall(args, theme, context)`: the component for a call with these arguments.
#[must_use]
pub fn render_call(args: &Value) -> Arc<dyn RenderedTree> {
    let code = match args.get("code") {
        None | Some(Value::Null) => CallArgs::Script(String::new()),
        Some(Value::String(code)) => CallArgs::Script(code.clone()),
        Some(_) => CallArgs::Invalid,
    };
    Arc::new(CallComponent { args: code })
}

/// The component `renderResult` returns (`renderer.ts:93-149`).
#[derive(Debug)]
struct ResultComponent {
    calls: Vec<CodemodeNestedCall>,
    /// The text blocks of the result's content, past the `Script completed` header.
    output: Vec<String>,
    full_output_path: Option<String>,
    is_partial: bool,
    is_error: bool,
}

impl RenderedTree for ResultComponent {
    fn tree<'a>(&self, ctx: &TreeCtx<'a>) -> RenderNode<'a> {
        let theme = ctx.theme;
        let expanded = ctx.expanded;
        let mut children = Vec::new();
        if !self.calls.is_empty() {
            let shown = if expanded {
                self.calls.as_slice()
            } else {
                let from = self.calls.len().saturating_sub(CALL_PREVIEW_COUNT);
                self.calls.get(from..).unwrap_or_default()
            };
            let mut lines: Vec<String> = shown
                .iter()
                .map(|call| format_call(call, theme, expanded))
                .collect();
            if shown.len() < self.calls.len() {
                lines.insert(
                    0,
                    format!(
                        "{} {}{}",
                        theme.fg(
                            "muted",
                            &format!("... ({} earlier calls,", self.calls.len() - shown.len())
                        ),
                        theme.key_hint("app.tools.expand", "to expand"),
                        theme.fg("muted", ")"),
                    ),
                );
            }
            // Collapsed rows hide earlier calls, so the total covers every call.
            let priced: Vec<f64> = self.calls.iter().filter_map(truthy_cost).collect();
            if priced.len() > 1 {
                let total: f64 = priced.iter().sum();
                lines.push(theme.fg("muted", &format!("Model calls: {}", format_cost(total))));
            }
            children.push(RenderNode::Spacer(1));
            children.push(RenderNode::Text(lines.join("\n")));
        }

        let output = if self.is_partial {
            String::new()
        } else {
            let joined = self
                .output
                .iter()
                .map(|text| theme.display_text(text))
                .collect::<Vec<_>>()
                .join("\n");
            js_trim(&joined).to_owned()
        };
        if !output.is_empty() {
            let color = if self.is_error { "error" } else { "toolOutput" };
            let styled = output
                .replace('\t', "   ")
                .split('\n')
                .map(|line| theme.fg(color, line))
                .collect::<Vec<_>>()
                .join("\n");
            children.push(RenderNode::Spacer(1));
            if expanded {
                children.push(RenderNode::Text(styled));
            } else {
                // Limit wrapped lines, not logical ones: script output is often one long JSON line.
                children.push(RenderNode::VisualLinePreview {
                    text: styled,
                    max_visual_lines: OUTPUT_PREVIEW_LINES,
                    keep: PreviewKeep::Start,
                    hint: Box::new(move |hidden| expand_hint(theme, hidden, "lines")),
                });
                // The collapsed preview hides the truncation notice at the end, so name the file here.
                if let Some(path) = self
                    .full_output_path
                    .as_deref()
                    .filter(|path| !path.is_empty())
                {
                    children.push(RenderNode::Text(
                        theme.fg("muted", &format!("Full output: {path}")),
                    ));
                }
            }
        }
        RenderNode::Container(children)
    }
}

/// `renderResult(result, options, theme, context)`: the component for `result`, a
/// `{content, details}` value, under `opts` (`isPartial`, `isError`).
///
/// `details` that do not parse as [`CodemodeToolDetails`] (a result rejected before the script ran,
/// which carries none) list no calls, as `result.details?.calls ?? []` does.
#[must_use]
pub fn render_result(result: &Value, opts: &RenderOptions) -> Arc<dyn RenderedTree> {
    let details: Option<CodemodeToolDetails> = result
        .get("details")
        .and_then(|details| serde_json::from_value(details.clone()).ok());
    let blocks = result
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    // Drop the "Script completed\nWall time ...\nOutput:\n" header. Rejected input (invalid options)
    // has no header.
    let has_header = blocks.first().is_some_and(|first| {
        first.get("type").and_then(Value::as_str) == Some("text")
            && first
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(is_script_header)
    });
    let skip = usize::from(has_header);
    let output = blocks
        .iter()
        .skip(skip)
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .map(|block| {
            block
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    let (calls, full_output_path) = match details {
        Some(details) => (details.calls, details.full_output_path),
        None => (Vec::new(), None),
    };
    Arc::new(ResultComponent {
        calls,
        output,
        full_output_path,
        is_partial: opts.is_partial,
        is_error: opts.is_error,
    })
}

#[cfg(test)]
// A failed assertion in a test IS a panic; the no-panic policy is about the shipped surface.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests;
