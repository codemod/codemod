//! The process-wide state end to end: this test binary is its own process,
//! so it can set `CODEMOD_TIMINGS_FILE` before the first call.

use bench_timings::{Phase, FILE_ENV, ROOT_PID_ENV, T0_ENV};

#[test]
fn writes_one_merged_file_at_finish() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.json");
    let start = bench_timings::clock_ns();
    std::env::set_var(FILE_ENV, &path);
    std::env::remove_var(ROOT_PID_ENV);
    std::env::set_var(T0_ENV, start.saturating_sub(1_000).to_string());
    bench_timings::init("test-arm");
    assert!(bench_timings::enabled());

    bench_timings::span(Phase::Boot, || {
        std::thread::sleep(std::time::Duration::from_millis(2))
    });
    let timer = bench_timings::start();
    bench_timings::stop(Phase::Transform, timer);
    std::thread::scope(|scope| {
        for _ in 0..3 {
            scope.spawn(|| {
                bench_timings::add(Phase::Parse, 10, 1);
                bench_timings::span_cpu(Phase::Read, || {
                    let mut x = 0u64;
                    for i in 0..200_000u64 {
                        x = x.wrapping_add(std::hint::black_box(i));
                    }
                    x
                });
            });
        }
    });
    bench_timings::mark("engine_done");
    bench_timings::set_threads_used(3);
    // A child that wrote its file is listed; one that did not is not.
    let child = format!("{}.4242.json", path.display());
    std::fs::write(&child, "{}").unwrap();
    bench_timings::register_child(4242);
    bench_timings::register_child(4243);

    let written = bench_timings::finish().expect("written");
    assert_eq!(written, path.to_string_lossy());
    // Once only.
    assert_eq!(bench_timings::finish(), Some(written.clone()));

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["arm"], "test-arm");
    assert_eq!(value["pid"], std::process::id());
    assert!(value["parent_pid"].as_u64().is_some());
    assert!(value["exec_to_main_ns"].as_i64().unwrap() >= 1_000);
    let wall = value["wall_ns"].as_u64().unwrap();
    let phases = &value["phases"];
    assert!(phases["boot"]["ns"].as_u64().unwrap() >= 2_000_000);
    assert!(phases["boot"]["ns"].as_u64().unwrap() <= wall);
    assert_eq!(phases["boot"]["count"], 1);
    assert_eq!(phases["transform"]["count"], 1);
    assert_eq!(phases["parse"], serde_json::json!({ "ns": 30, "count": 3 }));
    assert_eq!(phases["read"]["ns"], 0);
    assert!(phases["read"]["cpu_ns"].as_u64().unwrap() > 0);
    assert!(phases.get("commit").is_none());
    assert!(value["marks"]["engine_done"].as_u64().unwrap() <= wall);
    assert_eq!(value["threads_used"], 3);
    assert_eq!(value["children"], serde_json::json!([child]));
}
