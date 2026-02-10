use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct RateWindow {
    pub limit: u32,
    pub window: Duration,
}

#[derive(Debug, Clone)]
pub struct RateLimiter {
    window: RateWindow,
    count: u32,
    start: Instant,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            window: RateWindow { limit, window },
            count: 0,
            start: Instant::now(),
        }
    }

    pub fn check(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.start) >= self.window.window {
            self.start = now;
            self.count = 0;
        }
        self.count += 1;
        self.count <= self.window.limit
    }
}
