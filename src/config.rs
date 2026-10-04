//! User preferences only. No captured window titles, content, or colors persist.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HotkeyPreference {
    pub modifiers: u32,
    pub key: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub version: u32,
    pub language: String,
    pub dark: bool,
    pub always_on_top: bool,
    pub tray_enabled: bool,
    pub minimize_to_tray: bool,
    pub hotkeys_enabled: bool,
    pub hotkeys: [HotkeyPreference; 3],
    pub highlight_text: [u8; 3],
    pub highlight_background: [u8; 3],
    pub highlight_bold: bool,
}
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: 1,
            language: "zh-CN".into(),
            dark: false,
            always_on_top: false,
            tray_enabled: true,
            minimize_to_tray: true,
            hotkeys_enabled: true,
            hotkeys: [
                HotkeyPreference {
                    modifiers: 3,
                    key: 0x57,
                },
                HotkeyPreference {
                    modifiers: 3,
                    key: 0x43,
                },
                HotkeyPreference {
                    modifiers: 3,
                    key: 0x53,
                },
            ],
            highlight_text: [255, 0, 0],
            highlight_background: [255, 255, 0],
            highlight_bold: true,
        }
    }
}
impl AppSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("不支持此设置文件版本 / Unsupported settings version".into());
        }
        if !matches!(self.language.as_str(), "zh-CN" | "en-US") {
            return Err("语言只支持中文或英文 / Language must be Chinese or English".into());
        }
        for h in &self.hotkeys {
            if h.modifiers & !0x000f != 0
                || h.modifiers == 0
                || !(0x30..=0x5a).contains(&h.key) && !(0x70..=0x87).contains(&h.key)
            {
                return Err(
                    "热键应带 Ctrl/Alt/Shift/Win 修饰键及字母、数字或 F1-F24 / Invalid hotkey"
                        .into(),
                );
            }
        }
        for i in 0..3 {
            for j in i + 1..3 {
                if self.hotkeys[i] == self.hotkeys[j] {
                    return Err("三个热键不能重复 / Hotkeys must be distinct".into());
                }
            }
        }
        Ok(())
    }
}
fn settings_path() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(|p| PathBuf::from(p).join("CoralSpyNext").join("settings.json"))
        .ok_or_else(|| "找不到 LOCALAPPDATA，设置未保存 / LOCALAPPDATA unavailable".into())
}
pub fn load() -> (AppSettings, Option<String>) {
    let p = match settings_path() {
        Ok(p) => p,
        Err(e) => return (AppSettings::default(), Some(e)),
    };
    let data = match std::fs::read(p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (AppSettings::default(), None)
        }
        Err(e) => {
            return (
                AppSettings::default(),
                Some(format!(
                    "读取设置失败，已用默认设置 / Settings read failed: {e}"
                )),
            )
        }
    };
    if data.len() > 65536 {
        return (
            AppSettings::default(),
            Some("设置文件过大，已使用默认值 / Settings file too large".into()),
        );
    }
    match serde_json::from_slice::<AppSettings>(&data)
        .and_then(|s| s.validate().map(|_| s).map_err(serde::de::Error::custom))
    {
        Ok(s) => (s, None),
        Err(e) => (
            AppSettings::default(),
            Some(format!("设置无效，已使用默认值 / Invalid settings: {e}")),
        ),
    }
}
pub fn save(settings: &AppSettings) -> Result<(), String> {
    settings.validate()?;
    let p = settings_path()?;
    let dir = p.parent().ok_or("Invalid settings path")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let data = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    let temp = dir.join(format!("settings.{}.tmp", std::process::id()));
    std::fs::write(&temp, data).map_err(|e| format!("写入设置失败 / Cannot save settings: {e}"))?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let src: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
        let result = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                src.as_ptr(),
                dst.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            let e = std::io::Error::last_os_error();
            let _ = std::fs::remove_file(temp);
            return Err(format!("保存设置失败 / Cannot replace settings: {e}"));
        }
    }
    #[cfg(not(windows))]
    std::fs::rename(temp, p).map_err(|e| e.to_string())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_valid() {
        assert!(AppSettings::default().validate().is_ok());
    }
    #[test]
    fn rejects_duplicates() {
        let mut s = AppSettings::default();
        s.hotkeys[1] = s.hotkeys[0];
        assert!(s.validate().is_err());
    }
    #[test]
    fn settings_roundtrip() {
        let s = AppSettings::default();
        let t: AppSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s.hotkeys, t.hotkeys);
    }
    #[test]
    fn rejects_corrupt_modifiers() {
        let mut s = AppSettings::default();
        s.hotkeys[0].modifiers = 0xffff;
        assert!(s.validate().is_err());
    }
}
