# 来源与依赖

CoralSpyNext 为重新实现的 Rust 项目，功能方向参考 Coral Studio 的 CoralSpy 1.0（2004）。本项目不包含、链接或运行原 CoralSpy 可执行文件、DLL、图标或程序代码，亦不宣称获得原作者背书。

直接使用的开源依赖：
- eframe / egui 0.31.1：MIT OR Apache-2.0，https://github.com/emilk/egui
- windows-sys 0.59.0：MIT OR Apache-2.0，https://github.com/microsoft/windows-rs
- serde / serde_json：MIT OR Apache-2.0，https://github.com/serde-rs
- embed-resource：MIT，https://github.com/nabijaczleweli/rust-embed-resource

完整版本由 Cargo.lock 锁定，传递依赖的许可证以各 crates.io 软件包内的 LICENSE 为准。界面读取本机安装的字体；没有重新分发 Windows 字体。
