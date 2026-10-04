//! Classic native multi-window UI with one bounded background command worker.
use crate::hook_ui;
use coralspy_hook_client::{
    self as hook, Architecture, CaptureData, CaptureError, CaptureLimits, CaptureRequest,
    ErrorCode, Operation, Target, UiLanguage,
};
use coralspynext::{
    accessibility,
    config::{self, AppSettings},
    desktop::{DesktopEvent, DesktopService, HotkeyBinding},
    extras,
    legacy::{self, LegacyAction, LegacySnapshot},
    locale,
    model::{
        hwnd_text, ColorSample, ContentSnapshot, IconImage, IconSnapshot, MenuSnapshot, WindowInfo,
    },
    platform,
};
use eframe::egui::{
    self, Align, Color32, FontFamily, FontId, Layout, RichText, Sense, Stroke, Vec2,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
const ACCENT: Color32 = Color32::from_rgb(70, 110, 175);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
#[derive(Clone, Copy, PartialEq, Eq)]
enum PickKind {
    Window,
    Color,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum PickMode {
    DragRelease,
    CtrlLock,
    HotkeyToggle,
}
struct Picker {
    kind: PickKind,
    mode: PickMode,
    started: Instant,
    next_poll: Instant,
    armed: bool,
    target: Option<(u64, i32, i32)>,
    sample: Option<ColorSample>,
    error: Option<String>,
}
#[derive(Clone)]
enum DetailKind {
    Content,
    Menu,
    Legacy(Option<usize>),
    Action(Option<usize>, LegacyAction),
    Highlight {
        frame: Option<usize>,
        needle: String,
        text: [u8; 3],
        background: [u8; 3],
        bold: bool,
    },
}
enum DetailPayload {
    Content {
        content: Result<ContentSnapshot, String>,
        icons: Result<IconSnapshot, String>,
    },
    Menu(MenuSnapshot),
    Legacy(LegacySnapshot),
    Action(String, bool),
}
struct HookConfirmation {
    operation: Operation,
    target: Option<WindowInfo>,
    selection_epoch: u64,
    architecture: Architecture,
    consent: bool,
}
enum Job {
    Inspect {
        id: u64,
        hwnd: u64,
        expected: Option<(u32, String)>,
    },
    Export {
        id: u64,
        text: String,
        json: bool,
    },
    Details {
        id: u64,
        hwnd: u64,
        expected: (u32, String),
        kind: DetailKind,
    },
    SaveIcon {
        id: u64,
        icon: IconImage,
    },
    SaveNamed {
        id: u64,
        text: String,
        format: &'static str,
    },
    Hook {
        id: u64,
        request: CaptureRequest,
        expected: Option<WindowInfo>,
        cancel: Arc<AtomicBool>,
    },
    SaveRawRtf {
        id: u64,
        bytes: Vec<u8>,
    },
    Download {
        id: u64,
        url: String,
    },
}
impl Job {
    fn id(&self) -> u64 {
        match self {
            Self::Inspect { id, .. }
            | Self::Export { id, .. }
            | Self::Details { id, .. }
            | Self::SaveIcon { id, .. }
            | Self::SaveNamed { id, .. }
            | Self::Download { id, .. }
            | Self::Hook { id, .. }
            | Self::SaveRawRtf { id, .. } => *id,
        }
    }
}
enum Reply {
    Hook {
        id: u64,
        result: Result<Box<hook_ui::Snapshot>, CaptureError>,
    },
    Inspected {
        id: u64,
        hwnd: u64,
        result: Result<WindowInfo, String>,
    },
    Exported {
        id: u64,
        result: Result<Option<String>, String>,
    },
    Details {
        id: u64,
        hwnd: u64,
        result: Result<Box<DetailPayload>, String>,
    },
}
impl Reply {
    fn id(&self) -> u64 {
        match self {
            Self::Inspected { id, .. }
            | Self::Exported { id, .. }
            | Self::Details { id, .. }
            | Self::Hook { id, .. } => *id,
        }
    }
}
struct Worker {
    sender: SyncSender<Job>,
    receiver: Receiver<Reply>,
}
impl Worker {
    fn new(ctx: egui::Context) -> Self {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (answers, receiver) = mpsc::sync_channel::<Reply>(2);
        thread::Builder::new()
            .name("coralspynext-inspector".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let reply = match job {
                        Job::Inspect { id, hwnd, expected } => Reply::Inspected {
                            id,
                            hwnd,
                            result: platform::inspect_window(hwnd).and_then(|info| {
                                if expected.as_ref().is_some_and(|(pid, class)| {
                                    *pid != info.pid || *class != info.class_name
                                }) {
                                    Err(locale::label("目标已关闭或句柄被复用，请重新选取。", "The target closed or its handle was reused. Select it again.").into())
                                } else {
                                    Ok(info)
                                }
                            }),
                        },
                        Job::Export { id, text, json } => Reply::Exported {
                            id,
                            result: platform::save_text_dialog(&text, json),
                        },
                        Job::SaveIcon { id, icon } => Reply::Exported {
                            id,
                            result: extras::save_icon(&icon),
                        },
                        Job::SaveNamed { id, text, format } => Reply::Exported {
                            id,
                            result: platform::save_text_as(&text, format),
                        },
                        Job::SaveRawRtf { id, bytes } => Reply::Exported {
                            id, result: platform::save_bytes_as(&bytes, "rtf"),
                        },
                        Job::Hook { id, request, expected, cancel } => Reply::Hook {
                            id, result: capture_hook(request, expected, &cancel).map(Box::new),
                        },
                        Job::Download { id, url } => Reply::Exported {
                            id,
                            result: legacy::download_url(&url),
                        },
                        Job::Details {
                            id,
                            hwnd,
                            expected,
                            kind,
                        } => {
                            let result = (|| {
                                if platform::window_identity(hwnd)? != expected {
                                    return Err(locale::label("目标已关闭或句柄被复用，请重新选取。", "The target closed or its handle was reused. Select it again.").into());
                                }
                                let data = match kind {
                                    DetailKind::Content => DetailPayload::Content {
                                        content: accessibility::inspect(hwnd),
                                        icons: extras::inspect_icons(hwnd),
                                    },
                                    DetailKind::Menu => {
                                        DetailPayload::Menu(extras::inspect_menus(hwnd)?)
                                    }
                                    DetailKind::Legacy(frame) => {
                                        DetailPayload::Legacy(legacy::inspect(hwnd, frame)?)
                                    }
                                    DetailKind::Action(frame, action) => DetailPayload::Action(
                                        legacy::action(hwnd, frame, action)?,
                                        true,
                                    ),
                                    DetailKind::Highlight {
                                        frame,
                                        needle,
                                        text,
                                        background,
                                        bold,
                                    } => DetailPayload::Action(
                                        legacy::highlight(
                                            hwnd, frame, &needle, text, background, bold,
                                        )?,
                                        false,
                                    ),
                                };
                                if platform::window_identity(hwnd)? != expected {
                                    return Err(locale::label("读取期间窗口身份发生变化，已丢弃结果。", "The window identity changed during inspection; the result was discarded.").into());
                                }
                                Ok(Box::new(data))
                            })();
                            Reply::Details { id, hwnd, result }
                        }
                    };
                    if answers.send(reply).is_err() {
                        break;
                    }
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            })
            .unwrap_or_else(|_| panic!("{}", locale::label("无法启动检查线程", "Cannot start inspection worker")));
        Self { sender, receiver }
    }
}
pub struct CoralSpyApp {
    dark: bool,
    worker: Worker,
    worker_failed: bool,
    next_id: u64,
    in_flight: Option<u64>,
    inspect_id: u64,
    detail_id: u64,
    pending_inspect: Option<Job>,
    pending_export: Option<Job>,
    pending_detail: Option<Job>,
    inspecting: bool,
    exporting: bool,
    selected: Option<u64>,
    selected_identity: Option<(u32, String)>,
    info: Option<WindowInfo>,
    inspection_error: Option<String>,
    picker: Option<Picker>,
    color: Option<ColorSample>,
    color_history: Vec<ColorSample>,
    status: String,
    status_error: bool,
    details_open: bool,
    color_open: bool,
    options_open: bool,
    detail_tab: usize,
    topmost: bool,
    english: bool,
    color_edit: [u8; 3],
    selected_row: Option<usize>,
    content_rows: Vec<(usize, String, String, String)>,
    content_text: String,
    content_warning: String,
    tree_expanded: bool,
    reading_details: bool,
    mouse_position: Option<(i32, i32)>,
    content_snapshot: Option<ContentSnapshot>,
    menu_snapshot: Option<MenuSnapshot>,
    legacy_snapshot: Option<LegacySnapshot>,
    icons: Vec<IconImage>,
    icon_textures: Vec<egui::TextureHandle>,
    legacy_frame: Option<usize>,
    legacy_filter: usize,
    legacy_link: Option<usize>,
    highlight_needle: String,
    settings: AppSettings,
    settings_draft: AppSettings,
    desktop: Option<DesktopService>,
    hotkey_edit: usize,
    external_lock: bool,
    was_minimized: bool,
    hotkeys_tested: bool,
    menu_capture: bool,
    legacy_more: bool,
    pending_hook: Option<Job>,
    hook_id: u64,
    hook_running_id: Option<u64>,
    closing_after_hook: bool,
    hook_cancel: Option<Arc<AtomicBool>>,
    hook_confirmation: Option<HookConfirmation>,
    hook_result: Option<hook_ui::Snapshot>,
    hook_error: Option<CaptureError>,
}
impl CoralSpyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let (settings, warning) = config::load();
        locale::set_language(&settings.language);
        apply_theme(&cc.egui_ctx, settings.dark);
        let (desktop, desktop_warning) = match DesktopService::new() {
            Ok(service) => (Some(service), None),
            Err(error) => (None, Some(error)),
        };
        let mut app = Self {
            dark: settings.dark,
            worker: Worker::new(cc.egui_ctx.clone()),
            worker_failed: false,
            next_id: 0,
            in_flight: None,
            inspect_id: 0,
            detail_id: 0,
            pending_inspect: None,
            pending_export: None,
            pending_detail: None,
            inspecting: false,
            exporting: false,
            selected: None,
            selected_identity: None,
            info: None,
            inspection_error: None,
            picker: None,
            color: None,
            color_history: Vec::new(),
            status: locale::label(
                "拖动右侧准星，然后瞄准目标窗口或控件。",
                "Drag the crosshair on the right to a target window or control.",
            )
            .into(),
            status_error: false,
            details_open: false,
            color_open: false,
            options_open: false,
            detail_tab: 0,
            topmost: settings.always_on_top,
            english: settings.language == "en-US",
            color_edit: [0, 0, 0],
            selected_row: None,
            content_rows: Vec::new(),
            content_text: String::new(),
            content_warning: String::new(),
            tree_expanded: true,
            reading_details: false,
            mouse_position: None,
            content_snapshot: None,
            menu_snapshot: None,
            legacy_snapshot: None,
            icons: Vec::new(),
            icon_textures: Vec::new(),
            legacy_frame: None,
            legacy_filter: 0,
            legacy_link: None,
            highlight_needle: String::new(),
            settings_draft: settings.clone(),
            settings,
            desktop,
            hotkey_edit: 0,
            external_lock: false,
            was_minimized: false,
            hotkeys_tested: false,
            menu_capture: false,
            legacy_more: false,
            pending_hook: None,
            hook_id: 0,
            hook_running_id: None,
            closing_after_hook: false,
            hook_cancel: None,
            hook_confirmation: None,
            hook_result: None,
            hook_error: None,
        };
        let registration = app.desktop.as_ref().map(|service| {
            let handle = cc.window_handle().map_err(|e| {
                format!(
                    "{}: {e}",
                    locale::label("无法访问主窗口", "Cannot access main window")
                )
            })?;
            let hwnd = match handle.as_raw() {
                RawWindowHandle::Win32(window) => window.hwnd.get() as usize as u64,
                _ => {
                    return Err(locale::label(
                        "不支持此原生窗口句柄",
                        "Unsupported native window handle",
                    )
                    .into())
                }
            };
            let context = cc.egui_ctx.clone();
            service.set_gui_window(
                hwnd,
                std::sync::Arc::new(move || context.request_repaint_of(egui::ViewportId::ROOT)),
            )
        });
        if let Some(Err(error)) = registration {
            app.desktop = None;
            app.notify(error, true);
        }
        app.configure_desktop();
        if let Some(w) = warning.or(desktop_warning) {
            app.notify(w, true);
        }
        cc.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::WindowLevel(if app.topmost {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }));
        app
    }
    fn id(&mut self) -> u64 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }
    fn notify(&mut self, message: impl Into<String>, error: bool) {
        self.status = message.into();
        self.status_error = error;
    }
    fn select(&mut self, hwnd: u64) {
        self.inspect_target(hwnd, self.selected_identity.clone());
    }
    fn inspect_target(&mut self, hwnd: u64, expected: Option<(u32, String)>) {
        if self.worker_failed {
            return;
        }
        self.clear_hook();
        let id = self.id();
        self.inspect_id = id;
        self.detail_id = id;
        self.pending_detail = None;
        self.reading_details = false;
        self.selected = Some(hwnd);
        self.selected_identity = expected.clone();
        self.info = None;
        self.inspection_error = None;
        self.inspecting = true;
        self.content_snapshot = None;
        self.menu_snapshot = None;
        self.legacy_snapshot = None;
        self.content_rows.clear();
        self.content_text.clear();
        self.content_warning.clear();
        self.icons.clear();
        self.icon_textures.clear();
        self.legacy_frame = None;
        self.selected_row = None;
        self.pending_inspect = Some(Job::Inspect { id, hwnd, expected });
    }
    fn export(&mut self, info: &WindowInfo, json: bool) {
        self.cancel_pick();
        if self.exporting || self.worker_failed {
            return;
        }
        let text = if json {
            match serde_json::to_string_pretty(info) {
                Ok(v) => v,
                Err(e) => {
                    self.notify(e.to_string(), true);
                    return;
                }
            }
        } else {
            full_report(info, self.english)
        };
        let id = self.id();
        self.exporting = true;
        self.pending_export = Some(Job::Export { id, text, json });
    }
    fn fail_worker(&mut self) {
        self.clear_hook();
        self.worker_failed = true;
        self.hook_running_id = None;
        self.in_flight = None;
        self.inspecting = false;
        self.reading_details = false;
        self.exporting = false;
        self.pending_inspect = None;
        self.pending_export = None;
        self.pending_detail = None;
        self.notify(
            locale::label(
                "后台检查线程已停止，请重新打开程序。",
                "The inspection worker stopped. Please reopen the program.",
            ),
            true,
        );
    }
    fn service_worker(&mut self, ctx: &egui::Context) {
        loop {
            match self.worker.receiver.try_recv() {
                Ok(reply) => {
                    if self.in_flight == Some(reply.id()) {
                        self.in_flight = None;
                    }
                    if matches!(&reply, Reply::Hook { id, .. } if Some(*id) == self.hook_running_id)
                    {
                        self.hook_running_id = None;
                    }
                    match reply {
                        Reply::Hook { id, result } if id == self.hook_id => {
                            self.hook_cancel = None;
                            self.reading_details = false;
                            match result {
                                Ok(snapshot) => {
                                    self.content_warning.clear();
                                    self.hook_result = Some(*snapshot);
                                    self.notify(self.tr("本次 Hook 已结束，内容仅保留在内存中。", "This Hook capture finished. Content is held in memory only."), false);
                                    self.rebuild_rows();
                                }
                                Err(error) => {
                                    self.notify(
                                        hook_ui::error_text(&error, self.english),
                                        error.code != ErrorCode::Cancelled,
                                    );
                                    self.hook_error = Some(error);
                                }
                            }
                        }
                        Reply::Inspected { id, hwnd, result }
                            if id == self.inspect_id && self.selected == Some(hwnd) =>
                        {
                            self.inspecting = false;
                            match result {
                                Ok(info) => {
                                    self.selected_identity =
                                        Some((info.pid, info.class_name.clone()));
                                    self.info = Some(info);
                                    if self.menu_capture {
                                        self.details_open = true;
                                        self.detail_tab = 6;
                                        self.queue_detail(DetailKind::Menu);
                                    }
                                    self.notify(
                                        self.tr("已读取目标窗口。", "Target captured."),
                                        false,
                                    );
                                }
                                Err(error) => {
                                    self.inspection_error = Some(error.clone());
                                    self.notify(error, true);
                                }
                            }
                        }
                        Reply::Exported { result, .. } => {
                            self.exporting = false;
                            match result {
                                Ok(Some(path)) => self.notify(
                                    format!("{}: {path}", self.tr("已保存", "Saved")),
                                    false,
                                ),
                                Ok(None) => {
                                    self.notify(self.tr("已取消保存", "Save cancelled"), false)
                                }
                                Err(error) => self.notify(error, true),
                            }
                        }
                        Reply::Details { id, hwnd, result }
                            if id == self.detail_id && self.selected == Some(hwnd) =>
                        {
                            self.reading_details = false;
                            match result {
                                Ok(data) => {
                                    match *data {
                                        DetailPayload::Content { content, icons } => {
                                            self.content_warning.clear();
                                            match content {
                                                Ok(snapshot) => {
                                                    self.content_warning =
                                                        snapshot.warnings.join("\n");
                                                    self.content_snapshot = Some(snapshot);
                                                }
                                                Err(error) => self.content_warning = error,
                                            };
                                            self.icons.clear();
                                            self.icon_textures.clear();
                                            match icons {
                                                Ok(snapshot) => {
                                                    for (index, icon) in
                                                        snapshot.icons.into_iter().enumerate()
                                                    {
                                                        if icon.width > 0
                                                            && icon.height > 0
                                                            && icon.rgba.len()
                                                                == icon.width as usize
                                                                    * icon.height as usize
                                                                    * 4
                                                        {
                                                            let image=egui::ColorImage::from_rgba_unmultiplied([icon.width as usize,icon.height as usize],&icon.rgba);
                                                            self.icon_textures.push(
                                                                ctx.load_texture(
                                                                    format!(
                                                                        "target-icon-{id}-{index}"
                                                                    ),
                                                                    image,
                                                                    egui::TextureOptions::LINEAR,
                                                                ),
                                                            );
                                                            self.icons.push(icon);
                                                        }
                                                    }
                                                    if !snapshot.warnings.is_empty() {
                                                        self.content_warning.push_str(&format!(
                                                            "\n{}",
                                                            snapshot.warnings.join("\n")
                                                        ));
                                                    }
                                                }
                                                Err(error) => {
                                                    self.content_warning.push_str(&format!(
                                                        "\n{}: {error}",
                                                        locale::label("图标", "Icons")
                                                    ))
                                                }
                                            }
                                        }
                                        DetailPayload::Menu(snapshot) => {
                                            self.content_warning = snapshot.warnings.join("\n");
                                            self.menu_snapshot = Some(snapshot);
                                        }
                                        DetailPayload::Legacy(snapshot) => {
                                            self.content_warning = snapshot.warnings.join("\n");
                                            self.legacy_link = None;
                                            self.legacy_snapshot = Some(snapshot);
                                        }
                                        DetailPayload::Action(message, refresh) => {
                                            self.notify(message, false);
                                            if refresh {
                                                self.legacy_snapshot = None;
                                                self.legacy_link = None;
                                                self.queue_detail(DetailKind::Legacy(
                                                    self.legacy_frame,
                                                ));
                                            }
                                        }
                                    }
                                    self.rebuild_rows();
                                }
                                Err(error) => {
                                    self.content_warning = error.clone();
                                    self.notify(error, true);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.worker_failed {
                        self.fail_worker();
                    }
                    break;
                }
            }
        }
        if self.in_flight.is_none() && !self.worker_failed {
            let job = self
                .pending_export
                .take()
                .or_else(|| self.pending_inspect.take())
                .or_else(|| self.pending_hook.take())
                .or_else(|| self.pending_detail.take());
            if let Some(job) = job {
                let id = job.id();
                let hook_job = matches!(&job, Job::Hook { .. });
                match self.worker.sender.try_send(job) {
                    Ok(()) => {
                        self.in_flight = Some(id);
                        if hook_job {
                            self.hook_running_id = Some(id);
                        }
                    }
                    Err(TrySendError::Full(job)) => match job {
                        Job::Inspect { .. } => self.pending_inspect = Some(job),
                        Job::Details { .. } => self.pending_detail = Some(job),
                        Job::Hook { .. } => self.pending_hook = Some(job),
                        _ => self.pending_export = Some(job),
                    },
                    Err(TrySendError::Disconnected(_)) => self.fail_worker(),
                }
            }
        }
    }
    fn begin_pick(&mut self, kind: PickKind, mode: PickMode) {
        let now = Instant::now();
        self.picker = Some(Picker {
            kind,
            mode,
            started: now,
            next_poll: now,
            armed: false,
            target: None,
            sample: None,
            error: None,
        });
        self.external_lock = false;
        self.notify(
            self.tr(
                "移动鼠标后按 Ctrl 锁定，Esc 取消。",
                "Move the pointer, press Ctrl to lock; Esc cancels.",
            ),
            false,
        );
    }
    fn service_picker(&mut self, ctx: &egui::Context) {
        if self.picker.is_none() {
            return;
        }
        ctx.request_repaint_after(POLL_INTERVAL);
        let now = Instant::now();
        if self.picker.as_ref().is_some_and(|p| now < p.next_poll) {
            return;
        }
        if platform::key_down(0x1b) {
            self.cancel_pick();
            self.notify(self.tr("已取消拾取", "Capture cancelled"), false);
            return;
        }
        let mode = self.picker.as_ref().expect("picker active").mode;
        let ctrl = mode == PickMode::CtrlLock && platform::key_down(0x11);
        let mouse = mode == PickMode::DragRelease && platform::key_down(0x01);
        let picker = self.picker.as_mut().expect("picker active");
        picker.next_poll = now + POLL_INTERVAL;
        if mode != PickMode::CtrlLock {
            picker.armed = true;
        }
        if !picker.armed {
            if !ctrl && now.duration_since(picker.started) >= Duration::from_millis(180) {
                picker.armed = true;
            }
            return;
        }
        match picker.kind {
            PickKind::Window => match platform::cursor_target() {
                Ok(target) => {
                    picker.target = Some(target);
                    picker.error = None;
                    self.mouse_position = Some((target.1, target.2));
                }
                Err(error) => {
                    picker.target = None;
                    picker.error = Some(error);
                }
            },
            PickKind::Color => match platform::sample_color() {
                Ok(sample) => {
                    picker.sample = Some(sample);
                    picker.error = None;
                }
                Err(error) => {
                    picker.sample = None;
                    picker.error = Some(error);
                }
            },
        }
        if (mode == PickMode::DragRelease && !mouse)
            || (mode == PickMode::CtrlLock && ctrl)
            || (mode == PickMode::HotkeyToggle && self.external_lock)
        {
            self.lock_picker();
        }
    }
    fn lock_picker(&mut self) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        self.external_lock = false;
        match picker.kind {
            PickKind::Window => match picker.target {
                Some((hwnd, x, y)) => {
                    self.mouse_position = Some((x, y));
                    match platform::window_identity(hwnd) {
                        Ok(identity) => self.inspect_target(hwnd, Some(identity)),
                        Err(error) => self.notify(error, true),
                    }
                }
                None => self.notify(
                    picker.error.unwrap_or_else(|| {
                        locale::label(
                            "此位置没有可读取窗口。",
                            "There is no readable window at this position.",
                        )
                        .into()
                    }),
                    true,
                ),
            },
            PickKind::Color => match picker.sample {
                Some(sample) => {
                    self.color = Some(sample);
                    self.color_edit = [sample.r, sample.g, sample.b];
                    self.color_history
                        .retain(|c| (c.r, c.g, c.b) != (sample.r, sample.g, sample.b));
                    self.color_history.insert(0, sample);
                    self.color_history.truncate(24);
                    self.notify(
                        format!("{}  ({}, {})", sample.hex(), sample.x, sample.y),
                        false,
                    );
                }
                None => self.notify(
                    picker.error.unwrap_or_else(|| {
                        locale::label(
                            "此位置无法采集颜色。",
                            "The screen color at this position could not be sampled.",
                        )
                        .into()
                    }),
                    true,
                ),
            },
        }
    }

    fn configure_desktop(&mut self) {
        let bindings = self.settings.hotkeys.map(|h| HotkeyBinding {
            modifiers: h.modifiers,
            key: h.key,
        });
        if let Some(service) = &self.desktop {
            let hotkeys = service.set_hotkeys(self.settings.hotkeys_enabled, bindings);
            let tray = service.set_tray(self.settings.tray_enabled);
            let language = service.set_language(self.settings.language == "en-US");
            if let Err(error) = hotkeys {
                self.settings.hotkeys_enabled = false;
                self.notify(error, true);
            }
            if let Err(error) = language {
                self.notify(error, true);
            }
            if let Err(error) = tray {
                self.settings.tray_enabled = false;
                self.settings.minimize_to_tray = false;
                self.notify(error, true);
            }
        } else {
            self.settings.hotkeys_enabled = false;
            self.settings.tray_enabled = false;
            self.settings.minimize_to_tray = false;
        }
    }
    fn open_options(&mut self) {
        self.cancel_pick();
        if self.options_open {
            return;
        }
        self.hotkeys_tested = false;
        self.settings_draft = self.settings.clone();
        self.options_open = true;
    }
    fn test_hotkeys(&mut self, enable: bool) {
        let bindings = self.settings_draft.hotkeys.map(|h| HotkeyBinding {
            modifiers: h.modifiers,
            key: h.key,
        });
        let result = self
            .desktop
            .as_ref()
            .ok_or_else(|| {
                locale::label("桌面服务不可用", "Desktop service unavailable").to_string()
            })
            .and_then(|service| service.set_hotkeys(enable, bindings));
        self.hotkeys_tested = true;
        match result {
            Ok(()) => {
                self.settings_draft.hotkeys_enabled = enable;
                self.notify(
                    self.tr(
                        "热键已临时应用；确定保存，取消恢复。",
                        "Hotkeys applied temporarily. OK saves; Cancel restores.",
                    ),
                    false,
                );
            }
            Err(error) => {
                self.settings_draft.hotkeys_enabled = false;
                self.notify(error, true);
            }
        }
    }
    fn cancel_options(&mut self) {
        if self.hotkeys_tested {
            let bindings = self.settings.hotkeys.map(|h| HotkeyBinding {
                modifiers: h.modifiers,
                key: h.key,
            });
            if let Some(service) = &self.desktop {
                if let Err(error) = service.set_hotkeys(self.settings.hotkeys_enabled, bindings) {
                    self.settings.hotkeys_enabled = false;
                    self.notify(error, true);
                }
            }
        }
        self.hotkeys_tested = false;
        self.options_open = false;
    }
    fn apply_settings(&mut self, ctx: &egui::Context) -> bool {
        if let Err(error) = self.settings_draft.validate() {
            self.notify(error, true);
            return false;
        }
        // Persistence must succeed before committing the draft or clearing
        // the temporary-hotkey rollback marker used by Cancel.
        if let Err(error) = config::save(&self.settings_draft) {
            self.notify(error, true);
            return false;
        }
        let language_changed = self.settings.language != self.settings_draft.language;
        self.hotkeys_tested = false;
        self.settings = self.settings_draft.clone();
        locale::set_language(&self.settings.language);
        self.configure_desktop();
        self.settings_draft = self.settings.clone();
        self.english = self.settings.language == "en-US";
        self.dark = self.settings.dark;
        self.topmost = self.settings.always_on_top;
        apply_theme(ctx, self.dark);
        if language_changed {
            let selected_row = self.selected_row;
            self.rebuild_rows();
            self.selected_row = selected_row.filter(|index| *index < self.content_rows.len());
        }
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::WindowLevel(if self.topmost {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }),
        );
        true
    }
    fn save_preferences(&mut self) {
        if let Err(error) = config::save(&self.settings) {
            self.notify(error, true);
        }
    }
    fn show_main(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Minimized(false),
        );
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
    }
    fn service_desktop(&mut self, ctx: &egui::Context) {
        if self
            .desktop
            .as_ref()
            .is_some_and(|service| !service.is_running())
        {
            self.settings.hotkeys_enabled = false;
            self.settings.tray_enabled = false;
            self.settings.minimize_to_tray = false;
            self.desktop = None;
            self.show_main(ctx);
            self.notify(
                self.tr(
                    "托盘服务已停止，主窗口保持可见。",
                    "The tray service stopped; the main window remains visible.",
                ),
                true,
            );
        }
        let mut restored_this_frame = false;
        for _ in 0..32 {
            let event = self.desktop.as_ref().and_then(DesktopService::try_recv);
            let Some(event) = event else {
                break;
            };
            match event {
                DesktopEvent::ShowMain => {
                    self.show_main(ctx);
                    restored_this_frame = true;
                }
                DesktopEvent::ShowColor => {
                    self.show_main(ctx);
                    restored_this_frame = true;
                    self.color_open = true;
                }
                DesktopEvent::StartCapture => {
                    self.show_main(ctx);
                    restored_this_frame = true;
                    if self
                        .picker
                        .as_ref()
                        .is_some_and(|p| p.mode == PickMode::HotkeyToggle)
                    {
                        self.external_lock = true;
                    } else {
                        self.begin_pick(PickKind::Window, PickMode::HotkeyToggle);
                        self.notify(self.tr("移动鼠标后，再按一次准星热键锁定；Esc 取消。","Move the pointer, then press the crosshair hotkey again; Esc cancels."),false);
                    }
                }
                DesktopEvent::ShowOptions => {
                    self.show_main(ctx);
                    restored_this_frame = true;
                    self.open_options();
                }
                DesktopEvent::ShowAbout => {
                    self.show_main(ctx);
                    restored_this_frame = true;
                    self.open_details();
                    self.detail_tab = 7;
                }
                DesktopEvent::Exit => {
                    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close)
                }
                DesktopEvent::Notice(message) => self.notify(message, true),
            }
        }
        if self.desktop.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if !restored_this_frame
            && minimized
            && !self.was_minimized
            && self.settings.tray_enabled
            && self
                .desktop
                .as_ref()
                .is_some_and(DesktopService::is_running)
            && self.settings.minimize_to_tray
        {
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Visible(false),
            );
        }
        self.was_minimized = minimized;
    }
    fn queue_detail(&mut self, kind: DetailKind) {
        self.cancel_pick();
        self.clear_hook();
        let Some(hwnd) = self.selected else {
            self.content_warning = self
                .tr(
                    "请先拖动准星选择目标窗口。",
                    "Drag the crosshair to select a target first.",
                )
                .into();
            return;
        };
        let Some(expected) = self.selected_identity.clone() else {
            self.content_warning = self
                .tr(
                    "正在确认窗口身份，请稍后重试。",
                    "Wait for the window identity check, then retry.",
                )
                .into();
            return;
        };
        if self.worker_failed {
            return;
        }
        if matches!(&kind, DetailKind::Action(..) | DetailKind::Highlight { .. }) {
            // A command can partially change the remote page even when its
            // response times out. Never keep that old source exportable.
            self.legacy_snapshot = None;
            self.legacy_link = None;
        }
        let id = self.id();
        self.detail_id = id;
        self.reading_details = true;
        self.content_warning = self
            .tr(
                "正在读取，受保护控件会保留保护标记…",
                "Reading; protected controls remain redacted…",
            )
            .into();
        self.pending_detail = Some(Job::Details {
            id,
            hwnd,
            expected,
            kind,
        });
    }
    fn rebuild_rows(&mut self) {
        self.content_rows.clear();
        self.selected_row = None;
        if let Some(snapshot) = self
            .hook_result
            .as_ref()
            .filter(|snapshot| snapshot.applies_to(self.detail_tab))
        {
            self.content_rows = snapshot.rows(self.english);
            self.content_text = snapshot.report(self.english);
            return;
        }
        if self.detail_tab == 6 {
            if let Some(menu) = &self.menu_snapshot {
                for entry in &menu.entries {
                    self.content_rows.push((
                        entry.depth,
                        if entry.separator {
                            "────────".into()
                        } else {
                            entry.label.clone()
                        },
                        format!("ID {}", entry.id),
                        format!(
                            "{}{}{}",
                            if entry.enabled {
                                ""
                            } else {
                                locale::label("禁用 ", "Disabled ")
                            },
                            if entry.checked { "✓ " } else { "" },
                            if entry.submenu {
                                locale::label("子菜单", "Submenu")
                            } else {
                                ""
                            }
                        ),
                    ));
                }
            }
        } else if let Some(snapshot) = &self.content_snapshot {
            self.content_rows = coralspynext::content_view::logical_rows(snapshot, self.detail_tab);
        }
        self.content_text = if self.detail_tab == 6 {
            let mut report = self
                .content_rows
                .iter()
                .map(|(depth, name, role, value)| {
                    format!(
                        "{}{}\t{}\t{}",
                        "  ".repeat((*depth).min(32)),
                        name,
                        role,
                        value
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(snapshot) = &self.menu_snapshot {
                for warning in &snapshot.warnings {
                    report.push_str(&format!(
                        "\n{}: {warning}",
                        locale::label("警告", "WARNING")
                    ));
                }
            }
            report
        } else if let Some(snapshot) = &self.content_snapshot {
            if self.detail_tab == 5 {
                coralspynext::content_view::rich_text(snapshot)
            } else {
                coralspynext::content_view::report(snapshot, self.detail_tab)
            }
        } else {
            String::new()
        };
    }
    fn icon_contents(&mut self, ui: &mut egui::Ui) {
        ui.label(self.tr("图标", "Icons"));
        let slots = classic_icon_slots(&self.icons);
        let sources = [
            self.tr("窗口大图标", "Window large icon"),
            self.tr("窗口小图标", "Window small icon"),
            self.tr("窗口类大图标", "Class large icon"),
            self.tr("窗口类小图标", "Class small icon"),
        ];
        let sizes = [32.0, 16.0, 32.0, 16.0];
        let mut save = None;
        // Match the original four fixed 36×36 panels, including the slightly
        // wider gap between the window and class pairs. No missing source is
        // replaced with a fabricated or unrelated executable icon.
        let (area, _) = ui.allocate_exact_size(Vec2::new(164.0, 36.0), Sense::hover());
        for (slot, offset) in [0.0, 40.0, 88.0, 128.0].into_iter().enumerate() {
            let panel =
                egui::Rect::from_min_size(area.min + Vec2::new(offset, 0.0), Vec2::splat(36.0));
            ui.painter()
                .rect_filled(panel, 0.0, ui.visuals().panel_fill);
            ui.painter().rect_stroke(
                panel,
                0.0,
                Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
                egui::StrokeKind::Inside,
            );
            if let Some(index) = slots[slot].filter(|index| *index < self.icon_textures.len()) {
                let texture = &self.icon_textures[index];
                let picture =
                    egui::Rect::from_center_size(panel.center(), Vec2::splat(sizes[slot]));
                ui.painter().image(
                    texture.id(),
                    picture,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                let response = ui
                    .interact(panel, ui.id().with(("classic_icon", slot)), Sense::click())
                    .on_hover_text(format!(
                        "{} ({}×{})\n{}\n{}",
                        sources[slot],
                        sizes[slot] as u32,
                        sizes[slot] as u32,
                        locale::icon_source_label(&self.icons[index].kind),
                        self.tr(
                            "单击保存实际图标为 ICO",
                            "Click to save the actual icon as ICO"
                        )
                    ));
                if response.clicked() && !self.exporting {
                    save = Some(index);
                }
            } else {
                ui.painter().text(
                    panel.center(),
                    egui::Align2::CENTER_CENTER,
                    "—",
                    FontId::proportional(12.0),
                    ui.visuals().weak_text_color(),
                );
                ui.interact(panel, ui.id().with(("empty_icon", slot)), Sense::hover())
                    .on_hover_text(format!(
                        "{} ({}×{})\n{}",
                        sources[slot],
                        sizes[slot] as u32,
                        sizes[slot] as u32,
                        self.tr(
                            "此来源未提供可读取图标",
                            "No readable icon from this source"
                        )
                    ));
            }
        }
        ui.small(self.tr(
            "顺序：窗口大/小；窗口类大/小。空槽表示该来源不可用。",
            "Order: window large/small; class large/small. Empty slots mean unavailable sources.",
        ));
        let extra: Vec<usize> = (0..self.icons.len())
            .filter(|index| !slots.contains(&Some(*index)))
            .collect();
        if !extra.is_empty() {
            egui::CollapsingHeader::new(self.tr("更多图标", "More icons")).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for index in extra {
                        let icon = &self.icons[index];
                        let Some(texture) = self.icon_textures.get(index) else {
                            continue;
                        };
                        ui.vertical(|ui| {
                            if ui
                                .add(
                                    egui::Image::new(texture)
                                        .fit_to_exact_size(Vec2::splat(36.0))
                                        .sense(Sense::click()),
                                )
                                .on_hover_text(self.tr("单击保存 ICO", "Click to save ICO"))
                                .clicked()
                                && !self.exporting
                            {
                                save = Some(index);
                            }
                            ui.small(locale::icon_source_label(&icon.kind));
                        });
                    }
                });
            });
        }
        if let Some(index) = save {
            self.cancel_pick();
            let id = self.id();
            self.exporting = true;
            self.pending_export = Some(Job::SaveIcon {
                id,
                icon: self.icons[index].clone(),
            });
        }
    }

    fn tr<'a>(&self, zh: &'a str, en: &'a str) -> &'a str {
        if self.english {
            en
        } else {
            zh
        }
    }
    fn cancel_pick(&mut self) {
        self.picker = None;
        self.external_lock = false;
    }
    fn open_details(&mut self) {
        self.cancel_pick();
        self.details_open = true;
    }
    fn refresh_target(&mut self) {
        self.cancel_pick();
        if let Some(hwnd) = self.selected {
            self.select(hwnd);
        } else {
            self.notify(
                self.tr(
                    "请先拖动准星选取目标。",
                    "Drag the crosshair to select a target first.",
                ),
                false,
            );
        }
    }
    fn save_named(&mut self, text: String, format: &'static str) {
        self.cancel_pick();
        if self.exporting || self.worker_failed {
            return;
        }
        let id = self.id();
        self.exporting = true;
        self.pending_export = Some(Job::SaveNamed { id, text, format });
    }
    fn save_string(&mut self, text: String) {
        self.cancel_pick();
        if self.exporting || self.worker_failed {
            return;
        }
        let id = self.id();
        self.exporting = true;
        self.pending_export = Some(Job::Export {
            id,
            text,
            json: false,
        });
    }
    fn main_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if tool(
                ui,
                0,
                self.tr("显示颜色拾取器", "Show color picker"),
                self.color_open,
            ) {
                self.cancel_pick();
                self.color_open = true;
            }
            ui.separator();
            if tool(ui, 1, self.tr("刷新信息", "Refresh information"), false) {
                self.refresh_target();
            }
            if tool(
                ui,
                2,
                self.tr("查看详情", "View details"),
                self.details_open,
            ) {
                self.open_details();
            }
            if tool(
                ui,
                3,
                self.tr("恢复鼠标指针 / 取消拾取", "Restore cursor / cancel capture"),
                false,
            ) {
                self.cancel_pick();
            }
            if tool(
                ui,
                4,
                self.tr("捕捉菜单", "Capture menu"),
                self.menu_capture,
            ) {
                self.menu_capture = !self.menu_capture;
                if self.menu_capture {
                    self.open_details();
                    self.detail_tab = 6;
                    self.rebuild_rows();
                    if self.selected.is_some() {
                        self.request_content();
                    } else {
                        self.notify(
                            self.tr(
                                "菜单捕捉已开启，请拖动准星到目标菜单。",
                                "Menu capture is on. Drag the crosshair to a menu.",
                            ),
                            false,
                        );
                    }
                }
            }
            ui.separator();
            if tool(
                ui,
                5,
                self.tr("复制标题内容到剪贴板", "Copy title to clipboard"),
                false,
            ) {
                if let Some(info) = &self.info {
                    ui.ctx().copy_text(info.title.clone());
                }
            }
            if tool(
                ui,
                6,
                self.tr("保存标题内容到硬盘", "Save title to disk"),
                false,
            ) {
                if let Some(info) = &self.info {
                    self.save_string(info.title.clone());
                }
            }
            ui.separator();
            if tool(ui, 7, self.tr("选项", "Options"), self.options_open) {
                self.cancel_pick();
                self.open_options();
            }
            if tool(
                ui,
                8,
                self.tr("激活 / 取消热键", "Enable / disable hotkeys"),
                self.settings.hotkeys_enabled,
            ) {
                self.settings.hotkeys_enabled = !self.settings.hotkeys_enabled;
                self.configure_desktop();
                self.save_preferences();
            }
            if tool(
                ui,
                9,
                self.tr("最小化到系统托盘", "Minimize to system tray"),
                false,
            ) {
                if self.settings.tray_enabled
                    && self
                        .desktop
                        .as_ref()
                        .is_some_and(DesktopService::is_running)
                {
                    self.cancel_pick();
                    ui.ctx().send_viewport_cmd_to(
                        egui::ViewportId::ROOT,
                        egui::ViewportCommand::Visible(false),
                    );
                } else {
                    self.notify(
                        self.tr(
                            "托盘未启用，请在选项中启用。",
                            "Enable the tray in Options first.",
                        ),
                        true,
                    );
                    self.open_options();
                }
            }
            ui.separator();
            if tool(ui, 10, self.tr("关于这玩意儿", "About this program"), false) {
                self.open_details();
                self.detail_tab = 7;
            }
            ui.separator();
            if tool(
                ui,
                11,
                self.tr("始终最前端显示", "Always on top"),
                self.topmost,
            ) {
                self.topmost = !self.topmost;
                self.settings.always_on_top = self.topmost;
                self.save_preferences();
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::WindowLevel(if self.topmost {
                        egui::WindowLevel::AlwaysOnTop
                    } else {
                        egui::WindowLevel::Normal
                    }));
            }
        });
    }
    fn crosshair(&mut self, ui: &mut egui::Ui, kind: PickKind) {
        let compact = kind == PickKind::Color;
        let (rect, response) = ui.allocate_exact_size(
            Vec2::splat(if compact { 48.0 } else { 58.0 }),
            Sense::click_and_drag(),
        );
        let color = if self.picker.as_ref().is_some_and(|p| p.kind == kind) {
            Color32::from_rgb(175, 36, 36)
        } else {
            ui.visuals().text_color()
        };
        ui.painter()
            .rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
        let c = rect.center();
        ui.painter()
            .circle_stroke(c, 15.0, Stroke::new(1.5_f32, color));
        ui.painter()
            .circle_stroke(c, 5.0, Stroke::new(1.0_f32, color));
        ui.painter().line_segment(
            [c - Vec2::new(22.0, 0.0), c + Vec2::new(22.0, 0.0)],
            Stroke::new(1.0_f32, color),
        );
        ui.painter().line_segment(
            [c - Vec2::new(0.0, 22.0), c + Vec2::new(0.0, 22.0)],
            Stroke::new(1.0_f32, color),
        );
        if response.drag_started_by(egui::PointerButton::Primary) && self.picker.is_none() {
            self.begin_pick(kind, PickMode::DragRelease);
            self.notify(
                self.tr(
                    "按住左键拖动准星，松开选取；Esc 取消。",
                    "Drag while holding the left button; release to capture. Esc cancels.",
                ),
                false,
            );
        }
        let response = response.on_hover_text(self.tr(
            "拖动准星，然后瞄准目标窗口或控件\n按住左键拖出本窗口，松开选取；Esc 取消。",
            "Drag the crosshair to the target; release to capture. Esc cancels.",
        ));
        response.context_menu(|ui| {
            if ui.button(self.tr("Ctrl 拾取", "Ctrl capture")).clicked() {
                self.begin_pick(kind, PickMode::CtrlLock);
                ui.close_menu();
            }
        });
        if !compact {
            ui.label(self.tr("拖动准星", "Drag target"));
            if ui
                .small_button(self.tr("Ctrl 拾取", "Ctrl capture"))
                .on_hover_text(self.tr(
                    "移动鼠标后按 Ctrl 锁定，Esc 取消。",
                    "Move the pointer, press Ctrl to lock; Esc cancels.",
                ))
                .clicked()
            {
                self.begin_pick(kind, PickMode::CtrlLock);
            }
        }
    }
    fn main_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            let left_width = (ui.available_width() - 88.0).max(270.0);
            ui.allocate_ui_with_layout(
                Vec2::new(left_width, 205.0),
                Layout::top_down(Align::LEFT),
                |ui| {
                    ui.set_width(left_width);
                    ui.group(|ui| {
                        ui.set_width(left_width - 14.0);
                        ui.label(self.tr("信息", "Information"));
                        let live = self.picker.as_ref().and_then(|p| p.target);
                        let pos = live
                            .map(|(_, x, y)| (x, y))
                            .or(self.mouse_position)
                            .map(|(x, y)| format!("{x}, {y}"))
                            .unwrap_or_default();
                        let hwnd = live
                            .map(|t| t.0)
                            .or(self.selected)
                            .map(hwnd_text)
                            .unwrap_or_default();
                        let class = self
                            .info
                            .as_ref()
                            .filter(|info| live.is_none_or(|t| t.0 == info.hwnd))
                            .map(|i| i.class_name.clone())
                            .unwrap_or_default();
                        let title = self
                            .info
                            .as_ref()
                            .filter(|info| live.is_none_or(|t| t.0 == info.hwnd))
                            .map(|i| i.title.clone())
                            .unwrap_or_default();
                        egui::Grid::new("main_properties")
                            .num_columns(2)
                            .spacing([7.0, 7.0])
                            .show(ui, |ui| {
                                readonly_row(
                                    ui,
                                    self.tr("鼠标:", "Mouse:"),
                                    &pos,
                                    false,
                                    left_width - 76.0,
                                );
                                readonly_row(
                                    ui,
                                    self.tr("句柄:", "Handle:"),
                                    &hwnd,
                                    false,
                                    left_width - 76.0,
                                );
                                readonly_row(
                                    ui,
                                    self.tr("类型:", "Class:"),
                                    &class,
                                    false,
                                    left_width - 76.0,
                                );
                                readonly_row(
                                    ui,
                                    self.tr("标题:", "Title:"),
                                    &title,
                                    true,
                                    left_width - 76.0,
                                );
                                readonly_row(
                                    ui,
                                    self.tr("密码:", "Password:"),
                                    self.tr("未读取 · 保护隐私", "Not read · private"),
                                    false,
                                    left_width - 76.0,
                                );
                            });
                    });
                },
            );
            ui.vertical(|ui| {
                ui.add_space(13.0);
                self.crosshair(ui, PickKind::Window);
            });
        });
    }
    fn clear_hook(&mut self) {
        if let Some(cancel) = self.hook_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.pending_hook = None;
        self.hook_confirmation = None;
        if self.hook_result.take().is_some() {
            self.content_rows.clear();
            self.content_text.clear();
            self.selected_row = None;
        }
        self.hook_error = None;
        self.hook_id = self.id();
    }
    fn confirm_hook(&mut self, operation: Operation) {
        self.cancel_pick();
        if self.worker_failed
            || self.in_flight.is_some()
            || self.reading_details
            || self.inspecting
            || self.exporting
        {
            return;
        }
        let target = if operation == Operation::MenuDesktopOnce {
            None
        } else {
            let Some(info) = self.info.as_ref().filter(|info| {
                Some(info.hwnd) == self.selected
                    && self.selected_identity.as_ref() == Some(&(info.pid, info.class_name.clone()))
            }) else {
                return;
            };
            Some(info.clone())
        };
        self.hook_confirmation = Some(HookConfirmation {
            operation,
            target,
            selection_epoch: self.inspect_id,
            architecture: Architecture::X64,
            consent: false,
        });
        self.hook_error = None;
    }
    fn start_hook(&mut self, confirmation: HookConfirmation) {
        if !confirmation.consent
            || confirmation.selection_epoch != self.inspect_id
            || self.worker_failed
            || self.in_flight.is_some()
        {
            return;
        }
        if let Some(info) = &confirmation.target {
            if self.info.as_ref().is_none_or(|current| {
                current.hwnd != info.hwnd
                    || current.pid != info.pid
                    || current.tid != info.tid
                    || current.class_name != info.class_name
            }) {
                return;
            }
        }
        self.clear_hook();
        self.content_snapshot = None;
        self.menu_snapshot = None;
        self.content_rows.clear();
        self.content_text.clear();
        self.content_warning.clear();
        self.selected_row = None;
        let id = self.id();
        let cancel = Arc::new(AtomicBool::new(false));
        let desktop = confirmation.operation == Operation::MenuDesktopOnce;
        let request = CaptureRequest {
            operation: confirmation.operation,
            architecture: confirmation.architecture,
            target: confirmation.target.as_ref().map(|info| Target {
                hwnd: info.hwnd,
                pid: info.pid,
                tid: info.tid,
            }),
            limits: CaptureLimits {
                timeout_ms: if desktop { 10_000 } else { 5_000 },
                ..CaptureLimits::default()
            },
            visible_capture_consent: true,
            desktop_menu_consent: desktop,
            language: if self.english {
                UiLanguage::English
            } else {
                UiLanguage::SimplifiedChinese
            },
        };
        self.hook_id = id;
        self.detail_id = id;
        self.pending_detail = None;
        self.reading_details = true;
        self.hook_cancel = Some(Arc::clone(&cancel));
        self.pending_hook = Some(Job::Hook {
            id,
            request,
            expected: confirmation.target,
            cancel,
        });
        self.notify(
            self.tr(
                "正在启动可见的一次性 Hook；可随时取消。",
                "Starting a visible one-shot Hook capture; you can cancel at any time.",
            ),
            false,
        );
    }
    fn hook_controls(&mut self, ui: &mut egui::Ui) {
        let Some(operation) = hook_ui::operation_for_tab(self.detail_tab) else {
            return;
        };
        let ready = !self.worker_failed
            && self.in_flight.is_none()
            && !self.inspecting
            && !self.reading_details
            && !self.exporting;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    ready && self.info.is_some(),
                    egui::Button::new(self.tr("Hook 读取…", "Read via Hook…")),
                )
                .clicked()
            {
                self.confirm_hook(operation);
            }
            if self.detail_tab == 6
                && ui
                    .add_enabled(
                        ready,
                        egui::Button::new(self.tr("桌面菜单一次…", "Desktop menu once…")),
                    )
                    .clicked()
            {
                self.confirm_hook(Operation::MenuDesktopOnce);
            }
        });
        if let Some(error) = &self.hook_error {
            ui.colored_label(
                Color32::from_rgb(170, 65, 30),
                hook_ui::error_text(error, self.english),
            );
            ui.small(self.tr(
                "需要时可自行点击“API 读取”尝试标准接口。",
                "You may explicitly choose Read via API to try the standard interface.",
            ));
            egui::CollapsingHeader::new(self.tr("技术信息", "Technical details")).show(ui, |ui| {
                ui.label(&error.message);
            });
        }
        if let Some(snapshot) = self
            .hook_result
            .as_ref()
            .filter(|snapshot| snapshot.applies_to(self.detail_tab))
        {
            let source = &snapshot.result.actual_target;
            ui.small(format!(
                "Hook · {} · HWND {} · PID {} / TID {}",
                hook_ui::architecture_name(snapshot.architecture),
                hwnd_text(source.hwnd),
                source.pid,
                source.tid
            ));
            if let Some(info) = &snapshot.source {
                ui.small(format!("{} · {}", info.process_name, info.class_name));
            } else {
                ui.small(self.tr("进程/类名现已不可查询；以上身份来自本次捕获。", "Process/class details are no longer queryable; the identity above comes from this capture."));
            }
            ui.small(snapshot.counts(self.english));
            if matches!(snapshot.result.data, CaptureData::ListView { .. }) {
                ui.small(self.tr("列标题为控件原文；空标题保持为空，未捕获标题的列使用序号。", "Headers are original control text. Empty headings stay empty; columns without a captured header use an ordinal label."));
            }
            if snapshot.result.truncated {
                ui.colored_label(Color32::from_rgb(170,65,30),self.tr("已截断：达到数量、长度、大小或时间限制；内容不完整。", "Truncated: a count, text, size, or time limit was reached. Content is incomplete."));
            }
            if let CaptureData::RichEditRtf { .. } = &snapshot.result.data {
                let bytes = snapshot.result.complete_rtf_bytes().map(<[u8]>::to_vec);
                let complete = bytes.is_some();
                if ui
                    .add_enabled(
                        complete && !self.exporting,
                        egui::Button::new(self.tr("保存原始 RTF", "Save original RTF")),
                    )
                    .clicked()
                {
                    if let Some(bytes) = bytes {
                        let id = self.id();
                        self.exporting = true;
                        self.pending_export = Some(Job::SaveRawRtf { id, bytes });
                    }
                }
                if !complete {
                    ui.small(self.tr(
                        "原始 RTF 流不完整，已禁用 .rtf 文档导出。",
                        "The original RTF stream is incomplete. RTF document export is disabled.",
                    ));
                }
            }
        } else if self.content_snapshot.is_some() || self.menu_snapshot.is_some() {
            ui.small(self.tr(
                "来源：标准 API / UI Automation 快照",
                "Source: standard API / UI Automation snapshot",
            ));
        }
        let Some(mut confirmation) = self.hook_confirmation.take() else {
            return;
        };
        if confirmation.selection_epoch != self.inspect_id {
            return;
        }
        let mut start = false;
        let mut dismiss = false;
        ui.group(|ui| {
            ui.strong(format!("{}: {}", self.tr("确认一次读取", "Confirm one capture"), hook_ui::operation_name(confirmation.operation,self.english)));
            if let Some(info) = &confirmation.target {
                ui.label(format!("HWND {}\nPID {} / TID {}\n{}\n{}",hwnd_text(info.hwnd),info.pid,info.tid,info.process_name,info.class_name));
                ui.small(self.tr("仅所选线程；架构自动检测（x86 / x64）。固定组件会暂时加载到此目标进程，并在请求结束时移除 Hook。", "Selected thread only; architecture is detected automatically (x86 / x64). The fixed component temporarily loads into this target process; the Hook is removed when the request ends."));
            } else {
                ui.colored_label(Color32::from_rgb(170,65,30),self.tr("桌面范围：仅捕获所选架构中下一次打开的原生菜单。", "Desktop scope: capture only the next opened native menu of the chosen architecture."));
                ui.horizontal(|ui| {
                    ui.label(self.tr("此次架构", "Architecture for this capture"));
                    ui.selectable_value(&mut confirmation.architecture,Architecture::X64,"x64");
                    ui.selectable_value(&mut confirmation.architecture,Architecture::X86,"x86");
                });
                ui.small(self.tr("不是同时捕获两种架构。开始后请打开目标菜单；最多 10 秒自动结束。临时 Hook 可加载到该架构的桌面应用；不持续监视。", "Both architectures are not captured together. Open the target menu after starting; capture ends within 10 seconds. The temporary Hook may load into desktop apps of this architecture; it does not keep monitoring."));
            }
            if confirmation.operation == Operation::MenuTarget {
                ui.small(self.tr("请选择菜单所属的主窗口。开始后在 5 秒内打开该窗口菜单；弹出菜单自身不是此 Hook 的目标。", "Select the window that owns the menu. Open that window's menu within 5 seconds after starting; the popup itself is not this Hook's target."));
            }
            ui.small(self.tr("可见状态条和取消按钮；拒绝高权限、不匹配架构和受保护控件。无自动保存或网络发送。", "Visible indicator and Cancel button; higher-integrity, wrong-architecture, and protected targets are rejected. No automatic saving or network transmission."));
            ui.small(self.tr("限制：512 行 × 32 列 / 1024 节点 / 深度 32 / 每项 2048 UTF-16 / 总计 1 MiB；所选目标最多 5 秒。", "Limits: 512 rows × 32 columns / 1024 nodes / depth 32 / 2048 UTF-16 units per item / 1 MiB total; selected targets have a 5-second limit."));
            egui::CollapsingHeader::new(self.tr("限制与注意事项", "Limitations")).show(ui, |ui| {
                ui.small(self.tr("菜单快照可能早于应用的菜单初始化处理，因此动态菜单项可能尚未更新。取消会结束辅助进程并移除 Hook，但不能强制中止控件已在执行的调用；挂起控件可能仍阻塞其自身线程。", "A menu snapshot may precede the application's menu-initialization handler, so dynamic items may not be updated yet. Cancellation ends the broker and removes its Hook, but cannot unwind a control call already in progress; a hung control may still block its own thread."));
            });
            let consent_label = if confirmation.target.is_some() { self.tr("我确认本次读取上述目标", "I approve this capture of the target above") } else { self.tr("我明确同意本次桌面范围菜单捕获", "I explicitly approve this one desktop-wide menu capture") };
            ui.checkbox(&mut confirmation.consent,consent_label);
            ui.horizontal(|ui| {
                if ui.add_enabled(confirmation.consent && ready,egui::Button::new(self.tr("开始一次读取", "Start one capture"))).clicked() { start=true; }
                if ui.button(self.tr("取消", "Cancel")).clicked() { dismiss=true; }
            });
        });
        if start {
            self.start_hook(confirmation);
        } else if !dismiss {
            self.hook_confirmation = Some(confirmation);
        }
    }
    fn request_content(&mut self) {
        let kind = match self.detail_tab {
            3 | 4 => DetailKind::Legacy(self.legacy_frame),
            6 => DetailKind::Menu,
            _ => DetailKind::Content,
        };
        self.queue_detail(kind);
    }
    fn detail_window(&mut self, ui: &mut egui::Ui) {
        let labels = [
            self.tr("常规", "General"),
            "ListV",
            "TreeV",
            "IE",
            "IE2",
            "RichEdit",
            self.tr("菜单", "Menu"),
            self.tr("关于", "About"),
        ];
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for (i, label) in labels.iter().enumerate() {
                if ui.selectable_label(self.detail_tab == i, *label).clicked() {
                    self.detail_tab = i;
                    self.hook_confirmation = None;
                    self.tree_expanded = true;
                    self.rebuild_rows();
                }
            }
        });
        ui.separator();
        if let Some(cancel) = &self.hook_cancel {
            ui.horizontal(|ui| {
                ui.spinner();
                if ui
                    .add_enabled(
                        !cancel.load(Ordering::Acquire),
                        egui::Button::new(self.tr("取消 Hook", "Cancel Hook")),
                    )
                    .clicked()
                {
                    cancel.store(true, Ordering::Release);
                }
                ui.small(self.tr("一次性捕获正在进行", "One-shot capture in progress"));
            });
        }
        if self.detail_tab == 7 {
            self.about_contents(ui);
            return;
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.selected.is_some() && !self.reading_details && self.hook_cancel.is_none(),
                    egui::Button::new(self.tr("API 读取", "Read via API")),
                )
                .clicked()
            {
                self.request_content();
            }
            if matches!(self.detail_tab, 0 | 1 | 2 | 6) {
                ui.label(format!(
                    "{} {}",
                    self.tr("项目数:", "Items:"),
                    self.content_rows.len()
                ));
            }
            if self.reading_details {
                ui.spinner();
            }
        });
        self.hook_controls(ui);
        if !self.content_warning.is_empty() {
            ui.label(
                RichText::new(&self.content_warning)
                    .small()
                    .color(Color32::from_rgb(146, 76, 28)),
            );
        }
        match self.detail_tab {
            0 => {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label("Listbox/Combobox");
                    self.content_table(ui, false);
                });
                ui.add_space(8.0);
                ui.group(|ui| {
                    self.icon_contents(ui);
                });
                if let Some(info) = self.info.clone() {
                    egui::CollapsingHeader::new(self.tr("窗口属性", "Window properties")).show(
                        ui,
                        |ui| {
                            let report = full_report(&info, self.english);
                            let mut report_view = report.as_str();
                            ui.add(
                                egui::TextEdit::multiline(&mut report_view)
                                    .desired_rows(8)
                                    .desired_width(f32::INFINITY),
                            );
                            ui.horizontal(|ui| {
                                if ui.button(self.tr("复制报告", "Copy report")).clicked() {
                                    ui.ctx().copy_text(full_report(&info, self.english));
                                }
                                if ui.button("JSON").clicked() {
                                    self.export(&info, true);
                                }
                            });
                        },
                    );
                }
            }
            1 | 2 => {
                if self.detail_tab == 2 {
                    ui.horizontal(|ui| {
                        if ui.button(self.tr("全部展开", "Expand all")).clicked() {
                            self.tree_expanded = true;
                        }
                        if ui.button(self.tr("收缩所有", "Collapse all")).clicked() {
                            self.tree_expanded = false;
                        }
                    });
                }
                self.content_table(ui, self.detail_tab == 2);
            }
            3 => self.ie_contents(ui, false),
            4 => self.ie_contents(ui, true),
            5 => {
                let has_raw = self
                    .hook_result
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.applies_to(5));
                ui.small(if has_raw {
                    self.tr("Hook 原始流：保存原始 RTF 可保留控件提供的格式。此处不将原始字节伪装成纯文本预览。", "Hook raw stream: Save original RTF preserves the format supplied by the control. Raw bytes are not shown as a plain-text preview.")
                } else {
                    self.tr("API / UI Automation 纯文本预览。文本 RTF 导出不保留原格式；使用 Hook 可读取原始 RTF。", "API / UI Automation plain-text preview. Text-only RTF export loses formatting; use Hook for original RTF.")
                });
                if !has_raw {
                    let mut text = self.content_text.as_str();
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .desired_rows(18)
                            .desired_width(f32::INFINITY),
                    );
                    self.content_actions(ui);
                }
            }
            6 => {
                self.content_table(ui, true);
            }
            _ => {}
        }
    }
    fn content_export_available(&self) -> bool {
        !self.content_text.is_empty()
            || self.hook_result.as_ref().is_some_and(|snapshot| {
                snapshot.applies_to(self.detail_tab)
                    && !matches!(snapshot.result.data, CaptureData::RichEditRtf { .. })
            })
    }
    fn content_export_text(&self) -> String {
        self.hook_result
            .as_ref()
            .filter(|snapshot| snapshot.applies_to(self.detail_tab))
            .map(|snapshot| snapshot.export_report(self.english))
            .unwrap_or_else(|| self.content_text.clone())
    }
    fn current_content_row(&self) -> Option<String> {
        let index = self.selected_row?;
        if let Some(snapshot) = self
            .hook_result
            .as_ref()
            .filter(|snapshot| snapshot.applies_to(self.detail_tab))
        {
            return snapshot.row_report(index, self.english);
        }
        self.content_rows
            .get(index)
            .map(|row| format!("{}\t{}\t{}", row.1, row.2, row.3))
    }
    fn content_actions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.content_export_available(),
                    egui::Button::new(self.tr("复制全部", "Copy all")),
                )
                .clicked()
            {
                ui.ctx().copy_text(self.content_export_text());
            }
            if ui
                .add_enabled(
                    self.content_export_available() && !self.exporting,
                    egui::Button::new(if self.detail_tab == 5 {
                        self.tr("保存文本 RTF", "Save text-only RTF")
                    } else {
                        self.tr("保存全部", "Save all")
                    }),
                )
                .clicked()
            {
                if self.detail_tab == 5 {
                    self.save_named(
                        coralspynext::formats::text_to_rtf(&self.content_text),
                        "rtf",
                    );
                } else {
                    self.save_string(self.content_export_text());
                }
            }
            if ui
                .add_enabled(
                    self.selected_row.is_some(),
                    egui::Button::new(self.tr("复制当前", "Copy current")),
                )
                .clicked()
            {
                if let Some(row) = self.current_content_row() {
                    ui.ctx().copy_text(row);
                }
            }
            if ui
                .add_enabled(
                    self.selected_row.is_some(),
                    egui::Button::new(self.tr("保存当前", "Save current")),
                )
                .clicked()
            {
                if let Some(row) = self.current_content_row() {
                    self.save_string(row);
                }
            }
        });
    }
    fn content_table(&mut self, ui: &mut egui::Ui, tree: bool) {
        self.content_actions(ui);
        if self.detail_tab == 1 {
            ui.small(self.tr("预览前 100 条；保存全部已捕获条目。未暴露或虚拟化的数据不会被补造。","Preview: first 100 items. Save exports all captured items; unexposed or virtualized items are not invented."));
        }
        egui::ScrollArea::both()
            .id_salt(("content", self.detail_tab))
            .max_height(240.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.detail_tab == 1 {
                    let headers = self
                        .hook_result
                        .as_ref()
                        .filter(|snapshot| snapshot.applies_to(1))
                        .map(|snapshot| snapshot.list_headers(self.english))
                        .unwrap_or_else(|| {
                            self.content_snapshot
                                .as_ref()
                                .map(coralspynext::content_view::list_headers)
                                .unwrap_or_default()
                        });
                    let structured_cells = self
                        .hook_result
                        .as_ref()
                        .filter(|snapshot| snapshot.applies_to(1))
                        .map(hook_ui::Snapshot::list_cells)
                        .unwrap_or_else(|| {
                            self.content_snapshot
                                .as_ref()
                                .map(coralspynext::content_view::list_cells)
                                .unwrap_or_default()
                        });
                    egui::Grid::new("listview_cells")
                        .striped(true)
                        .spacing([14.0, 5.0])
                        .show(ui, |ui| {
                            if headers.is_empty() {
                                ui.strong(self.tr("项目", "Item"));
                                ui.strong(self.tr("值", "Value"));
                            } else {
                                for header in &headers {
                                    ui.strong(header);
                                }
                            }
                            ui.end_row();
                            for (i, (_, name, _, value)) in
                                self.content_rows.iter().enumerate().take(100)
                            {
                                let cells: Vec<&str> = if let Some(row) =
                                    structured_cells.get(i).filter(|row| !row.is_empty())
                                {
                                    // Keep empty cells and literal tabs within a cell intact.
                                    row.iter().map(String::as_str).collect()
                                } else if headers.is_empty() || headers.len() > 1 {
                                    vec![name.as_str(), value.as_str()]
                                } else {
                                    vec![name.as_str()]
                                };
                                for (column, cell) in cells.iter().enumerate() {
                                    if column == 0 {
                                        if ui
                                            .selectable_label(self.selected_row == Some(i), *cell)
                                            .clicked()
                                        {
                                            self.selected_row = Some(i);
                                        }
                                    } else {
                                        ui.add(egui::Label::new(*cell).selectable(true));
                                    }
                                }
                                ui.end_row();
                            }
                        });
                    return;
                }
                for (i, (depth, name, role, value)) in
                    self.content_rows
                        .iter()
                        .enumerate()
                        .take(if self.detail_tab == 1 {
                            100
                        } else {
                            usize::MAX
                        })
                {
                    if tree && !self.tree_expanded && *depth > 0 {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        if tree {
                            ui.add_space((*depth).min(12) as f32 * 12.0);
                        }
                        if ui
                            .selectable_label(
                                self.selected_row == Some(i),
                                format!("{name}    {role}    {value}"),
                            )
                            .clicked()
                        {
                            self.selected_row = Some(i);
                        }
                    });
                }
                if self.content_rows.is_empty() {
                    ui.label(self.tr(
                        "暂无数据。请选择 API 或 Hook 读取。",
                        "No data. Choose an API or Hook capture.",
                    ));
                }
            });
    }
    fn ie_contents(&mut self, ui: &mut egui::Ui, more: bool) {
        ui.small(self.tr(
            "页面快照；导航加载完成后可再次读取。",
            "Page snapshot. Read again after navigation finishes loading.",
        ));
        let snapshot = self.legacy_snapshot.clone();
        if !more {
            egui::Grid::new("ie_fields")
                .num_columns(2)
                .spacing([6.0, 4.0])
                .show(ui, |ui| {
                    readonly_row(
                        ui,
                        self.tr("位置:", "Location:"),
                        snapshot.as_ref().map(|s| s.location.as_str()).unwrap_or(""),
                        false,
                        ui.available_width() - 70.0,
                    );
                    readonly_row(
                        ui,
                        self.tr("标题:", "Title:"),
                        snapshot.as_ref().map(|s| s.title.as_str()).unwrap_or(""),
                        false,
                        ui.available_width() - 70.0,
                    );
                    readonly_row(
                        ui,
                        self.tr("程序:", "Application:"),
                        snapshot
                            .as_ref()
                            .map(|s| s.application.as_str())
                            .unwrap_or(""),
                        false,
                        ui.available_width() - 70.0,
                    );
                });
            ui.horizontal(|ui| {
                ui.label(self.tr("框架:", "Frame:"));
                let old = self.legacy_frame;
                let label = self
                    .legacy_frame
                    .and_then(|i| snapshot.as_ref().and_then(|s| s.frames.get(i)).cloned())
                    .unwrap_or_else(|| self.tr("最上", "Top").to_string());
                egui::ComboBox::from_id_salt("ie_frame")
                    .selected_text(label)
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.legacy_frame,
                            None,
                            if self.english { "Top" } else { "最上" },
                        );
                        if let Some(s) = &snapshot {
                            for (i, name) in s.frames.iter().enumerate() {
                                ui.selectable_value(
                                    &mut self.legacy_frame,
                                    Some(i),
                                    format!("{i}: {name}"),
                                );
                            }
                        }
                    });
                if ui.small_button(self.tr("最上", "Top")).clicked() {
                    self.legacy_frame = None;
                }
                if old != self.legacy_frame {
                    self.legacy_snapshot = None;
                    self.queue_detail(DetailKind::Legacy(self.legacy_frame));
                }
                if ui.small_button(self.tr("更多 >>", "More >>")).clicked() {
                    self.detail_tab = 4;
                }
            });
            ui.horizontal(|ui| {
                for (action, zh, en) in [
                    (LegacyAction::Back, "返回", "Back"),
                    (LegacyAction::Forward, "前进", "Forward"),
                    (LegacyAction::Stop, "停止", "Stop"),
                    (LegacyAction::Refresh, "刷新", "Refresh"),
                    (LegacyAction::Home, "主页", "Home"),
                ] {
                    if ui
                        .add_enabled(
                            snapshot.is_some() && !self.reading_details,
                            egui::Button::new(self.tr(zh, en)).small(),
                        )
                        .clicked()
                    {
                        self.queue_detail(DetailKind::Action(self.legacy_frame, action));
                    }
                }
                if ui.small_button(self.tr("复制", "Copy")).clicked() {
                    if let Some(s) = &snapshot {
                        ui.ctx().copy_text(s.source.clone());
                    }
                }
                if ui.small_button(self.tr("保存 HTML", "Save HTML")).clicked() {
                    if let Some(s) = &snapshot {
                        self.save_named(s.source.clone(), "html");
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(self.tr("高亮显示:", "Highlight:"));
                ui.add(egui::TextEdit::singleline(&mut self.highlight_needle).desired_width(210.0));
                if ui
                    .add_enabled(
                        snapshot.is_some()
                            && !self.highlight_needle.is_empty()
                            && !self.reading_details,
                        egui::Button::new(self.tr("高亮显示", "Highlight")),
                    )
                    .on_hover_text(self.tr(
                        "会更改目标页面文字格式，效果可能保留到刷新页面。",
                        "Changes the target page formatting; effects may remain until reload.",
                    ))
                    .clicked()
                {
                    self.queue_detail(DetailKind::Highlight {
                        frame: self.legacy_frame,
                        needle: self.highlight_needle.clone(),
                        text: self.settings.highlight_text,
                        background: self.settings.highlight_background,
                        bold: self.settings.highlight_bold,
                    });
                }
            });
            ui.label(self.tr("源代码:", "Source:"));
            let mut source = snapshot.as_ref().map(|s| s.source.as_str()).unwrap_or("");
            egui::ScrollArea::both()
                .id_salt("ie_source")
                .max_height(195.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut source)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(12),
                    );
                });
        } else {
            ui.horizontal(|ui| {
                let filters = [
                    self.tr("所有链接", "All links"),
                    self.tr("外部链接", "External links"),
                    self.tr("图片", "Images"),
                    "Flash",
                ];
                egui::ComboBox::from_id_salt("ie_links_filter")
                    .selected_text(filters[self.legacy_filter])
                    .show_ui(ui, |ui| {
                        for (i, name) in filters.iter().enumerate() {
                            if ui
                                .selectable_value(&mut self.legacy_filter, i, *name)
                                .clicked()
                            {
                                self.legacy_link = None;
                            }
                        }
                    });
                ui.small(self.tr("[双击复制到剪贴板]", "[Double-click to copy]"));
                if ui.small_button(self.tr("更多 >>", "More >>")).clicked() {
                    self.legacy_more = !self.legacy_more;
                }
            });
            egui::ScrollArea::both()
                .id_salt("ie_links")
                .max_height(130.0)
                .min_scrolled_height(85.0)
                .show(ui, |ui| {
                    if let Some(s) = &snapshot {
                        for (i, link) in s.links.iter().enumerate() {
                            let show = match self.legacy_filter {
                                0 => matches!(link.kind.as_str(), "link" | "external"),
                                1 => link.kind == "external",
                                2 => link.kind == "image",
                                _ => link.kind == "flash",
                            };
                            if !show {
                                continue;
                            }
                            let response = ui.selectable_label(
                                self.legacy_link == Some(i),
                                format!("{}  {}", link.url, link.text),
                            );
                            if response.clicked() {
                                self.legacy_link = Some(i);
                            }
                            if response.double_clicked() {
                                ui.ctx().copy_text(link.url.clone());
                            }
                        }
                    }
                });
            ui.horizontal(|ui|{
                let selected=self.legacy_link.and_then(|i|snapshot.as_ref().and_then(|s|s.links.get(i)));
                if ui.add_enabled(selected.is_some(),egui::Button::new(self.tr("复制 URL","Copy URL"))).clicked(){if let Some(link)=selected{ui.ctx().copy_text(link.url.clone());}}
                if ui.add_enabled(selected.is_some()&&!self.exporting,egui::Button::new(self.tr("下载","Download"))).on_hover_text(self.tr("保存所选 HTTP(S) 资源。无登录凭证，不下载可执行类型，不跟随重定向。","Save the selected HTTP(S) resource. No credentials, executable types, or redirects.")).clicked(){if let Some(link)=selected{self.cancel_pick();let id=self.id();self.exporting=true;self.pending_export=Some(Job::Download{id,url:link.url.clone()});}}
                if ui.button(self.tr("保存链接","Save links")).clicked(){if let Some(s)=&snapshot{self.save_string(s.links.iter().map(|l|format!("{}\t{}\t{}",l.kind,l.url,l.text)).collect::<Vec<_>>().join("\n"));}}
            });
            if self.legacy_more {
                if let Some(snapshot) = &snapshot {
                    ui.group(|ui| {
                        ui.label(format!(
                            "{}  ·  links {}  ·  forms {}  ·  frames {}",
                            hwnd_text(snapshot.hwnd),
                            snapshot.links.len(),
                            snapshot.forms.len(),
                            snapshot.frames.len()
                        ));
                        if let Some(link) = self.legacy_link.and_then(|i| snapshot.links.get(i)) {
                            ui.label(format!("{}: {}", link.kind, link.url));
                            ui.label(&link.text);
                        }
                        if ui
                            .small_button(self.tr("保存详情 JSON", "Save details JSON"))
                            .clicked()
                        {
                            match serde_json::to_string_pretty(snapshot) {
                                Ok(text) => self.save_named(text, "json"),
                                Err(error) => self.notify(error.to_string(), true),
                            }
                        }
                    });
                }
            }
            ui.separator();
            ui.label(self.tr("表单元素:", "Form elements:"));
            egui::ScrollArea::both()
                .id_salt("ie_forms")
                .max_height(160.0)
                .show(ui, |ui| {
                    egui::Grid::new("ie_forms_table")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong(self.tr("类型", "Type"));
                            ui.strong(self.tr("名称", "Name"));
                            ui.strong(self.tr("值", "Value"));
                            ui.end_row();
                            if let Some(s) = &snapshot {
                                for form in &s.forms {
                                    ui.label(&form.kind);
                                    ui.label(if form.protected {
                                        locale::label("[受保护]", "[Protected]")
                                    } else {
                                        &form.name
                                    });
                                    let value = if form.protected {
                                        locale::label("[密码受保护]", "[Password protected]")
                                    } else {
                                        &form.value
                                    };
                                    let response = ui.add(egui::Label::new(value).selectable(true));
                                    if response.double_clicked() {
                                        ui.ctx().copy_text(value.to_string());
                                    }
                                    ui.end_row();
                                }
                            }
                        });
                });
        }
        if snapshot.is_none() {
            ui.label(self.tr("选择旧版 IE / MSHTML 窗口后点击“读取详情”。Chromium 页面不支持此旧接口。","Select a legacy IE / MSHTML window and click Read details. Chromium does not expose this legacy interface."));
        }
    }
    fn about_contents(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(15.0);
            ui.heading("CoralSpyNext");
            ui.label(format!(
                "v{} · Rust · Windows 11 x64",
                env!("CARGO_PKG_VERSION")
            ));
            ui.label(format!(
                "{}: {}",
                self.tr("最后编译", "Built"),
                env!("BUILD_UTC")
            ));
            ui.add_space(8.0);
            ui.label(self.tr(
                "经典 CoralSpy 界面的独立现代重写",
                "An independent modern rewrite of the classic CoralSpy interface",
            ));
            ui.label(self.tr(
                "保留原版窗口布局与操作习惯。",
                "Preserves the classic window layout and interaction.",
            ));
        });
        ui.add_space(8.0);
        ui.label("Original CoralSpy 1.0: Copyright 2003–2004 Coral Studio. 2004-07-31.");
        ui.label(self.tr("CoralSpyNext 为独立重写，原作名称与作者归原作者所有。","CoralSpyNext is an independent rewrite; original names and credits remain with their authors."));
        ui.horizontal(|ui| {
            ui.hyperlink_to("GitHub", "https://github.com/Antman2023/CoralSpyNext");
            ui.hyperlink_to(
                "Issues",
                "https://github.com/Antman2023/CoralSpyNext/issues",
            );
        });
        ui.add_space(14.0);
        ui.group(|ui| { ui.label(self.tr("可查看详情的控件类型", "Supported control categories")); ui.label("Listbox / Combobox / ListView / TreeView / RichEdit\nUI Automation / Win32 menus / Legacy IE-MSHTML"); });
        ui.add_space(10.0);
        ui.label(self.tr("密码始终保护。受权限、应用实现或超时影响时，明确显示不可用。不会自动提权、注入进程或记录键盘。", "Passwords remain protected. Permission, provider and timeout limits are shown explicitly. No automatic elevation, injection or key logging."));
    }
    fn color_window(&mut self, ui: &mut egui::Ui) {
        let sample = self.picker.as_ref().and_then(|p| p.sample).or(self.color);
        let [r, g, b] = sample.map(|c| [c.r, c.g, c.b]).unwrap_or(self.color_edit);
        let colorref = r as u32 | ((g as u32) << 8) | ((b as u32) << 16);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let mut swatch = [r, g, b];
            if ui
                .color_edit_button_srgb(&mut swatch)
                .on_hover_text(self.tr("选择颜色", "Choose color"))
                .changed()
            {
                self.cancel_pick();
                self.color_edit = swatch;
                self.color = None;
            }
            color_field(ui, "DEC", &colorref.to_string(), 92.0);
            color_field(ui, "HEX", &format!("0x{colorref:06X}"), 95.0);
            color_field(ui, "HTML", &format!("#{r:02X}{g:02X}{b:02X}"), 92.0);
            color_field(ui, self.tr("红", "Red"), &r.to_string(), 39.0);
            color_field(ui, self.tr("绿", "Green"), &g.to_string(), 39.0);
            color_field(ui, self.tr("蓝", "Blue"), &b.to_string(), 39.0);
            ui.add_space((ui.available_width() - 48.0).max(0.0));
            self.crosshair(ui, PickKind::Color);
        });
    }
    fn options_window(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(self.tr("热键", "Hotkeys"));
            let action_names = [
                self.tr("显示主窗口", "Show main window"),
                self.tr("显示颜色拾取器", "Show color picker"),
                self.tr("准星－按两次", "Crosshair — press twice"),
            ];
            egui::ComboBox::from_id_salt("hotkey_action")
                .selected_text(action_names[self.hotkey_edit])
                .show_ui(ui, |ui| {
                    for (i, name) in action_names.iter().enumerate() {
                        ui.selectable_value(&mut self.hotkey_edit, i, *name);
                    }
                });
            let mut binding = self.settings_draft.hotkeys[self.hotkey_edit];
            ui.horizontal(|ui| {
                for (mask, label) in [(2, "Ctrl"), (1, "Alt"), (4, "Shift"), (8, "Win")] {
                    let mut on = binding.modifiers & mask != 0;
                    if ui.checkbox(&mut on, label).changed() {
                        if on {
                            binding.modifiers |= mask;
                        } else {
                            binding.modifiers &= !mask;
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("hotkey_key")
                    .selected_text(key_name(binding.key))
                    .width(65.0)
                    .show_ui(ui, |ui| {
                        for key in (0x30..=0x39).chain(0x41..=0x5a).chain(0x70..=0x87) {
                            ui.selectable_value(&mut binding.key, key, key_name(key));
                        }
                    });
                ui.checkbox(
                    &mut self.settings_draft.hotkeys_enabled,
                    locale::label("启用热键", "Enable hotkeys"),
                );
            });
            self.settings_draft.hotkeys[self.hotkey_edit] = binding;
            ui.horizontal(|ui| {
                if ui.button(self.tr("注册热键", "Register")).clicked() {
                    self.test_hotkeys(true);
                }
                if ui.button(self.tr("取消热键", "Unregister")).clicked() {
                    self.test_hotkeys(false);
                }
            });
        });
        ui.add_space(6.0);
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(self.tr("常规", "General"));
            ui.horizontal(|ui| {
                ui.label(self.tr("语言:", "Language:"));
                ui.selectable_value(&mut self.settings_draft.language, "zh-CN".into(), "Chinese");
                ui.selectable_value(&mut self.settings_draft.language, "en-US".into(), "English");
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.settings_draft.dark, locale::label("深色", "Dark"));
                ui.checkbox(
                    &mut self.settings_draft.always_on_top,
                    locale::label("置顶", "On top"),
                );
            });
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut self.settings_draft.tray_enabled,
                    locale::label("托盘", "Tray"),
                );
                ui.checkbox(
                    &mut self.settings_draft.minimize_to_tray,
                    locale::label("最小化到托盘", "Minimize to tray"),
                );
            });
        });
        ui.add_space(6.0);
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(self.tr("IE 高亮显示", "IE highlight"));
            ui.horizontal(|ui| {
                ui.label(self.tr("文字:", "Text:"));
                ui.color_edit_button_srgb(&mut self.settings_draft.highlight_text);
                ui.label(self.tr("背景:", "Background:"));
                ui.color_edit_button_srgb(&mut self.settings_draft.highlight_background);
                ui.checkbox(
                    &mut self.settings_draft.highlight_bold,
                    locale::label("粗体", "Bold"),
                );
            });
            let mut text = RichText::new(self.tr("这就是预览效果。", "This is a preview."))
                .color(Color32::from_rgb(
                    self.settings_draft.highlight_text[0],
                    self.settings_draft.highlight_text[1],
                    self.settings_draft.highlight_text[2],
                ))
                .background_color(Color32::from_rgb(
                    self.settings_draft.highlight_background[0],
                    self.settings_draft.highlight_background[1],
                    self.settings_draft.highlight_background[2],
                ));
            if self.settings_draft.highlight_bold {
                text = text.strong();
            }
            ui.label(text);
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(self.tr("确定", "OK")).clicked() && self.apply_settings(ui.ctx()) {
                self.options_open = false;
            }
            if ui.button(self.tr("取消", "Cancel")).clicked() {
                self.cancel_options();
            }
        });
        if self.status_error {
            ui.label(
                RichText::new(&self.status)
                    .small()
                    .color(Color32::from_rgb(160, 40, 30)),
            );
        }
    }
    fn child_windows(&mut self, ctx: &egui::Context) {
        if self.details_open {
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("details"),
                egui::ViewportBuilder::default()
                    .with_title(self.tr("查看详情", "View details"))
                    .with_inner_size([510.0, 460.0])
                    .with_min_inner_size([490.0, 380.0]),
                |ctx, _| {
                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.details_open = false;
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("detail_outer")
                            .show(ui, |ui| self.detail_window(ui));
                    });
                },
            );
        }
        if self.color_open {
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("colors"),
                egui::ViewportBuilder::default()
                    .with_title(self.tr("颜色拾取器", "Color picker"))
                    .with_inner_size([620.0, 66.0])
                    .with_min_inner_size([620.0, 66.0])
                    .with_resizable(false),
                |ctx, _| {
                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.color_open = false;
                        self.cancel_pick();
                    }
                    egui::CentralPanel::default().show(ctx, |ui| self.color_window(ui));
                },
            );
        }
        if self.options_open {
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("options"),
                egui::ViewportBuilder::default()
                    .with_title(self.tr("选项", "Options"))
                    .with_inner_size([420.0, 350.0])
                    .with_resizable(false),
                |ctx, _| {
                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.cancel_options();
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("options_scroll")
                            .show(ui, |ui| self.options_window(ui));
                    });
                },
            );
        }
    }
}

