//! Boot measurements of a confos disk under cloud firmware.
//!
//! The firmware and the stubs log the same boot chain on every platform; what
//! differs is which events land in which register and with which hash:
//!
//! | Platform | Register | Events |
//! |---|---|---|
//! | TDX, GCE firmware | RTMR[1] (SHA-384) | [`BootDigests::tdx_rtmr1_events`] |
//! | TDX, GCE firmware | RTMR[2] (SHA-384) | systemd-stub section events only |
//! | Azure vTPM | PCR4 (SHA-256) | boot-application events, no GPT/ExitBootServices |
//! | Azure vTPM | PCR9 (SHA-256) | kernel EFI stub: command line, assembled initrd |
//! | Azure vTPM | PCR11 (SHA-256) | systemd-stub section events |
//!
//! On GCE the kernel EFI stub's command-line and initrd events are not in the
//! CC event log, so RTMR[2] holds only systemd-stub's section events. (The
//! stub logs through the TPM protocol when one is present and through the CC
//! protocol only otherwise; GCE TDX VMs have a vTPM.) The command line and
//! initrd are still covered by their UKI section events.
//!
//! Firmware-owned registers are not predicted: MRTD and RTMR[0] on GCE (MRTD
//! is Google's firmware, endorsed by Google's signed launch endorsement), and
//! the Azure paravisor's MRTD/RTMRs and PCR0-3/7. Verifiers take those from
//! the cloud provider, not from the image build.

use anyhow::{Context, Result};

use crate::alg::HashAlg;
use crate::{esp, pe, rtmr};

/// What the firmware loads from `\EFI\BOOT\BOOTX64.EFI`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootPath {
    /// systemd-boot, which then loads the UKI: two image loads.
    SystemdBoot,
    /// The UKI itself: one image load.
    DirectUki,
}

impl BootPath {
    pub fn as_str(self) -> &'static str {
        match self {
            BootPath::SystemdBoot => "systemd-boot",
            BootPath::DirectUki => "direct-uki",
        }
    }
}

/// The digests one boot of a disk produces, hashed with one algorithm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootDigests {
    pub alg: HashAlg,
    pub boot_path: BootPath,
    /// EV_EFI_GPT_EVENT.
    pub gpt: Vec<u8>,
    /// Authenticode of systemd-boot; `None` on the direct-UKI path.
    pub bootloader: Option<Vec<u8>>,
    /// Authenticode of the UKI.
    pub uki: Vec<u8>,
    /// systemd-stub: name then contents of each measured UKI section.
    pub uki_sections: Vec<Vec<u8>>,
    /// Kernel EFI stub: the command line (LoadOptions), then the initrd.
    pub kernel_stub: Vec<Vec<u8>>,
}

impl BootDigests {
    /// `uki` is the UKI the boot ends in; `disk` is the whole disk image.
    pub fn from_image(uki: &[u8], disk: &[u8], alg: HashAlg) -> Result<Self> {
        let first = esp::read_esp_file(disk, esp::FALLBACK_BOOTLOADER_PATH)
            .with_context(|| format!("reading {} from the disk", esp::FALLBACK_BOOTLOADER_PATH))?;
        let first_sections = pe::parse_sections(&first).context("parsing BOOTX64.EFI")?;
        let (boot_path, bootloader) = if first_sections.iter().any(|(name, _)| name == ".linux") {
            anyhow::ensure!(first == uki, "BOOTX64.EFI is a UKI, but not the UKI given");
            (BootPath::DirectUki, None)
        } else {
            let hash = pe::authenticode(&first, alg).context("Authenticode of systemd-boot")?;
            (BootPath::SystemdBoot, Some(hash))
        };
        let sections = pe::parse_sections(uki).context("parsing the UKI")?;
        let (mut uki_sections, mut kernel_stub) = (Vec::new(), Vec::new());
        for (kind, _, digest) in rtmr::stub_digests(&sections, alg)? {
            match kind.as_str() {
                "name" | "data" => uki_sections.push(digest),
                _ => kernel_stub.push(digest),
            }
        }
        Ok(BootDigests {
            alg,
            boot_path,
            gpt: alg.hash(&rtmr::gpt_event_data(disk)?),
            bootloader,
            uki: pe::authenticode(uki, alg).context("Authenticode of the UKI")?,
            uki_sections,
            kernel_stub,
        })
    }

    /// The firmware's boot-application events up to the last image load:
    /// "Calling EFI Application from Boot Option", the separator, then one
    /// EV_EFI_BOOT_SERVICES_APPLICATION per loaded PE. `gpt` adds the GPT
    /// event before the first load (TDX RTMR[1]; a TPM puts it in PCR5).
    fn boot_application_events(&self, gpt: bool) -> Vec<Vec<u8>> {
        let mut events = vec![
            self.alg.hash(b"Calling EFI Application from Boot Option"),
            self.alg.hash(&[0; 4]),
        ];
        if gpt {
            events.push(self.gpt.clone());
        }
        events.extend(self.bootloader.clone());
        events.push(self.uki.clone());
        events
    }

    /// RTMR[1] events as TDVF (confos OVMF) and GCE's firmware log them.
    pub fn tdx_rtmr1_events(&self) -> Vec<Vec<u8>> {
        let mut events = self.boot_application_events(true);
        events.push(self.alg.hash(b"Exit Boot Services Invocation"));
        events.push(self.alg.hash(b"Exit Boot Services Returned with Success"));
        events
    }
}

/// Expected RTMR[1]/RTMR[2] of the image on a GCE TDX VM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GcpTdx {
    pub rtmr1: Vec<u8>,
    pub rtmr2: Vec<u8>,
}

impl GcpTdx {
    pub fn from_digests(boot: &BootDigests) -> Self {
        assert_eq!(boot.alg, HashAlg::Sha384, "RTMRs are SHA-384");
        GcpTdx {
            rtmr1: boot.alg.extend(&boot.tdx_rtmr1_events()),
            rtmr2: boot.alg.extend(&boot.uki_sections),
        }
    }
}

/// Expected SHA-256 vTPM PCR4/PCR9/PCR11 of the image on an Azure CVM, before
/// userspace extends PCR11 (confos ships no systemd-pcrphase).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AzureVtpm {
    pub pcr4: Vec<u8>,
    pub pcr9: Vec<u8>,
    pub pcr11: Vec<u8>,
}

impl AzureVtpm {
    pub fn from_digests(boot: &BootDigests) -> Self {
        assert_eq!(
            boot.alg,
            HashAlg::Sha256,
            "the Azure vTPM bank read is SHA-256"
        );
        AzureVtpm {
            pcr4: boot.alg.extend(&boot.boot_application_events(false)),
            pcr9: boot.alg.extend(&boot.kernel_stub),
            pcr11: boot.alg.extend(&boot.uki_sections),
        }
    }
}

/// Both clouds' expectations for one disk and its UKI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudMeasurement {
    pub boot_path: BootPath,
    pub gcp_tdx: GcpTdx,
    pub azure_vtpm: AzureVtpm,
}

pub fn measure(uki: &[u8], disk: &[u8]) -> Result<CloudMeasurement> {
    let sha384 = BootDigests::from_image(uki, disk, HashAlg::Sha384)?;
    let sha256 = BootDigests::from_image(uki, disk, HashAlg::Sha256)?;
    Ok(CloudMeasurement {
        boot_path: sha384.boot_path,
        gcp_tdx: GcpTdx::from_digests(&sha384),
        azure_vtpm: AzureVtpm::from_digests(&sha256),
    })
}
