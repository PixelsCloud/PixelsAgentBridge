// Compile/run with Linux headers only; never opens a screen, compositor or GPU.
#include <cstdarg>
#include <cerrno>
#include <cstdint>
#include <sys/syscall.h>
#include <unistd.h>
#include <xf86drmMode.h>

static long observed_number;
static int observed_process, observed_descriptor, observed_flags;
static long controlled_syscall(long number, ...) noexcept {
    observed_number = number;
    va_list values;
    va_start(values, number);
    observed_process = va_arg(values, int);
    observed_descriptor = va_arg(values, int);
    observed_flags = va_arg(values, int);
    va_end(values);
    errno = EPERM;
    return -1;
}

#ifdef PAB_TEST_OLD_SYSCALL_HEADERS
#undef SYS_pidfd_getfd
#undef __NR_pidfd_getfd
#else
#ifndef SYS_pidfd_getfd
#define SYS_pidfd_getfd 438
#endif
#endif
#define syscall controlled_syscall
#include "drm_compat.h"
#undef syscall

#include <cstring>
#include <iostream>
int main() {
    using namespace pab_linux::dma;
    const auto name = [](uint32_t type, const char* expected) {
        const char* actual = connector_type_name(type);
        return actual && !std::strcmp(actual, expected);
    };
    if (!name(DRM_MODE_CONNECTOR_DisplayPort, "DP") ||
        !name(DRM_MODE_CONNECTOR_HDMIA, "HDMI-A") ||
        !name(DRM_MODE_CONNECTOR_eDP, "eDP") ||
        connector_type_name(0) || connector_type_name(UINT32_MAX)) return 1;
    // Keep the pidfd (not a numeric process ID), descriptor and kernel error
    // intact. No fallback may reopen /proc/PID/fd after a failed pinned request.
    if (duplicate_process_fd(31, 47) != -1 || errno != EPERM ||
        observed_number != 438 || observed_process != 31 ||
        observed_descriptor != 47 || observed_flags != 0) return 2;
    std::cout << "Linux media compatibility probe passed\n";
}
