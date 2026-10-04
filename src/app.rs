//! The native UI. All potentially slow window queries run on one bounded worker.
//! Global key state is read only during a picker explicitly started by the user.
use std::{
    collections::HashSet,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    thread,
    time::{Duration, Instant},
};

use coralspynext::{
    model::{hwnd_text, matches_filter, ColorSample, WindowInfo, WindowNode},
    platform,
};
use eframe::egui::{
    self, Align, Align2, Color32, FontFamily, FontId, Layout, RichText, Sense, Stroke, Vec2,
};

const ACCENT: Color32 = Color32::from_rgb(248, 139, 113);
const POLL_INTERVAL: Duration = Duration::from_millis(45);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Windows,
    Colors,
    About,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum PickKind {
    Window,
    Color,
}
struct Picker {
    kind: PickKind,
    started: Instant,
    next_poll: Instant,
    armed: bool,
    target: Option<(u64, i32, i32)>,
    sample: Option<ColorSample>,
    error: Option<String>,
}

enum Job {
    Enumerate {
        id: u64,
    },
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
}
impl Job {
    fn id(&self) -> u64 {
        match self {
            Self::Enumerate { id } | Self::Inspect { id, .. } | Self::Export { id, .. } => *id,
        }
    }
}
enum Reply {
    Enumerated {
        id: u64,
        result: Result<Vec<WindowNode>, String>,
        notice: Option<String>,
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
}
impl Reply {
    fn id(&self) -> u64 {
        match self {
            Self::Enumerated { id, .. }
            | Self::Inspected { id, .. }
            | Self::Exported { id, .. } => *id,
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
        // No thread is created on a frame, refresh, or selection. Closing the UI
        // drops the channels and lets the worker exit without blocking shutdown.
        thread::Builder::new()
            .name("coralspynext-inspector".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let reply = match job {
                        Job::Enumerate { id } => {
                            let result = platform::enumerate_windows();
                            let notice = platform::enumeration_notice();
                            Reply::Enumerated { id, result, notice }
                        }
                        Job::Inspect { id, hwnd, expected } => {
                            let result = platform::inspect_window(hwnd).and_then(|info| {
                                if expected.as_ref().is_some_and(|(pid, class_name)| {
                                    *pid != info.pid || *class_name != info.class_name
                                }) {
                                    Err("目标已关闭或句柄被复用，请刷新窗口列表。".into())
                                } else {
                                    Ok(info)
                                }
                            });
                            Reply::Inspected { id, hwnd, result }
                        }
                        Job::Export { id, text, json } => Reply::Exported {
                            id,
                            result: platform::save_text_dialog(&text, json),
                        },
                    };
                    if answers.send(reply).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("Unable to start the window inspection worker");
        Self { sender, receiver }
    }
}

pub struct CoralSpyApp {
    page: Page,
    dark: bool,
    worker: Worker,
    worker_failed: bool,
    next_id: u64,
    in_flight: Option<u64>,
    refresh_id: u64,
    inspect_id: u64,
    pending_refresh: Option<Job>,
    pending_inspect: Option<Job>,
    pending_export: Option<Job>,
    refreshing: bool,
    inspecting: bool,
    exporting: bool,
    nodes: Vec<WindowNode>,
    collapsed: HashSet<u64>,
    filter: String,
    visible_only: bool,
    selected: Option<u64>,
    selected_identity: Option<(u32, String)>,
    info: Option<WindowInfo>,
    inspection_error: Option<String>,
    enumeration_error: Option<String>,
    enumeration_notice: Option<String>,
    last_refresh: Option<Instant>,
    picker: Option<Picker>,
    color: Option<ColorSample>,
    color_history: Vec<ColorSample>,
    status: String,
    status_error: bool,
    focus_search: bool,
}

impl CoralSpyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx, true);
        let mut app = Self {
            page: Page::Windows,
            dark: true,
            worker: Worker::new(cc.egui_ctx.clone()),
            worker_failed: false,
            next_id: 0,
            in_flight: None,
            refresh_id: 0,
            inspect_id: 0,
            pending_refresh: None,
            pending_inspect: None,
            pending_export: None,
            refreshing: false,
            inspecting: false,
            exporting: false,
            nodes: Vec::new(),
            collapsed: HashSet::new(),
            filter: String::new(),
            visible_only: false,
            selected: None,
            selected_identity: None,
            info: None,
            inspection_error: None,
            enumeration_error: None,
            enumeration_notice: None,
            last_refresh: None,
            picker: None,
            color: None,
            color_history: Vec::new(),
            status: "就绪 · 在本机读取窗口元数据".into(),
            status_error: false,
            focus_search: false,
        };
        app.refresh();
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
    fn refresh(&mut self) {
        if self.worker_failed {
            return;
        }
        let id = self.id();
        self.refresh_id = id;
        self.pending_refresh = Some(Job::Enumerate { id });
        self.refreshing = true;
        self.enumeration_error = None;
    }
    fn select(&mut self, hwnd: u64) {
        let expected = if self.selected == Some(hwnd) {
            self.selected_identity.clone()
        } else {
            self.nodes
                .iter()
                .find(|node| node.hwnd == hwnd)
                .map(|node| (node.pid, node.class_name.clone()))
        };
        self.inspect_target(hwnd, expected);
    }
    fn inspect_target(&mut self, hwnd: u64, expected: Option<(u32, String)>) {
        if self.worker_failed {
            return;
        }
        let id = self.id();
        self.inspect_id = id;
        self.selected = Some(hwnd);
        self.selected_identity = expected.clone();
        self.info = None;
        self.inspection_error = None;
        self.inspecting = true;
        self.pending_inspect = Some(Job::Inspect { id, hwnd, expected });
        self.page = Page::Windows;
    }
    fn export(&mut self, info: &WindowInfo, json: bool) {
        self.picker = None;
        if self.exporting || self.worker_failed {
            return;
        }
        let text = if json {
            match serde_json::to_string_pretty(info) {
                Ok(value) => value,
                Err(error) => {
                    self.notify(format!("无法生成 JSON：{error}"), true);
                    return;
                }
            }
        } else {
            full_report(info)
        };
        let id = self.id();
        self.pending_export = Some(Job::Export { id, text, json });
        self.exporting = true;
        self.notify("请选择导出文件的保存位置", false);
    }
    fn fail_worker(&mut self) {
        self.worker_failed = true;
        self.in_flight = None;
        self.refreshing = false;
        self.inspecting = false;
        self.exporting = false;
        self.pending_refresh = None;
        self.pending_inspect = None;
        self.pending_export = None;
        self.notify("后台检查线程已停止，请关闭并重新打开 CoralSpyNext。", true);
    }
    fn service_worker(&mut self) {
        loop {
            match self.worker.receiver.try_recv() {
                Ok(reply) => {
                    if self.in_flight == Some(reply.id()) {
                        self.in_flight = None;
                    }
                    match reply {
                        Reply::Enumerated { id, result, notice } if id == self.refresh_id => {
                            self.refreshing = false;
                            match result {
                                Ok(nodes) => {
                                    self.nodes = nodes;
                                    self.enumeration_notice = notice;
                                    self.last_refresh = Some(Instant::now());
                                    let present: HashSet<u64> =
                                        self.nodes.iter().map(|n| n.hwnd).collect();
                                    self.collapsed.retain(|h| present.contains(h));
                                    self.notify(
                                        format!(
                                            "已读取 {} 个窗口 · 点击条目查看详情",
                                            self.nodes.len()
                                        ),
                                        false,
                                    );
                                }
                                Err(error) => {
                                    self.enumeration_error = Some(error.clone());
                                    self.notify(format!("窗口列表刷新失败：{error}"), true);
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
                                    self.notify("已读取目标窗口 · 数据为当前快照", false);
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
                                Ok(Some(path)) => self.notify(format!("已导出：{path}"), false),
                                Ok(None) => self.notify("已取消导出", false),
                                Err(error) => self.notify(format!("导出失败：{error}"), true),
                            }
                        }
                        _ => {} // A newer refresh or selection superseded this reply.
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
                .or_else(|| self.pending_refresh.take());
            if let Some(job) = job {
                let id = job.id();
                match self.worker.sender.try_send(job) {
                    Ok(()) => self.in_flight = Some(id),
                    Err(TrySendError::Full(job)) => match job {
                        Job::Enumerate { .. } => self.pending_refresh = Some(job),
                        Job::Inspect { .. } => self.pending_inspect = Some(job),
                        Job::Export { .. } => self.pending_export = Some(job),
                    },
                    Err(TrySendError::Disconnected(_)) => self.fail_worker(),
                }
            }
        }
    }
    fn begin_pick(&mut self, kind: PickKind) {
        let now = Instant::now();
        self.picker = Some(Picker {
            kind,
            started: now,
            next_poll: now,
            armed: false,
            target: None,
            sample: None,
            error: None,
        });
        self.notify("移动鼠标到目标，按 Ctrl 锁定；按 Esc 取消。", false);
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
        // This is deliberately restricted to Esc and Ctrl, and only
        // exists for the lifetime of an explicitly activated picker.
        if platform::key_down(0x1b) {
            self.picker = None;
            self.notify("已取消拾取", false);
            return;
        }
        let down = platform::key_down(0x11);
        let picker = self.picker.as_mut().expect("picker checked above");
        picker.next_poll = now + POLL_INTERVAL;
        if !picker.armed {
            // Require the lock key to be released before arming, avoiding an accidental capture.
            if !down && now.duration_since(picker.started) >= Duration::from_millis(250) {
                picker.armed = true;
            }
            return;
        }
        match picker.kind {
            PickKind::Window => match platform::cursor_target() {
                Ok(target) => {
                    picker.target = Some(target);
                    picker.error = None;
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
        if down {
            let picker = self.picker.take().expect("picker checked above");
            match picker.kind {
                PickKind::Window => match picker.target {
                    Some((hwnd, _, _)) => match platform::window_identity(hwnd) {
                        Ok(identity) => self.inspect_target(hwnd, Some(identity)),
                        Err(error) => self.notify(error, true),
                    },
                    None => self.notify(
                        picker
                            .error
                            .unwrap_or_else(|| "此位置没有可读取的窗口，请重试。".into()),
                        true,
                    ),
                },
                PickKind::Color => match picker.sample {
                    Some(sample) => {
                        self.color = Some(sample);
                        self.color_history
                            .retain(|c| (c.r, c.g, c.b) != (sample.r, sample.g, sample.b));
                        self.color_history.insert(0, sample);
                        self.color_history.truncate(24);
                        self.notify(
                            format!(
                                "已采集 {} · 屏幕坐标 ({}, {})",
                                sample.hex(),
                                sample.x,
                                sample.y
                            ),
                            false,
                        );
                    }
                    None => self.notify(
                        picker
                            .error
                            .unwrap_or_else(|| "无法读取此位置的屏幕颜色，请重试。".into()),
                        true,
                    ),
                },
            }
        }
    }

    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .exact_height(72.0)
            .frame(
                egui::Frame::default()
                    .fill(surface(self.dark))
                    .inner_margin(egui::Margin::symmetric(22, 14)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    draw_brand(ui, 36.0);
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("CoralSpyNext").size(22.0).strong());
                        ui.label(
                            RichText::new("WINDOW INSPECTOR  /  窗口洞察")
                                .size(10.5)
                                .color(muted(self.dark)),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button(if self.dark {
                                "浅色外观"
                            } else {
                                "深色外观"
                            })
                            .on_hover_text("切换明暗主题")
                            .clicked()
                        {
                            self.dark = !self.dark;
                            apply_theme(ctx, self.dark);
                        }
                        ui.add_space(10.0);
                        pill(ui, "本地运行", self.dark, false);
                    });
                });
            });
    }
    fn sidebar(&mut self, ctx: &egui::Context) {
        let compact = ctx.screen_rect().width() < 960.0;
        egui::SidePanel::left("navigation")
            .resizable(false)
            .exact_width(if compact { 126.0 } else { 164.0 })
            .frame(
                egui::Frame::default()
                    .fill(surface(self.dark))
                    .inner_margin(egui::Margin::symmetric(14, 20)),
            )
            .show(ctx, |ui| {
                ui.label(RichText::new("工作台").size(11.0).color(muted(self.dark)));
                ui.add_space(14.0);
                for (page, name, sub) in [
                    (Page::Windows, "窗口检查", "WINDOWS"),
                    (Page::Colors, "屏幕取色", "COLORS"),
                    (Page::About, "关于与边界", "ABOUT"),
                ] {
                    let selected = self.page == page;
                    let text = format!("{name}\n{sub}");
                    let response = ui.add_sized(
                        [ui.available_width(), 56.0],
                        egui::Button::new(RichText::new(text).size(13.0))
                            .fill(if selected {
                                accent_bg(self.dark)
                            } else {
                                Color32::TRANSPARENT
                            })
                            .stroke(if selected {
                                Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.5))
                            } else {
                                Stroke::NONE
                            }),
                    );
                    if response.clicked() {
                        if self.picker.take().is_some() {
                            self.notify("已取消拾取", false);
                        }
                        self.page = page;
                    }
                    ui.add_space(8.0);
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.label(
                        RichText::new("v0.1.0  ·  x64")
                            .size(11.0)
                            .color(muted(self.dark)),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("只读检查\n默认普通权限")
                            .size(12.0)
                            .color(muted(self.dark)),
                    );
                    ui.add_space(8.0);
                    ui.separator();
                });
            });
    }
    fn footer(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(32.0)
            .frame(
                egui::Frame::default()
                    .fill(surface(self.dark))
                    .inner_margin(egui::Margin::symmetric(18, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.in_flight.is_some() {
                        ui.add(egui::Spinner::new().size(13.0));
                    } else {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                        ui.painter().circle_filled(
                            rect.center(),
                            3.0,
                            if self.status_error {
                                error_color(self.dark)
                            } else {
                                ACCENT
                            },
                        );
                    }
                    ui.add(
                        egui::Label::new(RichText::new(&self.status).size(11.5).color(
                            if self.status_error {
                                error_color(self.dark)
                            } else {
                                muted(self.dark)
                            },
                        ))
                        .truncate(),
                    )
                    .on_hover_text(&self.status);
                });
            });
    }
    fn picker_banner(&self, ui: &mut egui::Ui) {
        let Some(picker) = &self.picker else {
            return;
        };
        egui::Frame::default()
            .fill(accent_bg(self.dark))
            .stroke(Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.6)))
            .corner_radius(10)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(if picker.kind == PickKind::Window {
                        "正在拾取窗口"
                    } else {
                        "正在采集屏幕颜色"
                    })
                    .strong()
                    .color(accent_text(self.dark)),
                );
                ui.label("将鼠标移到目标位置，按 Ctrl 锁定；按 Esc 取消。");
                ui.label(
                    RichText::new("无需点击目标应用，拾取不会模拟或拦截任何输入。")
                        .size(12.0)
                        .color(muted(self.dark)),
                );
                if !picker.armed {
                    ui.small("请先松开 Ctrl 键…");
                }
                if let Some((hwnd, x, y)) = picker.target {
                    ui.monospace(format!("{}    X {x}  Y {y}", hwnd_text(hwnd)));
                }
                if let Some(sample) = picker.sample {
                    ui.monospace(format!(
                        "{}    {}    X {}  Y {}",
                        sample.hex(),
                        sample.rgb(),
                        sample.x,
                        sample.y
                    ));
                }
                if let Some(error) = &picker.error {
                    ui.colored_label(error_color(self.dark), error);
                }
            });
        ui.add_space(14.0);
    }
    fn windows_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("窗口检查");
                ui.label(
                    RichText::new("从窗口树选择，或直接拾取屏幕上的目标。 ")
                        .size(12.5)
                        .color(muted(self.dark)),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        self.picker.is_none() && !self.worker_failed,
                        primary_button("＋  拾取窗口"),
                    )
                    .clicked()
                {
                    self.begin_pick(PickKind::Window);
                }
                if ui
                    .add_enabled(
                        !self.refreshing && !self.worker_failed,
                        egui::Button::new("刷新  F5"),
                    )
                    .clicked()
                {
                    self.refresh();
                }
            });
        });
        ui.add_space(16.0);
        self.picker_banner(ui);
        let width = ui.available_width();
        let tree_width = (width * 0.38).clamp(245.0, 355.0);
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(tree_width, height),
                Layout::top_down(Align::LEFT),
                |ui| {
                    ui.set_width(tree_width);
                    ui.set_min_height(height);
                    self.window_tree(ui);
                },
            );
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(
                Vec2::new((width - tree_width - 34.0).max(240.0), height),
                Layout::top_down(Align::LEFT),
                |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .id_salt("window_details")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.window_details(ui));
                },
            );
        });
    }
    fn window_tree(&mut self, ui: &mut egui::Ui) {
        let response = ui.add_sized(
            [ui.available_width(), 34.0],
            egui::TextEdit::singleline(&mut self.filter).hint_text("搜索标题、类名、PID、HWND"),
        );
        if self.focus_search {
            response.request_focus();
            self.focus_search = false;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.visible_only, "仅可见");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("展开")
                    .on_hover_text("展开所有层级")
                    .clicked()
                {
                    self.collapsed.clear();
                }
                if ui
                    .small_button("折叠")
                    .on_hover_text("仅显示顶层窗口")
                    .clicked()
                {
                    self.collapsed = self.nodes.iter().map(|n| n.hwnd).collect();
                }
            });
        });
        ui.add_space(6.0);
        if let Some(error) = &self.enumeration_error {
            ui.colored_label(error_color(self.dark), error);
        }
        if let Some(notice) = &self.enumeration_notice {
            ui.label(
                RichText::new(notice)
                    .size(11.0)
                    .color(accent_text(self.dark)),
            );
        }
        let parents: HashSet<u64> = self
            .nodes
            .iter()
            .filter(|n| n.parent != 0)
            .map(|n| n.parent)
            .collect();
        let searching = !self.filter.trim().is_empty();
        let mut hidden_depth = None;
        let visible: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                if !searching {
                    if let Some(depth) = hidden_depth {
                        if node.depth > depth {
                            return None;
                        }
                        hidden_depth = None;
                    }
                    if self.collapsed.contains(&node.hwnd) {
                        hidden_depth = Some(node.depth);
                    }
                }
                (matches_filter(node, &self.filter) && (!self.visible_only || node.visible))
                    .then_some(index)
            })
            .collect();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} 项 / 共 {} 个窗口",
                    visible.len(),
                    self.nodes.len()
                ))
                .size(11.0)
                .color(muted(self.dark)),
            );
            if self.refreshing {
                ui.add(egui::Spinner::new().size(12.0));
            }
        });
        ui.add_space(6.0);
        if visible.is_empty() {
            ui.add_space(35.0);
            ui.label(if self.refreshing {
                "正在读取窗口列表…"
            } else if self.nodes.is_empty() {
                "暂无可读取的窗口。点击刷新重试。"
            } else {
                "没有匹配项，试试更短的关键词。"
            });
        }
        let mut selection = None;
        let mut collapse_toggle = None;
        egui::ScrollArea::vertical()
            .id_salt("window_tree")
            .auto_shrink([false, false])
            .show_rows(ui, 54.0, visible.len(), |ui, range| {
                for row in range {
                    let node = &self.nodes[visible[row]];
                    let (rect, response) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 54.0), Sense::click());
                    let selected = self.selected == Some(node.hwnd);
                    let fill = if selected {
                        accent_bg(self.dark)
                    } else if response.hovered() {
                        hover_bg(self.dark)
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter()
                        .rect_filled(rect.shrink2(Vec2::new(0.0, 2.0)), 7.0, fill);
                    if selected {
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                rect.min + Vec2::new(0.0, 10.0),
                                Vec2::new(3.0, 34.0),
                            ),
                            2.0,
                            ACCENT,
                        );
                    }
                    let indentation = (node.depth.min(4) as f32) * 11.0;
                    let x = rect.left() + 9.0 + indentation;
                    let disclosure = egui::Rect::from_min_size(
                        egui::pos2(x, rect.top() + 14.0),
                        Vec2::new(15.0, 24.0),
                    );
                    if parents.contains(&node.hwnd) {
                        let arrow = if self.collapsed.contains(&node.hwnd) && !searching {
                            "+"
                        } else {
                            "−"
                        };
                        ui.painter().text(
                            disclosure.center(),
                            Align2::CENTER_CENTER,
                            arrow,
                            FontId::monospace(15.0),
                            muted(self.dark),
                        );
                        if ui
                            .interact(
                                disclosure,
                                ui.id().with(("collapse", node.hwnd)),
                                Sense::click(),
                            )
                            .clicked()
                        {
                            collapse_toggle = Some(node.hwnd);
                        }
                    }
                    let painter = ui.painter().with_clip_rect(egui::Rect::from_min_max(
                        egui::pos2(x + 21.0, rect.top()),
                        rect.max - Vec2::new(8.0, 0.0),
                    ));
                    let title = if node.title.trim().is_empty() {
                        "（无窗口标题）"
                    } else {
                        node.title.trim()
                    };
                    painter.text(
                        egui::pos2(x + 21.0, rect.top() + 8.0),
                        Align2::LEFT_TOP,
                        title,
                        FontId::proportional(13.0),
                        if node.visible {
                            ui.visuals().text_color()
                        } else {
                            muted(self.dark)
                        },
                    );
                    painter.text(
                        egui::pos2(x + 21.0, rect.top() + 29.0),
                        Align2::LEFT_TOP,
                        format!("{}  ·  PID {}", node.class_name, node.pid),
                        FontId::proportional(10.5),
                        muted(self.dark),
                    );
                    if response.clicked()
                        && !disclosure
                            .contains(response.interact_pointer_pos().unwrap_or(egui::Pos2::ZERO))
                    {
                        selection = Some(node.hwnd);
                    }
                    response.on_hover_text(format!(
                        "{}\n{}\n{}\nPID {} · {}",
                        title,
                        node.class_name,
                        hwnd_text(node.hwnd),
                        node.pid,
                        if node.visible { "可见" } else { "隐藏" }
                    ));
                }
            });
        if let Some(hwnd) = collapse_toggle {
            if !self.collapsed.remove(&hwnd) {
                self.collapsed.insert(hwnd);
            }
        }
        if let Some(hwnd) = selection {
            // An explicit click uses the latest tree snapshot, even if its HWND
            // equals a stale prior selection. Recheck keeps the old identity.
            let expected = self
                .nodes
                .iter()
                .find(|node| node.hwnd == hwnd)
                .map(|node| (node.pid, node.class_name.clone()));
            self.inspect_target(hwnd, expected);
        }
    }
    fn window_details(&mut self, ui: &mut egui::Ui) {
        if self.inspecting {
            ui.add_space(70.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("正在检查目标窗口…");
            });
            if let Some(hwnd) = self.selected {
                ui.monospace(hwnd_text(hwnd));
            }
            return;
        }
        if let Some(error) = self.inspection_error.clone() {
            ui.add_space(30.0);
            ui.heading("暂时无法读取");
            ui.add_space(10.0);
            ui.colored_label(error_color(self.dark), error);
            ui.add_space(8.0);
            ui.label("目标可能已关闭，或受到 Windows 权限与应用保护限制。可重新拾取其他窗口。");
            if ui.button("重试所选窗口").clicked() {
                if let Some(hwnd) = self.selected {
                    self.select(hwnd);
                }
            }
            return;
        }
        let Some(info) = self.info.clone() else {
            ui.add_space(70.0);
            draw_empty_window(ui, self.dark);
            ui.add_space(18.0);
            ui.heading("选择一个窗口，开始洞察");
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "在左侧浏览窗口树，或点击「拾取窗口」\n读取句柄、进程、位置与样式信息。",
                )
                .color(muted(self.dark)),
            );
            ui.add_space(20.0);
            ui.label(
                RichText::new("Ctrl + F  搜索窗口     F5  刷新列表")
                    .size(12.0)
                    .color(muted(self.dark)),
            );
            return;
        };
        ui.label(
            RichText::new("所选窗口 / SNAPSHOT")
                .size(10.5)
                .color(accent_text(self.dark)),
        );
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(
                RichText::new(nonempty(&info.title, "无窗口标题"))
                    .size(21.0)
                    .strong(),
            )
            .wrap(),
        );
        ui.label(
            RichText::new(nonempty(&info.class_name, "未知窗口类"))
                .monospace()
                .color(muted(self.dark)),
        );
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            pill(
                ui,
                if info.visible { "可见" } else { "隐藏" },
                self.dark,
                info.visible,
            );
            pill(
                ui,
                if info.enabled {
                    "已启用"
                } else {
                    "已禁用"
                },
                self.dark,
                false,
            );
            if info.minimized {
                pill(ui, "已最小化", self.dark, false);
            }
            if info.maximized {
                pill(ui, "已最大化", self.dark, false);
            }
            pill(
                ui,
                if info.is_unicode { "Unicode" } else { "ANSI" },
                self.dark,
                false,
            );
        });
        ui.add_space(14.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("复制报告").clicked() {
                ui.ctx().copy_text(full_report(&info));
                self.notify("窗口报告已复制", false);
            }
            if ui
                .add_enabled(!self.exporting, egui::Button::new("导出 JSON"))
                .clicked()
            {
                self.export(&info, true);
            }
            if ui
                .add_enabled(!self.exporting, egui::Button::new("导出文本"))
                .clicked()
            {
                self.export(&info, false);
            }
            if ui.small_button("重新检查").clicked() {
                self.select(info.hwnd);
            }
        });
        section(ui, "标识与进程", self.dark);
        egui::Grid::new("identity_grid")
            .num_columns(2)
            .spacing([14.0, 10.0])
            .striped(false)
            .show(ui, |ui| {
                data_row(ui, "窗口句柄", &hwnd_text(info.hwnd), true, self.dark);
                data_row(ui, "父窗口", &hwnd_text(info.parent), true, self.dark);
                data_row(
                    ui,
                    "进程名称",
                    nonempty(&info.process_name, "不可用 / 无读取权限"),
                    false,
                    self.dark,
                );
                data_row(ui, "进程 PID", &info.pid.to_string(), true, self.dark);
                data_row(ui, "线程 TID", &info.tid.to_string(), true, self.dark);
            });
        section(ui, "尺寸与坐标", self.dark);
        egui::Grid::new("geometry_grid")
            .num_columns(2)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                data_row(
                    ui,
                    "屏幕位置",
                    &format!("X {}    Y {}", info.rect.left, info.rect.top),
                    true,
                    self.dark,
                );
                data_row(
                    ui,
                    "窗口大小",
                    &format!("{} × {} px", info.rect.width(), info.rect.height()),
                    true,
                    self.dark,
                );
                data_row(
                    ui,
                    "窗口边界",
                    &format!(
                        "({}, {}) → ({}, {})",
                        info.rect.left, info.rect.top, info.rect.right, info.rect.bottom
                    ),
                    true,
                    self.dark,
                );
                data_row(
                    ui,
                    "客户区大小",
                    &format!(
                        "{} × {} px",
                        info.client_rect.width(),
                        info.client_rect.height()
                    ),
                    true,
                    self.dark,
                );
                data_row(
                    ui,
                    "客户区边界",
                    &format!(
                        "({}, {}) → ({}, {})",
                        info.client_rect.left,
                        info.client_rect.top,
                        info.client_rect.right,
                        info.client_rect.bottom
                    ),
                    true,
                    self.dark,
                );
                data_row(
                    ui,
                    "窗口 DPI",
                    &if info.dpi == 0 {
                        "不可用".into()
                    } else {
                        format!("{}  ·  {:.0}%", info.dpi, info.dpi as f32 / 96.0 * 100.0)
                    },
                    true,
                    self.dark,
                );
            });
        section(ui, "窗口样式", self.dark);
        egui::Grid::new("style_grid")
            .num_columns(2)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                data_row(
                    ui,
                    "Style",
                    &format!("0x{:08X}", info.style),
                    true,
                    self.dark,
                );
                data_row(
                    ui,
                    "ExStyle",
                    &format!("0x{:08X}", info.ex_style),
                    true,
                    self.dark,
                );
            });
        ui.add_space(9.0);
        ui.label(
            RichText::new(style_names(info.style, info.ex_style))
                .size(11.0)
                .color(muted(self.dark)),
        );
        section(ui, "读取说明", self.dark);
        ui.label(
            RichText::new(&info.text_status)
                .size(12.0)
                .color(muted(self.dark)),
        );
        ui.add_space(7.0);
        ui.label(RichText::new("仅显示系统可提供的标题元数据。不会读取密码、输入框内容，不使用钩子或进程注入。坐标使用像素；受保护窗口可能只提供部分字段。").size(11.5).color(muted(self.dark)));
        ui.add_space(18.0);
    }
    fn colors_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("屏幕取色");
                ui.label(
                    RichText::new("捕捉屏幕像素，留下刚刚发现的颜色。 ")
                        .size(12.5)
                        .color(muted(self.dark)),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(self.picker.is_none(), primary_button("＋  开始取色"))
                    .clicked()
                {
                    self.begin_pick(PickKind::Color);
                }
            });
        });
        ui.add_space(20.0);
        self.picker_banner(ui);
        egui::ScrollArea::vertical().id_salt("colors").auto_shrink([false, false]).show(ui, |ui| {
            let sample = self.picker.as_ref().and_then(|p| p.sample).or(self.color);
            egui::Frame::default().fill(surface(self.dark)).corner_radius(14).inner_margin(24).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(sample) = sample {
                    ui.horizontal_top(|ui| {
                        let side = (ui.available_width() * 0.32).clamp(135.0, 220.0);
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
                        ui.painter().rect_filled(rect, 12.0, Color32::from_rgb(sample.r, sample.g, sample.b));
                        ui.add_space(24.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(if self.picker.is_some() { "实时预览" } else { "已采集颜色" }).size(11.0).color(muted(self.dark)));
                            ui.add_space(12.0);
                            ui.label(RichText::new(sample.hex()).size(34.0).monospace().strong());
                            ui.label(RichText::new(sample.rgb()).size(16.0).monospace());
                            ui.add_space(14.0);
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("复制 HEX").clicked() { ui.ctx().copy_text(sample.hex()); self.notify("HEX 颜色值已复制", false); }
                                if ui.button("复制 RGB").clicked() { ui.ctx().copy_text(sample.rgb()); self.notify("RGB 颜色值已复制", false); }
                            });
                            ui.add_space(14.0);
                            ui.label(RichText::new(format!("屏幕坐标  X {}  ·  Y {}", sample.x, sample.y)).size(12.0).color(muted(self.dark)));
                            ui.label(RichText::new(format!("Win32 COLORREF  0x{:08X}", sample.colorref())).size(11.0).monospace().color(muted(self.dark)));
                        });
                    });
                } else {
                    ui.set_min_height(210.0);
                    ui.add_space(35.0); ui.heading("还没有采集颜色"); ui.add_space(12.0);
                    ui.label("点击「开始取色」，移动鼠标到屏幕上的任意位置。");
                    ui.label(RichText::new("按 Ctrl 锁定颜色，Esc 随时取消。 ").color(muted(self.dark)));
                }
            });
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("最近采集").size(16.0).strong());
                ui.label(RichText::new(format!("{} / 24", self.color_history.len())).size(11.0).color(muted(self.dark)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add_enabled(!self.color_history.is_empty(), egui::Button::new("清空历史").small()).clicked() { self.color_history.clear(); }
                });
            });
            ui.add_space(10.0);
            if self.color_history.is_empty() { ui.label(RichText::new("颜色记录仅保存在本次会话中。").size(12.0).color(muted(self.dark))); }
            let mut picked = None;
            ui.horizontal_wrapped(|ui| {
                for sample in &self.color_history {
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(82.0, 84.0), Sense::click());
                    ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(76.0, 53.0)), 8.0, Color32::from_rgb(sample.r, sample.g, sample.b));
                    ui.painter().text(rect.min + Vec2::new(0.0, 60.0), Align2::LEFT_TOP, sample.hex(), FontId::monospace(12.0), ui.visuals().text_color());
                    if response.clicked() { picked = Some(*sample); }
                    response.on_hover_text(format!("{} · ({}, {})\n点击查看", sample.rgb(), sample.x, sample.y));
                }
            });
            if let Some(sample) = picked { self.color = Some(sample); }
            ui.add_space(20.0);
            ui.label(RichText::new("数值来自 Windows 桌面像素采样。HDR、色彩管理与受保护内容可能影响屏幕显示和采样结果。").size(11.5).color(muted(self.dark)));
        });
    }
    fn about_page(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().id_salt("about").show(ui, |ui| {
            ui.add_space(14.0); draw_brand(ui, 58.0); ui.add_space(20.0);
            ui.label(RichText::new("CoralSpyNext").size(32.0).strong());
            ui.label(RichText::new("熟悉的窗口检查，更清晰的现代体验。").size(16.0).color(muted(self.dark)));
            ui.add_space(24.0);
            about_card(ui, "为今天的 Windows 重写", "受经典 CoralSpy 启发的独立实现，采用 Rust、Win32 和 egui。面向 Windows 11 的 x64 桌面，支持高 DPI 与多显示器。", self.dark);
            ui.add_space(12.0);
            about_card(ui, "小而实用的工作台", "窗口树与快捷搜索；鼠标定位目标；句柄、进程、客户区、样式与 DPI 快照；JSON / 文本导出；屏幕取色及本次会话的颜色历史。", self.dark);
            ui.add_space(12.0);
            about_card(ui, "明确的安全边界", "所有检查在本机进行，无需联网。默认以普通权限运行，不自动提权，不安装服务。不读取密码或输入框，不记录键盘，不注入进程，不修改目标窗口。只有启动拾取时才轮询 Esc 和 Ctrl 键。", self.dark);
            ui.add_space(12.0);
            about_card(ui, "哪些内容可能读不到", "已关闭、受保护、更高权限或特殊渲染的窗口，可能无法完整读取。浏览器与现代应用中的某些控件没有独立 HWND；不会将网页 DOM 或绘制元素伪装为窗口。列表是有数量和时间上限的快照，刷新以获取最新状态。", self.dark);
            ui.add_space(24.0);
            ui.label(RichText::new("快捷键").size(15.0).strong()); ui.add_space(8.0);
            ui.label("F5  刷新窗口列表\nCtrl + F  搜索窗口\nCtrl  锁定正在拾取的目标\nEsc  取消正在进行的拾取");
            ui.add_space(20.0);
            ui.label(RichText::new("CoralSpyNext 0.1.0  ·  Rust  ·  MIT").size(11.0).color(muted(self.dark)));
        });
    }
}

