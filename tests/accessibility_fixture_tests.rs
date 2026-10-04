//! UI Automation integration tests against owned, disposable Win32 fixtures.
//! Run on an unlocked Windows 11 desktop: cargo test --test accessibility_fixture_tests.
//! Linux cross-compilation checks API/type correctness, not provider behavior.
#![cfg(windows)]
use coralspynext::{accessibility, model::ContentSnapshot};
use std::{
    io::Read,
    mem::{size_of, zeroed},
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HWND},
    System::{
        LibraryLoader::{GetModuleHandleW, LoadLibraryW},
        Threading::CREATE_NO_WINDOW,
    },
    UI::{Controls::*, WindowsAndMessaging::*},
};

static SERIAL: Mutex<()> = Mutex::new(());

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
unsafe fn window(class: &str, text: &str, parent: HWND, style: u32, y: i32) -> HWND {
    let hwnd = CreateWindowExW(
        0,
        wide(class).as_ptr(),
        wide(text).as_ptr(),
        style,
        10,
        y,
        420,
        120,
        parent,
        null_mut(),
        GetModuleHandleW(null_mut()),
        null_mut(),
    );
    assert!(
        !hwnd.is_null(),
        "fixture creation failed: {}",
        std::io::Error::last_os_error()
    );
    hwnd
}

struct Fixture {
    list: u64,
    combo: u64,
    tree: u64,
    listview: u64,
    rich: u64,
    password: u64,
    root: u64,
    stop: Arc<AtomicBool>,
    owner: Option<JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let (send, receive) = mpsc::sync_channel(1);
        let owner = thread::spawn(move || unsafe {
            let init = INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_LISTVIEW_CLASSES | ICC_TREEVIEW_CLASSES,
            };
            assert_ne!(InitCommonControlsEx(&init), 0);
            let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
            assert!(!library.is_null(), "Windows RichEdit library unavailable");
            let root = window(
                "STATIC",
                "CoralSpy UIA fixture",
                null_mut(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                10,
            );
            let normal = WS_CHILD | WS_VISIBLE | WS_TABSTOP;
            let list = window("LISTBOX", "", root, normal | LBS_HASSTRINGS as u32, 10);
            for text in ["List alpha 甲", "List beta 乙"] {
                assert_ne!(
                    SendMessageW(list, LB_ADDSTRING, 0, wide(text).as_ptr() as isize),
                    LB_ERR as isize
                );
            }
            let combo = window("COMBOBOX", "", root, normal | CBS_DROPDOWNLIST as u32, 140);
            for text in ["Combo alpha", "Combo beta"] {
                assert_ne!(
                    SendMessageW(combo, CB_ADDSTRING, 0, wide(text).as_ptr() as isize),
                    CB_ERR as isize
                );
            }
            SendMessageW(combo, CB_SETCURSEL, 1, 0);
            let tree = window("SysTreeView32", "", root, normal, 270);
            let mut item_text = wide("Tree leaf 丙");
            let mut insert: TVINSERTSTRUCTW = zeroed();
            insert.hParent = TVI_ROOT;
            insert.hInsertAfter = TVI_LAST;
            insert.Anonymous.item.mask = TVIF_TEXT;
            insert.Anonymous.item.pszText = item_text.as_mut_ptr();
            assert_ne!(
                SendMessageW(tree, TVM_INSERTITEMW, 0, &insert as *const _ as isize),
                0
            );
            let listview = window("SysListView32", "", root, normal | LVS_REPORT, 400);
            for (i, text) in ["Column one", "Column two"].iter().enumerate() {
                let mut label = wide(text);
                let mut column: LVCOLUMNW = zeroed();
                column.mask = LVCF_TEXT | LVCF_WIDTH;
                column.pszText = label.as_mut_ptr();
                column.cx = 160;
                assert_ne!(
                    SendMessageW(listview, LVM_INSERTCOLUMNW, i, &column as *const _ as isize),
                    -1
                );
            }
            let mut row_text = wide("Row delta 丁");
            let mut row: LVITEMW = zeroed();
            row.mask = LVIF_TEXT;
            row.pszText = row_text.as_mut_ptr();
            assert_ne!(
                SendMessageW(listview, LVM_INSERTITEMW, 0, &row as *const _ as isize),
                -1
            );
            let mut sub_text = wide("Cell epsilon 戊");
            row.iSubItem = 1;
            row.pszText = sub_text.as_mut_ptr();
            assert_ne!(
                SendMessageW(listview, LVM_SETITEMTEXTW, 0, &row as *const _ as isize),
                0
            );
            let rich = window(
                "RICHEDIT50W",
                "RichEdit public text 己\r\nSecond paragraph.",
                root,
                normal | ES_MULTILINE as u32,
                530,
            );
            let password = window(
                "EDIT",
                "DO_NOT_EXPORT_PASSWORD_8f72",
                root,
                normal | ES_PASSWORD as u32,
                660,
            );
            let _protected_descendant = window(
                "STATIC",
                "DO_NOT_EXPORT_PROTECTED_CHILD_2e01",
                password,
                WS_CHILD | WS_VISIBLE,
                0,
            );
            send.send([root, list, combo, tree, listview, rich, password].map(|h| h as u64))
                .unwrap();
            let mut message: MSG = zeroed();
            while !thread_stop.load(Ordering::Acquire) {
                while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                thread::sleep(Duration::from_millis(2));
            }
            DestroyWindow(root); // Also destroys every child before unloading RichEdit.
            FreeLibrary(library);
        });
        let [root, list, combo, tree, listview, rich, password] = receive
            .recv_timeout(Duration::from_secs(10))
            .expect("fixture owner did not initialize");
        Self {
            root,
            list,
            combo,
            tree,
            listview,
            rich,
            password,
            stop,
            owner: Some(owner),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(owner) = self.owner.take() {
            owner.join().expect("fixture owner panicked");
        }
    }
}

