//! PROV-083a — `utils/transcript.rs` against the two upstream fixtures the row cites,
//! `packages/ai/test/system-message-replay.test.ts` and
//! `packages/ai/test/transcript-tool-changes.test.ts` @v0.87.1.

use crate::context::{Context, ToolDef, ToolReference};
use crate::utils::transcript::{
    collapse_system_messages, create_initial_system_message, declarations_equal,
    get_current_system_message, get_current_system_prompt, get_current_tools, get_declared_tools,
    get_initial_system_message, get_tool_state_changes, has_non_additive_tool_changes,
    has_tool_redefinitions, normalize_context, resolve_transcript, resolve_transcript_tools,
    without_initial_system_message,
};
use cyrup_core::{Content, Message, Sections, SystemMessage};

fn tool(name: &str, params: serde_json::Value) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: format!("{name} description"),
        parameters: params,
        constrained_sampling: None,
    }
}

fn plain(name: &str) -> ToolDef {
    tool(name, serde_json::json!({ "type": "object" }))
}

fn system(msg: SystemMessage) -> Message {
    Message::System(msg)
}

fn user(text: &str, timestamp: i64) -> Message {
    Message::User {
        content: vec![Content::text(text)],
        timestamp,
    }
}

fn delta(
    content: &str,
    sections: Option<Sections>,
    added: Vec<ToolDef>,
    removed: Vec<&str>,
    timestamp: i64,
) -> Message {
    system(SystemMessage {
        content: if content.is_empty() {
            Vec::new()
        } else {
            vec![Content::text(content)]
        },
        sections,
        tools_added: added,
        tools_removed: removed.into_iter().map(ToolReference::new).collect(),
        timestamp,
    })
}

fn names(tools: &[ToolDef]) -> Vec<&str> {
    tools.iter().map(|t| t.name.as_str()).collect()
}

/// `createInitialSystemMessage` returns `undefined` for an empty prompt AND no tools
/// (`transcript.ts:15-17`), so `normalizeContext` leaves an empty context empty (`:32-34`).
#[test]
fn normalize_context_folds_prompt_and_tools_into_a_leading_system_message() {
    let empty = normalize_context(&Context::default());
    assert!(empty.messages().is_empty(), "an empty context stays empty");

    // An empty-STRING prompt is not a prompt: `systemPrompt !== undefined && length > 0` (`:16`).
    let blank = normalize_context(&Context {
        system_prompt: Some(String::new()),
        messages: Vec::new(),
        tools: Vec::new(),
    });
    assert!(blank.messages().is_empty(), "`\"\"` is not a prompt");

    let ctx = Context {
        system_prompt: Some("be helpful".to_string()),
        messages: vec![user("hi", 5)],
        tools: vec![plain("read")],
    };
    let normalized = normalize_context(&ctx);
    assert_eq!(normalized.messages().len(), 2);
    let head = get_initial_system_message(normalized.messages()).expect("leading system message");
    assert_eq!(head.timestamp, 0, "`timestamp: 0` (`transcript.ts:21`)");
    assert_eq!(names(&head.tools_added), vec!["read"]);
    assert_eq!(
        crate::utils::text::get_system_message_text(head),
        "be helpful"
    );
    // The rest of the transcript is untouched, and dropping the head recovers it.
    assert_eq!(
        without_initial_system_message(normalized.messages()),
        &[user("hi", 5)]
    );

    // Tools alone still produce the head, with an EMPTY prompt (`content: systemPrompt ?? ""`).
    let tools_only = normalize_context(&Context {
        system_prompt: None,
        messages: Vec::new(),
        tools: vec![plain("read")],
    });
    let head = get_initial_system_message(tools_only.messages()).expect("leading system message");
    assert!(head.content.is_empty());
    assert_eq!(names(&head.tools_added), vec!["read"]);
    // And `createInitialSystemMessage` agrees with what `normalizeContext` built.
    assert_eq!(
        create_initial_system_message(None, &[plain("read")]).as_ref(),
        Some(head)
    );
}

