use std::{os::unix::process::CommandExt as _, path::PathBuf, process::Command};

use crate::tools;

/// Validate a QEMU memory string (e.g. "4G", "512M").
/// Rejects values containing commas or other characters that could inject
/// additional properties into QEMU's -object comma-delimited argument format.
pub fn validate_memory(s: &str) -> anyhow::Result<()> {
    // Must be digits optionally followed by a single size suffix
    let valid = !s.is_empty()
        && s.bytes().last().is_some_and(|last| {
            let suffix = b"KMGTkmgt";
            if suffix.contains(&last) {
                s[..s.len() - 1].bytes().all(|b| b.is_ascii_digit())
            } else {
                s.bytes().all(|b| b.is_ascii_digit())
            }
        });
    if !valid {
        anyhow::bail!(
            "invalid memory format: {:?} (expected digits with optional K/M/G/T suffix)",
            s
        );
    }
    Ok(())
}

/// Parse a human/QEMU-style size string (e.g. "20G", "512M") into bytes.
/// Suffixes K/M/G/T are powers of 1024 (case-insensitive); no suffix = bytes.
pub fn parse_size_to_bytes(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty size");
    }
    let last = s.as_bytes()[s.len() - 1];
    let (num, mult): (&str, u64) = match last {
        b'K' | b'k' => (&s[..s.len() - 1], 1024),
        b'M' | b'm' => (&s[..s.len() - 1], 1024 * 1024),
        b'G' | b'g' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        b'T' | b't' => (&s[..s.len() - 1], 1024u64 * 1024 * 1024 * 1024),
        b'0'..=b'9' => (s, 1),
        _ => anyhow::bail!("invalid size suffix in {s:?} (use K/M/G/T or plain bytes)"),
    };
    let value: u64 = num
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid size number in {s:?}"))?;
    value
        .checked_mul(mult)
        .ok_or_else(|| anyhow::anyhow!("size too large: {s:?}"))
}

/// The detected QEMU capability tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QemuTier {
    /// Full SEV-SNP + IGVM (confidential computing with measured boot).
    SevSnp,
    /// KVM acceleration only (no confidential computing).
    Kvm,
    /// Pure software emulation (no KVM, no SEV-SNP).
    Emulated,
}

/// Select the best tier from parsed QEMU capabilities.
pub fn select_tier(object_help_output: &str, kvm_available: bool) -> QemuTier {
    let has_sev_snp = object_help_output.contains("sev-snp-guest");
    let has_igvm = object_help_output.contains("igvm-cfg");

    if has_sev_snp && has_igvm && kvm_available {
        QemuTier::SevSnp
    } else if kvm_available {
        QemuTier::Kvm
    } else {
        QemuTier::Emulated
    }
}

/// Detect the best available QEMU tier for a specific binary.
/// For SNP, we check QEMU capabilities + /dev/kvm existence (not writability,
/// since we'll run with sudo anyway for /dev/sev access).
pub fn detect_tier_for(qemu_bin: &str) -> anyhow::Result<QemuTier> {
    let resolved = if std::path::Path::new(qemu_bin).exists() {
        qemu_bin.to_string()
    } else {
        // If it's a bare name like "qemu-system-x86_64", resolve via PATH
        let path = tools::require(qemu_bin)?;
        path.to_string_lossy().to_string()
    };

    let object_help = tools::run_command(&resolved, &["-object", "help"])?;

    // Check /dev/kvm exists (not writability — sudo handles that for SNP)
    let kvm_available = std::path::Path::new("/dev/kvm").exists();

    Ok(select_tier(&object_help, kvm_available))
}

