//! OCI `createContainer` hook: measure the workload container image into TDX
//! **RTMR[3]** so a `kata-qemu-tdx` guest's attestation binds *which* container
//! it is running — not just the measured base image.
//!
//! Baked into the kata-guest-tdx dm-verity rootfs (so the hook binary is itself
//! measured into MRTD/RTMR[2] and cannot be shadowed), and run by kata-agent
//! before the workload process starts. The measurement is taken by trusted,
//! measured code *before* the workload gets control, and RTMR[3] is
//! hardware-append-only — so a compromised workload cannot alter it.
//!
//! ## Extend convention (MUST match `tdx-measure rtmr3-from-images`)
//! Per workload container, in creation order:
//!   event_digest = SHA384("sha256:" + lowercase-hex)
//!   RTMR[3]      = extend(RTMR[3], event_digest)   // kernel: SHA384(RTMR3 ‖ event_digest)
//!
//! ## Requirements
//! - The pod must be **deployed by digest** (`image: repo@sha256:…`). A tag is
//!   not content-bound, so the hook **fails closed** on one (the container will
//!   not start).
//! - The kata config must forward the image annotation into the guest, i.e. add
//!   `io.kubernetes.cri.image-name` to `container_annotations`.

use std::io::Read;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha384};

/// Linux TSM sysfs RTMR-extend interface (same one the CoCo attestation-agent
/// uses). Writing a 48-byte SHA-384 triggers TDG.MR.RTMR.EXTEND on RTMR[3].
const RTMR3_SYSFS: &str = "/sys/devices/virtual/misc/tdx_guest/measurements/rtmr3:sha384";

fn main() -> Result<()> {
    // OCI hooks receive the container state as JSON on stdin.
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("read OCI state from stdin")?;
    let state: Value = serde_json::from_str(&input).context("parse OCI state JSON")?;

    let annotations = load_annotations(&state)?;
    let get = |k: &str| annotations.get(k).and_then(Value::as_str);

    // Measure only real workload containers, not the pause/sandbox infra container.
    if get("io.kubernetes.cri.container-type") != Some("container") {
        return Ok(());
    }

    let image = get("io.kubernetes.cri.image-name").context(
        "missing io.kubernetes.cri.image-name annotation \
         (add it to the kata `container_annotations` allowlist)",
    )?;
    let event_digest = workload_event_digest(image)?;

    // The write performs the RTMR extend; RTMR[3] = SHA384(RTMR[3] ‖ event_digest).
    std::fs::write(RTMR3_SYSFS, event_digest)
        .with_context(|| format!("extend RTMR[3] via {RTMR3_SYSFS}"))?;

    Ok(())
}

/// Annotations may be carried on the OCI state directly (runtime-spec ≥ 1.0.2)
/// or only in the bundle's `config.json`. Prefer the state; fall back to the file.
fn load_annotations(state: &Value) -> Result<Value> {
    if let Some(a) = state.get("annotations").filter(|a| a.is_object()) {
        return Ok(a.clone());
    }
    let bundle = state
        .get("bundle")
        .and_then(Value::as_str)
        .context("OCI state has neither annotations nor a bundle path")?;
    let cfg = std::fs::read_to_string(format!("{bundle}/config.json"))
        .with_context(|| format!("read {bundle}/config.json"))?;
    Ok(serde_json::from_str::<Value>(&cfg)?
        .get("annotations")
        .cloned()
        .unwrap_or(Value::Null))
}

/// SHA-384 of the canonical `sha256:<hex>` digest — the per-container event that
/// gets extended into RTMR[3]. **Fails closed** on an unpinned (tag) image.
fn workload_event_digest(image: &str) -> Result<[u8; 48]> {
    let digest = image.split_once('@').map(|(_, d)| d).unwrap_or(image);
    let hex = digest.strip_prefix("sha256:").with_context(|| {
        format!("image '{image}' is not digest-pinned (…@sha256:<hex>); refusing to measure a tag")
    })?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("malformed sha256 digest in image '{image}'");
    }
    let canonical = format!("sha256:{}", hex.to_ascii_lowercase());
    Ok(Sha384::digest(canonical.as_bytes()).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Sha384;

    // extend(0, event): must equal tdx-measure's golden RTMR[3] for one container.
    #[test]
    fn event_digest_matches_tdx_measure_convention() {
        let ev = workload_event_digest(
            "docker.io/library/busybox@sha256:9532d8c39891ca2ecde4d30d7710e01fb739c87a8b9299685c63704296b16028",
        )
        .unwrap();
        let mut h = Sha384::new();
        h.update([0u8; 48]);
        h.update(ev);
        assert_eq!(
            hex::encode(h.finalize()),
            // == `tdx-measure rtmr3-from-images --digest sha256:9532…`
            "1ad70a34f3ac77a222e512c44691d55cc10f9929ac602be81f8aa42f15013fac4da2231f67176b05ff670f1f8f7a7e21"
        );
    }

    #[test]
    fn fails_closed_on_tag() {
        assert!(workload_event_digest("busybox:1.37").is_err());
        assert!(workload_event_digest("busybox").is_err());
        assert!(workload_event_digest("repo@sha256:deadbeef").is_err());
    }
}
