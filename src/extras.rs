//! Bounded, read-only Win32 menu and icon inspection.
//! Only documented metadata APIs and a timed WM_GETICON are used. Borrowed
//! native menu/icon handles are never destroyed or changed.

use crate::model::{IconImage, IconSnapshot, MenuSnapshot};

/// Serialize one RGBA image into an uncompressed, 32-bit Windows ICO file.
pub fn icon_to_ico(icon: &IconImage) -> Result<Vec<u8>, String> {
    encode_ico(icon)
}

#[cfg(windows)]
pub use native::{inspect_icons, inspect_menus, save_icon};

#[cfg(not(windows))]
pub fn inspect_menus(_hwnd: u64) -> Result<MenuSnapshot, String> {
    Err(
        crate::locale::label("菜单检查仅支持 Windows", "Menu inspection requires Windows")
            .to_owned(),
    )
}
#[cfg(not(windows))]
pub fn inspect_icons(_hwnd: u64) -> Result<IconSnapshot, String> {
    Err(
        crate::locale::label("图标检查仅支持 Windows", "Icon inspection requires Windows")
            .to_owned(),
    )
}
#[cfg(not(windows))]
pub fn save_icon(_icon: &IconImage) -> Result<Option<String>, String> {
    Err(crate::locale::label(
        "图标保存对话框仅支持 Windows",
        "The icon save dialog requires Windows",
    )
    .to_owned())
}

