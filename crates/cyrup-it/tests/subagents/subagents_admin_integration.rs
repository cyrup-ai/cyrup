//! VL-S11a — `/subagents`, the admin surface, driven through the REAL
//! `SubagentsExtension::execute_command` (the `cyrup_ext::native::NativeExtension` trait method a
//! user typing `/subagents …` reaches).
//!
//! Upstream is `openSubagentsAdmin` (pi `slash/subagents-admin.ts:396-460` @v0.68.0). Four of its
//! behaviours are only observable from OUTSIDE the module, which is why they are asserted here
//! rather than in the crate's own `#[cfg(test)]` modules:
//!
//! * **§I.6** — the no-UI branch (`:412-415`) emits the full metadata block and returns without
//!   ever reaching the action selector. A stub handler returning `Ok(String::new())` satisfies
//!   every in-crate unit test of the renderer and fails here.
//! * **§I.7** — a builtin `model` override is a WRITE to `settings.json`, and the only honest
//!   proof that it landed in the right scope's file is to read the file AND re-run discovery.
//! * **§I.9 and the RUNTIME-registered case** — the two READ-ONLY refusals that are actually
//!   reachable on the production path. Both depend on discovery state (an env var, an in-process
//!   registration) that no unit test over the message builder can establish, and both go through
//!   `read_only_agent_message`'s caller, which is only consulted after `saves_through_settings`
//!   answered `false`.
//!
//! # §I.8 is deliberately absent
//!
//! The batch spec asks for an IT proving a PACKAGE-sourced agent refuses with
//! `Cannot update '<n>' model because that field is owned by its read-only package definition.`
//! **That branch is unreachable on the production path at v0.68.0 and this file does not assert
//! it.** `savesThroughSettings` returns `true` for every package agent (`subagents-admin.ts:127`),
//! so `saveAgentModel` (`:308`), `saveAgentThinking` (`:331`), `saveAgentSystemPrompt` (`:364`) and
//! `editSystemPrompt` (`:382`) all take the settings-override branch and never consult
//! `readOnlyAgentMessage` (`:154-156`) for a package agent. That is upstream's design, not a
//! porting gap: a package FILE is read-only, a settings override ON that agent is not. An IT
//! asserting the sentence would therefore either fail against a faithful port or pass only against
//! a stub — the worst of the two outcomes. The sentence's BYTES are pinned where they belong, in
//! `registration/subagents_admin.rs`'s `read_only_agent_message_sentences`.
//!
//! # The metadata block's real shape
//!
//! `metadataFor` (`subagents-admin.ts:185-214`) is ported as `subagents_admin::metadata_for`,
//! which routes through the public `handle_management_action(cfg, "get", …)` — whose single-agent
//! answer is exactly `discovery::management::render::format_agent_detail`
//! (`discovery/management/render.rs:39`), since `handle_get` joins one block
//! (`handlers.rs:386-388`). That renderer's FIRST line is
//! `format!("Agent: {} ({})", a.name, source_str(a.source))` with `source_str(Builtin) ==
//! "builtin"` (`discovery/management/helpers.rs:96`), so the identity line for the bundled `scout`
//! persona is exactly `Agent: scout (builtin)` — read off the renderer, not assumed — and the
//! `System prompt mode:` line is `render.rs:76-83`, which for `scout` reads `replace` because
//! `resources/agents/scout.md` declares `systemPromptMode: replace`.
//!
//! Two differences from upstream's `metadataFor` are asserted AROUND rather than against:
//! `format_agent_detail` emits no `Override: <scope> (<path>)` line (upstream `:211`), and it OMITS
//! the `Model:` line when an agent declares no model where upstream prints `default / inherit`.
//! The second is what makes §I.7's re-read assertion sharp: the line does not exist before the
//! override and does exist after it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use cyrup_ext::host::{DialogOptions, HostServices, NotifyKind};
use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

/// The bundled builtin every case below administers. Declared in
/// `crates/cyrup-ext-subagents/resources/agents/scout.md` with no `model:` key, which is the
/// precondition §I.7's before/after assertion rests on.
const BUILTIN: &str = "scout";

// =================================================================================================
// Harness
// =================================================================================================

fn ui_ctx(cwd: &Path) -> HostCtx {
    HostCtx::command(ExtMode::Tui, true, cwd.to_path_buf())
}

