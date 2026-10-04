use crate::bounded::{module_path_nonce, text_len, transfer_len, EditStream, SF_RTF};
use coralspy_hook_protocol::{
    mapping_name, Architecture, Operation, RecordWriter, RequestHeader, ResponseHeader, ResultKind,
    SharedMemory, Status, MAX_RESULT_BYTES, RECORD_FLAG_CHECKED, RECORD_FLAG_DEFAULT,
    RECORD_FLAG_DEPTH_LIMIT, RECORD_FLAG_DISABLED, RECORD_FLAG_EXPANDED, RECORD_FLAG_HAS_CHILDREN,
    RECORD_FLAG_OWNER_DRAW, RECORD_FLAG_SELECTED, RECORD_FLAG_SEPARATOR,
    RECORD_FLAG_TEXT_TRUNCATED, RESULT_FLAG_TRUNCATED, STATE_COMPLETE, STATE_PENDING,
    STATE_RUNNING,
};
use core::{
    ffi::c_void,
    mem::{size_of, size_of_val, zeroed},
    ptr::{addr_of, addr_of_mut, null_mut},
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, BOOL, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
    },
    Security::{
        CopySid, EqualSid, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation,
        IsValidSid, TokenIntegrityLevel, TokenUser, PSID, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
        TOKEN_USER,
    },
    System::{
        LibraryLoader::GetModuleFileNameW,
        Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE},
        SystemInformation::GetTickCount64,
        Threading::{
            GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetProcessInformation,
            OpenProcess, OpenProcessToken, ProcessProtectionLevelInfo,
            PROCESS_PROTECTION_LEVEL_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
            PROTECTION_LEVEL_NONE,
        },
    },
    UI::{Controls::*, WindowsAndMessaging::*},
};

