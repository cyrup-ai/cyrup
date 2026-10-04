//! `callToolPausingForElicitation`'s deadline (`elicitation-handler.ts:38-121`) — MCP-606 and
//! MCP-607.
//!
//! # What upstream does, and why cyrup cannot simply set a flag
//!
//! A server's `elicitation/create` arrives as its **own** request, and transports do not reliably
//! tie it to the `tools/call` that triggered it, so an open prompt pauses the deadline of every
//! tool call in flight on that client:
//!
//! ```text
//! interface PromptPause { open: number; deadlines: Set<{ pause(): void; resume(): void }> }
//! if (state.open++ === 0) for (const deadline of state.deadlines) deadline.pause();   // :54
//! if (--state.open === 0) for (const deadline of state.deadlines) deadline.resume();  // :58
//! ```
//!
//! and every `callTool` additionally asks for progress, each notification restarting the timeout
//! (`:72-83`, `:111-116`). Upstream's wrapper hands the SDK `timeout: MAX_TIMER_MS` and drives a
//! `setTimeout` of its own, because "the SDK's own timer cannot be paused".
//!
//! `rmcp` is in the same position with one wrinkle. `PeerRequestOptions` **does** carry
//! `reset_timeout_on_progress`, but it is honoured only by `RequestHandle::await_response`, which
//! **consumes** the handle — and [`crate::live`] keeps the handle precisely so it can
//! `RequestHandle::cancel` when the owned signal fires (`live.rs`'s `request_on_peer`). Setting the
//! flag without surrendering the handle is worse than not setting it: rmcp registers an mpsc
//! watcher in `Peer::progress_timeout_watchers` keyed by the progress token and removes it only in
//! `await_response` or `cancel`, so every call that returned normally would leak one.
//!
//! So the deadline is driven here, cyrup-side, exactly as upstream drives it host-side: this module
//! is upstream's `{ pause, resume }` pair plus upstream's progress restart, and rmcp's flag stays
//! at its default. The error the expired path produces is unchanged — `request_on_peer`'s existing
//! `Settled::TimedOut` arm, which cancels the request on the wire with
//! `RequestHandle::REQUEST_TIMEOUT_REASON` and reports `MCP request timed out after {n} ms`.
//!
//! **cyrup needs no `onprogress` shim.** `Peer::send_request_with_option_and_subscription` sets a
//! progress token on the `_meta` of **every** request unconditionally, where the TypeScript SDK
//! only sends one when `onprogress` is set — which is the whole reason upstream's wrapper installs
//! a no-op handler (`const onprogress = options?.onprogress ?? (() => {})`, `:81`). The token is
//! already on the wire here, so a server that reports progress already reports it; what was missing
//! is anything that *listened*. [`ClientHandler::on_progress`] is that listener.
//!
//! [`ClientHandler::on_progress`]: rmcp::handler::client::ClientHandler::on_progress

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use rmcp::model::ProgressToken;
use tokio::sync::watch;
// `tokio::time::Instant`, NOT `std::time::Instant`: the elapsed figure this deadline subtracts has
// to come from the same clock [`tokio::time::sleep`] reads, or a paused-clock test measures a
// pause against real wall time and the two disagree by exactly the amount under test. Outside a
// runtime it is `std::time::Instant` (`tokio::time::clock`), so production behaviour is unchanged.
use tokio::time::Instant;

/// One in-flight call's pausable, restartable deadline — upstream's
/// `{ pause(), resume() }` object (`elicitation-handler.ts:86-103`) with its `remaining` /
/// `startedAt` pair.
///
/// A deadline is created **paused** (`timer === undefined`), as upstream's is: the wrapper calls
/// `deadline.resume()` immediately only `if (state.open === 0)` (`:105`), so a call that starts
/// while a prompt is already open does not begin spending its budget.
#[derive(Debug)]
pub struct CallDeadline {
    /// `const timeout = Math.min(options?.timeout ?? DEFAULT_REQUEST_TIMEOUT_MSEC, MAX_TIMER_MS)` —
    /// the full budget a restart returns to.
    budget: Duration,
    state: Mutex<DeadlineState>,
    /// Bumped on every `pause` / `resume` / `restart`. A [`watch`] channel rather than a
    /// [`tokio::sync::Notify`] because the waiter must not miss a change that lands between its
    /// snapshot and its next registration — `Notify` wakes only already-registered waiters, and a
    /// missed `pause` would let the call expire with the prompt still open, which is the bug.
    generation: watch::Sender<u64>,
}

