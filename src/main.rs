#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;

#[cfg(windows)]
fn main() {
    if helper_mode() {
        return;
    }
    coralspynext::platform::initialize_dpi();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("CoralSpyNext")
            .with_inner_size([420.0, 275.0])
            .with_min_inner_size([420.0, 275.0])
            .with_resizable(false)
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

#[cfg(windows)]
fn helper_mode() -> bool {
    use std::io::{Read, Write};
    let args: Vec<String> = std::env::args().collect();
    let Some(mode) = args.get(1).map(String::as_str) else {
        return false;
    };
    if !mode.starts_with("--") {
        return false;
    }
    // Independent last-resort deadline: a helper cannot survive indefinitely
    // if its parent exits before the Job Object binding or while COM blocks.
    if matches!(
        mode,
        "--accessibility-worker"
            | "--legacy-worker"
            | "--legacy-action-worker"
            | "--legacy-highlight-worker"
            | "--legacy-download-worker"
    ) {
        let seconds = if mode == "--legacy-download-worker" {
            35
        } else {
            12
        };
        if std::thread::Builder::new()
            .name("helper-deadline".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(seconds));
                std::process::exit(124);
            })
            .is_err()
        {
            let result = Result::<(), String>::Err("Cannot start helper watchdog".into());
            let mut stdout = std::io::stdout().lock();
            let _ = serde_json::to_writer(&mut stdout, &result);
            let _ = stdout.flush();
            return true;
        }
    }
    let hwnd = || {
        args.get(2)
            .filter(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()))
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|h| *h != 0)
            .ok_or_else(|| "Invalid HWND".to_string())
    };
    let frame = || -> Result<Option<usize>, String> {
        match args.get(3).map(String::as_str) {
            Some("top") => Ok(None),
            Some(value) if !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) => value
                .parse::<usize>()
                .map(Some)
                .map_err(|_| "Invalid frame index".into()),
            _ => Err("Invalid frame; expected top or a numeric index".into()),
        }
    };
    let bytes = match mode {
        "--accessibility-worker" if args.len() == 3 => {
            serde_json::to_vec(&hwnd().and_then(coralspynext::accessibility::inspect_in_process))
        }
        "--legacy-worker" if args.len() == 4 => serde_json::to_vec(&(|| {
            coralspynext::legacy::inspect_in_process(hwnd()?, frame()?)
        })()),
        "--legacy-action-worker" if args.len() == 5 => serde_json::to_vec(&(|| {
            let action = args
                .get(4)
                .and_then(|a| coralspynext::legacy::LegacyAction::parse(a))
                .ok_or_else(|| "Invalid legacy action".to_string())?;
            coralspynext::legacy::action_in_process(hwnd()?, frame()?, action)
        })()),
        "--legacy-highlight-worker" if args.len() == 4 => {
            let result = (|| {
                let hwnd = hwnd()?;
                let frame = frame()?;
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take(16_385)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 16_384 {
                    return Err("Highlight request too large".into());
                }
                let request: coralspynext::legacy::LegacyHighlightRequest =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                coralspynext::legacy::highlight_in_process(hwnd, frame, &request)
            })();
            serde_json::to_vec(&result)
        }
        "--legacy-download-worker" if args.len() == 2 => {
            let result = (|| {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take(65_537)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 65_536 {
                    return Err("Download request too large".into());
                }
                let request: coralspynext::legacy::LegacyDownloadRequest =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                coralspynext::legacy::download_in_process(&request)
            })();
            serde_json::to_vec(&result)
        }
        _ => serde_json::to_vec(&Result::<(), String>::Err(
            "Unknown helper mode or invalid arguments".into(),
        )),
    };
    let mut stdout = std::io::stdout().lock();
    if let Ok(bytes) = bytes {
        let _ = stdout.write_all(&bytes);
        let _ = stdout.flush();
    }
    true
}
