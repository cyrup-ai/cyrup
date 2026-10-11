//! TUI-011 — the `/changelog` parser, the "What's New" startup gate and the link rewriting.
//!
//! The row was held open twice and its 2026-09-14 attempt recorded six defects. Three are parser
//! semantics and are pinned here: the version header is ANCHORED (a prose heading is dropped, not
//! turned into a phantom release), `getNewEntries` DEGRADES a corrupt persisted version to `0.0.0`,
//! and `/changelog` reverses file order where the startup notice does not.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::changelog::{
    ChangelogEntry, EMBEDDED, StartupNotice, command_markdown, condensed_notice, new_entries,
    normalize_changelog_links, parse_changelog, startup_notice,
};

fn entry(major: u64, minor: u64, patch: u64) -> ChangelogEntry {
    ChangelogEntry {
        major,
        minor,
        patch,
        content: String::new(),
    }
}

/// `parseChangelog`'s regex is `/##\s+\[?(\d+)\.(\d+)\.(\d+)\]?/` matched against the WHOLE line
/// (`changelog.ts:134`), so it is anchored at the `## `. A heading whose first token is prose does
/// NOT match, and upstream drops the section (`:145-148`).
///
/// This is the 2026-09-14 attempt's defect 5: it stripped `"## "` and searched the remainder
/// unanchored, turning both headings below into real entries, and its doc comment asserted
/// upstream was unanchored — which is the opposite of the truth.
#[test]
fn a_prose_heading_is_dropped_not_read_as_a_phantom_release() {
    let md = "\
# Changelog

## Version 1.2.3
not a release — the version is not the first token

## Unreleased (since 1.4.0)
also not a release

## 2.0.0 - 2026-01-01
a real one
";
    let entries = parse_changelog(md);
    assert_eq!(
        entries.len(),
        1,
        "only the anchored header is an entry: {:?}",
        entries
            .iter()
            .map(ChangelogEntry::version)
            .collect::<Vec<_>>()
    );
    assert_eq!(entries[0].version(), "2.0.0");
    assert!(
        entries[0].content.contains("a real one"),
        "the section body follows its header"
    );
    assert!(
        !entries[0].content.contains("not a release"),
        "a dropped section's body must not leak into the next entry"
    );
}

/// The bracketed Keep-a-Changelog form, which the `\[?` and `\]?` of the pattern allow, and which
/// the `(\d+)` groups match as a PREFIX of their segment so the trailing `]` and date are ignored.
#[test]
fn the_bracketed_form_and_a_trailing_date_both_parse() {
    let entries = parse_changelog("## [1.2.3] - 2026-02-03\nbody\n");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].version(), "1.2.3");
}

/// The header line is part of the entry's content — pi's `currentLines = [line]`
/// (`changelog.ts:139`) — so a rendered entry carries its own `##` heading.
#[test]
fn an_entrys_content_includes_its_header_line() {
    let entries = parse_changelog("## 1.0.0\nbody\n");
    assert!(entries[0].content.starts_with("## 1.0.0"));
}

/// `getNewEntries` parses the persisted version with `Number(...) || 0` per part
/// (`changelog.ts:168-174`), so a corrupt or absent part reads as `0` rather than failing. That is
/// the arm a mangled settings file takes, and it must show everything rather than nothing.
#[test]
fn a_corrupt_persisted_version_degrades_to_zero_and_shows_everything() {
    let entries = vec![entry(0, 0, 1), entry(1, 0, 0)];
    // Wholly unparseable reads as 0.0.0, so everything is newer.
    for junk in ["abc", "", "not.a.version"] {
        assert_eq!(
            new_entries(&entries, junk).len(),
            2,
            "{junk:?} must degrade to 0.0.0, leaving both entries newer"
        );
    }
    // The degradation is PER PART, not wholesale: `Number("x") || 0` is `0` for that part alone,
    // so `1.x.y` reads as `1.0.0` and nothing is newer. Writing this test the other way is how the
    // distinction got noticed — the implementation already had it right.
    assert_eq!(
        new_entries(&entries, "1.x.y").len(),
        0,
        "`1.x.y` degrades to 1.0.0, not 0.0.0"
    );
    // A MISSING part defaults the same way, so a short version is not an error either.
    assert_eq!(
        new_entries(&entries, "1").len(),
        0,
        "`1` is 1.0.0, so nothing is newer"
    );
    assert_eq!(
        new_entries(&entries, "0.0").len(),
        2,
        "`0.0` is 0.0.0, so both are newer"
    );
    // And the ordinary case still discriminates.
    assert_eq!(new_entries(&entries, "0.0.1").len(), 1);
    assert_eq!(new_entries(&entries, "1.0.0").len(), 0);
}

/// `handleChangelogCommand` reverses file order (`interactive-mode.ts:6781`); the startup notice
/// joins `newEntries` in file order (`:1353`). The asymmetry is upstream's and is NOT unified.
#[test]
fn the_command_reverses_file_order_where_the_startup_notice_does_not() {
    let md = "## 2.0.0\nsecond\n\n## 1.0.0\nfirst\n";
    let cmd = command_markdown(md);
    let older = cmd.find("first").expect("1.0.0 body");
    let newer = cmd.find("second").expect("2.0.0 body");
    assert!(
        older < newer,
        "/changelog puts the OLDEST first (file order reversed):\n{cmd}"
    );

    let StartupNotice::Show(notice) = startup_notice(md, false, Some("0.0.1")) else {
        panic!("both entries are newer than 0.0.1");
    };
    let older = notice.find("first").expect("1.0.0 body");
    let newer = notice.find("second").expect("2.0.0 body");
    assert!(
        newer < older,
        "the notice keeps FILE order, newest first:\n{notice}"
    );
}

