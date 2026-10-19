//! Reading what a VST3 plugin is: its factory classes, its buses (and negotiating the
//! arrangements the host drives), its parameters and units, its MIDI mapping and note
//! expressions, and turning that into the format-neutral [`PluginLayout`].
//!
//! The decisions are pure functions ([`unit_path`], [`kind_of`], [`roles`]) tested without a
//! plugin; the COM reading around them is exercised by the backend tests against the in-repo
//! VST3 test plugins.

use crate::util::{c_array_text, utf16_text};
use crate::values::Steps;
use atomik_device_sdk::{
    Channels, MAX_PLUGIN_PARAMS, PluginKindHint, PluginLayout, PluginParamInfo,
};
use std::ffi::c_void;
use vst3::ComPtr;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;

/// One audio effect class of a factory (a component class a host may create).
#[derive(Debug, Clone)]
pub(crate) struct ClassInfo {
    pub cid: TUID,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub sub_categories: String,
}

/// The category of the classes a host instantiates (`kVstAudioEffectClass`).
const AUDIO_CLASS: &str = "Audio Module Class";

/// The audio module classes of a factory, with vendor and version from `IPluginFactory2` when
/// the factory has it (otherwise the factory's own vendor).
pub(crate) fn audio_classes(factory: &ComPtr<IPluginFactory>) -> Vec<ClassInfo> {
    let mut factory_vendor = String::new();
    // SAFETY: a zeroed PFactoryInfo is valid; the call fills it.
    let mut finfo: PFactoryInfo = unsafe { std::mem::zeroed() };
    // SAFETY: a live factory and a valid out pointer.
    if unsafe { factory.getFactoryInfo(&mut finfo) } == kResultOk {
        factory_vendor = c_array_text(&finfo.vendor);
    }
    let f2 = factory.cast::<IPluginFactory2>();
    // SAFETY: a live factory.
    let count = unsafe { factory.countClasses() };
    let mut out = Vec::new();
    for i in 0..count {
        if let Some(f2) = &f2 {
            // SAFETY: a zeroed PClassInfo2 is valid; the call fills it.
            let mut info: PClassInfo2 = unsafe { std::mem::zeroed() };
            // SAFETY: a live factory and a valid out pointer.
            if unsafe { f2.getClassInfo2(i, &mut info) } != kResultOk {
                continue;
            }
            if c_array_text(&info.category) != AUDIO_CLASS {
                continue;
            }
            out.push(ClassInfo {
                cid: info.cid,
                name: c_array_text(&info.name),
                vendor: c_array_text(&info.vendor),
                version: c_array_text(&info.version),
                sub_categories: c_array_text(&info.subCategories),
            });
        } else {
            // SAFETY: a zeroed PClassInfo is valid; the call fills it.
            let mut info: PClassInfo = unsafe { std::mem::zeroed() };
            // SAFETY: a live factory and a valid out pointer.
            if unsafe { factory.getClassInfo(i, &mut info) } != kResultOk {
                continue;
            }
            if c_array_text(&info.category) != AUDIO_CLASS {
                continue;
            }
            out.push(ClassInfo {
                cid: info.cid,
                name: c_array_text(&info.name),
                vendor: factory_vendor.clone(),
                version: String::new(),
                sub_categories: String::new(),
            });
        }
    }
    for c in &mut out {
        if c.vendor.is_empty() {
            c.vendor = factory_vendor.clone();
        }
    }
    out
}

// ----- buses ---------------------------------------------------------------------------------

/// What a bus is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// The main bus.
    Main,
    /// The sidechain: the first aux input bus.
    Sidechain,
    /// Any other bus: deactivated (a further aux output, a second aux input).
    Unused,
}

/// One bus as the host drives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BusPlan {
    pub channels: u32,
    pub role: Role,
}

impl BusPlan {
    /// Whether the host activates this bus.
    pub(crate) fn active(&self) -> bool {
        self.role != Role::Unused
    }
}

/// The audio and event buses of a plugin and how the host drives them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AudioPlan {
    pub inputs: Vec<BusPlan>,
    pub outputs: Vec<BusPlan>,
    pub event_in: bool,
    pub event_out: bool,
}

