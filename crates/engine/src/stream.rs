//! Disk streaming for large audio assets: a non-RT [`Streamer`] thread keeps one
//! lock-free ring of audio *chunks* per streamed audio track ("lane"), and the audio thread
//! only ever pops from it.
//!
//! # Design
//!
//! * **Lane.** One lane per audio track that has a region on a [`crate::SongAsset::Stream`]
//!   asset. It is created by `EngineHandle::submit_song` (non-RT): a preallocated SPSC ring of
//!   [`Chunk`]s (256 frames of stereo each; capacity 2 s of audio, so at least 1 s of
//!   read-ahead is always kept ahead) whose consumer travels inside the snapshot to the audio
//!   thread and whose producer goes to the streamer together with the track's immutable region
//!   list (an `Arc`, shared with the snapshot).
//! * **Playback order.** The ring carries the track's *streamed regions' finished output in
//!   playback order* (clip gain, fades, gain envelope and crossfades applied, two overlapping
//!   regions mixed, a reversed region read backwards; see `audio.rs`): chunk `k` holds the frames `seq0..seq0 + len` counted from the start of the current
//!   *epoch*, where the streamer follows exactly the player's loop rule (at song frame `le` it
//!   continues at `ls`; a start beyond `le` plays on linearly). Frames outside streamed regions
//!   (gaps, memory regions) are zeros. Because chunks are tagged by playback index instead of
//!   song frame, loop wraps need no refill: the data after the wrap is already queued.
//! * **Epochs and refill requests.** Whenever the audio thread's position stops being the
//!   continuation of the lane's last batch (the first batch, a locate, a snapshot swap) or the
//!   loop range changes, it bumps the lane's epoch and sends a [`StreamRequest`] (epoch, song
//!   frame, loop) through a wait-free SPSC queue to the streamer; a full queue is retried at
//!   every batch. The streamer restarts the lane from that frame; chunks of older epochs that
//!   are still in the ring are discarded by the audio thread. This also covers a stopped
//!   transport: the lane prefetches at the stopped position so playing starts without an
//!   underrun. Request cost on the audio thread: a few comparisons and one queue push.
//! * **RT consumption.** Every playing batch consumes exactly the segment's frames from the
//!   lane (used for streamed-region frames, discarded elsewhere), so the ring stays in step.
//!   If the data is not there (the streamer is behind) the missing frames are silence and the
//!   underrun counter (`EngineHandle::stream_underruns`) goes up by one per segment-region read
//!   that lacked data; nothing ever blocks. Late chunks are skipped to the current playback
//!   index.
//! * **Determinism.** Offline bounces use `Memory` assets only. With a streamer that keeps up,
//!   the audio equals the `Memory` output bit for bit (same samples, same gain code, the same
//!   assign-then-add order where two regions overlap).
//! * **Following a clock.** While the player follows an external clock the lane is read by
//!   `audio_follow` instead: its epoch starts at the own-timeline frame the song is at
//!   ([`StreamLaneRt::follow_seek`], no loop: a wrap is a new epoch), a streamed region plays
//!   from the ring at the clip's own rate once the clock reaches its start, and between regions
//!   the cursor skips ahead to the song's place ([`StreamLaneRt::skip_to`]). The read-ahead is
//!   what the ring holds: a region is consumed at one ring frame per frame whatever the clock's
//!   tempo, and the gaps, which the streamer fills with zeros at many times real time, are
//!   skipped at the clock's pace, so the same two seconds of ring cover any followed tempo.
//! * **Streamer.** One thread, spawned lazily by the first `submit_song` with a streamed
//!   asset, joined when the handle is dropped. It polls requests and lane commands and fills
//!   each active lane round-robin, a few chunks at a time, so a slow source delays other lanes
//!   but never the audio thread. `StreamSource::read` runs only here. While a round fills
//!   something or serves a request the next one follows at once; a round with nothing to do
//!   waits [`SHORT_PARK`] (1 ms), and only after [`QUIET_ROUNDS`] such rounds in a row (about a
//!   tenth of a second in which nothing was filled, served or applied, which a playing
//!   transport never reaches: a chunk leaves a ring every few milliseconds) does it wait
//!   [`IDLE_PARK`] between rounds. A new set of lanes and every transport command the app sends
//!   the engine wake it at once and restart that quiet time ([`StreamControl::wake`]), so a
//!   locate, a start or a loop change made from the interface is seen as fast as ever. The one
//!   thing that still waits up to [`IDLE_PARK`] is a refill request that only the audio thread
//!   makes after a long quiet (a followed external clock that relocates): the audio thread
//!   cannot wake the streamer, it takes no syscall.

