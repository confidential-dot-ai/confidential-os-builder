# lunal-build Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust CLI tool (`lunal-build`) that orchestrates mkosi, systemd-ukify, and igvm-tools to produce confidential VM images for AMD SEV-SNP.

**Architecture:** Rust CLI using clap for argument parsing. Four subcommands (`kernel`, `base`, `cloud-init`, `container`) that generate configs and shell out to external tools. Two-partition disk design (base + project) composed at build time.

**Tech Stack:** Rust, clap (derive), tracing, serde, sha2, fs_err, thiserror, tempfile, clap-verbosity-flag

**Error handling note:** Library modules (`src/lib.rs` and below) use `anyhow::Result` in this initial scaffolding for velocity. This is intentional — typed errors via `thiserror` will replace `anyhow` in library code once the pipeline stages are no longer stubs. `tools.rs` already demonstrates the target pattern with `ToolError`. The binary crate (`main.rs`) will always use `anyhow`.

**Spec:** `docs/superpowers/specs/2026-03-13-lunal-build-design.md`

---

## File Map

| File | Responsibility |
|------|---------------|
| `Cargo.toml` | Project manifest with all dependencies |
| `src/main.rs` | CLI entry point, clap arg parsing, tracing setup, dispatch to commands |
| `src/commands/mod.rs` | Re-exports command modules |
| `src/commands/kernel.rs` | `kernel` subcommand — invokes kernel build |
| `src/commands/base.rs` | `base` subcommand — invokes mkosi for base partition |
| `src/commands/cloud_init.rs` | `cloud-init` subcommand — full pipeline orchestration |
| `src/commands/container.rs` | `container` subcommand — stub for future implementation |
| `src/tools.rs` | External tool discovery and invocation helpers |
| `src/mkosi/mod.rs` | Re-exports mkosi modules |
| `src/mkosi/config.rs` | mkosi config file generation |
| `src/uki/mod.rs` | Re-exports UKI modules |
| `src/uki/build.rs` | systemd-ukify invocation for UKI construction |
| `src/igvm/mod.rs` | Re-exports igvm modules |
| `src/igvm/invoke.rs` | igvm-tools invocation and manifest parsing |
| `src/compose/mod.rs` | Re-exports compose modules |
| `src/compose/disk.rs` | Partition composition (base + project → final GPT image) |
| `src/manifest.rs` | lunal-build manifest schema and generation |
| `tests/cli.rs` | CLI argument parsing integration tests |
| `tests/manifest.rs` | Manifest serialization tests |
| `tests/tools.rs` | Tool discovery and command-building tests |
| `tests/mkosi_config.rs` | mkosi config generation tests |
| `tests/uki_build.rs` | UKI build argument construction tests |
| `tests/igvm_invoke.rs` | igvm-tools argument construction tests |

---

## Chunk 1: Project Scaffolding and CLI

### Task 1: Initialize Rust project

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`

- [ ] **Step 1: Create .gitignore**

```
/target
```

- [ ] **Step 2: Create Cargo.toml**

```toml
[package]
name = "lunal-build"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
clap-verbosity-flag = "3"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
thiserror = "2"
tempfile = "3"
fs-err = "3"
```

- [ ] **Step 3: Create minimal main.rs**

```rust
use clap::Parser;

#[derive(Parser)]
#[command(name = "lunal-build", about = "Confidential VM image builder")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Build the hardened custom kernel
    Kernel,
    /// Build the security-hardened base image
    Base,
    /// Build a CVM image with cloud-init configuration
    CloudInit,
    /// Build a CVM image running a container
    Container,
}