fn headless_ctx(cwd: &Path) -> HostCtx {
    HostCtx::command(ExtMode::Tui, false, cwd.to_path_buf())
}

fn extension(home: &Path, cwd: &Path) -> SubagentsExtension {
    SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: Roots::sandboxed(home),
            ..SubagentExtensionConfig::default()
        },
        cwd.to_path_buf(),
    )
}

/// `<home>/.cyrup/agents/settings.json` — the USER-scope settings file
/// `SubagentExecutor::discovery_config_on_disk`
/// (`extension/executor/resolve.rs:138-143`) resolves for a sandboxed root, and therefore the one
/// file a `"user"` scope pick must land in. Named here rather than globbed, because "some
/// settings.json somewhere gained the key" is exactly the weaker assertion §I.7 exists to refuse.
fn user_settings_path(home: &Path) -> std::path::PathBuf {
    home.join(".cyrup").join("agents").join("settings.json")
}

/// The `subagents.agentOverrides.<agent>.model` value in `path`, or `None` when any level of that
/// chain is absent. pi `mergeBuiltinAgentOverride`'s written shape (`agents.ts:1301-1327`), ported
/// at `discovery/settings_write.rs:158`.
fn override_model(path: &Path, agent: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("subagents")?
        .get("agentOverrides")?
        .get(agent)?
        .get("model")?
        .as_str()
        .map(str::to_string)
}

/// A real, registry-backed `provider/id` — pi's `modelFullId` (`subagents-admin.ts:71-73`), which
/// is the exact string `chooseModel`'s selector items carry and therefore the exact string a
/// scripted pick must return. Taken from the live builtin catalog rather than hard-coded so the
/// pick can never be a value the model registry would reject as unknown.
fn a_real_model_id() -> String {
    let entry = cyrup_provider::catalog::builtin_catalog()
        .iter()
        .find(|m| m.provider.as_str() == "anthropic")
        .expect("the builtin catalog ships at least one anthropic model");
    format!("{}/{}", entry.provider.as_str(), entry.id.as_str())
}

/// A `HostServices` whose `select`/`custom` answer from a fixed script, recording every prompt.
///
/// This is `ctx.ui.select` — `selectFromList`'s flat-title branch (`subagents-admin.ts:216-228`),
/// which the port deliberately takes instead of `ctx.ui.custom` so exactly this kind of script can
/// drive it. `custom` is scripted from the same queue anyway: it costs one method and it means a
/// port that later routes the picker through the overlay door fails on the ASSERTION rather than
/// on an unanswered prompt.
///
/// The queue is answered in order and an exhausted queue answers `None`, which every caller reads
/// as upstream's `undefined` — a dismissal. `choose_override_scope` can short-circuit to `user`
/// without asking (`:798-800`, when there is no project settings path); the unconsumed `"user"`
/// entry is then simply never read.
#[derive(Default)]
struct ScriptedSelect {
    answers: Mutex<std::collections::VecDeque<String>>,
    prompts: Mutex<Vec<String>>,
    notifications: Mutex<Vec<String>>,
}

impl ScriptedSelect {
    fn new(answers: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.iter().map(|a| (*a).to_string()).collect()),
            prompts: Mutex::new(Vec::new()),
            notifications: Mutex::new(Vec::new()),
        })
    }

    fn next_answer(&self, prompt: String) -> Option<String> {
        self.prompts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(prompt);
        self.answers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    /// Everything the command said to the human, for a failure message that can distinguish "the
    /// handler refused" from "the handler never ran".
    fn transcript(&self) -> String {
        let prompts = self
            .prompts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let notices = self
            .notifications
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        format!("prompts: {prompts:#?}\nnotifications: {notices:#?}")
    }
}

impl HostServices for ScriptedSelect {
    fn select(
        &self,
        prompt: &str,
        _options: &serde_json::Value,
        _opts: &DialogOptions,
    ) -> Option<String> {
        self.next_answer(prompt.to_string())
    }

    fn custom(&self, spec: &serde_json::Value) -> Option<String> {
        self.next_answer(spec.to_string())
    }

    fn notify(&self, message: &str, _kind: NotifyKind) {
        self.notifications
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message.to_string());
    }
}

