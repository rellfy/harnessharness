use crate::error::Error;
use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;

const DEFAULT_MAX_ATTEMPTS: u32 = 3;
const DEFAULT_INITIAL_DELAY: Duration = Duration::from_millis(500);
const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(30);
const DEFAULT_MULTIPLIER: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

pub async fn retry<T, F, Fut>(policy: &RetryPolicy, mut operation: F) -> Result<T, Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    let mut failed_attempts = 0;
    loop {
        let error = match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        failed_attempts += 1;
        let is_exhausted = failed_attempts >= policy.max_attempts;
        if is_exhausted || !error.get_is_retryable() {
            return Err(error);
        }
        sleep(policy.delay_after(failed_attempts)).await;
    }
}

impl RetryPolicy {
    pub fn none() -> Self {
        Self::default().max_attempts(1)
    }

    pub fn max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    pub fn initial_delay(mut self, initial_delay: Duration) -> Self {
        self.initial_delay = initial_delay;
        self
    }

    pub fn max_delay(mut self, max_delay: Duration) -> Self {
        self.max_delay = max_delay;
        self
    }

    pub fn multiplier(mut self, multiplier: f64) -> Self {
        self.multiplier = multiplier;
        self
    }

    pub fn delay_after(&self, failed_attempts: u32) -> Duration {
        let exponent = failed_attempts.saturating_sub(1) as i32;
        let delay = self.initial_delay.mul_f64(self.multiplier.powi(exponent));
        delay.min(self.max_delay)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            initial_delay: DEFAULT_INITIAL_DELAY,
            max_delay: DEFAULT_MAX_DELAY,
            multiplier: DEFAULT_MULTIPLIER,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering;

    fn instant_policy(max_attempts: u32) -> RetryPolicy {
        RetryPolicy::default()
            .max_attempts(max_attempts)
            .initial_delay(Duration::ZERO)
    }

    fn retryable_error() -> Error {
        Error::Provider {
            provider: "test",
            status: 503,
            body: String::new(),
        }
    }

    fn fatal_error() -> Error {
        Error::Provider {
            provider: "test",
            status: 400,
            body: String::new(),
        }
    }

    #[tokio::test]
    async fn retries_until_success() {
        let calls = AtomicU32::new(0);
        let result = retry(&instant_policy(3), || async {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            match call < 2 {
                true => Err(retryable_error()),
                false => Ok(call),
            }
        })
        .await;
        assert_eq!(result.unwrap(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn stops_after_max_attempts() {
        let calls = AtomicU32::new(0);
        let result: Result<(), Error> = retry(&instant_policy(3), || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(retryable_error())
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn does_not_retry_fatal_errors() {
        let calls = AtomicU32::new(0);
        let result: Result<(), Error> = retry(&instant_policy(3), || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(fatal_error())
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn delay_grows_exponentially_and_caps() {
        let policy = RetryPolicy::default()
            .initial_delay(Duration::from_secs(1))
            .max_delay(Duration::from_secs(5))
            .multiplier(2.0);
        assert_eq!(policy.delay_after(1), Duration::from_secs(1));
        assert_eq!(policy.delay_after(2), Duration::from_secs(2));
        assert_eq!(policy.delay_after(3), Duration::from_secs(4));
        assert_eq!(policy.delay_after(4), Duration::from_secs(5));
    }
}
