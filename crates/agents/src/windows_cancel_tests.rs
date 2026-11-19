use super::*;
use crate::adapter::{Launch, Parser};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
};

const HELPER: &str = "run::windows_cancel_tests::agent_helper";
const MODE: &str = "CODEMOD_TEST_AGENT_HELPER_MODE";

struct FakeAgent;
impl Parser for FakeAgent {}
impl Adapter for FakeAgent {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn label(&self) -> &'static str {
        "Fake agent"
    }

    fn executables(&self) -> &'static [&'static str] {
        &[]
    }

    fn read_only(&self) -> ReadOnly {
        ReadOnly::Unsupported
    }

    fn launch(&self, _: &Task<'_>, _: &Path) -> Launch {
        Launch::new(Self)
            .args(["--exact", HELPER, "--ignored", "--nocapture"])
            .env(MODE, "child")
    }
}

#[test]
#[ignore = "launched as a subprocess by the cancellation test"]
fn agent_helper() {
    match std::env::var(MODE).as_deref() {
        Ok("child") => {
            let mut grandchild = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", HELPER, "--ignored", "--nocapture"])
                .env(MODE, "grandchild")
                .spawn()
                .unwrap();
            eprintln!("agent-pids {} {}", std::process::id(), grandchild.id());
            let _ = grandchild.wait();
        }
        // Bound the test helper's lifetime even if cleanup is broken.
        Ok("grandchild") => std::thread::sleep(Duration::from_secs(45)),
        _ => {}
    }
}

#[test]
fn canceling_stops_the_windows_agent_and_its_grandchild() {
    let cwd = tempfile::tempdir().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let run = Run {
        adapter: &FakeAgent,
        executable: std::env::current_exe().unwrap(),
        task: Task {
            prompt: "",
            system_prompt: None,
            model: None,
            cwd: cwd.path(),
            autonomy: Autonomy::Full,
        },
        cancel: Some(Arc::clone(&cancel)),
        timeout: Some(Duration::from_secs(20)),
    };
    let mut processes = Vec::new();
    let result = run.run(&mut |event| {
        let AgentEvent::Output {
            stream: Stream::Stderr,
            line,
        } = event
        else {
            return;
        };
        let Some(pids) = line.strip_prefix("agent-pids ") else {
            return;
        };
        for pid in pids.split_whitespace() {
            let pid = pid.parse().unwrap();
            // SAFETY: OpenProcess takes a numeric ID and returns an owned handle.
            let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            assert!(!handle.is_null(), "cannot open process {pid}");
            // SAFETY: ownership of the successful OpenProcess result transfers here.
            let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
            // SAFETY: the handle is valid and has synchronization access.
            assert_eq!(
                unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
                WAIT_TIMEOUT,
                "process {pid} exited before cancellation"
            );
            processes.push((pid, handle));
        }
        cancel.store(true, Ordering::SeqCst);
    });
    assert!(
        matches!(result, Err(RunError::Canceled { .. })),
        "{result:?}"
    );
    assert_eq!(processes.len(), 2);
    // Retain the original handles, so PID recycling cannot fool this check.
    for (pid, handle) in processes {
        // SAFETY: the handle is valid and has synchronization access.
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 5_000) },
            WAIT_OBJECT_0,
            "process {pid} survived cancellation"
        );
    }
}
