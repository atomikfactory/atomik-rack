//! Windows only: the MCP server links the app, whose Save box is a task dialog that needs
//! Common Controls version 6, so the program embeds the same manifest the app does. On every
//! other platform this build script does nothing.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../app/windows/atomik.manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest directory");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{dir}/../app/windows/atomik.manifest");
    }
}
