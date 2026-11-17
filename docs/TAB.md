# Guitar tab: the user guide

Atomik Rack reads, edits, plays and prints tablature for fretted instruments, shows chords over lyrics, and has a few
tools to learn a part: slow practice, a loop, a count-in, a fretboard, a chord trainer, and a mode that listens to your
playing and tells you how close you were. Everything works on local files: nothing is searched for, downloaded or sent
anywhere.

This guide is for musicians. It says what to click and press, and what a file keeps and loses on the way in and out. The
command ids in the last section are for the command palette, key bindings and the QA tools.

- [The instruments](#the-instruments)
- [Opening a tab or a chord sheet](#opening-a-tab-or-a-chord-sheet)
- [Saving and printing](#saving-and-printing)
- [The Tab Editor](#the-tab-editor)
- [The Chord Sheet pane](#the-chord-sheet-pane)
- [Practice](#practice)
- [The fretboard](#the-fretboard)
- [The Chord Trainer](#the-chord-trainer)
- [The sound: Plucked Strings](#the-sound-plucked-strings)
- [The exercises](#the-exercises)
- [Troubleshooting](#troubleshooting)
- [Commands reference](#commands-reference)

## The instruments

A tab belongs to an instrument and a tuning. The instrument decides how many lines the tab has and which fret numbers
are possible. These 24 are built in:

| Family | Instruments |
|---|---|
| Guitar | 6 strings, 7 strings (low B), 8 strings (low F#), 12 strings (octave pairs) |
| Bass | 4, 5 and 6 strings |
| Ukulele | soprano, concert, tenor (each re-entrant gCEA or low G), baritone |
| Banjo | 5 strings (with the short drone string), 4 strings tenor, 4 strings plectrum |
| Mandolin family | mandolin, mandola, octave mandolin, mandocello (paired strings) |
| Others | Irish bouzouki, balalaika prima, resonator guitar and lap steel, cavaquinho, charango, renaissance lute |

Things that are worth knowing:

- **The first line of a tab is the highest-sounding string**, as printed tab is always read. In a re-entrant tuning (a
  ukulele with a high G, a charango) the lines are not in order of pitch, and Atomik Rack never assumes they are.
- **The banjo's short string** starts at the fifth fret of the neck: it is drawn as the bottom line, the lowest fret
  number it can show is 5 (or higher with a capo), and an open note on it is an open string.
- **Tunings.** Each instrument has presets. The six-string guitar has Standard, Drop D, DADGAD, Open G, Open D, Open E,
  half step down, full step down and Drop C; the 5-string banjo has Open G, Double C, Open D, Standard C and Modal G;
  the ukuleles have re-entrant and low G; the basses have Drop D, half step down or a high C where they make sense. You
  can also build a **custom tuning**: any number of strings from 1 to 12, any pitch for each.
- **Capo.** Fret numbers in a tab are counted from the capo, as in a printed tab ("fret 3 above a capo on 2"). The capo
  can cover all strings. A capo file that names only some strings is kept as it is but cannot be made in the dialog.

## Opening a tab or a chord sheet

### Tabs and scores

File > Import > **Import Tab or Score File...** opens one of these:

| Format | Extensions |
|---|---|
| Guitar Pro files | `.gp3` `.gp4` `.gp5` `.gpx` `.gp` |
| MusicXML | `.musicxml` `.mxl` (compressed) and `.xml` |
| ASCII tab (plain text) | `.txt` |

You can also drop the file on the window or double-click it in the file browser. **Import Tab or Score as Song...** also
takes over the tempo changes, the time signatures, the section markers, the key and the chord symbols of the file,
starting at the beginning of the song; the plain import adds the tracks at the playhead and leaves the song's tempo and
meter alone. Either way one undo step takes the import back.

Each track of the file becomes a track of the song. Its notes become a note clip that carries the tab (string, fret,
techniques, tuning, capo) and plays through a **Plucked Strings** device set up for the instrument (see
[the sound](#the-sound-plucked-strings)). A drum track plays through the ordinary synth for now.

**What is kept:** title and credits, tempo and tempo changes, time signatures, key, section markers, repeats and
alternative endings (written out in full, so a repeated bar plays each time), track names, tunings, capo, number of
frets, durations with dots and tuplets, rests, ties (joined into one long note), several voices, chord diagrams,
lyrics, and the techniques: hammer-on, pull-off, slides (six kinds), bends and releases, vibrato, harmonics (natural,
artificial, tapped, pinch), palm mute, let ring, dead and ghost notes, accents, staccato, tapping, slap and pop,
tremolo picking, grace notes, whammy bar, fade-in and strum direction.

**What is dropped** (the import message counts what it left out): the mixer, effects and amp settings of the file, page
layout, colours, beaming, fingering marks, ornaments and trills, beat text, jumps such as D.C., D.S. and coda, and any
second and further staves of a track. A note that does not fit on the instrument is kept and flagged, never lost.

Three things to know about accuracy:

- **Guitar Pro and MusicXML readers have been checked against test files made by this program, not against files from
  other programs.** Open a real file and compare tuning, repeats, tempo, bends and slides with what you expect; if
  something looks wrong, tell us which file format and version it was.
- **ASCII tab has no rhythm.** The reader spaces the notes of each bar evenly over its columns and assumes 4/4 at 100
  beats per minute unless told otherwise, so rhythm is approximate. Tuning is taken from the string labels when they are
  recognisable. A plain `b` is a full-step bend.
- **A very long score** is cut at the limits of the reader (20 000 bars, a million notes) with a message.

### Chord sheets

File > Import > **Import Chord Sheet...** reads a text file with chords over lyrics, or a ChordPro file with chords in
brackets (`.txt`, `.crd`, `.chopro`, `.chordpro`, `.cho`, `.pro`). A line counts as a chord line when most of its words
read as chords; the chord sits over the lyric letter below it. Directives such as `{title: }`, `{capo: }` and
`{start_of_chorus}` are understood, as are section headers such as `[Verse]`. The sheet is added to the song and opens
in the [Chord Sheet pane](#the-chord-sheet-pane).

Chord names are read in a wide grammar: sharps and flats (also the symbols), minor, major sevenths, diminished,
augmented, suspended, power chords, sixths, ninths to thirteenths, added notes, altered degrees such as `b9` or `#11`,
slash basses. The root must be a capital letter, so a lyric word like "a" is never taken for a chord.

## Saving and printing

File > Export has these entries for tab:

- **Export Tab as MusicXML...** (a name ending in `.mxl` writes the compressed form). It writes the tuning, the capo,
  strings and frets, techniques, tempo, meter, key, markers, chords and lyrics, so another program can read it back
  exactly as it was.
- **Export Tab as Text...** writes an ASCII tab: one block of lines per system, a column grid for the smallest note
  value of each bar, and a legend for the technique marks.
- **Export Tab as PDF...** writes printable pages: the title, the tab staves, the bar numbers, the chord diagrams a tab
  uses, with ties and the technique marks. The paper is A4 or Letter by the region of your system. Percussion tracks
  are skipped with a message.
- **Export Chord Sheet as Text...** and **Export Chord Sheet as PDF...** write the song's chord sheet. When the Chord
  Sheet pane is open they write what it shows, with its transposition, capo and instrument diagrams.

**Scope.** A tab export writes the selected note clip when there is one, otherwise the whole song (every note track).

**Limits.**

- A PDF uses the standard PDF fonts, so it holds **only Latin-1 letters** (Western European languages). Other
  characters are printed as `?` and the export says how many.
- There is no options dialog for the PDF yet: the number of bars per line, the margins and the text size are fixed, and
  rests are not drawn (as in the editor).
- A MusicXML export never writes repeat signs: a song is written out in full. Ties across a bar line are written as two
  tied notes. A text export does not draw ties or pre-bends.
- Guitar Pro files cannot be written.

## The Tab Editor

**View > Editors > Tab Editor** opens the editor on the selected note clip. **Switch Piano Roll and Tab** swaps the
two views of the same clip. Notes live in the clip as ordinary notes, so everything you do is one undo step and the
clip still plays and edits like any other.

A clip that has no tab yet shows the notes placed on the neck automatically (the fretter prefers low hand positions,
open strings and short stretches). The first edit writes those choices down.

The pane has the instrument name at the top left (click it to change the instrument, tuning or capo), the **technique
toolbar**, the practice bar, a bar ruler, the staff with one line per string, and the fretboard strip under it. Above
the staff are the tempo, the section markers and the chord names with their diagrams; under it the lyrics.

### Entering notes

1. Click a string and a position on the staff, or move the cursor with the arrow keys.
2. Type the fret number: `0` to `9`. For a two-digit fret type both digits quickly (within about half a second): `1`
   then `2` makes fret 12. Typing a note plays it, so you can hear what you write.
3. Choose the length with the note value keys, then `Left` and `Right` step by that length.

**The cursor does not advance after a note**: press `Right`. A rest is a place where you enter nothing (rests are not
drawn). The note value keys also change the length of the note under the cursor.

| Key | Meaning |
|---|---|
| `W` `H` `Q` `E` `S` `T` `X` | whole, half, quarter, eighth, sixteenth, thirty-second, sixty-fourth note |
| `.` | dotted |
| `Y` | triplet |
| `Left` `Right` | previous and next position |
| `Up` `Down` | a string up or down |
| `Ctrl+Left` `Ctrl+Right` | previous and next bar |
| `=` and `-` | zoom in and out |

### Techniques

Select notes (or leave the cursor on one) and press the key. A press adds the technique when any selected note lacks it
and removes it when all have it. Hammer-on and pull-off mark the note that is played by the hammer or pull. A slide is
marked on the note it leaves.

| Key | Technique | Key | Technique |
|---|---|---|---|
| `Shift+H` | hammer-on | `V` | vibrato |
| `Shift+P` | pull-off | `Shift+V` | wide vibrato |
| `Shift+L` | legato slide | `N` | natural harmonic |
| `Shift+S` | shift slide | `Shift+N` | artificial harmonic |
| `B` | bend | `M` | palm mute |
| `Shift+B` | bend and release | `R` | let ring |
| `Alt+B` | pre-bend | `D` | dead note |
| `A` | accent | `G` | ghost note |
| `Shift+T` | tap | `Shift+C` | clear all techniques |

The other slide kinds (in from below or above, out down or up), the bend amount (quarter tone to two tones, a chip in
the toolbar and `editor.tab_bend_amount`) and pre-bend and release are in the right-click menu and the command
palette.

Techniques are heard: bends, slides, vibrato, harmonics and the whammy bar change the pitch of the note; palm mute damps
it; a dead note is a short click; ghost notes are quieter and accents louder; staccato shortens and let ring lengthens.
Hammer-on, pull-off, tap, slap, pop, tremolo picking and grace notes are shown on the tab but do not change the sound.

### Selecting and editing

- **Select:** `Shift+Left` and `Shift+Right` take whole columns; `Shift+Up` and `Shift+Down` switch to a rectangle of
  cells; dragging selects a rectangle; `Ctrl+A` selects everything.
- **Copy, cut, paste, delete** work as everywhere, with strings, frets and techniques. The tab editor has its own
  clipboard: you cannot paste between the tab editor and the piano roll, and it does not use the system clipboard.
- **Change string** (`Alt+Up`, `Alt+Down`) moves the selected notes to the next string that can play the same pitch,
  and changes the fret number to match. If one note cannot move, none does.
- **Transpose** (`Ctrl+Up`, `Ctrl+Down`: a semitone; with `Shift` an octave) shifts the pitch and finds new frets.
- **Re-fret** (right-click): *Re-fret at Position...* lists neck positions (the window of frets starting at the one you
  pick); *to the Lowest Frets* and *Automatically* place the notes by the fretter again.
- **Instrument, Tuning and Capo** (click the name): choose the instrument and tuning from the catalogue, set custom
  pitches with the plus and minus buttons for every string (hold `Shift` for an octave), and the capo. Then choose what
  the notes do. *Keep pitches (re-fret)* leaves the music unchanged and finds new fret numbers for the new tuning; notes
  the new neck cannot play lose their fret and are listed. *Keep frets (pitches change)* keeps the numbers and the
  music follows the tuning, as when you retune a guitar and play the same tab.
- **Loop:** drag over the bar ruler to set the song loop to those bars; a click only moves the playhead.

Moving notes in the piano roll forgets their strings and techniques (they are placed again by the fretter). Edit in the
tab editor when you want to keep them. The clip library and the clipboard of the arrangement do not carry the tab.

## The Chord Sheet pane

**View > Editors > Chord Sheet** shows the song's chord sheets: the chord names over the lyrics, a strip of the chords
the sheet uses with their diagrams, and a toolbar. Click a chord name and its diagram opens for the instrument you
chose; the arrow keys cycle through other fingerings.

- **Transpose** by semitones, **Capo** (the chords shown change to the shapes you play with that capo: the diagrams are
  the shapes, not the sound), the **sharps or flats** preference, and the text size. *Apply to Sheet* writes what you
  see into the sheet; until then these are only views.
- **Instrument and tuning** for the diagrams: pick any instrument of the catalogue; the tuning chip cycles its presets.
- **Edit as text** opens the sheet as plain text. `Ctrl+Enter` or Apply reads it back as one undo step.
- **New**, **Rename**, **Delete** and the arrows move between sheets; the sheets are saved with the project.
- **Autoscroll.** The sheet scrolls at a speed of your choosing (Scroll or Pause and lines per minute). A sheet
  imported from a score that has lyrics follows the song instead, because its lines are linked to bars.

Sheets can be exported as text or PDF (see above). Linking lines to bars by hand and sending the chords to the chord
lane are not available yet.

## Practice

The **practice bar** sits under the toolbar of the Tab Editor and the Chord Sheet pane. Every control is also a command
of Transport > Practice, so it works from the menu and the command palette. None of the practice settings is saved
with the project; a new project starts at full speed.

- **Speed, 25 % to 150 %.** Slow down to learn a passage. The click, the count-in, the loop and every instrument run on
  the slowed clock and the pitch of the notes does not change. **Audio clips are muted while the speed is not 100 %**
  (including warped ones), because a clip cannot be played at a different rate here. A bounce or an export is always at
  full speed. Recording is refused at a speed other than 100 %.
- **Count-in.** Off, 1 or 2 bars of clicks before Play starts, even when the click is off.
- **Click.** The metronome.
- **Loop.** Loops the selected bars: in the tab editor the bars of the selected cells (or the bar under the cursor), in
  the chord sheet the bars of the lines linked to bars.
- **Speed trainer.** Start, step, passes per step and target. With the loop on, every N clean passes the speed goes up
  by the step until it reaches the target. Without Listen on, every pass counts as clean.

### Listen and grade

Click **Listen** and play the tab on your instrument, through an audio interface input. The practice bar names the input
("In 1"); the **Input** chip steps through the channels and stereo pairs. When a pass of the loop ends (about a third of
a second later) the notes are coloured: green hit, amber early or late, red wrong note, dim red missed. The bar
shows the
accuracy of the pass, your streak, and the history of the last passes. **Clear** forgets them.

- Each expected note or chord is compared with what you played within a window of 80 ms. A right note in the first half
  of the window is a hit, in the second half it is early or late. Accuracy counts a hit as 1, early or late as one half,
  and wrong, missed and extra notes as 0.
- A chord (a strum) is matched by its pitch classes, so any voicing of the chord counts.
- With the speed trainer on and Listen on, only passes at or above the accuracy threshold (90 % by default, 50 to 100
  in steps of 5) are clean.
- **Latency.** Your audio device delays what it hears and what it plays. The device's own delay is taken into account;
  to correct the rest, click **Latency** and play along to the click for 8 notes: the median lateness is added to a
  saved offset, so the verdicts centre on zero. The offset is kept between sessions.

**Limits.** The detector needs a clean single line: notes below about **55 Hz** (the lowest string of a five- or
six-string bass, a drop-tuned low string) may come out without a pitch. It accepts an **octave error** as a right note
(it compares pitch classes), so it will not tell a note from the same note an octave away. Two notes ringing at once
read as a chord. Results appear **per pass**, not note by note. Count-in beats are not graded. Recording and sampling
pause listening and it resumes afterwards. Results are not saved; they live for the session.

## The fretboard

Under the staff of the Tab Editor there is a **fretboard strip**: a horizontal neck for the current tuning, any
number of
strings, with the banjo's short string starting at its peg, inlays, and the capo. While the song plays, the notes that
sound are lit and the next group of notes is ghosted; when stopped it shows the notes at the cursor. A chord with a
diagram can be shown as rings with finger numbers.

View > Editors has the controls: **Fretboard** (show or hide the strip), **Detach Fretboard** (a window of its own that
you can put beside the song) and **Fretboard Options**: *Left-handed* (mirrors the neck), *Chord Voicing*, and the dot
text (*Note Names*, *Fret Numbers*, *Intervals*). The choices and the open window are remembered between sessions. In
a pane too short for the staff and the strip, the strip is left out. With Listen on, the dots are coloured by the
verdict.

## The Chord Trainer

**View > Editors > Chord Trainer** drills chord changes. Pick an instrument and tuning (a banjo, ukulele and mandolin
are as good as a guitar), a capo, and where the chords come from: the chords of a **key** (triads or sevenths), a
**list**
you type (*Edit List...*, wrong names are flagged), or the **song's** chords. Click a diagram to cycle its fingerings.

Three modes:

- **Timed:** a click at a tempo of 30 to 240 beats per minute, one chord every 1 to 16 beats, the *Now* and *Next*
  diagrams and the beat dots following. Nothing is checked.
- **Timed + Listen** (verified): the chord must be heard near its beat on the audio input.
- **Listen:** the drill waits until you play the chord, and then moves on. *Skip* gives one up.

The order is in sequence, random, or **One Pair** (two chords back and forth). The trainer counts your **changes per
minute** for each pair of chords and keeps the best, with the hit rate, in a table; the table is saved between sessions
(not in the project) and *Reset* clears it. A pair is scored only after at least 3 judged changes, and a best only
counts
at a hit rate of 75 % or more, so use One Pair to train a score. The click does not use the song transport, which stays
stopped. The pane cannot be detached, and its setup (instrument, key, tempo) is not remembered, only the scores.

## The sound: Plucked Strings

The **Plucked Strings** instrument device (`atomik.plucked`) is a string model: a plucked string with a body, a pick
position, a pick hardness, palm muting, stiffness and string pairs for the mandolin and 12-string. It answers to the
techniques above and bends smoothly. Its panel has the sections STRING, PLUCK, COURSE, BODY, OUTPUT and PLAY; the body
choice has None, Steel, Nylon, Electric, Bass, Upright, Ukulele, Banjo, Mandolin, Resonator and Harp. See
[DEVICES.md](DEVICES.md#plucked-strings-atomikplucked-instrument) for every parameter.

14 factory patches (category Plucked) ship with it: Steel Acoustic, Nylon Classical, Clean Electric, Muted Electric,
Electric Bass Finger, Electric Bass Pick, Upright Bass, Ukulele, Banjo, Mandolin, 12-String, Bouzouki, Dobro and
Harp Pluck.

**Presets on import.** A tab import loads the patch that suits each track's instrument: guitars of 6, 7 and 8 strings
Steel Acoustic; the 12-string 12-String; the three basses Electric Bass Finger; every ukulele and the cavaquinho
Ukulele;
every banjo Banjo; mandolin, mandola, octave mandolin, mandocello, balalaika and charango Mandolin; the bouzouki
Bouzouki; the resonator guitar Dobro; the lute Nylon Classical. Anything else gets Steel Acoustic. If the factory folder
is not found you still get a Plucked Strings device, with its default sound. Changing the instrument in the Tab Editor
changes the tab, not the sound: swap the patch yourself. Percussion tracks play on the plain synth until a drum sound
for them is made.

## The exercises

**Factory > Projects > Exercises** in the file browser has 11 small practice projects, each a tab with a click-friendly
tempo and a chord sheet that only counts the beats: Guitar C Major Scale, Guitar Minor Pentatonic Box, Guitar
Fingerpicking Arpeggios, Guitar Chord Changes G C D Em, Guitar Technique Sampler, Bass Root and Fifth Lines, Banjo
Rolls,
Ukulele Strum Patterns, Ukulele Chord Changes C Am F G, Mandolin Tremolo Study and Mandolin G Major Scale. All the
material is original. Open one, open the Tab Editor, set a loop and slow it down.

## Troubleshooting

- **The import says the file is not a tab or score.** Check the extension and the format. Guitar Pro 3, 4 and 5 files,
  `.gpx` and `.gp` files are read; damaged files and other formats are refused with a message. A plain `.xml` is read
  as MusicXML when it is one.
- **Frets look wrong, or some notes have no number.** The notes did not fit the instrument. Set the right instrument
  and tuning with the name at the top left (use *Keep pitches*), or re-fret the notes.
- **A repeated bar appears twice.** Repeats are written out when a file is read, so the song plays as a player would.
- **The rhythm of an ASCII tab is off.** ASCII tab carries no rhythm; the reader spaces the notes evenly. Fix the
  lengths with the note value keys.
- **A MusicXML or Guitar Pro file differs from what another program shows.** These readers are checked against files
  this program made. Please report the file kind and version.
- **The PDF shows question marks.** A PDF holds Latin-1 only. Use plain letters in the title and the chord names.
- **There is no sound from an imported tab.** Check that the track is not muted and that its Plucked Strings device is
  in the rack. A drum track plays through the plain synth.
- **Audio clips went quiet, or the playhead and the click run slowly.** The practice speed is not 100 %. Audio clips are
  muted off full speed. Set it back to 100 %; a bounce is always at full speed.
- **Listen shows "no input", or nothing is graded.** Set up an audio input in Options > Audio and MIDI Setup. Only a
  clip that is open in the Tab Editor is graded.
- **Every note is graded early or late.** Use **Latency** and play along to the click for 8 notes.
- **Low notes are missed.** Notes below about 55 Hz may not be detected. Play the part an octave up.
- **Recording is refused.** The practice speed must be 100 % to record.
- **Pasting from the piano roll into the tab editor does nothing.** The two have separate clipboards. Copy and paste
  inside the tab editor.
- **The Fretboard strip is gone.** The pane is too short. Make it taller, or use **Detach Fretboard**.
- **A chord shows no diagram.** The name is outside the grammar, or no fingering fits the neck. Check the spelling, the
  instrument and the capo.

## Commands reference

Every command can be run from the command palette and `session_command`, and bound to a key in Settings > Keyboard.
The listed keys are the defaults of both keymap profiles; macOS uses `Cmd` for `Ctrl` and `Opt` for `Alt`. A command
with an argument is written `id argument`. A test checks this table against the registry, and
[KEYBOARD.md](KEYBOARD.md) is the full keyboard reference.

### Files

| Command | Menu | Key |
|---|---|---|
| `file.import_tab` | File > Import > Import Tab or Score File... | - |
| `file.import_tab_song` | File > Import > Import Tab or Score as Song... | - |
| `file.import_sheet` | File > Import > Import Chord Sheet... | - |
| `file.export_tab_musicxml` | File > Export > Export Tab as MusicXML... | - |
| `file.export_tab_text` | File > Export > Export Tab as Text... | - |
| `file.export_tab_pdf` | File > Export > Export Tab as PDF... | - |
| `file.export_sheet_text` | File > Export > Export Chord Sheet as Text... | - |
| `file.export_sheet_pdf` | File > Export > Export Chord Sheet as PDF... | - |

### Panes

| Command | Menu | Key |
|---|---|---|
| `view.toggle_tab_editor` | View > Editors > Tab Editor | - |
| `edit.toggle_tab_or_piano_roll` | View > Editors > Switch Piano Roll and Tab | - |
| `view.toggle_chord_sheet` | View > Editors > Chord Sheet | - |
| `view.toggle_chord_trainer` | View > Editors > Chord Trainer | - |
| `view.toggle_fretboard` | View > Editors > Fretboard | - |
| `view.detach_fretboard` | View > Editors > Detach Fretboard | - |
| `view.fretboard_left_handed` | View > Editors > Fretboard Options > Left-handed | - |
| `view.fretboard_voicing` | View > Editors > Fretboard Options > Chord Voicing | - |
| `view.fretboard_labels notes` | View > Editors > Fretboard Options > Note Names | - |
| `view.fretboard_labels frets` | View > Editors > Fretboard Options > Fret Numbers | - |
| `view.fretboard_labels intervals` | View > Editors > Fretboard Options > Intervals | - |

### Tab Editor

Menus: the technique and editing commands are in the right-click menu of the staff and the toolbar.

| Command | What it does | Key |
|---|---|---|
| `editor.tab_cursor_left` | Cursor to the previous position | `Left` |
| `editor.tab_cursor_right` | Cursor to the next position | `Right` |
| `editor.tab_cursor_up` | Cursor to a higher string | `Up` |
| `editor.tab_cursor_down` | Cursor to a lower string | `Down` |
| `editor.tab_bar_prev` | Previous bar | `Ctrl+Left` |
| `editor.tab_bar_next` | Next bar | `Ctrl+Right` |
| `editor.tab_extend left` | Extend the selection left | `Shift+Left` |
| `editor.tab_extend right` | Extend the selection right | `Shift+Right` |
| `editor.tab_extend up` | Extend the selection up | `Shift+Up` |
| `editor.tab_extend down` | Extend the selection down | `Shift+Down` |
| `editor.tab_duration whole` | Whole note | `W` |
| `editor.tab_duration half` | Half note | `H` |
| `editor.tab_duration quarter` | Quarter note | `Q` |
| `editor.tab_duration eighth` | Eighth note | `E` |
| `editor.tab_duration sixteenth` | Sixteenth note | `S` |
| `editor.tab_duration thirty_second` | Thirty-second note | `T` |
| `editor.tab_duration sixty_fourth` | Sixty-fourth note | `X` |
| `editor.tab_dotted` | Dotted | `.` |
| `editor.tab_triplet` | Triplet | `Y` |
| `editor.tab_technique hammer_on` | Hammer-on | `Shift+H` |
| `editor.tab_technique pull_off` | Pull-off | `Shift+P` |
| `editor.tab_technique slide_legato` | Legato slide | `Shift+L` |
| `editor.tab_technique slide_shift` | Shift slide | `Shift+S` |
| `editor.tab_technique bend` | Bend | `B` |
| `editor.tab_technique bend_release` | Bend and release | `Shift+B` |
| `editor.tab_technique pre_bend` | Pre-bend | `Alt+B` |
| `editor.tab_technique vibrato` | Vibrato | `V` |
| `editor.tab_technique vibrato_wide` | Wide vibrato | `Shift+V` |
| `editor.tab_technique harmonic` | Natural harmonic | `N` |
| `editor.tab_technique harmonic_artificial` | Artificial harmonic | `Shift+N` |
| `editor.tab_technique palm_mute` | Palm mute | `M` |
| `editor.tab_technique let_ring` | Let ring | `R` |
| `editor.tab_technique dead` | Dead note | `D` |
| `editor.tab_technique ghost` | Ghost note | `G` |
| `editor.tab_technique accent` | Accent | `A` |
| `editor.tab_technique tap` | Tapping | `Shift+T` |
| `editor.tab_technique clear` | Clear all techniques | `Shift+C` |
| `editor.tab_bend_amount` | Amount of the next bends (quarter_tone to two_tones) | - |
| `editor.tab_string_up` | Move to the higher string | `Alt+Up` |
| `editor.tab_string_down` | Move to the lower string | `Alt+Down` |
| `editor.tab_transpose up` | Up a semitone | `Ctrl+Up` |
| `editor.tab_transpose down` | Down a semitone | `Ctrl+Down` |
| `editor.tab_transpose octave_up` | Up an octave | `Ctrl+Shift+Up` |
| `editor.tab_transpose octave_down` | Down an octave | `Ctrl+Shift+Down` |
| `editor.tab_refret` | Re-fret (choose, automatic, lowest) | - |
| `editor.tab_tuning` | Instrument, Tuning and Capo | - |
| `editor.tab_zoom_in` | Zoom in | `=` |
| `editor.tab_zoom_out` | Zoom out | `-` |

### Chord Sheet

| Command | What it does | Key |
|---|---|---|
| `editor.sheet_select` | Select the next or previous sheet | - |
| `editor.sheet_new` | New chord sheet | - |
| `editor.sheet_rename` | Rename the sheet | - |
| `editor.sheet_delete` | Delete the sheet | - |
| `editor.sheet_edit_text` | Edit the sheet as text | - |
| `editor.sheet_apply_transpose` | Write the shown transposition into the sheet | - |
| `editor.sheet_transpose` | Transpose the view (up, down, reset) | - |
| `editor.sheet_capo` | Capo of the view (up, down, off) | - |
| `editor.sheet_spelling` | Sharps or flats | - |
| `editor.sheet_font` | Text size | - |
| `editor.sheet_autoscroll` | Autoscroll (toggle, faster, slower, sync) | - |

### Practice (Transport > Practice)

| Command | What it does | Key |
|---|---|---|
| `practice.loop_selection` | Loop the selected bars | - |
| `practice.speed` | Speed in percent (25 to 150) | - |
| `practice.speed_up` | Faster by one step | - |
| `practice.speed_down` | Slower by one step | - |
| `practice.count_in` | Count-in: off, 1 or 2 bars | - |
| `practice.trainer` | Speed trainer on or off | - |
| `practice.trainer_start` | Trainer start speed | - |
| `practice.trainer_step` | Trainer step | - |
| `practice.trainer_after` | Clean passes per step | - |
| `practice.trainer_target` | Trainer target speed | - |
| `practice.listen` | Listen and grade on or off | - |
| `practice.listen_input` | Next listening input | - |
| `practice.listen_calibrate` | Calibrate the listening latency | - |
| `practice.listen_clear` | Clear the results | - |
| `practice.listen_offset` | Listening offset up or down | - |
| `practice.listen_threshold` | Accuracy needed for a clean pass | - |

### Chord Trainer

| Command | What it does | Key |
|---|---|---|
| `editor.trainer_start` | Start or stop the drill | - |
| `editor.trainer_skip` | Skip the chord (listen mode) | - |
| `editor.trainer_mode` | Mode: timed, verified, listen | - |
| `editor.trainer_order` | Order: sequential, random, pair | - |
| `editor.trainer_pair` | Pick chord A or B of a pair | - |
| `editor.trainer_bpm` | Tempo | - |
| `editor.trainer_beats` | Beats per chord | - |
| `editor.trainer_source` | Chords from a key, a list or the song | - |
| `editor.trainer_key` | Key (up, down, major, minor) | - |
| `editor.trainer_sevenths` | Triads or sevenths | - |
| `editor.trainer_edit_list` | Type a list of chords | - |
| `editor.trainer_instrument` | Instrument | - |
| `editor.trainer_tuning` | Next tuning | - |
| `editor.trainer_capo` | Capo | - |
| `editor.trainer_voicing` | Next or previous fingering | - |
| `editor.trainer_reset_scores` | Reset the best scores | - |