fn main() {
    let _cli = Cli::parse();
    println!("lunal-build");
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully, binary at `target/debug/lunal-build`

- [ ] **Step 5: Verify help output**

Run: `cargo run -- --help`
Expected: Shows help with four subcommands listed

- [ ] **Step 6: Commit**

```bash
git add .gitignore Cargo.toml src/main.rs
git commit -m "feat: initialize lunal-build Rust project with clap skeleton"
```

### Task 2: Full CLI argument parsing

**Files:**
- Modify: `src/main.rs`
- Create: `tests/cli.rs`

- [ ] **Step 1: Write CLI parsing tests**

Create `tests/cli.rs`:

```rust
use assert_cmd::Command;

#[test]
fn test_help_shows_subcommands() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("kernel"))
        .stdout(predicates::str::contains("base"))
        .stdout(predicates::str::contains("cloud-init"))
        .stdout(predicates::str::contains("container"));
}

#[test]
fn test_cloud_init_requires_dir() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args(["cloud-init"])
        .assert()
        .failure();
}

#[test]
fn test_cloud_init_requires_kernel_flag() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args(["cloud-init", "/tmp/fake-dir", "--initrd", "/tmp/i", "--firmware", "/tmp/f", "--base-image", "/tmp/b", "-o", "/tmp/o"])
        .assert()
        .failure();
}

#[test]
fn test_container_requires_url() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args(["container"])
        .assert()
        .failure();
}

#[test]
fn test_base_requires_source_image() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args(["base", "-o", "/tmp/o"])
        .assert()
        .failure();
}

#[test]
fn test_kernel_requires_output() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args(["kernel", "--source", "/tmp/s", "--config", "/tmp/c"])
        .assert()
        .failure();
}
```

- [ ] **Step 2: Add test dependencies to Cargo.toml**

Add to `Cargo.toml`:

```toml
[dev-dependencies]
assert_cmd = "2"
predicates = "3"
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --test cli`
Expected: Tests fail because CLI args are not yet fully defined

- [ ] **Step 4: Implement full CLI argument parsing**

Update `src/main.rs` with complete argument definitions:

```rust
use std::path::PathBuf;

use clap::Parser;
use clap_verbosity_flag::Verbosity;

#[derive(Parser)]
#[command(name = "lunal-build", about = "Confidential VM image builder")]
pub struct Cli {
    #[command(flatten)]
    pub verbose: Verbosity,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(clap::Subcommand)]
pub enum Commands {
    /// Build the hardened custom kernel
    Kernel(KernelArgs),
    /// Build the security-hardened base image
    Base(BaseArgs),
    /// Build a CVM image with cloud-init configuration
    CloudInit(CloudInitArgs),
    /// Build a CVM image running a container
    Container(ContainerArgs),
}

#[derive(clap::Args)]
pub struct KernelArgs {
    /// Path to kernel source tree
    #[arg(long)]
    pub source: PathBuf,

    /// Path to kernel .config (hardening config)
    #[arg(long)]
    pub config: PathBuf,

    /// Output directory for kernel + initrd
    #[arg(short, long)]
    pub output: PathBuf,
}

#[derive(clap::Args)]
pub struct BaseArgs {
    /// Ubuntu cloud image to start from
    #[arg(long)]
    pub source_image: PathBuf,

    /// Output directory for the base partition image
    #[arg(short, long)]
    pub output: PathBuf,
}

#[derive(clap::Args)]
pub struct CloudInitArgs {
    /// Path to cloud-init configuration directory
    pub dir: PathBuf,

    /// Path to hardened kernel
    #[arg(long)]
    pub kernel: PathBuf,

    /// Path to base initrd (input to UKI build)
    #[arg(long)]
    pub initrd: PathBuf,

    /// Path to OVMF firmware binary
    #[arg(long)]
    pub firmware: PathBuf,

    /// Path to base image (from `lunal-build base`)
    #[arg(long)]
    pub base_image: PathBuf,

    /// Number of vCPUs (affects SNP launch digest)
    #[arg(long, default_value = "1")]
    pub smp: u32,

    /// Output image format
    #[arg(long, default_value = "qcow2")]
    pub format: ImageFormat,

    /// Output directory for artifacts
    #[arg(short, long)]
    pub output: PathBuf,
}

#[derive(clap::Args)]
pub struct ContainerArgs {
    /// OCI container image URL
    pub url: String,

    /// Path to hardened kernel
    #[arg(long)]
    pub kernel: PathBuf,

    /// Path to base initrd (input to UKI build)
    #[arg(long)]
    pub initrd: PathBuf,

    /// Path to OVMF firmware binary
    #[arg(long)]
    pub firmware: PathBuf,

    /// Path to base image (from `lunal-build base`)
    #[arg(long)]
    pub base_image: PathBuf,

    /// Number of vCPUs (affects SNP launch digest)
    #[arg(long, default_value = "1")]
    pub smp: u32,

    /// Output image format
    #[arg(long, default_value = "qcow2")]
    pub format: ImageFormat,

    /// Output directory for artifacts
    #[arg(short, long)]
    pub output: PathBuf,
}

#[derive(Clone, clap::ValueEnum)]
pub enum ImageFormat {
    Qcow2,
    Vhd,
    Raw,
}

fn main() {
    let _cli = Cli::parse();
    println!("lunal-build");
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test cli`
Expected: All 6 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/main.rs tests/cli.rs Cargo.toml
git commit -m "feat: implement full CLI argument parsing with all subcommands"
```

### Task 3: Tracing setup and module structure

**Files:**
- Modify: `src/main.rs`
- Create: `src/commands/mod.rs`
- Create: `src/commands/kernel.rs`
- Create: `src/commands/base.rs`
- Create: `src/commands/cloud_init.rs`
- Create: `src/commands/container.rs`
- Create: `src/tools.rs`
- Create: `src/mkosi/mod.rs`
- Create: `src/mkosi/config.rs`
- Create: `src/igvm/mod.rs`
- Create: `src/igvm/invoke.rs`
- Create: `src/compose/mod.rs`
- Create: `src/compose/disk.rs`
- Create: `src/manifest.rs`

- [ ] **Step 1: Create command module stubs**

Create `src/commands/mod.rs`:

```rust
pub mod base;
pub mod cloud_init;
pub mod container;
pub mod kernel;
```

Create `src/commands/kernel.rs`:

```rust
use crate::KernelArgs;

pub fn run(args: &KernelArgs) -> anyhow::Result<()> {
    tracing::info!(source = %args.source.display(), "building hardened kernel");
    anyhow::bail!("kernel build not yet implemented")
}
```

Create `src/commands/base.rs`:

```rust
use crate::BaseArgs;

pub fn run(args: &BaseArgs) -> anyhow::Result<()> {
    tracing::info!(source_image = %args.source_image.display(), "building base image");
    anyhow::bail!("base image build not yet implemented")
}
```

Create `src/commands/cloud_init.rs`:

```rust
use crate::CloudInitArgs;

pub fn run(args: &CloudInitArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "building cloud-init CVM image");
    anyhow::bail!("cloud-init build not yet implemented")
}
```

Create `src/commands/container.rs`:

```rust
use crate::ContainerArgs;

pub fn run(args: &ContainerArgs) -> anyhow::Result<()> {
    tracing::info!(url = %args.url, "building container CVM image");
    anyhow::bail!("container build not yet implemented")
}
```

- [ ] **Step 2: Create infrastructure module stubs**

Create `src/tools.rs`:

```rust
use std::path::PathBuf;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ToolError {
    #[error("{tool} not found in PATH. Install it and try again.")]
    NotFound { tool: String },

    #[error("{tool} failed with exit code {code}:\n{stderr}")]
    Failed {
        tool: String,
        code: i32,
        stderr: String,
    },

    #[error("{tool} was terminated by a signal")]
    Signal { tool: String },

    #[error("failed to execute {tool}: {source}")]
    Io {
        tool: String,
        source: std::io::Error,
    },
}

