//! (memory): the heap a plugin catalogue and one hosted plugin instance hold, measured with the
//! counting allocator on synthetic plugins of 50, 500 and 2 000 params. The figures are printed
//! (`cargo test -p atomik-plugin-host --test memory_figures -- --nocapture`) and bounded by
//! ceilings, so a change that makes either grow is caught; the registered catalogue is also checked
//! to answer every query as the plain path does. Own binary: the allocator guard counts
//! per thread, and every measurement builds what it measures on the test's own thread.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::MAX_EVENTS_PER_BATCH;
use atomik_device_sdk::{
    PluginFormat, PluginLayout, PluginParamInfo, PluginRef, Registry, RunMode, descriptor_for,
};
use atomik_plugin_host::{
    CachedFile, FileStamp, InstantiateRequest, NullBackend, NullBehavior, NullPlugin, PluginCache,
    PluginCatalog, PluginEvent, PluginManager, PluginSettings, ScannedPlugin, mark_main_thread,
};
use atomik_testkit::no_alloc::held_heap_of;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use support::{SR, fx_layout};

const FILES: usize = 132;

/// A layout of `n` params with names and module paths as a real plugin has them.
fn layout_of(n: usize) -> PluginLayout {
    let mut layout = fx_layout();
    layout.params = (0..n as u32)
        .map(|i| PluginParamInfo {
            id: 1_000 + i,
            name: format!("Oscillator {} Parameter {i}", i % 7),
            module: format!("Section {}/Group {}", i % 5, i % 11),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            stepped: false,
            steps: 0,
        })
        .collect();
    layout
}

fn scanned(tag: &str, file: usize, params: usize) -> CachedFile {
    let plugin = PluginRef {
        format: PluginFormat::Clap,
        id: format!("mem.{tag}.{file}"),
    };
    let layout = layout_of(params);
    CachedFile {
        path: PathBuf::from(format!("C:/Plugins/Vendor {file}/Plugin {file}.clap")),
        format: PluginFormat::Clap,
        stamp: FileStamp {
            size: 1,
            mtime_ns: 1,
        },
        scanner_version: atomik_plugin_host::SCANNER_VERSION,
        scanned_secs: 1,
        plugins: vec![ScannedPlugin {
            category: atomik_device_sdk::PluginCategory::from_kind(layout.kind_hint),
            plugin,
            layout,
            unsupported: None,
            params_truncated: false,
        }],
    }
}

fn enabled() -> PluginSettings {
    PluginSettings {
        enabled: true,
        ..PluginSettings::default()
    }
}

/// The figures of one catalogue size, in bytes.
struct Catalogue {
    cache: isize,
    entries: isize,
    /// The registered catalogue: lazy slots, no descriptor built.
    registered: isize,
    /// What reading the descriptor of five plugins builds.
    five: isize,
    /// What reading the descriptor of every other plugin builds (what an eager registry held
    /// from the start, together with `five`).
    rest: isize,
    layout_clone: isize,
}

fn measure_catalogue(tag: &str, params: usize) -> Catalogue {
    let (cache, cache_bytes) = held_heap_of(|| {
        let mut cache = PluginCache::default();
        for f in 0..FILES {
            cache.insert_file(scanned(tag, f, params));
        }
        cache
    });
    let (mut catalog, entries) = held_heap_of(|| PluginCatalog::from_cache(&cache, &enabled()));
    let mut registry = Registry::new();
    let (report, registered) = held_heap_of(|| {
        catalog.register_pinned_into(&mut registry, &mut |_, _| {}, &BTreeMap::new())
    });
    assert_eq!(report.registered, FILES, "{:?}", report.errors);
    let ids: Vec<String> = registry.plugin_type_ids().map(str::to_string).collect();
    let (_, five) = held_heap_of(|| {
        for id in &ids[..5] {
            std::hint::black_box(registry.descriptor(id).expect("registered"));
        }
    });
    let (_, rest) = held_heap_of(|| {
        for id in &ids[5..] {
            std::hint::black_box(registry.descriptor(id).expect("registered"));
        }
    });
    assert_eq!(registry.plugin_descriptors_built(), FILES);
    let layout = layout_of(params);
    let (clone, layout_clone) = held_heap_of(|| layout.clone());
    drop(clone);
    // Keep everything alive to here: the figures above are what is held while the app runs.
    std::hint::black_box((&cache, &catalog, &registry));
    Catalogue {
        cache: cache_bytes,
        entries,
        registered,
        five,
        rest,
        layout_clone,
    }
}

