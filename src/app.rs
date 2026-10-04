//! Classic native multi-window UI with one bounded background command worker.
use coralspynext::{
    accessibility,
    config::{self, AppSettings},
    desktop::{DesktopEvent, DesktopService, HotkeyBinding},
    extras,
    legacy::{self, LegacyAction, LegacySnapshot},
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
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
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
            | Self::Download { id, .. } => *id,
        }
    }
}
enum Reply {
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
            Self::Inspected { id, .. } | Self::Exported { id, .. } | Self::Details { id, .. } => {
                *id
            }
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
                                    Err("目标已关闭或句柄被复用，请重新选取。".into())
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
                                    return Err("目标已关闭或句柄被复用，请重新选取。".into());
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
                                    return Err("读取期间窗口身份发生变化，已丢弃结果。".into());
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
            .expect("Cannot start inspection worker");
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
}
impl CoralSpyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let (settings, warning) = config::load();
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
            status: "拖动右侧准星，然后瞄准目标窗口或控件。".into(),
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
        };
        let registration = app.desktop.as_ref().map(|service| {
            let handle = cc
                .window_handle()
                .map_err(|e| format!("Cannot access main window: {e}"))?;
            let hwnd = match handle.as_raw() {
                RawWindowHandle::Win32(window) => window.hwnd.get() as usize as u64,
                _ => return Err("Unsupported native window handle".into()),
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
            full_report(info)
        };
        let id = self.id();
        self.exporting = true;
        self.pending_export = Some(Job::Export { id, text, json });
    }
    fn fail_worker(&mut self) {
        self.worker_failed = true;
        self.in_flight = None;
        self.inspecting = false;
        self.reading_details = false;
        self.exporting = false;
        self.pending_inspect = None;
        self.pending_export = None;
        self.pending_detail = None;
        self.notify("后台检查线程已停止，请重新打开程序。", true);
    }
    fn service_worker(&mut self, ctx: &egui::Context) {
        loop {
            match self.worker.receiver.try_recv() {
                Ok(reply) => {
                    if self.in_flight == Some(reply.id()) {
                        self.in_flight = None;
                    }
                    match reply {
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
                                                Err(error) => self
                                                    .content_warning
                                                    .push_str(&format!("\n图标: {error}")),
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
                .or_else(|| self.pending_detail.take());
            if let Some(job) = job {
                let id = job.id();
                match self.worker.sender.try_send(job) {
                    Ok(()) => self.in_flight = Some(id),
                    Err(TrySendError::Full(job)) => match job {
                        Job::Inspect { .. } => self.pending_inspect = Some(job),
                        Job::Details { .. } => self.pending_detail = Some(job),
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
                    picker
                        .error
                        .unwrap_or_else(|| "此位置没有可读取窗口。".into()),
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
                    picker
                        .error
                        .unwrap_or_else(|| "此位置无法采集颜色。".into()),
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
            .ok_or_else(|| "Desktop service unavailable".to_string())
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
        self.hotkeys_tested = false;
        self.settings = self.settings_draft.clone();
        self.configure_desktop();
        self.settings_draft = self.settings.clone();
        self.english = self.settings.language == "en-US";
        self.dark = self.settings.dark;
        self.topmost = self.settings.always_on_top;
        apply_theme(ctx, self.dark);
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
                                "禁用 Disabled "
                            },
                            if entry.checked { "✓ " } else { "" },
                            if entry.submenu {
                                "子菜单 Submenu"
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
                    report.push_str(&format!("\nWARNING: {warning}"));
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
        if self.icons.is_empty() {
            ui.label(self.tr(
                "目标未提供可读取的图标。",
                "No readable icons were provided by the target.",
            ));
            return;
        }
        let mut save = None;
        ui.horizontal_wrapped(|ui| {
            for (i, (icon, texture)) in self.icons.iter().zip(&self.icon_textures).enumerate() {
                ui.vertical(|ui| {
                    if ui
                        .add(
                            egui::Image::new(texture)
                                .fit_to_exact_size(Vec2::splat(36.0))
                                .sense(Sense::click()),
                        )
                        .on_hover_text(self.tr("保存图标", "Save icon"))
                        .clicked()
                        && !self.exporting
                    {
                        save = Some(i);
                    }
                    ui.small(&icon.kind);
                    if ui
                        .add_enabled(
                            !self.exporting,
                            egui::Button::new(self.tr("保存 ICO", "Save ICO")).small(),
                        )
                        .clicked()
                    {
                        save = Some(i);
                    }
                });
            }
        });
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
                    self.tree_expanded = true;
                    self.rebuild_rows();
                }
            }
        });
        ui.separator();
        if self.detail_tab == 7 {
            self.about_contents(ui);
            return;
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.selected.is_some() && !self.reading_details,
                    egui::Button::new(self.tr("读取详情", "Read details")),
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
                            let report = full_report(&info);
                            let mut report_view = report.as_str();
                            ui.add(
                                egui::TextEdit::multiline(&mut report_view)
                                    .desired_rows(8)
                                    .desired_width(f32::INFINITY),
                            );
                            ui.horizontal(|ui| {
                                if ui.button(self.tr("复制报告", "Copy report")).clicked() {
                                    ui.ctx().copy_text(full_report(&info));
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
                ui.small(self.tr("导出为文本 RTF；不保留原控件样式或嵌入对象。","Exports text-only RTF; original styling and embedded objects are not preserved."));
                let mut text = self.content_text.as_str();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(18)
                        .desired_width(f32::INFINITY),
                );
                self.content_actions(ui);
            }
            6 => {
                self.content_table(ui, true);
            }
            _ => {}
        }
    }
    fn content_actions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(self.tr("复制全部", "Copy all")).clicked() {
                ui.ctx().copy_text(self.content_text.clone());
            }
            if ui.button(self.tr("保存全部", "Save all")).clicked() {
                if self.detail_tab == 5 {
                    self.save_named(
                        coralspynext::formats::text_to_rtf(&self.content_text),
                        "rtf",
                    );
                } else {
                    self.save_string(self.content_text.clone());
                }
            }
            if ui
                .add_enabled(
                    self.selected_row.is_some(),
                    egui::Button::new(self.tr("复制当前", "Copy current")),
                )
                .clicked()
            {
                if let Some(row) = self.selected_row.and_then(|i| self.content_rows.get(i)) {
                    ui.ctx()
                        .copy_text(format!("{}\t{}\t{}", row.1, row.2, row.3));
                }
            }
            if ui
                .add_enabled(
                    self.selected_row.is_some(),
                    egui::Button::new(self.tr("保存当前", "Save current")),
                )
                .clicked()
            {
                if let Some(row) = self.selected_row.and_then(|i| self.content_rows.get(i)) {
                    self.save_string(format!("{}\t{}\t{}", row.1, row.2, row.3));
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
                        .content_snapshot
                        .as_ref()
                        .map(coralspynext::content_view::list_headers)
                        .unwrap_or_default();
                    let structured_cells = self
                        .content_snapshot
                        .as_ref()
                        .map(coralspynext::content_view::list_cells)
                        .unwrap_or_default();
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
                    ui.label(self.tr("暂无数据。点击“读取详情”。", "No data. Click Read details."));
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
                                        "[受保护 / Protected]"
                                    } else {
                                        &form.name
                                    });
                                    let value = if form.protected {
                                        "[密码受保护 / Password protected]"
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
            color_field(ui, "Red", &r.to_string(), 39.0);
            color_field(ui, "Green", &g.to_string(), 39.0);
            color_field(ui, "Blue", &b.to_string(), 39.0);
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
                    "启用热键 / Enable hotkeys",
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
                ui.checkbox(&mut self.settings_draft.dark, "Dark / 深色");
                ui.checkbox(&mut self.settings_draft.always_on_top, "置顶 / On top");
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.settings_draft.tray_enabled, "托盘 / Tray");
                ui.checkbox(
                    &mut self.settings_draft.minimize_to_tray,
                    "最小化到托盘 / Minimize to tray",
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
                ui.checkbox(&mut self.settings_draft.highlight_bold, "粗体 / Bold");
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

impl eframe::App for CoralSpyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.service_worker(ctx);
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
    response.on_hover_text(format!("{label}: {value}\nCtrl+C 复制 / copy"));
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
fn style_names(style: u32, ex_style: u32) -> String {
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
        "未匹配常见样式标志；完整值见上方。".into()
    } else {
        names.join("  ·  ")
    }
}
fn full_report(info: &WindowInfo) -> String {
    format!(
        "CoralSpyNext — 窗口检查快照\n\n窗口句柄: {}\n父窗口句柄: {}\n标题: {}\n类名: {}\n进程名称: {}\nPID: {}\nTID: {}\n\n窗口边界: ({}, {}) → ({}, {})\n窗口大小: {} × {} px\n客户区边界: ({}, {}) → ({}, {})\n客户区大小: {} × {} px\nDPI: {}\n\n可见: {}\n启用: {}\n最小化: {}\n最大化: {}\nUnicode: {}\nStyle: 0x{:08X}\nExStyle: 0x{:08X}\n常见标志: {}\n\n读取状态: {}\n\n仅本机窗口元数据，不读取密码或输入框内容。\n",
        hwnd_text(info.hwnd), hwnd_text(info.parent), info.title, info.class_name, info.process_name, info.pid, info.tid,
        info.rect.left, info.rect.top, info.rect.right, info.rect.bottom, info.rect.width(), info.rect.height(),
        info.client_rect.left, info.client_rect.top, info.client_rect.right, info.client_rect.bottom, info.client_rect.width(), info.client_rect.height(), info.dpi,
        info.visible, info.enabled, info.minimized, info.maximized, info.is_unicode, info.style, info.ex_style, style_names(info.style, info.ex_style), info.text_status,
    )
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
