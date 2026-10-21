//! Minimal runtime binding to NVIDIA's NVML (`libnvidia-ml.so.1`).
//!
//! The library is `dlopen`ed rather than linked so the exporter starts (and
//! reports NVML down) on a boot without the driver. Only the handful of
//! entry points the exporter reads are bound; every one except init and
//! enumeration is optional, and a missing symbol reads as
//! [`Error::FUNCTION_NOT_FOUND`] so that series is simply omitted.
//!
//! Layouts and constants follow `nvml.h` (driver r535 and later).

use std::ffi::{c_char, c_int, c_uint, c_ulong, c_ushort, c_void, CStr, CString};
use std::fmt;

type Return = c_int;
type RawDevice = *mut c_void;
type RawEventSet = *mut c_void;

/// An `nvmlReturn_t` other than `NVML_SUCCESS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Error(pub i32);

impl Error {
    pub const NOT_SUPPORTED: Error = Error(3);
    pub const NO_PERMISSION: Error = Error(4);
    #[cfg(test)]
    pub const DRIVER_NOT_LOADED: Error = Error(9);
    pub const TIMEOUT: Error = Error(10);
    pub const FUNCTION_NOT_FOUND: Error = Error(13);
    #[cfg(test)]
    pub const GPU_IS_LOST: Error = Error(15);
    pub const UNKNOWN: Error = Error(999);

    /// The device or driver does not offer this reading (for example a
    /// profiling counter in confidential-compute mode). Expected: the series
    /// is omitted, not an error.
    pub fn is_unsupported(self) -> bool {
        matches!(
            self,
            Error::NOT_SUPPORTED | Error::NO_PERMISSION | Error::FUNCTION_NOT_FOUND
        )
    }

    fn name(self) -> &'static str {
        match self.0 {
            1 => "UNINITIALIZED",
            2 => "INVALID_ARGUMENT",
            3 => "NOT_SUPPORTED",
            4 => "NO_PERMISSION",
            5 => "ALREADY_INITIALIZED",
            6 => "NOT_FOUND",
            7 => "INSUFFICIENT_SIZE",
            8 => "INSUFFICIENT_POWER",
            9 => "DRIVER_NOT_LOADED",
            10 => "TIMEOUT",
            11 => "IRQ_ISSUE",
            12 => "LIBRARY_NOT_FOUND",
            13 => "FUNCTION_NOT_FOUND",
            14 => "CORRUPTED_INFOROM",
            15 => "GPU_IS_LOST",
            16 => "RESET_REQUIRED",
            17 => "OPERATING_SYSTEM",
            18 => "LIB_RM_VERSION_MISMATCH",
            19 => "IN_USE",
            20 => "MEMORY",
            21 => "NO_DATA",
            22 => "VGPU_ECC_NOT_ENABLED",
            23 => "INSUFFICIENT_RESOURCES",
            24 => "FREQ_NOT_SUPPORTED",
            25 => "ARGUMENT_VERSION_MISMATCH",
            26 => "DEPRECATED",
            27 => "NOT_READY",
            28 => "GPU_NOT_FOUND",
            29 => "INVALID_STATE",
            _ => "UNKNOWN",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NVML_ERROR_{} ({})", self.name(), self.0)
    }
}

fn check(ret: Return) -> Result<(), Error> {
    if ret == 0 {
        Ok(())
    } else {
        Err(Error(ret))
    }
}

/// `nvmlClockType_t`.
#[derive(Clone, Copy, Debug)]
pub enum Clock {
    Sm = 1,
    Mem = 2,
}

/// `nvmlMemoryErrorType_t`: corrected (single-bit) or uncorrected (double-bit).
#[derive(Clone, Copy, Debug)]
pub enum EccError {
    Corrected = 0,
    Uncorrected = 1,
}

/// `nvmlEccCounterType_t`: since driver load, or over the GPU's lifetime.
#[derive(Clone, Copy, Debug)]
pub enum EccCounter {
    Volatile = 0,
    Aggregate = 1,
}

