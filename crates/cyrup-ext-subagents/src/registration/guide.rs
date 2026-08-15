//! SUBA-055 — `action: "guide"`: the packaged, version-matched subagents documentation.
//!
//! Port of `pi-subagents/src/extension/subagent-guide.ts` @v0.47.1 (new file at that tag;
//! `git cat-file -e v0.43.0:src/extension/subagent-guide.ts` fails, so this is drift, not a
//! never-ported baseline feature).
//!
//! # Why an orchestrator needs this
//!
//! A model that has drifted from the tool surface — because it was compacted, because it inherited
//! a stale system prompt, or because it is simply guessing — has no in-band way to re-read the
//! contract. Upstream's answer is `{action:"guide", topic:"tool-reference"}`, which returns the
//! documentation that shipped WITH this build. The version-matching is the whole point: docs
//! fetched from a website describe some other release.
//!
//! # [CYRUP-DELTA] — `include_str!` instead of `fs.readFileSync`
//!
//! Upstream resolves `packageRoot` from `import.meta.url` and reads `README.md` / `docs/<topic>.md`
//! off disk at call time (`subagent-guide.ts:20`, `:26-38`), because an npm package ships its
//! `docs/` directory beside its compiled JS. cyrup ships a single static binary with no package
//! root to resolve, so the same files are embedded at COMPILE time from
//! `crates/cyrup-ext-subagents/resources/docs/`.
//!
//! Three consequences, all of them in cyrup's favour and all deliberate:
//!
//! * **Version-matching is structural, not conventional.** Upstream's docs can be edited on disk
//!   after install and drift away from the binary; these cannot. The bytes returned by `guide` are
//!   the bytes that were in the tree when this binary was built, which is the property the feature
//!   exists for.
//! * **`readSubagentGuide` cannot fail.** Upstream throws `Failed to read packaged subagents guide
//!   '<topic>': <io error>` when the file is missing (`:34-36`). An embedded `&'static str` has no
//!   such failure mode, so [`read_subagent_guide`] returns `String`, not `Result`. The unknown-TOPIC
//!   message — which upstream returns as an ordinary value rather than throwing (`:28`) — is ported
//!   byte-identically, because that one IS reachable: it is driven by caller input.
//! * **A missing doc file is a build error, not a runtime error.** `include_str!` fails to compile.
//!
//! The topic list and its order are upstream's (`:5-16`), and
//! [`the_topic_list_is_upstreams_verbatim_and_every_topic_resolves`] pins both the list and the
//! fact that every entry actually resolves to embedded bytes.

/// pi `SUBAGENT_GUIDE_TOPICS` (`extension/subagent-guide.ts:5-16` @v0.47.1), in upstream's order.
///
/// The order is load-bearing twice over: it is what the unknown-topic message joins with `", "`,
/// and `overview` being FIRST is what makes it the natural default a caller who omits `topic` gets.
pub const SUBAGENT_GUIDE_TOPICS: &[&str] = &[
    "overview",
    "workflows",
    "agents",
    "missions",
    "observability",
    "tool-reference",
    "configuration",
    "models",
    "watchdog",
    "extension-api",
];

/// The default topic when the caller omits one — pi's `topic = "overview"` default parameter
/// (`subagent-guide.ts:26`).
pub const DEFAULT_GUIDE_TOPIC: &str = "overview";

/// pi's `overview` special case (`subagent-guide.ts:31-33`): the overview is the package README,
/// every other topic is `docs/<topic>.md`.
const OVERVIEW: &str = include_str!("../../resources/docs/README.md");
const WORKFLOWS: &str = include_str!("../../resources/docs/workflows.md");
const AGENTS: &str = include_str!("../../resources/docs/agents.md");
const MISSIONS: &str = include_str!("../../resources/docs/missions.md");
const OBSERVABILITY: &str = include_str!("../../resources/docs/observability.md");
const TOOL_REFERENCE: &str = include_str!("../../resources/docs/tool-reference.md");
const CONFIGURATION: &str = include_str!("../../resources/docs/configuration.md");
const MODELS: &str = include_str!("../../resources/docs/models.md");
const WATCHDOG: &str = include_str!("../../resources/docs/watchdog.md");
const EXTENSION_API: &str = include_str!("../../resources/docs/extension-api.md");

/// pi `isGuideTopic` (`subagent-guide.ts:22-24`).
#[must_use]
pub fn is_guide_topic(value: &str) -> bool {
    SUBAGENT_GUIDE_TOPICS.contains(&value)
}

/// The embedded bytes for a known topic. `None` for anything not in
/// [`SUBAGENT_GUIDE_TOPICS`] — which is the SAME predicate as [`is_guide_topic`], expressed once
/// here so a topic can never be advertised without a document behind it (the advertise-vs-dispatch
/// invariant this crate applies to verbs, applied to topics).
#[must_use]
fn guide_body(topic: &str) -> Option<&'static str> {
    Some(match topic {
        "overview" => OVERVIEW,
        "workflows" => WORKFLOWS,
        "agents" => AGENTS,
        "missions" => MISSIONS,
        "observability" => OBSERVABILITY,
        "tool-reference" => TOOL_REFERENCE,
        "configuration" => CONFIGURATION,
        "models" => MODELS,
        "watchdog" => WATCHDOG,
        "extension-api" => EXTENSION_API,
        _ => return None,
    })
}

