//! Pure helpers shared by the callback and host-independent safety tests.
use coralspy_hook_protocol::parse_hook_filename;

// richedit.h wraps EDITSTREAM in #pragma pack(push, 4), including on x64.
#[repr(C, packed(4))]
pub(crate) struct EditStream {
    pub cookie: usize,
    pub error: u32,
    pub callback: unsafe extern "system" fn(usize, *mut u8, i32, *mut i32) -> u32,
}

pub(crate) const SF_RTF: usize = 0x0002;

pub(crate) fn transfer_len(wanted: usize, used: usize, capacity: usize) -> usize {
    wanted.min(capacity.saturating_sub(used))
}

pub(crate) fn text_len(text: &[u16]) -> usize {
    text.iter()
        .position(|unit| *unit == 0)
        .unwrap_or(text.len())
}

pub(crate) fn module_path_nonce(path: &[u16]) -> Option<[u8; 16]> {
    let mut start = 0;
    for (index, unit) in path.iter().enumerate() {
        if *unit == b'\\' as u16 || *unit == b'/' as u16 {
            start = index + 1;
        }
    }
    let filename = &path[start..];
    let mut ascii = [0u8; 43];
    if filename.len() != ascii.len() {
        return None;
    }
    for (out, unit) in ascii.iter_mut().zip(filename.iter()) {
        if *unit > 127 {
            return None;
        }
        *out = *unit as u8;
    }
    if &ascii[..7] != b"cshook-"
        || &ascii[39..] != b".dll"
        || ascii[7..39]
            .iter()
            .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(b))
    {
        return None;
    }
    parse_hook_filename(&ascii).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    #[test]
    fn editstream_matches_pack4_sdk_layout() {
        assert_eq!(offset_of!(EditStream, cookie), 0);
        assert_eq!(offset_of!(EditStream, error), size_of::<usize>());
        assert_eq!(offset_of!(EditStream, callback), size_of::<usize>() + 4);
        assert_eq!(size_of::<EditStream>(), size_of::<usize>() * 2 + 4);
        assert_eq!(align_of::<EditStream>(), 4);
        assert_eq!(SF_RTF, 2);
    }

    #[test]
    fn transfer_budget_cannot_overflow_or_exceed_mapping() {
        assert_eq!(transfer_len(100, 0, 64), 64);
        assert_eq!(transfer_len(100, 62, 64), 2);
        assert_eq!(transfer_len(100, 64, 64), 0);
        assert_eq!(transfer_len(usize::MAX, usize::MAX, 64), 0);
        assert_eq!(transfer_len(usize::MAX, 0, 64), 64);
        assert_eq!(transfer_len(0, 0, 64), 0);
    }

    #[test]
    fn text_length_never_reads_past_buffer_or_converts_units() {
        assert_eq!(text_len(&[]), 0);
        assert_eq!(text_len(&[1, 2, 0, 3]), 2);
        assert_eq!(text_len(&[1, 2]), 2);
        assert_eq!(text_len(&[0xd800, 0]), 1);
    }

    fn nonce(path: &str) -> Option<[u8; 16]> {
        module_path_nonce(&path.encode_utf16().collect::<Vec<_>>())
    }

    #[test]
    fn exact_nonce_basename_is_required() {
        let valid = "cshook-0102030405060708090a0b0c0d0e0f10.dll";
        let expected = Some([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        assert_eq!(nonce(valid), expected);
        assert_eq!(nonce(&format!("C:\\用户\\private\\{valid}")), expected);
        assert_eq!(nonce(&format!("C:/private/{valid}")), expected);
        assert_eq!(nonce(&valid.to_uppercase()), None);
        assert_eq!(nonce(&format!("{valid}.dll")), None);
        assert_eq!(nonce("coralspy_hook_payload.dll"), None);
        assert_eq!(nonce("cshook-00000000000000000000000000000000.dll"), None);
        assert_eq!(nonce("cshook-0102030405060708090a0b0c0d0e0f1x.dll"), None);
        assert_eq!(nonce(&format!("{valid}/other.dll")), None);
    }
}
