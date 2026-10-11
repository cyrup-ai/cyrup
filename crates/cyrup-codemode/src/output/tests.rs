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
    let expected = "Warning: truncated output (original token count: 6)\nTotal output lines: 1\n\n1234567890…1 tokens truncated…234567890X\n\n[Full output: /spill/pi-codemode-0011223344556677.txt (read or tools.read with offset/limit)]";
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
                "Warning: truncated output (original token count: 4)\nTotal output lines: 3\n\naa…3 tokens truncated…cc\n\n[Full output: /p (read or tools.read with offset/limit)]"
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
        "\n\n[Full output: {} (read or tools.read with offset/limit)]",
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
        // [CYRUP-DELTA] The recorded footer is upstream's "(read with offset/limit)".
        let want: Vec<OutputItem> = items_from(&case["out"])
            .into_iter()
            .map(|item| match item {
                OutputItem::Text(text) => OutputItem::Text(text.replace(
                    "(read with offset/limit)",
                    "(read or tools.read with offset/limit)",
                )),
                other => other,
            })
            .collect();
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

// ── CODE-019 (pi `d677d0ee7` @v1.0.3): saved images and private output files ─────────────────────

fn png(data: &str) -> OutputItem {
    OutputItem::Image {
        data: data.into(),
        mime_type: "image/png".into(),
    }
}

/// `AAAA` is three zero bytes.
const THREE_BYTES: &str = "AAAA";

#[test]
fn each_image_follows_a_text_item_naming_the_file_it_was_saved_to() {
    let mut saved: Vec<(String, Vec<u8>)> = Vec::new();
    let out = label_images(
        vec![text("a"), png(THREE_BYTES), text("b")],
        |mime, bytes| {
            saved.push((mime.to_owned(), bytes.to_vec()));
            Ok(PathBuf::from("/tmp/pi-codemode-0011223344556677.png"))
        },
    )
    .unwrap();
    assert_eq!(
        out,
        vec![
            text("a"),
            text("[Image saved to /tmp/pi-codemode-0011223344556677.png (image/png, 3B)]"),
            png(THREE_BYTES),
            text("b"),
        ]
    );
    assert_eq!(saved, vec![("image/png".to_owned(), vec![0, 0, 0])]);
}

#[test]
fn an_image_shown_twice_is_saved_once_and_both_copies_name_the_same_file() {
    let mut calls = 0;
    let out = label_images(vec![png(THREE_BYTES), png(THREE_BYTES)], |_, _| {
        calls += 1;
        Ok(PathBuf::from("/tmp/one.png"))
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(out[0], out[2]);
    assert_eq!(out.len(), 4);
}

#[test]
fn a_failed_write_becomes_the_label_and_the_image_stays() {
    let out = label_images(vec![png(THREE_BYTES)], |_, _| {
        Err(SpillError::Write(io::Error::other("disk full")))
    })
    .unwrap();
    assert_eq!(
        out,
        vec![
            text("[Image (image/png, 3B) could not be saved: disk full]"),
            png(THREE_BYTES)
        ]
    );
}

#[test]
fn a_type_with_no_extension_is_refused_before_anything_is_written() {
    let bmp = OutputItem::Image {
        data: THREE_BYTES.into(),
        mime_type: "image/bmp".into(),
    };
    let result = label_images(vec![text("x"), bmp], |_, _| panic!("nothing is written"));
    assert!(matches!(result, Err(ImageLabelError::NoExtension(ref mime)) if mime == "image/bmp"));
    assert_eq!(
        result.unwrap_err().to_string(),
        "No file extension for image type image/bmp"
    );
}

#[test]
fn the_four_image_types_image_accepts_have_their_upstream_extensions() {
    assert_eq!(image_extension("image/png"), Some(".png"));
    assert_eq!(image_extension("image/jpeg"), Some(".jpg"));
    assert_eq!(image_extension("image/gif"), Some(".gif"));
    assert_eq!(image_extension("image/webp"), Some(".webp"));
    assert_eq!(image_extension("image/svg+xml"), None);
}

#[test]
fn sizes_read_as_upstreams_format_size_with_to_fixed_ties_away_from_zero() {
    assert_eq!(format_size(0), "0B");
    assert_eq!(format_size(1023), "1023B");
    assert_eq!(format_size(1024), "1.0KB");
    // 1.25 KB is a tie: `toFixed(1)` gives 1.3 where `{:.1}` gives 1.2.
    assert_eq!(format_size(1280), "1.3KB");
    assert_eq!(format_size(1024 * 1024 - 1), "1024.0KB");
    assert_eq!(format_size(1024 * 1024), "1.0MB");
    assert_eq!(format_size(5 * 1024 * 1024 + 512 * 1024), "5.5MB");
}

#[test]
fn a_saved_image_is_a_new_file_readable_only_by_its_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = save_image_output(dir.path(), "image/png", &[1, 2, 3]).unwrap();
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("pi-codemode-")
            && name.ends_with(".png")
            && name.len() == "pi-codemode-".len() + 16 + ".png".len(),
        "{name}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), [1, 2, 3]);
    assert_owner_only(&path);
    assert!(save_image_output(dir.path(), "image/bmp", &[1]).is_err());
}

#[test]
fn the_text_spill_is_readable_only_by_its_owner_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = spill_output(dir.path(), "full text").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "full text");
    assert_owner_only(&path);
}