static MODULE: AtomicUsize = AtomicUsize::new(0);
static BUSY: AtomicBool = AtomicBool::new(false);
static TRIGGER: AtomicU32 = AtomicU32::new(0);
const TRIGGER_NAME: *const u16 = windows_sys::w!("CoralSpyNext.BoundedCapture.v1");
const TEXT_CAP: usize = 2048;
const DEPTH_CAP: usize = 32;
// These RichEdit definitions come from richedit.h, whose structures use pack(4).
const EM_STREAMOUT: u32 = WM_USER + 74;
/// Loader-lock entrypoint: save only the module handle. No file/mapping/window
/// calls, threads, allocation, configuration, or capture happen here.
#[no_mangle]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _: *mut c_void) -> BOOL {
    if reason == 1 {
        // DLL_PROCESS_ATTACH
        MODULE.store(module as usize, Ordering::Relaxed);
    }
    1
}

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// Windows supplies a valid CWPSTRUCT for HC_ACTION. Always forward the hook
/// chain, including malformed requests, recursion, and mismatched architectures.
/// No panic may unwind across this boundary (workspace panic=abort).
#[no_mangle]
pub unsafe extern "system" fn CoralSpyHook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && lparam != 0 {
        let event = &*(lparam as *const CWPSTRUCT);
        let mut trigger = TRIGGER.load(Ordering::Relaxed);
        if trigger == 0 {
            trigger = RegisterWindowMessageW(TRIGGER_NAME);
            if trigger != 0 {
                TRIGGER.store(trigger, Ordering::Relaxed);
            }
        }
        let is_menu = event.message == WM_INITMENU || event.message == WM_INITMENUPOPUP;
        if (is_menu || (trigger != 0 && event.message == trigger))
            && event_is_local(event.hwnd)
            && process_is_unprotected()
            && BUSY
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        {
            let _guard = BusyGuard;
            capture_request(event, is_menu);
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

unsafe fn process_is_unprotected() -> bool {
    let mut protection = PROCESS_PROTECTION_LEVEL_INFORMATION { ProtectionLevel: 0 };
    GetProcessInformation(
        GetCurrentProcess(),
        ProcessProtectionLevelInfo,
        (&mut protection as *mut PROCESS_PROTECTION_LEVEL_INFORMATION).cast(),
        size_of::<PROCESS_PROTECTION_LEVEL_INFORMATION>() as u32,
    ) != 0
        && protection.ProtectionLevel == PROTECTION_LEVEL_NONE
}

// The mapping ACL alone is not enough to establish matching integrity. In
// particular, a desktop hook can visit other same-user processes. Fail closed
// unless the event process and the nonce-request broker have the same SID and IL.
struct TokenIdentity {
    user: [u64; 9], // Aligned storage for a maximal 68-byte SID, no heap allocation.
    integrity: u32,
}

unsafe fn sid_is_in_buffer(sid: PSID, buffer: &[u64; 512], used: u32) -> bool {
    let start = buffer.as_ptr() as usize;
    let end = start.saturating_add((used as usize).min(size_of::<[u64; 512]>()));
    let address = sid as usize;
    if address < start || address.saturating_add(8) > end {
        return false;
    }
    let count = *((sid as *const u8).add(1)) as usize;
    if count > 15 || address.saturating_add(8 + 4 * count) > end {
        return false;
    }
    IsValidSid(sid) != 0
}

unsafe fn token_identity(token: HANDLE) -> Option<TokenIdentity> {
    let mut buffer = [0u64; 512];
    let mut used = 0;
    if GetTokenInformation(
        token,
        TokenUser,
        buffer.as_mut_ptr().cast(),
        size_of_val(&buffer) as u32,
        &mut used,
    ) == 0
        || used < size_of::<TOKEN_USER>() as u32
        || used > size_of_val(&buffer) as u32
    {
        return None;
    }
    let user = (*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid;
    if !sid_is_in_buffer(user, &buffer, used) {
        return None;
    }
    let mut result = TokenIdentity {
        user: [0; 9],
        integrity: 0,
    };
    if CopySid(
        size_of_val(&result.user) as u32,
        result.user.as_mut_ptr().cast(),
        user,
    ) == 0
    {
        return None;
    }
    buffer.fill(0);
    if GetTokenInformation(
        token,
        TokenIntegrityLevel,
        buffer.as_mut_ptr().cast(),
        size_of_val(&buffer) as u32,
        &mut used,
    ) == 0
        || used < size_of::<TOKEN_MANDATORY_LABEL>() as u32
        || used > size_of_val(&buffer) as u32
    {
        return None;
    }
    let label = (*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL))
        .Label
        .Sid;
    if !sid_is_in_buffer(label, &buffer, used) {
        return None;
    }
    let count = *GetSidSubAuthorityCount(label);
    if count == 0 {
        return None;
    }
    result.integrity = *GetSidSubAuthority(label, u32::from(count) - 1);
    Some(result)
}

unsafe fn broker_identity_matches(broker_pid: u32) -> bool {
    if broker_pid == 0 || broker_pid == GetCurrentProcessId() {
        return false;
    }
    let broker = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, broker_pid);
    if broker.is_null() {
        return false;
    }
    let mut own_token = null_mut();
    let mut broker_token = null_mut();
    let opened = OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut own_token) != 0
        && OpenProcessToken(broker, TOKEN_QUERY, &mut broker_token) != 0;
    let matches = if opened {
        match (token_identity(own_token), token_identity(broker_token)) {
            (Some(mut own), Some(mut remote)) => {
                own.integrity == remote.integrity
                    && EqualSid(
                        own.user.as_mut_ptr().cast(),
                        remote.user.as_mut_ptr().cast(),
                    ) != 0
            }
            _ => false,
        }
    } else {
        false
    };
    if !own_token.is_null() {
        CloseHandle(own_token);
    }
    if !broker_token.is_null() {
        CloseHandle(broker_token);
    }
    CloseHandle(broker);
    matches
}

