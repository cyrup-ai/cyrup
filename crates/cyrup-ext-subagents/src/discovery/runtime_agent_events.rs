//! SUBA-143 — the runtime-agent registration EVENT bridge: a port of pi
//! `src/agents/runtime-agent-events.ts` (70 lines @v0.71.0, introduced at v0.64.0), re-exported
//! upstream as public API from `src/api/agents.ts:3-10` and installed from
//! `src/extension/index.ts:894` (`registerRuntimeAgentEventListener(pi)`, first entry of
//! `eventUnsubscribes`).
//!
//! Without it, a runtime agent can be contributed only by code that HOLDS the owning runtime — the
//! native `SubagentExecutor::register_agent` seam. Upstream's whole point is that a SIBLING
//! extension, which has the bus and nothing else, can contribute one too. This module is that
//! second door, and it opens onto the same [`RuntimeAgentRegistry`] the native door does.
//!
//! # Upstream's contract, and the one thing about it that cannot survive a bus
//!
//! Upstream emits `pi-subagents:runtime-agent-register:v1` with a MUTABLE request object
//! (`{ version: 1, name, definition, result? }`, `runtime-agent-events.ts:11-16`). pi's
//! `EventEmitter` runs listeners synchronously inside `emit`, so the owner's listener writes
//! `request.result = { ok, registration } | { ok: false, error }` into that same object
//! (`:53-70`) and `registerAgentViaEvents` reads it back off its own local the instant `emit`
//! returns (`:34-46`).
//!
//! cyrup's [`cyrup_ext::bus::SharedBus`] QUEUES emits and fans out by VALUE (`bus.rs:80-93`, with
//! the CYRUP-DELTA stated there: a WASM guest emitting from inside its own `bus.emit` import
//! already holds its store, so delivering into it would re-enter a borrowed store). A payload the
//! listener mutates is therefore not a channel back to the emitter, and no amount of Rust changes
//! that — it is a property of the host, not of the language.
//!
//! So the RESULT travels the way every other cyrup request/response on this bus travels: on a
//! reply topic keyed by a caller-chosen `requestId`, exactly as the subagent RPC bridge does
//! (`crate::extension::rpc`, whose module doc is the client contract this one follows). Four
//! topics, two pairs:
//!
//! | topic | direction | payload |
//! |---|---|---|
//! | [`RUNTIME_AGENT_REGISTER_EVENT`] | client → subagents | `{version: 1, requestId, name, definition}` |
//! | `pi-subagents:runtime-agent-register:v1:reply:<requestId>` | subagents → client | `{version: 1, requestId, ok: true, registration: {registrationId}}` \| `{version: 1, requestId, ok: false, error: {message}}` |
//! | [`RUNTIME_AGENT_DISPOSE_EVENT`] | client → subagents | `{version: 1, requestId, registrationId}` |
//! | `pi-subagents:runtime-agent-dispose:v1:reply:<requestId>` | subagents → client | `{version: 1, requestId, ok: true}` \| `{version: 1, requestId, ok: false, error: {message}}` |
//!
//! **Subscribe to the FULL reply topic before you emit** — `SharedBus` has no prefix matching.
//! [`runtime_agent_register_reply_event`] and [`runtime_agent_dispose_reply_event`] spell them, and
//! [`runtime_agent_register_request`] / [`runtime_agent_dispose_request`] build the envelopes, so a
//! client never hand-writes one. [`read_registration_reply`] and [`read_disposal_reply`] are the
//! reading half of upstream's `registerAgentViaEvents` (`:34-46`), verbatim refusals included.
//!
//! The reply lands INSIDE the same `deliver_bus_events(..)` call that carried the request
//! (`BusFanout::drain_bus` re-checks the queue each round), so a client round-trips with no pump
//! and nothing to sleep on — see [`crate::extension::rpc`]'s "Delivery is deferred, but not slow".
//!
//! # `[CYRUP-DELTA, mechanism]` — the registration HANDLE is an opaque token, not an object
//!
//! Upstream's success result carries a `RuntimeAgentRegistration` whose `dispose()` the caller
//! invokes directly (`runtime-agent-registry.ts:386-397`). A function cannot cross a JSON bus, so
//! the reply carries a `registrationId` — an unguessable v4 token minted per registration — and
//! [`RUNTIME_AGENT_DISPOSE_EVENT`] is the call. The BEHAVIOUR is upstream's, point for point:
//!
//! * the token names exactly one record, as upstream's object identity does
//!   (`entry !== record`, `:393`), so a later registration re-using a disposed NAME is never
//!   removed by an older token;
//! * disposal is idempotent and cannot fail — upstream's `dispose(): void` has no throw and its
//!   `if (disposed) return;` (`:389`) makes the second call a no-op. A token this bridge already
//!   disposed, one it never minted, and one cleared by [`RuntimeAgentEventBridge::clear`] are
//!   therefore all answered `{ok: true}`: every one of them is upstream's no-op;
//! * a token is minted from `Uuid::new_v4`, not from a counter, so one client cannot dispose
//!   another's registration by guessing — which is the capability upstream's object reference
//!   carries for free.
//!
//! Disposed tokens are REMOVED from the map rather than tombstoned, so a client looping
//! register/dispose cannot grow this bridge without bound.
//!
//! # One owner, so upstream's "first handler wins" has nothing to arbitrate
//!
//! `docs/extension-api.md:198` @v0.71.0: *"If more than one owner listens, the first handler that
//! writes `request.result` wins."* Upstream needs that rule because a Pi process can load the
//! pi-subagents npm package more than once. cyrup cannot: `cyrup-ext-subagents` is a compiled-in
//! native the host registers under one fixed extension id (`extension/host/mod.rs`'s
//! `EXTENSION_ID`), and
//! `SharedBus::subscribe` is idempotent per `(owner, topic)` (`bus.rs:46-53`), so exactly one
//! handler ever answers and exactly one reply is ever emitted per request. The arbitration rule is
//! therefore not ported — there is nothing for it to arbitrate — rather than ported as dead code.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Map, Value};

