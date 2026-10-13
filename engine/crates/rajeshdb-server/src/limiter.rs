//! Token-bucket rate limiter keyed by string (API key id or client IP).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

struct Bucket {
    tokens: f64,
    last: Instant,
}

pub struct Limiter {
    rate: f64,
    burst: f64,
    map: Mutex<HashMap<String, Bucket>>,
}

impl Limiter {
    pub fn new(rate_per_sec: f64, burst: f64) -> Limiter {
        Limiter { rate: rate_per_sec.max(0.0), burst: burst.max(1.0), map: Mutex::new(HashMap::new()) }
    }

    /// Returns true if the request may proceed (and consumes one token).
    pub fn allow(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() > 10_000 {
            m.retain(|_, b| now.duration_since(b.last).as_secs() < 600);
        }
        let burst = self.burst;
        let b = m.entry(key.to_string()).or_insert(Bucket { tokens: burst, last: now });
        let elapsed = now.duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * self.rate).min(burst);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Limiter;

    #[test]
    fn burst_then_block_and_keys_are_independent() {
        let l = Limiter::new(0.0, 3.0);
        assert!(l.allow("a"));
        assert!(l.allow("a"));
        assert!(l.allow("a"));
        assert!(!l.allow("a"));
        assert!(l.allow("b"));
    }
}
