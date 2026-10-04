//! User-initiated, read-only UI Automation inspection of one selected HWND.
//!
//! UIA supplies ListBox/ComboBox/ListView/TreeView items and Text/Value pattern
//! content without remote-memory reads, injected hooks, keystrokes or expansion.
//! A disposable helper process contains providers that ignore UIA timeouts.
//! Password status is checked before any content property or pattern is read;
//! protected or unclassified elements are redacted and their subtrees skipped.

use crate::{
    locale::label,
    model::{ContentNode, ContentSnapshot},
};
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
    core::{Interface, BSTR},
    Win32::{
        Foundation::HWND,
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_MULTITHREADED,
        },
        UI::Accessibility::*,
    },
};
use windows_sys::Win32::{
    System::Threading::CREATE_NO_WINDOW,
    UI::WindowsAndMessaging::{
        GetClassNameW, GetWindowThreadProcessId, IsWindow, STATE_SYSTEM_PROTECTED,
        STATE_SYSTEM_VALID,
    },
};

pub const MAX_DEPTH: usize = 32;
pub const MAX_NODES: usize = 5_000;
pub const MAX_TEXT_BYTES: usize = 1_048_576;
const WALK_BUDGET: Duration = Duration::from_secs(3);
const WORKER_BUDGET: Duration = Duration::from_secs(8);
// JSON may escape one byte into six and includes both structured and plain text.
const MAX_WIRE_BYTES: usize = 16 * 1_048_576;
const PROTECTED: &str = "[受保护内容：已跳过]";
const UNVERIFIED: &str = "[无法确认密码保护状态：已跳过]";

// Select only program-owned messages; provider text and error details stay intact.
macro_rules! localized_format {
    ($zh:literal, $en:literal) => {
        if crate::locale::is_english() {
            format!($en)
        } else {
            format!($zh)
        }
    };
}

#[derive(Debug, PartialEq, Eq)]
struct WindowIdentity {
    pid: u32,
    tid: u32,
    class: Vec<u16>,
}

fn identity(hwnd: u64) -> Result<WindowIdentity, String> {
    let address = usize::try_from(hwnd).map_err(|_| {
        label(
            "窗口句柄超出指针宽度",
            "The window handle exceeds the pointer width",
        )
    })?;
    let window = address as windows_sys::Win32::Foundation::HWND;
    if address == 0 || unsafe { IsWindow(window) } == 0 {
        return Err(label(
            "所选窗口无效或已经关闭",
            "The selected window is invalid or has closed",
        )
        .to_owned());
    }
    let mut pid = 0;
    let tid = unsafe { GetWindowThreadProcessId(window, &mut pid) };
    let mut class = [0u16; 256];
    let count = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    if tid == 0 || pid == 0 || count <= 0 {
        return Err(label(
            "无法确认所选窗口的进程与类型；请重新选取",
            "Cannot verify the selected window's process and class; select it again",
        )
        .to_owned());
    }
    Ok(WindowIdentity {
        pid,
        tid,
        class: class[..count as usize].to_vec(),
    })
}

