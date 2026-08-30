---
stage: qa
status: completed
type: implementation
updated: 2026-08-30 02:35
---

# `edit` tool — regression guards for the two `[CYRUP-DELTA]`s in `edit_diff.rs`

> **Note on this augmentation.** `/aug` normally strips language calling for tests. That rule
> is not applied here: QA reduced this file to exactly one outstanding item, and that item *is*
> the guards. Applying the rule would delete the task rather than sharpen it. Everything else
> `/aug` asks for — sources, verified values, a single prescribed path — is below.

---

## 1. Settled — do not redo

The matcher work shipped and was accepted at QA (9/10). In
[`edit_diff.rs`](../../crates/cyrup-tools/src/tools/edit_diff.rs): the line-anchored tier
(`line_anchored_matches` **:325**), its uniform-offset bound (**:397-402**), the uniqueness
fallback (**:809**), `reindent` (**:436**), and the near-miss diagnostic (`nearest_region`
**:698**, `err_not_found` **:726**). `normalize_for_fuzzy` and the overlay's line-count guard
are untouched. Research write-up and prescription: commit `06c23fc`.

**Accepted deviation — do not revert.** `reindent` takes `Option<&str>`, not the prescribed
`&str` with an empty-string sentinel. `Some("")` is a real line-anchored outcome (a needle
authored *more* indented than the file dedents to a zero delta) and must still re-base;
the sentinel silently skipped it and wrote at a depth the file never had (`ce78c6c`).

**Two measured properties that are NOT defects** — recorded so they are not re-litigated:
`nearest_region` is O(lines × needle) and costs ~1.7 s at 50k lines with a 40-line needle in
release; and it is indentation-blind, because `TextDiff::ratio()` at line granularity scores a
candidate that differs only in indent at ~0 — matching aider's `find_similar_lines`, which
also compares unnormalized lines
([`editblock_coder.py:602`](../../tmp/aider/aider/coders/editblock_coder.py)).

---

## 2. Why guards, and what "guard" means here

`edit_diff.rs`'s own `mod tests` (**:910-1069**) pins **eleven** behaviours of this matcher —
`exact_multi_edit`, `fuzzy_curly_quote_and_dash_and_trailing_ws`,
`fuzzy_nfkc_ligature_and_fullwidth`, `not_found_error_is_indexed_for_multi`, `duplicate_error`,
`empty_old_text_error`, `no_change_error`, `overlap_error`, `bom_and_crlf_roundtrip`,
`detect_line_ending_first_wins_and_cr_only_folds`, `patch_and_first_line`. Every prior
behaviour is covered. The two new ones are covered by nothing.

The risk is demonstrated, not hypothetical: the `&str` sentinel in §1 was written, compiled,
clippy-clean, committed and pushed before the corruption was found.

**House style, taken from the module itself** (**:912-1069**):

- `mod tests` already carries `#[allow(clippy::unwrap_used, clippy::indexing_slicing)]`
  (**:911**), so `.unwrap()` / `.unwrap_err()` are the idiom — see `duplicate_error`.
- Drive the public `apply_edits_to_normalized_content`, not the private helpers. All eleven
  existing behaviour tests do; so should these, and all eight below are reachable that way.
- Names state the **behaviour**, not the mechanism (`not_found_error_is_indexed_for_multi`).
- Failure messages interpolate the value: `assert!(cond, "got: {}", e.0)`.
- Comments cite the upstream line they preserve or diverge from (`edit-diff.ts:NN`).

**RED lever — the repo's convention, and it is executed, not predicted.** The precedent is
[`MEDIUM-delta-cyrup-tools-src-tools-bash-rs-72.md:15-30`](../done/2026-08-23-00-08/MEDIUM-delta-cyrup-tools-src-tools-bash-rs-72.md),
which records: what was reverted, **which named test failed at which `file:line`**, with what
message, and how many of N tests. It also records a lever prediction that turned out **false**
and says so. §4 below predicts a lever for each guard; those predictions are inputs to be
executed, and any that is disproved is written down as disproved, not quietly dropped.

---

## 3. The eight guards

Append to `mod tests`, after `overlap_error` (**:1060-1063**), before the closing brace at
**:1069**. Every expected value below was produced by running the current code — none is
predicted.