use crate::audio::{LaneData, SongAsset};
use atomik_core::BATCH;
use rtrb::{Consumer, Producer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

/// Frames per ring chunk.
pub(crate) const CHUNK: usize = 256;
/// Seconds of audio a lane ring holds.
pub(crate) const RING_SECONDS: f64 = 2.0;
/// Seconds of read-ahead after which a lane counts as primed.
pub(crate) const PRIME_SECONDS: f64 = 1.0;
/// Chunks filled per lane per round.
const ROUND_CHUNKS: usize = 4;
/// How long the streamer sleeps between rounds once it has been quiet for [`QUIET_ROUNDS`]
/// rounds. The audio thread only pushes to a wait-free queue, so this bounds how late a refill
/// request that nothing woke the thread for is seen.
pub(crate) const IDLE_PARK: Duration = Duration::from_millis(10);
/// How long the streamer sleeps after a round with nothing to do, until it has been quiet long.
const SHORT_PARK: Duration = Duration::from_millis(1);
/// Rounds in a row with nothing filled, served or applied after which the streamer rests long.
pub(crate) const QUIET_ROUNDS: u32 = 100;
/// Capacity of the request queue (audio thread -> streamer).
pub(crate) const REQUEST_CAPACITY: usize = 256;

/// One ring entry: `len <= CHUNK` stereo frames of playback index `seq0..seq0 + len`.
#[derive(Clone, Copy)]
pub(crate) struct Chunk {
    epoch: u32,
    seq0: u64,
    len: u16,
    data: [[f32; CHUNK]; 2],
}

/// A refill request: restart the lane `lane` at song frame `start` with loop `lp`, as epoch
/// `epoch`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamRequest {
    lane: u64,
    epoch: u32,
    start: u64,
    lp: Option<(u64, u64)>,
}

/// Counters shared by the audio thread, the streamer and the handle.
#[derive(Debug)]
pub(crate) struct StreamStats {
    /// Segment reads that lacked data (audio thread increments).
    pub(crate) underruns: AtomicU64,
    /// Interim-warp segment reads that wanted a real-time stretcher and found none free, so
    /// they played the plain Hermite read (audio thread increments).
    pub(crate) stretch_fallbacks: AtomicU64,
    /// Every lane has `PRIME_SECONDS` buffered (streamer updates).
    pub(crate) primed: AtomicBool,
    /// Refill requests the audio thread has queued / the streamer has handled (equal when no
    /// request is in flight).
    pub(crate) requested: AtomicU64,
    pub(crate) served: AtomicU64,
    /// Lane-set commands the handle has sent / the streamer has applied.
    pub(crate) commands_sent: AtomicU64,
    pub(crate) commands_applied: AtomicU64,
    /// Rounds the streamer thread has run (a gauge of how busy it is).
    pub(crate) rounds: AtomicU64,
    /// Wake-ups from the non-real-time side (lane sets, transport commands).
    pub(crate) wakes: AtomicU64,
}

impl StreamStats {
    /// True when no request or command is in flight and every lane has its read-ahead.
    pub(crate) fn is_primed(&self) -> bool {
        let served = self.served.load(Ordering::Acquire);
        let applied = self.commands_applied.load(Ordering::Acquire);
        self.primed.load(Ordering::Acquire)
            && served == self.requested.load(Ordering::Acquire)
            && applied == self.commands_sent.load(Ordering::Acquire)
    }
}

