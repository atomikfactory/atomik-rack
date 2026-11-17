//! The tab printout: header, track blocks, systems of bars.
//!
//! Layout works in points with y counted down. One bar's width follows its content: the closest
//! pair of columns (notes that start together) decides how many points a tick gets, with a floor
//! so a bar with one chord is not a sliver. Bars are packed into systems greedily and each system
//! is stretched to the full width (the last one only when it is more than half full). Within a
//! bar a column sits at its tick's share of the bar, so a rest is white space and a triplet sits
//! where it belongs.
//!
//! A system, from the top: section markers, tempo, chord diagrams, chord names, the let ring and
//! palm mute spans, a strip for the marks above the notes (bend arrows, tapping, vibrato...), the
//! staff, the rhythm stems and beams, the lyrics.

use super::diagram::{diagram_height, draw_diagram, start_frets_of};
use super::draw::{Anchor, Ink};
use super::font::{Font, text_width};
use super::{Layout, PageSize, Pager, PdfError, wrap};
use crate::chord::ChordSymbol;
use crate::instrument::{Family, find_instrument};
use crate::score::{
    BendKind, HarmonicKind, PPQ, SlideKind, TabNote, TabScore, TabTrack, TechFlags, VibratoKind,
};
use crate::voicing::{Voicing, VoicingCache, VoicingOptions};
use std::collections::BTreeMap;

/// What a tab printout looks like.
#[derive(Debug, Clone, PartialEq)]
pub struct TabPdfOptions {
    /// The paper.
    pub page: PageSize,
    /// The margin on every side in points (clamped to 20..=100).
    pub margin: f32,
    /// A fixed number of bars on every line; `None` fits as many as the width allows.
    pub bars_per_line: Option<u32>,
    /// Chord diagrams: the legend of the chords a track uses and the diagrams the track pins above
    /// its bars. Chord names are printed either way.
    pub diagrams: bool,
}

impl Default for TabPdfOptions {
    fn default() -> Self {
        Self {
            page: PageSize::A4,
            margin: 42.0,
            bars_per_line: None,
            diagrams: true,
        }
    }
}

/// The distance between two tab lines.
const LG: f32 = 7.4;
/// The size of the fret numbers.
const FS: f32 = 7.2;
/// Width left of the first bar for the TAB letters.
const GUTTER: f32 = 18.0;
const LPAD: f32 = 6.0;
const RPAD: f32 = 5.0;
const SIG_W: f32 = 13.0;
const MIN_INNER: f32 = 30.0;
/// Width of an inline chord diagram and of one in the legend.
const DIA_W: f32 = 34.0;
const LEGEND_W: f32 = 36.0;
const LEGEND_MAX: usize = 24;

/// Lays a tab score out on pages (see the module docs).
pub fn layout_tab(score: &TabScore, opts: &TabPdfOptions) -> Result<Layout, PdfError> {
    let tracks: Vec<&TabTrack> = score
        .tracks
        .iter()
        .filter(|t| !t.is_percussion && !t.notes.is_empty() && !t.tuning.strings.is_empty())
        .collect();
    if tracks.is_empty() {
        return Err(PdfError::Empty("the score has no fretted notes"));
    }
    let mut warnings = Vec::new();
    let skipped = score
        .tracks
        .iter()
        .filter(|t| t.is_percussion && !t.notes.is_empty())
        .count();
    if skipped > 0 {
        warnings.push(format!(
            "{skipped} percussion track{} not printed",
            if skipped == 1 { " was" } else { "s were" }
        ));
    }
    let margin = if opts.margin.is_finite() {
        opts.margin.clamp(20.0, 100.0)
    } else {
        42.0
    };
    let mut pager = Pager::new(opts.page.points(), margin);
    header(&mut pager, score);
    let grid = score.bar_grid();
    let meters = bar_meters(score, grid.len());
    let mut cache = VoicingCache::new();
    for t in &tracks {
        if pager.full {
            break;
        }
        print_track(
            &mut pager,
            score,
            t,
            &grid,
            &meters,
            opts,
            &mut cache,
            tracks.len() > 1,
        );
    }
    let title = if score.title.trim().is_empty() {
        tracks[0].name.clone()
    } else {
        score.title.clone()
    };
    let author = if score.artist.trim().is_empty() {
        score.music_by.clone()
    } else {
        score.artist.clone()
    };
    Ok(pager.finish(&title, &author, warnings))
}

/// The meter of every bar.
fn bar_meters(score: &TabScore, bars: usize) -> Vec<(u8, u8)> {
    let mut ms = score.meter.clone();
    ms.sort_by_key(|m| m.0);
    let mut out = Vec::with_capacity(bars);
    let (mut num, mut den) = (4u8, 4u8);
    let mut mi = 0;
    for b in 0..bars {
        while mi < ms.len() && ms[mi].0 as usize <= b {
            num = ms[mi].1.max(1);
            den = ms[mi].2.max(1);
            mi += 1;
        }
        out.push((num, den));
    }
    out
}

