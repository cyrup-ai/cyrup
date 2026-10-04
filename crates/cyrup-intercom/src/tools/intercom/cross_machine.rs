//! The `name@machine` arm of `intercom{action:"send"}` (`v0.16.0 index.ts:1704-1733`, inside the
//! shared `deliverMessage`) — ICOM-074's sending half.
//!
//! Split from [`super::send`] rather than inlined because this arm shares nothing with the local
//! one after the confirm: no target resolution, no reply-tracker interaction, no `SendOptions`, no
//! broker round trip. It takes the delivery through SSH, and the only state it touches is the
//! confirm dialog and the audit entry.

use std::sync::Arc;

use cyrup_core::{ToolError, ToolResult};

use crate::cross_machine::{
    CommandRunner, CrossMachineDeps, CrossMachineOrigin, relay_sender_name, send_cross_machine,
};
use crate::session_state::SharedIntercomState;
use crate::tools::{detailed_result, text_result};
use crate::transport::protocol::now_ms;

/// `process.env.HERDR_BIN_PATH ?? "herdr"` (`v0.16.0 cross-machine-transport.ts:79`,
/// `index.ts:3052`).
///
/// **Not `HERDR_BIN`,** and not a typo here: upstream's cross-machine files read this *other*
/// spelling, while `project-agent.ts:68` (cyrup: [`cyrup_herdr::cli::HERDR_BIN`]) reads
/// `HERDR_BIN`, and neither falls back to the other. Adding a fallback would be inventing upstream
/// behaviour, so the two variables stay as upstream has them; both belong to the herdr vendor, so
/// neither takes a `CYRUP_` prefix.
pub const HERDR_BIN_PATH: &str = "HERDR_BIN_PATH";

/// The `herdr` binary cross-machine discovery runs, from [`HERDR_BIN_PATH`].
///
/// A blank value falls through to `"herdr"`, as every other binary ladder in this workspace does
/// for a variable that was unset badly.
#[must_use]
pub(super) fn herdr_bin_from(env: impl Fn(&str) -> Option<String>) -> String {
    env(HERDR_BIN_PATH)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| cyrup_herdr::cli::HERDR_BIN_DEFAULT.to_string())
}