impl Default for StreamStats {
    fn default() -> Self {
        Self {
            underruns: AtomicU64::new(0),
            stretch_fallbacks: AtomicU64::new(0),
            primed: AtomicBool::new(true),
            requested: AtomicU64::new(0),
            served: AtomicU64::new(0),
            commands_sent: AtomicU64::new(0),
            commands_applied: AtomicU64::new(0),
            rounds: AtomicU64::new(0),
            wakes: AtomicU64::new(0),
        }
    }
}

/// The audio-thread half of a lane (lives in the snapshot's `AudioRt`).
pub(crate) struct StreamLaneRt {
    id: u64,
    rx: Consumer<Chunk>,
    epoch: u32,
    valid: bool,
    pending: bool,
    /// Song frame of the next batch if the transport continues.
    next: u64,
    lp: Option<(u64, u64)>,
    /// Playback index of the next frame to consume.
    played: u64,
    /// The epoch was opened by [`Self::follow_seek`]: its index 0 is song frame `origin` of the
    /// song's own timeline, without a loop.
    followed: bool,
    origin: u64,
}

impl StreamLaneRt {
    pub(crate) fn new(id: u64, rx: Consumer<Chunk>) -> Self {
        Self {
            id,
            rx,
            epoch: 0,
            valid: false,
            pending: false,
            next: 0,
            lp: None,
            played: 0,
            followed: false,
            origin: 0,
        }
    }

    /// Called at every batch start (playing or stopped) with the current position and loop:
    /// opens a new epoch and requests a refill when the position or loop is not the
    /// continuation of the last batch; retries a request the queue could not take.
    pub(crate) fn sync(
        &mut self,
        pos: u64,
        lp: Option<(u64, u64)>,
        requests: &mut Producer<StreamRequest>,
        stats: &StreamStats,
    ) {
        if !(self.valid && !self.followed && self.next == pos && self.lp == lp) {
            self.epoch = self.epoch.wrapping_add(1);
            self.valid = true;
            self.followed = false;
            self.next = pos;
            self.lp = lp;
            self.played = 0;
            self.pending = true;
            // Drop what the old epoch left in the ring (one wait-free commit), so the streamer
            // has room to refill even while the transport is stopped.
            let stale = self.rx.slots();
            if stale > 0
                && let Ok(chunk) = self.rx.read_chunk(stale)
            {
                chunk.commit_all();
            }
        }
        self.push_pending(pos, lp, requests, stats);
    }

    /// Sends the refill request of the open epoch if the queue could not take it before.
    fn push_pending(
        &mut self,
        pos: u64,
        lp: Option<(u64, u64)>,
        requests: &mut Producer<StreamRequest>,
        stats: &StreamStats,
    ) {
        if self.pending {
            let request = StreamRequest {
                lane: self.id,
                epoch: self.epoch,
                start: pos,
                lp,
            };
            self.pending = requests.push(request).is_err();
            if !self.pending {
                stats.requested.fetch_add(1, Ordering::Release);
            }
        }
    }

    /// While the player follows an external clock (see the module docs): makes sure the lane
    /// reads from `origin`, a frame of the song's own timeline, with no loop. An epoch that
    /// already starts there and has not been consumed is kept (the read-ahead of a stopped
    /// transport survives the start); any other opens a new one and requests a refill. Also
    /// retries a request the queue could not take.
    pub(crate) fn follow_seek(
        &mut self,
        origin: u64,
        requests: &mut Producer<StreamRequest>,
        stats: &StreamStats,
    ) {
        if !(self.valid && self.followed && self.origin == origin && self.played == 0) {
            self.epoch = self.epoch.wrapping_add(1);
            self.valid = true;
            self.followed = true;
            self.origin = origin;
            self.lp = None;
            self.played = 0;
            self.pending = true;
            let stale = self.rx.slots();
            if stale > 0
                && let Ok(chunk) = self.rx.read_chunk(stale)
            {
                chunk.commit_all();
            }
        }
        self.push_pending(origin, None, requests, stats);
    }

