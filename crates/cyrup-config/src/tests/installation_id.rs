//! PROV-118's supplier, asserted **through `cyrup_config::login::login`** — pi
//! `SettingsManager.getOrCreateDeviceId` (`packages/coding-agent/src/core/settings-manager.ts:1175-1182`
//! @v1.0.1), wired at `modes/interactive/interactive-mode.ts:6262`.
//!
//! Every test here drives the REAL "Sign in with ChatGPT" strategy
//! ([`OpenAiChatGptOAuth`]) through the REAL [`login`], because the defect this row was open for
//! was not a broken generator — it was a generator nothing called. `LoginOptions`,
//! `GetDeviceIdFn` and `with_device_id` all existed and were tested; the only `with_device_id`
//! caller in the tree was a test, so `/login openai` failed at its first statement with
//! "Sign in with ChatGPT requires a device ID (UUID) for this installation". A unit test of the
//! generator would still pass with the wiring removed. These do not.
//!
//! The pure arms of the decision ([`crate::settings::DeviceIdDecision`]) are unit-tested next to
//! the code, in `settings/installation.rs`.
//!
//! **No OAuth round trip, no browser, no socket.** The interaction carries an
//! already-cancelled token, and `CallbackServer::start` returns `OAuthError::Cancelled` before it
//! binds anything (`auth/oauth/callback.rs:305-311`) — so the attempt runs the device-id gate, the
//! PKCE/state draws, and then stops. Nothing listens, nothing dials out, and no browser is
//! involved at any point (nothing under `cyrup-provider/src/auth/` launches one; the front-end
//! does). `callback_port: 0` is set anyway, so even if that early return ever moved, these tests
//! could not contend for production port 1455.
//!
//! That the attempt ends in **cancellation** is itself the assertion: the gate is the first
//! statement of the flow and returns with `?`, so only a login that got past it can reach a
//! cancellable state — see [`openai_login_reaches_the_flow_past_the_device_id_check`].

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use cyrup_core::{CancelToken, ProviderId};
use cyrup_provider::auth::ProviderAuth;
use cyrup_provider::auth::oauth::ScriptedInteraction;
use cyrup_provider::auth::oauth::openai_chatgpt::{OpenAiChatGptEndpoints, OpenAiChatGptOAuth};
use serde_json::Value;

use crate::auth::{AuthStatus, AuthStore};
use crate::login::{AuthType, LoginError, ProviderLoginInput, login};
use crate::settings::{
    DEVICE_ID_KEY, FileSettingsStore, InstallationId, SettingsManager, SettingsScope,
};

/// `:228` — the message the flow refuses with when no usable installation id reaches it. Copied
/// rather than imported because upstream's constant is private to the flow; if it ever drifts,
/// [`openai_login_reaches_the_flow_past_the_device_id_check`] still fails, because its real
/// assertion is positive (the attempt got far enough to be cancelled).
const MISSING_DEVICE_ID_MESSAGE: &str =
    "Sign in with ChatGPT requires a device ID (UUID) for this installation";