fn header(p: &mut Pager, score: &TabScore) {
    let (l, w) = (p.left(), p.width());
    let mut any = false;
    if !score.title.trim().is_empty() {
        for line in wrap(Font::Bold, 20.0, &score.title, w) {
            p.text(
                l,
                p.y + 16.0,
                20.0,
                Font::Bold,
                Ink::Black,
                Anchor::Start,
                &line,
            );
            p.y += 24.0;
        }
        any = true;
    }
    if !score.subtitle.trim().is_empty() {
        p.text(
            l,
            p.y + 10.0,
            12.0,
            Font::Regular,
            Ink::Dark,
            Anchor::Start,
            &score.subtitle,
        );
        p.y += 15.0;
        any = true;
    }
    let by_line = [score.artist.trim(), score.album.trim()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" - ");
    if !by_line.is_empty() {
        p.text(
            l,
            p.y + 9.0,
            11.0,
            Font::Regular,
            Ink::Dark,
            Anchor::Start,
            &by_line,
        );
        p.y += 14.0;
        any = true;
    }
    let credits: Vec<String> = [
        ("Words by", &score.words_by),
        ("Music by", &score.music_by),
        ("Tab by", &score.tab_by),
    ]
    .iter()
    .filter(|(_, v)| !v.trim().is_empty())
    .map(|(k, v)| format!("{k} {}", v.trim()))
    .collect();
    if !credits.is_empty() {
        p.text(
            l,
            p.y + 7.0,
            8.0,
            Font::Regular,
            Ink::Mid,
            Anchor::Start,
            &credits.join("   \u{b7}   "),
        );
        p.y += 11.0;
        any = true;
    }
    if !score.notices.trim().is_empty() {
        for line in wrap(Font::Regular, 8.0, &score.notices, w)
            .into_iter()
            .take(4)
        {
            p.text(
                l,
                p.y + 7.0,
                8.0,
                Font::Regular,
                Ink::Mid,
                Anchor::Start,
                &line,
            );
            p.y += 10.0;
        }
        any = true;
    }
    if !score.copyright.trim().is_empty() {
        p.text(
            l,
            p.y + 7.0,
            7.5,
            Font::Regular,
            Ink::Mid,
            Anchor::Start,
            &score.copyright,
        );
        p.y += 10.0;
        any = true;
    }
    if any {
        p.y += 6.0;
    }
}

/// One column: the notes that start together.
struct Col {
    tick: i64,
    notes: Vec<usize>,
    len: i64,
}

struct Bar {
    tick: i64,
    len: i64,
    sig: Option<(u8, u8)>,
    cols: Vec<Col>,
    inner: f32,
}

impl Bar {
    fn natural(&self) -> f32 {
        LPAD + if self.sig.is_some() { SIG_W } else { 0.0 } + self.inner + RPAD
    }
}

struct Geom {
    bar: usize,
    x0: f32,
    w: f32,
    inner_x0: f32,
    inner_w: f32,
}

/// What the notes of one track need while it prints.
struct Ctx<'a> {
    track: &'a TabTrack,
    notes: Vec<&'a TabNote>,
    prev: Vec<Option<usize>>,
    next: Vec<Option<usize>>,
    n: usize,
    starts: Vec<u8>,
}

fn fret_text(n: &TabNote) -> String {
    let f = n.fret.to_string();
    if n.tech.has(TechFlags::DEAD) {
        "x".to_owned()
    } else if n.tech.harmonic.is_some() {
        format!("<{f}>")
    } else if n.tech.has(TechFlags::GHOST) || n.tie {
        format!("({f})")
    } else {
        f
    }
}

fn col_width(ctx: &Ctx, c: &Col) -> f32 {
    c.notes
        .iter()
        .map(|&i| text_width(Font::Bold, FS, &fret_text(ctx.notes[i])) + 2.5)
        .fold(7.0, f32::max)
}

#[allow(clippy::too_many_arguments, reason = "one track's printing context")]
fn print_track(
    p: &mut Pager,
    score: &TabScore,
    track: &TabTrack,
    grid: &[(i64, i64)],
    meters: &[(u8, u8)],
    opts: &TabPdfOptions,
    cache: &mut VoicingCache,
    several: bool,
) {
    let n = track.tuning.strings.len().min(12);
    let mut notes: Vec<&TabNote> = track
        .notes
        .iter()
        .filter(|x| usize::from(x.string) < n)
        .collect();
    notes.sort_by_key(|x| (x.tick, x.string));
    if notes.is_empty() {
        return;
    }
    let mut prev = vec![None; notes.len()];
    let mut next = vec![None; notes.len()];
    let mut last_on: Vec<Option<usize>> = vec![None; n];
    for (i, x) in notes.iter().enumerate() {
        let s = usize::from(x.string);
        if let Some(j) = last_on[s] {
            prev[i] = Some(j);
            next[j] = Some(i);
        }
        last_on[s] = Some(i);
    }
    let ctx = Ctx {
        track,
        notes,
        prev,
        next,
        n,
        starts: start_frets_of(&track.tuning, track.capo),
    };

    // Heading, tuning line and legend.
    let first_row = if opts.diagrams {
        legend_entries(track, cache)
            .first()
            .map_or(0.0, |_| diagram_height(LEGEND_W, true) + 14.0)
    } else {
        0.0
    };
    p.ensure(44.0 + first_row.max(60.0));
    heading(p, track, several);
    if opts.diagrams {
        legend(p, &ctx, cache);
    }

    // Bars: from the first up to the one with the last note.
    let bar_of = |tick: i64| -> usize {
        grid.partition_point(|b| b.0 <= tick)
            .saturating_sub(1)
            .min(grid.len().saturating_sub(1))
    };
    let last_bar = bar_of(ctx.notes.iter().map(|x| x.tick).max().unwrap_or(0));
    let mut bars: Vec<Bar> = (0..=last_bar)
        .map(|b| Bar {
            tick: grid[b].0,
            len: grid[b].1.max(1),
            sig: (b == 0 || meters[b] != meters[b - 1]).then_some(meters[b]),
            cols: Vec::new(),
            inner: MIN_INNER,
        })
        .collect();
    for (i, x) in ctx.notes.iter().enumerate() {
        let b = bar_of(x.tick);
        let bar = &mut bars[b];
        match bar.cols.last_mut() {
            Some(c) if c.tick == x.tick => {
                c.notes.push(i);
                c.len = c.len.max(x.len);
            }
            _ => bar.cols.push(Col {
                tick: x.tick,
                notes: vec![i],
                len: x.len,
            }),
        }
    }
    let avail = p.width() - GUTTER;
    for bar in &mut bars {
        let mut ppt = 0.0f32;
        for w in bar.cols.windows(2) {
            let need = col_width(&ctx, &w[0]) / 2.0 + col_width(&ctx, &w[1]) / 2.0 + 2.0;
            ppt = ppt.max(need / (w[1].tick - w[0].tick).max(1) as f32);
        }
        let cap = (avail - LPAD - RPAD - SIG_W).max(MIN_INNER);
        bar.inner = (bar.len as f32 * ppt).clamp(MIN_INNER, cap);
    }

    // Systems.
    let per_line = opts.bars_per_line.map(|n| n.clamp(1, 32) as usize);
    let mut systems: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut sum = 0.0f32;
    for (i, bar) in bars.iter().enumerate() {
        let w = bar.natural();
        if !cur.is_empty() && (sum + w > avail || per_line.is_some_and(|n| cur.len() >= n)) {
            systems.push(std::mem::take(&mut cur));
            sum = 0.0;
        }
        cur.push(i);
        sum += w;
    }
    if !cur.is_empty() {
        systems.push(cur);
    }
    let count = systems.len();
    for (si, sys) in systems.iter().enumerate() {
        if p.full {
            break;
        }
        let natural: f32 = sys.iter().map(|&i| bars[i].natural()).sum();
        let stretch = si + 1 < count || natural >= avail * 0.5;
        let factor = if stretch { avail / natural } else { 1.0 };
        let mut x = p.left() + GUTTER;
        let mut geoms = Vec::with_capacity(sys.len());
        for &bi in sys {
            let bar = &bars[bi];
            let w = bar.natural() * factor;
            let sig_w = if bar.sig.is_some() { SIG_W } else { 0.0 };
            geoms.push(Geom {
                bar: bi,
                x0: x,
                w,
                inner_x0: x + LPAD + sig_w,
                inner_w: (w - LPAD - sig_w - RPAD).max(4.0),
            });
            x += w;
        }
        let sp = SystemPrinter {
            score,
            ctx: &ctx,
            bars: &bars,
            geoms: &geoms,
            first: si == 0,
            last: si + 1 == count,
            numbers: sys.first().copied().unwrap_or(0) + 1,
            diagrams: opts.diagrams,
        };
        sp.print(p);
    }
}

