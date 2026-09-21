//! 固定窗口速率限制。
//!
//! 同步与绑定接口只靠 `device_token` 这一把不记名密钥鉴权，一旦被猜中或泄露即可读写
//! 他人计划；注册接口更是完全匿名。限流把这两类滥用的成本抬高到不可行的程度。
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 无论攻击者构造多少不同 key，限流表都不能无界增长。
const MAX_BUCKETS: usize = 32_768;
const CLEANUP_INTERVAL: Duration = Duration::from_secs(30);

struct Bucket {
    window_start: Instant,
    window: Duration,
    count: u32,
}

struct LimiterState {
    buckets: HashMap<String, Bucket>,
    next_cleanup: Instant,
}

#[derive(Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<LimiterState>>,
    max_buckets: usize,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(LimiterState {
                buckets: HashMap::new(),
                next_cleanup: Instant::now(),
            })),
            max_buckets: MAX_BUCKETS,
        }
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次访问；窗口内已达 `limit` 时返回 `false`。
    pub fn allow(&self, key: &str, limit: u32, window: Duration) -> bool {
        let now = Instant::now();
        let mut state = self.state.lock().expect("限流表锁中毒");
        if now >= state.next_cleanup {
            state
                .buckets
                .retain(|_, bucket| now.duration_since(bucket.window_start) < bucket.window);
            state.next_cleanup = now + CLEANUP_INTERVAL;
        }
        if !state.buckets.contains_key(key) && state.buckets.len() >= self.max_buckets {
            // 容量耗尽时失败关闭，避免用随机 token/IP 把进程内存撑爆。
            return false;
        }
        let bucket = state.buckets.entry(key.to_string()).or_insert(Bucket {
            window_start: now,
            window,
            count: 0,
        });
        if now.duration_since(bucket.window_start) >= bucket.window {
            *bucket = Bucket {
                window_start: now,
                window,
                count: 0,
            };
        }
        if bucket.count >= limit {
            return false;
        }
        bucket.count += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_limit_within_window() {
        let limiter = RateLimiter::new();
        let window = Duration::from_secs(60);
        for _ in 0..3 {
            assert!(limiter.allow("ip:1.2.3.4", 3, window));
        }
        assert!(!limiter.allow("ip:1.2.3.4", 3, window));
    }

    #[test]
    fn keys_are_independent() {
        let limiter = RateLimiter::new();
        let window = Duration::from_secs(60);
        assert!(limiter.allow("token:a", 1, window));
        assert!(!limiter.allow("token:a", 1, window));
        assert!(limiter.allow("token:b", 1, window));
    }

    #[test]
    fn allows_again_after_window_elapses() {
        let limiter = RateLimiter::new();
        assert!(limiter.allow("k", 1, Duration::from_millis(0)));
        assert!(limiter.allow("k", 1, Duration::from_millis(0)));
    }

    #[test]
    fn refuses_new_keys_after_capacity_but_keeps_existing_buckets_working() {
        let limiter = RateLimiter {
            state: Arc::new(Mutex::new(LimiterState {
                buckets: HashMap::new(),
                next_cleanup: Instant::now() + CLEANUP_INTERVAL,
            })),
            max_buckets: 2,
        };
        let window = Duration::from_secs(60);
        assert!(limiter.allow("a", 2, window));
        assert!(limiter.allow("b", 1, window));
        assert!(!limiter.allow("c", 1, window));
        assert!(limiter.allow("a", 2, window));
    }
}
