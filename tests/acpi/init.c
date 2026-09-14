/* Disposable test init: enumerate ACPI markers, then shut down the VM. */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/stat.h>
#include <sys/sysinfo.h>
#include <unistd.h>

int main(void)
{
    setbuf(stdout, NULL);
    mkdir("/proc", 0755);
    mkdir("/sys", 0755);
    if (mount("proc", "/proc", "proc", 0, NULL) ||
        mount("sysfs", "/sys", "sysfs", 0, NULL)) {
        puts("AMLTEST: mount failed");
        goto shutdown;
    }
    DIR *dir = opendir("/sys/bus/acpi/devices");
    if (!dir) {
        puts("AMLTEST: ACPI devices missing");
        goto shutdown;
    }
    struct dirent *entry;
    while ((entry = readdir(dir)))
        printf("AMLTEST: DEVICE %s\n", entry->d_name);
    closedir(dir);
    const char *tables[] = {"FACP", "APIC", "HPET", "MCFG"};
    for (size_t i = 0; i < sizeof(tables) / sizeof(*tables); i++) {
        char path[128];
        unsigned char data[4096];
        snprintf(path, sizeof(path), "/sys/firmware/acpi/tables/%s", tables[i]);
        int fd = open(path, O_RDONLY);
        if (fd < 0) continue;
        ssize_t length = read(fd, data, sizeof(data));
        close(fd);
        if (length < 36 || length == (ssize_t)sizeof(data)) continue;
        printf("AMLTEST: TABLE %s ", tables[i]);
        for (ssize_t n = 0; n < length; n++) printf("%02x", data[n]);
        puts("");
    }
    printf("AMLTEST: CPUS %ld\n", sysconf(_SC_NPROCESSORS_ONLN));
    struct sysinfo info;
    if (!sysinfo(&info))
        printf("AMLTEST: MEMORY_MB %lu\n", info.totalram / (1024 * 1024 / info.mem_unit));
    dir = opendir("/sys/bus/pci/devices");
    unsigned int pci_count = 0;
    if (dir) {
        while ((entry = readdir(dir))) {
            if (entry->d_name[0] == '.') continue;
            pci_count++;
            printf("AMLTEST: PCI %s\n", entry->d_name);
        }
        closedir(dir);
    }
    printf("AMLTEST: PCI_COUNT %u\n", pci_count);
    puts("AMLTEST: USERSPACE_REACHED");
shutdown:
    fflush(stdout);
    reboot(RB_POWER_OFF);
    for (;;) pause();
}
