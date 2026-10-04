//! Fixed-function, explicit-operation capture API. No DLL paths or executable bytes.
//! Call `capture` from a background thread; the broker shows its own capture indicator.
/// Result flags belong to this protocol, not the raw Win32 style/state namespace.
pub use coralspy_hook_protocol::{
    RECORD_FLAG_CHECKED, RECORD_FLAG_DEFAULT, RECORD_FLAG_DEPTH_LIMIT, RECORD_FLAG_DISABLED,
    RECORD_FLAG_EXPANDED, RECORD_FLAG_HAS_CHILDREN, RECORD_FLAG_NODE_LIMIT, RECORD_FLAG_OWNER_DRAW,
    RECORD_FLAG_SELECTED, RECORD_FLAG_SEPARATOR, RECORD_FLAG_TEXT_TRUNCATED,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    RichEditRtf,
    ListView,
    TreeView,
    MenuTarget,
    MenuDesktopOnce,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub hwnd: u64,
    pub pid: u32,
    pub tid: u32,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X86,
    X64,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureLimits {
    pub timeout_ms: u32,
    pub max_rows: u32,
    pub max_columns: u32,
    pub max_nodes: u32,
    pub max_depth: u32,
    pub max_text_units: u32,
    pub max_result_bytes: u32,
}
impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            timeout_ms: 5_000,
            max_rows: 512,
            max_columns: 32,
            max_nodes: 1024,
            max_depth: 32,
            max_text_units: 2048,
            max_result_bytes: 1024 * 1024,
        }
    }
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UiLanguage {
    #[default]
    English,
    SimplifiedChinese,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRequest {
    #[serde(default)]
    pub language: UiLanguage,
    pub operation: Operation,
    pub architecture: Architecture,
    pub target: Option<Target>,
    pub limits: CaptureLimits,
    /// Required on every invocation. The broker additionally creates and verifies its indicator.
    pub visible_capture_consent: bool,
    /// Required separately for the explicitly selected, short-lived desktop-wide menu operation.
    pub desktop_menu_consent: bool,
}
impl CaptureRequest {
    pub fn validate(&self) -> Result<(), CaptureError> {
        let bad = |s: &str| CaptureError::new(ErrorCode::InvalidInput, s);
        if !self.visible_capture_consent {
            return Err(bad("An explicit visible capture operation is required"));
        }
        match (self.operation, self.target) {
            (Operation::MenuDesktopOnce, None) if self.desktop_menu_consent => {}
            (Operation::MenuDesktopOnce, _) => {
                return Err(bad(
                    "Desktop menu capture requires separate explicit consent and no target",
                ))
            }
            (_, Some(t))
                if t.hwnd != 0 && t.pid != 0 && t.tid != 0 && !self.desktop_menu_consent => {}
            _ => {
                return Err(bad(
                    "A selected HWND/PID/TID is required for this operation",
                ))
            }
        }
        let l = self.limits;
        if l.timeout_ms == 0
            || l.timeout_ms > 10_000
            || l.max_rows == 0
            || l.max_rows > 512
            || l.max_columns == 0
            || l.max_columns > 32
            || l.max_nodes == 0
            || l.max_nodes > 1024
            || l.max_depth == 0
            || l.max_depth > 32
            || l.max_text_units == 0
            || l.max_text_units > 2048
            || l.max_result_bytes < 20
            || l.max_result_bytes > 1024 * 1024
        {
            return Err(bad("Capture limits are outside the fixed safety bounds"));
        }
        if matches!(self.architecture, Architecture::X86)
            && self.target.is_some_and(|t| t.hwnd > u32::MAX as u64)
        {
            return Err(bad("The HWND does not fit the selected x86 architecture"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    InvalidRequest,
    TargetMismatch,
    ClassMismatch,
    PasswordControl,
    AccessDenied,
    IntegrityMismatch,
    WrongArchitecture,
    TimedOut,
    Cancelled,
    Unsupported,
    ControlError,
    Ipc,
    MissingBroker,
    OsError,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureError {
    pub code: ErrorCode,
    pub message: String,
    pub win32_error: Option<u32>,
}
impl CaptureError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            win32_error: None,
        }
    }
    pub fn os(code: ErrorCode, message: impl Into<String>, error: u32) -> Self {
        Self {
            code,
            message: message.into(),
            win32_error: Some(error),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListColumn {
    pub index: u32,
    pub flags: u32,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub row: u32,
    pub column: u32,
    pub flags: u32,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub depth: u32,
    pub id: u32,
    pub flags: u32,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuItem {
    pub depth: u32,
    pub id: u32,
    pub flags: u32,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureData {
    RichEditRtf {
        bytes: Vec<u8>,
        complete: bool,
    },
    ListView {
        columns: Vec<ListColumn>,
        cells: Vec<Cell>,
    },
    TreeView {
        nodes: Vec<TreeNode>,
    },
    Menu {
        root_menu: u64,
        items: Vec<MenuItem>,
    },
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReportedCounts {
    pub rows: Option<u32>,
    pub columns: Option<u32>,
    pub nodes: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureResult {
    pub actual_target: Target,
    pub truncated: bool,
    pub reported_counts: ReportedCounts,
    pub captured_records: u32,
    pub data: CaptureData,
}
impl CaptureResult {
    /// Only complete raw streams may be exported as an RTF document. A truncated
    /// stream is diagnostic partial bytes and can end in the middle of RTF syntax.
    pub fn complete_rtf_bytes(&self) -> Option<&[u8]> {
        match &self.data {
            CaptureData::RichEditRtf {
                bytes,
                complete: true,
            } if !self.truncated => Some(bytes),
            _ => None,
        }
    }
}
pub type CaptureOutcome = Result<CaptureResult, CaptureError>;

/// Launch only the fixed broker matching the target architecture from this app's directory.
/// Cancellation kills this one-shot broker job, automatically removing its OS hooks.
pub fn capture(request: CaptureRequest, cancel: &AtomicBool) -> CaptureOutcome {
    request.validate()?;
    #[cfg(windows)]
    {
        windows::capture(request, cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = cancel;
        Err(CaptureError::new(
            ErrorCode::Unsupported,
            "Windows is required",
        ))
    }
}
/// Resolve the current selected target's bitness using limited query access only.
pub fn target_architecture(target: Target) -> Result<Architecture, CaptureError> {
    #[cfg(windows)]
    {
        windows::target_architecture(target)
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        Err(CaptureError::new(
            ErrorCode::Unsupported,
            "Windows is required",
        ))
    }
}
#[cfg(windows)]
mod windows;

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> CaptureRequest {
        CaptureRequest {
            language: UiLanguage::English,
            operation: Operation::ListView,
            architecture: Architecture::X64,
            target: Some(Target {
                hwnd: 1,
                pid: 2,
                tid: 3,
            }),
            limits: CaptureLimits::default(),
            visible_capture_consent: true,
            desktop_menu_consent: false,
        }
    }
    #[test]
    fn explicit_capture_required() {
        let mut r = request();
        assert!(r.validate().is_ok());
        r.visible_capture_consent = false;
        assert!(r.validate().is_err());
    }
    #[test]
    fn desktop_requires_separate_consent() {
        let mut r = request();
        r.operation = Operation::MenuDesktopOnce;
        assert!(r.validate().is_err());
        r.target = None;
        assert!(r.validate().is_err());
        r.desktop_menu_consent = true;
        assert!(r.validate().is_ok());
        r.operation = Operation::TreeView;
        assert!(r.validate().is_err());
    }
    #[test]
    fn reject_caps_and_unexpected_fields() {
        let mut r = request();
        r.limits.timeout_ms = 10_001;
        assert!(r.validate().is_err());
        assert!(serde_json::from_str::<CaptureRequest>(r#"{"dll":"evil.dll"}"#).is_err());
    }
    #[test]
    fn preserve_rtf_bytes_json() {
        let d = CaptureData::RichEditRtf {
            bytes: vec![0, 255, 123, 92, 125],
            complete: true,
        };
        let encoded = serde_json::to_vec(&d).unwrap();
        if let CaptureData::RichEditRtf { bytes, .. } = serde_json::from_slice(&encoded).unwrap() {
            assert_eq!(bytes, [0, 255, 123, 92, 125]);
        } else {
            panic!()
        }
    }
    #[test]
    fn incomplete_rtf_can_never_use_export_accessor() {
        let mut result = CaptureResult {
            actual_target: Target {
                hwnd: 1,
                pid: 2,
                tid: 3,
            },
            truncated: false,
            reported_counts: ReportedCounts::default(),
            captured_records: 0,
            data: CaptureData::RichEditRtf {
                bytes: b"{\\rtf1 test}".to_vec(),
                complete: true,
            },
        };
        assert!(result.complete_rtf_bytes().is_some());
        result.truncated = true;
        assert!(result.complete_rtf_bytes().is_none());
        result.truncated = false;
        if let CaptureData::RichEditRtf { complete, .. } = &mut result.data {
            *complete = false;
        }
        assert!(result.complete_rtf_bytes().is_none());
    }
    #[test]
    fn each_zero_or_excess_cap_is_rejected() {
        for field in 0..7 {
            for excessive in [false, true] {
                let mut r = request();
                let l = &mut r.limits;
                let (value, max) = match field {
                    0 => (&mut l.timeout_ms, 10000),
                    1 => (&mut l.max_rows, 512),
                    2 => (&mut l.max_columns, 32),
                    3 => (&mut l.max_nodes, 1024),
                    4 => (&mut l.max_depth, 32),
                    5 => (&mut l.max_text_units, 2048),
                    _ => (&mut l.max_result_bytes, 1024 * 1024),
                };
                *value = if excessive { max + 1 } else { 0 };
                assert!(r.validate().is_err());
            }
        }
    }
    #[test]
    fn x86_identity_and_unexpected_payload_fields_are_rejected() {
        let mut r = request();
        r.architecture = Architecture::X86;
        r.target.as_mut().unwrap().hwnd = u32::MAX as u64 + 1;
        assert!(r.validate().is_err());
        let mut json = serde_json::to_value(request()).unwrap();
        json["dll_path"] = serde_json::json!("payload.dll");
        assert!(serde_json::from_value::<CaptureRequest>(json).is_err());
    }
}
