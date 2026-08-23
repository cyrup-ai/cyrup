//! `LiveHostServices` — the core capability grants: the live model/state reads, the control sink,
//! the `exec` and `proc.spawn` grants (including the fallback exec ceiling), the `ui.*`
//! request/reply round trip and its timeout semantics, plus the deny-all baseline.
//!
//! Relocated verbatim out of `host_services.rs`'s inline `mod tests` when that file became the
//! `src/host_services/` directory, so these run in the crate's ONE test binary like every other
//! module here. The banners that partitioned that module are now the four sibling files:
//! [`super::host_services_introspection`], [`super::host_services_session_view`],
//! [`super::host_services_oauth`] and [`super::host_services_custom_seam`] — which also reach for
//! this file's [`svc_with`] helper.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, ModelRef};
use cyrup_ext::host::{ControlOp, DialogOptions, HostServices, ProcSpawnSpec};
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use serde_json::{json, Value};

use crate::host_services::{LiveHostServices, UiKind, UiReply, UiRequest};

/// A backend seeded with the real local process ops + a temp cwd (the `exec` grant path). Shared
/// with the four sibling `host_services_*.rs` files, which were one `mod tests` with this one.
pub(super) fn svc_with(provider: Arc<dyn Provider>) -> LiveHostServices {
    LiveHostServices::new(provider, cyrup_tools::Backend::default().proc, std::env::temp_dir())
}

#[test]
fn reflects_live_model_and_models_catalog() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider.clone());

    // Before wiring: no current model, control denied, but the catalog is live from the provider.
    assert!(svc.current_model().is_none());
    assert!(svc.control(ControlOp::Reload).is_err());
    let models = svc.models();
    assert!(models.is_array(), "models() must serialize the provider catalog");
    assert!(!models.as_array().unwrap().is_empty(), "faux provider has at least one model");

    // After the session pushes its active model, the read reflects it.
    let m = ModelRef { provider: "faux".into(), api: None, model: "faux-1".into() };
    svc.update_model(m, 128_000, Some("medium".into()));
    svc.update_state(Some("my session".into()), 42);
    assert_eq!(svc.current_model().as_deref(), Some("faux/faux-1"));
    assert_eq!(svc.thinking_level().as_deref(), Some("medium"));
    assert_eq!(svc.session_name().as_deref(), Some("my session"));
    let usage = svc.context_usage();
    assert_eq!(usage["usedTokens"], json!(42));
    assert_eq!(usage["contextWindow"], json!(128_000));
}

