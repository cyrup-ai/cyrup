//! The extension-bus frames and the shared `extensions`-field validators
//! (`v0.9.2 broker/broker.ts:446-456,551-585,961-969,1159-1182`).
//!
//! ICOM-016 landed the bus EFFECTS, so the broker now advertises
//! [`EXTENSION_BUS_FEATURE`](crate::transport::protocol::EXTENSION_BUS_FEATURE) on `registered` and
//! a conforming pi client sends these frames as a matter of course. This module owns all four
//! halves: the capability bookkeeping, the per-namespace owner election
//! ([`BrokerState::recompute_namespace_owners`]), the `extension_publish` fan-out, and the
//! revision-checked commit that drives [`super::extension_state`].
//!
//! [`extensions_field_is_valid`] lives here rather than in a validation grab-bag because it IS the
//! `extensions` field's guard — upstream shares it verbatim between `case "register"` and
//! `case "extension_capabilities_update"`, which is why `super::session` imports it.

use tokio::sync::mpsc::UnboundedSender;

use sha2::{Digest as _, Sha256};

use crate::transport::protocol::{
    BrokerMessage, ExtensionAudience, ExtensionCapability, ExtensionOwnerRef, ScopeId, now_ms,
};

use super::extension_state::{hex, serialize_payload};
use super::frame::{FrameResult, send_msg};
use super::js::{js_safe_u64, js_string_or_empty, js_to_string};
use super::limits::{
    MAX_EXTENSION_MESSAGE_BYTES, MAX_EXTENSION_STATE_BYTES, MAX_EXTENSIONS_PER_SESSION,
};
use super::routing::SessionKey;
use super::state::BrokerState;

/// `scopedExtensionKey(scopeId, namespace)` (`v0.13.0 broker/broker.ts:152-154`), as a type rather
/// than upstream's `JSON.stringify([scopeId ?? null, namespace])` string.
///
/// Same discipline as [`SessionKey`]: a bare namespace stops being a usable key of
/// `BrokerState::namespace_owners`, so an election or an owner lookup that forgot the scope cannot
/// be written. A namespace advertised in two scopes elects two independent owners.
///
/// The derived `Ord` puts `scope_id: None` before every `Some` and then orders by namespace —
/// total and deterministic, which is the property `super::state`'s `BTreeMap` is documented for.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct NamespaceKey {
    /// The advertising session's scope, `None` for unscoped.
    pub(super) scope_id: Option<ScopeId>,
    /// The extension namespace.
    pub(super) namespace: String,
}

/// `scopedExtensionStateNamespace(scopeId, namespace)` (`v0.13.0 broker/broker.ts:156-161`).
///
/// **UNSCOPED RETURNS THE BARE NAMESPACE.** That is not a shortcut — it is the opt-in guarantee for
/// persistence: [`super::extension_state::ExtensionStateManager`] names each file by
/// `sha256(<this string>)`, so every file already on disk under `<intercomDir>/extension-state/`
/// keeps its name and its contents. Only a scoped session gets the tagged form, and therefore a
/// different file.
///
/// The tagged form must match upstream's JSON byte for byte, because it IS the persistence key and
/// a pi broker may share the directory. `serde_json::Value::to_string` and `JSON.stringify` agree
/// here: both emit `["a","b","c"]` with no spaces, both escape only `"`, `\` and C0 controls, and
/// neither escapes `/` or non-ASCII. `namespace_is_valid` narrows the namespace to
/// `[a-z0-9._/-]` anyway, and the scope reaches this function only as a lowercase sha256 hex digest.
pub(super) fn scoped_extension_state_namespace(
    scope_id: Option<&ScopeId>,
    namespace: &str,
) -> String {
    match scope_id {
        None => namespace.to_string(),
        Some(scope) => serde_json::json!([
            "scope",
            hex(&Sha256::digest(scope.as_str().as_bytes())),
            namespace
        ])
        .to_string(),
    }
}

