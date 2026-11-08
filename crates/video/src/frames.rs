//! The frames the viewer shows: a small cache of decoded pictures and the worker that fills it.
//!
//! The window never reads or decodes a frame itself. It asks a [`FrameDecoder`] for the frames
//! it wants (the one at the song position first, then the likeliest next ones, see
//! [`crate::timeline::wanted`]) and takes what has been decoded with [`FrameDecoder::poll`]. The
//! worker thread opens the proxy, reads each frame (one seek) and decodes it. A new request
//! replaces the old one: a frame the song has already passed is never decoded, so a slow decode
//! drops frames instead of lagging behind the song.
//!
//! [`FrameCache`] keeps the decoded pictures under a byte budget, the least recently used out
//! first, so decode-ahead frames and the frames around the position stay while an hour of
//! scrubbing does not grow the memory.

use crate::VideoError;
use crate::decode::{Picture, decode_jpeg};
use crate::proxy::{ProxyInfo, ProxyReader};
use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::sync::mpsc::{Receiver, RecvError, Sender, TryRecvError, channel};

/// The bytes of decoded pictures the cache keeps: about 50 frames of a 640 by 360 proxy.
pub const CACHE_BYTES: usize = 48 * 1024 * 1024;

/// Decoded pictures by frame number, the least recently used leaving first.
#[derive(Debug)]
pub struct FrameCache {
    frames: BTreeMap<u32, (Picture, u64)>,
    bytes: usize,
    budget: usize,
    clock: u64,
}

impl Default for FrameCache {
    fn default() -> Self {
        Self::new(CACHE_BYTES)
    }
}

impl FrameCache {
    /// An empty cache that keeps at most `budget` bytes of pixels (always at least the newest
    /// picture).
    pub fn new(budget: usize) -> Self {
        Self {
            frames: BTreeMap::new(),
            bytes: 0,
            budget,
            clock: 0,
        }
    }

    /// The picture of frame `n`, which counts as used now.
    pub fn get(&mut self, n: u32) -> Option<&Picture> {
        self.clock += 1;
        let clock = self.clock;
        let (picture, used) = self.frames.get_mut(&n)?;
        *used = clock;
        Some(picture)
    }

    /// Whether frame `n` is held (it does not count as used).
    pub fn contains(&self, n: u32) -> bool {
        self.frames.contains_key(&n)
    }

    /// Adds the picture of frame `n` and drops the least recently used until the budget holds.
    pub fn insert(&mut self, n: u32, picture: Picture) {
        self.clock += 1;
        self.bytes += picture.bytes();
        if let Some((old, _)) = self.frames.insert(n, (picture, self.clock)) {
            self.bytes -= old.bytes();
        }
        while self.bytes > self.budget && self.frames.len() > 1 {
            let oldest = self
                .frames
                .iter()
                .filter(|(k, _)| **k != n)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| *k);
            let Some(oldest) = oldest else { break };
            if let Some((gone, _)) = self.frames.remove(&oldest) {
                self.bytes -= gone.bytes();
            }
        }
    }

    /// Forgets every picture (another film).
    pub fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
    }

    /// How many pictures are held.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The bytes of pixels held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The frames held, in order.
    pub fn held(&self) -> Vec<u32> {
        self.frames.keys().copied().collect()
    }
}

/// What the worker reports.
#[derive(Debug)]
pub enum Event {
    /// The proxy is open: what it states.
    Opened(ProxyInfo),
    /// Frame `n` is decoded.
    Frame(u32, Picture),
    /// Frame `n` could not be read or decoded (the message is for the user); `None` for the
    /// proxy itself (it could not be opened).
    Failed(Option<u32>, String),
}

enum Request {
    /// Decode these frames, in this order (replaces the list before it).
    Want(Vec<u32>),
}

/// The worker that opens a proxy and decodes the frames asked of it (see the module docs).
///
/// Dropping the decoder tells the thread to stop; it ends after the frame it is decoding.
#[derive(Debug)]
pub struct FrameDecoder {
    tx: Sender<Request>,
    rx: Receiver<Event>,
}

