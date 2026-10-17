#pragma once
#include "drm_export.h"
#include <dirent.h>
#include <sys/syscall.h>
#include <sys/un.h>

namespace pab_linux::dma {
inline std::string broker_path() {
    const char* data = getenv("PAB_DATA_DIR");
    if (!data || data[0] != '/') throw std::runtime_error("No supervised DMA channel");
    std::string parent(data);
    const auto split = parent.find_last_of('/');
    if (split == std::string::npos || parent.substr(split) != "/data")
        throw std::runtime_error("No supervised DMA profile");
    parent.resize(split);
    struct stat info{};
    if (lstat(parent.c_str(), &info) || !S_ISDIR(info.st_mode) || info.st_uid || (info.st_mode & 0022))
        throw std::runtime_error("DMA profile is not supervisor-owned");
    return parent + "/dma.sock";
}
inline Fd connect_broker(const std::string& connector) {
    const auto path = broker_path();
    sockaddr_un address{}; address.sun_family = AF_UNIX;
    if (path.size() >= sizeof(address.sun_path) || connector.empty() || connector.size() > 120)
        throw std::runtime_error("DMA broker request outside bounds");
    memcpy(address.sun_path, path.c_str(), path.size() + 1);
    Fd socket(::socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC | SOCK_NONBLOCK, 0));
    if (socket.get() < 0 || ::connect(socket.get(), reinterpret_cast<sockaddr*>(&address), sizeof(address)))
        throw std::runtime_error("DMA supervisor is unavailable");
    ucred peer{}; socklen_t length = sizeof(peer);
    if (getsockopt(socket.get(), SOL_SOCKET, SO_PEERCRED, &peer, &length) || length != sizeof(peer) || peer.uid)
        throw std::runtime_error("DMA endpoint is not the system supervisor");
    // Small first packet selects a connector; subsequent packets retain the
    // existing versioned request/SCM_RIGHTS protocol. Never send a device path.
    const auto request = std::string("PABD1:") + connector;
    if (send(socket.get(), request.data(), request.size(), MSG_NOSIGNAL) != ssize_t(request.size()))
        throw std::runtime_error("Cannot request supervised DMA capture");
    return socket;
}
inline std::string read_connector(int socket) {
    channel(socket);
    pollfd pending{socket, POLLIN, 0};
    int ready;
    do { ready = poll(&pending, 1, 3000); } while (ready < 0 && errno == EINTR);
    if (ready <= 0) throw std::runtime_error("DMA connector request timed out");
    std::array<char, 126> buffer{};
    const auto count = recv(socket, buffer.data(), buffer.size(), MSG_TRUNC);
    if (count <= 6 || count > ssize_t(buffer.size()) || memcmp(buffer.data(), "PABD1:", 6))
        throw std::runtime_error("Invalid DMA connector request");
    std::string connector(buffer.data() + 6, size_t(count) - 6);
    if (connector.find_first_not_of("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_") != std::string::npos)
        throw std::runtime_error("Invalid DMA connector name");
    return connector;
}
// Resolve nodes from the pinned compositor file, not caller-supplied paths.
// vmwgfx requires its original DRM file; other drivers use a new primary file.
inline void export_session(unsigned pid, int process, int socket) {
    if (geteuid() != 0 || pid <= 1 || process < 3 || socket < 3)
        throw std::runtime_error("DMA export requires the session supervisor");
    const auto connector = read_connector(socket);
    const auto directory = std::string("/proc/") + std::to_string(pid) + "/fd";
    struct DirDrop { void operator()(DIR* d) const { if (d) closedir(d); } };
    std::unique_ptr<DIR, DirDrop> descriptors(opendir(directory.c_str()));
    if (!descriptors) throw std::runtime_error("Compositor descriptors are unavailable");
    Fd card, render; bool borrowed = false; unsigned matches = 0;
    while (auto entry = readdir(descriptors.get())) {
        const std::string name(entry->d_name);
        if (name.empty() || name.find_first_not_of("0123456789") != std::string::npos) continue;
        const auto path = directory + "/" + name;
        std::array<char, 256> link{};
        const auto size = readlink(path.c_str(), link.data(), link.size());
        if (size < 0 || size >= ssize_t(link.size())) continue;
        const std::string node(link.data(), size_t(size));
        if (node.rfind("/dev/dri/card", 0) != 0) continue;
        // pidfd_getfd pins the original process even if the numeric PID is
        // reused while procfs is enumerated. A dead original cannot export.
        Fd candidate(int(syscall(SYS_pidfd_getfd, process, number(name.c_str(), 1048576), 0)));
        if (candidate.get() < 0) throw std::runtime_error("Cannot duplicate compositor DRM descriptor");
        struct stat actual{}, expected{};
        if (fstat(candidate.get(), &actual) || stat(node.c_str(), &expected) ||
            !S_ISCHR(actual.st_mode) || actual.st_rdev != expected.st_rdev)
            throw std::runtime_error("Compositor DRM descriptor changed");
        drmDevicePtr device = nullptr;
        if (drmGetDevice2(candidate.get(), 0, &device) || !device) continue;
        struct DeviceDrop { drmDevicePtr* p; ~DeviceDrop() { drmFreeDevice(p); } } drop{&device};
        if (!(device->available_nodes & (1 << DRM_NODE_PRIMARY)) ||
            !(device->available_nodes & (1 << DRM_NODE_RENDER))) continue;
        Fd observer(open(device->nodes[DRM_NODE_PRIMARY], O_RDONLY | O_CLOEXEC | O_NOFOLLOW));
        if (observer.get() < 0) continue;
        Resources resources(drmModeGetResources(observer.get()));
        if (!resources || resources->count_connectors < 1 || resources->count_connectors > 64) continue;
        bool found = false;
        for (int i = 0; i < resources->count_connectors; ++i) {
            Connector output(drmModeGetConnector(observer.get(), resources->connectors[i]));
            if (!output) continue;
            const char* type = drmModeGetConnectorTypeName(output->connector_type);
            if (type && std::string(type) + "-" + std::to_string(output->connector_type_id) == connector &&
                output->connection == DRM_MODE_CONNECTED && output->encoder_id) found = true;
        }
        if (!found) continue;
        // Some compositors retain duplicate fds for the same primary device.
        if (card.get() >= 0) {
            struct stat previous{};
            if (!fstat(card.get(), &previous) && previous.st_rdev == actual.st_rdev) continue;
        }
        if (++matches != 1) throw std::runtime_error("Ambiguous DRM connector across GPUs");
        drmVersionPtr version = drmGetVersion(candidate.get());
        borrowed = version && version->name_len == 6 && !memcmp(version->name, "vmwgfx", 6);
        if (version) drmFreeVersion(version);
        card = borrowed ? std::move(candidate) : std::move(observer);
        render = Fd(open(device->nodes[DRM_NODE_RENDER], O_RDWR | O_CLOEXEC | O_NOFOLLOW));
        if (render.get() < 0) throw std::runtime_error("Cannot open DRM render node");
    }
    if (matches != 1) throw std::runtime_error("No compositor DRM output matches the selected screen");
    export_loop(card.get(), render.get(), socket, connector, borrowed);
}
}