/// `NamespaceOwner` (`v0.9.2 broker/broker.ts:60-64`, `v0.13.0` `:76-83`).
///
/// pi's `socket` identity is this port's `conn_id`: an identity takeover reassigns the id to a new
/// connection, and upstream's `existing.socket !== winner.session.socket` is precisely the check
/// that a re-elected owner on a NEW socket gets a NEW epoch — dropping it would let a superseded
/// connection keep committing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NamespaceOwner {
    /// `sessionKey` (`v0.13.0 broker/broker.ts:79`) — the composite key, because once `sessions` is
    /// keyed by [`SessionKey`] a bare id can no longer index it. The bare id still goes on the
    /// WIRE: [`ExtensionOwnerRef::owner_id`] carries `key.id`, exactly as upstream sends
    /// `winner.session.info.id` (`:1420`), so a scoped session's `extension_owner` frame is
    /// indistinguishable from an unscoped one's.
    pub(super) session_key: SessionKey,
    pub(super) conn_id: u64,
    pub(super) epoch: String,
}

/// pi's `extensions` field guard, shared verbatim by `case "register"`
/// (`v0.9.2 broker/broker.ts:446-456`) and `case "extension_capabilities_update"`
/// (`v0.9.2 broker/broker.ts:559-567`): the value must be an ARRAY of at most
/// [`MAX_EXTENSIONS_PER_SESSION`] entries, each passing `validateExtensionCapability`
/// (`v0.9.2 broker/broker.ts:1159-1168`). Anything else `throw`s, i.e. destroys the socket.
///
/// The per-entry decode into [`ExtensionCapability`] reproduces upstream's
/// `typeof c.namespace !== "string" || typeof c.ownerEligible !== "boolean"` check *and* its
/// rejection of an array-shaped entry (`[]["namespace"]` is `undefined`), the latter because that
/// struct is `[MAP-ONLY]` — see `crate::transport::protocol`.
pub(super) fn extensions_field_is_valid(extensions: &serde_json::Value) -> bool {
    let Some(items) = extensions.as_array() else {
        return false;
    };
    if items.len() > MAX_EXTENSIONS_PER_SESSION {
        return false;
    }
    items.iter().all(|item| {
        serde_json::from_value::<ExtensionCapability>(item.clone())
            .is_ok_and(|cap| namespace_is_valid(&cap.namespace))
    })
}

/// `validateNamespace` (`v0.9.2 broker/broker.ts:1170-1182`): `^[a-z0-9][a-z0-9._/-]{0,63}$`, with
/// the length bound checked first.
///
/// [CYRUP-DELTA] pi's `ns.length` counts UTF-16 code units and this counts `char`s; the two can
/// disagree only for non-ASCII input, which the character test rejects on both sides anyway, so the
/// accepted set is identical.
fn namespace_is_valid(ns: &str) -> bool {
    if ns.is_empty() || ns.chars().count() > 64 {
        return false;
    }
    let mut chars = ns.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/' | '-'))
}

