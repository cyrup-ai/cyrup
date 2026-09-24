//! `formatHerdrMachineHint(machine, text)` (`src/runs/shared/herdr-machine.ts:255-279` @v0.68.0):
//! one operator hint for a predictable remote failure, matched against the error text plus the
//! stderr tail, first match wins — and `decorateHerdrMachineResult`'s error decoration
//! (`src/runs/background/subagent-runner.ts:714-722`).
//!
//! Its output half (the "changes live on the machine" note for an exit-0 `-writer` external run,
//! `:723-724`) has no port: upstream's only caller sits on the ssh-wrapped external path that its
//! pane-native branch (`:900`) returns ahead of for every placed run, and a placed external run
//! always settles at exit 1 (`:928`), so the note is never produced upstream.
//!
//! The five patterns are upstream's regexes translated flag for flag (`i` → `(?i)`, `m` →
//! `(?m)`); the sentences are upstream's with the product name adapted.

use std::sync::LazyLock;

use regex::Regex;

use super::HerdrMachineReference;

type Hint = fn(&HerdrMachineReference) -> String;

static HINTS: LazyLock<Vec<(Regex, Hint)>> = LazyLock::new(|| {
    let table: [(&str, Hint); 5] = [
        (
            r"(?i)Host key verification failed|REMOTE HOST IDENTIFICATION HAS CHANGED|Permission denied \(publickey|Permission denied, please try again|No such identity|Could not resolve hostname|Connection (timed out|refused)",
            |machine| {
                format!(
                    "ssh could not reach or authenticate with {}. Connect once interactively with ssh {} to accept the host key or fix the identity; BatchMode never prompts.",
                    machine.target, machine.target
                )
            },
        ),
        (
            r"is not recognized as|CommandNotFoundException|PowerShell|At line:\d+ char:\d+",
            |machine| {
                format!(
                    "Machine '{}' is not a POSIX host. External-cli runs support POSIX ssh targets only.",
                    machine.display_name()
                )
            },
        ),
        (
            r"(?i)\bcd: .*(No such file or directory|not a directory|can't cd)|exit(?:ed with)? code 125\b",
            |machine| {
                format!(
                    "Nothing at {} on {}. Clone the repo there first; cyrup never clones, pulls, or checks out on a machine.",
                    machine.cwd,
                    machine.display_name()
                )
            },
        ),
        (
            r"(?im)(?:command not found|not found)\s*$|No such file or directory\s*$|exit(?:ed with)? code 127\b",
            |machine| {
                format!(
                    "The agent CLI was not found on {}. Non-interactive shells skip rc files and the PATH prefix did not find it; set the agent's command to the absolute path on that machine.",
                    machine.display_name()
                )
            },
        ),
        (
            r"(?i)not logged in|unauthorized|authentication_error|invalid api key|please run .*login|OAuth token",
            |machine| {
                format!(
                    "Remote runs use the machine's own credentials. Log in to the agent CLI on {} once.",
                    machine.display_name()
                )
            },
        ),
    ];
    table
        .into_iter()
        .filter_map(|(pattern, hint)| Regex::new(pattern).ok().map(|regex| (regex, hint)))
        .collect()
});

/// `formatHerdrMachineHint(machine, text)`.
#[must_use]
pub fn format_herdr_machine_hint(machine: &HerdrMachineReference, text: &str) -> Option<String> {
    HINTS
        .iter()
        .find(|(pattern, _)| pattern.is_match(text))
        .map(|(_, hint)| hint(machine))
}

/// `decorateHerdrMachineResult`'s error half (`subagent-runner.ts:715-722`): on a non-zero exit,
/// append the matching hint to the error (or to `Remote command exited with code N.` when there
/// was none).
#[must_use]
pub fn decorate_machine_error(
    machine: &HerdrMachineReference,
    exit_code: i32,
    error: Option<String>,
    stderr_tail: &str,
) -> Option<String> {
    if exit_code == 0 {
        return error;
    }
    let text = format!(
        "{}\n{stderr_tail}\nexit code {exit_code}",
        error.as_deref().unwrap_or_default()
    );
    match format_herdr_machine_hint(machine, &text) {
        Some(hint) => Some(format!(
            "{}\n{hint}",
            error.unwrap_or_else(|| format!("Remote command exited with code {exit_code}."))
        )),
        None => error,
    }
}