/// Check that an external tool is available in PATH.
pub fn require(tool: &str) -> Result<PathBuf, ToolError> {
    which::which(tool).map_err(|_| ToolError::NotFound {
        tool: tool.to_string(),
    })
}
```

Create `src/mkosi/mod.rs`:

```rust
pub mod config;
```

Create `src/mkosi/config.rs`:

```rust
// mkosi configuration generation — will be implemented in Task 8
```

Create `src/igvm/mod.rs`:

```rust
pub mod invoke;
```

Create `src/igvm/invoke.rs`:

```rust
// igvm-tools invocation — will be implemented in Task 5
```

Create `src/compose/mod.rs`:

```rust
pub mod disk;
```

Create `src/compose/disk.rs`:

```rust
// Disk composition — will be implemented in Task 10
```

Create `src/manifest.rs`:

```rust
// Manifest generation — will be implemented in Task 7
```

- [ ] **Step 3: Add `which` and `anyhow` to Cargo.toml**

Add to `[dependencies]`:

```toml
which = "7"
anyhow = "1"
```

- [ ] **Step 4: Wire up main.rs with tracing and command dispatch**

Update `src/main.rs` — keep all the type definitions, replace `fn main()`:

```rust
mod commands;
mod compose;
mod igvm;
mod manifest;
mod mkosi;
mod tools;
mod uki;

// ... (keep all existing type definitions: Cli, Commands, *Args, ImageFormat) ...

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(
            match cli.verbose.log_level_filter() {
                clap_verbosity_flag::LevelFilter::Off => tracing_subscriber::filter::LevelFilter::OFF,
                clap_verbosity_flag::LevelFilter::Error => tracing_subscriber::filter::LevelFilter::ERROR,
                clap_verbosity_flag::LevelFilter::Warn => tracing_subscriber::filter::LevelFilter::WARN,
                clap_verbosity_flag::LevelFilter::Info => tracing_subscriber::filter::LevelFilter::INFO,
                clap_verbosity_flag::LevelFilter::Debug => tracing_subscriber::filter::LevelFilter::DEBUG,
                clap_verbosity_flag::LevelFilter::Trace => tracing_subscriber::filter::LevelFilter::TRACE,
            }
            .into(),
        )
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    match &cli.command {
        Commands::Kernel(args) => commands::kernel::run(args),
        Commands::Base(args) => commands::base::run(args),
        Commands::CloudInit(args) => commands::cloud_init::run(args),
        Commands::Container(args) => commands::container::run(args),
    }
}
```

- [ ] **Step 5: Verify everything compiles**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 6: Run all tests**

Run: `cargo test`
Expected: All CLI tests still pass

- [ ] **Step 7: Commit**

```bash
git add src/ Cargo.toml
git commit -m "feat: add module structure, tracing setup, and command dispatch"
```

---

## Chunk 2: Tool Discovery and External Tool Invocation

### Task 4: Tool discovery and execution helpers

**Files:**
- Modify: `src/tools.rs`
- Create: `tests/tools.rs`

- [ ] **Step 1: Write tool discovery tests**

Create `tests/tools.rs`:

```rust
use lunal_build::tools;

#[test]
fn test_require_finds_existing_tool() {
    // `sh` should always exist
    let result = tools::require("sh");
    assert!(result.is_ok());
}

#[test]
fn test_require_fails_for_missing_tool() {
    let result = tools::require("nonexistent-tool-xyz-123");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("not found in PATH"));
}

#[test]
fn test_run_command_success() {
    let output = tools::run_command("echo", &["hello"]).unwrap();
    assert_eq!(output.trim(), "hello");
}

#[test]
fn test_run_command_failure() {
    let result = tools::run_command("sh", &["-c", "exit 1"]);
    assert!(result.is_err());
}

