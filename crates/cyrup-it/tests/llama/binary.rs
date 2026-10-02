//! The llama.cpp chain through the REAL `cyrup` binary: argv in, stdout and the wire out.
//!
//! Every child is hermetic (`support::env::hermetic`: `env_clear` plus an allowlist), pointed at a
//! temp agent dir, `--offline`, and the loopback fake router ([`super::fake`]). Nothing here can
//! reach a real provider or a real llama-server.
//!
//! # How a llama.cpp model reaches a non-interactive run
//!
//! Pi has no startup network read for it. `createAgentSessionServices` restores the PERSISTED
//! catalog with no network (`core/agent-session-services.ts:190-206`, `refresh({ allowNetwork:
//! false })`), and the catalog is written by an earlier refresh: `/llama`'s, or the detached
//! `void modelRuntime.refresh()` after startup (`main.ts:936`). The provider's `refreshModels`
//! restores that cache first (`extensions/llama/provider.ts:203-221`). So these tests seed the two
//! things a configured install has on disk, exactly as `/login llama.cpp` and a completed refresh
//! leave them: the credential in `auth.json` and the catalog in `models-store.json`.
//!
//! # What these tests stand on
//!
//! The host wiring the builder makes, which every test that needs the persisted llama.cpp catalog
//! to be visible to the binary (`--list-models`, `--model llama.cpp/<id>`) depends on, and which
//! its failure message names when it is missing:
//!
//! 1. the startup restore: `SessionBuilder::build` calls `GuestProviderRegistry::restore_cached`
//!    over the `<agent_dir>/models-store.json` it attaches, so the cached catalog is handed to the
//!    provider (pi `agent-session-services.ts:190-206`);
//! 2. `--model <provider>/<id>` resolution against an EXTENSION-registered provider: the launch
//!    path defers a provider no built-in or `models.json` block declares, and the builder resolves
//!    the pattern once the extension providers exist (pi resolves after the extensions loaded,
//!    `main.ts` `buildSessionOptions`);
//! 3. the provider's credential store: the builder attaches the source that answers
//!    `HostServices::provider_credentials` before the native's `init`, so the stored key reaches
//!    the stream. The same attachments make `/llama`'s `provider_auth` and `refresh_provider` host
//!    verbs answer.
//!
//! [`super::session_chain`] holds the same behaviours one step short of the binary, so a regression
//! there is told apart from one in the binary's own launch path.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::fake::{FakeLlama, PLAIN_TEMPLATE, THINKING_TEMPLATE, model_with};
use super::fixture::{
    Fx, PROVIDER, cached_chat_model, fixture, seed_catalog_cache, store_credential,
};

const KEY: &str = "sk-llama-fixture";

/// A hermetic `cyrup` command over `fx`: cleared environment, the temp agent dir, offline.
fn cyrup(fx: &Fx) -> std::process::Command {
    let mut cmd = crate::support::env::hermetic(crate::support::bins::cyrup(), &fx.home);
    cmd.current_dir(&fx.cwd)
        .env("CYRUP_AGENT_DIR", &fx.agent_dir)
        .env("CYRUP_OFFLINE", "1")
        .stdin(Stdio::null());
    cmd
}

