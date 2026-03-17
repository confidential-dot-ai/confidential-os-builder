# Future plans

1. Remove mkosi rust code and just maintain a mkosi configuration folder for the base image.
2. Add a flag to `steep run` that uses qemu to run a built VM image without IGVM turned on, for testing and demonstration on machines where we don't have both SEV-SNP and KVM available.
