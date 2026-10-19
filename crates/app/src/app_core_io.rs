//! The Audio I/O panel's data in the app core:
//! the host device's channel counts and the per-frame peaks of every socket.
//!
//! **Inputs** are metered at the interface's own `in_k` output endpoints (the engine can meter
//! any audio output endpoint, host inputs included). **Outputs** `out_k` are audio *inputs* of
//! the interface (summing) and cannot be metered by endpoint, so the peak shown is the largest
//! peak of the *source* endpoints of the cables that end there (the master's `main`, a
//! channel's `out`...); none means silence. This is the variant that needs no tap
//! inside the engine batch loop.
//!
//! A meter read resets its peak, so an endpoint the console also meters (the master's `main`
//! outputs, a channel that goes straight to an output) is read **once** per frame and the
//! reading is handed to the console feed of the same frame (`IoMeters::shared`).

use super::*;
use atomik_engine::MeterReading;

/// One metered endpoint of the Audio I/O panel.
#[derive(Debug, Clone, Copy)]
struct IoTap {
    endpoint: Endpoint,
    /// Also an endpoint of a console strip meter: its reading goes to the console too.
    shared: bool,
}

/// The metered endpoints of the Audio I/O sockets and the per-frame buffers (allocated when the
/// routing is submitted, never per frame).
#[derive(Debug, Default)]
pub(super) struct IoMeters {
    taps: Vec<IoTap>,
    /// Per socket (inputs, then outputs), the indices into `taps` that feed it.
    sockets: Vec<Vec<usize>>,
    /// This frame's peak per tap.
    reads: Vec<f32>,
    /// This frame's peak per socket.
    peaks: Vec<f32>,
    /// This frame's readings of the endpoints the console meters too.
    pub(super) shared: Vec<(Endpoint, MeterReading)>,
}

/// Adds `e` to the metered set of `topology` and to the taps of `io` once; its index. `None`
/// when the topology refuses it (not an audio output): such a source reads silence.
fn tap_of(
    topology: &mut Topology,
    io: &mut IoMeters,
    console_points: &[(DeviceKey, [Endpoint; 3])],
    e: Endpoint,
) -> Option<usize> {
    topology.add_meter(e).ok()?;
    if let Some(i) = io.taps.iter().position(|t| t.endpoint == e) {
        return Some(i);
    }
    let shared = console_points.iter().any(|(_, ends)| ends.contains(&e));
    io.taps.push(IoTap {
        endpoint: e,
        shared,
    });
    Some(io.taps.len() - 1)
}

impl<S: EngineSink> AppCore<S> {
    /// Plans the Audio I/O meters for `topology` (the routing being submitted) and registers
    /// them as read-only meters. Called by `submit_routing_with` after the console's.
    pub(super) fn plan_io_meters(
        &self,
        topology: &mut Topology,
        expanded: &ExpandedRack,
        console_points: &[(DeviceKey, [Endpoint; 3])],
    ) -> IoMeters {
        let doc = self.document.doc();
        let cap = atomik_ui::interface::MAX_OUTPUTS;
        let n_in = usize::from(doc.input_channels).min(cap);
        let n_out = usize::from(doc.output_channels).min(cap);
        let mut io = IoMeters::default();
        let interface = |socket: String| SocketRef {
            device: DeviceKey(INTERFACE_KEY.to_string()),
            socket,
        };
        for k in 1..=n_in {
            let tap = self
                .endpoint(topology, &interface(format!("in_{k}")))
                .and_then(|e| tap_of(topology, &mut io, console_points, e));
            io.sockets.push(tap.into_iter().collect());
        }
        for k in 1..=n_out {
            let socket = format!("out_{k}");
            let mut taps = Vec::new();
            for cable in &expanded.cables {
                if cable.to.device.as_str() != INTERFACE_KEY || cable.to.socket != socket {
                    continue;
                }
                let tap = self
                    .endpoint(topology, &cable.from)
                    .and_then(|e| tap_of(topology, &mut io, console_points, e));
                if let Some(t) = tap
                    && !taps.contains(&t)
                {
                    taps.push(t);
                }
            }
            io.sockets.push(taps);
        }
        io.reads = vec![0.0; io.taps.len()];
        io.peaks = vec![0.0; io.sockets.len()];
        io.shared = Vec::with_capacity(io.taps.len());
        io
    }

    /// Tells the rack view how many channels the host audio device has (`None`: the offline
    /// sink, every channel exists). Cheap when nothing changed; called every frame and after
    /// every engine start, so a changed stream is picked up.
    pub(super) fn sync_io_host(&mut self) {
        let host = self.sink.host_channels();
        self.view.set_io_host_channels(host);
    }

    /// Feeds the Audio I/O panel this frame's peaks: reads each metered endpoint once, takes
    /// per socket the largest peak of its endpoints, and hands the result to the rack view.
    /// Allocation-free after the routing was planned. Call once per frame, before
    /// [`Self::update_console_meters`] (which gets the readings both need).
    pub fn feed_io_meters(&mut self) {
        let io = &mut self.io_meters;
        if io.sockets.is_empty() {
            return;
        }
        io.shared.clear();
        for (read, tap) in io.reads.iter_mut().zip(&io.taps) {
            let reading = self.sink.read_meter(tap.endpoint);
            *read = reading.map_or(0.0, |r| {
                if r.peak.is_nan() {
                    0.0
                } else {
                    r.peak.max(0.0)
                }
            });
            if let (true, Some(r)) = (tap.shared, reading) {
                io.shared.push((tap.endpoint, r));
            }
        }
        for (peak, taps) in io.peaks.iter_mut().zip(&io.sockets) {
            *peak = taps.iter().map(|&t| io.reads[t]).fold(0.0, f32::max);
        }
        self.view.set_io_peaks(&io.peaks);
    }

    /// The console strips' readings; a reading in `shared` (spent by [`Self::feed_io_meters`]
    /// this frame) wins over a new read, which would only see what came after.
    pub(super) fn console_readings_with(
        &self,
        shared: &[(Endpoint, MeterReading)],
    ) -> Vec<(DeviceKey, ChannelMeters)> {
        let read = |e: &Endpoint| {
            shared
                .iter()
                .find(|(x, _)| x == e)
                .map(|(_, r)| *r)
                .or_else(|| self.sink.read_meter(*e))
        };
        self.meter_points
            .iter()
            .map(|(k, [l, r, g])| (k.clone(), channel_meters(read(l), read(r), read(g))))
            .collect()
    }
}