/// The two settings files, laid out as upstream's own `deviceId` test does
/// (`packages/coding-agent/test/settings-manager.test.ts:110-127`): a global `settings.json` under
/// the agent dir and a project one under the project dir.
struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    fn new(name: &str) -> Self {
        Self {
            dir: tempfile::Builder::new()
                .prefix(&format!("cyrup-device-id-{name}-"))
                .tempdir()
                .unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn global_path(&self) -> std::path::PathBuf {
        self.root().join("settings.json")
    }

    fn project_path(&self) -> std::path::PathBuf {
        self.root().join("project-settings.json")
    }

    fn write_global(&self, text: &str) {
        std::fs::write(self.global_path(), text).unwrap();
    }

    fn write_project(&self, text: &str) {
        std::fs::write(self.project_path(), text).unwrap();
    }

    fn read_global(&self) -> Option<String> {
        std::fs::read_to_string(self.global_path()).ok()
    }

    fn global_json(&self) -> serde_json::Map<String, Value> {
        serde_json::from_str(&self.read_global().expect("global settings written")).unwrap()
    }

    /// A reader of the same two files — a *fresh* one each time, which is how the persistence
    /// assertions stand in for upstream's `SettingsManager.create(projectDir, agentDir)` in a
    /// second process.
    fn reader(&self) -> SettingsManager {
        SettingsManager::load(
            Arc::new(FileSettingsStore::new(
                self.global_path(),
                self.project_path(),
            )),
            true,
        )
    }

    /// The `auth.json` the credential half of `login` writes to. Never reached by these tests —
    /// the login always fails — but `login` must be given a real store.
    fn credentials(&self) -> AuthStore {
        AuthStore::at(self.root().join("auth.json"))
    }
}

/// The production `openai` OAuth strategy, with only the callback *port* moved to an ephemeral one.
///
/// Everything the device-id gate touches is untouched: `agent_host_id` runs on
/// `options.device_id()` before any listener exists (`openai-chatgpt.ts:237`, ahead of
/// `startCallbackServer`), so this is the real check, not a stand-in.
fn openai_provider() -> ProviderLoginInput {
    let endpoints = OpenAiChatGptEndpoints {
        callback_host: Some("127.0.0.1".to_string()),
        callback_port: 0,
        ..OpenAiChatGptEndpoints::default()
    };
    ProviderLoginInput {
        id: ProviderId::from("openai"),
        name: "OpenAI".to_string(),
        auth: ProviderAuth::with_oauth(Arc::new(OpenAiChatGptOAuth::with_endpoints(endpoints))),
        status: AuthStatus {
            configured: false,
            source: None,
            label: None,
        },
        using_oauth: false,
    }
}

/// Run `/login openai` against `settings` and return the error it ends with.
///
/// The interaction carries an already-cancelled token, so the flow is abandoned as soon as it
/// starts waiting for the browser. `login` is the real one; `settings` is the real
/// [`SettingsManager`], reading and writing the real files.
async fn attempt_openai_login(tree: &Tree, settings: &SettingsManager) -> LoginError {
    let cancel = CancelToken::new();
    cancel.cancel();
    let interaction = ScriptedInteraction::new(Vec::new()).with_cancel(cancel);
    login(
        &tree.credentials(),
        &[openai_provider()],
        &ProviderId::from("openai"),
        AuthType::Oauth,
        &interaction,
        settings,
    )
    .await
    .expect_err("a cancelled login cannot produce a credential")
}

/// The `deviceId` the global settings file now holds, parsed through the newtype so a non-UUID
/// fails here rather than silently passing.
fn stored_installation_id(tree: &Tree) -> InstallationId {
    let raw = tree.global_json();
    let text = raw
        .get(DEVICE_ID_KEY)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("global settings must carry a {DEVICE_ID_KEY}: {raw:?}"));
    InstallationId::from_str(text).unwrap_or_else(|e| panic!("{text:?} is not a UUID: {e}"))
}

// ---------------------------------------------------------------------------
// The headline: the login path gets past the device-id check
// ---------------------------------------------------------------------------

/// **PROV-118's whole reason for being open.** `/login` for `openai` must reach the flow instead of
/// dying at `agentHostId` (`openai-chatgpt.ts:237`).
///
/// The assertion is positive, not a `!contains`: `agent_host_id(...)?` is the FIRST statement of
/// `OpenAiChatGptOAuth::login` and returns with `?`, so it bypasses the `finally`-equivalent
/// `match` that rewrites an aborted login as `OAuthError::Cancelled` (`:285-287`). A
/// `LoginError::is_cancelled()` therefore **proves** control reached the cancellable part of the
/// flow — i.e. past the gate. Without a supplier the error is
/// [`MISSING_DEVICE_ID_MESSAGE`] and `is_cancelled()` is false.
///
/// It also proves the flow actually *asked*: the id is on disk afterwards, which only happens when
/// `options.device_id()` invoked the callback.
#[tokio::test]
async fn openai_login_reaches_the_flow_past_the_device_id_check() {
    let tree = Tree::new("headline");
    let settings = tree.reader();

    let error = attempt_openai_login(&tree, &settings).await;

    assert!(
        error.is_cancelled(),
        "login must get past the device-id gate and into the cancellable flow, got: {error}"
    );
    assert_ne!(
        error.to_string(),
        MISSING_DEVICE_ID_MESSAGE,
        "the gate that kept PROV-118 open must not fire"
    );
    // The flow asked for an id, so one was minted and persisted.
    let _ = stored_installation_id(&tree);
}

