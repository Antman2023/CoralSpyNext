mod build_stamp;

fn main() {
    println!("cargo:rerun-if-changed=build_stamp.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let override_epoch = match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => panic!("Invalid SOURCE_DATE_EPOCH: {error}"),
    };
    let now = if override_epoch.is_some() {
        0
    } else {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Build system clock predates the Unix epoch")
            .as_secs()
    };
    let seconds = build_stamp::resolve_epoch(override_epoch.as_deref(), now)
        .expect("Invalid build timestamp");
    let stamp = build_stamp::format_utc(seconds).expect("Cannot format build timestamp");
    println!("cargo:rustc-env=BUILD_UTC={stamp}");
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=assets/coralspynext.ico");
    println!("cargo:rerun-if-changed=assets/coralspynext.png");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/app.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
