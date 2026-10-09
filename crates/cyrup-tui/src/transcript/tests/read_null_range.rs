//! TOOL-056 — strict tool schemas make models send `null` for omitted optional fields, and pi's
//! `formatReadLineRange` treats that `null` as omitted (`renderers/read.ts:28-34` @v1.1.0, issue
//! #9996). These paint the real transcript rows, so both header sites that call
//! [`read_line_range`] are covered: the plain header and the collapsed compact one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::ansi::strip_ansi;
use crate::transcript::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;

/// Paint one live `read` row and return its text, escapes stripped.
fn render(args: serde_json::Value) -> String {
    let mut v = TranscriptView::new();
    v.set_cwd(Some(std::path::PathBuf::from("/tmp/aug-read-null")));
    v.push_tool_start("read", args);
    let theme = UiTheme::dark();
    let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
    term.draw(|frame| {
        let area = frame.area();
        v.render(frame, area, &theme);
    })
    .unwrap();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    strip_ansi(&out)
}

/// Mirrors pi `test/tool-execution-component.test.ts:307-321` @v1.1.0 ("renders read calls with
/// null offset and limit as full-file reads").
#[test]
fn renders_read_calls_with_null_offset_and_limit_as_full_file_reads() {
    let rendered = render(json!({ "path": "src/example.ts", "offset": null, "limit": null }));
    assert!(rendered.contains("read src/example.ts"), "{rendered}");
    assert!(!rendered.contains("src/example.ts:"), "{rendered}");
}

/// The compact header (`renderers/read.ts:99,108` @v1.1.0) runs the same rule.
#[test]
fn the_compact_read_header_drops_a_null_range_too() {
    let rendered =
        render(json!({ "path": "/tmp/aug-read-null/CLAUDE.md", "offset": null, "limit": null }));
    assert!(rendered.contains("read resource CLAUDE.md"), "{rendered}");
    assert!(!rendered.contains("CLAUDE.md:"), "{rendered}");

    // A real range still renders beside a `null` partner.
    let rendered = render(json!({ "path": "src/example.ts", "offset": 10, "limit": null }));
    assert!(rendered.contains("read src/example.ts:10"), "{rendered}");
    assert!(!rendered.contains("src/example.ts:10-"), "{rendered}");
}