/// The id reaches the flow in the form it needs — a UUID that the provider's own
/// `agent_host_id` accepts and lowercases into `urn:uuid:<uuid>`.
///
/// Asserted on the value the login path *delivered*, not on what the generator returned: the
/// supplier is built inside `login`, so what lands on disk is what the callback handed the flow.
#[tokio::test]
async fn the_id_the_flow_received_is_a_uuid() {
    let tree = Tree::new("uuid-shape");
    let settings = tree.reader();

    let error = attempt_openai_login(&tree, &settings).await;
    assert!(error.is_cancelled(), "{error}");

    let id = stored_installation_id(&tree);
    assert_eq!(id.as_str().len(), 36);
    assert_eq!(
        id.as_str(),
        id.as_str().to_lowercase(),
        "persisted lowercase, the form `ext_agent_host_id` transmits"
    );
}

// ---------------------------------------------------------------------------
// Stability
// ---------------------------------------------------------------------------

/// `return this.globalSettings.deviceId` with the `if` body skipped (`:1176`, `:1181`): a second
/// login reuses the id, and — because upstream's generate-and-save branch does not run — the
/// settings file is **not rewritten**.
///
/// The file is seeded with deliberately non-canonical formatting (4-space indent, no trailing
/// newline). cyrup writes `to_pretty()`, which is 2-space with a trailing newline, so any write at
/// all is visible as a byte difference.
#[tokio::test]
async fn a_second_login_reuses_the_id_and_does_not_rewrite_the_file() {
    let tree = Tree::new("stable");
    let settings = tree.reader();

    let error = attempt_openai_login(&tree, &settings).await;
    assert!(error.is_cancelled(), "{error}");
    let first = stored_installation_id(&tree);

    // Rewrite the same content in a shape cyrup would never emit, so a second write is detectable.
    let seeded = format!("{{\n    \"{DEVICE_ID_KEY}\": \"{first}\"\n}}");
    tree.write_global(&seeded);

    // A fresh reader, as the next process would have.
    let settings = tree.reader();
    let error = attempt_openai_login(&tree, &settings).await;
    assert!(error.is_cancelled(), "{error}");

    assert_eq!(
        stored_installation_id(&tree),
        first,
        "the second login must hand the flow the SAME installation id"
    );
    assert_eq!(
        tree.read_global().as_deref(),
        Some(seeded.as_str()),
        "an id that is already present must not cause a write: the file must be byte-identical"
    );
}

/// Two reads inside one login see the same value — the upstream contract, "apps can create the ID
/// on first use and must return the same ID on every later call" (`auth/types.ts:211-213`).
///
/// Driven through the supplier `login` itself builds, via the public
/// [`SettingsManager::installation_id`] seam, so this is the same construction path.
#[tokio::test]
async fn the_supplier_returns_one_value_however_often_it_is_called() {
    let tree = Tree::new("idempotent");
    let settings = tree.reader();
    let supplier = settings.installation_id();
    let callback = supplier.callback();

    let values: std::collections::HashSet<String> = (0..8).map(|_| callback()).collect();
    assert_eq!(values.len(), 1, "getDeviceId must be stable: {values:?}");
}