impl AudioPlan {
    fn find(buses: &[BusPlan], role: Role) -> Option<(usize, BusPlan)> {
        buses
            .iter()
            .copied()
            .enumerate()
            .find(|(_, b)| b.role == role)
    }

    /// The main input bus (index, plan).
    pub(crate) fn main_in(&self) -> Option<(usize, BusPlan)> {
        Self::find(&self.inputs, Role::Main)
    }

    /// The sidechain bus.
    pub(crate) fn sidechain(&self) -> Option<(usize, BusPlan)> {
        Self::find(&self.inputs, Role::Sidechain)
    }

    /// The main output bus.
    pub(crate) fn main_out(&self) -> Option<(usize, BusPlan)> {
        Self::find(&self.outputs, Role::Main)
    }
}

/// Which input bus is the main one (the first flagged main, else the first), which is the
/// sidechain (the first other one) and the rest unused.
pub(crate) fn roles(is_main: &[bool]) -> Vec<Role> {
    let main = is_main
        .iter()
        .position(|&m| m)
        .or(if is_main.is_empty() { None } else { Some(0) });
    let mut side_taken = false;
    is_main
        .iter()
        .enumerate()
        .map(|(i, _)| {
            if Some(i) == main {
                Role::Main
            } else if !side_taken {
                side_taken = true;
                Role::Sidechain
            } else {
                Role::Unused
            }
        })
        .collect()
}

/// The channels of a speaker arrangement (one per speaker bit).
pub(crate) fn speaker_channels(arrangement: u64) -> u32 {
    arrangement.count_ones()
}

fn channels_of(n: u32, what: &str) -> Result<Channels, String> {
    match n {
        1 => Ok(Channels::Mono),
        2 => Ok(Channels::Stereo),
        _ => Err(format!(
            "unsupported {what} of {n} channels (mono or stereo only)"
        )),
    }
}

/// The SDK channel type of a plan's main bus.
pub(crate) fn main_channels(
    bus: Option<(usize, BusPlan)>,
    what: &str,
) -> Result<Option<Channels>, String> {
    bus.map(|(_, b)| channels_of(b.channels, what)).transpose()
}

const K_AUDIO: MediaType = MediaTypes_::kAudio as MediaType;
const K_EVENT: MediaType = MediaTypes_::kEvent as MediaType;
const K_INPUT: BusDirection = BusDirections_::kInput as BusDirection;
const K_OUTPUT: BusDirection = BusDirections_::kOutput as BusDirection;

fn bus_info(
    component: &ComPtr<IComponent>,
    media: MediaType,
    dir: BusDirection,
    i: i32,
) -> Option<BusInfo> {
    // SAFETY: a zeroed BusInfo is valid; the call fills it.
    let mut info: BusInfo = unsafe { std::mem::zeroed() };
    // SAFETY: a live component and a valid out pointer.
    (unsafe { component.getBusInfo(media, dir, i, &mut info) } == kResultOk).then_some(info)
}

