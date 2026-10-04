//! Session-scoped Windows notification-area and RegisterHotKey integration.
//!
//! The service owns its window and all native resources on one worker thread.
//! It never installs hooks, reads typed input, or changes persistent OS settings.

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum DesktopEvent {
    ShowMain,
    ShowColor,
    StartCapture,
    ShowOptions,
    ShowAbout,
    Exit,
    Notice(String),
}

/// Win32 MOD_ALT=1, MOD_CONTROL=2, MOD_SHIFT=4, MOD_WIN=8 and a virtual-key code.
/// MOD_NOREPEAT is managed by the service, not saved in user configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyBinding {
    pub modifiers: u32,
    pub key: u32,
}

pub const DEFAULT_HOTKEYS: [HotkeyBinding; 3] = [
    HotkeyBinding {
        modifiers: 3,
        key: 0x57,
    },
    HotkeyBinding {
        modifiers: 3,
        key: 0x43,
    },
    HotkeyBinding {
        modifiers: 3,
        key: 0x53,
    },
];

pub const fn default_hotkey_bindings() -> [HotkeyBinding; 3] {
    DEFAULT_HOTKEYS
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        DEFAULT_HOTKEYS[0]
    }
}

impl HotkeyBinding {
    pub const fn defaults() -> [Self; 3] {
        DEFAULT_HOTKEYS
    }

    /// Reject malformed bindings and keys reserved exclusively for modifiers,
    /// mouse buttons, or the debugger. Windows decides whether a valid binding
    /// conflicts with another application's registration or a system shortcut.
    pub fn validate(self) -> Result<(), String> {
        if self.modifiers & !0x0f != 0 {
            return Err(crate::locale::label(
                "修饰键仅支持 Ctrl、Alt、Shift、Win",
                "Modifiers must be Ctrl, Alt, Shift, or Win",
            )
            .to_owned());
        }
        if self.modifiers == 0 {
            return Err(crate::locale::label("全局热键至少需要一个修饰键，避免占用普通输入", "Global hotkeys require at least one modifier to avoid interfering with normal typing").to_owned());
        }
        if !(0x08..=0xfe).contains(&self.key)
            || matches!(self.key, 0x10..=0x12 | 0x5b..=0x5c | 0xa0..=0xa5)
        {
            return Err(crate::locale::label(
                "请选择有效的非修饰键 Windows 虚拟键码",
                "Choose a valid Windows virtual-key code that is not a modifier",
            )
            .to_owned());
        }
        if self.key == 0x7b {
            return Err(crate::locale::label(
                "F12 由 Windows 调试器保留，请选择其他按键",
                "F12 is reserved for the Windows debugger; choose another key",
            )
            .to_owned());
        }
        Ok(())
    }
}

impl std::fmt::Display for HotkeyBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (mask, name) in [(2, "Ctrl"), (1, "Alt"), (4, "Shift"), (8, "Win")] {
            if self.modifiers & mask != 0 {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            0x30..=0x39 | 0x41..=0x5a => write!(f, "{}", char::from_u32(self.key).unwrap()),
            0x70..=0x87 => write!(f, "F{}", self.key - 0x6f),
            _ => write!(f, "VK 0x{:02X}", self.key),
        }
    }
}