impl eframe::App for CoralSpyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.service_worker();
        self.service_picker(ctx);
        if self.picker.is_none() {
            if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
                self.refresh();
            }
            if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::F)) {
                self.page = Page::Windows;
                self.focus_search = true;
            }
        }
        self.header(ctx);
        self.footer(ctx);
        self.sidebar(ctx);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(background(self.dark))
                    .inner_margin(egui::Margin::same(22)),
            )
            .show(ctx, |ui| match self.page {
                Page::Windows => self.windows_page(ui),
                Page::Colors => self.colors_page(ui),
                Page::About => self.about_page(ui),
            });
        self.service_worker();
        if self.in_flight.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
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
    visuals.panel_fill = background(dark);
    visuals.window_fill = surface(dark);
    visuals.extreme_bg_color = if dark {
        Color32::from_rgb(13, 19, 28)
    } else {
        Color32::from_rgb(249, 250, 252)
    };
    visuals.selection.bg_fill = accent_bg(dark);
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
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(12.0, 8.0);
        style.spacing.interact_size.y = 30.0;
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
fn background(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(16, 22, 32)
    } else {
        Color32::from_rgb(241, 244, 248)
    }
}
fn surface(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(23, 31, 43)
    } else {
        Color32::WHITE
    }
}
fn hover_bg(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(34, 43, 56)
    } else {
        Color32::from_rgb(232, 236, 241)
    }
}
fn accent_text(dark: bool) -> Color32 {
    if dark {
        ACCENT
    } else {
        Color32::from_rgb(145, 57, 32)
    }
}
fn accent_bg(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(63, 43, 40)
    } else {
        Color32::from_rgb(255, 227, 217)
    }
}
fn muted(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(163, 176, 194)
    } else {
        Color32::from_rgb(82, 96, 116)
    }
}
fn error_color(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(255, 155, 152)
    } else {
        Color32::from_rgb(171, 39, 36)
    }
}
fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(
        RichText::new(text)
            .strong()
            .color(Color32::from_rgb(40, 24, 23)),
    )
    .fill(ACCENT)
    .min_size(Vec2::new(110.0, 35.0))
}
fn nonempty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}
fn pill(ui: &mut egui::Ui, text: &str, dark: bool, accent: bool) {
    egui::Frame::default()
        .fill(if accent {
            accent_bg(dark)
        } else {
            hover_bg(dark)
        })
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.5).color(if accent && dark {
                ACCENT
            } else {
                muted(dark)
            }));
        });
}
fn section(ui: &mut egui::Ui, title: &str, dark: bool) {
    ui.add_space(18.0);
    ui.separator();
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong().size(13.0).color(if dark {
        Color32::from_rgb(213, 222, 234)
    } else {
        Color32::from_rgb(46, 61, 81)
    }));
    ui.add_space(8.0);
}
fn data_row(ui: &mut egui::Ui, key: &str, value: &str, mono: bool, dark: bool) {
    ui.label(RichText::new(key).size(12.0).color(muted(dark)));
    let text = if mono {
        RichText::new(value).monospace().size(12.0)
    } else {
        RichText::new(value).size(12.0)
    };
    ui.add(egui::Label::new(text).wrap().selectable(true));
    ui.end_row();
}
fn about_card(ui: &mut egui::Ui, title: &str, text: &str, dark: bool) {
    egui::Frame::default()
        .fill(surface(dark))
        .corner_radius(10)
        .inner_margin(18)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(15.0).strong());
            ui.add_space(7.0);
            ui.label(RichText::new(text).size(13.0).color(muted(dark)));
        });
}
fn draw_brand(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().rect_filled(rect, size * 0.23, ACCENT);
    let ink = Color32::from_rgb(50, 29, 29);
    let inner = rect.shrink(size * 0.25);
    let stroke = Stroke::new(size * 0.07, ink);
    ui.painter()
        .line_segment([inner.left_top(), inner.right_top()], stroke);
    ui.painter()
        .line_segment([inner.left_top(), inner.left_bottom()], stroke);
    ui.painter()
        .line_segment([inner.left_bottom(), inner.right_bottom()], stroke);
    ui.painter()
        .circle_filled(inner.right_center(), size * 0.07, ink);
}
fn draw_empty_window(ui: &mut egui::Ui, dark: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(92.0, 68.0), Sense::hover());
    ui.painter().rect_filled(rect, 10.0, hover_bg(dark));
    ui.painter().line_segment(
        [
            rect.min + Vec2::new(0.0, 20.0),
            rect.min + Vec2::new(92.0, 20.0),
        ],
        Stroke::new(1.0_f32, muted(dark).gamma_multiply(0.4)),
    );
    for x in [12.0, 22.0, 32.0] {
        ui.painter()
            .circle_filled(rect.min + Vec2::new(x, 10.0), 2.0, ACCENT);
    }
    ui.painter().line_segment(
        [
            rect.min + Vec2::new(18.0, 38.0),
            rect.min + Vec2::new(72.0, 38.0),
        ],
        Stroke::new(3.0_f32, muted(dark).gamma_multiply(0.4)),
    );
    ui.painter().line_segment(
        [
            rect.min + Vec2::new(18.0, 49.0),
            rect.min + Vec2::new(55.0, 49.0),
        ],
        Stroke::new(3.0_f32, muted(dark).gamma_multiply(0.25)),
    );
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
