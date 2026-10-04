//! Read-only Win32 metadata backend for CoralSpyNext.
//!
//! We never send WM_GETTEXT to foreign controls, read process memory, install
//! hooks, inject code, or request elevation. All strings cross Win32 as UTF-16.
//! Enumeration and inspection belong on a worker thread; only pointer/color
//! sampling and the explicitly allowed picking keys are intended per frame.

use crate::model::{ColorSample, WindowInfo, WindowNode, WindowRect};
use std::{
    cell::RefCell,
    collections::HashSet,
    ffi::OsString,
    mem::size_of,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    ptr::null_mut,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, SetLastError, BOOL, HANDLE, HWND, LPARAM, POINT, RECT,
    },
    Graphics::Gdi::{GetDC, GetPixel, MapWindowPoints, ReleaseDC, CLR_INVALID, HDC},
    System::Threading::{
        GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW,
        PROCESS_QUERY_LIMITED_INFORMATION,
    },
    UI::{
        Controls::Dialogs::{
            CommDlgExtendedError, GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR,
            OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
        },
        HiDpi::{
            GetDpiForWindow, SetProcessDpiAwarenessContext,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        },
        Input::KeyboardAndMouse::{GetAsyncKeyState, IsWindowEnabled},
        WindowsAndMessaging::{
            ChildWindowFromPointEx, EnumChildWindows, EnumWindows, GetAncestor, GetClassNameW,
            GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowLongW, GetWindowRect,
            GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowUnicode,
            IsWindowVisible, IsZoomed, WindowFromPoint, CWP_SKIPINVISIBLE, GA_PARENT, GA_ROOT,
            GWL_EXSTYLE, GWL_STYLE, WS_CHILD,
        },
    },
};

pub const ENUM_MAX_WINDOWS: usize = 6_000;
pub const ENUM_MAX_DEPTH: usize = 32;
const ENUM_MAX_VISITS: usize = 24_000;
const ENUM_BUDGET: Duration = Duration::from_millis(2_500);
const TITLE_CAPACITY: usize = 2_048;