unsafe fn event_is_local(hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return false;
    }
    let mut pid = 0;
    let tid = GetWindowThreadProcessId(hwnd, &mut pid);
    // A cross-bitness desktop hook can execute in the broker's context. Reject
    // that path before any control message containing a local pointer is sent.
    tid != 0 && pid == GetCurrentProcessId() && tid == GetCurrentThreadId()
}

fn architecture() -> Architecture {
    #[cfg(target_pointer_width = "64")]
    {
        Architecture::X64
    }
    #[cfg(target_pointer_width = "32")]
    {
        Architecture::X86
    }
}

unsafe fn module_nonce() -> Option<[u8; 16]> {
    let module = MODULE.load(Ordering::Relaxed) as HINSTANCE;
    if module.is_null() {
        return None;
    }
    let mut path = [0u16; 1024];
    let n = GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) as usize;
    if n == 0 || n >= path.len() {
        return None;
    }
    module_path_nonce(&path[..n])
}

unsafe fn capture_request(event: &CWPSTRUCT, is_menu: bool) {
    let Some(nonce) = module_nonce() else {
        return;
    };
    let ascii = mapping_name(&nonce);
    let mut name = [0u16; 52];
    for (out, byte) in name.iter_mut().zip(ascii.iter()) {
        *out = *byte as u16;
    }
    let handle = OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, 0, name.as_ptr());
    if handle.is_null() {
        return;
    }
    let view = MapViewOfFile(
        handle,
        FILE_MAP_READ | FILE_MAP_WRITE,
        0,
        0,
        size_of::<SharedMemory>(),
    );
    if !view.Value.is_null() {
        consume_mapping(view.Value.cast(), &nonce, event, is_menu);
        UnmapViewOfFile(view);
    }
    CloseHandle(handle);
}

unsafe fn consume_mapping(
    shared: *mut SharedMemory,
    nonce: &[u8; 16],
    event: &CWPSTRUCT,
    is_menu: bool,
) {
    let state = &*addr_of!((*shared).state);
    if state.load(Ordering::Acquire) != STATE_PENDING {
        return;
    }
    // The broker publishes a fully initialized immutable request before installing
    // the hook. Only the state and response are changed after publication.
    let request = core::ptr::read_volatile(addr_of!((*shared).request));
    let Ok(operation) = request.validate(nonce, architecture(), GetTickCount64()) else {
        return;
    };
    if !event_matches(&request, operation, event, is_menu)
        || !broker_identity_matches(request.broker_pid)
    {
        return;
    }
    if GetTickCount64() >= request.expires_at_ms
        || (&*addr_of!((*shared).cancelled)).load(Ordering::Acquire) != 0
    {
        return;
    }
    if state
        .compare_exchange(
            STATE_PENDING,
            STATE_RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return;
    }
    let context = CaptureContext {
        request: &request,
        cancelled: &*addr_of!((*shared).cancelled),
    };
    let mut response = ResponseHeader {
        kind: operation.result_kind() as u32,
        actual_hwnd: event.hwnd as usize as u64,
        actual_pid: GetCurrentProcessId(),
        actual_tid: GetCurrentThreadId(),
        ..ResponseHeader::new(Status::Ok, operation.result_kind(), 0, 0)
    };
    // This slice is restricted to the output region claimed by the winning
    // callback. No &mut SharedMemory is ever constructed across processes.
    let output = core::slice::from_raw_parts_mut(
        addr_of_mut!((*shared).payload).cast::<u8>(),
        (request.max_result_bytes as usize).min(MAX_RESULT_BYTES),
    );
    let status = match operation {
        Operation::RichEditRtf => capture_rtf(event.hwnd, &context, output, &mut response),
        Operation::ListView => capture_list(event.hwnd, &context, output, &mut response),
        Operation::TreeView => capture_tree(event.hwnd, &context, output, &mut response),
        Operation::MenuTarget | Operation::MenuDesktopOnce => {
            capture_menu(event.wParam as HMENU, &context, output, &mut response)
        }
    };
    let status = context.stop().unwrap_or(status);
    if status != Status::Ok && status != Status::Truncated {
        // Failure responses never expose a partial document or incomplete record
        // set. They remain present only during this single mapped request.
        let written = (response.bytes_written as usize).min(output.len());
        output[..written].fill(0);
        response.bytes_written = 0;
        response.record_count = 0;
    }
    response.status = status as u32;
    if status == Status::Truncated {
        response.flags |= RESULT_FLAG_TRUNCATED;
    }
    core::ptr::write_volatile(addr_of_mut!((*shared).response), response);
    state.store(STATE_COMPLETE, Ordering::Release);
}

