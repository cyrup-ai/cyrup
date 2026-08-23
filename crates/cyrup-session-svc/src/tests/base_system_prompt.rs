//! The ASSEMBLED system prompt: everything that feeds it, and that it stays live.
//!
//! One file for the whole prompt-assembly seam, because every defect here has the same shape — a
//! contribution that reaches the registry but not the prompt, or a prompt that is rebuilt and then
//! silently reverted. The contributors are the ACTIVE TOOL SET (built-in snippets/guidelines, the
//! `tools`/`excludeTools` selection, `setActiveToolsByName`, extension-registered tools) and the
//! DISCOVERED RESOURCES (`.cyrup/SYSTEM.md` / `APPEND_SYSTEM.md`, user-tier `~/.agents/skills`,
//! extension-contributed skills).
//!
//! Pi keeps `_baseSystemPrompt` as MUTABLE state (`agent-session.ts:371`): `setActiveToolsByName`
//! reassigns it from `_rebuildSystemPrompt(validToolNames)` (:939) and the run path then reads the
//! LIVE field both when handing the prompt to `before_agent_start` (:1228) and when resetting the
//! agent because no handler replaced it (:1252).
//!
//! cyrup's `assemble_run_messages` used to read the builder-frozen `services.system_prompt`
//! instead, so the first run AFTER a `/tools` toggle reverted the prompt to the startup tool set —
//! but only once some extension subscribed to `BeforeAgentStart` (without a subscriber the fast
//! path returns early and the rebuild survives), which made it look like an extension bug.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{Content, ExtensionId, StopReason, Tool, ToolError, ToolResult, ToolUpdateSink};
use cyrup_ext::{
    EventKind, ExtError, HandledValue, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use super::common::{base_config, fixture};
use crate::SessionBuilder;
use futures::StreamExt;

// ============================================================ the live base across a run reset ====

/// A `before_agent_start` subscriber that changes NOTHING — the minimal condition that takes the
/// dispatch path instead of the `no_subscribers` fast path. Stands in for the real-world case:
/// `cyrup-permission-system` subscribes to `BeforeAgentStart` (extension.rs:1081) and arms itself
/// merely from the presence of a `cyrup-permissions.jsonc` file.
struct PassiveStartSubscriber;

#[async_trait::async_trait]
impl NativeExtension for PassiveStartSubscriber {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("passive-start-subscriber")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart]);
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// A tool-set rebuild becomes the new BASE prompt: the next run's `before_agent_start` reset must
/// restore the REBUILT prompt, not the one the builder assembled at session start.
#[tokio::test]
async fn tool_rebuild_updates_the_base_prompt_the_next_run_resets_to() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);

    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .with_native_extension(Arc::new(PassiveStartSubscriber))
        .build()
        .await
        .expect("build");

    let startup = session.system_prompt().to_string();

    // Narrow the active set to a single tool (Pi `setActiveToolsByName`, agent-session.ts:939).
    let all = session.all_tools();
    assert!(all.len() > 1, "fixture must expose more than one enable-able tool: {}", all.len());
    let keep = all[0].name.clone();
    session.set_active_tools_by_name(std::slice::from_ref(&keep)).await;

    let rebuilt = session.current_system_prompt().await;
    assert_ne!(rebuilt, startup, "narrowing the tool set must change the assembled prompt");
    assert_eq!(session.base_system_prompt(), rebuilt, "the rebuild is the new live base");

    // A run with a `before_agent_start` subscriber that replaces nothing.
    let stream = session.prompt("hello").await.expect("prompt accepted");
    session.wait_for_idle().await;
    let _ = stream.collect::<Vec<_>>().await;

    assert_eq!(
        session.current_system_prompt().await,
        rebuilt,
        "before_agent_start reverted the agent to the STARTUP prompt, discarding the tool rebuild"
    );
    assert_eq!(session.base_system_prompt(), rebuilt, "the live base must still be the rebuild");
    assert_eq!(session.active_tool_names(), vec![keep], "the active tool set must be unchanged");
}

// ================================================== the ACTIVE TOOL SET as a contributor ====