#[derive(Debug)]
struct DeadlineState {
    /// Budget left as of the last pause. While running, the live figure is this minus
    /// `running_since.elapsed()`.
    remaining: Duration,
    /// `startedAt`, or `None` for upstream's `timer === undefined`.
    running_since: Option<Instant>,
}

impl CallDeadline {
    /// A paused deadline with `budget` left.
    #[must_use]
    pub fn new(budget: Duration) -> Arc<Self> {
        Arc::new(Self {
            budget,
            state: Mutex::new(DeadlineState {
                remaining: budget,
                running_since: None,
            }),
            generation: watch::channel(0).0,
        })
    }

    /// The budget this deadline was built with — what the timed-out error reports, so the message
    /// names the configured `requestTimeoutMs` and not the fragment that happened to be left.
    #[must_use]
    pub fn budget(&self) -> Duration {
        self.budget
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut DeadlineState) -> R) -> R {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }

    /// ```text
    /// pause() { if (timer === undefined) return; clearTimeout(timer); timer = undefined;
    ///           remaining -= Date.now() - startedAt; }
    /// ```
    pub fn pause(&self) {
        let changed = self.with_state(|state| {
            let Some(since) = state.running_since.take() else {
                return false;
            };
            state.remaining = state.remaining.saturating_sub(since.elapsed());
            true
        });
        if changed {
            self.bump();
        }
    }

    /// ```text
    /// resume() { startedAt = Date.now(); timer = setTimeout(abort, Math.max(remaining, 0)); }
    /// ```
    ///
    /// `Math.max(remaining, 0)` is [`Duration`]'s floor: a deadline whose budget ran out while a
    /// prompt was open fires as soon as it resumes, which is upstream's documented one exception —
    /// "a deadline that has already elapsed is pushed out of the way and the deadline aborts the
    /// call".
    pub fn resume(&self) {
        let changed = self.with_state(|state| {
            if state.running_since.is_some() {
                return false;
            }
            state.running_since = Some(Instant::now());
            true
        });
        if changed {
            self.bump();
        }
    }

    /// The progress handler's `pause(); remaining = timeout; if (running) resume();`
    /// (`elicitation-handler.ts:111-116`).
    ///
    /// The `if (running)` is why this is one method and not `pause` + a setter + `resume`: a
    /// progress notification that arrives while a prompt is open must refill the budget **without**
    /// starting the clock, or the prompt's pause is silently undone.
    pub fn restart(&self) {
        self.with_state(|state| {
            let was_running = state.running_since.is_some();
            state.remaining = self.budget;
            state.running_since = was_running.then(Instant::now);
        });
        self.bump();
    }

    fn bump(&self) {
        self.generation.send_modify(|generation| {
            *generation = generation.wrapping_add(1);
        });
    }

    /// A receiver for [`Self::expired`]. Taken once, before the first snapshot, so no change can
    /// land in the gap.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }

    /// Time left right now, or `None` while paused.
    fn snapshot(&self) -> Option<Duration> {
        self.with_state(|state| {
            let since = state.running_since?;
            Some(state.remaining.saturating_sub(since.elapsed()))
        })
    }

    /// Resolves when the deadline has actually elapsed — never while it is paused.
    ///
    /// Written as a loop over the generation channel rather than a single `sleep`, because every
    /// `pause` / `resume` / `restart` changes how long the remaining wait is; the `select!` that
    /// drives this races it against the response and the owned signal, so abandoning the future
    /// mid-sleep is the normal exit.
    pub async fn expired(&self, generation: &mut watch::Receiver<u64>) {
        loop {
            match self.snapshot() {
                Some(left) => {
                    tokio::select! {
                        () = tokio::time::sleep(left) => return,
                        changed = generation.changed() => {
                            if changed.is_err() {
                                // The sender lives on `self`, so this is unreachable while this
                                // borrow exists; waiting forever is still the safe answer, because
                                // returning would report a timeout that did not happen.
                                std::future::pending::<()>().await;
                            }
                        }
                    }
                }
                None => {
                    if generation.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            }
        }
    }
}

/// `const promptPauses = new WeakMap<Client, PromptPause>()` (`elicitation-handler.ts:44`) plus the
/// progress-token index cyrup needs because its progress notifications arrive on the
/// [`rmcp::handler::client::ClientHandler`] rather than on a per-request `onprogress` callback.
///
/// Keyed by **server name** where upstream keys by `Client`: the manager holds at most one
/// connection per `mcpServers` key, so the two are the same granularity, and a name survives the
/// reconnect that replaces the client — which is what a call straddling a reconnect needs.
#[derive(Debug, Default)]
pub struct CallDeadlines {
    inner: Mutex<Registry>,
}