#[test]
fn control_routes_to_the_wired_sink() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    svc.set_control_sink(Arc::new(move |_op| {
        h.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    svc.control(ControlOp::Reload).expect("control routes to the sink");
    svc.control(ControlOp::Compact { custom_instructions: None }).expect("control routes to the sink");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

/// The `exec` grant runs a DIRECT argv (shell:false) command and returns the REAL captured
/// output/code/killed — 1:1 with Pi `execCommand` (exec.ts:34-46). Multi-thread runtime so the
/// sync grant can `block_in_place` on the async process ops.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_runs_argv_with_cwd_env_and_reports_killed_on_timeout() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);

    // 1) Real stdout + exit code, NO shell (argv `echo hi`).
    let out = svc
        .exec("echo", &["hi".to_string()], &json!({}), CancelToken::new())
        .expect("echo runs via the exec grant");
    assert_eq!(out.stdout, "hi\n");
    assert_eq!(out.code, 0);
    assert!(!out.killed, "a natural exit is not `killed`");

    // 2) shell:false — an argv that a shell would splice is passed literally, so `echo` prints the
    //    metacharacters verbatim (proves no `bash -c` word-splitting).
    let out = svc
        .exec("echo", &["a; echo b".to_string()], &json!({}), CancelToken::new())
        .expect("echo runs");
    assert_eq!(out.stdout, "a; echo b\n", "argv is literal — no shell interpretation");

    // 3) `cwd` option honored (Pi `opts?.cwd ?? cwd`).
    let tmp = std::env::temp_dir();
    let out = svc
        .exec("pwd", &[], &json!({ "cwd": tmp.to_string_lossy() }), CancelToken::new())
        .expect("pwd runs");
    let printed = std::fs::canonicalize(out.stdout.trim_end()).unwrap_or_default();
    assert_eq!(printed, std::fs::canonicalize(&tmp).unwrap_or(tmp), "exec ran in the given cwd");

    // 4) a guest-supplied `env` key is IGNORED — Pi's real `execCommand` (exec.ts:41-45) never
    //    accepts an env override at all; the child only inherits the host's own ambient
    //    environment (Node `spawn()`'s default when no `env` key is passed). If the `exec` grant
    //    honored a guest's `env`, `printenv` would see the injected value; instead the lookup
    //    variable must be genuinely UNSET in the child (nonzero exit, empty stdout) — proving
    //    this is NOT new ambient authority beyond Pi's real surface.
    let out = svc
        .exec(
            "printenv",
            &["CYRUP_EXEC_TEST_ENV_MUST_BE_IGNORED".to_string()],
            &json!({ "env": { "CYRUP_EXEC_TEST_ENV_MUST_BE_IGNORED": "injected" } }),
            CancelToken::new(),
        )
        .expect("printenv runs (even though the variable it looks up is unset)");
    assert_ne!(
        out.code, 0,
        "a guest-supplied `env` override must be ignored — printenv must NOT find an injected \
         value"
    );
    assert!(out.stdout.is_empty(), "no injected value may ever reach the child's environment");

    // 5) a timeout ⇒ the host SIGTERMs the group, then (since `sleep` obeys SIGTERM and dies
    //    well within the 5s grace period, no SIGKILL escalation needed here) reports
    //    `killed=true` (Pi `killProcess` sets `killed`, exec.ts:52-63). Asserted under BOTH
    //    spellings: pi's real key is `timeout` (`ExecOptions.timeout?: number`, `core/exec.ts:15`
    //    @v0.83.0) — the host used to accept ONLY cyrup's SDK spelling `timeoutMs`, so a bag
    //    written by anything else was silently ignored and fell through to the 120s ceiling.
    for key in ["timeout", "timeoutMs"] {
        let opts =
            Value::Object(serde_json::Map::from_iter([(key.to_string(), json!(100))]));
        let out = svc
            .exec("sleep", &["30".to_string()], &opts, CancelToken::new())
            .expect("sleep runs then is killed on timeout");
        assert!(out.killed, "a timed-out exec is `killed` under the `{key}` key");
    }

    // 6) an already-aborted signal (pre-cancelled token) kills immediately ⇒ `killed=true`.
    let cancelled = CancelToken::new();
    cancelled.cancel();
    let out = svc
        .exec("sleep", &["30".to_string()], &json!({}), cancelled)
        .expect("a pre-cancelled exec resolves");
    assert!(out.killed, "a pre-aborted signal kills the exec");

    // 7) a well-behaved child that TRAPS SIGTERM and exits itself with its OWN real code must
    //    have that REAL code surfaced through the grant end-to-end — `killed` is orthogonal,
    //    never masking it — 1:1 with Pi's `{code, killed}` (`exec.ts:97`; `child-process.ts:73-
    //    80`'s `finalize(exitCode)` always carries the real observed code).
    let out = svc
        .exec(
            "sh",
            &["-c".to_string(), "trap 'exit 7' TERM; while true; do sleep 1; done".to_string()],
            &json!({ "timeoutMs": 100 }),
            CancelToken::new(),
        )
        .expect("the SIGTERM-trapping child runs then exits itself");
    assert_eq!(out.code, 7, "the child's own real exit code survives a host-initiated kill");
    assert!(out.killed, "a timeout-initiated kill is still `killed`, independent of `code`");
}

/// L4 round-12 finding #3: `exec`'s `cwd` option must treat a guest-supplied EMPTY string the
/// same as an OMITTED one — falling back to the session cwd — not short-circuit
/// `unwrap_or_else` with an empty override. Pi's real `ctx.exec` (`loader.ts:319`:
/// `options?.cwd ?? cwd`) only falls back via `??` on `undefined`/`null`; a literal `""` stays
/// `""` all the way to Node's `child_process.spawn({cwd:""})`, which (verified live) treats a
/// FALSY cwd as "no override" and inherits the parent's ambient cwd rather than erroring —
/// `self.cwd` (the session's project directory) is the cyrup-analog of that ambient fallback.
/// Verified by actually running `pwd` inside the spawned child and reading its REAL stdout +
/// exit code (pre-fix: `std::process::Command::current_dir("")` hard-fails the spawn, which
/// `exec`'s `Err(_) => Ok(ExecOutput{code:1,..})` mapping — Pi's `execCommand` never rejects,
/// exec.ts:99-105 — turned into a SILENT `code:1`/empty-stdout failure instead of running in the
/// session cwd).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_treats_an_empty_guest_cwd_the_same_as_omitted() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session_cwd = std::env::temp_dir();
    let svc = svc_with(provider);

    let out = svc
        .exec("pwd", &[], &json!({ "cwd": "" }), CancelToken::new())
        .expect("pwd runs even though the guest passed an empty cwd");
    assert_eq!(out.code, 0, "must NOT silently degrade to code:1 the way a hard current_dir(\"\") spawn failure would");
    let printed = std::fs::canonicalize(out.stdout.trim_end()).unwrap_or_default();
    assert_eq!(
        printed,
        std::fs::canonicalize(&session_cwd).unwrap_or(session_cwd),
        "an empty guest cwd must fall back to the SESSION's cwd, exactly like an omitted one"
    );
}

/// L4 review: `exec` must never be truly UNBOUNDED when the guest gives no `timeoutMs` (or `0`) —
/// unlike Pi's own `execCommand` (exec.ts:74-79), which is also unbounded absent a `timeout` but
/// can still be interrupted live via a real `AbortSignal` listener (exec.ts:65-72), cyrup's `exec`
/// grant blocks the guest wasm-suspended for the ENTIRE synchronous host call — a `signalId` can
/// only pre-cancel an ALREADY-aborted signal at call entry, never mid-run — so an untimed call has
/// no live escape hatch at all without `DEFAULT_EXEC_TIMEOUT`. Proven with a REAL never-exiting
/// child (`sleep 3600`) and NO `timeoutMs` in `opts` at all, against a tiny overridden fallback
/// (`with_exec_timeout`) so the test doesn't wait the full 120s production ceiling: the exec call
/// must still return promptly, `killed`, via the SAME SIGTERM-then-grace-then-SIGKILL escalation
/// an explicit guest timeout gets (proving the fallback is fed into `exec_argv`'s real `timeout`
/// parameter, not merely abandoning/leaking the child via an outer future-drop).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_with_no_timeout_ms_still_gets_killed_by_the_fallback_ceiling() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = LiveHostServices::with_exec_timeout(
        provider,
        cyrup_tools::Backend::default().proc,
        std::env::temp_dir(),
        Duration::from_millis(100),
    );

    let started = std::time::Instant::now();
    let out = svc
        .exec("sleep", &["3600".to_string()], &json!({}), CancelToken::new())
        .expect("exec resolves even though the guest gave no timeoutMs at all");
    let elapsed = started.elapsed();

    assert!(out.killed, "the fallback ceiling must kill an untimed exec — a 3600s sleep can never exit on its own");
    assert!(
        elapsed < Duration::from_secs(10),
        "must be bounded by the (overridden, 100ms) fallback ceiling, not the real 3600s sleep — \
         took {elapsed:?}"
    );
}