use super::runtime_registry::{
    RuntimeAgentRegistration, RuntimeAgentRegistry, validate_agent_name,
};

/// pi `RUNTIME_AGENT_REGISTER_EVENT` (`runtime-agent-events.ts:4`) — the request topic, spelled
/// exactly as upstream spells it so a client written against pi's constant names cyrup's topic.
pub const RUNTIME_AGENT_REGISTER_EVENT: &str = "pi-subagents:runtime-agent-register:v1";

/// pi `RUNTIME_AGENT_REGISTER_VERSION` (`runtime-agent-events.ts:5`).
pub const RUNTIME_AGENT_REGISTER_VERSION: u32 = 1;

/// The register reply-topic prefix. Never subscribe to this — see the module doc; subscribe to
/// [`runtime_agent_register_reply_event`]'s full topic.
pub const RUNTIME_AGENT_REGISTER_REPLY_EVENT_PREFIX: &str =
    "pi-subagents:runtime-agent-register:v1:reply:";

/// The disposal request topic — the bus call that stands in for upstream's
/// `RuntimeAgentRegistration.dispose()` (`runtime-agent-registry.ts:386-397`). See the module
/// doc's `[CYRUP-DELTA, mechanism]`.
pub const RUNTIME_AGENT_DISPOSE_EVENT: &str = "pi-subagents:runtime-agent-dispose:v1";

/// The disposal reply-topic prefix. As with the register prefix, subscribe to the FULL topic
/// [`runtime_agent_dispose_reply_event`] spells.
pub const RUNTIME_AGENT_DISPOSE_REPLY_EVENT_PREFIX: &str =
    "pi-subagents:runtime-agent-dispose:v1:reply:";

/// pi's "no result came back" refusal (`runtime-agent-events.ts:37`), verbatim. Upstream reaches
/// it when `emit` returned with `request.result` still `undefined`; a cyrup client reaches it when
/// no reply arrived on its reply topic. Same condition, same sentence.
pub const RUNTIME_AGENT_BRIDGE_ABSENT_MESSAGE: &str = "pi-subagents is not installed, not ready, or does not support runtime agent event registration.";

/// pi's malformed-result refusal (`runtime-agent-events.ts:46`), verbatim.
pub const RUNTIME_AGENT_MALFORMED_RESULT_MESSAGE: &str =
    "pi-subagents returned a malformed runtime agent registration result.";

/// The full reply topic for one register request id.
#[must_use]
pub fn runtime_agent_register_reply_event(request_id: &str) -> String {
    format!("{RUNTIME_AGENT_REGISTER_REPLY_EVENT_PREFIX}{request_id}")
}

/// The full reply topic for one disposal request id.
#[must_use]
pub fn runtime_agent_dispose_reply_event(request_id: &str) -> String {
    format!("{RUNTIME_AGENT_DISPOSE_REPLY_EVENT_PREFIX}{request_id}")
}