/// Run `/subagents <args>` and return whatever the handler rendered.
///
/// `execute_command` wraps a `SubagentError` as rendered text rather than an `ExtError` (the
/// convention every sibling command in `registration_commands_integration.rs` relies on), so the
/// refusal cases below read the SENTENCE out of the `Ok` arm — an `ExtError` here would mean the
/// command is not dispatched at all, which is a different failure and gets its own message.
async fn subagents(ext: &SubagentsExtension, args: &str, ctx: &HostCtx) -> String {
    ext.execute_command("subagents", args, ctx)
        .await
        .expect(
            "`/subagents` must be a DISPATCHED command: an ExtError here means no \
             `SlashCommandName::Subagents` arm exists in `dispatch_slash`",
        )
        .expect("the `/subagents` handler renders text")
}

// =================================================================================================
// §I.6 — the no-UI metadata block
// =================================================================================================

/// pi `:413-417`: with no UI, `/subagents <name>` emits `metadataFor(agent)` as the whole answer
/// and returns before the action selector exists.
///
/// Gutted: a stub returning `Ok(String::new())`, or one that falls through to the interactive
/// selector, produces no `Agent: scout (builtin)` first line and the first assertion fires.
/// A handler that renders a DIFFERENT renderer (a second, hand-rolled `metadataFor`) fails the
/// `System prompt mode:` assertion, because only `format_agent_detail` emits that label.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagents_with_no_ui_emits_the_full_metadata_block_for_the_named_agent() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    let ext = extension(home.path(), cwd.path());

    let output = subagents(&ext, BUILTIN, &headless_ctx(cwd.path())).await;

    assert!(
        output.starts_with("Agent: scout (builtin)"),
        "the no-UI branch must emit `format_agent_detail`'s own first line \
         (`render.rs:42`, `Agent: {{name}} ({{source}})`) as the head of the block; got:\n{output}"
    );
    assert!(
        output.contains("System prompt mode: replace"),
        "the block must carry `render.rs:76-83`'s system-prompt-mode line, and `scout.md` \
         declares `systemPromptMode: replace`; got:\n{output}"
    );
    assert!(
        output.contains("Description: "),
        "a metadata BLOCK, not a one-line summary: `metadataSummary` (pi `:354-360`) is the \
         three-field picker line and is NOT what the no-UI branch emits; got:\n{output}"
    );
    // The precondition §I.7's after-assertion rests on, asserted here so a scout.md that grew a
    // `model:` key fails THIS test with a clear reason rather than making the next one vacuous.
    assert!(
        !output.contains("\nModel: "),
        "precondition: the bundled `scout` declares no model, so `render.rs:59` emits no \
         `Model:` line yet; got:\n{output}"
    );
}

// =================================================================================================
// §I.7 — a builtin override really persists, and a re-read really sees it
// =================================================================================================

/// pi `saveAgentModel` (`:307-329`) for a BUILTIN: `savesThroughSettings` is `true` (`:126`), so
/// the pick is written through `persistSettingsField` → `mergeBuiltinAgentOverride`, and the
/// answer names the scope and the file.
///
/// Both halves are load-bearing and they fail differently:
///
/// * the FILE assertion fails if nothing was written at all, or if the key nests differently from
///   pi's `subagents.agentOverrides.<name>.model`;
/// * the RE-READ assertion fails if the write landed in the wrong scope's path — a bug the file
///   assertion alone cannot see, because a test that reads back whatever path the writer chose can
///   never disagree with the writer. Re-running `/subagents scout` goes through
///   `discovery_config_on_disk`'s OWN user/project path resolution instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_builtin_model_override_is_written_to_user_settings_and_seen_by_a_re_discovery() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    let ext = extension(home.path(), cwd.path());

    let model = a_real_model_id();
    // pi's two interactive picks, in `saveAgentModel`'s own order: the model
    // (`chooseModel`, `:283`), then the scope (`chooseOverrideScope`, `:309`).
    let host = ScriptedSelect::new(&[model.as_str(), "user"]);
    ext.executor()
        .set_host_services(Arc::clone(&host) as Arc<dyn HostServices>);

    let settings = user_settings_path(home.path());
    assert!(
        override_model(&settings, BUILTIN).is_none(),
        "precondition: no override yet at {}",
        settings.display()
    );

    // `<agent> model` is pi's preset-action form (`:420`) — with an action token present the
    // handler never opens the action selector, so the ONLY prompts are the two scripted above.
    let output = subagents(&ext, &format!("{BUILTIN} model"), &ui_ctx(cwd.path())).await;

    assert!(
        output.contains(&format!("'{BUILTIN}'")) && output.contains(&model),
        "the answer must be pi `:312`'s save receipt, naming the agent and the chosen model; \
         got: {output}\n{}",
        host.transcript()
    );

    // (1) THE BYTES ON DISK, at the user scope's own path.
    assert_eq!(
        override_model(&settings, BUILTIN).as_deref(),
        Some(model.as_str()),
        "`subagents.agentOverrides.{BUILTIN}.model` must be in {} after a `user`-scope save; \
         file contents: {:?}\n{}",
        settings.display(),
        std::fs::read_to_string(&settings).ok(),
        host.transcript()
    );

    // (2) THE RE-READ: a fresh discovery, through the command's own path resolution, reports it.
    let after = subagents(&ext, BUILTIN, &headless_ctx(cwd.path())).await;
    assert!(
        after.contains(&format!("Model: {model}")),
        "a re-discovery must report the override on the agent — a write to a path the reader \
         does not consult passes assertion (1) and fails here; got:\n{after}"
    );
}

