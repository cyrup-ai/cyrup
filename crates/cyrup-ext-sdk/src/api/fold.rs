//! EXT-089 — how SEVERAL handlers one extension registered for the same event combine into the ONE
//! outcome its `on-*` export returns.
//!
//! pi's `on(event, handler)` pushes onto the event's handler list
//! (`core/extensions/loader.ts:256-271` @v0.87.1), and every `emit*` in
//! `core/extensions/runner.ts` @v0.87.1 walks each extension's list in registration order, feeding
//! each handler the value the previous one produced. The host folds ACROSS extensions the same way
//! (`cyrup-ext` `dispatch.rs`), but it sees one outcome per extension, so a guest has to fold its
//! own handlers first. Each arm below names the upstream emitter it follows:
//!
//! | event | upstream | combination |
//! |---|---|---|
//! | `tool_call` | `emitToolCall` `:1134-1152` | the first block returns; the rewritten input chains |
//! | `tool_result` | `emitToolResult` `:1082-1132` | `content`/`details`/`isError`/`usage` chain, key by key |
//! | `context`, `context_with_system` | `emitContext` `:1190-1251` | the returned messages chain |
//! | `message_end` | `emitMessageEnd` `:1043-1080` | a same-role replacement chains; another role is skipped |
//! | `before_agent_start` | `emitBeforeAgentStart` `:1312-1364` | every `message` accumulates; the last `systemPrompt` wins and chains |
//! | `input` | `emitInput` `:1412-1451` | `handled` returns; a transform's text/images chain |
//! | `user_bash` | `emitUserBash` `:1154-1188` | the first result returns |
//! | `before_provider_request` | `emitBeforeProviderRequest` `:1253-1282` | the returned payload chains |
//! | `before_provider_headers` | `emitBeforeProviderHeaders` `:1284-1310` | the header edits chain; `null` deletes |
//! | `resources_discover` | `emitResourcesDiscover` `:1366-1409` | every handler's paths concatenate |
//! | `project_trust` | `emitProjectTrustEvent` `:291-316` | the first `yes`/`no` returns; `undecided` falls through |
//! | `session_before_*` | `emit` `:988-1017` | a cancel (block) returns; the last result wins |
//! | every notify event | `emit` `:988-1017` | all run; nothing to combine |
//!
//! A block and a `handled` end the walk wherever the host's own chain would end it
//! (`dispatch.rs::block_mutate_chain`), so a guest with two handlers reaches the host exactly as
//! two extensions with one handler each would.

use super::{Handler, RawOutcome, kind};
use crate::ctx::Ctx;
use serde_json::{Map, Value};

/// Run `handlers` in order over `args` and combine their outcomes (see the module docs).
pub(super) fn run(kind: u8, args: &[&str], handlers: &[&Handler], ctx: &Ctx) -> RawOutcome {
    // One handler: its own outcome, byte for byte — nothing to combine.
    if let [only] = handlers {
        return only(args, ctx);
    }
    let mut args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
    let mut patch: Option<Value> = None;
    let mut resources: Vec<Value> = Vec::new();
    let mut undecided: Option<String> = None;
    for handler in handlers {
        let view: Vec<&str> = args.iter().map(String::as_str).collect();
        match handler(&view, ctx) {
            RawOutcome::Noop => {}
            RawOutcome::Block(reason, terminate) => return RawOutcome::Block(reason, terminate),
            RawOutcome::Handled(value) => match kind {
                kind::RESOURCES_DISCOVER => {
                    if let Ok(v) = serde_json::from_str(&value) {
                        resources.push(v);
                    }
                }
                kind::PROJECT_TRUST if !trust_decided(&value) => undecided = Some(value),
                _ => return RawOutcome::Handled(value),
            },
            RawOutcome::Mutate(value) => {
                // An unparseable patch is the host's no-op (`decode_outcome`), so it changes
                // nothing here either.
                if let Ok(v) = serde_json::from_str(&value) {
                    chain(kind, &mut args, &mut patch, v);
                }
            }
        }
    }
    if !resources.is_empty() {
        return RawOutcome::Handled(concat_resources(&resources).to_string());
    }
    if let Some(p) = patch {
        return RawOutcome::Mutate(p.to_string());
    }
    undecided.map_or(RawOutcome::Noop, RawOutcome::Handled)
}

/// Replace ordered arg `i` (the handlers' args are the event's fields in WIT parameter order).
fn set(args: &mut [String], i: usize, value: String) {
    if let Some(slot) = args.get_mut(i) {
        *slot = value;
    }
}

/// Insert every key of `from` into the combined object patch, a later key overriding an earlier.
fn merge_keys(patch: &mut Option<Value>, from: &Map<String, Value>) {
    let target = patch.get_or_insert_with(|| Value::Object(Map::new()));
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    if let Some(obj) = target.as_object_mut() {
        for (k, v) in from {
            obj.insert(k.clone(), v.clone());
        }
    }
}

