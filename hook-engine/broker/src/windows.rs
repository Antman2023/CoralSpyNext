#[path = "guards.rs"]
mod guards;
#[path = "ipc.rs"]
mod ipc;
use coralspy_hook_client::{self as api, CaptureError, CaptureOutcome, ErrorCode};
use coralspy_hook_protocol as p;
use guards::*;
use std::{
    io::{BufRead, BufReader},
    ptr::{addr_of, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::*, SystemInformation::GetTickCount64, Threading::*},
    UI::WindowsAndMessaging::*,
};
struct CancelOnDrop(*mut p::SharedMemory);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        unsafe {
            (*self.0).cancelled.store(1, Ordering::Release);
        }
    }
}
struct Hook(HHOOK);
impl Drop for Hook {
    fn drop(&mut self) {
        unsafe {
            UnhookWindowsHookEx(self.0);
        }
    }
}
fn op(o: api::Operation) -> p::Operation {
    match o {
        api::Operation::RichEditRtf => p::Operation::RichEditRtf,
        api::Operation::ListView => p::Operation::ListView,
        api::Operation::TreeView => p::Operation::TreeView,
        api::Operation::MenuTarget => p::Operation::MenuTarget,
        api::Operation::MenuDesktopOnce => p::Operation::MenuDesktopOnce,
    }
}
fn arch(a: api::Architecture) -> p::Architecture {
    match a {
        api::Architecture::X86 => p::Architecture::X86,
        api::Architecture::X64 => p::Architecture::X64,
    }
}
fn status_error(s: p::Status, win32: u32) -> CaptureError {
    let (code, message) = match s {
        p::Status::ClassMismatch => (
            ErrorCode::ClassMismatch,
            "Selected control class does not match this fixed operation",
        ),
        p::Status::PasswordControl => (
            ErrorCode::PasswordControl,
            "Password-protected controls are excluded",
        ),
        p::Status::AccessDenied => (
            ErrorCode::AccessDenied,
            "Target security boundary denied capture",
        ),
        p::Status::Expired => (ErrorCode::TimedOut, "Capture expired before completion"),
        p::Status::Unsupported => (ErrorCode::Unsupported, "This target/control is unsupported"),
        p::Status::WrongArchitecture => (
            ErrorCode::WrongArchitecture,
            "Payload/target architecture mismatch",
        ),
        p::Status::Cancelled => (ErrorCode::Cancelled, "Capture cancelled"),
        p::Status::ControlError => (ErrorCode::ControlError, "Target control returned an error"),
        _ => (ErrorCode::InvalidRequest, "Invalid capture protocol record"),
    };
    let mut e = CaptureError::new(code, message);
    if win32 != 0 {
        e.win32_error = Some(win32);
    }
    e
}