thread_local! {
    static ENUM_NOTICE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Read this on the same worker thread immediately after `enumerate_windows`.
/// A notice means the returned snapshot is valid but intentionally partial.
pub fn enumeration_notice() -> Option<String> {
    ENUM_NOTICE.with(|notice| notice.borrow().clone())
}

fn handle_value(hwnd: HWND) -> u64 {
    hwnd as usize as u64
}

fn checked_handle(value: u64) -> Result<HWND, String> {
    let address =
        usize::try_from(value).map_err(|_| "窗口句柄超出当前程序的指针宽度".to_owned())?;
    if address == 0 {
        return Err("窗口句柄不能为空".to_owned());
    }
    Ok(address as HWND)
}

fn win32_error(operation: &str, code: u32) -> String {
    if code == 0 {
        format!("{operation}失败；窗口可能已关闭或当前桌面不可访问")
    } else {
        format!(
            "{operation}失败（Win32 {code}：{}）",
            std::io::Error::from_raw_os_error(code as i32)
        )
    }
}

fn last_error(operation: &str) -> String {
    // SAFETY: GetLastError is thread-local and has no pointer arguments.
    win32_error(operation, unsafe { GetLastError() })
}

fn class_name(hwnd: HWND) -> Result<String, String> {
    let mut text = [0u16; 256];
    // SAFETY: text is a live, writable buffer; HWND is borrowed and may safely
    // become invalid between calls (Win32 then returns failure).
    let count = unsafe { GetClassNameW(hwnd, text.as_mut_ptr(), text.len() as i32) };
    if count <= 0 {
        return Err(last_error("读取窗口类型"));
    }
    Ok(String::from_utf16_lossy(&text[..count as usize]))
}

/// Conservative privacy filter, including common Edit/RichEdit wrappers.
fn is_input_class(class: &str) -> bool {
    let class = class.to_ascii_lowercase();
    class.contains("edit")
        || class.contains("password")
        || class.contains("textbox")
        || class.contains("textinput")
        || class.contains("combobox")
        || class == "scintilla"
        || class.ends_with("memo")
}

fn decode_title(text: &[u16], possibly_truncated: bool) -> String {
    // A full buffer can split a surrogate pair. Drop only that incomplete
    // boundary instead of introducing a replacement character into a title.
    let text = if possibly_truncated
        && text
            .last()
            .is_some_and(|unit| (0xD800..=0xDBFF).contains(unit))
    {
        &text[..text.len() - 1]
    } else {
        text
    };
    String::from_utf16_lossy(text)
}

fn safe_title(hwnd: HWND, class: &str, pid: u32) -> (String, String) {
    if is_input_class(class) {
        return (
            "[输入控件：仅元数据]".to_owned(),
            "隐私保护：不读取 Edit、RichEdit、密码及常见输入控件的文本".to_owned(),
        );
    }
    // Unknown custom controls can store user input in the same Win32 caption
    // slot as ordinary labels. A class denylist is insufficient: never read
    // any descendant's caption, regardless of its class name.
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if style & WS_CHILD != 0 || root != hwnd {
        return (
            "[子控件：仅元数据]".to_owned(),
            "隐私保护：所有子控件（含未知自定义控件）均不读取文本，仅检查元数据".to_owned(),
        );
    }
    // GetWindowTextW sends WM_GETTEXT for *same-process* HWNDs, which could
    // deadlock against our own GUI. Do not invoke it for any of our controls.
    if pid == unsafe { GetCurrentProcessId() } {
        return (
            "[CoralSpyNext 自身窗口]".to_owned(),
            "自身窗口仅显示元数据，避免同步读取阻塞界面".to_owned(),
        );
    }
    let mut text = [0u16; TITLE_CAPACITY];
    // For a foreign top-level window this reads OS-maintained caption metadata and
    // does not wait for that process's window procedure. No control messages.
    let (count, error) = unsafe {
        SetLastError(0);
        let count = GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32);
        (count, GetLastError())
    };
    if count <= 0 {
        let status = if error != 0 {
            win32_error("读取缓存标题", error)
        } else {
            "无缓存标题；未尝试读取应用内容或控件文本".to_owned()
        };
        return (String::new(), status);
    }
    let mut status = "仅读取顶层窗口的缓存标题；不读取子控件或输入文本".to_owned();
    let possibly_truncated = count as usize == text.len() - 1;
    if possibly_truncated {
        status.push_str("；标题可能已截断（最多 2047 个 UTF-16 单元）");
    }
    (
        decode_title(&text[..count as usize], possibly_truncated),
        status,
    )
}

struct TopLevels {
    started: Instant,
    handles: Vec<HWND>,
    limited: bool,
}

unsafe extern "system" fn collect_top_level(hwnd: HWND, parameter: LPARAM) -> BOOL {
    // SAFETY: EnumWindows invokes this synchronously with the exact live
    // TopLevels pointer supplied below, on the calling thread.
    let state = &mut *(parameter as *mut TopLevels);
    if state.handles.len() >= ENUM_MAX_WINDOWS || state.started.elapsed() >= ENUM_BUDGET {
        state.limited = true;
        return 0;
    }
    state.handles.push(hwnd);
    1
}

struct Enumeration {
    started: Instant,
    nodes: Vec<WindowNode>,
    seen: HashSet<u64>,
    root: HWND,
    visits: usize,
    notice: Option<&'static str>,
}

impl Enumeration {
    fn exhausted(&mut self) -> bool {
        if self.nodes.len() >= ENUM_MAX_WINDOWS || self.visits >= ENUM_MAX_VISITS {
            self.notice =
                Some("窗口数量达到安全上限，当前显示部分结果；可使用指针直接检查未列出的窗口。");
            true
        } else if self.started.elapsed() >= ENUM_BUDGET {
            self.notice =
                Some("窗口枚举达到 2.5 秒时间上限，当前显示部分结果；可刷新或使用指针直接检查。");
            true
        } else {
            false
        }
    }