/// Facade parity vs Pi `agent-session.ts` / `sdk.ts`: the `tools` / `excludeTools` selection — an allowlist keeps only what it names,
/// a denylist drops only what it names, observed through the assembled system prompt.
#[tokio::test]
async fn tool_selection_allowlist_and_excludelist() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());

    // The snippets are the tools' OWN Pi-verbatim `promptSnippet` (read.ts:213, bash.ts:328),
    // read off the `Tool` vtable — not a session-svc paraphrase table (gap 04, TOOL-003).
    const READ_SNIPPET: &str = "- read: Read file contents";
    const BASH_SNIPPET: &str = "- bash: Execute bash commands (ls, grep, find, etc.)";

    // Allowlist: only `read` is active → the system prompt advertises read, not bash.
    let mut allow = base_config(&fx);
    allow.tools = Some(vec!["read".to_string()]);
    let s_allow = SessionBuilder::new(faux.clone(), allow).build().await.unwrap();
    let p = s_allow.system_prompt();
    assert!(p.contains(READ_SNIPPET), "read tool should be active:\n{p}");
    assert!(!p.contains(BASH_SNIPPET), "bash should be excluded by the allowlist");

    // Denylist: exclude `bash` → its snippet disappears while others remain.
    let mut deny = base_config(&fx);
    deny.exclude_tools = vec!["bash".to_string()];
    let s_deny = SessionBuilder::new(faux, deny).build().await.unwrap();
    let pd = s_deny.system_prompt();
    assert!(pd.contains(READ_SNIPPET), "read should still be active");
    assert!(!pd.contains(BASH_SNIPPET), "bash should be excluded by the denylist");
}

/// TOOL-003: the built-ins' OWN `promptSnippet`/`promptGuidelines` reach the assembled system
/// prompt. Pre-fix `builder.rs` substituted a hardcoded name→paraphrase table and emitted ZERO
/// tool guidelines, so every assertion below failed.
#[tokio::test]
async fn builtin_prompt_snippets_and_guidelines_reach_the_system_prompt() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();
    let p = session.system_prompt().to_string();

    // (a) "Available tools" carries Pi's verbatim snippets, not the old paraphrases — for the
    // DEFAULT-ACTIVE tools only. pi's own builder hardcodes the same four as its fallback:
    // `const tools = selectedTools || ["read", "bash", "edit", "write"]` (`system-prompt.ts:80`),
    // and lists a tool only if it is in that set.
    for want in [
        "- read: Read file contents",
        "- write: Create or overwrite files",
        "- edit: Make precise file edits with exact text replacement, including multiple disjoint edits in one call",
        "- bash: Execute bash commands (ls, grep, find, etc.)",
    ] {
        assert!(p.contains(want), "missing snippet {want:?} in system prompt:\n{p}");
    }
    // ...and NOT for the three built-ins pi does not activate by default. This assertion is the
    // parity property: it previously required all seven, which is what encoded the divergence —
    // cyrup advertised `grep`/`find`/`ls` in every request's tool array AND system prompt, so the
    // model routed searches to them instead of `bash`, producing different transcripts than pi for
    // identical inputs. They remain enable-able via `set_active_tools_by_name`.
    for absent in [
        "- grep: Search file contents for patterns (respects .gitignore)",
        "- find: Find files by glob pattern (respects .gitignore)",
        "- ls: List directory contents",
    ] {
        assert!(
            !p.contains(absent),
            "{absent:?} must NOT be in the default system prompt (pi system-prompt.ts:80):\n{p}"
        );
    }
    for gone in [
        "Read a file from the workspace",
        "Write a file to the workspace",
        "Edit a file with a find/replace",
        "Run a shell command",
        "Find files by glob\n",
        "List a directory",
    ] {
        assert!(!p.contains(gone), "stale session-svc paraphrase {gone:?} still in prompt:\n{p}");
    }

    // (b) "Guidelines" carries the six Pi tool guidelines (read 1, write 1, edit 4).
    for want in [
        "- Use read to examine files instead of cat or sed.",
        "- Use write only for new files or complete rewrites.",
        "- Use edit for precise changes (edits[].oldText must match exactly)",
        "- When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls",
        "- Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.",
        "- Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.",
    ] {
        assert!(p.contains(want), "missing guideline {want:?} in system prompt:\n{p}");
    }
}

// A trivial custom tool (Pi `customTools`).
struct EchoTool {
    params: serde_json::Value,
}
impl EchoTool {
    fn new() -> Self {
        Self { params: serde_json::json!({"type": "object", "properties": {}}) }
    }
}
#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        "Echo a message"
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Echo a message back")
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _args: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult { content: vec![Content::text("echo")], details: None, terminate: false, ..Default::default() })
    }
}