/// The `proc` grant's `spawn` defaults an OMITTED `cwd` to the session's own project directory —
/// the SAME fallback `exec` applies (test 3 above, `opts.cwd ?? self.cwd`) — rather than
/// silently inheriting the HOST PROCESS's own ambient cwd (`tokio::process::Command`'s default
/// when no `.current_dir()` call is made at all). Verified by actually running `pwd` inside the
/// spawned child and reading its REAL stdout, not asserting on `ProcSpawnSpec` construction.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proc_spawn_defaults_omitted_cwd_to_the_session_cwd() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);
    let session_cwd = std::env::temp_dir();

    // No `cwd` in the spec at all — must run in the SESSION's cwd, not the host's ambient one
    // (this test binary's own cwd is the crate root, which must NOT be what `pwd` prints).
    let spec = ProcSpawnSpec {
        cmd: "pwd".to_string(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: None,
        capture_stderr: false,
    };
    let handle = svc.proc_spawn(&spec).expect("pwd spawns with no cwd override");
    let stdout = wait_for_exit_and_read_stdout(&svc, handle).await;
    let printed = std::fs::canonicalize(stdout.trim_end()).unwrap_or_default();
    assert_eq!(
        printed,
        std::fs::canonicalize(&session_cwd).unwrap_or(session_cwd),
        "an omitted cwd must default to the SESSION's cwd, not the host process's ambient one"
    );

    // An EXPLICIT `cwd` in the spec is still honored verbatim (the fallback only fires when
    // `cwd` is `None`, never overriding a guest-supplied value).
    let explicit = std::env::current_dir().expect("host has a cwd");
    let spec = ProcSpawnSpec {
        cmd: "pwd".to_string(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: Some(explicit.clone()),
        capture_stderr: false,
    };
    let handle = svc.proc_spawn(&spec).expect("pwd spawns with an explicit cwd");
    let stdout = wait_for_exit_and_read_stdout(&svc, handle).await;
    let printed = std::fs::canonicalize(stdout.trim_end()).unwrap_or_default();
    assert_eq!(
        printed,
        std::fs::canonicalize(&explicit).unwrap_or(explicit),
        "an explicit cwd is honored verbatim, not overridden by the session-cwd fallback"
    );
}

/// Regression test: the session's OWN host-injected default `cwd` (the fallback `proc_spawn`
/// applies when a guest omits `cwd` entirely, above) must reach the real child VERBATIM, never
/// re-interpolated — even when that literal project-directory path happens to contain a
/// `${VAR}`-shaped substring. Before the fix (`caps/proc.rs`'s `ProcCaps::spawn` used to run
/// EVERY `Some(cwd)` — guest-supplied or host-injected — through `resolve_config_path`), this
/// spawn call failed outright with ENOENT: interpolating the unset `${MY_REPRO_VAR}` down to an
/// empty string produced a directory that doesn't exist on disk (only the literal
/// `${MY_REPRO_VAR}`-named one, created below, does). Verified live: actually spawns `pwd`
/// through the real `proc_spawn` grant and reads its real stdout.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proc_spawn_never_reinterpolates_the_host_injected_default_cwd() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let base = std::env::temp_dir();
    let weird = base.join("cyrup-session-cwd-${MY_REPRO_VAR}-dir");
    std::fs::create_dir_all(&weird).expect("create the literal, unusual session cwd");
    let svc = LiveHostServices::new(provider, cyrup_tools::Backend::default().proc, weird.clone());

    let spec = ProcSpawnSpec {
        cmd: "pwd".to_string(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: None,
        capture_stderr: false,
    };
    let handle = svc
        .proc_spawn(&spec)
        .expect("pwd must spawn successfully in the session's literal cwd, not ENOENT");
    let stdout = wait_for_exit_and_read_stdout(&svc, handle).await;
    assert_eq!(
        std::fs::canonicalize(stdout.trim_end()).unwrap_or_default(),
        std::fs::canonicalize(&weird).unwrap_or(weird),
        "the host-injected default cwd must survive byte-for-byte, not have ${{MY_REPRO_VAR}} \
         interpolated out of it"
    );
}

/// Poll `proc_poll_exit` until the child reaps, then drain its real stdout — used by tests that
/// need a spawned child's actual captured output rather than just an `Ok` handle.
#[cfg(unix)]
async fn wait_for_exit_and_read_stdout(svc: &LiveHostServices, handle: u32) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if svc.proc_poll_exit(handle).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let bytes = svc.proc_read_stdout(handle, 65536).expect("read_stdout on a live handle");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// With NO ui sink attached (headless print/json: `set_ui_sink` is never called), the ui grant
/// falls through to the trait deny defaults WITHOUT blocking — byte-for-byte Pi `noOpUIContext`
/// (confirm=false, input/select/editor=None). A single-thread runtime proves it never touches
/// `block_in_place` (which would panic here) on the headless path.
#[test]
fn headless_ui_returns_deny_defaults_without_a_sink() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);
    assert!(!svc.confirm("ok?", "body", &DialogOptions::default()));
    assert_eq!(svc.input("name?", Some("placeholder"), &DialogOptions::default()), None);
    assert_eq!(svc.select("pick", &json!(["a", "b"]), &DialogOptions::default()), None);
    assert_eq!(svc.editor("title", "seed"), None);
}

