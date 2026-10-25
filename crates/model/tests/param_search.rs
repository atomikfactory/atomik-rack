//! The parameter search (model half): what the index lists and where each param belongs, the
//! ranking and the filters, the bounded results, and the signature that tells a caller when to
//! rebuild.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, Document, MAX_RESULTS, ParamIndex, SearchTarget, new_project_doc,
};
use common::*;

/// The starter with two synths (`lead`, `pad`), a VCA over the first channel, a container with a
/// delay inside, and a delay effect that is not on any strip.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("lead")),
            add(usize::MAX, SYNTH, Some("pad")),
            Command::Rename {
                key: key("lead"),
                name: "Lead".into(),
            },
            Command::Rename {
                key: key("pad"),
                name: "Pad".into(),
            },
            Command::Console(ConsoleCommand::CreateVca {
                name: "Drums".into(),
                members: vec![key("channel")],
            }),
            add(usize::MAX, CONTAINER, Some("box")),
            add_inner("box", 0, DELAY, "echo"),
            add(usize::MAX, DELAY, Some("loose")),
        ],
    );
    (d, r)
}

fn target(device: &str, param: &str) -> SearchTarget {
    SearchTarget::Param {
        device: key(device),
        param: param.into(),
    }
}

fn first<'a>(index: &'a ParamIndex, query: &str) -> &'a atomik_model::ParamEntry {
    index.search(query, 10).into_iter().next().expect(query)
}

#[test]
fn the_index_lists_every_param_of_every_device_and_the_vca_levels() {
    let (d, r) = project();
    let index = ParamIndex::build(d.doc(), &r);
    let expected: usize = d
        .doc()
        .devices
        .iter()
        .map(|dev| {
            r.get(&dev.type_id)
                .unwrap()
                .descriptor
                .params
                .iter()
                .filter(|p| !p.key.starts_with("internal."))
                .count()
        })
        .sum::<usize>()
        + r.get(DELAY).unwrap().descriptor.params.len() // the delay inside the container
        + 1; // the VCA
    assert_eq!(index.len(), expected);
    assert!(!index.is_empty());
    // Device params are there by key, the inside of a container by path.
    assert!(
        index
            .entries()
            .iter()
            .any(|e| e.target == target("lead", "filter.cutoff"))
    );
    assert!(
        index
            .entries()
            .iter()
            .any(|e| e.target == target("box/echo", "mix"))
    );
    // Internal params of the strips are not.
    assert!(index.entries().iter().all(|e| match &e.target {
        SearchTarget::Param { param, .. } => !param.starts_with("internal."),
        SearchTarget::Vca { .. } => true,
    }));
}

#[test]
fn every_param_reads_track_device_param() {
    let (d, r) = project();
    let index = ParamIndex::build(d.doc(), &r);
    let find = |t: SearchTarget| index.entries().iter().find(|e| e.target == t).unwrap();
    let cutoff = find(target("lead", "filter.cutoff"));
    assert_eq!(
        (cutoff.track.as_str(), cutoff.device.as_str()),
        ("Lead", "Lead")
    );
    assert_eq!(cutoff.unit, "Hz");
    // The strip of the first instrument is its track; the master is the master.
    let fader = find(target("channel", "fader"));
    assert_eq!(fader.track, "Lead");
    assert_eq!(fader.device, "Channel");
    assert_eq!(find(target("channel-2", "fader")).track, "Pad");
    assert_eq!(find(target("master", "fader")).track, "Master");
    // A container's inside reads as the container, a loose device as the rack.
    assert_eq!(find(target("box/echo", "mix")).track, "Container");
    assert_eq!(find(target("loose", "mix")).track, "Rack");
    let vca = find(SearchTarget::Vca { id: 1 });
    assert_eq!(
        (vca.track.as_str(), vca.device.as_str(), vca.param.as_str()),
        ("Mixer", "Drums", "Level")
    );
}