pub fn main_capture() -> CaptureOutcome {
    // Independent process watchdog bounds blocked setup and pipe input as well as capture.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(15));
        unsafe { ExitProcess(124) }
    });
    if std::env::args_os().len() != 1 {
        return Err(CaptureError::new(
            ErrorCode::InvalidInput,
            "Broker accepts one fixed JSON stdin request and no command-line options",
        ));
    }
    let mut reader = BufReader::new(std::io::stdin());
    let mut line = Vec::new();
    {
        let mut limited = std::io::Read::take(&mut reader, 16 * 1024 + 1);
        limited
            .read_until(b'\n', &mut line)
            .map_err(|e| CaptureError::new(ErrorCode::Ipc, e.to_string()))?;
    }
    if line.len() > 16 * 1024 {
        return Err(CaptureError::new(
            ErrorCode::InvalidInput,
            "Request exceeds fixed input limit",
        ));
    }
    let request: api::CaptureRequest = serde_json::from_slice(&line).map_err(|e| {
        CaptureError::new(
            ErrorCode::InvalidInput,
            format!("Invalid capture request: {e}"),
        )
    })?;
    request.validate()?;
    if request.architecture != own_arch() {
        return Err(CaptureError::new(
            ErrorCode::WrongArchitecture,
            "Use the fixed broker matching this requested architecture",
        ));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let mut line = Vec::new();
        let mut limited = std::io::Read::take(&mut reader, 16);
        let read = limited.read_until(b'\n', &mut line);
        if !matches!(read,Ok(n) if n>0) || line == b"cancel\n" {
            signal.store(true, Ordering::Release);
        }
    });
    unsafe { run(request, &cancel) }
}
unsafe fn run(request: api::CaptureRequest, cancel: &AtomicBool) -> CaptureOutcome {
    if cancel.load(Ordering::Acquire) {
        return Err(CaptureError::new(
            ErrorCode::Cancelled,
            "Capture cancelled before activation",
        ));
    }
    // The target process handle keeps its process identity alive throughout the request.
    let _target = match request.target {
        Some(t) => Some(verify_target(t, request.architecture)?),
        None => None,
    };
    let desktop = matches!(request.operation, api::Operation::MenuDesktopOnce);
    let indicator = Indicator::show(
        desktop,
        request.target.map(|t| t.pid),
        request.limits.timeout_ms,
        request.language,
    )?;
    let security = PrivateSecurity::new()?;
    let nonce = ipc::nonce()?;
    let now = GetTickCount64();
    let target = request
        .target
        .map(|t| p::Target {
            hwnd: t.hwnd,
            pid: t.pid,
            tid: t.tid,
        })
        .unwrap_or_default();
    let mut header = p::RequestHeader::new(
        op(request.operation),
        arch(request.architecture),
        nonce,
        target,
        GetCurrentProcessId(),
        now,
        request.limits.timeout_ms,
    )
    .map_err(|s| status_error(s, 0))?;
    header.max_rows = request.limits.max_rows;
    header.max_columns = request.limits.max_columns;
    header.max_nodes = request.limits.max_nodes;
    header.max_depth = request.limits.max_depth;
    header.max_text_units = request.limits.max_text_units;
    header.max_result_bytes = request.limits.max_result_bytes;
    header
        .validate_for_broker(
            &nonce,
            GetCurrentProcessId(),
            arch(request.architecture),
            now,
        )
        .map_err(|s| status_error(s, 0))?;
    let mapping = ipc::Mapping::new(header, &security)?;
    let dll = ipc::EmbeddedDll::create(&nonce, &security)?;
    let module = ipc::Module::load(&dll)?;
    let proc = GetProcAddress(module.0, c"CoralSpyHook".as_ptr().cast())
        .ok_or_else(|| last(ErrorCode::OsError, "Built-in payload export is absent"))?;
    let callback: HOOKPROC = Some(std::mem::transmute::<
        unsafe extern "system" fn() -> isize,
        unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT,
    >(proc));
    // Revalidate immediately before installation; never elevate or broaden the chosen target.
    if let Some(t) = request.target {
        let _ = verify_target(t, request.architecture)?;
    }
    if cancel.load(Ordering::Acquire) || IsWindowVisible(indicator.0) == 0 {
        return Err(CaptureError::new(
            ErrorCode::Cancelled,
            "Required capture indicator was closed",
        ));
    }
    let h = SetWindowsHookExW(
        WH_CALLWNDPROC,
        callback,
        module.0,
        if desktop { 0 } else { target.tid },
    );
    if h.is_null() {
        return Err(last(
            ErrorCode::AccessDenied,
            "Windows refused the bounded hook; no privilege bypass is attempted",
        ));
    }
    let hook = Hook(h);
    // Declared after Hook so all early-return paths signal cancellation before unhook.
    let _cancel_on_drop = CancelOnDrop(mapping.ptr);
    if !matches!(
        request.operation,
        api::Operation::MenuTarget | api::Operation::MenuDesktopOnce
    ) {
        let message = RegisterWindowMessageW(wide("CoralSpyNext.BoundedCapture.v1").as_ptr());
        if message == 0 || SendNotifyMessageW(target.hwnd as usize as HWND, message, 0, 0) == 0 {
            return Err(last(
                ErrorCode::AccessDenied,
                "Target message was blocked or target disappeared",
            ));
        }
    }
    let result = loop {
        let mut message: MSG = std::mem::zeroed();
        // A flooded message queue must not postpone the capture deadline.
        for _ in 0..64 {
            if cancel.load(Ordering::Acquire)
                || GetTickCount64() >= header.expires_at_ms
                || PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) == 0
            {
                break;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if cancel.load(Ordering::Acquire)
            || IsWindow(indicator.0) == 0
            || IsWindowVisible(indicator.0) == 0
        {
            break Err(CaptureError::new(
                ErrorCode::Cancelled,
                "Capture cancelled; hook removed",
            ));
        }
        if GetTickCount64() >= header.expires_at_ms {
            break Err(CaptureError::new(
                ErrorCode::TimedOut,
                "No complete result before the capture deadline",
            ));
        }
        let state = (*mapping.ptr).state.load(Ordering::Acquire);
        if state == p::STATE_COMPLETE {
            break decode(mapping.ptr, &header);
        }
        if state != p::STATE_PENDING && state != p::STATE_RUNNING {
            break Err(CaptureError::new(ErrorCode::Ipc, "Invalid IPC state"));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    (*mapping.ptr).cancelled.store(1, Ordering::Release);
    drop(hook); // Explicit ordering: unhook, unload broker module, unlock/remove DLL, unmap.
    result
}
unsafe fn decode(mapping: *mut p::SharedMemory, request: &p::RequestHeader) -> CaptureOutcome {
    let response = addr_of!((*mapping).response).read_volatile();
    let status = response.validate(request).map_err(|s| status_error(s, 0))?;
    if !matches!(status, p::Status::Ok | p::Status::Truncated) {
        return Err(status_error(status, response.win32_error));
    }
    let bytes = std::slice::from_raw_parts(
        addr_of!((*mapping).payload) as *const u8,
        response.bytes_written as usize,
    )
    .to_vec();
    response
        .validate_payload(request, &bytes)
        .map_err(|s| status_error(s, 0))?;
    let kind = p::ResultKind::try_from(response.kind).map_err(|s| status_error(s, 0))?;
    let invalid = || CaptureError::new(ErrorCode::Ipc, "Invalid bounded result records");
    let data = if kind == p::ResultKind::Rtf {
        api::CaptureData::RichEditRtf {
            bytes,
            complete: status == p::Status::Ok,
        }
    } else {
        let records =
            p::RecordReader::new(&bytes, kind, request.limits()).map_err(|_| invalid())?;
        let mut columns = Vec::new();
        let mut cells = Vec::new();
        let mut nodes = Vec::new();
        let mut items = Vec::new();
        let mut count = 0;
        for r in records {
            let r = r.map_err(|_| invalid())?;
            count += 1;
            let text = String::from_utf16_lossy(&r.text.collect::<Vec<_>>());
            match kind {
                p::ResultKind::ListView => {
                    if r.kind == p::RecordKind::Column {
                        columns.push(api::ListColumn {
                            index: r.id,
                            flags: r.flags,
                            text,
                        });
                    } else {
                        cells.push(api::Cell {
                            row: r.id,
                            column: r.depth,
                            flags: r.flags,
                            text,
                        });
                    }
                }
                p::ResultKind::TreeView => nodes.push(api::TreeNode {
                    depth: r.depth,
                    id: r.id,
                    flags: r.flags,
                    text,
                }),
                p::ResultKind::Menu => items.push(api::MenuItem {
                    depth: r.depth,
                    id: r.id,
                    flags: r.flags,
                    text,
                }),
                _ => return Err(invalid()),
            }
        }
        if count != response.record_count {
            return Err(invalid());
        }
        match kind {
            p::ResultKind::ListView => api::CaptureData::ListView { columns, cells },
            p::ResultKind::TreeView => api::CaptureData::TreeView { nodes },
            p::ResultKind::Menu => api::CaptureData::Menu {
                root_menu: response.root_menu,
                items,
            },
            _ => return Err(invalid()),
        }
    };
    Ok(api::CaptureResult {
        actual_target: api::Target {
            hwnd: response.actual_hwnd,
            pid: response.actual_pid,
            tid: response.actual_tid,
        },
        truncated: status == p::Status::Truncated,
        reported_counts: api::ReportedCounts {
            rows: known(response.reported_total_rows),
            columns: known(response.reported_total_columns),
            nodes: known(response.reported_total_nodes),
        },
        captured_records: response.record_count,
        data,
    })
}

fn known(value: u32) -> Option<u32> {
    if value == p::UNKNOWN_COUNT {
        None
    } else {
        Some(value)
    }
}