/// The ui GRANT round-trips a dialog through a scripted [`crate::host_services::UiSink`] renderer:
/// the guest-facing (sync) `confirm`/`input`/`select`/`editor` block on a one-shot while a
/// concurrent responder answers each [`UiRequest`], exactly as the interactive TUI selector / RPC
/// round-trip does at runtime. Multi-thread so the `block_in_place` + `block_on` reply-wait is
/// legal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_grant_round_trips_through_a_scripted_sink() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = Arc::new(svc_with(provider));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    svc.set_ui_sink(tx);

    // L4 review §2.6/§2.7 live proof: capture each request's `message`/`placeholder` as the
    // scripted renderer sees them, so the test can assert they arrived distinct from `prompt`.
    #[derive(Clone, Debug)]
    struct Seen {
        kind: UiKind,
        prompt: String,
        message: String,
        placeholder: Option<String>,
    }
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    // The scripted renderer: reply to each request by kind (like a user picking in the selector).
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            crate::sync::lock(&seen2).push(Seen {
                kind: req.kind,
                prompt: req.prompt.clone(),
                message: req.message.clone(),
                placeholder: req.placeholder.clone(),
            });
            let reply = match req.kind {
                UiKind::Confirm => UiReply::Confirm(true),
                UiKind::Input => UiReply::Text(Some(format!("answer:{}", req.prompt))),
                UiKind::Select => {
                    // Echo back the LAST option string as the chosen value proof.
                    let chosen = req
                        .options
                        .as_array()
                        .and_then(|a| a.last())
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    UiReply::Text(chosen)
                }
                // Echo the seed text (Pi `prefill`, now `req.message` — L4 review §2, editor
                // title fix): proves `editor`'s two strings arrive on distinct fields, not both
                // squashed onto `prompt`.
                UiKind::Editor => UiReply::Text(Some(format!("edited:{}", req.message))),
            };
            let _ = req.reply.send(reply);
        }
    });

    // Each guest-facing call blocks until the responder answers (run on a blocking-capable worker).
    let s1 = svc.clone();
    let confirm = tokio::task::spawn_blocking(move || {
        s1.confirm("proceed?", "a large formatted body, distinct from the title", &DialogOptions::default())
    })
    .await
    .expect("confirm task");
    assert!(confirm, "confirm round-trips the scripted `true`");

    let s2 = svc.clone();
    let input = tokio::task::spawn_blocking(move || {
        s2.input("name?", Some("e.g. Ada Lovelace"), &DialogOptions::default())
    })
    .await
    .expect("input task");
    assert_eq!(input.as_deref(), Some("answer:name?"));

    // §2.6: the confirm `message` reached the renderer verbatim, distinct from `prompt` (title).
    // §2.7: the input `placeholder` reached the renderer verbatim (`Some`, not dropped).
    let seen_snapshot = crate::sync::lock(&seen).clone();
    assert_eq!(
        seen_snapshot
            .iter()
            .find(|s| s.kind == UiKind::Confirm)
            .map(|s| (s.prompt.as_str(), s.message.as_str())),
        Some(("proceed?", "a large formatted body, distinct from the title")),
        "confirm's message body round-trips separately from its title: {seen_snapshot:?}"
    );
    assert_eq!(
        seen_snapshot.iter().find(|s| s.kind == UiKind::Input).map(|s| s.placeholder.clone()),
        Some(Some("e.g. Ada Lovelace".to_string())),
        "input's placeholder round-trips instead of being dropped: {seen_snapshot:?}"
    );

    let s3 = svc.clone();
    let select = tokio::task::spawn_blocking(move || {
        s3.select("pick one", &json!(["x", "y", "z"]), &DialogOptions::default())
    })
    .await
    .expect("select task");
    assert_eq!(
        select.as_deref(),
        Some("z"),
        "select returns the chosen option STRING (Pi types.ts:127, world.wit:259)"
    );

    let s4 = svc.clone();
    let editor = tokio::task::spawn_blocking(move || s4.editor("edit this file", "hello"))
        .await
        .expect("editor task");
    assert_eq!(editor.as_deref(), Some("edited:hello"));

    // L4 review §2 (editor title fix) live proof: `editor`'s title reached the renderer on
    // `prompt`, distinct from its seed text on `message` — mirrors the confirm/input assertions
    // above, closing the same class of dropped-field bug for `editor`.
    let seen = crate::sync::lock(&seen).clone();
    assert_eq!(
        seen.iter()
            .find(|s| s.kind == UiKind::Editor)
            .map(|s| (s.prompt.as_str(), s.message.as_str())),
        Some(("edit this file", "hello")),
        "editor's title round-trips separately from its seed text: {seen:?}"
    );
}

