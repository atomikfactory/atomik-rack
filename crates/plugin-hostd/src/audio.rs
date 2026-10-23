//! The audio thread of the child.
//!
//! It owns the [`ChildPort`] for the whole life of the process and the real processor between
//! `Activate` and `Deactivate`. It answers every request of the parent in the order it comes:
//! with the processor when there is one, with an error answer when there is not. Between
//! requests (at least every 10 ms) it takes the processor over from or hands it back to the main
//! thread, and passes on a wake-up the plugin asked for.
//!
//! After setup the loop makes no allocation of its own: the port's buffers are fixed, and the
//! hand-over messages are the only thing that move between threads.

use atomik_plugin_host::{AudioThreadGuard, FtzGuard, PluginProcessor, ProcessStatus};
use atomik_plugin_ipc::{AudioOp, ChildPort, ProcessorInfo};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

/// How long the thread waits for a request before it looks at its other duties.
const WAIT: Duration = Duration::from_millis(10);

/// How long the main thread waits for the audio thread to take or give a processor.
pub const HANDOVER_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a shutdown waits for a processor that is stuck inside its plugin.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

enum Command {
    Install(Box<dyn PluginProcessor>, Sender<()>),
    Take(Sender<Option<Box<dyn PluginProcessor>>>),
}

/// The main thread's handle on the audio thread.
pub struct AudioThread {
    commands: Sender<Command>,
}

impl AudioThread {
    /// Starts the audio thread over `port`.
    ///
    /// # Errors
    /// When the operating system refuses a thread.
    pub fn spawn(port: ChildPort) -> io::Result<Self> {
        let (commands, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("host-audio".to_string())
            .spawn(move || serve(port, &rx))?;
        Ok(Self { commands })
    }

    /// Gives the audio thread the processor; returns once it has taken it over (and told the
    /// parent its sleep rule and tail). False when it did not answer in time.
    pub fn install(&self, processor: Box<dyn PluginProcessor>) -> bool {
        let (ack, acked) = mpsc::channel();
        self.commands.send(Command::Install(processor, ack)).is_ok()
            && acked.recv_timeout(HANDOVER_TIMEOUT).is_ok()
    }

    /// Takes the processor back (stopped if the parent left it started). `None` when the audio
    /// thread has none or did not answer in time.
    pub fn take(&self, timeout: Duration) -> Option<Box<dyn PluginProcessor>> {
        let (reply, replied) = mpsc::channel();
        self.commands.send(Command::Take(reply)).ok()?;
        // A timeout or a vanished audio thread both mean there is nothing to hand back.
        replied.recv_timeout(timeout).unwrap_or_default()
    }
}

/// The processor and what the thread knows about it.
struct Held {
    processor: Box<dyn PluginProcessor>,
    started: bool,
    /// The plugin panicked: every request is answered with an error from now on.
    poisoned: bool,
}

fn priority() {
    // Failing to raise the priority is not an error: the plugin still works.
    let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Max);
}

fn publish_info(port: &ChildPort, held: &Held) {
    port.set_info(ProcessorInfo {
        sleep_rule: held.processor.sleep_rule(),
        tail: held.processor.tail_samples(),
    });
}

fn serve(mut port: ChildPort, commands: &Receiver<Command>) {
    priority();
    let mut held: Option<Held> = None;
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Install(processor, ack)) => {
                    let h = Held {
                        processor,
                        started: false,
                        poisoned: false,
                    };
                    publish_info(&port, &h);
                    held = Some(h);
                    let _ = ack.send(());
                }
                Ok(Command::Take(reply)) => {
                    let taken = held.take().map(|mut h| {
                        if h.started && !h.poisoned {
                            let _guard = AudioThreadGuard::enter();
                            let _ = catch_unwind(AssertUnwindSafe(|| {
                                h.processor.stop_processing();
                            }));
                        }
                        h.processor
                    });
                    let _ = reply.send(taken);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if let Some(request) = port.wait_request(WAIT) {
            let op = request.op();
            match held.as_mut() {
                Some(h) if !h.poisoned => {
                    answer(request, op, h);
                    if op == AudioOp::Process {
                        publish_info(&port, h);
                    }
                }
                _ => request.complete(|_| (ProcessStatus::Error, false)),
            }
        }
        if let Some(h) = held.as_ref()
            && h.processor.wake_requested()
        {
            port.request_wake();
        }
    }
}

