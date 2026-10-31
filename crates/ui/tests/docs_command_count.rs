//! The number of commands `docs/ARCHITECTURE.md` quotes is the number the registry holds.

use atomik_ui::keymap::Registry;

#[test]
fn the_architecture_document_quotes_the_command_count() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/ARCHITECTURE.md");
    let doc = std::fs::read_to_string(path).unwrap();
    let marker = "`Registry::bundled()` lists every command (";
    let at = doc
        .find(marker)
        .expect("the architecture document says how many commands the registry lists");
    let quoted: String = doc[at + marker.len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    assert_eq!(
        quoted,
        Registry::bundled().commands().len().to_string(),
        "docs/ARCHITECTURE.md quotes the number of commands: update it when a command is added"
    );
}
