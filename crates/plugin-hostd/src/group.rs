//! A group child: one process that hosts several plugin instances.
//!
//! The child is started with `--group <format> --parent-pid <pid>`. Its stdin and stdout carry
//! slot frames (see `atomik_plugin_ipc::group`): slot 0 is the group, every other slot is one
//! instance. An `Attach` on slot 0 makes the instance: its shared block, its doorbell, **its own
//! audio thread** and its own [`Control`], which then speaks the ordinary control protocol on its
//! slot. One audio thread per instance keeps the real-time rule of the ungrouped child: an
//! instance's batch is served by a thread that serves nothing else, so a slow plugin delays only
//! its own audio, and no lock or queue is shared between instances on the audio path.
//!
//! The main thread is the main thread of every plugin in the group (control channel, timers,
//! editor windows). A crash, an abort or a hang of one plugin ends or stalls the whole process:
//! that is the group's crash domain, and the parent treats every instance of the group alike.

use crate::args::{Args, Format};
use crate::audio::AudioThread;
use crate::control::{self, Control, Event, WindowInput};
use crate::editor::WindowSystem;
use atomik_plugin_host::mark_main_thread;
use atomik_plugin_ipc::lifetime::exit_after;
use atomik_plugin_ipc::{
    ChildPort, Doorbell, DoorbellName, FromGroup, GROUP_SLOT, MAX_GROUP_INSTANCES, SharedBlock,
    SlotWriter, ToChild, ToGroup, read_frame, read_slot_frame, write_frame, write_slot_frame,
};
use std::collections::BTreeMap;
use std::io::{Cursor, ErrorKind, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};

/// What happened that the group has to react to.
#[derive(Debug)]
pub enum GroupEvent {
    /// A request for one instance, with its blob.
    Frame {
        /// The instance's slot.
        slot: u32,
        /// The request.
        frame: Box<(ToChild, Option<Vec<u8>>)>,
    },
    /// The parent adds an instance.
    Attach(Box<ToGroup>),
    /// A frame on a slot could not be read: that instance is dropped.
    Broken {
        /// The slot.
        slot: u32,
        /// Why.
        text: String,
    },
    /// Stdin ended: the parent is gone or closed the channel.
    Eof,
    /// Time to poll the plugins and forward their messages.
    Tick,
}

/// Where the helper threads deliver group events.
pub trait PostGroup: Clone + Send + 'static {
    /// Delivers `event`; false when the loop is gone.
    fn post(&self, event: GroupEvent) -> bool;
}

impl PostGroup for mpsc::Sender<GroupEvent> {
    fn post(&self, event: GroupEvent) -> bool {
        self.send(event).is_ok()
    }
}

/// The window systems of the instances: one per slot.
pub trait SlotSystems {
    /// The window system of `slot`.
    fn select(&mut self, slot: u32) -> &mut dyn WindowSystem;
    /// The instance left: its window goes with it.
    fn forget(&mut self, slot: u32);
}

/// The no-display window systems: every window request fails.
pub struct NoSlotWindows(crate::driver::NoWindows);

impl Default for NoSlotWindows {
    fn default() -> Self {
        Self(crate::driver::NoWindows)
    }
}

impl SlotSystems for NoSlotWindows {
    fn select(&mut self, _slot: u32) -> &mut dyn WindowSystem {
        &mut self.0
    }
    fn forget(&mut self, _slot: u32) {}
}

/// The child's stdout, shared by the instances: each frame is written whole under the lock.
#[derive(Clone)]
struct SharedOut(Arc<Mutex<Box<dyn Write + Send>>>);

impl Write for SharedOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .flush()
    }
}

/// The main-thread half of a group child.
pub struct GroupHost {
    format: Format,
    parent_pid: u32,
    format_override: Option<String>,
    out: SharedOut,
    slots: BTreeMap<u32, Control>,
}

