# steep demo: Repeatable End-to-End Demonstration

## Overview

A runnable demo that exercises both `steep cloud-init` and `steep container` end-to-end. Each example builds a confidential VM image running caddy serving a static success page. `bin/demo` runs both in parallel via tmux; each example can also be run independently.

## File Layout

```
examples/
├── cloud-init/
│   ├── run.sh          # full pipeline: base → cloud-init → steep run --port-forward 8080:80
│   ├── meta-data       # cloud-init instance-id + hostname
│   └── user-data       # installs caddy via apt repo, writes index.html, starts caddy
└── container/
    ├── run.sh          # full pipeline: base → container → steep run --port-forward 8081:80
    ├── Dockerfile      # FROM caddy:latest + baked-in Caddyfile + index.html
    ├── Caddyfile       # :80 { root * /usr/share/caddy; file_server }
    └── index.html      # static success page

bin/
└── demo               # opens tmux session, runs base once then each run.sh in a pane
```

## `steep base` Changes: copy output to `args.output`

Currently `commands/base.rs` creates `args.output` but never writes anything into it — the mkosi output lands in a `TempDir` that is dropped at the end of the function. `steep base` must be fixed to copy the mkosi output into `args.output/base.raw` so downstream commands can use it.

Change: after `config.invoke(work_dir.path())`, copy `work_dir/image.raw` to `args.output/base.raw`. `image.raw` is the default mkosi output filename when no `Output=` key is set.

## `steep run` Changes: `--port-forward`

`RunArgs` gains an optional, repeatable flag:

```
--port-forward HOST:GUEST   # e.g. --port-forward 8080:80
```

Multiple forwards are supported (e.g. `--port-forward 8080:80 --port-forward 8443:443`).

`QemuArgs` gains a new field:

```rust
pub port_forwards: Vec<(u16, u16)>,  // (host_port, guest_port)
```

`commands/run.rs` populates this field from `args.port_forwards` when constructing `QemuArgs`.

In `QemuArgs::to_args()`, if `port_forwards` is non-empty, all forwards are combined into a single `-netdev user` device:

```
-netdev user,id=net0,hostfwd=tcp::8080-:80,hostfwd=tcp::8443-:443
-device virtio-net-pci,netdev=net0
```

The `-device virtio-net-pci,netdev=net0` line appears once regardless of how many forwards are specified. If `port_forwards` is empty, no network device is added (preserving the current behaviour).

## Shared Inputs

| Input | Value |
|-------|-------|
| Kernel | `/boot/vmlinuz-$(uname -r)` |
| Initrd | `/boot/initrd.img-$(uname -r)` |
| Firmware | `~/.local/share/steep/OVMF.fd` |
| Base image URL | `https://cloud-images.ubuntu.com/resolute/current/resolute-server-cloudimg-amd64v3.img` |
| Base image cache | `~/.local/share/steep/base-inputs/resolute-server-cloudimg-amd64v3.img` (managed by `steep base` via `source::resolve()`) |
| Service port (cloud-init) | 8080 (host) → 80 (guest) |
| Service port (container) | 8081 (host) → 80 (guest) |

## Example: `examples/cloud-init/`

### `meta-data`

```yaml
instance-id: steep-demo-cloud-init
local-hostname: steep-demo
```

### `user-data`

Installs caddy from the official apt repo, writes the success page, writes the Caddyfile after install, and starts caddy on port 80. The Caddyfile is written in `runcmd` (after `apt-get install caddy`) to avoid being overwritten by the package's default conffile.

```yaml
#cloud-config
packages:
  - debian-keyring
  - debian-archive-keyring
  - apt-transport-https
  - curl

write_files:
  - path: /var/www/html/index.html
    content: |
      <!DOCTYPE html>
      <html><head><title>steep demo</title></head>
      <body><h1>steep demo</h1>
      <p>Served by caddy inside a confidential VM built with steep (cloud-init).</p>
      </body></html>

runcmd:
  - curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  - curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | tee /etc/apt/sources.list.d/caddy-stable.list
  - apt-get update
  - apt-get install -y caddy
  - |
    cat > /etc/caddy/Caddyfile << 'EOF'
    :80 {
        root * /var/www/html
        file_server
    }
    EOF
  - systemctl enable --now caddy
```

### `run.sh` pipeline

```
KERNEL=/boot/vmlinuz-$(uname -r)
INITRD=/boot/initrd.img-$(uname -r)
FIRMWARE=~/.local/share/steep/OVMF.fd
BASE_IMAGE=output/demo/base/base.raw
OUTPUT=output/demo/cloud-init
PORT=8080
```

Steps:
1. Parse `--force` flag; if set, remove `$OUTPUT` before proceeding
2. Build base image if `output/demo/base/base.raw` does not exist:
   ```bash
   steep base \
       --source-image "https://cloud-images.ubuntu.com/resolute/current/resolute-server-cloudimg-amd64v3.img" \
       -o output/demo/base
   ```
   (`steep base` handles download + caching internally via `source::resolve()`)
