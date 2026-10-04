//! Optional compatibility with an existing MSHTML Internet Explorer_Server.
//!
//! Never creates an IE instance, reinstalls an engine, injects code, reads
//! another process's memory, changes privileges, or bypasses cross-origin rules.
//! All COM providers run in a disposable, eight-second helper process.
//! Password values are never requested. HTML comes from a detached clone and
//! is suppressed unless every input was classified and no password was found.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LegacySnapshot {
    pub hwnd: u64,
    pub location: String,
    pub title: String,
    pub application: String,
    pub source: String,
    /// Names of immediate frames in the top document; indices are zero-based.
    pub frames: Vec<String>,
    pub links: Vec<LegacyLink>,
    pub forms: Vec<LegacyForm>,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LegacyLink {
    pub kind: String,
    pub url: String,
    pub text: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LegacyForm {
    pub kind: String,
    pub name: String,
    pub value: String,
    pub protected: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum LegacyAction {
    Back,
    Forward,
    Stop,
    Refresh,
    Home,
    Highlight,
}
impl LegacyAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Back => "back",
            Self::Forward => "forward",
            Self::Stop => "stop",
            Self::Refresh => "refresh",
            Self::Home => "home",
            Self::Highlight => "highlight",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "back" => Self::Back,
            "forward" => Self::Forward,
            "stop" => Self::Stop,
            "refresh" => Self::Refresh,
            "home" => Self::Home,
            "highlight" => Self::Highlight,
            _ => return None,
        })
    }
}

use std::{
    io::Read,
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    core::{IUnknown, Interface, BSTR, GUID, PCWSTR, VARIANT},
    Win32::{
        Foundation::{LRESULT, WPARAM},
        System::Com::{
            CoInitializeEx, CoUninitialize, IDispatch, IServiceProvider, COINIT_APARTMENTTHREADED,
            DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPPARAMS,
        },
        UI::Accessibility::ObjectFromLresult,
    },
};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    System::Threading::{
        GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, CREATE_NO_WINDOW,
        PROCESS_QUERY_LIMITED_INFORMATION,
    },
    UI::WindowsAndMessaging::{
        GetClassNameW, GetWindowThreadProcessId, IsWindow, RegisterWindowMessageW,
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    },
};

const SOURCE_LIMIT: usize = 1_048_576;
const TEXT_LIMIT: usize = 4_096;
const ITEM_LIMIT: usize = 5_000;
const FRAME_LIMIT: usize = 64;
const WIRE_LIMIT: usize = 16 * 1_048_576;
const WORKER_TIMEOUT: Duration = Duration::from_secs(8);
const WALK_TIMEOUT: Duration = Duration::from_secs(4);
const PROTECTED: &str = "[密码内容受保护，未读取]";
const IID_HTML_DOCUMENT2: GUID = GUID::from_u128(0x332c4425_26cb_11d0_b483_00c04fd90119);
const SID_WEB_BROWSER_APP: GUID = GUID::from_u128(0x0002df05_0000_0000_c000_000000000046);
const SID_TOP_LEVEL_BROWSER: GUID = GUID::from_u128(0x4c96be40_915c_11cf_99d3_00aa004ae837);

#[derive(PartialEq, Eq)]
struct WindowIdentity {
    pid: u32,
    tid: u32,
    class: String,
}
fn identity(hwnd: u64) -> Result<WindowIdentity, String> {
    let value = usize::try_from(hwnd).map_err(|_| "窗口句柄超出指针宽度")?;
    let handle = value as windows_sys::Win32::Foundation::HWND;
    if value == 0 || unsafe { IsWindow(handle) } == 0 {
        return Err("窗口已关闭或句柄无效".into());
    }
    let mut pid = 0;
    let tid = unsafe { GetWindowThreadProcessId(handle, &mut pid) };
    let mut name = [0u16; 256];
    let count = unsafe { GetClassNameW(handle, name.as_mut_ptr(), name.len() as i32) };
    if tid == 0 || pid == 0 || count <= 0 {
        return Err("无法确认所选窗口的身份".into());
    }
    let class = String::from_utf16_lossy(&name[..count as usize]);
    if class != "Internet Explorer_Server" {
        return Err(format!("此窗口的类为 {class}，未暴露旧版 MSHTML 接口。IE / IE2 仅兼容已存在的 Internet Explorer_Server 控件；现代 Edge、Chrome、Firefox 和 WebView2 不支持此通道。可在内容页尝试 UI Automation。"));
    }
    if pid == unsafe { GetCurrentProcessId() } {
        return Err("不检查检查器自身的 HTML 控件".into());
    }
    Ok(WindowIdentity { pid, tid, class })
}

