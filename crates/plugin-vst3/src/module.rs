//! Loading a `.vst3` module: the file or bundle layout per operating system, the module
//! entry and exit functions, and `GetPluginFactory`.
//!
//! * Windows: `X.vst3` is a DLL, or a bundle folder with `Contents/<arch>-win/X.vst3`.
//! * macOS: `X.vst3` is a bundle, `Contents/MacOS/<CFBundleExecutable>`; entry `bundleEntry`
//!   (older SDKs `BundleEntry`) with a `CFBundleRef`, exit `bundleExit`.
//! * Linux: `X.vst3` is a bundle, `Contents/<arch>-linux/X.so`; entry `ModuleEntry` with the
//!   library handle, exit `ModuleExit`.
//!
//! Layout and entry names are those of the VST3 developer portal's "Plug-in Format" page and the
//! SDK's module code.

use atomik_plugin_host::BackendError;
use atomik_plugin_host::paths::Os;
#[cfg(not(windows))]
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use vst3::{ComPtr, Steinberg::IPluginFactory};

/// The folder name of this build's CPU in a bundle (`x86_64`, `arm64`, ...) for `os`.
fn arch_folders(os: Os, arch: &str) -> Vec<&'static str> {
    match (os, arch) {
        (Os::Windows, "x86_64") => vec!["x86_64-win"],
        (Os::Windows, "aarch64") => vec!["arm64-win", "arm64x-win", "arm64ec-win"],
        (Os::Windows, "x86") => vec!["x86-win"],
        (Os::Linux, "x86_64") => vec!["x86_64-linux"],
        (Os::Linux, "aarch64") => vec!["aarch64-linux", "arm64-linux"],
        (Os::Linux, "arm") => vec!["armv7l-linux", "armv8l-linux"],
        (Os::Linux, "x86") => vec!["i386-linux"],
        _ => Vec::new(),
    }
}

/// The `CFBundleExecutable` of an `Info.plist`, if it names one.
pub(crate) fn plist_executable(plist: &str) -> Option<String> {
    let key = "<key>CFBundleExecutable</key>";
    let rest = &plist[plist.find(key)? + key.len()..];
    let open = rest.find("<string>")? + "<string>".len();
    let close = rest[open..].find("</string>")? + open;
    let name = rest[open..close].trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The binaries a `.vst3` path may hold, most likely first. A file is its own binary; a bundle
/// folder is searched by the layout of `os` for CPU `arch` (`std::env::consts::ARCH` names).
pub(crate) fn module_binaries(path: &Path, os: Os, arch: &str) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let contents = path.join("Contents");
    match os {
        Os::Windows => arch_folders(os, arch)
            .into_iter()
            .map(|a| contents.join(a).join(format!("{stem}.vst3")))
            .collect(),
        Os::Linux => arch_folders(os, arch)
            .into_iter()
            .map(|a| contents.join(a).join(format!("{stem}.so")))
            .collect(),
        Os::MacOs => {
            let mut out = Vec::new();
            if let Ok(plist) = std::fs::read_to_string(contents.join("Info.plist"))
                && let Some(exe) = plist_executable(&plist)
            {
                out.push(contents.join("MacOS").join(exe));
            }
            out.push(contents.join("MacOS").join(&stem));
            out
        }
    }
}

// SAFETY: these are the signatures of the VST3 module entry points, called only on
// symbols resolved by their documented names (see `Module::enter`, `Module::factory_of`).
#[cfg(not(windows))]
type EntryFn = unsafe extern "system" fn(*mut c_void) -> bool;
// SAFETY: as above.
#[cfg(windows)]
type EntryNoArgFn = unsafe extern "system" fn() -> bool;
// SAFETY: as above.
type ExitFn = unsafe extern "system" fn() -> bool;
// SAFETY: as above.
type FactoryFn = unsafe extern "system" fn() -> *mut IPluginFactory;

// SAFETY: not an unsafe operation, a library type whose loading functions are unsafe and are
// each called with a SAFETY comment below.
#[cfg(unix)]
use libloading::os::unix::Library as RawLibrary;
#[cfg(windows)]
use libloading::os::windows::Library as RawLibrary;

