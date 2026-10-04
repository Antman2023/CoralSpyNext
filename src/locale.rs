//! Bounded, offline localization for program-owned messages only.
//!
//! The selected language is process-wide so background workers and native
//! dialogs agree with the interface. Never pass inspected window titles,
//! document text, form values, process paths, or provider errors to `message`.
//! Dynamic messages are formatted from paired templates at their construction
//! site, leaving every interpolated value unchanged. Previously captured
//! dynamic messages retain their capture language until the next inspection.

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);
const HELPER_LANGUAGE_ENV: &str = "CORALSPYNEXT_LANG";

/// Accept only the application's supported English identifiers. Any other
/// input selects the original Chinese language; no network translator is used.
pub fn set_language(language: &str) {
    ENGLISH.store(language_is_english(language), Ordering::Relaxed);
}

fn language_is_english(language: &str) -> bool {
    matches!(language, "en" | "en-US")
}

pub fn is_english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// A bounded value for child-process environment propagation.
pub fn language_code() -> &'static str {
    label("zh-CN", "en-US")
}

/// Helpers do not load user settings. They only accept the bounded language
/// value supplied by their parent and otherwise retain the Chinese default.
pub fn init_helper_language() {
    let language = std::env::var(HELPER_LANGUAGE_ENV).unwrap_or_default();
    set_language(&language);
}

pub fn label(zh: &'static str, en: &'static str) -> &'static str {
    label_for_language(is_english(), zh, en)
}

fn label_for_language(english: bool, zh: &'static str, en: &'static str) -> &'static str {
    if english {
        en
    } else {
        zh
    }
}

/// Select paired format literals before interpolation. Dynamic content, paths,
/// class names, error codes, and raw OS/provider details are never translated.
#[macro_export]
macro_rules! localized_format {
    (@language $english:expr, $zh:literal, $en:literal $($rest:tt)*) => {{
        if $english {
            format!($en $($rest)*)
        } else {
            format!($zh $($rest)*)
        }
    }};
    ($zh:literal, $en:literal $($rest:tt)*) => {
        $crate::localized_format!(@language $crate::locale::is_english(), $zh, $en $($rest)*)
    };
}

/// Display stable icon-source identifiers without changing stored `kind` tokens.
/// Only the program's exact source names are translated; unknown tokens survive.
pub fn icon_source_label(source: &str) -> String {
    icon_source_label_for_language(source, is_english())
}