/// Main must dispatch `--legacy-worker <hwnd> <top|zero-based-frame>` before GUI
/// startup and serialize one `Result<LegacySnapshot, String>` to stdout.
pub fn inspect(hwnd: u64, frame: Option<usize>) -> Result<LegacySnapshot, String> {
    let bytes = run_worker(hwnd, frame, None)?;
    let result: Result<LegacySnapshot, String> =
        serde_json::from_slice(&bytes).map_err(|e| format!("MSHTML 检查结果格式无效：{e}"))?;
    let snapshot = result?;
    if snapshot.hwnd != hwnd {
        return Err("MSHTML 检查结果与所选窗口不一致".into());
    }
    Ok(snapshot)
}
/// Call only for an explicit UI action. Main dispatches
/// `--legacy-action-worker <hwnd> <top|frame> <action>` to action_in_process.
pub fn action(hwnd: u64, frame: Option<usize>, requested: LegacyAction) -> Result<String, String> {
    let bytes = run_worker(hwnd, frame, Some(requested))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("MSHTML 操作结果格式无效：{e}"))?
}
fn run_worker(
    hwnd: u64,
    frame: Option<usize>,
    action: Option<LegacyAction>,
) -> Result<Vec<u8>, String> {
    let before = identity(hwnd)?;
    if frame.is_some_and(|index| index >= FRAME_LIMIT) {
        return Err("框架序号超过 64 个框架的检查上限".into());
    }
    let mut args = vec![
        hwnd.to_string(),
        frame.map(|n| n.to_string()).unwrap_or_else(|| "top".into()),
    ];
    if let Some(action) = action {
        args.push(action.as_str().into());
    }
    let bytes = bounded_worker(
        if action.is_some() {
            "--legacy-action-worker"
        } else {
            "--legacy-worker"
        },
        &args,
        None,
        WORKER_TIMEOUT,
    )?;
    if identity(hwnd)? != before {
        return Err("操作期间所选窗口已改变，请重新选取。若已请求导航，请先查看目标窗口。".into());
    }
    Ok(bytes)
}
fn bounded_worker(
    flag: &str,
    args: &[String],
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let exe = std::env::current_exe().map_err(|e| format!("无法定位检查程序：{e}"))?;
    let mut child = Command::new(exe)
        .arg(flag)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("无法启动辅助进程：{e}"))?;
    // All legacy helpers share this spawn path. The job remains owned until
    // after kill/wait/reap, and Windows terminates it if the GUI exits first.
    let _job_guard = match crate::helper_guard::bind_child(&child) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    // Stdin is written concurrently: a mis-dispatched or hung child must not
    // block before the parent starts enforcing its hard deadline.
    let writer = if let Some(input) = input {
        if input.len() > 65536 {
            let _ = child.kill();
            let _ = child.wait();
            return Err("辅助请求过大".into());
        }
        let Some(mut stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("无法写入辅助进程".into());
        };
        let input = input.to_vec();
        match thread::Builder::new()
            .name("legacy-request-writer".into())
            .spawn(move || stdin.write_all(&input))
        {
            Ok(writer) => Some(writer),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("创建请求线程失败：{e}"));
            }
        }
    } else {
        None
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        return Err("无法读取辅助进程输出".into());
    };
    let overflow = Arc::new(AtomicBool::new(false));
    let over = Arc::clone(&overflow);
    let reader = match thread::Builder::new()
        .name("legacy-result-reader".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .by_ref()
                .take((WIRE_LIMIT + 1) as u64)
                .read_to_end(&mut bytes);
            if bytes.len() > WIRE_LIMIT {
                over.store(true, Ordering::Release);
            }
            result
                .map(|_| bytes)
                .map_err(|e| format!("读取结果失败：{e}"))
        }) {
        Ok(reader) => reader,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(writer) = writer {
                let _ = writer.join();
            }
            return Err(format!("创建读取线程失败：{e}"));
        }
    };
    let started = Instant::now();
    let status = loop {
        if overflow.load(Ordering::Acquire) {
            break Err("返回结果超过安全大小上限".to_owned());
        }
        if started.elapsed() >= timeout {
            let detail = if flag == "--legacy-download-worker" {
                "下载已停止，未替换目标文件；临时片段将被清理。"
            } else {
                "若请求了导航或高亮，可能已有部分变化；请先检查目标窗口。"
            };
            break Err(format!(
                "辅助进程在 {} 秒内未完成，已停止。{detail}",
                timeout.as_secs()
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => break Err(format!("等待进程失败：{e}")),
        }
    };
    if status.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let write_result = writer.map(|writer| writer.join());
    let bytes = reader.join().map_err(|_| "输出读取线程异常")?;
    let status = status?;
    if let Some(result) = write_result {
        result
            .map_err(|_| "请求写入线程异常")?
            .map_err(|e| format!("写入请求失败：{e}"))?;
    }
    if !status.success() {
        return Err(format!("辅助进程异常退出：{status}"));
    }
    let bytes = bytes?;
    if bytes.len() > WIRE_LIMIT {
        return Err("结果超过安全大小上限".into());
    }
    Ok(bytes)
}

struct Apartment;
impl Apartment {
    fn new() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|e| format!("无法初始化 MSHTML COM STA：{e}"))?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

fn document(hwnd: u64) -> Result<IDispatch, String> {
    identity(hwnd)?;
    let message_name: Vec<u16> = "WM_HTML_GETOBJECT\0".encode_utf16().collect();
    let message = unsafe { RegisterWindowMessageW(message_name.as_ptr()) };
    if message == 0 {
        return Err("无法注册旧版 MSHTML 查询消息".into());
    }
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd as usize as _,
            message,
            0,
            0,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            1200,
            &mut result,
        )
    };
    if ok == 0 || result == 0 {
        return Err("所选 MSHTML 控件未返回文档接口。可能不支持此历史兼容通道、未加载文档、已挂起或拒绝访问；不会绕过权限。".into());
    }
    let mut raw = std::ptr::null_mut();
    unsafe {
        ObjectFromLresult(
            LRESULT(result as isize),
            &IID_HTML_DOCUMENT2,
            WPARAM(0),
            &mut raw,
        )
    }
    .map_err(|e| format!("无法取得 MSHTML 文档接口：{e}"))?;
    if raw.is_null() {
        return Err("MSHTML 返回了空文档接口".into());
    }
    // Every COM interface starts with IUnknown. Obtain a real IDispatch via QI
    // rather than treating an arbitrary pointer as an undocumented DOM vtable.
    let unknown = unsafe { IUnknown::from_raw(raw) };
    unknown
        .cast::<IDispatch>()
        .map_err(|e| format!("文档没有 IDispatch 接口：{e}"))
}
fn invoke(
    object: &IDispatch,
    member: &str,
    flags: DISPATCH_FLAGS,
    args: Vec<VARIANT>,
) -> Result<VARIANT, String> {
    let name: Vec<u16> = member.encode_utf16().chain(Some(0)).collect();
    let name_ptr = PCWSTR(name.as_ptr());
    let mut id = 0;
    let empty = GUID::zeroed();
    unsafe { object.GetIDsOfNames(&empty, &name_ptr, 1, 0, &mut id) }
        .map_err(|e| format!("{member} 不可用（{}）", e.code()))?;
    let mut args: Vec<VARIANT> = args.into_iter().rev().collect();
    let params = DISPPARAMS {
        rgvarg: args.as_mut_ptr(),
        cArgs: args.len() as u32,
        ..Default::default()
    };
    let mut output = VARIANT::new();
    // Do not include provider exception strings: they can contain page content.
    unsafe { object.Invoke(id, &empty, 0, flags, &params, Some(&mut output), None, None) }
        .map_err(|e| format!("{member} 读取或操作失败（{}）", e.code()))?;
    Ok(output)
}
fn get(object: &IDispatch, name: &str) -> Result<VARIANT, String> {
    invoke(object, name, DISPATCH_PROPERTYGET, vec![])
}
fn method(object: &IDispatch, name: &str, args: Vec<VARIANT>) -> Result<VARIANT, String> {
    invoke(object, name, DISPATCH_METHOD, args)
}
fn dispatch(value: &VARIANT) -> Result<IDispatch, String> {
    unsafe {
        let raw = &value.as_raw().Anonymous.Anonymous;
        if raw.vt == 9 {
            // VT_DISPATCH; borrowed pointer is cloned before variant drops.
            return IDispatch::from_raw_borrowed(&raw.Anonymous.pdispVal)
                .cloned()
                .ok_or_else(|| "DOM 返回空对象".into());
        }
        if raw.vt == 13 {
            // VT_UNKNOWN
            return IUnknown::from_raw_borrowed(&raw.Anonymous.punkVal)
                .ok_or("DOM 返回空对象")?
                .cast()
                .map_err(|e| format!("DOM 对象无 IDispatch：{}", e.code()));
        }
    }
    Err("DOM 返回值不是对象".into())
}
fn object(object: &IDispatch, name: &str) -> Result<IDispatch, String> {
    dispatch(&get(object, name)?)
}
fn string_value(value: &VARIANT, limit: usize) -> Result<String, String> {
    // Avoid VariantChangeType on VT_DISPATCH: conversion can invoke arbitrary
    // default members. Only a genuine BSTR is eligible as a text result.
    if unsafe { value.as_raw().Anonymous.Anonymous.vt } != 8 {
        return Err("DOM 返回值不是文本".into());
    }
    let value = BSTR::try_from(value).map_err(|_| "无法读取 DOM 文本")?;
    let text: String = char::decode_utf16(value.as_wide().iter().copied().take(limit))
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    Ok(truncate_utf8(text, limit))
}
fn text(object: &IDispatch, name: &str) -> Result<String, String> {
    string_value(&get(object, name)?, TEXT_LIMIT)
}
// Never return a truncated address that a later Download button could treat
// as a different valid resource. Oversized URLs are omitted with a warning.
fn url_text(object: &IDispatch, name: &str) -> Result<String, String> {
    let value = get(object, name)?;
    if unsafe { value.as_raw().Anonymous.Anonymous.vt } != 8 {
        return Err("资源地址不是文本".into());
    }
    let value = BSTR::try_from(&value).map_err(|_| "无法读取资源地址")?;
    if value.len() > 8192 {
        return Err("资源地址超过 8192 字符，已跳过以避免截断后下载错误地址".into());
    }
    let text = String::from_utf16_lossy(value.as_wide());
    if text.len() > 8192 {
        return Err("资源地址超过 8192 字节，已跳过以避免截断后下载错误地址".into());
    }
    Ok(text)
}

