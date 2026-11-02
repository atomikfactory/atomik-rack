//! The modulation matrix: sources, targets and the slot bus a device sums at its control rate.
//!
//! A device that opts in declares [`ModTables`]: the **sources** it computes itself (its
//! envelopes, LFOs, velocity, mod wheel, pressure...) and the **targets** it lets a source move
//! (each one a parameter of the device). The user connects them in up to [`MOD_SLOTS`] **slots**;
//! a slot is `source -> target` with an amount, and optionally a `via` source that scales the
//! amount (a mod wheel that fades an LFO in). The engine knows nothing about slots: they reach
//! the device as ordinary params ([`mod_slot_params`]) and the device sums them with a
//! [`ModBus`].
//!
//! * Source and target ids are indices into the tables and never change once released: a new
//!   entry is appended. A slot stores ids, so a project survives a table that grows.
//! * Slots are summed in slot order in normalised space (the `add_norm` convention of the CV
//!   inputs): `offset = sum(amount * source * via)` with `+ - * /` only, so the result is
//!   bit-identical on every platform. The device adds the offset to the target's own normalised
//!   or natural-unit value and clamps it.
//! * The bus never allocates and costs one bool test while no slot is in use.
//!
//! See "Modulation matrix" in `docs/EXTENDING.md`.

use crate::{DescriptorError, DeviceDescriptor};
use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};

/// Slots per device.
pub const MOD_SLOTS: usize = 8;
/// Most sources a device can declare (a slot stores a source as `id + 1` in a small param).
pub const MAX_MOD_SOURCES: usize = 16;
/// Most targets a device can declare.
pub const MAX_MOD_TARGETS: usize = 16;
/// Params per slot: source, target, via and amount.
pub const MOD_SLOT_FIELDS: usize = 4;
/// Params of the whole slot block.
pub const MOD_SLOT_PARAMS: usize = MOD_SLOTS * MOD_SLOT_FIELDS;

/// Something a device computes that can move a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModSource {
    /// Id == index in the source table. Stored in projects: never reorder, append.
    pub id: u16,
    /// Stable machine key, e.g. `"lfo1"`. Unique among the sources.
    pub key: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// True when the source swings both ways (`-1..1`, an LFO), false for `0..1` (an envelope,
    /// velocity). Display only: the bus clamps every source to `-1..1`.
    pub bipolar: bool,
}

/// A parameter a source may move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModTarget {
    /// Id == index in the target table. Stored in projects: never reorder, append.
    pub id: u16,
    /// Stable machine key, e.g. `"cutoff"`. Unique among the targets.
    pub key: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// The device param the target belongs to. A slot never changes the stored param: the
    /// device adds the slot sum to it while it renders.
    pub param: ParamId,
}

/// A device's modulation declaration, registered with
/// [`crate::Registry::register_with_mods`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModTables {
    /// The sources, `id == index`.
    pub sources: &'static [ModSource],
    /// The targets, `id == index`.
    pub targets: &'static [ModTarget],
    /// Id of the first param of the device's slot block: the [`MOD_SLOT_PARAMS`] params made by
    /// [`mod_slot_params`] starting at this id.
    pub first_param: u32,
}

/// The four params of one slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModField {
    /// The source: 0 is none, `n` is source id `n - 1`.
    Source,
    /// The target: 0 is none, `n` is target id `n - 1`.
    Target,
    /// The scaling source: 0 is none (the amount stands alone), `n` is source id `n - 1`.
    Via,
    /// The amount, `-1..=1`; automatable like any param.
    Amount,
}

impl ModField {
    /// All fields in param order.
    pub const ALL: [ModField; MOD_SLOT_FIELDS] = [
        ModField::Source,
        ModField::Target,
        ModField::Via,
        ModField::Amount,
    ];

    /// Param key of this field of slot `slot` (`0..MOD_SLOTS`). Source, target and via are
    /// `internal.` params (set from the slot list, never stored as params); the amount is a
    /// plain param so it can be automated, mapped and saved in a patch.
    pub const fn key(self, slot: usize) -> &'static str {
        match self {
            ModField::Source => SOURCE_KEYS[slot],
            ModField::Target => TARGET_KEYS[slot],
            ModField::Via => VIA_KEYS[slot],
            ModField::Amount => AMOUNT_KEYS[slot],
        }
    }

    /// Human-readable name of this field of slot `slot`.
    pub const fn name(self, slot: usize) -> &'static str {
        match self {
            ModField::Source => SOURCE_NAMES[slot],
            ModField::Target => TARGET_NAMES[slot],
            ModField::Via => VIA_NAMES[slot],
            ModField::Amount => AMOUNT_NAMES[slot],
        }
    }
}

