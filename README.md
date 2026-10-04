<p align="center">
  <img src="assets/coralspynext.png" width="112" height="112" alt="CoralSpyNext 应用图标">
</p>

# CoralSpyNext

[![构建、验证与发布](https://github.com/Antman2023/CoralSpyNext/actions/workflows/release.yml/badge.svg)](https://github.com/Antman2023/CoralSpyNext/actions/workflows/release.yml)

**面向 Windows 11 x64 的窗口与控件检查工具，使用 Rust 独立重写。**

保留经典 CoralSpy 的紧凑操作布局：顶部工具栏、左侧窗口属性、右侧拖动准星，详情、取色器和选项各自使用独立窗口。用熟悉的方式定位窗口，查看控件内容、菜单、图标与屏幕颜色。

[下载](https://github.com/Antman2023/CoralSpyNext/releases) · [快速上手](#快速上手) · [从源码构建](#从源码构建) · [功能对应表](docs/PARITY_STATUS.md) · [反馈问题](https://github.com/Antman2023/CoralSpyNext/issues)

> 当前源码版本为 **0.3.1**，仍待 CI 验证与 `v0.3.1` 标签发布。Linux 检查和 GNU 交叉构建已执行；Windows 11 实机交互验收尚未完成。发布状态请以 Releases 和对应提交的 Actions 记录为准。

## 下载与运行

1. 打开 [Releases](https://github.com/Antman2023/CoralSpyNext/releases)，选择已发布版本的 `CoralSpyNext-v<版本>-windows-x64.zip`。如尚无发布包，可按下文从源码构建。
2. **完整解压 ZIP**，保留以下三个程序在同一目录。
3. 运行 `coralspynext.exe`。无需安装，也不需要旧版 CoralSpy 文件或管理员权限。

```text
CoralSpyNext-v<版本>-windows-x64/
├── coralspynext.exe               主程序（x64）
├── coralspy-hook-broker-x86.exe   固定用途的 x86 检查组件
└── coralspy-hook-broker-x64.exe   固定用途的 x64 检查组件
```

发布包同时提供源码、依赖许可证、SHA-256 校验和及构建信息。两个辅助程序各自内嵌对应架构的固定 DLL，无需另行安装 DLL。x86 辅助程序用于检查 32 位目标，主程序仍需在 x64 Windows 上运行。

程序目前**未签名**。请只运行可信来源的构建，并核对发布的校验和；不要为运行程序关闭系统安全防护。

## 快速上手

1. **选取窗口**：将主窗口右侧的准星拖到目标窗口或控件上，释放后锁定；按 `Esc` 取消。
2. **查看属性**：直接读取标题、类名、句柄、进程、坐标、尺寸等信息。工具栏可复制或保存标题。
3. **读取详情**：打开“详情”，切换到对应页签并点击“读取详情”。更换目标后请重新读取，再复制或保存所需内容。
4. **屏幕取色**：打开取色器，用准星选取屏幕颜色，查看 RGB、HTML HEX 或 Windows COLORREF。

默认全局热键可在“选项”中修改或关闭；发生占用冲突时会提示，不会抢占其他程序的热键。

| 热键 | 操作 |
| --- | --- |
| `Ctrl+Alt+W` | 显示主窗口 |
| `Ctrl+Alt+C` | 显示取色器 |
| `Ctrl+Alt+S` | 第一次开始准星拾取，第二次结束并锁定 |

看到“截断”“不可访问”或“不支持”时，请结合提示判断结果；空结果不一定表示目标没有内容。

## 功能概览

- **窗口属性**：Unicode 标题、x64 HWND、类名、PID/TID、进程文件名、窗口矩形、鼠标坐标、DPI、样式与状态。
- **控件内容**：通过系统 UI Automation 读取目标公开的文本、列表和树形内容，支持复制、保存及本地视图展开/收起。
- **原生控件检查**：经明确确认后，通过固定用途 Hook 读取 ListView 列标题与单元格、TreeView 层级、菜单状态，以及 RichEdit 原始 RTF 流。
- **菜单与图标**：查看原生菜单栏、系统菜单和弹出菜单的层级、ID、勾选及禁用状态；预览窗口/类图标并导出 ICO。
- **屏幕取色**：独立紧凑取色器，提供色块预览、RGB、HTML HEX、COLORREF 十进制与十六进制。
- **日常操作**：主窗口置顶、系统托盘、三组可配置全局热键、中文/English、浅色/深色外观。
- **历史 IE 兼容**：对已有且可访问的 MSHTML 宿主读取文档、源码、表单、链接和图片等；宿主支持时可导航及高亮文本。

完整对应关系与未覆盖项见 [功能对应表](docs/PARITY_STATUS.md)；原版可恢复的 89 个界面入口见 [原版功能清单](docs/ORIGINAL_FEATURE_INVENTORY.md)。

### Hook 检查如何使用

在详情的 ListV、TreeV、RichEdit 或菜单页主动选择 Hook 读取，核对目标窗口、进程和操作后确认。目标限定模式自动匹配 x86/x64，要求目标与本程序属于同一用户、相同完整性级别，且不是受保护进程。

- 每次默认 **5 秒**，最长 **10 秒**；捕获时显示置顶提示，关闭提示或点击取消即可请求停止。
- 桌面菜单一次捕获是单独操作，需要另行确认并明确选择 x86 或 x64，不会持续监听。
- 结果会标记数量与截断状态。仅完整收到的原始 RTF 流可保存为原始 RTF；UIA 文本导出是独立路径。
- Hook 在目标线程中执行；取消可停止后续读取，但无法强行中断已经阻塞的目标窗口过程，卸载与清理可能延后。

实现方式、固定数据上限和生命周期说明见 [Hook 文档](hook-engine/README.md)。

## 隐私与已知限制

- **不读取密码、不提权、不绕过访问限制**，不提供通用进程内存读取或任意 DLL 加载接口。
- **“保存全部”指全部已捕获数据**。虚拟化、折叠或未公开的条目可能无法取得；程序不会替你滚动、展开或激活目标控件。Owner-data ListView 不支持 Hook 读取，可尝试公开 UIA 路径。
- **动态菜单可能不完整**：Hook 快照在目标处理菜单初始化消息前取得，随后新增的项目可能尚未出现。
- **历史兼容有前提**：IE 功能依赖已有 MSHTML 宿主，不适用于 Edge/Chrome 的现代 DOM；Flash 仅能读取资源引用，不安装或运行 Flash。
- **Windows 11 实机验收仍待完成**，包括混合 DPI、托盘、真实第三方控件以及应用图标的视觉表现。Windows runner 自有控件测试也不能代替这些交互验收。详见 [测试与验收范围](TESTING.md)。

程序除用户明确发起的资源下载外不联网。下载仅使用 HTTP(S)，不携带 cookies 或认证、不跟随重定向、不关闭证书检查；单次限制为 30 秒、50 MiB。

选项保存在 `%LOCALAPPDATA%\CoralSpyNext\settings.json`。捕获内容不会自动保存；只有用户选择导出时才写入指定位置。导出报告可能包含私人应用内容，分享前请检查。

## 从源码构建

工具链固定为 **Rust 1.99.0**；主程序和 `hook-engine` 各自使用已提交的 `Cargo.lock`。所有构建命令使用 `--locked`，避免无意更新依赖。

```sh
git clone https://github.com/Antman2023/CoralSpyNext.git
cd CoralSpyNext
```

### Linux → Windows GNU（自动发布使用此路径）

需要 Git、Python 3.11+、通过 rustup 安装的 Rust，以及 x86/x64 MinGW-w64。以下以 Ubuntu 24.04 为例；此交叉构建路径已在开发环境执行。

```sh
sudo apt-get update
sudo apt-get install gcc-mingw-w64-x86-64-posix gcc-mingw-w64-i686-posix binutils-mingw-w64
rustup target add x86_64-pc-windows-gnu i686-pc-windows-gnu

export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc-posix
export CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER=i686-w64-mingw32-gcc-posix

cargo test --locked --all-targets
(cd hook-engine && cargo test --locked --workspace && ./scripts/build-windows.sh)
cargo build --locked --release --target x86_64-pc-windows-gnu
```

主程序位于 `target/x86_64-pc-windows-gnu/release/coralspynext.exe`；两个辅助程序位于 `hook-engine/dist/`。将三个 EXE 放在同一目录后，在 Windows 上运行。

如需生成完整便携包，继续执行：

```sh
(cd hook-engine && bash fixtures/build-fixture.sh)
python3 scripts/package-release.py
```

输出位于 `dist/release/`。打包脚本不会执行 Windows 原生测试；发布前仍需通过后续验证。

### Windows MSVC（可选，尚未实测）

需要 PowerShell 7、Rust，以及 Visual Studio Build Tools 的“使用 C++ 的桌面开发”组件和 Windows SDK。在开发者 PowerShell 中执行：

```powershell
rustup target add x86_64-pc-windows-msvc i686-pc-windows-msvc
pwsh ./scripts/build-windows.ps1
.\dist\CoralSpyNext\coralspynext.exe
```

此脚本已提供，但尚未在 Windows 上执行验证；当前自动发布不依赖该 MSVC 构建路径。

## GitHub Actions 与自动发布

仓库使用 [release.yml](.github/workflows/release.yml) 完成构建、验证和发布：

1. **触发构建**：推送 `main`、向 `main` 提交 PR 或手动运行，会执行检查与构建，不创建 Release。
2. **Linux 构建**：运行格式检查、Clippy、单元测试及二进制审计，生成 x64 GUI、x86/x64 辅助程序和便携包。
3. **Windows 验证**：将同批产物交给 Windows Server 2022 runner，运行自有原生控件测试及明确启用的目标限定 Hook 测试，不启动桌面全局捕获。
4. **标签发布**：只有推送与 `Cargo.toml` 版本完全一致的 `v<版本>` 标签，且前述检查全部通过，才会发布 Release。已有 Release 不会被自动覆盖。

Release 附带 ZIP、SHA-256、构建清单和原生验证日志。维护者应先确认 `main` 的检查通过，再创建版本标签；流水线不会自动增加版本或创建标签。更多细节见 [发布说明](docs/RELEASING.md)，实际结果见 [Actions](https://github.com/Antman2023/CoralSpyNext/actions/workflows/release.yml)。

## 参与贡献

欢迎提交问题、复现步骤和改进 PR。报告问题时，请附上版本、Windows 版本、目标程序架构、DPI 设置及操作步骤；日志和截图请先移除私人信息。

提交代码前，建议对主程序和 Hook 工作区分别运行：

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
(cd hook-engine && cargo fmt --all -- --check && cargo clippy --locked --workspace --all-targets -- -D warnings && cargo test --locked --workspace)
```

涉及 Windows 行为的改动，请按 [TESTING.md](TESTING.md) 补充可复现的验证记录，并区分“已编译”“已运行”与“已实机验收”。

## 许可证与来源

本项目采用 [MIT 许可证](LICENSE)。

功能方向参考 Coral Studio 的 CoralSpy 1.0（2004），代码与应用图标均独立重写/设计。本项目不包含、链接或运行原版可执行文件、DLL、图标或程序代码，也不代表原作者或声称获得其背书。开源依赖及来源说明见 [THIRD_PARTY.md](THIRD_PARTY.md)。