unsafe fn event_matches(
    request: &RequestHeader,
    op: Operation,
    event: &CWPSTRUCT,
    is_menu: bool,
) -> bool {
    if op == Operation::MenuDesktopOnce {
        return is_menu;
    }
    if request.target_pid != GetCurrentProcessId() || request.target_tid != GetCurrentThreadId() {
        return false;
    }
    let target = request.target_hwnd as usize as HWND;
    if !event_is_local(target) {
        return false;
    }
    if op == Operation::MenuTarget {
        if !is_menu {
            return false;
        }
        // Keep captured identity exactly equal to the broker-validated target.
        // The user must select the owner window that receives menu activation.
        return target == event.hwnd;
    }
    !is_menu && target == event.hwnd
}

struct CaptureContext<'a> {
    request: &'a RequestHeader,
    cancelled: &'a AtomicU32,
}
impl CaptureContext<'_> {
    unsafe fn stop(&self) -> Option<Status> {
        if self.cancelled.load(Ordering::Acquire) != 0 {
            Some(Status::Cancelled)
        } else if GetTickCount64() >= self.request.expires_at_ms {
            Some(Status::Expired)
        } else {
            None
        }
    }
}
macro_rules! check_records {
    ($context:expr, $writer:expr, $response:expr) => {
        if let Some(status) = $context.stop() {
            return finish_records(&$writer, $response, status);
        }
    };
}

unsafe fn has_class(hwnd: HWND, allowed: &[&[u8]]) -> bool {
    let mut class = [0u16; 64];
    let len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
    if len <= 0 || len as usize >= class.len() - 1 {
        return false;
    }
    allowed.iter().any(|name| {
        name.len() == len as usize
            && name
                .iter()
                .zip(class.iter())
                .all(|(a, b)| *b <= 127 && a.eq_ignore_ascii_case(&(*b as u8)))
    })
}

#[cfg(target_pointer_width = "64")]
unsafe fn style(hwnd: HWND) -> u32 {
    GetWindowLongPtrW(hwnd, GWL_STYLE) as u32
}
#[cfg(target_pointer_width = "32")]
unsafe fn style(hwnd: HWND) -> u32 {
    GetWindowLongW(hwnd, GWL_STYLE) as u32
}

struct StreamWriter<'a> {
    output: &'a mut [u8],
    used: usize,
    deadline: u64,
    cancelled: &'a AtomicU32,
    status: Status,
}

