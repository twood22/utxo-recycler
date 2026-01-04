use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Simple in-memory rate limiter using a sliding window.
pub struct RateLimiter {
    /// Map of IP -> (request count, window start time)
    requests: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    /// Maximum requests allowed per window
    max_requests: u32,
    /// Window duration
    window: Duration,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            requests: Mutex::new(HashMap::new()),
            max_requests,
            window: Duration::from_secs(window_secs),
        }
    }

    /// Check if a request from the given IP should be allowed.
    /// Returns Ok(()) if allowed, Err(seconds_until_reset) if rate limited.
    pub async fn check(&self, ip: IpAddr) -> Result<(), u64> {
        let mut requests = self.requests.lock().await;
        let now = Instant::now();

        // Clean up old entries periodically (every 100 requests)
        if requests.len() > 100 {
            requests.retain(|_, (_, start)| now.duration_since(*start) < self.window * 2);
        }

        match requests.get_mut(&ip) {
            Some((count, window_start)) => {
                // Check if window has expired
                if now.duration_since(*window_start) >= self.window {
                    // Reset window
                    *count = 1;
                    *window_start = now;
                    Ok(())
                } else if *count >= self.max_requests {
                    // Rate limited
                    let elapsed = now.duration_since(*window_start);
                    let remaining = self.window.saturating_sub(elapsed);
                    Err(remaining.as_secs() + 1)
                } else {
                    // Increment count
                    *count += 1;
                    Ok(())
                }
            }
            None => {
                // First request from this IP
                requests.insert(ip, (1, now));
                Ok(())
            }
        }
    }

    /// Get the current request count for an IP (for monitoring)
    pub async fn get_count(&self, ip: IpAddr) -> u32 {
        let requests = self.requests.lock().await;
        requests.get(&ip).map(|(count, _)| *count).unwrap_or(0)
    }
}