fn count(object: &IDispatch) -> Result<usize, String> {
    let value = get(object, "length")?;
    if !matches!(
        unsafe { value.as_raw().Anonymous.Anonymous.vt },
        2 | 3 | 17 | 18 | 19 | 22 | 23
    ) {
        return Err("DOM 集合长度不是整数".into());
    }
    let value = i32::try_from(&value).map_err(|_| "DOM 集合长度无效")?;
    usize::try_from(value).map_err(|_| "DOM 集合长度无效".into())
}
fn item(collection: &IDispatch, index: usize) -> Result<IDispatch, String> {
    dispatch(&invoke(
        collection,
        "item",
        DISPATCH_METHOD | DISPATCH_PROPERTYGET,
        vec![VARIANT::from(index as i32)],
    )?)
}
fn tags(root: &IDispatch, tag: &str) -> Result<IDispatch, String> {
    dispatch(&method(
        root,
        "getElementsByTagName",
        vec![VARIANT::from(tag)],
    )?)
}
fn truncate_utf8(mut value: String, limit: usize) -> String {
    if value.len() > limit {
        let mut at = limit;
        while !value.is_char_boundary(at) {
            at -= 1;
        }
        value.truncate(at);
    }
    value
}
fn warning(warnings: &mut Vec<String>, warning: impl Into<String>) {
    if warnings.len() < 32 {
        let warning = warning.into();
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
}
fn property(object: &IDispatch, member: &str, warnings: &mut Vec<String>) -> String {
    match text(object, member) {
        Ok(value) => value,
        Err(error) => {
            warning(warnings, error);
            String::new()
        }
    }
}
fn process_path(hwnd: u64) -> String {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd as usize as _, &mut pid) };
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return String::new();
    }
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) };
    unsafe { CloseHandle(process) };
    if ok == 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&path[..length as usize])
    }
}
fn browser(document: &IDispatch) -> Result<IDispatch, String> {
    let provider = document
        .cast::<IServiceProvider>()
        .or_else(|_| {
            object(document, "parentWindow")
                .map_err(|_| {
                    windows::core::Error::from_hresult(windows::core::HRESULT(0x80004002u32 as i32))
                })?
                .cast()
        })
        .map_err(|_| "宿主未暴露浏览器服务；此操作不可用".to_owned())?;
    if let Ok(browser) = unsafe { provider.QueryService::<IDispatch>(&SID_WEB_BROWSER_APP) } {
        return Ok(browser);
    }
    let top = unsafe { provider.QueryService::<IServiceProvider>(&SID_TOP_LEVEL_BROWSER) }
        .map_err(|_| "宿主未暴露顶层浏览器服务；此操作不可用")?;
    unsafe { top.QueryService::<IDispatch>(&SID_WEB_BROWSER_APP) }
        .map_err(|_| "宿主没有公开 WebBrowser 应用接口；此操作不可用".into())
}
fn selected_document(top: &IDispatch, frame: Option<usize>) -> Result<IDispatch, String> {
    match frame {
        None => Ok(top.clone()),
        Some(index) => {
            if index >= FRAME_LIMIT {
                return Err("框架序号超过检查上限".into());
            }
            let frames = object(top, "frames")?;
            if index >= count(&frames)? {
                return Err("此框架已不存在，请重新检查".into());
            }
            object(&item(&frames, index)?, "document")
                .map_err(|e| format!("无法读取框架 {index}：{e}。跨域和宿主限制不会被绕过。"))
        }
    }
}

/// Runs only inside the disposable helper process.
pub fn inspect_in_process(hwnd: u64, frame: Option<usize>) -> Result<LegacySnapshot, String> {
    let before = identity(hwnd)?;
    let _apartment = Apartment::new()?;
    let top = document(hwnd)?;
    let started = Instant::now();
    let mut snapshot = LegacySnapshot {
        hwnd,
        application: process_path(hwnd),
        ..Default::default()
    };
    warning(&mut snapshot.warnings, "此页使用历史 MSHTML 兼容接口，只适用于宿主已有的 Internet Explorer_Server；不会安装或恢复 IE / Flash。");
    match object(&top, "frames").and_then(|frames| Ok((count(&frames)?, frames))) {
        Ok((length, frames)) => {
            if length > FRAME_LIMIT {
                warning(
                    &mut snapshot.warnings,
                    "仅列出前 64 个直接子框架；不会递归遍历框架。",
                );
            }
            for index in 0..length.min(FRAME_LIMIT) {
                if started.elapsed() >= WALK_TIMEOUT {
                    warning(
                        &mut snapshot.warnings,
                        "框架枚举达到时间上限，列表可能不完整。",
                    );
                    break;
                }
                let name = item(&frames, index)
                    .and_then(|frame| text(&frame, "name"))
                    .unwrap_or_default();
                snapshot.frames.push(if name.is_empty() {
                    format!("框架 {}", index + 1)
                } else {
                    format!("{} · {name}", index + 1)
                });
            }
        }
        Err(error) => warning(&mut snapshot.warnings, format!("无法列出框架：{error}")),
    }
    let selected = selected_document(&top, frame)?;
    snapshot.location = property(&selected, "URL", &mut snapshot.warnings);
    snapshot.title = property(&selected, "title", &mut snapshot.warnings);
    if snapshot.application.is_empty() {
        warning(&mut snapshot.warnings, "无法读取宿主可执行文件路径。");
    }
    // Take a detached deep clone, then classify/read only this inert snapshot.
    // No event handler, script, navigation, DOM insertion, or page mutation is
    // requested. Cloning avoids a live type=password -> type=text scan race.
    let cloned = object(&selected, "documentElement")
        .and_then(|root| dispatch(&method(&root, "cloneNode", vec![VARIANT::from(true)])?));
    match cloned {
        Ok(root) => inspect_clone(&root, &started, &mut snapshot),
        Err(error) => warning(
            &mut snapshot.warnings,
            format!("无法创建安全的脱离 DOM 副本，已跳过源码、链接和表单：{error}"),
        ),
    }
    if identity(hwnd)? != before {
        return Err("检查期间目标窗口身份已改变；结果已丢弃".into());
    }
    Ok(snapshot)
}

/// Failure and an empty type are protected just like an explicit password.
fn input_is_protected(kind: Option<&str>) -> bool {
    kind.is_none_or(|kind| kind.trim().is_empty() || kind.trim().eq_ignore_ascii_case("password"))
}

