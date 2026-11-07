//! Merging per-thread accumulators and rendering the schema-1 JSON. Pure
//! functions over plain data (no process state), always compiled, so they
//! are unit-tested with or without the `timings` feature.

use std::fmt::Write as _;

use crate::{N_PHASES, PHASE_NAMES};

/// One thread's accumulators, indexed by [`crate::Phase`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreadPhases {
    /// Wall time of `span`/`stop` spans on this thread.
    pub wall: [u64; N_PHASES],
    /// Number of wall spans.
    pub count: [u64; N_PHASES],
    /// CPU time of `span_cpu` spans on this thread.
    pub cpu: [u64; N_PHASES],
}

/// One phase after merging every thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhaseTotal {
    pub ns: u64,
    pub count: u64,
    /// 0 is omitted from the JSON.
    pub cpu_ns: u64,
}

impl PhaseTotal {
    pub fn is_empty(&self) -> bool {
        self.ns == 0 && self.count == 0 && self.cpu_ns == 0
    }
}

/// `ns` = max wall over threads, `count` = sum, `cpu_ns` = the sum of CPU
/// spans plus, when two or more threads recorded wall, the sum of wall.
pub fn merge(threads: &[ThreadPhases]) -> [PhaseTotal; N_PHASES] {
    let mut totals = [PhaseTotal::default(); N_PHASES];
    for (phase, total) in totals.iter_mut().enumerate() {
        let mut walls = 0u32;
        let mut wall_sum = 0u64;
        for thread in threads {
            let wall = thread.wall[phase];
            if wall > 0 || thread.count[phase] > 0 {
                walls += 1;
            }
            wall_sum += wall;
            total.ns = total.ns.max(wall);
            total.count += thread.count[phase];
            total.cpu_ns += thread.cpu[phase];
        }
        if walls >= 2 {
            total.cpu_ns += wall_sum;
        }
    }
    totals
}

/// Everything one process reports.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub arm: String,
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub exec_to_main_ns: Option<i64>,
    pub wall_ns: u64,
    pub phases: [PhaseTotal; N_PHASES],
    pub marks: Vec<(String, u64)>,
    pub threads_used: Option<u64>,
    pub children: Vec<String>,
}

/// `{"<phase>":{"ns":..,"count":..[,"cpu_ns":..]},...}`, empty phases omitted.
pub fn phases_json(phases: &[PhaseTotal; N_PHASES]) -> String {
    let mut out = String::from("{");
    let mut first = true;
    for (name, total) in PHASE_NAMES.iter().zip(phases) {
        if total.is_empty() {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        let _ = write!(
            out,
            "\"{name}\":{{\"ns\":{},\"count\":{}",
            total.ns, total.count
        );
        if total.cpu_ns > 0 {
            let _ = write!(out, ",\"cpu_ns\":{}", total.cpu_ns);
        }
        out.push('}');
    }
    out.push('}');
    out
}

/// The schema-1 document.
pub fn render(report: &Report) -> String {
    let mut out = String::from("{\"schema\":1,\"arm\":");
    push_string(&mut out, &report.arm);
    let _ = write!(out, ",\"pid\":{},\"parent_pid\":", report.pid);
    match report.parent_pid {
        Some(pid) => {
            let _ = write!(out, "{pid}");
        }
        None => out.push_str("null"),
    }
    out.push_str(",\"exec_to_main_ns\":");
    match report.exec_to_main_ns {
        Some(ns) => {
            let _ = write!(out, "{ns}");
        }
        None => out.push_str("null"),
    }
    let _ = write!(out, ",\"wall_ns\":{},\"phases\":", report.wall_ns);
    out.push_str(&phases_json(&report.phases));
    out.push_str(",\"marks\":{");
    for (index, (name, ns)) in report.marks.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        push_string(&mut out, name);
        let _ = write!(out, ":{ns}");
    }
    out.push('}');
    if let Some(threads) = report.threads_used {
        let _ = write!(out, ",\"threads_used\":{threads}");
    }
    out.push_str(",\"children\":[");
    for (index, child) in report.children.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        push_string(&mut out, child);
    }
    out.push_str("]}\n");
    out
}

/// A JSON string literal.
pub fn push_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

