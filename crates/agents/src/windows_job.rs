use std::io;
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
};

pub(crate) struct ProcessGroup(OwnedHandle);

impl ProcessGroup {
    pub(crate) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        let group = Self::new()?;
        // A running child could spawn a descendant before it is assigned.
        let mut child = command.creation_flags(CREATE_SUSPENDED).spawn()?;
        if let Err(error) = group.assign_and_resume(&child) {
            // In particular, an assignment failure leaves a suspended child
            // that closing the job cannot clean up.
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((child, group))
    }

    fn new() -> io::Result<Self> {
        // SAFETY: no custom security attributes or name; a successful call
        // returns a new, non-inheritable handle owned by this process.
        let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: transfer ownership of the newly created handle.
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the buffer has the type and size required by this info class.
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(job))
    }

    fn assign_and_resume(&self, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are valid, owned and stay open during the call.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        resume_primary_thread(child)
    }

    pub(crate) fn stop(self, child: &mut Child) {
        // SAFETY: the job handle remains open for this call.
        unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
        // KILL_ON_JOB_CLOSE also cleans up on unwind or if termination fails.
        drop(self);
        let _ = child.wait();
    }
}

fn resume_primary_thread(child: &Child) -> io::Result<()> {
    // Stable std does not expose the primary thread handle. Since the process
    // was created suspended, the only thread it owns is the primary thread.
    // SAFETY: a system thread snapshot ignores the process ID argument.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful snapshot call returned a new owned handle.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: `entry` is writable and advertises its struct size.
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
    while found != 0 {
        if entry.th32OwnerProcessID == child.id() {
            // SAFETY: the ID came from the snapshot; the suspended child
            // hasn't executed and its process handle is still held.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: OpenThread returned a new owned thread handle.
            let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
            // SAFETY: the handle was opened with THREAD_SUSPEND_RESUME.
            if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            return Ok(());
        }
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        // SAFETY: the same valid snapshot and writable entry as above.
        found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "suspended agent's primary thread was not found",
        ))
    } else {
        Err(error)
    }
}
