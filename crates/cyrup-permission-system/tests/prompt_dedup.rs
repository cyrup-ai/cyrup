//! FULLY-WIRED REGRESSION PROOF that the prompt-dedup cache covers **every** ask surface, not just
//! the main check — pi keeps the cache inside `promptPermission` itself
//! (`pi-permission-system` v0.7.1 `src/index.ts:1798-1815` lookup, `:1890-1892` store), so all three
//! call sites that route through it are deduplicated identically:
//!
//! - skill-read (`index.ts:2282-2292`, `source: "skill_read"`),
//! - external-directory (`index.ts:2369-2378`, `source: "tool_call"`),
//! - the main `ask` check (`index.ts:2469`).
//!
//! A re-emitted IDENTICAL `tool_call` (same `toolCallId` + same fingerprint) must therefore render
//! ZERO additional prompts on ANY of them — upstream's own `tests/edit-decision-deduplication-red.
//! test.ts` is the regression proof for that invariant.
//!
//! BEFORE the fix, cyrup's cache lookup/store lived in `resolve_ask` (the main check) rather than in
//! `prompt_decision` (the `promptPermission` port), so `resolve_skill_read` and
//! `resolve_external_directory` called the prompting core directly and bypassed the cache entirely:
//! a re-emitted identical `tool_call` opened a SECOND modal dialog for the same skill-file read /
//! out-of-workdir path. Both tests below fail against that behavior with `2` prompts observed.
//!
//! Each test drives a real `PermissionSystemExtension` through the registered `before_tool_call` gate
//! (`NativeExtension::on_event(ToolCall)`) with a scripted [`AskChannel`] that COUNTS prompts — the
//! same seam `tests/forwarding_persist.rs` uses. "Allow Once" is deliberate: it persists nothing, so
//! the only thing that can collapse the second prompt is the dedup cache.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};

use cyrup_core::ToolCallId;
use cyrup_ext::{ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension};
use cyrup_permission_system::{
    AskChannel, AskOutcome, ExtensionConfig, ManagerPaths, PermissionDecisionState,
    PermissionPromptDecision, PermissionSystemExtension, PromptOpts,
};
use serde_json::{json, Value};

/// A scripted [`HostServices`] whose ONLY override is [`HostServices::all_tool_names`] — the full
/// registry the unknown-tool gate (pi `index.ts:2218-2228`) checks BEFORE any permission check.
/// Without it `read` reads as unregistered against the default empty registry and the gate blocks
/// before ever reaching the prompting core.
struct RegistryServices;
impl HostServices for RegistryServices {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["read".to_string()])
    }
}

