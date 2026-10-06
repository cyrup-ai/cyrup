//! CODE-014 — cyrup's prompt sections, diff and system-row bytes against pi v1.0.0's OWN output.
//!
//! `fixtures/pi/code014-prompt-sections.pi-captured.json` is what pi's real `buildSystemPromptSections`,
//! `diffSystemPromptSections`, agent loop and `getCurrentSystemMessage` printed for fixed inputs
//! (see `fixtures-capture/code014/README.md`). These tests run the same inputs through cyrup's
//! port and compare — the sections' text, the diff's JSON, and the BYTES of every system row.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{Message, Sections, SystemMessage};
use cyrup_resources::SkillPointer;
use cyrup_session::prompt::{
    ContextFile, ContextScope, DocsPointers, PromptInputs, SystemPromptBuilder,
    ToolPromptContribution, diff_system_prompt_sections,
};
use serde_json::Value;

fn capture() -> Value {
    serde_json::from_str(include_str!(
        "../../fixtures/pi/code014-prompt-sections.pi-captured.json"
    ))
    .expect("the capture is JSON")
}

fn arc(s: &str) -> Arc<str> {
    Arc::from(s)
}

/// The options `capture.ts` hands `buildSystemPromptSections`, in cyrup's terms.
fn inputs() -> PromptInputs {
    PromptInputs {
        selected_tools: Some(vec![arc("read"), arc("bash")]),
        tool_contributions: vec![
            ToolPromptContribution::snippet("read", "Read file contents")
                .with_guideline("Use read to examine files instead of cat or sed."),
            ToolPromptContribution::snippet("bash", "Execute bash commands"),
        ],
        prompt_guidelines: vec![arc("Keep answers short")],
        append_system_prompt: Some(arc("APPENDED TEXT")),
        cwd: PathBuf::from("/work/proj"),
        context_files: Arc::from(vec![
            ContextFile {
                path: PathBuf::from("/work/proj/AGENTS.md"),
                content: arc("house rules"),
                scope: ContextScope::Cwd,
            },
            ContextFile {
                path: PathBuf::from("/work/AGENTS.md"),
                content: arc("more rules"),
                scope: ContextScope::Cwd,
            },
        ]),
        skills: Arc::<[SkillPointer]>::from(Vec::new()),
        docs: DocsPointers {
            readme: Some(PathBuf::from("/pi/README.md")),
            docs: Some(PathBuf::from("/pi/docs")),
            examples: Some(PathBuf::from("/pi/examples")),
        },
        ..PromptInputs::default()
    }
}

fn text_of<'a>(sections: &'a Sections, name: &str) -> &'a str {
    sections
        .get(name)
        .flatten()
        .unwrap_or_else(|| panic!("section `{name}` missing"))
}

fn names(sections: &Sections) -> Vec<&str> {
    sections.iter().map(|(n, _)| n).collect()
}

fn pi_names(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

/// The ONLY places cyrup's text differs from pi's are `[CYRUP-DELTA]`s: the product name in the
/// preamble and the docs section, and the docs line that lists documentation cyrup does not ship.
/// Everything else — the names, their order, and the text of `tools`, `rules`, `addendum`,
/// `project_context` and `cwd` — must be pi's, byte for byte.
#[test]
fn the_default_sections_are_pis_apart_from_the_two_named_deltas() {
    let pi = capture();
    let ours = SystemPromptBuilder::new().build_sections(&inputs());

    assert_eq!(names(&ours), pi_names(&pi["sections"]), "names and order");
    for name in ["tools", "rules", "addendum", "project_context", "cwd"] {
        assert_eq!(
            text_of(&ours, name),
            pi["sections"][name].as_str().unwrap(),
            "section `{name}`"
        );
    }

    // docs: pi's text with the product name and the one line cyrup omits.
    let pi_docs = pi["sections"]["docs"].as_str().unwrap();
    let pi_docs: Vec<&str> = pi_docs
        .lines()
        .filter(|l| !l.starts_with("- When asked about:"))
        .collect();
    let ours_docs = text_of(&ours, "docs")
        .replace("cyrup documentation", "Pi documentation")
        .replace("cyrup", "pi");
    assert_eq!(ours_docs.lines().collect::<Vec<_>>(), pi_docs);
}

/// A custom prompt replaces the preamble and the default `tools`/`rules`/`docs`, and nothing else.
#[test]
fn a_custom_prompt_has_pis_sections() {
    let pi = capture();
    let ours = SystemPromptBuilder::new().build_sections(&PromptInputs {
        custom_prompt: Some(arc("MY OWN PROMPT")),
        ..inputs()
    });
    assert_eq!(names(&ours), pi_names(&pi["sectionsCustom"]));
    for (name, text) in pi["sectionsCustom"].as_object().unwrap() {
        assert_eq!(text_of(&ours, name), text.as_str().unwrap(), "`{name}`");
    }
}

/// No selected tool: pi's `(none)` placeholder, the two baseline rules, and a Windows cwd with its
/// backslashes turned into slashes.
#[test]
fn an_empty_tool_set_and_a_windows_cwd_are_pis() {
    let pi = capture();
    let ours = SystemPromptBuilder::new().build_sections(&PromptInputs {
        selected_tools: Some(Vec::new()),
        cwd: PathBuf::from("C:\\win\\dir"),
        ..PromptInputs::default()
    });
    // pi always emits `docs`; cyrup's guard leaves it out while the pointers are not wired (SESS-035).
    let pi_without_docs: Vec<&str> = pi_names(&pi["sectionsNone"])
        .into_iter()
        .filter(|n| *n != "docs")
        .collect();
    assert_eq!(names(&ours), pi_without_docs);
    for name in ["tools", "rules", "cwd"] {
        assert_eq!(
            text_of(&ours, name),
            pi["sectionsNone"][name].as_str().unwrap(),
            "`{name}`"
        );
    }
}

fn sections_of(v: &Value) -> Sections {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().map(str::to_owned)))
        .collect()
}