#[derive(Debug, Default)]
struct Registry {
    servers: HashMap<String, ServerPauses>,
    /// `progressToken → deadline`, for [`CallDeadlines::progress`].
    by_token: HashMap<ProgressToken, Weak<CallDeadline>>,
}

#[derive(Debug, Default)]
struct ServerPauses {
    /// `state.open` — how many of this server's elicitation prompts are on screen.
    open: usize,
    /// `state.deadlines`. `Weak`, so a leaked registration cannot keep a finished call's deadline
    /// alive; [`CallDeadlines::release`] is the `finally` that normally removes it.
    deadlines: Vec<Weak<CallDeadline>>,
}

/// What [`CallDeadlines::register`] hands back: the deadline, and whether the caller should start
/// it (`if (state.open === 0) deadline.resume()`).
#[derive(Debug)]
pub struct Registration {
    pub deadline: Arc<CallDeadline>,
    /// `false` when a prompt for this server is already open, so the new call starts paused.
    pub start_running: bool,
}

impl CallDeadlines {
    fn with<R>(&self, f: impl FnOnce(&mut Registry) -> R) -> R {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }

    /// `state.deadlines.add(deadline); if (state.open === 0) deadline.resume();`
    /// (`elicitation-handler.ts:104-105`), with the resume left to the caller so the returned
    /// deadline is never running before the caller holds it.
    pub fn register(&self, server: &str, budget: Duration) -> Registration {
        let deadline = CallDeadline::new(budget);
        let start_running = self.with(|registry| {
            let entry = registry.servers.entry(server.to_string()).or_default();
            entry.deadlines.retain(|weak| weak.strong_count() > 0);
            entry.deadlines.push(Arc::downgrade(&deadline));
            entry.open == 0
        });
        if start_running {
            deadline.resume();
        }
        Registration {
            deadline,
            start_running,
        }
    }

    /// Index a registered deadline by the progress token rmcp assigned its request, so
    /// [`Self::progress`] can find it. Called after the send, which is when the token exists.
    pub fn index_token(&self, token: ProgressToken, deadline: &Arc<CallDeadline>) {
        self.with(|registry| {
            registry.by_token.insert(token, Arc::downgrade(deadline));
        });
    }

    /// `finally { state.deadlines.delete(deadline); deadline.pause(); }`
    /// (`elicitation-handler.ts:119-121`).
    pub fn release(
        &self,
        server: &str,
        token: Option<&ProgressToken>,
        deadline: &Arc<CallDeadline>,
    ) {
        self.with(|registry| {
            if let Some(entry) = registry.servers.get_mut(server) {
                entry
                    .deadlines
                    .retain(|weak| !weak.ptr_eq(&Arc::downgrade(deadline)));
                if entry.open == 0 && entry.deadlines.is_empty() {
                    registry.servers.remove(server);
                }
            }
            if let Some(token) = token {
                registry.by_token.remove(token);
            }
        });
        deadline.pause();
    }

    /// `if (state.open++ === 0) for (const deadline of state.deadlines) deadline.pause();`
    /// (`:54`). Returns nothing: an unknown server is a server with no calls in flight, and the
    /// count still has to be raised so the matching [`Self::prompt_closed`] balances.
    pub fn prompt_opened(&self, server: &str) {
        let paused = self.with(|registry| {
            let entry = registry.servers.entry(server.to_string()).or_default();
            entry.open += 1;
            if entry.open != 1 {
                return Vec::new();
            }
            entry
                .deadlines
                .iter()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        });
        for deadline in paused {
            deadline.pause();
        }
    }

    /// `if (--state.open === 0) for (const deadline of state.deadlines) deadline.resume();` (`:58`).
    pub fn prompt_closed(&self, server: &str) {
        let resumed = self.with(|registry| {
            let Some(entry) = registry.servers.get_mut(server) else {
                return Vec::new();
            };
            entry.open = entry.open.saturating_sub(1);
            if entry.open != 0 {
                return Vec::new();
            }
            let live: Vec<_> = entry.deadlines.iter().filter_map(Weak::upgrade).collect();
            if live.is_empty() {
                registry.servers.remove(server);
            }
            live
        });
        for deadline in resumed {
            deadline.resume();
        }
    }