macro_rules! slot_strings {
    ($($name:ident: $prefix:literal $suffix:literal;)*) => {
        $(
            const $name: [&str; MOD_SLOTS] = [
                concat!($prefix, "1", $suffix),
                concat!($prefix, "2", $suffix),
                concat!($prefix, "3", $suffix),
                concat!($prefix, "4", $suffix),
                concat!($prefix, "5", $suffix),
                concat!($prefix, "6", $suffix),
                concat!($prefix, "7", $suffix),
                concat!($prefix, "8", $suffix),
            ];
        )*
    };
}

slot_strings! {
    SOURCE_KEYS: "internal.mod" "_source";
    TARGET_KEYS: "internal.mod" "_target";
    VIA_KEYS: "internal.mod" "_via";
    AMOUNT_KEYS: "mod" "_amount";
    SOURCE_NAMES: "Mod " " source";
    TARGET_NAMES: "Mod " " target";
    VIA_NAMES: "Mod " " via";
    AMOUNT_NAMES: "Mod " " amount";
}

/// The param key of one field of one slot as a string (`slot` is `0..MOD_SLOTS`).
pub fn mod_param_key(slot: usize, field: ModField) -> String {
    field.key(slot.min(MOD_SLOTS - 1)).to_owned()
}

/// The slot block of a device's param table, ids `first..first + MOD_SLOT_PARAMS`. Join it to
/// the device's own params (see `join_params`) so the block ends where the descriptor says.
pub const fn mod_slot_params(first: u32) -> [ParamDescriptor; MOD_SLOT_PARAMS] {
    const BLANK: ParamDescriptor = ParamDescriptor {
        id: ParamId(0),
        key: "",
        name: "",
        min: 0.0,
        max: 0.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: true,
    };
    let mut out = [BLANK; MOD_SLOT_PARAMS];
    let mut slot = 0;
    while slot < MOD_SLOTS {
        let mut f = 0;
        while f < MOD_SLOT_FIELDS {
            let field = ModField::ALL[f];
            let amount = matches!(field, ModField::Amount);
            let max = match field {
                ModField::Amount => 1.0,
                _ => MAX_MOD_SOURCES as f32,
            };
            out[slot * MOD_SLOT_FIELDS + f] = ParamDescriptor {
                id: ParamId(first + (slot * MOD_SLOT_FIELDS + f) as u32),
                key: field.key(slot),
                name: field.name(slot),
                min: if amount { -1.0 } else { 0.0 },
                max,
                default: 0.0,
                taper: Taper::Linear,
                unit: if amount { Unit::Percent } else { Unit::None },
                discrete: !amount,
            };
            f += 1;
        }
        slot += 1;
    }
    out
}

/// Joins two param tables into one (`A + B` entries), for a device whose descriptor is its own
/// params followed by [`mod_slot_params`].
pub const fn join_params<const A: usize, const B: usize, const C: usize>(
    a: [ParamDescriptor; A],
    b: [ParamDescriptor; B],
) -> [ParamDescriptor; C] {
    assert!(A + B == C, "the joined table has A + B entries");
    let mut out = [a[0]; C];
    let mut i = 0;
    while i < A {
        out[i] = a[i];
        i += 1;
    }
    let mut j = 0;
    while j < B {
        out[A + j] = b[j];
        j += 1;
    }
    out
}

