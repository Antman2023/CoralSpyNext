use coralspynext::model::*;
#[test]
fn handles_are_64_bit() {
    assert_eq!(hwnd_text(0x1234_abcd_9876_fedc), "0x1234ABCD9876FEDC");
}
#[test]
fn colors_match_windows_colorref_order() {
    let c = ColorSample {
        x: -1920,
        y: 24,
        r: 0x12,
        g: 0x34,
        b: 0x56,
    };
    assert_eq!(c.hex(), "#123456");
    assert_eq!(c.rgb(), "rgb(18, 52, 86)");
    assert_eq!(c.colorref(), 0x563412);
}
#[test]
fn negative_monitor_coordinates_preserved() {
    let r = WindowRect {
        left: -1920,
        top: -120,
        right: 0,
        bottom: 1080,
    };
    assert_eq!(r.width(), 1920);
    assert_eq!(r.height(), 1200);
}
#[test]
fn dimension_arithmetic_is_bounded() {
    let r = WindowRect {
        left: i32::MIN,
        right: i32::MAX,
        ..Default::default()
    };
    assert_eq!(r.width(), i32::MAX);
}
#[test]
fn search_accepts_unicode_class_pid_and_hex() {
    let n = WindowNode {
        hwnd: 0xabcd,
        title: "文档 - Notepad".into(),
        class_name: "Notepad".into(),
        pid: 420,
        ..Default::default()
    };
    for q in ["文档", "NOTEpad", "420", "abcd", " "] {
        assert!(matches_filter(&n, q));
    }
    assert!(!matches_filter(&n, "missing"));
}
#[test]
fn export_roundtrips_unicode_and_newlines() {
    let i = WindowInfo {
        title: "你好\nRust 🦀".into(),
        hwnd: 0xffff_eeee_dddd_cccc,
        rect: WindowRect {
            left: -100,
            top: 1,
            right: 800,
            bottom: 601,
        },
        ..Default::default()
    };
    let s = serde_json::to_string_pretty(&i).unwrap();
    let parsed: WindowInfo = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed.title, i.title);
    assert_eq!(parsed.hwnd, i.hwnd);
    assert!(window_text(&i).contains("900 × 600"));
}
