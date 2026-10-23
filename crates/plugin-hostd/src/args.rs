//! The command line of the child.

use std::ffi::OsString;
use std::path::PathBuf;

/// Which plugin format the child hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A CLAP plugin file.
    Clap,
    /// A VST3 plugin file or bundle.
    Vst3,
    /// The built-in test plugins (no plugin file).
    Null,
}

/// The parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// The plugin format.
    pub format: Format,
    /// The plugin file (ignored for [`Format::Null`]).
    pub plugin_file: PathBuf,
    /// The plugin's id inside the file.
    pub plugin_id: String,
    /// The shared block file the parent created.
    pub block: PathBuf,
    /// The doorbell name the parent created.
    pub doorbell: String,
    /// The parent's process id.
    pub parent_pid: u32,
}

/// Environment variable that makes the child host the built-in `null` plugins whatever format
/// the command line names (`ATOMIK_PLUGIN_HOST_FORMAT=null`). The sandbox always passes the real
/// format, so this is how the end-to-end tests run the null plugins through it.
pub const FORMAT_OVERRIDE_ENV: &str = "ATOMIK_PLUGIN_HOST_FORMAT";

/// `args` with the format replaced by `null` when `value` (the override variable) says so.
pub fn with_format_override(args: Args, value: Option<&str>) -> Args {
    if value == Some("null") {
        Args {
            format: Format::Null,
            ..args
        }
    } else {
        args
    }
}

/// Parses `<format> <plugin-file> <plugin-id> --block <path> --doorbell <name> --parent-pid
/// <pid>` (the flags in any order). The error is a short sentence for stderr.
pub fn parse(args: &[OsString]) -> Result<Args, String> {
    let mut positional: Vec<&OsString> = Vec::new();
    let mut block = None;
    let mut doorbell = None;
    let mut parent_pid = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let flag = match arg.to_str() {
            Some(f @ ("--block" | "--doorbell" | "--parent-pid")) => f,
            _ => {
                positional.push(arg);
                continue;
            }
        };
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag {
            "--block" => block = Some(PathBuf::from(value)),
            "--doorbell" => doorbell = Some(text(value, flag)?),
            _ => {
                let t = text(value, flag)?;
                parent_pid = Some(
                    t.parse::<u32>()
                        .map_err(|_| format!("{flag} is not a process id: {t}"))?,
                );
            }
        }
    }
    let [format, plugin_file, plugin_id] = positional.as_slice() else {
        return Err(match positional.get(3) {
            Some(extra) => format!("unexpected argument {}", extra.to_string_lossy()),
            None => "expected <format> <plugin-file> <plugin-id>".to_string(),
        });
    };
    let format = match format.to_str() {
        Some("clap") => Format::Clap,
        Some("vst3") => Format::Vst3,
        Some("null") => Format::Null,
        _ => {
            return Err(format!(
                "unknown format {} (clap, vst3 or null)",
                format.to_string_lossy()
            ));
        }
    };
    Ok(Args {
        format,
        plugin_file: PathBuf::from(plugin_file),
        plugin_id: text(plugin_id, "<plugin-id>")?,
        block: block.ok_or("--block is missing")?,
        doorbell: doorbell.ok_or("--doorbell is missing")?,
        parent_pid: parent_pid.ok_or("--parent-pid is missing")?,
    })
}

fn text(value: &OsString, what: &str) -> Result<String, String> {
    value
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{what} is not valid text"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    const FULL: [&str; 9] = [
        "clap",
        "C:/x/a.clap",
        "atomik.test.gain",
        "--block",
        "/tmp/b",
        "--doorbell",
        "bell",
        "--parent-pid",
        "42",
    ];

    #[test]
    fn the_override_variable_turns_any_format_into_null_and_nothing_else_does() {
        let a = parse(&os(&FULL)).unwrap();
        assert_eq!(
            with_format_override(a.clone(), Some("null")).format,
            Format::Null
        );
        assert_eq!(with_format_override(a.clone(), None).format, Format::Clap);
        assert_eq!(
            with_format_override(a, Some("vst3")).format,
            Format::Clap,
            "only null is an override"
        );
    }

    #[test]
    fn a_full_command_line_is_parsed() {
        let a = parse(&os(&FULL)).unwrap();
        assert_eq!(a.format, Format::Clap);
        assert_eq!(a.plugin_file, PathBuf::from("C:/x/a.clap"));
        assert_eq!(a.plugin_id, "atomik.test.gain");
        assert_eq!(a.block, PathBuf::from("/tmp/b"));
        assert_eq!(a.doorbell, "bell");
        assert_eq!(a.parent_pid, 42);
    }

    #[test]
    fn the_three_formats_are_named() {
        for (name, format) in [
            ("clap", Format::Clap),
            ("vst3", Format::Vst3),
            ("null", Format::Null),
        ] {
            let mut v = FULL.to_vec();
            v[0] = name;
            assert_eq!(parse(&os(&v)).unwrap().format, format);
        }
    }

    #[test]
    fn the_flags_may_come_in_any_order() {
        let a = parse(&os(&[
            "null",
            "-",
            "null.gain",
            "--parent-pid",
            "7",
            "--doorbell",
            "d",
            "--block",
            "b",
        ]))
        .unwrap();
        assert_eq!((a.parent_pid, a.doorbell.as_str()), (7, "d"));
    }

    #[test]
    fn an_unknown_format_is_refused() {
        let mut v = FULL.to_vec();
        v[0] = "vst2";
        assert!(parse(&os(&v)).unwrap_err().contains("vst2"));
    }

    #[test]
    fn a_missing_flag_is_named() {
        for flag in ["--block", "--doorbell", "--parent-pid"] {
            let mut v = FULL.to_vec();
            let at = v.iter().position(|a| *a == flag).unwrap();
            v.drain(at..at + 2);
            assert!(parse(&os(&v)).unwrap_err().contains(flag), "{flag}");
        }
    }

    #[test]
    fn a_flag_without_a_value_is_refused() {
        let mut v = FULL.to_vec();
        v.truncate(8);
        assert!(parse(&os(&v)).unwrap_err().contains("--parent-pid"));
    }

    #[test]
    fn a_pid_that_is_not_a_number_is_refused() {
        let mut v = FULL.to_vec();
        v[8] = "abc";
        assert!(parse(&os(&v)).unwrap_err().contains("abc"));
    }

    #[test]
    fn too_few_or_stray_arguments_are_refused() {
        assert!(parse(&os(&[])).is_err());
        assert!(parse(&os(&["clap", "f"])).is_err());
        let mut v = FULL.to_vec();
        v.push("extra");
        assert!(parse(&os(&v)).unwrap_err().contains("extra"));
    }
}