/// pi `readSubagentGuide` (`extension/subagent-guide.ts:26-38` @v0.47.1).
///
/// `topic = None` is upstream's default parameter, i.e. [`DEFAULT_GUIDE_TOPIC`]. An unknown topic
/// returns upstream's message VERBATIM rather than erroring — that is deliberate upstream and it
/// matters: the caller is a model, and a tool ERROR costs it a turn of recovery where a plain
/// string with the valid list in it does not.
#[must_use]
pub fn read_subagent_guide(topic: Option<&str>) -> String {
    let topic = topic.map(str::trim).filter(|t| !t.is_empty()).unwrap_or(DEFAULT_GUIDE_TOPIC);
    match guide_body(topic) {
        Some(body) => body.to_string(),
        None => format!(
            "Unknown subagents guide topic '{topic}'. Valid topics: {}. No files were changed.",
            SUBAGENT_GUIDE_TOPICS.join(", ")
        ),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// Pre-fix this whole module did not exist: `rg '"guide"' crates/cyrup-ext-subagents/src` was
    /// zero-hit, so there was no topic list to compare and no bytes to resolve.
    #[test]
    fn the_topic_list_is_upstreams_verbatim_and_every_topic_resolves() {
        assert_eq!(
            SUBAGENT_GUIDE_TOPICS,
            [
                "overview",
                "workflows",
                "agents",
                "missions",
                "observability",
                "tool-reference",
                "configuration",
                "models",
                "watchdog",
                "extension-api",
            ],
            "pi SUBAGENT_GUIDE_TOPICS (extension/subagent-guide.ts:5-16 @v0.47.1), in order"
        );
        for topic in SUBAGENT_GUIDE_TOPICS {
            let body = guide_body(topic).unwrap_or_default();
            assert!(
                !body.trim().is_empty(),
                "topic {topic} is advertised but resolves to no packaged document"
            );
            assert!(
                is_guide_topic(topic),
                "topic {topic} resolves to bytes but is_guide_topic denies it"
            );
        }
    }

    /// The item's own Verify, first half: `{action:"guide"}` with no topic must return the
    /// overview. Pre-fix there was no function to call.
    #[test]
    fn an_omitted_topic_returns_the_overview() {
        assert_eq!(read_subagent_guide(None), OVERVIEW);
        assert_eq!(read_subagent_guide(Some("")), OVERVIEW);
        assert_eq!(read_subagent_guide(Some("  ")), OVERVIEW);
        assert_eq!(read_subagent_guide(Some("overview")), OVERVIEW);
    }

    /// The item's own Verify, second half: upstream's exact unknown-topic sentence, byte for byte.
    /// Pre-fix the tool answered `unknown subagent action 'guide'` and never reached a topic at all.
    #[test]
    fn an_unknown_topic_returns_upstreams_exact_message() {
        assert_eq!(
            read_subagent_guide(Some("bogus")),
            "Unknown subagents guide topic 'bogus'. Valid topics: overview, workflows, agents, \
             missions, observability, tool-reference, configuration, models, watchdog, \
             extension-api. No files were changed."
        );
    }

    /// The verb is ADVERTISED, in pi's own position. Pre-fix `rg '"guide"'` over the crate was
    /// zero-hit and `{action:"guide"}` landed on the unknown-action arm.
    #[test]
    fn guide_is_advertised_in_pis_own_position() {
        let actions = crate::extension::subagent_actions();
        let at = actions
            .iter()
            .position(|a| *a == "guide")
            .expect("`guide` must be in SUBAGENT_ACTIONS");
        assert_eq!(
            actions.get(at - 1).copied(),
            Some("models"),
            "pi `shared/types.ts:1968` @v0.47.1 orders `… \"models\", \"children.list\", \
             \"guide\", \"create\", …`; cyrup omits the unported `children.list`, so `guide` \
             follows `models` directly"
        );
        assert_eq!(actions.get(at + 1).copied(), Some("create"));
    }

    /// The packaged docs are the ones a model reads to recover the tool surface, so the
    /// tool-reference topic has to actually name the verbs this build dispatches. This is the
    /// version-matching claim made mechanical: if a later change adds a verb to
    /// `SUBAGENT_ACTIONS` without documenting it, this goes red.
    #[test]
    fn the_tool_reference_topic_names_every_dispatched_verb() {
        let body = TOOL_REFERENCE;
        for action in crate::extension::subagent_actions() {
            assert!(
                body.contains(action),
                "packaged tool-reference does not mention the dispatched action {action:?}"
            );
        }
    }
}
