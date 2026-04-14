# Future plans

- [ ] merge steep seal command into build command
- [x] create GHA pipeline to build base image
- [x] push base image into GHCR after builds
- [x] add k8s base image and pushed to GHCR
- [ ] add kettle builder service image pushed to GHCR

## Base image

- [x] Build a base image, from Ubuntu Resolute Raccoon, that can host our refinements
- [x] Finalize `steep base` command that builds the image using mkosi
- [x] Create an image overlay for any uses of the base image
- [ ] Add firewall rules that block all incoming and outgoing traffic at the end of the build

## cloud-init and project images

- [x] Configure the base image with cloud-init and runs a mounted ISO with cloud-init config
- [x] Release a built image, with instructions for use, that accepts cloud-init configuration
- [x] Release an image with privateclaw cloud-init baked in already
- [x] Finalize `steep cloud-init` that accepts a cloud-init dir and creates the image files and qemu commands
- [ ] Switch from qcow2 backed by base image to dm-verity with overlayfs and a copy-on-write partition in one image
- [ ] Call `run` with the option to run cloud-init can run, clean up after itself, and save a prepared image

## `steep run`

- [x] Accept a steep output directory, with base and cloud-init images, and run qemu
- [ ] Use kvm, but only if available
- [ ] Option to save an image and shut down the VM after cloud-init finishes running

## Kernel hardening

- [ ] Build a kernel via `steep kernel`
- [ ] Add kernel hardening configurations
- [ ] Modify cloud-init to build the kernel and adjust the qemu command to use it
