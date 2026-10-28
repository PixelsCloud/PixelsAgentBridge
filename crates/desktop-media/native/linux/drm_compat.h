#pragma once
#include <cerrno>
#include <cstdint>
#include <sys/syscall.h>
#include <unistd.h>

namespace pab_linux::dma {
// Connector numbers/names are the DRM UAPI, including on older libdrm builds
// that do not export drmModeGetConnectorTypeName. Unknown types stay rejected.
inline const char* connector_type_name(uint32_t type) noexcept {
    static constexpr const char* names[] = {
        nullptr, "VGA", "DVI-I", "DVI-D", "DVI-A", "Composite", "SVIDEO",
        "LVDS", "Component", "DIN", "DP", "HDMI-A", "HDMI-B", "TV",
        "eDP", "Virtual", "DSI", "DPI", "Writeback", "SPI", "USB"
    };
    return type < sizeof(names) / sizeof(names[0]) ? names[type] : nullptr;
}

inline int duplicate_process_fd(int process, int descriptor) noexcept {
#if defined(SYS_pidfd_getfd)
    return int(syscall(SYS_pidfd_getfd, process, descriptor, 0));
#elif defined(__NR_pidfd_getfd)
    return int(syscall(__NR_pidfd_getfd, process, descriptor, 0));
#elif (defined(__x86_64__) && !defined(__ILP32__)) || defined(__aarch64__)
    // Linux's x86_64 and asm-generic syscall tables assign 438. Older libc
    // headers may omit the macro while the running kernel supports the call.
    return int(syscall(438L, process, descriptor, 0));
#else
    errno = ENOSYS;
    return -1;
#endif
}
}