    fn push(&mut self, hwnd: HWND, parent: u64, depth: usize) {
        let value = handle_value(hwnd);
        if self.seen.contains(&value) {
            return;
        }
        let mut pid = 0;
        // SAFETY: pid points to a live u32; stale HWNDs are rejected by Win32.
        let tid = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if tid == 0 || pid == 0 {
            return;
        }
        let Ok(class) = class_name(hwnd) else { return };
        let (title, _) = safe_title(hwnd, &class, pid);
        self.seen.insert(value);
        self.nodes.push(WindowNode {
            hwnd: value,
            parent,
            depth,
            title,
            class_name: class,
            pid,
            visible: unsafe { IsWindowVisible(hwnd) != 0 },
        });
    }
}

unsafe extern "system" fn collect_child(hwnd: HWND, parameter: LPARAM) -> BOOL {
    // SAFETY: See collect_top_level; this callback borrows Enumeration only
    // during the synchronous EnumChildWindows call, with no nested callbacks.
    let state = &mut *(parameter as *mut Enumeration);
    if state.exhausted() {
        return 0;
    }
    state.visits += 1;
    let parent = GetAncestor(hwnd, GA_PARENT);
    if parent.is_null() {
        return 1;
    }
    let mut ancestor = parent;
    for depth in 1..=ENUM_MAX_DEPTH {
        if ancestor == state.root {
            state.push(hwnd, handle_value(parent), depth);
            return 1;
        }
        let next = GetAncestor(ancestor, GA_PARENT);
        if next.is_null() || next == ancestor {
            return 1; // The window was moved/destroyed during the snapshot.
        }
        ancestor = next;
    }
    state.notice = Some("窗口层级超过 32 层，过深的控件已省略；可使用指针直接检查。");
    1
}

/// Snapshot desktop-app top-level windows and their descendants without
/// messaging foreign applications. Handles are snapshots, never owned by us.
pub fn enumerate_windows() -> Result<Vec<WindowNode>, String> {
    ENUM_NOTICE.with(|notice| *notice.borrow_mut() = None);
    let started = Instant::now();
    let mut tops = TopLevels {
        started,
        handles: Vec::new(),
        limited: false,
    };
    // SAFETY: The callback context lives until this synchronous call returns.
    let ok = unsafe {
        SetLastError(0);
        EnumWindows(
            Some(collect_top_level),
            &mut tops as *mut TopLevels as LPARAM,
        )
    };
    if ok == 0 && !tops.limited {
        return Err(last_error("枚举顶层窗口"));
    }
    let mut state = Enumeration {
        started,
        nodes: Vec::new(),
        seen: HashSet::new(),
        root: null_mut(),
        visits: 0,
        notice: tops
            .limited
            .then_some("顶层窗口枚举达到安全上限，当前显示部分结果。"),
    };
    for root in tops.handles {
        if state.exhausted() {
            break;
        }
        state.visits += 1;
        state.root = root;
        state.push(root, 0, 0);
        // A root can disappear between EnumWindows and this callback. The
        // API ignores stale handles and its return value is not meaningful.
        unsafe {
            EnumChildWindows(
                root,
                Some(collect_child),
                &mut state as *mut Enumeration as LPARAM,
            );
        }
    }
    ENUM_NOTICE.with(|notice| *notice.borrow_mut() = state.notice.map(str::to_owned));
    Ok(state.nodes)
}

struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: This wrapper exclusively owns a successful OpenProcess handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn process_basename(pid: u32) -> String {
    // Limited-query access only: no VM_READ, debug privileges or elevation.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return last_error("读取进程名称（可能无权限或进程已退出）");
    }
    let handle = ProcessHandle(handle);
    let mut path = vec![0u16; 32_768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return last_error("读取进程名称");
    }
    // Keep only the executable basename; do not expose profile/directory paths.
    let path = PathBuf::from(OsString::from_wide(&path[..length as usize]));
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "进程名称不可用".to_owned())
}

