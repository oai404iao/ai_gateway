#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

/* Inject only into test-owned descriptors; never exhaust host storage. */
static int fault_errno(int fd, int syncing) {
    const char *target = getenv("E2E_SPOOL_FAULT_PATH");
    const char *control = getenv("E2E_SPOOL_FAULT_CONTROL");
    if (target != NULL && control != NULL) {
        char descriptor[64];
        char path[PATH_MAX + 1];
        snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
        ssize_t size = readlink(descriptor, path, PATH_MAX);
        if (size >= 0) {
            path[size] = '\0';
            size_t target_length = strlen(target);
            if (strcmp(path, target) == 0 ||
                (target_length > 0 && target[target_length - 1] == '/' &&
                 strncmp(path, target, target_length) == 0)) {
                int input = open(control, O_RDONLY | O_CLOEXEC);
                if (input >= 0) {
                    char mode[16] = {0};
                    ssize_t bytes = read(input, mode, sizeof(mode) - 1);
                    close(input);
                    if (bytes > 0) {
                        if (syncing && strcmp(mode, "EIO_SYNC") == 0) return EIO;
                        if (!syncing && strcmp(mode, "ENOSPC") == 0) return ENOSPC;
                        if (!syncing && strcmp(mode, "EACCES") == 0) return EACCES;
                    }
                }
            }
        }
    }
    return 0;
}

ssize_t write(int fd, const void *buffer, size_t length) {
    int error = fault_errno(fd, 0);
    if (error != 0) { errno = error; return -1; }
    return syscall(SYS_write, fd, buffer, length);
}

int fsync(int fd) {
    int error = fault_errno(fd, 1);
    if (error != 0) { errno = error; return -1; }
    return syscall(SYS_fsync, fd);
}

int fdatasync(int fd) {
    int error = fault_errno(fd, 1);
    if (error != 0) { errno = error; return -1; }
    return syscall(SYS_fdatasync, fd);
}