/// Negotiates the bus arrangements: proposes stereo for the main buses and the
/// sidechain, accepts what the plugin answers when it is mono or stereo, activates the buses the
/// host drives and deactivates the others.
///
/// # Errors
/// The reason the plugin cannot be hosted.
pub(crate) fn negotiate(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
) -> Result<AudioPlan, String> {
    // SAFETY: a live component.
    let (n_in, n_out) = unsafe {
        (
            component.getBusCount(K_AUDIO, K_INPUT).max(0),
            component.getBusCount(K_AUDIO, K_OUTPUT).max(0),
        )
    };
    let infos = |dir: BusDirection, n: i32| -> Vec<Option<BusInfo>> {
        (0..n)
            .map(|i| bus_info(component, K_AUDIO, dir, i))
            .collect()
    };
    let in_infos = infos(K_INPUT, n_in);
    let out_infos = infos(K_OUTPUT, n_out);
    let main_flag = |i: &Option<BusInfo>| {
        i.as_ref()
            .is_some_and(|b| b.busType == BusTypes_::kMain as BusType)
    };
    let in_roles = roles(&in_infos.iter().map(main_flag).collect::<Vec<_>>());
    let out_roles: Vec<Role> = roles(&out_infos.iter().map(main_flag).collect::<Vec<_>>())
        .into_iter()
        .map(|r| if r == Role::Main { r } else { Role::Unused })
        .collect();

    // What the plugin currently has, as the proposal's base for the buses we do not care about.
    let current = |dir: BusDirection, i: i32, fallback: u64| -> u64 {
        let mut a: SpeakerArrangement = 0;
        // SAFETY: a live processor and a valid out pointer.
        if unsafe { processor.getBusArrangement(dir, i, &mut a) } == kResultOk {
            a
        } else {
            fallback
        }
    };
    let mut ins: Vec<SpeakerArrangement> = (0..n_in)
        .map(|i| match in_roles[i as usize] {
            Role::Main | Role::Sidechain => SpeakerArr::kStereo,
            Role::Unused => current(K_INPUT, i, SpeakerArr::kStereo),
        })
        .collect();
    let mut outs: Vec<SpeakerArrangement> = (0..n_out)
        .map(|i| match out_roles[i as usize] {
            Role::Main => SpeakerArr::kStereo,
            _ => current(K_OUTPUT, i, SpeakerArr::kStereo),
        })
        .collect();
    let set = |ins: &mut [SpeakerArrangement], outs: &mut [SpeakerArrangement]| -> bool {
        // SAFETY: the pointers address `ins` and `outs`, which outlive the call.
        unsafe {
            processor.setBusArrangements(
                ins.as_mut_ptr(),
                ins.len() as i32,
                outs.as_mut_ptr(),
                outs.len() as i32,
            ) == kResultTrue
        }
    };
    if !set(&mut ins, &mut outs) {
        // The plugin keeps its own arrangement: read it back and accept it when it is usable.
        for i in 0..n_in {
            ins[i as usize] = current(K_INPUT, i, ins[i as usize]);
        }
        for i in 0..n_out {
            outs[i as usize] = current(K_OUTPUT, i, outs[i as usize]);
        }
        let usable = |roles: &[Role], arr: &[u64]| {
            roles
                .iter()
                .zip(arr)
                .filter(|(r, _)| **r == Role::Main)
                .all(|(_, a)| matches!(speaker_channels(*a), 1 | 2))
        };
        if usable(&in_roles, &ins) && usable(&out_roles, &outs) {
            let _ = set(&mut ins, &mut outs);
        }
    }

    let plan_of = |roles: &[Role], arr: &[u64]| -> Vec<BusPlan> {
        roles
            .iter()
            .zip(arr)
            .map(|(r, a)| {
                let channels = speaker_channels(*a);
                // A sidechain that is not mono or stereo cannot be fed.
                let role = if *r == Role::Sidechain && !matches!(channels, 1 | 2) {
                    Role::Unused
                } else {
                    *r
                };
                BusPlan { channels, role }
            })
            .collect()
    };
    let plan = AudioPlan {
        inputs: plan_of(&in_roles, &ins),
        outputs: plan_of(&out_roles, &outs),
        // SAFETY: a live component.
        event_in: unsafe { component.getBusCount(K_EVENT, K_INPUT) } > 0,
        // SAFETY: a live component.
        event_out: unsafe { component.getBusCount(K_EVENT, K_OUTPUT) } > 0,
    };
    // The main buses must be mono or stereo.
    main_channels(plan.main_in(), "main input")?;
    main_channels(plan.main_out(), "main output")?;
    // 32-bit float is required (a 64-bit-only plugin cannot be hosted).
    // SAFETY: a live processor.
    if unsafe { processor.canProcessSampleSize(SymbolicSampleSizes_::kSample32 as i32) }
        != kResultOk
    {
        return Err("the plugin cannot process 32-bit float audio".to_string());
    }
    // Activate what the host drives, deactivate the rest.
    for (i, b) in plan.inputs.iter().enumerate() {
        // SAFETY: a live component and a valid bus index.
        unsafe { component.activateBus(K_AUDIO, K_INPUT, i as i32, u8::from(b.active())) };
    }
    for (i, b) in plan.outputs.iter().enumerate() {
        // SAFETY: as above.
        unsafe { component.activateBus(K_AUDIO, K_OUTPUT, i as i32, u8::from(b.active())) };
    }
    // Event buses: the first of each direction is driven, any further one is deactivated.
    for dir in [K_INPUT, K_OUTPUT] {
        // SAFETY: a live component.
        let count = unsafe { component.getBusCount(K_EVENT, dir) }.max(0);
        for i in 0..count {
            // SAFETY: a live component and a valid bus index.
            unsafe { component.activateBus(K_EVENT, dir, i, u8::from(i == 0)) };
        }
    }
    Ok(plan)
}

