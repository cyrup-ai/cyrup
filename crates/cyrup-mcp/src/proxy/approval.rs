//! `tool-approval.ts` — the approval predicate and the approval dialog
//! (MCP-231, MCP-232).
//!
//! See [`crate::proxy`] for the module overview.


use indexmap::{IndexMap, IndexSet};
use serde_json::{Map as JsonMap, Value};

use cyrup_core::{CancelToken};

use crate::config::{
    BoolOrList, McpConfig,
    ToolPrefix,
};
use crate::state::McpState;
use crate::proxy::constants::{
    APPROVAL_OPTIONS, APPROVAL_PREVIEW_LENGTH, APPROVE_FOR_SESSION_OPTION, APPROVE_ONCE_OPTION,
};
use crate::proxy::env::{ApprovalOrigin, ApprovalOutcome};
use crate::proxy::tool_metadata::{ToolMetadata, matches_tool_pattern, resolve_tool_prefix, tool_name_candidates};

// ==================================================================================================
// 15 · `tool-approval.ts` — the approval predicate and the approval dialog (MCP-231, MCP-232)
// ==================================================================================================
//
// Upstream these are `tool-approval.ts`'s two exported functions over the mutable
// `McpExtensionState` record. They land here rather than in a module of their own for the reason
// section 4 already gives: `ToolMetadata`, `ApprovalOrigin`, `ApprovalOutcome`,
// `tool_name_candidates` and `matches_tool_pattern` are all in this file, the sole caller
// ([`execute_call`], phase 8) is in this file, and the third piece — the session cache key — is in
// [`crate::state`] beside the set it keys, exactly where `tool-approval.ts:151-152 @v2.26.1` puts it
// relative to `state.approvedToolCalls`.
//
// **Free functions, not `ProxyEnv` methods.** [`ProxyEnv::ensure_tool_call_approved`] and
// [`ProxyEnv::is_tool_call_approval_required`] stay on the trait — that is the seam a mode test
// scripts a denial through — but the trait has no production implementor yet, and upstream's are
// free functions over the state. So the *port* is here, and the eventual production `ProxyEnv`
// forwards to it in two lines. Anything else would put the gate somewhere a direct tool
// (`direct-tools.ts:432`, which has no `ProxyEnv` at all) cannot reach it.

/// `tool-approval.ts:35-93 @v2.26.1` `isToolCallApprovalRequired(config, serverName, toolMeta,
/// toolMetadata?)` — does this tool prompt before it runs? (MCP-231)
///
/// # The ladder
///
/// A per-server `approveTools` wins on **presence**, not on truthiness: `approveTools: false` on a
/// server switches approval off for it even when the global setting is `true`. `true` always
/// requires; anything that is not a non-empty list never does.
///
/// # The legacy arm, and the collision test that makes it safe
///
/// A pattern is first matched against the tool's **current** names
/// (`tool_name_candidates(..., include_legacy = false)`). Only when that misses does the
/// pre-2.x residue get a look — the legacy-inclusive set minus everything already current, plus one
/// explicit injection: the first non-bare current candidate with `-` mapped to `_`, which is the
/// spelling a config written against an older adapter would carry. That residue only gates the tool
/// for a pattern that does **not** also reach some other *current* tool name, which is what stops a
/// stale `approveTools` entry from silently gating a different server's tool after a rename.
///
/// # The two scopes differ in exactly one expression
///
/// `otherCurrentCandidates` — this server's tools under this server's prefix for the server scope,
/// every server's tools each under its own prefix for the global one. Upstream writes the whole
/// twenty-line block twice; here it is one parameter, per 13e's own instruction.
///
/// # The `tool_metadata == None` asymmetry is real
///
/// With no metadata to test collisions against, the **server** scope falls back to matching the
/// full legacy-inclusive set while the **global** scope returns `false`. That is not a bug to
/// normalise: a server-scoped `approveTools` names tools the user has already scoped to one server,
/// so a legacy alias cannot reach anything else, whereas a global pattern with no way to check
/// collisions must not gate on a guess. `ensure_tool_call_approved` never takes this path — it
/// always supplies a map, as upstream always passes `state.toolMetadata` — so it is reachable only
/// from `describe`/`search`'s marker and from a caller that omits the argument.
#[must_use]
pub fn is_tool_call_approval_required(
    config: &McpConfig,
    server_name: &str,
    tool: &ToolMetadata,
    tool_metadata: Option<&IndexMap<String, Vec<ToolMetadata>>>,
) -> bool {
    let definition = config.mcp_servers.get(server_name);
    let server_approval = definition.and_then(|entry| entry.approve_tools.as_ref());
    // `serverApproval !== undefined ? serverApproval : config.settings?.approveTools` — presence,
    // not truthiness, so a per-server `false` beats a global `true`.
    let approval = match server_approval {
        Some(value) => Some(value),
        None => config.settings_or_default().approve_tools(),
    };
    let patterns: &[String] = match approval {
        // `if (approval === true) return true;`
        Some(BoolOrList::All(true)) => return true,
        Some(BoolOrList::Named(list)) if !list.is_empty() => list.as_slice(),
        // `if (!Array.isArray(approval) || approval.length === 0) return false;` — which is
        // `false`, an empty list, and an absent value alike.
        _ => return false,
    };

    let prefix = resolve_tool_prefix(definition, config.tool_prefix());
    let current = tool_name_candidates(&tool.original_name, server_name, prefix, false);
    // Both scopes run this test first and identically, so it is hoisted out of the branch.
    if matches_tool_pattern(&current, patterns) {
        return true;
    }

    let Some(metadata) = tool_metadata else {
        return if server_approval.is_some() {
            // `matchesToolPattern(getToolNameCandidates(originalName, serverName, prefix), approval)`
            // — the DEFAULT fourth argument, i.e. legacy-inclusive and *not* minus the current set.
            matches_tool_pattern(
                &tool_name_candidates(&tool.original_name, server_name, prefix, true),
                patterns,
            )
        } else {
            false
        };
    };

    let other_current = if server_approval.is_some() {
        // Server scope: `toolMetadata.get(serverName) ?? []`, under THIS server's prefix.
        let mut set = IndexSet::new();
        for other in metadata.get(server_name).map(Vec::as_slice).unwrap_or_default() {
            set.extend(tool_name_candidates(&other.original_name, server_name, prefix, false));
        }
        set
    } else {
        // Global scope: every server, each under `resolveToolPrefix(config.mcpServers[name], …)`.
        let mut set = IndexSet::new();
        for (other_server, tools) in metadata {
            let other_prefix =
                resolve_tool_prefix(config.mcp_servers.get(other_server), config.tool_prefix());
            for other in tools {
                set.extend(tool_name_candidates(
                    &other.original_name,
                    other_server,
                    other_prefix,
                    false,
                ));
            }
        }
        set
    };

    approval_legacy_arm(&tool.original_name, server_name, prefix, patterns, &current, other_current)
}

