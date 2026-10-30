//! Fault containment catches device panics with `catch_unwind`, so no build profile may turn
//! panics into aborts.

#[test]
fn no_profile_sets_panic_abort() {
    let manifest = include_str!("../../../Cargo.toml");
    for (n, line) in manifest.lines().enumerate() {
        let code = line.split('#').next().unwrap_or("");
        let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            !(compact.starts_with("panic=") && compact.contains("abort")),
            "Cargo.toml:{}: `panic = \"abort\"` would defeat the engine's fault containment",
            n + 1
        );
    }
}

#[test]
fn the_release_profile_says_unwind_explicitly() {
    let manifest = include_str!("../../../Cargo.toml");
    let release = manifest
        .split("[profile.release]")
        .nth(1)
        .and_then(|rest| rest.split("\n[").next())
        .expect("the manifest has a [profile.release] section");
    assert!(
        release
            .lines()
            .any(|l| l.replace(' ', "") == "panic=\"unwind\""),
        "[profile.release] must set panic = \"unwind\""
    );
}
