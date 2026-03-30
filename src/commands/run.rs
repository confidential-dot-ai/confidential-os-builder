use std::process::Command;

use crate::tools;
use crate::RunArgs;

pub fn run(args: &RunArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "launching VM");

    // Step 1: Validate directory exists
    if !args.dir.exists() {
        anyhow::bail!("output directory not found: {}", args.dir.display());
    }

    // // Step 2: Read manifest
    // let manifest_path = args.dir.join("manifest.json");
    // if !manifest_path.exists() {
    //     anyhow::bail!("manifest.json not found in {}", args.dir.display());
    // }
    // let manifest = manifest::read_manifest(&manifest_path)?;

    // // Step 3: Detect QEMU tier
    // let tier = crate::qemu::detect_tier()?;

    // // Step 4: Print warnings for degraded tiers
    // match tier {
    //     QemuTier::SevSnp => {}
    //     QemuTier::Kvm => {
    //         eprintln!("WARNING: QEMU lacks IGVM/SEV-SNP support. Running with KVM acceleration only — no confidential computing guarantees.");
    //     }
    //     QemuTier::Emulated => {
    //         eprintln!("WARNING: Neither SEV-SNP nor KVM available. Running in pure emulation mode — this will be slow.");
    //     }
    // }

    // // Step 5: Validate artifacts based on tier
    // let igvm_path;
    // let uki_path;
    // let firmware_path;

    // match tier {
    //     QemuTier::SevSnp => {
    //         let path = args.dir.join("guest.igvm");
    //         if !path.exists() {
    //             anyhow::bail!("guest.igvm not found in {}", args.dir.display());
    //         }
    //         igvm_path = Some(path);
    //         uki_path = None;
    //         firmware_path = None;
    //     }
    //     QemuTier::Kvm | QemuTier::Emulated => {
    //         let uki = args.dir.join("uki.efi");
    //         if !uki.exists() {
    //             anyhow::bail!("uki.efi not found in {}", args.dir.display());
    //         }
    //         let fw = PathBuf::from(&manifest.inputs.firmware.path);
    //         if !fw.exists() {
    //             anyhow::bail!(
    //                 "firmware not found at {} (recorded in manifest). The OVMF firmware from build time must still be present.",
    //                 manifest.inputs.firmware.path
    //             );
    //         }
    //         igvm_path = None;
    //         uki_path = Some(uki);
    //         firmware_path = Some(fw);
    //     }
    // }

    // // Step 6: Find disk image using format from manifest
    // let disk_path = args.dir.join(format!("disk.{}", manifest.build.format));
    // if !disk_path.exists() {
    //     anyhow::bail!(
    //         "disk.{} not found in {}",
    //         manifest.build.format,
    //         args.dir.display()
    //     );
    // }

    // // Step 8: Parse port forwards
    // let port_forwards = args
    //     .port_forward
    //     .iter()
    //     .map(|s| {
    //         let (host_str, guest_str) = s.split_once(':').ok_or_else(|| {
    //             anyhow::anyhow!("invalid --port-forward format, expected HOST:GUEST: {s}")
    //         })?;
    //         let host = host_str
    //             .parse::<u16>()
    //             .map_err(|_| anyhow::anyhow!("invalid host port in --port-forward: {host_str}"))?;
    //         let guest = guest_str.parse::<u16>().map_err(|_| {
    //             anyhow::anyhow!("invalid guest port in --port-forward: {guest_str}")
    //         })?;
    //         Ok((host, guest))
    //     })
    //     .collect::<anyhow::Result<Vec<_>>>()?;

    // // Step 9: Launch QEMU
    // let qemu_args = QemuArgs {
    //     tier,
    //     igvm: igvm_path,
    //     uki: uki_path,
    //     firmware: firmware_path,
    //     disk: disk_path,
    //     disk_format: "qcow2".to_string(),
    //     smp: manifest.build.smp,
    //     memory: manifest.build.memory,
    //     port_forwards,
    // };
    // crate::qemu::launch(&qemu_args)?;

    Command::new("mkdir").args(["-p", "/tmp/swtpm"]).output()?;
    Command::new("swtpm")
        .args([
            "socket",
            "--tpmstate",
            "dir=/tmp/swtpm",
            "--ctrl",
            "type=unixio,path=/tmp/swtpm/sock",
            "--tpm2",
        ])
        .stdin(std::process::Stdio::null())
        .spawn()?;

    let ci_image = args.dir.join("seed.iso");
    let ci_drive = format!("file={},index=0,media=cdrom", ci_image.to_string_lossy());
    let image_path = args.dir.join("image.qcow2");
    let image_blockdev = format!("driver=qcow2,node-name=mkosi,discard=unmap,file.driver=file,file.filename={},file.aio=io_uring,cache.direct=yes,cache.no-flush=no", image_path.to_string_lossy());
    let args = vec![
        "-machine", "type=q35,smm=off,hpet=off",
        "-smp", "2",
        "-m", "2048M",
        "-drive", &ci_drive,
        "-object", "rng-random,filename=/dev/urandom,id=rng0",
        "-device", "virtio-rng-pci,rng=rng0,id=rng-device0",
        "-device", "virtio-balloon,free-page-reporting=on",
        "-no-user-config",
        "-nic", "user,model=virtio-net-pci,hostfwd=tcp::8888-:80",
        "-cpu", "max",
        "-accel", "tcg",
        "-nographic",
        "-nodefaults",
        "-chardev", "stdio,mux=on,id=console,signal=off",
        "-device", "virtio-serial-pci,id=mkosi-virtio-serial-pci",
        "-device", "virtconsole,chardev=console",
        "-mon", "console",
        "-drive", "if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd",
        "-device", "virtio-scsi-pci,id=mkosi",
        "-blockdev", &image_blockdev,
        "-device", "virtio-blk-pci,drive=mkosi,bootindex=1",
        "-smbios", "type=11,value=io.systemd.stub.kernel-cmdline-extra=systemd.wants=network.target SYSTEMD_SULOGIN_FORCE=1 rw module_blacklist=vmw_vmci systemd.tty.term.hvc0=xterm-256color systemd.tty.columns.hvc0=230 systemd.tty.rows.hvc0=36 ip=enc0:any ip=enp0s1:any ip=enp0s2:any ip=host0:any ip=none loglevel=4 systemd.tty.term.console=xterm-256color systemd.tty.columns.console=230 systemd.tty.rows.console=36 console=hvc0 TERM=xterm-256color",
        "-smbios", "type=11,value=io.systemd.boot.kernel-cmdline-extra=systemd.wants=network.target SYSTEMD_SULOGIN_FORCE=1 rw module_blacklist=vmw_vmci systemd.tty.term.hvc0=xterm-256color systemd.tty.columns.hvc0=230 systemd.tty.rows.hvc0=36 ip=enc0:any ip=enp0s1:any ip=enp0s2:any ip=host0:any ip=none loglevel=4 systemd.tty.term.console=xterm-256color systemd.tty.columns.console=230 systemd.tty.rows.console=36 console=hvc0 TERM=xterm-256color",
        "-chardev", "socket,id=chrtpm,path=/tmp/swtpm/sock",
        "-tpmdev", "emulator,id=tpm0,chardev=chrtpm",
        "-device", "tpm-tis,tpmdev=tpm0"
    ];
    tools::run_command_exec("qemu-system-x86_64", &args)?;

    Ok(())
}