fn encode_ico(icon: &IconImage) -> Result<Vec<u8>, String> {
    if icon.width == 0 || icon.height == 0 || icon.width > 256 || icon.height > 256 {
        return Err(crate::locale::label(
            "ICO 图标尺寸必须介于 1 和 256 像素之间",
            "ICO dimensions must be between 1 and 256 pixels",
        )
        .to_owned());
    }
    let width = icon.width as usize;
    let height = icon.height as usize;
    if icon.rgba.len() != width * height * 4 {
        return Err(crate::locale::label(
            "图标 RGBA 数据长度与尺寸不符",
            "The icon's RGBA data length does not match its dimensions",
        )
        .to_owned());
    }
    let mask_stride = width.div_ceil(32) * 4;
    let pixel_bytes = width * height * 4;
    let dib_bytes = 40 + pixel_bytes + mask_stride * height;
    let mut data = Vec::with_capacity(22 + dib_bytes);
    data.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    data.extend_from_slice(&[icon.width as u8, icon.height as u8, 0, 0]);
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&32u16.to_le_bytes());
    data.extend_from_slice(&(dib_bytes as u32).to_le_bytes());
    data.extend_from_slice(&22u32.to_le_bytes());
    data.extend_from_slice(&40u32.to_le_bytes());
    data.extend_from_slice(&(icon.width as i32).to_le_bytes());
    data.extend_from_slice(&((icon.height * 2) as i32).to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&32u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    data.extend_from_slice(&[0; 16]);
    for y in (0..height).rev() {
        for pixel in icon.rgba[y * width * 4..(y + 1) * width * 4]
            .as_chunks::<4>()
            .0
            .iter()
        {
            data.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    for y in (0..height).rev() {
        let start = data.len();
        data.resize(start + mask_stride, 0);
        for x in 0..width {
            if icon.rgba[(y * width + x) * 4 + 3] == 0 {
                data[start + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    Ok(data)
}

/// Convert native BGRA pixels to straight RGBA. Native alpha icons use
/// premultiplied color; legacy images instead take opacity from their AND mask.
#[cfg(any(windows, test))]
fn decode_pixels(color: &[u8], mask: &[u8], native_alpha: bool) -> Vec<u8> {
    color
        .as_chunks::<4>()
        .0
        .iter()
        .zip(mask.as_chunks::<4>().0.iter())
        .flat_map(|(pixel, mask_pixel)| {
            let alpha = if native_alpha {
                pixel[3]
            } else if mask_pixel[0] == 0 {
                255
            } else {
                0
            };
            let channel = |value: u8| {
                if alpha == 0 {
                    0
                } else if native_alpha {
                    ((u32::from(value) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255)
                        as u8
                } else {
                    value
                }
            };
            [
                channel(pixel[2]),
                channel(pixel[1]),
                channel(pixel[0]),
                alpha,
            ]
        })
        .collect()
}

/// Accept only canonical drive-qualified paths before querying filesystem
/// metadata. UNC, extended/device namespaces, streams and relative traversal
/// are deliberately excluded from optional executable-icon fallback.
#[cfg(any(windows, test))]
fn local_drive_root(path: &[u16]) -> Option<[u16; 4]> {
    if path.len() <= 3 || path.len() >= 32_768 {
        return None;
    }
    if !((b'A' as u16..=b'Z' as u16).contains(&path[0])
        || (b'a' as u16..=b'z' as u16).contains(&path[0]))
        || path[1] != b':' as u16
        || path[2] != b'\\' as u16
    {
        return None;
    }
    let mut components = 0;
    for component in path[3..].split(|unit| *unit == b'\\' as u16) {
        components += 1;
        if components > 256
            || component.is_empty()
            || component == [b'.' as u16]
            || component == [b'.' as u16, b'.' as u16]
            || component
                .iter()
                .any(|unit| *unit < 32 || b":/?*\"<>|".iter().any(|c| u16::from(*c) == *unit))
        {
            return None;
        }
    }
    Some([path[0], b':' as u16, b'\\' as u16, 0])
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::model::MenuEntry;
    use std::{
        collections::HashSet,
        ffi::OsString,
        mem::{size_of, zeroed},
        os::windows::ffi::OsStringExt,
        path::PathBuf,
        ptr::null_mut,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, SetLastError, HANDLE, HWND},
        Graphics::Gdi::{
            DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
        },
        Storage::FileSystem::{
            GetDriveTypeW, GetFileAttributesExW, GetFileExInfoStandard, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT, WIN32_FILE_ATTRIBUTE_DATA,
        },
        System::Threading::{
            GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
        UI::{
            Controls::Dialogs::{
                CommDlgExtendedError, GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR,
                OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
            },
            Shell::ExtractIconExW,
            WindowsAndMessaging::{
                CopyIcon, DestroyIcon, GetClassLongPtrW, GetClassNameW, GetForegroundWindow,
                GetIconInfo, GetMenu, GetMenuBarInfo, GetMenuItemCount, GetMenuItemInfoW,
                GetSubMenu, GetSystemMenu, GetWindowThreadProcessId, IsWindow, SendMessageTimeoutW,
                GCLP_HICON, GCLP_HICONSM, HICON, HMENU, ICONINFO, ICON_BIG, ICON_SMALL,
                ICON_SMALL2, MENUBARINFO, MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED, MFT_OWNERDRAW,
                MFT_SEPARATOR, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU,
                OBJID_CLIENT, SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT, WM_GETICON,
            },
        },
    };

    const MENU_MAX_ENTRIES: usize = 4096;
    const MENU_MAX_DEPTH: usize = 32;
    const MENU_TEXT_CAPACITY: usize = 2048;
    const INSPECTION_BUDGET: Duration = Duration::from_secs(2);
    const ICON_TIMEOUT_MS: u32 = 150;
    const ICON_MAX_DIMENSION: u32 = 256;
    const WARNING_CAPACITY: usize = 32;

    fn warn(warnings: &mut Vec<String>, message: impl Into<String>) {
        let message = message.into();
        if warnings.len() < WARNING_CAPACITY && !warnings.contains(&message) {
            warnings.push(message);
        }
    }

    fn win32_error(operation: &str) -> String {
        let code = unsafe { GetLastError() };
        if code == 0 {
            crate::localized_format!("{operation}失败；目标可能已关闭、无响应或不可访问", "{operation} failed; the target may have closed, stopped responding, or become inaccessible")
        } else {
            crate::localized_format!(
                "{operation}失败（Win32 {code}：{}）",
                "{operation} failed (Win32 {code}: {})",
                std::io::Error::from_raw_os_error(code as i32)
            )
        }
    }

    fn checked_window(value: u64) -> Result<(HWND, (u32, u32)), String> {
        let address = usize::try_from(value).map_err(|_| {
            crate::locale::label(
                "窗口句柄超出指针宽度",
                "The window handle exceeds the pointer width",
            )
            .to_owned()
        })?;
        if address == 0 || address == 0xffff {
            return Err(crate::locale::label(
                "请选择有效窗口；不能使用空句柄或广播句柄",
                "Select a valid window; null and broadcast handles are not allowed",
            )
            .to_owned());
        }
        let hwnd = address as HWND;
        Ok((hwnd, identity(hwnd)?))
    }

    fn identity(hwnd: HWND) -> Result<(u32, u32), String> {
        let mut pid = 0;
        // These APIs only read OS-maintained metadata. An invalid/recycled HWND
        // fails or yields a different owner, checked again before publication.
        let tid = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if tid == 0 || pid == 0 || unsafe { IsWindow(hwnd) } == 0 {
            return Err(crate::locale::label(
                "窗口已关闭或句柄已失效，请重新选择",
                "The window has closed or its handle is no longer valid; select it again",
            )
            .to_owned());
        }
        Ok((pid, tid))
    }

    fn ensure_identity(hwnd: HWND, original: (u32, u32)) -> Result<(), String> {
        if identity(hwnd)? != original {
            return Err(crate::locale::label("检查期间窗口句柄被重复使用；已丢弃结果，请重新选择", "The window handle was reused during inspection; results were discarded. Select it again").to_owned());
        }
        Ok(())
    }

    struct MenuWalker {
        started: Instant,
        seen: HashSet<usize>,
        entries: Vec<MenuEntry>,
        warnings: Vec<String>,
    }

    impl MenuWalker {
        fn at_limit(&mut self) -> bool {
            if self.entries.len() >= MENU_MAX_ENTRIES {
                warn(
                    &mut self.warnings,
                    crate::locale::label(
                        "菜单项目已达到 4096 项上限，结果已截断",
                        "The menu reached the 4096-item limit; results were truncated",
                    ),
                );
                true
            } else if self.started.elapsed() >= INSPECTION_BUDGET {
                warn(
                    &mut self.warnings,
                    crate::locale::label(
                        "菜单读取超过 2 秒预算，结果已截断",
                        "Menu inspection exceeded its 2-second budget; results were truncated",
                    ),
                );
                true
            } else {
                false
            }
        }

        fn visit(&mut self, menu: HMENU, depth: usize) {
            if self.at_limit() || menu.is_null() {
                return;
            }
            if depth >= MENU_MAX_DEPTH {
                warn(
                    &mut self.warnings,
                    crate::locale::label(
                        "菜单层级已达到 32 层上限，深层项目已省略",
                        "The menu reached the 32-level depth limit; deeper items were omitted",
                    ),
                );
                return;
            }
            if !self.seen.insert(menu as usize) {
                warn(
                    &mut self.warnings,
                    crate::locale::label(
                        "检测到重复菜单句柄，已跳过重复分支",
                        "A repeated menu handle was detected; the duplicate branch was skipped",
                    ),
                );
                return;
            }
            let count = unsafe { GetMenuItemCount(menu) };
            if count < 0 {
                warn(
                    &mut self.warnings,
                    win32_error(crate::locale::label(
                        "读取菜单项目数量",
                        "Read menu item count",
                    )),
                );
                return;
            }
            for position in 0..count as u32 {
                if self.at_limit() {
                    break;
                }
                // No item-data pointers, bitmaps or owner-draw callbacks are
                // requested or dereferenced. A destroyed menu simply fails.
                let mut info: MENUITEMINFOW = unsafe { zeroed() };
                info.cbSize = size_of::<MENUITEMINFOW>() as u32;
                info.fMask = MIIM_FTYPE | MIIM_ID | MIIM_STATE | MIIM_SUBMENU;
                if unsafe { GetMenuItemInfoW(menu, position, 1, &mut info) } == 0 {
                    warn(
                        &mut self.warnings,
                        crate::locale::label(
                            "部分菜单项目读取失败；目标菜单可能正在变化",
                            "Some menu items could not be read; the target menu may be changing",
                        ),
                    );
                    continue;
                }
                let separator = info.fType & MFT_SEPARATOR != 0;
                let owner_draw = info.fType & MFT_OWNERDRAW != 0;
                let mut label = String::new();
                if !separator {
                    let mut text_info: MENUITEMINFOW = unsafe { zeroed() };
                    text_info.cbSize = size_of::<MENUITEMINFOW>() as u32;
                    text_info.fMask = MIIM_STRING;
                    if unsafe { GetMenuItemInfoW(menu, position, 1, &mut text_info) } != 0
                        && text_info.cch > 0
                        && !self.at_limit()
                    {
                        let requested = text_info.cch as usize;
                        let capacity = (requested.saturating_add(1)).min(MENU_TEXT_CAPACITY);
                        let mut text = vec![0u16; capacity];
                        text_info.dwTypeData = text.as_mut_ptr();
                        text_info.cch = capacity as u32;
                        if unsafe { GetMenuItemInfoW(menu, position, 1, &mut text_info) } != 0 {
                            let mut length = text.iter().position(|c| *c == 0).unwrap_or(capacity);
                            if requested >= capacity {
                                warn(
                                    &mut self.warnings,
                                    crate::locale::label("部分菜单文本超过 2047 个 UTF-16 单元，已截断", "Some menu text exceeds 2047 UTF-16 units and was truncated"),
                                );
                                if length > 0 && (0xD800..=0xDBFF).contains(&text[length - 1]) {
                                    length -= 1;
                                }
                            }
                            label = String::from_utf16_lossy(&text[..length]);
                        } else {
                            warn(
                                &mut self.warnings,
                                crate::locale::label("部分菜单文本读取失败；目标菜单可能正在变化", "Some menu text could not be read; the target menu may be changing"),
                            );
                        }
                    }
                    if label.is_empty() {
                        label = if owner_draw {
                            crate::locale::label(
                                "[自绘菜单项：未提供文字]",
                                "[Owner-drawn menu item: no text provided]",
                            )
                        } else {
                            crate::locale::label("[无文字菜单项]", "[Menu item without text]")
                        }
                        .to_owned();
                    }
                }
                let submenu = unsafe { GetSubMenu(menu, position as i32) };
                if submenu != info.hSubMenu {
                    warn(
                        &mut self.warnings,
                        crate::locale::label(
                            "菜单在读取期间发生变化；子菜单结果可能不完整",
                            "The menu changed during inspection; submenu results may be incomplete",
                        ),
                    );
                }
                self.entries.push(MenuEntry {
                    depth,
                    label,
                    id: info.wID,
                    enabled: info.fState & MFS_DISABLED == 0,
                    checked: info.fState & MFS_CHECKED != 0,
                    separator,
                    submenu: !submenu.is_null(),
                });
                if !submenu.is_null() {
                    self.visit(submenu, depth + 1);
                }
            }
        }
    }

    /// Capture the selected window's native menu without opening, invoking or
    /// modifying it. Custom/modern popup contents can use the app's UIA tab.
    pub fn inspect_menus(value: u64) -> Result<MenuSnapshot, String> {
        let (hwnd, owner) = checked_window(value)?;
        let mut walker = MenuWalker {
            started: Instant::now(),
            seen: HashSet::new(),
            entries: Vec::new(),
            warnings: Vec::new(),
        };
        let menu = unsafe { GetMenu(hwnd) };
        if !menu.is_null() {
            walker.visit(menu, 0);
        } else {
            let mut class = [0u16; 256];
            let length = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
            let popup =
                length > 0 && String::from_utf16_lossy(&class[..length as usize]) == "#32768";
            if popup {
                // GetMenuBarInfo(OBJID_CLIENT) is documented specifically
                // for the popup menu associated with the selected HWND.
                // Do not use private MN_* messages or remote process memory.
                let mut bar: MENUBARINFO = unsafe { zeroed() };
                bar.cbSize = size_of::<MENUBARINFO>() as u32;
                if unsafe { GetMenuBarInfo(hwnd, OBJID_CLIENT, 0, &mut bar) } != 0
                    && !bar.hMenu.is_null()
                {
                    walker.visit(bar.hMenu, 0);
                } else {
                    warn(&mut walker.warnings, crate::locale::label("弹出菜单未提供可访问的原生菜单句柄，请使用窗口内容页的 UI Automation 读取可访问菜单项", "The popup did not provide an accessible native menu handle; use UI Automation on the Content page to read accessible menu items"));
                }
            } else {
                // FALSE retrieves the current system menu. TRUE would reset
                // the target menu and is deliberately never used.
                let system_menu = unsafe { GetSystemMenu(hwnd, 0) };
                if !system_menu.is_null() {
                    warn(
                        &mut walker.warnings,
                        crate::locale::label("该窗口没有传统菜单栏；以下为窗口系统菜单（并非应用菜单栏）", "This window has no traditional menu bar; the following items are from its window system menu, not the application's menu bar"),
                    );
                    walker.visit(system_menu, 0);
                } else {
                    warn(&mut walker.warnings, crate::locale::label("未提供原生菜单；自绘、浏览器和现代应用菜单可在窗口内容页尝试 UI Automation", "No native menu was provided; try UI Automation on the Content page for owner-drawn, browser, and modern application menus"));
                }
            }
        }
        ensure_identity(hwnd, owner)?;
        Ok(MenuSnapshot {
            hwnd: value,
            entries: walker.entries,
            warnings: walker.warnings,
        })
    }

    struct OwnedIcon(HICON);
    impl Drop for OwnedIcon {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // Only CopyIcon and ExtractIconEx-created handles enter here.
                unsafe {
                    DestroyIcon(self.0);
                }
            }
        }
    }
    struct OwnedBitmap(HBITMAP);
    impl Drop for OwnedBitmap {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    DeleteObject(self.0);
                }
            }
        }
    }
    struct ScreenDc(HDC);
    impl Drop for ScreenDc {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    ReleaseDC(null_mut(), self.0);
                }
            }
        }
    }
    struct ProcessHandle(HANDLE);
    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    fn bitmap_dimensions(bitmap: HBITMAP) -> Result<(u32, u32, u16), String> {
        let mut details: BITMAP = unsafe { zeroed() };
        let copied = unsafe {
            GetObjectW(
                bitmap,
                size_of::<BITMAP>() as i32,
                (&mut details as *mut BITMAP).cast(),
            )
        };
        if copied != size_of::<BITMAP>() as i32 || details.bmWidth <= 0 || details.bmHeight == 0 {
            return Err(crate::locale::label(
                "图标位图信息无效",
                "Invalid icon bitmap information",
            )
            .to_owned());
        }
        Ok((
            details.bmWidth as u32,
            details.bmHeight.unsigned_abs(),
            details.bmBitsPixel,
        ))
    }

    fn bitmap_bgra(dc: HDC, bitmap: HBITMAP, width: u32, height: u32) -> Result<Vec<u8>, String> {
        if width == 0
            || height == 0
            || width > ICON_MAX_DIMENSION
            || height > ICON_MAX_DIMENSION * 2
        {
            return Err(crate::locale::label(
                "图标位图尺寸超过安全上限",
                "Icon bitmap dimensions exceed the safety limit",
            )
            .to_owned());
        }
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let mut info: BITMAPINFO = unsafe { zeroed() };
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width as i32;
        info.bmiHeader.biHeight = -(height as i32); // Top-down pixels.
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        info.bmiHeader.biSizeImage = pixels.len() as u32;
        // GetIconInfo's bitmaps are owned copies and are never selected into
        // a DC, satisfying GetDIBits' bitmap-selection requirement.
        let lines = unsafe {
            GetDIBits(
                dc,
                bitmap,
                0,
                height,
                pixels.as_mut_ptr().cast(),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        if lines != height as i32 {
            return Err(win32_error(crate::locale::label(
                "读取图标像素",
                "Read icon pixels",
            )));
        }
        Ok(pixels)
    }

    fn icon_image(
        icon: &OwnedIcon,
        kind: &str,
        warnings: &mut Vec<String>,
    ) -> Result<IconImage, String> {
        let mut info: ICONINFO = unsafe { zeroed() };
        if unsafe { GetIconInfo(icon.0, &mut info) } == 0 {
            return Err(win32_error(crate::locale::label(
                "读取图标位图",
                "Read icon bitmap",
            )));
        }
        // GetIconInfo creates both bitmaps. They must be released even when
        // later validation or allocation fails.
        let mask = OwnedBitmap(info.hbmMask);
        let color = OwnedBitmap(info.hbmColor);
        if mask.0.is_null() {
            return Err(crate::locale::label(
                "图标没有有效的透明蒙版",
                "The icon has no valid transparency mask",
            )
            .to_owned());
        }
        let (mask_width, mask_height, _) = bitmap_dimensions(mask.0)?;
        let monochrome = color.0.is_null();
        let (width, height, bit_depth) = if monochrome {
            if mask_height % 2 != 0 {
                return Err(crate::locale::label(
                    "单色图标的 AND/XOR 蒙版高度无效",
                    "Invalid AND/XOR mask height for the monochrome icon",
                )
                .to_owned());
            }
            (mask_width, mask_height / 2, 1)
        } else {
            bitmap_dimensions(color.0)?
        };
        if width == 0 || height == 0 || width > ICON_MAX_DIMENSION || height > ICON_MAX_DIMENSION {
            return Err(crate::locale::label(
                "图标超过 256 × 256 像素上限，已跳过",
                "The icon exceeds the 256 × 256 pixel limit and was skipped",
            )
            .to_owned());
        }
        if mask_width != width || mask_height != height * if monochrome { 2 } else { 1 } {
            return Err(crate::locale::label(
                "图标颜色位图与透明蒙版尺寸不一致",
                "The icon color bitmap and transparency mask have different dimensions",
            )
            .to_owned());
        }
        let dc = ScreenDc(unsafe { GetDC(null_mut()) });
        if dc.0.is_null() {
            return Err(win32_error(crate::locale::label(
                "创建图标读取设备上下文",
                "Create an icon-reading device context",
            )));
        }
        let mut mask_pixels = bitmap_bgra(dc.0, mask.0, mask_width, mask_height)?;
        let pixel_count = width as usize * height as usize * 4;
        let mut color_pixels = if monochrome {
            mask_pixels[pixel_count..].to_vec()
        } else {
            bitmap_bgra(dc.0, color.0, width, height)?
        };
        mask_pixels.truncate(pixel_count);
        let native_alpha = !monochrome
            && bit_depth == 32
            && color_pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
        if !native_alpha {
            let mut inverted = false;
            for (pixel, mask_pixel) in color_pixels
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(mask_pixels.as_chunks_mut::<4>().0.iter_mut())
            {
                // AND=1 with a nonzero XOR image means background inversion,
                // which cannot be represented by ordinary RGBA. Show its XOR
                // color against black, and disclose that approximation.
                if mask_pixel[0] != 0 && pixel[..3].iter().any(|c| *c != 0) {
                    inverted = true;
                    mask_pixel[0] = 0;
                }
            }
            if inverted {
                warn(
                    warnings,
                    crate::locale::label("图标含旧式背景反色像素；RGBA 预览和导出以黑底效果近似显示这些像素", "The icon contains legacy background-inversion pixels; the RGBA preview and export approximate these pixels against a black background"),
                );
            }
        }
        let rgba = super::decode_pixels(&color_pixels, &mask_pixels, native_alpha);
        Ok(IconImage {
            width,
            height,
            rgba,
            kind: kind.to_owned(),
        })
    }

    fn append_icon(snapshot: &mut IconSnapshot, owned: OwnedIcon, kind: &str) {
        match icon_image(&owned, kind, &mut snapshot.warnings) {
            Ok(icon) => {
                if let Some(existing) = snapshot.icons.iter_mut().find(|existing| {
                    existing.width == icon.width
                        && existing.height == icon.height
                        && existing.rgba == icon.rgba
                }) {
                    if !existing.kind.contains(kind) {
                        existing.kind.push_str(" / ");
                        existing.kind.push_str(kind);
                    }
                } else {
                    snapshot.icons.push(icon);
                }
            }
            Err(error) => {
                let displayed_kind = crate::locale::icon_source_label(kind);
                warn(
                    &mut snapshot.warnings,
                    crate::localized_format!(
                        "{displayed_kind}：{error}",
                        "{displayed_kind}: {error}"
                    ),
                );
            }
        }
    }

    fn append_borrowed_icon(snapshot: &mut IconSnapshot, borrowed: HICON, kind: &str) {
        if borrowed.is_null() {
            return;
        }
        // Never destroy a handle borrowed from the target process/class.
        // Copy immediately so later target updates cannot affect our bitmap.
        let owned = OwnedIcon(unsafe { CopyIcon(borrowed) });
        if owned.0.is_null() {
            let displayed_kind = crate::locale::icon_source_label(kind);
            warn(
                &mut snapshot.warnings,
                crate::localized_format!(
                    "{displayed_kind}：{}",
                    "{displayed_kind}: {}",
                    win32_error(crate::locale::label("复制图标", "Copy icon"))
                ),
            );
        } else {
            append_icon(snapshot, owned, kind);
        }
    }

    fn local_executable_is_safe(
        path: &[u16],
        started: Instant,
        warnings: &mut Vec<String>,
    ) -> bool {
        let Some(root) = super::local_drive_root(path) else {
            warn(
                warnings,
                crate::locale::label("已跳过程序文件图标：仅允许本地盘符绝对路径，不读取网络、设备或特殊路径", "Program-file icons were skipped: only absolute local drive-letter paths are allowed; network, device, and special paths are not read"),
            );
            return false;
        };
        if started.elapsed() >= INSPECTION_BUDGET {
            warn(
                warnings,
                crate::locale::label(
                    "图标读取已达到 2 秒预算，已跳过程序文件图标",
                    "Icon inspection reached its 2-second budget; program-file icons were skipped",
                ),
            );
            return false;
        }
        // DRIVE_FIXED = 3 is documented by GetDriveTypeW. Its constant lives
        // under a different optional windows-sys feature; no extra feature is
        // needed merely to compare this return value.
        if unsafe { GetDriveTypeW(root.as_ptr()) } != 3 {
            warn(warnings, crate::locale::label("已跳过程序文件图标：驱动器不是已确认的本地固定磁盘（网络、可移动或未知磁盘均不读取）", "Program-file icons were skipped: the drive is not a confirmed local fixed disk; network, removable, and unknown drives are not read"));
            return false;
        }
        let mut prefix = path.to_vec();
        prefix.push(0);
        for end in (3..=path.len()).filter(|end| *end == path.len() || path[*end] == b'\\' as u16) {
            if started.elapsed() >= INSPECTION_BUDGET {
                warn(warnings, crate::locale::label("图标读取已达到 2 秒预算，已跳过程序文件图标", "Icon inspection reached its 2-second budget; program-file icons were skipped"));
                return false;
            }
            let saved = prefix[end];
            prefix[end] = 0;
            let mut metadata: WIN32_FILE_ATTRIBUTE_DATA = unsafe { zeroed() };
            // Inspect components in order. GetFileAttributesEx reports the
            // link itself; reject it before traversing into any descendant,
            // including junctions, volume mounts and cloud placeholders.
            let success = unsafe {
                GetFileAttributesExW(
                    prefix.as_ptr(),
                    GetFileExInfoStandard,
                    (&mut metadata as *mut WIN32_FILE_ATTRIBUTE_DATA).cast(),
                )
            };
            prefix[end] = saved;
            if success == 0 {
                warn(warnings, crate::locale::label("已跳过程序文件图标：无法确认本地路径属性", "Program-file icons were skipped: local path attributes could not be verified"));
                return false;
            }
            let forbidden = FILE_ATTRIBUTE_REPARSE_POINT
                | FILE_ATTRIBUTE_OFFLINE
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;
            if metadata.dwFileAttributes & forbidden != 0 {
                warn(
                    warnings,
                    crate::locale::label("已跳过程序文件图标：路径包含重解析点、离线文件或需要下载的云端文件", "Program-file icons were skipped: the path contains a reparse point, offline file, or cloud file requiring download"),
                );
                return false;
            }
            if end == path.len() {
                let bytes =
                    (u64::from(metadata.nFileSizeHigh) << 32) | u64::from(metadata.nFileSizeLow);
                if metadata.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
                    || bytes == 0
                    || bytes > 64 * 1024 * 1024
                {
                    warn(
                        warnings,
                        crate::locale::label("已跳过程序文件图标：仅读取不超过 64 MiB 的非空本地程序文件", "Program-file icons were skipped: only nonempty local program files up to 64 MiB are read"),
                    );
                    return false;
                }
            } else if metadata.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                warn(
                    warnings,
                    crate::locale::label(
                        "已跳过程序文件图标：父路径不是目录",
                        "Program-file icons were skipped: the parent path is not a directory",
                    ),
                );
                return false;
            }
        }
        if started.elapsed() >= INSPECTION_BUDGET {
            warn(
                warnings,
                crate::locale::label(
                    "图标读取已达到 2 秒预算，已跳过程序文件图标",
                    "Icon inspection reached its 2-second budget; program-file icons were skipped",
                ),
            );
            return false;
        }
        true
    }

    fn executable_icons(snapshot: &mut IconSnapshot, pid: u32, started: Instant) {
        let process =
            ProcessHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) });
        if process.0.is_null() {
            warn(
                &mut snapshot.warnings,
                win32_error(crate::locale::label(
                    "读取程序图标路径",
                    "Read program icon path",
                )),
            );
            return;
        }
        let mut path = vec![0u16; 32_768];
        let mut count = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut count) } == 0 {
            warn(
                &mut snapshot.warnings,
                win32_error(crate::locale::label(
                    "读取程序图标路径",
                    "Read program icon path",
                )),
            );
            return;
        }
        if count == 0 || count as usize >= path.len() {
            warn(
                &mut snapshot.warnings,
                crate::locale::label("程序图标路径长度无效", "Invalid program icon path length"),
            );
            return;
        }
        path[count as usize] = 0;
        if !local_executable_is_safe(&path[..count as usize], started, &mut snapshot.warnings) {
            return;
        }
        let mut big = null_mut();
        let mut small = null_mut();
        // ExtractIconEx reads resource data through the documented Shell API;
        // never LoadLibrary or execute any code from the target executable.
        // The local-drive/size checks reduce blocking risk, but synchronous
        // disk/filter/Shell APIs have no cancellation deadline. Concurrent
        // filesystem changes also prevent a path check from being a security
        // boundary. A hard time bound would require a separate worker process.
        let result = unsafe { ExtractIconExW(path.as_ptr(), 0, &mut big, &mut small, 1) };
        let big = OwnedIcon(big);
        let small = OwnedIcon(small);
        if started.elapsed() >= INSPECTION_BUDGET {
            warn(
                &mut snapshot.warnings,
                crate::locale::label("本地程序文件图标读取超过 2 秒；Windows 磁盘和资源读取调用无法在进程内强制中止", "Local program-file icon inspection exceeded 2 seconds; Windows disk and resource-reading calls cannot be forcibly stopped within the process"),
            );
        }
        if result == u32::MAX {
            warn(
                &mut snapshot.warnings,
                win32_error(crate::locale::label("提取程序图标", "Extract program icon")),
            );
            return;
        }
        if !small.0.is_null() {
            append_icon(snapshot, small, "程序文件小图标");
        }
        if !big.0.is_null() {
            append_icon(snapshot, big, "程序文件大图标");
        }
    }

    /// Read window/class icons, falling back to the executable's first icon.
    /// Call on an independent worker thread, never the UI thread.
    pub fn inspect_icons(value: u64) -> Result<IconSnapshot, String> {
        let (hwnd, owner) = checked_window(value)?;
        let started = Instant::now();
        let mut snapshot = IconSnapshot {
            hwnd: value,
            ..Default::default()
        };
        if owner.0 != unsafe { GetCurrentProcessId() } {
            for (request, kind) in [
                (ICON_SMALL2, "窗口小图标2"),
                (ICON_SMALL, "窗口小图标"),
                (ICON_BIG, "窗口大图标"),
            ] {
                ensure_identity(hwnd, owner)?;
                if started.elapsed() >= INSPECTION_BUDGET {
                    warn(
                        &mut snapshot.warnings,
                        crate::locale::label("图标读取已达到 2 秒预算，已停止其他查询", "Icon inspection reached its 2-second budget; remaining queries were stopped"),
                    );
                    break;
                }
                let mut result = 0usize;
                unsafe {
                    SetLastError(0);
                }
                // WM_GETICON has no cross-process buffer; no custom/control
                // messages, hooks or remote-memory operations are used.
                let success = unsafe {
                    SendMessageTimeoutW(
                        hwnd,
                        WM_GETICON,
                        request as usize,
                        0,
                        SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
                        ICON_TIMEOUT_MS,
                        &mut result,
                    )
                };
                if success == 0 {
                    warn(
                        &mut snapshot.warnings,
                        crate::locale::label("窗口图标查询超时或被目标拒绝；继续尝试窗口类图标", "The window icon query timed out or was rejected; trying window-class icons instead"),
                    );
                    break;
                }
                append_borrowed_icon(&mut snapshot, result as HICON, kind);
            }
        } else {
            warn(
                &mut snapshot.warnings,
                crate::locale::label("自身窗口跳过同步图标消息，仅尝试窗口类和程序文件图标", "Synchronous icon messages are skipped for this program's own windows; only window-class and program-file icons are tried"),
            );
        }
        ensure_identity(hwnd, owner)?;
        if started.elapsed() < INSPECTION_BUDGET {
            for (index, kind) in [(GCLP_HICONSM, "窗口类小图标"), (GCLP_HICON, "窗口类大图标")]
            {
                let borrowed = unsafe { GetClassLongPtrW(hwnd, index) } as HICON;
                append_borrowed_icon(&mut snapshot, borrowed, kind);
            }
        }
        ensure_identity(hwnd, owner)?;
        if snapshot.icons.is_empty() && started.elapsed() < INSPECTION_BUDGET {
            executable_icons(&mut snapshot, owner.0, started);
        }
        ensure_identity(hwnd, owner)?;
        if snapshot.icons.is_empty() {
            warn(
                &mut snapshot.warnings,
                crate::locale::label("该窗口未提供可读取图标，或当前权限不允许访问", "This window provided no readable icon, or access is not permitted at the current privilege level"),
            );
        }
        Ok(snapshot)
    }

    static SAVE_DIALOG_OPEN: AtomicBool = AtomicBool::new(false);
    struct SaveDialogGuard;
    impl Drop for SaveDialogGuard {
        fn drop(&mut self) {
            SAVE_DIALOG_OPEN.store(false, Ordering::Release);
        }
    }

    /// Ask the user for a Unicode path, then save actual ICO bytes. Cancel is
    /// Ok(None); overwrite confirmation is provided by the native dialog.
    pub fn save_icon(icon: &IconImage) -> Result<Option<String>, String> {
        let bytes = super::icon_to_ico(icon)?;
        if SAVE_DIALOG_OPEN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(crate::locale::label(
                "已有图标保存对话框打开，请先完成或取消它",
                "An icon save dialog is already open; finish or cancel it first",
            )
            .to_owned());
        }
        let _guard = SaveDialogGuard;
        let mut file = vec![0u16; 32_768];
        for (out, unit) in file.iter_mut().zip("CoralSpyNext-icon.ico".encode_utf16()) {
            *out = unit;
        }
        let filter: Vec<u16> = crate::locale::label(
            "Windows 图标 (*.ico)\0*.ico\0\0",
            "Windows icons (*.ico)\0*.ico\0\0",
        )
        .encode_utf16()
        .collect();
        let extension: Vec<u16> = "ico\0".encode_utf16().collect();
        let title: Vec<u16> =
            crate::locale::label("保存 CoralSpyNext 图标\0", "Save CoralSpyNext icon\0")
                .encode_utf16()
                .collect();
        let foreground = unsafe { GetForegroundWindow() };
        let owner = if identity(foreground)
            .is_ok_and(|owner| owner.0 == unsafe { GetCurrentProcessId() })
        {
            foreground
        } else {
            null_mut()
        };
        let mut dialog: OPENFILENAMEW = unsafe { zeroed() };
        dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = owner;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.nFilterIndex = 1;
        dialog.lpstrFile = file.as_mut_ptr();
        dialog.nMaxFile = file.len() as u32;
        dialog.lpstrTitle = title.as_ptr();
        dialog.lpstrDefExt = extension.as_ptr();
        dialog.Flags = OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST;
        if unsafe { GetSaveFileNameW(&mut dialog) } == 0 {
            let code = unsafe { CommDlgExtendedError() };
            return if code == 0 {
                Ok(None)
            } else {
                Err(crate::localized_format!(
                    "图标保存对话框失败（0x{code:08X}）",
                    "Icon save dialog failed (0x{code:08X})"
                ))
            };
        }
        let length = file.iter().position(|unit| *unit == 0).ok_or_else(|| {
            crate::locale::label(
                "保存路径缺少结束符",
                "The save path is missing its terminator",
            )
            .to_owned()
        })?;
        if length == 0 {
            return Err(
                crate::locale::label("未选择保存路径", "No save path was selected").to_owned(),
            );
        }
        // Preserve exact UTF-16 for filesystem access, including paths which
        // cannot be represented losslessly by a Rust UTF-8 String.
        let path = PathBuf::from(OsString::from_wide(&file[..length]));
        std::fs::write(&path, bytes).map_err(|error| {
            crate::localized_format!(
                "无法保存 ICO 图标：{error}",
                "Could not save the ICO icon: {error}"
            )
        })?;
        Ok(Some(path.to_string_lossy().into_owned()))
    }
    #[cfg(test)]
    mod native_tests {
        use super::*;
        use windows_sys::Win32::{
            Graphics::Gdi::CreateBitmap,
            UI::WindowsAndMessaging::{
                AppendMenuW, CreateIconIndirect, CreateMenu, CreatePopupMenu, DestroyMenu,
                MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
            },
        };

        struct TestMenu(HMENU);
        impl Drop for TestMenu {
            fn drop(&mut self) {
                // These menus belong exclusively to the test fixture.
                unsafe {
                    DestroyMenu(self.0);
                }
            }
        }

        fn walker() -> MenuWalker {
            MenuWalker {
                started: Instant::now(),
                seen: HashSet::new(),
                entries: Vec::new(),
                warnings: Vec::new(),
            }
        }

        fn wide(text: &str) -> Vec<u16> {
            text.encode_utf16().chain(Some(0)).collect()
        }

        #[test]
        fn reads_owned_menu_hierarchy_without_changing_it() {
            let menu = TestMenu(unsafe { CreateMenu() });
            let child = TestMenu(unsafe { CreatePopupMenu() });
            assert!(!menu.0.is_null() && !child.0.is_null());
            let item_text = wide("保存(&S)\tCtrl+S · 测试");
            let parent_text = wide("文件(&F)");
            assert_ne!(
                unsafe {
                    AppendMenuW(
                        child.0,
                        MF_STRING | MF_CHECKED | MF_GRAYED,
                        42,
                        item_text.as_ptr(),
                    )
                },
                0
            );
            assert_ne!(
                unsafe { AppendMenuW(child.0, MF_SEPARATOR, 0, std::ptr::null()) },
                0
            );
            assert_ne!(
                unsafe { AppendMenuW(menu.0, MF_POPUP, child.0 as usize, parent_text.as_ptr()) },
                0
            );
            // The parent menu now owns the submenu, including its destruction.
            std::mem::forget(child);
            let mut inspection = walker();
            inspection.visit(menu.0, 0);
            assert!(inspection.warnings.is_empty(), "{:?}", inspection.warnings);
            assert_eq!(inspection.entries.len(), 3);
            assert_eq!(inspection.entries[0].label, "文件(&F)");
            assert!(inspection.entries[0].submenu);
            assert_eq!(inspection.entries[1].depth, 1);
            assert_eq!(inspection.entries[1].id, 42);
            assert_eq!(inspection.entries[1].label, "保存(&S)\tCtrl+S · 测试");
            assert!(inspection.entries[1].checked);
            assert!(!inspection.entries[1].enabled);
            assert!(inspection.entries[2].separator);
            assert_eq!(unsafe { GetMenuItemCount(menu.0) }, 1);
            assert_eq!(unsafe { GetMenuItemCount(GetSubMenu(menu.0, 0)) }, 2);
        }

        #[test]
        fn menu_depth_and_time_limits_are_reported() {
            let menu = TestMenu(unsafe { CreateMenu() });
            assert!(!menu.0.is_null());
            let mut depth_limited = walker();
            depth_limited.visit(menu.0, MENU_MAX_DEPTH);
            assert!(depth_limited.entries.is_empty());
            assert!(depth_limited.warnings.iter().any(|w| w.contains("32")));
            let mut time_limited = walker();
            time_limited.started = Instant::now() - Duration::from_secs(3);
            time_limited.visit(menu.0, 0);
            assert!(time_limited.entries.is_empty());
            assert!(time_limited.warnings.iter().any(|w| w.contains("2 秒")));
        }

        #[test]
        fn menu_entry_limit_and_duplicate_handles_are_reported() {
            let menu = TestMenu(unsafe { CreateMenu() });
            assert!(!menu.0.is_null());
            let mut capped = walker();
            capped
                .entries
                .resize(MENU_MAX_ENTRIES, MenuEntry::default());
            capped.visit(menu.0, 0);
            assert_eq!(capped.entries.len(), MENU_MAX_ENTRIES);
            assert!(capped.warnings.iter().any(|w| w.contains("4096")));
            let mut duplicate = walker();
            duplicate.visit(menu.0, 0);
            duplicate.visit(menu.0, 0);
            assert!(duplicate.warnings.iter().any(|w| w.contains("重复")));
        }

        #[test]
        fn extracts_owned_monochrome_icon_without_destroying_source() {
            // A monochrome icon stores an AND plane above an XOR plane.
            // All-zero planes render solid opaque black independent of row order.
            let bits = [0u8; 64];
            let mask = OwnedBitmap(unsafe { CreateBitmap(16, 32, 1, 1, bits.as_ptr().cast()) });
            assert!(!mask.0.is_null());
            let mut info: ICONINFO = unsafe { zeroed() };
            info.fIcon = 1;
            info.hbmMask = mask.0;
            let source = OwnedIcon(unsafe { CreateIconIndirect(&info) });
            assert!(!source.0.is_null());
            let mut snapshot = IconSnapshot::default();
            append_borrowed_icon(&mut snapshot, source.0, "测试图标");
            assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
            assert_eq!(snapshot.icons.len(), 1);
            let result = &snapshot.icons[0];
            assert_eq!((result.width, result.height), (16, 16));
            assert!(result
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [0, 0, 0, 255]));
            // Repeating extraction proves the original borrowed handle remains valid.
            append_borrowed_icon(&mut snapshot, source.0, "第二次读取");
            assert_eq!(snapshot.icons.len(), 1);
            assert!(snapshot.icons[0].kind.contains("第二次读取"));
        }

        #[test]
        fn rejects_invalid_or_broadcast_windows() {
            for hwnd in [0, 0xffff, u64::MAX] {
                assert!(inspect_menus(hwnd).is_err());
                assert!(inspect_icons(hwnd).is_err());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(width: u32, height: u32, rgba: Vec<u8>) -> IconImage {
        IconImage {
            width,
            height,
            rgba,
            kind: "test".to_owned(),
        }
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn ico_header_and_bgra_bottom_up_are_valid() {
        let image = icon(1, 2, vec![1, 2, 3, 255, 4, 5, 6, 0]);
        let bytes = icon_to_ico(&image).unwrap();
        assert_eq!(&bytes[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!(&bytes[6..10], &[1, 2, 0, 0]);
        assert_eq!(u32_at(&bytes, 14) as usize, bytes.len() - 22);
        assert_eq!(u32_at(&bytes, 18), 22);
        assert_eq!(u32_at(&bytes, 22), 40);
        assert_eq!(u32_at(&bytes, 26), 1);
        assert_eq!(u32_at(&bytes, 30), 4);
        assert_eq!(&bytes[62..70], &[6, 5, 4, 0, 3, 2, 1, 255]);
        assert_eq!(&bytes[70..78], &[0x80, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn ico_256_dimension_is_zero_in_directory_only() {
        let bytes = icon_to_ico(&icon(256, 256, vec![255; 256 * 256 * 4])).unwrap();
        assert_eq!(&bytes[6..8], &[0, 0]);
        assert_eq!(u32_at(&bytes, 26), 256);
        assert_eq!(u32_at(&bytes, 30), 512);
        assert_eq!(bytes.len(), 22 + 40 + 256 * 256 * 4 + 32 * 256);
    }

    #[test]
    fn ico_and_mask_is_dword_aligned_and_msb_first() {
        let mut pixels = vec![255; 33 * 4];
        pixels[3] = 0;
        pixels[31 * 4 + 3] = 0;
        pixels[32 * 4 + 3] = 0;
        let bytes = icon_to_ico(&icon(33, 1, pixels)).unwrap();
        assert_eq!(&bytes[62 + 132..], &[0x80, 0, 0, 1, 0x80, 0, 0, 0]);
    }

    #[test]
    fn ico_rejects_invalid_dimensions_and_buffer_length() {
        for (width, height, data) in [
            (0, 1, vec![]),
            (1, 0, vec![]),
            (257, 1, vec![]),
            (1, 257, vec![]),
            (u32::MAX, u32::MAX, vec![]),
            (1, 1, vec![0; 3]),
            (1, 1, vec![0; 5]),
        ] {
            assert!(icon_to_ico(&icon(width, height, data)).is_err());
        }
    }

    #[test]
    fn executable_fallback_accepts_only_plain_drive_paths() {
        for (path, drive) in [
            (r"C:\Windows\system32\notepad.exe", 'C'),
            (r"d:\软件\应用.exe", 'd'),
        ] {
            let wide: Vec<u16> = path.encode_utf16().collect();
            assert_eq!(
                local_drive_root(&wide),
                Some([drive as u16, b':' as u16, b'\\' as u16, 0])
            );
        }
        for path in [
            r"\\server\share\app.exe",
            r"\\?\UNC\server\share\app.exe",
            r"\\?\C:\app.exe",
            r"\\.\C:\app.exe",
            r"\Device\HarddiskVolume1\app.exe",
            r"C:app.exe",
            r"C:\",
            r"app.exe",
            r"C:\folder\..\app.exe",
            r"C:\.\app.exe",
            r"C:\\app.exe",
            r"C:\app.exe:stream",
            "C:\\app\0.exe",
            "https://example/app.exe",
        ] {
            assert!(
                local_drive_root(&path.encode_utf16().collect::<Vec<_>>()).is_none(),
                "{path:?}"
            );
        }
    }

    #[test]
    fn legacy_mask_sets_transparency_and_preserves_color() {
        let rgba = decode_pixels(
            &[3, 2, 1, 0, 6, 5, 4, 0],
            &[0, 0, 0, 0, 255, 255, 255, 0],
            false,
        );
        assert_eq!(rgba, [1, 2, 3, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn native_alpha_is_unpremultiplied_and_ignores_and_mask() {
        let rgba = decode_pixels(&[16, 32, 64, 128, 200, 200, 200, 0], &[255; 8], true);
        assert_eq!(rgba, [128, 64, 32, 128, 0, 0, 0, 0]);
    }
}
