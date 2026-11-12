//! `vision-demo`: the hand tracking engine from the command line, for hand checks with a real
//! camera.
//!
//! ```text
//! vision-demo cameras                      list the cameras
//! vision-demo models [--download]          the state of the hand models; fetch them after a y/N prompt
//! vision-demo track [--camera N]           print the state of the hand ten times a second
//! vision-demo record out.json --seconds 10  save the landmarks (no images) for test fixtures
//! ```
//!
//! The camera is on only while `track` or `record` runs. Frames are kept in memory only.

// A command-line tool prints to the terminal and reads the wall clock for its own timing.
#![allow(clippy::print_stdout, clippy::print_stderr, clippy::disallowed_methods)]

use atomik_vision::Observation;
use atomik_vision::camera::{Camera, CameraId, SystemCamera};
use atomik_vision::download::{ModelStore, Progress, UreqFetcher};
use atomik_vision::gesture::{GestureEvent, HandState};
use atomik_vision::models::{
    FrameHands, FrameView, HandFinder, HandModels, HandPipeline, ModelError, PipelineConfig,
};
use atomik_vision::tracker::{FinderFactory, Tracker, TrackerConfig, TrackerEvent};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(
    name = "vision-demo",
    about = "Hand tracking from the command line (hand checks for the webcam controller)"
)]
struct Cli {
    /// The folder that holds `models/hands/` (default: the folder of the application's settings).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the cameras.
    Cameras,
    /// Show the state of the hand model files, or download them.
    Models {
        /// Download the files after a y/N question.
        #[arg(long)]
        download: bool,
    },
    /// Print the state of the hand: `hand=R pinch=on twist=+23.4° size=0.31 fps=30 infer=12ms`.
    Track {
        /// The camera, as numbered by `cameras`.
        #[arg(long, default_value_t = 0)]
        camera: usize,
        /// How many hands to follow (1 or 2).
        #[arg(long, default_value_t = 1)]
        hands: usize,
        /// Stop after this many seconds (default: until Ctrl+C).
        #[arg(long)]
        seconds: Option<f64>,
    },
    /// Save the landmark stream (no images) as JSON.
    Record {
        /// The file to write.
        out: PathBuf,
        /// How long to record.
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
        /// The camera, as numbered by `cameras`.
        #[arg(long, default_value_t = 0)]
        camera: usize,
        /// How many hands to follow (1 or 2).
        #[arg(long, default_value_t = 1)]
        hands: usize,
    },
}

/// The folder of the application's settings, where the models go (the same on every run).
fn default_data_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        var("APPDATA").map(|d| d.join("Atomik Rack"))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Application Support/Atomik Rack"))
    } else {
        var("XDG_CONFIG_HOME")
            .map(|d| d.join("atomik-rack"))
            .or_else(|| var("HOME").map(|h| h.join(".config/atomik-rack")))
    }
}

