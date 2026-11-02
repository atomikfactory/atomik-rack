//! Search paths per OS (checked against the CLAP `entry.h` comment), `CLAP_PATH`, the
//! recursive file search, the symlink and depth cut, deterministic order.

use atomik_device_sdk::PluginFormat;
use atomik_plugin_host::paths::{
    MAX_SEARCH_DEPTH, Os, default_search_paths, find_plugin_files, search_roots, vst2_only_folders,
};
use std::ffi::OsString;
use std::path::PathBuf;

fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |k| {
        pairs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| OsString::from(v))
    }
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-hs-paths-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn clap_defaults_per_os_match_the_header() {
    let win = env(&[("COMMONPROGRAMFILES", "C:/cpf"), ("LOCALAPPDATA", "C:/lad")]);
    assert_eq!(
        default_search_paths(PluginFormat::Clap, Os::Windows, &win),
        [
            PathBuf::from("C:/cpf").join("CLAP"),
            PathBuf::from("C:/lad")
                .join("Programs")
                .join("Common")
                .join("CLAP")
        ]
    );
    let home = env(&[("HOME", "/home/me")]);
    assert_eq!(
        default_search_paths(PluginFormat::Clap, Os::MacOs, &home),
        [
            PathBuf::from("/Library/Audio/Plug-Ins/CLAP"),
            PathBuf::from("/home/me").join("Library/Audio/Plug-Ins/CLAP")
        ]
    );
    assert_eq!(
        default_search_paths(PluginFormat::Clap, Os::Linux, &home),
        [
            PathBuf::from("/home/me").join(".clap"),
            PathBuf::from("/usr/lib/clap")
        ]
    );
}

#[test]
fn vst3_defaults_are_the_folders_of_the_vst3_documentation() {
    // Checked on 2026-10-18 against the VST3 developer portal, "Plug-in Locations": Windows
    // %COMMONPROGRAMFILES%\VST3 and %LOCALAPPDATA%\Programs\Common\VST3; macOS
    // /Library/Audio/Plug-ins/VST3, ~/Library/Audio/Plug-ins/VST3 and the /Network one (the macOS
    // file system is case-insensitive, the folder is spelled `Plug-Ins` like the CLAP one);
    // Linux ~/.vst3 and the lib and lib64 folders under /usr and /usr/local.
    let home = env(&[("HOME", "/home/me")]);
    assert_eq!(
        default_search_paths(PluginFormat::Vst3, Os::Linux, &home),
        [
            PathBuf::from("/home/me").join(".vst3"),
            PathBuf::from("/usr/lib64/vst3"),
            PathBuf::from("/usr/lib/vst3"),
            PathBuf::from("/usr/local/lib64/vst3"),
            PathBuf::from("/usr/local/lib/vst3")
        ]
    );
    assert_eq!(
        default_search_paths(PluginFormat::Vst3, Os::MacOs, &home),
        [
            PathBuf::from("/Library/Audio/Plug-Ins/VST3"),
            PathBuf::from("/home/me").join("Library/Audio/Plug-Ins/VST3"),
            PathBuf::from("/Network/Library/Audio/Plug-Ins/VST3"),
        ]
    );
    let win = env(&[("COMMONPROGRAMFILES", "C:/cpf"), ("LOCALAPPDATA", "C:/lad")]);
    assert_eq!(
        default_search_paths(PluginFormat::Vst3, Os::Windows, &win),
        [
            PathBuf::from("C:/cpf").join("VST3"),
            PathBuf::from("C:/lad")
                .join("Programs")
                .join("Common")
                .join("VST3"),
        ]
    );
}

#[test]
fn the_clap_defaults_are_unchanged_by_the_vst3_folders() {
    let home = env(&[("HOME", "/home/me")]);
    assert_eq!(
        default_search_paths(PluginFormat::Clap, Os::MacOs, &home).len(),
        2
    );
}