/// `<path>` for the top-level process, else `<path>.<pid>.json`.
pub fn file_for(path: &str, pid: u32, top_level: bool) -> String {
    if top_level {
        path.to_string()
    } else {
        format!("{path}.{pid}.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Phase;

    fn thread(entries: &[(Phase, u64, u64, u64)]) -> ThreadPhases {
        let mut phases = ThreadPhases::default();
        for &(phase, wall, count, cpu) in entries {
            phases.wall[phase as usize] = wall;
            phases.count[phase as usize] = count;
            phases.cpu[phase as usize] = cpu;
        }
        phases
    }

    #[test]
    fn one_thread_reports_its_wall_and_no_cpu() {
        let totals = merge(&[thread(&[(Phase::Parse, 100, 4, 0)])]);
        assert_eq!(
            totals[Phase::Parse as usize],
            PhaseTotal {
                ns: 100,
                count: 4,
                cpu_ns: 0
            }
        );
        assert!(totals[Phase::Transform as usize].is_empty());
    }

    #[test]
    fn several_threads_take_the_max_wall_and_sum_into_cpu() {
        let totals = merge(&[
            thread(&[(Phase::Transform, 300, 3, 0), (Phase::Setup, 10, 3, 0)]),
            thread(&[(Phase::Transform, 500, 5, 0)]),
            thread(&[(Phase::Transform, 200, 2, 0)]),
        ]);
        assert_eq!(
            totals[Phase::Transform as usize],
            PhaseTotal {
                ns: 500,
                count: 10,
                cpu_ns: 1000
            }
        );
        // Only one thread recorded setup: no cpu_ns.
        assert_eq!(
            totals[Phase::Setup as usize],
            PhaseTotal {
                ns: 10,
                count: 3,
                cpu_ns: 0
            }
        );
    }

    #[test]
    fn cpu_spans_add_to_cpu_only() {
        let totals = merge(&[
            thread(&[(Phase::Select, 40, 1, 0)]),
            thread(&[(Phase::Select, 0, 0, 25)]),
            thread(&[(Phase::Select, 0, 0, 30)]),
        ]);
        assert_eq!(
            totals[Phase::Select as usize],
            PhaseTotal {
                ns: 40,
                count: 1,
                cpu_ns: 55
            }
        );
        let worker_only = merge(&[thread(&[(Phase::Parse, 0, 0, 7)])]);
        assert_eq!(
            worker_only[Phase::Parse as usize],
            PhaseTotal {
                ns: 0,
                count: 0,
                cpu_ns: 7
            }
        );
    }

    #[test]
    fn renders_the_schema() {
        let mut phases = [PhaseTotal::default(); N_PHASES];
        phases[Phase::Parse as usize] = PhaseTotal {
            ns: 5,
            count: 2,
            cpu_ns: 9,
        };
        phases[Phase::IdleWait as usize] = PhaseTotal {
            ns: 7,
            count: 1,
            cpu_ns: 0,
        };
        let text = render(&Report {
            arm: "ya\"ml".into(),
            pid: 42,
            parent_pid: None,
            exec_to_main_ns: Some(-3),
            wall_ns: 99,
            phases,
            marks: vec![("engine_done".into(), 11), ("poll_saw".into(), 18)],
            threads_used: Some(12),
            children: vec!["/tmp/t.json.43.json".into()],
        });
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "schema": 1,
                "arm": "ya\"ml",
                "pid": 42,
                "parent_pid": null,
                "exec_to_main_ns": -3,
                "wall_ns": 99,
                "phases": {
                    "parse": { "ns": 5, "count": 2, "cpu_ns": 9 },
                    "idle_wait": { "ns": 7, "count": 1 }
                },
                "marks": { "engine_done": 11, "poll_saw": 18 },
                "threads_used": 12,
                "children": ["/tmp/t.json.43.json"]
            })
        );
        let empty = render(&Report::default());
        let value: serde_json::Value = serde_json::from_str(&empty).unwrap();
        assert_eq!(value["phases"], serde_json::json!({}));
        assert!(value.get("threads_used").is_none());
        assert_eq!(value["exec_to_main_ns"], serde_json::Value::Null);
    }

    #[test]
    fn escapes_control_characters() {
        let mut out = String::new();
        push_string(&mut out, "a\u{1}\n\\");
        assert_eq!(out, "\"a\\u0001\\n\\\\\"");
        assert_eq!(file_for("/t.json", 7, true), "/t.json");
        assert_eq!(file_for("/t.json", 7, false), "/t.json.7.json");
    }

    #[test]
    fn phase_names_round_trip() {
        for phase in Phase::ALL {
            assert_eq!(Phase::from_name(phase.name()), Some(phase));
        }
        assert_eq!(Phase::from_name("nope"), None);
    }
}
