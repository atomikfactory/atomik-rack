//! The live-play lifecycle: build, engine, stream, MIDI, stats loop, stop.

use crate::args::Args;
use crate::backend::{Backend, LiveStream, MidiPort};
use crate::error::PlayError;
use crate::listing::host_name;
use crate::stats::{RunTotals, event_line};
use crate::target::{prepare, resolve_target};
use atomik_audio_io::StreamSpec;
use atomik_engine::{EngineOptions, FrameClock, WorkerConfig, new_engine_with_options};
use atomik_render::build::build;
use atomik_render::default_registry;
use atomik_render::patch::Patch;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Granularity of the main loop: Ctrl-C is noticed within one tick.
pub const TICK: Duration = Duration::from_millis(100);
/// Ticks per second (stats, garbage collection and `--duration` count whole seconds).
pub const TICKS_PER_SECOND: u32 = 10;
/// When the host did not accept the requested buffer size, ticks to wait for the first
/// callback to report the actual size (used as the MIDI latency).
pub const BUFFER_PROBE_TICKS: u32 = 10;

/// Plays `args.patch` until `stop` is set or `args.duration` seconds have elapsed, then returns
/// the run totals. `sleep` paces the loop (`std::thread::sleep` in the binary). Stats lines and
/// the summary go to `stdout`; info, warnings and the start banner to `stderr`.
///
/// Xrun events are drained every tick: with `--stats` each one is printed at once as an `xrun`
/// line. With `--log FILE`, the banner, every stats and xrun line and the summary are also
/// written to FILE (whether or not `--stats` is given). All file I/O happens here, on the
/// control thread.
pub fn play<B: Backend>(
    args: &Args,
    backend: &B,
    stop: &AtomicBool,
    sleep: &mut dyn FnMut(Duration),
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<RunTotals, PlayError> {
    // 0. The log file, before anything is opened.
    let mut report = Report::new(stdout, args)?;
    // 1. Build the patch (notes dropped; a probe note on the MIDI target resolves its ids).
    let path = args
        .patch
        .as_ref()
        .ok_or_else(|| PlayError::Args("a PATCH file is required".into()))?;
    let text = std::fs::read_to_string(path).map_err(|source| PlayError::Read {
        path: path.clone(),
        source,
    })?;
    let mut patch = Patch::from_json(&text)?;
    let registry = default_registry();
    let wants_target = args.midi.is_some() || args.to.is_some();
    let target = resolve_target(&patch, &registry, args.to.as_deref())?;
    if args.midi.is_some() && target.is_none() {
        return Err(crate::target::TargetError::NoNotesInput.into());
    }
    let target = target.filter(|_| wants_target);
    let prepared = prepare(&mut patch, args.rate, target.as_ref());
    for line in prepared.messages() {
        let _ = writeln!(stderr, "{line}");
    }
    let mut built = build(&patch, &registry)?;
    for w in std::mem::take(&mut built.warnings) {
        let _ = writeln!(stderr, "warning: {w}");
    }
    let target_ids = built.notes.first().map(|n| (n.device, n.socket));

    // 2. Engine with workers; take the live sender before submitting the plan.
    let options = EngineOptions {
        workers: WorkerConfig {
            // At most cores - 1: the audio thread keeps a core of its own.
            threads: atomik_audio_io::cap_workers_here(args.workers),
            // Workers render on the audio deadline too: same RT priority as the callback.
            on_thread_start: Some(atomik_audio_io::promote_worker_thread),
        },
        // Keep workers awake between callbacks (0 = the engine default).
        worker_linger_spins: args.worker_linger,
    };
    let (engine, mut handle) = new_engine_with_options(built.config, options);
    let live = handle.live_notes();
    // 3. Plan, clock, stream.
    handle.submit_plan(built.plan)?;
    let clock = Arc::new(FrameClock::new());
    let spec = StreamSpec {
        host: args.host.kind(),
        device: args.device.clone(),
        sample_rate: patch.sample_rate,
        buffer_frames: args.buffer,
        input_channels: 0,
        output_channels: patch.output_channels,
    };
    let stream = backend.start(&spec, engine, Arc::clone(&clock))?;
    let mut totals = RunTotals::default();

    // 4. MIDI, with a latency of one actual host buffer.
    let latency = buffer_frames(&stream, args.buffer, &mut totals, sleep);
    let midi = match (&args.midi, target_ids, live) {
        (Some(name), Some(ids), Some(sender)) => {
            let opened = backend.open_midi(
                name,
                ids,
                sender,
                Arc::clone(&clock),
                f64::from(patch.sample_rate),
                u64::from(latency),
            );
            match opened {
                Ok(port) => Some(port),
                Err(e) => {
                    drop(stream.stop());
                    return Err(e.into());
                }
            }
        }
        _ => None,
    };
    let mut text = Vec::new();
    banner(
        &mut text,
        args,
        &stream,
        latency,
        midi.as_ref(),
        target.as_ref(),
    );
    let _ = stderr.write_all(&text);
    report.log_only(&String::from_utf8_lossy(&text));

    // 5./6. Main loop: every second read telemetry, print stats, collect garbage.
    let mut seconds = 0u64;
    'run: loop {
        for _ in 0..TICKS_PER_SECOND {
            if stop.load(Ordering::SeqCst) {
                break 'run;
            }
            sleep(TICK);
            report.events(&stream);
        }
        seconds += 1;
        let midi_dropped = midi.as_ref().map_or(0, MidiPort::dropped);
        totals.record(
            stream.telemetry(),
            seconds,
            midi_dropped,
            handle.dropped_events(),
        );
        handle.collect_garbage();
        report.stat(&totals.line().to_string());
        report.flush();
        if args.duration.is_some_and(|d| seconds >= d) {
            break;
        }
    }

    // 7. Stop: close MIDI, stop the stream, drop the engine here (not on the audio thread).
    let midi_dropped = midi.as_ref().map_or(0, MidiPort::dropped);
    drop(midi);
    let telemetry = stream.telemetry();
    let xrun_events_dropped = telemetry.events_dropped;
    totals.record(telemetry, seconds, midi_dropped, handle.dropped_events());
    report.events(&stream);
    let stopped = stream.stop();
    if let Err(e) = &stopped {
        let _ = writeln!(stderr, "atomik-play: warning: {e}");
    }
    drop(stopped);
    handle.collect_garbage();
    if xrun_events_dropped > 0 {
        let warning = format!(
            "atomik-play: warning: {xrun_events_dropped} xrun event(s) were dropped (event channel full)"
        );
        let _ = writeln!(stderr, "{warning}");
        report.log_only(&format!(
            "{warning}
"
        ));
    }
    report.always(&totals.summary(args.duration));
    report.flush();
    Ok(totals)
}