#[cfg(target_os = "macos")]
mod bundle {
    //! The `CFBundleRef` of a bundle, for `bundleEntry`.
    use std::ffi::c_void;
    use std::path::Path;

    // SAFETY: the declarations match the CoreFoundation C API.
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: *const c_void,
            buffer: *const u8,
            length: isize,
            is_directory: bool,
        ) -> *const c_void;
        fn CFBundleCreate(allocator: *const c_void, url: *const c_void) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }

    /// A retained `CFBundleRef`, released on drop.
    pub(super) struct Bundle(*const c_void);

    impl Bundle {
        pub(super) fn open(path: &Path) -> Option<Self> {
            let text = path.to_str()?;
            // SAFETY: the buffer and its length describe `text`; a null allocator is the default.
            let url = unsafe {
                CFURLCreateFromFileSystemRepresentation(
                    std::ptr::null(),
                    text.as_ptr(),
                    text.len() as isize,
                    true,
                )
            };
            if url.is_null() {
                return None;
            }
            // SAFETY: `url` is a live CFURL.
            let bundle = unsafe { CFBundleCreate(std::ptr::null(), url) };
            // SAFETY: we own the URL reference created above.
            unsafe { CFRelease(url) };
            (!bundle.is_null()).then_some(Self(bundle))
        }

        pub(super) fn raw(&self) -> *mut c_void {
            self.0.cast_mut()
        }
    }

    impl Drop for Bundle {
        fn drop(&mut self) {
            // SAFETY: we own the bundle reference.
            unsafe { CFRelease(self.0) };
        }
    }
}

fn refused(path: &Path) -> BackendError {
    BackendError::Failed(format!(
        "{}: the module refused to initialise",
        path.display()
    ))
}

/// A loaded VST3 module: its library, its factory and its exit function. Dropping it releases
/// the factory, runs the exit function and unloads the library.
pub(crate) struct Module {
    path: PathBuf,
    factory: Option<ComPtr<IPluginFactory>>,
    exit: Option<ExitFn>,
    lib: Option<RawLibrary>,
    #[cfg(target_os = "macos")]
    _bundle: Option<bundle::Bundle>,
}

// SAFETY: the factory is a COM object whose methods the VST3 specification allows to be called
// from any thread (factory calls are thread-safe), the exit function is a plain function pointer
// and the library handle is only used to unload. The module is shared behind an `Arc`.
unsafe impl Send for Module {}
// SAFETY: see above; shared access only reads the factory pointer.
unsafe impl Sync for Module {}

impl std::fmt::Debug for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Module").field("path", &self.path).finish()
    }
}

impl Module {
    /// Loads the module at `path`, runs its entry and fetches its factory.
    ///
    /// # Errors
    /// [`BackendError::NotAPlugin`] when the file cannot be loaded or has no factory function,
    /// [`BackendError::Failed`] when the module's entry refuses.
    pub(crate) fn load(path: &Path) -> Result<Arc<Self>, BackendError> {
        let not = |why: String| BackendError::NotAPlugin(format!("{}: {why}", path.display()));
        if !path.exists() {
            return Err(not("no such file".to_string()));
        }
        let candidates = module_binaries(path, Os::current(), std::env::consts::ARCH);
        let Some(binary) = candidates.iter().find(|c| c.is_file()) else {
            return Err(not(
                "no module binary for this operating system and CPU in the bundle".to_string(),
            ));
        };
        // The loader needs an absolute path to search the module's own folder for its DLLs.
        let binary = std::path::absolute(binary).unwrap_or_else(|_| binary.clone());
        // SAFETY: loading a plugin module runs its code; this is the point of hosting. The app
        // only loads files the scanner process already loaded without a crash.
        let lib = unsafe { open_library(&binary) }.map_err(|e| not(e.to_string()))?;

        let mut module = Module {
            path: path.to_path_buf(),
            factory: None,
            exit: None,
            lib: None,
            #[cfg(target_os = "macos")]
            _bundle: None,
        };
        // SAFETY: the symbols are looked up by their documented names and called with their
        // documented signatures (VST3 module entry points).
        unsafe { module.enter(lib, path)? };
        // SAFETY: as above.
        let factory = unsafe { module.factory_of() }?;
        module.factory = Some(factory);
        Ok(Arc::new(module))
    }