/// A scripted channel answering "Allow Once" (pi `permission-dialog.ts` `APPROVE_ONCE_OPTION`) and
/// COUNTING how many dialogs it serviced. `Once` persists nothing — no session rule, no store
/// overlay — so the prompt count is a clean readout of the dedup cache alone.
struct CountingOnceChannel {
    prompts: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl AskChannel for CountingOnceChannel {
    async fn confirm(&self, _title: &str, _message: &str, _opts: PromptOpts) -> AskOutcome {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        AskOutcome::Decided(PermissionPromptDecision {
            approved: true,
            state: PermissionDecisionState::Once,
            denial_reason: None,
        })
    }
}

/// This binary's own env lock (PERM-020).
///
/// `crate::ext_config::env_lock()` is crate-private, so an INTEGRATION binary cannot reach it; this
/// is the same guarantee for this compilation unit. Every test body in this file takes it, so the
/// `set_var` in [`ensure_subagent_child`] can never be concurrent with another test's `getenv` —
/// and every test here does `getenv` constantly, because the gate reads `CYRUP_SUBAGENT_CHILD`
/// (`extension::is_subagent_child`), `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH`
/// (`ExtensionConfig::resolve_config_path`) and `CYRUP_PERMISSION_SYSTEM_LOGS_DIR`
/// (`logging::resolve_logs_dir`) on the decision path. In Rust 2024 an unsynchronized
/// `set_var`/`getenv` pair is undefined behaviour, not merely flaky ordering.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Drive `body` to completion with [`ENV_LOCK`] held for the whole test, recovering from poisoning
/// so one failing test does not cascade into spurious failures in its siblings.
///
/// The lock is taken in this SYNCHRONOUS frame, around `block_on`, rather than at the top of an
/// `async` test body. The span covered is identical — every `.await` in the body runs with the lock
/// held, which it must, because the gate calls `getenv` on the decision path. What changes is where
/// the `MutexGuard` lives. Inside an `async fn` it is captured in the generated future's state
/// (`clippy::await_holding_lock`): the guard then makes the future `!Send`, it is owned by whichever
/// thread last resumed the future rather than by one identifiable thread, and it is released only
/// when the runtime drops the future — including down cancellation paths no test here wrote. A
/// `std::sync::Mutex` is not reentrant and has no deadlock detection, so a guard whose release is
/// contingent on runtime bookkeeping is the shape that hangs a whole test binary instead of failing
/// it. On a stack frame the guard's lifetime is the frame's, and unwinding releases it.
///
/// `block_on` on a CURRENT-THREAD runtime is also what `#[tokio::test]` builds by default, so the
/// scheduling the bodies see (including `tokio::spawn` + `yield_now` in the PERM-014 test) is
/// unchanged.
fn with_env_lock<F: std::future::Future>(body: F) -> F::Output {
    let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(body)
}

/// `prompt_decision`'s fail-fast pre-check (pi `canRequestPermissionConfirmation`,
/// `index.ts:2263,2351,2452`) is `hasUI || isSubagent || yoloMode`, and the channel it then selects is
/// the injected `ask_channel` only when `has_ui` is false. Marking this process child-shaped is what
/// routes the prompt to [`CountingOnceChannel`] instead of a live `LocalAskChannel`. Set ONCE and
/// never unset: both tests in this binary need it and may run concurrently.
///
/// PERM-020: the caller MUST already be inside [`with_env_lock`]. The `Once` alone does not help — it makes
/// the write happen once, but says nothing about whether another test is reading the environment
/// while it happens.
fn ensure_subagent_child() {
    static SET: Once = Once::new();
    SET.call_once(|| {
        // SAFETY: serialized by `ENV_LOCK`, which the caller holds for the whole test body.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("CYRUP_SUBAGENT_CHILD", "1");
        }
    });
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// Build an installed extension over `global` policy with the counting channel injected. Returns the
/// extension and its prompt counter.
async fn ext_with_counting_channel(
    agent_dir: &Path,
    global: &str,
) -> (PermissionSystemExtension, Arc<AtomicUsize>) {
    let policy_path = agent_dir.join("cyrup-permissions.jsonc");
    write(&policy_path, global);
    let paths = ManagerPaths {
        global_config_path: policy_path,
        agents_dir: agent_dir.join("agents"),
        project_global_config_path: None,
        project_agents_dir: None,
        legacy_global_settings_path: agent_dir.join("settings.json"),
        global_mcp_config_path: agent_dir.join("mcp.json"),
        mcp_server_names_override: None,
    };
    let prompts = Arc::new(AtomicUsize::new(0));
    let ext = PermissionSystemExtension::from_parts(
        paths,
        ExtensionConfig::default(),
        Arc::new(CountingOnceChannel { prompts: Arc::clone(&prompts) }),
    );
    ext.set_host_services(Arc::new(RegistryServices));
    let mut api = InitApi::new();
    ext.init(&mut api).await.unwrap();
    (ext, prompts)
}

fn ctx(cwd: &Path) -> HostCtx {
    // A headless event-tier ctx — the exact shape the dispatcher hands `before_tool_call`.
    HostCtx::event(ExtMode::Print, false, cwd.to_path_buf())
}

/// The SAME `tool_call` twice means the SAME `call_id` — that is the whole point: pi's cache key is
/// `requestId (= toolCallId) \0 sha256(fingerprint)` (`index.ts:728-737`).
fn read_call(call_id: &str, path: &str) -> HostEvent {
    HostEvent::ToolCall {
        call_id: ToolCallId::from(call_id),
        name: "read".to_string(),
        input: json!({ "path": path }),
    }
}

/// RED BEFORE THE FIX — and red at COMPILE time, which is the strongest form available here.
///
/// `std::sync::MutexGuard` is `!Send`. While each test took `env_guard()` at the top of its `async`
/// body, the guard was captured in that body's future and every one of these futures was `!Send`,
/// so this assertion did not compile ("`MutexGuard<'_, ()>` cannot be sent between threads safely
/// ... required because it appears within the type of the future"). It compiles now precisely
/// because the guard lives on [`with_env_lock`]'s stack frame instead of inside the futures — which
/// is the whole content of the fix, not a proxy for it.
///
/// Constructing an `async fn`'s future runs none of its body, so this test opens no tempdir, writes
/// no policy file and touches no environment variable; it needs no lock of its own.
#[test]
fn env_locked_bodies_do_not_carry_the_guard_across_their_awaits() {
    fn assert_send<F: Send>(_: F) {}
    assert_send(reemitted_skill_read_reuses_the_cached_decision_with_no_second_prompt_body());
    assert_send(
        reemitted_external_directory_read_reuses_the_cached_decision_with_no_second_prompt_body(),
    );
    assert_send(two_concurrent_identical_asks_collapse_to_one_prompt_body());
}

// ================================================================================================
// (1) SKILL-READ ask surface (pi `index.ts:2282-2292`, `source: "skill_read"`).
// ================================================================================================

#[test]
fn reemitted_skill_read_reuses_the_cached_decision_with_no_second_prompt() {
    with_env_lock(reemitted_skill_read_reuses_the_cached_decision_with_no_second_prompt_body());
}

async fn reemitted_skill_read_reuses_the_cached_decision_with_no_second_prompt_body() {
    ensure_subagent_child();
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    // The `read` TOOL is denied, so a Noop can ONLY come from the skill-read bypass; the `deploy`
    // SKILL is `ask`, so that bypass has to prompt.
    let (ext, prompts) = ext_with_counting_channel(
        agent_dir,
        r#"{ "tools": { "read": "deny" }, "skills": { "deploy": "ask" } }"#,
    )
    .await;

    let cwd = agent_dir.to_path_buf();
    let skill_file = format!("{}/skills/deploy/SKILL.md", cwd.to_string_lossy());
    // before_agent_start caches the skill-enforcement entries the read gate resolves against
    // (pi `resolveSkillPromptEntries`, `index.ts:2175`).
    let system_prompt = format!(
        "<available_skills>\n  <skill>\n    <name>deploy</name>\n    <description>d</description>\n    <location>{skill_file}</location>\n  </skill>\n</available_skills>"
    );
    let _ = ext
        .on_event(
            &HostEvent::BeforeAgentStart {
                prompt: String::new(),
                images: Value::Null,
                system_prompt,
                options: Value::Null,
                injected: Vec::new(),
            },
            &ctx(&cwd),
        )
        .await;

    let call = read_call("call-skill-1", &skill_file);

    let first = ext.on_event(&call, &ctx(&cwd)).await;
    assert!(
        matches!(first, HookOutcome::Noop),
        "the human allowed the skill read once → it must proceed; got {first:?}"
    );
    assert_eq!(prompts.load(Ordering::SeqCst), 1, "the first skill-read ask surfaced one dialog");

    // The IDENTICAL tool_call, re-emitted. pi reuses the cached decision (collapsed to Allow-Once)
    // and renders ZERO additional prompts.
    let second = ext.on_event(&call, &ctx(&cwd)).await;
    assert!(
        matches!(second, HookOutcome::Noop),
        "the reused decision must still allow the read; got {second:?}"
    );
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        1,
        "a re-emitted IDENTICAL skill-read tool_call must reuse the cached decision, \
         never open a second dialog"
    );
}

