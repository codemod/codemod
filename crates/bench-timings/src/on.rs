//! The `timings` feature: process-wide state behind one flag read from the
//! environment once.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

use crate::report::{self, Report, ThreadPhases};
use crate::{Phase, FILE_ENV, N_PHASES, ROOT_PID_ENV, T0_ENV};

static ON: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os(FILE_ENV).is_some_and(|value| !value.is_empty()));

/// Main start (monotonic raw clock, ns).
static START: OnceLock<u64> = OnceLock::new();
static ARM: Mutex<Option<String>> = Mutex::new(None);
static SLOTS: Mutex<Vec<&'static Slot>> = Mutex::new(Vec::new());
static MARKS: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());
static CHILDREN: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// 0: unset.
static THREADS_USED: AtomicU64 = AtomicU64::new(0);
static FINISHED: OnceLock<Option<String>> = OnceLock::new();

/// One thread's accumulators. Only the owning thread writes (plain
/// load + store, never contended); [`finish`] reads every slot.
struct Slot {
    wall: [AtomicU64; N_PHASES],
    count: [AtomicU64; N_PHASES],
    cpu: [AtomicU64; N_PHASES],
}

impl Slot {
    fn snapshot(&self) -> ThreadPhases {
        let read = |values: &[AtomicU64; N_PHASES]| {
            std::array::from_fn(|index| values[index].load(Ordering::Relaxed))
        };
        ThreadPhases {
            wall: read(&self.wall),
            count: read(&self.count),
            cpu: read(&self.cpu),
        }
    }
}

thread_local! {
    static SLOT: Cell<Option<&'static Slot>> = const { Cell::new(None) };
}

#[inline(always)]
fn bump(value: &AtomicU64, by: u64) {
    value.store(
        value.load(Ordering::Relaxed).wrapping_add(by),
        Ordering::Relaxed,
    );
}

/// This thread's slot, registered (leaked: slots are tiny and read at exit)
/// on first use.
#[inline]
fn with_slot(f: impl FnOnce(&Slot)) {
    SLOT.with(|cell| {
        let slot = match cell.get() {
            Some(slot) => slot,
            None => {
                let slot: &'static Slot = Box::leak(Box::new(Slot {
                    wall: std::array::from_fn(|_| AtomicU64::new(0)),
                    count: std::array::from_fn(|_| AtomicU64::new(0)),
                    cpu: std::array::from_fn(|_| AtomicU64::new(0)),
                }));
                SLOTS
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(slot);
                cell.set(Some(slot));
                slot
            }
        };
        f(slot)
    });
}

/// The monotonic raw clock in nanoseconds (`CLOCK_UPTIME_RAW` on macOS).
#[inline(always)]
pub fn clock_ns() -> u64 {
    #[cfg(target_vendor = "apple")]
    {
        extern "C" {
            // <time.h> on Apple platforms (not bound by the libc crate).
            fn clock_gettime_nsec_np(clock_id: libc::clockid_t) -> u64;
        }
        // SAFETY: no preconditions; reads the commpage clock.
        unsafe { clock_gettime_nsec_np(libc::CLOCK_UPTIME_RAW) }
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        clock(libc::CLOCK_MONOTONIC)
    }
}

/// CPU time of the calling thread in nanoseconds.
#[inline]
pub fn thread_cpu_ns() -> u64 {
    clock(libc::CLOCK_THREAD_CPUTIME_ID)
}

#[inline]
fn clock(id: libc::clockid_t) -> u64 {
    let mut spec = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `spec` is a valid out pointer.
    if unsafe { libc::clock_gettime(id, &mut spec) } != 0 {
        return 0;
    }
    (spec.tv_sec as u64)
        .wrapping_mul(1_000_000_000)
        .wrapping_add(spec.tv_nsec as u64)
}

/// Whether timings are collected in this process.
#[inline(always)]
pub fn enabled() -> bool {
    *ON
}

fn start_ns() -> u64 {
    *START.get_or_init(clock_ns)
}

/// Record the main start (first call wins) and the arm name. Call it first
/// thing in `main`.
pub fn init(arm: &str) {
    if !enabled() {
        return;
    }
    start_ns();
    set_arm(arm);
}

/// Replace the arm name.
pub fn set_arm(arm: &str) {
    if !enabled() {
        return;
    }
    *ARM.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(arm.to_string());
}

/// A started span; `0` when timings are off.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct Timer(u64);

/// Start a wall span.
#[inline(always)]
pub fn start() -> Timer {
    if enabled() {
        Timer(clock_ns())
    } else {
        Timer(0)
    }
}