#[test]
fn an_output_file_is_never_written_through_a_path_someone_else_placed() {
    let dir = tempfile::tempdir().unwrap();
    let token = [9u8; 8];
    let planted = dir
        .path()
        .join(output_file_name("pi-codemode", token, ".png"));
    std::fs::write(&planted, "theirs").unwrap();
    let result = write_output_file_named(dir.path(), "pi-codemode", ".png", token, b"ours");
    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(&planted).unwrap(), "theirs");
}

fn console(value: &str) -> OutputItem {
    OutputItem::Console(value.to_owned())
}

/// `formatOutput` then `joinAdjacentText`, as the execute path applies them, rendered as the one
/// text the model reads (pi `agent-session-codemode.test.ts:396-418` @v1.1.0).
#[test]
fn several_text_items_get_numbered_headers_and_console_lines_go_last_in_one_block() {
    let laid_out = join_adjacent_text(format_output(vec![
        text("one\ntwo"),
        console("a"),
        console("b"),
        text("three\n"),
        text("4"),
    ]));
    assert_eq!(
        laid_out,
        vec![text(
            "==> text 1/3 <==\none\ntwo\n==> text 2/3 <==\nthree\n==> text 3/3 <==\n4\n<console_output>\na\nb\n</console_output>"
        )]
    );
}

#[test]
fn a_single_text_item_gets_no_header_and_images_keep_their_place() {
    assert_eq!(
        format_output(vec![text("only"), image(), console("log")]),
        vec![
            text("only"),
            image(),
            text("<console_output>\nlog\n</console_output>")
        ]
    );
    assert_eq!(
        format_output(vec![text("a"), image(), text("b")]),
        vec![
            text("==> text 1/2 <==\na"),
            image(),
            text("==> text 2/2 <==\nb")
        ]
    );
    assert_eq!(format_output(Vec::new()), Vec::new());
}

#[test]
fn join_adjacent_text_adds_a_newline_only_where_a_part_does_not_end_one() {
    assert_eq!(
        join_adjacent_text(vec![
            text("a"),
            text("b\n"),
            text("c"),
            image(),
            text(""),
            text("d"),
            image(),
        ]),
        vec![text("a\nb\nc"), image(), text("d"), image()]
    );
}

/// Mode bits exist on Unix only (`OUTPUT_FILE_MODE` is ignored on Windows upstream too).
fn assert_owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}: {mode:o}", path.display());
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn is_recorded(path: &Path) -> bool {
    cyrup_core::spilled_files::any_recorded(|known| known == path)
}

/// The result names these files and tells the model to read them; a permission policy that guards
/// reads outside the project needs the record to tell them from any other file in the temp
/// directory.
#[test]
fn a_spill_and_a_saved_image_are_recorded_as_files_the_model_may_read() {
    let dir = tempfile::tempdir().unwrap();
    let spill = spill_output(dir.path(), "full text").unwrap();
    let image = save_image_output(dir.path(), "image/png", &[1, 2, 3]).unwrap();
    assert!(is_recorded(&spill), "{spill:?}");
    assert!(is_recorded(&image), "{image:?}");
    // A neighbour nobody wrote through here is not.
    assert!(!is_recorded(&dir.path().join(spill_file_name([9; 8]))));
}

#[test]
fn a_spill_that_could_not_be_written_is_not_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let token = [4, 4, 4, 4, 4, 4, 4, 4];
    let missing = dir.path().join("does-not-exist");
    assert!(write_spill(&missing, token, "x").is_err());
    assert!(!is_recorded(&missing.join(spill_file_name(token))));
}