/// pi's exact empty string (`interactive-mode.ts:6785`) when nothing parses — which is also what
/// the stub this row replaced always returned.
#[test]
fn an_unparseable_changelog_gives_pis_empty_string() {
    assert_eq!(
        command_markdown("# Changelog\n\nnothing versioned here\n"),
        "No changelog entries found."
    );
}

/// `getChangelogForDisplay`'s four arms (`interactive-mode.ts:1332-1356`).
#[test]
fn the_startup_gate_takes_each_of_pis_four_arms() {
    let md = "## 1.0.0\nnotes\n";

    // A resumed session shows nothing and records nothing (`:1333-1335`).
    assert_eq!(
        startup_notice(md, true, Some("0.0.1")),
        StartupNotice::Resumed
    );

    // A fresh install RECORDS the version and shows nothing — pi deliberately swallows the first
    // changelog (`:1342-1347`).
    assert_eq!(startup_notice(md, false, None), StartupNotice::RecordOnly);

    // Something newer than the persisted version shows and persists (`:1350-1354`).
    let StartupNotice::Show(body) = startup_notice(md, false, Some("0.0.9")) else {
        panic!("1.0.0 is newer than 0.0.9");
    };
    assert!(body.contains("notes"));

    // Nothing newer: silent (`:1356`). This is the second boot of the Verify line.
    assert_eq!(
        startup_notice(md, false, Some("1.0.0")),
        StartupNotice::UpToDate
    );
}

/// A floating `main`/`master` link is re-pointed at the entry's own tag, so a link in an old entry
/// keeps resolving to the tree that entry described (`changelog.ts:74-81`).
#[test]
fn a_floating_branch_link_is_repointed_at_the_entrys_tag() {
    let e = entry(1, 2, 3);
    let out = normalize_changelog_links(
        "see [the file](https://github.com/cyrup-ai/cyrup/blob/main/README.md)",
        &e,
    );
    assert!(
        out.contains("/blob/v1.2.3/README.md"),
        "main must become the entry's tag: {out}"
    );
}

/// A relative target becomes a repo URL at the entry's tag, with `blob` for a file and `tree` for a
/// directory — `isDirectoryTarget` keys on a trailing `/` or a dotless basename
/// (`changelog.ts:60-67`).
#[test]
fn a_relative_link_resolves_to_blob_for_a_file_and_tree_for_a_directory() {
    let e = entry(0, 9, 0);
    let file = normalize_changelog_links("[x](docs/guide/cli.md)", &e);
    assert!(file.contains("/blob/v0.9.0/docs/guide/cli.md"), "{file}");

    let dir = normalize_changelog_links("[x](docs/gap-analysis/)", &e);
    assert!(dir.contains("/tree/v0.9.0/docs/gap-analysis"), "{dir}");

    let dotless = normalize_changelog_links("[x](crates)", &e);
    assert!(dotless.contains("/tree/v0.9.0/crates"), "{dotless}");
}

/// An absolute URL, a protocol-relative one and a bare fragment are left alone
/// (`changelog.ts:83-85`); a target that escapes the repo is too (`:54-56`).
#[test]
fn links_that_are_not_repo_relative_are_left_untouched() {
    let e = entry(1, 0, 0);
    for target in [
        "https://example.com/x",
        "//example.com/x",
        "#a-heading",
        "mailto:someone@example.com",
        "../../../etc/passwd",
    ] {
        let md = format!("[x]({target})");
        assert_eq!(
            normalize_changelog_links(&md, &e),
            md,
            "{target} must pass through unchanged"
        );
    }
}

/// The condensed form under `collapseChangelog` (`interactive-mode.ts:858-860`), whose `/changelog`
/// is BOLD — so it is returned as three pieces for the renderer to style rather than one string.
#[test]
fn the_condensed_notice_names_the_latest_version_and_isolates_the_command() {
    let (before, cmd, after) = condensed_notice("## 3.4.5\nwhatever\n", "9.9.9");
    assert_eq!(before, "Updated to v3.4.5. Use ");
    assert_eq!(cmd, "/changelog", "the bold span is the command alone");
    assert_eq!(after, " to view full changelog.");

    // With no parseable entry it falls back to the RUNNING version (`:859`).
    let (before, _, _) = condensed_notice("nothing versioned", "9.9.9");
    assert_eq!(before, "Updated to v9.9.9. Use ");
}

/// The repo's OWN `CHANGELOG.md`, compiled in by `include_str!`. This is the guard that the feature
/// is live rather than merely implemented: the row stayed open because a missing file made the port
/// return the stub's exact string on every installed copy.
#[test]
fn the_embedded_changelog_parses_and_matches_the_crate_version() {
    let entries = parse_changelog(EMBEDDED);
    assert!(
        !entries.is_empty(),
        "the shipped CHANGELOG.md must parse into at least one entry"
    );
    assert_ne!(
        command_markdown(EMBEDDED),
        "No changelog entries found.",
        "/changelog must render real content, not the stub's string"
    );
    // The newest entry is the version the binary reports, so the gate converges: a fresh install
    // records this version and the next boot is silent.
    assert_eq!(
        entries[0].version(),
        env!("CARGO_PKG_VERSION"),
        "the newest changelog entry must be the crate version, or the startup notice either fires \
         forever or can never fire"
    );
}