/// Fold one handler's mutate patch `v` into the args the NEXT handler sees and into the combined
/// patch the host receives.
fn chain(kind: u8, args: &mut [String], patch: &mut Option<Value>, v: Value) {
    match kind {
        // `emitToolCall`: handlers edit `event.input` in place, so the next one sees the rewrite.
        kind::TOOL_CALL => {
            set(args, 2, v.to_string());
            *patch = Some(v);
        }
        // `emitContext` (both phases) and `emitBeforeProviderRequest`: the returned value replaces
        // the current one wholesale and is what the next handler is given.
        kind::CONTEXT | kind::CONTEXT_WITH_SYSTEM | kind::BEFORE_PROVIDER_REQUEST => {
            set(args, 0, v.to_string());
            *patch = Some(v);
        }
        // `emitMessageEnd`: a replacement whose `role` differs is reported and SKIPPED
        // (`:1055-1062`); the current message stays for the next handler.
        kind::MESSAGE_END => {
            let current = args
                .first()
                .and_then(|m| serde_json::from_str::<Value>(m).ok())
                .and_then(|m| m.get("role").cloned());
            if v.get("role") != current.as_ref() {
                return;
            }
            set(args, 0, v.to_string());
            *patch = Some(v);
        }
        // `emitToolResult`: each field a handler returns overwrites `currentEvent`'s
        // (`:1093-1108`), so the combined patch is the key-wise merge. Args:
        // `[call_id, name, input, content, is_error, details, usage]`.
        kind::TOOL_RESULT => {
            let Some(obj) = v.as_object() else { return };
            if let Some(c) = obj.get("content") {
                set(args, 3, c.to_string());
            }
            if let Some(e) = obj.get("isError").and_then(Value::as_bool) {
                set(args, 4, e.to_string());
            }
            if let Some(d) = obj.get("details") {
                set(args, 5, d.to_string());
            }
            if let Some(u) = obj.get("usage") {
                set(args, 6, u.to_string());
            }
            merge_keys(patch, obj);
        }
        // `emitInput`: a transform rewrites the text, and the images when it supplies them
        // (`result.images ?? currentImages`, `:1434-1437`). A patch without `text` is not a
        // transform (the host's `decode_patch` drops it). Args: `[text, images, source, behavior]`.
        kind::INPUT => {
            let Some(obj) = v.as_object() else { return };
            let Some(text) = obj.get("text").and_then(Value::as_str) else {
                return;
            };
            set(args, 0, text.to_string());
            if let Some(images) = obj.get("images") {
                set(args, 1, images.to_string());
            }
            merge_keys(patch, obj);
        }
        // `emitBeforeAgentStart`: every handler's `message` is pushed (`:1345`), and a
        // `systemPrompt` becomes the prompt the next handler reads (`:1346-1348`). One outcome
        // carries them all as `messages`, which the host's `decode_patch` reads after `message`.
        // Args: `[prompt, images, system_prompt, options]`.
        kind::BEFORE_AGENT_START => {
            let Some(obj) = v.as_object() else { return };
            let target = patch.get_or_insert_with(|| Value::Object(Map::new()));
            let Some(combined) = target.as_object_mut() else {
                return;
            };
            if let Some(system) = obj.get("systemPrompt").and_then(Value::as_str) {
                set(args, 2, system.to_string());
                combined.insert("systemPrompt".into(), Value::String(system.to_string()));
            }
            let added = obj
                .get("message")
                .into_iter()
                .chain(
                    obj.get("messages")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten(),
                )
                .cloned();
            if let Value::Array(messages) = combined
                .entry("messages")
                .or_insert_with(|| Value::Array(Vec::new()))
            {
                messages.extend(added);
            }
        }
        // `emitBeforeProviderHeaders`: handlers edit ONE `headers` object in place — set a key,
        // delete it with `null` — so the next handler sees the edits, and the combined patch is the
        // key-wise merge (a later `null` still deletes, a later value still sets).
        kind::BEFORE_PROVIDER_HEADERS => {
            let Some(obj) = v.as_object() else { return };
            let mut headers = args
                .first()
                .and_then(|h| serde_json::from_str::<Value>(h).ok())
                .unwrap_or_else(|| Value::Object(Map::new()));
            if let Some(dst) = headers.as_object_mut() {
                for (k, val) in obj {
                    if val.is_null() {
                        dst.remove(k);
                    } else {
                        dst.insert(k.clone(), val.clone());
                    }
                }
            }
            set(args, 0, headers.to_string());
            merge_keys(patch, obj);
        }
        // `emit` for `session_before_compact`/`session_before_tree` (and any other kind with a
        // patch): `result = handlerResult`, the last one wins (`:997-1001`).
        _ => *patch = Some(v),
    }
}

/// pi's `project_trust` tri-state (`ProjectTrustEventDecision`): `yes`/`no` decide, anything else
/// falls through. Mirrors the host's `aggregate::parse_trust_decision`, legacy boolean included.
fn trust_decided(value: &str) -> bool {
    match serde_json::from_str::<Value>(value)
        .ok()
        .as_ref()
        .and_then(|v| v.get("trusted"))
    {
        Some(Value::String(s)) => s == "yes" || s == "no",
        Some(Value::Bool(_)) => true,
        _ => false,
    }
}

/// `emitResourcesDiscover`: every handler's `skillPaths`/`promptPaths`/`themePaths`, concatenated
/// in handler order with no de-duplication (`:1386-1394`).
fn concat_resources(results: &[Value]) -> Value {
    let mut out = Map::new();
    for key in ["skillPaths", "promptPaths", "themePaths"] {
        let paths: Vec<Value> = results
            .iter()
            .filter_map(|r| r.get(key).and_then(Value::as_array))
            .flatten()
            .cloned()
            .collect();
        out.insert(key.into(), Value::Array(paths));
    }
    Value::Object(out)
}