/// Where run output goes: stdout (stats and xrun lines only with `--stats`) and the optional
/// `--log` file (everything).
struct Report<'a> {
    stdout: &'a mut dyn Write,
    stats: bool,
    log: Option<BufWriter<File>>,
}

impl<'a> Report<'a> {
    fn new(stdout: &'a mut dyn Write, args: &Args) -> Result<Self, PlayError> {
        let log = match &args.log {
            Some(path) => Some(BufWriter::new(File::create(path).map_err(|source| {
                PlayError::Log {
                    path: path.clone(),
                    source,
                }
            })?)),
            None => None,
        };
        Ok(Self {
            stdout,
            stats: args.stats,
            log,
        })
    }

    /// A `--stats` line: stdout with `--stats`, and the log.
    fn stat(&mut self, line: &str) {
        if self.stats {
            let _ = writeln!(self.stdout, "{line}");
        }
        self.log_line(line);
    }

    /// A line for stdout and the log, whatever the flags (the summary).
    fn always(&mut self, line: &str) {
        let _ = writeln!(self.stdout, "{line}");
        self.log_line(line);
    }

    /// Text for the log only (already newline-terminated).
    fn log_only(&mut self, text: &str) {
        if let Some(log) = &mut self.log {
            let _ = log.write_all(text.as_bytes());
        }
    }

    fn log_line(&mut self, line: &str) {
        if let Some(log) = &mut self.log {
            let _ = writeln!(log, "{line}");
        }
    }

    /// Drains the stream's xrun events and reports each one as an `xrun` line.
    fn events<S: LiveStream>(&mut self, stream: &S) {
        let events = stream.drain_events();
        for e in &events {
            self.stat(&event_line(e));
        }
        if !events.is_empty() {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let _ = self.stdout.flush();
        if let Some(log) = &mut self.log {
            let _ = log.flush();
        }
    }
}

/// The actual host buffer size: the fixed size the host accepted, else the size reported by
/// the first callbacks (waiting up to [`BUFFER_PROBE_TICKS`]), else `requested`.
fn buffer_frames<S: LiveStream>(
    stream: &S,
    requested: u32,
    totals: &mut RunTotals,
    sleep: &mut dyn FnMut(Duration),
) -> u32 {
    if let Some(accepted) = stream.info().requested_buffer_frames {
        return accepted;
    }
    for _ in 0..BUFFER_PROBE_TICKS {
        sleep(TICK);
        let t = stream.telemetry();
        if t.buffer_frames > 0 {
            // Keep the counters; the load window restarts at the next read anyway.
            totals.record(t, 0, 0, 0);
            return t.buffer_frames;
        }
    }
    requested
}

fn banner<S: LiveStream, M: MidiPort>(
    stderr: &mut dyn Write,
    args: &Args,
    stream: &S,
    latency: u32,
    midi: Option<&M>,
    target: Option<&crate::target::Target>,
) {
    let info = stream.info();
    let requested = info
        .requested_buffer_frames
        .map_or_else(|| "host default period".to_string(), |n| n.to_string());
    let _ = writeln!(
        stderr,
        "atomik-play: {} \"{}\" at {} Hz, {} ch {}, buffer {} (requested {}), workers {}",
        host_name(info.host),
        info.output_device,
        info.sample_rate,
        info.output_channels,
        info.sample_format,
        requested,
        args.buffer,
        args.workers,
    );
    if args.workers > 0 {
        let _ = writeln!(
            stderr,
            "atomik-play: worker linger {} spins{}",
            args.worker_linger,
            if args.worker_linger == 0 {
                " (engine default)"
            } else {
                ""
            }
        );
    }
    match (midi, target) {
        (Some(port), Some(target)) => {
            let _ = writeln!(
                stderr,
                "atomik-play: MIDI {} -> {target} (latency {latency} frames)",
                port.port_name()
            );
        }
        _ => {
            let _ = writeln!(stderr, "atomik-play: no --midi given; no live MIDI input");
        }
    }
    let until = args.duration.map_or_else(
        || "until Ctrl-C".to_string(),
        |d| format!("for {d} s (Ctrl-C stops early)"),
    );
    let _ = writeln!(stderr, "atomik-play: playing {until}");
}