impl FrameDecoder {
    /// Starts a worker that opens the proxy `open` returns (on its own thread: the file is not
    /// touched on the caller's) and decodes what is asked.
    ///
    /// # Errors
    /// The thread cannot be started.
    pub fn spawn<R, F>(open: F) -> std::io::Result<Self>
    where
        R: Read + Seek,
        F: FnOnce() -> Result<ProxyReader<R>, VideoError> + Send + 'static,
    {
        let (tx, requests) = channel();
        let (events, rx) = channel();
        std::thread::Builder::new()
            .name("atomik-video-frames".into())
            .spawn(move || work(open, &requests, &events))?;
        Ok(Self { tx, rx })
    }

    /// Starts a worker on the proxy file at `path`.
    ///
    /// # Errors
    /// The thread cannot be started (a file that cannot be opened is an [`Event::Failed`]).
    pub fn spawn_file(path: std::path::PathBuf) -> std::io::Result<Self> {
        Self::spawn(move || ProxyReader::open(std::fs::File::open(&path)?))
    }

    /// Asks for `frames`, the first one first. It replaces the earlier request: what the worker
    /// has not started is dropped.
    pub fn want(&self, frames: Vec<u32>) {
        let _ = self.tx.send(Request::Want(frames));
    }

    /// Everything the worker has finished since the last call, in order.
    pub fn poll(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }
}

