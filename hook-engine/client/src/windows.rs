use super::*;
use std::{
    io::{Read, Write},
    os::windows::io::AsRawHandle,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::JobObjects::*,
};
struct Job(HANDLE);
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn job(child: &std::process::Child) -> Result<Job, CaptureError> {
    unsafe {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() {
            return Err(last("Cannot create bounded capture job"));
        }
        let job = Job(handle);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as _,
            std::mem::size_of_val(&limits) as u32,
        ) == 0
            || AssignProcessToJobObject(handle, child.as_raw_handle() as HANDLE) == 0
        {
            return Err(last("Cannot contain capture broker in a kill-on-close job"));
        }
        Ok(job)
    }
}
fn last(message: &str) -> CaptureError {
    CaptureError::os(ErrorCode::OsError, message, unsafe {
        windows_sys::Win32::Foundation::GetLastError()
    })
}
pub fn capture(request: CaptureRequest, cancel: &AtomicBool) -> CaptureOutcome {
    let exe = std::env::current_exe()
        .map_err(|e| CaptureError::new(ErrorCode::MissingBroker, e.to_string()))?;
    let filename = match request.architecture {
        Architecture::X86 => "coralspy-hook-broker-x86.exe",
        Architecture::X64 => "coralspy-hook-broker-x64.exe",
    };
    let path = exe
        .parent()
        .ok_or_else(|| {
            CaptureError::new(
                ErrorCode::MissingBroker,
                "Cannot find application directory",
            )
        })?
        .join(filename);
    if !path.is_file() {
        return Err(CaptureError::new(
            ErrorCode::MissingBroker,
            format!("Install the matching fixed broker beside the application: {filename}"),
        ));
    }
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err(CaptureError::new(ErrorCode::Cancelled, "Capture cancelled"));
    }
    // No arbitrary executable argument/path, shell, elevation, or alternate payload endpoint.
    let mut child = Command::new(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| CaptureError::new(ErrorCode::OsError, e.to_string()))?;
    let job = match job(&child) {
        Ok(v) => v,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    let mut stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .by_ref()
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let mut wire = serde_json::to_vec(&request)
        .map_err(|e| CaptureError::new(ErrorCode::InvalidInput, e.to_string()))?;
    wire.push(b'\n');
    let mut stdin = child.stdin.take().unwrap();
    let write_result = stdin.write_all(&wire).and_then(|_| stdin.flush());
    if let Err(e) = write_result {
        drop(job);
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        return Err(CaptureError::new(ErrorCode::Ipc, e.to_string()));
    }
    let deadline = Instant::now() + Duration::from_millis(request.limits.timeout_ms as u64 + 3_000);
    let outcome = loop {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            break Err(CaptureError::new(ErrorCode::Cancelled, "Capture cancelled"));
        }
        if Instant::now() >= deadline {
            break Err(CaptureError::new(
                ErrorCode::TimedOut,
                "Capture broker exceeded its bounded deadline",
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => break Err(CaptureError::new(ErrorCode::OsError, e.to_string())),
        }
    };
    if outcome.is_err() {
        let _ = stdin.write_all(b"cancel\n");
        let _ = stdin.flush();
        let grace = Instant::now() + Duration::from_millis(300);
        while Instant::now() < grace {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    drop(job); // Close inherited pipe handles in any broker descendants before joining.
    let bytes = reader
        .join()
        .map_err(|_| CaptureError::new(ErrorCode::Ipc, "Broker response reader failed"))?
        .map_err(|e| CaptureError::new(ErrorCode::Ipc, e.to_string()))?;
    let status = outcome?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(CaptureError::new(
            ErrorCode::Ipc,
            "Broker response exceeds the fixed cap",
        ));
    }
    if !status.success() {
        return Err(CaptureError::new(
            ErrorCode::OsError,
            "Capture broker exited before a complete result",
        ));
    }
    let result: CaptureOutcome = serde_json::from_slice(&bytes)
        .map_err(|e| CaptureError::new(ErrorCode::Ipc, format!("Invalid broker response: {e}")))?;
    if let Ok(ref result) = result {
        if request.target.is_some_and(|t| t != result.actual_target) {
            return Err(CaptureError::new(
                ErrorCode::TargetMismatch,
                "Broker returned a different target",
            ));
        }
        let matches = matches!(
            (request.operation, &result.data),
            (Operation::RichEditRtf, CaptureData::RichEditRtf { .. })
                | (Operation::ListView, CaptureData::ListView { .. })
                | (Operation::TreeView, CaptureData::TreeView { .. })
                | (
                    Operation::MenuTarget | Operation::MenuDesktopOnce,
                    CaptureData::Menu { .. }
                )
        );
        if !matches {
            return Err(CaptureError::new(
                ErrorCode::Ipc,
                "Broker returned an unexpected result type",
            ));
        }
    }
    result
}

pub fn target_architecture(target: Target) -> Result<Architecture, CaptureError> {
    use windows_sys::Win32::{
        Foundation::*,
        System::{SystemInformation::*, Threading::*},
        UI::WindowsAndMessaging::*,
    };
    unsafe {
        if target.hwnd == 0 || target.hwnd > usize::MAX as u64 {
            return Err(CaptureError::new(
                ErrorCode::TargetMismatch,
                "Invalid selected window",
            ));
        }
        let hwnd = target.hwnd as usize as HWND;
        let mut pid = 0;
        let tid = GetWindowThreadProcessId(hwnd, &mut pid);
        if IsWindow(hwnd) == 0 || pid != target.pid || tid != target.tid {
            return Err(CaptureError::new(
                ErrorCode::TargetMismatch,
                "Selected window identity changed",
            ));
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return Err(CaptureError::os(
                ErrorCode::AccessDenied,
                "Cannot query target architecture",
                GetLastError(),
            ));
        }
        let mut pm = 0;
        let mut nm = 0;
        let ok = IsWow64Process2(process, &mut pm, &mut nm);
        let error = GetLastError();
        CloseHandle(process);
        if ok == 0 {
            return Err(CaptureError::os(
                ErrorCode::Unsupported,
                "Cannot determine target architecture",
                error,
            ));
        }
        match if pm == IMAGE_FILE_MACHINE_UNKNOWN {
            nm
        } else {
            pm
        } {
            IMAGE_FILE_MACHINE_I386 => Ok(Architecture::X86),
            IMAGE_FILE_MACHINE_AMD64 => Ok(Architecture::X64),
            _ => Err(CaptureError::new(
                ErrorCode::WrongArchitecture,
                "Only x86 and x64 targets are supported",
            )),
        }
    }
}