/// One finished run.
struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(mut cmd: std::process::Command) -> Run {
    let out = cmd.output().expect("spawn cyrup");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// A router with the two models the cache below names, a configured install: stored credential
/// and the persisted catalog a completed refresh wrote.
async fn configured_install(fx: &Fx) -> FakeLlama {
    let fake = FakeLlama::start(vec![
        model_with("qwen3", "loaded", json!({ "meta": { "n_ctx": 8192 } })),
        model_with("plain", "loaded", json!({ "meta": { "n_ctx": 4096 } })),
    ])
    .await;
    fake.set_chat_template("qwen3", THINKING_TEMPLATE);
    fake.set_chat_template("plain", PLAIN_TEMPLATE);
    store_credential(fx, fake.url(), Some(KEY));
    seed_catalog_cache(
        fx,
        vec![
            cached_chat_model("qwen3", fake.url(), 8192, true),
            cached_chat_model("plain", fake.url(), 4096, false),
        ],
    );
    fake
}

/// The `--list-models` rows of the llama.cpp provider.
fn llama_rows(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter(|line| line.split_whitespace().next() == Some(PROVIDER))
        .map(str::to_string)
        .collect()
}

// ------------------------------------------------------------------------------- --list-models --

/// Scenario 1, listing: the configured server's models appear in `--list-models`, with their
/// context window and thinking support, read from the persisted catalog without touching the
/// network (pi lists after the cache-only restore and exits, `main.ts:866-871`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_shows_the_configured_servers_models() {
    let fx = fixture();
    let fake = configured_install(&fx).await;

    let mut cmd = cyrup(&fx);
    cmd.arg("--list-models");
    let out = run(cmd);

    assert_eq!(
        out.code, 0,
        "stderr: {}\nstdout: {}",
        out.stderr, out.stdout
    );
    let rows: Vec<Vec<String>> = llama_rows(&out.stdout)
        .iter()
        .map(|row| row.split_whitespace().map(str::to_string).collect())
        .collect();
    // `provider model context max-out thinking images` (`cli/list-models.ts`): `maxTokens` is the
    // context window (`provider.ts:107`), `qwen3` reasons (its template mentions `enable_thinking`).
    // Counts print through `formatTokenCount` (`cli/list-models.ts:14-24`): a whole number of
    // thousands is `NK`, anything else `N.toFixed(1)K`, so 4096 is `4.1K` and 8192 is `8.2K`.
    for expected in [
        ["llama.cpp", "plain", "4.1K", "4.1K", "no", "no"],
        ["llama.cpp", "qwen3", "8.2K", "8.2K", "yes", "no"],
    ] {
        assert!(
            rows.iter().any(|row| row == &expected),
            "`--list-models` lacks the cached llama.cpp row {expected:?}; startup never handed the \
             persisted catalog to the provider (see this file's header, point 1). Output:\n{}",
            out.stdout
        );
    }
    assert!(
        fake.requests().is_empty(),
        "a listing run is cache-only and issues no request: {:?}",
        fake.requests()
    );
}

/// Scenario 2: with no configuration llama.cpp is unavailable and nothing is listed, even though a
/// catalog is cached on disk and a server is running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_without_configuration_lists_nothing_for_llama() {
    let fx = fixture();
    let fake = configured_install(&fx).await;
    std::fs::remove_file(fx.agent_dir.join("auth.json")).unwrap();

    let mut cmd = cyrup(&fx);
    cmd.arg("--list-models");
    let out = run(cmd);

    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(
        llama_rows(&out.stdout).is_empty(),
        "an unconfigured llama.cpp lists nothing: {}",
        out.stdout
    );
    assert!(
        out.stdout.contains("No models available"),
        "with nothing configured the listing is pi's no-models guidance: {}",
        out.stdout
    );
    assert!(fake.requests().is_empty());
}

/// Scenario 3: `--no-extensions` drops the built-in, so a fully configured install lists nothing
/// for it. Its control is [`list_models_shows_the_configured_servers_models`], the same fixture
/// without the flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_extensions_removes_llama_from_the_listing() {
    let fx = fixture();
    let _fake = configured_install(&fx).await;

    let mut cmd = cyrup(&fx);
    cmd.args(["--no-extensions", "--list-models"]);
    let out = run(cmd);

    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(
        llama_rows(&out.stdout).is_empty(),
        "`--no-extensions` drops the llama.cpp built-in: {}",
        out.stdout
    );
}

// ----------------------------------------------------------------------------------- one-shot --

