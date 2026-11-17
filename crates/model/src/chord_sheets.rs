//! The chord sheets a song keeps (`Song::sheets`): limits, checks and the list edits.
//!
//! A sheet is the plain data of [`atomik_tab::sheet::ChordSheet`] (chords anchored to the
//! characters of lyric lines, sections, comments, tab text, an optional link of a line to a bar of
//! the song). The song holds a short list of them; every edit replaces the whole list with one
//! [`crate::song::SongCommand::SetSheets`], so it is one undo step. The list is saved only while it
//! holds a sheet, as an optional field an older reader skips.

use atomik_tab::sheet::{
    ChordSheet, MAX_CHORDS_PER_LINE, MAX_LINE_CHARS, MAX_LINES, MAX_SHEET_BYTES, SheetBlock,
};

/// The most sheets a song keeps.
pub const MAX_SHEETS: usize = 64;
/// The longest title, artist, meta name or meta value, in characters.
pub const MAX_SHEET_FIELD: usize = 256;
/// The most `name: value` facts a sheet keeps.
pub const MAX_SHEET_META: usize = 64;
/// The most lines one block of tab text keeps.
pub const MAX_TAB_BLOCK_LINES: usize = 256;
/// The highest character index a chord may be anchored at (a chord-only line may run past its text).
pub const MAX_CHORD_INDEX: usize = 4 * MAX_LINE_CHARS;
/// The highest bar a line may be linked to.
pub const MAX_LINKED_BAR: u32 = 100_000;
/// The highest capo of a sheet.
pub const MAX_SHEET_CAPO: u8 = 24;

fn text_bad(text: &str, max: usize) -> bool {
    text.chars().count() > max || text.chars().any(|c| c.is_control())
}

/// Checks a song's sheets: counts, lengths and anchors within the limits. `Err` names the first
/// fault. Text that is not a valid chord cannot be in a sheet (chords are parsed), so the checks
/// are about size and order.
pub fn check_sheets(sheets: &[ChordSheet]) -> Result<(), String> {
    if sheets.len() > MAX_SHEETS {
        return Err(format!("a song keeps at most {MAX_SHEETS} chord sheets"));
    }
    for (s, sheet) in sheets.iter().enumerate() {
        let at = |m: String| format!("sheet {s}: {m}");
        if text_bad(&sheet.title, MAX_SHEET_FIELD) || text_bad(&sheet.artist, MAX_SHEET_FIELD) {
            return Err(at(format!(
                "the title and the artist are at most {MAX_SHEET_FIELD} characters"
            )));
        }
        if sheet.capo > MAX_SHEET_CAPO {
            return Err(at(format!("the capo is at most fret {MAX_SHEET_CAPO}")));
        }
        if sheet.meta.len() > MAX_SHEET_META
            || sheet
                .meta
                .iter()
                .any(|(k, v)| text_bad(k, MAX_SHEET_FIELD) || text_bad(v, MAX_SHEET_FIELD))
        {
            return Err(at(format!(
                "at most {MAX_SHEET_META} facts of {MAX_SHEET_FIELD} characters"
            )));
        }
        if sheet.blocks.len() > MAX_LINES {
            return Err(at(format!("at most {MAX_LINES} lines")));
        }
        let mut bytes = 0usize;
        for (b, block) in sheet.blocks.iter().enumerate() {
            let at = |m: &str| at(format!("block {b}: {m}"));
            match block {
                SheetBlock::Section(t) | SheetBlock::Comment(t) => {
                    bytes += t.len();
                    if text_bad(t, MAX_LINE_CHARS) {
                        return Err(at(
                            "a heading or comment is one line of at most 1000 characters",
                        ));
                    }
                }
                SheetBlock::Line { text, chords, bar } => {
                    bytes += text.len() + chords.len() * 8;
                    if text_bad(text, MAX_LINE_CHARS) {
                        return Err(at("a lyric line is at most 1000 characters"));
                    }
                    if chords.len() > MAX_CHORDS_PER_LINE {
                        return Err(at("too many chords on a line"));
                    }
                    if chords.windows(2).any(|w| w[0].0 > w[1].0)
                        || chords.iter().any(|(i, _)| *i > MAX_CHORD_INDEX)
                    {
                        return Err(at("chords must ascend and stay near the line"));
                    }
                    if bar.is_some_and(|n| n > MAX_LINKED_BAR) {
                        return Err(at("the linked bar is out of range"));
                    }
                }
                SheetBlock::Tab(t) => {
                    if t.lines.len() > MAX_TAB_BLOCK_LINES
                        || t.lines.iter().any(|l| text_bad(l, MAX_LINE_CHARS))
                    {
                        return Err(at("a tab block is at most 256 lines of 1000 characters"));
                    }
                    bytes += t.lines.iter().map(String::len).sum::<usize>();
                }
                SheetBlock::Blank => {}
            }
        }
        if bytes > MAX_SHEET_BYTES {
            return Err(at("the sheet is too large".to_owned()));
        }
    }
    Ok(())
}