/// The heap one more instance of a plugin of `params` params holds, and the heap of the pieces.
struct Instance {
    total: isize,
    device_only: isize,
    layout_clone: isize,
    handle_layout: isize,
}

fn instance_request(id: &str) -> InstantiateRequest {
    InstantiateRequest {
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: id.to_string(),
        },
        state: None,
        sample_rate: SR,
        mode: RunMode::Realtime,
        pinned: false,
        powered: true,
    }
}

fn measure_instance(params: usize) -> Instance {
    mark_main_thread();
    let id = format!("mem.inst.{params}");
    let layout = layout_of(params);
    let backend = NullBackend::new().with_plugin(NullPlugin {
        path: PathBuf::from(format!("{id}.clap")),
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: id.clone(),
        },
        layout: layout.clone(),
        behavior: NullBehavior::Gain(0.5),
        latency: 0,
    });
    let mut manager = PluginManager::new();
    manager.add_backend(Box::new(backend));
    let request = instance_request(&id);
    // The first instance interns the descriptor of the layout: that is the plugin's cost, not the
    // instance's.
    let first = manager.begin_create(&request).unwrap();
    let _warm = manager.poll_create(first).unwrap().unwrap();
    let (handle, total) = held_heap_of(|| {
        let t = manager.begin_create(&request).unwrap();
        manager.poll_create(t).unwrap().unwrap()
    });
    let handle_layout = held_heap_of(|| handle.layout.clone()).1;
    let (clone, layout_clone) = held_heap_of(|| layout.clone());
    drop(clone);
    // The device alone: the parameter tables, the event lists and the scratch buffers.
    let descriptor = descriptor_for(
        &PluginRef {
            format: PluginFormat::Clap,
            id: id.clone(),
        },
        &layout,
    )
    .unwrap();
    let processor = Box::new(support::Probe::new().0);
    let (end, _manager_end) = atomik_plugin_host::plugin_channel(atomik_plugin_host::InstanceId(9));
    let (device, device_only) = held_heap_of(|| {
        atomik_plugin_host::PluginDevice::new(descriptor, &layout, processor, end, SR)
    });
    std::hint::black_box((&handle, &device, &manager));
    Instance {
        total,
        device_only,
        layout_clone,
        handle_layout,
    }
}

fn kib(bytes: isize) -> String {
    format!("{:.1} KiB", bytes as f64 / 1024.0)
}

#[test]
fn the_catalogue_of_132_plugin_files_holds_this_much_at_start() {
    for (tag, params) in [("a", 50usize), ("b", 500), ("c", 2_000)] {
        let c = measure_catalogue(tag, params);
        let eager = c.registered + c.five + c.rest;
        let all = c.cache + c.entries + c.registered;
        let per_param = all as f64 / (FILES * params) as f64;
        println!(
            "catalogue {params:>4} params x {FILES} files: scan entries {}, catalog entries {}, \
             registered (leaked descriptors, table of interned layouts) {}, one layout {}; \
             all three {} = {per_param:.0} B per param",
            kib(c.cache),
            kib(c.entries),
            kib(c.registered),
            kib(c.layout_clone),
            kib(all),
        );
        println!(
            "descriptors {params:>4} params x {FILES} files: at start {}, after reading 5 {}, after reading all {} (eager)",
            kib(c.registered),
            kib(c.registered + c.five),
            kib(eager),
        );
        let table = (FILES * params) as isize;
        // What the registry adds is the leaked descriptors alone (about 116 bytes a param): the
        // table of interned layouts shares the catalog's copy instead of keeping its own, which
        // was another 120 bytes a param.
        assert!(
            eager <= 135 * table + 400 * 1024,
            "registered {} for {table} params",
            kib(eager)
        );
        // Registering builds no descriptor: the catalogue holds a small slot per plugin, whatever
        // its param count, and reading five descriptors costs five plugins, not the catalogue.
        assert!(c.registered <= 200 * 1024, "at start {}", kib(c.registered));
        assert!(
            c.five <= (c.five + c.rest) * 6 / FILES as isize,
            "five {} of {}",
            kib(c.five),
            kib(c.five + c.rest)
        );
        // The catalog entries hold one layout each and nothing else of size.
        assert!(
            c.entries <= c.layout_clone * FILES as isize * 11 / 10 + 256 * 1024,
            "entries {}",
            kib(c.entries)
        );
    }
}