```rust
    /// [CYRUP-DELTA] Tier 3 (`line_anchored_matches`). Pi's `fuzzyFindText`
    /// (edit-diff.ts:206-244) stops after the normalized-buffer pass, so an `oldText` that is
    /// the right code at the wrong indent depth is *not found* upstream and is found here.
    #[test]
    fn line_anchored_tier_rebases_to_the_files_own_indent() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\n";
        let want = "mod a {\n    fn foo() {\n        baz();\n    }\n}\n";
        // Authored with no indentation at all.
        let edits = vec![(
            "fn foo() {\n    bar();\n}".to_string(),
            "fn foo() {\n    baz();\n}".to_string(),
        )];
        let r = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap();
        assert_eq!(r.new_content, want);
        // Authored carrying only SOME of it — 2 spaces where the file has 4. The needle is
        // outdented by its own minimum first, so both spellings land the same
        // (aider `replace_part_with_missing_leading_whitespace`, editblock_coder.py:248-255).
        let edits = vec![(
            "  fn foo() {\n      bar();\n  }".to_string(),
            "  fn foo() {\n      baz();\n  }".to_string(),
        )];
        let r = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap();
        assert_eq!(r.new_content, want);
    }

    /// The false-apply bound. The indentation delta must be ONE value across every non-blank
    /// line; a block whose tokens coincide at ragged depths is refused, never rewritten.
    #[test]
    fn ragged_indentation_is_refused_rather_than_mangled() {
        // Same tokens as the needle, but at depths 4, 12 and 6 — no single offset fits.
        let content = "mod a {\n    fn foo() {\n            bar();\n      }\n}\n";
        let edits = vec![("fn foo() {\n    bar();\n}".to_string(), "X".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap_err();
        assert!(
            e.0.starts_with("Could not find the exact text in f.rs."),
            "got: {}",
            e.0
        );
    }

    /// `count_occurrences` only sees normalized SUBSTRING occurrences and reports 0 for a
    /// line-anchored match, so the uniqueness rule — the other half of the false-apply bound —
    /// has to be fed from the tier that actually matched.
    #[test]
    fn two_indent_different_copies_are_a_duplicate_not_a_write() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\nmod b {\n        fn foo() {\n            bar();\n        }\n}\n";
        let edits = vec![("fn foo() {\n    bar();\n}".to_string(), "X".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap_err();
        assert!(
            e.0.contains("Found 2 occurrences of the text in f.rs"),
            "got: {}",
            e.0
        );
    }

    /// A needle authored MORE indented than the file dedents to a zero delta. That is a real
    /// line-anchored match, not the absence of one: the replacement — authored at the same
    /// too-deep margin — still has to be re-based, or correct text lands at an indentation the
    /// file never had.
    #[test]
    fn a_zero_indent_delta_still_rebases_the_replacement() {
        let content = "a\n  b\n";
        let edits = vec![("  a\n    b".to_string(), "  a\n    c".to_string())];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "a\n  c\n");
    }

    /// Tiers 1 and 2 are pi's, and the new tier must leave them bit-for-bit alone.
    #[test]
    fn exact_and_fuzzy_tiers_are_untouched_by_the_line_anchored_tier() {
        // Tier 1: the needle is a substring INSIDE an indented line. Pi replaces exactly those
        // bytes and re-indents nothing; a `reindent` that fired here would mangle the result.
        let content = "fn a() {\n\t\tone();\n}\n";
        let edits = vec![("one();".to_string(), "\tone();\n\ttwo();".to_string())];
        let r = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap();
        assert_eq!(r.new_content, "fn a() {\n\t\t\tone();\n\ttwo();\n}\n");
        // Tier 2: a trailing-whitespace-only difference still takes the fuzzy overlay.
        let content = "alpha   \nbeta\n";
        let edits = vec![("alpha\nbeta".to_string(), "gamma\ndelta".to_string())];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "gamma\ndelta\n");
    }

    /// Pi sends an all-whitespace `oldText` into `countOccurrences` (edit-diff.ts:333), which
    /// raises the DUPLICATE error — different remediation advice from "not found". The
    /// line-anchored count must stay a FALLBACK, consulted only when the substring count is 0,
    /// or this input changes its answer.
    #[test]
    fn whitespace_only_old_text_still_reports_duplicates() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\n";
        let edits = vec![("   ".to_string(), "X".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap_err();
        assert!(e.0.contains("occurrences"), "got: {}", e.0);
        assert!(!e.0.contains("Could not find"), "got: {}", e.0);
    }

    /// [CYRUP-DELTA] Pi stops at the sentence (edit-diff.ts:258-267); cyrup appends the closest
    /// region so the caller repairs the needle in one round. Similarity picks what to SHOW and
    /// never what to write.
    #[test]
    fn not_found_names_the_closest_region() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\n";
        let edits = vec![(
            "    fn foo() {\n        quux();\n    }".to_string(),
            "X".to_string(),
        )];
        let e = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap_err();
        // Pi's sentence still LEADS the message.
        assert!(
            e.0.starts_with(
                "Could not find the exact text in f.rs. The old text must match exactly including all whitespace and newlines."
            ),
            "got: {}",
            e.0
        );
        // The appended region names its 1-indexed start line and shows the file's real bytes.
        assert!(
            e.0.contains("Closest region in f.rs starts at line 1:"),
            "got: {}",
            e.0
        );
        assert!(e.0.contains("bar();"), "got: {}", e.0);
    }

    /// The delta is ADDITIVE: below the similarity floor the message is byte-identical to pi's.
    /// Without this, lowering the floor would decorate every failure with a bogus region.
    #[test]
    fn a_far_miss_keeps_pis_bare_sentence() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\n";
        let edits = vec![(
            "totally unrelated content here\nnot in the file at all\nnope".to_string(),
            "X".to_string(),
        )];
        let e = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap_err();
        assert_eq!(
            e.0,
            "Could not find the exact text in f.rs. The old text must match exactly including all whitespace and newlines."
        );
    }
```