/// `SettingsManager.create(projectDir, agentDir).getOrCreateDeviceId()` in a later process returns
/// the same id (`test/settings-manager.test.ts:126`). The id must survive the process that minted
/// it, or every login looks like a new install to OpenAI.
#[tokio::test]
async fn the_id_survives_a_fresh_reader_of_the_same_directory() {
    let tree = Tree::new("persist");

    let error = attempt_openai_login(&tree, &tree.reader()).await;
    assert!(error.is_cancelled(), "{error}");
    let minted = stored_installation_id(&tree);

    // A brand-new manager over the same files — nothing shared with the first but the directory.
    let next = tree.reader();
    let callback = next.installation_id().callback();
    assert_eq!(
        callback(),
        minted.as_str(),
        "a later process must reuse the persisted installation id"
    );

    // And a second login through that fresh reader hands the flow the same id.
    let error = attempt_openai_login(&tree, &next).await;
    assert!(error.is_cancelled(), "{error}");
    assert_eq!(stored_installation_id(&tree), minted);
}

// ---------------------------------------------------------------------------
// Layer
// ---------------------------------------------------------------------------

/// **GLOBAL, not project.** Upstream reads `this.globalSettings.deviceId`, never the merged
/// `this.settings`, and says why: "Project settings are ignored so a committed project settings
/// file cannot give every clone the same ID" (`:1172-1174`). Its own test seeds
/// `{"deviceId":"project-device"}` into the project file and asserts a generated UUID wins
/// (`test/settings-manager.test.ts:113-127`).
///
/// This is not cosmetic: `deviceId` identifies one *installation*. Honouring a project layer would
/// give one machine as many identities as it has checkouts, and would leak one project's id into
/// every other clone of it.
#[tokio::test]
async fn the_id_lands_in_the_global_layer_and_a_project_one_is_ignored() {
    let tree = Tree::new("global-only");
    tree.write_global("{\"theme\":\"dark\"}");
    let project = format!("{{\"{DEVICE_ID_KEY}\":\"project-device\"}}");
    tree.write_project(&project);

    let settings = tree.reader();
    assert!(
        settings.project().device_id().is_some(),
        "the project layer really does carry a deviceId, so the test is not vacuous"
    );

    let error = attempt_openai_login(&tree, &settings).await;
    assert!(error.is_cancelled(), "{error}");

    let minted = stored_installation_id(&tree);
    assert_ne!(
        minted.as_str(),
        "project-device",
        "a project settings file must not be able to supply the installation id"
    );
    assert_eq!(
        std::fs::read_to_string(tree.project_path()).unwrap(),
        project,
        "the project file must not be written to at all"
    );
}

/// The write goes to the GLOBAL scope even when the project scope is untrusted — the scope that
/// would refuse a write ([`crate::error::ConfigError::Untrusted`]) is not the one involved.
#[tokio::test]
async fn the_id_is_created_even_when_the_project_is_untrusted() {
    let tree = Tree::new("untrusted");
    let settings = SettingsManager::load(
        Arc::new(FileSettingsStore::new(
            tree.global_path(),
            tree.project_path(),
        )),
        false,
    );

    let error = attempt_openai_login(&tree, &settings).await;
    assert!(error.is_cancelled(), "{error}");
    let _ = stored_installation_id(&tree);
}

// ---------------------------------------------------------------------------
// Not corrupting the user's settings
// ---------------------------------------------------------------------------

/// `expect(JSON.parse(readFileSync(settingsPath))).toEqual({ theme: "dark", deviceId })`
/// (`test/settings-manager.test.ts:127`): a first login ADDS one key and changes nothing else.
///
/// Seeded with a nested block and an unknown key as well, because the write goes through
/// `persist_nested`'s scoped read-modify-write (R-07-004) and a whole-document rewrite from the
/// in-memory view is exactly the failure this guards.
#[tokio::test]
async fn a_first_login_preserves_every_unrelated_setting() {
    let tree = Tree::new("preserve");
    let seeded = serde_json::json!({
        "theme": "dark",
        "defaultModel": "gpt-5",
        "terminal": { "showImages": true, "wheelScrollLines": 3 },
        "somethingCyrupDoesNotModel": ["keep", "me"],
    });
    tree.write_global(&serde_json::to_string_pretty(&seeded).unwrap());

    let error = attempt_openai_login(&tree, &tree.reader()).await;
    assert!(error.is_cancelled(), "{error}");

    let after = tree.global_json();
    let minted = stored_installation_id(&tree);

    let mut expected = seeded.as_object().unwrap().clone();
    expected.insert(
        DEVICE_ID_KEY.to_string(),
        Value::String(minted.as_str().to_string()),
    );
    assert_eq!(
        after, expected,
        "a first login must add exactly `{DEVICE_ID_KEY}` and leave every other key untouched"
    );
}

