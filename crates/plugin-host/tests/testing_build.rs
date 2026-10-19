//! The test support builds a test plugin cdylib on demand under a lock, locates
//! it, and fails loudly (never skips) when it cannot.

use atomik_plugin_host::testing::{
    TestingError, build_cdylib, cdylib_file_name, clap_test_plugin, locate_cdylib,
};

#[test]
fn building_a_test_plugin_gives_the_path_of_its_library() {
    let path = build_cdylib("atomik-test-clap-leak").expect("the leak plugin builds");
    assert!(path.is_file(), "{}", path.display());
    assert_eq!(
        path.file_name().unwrap().to_string_lossy(),
        cdylib_file_name("atomik_test_clap_leak")
    );
    // Building again is cheap and gives the same file.
    assert_eq!(build_cdylib("atomik-test-clap-leak").unwrap(), path);
}

#[test]
fn concurrent_builds_of_the_same_plugin_both_succeed() {
    let handles: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(|| build_cdylib("atomik-test-clap-bad")))
        .collect();
    let paths: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().unwrap().expect("builds"))
        .collect();
    assert!(paths.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn an_unknown_package_is_a_loud_build_error_naming_it() {
    let err = build_cdylib("atomik-test-no-such-plugin").unwrap_err();
    match err {
        TestingError::BuildFailed(name, _) => assert_eq!(name, "atomik-test-no-such-plugin"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn locating_a_library_that_was_never_built_fails_loudly() {
    let err = locate_cdylib("atomik_test_never_built").unwrap_err();
    assert!(matches!(err, TestingError::NotBuilt(..)));
    assert!(err.to_string().contains("atomik_test_never_built"), "{err}");
}

#[test]
fn a_clap_test_plugin_is_built_and_copied_to_a_dot_clap_file() {
    let path = clap_test_plugin("atomik-test-clap-leak").expect("installed");
    assert!(path.is_file());
    assert_eq!(path.extension().unwrap(), "clap");
    assert!(
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("atomik_test_clap_leak"),
        "{}",
        path.display()
    );
}

#[test]
fn a_vst3_test_plugin_is_built_and_installed_as_a_bundle() {
    use atomik_plugin_host::paths::Os;
    use atomik_plugin_host::testing::{vst3_bundle_binary, vst3_test_plugin};
    let bundle = vst3_test_plugin("atomik-test-vst3-leak").expect("installed");
    assert!(bundle.is_dir(), "{}", bundle.display());
    assert_eq!(bundle.extension().unwrap(), "vst3");
    let binary = bundle.join(
        vst3_bundle_binary(
            Os::current(),
            std::env::consts::ARCH,
            "atomik_test_vst3_leak",
        )
        .expect("this system has a layout"),
    );
    assert!(binary.is_file(), "{}", binary.display());
    if cfg!(target_os = "macos") {
        assert!(bundle.join("Contents/Info.plist").is_file());
    }
    // A second call is a no-op that gives the same bundle.
    assert_eq!(vst3_test_plugin("atomik-test-vst3-leak").unwrap(), bundle);
}

#[test]
fn the_bundle_binary_follows_the_layout_of_each_operating_system() {
    use atomik_plugin_host::paths::Os;
    use atomik_plugin_host::testing::vst3_bundle_binary;
    use std::path::PathBuf;
    assert_eq!(
        vst3_bundle_binary(Os::Windows, "x86_64", "x"),
        Some(PathBuf::from("Contents/x86_64-win/x.vst3"))
    );
    assert_eq!(
        vst3_bundle_binary(Os::Windows, "aarch64", "x"),
        Some(PathBuf::from("Contents/arm64-win/x.vst3"))
    );
    assert_eq!(
        vst3_bundle_binary(Os::Linux, "x86_64", "x"),
        Some(PathBuf::from("Contents/x86_64-linux/x.so"))
    );
    assert_eq!(
        vst3_bundle_binary(Os::MacOs, "aarch64", "x"),
        Some(PathBuf::from("Contents/MacOS/x"))
    );
    assert_eq!(vst3_bundle_binary(Os::Linux, "riscv64", "x"), None);
}

#[test]
fn a_vst3_package_that_is_not_built_fails_loudly() {
    use atomik_plugin_host::testing::vst3_test_plugin;
    assert!(matches!(
        vst3_test_plugin("atomik-test-no-such-plugin"),
        Err(TestingError::BuildFailed(..))
    ));
}