/// The list with `sheet` added at the end; `None` beyond [`MAX_SHEETS`].
pub fn with_sheet(sheets: &[ChordSheet], sheet: ChordSheet) -> Option<Vec<ChordSheet>> {
    if sheets.len() >= MAX_SHEETS {
        return None;
    }
    let mut out = sheets.to_vec();
    out.push(sheet);
    Some(out)
}

/// The list with sheet `index` replaced; `None` for an index the list lacks.
pub fn with_sheet_replaced(
    sheets: &[ChordSheet],
    index: usize,
    sheet: ChordSheet,
) -> Option<Vec<ChordSheet>> {
    let mut out = sheets.to_vec();
    *out.get_mut(index)? = sheet;
    Some(out)
}

/// The list without sheet `index`; `None` for an index the list lacks.
pub fn without_sheet(sheets: &[ChordSheet], index: usize) -> Option<Vec<ChordSheet>> {
    if index >= sheets.len() {
        return None;
    }
    let mut out = sheets.to_vec();
    out.remove(index);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_tab::sheet::parse_sheet;

    fn sheet() -> ChordSheet {
        parse_sheet("[Verse]\n[Am]la la [F]la\n")
            .expect("a made-up sheet reads")
            .sheet
    }

    #[test]
    fn a_parsed_sheet_passes_and_edits_keep_the_order() {
        let a = sheet();
        assert_eq!(check_sheets(std::slice::from_ref(&a)), Ok(()));
        let mut b = a.clone();
        b.title = "Other".into();
        let list = with_sheet(std::slice::from_ref(&a), b.clone()).unwrap();
        assert_eq!(list.len(), 2);
        let list = with_sheet_replaced(&list, 0, b.clone()).unwrap();
        assert_eq!(list[0].title, "Other");
        let list = without_sheet(&list, 0).unwrap();
        assert_eq!(list, vec![b]);
        assert!(without_sheet(&list, 5).is_none());
        assert!(with_sheet_replaced(&list, 5, sheet()).is_none());
    }

    #[test]
    fn the_check_refuses_oversized_or_disordered_data() {
        let mut s = sheet();
        s.capo = 99;
        assert!(check_sheets(&[s]).unwrap_err().contains("capo"));
        let mut s = sheet();
        s.title = "x".repeat(MAX_SHEET_FIELD + 1);
        assert!(check_sheets(&[s]).is_err());
        let mut s = sheet();
        s.blocks
            .push(SheetBlock::lyric(&"y".repeat(MAX_LINE_CHARS + 1)));
        assert!(check_sheets(&[s]).unwrap_err().contains("lyric"));
        let mut s = sheet();
        if let Some(SheetBlock::Line { chords, .. }) = s
            .blocks
            .iter_mut()
            .find(|b| matches!(b, SheetBlock::Line { chords, .. } if chords.len() > 1))
        {
            chords.reverse();
        }
        assert!(check_sheets(&[s]).unwrap_err().contains("ascend"));
        let many: Vec<ChordSheet> = (0..=MAX_SHEETS).map(|_| sheet()).collect();
        assert!(check_sheets(&many).is_err());
        assert!(with_sheet(&many[..MAX_SHEETS], sheet()).is_none());
    }
}
