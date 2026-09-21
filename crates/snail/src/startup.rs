//! Startup timeline instrumentation (plan.md E0.7).
//!
//! Cheap enough to leave on permanently. It is how a 60 ms blocking read gets found: the marks are
//! logged as they happen and kept for the dev overlay (E0.8) and `--bench` (E17.1).
//!
//! The named marks the plan expects — `gpui_init`, `fonts`, `services`, `models`, `window_opened`,
//! `shell_built`, `first_frame` — are only all present once E1/E2 create fonts, services and
//! models. `mark` is deliberately infallible and lock-light so it can be called anywhere.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Timeline {
    start: Instant,
    marks: Vec<(&'static str, f64)>,
}

fn timeline() -> &'static Mutex<Timeline> {
    static TIMELINE: OnceLock<Mutex<Timeline>> = OnceLock::new();
    TIMELINE.get_or_init(|| {
        Mutex::new(Timeline {
            start: Instant::now(),
            marks: Vec::new(),
        })
    })
}

/// Start the clock. Call first thing in `main`.
pub fn begin() {
    let _ = timeline();
}

/// Record a named point, in milliseconds since [`begin`].
pub fn mark(name: &'static str) {
    let elapsed_ms = {
        let mut timeline = timeline().lock().expect("startup lock poisoned");
        let elapsed_ms = timeline.start.elapsed().as_secs_f64() * 1000.0;
        timeline.marks.push((name, elapsed_ms));
        elapsed_ms
    };
    log::info!("startup {name} @ {elapsed_ms:.1}ms");
}

/// Every mark recorded so far, for the dev overlay.
#[allow(dead_code)] // Read by the E0.8 dev overlay; kept as the stable accessor.
pub fn snapshot() -> Vec<(&'static str, f64)> {
    timeline()
        .lock()
        .expect("startup lock poisoned")
        .marks
        .clone()
}

/// A one-line `gpui_init=12.3ms first_frame=140.1ms` summary.
#[allow(dead_code)] // Read by the E0.8 dev overlay and E17.1's bench.
pub fn summary() -> String {
    snapshot()
        .into_iter()
        .map(|(name, ms)| format!("{name}={ms:.1}ms"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The timeline is process-global and tests run in parallel, so serialize the ones that assert
    /// on it and compare positions rather than absolute indices.
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn marks_are_recorded_in_order_and_monotonic() {
        let _guard = LOCK.lock().unwrap();
        begin();
        mark("gpui_init");
        mark("shell_built");
        let marks = snapshot();
        let first = marks
            .iter()
            .rposition(|(name, _)| *name == "gpui_init")
            .unwrap();
        let second = marks
            .iter()
            .rposition(|(name, _)| *name == "shell_built")
            .unwrap();
        assert!(first < second, "gpui_init should precede shell_built");
        assert!(marks[second].1 >= marks[first].1);
    }

    #[test]
    fn summary_renders_every_mark() {
        let _guard = LOCK.lock().unwrap();
        mark("first_frame");
        let summary = summary();
        assert!(summary.contains("first_frame="), "{summary}");
    }
}