/// `nvmlEventTypeXidCriticalError`.
pub const EVENT_XID_CRITICAL: u64 = 0x8;
/// `NVML_FI_DEV_MEMORY_TEMP`: HBM temperature in C.
pub const FI_DEV_MEMORY_TEMP: u32 = 82;
/// `NVML_FI_DEV_NVLINK_THROUGHPUT_DATA_TX` / `_RX`: cumulative KiB.
pub const FI_DEV_NVLINK_THROUGHPUT_DATA_TX: u32 = 138;
pub const FI_DEV_NVLINK_THROUGHPUT_DATA_RX: u32 = 139;
/// NVLink field scope meaning "all links of the GPU" (what DCGM passes).
pub const SCOPE_ALL_LINKS: u32 = u32::MAX;
const UUID_BUFFER_SIZE: usize = 96;

#[repr(C)]
#[derive(Default)]
struct RawUtilization {
    gpu: c_uint,
    memory: c_uint,
}

/// `nvmlMemory_t`, in bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Memory {
    pub total: u64,
    pub free: u64,
    pub used: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
union RawValue {
    d: f64,
    si: c_int,
    ui: c_uint,
    ul: c_ulong,
    ull: u64,
    sll: i64,
    us: c_ushort,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawFieldValue {
    field_id: c_uint,
    scope_id: c_uint,
    timestamp: i64,
    latency_usec: i64,
    value_type: c_uint,
    nvml_return: Return,
    value: RawValue,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<RawFieldValue>() == 40);

#[repr(C)]
struct RawEventData {
    device: RawDevice,
    event_type: u64,
    event_data: u64,
    gpu_instance_id: c_uint,
    compute_instance_id: c_uint,
}

/// A numeric `nvmlFieldValue_t` payload.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldNumber {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
}

/// `nvmlDeviceGetRemappedRows` output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemappedRows {
    pub correctable: u32,
    pub uncorrectable: u32,
    pub pending: bool,
    pub failure: bool,
}

/// One delivered NVML event.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub device: Device,
    pub event_type: u64,
    pub data: u64,
}

/// An `nvmlDevice_t`. NVML handles stay valid until `nvmlShutdown`, which
/// this process never calls, and NVML is thread-safe.
#[derive(Clone, Copy, Debug)]
pub struct Device(RawDevice);
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

/// An `nvmlEventSet_t`.
pub struct EventSet(RawEventSet);
unsafe impl Send for EventSet {}

macro_rules! api {
    ($( $field:ident = $symbol:literal : fn($($arg:ty),*); )*) => {
        struct Api {
            $( $field: Option<unsafe extern "C" fn($($arg),*) -> Return>, )*
        }

        impl Api {
            /// # Safety
            /// `lib` must be a live `dlopen` handle for libnvidia-ml, whose
            /// symbols have the C signatures declared above.
            unsafe fn load(lib: *mut c_void) -> Api {
                Api {
                    $( $field: symbol(lib, concat!($symbol, "\0")).map(|ptr| {
                        std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) -> Return>(ptr)
                    }), )*
                }
            }
        }
    };
}