fn work<R, F>(open: F, requests: &Receiver<Request>, events: &Sender<Event>)
where
    R: Read + Seek,
    F: FnOnce() -> Result<ProxyReader<R>, VideoError>,
{
    let mut reader = match open() {
        Ok(r) => r,
        Err(e) => {
            let _ = events.send(Event::Failed(None, e.to_string()));
            return;
        }
    };
    let _ = events.send(Event::Opened(reader.info()));
    let mut bytes = Vec::new();
    let mut todo: Vec<u32> = Vec::new();
    loop {
        // Nothing to do: wait. Something to do: only look at what has come in meanwhile.
        let next = if todo.is_empty() {
            requests
                .recv()
                .map_err(|RecvError| TryRecvError::Disconnected)
        } else {
            requests.try_recv()
        };
        match next {
            Ok(Request::Want(list)) => {
                todo = list;
                todo.reverse();
                // The newest request may have company: only the last one counts.
                continue;
            }
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }
        let Some(n) = todo.pop() else { continue };
        let result = reader
            .read_frame(n, &mut bytes)
            .and_then(|()| decode_jpeg(&bytes));
        let event = match result {
            Ok(picture) => Event::Frame(n, picture),
            Err(e) => Event::Failed(Some(n), e.to_string()),
        };
        if events.send(event).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Solid, solid_jpeg};
    use crate::probe::Rational;
    use crate::proxy::ProxyWriter;
    use std::io::Cursor;
    use std::time::Duration;

    fn picture(side: u32) -> Picture {
        Picture {
            width: side,
            height: side,
            rgba: std::sync::Arc::new(vec![0; (side * side * 4) as usize]),
        }
    }

    #[test]
    fn the_cache_drops_the_least_recently_used_first() {
        // Room for three 16x16 pictures.
        let mut cache = FrameCache::new(3 * 16 * 16 * 4);
        for n in 0..3 {
            cache.insert(n, picture(16));
        }
        assert!(cache.get(0).is_some());
        cache.insert(3, picture(16));
        assert_eq!(
            cache.held(),
            [0, 2, 3],
            "frame 1 was the least recently used"
        );
        assert_eq!(cache.bytes(), 3 * 16 * 16 * 4);
        assert!(cache.contains(2) && !cache.contains(1));
    }

    #[test]
    fn replacing_a_frame_does_not_count_it_twice() {
        let mut cache = FrameCache::new(10 * 16 * 16 * 4);
        cache.insert(5, picture(16));
        cache.insert(5, picture(16));
        assert_eq!((cache.len(), cache.bytes()), (1, 16 * 16 * 4));
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn a_picture_bigger_than_the_budget_is_still_held() {
        let mut cache = FrameCache::new(10);
        cache.insert(1, picture(16));
        assert_eq!(cache.held(), [1]);
        cache.insert(2, picture(16));
        assert_eq!(cache.held(), [2]);
    }

    fn proxy_of(colours: &[Solid]) -> Vec<u8> {
        let mut w =
            ProxyWriter::new(Cursor::new(Vec::new()), Rational { num: 25, den: 1 }).unwrap();
        for c in colours {
            w.push_frame(solid_jpeg(*c)).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    /// Polls until `done` holds for the events so far (or about five seconds pass).
    fn collect(decoder: &FrameDecoder, done: impl Fn(&[Event]) -> bool) -> Vec<Event> {
        let mut all = Vec::new();
        for _ in 0..2_500 {
            all.extend(decoder.poll());
            if done(&all) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        all
    }

    #[test]
    fn the_worker_opens_the_proxy_and_decodes_what_is_asked_in_order() {
        let bytes = proxy_of(&[Solid::Red, Solid::Green, Solid::Blue, Solid::White]);
        let decoder = FrameDecoder::spawn(move || ProxyReader::open(Cursor::new(bytes))).unwrap();
        decoder.want(vec![2, 0, 3]);
        let events = collect(&decoder, |e| e.len() >= 4);
        let Event::Opened(info) = &events[0] else {
            panic!("first the proxy opens: {events:?}")
        };
        assert_eq!((info.frames, info.width, info.height), (4, 16, 16));
        let frames: Vec<u32> = events
            .iter()
            .filter_map(|e| match e {
                Event::Frame(n, _) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(frames, [2, 0, 3]);
        let Event::Frame(_, blue) = &events[1] else {
            panic!("{events:?}")
        };
        let [r, g, b, _] = blue.pixel(8, 8).unwrap();
        assert!(b > 200 && r < 60 && g < 60, "frame 2 is blue: {r} {g} {b}");
    }

    #[test]
    fn a_frame_past_the_end_and_a_bad_proxy_are_reported_not_fatal() {
        let bytes = proxy_of(&[Solid::Red, Solid::Green]);
        let decoder = FrameDecoder::spawn(move || ProxyReader::open(Cursor::new(bytes))).unwrap();
        decoder.want(vec![7, 1]);
        let events = collect(&decoder, |e| e.len() >= 3);
        assert!(matches!(events[1], Event::Failed(Some(7), _)), "{events:?}");
        assert!(matches!(events[2], Event::Frame(1, _)), "{events:?}");

        let broken = FrameDecoder::spawn(|| ProxyReader::open(Cursor::new(vec![1, 2, 3]))).unwrap();
        let events = collect(&broken, |e| !e.is_empty());
        assert!(matches!(events[0], Event::Failed(None, _)), "{events:?}");
    }

    #[test]
    fn a_new_request_replaces_the_old_one() {
        let bytes = proxy_of(&[Solid::Red; 40]);
        let decoder = FrameDecoder::spawn(move || ProxyReader::open(Cursor::new(bytes))).unwrap();
        // The worker decodes a frame in a fraction of a millisecond: ask in one go, then ask
        // again for one frame, and check that nothing is decoded twice and the last ask is met.
        decoder.want((0..40).collect());
        decoder.want(vec![39]);
        let events = collect(&decoder, |e| {
            e.iter().any(|e| matches!(e, Event::Frame(39, _)))
        });
        let frames: Vec<u32> = events
            .iter()
            .filter_map(|e| match e {
                Event::Frame(n, _) => Some(*n),
                _ => None,
            })
            .collect();
        assert!(frames.contains(&39), "{frames:?}");
        let mut sorted = frames.clone();
        sorted.dedup();
        assert_eq!(sorted, frames, "no frame twice");
    }

    #[test]
    fn dropping_the_decoder_ends_the_thread() {
        let bytes = proxy_of(&[Solid::Red]);
        let decoder = FrameDecoder::spawn(move || ProxyReader::open(Cursor::new(bytes))).unwrap();
        let events = collect(&decoder, |e| !e.is_empty());
        assert!(matches!(events[0], Event::Opened(_)));
        drop(decoder);
    }
}
