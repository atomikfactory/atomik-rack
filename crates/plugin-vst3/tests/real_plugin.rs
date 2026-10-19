//! Loads a real, third-party VST3 plugin through `Vst3Backend` and `PluginManager` the way the
//! app does (the real-plugin fix). Not part of CI: it needs a plugin on this machine.
//!
//! ```text
//! ATOMIK_REAL_VST3="<path to a .vst3 file or bundle>" \
//!   cargo test -p atomik-plugin-vst3 --test real_plugin -- --ignored --nocapture
//! ```
//!
//! Optional: `ATOMIK_REAL_ID=<32 hex>` skips the in-process scan and registers the file the way the
//! app does from its scan cache (first class when unset), `ATOMIK_REAL_OLE=1` initialises OLE on
//! the main thread first (what winit does on Windows). The plugin runs in this test process:
//! run it under a timeout. It never writes to the plugin folders.

#![allow(clippy::disallowed_methods)] // Justification: a manual harness for real plugins; timing is its output.
#![allow(clippy::float_cmp)] // Justification: the meter state is a small whole number and silence is exact zero.

use atomik_core::{
    BATCH, Batch, MAX_EVENTS_PER_BATCH, NoteBuffer, NoteEvent, NoteKind, SampleRate, Tempo,
    TimeInfo,
};
use atomik_device_sdk::{
    Device, Direction, PluginFormat, PluginRef, PrepareInfo, ProcessContext, SocketKind,
};
use atomik_plugin_host::{InstantiateRequest, PluginManager, mark_main_thread};
use atomik_plugin_vst3::Vst3Backend;
use std::path::PathBuf;
use std::time::Instant;

/// A device driver like the test kit's harness, without its allocation guard (a real plugin
/// allocates when it likes).
struct Rig {
    device: Box<dyn Device>,
    signals: Vec<Batch>,
    notes: Vec<NoteBuffer>,
    connected: Vec<bool>,
    time: TimeInfo,
}

impl Rig {
    fn new(mut device: Box<dyn Device>, rate: f64) -> Self {
        let sr = SampleRate::new(rate).unwrap();
        device.prepare(&PrepareInfo {
            sample_rate: sr,
            seed: 0,
        });
        let sockets = device.descriptor().sockets;
        Self {
            signals: vec![[0.0; BATCH]; sockets.len()],
            notes: sockets
                .iter()
                .map(|s| {
                    NoteBuffer::with_capacity(if s.kind == SocketKind::Notes {
                        MAX_EVENTS_PER_BATCH
                    } else {
                        0
                    })
                })
                .collect(),
            connected: sockets
                .iter()
                .map(|s| s.direction == Direction::Output)
                .collect(),
            time: TimeInfo::at(0, sr, Tempo::new(120.0).unwrap(), true),
            device,
        }
    }

    fn push_note(&mut self, key: &str, e: NoteEvent) {
        let id = self.device.descriptor().socket_by_key(key).unwrap().id.0 as usize;
        self.connected[id] = true;
        self.notes[id].push(e).unwrap();
    }

    fn output(&self, key: &str) -> &Batch {
        let id = self.device.descriptor().socket_by_key(key).unwrap().id.0 as usize;
        &self.signals[id]
    }

    fn process(&mut self) {
        let sockets = self.device.descriptor().sockets;
        for (i, s) in sockets.iter().enumerate() {
            if s.direction == Direction::Output {
                self.signals[i] = [0.0; BATCH];
                self.notes[i].clear();
            }
        }
        let mut ctx = ProcessContext::new(
            self.time,
            &mut self.signals,
            &mut self.notes,
            &self.connected,
        );
        self.device.process(&mut ctx);
        for (i, s) in sockets.iter().enumerate() {
            if s.direction.is_input() {
                self.notes[i].clear();
            }
        }
        self.time.frame += BATCH as u64;
        self.time.tick += BATCH as f64 * self.time.ticks_per_frame;
        self.time.loop_wrap = None;
    }
}

fn step(t: &Instant, what: &str) {
    println!("[{:>8.3}s] {what}", t.elapsed().as_secs_f64());
}

#[cfg(windows)]
fn ole_init() {
    // SAFETY: the declarations match the Win32 API.
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn OleInitialize(reserved: *mut std::ffi::c_void) -> i32;
    }
    // SAFETY: a plain COM initialisation call on the thread that will host the plugin.
    let r = unsafe { OleInitialize(std::ptr::null_mut()) };
    println!("OleInitialize -> {r:#x}");
}