impl BrokerState {
    /// `recomputeNamespaceOwners` (`v0.9.2 broker/broker.ts:1184-1261`). Called from all four of
    /// pi's sites: register (`:509`), unregister (`:544`), socket close (`:337`) and
    /// `extension_capabilities_update` (`:569`).
    pub(super) fn recompute_namespace_owners(&mut self) {
        // `new Set(this.namespaceOwners.keys())` + every advertised namespace (`:1185-1189`),
        // each paired with the ADVERTISING SESSION'S scope (`v0.13.0 broker/broker.ts:1346-1360`)
        // — so a namespace advertised in two scopes is two entries and elects two owners.
        let mut namespaces: std::collections::BTreeSet<NamespaceKey> =
            self.namespace_owners.keys().cloned().collect();
        for (key, session) in self.sessions_in_order() {
            namespaces.extend(session.extensions.iter().map(|e| NamespaceKey {
                scope_id: key.scope.clone(),
                namespace: e.namespace.clone(),
            }));
        }

        for ns_key in namespaces {
            let namespace = ns_key.namespace.clone();
            let scope = ns_key.scope_id.clone();
            // Candidates: sessions advertising this namespace with `ownerEligible`, IN THIS SCOPE
            // (`sameScope(session.scopeId, scopeId) && …`, `v0.13.0 broker/broker.ts:1366-1373`).
            // Collected owned, so the elected winner can be written back under `&mut self` below.
            let mut candidates: Vec<(SessionKey, u64, u64)> = self
                .sessions_in_order()
                .filter(|(key, s)| {
                    key.in_scope(scope.as_ref())
                        && s.extensions
                            .iter()
                            .any(|e| e.namespace == namespace && e.owner_eligible)
                })
                .map(|(key, s)| (key.clone(), s.owner_order, s.conn_id))
                .collect();

            // `candidates.sort((a, b) => ownerOrder, then sessionId.localeCompare)` (`:1220-1226`).
            // The id tie-break is unreachable — `owner_order` comes from a monotonic counter and is
            // unique per LIVE session — so byte order stands in for `localeCompare` with no
            // reachable difference; it is ported for shape, not for effect.
            candidates.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

            let Some((winner_key, _, winner_conn)) = candidates.first().cloned() else {
                // `if (this.namespaceOwners.delete(namespaceKey))` (`:1377-1387`): only a namespace
                // that WAS owned announces its vacancy, and only to capable sessions IN ITS SCOPE.
                if self.namespace_owners.remove(&ns_key).is_some() {
                    self.notify_namespace_capable(
                        scope.as_ref(),
                        &namespace,
                        &ExtensionOwnerRef::default(),
                    );
                }
                continue;
            };

            let existing = self.namespace_owners.get(&ns_key);
            let owner_changed = existing.is_none_or(|o| o.session_key != winner_key);
            let socket_changed = existing.is_some_and(|o| o.conn_id != winner_conn);
            if !owner_changed && !socket_changed {
                continue;
            }
            let epoch = uuid::Uuid::new_v4().to_string();
            let winner_id = winner_key.id.clone();
            self.namespace_owners.insert(
                ns_key.clone(),
                NamespaceOwner {
                    session_key: winner_key,
                    conn_id: winner_conn,
                    epoch: epoch.clone(),
                },
            );
            self.notify_namespace_capable(
                scope.as_ref(),
                &namespace,
                &ExtensionOwnerRef {
                    owner_id: Some(winner_id),
                    owner_epoch: Some(epoch),
                },
            );
        }
    }

    /// The `extension_owner` fan-out both arms of the election share
    /// (`v0.9.2 broker/broker.ts:1205-1211` and `:1243-1257`): every session that advertises the
    /// namespace, in join order.
    ///
    /// [CYRUP-DELTA] The vacancy arm upstream tests `session.extensions?.some(…)` and the election
    /// arm tests `session.extensions?.length && …some(…)`; `.some()` on an empty array is already
    /// `false`, so the two conditions are the same set and one helper serves both.
    ///
    /// ICOM-055 — `sameScope(session.scopeId, scopeId) && …` (`v0.13.0 broker/broker.ts:1380`,
    /// `:1419`): a vacancy or an election in one scope is announced only inside that scope.
    fn notify_namespace_capable(
        &self,
        scope: Option<&ScopeId>,
        namespace: &str,
        owner: &ExtensionOwnerRef,
    ) {
        for (key, session) in self.sessions_in_order() {
            if key.in_scope(scope) && session.extensions.iter().any(|e| e.namespace == namespace) {
                send_msg(
                    &session.tx,
                    &BrokerMessage::ExtensionOwner {
                        namespace: namespace.to_string(),
                        owner: owner.clone(),
                    },
                );
            }
        }
    }

