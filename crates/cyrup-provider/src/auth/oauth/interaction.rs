//! The login-time conversation between a flow and the front-end.
//!
//! Ports pi v0.83.0 `packages/ai/src/auth/types.ts:119-187` — `AuthPrompt`, `AuthInfoLink`,
//! `AuthEvent` and `AuthInteraction`. Upstream keeps these in `auth/types.ts` because api-key
//! logins use them too; they live under `oauth/` here purely because `auth/types.rs` is outside
//! this change's blast radius (see `not_done`), and the intent is that they move up unchanged.
//!
//! `AbortSignal` maps to [`CancelToken`] — cyrup's single cancellation mechanism (arch-00 §3.2).
//! Both the whole-login signal (`AuthInteraction.signal`, `types.ts:151`) and the per-prompt one
//! (`AuthPrompt.signal`, `types.ts:119`) are modelled, because `openrouter.ts:273-283` depends on
//! cancelling *one prompt* while the login continues.

use super::OAuthError;
use cyrup_core::CancelToken;

/// The kind of answer a prompt wants (`types.ts:120-124`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthPromptKind {
    /// `{ type: "text" }` — echoed free text.
    Text,
    /// `{ type: "secret" }` — masked entry (API keys).
    Secret,
    /// `{ type: "select" }` — pick one of `options`; the answer is the option **id**
    /// (`types.ts:156`).
    Select,
    /// `{ type: "manual_code" }` — paste an authorization code or the full redirect URL, the
    /// headless escape hatch raced against the callback server (`openrouter.ts:274-279`).
    ManualCode,
}

/// One selectable option (`types.ts:122`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthSelectOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

/// A prompt shown to the user during login (`AuthPrompt`, `types.ts:119-124`).
#[derive(Clone, Debug, Default)]
pub struct AuthPrompt {
    pub kind: Option<AuthPromptKind>,
    pub message: String,
    pub placeholder: Option<String>,
    /// Only meaningful for [`AuthPromptKind::Select`].
    pub options: Vec<AuthSelectOption>,
    /// Cancels **this prompt** without cancelling the login (`AuthPrompt.signal`,
    /// `types.ts:119`). A cancelled prompt resolves as [`OAuthError::Cancelled`].
    pub cancel: Option<CancelToken>,
}

impl AuthPrompt {
    pub fn text(message: impl Into<String>) -> Self {
        Self {
            kind: Some(AuthPromptKind::Text),
            message: message.into(),
            ..Default::default()
        }
    }
    pub fn secret(message: impl Into<String>) -> Self {
        Self {
            kind: Some(AuthPromptKind::Secret),
            message: message.into(),
            ..Default::default()
        }
    }
    pub fn select(message: impl Into<String>, options: Vec<AuthSelectOption>) -> Self {
        Self {
            kind: Some(AuthPromptKind::Select),
            message: message.into(),
            options,
            ..Default::default()
        }
    }
    pub fn manual_code(message: impl Into<String>) -> Self {
        Self {
            kind: Some(AuthPromptKind::ManualCode),
            message: message.into(),
            ..Default::default()
        }
    }
    #[must_use]
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }
    #[must_use]
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

/// A link rendered beside an `info` event (`AuthInfoLink`, `types.ts:126-129`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthInfoLink {
    pub url: String,
    pub label: Option<String>,
}

/// Out-of-band login progress (`AuthEvent`, `types.ts:131-140`).
#[derive(Clone, Debug, PartialEq)]
pub enum AuthEvent {
    Info {
        message: String,
        links: Vec<AuthInfoLink>,
    },
    /// The URL the user must open. Flows emit this *before* blocking on the callback
    /// (`openrouter.ts:268-272`).
    AuthUrl {
        url: String,
        instructions: Option<String>,
    },
    /// RFC 8628 user code + verification URI, plus the server's polling hints.
    DeviceCode {
        user_code: String,
        verification_uri: String,
        interval_seconds: Option<f64>,
        expires_in_seconds: Option<f64>,
    },
    Progress {
        message: String,
    },
}

/// Login interaction callbacks (`AuthInteraction`, `types.ts:150-155`), serving both api-key and
/// OAuth flows.
///
/// `prompt` rejects on cancel/abort — [`OAuthError::Cancelled`], whose message is upstream's
/// `"Login cancelled"`.
#[async_trait::async_trait]
pub trait AuthInteraction: Send + Sync {
    /// Aborts the **whole** login (`AuthInteraction.signal`, `types.ts:151`).
    fn cancel(&self) -> Option<&CancelToken> {
        None
    }

    /// Ask the user something. Returns the entered text, or the selected option's id.
    async fn prompt(&self, prompt: AuthPrompt) -> Result<String, OAuthError>;

    /// Report progress. Never blocks and never fails — upstream's `notify` returns `void`.
    fn notify(&self, event: AuthEvent);
}