struct Limits {
    started: Instant,
    text_left: usize,
    stopped: bool,
}
impl Limits {
    fn take(&mut self, value: String) -> String {
        let value = truncate_utf8(value, self.text_left.min(TEXT_LIMIT));
        self.text_left = self.text_left.saturating_sub(value.len());
        value
    }
    fn available(&mut self) -> bool {
        if self.started.elapsed() >= WALK_TIMEOUT || self.text_left == 0 {
            self.stopped = true;
            false
        } else {
            true
        }
    }
}
fn inspect_clone(root: &IDispatch, started: &Instant, snapshot: &mut LegacySnapshot) {
    let mut limits = Limits {
        started: *started,
        text_left: SOURCE_LIMIT,
        stopped: false,
    };
    let mut source_safe = true;
    match tags(root, "input").and_then(|inputs| Ok((count(&inputs)?, inputs))) {
        Ok((length, inputs)) => {
            if length > ITEM_LIMIT {
                source_safe = false;
                warning(
                    &mut snapshot.warnings,
                    "输入控件超过 5000 个；表单已截断，无法完成密码检测，源码已隐藏。",
                );
            }
            for index in 0..length.min(ITEM_LIMIT) {
                if !limits.available() {
                    source_safe = false;
                    break;
                }
                match item(&inputs, index) {
                    Ok(input) => {
                        // Never request value until type is positively known.
                        let kind = text(&input, "type");
                        let protected = input_is_protected(kind.as_deref().ok());
                        if protected {
                            source_safe = false;
                        }
                        let name = limits.take(text(&input, "name").unwrap_or_default());
                        let kind = match kind {
                            Ok(kind) => kind,
                            Err(error) => {
                                warning(&mut snapshot.warnings, error);
                                "unknown".into()
                            }
                        };
                        let value = if protected {
                            PROTECTED.into()
                        } else {
                            match text(&input, "value") {
                                Ok(value) => limits.take(value),
                                Err(error) => {
                                    warning(&mut snapshot.warnings, error);
                                    "[无法读取]".into()
                                }
                            }
                        };
                        snapshot.forms.push(LegacyForm {
                            kind,
                            name,
                            value,
                            protected,
                        });
                    }
                    Err(error) => {
                        source_safe = false;
                        warning(
                            &mut snapshot.warnings,
                            format!("输入控件未能检查，源码已隐藏：{error}"),
                        );
                    }
                }
            }
        }
        Err(error) => {
            source_safe = false;
            warning(
                &mut snapshot.warnings,
                format!("未能确认密码输入状态，源码已隐藏：{error}"),
            );
        }
    }
    // Serialization occurs only after the detached clone was completely
    // classified. A password field (even empty) suppresses the entire source.
    if source_safe && limits.available() {
        match get(root, "outerHTML").and_then(|value| {
            if unsafe { value.as_raw().Anonymous.Anonymous.vt } != 8 {
                return Err("源码不是字符串".into());
            }
            let source = BSTR::try_from(&value).map_err(|_| "无法读取 HTML 字符串")?;
            let units = source.len();
            let text: String =
                char::decode_utf16(source.as_wide().iter().copied().take(SOURCE_LIMIT))
                    .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect();
            let truncated = units > SOURCE_LIMIT || text.len() > SOURCE_LIMIT;
            Ok((truncate_utf8(text, SOURCE_LIMIT), truncated))
        }) {
            Ok((source, truncated)) => {
                snapshot.source = source;
                if truncated {
                    warning(&mut snapshot.warnings, "HTML 源码已截断至 1 MiB。");
                }
            }
            Err(error) => warning(
                &mut snapshot.warnings,
                format!("无法读取 HTML 源码：{error}"),
            ),
        }
    } else {
        warning(
            &mut snapshot.warnings,
            "检测到密码输入控件，或未能完整确认其状态：整份 HTML 源码已隐藏，密码值从未请求。",
        );
    }
    for tag in ["textarea", "select", "button"] {
        if !limits.available() || snapshot.forms.len() >= ITEM_LIMIT {
            break;
        }
        match tags(root, tag).and_then(|items| Ok((count(&items)?, items))) {
            Ok((length, items)) => {
                let remaining = ITEM_LIMIT - snapshot.forms.len();
                if length > remaining {
                    warning(
                        &mut snapshot.warnings,
                        "表单字段达到 5000 项上限，列表已截断。",
                    );
                }
                for index in 0..length.min(remaining) {
                    if !limits.available() {
                        break;
                    }
                    match item(&items, index) {
                        Ok(field) => {
                            let name = limits.take(text(&field, "name").unwrap_or_default());
                            let value =
                                limits.take(property(&field, "value", &mut snapshot.warnings));
                            snapshot.forms.push(LegacyForm {
                                kind: tag.into(),
                                name,
                                value,
                                protected: false,
                            });
                        }
                        Err(error) => warning(&mut snapshot.warnings, error),
                    }
                }
            }
            Err(error) => warning(
                &mut snapshot.warnings,
                format!("{tag} 表单字段不可用：{error}"),
            ),
        }
    }
    if !snapshot.forms.is_empty() {
        warning(&mut snapshot.warnings, "表单值来自脱离 DOM 的副本；某些旧宿主的克隆只保留默认值，未必保留刚修改的实时值。单个文本限 4096 字节。");
    }
    for (tag, url_property, default_kind) in [
        ("a", "href", "link"),
        ("area", "href", "link"),
        ("img", "src", "image"),
        ("script", "src", "resource"),
        ("link", "href", "resource"),
        ("iframe", "src", "resource"),
        ("embed", "src", "resource"),
        ("object", "data", "resource"),
    ] {
        if !limits.available() || snapshot.links.len() >= ITEM_LIMIT {
            break;
        }
        match tags(root, tag).and_then(|items| Ok((count(&items)?, items))) {
            Ok((length, items)) => {
                let remaining = ITEM_LIMIT - snapshot.links.len();
                if length > remaining {
                    warning(
                        &mut snapshot.warnings,
                        "链接与资源达到 5000 项上限，列表已截断。",
                    );
                }
                for index in 0..length.min(remaining) {
                    if !limits.available() {
                        break;
                    }
                    let element = match item(&items, index) {
                        Ok(element) => element,
                        Err(error) => {
                            warning(&mut snapshot.warnings, error);
                            continue;
                        }
                    };
                    let mut url = match url_text(&element, url_property) {
                        Ok(url) => url,
                        Err(error) => {
                            warning(&mut snapshot.warnings, error);
                            continue;
                        }
                    };
                    if tag == "object" && url.is_empty() {
                        url = flash_movie(&element).unwrap_or_default();
                    }
                    if url.is_empty() {
                        continue;
                    }
                    let mime = if tag == "embed" || tag == "object" {
                        text(&element, "type").unwrap_or_default()
                    } else {
                        String::new()
                    };
                    let kind = if is_flash_url(&url)
                        || mime.eq_ignore_ascii_case("application/x-shockwave-flash")
                    {
                        "flash"
                    } else if default_kind == "link" && is_external(&snapshot.location, &url) {
                        "external"
                    } else {
                        default_kind
                    };
                    let text = if tag == "img" {
                        text(&element, "alt").unwrap_or_default()
                    } else if tag == "a" || tag == "area" {
                        text(&element, "innerText")
                            .or_else(|_| text(&element, "alt"))
                            .unwrap_or_default()
                    } else {
                        format!("<{tag}>")
                    };
                    if url.len() > limits.text_left {
                        limits.stopped = true;
                        break;
                    }
                    limits.text_left -= url.len();
                    let text = limits.take(text);
                    snapshot.links.push(LegacyLink {
                        kind: kind.into(),
                        url,
                        text,
                    });
                }
            }
            Err(error) => warning(
                &mut snapshot.warnings,
                format!("{tag} 资源列表不可用：{error}"),
            ),
        }
    }
    if limits.stopped {
        warning(
            &mut snapshot.warnings,
            "DOM 检查达到 4 秒或文本总量 1 MiB 上限；部分内容未读取。",
        );
    }
    if snapshot.links.iter().any(|link| link.kind == "flash") {
        warning(
            &mut snapshot.warnings,
            "Flash 项仅为页面已有的资源地址；不加载、播放、启用或安装 Flash。",
        );
    }
}
fn flash_movie(element: &IDispatch) -> Result<String, String> {
    let params = tags(element, "param")?;
    for index in 0..count(&params)?.min(64) {
        let param = item(&params, index)?;
        if text(&param, "name")?.eq_ignore_ascii_case("movie") {
            return url_text(&param, "value");
        }
    }
    Ok(String::new())
}
fn is_flash_url(url: &str) -> bool {
    url.split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .ends_with(".swf")
}
/// Normalize the origin of an absolute HTTP(S) DOM URL. Credentials are not
/// part of an origin; default ports disappear and IPv6 stays bracketed.
fn web_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let (scheme, default_port) = if scheme.eq_ignore_ascii_case("http") {
        ("http", 80u16)
    } else if scheme.eq_ignore_ascii_case("https") {
        ("https", 443u16)
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let (host, port_text) = if let Some(ipv6) = authority.strip_prefix('[') {
        let (address, suffix) = ipv6.split_once(']')?;
        let address = address.parse::<std::net::Ipv6Addr>().ok()?;
        let port = if suffix.is_empty() {
            None
        } else {
            Some(suffix.strip_prefix(':')?)
        };
        (format!("[{address}]"), port)
    } else {
        let (host, port) = authority
            .rsplit_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        if host.is_empty()
            || host
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || ":[]\\".contains(c))
        {
            return None;
        }
        (host.to_ascii_lowercase(), port)
    };
    let port = match port_text {
        None | Some("") => default_port,
        Some(port) if port.bytes().all(|c| c.is_ascii_digit()) => port.parse::<u16>().ok()?,
        Some(_) => return None,
    };
    Some(if port == default_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    })
}