/// L4 review §2.2: a dialog whose renderer NEVER answers still resolves within `opts.timeout_ms` —
/// Pi's `createDialogPromise` host-armed `setTimeout(() => resolve(defaultValue), opts.timeout)`
/// (`rpc-mode.ts:114-119`) ALWAYS settles the awaited Promise regardless of client behavior. The
/// scripted renderer here receives every request and drops it on the floor (never replies), proving
/// `ui_roundtrip` races the reply against a REAL timer rather than blocking forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_grant_honors_timeout_ms_and_resolves_to_the_default_on_no_response() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = Arc::new(svc_with(provider));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    svc.set_ui_sink(tx);
    // The "hung client": receives every request and HOLDS it (keeping `req.reply` open, exactly
    // like the RPC loop's `pending` map keeps a live entry) but never sends a reply — the real
    // shape of a non-responding client, as opposed to a dropped sender (which would resolve the
    // receiver immediately with an error and prove nothing about the timeout race).
    let held: Arc<Mutex<Vec<UiRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let held2 = held.clone();
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            crate::sync::lock(&held2).push(req);
        }
    });

    let opts = DialogOptions { timeout_ms: Some(50), signal_id: None };

    let s1 = svc.clone();
    let o1 = opts.clone();
    let started = tokio::time::Instant::now();
    let confirm = tokio::task::spawn_blocking(move || s1.confirm("proceed?", "body", &o1))
        .await
        .expect("confirm task");
    let elapsed = started.elapsed();
    assert!(!confirm, "an unanswered confirm resolves to Pi's `false` default, not a hang");
    assert!(
        elapsed < Duration::from_secs(2),
        "confirm must settle close to the 50ms timeout, not hang indefinitely (took {elapsed:?})"
    );

    let s2 = svc.clone();
    let o2 = opts.clone();
    let input = tokio::task::spawn_blocking(move || s2.input("name?", Some("placeholder"), &o2))
        .await
        .expect("input task");
    assert_eq!(input, None, "an unanswered input resolves to Pi's `undefined` default");

    let s3 = svc.clone();
    let o3 = opts;
    let select = tokio::task::spawn_blocking(move || s3.select("pick", &json!(["a", "b"]), &o3))
        .await
        .expect("select task");
    assert_eq!(select, None, "an unanswered select resolves to Pi's `undefined` default");
}

