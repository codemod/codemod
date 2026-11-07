//! Without the `timings` feature: the same API, every body empty.

use crate::report::Report;
use crate::Phase;

#[inline(always)]
pub fn enabled() -> bool {
    false
}

#[inline(always)]
pub fn init(_arm: &str) {}

#[inline(always)]
pub fn set_arm(_arm: &str) {}

/// Zero-sized: a span costs nothing.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct Timer;

#[inline(always)]
pub fn start() -> Timer {
    Timer
}

#[inline(always)]
pub fn stop(_phase: Phase, _timer: Timer) {}

#[derive(Debug)]
#[must_use]
pub struct Span;

impl Span {
    #[inline(always)]
    pub fn new(_phase: Phase) -> Span {
        Span
    }

    #[inline(always)]
    pub fn pause(&mut self) {}

    #[inline(always)]
    pub fn resume(&mut self) {}
}

#[derive(Debug)]
#[must_use]
pub struct Residual;

impl Residual {
    #[inline(always)]
    pub fn new(_phase: Phase) -> Residual {
        Residual
    }

    #[inline(always)]
    pub fn finish(self) {}
}

#[inline(always)]
pub fn span<T>(_phase: Phase, f: impl FnOnce() -> T) -> T {
    f()
}

#[inline(always)]
pub fn span_cpu<T>(_phase: Phase, f: impl FnOnce() -> T) -> T {
    f()
}

#[inline(always)]
pub fn parallel<T>(_threads: usize, f: impl FnOnce() -> T) -> T {
    f()
}

#[inline(always)]
pub fn add(_phase: Phase, _ns: u64, _count: u64) {}

#[inline(always)]
pub fn add_cpu(_phase: Phase, _ns: u64) {}

#[inline(always)]
pub fn since_start_ns() -> u64 {
    0
}

#[inline(always)]
pub fn mark(_name: &str) {}

#[inline(always)]
pub fn mark_ns(_name: &str) -> Option<u64> {
    None
}

#[inline(always)]
pub fn set_threads_used(_threads: usize) {}

#[inline(always)]
pub fn register_child(_pid: u32) {}

#[inline(always)]
pub fn phases_json() -> String {
    String::from("{}")
}

#[inline(always)]
pub fn is_top_level() -> bool {
    true
}

#[inline(always)]
pub fn report() -> Report {
    Report::default()
}

#[inline(always)]
pub fn finish() -> Option<String> {
    None
}