/// Format an argv as a shell-pasteable string, single-quoting any arg that
/// contains whitespace or shell metacharacters.
fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            let needs_quote = a.is_empty()
                || a.bytes()
                    .any(|b| !(b.is_ascii_alphanumeric() || b"@%_-+=:,./".contains(&b)));
            if needs_quote {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reject paths containing commas — QEMU uses comma-delimited key=value in
/// -object/-drive args, so a comma in a path injects additional properties.
fn reject_comma_in_path(label: &str, path: &std::path::Path) -> anyhow::Result<()> {
    if path.to_string_lossy().contains(',') {
        anyhow::bail!(
            "{label} path contains a comma, which would be misinterpreted by QEMU: {}",
            path.display()
        );
    }
    Ok(())
}

/// Arguments for launching a VM with QEMU.
pub struct QemuArgs {
    pub tier: QemuTier,
    pub qemu_bin: String,
    pub igvm: Option<PathBuf>,
    pub uki: Option<PathBuf>,
    pub firmware: Option<PathBuf>,
    pub disk: PathBuf,
    pub disk_format: String,
    pub smp: u32,
    pub memory: String,
    pub port_forwards: Vec<(u16, u16)>,
    /// Optional writable ephemeral scratch disk. Attached with
    /// `serial=confai-scratch` so the guest initrd recognizes it as the
    /// encrypted-overlay backing disk via /sys/block/<dev>/serial.
    pub scratch: Option<PathBuf>,
}

impl QemuArgs {
    /// Build the QEMU command-line arguments.
    pub fn to_args(&self) -> anyhow::Result<Vec<String>> {
        // Validate all paths that will be interpolated into comma-delimited QEMU args
        reject_comma_in_path("disk", &self.disk)?;
        if let Some(ref p) = self.igvm {
            reject_comma_in_path("igvm", p)?;
        }
        if let Some(ref p) = self.uki {
            reject_comma_in_path("uki", p)?;
        }
        if let Some(ref p) = self.firmware {
            reject_comma_in_path("firmware", p)?;
        }

        let mut args = match self.tier {
            QemuTier::SevSnp => {
                let igvm = self
                    .igvm
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("SevSnp tier requires igvm path"))?;
                vec![
                    "-enable-kvm".to_string(),
                    "-cpu".to_string(),
                    "EPYC-Genoa".to_string(),
                    "-machine".to_string(),
                    "q35,confidential-guest-support=sev0,igvm-cfg=igvm0,memory-backend=ram1,kernel-irqchip=split".to_string(),
                    "-object".to_string(),
                    format!("igvm-cfg,id=igvm0,file={}", igvm.display()),
                    "-object".to_string(),
                    format!("memory-backend-memfd,id=ram1,size={},share=true", self.memory),
                    "-object".to_string(),
                    "sev-snp-guest,id=sev0,cbitpos=51,reduced-phys-bits=1".to_string(),
                    "-no-reboot".to_string(),
                    "-chardev".to_string(),
                    "stdio,id=hvc0,signal=off,mux=on".to_string(),
                    "-device".to_string(),
                    "virtio-serial-pci,id=virtser0".to_string(),
                    "-device".to_string(),
                    "virtconsole,chardev=hvc0,id=console0".to_string(),
                    "-mon".to_string(),
                    "chardev=hvc0,mode=readline".to_string(),
                ]
            }
            QemuTier::Kvm | QemuTier::Emulated => {
                let uki = self
                    .uki
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Kvm/Emulated tier requires uki path"))?;
                let firmware = self
                    .firmware
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Kvm/Emulated tier requires firmware path"))?;
                let mut v = vec!["-machine".to_string(), "q35".to_string()];
                if self.tier == QemuTier::Kvm {
                    v.push("-enable-kvm".to_string());
                }
                v.extend([
                    "-drive".to_string(),
                    format!(
                        "if=pflash,format=raw,readonly=on,file={}",
                        firmware.display()
                    ),
                    "-kernel".to_string(),
                    uki.display().to_string(),
                ]);
                v.extend([
                    "-chardev".to_string(),
                    "stdio,id=hvc0,signal=off,mux=on".to_string(),
                    "-device".to_string(),
                    "virtio-serial-pci,id=virtser0".to_string(),
                    "-device".to_string(),
                    "virtconsole,chardev=hvc0,id=console0".to_string(),
                    "-mon".to_string(),
                    "chardev=hvc0,mode=readline".to_string(),
                ]);
                v
            }
        };

        // Validate disk_format before interpolation into comma-delimited QEMU arg
        let allowed_formats = ["raw", "qcow2"];
        if !allowed_formats.contains(&self.disk_format.as_str()) {
            anyhow::bail!("unsupported disk format: {:?}", self.disk_format);
        }
        args.push("-drive".to_string());
        args.push(format!(
            "file={},format={},if=virtio,readonly=on",
            self.disk.display(),
            self.disk_format
        ));
        if let Some(ref scratch) = self.scratch {
            reject_comma_in_path("scratch", scratch)?;
            args.push("-drive".to_string());
            // `serial=` here is NOT a serial port — it's the virtio block
            // device's serial-number attribute, an arbitrary identifier string
            // QEMU exposes to the guest via virtio's device descriptor. Linux
            // surfaces it at /sys/block/<dev>/serial the moment the device is
            // enumerated, before any block I/O.
            //
            // We abuse this as a side-channel signal from cluster→guest: the
            // initrd reads the serial and gates the encrypted-overlay path on
            // serial == "confai-scratch", which lets us skip having to read
            // a filesystem LABEL off the disk (which would require pre-mkfs'ing
            // the disk cluster-side, ~5-7s of orchestration latency per launch).
            // KubeVirt exposes the same attribute via `Disk.serial` in the VM
            // spec; confai sets that on its datadisk emission.
            args.push(format!(
                "file={},format=raw,if=virtio,serial=confai-scratch",
                scratch.display()
            ));
        }
        args.push("-smp".to_string());
        args.push(self.smp.to_string());
        args.push("-m".to_string());
        args.push(self.memory.clone());
        args.push("-display".to_string());
        args.push("none".to_string());
        args.push("-serial".to_string());
        args.push("none".to_string());

        if !self.port_forwards.is_empty() {
            let hostfwds: String = self
                .port_forwards
                .iter()
                .map(|(h, g)| format!("hostfwd=tcp::{}-:{}", h, g))
                .collect::<Vec<_>>()
                .join(",");
            args.push("-netdev".to_string());
            args.push(format!("user,id=net0,{}", hostfwds));
            args.push("-device".to_string());
            args.push("virtio-net-pci,netdev=net0".to_string());
        }

        Ok(args)
    }
}