/// `timeout_ms: 0` means NO timeout, not an instant one — Pi's `createDialogPromise` only arms
/// its `setTimeout` `if (opts?.timeout)` (`rpc-mode.ts:114`; falsy-zero ⇒ no timer at all). Proven
/// here the same way the honors-timeout test proves the OPPOSITE: a REAL (delayed, non-default)
/// reply arrives well after `Duration::from_millis(0)` would already have elapsed under the old
/// unconditional `.map(Duration::from_millis)` — if `0` were mistakenly armed as a real timer, the
/// race would resolve to the default (`false`) near-instantly and NEVER see this later reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_grant_timeout_ms_zero_means_no_timeout_not_an_instant_one() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = Arc::new(svc_with(provider));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    svc.set_ui_sink(tx);
    tokio::spawn(async move {
        if let Some(req) = rx.recv().await {
            // A REAL answer, deliberately delayed well past when a (bugged) 0ms timer would have
            // already fired and resolved the call to the default.
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = req.reply.send(UiReply::Confirm(true));
        }
    });

    let opts = DialogOptions { timeout_ms: Some(0), signal_id: None };
    let started = tokio::time::Instant::now();
    let confirm = tokio::task::spawn_blocking(move || svc.confirm("proceed?", "body", &opts))
        .await
        .expect("confirm task");
    let elapsed = started.elapsed();

    assert!(
        confirm,
        "timeout_ms:0 must wait for the REAL reply (true), not short-circuit to the `false` \
         default the way a genuine 0ms timeout would"
    );
    assert!(
        elapsed >= Duration::from_millis(120),
        "the call must have actually WAITED for the delayed reply, not resolved near-instantly \
         to the default (took {elapsed:?}, expected >= ~150ms)"
    );
}

