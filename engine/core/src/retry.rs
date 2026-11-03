//! Retry backoff policy.

use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base: Duration::from_secs(5),
            max_delay: Duration::from_secs(300),
        }
    }
}

impl RetryPolicy {
    /// Delay before `attempt` (1-based), doubling each time and capped at
    /// `max_delay`. Returns `None` once the attempt budget is exhausted.
    pub fn delay_for(&self, attempt: u32) -> Option<Duration> {
        if attempt == 0 || attempt > self.max_attempts {
            return None;
        }
        // Saturate rather than overflow on large attempt counts.
        let factor = 2u32.saturating_pow(attempt - 1);
        let delay = self.base.saturating_mul(factor);
        Some(delay.min(self.max_delay))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_doubles_per_attempt() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_for(1), Some(Duration::from_secs(5)));
        assert_eq!(p.delay_for(2), Some(Duration::from_secs(10)));
        assert_eq!(p.delay_for(3), Some(Duration::from_secs(20)));
    }

    #[test]
    fn delay_is_capped() {
        let p = RetryPolicy {
            max_attempts: 20,
            ..Default::default()
        };
        assert_eq!(p.delay_for(20), Some(Duration::from_secs(300)));
    }

    #[test]
    fn budget_is_finite() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_for(0), None);
        assert_eq!(p.delay_for(6), None);
    }
}