/// `getCurrentTools` (`transcript.ts:62-70`): remove-then-add PER MESSAGE, insertion-ordered.
#[test]
fn get_current_tools_replays_removals_then_additions_in_insertion_order() {
    let msgs = vec![
        delta("", None, vec![plain("A"), plain("B")], vec![], 0),
        delta("", None, vec![], vec!["A"], 1),
        delta("", None, vec![plain("C")], vec![], 2),
    ];
    assert_eq!(names(&get_current_tools(&msgs)), vec!["B", "C"]);

    // A RE-added name keeps its original position — `Map.set` on an existing key
    // (`transcript.ts:67`). Were the accumulator a plain push-list, `A` would move to the end.
    let readded = vec![
        delta("", None, vec![plain("A"), plain("B")], vec![], 0),
        delta(
            "",
            None,
            vec![tool("A", serde_json::json!({"type":"string"}))],
            vec![],
            1,
        ),
    ];
    assert_eq!(names(&get_current_tools(&readded)), vec!["A", "B"]);

    // Remove-THEN-add inside ONE message: the name survives, with the new definition.
    let churn = vec![
        delta("", None, vec![plain("A")], vec![], 0),
        system(SystemMessage {
            content: Vec::new(),
            sections: None,
            tools_added: vec![tool("A", serde_json::json!({"type":"string"}))],
            tools_removed: vec![ToolReference::new("A")],
            timestamp: 1,
        }),
    ];
    let current = get_current_tools(&churn);
    assert_eq!(names(&current), vec!["A"]);
    assert_eq!(current[0].parameters, serde_json::json!({"type":"string"}));

    // Non-system messages are transparent (`isSystemMessage` guard, `:64`).
    let interleaved = vec![
        delta("", None, vec![plain("A")], vec![], 0),
        user("hi", 1),
        delta("", None, vec![], vec!["A"], 2),
    ];
    assert!(get_current_tools(&interleaved).is_empty());
}

/// `getCurrentSystemMessage` (`transcript.ts:77-101`): content joined `"\n\n"`, sections patched by
/// name, `null` deletes, the FIRST timestamp wins, `None` with neither a timestamp nor a tool.
#[test]
fn get_current_system_message_replays_content_sections_and_timestamp() {
    assert!(
        get_current_system_message(&[user("hi", 9)]).is_none(),
        "no system message and no tools ⇒ None (`transcript.ts:94`)"
    );

    let base_sections = Sections::from_iter([
        ("tone".to_string(), Some("terse".to_string())),
        ("scope".to_string(), Some("repo".to_string())),
    ]);
    let msgs = vec![
        delta(
            "base prompt",
            Some(base_sections),
            vec![plain("read")],
            vec![],
            7,
        ),
        user("hi", 8),
        delta(
            "extra instruction",
            Some(Sections::from_iter([
                // Patch by name — `scope` keeps its POSITION and takes the new value.
                ("scope".to_string(), Some("workspace".to_string())),
                // `null` DELETES.
                ("tone".to_string(), None),
                ("style".to_string(), Some("plain".to_string())),
            ])),
            vec![],
            vec![],
            99,
        ),
    ];
    let head = get_current_system_message(&msgs).expect("a replayed head");
    assert_eq!(
        head.timestamp, 7,
        "`timestamp ??= message.timestamp` — the FIRST wins (`transcript.ts:81`)"
    );
    assert_eq!(
        crate::utils::text::content_text_default(&head.content),
        "base prompt\n\nextra instruction",
        "later content is APPENDED, joined `\\n\\n` (`transcript.ts:97`)"
    );
    let sections = head.sections.as_ref().expect("sections survive");
    assert_eq!(
        sections.iter().collect::<Vec<_>>(),
        vec![("scope", Some("workspace")), ("style", Some("plain"))],
        "`tone` deleted, `scope` patched IN PLACE, `style` appended — insertion order, not sorted"
    );
    assert_eq!(names(&head.tools_added), vec!["read"]);

    // `getCurrentSystemPrompt` renders that head: content, then the section VALUES, `"\n\n"`-joined
    // (`utils/text.ts:15-21`).
    assert_eq!(
        get_current_system_prompt(&msgs),
        "base prompt\n\nextra instruction\n\nworkspace\n\nplain"
    );

    // A transcript with NO system message but a surviving tool still yields a head at `timestamp: 0`.
    // (Unreachable through `normalizeContext`, but `getCurrentTools` is what decides, not the role.)
    assert_eq!(get_current_system_prompt(&[user("hi", 1)]), "");
}

