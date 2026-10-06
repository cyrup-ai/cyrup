//! Helpers the script globals share: how a spread call's arguments arrive and how a global is
//! declared to the sandbox.

use std::future::Future;
use std::sync::Arc;

use cyrup_codemode::types::ToolDeclaration;
use futures::FutureExt as _;
use serde_json::Value;

use crate::types::{CodemodeTool, CodemodeToolContext, ToolResult};

/// The arguments of a `spread: true` global: the script's call arguments, as the JSON array the
/// sandbox hands over (`execute.ts:476`, `args as [...]`). A missing array is an empty call.
pub(super) fn spread(args: Option<Value>) -> Vec<Value> {
    match args {
        Some(Value::Array(values)) => values,
        _ => Vec::new(),
    }
}

/// A global that takes the whole argument list (`{ name, spread: true, execute }`,
/// `execute.ts:469-472`).
pub(super) fn spread_global<F, Fut>(name: &str, run: F) -> CodemodeTool
where
    F: Fn(Vec<Value>, CodemodeToolContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ToolResult> + Send + 'static,
{
    CodemodeTool {
        declaration: ToolDeclaration {
            name: name.to_owned(),
            spread: true,
            ..ToolDeclaration::default()
        },
        execute: Arc::new(move |args, context| run(spread(args), context).boxed()),
    }
}

/// A script value in an error message (`describeValue`, `execute.ts:92-100`): `undefined`, `null`,
/// `a string`, `an array`, or the keys of an object (`{ prompt }`).
///
/// `None` is JavaScript's `undefined`: an absent argument or an absent object member.
pub(super) fn describe_value(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Array(items)) if items.is_empty() => "an empty array".to_owned(),
        Some(Value::Array(_)) => "an array".to_owned(),
        Some(Value::Object(map)) => {
            let keys = cyrup_codemode::js::own_keys(map);
            if keys.is_empty() {
                return "{}".to_owned();
            }
            let shown: Vec<&str> = keys.iter().take(6).copied().collect();
            format!(
                "{{ {}{} }}",
                shown.join(", "),
                if keys.len() > 6 { ", ..." } else { "" }
            )
        }
        Some(Value::String(_)) => "a string".to_owned(),
        Some(Value::Number(_)) => "a number".to_owned(),
        Some(Value::Bool(_)) => "a boolean".to_owned(),
    }
}