// ================================================================================================
// (2) EXTERNAL-DIRECTORY ask surface (pi `index.ts:2369-2378`, `source: "tool_call"`).
// ================================================================================================

#[test]
fn reemitted_external_directory_read_reuses_the_cached_decision_with_no_second_prompt() {
    with_env_lock(
        reemitted_external_directory_read_reuses_the_cached_decision_with_no_second_prompt_body(),
    );
}

async fn reemitted_external_directory_read_reuses_the_cached_decision_with_no_second_prompt_body() {
    ensure_subagent_child();
    let cwd_dir = tempfile::tempdir().unwrap();
    let outside_dir = tempfile::tempdir().unwrap();
    let agent_dir = cwd_dir.path();
    // `read` itself is allowed, so the ONLY gate that prompts is the external-directory guard; an
    // approved-Once falls through to the (allowed) main check.
    let (ext, prompts) =
        ext_with_counting_channel(agent_dir, r#"{ "read": "allow", "external_directory": "ask" }"#)
            .await;

    let cwd = agent_dir.to_path_buf();
    let outside_path = outside_dir.path().join("secret.txt").to_string_lossy().into_owned();
    let call = read_call("call-ext-1", &outside_path);

    let first = ext.on_event(&call, &ctx(&cwd)).await;
    assert!(
        matches!(first, HookOutcome::Noop),
        "the human allowed the out-of-workdir read once → it must proceed; got {first:?}"
    );
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        1,
        "the first external-directory ask surfaced one dialog"
    );

    // The IDENTICAL tool_call, re-emitted. "Allow Once" persisted NOTHING (no session rule), so the
    // dedup cache is the only thing that can collapse this — exactly pi's behavior.
    let second = ext.on_event(&call, &ctx(&cwd)).await;
    assert!(
        matches!(second, HookOutcome::Noop),
        "the reused decision must still allow the read; got {second:?}"
    );
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        1,
        "a re-emitted IDENTICAL out-of-workdir tool_call must reuse the cached decision, \
         never open a second dialog"
    );
}