/// `collapseSystemMessages` (`transcript.ts:114-118`) and `resolveTranscript` (`:121-126`).
#[test]
fn collapse_and_resolve_fold_later_system_messages_only_when_unsupported() {
    let ctx = normalize_context(&Context {
        system_prompt: Some("base".to_string()),
        messages: vec![
            user("hi", 1),
            delta("more", None, vec![plain("late")], vec![], 2),
            user("again", 3),
        ],
        tools: vec![plain("read")],
    });
    assert_eq!(ctx.messages().len(), 4);

    let collapsed = collapse_system_messages(&ctx);
    assert_eq!(
        collapsed.messages().len(),
        3,
        "the later head is folded away"
    );
    let head = get_initial_system_message(collapsed.messages()).expect("leading head");
    assert_eq!(
        crate::utils::text::content_text_default(&head.content),
        "base\n\nmore"
    );
    assert_eq!(names(&head.tools_added), vec!["read", "late"]);
    assert!(
        matches!(collapsed.messages().get(1), Some(Message::User { .. })),
        "every later system message is dropped"
    );

    // `supportsMidConvoSystemMessages` truthy ⇒ the transcript is returned untouched.
    assert_eq!(resolve_transcript(&ctx, Some(true)), ctx);
    // Falsy AND absent both collapse — `supportsMidConvoSystemMessages ? context : collapse(...)`.
    assert_eq!(resolve_transcript(&ctx, Some(false)), collapsed);
    assert_eq!(resolve_transcript(&ctx, None), collapsed);
}

/// `declarationsEqual` (`transcript.ts:148-150`) and `getToolStateChanges` (`:158-175`).
#[test]
fn declarations_equal_compares_the_declared_interface_only() {
    let a = tool("t", serde_json::json!({ "type": "object", "a": 1, "b": 2 }));
    // Key ORDER in `parameters` is not representable in cyrup's JSON model (`serde_json::Map` is a
    // `BTreeMap`), which is exactly why upstream's canonical-serialization trick is unnecessary here.
    let reordered = tool("t", serde_json::json!({ "b": 2, "type": "object", "a": 1 }));
    assert!(declarations_equal(&a, &reordered));

    // An ABSENT `constrainedSampling` on both sides compares equal
    // (`...(tool.constrainedSampling === undefined ? {} : {...})`, `:135`).
    assert!(declarations_equal(&plain("t"), &plain("t")));

    // A changed `parameters` is a DIFFERENT declaration.
    let changed = tool("t", serde_json::json!({ "type": "object", "a": 2, "b": 2 }));
    assert!(!declarations_equal(&a, &changed));
    // So is a changed description.
    let mut described = a.clone();
    described.description = "other".to_string();
    assert!(!declarations_equal(&a, &described));

    // A changed definition is a REMOVAL followed by an ADDITION — it appears in both lists (`:159`).
    let changes =
        get_tool_state_changes(&[a.clone(), plain("gone")], std::slice::from_ref(&changed));
    assert_eq!(names(&changes.tools_added), vec!["t"]);
    assert_eq!(
        changes
            .tools_removed
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        vec!["t", "gone"]
    );
    // An unchanged tool appears in NEITHER list.
    let none = get_tool_state_changes(std::slice::from_ref(&a), std::slice::from_ref(&a));
    assert!(none.tools_added.is_empty() && none.tools_removed.is_empty());
}