#[test]
fn a_query_finds_a_param_by_device_track_param_and_unit() {
    let (d, r) = project();
    let index = ParamIndex::build(d.doc(), &r);
    // By param name: the cutoff of a synth, the best match being the shortest name.
    let top = first(&index, "cutoff");
    assert!(top.param.to_lowercase().contains("cutoff"));
    // The device or track narrows it to one synth.
    assert_eq!(
        first(&index, "lead cutoff").target,
        target("lead", "filter.cutoff")
    );
    assert_eq!(
        first(&index, "pad cutoff").target,
        target("pad", "filter.cutoff")
    );
    // The path form of the result row works as a query.
    assert_eq!(
        first(&index, "pad > cutoff").target,
        target("pad", "filter.cutoff")
    );
    // By key.
    assert_eq!(
        first(&index, "filter.cutoff lead").target,
        target("lead", "filter.cutoff")
    );
    // By unit: the rows that come first are Hz params.
    let hz = index.search("lead hz", 3);
    assert_eq!(hz.len(), 3);
    assert!(hz.iter().all(|e| e.track == "Lead" && e.unit == "Hz"));
    // The VCA by its name.
    assert_eq!(first(&index, "drums").target, SearchTarget::Vca { id: 1 });
    // Case does not matter.
    assert_eq!(
        first(&index, "LEAD CUTOFF").target,
        target("lead", "filter.cutoff")
    );
}

#[test]
fn every_word_must_match_and_a_fuzzy_word_still_finds_the_param() {
    let (d, r) = project();
    let index = ParamIndex::build(d.doc(), &r);
    assert!(index.search("lead nosuchparam", 10).is_empty());
    assert!(index.search("zzzzzz", 10).is_empty());
    // The letters in order, close together.
    let fuzzy = index.search("lead cutof", 3);
    assert_eq!(fuzzy[0].target, target("lead", "filter.cutoff"));
    let loose = index.search("lead fltr", 50);
    assert!(loose.iter().any(|e| e.key.starts_with("filter.")));
}

#[test]
fn a_prefix_beats_a_substring_and_the_order_is_stable() {
    let (d, r) = project();
    let index = ParamIndex::build(d.doc(), &r);
    // `pad` starts the track and device of the pad's params, and is inside `Padding`-like words
    // nowhere else: the pad's rows come before anything that only contains the letters.
    let rows = index.search("pad", 20);
    assert!(
        rows.iter()
            .take(10)
            .all(|e| e.device == "Pad" || e.track == "Pad")
    );
    // The same query gives the same rows.
    let again = index.search("pad", 20);
    assert_eq!(rows, again);
}

#[test]
fn results_are_bounded() {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    for n in 0..40 {
        let k = format!("s{n}");
        d.apply(add(usize::MAX, SYNTH, Some(&k)), &r).unwrap();
    }
    let index = ParamIndex::build(d.doc(), &r);
    assert!(index.len() > MAX_RESULTS);
    assert_eq!(index.search("", 50).len(), 50);
    assert_eq!(index.search("a", usize::MAX).len(), MAX_RESULTS);
    assert_eq!(index.search("", usize::MAX).len(), MAX_RESULTS);
    assert_eq!(index.search("cutoff", 7).len(), 7);
    // An empty query lists the first rows in rack order.
    let all = index.search("", 3);
    assert_eq!(all[0], &index.entries()[0]);
    assert_eq!(all[2], &index.entries()[2]);
}

#[test]
fn the_signature_follows_the_structure_and_not_the_values() {
    let (mut d, r) = project();
    let before = ParamIndex::signature_of(d.doc());
    assert_eq!(ParamIndex::build(d.doc(), &r).signature(), before);
    d.apply(
        Command::SetParam {
            key: key("lead"),
            param: "filter.cutoff".into(),
            value: 900.0,
            merge: None,
        },
        &r,
    )
    .unwrap();
    assert_eq!(ParamIndex::signature_of(d.doc()), before);
    d.apply(
        Command::Rename {
            key: key("lead"),
            name: "Bass".into(),
        },
        &r,
    )
    .unwrap();
    let renamed = ParamIndex::signature_of(d.doc());
    assert_ne!(renamed, before);
    d.apply(add(usize::MAX, DELAY, Some("more")), &r).unwrap();
    assert_ne!(ParamIndex::signature_of(d.doc()), renamed);
    // The renamed track shows in the rebuilt index.
    let index = ParamIndex::build(d.doc(), &r);
    assert_eq!(
        first(&index, "bass cutoff").target,
        target("lead", "filter.cutoff")
    );
}

#[test]
fn macros_of_a_bank_are_searchable_like_any_param() {
    let (mut d, r) = project();
    d.apply(
        Command::Macro(atomik_model::MacroCommand::Assign {
            bank: None,
            index: 0,
            device: key("lead"),
            param: "filter.cutoff".into(),
            min: None,
            max: None,
            invert: false,
        }),
        &r,
    )
    .unwrap();
    let index = ParamIndex::build(d.doc(), &r);
    let top = first(&index, "macro bank macro 1");
    assert_eq!(top.device, "Macro Bank");
    assert_eq!(top.track, "Rack");
    assert!(matches!(&top.target, SearchTarget::Param { param, .. } if param == "macro1"));
}