impl Drop for CoralSpyApp {
    fn drop(&mut self) {
        self.clear_hook();
    }
}

impl eframe::App for CoralSpyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested())
            && (self.hook_running_id.is_some() || self.pending_hook.is_some())
        {
            self.closing_after_hook = true;
            if let Some(cancel) = &self.hook_cancel {
                cancel.store(true, Ordering::Release);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.notify(
                self.tr(
                    "正在停止 Hook，完成后退出…",
                    "Stopping the Hook capture before closing…",
                ),
                false,
            );
        }
        self.service_worker(ctx);
        if self.closing_after_hook && self.hook_running_id.is_none() && self.pending_hook.is_none()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.service_picker(ctx);
        self.service_desktop(ctx);
        if self.picker.is_none() && ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.refresh_target();
        }
        egui::TopBottomPanel::top("classic_toolbar")
            .exact_height(35.0)
            .show(ctx, |ui| self.main_toolbar(ui));
        egui::TopBottomPanel::bottom("classic_status")
            .exact_height(25.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.in_flight.is_some() {
                        ui.add(egui::Spinner::new().size(12.0));
                    }
                    ui.add(egui::Label::new(RichText::new(&self.status).size(10.5)).truncate())
                        .on_hover_text(&self.status);
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .inner_margin(8)
                    .fill(ctx.style().visuals.panel_fill),
            )
            .show(ctx, |ui| self.main_panel(ui));
        self.child_windows(ctx);
        self.service_worker(ctx);
        if self.in_flight.is_some() || self.picker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
}

