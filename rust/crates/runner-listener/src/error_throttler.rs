// ErrorThrottler mapping the C# error throttling in Runner.cs.
// Provides exponential backoff (1s to 60s) for retryable errors in the message loop.

use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Minimum backoff delay.
const MIN_BACKOFF: Duration = Duration::from_secs(1);

/// Maximum backoff delay.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Multiplier for exponential growth.
const BACKOFF_MULTIPLIER: f64 = 2.0;

/// Exponential backoff error throttler.
///
/// Each call to `increment_and_wait` doubles the delay (capped at 60s).
/// Calling `reset` returns the delay to 1s.
pub struct ErrorThrottler {
    current_delay: Duration,
}

impl ErrorThrottler {
    /// Create a new `ErrorThrottler` starting at the minimum backoff.
    pub fn new() -> Self {
        Self {
            current_delay: MIN_BACKOFF,
        }
    }

    /// Reset the backoff delay to the minimum.
    pub fn reset(&mut self) {
        self.current_delay = MIN_BACKOFF;
    }

    /// Returns the current delay without incrementing.
    pub fn current_delay(&self) -> Duration {
        self.current_delay
    }

    /// Increment the delay and sleep for the current period.
    ///
    /// Returns `true` if the delay completed normally, `false` if cancelled.
    pub async fn increment_and_wait(&mut self, cancel: CancellationToken) -> bool {
        let delay = self.current_delay;

        tracing::warn!(
            "Error throttling: waiting {:.1}s before retry",
            delay.as_secs_f64()
        );

        let completed = tokio::select! {
            _ = tokio::time::sleep(delay) => true,
            _ = cancel.cancelled() => false,
        };

        // Increment delay for next time
        let next_ms = (delay.as_millis() as f64 * BACKOFF_MULTIPLIER) as u64;
        self.current_delay = Duration::from_millis(next_ms).min(MAX_BACKOFF);

        completed
    }

    /// Just increment the delay without waiting (useful when you handle the delay elsewhere).
    pub fn increment(&mut self) {
        let next_ms = (self.current_delay.as_millis() as f64 * BACKOFF_MULTIPLIER) as u64;
        self.current_delay = Duration::from_millis(next_ms).min(MAX_BACKOFF);
    }
}

impl Default for ErrorThrottler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_delay() {
        let throttler = ErrorThrottler::new();
        assert_eq!(throttler.current_delay(), MIN_BACKOFF);
    }

    #[test]
    fn test_increment() {
        let mut throttler = ErrorThrottler::new();
        throttler.increment();
        assert_eq!(throttler.current_delay(), Duration::from_secs(2));
        throttler.increment();
        assert_eq!(throttler.current_delay(), Duration::from_secs(4));
    }

    #[test]
    fn test_max_backoff() {
        let mut throttler = ErrorThrottler::new();
        for _ in 0..20 {
            throttler.increment();
        }
        assert_eq!(throttler.current_delay(), MAX_BACKOFF);
    }

    #[test]
    fn test_reset() {
        let mut throttler = ErrorThrottler::new();
        throttler.increment();
        throttler.increment();
        throttler.reset();
        assert_eq!(throttler.current_delay(), MIN_BACKOFF);
    }
}
