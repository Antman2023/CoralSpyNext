use std::{env, fs, path::PathBuf};
fn main() {
    println!("cargo:rustc-check-cfg=cfg(embedded_payload)");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let target = env::var("TARGET").unwrap();
    let machine: u16 = match target.as_str() {
        "x86_64-pc-windows-gnu" | "x86_64-pc-windows-msvc" => 0x8664,
        "i686-pc-windows-gnu" | "i686-pc-windows-msvc" => 0x014c,
        _ => panic!("Only matching x86/x64 Windows builds are supported"),
    };
    // Intentionally fixed, internal build output. No caller-controlled payload path.
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let dll = root
        .join("target")
        .join(&target)
        .join("release/coralspy_hook_payload.dll");
    println!("cargo:rerun-if-changed={}", dll.display());
    let bytes = fs::read(&dll).expect("Build coralspy-hook-payload --release for this target first, using scripts/build-windows.sh");
    assert!(
        bytes.len() > 0x40 && &bytes[..2] == b"MZ",
        "Internal payload is not PE"
    );
    let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
    assert!(
        pe.checked_add(6).is_some_and(|v| v <= bytes.len()),
        "Invalid PE header"
    );
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    assert_eq!(
        u16::from_le_bytes(bytes[pe + 4..pe + 6].try_into().unwrap()),
        machine,
        "Payload architecture mismatch"
    );
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("payload.dll"),
        bytes,
    )
    .unwrap();
    println!("cargo:rustc-cfg=embedded_payload");
}