/// Facade parity vs Pi `agent-session.ts`: dynamic tools + custom tools — toggling a built-in changes the ACTIVE set, and a
/// caller-registered custom tool joins it.
#[tokio::test]
async fn dynamic_tools_toggle_active_set_and_register_custom() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = base_config(&fx);
    cfg.custom_tools = vec![Arc::new(EchoTool::new())];
    let session = SessionBuilder::new(faux, cfg).build().await.unwrap();

    // The default active set is the built-in selection; the custom tool is enable-able but inactive.
    let active = session.active_tool_names();
    assert!(active.contains(&"read".to_string()), "read active by default: {active:?}");
    let all: Vec<String> = session.all_tools().into_iter().map(|t| t.name).collect();
    assert!(all.contains(&"echo".to_string()), "custom tool registered: {all:?}");
    assert!(session.tool_definition("echo").is_some());
    assert!(
        !session.active_tool_names().contains(&"echo".to_string()),
        "custom tool not auto-activated"
    );

    // Toggle the active set down to just read + echo; the agent's tool array follows.
    session.set_active_tools_by_name(&["read".to_string(), "echo".to_string()]).await;
    let active = session.active_tool_names();
    assert_eq!(active, vec!["read".to_string(), "echo".to_string()]);
    let snap = session.agent_messages().await; // force a snapshot to ensure no panic
    let _ = snap;
    // The agent's tool set now reflects the toggle.
    assert!(session.tool_definition("echo").unwrap().active, "echo is active after toggle");
    assert!(!session.tool_definition("write").map(|t| t.active).unwrap_or(false), "write toggled off");

    // Unknown names are ignored.
    session.set_active_tools_by_name(&["read".to_string(), "nope".to_string()]).await;
    assert_eq!(session.active_tool_names(), vec!["read".to_string()]);
}

// ======================================== an EXTENSION-registered tool as a contributor ====

/// EXT-038 / TOOL-021 — an extension-contributed tool's `promptSnippet` and `promptGuidelines`
/// must reach the SYSTEM PROMPT.
///
/// pi builds `_toolPromptSnippets` / `_toolPromptGuidelines` from `definitionRegistry`, which is
/// the base definitions with `allCustomTools` merged over them by name
/// (`core/agent-session.ts:2471-2504` @v0.83.0) — so an extension tool contributes its own text,
/// and an extension OVERRIDE of a built-in contributes the override's text instead of the
/// built-in's.
///
/// RED before this pass, for two independent reasons:
/// 1. `SessionBuilder` derived `selected_tools` + `tool_contributions` from `base_tools` alone and
///    only called `ext_host.active_tools(&base_tools)` AFTER the prompt had been built, so a guest
///    could register a fully-described tool and the model was never told it existed;
/// 2. `Tool::prompt_guidelines` returned `&[&str]`, which no tool owning `Vec<String>` can
///    implement — so even with the ordering fixed the guidelines leg had no reader (TOOL-021).
#[tokio::test]
async fn an_extension_tools_snippet_and_guidelines_reach_the_system_prompt() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx))
        .with_native_extension(Arc::new(DescribedToolExtension))
        .build()
        .await
        .expect("build");

    let prompt = session.system_prompt().to_string();
    assert!(
        prompt.contains("Deploys the thing to the place"),
        "the extension tool's `promptSnippet` must reach the Available tools section: {prompt}"
    );
    assert!(
        prompt.contains("Always dry-run deploy before deploying for real"),
        "the extension tool's `promptGuidelines` must reach the Guidelines section: {prompt}"
    );
}

/// A native extension contributing one tool with a snippet AND owned (`String`) guidelines — the
/// same ownership shape a WASM guest's `ToolDescriptor` has.
struct DescribedToolExtension;

#[async_trait::async_trait]
impl NativeExtension for DescribedToolExtension {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("described-tool-extension")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::new(DeployTool {
            params: serde_json::json!({"type": "object", "properties": {}}),
            guidelines: vec!["Always dry-run deploy before deploying for real".to_string()],
        }));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

struct DeployTool {
    params: serde_json::Value,
    guidelines: Vec<String>,
}

#[async_trait::async_trait]
impl cyrup_core::Tool for DeployTool {
    fn name(&self) -> &str {
        "deploy"
    }
    fn description(&self) -> &str {
        "deploy things"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Deploys the thing to the place")
    }
    fn prompt_guidelines(&self) -> Vec<&str> {
        self.guidelines.iter().map(String::as_str).collect()
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _params: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
        Ok(cyrup_core::ToolResult::default())
    }
}