/// L4 review §2.5 (the shared mechanism half): a reply sent on the SAME one-shot `ui_roundtrip` is
/// waiting on unblocks it immediately, well before a long `timeout_ms` would otherwise elapse. This
/// is exactly what the RPC loop's `force_resolve_pending` (`rpc.rs`, wired to `abort`/`abort_retry`)
/// does to LIVE-dismiss an already-open dialog — no separate cancellation channel is needed because
/// forcing the existing reply is sufficient, and this proves that path is genuinely live, not merely
/// a pre-flight snapshot check.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_grant_force_resolved_reply_unblocks_before_a_long_timeout_elapses() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = Arc::new(svc_with(provider));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    svc.set_ui_sink(tx);
    // Simulate a live "abort": as soon as the dialog opens, force-resolve it directly (the same
    // action `force_resolve_pending` takes) instead of waiting for a real user response.
    tokio::spawn(async move {
        if let Some(req) = rx.recv().await {
            let _ = req.reply.send(UiReply::Confirm(false));
        }
    });

    // A 10-second timeout that must NOT be what unblocks this call.
    let opts = DialogOptions { timeout_ms: Some(10_000), signal_id: None };
    let started = tokio::time::Instant::now();
    let confirm = tokio::task::spawn_blocking(move || svc.confirm("proceed?", "body", &opts))
        .await
        .expect("confirm task");
    let elapsed = started.elapsed();
    assert!(!confirm);
    assert!(
        elapsed < Duration::from_secs(2),
        "a force-resolved reply must win the race immediately, not wait out the 10s timeout (took {elapsed:?})"
    );
}

/// The DEFAULT (deny-all) backend denies exec with Pi's "not granted" message — the untrusted
/// analog (an untrusted extension gets `DenyServices`, arch-08 §5.6).
#[test]
fn deny_services_refuses_exec() {
    use cyrup_ext::host::{DenyServices, HostServices as _};
    let err = DenyServices
        .exec("echo", &["hi".to_string()], &json!({}), CancelToken::new())
        .expect_err("deny-all backend refuses exec");
    assert!(err.contains("not granted"), "denied with the Pi message: {err}");
}

