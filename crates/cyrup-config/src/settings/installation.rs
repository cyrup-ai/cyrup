//! The stable **installation** id — pi `SettingsManager.getOrCreateDeviceId`
//! (`packages/coding-agent/src/core/settings-manager.ts:1175-1182` @v1.0.1).
//!
//! PROV-118 ported "Sign in with ChatGPT" into `cyrup-provider`, which refuses to start
//! authorization without one:
//! [`cyrup_provider::auth::oauth`]'s `agent_host_id` lowercases the id into `urn:uuid:<uuid>` and
//! sends it as the authorize request's `ext_agent_host_id`, so OpenAI's authorization server
//! recognises repeat logins from the same install. Nothing supplied it, so `/login openai` failed
//! at its first statement with "Sign in with ChatGPT requires a device ID (UUID) for this
//! installation". This module is the supplier.
//!
//! Upstream, verbatim at the pin:
//!
//! ```ts
//! /**
//!  * Stable ID of this installation, e.g. sent to OpenAI as its agent host ID.
//!  * Created on first use. Project settings are ignored so a committed project
//!  * settings file cannot give every clone the same ID.
//!  */
//! getOrCreateDeviceId(): string {
//!     if (!this.globalSettings.deviceId) {
//!         this.globalSettings.deviceId = randomUUID();
//!         this.markModified("deviceId");
//!         this.save();
//!     }
//!     return this.globalSettings.deviceId;
//! }
//! ```
//!
//! Three properties of that body are load-bearing and each is pinned by a test here:
//!
//! 1. **`this.globalSettings`, not `this.settings`.** The id is read off the raw GLOBAL document,
//!    never the merged view, so a project `.cyrup/settings.json` cannot supply it — upstream's own
//!    test seeds `{"deviceId":"project-device"}` into the project file and asserts the generated
//!    UUID wins (`packages/coding-agent/test/settings-manager.test.ts:110-127`). This is a
//!    per-*installation* id: honouring a project layer would give one machine several identities
//!    and leak one project's id into another. [`Settings::device_id`](super::layer::Settings::device_id)
//!    is therefore the only reader,
//!    and it takes the global layer.
//! 2. **The `if` guard is JS truthiness, and it never overwrites.** Any truthy stored value is
//!    returned verbatim — including one that is not a UUID. Upstream does *not* repair it; the
//!    provider then refuses the login with the same message a missing id produces. That is
//!    [`DeviceIdDecision::KeepUnusable`], and it is deliberate: silently reminting would destroy a
//!    value the user (or another tool) put there, and would make a login succeed where pi's fails.
//! 3. **It is only called by flows that need it.** Upstream wires exactly one call site, the login
//!    path (`modes/interactive/interactive-mode.ts:6262`,
//!    `{ getDeviceId: () => this.settingsManager.getOrCreateDeviceId() }`), and passes it for every
//!    provider — but as a *callback*, so the id is created only when a flow actually asks. Logging
//!    in to `anthropic` writes nothing. [`InstallationIdSupplier::newly_created`] reproduces that:
//!    it reports an id to persist only once the callback has been invoked.
//!
//! ## Shape (docs/RUST-DESIGN-REVIEW.md)
//!
//! * **Newtype with parse, don't validate** ([`InstallationId`]). The invariant "is a UUID" already
//!   existed — `cyrup-provider` re-tested it at the wire boundary and failed the login when it did
//!   not hold. It is carried by the type instead: the inner `String` is private, there is no
//!   `Default` (no valid default installation exists) and no `Deserialize` (serde is a construction
//!   path, and this value arrives as untyped settings JSON), so the only ways in are
//!   [`InstallationId::generate`] and [`InstallationId::from_str`]. The provider's own check stays —
//!   it accepts a bare `String` from any embedder, so that is defence in depth, not a duplicate.
//! * **Explicit domain enum** ([`DeviceIdDecision`]). The three outcomes of looking at the stored
//!   value are named for the domain rather than encoded as `Option<Option<String>>`.
//! * **Functional core, imperative shell.** [`DeviceIdDecision::for_stored`] is a pure, total
//!   function of the stored JSON value: both the "already present" and the "absent" arms are
//!   testable without a settings file. [`InstallationIdSupplier`] is the shell that mints on the
//!   `Create` arm and records whether anyone asked; the awaited write lives on
//!   [`crate::settings::SettingsManager::persist_installation_id`].

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use super::manager::random_uuid_v4;

