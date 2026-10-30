//! Windows only: embeds the application icon in `atomik.exe`, so Explorer, the taskbar, the Start
//! menu and the installer show it. On every other platform this build script does nothing.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=windows/atomik.rc");
    println!("cargo:rerun-if-changed=../../assets/icon/atomik-rack.ico");
    #[cfg(windows)]
    embed_resource::compile_for("windows/atomik.rc", ["atomik"], embed_resource::NONE)
        .manifest_required()
        .expect("embed the application icon");
}