#[cfg(any(windows, test))]
fn validate_bindings(bindings: &[HotkeyBinding; 3]) -> Result<(), String> {
    for (index, binding) in bindings.iter().enumerate() {
        binding.validate().map_err(|error| {
            crate::localized_format!("热键 {}：{error}", "Hotkey {}: {error}", index + 1)
        })?;
        if bindings[..index].contains(binding) {
            return Err(crate::localized_format!(
                "热键 {} 与前面的热键重复：{binding}",
                "Hotkey {} duplicates an earlier hotkey: {binding}",
                index + 1
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
pub use native::DesktopService;

#[cfg(windows)]
mod native {
    use super::{validate_bindings, DesktopEvent, HotkeyBinding, DEFAULT_HOTKEYS};
    use std::{
        cell::Cell,
        mem::{size_of, zeroed},
        ptr::{null, null_mut},
        sync::{
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
            mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
            Arc, Mutex,
        },
        thread::{self, JoinHandle},
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT},
            Shell::{
                Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP,
                NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETFOCUS, NIM_SETVERSION, NIN_SELECT,
                NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, NOTIFYICON_VERSION_4,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
                DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, EndMenu, GetCursorPos,
                GetMessageW, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindow,
                KillTimer, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
                SetForegroundWindow, SetMenuDefaultItem, SetTimer, SetWindowLongPtrW,
                ShowWindowAsync, TrackPopupMenu, TranslateMessage, UnregisterClassW, CREATESTRUCTW,
                GWLP_USERDATA, HICON, MF_SEPARATOR, MF_STRING, MSG, SW_RESTORE, SW_SHOW,
                TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_CONTEXTMENU,
                WM_DESTROY, WM_ENDSESSION, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_NCCREATE, WM_NCDESTROY,
                WM_NULL, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
            },
        },
    };

    const WM_WAKE: u32 = WM_APP + 41;
    const WM_TRAY: u32 = WM_APP + 42;
    const TRAY_ID: u32 = 1;
    const POLL_TIMER: usize = 1;
    const HOTKEY_IDS: [i32; 3] = [0x4251, 0x4252, 0x4253];
    const HOTKEY_NAMES: [(&str, &str); 3] = [
        ("主窗口", "Main window"),
        ("颜色查看器", "Color viewer"),
        ("开始捕获", "Start capture"),
    ];
    const START_TIMEOUT: Duration = Duration::from_secs(4);
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
    const STOP_TIMEOUT: Duration = Duration::from_secs(2);
    const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
    static NEXT_CLASS: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone)]
    struct GuiTarget {
        hwnd: usize,
        waker: Arc<dyn Fn() + Send + Sync>,
    }

    #[derive(Default)]
    struct Shared {
        hwnd: AtomicUsize,
        stopping: AtomicBool,
        alive: AtomicBool,
        // Written by the worker's binding command and snapshotted for fatal paths.
        // Clone the target and release the lock before any native call or callback.
        gui: Mutex<Option<GuiTarget>>,
    }

    enum CommandKind {
        Hotkeys(bool, [HotkeyBinding; 3]),
        Tray(bool),
        Language(bool),
        GuiWindow(u64, Arc<dyn Fn() + Send + Sync>),
    }

    struct Command {
        kind: CommandKind,
        cancelled: Arc<AtomicBool>,
        reply: SyncSender<Result<(), String>>,
    }

    enum NativeEvent {
        Hotkey(usize, isize),
        Tray(usize, isize),
        TaskbarCreated,
        Close,
    }

    /// Only immutable callback data is stored in GWLP_USERDATA. Mutable service
    /// state stays in Worker outside WndProc, which may be reentered by Win32.
    struct WindowContext {
        native: Sender<NativeEvent>,
        shared: Arc<Shared>,
        taskbar_created: u32,
    }

    /// Construct once and retain for the GUI's lifetime. Bind its native root
    /// with set_gui_window() before enabling minimize-to-tray. Event delivery
    /// restores that root natively before requesting an egui repaint: repaint
    /// alone cannot wake an invisible winit window on Windows.
    /// Starting the service does not itself enable the tray or global hotkeys.
    pub struct DesktopService {
        commands: Sender<Command>,
        events: Receiver<DesktopEvent>,
        shared: Arc<Shared>,
        finished: Receiver<()>,
        worker: Option<JoinHandle<()>>,
        disconnect_reported: Cell<bool>,
    }

    impl DesktopService {
        pub fn new() -> Result<Self, String> {
            let (commands, command_rx) = mpsc::channel();
            let (event_tx, events) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let (finished_tx, finished) = mpsc::sync_channel(1);
            let shared = Arc::new(Shared::default());
            let worker_shared = Arc::clone(&shared);
            let worker = thread::Builder::new()
                .name("coralspy-desktop".to_owned())
                .spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run_worker(command_rx, &event_tx, &ready_tx, Arc::clone(&worker_shared))
                    }));
                    worker_shared.alive.store(false, Ordering::Release);
                    match result {
                        Ok(Err(error)) => {
                            let _ = ready_tx.try_send(Err(error.clone()));
                            emit_event(
                                &worker_shared,
                                &event_tx,
                                DesktopEvent::Notice(error),
                                false,
                            );
                            emit_event(&worker_shared, &event_tx, DesktopEvent::ShowMain, false);
                        }
                        Err(_) => {
                            let message = crate::locale::label("桌面服务意外停止；托盘和全局热键已停止", "The desktop service stopped unexpectedly; the tray and global hotkeys have stopped").to_owned();
                            let _ = ready_tx.try_send(Err(message.clone()));
                            emit_event(
                                &worker_shared,
                                &event_tx,
                                DesktopEvent::Notice(message),
                                false,
                            );
                            emit_event(&worker_shared, &event_tx, DesktopEvent::ShowMain, false);
                        }
                        Ok(Ok(())) if !worker_shared.stopping.load(Ordering::Acquire) => {
                            emit_event(
                                &worker_shared,
                                &event_tx,
                                DesktopEvent::Notice(
                                    crate::locale::label("桌面服务意外退出，托盘和全局热键已停止", "The desktop service exited unexpectedly; the tray and global hotkeys have stopped").to_owned(),
                                ),
                                false,
                            );
                            emit_event(&worker_shared, &event_tx, DesktopEvent::ShowMain, false);
                        }
                        Ok(Ok(())) => {}
                    }
                    worker_shared.hwnd.store(0, Ordering::Release);
                    worker_shared.alive.store(false, Ordering::Release);
                    let _ = finished_tx.send(());
                })
                .map_err(|error| crate::localized_format!("无法启动桌面服务：{error}", "Could not start the desktop service: {error}"))?;
            match ready_rx.recv_timeout(START_TIMEOUT) {
                Ok(Ok(())) => Ok(Self {
                    commands,
                    events,
                    shared,
                    finished,
                    worker: Some(worker),
                    disconnect_reported: Cell::new(false),
                }),
                status => {
                    request_stop(&shared);
                    // Joining a thread stuck in the shell would freeze the GUI.
                    // Detach after the bound; it still owns all cleanup resources.
                    if finished.recv_timeout(STOP_TIMEOUT).is_ok() {
                        let _ = worker.join();
                    }
                    match status {
                        Ok(Err(error)) => Err(error),
                        Err(error) => Err(crate::localized_format!(
                            "桌面服务启动未完成：{error}",
                            "Desktop service startup did not complete: {error}"
                        )),
                        Ok(Ok(())) => unreachable!(),
                    }
                }
            }
        }

        /// Bind only this application's own GUI root. This command is processed
        /// by the native worker, which verifies process ownership before storing
        /// the HWND. Every later restore rechecks ownership to reject stale handles.
        pub fn set_gui_window(
            &self,
            hwnd: u64,
            waker: Arc<dyn Fn() + Send + Sync>,
        ) -> Result<(), String> {
            self.request(CommandKind::GuiWindow(hwnd, waker))
        }

        /// False after any fatal error or bounded command timeout. Callers can
        /// clear active indicators and show the main window when this is false.
        pub fn is_running(&self) -> bool {
            self.shared.alive.load(Ordering::Acquire)
                && !self.shared.stopping.load(Ordering::Acquire)
        }

        pub fn try_recv(&self) -> Option<DesktopEvent> {
            match self.events.try_recv() {
                Ok(event) => Some(event),
                Err(TryRecvError::Disconnected)
                    if !self.shared.stopping.load(Ordering::Acquire)
                        && !self.disconnect_reported.replace(true) =>
                {
                    wake_gui(&self.shared, true, false);
                    Some(DesktopEvent::Notice(
                        crate::locale::label("桌面服务连接已关闭，托盘和全局热键不可用；请重新启动程序", "The desktop service connection is closed; the tray and global hotkeys are unavailable. Restart the application").to_owned(),
                    ))
                }
                Err(_) => None,
            }
        }

        /// All-or-disabled: an invalid/conflicting configuration leaves none of
        /// the three hotkeys registered. Each native conflict also emits Notice.
        pub fn set_hotkeys(
            &self,
            enabled: bool,
            bindings: [HotkeyBinding; 3],
        ) -> Result<(), String> {
            self.request(CommandKind::Hotkeys(enabled, bindings))
        }

        pub fn set_tray(&self, enabled: bool) -> Result<(), String> {
            self.request(CommandKind::Tray(enabled))
        }

        /// Updates the native tray tooltip and menu to match the GUI language.
        pub fn set_language(&self, english: bool) -> Result<(), String> {
            self.request(CommandKind::Language(english))
        }

        fn request(&self, kind: CommandKind) -> Result<(), String> {
            if self.shared.stopping.load(Ordering::Acquire)
                || !self.shared.alive.load(Ordering::Acquire)
            {
                wake_gui(&self.shared, true, false);
                return Err(crate::locale::label(
                    "桌面服务已经停止",
                    "The desktop service has stopped",
                )
                .to_owned());
            }
            let (reply, result) = mpsc::sync_channel(1);
            let cancelled = Arc::new(AtomicBool::new(false));
            self.commands
                .send(Command {
                    kind,
                    cancelled: Arc::clone(&cancelled),
                    reply,
                })
                .map_err(|_| {
                    wake_gui(&self.shared, true, false);
                    crate::locale::label(
                        "桌面服务连接已关闭",
                        "The desktop service connection is closed",
                    )
                    .to_owned()
                })?;
            let hwnd = self.shared.hwnd.load(Ordering::Acquire) as HWND;
            // SAFETY: PostMessage does not dereference HWND or share Rust memory;
            // only the worker accesses window-owned state. No pointer is posted.
            if hwnd.is_null() || unsafe { PostMessageW(hwnd, WM_WAKE, 0, 0) } == 0 {
                cancelled.store(true, Ordering::Release);
                let error = last_error(crate::locale::label(
                    "唤醒桌面服务失败，服务正在停止",
                    "Could not wake the desktop service; it is stopping",
                ));
                request_stop(&self.shared);
                wake_gui(&self.shared, true, false);
                return Err(error);
            }
            match result.recv_timeout(COMMAND_TIMEOUT) {
                Ok(result) => result,
                Err(error) => {
                    cancelled.store(true, Ordering::Release);
                    request_stop(&self.shared);
                    wake_gui(&self.shared, true, false);
                    Err(crate::localized_format!(
                        "桌面服务响应未确认，已请求停止托盘与全局热键：{error}", "The desktop service response was not confirmed; shutdown of the tray and global hotkeys was requested: {error}"
                    ))
                }
            }
        }
    }

    impl Drop for DesktopService {
        fn drop(&mut self) {
            request_stop(&self.shared);
            if self.finished.recv_timeout(STOP_TIMEOUT).is_ok() {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
            }
            // A hung Explorer must never prevent the application from exiting.
            // Dropping the remaining JoinHandle detaches rather than blocks.
        }
    }

    fn owned_gui_window(address: usize) -> Option<HWND> {
        let hwnd = address as HWND;
        if hwnd.is_null() {
            return None;
        }
        let mut process = 0;
        // SAFETY: Win32 validates borrowed HWND values; no foreign memory is read.
        // A vanished/reused handle is accepted only if it still belongs to us.
        if unsafe { IsWindow(hwnd) } == 0
            || unsafe { GetWindowThreadProcessId(hwnd, &mut process) } == 0
            || process != std::process::id()
        {
            None
        } else {
            Some(hwnd)
        }
    }

    fn gui_target(shared: &Shared) -> Option<GuiTarget> {
        // The guard is gone before returning. No Win32 call or arbitrary callback
        // ever executes under this lock, including on panic/error shutdown paths.
        shared.gui.lock().ok().and_then(|target| target.clone())
    }

    fn restore_gui(target: &GuiTarget, foreground: bool) {
        if let Some(hwnd) = owned_gui_window(target.hwnd) {
            // SAFETY: This is the validated GUI HWND in our own process. The async
            // variant never waits for its thread, which may be awaiting our reply.
            let command = if unsafe { IsIconic(hwnd) } != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            };
            unsafe {
                ShowWindowAsync(hwnd, command);
            }
            if foreground && owned_gui_window(target.hwnd).is_some() {
                // Windows may deny focus stealing; visibility still gets restored.
                unsafe {
                    SetForegroundWindow(hwnd);
                }
            }
        }
    }

    fn invoke_waker(target: &GuiTarget) {
        // A callback should only post a repaint request. Keep an unexpected Rust
        // panic from preventing the worker's resource cleanup in unwind builds.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (target.waker)()));
    }

    fn wake_gui(shared: &Shared, reveal: bool, foreground: bool) {
        if let Some(target) = gui_target(shared) {
            if reveal {
                restore_gui(&target, foreground);
            }
            invoke_waker(&target);
        }
    }

    /// Central event order: reveal the native root, enqueue the semantic event,
    /// then wake egui. This works even when the root is hidden and has no WM_PAINT.
    fn emit_event(
        shared: &Shared,
        events: &Sender<DesktopEvent>,
        event: DesktopEvent,
        user_event: bool,
    ) {
        let target = gui_target(shared);
        if let Some(target) = &target {
            restore_gui(target, user_event);
        }
        let _ = events.send(event);
        if let Some(target) = &target {
            invoke_waker(target);
        }
    }

    fn request_stop(shared: &Shared) {
        shared.stopping.store(true, Ordering::Release);
        let hwnd = shared.hwnd.load(Ordering::Acquire) as HWND;
        if !hwnd.is_null() {
            // SAFETY: Asynchronous close; no Rust pointers cross thread boundaries.
            unsafe {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn last_error(operation: &str) -> String {
        // SAFETY: GetLastError is thread-local and takes no pointer arguments.
        let code = unsafe { GetLastError() };
        crate::localized_format!(
            "{operation}（Win32 {code}：{}）",
            "{operation} (Win32 {code}: {})",
            std::io::Error::from_raw_os_error(code as i32)
        )
    }

    struct NativeWindow {
        hwnd: HWND,
        module: HINSTANCE,
        class_name: Vec<u16>,
        // The allocation never moves; it outlives DestroyWindow/WM_NCDESTROY.
        context: Box<WindowContext>,
    }

    impl NativeWindow {
        fn new(native: Sender<NativeEvent>, shared: Arc<Shared>) -> Result<Self, String> {
            // SAFETY: NULL requests this process's existing module, borrowed only.
            let module = unsafe { GetModuleHandleW(null()) };
            if module.is_null() {
                return Err(last_error(crate::locale::label(
                    "读取程序模块失败",
                    "Could not read the application module",
                )));
            }
            let taskbar_name = wide("TaskbarCreated");
            // SAFETY: The UTF-16 buffer is terminated and valid for this call.
            let taskbar_created = unsafe { RegisterWindowMessageW(taskbar_name.as_ptr()) };
            if taskbar_created == 0 {
                return Err(last_error(crate::locale::label(
                    "注册任务栏恢复消息失败",
                    "Could not register the taskbar recovery message",
                )));
            }
            let context = Box::new(WindowContext {
                native,
                shared,
                taskbar_created,
            });
            let class_name = wide(&format!(
                "CoralSpyNext.Desktop.{}.{}",
                std::process::id(),
                NEXT_CLASS.fetch_add(1, Ordering::Relaxed)
            ));
            // SAFETY: Zero is valid for unused WNDCLASSW fields.
            let mut class: WNDCLASSW = unsafe { zeroed() };
            class.lpfnWndProc = Some(window_proc);
            class.hInstance = module;
            class.lpszClassName = class_name.as_ptr();
            // SAFETY: Class strings and callback remain valid for its lifetime.
            if unsafe { RegisterClassW(&class) } == 0 {
                return Err(last_error(crate::locale::label(
                    "注册桌面服务窗口类失败",
                    "Could not register the desktop service window class",
                )));
            }
            // A hidden top-level window (not HWND_MESSAGE) receives Explorer's
            // TaskbarCreated broadcast. It never becomes a visible taskbar item.
            // SAFETY: The boxed immutable context remains stable until destruction.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    class_name.as_ptr(),
                    class_name.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    null_mut(),
                    null_mut(),
                    module,
                    (&*context as *const WindowContext).cast(),
                )
            };
            if hwnd.is_null() {
                let error = last_error(crate::locale::label(
                    "创建桌面服务窗口失败",
                    "Could not create the desktop service window",
                ));
                // SAFETY: This class was registered here and has no live windows.
                unsafe {
                    UnregisterClassW(class_name.as_ptr(), module);
                }
                return Err(error);
            }
            let window = Self {
                hwnd,
                module,
                class_name,
                context,
            };
            // A modest timer also observes cancellation if posting a wake fails.
            // SAFETY: The timer belongs to our window and runs on this thread.
            if unsafe { SetTimer(hwnd, POLL_TIMER, 250, None) } == 0 {
                return Err(last_error(crate::locale::label(
                    "创建桌面服务退出计时器失败",
                    "Could not create the desktop service shutdown timer",
                )));
            }
            window
                .context
                .shared
                .hwnd
                .store(hwnd as usize, Ordering::Release);
            Ok(window)
        }
    }

    impl Drop for NativeWindow {
        fn drop(&mut self) {
            self.context.shared.hwnd.store(0, Ordering::Release);
            // SAFETY: All three resources belong to this worker thread. DestroyWindow
            // completes callbacks before the stable context allocation is released.
            unsafe {
                KillTimer(self.hwnd, POLL_TIMER);
                DestroyWindow(self.hwnd);
                UnregisterClassW(self.class_name.as_ptr(), self.module);
            }
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCCREATE {
            // SAFETY: Win32 provides CREATESTRUCTW for WM_NCCREATE. Its lpCreateParams
            // is our stable Box allocation, owned through NativeWindow destruction.
            let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            }
            return 1;
        }
        // SAFETY: The pointer is either unset or our immutable WindowContext.
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WindowContext;
        if message == WM_NCDESTROY {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
        }
        if !pointer.is_null() {
            // No mutable reference to worker state is ever taken in callbacks.
            let context = unsafe { &*pointer };
            let native = match message {
                WM_HOTKEY => Some(NativeEvent::Hotkey(wparam, lparam)),
                WM_TRAY => Some(NativeEvent::Tray(wparam, lparam)),
                WM_CLOSE => {
                    context.shared.stopping.store(true, Ordering::Release);
                    unsafe {
                        EndMenu();
                    }
                    Some(NativeEvent::Close)
                }
                WM_WAKE => {
                    unsafe {
                        EndMenu();
                    }
                    return 0;
                }
                WM_TIMER => {
                    if context.shared.stopping.load(Ordering::Acquire) {
                        unsafe {
                            EndMenu();
                        }
                    }
                    return 0;
                }
                WM_ENDSESSION if wparam != 0 => {
                    context.shared.stopping.store(true, Ordering::Release);
                    unsafe {
                        EndMenu();
                    }
                    Some(NativeEvent::Close)
                }
                WM_DESTROY => {
                    unsafe {
                        PostQuitMessage(0);
                    }
                    return 0;
                }
                value if value == context.taskbar_created => Some(NativeEvent::TaskbarCreated),
                _ => None,
            };
            if let Some(native) = native {
                let _ = context.native.send(native);
                return 0;
            }
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    struct OwnedIcon(HICON);
    impl Drop for OwnedIcon {
        fn drop(&mut self) {
            // SAFETY: Created by CreateIcon, never a borrowed/shared stock icon.
            unsafe {
                DestroyIcon(self.0);
            }
        }
    }

    fn create_coral_icon(module: HINSTANCE) -> Result<OwnedIcon, String> {
        let mut mask = [0xff_u8; 128]; // 32 rows, DWORD-aligned 1-bit AND mask.
        let mut pixels = [0_u8; 32 * 32 * 4]; // BGRA pixels, opaque colored circle.
        for y in 0..32_i32 {
            for x in 0..32_i32 {
                let dx = 2 * x - 31;
                let dy = 2 * y - 31;
                let distance = dx * dx + dy * dy;
                if distance <= 29 * 29 {
                    let index = (y as usize * 32 + x as usize) * 4;
                    mask[y as usize * 4 + x as usize / 8] &= !(0x80 >> (x as usize % 8));
                    let letter =
                        (12 * 12..=21 * 21).contains(&distance) && (dx < 6 || dy.abs() > 12);
                    let color = if letter {
                        [255, 255, 255, 255]
                    } else {
                        [90, 105, 245, 255]
                    };
                    pixels[index..index + 4].copy_from_slice(&color);
                }
            }
        }
        // SAFETY: Both buffers have the required dimensions/bit depth and remain
        // alive until CreateIcon copies them; the returned icon is independently owned.
        let icon = unsafe { CreateIcon(module, 32, 32, 1, 32, mask.as_ptr(), pixels.as_ptr()) };
        if icon.is_null() {
            Err(last_error(crate::locale::label(
                "创建托盘图标失败",
                "Could not create the tray icon",
            )))
        } else {
            Ok(OwnedIcon(icon))
        }
    }

    struct Worker {
        window: NativeWindow,
        events: Sender<DesktopEvent>,
        bindings: [HotkeyBinding; 3],
        registered: [bool; 3],
        tray_requested: bool,
        english: bool,
        tray_added: bool,
        icon: Option<OwnedIcon>,
    }

    impl Worker {
        fn emit(&self, event: DesktopEvent, user_event: bool) {
            emit_event(&self.window.context.shared, &self.events, event, user_event);
        }

        fn notice(&self, text: String) {
            self.emit(DesktopEvent::Notice(text), false);
        }

        fn set_gui_window(
            &self,
            address: u64,
            waker: Arc<dyn Fn() + Send + Sync>,
        ) -> Result<(), String> {
            let address = usize::try_from(address).map_err(|_| {
                crate::locale::label(
                    "GUI 窗口句柄超出当前指针宽度",
                    "The GUI window handle exceeds the current pointer width",
                )
                .to_owned()
            })?;
            if owned_gui_window(address).is_none() || address == self.window.hwnd as usize {
                return Err(crate::locale::label(
                    "只能绑定当前程序自己的 GUI 窗口",
                    "Only this application's own GUI window can be bound",
                )
                .to_owned());
            }
            let target = GuiTarget {
                hwnd: address,
                waker,
            };
            let previous = {
                let mut saved = self.window.context.shared.gui.lock().map_err(|_| {
                    crate::locale::label("GUI 唤醒状态不可用", "GUI wake state is unavailable")
                        .to_owned()
                })?;
                saved.replace(target.clone())
            };
            // Even destruction of an old callback's captured state is outside the lock.
            drop(previous);
            // Covers events queued during startup before the GUI target was known.
            // Do not unminimize a user-requested startup state just to bind it.
            invoke_waker(&target);
            Ok(())
        }

        fn unregister_hotkeys(&mut self) -> Result<(), String> {
            let mut errors = Vec::new();
            for (index, registered) in self.registered.iter_mut().enumerate() {
                if *registered {
                    // SAFETY: This thread owns both the window and registration.
                    if unsafe { UnregisterHotKey(self.window.hwnd, HOTKEY_IDS[index]) } != 0 {
                        *registered = false;
                    } else {
                        errors.push(last_error(&crate::localized_format!(
                            "注销{}热键失败",
                            "Could not unregister the {} hotkey",
                            crate::locale::label(HOTKEY_NAMES[index].0, HOTKEY_NAMES[index].1)
                        )));
                    }
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                // Do not keep running or claim the hotkeys are disabled when
                // unregistration failed. Window/thread destruction releases them.
                self.window
                    .context
                    .shared
                    .stopping
                    .store(true, Ordering::Release);
                let error = crate::localized_format!(
                    "{}；桌面服务正在停止",
                    "{}; the desktop service is stopping",
                    errors.join(crate::locale::label("；", "; "))
                );
                self.notice(error.clone());
                self.emit(DesktopEvent::ShowMain, false);
                Err(error)
            }
        }

        fn set_hotkeys(
            &mut self,
            enabled: bool,
            bindings: [HotkeyBinding; 3],
        ) -> Result<(), String> {
            if enabled
                && self.registered.iter().all(|registered| *registered)
                && self.bindings == bindings
            {
                return Ok(());
            }
            self.unregister_hotkeys()?;
            if !enabled {
                return Ok(());
            }
            if let Err(error) = validate_bindings(&bindings) {
                self.notice(error.clone());
                return Err(error);
            }
            self.bindings = bindings;
            let mut errors = Vec::new();
            for (index, binding) in bindings.iter().enumerate() {
                // SAFETY: This window was created on the current thread. Distinct
                // IDs and explicit unregister avoid accumulating old registrations.
                let success = unsafe {
                    RegisterHotKey(
                        self.window.hwnd,
                        HOTKEY_IDS[index],
                        binding.modifiers | MOD_NOREPEAT,
                        binding.key,
                    )
                } != 0;
                self.registered[index] = success;
                if !success {
                    let error = last_error(&crate::localized_format!(
                        "{}热键 {binding} 注册失败，可能被其他程序或 Windows 占用", "Could not register the {} hotkey {binding}; another application or Windows may be using it",
                        crate::locale::label(HOTKEY_NAMES[index].0, HOTKEY_NAMES[index].1)
                    ));
                    self.notice(error.clone());
                    errors.push(error);
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                if let Err(error) = self.unregister_hotkeys() {
                    errors.push(error);
                }
                Err(crate::localized_format!(
                    "全局热键未启用：{}",
                    "Global hotkeys are not enabled: {}",
                    errors.join(crate::locale::label("；", "; "))
                ))
            }
        }

        fn icon_data(&self) -> NOTIFYICONDATAW {
            // SAFETY: All unused notification fields are allowed to be zero.
            let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
            data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            data.hWnd = self.window.hwnd;
            data.uID = TRAY_ID;
            data
        }

        fn add_tray(&mut self) -> Result<(), String> {
            if self.tray_added {
                return Ok(());
            }
            if self.icon.is_none() {
                self.icon = Some(create_coral_icon(self.window.module)?);
            }
            let mut data = self.icon_data();
            data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
            data.uCallbackMessage = WM_TRAY;
            data.hIcon = self.icon.as_ref().map_or(null_mut(), |icon| icon.0);
            let tip = self.tooltip();
            data.szTip[..tip.len()].copy_from_slice(&tip);
            // Shell_NotifyIcon does not promise useful GetLastError diagnostics.
            // SAFETY: Every referenced buffer and icon is owned and live.
            if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
                return Err(crate::locale::label("无法添加通知区域图标；Windows 任务栏可能尚未就绪", "Could not add the notification area icon; the Windows taskbar may not be ready").to_owned());
            }
            self.tray_added = true;
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            if unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) } == 0 {
                self.remove_tray();
                return Err(crate::locale::label(
                    "无法启用通知区域图标的 Windows 11 交互模式",
                    "Could not enable Windows 11 interaction mode for the notification area icon",
                )
                .to_owned());
            }
            Ok(())
        }

        fn tooltip(&self) -> Vec<u16> {
            wide(if self.english {
                "CoralSpyNext · Window capture and color viewer"
            } else {
                "CoralSpyNext · 窗口捕获与颜色查看"
            })
        }

        fn set_language(&mut self, english: bool) -> Result<(), String> {
            self.english = english;
            if self.tray_added {
                let mut data = self.icon_data();
                data.uFlags = NIF_TIP | NIF_SHOWTIP;
                let tip = self.tooltip();
                data.szTip[..tip.len()].copy_from_slice(&tip);
                // SAFETY: This updates our live icon with a bounded UTF-16 tooltip.
                if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } == 0 {
                    return Err(crate::locale::label(
                        "无法更新托盘提示语言",
                        "Could not update tray tooltip",
                    )
                    .to_owned());
                }
            }
            Ok(())
        }

        fn remove_tray(&mut self) {
            if self.tray_added {
                let data = self.icon_data();
                // SAFETY: This identifies only the icon owned by this window.
                unsafe {
                    Shell_NotifyIconW(NIM_DELETE, &data);
                }
                self.tray_added = false;
            }
        }

        fn set_tray(&mut self, enabled: bool) -> Result<(), String> {
            self.tray_requested = enabled;
            if enabled {
                match self.add_tray() {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        self.tray_requested = false;
                        self.notice(error.clone());
                        Err(error)
                    }
                }
            } else {
                self.remove_tray();
                Ok(())
            }
        }

        fn handle_native(&mut self, event: NativeEvent) {
            match event {
                NativeEvent::Hotkey(id, data) => {
                    if let Some(index) = HOTKEY_IDS.iter().position(|key| *key as usize == id) {
                        let binding = self.bindings[index];
                        // Discard queued messages for an old/disabled binding.
                        if self.registered[index]
                            && (data as u32 & 0xffff) == binding.modifiers
                            && ((data as u32 >> 16) & 0xffff) == binding.key
                        {
                            let event = match index {
                                0 => DesktopEvent::ShowMain,
                                1 => DesktopEvent::ShowColor,
                                _ => DesktopEvent::StartCapture,
                            };
                            self.emit(event, true);
                        }
                    }
                }
                NativeEvent::Tray(_anchor, data) if self.tray_added => {
                    if (data as u32 >> 16) & 0xffff != TRAY_ID {
                        return;
                    }
                    match data as u32 & 0xffff {
                        NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONDBLCLK => {
                            self.emit(DesktopEvent::ShowMain, true);
                        }
                        WM_CONTEXTMENU => {
                            if let Err(error) = self.show_menu() {
                                self.notice(error);
                            }
                        }
                        _ => {}
                    }
                }
                NativeEvent::TaskbarCreated if self.tray_requested => {
                    self.tray_added = false;
                    if let Err(error) = self.add_tray() {
                        self.notice(crate::localized_format!(
                            "任务栏重启后恢复托盘图标失败：{error}；请在选项中重新启用托盘", "Could not restore the tray icon after the taskbar restarted: {error}; re-enable the tray in Options"
                        ));
                        // A hidden GUI must regain a usable route to its controls.
                        self.emit(DesktopEvent::ShowMain, false);
                    }
                }
                NativeEvent::Close => self
                    .window
                    .context
                    .shared
                    .stopping
                    .store(true, Ordering::Release),
                _ => {}
            }
        }

        fn show_menu(&self) -> Result<(), String> {
            struct Menu(windows_sys::Win32::UI::WindowsAndMessaging::HMENU);
            impl Drop for Menu {
                fn drop(&mut self) {
                    unsafe {
                        DestroyMenu(self.0);
                    }
                }
            }
            // SAFETY: An owned popup menu contains copied strings, no foreign callbacks.
            let menu = Menu(unsafe { CreatePopupMenu() });
            if menu.0.is_null() {
                return Err(last_error(crate::locale::label(
                    "创建托盘菜单失败",
                    "Could not create the tray menu",
                )));
            }
            let labels = if self.english {
                [
                    "Main window (&W)",
                    "Color viewer (&C)",
                    "Start capture (&S)",
                    "Options (&O)",
                    "About (&A)",
                    "",
                    "Exit (&X)",
                ]
            } else {
                [
                    "主窗口(&W)",
                    "颜色查看器(&C)",
                    "开始捕获(&S)",
                    "选项(&O)",
                    "关于(&A)",
                    "",
                    "退出(&X)",
                ]
            };
            for (id, title) in [1, 2, 3, 4, 5, 0, 6].into_iter().zip(labels) {
                let title = wide(title);
                let flags = if id == 0 { MF_SEPARATOR } else { MF_STRING };
                if unsafe { AppendMenuW(menu.0, flags, id, title.as_ptr()) } == 0 {
                    return Err(last_error(crate::locale::label(
                        "填充托盘菜单失败",
                        "Could not populate the tray menu",
                    )));
                }
            }
            let mut position = POINT { x: 0, y: 0 };
            // Prefer the icon's actual anchor, including keyboard invocation and
            // overflow flyouts. Fall back to cursor position if Explorer has moved it.
            let mut identifier: NOTIFYICONIDENTIFIER = unsafe { zeroed() };
            identifier.cbSize = size_of::<NOTIFYICONIDENTIFIER>() as u32;
            identifier.hWnd = self.window.hwnd;
            identifier.uID = TRAY_ID;
            let mut rect: RECT = unsafe { zeroed() };
            unsafe {
                if Shell_NotifyIconGetRect(&identifier, &mut rect) >= 0 {
                    position.x = rect.left;
                    position.y = rect.bottom;
                } else if GetCursorPos(&mut position) == 0 {
                    return Err(last_error(crate::locale::label(
                        "读取托盘菜单位置失败",
                        "Could not read the tray menu position",
                    )));
                }
                SetMenuDefaultItem(menu.0, 1, 0);
                SetForegroundWindow(self.window.hwnd);
            }
            // TrackPopupMenu runs a nested message loop. WndProc only queues immutable
            // messages, so there is no aliasing of this Worker during reentrancy.
            let command = unsafe {
                TrackPopupMenu(
                    menu.0,
                    TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                    position.x,
                    position.y,
                    0,
                    self.window.hwnd,
                    null(),
                )
            };
            // Required by the taskbar popup-menu pattern to dismiss reliably next time.
            unsafe {
                PostMessageW(self.window.hwnd, WM_NULL, 0, 0);
            }
            let event = match command {
                1 => Some(DesktopEvent::ShowMain),
                2 => Some(DesktopEvent::ShowColor),
                3 => Some(DesktopEvent::StartCapture),
                4 => Some(DesktopEvent::ShowOptions),
                5 => Some(DesktopEvent::ShowAbout),
                6 => Some(DesktopEvent::Exit),
                _ => None,
            };
            // Return keyboard focus before dispatching the selection; otherwise
            // Explorer could take focus back after we foregrounded the GUI root.
            let data = self.icon_data();
            unsafe {
                Shell_NotifyIconW(NIM_SETFOCUS, &data);
            }
            if let Some(event) = event {
                self.emit(event, true);
            }
            Ok(())
        }
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.unregister_hotkeys();
            self.remove_tray();
            // NativeWindow drops first after this method, then the owned icon.
            // Neither the shell nor the window retains the icon at that point.
        }
    }

    fn run_worker(
        commands: Receiver<Command>,
        events: &Sender<DesktopEvent>,
        ready: &SyncSender<Result<(), String>>,
        shared: Arc<Shared>,
    ) -> Result<(), String> {
        let (native_tx, native_rx) = mpsc::channel();
        let window = NativeWindow::new(native_tx, Arc::clone(&shared))?;
        let mut worker = Worker {
            window,
            events: events.clone(),
            bindings: DEFAULT_HOTKEYS,
            registered: [false; 3],
            tray_requested: false,
            english: false,
            tray_added: false,
            icon: None,
        };
        if shared.stopping.load(Ordering::Acquire) {
            return Ok(());
        }
        shared.alive.store(true, Ordering::Release);
        if ready.send(Ok(())).is_err() {
            return Ok(());
        }
        let mut message: MSG = unsafe { zeroed() };
        while !shared.stopping.load(Ordering::Acquire) {
            while let Ok(command) = commands.try_recv() {
                if command.cancelled.load(Ordering::Acquire) {
                    continue;
                }
                let result = match command.kind {
                    CommandKind::Hotkeys(enabled, bindings) => {
                        worker.set_hotkeys(enabled, bindings)
                    }
                    CommandKind::Tray(enabled) => worker.set_tray(enabled),
                    CommandKind::Language(english) => worker.set_language(english),
                    CommandKind::GuiWindow(hwnd, waker) => worker.set_gui_window(hwnd, waker),
                };
                let _ = command.reply.send(result);
                if shared.stopping.load(Ordering::Acquire) {
                    break;
                }
            }
            while let Ok(event) = native_rx.try_recv() {
                if shared.stopping.load(Ordering::Acquire) {
                    break;
                }
                worker.handle_native(event);
            }
            if shared.stopping.load(Ordering::Acquire) {
                break;
            }
            // SAFETY: MSG is writable; NULL collects this worker's messages only.
            let status = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
            if status == -1 {
                return Err(last_error(crate::locale::label(
                    "桌面服务消息循环失败",
                    "The desktop service message loop failed",
                )));
            }
            if status == 0 {
                break;
            }
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }
    #[cfg(test)]
    mod wake_tests {
        use super::*;

        #[test]
        fn event_is_queued_before_callback_and_callback_holds_no_gui_lock() {
            let shared = Arc::new(Shared::default());
            let (sender, receiver) = mpsc::channel();
            let receiver = Arc::new(Mutex::new(receiver));
            let observed = Arc::new(AtomicBool::new(false));
            let weak_shared = Arc::downgrade(&shared);
            let outcome = Arc::clone(&observed);
            let waker = Arc::new(move || {
                let lock_free = weak_shared
                    .upgrade()
                    .is_some_and(|shared| shared.gui.try_lock().is_ok());
                let queued = matches!(
                    receiver.lock().unwrap().try_recv(),
                    Ok(DesktopEvent::ShowMain)
                );
                outcome.store(lock_free && queued, Ordering::Release);
            });
            // An invalid native target still wakes the event consumer without
            // operating on a window. No interactive Windows desktop is required.
            *shared.gui.lock().unwrap() = Some(GuiTarget { hwnd: 0, waker });
            emit_event(&shared, &sender, DesktopEvent::ShowMain, true);
            assert!(observed.load(Ordering::Acquire));
        }

        #[test]
        fn reject_null_gui_window() {
            assert!(owned_gui_window(0).is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bindings_are_unique_and_valid() {
        assert_eq!(
            DEFAULT_HOTKEYS.map(|binding| binding.key),
            [0x57, 0x43, 0x53]
        );
        assert!(validate_bindings(&DEFAULT_HOTKEYS).is_ok());
        assert_eq!(DEFAULT_HOTKEYS[0].to_string(), "Ctrl+Alt+W");
    }

    #[test]
    fn reject_invalid_or_unsafe_bindings() {
        for modifiers in [0, 0x10, 0x4000, 0x4003, u32::MAX] {
            assert!(HotkeyBinding {
                modifiers,
                key: 0x57
            }
            .validate()
            .is_err());
        }
        for key in [
            0, 1, 7, 0x10, 0x11, 0x12, 0x5b, 0x5c, 0x7b, 0xa0, 0xa5, 0xff, 0x100,
        ] {
            assert!(HotkeyBinding { modifiers: 3, key }.validate().is_err());
        }
        for modifiers in 1..=15 {
            assert!(HotkeyBinding {
                modifiers,
                key: 0x70
            }
            .validate()
            .is_ok());
        }
        assert!(validate_bindings(&[DEFAULT_HOTKEYS[0]; 3]).is_err());
    }

    #[test]
    fn binding_configuration_round_trips() {
        let json = serde_json::to_string(&DEFAULT_HOTKEYS).unwrap();
        let loaded: [HotkeyBinding; 3] = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, DEFAULT_HOTKEYS);
        assert!(validate_bindings(&loaded).is_ok());
    }
}