fn capture_hook(
    mut request: CaptureRequest,
    expected: Option<WindowInfo>,
    cancel: &AtomicBool,
) -> Result<hook_ui::Snapshot, CaptureError> {
    let mismatch = || {
        CaptureError::new(
            ErrorCode::TargetMismatch,
            "Selected HWND/PID/TID/class identity changed",
        )
    };
    if cancel.load(Ordering::Acquire) {
        return Err(CaptureError::new(
            ErrorCode::Cancelled,
            "Capture cancelled before launch",
        ));
    }
    if let Some(info) = &expected {
        if platform::window_identity(info.hwnd).map_err(|_| mismatch())?
            != (info.pid, info.class_name.clone())
        {
            return Err(mismatch());
        }
        let target = Target {
            hwnd: info.hwnd,
            pid: info.pid,
            tid: info.tid,
        };
        if request.target != Some(target) {
            return Err(mismatch());
        }
        request.architecture = hook::target_architecture(target)?;
    }
    let operation = request.operation;
    let architecture = request.architecture;
    let result = hook::capture(request, cancel)?;
    if cancel.load(Ordering::Acquire) {
        return Err(CaptureError::new(
            ErrorCode::Cancelled,
            "Capture cancelled; result discarded",
        ));
    }
    if let Some(info) = &expected {
        if result.actual_target
            != (Target {
                hwnd: info.hwnd,
                pid: info.pid,
                tid: info.tid,
            })
        {
            return Err(mismatch());
        }
    }
    let source = expected.or_else(|| {
        platform::inspect_window(result.actual_target.hwnd)
            .ok()
            .filter(|info| {
                info.pid == result.actual_target.pid && info.tid == result.actual_target.tid
            })
    });
    Ok(hook_ui::Snapshot {
        operation,
        architecture,
        result,
        source,
    })
}

