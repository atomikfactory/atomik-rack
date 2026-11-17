//! Fixtures for the chord sheet pane: songs holding made-up chord sheets.

use super::*;
use atomik_model::{Command, Document, RackDoc};
use atomik_tab::sheet::{ChordSheet, SheetBlock, parse_sheet};
use atomik_ui::kurbo::Rect;
use atomik_ui::{ChordSheetView, Theme, UiAction};

pub const SHEET_BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 520.0);

/// A made-up sheet in the chords-over-lyrics form.
pub const LANTERN: &str = "title: Lantern Road
artist: Nobody Special
key: G

[Verse 1]
G           D
The lantern swings along the road
Em          C
and every shadow knows my name

[Chorus]
C       G
Walk on, walk on
D           G
the morning isn't far
";

/// Another made-up sheet, in the bracket form.
pub const HARBOUR: &str = "{title: Harbour Lights}
{capo: 2}

[Verse]
[Am]Boats come [F]home at [C]night
[G]Lamps along the [Am]quay
";

pub fn sheet(text: &str) -> ChordSheet {
    parse_sheet(text).expect("a made-up sheet reads").sheet
}

/// A song with the given sheets.
pub fn sheet_doc(sheets: Vec<ChordSheet>) -> RackDoc {
    let mut doc = reference_doc();
    doc.song_mut().sheets = sheets;
    doc
}

/// A sheet of `n` lyric lines, every fourth line linked to a bar (starting at bar 1, one line
/// per bar when `link` is set).
pub fn long_sheet(n: usize, link: bool) -> ChordSheet {
    let mut s = ChordSheet {
        title: "Long Song".into(),
        ..ChordSheet::default()
    };
    for i in 0..n {
        let text = format!("line number {i} of the made-up song");
        let chord = if i % 2 == 0 { "Am" } else { "F" };
        let mut b = SheetBlock::lyric(&text);
        if let SheetBlock::Line { chords, bar, .. } = &mut b {
            chords.push((0, atomik_tab::ChordSymbol::parse(chord).expect("chord")));
            if link {
                *bar = Some(i as u32 + 1);
            }
        }
        s.blocks.push(b);
    }
    s
}

/// A view on `doc`, placed and synced.
pub fn sheet_view(doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> ChordSheetView {
    let mut v = ChordSheetView::new(Theme::dark());
    v.set_bounds(SHEET_BOUNDS);
    v.sync(doc, reg);
    v
}

/// Applies the commands among `actions` to `document`; returns how many were applied.
pub fn apply_sheet(
    document: &mut Document,
    reg: &atomik_device_sdk::Registry,
    actions: &[UiAction],
) -> usize {
    let mut n = 0;
    for a in actions {
        if let UiAction::Command(c) = a {
            let c: Command = c.clone();
            document
                .apply(c.clone(), reg)
                .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}"));
            n += 1;
        }
    }
    n
}