impl GroupHost {
    /// A group of `format` whose frames go to `out`.
    pub fn new(
        format: Format,
        parent_pid: u32,
        format_override: Option<String>,
        out: Box<dyn Write + Send>,
    ) -> Self {
        Self {
            format,
            parent_pid,
            format_override,
            out: SharedOut(Arc::new(Mutex::new(out))),
            slots: BTreeMap::new(),
        }
    }

    fn say(&mut self, msg: &FromGroup) {
        let mut inner = Vec::new();
        if write_frame(&mut inner, msg, None).is_ok() {
            // A broken pipe here shows as EOF on stdin, which ends the process.
            let _ = write_slot_frame(&mut self.out, GROUP_SLOT, &inner);
        }
    }

    /// Reacts to one event; `Some(code)` ends the child with that exit code.
    pub fn on_event(&mut self, event: GroupEvent, systems: &mut dyn SlotSystems) -> Option<u8> {
        match event {
            GroupEvent::Eof => {
                for (slot, control) in &mut self.slots {
                    control.shutdown(systems.select(*slot));
                }
                self.slots.clear();
                Some(0)
            }
            GroupEvent::Attach(attach) => {
                let ToGroup::Attach {
                    slot,
                    plugin_file,
                    plugin_id,
                    block,
                    doorbell,
                } = *attach;
                if let Err(text) = self.attach(slot, plugin_file, plugin_id, block, doorbell) {
                    self.say(&FromGroup::AttachFailed { slot, text });
                }
                None
            }
            GroupEvent::Broken { slot, text } => {
                eprintln!("atomik-plugin-host: slot {slot} broke its channel: {text}");
                self.detach(slot, systems);
                None
            }
            GroupEvent::Frame { slot, frame } => {
                let control = self.slots.get_mut(&slot)?;
                let code = control.on_event(Event::Frame(frame), systems.select(slot));
                if code.is_some() {
                    self.detach(slot, systems);
                }
                None
            }
            GroupEvent::Tick => {
                let mut gone = Vec::new();
                for (slot, control) in &mut self.slots {
                    if control
                        .on_event(Event::Tick, systems.select(*slot))
                        .is_some()
                    {
                        gone.push(*slot);
                    }
                }
                for slot in gone {
                    self.detach(slot, systems);
                }
                None
            }
        }
    }

    /// Reacts to what the user did to the editor window of `slot`.
    pub fn on_window(
        &mut self,
        slot: u32,
        input: WindowInput,
        systems: &mut dyn SlotSystems,
    ) -> Option<u8> {
        let control = self.slots.get_mut(&slot)?;
        if control.on_window(input, systems.select(slot)).is_some() {
            self.detach(slot, systems);
        }
        None
    }

    /// Makes the instance of `slot`: block, doorbell, audio thread, control; then it greets.
    fn attach(
        &mut self,
        slot: u32,
        plugin_file: String,
        plugin_id: String,
        block: String,
        doorbell: String,
    ) -> Result<(), String> {
        if slot == GROUP_SLOT || self.slots.contains_key(&slot) {
            return Err(format!("slot {slot} cannot be attached"));
        }
        if self.slots.len() >= MAX_GROUP_INSTANCES {
            return Err(format!(
                "a group holds at most {MAX_GROUP_INSTANCES} instances"
            ));
        }
        let block_path = PathBuf::from(block);
        let shared = SharedBlock::open(&block_path)
            .map_err(|e| format!("cannot open the plugin block {}: {e}", block_path.display()))?;
        let bell = Doorbell::open(&DoorbellName(doorbell.clone()), &shared)
            .map_err(|e| format!("cannot open the doorbell {doorbell}: {e}"))?;
        let audio = AudioThread::spawn(ChildPort::new(shared, bell))
            .map_err(|e| format!("cannot start the audio thread: {e}"))?;
        let args = crate::args::with_format_override(
            Args {
                format: self.format,
                plugin_file: PathBuf::from(plugin_file),
                plugin_id,
                block: block_path,
                doorbell,
                parent_pid: self.parent_pid,
            },
            self.format_override.as_deref(),
        );
        let backend = control::make_backend(&args);
        let writer = SlotWriter::new(slot, self.out.clone());
        let mut control = Control::new(&args, backend, audio, Box::new(writer));
        if !control.greet() {
            return Err("the control channel is closed".to_string());
        }
        self.slots.insert(slot, control);
        Ok(())
    }