/// Isolates all provider calls from the GUI. The executable must dispatch
/// `--accessibility-worker <decimal HWND>` to `inspect_in_process` before GUI
/// initialization, and write a single JSON `Result<ContentSnapshot, String>`.
pub fn inspect(hwnd: u64) -> Result<ContentSnapshot, String> {
    let before = identity(hwnd)?;
    let executable = std::env::current_exe().map_err(|e| {
        localized_format!(
            "定位检查程序失败：{e}",
            "Failed to locate the inspection executable: {e}"
        )
    })?;
    let mut child = Command::new(executable)
        .arg("--accessibility-worker")
        .arg(hwnd.to_string())
        .env("CORALSPYNEXT_LANG", crate::locale::language_code())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| {
            localized_format!(
                "启动 UI Automation 检查进程失败：{e}",
                "Failed to start the UI Automation inspection process: {e}"
            )
        })?;
    // Kernel containment survives abrupt parent exit while COM is hung. Keep
    // the job alive until every normal/error path has killed and reaped child.
    let _job_guard = match crate::helper_guard::bind_child(&child) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(label(
            "无法读取 UI Automation 检查进程输出",
            "Cannot read output from the UI Automation inspection process",
        )
        .into());
    };
    let overflow = Arc::new(AtomicBool::new(false));
    let reader_overflow = Arc::clone(&overflow);
    let reader = match thread::Builder::new()
        .name("uia-result-reader".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            // Keep draining while the child runs, so even a full result cannot block
            // its stdout pipe. take() caps allocations even for malformed output.
            stdout
                .by_ref()
                .take((MAX_WIRE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| {
                    localized_format!(
                        "读取检查结果失败：{e}",
                        "Failed to read inspection results: {e}"
                    )
                })?;
            if bytes.len() > MAX_WIRE_BYTES {
                reader_overflow.store(true, Ordering::Release);
            }
            Ok::<_, String>(bytes)
        }) {
        Ok(reader) => reader,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(localized_format!(
                "创建结果读取线程失败：{e}",
                "Failed to create the result-reader thread: {e}"
            ));
        }
    };
    let started = Instant::now();
    let outcome = loop {
        if overflow.load(Ordering::Acquire) {
            break Err(label(
                "UI Automation 返回结果超过安全大小限制",
                "UI Automation results exceed the safe size limit",
            )
            .to_owned());
        }
        if started.elapsed() >= WORKER_BUDGET {
            break Err(label("UI Automation 提供程序在 8 秒内未响应；检查进程已停止。目标程序可能挂起或拒绝访问。", "The UI Automation provider did not respond within 8 seconds; the inspection process was stopped. The target application may be unresponsive or denying access.").to_owned());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                break Err(localized_format!(
                    "等待检查进程失败：{e}",
                    "Failed to wait for the inspection process: {e}"
                ))
            }
        }
    };
    if outcome.is_err() {
        let _ = child.kill();
    }
    // Always reap the helper and join the bounded pipe reader, including all
    // timeout/error paths. The helper never starts child processes of its own.
    let _ = child.wait();
    let bytes = reader.join().map_err(|_| {
        label(
            "检查结果读取线程失败",
            "The inspection result-reader thread failed",
        )
        .to_owned()
    })?;
    let status = outcome?;
    let bytes = bytes?;
    if overflow.load(Ordering::Acquire) {
        return Err(label(
            "检查结果超过安全大小限制",
            "Inspection results exceed the safe size limit",
        )
        .into());
    }
    if !status.success() {
        return Err(localized_format!(
            "UI Automation 检查进程异常退出（{status}）",
            "The UI Automation inspection process exited unexpectedly ({status})"
        ));
    }
    if identity(hwnd)? != before {
        return Err(label("检查期间所选窗口已改变；结果已丢弃，请重新选取", "The selected window changed during inspection; results were discarded. Select it again").into());
    }
    let result: Result<ContentSnapshot, String> = serde_json::from_slice(&bytes).map_err(|e| {
        localized_format!(
            "UI Automation 检查结果格式无效：{e}",
            "Invalid UI Automation inspection result format: {e}"
        )
    })?;
    let snapshot = result?;
    if snapshot.hwnd != hwnd {
        return Err(label(
            "检查结果与所选窗口不匹配",
            "Inspection results do not match the selected window",
        )
        .into());
    }
    Ok(snapshot)
}

struct ComApartment;
impl ComApartment {
    fn new() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|e| {
                localized_format!(
                    "初始化 UI Automation MTA 失败：{e}",
                    "Failed to initialize the UI Automation MTA: {e}"
                )
            })?;
        Ok(Self)
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

// Traversal and bounded text helpers follow below.

