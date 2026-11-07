//! `bench-exec <command> [args...]`: set `BENCH_T0_NS` to the monotonic raw
//! clock (`CLOCK_UPTIME_RAW` on macOS) and exec the command in this process,
//! so the command's timings report `exec_to_main_ns` (exec + loader +
//! runtime start until its main). Exit 127 when the command cannot run.

use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(program) = args.next() else {
        eprintln!("usage: bench-exec <command> [args...]");
        return ExitCode::from(2);
    };
    let mut command = Command::new(&program);
    command.args(args);
    command.env(bench_timings::T0_ENV, bench_timings::clock_ns().to_string());
    let error = command.exec();
    eprintln!("bench-exec: {}: {error}", program.to_string_lossy());
    ExitCode::from(127)
}
