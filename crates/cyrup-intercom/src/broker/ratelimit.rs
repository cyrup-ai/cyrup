//! Per-connection token bucket (`broker.ts:28-29,211-215,270-284`): capacity 240, refill 120/s.
//! An empty bucket makes the broker reply with an `error` frame and destroy the connection
//! (`broker.ts:218-222`).

/// `RATE_LIMIT_CAPACITY = 240` (`broker.ts:28`).
pub const RATE_LIMIT_CAPACITY: f64 = 240.0;
/// `RATE_LIMIT_REFILL_PER_SECOND = 120` (`broker.ts:29`).
pub const RATE_LIMIT_REFILL_PER_SECOND: f64 = 120.0;

/// A per-connection token bucket. `consume` refills lazily based on elapsed wall-clock ms since the
/// last refill, caps at [`RATE_LIMIT_CAPACITY`], then spends one token (`consumeToken`,
/// `broker.ts:270-284`).
#[derive(Debug, Clone)]
pub struct TokenBucket {
    tokens: f64,
    last_refill_at_ms: u64,
}

impl TokenBucket {
    /// A full bucket as of `now_ms` (a fresh connection starts at capacity, `broker.ts:212-214`).
    #[must_use]
    pub fn new(now_ms: u64) -> Self {
        Self { tokens: RATE_LIMIT_CAPACITY, last_refill_at_ms: now_ms }
    }

    /// Refill by elapsed time, then attempt to spend one token. Returns `true` if a token was
    /// available (proceed), `false` if the bucket is empty (destroy the connection).
    pub fn consume(&mut self, now_ms: u64) -> bool {
        let elapsed_ms = now_ms.saturating_sub(self.last_refill_at_ms);
        if elapsed_ms > 0 {
            let refill = (elapsed_ms as f64) * RATE_LIMIT_REFILL_PER_SECOND / 1000.0;
            self.tokens = (self.tokens + refill).min(RATE_LIMIT_CAPACITY);
            self.last_refill_at_ms = now_ms;
        }
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn spends_up_to_capacity_then_denies() {
        let mut b = TokenBucket::new(0);
        for _ in 0..240 {
            assert!(b.consume(0), "each of the first 240 tokens is available at t=0");
        }
        assert!(!b.consume(0), "the 241st consume at the same instant is denied");
    }

    #[test]
    fn refills_over_time() {
        let mut b = TokenBucket::new(0);
        for _ in 0..240 {
            assert!(b.consume(0));
        }
        assert!(!b.consume(0));
        // 120 tokens/sec → 1000ms yields the full 240-cap back (capped).
        assert!(b.consume(1000), "after 1s the bucket has refilled and a token is available");
    }

    #[test]
    fn refill_is_capped_at_capacity() {
        let mut b = TokenBucket::new(0);
        // Idle a long time: refill must not exceed the cap, so only 240 are ever spendable at once.
        let mut spent = 0;
        while b.consume(10_000) {
            spent += 1;
            if spent > 1000 {
                break;
            }
        }
        assert_eq!(spent, 240, "an idle bucket refills to at most the 240 capacity, not unbounded");
    }
}
