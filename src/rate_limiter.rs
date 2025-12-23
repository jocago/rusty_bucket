use std::thread;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    enabled: bool,
    bytes_per_second: u64,
    window_start: Instant,
    bytes_transferred: u64,
    total_bytes_transferred: u64,
}

impl RateLimiter {
    pub fn new(bytes_per_second: Option<u64>, megabytes_per_minute: Option<u64>) -> Self {
        let (enabled, bytes_per_second) = match (bytes_per_second, megabytes_per_minute) {
            (Some(bps), _) => (true, bps),
            (_, Some(mb_per_min)) => (true, mb_per_min * 1024 * 1024 / 60),
            (None, None) => (false, 0),
        };

        Self {
            enabled,
            bytes_per_second,
            window_start: Instant::now(),
            bytes_transferred: 0,
            total_bytes_transferred: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn get_rate_limit(&self) -> Option<u64> {
        if self.enabled {
            Some(self.bytes_per_second)
        } else {
            None
        }
    }

    pub fn get_current_rate(&self) -> f64 {
        let elapsed = self.window_start.elapsed();
        if elapsed.as_secs_f64() > 0.0 {
            self.bytes_transferred as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        }
    }

    pub fn get_total_transferred(&self) -> u64 {
        self.total_bytes_transferred
    }

    pub fn record_transfer(&mut self, bytes: u64) {
        self.bytes_transferred += bytes;
        self.total_bytes_transferred += bytes;

        // Reset window if we've been tracking for more than 1 second
        if self.window_start.elapsed() >= Duration::from_secs(1) {
            self.window_start = Instant::now();
            self.bytes_transferred = 0;
        }
    }

    pub fn throttle(&mut self) {
        if !self.enabled || self.bytes_per_second == 0 {
            return;
        }

        let target_duration =
            Duration::from_secs_f64(self.bytes_transferred as f64 / self.bytes_per_second as f64);

        let elapsed = self.window_start.elapsed();

        if elapsed < target_duration {
            // We're ahead of schedule, need to slow down
            let sleep_duration = target_duration - elapsed;
            thread::sleep(sleep_duration);

            // Reset tracking after sleeping
            self.window_start = Instant::now();
            self.bytes_transferred = 0;
        }
    }

    pub fn throttle_chunk(&mut self, chunk_size: usize, total_size: u64) {
        if !self.enabled {
            return;
        }

        self.record_transfer(chunk_size as u64);
        self.throttle();

        // Also do progressive throttling for large files
        if total_size > self.bytes_per_second * 10 {
            // For files larger than 10 seconds worth of data at max speed,
            // do more frequent throttling
            let progress = self.total_bytes_transferred as f64 / total_size as f64;
            if progress % 0.1 < 0.01 {
                // Every 10% progress
                self.throttle();
            }
        }
    }
}