/// Scenario 1, the turn: `cyrup -p --model llama.cpp/qwen3` streams its reply through the router's
/// `/v1/chat/completions`, authenticated with the stored key, with `enable_thinking` in
/// `chat_template_kwargs` following `--thinking` (`provider.ts:97-98`, `:119-125`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_one_shot_turn_streams_through_the_router() {
    let fx = fixture();
    let fake = configured_install(&fx).await;
    fake.set_reply("Hello from llama.cpp");

    let mut cmd = cyrup(&fx);
    cmd.args([
        "-p",
        "--no-session",
        "--model",
        "llama.cpp/qwen3",
        "--thinking",
        "medium",
        "say hi",
    ]);
    let out = run(cmd);

    assert_eq!(
        out.code, 0,
        "`--model llama.cpp/qwen3` was not accepted, see this file's header, point 2.\nstderr: {}\nstdout: {}",
        out.stderr, out.stdout
    );
    assert!(
        out.stdout.contains("Hello from llama.cpp"),
        "the reply streamed off the router reaches stdout: {}",
        out.stdout
    );
    let calls = fake.wait_for("POST", "/v1/chat/completions", 1).await;
    let request = &calls[0];
    let body = request.json();
    assert_eq!(body["model"], "qwen3");
    assert_eq!(body["stream"], true);
    assert_eq!(
        body["chat_template_kwargs"]["enable_thinking"], true,
        "{body}"
    );
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {KEY}").as_str()),
        "the stored key reaches the stream (this file's header, point 3)"
    );

    // `--thinking off` flips the same kwarg.
    let mut cmd = cyrup(&fx);
    cmd.args([
        "-p",
        "--no-session",
        "--model",
        "llama.cpp/qwen3",
        "--thinking",
        "off",
        "say hi",
    ]);
    let out = run(cmd);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    let calls = fake.wait_for("POST", "/v1/chat/completions", 2).await;
    assert_eq!(
        calls[1].json()["chat_template_kwargs"]["enable_thinking"],
        false,
        "{}",
        calls[1].body
    );
}

/// `--provider llama.cpp --model custom-id` reaches the extension's provider for an id its catalog
/// does not list: the launch defers the provider choice to the session, and the explicit
/// `--provider` is folded into the pattern the session resolves as `llama.cpp/custom-id`, which is
/// what lets the session find the owning provider by the prefix (`resolveCliModel`'s custom-id
/// fallback, `model-resolver.ts:475-501`). Without the folding the bare, unlisted id names no
/// provider at all and the launch is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_provider_flag_qualifies_an_id_the_catalog_does_not_list() {
    let fx = fixture();
    let fake = configured_install(&fx).await;
    fake.set_reply("Hello via --provider");

    let mut cmd = cyrup(&fx);
    cmd.args([
        "-p",
        "--no-session",
        "--provider",
        PROVIDER,
        "--model",
        "custom-id",
        "say hi",
    ]);
    let out = run(cmd);

    assert_eq!(
        out.code, 0,
        "`--provider llama.cpp --model custom-id` was not accepted.\nstderr: {}\nstdout: {}",
        out.stderr, out.stdout
    );
    assert!(
        out.stdout.contains("Hello via --provider"),
        "the reply came off the llama.cpp router: {}",
        out.stdout
    );
    fake.wait_for("POST", "/v1/chat/completions", 1).await;
}

// ---------------------------------------------------------------------------------- /llama -----

/// A `cyrup --mode rpc` child whose stdout is read line by line on a thread, so a test can wait
/// for a specific event with a deadline instead of for EOF.
struct Rpc {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    lines: Receiver<String>,
    seen: Vec<Value>,
}

impl Rpc {
    fn spawn(fx: &Fx, extra: &[&str]) -> Self {
        Self::spawn_with(fx, extra, false)
    }

    /// `online` lifts the `CYRUP_OFFLINE` the shared [`cyrup`] command sets, so the startup refresh
    /// runs (the update check is interactive-only, and is switched off anyway).
    fn spawn_with(fx: &Fx, extra: &[&str], online: bool) -> Self {
        let mut cmd = cyrup(fx);
        if online {
            cmd.env_remove("CYRUP_OFFLINE")
                .env("CYRUP_SKIP_VERSION_CHECK", "1");
        }
        cmd.args(["--mode", "rpc", "--no-session", "--model", "faux/faux-1"])
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().expect("spawn cyrup --mode rpc");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            seen: Vec::new(),
        }
    }

    fn send(&mut self, command: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{command}").unwrap();
        stdin.flush().unwrap();
    }

    /// Read events until one satisfies `done` (it is included in `seen`), or fail after 30 s.
    fn read_until(&mut self, what: &str, done: impl Fn(&Value) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    let Ok(event) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    let finished = done(&event);
                    self.seen.push(event);
                    if finished {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("no {what} within 30s; saw {:#?}", self.seen)
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!(
                        "the child closed stdout before {what}; saw {:#?}",
                        self.seen
                    )
                }
            }
        }
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        // EOF on stdin is the protocol's shutdown; the kill is only for a wedged child.
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn is_llama_warning(event: &Value) -> bool {
    event["type"] == "extension_ui_request"
        && event["method"] == "notify"
        && event["message"] == "/llama is available in interactive mode"
        && event["notifyType"] == "warning"
}