#[test]
fn a_registered_catalogue_answers_as_the_plain_path_does() {
    let mut cache = PluginCache::default();
    for f in 0..20 {
        cache.insert_file(scanned("q", f, 30 + f));
    }
    let mut catalog = PluginCatalog::from_cache(&cache, &enabled());
    let pinned_plugin = PluginRef {
        format: PluginFormat::Clap,
        id: "mem.q.3".to_string(),
    };
    let pinned_row = PluginParamInfo {
        id: 99_999,
        name: "Only the project uses this".to_string(),
        module: String::new(),
        min: 0.0,
        max: 1.0,
        default: 0.0,
        stepped: false,
        steps: 0,
    };
    let pins = BTreeMap::from([(pinned_plugin.clone(), vec![pinned_row.clone()])]);
    let mut registry = Registry::new();
    let mut told = Vec::new();
    let report = catalog.register_pinned_into(
        &mut registry,
        &mut |plugin, path| told.push((plugin.clone(), path.to_path_buf())),
        &pins,
    );
    assert_eq!((report.registered, report.errors.len()), (20, 0));
    assert_eq!(told.len(), 20);
    assert_eq!(registry.plugin_type_ids().count(), 20);
    for file in cache.files() {
        for scanned in &file.plugins {
            let plugin = &scanned.plugin;
            let entry = catalog.entry(plugin).expect("listed");
            assert_eq!(*entry.layout, scanned.layout, "the entry keeps the scan");
            // The same descriptor the plain call builds, down to the pointer.
            let mut listed = scanned.layout.clone();
            if *plugin == pinned_plugin {
                atomik_plugin_host::layout::merge_pinned(
                    &mut listed,
                    std::slice::from_ref(&pinned_row),
                );
            }
            let plain = descriptor_for(plugin, &listed).unwrap();
            let registered = registry.descriptor(&plugin.type_id()).expect("registered");
            assert!(std::ptr::eq(plain, registered), "{}", plugin.id);
            assert_eq!(registered.name, scanned.layout.name);
            let first = &scanned.layout.params[0];
            let by_key = registered
                .params
                .iter()
                .find(|p| p.key == format!("p{}", first.id))
                .expect("a param is found by its key");
            assert_eq!(by_key.name, first.name);
            let rows = scanned.layout.params.len() + usize::from(*plugin == pinned_plugin);
            assert_eq!(
                registered.params.len(),
                1 + rows + 2 * atomik_device_sdk::PLUGIN_CV_INPUTS + 1,
                "bypass, the plugin's params, the CV programmer and the MPE switch"
            );
        }
    }
    // The pinned row is in the table of that plugin and in no other.
    let key = format!("p{}", pinned_row.id);
    let has = |id: &str| {
        registry
            .descriptor(&format!("clap.{id}"))
            .is_some_and(|d| d.params.iter().any(|p| p.key == key))
    };
    assert!(has("mem.q.3") && !has("mem.q.4"));
    // The tables the registry leaked share the catalog's layouts: one copy per plugin, not two
    // (the pinned plugin's table was built from a merged layout, which only the registry holds).
    for e in catalog.entries() {
        let held = Arc::strong_count(&e.layout);
        assert_eq!(
            held,
            if e.plugin == pinned_plugin { 1 } else { 2 },
            "{}",
            e.plugin.id
        );
    }
}

#[test]
fn one_hosted_instance_holds_this_much() {
    for (params, ceiling) in [(50usize, 300isize), (500, 520), (2_000, 1_350)] {
        let i = measure_instance(params);
        let events = (4 * MAX_EVENTS_PER_BATCH + 256 + params + 1 + MAX_EVENTS_PER_BATCH)
            * std::mem::size_of::<PluginEvent>();
        println!(
            "instance {params:>4} params: manager side (device, instance, listed layout, handle \
             layout) {}, device alone {} (its event lists are reserved and never written: {}), \
             one layout {}, handle layout {}",
            kib(i.total),
            kib(i.device_only),
            kib(events as isize),
            kib(i.layout_clone),
            kib(i.handle_layout),
        );
        assert!(
            i.total <= ceiling * 1024,
            "{params} params: {} over {ceiling} KiB",
            kib(i.total)
        );
    }
}
