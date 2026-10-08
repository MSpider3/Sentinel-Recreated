use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Failed face attempts allowed per user within `ATTEMPT_WINDOW`.
pub const MAX_FAILED_ATTEMPTS: usize = 5;
pub const ATTEMPT_WINDOW: Duration = Duration::from_secs(60);

/// Pauses face authentication for a user after repeated failures, so nobody
/// can keep presenting photos until one gets through. It only ever makes the
/// daemon say "use the password" — it cannot lock anyone out.
#[derive(Default)]
pub struct AttemptLimiter {
    failures: HashMap<String, Vec<Instant>>,
}

impl AttemptLimiter {
    fn recent(&mut self, user: &str, now: Instant) -> usize {
        match self.failures.get_mut(user) {
            Some(times) => {
                times.retain(|t| now.saturating_duration_since(*t) < ATTEMPT_WINDOW);
                times.len()
            }
            None => 0,
        }
    }

    pub fn is_blocked(&mut self, user: &str, now: Instant) -> bool {
        self.recent(user, now) >= MAX_FAILED_ATTEMPTS
    }

    pub fn record_failure(&mut self, user: &str, now: Instant) {
        self.failures.entry(user.to_string()).or_default().push(now);
    }

    pub fn clear(&mut self, user: &str) {
        self.failures.remove(user);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocks_after_max_failures_then_recovers() {
        let mut limiter = AttemptLimiter::default();
        let t0 = Instant::now();

        for i in 0..MAX_FAILED_ATTEMPTS {
            assert!(!limiter.is_blocked("alice", t0));
            limiter.record_failure("alice", t0 + Duration::from_secs(i as u64));
        }
        assert!(limiter.is_blocked("alice", t0 + Duration::from_secs(10)));
        // Other users are unaffected.
        assert!(!limiter.is_blocked("bob", t0 + Duration::from_secs(10)));
        // The block lifts once the oldest failure leaves the window.
        assert!(!limiter.is_blocked("alice", t0 + ATTEMPT_WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn test_success_clears_failures() {
        let mut limiter = AttemptLimiter::default();
        let t0 = Instant::now();
        for _ in 0..MAX_FAILED_ATTEMPTS {
            limiter.record_failure("alice", t0);
        }
        assert!(limiter.is_blocked("alice", t0));
        limiter.clear("alice");
        assert!(!limiter.is_blocked("alice", t0));
    }
}
