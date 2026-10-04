//! Real Win32 fixture tests. These do not inspect arbitrary user windows.
#![cfg(windows)]
use coralspynext::platform;
use std::ptr::null_mut;
use windows_sys::Win32::{
    Foundation::HWND,
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentProcessId},
    UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_CHILD, WS_OVERLAPPEDWINDOW},
};
struct TestWindow(HWND);
impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}
fn create(class: &str, title: &str, parent: HWND) -> TestWindow {
    let class: Vec<u16> = class.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    let style = if parent.is_null() {
        WS_OVERLAPPEDWINDOW
    } else {
        WS_CHILD
    };
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            style,
            100,
            100,
            320,
            200,
            parent,
            null_mut(),
            GetModuleHandleW(null_mut()),
            null_mut(),
        )
    };
    assert!(
        !hwnd.is_null(),
        "Win32 fixture window creation failed: {}",
        std::io::Error::last_os_error()
    );
    TestWindow(hwnd)
}
#[test]
fn inspects_real_window_metadata_without_own_caption() {
    let window = create("STATIC", "Fixture only — 测试窗口", null_mut());
    let info = platform::inspect_window(window.0 as u64).unwrap();
    assert_eq!(info.hwnd, window.0 as u64);
    assert_eq!(info.pid, unsafe { GetCurrentProcessId() });
    assert_eq!(info.class_name.to_ascii_lowercase(), "static");
    assert!(info.rect.width() > 0 && info.rect.height() > 0);
    assert!(info.title.contains("自身"));
    assert!(!info.title.contains("Fixture only"));
}
#[test]
fn custom_child_text_is_never_exposed() {
    let parent = create("STATIC", "parent fixture", null_mut());
    let child = create("STATIC", "PRIVATE INPUT SENTINEL 你好", parent.0);
    let info = platform::inspect_window(child.0 as u64).unwrap();
    assert_eq!(info.parent, parent.0 as u64);
    assert!(!info.title.contains("PRIVATE INPUT SENTINEL"));
    assert!(info.text_status.contains("子控件") || info.text_status.contains("元数据"));
}
#[test]
fn invalid_and_destroyed_handles_fail_cleanly() {
    assert!(!platform::is_window(0));
    assert!(platform::inspect_window(0).is_err());
    let window = create("STATIC", "temporary fixture", null_mut());
    let hwnd = window.0 as u64;
    drop(window);
    assert!(!platform::is_window(hwnd));
    assert!(platform::inspect_window(hwnd).is_err());
}
#[test]
fn keyboard_api_rejects_non_picker_keys() {
    assert!(!platform::key_down(0x41)); // A: never read unrelated keys.
    assert!(!platform::key_down(0x0D)); // Enter: cannot activate target controls.
}