// ----- parameters and units ---------------------------------------------------------------------

/// The `/`-joined names of the unit chain of `id` below the root (`""` for the root or an
/// unknown unit). `units` holds `(id, parent id, name)`.
pub(crate) fn unit_path(units: &[(i32, i32, String)], id: i32) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut at = id;
    // A cycle guard: unit trees are shallow.
    for _ in 0..32 {
        if at == kRootUnitId || at == kNoParentUnitId {
            break;
        }
        let Some((_, parent, name)) = units.iter().find(|(u, _, _)| *u == at) else {
            break;
        };
        parts.push(name);
        at = *parent;
    }
    parts.reverse();
    parts.join("/")
}

/// The units a plugin declares: `(id, parent, name)`.
fn read_units(unit_info: Option<&ComPtr<IUnitInfo>>) -> Vec<(i32, i32, String)> {
    let Some(u) = unit_info else {
        return Vec::new();
    };
    // SAFETY: a live unit info.
    let count = unsafe { u.getUnitCount() }.max(0);
    let mut out = Vec::new();
    for i in 0..count {
        // SAFETY: a zeroed UnitInfo is valid; the call fills it.
        let mut info: UnitInfo = unsafe { std::mem::zeroed() };
        // SAFETY: a live unit info and a valid out pointer.
        if unsafe { u.getUnitInfo(i, &mut info) } == kResultOk {
            out.push((info.id, info.parentUnitId, utf16_text(&info.name)));
        }
    }
    out
}

/// The parameters the host lists: not hidden, not read-only, not a program change, in the
/// plugin's own order, capped at [`MAX_PLUGIN_PARAMS`]. Also the step counts of the
/// stepped ones (sorted by id) and whether the table was cut.
pub(crate) fn read_params(
    controller: Option<&ComPtr<IEditController>>,
) -> (Vec<PluginParamInfo>, Steps, bool) {
    let Some(c) = controller else {
        return (Vec::new(), Vec::new(), false);
    };
    let unit_info = c.cast::<IUnitInfo>();
    let units = read_units(unit_info.as_ref());
    // SAFETY: a live controller.
    let count = unsafe { c.getParameterCount() }.max(0);
    let mut out: Vec<PluginParamInfo> = Vec::new();
    let mut steps: Steps = Vec::new();
    let mut truncated = false;
    let skip = ParameterInfo_::ParameterFlags_::kIsHidden
        | ParameterInfo_::ParameterFlags_::kIsReadOnly
        | ParameterInfo_::ParameterFlags_::kIsProgramChange;
    for i in 0..count {
        // SAFETY: a zeroed ParameterInfo is valid; the call fills it.
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        // SAFETY: a live controller and a valid out pointer.
        if unsafe { c.getParameterInfo(i, &mut info) } != kResultOk {
            continue;
        }
        if info.flags & skip != 0 || out.iter().any(|p| p.id == info.id) {
            continue;
        }
        if out.len() >= MAX_PLUGIN_PARAMS {
            truncated = true;
            break;
        }
        let stepped = info.stepCount > 0;
        if stepped {
            steps.push((info.id, info.stepCount as u32));
        }
        out.push(PluginParamInfo {
            id: info.id,
            name: utf16_text(&info.title),
            module: unit_path(&units, info.unitId),
            min: 0.0,
            max: 1.0,
            default: info.defaultNormalizedValue,
            stepped,
            steps: info.stepCount.max(0) as u32,
        });
    }
    steps.sort_unstable();
    (out, steps, truncated)
}

