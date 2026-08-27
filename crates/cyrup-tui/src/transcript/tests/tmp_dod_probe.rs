#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
use serde_json::json;

use crate::transcript::*;

fn txt(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}
fn joined(lines: &[Line<'static>]) -> String {
    lines.iter().map(txt).collect::<Vec<_>>().join("\n")
}
fn run_lines(name: &str, args: Value, expanded: bool, opts: ImageOpts<'_>) -> Vec<Line<'static>> {
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    view.push_tool_start(name, args);
    let run = view.active_tools()[0].clone();
    tool_lines(&run, expanded, 100, &theme, opts).into_iter().collect()
}
fn header(args: Value) -> String {
    let cwd = std::path::Path::new("/w/project");
    let opts = ImageOpts { cwd: Some(cwd), ..ImageOpts::default() };
    let lines = run_lines("read", args, false, opts);
    joined(&lines).lines().map(str::trim).find(|l| !l.is_empty()).unwrap().to_string()
}

#[test]
fn dod_probe() {
    // 1
    assert_eq!(header(json!({"path":"f.txt","offset":2.0,"limit":3.0})), "read f.txt:2-4");
    assert_eq!(header(json!({"path":"f.txt","offset":2,"limit":3})), "read f.txt:2-4");
    // 2
    assert_eq!(header(json!({"path":"f.txt","offset":2.0})), "read f.txt:2");
    assert_eq!(header(json!({"path":"f.txt","limit":3.0})), "read f.txt:1-3");
    // 3
    assert_eq!(header(json!({"path":"f.txt","offset":2})), "read f.txt:2");
    assert_eq!(header(json!({"path":"f.txt","limit":3})), "read f.txt:1-3");
    assert_eq!(header(json!({"path":"f.txt","offset":0})), "read f.txt:0");
    assert_eq!(header(json!({"path":"f.txt","offset":-1})), "read f.txt:-1");
    assert_eq!(header(json!({"path":"f.txt"})), "read f.txt");
    assert_eq!(header(json!({"path":"f.txt","offset":2,"limit":3.0})), "read f.txt:2-4");
    // 4
    assert_eq!(header(json!({"path":"f.txt","offset":2.5,"limit":3})), "read f.txt:2.5-4.5");
    // 5
    assert_eq!(header(json!({"path":"f.txt","offset":1,"limit":0})), "read f.txt:1");
    // 6
    assert_eq!(header(json!({"path":"f.txt","offset":-0.0})), "read f.txt:0");
    // 7
    assert_eq!(header(json!({"path":"f.txt","offset":null})), "read f.txt");
    assert_eq!(header(json!({"path":"f.txt","offset":"2"})), "read f.txt");
    assert_eq!(header(json!({"path":"f.txt","limit":[]})), "read f.txt");
    // 8 compact
    let cwd = std::path::Path::new("/w/project");
    let opts = ImageOpts { cwd: Some(cwd), ..ImageOpts::default() };
    let sk = run_lines("read", json!({"path":"x/SKILL.md","offset":2.0,"limit":3.0}), false, opts);
    assert!(joined(&sk).contains("[skill] x:2-4 (ctrl+o to expand)"), "{}", joined(&sk));
    let rs = run_lines("read", json!({"path":"AGENTS.md","offset":2.0,"limit":3.0}), false, opts);
    assert!(
        joined(&rs).contains("read resource AGENTS.md:2-4 (ctrl+o to expand)"),
        "{}",
        joined(&rs)
    );
    // 9 expanded
    let ex = run_lines("read", json!({"path":"x/SKILL.md","offset":2.0,"limit":3.0}), true, opts);
    assert!(joined(&ex).contains("read x/SKILL.md:2-4"), "{}", joined(&ex));
    // 10 bash timeout
    let b1 = run_lines("bash", json!({"command":"ls","timeout":120}), false, opts);
    assert!(joined(&b1).contains("(timeout 120s)"), "{}", joined(&b1));
    let b2 = run_lines("bash", json!({"command":"ls","timeout":1.5}), false, opts);
    assert!(joined(&b2).contains("(timeout 1.5s)"), "{}", joined(&b2));
    let b3 = run_lines("bash", json!({"command":"ls","timeout":0}), false, opts);
    assert!(!joined(&b3).contains("timeout"), "{}", joined(&b3));
    // 11 grep/ls/find untouched
    let g = run_lines("grep", json!({"pattern":"x","limit":5}), false, opts);
    println!("GREP: {}", joined(&g));
    let l = run_lines("ls", json!({"path":".","limit":5}), false, opts);
    println!("LS: {}", joined(&l));
    let f = run_lines("find", json!({"pattern":"*","limit":5}), false, opts);
    println!("FIND: {}", joined(&f));
    // warning style on the suffix span
    let theme = UiTheme::dark();
    let plain = run_lines("read", json!({"path":"f.txt","offset":2.0,"limit":3.0}), false, opts);
    let h = plain.iter().find(|l| txt(l).contains("f.txt")).unwrap();
    let last = h.spans.iter().find(|s| s.content.as_ref() == ":2-4").expect("suffix span");
    assert_eq!(last.style, theme.warning_style());
}