/// `hasNonAdditiveToolChanges` (`transcript.ts:206-217`), `hasToolRedefinitions` (`:192-203`) and
/// `getDeclaredTools` (`:178-186`).
#[test]
fn non_additive_tool_changes_trip_on_a_removal_or_any_repeat() {
    let pure = vec![
        delta("", None, vec![plain("A")], vec![], 0),
        delta("", None, vec![plain("B")], vec![], 1),
    ];
    assert!(!has_non_additive_tool_changes(&pure));
    assert!(!has_tool_redefinitions(&pure));

    let removed = vec![
        delta("", None, vec![plain("A")], vec![], 0),
        delta("", None, vec![], vec!["A"], 1),
    ];
    assert!(has_non_additive_tool_changes(&removed));
    assert!(
        !has_tool_redefinitions(&removed),
        "a removal is not a REDEFINITION"
    );

    // A same-name re-declaration with a DIFFERENT schema trips both.
    let redefined = vec![
        delta("", None, vec![plain("A")], vec![], 0),
        delta(
            "",
            None,
            vec![tool("A", serde_json::json!({"type":"string"}))],
            vec![],
            1,
        ),
    ];
    assert!(has_non_additive_tool_changes(&redefined));
    assert!(has_tool_redefinitions(&redefined));

    // An IDENTICAL re-declaration trips `hasNonAdditiveToolChanges` (any repeated name does, `:213`)
    // but NOT `hasToolRedefinitions` (`declarationsEqual` short-circuits it, `:198`). That asymmetry
    // is upstream's and is the reason the two functions both exist.
    let identical = vec![
        delta("", None, vec![plain("A")], vec![], 0),
        delta("", None, vec![plain("A")], vec![], 1),
    ];
    assert!(has_non_additive_tool_changes(&identical));
    assert!(!has_tool_redefinitions(&identical));

    // `getDeclaredTools` ignores removals and keeps FIRST-declaration order (`:178`).
    assert_eq!(names(&get_declared_tools(&removed)), vec!["A"]);
    assert_eq!(
        names(&get_declared_tools(&redefined)),
        vec!["A"],
        "a re-declaration replaces the definition in place"
    );
    assert_eq!(
        get_declared_tools(&redefined)[0].parameters,
        serde_json::json!({"type":"string"})
    );
}

/// `resolveTranscriptTools` (`transcript.ts:236-244`).
#[test]
fn resolve_transcript_tools_anchors_only_a_purely_additive_transcript() {
    let additive = vec![
        delta("base", None, vec![plain("A")], vec![], 0),
        user("hi", 1),
        delta("", None, vec![plain("B")], vec![], 2),
    ];
    let anchored = resolve_transcript_tools(&additive, true);
    assert!(anchored.anchors_additions);
    assert_eq!(
        names(&anchored.request_tools),
        vec!["A"],
        "ONLY the leading message's tools go top-level; `B` loads where it appears"
    );

    // `supportsToolAdditions` false ⇒ the complete current set, top-level.
    let unanchored = resolve_transcript_tools(&additive, false);
    assert!(!unanchored.anchors_additions);
    assert_eq!(names(&unanchored.request_tools), vec!["A", "B"]);

    // Once a tool is REMOVED, anchoring is off even with support, and the fallback is
    // `getCurrentTools` — so the removed tool is gone from the request entirely.
    let with_removal = vec![
        delta("base", None, vec![plain("A"), plain("B")], vec![], 0),
        delta("", None, vec![], vec!["A"], 1),
    ];
    let fallen_back = resolve_transcript_tools(&with_removal, true);
    assert!(!fallen_back.anchors_additions);
    assert_eq!(names(&fallen_back.request_tools), vec!["B"]);

    // No leading system message at all ⇒ `?? []` (`:241`).
    let headless = resolve_transcript_tools(&[user("hi", 0)], true);
    assert!(headless.anchors_additions && headless.request_tools.is_empty());
}