/// End a wall span started by [`start`] on this thread.
#[inline(always)]
pub fn stop(phase: Phase, timer: Timer) {
    if timer.0 != 0 {
        add(phase, clock_ns().saturating_sub(timer.0), 1);
    }
}

/// `f` as one wall span of `phase` on this thread.
#[inline(always)]
pub fn span<T>(phase: Phase, f: impl FnOnce() -> T) -> T {
    let timer = start();
    let value = f();
    stop(phase, timer);
    value
}

/// `f` as CPU time of `phase` (a worker thread under a region whose wall
/// another thread records).
#[inline(always)]
pub fn span_cpu<T>(phase: Phase, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let started = thread_cpu_ns();
    let value = f();
    add_cpu(phase, thread_cpu_ns().saturating_sub(started));
    value
}

/// Add `ns` of wall time in `count` spans to this thread's `phase`.
#[inline]
pub fn add(phase: Phase, ns: u64, count: u64) {
    if !enabled() {
        return;
    }
    with_slot(|slot| {
        bump(&slot.wall[phase as usize], ns);
        bump(&slot.count[phase as usize], count);
    });
}

/// Add `ns` of CPU time to this thread's `phase`.
#[inline]
pub fn add_cpu(phase: Phase, ns: u64) {
    if !enabled() {
        return;
    }
    with_slot(|slot| bump(&slot.cpu[phase as usize], ns));
}

/// Nanoseconds since main start.
pub fn since_start_ns() -> u64 {
    if !enabled() {
        return 0;
    }
    clock_ns().saturating_sub(start_ns())
}

/// Record `name` at now (ns since main start); a later mark replaces it.
pub fn mark(name: &str) {
    if !enabled() {
        return;
    }
    let at = since_start_ns();
    let mut marks = MARKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match marks.iter_mut().find(|(existing, _)| existing == name) {
        Some(entry) => entry.1 = at,
        None => marks.push((name.to_string(), at)),
    }
}

/// A recorded mark.
pub fn mark_ns(name: &str) -> Option<u64> {
    if !enabled() {
        return None;
    }
    MARKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .find(|(existing, _)| existing == name)
        .map(|(_, at)| *at)
}

/// The size of the pool doing the work.
pub fn set_threads_used(threads: usize) {
    if enabled() {
        THREADS_USED.store(threads as u64, Ordering::Relaxed);
    }
}

/// A child process that writes its own timings file
/// (`<path>.<pid>.json`); listed in `children` if the file exists at exit.
pub fn register_child(pid: u32) {
    if !enabled() {
        return;
    }
    let Some(path) = std::env::var_os(FILE_ENV) else {
        return;
    };
    CHILDREN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(report::file_for(&path.to_string_lossy(), pid, false));
}

/// Every phase merged over the threads so far, as the `phases` JSON object.
pub fn phases_json() -> String {
    report::phases_json(&report::merge(&snapshots()))
}

fn snapshots() -> Vec<ThreadPhases> {
    SLOTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|slot| slot.snapshot())
        .collect()
}

/// Whether this process writes `<path>` (else `<path>.<pid>.json`).
pub fn is_top_level() -> bool {
    match std::env::var(ROOT_PID_ENV) {
        Ok(root) => root.trim() == std::process::id().to_string(),
        Err(_) => true,
    }
}

/// The report as of now.
pub fn report() -> Report {
    let pid = std::process::id();
    // SAFETY: no preconditions.
    let parent = unsafe { libc::getppid() };
    let start = start_ns();
    let exec_to_main_ns = std::env::var(T0_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|t0| start as i64 - t0 as i64);
    let children = CHILDREN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .filter(|path| std::path::Path::new(path).exists())
        .cloned()
        .collect();
    let threads = THREADS_USED.load(Ordering::Relaxed);
    Report {
        arm: ARM
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        pid,
        parent_pid: (parent > 0).then_some(parent as u32),
        exec_to_main_ns,
        wall_ns: clock_ns().saturating_sub(start),
        phases: report::merge(&snapshots()),
        marks: MARKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
        threads_used: (threads > 0).then_some(threads),
        children,
    }
}

/// Write this process's file once (later calls do nothing) and return its
/// path. Call it right before the process exits.
pub fn finish() -> Option<String> {
    if !enabled() {
        return None;
    }
    FINISHED
        .get_or_init(|| {
            let base = std::env::var_os(FILE_ENV)?.to_string_lossy().into_owned();
            let path = report::file_for(&base, std::process::id(), is_top_level());
            let text = report::render(&report());
            std::fs::write(&path, text).ok().map(|()| path)
        })
        .clone()
}