/// Scenario 5: `/llama` outside the interactive UI warns and does nothing else (`index.ts:185-191`:
/// `if (ctx.mode !== "tui") { ctx.ui.notify("/llama is available in interactive mode", "warning");
/// return; }`). RPC mode is where a non-interactive notification is observable: print and json
/// modes bind no UI and pi's `notify` there is a no-op, so there is nothing to read there. (A
/// prompt that is only a slash command also left `--mode print` running past 60 s when this was
/// tried by hand, with `/mcp` as well as `/llama`: a host defect, not an llama.cpp one, recorded in
/// the lane report.)
///
/// The command is consumed by the extension, so the model never sees it: no agent run starts.
#[test]
fn the_rpc_slash_command_warns_in_a_non_interactive_mode() {
    let fx = fixture();
    let mut rpc = Rpc::spawn(&fx, &[]);
    rpc.send(&json!({ "id": "1", "type": "prompt", "message": "/llama" }));

    rpc.read_until("the /llama warning", is_llama_warning);

    assert!(
        rpc.seen
            .iter()
            .all(|event| event["type"] != "agent_start" && event["type"] != "message_start"),
        "the command was handled by the extension, not sent to the model: {:#?}",
        rpc.seen
    );
}

/// The control for scenario 3 at the command seam: `--no-extensions` leaves nothing to handle
/// `/llama`, so the text goes to the model as an ordinary prompt and no warning is ever emitted.
#[test]
fn no_extensions_leaves_the_slash_command_to_the_model() {
    let fx = fixture();
    let mut rpc = Rpc::spawn(&fx, &["--no-extensions"]);
    rpc.send(&json!({ "id": "1", "type": "prompt", "message": "/llama" }));

    rpc.read_until("the run to settle", |event| {
        event["type"] == "agent_settled"
    });

    assert!(
        rpc.seen.iter().any(|event| {
            event["type"] == "message_start"
                && event["message"]["role"] == "user"
                && event["message"]["content"][0]["text"] == "/llama"
        }),
        "`/llama` reached the model as a user message: {:#?}",
        rpc.seen
    );
    assert!(
        !rpc.seen.iter().any(is_llama_warning),
        "no extension handled it: {:#?}",
        rpc.seen
    );
}

/// The detached startup refresh of the extension providers' catalogs (`main.ts:931-936`): an rpc
/// run that is not offline reads the configured server's `GET /models` on its own, with no prompt
/// and no `/llama`, so the catalog a person looks at in `/model` is current.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_online_rpc_run_refreshes_the_llama_catalog_at_startup() {
    let fx = fixture();
    let fake = configured_install(&fx).await;

    let _rpc = Rpc::spawn_with(&fx, &[], true);

    let listings = fake.wait_for("GET", "/models", 1).await;
    assert_eq!(
        listings[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str()),
        "the refresh authenticates with the stored key"
    );
}

/// The control for the startup refresh: the same install, same mode, with `CYRUP_OFFLINE` set (as
/// every other test here has it) reads nothing from the server, so the request above is the
/// refresh and not something the session does anyway.
#[test]
fn an_offline_rpc_run_issues_no_llama_request() {
    let fx = fixture();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let fake = runtime.block_on(configured_install(&fx));

    let mut rpc = Rpc::spawn(&fx, &[]);
    // A settled prompt proves startup is over, so an absent request is not just a slow one.
    rpc.send(&json!({ "id": "1", "type": "prompt", "message": "hello" }));
    rpc.read_until("the run to settle", |event| {
        event["type"] == "agent_settled"
    });

    assert!(
        fake.requests().is_empty(),
        "an offline run issues no request to the llama.cpp server: {:?}",
        fake.requests()
    );
}