api! {
    init = "nvmlInit_v2": fn();
    device_get_count = "nvmlDeviceGetCount_v2": fn(*mut c_uint);
    device_get_handle_by_index = "nvmlDeviceGetHandleByIndex_v2": fn(c_uint, *mut RawDevice);
    device_get_index = "nvmlDeviceGetIndex": fn(RawDevice, *mut c_uint);
    device_get_uuid = "nvmlDeviceGetUUID": fn(RawDevice, *mut c_char, c_uint);
    device_get_utilization_rates = "nvmlDeviceGetUtilizationRates": fn(RawDevice, *mut RawUtilization);
    device_get_memory_info = "nvmlDeviceGetMemoryInfo": fn(RawDevice, *mut Memory);
    device_get_temperature = "nvmlDeviceGetTemperature": fn(RawDevice, c_uint, *mut c_uint);
    device_get_field_values = "nvmlDeviceGetFieldValues": fn(RawDevice, c_int, *mut RawFieldValue);
    device_get_power_usage = "nvmlDeviceGetPowerUsage": fn(RawDevice, *mut c_uint);
    device_get_total_energy_consumption = "nvmlDeviceGetTotalEnergyConsumption": fn(RawDevice, *mut u64);
    device_get_clock_info = "nvmlDeviceGetClockInfo": fn(RawDevice, c_uint, *mut c_uint);
    device_get_current_clocks_event_reasons = "nvmlDeviceGetCurrentClocksEventReasons": fn(RawDevice, *mut u64);
    device_get_current_clocks_throttle_reasons = "nvmlDeviceGetCurrentClocksThrottleReasons": fn(RawDevice, *mut u64);
    device_get_total_ecc_errors = "nvmlDeviceGetTotalEccErrors": fn(RawDevice, c_uint, c_uint, *mut u64);
    device_get_remapped_rows = "nvmlDeviceGetRemappedRows": fn(RawDevice, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint);
    device_get_pcie_replay_counter = "nvmlDeviceGetPcieReplayCounter": fn(RawDevice, *mut c_uint);
    device_get_supported_event_types = "nvmlDeviceGetSupportedEventTypes": fn(RawDevice, *mut u64);
    device_register_events = "nvmlDeviceRegisterEvents": fn(RawDevice, u64, RawEventSet);
    event_set_create = "nvmlEventSetCreate": fn(*mut RawEventSet);
    event_set_wait = "nvmlEventSetWait_v2": fn(RawEventSet, *mut RawEventData, c_uint);
}

/// # Safety
/// `lib` must be a live `dlopen` handle and `name` NUL-terminated.
unsafe fn symbol(lib: *mut c_void, name: &str) -> Option<*mut c_void> {
    let ptr = libc::dlsym(lib, name.as_ptr().cast());
    (!ptr.is_null()).then_some(ptr)
}