/// A scripted [`AuthInteraction`] for tests: answers prompts from a queue, in order, and records
/// every prompt and event.
///
/// Public (not `#[cfg(test)]`) because the per-provider flow modules that build on this substrate
/// live in other files and need it to test a login end-to-end without a human.
pub struct ScriptedInteraction {
    answers: std::sync::Mutex<std::collections::VecDeque<Result<String, OAuthError>>>,
    prompts: std::sync::Mutex<Vec<AuthPrompt>>,
    events: std::sync::Mutex<Vec<AuthEvent>>,
    cancel: Option<CancelToken>,
    /// When set, `prompt` waits for its prompt-level cancel token instead of answering — the
    /// `manual_code` prompt that loses the race against the callback server.
    block_when_empty: bool,
}

impl ScriptedInteraction {
    /// Answers, consumed in order. A prompt with no answer left fails as
    /// [`OAuthError::Failed`] unless [`Self::blocking_when_empty`] is set.
    pub fn new(answers: Vec<Result<String, OAuthError>>) -> Self {
        Self {
            answers: std::sync::Mutex::new(answers.into_iter().collect()),
            prompts: std::sync::Mutex::new(Vec::new()),
            events: std::sync::Mutex::new(Vec::new()),
            cancel: None,
            block_when_empty: false,
        }
    }

    /// Exhausted prompts block until their own cancel token fires, modelling a user who never
    /// answers.
    #[must_use]
    pub fn blocking_when_empty(mut self) -> Self {
        self.block_when_empty = true;
        self
    }

    #[must_use]
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn prompts(&self) -> Vec<AuthPrompt> {
        self.prompts.lock().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn events(&self) -> Vec<AuthEvent> {
        self.events.lock().map(|e| e.clone()).unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl AuthInteraction for ScriptedInteraction {
    fn cancel(&self) -> Option<&CancelToken> {
        self.cancel.as_ref()
    }

    async fn prompt(&self, prompt: AuthPrompt) -> Result<String, OAuthError> {
        let prompt_cancel = prompt.cancel.clone();
        if let Ok(mut prompts) = self.prompts.lock() {
            prompts.push(prompt);
        }
        let next = self.answers.lock().ok().and_then(|mut a| a.pop_front());
        match next {
            Some(answer) => answer,
            None if self.block_when_empty => {
                match prompt_cancel {
                    Some(token) => {
                        token.cancelled().await;
                        Err(OAuthError::Cancelled)
                    }
                    // No prompt-level token: wait on the login-level one, else hang forever
                    // would deadlock the test, so fail loudly instead.
                    None => match self.cancel.clone() {
                        Some(token) => {
                            token.cancelled().await;
                            Err(OAuthError::Cancelled)
                        }
                        None => Err(OAuthError::Failed(
                            "ScriptedInteraction: no answers left and no cancel token".to_string(),
                        )),
                    },
                }
            }
            None => Err(OAuthError::Failed(
                "ScriptedInteraction: no answer scripted for this prompt".to_string(),
            )),
        }
    }

    fn notify(&self, event: AuthEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
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

    #[tokio::test]
    async fn answers_prompts_in_order_and_records_events() {
        let interaction =
            ScriptedInteraction::new(vec![Ok("sk-test".to_string()), Ok("gpt".into())]);
        interaction.notify(AuthEvent::Progress {
            message: "Listening".into(),
        });
        assert_eq!(
            interaction
                .prompt(AuthPrompt::secret("key?"))
                .await
                .unwrap(),
            "sk-test"
        );
        assert_eq!(
            interaction
                .prompt(AuthPrompt::select(
                    "model?",
                    vec![AuthSelectOption {
                        id: "gpt".into(),
                        label: "GPT".into(),
                        description: None,
                    }]
                ))
                .await
                .unwrap(),
            "gpt"
        );
        let prompts = interaction.prompts();
        assert_eq!(prompts[0].kind, Some(AuthPromptKind::Secret));
        assert_eq!(prompts[1].kind, Some(AuthPromptKind::Select));
        assert_eq!(
            interaction.events(),
            vec![AuthEvent::Progress {
                message: "Listening".into()
            }]
        );
    }

    /// The `openrouter.ts:273-283` shape: a `manual_code` prompt cancelled by its own token while
    /// the login itself is untouched.
    #[tokio::test]
    async fn per_prompt_cancel_resolves_as_login_cancelled() {
        let interaction = ScriptedInteraction::new(Vec::new()).blocking_when_empty();
        let token = CancelToken::new();
        let prompt = AuthPrompt::manual_code("paste the redirect URL").with_cancel(token.clone());
        let handle = tokio::spawn(async move {
            let interaction = interaction;
            interaction.prompt(prompt).await
        });
        token.cancel();
        let err = handle.await.unwrap().unwrap_err();
        assert_eq!(err.to_string(), "Login cancelled");
    }
}
