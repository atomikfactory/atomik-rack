//! `RackDoc::cable_fits`: the dry run the jack menu uses (the rules of `Command::Connect`,
//! nothing changed).

mod common;

use common::*;

#[test]
fn fits_follows_the_connect_rules_without_changing_anything() {
    let reg = registry();
    let d = chain(&reg);
    let before = d.doc().cables.clone();
    // A free input takes a compatible output.
    assert!(
        d.doc()
            .cable_fits(&reg, &cable("synth:out_l", "mixer:ch2_in_l"), None)
    );
    // An occupied plain input refuses, unless its cable is the one replaced.
    let taken = cable("delay:out_l", "mixer:ch1_in_l");
    let other = cable("synth:out_l", "mixer:ch1_in_l");
    assert!(!d.doc().cable_fits(&reg, &other, None));
    assert!(d.doc().cable_fits(&reg, &other, Some(&taken)));
    // A loop is refused.
    assert!(!d.doc().cable_fits(
        &reg,
        &cable("mixer:master_l", "delay:in_l"),
        Some(&cable("synth:out_l", "delay:in_l"))
    ));
    assert_eq!(d.doc().cables, before);
}