/// `diffSystemPromptSections`, on the pairs `capture.ts` runs through pi's: the same patch, key for
/// key and in the same order, and `undefined` exactly where cyrup says `None`.
#[test]
fn the_diff_is_pis_on_every_captured_pair() {
    let pi = capture();
    let prev = sections_of(&serde_json::json!({
        "preamble": "p", "tools": "t1", "gone": "g", "cwd": "c"
    }));
    let cur = sections_of(&serde_json::json!({
        "preamble": "p", "tools": "t2", "cwd": "c", "fresh": "f"
    }));
    let unknown_prev = sections_of(&serde_json::json!({
        "preamble": "pi", "experimental_pi_only": "<x>\nkept by pi\n</x>", "cwd": "<cwd>\n/w\n</cwd>"
    }));
    let unknown_cur = sections_of(&serde_json::json!({
        "preamble": "cyrup", "cwd": "<cwd>\n/w\n</cwd>"
    }));
    let empty = Sections::new();

    let cases: [(&str, Option<Sections>); 5] = [
        (
            "changedNewRemoved",
            diff_system_prompt_sections(&prev, &cur),
        ),
        ("identical", diff_system_prompt_sections(&cur, &cur)),
        ("bothEmpty", diff_system_prompt_sections(&empty, &empty)),
        ("fromEmpty", diff_system_prompt_sections(&empty, &cur)),
        (
            "unknownFromPi",
            diff_system_prompt_sections(&unknown_prev, &unknown_cur),
        ),
    ];
    for (key, ours) in cases {
        let want = &pi["diffs"][key];
        let got = match &ours {
            Some(patch) => serde_json::to_value(patch).unwrap(),
            None => Value::Null,
        };
        assert_eq!(got, *want, "diff `{key}`");
        // …and in the same key ORDER, which `Value` equality does not see.
        assert_eq!(
            serde_json::to_string(&got).unwrap(),
            serde_json::to_string(want).unwrap(),
            "diff `{key}`, as bytes"
        );
    }
}

/// The three system rows pi's loop wrote, and the snapshot a compaction entry carries, come back out
/// of cyrup as the very bytes pi's `JSON.stringify` printed.
#[test]
fn pis_system_rows_rewrite_byte_for_byte() {
    let pi = capture();
    let mut checked = 0;
    for key in ["loopPromptAndTools", "loopToolsOnly", "loopSwap"] {
        for row in pi[key].as_array().unwrap() {
            let row = row.as_str().unwrap();
            let message: Message = serde_json::from_str(row).unwrap();
            assert_eq!(serde_json::to_string(&message).unwrap(), row, "`{key}`");
            checked += 1;
        }
    }
    assert_eq!(checked, 3);

    let snapshot = pi["compactionSnapshot"].as_str().unwrap();
    let parsed: SystemMessage = serde_json::from_str(snapshot).unwrap();
    struct Replayed<'a>(&'a SystemMessage);
    impl serde::Serialize for Replayed<'_> {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            self.0.serialize_replayed(s)
        }
    }
    assert_eq!(
        serde_json::to_string(&Replayed(&parsed)).unwrap(),
        snapshot,
        "a compaction's `systemMessage` keeps pi's replay order"
    );
    // The two shapes really are different strings for the same fields, so neither check is vacuous.
    assert_ne!(
        serde_json::to_string(&parsed).unwrap(),
        snapshot,
        "a `message` row writes `timestamp` before `toolsAdded`"
    );
}
