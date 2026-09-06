//! Exponential backoff with full jitter, for reconnect loops.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    current: Duration,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Self {
        Self {
            base,
            max,
            current: base,
        }
    }

    /// Reset after a successful connection.
    pub fn reset(&mut self) {
        self.current = self.base;
    }

    /// Next delay to wait, then advance. "Full jitter": a uniform random
    /// value in `[0, current]`, after which `current` doubles up to `max`.
    pub fn next_delay(&mut self) -> Duration {
        let ceiling = self.current;
        let millis = ceiling.as_millis().max(1) as u64;
        let jittered = Duration::from_millis(fastrand::u64(0..=millis));
        self.current = (self.current * 2).min(self.max);
        jittered
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Duration::from_secs(1), Duration::from_secs(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_then_caps() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(8));
        for _ in 0..10 {
            let d = b.next_delay();
            assert!(d <= Duration::from_secs(8));
        }
        // ceiling has saturated at max
        assert_eq!(b.current, Duration::from_secs(8));
    }

    #[test]
    fn reset_returns_to_base() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));
        for _ in 0..5 {
            b.next_delay();
        }
        b.reset();
        assert_eq!(b.current, Duration::from_secs(1));
    }
}
