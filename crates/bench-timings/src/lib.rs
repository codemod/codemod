//! Phase timings for the runtime benchmark (`benchmarks/runtimes`).
//!
//! Enabled only when the `timings` cargo feature is on **and** the
//! environment variable `CODEMOD_TIMINGS_FILE=<path>` is set. Without the
//! feature every function here is an empty `#[inline(always)]` body and
//! [`Timer`] is a zero-sized type, so instrumented code compiles to what it
//! was. With the feature but without the variable, each call is one load of
//! a never-written flag.
//!
//! Each instrumented process writes exactly one JSON file at exit
//! ([`finish`]): to `<path>` when it is the top-level process, else to
//! `<path>.<pid>.json`. A process is top-level unless
//! `CODEMOD_TIMINGS_ROOT_PID` is set to another process id (the top-level
//! Node CLI sets it for its children). Schema 1:
//!
//! ```json
//! { "schema": 1, "arm": "quickjs-host", "pid": 1, "parent_pid": 0,
//!   "exec_to_main_ns": 1200000, "wall_ns": 52000000,
//!   "phases": { "parse": { "ns": 1, "count": 2, "cpu_ns": 3 } },
//!   "marks": { "engine_done": 40000000 }, "threads_used": 12, "children": [] }
//! ```
//!
//! Accumulators are per thread (each thread owns a slot it alone writes; the
//! slots are only read at exit), so hot paths touch no shared cache line.
//! Merge across threads, per phase: `ns` and `count` are sums, `cpu_ns`
//! (present when non-zero) the sum of CPU time. Wall spans on different
//! threads are sequential parts of one critical path (async code resuming on
//! another runtime thread). Concurrent work records no per-thread wall:
//!
//! - a worker pool runs inside [`parallel`]: its threads' spans go to region
//!   totals, and the owning thread records each phase's total divided by the
//!   pool size as wall (the pool's average per-thread time) and the total as
//!   `cpu_ns`;
//! - a helper thread under a region whose wall another thread records uses
//!   [`span_cpu`] (its CPU time only).
//!
//! `exec_to_main_ns` is the process's main start minus `BENCH_T0_NS`, the
//! monotonic clock read by the `bench-exec` launcher
//! (`benchmarks/runtimes/bench-exec.c`) right before it execs the command:
//! `CLOCK_UPTIME_RAW` on macOS (the clock of `mach_absolute_time` and Node's
//! `process.hrtime`), `CLOCK_MONOTONIC` elsewhere (Node's `process.hrtime`
//! on Linux). A Node parent sets it for the children it times.

/// The environment variable that turns timings on and names the file.
pub const FILE_ENV: &str = "CODEMOD_TIMINGS_FILE";
/// Set by the top-level process for its children (its pid).
pub const ROOT_PID_ENV: &str = "CODEMOD_TIMINGS_ROOT_PID";
/// The launcher's clock reading (nanoseconds, monotonic raw clock).
pub const T0_ENV: &str = "BENCH_T0_NS";

/// The fixed phase names of the timings contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Phase {
    Boot = 0,
    Plan,
    Select,
    Read,
    Setup,
    Parse,
    Transform,
    Convert,
    Conflict,
    Commit,
    IdleWait,
}

/// Number of phases.
pub const N_PHASES: usize = 11;

/// Phase names in [`Phase`] order.
pub const PHASE_NAMES: [&str; N_PHASES] = [
    "boot",
    "plan",
    "select",
    "read",
    "setup",
    "parse",
    "transform",
    "convert",
    "conflict",
    "commit",
    "idle_wait",
];

impl Phase {
    pub const ALL: [Phase; N_PHASES] = [
        Phase::Boot,
        Phase::Plan,
        Phase::Select,
        Phase::Read,
        Phase::Setup,
        Phase::Parse,
        Phase::Transform,
        Phase::Convert,
        Phase::Conflict,
        Phase::Commit,
        Phase::IdleWait,
    ];

    pub const fn name(self) -> &'static str {
        PHASE_NAMES[self as usize]
    }

    pub fn from_name(name: &str) -> Option<Phase> {
        PHASE_NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| Phase::ALL[index])
    }
}

#[cfg(feature = "timings")]
mod on;
#[cfg(feature = "timings")]
pub use on::*;

#[cfg(not(feature = "timings"))]
mod off;
#[cfg(not(feature = "timings"))]
pub use off::*;

/// Pure merge and JSON rendering, independent of the process-wide state.
pub mod report;