/// CFG-001, inherited from [`SettingsManager::persist_nested`]: a global file cyrup could not parse
/// is never rewritten, and the login still runs rather than being blocked by it.
///
/// Upstream matches on both counts — `save()` returns early when `globalSettingsLoadError` is set
/// (`:739-741`), and `getOrCreateDeviceId` returns the id it generated regardless, so the flow
/// proceeds with an id that is valid for this process and simply not durable.
#[tokio::test]
async fn an_unparseable_global_file_is_not_rewritten_and_does_not_block_the_login() {
    let tree = Tree::new("corrupt");
    let broken = "{ \"theme\": \"dark\", }";
    tree.write_global(broken);

    let error = attempt_openai_login(&tree, &tree.reader()).await;
    assert!(
        error.is_cancelled(),
        "an unwritable settings file must not make /login unusable, got: {error}"
    );
    assert_eq!(
        tree.read_global().as_deref(),
        Some(broken),
        "a file cyrup could not parse must be left byte-for-byte alone"
    );
}

// ---------------------------------------------------------------------------
// A malformed persisted id
// ---------------------------------------------------------------------------

/// A stored `deviceId` that is not a UUID is **kept verbatim and the login is refused** — upstream's
/// behaviour, matched deliberately.
///
/// `if (!this.globalSettings.deviceId)` is false for any truthy value, so upstream never reaches
/// `randomUUID()`: it returns `"not-a-uuid"`, and `UUID_PATTERN.test` then fails in `agentHostId`,
/// which is the same refusal a missing id produces. Reminting here would be a double divergence —
/// it would destroy a value cyrup did not write, and it would make a login SUCCEED where pi's
/// fails.
///
/// So this is the one case where `/login openai` still reports the PROV-118 message, and that is
/// correct: the fix is for the user to remove the bad value, which the next login then mints.
#[tokio::test]
async fn a_malformed_persisted_id_is_kept_and_the_login_is_refused() {
    let tree = Tree::new("malformed");
    let seeded = format!("{{\"theme\":\"dark\",\"{DEVICE_ID_KEY}\":\"not-a-uuid\"}}");
    tree.write_global(&seeded);

    let error = attempt_openai_login(&tree, &tree.reader()).await;

    assert_eq!(
        error.to_string(),
        MISSING_DEVICE_ID_MESSAGE,
        "a truthy non-UUID is handed to the flow unchanged, and the flow refuses it"
    );
    assert!(
        !error.is_cancelled(),
        "the refusal happens at the gate, before the cancellable part of the flow"
    );
    assert_eq!(
        tree.read_global().as_deref(),
        Some(seeded.as_str()),
        "upstream never overwrites a truthy deviceId, so neither does cyrup"
    );

    // Clearing the bad value is all it takes; the next login mints a usable one.
    tree.write_global("{\"theme\":\"dark\"}");
    let error = attempt_openai_login(&tree, &tree.reader()).await;
    assert!(error.is_cancelled(), "{error}");
    let _ = stored_installation_id(&tree);
}

// ---------------------------------------------------------------------------
// Laziness
// ---------------------------------------------------------------------------

