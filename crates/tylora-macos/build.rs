fn main() {
    use std::{env, path::PathBuf, process::Command};
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let archive = out.join("libtylora_window_move_bridge.a");
    let mut objects = Vec::new();
    for source in [
        "src/spaces/window_move_bridge.m",
        "src/native/status_item.m",
        "src/native/cheat_sheet.m",
    ] {
        let name = PathBuf::from(source).with_extension("o");
        let object = out.join(name.file_name().expect("object name"));
        let status = Command::new("clang")
            .args(["-fno-objc-arc", "-fvisibility=hidden", "-c", source, "-o"])
            .arg(&object)
            .status()
            .expect("clang is required for the macOS bridge");
        assert!(status.success(), "Objective-C compilation failed: {source}");
        println!("cargo:rerun-if-changed={source}");
        objects.push(object);
    }
    let _ = std::fs::remove_file(&archive);
    let status = Command::new("ar")
        .arg("rcs")
        .arg(&archive)
        .args(&objects)
        .status()
        .expect("ar is required for the macOS bridge");
    assert!(status.success(), "Objective-C bridge archive failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=tylora_window_move_bridge");
    println!("cargo:rustc-link-lib=framework=ApplicationServices");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
}