/// Launch a VM using QEMU.
pub fn launch(args: &QemuArgs) -> anyhow::Result<()> {
    let qemu_bin = if std::path::Path::new(&args.qemu_bin).exists() {
        args.qemu_bin.clone()
    } else {
        let path = tools::require(&args.qemu_bin)?;
        path.to_string_lossy().to_string()
    };

    let cmd_args = args.to_args()?;
    match args.tier {
        QemuTier::SevSnp => {
            let igvm = args
                .igvm
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("SevSnp tier requires igvm path"))?;
            tracing::info!(
                igvm = %igvm.display(),
                disk = %args.disk.display(),
                smp = args.smp,
                memory = %args.memory,
                "launching CVM via QEMU (SEV-SNP)"
            );
            // SNP requires sudo for /dev/sev
            let mut sudo_args = vec![qemu_bin.to_string()];
            sudo_args.extend(cmd_args);
            println!("sudo {}", shell_join(&sudo_args));
            let err = Command::new("sudo").args(&sudo_args).exec();
            // exec() only returns on failure
            anyhow::bail!("failed to exec sudo qemu: {err}");
        }
        QemuTier::Kvm | QemuTier::Emulated => {
            let uki = args
                .uki
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Kvm/Emulated tier requires uki path"))?;
            let firmware = args
                .firmware
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Kvm/Emulated tier requires firmware path"))?;
            tracing::info!(
                uki = %uki.display(),
                firmware = %firmware.display(),
                disk = %args.disk.display(),
                smp = args.smp,
                memory = %args.memory,
                tier = ?args.tier,
                "launching VM via QEMU"
            );
            let mut full = vec![qemu_bin.clone()];
            full.extend(cmd_args.iter().cloned());
            println!("{}", shell_join(&full));
            let err = Command::new(qemu_bin).args(&cmd_args).exec();
            anyhow::bail!("failed to exec qemu: {err}");
        }
    }
}
