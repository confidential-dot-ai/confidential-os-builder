//! The production [`Collect`] implementation: NVML, plus a background
//! thread that records the last critical XID per GPU.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::collector::Collect;
use crate::metrics::{self, ErrorLog, GpuDevice, GpuSource, NvlinkRates, Snapshot, Value};
use crate::nvml::{
    self, Clock, Device, EccCounter, EccError, Error, Memory, Nvml, RemappedRows,
    EVENT_XID_CRITICAL,
};

/// How long to wait before retrying a failed NVML load/init (driver not up
/// yet, or a GPU-less boot).
const INIT_RETRY: Duration = Duration::from_secs(30);
const XID_WAIT_MS: u32 = 5_000;

/// Last critical XID per GPU index. A GPU is present in the map only once
/// XID events are registered for it; its value is 0 until the first XID.
#[derive(Default)]
pub struct XidTable(Mutex<HashMap<u32, u64>>);

impl XidTable {
    fn registered(&self, index: u32) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(index)
            .or_insert(0);
    }

    fn record(&self, index: u32, xid: u64) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(index, xid);
    }

    fn get(&self, index: u32) -> Option<u64> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&index)
            .copied()
    }
}

pub struct NvmlBackend {
    library: String,
    nvml: Option<Arc<Nvml>>,
    retry_at: Instant,
    last_error: Option<String>,
    xid: Arc<XidTable>,
    nvlink: NvlinkRates,
    log: ErrorLog,
}

impl NvmlBackend {
    pub fn new(library: String) -> NvmlBackend {
        NvmlBackend {
            library,
            nvml: None,
            retry_at: Instant::now(),
            last_error: None,
            xid: Arc::default(),
            nvlink: NvlinkRates::default(),
            log: ErrorLog::default(),
        }
    }

    fn ensure_loaded(&mut self) {
        if self.nvml.is_some() || Instant::now() < self.retry_at {
            return;
        }
        match Nvml::load(&self.library) {
            Ok(nvml) => {
                let nvml = Arc::new(nvml);
                eprintln!("NVML initialised from {}", self.library);
                let (n, x) = (nvml.clone(), self.xid.clone());
                if let Err(e) = thread::Builder::new()
                    .name("xid-events".into())
                    .spawn(move || watch_xids(&n, &x))
                {
                    eprintln!("cannot start XID event thread: {e}; DCGM_FI_DEV_XID_ERRORS omitted");
                }
                self.nvml = Some(nvml);
            }
            Err(e) => {
                let message = e.to_string();
                if self.last_error.as_deref() != Some(&message) {
                    eprintln!(
                        "{message}; serving gpu_metrics_nvml_up 0, retrying every {}s",
                        INIT_RETRY.as_secs()
                    );
                    self.last_error = Some(message);
                }
                self.retry_at = Instant::now() + INIT_RETRY;
            }
        }
    }
}

impl Collect for NvmlBackend {
    fn start(&mut self) {
        // Load at boot, not on the first scrape, so XIDs are watched from
        // the start.
        self.ensure_loaded();
    }

    fn collect(&mut self) -> Snapshot {
        self.ensure_loaded();
        let Some(nvml) = &self.nvml else {
            return Snapshot::down();
        };
        let source = NvmlSource {
            nvml: nvml.clone(),
            xid: self.xid.clone(),
        };
        metrics::collect(&source, &mut self.nvlink, Instant::now(), &mut self.log)
    }
}