// ========================================== DISCOVERED RESOURCES as contributors ====

/// CFG-035 — `.cyrup/SYSTEM.md` / `.cyrup/APPEND_SYSTEM.md` must actually be READ.
///
/// pi `discoverSystemPromptFile` / `discoverAppendSystemPromptFile`
/// (`core/resource-loader.ts:1022-1032`, `:1034-1044` @v0.83.0), consumed at `:525` and `:531-535`:
/// the project file `<cwd>/.cyrup/<name>` wins when the project is TRUSTED, else the global
/// `<agent_dir>/<name>`, else nothing — and exactly ONE path is returned per leg.
///
/// RED before this pass: both filenames existed in cyrup only as TRUST-GATE MARKERS
/// (`cyrup-config/src/trust.rs:208-209`). `SessionBuilder` set `custom_prompt` /
/// `append_system_prompt` from the CLI fields and nothing else, so cyrup asked the user to trust a
/// project *because of* a file it never opened: the assembled prompt would carry neither string.
#[tokio::test]
async fn system_md_and_append_system_md_are_discovered_and_read() {
    let fx = fixture();
    std::fs::create_dir_all(fx.cwd.join(".cyrup")).unwrap();
    std::fs::write(fx.cwd.join(".cyrup/SYSTEM.md"), "PROJECT-SYSTEM-BODY").unwrap();
    std::fs::write(fx.cwd.join(".cyrup/APPEND_SYSTEM.md"), "PROJECT-APPEND-BODY").unwrap();
    // A global pair too, to pin the PRECEDENCE: the trusted project file must win.
    std::fs::write(fx.agent_dir.join("SYSTEM.md"), "GLOBAL-SYSTEM-BODY").unwrap();

    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.expect("build");

    let prompt = session.system_prompt().to_string();
    assert!(
        prompt.contains("PROJECT-SYSTEM-BODY"),
        "the trusted project `.cyrup/SYSTEM.md` must REPLACE the default body: {prompt}"
    );
    assert!(
        !prompt.contains("GLOBAL-SYSTEM-BODY"),
        "the project file wins outright; upstream returns on the first hit and does not stack"
    );
    assert!(
        prompt.contains("PROJECT-APPEND-BODY"),
        "`.cyrup/APPEND_SYSTEM.md` must be appended: {prompt}"
    );
}

/// CFG-035, the trust gate: the gate applies to the PROJECT rung ONLY, so an untrusted project
/// falls THROUGH to the global `<agent_dir>/SYSTEM.md` rather than yielding nothing
/// (`resource-loader.ts:1023-1030` — the `existsSync(globalPath)` rung is outside the
/// `isProjectTrusted()` guard).
#[tokio::test]
async fn an_untrusted_project_falls_through_to_the_global_system_md() {
    let fx = fixture();
    std::fs::create_dir_all(fx.cwd.join(".cyrup")).unwrap();
    std::fs::write(fx.cwd.join(".cyrup/SYSTEM.md"), "PROJECT-SYSTEM-BODY").unwrap();
    std::fs::write(fx.agent_dir.join("SYSTEM.md"), "GLOBAL-SYSTEM-BODY").unwrap();

    let mut cfg = base_config(&fx);
    cfg.trust_override = Some(false);
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, cfg).build().await.expect("build");

    let prompt = session.system_prompt().to_string();
    assert!(
        prompt.contains("GLOBAL-SYSTEM-BODY"),
        "an untrusted project must still get the GLOBAL file: {prompt}"
    );
    assert!(
        !prompt.contains("PROJECT-SYSTEM-BODY"),
        "the untrusted project file must not be read"
    );
}