/// How many controller numbers `IMidiMapping` is asked about: 0 to 127, aftertouch (128) and
/// pitch bend (129).
pub(crate) const MIDI_CONTROLLERS: usize = 130;

/// The params the plugin maps MIDI controllers to, for the first event bus and channel 0
/// (`None` where a controller has no assignment).
pub(crate) fn read_midi_map(controller: Option<&ComPtr<IEditController>>) -> Vec<Option<u32>> {
    let mut table = vec![None; MIDI_CONTROLLERS];
    let Some(map) = controller.and_then(|c| c.cast::<IMidiMapping>()) else {
        return table;
    };
    for (n, slot) in table.iter_mut().enumerate() {
        let mut id: ParamID = 0;
        // SAFETY: a live mapping and a valid out pointer.
        if unsafe { map.getMidiControllerAssignment(0, 0, n as i16, &mut id) } == kResultTrue {
            *slot = Some(id);
        }
    }
    table
}

/// The note expression types the plugin supports on the first event bus, as a bit per type id
/// (type ids above 31 are not tracked).
pub(crate) fn read_expressions(controller: Option<&ComPtr<IEditController>>) -> u32 {
    let Some(ex) = controller.and_then(|c| c.cast::<INoteExpressionController>()) else {
        return 0;
    };
    // SAFETY: a live controller.
    let count = unsafe { ex.getNoteExpressionCount(0, 0) }.max(0);
    let mut mask = 0u32;
    for i in 0..count {
        // SAFETY: a zeroed info is valid; the call fills it.
        let mut info: NoteExpressionTypeInfo = unsafe { std::mem::zeroed() };
        // SAFETY: a live controller and a valid out pointer.
        if unsafe { ex.getNoteExpressionInfo(0, 0, i, &mut info) } == kResultOk && info.typeId < 32
        {
            mask |= 1 << info.typeId;
        }
    }
    mask
}

/// The plugin's kind: an instrument has note input, audio output and the `Instrument`
/// sub category; a plugin with note output and no audio output is a note effect; the rest are
/// effects.
pub(crate) fn kind_of(
    sub_categories: &str,
    notes_in: bool,
    notes_out: bool,
    has_audio_out: bool,
) -> PluginKindHint {
    let instrument = sub_categories.split('|').any(|c| c.trim() == "Instrument");
    if !has_audio_out && notes_out {
        PluginKindHint::NoteEffect
    } else if instrument && notes_in && has_audio_out {
        PluginKindHint::Instrument
    } else {
        PluginKindHint::Effect
    }
}

/// Everything read from a created plugin.
#[derive(Debug, Clone)]
pub(crate) struct Read {
    pub layout: PluginLayout,
    pub plan: AudioPlan,
    pub steps: Steps,
    pub midi: Vec<Option<u32>>,
    pub expressions: u32,
    pub params_truncated: bool,
}

/// What an instance holds before its buses and params are read.
pub(crate) fn empty_read(class: &ClassInfo) -> Read {
    Read {
        layout: PluginLayout {
            name: class.name.clone(),
            vendor: class.vendor.clone(),
            version: class.version.clone(),
            kind_hint: PluginKindHint::Effect,
            main_in: None,
            main_out: None,
            sidechain: false,
            notes_in: false,
            notes_out: false,
            params: Vec::new(),
        },
        plan: AudioPlan::default(),
        steps: Vec::new(),
        midi: vec![None; MIDI_CONTROLLERS],
        expressions: 0,
        params_truncated: false,
    }
}

/// Reads the layout of a plugin whose buses are negotiated.
///
/// # Errors
/// The reason the plugin cannot be hosted.
pub(crate) fn read(
    class: &ClassInfo,
    plan: AudioPlan,
    controller: Option<&ComPtr<IEditController>>,
) -> Result<Read, String> {
    let main_in = main_channels(plan.main_in(), "main input")?;
    let main_out = main_channels(plan.main_out(), "main output")?;
    let (params, steps, params_truncated) = read_params(controller);
    let layout = PluginLayout {
        name: class.name.clone(),
        vendor: class.vendor.clone(),
        version: class.version.clone(),
        kind_hint: kind_of(
            &class.sub_categories,
            plan.event_in,
            plan.event_out,
            main_out.is_some(),
        ),
        main_in,
        main_out,
        sidechain: plan.sidechain().is_some(),
        notes_in: plan.event_in,
        notes_out: plan.event_out,
        params,
    };
    Ok(Read {
        layout,
        plan,
        steps,
        midi: read_midi_map(controller),
        expressions: read_expressions(controller),
        params_truncated,
    })
}