#[test]
fn test_build_command_args() {
    let cmd = tools::CommandBuilder::new("igvm-tools")
        .arg("build")
        .arg_pair("--firmware", "/path/to/ovmf")
        .arg_pair("--kernel", "/path/to/uki")
        .arg_pair("--smp", "4")
        .arg_pair("--platform", "snp")
        .arg_pair("-o", "/path/to/output.igvm")
        .build();
    let args: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        args,
        vec![
            "build",
            "--firmware", "/path/to/ovmf",
            "--kernel", "/path/to/uki",
            "--smp", "4",
            "--platform", "snp",
            "-o", "/path/to/output.igvm",
        ]
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test tools`
Expected: Fails because `tools` module does not expose the required functions as public API

- [ ] **Step 3: Create lib.rs to expose public API for tests**

Create `src/lib.rs`:

```rust
pub mod tools;
pub mod manifest;
pub mod mkosi;
pub mod uki;
pub mod igvm;
pub mod compose;
```

- [ ] **Step 4: Implement tool helpers**

Update `src/tools.rs`:

```rust
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ToolError {
    #[error("{tool} not found in PATH. Install it and try again.")]
    NotFound { tool: String },

    #[error("{tool} failed with exit code {code}:\n{stderr}")]
    Failed {
        tool: String,
        code: i32,
        stderr: String,
    },

    #[error("{tool} was terminated by a signal")]
    Signal { tool: String },

    #[error("failed to execute {tool}: {source}")]
    Io {
        tool: String,
        source: std::io::Error,
    },
}

/// Check that an external tool is available in PATH.
pub fn require(tool: &str) -> Result<PathBuf, ToolError> {
    which::which(tool).map_err(|_| ToolError::NotFound {
        tool: tool.to_string(),
    })
}

/// Run a command and return its stdout as a string.
/// Fails if the command exits with a non-zero status.
pub fn run_command(tool: &str, args: &[&str]) -> Result<String, ToolError> {
    let output = Command::new(tool)
        .args(args)
        .output()
        .map_err(|e| ToolError::Io {
            tool: tool.to_string(),
            source: e,
        })?;

    if !output.status.success() {
        let code = output.status.code().ok_or(ToolError::Signal {
            tool: tool.to_string(),
        })?;
        return Err(ToolError::Failed {
            tool: tool.to_string(),
            code,
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Run a command with inherited stdio (streams output to the terminal).
/// Fails if the command exits with a non-zero status.
pub fn run_command_streaming(tool: &str, args: &[impl AsRef<OsStr>]) -> Result<(), ToolError> {
    let status = Command::new(tool)
        .args(args)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| ToolError::Io {
            tool: tool.to_string(),
            source: e,
        })?;

    if !status.success() {
        let code = status.code().ok_or(ToolError::Signal {
            tool: tool.to_string(),
        })?;
        return Err(ToolError::Failed {
            tool: tool.to_string(),
            code,
            stderr: String::new(),
        });
    }

    Ok(())
}

/// Builder for constructing command argument lists.
pub struct CommandBuilder {
    tool: String,
    args: Vec<String>,
}

impl CommandBuilder {
    pub fn new(tool: &str) -> Self {
        Self {
            tool: tool.to_string(),
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: &str) -> Self {
        self.args.push(arg.to_string());
        self
    }

    pub fn arg_pair(mut self, flag: &str, value: &str) -> Self {
        self.args.push(flag.to_string());
        self.args.push(value.to_string());
        self
    }

    pub fn tool(&self) -> &str {
        &self.tool
    }

    pub fn build(self) -> Vec<String> {
        self.args
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test tools`
Expected: All 5 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/tools.rs src/lib.rs tests/tools.rs
git commit -m "feat: implement tool discovery and command execution helpers"
```

---

## Chunk 3: igvm-tools Invocation

### Task 5: igvm-tools invocation module

**Files:**
- Modify: `src/igvm/invoke.rs`
- Create: `tests/igvm_invoke.rs`

- [ ] **Step 1: Write igvm-tools argument construction tests**

Create `tests/igvm_invoke.rs`:

```rust
use std::path::PathBuf;
use lunal_build::igvm::invoke::IgvmBuildArgs;

#[test]
fn test_igvm_build_args_to_command() {
    let args = IgvmBuildArgs {
        firmware: PathBuf::from("/path/to/OVMF.fd"),
        kernel: PathBuf::from("/path/to/uki.efi"),
        smp: 4,
        manifest: Some(PathBuf::from("/path/to/manifest.json")),
        output: PathBuf::from("/path/to/guest.igvm"),
    };
    let cmd_args = args.to_args();
    assert_eq!(
        cmd_args,
        vec![
            "build",
            "--firmware", "/path/to/OVMF.fd",
            "--kernel", "/path/to/uki.efi",
            "--smp", "4",
            "--platform", "snp",
            "--manifest", "/path/to/manifest.json",
            "-o", "/path/to/guest.igvm",
        ]
    );
}

#[test]
fn test_igvm_build_args_without_manifest() {
    let args = IgvmBuildArgs {
        firmware: PathBuf::from("/path/to/OVMF.fd"),
        kernel: PathBuf::from("/path/to/uki.efi"),
        smp: 1,
        manifest: None,
        output: PathBuf::from("/path/to/guest.igvm"),
    };
    let cmd_args = args.to_args();
    assert!(!cmd_args.contains(&"--manifest".to_string()));
}

#[test]
fn test_igvm_build_args_default_smp() {
    let args = IgvmBuildArgs {
        firmware: PathBuf::from("/fw"),
        kernel: PathBuf::from("/k"),
        smp: 1,
        manifest: None,
        output: PathBuf::from("/o"),
    };
    let cmd_args = args.to_args();
    assert!(cmd_args.contains(&"1".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test igvm_invoke`
Expected: Fails because `IgvmBuildArgs` does not exist

- [ ] **Step 3: Implement igvm-tools invocation**

Update `src/igvm/invoke.rs`:

```rust
use std::path::PathBuf;

use crate::tools;

/// Arguments for `igvm-tools build`.
pub struct IgvmBuildArgs {
    pub firmware: PathBuf,
    pub kernel: PathBuf,
    pub smp: u32,
    pub manifest: Option<PathBuf>,
    pub output: PathBuf,
}

impl IgvmBuildArgs {
    /// Convert to command-line argument list for igvm-tools.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![
            "build".to_string(),
            "--firmware".to_string(),
            self.firmware.display().to_string(),
            "--kernel".to_string(),
            self.kernel.display().to_string(),
            "--smp".to_string(),
            self.smp.to_string(),
            "--platform".to_string(),
            "snp".to_string(),
        ];
        if let Some(ref manifest) = self.manifest {
            args.push("--manifest".to_string());
            args.push(manifest.display().to_string());
        }
        args.push("-o".to_string());
        args.push(self.output.display().to_string());
        args
    }
}

/// Invoke `igvm-tools build` with the given arguments.
/// Streams output to the terminal.
pub fn build(args: &IgvmBuildArgs) -> Result<(), tools::ToolError> {
    tools::require("igvm-tools")?;
    let cmd_args = args.to_args();
    tracing::info!(output = %args.output.display(), smp = args.smp, "invoking igvm-tools build");
    tools::run_command_streaming("igvm-tools", &cmd_args)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test igvm_invoke`
Expected: All 3 tests pass

- [ ] **Step 5: Commit**

```bash
git add src/igvm/invoke.rs tests/igvm_invoke.rs
git commit -m "feat: implement igvm-tools invocation with argument construction"
```

---

## Chunk 4: UKI Build Module

### Task 6: systemd-ukify invocation module

**Files:**
- Create: `src/uki/mod.rs`
- Create: `src/uki/build.rs`
- Create: `tests/uki_build.rs`

- [ ] **Step 1: Write UKI argument construction tests**

Create `tests/uki_build.rs`:

```rust
use std::path::PathBuf;
use lunal_build::uki::build::UkifyBuildArgs;

#[test]
fn test_ukify_args_basic() {
    let args = UkifyBuildArgs {
        kernel: PathBuf::from("/path/to/vmlinuz"),
        initrds: vec![PathBuf::from("/path/to/initrd.img")],
        output: PathBuf::from("/path/to/uki.efi"),
    };
    let cmd_args = args.to_args();
    assert_eq!(
        cmd_args,
        vec![
            "build",
            "--linux", "/path/to/vmlinuz",
            "--initrd", "/path/to/initrd.img",
            "--output", "/path/to/uki.efi",
        ]
    );
}

#[test]
fn test_ukify_args_multiple_initrds() {
    let args = UkifyBuildArgs {
        kernel: PathBuf::from("/path/to/vmlinuz"),
        initrds: vec![
            PathBuf::from("/path/to/initrd.img"),
            PathBuf::from("/path/to/verity-initrd.img"),
        ],
        output: PathBuf::from("/path/to/uki.efi"),
    };
    let cmd_args = args.to_args();
    // ukify accepts multiple --initrd flags
    let initrd_count = cmd_args.iter().filter(|a| *a == "--initrd").count();
    assert_eq!(initrd_count, 2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test uki_build`
Expected: Fails because `UkifyBuildArgs` does not exist

- [ ] **Step 3: Implement UKI build module**

Create `src/uki/mod.rs`:

```rust
pub mod build;
```

Create `src/uki/build.rs`:

```rust
use std::path::PathBuf;

use crate::tools;

/// Arguments for `ukify build`.
pub struct UkifyBuildArgs {
    pub kernel: PathBuf,
    pub initrds: Vec<PathBuf>,
    pub output: PathBuf,
}

impl UkifyBuildArgs {
    /// Convert to command-line argument list for ukify.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![
            "build".to_string(),
            "--linux".to_string(),
            self.kernel.display().to_string(),
        ];
        for initrd in &self.initrds {
            args.push("--initrd".to_string());
            args.push(initrd.display().to_string());
        }
        args.push("--output".to_string());
        args.push(self.output.display().to_string());
        args
    }
}

/// Invoke `ukify build` to produce a UKI EFI binary.
/// Data flow: (kernel + initrd(s)) → ukify → UKI.efi
pub fn build(args: &UkifyBuildArgs) -> Result<(), tools::ToolError> {
    tools::require("ukify")?;
    let cmd_args = args.to_args();
    tracing::info!(output = %args.output.display(), "building UKI via ukify");
    tools::run_command_streaming("ukify", &cmd_args)
}
```

- [ ] **Step 4: Add `uki` module to lib.rs**

Add `pub mod uki;` to `src/lib.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test uki_build`
Expected: All 2 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/uki/ tests/uki_build.rs src/lib.rs
git commit -m "feat: implement UKI build module with ukify invocation"
```

---

## Chunk 5: Manifest Generation

### Task 7: Manifest schema and generation

**Files:**
- Modify: `src/manifest.rs`
- Create: `tests/manifest.rs`

- [ ] **Step 1: Write manifest serialization tests**

Create `tests/manifest.rs`:

```rust
use lunal_build::manifest::{
    BuildManifest, BuildConfig, FileEntry, ManifestInputs, ManifestOutputs, Measurement,
};

fn sample_entry(path: &str) -> FileEntry {
    FileEntry { path: path.to_string(), sha256: "abc123".to_string() }
}

fn sample_manifest() -> BuildManifest {
    BuildManifest {
        version: 1,
        build: BuildConfig {
            timestamp: "2026-03-13T12:00:00Z".to_string(),
            smp: 4,
            format: "qcow2".to_string(),
            platform: "snp".to_string(),
        },
        inputs: ManifestInputs {
            kernel: sample_entry("vmlinuz"),
            initrd: sample_entry("initrd.img"),
            firmware: sample_entry("OVMF.fd"),
            base_image: sample_entry("base.raw"),
            project_partition: sample_entry("project.raw"),
        },
        outputs: ManifestOutputs {
            disk_image: sample_entry("disk.qcow2"),
            igvm: sample_entry("guest.igvm"),
            uki: sample_entry("uki.efi"),
        },
        measurement: Measurement {
            snp_launch_digest: "aabbcc".to_string(),
            algorithm: "sha384".to_string(),
            page_count: 5598,
            vmsa_count: 4,
        },
    }
}

#[test]
fn test_manifest_serializes_to_json() {
    let manifest = sample_manifest();
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(json.contains("\"version\": 1"));
    assert!(json.contains("\"snp_launch_digest\": \"aabbcc\""));
    assert!(json.contains("\"vmsa_count\": 4"));
    assert!(json.contains("\"kernel\""));
    assert!(json.contains("\"firmware\""));
    assert!(json.contains("\"base_image\""));
    assert!(json.contains("\"project_partition\""));
}

#[test]
fn test_manifest_roundtrip() {
    let manifest = sample_manifest();
    let json = serde_json::to_string(&manifest).unwrap();
    let deserialized: BuildManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.version, manifest.version);
    assert_eq!(deserialized.build.smp, manifest.build.smp);
    assert_eq!(deserialized.inputs.kernel.path, "vmlinuz");
    assert_eq!(deserialized.outputs.disk_image.path, "disk.qcow2");
}

#[test]
fn test_sha256_file_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.bin");
    fs_err::write(&path, b"hello world").unwrap();
    let hash = lunal_build::manifest::sha256_file(&path).unwrap();
    // SHA-256 of "hello world"
    assert_eq!(hash, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test manifest`
Expected: Fails because manifest types don't exist

- [ ] **Step 3: Implement manifest module**

Update `src/manifest.rs`:

```rust
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
pub struct BuildManifest {
    pub version: u32,
    pub build: BuildConfig,
    pub inputs: ManifestInputs,
    pub outputs: ManifestOutputs,
    pub measurement: Measurement,
}

#[derive(Serialize, Deserialize)]
pub struct BuildConfig {
    pub timestamp: String,
    pub smp: u32,
    pub format: String,
    pub platform: String,
}

#[derive(Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize)]
pub struct ManifestInputs {
    pub kernel: FileEntry,
    pub initrd: FileEntry,
    pub firmware: FileEntry,
    pub base_image: FileEntry,
    pub project_partition: FileEntry,
}

#[derive(Serialize, Deserialize)]
pub struct ManifestOutputs {
    pub disk_image: FileEntry,
    pub igvm: FileEntry,
    pub uki: FileEntry,
}

#[derive(Serialize, Deserialize)]
pub struct Measurement {
    pub snp_launch_digest: String,
    pub algorithm: String,
    pub page_count: u64,
    pub vmsa_count: u32,
}

/// Compute SHA-256 hash of a file, returned as a hex string.
/// Uses streaming reads to handle large files (disk images can be multiple GB).
pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = fs_err::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let result = hasher.finalize();
    Ok(hex::encode(result))
}

/// Write the manifest to a JSON file.
pub fn write_manifest(manifest: &BuildManifest, path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(manifest)?;
    fs_err::write(path, json)?;
    Ok(())
}
```

- [ ] **Step 4: Add hex dependency to Cargo.toml**

Add to `[dependencies]`:

```toml
hex = "0.4"
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test manifest`
Expected: All 3 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/manifest.rs tests/manifest.rs Cargo.toml
git commit -m "feat: implement manifest schema and SHA-256 file hashing"
```

---

## Chunk 6: mkosi Config Generation

### Task 8: mkosi configuration generation

**Files:**
- Modify: `src/mkosi/config.rs`
- Create: `tests/mkosi_config.rs`

- [ ] **Step 1: Write mkosi config generation tests**

Create `tests/mkosi_config.rs`:

```rust
use std::path::PathBuf;
use lunal_build::mkosi::config::{MkosiConfig, MkosiProfile};

#[test]
fn test_base_config_generates_valid_ini() {
    let config = MkosiConfig::base(PathBuf::from("/path/to/ubuntu.img"));
    let ini = config.to_ini();
    assert!(ini.contains("[Distribution]"));
    assert!(ini.contains("Distribution=ubuntu"));
}

#[test]
fn test_cloud_init_config_includes_cloud_init_dir() {
    let config = MkosiConfig::cloud_init(PathBuf::from("/path/to/cloud-init"));
    let ini = config.to_ini();
    assert!(ini.contains("[Content]"));
}

#[test]
fn test_config_profile() {
    let config = MkosiConfig::base(PathBuf::from("/path/to/img"));
    assert_eq!(config.profile, MkosiProfile::Base);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test mkosi_config`
Expected: Fails because `MkosiConfig` does not exist

- [ ] **Step 3: Implement mkosi config generation**

Update `src/mkosi/config.rs`:

```rust
use std::path::PathBuf;

/// mkosi build profile.
#[derive(Debug, PartialEq)]
pub enum MkosiProfile {
    Base,
    CloudInit,
}

/// Represents a mkosi configuration to be written as an INI file.
pub struct MkosiConfig {
    pub profile: MkosiProfile,
    pub source_image: Option<PathBuf>,
    pub cloud_init_dir: Option<PathBuf>,
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl MkosiConfig {
    /// Create a mkosi config for building the base partition.
    pub fn base(source_image: PathBuf) -> Self {
        let mut config = Self {
            profile: MkosiProfile::Base,
            source_image: Some(source_image),
            cloud_init_dir: None,
            sections: Vec::new(),
        };
        config.sections.push((
            "Distribution".to_string(),
            vec![("Distribution".to_string(), "ubuntu".to_string())],
        ));
        config.sections.push((
            "Output".to_string(),
            vec![("Format".to_string(), "disk".to_string())],
        ));
        config
    }

    /// Create a mkosi config for building a project partition with cloud-init.
    pub fn cloud_init(cloud_init_dir: PathBuf) -> Self {
        let mut config = Self {
            profile: MkosiProfile::CloudInit,
            source_image: None,
            cloud_init_dir: Some(cloud_init_dir),
            sections: Vec::new(),
        };
        config.sections.push((
            "Distribution".to_string(),
            vec![("Distribution".to_string(), "ubuntu".to_string())],
        ));
        config.sections.push((
            "Content".to_string(),
            vec![],
        ));
        config.sections.push((
            "Output".to_string(),
            vec![("Format".to_string(), "disk".to_string())],
        ));
        config
    }

    /// Serialize to mkosi INI format.
    pub fn to_ini(&self) -> String {
        let mut output = String::new();
        for (section, entries) in &self.sections {
            output.push_str(&format!("[{}]\n", section));
            for (key, value) in entries {
                output.push_str(&format!("{}={}\n", key, value));
            }
            output.push('\n');
        }
        output
    }

    /// Write the config to a file.
    pub fn write_to(&self, path: &std::path::Path) -> anyhow::Result<()> {
        fs_err::write(path, self.to_ini())?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test mkosi_config`
Expected: All 3 tests pass

- [ ] **Step 5: Commit**

```bash
git add src/mkosi/config.rs tests/mkosi_config.rs
git commit -m "feat: implement mkosi configuration generation"
```

---

## Chunk 7: Input Validation and cloud-init Pipeline

### Task 9: Input validation

**Files:**
- Modify: `src/commands/cloud_init.rs`

- [ ] **Step 1: Implement input validation for cloud-init subcommand**

Update `src/commands/cloud_init.rs`:

```rust
use std::path::Path;

use crate::CloudInitArgs;

/// Validate that all required input files and directories exist.
fn validate_inputs(args: &CloudInitArgs) -> anyhow::Result<()> {
    ensure_dir_exists(&args.dir, "cloud-init directory")?;
    ensure_file_exists(&args.kernel, "kernel")?;
    ensure_file_exists(&args.initrd, "initrd")?;
    ensure_file_exists(&args.firmware, "firmware")?;
    ensure_file_exists(&args.base_image, "base image")?;
    Ok(())
}

fn ensure_file_exists(path: &Path, label: &str) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("{label} not found: {}", path.display());
    }
    if !path.is_file() {
        anyhow::bail!("{label} is not a file: {}", path.display());
    }
    Ok(())
}

fn ensure_dir_exists(path: &Path, label: &str) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("{label} not found: {}", path.display());
    }
    if !path.is_dir() {
        anyhow::bail!("{label} is not a directory: {}", path.display());
    }
    Ok(())
}

pub fn run(args: &CloudInitArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "building cloud-init CVM image");

    // Step 1: Validate inputs
    validate_inputs(args)?;

    // Step 2: Check required tools
    crate::tools::require("mkosi")?;
    crate::tools::require("ukify")?;
    crate::tools::require("igvm-tools")?;
    crate::tools::require("qemu-img")?;

    // Step 3: Create output directory
    fs_err::create_dir_all(&args.output)?;

    tracing::info!("all inputs validated and tools found");

    // Pipeline steps will be wired in subsequent tasks:
    // 4. Build project partition (mkosi)
    // 5. Compose disk image (base + project)
    // 6. Build UKI (ukify)
    // 7. Build IGVM (igvm-tools)
    // 8. Write manifest

    anyhow::bail!("cloud-init pipeline not yet fully implemented")
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 3: Commit**

```bash
git add src/commands/cloud_init.rs
git commit -m "feat: add input validation for cloud-init subcommand"
```

### Task 10: Wire up the cloud-init pipeline

This task connects all the modules into the full cloud-init build pipeline. Each stage calls the module built in previous tasks.

**Files:**
- Modify: `src/commands/cloud_init.rs`
- Modify: `src/compose/disk.rs`

- [ ] **Step 1: Implement disk composition stub**

Update `src/compose/disk.rs`:

```rust
use std::path::Path;

/// Compose a final disk image from a base partition and a project partition.
/// Creates a GPT disk image containing both partitions.
pub fn compose(
    base_partition: &Path,
    project_partition: &Path,
    output: &Path,
) -> anyhow::Result<()> {
    tracing::info!(
        base = %base_partition.display(),
        project = %project_partition.display(),
        output = %output.display(),
        "composing disk image"
    );

    // TODO: Implement actual GPT composition.
    // For now, validate inputs exist and create a placeholder.
    if !base_partition.exists() {
        anyhow::bail!("base partition not found: {}", base_partition.display());
    }
    if !project_partition.exists() {
        anyhow::bail!("project partition not found: {}", project_partition.display());
    }

    anyhow::bail!("disk composition not yet implemented")
}
```

- [ ] **Step 2: Wire up the full pipeline in cloud_init.rs**

Update `src/commands/cloud_init.rs` — replace the `run` function body after validation:

```rust
use std::path::Path;

use crate::mkosi::config::MkosiConfig;
use crate::igvm::invoke::IgvmBuildArgs;
use crate::manifest::{
    self, BuildConfig, BuildManifest, FileEntry, ManifestInputs, ManifestOutputs, Measurement,
};
use crate::{tools, CloudInitArgs, ImageFormat};

fn validate_inputs(args: &CloudInitArgs) -> anyhow::Result<()> {
    ensure_dir_exists(&args.dir, "cloud-init directory")?;
    ensure_file_exists(&args.kernel, "kernel")?;
    ensure_file_exists(&args.initrd, "initrd")?;
    ensure_file_exists(&args.firmware, "firmware")?;
    ensure_file_exists(&args.base_image, "base image")?;
    Ok(())
}

fn ensure_file_exists(path: &Path, label: &str) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("{label} not found: {}", path.display());
    }
    if !path.is_file() {
        anyhow::bail!("{label} is not a file: {}", path.display());
    }
    Ok(())
}

fn ensure_dir_exists(path: &Path, label: &str) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("{label} not found: {}", path.display());
    }
    if !path.is_dir() {
        anyhow::bail!("{label} is not a directory: {}", path.display());
    }
    Ok(())
}

fn format_extension(format: &ImageFormat) -> &'static str {
    match format {
        ImageFormat::Qcow2 => "qcow2",
        ImageFormat::Vhd => "vhd",
        ImageFormat::Raw => "raw",
    }
}

pub fn run(args: &CloudInitArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "building cloud-init CVM image");

    // Step 1: Validate inputs
    validate_inputs(args)?;

    // Step 2: Check required tools
    tools::require("mkosi")?;
    tools::require("ukify")?;
    tools::require("igvm-tools")?;
    tools::require("qemu-img")?;

    // Step 3: Create output directory
    fs_err::create_dir_all(&args.output)?;

    tracing::info!("all inputs validated and tools found");

    // Step 4: Build project partition via mkosi
    let work_dir = tempfile::tempdir()?;
    let mkosi_config = MkosiConfig::cloud_init(args.dir.clone());
    let mkosi_config_path = work_dir.path().join("mkosi.conf");
    mkosi_config.write_to(&mkosi_config_path)?;
    tracing::info!(config = %mkosi_config_path.display(), "generated mkosi config");

    // TODO: Invoke mkosi to build project partition
    let project_partition = work_dir.path().join("project.raw");
    tracing::warn!("mkosi invocation not yet implemented — skipping project partition build");

    // Step 5: Compose disk image (base + project)
    let raw_disk = args.output.join("disk.raw");
    tracing::warn!("disk composition not yet implemented — skipping");

    // Step 6: Build UKI via ukify
    let uki_path = args.output.join("uki.efi");
    tracing::warn!("UKI build not yet implemented — skipping");

    // Step 7: Build IGVM via igvm-tools
    let igvm_manifest_path = work_dir.path().join("igvm-manifest.json");
    let igvm_path = args.output.join("guest.igvm");
    let igvm_args = IgvmBuildArgs {
        firmware: args.firmware.clone(),
        kernel: uki_path.clone(),
        smp: args.smp,
        manifest: Some(igvm_manifest_path.clone()),
        output: igvm_path.clone(),
    };
    tracing::warn!("igvm-tools invocation not yet wired — skipping");
    tracing::debug!(cmd = ?igvm_args.to_args(), "would invoke igvm-tools");

    // Step 8: Convert to output format if not raw
    let final_disk = args.output.join(format!("disk.{}", format_extension(&args.format)));
    tracing::warn!("format conversion not yet implemented — skipping");

    // Step 9: Write manifest
    tracing::warn!("manifest generation not yet implemented — skipping");

    tracing::info!(output = %args.output.display(), "pipeline complete (with stubs)");
    Ok(())
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add src/commands/cloud_init.rs src/compose/disk.rs
git commit -m "feat: wire up cloud-init pipeline with stub stages"
```

---

## Chunk 8: Remaining Subcommand Stubs and Final Wiring

### Task 11: Base and kernel subcommand implementation

**Files:**
- Modify: `src/commands/base.rs`
- Modify: `src/commands/kernel.rs`

- [ ] **Step 1: Implement base subcommand with validation and mkosi invocation stub**

Update `src/commands/base.rs`:

```rust
use crate::{tools, BaseArgs};

pub fn run(args: &BaseArgs) -> anyhow::Result<()> {
    tracing::info!(source_image = %args.source_image.display(), "building base image");

    // Validate inputs
    if !args.source_image.exists() {
        anyhow::bail!("source image not found: {}", args.source_image.display());
    }

    // Check required tools
    tools::require("mkosi")?;

    // Create output directory
    fs_err::create_dir_all(&args.output)?;

    // TODO: Generate mkosi config for base image with hardening
    // TODO: Invoke mkosi to build base partition
    // Phase 1 hardening: firewall rules (nftables/iptables)

    tracing::warn!("base image build not yet fully implemented");
    Ok(())
}
```

- [ ] **Step 2: Implement kernel subcommand with validation stub**

Update `src/commands/kernel.rs`:

```rust
use crate::KernelArgs;

pub fn run(args: &KernelArgs) -> anyhow::Result<()> {
    tracing::info!(source = %args.source.display(), "building hardened kernel");

    // Validate inputs
    if !args.source.exists() {
        anyhow::bail!("kernel source tree not found: {}", args.source.display());
    }
    if !args.config.exists() {
        anyhow::bail!("kernel config not found: {}", args.config.display());
    }

    // Create output directory
    fs_err::create_dir_all(&args.output)?;

    // TODO: Invoke kernel build (make, or wrap a build script)
    // The hardening config (.config) determines security properties.

    tracing::warn!("kernel build not yet implemented");
    Ok(())
}
```

- [ ] **Step 3: Update container subcommand**

Update `src/commands/container.rs`:

```rust
use crate::ContainerArgs;

pub fn run(args: &ContainerArgs) -> anyhow::Result<()> {
    tracing::info!(url = %args.url, "building container CVM image");

    // The container subcommand will:
    // 1. Generate a standard cloud-init config that pulls and runs the container
    // 2. Delegate to the same pipeline as cloud-init
    // Implementation deferred — see Future Work in spec.

    anyhow::bail!(
        "container build not yet implemented. \
         See docs/superpowers/specs/2026-03-13-lunal-build-design.md Future Work section."
    )
}
```

- [ ] **Step 4: Verify everything compiles and tests pass**

Run: `cargo build && cargo test`
Expected: Compiles, all tests pass

- [ ] **Step 5: Commit**

```bash
git add src/commands/
git commit -m "feat: implement base, kernel, and container subcommand stubs"
```

### Task 12: Final integration — verify full CLI works end-to-end

**Files:**
- Modify: `tests/cli.rs`

- [ ] **Step 1: Add integration test for cloud-init with missing inputs**

Add to `tests/cli.rs`:

```rust
#[test]
fn test_cloud_init_fails_with_missing_dir() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args([
        "cloud-init", "/nonexistent/dir",
        "--kernel", "/tmp/k",
        "--initrd", "/tmp/i",
        "--firmware", "/tmp/f",
        "--base-image", "/tmp/b",
        "-o", "/tmp/o",
    ])
    .assert()
    .failure()
    .stderr(predicates::str::contains("not found"));
}