fn readonly_row(ui: &mut egui::Ui, label: &str, value: &str, multiline: bool, width: f32) {
    ui.label(label);
    let mut view = value;
    if multiline {
        ui.add(
            egui::TextEdit::multiline(&mut view)
                .desired_width(width)
                .desired_rows(2)
                .font(egui::TextStyle::Body),
        );
    } else {
        ui.add(egui::TextEdit::singleline(&mut view).desired_width(width));
    }
    ui.end_row();
}
fn color_field(ui: &mut egui::Ui, label: &str, value: &str, width: f32) {
    let mut view = value;
    let response = ui.add(egui::TextEdit::singleline(&mut view).desired_width(width));
    response.on_hover_text(format!(
        "{label}: {value}\n{}",
        locale::label("Ctrl+C 复制", "Ctrl+C to copy")
    ));
}
fn tool(ui: &mut egui::Ui, index: usize, tooltip: &str, active: bool) -> bool {
    let response = ui.add_sized([25.0, 25.0], egui::Button::new("").selected(active));
    let rect = response.rect.shrink(5.0);
    let p = ui.painter().with_clip_rect(response.rect);
    let ink = ui.visuals().text_color();
    let stroke = Stroke::new(1.2_f32, ink);
    let c = rect.center();
    let line = |a: Vec2, b: Vec2| {
        p.line_segment([rect.min + a, rect.min + b], stroke);
    };
    match index {
        0 => {
            for (offset, color) in [
                (Vec2::new(4.0, 4.0), Color32::from_rgb(200, 60, 55)),
                (Vec2::new(11.0, 4.0), Color32::from_rgb(50, 145, 80)),
                (Vec2::new(4.0, 11.0), Color32::from_rgb(55, 95, 190)),
                (Vec2::new(11.0, 11.0), Color32::from_rgb(215, 166, 35)),
            ] {
                p.circle_filled(rect.min + offset, 3.0, color);
            }
        }
        1 => {
            p.circle_stroke(c, 6.0, stroke);
            line(Vec2::new(13.0, 0.0), Vec2::new(13.0, 6.0));
            line(Vec2::new(13.0, 6.0), Vec2::new(8.0, 5.0));
        }
        2 => {
            p.rect_stroke(rect.shrink(1.0), 0.0, stroke, egui::StrokeKind::Inside);
            for y in [4.0, 7.0, 10.0] {
                line(Vec2::new(4.0, y), Vec2::new(11.0, y));
            }
        }
        3 => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    rect.min,
                    rect.min + Vec2::new(3.0, 13.0),
                    rect.min + Vec2::new(6.0, 8.0),
                    rect.min + Vec2::new(12.0, 7.0),
                ],
                ink,
                Stroke::NONE,
            ));
        }
        4 => {
            for y in [3.0, 7.0, 11.0] {
                line(Vec2::new(1.0, y), Vec2::new(14.0, y));
                p.circle_filled(rect.min + Vec2::new(2.0, y), 1.0, ink);
            }
        }
        5 => {
            p.rect_stroke(
                egui::Rect::from_min_size(rect.min, Vec2::new(10.0, 11.0)),
                0.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.rect_filled(
                egui::Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(10.0, 11.0)),
                0.0,
                ui.visuals().panel_fill,
            );
            p.rect_stroke(
                egui::Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(10.0, 11.0)),
                0.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        6 => {
            p.rect_filled(rect, 1.0, Color32::from_rgb(55, 95, 160));
            p.rect_filled(
                egui::Rect::from_min_size(rect.min + Vec2::new(3.0, 1.0), Vec2::new(8.0, 5.0)),
                0.0,
                Color32::from_gray(225),
            );
            p.rect_filled(
                egui::Rect::from_min_size(rect.min + Vec2::new(3.0, 9.0), Vec2::new(8.0, 5.0)),
                0.0,
                Color32::from_gray(245),
            );
        }
        7 => {
            p.circle_stroke(c, 4.0, stroke);
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::FRAC_PI_4;
                let v = Vec2::angled(angle);
                p.line_segment([c + v * 4.0, c + v * 7.0], stroke);
            }
        }
        8 => {
            p.rect_stroke(rect, 1.0, stroke, egui::StrokeKind::Inside);
            for y in [4.0, 8.0] {
                for x in [3.0, 7.0, 11.0] {
                    p.circle_filled(rect.min + Vec2::new(x, y), 0.8, ink);
                }
            }
            line(Vec2::new(4.0, 12.0), Vec2::new(10.0, 12.0));
        }
        9 => {
            line(Vec2::new(1.0, 10.0), Vec2::new(1.0, 14.0));
            line(Vec2::new(1.0, 14.0), Vec2::new(14.0, 14.0));
            line(Vec2::new(14.0, 14.0), Vec2::new(14.0, 10.0));
            line(Vec2::new(7.0, 1.0), Vec2::new(7.0, 10.0));
            line(Vec2::new(3.0, 6.0), Vec2::new(7.0, 10.0));
            line(Vec2::new(11.0, 6.0), Vec2::new(7.0, 10.0));
        }
        10 => {
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                "?",
                FontId::proportional(18.0),
                Color32::from_rgb(45, 95, 170),
            );
        }
        _ => {
            line(Vec2::new(4.0, 1.0), Vec2::new(11.0, 1.0));
            line(Vec2::new(4.0, 1.0), Vec2::new(4.0, 8.0));
            line(Vec2::new(11.0, 1.0), Vec2::new(11.0, 8.0));
            line(Vec2::new(2.0, 8.0), Vec2::new(13.0, 8.0));
            line(Vec2::new(7.0, 8.0), Vec2::new(7.0, 15.0));
        }
    }
    response.on_hover_text(tooltip).clicked()
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windows = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into());
    // Use installed system fonts only. A .ttc's first face is selected by egui.
    for filename in [
        "msyh.ttc",
        "msyh.ttf",
        "simhei.ttf",
        "simsun.ttc",
        "segoeui.ttf",
    ] {
        if let Ok(data) = std::fs::read(windows.join("Fonts").join(filename)) {
            fonts
                .font_data
                .insert("windows_ui".into(), egui::FontData::from_owned(data).into());
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "windows_ui".into());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push("windows_ui".into());
            break;
        }
    }
    ctx.set_fonts(fonts);
}
fn apply_theme(ctx: &egui::Context, dark: bool) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = if dark {
        Color32::from_rgb(40, 40, 43)
    } else {
        Color32::from_rgb(240, 240, 240)
    };
    visuals.window_fill = visuals.panel_fill;
    visuals.extreme_bg_color = if dark {
        Color32::from_rgb(13, 19, 28)
    } else {
        Color32::from_rgb(249, 250, 252)
    };
    visuals.selection.bg_fill = if dark {
        Color32::from_rgb(65, 80, 110)
    } else {
        Color32::from_rgb(185, 210, 239)
    };
    visuals.selection.stroke = Stroke::new(
        1.0_f32,
        if dark {
            ACCENT
        } else {
            Color32::from_rgb(153, 63, 39)
        },
    );
    visuals.hyperlink_color = if dark {
        ACCENT
    } else {
        Color32::from_rgb(145, 57, 32)
    };
    visuals.widgets.noninteractive.bg_stroke.color = if dark {
        Color32::from_rgb(45, 55, 69)
    } else {
        Color32::from_rgb(214, 219, 226)
    };
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
    visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
    visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::ZERO;
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.item_spacing = Vec2::new(5.0, 4.0);
        style.spacing.button_padding = Vec2::new(6.0, 4.0);
        style.spacing.interact_size.y = 23.0;
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, FontId::proportional(24.0));
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, FontId::monospace(12.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(11.0));
    });
}
fn style_names(style: u32, ex_style: u32, english: bool) -> String {
    let mut names = Vec::new();
    for (bit, name) in [
        (0x8000_0000, "WS_POPUP"),
        (0x4000_0000, "WS_CHILD"),
        (0x1000_0000, "WS_VISIBLE"),
        (0x0800_0000, "WS_DISABLED"),
        (0x00C0_0000, "WS_CAPTION"),
        (0x0004_0000, "WS_THICKFRAME"),
    ] {
        if style & bit == bit {
            names.push(name);
        }
    }
    for (bit, name) in [
        (0x0000_0008, "WS_EX_TOPMOST"),
        (0x0000_0080, "WS_EX_TOOLWINDOW"),
        (0x0004_0000, "WS_EX_APPWINDOW"),
        (0x0008_0000, "WS_EX_LAYERED"),
        (0x0020_0000, "WS_EX_NOREDIRECTIONBITMAP"),
    ] {
        if ex_style & bit == bit {
            names.push(name);
        }
    }
    if names.is_empty() {
        if english {
            "No common flags matched; see the raw values above.".into()
        } else {
            "未匹配常见样式标志；完整值见上方。".into()
        }
    } else {
        names.join("  ·  ")
    }
}
fn full_report(info: &WindowInfo, english: bool) -> String {
    if english {
        format!(
        "CoralSpyNext — Window inspection snapshot\n\nWindow handle: {}\nParent handle: {}\nTitle: {}\nClass: {}\nProcess name: {}\nPID: {}\nTID: {}\n\nWindow bounds: ({}, {}) → ({}, {})\nWindow size: {} × {} px\nClient bounds: ({}, {}) → ({}, {})\nClient size: {} × {} px\nDPI: {}\n\nVisible: {}\nEnabled: {}\nMinimized: {}\nMaximized: {}\nUnicode: {}\nStyle: 0x{:08X}\nExStyle: 0x{:08X}\nCommon flags: {}\n\nRead status: {}\n\nLocal window metadata only. Passwords and input field contents are not read.\n",
        hwnd_text(info.hwnd), hwnd_text(info.parent), info.title, info.class_name, info.process_name, info.pid, info.tid,
        info.rect.left, info.rect.top, info.rect.right, info.rect.bottom, info.rect.width(), info.rect.height(),
        info.client_rect.left, info.client_rect.top, info.client_rect.right, info.client_rect.bottom, info.client_rect.width(), info.client_rect.height(), info.dpi,
        info.visible, info.enabled, info.minimized, info.maximized, info.is_unicode, info.style, info.ex_style, style_names(info.style, info.ex_style, english), info.text_status,
    )
    } else {
        format!(
        "CoralSpyNext — 窗口检查快照\n\n窗口句柄: {}\n父窗口句柄: {}\n标题: {}\n类名: {}\n进程名称: {}\nPID: {}\nTID: {}\n\n窗口边界: ({}, {}) → ({}, {})\n窗口大小: {} × {} px\n客户区边界: ({}, {}) → ({}, {})\n客户区大小: {} × {} px\nDPI: {}\n\n可见: {}\n启用: {}\n最小化: {}\n最大化: {}\nUnicode: {}\nStyle: 0x{:08X}\nExStyle: 0x{:08X}\n常见标志: {}\n\n读取状态: {}\n\n仅本机窗口元数据，不读取密码或输入框内容。\n",
        hwnd_text(info.hwnd), hwnd_text(info.parent), info.title, info.class_name, info.process_name, info.pid, info.tid,
        info.rect.left, info.rect.top, info.rect.right, info.rect.bottom, info.rect.width(), info.rect.height(),
        info.client_rect.left, info.client_rect.top, info.client_rect.right, info.client_rect.bottom, info.client_rect.width(), info.client_rect.height(), info.dpi,
        info.visible, info.enabled, info.minimized, info.maximized, info.is_unicode, info.style, info.ex_style, style_names(info.style, info.ex_style, english), info.text_status,
    )
    }
}