// ================================================================================================
// (3) PERM-014 — the CONCURRENT-duplicate window.
// ================================================================================================

/// A channel that BLOCKS inside `confirm` until the test opens the gate, so the second identical ask
/// arrives while the first prompt is genuinely still open — the window pi closes by caching the
/// unsettled `decisionPromise` (v0.8.0 `index.ts:1633`, run BEFORE the `await` at `:1637`).
///
/// The gate is a LEVEL-triggered `watch`, deliberately not a `Notify`. `Notify::notify_waiters()`
/// is edge-triggered and stores no permit, so any `confirm` that arrives after the release parks
/// forever — which converts the regression this test exists to catch (a SECOND dialog) from a loud
/// `prompts == 2` failure into an unbounded HANG. With a `watch`, a late `confirm` returns
/// immediately and the count assertion fails loudly instead.
struct GatedChannel {
    prompts: Arc<AtomicUsize>,
    gate: tokio::sync::watch::Receiver<bool>,
}

#[async_trait::async_trait]
impl AskChannel for GatedChannel {
    async fn confirm(&self, _title: &str, _message: &str, _opts: PromptOpts) -> AskOutcome {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        let mut gate = self.gate.clone();
        while !*gate.borrow_and_update() {
            if gate.changed().await.is_err() {
                break;
            }
        }
        AskOutcome::Decided(PermissionPromptDecision {
            approved: true,
            state: PermissionDecisionState::Once,
            denial_reason: None,
        })
    }
}

/// Drive every other task on this CURRENT-THREAD runtime until each has run to its next suspension
/// point. Deterministic by construction — a fixed number of polls, never a wall-clock sleep: the
/// only things any task in this test can park on are the [`GatedChannel`] gate and the dedup
/// cache's in-flight wait, so once a task has been polled it runs to one of those and stays there.
async fn settle() {
    for _ in 0..256 {
        tokio::task::yield_now().await;
    }
}