fn rect_value(rect: RECT) -> WindowRect {
    WindowRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

pub fn inspect_window(value: u64) -> Result<WindowInfo, String> {
    let hwnd = checked_handle(value)?;
    if unsafe { IsWindow(hwnd) } == 0 {
        return Err("窗口已关闭或句柄无效，请重新选取".to_owned());
    }
    let mut pid = 0;
    let tid = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if tid == 0 || pid == 0 {
        return Err(last_error("读取窗口所属进程"));
    }
    let class = class_name(hwnd)?;
    let (title, text_status) = safe_title(hwnd, &class, pid);
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut client = rect;
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err(last_error("读取窗口区域"));
    }
    if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
        return Err(last_error("读取客户区"));
    }
    // GWL_STYLE / GWL_EXSTYLE are 32-bit values even in a 64-bit program.
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    let parent = if style & WS_CHILD != 0 {
        handle_value(unsafe { GetAncestor(hwnd, GA_PARENT) })
    } else {
        0 // An owned top-level popup has an owner, not a child-tree parent.
    };
    let info = WindowInfo {
        hwnd: value,
        parent,
        pid,
        tid,
        title,
        class_name: class,
        process_name: process_basename(pid),
        rect: rect_value(rect),
        client_rect: rect_value(client), // GetClientRect uses local coordinates.
        visible: unsafe { IsWindowVisible(hwnd) != 0 },
        enabled: unsafe { IsWindowEnabled(hwnd) != 0 },
        minimized: unsafe { IsIconic(hwnd) != 0 },
        maximized: unsafe { IsZoomed(hwnd) != 0 },
        is_unicode: unsafe { IsWindowUnicode(hwnd) != 0 },
        style,
        ex_style,
        dpi: unsafe { GetDpiForWindow(hwnd) },
        text_status,
    };
    let mut final_pid = 0;
    let final_tid = unsafe { GetWindowThreadProcessId(hwnd, &mut final_pid) };
    if final_tid != tid || final_pid != pid || unsafe { IsWindow(hwnd) } == 0 {
        return Err("检查期间窗口已关闭或发生变化，请重新选取".to_owned());
    }
    Ok(info)
}

fn cursor_position() -> Result<POINT, String> {
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return Err(last_error("读取指针位置"));
    }
    Ok(point)
}

/// Descend from the root using client-coordinate hit tests. Unlike
/// WindowFromPoint alone, this can select Static and disabled child controls.
pub fn cursor_target() -> Result<(u64, i32, i32), String> {
    let point = cursor_position()?;
    let hit = unsafe { WindowFromPoint(point) };
    if hit.is_null() {
        return Err("指针下没有可检查的窗口；安全桌面可能不可访问".to_owned());
    }
    let root = unsafe { GetAncestor(hit, GA_ROOT) };
    let mut target = if root.is_null() { hit } else { root };
    // A fixed iteration cap and repeated-handle check protect against windows
    // being reparented or destroyed while the cursor moves.
    let mut seen = [0u64; ENUM_MAX_DEPTH + 1];
    seen[0] = handle_value(target);
    for depth in 0..ENUM_MAX_DEPTH {
        let mut client_point = point;
        // MapWindowPoints handles WS_EX_LAYOUTRTL mirroring correctly. Zero
        // displacement is successful, so distinguish it with GetLastError.
        let mapping_failed = unsafe {
            SetLastError(0);
            let displacement = MapWindowPoints(null_mut(), target, &mut client_point, 1);
            displacement == 0 && GetLastError() != 0
        };
        if mapping_failed {
            break;
        }
        let child = unsafe { ChildWindowFromPointEx(target, client_point, CWP_SKIPINVISIBLE) };
        if child.is_null() || child == target || unsafe { IsWindow(child) } == 0 {
            break;
        }
        let value = handle_value(child);
        if seen[..=depth].contains(&value) {
            break;
        }
        seen[depth + 1] = value;
        target = child;
    }
    if unsafe { IsWindow(target) } == 0 {
        return Err("指针下的窗口已关闭，请重新选取".to_owned());
    }
    Ok((handle_value(target), point.x, point.y))
}

struct ScreenDc(HDC);
impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: This DC comes from GetDC(NULL) on this thread, never DeleteDC.
        unsafe {
            ReleaseDC(null_mut(), self.0);
        }
    }
}

