//! `gpu-metrics`: serve this guest's NVIDIA GPU metrics in Prometheus text
//! format on `GET /metrics` (port 9400 by default, as dcgm-exporter does).
//!
//! Reads NVML (`libnvidia-ml.so.1`, loaded at runtime) on every scrape, with
//! dcgm-exporter's metric names and units. Needs no network access, writes
//! nothing to disk, and keeps serving (`gpu_metrics_nvml_up 0`) when NVML or
//! the GPUs are absent. Shipped in the `gpu` mkosi profile and run by
//! `gpu-metrics.service`.

mod backend;
mod collector;
mod metrics;
mod nvml;
mod server;

use std::net::TcpListener;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

const USAGE: &str = "\
Usage: gpu-metrics [--listen ADDR:PORT] [--timeout-ms MS] [--nvml-library PATH]

Serve NVIDIA GPU metrics (dcgm-exporter names) at GET /metrics.

  --listen ADDR:PORT    address to bind (default [::]:9400, falling back to
                        0.0.0.0:9400 when IPv6 is unavailable)
  --timeout-ms MS       per-scrape deadline for reading NVML (default 5000)
  --nvml-library PATH   NVML library to load (default libnvidia-ml.so.1)
";

struct Args {
    listen: Option<String>,
    timeout: Duration,
    library: String,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut parsed = Args {
        listen: None,
        timeout: Duration::from_millis(5000),
        library: "libnvidia-ml.so.1".to_owned(),
    };
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(v.to_owned())),
            _ => (arg, None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(None),
            "-V" | "--version" => {
                println!("gpu-metrics {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--listen" => parsed.listen = Some(value()?),
            "--timeout-ms" => {
                let ms: u64 = value()?
                    .parse()
                    .map_err(|_| "--timeout-ms takes a number of milliseconds".to_owned())?;
                if ms == 0 {
                    return Err("--timeout-ms must be positive".into());
                }
                parsed.timeout = Duration::from_millis(ms);
            }
            "--nvml-library" => parsed.library = value()?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Some(parsed))
}

fn bind(listen: Option<&str>) -> std::io::Result<TcpListener> {
    match listen {
        Some(addr) => TcpListener::bind(addr),
        // [::] is dual-stack on Linux (net.ipv6.bindv6only=0).
        None => TcpListener::bind("[::]:9400").or_else(|_| TcpListener::bind("0.0.0.0:9400")),
    }
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("gpu-metrics: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let listener = match bind(args.listen.as_deref()) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("gpu-metrics: bind: {e}");
            return ExitCode::FAILURE;
        }
    };
    let collector =
        match collector::Collector::spawn(backend::NvmlBackend::new(args.library), args.timeout) {
            Ok(collector) => collector,
            Err(e) => {
                eprintln!("gpu-metrics: start collector: {e}");
                return ExitCode::FAILURE;
            }
        };
    if let Ok(addr) = listener.local_addr() {
        eprintln!("serving GPU metrics on http://{addr}/metrics");
    }
    let render = Arc::new(move || metrics::render(&collector.scrape()));
    match server::serve(listener, render) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gpu-metrics: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Args>, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults_and_overrides() {
        let a = parse(&[]).unwrap().unwrap();
        assert_eq!(a.listen, None);
        assert_eq!(a.timeout, Duration::from_millis(5000));
        assert_eq!(a.library, "libnvidia-ml.so.1");

        let a = parse(&[
            "--listen",
            "127.0.0.1:9",
            "--timeout-ms=250",
            "--nvml-library",
            "/x.so",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(a.listen.as_deref(), Some("127.0.0.1:9"));
        assert_eq!(a.timeout, Duration::from_millis(250));
        assert_eq!(a.library, "/x.so");

        assert!(parse(&["--help"]).unwrap().is_none());
        assert!(parse(&["--timeout-ms", "0"]).is_err());
        assert!(parse(&["--timeout-ms"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }
}