/// CFG-035 — an explicit CLI `--system-prompt` / `--append-system-prompt` SUPPRESSES discovery
/// (`this.systemPromptSource ?? this.discoverSystemPromptFile()`, `:525`; and `if (!appendSources)`
/// at `:531`). pi does not stack the CLI value on top of the discovered file.
#[tokio::test]
async fn a_cli_prompt_suppresses_discovery_rather_than_stacking() {
    let fx = fixture();
    std::fs::create_dir_all(fx.cwd.join(".cyrup")).unwrap();
    std::fs::write(fx.cwd.join(".cyrup/SYSTEM.md"), "PROJECT-SYSTEM-BODY").unwrap();
    std::fs::write(fx.cwd.join(".cyrup/APPEND_SYSTEM.md"), "PROJECT-APPEND-BODY").unwrap();

    let mut cfg = base_config(&fx);
    cfg.system_prompt = Some("CLI-SYSTEM-BODY".to_string());
    cfg.append_system_prompt = Some("CLI-APPEND-BODY".to_string());
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, cfg).build().await.expect("build");

    let prompt = session.system_prompt().to_string();
    assert!(prompt.contains("CLI-SYSTEM-BODY"), "{prompt}");
    assert!(prompt.contains("CLI-APPEND-BODY"), "{prompt}");
    assert!(!prompt.contains("PROJECT-SYSTEM-BODY"), "discovery must be suppressed: {prompt}");
    assert!(!prompt.contains("PROJECT-APPEND-BODY"), "discovery must be suppressed: {prompt}");
}

/// The session-svc builder plumbs `DiscoveryConfig.user_agents_dir = $HOME/.agents` (Pi
/// `userAgentsSkillsDir`, package-manager.ts:2286), so a skill placed at `$HOME/.agents/skills/<name>`
/// is discovered by the ASSEMBLED session as a user-tier source (and is not trust-gated).
#[tokio::test]
async fn builder_loads_user_tier_agents_skills() {
    let fx = fixture();
    // A distinct HOME with a user-tier `.agents/skills/userskill` skill.
    let home = fx._tmp.path().join("home");
    let skill_dir = home.join(".agents").join("skills").join("userskill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: userskill\ndescription: a user-tier skill\n---\n\nUSER_SKILL_BODY\n",
    )
    .unwrap();

    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = base_config(&fx);
    cfg.home = home;
    cfg.trust_override = Some(false); // user-tier skills are NOT trust-gated.
    let session = SessionBuilder::new(faux, cfg).build().await.expect("build");

    let catalog = session.slash_command_catalog();
    let has_user_skill = catalog.iter().any(|c| {
        c.get("name").and_then(serde_json::Value::as_str) == Some("skill:userskill")
    });
    assert!(has_user_skill, "user-tier ~/.agents/skills/userskill must be discovered: {catalog:?}");
}

/// A native extension that contributes a skill path via `resources_discover` (Pi handler returning
/// `{ skillPaths: [...] }`).
struct ResourceContributor {
    skill_path: PathBuf,
}
#[async_trait::async_trait]
impl NativeExtension for ResourceContributor {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("resource-contributor")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ResourcesDiscover]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::ResourcesDiscover { .. } => HookOutcome::Handled(HandledValue(serde_json::json!({
                "skillPaths": [self.skill_path.to_string_lossy()],
            }))),
            _ => HookOutcome::Noop,
        }
    }
}

/// gap-09 #17b — `extendResourcesFromExtensions("startup")` (Pi agent-session.ts:2112-2135): the
/// skill/prompt/theme paths a `resources_discover` handler contributes are merged into the resource
/// registry BEFORE skill pointers + the system prompt are derived, so the contributed skill appears
/// in the assembled prompt (Pi `extendResourcesFromExtensions` → `_rebuildSystemPrompt`).
#[tokio::test]
async fn extension_contributed_skill_is_merged_into_resources_and_system_prompt() {
    let fx = fixture();
    // An out-of-tree skill file the extension will contribute (NOT under any discovery root).
    let skill_dir = fx._tmp.path().join("ext-skills").join("extskill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_md = skill_dir.join("SKILL.md");
    std::fs::write(
        &skill_md,
        "---\nname: extskill\ndescription: contributed by an extension\n---\n\nEXT_SKILL_BODY\n",
    )
    .unwrap();

    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx))
        .with_native_extension(Arc::new(ResourceContributor { skill_path: skill_md.clone() }))
        .build()
        .await
        .unwrap();

    assert!(
        session.resources().skills.contains("extskill"),
        "the extension-contributed skill is merged into the resource registry"
    );
    assert!(
        session.system_prompt().contains("extskill"),
        "the contributed skill is listed in the rebuilt system prompt"
    );
}

/// gap-09 #17b, the negative half: with no `resources_discover` contribution the discovered registry
/// is left untouched (Pi's early returns at agent-session.ts:2118/2124) — no extension skill leaks
/// in.
#[tokio::test]
async fn no_extension_contribution_leaves_registry_untouched() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();
    assert!(
        !session.resources().skills.contains("extskill"),
        "no contribution means no extension-supplied skill"
    );
}