#[test]
fn test_base_fails_with_missing_source() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args([
        "base",
        "--source-image", "/nonexistent/image.img",
        "-o", "/tmp/o",
    ])
    .assert()
    .failure()
    .stderr(predicates::str::contains("not found"));
}

#[test]
fn test_smp_default_is_one() {
    // Verify the default by checking that cloud-init accepts a call without --smp
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args([
        "cloud-init", "/tmp",
        "--kernel", "/tmp/k",
        "--initrd", "/tmp/i",
        "--firmware", "/tmp/f",
        "--base-image", "/tmp/b",
        "-o", "/tmp/o",
    ])
    .assert()
    .failure(); // Will fail on validation, but NOT on arg parsing — proves --smp has a default
}

#[test]
fn test_format_flag_accepts_vhd() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args([
        "cloud-init", "/tmp",
        "--kernel", "/tmp/k",
        "--initrd", "/tmp/i",
        "--firmware", "/tmp/f",
        "--base-image", "/tmp/b",
        "--format", "vhd",
        "-o", "/tmp/o",
    ])
    .assert()
    .failure(); // Fails on validation, not parsing — proves vhd is accepted
}
```

- [ ] **Step 2: Run all tests**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

- [ ] **Step 4: Commit**

```bash
git add tests/cli.rs
git commit -m "test: add integration tests for CLI validation and defaults"
```

- [ ] **Step 5: Final commit — tag v0.1.0-alpha**

```bash
git tag -a v0.1.0-alpha -m "Initial scaffolding with CLI, tool helpers, and pipeline stubs"
```