pub fn app_icon() -> egui::IconData {
    let size = 64usize;
    let mut rgba = vec![0; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let dx = (x as i32 - 32).abs();
            let dy = (y as i32 - 32).abs();
            let inside = dx < 30
                && dy < 30
                && (dx < 20 || dy < 20 || (dx - 20).pow(2) + (dy - 20).pow(2) < 100);
            if !inside {
                continue;
            }
            let ink = ((17..23).contains(&x) && (17..47).contains(&y))
                || ((17..47).contains(&x) && ((17..23).contains(&y) || (41..47).contains(&y)))
                || ((x as i32 - 44).pow(2) + (y as i32 - 32).pow(2) < 17);
            let pixel = if ink {
                [50, 29, 29, 255]
            } else {
                [248, 139, 113, 255]
            };
            rgba[(y * size + x) * 4..(y * size + x) * 4 + 4].copy_from_slice(&pixel);
        }
    }
    egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    }
}
fn key_name(key: u32) -> String {
    match key {
        0x30..=0x39 | 0x41..=0x5a => char::from_u32(key).unwrap_or('?').to_string(),
        0x70..=0x87 => format!("F{}", key - 0x6f),
        _ => format!("0x{key:02X}"),
    }
}

fn classic_icon_slots(icons: &[IconImage]) -> [Option<usize>; 4] {
    let source = |token: &str| {
        icons
            .iter()
            .position(|icon| icon.kind.split(" / ").any(|part| part == token))
    };
    [
        source("窗口大图标"),
        source("窗口小图标").or_else(|| source("窗口小图标2")),
        source("窗口类大图标"),
        source("窗口类小图标"),
    ]
}