#[cfg(not(windows))]
fn ole_init() {}

#[test]
#[ignore = "needs ATOMIK_REAL_VST3 (a real plugin on this machine)"]
fn a_real_plugin_loads_activates_processes_and_keeps_its_state() {
    let Some(path) = std::env::var_os("ATOMIK_REAL_VST3").map(PathBuf::from) else {
        println!("ATOMIK_REAL_VST3 is not set; nothing to do");
        return;
    };
    let t = Instant::now();
    mark_main_thread();
    if std::env::var_os("ATOMIK_REAL_OLE").is_some() {
        ole_init();
    }
    let backend = Vst3Backend::new();
    use atomik_plugin_host::PluginBackend;
    let id = if let Ok(id) = std::env::var("ATOMIK_REAL_ID") {
        backend.register_plugin_file(&id, &path);
        step(&t, &format!("registered {id} without loading"));
        id
    } else {
        step(&t, "scan_file ...");
        let found = backend.scan_file(&path).expect("scans");
        for f in &found {
            println!(
                "  class {} `{}` by `{}` {} unsupported={:?} params={} truncated={} in={:?} out={:?} notes_in={}",
                f.plugin.id,
                f.layout.name,
                f.layout.vendor,
                f.layout.version,
                f.unsupported,
                f.layout.params.len(),
                f.params_truncated,
                f.layout.main_in,
                f.layout.main_out,
                f.layout.notes_in
            );
        }
        step(&t, "scan_file done");
        found[0].plugin.id.clone()
    };
    let plugin = PluginRef {
        format: PluginFormat::Vst3,
        id,
    };
    let mut manager = PluginManager::new();
    manager.add_backend(Box::new(backend));
    step(&t, "instantiate ...");
    let handle = manager
        .instantiate(&InstantiateRequest {
            plugin: plugin.clone(),
            state: None,
            sample_rate: 48_000.0,
            mode: atomik_device_sdk::RunMode::Realtime,
            pinned: false,
            powered: true,
        })
        .expect("instantiates");
    step(&t, "instantiate done");
    let layout = handle.layout.clone();
    println!(
        "layout `{}` params={} has_gui={} latency={:?}",
        layout.name,
        layout.params.len(),
        manager.has_gui(handle.id),
        manager.latency(handle.id)
    );
    let sockets = handle.descriptor.sockets;
    for s in sockets {
        println!("  socket {} {:?} {:?}", s.key, s.kind, s.direction);
    }
    let notes_key = sockets
        .iter()
        .find(|s| s.kind == SocketKind::Notes && s.direction == Direction::Input)
        .map(|s| s.key);
    let outs: Vec<&str> = sockets
        .iter()
        .filter(|s| s.kind != SocketKind::Notes && s.direction == Direction::Output)
        .map(|s| s.key)
        .collect();
    let id = handle.id;
    let mut h = Rig::new(Box::new(handle.device), 48_000.0);
    for o in &outs {
        let _ = o;
    }
    step(&t, "processing silence");
    let mut peak = 0.0f32;
    for _ in 0..200 {
        h.process();
        manager.poll();
    }
    for o in &outs {
        peak = peak.max(h.output(o).iter().fold(0.0, |m, s| m.max(s.abs())));
    }
    println!("peak after silence = {peak}");
    if let Some(nk) = notes_key {
        step(&t, "note on");
        h.push_note(
            nk,
            NoteEvent {
                offset: 0,
                kind: NoteKind::On {
                    key: 60,
                    velocity: 0.8,
                },
            },
        );
        let mut loud = 0.0f32;
        for i in 0..600 {
            h.process();
            if i % 50 == 0 {
                manager.poll();
            }
            for o in &outs {
                loud = loud.max(h.output(o).iter().fold(0.0, |m, s| m.max(s.abs())));
            }
            // The plugin may need a few blocks (voice start, thread hand-over).
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
        println!("peak with a held note = {loud} (blocks of {BATCH})");
    } else {
        println!("no note input; fed silence only");
    }
    step(&t, "capture state");
    let state = manager.capture_state(id).expect("captures");
    println!("state = {} bytes", state.0.len());
    manager.restore_state(id, &state).expect("restores");
    step(&t, "restore done");
    for p in layout.params.iter().take(3) {
        println!("  param {} `{}`", p.id, p.name);
    }
    #[cfg(windows)]
    {
        // The editor needs a window; open it on a hidden top-level popup of our own.
        if manager.has_gui(id) {
            step(&t, "open editor on a message-less hidden window");
            if let Some(hwnd) = hidden_window() {
                match manager.open_gui(
                    id,
                    atomik_plugin_host::NativeParent::Win32(hwnd as usize),
                    1.0,
                ) {
                    Ok(size) => println!("editor opened {size:?}"),
                    Err(e) => println!("editor failed: {e}"),
                }
                for _ in 0..20 {
                    manager.poll();
                    pump();
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                let _ = manager.close_gui(id);
                step(&t, "editor closed");
            }
        }
    }
    drop(h);
    for _ in 0..4 {
        manager.poll();
    }
    step(&t, "done");
    drop(manager);
}

#[cfg(windows)]
fn hidden_window() -> Option<isize> {
    use std::ffi::c_void;
    // SAFETY: the declarations match the Win32 API.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn CreateWindowExW(
            ex: u32,
            class: *const u16,
            name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: isize,
            menu: isize,
            inst: isize,
            param: *mut c_void,
        ) -> isize;
    }
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let name: Vec<u16> = "atomik editor host\0".encode_utf16().collect();
    // WS_POPUP | WS_VISIBLE at a small size; a STATIC control stands in for the winit window.
    // SAFETY: valid NUL-terminated strings and null parent/menu/instance for a top-level window.
    let h = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            name.as_ptr(),
            0x8000_0000 | 0x1000_0000,
            0,
            0,
            800,
            600,
            0,
            0,
            0,
            std::ptr::null_mut(),
        )
    };
    (h != 0).then_some(h)
}

