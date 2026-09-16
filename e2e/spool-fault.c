#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

/* Inject only into the test-owned journal descriptor; never exhaust host storage. */
ssize_t write(int fd, const void *buffer, size_t length) {
    const char *target = getenv("E2E_SPOOL_FAULT_PATH");
    const char *control = getenv("E2E_SPOOL_FAULT_CONTROL");
    if (target != NULL && control != NULL) {
        char descriptor[64];
        char path[PATH_MAX + 1];
        snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
        ssize_t size = readlink(descriptor, path, PATH_MAX);
        if (size >= 0) {
            path[size] = '\0';
            if (strcmp(path, target) == 0) {
                int input = open(control, O_RDONLY | O_CLOEXEC);
                if (input >= 0) {
                    char mode[16] = {0};
                    ssize_t bytes = read(input, mode, sizeof(mode) - 1);
                    close(input);
                    if (bytes > 0 && (strcmp(mode, "ENOSPC") == 0 || strcmp(mode, "EACCES") == 0)) {
                        errno = strcmp(mode, "ENOSPC") == 0 ? ENOSPC : EACCES;
                        return -1;
                    }
                }
            }
        }
    }
    return syscall(SYS_write, fd, buffer, length);
}