struct Walk {
    automation: IUIAutomation,
    snapshot: ContentSnapshot,
    started: Instant,
    remaining: usize,
    had_content: bool,
    stopped: bool,
    elements: Vec<IUIAutomationElement>,
    parents: Vec<Option<usize>>,
    safe_subtree: Vec<bool>,
}
impl Walk {
    fn warn(&mut self, message: &str) {
        if self.snapshot.warnings.len() < 32 && !self.snapshot.warnings.iter().any(|w| w == message)
        {
            self.snapshot.warnings.push(message.to_owned());
        }
    }
    fn limit(&mut self, message: &str) {
        self.snapshot.truncated = true;
        self.warn(message);
    }
    fn time_left(&mut self) -> bool {
        if self.started.elapsed() >= WALK_BUDGET {
            self.stopped = true;
            self.limit(label(
                "已达到 3 秒内容采集预算；当前结果不完整。",
                "The 3-second content collection budget was reached; results are incomplete.",
            ));
        }
        self.started.elapsed() < WALK_BUDGET
    }
    fn continue_walk(&mut self) -> bool {
        if self.snapshot.nodes.len() >= MAX_NODES {
            self.stopped = true;
            self.limit(label(
                "已达到 5000 个元素限制；其余内容未读取。",
                "The 5,000-element limit was reached; remaining content was not read.",
            ));
        }
        if self.remaining == 0 {
            self.stopped = true;
            self.limit(label(
                "已达到 1 MiB 内容限制；其余内容未读取。",
                "The 1 MiB content limit was reached; remaining content was not read.",
            ));
        }
        let time_left = self.time_left();
        !self.stopped && time_left
    }
    fn text(&mut self, text: BSTR) -> String {
        let (output, truncated) = bounded_utf16(text.as_wide(), self.remaining);
        self.remaining -= output.len();
        if truncated {
            self.limit(label(
                "已达到 1 MiB 内容限制；部分字段已截断。",
                "The 1 MiB content limit was reached; some fields were truncated.",
            ));
        }
        output
    }
    fn property(&mut self, value: windows::core::Result<BSTR>) -> String {
        match value {
            Ok(value) => self.text(value),
            Err(_) => {
                self.limit(label("部分元素属性无法读取；提供程序可能拒绝访问或元素已消失。", "Some element properties could not be read; the provider may be denying access or the element may have disappeared."));
                String::new()
            }
        }
    }
    fn scan(
        &mut self,
        element: &IUIAutomationElement,
        walker: &IUIAutomationTreeWalker,
        depth: usize,
        parent: Option<usize>,
    ) -> bool {
        if !self.continue_walk() {
            return false;
        }
        // Phase one reads no Name, Value, Text or legacy content anywhere. A
        // container's TextPattern may aggregate descendants, so every descendant
        // must be classified before any of that container's content is requested.
        let password = password_status(element, &self.automation);
        let protected = password != Some(false);
        if !self.time_left() {
            return false;
        }
        let control_type = unsafe { element.CurrentControlType() }.ok();
        let mut node = ContentNode {
            depth,
            role: role_name(control_type).to_owned(),
            is_password: protected,
            ..Default::default()
        };
        if self.time_left() {
            node.class_name = self.property(unsafe { element.CurrentClassName() });
        }
        if self.time_left() {
            node.automation_id = self.property(unsafe { element.CurrentAutomationId() });
        }
        if protected {
            node.value = if password == Some(true) {
                label(PROTECTED, "[Protected content: skipped]")
            } else {
                label(
                    UNVERIFIED,
                    "[Password protection status could not be verified: skipped]",
                )
            }
            .to_owned();
            self.had_content |= password == Some(true);
            self.warn(label("密码或无法确认保护状态的元素及其整个子树已跳过；不读取名称、值或文本。", "Password elements, elements with unverified protection status, and their entire subtrees were skipped; names, values, and text were not read."));
        }
        let index = self.snapshot.nodes.len();
        self.snapshot.nodes.push(node);
        self.elements.push(element.clone());
        self.parents.push(parent);
        self.safe_subtree.push(false);
        if protected || !self.continue_walk() {
            return false;
        }
        let mut child = match navigate(walker, element, true) {
            Ok(Some(child)) => child,
            Ok(None) => {
                self.safe_subtree[index] = true;
                return true;
            }
            Err(_) => {
                self.navigation_error();
                return false;
            }
        };
        if depth >= MAX_DEPTH {
            self.limit(label("已达到 32 层深度限制；更深的子元素未读取，祖先的聚合内容已跳过。", "The 32-level depth limit was reached; deeper elements were not read, and aggregated ancestor content was skipped."));
            return false;
        }
        let mut safe = true;
        loop {
            safe &= self.scan(&child, walker, depth + 1, Some(index));
            if !self.continue_walk() {
                return false;
            }
            child = match navigate(walker, &child, false) {
                Ok(Some(next)) => next,
                Ok(None) => break,
                Err(_) => {
                    self.navigation_error();
                    return false;
                }
            };
        }
        self.safe_subtree[index] = safe;
        safe
    }
    fn read_verified_content(&mut self) {
        // Read leaves before ancestors. If a node changes its protection state
        // between phases, invalidate all not-yet-read ancestors as well.
        for index in (0..self.elements.len()).rev() {
            if !self.safe_subtree[index] {
                if !self.snapshot.nodes[index].is_password {
                    self.warn(
                        label("子树含受保护、未验证或无法访问的元素；对应祖先的聚合名称/值/文本已跳过。", "The subtree contains protected, unverified, or inaccessible elements; aggregated ancestor names, values, and text were skipped."),
                    );
                }
                continue;
            }
            if !self.time_left() || self.remaining == 0 {
                self.limit(label("内容读取达到时间或大小限制；只返回已安全读取的部分。", "Content reading reached the time or size limit; only safely read content is returned."));
                break;
            }
            let element = self.elements[index].clone();
            if password_status(&element, &self.automation) != Some(false) {
                invalidate_ancestors(&mut self.safe_subtree, &self.parents, index);
                self.snapshot.nodes[index].is_password = true;
                self.snapshot.nodes[index].value = label(
                    UNVERIFIED,
                    "[Password protection status could not be verified: skipped]",
                )
                .to_owned();
                self.warn(label("采集期间保护状态发生变化或不可验证；该元素和祖先聚合内容已跳过。", "Protection status changed or could not be verified during collection; the element and aggregated ancestor content were skipped."));
                continue;
            }
            let mut node = self.snapshot.nodes[index].clone();
            if self.time_left() {
                node.name = self.property(unsafe { element.CurrentName() });
                self.had_content |= !node.name.is_empty();
            }
            if self.time_left() && self.remaining > 0 {
                // TextPattern covers RichEdit and document controls. GetText has an
                // explicit UTF-16 limit; never use the unbounded -1 form.
                if let Ok(pattern) = unsafe {
                    element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                } {
                    match unsafe { pattern.DocumentRange() } {
                        Ok(range) if self.time_left() => {
                            let maximum =
                                self.remaining.saturating_add(1).min(i32::MAX as usize) as i32;
                            match unsafe { range.GetText(maximum) } {
                            Ok(text) => {
                                if text.len() >= maximum as usize {
                                    self.limit(
                                        label("TextPattern 文本可能超过长度限制；仅返回有界片段。", "TextPattern content may exceed the length limit; only a bounded excerpt is returned."),
                                    );
                                }
                                node.value = self.text(text);
                                self.had_content = true; // A supported empty editor is a real result.
                            }
                            Err(_) => self.warn(
                                label("TextPattern 已提供，但无法读取文档内容；提供程序可能拒绝访问。", "TextPattern is available, but document content could not be read; the provider may be denying access."),
                            ),
                        }
                        }
                        Ok(_) => {}
                        Err(_) => self.warn(label("TextPattern 无法返回文档范围；内容可能已不可用。", "TextPattern could not return the document range; the content may no longer be available.")),
                    }
                }
            }
            if node.value.is_empty() && self.time_left() && self.remaining > 0 {
                if let Ok(pattern) = unsafe {
                    element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                } {
                    match unsafe { pattern.CurrentValue() } {
                        Ok(value) => {
                            node.value = self.text(value);
                            self.had_content = true;
                        }
                        Err(_) => self
                            .warn(label("ValuePattern 已提供，但值不可读取；内容可能受限或元素已消失。", "ValuePattern is available, but its value could not be read; the content may be restricted or the element may have disappeared.")),
                    }
                }
            }
            // Legacy patterns improve read-only compatibility with classic controls
            // exposed through the documented MSAA-to-UIA bridge. Never set a value or
            // invoke an action, and never query any legacy content on password nodes.
            if node.value.is_empty() && self.time_left() && self.remaining > 0 {
                if let Ok(pattern) = unsafe {
                    element.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                        UIA_LegacyIAccessiblePatternId,
                    )
                } {
                    if let Ok(value) = unsafe { pattern.CurrentValue() } {
                        node.value = self.text(value);
                        self.had_content |= !node.value.is_empty();
                    }
                }
            }
            if self.time_left()
                && (unsafe {
                    element.GetCurrentPatternAs::<IUIAutomationItemContainerPattern>(
                        UIA_ItemContainerPatternId,
                    )
                }
                .is_ok()
                    || (self.time_left()
                        && unsafe {
                            element.GetCurrentPatternAs::<IUIAutomationVirtualizedItemPattern>(
                                UIA_VirtualizedItemPatternId,
                            )
                        }
                        .is_ok()))
            {
                self.warn(label("提供程序使用虚拟化项目；仅采集当前暴露的元素，不自动滚动、展开或 Realize 项目。", "The provider uses virtualized items; only currently exposed elements are collected. Items are not automatically scrolled, expanded, or realized."));
            }
            if self.time_left() {
                if let Ok(pattern) = unsafe {
                    element.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                        UIA_ExpandCollapsePatternId,
                    )
                } {
                    if matches!(unsafe { pattern.CurrentExpandCollapseState() }, Ok(state) if state == ExpandCollapseState_Collapsed || state == ExpandCollapseState_PartiallyExpanded)
                    {
                        self.warn(label("检测到折叠内容；仅采集提供程序目前暴露的子项，不改变目标界面。", "Collapsed content was detected; only children currently exposed by the provider are collected, without changing the target interface."));
                    }
                }
            }

