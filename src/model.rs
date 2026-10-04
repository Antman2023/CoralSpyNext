use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl WindowRect {
    pub fn width(&self) -> i32 {
        self.right.saturating_sub(self.left)
    }
    pub fn height(&self) -> i32 {
        self.bottom.saturating_sub(self.top)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WindowInfo {
    pub hwnd: u64,
    pub parent: u64,
    pub pid: u32,
    pub tid: u32,
    pub title: String,
    pub class_name: String,
    pub process_name: String,
    pub rect: WindowRect,
    pub client_rect: WindowRect,
    pub visible: bool,
    pub enabled: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub is_unicode: bool,
    pub style: u32,
    pub ex_style: u32,
    pub dpi: u32,
    pub text_status: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WindowNode {
    pub hwnd: u64,
    pub parent: u64,
    pub depth: usize,
    pub title: String,
    pub class_name: String,
    pub pid: u32,
    pub visible: bool,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColorSample {
    pub x: i32,
    pub y: i32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}
impl ColorSample {
    pub fn hex(&self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
    pub fn rgb(&self) -> String {
        format!("rgb({}, {}, {})", self.r, self.g, self.b)
    }
    pub fn colorref(&self) -> u32 {
        self.r as u32 | (self.g as u32) << 8 | (self.b as u32) << 16
    }
}
pub fn hwnd_text(hwnd: u64) -> String {
    format!("0x{hwnd:016X}")
}
pub fn matches_filter(node: &WindowNode, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || node.title.to_lowercase().contains(&q)
        || node.class_name.to_lowercase().contains(&q)
        || node.pid.to_string().contains(&q)
        || hwnd_text(node.hwnd).to_lowercase().contains(&q)
}
pub fn window_text(info: &WindowInfo) -> String {
    format!("CoralSpyNext — 窗口检查结果\nHWND: {}\n父句柄: {}\n标题: {}\n类型: {}\n进程: {}\nPID: {} / TID: {}\n位置: ({}, {})\n大小: {} × {} px\nDPI: {}\n可见: {} / 启用: {}\nStyle: 0x{:08X}\nExStyle: 0x{:08X}\n文本状态: {}\n", hwnd_text(info.hwnd), hwnd_text(info.parent), info.title, info.class_name, info.process_name, info.pid, info.tid, info.rect.left, info.rect.top, info.rect.width(), info.rect.height(), info.dpi, info.visible, info.enabled, info.style, info.ex_style, info.text_status)
}
