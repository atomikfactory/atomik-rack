//! Windows only: embeds the application icon in `atomik.exe`, so Explorer, the taskbar, the Start
//! menu and the installer show it, and a manifest asking for Common Controls version 6 in every
//! program of this package (the Save / Don't Save / Cancel box is a task dialog). On every
//! other platform this build script does nothing.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=windows/atomik.rc");
    println!("cargo:rerun-if-changed=../../assets/icon/atomik-rack.ico");
    println!("cargo:rerun-if-changed=windows/atomik.manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest directory");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{dir}\\windows\\atomik.manifest");
    }
    #[cfg(windows)]
    embed_resource::compile_for("windows/atomik.rc", ["atomik"], embed_resource::NONE)
        .manifest_required()
        .expect("embed the application icon");
}