/// Runs one request on the processor.
fn answer(request: atomik_plugin_ipc::Request<'_>, op: AudioOp, held: &mut Held) {
    let _guard = AudioThreadGuard::enter();
    let Held {
        processor,
        started,
        poisoned,
    } = held;
    request.complete(|io| {
        let run = catch_unwind(AssertUnwindSafe(|| match op {
            AudioOp::Process => {
                // The same floating-point environment the device gives an in-process plugin.
                let _flush = FtzGuard::enter();
                (processor.process(io), true)
            }
            AudioOp::Start => {
                let ok = processor.start_processing();
                *started = ok;
                (ProcessStatus::Continue, ok)
            }
            AudioOp::Stop => {
                processor.stop_processing();
                *started = false;
                (ProcessStatus::Continue, true)
            }
            AudioOp::Reset => {
                processor.reset();
                (ProcessStatus::Continue, true)
            }
        }));
        run.unwrap_or_else(|_| {
            *poisoned = true;
            (ProcessStatus::Error, false)
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::BATCH;
    use atomik_plugin_host::{
        EventList, FLUSH_SUPPORTED, PluginIo, PluginTransport, denormals_flushed, is_audio_thread,
    };
    use atomik_plugin_ipc::{
        Deadline, Doorbell, DoorbellName, ParentPort, Poll, SharedBlock, block_file_name,
        default_block_dir,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// A processor that records the thread state it was called in.
    struct Probe {
        flushed: Arc<AtomicBool>,
        audio: Arc<AtomicBool>,
    }

    impl PluginProcessor for Probe {
        fn start_processing(&mut self) -> bool {
            true
        }
        fn stop_processing(&mut self) {}
        fn reset(&mut self) {}
        fn process(&mut self, _io: &mut PluginIo<'_>) -> ProcessStatus {
            self.flushed.store(denormals_flushed(), Ordering::SeqCst);
            self.audio.store(is_audio_thread(), Ordering::SeqCst);
            ProcessStatus::Continue
        }
    }

    static NEXT: AtomicU64 = AtomicU64::new(800_000);

    #[test]
    fn the_plugin_runs_with_denormals_flushed_and_on_a_marked_audio_thread() {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = default_block_dir().join(block_file_name(pid, n));
        let name = DoorbellName::for_instance(pid, n);
        let parent_block = SharedBlock::create(&path).unwrap();
        let parent_bell = Doorbell::create(&name, &parent_block).unwrap();
        let child_block = SharedBlock::open(&path).unwrap();
        let child_bell = Doorbell::open(&name, &child_block).unwrap();
        let mut parent = ParentPort::new(parent_block, parent_bell);
        let audio = AudioThread::spawn(ChildPort::new(child_block, child_bell)).unwrap();

        let (flushed, on_audio) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        assert!(audio.install(Box::new(Probe {
            flushed: Arc::clone(&flushed),
            audio: Arc::clone(&on_audio),
        })));
        let mut out = [[0.0; BATCH]; 2];
        let mut events = EventList::with_capacity(8);
        let mut io = PluginIo {
            frames: BATCH,
            main_in: &[],
            main_in_silent: true,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut out,
            events_in: &[],
            events_out: &mut events,
            transport: PluginTransport::default(),
            dropped: 0,
        };
        let t = parent.submit(AudioOp::Process, &io);
        assert_eq!(
            parent.wait_done(t, Deadline::after(Duration::from_secs(10))),
            Poll::Done
        );
        parent.take_response(t, &mut io);
        assert!(
            on_audio.load(Ordering::SeqCst),
            "the plugin is on an audio thread"
        );
        assert_eq!(
            flushed.load(Ordering::SeqCst),
            FLUSH_SUPPORTED,
            "denormals are flushed as in process"
        );
    }
}