    /// Retries the request of a followed epoch that the queue could not take.
    pub(crate) fn follow_retry(
        &mut self,
        requests: &mut Producer<StreamRequest>,
        stats: &StreamStats,
    ) {
        self.push_pending(self.origin, None, requests, stats);
    }

    /// The song frame the followed epoch started at, or `None` when the lane is not in one.
    pub(crate) fn follow_origin(&self) -> Option<u64> {
        (self.valid && self.followed).then_some(self.origin)
    }

    /// Playback index of the next frame to consume.
    pub(crate) fn played(&self) -> u64 {
        self.played
    }

    /// Moves the cursor forward to playback index `idx` without using the frames (never back;
    /// frames the streamer has not delivered yet are not counted as missing).
    pub(crate) fn skip_to(&mut self, idx: u64) {
        if idx > self.played {
            self.pull((idx - self.played) as usize, None);
        }
    }

    /// Records where the transport ended the batch (the continuation point).
    pub(crate) fn end_batch(&mut self, pos: u64) {
        if !self.followed {
            self.next = pos;
        }
    }

    /// Consumes `n` frames without using them.
    pub(crate) fn skip(&mut self, n: usize) {
        self.pull(n, None);
    }

    /// Consumes `n` frames into `l` and `r` (each `n` long); returns the number of frames that
    /// were not available (left as silence).
    pub(crate) fn take(&mut self, n: usize, l: &mut [f32], r: &mut [f32]) -> usize {
        self.pull(n, Some((l, r)))
    }

    fn pull(&mut self, n: usize, mut dst: Option<(&mut [f32], &mut [f32])>) -> usize {
        let mut done = 0;
        let mut missing = 0;
        while done < n {
            let Ok(c) = self.rx.peek() else {
                missing += n - done;
                self.played += (n - done) as u64;
                break;
            };
            if c.epoch != self.epoch || self.played >= c.seq0 + u64::from(c.len) {
                let _ = self.rx.pop(); // stale epoch or already consumed
                continue;
            }
            if c.seq0 > self.played {
                // The streamer's chunk starts later than we are: a gap of silence.
                let gap = ((c.seq0 - self.played) as usize).min(n - done);
                missing += gap;
                self.played += gap as u64;
                done += gap;
                continue;
            }
            let from = (self.played - c.seq0) as usize;
            let take = (usize::from(c.len) - from).min(n - done);
            if let Some((l, r)) = dst.as_mut() {
                l[done..done + take].copy_from_slice(&c.data[0][from..from + take]);
                r[done..done + take].copy_from_slice(&c.data[1][from..from + take]);
            }
            self.played += take as u64;
            done += take;
        }
        missing
    }
}

/// The streamer-side half of a lane.
pub(crate) struct LaneFill {
    id: u64,
    data: Arc<LaneData>,
    tx: Producer<Chunk>,
    prime_frames: u64,
    epoch: u32,
    ph: u64,
    seq: u64,
    lp: Option<(u64, u64)>,
    active: bool,
    produced: u64,
    primed: bool,
    scratch: Vec<Vec<f32>>,
}

impl LaneFill {
    pub(crate) fn new(
        id: u64,
        data: Arc<LaneData>,
        tx: Producer<Chunk>,
        prime_frames: u64,
    ) -> Self {
        Self {
            id,
            data,
            tx,
            prime_frames,
            epoch: 0,
            ph: 0,
            seq: 0,
            lp: None,
            active: false,
            produced: 0,
            primed: false,
            scratch: Vec::new(),
        }
    }

    fn restart(&mut self, r: &StreamRequest) {
        self.epoch = r.epoch;
        self.ph = r.start;
        self.seq = 0;
        self.lp = r.lp;
        self.active = true;
        self.produced = 0;
        self.primed = false;
    }

