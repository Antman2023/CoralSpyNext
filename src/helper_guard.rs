//! Kernel-enforced lifetime containment for this application's own helpers.
//!
//! An unnamed, non-inheritable job handle stays in the parent. Closing the final
//! handle (including abrupt parent exit) terminates every assigned helper, even
//! if its COM call and the parent's watchdog thread cannot make progress.

use std::{
    mem::{size_of, zeroed},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::Child,
    ptr::null,
};
use windows_sys::Win32::{
    Foundation::HANDLE,
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

/// Keep this guard alive until the helper has exited and been reaped.
/// The owned handle is intentionally non-inheritable and never exposed to the
/// child, so the child cannot keep its job alive after the parent terminates.
#[derive(Debug)]
#[must_use = "dropping the job guard terminates its assigned helper"]
pub struct JobGuard {
    _handle: OwnedHandle,
}

/// Bind only the newly spawned helper represented by `child` to a kill-on-close
/// job. Call immediately after spawning, before starting stdout readers or COM
/// work. On failure the caller must kill and wait for its child before returning.
/// A helper-local watchdog covers the small spawn-to-assignment race.
pub fn bind_child(child: &Child) -> Result<JobGuard, String> {
    bind_process(child.as_raw_handle())
}

fn create_job() -> Result<JobGuard, String> {
    // SAFETY: null security attributes create a non-inheritable handle; the null
    // name creates a new private job rather than opening another process's job.
    let handle = unsafe { CreateJobObjectW(null(), null()) };
    if handle.is_null() {
        return Err(crate::localized_format!(
            "创建检查进程生命周期保护失败：{}",
            "Could not create the inspection process lifetime guard: {}",
            std::io::Error::last_os_error()
        ));
    }
    let guard = JobGuard {
        // SAFETY: CreateJobObjectW returned a unique owned, non-null handle.
        _handle: unsafe { OwnedHandle::from_raw_handle(handle) },
    };
    // SAFETY: this C ABI structure permits zero for every field; only the limit
    // flag is enabled. No access permissions or target-process policy changes.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: the handle belongs to this guard, and limits is a live buffer of
    // exactly the documented structure size for this information class.
    if unsafe {
        SetInformationJobObject(
            guard._handle.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(crate::localized_format!(
            "设置检查进程退出保护失败：{}",
            "Could not configure the inspection process exit guard: {}",
            std::io::Error::last_os_error()
        )); // guard closes the unassigned job on this path too.
    }
    Ok(guard)
}

fn bind_process(process: HANDLE) -> Result<JobGuard, String> {
    let guard = create_job()?;
    // SAFETY: the public entry point borrows the process handle from a live
    // Child; Windows validates the handle and assignment permissions. We never
    // open, alter, or assign the inspected target process to this job.
    if unsafe { AssignProcessToJobObject(guard._handle.as_raw_handle(), process) } == 0 {
        return Err(crate::localized_format!(
            "关联检查进程退出保护失败：{}",
            "Could not attach the inspection process exit guard: {}",
            std::io::Error::last_os_error()
        )); // guard closes even if an enclosing job disallows assignment.
    }
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT},
        System::JobObjects::QueryInformationJobObject,
    };

    #[test]
    fn private_job_is_kill_on_close_and_not_inheritable() {
        let guard = create_job().expect("create owned lifecycle job");
        let mut flags = 0;
        assert_ne!(
            unsafe { GetHandleInformation(guard._handle.as_raw_handle(), &mut flags) },
            0
        );
        assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        assert_ne!(
            unsafe {
                QueryInformationJobObject(
                    guard._handle.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    &mut limits as *mut _ as *mut _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(
            limits.BasicLimitInformation.LimitFlags,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        );
    }

    #[test]
    fn invalid_child_handle_fails_without_binding_any_process() {
        let error = bind_process(std::ptr::null_mut()).expect_err("null cannot be assigned");
        assert!(error.contains("关联检查进程退出保护失败"));
    }
}
