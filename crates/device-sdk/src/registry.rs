//! Registry of device factories keyed by type id.

use crate::plugin::{PluginStandIn, is_plugin_type_id, placeholder_stand_in};
use crate::{DescriptorError, Device, DeviceDescriptor};
use std::collections::{BTreeMap, BTreeSet};

/// Creates instances of one device type.
#[derive(Clone, Copy)]
pub struct DeviceFactory {
    /// The type's descriptor.
    pub descriptor: &'static DeviceDescriptor,
    /// Constructor. The returned device still needs [`Device::prepare`].
    pub create: fn() -> Box<dyn Device>,
}

impl std::fmt::Debug for DeviceFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceFactory")
            .field("type_id", &self.descriptor.type_id)
            .finish()
    }
}

/// Registration failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// The type id is already registered.
    #[error("device type `{0}` is already registered")]
    Duplicate(&'static str),
    /// The descriptor is invalid.
    #[error("device type `{type_id}` has an invalid descriptor: {source}")]
    InvalidDescriptor {
        /// Offending type.
        type_id: &'static str,
        /// Reason.
        source: DescriptorError,
    },
    /// A static registration used the reserved `clap.` or `vst3.` prefix.
    #[error("device type `{0}` uses a reserved plugin prefix")]
    ReservedPrefix(&'static str),
    /// A plugin registration whose type id is not in the plugin namespace.
    #[error("`{0}` is not a plugin type id (`clap.` or `vst3.`)")]
    NotAPluginType(String),
    /// The descriptor carries another type id than the one it is registered under.
    #[error("plugin type `{type_id}` registered with the descriptor of `{descriptor}`")]
    TypeIdMismatch {
        /// The id given to `register_plugin`.
        type_id: String,
        /// The id inside the descriptor.
        descriptor: String,
    },
    /// The plugin type id is already registered.
    #[error("plugin type `{0}` is already registered")]
    DuplicatePlugin(String),
    /// No plugin type with this id is registered.
    #[error("plugin type `{0}` is not registered")]
    UnknownPlugin(String),
}

/// Device factories by type id, in deterministic (sorted) order.
#[derive(Debug, Default)]
pub struct Registry {
    factories: BTreeMap<&'static str, DeviceFactory>,
    /// Types that take a sample map.
    sampled: BTreeSet<&'static str>,
    /// Types that take pattern data.
    players: BTreeSet<&'static str>,
    /// Types that take curve rows with their pattern data.
    curved: BTreeSet<&'static str>,
    /// Types that take drum pattern data.
    drummed: BTreeSet<&'static str>,
    /// Dynamically registered plugin types. Kept apart from `factories`, so the
    /// sorted static list and its counts do not change.
    plugins: BTreeMap<String, DeviceFactory>,
}

impl Registry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a factory after validating its descriptor.
    pub fn register(&mut self, factory: DeviceFactory) -> Result<(), RegistryError> {
        let type_id = factory.descriptor.type_id;
        if is_plugin_type_id(type_id) {
            return Err(RegistryError::ReservedPrefix(type_id));
        }
        factory
            .descriptor
            .validate()
            .map_err(|source| RegistryError::InvalidDescriptor { type_id, source })?;
        if self.factories.contains_key(type_id) {
            return Err(RegistryError::Duplicate(type_id));
        }
        self.factories.insert(type_id, factory);
        Ok(())
    }

    /// Registers a factory whose devices take a sample map ([`crate::Device::swap_samples`]).
    pub fn register_with_samples(&mut self, factory: DeviceFactory) -> Result<(), RegistryError> {
        self.register(factory)?;
        self.sampled.insert(factory.descriptor.type_id);
        Ok(())
    }

    /// Whether `type_id` was registered with [`Registry::register_with_samples`].
    pub fn takes_samples(&self, type_id: &str) -> bool {
        self.sampled.contains(type_id)
    }

    /// Registers a factory whose devices take pattern data
    /// ([`crate::Device::swap_player_data`]).
    pub fn register_with_player_data(
        &mut self,
        factory: DeviceFactory,
    ) -> Result<(), RegistryError> {
        self.register(factory)?;
        self.players.insert(factory.descriptor.type_id);
        Ok(())
    }

    /// Whether `type_id` was registered with [`Registry::register_with_player_data`].
    pub fn takes_player_data(&self, type_id: &str) -> bool {
        self.players.contains(type_id)
    }

    /// Registers a factory whose devices take pattern data and curve rows
    /// ([`crate::PlayerData::CvSeq`]); such a type is also listed by
    /// [`Registry::takes_player_data`].
    pub fn register_with_curves(&mut self, factory: DeviceFactory) -> Result<(), RegistryError> {
        self.register_with_player_data(factory)?;
        self.curved.insert(factory.descriptor.type_id);
        Ok(())
    }

    /// Whether `type_id` was registered with [`Registry::register_with_curves`].
    pub fn takes_curves(&self, type_id: &str) -> bool {
        self.curved.contains(type_id)
    }

    /// Registers a factory whose devices take drum pattern data ([`crate::PlayerData::Drums`]);
    /// such a type is also listed by [`Registry::takes_player_data`].
    pub fn register_with_drums(&mut self, factory: DeviceFactory) -> Result<(), RegistryError> {
        self.register_with_player_data(factory)?;
        self.drummed.insert(factory.descriptor.type_id);
        Ok(())
    }

    /// Whether `type_id` was registered with [`Registry::register_with_drums`].
    pub fn takes_drums(&self, type_id: &str) -> bool {
        self.drummed.contains(type_id)
    }

    /// Registers a plugin device type. Non-RT; `&mut self` on the thread that
    /// owns the registry. `type_id` is `clap.<id>` or `vst3.<hex>` and must equal the
    /// descriptor's own type id (the descriptor comes from [`crate::descriptor_for`]).
    pub fn register_plugin(
        &mut self,
        type_id: String,
        descriptor: &'static DeviceDescriptor,
    ) -> Result<(), RegistryError> {
        if !is_plugin_type_id(&type_id) {
            return Err(RegistryError::NotAPluginType(type_id));
        }
        if descriptor.type_id != type_id {
            return Err(RegistryError::TypeIdMismatch {
                type_id,
                descriptor: descriptor.type_id.to_string(),
            });
        }
        descriptor
            .validate()
            .map_err(|source| RegistryError::InvalidDescriptor {
                type_id: descriptor.type_id,
                source,
            })?;
        if self.plugins.contains_key(&type_id) {
            return Err(RegistryError::DuplicatePlugin(type_id));
        }
        self.plugins.insert(
            type_id,
            DeviceFactory {
                descriptor,
                create: placeholder_stand_in,
            },
        );
        Ok(())
    }

    /// Removes a plugin type (a rescan or a disabled plugin). Existing devices keep running.
    pub fn unregister_plugin(&mut self, type_id: &str) -> Result<(), RegistryError> {
        self.plugins
            .remove(type_id)
            .map(|_| ())
            .ok_or_else(|| RegistryError::UnknownPlugin(type_id.to_string()))
    }

    /// Whether `type_id` is a registered plugin type.
    pub fn is_plugin(&self, type_id: &str) -> bool {
        self.plugins.contains_key(type_id)
    }

    /// Registered plugin type ids in sorted order.
    pub fn plugin_type_ids(&self) -> impl Iterator<Item = &str> + '_ {
        self.plugins.keys().map(String::as_str)
    }

    /// The descriptor of `type_id`, built-in or plugin.
    pub fn descriptor(&self, type_id: &str) -> Option<&'static DeviceDescriptor> {
        self.get(type_id).map(|f| f.descriptor)
    }

    /// Creates a new (unprepared) instance of `type_id`. For a plugin type this is the
    /// **stand-in** device ([`PluginStandIn`]); the real plugin device is built by the plugin
    /// host.
    pub fn create(&self, type_id: &str) -> Option<Box<dyn Device>> {
        if let Some(f) = self.factories.get(type_id) {
            return Some((f.create)());
        }
        self.plugins
            .get(type_id)
            .map(|f| Box::new(PluginStandIn::new(f.descriptor)) as Box<dyn Device>)
    }

    /// The factory for `type_id`. For a plugin type the factory's `descriptor` is right, but its
    /// `create` cannot know the plugin (a `fn()` carries no data) and builds a placeholder:
    /// use [`Registry::create`] to get the stand-in.
    pub fn get(&self, type_id: &str) -> Option<&DeviceFactory> {
        self.factories
            .get(type_id)
            .or_else(|| self.plugins.get(type_id))
    }

    /// Registered type ids in sorted order.
    pub fn type_ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.factories.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::tests::GOOD;
    use crate::{PrepareInfo, ProcessContext};
    use atomik_core::{ParamDescriptor, ParamId};

    struct Null;
    impl Device for Null {
        fn descriptor(&self) -> &'static DeviceDescriptor {
            &GOOD
        }
        fn prepare(&mut self, _: &PrepareInfo) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: ParamId, _: f32) {}
        fn process(&mut self, _: &mut ProcessContext<'_>) {}
    }

    fn factory() -> DeviceFactory {
        DeviceFactory {
            descriptor: &GOOD,
            create: || Box::new(Null),
        }
    }

    #[test]
    fn registry_register_duplicate_errors() {
        let mut r = Registry::new();
        assert_eq!(r.register(factory()), Ok(()));
        assert_eq!(
            r.register(factory()),
            Err(RegistryError::Duplicate("test.good"))
        );
    }

    #[test]
    fn registry_marks_types_that_take_samples() {
        let mut r = Registry::new();
        assert!(!r.takes_samples("test.good"));
        assert_eq!(r.register_with_samples(factory()), Ok(()));
        assert!(r.takes_samples("test.good"));
        assert!(!r.takes_samples("test.other"));
        assert_eq!(
            r.register_with_samples(factory()),
            Err(RegistryError::Duplicate("test.good"))
        );
        let mut plain = Registry::new();
        plain.register(factory()).unwrap();
        assert!(!plain.takes_samples("test.good"));
    }

    #[test]
    fn default_swap_samples_hands_the_map_back() {
        let map = std::sync::Arc::new(crate::SampleMap::new(Vec::new(), Vec::new()).unwrap());
        let back = Null.swap_samples(Some(map.clone())).unwrap();
        assert!(std::sync::Arc::ptr_eq(&map, &back));
        assert!(Null.swap_samples(None).is_none());
    }

    #[test]
    fn registry_marks_types_that_take_player_data() {
        let mut r = Registry::new();
        assert!(!r.takes_player_data("test.good"));
        assert_eq!(r.register_with_player_data(factory()), Ok(()));
        assert!(r.takes_player_data("test.good"));
        assert!(!r.takes_player_data("test.other"));
        assert!(
            !r.takes_samples("test.good"),
            "the two opt-ins are separate"
        );
        assert_eq!(
            r.register_with_player_data(factory()),
            Err(RegistryError::Duplicate("test.good"))
        );
        let mut plain = Registry::new();
        plain.register(factory()).unwrap();
        assert!(!plain.takes_player_data("test.good"));
        let mut sampled = Registry::new();
        sampled.register_with_samples(factory()).unwrap();
        assert!(!sampled.takes_player_data("test.good"));
    }

    #[test]
    fn registry_marks_types_that_take_curves() {
        let mut r = Registry::new();
        assert!(!r.takes_curves("test.good"));
        assert_eq!(r.register_with_curves(factory()), Ok(()));
        assert!(r.takes_curves("test.good"));
        assert!(
            r.takes_player_data("test.good"),
            "curves come with pattern data"
        );
        assert!(!r.takes_curves("test.other"));
        let mut plain = Registry::new();
        plain.register_with_player_data(factory()).unwrap();
        assert!(!plain.takes_curves("test.good"));
    }

    #[test]
    fn registry_marks_types_that_take_drums() {
        let mut r = Registry::new();
        assert!(!r.takes_drums("test.good"));
        assert_eq!(r.register_with_drums(factory()), Ok(()));
        assert!(r.takes_drums("test.good"));
        assert!(
            r.takes_player_data("test.good"),
            "drum data is pattern data"
        );
        assert!(!r.takes_curves("test.good"));
        assert!(!r.takes_drums("test.other"));
    }

    #[test]
    fn default_swap_player_data_hands_the_data_back() {
        let data: crate::PlayerDataRef =
            std::sync::Arc::new(crate::PlayerData::StepSeq(crate::StepSeqData::default()));
        let back = Null.swap_player_data(Some(data.clone())).unwrap();
        assert!(std::sync::Arc::ptr_eq(&data, &back));
        assert!(Null.swap_player_data(None).is_none());
    }

    #[test]
    fn registry_register_invalid_descriptor_errors() {
        static BAD_P: [ParamDescriptor; 1] = [ParamDescriptor {
            id: ParamId(7),
            ..crate::descriptor::tests::GAIN
        }];
        static BAD: DeviceDescriptor = DeviceDescriptor {
            params: &BAD_P,
            ..GOOD
        };
        let mut r = Registry::new();
        let e = r.register(DeviceFactory {
            descriptor: &BAD,
            create: || Box::new(Null),
        });
        assert!(matches!(e, Err(RegistryError::InvalidDescriptor { .. })));
    }

    #[test]
    fn registry_create_known_and_unknown() {
        let mut r = Registry::new();
        r.register(factory()).unwrap();
        assert_eq!(
            r.create("test.good").map(|d| d.descriptor().type_id),
            Some("test.good")
        );
        assert!(r.create("nope").is_none());
        assert!(r.get("test.good").is_some());
        assert_eq!(r.type_ids().collect::<Vec<_>>(), vec!["test.good"]);
    }

    // ----- dynamic plugin types ----------------------------------------

    fn plugin_descriptor(id: &str) -> &'static DeviceDescriptor {
        use crate::plugin::*;
        descriptor_for(
            &PluginRef {
                format: PluginFormat::Clap,
                id: id.to_string(),
            },
            &PluginLayout {
                name: "Dyn".to_string(),
                vendor: "V".to_string(),
                version: "1".to_string(),
                kind_hint: PluginKindHint::Effect,
                main_in: Some(Channels::Stereo),
                main_out: Some(Channels::Stereo),
                sidechain: false,
                notes_in: false,
                notes_out: false,
                params: Vec::new(),
            },
        )
        .unwrap()
    }

    #[test]
    fn register_plugin_adds_a_type_that_create_and_get_resolve() {
        let mut r = Registry::new();
        let d = plugin_descriptor("reg.a");
        assert!(!r.is_plugin("clap.reg.a"));
        assert_eq!(r.register_plugin("clap.reg.a".to_string(), d), Ok(()));
        assert!(r.is_plugin("clap.reg.a"));
        assert!(!r.is_plugin("atomik.x"));
        assert!(std::ptr::eq(r.get("clap.reg.a").unwrap().descriptor, d));
        assert!(std::ptr::eq(r.descriptor("clap.reg.a").unwrap(), d));
        // `create` builds the stand-in, with the right descriptor.
        let dev = r.create("clap.reg.a").unwrap();
        assert!(std::ptr::eq(dev.descriptor(), d));
        assert_eq!(r.plugin_type_ids().collect::<Vec<_>>(), vec!["clap.reg.a"]);
    }

    #[test]
    fn plugin_types_live_apart_from_the_static_list() {
        let mut r = Registry::new();
        r.register(factory()).unwrap();
        r.register_plugin("clap.reg.b".to_string(), plugin_descriptor("reg.b"))
            .unwrap();
        assert_eq!(r.type_ids().collect::<Vec<_>>(), vec!["test.good"]);
        assert_eq!(
            r.descriptor("test.good").map(|d| d.type_id),
            Some("test.good"),
            "descriptor() covers both maps"
        );
        assert!(r.descriptor("nope").is_none());
    }

    #[test]
    fn register_plugin_enforces_the_prefix_rule_and_duplicates() {
        let mut r = Registry::new();
        let d = plugin_descriptor("reg.c");
        assert_eq!(
            r.register_plugin("atomik.reg.c".to_string(), d),
            Err(RegistryError::NotAPluginType("atomik.reg.c".to_string()))
        );
        assert_eq!(
            r.register_plugin("clap.other".to_string(), d),
            Err(RegistryError::TypeIdMismatch {
                type_id: "clap.other".to_string(),
                descriptor: "clap.reg.c".to_string()
            }),
            "the descriptor must carry the type id it is registered under"
        );
        r.register_plugin("clap.reg.c".to_string(), d).unwrap();
        assert_eq!(
            r.register_plugin("clap.reg.c".to_string(), d),
            Err(RegistryError::DuplicatePlugin("clap.reg.c".to_string()))
        );
    }

    #[test]
    fn the_plugin_prefixes_are_reserved_for_static_registration() {
        static RESERVED: DeviceDescriptor = DeviceDescriptor {
            type_id: "clap.sneaky",
            ..GOOD
        };
        let mut r = Registry::new();
        assert_eq!(
            r.register(DeviceFactory {
                descriptor: &RESERVED,
                create: || Box::new(Null),
            }),
            Err(RegistryError::ReservedPrefix("clap.sneaky"))
        );
    }

    #[test]
    fn unregister_plugin_removes_it_and_reports_unknown_ids() {
        let mut r = Registry::new();
        r.register_plugin("clap.reg.d".to_string(), plugin_descriptor("reg.d"))
            .unwrap();
        assert_eq!(r.unregister_plugin("clap.reg.d"), Ok(()));
        assert!(!r.is_plugin("clap.reg.d"));
        assert!(r.get("clap.reg.d").is_none());
        assert!(r.create("clap.reg.d").is_none());
        assert_eq!(
            r.unregister_plugin("clap.reg.d"),
            Err(RegistryError::UnknownPlugin("clap.reg.d".to_string()))
        );
        // A built-in type is not a plugin type and cannot be unregistered this way.
        r.register(factory()).unwrap();
        assert_eq!(
            r.unregister_plugin("test.good"),
            Err(RegistryError::UnknownPlugin("test.good".to_string()))
        );
        assert!(r.get("test.good").is_some());
        // The id can be registered again afterwards (a rescan re-registers).
        r.register_plugin("clap.reg.d".to_string(), plugin_descriptor("reg.d"))
            .unwrap();
    }

    #[test]
    fn takes_samples_and_player_data_are_false_for_plugin_types() {
        let mut r = Registry::new();
        r.register_plugin("clap.reg.e".to_string(), plugin_descriptor("reg.e"))
            .unwrap();
        assert!(!r.takes_samples("clap.reg.e"));
        assert!(!r.takes_player_data("clap.reg.e"));
    }
}