/// The settings key, in upstream's spelling (`settings-manager.ts:158`, declared there as
/// `deviceId?: string; // stable UUID of this installation, created when a login first needs it;
/// global setting only`).
pub const DEVICE_ID_KEY: &str = "deviceId";

/// The character length of the 8-4-4-4-12 UUID form, matching
/// `cyrup_provider::auth::oauth::openai_chatgpt::UUID_PATTERN_LEN` and upstream's `UUID_PATTERN`
/// (`packages/ai/src/auth/oauth/openai-chatgpt.ts:18`).
const UUID_LEN: usize = 36;

/// The group widths of the 8-4-4-4-12 form.
const UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// The stable id of **this app installation** (pi `Settings.deviceId`).
///
/// It is sent to OpenAI as `ext_agent_host_id` so their authorization server recognises repeat
/// logins from the same install. It is **not** a user identifier, **not** a secret, and **not** a
/// device fingerprint: it is a random UUID minted once and persisted, never derived from hardware,
/// hostname, MAC address or username. Deriving it would identify the machine or the person across
/// installs — a privacy regression, and not what upstream does (`randomUUID()`).
///
/// ## The exact guarantee
///
/// What becomes impossible: holding an `InstallationId` whose text is not the 8-4-4-4-12 hex form.
/// The field is private, there is no public constructor that skips the parse, no `Default`, and no
/// `Deserialize`.
///
/// What remains possible and is still tested elsewhere: the *stored* settings value is arbitrary
/// JSON written by anyone, so reading it can fail — that is [`DeviceIdDecision`]'s job, not this
/// type's. And `cyrup-provider` still validates the `String` it is handed, because its public API
/// accepts one from any embedder.
///
/// `Clone` is deliberate: this is a long-lived identifier read many times per process, not one-use
/// authority.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InstallationId(String);

impl InstallationId {
    /// `randomUUID()` (`settings-manager.ts:1177`). A fresh random v4 UUID.
    ///
    /// Shares [`random_uuid_v4`] with `setEnableAnalytics`'s `trackingId`, which is upstream's own
    /// sibling of this function (`settings-manager.ts:943-951`) — the same "generate once into the
    /// global layer on first use" shape.
    pub fn generate() -> Self {
        let generated = random_uuid_v4();
        debug_assert!(
            parse_uuid(&generated).is_some(),
            "random_uuid_v4 must emit the 8-4-4-4-12 form"
        );
        Self(generated)
    }

    /// The id as upstream persists and transmits it: lowercase 8-4-4-4-12.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InstallationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a stored `deviceId` is not an [`InstallationId`].
///
/// Carries no copy of the offending text: the only caller that needs it keeps the original to hand
/// back verbatim ([`DeviceIdDecision::KeepUnusable`]), exactly as upstream does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstallationIdParseError;

impl fmt::Display for InstallationIdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("installation id must be a UUID (8-4-4-4-12 hex)")
    }
}

impl std::error::Error for InstallationIdParseError {}

impl FromStr for InstallationId {
    type Err = InstallationIdParseError;

    /// The parse half of parse-don't-validate. Accepts upstream's case-insensitive `UUID_PATTERN`
    /// and normalises to lowercase, which is the form `agent_host_id` transmits
    /// (`openai-chatgpt.ts:227-230`, `id.toLowerCase()`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_uuid(s).map(Self).ok_or(InstallationIdParseError)
    }
}

/// `UUID_PATTERN.test(value)` plus the lowercasing, as one fallible parse.
///
/// Written out rather than compiled as a regex for the same reason
/// `cyrup_provider::auth::oauth`'s `is_uuid` is: the shape is fixed.
fn parse_uuid(value: &str) -> Option<String> {
    if value.len() != UUID_LEN {
        return None;
    }
    let mut rest = value;
    for (index, width) in UUID_GROUPS.iter().enumerate() {
        if index > 0 {
            rest = rest.strip_prefix('-')?;
        }
        if rest.len() < *width {
            return None;
        }
        let (group, tail) = rest.split_at(*width);
        if !group.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        rest = tail;
    }
    rest.is_empty().then(|| value.to_lowercase())
}

