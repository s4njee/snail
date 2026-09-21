//! The dev overlay state (plan.md E0.8): frame p50/p99, RSS, the startup timeline, and — once
//! they exist — visible row count, last sync duration and store size.
//!
//! Toggled with F2, or shown from launch with `SNAIL_OVERLAY=1`. Only while visible does the shell
//! schedule a notify per frame, so a closed overlay costs nothing (E0.8, idle CPU budget).

use std::time::Instant;

use gpui::{FrameEvent, FrameTimingCollector};

use crate::frame_stats::FrameWindow;
use crate::rss;

pub struct DevOverlay {
    pub visible: bool,
    pub interval: FrameWindow,
    pub draws: FrameWindow,
    pub presents: FrameWindow,
    collector: FrameTimingCollector,
    last_frame: Option<Instant>,
}

impl DevOverlay {
    pub fn new() -> Self {
        Self {
            visible: std::env::var_os("SNAIL_OVERLAY").is_some(),
            interval: FrameWindow::default(),
            draws: FrameWindow::default(),
            presents: FrameWindow::default(),
            collector: FrameTimingCollector::new(),
            last_frame: None,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        // A reopened overlay should show current frames, not the stall before it was closed.
        self.last_frame = None;
    }

    /// Record the time since the previous frame and drain any profiler frame events.
    pub fn tick(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last_frame {
            self.interval.push(now - last);
        }
        self.last_frame = Some(now);

        let events = self.collector.collect_unseen();
        for event in events {
            match event {
                FrameEvent::Draw(timing) => self.draws.push(timing.draw_duration()),
                FrameEvent::Present(timing) => self
                    .presents
                    .push(timing.present_end.duration_since(timing.present_start)),
            }
        }
    }

    /// The overlay's text, one entry per line.
    pub fn lines(&self, startup: &str) -> Vec<String> {
        let p = |window: &FrameWindow, percentile: f64| {
            window
                .percentile_ms(percentile)
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| "–".to_string())
        };
        let rss = rss::sample();
        vec![
            format!(
                "frame   p50 {:>5}  p99 {:>5} ms",
                p(&self.interval, 50.0),
                p(&self.interval, 99.0)
            ),
            format!(
                "draw    p50 {:>5}  p99 {:>5} ms",
                p(&self.draws, 50.0),
                p(&self.draws, 99.0)
            ),
            format!(
                "present p50 {:>5}  p99 {:>5} ms",
                p(&self.presents, 50.0),
                p(&self.presents, 99.0)
            ),
            format!(
                "rss     {} now  {} peak",
                bytes(rss.current_bytes),
                bytes(rss.peak_bytes)
            ),
            format!("rows  –    store –    last sync –"),
            format!("startup {startup}"),
        ]
    }
}

impl Default for DevOverlay {
    fn default() -> Self {
        Self::new()
    }
}

fn bytes(value: Option<u64>) -> String {
    match value {
        Some(bytes) => format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0)),
        None => "–".to_string(),
    }
}