    /// Fills up to `ROUND_CHUNKS` chunks; true if it produced any.
    fn fill(&mut self) -> bool {
        if !self.active {
            return false;
        }
        let mut did = false;
        for _ in 0..ROUND_CHUNKS {
            if self.tx.slots() == 0 {
                break;
            }
            if let Some((ls, le)) = self.lp
                && self.ph == le
            {
                self.ph = ls;
            }
            let len = match self.lp {
                Some((_, le)) if self.ph < le => CHUNK.min((le - self.ph) as usize),
                _ => CHUNK,
            };
            let mut chunk = Chunk {
                epoch: self.epoch,
                seq0: self.seq,
                len: len as u16,
                data: [[0.0; CHUNK]; 2],
            };
            self.render(self.ph, len, &mut chunk);
            if self.tx.push(chunk).is_err() {
                break;
            }
            self.ph += len as u64;
            self.seq += len as u64;
            self.produced += len as u64;
            did = true;
        }
        self.primed = self.produced >= self.prime_frames || self.tx.slots() == 0;
        did
    }

    /// Renders the streamed regions of song frames `a..a + len` into `chunk`: the samples
    /// times the region gain, the first region over a frame assigns and a second one (a
    /// crossfade) adds, as the audio thread does for memory regions.
    fn render(&mut self, a: u64, len: usize, chunk: &mut Chunk) {
        let b = a + len as u64;
        let data = Arc::clone(&self.data);
        let first = data.regions.partition_point(|r| r.end() <= a);
        let mut written = a;
        for r in &data.regions[first..] {
            if r.song_start_frame >= b {
                break;
            }
            let SongAsset::Stream {
                frames,
                channels,
                source,
            } = &data.assets[r.asset.0 as usize]
            else {
                continue;
            };
            let lo = a.max(r.song_start_frame);
            let hi = b.min(r.end());
            let n = (hi - lo) as usize;
            let i0 = lo - r.song_start_frame;
            // The region frames `k0..k1` that read inside the asset; `src_lo` is the lowest
            // asset frame wanted and `m` how many to read.
            let (k0, k1, src_lo) = if r.reverse {
                let s0 = r.source_offset - i0;
                let lead = if *frames == 0 {
                    n as u64
                } else {
                    s0.saturating_sub(frames - 1).min(n as u64)
                } as usize;
                (lead, n, (s0 + 1).saturating_sub(n as u64))
            } else {
                let src0 = r.source_offset + i0;
                let valid = (n as u64).min(frames.saturating_sub(src0)) as usize;
                (0, valid, src0)
            };
            let m = k1.saturating_sub(k0);
            if m > 0 {
                let channels = usize::from(*channels);
                self.scratch.resize_with(channels, Vec::new);
                for c in &mut self.scratch {
                    c.clear();
                    c.resize(m, 0.0);
                }
                let read = source.read(src_lo, &mut self.scratch).min(m);
                let second = channels.min(2) - 1;
                let mut g = [0.0f32; CHUNK];
                r.gains(data.envelope(r), i0 + k0 as u64, &mut g[..m]);
                let at = (lo - a) as usize;
                let added = (written.saturating_sub(lo)).min(n as u64) as usize;
                for k in k0..k1 {
                    // Scratch holds ascending asset frames; a reversed region walks it down.
                    let j = if r.reverse { k1 - 1 - k } else { k - k0 };
                    let (vl, vr) = if j < read {
                        let gk = g[k - k0];
                        (self.scratch[0][j] * gk, self.scratch[second][j] * gk)
                    } else {
                        (0.0, 0.0)
                    };
                    if k < added {
                        chunk.data[0][at + k] += vl;
                        chunk.data[1][at + k] += vr;
                    } else {
                        chunk.data[0][at + k] = vl;
                        chunk.data[1][at + k] = vr;
                    }
                }
            }
            written = written.max(hi);
        }
    }
}