#[test]
fn missing_environment_drops_only_the_dependent_paths() {
    let none = env(&[]);
    assert_eq!(
        default_search_paths(PluginFormat::Clap, Os::Linux, &none),
        [PathBuf::from("/usr/lib/clap")]
    );
    assert!(default_search_paths(PluginFormat::Clap, Os::Windows, &none).is_empty());
}

#[test]
fn clap_path_comes_first_extras_last_and_duplicates_collapse() {
    let e = env(&[
        ("HOME", "/h"),
        ("CLAP_PATH", "/opt/a:/usr/lib/clap::/opt/b"),
    ]);
    let roots = search_roots(
        PluginFormat::Clap,
        Os::Linux,
        &e,
        &[PathBuf::from("/extra"), PathBuf::from("/opt/a")],
    );
    assert_eq!(
        roots,
        [
            PathBuf::from("/opt/a"),
            PathBuf::from("/usr/lib/clap"),
            PathBuf::from("/opt/b"),
            PathBuf::from("/h").join(".clap"),
            PathBuf::from("/extra"),
        ]
    );
    let w = env(&[("CLAP_PATH", "D:/x;D:/y")]);
    let roots = search_roots(PluginFormat::Clap, Os::Windows, &w, &[]);
    assert_eq!(roots, [PathBuf::from("D:/x"), PathBuf::from("D:/y")]);
    // CLAP_PATH is not a VST3 variable.
    let roots = search_roots(PluginFormat::Vst3, Os::Linux, &e, &[]);
    assert!(!roots.contains(&PathBuf::from("/opt/a")));
}

