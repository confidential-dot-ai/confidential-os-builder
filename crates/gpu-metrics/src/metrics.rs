//! What the exporter serves: the metric catalogue, one scrape's readings,
//! and their Prometheus text rendering.
//!
//! Names, units and help text follow NVIDIA dcgm-exporter's default counters
//! so stock DCGM dashboards and alerts work unchanged. Every device series
//! carries exactly two labels, `gpu` (NVML index) and `UUID`.
//!
//! Readings come from a [`GpuSource`]; production uses NVML, tests use a
//! fake. A reading the device does not support (NVML `NOT_SUPPORTED`,
//! `NO_PERMISSION`, or a missing entry point — e.g. counters NVIDIA disables
//! in confidential-compute mode) omits that one series; it never fails the
//! scrape.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::time::Instant;

use crate::nvml::{Clock, EccCounter, EccError, Error, Memory, RemappedRows};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Gauge,
    Counter,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Gauge => "gauge",
            Kind::Counter => "counter",
        }
    }
}

macro_rules! metrics {
    ($( $variant:ident => $name:literal, $kind:ident, $help:literal; )*) => {
        /// Device metric families, in output order.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Metric { $( $variant, )* }

        impl Metric {
            pub const ALL: &'static [Metric] = &[ $( Metric::$variant, )* ];

            pub fn name(self) -> &'static str {
                match self { $( Metric::$variant => $name, )* }
            }

            pub fn kind(self) -> Kind {
                match self { $( Metric::$variant => Kind::$kind, )* }
            }

            pub fn help(self) -> &'static str {
                match self { $( Metric::$variant => $help, )* }
            }
        }
    };
}

metrics! {
    GpuUtil => "DCGM_FI_DEV_GPU_UTIL", Gauge, "GPU utilization (in %).";
    MemCopyUtil => "DCGM_FI_DEV_MEM_COPY_UTIL", Gauge, "Memory utilization (in %).";
    FbUsed => "DCGM_FI_DEV_FB_USED", Gauge, "Framebuffer memory used (in MiB).";
    FbFree => "DCGM_FI_DEV_FB_FREE", Gauge, "Framebuffer memory free (in MiB).";
    GpuTemp => "DCGM_FI_DEV_GPU_TEMP", Gauge, "GPU temperature (in C).";
    MemoryTemp => "DCGM_FI_DEV_MEMORY_TEMP", Gauge, "Memory temperature (in C).";
    PowerUsage => "DCGM_FI_DEV_POWER_USAGE", Gauge, "Power draw (in W).";
    TotalEnergy => "DCGM_FI_DEV_TOTAL_ENERGY_CONSUMPTION", Counter, "Total energy consumption since boot (in mJ).";
    SmClock => "DCGM_FI_DEV_SM_CLOCK", Gauge, "SM clock frequency (in MHz).";
    MemClock => "DCGM_FI_DEV_MEM_CLOCK", Gauge, "Memory clock frequency (in MHz).";
    ClockThrottleReasons => "DCGM_FI_DEV_CLOCK_THROTTLE_REASONS", Gauge, "Clock throttle reasons bitmask (nvmlClocksEventReasons).";
    EccSbeVolTotal => "DCGM_FI_DEV_ECC_SBE_VOL_TOTAL", Counter, "Total number of single-bit volatile ECC errors.";
    EccDbeVolTotal => "DCGM_FI_DEV_ECC_DBE_VOL_TOTAL", Counter, "Total number of double-bit volatile ECC errors.";
    EccSbeAggTotal => "DCGM_FI_DEV_ECC_SBE_AGG_TOTAL", Counter, "Total number of single-bit persistent ECC errors.";
    EccDbeAggTotal => "DCGM_FI_DEV_ECC_DBE_AGG_TOTAL", Counter, "Total number of double-bit persistent ECC errors.";
    RowRemapFailure => "DCGM_FI_DEV_ROW_REMAP_FAILURE", Gauge, "Whether remapping of rows has failed.";
    UncorrectableRemappedRows => "DCGM_FI_DEV_UNCORRECTABLE_REMAPPED_ROWS", Counter, "Number of remapped rows for uncorrectable errors.";
    CorrectableRemappedRows => "DCGM_FI_DEV_CORRECTABLE_REMAPPED_ROWS", Counter, "Number of remapped rows for correctable errors.";
    PcieReplayCounter => "DCGM_FI_DEV_PCIE_REPLAY_COUNTER", Counter, "Total number of PCIe retries.";
    XidErrors => "DCGM_FI_DEV_XID_ERRORS", Gauge, "Value of the last XID error encountered.";
    NvlinkBandwidthTotal => "DCGM_FI_DEV_NVLINK_BANDWIDTH_TOTAL", Gauge, "Total NVLink data throughput (TX+RX) over all links since the previous reading (in MB/s, DCGM semantics).";
}

