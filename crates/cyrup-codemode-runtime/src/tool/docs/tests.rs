//! The script reference is written where the model can read it, whatever the install looks like.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{EMBEDDED_CODEMODE_DOCS, materialise_codemode_docs};

#[test]
fn the_embedded_page_is_the_docs_page_of_the_repository() {
    let on_disk = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/codemode.md"
    ))
    .unwrap();
    assert_eq!(EMBEDDED_CODEMODE_DOCS, on_disk);
    assert!(EMBEDDED_CODEMODE_DOCS.starts_with("# Codemode\n"));
}

#[test]
fn the_page_is_written_under_the_agent_directory() {
    let agent_dir = tempfile::tempdir().unwrap();
    let path = materialise_codemode_docs(agent_dir.path()).unwrap();
    assert_eq!(path, agent_dir.path().join("docs").join("codemode.md"));
    assert!(path.is_absolute());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        EMBEDDED_CODEMODE_DOCS
    );
    // Nothing else is left in the directory (the staging file is gone).
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["codemode.md"]);
}

/// A page an older build left is replaced, so the model reads the reference of the running binary;
/// one that is already current is not rewritten.
#[test]
fn a_page_that_differs_is_refreshed_and_a_current_one_is_left_alone() {
    let agent_dir = tempfile::tempdir().unwrap();
    let page = agent_dir.path().join("docs").join("codemode.md");
    std::fs::create_dir_all(page.parent().unwrap()).unwrap();
    std::fs::write(&page, "# Codemode\n\nthe page of an older build\n").unwrap();

    assert_eq!(materialise_codemode_docs(agent_dir.path()).unwrap(), page);
    assert_eq!(
        std::fs::read_to_string(&page).unwrap(),
        EMBEDDED_CODEMODE_DOCS
    );

    let modified = std::fs::metadata(&page).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    materialise_codemode_docs(agent_dir.path()).unwrap();
    assert_eq!(
        std::fs::metadata(&page).unwrap().modified().unwrap(),
        modified,
        "a current page is not rewritten"
    );
}

#[test]
fn a_directory_that_cannot_be_written_is_an_error_not_a_dangling_path() {
    let agent_dir = tempfile::tempdir().unwrap();
    // `docs` is a file, so the directory cannot be made.
    std::fs::write(agent_dir.path().join("docs"), "not a directory").unwrap();
    assert!(materialise_codemode_docs(agent_dir.path()).is_err());
}

fn extension() -> crate::CodemodeExtension {
    crate::CodemodeExtension::new(
        crate::tool::CodemodeHostSlot::new(),
        std::sync::Arc::new(crate::tool::UnavailableSandboxFactory),
    )
}

/// An installed binary has no `docs/` next to it, so the path the model is told to read is the copy
/// written under the agent directory: it exists, and it is where the extension says it is.
#[test]
fn the_extension_points_the_model_at_the_copy_under_the_agent_directory() {
    let agent_dir = tempfile::tempdir().unwrap();
    let extension = extension().with_agent_dir(agent_dir.path());
    let expected = agent_dir.path().join("docs").join("codemode.md");
    assert_eq!(extension.docs_path(), expected.to_str().unwrap());
    assert!(
        std::path::Path::new(extension.docs_path()).is_file(),
        "the path the model reads must exist"
    );
}

/// When the copy cannot be written, the tool keeps the path it had instead of one to nothing.
#[test]
fn an_agent_directory_that_cannot_be_written_leaves_the_docs_path_alone() {
    let agent_dir = tempfile::tempdir().unwrap();
    std::fs::write(agent_dir.path().join("docs"), "not a directory").unwrap();
    let before = extension().docs_path().to_owned();
    assert_eq!(
        extension().with_agent_dir(agent_dir.path()).docs_path(),
        before
    );
}

// ------------------------------------------------------------------------------------ the guide --

use std::path::{Path, PathBuf};

fn docs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs")
}

/// The page the binary ships is `docs/codemode.md`, and the model reads it, so its links to the
/// guide are written from `docs/` (`guide/reference/cli.md`). The book (`book.toml`: `src =
/// docs/guide`) cannot include a file from outside its source directory with those links resolving,
/// so it carries a copy at `docs/guide/guides/codemode.md` whose only difference is the links,
/// written from that directory (`../reference/cli.md`). This pins the copy to the page: an edit to
/// one without the other fails here, instead of leaving the guide describing an older tool.
///
/// To regenerate the copy: `sed 's|](guide/|](../|g' docs/codemode.md > docs/guide/guides/codemode.md`.
#[test]
fn the_guide_page_is_the_shipped_page_with_the_links_of_its_own_directory() {
    let shipped = std::fs::read_to_string(docs_dir().join("codemode.md")).unwrap();
    let in_guide = std::fs::read_to_string(docs_dir().join("guide/guides/codemode.md")).unwrap();
    assert_eq!(
        in_guide,
        shipped.replace("](guide/", "](../"),
        "docs/guide/guides/codemode.md differs from docs/codemode.md; regenerate it with: \
         sed 's|](guide/|](../|g' docs/codemode.md > docs/guide/guides/codemode.md"
    );
}

/// The book's table of contents lists the page; a page `SUMMARY.md` does not name is not in the
/// book, which is how `docs/codemode.md` stayed out of it.
#[test]
fn the_guide_summary_lists_the_codemode_page() {
    let summary = std::fs::read_to_string(docs_dir().join("guide/SUMMARY.md")).unwrap();
    assert!(
        summary
            .lines()
            .any(|line| line.trim_end() == "- [Codemode scripts](guides/codemode.md)"),
        "{summary}"
    );
}