    /// The progress handler's half of `b61c6ef` (MCP-607): one notification refills the budget of
    /// the call it names.
    ///
    /// Keyed by token, not by server: upstream's `onprogress` is per request, and restarting every
    /// deadline on the connection would let one chatty call hold another's open indefinitely.
    pub fn progress(&self, token: &ProgressToken) {
        let deadline = self.with(|registry| registry.by_token.get(token).and_then(Weak::upgrade));
        if let Some(deadline) = deadline {
            deadline.restart();
        }
    }

    /// How many prompts this server has open — for the tests, and for nothing else.
    #[must_use]
    pub fn open_prompts(&self, server: &str) -> usize {
        self.with(|registry| registry.servers.get(server).map_or(0, |entry| entry.open))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use rmcp::model::NumberOrString;

    fn token(id: i64) -> ProgressToken {
        ProgressToken(NumberOrString::Number(id))
    }

    /// A deadline that has nothing to race is the baseline: it expires after its budget.
    #[tokio::test(start_paused = true)]
    async fn a_running_deadline_expires_after_its_budget() {
        let deadlines = CallDeadlines::default();
        let registered = deadlines.register("srv", Duration::from_millis(100));
        assert!(
            registered.start_running,
            "no prompt is open, so the call starts spending"
        );
        let deadline = registered.deadline;
        let mut generation = deadline.subscribe();
        tokio::time::timeout(
            Duration::from_millis(500),
            deadline.expired(&mut generation),
        )
        .await
        .expect("the deadline must fire");
    }

    /// MCP-606, `53cdc0b` (#729) — "an open prompt pauses the deadline of every tool call in flight
    /// on that client". The prompt is held open past the whole budget and the call still has time
    /// left when it closes.
    #[tokio::test(start_paused = true)]
    async fn an_open_prompt_pauses_the_deadline_and_closing_it_resumes() {
        let deadlines = CallDeadlines::default();
        let deadline = deadlines
            .register("srv", Duration::from_millis(100))
            .deadline;
        let mut generation = deadline.subscribe();

        deadlines.prompt_opened("srv");
        assert_eq!(deadlines.open_prompts("srv"), 1);
        // Five budgets' worth of the user reading the form.
        assert!(
            tokio::time::timeout(
                Duration::from_millis(500),
                deadline.expired(&mut generation)
            )
            .await
            .is_err(),
            "the deadline must not fire while the prompt is open"
        );

        deadlines.prompt_closed("srv");
        assert_eq!(deadlines.open_prompts("srv"), 0);
        // …and it now fires on its own schedule, from where it was paused.
        tokio::time::timeout(
            Duration::from_millis(500),
            deadline.expired(&mut generation),
        )
        .await
        .expect("the deadline resumes after the prompt closes");
    }

    /// `state.open` is a counter, not a flag (`elicitation-handler.ts:54`, `:58`): two prompts on
    /// one server resume only on the second close.
    #[tokio::test(start_paused = true)]
    async fn the_pause_is_counted_so_the_last_prompt_resumes() {
        let deadlines = CallDeadlines::default();
        let deadline = deadlines
            .register("srv", Duration::from_millis(100))
            .deadline;
        let mut generation = deadline.subscribe();

        deadlines.prompt_opened("srv");
        deadlines.prompt_opened("srv");
        deadlines.prompt_closed("srv");
        assert_eq!(deadlines.open_prompts("srv"), 1);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(500),
                deadline.expired(&mut generation)
            )
            .await
            .is_err(),
            "one prompt is still open"
        );
        deadlines.prompt_closed("srv");
        tokio::time::timeout(
            Duration::from_millis(500),
            deadline.expired(&mut generation),
        )
        .await
        .expect("the last close resumes it");
    }

    /// The registry is per server: a prompt on one server must not pause another's calls, which is
    /// what keying by `Client` buys upstream.
    #[tokio::test(start_paused = true)]
    async fn a_prompt_on_one_server_does_not_pause_another() {
        let deadlines = CallDeadlines::default();
        let other = deadlines
            .register("other", Duration::from_millis(100))
            .deadline;
        let mut generation = other.subscribe();
        deadlines.prompt_opened("srv");
        tokio::time::timeout(Duration::from_millis(500), other.expired(&mut generation))
            .await
            .expect("the other server's call keeps its own clock");
    }

    /// MCP-607, `b61c6ef` (#763) — "each progress notification restarts the timeout". Progress
    /// arrives every 60 ms against a 100 ms budget, five times over, and the call survives.
    #[tokio::test(start_paused = true)]
    async fn progress_restarts_the_deadline_and_is_keyed_by_token() {
        let deadlines = Arc::new(CallDeadlines::default());
        let deadline = deadlines
            .register("srv", Duration::from_millis(100))
            .deadline;
        deadlines.index_token(token(1), &deadline);
        let quiet = deadlines
            .register("srv", Duration::from_millis(100))
            .deadline;
        deadlines.index_token(token(2), &quiet);

        let ticker = {
            let deadlines = Arc::clone(&deadlines);
            tokio::spawn(async move {
                for _ in 0..5 {
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    deadlines.progress(&token(1));
                }
            })
        };

        let mut generation = deadline.subscribe();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(350),
                deadline.expired(&mut generation)
            )
            .await
            .is_err(),
            "a call reporting progress every 60ms must outlive a 100ms budget"
        );
        ticker.await.expect("the ticker finishes");

        // …and the sibling, which reported nothing, died on schedule: the restart is per token.
        // Well inside its own 100 ms budget, which 350 ms of wall clock has long since spent: the
        // bound is "promptly", not "in the same instant" — tokio's timer rounds up to the next
        // millisecond tick, so a zero-length sleep is not a zero-length wait.
        let mut quiet_generation = quiet.subscribe();
        tokio::time::timeout(
            Duration::from_millis(50),
            quiet.expired(&mut quiet_generation),
        )
        .await
        .expect("the quiet call expired while the chatty one was being kept alive");
    }

    /// Upstream's documented exception (`elicitation-handler.ts:68-70`): a deadline that already
    /// elapsed "is pushed out of the way and the deadline aborts the call". Resuming such a
    /// deadline fires it at once rather than granting it a fresh budget.
    #[tokio::test(start_paused = true)]
    async fn a_deadline_that_elapsed_while_paused_fires_the_moment_it_resumes() {
        let deadlines = CallDeadlines::default();
        let deadline = deadlines
            .register("srv", Duration::from_millis(100))
            .deadline;
        let mut generation = deadline.subscribe();
        // Spend the whole budget, then pause with nothing left.
        tokio::time::sleep(Duration::from_millis(100)).await;
        deadlines.prompt_opened("srv");
        assert!(
            tokio::time::timeout(
                Duration::from_millis(500),
                deadline.expired(&mut generation)
            )
            .await
            .is_err(),
            "even an exhausted deadline waits while the prompt is open"
        );
        deadlines.prompt_closed("srv");
        // Half the budget: enough to clear tokio's millisecond timer granularity, far too little
        // for the resume to have granted a fresh 100 ms.
        tokio::time::timeout(Duration::from_millis(50), deadline.expired(&mut generation))
            .await
            .expect("an exhausted deadline fires immediately on resume");
    }

    /// `if (state.open === 0) deadline.resume()` (`:105`) — a call that starts while a prompt is
    /// already open does not begin spending, so the user answering one server's form does not
    /// quietly consume the budget of the call they are about to trigger.
    #[tokio::test(start_paused = true)]
    async fn a_call_registered_while_a_prompt_is_open_starts_paused() {
        let deadlines = CallDeadlines::default();
        deadlines.prompt_opened("srv");
        let registered = deadlines.register("srv", Duration::from_millis(100));
        assert!(!registered.start_running);
        let deadline = registered.deadline;
        let mut generation = deadline.subscribe();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(500),
                deadline.expired(&mut generation)
            )
            .await
            .is_err()
        );
        deadlines.prompt_closed("srv");
        tokio::time::timeout(
            Duration::from_millis(500),
            deadline.expired(&mut generation),
        )
        .await
        .expect("it gets its full budget from the close");
    }

    /// `finally { state.deadlines.delete(deadline); deadline.pause(); }` — a released deadline is
    /// gone from both indices, so a late progress notification or a prompt opening afterwards
    /// touches nothing.
    #[tokio::test]
    async fn release_removes_the_deadline_from_both_indices() {
        let deadlines = CallDeadlines::default();
        let deadline = deadlines
            .register("srv", Duration::from_millis(100))
            .deadline;
        deadlines.index_token(token(7), &deadline);
        deadlines.release("srv", Some(&token(7)), &deadline);
        assert_eq!(deadlines.open_prompts("srv"), 0);
        // No panic, no effect: the token is unknown and the server has no deadlines left.
        deadlines.progress(&token(7));
        deadlines.prompt_opened("srv");
        deadlines.prompt_closed("srv");
        assert!(deadline.snapshot().is_none(), "release pauses it");
    }
}