    /// Runs the module entry (`InitDll`, `bundleEntry` or `ModuleEntry`) and keeps the library.
    ///
    /// # Safety
    /// `lib` must be a freshly loaded VST3 module library.
    #[cfg(windows)]
    unsafe fn enter(&mut self, lib: RawLibrary, path: &Path) -> Result<(), BackendError> {
        // SAFETY: documented names and signatures; see the caller.
        unsafe {
            // `InitDll` and `ExitDll` are optional on Windows.
            if let Ok(init) = lib.get::<EntryNoArgFn>(b"InitDll ")
                && !init()
            {
                return Err(refused(path));
            }
            self.exit = lib.get::<ExitFn>(b"ExitDll ").ok().map(|f| *f);
        }
        self.lib = Some(lib);
        Ok(())
    }

    /// Runs the module entry and keeps the library (macOS).
    ///
    /// # Safety
    /// `lib` must be a freshly loaded VST3 module library.
    #[cfg(target_os = "macos")]
    unsafe fn enter(&mut self, lib: RawLibrary, path: &Path) -> Result<(), BackendError> {
        // SAFETY: documented names and signatures; see the caller.
        unsafe {
            let entry: EntryFn = *lib
                .get::<EntryFn>(b"bundleEntry ")
                .or_else(|_| lib.get::<EntryFn>(b"BundleEntry "))
                .map_err(|_| {
                    BackendError::NotAPlugin(format!("{}: no bundleEntry function", path.display()))
                })?;
            let bundle = bundle::Bundle::open(path)
                .ok_or_else(|| BackendError::Failed(format!("{}: not a bundle", path.display())))?;
            if !entry(bundle.raw()) {
                return Err(refused(path));
            }
            self.exit = lib
                .get::<ExitFn>(b"bundleExit ")
                .or_else(|_| lib.get::<ExitFn>(b"BundleExit "))
                .ok()
                .map(|f| *f);
            self._bundle = Some(bundle);
        }
        self.lib = Some(lib);
        Ok(())
    }

    /// Runs the module entry and keeps the library (Linux and other Unix).
    ///
    /// # Safety
    /// `lib` must be a freshly loaded VST3 module library.
    #[cfg(all(unix, not(target_os = "macos")))]
    unsafe fn enter(&mut self, lib: RawLibrary, path: &Path) -> Result<(), BackendError> {
        // The entry receives the library handle, so hand it the real one.
        let handle = lib.into_raw();
        // SAFETY: `handle` came from `into_raw` just now; this re-wraps the same handle.
        let lib = unsafe { RawLibrary::from_raw(handle) };
        // SAFETY: documented names and signatures; see the caller.
        unsafe {
            let entry: EntryFn = *lib.get::<EntryFn>(b"ModuleEntry ").map_err(|_| {
                BackendError::NotAPlugin(format!("{}: no ModuleEntry function", path.display()))
            })?;
            if !entry(handle) {
                return Err(refused(path));
            }
            self.exit = lib.get::<ExitFn>(b"ModuleExit ").ok().map(|f| *f);
        }
        self.lib = Some(lib);
        Ok(())
    }

    /// Calls `GetPluginFactory`.
    ///
    /// # Safety
    /// The library must be entered.
    unsafe fn factory_of(&self) -> Result<ComPtr<IPluginFactory>, BackendError> {
        let not = |why: &str| BackendError::NotAPlugin(format!("{}: {why}", self.path.display()));
        let lib = self.lib.as_ref().ok_or_else(|| not("not loaded"))?;
        // SAFETY: documented name and signature.
        let get = unsafe { lib.get::<FactoryFn>(b"GetPluginFactory\0") }
            .map_err(|_| not("no GetPluginFactory function"))?;
        // SAFETY: `GetPluginFactory` returns one owned reference or null.
        let ptr = unsafe { get() };
        // SAFETY: a non-null result is an owned IPluginFactory reference.
        unsafe { ComPtr::from_raw(ptr) }.ok_or_else(|| not("GetPluginFactory returned null"))
    }