/// The tail both scopes of [`is_tool_call_approval_required`] share (`tool-approval.ts:53-67 @v2.26.1`,
/// repeated verbatim at `:74-92` of the same tag).
///
/// The **order** of the two mutations is load-bearing and is upstream's: the `-`→`_` alias is added
/// to the legacy set *before* the current candidates are deleted from it. If the emitted name
/// carries no `-` it IS a current candidate, and adding it after the deletion would smuggle a
/// current name into the legacy set — turning a pattern that already failed the current test into a
/// match.
fn approval_legacy_arm(
    original_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    patterns: &[String],
    current: &IndexSet<String>,
    mut other_current: IndexSet<String>,
) -> bool {
    let mut legacy = tool_name_candidates(original_name, server_name, prefix, true);
    // `[...currentCandidates].find(c => c !== toolMeta.originalName)?.replace(/-/g, "_")` — the
    // first prefixed spelling, normalised. `IndexSet` iterates in insertion order, which is what
    // makes "first" mean the same thing here as in a JS `Set`.
    if let Some(emitted) = current.iter().find(|candidate| *candidate != original_name) {
        legacy.insert(emitted.replace('-', "_"));
    }
    for candidate in current {
        legacy.shift_remove(candidate);
    }
    for candidate in current {
        other_current.shift_remove(candidate);
    }
    patterns.iter().any(|pattern| {
        matches_tool_pattern(&legacy, std::slice::from_ref(pattern))
            && !matches_tool_pattern(&other_current, std::slice::from_ref(pattern))
    })
}

/// `tool-approval.ts:174-176 @v2.26.1` — `JSON.stringify(args ?? {}, null, 2)` → `sanitizeTerminalText` →
/// the 500-character preview with a literal `...` tail.
///
/// The order is the security property, and the two halves cover different bytes. `JSON.stringify`
/// escapes `U+0000..U+001F` (so an `ESC` in an argument *value* reaches the dialog as the literal
/// six characters `\u001b`, inert), but it emits `U+007F` and the whole C1 block —
/// **including `U+009D`, the one-byte OSC introducer** — raw. Sanitising the rendered JSON is what
/// neutralises those. Sanitising the arguments *before* rendering would instead let the renderer
/// re-introduce nothing and would corrupt the values shown; sanitising after is both safe and
/// faithful.
///
/// `sanitized.length > 500` and `.slice(0, 500)` count UTF-16 code units in JS, so this counts
/// [`str::encode_utf16`] — the same measure [`crate::proxy::truncate_at_word`] uses and for the same reason. The
/// one divergence, also shared with it: a cut that would land inside an astral character stops
/// before it rather than emitting the lone surrogate JS would.
///
/// **Recorded display divergence.** `JSON.stringify` emits object keys in insertion order;
/// `serde_json` without `preserve_order` emits them sorted, and the arguments arrived through
/// `serde_json` in the first place, so the model's original order is not recoverable here at all.
/// This affects only what the dialog *shows*. It cannot affect what is approved: the cache key runs
/// over [`crate::dirs::stable_stringify`], which sorts keys by construction
/// ([`crate::state::approval_cache_key`]).
fn approval_argument_preview(args: &Value) -> String {
    let empty = Value::Object(JsonMap::new());
    let rendered = serde_json::to_string_pretty(if args.is_null() { &empty } else { args })
        .unwrap_or_else(|_| "{}".to_string());
    let sanitized = crate::ui::sanitize_terminal_text(&rendered);
    if sanitized.encode_utf16().count() <= APPROVAL_PREVIEW_LENGTH {
        return sanitized;
    }
    let mut cut = sanitized.len();
    let mut used = 0usize;
    for (index, ch) in sanitized.char_indices() {
        let width = ch.len_utf16();
        if used + width > APPROVAL_PREVIEW_LENGTH {
            cut = index;
            break;
        }
        used += width;
    }
    format!("{}...", sanitized.get(..cut).unwrap_or(&sanitized))
}

