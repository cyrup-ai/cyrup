//! pi `27c7b6ff4` (v1.1.0, #10504) on the production path: a user `!` command whose backend splits
//! an escape sequence across output chunks reaches the result, the stream and the recorded
//! `bashExecution` message with the sequence stripped whole. Mirrors
//! `agent-session-bash-persistence.test.ts` "escape sequences split across output chunks"
//! @v1.1.0, through `execute_bash` with a [`BashOperations`] that emits the given chunks.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{BashOptions, SessionBuilder, SessionConfig};
use cyrup_agent::{AgentMessage, AppRole};
use cyrup_core::{StopReason, ToolError};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_tools::ExitStatus;
use cyrup_tools::ops::{BashExecOptions, BashOperations};
use tempfile::TempDir;

/// A backend that emits `chunks` as the command's output and exits 0.
struct ChunkedOps {
    chunks: Vec<Vec<u8>>,
}

#[async_trait::async_trait]
impl BashOperations for ChunkedOps {
    async fn exec(
        &self,
        _command: &str,
        _cwd: &std::path::Path,
        opts: BashExecOptions<'_>,
    ) -> Result<ExitStatus, ToolError> {
        for chunk in &self.chunks {
            (opts.on_data)(chunk);
        }
        Ok(ExitStatus::Exited(0))
    }
}

/// Run one `!` command over `chunks`; returns the result's output, everything streamed to the
/// caller's sink, and the `output` of the `bashExecution` message the session recorded.
async fn run_chunks(chunks: &[&[u8]]) -> (String, String, String) {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .build()
        .await
        .expect("build");

    let streamed = Arc::new(Mutex::new(String::new()));
    let sink_out = streamed.clone();
    let ops = ChunkedOps {
        chunks: chunks.iter().map(|chunk| chunk.to_vec()).collect(),
    };
    let result = session
        .execute_bash(
            "custom",
            BashOptions {
                exclude_from_context: false,
                id: None,
                operations: Some(Arc::new(ops) as Arc<dyn BashOperations>),
            },
            Some(Box::new(move |delta: &str| {
                sink_out.lock().unwrap().push_str(delta);
            })),
        )
        .await
        .expect("the command settles");
    let recorded = session
        .agent_messages()
        .await
        .iter()
        .rev()
        .find_map(|message| match message {
            AgentMessage::App {
                role: AppRole::BashExecution,
                payload,
            } => payload["output"].as_str().map(str::to_owned),
            _ => None,
        })
        .expect("a recorded bashExecution");
    let streamed = streamed.lock().unwrap().clone();
    (result.output, streamed, recorded)
}

#[tokio::test]
async fn strips_a_color_reset_split_inside_its_parameters() {
    let (output, streamed, recorded) =
        run_chunks(&[b"\x1b[31mERROR: file.py:1\x1b[0", b"m\n"]).await;
    assert_eq!(output, "ERROR: file.py:1\n");
    assert_eq!(streamed, "ERROR: file.py:1\n");
    assert_eq!(recorded, "ERROR: file.py:1\n");
}

#[tokio::test]
async fn strips_a_color_code_split_right_after_esc() {
    let (output, streamed, _) = run_chunks(&[b"before\x1b", b"[32mafter\n"]).await;
    assert_eq!(output, "beforeafter\n");
    assert_eq!(streamed, "beforeafter\n");
}

#[tokio::test]
async fn strips_an_osc_sequence_split_before_its_terminator() {
    let (output, _, _) = run_chunks(&[b"a\x1b]0;window ", b"title\x1b", b"\\b\n"]).await;
    assert_eq!(output, "ab\n");
}

#[tokio::test]
async fn flushes_an_incomplete_multi_byte_character_at_the_end_of_output() {
    let e_acute = "\u{e9}".as_bytes();
    let (output, streamed, _) = run_chunks(&[b"ok", &e_acute[..1]]).await;
    assert_eq!(output, "ok\u{FFFD}");
    assert_eq!(streamed, "ok\u{FFFD}");
}
