//! Hardware (gated by `ATOMIK_HW_TESTS=1`): lists MIDI inputs and reports the Bome virtual
//! port if present. Opens nothing and never sends MIDI.

use atomik_midi_io::list_inputs;

#[test]
fn hw_list_inputs_reports_bome_virtual_port_if_present() {
    if std::env::var("ATOMIK_HW_TESTS").as_deref() != Ok("1") {
        return;
    }
    let inputs = list_inputs().expect("MIDI backend");
    for name in &inputs {
        println!("MIDI input: {name}");
    }
    match inputs.iter().find(|n| n.to_lowercase().contains("bome")) {
        Some(bome) => println!("Bome virtual MIDI port present: {bome}"),
        None => println!("no Bome virtual MIDI port found"),
    }
}