/// What the GLOBAL layer's `deviceId` means for the next login — the `if` in
/// `getOrCreateDeviceId` (`settings-manager.ts:1176`), as an exhaustive domain enum.
///
/// Produced by the pure [`Self::for_stored`]; consumed by [`InstallationIdSupplier`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceIdDecision {
    /// A usable id is already stored. Return it; **write nothing** (upstream's `if` body is
    /// skipped, so no `markModified`/`save` runs and the settings file is not touched).
    Reuse(InstallationId),
    /// A truthy value is stored that is not a UUID.
    ///
    /// Upstream returns it **verbatim** — `!this.globalSettings.deviceId` is false for any truthy
    /// value, so the generate-and-save branch never runs — and
    /// `cyrup_provider::auth::oauth`'s `agent_host_id` then refuses the login with the same
    /// message a missing id produces. cyrup matches that: the stored text is handed back unchanged,
    /// the file is **not** rewritten, and the login fails. Reminting here would both destroy a
    /// value cyrup did not write and make a login succeed where pi's fails.
    KeepUnusable(String),
    /// No usable value yet — absent, `null`, `""`, or any other JS-falsy value. Mint one and
    /// persist it to the GLOBAL scope.
    ///
    /// Carries no id, which keeps [`Self::for_stored`] a pure, deterministic function of the stored
    /// value; the mint happens in [`InstallationIdSupplier::new`].
    Create,
}

impl DeviceIdDecision {
    /// Pure. `stored` is the raw JSON value at `deviceId` in the **global** layer, as
    /// [`Settings::device_id`] yields it.
    ///
    /// [`Settings::device_id`]: super::layer::Settings::device_id
    ///
    /// The falsy set is JavaScript's, because upstream's guard is `if (!...)` on an untyped
    /// document rather than a typed check. `deviceId` is declared `string | undefined`, but
    /// settings are user-edited JSON, so the value can be anything:
    ///
    /// | stored | JS | here |
    /// |---|---|---|
    /// | absent / `null` / `""` / `false` / `0` | falsy → generate | [`Self::Create`] |
    /// | a UUID string | truthy → returned | [`Self::Reuse`] |
    /// | any other truthy value (`"nope"`, `42`, `{}`) | truthy → returned as-is, login refused | [`Self::KeepUnusable`] |
    ///
    /// The non-string truthy row matters: TypeScript's declared type is a lie about a JSON file a
    /// user can edit, and `UUID_PATTERN.test(42)` coerces to `"42"` and fails. Treating `42` as
    /// absent would mint an id and let the login succeed where pi's refuses, so a truthy non-string
    /// is `KeepUnusable` and the login is refused, exactly as upstream's is.
    ///
    /// The text such a value is carried as is its JSON rendering, which is NOT always what JS
    /// coercion would produce (`{}` renders as `"{}"` here and `"[object Object]"` there). That
    /// difference is unobservable: no coercion of a non-string is a UUID, so both implementations
    /// refuse the login with the same message, and the value is only ever rendered into that
    /// refusal path. The JSON form is kept because it is the more useful diagnostic.
    pub fn for_stored(stored: Option<&Value>) -> Self {
        let Some(value) = stored.filter(|v| is_js_truthy(v)) else {
            return Self::Create;
        };
        let text = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        match InstallationId::from_str(&text) {
            Ok(id) => Self::Reuse(id),
            Err(_) => Self::KeepUnusable(text),
        }
    }
}