    /// The per-capability replay upstream duplicates between `register` (`:512-528`) and
    /// `extension_capabilities_update` (`:570-585`), factored once.
    ///
    /// The `extension_owner` frame is UNCONDITIONAL — an unowned namespace answers with the
    /// ownerless [`ExtensionOwnerRef::default`], which is how a joining session learns the namespace
    /// exists and has no owner. The `extension_state` frame follows only when a state has ever been
    /// committed.
    pub(super) fn replay_extension_state(
        &mut self,
        self_tx: &UnboundedSender<Vec<u8>>,
        scope: Option<&ScopeId>,
        namespaces: &[String],
    ) {
        for namespace in namespaces {
            // `this.namespaceOwners.get(scopedExtensionKey(scopeId, ext.namespace))`
            // (`v0.13.0 broker/broker.ts:511`, `:568`).
            let owner = self
                .namespace_owners
                .get(&NamespaceKey {
                    scope_id: scope.cloned(),
                    namespace: namespace.clone(),
                })
                .map_or_else(ExtensionOwnerRef::default, |o| ExtensionOwnerRef {
                    owner_id: Some(o.session_key.id.clone()),
                    owner_epoch: Some(o.epoch.clone()),
                });
            send_msg(
                self_tx,
                &BrokerMessage::ExtensionOwner {
                    namespace: namespace.clone(),
                    owner,
                },
            );
            // `this.extensionStateManager.loadState(scopedExtensionStateNamespace(scopeId,
            // ext.namespace))` (`v0.13.0 broker/broker.ts:517`, `:574`).
            let state_namespace = scoped_extension_state_namespace(scope, namespace);
            if let Some(state) = self.extension_state.load_state(&state_namespace) {
                let (revision, payload) = (state.revision, state.payload.clone());
                send_msg(
                    self_tx,
                    &BrokerMessage::ExtensionState {
                        namespace: namespace.clone(),
                        revision,
                        payload: Some(payload),
                    },
                );
            }
        }
    }

    /// `case "extension_capabilities_update"` (`v0.9.2 broker/broker.ts:551-585`).
    ///
    /// pi's **validation prefix** (`:559-567`) runs before any effect and every one of its failures
    /// is a `throw` → `socket.destroy` (`framing.ts:44-51`), including the array-shaped
    /// `[["ns", true]]` which serde would otherwise fill into [`ExtensionCapability`] positionally.
    ///
    /// The "before register" throw at `:552-554` is covered by the shared pre-registration guard in
    /// [`Self::handle_frame`]. The "session not found" throw at `:555-558` is ported below: pi's
    /// `session.socket !== socket` is [`Self::session_owns_connection`] here, and it IS reachable —
    /// an identity takeover (`handle_register`'s `close.notify_one()`) reassigns the id to the new
    /// connection before the superseded reader task observes the notify, so a frame already in the
    /// old socket's buffer arrives with a stale `conn_id`. Upstream destroys that connection; so
    /// does this.
    pub(super) fn handle_extension_capabilities_update(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_key: &Option<SessionKey>,
    ) -> FrameResult {
        let Some(current_key) = session_key.as_ref() else {
            return FrameResult::protocol_error();
        };
        // `throw new Error("Extension capability session not found")` (`:556-558`) — fatal.
        if !self.session_owns_connection(current_key, conn_id) {
            return FrameResult::protocol_error();
        }
        // `!Array.isArray(undefined)` is true, so a missing field throws upstream too (`:559-562`).
        let extensions = value.get("extensions").unwrap_or(&serde_json::Value::Null);
        if !extensions_field_is_valid(extensions) {
            return FrameResult::protocol_error();
        }
        // The guard above has already proved every element decodes, so this arm is unreachable for
        // a frame that got here.
        let Ok(capabilities) =
            serde_json::from_value::<Vec<ExtensionCapability>>(extensions.clone())
        else {
            return FrameResult::protocol_error();
        };
        let namespaces: Vec<String> = capabilities.iter().map(|c| c.namespace.clone()).collect();
        if let Some(session) = self.sessions.get_mut(current_key) {
            session.extensions = capabilities; // `session.extensions = extensions` (`:568`)
        }
        self.recompute_namespace_owners(); // `:569`
        // `session.scopeId` (`v0.13.0 broker/broker.ts:568,574`).
        self.replay_extension_state(self_tx, current_key.scope.clone().as_ref(), &namespaces); // `:570-585`
        FrameResult::cont()
    }

