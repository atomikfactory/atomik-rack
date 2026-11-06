//! The files a plugin state names: found in text states only, replaced whole, one undo step to
//! put the old state back.
#![allow(missing_docs)]

mod common;

use atomik_device_sdk::PluginStateBlob;
use atomik_model::{Command, Document, PluginCommand, empty_doc, rewrite_paths};
use common::plugin::*;
use common::*;

fn doc_with_state(state: &[u8]) -> Document {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add_plugin(0, &synth_ref(), &synth_layout(), None), &r)
        .unwrap();
    d.apply(
        Command::Plugin(PluginCommand::SetState {
            device: key("test-synth"),
            state: PluginStateBlob(state.to_vec()),
            merge: None,
        }),
        &r,
    )
    .unwrap();
    d
}

fn paths(d: &Document) -> Vec<String> {
    d.doc()
        .plugin_file_refs()
        .into_iter()
        .map(|r| r.path)
        .collect()
}

#[test]
fn a_text_state_names_its_files() {
    let d = doc_with_state(
        br#"<state><ir file="D:\Plugins\IR\Hall one.wav"/><preset>/home/name/presets/pad.fxp</preset>
        <plain>C:/Samples/kick.flac</plain></state>"#,
    );
    assert_eq!(
        paths(&d),
        [
            "D:\\Plugins\\IR\\Hall one.wav",
            "/home/name/presets/pad.fxp",
            "C:/Samples/kick.flac"
        ]
    );
}

#[test]
fn a_json_state_with_doubled_backslashes_gives_the_single_path() {
    let d = doc_with_state(br#"{"ir": "C:\\Plugins\\Users\\name\\ir.wav", "gain": 1.5}"#);
    let refs = d.doc().plugin_file_refs();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].path, "C:\\Plugins\\Users\\name\\ir.wav");
    assert_eq!(refs[0].raw, "C:\\\\Plugins\\\\Users\\\\name\\\\ir.wav");
    assert_eq!(refs[0].device, key("test-synth"));
}

#[test]
fn text_that_is_not_an_absolute_file_path_is_left_alone() {
    let d = doc_with_state(
        br#"<a href="relative/ir.wav"/><b>/tag</b><c>http://host/x.wav</c><d>C:\Plugins\folder</d>
        <e>/x.wav</e><f>/dir/noext</f>"#,
    );
    assert!(paths(&d).is_empty(), "{:?}", paths(&d));
}

#[test]
fn a_binary_state_is_never_read() {
    let mut bytes = vec![0xff, 0xfe, 0x00, 0x80];
    bytes.extend_from_slice(br#""C:\Plugins\IR\a.wav""#);
    let d = doc_with_state(&bytes);
    assert!(paths(&d).is_empty());
    let state = PluginStateBlob(bytes);
    assert!(
        rewrite_paths(
            &state,
            &[("C:\\Plugins\\IR\\a.wav".into(), "D:\\Plugins\\b.wav".into())]
        )
        .is_none()
    );
}

#[test]
fn rewriting_replaces_whole_paths_and_keeps_the_escaping() {
    let state = PluginStateBlob(
        br#"{"a": "C:\\Plugins\\IR\\a.wav", "b": "C:\\Plugins\\IR\\a.wav.bak", "c": "C:\\Plugins\\IR\\a.wav"}"#.to_vec(),
    );
    let changed = rewrite_paths(
        &state,
        &[(
            "C:\\\\Plugins\\\\IR\\\\a.wav".into(),
            "E:\\Plugins\\Proj\\Files\\a.wav".into(),
        )],
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(changed.0).unwrap(),
        r#"{"a": "E:\\Plugins\\Proj\\Files\\a.wav", "b": "C:\\Plugins\\IR\\a.wav.bak", "c": "E:\\Plugins\\Proj\\Files\\a.wav"}"#
    );
    // A path that is not in the state changes nothing.
    assert!(rewrite_paths(&state, &[("Z:\\Plugins\\none.wav".into(), "x".into())]).is_none());
}

#[test]
fn a_rewritten_state_is_one_undo_step_that_brings_the_old_one_back() {
    let r = plugin_registry();
    let old = br#"<ir file="D:\Plugins\IR\hall.wav"/>"#;
    let mut d = doc_with_state(old);
    let refs = d.doc().plugin_file_refs();
    let state = plugin_of(d.doc(), "test-synth").state.clone().unwrap();
    let new = rewrite_paths(
        &state,
        &[(
            refs[0].raw.clone(),
            "E:\\Plugins\\P\\Files\\hall.wav".into(),
        )],
    )
    .unwrap();
    d.apply(
        Command::Plugin(PluginCommand::SetState {
            device: key("test-synth"),
            state: new,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    assert_eq!(paths(&d), ["E:\\Plugins\\P\\Files\\hall.wav"]);
    d.undo().unwrap();
    assert_eq!(paths(&d), ["D:\\Plugins\\IR\\hall.wav"]);
}