#[test]
fn finds_files_and_bundles_recursively_sorted() {
    let d = temp("find");
    std::fs::create_dir_all(d.join("b/deep")).unwrap();
    std::fs::create_dir_all(d.join("Bundle.clap/Contents")).unwrap();
    std::fs::write(d.join("z.clap"), b"x").unwrap();
    std::fs::write(d.join("a.CLAP"), b"x").unwrap();
    std::fs::write(d.join("b/deep/m.clap"), b"x").unwrap();
    std::fs::write(d.join("b/readme.txt"), b"x").unwrap();
    std::fs::write(d.join("other.vst3"), b"x").unwrap();
    std::fs::write(d.join("Bundle.clap/Contents/inner.clap"), b"x").unwrap();
    let found = find_plugin_files(PluginFormat::Clap, &[d.clone(), d.join("nope")]);
    let rel: Vec<String> = found
        .iter()
        .map(|p| {
            p.strip_prefix(&d)
                .unwrap()
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        })
        .collect();
    assert_eq!(rel, ["Bundle.clap", "a.CLAP", "b/deep/m.clap", "z.clap"]);
    let v = find_plugin_files(PluginFormat::Vst3, std::slice::from_ref(&d));
    assert_eq!(v.len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn overlapping_roots_list_a_file_once() {
    let d = temp("overlap");
    std::fs::create_dir_all(d.join("sub")).unwrap();
    std::fs::write(d.join("sub/p.clap"), b"x").unwrap();
    let found = find_plugin_files(PluginFormat::Clap, &[d.clone(), d.join("sub")]);
    assert_eq!(found.len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn depth_is_cut() {
    let d = temp("depth");
    let mut deep = d.clone();
    for i in 0..=MAX_SEARCH_DEPTH + 1 {
        deep = deep.join(format!("d{i}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("far.clap"), b"x").unwrap();
    let ok = d.join("d0");
    std::fs::write(ok.join("near.clap"), b"x").unwrap();
    let found = find_plugin_files(PluginFormat::Clap, std::slice::from_ref(&d));
    assert_eq!(found.len(), 1);
    assert!(found[0].ends_with("near.clap"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn vst3_bundles_and_single_files_are_found_below_nested_folders_without_entering_bundles() {
    let d = temp("vst3-find");
    // A bundle (a folder named *.vst3 holding the real module), a single-file module and a
    // nested vendor folder with another bundle.
    let bundle = d.join("Gain.vst3");
    std::fs::create_dir_all(bundle.join("Contents/x86_64-win")).unwrap();
    std::fs::write(bundle.join("Contents/x86_64-win/Gain.vst3"), b"x").unwrap();
    std::fs::write(d.join("Single.vst3"), b"x").unwrap();
    std::fs::create_dir_all(d.join("Vendor/Suite/Deep.vst3")).unwrap();
    std::fs::write(d.join("Vendor/readme.txt"), b"x").unwrap();
    std::fs::write(d.join("Vendor/other.clap"), b"x").unwrap();
    let found = find_plugin_files(PluginFormat::Vst3, std::slice::from_ref(&d));
    let names: Vec<String> = found
        .iter()
        .map(|p| {
            p.strip_prefix(&d)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        names,
        ["Gain.vst3", "Single.vst3", "Vendor/Suite/Deep.vst3"],
        "bundles are not entered; clap files are not vst3"
    );
    let clap = find_plugin_files(PluginFormat::Clap, std::slice::from_ref(&d));
    assert_eq!(clap.len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_folder_of_only_vst2_plugins_is_reported_by_dll_or_by_its_name() {
    let d = temp("vst2");
    // .dll files and no .vst3 or .clap: VST2.
    let dlls = d.join("MyDlls");
    std::fs::create_dir_all(dlls.join("Vendor")).unwrap();
    std::fs::write(dlls.join("Vendor/synth.dll"), b"x").unwrap();
    // The standard VST2 folder names count even when they are empty.
    let std_names = [
        d.join("VSTPlugins"),
        d.join("Vstplugins"),
        d.join("VstPlugins"),
    ];
    for n in &std_names {
        std::fs::create_dir_all(n).unwrap();
    }
    // A folder with both a .dll (a plugin's helper library) and a .vst3 is a VST3 folder.
    let mixed = d.join("Mixed");
    std::fs::create_dir_all(&mixed).unwrap();
    std::fs::write(mixed.join("helper.dll"), b"x").unwrap();
    std::fs::write(mixed.join("real.vst3"), b"x").unwrap();
    // A folder with nothing relevant, and one that does not exist.
    let empty = d.join("Empty");
    std::fs::create_dir_all(&empty).unwrap();
    let missing = d.join("gone");
    let folders = vec![
        dlls.clone(),
        std_names[0].clone(),
        std_names[1].clone(),
        std_names[2].clone(),
        mixed,
        empty,
        missing,
    ];
    let hinted = vst2_only_folders(&folders);
    assert_eq!(
        hinted,
        [
            dlls,
            std_names[0].clone(),
            std_names[1].clone(),
            std_names[2].clone()
        ]
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn symlink_loops_terminate() {
    let d = temp("loop");
    std::fs::create_dir_all(d.join("a")).unwrap();
    std::fs::write(d.join("a/p.clap"), b"x").unwrap();
    std::os::unix::fs::symlink(&d, d.join("a/back")).unwrap();
    let found = find_plugin_files(PluginFormat::Clap, std::slice::from_ref(&d));
    use std::path::Path;
    assert!(!found.is_empty() && found.len() <= MAX_SEARCH_DEPTH + 1);
    assert!(Path::new(&found[0]).exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_scanner_count_follows_the_cores_between_two_and_eight() {
    use atomik_plugin_host::{MAX_PARALLEL_SCANS, parallel_scans};
    assert_eq!(parallel_scans(1), 2);
    assert_eq!(parallel_scans(4), 2);
    assert_eq!(parallel_scans(8), 4);
    assert_eq!(parallel_scans(12), 6);
    assert_eq!(parallel_scans(64), MAX_PARALLEL_SCANS);
    assert_eq!(MAX_PARALLEL_SCANS, 8);
}