unsafe extern "system" fn stream_out(
    cookie: usize,
    bytes: *mut u8,
    length: i32,
    written: *mut i32,
) -> u32 {
    if cookie == 0 || written.is_null() {
        return 1;
    }
    *written = 0;
    let stream = &mut *(cookie as *mut StreamWriter<'_>);
    if length < 0 || (length != 0 && bytes.is_null()) {
        stream.status = Status::ControlError;
        return 1;
    }
    if stream.cancelled.load(Ordering::Acquire) != 0 {
        stream.status = Status::Cancelled;
        return 1;
    }
    if GetTickCount64() >= stream.deadline {
        stream.status = Status::Expired;
        return 1;
    }
    let wanted = length as usize;
    let copy_len = transfer_len(wanted, stream.used, stream.output.len());
    if copy_len != 0 {
        core::ptr::copy_nonoverlapping(
            bytes,
            stream.output.as_mut_ptr().add(stream.used),
            copy_len,
        );
        stream.used += copy_len;
        *written = copy_len as i32;
    }
    if copy_len < wanted {
        stream.status = Status::Truncated;
        return 1;
    }
    0
}

unsafe fn capture_rtf(
    hwnd: HWND,
    context: &CaptureContext<'_>,
    output: &mut [u8],
    response: &mut ResponseHeader,
) -> Status {
    let request = context.request;
    if let Some(status) = context.stop() {
        return status;
    }
    if !has_class(
        hwnd,
        &[b"RichEdit", b"RichEdit20A", b"RichEdit20W", b"RICHEDIT50W"],
    ) {
        return Status::ClassMismatch;
    }
    // This operation never reads a masked control. Do not stream before both checks.
    if style(hwnd) & ES_PASSWORD as u32 != 0 || SendMessageW(hwnd, EM_GETPASSWORDCHAR, 0, 0) != 0 {
        return Status::PasswordControl;
    }
    if let Some(status) = context.stop() {
        return status;
    }
    let mut stream = StreamWriter {
        output,
        used: 0,
        deadline: request.expires_at_ms,
        cancelled: context.cancelled,
        status: Status::Ok,
    };
    let mut edit = EditStream {
        cookie: (&mut stream as *mut StreamWriter<'_>) as usize,
        error: 0,
        callback: stream_out,
    };
    // SF_RTF is 0x0002. The historical 0x0012 is NOT a raw RTF stream format.
    // Output stays byte-exact; UTF-16 conversion would corrupt the RTF stream.
    SendMessageW(
        hwnd,
        EM_STREAMOUT,
        SF_RTF,
        (&mut edit as *mut EditStream) as LPARAM,
    );
    // Preserve the byte count so consume_mapping can wipe on a late timeout.
    response.bytes_written = stream.used as u32;
    if stream.status != Status::Ok {
        return stream.status;
    }
    if edit.error != 0 {
        response.win32_error = edit.error;
        return Status::ControlError;
    }
    context.stop().unwrap_or(Status::Ok)
}

fn finish_records(
    writer: &RecordWriter<'_>,
    response: &mut ResponseHeader,
    status: Status,
) -> Status {
    response.bytes_written = writer.bytes_written() as u32;
    response.record_count = writer.record_count();
    status
}

unsafe fn capture_list(
    hwnd: HWND,
    context: &CaptureContext<'_>,
    output: &mut [u8],
    response: &mut ResponseHeader,
) -> Status {
    let request = context.request;
    if let Some(status) = context.stop() {
        return status;
    }
    if !has_class(hwnd, &[b"SysListView32"]) {
        return Status::ClassMismatch;
    }
    // LVM_GETITEMTEXT is explicitly unsupported for LVS_OWNERDATA. Do not try
    // pointer tricks or synthesize values for a virtualized application model.
    if style(hwnd) & LVS_OWNERDATA != 0 {
        return Status::Unsupported;
    }
    if let Some(status) = context.stop() {
        return status;
    }
    let limits = request.limits();
    let Ok(mut writer) = RecordWriter::new(output, ResultKind::ListView, limits) else {
        return Status::InvalidRequest;
    };
    let row_count = SendMessageW(hwnd, LVM_GETITEMCOUNT, 0, 0);
    check_records!(context, writer, response);
    if row_count < 0 || row_count as u64 > u64::from(u32::MAX) {
        return Status::ControlError;
    }
    let header = SendMessageW(hwnd, LVM_GETHEADER, 0, 0) as HWND;
    check_records!(context, writer, response);
    let source_columns = if header.is_null() {
        0
    } else if event_is_local(header) {
        let count = SendMessageW(header, HDM_GETITEMCOUNT, 0, 0);
        check_records!(context, writer, response);
        if count < 0 || count as u64 > u64::from(u32::MAX) {
            return Status::ControlError;
        }
        count as u32
    } else {
        return Status::ControlError;
    };
    check_records!(context, writer, response);
    response.reported_total_rows = row_count as u32;
    response.reported_total_columns = source_columns;
    let rows = (row_count as u32).min(limits.max_rows);
    // Non-report ListViews can have item labels without a header. Preserve the
    // reported header count (zero) while reading their single data column.
    let data_columns = source_columns.max(1);
    let columns = data_columns.min(limits.max_columns);
    let header_columns = source_columns.min(limits.max_columns);
    let mut status = if rows < row_count as u32 || columns < data_columns {
        Status::Truncated
    } else {
        Status::Ok
    };
    let text_limit = (limits.max_text_units as usize).min(TEXT_CAP);
    let mut buffer = [0u16; TEXT_CAP + 1];
    // Header records precede cells and use the same selected ListView's typed
    // LVM_GETCOLUMNW interface. No pointer-bearing message crosses a process or
    // thread boundary. Shared output capacity still caps the entire response.
    for column in 0..header_columns {
        check_records!(context, writer, response);
        buffer.fill(0);
        let mut header_column: LVCOLUMNW = zeroed();
        header_column.mask = LVCF_TEXT;
        header_column.pszText = buffer.as_mut_ptr();
        header_column.cchTextMax = (text_limit + 1) as i32;
        let found = SendMessageW(
            hwnd,
            LVM_GETCOLUMNW,
            column as WPARAM,
            (&mut header_column as *mut LVCOLUMNW) as LPARAM,
        );
        check_records!(context, writer, response);
        if found == 0 {
            return finish_records(&writer, response, Status::ControlError);
        }
        let len = text_len(&buffer[..text_limit]);
        let flags = if len == text_limit {
            status = Status::Truncated;
            RECORD_FLAG_TEXT_TRUNCATED
        } else {
            0
        };
        if writer.push_column(column, flags, &buffer[..len]).is_err() {
            return finish_records(&writer, response, Status::Truncated);
        }
    }
    for row in 0..rows {
        for column in 0..columns {
            check_records!(context, writer, response);
            buffer.fill(0);
            let mut item: LVITEMW = zeroed();
            item.mask = LVIF_TEXT;
            item.iItem = row as i32;
            item.iSubItem = column as i32;
            item.pszText = buffer.as_mut_ptr();
            item.cchTextMax = (text_limit + 1) as i32;
            let copied = SendMessageW(
                hwnd,
                LVM_GETITEMTEXTW,
                row as usize,
                (&mut item as *mut LVITEMW) as LPARAM,
            );
            check_records!(context, writer, response);
            if copied < 0 || copied as usize > text_limit {
                return finish_records(&writer, response, Status::ControlError);
            }
            let len = text_len(&buffer[..text_limit]).min(copied as usize);
            let flags = if len == text_limit {
                status = Status::Truncated;
                RECORD_FLAG_TEXT_TRUNCATED
            } else {
                0
            };
            if writer
                .push_cell(row, column, flags, &buffer[..len])
                .is_err()
            {
                return finish_records(&writer, response, Status::Truncated);
            }
        }
    }
    finish_records(&writer, response, context.stop().unwrap_or(status))
}

unsafe fn next_tree(hwnd: HWND, relation: u32, item: isize) -> isize {
    SendMessageW(hwnd, TVM_GETNEXTITEM, relation as WPARAM, item as LPARAM)
}

unsafe fn capture_tree(
    hwnd: HWND,
    context: &CaptureContext<'_>,
    output: &mut [u8],
    response: &mut ResponseHeader,
) -> Status {
    let request = context.request;
    if let Some(status) = context.stop() {
        return status;
    }
    if !has_class(hwnd, &[b"SysTreeView32"]) {
        return Status::ClassMismatch;
    }
    if let Some(status) = context.stop() {
        return status;
    }
    let limits = request.limits();
    let Ok(mut writer) = RecordWriter::new(output, ResultKind::TreeView, limits) else {
        return Status::InvalidRequest;
    };
    let total = SendMessageW(hwnd, TVM_GETCOUNT, 0, 0);
    check_records!(context, writer, response);
    if total < 0 {
        return Status::ControlError;
    }
    response.reported_total_nodes = total as u32;
    let mut pending = [0isize; DEPTH_CAP];
    let mut depth = 0usize;
    let mut current = next_tree(hwnd, TVGN_ROOT, 0);
    check_records!(context, writer, response);
    let mut buffer = [0u16; TEXT_CAP + 1];
    let text_limit = (limits.max_text_units as usize).min(TEXT_CAP);
    let mut status = Status::Ok;
    while current != 0 {
        check_records!(context, writer, response);
        if writer.record_count() >= limits.max_nodes {
            return finish_records(&writer, response, Status::Truncated);
        }
        buffer.fill(0);
        let mut item: TVITEMW = zeroed();
        item.mask = TVIF_TEXT | TVIF_STATE;
        item.stateMask = TVIS_EXPANDED | TVIS_SELECTED;
        item.hItem = current;
        item.pszText = buffer.as_mut_ptr();
        item.cchTextMax = (text_limit + 1) as i32;
        let got_item = SendMessageW(hwnd, TVM_GETITEMW, 0, (&mut item as *mut TVITEMW) as LPARAM);
        check_records!(context, writer, response);
        if got_item == 0 {
            return finish_records(&writer, response, Status::ControlError);
        }
        let len = text_len(&buffer[..text_limit]);
        let mut flags = if len == text_limit {
            status = Status::Truncated;
            RECORD_FLAG_TEXT_TRUNCATED
        } else {
            0
        };
        if item.state & TVIS_EXPANDED != 0 {
            flags |= RECORD_FLAG_EXPANDED;
        }
        if item.state & TVIS_SELECTED != 0 {
            flags |= RECORD_FLAG_SELECTED;
        }
        check_records!(context, writer, response);
        let sibling = next_tree(hwnd, TVGN_NEXT, current);
        check_records!(context, writer, response);
        let child = next_tree(hwnd, TVGN_CHILD, current);
        check_records!(context, writer, response);
        if child != 0 {
            flags |= RECORD_FLAG_HAS_CHILDREN;
        }
        if child != 0 && (depth + 1 >= limits.max_depth as usize || depth + 1 >= DEPTH_CAP) {
            flags |= RECORD_FLAG_DEPTH_LIMIT;
        }
        if writer
            .push_tree(depth as u32, writer.record_count(), flags, &buffer[..len])
            .is_err()
        {
            return finish_records(&writer, response, Status::Truncated);
        }
        if child != 0 && depth + 1 < limits.max_depth as usize && depth + 1 < DEPTH_CAP {
            pending[depth] = sibling;
            depth += 1;
            current = child;
        } else {
            if child != 0 {
                status = Status::Truncated;
            }
            current = sibling;
            while current == 0 && depth != 0 {
                depth -= 1;
                current = pending[depth];
                pending[depth] = 0;
            }
        }
    }
    finish_records(&writer, response, context.stop().unwrap_or(status))
}

#[derive(Clone, Copy)]
struct MenuLevel {
    menu: HMENU,
    next: u32,
    count: u32,
}

unsafe fn capture_menu(
    menu: HMENU,
    context: &CaptureContext<'_>,
    output: &mut [u8],
    response: &mut ResponseHeader,
) -> Status {
    let request = context.request;
    if let Some(status) = context.stop() {
        return status;
    }
    if menu.is_null() || IsMenu(menu) == 0 {
        return Status::ControlError;
    }
    if let Some(status) = context.stop() {
        return status;
    }
    response.root_menu = menu as usize as u64;
    let limits = request.limits();
    let Ok(mut writer) = RecordWriter::new(output, ResultKind::Menu, limits) else {
        return Status::InvalidRequest;
    };
    let count = GetMenuItemCount(menu);
    check_records!(context, writer, response);
    if count < 0 {
        response.win32_error = GetLastError();
        return Status::ControlError;
    }
    let empty = MenuLevel {
        menu: null_mut(),
        next: 0,
        count: 0,
    };
    let mut stack = [empty; DEPTH_CAP];
    stack[0] = MenuLevel {
        menu,
        next: 0,
        count: count as u32,
    };
    let mut depth = 0usize;
    let mut status = Status::Ok;
    let text_limit = (limits.max_text_units as usize).min(TEXT_CAP);
    let mut buffer = [0u16; TEXT_CAP + 1];
    loop {
        check_records!(context, writer, response);
        if stack[depth].next >= stack[depth].count {
            if depth == 0 {
                break;
            }
            depth -= 1;
            continue;
        }
        if writer.record_count() >= limits.max_nodes {
            return finish_records(&writer, response, Status::Truncated);
        }
        buffer.fill(0);
        let mut info: MENUITEMINFOW = zeroed();
        info.cbSize = size_of::<MENUITEMINFOW>() as u32;
        // Never request/dereference dwItemData, bitmaps, function pointers or
        // owner-draw payloads. Only Win32's typed menu metadata and text are read.
        info.fMask = MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING;
        info.dwTypeData = buffer.as_mut_ptr();
        info.cch = (text_limit + 1) as u32;
        let got_item = GetMenuItemInfoW(stack[depth].menu, stack[depth].next, 1, &mut info);
        check_records!(context, writer, response);
        if got_item == 0 {
            response.win32_error = GetLastError();
            return finish_records(&writer, response, Status::ControlError);
        }
        stack[depth].next += 1;
        let len = text_len(&buffer[..text_limit]);
        let has_children = !info.hSubMenu.is_null();
        let depth_limited =
            has_children && (depth + 1 >= limits.max_depth as usize || depth + 1 >= DEPTH_CAP);
        let mut flags = 0;
        if len == text_limit {
            flags |= RECORD_FLAG_TEXT_TRUNCATED;
            status = Status::Truncated;
        }
        if has_children {
            flags |= RECORD_FLAG_HAS_CHILDREN;
        }
        if info.fState & MFS_DISABLED != 0 {
            flags |= RECORD_FLAG_DISABLED;
        }
        if info.fState & MFS_CHECKED != 0 {
            flags |= RECORD_FLAG_CHECKED;
        }
        if info.fState & MFS_DEFAULT != 0 {
            flags |= RECORD_FLAG_DEFAULT;
        }
        if info.fType & MFT_SEPARATOR != 0 {
            flags |= RECORD_FLAG_SEPARATOR;
        }
        if info.fType & MFT_OWNERDRAW != 0 {
            flags |= RECORD_FLAG_OWNER_DRAW;
        }
        if depth_limited {
            flags |= RECORD_FLAG_DEPTH_LIMIT;
            status = Status::Truncated;
        }
        if writer
            .push_menu(depth as u32, info.wID, flags, &buffer[..len])
            .is_err()
        {
            return finish_records(&writer, response, Status::Truncated);
        }
        if has_children && !depth_limited {
            check_records!(context, writer, response);
            let count = GetMenuItemCount(info.hSubMenu);
            check_records!(context, writer, response);
            if count < 0 {
                response.win32_error = GetLastError();
                return finish_records(&writer, response, Status::ControlError);
            }
            depth += 1;
            stack[depth] = MenuLevel {
                menu: info.hSubMenu,
                next: 0,
                count: count as u32,
            };
        }
    }
    if status == Status::Ok {
        response.reported_total_nodes = writer.record_count();
    }
    finish_records(&writer, response, context.stop().unwrap_or(status))
}