fn dlerror() -> String {
    // SAFETY: dlerror returns NULL or a NUL-terminated thread-local string.
    let ptr = unsafe { libc::dlerror() };
    if ptr.is_null() {
        "unknown dlopen error".to_owned()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

/// A loaded and initialised NVML.
///
/// Never shut down and never `dlclose`d: the exporter keeps one NVML client
/// for the life of the process (the same way DCGM does), so it does not
/// churn driver attach/detach on every scrape.
pub struct Nvml {
    api: Api,
}

// The API table holds plain function pointers into a library that stays
// mapped for the life of the process, and NVML is documented thread-safe.
unsafe impl Send for Nvml {}
unsafe impl Sync for Nvml {}

/// Why NVML could not be brought up.
#[derive(Debug)]
pub enum LoadError {
    Dlopen(String),
    MissingSymbol(&'static str),
    Init(Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Dlopen(e) => write!(f, "cannot load NVML: {e}"),
            LoadError::MissingSymbol(s) => write!(f, "NVML lacks required symbol {s}"),
            LoadError::Init(e) => write!(f, "nvmlInit_v2 failed: {e}"),
        }
    }
}

macro_rules! call {
    ($self:ident . $field:ident ( $($arg:expr),* )) => {{
        let f = $self.api.$field.ok_or(Error::FUNCTION_NOT_FOUND)?;
        // SAFETY: arguments match the nvml.h prototype bound in `api!`, and
        // every out-pointer refers to a live, correctly typed local.
        check(unsafe { f($($arg),*) })
    }};
}

impl Nvml {
    /// `dlopen` the library at `path` (a bare soname uses the loader's search
    /// path) and call `nvmlInit_v2`.
    pub fn load(path: &str) -> Result<Nvml, LoadError> {
        let c_path = CString::new(path).map_err(|_| LoadError::Dlopen("path has NUL".into()))?;
        // SAFETY: plain dlopen of a shared library by name.
        let lib = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if lib.is_null() {
            return Err(LoadError::Dlopen(dlerror()));
        }
        // SAFETY: lib is a live handle to libnvidia-ml (or a library that
        // exports the same names, which we must trust as the loader does).
        let api = unsafe { Api::load(lib) };
        for (present, name) in [
            (api.init.is_some(), "nvmlInit_v2"),
            (api.device_get_count.is_some(), "nvmlDeviceGetCount_v2"),
            (
                api.device_get_handle_by_index.is_some(),
                "nvmlDeviceGetHandleByIndex_v2",
            ),
            (api.device_get_uuid.is_some(), "nvmlDeviceGetUUID"),
        ] {
            if !present {
                return Err(LoadError::MissingSymbol(name));
            }
        }
        let nvml = Nvml { api };
        nvml.initialize().map_err(LoadError::Init)?;
        Ok(nvml)
    }

    fn initialize(&self) -> Result<(), Error> {
        call!(self.init())
    }

    pub fn device_count(&self) -> Result<u32, Error> {
        let mut count: c_uint = 0;
        call!(self.device_get_count(&mut count))?;
        Ok(count)
    }

    pub fn device_by_index(&self, index: u32) -> Result<Device, Error> {
        let mut raw: RawDevice = std::ptr::null_mut();
        call!(self.device_get_handle_by_index(index, &mut raw))?;
        Ok(Device(raw))
    }

    pub fn device_index(&self, device: Device) -> Result<u32, Error> {
        let mut index: c_uint = 0;
        call!(self.device_get_index(device.0, &mut index))?;
        Ok(index)
    }

    pub fn uuid(&self, device: Device) -> Result<String, Error> {
        let mut buf = [0 as c_char; UUID_BUFFER_SIZE];
        call!(self.device_get_uuid(device.0, buf.as_mut_ptr(), buf.len() as c_uint))?;
        // NVML NUL-terminates within the buffer; force it in case it did not.
        buf[UUID_BUFFER_SIZE - 1] = 0;
        // SAFETY: buf is NUL-terminated.
        Ok(unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned())
    }

    /// (GPU, memory-controller) busy percentages over the last sample period.
    pub fn utilization(&self, device: Device) -> Result<(u32, u32), Error> {
        let mut util = RawUtilization::default();
        call!(self.device_get_utilization_rates(device.0, &mut util))?;
        Ok((util.gpu, util.memory))
    }

    pub fn memory(&self, device: Device) -> Result<Memory, Error> {
        let mut mem = Memory::default();
        call!(self.device_get_memory_info(device.0, &mut mem))?;
        Ok(mem)
    }

    /// GPU die temperature in C (`NVML_TEMPERATURE_GPU`).
    pub fn gpu_temperature(&self, device: Device) -> Result<u32, Error> {
        let mut temp: c_uint = 0;
        call!(self.device_get_temperature(device.0, 0, &mut temp))?;
        Ok(temp)
    }

    /// One `nvmlDeviceGetFieldValues` reading.
    pub fn field(
        &self,
        device: Device,
        field_id: u32,
        scope_id: u32,
    ) -> Result<FieldNumber, Error> {
        let mut value = RawFieldValue {
            field_id,
            scope_id,
            timestamp: 0,
            latency_usec: 0,
            value_type: 0,
            nvml_return: 0,
            value: RawValue { ull: 0 },
        };
        call!(self.device_get_field_values(device.0, 1, &mut value))?;
        check(value.nvml_return)?;
        // SAFETY: value_type names the union member NVML wrote.
        let number = unsafe {
            match value.value_type {
                0 => FieldNumber::Float(value.value.d),
                1 => FieldNumber::Unsigned(u64::from(value.value.ui)),
                // c_ulong is 32 bits on some targets, 64 on Linux x86_64.
                #[allow(clippy::unnecessary_cast)]
                2 => FieldNumber::Unsigned(value.value.ul as u64),
                3 => FieldNumber::Unsigned(value.value.ull),
                4 => FieldNumber::Signed(value.value.sll),
                5 => FieldNumber::Signed(i64::from(value.value.si)),
                6 => FieldNumber::Unsigned(u64::from(value.value.us)),
                _ => return Err(Error::UNKNOWN),
            }
        };
        Ok(number)
    }

    /// Board power draw in milliwatts.
    pub fn power_usage_mw(&self, device: Device) -> Result<u32, Error> {
        let mut mw: c_uint = 0;
        call!(self.device_get_power_usage(device.0, &mut mw))?;
        Ok(mw)
    }

    /// Energy consumed since the driver was last reloaded, in millijoules.
    pub fn total_energy_mj(&self, device: Device) -> Result<u64, Error> {
        let mut mj: u64 = 0;
        call!(self.device_get_total_energy_consumption(device.0, &mut mj))?;
        Ok(mj)
    }

    /// Current clock in MHz.
    pub fn clock_mhz(&self, device: Device, clock: Clock) -> Result<u32, Error> {
        let mut mhz: c_uint = 0;
        call!(self.device_get_clock_info(device.0, clock as c_uint, &mut mhz))?;
        Ok(mhz)
    }

    /// `nvmlClocksEventReasons` bitmask (formerly "throttle reasons"). Falls
    /// back to the deprecated entry point on drivers without the new name.
    pub fn clock_event_reasons(&self, device: Device) -> Result<u64, Error> {
        let mut reasons: u64 = 0;
        if self.api.device_get_current_clocks_event_reasons.is_some() {
            call!(self.device_get_current_clocks_event_reasons(device.0, &mut reasons))?;
        } else {
            call!(self.device_get_current_clocks_throttle_reasons(device.0, &mut reasons))?;
        }
        Ok(reasons)
    }

    pub fn total_ecc_errors(
        &self,
        device: Device,
        error: EccError,
        counter: EccCounter,
    ) -> Result<u64, Error> {
        let mut count: u64 = 0;
        call!(self.device_get_total_ecc_errors(
            device.0,
            error as c_uint,
            counter as c_uint,
            &mut count
        ))?;
        Ok(count)
    }

    pub fn remapped_rows(&self, device: Device) -> Result<RemappedRows, Error> {
        let (mut corr, mut unc, mut pending, mut failure): (c_uint, c_uint, c_uint, c_uint) =
            (0, 0, 0, 0);
        call!(self.device_get_remapped_rows(
            device.0,
            &mut corr,
            &mut unc,
            &mut pending,
            &mut failure
        ))?;
        Ok(RemappedRows {
            correctable: corr,
            uncorrectable: unc,
            pending: pending != 0,
            failure: failure != 0,
        })
    }

    pub fn pcie_replay_counter(&self, device: Device) -> Result<u32, Error> {
        let mut count: c_uint = 0;
        call!(self.device_get_pcie_replay_counter(device.0, &mut count))?;
        Ok(count)
    }

    pub fn supported_event_types(&self, device: Device) -> Result<u64, Error> {
        let mut types: u64 = 0;
        call!(self.device_get_supported_event_types(device.0, &mut types))?;
        Ok(types)
    }

    pub fn event_set_create(&self) -> Result<EventSet, Error> {
        let mut set: RawEventSet = std::ptr::null_mut();
        call!(self.event_set_create(&mut set))?;
        Ok(EventSet(set))
    }

    pub fn register_events(&self, device: Device, types: u64, set: &EventSet) -> Result<(), Error> {
        call!(self.device_register_events(device.0, types, set.0))
    }

    /// Block up to `timeout_ms` for the next event on `set`
    /// ([`Error::TIMEOUT`] if none arrived).
    pub fn wait_event(&self, set: &EventSet, timeout_ms: u32) -> Result<Event, Error> {
        let mut data = RawEventData {
            device: std::ptr::null_mut(),
            event_type: 0,
            event_data: 0,
            gpu_instance_id: 0,
            compute_instance_id: 0,
        };
        call!(self.event_set_wait(set.0, &mut data, timeout_ms))?;
        Ok(Event {
            device: Device(data.device),
            event_type: data.event_type,
            data: data.event_data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_is_a_load_error_not_a_panic() {
        match Nvml::load("libnvidia-ml-does-not-exist.so.1") {
            Err(LoadError::Dlopen(msg)) => assert!(!msg.is_empty()),
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("loaded a library that does not exist"),
        }
    }

    #[test]
    fn unsupported_errors_are_classified() {
        assert!(Error::NOT_SUPPORTED.is_unsupported());
        assert!(Error::NO_PERMISSION.is_unsupported());
        assert!(Error::FUNCTION_NOT_FOUND.is_unsupported());
        assert!(!Error::GPU_IS_LOST.is_unsupported());
        assert_eq!(
            Error::NOT_SUPPORTED.to_string(),
            "NVML_ERROR_NOT_SUPPORTED (3)"
        );
    }
}