fn heading(p: &mut Pager, track: &TabTrack, several: bool) {
    let l = p.left();
    let inst = find_instrument(&track.instrument_id).map(|i| i.name);
    let mut title = track.name.trim().to_owned();
    if title.is_empty() && several {
        title = inst.unwrap_or("Track").to_owned();
    }
    if !title.is_empty() {
        p.text(
            l,
            p.y + 11.0,
            12.0,
            Font::Bold,
            Ink::Black,
            Anchor::Start,
            &title,
        );
        p.y += 16.0;
    }
    let labels: Vec<&str> = track
        .tuning
        .strings
        .iter()
        .rev()
        .map(|s| s.label.as_str())
        .collect();
    let mut line = String::new();
    match inst {
        Some(name) => line.push_str(name),
        None => line.push_str(&format!("{} strings", track.tuning.strings.len())),
    }
    line.push_str(&format!(
        "   \u{b7}   Tuning, bottom line first: {}",
        labels.join(" ")
    ));
    if track.capo.fret > 0 {
        line.push_str(&format!("   \u{b7}   Capo {}", track.capo.fret));
        if track.capo.strings.is_some() {
            line.push_str(" (some strings)");
        }
    }
    if let Some(d) = track.tuning.strings.iter().find(|s| s.start_fret > 0) {
        line.push_str(&format!(
            "   \u{b7}   {} string starts at fret {}",
            d.label, d.start_fret
        ));
    }
    p.text(
        l,
        p.y + 8.0,
        8.0,
        Font::Regular,
        Ink::Dark,
        Anchor::Start,
        &line,
    );
    p.y += 14.0;
}

/// The chords of a track in order of first use, with the fingering to draw.
fn legend_entries(track: &TabTrack, cache: &mut VoicingCache) -> Vec<(String, Voicing)> {
    let mut seen: Vec<(String, Option<ChordSymbol>)> = Vec::new();
    let mut entries: Vec<(i64, String, Option<ChordSymbol>)> = Vec::new();
    for (t, c) in &track.chords {
        entries.push((*t, c.name(), Some(c.clone())));
    }
    for d in &track.diagrams {
        entries.push((d.tick, d.name.clone(), ChordSymbol::parse(&d.name).ok()));
    }
    entries.sort_by_key(|e| e.0);
    for (_, name, sym) in entries {
        if !seen.iter().any(|(n, _)| *n == name) {
            seen.push((name, sym));
        }
    }
    let bass = find_instrument(&track.instrument_id).is_some_and(|i| i.family == Family::Bass);
    let opts = if bass {
        VoicingOptions::bass()
    } else {
        VoicingOptions::default()
    };
    let mut out = Vec::new();
    for (name, sym) in seen.into_iter().take(LEGEND_MAX) {
        let pinned = track.diagrams.iter().find(|d| d.name == name);
        let voicing = match (pinned, &sym) {
            (Some(d), _) => Some(d.voicing.clone()),
            (None, Some(s)) => cache
                .get(&track.tuning, track.capo, s, &opts)
                .first()
                .cloned(),
            _ => None,
        };
        if let Some(v) = voicing
            && v.frets.len() == track.tuning.strings.len()
        {
            out.push((name, v));
        }
    }
    out
}

