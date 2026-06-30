//! Standalone-HTML session export (spec/tui/04; Pi `core/export-html/index.ts`
//! `exportSessionToHtml`, wired from `interactive-mode.ts:5102-5116` `handleExportCommand`).
//!
//! Pi's `/export <path>` chooses the format **by extension**: a `.jsonl` target writes the raw
//! transcript (`exportToJsonl`); **any other** target (including no path) writes a styled HTML
//! document (`exportToHtml`, the *default*). cyrup mirrors that routing in `app.rs::execute_command`;
//! this module renders the HTML body.
//!
//! The session already serializes its transcript to JSONL (`cyrup-session::manager::export_jsonl`):
//! a header line plus one line per [`Entry`](cyrup_session::entry::Entry). Rather than re-derive the
//! L5 entry schema here, [`session_jsonl_to_html`] walks each line as a generic `serde_json::Value`,
//! pulls the entry `type`/`role` and every `text` string it carries, HTML-escapes them, and lays them
//! out as per-message sections under a small self-contained CSS scaffold. Pi's *rich* tool-call cards
//! (`export-html/tool-renderer.ts`) are the one residual the L5 `export_to_html` would add; the
//! document this produces is a real, openable transcript.

use serde_json::Value;

/// Escape the five HTML-significant characters so arbitrary transcript text renders literally.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Recursively collect every string under a `"text"` key, in document order (the transcript's
/// human-readable content lives in `text` fields across user/assistant/tool entries).
fn collect_text(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "text"
                    && let Value::String(s) = v
                    && !s.is_empty()
                {
                    out.push(s.clone());
                } else {
                    collect_text(v, out);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_text(item, out);
            }
        }
        _ => {}
    }
}

/// The entry's display role/type for the section header (`type` tag, falling back to `role`).
fn entry_role(value: &Value) -> String {
    value
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| value.get("role").and_then(Value::as_str))
        .unwrap_or("entry")
        .to_string()
}

/// Render the session JSONL (`export_jsonl` output: header line + one entry per line) into a
/// standalone HTML document. The first line is the header (title/cwd); subsequent lines are entries.
/// Unparseable lines are skipped (never panics).
pub fn session_jsonl_to_html(jsonl: &str) -> String {
    let mut lines = jsonl.lines().filter(|l| !l.trim().is_empty());
    let header: Option<Value> = lines.next().and_then(|l| serde_json::from_str(l).ok());
    let title = header
        .as_ref()
        .and_then(|h| h.get("name").or_else(|| h.get("title")).and_then(Value::as_str))
        .unwrap_or("Session")
        .to_string();
    let cwd = header
        .as_ref()
        .and_then(|h| h.get("cwd").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();

    let mut body = String::new();
    for line in lines {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        let role = entry_role(&value);
        let mut texts = Vec::new();
        collect_text(&value, &mut texts);
        if texts.is_empty() {
            continue;
        }
        let role_class = escape(&role);
        body.push_str(&format!(
            "<section class=\"entry {role_class}\">\n  <header class=\"role\">{}</header>\n",
            escape(&role)
        ));
        for text in texts {
            body.push_str(&format!("  <pre class=\"text\">{}</pre>\n", escape(&text)));
        }
        body.push_str("</section>\n");
    }

    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title_esc}</title>\n<style>\n{css}\n</style>\n</head>\n<body>\n\
         <h1>{title_esc}</h1>\n{cwd_block}<main>\n{body}</main>\n</body>\n</html>\n",
        title_esc = escape(&title),
        css = EXPORT_CSS,
        cwd_block = if cwd.is_empty() {
            String::new()
        } else {
            format!("<p class=\"cwd\">{}</p>\n", escape(&cwd))
        },
        body = body,
    )
}

/// Minimal self-contained stylesheet for the exported document (dark transcript, role-tinted blocks).
const EXPORT_CSS: &str = "\
body{font-family:-apple-system,Segoe UI,Roboto,sans-serif;max-width:48rem;margin:2rem auto;\
padding:0 1rem;background:#1e1e2e;color:#cdd6f4;line-height:1.5}\
h1{font-size:1.4rem}.cwd{color:#9399b2;font-size:.85rem;margin-top:-.5rem}\
section.entry{border-left:3px solid #45475a;padding:.25rem .75rem;margin:1rem 0;background:#181825;\
border-radius:.25rem}.role{font-weight:600;text-transform:capitalize;color:#89b4fa;font-size:.8rem;\
letter-spacing:.03em;margin-bottom:.25rem}section.user .role{color:#a6e3a1}\
section.assistant .role{color:#89b4fa}pre.text{white-space:pre-wrap;word-break:break-word;margin:.25rem 0;\
font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.85rem}";