/// JavaScript truthiness for a `serde_json::Value`, as `if (!this.globalSettings.deviceId)` applies
/// it: `undefined`/`null`, `""`, `false`, `0` and `NaN` are falsy; every object, array and
/// non-empty string is truthy.
fn is_js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// The imperative shell around [`DeviceIdDecision`]: the `getDeviceId` callback a login flow calls,
/// plus the id that still has to be written.
///
/// Built by [`crate::settings::SettingsManager::installation_id`] from the loaded global layer, and
/// consumed by `cyrup_config::login::login`, which is the only production caller — so there is no
/// way for a front-end to run a login without one.
///
/// ## Why this is split in two, and what the split costs
///
/// A **mechanism** difference from upstream, stated plainly rather than tagged `[CYRUP-DELTA]`:
/// that tag is for a behavioural improvement at full parity, and what changes here is *when* the
/// write lands, which is a timing difference even though it is the favourable direction.
///
/// `GetDeviceIdFn` is a synchronous `Fn() -> String`, matching upstream's synchronous
/// `getOrCreateDeviceId()`. Upstream can persist from inside it because its `save()` only
/// *enqueues*: `save` snapshots the global document and chains a task onto `this.writeQueue`
/// (`settings-manager.ts:736-751` → `enqueueWrite`, `:683-696`), so the function returns before the
/// file is written and a write failure is swallowed into `recordError` rather than thrown. Its own
/// test has to `await first.flush()` before reading the file back
/// (`test/settings-manager.test.ts:122`).
///
/// cyrup's settings write is `async` (the scope lock is), and a sync callback cannot await. So the
/// callback returns the id immediately — exactly as upstream's does — and the write is performed by
/// `login`, awaited, immediately after the flow it was created for finishes.
///
/// What is gained: when `login` returns, the id is already durable, so the next process cannot see
/// a login that minted an id the file never received. Upstream needs `flush()` (`:777`,
/// `await this.writeQueue`) for that, and only its tests call it — in the app, a process that exits
/// between the callback returning and the queue draining loses the id and the next run mints a new
/// one. cyrup shrinks that window to the one write; it does not eliminate it, since a process can
/// still die inside `persist_installation_id`.
///
/// What is NOT lost: the callback is still synchronous and still returns the final id; the id is
/// still created only when a flow asks; a write failure still does not fail the login, matching
/// `recordError`. Nothing between the two points reads the file, so no caller can tell the
/// orderings apart.
pub struct InstallationIdSupplier {
    /// What the callback returns. For [`DeviceIdDecision::Create`] this is the freshly minted id;
    /// minting is pure computation and stays unobservable until `newly_created` is persisted.
    id: Option<InstallationId>,
    /// The exact text to hand back, including a `KeepUnusable` value that is not a UUID.
    value: String,
    /// Whether a login flow actually invoked the callback. `Arc` because the callback is `'static`
    /// and outlives the borrow this is read through.
    asked: Arc<AtomicBool>,
    /// Whether the id still has to be written (the `Create` arm).
    needs_write: bool,
}

impl InstallationIdSupplier {
    /// Mint on the [`DeviceIdDecision::Create`] arm; carry the stored value otherwise.
    pub fn new(decision: DeviceIdDecision) -> Self {
        let (id, value, needs_write) = match decision {
            DeviceIdDecision::Reuse(id) => {
                let value = id.as_str().to_string();
                (Some(id), value, false)
            }
            DeviceIdDecision::KeepUnusable(text) => (None, text, false),
            DeviceIdDecision::Create => {
                let id = InstallationId::generate();
                let value = id.as_str().to_string();
                (Some(id), value, true)
            }
        };
        Self {
            id,
            value,
            asked: Arc::new(AtomicBool::new(false)),
            needs_write,
        }
    }