impl ModTables {
    /// Checks the tables against the descriptor of the device that declares them: ids equal
    /// indices, keys are unique, every target names a real param of the device (outside the slot
    /// block itself), the counts are within [`MAX_MOD_SOURCES`] and [`MAX_MOD_TARGETS`], and the
    /// slot block sits in the param table exactly as [`mod_slot_params`] makes it.
    pub fn validate(&self, desc: &DeviceDescriptor) -> Result<(), DescriptorError> {
        if self.sources.len() > MAX_MOD_SOURCES {
            return Err(DescriptorError::TooManyModEntries {
                what: "sources",
                count: self.sources.len(),
                max: MAX_MOD_SOURCES,
            });
        }
        if self.targets.len() > MAX_MOD_TARGETS {
            return Err(DescriptorError::TooManyModEntries {
                what: "targets",
                count: self.targets.len(),
                max: MAX_MOD_TARGETS,
            });
        }
        for (index, s) in self.sources.iter().enumerate() {
            if usize::from(s.id) != index {
                return Err(DescriptorError::ModIdMismatch {
                    key: s.key,
                    id: s.id,
                    index,
                });
            }
            if self.sources[..index].iter().any(|t| t.key == s.key) {
                return Err(DescriptorError::DuplicateKey(s.key));
            }
        }
        let block = self.first_param as usize..self.first_param as usize + MOD_SLOT_PARAMS;
        for (index, t) in self.targets.iter().enumerate() {
            if usize::from(t.id) != index {
                return Err(DescriptorError::ModIdMismatch {
                    key: t.key,
                    id: t.id,
                    index,
                });
            }
            if self.targets[..index].iter().any(|u| u.key == t.key) {
                return Err(DescriptorError::DuplicateKey(t.key));
            }
            let at = t.param.0 as usize;
            if at >= desc.params.len() || block.contains(&at) {
                return Err(DescriptorError::ModUnknownParam {
                    key: t.key,
                    param: t.param.0,
                });
            }
        }
        let expect = mod_slot_params(self.first_param);
        for (i, want) in expect.iter().enumerate() {
            let ok = desc
                .params
                .get(block.start + i)
                .is_some_and(|have| have == want);
            if !ok {
                return Err(DescriptorError::ModSlotBlock {
                    first: self.first_param,
                });
            }
        }
        Ok(())
    }

    /// The source with `key`.
    pub fn source_by_key(&self, key: &str) -> Option<&ModSource> {
        self.sources.iter().find(|s| s.key == key)
    }

    /// The target with `key`.
    pub fn target_by_key(&self, key: &str) -> Option<&ModTarget> {
        self.targets.iter().find(|t| t.key == key)
    }

    /// Whether param `id` belongs to the slot block.
    pub fn is_slot_param(&self, id: u32) -> bool {
        id >= self.first_param && (id - self.first_param) < MOD_SLOT_PARAMS as u32
    }
}

/// One slot as the bus holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    /// Source id + 1, 0 for none.
    source: u8,
    /// Target id + 1, 0 for none.
    target: u8,
    /// Via source id + 1, 0 for none.
    via: u8,
    amount: f32,
}

impl Slot {
    const EMPTY: Slot = Slot {
        source: 0,
        target: 0,
        via: 0,
        amount: 0.0,
    };

    /// A slot with nothing to add: no source, no target or a zero amount.
    fn is_idle(&self) -> bool {
        self.source == 0 || self.target == 0 || self.amount == 0.0
    }
}

/// The slots of one device, summed at the control rate. `Copy` and fixed-size: a device keeps
/// one next to its other shared state and copies it with it, with no allocation anywhere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModBus {
    first: u32,
    slots: [Slot; MOD_SLOTS],
    /// One bit per target id that some live slot reaches.
    reached: u16,
}