3. Build cloud-init image if `$OUTPUT/manifest.json` does not exist:
   ```bash
   steep cloud-init examples/cloud-init \
       --kernel "$KERNEL" \
       --initrd "$INITRD" \
       --firmware "$FIRMWARE" \
       --base-image "$BASE_IMAGE" \
       --service-port 80 \
       -o "$OUTPUT"
   ```
4. Print URL: `http://localhost:8080` (printed before launch so the user can copy it while the VM boots; caddy takes ~10–30s to be reachable after the VM starts)
5. Launch VM (foreground):
   ```bash
   sudo steep run --port-forward 8080:80 "$OUTPUT"
   ```

## Example: `examples/container/`

### `index.html`

```html
<!DOCTYPE html>
<html><head><title>steep demo</title></head>
<body><h1>steep demo</h1>
<p>Served by caddy inside a confidential VM built with steep (container).</p>
</body></html>
```

### `Caddyfile`

```
:80 {
    root * /usr/share/caddy
    file_server
}
```

### `Dockerfile`

```dockerfile
FROM caddy:latest
COPY index.html /usr/share/caddy/index.html
COPY Caddyfile /etc/caddy/Caddyfile
```

### `run.sh` pipeline

```
KERNEL=/boot/vmlinuz-$(uname -r)
INITRD=/boot/initrd.img-$(uname -r)
FIRMWARE=~/.local/share/steep/OVMF.fd
BASE_IMAGE=output/demo/base/base.raw
OUTPUT=output/demo/container
PORT=8081
IMAGE=steep-demo-container:latest
```

Steps:
1. Parse `--force` flag; if set, remove `$OUTPUT` before proceeding
2. Build base image if `output/demo/base/base.raw` does not exist:
   ```bash
   steep base \
       --source-image "https://cloud-images.ubuntu.com/resolute/current/resolute-server-cloudimg-amd64v3.img" \
       -o output/demo/base
   ```
3. Build local container image (always, to keep it current):
   ```bash
   podman build -t steep-demo-container:latest examples/container/
   ```
4. Build container CVM image if `$OUTPUT/manifest.json` does not exist:
   ```bash
   steep container steep-demo-container:latest \
       --kernel "$KERNEL" \
       --initrd "$INITRD" \
       --firmware "$FIRMWARE" \
       --base-image "$BASE_IMAGE" \
       --service-port 80 \
       -o "$OUTPUT"
   ```
5. Print URL: `http://localhost:8081` (printed before launch; same boot-time caveat as cloud-init)
6. Launch VM (foreground):
   ```bash
   sudo steep run --port-forward 8081:80 "$OUTPUT"
   ```

## `bin/demo` Orchestrator

`bin/demo` builds the shared base image sequentially before splitting into panes, to avoid a race condition when both `run.sh` scripts are invoked in parallel with `--force`.

Steps:
1. Parse `--force`; pass through to both `run.sh` calls
2. Check `tmux` is available (fail with clear message if not)
3. Kill existing `steep-demo` tmux session if it exists (terminating any running VMs), then create a fresh one — this is intentional; re-running `bin/demo` always starts clean
4. Build base image (outside tmux, sequentially):
   - If `--force`: remove `output/demo/base/`, then run `steep base --source-image <URL> -o output/demo/base`
   - Otherwise: skip if `output/demo/base/base.raw` exists
5. Split tmux into two panes:
   - Left pane: `examples/cloud-init/run.sh [--force]` (base step skips, already built)
   - Right pane: `examples/container/run.sh [--force]` (base step skips, already built)
6. Attach to session

## Output Artifacts

```
output/demo/
├── base/
│   └── base.raw
├── cloud-init/
│   ├── disk.qcow2
│   ├── guest.igvm
│   ├── uki.efi
│   └── manifest.json
└── container/
    ├── disk.qcow2
    ├── guest.igvm
    ├── uki.efi
    └── manifest.json
```

## Idempotency

| Stage | Skip condition | `--force` behaviour |
|-------|---------------|---------------------|
| Base image download | Handled automatically by `source::resolve()` — skips if file exists in cache | Cache at `~/.local/share/steep/base-inputs/` is never invalidated; `steep base` re-runs mkosi from the cached file but does not re-download |
| `steep base` | `output/demo/base/base.raw` exists | Remove `output/demo/base/` and rerun `steep base` (download still skipped) |
| `steep cloud-init` | `output/demo/cloud-init/manifest.json` exists | Remove `output/demo/cloud-init/` and rebuild |
| `steep container` | `output/demo/container/manifest.json` exists | Remove `output/demo/container/` and rebuild |
| `podman build` | Never skipped | N/A — always re-run to pick up changes to `examples/container/` |

`--force` in each `run.sh` removes only that script's `$OUTPUT` directory (`output/demo/cloud-init` or `output/demo/container`). It never removes `output/demo/base/`. Base removal is handled exclusively by `bin/demo` before splitting panes, which prevents a race condition when both scripts run in parallel.

The `run.sh` scripts are not safe to run concurrently without `bin/demo`, as both contain the base build step and could race on `output/demo/base/` if it does not exist.

## QEMU Launch

The demo uses SEV-SNP hardware. `sudo steep run` is required for hardware access. The `--port-forward` flag adds user-mode networking to the QEMU invocation. Each VM runs in the foreground of its tmux pane (or terminal when run standalone).
