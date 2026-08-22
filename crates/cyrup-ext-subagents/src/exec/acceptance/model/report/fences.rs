//! Fenced-block scanning, mirroring pi's own regexes (`acceptance.ts:459-482, 423-427,
//! 494-515`) — locating a fenced report and extracting balanced JSON from it.

use serde_json::Value;

// --------------------------------------------------------------------------------------------
// Fenced-block scanning mirroring pi's regexes (acceptance.ts:459-482, 423-427, 494-515)
// --------------------------------------------------------------------------------------------

pub(crate) struct FenceMatch {
    /// Byte offset of the start of the whole match (INCLUDING an optional leading `\n`, for the
    /// trailing-fence variant — [`with_leading_newline`] records whether that newline was
    /// consumed).
    pub(crate) index: usize,
    /// Byte offset immediately after the whole match (including trailing `\s*` for the
    /// strip variant).
    pub(crate) end: usize,
    pub(crate) tag: String,
    pub(crate) body: String,
}

/// Locate every fenced block whose opening tag (case-insensitively) is one of `tags`, mirroring
/// pi's `` /```${tag}\s*\n([\s\S]*?)```/gi ``. `with_leading_newline` extends `index` back over one
/// optional leading `\n` and `with_trailing_ws` extends `end` over a trailing `\s*` run — the two
/// extensions pi's `stripAcceptanceReport` regex adds over its `parseAcceptanceReport` one.
pub(crate) fn fenced_matches(
    text: &str,
    tags: &[&str],
    with_leading_newline: bool,
    with_trailing_ws: bool,
) -> Vec<FenceMatch> {
    let bytes = text.as_bytes();
    let mut matches = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = text.get(search_from..).and_then(|s| s.find("```")) {
        let fence_at = search_from + rel;
        let after_fence = fence_at + 3;
        // Read the tag token: characters up to the first whitespace/newline.
        let rest = text.get(after_fence..).unwrap_or("");
        let tag_end_rel = rest
            .find(|c: char| c.is_whitespace())
            .unwrap_or(rest.len());
        let tag = rest.get(..tag_end_rel).unwrap_or("").to_string();
        let tag_lower = tag.to_ascii_lowercase();
        if !tags.contains(&tag_lower.as_str()) {
            search_from = after_fence;
            continue;
        }
        // `\s*\n`: everything from tag end up to and including the first `\n` must be whitespace.
        let after_tag = after_fence + tag_end_rel;
        let after_tag_rest = text.get(after_tag..).unwrap_or("");
        let Some(nl_rel) = after_tag_rest.find('\n') else {
            search_from = after_fence;
            continue;
        };
        let inter = after_tag_rest.get(..nl_rel).unwrap_or("");
        if !inter.chars().all(char::is_whitespace) {
            search_from = after_fence;
            continue;
        }
        let body_start = after_tag + nl_rel + 1;
        // Body is non-greedy up to the next "```".
        let body_rest = text.get(body_start..).unwrap_or("");
        let Some(close_rel) = body_rest.find("```") else {
            search_from = after_fence;
            continue;
        };
        let close_at = body_start + close_rel;
        let body = body_rest.get(..close_rel).unwrap_or("").to_string();
        let mut end = close_at + 3;
        if with_trailing_ws {
            let tail = text.get(end..).unwrap_or("");
            let ws_len = tail
                .char_indices()
                .find(|(_, c)| !c.is_whitespace())
                .map(|(i, _)| i)
                .unwrap_or(tail.len());
            end += ws_len;
        }
        let mut index = fence_at;
        if with_leading_newline && fence_at > 0 && bytes.get(fence_at - 1) == Some(&b'\n') {
            index = fence_at - 1;
        }
        matches.push(FenceMatch {
            index,
            end,
            tag: tag_lower,
            body,
        });
        search_from = end.max(after_fence);
    }
    matches
}

/// `fencedBlocks(output, tag)` (acceptance.ts:423-427): every fenced block body (trimmed,
/// non-empty) for the given tags.
pub(crate) fn fenced_block_bodies(output: &str, tags: &[&str]) -> Vec<String> {
    fenced_matches(output, tags, false, false)
        .into_iter()
        .map(|m| m.body.trim().to_string())
        .filter(|body| !body.is_empty())
        .collect()
}

/// `extractBalancedJson` (acceptance.ts:459-482).
pub(crate) fn extract_balanced_json(text: &str, start: usize) -> Option<String> {
    let mut depth = 0i64;
    let mut in_string = false;
    let mut escaped = false;
    let mut end: Option<usize> = Option::None;
    for (offset, ch) in text.get(start..).unwrap_or("").char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            continue;
        }
        if ch == '{' {
            depth += 1;
        }
        if ch == '}' {
            depth -= 1;
            if depth == 0 {
                end = Some(start + offset + ch.len_utf8());
                break;
            }
        }
    }
    end.and_then(|e| text.get(start..e)).map(str::to_string)
}

/// `parseReportJson` (acceptance.ts:646-658).
pub(crate) fn parse_report_json(body: &str) -> Result<Value, String> {
    let trimmed = body.trim();
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => Ok(value),
        Err(err) => {
            if let Some(json_start) = trimmed.find('{')
                && json_start > 0
                    && let Some(json) = extract_balanced_json(trimmed, json_start) {
                        return serde_json::from_str::<Value>(&json)
                            .map_err(|e| e.to_string());
                    }
            Err(err.to_string())
        }
    }
}
