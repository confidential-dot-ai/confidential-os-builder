# Future plans

## Base image

- [x] Build a base image, from Ubuntu Resolute Raccoon, that can host our refinements
- [ ] Finalize `steep base` command that builds the image using mkosi
- [ ] Compress the base image, make it read-only, and boot with an overlay for writes

## cloud-init and project images

- [x] Configure the base image with cloud-init and runs a mounted ISO with cloud-init config
- [x] Release a built image, with instructions for use, that accepts cloud-init configuration
- [ ] Release an image with privateclaw cloud-init baked in already
- [ ] Finalize `steep cloud-init` that accepts a cloud-init dir and creates the image files and qemu commands

## `steep run`

- [ ] Accept a steep output directory, with base and cloud-init images, and run qemu

## Kernel hardening

- [ ] Build a kernel via `steep kernel`
- [ ] Add kernel hardening configurations
- [ ] Modify cloud-init to build the kernel and adjust the qemu command to use it
