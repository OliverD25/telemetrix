//! NVIDIA GPUs through NVML, loaded at run time: there is no link-time
//! dependency, and a PC without an NVIDIA driver simply has no GPU card.
//! Windows: `nvml.dll`; Linux: `libnvidia-ml.so.1`. Both come with the driver.

use std::ffi::{CStr, c_char, c_int, c_uint, c_void};

use super::GpuMetric;

type Ret = c_int;
type Device = *mut c_void;

const SUCCESS: Ret = 0;
/// NVML_DEVICE_NAME_V2_BUFFER_SIZE.
const NAME_LEN: usize = 96;
/// NVML_TEMPERATURE_GPU: the GPU die.
const TEMPERATURE_GPU: c_int = 0;

#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

#[repr(C)]
#[derive(Default)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}

// NVML uses the C calling convention; on x86_64 Windows that is the only one.
type NoArgs = unsafe extern "C" fn() -> Ret;
type CountFn = unsafe extern "C" fn(*mut c_uint) -> Ret;
type HandleFn = unsafe extern "C" fn(c_uint, *mut Device) -> Ret;
type NameFn = unsafe extern "C" fn(Device, *mut c_char, c_uint) -> Ret;
type UtilizationFn = unsafe extern "C" fn(Device, *mut Utilization) -> Ret;
type MemoryFn = unsafe extern "C" fn(Device, *mut Memory) -> Ret;
type TemperatureFn = unsafe extern "C" fn(Device, c_int, *mut c_uint) -> Ret;
type PowerFn = unsafe extern "C" fn(Device, *mut c_uint) -> Ret;

struct Api {
    init: NoArgs,
    shutdown: NoArgs,
    count: CountFn,
    handle: HandleFn,
    name: NameFn,
    utilization: UtilizationFn,
    memory: MemoryFn,
    temperature: TemperatureFn,
    power: PowerFn,
}

impl Api {
    fn resolve(lib: &lib::Library) -> Result<Self, String> {
        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                let name: &CStr = $name;
                let p = lib
                    .symbol(name)
                    .ok_or_else(|| format!("NVML has no {}", name.to_string_lossy()))?;
                // SAFETY: the symbol comes from NVML and has this signature (nvml.h).
                unsafe { std::mem::transmute::<*const c_void, $ty>(p) }
            }};
        }
        Ok(Self {
            init: sym!(c"nvmlInit_v2", NoArgs),
            shutdown: sym!(c"nvmlShutdown", NoArgs),
            count: sym!(c"nvmlDeviceGetCount_v2", CountFn),
            handle: sym!(c"nvmlDeviceGetHandleByIndex_v2", HandleFn),
            name: sym!(c"nvmlDeviceGetName", NameFn),
            utilization: sym!(c"nvmlDeviceGetUtilizationRates", UtilizationFn),
            memory: sym!(c"nvmlDeviceGetMemoryInfo", MemoryFn),
            temperature: sym!(c"nvmlDeviceGetTemperature", TemperatureFn),
            power: sym!(c"nvmlDeviceGetPowerUsage", PowerFn),
        })
    }
}

/// An initialised NVML with at least one GPU. Dropping it shuts NVML down
/// and unloads the library.
pub struct Nvml {
    api: Api,
    devices: Vec<(Device, String)>,
    // Dropped after `Drop::drop` has called nvmlShutdown.
    _lib: lib::Library,
}

impl Nvml {
    /// Any failure (no driver, a missing function, init error, no GPU) is
    /// an `Err` with a short reason; nothing panics.
    pub fn load() -> Result<Self, String> {
        let lib = lib::Library::open()?;
        let api = Api::resolve(&lib)?;
        // SAFETY: plain NVML calls with valid out-pointers.
        let code = unsafe { (api.init)() };
        if code != SUCCESS {
            return Err(format!("nvmlInit_v2 failed with code {code}"));
        }
        let mut nvml = Self {
            api,
            devices: Vec::new(),
            _lib: lib,
        };
        let mut count: c_uint = 0;
        // SAFETY: as above.
        if unsafe { (nvml.api.count)(&mut count) } != SUCCESS {
            return Err("NVML cannot count the GPUs".into());
        }
        for i in 0..count {
            let mut dev: Device = std::ptr::null_mut();
            // SAFETY: as above.
            if unsafe { (nvml.api.handle)(i, &mut dev) } != SUCCESS {
                continue;
            }
            let mut buf = [0 as c_char; NAME_LEN];
            // SAFETY: `buf` holds NAME_LEN bytes; NVML writes a nul-terminated name.
            let ok = unsafe { (nvml.api.name)(dev, buf.as_mut_ptr(), NAME_LEN as c_uint) };
            let name = if ok == SUCCESS {
                // SAFETY: NVML terminated the name inside `buf`.
                unsafe { CStr::from_ptr(buf.as_ptr()) }
                    .to_string_lossy()
                    .into_owned()
            } else {
                format!("NVIDIA GPU {i}")
            };
            nvml.devices.push((dev, name));
        }
        if nvml.devices.is_empty() {
            return Err("NVML found no NVIDIA GPU".into());
        }
        Ok(nvml)
    }

