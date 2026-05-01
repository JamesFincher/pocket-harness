use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub circuit_open_after: u32,
}

impl BackoffPolicy {
    pub fn new(initial_delay: Duration, max_delay: Duration, circuit_open_after: u32) -> Self {
        Self {
            initial_delay,
            max_delay,
            circuit_open_after: circuit_open_after.max(1),
        }
    }
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self::new(Duration::from_secs(1), Duration::from_secs(60), 6)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureDecision {
    pub consecutive_failures: u32,
    pub delay: Duration,
    pub circuit_open: bool,
}

#[derive(Debug, Clone)]
pub struct BackoffSupervisor {
    policy: BackoffPolicy,
    consecutive_failures: u32,
}

impl BackoffSupervisor {
    pub fn new(policy: BackoffPolicy) -> Self {
        Self {
            policy,
            consecutive_failures: 0,
        }
    }

    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }

    pub fn record_failure(&mut self) -> FailureDecision {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let multiplier = 1u32.checked_shl(self.consecutive_failures.saturating_sub(1).min(16));
        let delay = multiplier
            .and_then(|multiplier| self.policy.initial_delay.checked_mul(multiplier))
            .unwrap_or(self.policy.max_delay)
            .min(self.policy.max_delay);

        FailureDecision {
            consecutive_failures: self.consecutive_failures,
            delay,
            circuit_open: self.consecutive_failures >= self.policy.circuit_open_after,
        }
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }
}

impl Default for BackoffSupervisor {
    fn default() -> Self {
        Self::new(BackoffPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{BackoffPolicy, BackoffSupervisor};

    #[test]
    fn backoff_exponentially_caps_and_opens_circuit() {
        let policy = BackoffPolicy::new(Duration::from_secs(2), Duration::from_secs(10), 3);
        let mut supervisor = BackoffSupervisor::new(policy);

        let first = supervisor.record_failure();
        assert_eq!(first.delay, Duration::from_secs(2));
        assert!(!first.circuit_open);

        let second = supervisor.record_failure();
        assert_eq!(second.delay, Duration::from_secs(4));
        assert!(!second.circuit_open);

        let third = supervisor.record_failure();
        assert_eq!(third.delay, Duration::from_secs(8));
        assert!(third.circuit_open);

        let capped = supervisor.record_failure();
        assert_eq!(capped.delay, Duration::from_secs(10));
        assert!(capped.circuit_open);

        supervisor.record_success();
        assert_eq!(supervisor.consecutive_failures(), 0);
        assert_eq!(supervisor.record_failure().delay, Duration::from_secs(2));
    }
}
