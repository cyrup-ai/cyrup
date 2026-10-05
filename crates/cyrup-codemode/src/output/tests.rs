#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value;

use super::*;

fn text(value: &str) -> OutputItem {
    OutputItem::Text(value.to_owned())
}

fn image() -> OutputItem {
    OutputItem::Image {
        data: "AAAA".into(),
        mime_type: "image/png".into(),
    }
}

fn never_spill(_: &str) -> Result<PathBuf, SpillError> {
    panic!("the output is within budget, so nothing is spilled");
}

fn fake_spill(_: &str) -> Result<PathBuf, SpillError> {
    Ok(PathBuf::from("/spill/pi-codemode-0011223344556677.txt"))
}

#[test]
fn the_default_budget_is_ten_thousand_tokens_at_four_characters_each() {
    assert_eq!(DEFAULT_MAX_OUTPUT_TOKENS, 10_000);
    assert_eq!(CHARS_PER_TOKEN, 4);
}

#[test]
fn output_at_the_budget_passes_through_untouched() {
    // 5 tokens = 20 characters; exactly 20 is "not over".
    let items = vec![text("12345678901234567890"), image()];
    let out = truncate_output(items.clone(), 5, never_spill);
    assert_eq!(
        out,
        TruncatedOutput {
            items,
            full_output_path: None
        }
    );
}

#[test]
fn output_one_character_over_the_budget_becomes_one_head_and_tail_item() {
    let combined = "12345678901234567890X";
    let out = truncate_output(vec![text(combined)], 5, fake_spill);
    // budget 20: head 10, tail 10, removed 1 character -> ceil(1/4) = 1 token; original 21 -> 6.
    let expected = "Warning: truncated output (original token count: 6)\nTotal output lines: 1\n\n1234567890…1 tokens truncated…234567890X\n\n[Full output: /spill/pi-codemode-0011223344556677.txt (read with offset/limit)]";
    assert_eq!(
        out,
        TruncatedOutput {
            items: vec![text(expected)],
            full_output_path: Some(PathBuf::from("/spill/pi-codemode-0011223344556677.txt")),
        }
    );
}

#[test]
fn text_items_are_joined_with_a_newline_and_images_follow_the_single_text_item() {
    let items = vec![text("aaaa"), image(), text("bbbb\ncccc"), image()];
    let plan = plan_truncation(items, 1);
    let OutputPlan::Truncated(plan) = plan else {
        panic!("expected a truncation");
    };
    assert_eq!(plan.full_text(), "aaaa\nbbbb\ncccc");
    let out = plan.finish(Ok(PathBuf::from("/p")));
    // budget 4: head "aa", tail "cc"; 14 characters -> 4 tokens; removed 10 -> 3 tokens; 3 lines.
    assert_eq!(
        out.items,
        vec![
            text(
                "Warning: truncated output (original token count: 4)\nTotal output lines: 3\n\naa…3 tokens truncated…cc\n\n[Full output: /p (read with offset/limit)]"
            ),
            image(),
            image(),
        ]
    );
}

#[test]
fn a_zero_budget_keeps_neither_head_nor_tail() {
    let out = truncate_output(vec![text("abcdefgh")], 0, fake_spill);
    let OutputItem::Text(body) = &out.items[0] else {
        panic!("text expected");
    };
    assert!(
        body.contains("\n\n…2 tokens truncated…\n\n[Full output:"),
        "{body}"
    );
}

#[test]
fn head_and_tail_each_keep_half_the_budget() {
    // A 3-token budget is 12 characters: head 6, tail 6, and the 5 between them are 2 tokens.
    let out = truncate_output(vec![text("abcdefghijklmnopq")], 3, fake_spill);
    let OutputItem::Text(body) = &out.items[0] else {
        panic!("text expected");
    };
    assert!(
        body.contains("\n\nabcdef…2 tokens truncated…lmnopq\n\n"),
        "{body}"
    );
}

#[test]
fn no_text_means_no_truncation_even_with_a_zero_budget() {
    let items = vec![image()];
    assert_eq!(
        truncate_output(items.clone(), 0, never_spill),
        TruncatedOutput {
            items,
            full_output_path: None
        }
    );
    assert_eq!(
        truncate_output(Vec::new(), 0, never_spill),
        TruncatedOutput {
            items: Vec::new(),
            full_output_path: None
        }
    );
}

#[test]
fn length_is_counted_in_utf16_units_not_characters_or_bytes() {
    // Each emoji is 2 UTF-16 units: 5 of them are 10 units, over a 2-token (8 unit) budget.
    let over = truncate_output(vec![text("😀😀😀😀😀")], 2, fake_spill);
    assert!(
        matches!(&over.items[0], OutputItem::Text(body) if body.starts_with("Warning: truncated output (original token count: 3)"))
    );
    // 4 emoji are 8 units: exactly the budget, so untouched; as bytes (16) or scalars (4) it would differ.
    let items = vec![text("😀😀😀😀")];
    assert_eq!(truncate_output(items.clone(), 2, never_spill).items, items);
}

#[test]
fn a_cut_inside_a_surrogate_pair_leaves_the_replacement_character() {
    // 8 units, budget 4: head 2 units and tail 2 units are whole emoji.
    let out = truncate_output(vec![text("😀😀😀😀")], 1, fake_spill);
    let OutputItem::Text(body) = &out.items[0] else {
        panic!("text expected")
    };
    assert!(body.contains("\n\n😀…1 tokens truncated…😀\n\n"), "{body}");
    // 7 units, budget 4: head 2 units are `a` and the high half of the first emoji, which cannot
    // be a `String`; upstream leaves a lone surrogate there, which UTF-8 turns into U+FFFD.
    let shifted = truncate_output(vec![text("a😀😀😀")], 1, fake_spill);
    let OutputItem::Text(body) = &shifted.items[0] else {
        panic!("text expected")
    };
    assert!(
        body.contains("\n\na\u{fffd}…1 tokens truncated…😀\n\n"),
        "{body}"
    );
}

