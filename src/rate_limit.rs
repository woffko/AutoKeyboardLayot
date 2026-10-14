//! A sliding-window allowance: at most `max` grants in any period of `per`.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub struct RateLimit {
    max: usize,
    per: Duration,
    granted: VecDeque<Instant>,
}

impl RateLimit {
    pub fn new(max: usize, per: Duration) -> Self {
        Self {
            max,
            per,
            granted: VecDeque::new(),
        }
    }

    /// Grants one use at `now` when fewer than `max` were granted within the last `per`. A
    /// refused attempt is not recorded, so it does not extend the wait.
    pub fn try_take(&mut self, now: Instant) -> bool {
        while self
            .granted
            .front()
            .is_some_and(|&earlier| now.saturating_duration_since(earlier) >= self.per)
        {
            self.granted.pop_front();
        }
        if self.granted.len() < self.max {
            self.granted.push_back(now);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fourth_use_within_the_period_is_refused_until_the_first_one_ages_out() {
        let start = Instant::now();
        let minute = |n: u64| start + Duration::from_secs(60 * n);
        let mut limit = RateLimit::new(3, Duration::from_secs(3600));
        assert!(limit.try_take(minute(0)));
        assert!(limit.try_take(minute(1)));
        assert!(limit.try_take(minute(2)));
        assert!(!limit.try_take(minute(3)));
        // Refused attempts are not recorded: the slot frees exactly when the first grant expires.
        assert!(!limit.try_take(minute(59)));
        assert!(limit.try_take(minute(60)));
        assert!(!limit.try_take(minute(60)));
        assert!(limit.try_take(minute(61)));
    }

    #[test]
    fn a_limit_of_zero_never_grants() {
        let mut limit = RateLimit::new(0, Duration::from_secs(1));
        assert!(!limit.try_take(Instant::now()));
    }
}
