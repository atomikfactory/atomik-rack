//! Copying a file from outside the project into its folder: never over a file, never outside.
#![allow(missing_docs)]

mod common;

use atomik_audio_file::{ExternalCopy, MAX_EXTERNAL_BYTES, collect_external_file};
use common::*;
use std::path::Path;

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn a_file_is_copied_and_the_original_stays() {
    let dir = tmp_dir("ext-copy");
    let src = dir.join("outside").join("hall.wav");
    write(&src, b"impulse");
    let dest_dir = dir.join("proj").join("Files");
    let got = collect_external_file(&src, &dest_dir).unwrap();
    let dest = dest_dir.join("hall.wav");
    assert_eq!(
        got,
        ExternalCopy::Copied {
            dest: dest.clone(),
            bytes: 7
        }
    );
    assert_eq!(std::fs::read(&dest).unwrap(), b"impulse");
    assert_eq!(std::fs::read(&src).unwrap(), b"impulse");
    // No temporary file is left.
    assert_eq!(std::fs::read_dir(&dest_dir).unwrap().count(), 1);
}

#[test]
fn the_same_bytes_are_not_copied_twice_and_other_bytes_get_a_new_name() {
    let dir = tmp_dir("ext-names");
    let dest_dir = dir.join("Files");
    let a = dir.join("a").join("ir.wav");
    let b = dir.join("b").join("ir.wav");
    write(&a, b"one");
    write(&b, b"two");
    assert!(matches!(
        collect_external_file(&a, &dest_dir).unwrap(),
        ExternalCopy::Copied { .. }
    ));
    assert_eq!(
        collect_external_file(&a, &dest_dir).unwrap(),
        ExternalCopy::Present {
            dest: dest_dir.join("ir.wav")
        }
    );
    let got = collect_external_file(&b, &dest_dir).unwrap();
    assert_eq!(
        got,
        ExternalCopy::Copied {
            dest: dest_dir.join("ir 2.wav"),
            bytes: 3
        }
    );
    // Nothing that was there is overwritten.
    assert_eq!(std::fs::read(dest_dir.join("ir.wav")).unwrap(), b"one");
    assert_eq!(std::fs::read(dest_dir.join("ir 2.wav")).unwrap(), b"two");
}

#[test]
fn a_file_already_inside_the_folder_is_named_where_it_is() {
    let dir = tmp_dir("ext-inside");
    let dest_dir = dir.join("Files");
    let src = dest_dir.join("deep").join("x.fxp");
    write(&src, b"x");
    assert_eq!(
        collect_external_file(&src, &dest_dir).unwrap(),
        ExternalCopy::Present { dest: src }
    );
    assert_eq!(std::fs::read_dir(&dest_dir).unwrap().count(), 1);
}

#[test]
fn a_missing_file_a_folder_and_a_big_file_are_listed_not_copied() {
    let dir = tmp_dir("ext-refused");
    let dest_dir = dir.join("Files");
    assert_eq!(
        collect_external_file(&dir.join("none.wav"), &dest_dir).unwrap(),
        ExternalCopy::Missing
    );
    std::fs::create_dir_all(dir.join("folder.wav")).unwrap();
    assert_eq!(
        collect_external_file(&dir.join("folder.wav"), &dest_dir).unwrap(),
        ExternalCopy::Missing
    );
    let big = dir.join("big.wav");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(MAX_EXTERNAL_BYTES + 1)
        .unwrap();
    assert!(matches!(
        collect_external_file(&big, &dest_dir).unwrap(),
        ExternalCopy::Refused(_)
    ));
    assert!(!dest_dir.exists(), "nothing was written for any of them");
}