#[cfg(windows)]
fn pump() {
    #[repr(C)]
    struct Msg {
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        x: i32,
        y: i32,
    }
    // SAFETY: the declarations match the Win32 API.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn PeekMessageW(m: *mut Msg, h: isize, min: u32, max: u32, remove: u32) -> i32;
        fn TranslateMessage(m: *const Msg) -> i32;
        fn DispatchMessageW(m: *const Msg) -> isize;
    }
    let mut m = Msg {
        hwnd: 0,
        message: 0,
        wparam: 0,
        lparam: 0,
        time: 0,
        x: 0,
        y: 0,
    };
    // SAFETY: standard message pump on the thread that owns the window.
    unsafe {
        while PeekMessageW(&mut m, 0, 0, 0, 1) != 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

// ----- the live path: 128-frame callbacks, note-on, hold, note-off, silence, note-on ----------

/// One plugin loaded the way the app loads it, behind a [`Rig`].
struct Live {
    manager: PluginManager,
    rig: Rig,
    notes_key: &'static str,
    outs: Vec<&'static str>,
}

fn open_live(path: &std::path::Path, rate: f64) -> Live {
    use atomik_plugin_host::PluginBackend;
    mark_main_thread();
    if std::env::var_os("ATOMIK_REAL_OLE").is_some() {
        ole_init();
    }
    let backend = Vst3Backend::new();
    let id = if let Ok(id) = std::env::var("ATOMIK_REAL_ID") {
        backend.register_plugin_file(&id, path);
        id
    } else {
        backend.scan_file(path).expect("scans")[0].plugin.id.clone()
    };
    let mut manager = PluginManager::new();
    manager.add_backend(Box::new(backend));
    let handle = manager
        .instantiate(&InstantiateRequest {
            plugin: PluginRef {
                format: PluginFormat::Vst3,
                id,
            },
            state: None,
            sample_rate: rate,
            mode: atomik_device_sdk::RunMode::Realtime,
            pinned: false,
            powered: true,
        })
        .expect("instantiates");
    let sockets = handle.descriptor.sockets;
    let notes_key = sockets
        .iter()
        .find(|s| s.kind == SocketKind::Notes && s.direction == Direction::Input)
        .expect("an instrument")
        .key;
    let outs = sockets
        .iter()
        .filter(|s| s.kind != SocketKind::Notes && s.direction == Direction::Output)
        .map(|s| s.key)
        .collect();
    Live {
        rig: Rig::new(Box::new(handle.device), rate),
        manager,
        notes_key,
        outs,
    }
}

/// What one phase of the live sequence did.
#[derive(Default)]
struct Phase {
    peak: f32,
    /// Callbacks (128 frames) whose output was entirely silent.
    silent_callbacks: usize,
    callbacks: usize,
    sleeping_batches: usize,
    /// Microseconds per 64-frame batch.
    batch_us: Vec<f64>,
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v.get(((v.len() as f64 - 1.0) * p) as usize)
        .copied()
        .unwrap_or(0.0)
}

impl Live {
    /// Runs `callbacks` audio callbacks of 128 frames (two device batches each), optionally
    /// paced in real time, with `first` pushed before the first batch.
    fn run(&mut self, callbacks: usize, first: Option<NoteEvent>, paced: bool) -> Phase {
        let mut ph = Phase::default();
        if let Some(e) = first {
            let key = self.notes_key;
            self.rig.push_note(key, e);
        }
        let t0 = Instant::now();
        for cb in 0..callbacks {
            let mut cb_peak = 0.0f32;
            for _ in 0..2 {
                let t = Instant::now();
                self.rig.process();
                ph.batch_us.push(t.elapsed().as_secs_f64() * 1e6);
                for o in &self.outs {
                    cb_peak = cb_peak.max(
                        self.rig
                            .output(o)
                            .iter()
                            .fold(0.0, |m: f32, s| m.max(s.abs())),
                    );
                }
                let mut m = [0.0f32; 5];
                self.rig.device.meter_values(&mut m);
                if m[2] == 1.0 {
                    ph.sleeping_batches += 1;
                }
            }
            ph.callbacks += 1;
            ph.peak = ph.peak.max(cb_peak);
            if cb_peak == 0.0 {
                ph.silent_callbacks += 1;
            }
            if cb % 64 == 0 {
                self.manager.poll();
            }
            if paced {
                let due = std::time::Duration::from_secs_f64((cb + 1) as f64 * 128.0 / 48_000.0);
                while t0.elapsed() < due {
                    std::hint::spin_loop();
                }
            }
        }
        ph
    }
}

fn report(name: &str, mut p: Phase) -> Phase {
    let mean = p.batch_us.iter().sum::<f64>() / p.batch_us.len().max(1) as f64;
    let p99 = percentile(&mut p.batch_us, 0.99);
    let max = percentile(&mut p.batch_us, 1.0);
    println!(
        "{name:<22} peak {:.4} silent callbacks {}/{} sleeping batches {} | batch us mean {:.1} p99 {:.1} max {:.1} (budget 1333)",
        p.peak, p.silent_callbacks, p.callbacks, p.sleeping_batches, mean, p99, max
    );
    p
}

/// `ATOMIK_REAL_VST3=<instrument>`; `ATOMIK_REAL_PACED=1` runs the callbacks in real time.
/// Asserts the live contract: sound in every callback of a held note (after the attack), a
/// note-on that wakes a sleeping plugin audibly. The numbers are the cost of the plugin per
/// 64-frame batch at 48 kHz (build with `--release` for the real cost).
#[test]
#[ignore = "needs ATOMIK_REAL_VST3 (a real instrument on this machine)"]
fn a_real_instrument_sounds_through_128_frame_callbacks_and_wakes_from_sleep() {
    let Some(path) = std::env::var_os("ATOMIK_REAL_VST3").map(PathBuf::from) else {
        return;
    };
    let paced = std::env::var_os("ATOMIK_REAL_PACED").is_some();
    let mut live = open_live(&path, 48_000.0);
    let on = |key| NoteEvent {
        offset: 0,
        kind: NoteKind::On { key, velocity: 0.8 },
    };
    let off = |key| NoteEvent {
        offset: 0,
        kind: NoteKind::Off { key },
    };
    report("idle 1 s", live.run(375, None, paced));
    let held = report("note on, held 2 s", live.run(750, Some(on(60)), paced));
    report("note off, 1.5 s", live.run(562, Some(off(60)), paced));
    report("silence 2 s", live.run(750, None, paced));
    let again = report("note on again, 1 s", live.run(375, Some(on(64)), paced));
    report("note off, 1.5 s", live.run(562, Some(off(64)), paced));
    assert!(held.peak > 0.001, "the held note is audible: {}", held.peak);
    assert!(
        held.silent_callbacks <= 8,
        "the held note has gaps: {} silent callbacks of 750",
        held.silent_callbacks
    );
    assert!(again.peak > 0.001, "the second note is audible");
    assert!(
        again.silent_callbacks <= 8,
        "the woken instrument has gaps: {}",
        again.silent_callbacks
    );
    drop(live.rig);
    for _ in 0..4 {
        live.manager.poll();
    }
}