    /// `getDeviceId: () => string` — the callback handed to
    /// `cyrup_provider::LoginOptions::with_device_id`.
    ///
    /// Synchronous and infallible, like upstream's. Calling it twice returns the same string: that
    /// is the whole contract of the upstream doc comment, "apps can create the ID on first use and
    /// must return the same ID on every later call".
    ///
    /// Invoking it is what records that this login needed an id, which is how
    /// [`Self::newly_created`] stays as lazy as upstream's single call site.
    pub fn callback(&self) -> impl Fn() -> String + Send + Sync + 'static {
        let value = self.value.clone();
        let asked = Arc::clone(&self.asked);
        move || {
            asked.store(true, Ordering::SeqCst);
            value.clone()
        }
    }

    /// The id that must be persisted to the GLOBAL scope, or `None` when there is nothing to write.
    ///
    /// `None` in three cases, each of which is a file upstream also leaves untouched:
    ///
    /// * a usable id was already stored ([`DeviceIdDecision::Reuse`] — upstream skips the `if`
    ///   body);
    /// * a truthy non-UUID was stored ([`DeviceIdDecision::KeepUnusable`] — likewise skipped);
    /// * no login flow asked for an id, so upstream's one call site never ran.
    pub fn newly_created(&self) -> Option<&InstallationId> {
        if !self.needs_write || !self.asked.load(Ordering::SeqCst) {
            return None;
        }
        self.id.as_ref()
    }

    /// Whether a flow invoked [`Self::callback`]. Exposed for the tests that pin the laziness.
    pub fn was_asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// The pure core's "already present" arm, with no settings file in sight.
    #[test]
    fn a_stored_uuid_is_reused_and_lowercased() {
        let stored = Value::String("B8C1F0DE-1111-4222-8333-444455556666".to_string());
        match DeviceIdDecision::for_stored(Some(&stored)) {
            DeviceIdDecision::Reuse(id) => {
                assert_eq!(id.as_str(), "b8c1f0de-1111-4222-8333-444455556666");
            }
            other => panic!("expected Reuse, got {other:?}"),
        }
    }

    /// The pure core's "absent" arm, including every JS-falsy spelling of absence.
    #[test]
    fn an_absent_or_falsy_value_asks_for_a_fresh_id() {
        for stored in [
            None,
            Some(Value::Null),
            Some(Value::String(String::new())),
            Some(Value::Bool(false)),
            Some(serde_json::json!(0)),
        ] {
            assert_eq!(
                DeviceIdDecision::for_stored(stored.as_ref()),
                DeviceIdDecision::Create,
                "{stored:?} is falsy upstream, so an id must be minted"
            );
        }
    }

    /// A truthy non-UUID is kept verbatim, never repaired — `!deviceId` is false for it, so
    /// upstream's generate-and-save branch does not run.
    #[test]
    fn a_truthy_non_uuid_is_kept_verbatim() {
        for stored in [
            Value::String("project-device".to_string()),
            Value::String("not-a-uuid".to_string()),
            serde_json::json!(42),
        ] {
            let text = match &stored {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            assert_eq!(
                DeviceIdDecision::for_stored(Some(&stored)),
                DeviceIdDecision::KeepUnusable(text),
                "{stored:?} is truthy upstream and must be returned unchanged"
            );
        }
    }

    /// `randomUUID()` emits the form `UUID_PATTERN` accepts, so the id cyrup mints round-trips
    /// through the provider's own check.
    #[test]
    fn a_generated_id_parses_as_a_uuid() {
        for _ in 0..32 {
            let id = InstallationId::generate();
            assert_eq!(id.as_str().len(), UUID_LEN);
            assert_eq!(
                InstallationId::from_str(id.as_str()).map(|p| p.as_str().to_string()),
                Ok(id.as_str().to_string())
            );
        }
    }

    /// Two mints differ: this is a random id, not a derivation of the machine.
    #[test]
    fn generated_ids_are_random_not_derived() {
        let ids: std::collections::HashSet<String> = (0..16)
            .map(|_| InstallationId::generate().as_str().to_string())
            .collect();
        assert_eq!(ids.len(), 16, "generate must not be a function of the host");
    }

    /// The newtype's parse rejects everything `UUID_PATTERN` rejects.
    #[test]
    fn the_newtype_refuses_a_non_uuid() {
        for bad in [
            "",
            "nope",
            "project-device",
            "b8c1f0de11114222833344445555666",
            "b8c1f0de-1111-4222-8333-44445555666g",
            "b8c1f0de_1111_4222_8333_444455556666",
            "b8c1f0de-1111-4222-8333-4444555566667",
        ] {
            assert_eq!(
                InstallationId::from_str(bad),
                Err(InstallationIdParseError),
                "{bad:?} is not a UUID"
            );
        }
    }

    /// The callback returns the same string every time, and only a call records that one was
    /// needed.
    #[test]
    fn the_callback_is_stable_and_records_that_a_flow_asked() {
        let supplier = InstallationIdSupplier::new(DeviceIdDecision::Create);
        assert!(!supplier.was_asked());
        assert_eq!(supplier.newly_created(), None, "nobody asked yet");

        let callback = supplier.callback();
        let first = callback();
        let second = callback();
        assert_eq!(first, second);
        assert!(supplier.was_asked());
        assert_eq!(
            supplier.newly_created().map(|i| i.as_str().to_string()),
            Some(first)
        );
    }

    /// Nothing to write on the two arms whose file upstream leaves untouched.
    #[test]
    fn a_reused_or_unusable_id_is_never_written_back() {
        let reuse = InstallationIdSupplier::new(DeviceIdDecision::Reuse(
            InstallationId::from_str("b8c1f0de-1111-4222-8333-444455556666").unwrap(),
        ));
        let callback = reuse.callback();
        assert_eq!(callback(), "b8c1f0de-1111-4222-8333-444455556666");
        assert_eq!(reuse.newly_created(), None);

        let unusable =
            InstallationIdSupplier::new(DeviceIdDecision::KeepUnusable("nope".to_string()));
        let callback = unusable.callback();
        assert_eq!(callback(), "nope", "handed back verbatim");
        assert_eq!(unusable.newly_created(), None);
    }
}