/// The emitting half of pi `registerAgentViaEvents` (`runtime-agent-events.ts:29-34`): build the
/// request envelope. `definition` is the UNTYPED definition object upstream's `validateDefinition`
/// sees, which is what a JSON-speaking sibling extension has in hand.
#[must_use]
pub fn runtime_agent_register_request(request_id: &str, name: &str, definition: &Value) -> Value {
    serde_json::json!({
        "version": RUNTIME_AGENT_REGISTER_VERSION,
        "requestId": request_id,
        "name": name,
        "definition": definition,
    })
}

/// The disposal request envelope — the bus form of `registration.dispose()`.
#[must_use]
pub fn runtime_agent_dispose_request(request_id: &str, registration_id: &str) -> Value {
    serde_json::json!({
        "version": RUNTIME_AGENT_REGISTER_VERSION,
        "requestId": request_id,
        "registrationId": registration_id,
    })
}

/// The reading half of pi `registerAgentViaEvents` (`runtime-agent-events.ts:35-46`), against the
/// reply this bridge emits.
///
/// # Errors
///
/// * [`RUNTIME_AGENT_BRIDGE_ABSENT_MESSAGE`] when `reply` is `None` — upstream's `result ===
///   undefined` branch (`:36-38`).
/// * The registry's own refusal, verbatim, for an `{ok: false, error}` reply — upstream throws
///   `candidate.error` (`:44`).
/// * [`RUNTIME_AGENT_MALFORMED_RESULT_MESSAGE`] for anything else (`:46`).
///
/// On success the `registrationId` is returned: the token
/// [`runtime_agent_dispose_request`] takes.
pub fn read_registration_reply(reply: Option<&Value>) -> Result<String, String> {
    let Some(reply) = reply else {
        return Err(RUNTIME_AGENT_BRIDGE_ABSENT_MESSAGE.to_string());
    };
    // `:40-43` — `ok === true` AND a registration object carrying the disposal capability.
    if reply.get("ok") == Some(&Value::Bool(true))
        && let Some(id) = reply
            .get("registration")
            .and_then(|registration| registration.get("registrationId"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
    {
        return Ok(id.to_string());
    }
    // `:45` — `ok === false` AND an Error instance. On the wire, the Error is its message.
    if reply.get("ok") == Some(&Value::Bool(false))
        && let Some(message) = reply
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
    {
        return Err(message.to_string());
    }
    Err(RUNTIME_AGENT_MALFORMED_RESULT_MESSAGE.to_string())
}

/// [`read_registration_reply`]'s counterpart for a disposal reply: `Ok(())` for `{ok: true}`, the
/// carried message for `{ok: false, error}`, and upstream's two refusals otherwise.
///
/// # Errors
///
/// As [`read_registration_reply`].
pub fn read_disposal_reply(reply: Option<&Value>) -> Result<(), String> {
    let Some(reply) = reply else {
        return Err(RUNTIME_AGENT_BRIDGE_ABSENT_MESSAGE.to_string());
    };
    if reply.get("ok") == Some(&Value::Bool(true)) {
        return Ok(());
    }
    if reply.get("ok") == Some(&Value::Bool(false))
        && let Some(message) = reply
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
    {
        return Err(message.to_string());
    }
    Err(RUNTIME_AGENT_MALFORMED_RESULT_MESSAGE.to_string())
}

/// JS `String(value)` for the values a JSON payload can hold — the interpolation in upstream's
/// version refusal, `Unsupported runtime agent registration event version '${String(request.version)}'.`
/// (`runtime-agent-events.ts:57`). An ABSENT key is `undefined`, a string interpolates BARE (no
/// quotes), an array joins on `,` with null/undefined rendering empty, and any other object is
/// `[object Object]`.
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<String>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
    }
}

/// Which of the two request topics is being answered. The two differ only in the noun their
/// envelope refusals use and in what they do once parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestKind {
    Register,
    Dispose,
}

impl RequestKind {
    /// The noun upstream's version sentence carries. `registration` is upstream's own word
    /// (`runtime-agent-events.ts:57`); `disposal` is its counterpart on the topic upstream does
    /// not have.
    fn noun(self) -> &'static str {
        match self {
            Self::Register => "registration",
            Self::Dispose => "disposal",
        }
    }

    fn reply_topic(self, request_id: &str) -> String {
        match self {
            Self::Register => runtime_agent_register_reply_event(request_id),
            Self::Dispose => runtime_agent_dispose_reply_event(request_id),
        }
    }
}

/// The `requestId` fallback for a request too malformed to name its own reply topic — the subagent
/// RPC bridge's `safeReplyRequestId` (`rpc.ts:789-795`) convention, so a client that got the
/// envelope wrong still gets an answer instead of silence.
const UNKNOWN_REQUEST_ID: &str = "unknown";