            self.snapshot.nodes[index] = node;
        }
    }
    fn navigation_error(&mut self) {
        self.limit(
            label("部分子元素无法访问；提供程序可能超时、拒绝访问或正在改变。祖先聚合内容已跳过。", "Some child elements could not be accessed; the provider may have timed out, denied access, or changed. Aggregated ancestor content was skipped."),
        );
    }
}

/// Run only in the disposable helper (or a dedicated MTA test worker). Calling
/// this directly on a GUI thread would defeat the outer hard timeout.
pub fn inspect_in_process(hwnd: u64) -> Result<ContentSnapshot, String> {
    crate::locale::init_helper_language();
    let before = identity(hwnd)?;
    let started = Instant::now();
    let _apartment = ComApartment::new()?;
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
            .or_else(|_| unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) })
            .map_err(|e| {
                localized_format!(
                    "UI Automation 不可用：{e}",
                    "UI Automation is unavailable: {e}"
                )
            })?;
    let mut state = Walk {
        automation: automation.clone(),
        snapshot: ContentSnapshot {
            hwnd,
            source: label(
                "Windows UI Automation · 只读、用户主动检查",
                "Windows UI Automation · Read-only, user-initiated inspection",
            )
            .into(),
            ..Default::default()
        },
        started,
        remaining: MAX_TEXT_BYTES,
        had_content: false,
        stopped: false,
        elements: Vec::new(),
        parents: Vec::new(),
        safe_subtree: Vec::new(),
    };
    if let Ok(settings) = automation.cast::<IUIAutomation2>() {
        let connection = unsafe { settings.SetConnectionTimeout(300) };
        let transaction = unsafe { settings.SetTransactionTimeout(300) };
        if connection.is_err() || transaction.is_err() {
            state.warn(label("提供程序超时设置不可用；仍由独立检查进程的 8 秒硬超时保护。", "Provider timeout settings are unavailable; the separate inspection process still enforces an 8-second hard timeout."));
        }
        // This inspector never changes focus; turn off UIA's automatic focus.
        let _ = unsafe { settings.SetAutoSetFocus(false) };
    } else {
        state.warn(label("IUIAutomation2 不可用；仍由独立检查进程的 8 秒硬超时保护。", "IUIAutomation2 is unavailable; the separate inspection process still enforces an 8-second hard timeout."));
    }
    let root =
        unsafe { automation.ElementFromHandle(HWND(hwnd as usize as *mut _)) }.map_err(|e| {
            localized_format!(
                "所选窗口没有可访问的 UI Automation 提供程序：{e}",
                "The selected window has no accessible UI Automation provider: {e}"
            )
        })?;
    let provider_pid = unsafe { root.CurrentProcessId() }.map_err(|e| {
        localized_format!(
            "无法验证 UI Automation 元素所属进程：{e}",
            "Cannot verify the process owning the UI Automation element: {e}"
        )
    })?;
    if provider_pid as u32 != before.pid {
        return Err(label("UI Automation 根元素与所选窗口进程不匹配；结果已丢弃", "The UI Automation root element does not match the selected window's process; results were discarded").into());
    }
    // Raw view preserves classic item/subitem structure instead of filtering
    // intermediary nodes. Traversal remains confined to this selected root.
    let walker = unsafe { automation.RawViewWalker() }.map_err(|e| {
        localized_format!(
            "无法读取 UI Automation 树：{e}",
            "Cannot read the UI Automation tree: {e}"
        )
    })?;
    state.scan(&root, &walker, 0, None);
    state.read_verified_content();
    if identity(hwnd)? != before {
        return Err(label("采集期间所选窗口已关闭或改变；结果已丢弃，请重新选取", "The selected window closed or changed during collection; results were discarded. Select it again").into());
    }
    if state.snapshot.nodes.is_empty() || !state.had_content {
        // Report only structural counts and program-owned warnings. Never add
        // unverified provider Name/Value/Text data to an error diagnostic.
        let nodes = state.snapshot.nodes.len();
        let protected = state
            .snapshot
            .nodes
            .iter()
            .filter(|node| node.is_password)
            .count();
        let warnings = state.snapshot.warnings.join(" ");
        return Err(localized_format!(
            "提供程序没有返回可读内容。目标可能不支持 UI Automation、内容受限，或当前为空且没有文本模式；这不是成功的空结果。元素：{nodes}；受保护或未验证：{protected}。{warnings}",
            "The provider returned no readable content. The target may not support UI Automation, its content may be restricted, or it may be empty with no text pattern; this is not a successful empty result. Elements: {nodes}; protected or unverified: {protected}. {warnings}"
        ));
    }
    state
        .warn(label("结果取决于目标程序的可访问性提供程序；未暴露、虚拟化或更高权限的内容可能不可读取。", "Results depend on the target application's accessibility provider; unexposed, virtualized, or higher-privilege content may not be readable."));
    let (text, truncated) = render_nodes(&state.snapshot.nodes, MAX_TEXT_BYTES);
    state.snapshot.text = text;
    if truncated {
        state.limit(label("纯文本导出达到 1 MiB 限制；结构化节点可能包含额外字段。", "Plain-text export reached the 1 MiB limit; structured nodes may contain additional fields."));
    }
    Ok(state.snapshot)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiaPasswordStatus {
    Known(bool),
    Unsupported,
    Unverified,
}