    /// Ends the instance of `slot` and tells the parent.
    fn detach(&mut self, slot: u32, systems: &mut dyn SlotSystems) {
        if let Some(mut control) = self.slots.remove(&slot) {
            control.shutdown(systems.select(slot));
            drop(control);
            systems.forget(slot);
            self.say(&FromGroup::Detached { slot });
        }
    }
}

/// Reads slot frames from stdin until it ends, then posts `Eof`.
pub fn spawn_reader<P: PostGroup>(post: P) {
    let started = std::thread::Builder::new()
        .name("host-stdin".to_string())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            loop {
                let (slot, inner) = match read_slot_frame(&mut stdin) {
                    Ok(frame) => frame,
                    Err(e) => {
                        if e.kind() != ErrorKind::UnexpectedEof {
                            eprintln!("atomik-plugin-host: the control channel broke: {e}");
                        }
                        post.post(GroupEvent::Eof);
                        // As the ungrouped child: when the main thread is stuck inside a plugin
                        // call it cannot end the process, and this does.
                        exit_after(crate::driver::EOF_GRACE);
                        return;
                    }
                };
                let event = if slot == GROUP_SLOT {
                    match read_frame::<ToGroup>(&mut Cursor::new(inner)) {
                        Ok((msg, _)) => GroupEvent::Attach(Box::new(msg)),
                        Err(e) => {
                            eprintln!("atomik-plugin-host: a group message is unreadable: {e}");
                            continue;
                        }
                    }
                } else {
                    match read_frame::<ToChild>(&mut Cursor::new(inner)) {
                        Ok(frame) => GroupEvent::Frame {
                            slot,
                            frame: Box::new(frame),
                        },
                        Err(e) => GroupEvent::Broken {
                            slot,
                            text: e.to_string(),
                        },
                    }
                };
                if !post.post(event) {
                    return;
                }
            }
        });
    if let Err(e) = started {
        eprintln!("atomik-plugin-host: cannot start the stdin thread: {e}");
    }
}

/// Posts a tick every [`crate::driver::TICK`], never more than one that has not been handled.
pub fn spawn_ticker<P: PostGroup>(post: P, pending: Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    let started = std::thread::Builder::new()
        .name("host-ticker".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(crate::driver::TICK);
                if !pending.swap(true, Ordering::AcqRel) && !post.post(GroupEvent::Tick) {
                    return;
                }
            }
        });
    if let Err(e) = started {
        eprintln!("atomik-plugin-host: cannot start the ticker thread: {e}");
    }
}

/// Runs the plain loop: no windows. Returns the exit code.
pub fn run_plain(mut host: GroupHost) -> u8 {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (tx, rx) = mpsc::channel();
    let pending = Arc::new(AtomicBool::new(false));
    spawn_reader(tx.clone());
    spawn_ticker(tx, Arc::clone(&pending));
    let mut systems = NoSlotWindows::default();
    while let Ok(event) = rx.recv() {
        if matches!(event, GroupEvent::Tick) {
            pending.store(false, Ordering::Release);
        }
        if let Some(code) = host.on_event(event, &mut systems) {
            return code;
        }
    }
    0
}

/// Runs the group child: the winit loop when the system has a display, the plain loop otherwise.
pub fn run(host: GroupHost) -> u8 {
    mark_main_thread();
    match crate::winit_driver::run_group(host) {
        Ok(code) => code,
        Err(host) => run_plain(*host),
    }
}