/// The same three tests the RPC bridge's `assertRequestId` applies (`rpc.ts:342-347`): non-empty
/// after trim, and NO `\r`/`\n`.
///
/// The newline rule is a SECURITY property, not tidiness — the value is pasted straight into a bus
/// topic, and a newline would let a caller name a reply topic it was never given.
fn usable_request_id(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty() && !id.contains(['\r', '\n']))
        .map(str::to_string)
}

fn ok_reply(request_id: &str, extra: Option<(&str, Value)>) -> Value {
    let mut reply = Map::new();
    reply.insert(
        "version".to_string(),
        Value::from(RUNTIME_AGENT_REGISTER_VERSION),
    );
    reply.insert("requestId".to_string(), Value::from(request_id));
    reply.insert("ok".to_string(), Value::Bool(true));
    if let Some((key, value)) = extra {
        reply.insert(key.to_string(), value);
    }
    Value::Object(reply)
}

fn error_reply(request_id: &str, message: &str) -> Value {
    serde_json::json!({
        "version": RUNTIME_AGENT_REGISTER_VERSION,
        "requestId": request_id,
        "ok": false,
        "error": { "message": message },
    })
}

/// The installed listener pi's `registerRuntimeAgentEventListener` returns
/// (`runtime-agent-events.ts:49-70`), plus the token→handle map upstream does not need because its
/// handle is an object it hands straight to the caller.
///
/// The map is behind a [`Mutex`] rather than `&mut`-threaded for the RPC bridge's reason: bus
/// delivery is a `&self` trait method (`cyrup_ext::native::NativeExtension::on_bus_event`), and
/// upstream's single-threaded closure capture has no direct analogue. The lock is never held
/// across an `await` — [`RuntimeAgentEventBridge::dispatch`] is entirely synchronous.
#[derive(Debug, Default)]
pub struct RuntimeAgentEventBridge {
    handles: Mutex<HashMap<String, RuntimeAgentRegistration>>,
}

impl RuntimeAgentEventBridge {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn handles(&self) -> std::sync::MutexGuard<'_, HashMap<String, RuntimeAgentRegistration>> {
        self.handles.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The listener body (`runtime-agent-events.ts:50-69`): decide whether this topic is ours,
    /// parse, act, reply. Returns the reply TOPIC and the reply ENVELOPE so the caller only has to
    /// emit, exactly as `SubagentRpcBridge::dispatch` does.
    ///
    /// `None` means "not one of my topics" — upstream's listener is registered per channel and is
    /// never called for another one.
    ///
    /// There is no error path out of this function by construction: every fault becomes an
    /// `{ok: false}` reply, which is upstream's `catch` (`:66-68`) writing `{ok: false, error}`.
    pub fn dispatch(
        &self,
        topic: &str,
        raw: &Value,
        registry: &Arc<RuntimeAgentRegistry>,
    ) -> Option<(String, Value)> {
        let kind = match topic {
            RUNTIME_AGENT_REGISTER_EVENT => RequestKind::Register,
            RUNTIME_AGENT_DISPOSE_EVENT => RequestKind::Dispose,
            _ => return None,
        };
        Some(self.answer(kind, raw, registry))
    }

    fn answer(
        &self,
        kind: RequestKind,
        raw: &Value,
        registry: &Arc<RuntimeAgentRegistry>,
    ) -> (String, Value) {
        // Upstream's `:51` — a non-object payload is not a request. It cannot name a reply topic
        // either, so it is answered on the `unknown` topic rather than in silence.
        let Some(record) = raw.as_object() else {
            return (
                kind.reply_topic(UNKNOWN_REQUEST_ID),
                error_reply(
                    UNKNOWN_REQUEST_ID,
                    &format!("Runtime agent {} request must be an object.", kind.noun()),
                ),
            );
        };
        // FIRST, before anything else is even looked at: the reply topic is built out of this.
        let Some(request_id) = usable_request_id(record.get("requestId")) else {
            return (
                kind.reply_topic(UNKNOWN_REQUEST_ID),
                error_reply(
                    UNKNOWN_REQUEST_ID,
                    "Runtime agent event requestId must be a non-empty string without newlines.",
                ),
            );
        };
        let reply_topic = kind.reply_topic(&request_id);
        // Upstream `:55-57`, inside the try — so a version mismatch is ANSWERED, and
        // `registerAgentViaEvents` throws it at the caller.
        let version = record.get("version");
        if version.and_then(Value::as_u64) != Some(u64::from(RUNTIME_AGENT_REGISTER_VERSION)) {
            return (
                reply_topic,
                error_reply(
                    &request_id,
                    &format!(
                        "Unsupported runtime agent {} event version '{}'.",
                        kind.noun(),
                        js_string(version)
                    ),
                ),
            );
        }
        let reply = match kind {
            RequestKind::Register => self.register(&request_id, record, registry),
            RequestKind::Dispose => self.dispose(&request_id, record),
        };
        (reply_topic, reply)
    }