fn legend(p: &mut Pager, ctx: &Ctx, cache: &mut VoicingCache) {
    let entries = legend_entries(ctx.track, cache);
    if entries.is_empty() {
        return;
    }
    let gap = 8.0;
    let per_row = (((p.width() + gap) / (LEGEND_W + gap)).floor() as usize).max(1);
    let h = diagram_height(LEGEND_W, true);
    let l = p.left();
    for row in entries.chunks(per_row) {
        p.ensure(h + 8.0);
        let y = p.y;
        for (i, (name, v)) in row.iter().enumerate() {
            let x = l + i as f32 * (LEGEND_W + gap);

            draw_diagram(p.page(), (x, y), LEGEND_W, Some(name), v, &ctx.starts);
        }
        p.y += h + 8.0;
    }
    p.y += 2.0;
}

struct SystemPrinter<'a> {
    score: &'a TabScore,
    ctx: &'a Ctx<'a>,
    bars: &'a [Bar],
    geoms: &'a [Geom],
    first: bool,
    last: bool,
    /// The number of the first bar of the system (counted from 1).
    numbers: usize,
    diagrams: bool,
}

/// A text laid along a line that pushes the next one right when two would touch.
fn spread(items: &mut [(f32, String)], font: Font, size: f32, right: f32) {
    let mut min_x = f32::MIN;
    for (x, t) in items.iter_mut() {
        if *x < min_x {
            *x = min_x;
        }
        let w = text_width(font, size, t);
        min_x = *x + w + 3.0;
        if *x + w > right {
            *x = (right - w).max(0.0);
        }
    }
}