/// A source value as the bus reads it: non-finite is 0, then clamped to `-1..=1`.
#[inline]
fn clean(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

impl ModBus {
    /// An empty bus for a device whose slot block starts at param `first_param`
    /// ([`ModTables::first_param`]).
    pub const fn new(first_param: u32) -> Self {
        Self {
            first: first_param,
            slots: [Slot::EMPTY; MOD_SLOTS],
            reached: 0,
        }
    }

    /// Takes a param value if `id` is in the slot block and returns true; false for any other
    /// param. Call it from `Device::set_param` first. RT-safe.
    pub fn set_param(&mut self, id: u32, plain: f32) -> bool {
        if id < self.first {
            return false;
        }
        let at = (id - self.first) as usize;
        if at >= MOD_SLOT_PARAMS {
            return false;
        }
        let slot = &mut self.slots[at / MOD_SLOT_FIELDS];
        let choice = |v: f32| {
            if v.is_finite() {
                v.round().clamp(0.0, MAX_MOD_SOURCES as f32) as u8
            } else {
                0
            }
        };
        match ModField::ALL[at % MOD_SLOT_FIELDS] {
            ModField::Source => slot.source = choice(plain),
            ModField::Target => slot.target = choice(plain),
            ModField::Via => slot.via = choice(plain),
            ModField::Amount => slot.amount = clean(plain),
        }
        self.reached = 0;
        for s in &self.slots {
            if !s.is_idle() && s.target <= 16 {
                self.reached |= 1u16 << (s.target - 1);
            }
        }
        true
    }

    /// Whether any slot is in use. A device skips its modulation code entirely when false, so a
    /// device without slots renders exactly as before the bus existed.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.reached != 0
    }

    /// Whether some live slot reaches target `target` (a target id).
    #[inline]
    pub fn reaches(&self, target: u16) -> bool {
        target < 16 && self.reached & (1u16 << target) != 0
    }

    /// The sum of every live slot that reaches `target`, in slot order:
    /// `sum(amount * source * via)`. `sources` holds the current value of every source, indexed
    /// by source id (a missing entry reads 0); each is sanitised and clamped to `-1..=1` here.
    /// `via` is the second source clamped to `0..=1`; a slot without one is not scaled.
    /// Only `+`, `-`, `*` and `/` touch the values, so the sum is the same bit for bit everywhere.
    #[inline]
    pub fn offset(&self, target: u16, sources: &[f32]) -> f32 {
        if !self.reaches(target) {
            return 0.0;
        }
        let mut sum = 0.0f32;
        for s in &self.slots {
            if s.is_idle() || u16::from(s.target) != target + 1 {
                continue;
            }
            let value = clean(
                sources
                    .get(usize::from(s.source) - 1)
                    .copied()
                    .unwrap_or(0.0),
            );
            let scale = if s.via == 0 {
                1.0
            } else {
                clean(sources.get(usize::from(s.via) - 1).copied().unwrap_or(0.0)).max(0.0)
            };
            sum += s.amount * value * scale;
        }
        sum
    }

    /// `base + offset(target)` in normalised space, clamped to `0..=1`: the convention of the CV
    /// inputs (`cv_in::add_norm`) with the slot sum in place of `trim * cv`.
    #[inline]
    pub fn add_norm(&self, base: f32, target: u16, sources: &[f32]) -> f32 {
        (base + self.offset(target, sources)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::tests::{GAIN, GOOD};
    use crate::{DeviceDescriptor, Registry};

    const SOURCES: &[ModSource] = &[
        ModSource {
            id: 0,
            key: "lfo",
            name: "LFO",
            bipolar: true,
        },
        ModSource {
            id: 1,
            key: "env",
            name: "Envelope",
            bipolar: false,
        },
        ModSource {
            id: 2,
            key: "wheel",
            name: "Wheel",
            bipolar: false,
        },
    ];
    const TARGETS: &[ModTarget] = &[ModTarget {
        id: 0,
        key: "gain",
        name: "Gain",
        param: ParamId(0),
    }];
    const TABLES: ModTables = ModTables {
        sources: SOURCES,
        targets: TARGETS,
        first_param: 1,
    };
    static PARAMS: [ParamDescriptor; 1 + MOD_SLOT_PARAMS] = join_params([GAIN], mod_slot_params(1));
    static DESC: DeviceDescriptor = DeviceDescriptor {
        params: &PARAMS,
        ..GOOD
    };

    fn bus(slots: &[(usize, f32, f32, f32, f32)]) -> ModBus {
        // (slot, source, target, via, amount) as param values.
        let mut b = ModBus::new(1);
        for &(slot, source, target, via, amount) in slots {
            let at = 1 + (slot * MOD_SLOT_FIELDS) as u32;
            assert!(b.set_param(at, source));
            assert!(b.set_param(at + 1, target));
            assert!(b.set_param(at + 2, via));
            assert!(b.set_param(at + 3, amount));
        }
        b
    }

    #[test]
    fn the_slot_block_is_valid_and_keys_are_unique() {
        assert_eq!(DESC.validate(), Ok(()));
        assert_eq!(TABLES.validate(&DESC), Ok(()));
        assert_eq!(PARAMS[1].key, "internal.mod1_source");
        assert_eq!(PARAMS[4].key, "mod1_amount");
        assert_eq!(PARAMS[1 + MOD_SLOT_PARAMS - 1].key, "mod8_amount");
        assert_eq!(mod_param_key(2, ModField::Via), "internal.mod3_via");
    }

    #[test]
    fn validation_rejects_ids_that_do_not_match_the_tables() {
        const BAD_SOURCES: &[ModSource] = &[ModSource {
            id: 3,
            ..SOURCES[0]
        }];
        let t = ModTables {
            sources: BAD_SOURCES,
            ..TABLES
        };
        assert!(matches!(
            t.validate(&DESC),
            Err(DescriptorError::ModIdMismatch { id: 3, .. })
        ));
        const BAD_TARGETS: &[ModTarget] = &[ModTarget {
            id: 2,
            ..TARGETS[0]
        }];
        let t = ModTables {
            targets: BAD_TARGETS,
            ..TABLES
        };
        assert!(matches!(
            t.validate(&DESC),
            Err(DescriptorError::ModIdMismatch { id: 2, .. })
        ));
        const DUP: &[ModSource] = &[
            SOURCES[0],
            ModSource {
                id: 1,
                ..SOURCES[0]
            },
        ];
        let t = ModTables {
            sources: DUP,
            ..TABLES
        };
        assert_eq!(t.validate(&DESC), Err(DescriptorError::DuplicateKey("lfo")));
    }

    #[test]
    fn validation_rejects_a_target_that_is_no_param() {
        const GONE: &[ModTarget] = &[ModTarget {
            param: ParamId(99),
            ..TARGETS[0]
        }];
        let t = ModTables {
            targets: GONE,
            ..TABLES
        };
        assert!(matches!(
            t.validate(&DESC),
            Err(DescriptorError::ModUnknownParam { param: 99, .. })
        ));
        // A target cannot be a slot param itself.
        const SELF: &[ModTarget] = &[ModTarget {
            param: ParamId(4),
            ..TARGETS[0]
        }];
        let t = ModTables {
            targets: SELF,
            ..TABLES
        };
        assert!(matches!(
            t.validate(&DESC),
            Err(DescriptorError::ModUnknownParam { param: 4, .. })
        ));
    }

    #[test]
    fn validation_rejects_a_descriptor_without_the_slot_block() {
        // GOOD has one param only: no room for the block.
        assert!(matches!(
            TABLES.validate(&GOOD),
            Err(DescriptorError::ModSlotBlock { first: 1 })
        ));
        let moved = ModTables {
            first_param: 2,
            ..TABLES
        };
        assert!(matches!(
            moved.validate(&DESC),
            Err(DescriptorError::ModSlotBlock { first: 2 })
        ));
    }

    #[test]
    fn too_many_sources_are_refused() {
        static MANY: [ModSource; MAX_MOD_SOURCES + 1] = {
            let mut a = [SOURCES[0]; MAX_MOD_SOURCES + 1];
            let mut i = 0;
            while i < a.len() {
                a[i].id = i as u16;
                i += 1;
            }
            a
        };
        let t = ModTables {
            sources: &MANY,
            ..TABLES
        };
        assert!(matches!(
            t.validate(&DESC),
            Err(DescriptorError::TooManyModEntries {
                what: "sources",
                ..
            })
        ));
    }

    #[test]
    fn the_registry_validates_the_tables_and_keeps_them() {
        use crate::{Device, DeviceFactory, PrepareInfo, ProcessContext};
        struct Null;
        impl Device for Null {
            fn descriptor(&self) -> &'static DeviceDescriptor {
                &DESC
            }
            fn prepare(&mut self, _: &PrepareInfo) {}
            fn reset(&mut self) {}
            fn set_param(&mut self, _: ParamId, _: f32) {}
            fn process(&mut self, _: &mut ProcessContext<'_>) {}
        }
        let factory = DeviceFactory {
            descriptor: &DESC,
            create: || Box::new(Null),
        };
        let mut r = Registry::new();
        assert!(r.mod_tables("test.good").is_none());
        assert_eq!(r.register_with_mods(factory, &TABLES), Ok(()));
        assert_eq!(r.mod_tables("test.good"), Some(&TABLES));
        assert!(r.get("test.good").is_some());
        // An invalid table is refused and nothing is registered.
        let mut r = Registry::new();
        const GONE: &[ModTarget] = &[ModTarget {
            param: ParamId(99),
            ..TARGETS[0]
        }];
        static BAD: ModTables = ModTables {
            targets: GONE,
            ..TABLES
        };
        assert!(matches!(
            r.register_with_mods(factory, &BAD),
            Err(crate::RegistryError::InvalidDescriptor { .. })
        ));
        assert!(r.get("test.good").is_none());
    }

    #[test]
    fn an_empty_bus_adds_nothing() {
        let b = ModBus::new(1);
        assert!(!b.is_active());
        assert_eq!(b.offset(0, &[1.0, 1.0, 1.0]), 0.0);
        assert_eq!(b.add_norm(0.4, 0, &[1.0]), 0.4);
    }

    #[test]
    fn a_slot_adds_amount_times_source() {
        let b = bus(&[(0, 1.0, 1.0, 0.0, 0.5)]);
        assert!(b.is_active() && b.reaches(0) && !b.reaches(1));
        assert_eq!(b.offset(0, &[0.5, 0.0, 0.0]), 0.25);
        assert_eq!(b.offset(0, &[-1.0, 0.0, 0.0]), -0.5);
        assert_eq!(b.offset(1, &[1.0]), 0.0, "another target is untouched");
        assert_eq!(b.add_norm(0.9, 0, &[1.0]), 1.0, "clamped to the range");
        assert_eq!(b.add_norm(0.1, 0, &[-1.0]), 0.0);
    }

    #[test]
    fn slots_sum_in_order_and_a_negative_amount_subtracts() {
        let b = bus(&[(0, 1.0, 1.0, 0.0, 0.5), (3, 2.0, 1.0, 0.0, -0.25)]);
        assert_eq!(b.offset(0, &[0.5, 1.0, 0.0]), 0.25 - 0.25);
        let s = [1.0, 0.5, 0.0];
        assert_eq!(b.offset(0, &s), 0.5 + (-0.25 * 0.5));
    }

    #[test]
    fn via_scales_the_amount_and_clamps_to_zero_one() {
        // LFO (source 1) via the wheel (source 3).
        let b = bus(&[(0, 1.0, 1.0, 3.0, 1.0)]);
        assert_eq!(b.offset(0, &[0.8, 0.0, 0.0]), 0.0, "wheel down: no vibrato");
        assert_eq!(b.offset(0, &[0.8, 0.0, 0.5]), 0.4);
        assert_eq!(
            b.offset(0, &[0.8, 0.0, -1.0]),
            0.0,
            "negative via clamps to 0"
        );
    }

    #[test]
    fn idle_slots_and_bad_values_are_ignored() {
        // No target, no source, zero amount.
        for (s, t, a) in [(1.0, 0.0, 1.0), (0.0, 1.0, 1.0), (1.0, 1.0, 0.0)] {
            let b = bus(&[(0, s, t, 0.0, a)]);
            assert!(!b.is_active());
            assert_eq!(b.offset(0, &[1.0]), 0.0);
        }
        let b = bus(&[(0, 1.0, 1.0, 0.0, 1.0)]);
        assert_eq!(b.offset(0, &[f32::NAN]), 0.0);
        assert_eq!(b.offset(0, &[f32::INFINITY]), 0.0);
        assert_eq!(b.offset(0, &[7.0]), 1.0, "sources clamp to -1..1");
        assert_eq!(b.offset(0, &[]), 0.0, "a missing source reads 0");
        // A choice beyond the tables reads as a source that has no value.
        let b = bus(&[(0, 9.0, 1.0, 0.0, 1.0)]);
        assert_eq!(b.offset(0, &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn params_outside_the_block_are_not_taken() {
        let mut b = ModBus::new(5);
        assert!(!b.set_param(4, 1.0));
        assert!(!b.set_param(5 + MOD_SLOT_PARAMS as u32, 1.0));
        assert!(b.set_param(5, 1.0));
    }

    #[test]
    fn the_bus_is_copy_and_deterministic() {
        let a = bus(&[(0, 1.0, 1.0, 2.0, 0.3), (1, 2.0, 1.0, 0.0, -0.7)]);
        let b = a;
        let src = [0.3f32, 0.9, 0.6];
        assert_eq!(
            a.offset(0, &src).to_bits(),
            b.offset(0, &src).to_bits(),
            "same slots, same sources, same bits"
        );
    }
}