/// One sample value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
}

impl Value {
    fn write(self, out: &mut String) {
        let _ = match self {
            Value::Unsigned(v) => write!(out, "{v}"),
            Value::Signed(v) => write!(out, "{v}"),
            Value::Float(v) if v.is_nan() => write!(out, "NaN"),
            Value::Float(v) if v.is_infinite() => {
                write!(out, "{}", if v > 0.0 { "+Inf" } else { "-Inf" })
            }
            Value::Float(v) => write!(out, "{v}"),
        };
    }
}

/// One GPU's readings for a scrape.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuSample {
    pub index: u32,
    pub uuid: String,
    pub values: Vec<(Metric, Value)>,
}

/// Everything one scrape serves.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// NVML is loaded and initialised and enumerated at least one GPU.
    pub nvml_up: bool,
    /// Collection missed the per-scrape deadline; no device series served.
    pub timed_out: bool,
    /// GPUs NVML reported (before any were skipped for a failed UUID read).
    pub devices: u32,
    pub gpus: Vec<GpuSample>,
}

impl Snapshot {
    /// NVML unavailable (not loadable, not initialised, or no GPU).
    pub fn down() -> Snapshot {
        Snapshot {
            nvml_up: false,
            timed_out: false,
            devices: 0,
            gpus: Vec::new(),
        }
    }

    /// The collector did not answer within the per-scrape deadline.
    pub fn timed_out() -> Snapshot {
        Snapshot {
            timed_out: true,
            ..Snapshot::down()
        }
    }
}

/// Per-device readings. Each method mirrors one NVML call.
pub trait GpuDevice {
    fn uuid(&self) -> Result<String, Error>;
    /// (GPU, memory) utilization percent.
    fn utilization(&self) -> Result<(u32, u32), Error>;
    fn memory(&self) -> Result<Memory, Error>;
    fn gpu_temperature(&self) -> Result<u32, Error>;
    fn memory_temperature(&self) -> Result<Value, Error>;
    fn power_usage_mw(&self) -> Result<u32, Error>;
    fn total_energy_mj(&self) -> Result<u64, Error>;
    fn clock_mhz(&self, clock: Clock) -> Result<u32, Error>;
    fn clock_event_reasons(&self) -> Result<u64, Error>;
    fn total_ecc_errors(&self, error: EccError, counter: EccCounter) -> Result<u64, Error>;
    fn remapped_rows(&self) -> Result<RemappedRows, Error>;
    fn pcie_replay_counter(&self) -> Result<u32, Error>;
    /// Cumulative NVLink data KiB, TX + RX, over all links.
    fn nvlink_data_kib(&self) -> Result<u64, Error>;
}

/// Device enumeration plus the XID watcher's state.
pub trait GpuSource {
    type Device: GpuDevice;
    fn device_count(&self) -> Result<u32, Error>;
    fn device(&self, index: u32) -> Result<Self::Device, Error>;
    /// Last critical XID seen on `index` since boot (0 if none), or `None`
    /// when XID events could not be registered for that GPU.
    fn last_xid(&self, index: u32) -> Option<u64>;
}

/// Turns the cumulative NVLink counter into DCGM's
/// `DCGM_FI_DEV_NVLINK_BANDWIDTH_TOTAL`: the average rate between two
/// readings, KiB/s divided by 1000 ("MB/s"), 0 on the first reading or after
/// a counter reset.
#[derive(Default)]
pub struct NvlinkRates {
    previous: HashMap<String, (u64, Instant)>,
}