/// Register for critical XID events on every GPU and record each one.
/// Runs for the life of the process; a GPU whose registration fails simply
/// has no `DCGM_FI_DEV_XID_ERRORS` series.
fn watch_xids(nvml: &Nvml, table: &XidTable) {
    let set = match nvml.event_set_create() {
        Ok(set) => set,
        Err(e) => {
            eprintln!("nvmlEventSetCreate: {e}; DCGM_FI_DEV_XID_ERRORS omitted");
            return;
        }
    };
    let count = match nvml.device_count() {
        Ok(count) => count,
        Err(e) => {
            eprintln!("nvmlDeviceGetCount: {e}; DCGM_FI_DEV_XID_ERRORS omitted");
            return;
        }
    };
    let mut any = false;
    for index in 0..count {
        let registered = nvml.device_by_index(index).and_then(|dev| {
            let supported = nvml.supported_event_types(dev)?;
            if supported & EVENT_XID_CRITICAL == 0 {
                return Err(Error::NOT_SUPPORTED);
            }
            nvml.register_events(dev, EVENT_XID_CRITICAL, &set)
        });
        match registered {
            Ok(()) => {
                table.registered(index);
                any = true;
            }
            Err(e) => eprintln!(
                "gpu {index}: XID event registration: {e}; DCGM_FI_DEV_XID_ERRORS omitted"
            ),
        }
    }
    if !any {
        return;
    }
    let mut last_error = None;
    loop {
        match nvml.wait_event(&set, XID_WAIT_MS) {
            Ok(event) if event.event_type & EVENT_XID_CRITICAL != 0 => {
                match nvml.device_index(event.device) {
                    Ok(index) => {
                        eprintln!("gpu {index}: XID {}", event.data);
                        table.record(index, event.data);
                    }
                    Err(e) => eprintln!("XID {} on unknown GPU: {e}", event.data),
                }
            }
            Ok(_) | Err(Error::TIMEOUT) => {}
            Err(e) => {
                if last_error != Some(e) {
                    eprintln!("nvmlEventSetWait: {e}");
                    last_error = Some(e);
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

struct NvmlSource {
    nvml: Arc<Nvml>,
    xid: Arc<XidTable>,
}

impl GpuSource for NvmlSource {
    type Device = NvmlDevice;

    fn device_count(&self) -> Result<u32, Error> {
        self.nvml.device_count()
    }

    fn device(&self, index: u32) -> Result<NvmlDevice, Error> {
        Ok(NvmlDevice {
            nvml: self.nvml.clone(),
            dev: self.nvml.device_by_index(index)?,
        })
    }

    fn last_xid(&self, index: u32) -> Option<u64> {
        self.xid.get(index)
    }
}

struct NvmlDevice {
    nvml: Arc<Nvml>,
    dev: Device,
}

impl GpuDevice for NvmlDevice {
    fn uuid(&self) -> Result<String, Error> {
        self.nvml.uuid(self.dev)
    }

    fn utilization(&self) -> Result<(u32, u32), Error> {
        self.nvml.utilization(self.dev)
    }

    fn memory(&self) -> Result<Memory, Error> {
        self.nvml.memory(self.dev)
    }

    fn gpu_temperature(&self) -> Result<u32, Error> {
        self.nvml.gpu_temperature(self.dev)
    }

    fn memory_temperature(&self) -> Result<Value, Error> {
        Ok(
            match self.nvml.field(self.dev, nvml::FI_DEV_MEMORY_TEMP, 0)? {
                nvml::FieldNumber::Unsigned(v) => Value::Unsigned(v),
                nvml::FieldNumber::Signed(v) => Value::Signed(v),
                nvml::FieldNumber::Float(v) => Value::Float(v),
            },
        )
    }

    fn power_usage_mw(&self) -> Result<u32, Error> {
        self.nvml.power_usage_mw(self.dev)
    }

    fn total_energy_mj(&self) -> Result<u64, Error> {
        self.nvml.total_energy_mj(self.dev)
    }

    fn clock_mhz(&self, clock: Clock) -> Result<u32, Error> {
        self.nvml.clock_mhz(self.dev, clock)
    }

    fn clock_event_reasons(&self) -> Result<u64, Error> {
        self.nvml.clock_event_reasons(self.dev)
    }

    fn total_ecc_errors(&self, error: EccError, counter: EccCounter) -> Result<u64, Error> {
        self.nvml.total_ecc_errors(self.dev, error, counter)
    }

    fn remapped_rows(&self) -> Result<RemappedRows, Error> {
        self.nvml.remapped_rows(self.dev)
    }

    fn pcie_replay_counter(&self) -> Result<u32, Error> {
        self.nvml.pcie_replay_counter(self.dev)
    }

    /// The same read DCGM makes for this field: TX + RX data KiB with the
    /// all-links scope. NVIDIA disables these counters in some
    /// confidential-compute configurations; any error omits the series.
    fn nvlink_data_kib(&self) -> Result<u64, Error> {
        let as_u64 = |n| match n {
            nvml::FieldNumber::Unsigned(v) => Ok(v),
            nvml::FieldNumber::Signed(v) => u64::try_from(v).map_err(|_| Error::UNKNOWN),
            nvml::FieldNumber::Float(_) => Err(Error::UNKNOWN),
        };
        let tx = self
            .nvml
            .field(
                self.dev,
                nvml::FI_DEV_NVLINK_THROUGHPUT_DATA_TX,
                nvml::SCOPE_ALL_LINKS,
            )
            .and_then(as_u64)?;
        let rx = self
            .nvml
            .field(
                self.dev,
                nvml::FI_DEV_NVLINK_THROUGHPUT_DATA_RX,
                nvml::SCOPE_ALL_LINKS,
            )
            .and_then(as_u64)?;
        Ok(tx.saturating_add(rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_nvml_serves_down_and_backs_off() {
        let mut backend = NvmlBackend::new("libnvidia-ml-does-not-exist.so.1".into());
        backend.start();
        assert!(backend.nvml.is_none());
        assert!(backend.retry_at > Instant::now());
        let text = metrics::render(&backend.collect());
        assert!(text.contains("gpu_metrics_nvml_up 0\n"));
        assert!(text.contains("gpu_metrics_collect_timeout 0\n"));
    }

    #[test]
    fn xid_table_tracks_registration_and_last_value() {
        let table = XidTable::default();
        assert_eq!(table.get(0), None);
        table.registered(0);
        assert_eq!(table.get(0), Some(0));
        table.record(0, 48);
        table.record(0, 79);
        table.registered(0);
        assert_eq!(table.get(0), Some(79));
    }
}
