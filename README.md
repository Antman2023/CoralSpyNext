# CoralSpyNext

Rust 编写的 Windows 11 x64 窗口检查工具。0.2 按经典 CoralSpy 的紧凑结构重新安排界面：顶部工具栏、左侧属性、右侧拖动准星，详情/取色/选项使用独立窗口，默认浅色。

这是独立重写项目，不包含旧 CoralSpy 程序、DLL 或图标。目标是逐项覆盖经典操作，并明确现代 Windows 的能力边界。**“能编译”不等于已经完成原版逐行为、逐像素或 Windows 11 实机验收。**

## 功能

- **窗口拾取与属性**：拖动右侧准星，释放锁定；Esc 取消。Ctrl 备用拾取和按两次全局准星热键互不混用。Unicode 标题、x64 HWND、类名、鼠标坐标、PID/TID、进程文件名、矩形、DPI、样式与状态。
- **常规 / ListV / TreeV / RichEdit**：通过系统 UI Automation 的公开 Text、Value、LegacyIAccessible 等模式读取用户主动选择的控件。支持条目、层级、可访问文本、复制、保存和本地展开/收起；不会用系统 HWND 树冒充目标控件内容。
- **菜单**：原生菜单栏、系统菜单及 `#32768` 弹出菜单，包含层级、ID、禁用、勾选、分隔符。未知自绘菜单可能只能通过目标应用公开的可访问性内容取得信息。
- **图标**：窗口、类、受限本地可执行文件资源回退，透明预览与真正 ICO 导出。
- **IE / IE2 历史兼容**：对已存在的 `Internet Explorer_Server` / MSHTML 宿主读取文档、可访问框架、源码、表单、链接、图片及 Flash 资源引用；宿主支持时可返回/前进/停止/刷新/主页和 TextRange 高亮。不会安装 IE/Flash，也不会把 Edge/Chrome 的 HWND 当作 DOM 接口。
- **资源下载**：仅在用户点击下载并选择位置后发起 HTTP(S) 请求；不带 cookies 或认证，不跟随重定向，不关闭证书检查；30秒/50MiB 上限，通常失败会清理临时文件，成功才替换目标文件。强制结束/崩溃时可能在目标目录残留唯一命名的 .part 临时文件，原目标文件保持不变。
- **取色**：独立紧凑窗口，准星取屏幕颜色、Windows COLORREF 十进制/十六进制、HTML HEX、RGB 和色块选择。
- **托盘 / 置顶 / 热键**：本程序置顶开关、原生托盘菜单、显示主窗口/取色器/两次准星的三组可配置全局热键。冲突明确报错，不抢占其他程序已经注册的热键。程序退出后注销。
- **选项**：中文/English、明暗外观、热键、托盘及 IE 高亮颜色/粗体/预览。只持久化选项，不保存捕获的窗口/控件内容。

当前实际覆盖和仍待补的细节见 [PARITY_STATUS.md](docs/PARITY_STATUS.md)。原版 89 个可恢复界面入口的逐项清单见 [ORIGINAL_FEATURE_INVENTORY.md](docs/ORIGINAL_FEATURE_INVENTORY.md)。它描述对应项与验收要求，不是未执行测试的通过报告。

## 必须知道的边界

1. **密码与受保护内容**：只标记保护/不可用，不读取密码值，不注入 DLL、不读进程内存、不提权或绕过访问限制。UIA 先检查整个子树保护属性，保护状态未知或采集不完整时抑制可能聚合私密内容的父节点文本。
2. **虚拟化/折叠控件**：仅能保存提供程序实际公开、已采集的条目。不会自动滚动、展开或 Realize 目标界面；“保存全部”指全部已捕获数据。数量和截断警告必须一同理解，不能声称抓到了应用的隐藏全量内容。
3. **RichEdit**：可读取公开文本并导出合法 Unicode RTF。提供程序没有公开的原始 RTF 格式、嵌入对象和所有排版不能凭空恢复；文本 RTF 不等于原文档格式的逐字节复制。
4. **IE/Flash**：这是可选历史 MSHTML 兼容路径，宿主和系统行为决定可用性。现代 Microsoft 文档不把该旧桥接路径视作通用现代浏览器客户端接口。拒绝、跨源错误、无宿主能力时显示原因，不绕过保护，不运行 Flash。源代码在密码检测不确定时不导出；克隆 DOM 的表单值可能反映默认值而非所有宿主的实时编辑状态。
5. **未实机验收**：初版开发/构建在 Linux 完成。Windows 类型检查、链接和 fixture 编译不证明真实 Windows 11 上所有控件、显卡、DPI、托盘和浏览器宿主都正常。见 [TESTING.md](TESTING.md)。