    /// Upstream `:58-65`: validate the name, hand the UNTYPED definition to the registry, and
    /// answer with the handle or the refusal.
    fn register(
        &self,
        request_id: &str,
        record: &Map<String, Value>,
        registry: &Arc<RuntimeAgentRegistry>,
    ) -> Value {
        // `registerRuntimeAgent({ name: request.name as string, … })` — an absent or non-string
        // `name` reaches `validateString` upstream and produces ITS sentence, not a bridge one.
        let name = match validate_agent_name(record.get("name")) {
            Ok(name) => name,
            Err(error) => return error_reply(request_id, &error.to_string()),
        };
        // An absent `definition` is `undefined` upstream, which `validateDefinition` refuses with
        // "Runtime agent definition must be an object." `Value::Null` takes the same branch.
        let definition = record.get("definition").cloned().unwrap_or(Value::Null);
        match registry.register_value(&name, &definition) {
            Ok(registration) => {
                let registration_id = uuid::Uuid::new_v4().as_simple().to_string();
                self.handles().insert(registration_id.clone(), registration);
                tracing::debug!(
                    target: "cyrup_ext_subagents::runtime_agent_events",
                    request_id,
                    agent = %name,
                    registration_id = %registration_id,
                    "runtime agent registered over the inter-extension bus"
                );
                ok_reply(
                    request_id,
                    Some((
                        "registration",
                        serde_json::json!({ "registrationId": registration_id }),
                    )),
                )
            }
            Err(error) => error_reply(request_id, &error.to_string()),
        }
    }