/// PERM-014 (RED before the fix). `prompt_decision` used `DedupCache::get`, which treats an
/// in-flight entry as a MISS, and it stored the decision only AFTER the human answered. So two
/// concurrently-executing tool calls with the same dedup key each opened their own dialog — the
/// operator answered the same question twice, and the two answers could disagree. Reachability is
/// not hypothetical: `cyrup-agent/src/loop_fn.rs` documents tool execution as "Default parallel".
///
/// pi's ordering is `rememberPermissionPromptDecision(..., decisionPromise)` (`index.ts:1633`) then
/// `await decisionPromise` (`:1637`), so the follower hits `getCachedPermissionPromptDecision`
/// (`:1581-1583`) and `await`s the SAME promise (`:1585`).
///
/// The target path is INSIDE `cwd` on purpose. A path outside it engages the external-directory
/// guard (`decide`, pi `index.ts:2310-2414`) BEFORE the main check, and the two guards ask two
/// DIFFERENT questions — different `details.message`, therefore different fingerprints and
/// different cache keys (`DedupDetails::cache_key`), so one gated call legitimately raises TWO
/// prompts, exactly as pi does. That is a second ask surface, not a dedup failure, and it has no
/// place in a test whose readout is a prompt COUNT. Keeping the read in-workdir isolates the single
/// surface under test (`read: ask` → the main check).
/// The runtime is CURRENT-THREAD, as it was under `#[tokio::test(flavor = "current_thread")]`:
/// [`settle`]'s determinism depends on there being exactly one worker, so a `spawn`ed task can only
/// run while this body is parked in `yield_now`. [`with_env_lock`] builds precisely that runtime.
#[test]
fn two_concurrent_identical_asks_collapse_to_one_prompt() {
    with_env_lock(two_concurrent_identical_asks_collapse_to_one_prompt_body());
}

async fn two_concurrent_identical_asks_collapse_to_one_prompt_body() {
    ensure_subagent_child();
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    let policy_path = agent_dir.join("cyrup-permissions.jsonc");
    write(&policy_path, r#"{ "tools": { "read": "ask" } }"#);
    let prompts = Arc::new(AtomicUsize::new(0));
    let (gate_tx, gate_rx) = tokio::sync::watch::channel(false);
    let ext = Arc::new(PermissionSystemExtension::from_parts(
        ManagerPaths {
            global_config_path: policy_path,
            agents_dir: agent_dir.join("agents"),
            project_global_config_path: None,
            project_agents_dir: None,
            legacy_global_settings_path: agent_dir.join("settings.json"),
            global_mcp_config_path: agent_dir.join("mcp.json"),
            mcp_server_names_override: None,
        },
        ExtensionConfig::default(),
        Arc::new(GatedChannel { prompts: Arc::clone(&prompts), gate: gate_rx }),
    ));
    ext.set_host_services(Arc::new(RegistryServices));
    let mut api = InitApi::new();
    ext.init(&mut api).await.unwrap();

    let cwd = agent_dir.to_path_buf();
    let target = cwd.join("a.txt").to_string_lossy().into_owned();
    let call = move || read_call("call-concurrent", &target);

    // LEADER: enters the gate and parks inside the dialog.
    let leader = {
        let ext = Arc::clone(&ext);
        let cwd = cwd.clone();
        let call = call.clone();
        tokio::spawn(async move { ext.on_event(&call(), &ctx(&cwd)).await })
    };
    // Run the leader up to its suspension point — inside the open dialog — so the follower cannot
    // simply lose the race. No timing involved: `settle` yields, it does not sleep.
    settle().await;
    assert_eq!(prompts.load(Ordering::SeqCst), 1, "the leader must have opened exactly one prompt");

    // FOLLOWER: the SAME call_id and the SAME input ⇒ the same dedup key, while the leader's
    // decision is still unsettled.
    let follower = {
        let ext = Arc::clone(&ext);
        let cwd = cwd.clone();
        let call = call.clone();
        tokio::spawn(async move { ext.on_event(&call(), &ctx(&cwd)).await })
    };
    // Run the follower to ITS suspension point. If dedup works that is the in-flight wait; if it
    // regressed it is a second dialog, which the gate lets it reach — so this is a real RED
    // assertion, not one that a slow machine can pass vacuously.
    settle().await;
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        1,
        "a concurrent identical ask must await the in-flight decision, not open a SECOND dialog"
    );

    // Open the gate; both callers must now see the same approval.
    gate_tx.send(true).unwrap();
    let leader = leader.await.unwrap();
    let follower = follower.await.unwrap();
    assert!(matches!(leader, HookOutcome::Noop), "the leader's approval lets the call proceed");
    assert!(
        matches!(follower, HookOutcome::Noop),
        "the follower must receive the LEADER's decision, collapsed to Allow-Once"
    );
    assert_eq!(prompts.load(Ordering::SeqCst), 1, "exactly one prompt, total");
}