其他限制：HWND 身份检查是尽力而为；同一进程/类的极快句柄复用无法完全证明身份。屏幕颜色受 HDR、色彩管理、透明叠加、受保护视频和远程桌面影响。图标的网络/可移动/未知卷与大型资源回退会跳过。UIA 有 8 秒进程截止，菜单枚举/图标读取和条目都有预算与上限，失败明确显示。

## 运行

便携预览包由维护者单独提供。将 `CoralSpyNext-v0.2.0-windows-x64.zip` 整包解压，运行 `coralspynext.exe`；不需要旧软件文件，不要求管理员权限。包内包含源码、说明、SHA-256 与构建信息。

程序未签名。只运行可信构建，不要关闭系统安全防护；也可以审阅源码后自行构建。程序除明确资源下载操作外不联网。报告可能含私人应用内容，分享前请检查。

设置位置为 `%LOCALAPPDATA%\CoralSpyNext\settings.json`。内容、窗口标题、网页源码、颜色历史不自动落盘；仅用户选择导出时写入选定位置。

## 使用习惯

- 主窗右侧拖动准星到目标后释放；Esc 中止。为避免误触目标，不需要点击目标按钮。
- 点击“详情”，选原版页签，按“读取详情”；更换目标后重新读取。
- 主工具栏提供精确复制/保存标题；详情页提供对应内容的复制/保存。
- 默认全局热键：Ctrl+Alt+W 主窗，Ctrl+Alt+C 取色，Ctrl+Alt+S 开始/结束准星。可在选项里改键或关闭。
- 遇到“截断 / 不可访问 / 提供程序不支持”，不要把空结果当作目标真的没有内容。

## 构建

安装 [Rust 官方工具链](https://www.rust-lang.org/tools/install) 和 Visual Studio Build Tools 的“使用 C++ 的桌面开发”（含 Windows SDK），在 Windows PowerShell 执行：

```powershell
git clone https://github.com/Antman2023/CoralSpyNext.git
cd CoralSpyNext
cargo build --locked --release
.\target\release\coralspynext.exe
```

Rust 1.99.0 与 Cargo.lock 锁定依赖。推荐 Windows MSVC 构建。Linux 可运行纯函数测试，交叉编译需 MinGW-w64：

```sh
rustup target add x86_64-pc-windows-gnu
cargo test --locked --all-targets
cargo build --locked --release --target x86_64-pc-windows-gnu
```

检查：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked --all-targets`。

## Windows CI

`ci/windows.yml.example` 是非活动模板。维护者可将其复制为 `.github/workflows/windows.yml` 后启用 MSVC 构建。当前发布凭据无 workflow 写入权限，未扩权、未启用 Windows runner；仓库没有已通过的 Windows CI 可供声称。

## 代码结构

`platform` 元数据；`accessibility` 受限 UIA helper；`extras` 菜单/图标；`legacy` 历史 MSHTML 与明确下载；`desktop` 托盘/热键；`config` 选项；`content_view` 与 `formats` 数据视图/导出；`app` 经典布局。

MIT 许可证。原版 CoralSpy 为 Coral Studio 的历史作品，本项目不冒充原作者或声称获得其背书。依赖来源见 [THIRD_PARTY.md](THIRD_PARTY.md)。
