#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod x13_live_bash_tests {
    use crate::UiTheme;
    use crate::app::*;
    use cyrup_provider::Provider;
    use cyrup_provider::faux::FauxProvider;
    use cyrup_session_svc::AgentSession;
    use cyrup_session_svc::{SessionBuilder, SessionConfig};
    use ratatui::backend::TestBackend;
    use std::sync::Arc;

    /// ~3000 lines x ~40 bytes ≈ 120 KB — comfortably past `truncate.ts:11-12`'s 2000-line / 50 KB
    /// pair, so `bash-executor.ts`'s `ensureTempFile` spill and `truncateTail` both fire.
    const BIG: &str = "for i in $(seq 1 3000); do echo \"line-number-$i-padding-xxxxxxxxxx\"; done";

    async fn session(dir: &std::path::Path) -> Arc<AgentSession> {
        let cwd = dir.join("project");
        let agent_dir = dir.join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let mut cfg = SessionConfig::new(cwd, agent_dir);
        cfg.trust_override = Some(true);
        Arc::new(SessionBuilder::new(faux, cfg).build().await.unwrap())
    }

    /// Drive `spawn_session_bash` with EXACTLY the two `select!` arms the run loop uses, so the
    /// assertion covers the real wiring and not a re-implementation of it.
    async fn run_block(app: &mut App<TestBackend>, session: Arc<AgentSession>, command: &str) {
        app.state_mut()
            .transcript
            .start_bash(command.to_string(), false, None, None);
        let mut rx = spawn_session_bash(session, command.to_string(), false);
        while let Some(msg) = rx.recv().await {
            match msg {
                BashMsg::Chunk(chunk) => app.state_mut().transcript.bash_append(&chunk),
                BashMsg::Done {
                    exit_code,
                    cancelled,
                    truncated,
                    full_output_path,
                } => {
                    app.state_mut().transcript.bash_complete(
                        exit_code,
                        cancelled,
                        truncated,
                        full_output_path,
                    );
                    app.state_mut().transcript.commit_bash();
                    break;
                }
            }
        }
        app.draw().unwrap();
    }

    /// **X13 — a LIVE `!` run names its spool file.**
    ///
    /// `bash-execution.ts:195-199`:
    /// ```ts
    /// const wasTruncated = this.truncationResult?.truncated || contextTruncation.truncated;
    /// if (wasTruncated && this.fullOutputPath) {
    ///     statusParts.push(theme.fg("warning", `Output truncated. Full output: ${this.fullOutputPath}`));
    /// }
    /// ```
    /// `fullOutputPath` is `setComplete`'s FOURTH argument, which `handleBashCommand` passes from
    /// `result.fullOutputPath` (`interactive-mode.ts:6352`). The old local `sh -c` pump had no
    /// spool at all and always passed `false, None`, so the row was unreachable outside replay —
    /// even though `contextTruncation.truncated` was already true here (120 KB / 3000 lines), which
    /// is exactly why the `&& this.fullOutputPath` leg is what this test turns on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_live_bash_run_names_its_spool_file() {
        let dir = tempfile::tempdir().unwrap();
        let session = session(dir.path()).await;
        let mut app = App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap();
        run_block(&mut app, session, BIG).await;

        let out = app.scrollback_text();
        // TUI-N13: pi emits the whole status block as `new Text(`\n${statusParts.join("\n")}`, 1, 0)`
        // (`bash-execution.ts:201` @v0.83.0) — padding-left 1, WORD-WRAPPED to the terminal width —
        // and cyrup renders it the same way, so a status part longer than the width legitimately
        // occupies two visual lines in BOTH. The spool path comes from `std::env::temp_dir()`
        // (`cyrup-session-svc/src/bash.rs:258`), which on macOS is `/var/folders/<2>/<30>/T/`: at
        // that length ` Output truncated. Full output: <path>` is exactly 120 columns and the path
        // wraps onto the next line, while on Linux (`TMPDIR` unset -> `/tmp`) it does not. Reading a
        // single `.lines()` entry therefore asserted the length of the ambient TMPDIR rather than the
        // wiring under test. Flatten the wrap first; the assertion itself is unchanged and still
        // requires the RENDERED scrollback to name the executor's own spool file.
        let flat = out.split_whitespace().collect::<Vec<_>>().join(" ");
        let path = flat
            .split_once("Output truncated. Full output: ")
            .unwrap_or_else(|| panic!("no truncation row in a 120 KB live run:\n{out}"))
            .1
            .split_whitespace()
            .next()
            .unwrap_or_else(|| panic!("the truncation row named no file:\n{out}"))
            .to_string();
        assert!(
            path.contains("cyrup-bash-"),
            "the spool file is the executor's: {path}"
        );

        // The named file really holds the FULL output — the row is not a decorative string.
        let spooled = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("the spool file must be readable at {path}: {e}"));
        assert!(
            spooled.contains("line-number-1-padding"),
            "nothing dropped from the front"
        );
        assert!(
            spooled.contains("line-number-3000-padding"),
            "nor from the tail"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// MIRROR — a SMALL live run spools nothing, so the row must not appear. Proves the wiring
    /// forwards the executor's real report rather than hard-coding the other constant.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_small_live_bash_run_has_no_truncation_row() {
        let dir = tempfile::tempdir().unwrap();
        let session = session(dir.path()).await;
        let mut app = App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap();
        run_block(&mut app, session, "echo hello-small").await;

        let out = app.scrollback_text();
        assert!(out.contains("hello-small"), "the output rendered:\n{out}");
        assert!(
            !out.contains("Output truncated"),
            "and nothing was spooled:\n{out}"
        );
    }

    /// The run is recorded through the session's own `recordBashResult` (`agent-session.ts:2628`)
    /// — WITH the truncation fields, which is what makes the replay arm able to restore the row.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_live_run_records_a_bash_execution_message_carrying_the_spool_path() {
        let dir = tempfile::tempdir().unwrap();
        let session = session(dir.path()).await;
        let mut app = App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap();
        run_block(&mut app, session.clone(), BIG).await;

        let msgs = session.agent_messages().await;
        let payload = msgs
            .iter()
            .find_map(|m| match m {
                cyrup_agent::AgentMessage::App {
                    role: cyrup_agent::AppRole::BashExecution,
                    payload,
                    ..
                } => Some(serde_json::Value::Object(payload.clone())),
                _ => None,
            })
            .expect("`executeBash` records the run itself — the caller must not append its own");
        assert_eq!(payload["truncated"], true);
        let path = payload["fullOutputPath"]
            .as_str()
            .expect("the spool path persisted");
        assert!(path.contains("cyrup-bash-"));
        let _ = std::fs::remove_file(path);
    }
}