/// `mdBook` / GitHub heading ids: lower case, spaces to `-`, everything but letters, digits, `-` and
/// `_` dropped.
fn heading_id(heading: &str) -> String {
    heading
        .trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// The text of a Markdown file outside fenced code blocks, one entry per line.
fn prose_lines(text: &str) -> Vec<&str> {
    let mut fenced = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
        } else if !fenced {
            lines.push(line);
        }
    }
    lines
}

fn heading_ids(text: &str) -> Vec<String> {
    prose_lines(text)
        .into_iter()
        .filter_map(|line| {
            let hashes = line.chars().take_while(|c| *c == '#').count();
            let rest = line.get(hashes..)?;
            ((1..=6).contains(&hashes) && rest.starts_with(' ')).then(|| heading_id(rest))
        })
        .collect()
}

/// The targets of the `](target)` links on a line, with inline code spans left out.
fn link_targets(line: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    // Segments between backticks alternate prose and code; only the prose has links.
    for prose in line.split('`').step_by(2) {
        let mut rest = prose;
        while let Some(at) = rest.find("](") {
            let after = rest.get(at + 2..).unwrap_or_default();
            let Some(end) = after.find(')') else { break };
            if let Some(target) = after.get(..end) {
                targets.push(target);
            }
            rest = after.get(end..).unwrap_or_default();
        }
    }
    targets
}

fn markdown_files(dir: &Path, into: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            markdown_files(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            into.push(path);
        }
    }
}

/// Every relative link of the user guide, the README and the two copies of the codemode page lands
/// on a file that exists and, when it names a section, on a heading that exists. A link from a guide
/// page must also stay inside the book (`docs/guide`): the book leaves a link it cannot resolve as
/// it is, so nothing else notices one, and the codemode page was linked from three guide pages as
/// `../../codemode.md`, a file outside the book that only resolved on disk.
#[test]
fn every_relative_link_of_the_guide_and_the_codemode_page_resolves() {
    let docs = docs_dir();
    let book = docs.join("guide").canonicalize().unwrap();
    let mut files = vec![docs.join("../README.md"), docs.join("codemode.md")];
    markdown_files(&docs.join("guide"), &mut files);
    let mut broken = Vec::new();
    let mut checked = 0usize;
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        let base = file.parent().unwrap();
        for line in prose_lines(&text) {
            for target in link_targets(line) {
                // `https:`, `mailto:` and the like are not files.
                if target
                    .split('/')
                    .next()
                    .is_some_and(|first| first.contains(':'))
                {
                    continue;
                }
                let (path, anchor) = target.split_once('#').unwrap_or((target, ""));
                let resolved = if path.is_empty() {
                    file.clone()
                } else {
                    base.join(path)
                };
                checked += 1;
                if !resolved.is_file() {
                    broken.push(format!("{}: no file for `{target}`", file.display()));
                    continue;
                }
                let in_book = file.canonicalize().unwrap().starts_with(&book);
                if in_book && !resolved.canonicalize().unwrap().starts_with(&book) {
                    broken.push(format!("{}: `{target}` leaves the book", file.display()));
                    continue;
                }
                if !anchor.is_empty() && resolved.extension().is_some_and(|ext| ext == "md") {
                    let linked = std::fs::read_to_string(&resolved).unwrap();
                    if !heading_ids(&linked).iter().any(|id| id == anchor) {
                        broken.push(format!("{}: no heading for `{target}`", file.display()));
                    }
                }
            }
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
    assert!(
        checked > 100,
        "only {checked} links were checked; the scan is broken"
    );
}

/// The paragraphs of a Markdown file outside fenced code blocks: consecutive prose lines joined, so
/// a sentence that the page wraps across lines is one piece of text. A list item and a table row
/// start a piece of their own, since the items of a list are separate statements.
fn prose_paragraphs(text: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    for line in prose_lines(text) {
        let starts_item = line.starts_with("- ") || line.starts_with("* ") || line.starts_with('|');
        if (line.trim().is_empty() || starts_item) && !current.is_empty() {
            paragraphs.push(std::mem::take(&mut current));
        }
        if !line.trim().is_empty() {
            current.push_str(line);
            current.push(' ');
        }
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    paragraphs
}

/// `powershell` joined the registry after most of the guide was written, and the guide kept listing
/// seven built-ins ("`read`, `bash`, `edit`, `write`, `grep`, `find`, `ls`") in three places. A
/// paragraph that names seven or more of the registered built-ins in code spans is enumerating them,
/// and an enumeration that leaves one out is stale, not a choice. A paragraph that means a subset
/// names fewer than seven.
#[test]
fn a_prose_list_of_the_built_in_tools_names_every_registered_one() {
    let docs = docs_dir();
    let mut files = vec![docs.join("../README.md"), docs.join("codemode.md")];
    markdown_files(&docs.join("guide"), &mut files);
    let names = cyrup_tools::BUILTIN_NAMES;
    let mut stale = Vec::new();
    let mut lists = 0usize;
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for paragraph in prose_paragraphs(&text) {
            let missing: Vec<&str> = names
                .iter()
                .copied()
                .filter(|name| !paragraph.contains(&format!("`{name}`")))
                .collect();
            if names.len() - missing.len() < names.len() - 1 {
                continue;
            }
            lists += 1;
            if !missing.is_empty() {
                stale.push(format!(
                    "{}: lists the built-in tools without {missing:?}: {}",
                    file.display(),
                    paragraph.chars().take(120).collect::<String>()
                ));
            }
        }
    }
    assert!(stale.is_empty(), "{}", stale.join("\n"));
    assert!(
        lists >= 3,
        "only {lists} lists of the built-in tools were found; the scan is broken"
    );
}