impl SystemPrinter<'_> {
    fn range(&self) -> (i64, i64) {
        let a = &self.bars[self.geoms[0].bar];
        let z = &self.bars[self.geoms[self.geoms.len() - 1].bar];
        (a.tick, z.tick + z.len)
    }

    fn x_of(&self, tick: i64) -> Option<f32> {
        self.geoms.iter().find_map(|g| {
            let b = &self.bars[g.bar];
            (tick >= b.tick && tick < b.tick + b.len)
                .then(|| g.inner_x0 + (tick - b.tick) as f32 / b.len as f32 * g.inner_w)
        })
    }

    fn print(&self, p: &mut Pager) {
        let (t0, t1) = self.range();
        let ctx = self.ctx;
        let n = ctx.n;
        let track = ctx.track;
        let right = self.geoms.last().map_or(p.right(), |g| g.x0 + g.w);

        // What sits above the staff.
        let markers: Vec<(f32, String)> = self
            .score
            .markers
            .iter()
            .filter(|(t, _)| *t >= t0 && *t < t1)
            .filter_map(|(t, name)| Some((self.x_of(*t)? - 3.0, name.clone())))
            .collect();
        let mut tempos: Vec<(f32, f64)> = self
            .score
            .tempo
            .iter()
            .filter(|(t, _)| *t >= t0 && *t < t1)
            .filter_map(|(t, bpm)| Some((self.x_of(*t)? - 3.0, *bpm)))
            .collect();
        if self.first && !tempos.iter().any(|(x, _)| *x <= self.geoms[0].inner_x0) {
            let effective = self
                .score
                .tempo
                .iter()
                .filter(|(t, _)| *t <= t0)
                .max_by_key(|(t, _)| *t);
            if let Some((_, bpm)) = effective {
                tempos.insert(0, (self.geoms[0].inner_x0 - 3.0, *bpm));
            }
        }
        // Diagrams the track pins here: drawn when there is room, else the name joins the chords.
        let pinned: Vec<(f32, usize)> = if self.diagrams {
            track
                .diagrams
                .iter()
                .enumerate()
                .filter(|(_, d)| d.tick >= t0 && d.tick < t1 && d.voicing.frets.len() == n)
                .filter_map(|(i, d)| Some((self.x_of(d.tick)? - 4.0, i)))
                .collect()
        } else {
            Vec::new()
        };
        let mut drawn: Vec<(f32, usize)> = Vec::new();
        let mut names: Vec<(f32, String)> = Vec::new();
        for (k, &(x, i)) in pinned.iter().enumerate() {
            let room = pinned.get(k + 1).map_or(right - x, |n| n.0 - x);
            if room >= DIA_W + 1.0 && x + DIA_W <= right + 4.0 {
                drawn.push((x, i));
            } else {
                names.push((x, track.diagrams[i].name.clone()));
            }
        }
        let tol = PPQ / 4;
        for (t, c) in track.chords.iter().filter(|(t, _)| *t >= t0 && *t < t1) {
            let name = c.name();
            let covered = drawn.iter().any(|&(_, i)| {
                let d = &track.diagrams[i];
                d.name == name && (d.tick - t).abs() <= tol
            });
            if !covered && let Some(x) = self.x_of(*t) {
                names.push((x - 3.0, name));
            }
        }
        names.sort_by(|a, b| a.0.total_cmp(&b.0));
        spread(&mut names, Font::Bold, 9.0, right);

        let ordered: Vec<&Col> = self
            .geoms
            .iter()
            .flat_map(|g| self.bars[g.bar].cols.iter())
            .collect();
        let any = |f: &dyn Fn(&TabNote) -> bool| {
            ordered
                .iter()
                .any(|c| c.notes.iter().any(|&i| f(ctx.notes[i])))
        };
        let has_pm = any(&|x| x.tech.has(TechFlags::PALM_MUTE));
        let has_lr = any(&|x| x.tech.has(TechFlags::LET_RING));
        let has_bend = any(&|x| x.tech.bend.is_some());
        let has_marks = any(&|x| {
            let t = &x.tech;
            t.vibrato.is_some()
                || t.harmonic.is_some()
                || t.tremolo_division > 0
                || t.whammy.is_some()
                || t.flags.0
                    & (TechFlags::TAP.0
                        | TechFlags::SLAP.0
                        | TechFlags::POP.0
                        | TechFlags::THUMB.0
                        | TechFlags::ACCENT.0
                        | TechFlags::HEAVY_ACCENT.0
                        | TechFlags::STACCATO.0)
                    != 0
        });
        let lyrics: Vec<(f32, String)> = track
            .lyrics
            .iter()
            .filter(|(t, _)| *t >= t0 && *t < t1)
            .filter_map(|(t, s)| Some((self.x_of(*t)? - 2.0, s.clone())))
            .collect();

        let marker_row = if markers.is_empty() { 0.0 } else { 11.0 };
        let tempo_row = if tempos.is_empty() { 0.0 } else { 11.0 };
        let dia_row = if drawn.is_empty() {
            0.0
        } else {
            diagram_height(DIA_W, true) + 3.0
        };
        let chord_row = if names.is_empty() { 0.0 } else { 11.0 };
        let lr_row = if has_lr { 9.0 } else { 0.0 };
        let pm_row = if has_pm { 9.0 } else { 0.0 };
        let marks_h = if has_bend {
            17.0
        } else if has_marks {
            11.0
        } else {
            5.0
        };
        let staff_h = (n - 1) as f32 * LG;
        let rhythm_h = 26.0;
        let lyric_row = if lyrics.is_empty() { 0.0 } else { 11.0 };
        let above = marker_row + tempo_row + dia_row + chord_row + lr_row + pm_row + marks_h;
        let total = above + staff_h + rhythm_h + lyric_row + 12.0;
        p.ensure(total);
        let y0 = p.y;
        let left = p.left();
        let mut y = y0;

        for (x, name) in &markers {
            p.text(
                *x,
                y + 8.5,
                9.0,
                Font::Bold,
                Ink::Black,
                Anchor::Start,
                name,
            );
        }
        y += marker_row;
        for (x, bpm) in &tempos {
            // A quarter note, then = and the tempo.
            let page = p.page();
            page.dot((*x + 2.0, y + 7.0), 1.9, Ink::Black);
            page.line((*x + 3.8, y + 7.0), (*x + 3.8, y - 0.5), 0.7, Ink::Black);
            p.text(
                *x + 7.5,
                y + 8.0,
                8.0,
                Font::Regular,
                Ink::Black,
                Anchor::Start,
                &format!("= {}", fmt_bpm(*bpm)),
            );
        }
        y += tempo_row;
        for &(x, i) in &drawn {
            let d = &track.diagrams[i];

            draw_diagram(
                p.page(),
                (x, y),
                DIA_W,
                Some(&d.name),
                &d.voicing,
                &ctx.starts,
            );
        }
        y += dia_row;
        for (x, name) in &names {
            p.text(
                *x,
                y + 8.5,
                9.0,
                Font::Bold,
                Ink::Black,
                Anchor::Start,
                name,
            );
        }
        y += chord_row;
        let lr_y = y;
        y += lr_row;
        let pm_y = y;
        y += pm_row;
        let marks_y = y;
        y += marks_h;
        let top = y;
        let bottom = top + staff_h;

        // Spans of let ring and palm mute.
        for (flag, label, row_y, on) in [
            (TechFlags::LET_RING, "let ring", lr_y, has_lr),
            (TechFlags::PALM_MUTE, "P.M.", pm_y, has_pm),
        ] {
            if !on {
                continue;
            }
            let col_on = |c: &Col| c.notes.iter().any(|&i| ctx.notes[i].tech.has(flag));
            let xs: Vec<(f32, bool)> = self
                .geoms
                .iter()
                .flat_map(|g| {
                    let b = &self.bars[g.bar];
                    b.cols.iter().map(move |c| {
                        (
                            g.inner_x0 + (c.tick - b.tick) as f32 / b.len as f32 * g.inner_w,
                            col_on(c),
                        )
                    })
                })
                .collect();
            let mut i = 0;
            while i < xs.len() {
                if !xs[i].1 {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j + 1 < xs.len() && xs[j + 1].1 {
                    j += 1;
                }
                let (xa, xz) = (xs[i].0 - 4.0, xs[j].0 + 6.0);
                p.text(
                    xa,
                    row_y + 7.0,
                    7.0,
                    Font::Bold,
                    Ink::Black,
                    Anchor::Start,
                    label,
                );
                let lw = text_width(Font::Bold, 7.0, label);
                if xz > xa + lw + 2.0 {
                    let page = p.page();
                    page.dashed(
                        (xa + lw + 2.0, row_y + 4.5),
                        (xz, row_y + 4.5),
                        0.6,
                        Ink::Dark,
                        2.5,
                    );
                    page.line((xz, row_y + 4.5), (xz, row_y + 8.0), 0.6, Ink::Dark);
                }
                i = j + 1;
            }
        }

        // The staff.
        {
            let page = p.page();
            for s in 0..n {
                let yy = top + s as f32 * LG;
                page.line((left, yy), (right, yy), 0.5, Ink::Black);
            }
            page.line((left, top), (left, bottom), 0.8, Ink::Black);
            for g in self.geoms {
                let xe = g.x0 + g.w;
                let last_of_track = self.last && g.bar == self.geoms[self.geoms.len() - 1].bar;
                if last_of_track {
                    page.line((xe - 2.2, top), (xe - 2.2, bottom), 0.6, Ink::Black);
                    page.line((xe - 0.5, top), (xe - 0.5, bottom), 1.6, Ink::Black);
                } else {
                    page.line((xe, top), (xe, bottom), 0.6, Ink::Black);
                }
            }
        }
        // The TAB letters.
        let cx = left + GUTTER / 2.0 - 1.0;
        if n >= 4 {
            let size = (staff_h / 4.2).clamp(6.0, 11.0);
            let mid = top + staff_h / 2.0;
            for (k, ch) in ["T", "A", "B"].iter().enumerate() {
                p.text(
                    cx,
                    mid + (k as f32 - 1.0) * size * 1.05 + size * 0.36,
                    size,
                    Font::Bold,
                    Ink::Black,
                    Anchor::Middle,
                    ch,
                );
            }
        } else {
            p.text(
                cx,
                top + staff_h / 2.0 + 2.2,
                6.5,
                Font::Bold,
                Ink::Black,
                Anchor::Middle,
                "TAB",
            );
        }
        // The bar number of the first bar, and the time signatures.
        p.text(
            left,
            top - marks_h + 1.5,
            6.5,
            Font::Regular,
            Ink::Mid,
            Anchor::Start,
            &self.numbers.to_string(),
        );
        for g in self.geoms {
            if let Some((num, den)) = self.bars[g.bar].sig {
                let size = (staff_h * 0.42).clamp(7.0, 13.0);
                let mid = top + staff_h / 2.0;
                let x = g.x0 + LPAD / 2.0 + SIG_W / 2.0;
                p.text(
                    x,
                    mid - 0.8,
                    size,
                    Font::Bold,
                    Ink::Black,
                    Anchor::Middle,
                    &num.to_string(),
                );
                p.text(
                    x,
                    mid + size * 0.85,
                    size,
                    Font::Bold,
                    Ink::Black,
                    Anchor::Middle,
                    &den.to_string(),
                );
            }
        }

        // Notes.
        let mut xs: BTreeMap<usize, f32> = BTreeMap::new();
        for g in self.geoms {
            let b = &self.bars[g.bar];
            for c in &b.cols {
                let x = g.inner_x0 + (c.tick - b.tick) as f32 / b.len as f32 * g.inner_w;
                for &i in &c.notes {
                    xs.insert(i, x);
                }
            }
        }
        let marks_base = marks_y + marks_h - 3.5;
        for g in self.geoms {
            let b = &self.bars[g.bar];
            for c in &b.cols {
                let x = xs[&c.notes[0]];
                for &i in &c.notes {
                    let note = ctx.notes[i];
                    let y = top + f32::from(note.string) * LG;
                    let text = fret_text(note);
                    let tw = text_width(Font::Bold, FS, &text) + 1.6;
                    p.page()
                        .fill_rect(x - tw / 2.0, y - LG * 0.46, tw, LG * 0.92, Ink::White);
                    p.text(
                        x,
                        y + FS * 0.36,
                        FS,
                        Font::Bold,
                        Ink::Black,
                        Anchor::Middle,
                        &text,
                    );
                    self.note_marks(p, i, x, y, top, marks_y + marks_h - 10.0, &xs);
                }
                self.column_marks(p, c, x, marks_base);
            }
        }

        // Rhythm under the staff.
        for g in self.geoms {
            self.rhythm(p, g, bottom + 5.5);
        }
        y = bottom + rhythm_h;
        let mut lyr = lyrics;
        spread(&mut lyr, Font::Regular, 8.5, right);
        for (x, t) in &lyr {
            p.text(
                *x,
                y + 8.0,
                8.5,
                Font::Regular,
                Ink::Black,
                Anchor::Start,
                t,
            );
        }
        p.y = y0 + total;
    }

    /// The marks that belong to one note: slurs, slides, bends, vibrato, grace note.
    #[allow(clippy::too_many_arguments, reason = "one note's drawing context")]
    fn note_marks(
        &self,
        p: &mut Pager,
        i: usize,
        x: f32,
        y: f32,
        top: f32,
        bend_top: f32,
        xs: &BTreeMap<usize, f32>,
    ) {
        let ctx = self.ctx;
        let note = ctx.notes[i];
        let tech = &note.tech;
        let prev = ctx.prev[i];
        let next = ctx.next[i];
        let up = y - LG * 0.62;
        for (flag, letter) in [(TechFlags::HAMMER_ON, "H"), (TechFlags::PULL_OFF, "P")] {
            if tech.has(flag) {
                let xf = prev.and_then(|j| xs.get(&j).copied());
                self.slur(p, xf, x, up, Some(letter));
            }
        }
        if note.tie {
            let xf = prev.and_then(|j| xs.get(&j).copied());
            self.slur(p, xf, x, y + LG * 0.7, None);
        }
        if let Some(s) = tech.slide {
            let page = p.page();
            match s {
                SlideKind::Legato | SlideKind::Shift => {
                    let target = next.map(|j| (xs.get(&j).copied(), ctx.notes[j].fret));
                    let rising = target.is_none_or(|(_, f)| f >= note.fret);
                    let (ya, yz) = if rising {
                        (y + 2.6, y - 2.6)
                    } else {
                        (y - 2.6, y + 2.6)
                    };
                    let end = match target {
                        Some((Some(xn), _)) if xn - x > 16.0 => xn - 5.5,
                        _ => x + 13.0,
                    };
                    page.line((x + 5.0, ya), (end, yz), 0.7, Ink::Black);
                }
                SlideKind::InFromBelow => {
                    page.line((x - 13.0, y + 4.0), (x - 5.0, y - 2.5), 0.7, Ink::Black)
                }
                SlideKind::InFromAbove => {
                    page.line((x - 13.0, y - 4.0), (x - 5.0, y + 2.5), 0.7, Ink::Black)
                }
                SlideKind::OutDown => {
                    page.line((x + 5.0, y - 2.5), (x + 13.0, y + 4.0), 0.7, Ink::Black)
                }
                SlideKind::OutUp => {
                    page.line((x + 5.0, y + 2.5), (x + 13.0, y - 4.0), 0.7, Ink::Black)
                }
            }
        }
        if let Some(g) = tech.grace {
            p.text(
                x - 8.5,
                y - 1.5,
                5.5,
                Font::Bold,
                Ink::Black,
                Anchor::Middle,
                &g.fret.to_string(),
            );
        }
        if let Some(b) = &tech.bend {
            self.bend(p, b.kind(), b.peak(), x, y, bend_top.min(top - 6.0));
        }
    }

    fn slur(&self, p: &mut Pager, from: Option<f32>, to: f32, y: f32, letter: Option<&str>) {
        let (a, z) = match from {
            Some(xf) => (xf + 4.5, to - 4.5),
            None => (to - 15.0, to - 4.5),
        };
        if z - a < 2.0 {
            return;
        }
        let page = p.page();
        page.curve((a, y), ((a + z) / 2.0, y - 6.0), (z, y), 0.6, Ink::Black);
        if let Some(l) = letter {
            p.text(
                (a + z) / 2.0,
                y - 5.5,
                6.0,
                Font::Bold,
                Ink::Black,
                Anchor::Middle,
                l,
            );
        }
    }

    fn bend(&self, p: &mut Pager, kind: BendKind, peak: i32, x: f32, y: f32, ty: f32) {
        let tx = x + 9.0;
        let page = p.page();
        let head_up = |page: &mut super::draw::Page, hx: f32| {
            page.triangle(
                [(hx, ty), (hx - 2.2, ty + 4.0), (hx + 2.2, ty + 4.0)],
                Ink::Black,
            );
        };
        let pre = matches!(kind, BendKind::PreBend | BendKind::PreBendRelease);
        if pre {
            page.line((x, y - 3.4), (x, ty + 3.0), 0.6, Ink::Black);
            head_up(page, x);
        } else {
            page.curve(
                (x + 4.0, y - 1.0),
                (tx, y - 1.0),
                (tx, ty + 3.0),
                0.6,
                Ink::Black,
            );
            head_up(page, tx);
        }
        if matches!(kind, BendKind::BendRelease | BendKind::PreBendRelease) {
            let rx = tx + 8.0;
            let from = if pre { x } else { tx };
            page.curve(
                (from, ty + 1.0),
                (rx, ty + 1.0),
                (rx, y - 6.0),
                0.6,
                Ink::Dark,
            );
            page.triangle(
                [(rx, y - 2.0), (rx - 2.2, y - 6.0), (rx + 2.2, y - 6.0)],
                Ink::Dark,
            );
        }
        let label = bend_label(peak);
        p.text(
            if pre { x } else { tx },
            ty - 2.0,
            6.5,
            Font::Bold,
            Ink::Black,
            Anchor::Middle,
            &label,
        );
    }

    /// The marks above a column that apply to all its notes: tapping, slapping, accents and so on,
    /// the harmonic kind, vibrato, tremolo picking and the whammy bar.
    fn column_marks(&self, p: &mut Pager, c: &Col, x: f32, base: f32) {
        let ctx = self.ctx;
        let mut labels: Vec<&str> = Vec::new();
        let push = |s: &'static str, on: bool, labels: &mut Vec<&str>| {
            if on && !labels.contains(&s) {
                labels.push(s);
            }
        };
        let mut vibrato: Option<VibratoKind> = None;
        let mut tremolo = false;
        for &i in &c.notes {
            let t = &ctx.notes[i].tech;
            push("T", t.has(TechFlags::TAP), &mut labels);
            push("S", t.has(TechFlags::SLAP), &mut labels);
            push("P", t.has(TechFlags::POP), &mut labels);
            push("Th", t.has(TechFlags::THUMB), &mut labels);
            push(">", t.has(TechFlags::ACCENT), &mut labels);
            push("^", t.has(TechFlags::HEAVY_ACCENT), &mut labels);
            push(".", t.has(TechFlags::STACCATO), &mut labels);
            push("w/bar", t.whammy.is_some(), &mut labels);
            if let Some(h) = t.harmonic {
                let l = match h.kind {
                    HarmonicKind::Natural => None,
                    HarmonicKind::Artificial => Some("A.H."),
                    HarmonicKind::Pinch => Some("P.H."),
                    HarmonicKind::Tapped => Some("T.H."),
                    HarmonicKind::Semi => Some("S.H."),
                };
                if let Some(l) = l {
                    push(l, true, &mut labels);
                }
            }
            if t.vibrato.is_some() {
                vibrato = vibrato.or(t.vibrato);
            }
            tremolo |= t.tremolo_division > 0;
        }
        let mut right = x;
        if !labels.is_empty() {
            let text = labels.join(" ");
            let w = text_width(Font::Bold, 6.5, &text);
            p.text(x, base, 6.5, Font::Bold, Ink::Black, Anchor::Middle, &text);
            right = x + w / 2.0;
        }
        if tremolo {
            let page = p.page();
            for k in 0..3 {
                let dy = k as f32 * 2.0;
                page.line(
                    (x - 2.5, base - 1.0 - dy + 1.5),
                    (x + 2.5, base - 1.0 - dy - 1.5),
                    0.6,
                    Ink::Black,
                );
            }
            right = right.max(x + 3.0);
        }
        if let Some(v) = vibrato {
            let amp = if v == VibratoKind::Wide { 2.4 } else { 1.4 };
            let x0 = right + 2.5;
            let page = p.page();
            let mut prev = (x0, base - 2.0);
            for k in 0..6 {
                let next = (
                    x0 + (k + 1) as f32 * 2.6,
                    base - 2.0 + if k % 2 == 0 { -amp } else { amp },
                );
                page.line(prev, next, 0.6, Ink::Black);
                prev = next;
            }
        }
    }

    /// Stems, flags, beams, dots and triplet digits of one bar.
    fn rhythm(&self, p: &mut Pager, g: &Geom, top: f32) {
        let bar = &self.bars[g.bar];
        let bottom = top + 13.5;
        let (num, den) = (self.meter_of(g).0, self.meter_of(g).1);
        let unit = 4 * PPQ / i64::from(den.max(1));
        let beat = if den >= 8 && num >= 6 && num % 3 == 0 {
            3 * unit
        } else if den >= 8 {
            PPQ
        } else {
            unit
        };
        struct Item {
            x: f32,
            v: Value,
            beat: i64,
        }
        let mut items: Vec<Item> = Vec::with_capacity(bar.cols.len());
        for (k, c) in bar.cols.iter().enumerate() {
            let end = bar.cols.get(k + 1).map_or(bar.tick + bar.len, |n| n.tick);
            let len = c.len.min(end - c.tick).max(1);
            items.push(Item {
                x: g.inner_x0 + (c.tick - bar.tick) as f32 / bar.len as f32 * g.inner_w,
                v: Value::classify(len),
                beat: (c.tick - bar.tick) / beat.max(1),
            });
        }
        let page = p.page();
        for it in &items {
            let x = (it.x * 100.0).round() / 100.0;
            if it.v.division == 1 {
                page.line((x - 2.5, top + 4.0), (x + 2.5, top + 4.0), 0.7, Ink::Black);
            } else {
                page.line((x, top), (x, bottom), 0.6, Ink::Black);
            }
            if it.v.dotted {
                page.dot((x + 3.0, bottom - 1.5), 0.9, Ink::Black);
            }
        }
        let mut i = 0;
        while i < items.len() {
            if items[i].v.flags() == 0 {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < items.len()
                && items[j + 1].v.flags() > 0
                && items[j + 1].beat == items[i].beat
            {
                j += 1;
            }
            for level in 1..=4u8 {
                let y = bottom - f32::from(level - 1) * 2.8;
                for k in i..=j {
                    if items[k].v.flags() < level {
                        continue;
                    }
                    let x = items[k].x;
                    let joined_right = k < j && items[k + 1].v.flags() >= level;
                    let joined_left = k > i && items[k - 1].v.flags() >= level;
                    if joined_right {
                        page.line((x, y), (items[k + 1].x, y), 1.5, Ink::Black);
                    } else if !joined_left {
                        let dir = if j > i && k == j { -1.0 } else { 1.0 };
                        page.line((x, y), (x + 4.5 * dir, y - 2.5), 1.1, Ink::Black);
                    } else {
                        page.line((x - 4.5, y), (x, y), 1.5, Ink::Black);
                    }
                }
            }
            i = j + 1;
        }
        for it in &items {
            if it.v.triplet {
                p.text(
                    it.x,
                    bottom + 7.0,
                    6.0,
                    Font::Regular,
                    Ink::Dark,
                    Anchor::Middle,
                    "3",
                );
            }
        }
    }

    fn meter_of(&self, g: &Geom) -> (u8, u8) {
        // The signature written in this bar, else the nearest one before it.
        (0..=g.bar)
            .rev()
            .find_map(|b| self.bars[b].sig)
            .unwrap_or((4, 4))
    }
}