#[cfg(test)]
mod classic_icon_tests {
    use super::*;
    fn icon(kind: &str) -> IconImage {
        IconImage {
            kind: kind.into(),
            ..Default::default()
        }
    }
    #[test]
    fn report_language_never_translates_captured_fields() {
        let info = WindowInfo {
            title: "原始标题 Title: 未读取".into(),
            class_name: "自定义类".into(),
            process_name: "应用.exe".into(),
            text_status: "Provider detail: 原样保留".into(),
            ..Default::default()
        };
        let en = full_report(&info, true);
        let zh = full_report(&info, false);
        assert!(en.contains("Window handle:"));
        assert!(!en.contains("窗口句柄:"));
        assert!(zh.contains("窗口句柄:"));
        for value in [
            &info.title,
            &info.class_name,
            &info.process_name,
            &info.text_status,
        ] {
            assert!(en.contains(value));
            assert!(zh.contains(value));
        }
    }
    #[test]
    fn fixed_slots_use_sources_not_discovery_order() {
        let icons = vec![
            icon("窗口类小图标"),
            icon("窗口小图标2"),
            icon("程序文件大图标"),
            icon("窗口大图标"),
            icon("窗口类大图标"),
        ];
        assert_eq!(
            classic_icon_slots(&icons),
            [Some(3), Some(1), Some(4), Some(0)]
        );
    }
    #[test]
    fn merged_sources_can_share_the_real_image() {
        let icons = vec![
            icon("窗口小图标2 / 窗口类小图标"),
            icon("窗口大图标 / 窗口类大图标"),
        ];
        assert_eq!(
            classic_icon_slots(&icons),
            [Some(1), Some(0), Some(1), Some(0)]
        );
    }
    #[test]
    fn executable_fallback_does_not_fill_a_missing_source_slot() {
        assert_eq!(
            classic_icon_slots(&[icon("程序文件大图标"), icon("程序文件小图标")]),
            [None; 4]
        );
        assert_eq!(classic_icon_slots(&[]), [None; 4]);
    }
}