    /// pi's `session.socket !== socket` guard (`v0.9.2 broker/broker.ts:556,1272,1368`), expressed
    /// against cyrup's connection ids: the session must exist AND still be owned by the connection
    /// the frame arrived on.
    fn session_owns_connection(&self, session_key: &SessionKey, conn_id: u64) -> bool {
        self.sessions.get(session_key).map(|s| s.conn_id) == Some(conn_id)
    }

    /// `handleExtensionPublish` (`v0.9.2 broker/broker.ts:1262-1356`).
    ///
    /// Answering matters for the same reason it did for `cancel_message`: pi's client resolves an
    /// extension publish only on a broker frame, so a silent drop strands the caller.
    pub(super) fn handle_extension_publish(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_key: &Option<SessionKey>,
    ) -> FrameResult {
        let Some(current_key) = session_key.as_ref() else {
            return FrameResult::protocol_error();
        };
        let refuse = |error: &str| {
            send_msg(
                self_tx,
                &BrokerMessage::Error {
                    error: error.to_string(),
                },
            );
            FrameResult::cont()
        };

        // `!session || session.socket !== socket` → `"Session not found"`, NOT fatal (`:1271-1275`).
        if !self.session_owns_connection(current_key, conn_id) {
            return refuse("Session not found");
        }
        let scope = current_key.scope.clone();
        // `!session.extensions?.length` (`:1277-1280`) — now a REAL test, not a constant.
        let advertised: Vec<String> = self
            .sessions
            .get(current_key)
            .map(|s| s.extensions.iter().map(|e| e.namespace.clone()).collect())
            .unwrap_or_default();
        if advertised.is_empty() {
            return refuse("Session has not advertised extension capability");
        }

        // `typeof namespace !== "string" || !validateNamespace(namespace)` (`:1288-1291`).
        let Some(namespace) = value
            .get("namespace")
            .and_then(|v| v.as_str())
            .filter(|ns| namespace_is_valid(ns))
        else {
            return refuse("Invalid namespace");
        };
        // `audience !== "owner" && audience !== "capable"` (`:1293-1296`).
        let Ok(audience) = serde_json::from_value::<ExtensionAudience>(
            value
                .get("audience")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        ) else {
            return refuse("Invalid audience");
        };
        // `msg.ownerOnly === true` — a STRICT equality, so any other value is `false` (`:1285`).
        let owner_only = value.get("ownerOnly") == Some(&serde_json::Value::Bool(true));
        // `serializedPayloadSize(payload)` (`:1298-1302`): an absent payload is `null` here, i.e. a
        // refusal, because `JSON.stringify(undefined)` is `undefined` upstream too.
        let Some(payload_len) = serialize_payload(value.get("payload")).map(|j| j.len()) else {
            return refuse("Invalid extension payload or payload exceeds 16 KiB limit");
        };
        if payload_len > MAX_EXTENSION_MESSAGE_BYTES {
            return refuse("Invalid extension payload or payload exceeds 16 KiB limit");
        }
        // `hasCapability` (`:1305-1309`).
        if !advertised.iter().any(|ns| ns == namespace) {
            return refuse("Sender does not have capability for this namespace");
        }

        // `:1311-1329` — owner requirements. `this.namespaceOwners.get(scopedExtensionKey(
        // session.scopeId, namespace))` (`v0.13.0 broker/broker.ts:1484`).
        let owner = self
            .namespace_owners
            .get(&NamespaceKey {
                scope_id: scope.clone(),
                namespace: namespace.to_string(),
            })
            .cloned();
        if (audience == ExtensionAudience::Owner || owner_only) && owner.is_none() {
            return refuse("No owner for this namespace");
        }
        if owner_only && let Some(owner) = owner.as_ref() {
            let Some(epoch) = value.get("ownerEpoch").and_then(|v| v.as_str()) else {
                return refuse("ownerEpoch required for owner-only messages");
            };
            if *current_key != owner.session_key || conn_id != owner.conn_id || epoch != owner.epoch
            {
                return refuse("Owner validation failed");
            }
        }

        // The join-ordered fan-out (`:1332-1355`).
        let owner_ref =
            owner
                .as_ref()
                .map_or_else(ExtensionOwnerRef::default, |o| ExtensionOwnerRef {
                    owner_id: Some(o.session_key.id.clone()),
                    owner_epoch: Some(o.epoch.clone()),
                });
        let payload = value.get("payload").cloned();
        for (key, recipient) in self.sessions_in_order() {
            // `if (!sameScope(recipientSession.scopeId, session.scopeId)) continue;`
            // (`v0.13.0 broker/broker.ts:1504-1506`) — an extension message never leaves its scope.
            if !key.in_scope(scope.as_ref()) {
                continue;
            }
            if !recipient
                .extensions
                .iter()
                .any(|e| e.namespace == namespace)
            {
                continue;
            }
            // `shouldReceive` (`:1344-1348`). Note the publisher is NOT excluded from a `capable`
            // fan-out: pi routes a session's own publish back to it.
            let should_receive = audience == ExtensionAudience::Capable
                || owner
                    .as_ref()
                    .is_some_and(|o| key == &o.session_key && recipient.conn_id == o.conn_id);
            if should_receive {
                send_msg(
                    &recipient.tx,
                    &BrokerMessage::ExtensionMessage {
                        namespace: namespace.to_string(),
                        from_session_id: current_key.id.clone(),
                        owner: owner_ref.clone(),
                        payload: payload.clone(),
                    },
                );
            }
        }
        FrameResult::cont()
    }

