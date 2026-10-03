//! The one failure type of the crate, shared by the llama-server client ([`crate::client`]) and the
//! Hugging Face client ([`crate::huggingface`]).
//!
//! Upstream throws plain `Error`s from both `client.ts` and `huggingface.ts`, and the only thing
//! the extension ever reads off them is the message (`index.ts:11-20` matches `fetch failed`,
//! `timeout` and `network` to classify a connection error; `ui.ts` prints it). Two enums with the
//! same variants and the same texts would be two spellings of that one `Error`, so there is one.

/// Failure of a llama-server or Hugging Face call.
///
/// Every variant's [`Display`](std::fmt::Display) text matches what the JS runtime would put in
/// the thrown `Error`'s message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlamaError {
    /// The caller's cancellation token fired. Upstream surfaces the `AbortSignal`'s reason, which
    /// for a bare `abort()` is the DOMException "This operation was aborted".
    #[error("This operation was aborted")]
    Cancelled,
    /// The 15 s request timeout elapsed (`client.ts:174`, `huggingface.ts:75`); Node's
    /// `TimeoutError` message.
    #[error("The operation was aborted due to timeout")]
    Timeout,
    /// The request never produced a response (refused, reset, DNS, TLS). Node's `fetch` rejects
    /// with `TypeError: fetch failed`; the underlying cause is appended for diagnosis.
    #[error("{0}")]
    Transport(String),
    /// A server-reported or validation error whose text is the whole message (`client.ts:48-54`,
    /// `:183`, `:190`, `:193`, `:233`, `:293`; `huggingface.ts:88-95`, `:109`, `:122`).
    #[error("{0}")]
    Message(String),
}

impl LlamaError {
    pub(crate) fn message(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }

    /// A `reqwest` failure with its URL left out: the error's `Display` embeds the request URL, and
    /// that text reaches `/login` failures and `/llama` notifications.
    pub(crate) fn from_reqwest(error: reqwest::Error) -> Self {
        Self::transport(&error.without_url())
    }

    pub(crate) fn transport(error: &(dyn std::error::Error + 'static)) -> Self {
        let mut text = String::from("fetch failed");
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
        while let Some(cause) = source {
            let part = cause.to_string();
            if !part.is_empty() && !text.contains(&part) {
                text.push_str(": ");
                text.push_str(&part);
            }
            source = cause.source();
        }
        Self::Transport(text)
    }
}
