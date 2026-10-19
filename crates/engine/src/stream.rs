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
//! * **Playback order.** The ring carries the track's *streamed regions' raw samples in playback
//!   order*: chunk `k` holds the frames `seq0..seq0 + len` counted from the start of the current
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
//!   the audio equals the `Memory` output bit for bit (same samples, same gain/fade code).
//! * **Streamer.** One thread, spawned lazily by the first `submit_song` with a streamed
//!   asset, joined when the handle is dropped. It polls (1 ms) requests and lane commands and
//!   fills each active lane round-robin, a few chunks at a time, so a slow source delays other
//!   lanes but never the audio thread. `StreamSource::read` runs only here.

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
    /// Every lane has `PRIME_SECONDS` buffered (streamer updates).
    pub(crate) primed: AtomicBool,
    /// Refill requests the audio thread has queued / the streamer has handled (equal when no
    /// request is in flight).
    pub(crate) requested: AtomicU64,
    pub(crate) served: AtomicU64,
    /// Lane-set commands the handle has sent / the streamer has applied.
    pub(crate) commands_sent: AtomicU64,
    pub(crate) commands_applied: AtomicU64,
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
            primed: AtomicBool::new(true),
            requested: AtomicU64::new(0),
            served: AtomicU64::new(0),
            commands_sent: AtomicU64::new(0),
            commands_applied: AtomicU64::new(0),
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
        if !(self.valid && self.next == pos && self.lp == lp) {
            self.epoch = self.epoch.wrapping_add(1);
            self.valid = true;
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

    /// Records where the transport ended the batch (the continuation point).
    pub(crate) fn end_batch(&mut self, pos: u64) {
        self.next = pos;
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

    /// Reads the streamed regions' samples of song frames `a..a + len` into `chunk`.
    fn render(&mut self, a: u64, len: usize, chunk: &mut Chunk) {
        let b = a + len as u64;
        let data = Arc::clone(&self.data);
        let first = data.regions.partition_point(|r| r.end() <= a);
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
            let src0 = r.source_offset + (lo - r.song_start_frame);
            let n = ((hi - lo).min(frames.saturating_sub(src0))) as usize;
            if n == 0 {
                continue;
            }
            let channels = usize::from(*channels);
            self.scratch.resize_with(channels, Vec::new);
            for c in &mut self.scratch {
                c.clear();
                c.resize(n, 0.0);
            }
            let read = source.read(src0, &mut self.scratch).min(n);
            let at = (lo - a) as usize;
            chunk.data[0][at..at + read].copy_from_slice(&self.scratch[0][..read]);
            let second = &self.scratch[channels.min(2) - 1];
            chunk.data[1][at..at + read].copy_from_slice(&second[..read]);
        }
    }
}

enum Command {
    /// Replace the set of lanes (the previous lanes are dropped here).
    Lanes(Vec<LaneFill>),
}

/// The streamer thread and its command channel (owned by the engine handle).
pub(crate) struct Streamer {
    commands: Sender<Command>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Streamer {
    /// Spawns the thread (non-RT).
    pub(crate) fn spawn(requests: Consumer<StreamRequest>, stats: Arc<StreamStats>) -> Self {
        let (commands, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("atomik-streamer".into())
                .spawn(move || run(rx, requests, stats, stop))
                .ok()
        };
        Self {
            commands,
            stop,
            thread,
        }
    }

    /// Replaces the lanes the streamer serves.
    pub(crate) fn set_lanes(&self, lanes: Vec<LaneFill>) {
        let _ = self.commands.send(Command::Lanes(lanes));
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

fn run(
    commands: Receiver<Command>,
    mut requests: Consumer<StreamRequest>,
    stats: Arc<StreamStats>,
    stop: Arc<AtomicBool>,
) {
    let mut lanes: Vec<LaneFill> = Vec::new();
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
        if !worked {
            std::thread::park_timeout(Duration::from_millis(1));
        }
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
