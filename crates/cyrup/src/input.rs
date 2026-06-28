//! Prompt-input assembly: positionals + `@file` + piped stdin (arch-11 §6.2; R-11-006/024/025).
//!
//! `@`-prefixed positionals are file references whose **text** is inlined into the prompt body; bare
//! positionals are message words. The initial message merges (in order) the inlined file text, the
//! first bare message, and any piped stdin; remaining bare messages become sequential follow-ups
//! (R-11-009). Image `@file` attachment is deferred (see crate notes).

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Context;
use tokio::io::AsyncReadExt;

use crate::cli::Cli;

/// The assembled prompt inputs for a one-shot / interactive launch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// The first prompt: inlined `@file` text + first message + piped stdin, joined by blank lines.
    pub initial: String,
    /// Subsequent bare messages, replayed one prompt at a time after the initial run (R-11-009).
    pub follow_ups: Vec<String>,
}

impl Inputs {
    /// Whether there is any initial prompt text at all.
    pub fn is_empty(&self) -> bool {
        self.initial.is_empty()
    }
}

/// Split trailing positionals into `@file` references (the `@` stripped) and bare message words.
pub fn split_positionals(positionals: &[String]) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut messages = Vec::new();
    for arg in positionals {
        match arg.strip_prefix('@') {
            // `@@literal` is an escape for a bare message that legitimately starts with '@'.
            Some(rest) if arg.starts_with("@@") => messages.push(rest.to_string()),
            Some(path) if !path.is_empty() => files.push(PathBuf::from(path)),
            _ => messages.push(arg.clone()),
        }
    }
    (files, messages)
}

/// Merge the three input sources into [`Inputs`] (pure; the file/stdin reads happen in
/// [`build_inputs`]). The initial prompt is `file_text` ⧺ `messages[0]` ⧺ `piped`, blank-line joined.
pub fn compose_inputs(
    file_text: Option<String>,
    messages: &[String],
    piped: Option<String>,
) -> Inputs {
    let mut parts: Vec<String> = Vec::new();
    if let Some(text) = file_text {
        let text = text.trim_end().to_string();
        if !text.is_empty() {
            parts.push(text);
        }
    }
    if let Some(first) = messages.first()
        && !first.is_empty()
    {
        parts.push(first.clone());
    }
    if let Some(piped) = piped {
        let piped = piped.trim_end().to_string();
        if !piped.is_empty() {
            parts.push(piped);
        }
    }
    Inputs { initial: parts.join("\n\n"), follow_ups: messages.iter().skip(1).cloned().collect() }
}

/// Read all `@file` references and concatenate their text (blank-line separated).
async fn read_file_args(files: &[PathBuf]) -> anyhow::Result<Option<String>> {
    if files.is_empty() {
        return Ok(None);
    }
    let mut chunks = Vec::with_capacity(files.len());
    for path in files {
        let body = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("reading @file '{}'", path.display()))?;
        chunks.push(body.trim_end().to_string());
    }
    Ok(Some(chunks.join("\n\n")))
}

/// Read piped stdin to a string when stdin is not a TTY (R-11-006); `None` when interactive.
async fn read_piped_stdin() -> anyhow::Result<Option<String>> {
    if std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let mut buf = String::new();
    tokio::io::stdin().read_to_string(&mut buf).await.context("reading piped stdin")?;
    if buf.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(buf))
    }
}

/// Build the prompt inputs from the CLI: split positionals, read `@file` text, merge piped stdin.
pub async fn build_inputs(cli: &Cli) -> anyhow::Result<Inputs> {
    let (files, messages) = split_positionals(&cli.positionals);
    let file_text = read_file_args(&files).await?;
    let piped = read_piped_stdin().await?;
    Ok(compose_inputs(file_text, &messages, piped))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn split_separates_files_from_messages() {
        let (files, messages) = split_positionals(&s(&["@a.txt", "hello", "world", "@dir/b.md"]));
        assert_eq!(files, vec![PathBuf::from("a.txt"), PathBuf::from("dir/b.md")]);
        assert_eq!(messages, s(&["hello", "world"]));
    }

    #[test]
    fn double_at_is_a_literal_message() {
        let (files, messages) = split_positionals(&s(&["@@handle", "hi"]));
        assert!(files.is_empty());
        assert_eq!(messages, s(&["@handle", "hi"]));
    }

    #[test]
    fn compose_merges_file_message_and_stdin_in_order() {
        let inputs = compose_inputs(
            Some("FILE BODY".to_string()),
            &s(&["first", "second", "third"]),
            Some("PIPED\n".to_string()),
        );
        assert_eq!(inputs.initial, "FILE BODY\n\nfirst\n\nPIPED");
        assert_eq!(inputs.follow_ups, s(&["second", "third"]));
    }

    #[test]
    fn compose_handles_message_only_and_stdin_only() {
        let only_msg = compose_inputs(None, &s(&["just a message"]), None);
        assert_eq!(only_msg.initial, "just a message");
        assert!(only_msg.follow_ups.is_empty());

        let only_stdin = compose_inputs(None, &[], Some("from stdin".to_string()));
        assert_eq!(only_stdin.initial, "from stdin");
        assert!(only_stdin.is_empty().eq(&false));

        let nothing = compose_inputs(None, &[], None);
        assert!(nothing.is_empty());
    }
}