fn password_status(element: &IUIAutomationElement, automation: &IUIAutomation) -> Option<bool> {
    let uia = match unsafe { element.GetCurrentPropertyValueEx(UIA_IsPasswordPropertyId, true) } {
        Ok(value) if unsafe { value.as_raw().Anonymous.Anonymous.vt } == 11 => {
            // Accept exactly VT_BOOL, without VARIANT type coercion or defaults.
            bool::try_from(&value)
                .map(UiaPasswordStatus::Known)
                .unwrap_or(UiaPasswordStatus::Unverified)
        }
        Ok(value) => {
            // Only the documented NotSupported sentinel permits a fallback.
            // Malformed variants, access failures and expired elements do not.
            if matches!(unsafe { automation.CheckNotSupported(&value) }, Ok(result) if result.as_bool())
            {
                UiaPasswordStatus::Unsupported
            } else {
                UiaPasswordStatus::Unverified
            }
        }
        Err(_) => UiaPasswordStatus::Unverified,
    };
    classify_password_evidence(uia, || {
        // Standard MSAA-backed TreeView/ListView items can omit IsPassword.
        // The UIA bridge exposes their real numeric role/state through this
        // supported pattern, including STATE_SYSTEM_PROTECTED. Query no Name,
        // Value, Description, Text or other content to make this decision.
        // https://learn.microsoft.com/windows/win32/winauto/uiauto-implementinglegacyiaccessible
        // https://learn.microsoft.com/windows/win32/winauto/object-state-constants
        let legacy = unsafe {
            element.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                UIA_LegacyIAccessiblePatternId,
            )
        }
        .ok()?;
        let state = unsafe { legacy.CurrentState() }.ok()?;
        let role = unsafe { legacy.CurrentRole() }.ok()?;
        Some((role, state))
    })
}