enum Command {
    /// Replace the set of lanes (the previous lanes are dropped here).
    Lanes(Vec<LaneFill>),
}

/// The streamer thread and its command channel (owned by the engine handle).
pub(crate) struct Streamer {
    stats: Arc<StreamStats>,
    commands: Sender<Command>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Streamer {
    /// Spawns the thread (non-RT).
    pub(crate) fn spawn(requests: Consumer<StreamRequest>, stats: Arc<StreamStats>) -> Self {
        let (commands, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stats = Arc::clone(&stats);
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("atomik-streamer".into())
                .spawn(move || run(rx, requests, thread_stats, stop))
                .ok()
        };
        Self {
            stats,
            commands,
            stop,
            thread,
        }
    }

    /// Replaces the lanes the streamer serves.
    pub(crate) fn set_lanes(&self, lanes: Vec<LaneFill>) {
        let _ = self.commands.send(Command::Lanes(lanes));
        self.wake();
    }

    /// Wakes the thread out of a park and restarts its quiet time.
    fn wake(&self) {
        self.stats.wakes.fetch_add(1, Ordering::Release);
        if let Some(t) = &self.thread {
            t.thread().unpark();
        }
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

/// How long the streamer rests between rounds: it counts the rounds with nothing to do.
#[derive(Debug, Default)]
struct Rest {
    quiet: u32,
    wakes_seen: u64,
}

impl Rest {
    /// The rest after a round that `worked` (filled a chunk), `served` requests and `applied`
    /// lane-set commands, with `wakes` the count of wake-ups so far: none while there was
    /// something to do (the next round looks for more at once, as a refill must not be
    /// throttled), [`SHORT_PARK`] after a round with nothing to do, [`IDLE_PARK`] once
    /// [`QUIET_ROUNDS`] such rounds followed one another with no wake-up between.
    fn after(&mut self, worked: bool, served: u64, applied: u64, wakes: u64) -> Duration {
        if worked || served > 0 || applied > 0 || wakes != self.wakes_seen {
            self.wakes_seen = wakes;
            self.quiet = 0;
            return Duration::ZERO;
        }
        self.quiet = self.quiet.saturating_add(1);
        if self.quiet >= QUIET_ROUNDS {
            IDLE_PARK
        } else {
            SHORT_PARK
        }
    }
}

fn run(
    commands: Receiver<Command>,
    mut requests: Consumer<StreamRequest>,
    stats: Arc<StreamStats>,
    stop: Arc<AtomicBool>,
) {
    let mut lanes: Vec<LaneFill> = Vec::new();
    let mut rest = Rest::default();
    while !stop.load(Ordering::Acquire) {
        let mut applied = 0;
        while let Ok(Command::Lanes(new)) = commands.try_recv() {
            lanes = new;
            applied += 1;
        }
        let mut served = 0;
        while let Ok(req) = requests.pop() {
            if let Some(l) = lanes.iter_mut().find(|l| l.id == req.lane) {
                l.restart(&req);
            }
            served += 1;
        }
        let mut worked = false;
        for l in &mut lanes {
            worked |= l.fill();
        }
        stats.primed.store(
            lanes.iter().all(|l| l.primed || l.tx.is_abandoned()),
            Ordering::Release,
        );
        // Acknowledge only after `primed` reflects the restarts and new lanes.
        stats.served.fetch_add(served, Ordering::Release);
        stats.commands_applied.fetch_add(applied, Ordering::Release);
        stats.rounds.fetch_add(1, Ordering::Relaxed);
        let wakes = stats.wakes.load(Ordering::Acquire);
        std::thread::park_timeout(rest.after(worked, served, applied, wakes));
    }
}

/// Handle-side state: the streamer (spawned on first use), the request consumer waiting for it
/// and the counters.
pub(crate) struct StreamControl {
    pub(crate) stats: Arc<StreamStats>,
    requests: Option<Consumer<StreamRequest>>,
    streamer: Option<Streamer>,
    next_lane: u64,
}

impl StreamControl {
    pub(crate) fn new(stats: Arc<StreamStats>, requests: Consumer<StreamRequest>) -> Self {
        Self {
            stats,
            requests: Some(requests),
            streamer: None,
            next_lane: 1,
        }
    }

    /// Wakes the streamer (if there is one) and restarts its quiet time: the app calls it for
    /// every transport command, so a locate or a start finds the thread awake.
    pub(crate) fn wake(&self) {
        if let Some(s) = &self.streamer {
            s.wake();
        }
    }

    /// Attaches stream lanes to `song` (creating its rings) and hands their producers to the
    /// streamer. `sample_rate` sizes the rings. Call before the snapshot is queued.
    pub(crate) fn attach(&mut self, song: &mut crate::SongSnapshot, sample_rate: f64) {
        let ring = ((RING_SECONDS * sample_rate) / CHUNK as f64)
            .ceil()
            .max(2.0) as usize;
        let prime = (PRIME_SECONDS * sample_rate) as u64;
        let mut lanes = Vec::new();
        for (k, a) in song.audio.iter().enumerate() {
            if !a.streams {
                continue;
            }
            let (tx, rx) = rtrb::RingBuffer::new(ring);
            let id = self.next_lane;
            self.next_lane += 1;
            song.audio_rt[k].stream = Some(StreamLaneRt::new(id, rx));
            lanes.push(LaneFill::new(id, Arc::clone(&a.data), tx, prime));
        }
        if lanes.is_empty() && self.streamer.is_none() {
            return;
        }
        if self.streamer.is_none()
            && let Some(rx) = self.requests.take()
        {
            self.streamer = Some(Streamer::spawn(rx, Arc::clone(&self.stats)));
        }
        if let Some(s) = &self.streamer {
            self.stats.commands_sent.fetch_add(1, Ordering::Release);
            s.set_lanes(lanes);
        }
    }
}

const _: () = assert!(CHUNK <= u16::MAX as usize && BATCH <= CHUNK);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_with_nothing_to_do_waits_briefly_and_only_a_long_quiet_rests_long() {
        let mut r = Rest::default();
        // Something to do: no wait at all.
        assert_eq!(r.after(true, 0, 0, 0), Duration::ZERO);
        assert_eq!(
            r.after(false, 1, 0, 0),
            Duration::ZERO,
            "a request was served"
        );
        assert_eq!(
            r.after(false, 0, 1, 0),
            Duration::ZERO,
            "new lanes were applied"
        );
        // Nothing to do: the short wait, until the quiet time has passed.
        for _ in 1..QUIET_ROUNDS {
            assert_eq!(r.after(false, 0, 0, 0), SHORT_PARK);
        }
        assert_eq!(r.after(false, 0, 0, 0), IDLE_PARK, "quiet for the constant");
        assert_eq!(r.after(false, 0, 0, 0), IDLE_PARK);
        // A wake restarts the quiet time.
        assert_eq!(r.after(false, 0, 0, 1), Duration::ZERO);
        assert_eq!(r.after(false, 0, 0, 1), SHORT_PARK);
        // Work restarts it too.
        for _ in 0..QUIET_ROUNDS {
            r.after(false, 0, 0, 1);
        }
        assert_eq!(r.after(true, 0, 0, 1), Duration::ZERO);
        assert_eq!(r.after(false, 0, 0, 1), SHORT_PARK);
    }

    #[test]
    fn a_playing_pattern_of_one_busy_round_in_six_never_rests_long() {
        let mut r = Rest::default();
        for round in 0..10_000u32 {
            let busy = round % 6 == 0;
            let wait = r.after(busy, 0, 0, 0);
            assert_ne!(wait, IDLE_PARK, "round {round}");
        }
    }

    #[test]
    fn the_constants_keep_the_wait_short() {
        assert!(SHORT_PARK < IDLE_PARK);
        assert!(
            IDLE_PARK <= Duration::from_millis(50),
            "bounds the one case nothing wakes"
        );
    }
}