    pub fn names(&self) -> Vec<&str> {
        self.devices.iter().map(|(_, n)| n.as_str()).collect()
    }

    /// One reading per GPU; a value the GPU does not report is `None`.
    pub fn read(&self) -> Vec<GpuMetric> {
        let a = &self.api;
        self.devices
            .iter()
            .map(|(dev, name)| {
                let dev = *dev;
                let mut u = Utilization::default();
                let mut m = Memory::default();
                let (mut temp, mut mw): (c_uint, c_uint) = (0, 0);
                // SAFETY: valid device handles from this NVML session and
                // valid out-pointers.
                let (u_ok, m_ok, t_ok, p_ok) = unsafe {
                    (
                        (a.utilization)(dev, &mut u) == SUCCESS,
                        (a.memory)(dev, &mut m) == SUCCESS,
                        (a.temperature)(dev, TEMPERATURE_GPU, &mut temp) == SUCCESS,
                        (a.power)(dev, &mut mw) == SUCCESS,
                    )
                };
                GpuMetric {
                    name: name.clone(),
                    usage_pct: u_ok.then_some(u.gpu as f32),
                    mem_used_bytes: m_ok.then_some(m.used),
                    mem_total_bytes: m_ok.then_some(m.total),
                    temp_c: t_ok.then_some(temp as f32),
                    power_w: p_ok.then_some(mw as f32 / 1000.0),
                }
            })
            .collect()
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        // SAFETY: NVML was initialised in `load`.
        unsafe { (self.api.shutdown)() };
    }
}

#[cfg(windows)]
mod lib {
    use std::ffi::{CStr, c_void};
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        LoadLibraryExW,
    };

    pub struct Library(HMODULE);

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    impl Library {
        pub fn open() -> Result<Self, String> {
            // Only System32: a nvml.dll planted next to the program or in the
            // current folder is never loaded.
            let name = wide("nvml.dll".as_ref());
            // SAFETY: a nul-terminated wide string.
            let h = unsafe {
                LoadLibraryExW(
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
            };
            if !h.is_null() {
                return Ok(Self(h));
            }
            // Drivers before 2019 put it here instead.
            if let Some(pf) = std::env::var_os("ProgramFiles") {
                let path = std::path::Path::new(&pf).join(r"NVIDIA Corporation\NVSMI\nvml.dll");
                let path = wide(path.as_os_str());
                let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
                // SAFETY: a nul-terminated wide string.
                let h = unsafe { LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), flags) };
                if !h.is_null() {
                    return Ok(Self(h));
                }
            }
            Err("nvml.dll not found (no NVIDIA driver)".into())
        }

        pub fn symbol(&self, name: &CStr) -> Option<*const c_void> {
            // SAFETY: a loaded module and a nul-terminated name.
            unsafe { GetProcAddress(self.0, name.as_ptr().cast()) }.map(|f| f as *const c_void)
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: the handle came from LoadLibraryExW and is freed once.
            unsafe { FreeLibrary(self.0) };
        }
    }
}

#[cfg(unix)]
mod lib {
    use std::ffi::{CStr, c_void};

    pub struct Library(*mut c_void);

    impl Library {
        pub fn open() -> Result<Self, String> {
            // SAFETY: a nul-terminated name.
            let h = unsafe {
                libc::dlopen(
                    c"libnvidia-ml.so.1".as_ptr(),
                    libc::RTLD_NOW | libc::RTLD_LOCAL,
                )
            };
            if h.is_null() {
                Err("libnvidia-ml.so.1 not found (no NVIDIA driver)".into())
            } else {
                Ok(Self(h))
            }
        }

        pub fn symbol(&self, name: &CStr) -> Option<*const c_void> {
            // SAFETY: a loaded library and a nul-terminated name.
            let p = unsafe { libc::dlsym(self.0, name.as_ptr()) };
            (!p.is_null()).then_some(p as *const c_void)
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: the handle came from dlopen and is closed once.
            unsafe { libc::dlclose(self.0) };
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod lib {
    use std::ffi::{CStr, c_void};

    pub struct Library;

    impl Library {
        pub fn open() -> Result<Self, String> {
            Err("GPU readings are not supported on this system".into())
        }

        pub fn symbol(&self, _name: &CStr) -> Option<*const c_void> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_never_panics() {
        // With or without an NVIDIA driver: either GPUs with names or a reason.
        match Nvml::load() {
            Ok(nvml) => {
                let gpus = nvml.read();
                assert_eq!(gpus.len(), nvml.names().len());
                assert!(gpus.iter().all(|g| !g.name.is_empty()));
            }
            Err(reason) => assert!(!reason.is_empty()),
        }
    }
}