fn classify_password_evidence(
    uia: UiaPasswordStatus,
    legacy: impl FnOnce() -> Option<(u32, u32)>,
) -> Option<bool> {
    match uia {
        UiaPasswordStatus::Known(password) => Some(password),
        UiaPasswordStatus::Unverified => None,
        UiaPasswordStatus::Unsupported => {
            // Both fields must have been read successfully from a supported
            // pattern. Never equate an absent/error property with state zero.
            let (role, state) = legacy()?;
            if state & STATE_SYSTEM_PROTECTED != 0 {
                return Some(true);
            }
            if !(ROLE_SYSTEM_TITLEBAR..=ROLE_SYSTEM_OUTLINEBUTTON).contains(&role)
                || state & !(STATE_SYSTEM_VALID | STATE_SYSTEM_HASPOPUP) != 0
            {
                return None;
            }
            Some(false)
        }
    }
}

fn invalidate_ancestors(safe: &mut [bool], parents: &[Option<usize>], mut index: usize) {
    loop {
        safe[index] = false;
        match parents[index] {
            Some(parent) => index = parent,
            None => break,
        }
    }
}

fn navigate(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    first: bool,
) -> windows::core::Result<Option<IUIAutomationElement>> {
    // The generated wrappers turn successful null results into E_POINTER. Use
    // the same documented COM vtable, retaining the distinction between an empty
    // child list (S_OK + null) and a provider's actual E_POINTER/error response.
    let mut result = std::ptr::null_mut();
    unsafe {
        let vtable = Interface::vtable(walker);
        let method = if first {
            vtable.GetFirstChildElement
        } else {
            vtable.GetNextSiblingElement
        };
        method(walker.as_raw(), element.as_raw(), &mut result).ok()?;
        if result.is_null() {
            Ok(None)
        } else {
            Ok(Some(IUIAutomationElement::from_raw(result)))
        }
    }
}

