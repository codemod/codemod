//! `parallel` regions, pausable `Span`s, and `Residual`s end to end (own process: the
//! flag is read from the environment once).

use bench_timings::{Phase, FILE_ENV, ROOT_PID_ENV};

#[test]
fn regions_average_the_pool_spans_pause_and_residuals_subtract() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.json");
    std::env::set_var(FILE_ENV, &path);
    std::env::remove_var(ROOT_PID_ENV);
    bench_timings::init("regions");

    // Two sequential pools of 4 threads, each thread 40 ns of setup and 10 of
    // parse: per pool 160 + 40 ns, averaged over 4 threads = 40 + 10 ns.
    for _ in 0..2 {
        bench_timings::parallel(4, || {
            std::thread::scope(|scope| {
                for _ in 0..4 {
                    scope.spawn(|| {
                        bench_timings::add(Phase::Setup, 40, 1);
                        bench_timings::add(Phase::Parse, 10, 1);
                    });
                }
            });
        });
    }
    // A region started inside another counts toward the outer one.
    bench_timings::parallel(2, || {
        std::thread::scope(|scope| {
            scope.spawn(|| {
                bench_timings::parallel(1, || bench_timings::add(Phase::Transform, 30, 1));
            });
            scope.spawn(|| bench_timings::add(Phase::Transform, 10, 1));
        });
    });
    // Outside any region, the owner's own spans are plain wall.
    bench_timings::add(Phase::Commit, 7, 1);

    // A span records each running stretch, not the paused time.
    let mut span = bench_timings::Span::new(Phase::Plan);
    std::thread::sleep(std::time::Duration::from_millis(2));
    span.pause();
    std::thread::sleep(std::time::Duration::from_millis(20));
    span.resume();
    drop(span);

    // A residual adds the scope's time minus the spans nested in it.
    let residual = bench_timings::Residual::new(Phase::Convert);
    bench_timings::span(Phase::Read, || {
        std::thread::sleep(std::time::Duration::from_millis(20))
    });
    std::thread::sleep(std::time::Duration::from_millis(2));
    residual.finish();

    bench_timings::finish().expect("written");
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let phases = &value["phases"];
    assert_eq!(
        phases["setup"],
        serde_json::json!({ "ns": 80, "count": 8, "cpu_ns": 320 })
    );
    assert_eq!(
        phases["parse"],
        serde_json::json!({ "ns": 20, "count": 8, "cpu_ns": 80 })
    );
    assert_eq!(
        phases["transform"],
        serde_json::json!({ "ns": 20, "count": 2, "cpu_ns": 40 })
    );
    assert_eq!(phases["commit"], serde_json::json!({ "ns": 7, "count": 1 }));
    let plan = phases["plan"]["ns"].as_u64().unwrap();
    assert_eq!(phases["plan"]["count"], 2);
    assert!(plan >= 2_000_000, "plan {plan}");
    assert!(plan < 20_000_000, "plan {plan} includes the paused time");
    let read = phases["read"]["ns"].as_u64().unwrap();
    let convert = phases["convert"]["ns"].as_u64().unwrap();
    assert!(read >= 20_000_000, "read {read}");
    assert_eq!(phases["convert"]["count"], 1);
    assert!(convert >= 2_000_000, "convert {convert}");
    assert!(
        convert < 20_000_000,
        "convert {convert} includes the nested span"
    );
}
