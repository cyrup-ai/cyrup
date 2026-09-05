//! SUBA-074 stage 2 — how the prompt reaches the external child, and the temporary paths that
//! delivery creates.
//!
//! Upstream keeps three independent optional inputs — `prompt`, `promptFilePath?`,
//! `temporaryDirectories?` — and disambiguates them at one line,
//! `child.stdin.end(input.promptFilePath ? undefined : input.prompt)`
//! (`pi-subagents/src/runs/shared/external-cli-runner.ts:364` @v0.64.0). Three optionals admit
//! states the design forbids: a path set with the stdin write left in place delivers a private
//! handoff TWICE, once over the channel the adapter's design chose to avoid; a path set with no
//! file written makes the foreign CLI fail on a missing path with no cyrup-side diagnostic. Neither
//! is visible to types over there.
//!
//! Here the channel is one enum that OWNS its paths, and [`PreparedPrompt`] owns their removal
//! through `Drop`, which also covers upstream's early-error path (`:211-224`) — the one an
//! implementer most often forgets.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::runner::contract::PromptDeliveryKind;

/// `buildExternalCliPrompt(systemInstructions, task)` (`external-cli-runner.ts:26-28`).
#[must_use]
pub fn build_external_cli_prompt(system_instructions: &str, task: &str) -> String {
    format!(
        "<System instructions>\n{}\n\n<Task>\n{task}",
        system_instructions.trim()
    )
}

/// The single channel the prompt travels down. Exactly one per run, by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptDelivery {
    /// Written to the child's stdin, which is then closed (`:364`).
    Stdin,
    /// Written to `file` — created `wx` with mode `0o600` (`:204-209`) — inside `dir`, created with
    /// mode `0o700` (`:200-203`). `file` is also what the adapter names in argv, and `dir` is what
    /// it passes to `--add-dir` when the handoff falls outside the workspace.
    ///
    /// Reachable only from a prompt-file adapter ([`PromptDeliveryKind::PromptFile`]); the one
    /// upstream adapter that uses it (cursor-agent) is deferred out of this batch, so the variant
    /// exists and is exercised by tests but no shipped adapter selects it yet.
    PromptFile {
        /// The 0700 directory created for this run.
        dir: PathBuf,
        /// The 0600 handoff file inside it.
        file: PathBuf,
    },
}

impl PromptDelivery {
    /// Which [`PromptDeliveryKind`] this channel is — the link back to the published runner status.
    #[must_use]
    pub const fn kind(&self) -> PromptDeliveryKind {
        match self {
            Self::Stdin => PromptDeliveryKind::Stdin,
            Self::PromptFile { .. } => PromptDeliveryKind::PromptFile,
        }
    }

    /// Create whatever this channel needs and write the prompt into it.
    ///
    /// # Errors
    ///
    /// Any filesystem failure creating the 0700 directory or exclusively creating the 0600 file.
    /// Upstream raises the same failures out of its pre-spawn `try` block and settles the run with
    /// exit code 1 rather than spawning (`:199-224`).
    pub fn prepare(self, prompt: &str) -> std::io::Result<PreparedPrompt> {
        let mut created: Vec<PathBuf> = Vec::new();
        if let Self::PromptFile { dir, file } = &self {
            create_private_dir(dir)?;
            created.push(dir.clone());
            write_private_file(file, prompt)?;
            created.push(file.clone());
        }
        Ok(PreparedPrompt {
            delivery: self,
            prompt: prompt.to_string(),
            created,
        })
    }
}

/// A [`PromptDelivery`] whose side effects have happened, and whose cleanup is owned.
///
/// `Drop` removes every path this delivery created, in reverse creation order — upstream's
/// `cleanupTemporaryPaths` (`:193-196`), but on EVERY exit path including a panic and the
/// early-error return, rather than only the two places upstream remembers to call it.
#[derive(Debug)]
pub struct PreparedPrompt {
    delivery: PromptDelivery,
    prompt: String,
    created: Vec<PathBuf>,
}