    /// `handleExtensionStateCommit` (`v0.9.2 broker/broker.ts:1358-1495`).
    ///
    /// Every exit writes an `extension_state_result`. A commit is a promise upstream; dropping it
    /// silently hangs the committer forever, which is the same defect the `cancel_message` port
    /// already fixed.
    pub(super) fn handle_extension_state_commit(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_key: &Option<SessionKey>,
    ) -> FrameResult {
        let Some(current_key) = session_key.as_ref() else {
            return FrameResult::protocol_error();
        };
        let scope = current_key.scope.clone();

        // The two PRE-type-check echoes (`:1371`, `:1382`) coerce the raw value with
        // `String(msg.namespace || "")`, which is not the same expression `:1394` uses.
        let echo_ns = js_string_or_empty(value.get("namespace"));
        let early_refuse = |reason: &str| {
            send_msg(
                self_tx,
                &BrokerMessage::ExtensionStateResult {
                    namespace: echo_ns.clone(),
                    committed: false,
                    revision: 0,
                    reason: Some(reason.to_string()),
                },
            );
            FrameResult::cont()
        };
        if !self.session_owns_connection(current_key, conn_id) {
            return early_refuse("Session not found");
        }
        let advertised: Vec<String> = self
            .sessions
            .get(current_key)
            .map(|s| s.extensions.iter().map(|e| e.namespace.clone()).collect())
            .unwrap_or_default();
        if advertised.is_empty() {
            return early_refuse("Session has not advertised extension capability");
        }

        // `:1394` writes `String(namespace)`, NOT `String(namespace || "")` — observably different
        // for `namespace: 0`, which echoes `"0"` here and `""` in the two branches above.
        let Some(namespace) = value
            .get("namespace")
            .and_then(|v| v.as_str())
            .filter(|ns| namespace_is_valid(ns))
        else {
            send_msg(
                self_tx,
                &BrokerMessage::ExtensionStateResult {
                    namespace: value
                        .get("namespace")
                        .map_or_else(String::new, js_to_string),
                    committed: false,
                    revision: 0,
                    reason: Some("Invalid namespace".to_string()),
                },
            );
            return FrameResult::cont();
        };
        let namespace = namespace.to_string();
        // `const stateNamespace = scopedExtensionStateNamespace(session.scopeId, namespace)`
        // (`v0.13.0 broker/broker.ts:1581`): every revision read and the commit itself go through
        // the SCOPED namespace, while the `extension_state_result` frame still echoes the bare one.
        let state_namespace = scoped_extension_state_namespace(scope.as_ref(), &namespace);

        // Every refusal past the namespace check reports the CURRENT revision, not 0 (`:1409`,
        // `:1420`, `:1432`, `:1445`, `:1457`, `:1469`).
        macro_rules! refuse_current {
            ($reason:expr) => {{
                let revision = self.extension_state.current_revision(&state_namespace);
                send_msg(
                    self_tx,
                    &BrokerMessage::ExtensionStateResult {
                        namespace: namespace.clone(),
                        committed: false,
                        revision,
                        reason: Some(($reason).to_string()),
                    },
                );
                return FrameResult::cont();
            }};
        }

        let Some(owner_epoch) = value
            .get("ownerEpoch")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            refuse_current!("Invalid ownerEpoch")
        };
        let Some(expected_revision) = js_safe_u64(value.get("expectedRevision")) else {
            refuse_current!("Invalid expectedRevision")
        };
        let payload_ok = serialize_payload(value.get("payload"))
            .is_some_and(|j| j.len() <= MAX_EXTENSION_STATE_BYTES);
        if !payload_ok {
            refuse_current!("Invalid extension state or payload exceeds 64 KiB limit")
        }
        if !advertised.iter().any(|ns| ns == &namespace) {
            refuse_current!("Sender does not have capability for this namespace")
        }
        let Some(owner) = self
            .namespace_owners
            .get(&NamespaceKey {
                scope_id: scope.clone(),
                namespace: namespace.clone(),
            })
            .cloned()
        else {
            refuse_current!("No owner for this namespace")
        };
        if *current_key != owner.session_key
            || conn_id != owner.conn_id
            || owner_epoch != owner.epoch
        {
            refuse_current!("Owner validation failed")
        }

        let payload = value.get("payload").cloned();
        let result = self.extension_state.commit_state(
            &state_namespace,
            expected_revision,
            payload.as_ref(),
            now_ms(),
        );
        send_msg(
            self_tx,
            &BrokerMessage::ExtensionStateResult {
                namespace: namespace.clone(),
                committed: result.committed,
                revision: result.revision,
                reason: result.reason.map(str::to_string),
            },
        );
        if !result.committed {
            return FrameResult::cont();
        }
        // The commit fan-out to every capable session in join order, the committer INCLUDED
        // (`:1484-1495`).
        for (key, recipient) in self.sessions_in_order() {
            // `if (!sameScope(recipientSession.scopeId, session.scopeId)) continue;`
            // (`v0.13.0 broker/broker.ts:1668-1670`).
            if key.in_scope(scope.as_ref())
                && recipient
                    .extensions
                    .iter()
                    .any(|e| e.namespace == namespace)
            {
                send_msg(
                    &recipient.tx,
                    &BrokerMessage::ExtensionState {
                        namespace: namespace.clone(),
                        revision: result.revision,
                        payload: payload.clone(),
                    },
                );
            }
        }
        FrameResult::cont()
    }
}