/// A COM pointer from a raw out pointer the plugin filled (owned, or `None` for null).
///
/// # Safety
/// `obj` must be null or one owned reference to an interface of type `I`.
pub(crate) unsafe fn owned<I: vst3::Interface>(obj: *mut c_void) -> Option<ComPtr<I>> {
    // SAFETY: the caller guarantees ownership and the interface type.
    unsafe { ComPtr::from_raw(obj.cast::<I>()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_pick_the_flagged_main_then_the_first_other_as_sidechain() {
        assert_eq!(roles(&[]), []);
        assert_eq!(roles(&[true]), [Role::Main]);
        assert_eq!(roles(&[true, false]), [Role::Main, Role::Sidechain]);
        assert_eq!(roles(&[false, true]), [Role::Sidechain, Role::Main]);
        assert_eq!(
            roles(&[true, false, false, false]),
            [Role::Main, Role::Sidechain, Role::Unused, Role::Unused]
        );
        // No flag: the first bus is the main one.
        assert_eq!(roles(&[false, false]), [Role::Main, Role::Sidechain]);
    }

    #[test]
    fn speaker_arrangements_count_their_speakers() {
        assert_eq!(speaker_channels(SpeakerArr::kMono), 1);
        assert_eq!(speaker_channels(SpeakerArr::kStereo), 2);
        assert_eq!(speaker_channels(SpeakerArr::k51), 6);
        assert_eq!(speaker_channels(0), 0);
    }

    #[test]
    fn only_mono_and_stereo_main_buses_are_usable() {
        let b = |channels| {
            Some((
                0,
                BusPlan {
                    channels,
                    role: Role::Main,
                },
            ))
        };
        assert_eq!(main_channels(b(1), "x"), Ok(Some(Channels::Mono)));
        assert_eq!(main_channels(b(2), "x"), Ok(Some(Channels::Stereo)));
        assert_eq!(main_channels(None, "x"), Ok(None));
        let e = main_channels(b(6), "main input").unwrap_err();
        assert!(
            e.contains("6 channels") && e.contains("mono or stereo"),
            "{e}"
        );
    }

    #[test]
    fn unit_paths_join_the_chain_below_the_root() {
        let units = vec![
            (0, -1, "Root".to_string()),
            (1, 0, "Bank".to_string()),
            (2, 1, "Group 3".to_string()),
            (3, 0, "Main".to_string()),
        ];
        assert_eq!(unit_path(&units, 0), "");
        assert_eq!(unit_path(&units, 3), "Main");
        assert_eq!(unit_path(&units, 2), "Bank/Group 3");
        assert_eq!(unit_path(&units, 99), "", "an unknown unit has no path");
        let cyclic = vec![(1, 2, "A".to_string()), (2, 1, "B".to_string())];
        assert!(unit_path(&cyclic, 1).len() < 200, "a cycle ends");
    }

    #[test]
    fn the_kind_follows_buses_and_sub_categories() {
        assert_eq!(kind_of("Fx", false, false, true), PluginKindHint::Effect);
        assert_eq!(
            kind_of("Instrument|Synth", true, false, true),
            PluginKindHint::Instrument
        );
        assert_eq!(
            kind_of("Instrument", true, true, false),
            PluginKindHint::NoteEffect
        );
        // An `Instrument` that takes no notes is an effect, an `Fx` with notes in is one too.
        assert_eq!(
            kind_of("Instrument", false, false, true),
            PluginKindHint::Effect
        );
        assert_eq!(kind_of("Fx", true, false, true), PluginKindHint::Effect);
        assert_eq!(
            kind_of("Fx|Instrument|Reverb", true, false, true),
            PluginKindHint::Instrument
        );
        assert_eq!(
            kind_of("Instrumental", true, false, true),
            PluginKindHint::Effect
        );
    }
}
