//! The storage call context (ADR-0030 §10's `cx: &Cx`).
//!
//! Every method on [`Storage`] takes one, because every method may be in flight when the caller
//! gives up on it: `spec.md:4297-4331`'s interface threads a `Context` through all of them.
//!
//! # What it carries, and what it deliberately does not
//!
//! **Cancellation only.** ADR-0030 open question 6 records the reason: *"cyrup has no
//! value-carrying `Context`. The tree has 65 `CancellationToken` uses and nothing that carries
//! values."* So this type is the cancellation half, built on the workspace's one
//! `AbortSignal` equivalent (`cyrup-core/src/cancel.rs:9` aliases the same
//! `tokio_util::sync::CancellationToken`; this crate cannot name that alias because it must not
//! depend on `cyrup-core` — see [`crate`]'s module documentation).
//!
//! When a value-carrying context lands, the values belong **here**, not in a second parameter. Note
//! the direction that matters for S6: ADR-0030 §9.2's `FrameContext` is the type that must carry
//! the values *without* the token, so that a delivered frame cannot inherit the producer's
//! cancellation. `Cx` is the opposite end — it is the one that cancels.
//!
//! # Why it is not `CancellationToken` itself
//!
//! A bare token in the signature says *"this parameter is a cancellation token"*. A named context
//! says *"this is the call's context, which today cancels"*, which is what §10 means and what keeps
//! the value-carrying addition from being a signature change on every method of [`Storage`].
//!
//! [`Storage`]: crate::Storage

use tokio_util::sync::CancellationToken;

/// A storage call's context.
///
/// Cheap to clone (the token is an `Arc` internally), and deliberately **not** `Copy`: a context is
/// passed by reference through the trait, and cloning one is a statement that a spawned unit of
/// work shares this call's cancellation.
#[derive(Clone, Debug)]
pub struct Cx {
    cancel: CancellationToken,
}

impl Cx {
    /// A context that cancels when `cancel` does.
    #[must_use]
    pub const fn new(cancel: CancellationToken) -> Self {
        Self { cancel }
    }

    /// A context that is never cancelled.
    ///
    /// For a call with no caller to abandon it — a reference backend's unit test, a recovery pass
    /// at open — and named `detached` rather than `default` because [`Default`] on a context would
    /// make "no cancellation" the thing that happens when a caller forgets to thread one through.
    #[must_use]
    pub fn detached() -> Self {
        Self {
            cancel: CancellationToken::new(),
        }
    }

    /// Whether the caller has given up.
    ///
    /// A backend checks this between units of work it can abandon cleanly. It must **not** check it
    /// in the middle of a commit: `spec.md:4307-4311`'s two-class contract has no "cancelled"
    /// class, so a commit abandoned halfway is [`CommitError::Uncertain`], not a rejection.
    ///
    /// [`CommitError::Uncertain`]: crate::CommitError::Uncertain
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Wait until the caller gives up.
    pub async fn cancelled(&self) {
        self.cancel.cancelled().await;
    }

    /// The underlying token, for a backend that must hand it to an API that takes one.
    #[must_use]
    pub const fn token(&self) -> &CancellationToken {
        &self.cancel
    }
}