impl PreparedPrompt {
    /// What to write to the child's stdin before closing it — `Some(prompt)` for the stdin channel,
    /// `None` for the prompt-file channel, where stdin is closed empty.
    ///
    /// This is upstream's `input.promptFilePath ? undefined : input.prompt` (`:364`), asked exactly
    /// once, of a value that cannot be in both states.
    #[must_use]
    pub fn stdin_payload(&self) -> Option<&str> {
        match self.delivery {
            PromptDelivery::Stdin => Some(self.prompt.as_str()),
            PromptDelivery::PromptFile { .. } => None,
        }
    }

    /// The handoff file's path, for an adapter that names it in argv.
    #[must_use]
    pub fn prompt_file(&self) -> Option<&Path> {
        match &self.delivery {
            PromptDelivery::Stdin => None,
            PromptDelivery::PromptFile { file, .. } => Some(file.as_path()),
        }
    }

    /// This delivery's channel, for the published runner status.
    #[must_use]
    pub const fn kind(&self) -> PromptDeliveryKind {
        self.delivery.kind()
    }
}

impl Drop for PreparedPrompt {
    fn drop(&mut self) {
        // Reverse order: the file before the directory that contains it. Failures are ignored for
        // the same reason upstream's `{ force: true }` ignores ENOENT — a path already gone is the
        // outcome this wanted.
        for path in self.created.iter().rev() {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// `fs.mkdirSync(directory, { mode: 0o700 })` (`:201`) — note upstream does NOT pass `recursive`,
/// so an already-existing directory is an ERROR, not a silent reuse. That is load-bearing: the
/// handoff directory must be one this run created, never one that was lying around.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// `fs.openSync(promptFilePath, "wx", 0o600)` (`:205`) — EXCLUSIVE create, so an existing file at
/// that path fails the run rather than being overwritten or, worse, appended to.
fn write_private_file(file: &Path, contents: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut handle = options.open(file)?;
    handle.write_all(contents.as_bytes())?;
    handle.flush()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// `buildExternalCliPrompt` trims the system instructions and nothing else (`:26-28`).
    #[test]
    fn the_prompt_framing_is_upstreams_two_section_form() {
        assert_eq!(
            build_external_cli_prompt("  be careful  ", "do the thing"),
            "<System instructions>\nbe careful\n\n<Task>\ndo the thing"
        );
    }

    /// The stdin channel writes the prompt to stdin and creates nothing.
    #[test]
    fn the_stdin_channel_delivers_on_stdin_and_touches_no_path() {
        let prepared = PromptDelivery::Stdin.prepare("hello").unwrap();
        assert_eq!(prepared.stdin_payload(), Some("hello"));
        assert!(prepared.prompt_file().is_none());
        assert_eq!(prepared.kind(), PromptDeliveryKind::Stdin);
    }

    /// The prompt-file channel writes a 0600 file inside a 0700 directory, keeps stdin EMPTY, and
    /// removes both when the value is dropped — including when the run never got as far as spawning.
    #[test]
    fn the_prompt_file_channel_is_private_exclusive_and_self_cleaning() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("handoff");
        let file = dir.join("handoff.txt");
        {
            let prepared = PromptDelivery::PromptFile {
                dir: dir.clone(),
                file: file.clone(),
            }
            .prepare("private prompt")
            .unwrap();
            assert_eq!(
                prepared.stdin_payload(),
                None,
                "a prompt delivered by file must NOT also go down stdin"
            );
            assert_eq!(prepared.prompt_file(), Some(file.as_path()));
            assert_eq!(prepared.kind(), PromptDeliveryKind::PromptFile);
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "private prompt");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                    0o600
                );
                assert_eq!(
                    std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }
        assert!(!file.exists(), "the handoff file must not outlive the run");
        assert!(!dir.exists(), "nor the directory holding it");
    }

    /// The directory create is non-recursive and the file create is exclusive, so a pre-existing
    /// path fails the run rather than being reused (`:201`, `:205`).
    #[test]
    fn a_pre_existing_handoff_path_fails_rather_than_being_reused() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("handoff");
        std::fs::create_dir(&dir).unwrap();
        let err = PromptDelivery::PromptFile {
            dir: dir.clone(),
            file: dir.join("handoff.txt"),
        }
        .prepare("x")
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(
            dir.exists(),
            "a directory this run did not create is not removed by the failed attempt"
        );
    }
}
