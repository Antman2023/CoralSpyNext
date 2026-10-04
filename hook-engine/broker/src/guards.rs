use coralspy_hook_client::{Architecture, CaptureError, ErrorCode, Target};
use std::{
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::*,
    System::{SystemInformation::*, Threading::*},
    UI::WindowsAndMessaging::*,
};
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn last(code: ErrorCode, msg: &str) -> CaptureError {
    CaptureError::os(code, msg, unsafe { GetLastError() })
}
pub struct Handle(pub HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                CloseHandle(self.0);
            }
        }
    }
}
pub fn token(process: HANDLE) -> Result<Handle, CaptureError> {
    unsafe {
        let mut t = null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut t) == 0 {
            return Err(last(
                ErrorCode::AccessDenied,
                "Cannot query target token; security boundary is respected",
            ));
        }
        Ok(Handle(t))
    }
}
fn token_data(handle: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<u64>, CaptureError> {
    unsafe {
        let mut n = 0;
        GetTokenInformation(handle, class, null_mut(), 0, &mut n);
        if n == 0 || n > 65536 {
            return Err(last(
                ErrorCode::AccessDenied,
                "Invalid token information size",
            ));
        }
        let mut buf = vec![0u64; (n as usize).div_ceil(8)];
        if GetTokenInformation(handle, class, buf.as_mut_ptr() as _, n, &mut n) == 0 {
            return Err(last(
                ErrorCode::AccessDenied,
                "Cannot query security identity",
            ));
        }
        Ok(buf)
    }
}
pub fn sid_string(token: HANDLE) -> Result<String, CaptureError> {
    unsafe {
        let b = token_data(token, TokenUser)?;
        let u = &*(b.as_ptr() as *const TOKEN_USER);
        let mut out = null_mut();
        if Authorization::ConvertSidToStringSidW(u.User.Sid, &mut out) == 0 {
            return Err(last(ErrorCode::AccessDenied, "Cannot read account SID"));
        }
        let mut n = 0;
        while n < 184 && *out.add(n) != 0 {
            n += 1;
        }
        let result = String::from_utf16_lossy(std::slice::from_raw_parts(out, n));
        LocalFree(out as _);
        Ok(result)
    }
}
fn integrity(token: HANDLE) -> Result<u32, CaptureError> {
    unsafe {
        let b = token_data(token, TokenIntegrityLevel)?;
        let label = &*(b.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        if IsValidSid(label.Label.Sid) == 0 {
            return Err(CaptureError::new(
                ErrorCode::AccessDenied,
                "Invalid integrity SID",
            ));
        }
        let n = *GetSidSubAuthorityCount(label.Label.Sid);
        if n == 0 {
            return Err(CaptureError::new(
                ErrorCode::AccessDenied,
                "Missing integrity level",
            ));
        }
        Ok(*GetSidSubAuthority(label.Label.Sid, n as u32 - 1))
    }
}
pub fn architecture(process: HANDLE) -> Result<Architecture, CaptureError> {
    unsafe {
        let mut process_machine = 0;
        let mut native_machine = 0;
        if IsWow64Process2(process, &mut process_machine, &mut native_machine) == 0 {
            return Err(last(ErrorCode::Unsupported, "IsWow64Process2 is required"));
        }
        match if process_machine == IMAGE_FILE_MACHINE_UNKNOWN {
            native_machine
        } else {
            process_machine
        } {
            IMAGE_FILE_MACHINE_I386 => Ok(Architecture::X86),
            IMAGE_FILE_MACHINE_AMD64 => Ok(Architecture::X64),
            _ => Err(CaptureError::new(
                ErrorCode::WrongArchitecture,
                "Only native x86 and x64 targets are supported",
            )),
        }
    }
}
pub fn own_arch() -> Architecture {
    if cfg!(target_pointer_width = "64") {
        Architecture::X64
    } else {
        Architecture::X86
    }
}
pub fn verify_target(target: Target, arch: Architecture) -> Result<Handle, CaptureError> {
    unsafe {
        if target.hwnd > usize::MAX as u64 {
            return Err(CaptureError::new(
                ErrorCode::WrongArchitecture,
                "HWND does not fit this broker",
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
        if pid == GetCurrentProcessId() {
            return Err(CaptureError::new(
                ErrorCode::InvalidInput,
                "Broker cannot capture itself",
            ));
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return Err(last(
                ErrorCode::AccessDenied,
                "Target process is inaccessible; no elevation or bypass is attempted",
            ));
        }
        let process = Handle(h);
        let mut protection: PROCESS_PROTECTION_LEVEL_INFORMATION = std::mem::zeroed();
        if GetProcessInformation(
            h,
            ProcessProtectionLevelInfo,
            &mut protection as *mut _ as _,
            size_of::<PROCESS_PROTECTION_LEVEL_INFORMATION>() as u32,
        ) == 0
        {
            return Err(last(
                ErrorCode::AccessDenied,
                "Cannot verify target protection level",
            ));
        }
        if protection.ProtectionLevel != PROTECTION_LEVEL_NONE {
            return Err(CaptureError::new(
                ErrorCode::AccessDenied,
                "Protected targets are excluded",
            ));
        }
        if architecture(h)? != arch || arch != own_arch() {
            return Err(CaptureError::new(
                ErrorCode::WrongArchitecture,
                "A matching x86/x64 broker and target are required",
            ));
        }
        let theirs = token(h)?;
        let ours = token(GetCurrentProcess())?;
        if sid_string(theirs.0)? != sid_string(ours.0)?
            || integrity(theirs.0)? != integrity(ours.0)?
        {
            return Err(CaptureError::new(ErrorCode::IntegrityMismatch,"Target must belong to the same user and integrity level; restart at matching normal privilege"));
        }
        Ok(process)
    }
}
pub struct PrivateSecurity {
    ptr: PSECURITY_DESCRIPTOR,
}
impl PrivateSecurity {
    pub fn new() -> Result<Self, CaptureError> {
        unsafe {
            let t = token(GetCurrentProcess())?;
            let sid = sid_string(t.0)?;
            let s = wide(&format!("D:P(A;OICI;GA;;;SY)(A;OICI;GA;;;{sid})"));
            let mut ptr = null_mut();
            if Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW(
                s.as_ptr(),
                1,
                &mut ptr,
                null_mut(),
            ) == 0
            {
                return Err(last(
                    ErrorCode::AccessDenied,
                    "Cannot create private IPC security descriptor",
                ));
            }
            Ok(Self { ptr })
        }
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.ptr,
            bInheritHandle: 0,
        }
    }
}
impl Drop for PrivateSecurity {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.ptr as _);
        }
    }
}
pub struct Indicator(pub HWND);
impl Indicator {
    pub fn show(
        desktop: bool,
        pid: Option<u32>,
        timeout: u32,
        language: coralspy_hook_client::UiLanguage,
    ) -> Result<Self, CaptureError> {
        unsafe {
            let message = if desktop {
                format!("CoralSpyNext: DESKTOP MENU CAPTURE\nOne menu only. Stops within {} seconds.\nClose this window to cancel.",timeout.div_ceil(1000))
            } else {
                format!("CoralSpyNext: DEBUG CAPTURE\nSelected PID {}. Stops within {} seconds.\nClose this window to cancel.",pid.unwrap_or(0),timeout.div_ceil(1000))
            };
            let message = if language == coralspy_hook_client::UiLanguage::SimplifiedChinese {
                if desktop {
                    format!("CoralSpyNext：桌面菜单捕获\n仅一次菜单；{} 秒内自动结束。\n关闭此窗口即可取消。",timeout.div_ceil(1000))
                } else {
                    format!("CoralSpyNext：调试捕获\n所选进程 {}；{} 秒内自动结束。\n关闭此窗口即可取消。",pid.unwrap_or(0),timeout.div_ceil(1000))
                }
            } else {
                message
            };
            let class = wide("STATIC");
            let title = wide(&message);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE | 1, /* SS_CENTER, winuser.h */
                32,
                32,
                500,
                125,
                null_mut(),
                null_mut(),
                null_mut(),
                null(),
            );
            if hwnd.is_null() {
                return Err(last(
                    ErrorCode::OsError,
                    "Cannot display required capture indicator",
                ));
            }
            ShowWindow(hwnd, SW_SHOWNORMAL);
            windows_sys::Win32::Graphics::Gdi::UpdateWindow(hwnd);
            if IsWindowVisible(hwnd) == 0 {
                DestroyWindow(hwnd);
                return Err(CaptureError::new(
                    ErrorCode::OsError,
                    "Capture indicator is not visible",
                ));
            }
            Ok(Self(hwnd))
        }
    }
}
impl Drop for Indicator {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(self.0) != 0 {
                DestroyWindow(self.0);
            }
        }
    }
}
