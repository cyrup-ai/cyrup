//! A REAL `codemode` run drawn by the REAL interactive renderer.
//!
//! `cyrup-tui`'s own `codemode_renderer` tests feed hand-built events to the fold, and the session
//! tests read the tool result back as data. Neither shows what a person sees when a script that
//! made real calls (V8 sandbox, the built-in `read` and `bash` on disk) ends in an error after
//! showing an image. This test takes the events a real session emits for such a run, in order,
//! through the same `ingest_event_with_extensions` the interactive loop uses, and reads the
//! rendered cells and the committed scrollback.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use cyrup_codemode_runtime::CodemodeExtension;
use cyrup_codemode_runtime::tool::EngineSandboxFactory;
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text, faux_tool_call};
use cyrup_session_svc::{AgentSessionEvent, SessionBuilder, SessionConfig};
use cyrup_tui::{App, UiTheme};
use futures::StreamExt as _;
use ratatui::backend::TestBackend;
use serde_json::json;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

/// Every cell of the live region, row by row.
fn cells(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let mut live = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell((x, y)) {
                live.push_str(cell.symbol());
            }
        }
        live.push('\n');
    }
    live
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_script_that_fails_after_real_calls_and_an_image_is_drawn_with_its_calls_and_error()
{
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(cwd.join("a.txt"), "hello\n").unwrap();

    let faux = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    // `codemode` is a built-in of the path tier `--no-extensions` collapses.
    cfg.no_extensions = false;
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .with_codemode(CodemodeExtension::new(
            Default::default(),
            Arc::new(EngineSandboxFactory),
        ))
        .build()
        .await
        .unwrap()
        .into_shared();
    session
        .set_active_tools_by_name(&["codemode".to_owned(), "read".to_owned(), "bash".to_owned()])
        .await;

    let events: Arc<Mutex<Vec<AgentSessionEvent>>> = Arc::default();
    let mut stream = session.subscribe();
    let sink = Arc::clone(&events);
    tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            sink.lock().unwrap().push(event);
        }
    });

    let code = format!(
        "await tools.read({{ path: 'a.txt' }});\n\
         await tools.bash({{ command: 'echo hi; exit 2' }});\n\
         image('data:image/png;base64,{PNG}');\n\
         throw new Error('late')"
    );
    faux.set_response_steps(vec![
        faux_assistant_message(
            vec![faux_tool_call(
                "codemode".to_string(),
                json!({ "code": code }),
            )],
            StopReason::ToolUse,
        )
        .into(),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop).into(),
    ]);
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    // The collector drains on its own task: wait for it to reach the end of the run.
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(10)
        && !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, AgentSessionEvent::AgentSettled { .. }))
    {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let mut app = App::new(TestBackend::new(120, 40), UiTheme::dark()).unwrap();
    let host = session.ext_host().clone();
    let events = events.lock().unwrap().clone();
    for event in &events {
        app.ingest_event_with_extensions(event, &host).await;
    }
    app.draw().unwrap();
    let (cells, committed) = (cells(&app), app.scrollback_text());
    let seen = format!("{committed}\n{cells}");

    // The nested calls, in their parent's row: the glyph, the tool, its arguments. The bash that
    // exited 2 is a failed call even though the script received its result.
    assert!(seen.contains("✓ read {\"path\":\"a.txt\"}"), "{seen}");
    assert!(
        seen.contains("✗ bash {\"command\":\"echo hi; exit 2\"}"),
        "{seen}"
    );
    // The failure and where it happened.
    assert!(seen.contains("Error: late"), "{seen}");
    assert!(seen.contains("codemode.js:4:"), "{seen}");
    // The image the script showed is named by the file it was saved to.
    let saved = seen
        .split("[Image saved to ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .unwrap_or_else(|| panic!("no saved-image label:\n{seen}"))
        .to_owned();
    drop(std::fs::remove_file(&saved));
    // No nested call is drawn a second time as a row of its own, on either surface: a `read` tool
    // row of its own would read `read a.txt` and a `bash` one `$ echo hi; exit 2`.
    for (surface, text) in [("committed", &committed), ("cells", &cells)] {
        assert_eq!(
            text.matches("✓ read {\"path\"").count(),
            1,
            "{surface}:\n{text}"
        );
        assert!(!text.contains("read a.txt"), "{surface}:\n{text}");
        assert!(!text.contains("$ echo hi; exit 2"), "{surface}:\n{text}");
    }
}