impl NvlinkRates {
    pub fn observe(&mut self, uuid: &str, total_kib: u64, now: Instant) -> u64 {
        let previous = self.previous.insert(uuid.to_owned(), (total_kib, now));
        match previous {
            Some((before, at)) if total_kib >= before => {
                let seconds = now.saturating_duration_since(at).as_secs_f64();
                if seconds > 0.0 {
                    ((total_kib - before) as f64 / seconds / 1000.0) as u64
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    fn retain(&mut self, live: &HashSet<String>) {
        self.previous.retain(|uuid, _| live.contains(uuid));
    }
}

/// Logs each distinct (metric, NVML error) pair once, so an unsupported
/// field explains its missing series in the journal without repeating
/// every scrape.
#[derive(Default)]
pub struct ErrorLog {
    seen: HashSet<(&'static str, i32)>,
    quiet: bool,
}

impl ErrorLog {
    #[cfg(test)]
    pub fn quiet() -> ErrorLog {
        ErrorLog {
            quiet: true,
            ..ErrorLog::default()
        }
    }

    pub fn record(&mut self, what: &'static str, index: u32, error: Error) {
        if self.seen.insert((what, error.0)) && !self.quiet {
            let note = if error.is_unsupported() {
                "series omitted"
            } else {
                "series omitted this scrape"
            };
            eprintln!("gpu {index}: {what}: {error}; {note}");
        }
    }
}

/// Read every GPU once.
pub fn collect<S: GpuSource>(
    source: &S,
    nvlink: &mut NvlinkRates,
    now: Instant,
    log: &mut ErrorLog,
) -> Snapshot {
    let count = match source.device_count() {
        Ok(count) => count,
        Err(error) => {
            log.record("nvmlDeviceGetCount", 0, error);
            return Snapshot::down();
        }
    };
    let mut gpus = Vec::with_capacity(count as usize);
    for index in 0..count {
        let device = match source.device(index) {
            Ok(device) => device,
            Err(error) => {
                log.record("nvmlDeviceGetHandleByIndex", index, error);
                continue;
            }
        };
        // Both labels are mandatory; a GPU without a readable UUID is left
        // out (and visible as gpu_metrics_devices exceeding the series count).
        let uuid = match device.uuid() {
            Ok(uuid) => uuid,
            Err(error) => {
                log.record("nvmlDeviceGetUUID", index, error);
                continue;
            }
        };
        let mut sample = GpuSample {
            index,
            uuid,
            values: Vec::new(),
        };
        read_device(&device, &mut sample, nvlink, now, log);
        if let Some(xid) = source.last_xid(index) {
            sample
                .values
                .push((Metric::XidErrors, Value::Unsigned(xid)));
        }
        gpus.push(sample);
    }
    let live = gpus.iter().map(|g| g.uuid.clone()).collect();
    nvlink.retain(&live);
    Snapshot {
        nvml_up: count > 0,
        timed_out: false,
        devices: count,
        gpus,
    }
}

fn read_device<D: GpuDevice>(
    device: &D,
    sample: &mut GpuSample,
    nvlink: &mut NvlinkRates,
    now: Instant,
    log: &mut ErrorLog,
) {
    let index = sample.index;
    let values = &mut sample.values;
    let mut put = |metric: Metric, reading: Result<Value, Error>| match reading {
        Ok(value) => values.push((metric, value)),
        Err(error) => log.record(metric.name(), index, error),
    };
    let unsigned = |v: u32| Value::Unsigned(u64::from(v));
    const MIB: u64 = 1024 * 1024;

    match device.utilization() {
        Ok((gpu, mem)) => {
            put(Metric::GpuUtil, Ok(unsigned(gpu)));
            put(Metric::MemCopyUtil, Ok(unsigned(mem)));
        }
        Err(e) => put(Metric::GpuUtil, Err(e)),
    }
    match device.memory() {
        Ok(mem) => {
            put(Metric::FbUsed, Ok(Value::Unsigned(mem.used / MIB)));
            put(Metric::FbFree, Ok(Value::Unsigned(mem.free / MIB)));
        }
        Err(e) => put(Metric::FbUsed, Err(e)),
    }
    put(Metric::GpuTemp, device.gpu_temperature().map(unsigned));
    put(Metric::MemoryTemp, device.memory_temperature());
    put(
        Metric::PowerUsage,
        device
            .power_usage_mw()
            .map(|mw| Value::Float(f64::from(mw) / 1000.0)),
    );
    put(
        Metric::TotalEnergy,
        device.total_energy_mj().map(Value::Unsigned),
    );
    put(Metric::SmClock, device.clock_mhz(Clock::Sm).map(unsigned));
    put(Metric::MemClock, device.clock_mhz(Clock::Mem).map(unsigned));
    put(
        Metric::ClockThrottleReasons,
        device.clock_event_reasons().map(Value::Unsigned),
    );
    for (metric, error, counter) in [
        (
            Metric::EccSbeVolTotal,
            EccError::Corrected,
            EccCounter::Volatile,
        ),
        (
            Metric::EccDbeVolTotal,
            EccError::Uncorrected,
            EccCounter::Volatile,
        ),
        (
            Metric::EccSbeAggTotal,
            EccError::Corrected,
            EccCounter::Aggregate,
        ),
        (
            Metric::EccDbeAggTotal,
            EccError::Uncorrected,
            EccCounter::Aggregate,
        ),
    ] {
        put(
            metric,
            device.total_ecc_errors(error, counter).map(Value::Unsigned),
        );
    }
    match device.remapped_rows() {
        Ok(rows) => {
            put(
                Metric::RowRemapFailure,
                Ok(Value::Unsigned(u64::from(rows.failure))),
            );
            put(
                Metric::UncorrectableRemappedRows,
                Ok(unsigned(rows.uncorrectable)),
            );
            put(
                Metric::CorrectableRemappedRows,
                Ok(unsigned(rows.correctable)),
            );
        }
        Err(e) => put(Metric::RowRemapFailure, Err(e)),
    }
    put(
        Metric::PcieReplayCounter,
        device.pcie_replay_counter().map(unsigned),
    );
    put(
        Metric::NvlinkBandwidthTotal,
        device
            .nvlink_data_kib()
            .map(|kib| Value::Unsigned(nvlink.observe(&sample.uuid, kib, now))),
    );
}

/// Prometheus label-value escaping (backslash, double quote, newline).
fn escape_label(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
}

fn family_header(out: &mut String, name: &str, kind: Kind, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {}", kind.as_str());
}

/// Render a snapshot in the Prometheus text exposition format (0.0.4).
/// Each family's samples are contiguous; families with no samples are left
/// out entirely.
pub fn render(snapshot: &Snapshot) -> String {
    let mut out = String::with_capacity(512 + snapshot.gpus.len() * 2048);
    for (name, help, value) in [
        (
            "gpu_metrics_nvml_up",
            "Whether NVML is loaded and reported at least one GPU for this scrape (1) or not (0).",
            u64::from(snapshot.nvml_up),
        ),
        (
            "gpu_metrics_collect_timeout",
            "Whether reading NVML missed this scrape's deadline (1); device series are then omitted.",
            u64::from(snapshot.timed_out),
        ),
        (
            "gpu_metrics_devices",
            "Number of GPUs NVML reported.",
            u64::from(snapshot.devices),
        ),
    ] {
        family_header(&mut out, name, Kind::Gauge, help);
        let _ = writeln!(out, "{name} {value}");
    }

    for &metric in Metric::ALL {
        let mut header = false;
        for gpu in &snapshot.gpus {
            for &(m, value) in &gpu.values {
                if m != metric {
                    continue;
                }
                if !header {
                    family_header(&mut out, metric.name(), metric.kind(), metric.help());
                    header = true;
                }
                let _ = write!(out, "{}{{gpu=\"{}\",UUID=\"", metric.name(), gpu.index);
                escape_label(&gpu.uuid, &mut out);
                out.push_str("\"} ");
                value.write(&mut out);
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::Duration;

    #[derive(Clone)]
    struct FakeGpu {
        uuid: Result<String, Error>,
        utilization: Result<(u32, u32), Error>,
        memory: Result<Memory, Error>,
        gpu_temp: Result<u32, Error>,
        memory_temp: Result<Value, Error>,
        power_mw: Result<u32, Error>,
        energy_mj: Result<u64, Error>,
        sm_clock: Result<u32, Error>,
        mem_clock: Result<u32, Error>,
        reasons: Result<u64, Error>,
        ecc: Result<[u64; 4], Error>,
        rows: Result<RemappedRows, Error>,
        replay: Result<u32, Error>,
        nvlink_kib: RefCell<Result<u64, Error>>,
    }

    impl FakeGpu {
        fn b300(n: u32) -> FakeGpu {
            FakeGpu {
                uuid: Ok(format!("GPU-0000000{n}-aaaa-bbbb-cccc-dddddddddddd")),
                utilization: Ok((97, 41)),
                memory: Ok(Memory {
                    total: 288 * 1024 * MIB,
                    free: 8 * 1024 * MIB + MIB / 2,
                    used: 280 * 1024 * MIB,
                }),
                gpu_temp: Ok(61),
                memory_temp: Ok(Value::Unsigned(55)),
                power_mw: Ok(1_012_345),
                energy_mj: Ok(123_456_789_012),
                sm_clock: Ok(1965),
                mem_clock: Ok(3996),
                reasons: Ok(0x4),
                ecc: Ok([3, 0, 17, 1]),
                rows: Ok(RemappedRows {
                    correctable: 2,
                    uncorrectable: 1,
                    pending: false,
                    failure: false,
                }),
                replay: Ok(9),
                nvlink_kib: RefCell::new(Err(Error::NOT_SUPPORTED)),
            }
        }
    }

    const MIB: u64 = 1024 * 1024;

    impl GpuDevice for FakeGpu {
        fn uuid(&self) -> Result<String, Error> {
            self.uuid.clone()
        }
        fn utilization(&self) -> Result<(u32, u32), Error> {
            self.utilization
        }
        fn memory(&self) -> Result<Memory, Error> {
            self.memory
        }
        fn gpu_temperature(&self) -> Result<u32, Error> {
            self.gpu_temp
        }
        fn memory_temperature(&self) -> Result<Value, Error> {
            self.memory_temp
        }
        fn power_usage_mw(&self) -> Result<u32, Error> {
            self.power_mw
        }
        fn total_energy_mj(&self) -> Result<u64, Error> {
            self.energy_mj
        }
        fn clock_mhz(&self, clock: Clock) -> Result<u32, Error> {
            match clock {
                Clock::Sm => self.sm_clock,
                Clock::Mem => self.mem_clock,
            }
        }
        fn clock_event_reasons(&self) -> Result<u64, Error> {
            self.reasons
        }
        fn total_ecc_errors(&self, error: EccError, counter: EccCounter) -> Result<u64, Error> {
            self.ecc.map(|e| e[(counter as usize) * 2 + error as usize])
        }
        fn remapped_rows(&self) -> Result<RemappedRows, Error> {
            self.rows
        }
        fn pcie_replay_counter(&self) -> Result<u32, Error> {
            self.replay
        }
        fn nvlink_data_kib(&self) -> Result<u64, Error> {
            *self.nvlink_kib.borrow()
        }
    }

    struct FakeSource {
        count: Result<u32, Error>,
        gpus: Vec<Result<FakeGpu, Error>>,
        xid: HashMap<u32, u64>,
    }

    impl GpuSource for FakeSource {
        type Device = FakeGpu;
        fn device_count(&self) -> Result<u32, Error> {
            self.count
        }
        fn device(&self, index: u32) -> Result<FakeGpu, Error> {
            self.gpus[index as usize].clone()
        }
        fn last_xid(&self, index: u32) -> Option<u64> {
            self.xid.get(&index).copied()
        }
    }

    fn source(gpus: Vec<FakeGpu>) -> FakeSource {
        FakeSource {
            count: Ok(gpus.len() as u32),
            xid: (0..gpus.len() as u32).map(|i| (i, 0)).collect(),
            gpus: gpus.into_iter().map(Ok).collect(),
        }
    }

    fn scrape(src: &FakeSource) -> String {
        render(&collect(
            src,
            &mut NvlinkRates::default(),
            Instant::now(),
            &mut ErrorLog::quiet(),
        ))
    }

    #[test]
    fn renders_every_dcgm_series_with_gpu_and_uuid_labels() {
        let mut src = source(vec![FakeGpu::b300(0)]);
        src.xid.insert(0, 79);
        let text = scrape(&src);
        let expected = r#"# HELP gpu_metrics_nvml_up Whether NVML is loaded and reported at least one GPU for this scrape (1) or not (0).
# TYPE gpu_metrics_nvml_up gauge
gpu_metrics_nvml_up 1
# HELP gpu_metrics_collect_timeout Whether reading NVML missed this scrape's deadline (1); device series are then omitted.
# TYPE gpu_metrics_collect_timeout gauge
gpu_metrics_collect_timeout 0
# HELP gpu_metrics_devices Number of GPUs NVML reported.
# TYPE gpu_metrics_devices gauge
gpu_metrics_devices 1
# HELP DCGM_FI_DEV_GPU_UTIL GPU utilization (in %).
# TYPE DCGM_FI_DEV_GPU_UTIL gauge
DCGM_FI_DEV_GPU_UTIL{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 97
# HELP DCGM_FI_DEV_MEM_COPY_UTIL Memory utilization (in %).
# TYPE DCGM_FI_DEV_MEM_COPY_UTIL gauge
DCGM_FI_DEV_MEM_COPY_UTIL{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 41
# HELP DCGM_FI_DEV_FB_USED Framebuffer memory used (in MiB).
# TYPE DCGM_FI_DEV_FB_USED gauge
DCGM_FI_DEV_FB_USED{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 286720
# HELP DCGM_FI_DEV_FB_FREE Framebuffer memory free (in MiB).
# TYPE DCGM_FI_DEV_FB_FREE gauge
DCGM_FI_DEV_FB_FREE{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 8192
# HELP DCGM_FI_DEV_GPU_TEMP GPU temperature (in C).
# TYPE DCGM_FI_DEV_GPU_TEMP gauge
DCGM_FI_DEV_GPU_TEMP{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 61
# HELP DCGM_FI_DEV_MEMORY_TEMP Memory temperature (in C).
# TYPE DCGM_FI_DEV_MEMORY_TEMP gauge
DCGM_FI_DEV_MEMORY_TEMP{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 55
# HELP DCGM_FI_DEV_POWER_USAGE Power draw (in W).
# TYPE DCGM_FI_DEV_POWER_USAGE gauge
DCGM_FI_DEV_POWER_USAGE{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 1012.345
# HELP DCGM_FI_DEV_TOTAL_ENERGY_CONSUMPTION Total energy consumption since boot (in mJ).
# TYPE DCGM_FI_DEV_TOTAL_ENERGY_CONSUMPTION counter
DCGM_FI_DEV_TOTAL_ENERGY_CONSUMPTION{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 123456789012
# HELP DCGM_FI_DEV_SM_CLOCK SM clock frequency (in MHz).
# TYPE DCGM_FI_DEV_SM_CLOCK gauge
DCGM_FI_DEV_SM_CLOCK{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 1965
# HELP DCGM_FI_DEV_MEM_CLOCK Memory clock frequency (in MHz).
# TYPE DCGM_FI_DEV_MEM_CLOCK gauge
DCGM_FI_DEV_MEM_CLOCK{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 3996
# HELP DCGM_FI_DEV_CLOCK_THROTTLE_REASONS Clock throttle reasons bitmask (nvmlClocksEventReasons).
# TYPE DCGM_FI_DEV_CLOCK_THROTTLE_REASONS gauge
DCGM_FI_DEV_CLOCK_THROTTLE_REASONS{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 4
# HELP DCGM_FI_DEV_ECC_SBE_VOL_TOTAL Total number of single-bit volatile ECC errors.
# TYPE DCGM_FI_DEV_ECC_SBE_VOL_TOTAL counter
DCGM_FI_DEV_ECC_SBE_VOL_TOTAL{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 3
# HELP DCGM_FI_DEV_ECC_DBE_VOL_TOTAL Total number of double-bit volatile ECC errors.
# TYPE DCGM_FI_DEV_ECC_DBE_VOL_TOTAL counter
DCGM_FI_DEV_ECC_DBE_VOL_TOTAL{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 0
# HELP DCGM_FI_DEV_ECC_SBE_AGG_TOTAL Total number of single-bit persistent ECC errors.
# TYPE DCGM_FI_DEV_ECC_SBE_AGG_TOTAL counter
DCGM_FI_DEV_ECC_SBE_AGG_TOTAL{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 17
# HELP DCGM_FI_DEV_ECC_DBE_AGG_TOTAL Total number of double-bit persistent ECC errors.
# TYPE DCGM_FI_DEV_ECC_DBE_AGG_TOTAL counter
DCGM_FI_DEV_ECC_DBE_AGG_TOTAL{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 1
# HELP DCGM_FI_DEV_ROW_REMAP_FAILURE Whether remapping of rows has failed.
# TYPE DCGM_FI_DEV_ROW_REMAP_FAILURE gauge
DCGM_FI_DEV_ROW_REMAP_FAILURE{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 0
# HELP DCGM_FI_DEV_UNCORRECTABLE_REMAPPED_ROWS Number of remapped rows for uncorrectable errors.
# TYPE DCGM_FI_DEV_UNCORRECTABLE_REMAPPED_ROWS counter
DCGM_FI_DEV_UNCORRECTABLE_REMAPPED_ROWS{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 1
# HELP DCGM_FI_DEV_CORRECTABLE_REMAPPED_ROWS Number of remapped rows for correctable errors.
# TYPE DCGM_FI_DEV_CORRECTABLE_REMAPPED_ROWS counter
DCGM_FI_DEV_CORRECTABLE_REMAPPED_ROWS{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 2
# HELP DCGM_FI_DEV_PCIE_REPLAY_COUNTER Total number of PCIe retries.
# TYPE DCGM_FI_DEV_PCIE_REPLAY_COUNTER counter
DCGM_FI_DEV_PCIE_REPLAY_COUNTER{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 9
# HELP DCGM_FI_DEV_XID_ERRORS Value of the last XID error encountered.
# TYPE DCGM_FI_DEV_XID_ERRORS gauge
DCGM_FI_DEV_XID_ERRORS{gpu="0",UUID="GPU-00000000-aaaa-bbbb-cccc-dddddddddddd"} 79
"#;
        assert_eq!(text, expected);
    }

    #[test]
    fn families_stay_contiguous_across_eight_gpus() {
        let src = source((0..8).map(FakeGpu::b300).collect());
        let text = scrape(&src);
        assert!(text.contains("gpu_metrics_devices 8\n"));
        for &metric in Metric::ALL {
            if metric == Metric::NvlinkBandwidthTotal {
                continue;
            }
            let lines: Vec<usize> = text
                .lines()
                .enumerate()
                .filter(|(_, l)| l.starts_with(&format!("{}{{", metric.name())))
                .map(|(i, _)| i)
                .collect();
            assert_eq!(lines.len(), 8, "{}", metric.name());
            assert_eq!(lines[7] - lines[0], 7, "{} not contiguous", metric.name());
            assert_eq!(
                text.matches(&format!("# TYPE {} ", metric.name())).count(),
                1
            );
        }
        assert!(text.contains(
            "DCGM_FI_DEV_GPU_TEMP{gpu=\"7\",UUID=\"GPU-00000007-aaaa-bbbb-cccc-dddddddddddd\"} 61\n"
        ));
    }

    #[test]
    fn unsupported_fields_are_omitted_without_failing_the_scrape() {
        let mut gpu = FakeGpu::b300(0);
        gpu.memory_temp = Err(Error::NOT_SUPPORTED);
        gpu.utilization = Err(Error::NO_PERMISSION);
        gpu.ecc = Err(Error::NOT_SUPPORTED);
        gpu.rows = Err(Error::FUNCTION_NOT_FOUND);
        gpu.power_mw = Err(Error::GPU_IS_LOST);
        let mut src = source(vec![gpu]);
        src.xid.clear();
        let text = scrape(&src);
        assert!(text.contains("gpu_metrics_nvml_up 1\n"));
        for gone in [
            "DCGM_FI_DEV_MEMORY_TEMP",
            "DCGM_FI_DEV_GPU_UTIL",
            "DCGM_FI_DEV_MEM_COPY_UTIL",
            "DCGM_FI_DEV_ECC_SBE_VOL_TOTAL",
            "DCGM_FI_DEV_ECC_DBE_AGG_TOTAL",
            "DCGM_FI_DEV_ROW_REMAP_FAILURE",
            "DCGM_FI_DEV_CORRECTABLE_REMAPPED_ROWS",
            "DCGM_FI_DEV_POWER_USAGE",
            "DCGM_FI_DEV_XID_ERRORS",
            "DCGM_FI_DEV_NVLINK_BANDWIDTH_TOTAL",
        ] {
            assert!(!text.contains(gone), "{gone} should be omitted:\n{text}");
        }
        assert!(text.contains("DCGM_FI_DEV_GPU_TEMP{gpu=\"0\""));
        assert!(text.contains("DCGM_FI_DEV_FB_USED{gpu=\"0\""));
    }

    #[test]
    fn nvml_down_and_timeout_serve_only_status() {
        let down = render(&Snapshot::down());
        assert!(down.contains("gpu_metrics_nvml_up 0\n"));
        assert!(down.contains("gpu_metrics_collect_timeout 0\n"));
        assert!(!down.contains("DCGM_"));
        let late = render(&Snapshot::timed_out());
        assert!(late.contains("gpu_metrics_nvml_up 0\n"));
        assert!(late.contains("gpu_metrics_collect_timeout 1\n"));
        assert!(!late.contains("DCGM_"));

        let mut src = source(vec![]);
        assert!(scrape(&src).contains("gpu_metrics_nvml_up 0\n"));
        src.count = Err(Error::DRIVER_NOT_LOADED);
        assert!(scrape(&src).contains("gpu_metrics_nvml_up 0\n"));
    }

    #[test]
    fn gpu_without_uuid_is_skipped_but_counted() {
        let mut broken = FakeGpu::b300(1);
        broken.uuid = Err(Error::GPU_IS_LOST);
        let mut src = source(vec![FakeGpu::b300(0), broken, FakeGpu::b300(2)]);
        src.gpus.push(Err(Error::UNKNOWN));
        src.count = Ok(4);
        let text = scrape(&src);
        assert!(text.contains("gpu_metrics_devices 4\n"));
        assert!(text.contains("DCGM_FI_DEV_GPU_TEMP{gpu=\"0\""));
        assert!(!text.contains("{gpu=\"1\""));
        assert!(text.contains("DCGM_FI_DEV_GPU_TEMP{gpu=\"2\""));
        assert!(!text.contains("{gpu=\"3\""));
    }

    #[test]
    fn nvlink_total_is_a_rate_like_dcgm() {
        let gpu = FakeGpu::b300(0);
        *gpu.nvlink_kib.borrow_mut() = Ok(1_000_000);
        let src = source(vec![gpu]);
        let mut rates = NvlinkRates::default();
        let mut log = ErrorLog::quiet();
        let t0 = Instant::now();
        let first = render(&collect(&src, &mut rates, t0, &mut log));
        assert!(first.contains("# TYPE DCGM_FI_DEV_NVLINK_BANDWIDTH_TOTAL gauge\n"));
        assert!(first.contains("DCGM_FI_DEV_NVLINK_BANDWIDTH_TOTAL{gpu=\"0\",UUID=\"GPU-00000000-aaaa-bbbb-cccc-dddddddddddd\"} 0\n"));

        // 50,000,000 KiB over 10 s = 5,000,000 KiB/s = 5000 "MB/s".
        let Ok(dev) = &src.gpus[0] else {
            unreachable!()
        };
        *dev.nvlink_kib.borrow_mut() = Ok(51_000_000);
        let second = render(&collect(
            &src,
            &mut rates,
            t0 + Duration::from_secs(10),
            &mut log,
        ));
        assert!(second.contains("DCGM_FI_DEV_NVLINK_BANDWIDTH_TOTAL{gpu=\"0\",UUID=\"GPU-00000000-aaaa-bbbb-cccc-dddddddddddd\"} 5000\n"));

        // Counter reset reads as 0, not a huge wrap.
        *dev.nvlink_kib.borrow_mut() = Ok(10);
        let third = render(&collect(
            &src,
            &mut rates,
            t0 + Duration::from_secs(20),
            &mut log,
        ));
        assert!(third.contains("} 0\n"));
    }

    #[test]
    fn label_values_are_escaped() {
        let mut gpu = FakeGpu::b300(0);
        gpu.uuid = Ok("we\"ird\\uu\nid".into());
        let text = scrape(&source(vec![gpu]));
        assert!(text.contains(r#"DCGM_FI_DEV_GPU_TEMP{gpu="0",UUID="we\"ird\\uu\nid"} 61"#));
    }

    #[test]
    fn float_values_render_in_prometheus_syntax() {
        let mut out = String::new();
        for v in [
            Value::Float(250.0),
            Value::Float(0.5),
            Value::Float(f64::NAN),
            Value::Float(f64::INFINITY),
            Value::Signed(-3),
        ] {
            v.write(&mut out);
            out.push(' ');
        }
        assert_eq!(out, "250 0.5 NaN +Inf -3 ");
    }
}