#[test]
fn a_failed_spill_still_returns_the_truncated_output_with_the_reason() {
    let out = truncate_output(vec![text("abcdefghijklmnop")], 1, |_| {
        Err(SpillError::Write(io::Error::other("disk is full")))
    });
    assert_eq!(out.full_output_path, None);
    let OutputItem::Text(body) = &out.items[0] else {
        panic!("text expected")
    };
    assert!(
        body.ends_with("\n\n[Could not save the full output: disk is full]"),
        "{body}"
    );
    assert!(body.starts_with("Warning: truncated output (original token count: 4)"));
}

#[test]
fn the_spill_receives_the_full_text() {
    let mut received = None;
    let _ = truncate_output(vec![text("abcdefghijkl"), text("mnop")], 1, |full| {
        received = Some(full.to_owned());
        Ok(PathBuf::from("/p"))
    });
    assert_eq!(received.as_deref(), Some("abcdefghijkl\nmnop"));
}

#[test]
fn a_budget_beyond_u64_range_does_not_overflow() {
    let items = vec![text("abc")];
    assert_eq!(
        truncate_output(items.clone(), u64::MAX, never_spill).items,
        items
    );
    // 2^62 tokens * 4 characters is 2^64: a wrapping product would be a zero budget.
    assert_eq!(
        truncate_output(items.clone(), 1 << 62, never_spill).items,
        items
    );
}

#[test]
fn spill_file_names_are_sixteen_lowercase_hex_digits() {
    assert_eq!(
        spill_file_name([0x00, 0x11, 0x22, 0xab, 0xcd, 0xef, 0x0f, 0xf0]),
        "pi-codemode-001122abcdef0ff0.txt"
    );
}

#[test]
fn spill_writes_the_full_text_to_a_file_the_path_points_at() {
    let dir = tempfile::tempdir().unwrap();
    let path = spill_output(dir.path(), "full\ntext é 😀").unwrap();
    assert_eq!(path.parent().unwrap(), dir.path());
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("pi-codemode-") && name.ends_with(".txt"),
        "{name}"
    );
    let hex = &name["pi-codemode-".len()..name.len() - ".txt".len()];
    assert_eq!(hex.len(), 16);
    assert!(
        hex.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "{hex}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "full\ntext é 😀");
}

#[test]
fn two_spills_get_different_names() {
    let dir = tempfile::tempdir().unwrap();
    let a = spill_output(dir.path(), "a").unwrap();
    let b = spill_output(dir.path(), "b").unwrap();
    assert_ne!(a, b);
}

#[test]
fn spill_does_not_overwrite_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let token = [1, 2, 3, 4, 5, 6, 7, 8];
    write_spill(dir.path(), token, "first").unwrap();
    let second = write_spill(dir.path(), token, "second");
    assert!(
        matches!(second, Err(SpillError::Write(error)) if error.kind() == io::ErrorKind::AlreadyExists)
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(spill_file_name(token))).unwrap(),
        "first"
    );
}

#[test]
fn a_missing_directory_fails_the_spill_and_truncation_still_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("does-not-exist");
    assert!(matches!(
        spill_output(&missing, "x"),
        Err(SpillError::Write(_))
    ));
    let out = truncate_output(vec![text("abcdefghijklmnop")], 1, |full| {
        spill_output(&missing, full)
    });
    assert_eq!(out.full_output_path, None);
    let OutputItem::Text(body) = &out.items[0] else {
        panic!("text expected")
    };
    assert!(body.contains("[Could not save the full output: "), "{body}");
}

#[test]
fn a_truncated_output_end_to_end_leaves_the_full_text_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let full = "x".repeat(100);
    let out = truncate_output(vec![text(&full)], 5, |text| spill_output(dir.path(), text));
    let path = out.full_output_path.clone().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), full);
    let OutputItem::Text(body) = &out.items[0] else {
        panic!("text expected")
    };
    assert!(body.ends_with(&format!(
        "\n\n[Full output: {} (read with offset/limit)]",
        path.display()
    )));
}

// ---- byte parity with upstream's truncateOutput (execute.ts:277-300 @v1.0.1) ----

fn items_from(value: &Value) -> Vec<OutputItem> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| match item["type"].as_str().unwrap() {
            "text" => OutputItem::Text(item["text"].as_str().unwrap().to_owned()),
            _ => OutputItem::Image {
                data: item["data"].as_str().unwrap().to_owned(),
                mime_type: item["mimeType"].as_str().unwrap().to_owned(),
            },
        })
        .collect()
}

#[test]
fn truncation_agrees_with_upstream_on_the_corpus() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../testdata/truncate.json")).unwrap();
    let (mut truncated, mut passed) = (0, 0);
    for case in &cases {
        let items = items_from(&case["items"]);
        let max_tokens = case["maxTokens"].as_u64().unwrap();
        let want = items_from(&case["out"]);
        let mut spilled = None;
        let out = truncate_output(items.clone(), max_tokens, |full| {
            spilled = Some(full.to_owned());
            Ok(PathBuf::from("/spill/pi-codemode-0011223344556677.txt"))
        });
        assert_eq!(out.items, want, "{case}");
        match case["full"].as_str() {
            Some(full) => {
                assert_eq!(spilled.as_deref(), Some(full), "{case}");
                truncated += 1;
            }
            None => {
                assert_eq!(spilled, None, "{case}");
                assert_eq!(out.items, items);
                passed += 1;
            }
        }
    }
    assert!(truncated > 50 && passed > 50, "{truncated} {passed}");
}