fn worker(hwnd: u64) -> Result<ContentSnapshot, String> {
    // Calling accessibility::inspect here would re-launch the test harness;
    // explicitly use the application executable so its dispatcher is tested.
    // CI may relocate the test executable from a cross-compilation machine.
    // This override exists only in this test, never in the shipped application.
    let application = std::env::var_os("CORALSPY_TEST_APP")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_coralspynext").into());
    assert!(
        application.is_absolute() && application.is_file(),
        "CORALSPY_TEST_APP must identify the absolute path of the source-built application"
    );
    let mut child = Command::new(application)
        .args(["--accessibility-worker", &hwnd.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("worker spawn");
    let stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(16 * 1_048_576 + 1)
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "worker failed: {status}");
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            panic!("UIA fixture worker exceeded 12-second test limit");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.wait();
    let bytes = reader.join().unwrap();
    assert!(bytes.len() <= 16 * 1_048_576);
    serde_json::from_slice(&bytes).expect("worker must emit JSON Result, not start a GUI")
}

#[test]
fn classic_controls_expose_real_provider_content() {
    let _guard = SERIAL.lock().unwrap();
    let fixture = Fixture::new();
    for (hwnd, expected) in [
        (fixture.list, "List alpha 甲"),
        (fixture.combo, "Combo beta"),
        (fixture.tree, "Tree leaf 丙"),
        (fixture.listview, "Row delta 丁"),
        (fixture.listview, "Cell epsilon 戊"),
        (fixture.rich, "RichEdit public text 己"),
    ] {
        let snapshot = worker(hwnd).unwrap_or_else(|e| panic!("UIA fixture {expected}: {e}"));
        assert!(
            snapshot.text.contains(expected),
            "Missing {expected}; result: {snapshot:?}"
        );
        assert!(snapshot.nodes.len() <= accessibility::MAX_NODES);
        assert!(snapshot
            .nodes
            .iter()
            .all(|n| n.depth <= accessibility::MAX_DEPTH));
        assert!(snapshot.text.len() <= accessibility::MAX_TEXT_BYTES);
    }
}

#[test]
fn password_and_its_descendants_never_enter_any_export_field() {
    let _guard = SERIAL.lock().unwrap();
    let fixture = Fixture::new();
    for hwnd in [fixture.password, fixture.root] {
        let snapshot = worker(hwnd).expect("password inspection should return a redacted result");
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("DO_NOT_EXPORT_PASSWORD"));
        assert!(!json.contains("DO_NOT_EXPORT_PROTECTED_CHILD"));
        assert!(snapshot
            .nodes
            .iter()
            .any(|n| n.is_password && n.name.is_empty()));
        assert!(snapshot
            .warnings
            .iter()
            .any(|w| w.contains("密码") || w.contains("保护")));
    }
}

#[test]
fn invalid_and_destroyed_windows_return_errors() {
    let _guard = SERIAL.lock().unwrap();
    assert!(accessibility::inspect(0).is_err());
    assert!(worker(0).is_err());
    let hwnd = {
        let fixture = Fixture::new();
        fixture.rich
    };
    assert!(worker(hwnd).is_err());
}