// =================================================================================================
// §I.9 — a CYRUP_SUBAGENT_EXTRA_AGENT_DIRS agent refuses
// =================================================================================================

/// pi `isReadOnlyExtraAgent` (`:139-147`) + `readOnlyAgentMessage`'s third arm (`:158`):
/// `Cannot update '${agent.name}' because its definition in PI_SUBAGENT_EXTRA_AGENT_DIRS is
/// read-only.` — with cyrup's own env-var name substituted, which is the one rebrand §G.3 licenses
/// (`CYRUP_SUBAGENT_EXTRA_AGENT_DIRS`, `discovery/mod.rs:107`). The sentence SHAPE is upstream's,
/// so the assertion is anchored on `… is read-only.` and on the agent name, not on prose that
/// differs by a product noun.
///
/// This is the one test in this file that moves the process environment.
/// `AgentDiscoveryConfig::with_env_extras` (`discovery/mod.rs:741-744`) reads
/// `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` from `std::env` directly and takes no injected lookup, and
/// `isReadOnlyExtraAgent`'s path-containment check reads the SAME variable — so there is no seam
/// in this crate through which the behaviour can be reached otherwise. Sound here because
/// `cargo nextest` runs every test in its own process (this target's supported invocation; see
/// `main.rs`), and nothing else in this file reads that variable.
///
/// Gutted: drop the path-containment check and the edit goes through, so the assertion sees a
/// save receipt naming the model instead of the refusal — which is exactly what the second
/// assertion reports.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extra_agent_dirs_agent_refuses_a_model_edit_as_read_only() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    let extra = tempfile::tempdir().expect("extra agent dir tempdir");
    const AGENT: &str = "extra-dir-probe";

    std::fs::write(
        extra.path().join(format!("{AGENT}.md")),
        format!(
            "---\nname: {AGENT}\ndescription: a read-only extra-dir persona for the VL-S11a \
             refusal proof\n---\n\nYou are {AGENT}.\n"
        ),
    )
    .expect("write the extra-dir persona");

    // SAFETY: `cargo nextest run -p cyrup-it --features it` gives every test its own process, so
    // this variable is not shared with a concurrently-running test. See the doc comment above.
    unsafe {
        std::env::set_var(
            cyrup_ext_subagents::discovery::EXTRA_AGENT_DIRS_ENV_VAR,
            extra.path(),
        );
    }

    let ext = extension(home.path(), cwd.path());
    let model = a_real_model_id();
    let host = ScriptedSelect::new(&[model.as_str(), "user"]);
    ext.executor()
        .set_host_services(Arc::clone(&host) as Arc<dyn HostServices>);

    // Sanity: the extra dir really is folded into the USER tier (`with_prepended_user_extras`,
    // `discovery/mod.rs:752`), which is `isReadOnlyExtraAgent`'s own `agent.source !== "user"`
    // precondition (`:141`).
    let detail = subagents(&ext, AGENT, &headless_ctx(cwd.path())).await;
    assert!(
        detail.starts_with(&format!("Agent: {AGENT} (user)")),
        "precondition: a CYRUP_SUBAGENT_EXTRA_AGENT_DIRS persona is discovered at USER source; \
         got:\n{detail}"
    );

    let output = subagents(&ext, &format!("{AGENT} model"), &ui_ctx(cwd.path())).await;

    assert_eq!(
        output,
        format!(
            "Cannot update '{AGENT}' because its definition in CYRUP_SUBAGENT_EXTRA_AGENT_DIRS is \
             read-only."
        ),
        "pi `subagents-admin.ts:158`'s sentence, with the one substitution §G.3 licenses — cyrup's \
         own env-var name. Note it names NO field: upstream's third arm is agent-wide.\n{}",
        host.transcript()
    );
    assert!(
        !output.contains(&model),
        "the path-containment check must REFUSE, not save: a receipt naming the model here means \
         `isReadOnlyExtraAgent` let the edit through; got: {output}"
    );
    assert!(
        override_model(&user_settings_path(home.path()), AGENT).is_none(),
        "and nothing reached the settings file"
    );
}