/// A note value read from a length in ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Value {
    /// 1 whole, 2 half, 4 quarter, 8 eighth ... 64.
    pub division: u8,
    pub dotted: bool,
    pub triplet: bool,
}

impl Value {
    /// The note value nearest to `len` ticks.
    pub fn classify(len: i64) -> Value {
        let mut best = Value {
            division: 4,
            dotted: false,
            triplet: false,
        };
        let mut best_d = i64::MAX;
        for division in [1u8, 2, 4, 8, 16, 32, 64] {
            let base = 4 * PPQ / i64::from(division);
            for (dotted, triplet, ticks) in [
                (false, false, base),
                (true, false, base * 3 / 2),
                (false, true, base * 2 / 3),
            ] {
                let d = (len - ticks).abs() * 2 + i64::from(dotted || triplet);
                if d < best_d {
                    best_d = d;
                    best = Value {
                        division,
                        dotted,
                        triplet,
                    };
                }
            }
        }
        best
    }

    /// How many flags or beams the value has.
    pub fn flags(self) -> u8 {
        match self.division {
            8 => 1,
            16 => 2,
            32 => 3,
            64 => 4,
            _ => 0,
        }
    }
}

fn fmt_bpm(bpm: f64) -> String {
    if (bpm - bpm.round()).abs() < 0.005 {
        format!("{}", bpm.round() as i64)
    } else {
        format!("{bpm:.1}")
    }
}

/// The amount written over a bend arrow.
fn bend_label(cents: i32) -> String {
    match cents {
        c if c <= 0 => String::new(),
        50 => "1/4".into(),
        100 => "1/2".into(),
        150 => "3/4".into(),
        200 => "full".into(),
        250 => "1 1/4".into(),
        300 => "1 1/2".into(),
        400 => "2".into(),
        c => format!("{:.1}", f64::from(c) / 200.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_values_are_read_from_lengths() {
        let v = |len| Value::classify(len);
        assert_eq!(v(PPQ).division, 4);
        assert_eq!(v(PPQ / 2).division, 8);
        assert!(v(PPQ * 3 / 2).dotted);
        assert!(v(PPQ / 3).triplet && v(PPQ / 3).division == 8);
        assert!(v(PPQ * 2 / 3).triplet && v(PPQ * 2 / 3).division == 4);
        assert_eq!(v(4 * PPQ).division, 1);
        assert_eq!(v(PPQ / 4).flags(), 2);
    }

    #[test]
    fn bend_amounts_are_named() {
        assert_eq!(bend_label(200), "full");
        assert_eq!(bend_label(100), "1/2");
        assert_eq!(bend_label(500), "2.5");
    }
}