    /// The module's factory.
    pub(crate) fn factory(&self) -> Option<&ComPtr<IPluginFactory>> {
        self.factory.as_ref()
    }
}

/// Opens a library the way plugin hosts do: on Windows the DLL's own folder is searched first
/// for its dependencies.
///
/// # Safety
/// Runs the library's initialisation code.
unsafe fn open_library(binary: &Path) -> Result<RawLibrary, libloading::Error> {
    #[cfg(windows)]
    {
        // SAFETY: see the function's contract.
        unsafe {
            RawLibrary::load_with_flags(
                binary,
                libloading::os::windows::LOAD_WITH_ALTERED_SEARCH_PATH,
            )
        }
    }
    #[cfg(unix)]
    {
        // SAFETY: see the function's contract.
        unsafe { RawLibrary::new(binary) }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // The factory goes first, then the module's exit function, then the library.
        self.factory = None;
        if let Some(exit) = self.exit.take() {
            // SAFETY: the exit function of a module this struct entered; called once.
            unsafe { exit() };
        }
        self.lib = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("atomik-vst3-module-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_plist_executable_is_read_from_the_key() {
        let plist = "<plist><dict><key>CFBundleName</key><string>A</string>\n<key>CFBundleExecutable</key>\n  <string> Real Name </string></dict></plist>";
        assert_eq!(plist_executable(plist).as_deref(), Some("Real Name"));
        assert_eq!(plist_executable("<plist></plist>"), None);
        assert_eq!(
            plist_executable("<key>CFBundleExecutable</key><string></string>"),
            None
        );
    }

    #[test]
    fn a_bundle_folder_is_searched_by_the_layout_of_its_operating_system() {
        let d = temp("layout").join("Foo.vst3");
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(
            module_binaries(&d, Os::Windows, "x86_64"),
            [d.join("Contents/x86_64-win/Foo.vst3")]
        );
        assert_eq!(
            module_binaries(&d, Os::Windows, "aarch64"),
            [
                d.join("Contents/arm64-win/Foo.vst3"),
                d.join("Contents/arm64x-win/Foo.vst3"),
                d.join("Contents/arm64ec-win/Foo.vst3")
            ]
        );
        assert_eq!(
            module_binaries(&d, Os::Linux, "x86_64"),
            [d.join("Contents/x86_64-linux/Foo.so")]
        );
        assert_eq!(
            module_binaries(&d, Os::MacOs, "aarch64"),
            [d.join("Contents/MacOS/Foo")]
        );
        std::fs::create_dir_all(d.join("Contents")).unwrap();
        std::fs::write(
            d.join("Contents/Info.plist"),
            "<key>CFBundleExecutable</key><string>Bar</string>",
        )
        .unwrap();
        assert_eq!(
            module_binaries(&d, Os::MacOs, "aarch64"),
            [d.join("Contents/MacOS/Bar"), d.join("Contents/MacOS/Foo")]
        );
        assert!(module_binaries(&d, Os::Linux, "riscv64").is_empty());
    }

    #[test]
    fn a_plain_file_is_its_own_binary_and_a_missing_one_is_not_a_plugin() {
        let d = temp("file");
        let f = d.join("One.vst3");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(
            module_binaries(&f, Os::Windows, "x86_64"),
            std::slice::from_ref(&f)
        );
        assert!(matches!(
            Module::load(&d.join("missing.vst3")),
            Err(BackendError::NotAPlugin(_))
        ));
        // A file that is not a library: the loader refuses it.
        assert!(matches!(Module::load(&f), Err(BackendError::NotAPlugin(_))));
        // An empty bundle folder has no binary for this system.
        let empty = d.join("Empty.vst3");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(matches!(
            Module::load(&empty),
            Err(BackendError::NotAPlugin(_))
        ));
    }
}