pub fn sample_color() -> Result<ColorSample, String> {
    let point = cursor_position()?;
    let dc = unsafe { GetDC(null_mut()) };
    if dc.is_null() {
        return Err("无法读取当前桌面的屏幕颜色".to_owned());
    }
    let dc = ScreenDc(dc);
    let color = unsafe { GetPixel(dc.0, point.x, point.y) };
    if color == CLR_INVALID {
        return Err("此坐标无法采样；受保护的画面或安全桌面可能不可读取".to_owned());
    }
    Ok(ColorSample {
        x: point.x,
        y: point.y,
        r: (color & 0xff) as u8,
        g: ((color >> 8) & 0xff) as u8,
        b: ((color >> 16) & 0xff) as u8,
    })
}

/// The GUI must call this only while an explicit picking mode is active.
/// All other keys are rejected; no keyboard history, hook, or logging exists.
pub fn key_down(vk: u32) -> bool {
    matches!(vk, 0x01 | 0x11 | 0x1B) && unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// Lightweight identity snapshot for locking a pointer target before queuing
/// background inspection. The caller must compare it with the later result;
/// Windows can destroy/reuse an HWND after any successful check.
pub fn window_identity(value: u64) -> Result<(u32, String), String> {
    let hwnd = checked_handle(value)?;
    if unsafe { IsWindow(hwnd) } == 0 {
        return Err("窗口已关闭或句柄无效，请重新选取".to_owned());
    }
    let mut pid = 0;
    // Metadata queries only: no caption/control messages or process opening.
    let tid = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if tid == 0 || pid == 0 {
        return Err(last_error("读取窗口身份"));
    }
    let class = class_name(hwnd)?;
    let mut final_pid = 0;
    let final_tid = unsafe { GetWindowThreadProcessId(hwnd, &mut final_pid) };
    if final_tid != tid || final_pid != pid || unsafe { IsWindow(hwnd) } == 0 {
        return Err("读取身份期间窗口已关闭或发生变化，请重新选取".to_owned());
    }
    Ok((pid, class))
}

pub fn is_window(value: u64) -> bool {
    checked_handle(value).is_ok_and(|hwnd| unsafe { IsWindow(hwnd) != 0 })
}

/// Best effort: a manifest or GUI framework may already have set awareness.
pub fn initialize_dpi() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

static SAVE_DIALOG_OPEN: AtomicBool = AtomicBool::new(false);
struct SaveDialogGuard;
impl Drop for SaveDialogGuard {
    fn drop(&mut self) {
        SAVE_DIALOG_OPEN.store(false, Ordering::Release);
    }
}

/// Called only after the user's Export action. Cancellation writes nothing.
/// The native dialog confirms overwriting; the selected file is UTF-8.
pub fn save_text_dialog(text: &str, json: bool) -> Result<Option<String>, String> {
    save_text_as(text, if json { "json" } else { "txt" })
}

/// Explicit export formats, with native cancellation and overwrite confirmation.
pub fn save_text_as(text: &str, format: &str) -> Result<Option<String>, String> {
    let (name, filter_text, extension_text) = match format {
        "json" => (
            "CoralSpyNext-window.json",
            "JSON 文件 (*.json)\0*.json\0所有文件 (*.*)\0*.*\0\0",
            "json\0",
        ),
        "html" | "htm" => (
            "CoralSpyNext-page.html",
            "网页文件 (*.html)\0*.html;*.htm\0所有文件 (*.*)\0*.*\0\0",
            "html\0",
        ),
        "rtf" => (
            "CoralSpyNext-text.rtf",
            "RTF 文件 (*.rtf)\0*.rtf\0所有文件 (*.*)\0*.*\0\0",
            "rtf\0",
        ),
        "txt" => (
            "CoralSpyNext-window.txt",
            "文本文件 (*.txt)\0*.txt\0所有文件 (*.*)\0*.*\0\0",
            "txt\0",
        ),
        _ => return Err("不支持的导出格式 / Unsupported export format".into()),
    };
    if SAVE_DIALOG_OPEN
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("已有一个保存对话框打开，请先完成或取消它".to_owned());
    }
    let _guard = SaveDialogGuard;
    let mut file = vec![0u16; 32_768];
    for (out, unit) in file.iter_mut().zip(name.encode_utf16()) {
        *out = unit;
    }
    let filter: Vec<u16> = filter_text.encode_utf16().collect();
    let extension: Vec<u16> = extension_text.encode_utf16().collect();
    let title: Vec<u16> = "导出 CoralSpyNext 检查结果\0".encode_utf16().collect();
    let foreground = unsafe { GetForegroundWindow() };
    let mut owner_pid = 0;
    if !foreground.is_null() {
        unsafe {
            GetWindowThreadProcessId(foreground, &mut owner_pid);
        }
    }
    // Never use another application's foreground HWND as our dialog owner.
    let owner = if owner_pid == unsafe { GetCurrentProcessId() } {
        foreground
    } else {
        null_mut()
    };
    // SAFETY: OPENFILENAMEW is a plain FFI structure whose zero fields mean
    // disabled optional features. All referenced buffers outlive the dialog.
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = owner;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.nFilterIndex = 1;
    dialog.lpstrFile = file.as_mut_ptr();
    dialog.nMaxFile = file.len() as u32;
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrDefExt = extension.as_ptr();
    dialog.Flags = OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST;
    if unsafe { GetSaveFileNameW(&mut dialog) } == 0 {
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 {
            Ok(None)
        } else {
            Err(format!("保存对话框失败（通用对话框错误 0x{error:08X}）"))
        };
    }
    let length = file
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| "保存路径缺少结束符".to_owned())?;
    if length == 0 {
        return Err("未选择保存路径".to_owned());
    }
    // Preserve even non-Unicode Windows paths for I/O; lossy conversion is
    // used only in the success message, not for choosing the output file.
    let path = PathBuf::from(OsString::from_wide(&file[..length]));
    let html = matches!(format, "html" | "htm");
    let exported = if html {
        // BOM takes precedence over legacy GBK/GB2312 meta declarations.
        format!(
            "\u{feff}<!-- saved from url=(0014)about:internet -->\n{}",
            text.trim_start_matches('\u{feff}')
        )
    } else {
        text.to_owned()
    };
    std::fs::write(&path, exported.as_bytes()).map_err(|error| format!("无法保存文件：{error}"))?;
    if html {
        // Best effort Internet-zone ADS; the HTML Mark-of-the-Web above remains
        // on file systems without alternate data streams. Do not execute HTML.
        let mut zone = path.as_os_str().to_owned();
        zone.push(":Zone.Identifier");
        let _ = std::fs::write(PathBuf::from(zone), b"[ZoneTransfer]\r\nZoneId=3\r\n");
    }
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_input_control_wrappers() {
        for class in [
            "Edit",
            "RichEdit20W",
            "RICHEDIT50W",
            "WindowsForms10.EDIT.app.1",
            "TEdit",
            "TMemo",
            "PasswordBox",
            "TextBox",
            "ComboBox",
            "Scintilla",
        ] {
            assert!(is_input_class(class), "{class}");
        }
        for class in ["Static", "Button", "#32770", "Chrome_WidgetWin_1"] {
            assert!(!is_input_class(class), "{class}");
        }
    }

    #[test]
    fn handles_round_trip_without_truncating() {
        for value in [1, 0x12345678, usize::MAX as u64] {
            assert_eq!(handle_value(checked_handle(value).unwrap()), value);
        }
        assert!(checked_handle(0).is_err());
    }

    #[test]
    fn rejects_unrelated_key_queries() {
        assert!(!key_down(0x41));
        assert!(!key_down(u32::MAX));
    }

    #[test]
    fn truncated_title_does_not_split_a_surrogate_pair() {
        assert_eq!(decode_title(&[0x41, 0xD83D], true), "A");
        assert_eq!(decode_title(&[0x41, 0xD83D, 0xDE00], true), "A😀");
        assert_eq!(decode_title(&[0xD83D], true), "");
        assert_eq!(decode_title(&[], true), "");
        assert_eq!(decode_title(&[0x41, 0xD83D], false), "A\u{FFFD}");
        assert_eq!(decode_title(&[0x4E2D, 0x6587], true), "中文");
    }
}
