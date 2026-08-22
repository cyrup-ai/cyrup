//! The session-level retry-after-agent-end policy.
//!
//! Pi `agent-session.ts:778,561,2484-2572`. The agent layer drives provider-level retry
//! (`max_retries`/`max_retry_delay_ms`); this decides whether a final assistant turn carrying a
//! transient error is worth an exponential backoff and an `agent.continue()`.

use cyrup_agent::AgentMessage;
use cyrup_core::AssistantMessage;
use cyrup_provider::{RetryPolicy, is_context_overflow, is_retryable_assistant_error};

use crate::event::AgentSessionEvent;

use super::AgentSession;

impl AgentSession {
    /// Current retry attempt (0 when not retrying; Pi `retryAttempt` getter, agent-session.ts:778).
    pub fn retry_attempt(&self) -> u32 {
        *Self::lock(&self.retry_attempt)
    }

    /// Whether a retry backoff is in flight (Pi `isRetrying` getter, agent-session.ts:2553).
    pub fn is_retrying(&self) -> bool {
        Self::lock(&self.retry_cancel).is_some()
    }

    /// Whether auto-retry is enabled (runtime override, else the settings default; Pi
    /// `autoRetryEnabled`, agent-session.ts:2558).
    pub fn auto_retry_enabled(&self) -> bool {
        Self::lock(&self.auto_retry_override).unwrap_or(self.retry_enabled_default)
    }

    /// The retry policy handed to every summarization call (compaction, turn-prefix, branch).
    ///
    /// Pi passes `this.settingsManager.getRetrySettings()` — the RESOLVED SETTINGS, not the
    /// interactive auto-retry toggle (`agent-session.ts:1858,2132,2997`), so this deliberately
    /// reads the settings defaults rather than [`Self::auto_retry_enabled`]: pausing the visible
    /// turn-level auto-retry must not silently make a transient socket close abort a whole
    /// compaction.
    pub fn summarization_retry(&self) -> RetryPolicy {
        RetryPolicy::new(
            self.retry_enabled_default,
            self.retry_max_retries,
            self.retry_base_delay_ms,
        )
    }

    /// Toggle auto-retry (Pi `setAutoRetryEnabled`, agent-session.ts:2565). Facade-side override of
    /// the settings `retry.enabled` value (settings persistence lives one layer down).
    pub fn set_auto_retry_enabled(&self, enabled: bool) {
        *Self::lock(&self.auto_retry_override) = Some(enabled);
    }

    /// Cancel an in-flight retry backoff (Pi `abortRetry`, agent-session.ts:2548).
    pub fn abort_retry(&self) {
        if let Some(c) = Self::lock(&self.retry_cancel).as_ref() {
            c.cancel();
        }
    }

    /// Whether an assistant error is retryable (Pi `_isRetryableError`, agent-session.ts:2484).
    /// Context-overflow is handled by compaction, never retry.
    pub fn is_retryable_error(&self, message: &AssistantMessage) -> bool {
        // Pi `if (isContextOverflow(message, this.model?.contextWindow ?? 0)) return false;`
        // (agent-session.ts:2637).
        let window = { Some(Self::lock(&self.compaction_model).as_ref().map_or(0, |m| m.context_window)) };
        if is_context_overflow(message, window) {
            return false;
        }
        is_retryable_assistant_error(message)
    }

    /// Whether the run that just ended will retry (Pi `_willRetryAfterAgentEnd`, agent-session.ts:561).
    /// True iff auto-retry is enabled, the budget is not exhausted, and the last assistant message is
    /// a retryable error.
    pub fn will_retry_after_agent_end(&self, messages: &[AgentMessage]) -> bool {
        if !self.auto_retry_enabled() || self.retry_attempt() >= self.retry_max_retries {
            return false;
        }
        messages
            .iter()
            .rev()
            .find_map(|m| match m {
                AgentMessage::Assistant(a) => Some(self.is_retryable_error(a)),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// Prepare a retryable error for continuation with exponential backoff (Pi `_prepareRetry`,
    /// agent-session.ts:2495-2543). Returns `true` when the caller should continue the agent after
    /// the (abortable) backoff, `false` when retry is disabled, the budget is exhausted, or the wait
    /// was cancelled. Drops the trailing error message from the agent transcript before continuing.
    pub async fn prepare_retry(&self, message: &AssistantMessage) -> bool {
        if !self.auto_retry_enabled() {
            return false;
        }
        {
            let mut attempt = Self::lock(&self.retry_attempt);
            *attempt += 1;
            if *attempt > self.retry_max_retries {
                *attempt -= 1;
                return false;
            }
        }
        let attempt = self.retry_attempt();
        let delay_ms = self
            .retry_base_delay_ms
            .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)));
        self.fanout_emit(AgentSessionEvent::AutoRetryStart {
            attempt,
            max_attempts: self.retry_max_retries,
            delay_ms,
            error_message: message.error_message.clone().unwrap_or_else(|| "Unknown error".into()),
        })
        .await;
        // Drop the trailing error message from the agent transcript (kept in session for history).
        self.drop_trailing_assistant().await;
        // Abortable exponential backoff.
        let cancel = self.session_cancel.child_token();
        *Self::lock(&self.retry_cancel) = Some(cancel.clone());
        let slept = cancel
            .run_until_cancelled(tokio::time::sleep(std::time::Duration::from_millis(delay_ms)))
            .await
            .is_some();
        *Self::lock(&self.retry_cancel) = None;
        if !slept {
            let attempt = std::mem::replace(&mut *Self::lock(&self.retry_attempt), 0);
            self.fanout_emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt,
                final_error: Some("Retry cancelled".into()),
            })
            .await;
            return false;
        }
        true
    }

    /// Drop the trailing assistant message from the agent transcript (used by retry/overflow paths).
    pub(super) async fn drop_trailing_assistant(&self) {
        let mut msgs = self.agent.snapshot().await.messages;
        if matches!(msgs.last(), Some(AgentMessage::Assistant(_))) {
            msgs.pop();
            self.agent.set_messages(msgs).await;
        }
    }
}