fn is_external(document: &str, link: &str) -> bool {
    match (web_origin(document), web_origin(link)) {
        (Some(left), Some(right)) => left != right,
        _ => false,
    }
}

/// Performs only the exact user-requested navigation. It never executes script,
/// changes a security policy, submits a form, or creates a browser instance.
pub fn action_in_process(
    hwnd: u64,
    frame: Option<usize>,
    requested: LegacyAction,
) -> Result<String, String> {
    identity(hwnd)?;
    if matches!(requested, LegacyAction::Highlight) {
        return Err("页面高亮需要文字、前景色、背景色与粗体参数。请使用 IE 页的文字高亮按钮；此无参数导航命令不能执行高亮。".into());
    }
    let _apartment = Apartment::new()?;
    let top = document(hwnd)?;
    let selected = selected_document(&top, frame)?;
    // Prefer host WebBrowser methods for top-document navigation. A failed
    // invocation is never retried through a second route, because it may have
    // already navigated despite returning an error.
    if frame.is_none() {
        if let Ok(browser) = browser(&top) {
            let member = match requested {
                LegacyAction::Back => "GoBack",
                LegacyAction::Forward => "GoForward",
                LegacyAction::Stop => "Stop",
                LegacyAction::Refresh => "Refresh",
                LegacyAction::Home => "GoHome",
                LegacyAction::Highlight => unreachable!(),
            };
            method(&browser, member, vec![])?;
            return Ok(format!(
                "宿主已接受 {member} 请求；页面加载结果请在目标窗口查看。"
            ));
        }
    }
    let window = object(&selected, "parentWindow")?;
    match requested {
        LegacyAction::Back => { method(&object(&window, "history")?, "back", vec![])?; }
        LegacyAction::Forward => { method(&object(&window, "history")?, "forward", vec![])?; }
        LegacyAction::Stop => {
            let accepted = method(&selected, "execCommand", vec![VARIANT::from("Stop"), VARIANT::from(false), VARIANT::new()])?;
            if !true_result(&accepted) { return Err("文档拒绝了 Stop 命令；加载可能仍在继续。".into()); }
        }
        LegacyAction::Refresh => { method(&object(&window, "location")?, "reload", vec![VARIANT::from(false)])?; }
        LegacyAction::Home => return Err("宿主未提供 WebBrowser.GoHome；框架没有独立主页。为避免跳转到猜测的地址，此操作不可用。".into()),
        LegacyAction::Highlight => unreachable!(),
    }
    Ok(format!(
        "文档已接受 {} 请求；请在目标窗口查看结果。",
        requested.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncate_keeps_unicode_boundaries() {
        assert_eq!(truncate_utf8("a中文".into(), 5), "a中");
    }
    #[test]
    fn flash_is_only_a_resource_classification() {
        assert!(is_flash_url("https://example.test/a.SWF?x=1"));
        assert!(!is_flash_url("https://example.test/a.swf.html"));
    }
    #[test]
    fn external_requires_distinct_web_origins() {
        assert!(is_external("https://a.test/a", "https://b.test/a"));
        assert!(!is_external("https://a.test/a", "https://a.test/b"));
        assert!(!is_external("https://a.test", "javascript:alert(1)"));
    }
    #[test]
    fn origins_normalize_default_ports() {
        assert_eq!(
            web_origin("https://a.test/"),
            web_origin("https://a.test:443/")
        );
        assert_eq!(
            web_origin("http://a.test/"),
            web_origin("http://a.test:80/")
        );
        assert_eq!(
            web_origin("HTTPS://A.test:0443/a"),
            Some("https://a.test".into())
        );
        assert!(!is_external("https://a.test/", "https://a.test:443/path"));
        assert!(!is_external("http://a.test/", "http://a.test:80/path"));
        assert!(is_external("https://a.test/", "https://a.test:80/"));
        assert!(is_external("http://a.test/", "http://a.test:443/"));
        assert_eq!(
            web_origin("https://a.test:8443/"),
            Some("https://a.test:8443".into())
        );
    }
    #[test]
    fn origins_preserve_ipv6_and_nondefault_ports() {
        assert_eq!(
            web_origin("https://[2001:db8::1]:443/a"),
            Some("https://[2001:db8::1]".into())
        );
        assert_eq!(
            web_origin("http://[::1]:8080/"),
            Some("http://[::1]:8080".into())
        );
        assert_eq!(
            web_origin("http://[0:0:0:0:0:0:0:1]:80/"),
            web_origin("http://[::1]/")
        );
        assert!(!is_external("https://[::1]/", "https://[::1]:443/"));
        assert!(is_external("https://[::1]/", "https://[::1]:444/"));
        assert_eq!(
            web_origin("https://user:pass@a.test:443/a"),
            Some("https://a.test".into())
        );
        for invalid in [
            "https://[::1]oops/",
            "https://::1/",
            "https://a.test:65536/",
            "https://a.test:abc/",
            "https:///empty",
        ] {
            assert!(web_origin(invalid).is_none(), "{invalid}");
        }
    }
    #[test]
    fn password_and_unknown_inputs_fail_closed() {
        assert!(input_is_protected(None));
        assert!(input_is_protected(Some("")));
        assert!(input_is_protected(Some("password")));
        assert!(input_is_protected(Some("PASSWORD")));
        assert!(!input_is_protected(Some("text")));
        assert!(!input_is_protected(Some("hidden")));
    }
    #[test]
    fn unsafe_asset_extensions_are_rejected() {
        for name in [
            "a.exe", "A.EXE. ", "test.ps1", "foo.js", "a.url", "z.lnk", "a.msi",
        ] {
            assert!(unsafe_extension(name), "{name}");
        }
        for name in ["a.png", "a.css", "a.html", "clip.swf", "a.pdf", "a.bin"] {
            assert!(!unsafe_extension(name), "{name}");
        }
        assert!(unsafe_extension(&decode_url_ascii("/payload%2eexe")));
    }
    #[test]
    fn asset_names_cannot_choose_paths_or_scripts() {
        assert_eq!(asset_name("/path/picture.png"), "picture.png");
        assert_eq!(asset_name("/path/evil%2fnext.png"), "resource.bin");
        assert_eq!(asset_name("/path/run.exe"), "resource.bin");
        assert_eq!(asset_name("/"), "resource.bin");
    }
    #[test]
    fn highlight_input_is_bounded() {
        assert!(validate_needle("").is_err());
        assert!(validate_needle(" ").is_err());
        assert!(validate_needle("a\0b").is_err());
        assert!(validate_needle(&"a".repeat(1025)).is_err());
        assert!(validate_needle("正文").is_ok());
    }
    #[test]
    fn command_names_round_trip() {
        for action in [
            LegacyAction::Back,
            LegacyAction::Forward,
            LegacyAction::Stop,
            LegacyAction::Refresh,
            LegacyAction::Home,
            LegacyAction::Highlight,
        ] {
            assert_eq!(
                LegacyAction::parse(action.as_str()).unwrap().as_str(),
                action.as_str()
            );
        }
        assert!(LegacyAction::parse("execScript").is_none());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyHighlightRequest {
    pub needle: String,
    pub text: [u8; 3],
    pub background: [u8; 3],
    pub bold: bool,
}
/// Explicit user action. Formatting changes stay in the current page until its
/// normal undo/reload; no script is injected and no file is saved or submitted.
pub fn highlight(
    hwnd: u64,
    frame: Option<usize>,
    needle: &str,
    text: [u8; 3],
    background: [u8; 3],
    bold: bool,
) -> Result<String, String> {
    validate_needle(needle)?;
    let before = identity(hwnd)?;
    let request = LegacyHighlightRequest {
        needle: needle.into(),
        text,
        background,
        bold,
    };
    let input = serde_json::to_vec(&request).map_err(|e| format!("无法序列化高亮请求：{e}"))?;
    let args = [
        hwnd.to_string(),
        frame.map(|n| n.to_string()).unwrap_or_else(|| "top".into()),
    ];
    let bytes = bounded_worker(
        "--legacy-highlight-worker",
        &args,
        Some(&input),
        WORKER_TIMEOUT,
    )?;
    if identity(hwnd)? != before {
        return Err("高亮期间窗口已改变；请先检查目标窗口。".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("高亮结果格式无效：{e}"))?
}
fn validate_needle(needle: &str) -> Result<(), String> {
    if needle.trim().is_empty() {
        return Err("请先输入要高亮的文字".into());
    }
    if needle.len() > 1024 || needle.contains('\0') {
        return Err("高亮文字最多 1024 字节，且不能包含空字符".into());
    }
    Ok(())
}
fn true_result(value: &VARIANT) -> bool {
    (unsafe { value.as_raw().Anonymous.Anonymous.vt == 11 })
        && bool::try_from(value).unwrap_or(false)
}
fn range_command(range: &IDispatch, name: &str, value: VARIANT) -> Result<(), String> {
    if !true_result(&method(
        range,
        "execCommand",
        vec![VARIANT::from(name), VARIANT::from(false), value],
    )?) {
        return Err(format!(
            "宿主拒绝 {name} 格式命令；该页面可能不允许文字格式修改"
        ));
    }
    Ok(())
}
/// Dispatcher reads at most 16 KiB stdin and deserializes LegacyHighlightRequest.
pub fn highlight_in_process(
    hwnd: u64,
    frame: Option<usize>,
    request: &LegacyHighlightRequest,
) -> Result<String, String> {
    validate_needle(&request.needle)?;
    identity(hwnd)?;
    let _apartment = Apartment::new()?;
    let top = document(hwnd)?;
    let document = selected_document(&top, frame)?;
    let body = object(&document, "body")?;
    let range = dispatch(&method(&body, "createTextRange", vec![])?)?;
    let color = |rgb: [u8; 3]| format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
    let foreground = color(request.text);
    let background = color(request.background);
    let started = Instant::now();
    let mut matches = 0usize;
    let mut skipped = 0usize;
    for _ in 0..200 {
        if started.elapsed() >= WALK_TIMEOUT {
            return Ok(format!(
                "已高亮 {matches} 处文字；达到 4 秒上限。格式变化保留在当前页面，刷新可重置。"
            ));
        }
        let found = method(
            &range,
            "findText",
            vec![
                VARIANT::from(request.needle.as_str()),
                VARIANT::from(1_000_000i32),
                VARIANT::from(0i32),
            ],
        )?;
        if !true_result(&found) {
            return Ok(if matches == 0 {
                format!("未找到可高亮文字；已跳过 {skipped} 个表单或受保护区域。")
            } else {
                format!("已高亮 {matches} 处文字，跳过 {skipped} 个表单区域。仅修改当前页面显示，刷新可重置。")
            });
        }
        let mut element = dispatch(&method(&range, "parentElement", vec![])?)?;
        let mut safe = true;
        // Never format or inspect form-field content through text ranges.
        for depth in 0..64 {
            let tag = text(&element, "tagName")?.to_ascii_lowercase();
            if matches!(
                tag.as_str(),
                "input" | "textarea" | "select" | "option" | "script" | "style"
            ) {
                safe = false;
                break;
            }
            if tag == "body" || tag == "html" {
                break;
            }
            if depth == 63 {
                safe = false;
                break;
            }
            match object(&element, "parentElement") {
                Ok(parent) => element = parent,
                Err(_) => {
                    safe = false;
                    break;
                }
            }
        }
        if safe {
            let apply = (|| {
                range_command(&range, "ForeColor", VARIANT::from(foreground.as_str()))?;
                range_command(&range, "BackColor", VARIANT::from(background.as_str()))?;
                for attempt in 0..=2 {
                    let mixed = true_result(&method(
                        &range,
                        "queryCommandIndeterm",
                        vec![VARIANT::from("Bold")],
                    )?);
                    let currently_bold = true_result(&method(
                        &range,
                        "queryCommandState",
                        vec![VARIANT::from("Bold")],
                    )?);
                    if !mixed && currently_bold == request.bold {
                        break;
                    }
                    if attempt == 2 {
                        return Err("宿主没有应用所请求的粗体状态".into());
                    }
                    range_command(&range, "Bold", VARIANT::new())?;
                }
                Ok::<_, String>(())
            })();
            if let Err(error) = apply {
                return Err(format!("{error}。之前已高亮 {matches} 处；当前匹配也可能已有部分颜色变化。刷新页面可重置。"));
            }
            matches += 1;
        } else {
            skipped += 1;
        }
        method(&range, "collapse", vec![VARIANT::from(false)])?;
    }
    Ok(format!(
        "已高亮 {matches} 处文字；最多检查 200 个匹配。刷新页面可重置格式。"
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyDownloadRequest {
    pub url: String,
    pub path: String,
}
const DOWNLOAD_LIMIT: u64 = 50 * 1024 * 1024;
static DOWNLOAD_DIALOG_OPEN: AtomicBool = AtomicBool::new(false);
struct DownloadDialogGuard;
impl Drop for DownloadDialogGuard {
    fn drop(&mut self) {
        DOWNLOAD_DIALOG_OPEN.store(false, Ordering::Release);
    }
}
// Normal error/timeout cleanup. Abrupt GUI termination skips Rust destructors:
// its Job Object still kills the download helper, but a uniquely named .part
// file may remain beside the chosen destination. The final file is untouched.
struct PartialFile(std::path::PathBuf);
impl Drop for PartialFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Downloads one explicitly selected HTTP(S) asset. Choosing Save authorizes
/// that destination and the native dialog confirms any overwrite. The request
/// never inherits browser cookies, HTTP credentials or certificate exceptions.
pub fn download_url(url: &str) -> Result<Option<String>, String> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::{
        Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
        UI::{Controls::Dialogs::*, WindowsAndMessaging::GetForegroundWindow},
    };
    let parsed = parse_download_url(url)?;
    if DOWNLOAD_DIALOG_OPEN
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("已有下载保存对话框打开，请先完成或取消它".into());
    }
    let dialog_guard = DownloadDialogGuard;
    let mut file = vec![0u16; 32768];
    let default = asset_name(&parsed.path);
    for (out, unit) in file.iter_mut().zip(default.encode_utf16()) {
        *out = unit;
    }
    let title: Vec<u16> = "下载资源 · 仅 HTTP(S)，50 MiB / 30 秒上限，不自动登录或跳转\0"
        .encode_utf16()
        .collect();
    let filter: Vec<u16> = "资源文件 (*.*)\0*.*\0\0".encode_utf16().collect();
    let foreground = unsafe { GetForegroundWindow() };
    let mut owner_pid = 0;
    if !foreground.is_null() {
        unsafe { GetWindowThreadProcessId(foreground, &mut owner_pid) };
    }
    let owner = if owner_pid == unsafe { GetCurrentProcessId() } {
        foreground
    } else {
        std::ptr::null_mut()
    };
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = owner;
    dialog.lpstrFile = file.as_mut_ptr();
    dialog.nMaxFile = file.len() as u32;
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrFilter = filter.as_ptr();
    dialog.Flags = OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST;
    if unsafe { GetSaveFileNameW(&mut dialog) } == 0 {
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 {
            Ok(None)
        } else {
            Err(format!("下载保存对话框失败：0x{error:08X}"))
        };
    }
    drop(dialog_guard);
    let length = file
        .iter()
        .position(|value| *value == 0)
        .ok_or("保存路径无结束符")?;
    let destination = std::path::PathBuf::from(std::ffi::OsString::from_wide(&file[..length]));
    let destination_text = destination
        .to_str()
        .ok_or("保存路径无法表示为 Unicode，请选择另一个文件名")?;
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("无效的保存文件名")?;
    if filename.contains(':') || unsafe_extension(filename) {
        return Err(
            "不允许保存为可执行程序、脚本、快捷方式或备用数据流。请选择普通资源文件名。".into(),
        );
    }
    let parent = destination.parent().ok_or("保存目录无效")?;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "系统时钟无效")?
        .as_nanos();
    let temporary = parent.join(format!(
        ".coralspynext-{}-{unique}.part",
        std::process::id()
    ));
    let temporary_text = temporary.to_str().ok_or("临时路径无法表示为 Unicode")?;
    let partial_guard = PartialFile(temporary.clone());
    let request = LegacyDownloadRequest {
        url: url.into(),
        path: temporary_text.into(),
    };
    let input = serde_json::to_vec(&request).map_err(|e| format!("无法准备下载：{e}"))?;
    let bytes = bounded_worker(
        "--legacy-download-worker",
        &[],
        Some(&input),
        Duration::from_secs(30),
    )?;
    let result: Result<u64, String> =
        serde_json::from_slice(&bytes).map_err(|e| format!("下载结果无效：{e}"))?;
    let size = result?;
    let actual = std::fs::metadata(&temporary)
        .map_err(|e| format!("无法核对下载文件：{e}"))?
        .len();
    if size != actual || actual > DOWNLOAD_LIMIT {
        return Err("下载文件大小核对失败；没有覆盖目标文件".into());
    }
    // Mark files as Internet-zone content before publishing, where the filesystem
    // supports NTFS alternate streams. No downloaded content is ever opened.
    let _ = std::fs::write(
        format!("{temporary_text}:Zone.Identifier"),
        b"[ZoneTransfer]\r\nZoneId=3\r\n",
    );
    let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(format!(
            "资源已下载，但无法保存到指定位置：{}",
            std::io::Error::last_os_error()
        ));
    }
    drop(partial_guard);
    Ok(Some(destination_text.to_owned()))
}
pub fn download(url: &str) -> Result<Option<String>, String> {
    download_url(url)
}

struct DownloadUrl {
    host: Vec<u16>,
    port: u16,
    object: Vec<u16>,
    path: String,
    secure: bool,
}
fn wide_part(pointer: *mut u16, length: u32) -> Vec<u16> {
    if pointer.is_null() || length == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(pointer, length as usize) }.to_vec()
    }
}
fn parse_download_url(url: &str) -> Result<DownloadUrl, String> {
    use windows_sys::Win32::Networking::WinHttp::*;
    if url.len() > 8192 || url.chars().any(|c| c.is_control()) || url.contains('\\') {
        return Err("下载地址过长或包含无效字符".into());
    }
    let url = url.split('#').next().unwrap_or("");
    let wide: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
    let mut parts: URL_COMPONENTS = unsafe { std::mem::zeroed() };
    parts.dwStructSize = std::mem::size_of::<URL_COMPONENTS>() as u32;
    parts.dwHostNameLength = u32::MAX;
    parts.dwUrlPathLength = u32::MAX;
    parts.dwExtraInfoLength = u32::MAX;
    parts.dwUserNameLength = u32::MAX;
    parts.dwPasswordLength = u32::MAX;
    if unsafe { WinHttpCrackUrl(wide.as_ptr(), (wide.len() - 1) as u32, 0, &mut parts) } == 0 {
        return Err("无法解析下载地址；仅支持完整 HTTP(S) URL".into());
    }
    if parts.nScheme != WINHTTP_INTERNET_SCHEME_HTTP
        && parts.nScheme != WINHTTP_INTERNET_SCHEME_HTTPS
    {
        return Err("下载只支持 HTTP 和 HTTPS；不打开 file、javascript、data 或其他协议".into());
    }
    if parts.dwUserNameLength != 0 || parts.dwPasswordLength != 0 {
        return Err("下载地址不能内含用户名或密码；不会自动登录".into());
    }
    let mut host = wide_part(parts.lpszHostName, parts.dwHostNameLength);
    if host.is_empty() {
        return Err("下载地址缺少主机名".into());
    }
    host.push(0);
    let mut path = wide_part(parts.lpszUrlPath, parts.dwUrlPathLength);
    if path.is_empty() {
        path.push('/' as u16);
    }
    let path_text = String::from_utf16_lossy(&path);
    if unsafe_extension(&decode_url_ascii(&path_text)) {
        return Err("不下载可执行程序、脚本或快捷方式地址".into());
    }
    path.extend(wide_part(parts.lpszExtraInfo, parts.dwExtraInfoLength));
    path.push(0);
    Ok(DownloadUrl {
        host,
        port: parts.nPort,
        object: path,
        path: path_text,
        secure: parts.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS,
    })
}
fn decode_url_ascii(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut result = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            let hex = |value: u8| (value as char).to_digit(16);
            if let (Some(a), Some(b)) = (hex(bytes[at + 1]), hex(bytes[at + 2])) {
                result.push((a * 16 + b) as u8);
                at += 3;
                continue;
            }
        }
        result.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}
fn unsafe_extension(name: &str) -> bool {
    let name = name
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches(['.', ' '])
        .to_ascii_lowercase();
    let extension = name.rsplit('.').next().unwrap_or("");
    matches!(
        extension,
        "exe"
            | "com"
            | "dll"
            | "scr"
            | "pif"
            | "cpl"
            | "msi"
            | "msp"
            | "mst"
            | "bat"
            | "cmd"
            | "ps1"
            | "psm1"
            | "psd1"
            | "vbs"
            | "vbe"
            | "js"
            | "jse"
            | "wsf"
            | "wsh"
            | "hta"
            | "reg"
            | "lnk"
            | "url"
            | "scf"
            | "appref-ms"
            | "application"
            | "gadget"
            | "jar"
            | "chm"
            | "sh"
            | "bash"
            | "py"
            | "pl"
            | "rb"
            | "iso"
            | "img"
            | "vhd"
            | "vhdx"
    )
}
fn asset_name(path: &str) -> String {
    let name = decode_url_ascii(path.rsplit('/').next().unwrap_or(""));
    if name.is_empty()
        || name.len() > 180
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || unsafe_extension(&name)
    {
        "resource.bin".into()
    } else {
        name
    }
}
struct InternetHandle(*mut std::ffi::c_void);
impl InternetHandle {
    fn new(handle: *mut std::ffi::c_void, stage: &str) -> Result<Self, String> {
        if handle.is_null() {
            Err(format!("{stage}失败：{}", std::io::Error::last_os_error()))
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for InternetHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Networking::WinHttp::WinHttpCloseHandle(self.0);
        }
    }
}
fn http_ok(ok: i32, stage: &str) -> Result<(), String> {
    if ok == 0 {
        Err(format!(
            "{stage}失败：{}。不会绕过证书或登录限制。",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}
fn header(request: &InternetHandle, name: u32) -> Option<String> {
    use windows_sys::Win32::Networking::WinHttp::*;
    let mut buffer = [0u16; 512];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    let ok = unsafe {
        WinHttpQueryHeaders(
            request.0,
            name,
            std::ptr::null(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        None
    } else {
        let length = buffer
            .iter()
            .position(|&value| value == 0)
            .unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    }
}
/// No UI, saved credentials, cookies, redirects, application retries, cert exceptions or
/// execution of downloaded bytes. `path` must name a new temporary file.
pub fn download_in_process(request: &LegacyDownloadRequest) -> Result<u64, String> {
    use std::io::Write;
    use windows_sys::Win32::Networking::WinHttp::*;
    let parsed = parse_download_url(&request.url)?;
    if request.path.is_empty() || request.path.contains('\0') {
        return Err("临时保存路径无效".into());
    }
    let agent: Vec<u16> = "CoralSpyNext/1.0 explicit-asset-download\0"
        .encode_utf16()
        .collect();
    let session = InternetHandle::new(
        unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        },
        "初始化 HTTP",
    )?;
    http_ok(
        unsafe { WinHttpSetTimeouts(session.0, 5000, 5000, 5000, 5000) },
        "设置 HTTP 超时",
    )?;
    let connection = InternetHandle::new(
        unsafe { WinHttpConnect(session.0, parsed.host.as_ptr(), parsed.port, 0) },
        "连接服务器",
    )?;
    let verb: Vec<u16> = "GET\0".encode_utf16().collect();
    let response = InternetHandle::new(
        unsafe {
            WinHttpOpenRequest(
                connection.0,
                verb.as_ptr(),
                parsed.object.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                if parsed.secure {
                    WINHTTP_FLAG_SECURE
                } else {
                    0
                },
            )
        },
        "创建 HTTP 请求",
    )?;
    let disabled =
        WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION;
    http_ok(
        unsafe {
            WinHttpSetOption(
                response.0,
                WINHTTP_OPTION_DISABLE_FEATURE,
                (&disabled as *const u32).cast(),
                4,
            )
        },
        "禁用自动登录、Cookie 和跳转",
    )?;
    let autologon = WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH;
    http_ok(
        unsafe {
            WinHttpSetOption(
                response.0,
                WINHTTP_OPTION_AUTOLOGON_POLICY,
                (&autologon as *const u32).cast(),
                4,
            )
        },
        "禁用自动凭据",
    )?;
    http_ok(
        unsafe { WinHttpSendRequest(response.0, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0) },
        "发送下载请求",
    )?;
    http_ok(
        unsafe { WinHttpReceiveResponse(response.0, std::ptr::null_mut()) },
        "接收服务器响应",
    )?;
    let status = header(&response, WINHTTP_QUERY_STATUS_CODE)
        .and_then(|text| text.parse::<u16>().ok())
        .ok_or("服务器未返回有效 HTTP 状态码")?;
    if (300..400).contains(&status) {
        return Err(format!("服务器返回 HTTP {status} 跳转；为避免转向未知地址，未自动跟随。请在浏览器确认最终 HTTP(S) 资源地址后重试。"));
    }
    if status == 401 || status == 407 {
        return Err(format!(
            "下载需要登录或代理认证（HTTP {status}）；不会读取或发送浏览器凭据。"
        ));
    }
    if status != 200 {
        return Err(format!("服务器返回 HTTP {status}；没有保存资源"));
    }
    let expected =
        header(&response, WINHTTP_QUERY_CONTENT_LENGTH).and_then(|text| text.parse::<u64>().ok());
    if expected.is_some_and(|size| size > DOWNLOAD_LIMIT) {
        return Err("资源超过 50 MiB 下载上限".into());
    }
    let content_type = header(&response, WINHTTP_QUERY_CONTENT_TYPE)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if [
        "x-msdownload",
        "x-msdos-program",
        "vnd.microsoft.portable-executable",
        "x-sh",
        "x-powershell",
        "javascript",
        "x-python",
    ]
    .iter()
    .any(|kind| content_type.contains(kind))
    {
        return Err("服务器返回可执行程序或脚本类型，已停止下载".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&request.path)
        .map_err(|e| format!("无法创建下载临时文件：{e}"))?;
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    let started = Instant::now();
    let mut prefix = Vec::new();
    loop {
        if started.elapsed() >= Duration::from_secs(25) {
            return Err("下载达到 25 秒流式读取上限".into());
        }
        let mut read = 0;
        http_ok(
            unsafe {
                WinHttpReadData(
                    response.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut read,
                )
            },
            "读取资源",
        )?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > DOWNLOAD_LIMIT {
            return Err("资源超过 50 MiB 下载上限".into());
        }
        if prefix.len() < 4 {
            prefix.extend_from_slice(&buffer[..(read as usize).min(4 - prefix.len())]);
            if prefix.starts_with(b"MZ")
                || prefix.starts_with(b"\x7FELF")
                || prefix.starts_with(b"#!")
            {
                return Err("资源内容为可执行文件或脚本，已停止保存".into());
            }
        }
        file.write_all(&buffer[..read as usize])
            .map_err(|e| format!("保存资源失败：{e}"))?;
    }
    if expected.is_some_and(|size| size != total) {
        return Err("响应未完整下载；没有替换目标文件".into());
    }
    file.sync_all()
        .map_err(|e| format!("保存资源同步失败：{e}"))?;
    Ok(total)
}
