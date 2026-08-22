//! [`SessionCommand`] deserialization: the snake_case `type` discriminants and camelCase request
//! fields of pi's `rpc-types.ts`, asserted directly on the type with no session or runtime in play.

use crate::SessionCommand;

/// `SessionCommand` deserializes the documented snake_case `type` tags + Pi's camelCase fields
/// (`streamingBehavior`, `images`), 1:1 with `rpc-types.ts:22`.
#[test]
fn session_command_parses_streaming_behavior() {
    // The `id` is NOT a variant field (recovered from the raw line in `dispatch`, mirroring Pi's
    // `const id = command.id`); it is ignored by the command payload deserialization.
    let cmd: SessionCommand = serde_json::from_str(
        r#"{"type":"prompt","id":"7","message":"hi","streamingBehavior":"followUp"}"#,
    )
    .expect("parse prompt");
    match cmd {
        SessionCommand::Prompt { message, images, streaming_behavior } => {
            assert_eq!(message, "hi");
            assert!(images.is_empty());
            assert!(streaming_behavior.is_some());
        }
        other => panic!("expected Prompt, got {other:?}"),
    }
}

/// The camelCase request fields + new command tags deserialize 1:1 with `rpc-types.ts` (the `type`
/// discriminants are snake_case; multi-word fields are camelCase on the wire).
#[test]
fn session_command_parses_new_command_shapes() {
    // `set_model` takes `provider` + `modelId`.
    match serde_json::from_str::<SessionCommand>(
        r#"{"type":"set_model","provider":"anthropic","modelId":"claude"}"#,
    )
    .expect("parse set_model")
    {
        SessionCommand::SetModel { provider, model_id, .. } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(model_id, "claude");
        }
        other => panic!("expected SetModel, got {other:?}"),
    }
    // `fork` takes `entryId`.
    match serde_json::from_str::<SessionCommand>(r#"{"type":"fork","entryId":"e1"}"#)
        .expect("parse fork")
    {
        SessionCommand::Fork { entry_id, .. } => assert_eq!(entry_id, "e1"),
        other => panic!("expected Fork, got {other:?}"),
    }
    // `bash` takes `command` + `excludeFromContext`; `set_steering_mode` the `one-at-a-time` arg.
    serde_json::from_str::<SessionCommand>(
        r#"{"type":"bash","command":"ls","excludeFromContext":true}"#,
    )
    .expect("parse bash");
    serde_json::from_str::<SessionCommand>(r#"{"type":"set_steering_mode","mode":"one-at-a-time"}"#)
        .expect("parse set_steering_mode");
    serde_json::from_str::<SessionCommand>(r#"{"type":"new_session","parentSession":"p.jsonl"}"#)
        .expect("parse new_session");
}
