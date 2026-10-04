# 自动构建与发布

## 触发方式

- main push、PR、Actions 手动运行：构建、检查和原生 Windows 自有测试；不创建 Release。
- 推送 `v<package.version>` 标签：同样检查全部通过后发布。例如 Cargo.toml 的 `0.3.1` 对应 `v0.3.1`。
- 含连字符的预发布版本（如 `0.4.0-rc.1`）自动标记 prerelease。
- 不自动增加版本，不自动创建标签，不 force push，不覆盖已有 Release 或附件。

发布前同步 Cargo.toml、Cargo.lock、assets/app.rc 版本并提交到 main，确认 main 检查成功后再创建新标签。第一次发布也应走相同流程。仅创建或推送标签，不会替代前面的检查。

## 流水线

1. Ubuntu 24.04：固定 Rust 1.99.0、发行版 MinGW；主程序/引擎测试、严格 Clippy、格式检查。
2. 匹配位数的固定 payload 先构建，再嵌入 broker；检查 PE 架构、导出与系统依赖。
3. 构建 x64 GUI、便携 ZIP、源码和依赖许可证、SHA-256、独立的原生测试包。
4. Windows Server 2022：运行同批次的测试可执行文件和自有 UIA/Win32 控件；显式启用目标限定 Hook fixture。不会运行原版或桌面全局捕获。
5. 发布任务只有在前面两个任务通过且事件为版本标签 push 时获得 `contents: write`。使用临时内置 GITHUB_TOKEN，无持久 PAT/自建 secret。
6. 创建带附件的草稿，再发布。已有 Release 导致明确失败，不改写。若网络中断留下草稿，应人工核查已上传附件，再决定如何处理，流水线不会盲目覆盖。

工作流依赖的 GitHub 官方 actions 固定完整提交 SHA；checkout 不保留 git 凭据。PR 构建只有读取权限，不采用 pull_request_target。发布任务不执行产物里的脚本。

## 产物与测试声明

Release 含便携 ZIP、校验和、构建清单及原生测试日志。GUI 与两个 broker 必须位于同一目录。Release ZIP 的构建元数据记录跨编译阶段，随后 Windows job 的独立日志记录原生检查结果，避免在尚未执行测试时预先宣称成功。

“Windows runner 测试通过”不代表 Windows 11 逐像素、混合 DPI、全部第三方应用、IE 宿主和安全软件组合都已验收。查看 TESTING.md 和 docs/PARITY_STATUS.md。

## 本地检查

Linux（已安装匹配 Rust/MinGW）：

```sh
cargo test --locked --all-targets
(cd hook-engine && cargo test --locked --workspace && ./scripts/build-windows.sh && bash fixtures/build-fixture.sh)
cargo build --locked --release --target x86_64-pc-windows-gnu
python3 scripts/prepare-windows-tests.py
python3 scripts/package-release.py
```

Windows 原生验证只对该包自有测试程序执行。按工作流展开发布 ZIP 与测试 ZIP 后，显式运行脚本的 `-RunOwnedFixture` 参数；脚本会拒绝无此授权的 Hook 测试。桌面全局测试不属于自动发布流程。
