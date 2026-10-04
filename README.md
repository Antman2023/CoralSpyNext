# CoralSpyNext

用 Rust 编写的 Windows 11 x64 原生窗口检查与屏幕取色工具。灵感来自经典 CoralSpy，界面和代码均重新实现。

## 这一版可以做什么

- **窗口树**：枚举当前桌面的顶层窗口与子窗口，按标题、类名、PID、HWND 搜索。
- **准星检查**：开始选择后把指针移到目标，按 **Ctrl** 锁定；**Esc** 取消。不会点击或修改目标控件。
- **窗口详情**：64 位 HWND、父句柄、系统缓存标题、类名、PID/TID、进程文件名、屏幕位置/大小、客户区、DPI、可见/启用状态和窗口样式。
- **屏幕取色**：读取指针所在屏幕像素，显示 HEX、RGB 和 Windows COLORREF，可复制与查看本次会话历史。
- **导出**：复制结果，或通过 Windows 保存对话框导出 UTF-8 文本/JSON。
- **中文界面**：深色/浅色主题；读取系统已安装的中文字体；Per-Monitor V2 DPI 清单。

所有检查均由用户主动操作触发。程序不联网、不写入配置或后台运行，不要求管理员权限。关闭程序后会话数据即消失；只有主动保存的报告留在所选位置。

## 与旧版功能的对应关系

这是**实用核心重写版，不是旧版全部功能的完整复刻**。

| 旧版功能 | CoralSpyNext 0.1.0 |
| --- | --- |
| 准星选中窗口、鼠标位置、句柄/类名/标题 | 已有；改为 Ctrl 锁定，标题只读顶层系统缓存 |
| 颜色拾取与十进制/十六进制/RGB | 已有；提供 HEX、RGB、COLORREF 和会话历史 |
| 复制/保存标题与信息 | 已有；窗口元数据文本/JSON 报告 |
| ListBox/ComboBox、ListV、TreeV 内容读取 | 未实现；现有窗口树显示 HWND 层级，不是目标 TreeView 的条目内容 |
| RichEdit 内容、图标提取 | 未实现 |
| IE/IE2 源码、框架、表单、链接/图片/Flash 提取 | 未实现 |
| 菜单捕获/菜单条目读取 | 未实现 |
| 密码显示 | 未实现；输入内容不在本版范围 |
| 全局热键注册 | 未实现；仅应用内 F5/Ctrl+F 和主动拾取期间 Ctrl/Esc |
| 托盘常驻、置顶、鼠标指针恢复 | 未实现；本版不替换系统鼠标指针 |
| 英文/中文切换、IE 高亮设置 | 未实现；当前为中文界面 |

额外加入了 PID/TID、进程文件名、DPI、窗口样式、明暗主题、异步检查和状态错误提示。

## 下载和运行

本仓库提供完整可构建源码。便携预览包由维护者单独提供：取得 `CoralSpyNext-windows-x64.zip` 后，解压并运行 `coralspynext.exe`。包内还有说明和 SHA-256 校验值；仓库当前没有 GitHub Release 或活动 CI 产物。

初版可执行文件通过 Linux → Windows GNU 交叉编译生成，尚未在 Windows 11 桌面交互测试。

目标系统：**Windows 11 x64**。这是便携程序，不需要安装，也不需要旧 CoralSpy 的任何文件。

程序尚未进行代码签名。请仅运行你信任的构建，保留系统安全防护；如安全软件阻止运行，可以从源码自行构建。不要为此关闭安全软件。

## 使用

1. 启动后刷新窗口列表，选择一个窗口查看详情；搜索可缩小范围。
2. 想直接找到屏幕上的窗口，点击窗口选择按钮，然后移动指针到目标。松开原先按住的 Ctrl，再按一次 Ctrl 锁定；按 Esc 放弃选择。
3. 在取色页使用同样方式选取屏幕像素，再复制 HEX/RGB。
4. 检查结果可能含窗口标题和应用名称。导出或分享报告前请检查其中是否含私人信息。

选择模式只轮询 Ctrl/Esc 的当前状态，不记录按键、安装全局钩子或阻断输入。Ctrl 会正常传递给系统；请在目标应用空闲时使用。窗口本身不会被点击。

## 范围与已知限制

- **这不是原软件二进制重打包，也不是演示数据界面。** 窗口和颜色来自 Windows API。
- 不读取密码、输入框/富文本内容，不读取其他进程内存，不注入 DLL，不绕过权限保护。输入类控件仅显示元数据；自有窗口标题也不做同步读取以避免阻塞。
- 显示的是 HWND 窗口树。浏览器、Electron、UWP/WinUI 等界面的每个视觉元素未必有独立 HWND，不能据此承诺逐个检查所有现代 UI 元素；当前未集成 UI Automation。
- 无权限、目标关闭或句柄被复用时会报错/要求刷新；不可访问的进程名可能显示为无法读取。不会自动提权。
- 普通用户无法检查安全桌面、某些受保护窗口。屏幕取色不等同于应用内部原始颜色；HDR、色彩管理、受保护视频、透明层和远程桌面可能影响读数。
- 枚举最多 6,000 个窗口、32 层且有 2.5 秒预算；达到上限会提示结果不完整。窗口标题最多读取 2,047 个 UTF-16 单元。
- 列表为刷新时快照；详情为选择/刷新时快照，不持续监控所有应用。
- 目前不包含旧版的 IE/Flash 提取、列表内容抓取、密码显示、消息钩子和系统托盘常驻功能。
- eframe 使用 OpenGL 渲染，需要可用的图形驱动。GUI 初始化失败时会提供错误提示。

## 从源码构建

安装 [Rust 官方工具链](https://www.rust-lang.org/tools/install) 与 Visual Studio Build Tools 的 “使用 C++ 的桌面开发”（含 Windows SDK），在 Windows PowerShell 中执行：

```powershell
git clone https://github.com/Antman2023/CoralSpyNext.git
cd CoralSpyNext
cargo build --locked --release
.\target\release\coralspynext.exe
```

使用 Rust 1.99.0，依赖由 `Cargo.lock` 固定。推荐 Windows 原生 MSVC 构建。Linux 可以运行平台无关模型测试；GNU 交叉编译需要 MinGW-w64：

```sh
rustup target add x86_64-pc-windows-gnu
cargo test --locked --lib --tests
cargo build --locked --release --target x86_64-pc-windows-gnu
```

源码检查：

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

## Windows CI 模板

`ci/windows.yml.example` 可复制为 `.github/workflows/windows.yml` 后启用 Windows MSVC 构建。当前发布凭据没有 workflow 写入权限，因此未上传活动工作流，也未扩大账户权限。模板本身不会运行。

## 验证说明

自动构建能够验证 Rust 类型、Win32 链接、资源清单、代码规范和单元测试，**不能替代 Windows 11 桌面实际交互验收**。初版是在 Linux 环境开发，未在用户的 Windows 11 实机上运行。详细验收清单见 [TESTING.md](TESTING.md)。

## 项目结构

- `src/platform.rs`：有边界的只读 Win32 后端
- `src/app.rs`：egui 原生界面、后台工作线程和选择状态机
- `src/model.rs`：数据模型、格式化与过滤
- `assets/app.manifest`：DPI 感知及 asInvoker 权限声明
- `ci/windows.yml.example`：Windows 构建、检查和打包模板（当前未启用）

MIT 许可证。旧版名称及历史归原作者所有，来源与依赖见 [THIRD_PARTY.md](THIRD_PARTY.md)。
