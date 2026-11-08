//! Reading the version line of `ffmpeg -version`.

use std::fmt;

/// The oldest FFmpeg major version video accepts.
pub const MIN_MAJOR: u32 = 4;

/// The version an FFmpeg program states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegVersion {
    /// The version word as printed (`7.1.1-full_build`, `n6.0`, `N-113456-gabc`).
    pub text: String,
    /// Major and minor number; `None` for a development build, which states a revision instead.
    pub numbers: Option<(u32, u32)>,
}

impl FfmpegVersion {
    /// Whether video can use it: a release of [`MIN_MAJOR`] or later, or a development build
    /// (they are recent by nature, and the version cannot be told).
    pub fn is_supported(&self) -> bool {
        self.numbers.is_none_or(|(major, _)| major >= MIN_MAJOR)
    }
}

impl fmt::Display for FfmpegVersion {
    /// `FFmpeg 7.1`, or `FFmpeg (development build)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.numbers {
            Some((major, minor)) => write!(f, "FFmpeg {major}.{minor}"),
            None => write!(f, "FFmpeg (development build)"),
        }
    }
}

/// Reads the first `ffmpeg version ...` (or `ffprobe version ...`) line of `output`.
/// `None` when there is none: the program is not FFmpeg.
pub fn parse_version(output: &str) -> Option<FfmpegVersion> {
    let word = output.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        let name = words.next()?;
        if !matches!(name, "ffmpeg" | "ffprobe") || words.next()? != "version" {
            return None;
        }
        words.next()
    })?;
    Some(FfmpegVersion {
        text: word.to_owned(),
        numbers: numbers_of(word),
    })
}

/// `7.1.1-x` is `(7, 1)`, `n6.0` is `(6, 0)`, `5` is `(5, 0)`; a revision word has no numbers.
fn numbers_of(word: &str) -> Option<(u32, u32)> {
    let word = word
        .strip_prefix(['n'])
        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(word);
    let digits = word.chars().take_while(char::is_ascii_digit).count();
    // A revision word (`N-113456-gabc`, `git-2024-05-01`) starts with no digit; a date build
    // (`2024-05-01-git-abc`) starts with four, which is no FFmpeg release number.
    if digits == 0 || digits > 2 {
        return None;
    }
    let major: u32 = word[..digits].parse().ok()?;
    let rest = &word[digits..];
    match rest.strip_prefix('.') {
        Some(after) => {
            let minor: String = after.chars().take_while(char::is_ascii_digit).collect();
            Some((major, minor.parse().ok()?))
        }
        None if rest.is_empty() || rest.starts_with('-') => Some((major, 0)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(line: &str) -> Option<(u32, u32)> {
        parse_version(line).and_then(|v| v.numbers)
    }

    #[test]
    fn release_builds_state_their_numbers() {
        assert_eq!(
            numbers("ffmpeg version 7.1.1-full_build-www.example.org Copyright (c) 2000-2025"),
            Some((7, 1))
        );
        assert_eq!(numbers("ffmpeg version 6.0 Copyright (c)"), Some((6, 0)));
        assert_eq!(
            numbers("ffmpeg version 4.4.2-0ubuntu0.22.04.1 Copyright (c)"),
            Some((4, 4))
        );
        assert_eq!(numbers("ffmpeg version n6.1.2 Copyright"), Some((6, 1)));
        assert_eq!(numbers("ffprobe version 5 Copyright"), Some((5, 0)));
        assert_eq!(numbers("ffmpeg version 2.8.17 Copyright"), Some((2, 8)));
    }

    #[test]
    fn development_builds_have_no_numbers_and_are_accepted() {
        for line in [
            "ffmpeg version N-113456-gabcdef123 Copyright (c)",
            "ffmpeg version git-2024-05-01-abcdef Copyright (c)",
            "ffmpeg version 2024-05-01-git-abcdef-full_build Copyright (c)",
        ] {
            let v = parse_version(line).unwrap();
            assert!(v.is_supported(), "{line}");
            assert_eq!(v.to_string(), "FFmpeg (development build)", "{line}");
        }
    }

    #[test]
    fn the_minimum_is_four() {
        assert!(
            parse_version("ffmpeg version 4.0 x")
                .unwrap()
                .is_supported()
        );
        assert!(
            !parse_version("ffmpeg version 3.4.13 x")
                .unwrap()
                .is_supported()
        );
    }

    #[test]
    fn the_banner_may_follow_other_lines_and_other_programs_are_refused() {
        let text = "warning: something\nffmpeg version 7.0 Copyright (c)\nbuilt with gcc";
        assert_eq!(numbers(text), Some((7, 0)));
        assert_eq!(parse_version("hello world\nversion 7.0"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("ffmpeg version"), None);
        assert_eq!(parse_version("python version 3.12"), None);
    }

    #[test]
    fn the_version_reads_as_a_label() {
        let v = parse_version("ffmpeg version 7.1.1-full Copyright").unwrap();
        assert_eq!(v.to_string(), "FFmpeg 7.1");
        assert_eq!(v.text, "7.1.1-full");
    }
}