/// `tool-approval.ts:142-195 @v2.26.1` `ensureToolCallApproved(state, serverName, toolMeta, args, signal,
/// origin, approvalMetadata?)` — the user's last line of defence before an MCP tool runs with
/// model-chosen arguments (MCP-232).
///
/// # The order of the checks is the unit
///
/// 1. **Session cache** — [`crate::state::approval_cache_key`]'s `(server, tool, sha256(args))`
///    triple. A hit approves without asking.
/// 2. **Is approval required at all** — [`is_tool_call_approval_required`]. Not required ⇒
///    approved, and no dialog.
/// 3. **Is there a UI** — `if (!state.ui) return {ok:false, reason:"approval_required_headless"}`.
///    **This runs BEFORE the dialog, and that ordering is the point, not an implementation
///    detail.** `HostServices::select` answers `None` for a dismissed dialog *and* for no
///    interactive surface, so a port that called `select` first and inferred the reason from `None`
///    would report "the user declined" to a batch job with no user in the room — and, worse, the
///    two states would be one, so the caller could not tell an operator "run this interactively"
///    from "someone said no".
/// 4. **The dialog** — three options, and every other answer denies.
///
/// # Fail-closed, on every arm
///
/// `Deny`, a dismissal (`None`), an unknown label, a poisoned cache lock and a cancellation all
/// resolve to [`ApprovalOutcome::Denied`] or [`ApprovalOutcome::NoInteractiveSession`]. There is no
/// path on which not-answering approves.
///
/// # The two deltas from upstream, both deliberate
///
/// * **No approval broker.** `requestBrokerApproval`'s synchronous `EventEmitter.emit` with a
///   `claim(handler)` closure is MCP-233's cut: cyrup's bus is deferred and has no return channel,
///   and `ExtHooks::before_tool_call` — which `cyrup-permission-system` already subscribes,
///   already derives MCP targets on, and which is the one `EventKind` whose `fails_closed()` is
///   `true` — *is* the broker, structurally. What that costs is recorded there: no `abstain` (a
///   permission extension that declines to decide simply does not block, which lands in the same
///   place) and no host-level `allow_for_session` (this function's own cache covers it for MCP).
/// * **Cancellation cannot interrupt an open dialog.** Upstream wraps the `select` in
///   `abortable(..., combineAbortSignals(state.owner?.signal, signal))`, which rejects mid-dialog.
///   `HostServices::select` is a blocking sync bridge with no cancellation parameter this crate can
///   supply (`DialogOptions::signal_id` is the host's own route and nothing wires it), so the token
///   is checked on **both** sides of the dialog instead: a cancelled call denies without asking,
///   and an answer that arrives after a cancellation is discarded rather than cached. The dialog
///   itself stays on screen until the human dismisses it. Stated rather than silently changed.
///
/// **Parameter order note.** Upstream's is `(state, serverName, toolMeta, args, signal, origin,
/// approvalMetadata)`; here `origin` precedes `cancel` so the signature matches
/// [`crate::proxy::ProxyEnv::ensure_tool_call_approved`], which every other cancellable verb in this file already
/// spells with the token late. Same parameters, same meanings.
pub async fn ensure_tool_call_approved(
    state: &McpState,
    server_name: &str,
    tool: &ToolMetadata,
    args: &Value,
    origin: ApprovalOrigin,
    cancel: &CancelToken,
    approval_metadata: &IndexMap<String, Vec<ToolMetadata>>,
) -> ApprovalOutcome {
    // `origin` reaches only `requestBrokerApproval` upstream, and that is MCP-233's cut. It stays
    // in the signature because it is the caller's *statement of which surface is asking* — the
    // three-way derivation at each call site (`proxy`, `direct`, `resource`) is part of the port
    // and is asserted by the conformance tests — and because the broker's replacement, the
    // `before_tool_call` gate, is the natural place for it to become a fact again.
    let _ = origin;

    let cache_key = crate::state::approval_cache_key(server_name, &tool.original_name, args);
    // `approvedToolCalls.has(cacheKey)`. A poisoned lock reads as a MISS, so the worst a lock
    // panicked mid-insert can do is prompt the user a second time.
    if state.approved_tool_calls.lock().is_ok_and(|approved| approved.contains(&cache_key)) {
        return ApprovalOutcome::Approved;
    }

    if !is_tool_call_approval_required(&state.config, server_name, tool, Some(approval_metadata)) {
        return ApprovalOutcome::Approved;
    }

    // `if (!state.ui) return {ok: false, reason: "approval_required_headless"}` — BEFORE the
    // dialog. See the doc comment: this is what keeps "no UI" and "the user said no" apart.
    let Some(dialog) = state.dialog() else {
        return ApprovalOutcome::NoInteractiveSession;
    };

    // `ownedSignal = combineAbortSignals(state.owner?.signal, signal)` — the generation's token OR
    // the caller's. Read as a predicate rather than built with [`crate::abort::combine`]: that
    // helper spawns a joiner task so the result can be *awaited*, and this call site only ever
    // polls, so the task would be pure cost on every gated call.
    let cancelled = || !state.owner.is_active() || cancel.is_cancelled();
    if cancelled() {
        return ApprovalOutcome::Denied;
    }

    let title = format!(
        "MCP: {} wants to run {}",
        crate::ui::sanitize_terminal_text(server_name),
        crate::ui::sanitize_terminal_text(&tool.original_name)
    );
    let prompt = format!("{title}\n\nArguments:\n{}", approval_argument_preview(args));
    let decision = dialog.select(&prompt, &APPROVAL_OPTIONS).await;
    if cancelled() {
        // The answer arrived after the run was cancelled: discard it rather than caching a
        // session-wide approval nothing will use.
        return ApprovalOutcome::Denied;
    }

    match decision.as_deref() {
        Some(APPROVE_ONCE_OPTION) => ApprovalOutcome::Approved,
        Some(APPROVE_FOR_SESSION_OPTION) => {
            if let Ok(mut approved) = state.approved_tool_calls.lock() {
                approved.insert(cache_key);
            }
            // The insert is best-effort for the same reason the lookup is: a poisoned lock costs a
            // repeat prompt, never an ungated call. The approval itself still stands for THIS call.
            ApprovalOutcome::Approved
        }
        // `return {ok: false, reason: "denied"}` — the literal `Deny`, an unknown label, a
        // dismissal, a timeout, and a fenced (stopped-generation) handle all land here.
        _ => ApprovalOutcome::Denied,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::proxy::testsupport::{FakeEnv, config_with, metadata_with, stdio};
    use crate::state::McpStateParts;
    use serde_json::json;
    use crate::config::{McpSettings, ServerEntry};
    use crate::lifecycle::McpLifecycleManager;
    use crate::owner::McpRuntimeOwner;
    use crate::proxy::constants::DENY_OPTION;
    use crate::proxy::env::ProxyCtx;
    use crate::state::McpServerManager;
    use std::sync::{Arc, Mutex};

    // ==============================================================================================
    // MCP-231 / MCP-232 — `tool-approval.ts`, transcribed from `__tests__/tool-approval.test.ts`
    // ==============================================================================================
    //
    // The broker cases (`lets a broker allow/deny…`, `requires brokers to claim synchronously`,
    // `fails closed when a claimed broker …`, `propagates aborts while a claimed broker is
    // pending`) do not port: MCP-233 cuts the broker, and `ExtHooks::before_tool_call` is its
    // replacement. Their fail-closed *content* survives here as
    // `an_unrecognised_answer_denies` and `a_cancelled_call_denies_without_asking`.

    /// A scripted [`cyrup_ext::HostServices`] standing in for `{ ui: { select } }`.
    ///
    /// Records what it was asked, so a test can assert the dialog's exact text and — the assertion
    /// upstream makes most often — how many times it was asked *at all*.
    #[derive(Default)]
    struct ScriptedUi {
        answer: Mutex<Option<String>>,
        prompts: Mutex<Vec<String>>,
        options: Mutex<Vec<Vec<String>>>,
        /// The P-3 gate to observe from inside the dialog (MCP-471), when a test wires one.
        gate: Option<Arc<cyrup_ext::HumanWaitGate>>,
        waiting_during_dialog: Mutex<Vec<bool>>,
    }

    impl ScriptedUi {
        fn answering(answer: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer.map(str::to_string)),
                ..Self::default()
            })
        }

        fn watching(answer: Option<&str>, gate: Arc<cyrup_ext::HumanWaitGate>) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer.map(str::to_string)),
                gate: Some(gate),
                ..Self::default()
            })
        }

        fn prompt_count(&self) -> usize {
            self.prompts.lock().unwrap().len()
        }

        fn last_prompt(&self) -> String {
            self.prompts.lock().unwrap().last().cloned().unwrap_or_default()
        }
    }

    impl cyrup_ext::HostServices for ScriptedUi {
        fn select(
            &self,
            prompt: &str,
            options: &Value,
            _opts: &cyrup_ext::DialogOptions,
        ) -> Option<String> {
            if let Some(gate) = self.gate.as_ref() {
                self.waiting_during_dialog.lock().unwrap().push(gate.is_waiting());
            }
            self.prompts.lock().unwrap().push(prompt.to_string());
            self.options.lock().unwrap().push(
                options
                    .as_array()
                    .map(|list| {
                        list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
                    })
                    .unwrap_or_default(),
            );
            self.answer.lock().unwrap().clone()
        }
    }

    /// An [`McpState`] over a real owner, with or without an interactive surface —
    /// `createState({interactive})`.
    fn approval_state(config: McpConfig, ui: Option<Arc<ScriptedUi>>) -> Arc<McpState> {
        let owner = Arc::new(McpRuntimeOwner::new());
        let manager = Arc::new(McpServerManager::default());
        let lifecycle =
            Arc::new(McpLifecycleManager::new(Arc::clone(&manager), Arc::new(|_: &str| false)));
        Arc::new(McpState::new(McpStateParts {
            owner: Arc::clone(&owner),
            manager,
            lifecycle,
            config,
            programmatic_config: None,
            oauth_runtime: crate::oauth::create_oauth_runtime(None),
            auth_storage_options: crate::state::AuthStorageOptions::default(),
            ui: ui.map(|services| {
                Arc::new(crate::owner::OwnedServices::new(
                    services as Arc<dyn cyrup_ext::HostServices>,
                    Arc::clone(&owner),
                ))
            }),
            open_browser: Arc::new(|_| Box::pin(async { Ok(()) })),
            send_message: Arc::new(|_| {}),
        }))
    }

    /// `const tool = { name: "demo_search-records", originalName: "search-records", … }`.
    fn demo_tool() -> ToolMetadata {
        ToolMetadata::new("demo_search-records", "search-records", "Search records")
    }

    /// `{ mcpServers: { demo: { command: "demo", approveTools } } }`.
    fn demo_config(approve: Option<BoolOrList>) -> McpConfig {
        let mut config = config_with(&[("demo", stdio("demo"))]);
        if let Some(entry) = config.mcp_servers.get_mut("demo") {
            entry.approve_tools = approve;
        }
        config
    }

    fn settings_approving(patterns: &[&str]) -> Option<McpSettings> {
        Some(McpSettings {
            approve_tools: Some(BoolOrList::Named(
                patterns.iter().map(|p| (*p).to_string()).collect(),
            )),
            ..McpSettings::default()
        })
    }

    // ---- MCP-231 · `isToolCallApprovalRequired` --------------------------------------------------

    /// `"matches original, prefixed, and read_* resource tool names"` — the three cases, each with
    /// **no** `toolMetadata`, which is also the only path that reaches the `None` asymmetry.
    #[test]
    fn approval_matches_original_prefixed_and_resource_tool_names() {
        let by_original = demo_config(Some(BoolOrList::Named(vec!["search-records".to_string()])));
        assert!(is_tool_call_approval_required(&by_original, "demo", &demo_tool(), None));

        let by_prefixed =
            demo_config(Some(BoolOrList::Named(vec!["demo_search-records".to_string()])));
        assert!(is_tool_call_approval_required(&by_prefixed, "demo", &demo_tool(), None));

        // A global glob against a `short`-mode alias: `docs-mcp` prefixes as `docs`, so the
        // resource tool's current candidate set carries `docs_read_handbook`.
        let mut resource_config = config_with(&[("docs-mcp", ServerEntry::default())]);
        resource_config.settings = settings_approving(&["docs_read_*"]);
        let mut resource_tool =
            ToolMetadata::new("docs_read_handbook", "read_handbook", "Read handbook");
        resource_tool.resource_uri = Some("docs://handbook".to_string());
        assert!(is_tool_call_approval_required(&resource_config, "docs-mcp", &resource_tool, None));
    }

    /// `"gates exact global selectors without applying them through a legacy collision"` — the
    /// selector names `my_2d_server`'s tool exactly, and must not reach `my-server`'s through the
    /// hyphen-escaped legacy alias the two servers share.
    #[test]
    fn a_global_selector_does_not_gate_the_wrong_server_through_a_legacy_alias() {
        let mut config =
            config_with(&[("my-server", stdio("hyphen")), ("my_2d_server", stdio("escaped"))]);
        config.settings = settings_approving(&["my_2d_server_do_thing"]);
        let hyphen = ToolMetadata::new("my-server_do-thing", "do-thing", "");
        let escaped = ToolMetadata::new("my_2d_server_do_thing", "do_thing", "");
        let metadata = metadata_with(&[
            ("my-server", vec![hyphen.clone()]),
            ("my_2d_server", vec![escaped.clone()]),
        ]);

        assert!(!is_tool_call_approval_required(&config, "my-server", &hyphen, Some(&metadata)));
        assert!(is_tool_call_approval_required(&config, "my_2d_server", &escaped, Some(&metadata)));
    }

    /// `"matches safe server-scoped normalized approval selectors"` and `"matches safe global
    /// normalized approval selectors"` — the legacy `-`→`_` alias DOES gate when nothing else
    /// currently answers to it, under either scope.
    #[test]
    fn a_normalized_legacy_selector_gates_when_nothing_else_answers_to_it() {
        let scoped = ToolMetadata::new("my-server_do_thing", "do_thing", "");
        let metadata = metadata_with(&[("my-server", vec![scoped.clone()])]);

        let mut server_scope = config_with(&[("my-server", stdio("demo"))]);
        if let Some(entry) = server_scope.mcp_servers.get_mut("my-server") {
            entry.approve_tools =
                Some(BoolOrList::Named(vec!["my_server_do_thing".to_string()]));
        }
        assert!(is_tool_call_approval_required(&server_scope, "my-server", &scoped, Some(&metadata)));

        let mut global_scope = config_with(&[("my-server", stdio("demo"))]);
        global_scope.settings = settings_approving(&["my_server_do_thing"]);
        assert!(is_tool_call_approval_required(&global_scope, "my-server", &scoped, Some(&metadata)));
    }

    /// `"does not gate a same-server legacy collision"` — `demo_search_records` is the *current*
    /// name of one tool and the *legacy* alias of another on the same server. The selector gates
    /// the tool that owns the name now, and only that one.
    #[test]
    fn a_same_server_legacy_collision_gates_only_the_current_owner() {
        let hyphen = ToolMetadata::new("demo_search-records", "search-records", "");
        let underscore = ToolMetadata::new("demo_search_records", "search_records", "");
        let mut config = config_with(&[("demo", stdio("demo"))]);
        config.settings = settings_approving(&["demo_search_records"]);
        let metadata = metadata_with(&[("demo", vec![hyphen.clone(), underscore.clone()])]);

        assert!(!is_tool_call_approval_required(&config, "demo", &hyphen, Some(&metadata)));
        assert!(is_tool_call_approval_required(&config, "demo", &underscore, Some(&metadata)));
    }

    /// The ladder itself: `true` always gates, a per-server value beats the global on **presence**
    /// (so a per-server `false` survives a global `true`), and neither `false` nor an empty list
    /// gates anything.
    #[test]
    fn the_approval_ladder_reads_presence_not_truthiness() {
        let tool = demo_tool();
        let always = demo_config(Some(BoolOrList::All(true)));
        assert!(is_tool_call_approval_required(&always, "demo", &tool, None));

        let mut server_off = demo_config(Some(BoolOrList::All(false)));
        server_off.settings = Some(McpSettings {
            approve_tools: Some(BoolOrList::All(true)),
            ..McpSettings::default()
        });
        assert!(
            !is_tool_call_approval_required(&server_off, "demo", &tool, None),
            "a per-server `false` overrides a global `true` — presence wins, not truthiness"
        );

        let empty = demo_config(Some(BoolOrList::Named(Vec::new())));
        assert!(!is_tool_call_approval_required(&empty, "demo", &tool, None));
        assert!(!is_tool_call_approval_required(&demo_config(None), "demo", &tool, None));
    }

    /// The `tool_metadata == None` asymmetry 13e names: with no collision context the **server**
    /// scope falls back to the full legacy set, the **global** scope refuses to guess.
    #[test]
    fn the_absent_metadata_asymmetry_between_the_two_scopes_is_preserved() {
        let scoped = ToolMetadata::new("my-server_do_thing", "do_thing", "");

        let mut server_scope = config_with(&[("my-server", stdio("demo"))]);
        if let Some(entry) = server_scope.mcp_servers.get_mut("my-server") {
            entry.approve_tools =
                Some(BoolOrList::Named(vec!["my_server_do_thing".to_string()]));
        }
        assert!(is_tool_call_approval_required(&server_scope, "my-server", &scoped, None));

        let mut global_scope = config_with(&[("my-server", stdio("demo"))]);
        global_scope.settings = settings_approving(&["my_server_do_thing"]);
        assert!(!is_tool_call_approval_required(&global_scope, "my-server", &scoped, None));
    }

    // ---- MCP-232 · `ensureToolCallApproved` -------------------------------------------------------

    /// **The unit's headline assertion** (13e MCP-232 "verify"): no UI and a cancelled dialog are
    /// two different answers, and the only thing that keeps them apart is checking for a UI
    /// *before* calling `select`. `HostServices::select` returns `None` for both.
    #[tokio::test]
    async fn no_ui_and_a_dismissed_dialog_are_not_the_same_answer() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let args = json!({ "query": "private" });

        // `createState({approveTools: true, interactive: false})`.
        let headless = approval_state(demo_config(Some(BoolOrList::All(true))), None);
        assert_eq!(
            ensure_tool_call_approved(
                &headless,
                "demo",
                &tool,
                &args,
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
                &metadata,
            )
            .await,
            ApprovalOutcome::NoInteractiveSession
        );

        // The same call with a UI whose dialog is dismissed — upstream's `select` resolving
        // `undefined`.
        let ui = ScriptedUi::answering(None);
        let interactive =
            approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));
        assert_eq!(
            ensure_tool_call_approved(
                &interactive,
                "demo",
                &tool,
                &args,
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
                &metadata,
            )
            .await,
            ApprovalOutcome::Denied
        );
        assert_eq!(ui.prompt_count(), 1, "the headless check must not suppress a real dialog");
    }

    /// `"caches only Allow for session decisions"` as rewritten by `5bcd6c5` — three calls, two
    /// prompts, two cache entries. The reordered payload is the same request; the changed `id` is
    /// a new one.
    #[tokio::test]
    async fn allow_for_session_caches_per_argument_payload() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(APPROVE_FOR_SESSION_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));

        for args in [
            json!({ "record": { "id": "safe", "type": "demo" } }),
            json!({ "record": { "type": "demo", "id": "safe" } }),
            json!({ "record": { "id": "other", "type": "demo" } }),
        ] {
            assert_eq!(
                ensure_tool_call_approved(
                    &state,
                    "demo",
                    &tool,
                    &args,
                    ApprovalOrigin::Proxy,
                    &CancelToken::new(),
                    &metadata,
                )
                .await,
                ApprovalOutcome::Approved
            );
        }

        assert_eq!(ui.prompt_count(), 2, "the reordered payload reuses the first approval");
        assert_eq!(state.approved_tool_calls.lock().unwrap().len(), 2);
    }

    /// The other half of the same upstream case: `Allow once` approves and caches **nothing**, so
    /// an identical second call prompts again.
    #[tokio::test]
    async fn allow_once_approves_without_caching() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(APPROVE_ONCE_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));

        for _ in 0..2 {
            assert_eq!(
                ensure_tool_call_approved(
                    &state,
                    "demo",
                    &tool,
                    &json!({}),
                    ApprovalOrigin::Proxy,
                    &CancelToken::new(),
                    &metadata,
                )
                .await,
                ApprovalOutcome::Approved
            );
        }
        assert_eq!(ui.prompt_count(), 2);
        assert!(state.approved_tool_calls.lock().unwrap().is_empty());
    }

    /// `"returns approval_denied without throwing"`, plus the fail-closed default: **any** answer
    /// that is not one of the two `Allow …` strings denies — the literal `Deny`, and a label the
    /// dialog never offered.
    #[tokio::test]
    async fn an_unrecognised_answer_denies() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        for answer in [DENY_OPTION, "Allow", "allow once", ""] {
            let ui = ScriptedUi::answering(Some(answer));
            let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(ui));
            assert_eq!(
                ensure_tool_call_approved(
                    &state,
                    "demo",
                    &tool,
                    &json!({}),
                    ApprovalOrigin::Proxy,
                    &CancelToken::new(),
                    &metadata,
                )
                .await,
                ApprovalOutcome::Denied,
                "answer {answer:?} must not approve"
            );
        }
    }

    /// A tool no rule gates is approved with **no dialog at all**, even headless — the cheap path
    /// every non-gated MCP call takes.
    #[tokio::test]
    async fn an_ungated_tool_is_approved_without_asking() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(DENY_OPTION));
        let state = approval_state(demo_config(None), Some(Arc::clone(&ui)));
        assert_eq!(
            ensure_tool_call_approved(
                &state,
                "demo",
                &tool,
                &json!({}),
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
                &metadata,
            )
            .await,
            ApprovalOutcome::Approved
        );
        assert_eq!(ui.prompt_count(), 0);

        let headless = approval_state(demo_config(None), None);
        assert_eq!(
            ensure_tool_call_approved(
                &headless,
                "demo",
                &tool,
                &json!({}),
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
                &metadata,
            )
            .await,
            ApprovalOutcome::Approved
        );
    }

    /// The cancellation delta this port records: a token that is already cancelled denies
    /// **without opening a dialog**, which is `abortable`'s pre-await `throwIfAborted`.
    #[tokio::test]
    async fn a_cancelled_call_denies_without_asking() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(APPROVE_ONCE_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));
        let cancel = CancelToken::new();
        cancel.cancel();

        assert_eq!(
            ensure_tool_call_approved(
                &state,
                "demo",
                &tool,
                &json!({}),
                ApprovalOrigin::Proxy,
                &cancel,
                &metadata,
            )
            .await,
            ApprovalOutcome::Denied
        );
        assert_eq!(ui.prompt_count(), 0);

        // The generation's own token is the other half of `combineAbortSignals(state.owner.signal,
        // signal)`: a stopped generation denies just as an aborted caller does.
        let stopped = approval_state(demo_config(Some(BoolOrList::All(true))), Some(ui));
        let _ = stopped.owner.stop(None).await;
        assert_eq!(
            ensure_tool_call_approved(
                &stopped,
                "demo",
                &tool,
                &json!({}),
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
                &metadata,
            )
            .await,
            ApprovalOutcome::Denied
        );
    }

    /// `tool-approval.ts:177-184 @v2.26.1` — the dialog's exact text and its exact option list.
    #[tokio::test]
    async fn the_dialog_is_the_upstream_title_options_and_argument_block() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(APPROVE_ONCE_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));

        let _ = ensure_tool_call_approved(
            &state,
            "demo",
            &tool,
            &json!({ "query": "private" }),
            ApprovalOrigin::Proxy,
            &CancelToken::new(),
            &metadata,
        )
        .await;

        // The argument block is `sanitizeTerminalText(JSON.stringify(args, null, 2))`, and the
        // sanitiser's `/\s+/g -> " "` tail collapses the pretty-printer's newlines and indent to
        // single spaces. Upstream shows exactly this one-line form; the `null, 2` argument survives
        // only as the spaces between tokens. Reproduced, not "fixed".
        assert_eq!(
            ui.last_prompt(),
            "MCP: demo wants to run search-records\n\nArguments:\n{ \"query\": \"private\" }"
        );
        assert_eq!(
            ui.options.lock().unwrap().last().cloned().unwrap_or_default(),
            vec!["Allow once", "Allow for session", "Deny"]
        );
    }

    /// MCP-235's two interpolations plus the argument block: nothing a hostile server controls can
    /// repaint the dialog it appears in.
    ///
    /// The **names** go through `sanitizeTerminalText` directly. The **arguments** are protected by
    /// two different mechanisms, and the test asserts both, because each covers bytes the other
    /// does not: `JSON.stringify` escapes C0 (an `ESC` becomes the inert literal `\u001b`), and the
    /// sanitiser removes `U+007F` and the C1 block, which `JSON.stringify` emits raw.
    #[tokio::test]
    async fn a_hostile_name_or_argument_cannot_repaint_the_dialog() {
        let tool = ToolMetadata::new("evil", "run\u{1b}[2Jclear", "");
        let metadata = metadata_with(&[("evil\u{7}server", vec![tool.clone()])]);
        let mut config = config_with(&[("evil\u{7}server", stdio("demo"))]);
        if let Some(entry) = config.mcp_servers.get_mut("evil\u{7}server") {
            entry.approve_tools = Some(BoolOrList::All(true));
        }
        let ui = ScriptedUi::answering(Some(DENY_OPTION));
        let state = approval_state(config, Some(Arc::clone(&ui)));

        let _ = ensure_tool_call_approved(
            &state,
            "evil\u{7}server",
            &tool,
            // `c1` carries DEL + a C1 control, which JSON escaping does NOT touch; `c0` carries a
            // real CSI, which JSON escaping renders inert before the sanitiser ever sees it.
            &json!({ "c1": "a\u{7f}\u{85}b", "c0": "x\u{1b}[31my" }),
            ApprovalOrigin::Proxy,
            &CancelToken::new(),
            &metadata,
        )
        .await;

        let prompt = ui.last_prompt();
        assert!(!prompt.contains('\u{1b}'), "no ESC survives: {prompt:?}");
        assert!(!prompt.contains('\u{7}'), "no BEL survives: {prompt:?}");
        assert!(!prompt.contains('\u{7f}'), "no DEL survives: {prompt:?}");
        assert!(!prompt.contains('\u{85}'), "no C1 control survives: {prompt:?}");
        // BEL is a C0 control and collapses to ONE space; `ESC [ 2 J` is a complete CSI and is
        // removed outright, leaving the two halves of the tool name joined.
        assert!(
            prompt.starts_with("MCP: evil server wants to run runclear\n\nArguments:\n"),
            "{prompt:?}"
        );
        // DEL + C1 are one control run and become one space.
        assert!(prompt.contains("\"c1\": \"a b\""), "{prompt:?}");
        // The CSI arrived pre-escaped by the JSON renderer and is shown, inertly, as text.
        assert!(prompt.contains(r#""c0": "x\u001b[31my""#), "{prompt:?}");
    }

    /// `tool-approval.ts:176 @v2.26.1` — over 500 UTF-16 units the preview is cut and gets a literal `...`
    /// tail (three ASCII periods, not `…`). The sanitiser collapses the pretty-printer’s newlines
    /// to single spaces, so the budget is spent on content rather than on indentation.
    #[tokio::test]
    async fn the_argument_preview_is_capped_at_five_hundred_units() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(DENY_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));

        let _ = ensure_tool_call_approved(
            &state,
            "demo",
            &tool,
            &json!({ "blob": "x".repeat(4_000) }),
            ApprovalOrigin::Proxy,
            &CancelToken::new(),
            &metadata,
        )
        .await;

        let prompt = ui.last_prompt();
        let preview = prompt.split("Arguments:\n").nth(1).unwrap_or_default().to_string();
        assert!(preview.ends_with("..."), "the tail is three ASCII periods: {preview:?}");
        assert_eq!(
            preview.trim_end_matches("...").encode_utf16().count(),
            APPROVAL_PREVIEW_LENGTH
        );
    }

    /// MCP-471, end to end through the seam that actually carries it: the ctx a dispatch handed
    /// `McpExtension::on_event` is recorded on the state, and the approval dialog opened later —
    /// from `Tool::execute`, which has no ctx of its own — signals that very gate.
    ///
    /// This is the whole reason `McpState::human_wait_ctx` exists; with the slot unset the guard is
    /// silently never taken and nothing else in the system notices.
    #[tokio::test]
    async fn the_recorded_dispatch_ctx_reaches_the_approval_dialog() {
        let ctx = cyrup_ext::HostCtx::event(
            cyrup_ext::ExtMode::Tui,
            true,
            std::path::PathBuf::from("/workspace"),
        );
        let gate = ctx.human_wait_gate();
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::watching(Some(DENY_OPTION), Arc::clone(&gate));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));

        // Nothing recorded yet — the dialog still opens, it just forgives no budget.
        let _ = ensure_tool_call_approved(
            &state,
            "demo",
            &tool,
            &json!({ "n": 1 }),
            ApprovalOrigin::Proxy,
            &CancelToken::new(),
            &metadata,
        )
        .await;

        // `on_event` records the dispatch ctx…
        state.set_human_wait_ctx(&ctx);
        let _ = ensure_tool_call_approved(
            &state,
            "demo",
            &tool,
            &json!({ "n": 2 }),
            ApprovalOrigin::Proxy,
            &CancelToken::new(),
            &metadata,
        )
        .await;

        assert_eq!(
            *ui.waiting_during_dialog.lock().unwrap(),
            vec![false, true],
            "the second dialog runs under the P-3 guard the recorded ctx supplies"
        );
        assert!(!gate.is_waiting(), "and releases it when the dialog returns");
    }

    /// The [`ProxyCtx`] seam joins the state and the metadata map for both gates — the two-line
    /// body a production [`ProxyEnv`] forwards to, exercised end to end so the bridge cannot rot
    /// while the trait has no production implementor.
    #[tokio::test]
    async fn the_proxy_ctx_bridge_reaches_both_gates() {
        let tool = demo_tool();
        let ui = ScriptedUi::answering(Some(DENY_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));
        let ctx = Arc::new(ProxyCtx::new(Arc::clone(&state), Arc::new(FakeEnv::default())));
        ctx.with_metadata_mut(|metadata| {
            metadata.insert("demo".to_string(), vec![tool.clone()]);
        });

        assert!(ctx.approval_required("demo", &tool));
        assert_eq!(
            ctx.ensure_tool_call_approved(
                "demo",
                &tool,
                &json!({}),
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
            )
            .await,
            ApprovalOutcome::Denied
        );
        assert_eq!(ui.prompt_count(), 1);

        // The same context with no rule gating the tool answers without a dialog.
        let ungated = approval_state(demo_config(None), Some(Arc::clone(&ui)));
        let ungated_ctx = Arc::new(ProxyCtx::new(ungated, Arc::new(FakeEnv::default())));
        ungated_ctx.with_metadata_mut(|metadata| {
            metadata.insert("demo".to_string(), vec![tool.clone()]);
        });
        assert!(!ungated_ctx.approval_required("demo", &tool));
        assert_eq!(
            ungated_ctx
                .ensure_tool_call_approved(
                    "demo",
                    &tool,
                    &json!({}),
                    ApprovalOrigin::Proxy,
                    &CancelToken::new(),
                )
                .await,
            ApprovalOutcome::Approved
        );
        assert_eq!(ui.prompt_count(), 1, "an ungated tool opens no second dialog");
    }

    /// The two origin derivations differ only in their fallback, and that difference is the whole
    /// reason both exist (`proxy-modes.ts:1145` vs `direct-tools.ts:440 @v2.26.1`).
    #[test]
    fn the_two_origin_derivations_differ_only_in_their_fallback() {
        let uri = "docs://handbook".to_string();
        assert_eq!(ApprovalOrigin::for_proxy_call(None), ApprovalOrigin::Proxy);
        assert_eq!(ApprovalOrigin::for_direct_tool(None), ApprovalOrigin::Direct);
        assert_eq!(ApprovalOrigin::for_proxy_call(Some(&uri)), ApprovalOrigin::Resource);
        assert_eq!(ApprovalOrigin::for_direct_tool(Some(&uri)), ApprovalOrigin::Resource);
        assert_eq!(
            [ApprovalOrigin::Proxy.as_str(), ApprovalOrigin::Direct.as_str(), ApprovalOrigin::Resource.as_str()],
            ["proxy", "direct", "resource"]
        );
    }

}