/// "Called only by login flows that need it, so apps can create the ID on first use"
/// (`auth/types.ts:211-213`). Upstream passes `getDeviceId` for **every** provider, but as a
/// callback, so logging in to a provider whose flow does not need an installation id writes
/// nothing.
///
/// Uses a real `ProviderAuth` whose OAuth strategy never reads `options`, which is every flow but
/// `openai`'s.
#[tokio::test]
async fn a_flow_that_never_asks_leaves_the_settings_file_alone() {
    let tree = Tree::new("lazy");
    let seeded = "{\"theme\":\"dark\"}";
    tree.write_global(seeded);

    let settings = tree.reader();
    let providers = vec![ProviderLoginInput {
        id: ProviderId::from("anthropic"),
        name: "Anthropic".to_string(),
        auth: ProviderAuth::with_oauth(Arc::new(IndifferentOauth)),
        status: AuthStatus {
            configured: false,
            source: None,
            label: None,
        },
        using_oauth: false,
    }];
    let interaction = ScriptedInteraction::new(Vec::new());

    login(
        &tree.credentials(),
        &providers,
        &ProviderId::from("anthropic"),
        AuthType::Oauth,
        &interaction,
        &settings,
    )
    .await
    .expect("the indifferent flow succeeds");

    assert_eq!(
        tree.read_global().as_deref(),
        Some(seeded),
        "no flow asked for an installation id, so none may be minted or written"
    );
    assert!(
        settings.global().device_id().is_none(),
        "and none appears in the global layer"
    );
}

/// An OAuth strategy that ignores `LoginOptions` entirely — the shape of every flow but
/// `openai`'s.
struct IndifferentOauth;

#[async_trait::async_trait]
impl cyrup_provider::auth::OAuthAuth for IndifferentOauth {
    fn name(&self) -> &str {
        "Anthropic (Claude Pro/Max)"
    }

    async fn login(
        &self,
        _interaction: &dyn cyrup_provider::auth::oauth::AuthInteraction,
        _options: &cyrup_provider::LoginOptions,
    ) -> Result<cyrup_provider::Credential, cyrup_provider::auth::oauth::OAuthError> {
        Ok(cyrup_provider::Credential::Oauth {
            refresh: "rt".into(),
            access: "at".into(),
            expires: 1_700_000_000_000,
            ext: serde_json::Map::new(),
        })
    }

    async fn refresh(
        &self,
        cred: &cyrup_provider::Credential,
    ) -> Result<cyrup_provider::Credential, cyrup_provider::AuthError> {
        Ok(cred.clone())
    }

    async fn to_auth(
        &self,
        _cred: &cyrup_provider::Credential,
    ) -> Result<cyrup_provider::ModelAuth, cyrup_provider::AuthError> {
        Ok(cyrup_provider::ModelAuth::default())
    }
}

/// Belt and braces on the scope question, at the layer where it is decided: the reader takes the
/// GLOBAL document, so a project value cannot reach it even when the merged view carries one.
///
/// Also pins that `deviceId` is deliberately NOT in `GLOBAL_ONLY_KEYS` — the merged view still
/// shows the project value, exactly as upstream's `this.settings` does, because nothing reads it
/// from there.
#[test]
fn the_reader_takes_the_global_document_not_the_merged_view() {
    let store = Arc::new(crate::settings::InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, "{\"theme\":\"dark\"}");
    store.seed(
        SettingsScope::Project,
        &format!("{{\"{DEVICE_ID_KEY}\":\"project-device\"}}"),
    );
    let settings = SettingsManager::load(store, true);

    assert!(settings.global().device_id().is_none());
    assert_eq!(
        settings.project().device_id().and_then(Value::as_str),
        Some("project-device")
    );
    assert_eq!(
        settings
            .effective()
            .raw()
            .device_id()
            .and_then(Value::as_str),
        Some("project-device"),
        "the MERGED view still carries it, as upstream's `this.settings` does — the restriction \
         lives in the reader, which is why `deviceId` is not a GLOBAL_ONLY_KEY"
    );

    // And the supplier, which reads the global document, mints instead of honouring it.
    let supplier = settings.installation_id();
    let callback = supplier.callback();
    assert_ne!(callback(), "project-device");
    assert!(InstallationId::from_str(&callback()).is_ok());
}
