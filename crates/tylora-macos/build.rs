fn main() {
    use std::{env, path::PathBuf, process::Command};
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let object = out.join("window_move_bridge.o");
    let archive = out.join("libtylora_window_move_bridge.a");
    let status = Command::new("clang")
        .args([
            "-fno-objc-arc",
            "-fvisibility=hidden",
            "-c",
            "src/spaces/window_move_bridge.m",
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("clang is required for the macOS bridge");
    assert!(status.success(), "Objective-C bridge compilation failed");
    let status = Command::new("ar")
        .arg("rcs")
        .arg(&archive)
        .arg(&object)
        .status()
        .expect("ar is required for the macOS bridge");
    assert!(status.success(), "Objective-C bridge archive failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=tylora_window_move_bridge");
    println!("cargo:rustc-link-lib=framework=ApplicationServices");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rerun-if-changed=src/spaces/window_move_bridge.m");
}