    /// The bus form of `RuntimeAgentRegistration.dispose()` (`runtime-agent-registry.ts:386-397`).
    /// Upstream's `dispose(): void` cannot fail and is idempotent, so the only refusal here is an
    /// unusable `registrationId` — a shape error in the envelope, which upstream's typed call
    /// could not express in the first place.
    fn dispose(&self, request_id: &str, record: &Map<String, Value>) -> Value {
        let Some(registration_id) = record
            .get("registrationId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return error_reply(
                request_id,
                "Runtime agent disposal registrationId must be a non-empty string.",
            );
        };
        // Removed, not tombstoned. A token that is gone — already disposed, never minted, or
        // cleared at session teardown — is upstream's `if (disposed) return;` no-op.
        if let Some(registration) = self.handles().remove(registration_id) {
            registration.dispose();
        }
        ok_reply(request_id, None)
    }

    /// Drop every outstanding handle WITHOUT disposing, for session teardown: pi
    /// `clearRuntimeAgentsForPi(pi)` (`extension/index.ts:1042`) empties the whole partition at
    /// once, and `RuntimeAgentRegistry::clear` says outstanding handles become no-ops. Keeping
    /// them here would only let a stale token remove a record a REBUILT session registered.
    pub fn clear(&self) {
        self.handles().clear();
    }

    /// How many registration tokens are outstanding.
    #[must_use]
    pub fn len(&self) -> usize {
        self.handles().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handles().is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use serde_json::json;

    fn registry() -> Arc<RuntimeAgentRegistry> {
        Arc::new(RuntimeAgentRegistry::new())
    }

    fn definition() -> Value {
        json!({ "description": "Scout at runtime.", "systemPrompt": "Scout." })
    }

    #[test]
    fn another_topic_is_not_ours() {
        let bridge = RuntimeAgentEventBridge::new();
        assert!(
            bridge
                .dispatch("subagents:rpc:v1:request", &json!({}), &registry())
                .is_none()
        );
    }

    #[test]
    fn a_registration_round_trips_and_the_token_disposes_exactly_one_record() {
        let bridge = RuntimeAgentEventBridge::new();
        let registry = registry();
        let (topic, reply) = bridge
            .dispatch(
                RUNTIME_AGENT_REGISTER_EVENT,
                &runtime_agent_register_request("r1", "runtime-scout", &definition()),
                &registry,
            )
            .expect("our topic");
        assert_eq!(topic, "pi-subagents:runtime-agent-register:v1:reply:r1");
        assert_eq!(reply["version"], json!(1));
        assert_eq!(reply["requestId"], json!("r1"));
        assert_eq!(reply["ok"], json!(true));
        let registration_id = read_registration_reply(Some(&reply)).expect("ok");
        assert_eq!(registry.list().len(), 1);
        assert_eq!(registry.list()[0].name, "runtime-scout");

        // A SECOND agent, so disposal has to pick the right one.
        let second = bridge
            .dispatch(
                RUNTIME_AGENT_REGISTER_EVENT,
                &runtime_agent_register_request("r2", "runtime-other", &definition()),
                &registry,
            )
            .expect("our topic")
            .1;
        read_registration_reply(Some(&second)).expect("ok");
        assert_eq!(bridge.len(), 2);

        let (dispose_topic, dispose_reply) = bridge
            .dispatch(
                RUNTIME_AGENT_DISPOSE_EVENT,
                &runtime_agent_dispose_request("d1", &registration_id),
                &registry,
            )
            .expect("our topic");
        assert_eq!(
            dispose_topic,
            "pi-subagents:runtime-agent-dispose:v1:reply:d1"
        );
        read_disposal_reply(Some(&dispose_reply)).expect("ok");
        assert_eq!(registry.list().len(), 1);
        assert_eq!(registry.list()[0].name, "runtime-other");
        assert_eq!(bridge.len(), 1);
    }

    #[test]
    fn disposal_is_idempotent_and_an_unminted_token_is_the_same_no_op() {
        let bridge = RuntimeAgentEventBridge::new();
        let registry = registry();
        let reply = bridge
            .dispatch(
                RUNTIME_AGENT_REGISTER_EVENT,
                &runtime_agent_register_request("r1", "runtime-scout", &definition()),
                &registry,
            )
            .expect("our topic")
            .1;
        let registration_id = read_registration_reply(Some(&reply)).expect("ok");
        for request_id in ["d1", "d2"] {
            let reply = bridge
                .dispatch(
                    RUNTIME_AGENT_DISPOSE_EVENT,
                    &runtime_agent_dispose_request(request_id, &registration_id),
                    &registry,
                )
                .expect("our topic")
                .1;
            read_disposal_reply(Some(&reply)).expect("dispose never fails");
        }
        let never_minted = bridge
            .dispatch(
                RUNTIME_AGENT_DISPOSE_EVENT,
                &runtime_agent_dispose_request("d3", "0123456789abcdef"),
                &registry,
            )
            .expect("our topic")
            .1;
        read_disposal_reply(Some(&never_minted)).expect("an unknown token is upstream's no-op");
        assert!(registry.is_empty());
    }

    /// The registry's OWN refusals reach the client verbatim — upstream's listener catches and
    /// re-packages the very same `Error` (`runtime-agent-events.ts:66-68`).
    #[test]
    fn a_malformed_definition_answers_with_the_registrys_own_sentence() {
        let bridge = RuntimeAgentEventBridge::new();
        let registry = registry();
        for (payload, expected) in [
            (
                runtime_agent_register_request("r1", "a", &json!([])),
                "Runtime agent definition must be an object.",
            ),
            (
                runtime_agent_register_request(
                    "r1",
                    "a",
                    &json!({"description":"d","systemPrompt":"p","teleport":1}),
                ),
                "Runtime agent definition has unknown fields: teleport.",
            ),
            (
                runtime_agent_register_request("r1", " a", &definition()),
                "Runtime agent name must be a non-empty string without leading or trailing whitespace.",
            ),
            (
                json!({"version": 1, "requestId": "r1", "definition": definition()}),
                "Runtime agent name must be a non-empty string without leading or trailing whitespace.",
            ),
            (
                json!({"version": 1, "requestId": "r1", "name": "a"}),
                "Runtime agent definition must be an object.",
            ),
            (
                runtime_agent_register_request("r1", "scout", &definition()),
                "Runtime agent 'scout' collides with builtin agent 'scout'.",
            ),
        ] {
            let reply = bridge
                .dispatch(RUNTIME_AGENT_REGISTER_EVENT, &payload, &registry)
                .expect("our topic")
                .1;
            assert_eq!(reply["ok"], json!(false));
            assert_eq!(
                read_registration_reply(Some(&reply)).expect_err("refused"),
                expected
            );
        }
        assert!(registry.is_empty());
        assert!(bridge.is_empty());
    }

    /// `String(request.version)` — upstream interpolates it raw, so a string version is BARE.
    #[test]
    fn an_unsupported_version_is_refused_with_upstreams_sentence() {
        let bridge = RuntimeAgentEventBridge::new();
        let registry = registry();
        for (version, rendered) in [
            (json!(2), "2"),
            (json!("1"), "1"),
            (Value::Null, "null"),
            (json!(false), "false"),
            (json!({}), "[object Object]"),
        ] {
            let reply = bridge
                .dispatch(
                    RUNTIME_AGENT_REGISTER_EVENT,
                    &json!({"version": version, "requestId": "r1", "name": "a", "definition": definition()}),
                    &registry,
                )
                .expect("our topic")
                .1;
            assert_eq!(
                read_registration_reply(Some(&reply)).expect_err("refused"),
                format!("Unsupported runtime agent registration event version '{rendered}'.")
            );
        }
        // An ABSENT version is `undefined`, not `null`.
        let reply = bridge
            .dispatch(
                RUNTIME_AGENT_REGISTER_EVENT,
                &json!({"requestId": "r1", "name": "a", "definition": definition()}),
                &registry,
            )
            .expect("our topic")
            .1;
        assert_eq!(
            read_registration_reply(Some(&reply)).expect_err("refused"),
            "Unsupported runtime agent registration event version 'undefined'."
        );
        // And the disposal topic says `disposal`.
        let reply = bridge
            .dispatch(
                RUNTIME_AGENT_DISPOSE_EVENT,
                &json!({"version": 9, "requestId": "r1", "registrationId": "x"}),
                &registry,
            )
            .expect("our topic")
            .1;
        assert_eq!(
            read_disposal_reply(Some(&reply)).expect_err("refused"),
            "Unsupported runtime agent disposal event version '9'."
        );
        assert!(registry.is_empty());
    }

    /// A `requestId` carrying a newline could name a reply topic the caller was never given. It is
    /// rejected before the version is even read, and the refusal goes out on `…:unknown`.
    #[test]
    fn an_unusable_request_id_is_rejected_before_anything_else_and_answered_on_unknown() {
        let bridge = RuntimeAgentEventBridge::new();
        let registry = registry();
        for bad in [
            json!("victim\npi-subagents:runtime-agent-register:v1:reply:other"),
            json!("   "),
            json!(""),
            json!(7),
            Value::Null,
        ] {
            let (topic, reply) = bridge
                .dispatch(
                    RUNTIME_AGENT_REGISTER_EVENT,
                    // Version 9 too: if the id were not checked FIRST, this would answer the
                    // version instead.
                    &json!({"version": 9, "requestId": bad, "name": "a", "definition": definition()}),
                    &registry,
                )
                .expect("our topic");
            assert_eq!(
                topic,
                "pi-subagents:runtime-agent-register:v1:reply:unknown"
            );
            assert_eq!(
                read_registration_reply(Some(&reply)).expect_err("refused"),
                "Runtime agent event requestId must be a non-empty string without newlines."
            );
        }
        // A missing key takes the same branch.
        let (topic, reply) = bridge
            .dispatch(
                RUNTIME_AGENT_DISPOSE_EVENT,
                &json!({"version": 1}),
                &registry,
            )
            .expect("our topic");
        assert_eq!(topic, "pi-subagents:runtime-agent-dispose:v1:reply:unknown");
        assert_eq!(
            read_disposal_reply(Some(&reply)).expect_err("refused"),
            "Runtime agent event requestId must be a non-empty string without newlines."
        );
    }

    #[test]
    fn a_non_object_payload_is_still_answered() {
        let bridge = RuntimeAgentEventBridge::new();
        let registry = registry();
        let (topic, reply) = bridge
            .dispatch(RUNTIME_AGENT_REGISTER_EVENT, &json!("nope"), &registry)
            .expect("our topic");
        assert_eq!(
            topic,
            "pi-subagents:runtime-agent-register:v1:reply:unknown"
        );
        assert_eq!(
            read_registration_reply(Some(&reply)).expect_err("refused"),
            "Runtime agent registration request must be an object."
        );
        let (topic, reply) = bridge
            .dispatch(RUNTIME_AGENT_DISPOSE_EVENT, &json!([1, 2]), &registry)
            .expect("our topic");
        assert_eq!(topic, "pi-subagents:runtime-agent-dispose:v1:reply:unknown");
        assert_eq!(
            read_disposal_reply(Some(&reply)).expect_err("refused"),
            "Runtime agent disposal request must be an object."
        );
    }

    #[test]
    fn a_registration_id_that_is_not_a_string_is_refused() {
        let bridge = RuntimeAgentEventBridge::new();
        let registry = registry();
        for bad in [json!(7), json!(""), Value::Null] {
            let reply = bridge
                .dispatch(
                    RUNTIME_AGENT_DISPOSE_EVENT,
                    &json!({"version": 1, "requestId": "d1", "registrationId": bad}),
                    &registry,
                )
                .expect("our topic")
                .1;
            assert_eq!(
                read_disposal_reply(Some(&reply)).expect_err("refused"),
                "Runtime agent disposal registrationId must be a non-empty string."
            );
        }
    }

    /// pi `registerAgentViaEvents`'s two own refusals (`runtime-agent-events.ts:37,46`).
    #[test]
    fn the_client_reader_carries_upstreams_two_refusals() {
        assert_eq!(
            read_registration_reply(None).expect_err("absent"),
            RUNTIME_AGENT_BRIDGE_ABSENT_MESSAGE
        );
        assert_eq!(
            read_disposal_reply(None).expect_err("absent"),
            RUNTIME_AGENT_BRIDGE_ABSENT_MESSAGE
        );
        for malformed in [
            json!({"version": 1, "requestId": "r1"}),
            json!({"version": 1, "requestId": "r1", "ok": true}),
            json!({"version": 1, "requestId": "r1", "ok": true, "registration": {}}),
            json!({"version": 1, "requestId": "r1", "ok": false}),
            json!({"version": 1, "requestId": "r1", "ok": false, "error": "boom"}),
        ] {
            assert_eq!(
                read_registration_reply(Some(&malformed)).expect_err("malformed"),
                RUNTIME_AGENT_MALFORMED_RESULT_MESSAGE
            );
        }
        // A disposal reply only needs `ok`, so only the two without one are malformed.
        for malformed in [
            json!({"version": 1, "requestId": "r1"}),
            json!({"version": 1, "requestId": "r1", "ok": false}),
        ] {
            assert_eq!(
                read_disposal_reply(Some(&malformed)).expect_err("malformed"),
                RUNTIME_AGENT_MALFORMED_RESULT_MESSAGE
            );
        }
    }

    /// Two registrations of the SAME name, one disposed: upstream removes by record identity, not
    /// by name, so the survivor stays (`runtime-agent-registry.ts:393`).
    #[test]
    fn a_stale_token_never_removes_a_later_registration_of_the_same_name() {
        let bridge = RuntimeAgentEventBridge::new();
        let registry = registry();
        let first = read_registration_reply(Some(
            &bridge
                .dispatch(
                    RUNTIME_AGENT_REGISTER_EVENT,
                    &runtime_agent_register_request("r1", "runtime-scout", &definition()),
                    &registry,
                )
                .expect("our topic")
                .1,
        ))
        .expect("ok");
        // Free the name, then take it again.
        read_disposal_reply(Some(
            &bridge
                .dispatch(
                    RUNTIME_AGENT_DISPOSE_EVENT,
                    &runtime_agent_dispose_request("d1", &first),
                    &registry,
                )
                .expect("our topic")
                .1,
        ))
        .expect("ok");
        read_registration_reply(Some(
            &bridge
                .dispatch(
                    RUNTIME_AGENT_REGISTER_EVENT,
                    &runtime_agent_register_request("r2", "runtime-scout", &definition()),
                    &registry,
                )
                .expect("our topic")
                .1,
        ))
        .expect("ok");
        // The FIRST token again — it names a record that no longer exists.
        read_disposal_reply(Some(
            &bridge
                .dispatch(
                    RUNTIME_AGENT_DISPOSE_EVENT,
                    &runtime_agent_dispose_request("d2", &first),
                    &registry,
                )
                .expect("our topic")
                .1,
        ))
        .expect("ok");
        assert_eq!(registry.list().len(), 1, "the later registration survives");
        assert_eq!(registry.list()[0].name, "runtime-scout");
    }

    #[test]
    fn clear_drops_tokens_without_disposing_records() {
        let bridge = RuntimeAgentEventBridge::new();
        let registry = registry();
        let token = read_registration_reply(Some(
            &bridge
                .dispatch(
                    RUNTIME_AGENT_REGISTER_EVENT,
                    &runtime_agent_register_request("r1", "runtime-scout", &definition()),
                    &registry,
                )
                .expect("our topic")
                .1,
        ))
        .expect("ok");
        bridge.clear();
        assert!(bridge.is_empty());
        // The record is untouched — `clear` is teardown bookkeeping, not a disposal.
        assert_eq!(registry.list().len(), 1);
        // And the stale token is a no-op rather than a way to reach into a rebuilt session.
        read_disposal_reply(Some(
            &bridge
                .dispatch(
                    RUNTIME_AGENT_DISPOSE_EVENT,
                    &runtime_agent_dispose_request("d1", &token),
                    &registry,
                )
                .expect("our topic")
                .1,
        ))
        .expect("ok");
        assert_eq!(registry.list().len(), 1);
    }
}