fn icon_source_label_for_language(source: &str, english: bool) -> String {
    if !english {
        return source.to_owned();
    }
    source
        .split(" / ")
        .map(|part| match part {
            "窗口小图标2" => "Window small icon 2",
            "窗口小图标" => "Window small icon",
            "窗口大图标" => "Window large icon",
            "窗口类小图标" => "Window-class small icon",
            "窗口类大图标" => "Window-class large icon",
            "程序文件小图标" => "Program-file small icon",
            "程序文件大图标" => "Program-file large icon",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Re-render an exact known static program message. This deliberately does not
/// replace substrings or guess at dynamic content. Dynamic messages should use
/// `localized_format!` at construction instead.
pub fn message(text: &str) -> String {
    message_for_language(text, is_english())
}

fn message_for_language(text: &str, english: bool) -> String {
    for &(zh, en) in STATIC_MESSAGES {
        if english && text == zh {
            return en.to_owned();
        }
        if !english && text == en {
            return zh.to_owned();
        }
    }
    text.to_owned()
}

// Exact, program-owned static strings only. Keep stable icon source tokens,
// content roles and all inspected values out of this table.
const STATIC_MESSAGES: &[(&str, &str)] = &[
    ("所选窗口无效或已经关闭", "The selected window is invalid or has closed"),
    ("无法确认所选窗口的进程与类型；请重新选取", "Cannot verify the selected window's process and class; select it again"),
    ("无法读取 UI Automation 检查进程输出", "Cannot read output from the UI Automation inspection process"),
    ("UI Automation 返回结果超过安全大小限制", "UI Automation results exceed the safe size limit"),
    ("UI Automation 提供程序在 8 秒内未响应；检查进程已停止。目标程序可能挂起或拒绝访问。", "The UI Automation provider did not respond within 8 seconds; the inspection process was stopped. The target application may be unresponsive or denying access."),
    ("检查结果读取线程失败", "The inspection result-reader thread failed"),
    ("检查结果超过安全大小限制", "Inspection results exceed the safe size limit"),
    ("检查期间所选窗口已改变；结果已丢弃，请重新选取", "The selected window changed during inspection; results were discarded. Select it again"),
    ("检查结果与所选窗口不匹配", "Inspection results do not match the selected window"),
    ("已达到 3 秒内容采集预算；当前结果不完整。", "The 3-second content collection budget was reached; results are incomplete."),
    ("已达到 5000 个元素限制；其余内容未读取。", "The 5,000-element limit was reached; remaining content was not read."),
    ("已达到 1 MiB 内容限制；其余内容未读取。", "The 1 MiB content limit was reached; remaining content was not read."),
    ("已达到 1 MiB 内容限制；部分字段已截断。", "The 1 MiB content limit was reached; some fields were truncated."),
    ("部分元素属性无法读取；提供程序可能拒绝访问或元素已消失。", "Some element properties could not be read; the provider may be denying access or the element may have disappeared."),
    ("密码或无法确认保护状态的元素及其整个子树已跳过；不读取名称、值或文本。", "Password elements, elements with unverified protection status, and their entire subtrees were skipped; names, values, and text were not read."),
    ("已达到 32 层深度限制；更深的子元素未读取，祖先的聚合内容已跳过。", "The 32-level depth limit was reached; deeper elements were not read, and aggregated ancestor content was skipped."),
    ("子树含受保护、未验证或无法访问的元素；对应祖先的聚合名称/值/文本已跳过。", "The subtree contains protected, unverified, or inaccessible elements; aggregated ancestor names, values, and text were skipped."),
    ("内容读取达到时间或大小限制；只返回已安全读取的部分。", "Content reading reached the time or size limit; only safely read content is returned."),
    ("采集期间保护状态发生变化或不可验证；该元素和祖先聚合内容已跳过。", "Protection status changed or could not be verified during collection; the element and aggregated ancestor content were skipped."),
    ("TextPattern 文本可能超过长度限制；仅返回有界片段。", "TextPattern content may exceed the length limit; only a bounded excerpt is returned."),
    ("TextPattern 已提供，但无法读取文档内容；提供程序可能拒绝访问。", "TextPattern is available, but document content could not be read; the provider may be denying access."),
    ("TextPattern 无法返回文档范围；内容可能已不可用。", "TextPattern could not return the document range; the content may no longer be available."),
    ("ValuePattern 已提供，但值不可读取；内容可能受限或元素已消失。", "ValuePattern is available, but its value could not be read; the content may be restricted or the element may have disappeared."),
    ("提供程序使用虚拟化项目；仅采集当前暴露的元素，不自动滚动、展开或 Realize 项目。", "The provider uses virtualized items; only currently exposed elements are collected. Items are not automatically scrolled, expanded, or realized."),
    ("检测到折叠内容；仅采集提供程序目前暴露的子项，不改变目标界面。", "Collapsed content was detected; only children currently exposed by the provider are collected, without changing the target interface."),
    ("部分子元素无法访问；提供程序可能超时、拒绝访问或正在改变。祖先聚合内容已跳过。", "Some child elements could not be accessed; the provider may have timed out, denied access, or changed. Aggregated ancestor content was skipped."),
    ("Windows UI Automation · 只读、用户主动检查", "Windows UI Automation · Read-only, user-initiated inspection"),
    ("提供程序超时设置不可用；仍由独立检查进程的 8 秒硬超时保护。", "Provider timeout settings are unavailable; the separate inspection process still enforces an 8-second hard timeout."),
    ("IUIAutomation2 不可用；仍由独立检查进程的 8 秒硬超时保护。", "IUIAutomation2 is unavailable; the separate inspection process still enforces an 8-second hard timeout."),
    ("UI Automation 根元素与所选窗口进程不匹配；结果已丢弃", "The UI Automation root element does not match the selected window's process; results were discarded"),
    ("采集期间所选窗口已关闭或改变；结果已丢弃，请重新选取", "The selected window closed or changed during collection; results were discarded. Select it again"),
    ("提供程序没有返回可读内容。目标可能不支持 UI Automation、内容受限，或当前为空且没有文本模式；这不是成功的空结果。", "The provider returned no readable content. The target may not support UI Automation, its content may be restricted, or it may be empty with no text pattern; this is not a successful empty result."),
    ("结果取决于目标程序的可访问性提供程序；未暴露、虚拟化或更高权限的内容可能不可读取。", "Results depend on the target application's accessibility provider; unexposed, virtualized, or higher-privilege content may not be readable."),
    ("纯文本导出达到 1 MiB 限制；结构化节点可能包含额外字段。", "Plain-text export reached the 1 MiB limit; structured nodes may contain additional fields."),

    ("窗口句柄超出当前程序的指针宽度", "The window handle exceeds this program's pointer width"),
    ("窗口句柄不能为空", "The window handle cannot be null"),
    ("读取窗口类型", "Read window class"),
    ("[输入控件：仅元数据]", "[Input control: metadata only]"),
    ("隐私保护：不读取 Edit、RichEdit、密码及常见输入控件的文本", "Privacy protection: text from Edit, RichEdit, password, and common input controls is not read"),
    ("[子控件：仅元数据]", "[Child control: metadata only]"),
    ("隐私保护：所有子控件（含未知自定义控件）均不读取文本，仅检查元数据", "Privacy protection: only metadata is inspected for all child controls, including unknown custom controls; no text is read"),
    ("[CoralSpyNext 自身窗口]", "[CoralSpyNext window]"),
    ("自身窗口仅显示元数据，避免同步读取阻塞界面", "Only metadata is shown for this program's own windows to avoid blocking the interface"),
    ("读取缓存标题", "Read cached title"),
    ("无缓存标题；未尝试读取应用内容或控件文本", "No cached title; application content and control text were not requested"),
    ("仅读取顶层窗口的缓存标题；不读取子控件或输入文本", "Only the top-level window's cached title is read; child control and input text are not read"),
    ("；标题可能已截断（最多 2047 个 UTF-16 单元）", "; the title may be truncated (maximum 2047 UTF-16 units)"),
    ("窗口数量达到安全上限，当前显示部分结果；可使用指针直接检查未列出的窗口。", "The window count reached its safety limit; results are partial. Use the pointer to inspect windows not listed."),
    ("窗口枚举达到 2.5 秒时间上限，当前显示部分结果；可刷新或使用指针直接检查。", "Window enumeration reached the 2.5-second limit; results are partial. Refresh or inspect directly with the pointer."),
    ("窗口层级超过 32 层，过深的控件已省略；可使用指针直接检查。", "The window hierarchy exceeds 32 levels; deeper controls were omitted. Use the pointer to inspect them directly."),
    ("枚举顶层窗口", "Enumerate top-level windows"),
    ("顶层窗口枚举达到安全上限，当前显示部分结果。", "Top-level window enumeration reached its safety limit; results are partial."),
    ("读取进程名称（可能无权限或进程已退出）", "Read process name (access may be denied or the process may have exited)"),
    ("读取进程名称", "Read process name"),
    ("进程名称不可用", "Process name unavailable"),
    ("窗口已关闭或句柄无效，请重新选取", "The window has closed or its handle is invalid; select it again"),
    ("读取窗口所属进程", "Read window process"),
    ("读取窗口区域", "Read window rectangle"),
    ("读取客户区", "Read client rectangle"),
    ("检查期间窗口已关闭或发生变化，请重新选取", "The window closed or changed during inspection; select it again"),
    ("读取指针位置", "Read pointer position"),
    ("指针下没有可检查的窗口；安全桌面可能不可访问", "No inspectable window under the pointer; the secure desktop may be inaccessible"),
    ("指针下的窗口已关闭，请重新选取", "The window under the pointer has closed; select it again"),
    ("无法读取当前桌面的屏幕颜色", "Could not read screen colors on the current desktop"),
    ("此坐标无法采样；受保护的画面或安全桌面可能不可读取", "This coordinate cannot be sampled; protected content or the secure desktop may be unreadable"),
    ("读取窗口身份", "Read window identity"),
    ("读取身份期间窗口已关闭或发生变化，请重新选取", "The window closed or changed while reading its identity; select it again"),
    ("不支持的导出格式 / Unsupported export format", "Unsupported export format"),
    ("已有一个保存对话框打开，请先完成或取消它", "A save dialog is already open; finish or cancel it first"),
    ("保存路径缺少结束符", "The save path is missing its terminator"),
    ("未选择保存路径", "No save path was selected"),
    ("菜单检查仅支持 Windows", "Menu inspection requires Windows"),
    ("图标检查仅支持 Windows", "Icon inspection requires Windows"),
    ("图标保存对话框仅支持 Windows", "The icon save dialog requires Windows"),
    ("ICO 图标尺寸必须介于 1 和 256 像素之间", "ICO dimensions must be between 1 and 256 pixels"),
    ("图标 RGBA 数据长度与尺寸不符", "The icon's RGBA data length does not match its dimensions"),
    ("窗口句柄超出指针宽度", "The window handle exceeds the pointer width"),
    ("请选择有效窗口；不能使用空句柄或广播句柄", "Select a valid window; null and broadcast handles are not allowed"),
    ("窗口已关闭或句柄已失效，请重新选择", "The window has closed or its handle is no longer valid; select it again"),
    ("检查期间窗口句柄被重复使用；已丢弃结果，请重新选择", "The window handle was reused during inspection; results were discarded. Select it again"),
    ("菜单项目已达到 4096 项上限，结果已截断", "The menu reached the 4096-item limit; results were truncated"),
    ("菜单读取超过 2 秒预算，结果已截断", "Menu inspection exceeded its 2-second budget; results were truncated"),
    ("菜单层级已达到 32 层上限，深层项目已省略", "The menu reached the 32-level depth limit; deeper items were omitted"),
    ("检测到重复菜单句柄，已跳过重复分支", "A repeated menu handle was detected; the duplicate branch was skipped"),
    ("读取菜单项目数量", "Read menu item count"),
    ("部分菜单项目读取失败；目标菜单可能正在变化", "Some menu items could not be read; the target menu may be changing"),
    ("部分菜单文本超过 2047 个 UTF-16 单元，已截断", "Some menu text exceeds 2047 UTF-16 units and was truncated"),
    ("部分菜单文本读取失败；目标菜单可能正在变化", "Some menu text could not be read; the target menu may be changing"),
    ("[自绘菜单项：未提供文字]", "[Owner-drawn menu item: no text provided]"),
    ("[无文字菜单项]", "[Menu item without text]"),
    ("菜单在读取期间发生变化；子菜单结果可能不完整", "The menu changed during inspection; submenu results may be incomplete"),
    ("弹出菜单未提供可访问的原生菜单句柄，请使用窗口内容页的 UI Automation 读取可访问菜单项", "The popup did not provide an accessible native menu handle; use UI Automation on the Content page to read accessible menu items"),
    ("该窗口没有传统菜单栏；以下为窗口系统菜单（并非应用菜单栏）", "This window has no traditional menu bar; the following items are from its window system menu, not the application's menu bar"),
    ("未提供原生菜单；自绘、浏览器和现代应用菜单可在窗口内容页尝试 UI Automation", "No native menu was provided; try UI Automation on the Content page for owner-drawn, browser, and modern application menus"),
    ("图标位图信息无效", "Invalid icon bitmap information"),
    ("图标位图尺寸超过安全上限", "Icon bitmap dimensions exceed the safety limit"),
    ("读取图标像素", "Read icon pixels"),
    ("读取图标位图", "Read icon bitmap"),
    ("图标没有有效的透明蒙版", "The icon has no valid transparency mask"),
    ("单色图标的 AND/XOR 蒙版高度无效", "Invalid AND/XOR mask height for the monochrome icon"),
    ("图标超过 256 × 256 像素上限，已跳过", "The icon exceeds the 256 × 256 pixel limit and was skipped"),
    ("图标颜色位图与透明蒙版尺寸不一致", "The icon color bitmap and transparency mask have different dimensions"),
    ("创建图标读取设备上下文", "Create an icon-reading device context"),
    ("图标含旧式背景反色像素；RGBA 预览和导出以黑底效果近似显示这些像素", "The icon contains legacy background-inversion pixels; the RGBA preview and export approximate these pixels against a black background"),
    ("复制图标", "Copy icon"),
    ("已跳过程序文件图标：仅允许本地盘符绝对路径，不读取网络、设备或特殊路径", "Program-file icons were skipped: only absolute local drive-letter paths are allowed; network, device, and special paths are not read"),
    ("图标读取已达到 2 秒预算，已跳过程序文件图标", "Icon inspection reached its 2-second budget; program-file icons were skipped"),
    ("已跳过程序文件图标：驱动器不是已确认的本地固定磁盘（网络、可移动或未知磁盘均不读取）", "Program-file icons were skipped: the drive is not a confirmed local fixed disk; network, removable, and unknown drives are not read"),
    ("已跳过程序文件图标：无法确认本地路径属性", "Program-file icons were skipped: local path attributes could not be verified"),
    ("已跳过程序文件图标：路径包含重解析点、离线文件或需要下载的云端文件", "Program-file icons were skipped: the path contains a reparse point, offline file, or cloud file requiring download"),
    ("已跳过程序文件图标：仅读取不超过 64 MiB 的非空本地程序文件", "Program-file icons were skipped: only nonempty local program files up to 64 MiB are read"),
    ("已跳过程序文件图标：父路径不是目录", "Program-file icons were skipped: the parent path is not a directory"),
    ("读取程序图标路径", "Read program icon path"),
    ("程序图标路径长度无效", "Invalid program icon path length"),
    ("本地程序文件图标读取超过 2 秒；Windows 磁盘和资源读取调用无法在进程内强制中止", "Local program-file icon inspection exceeded 2 seconds; Windows disk and resource-reading calls cannot be forcibly stopped within the process"),
    ("提取程序图标", "Extract program icon"),
    ("图标读取已达到 2 秒预算，已停止其他查询", "Icon inspection reached its 2-second budget; remaining queries were stopped"),
    ("窗口图标查询超时或被目标拒绝；继续尝试窗口类图标", "The window icon query timed out or was rejected; trying window-class icons instead"),
    ("自身窗口跳过同步图标消息，仅尝试窗口类和程序文件图标", "Synchronous icon messages are skipped for this program's own windows; only window-class and program-file icons are tried"),
    ("该窗口未提供可读取图标，或当前权限不允许访问", "This window provided no readable icon, or access is not permitted at the current privilege level"),
    ("已有图标保存对话框打开，请先完成或取消它", "An icon save dialog is already open; finish or cancel it first"),
    ("[密码内容受保护，未读取]", "[Password content protected; not read]"),
    ("窗口已关闭或句柄无效", "The window has closed or its handle is invalid"),
    ("无法确认所选窗口的身份", "Could not verify the selected window's identity"),
    ("不检查检查器自身的 HTML 控件", "The inspector's own HTML controls are not inspected"),
    ("MSHTML 检查结果与所选窗口不一致", "The MSHTML inspection result does not match the selected window"),
    ("框架序号超过 64 个框架的检查上限", "The frame index exceeds the 64-frame inspection limit"),
    ("操作期间所选窗口已改变，请重新选取。若已请求导航，请先查看目标窗口。", "The selected window changed during the action; select it again. If navigation was requested, check the target window first."),
    ("辅助请求过大", "The helper request is too large"),
    ("无法写入辅助进程", "Could not write to the helper process"),
    ("无法读取辅助进程输出", "Could not read helper process output"),
    ("返回结果超过安全大小上限", "The returned result exceeds the safety size limit"),
    ("下载已停止，未替换目标文件；临时片段将被清理。", "The download was stopped without replacing the destination file; temporary fragments will be cleaned up."),
    ("若请求了导航或高亮，可能已有部分变化；请先检查目标窗口。", "If navigation or highlighting was requested, partial changes may already have occurred; check the target window first."),
    ("输出读取线程异常", "The output reader thread failed"),
    ("请求写入线程异常", "The request writer thread failed"),
    ("结果超过安全大小上限", "The result exceeds the safety size limit"),
    ("无法注册旧版 MSHTML 查询消息", "Could not register the legacy MSHTML query message"),
    ("所选 MSHTML 控件未返回文档接口。可能不支持此历史兼容通道、未加载文档、已挂起或拒绝访问；不会绕过权限。", "The selected MSHTML control did not return a document interface. It may not support this legacy channel, may have no loaded document, may be hung, or may deny access. Permissions will not be bypassed."),
    ("MSHTML 返回了空文档接口", "MSHTML returned a null document interface"),
    ("DOM 返回空对象", "DOM returned a null object"),
    ("DOM 返回值不是对象", "The DOM return value is not an object"),
    ("DOM 返回值不是文本", "The DOM return value is not text"),
    ("无法读取 DOM 文本", "Could not read DOM text"),
    ("资源地址不是文本", "The resource address is not text"),
    ("无法读取资源地址", "Could not read the resource address"),
    ("资源地址超过 8192 字符，已跳过以避免截断后下载错误地址", "The resource address exceeds 8192 characters; it was skipped to avoid downloading an incorrectly truncated address"),
    ("资源地址超过 8192 字节，已跳过以避免截断后下载错误地址", "The resource address exceeds 8192 bytes; it was skipped to avoid downloading an incorrectly truncated address"),
    ("DOM 集合长度不是整数", "The DOM collection length is not an integer"),
    ("DOM 集合长度无效", "Invalid DOM collection length"),
    ("宿主未暴露浏览器服务；此操作不可用", "The host does not expose a browser service; this action is unavailable"),
    ("宿主未暴露顶层浏览器服务；此操作不可用", "The host does not expose a top-level browser service; this action is unavailable"),
    ("宿主没有公开 WebBrowser 应用接口；此操作不可用", "The host does not expose the WebBrowser application interface; this action is unavailable"),
    ("框架序号超过检查上限", "The frame index exceeds the inspection limit"),
    ("此框架已不存在，请重新检查", "This frame no longer exists; inspect it again"),
    ("此页使用历史 MSHTML 兼容接口，只适用于宿主已有的 Internet Explorer_Server；不会安装或恢复 IE / Flash。", "This page uses the legacy MSHTML compatibility interface, only for a host's existing Internet Explorer_Server control. IE / Flash will not be installed or restored."),
    ("仅列出前 64 个直接子框架；不会递归遍历框架。", "Only the first 64 direct child frames are listed; frames are not traversed recursively."),
    ("框架枚举达到时间上限，列表可能不完整。", "Frame enumeration reached its time limit; the list may be incomplete."),
    ("无法读取宿主可执行文件路径。", "Could not read the host executable path."),
    ("检查期间目标窗口身份已改变；结果已丢弃", "The target window's identity changed during inspection; results were discarded"),
    ("输入控件超过 5000 个；表单已截断，无法完成密码检测，源码已隐藏。", "There are more than 5000 input controls; the form list was truncated, password detection could not finish, and source was hidden."),
    ("[无法读取]", "[Unreadable]"),
    ("源码不是字符串", "The source is not a string"),
    ("无法读取 HTML 字符串", "Could not read the HTML string"),
    ("HTML 源码已截断至 1 MiB。", "HTML source was truncated to 1 MiB."),
    ("检测到密码输入控件，或未能完整确认其状态：整份 HTML 源码已隐藏，密码值从未请求。", "A password input control was detected or its state could not be fully verified: all HTML source was hidden, and password values were never requested."),
    ("表单字段达到 5000 项上限，列表已截断。", "The form field list reached the 5000-item limit and was truncated."),
    ("表单值来自脱离 DOM 的副本；某些旧宿主的克隆只保留默认值，未必保留刚修改的实时值。单个文本限 4096 字节。", "Form values come from a detached DOM copy. Some legacy hosts preserve only default values in clones, rather than recently edited live values. Each text value is limited to 4096 bytes."),
    ("链接与资源达到 5000 项上限，列表已截断。", "The links and resources list reached the 5000-item limit and was truncated."),
    ("DOM 检查达到 4 秒或文本总量 1 MiB 上限；部分内容未读取。", "DOM inspection reached the 4-second or 1 MiB total-text limit; some content was not read."),
    ("Flash 项仅为页面已有的资源地址；不加载、播放、启用或安装 Flash。", "Flash items are only resource addresses already present in the page; Flash is not loaded, played, enabled, or installed."),
    ("页面高亮需要文字、前景色、背景色与粗体参数。请使用 IE 页的文字高亮按钮；此无参数导航命令不能执行高亮。", "Page highlighting requires text, foreground color, background color, and bold parameters. Use the text-highlight button on the IE page; this parameterless navigation command cannot highlight text."),
    ("文档拒绝了 Stop 命令；加载可能仍在继续。", "The document rejected the Stop command; loading may still be in progress."),
    ("宿主未提供 WebBrowser.GoHome；框架没有独立主页。为避免跳转到猜测的地址，此操作不可用。", "The host does not provide WebBrowser.GoHome, and frames have no independent home page. This action is unavailable to avoid navigating to a guessed address."),
    ("高亮期间窗口已改变；请先检查目标窗口。", "The window changed during highlighting; check the target window first."),
    ("请先输入要高亮的文字", "Enter the text to highlight first"),
    ("高亮文字最多 1024 字节，且不能包含空字符", "Highlight text must be at most 1024 bytes and contain no null characters"),
    ("宿主没有应用所请求的粗体状态", "The host did not apply the requested bold state"),
    ("已有下载保存对话框打开，请先完成或取消它", "A download save dialog is already open; finish or cancel it first"),
    ("保存路径无结束符", "The save path is missing its terminator"),
    ("保存路径无法表示为 Unicode，请选择另一个文件名", "The save path cannot be represented as Unicode; choose another filename"),
    ("无效的保存文件名", "Invalid save filename"),
    ("不允许保存为可执行程序、脚本、快捷方式或备用数据流。请选择普通资源文件名。", "Saving as an executable, script, shortcut, or alternate data stream is not allowed. Choose an ordinary resource filename."),
    ("保存目录无效", "Invalid save directory"),
    ("系统时钟无效", "Invalid system clock"),
    ("临时路径无法表示为 Unicode", "The temporary path cannot be represented as Unicode"),
    ("下载文件大小核对失败；没有覆盖目标文件", "The downloaded file's size could not be verified; the destination file was not overwritten"),
    ("下载地址过长或包含无效字符", "The download address is too long or contains invalid characters"),
    ("无法解析下载地址；仅支持完整 HTTP(S) URL", "Could not parse the download address; only complete HTTP(S) URLs are supported"),
    ("下载只支持 HTTP 和 HTTPS；不打开 file、javascript、data 或其他协议", "Downloads support only HTTP and HTTPS; file, javascript, data, and other schemes are not opened"),
    ("下载地址不能内含用户名或密码；不会自动登录", "The download address must not contain a username or password; automatic login is not used"),
    ("下载地址缺少主机名", "The download address has no host name"),
    ("不下载可执行程序、脚本或快捷方式地址", "Executable, script, and shortcut addresses are not downloaded"),
    ("临时保存路径无效", "Invalid temporary save path"),
    ("初始化 HTTP", "Initialize HTTP"),
    ("设置 HTTP 超时", "Set HTTP timeouts"),
    ("连接服务器", "Connect to server"),
    ("创建 HTTP 请求", "Create HTTP request"),
    ("禁用自动登录、Cookie 和跳转", "Disable automatic login, cookies, and redirects"),
    ("禁用自动凭据", "Disable automatic credentials"),
    ("发送下载请求", "Send download request"),
    ("接收服务器响应", "Receive server response"),
    ("服务器未返回有效 HTTP 状态码", "The server did not return a valid HTTP status code"),
    ("资源超过 50 MiB 下载上限", "The resource exceeds the 50 MiB download limit"),
    ("服务器返回可执行程序或脚本类型，已停止下载", "The server returned an executable or script content type; the download was stopped"),
    ("下载达到 25 秒流式读取上限", "The download reached the 25-second streaming-read limit"),
    ("读取资源", "Read resource"),
    ("资源内容为可执行文件或脚本，已停止保存", "The resource content is an executable or script; saving was stopped"),
    ("响应未完整下载；没有替换目标文件", "The response was not fully downloaded; the destination file was not replaced"),
    ("[受保护内容：已跳过]", "[Protected content: skipped]"),
    ("[无法确认密码保护状态：已跳过]", "[Password protection status could not be verified: skipped]"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_selection_is_bounded() {
        assert!(language_is_english("en-US"));
        assert!(language_is_english("en"));
        for other in ["zh-CN", "", "en-US-extra", "EN", "../../en", "日本語"] {
            assert!(!language_is_english(other));
        }
        assert_eq!(label_for_language(true, "中文", "English"), "English");
        assert_eq!(label_for_language(false, "中文", "English"), "中文");
    }

    #[test]
    fn static_messages_translate_exactly_and_round_trip() {
        let zh = "窗口句柄不能为空";
        let en = "The window handle cannot be null";
        assert_eq!(message_for_language(zh, true), en);
        assert_eq!(message_for_language(en, false), zh);
        assert_eq!(message_for_language(zh, false), zh);
        assert_eq!(message_for_language(en, true), en);
    }

    #[test]
    fn inspected_text_and_unknown_messages_are_not_rewritten() {
        for raw in [
            "用户标题：窗口句柄不能为空",
            r"C:\用户\无法读取\程序.exe",
            "<input value='窗口句柄不能为空'>",
            "Windows 提供程序返回的原始错误",
            "窗口大图标 / 窗口类大图标",
        ] {
            assert_eq!(message_for_language(raw, true), raw);
            assert_eq!(message_for_language(raw, false), raw);
        }
    }

    #[test]
    fn formatted_context_preserves_codes_paths_and_os_details() {
        let code = 5u32;
        let path = r"C:\用户\窗口句柄不能为空\应用.exe";
        let detail = "拒绝访问 (os error 5)";
        let output = crate::localized_format!(
            @language true,
            "无法保存 {path}（Win32 {code}：{detail}）",
            "Could not save {path} (Win32 {code}: {detail})"
        );
        assert_eq!(output, format!("Could not save {path} (Win32 5: {detail})"));
        assert!(output.contains(path));
        assert!(output.contains(detail));
        let error = 0x3002u32;
        assert_eq!(
            crate::localized_format!(
                @language true,
                "保存对话框失败（通用对话框错误 0x{error:08X}）",
                "Save dialog failed (common dialog error 0x{error:08X})"
            ),
            "Save dialog failed (common dialog error 0x00003002)"
        );
    }

    #[test]
    fn icon_source_display_preserves_stored_tokens_and_unknown_names() {
        let stored = "窗口大图标 / 窗口类大图标";
        assert_eq!(
            icon_source_label_for_language(stored, true),
            "Window large icon / Window-class large icon"
        );
        assert_eq!(icon_source_label_for_language(stored, false), stored);
        assert_eq!(icon_source_label_for_language("测试图标", true), "测试图标");
    }

    #[test]
    fn anonymous_and_named_format_arguments_keep_their_values() {
        let matches = 13;
        let class = "用户自定义窗口";
        assert_eq!(
            crate::localized_format!(@language true, "框架 {}", "Frame {}", 7),
            "Frame 7"
        );
        assert_eq!(
            crate::localized_format!(
                @language true,
                "{class}：{matches} 个",
                "{class}: {matches} items"
            ),
            "用户自定义窗口: 13 items"
        );
    }
}
