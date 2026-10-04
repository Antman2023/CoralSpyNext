#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;

#[cfg(windows)]
fn main() {
    coralspynext::platform::initialize_dpi();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("CoralSpyNext")
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([820.0, 560.0])
            .with_icon(app::app_icon()),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "CoralSpyNext",
        options,
        Box::new(|cc| Ok(Box::new(app::CoralSpyApp::new(cc)))),
    ) {
        let message: Vec<u16> =
            format!("CoralSpyNext 无法启动。\n\n{error}\n\n请检查显卡驱动与 Windows 桌面环境。")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
        let title: Vec<u16> = "CoralSpyNext · 启动失败"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // Both UTF-16 buffers remain alive for this synchronous native dialog.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "CoralSpyNext requires Windows 11 x64. The window inspector uses Windows desktop APIs."
    );
}
