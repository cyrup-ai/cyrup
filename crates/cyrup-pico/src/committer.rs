//! ADR-0030 F1's actor and ticket: the one place [`SessionMut`] lives, and a clonable handle onto it
//! that cannot be given the Session back.
//!
//! # Why the loop's future is `!Send`, and what a host does about it
//!
//! [`crate::Tx`] is `!Send` by construction (`PhantomData<*const ()>`, ADR-0030 §10), so the
//! future of a commit that holds one is `!Send` too, and so is [`Line::run`]. That is a real
//! consequence and not an oversight — PICO5-PLAN S0's `send_variant` module measured it: `'static`
//! alone is enough to stop a `Tx`-borrowing future being spawned, and what the `!Send` marker
//! additionally buys is exactly this, *"a `!Send` commit future, which F1 puts inside an actor"*.
//!
//! So the line is driven rather than spawned: a host runs [`Line::run`] on a
//! [`tokio::task::LocalSet`], on a dedicated thread with a current-thread runtime, or inline in its
//! own main loop. Nothing in this crate calls `tokio::spawn`, which is also why its `tokio` feature
//! list is `sync` alone.
//!
//! # Where the guarantee weakens, stated rather than hidden
//!
//! F1 is explicit: *"At the `Committer` boundary the guarantee is `checked`, not consuming, and this
//! document says so rather than claiming the consuming form while shipping an `Arc`."* A ticket
//! cannot be consumed, so its holder learns the Session is dead from
//! [`CommitReply::SessionDead`] — or, for every later call, from a closed channel, because the
//! receiver is dropped when the loop returns. That is weaker, and the failure is weaker too: a ticket
//! holder cannot reach adoption, publication or any in-memory baseline, so it can receive an error
//! but cannot cause divergence.
//!
//! # The nested commit, precisely
//!
//! `Tx` has no `commit` method and holds no [`Session`](crate::Session) — so the hazard upstream does
//! not even diagnose (a nested `session.commit()` queues on `#tail` and deadlocks permanently) has no
//! spelling through the transaction, and
//! `tests/compile-fail/a_nested_commit_cannot_be_started_from_the_transaction.rs` pins that. A
//! callback that captured a [`Committer`] from its **environment** is the residue ADR-0030 §2.1 calls
//! `guarded` rather than `unrepresentable`, and [`crate::hold`] is its runtime check. This crate does
//! not claim more than that.

use core::future::Future;
use core::pin::Pin;

use cyrup_pico_store::Cx;
use tokio::sync::{mpsc, oneshot};

use crate::outcome::{CommitOutcome, CommitReply};
use crate::session::SessionMut;
use crate::tx::{CallbackError, Reading, Tx, Writing};

/// What one job does, once, with the mutation handle.
///
/// `Option<SessionMut>` is the shape that matters: the handle comes back only when the commit gave it
/// back, so `None` *is* `CommitOutcome::Uncertain` one level up and there is nothing for the loop to
/// continue with.
///
/// The future is **not** `Send`, because a commit holds a `Tx` and a `Tx` is `!Send` by construction.
/// The boxed closure is, which is what lets a [`Job`] cross the channel while the work it does stays on
/// the line's own thread.
type LineWork = Pin<Box<dyn Future<Output = Option<SessionMut>>>>;

/// One unit of work for the line.
pub(crate) struct Job {
    run: Box<dyn FnOnce(SessionMut) -> LineWork + Send + 'static>,
}

/// Why a ticket call did not reach the line.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error("the Session mutation line is closed")]
pub struct LineClosed;

/// A clonable ticket onto one Session's mutation line.
///
/// Holding one does not let its holder read, adopt, publish or be handed a [`SessionMut`]: the only
/// thing it can do is submit a change and await a [`CommitReply`], none of whose variants carries a
/// handle.
#[derive(Clone, Debug)]
pub struct Committer(mpsc::Sender<Job>);

impl Committer {
    /// Submit a change and await its reply.
    ///
    /// # Errors
    ///
    /// [`LineClosed`] when the line has already returned — which is what every ticket holder other
    /// than the one that killed it observes. The one whose commit was fatal is told directly, through
    /// [`CommitReply::SessionDead`].
    pub async fn commit<R, F>(&self, cx: Cx, change: F) -> Result<CommitReply<R>, LineClosed>
    where
        R: Send + 'static,
        F: for<'tx> AsyncFnOnce(Tx<'tx, Reading>) -> Result<(R, Tx<'tx, Writing>), CallbackError>
            + Send
            + 'static,
    {
        let (reply, answer) = oneshot::channel();
        let job = Job {
            run: Box::new(move |session| {
                Box::pin(async move {
                    match session.commit(&cx, change).await {
                        CommitOutcome::NothingToCommit { result, session } => {
                            let _ = reply.send(CommitReply::NothingToCommit(result));
                            Some(session)
                        }
                        CommitOutcome::Committed {
                            result,
                            seq,
                            session,
                        } => {
                            let _ = reply.send(CommitReply::Committed { result, seq });
                            Some(session)
                        }
                        CommitOutcome::RolledBack { reason, session } => {
                            let _ = reply.send(CommitReply::RolledBack(reason));
                            Some(session)
                        }
                        CommitOutcome::Uncertain(u) => {
                            let _ = reply.send(CommitReply::SessionDead(u));
                            None
                        }
                    }
                })
            }),
        };
        self.0.send(job).await.map_err(|_| LineClosed)?;
        answer.await.map_err(|_| LineClosed)
    }

    /// Whether the line has returned.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

/// The mutation line: the **only** place a [`SessionMut`] lives.
///
/// Constructed by [`Line::new`], which hands back the ticket factory, and consumed by [`Line::run`],
/// which is the imperative shell.
#[derive(Debug)]
pub struct Line {
    rx: mpsc::Receiver<Job>,
    session: SessionMut,
}

impl Line {
    /// Wrap a mutation handle in a line, with a bounded queue.
    ///
    /// Bounded rather than unbounded deliberately: an unbounded queue turns a line that has stopped
    /// draining into unbounded memory growth with no signal, while a bounded one makes the caller's
    /// `send` wait — which is the backpressure `spec.md:1521`'s single mutation line implies.
    #[must_use]
    pub fn new(session: SessionMut, queue: usize) -> (Committer, Self) {
        let (tx, rx) = mpsc::channel(queue.max(1));
        (Committer(tx), Self { rx, session })
    }

    /// Drive the line until it closes or dies.
    ///
    /// Returns `Some(handle)` when every [`Committer`] has been dropped — an ordinary shutdown, after
    /// which the host calls [`SessionMut::close`] — and `None` when a commit was fatal. In the fatal
    /// case the handle **was moved into the commit and not returned**: there is nothing to put back,
    /// so the loop cannot continue even if someone wanted it to, and dropping `rx` on the way out
    /// closes every later ticket call (`G-INV-8` at the actor). The `Option` is the same guarantee one
    /// level up: a host that wants the Session back has to handle not getting it.
    ///
    /// The returned future is `!Send` — see this module's documentation.
    pub async fn run(self) -> Option<SessionMut> {
        let Self { mut rx, session } = self;
        let mut session = session;
        while let Some(job) = rx.recv().await {
            match (job.run)(session).await {
                Some(back) => session = back,
                None => return None,
            }
        }
        Some(session)
    }
}