fn bounded_utf16(input: &[u16], max_bytes: usize) -> (String, bool) {
    let mut result = String::with_capacity(input.len().min(max_bytes));
    for c in char::decode_utf16(input.iter().copied()) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        if result.len() + c.len_utf8() > max_bytes {
            return (result, true);
        }
        result.push(c);
    }
    (result, false)
}

fn render_nodes(nodes: &[ContentNode], max_bytes: usize) -> (String, bool) {
    let mut result = String::new();
    for node in nodes {
        let line = format!(
            "{}[{}] {}{}{}\n",
            "  ".repeat(node.depth.min(MAX_DEPTH)),
            node.role,
            node.name,
            if node.name.is_empty() || node.value.is_empty() {
                ""
            } else {
                " : "
            },
            node.value
        );
        let available = max_bytes.saturating_sub(result.len());
        if line.len() > available {
            let mut end = available;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            result.push_str(&line[..end]);
            return (result, true);
        }
        result.push_str(&line);
    }
    (result, false)
}

#[allow(non_upper_case_globals)] // Windows SDK identifier spelling.
fn role_name(kind: Option<UIA_CONTROLTYPE_ID>) -> &'static str {
    match kind {
        Some(UIA_ButtonControlTypeId) => "Button",
        Some(UIA_CalendarControlTypeId) => "Calendar",
        Some(UIA_CheckBoxControlTypeId) => "CheckBox",
        Some(UIA_ComboBoxControlTypeId) => "ComboBox",
        Some(UIA_EditControlTypeId) => "Edit",
        Some(UIA_HyperlinkControlTypeId) => "Hyperlink",
        Some(UIA_ImageControlTypeId) => "Image",
        Some(UIA_ListItemControlTypeId) => "ListItem",
        Some(UIA_ListControlTypeId) => "List",
        Some(UIA_MenuControlTypeId) => "Menu",
        Some(UIA_MenuBarControlTypeId) => "MenuBar",
        Some(UIA_MenuItemControlTypeId) => "MenuItem",
        Some(UIA_ProgressBarControlTypeId) => "ProgressBar",
        Some(UIA_RadioButtonControlTypeId) => "RadioButton",
        Some(UIA_ScrollBarControlTypeId) => "ScrollBar",
        Some(UIA_SliderControlTypeId) => "Slider",
        Some(UIA_SpinnerControlTypeId) => "Spinner",
        Some(UIA_StatusBarControlTypeId) => "StatusBar",
        Some(UIA_TabControlTypeId) => "Tab",
        Some(UIA_TabItemControlTypeId) => "TabItem",
        Some(UIA_TextControlTypeId) => "Text",
        Some(UIA_ToolBarControlTypeId) => "ToolBar",
        Some(UIA_ToolTipControlTypeId) => "ToolTip",
        Some(UIA_TreeControlTypeId) => "Tree",
        Some(UIA_TreeItemControlTypeId) => "TreeItem",
        Some(UIA_CustomControlTypeId) => "Custom",
        Some(UIA_GroupControlTypeId) => "Group",
        Some(UIA_ThumbControlTypeId) => "Thumb",
        Some(UIA_DataGridControlTypeId) => "DataGrid",
        Some(UIA_DataItemControlTypeId) => "DataItem",
        Some(UIA_DocumentControlTypeId) => "Document",
        Some(UIA_SplitButtonControlTypeId) => "SplitButton",
        Some(UIA_WindowControlTypeId) => "Window",
        Some(UIA_PaneControlTypeId) => "Pane",
        Some(UIA_HeaderControlTypeId) => "Header",
        Some(UIA_HeaderItemControlTypeId) => "HeaderItem",
        Some(UIA_TableControlTypeId) => "Table",
        Some(UIA_TitleBarControlTypeId) => "TitleBar",
        Some(UIA_SeparatorControlTypeId) => "Separator",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_uia_protection_is_authoritative_without_legacy_queries() {
        for password in [false, true] {
            assert_eq!(
                classify_password_evidence(UiaPasswordStatus::Known(password), || {
                    panic!("explicit UIA protection must not query legacy fallback")
                }),
                Some(password)
            );
        }
    }

    #[test]
    fn failed_or_malformed_uia_protection_never_uses_a_legacy_fallback() {
        assert_eq!(
            classify_password_evidence(UiaPasswordStatus::Unverified, || {
                panic!("failed UIA evidence must remain fail-closed")
            }),
            None
        );
    }

    #[test]
    fn unsupported_uia_needs_successful_legacy_role_and_state() {
        assert_eq!(
            classify_password_evidence(UiaPasswordStatus::Unsupported, || None),
            None
        );
        for role in [
            ROLE_SYSTEM_OUTLINEITEM,
            ROLE_SYSTEM_LISTITEM,
            ROLE_SYSTEM_TEXT,
            ROLE_SYSTEM_CELL,
        ] {
            assert_eq!(
                classify_password_evidence(UiaPasswordStatus::Unsupported, || Some((role, 0))),
                Some(false),
            );
        }
    }

    #[test]
    fn documented_legacy_popup_state_does_not_imply_protection() {
        assert_eq!(
            classify_password_evidence(UiaPasswordStatus::Unsupported, || {
                Some((ROLE_SYSTEM_LISTITEM, STATE_SYSTEM_HASPOPUP))
            }),
            Some(false),
        );
    }

    #[test]
    fn legacy_protected_flag_always_redacts_even_with_other_state_bits() {
        for role in [0, ROLE_SYSTEM_TEXT, ROLE_SYSTEM_LISTITEM, u32::MAX] {
            for state in [STATE_SYSTEM_PROTECTED, STATE_SYSTEM_PROTECTED | 0x0010_0004] {
                assert_eq!(
                    classify_password_evidence(UiaPasswordStatus::Unsupported, || Some((
                        role, state
                    ))),
                    Some(true),
                );
            }
        }
    }

    #[test]
    fn invalid_legacy_role_or_state_cannot_authorize_content() {
        for (role, state) in [
            (0, 0),
            (ROLE_SYSTEM_OUTLINEBUTTON + 1, 0),
            (u32::MAX, 0),
            (ROLE_SYSTEM_LISTITEM, 0x8000_0000),
        ] {
            assert_eq!(
                classify_password_evidence(UiaPasswordStatus::Unsupported, || Some((role, state))),
                None,
            );
        }
    }

    #[test]
    fn changing_password_invalidates_every_aggregate_ancestor_only() {
        let mut safe = vec![true; 5];
        let parents = vec![None, Some(0), Some(1), Some(0), Some(3)];
        invalidate_ancestors(&mut safe, &parents, 2);
        assert_eq!(safe, [false, false, false, true, true]);
    }
    #[test]
    fn text_limit_preserves_utf8_and_reports_truncation() {
        let input: Vec<u16> = "甲🦀乙".encode_utf16().collect();
        assert_eq!(bounded_utf16(&input, 6), ("甲".into(), true));
        assert_eq!(bounded_utf16(&input, 7), ("甲🦀".into(), true));
        assert_eq!(bounded_utf16(&input, 10), ("甲🦀乙".into(), false));
        assert_eq!(bounded_utf16(&input, 0), (String::new(), true));
    }
    #[test]
    fn invalid_utf16_is_safe_and_bounded() {
        assert_eq!(bounded_utf16(&[0xD800], 2), (String::new(), true));
        assert_eq!(bounded_utf16(&[0xD800], 3), ("�".into(), false));
    }
    #[test]
    fn rendered_text_has_hard_cap_and_valid_unicode() {
        let nodes = vec![ContentNode {
            name: "你好".repeat(100),
            role: "Text".into(),
            ..Default::default()
        }];
        let (text, cut) = render_nodes(&nodes, 23);
        assert!(cut && text.len() <= 23);
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
    }
}