---

## 4. RED levers to execute

One at a time: apply the lever, run `cargo test -p cyrup-tools`, record the **named test**,
its `file:line`, the message, and the count out of the suite total; then restore
`edit_diff.rs` byte-identically (verify with `shasum -a 256`) before the next.

| Guard | Lever — the single edit that must turn it red |
|---|---|
| `line_anchored_tier_rebases_to_the_files_own_indent` | Delete the tier-3 arm at **:494-502** (the `if let Some(m) = line_anchored_matches(..)` block) so the `None` arm falls straight through to not-found. |
| `ragged_indentation_is_refused_rather_than_mangled` | Delete the uniformity check at **:400-402** (`if indents.iter().any(\|d\| d != first)`). Expect a *write*, not an error — the mangling this bound exists to stop. |
| `two_indent_different_copies_are_a_duplicate_not_a_write` | Revert **:807-810** to a plain `let occ = count_occurrences(&replacement_base, old);`. Expect a silent first-match write. |
| `a_zero_indent_delta_still_rebases_the_replacement` | Change `reindent` (**:436**) back to `fn reindent(new_text: &str, indent: &str)` with `if indent.is_empty() { return new_text.to_string(); }`, and the call site to `&mr.indent.unwrap_or_default()`. Expect `"  a\n    c\n"` — the shipped bug. |
| `exact_and_fuzzy_tiers_are_untouched_by_the_line_anchored_tier` | Make `reindent` re-base unconditionally: drop the `None` early return and treat it as `Some("")`. The tier-1 substring case changes. |
| `whitespace_only_old_text_still_reports_duplicates` | Change **:807-810** to use `line_anchored_matches(..).len()` *always* rather than only when the substring count is 0. Expect "Could not find" in place of the duplicate error. |
| `not_found_names_the_closest_region` | Revert `err_not_found` (**:726**) to returning `head` unconditionally. |
| `a_far_miss_keeps_pis_bare_sentence` | Set `NEAR_MISS_THRESHOLD` (**:689**) to `0.0`. Expect a region appended to a message that should carry none. |

**One prediction is doubted and must be reported either way.** The all-blank early return in
`line_anchored_matches` (**:337-339**) is believed *not* load-bearing: with it removed, an
all-blank needle still pushes no indent, so `indents.first()` is `None` and the window is
skipped. If removing it leaves `whitespace_only_old_text_still_reports_duplicates` green, that
is the expected outcome — the lever for that guard is the `occ` row above, not this return.
Record the finding; do not "fix" the early return, which stands as documentation of intent.

---

## 5. Definition of done

1. The eight tests above exist in `edit_diff.rs`'s `mod tests`, appended after `overlap_error`.
2. Every lever in §4 has been executed and its result recorded — the named test, its
   `file:line`, the message, and the count — including any prediction that was disproved.
3. `edit_diff.rs` restored byte-identically after the last lever, verified by `shasum -a 256`
   against the value taken before the first.
4. No source behaviour changes: §1 ships as-is, §3's two measured properties are left alone.
5. `cargo test -p cyrup-tools` green (346 + 8 = 354), `cargo clippy --workspace --all-targets`
   exits 0, `edit_diff.rs` rustfmt-clean.
