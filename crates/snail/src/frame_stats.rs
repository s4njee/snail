//! Rolling frame timings for the dev overlay (plan.md E0.8).
//!
//! The overlay shows how long recent frames took, not an average since launch: a stall five minutes
//! ago should not hide in the numbers, and one should not linger after it passes. Samples live in a
//! fixed-size ring and percentiles are taken over what is in it. Adapted from the reference
//! project's `frame_stats.rs`.

use std::collections::VecDeque;
use std::time::Duration;

/// About two seconds of frames at 120 Hz.
pub const DEFAULT_WINDOW: usize = 240;

/// The most recent `capacity` durations of one kind (draw, present, or frame interval).
#[derive(Debug, Clone)]
pub struct FrameWindow {
    samples: VecDeque<Duration>,
    capacity: usize,
}

impl Default for FrameWindow {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW)
    }
}

impl FrameWindow {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Record one frame, dropping the oldest once the window is full.
    pub fn push(&mut self, sample: Duration) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The `p`th percentile in milliseconds (nearest rank; `p` in 0–100), or `None` before any frame.
    pub fn percentile_ms(&self, p: f64) -> Option<f64> {
        if self.samples.is_empty() {
            return None;
        }
        let mut ms: Vec<f64> = self
            .samples
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .collect();
        ms.sort_by(f64::total_cmp);
        let rank = ((p.clamp(0.0, 100.0) / 100.0) * (ms.len() - 1) as f64).round() as usize;
        Some(ms[rank])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn an_empty_window_has_no_percentiles() {
        let window = FrameWindow::new(4);
        assert_eq!(window.percentile_ms(50.0), None);
    }

    #[test]
    fn percentiles_ignore_the_oldest_once_full() {
        let mut window = FrameWindow::new(3);
        for value in [90, 90, 90, 5, 5] {
            window.push(ms(value));
        }
        // Two 90s were evicted; the window holds [90, 5, 5].
        assert_eq!(window.len(), 3);
        assert_eq!(window.percentile_ms(50.0), Some(5.0));
        assert_eq!(window.percentile_ms(100.0), Some(90.0));
    }
}