fn store(cli: &Cli) -> Result<ModelStore, String> {
    let dir = cli
        .data_dir
        .clone()
        .or_else(default_data_dir)
        .ok_or("no data folder: pass --data-dir")?;
    Ok(ModelStore::in_data_dir(&dir))
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

fn cameras(system: &SystemCamera) -> Result<Vec<atomik_vision::camera::CameraInfo>, String> {
    system.list().map_err(|e| e.to_string())
}

fn pick_camera(system: &SystemCamera, n: usize) -> Result<CameraId, String> {
    let list = cameras(system)?;
    list.get(n)
        .map(|c| c.id.clone())
        .ok_or_else(|| format!("no camera number {n} (there are {})", list.len()))
}

fn models(cli: &Cli, download: bool) -> Result<(), String> {
    let store = store(cli)?;
    println!("folder: {}", store.dir().display());
    println!("state: {:?}", store.status());
    for f in store.files() {
        println!("  {}  {}  {}", f.name, megabytes(f.size), f.url);
    }
    println!("licence: Apache-2.0 (Google, MediaPipe hand models)");
    if !download {
        return Ok(());
    }
    println!();
    println!(
        "This downloads {} from storage.googleapis.com, once. Nothing about you or this computer is sent.",
        megabytes(store.total_bytes())
    );
    print!("Download now? [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    if !answer.trim().eq_ignore_ascii_case("y") {
        println!("Not downloaded.");
        return Ok(());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let c = Arc::clone(&cancel);
    let _ = ctrlc::set_handler(move || c.store(true, Ordering::SeqCst));
    let mut report = |p: Progress| {
        print!(
            "\r  {} of {} ({} of {} files)   ",
            megabytes(p.received),
            megabytes(p.total),
            p.file + 1,
            p.files
        );
        let _ = std::io::stdout().flush();
    };
    let result = store.download(&UreqFetcher, &mut report, &cancel);
    println!();
    result.map_err(|e| e.to_string())?;
    println!("Ready: {:?}", store.status());
    Ok(())
}

/// The real hand finder behind a tap that keeps every observation.
struct Tee {
    inner: HandPipeline,
    seen: Arc<Mutex<Vec<Observation>>>,
    start: Instant,
}

impl HandFinder for Tee {
    fn process(&mut self, frame: FrameView<'_>) -> Result<FrameHands, ModelError> {
        let found = self.inner.process(frame)?;
        let obs = Observation {
            t: self.start.elapsed().as_secs_f64(),
            aspect: frame.width as f32 / frame.height.max(1) as f32,
            hands: found.hands.clone(),
        };
        if let Ok(mut v) = self.seen.lock() {
            v.push(obs);
        }
        Ok(found)
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

fn factory(
    cli: &Cli,
    hands: usize,
    tap: Option<Arc<Mutex<Vec<Observation>>>>,
) -> Result<FinderFactory, String> {
    let store = store(cli)?;
    let paths = store
        .verified_paths()
        .map_err(|e| format!("{e} (run `vision-demo models --download`)"))?;
    let dir = paths[0]
        .parent()
        .map(PathBuf::from)
        .ok_or("bad model folder")?;
    let cfg = PipelineConfig {
        max_hands: hands.clamp(1, 2),
        ..PipelineConfig::default()
    };
    Ok(Box::new(move || {
        let pipeline = HandPipeline::new(HandModels::load(&dir)?, cfg);
        Ok(match tap {
            Some(seen) => Box::new(Tee {
                inner: pipeline,
                seen,
                start: Instant::now(),
            }) as Box<dyn HandFinder>,
            None => Box::new(pipeline) as Box<dyn HandFinder>,
        })
    }))
}

fn describe(hands: &[HandState], fps: f32, infer_ms: f32) -> String {
    let mut parts: Vec<String> = hands
        .iter()
        .filter(|h| h.present)
        .map(|h| {
            format!(
                "hand={} pinch={} twist={:+.1}\u{b0} size={:.2}",
                h.handedness.letter(),
                if h.pinch { "on" } else { "off" },
                h.twist_angle.to_degrees(),
                h.hand_size
            )
        })
        .collect();
    if parts.is_empty() {
        parts.push("hand=- pinch=off twist=+0.0\u{b0} size=0.00".to_string());
    }
    format!(
        "{} fps={:.0} infer={:.0}ms",
        parts.join(" | "),
        fps,
        infer_ms
    )
}

fn run(
    cli: &Cli,
    camera: usize,
    hands: usize,
    seconds: Option<f64>,
    tap: Option<Arc<Mutex<Vec<Observation>>>>,
    quiet: bool,
) -> Result<(), String> {
    let system = SystemCamera;
    let id = pick_camera(&system, camera)?;
    let make = factory(cli, hands, tap)?;
    let _ = ctrlc::set_handler(|| STOP.store(true, Ordering::SeqCst));
    println!(
        "camera on: {} (Ctrl+C to stop)",
        cameras(&system)?
            .get(camera)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    );
    let tracker = Tracker::start(Arc::new(system), id, make, TrackerConfig::default());
    let events = tracker.events();
    let start = Instant::now();
    let mut last_print = Duration::ZERO;
    let mut result = Ok(());
    while !STOP.load(Ordering::SeqCst) && seconds.is_none_or(|s| start.elapsed().as_secs_f64() < s)
    {
        match events.recv_timeout(Duration::from_millis(100)) {
            Some(TrackerEvent::Frame(r)) => {
                if !quiet && start.elapsed() - last_print >= Duration::from_millis(100) {
                    last_print = start.elapsed();
                    println!("{}", describe(&r.hands, r.fps, r.infer_ms));
                }
            }
            Some(TrackerEvent::Gesture(GestureEvent::PinchStart { .. })) if !quiet => {
                println!("-- pinch start")
            }
            Some(TrackerEvent::Gesture(GestureEvent::PinchEnd { .. })) if !quiet => {
                println!("-- pinch end")
            }
            Some(TrackerEvent::Failed(m)) => {
                result = Err(m);
                break;
            }
            Some(TrackerEvent::Stopped) => break,
            _ => {}
        }
    }
    drop(tracker);
    println!("camera off");
    result
}

fn main() {
    let cli = Cli::parse();
    let result = match &cli.command {
        Cmd::Cameras => cameras(&SystemCamera).map(|list| {
            if list.is_empty() {
                println!("no camera found");
            }
            for (i, c) in list.iter().enumerate() {
                println!("{i}: {} ({})", c.name, c.description);
            }
        }),
        Cmd::Models { download } => models(&cli, *download),
        Cmd::Track {
            camera,
            hands,
            seconds,
        } => run(&cli, *camera, *hands, *seconds, None, false),
        Cmd::Record {
            out,
            seconds,
            camera,
            hands,
        } => {
            let seen = Arc::new(Mutex::new(Vec::new()));
            run(
                &cli,
                *camera,
                *hands,
                Some(*seconds),
                Some(Arc::clone(&seen)),
                true,
            )
            .and_then(|()| {
                let frames = seen.lock().map_err(|_| "poisoned".to_string())?;
                let text = serde_json::to_string(&*frames).map_err(|e| e.to_string())?;
                std::fs::write(out, text + "\n").map_err(|e| e.to_string())?;
                println!(
                    "wrote {} frames of landmarks (no images) to {}",
                    frames.len(),
                    out.display()
                );
                Ok(())
            })
        }
    };
    if let Err(message) = result {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}
