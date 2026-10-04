//! Platform-independent checks for the checked-in executable/runtime artwork.
const ICO: &[u8] = include_bytes!("../assets/coralspynext.ico");
const PNG: &[u8] = include_bytes!("../assets/coralspynext.png");
const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn be32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn assert_rgba_png(bytes: &[u8], size: u32) {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(be32(bytes, 8), 13, "PNG must start with a complete IHDR");
    assert_eq!(&bytes[12..16], b"IHDR");
    assert_eq!((be32(bytes, 16), be32(bytes, 20)), (size, size));
    assert_eq!(&bytes[24..26], &[8, 6], "8-bit RGBA preserves transparency");
    assert_eq!(&bytes[bytes.len() - 8..bytes.len() - 4], b"IEND");
}

#[test]
fn executable_icon_contains_all_required_sizes() {
    assert_eq!(le16(ICO, 0), 0);
    assert_eq!(le16(ICO, 2), 1, "ICON rather than CURSOR");
    assert_eq!(usize::from(le16(ICO, 4)), SIZES.len());
    let mut end = 6 + 16 * SIZES.len();
    for (index, size) in SIZES.into_iter().enumerate() {
        let entry = 6 + 16 * index;
        let stored_size = if size == 256 { 0 } else { size as u8 };
        assert_eq!(&ICO[entry..entry + 4], &[stored_size, stored_size, 0, 0]);
        assert_eq!(le16(ICO, entry + 4), 1);
        assert_eq!(le16(ICO, entry + 6), 32);
        let length = le32(ICO, entry + 8) as usize;
        let offset = le32(ICO, entry + 12) as usize;
        assert_eq!(offset, end, "frames must be contiguous and not overlap");
        end = offset.checked_add(length).unwrap();
        assert_rgba_png(&ICO[offset..end], size);
    }
    assert_eq!(end, ICO.len(), "no unreferenced icon data");
}

#[test]
fn runtime_png_is_the_exact_largest_executable_icon_frame() {
    let entry = 6 + 16 * (SIZES.len() - 1);
    let offset = le32(ICO, entry + 12) as usize;
    let length = le32(ICO, entry + 8) as usize;
    assert_eq!(PNG, &ICO[offset..offset + length]);
    assert_rgba_png(PNG, 256);
}

#[test]
fn icon_resource_is_embedded_and_tracked_by_the_build() {
    let resource = include_str!("../assets/app.rc");
    assert!(resource.contains("101 ICON \"assets/coralspynext.ico\""));
    let build = include_str!("../build.rs");
    assert!(build.contains("cargo:rerun-if-changed=assets/coralspynext.ico"));
    assert!(build.contains("cargo:rerun-if-changed=assets/coralspynext.png"));
}
