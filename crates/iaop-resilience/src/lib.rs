#![forbid(unsafe_code)]
//! Bounded retry, timeout, and circuit-breaker primitives.
//! Retries are opt-in per call and classified — never retry
//! non-idempotent side effects automatically.

use std::future::Future;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RetryError<E> {
    #[error("retry exhausted after {attempts} attempts: {last}")]
    Exhausted { attempts: u32, last: E },
    #[error("cancelled")]
    Cancelled,
    #[error("timed out after {0:?}")]
    Timeout(Duration),
}

/// Backoff schedule: exponential with full jitter, bounded by `max`.
pub fn backoff(attempt: u32, base: Duration, max: Duration) -> Duration {
    let exp = base.saturating_mul(1u32 << attempt.min(10));
    let capped = exp.min(max);
    // jitter: 50%..100% of capped
    let nanos = capped.as_nanos() as u64;
    let half = nanos / 2;
    Duration::from_nanos(half + (nanos - half) * (rand_simple() % 100) / 100)
}

fn rand_simple() -> u64 {
    // xorshift seeded from system time — sufficient for jitter, not crypto.
    use std::time::SystemTime;
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
        .unwrap_or(0x9e3779b97f4a7c15);
    t.wrapping_mul(0x2545F4914F6CDD1D) >> 16
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub max_elapsed: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
            max_elapsed: Duration::from_secs(30),
        }
    }
}

/// Executes `f` with bounded exponential backoff+jitter.
/// `retryable` classifies whether an error merits another attempt.
/// The future is recreated each attempt (f must be `FnMut -> Fut`).
pub async fn retry<F, Fut, T, E>(
    policy: &RetryPolicy,
    mut retryable: impl FnMut(&E) -> bool,
    mut f: F,
) -> Result<T, RetryError<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let start = Instant::now();
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt >= policy.max_attempts || !retryable(&e) {
                    return Err(RetryError::Exhausted {
                        attempts: attempt,
                        last: e,
                    });
                }
                if start.elapsed() + backoff(attempt, policy.base_delay, policy.max_delay)
                    > policy.max_elapsed
                {
                    return Err(RetryError::Exhausted {
                        attempts: attempt,
                        last: e,
                    });
                }
                tokio::time::sleep(backoff(attempt, policy.base_delay, policy.max_delay)).await;
            }
        }
    }
}

/// Wraps a future in an explicit timeout.
pub async fn with_timeout<F, T, E>(dur: Duration, f: F) -> Result<T, RetryError<E>>
where
    F: Future<Output = Result<T, E>>,
{
    match tokio::time::timeout(dur, f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(RetryError::Exhausted {
            attempts: 1,
            last: e,
        }),
        Err(_) => Err(RetryError::Timeout(dur)),
    }
}

/// Minimal circuit breaker: closed -> open after `threshold` consecutive
/// failures, half-open probe after `reset_after`.
#[derive(Debug)]
pub struct CircuitBreaker {
    threshold: u32,
    reset_after: Duration,
    failures: AtomicU32,
    opened_at: AtomicU64,
    state: std::sync::Mutex<CbState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CbState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Error)]
#[error("circuit breaker is open")]
pub struct CircuitOpen;

impl CircuitBreaker {
    pub fn new(threshold: u32, reset_after: Duration) -> Self {
        Self {
            threshold,
            reset_after,
            failures: AtomicU32::new(0),
            opened_at: AtomicU64::new(0),
            state: std::sync::Mutex::new(CbState::Closed),
        }
    }

    pub fn allow(&self) -> bool {
        let mut st = self.state.lock().unwrap();
        match *st {
            CbState::Closed => true,
            CbState::Open => {
                let opened_ms = self.opened_at.load(Ordering::Relaxed);
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if now_ms.saturating_sub(opened_ms) >= self.reset_after.as_millis() as u64 {
                    *st = CbState::HalfOpen;
                    true
                } else {
                    false
                }
            }
            CbState::HalfOpen => true,
        }
    }

    pub fn record_success(&self) {
        self.failures.store(0, Ordering::Relaxed);
        *self.state.lock().unwrap() = CbState::Closed;
    }

    pub fn record_failure(&self) {
        let n = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= self.threshold {
            *self.state.lock().unwrap() = CbState::Open;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            self.opened_at.store(now_ms, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn retry_succeeds_after_failures() {
        let mut calls = 0;
        let r = retry(
            &RetryPolicy {
                max_attempts: 3,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                max_elapsed: Duration::from_secs(5),
            },
            |_| true,
            || {
                calls += 1;
                async move { if calls < 3 { Err("boom") } else { Ok(42) } }
            },
        )
        .await;
        assert_eq!(r.unwrap(), 42);
    }

    #[tokio::test]
    async fn retry_does_not_retry_permanent() {
        let mut calls = 0;
        let r: Result<i32, RetryError<&str>> = retry(
            &RetryPolicy::default(),
            |e: &&str| *e != "fatal",
            || {
                calls += 1;
                async move { Err("fatal") }
            },
        )
        .await;
        assert!(matches!(r, Err(RetryError::Exhausted { attempts: 1, .. })));
    }

    #[tokio::test]
    async fn timeout_fires() {
        let r: Result<(), RetryError<&str>> = with_timeout(Duration::from_millis(10), async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(())
        })
        .await;
        assert!(matches!(r, Err(RetryError::Timeout(_))));
    }

    #[test]
    fn circuit_opens_after_threshold() {
        let cb = CircuitBreaker::new(2, Duration::from_secs(60));
        assert!(cb.allow());
        cb.record_failure();
        assert!(cb.allow());
        cb.record_failure();
        assert!(!cb.allow());
        cb.record_success();
        assert!(cb.allow());
    }
}