/// `v0.16.0 index.ts:1704-1733` — confirm, relay over SSH, append the audit entry, report.
///
/// `to` is the caller's `name@machine` verbatim; it has already passed
/// [`super::explicit_cross_machine_send_restriction`] and
/// [`crate::cross_machine::parse_cross_machine_target`], so this function neither re-validates it
/// nor falls back to a local send — a `@` target that cannot be relayed is an error, never a
/// silently-local delivery.
///
/// # Errors
/// The user cancelled is NOT an error (`Message cancelled by user`, as the local arm). Everything
/// else is `Explicit cross-machine message to "{to}" was not delivered: {reason}` — ONE sentence
/// for discovery, SSH and relay refusals alike, because from the model's side they are all "the
/// peer did not get it".
pub(super) async fn deliver_cross_machine(
    state: &Arc<SharedIntercomState>,
    runner: &dyn CommandRunner,
    to: &str,
    message: &str,
    origin: CrossMachineOrigin,
    confirm_send: bool,
) -> Result<ToolResult, ToolError> {
    // `v0.16.0 index.ts:1693-1701`: `Send to "${to}":\n\n${message}` — the CALLER's target, since
    // nothing is resolved yet, and no attachment text, since attachments are already refused.
    if confirm_send
        && let Some(services) = state.host_services()
        && !services.confirm(
            "Send Message",
            &format!("Send to \"{to}\":\n\n{message}"),
            &cyrup_ext::DialogOptions::default(),
        )
    {
        return Ok(text_result("Message cancelled by user"));
    }
    let herdr_bin = herdr_bin_from(|key| std::env::var(key).ok());
    let remote_command = state.config.cross_machine.remote_command.clone();
    let deps = CrossMachineDeps::new(runner, &herdr_bin, &remote_command);
    let remote = match send_cross_machine(to, message, origin, &deps).await {
        Ok(remote) => remote,
        Err(error) => {
            return Err(ToolError::new(format!(
                "Explicit cross-machine message to \"{to}\" was not delivered: {error}"
            )));
        }
    };
    // `relaySenderName({ name: remote.agent.name, machine: remote.machine.label })` (`:1717`) —
    // the DISCOVERED name and label, not the caller's spelling, so a case-insensitive match reports
    // what the remote machine actually calls itself.
    let remote_target = relay_sender_name(
        &remote.discovered.agent.name,
        &remote.discovered.machine.label,
    );
    if let Some(services) = state.host_services() {
        // `:1718-1723` — `{ to, message: { text }, timestamp, crossMachine: true }`. Narrower than
        // the local arm's entry on purpose: there is no message id to record (the relay minted one
        // on the far side) and no `replyTo` to record (reply relationships are refused).
        let appended = services.append_entry(
            "intercom_sent",
            &serde_json::json!({
                "to": remote_target,
                "message": { "text": message },
                "timestamp": now_ms(),
                "crossMachine": true,
            }),
        );
        if let Err(e) = appended {
            tracing::warn!(error = %e, kind = "intercom_sent", "intercom: failed to append audit entry");
        }
    }
    Ok(detailed_result(
        format!("Message sent to {remote_target} over SSH (origin identity is SSH-asserted)"),
        serde_json::json!({
            "delivered": true,
            "crossMachine": true,
            "machine": remote.discovered.machine.label,
            "target": remote.discovered.agent.name,
            "trust": "ssh-asserted",
        }),
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::sync::Mutex;
    use std::time::Duration;

    use cyrup_core::Content;

    use crate::config::IntercomConfig;
    use crate::cross_machine::CommandResult;

    use super::*;

    /// A runner that answers `herdr machine list`, `herdr --machine … agent list` and `ssh` from a
    /// fixed script, recording the `ssh` argv so the remote command line can be asserted.
    struct Relay {
        ssh: CommandResult,
        ssh_argv: Mutex<Vec<String>>,
    }

    const MACHINES: &str = r#"[{"label":"workstation","target":"user@ws","enabled":true}]"#;
    const AGENTS: &str = r#"{"agents":[{"agent":"cyrup","name":"reviewer"}]}"#;

    #[async_trait::async_trait]
    impl CommandRunner for Relay {
        async fn run(
            &self,
            command: &str,
            args: &[&str],
            _stdin: Option<&str>,
            _timeout: Option<Duration>,
        ) -> std::io::Result<CommandResult> {
            if command == "ssh" {
                *self.ssh_argv.lock().unwrap_or_else(|e| e.into_inner()) =
                    args.iter().map(|a| (*a).to_string()).collect();
                return Ok(self.ssh.clone());
            }
            Ok(CommandResult {
                stdout: if args.first() == Some(&"machine") {
                    MACHINES.to_string()
                } else {
                    AGENTS.to_string()
                },
                stderr: String::new(),
                code: 0,
                timed_out: false,
            })
        }
    }

    /// Records the audit entries and the confirm prompts, and answers the confirm as told.
    struct Host {
        confirm: bool,
        prompts: Mutex<Vec<String>>,
        entries: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl cyrup_ext::HostServices for Host {
        fn session_id(&self) -> Option<String> {
            Some("sess-local".to_string())
        }
        fn session_name(&self) -> Option<String> {
            Some("alice".to_string())
        }
        fn confirm(&self, _prompt: &str, message: &str, _opts: &cyrup_ext::DialogOptions) -> bool {
            self.prompts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(message.to_string());
            self.confirm
        }
        fn append_entry(
            &self,
            custom_type: &str,
            data: &serde_json::Value,
        ) -> Result<String, String> {
            self.entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((custom_type.to_string(), data.clone()));
            Ok("entry-1".to_string())
        }
    }

    fn fixture(
        ssh: CommandResult,
        confirm: bool,
    ) -> (Arc<SharedIntercomState>, Arc<Host>, Arc<Relay>) {
        let state = Arc::new(SharedIntercomState::new(
            IntercomConfig {
                cross_machine: crate::config::CrossMachineConfig {
                    machine_name: "laptop".to_string(),
                    remote_command: "cyrup-intercom-cli".to_string(),
                },
                ..IntercomConfig::default()
            },
            600_000,
            std::path::PathBuf::from("/w"),
        ));
        let host = Arc::new(Host {
            confirm,
            prompts: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
        });
        state.set_host_services(host.clone());
        let relay = Arc::new(Relay {
            ssh,
            ssh_argv: Mutex::new(Vec::new()),
        });
        (state, host, relay)
    }

    fn origin() -> CrossMachineOrigin {
        CrossMachineOrigin {
            name: "alice".to_string(),
            session_id: "sess-local".to_string(),
            machine: "laptop".to_string(),
        }
    }

    fn text(result: &ToolResult) -> String {
        result
            .content
            .iter()
            .map(|c| match c {
                Content::Text { text, .. } => text.to_string(),
                _ => String::new(),
            })
            .collect()
    }

    /// `v0.16.0 index.ts:1711-1726` — the success sentence, the `details` quintet, and the audit
    /// entry's `crossMachine: true`.
    #[tokio::test]
    async fn a_relayed_send_reports_ssh_asserted_trust_and_audits_the_remote_target() {
        let (state, host, relay) = fixture(
            CommandResult {
                stdout: r#"{"ok":true,"delivered":true,"id":"m-9"}"#.to_string(),
                stderr: String::new(),
                code: 0,
                timed_out: false,
            },
            true,
        );
        let result = deliver_cross_machine(
            &state,
            relay.as_ref(),
            "reviewer@workstation",
            "ship it",
            origin(),
            true,
        )
        .await
        .expect("delivered");

        assert_eq!(
            text(&result),
            "Message sent to reviewer@workstation over SSH (origin identity is SSH-asserted)"
        );
        assert_eq!(
            result.details,
            Some(serde_json::json!({
                "delivered": true,
                "crossMachine": true,
                "machine": "workstation",
                "target": "reviewer",
                "trust": "ssh-asserted",
            }))
        );
        assert_eq!(
            host.prompts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            ["Send to \"reviewer@workstation\":\n\nship it".to_string()],
            "the confirm names the CALLER's target and carries no attachment text"
        );
        let entries = host.entries.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "intercom_sent");
        assert_eq!(entries[0].1["to"], "reviewer@workstation");
        assert_eq!(entries[0].1["message"]["text"], "ship it");
        assert_eq!(entries[0].1["crossMachine"], true);
        assert!(
            entries[0].1.get("messageId").is_none()
                && entries[0].1["message"].get("replyTo").is_none(),
            "the relay minted the id on the far side and reply edges are refused: {:?}",
            entries[0].1
        );
        assert_eq!(
            relay
                .ssh_argv
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            [
                "user@ws".to_string(),
                "cyrup-intercom-cli relay --envelope-stdin --json".to_string()
            ],
            "`remoteCommand` comes from the config, not a literal"
        );
    }

    /// A declined confirm cancels WITHOUT running `ssh` — the dialog is a veto on the side effect.
    #[tokio::test]
    async fn a_declined_confirm_sends_nothing() {
        let (state, _host, relay) = fixture(
            CommandResult {
                stdout: r#"{"ok":true}"#.to_string(),
                stderr: String::new(),
                code: 0,
                timed_out: false,
            },
            false,
        );
        let result = deliver_cross_machine(
            &state,
            relay.as_ref(),
            "reviewer@workstation",
            "ship it",
            origin(),
            true,
        )
        .await
        .expect("cancelled is not an error");
        assert_eq!(text(&result), "Message cancelled by user");
        assert!(
            relay
                .ssh_argv
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "nothing is relayed after a declined confirm"
        );
    }

    /// `v0.16.0 index.ts:1728-1733` — ONE failure sentence, naming the caller's target, whatever
    /// went wrong underneath. The audit entry must NOT be appended for an undelivered send.
    #[tokio::test]
    async fn a_refused_relay_names_the_target_and_audits_nothing() {
        let (state, host, relay) = fixture(
            CommandResult {
                stdout: r#"{"ok":false,"error":"Session \"reviewer\" is not connected."}"#
                    .to_string(),
                stderr: String::new(),
                code: 1,
                timed_out: false,
            },
            false,
        );
        let error = deliver_cross_machine(
            &state,
            relay.as_ref(),
            "reviewer@workstation",
            "ship it",
            origin(),
            false,
        )
        .await
        .expect_err("refused");
        assert_eq!(
            error.to_string(),
            "Explicit cross-machine message to \"reviewer@workstation\" was not delivered: \
             Remote intercom delivery via workstation failed: Session \"reviewer\" is not connected."
        );
        assert!(
            host.entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "an undelivered send leaves no `intercom_sent` entry"
        );
    }

    /// `process.env.HERDR_BIN_PATH ?? "herdr"` (`v0.16.0 cross-machine-transport.ts:79`) — and a
    /// blank value is an env var that was unset badly, not a request to exec the empty string.
    #[test]
    fn the_herdr_binary_comes_from_herdr_bin_path_only() {
        assert_eq!(herdr_bin_from(|_| None), "herdr");
        assert_eq!(herdr_bin_from(|_| Some("  ".to_string())), "herdr");
        assert_eq!(
            herdr_bin_from(|key| (key == HERDR_BIN_PATH).then(|| "/opt/herdr".to_string())),
            "/opt/herdr"
        );
        assert_eq!(
            herdr_bin_from(|key| (key == "HERDR_BIN").then(|| "/opt/other".to_string())),
            "herdr",
            "upstream reads HERDR_BIN_PATH here and does NOT fall back to HERDR_BIN"
        );
    }
}