// =================================================================================================
// The RUNTIME-registered refusal — the other reachable arm of `readOnlyAgentMessage`
// =================================================================================================

/// pi `readOnlyAgentMessage`'s FIRST arm (`subagents-admin.ts:151-153`):
/// `Cannot update '<name>' <field> because that agent is runtime-registered by an extension; edit
/// its source definition instead.`
///
/// This arm is reachable where the package arm is not, and for a precise reason:
/// `savesThroughSettings` returns `true` for `builtin` and `package` outright (`:126-127`) and then
/// falls to `if (!agent.override) return false` (`:128`). A runtime agent has no settings override
/// — it is defined in process — so the guard answers `false` and the save path DOES consult the
/// refusal. Asserting it here rather than only in a unit test is what proves that chain: a port
/// that returned `true` for `Runtime` too would write a settings override for an agent whose
/// definition lives in an extension's code, and this test would see a save receipt.
///
/// Gutted: make `saves_through_settings` answer `true` for `Runtime` and the first assertion sees
/// `Saved user settings override for 'runtime-probe' with model '…'`. Collapse the three refusals
/// into one generic sentence and the byte assertion fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_registered_agent_refuses_a_model_edit_with_upstreams_exact_sentence() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    const AGENT: &str = "runtime-probe";

    let ext = extension(home.path(), cwd.path());
    // pi's public `registerAgent` (`api/agents.ts:2`), which is `mergeRuntimeAgents`' source and
    // therefore what puts an agent at `AgentSource::Runtime` in this executor's every discovery.
    // The handle is held for the whole test: dropping it does not dispose, but keeping it named
    // says that the registration's lifetime is deliberate.
    let _registration = ext
        .register_agent(
            AGENT,
            &cyrup_ext_subagents::discovery::runtime_registry::RuntimeAgentDefinition::new(
                "a runtime-registered persona for the VL-S11a read-only refusal proof",
                "You are runtime-probe.",
            ),
        )
        .expect("the runtime registration validates");

    let model = a_real_model_id();
    let host = ScriptedSelect::new(&[model.as_str(), "user"]);
    ext.executor()
        .set_host_services(Arc::clone(&host) as Arc<dyn HostServices>);

    // Sanity: the agent really is discovered, and really is at RUNTIME source. Without this a
    // not-found answer could satisfy a sloppier assertion by accident.
    let detail = subagents(&ext, AGENT, &headless_ctx(cwd.path())).await;
    assert!(
        detail.starts_with(&format!("Agent: {AGENT} (runtime)")),
        "precondition: `register_agent` must put the persona at runtime source in the same \
         discovery `/subagents` reads; got:\n{detail}"
    );

    let output = subagents(&ext, &format!("{AGENT} model"), &ui_ctx(cwd.path())).await;

    assert_eq!(
        output,
        format!(
            "Cannot update '{AGENT}' model because that agent is runtime-registered by an \
             extension; edit its source definition instead."
        ),
        "pi `subagents-admin.ts:152`'s sentence, verbatim and ALONE — the refusal is the whole \
         answer, not a prefix on a save receipt.\n{}",
        host.transcript()
    );
    assert!(
        override_model(&user_settings_path(home.path()), AGENT).is_none(),
        "and the refusal must write nothing to the settings file"
    );
}
