#pragma once
// Private, same-host descriptor channel. The supervisor owns authentication and
// session lifetime; neither a DRM control fd nor pixel bytes cross this channel.
#include "common.h"
#include <array>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <drm_fourcc.h>

namespace pab_linux::dma {
class Fd {
    int value_ = -1;
public:
    explicit Fd(int value = -1) : value_(value) {}
    ~Fd() { if (value_ >= 0) close(value_); }
    Fd(const Fd&) = delete;
    Fd& operator=(const Fd&) = delete;
    Fd(Fd&& other) noexcept : value_(other.release()) {}
    Fd& operator=(Fd&& other) noexcept {
        if (this != &other) { if (value_ >= 0) close(value_); value_ = other.release(); }
        return *this;
    }
    int get() const { return value_; }
    int release() { int value = value_; value_ = -1; return value; }
};
constexpr uint32_t wire_magic = 0x32414d44, wire_version = 1;
struct Metadata {
    uint32_t magic = wire_magic, version = wire_version;
    uint64_t sequence = 0;
    uint32_t width = 0, height = 0, format = 0, stride = 0, offset = 0, flags = 0;
    uint64_t modifier = 0;
    uint32_t crtc = 0, framebuffer = 0, connector = 0, reserved = 0;
};
static_assert(sizeof(Metadata) == 64);
struct Request { uint32_t magic = wire_magic, version = wire_version; uint64_t sequence = 0; };
static_assert(sizeof(Request) == 16);
struct Frame { Metadata info; Fd image, render; };

inline uint64_t validate(const Metadata& m, uint64_t sequence, unsigned minimum_size = 2) {
    if (m.magic != wire_magic || m.version != wire_version || m.sequence != sequence || m.reserved ||
        !m.crtc || !m.framebuffer || !m.connector || m.flags & ~2u ||
        m.width < minimum_size || m.height < minimum_size || m.width > 8192 || m.height > 8192 ||
        uint64_t(m.width) * m.height > 33554432 || m.stride < uint64_t(m.width) * 4 ||
        m.stride > INT32_MAX || m.offset > INT32_MAX)
        throw std::runtime_error("Invalid DMA frame metadata");
    switch (m.format) {
    case DRM_FORMAT_XRGB8888: case DRM_FORMAT_ARGB8888:
    case DRM_FORMAT_XBGR8888: case DRM_FORMAT_ABGR8888:
    case DRM_FORMAT_XRGB2101010: case DRM_FORMAT_ARGB2101010: break;
    default: throw std::runtime_error("Unsupported DMA frame pixel format");
    }
    const uint64_t bytes = uint64_t(m.offset) + uint64_t(m.stride) * m.height;
    if (bytes > 256 * 1024 * 1024) throw std::runtime_error("DMA frame extent exceeds bound");
    return bytes;
}
inline uint64_t validate_cursor_image(const Metadata& m, uint64_t sequence) {
    const auto extent = validate(m, sequence, 1);
    if (m.width > 512 || m.height > 512 || extent > 4 * 1024 * 1024 ||
        (m.format != DRM_FORMAT_ARGB8888 && m.format != DRM_FORMAT_ABGR8888))
        throw std::runtime_error("Unsupported DMA cursor image");
    return extent;
}
inline void channel(int fd) {
    int type = 0; socklen_t length = sizeof(type);
    if (fd < 3 || getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &length) || type != SOCK_SEQPACKET)
        throw std::runtime_error("DMA transport requires an inherited private packet socket");
}
inline void send_request(int socket, uint64_t sequence) {
    Request request; request.sequence = sequence;
    if (send(socket, &request, sizeof(request), MSG_NOSIGNAL) != sizeof(request))
        throw std::runtime_error("DMA request channel closed");
}
inline bool request(int socket, uint64_t sequence) {
    Request r{};
    const auto count = recv(socket, &r, sizeof(r), MSG_TRUNC);
    if (!count) return false;
    if (count != sizeof(r) || r.magic != wire_magic || r.version != wire_version || r.sequence != sequence)
        throw std::runtime_error("Invalid DMA request");
    return true;
}
inline void send_frame(int socket, const Frame& frame) {
    validate(frame.info, frame.info.sequence);
    std::array<int, 2> fds{frame.image.get(), frame.render.get()};
    if (fds[0] < 0 || fds[1] < 0) throw std::runtime_error("Missing DMA descriptors");
    alignas(cmsghdr) std::array<unsigned char, CMSG_SPACE(sizeof(fds))> control{};
    iovec data{const_cast<Metadata*>(&frame.info), sizeof(frame.info)};
    msghdr message{}; message.msg_iov = &data; message.msg_iovlen = 1;
    message.msg_control = control.data(); message.msg_controllen = control.size();
    auto* rights = CMSG_FIRSTHDR(&message);
    rights->cmsg_level = SOL_SOCKET; rights->cmsg_type = SCM_RIGHTS;
    rights->cmsg_len = CMSG_LEN(sizeof(fds));
    memcpy(CMSG_DATA(rights), fds.data(), sizeof(fds));
    if (sendmsg(socket, &message, MSG_NOSIGNAL) != sizeof(frame.info))
        throw std::runtime_error("DMA descriptor channel closed");
}
inline Frame receive(int socket, uint64_t sequence) {
    Metadata info{};
    // Receive more than the allowed count so malformed messages can be tested;
    // every descriptor delivered by the kernel is owned before validation.
    alignas(cmsghdr) std::array<unsigned char, CMSG_SPACE(4 * sizeof(int))> control{};
    iovec data{&info, sizeof(info)}; msghdr message{};
    message.msg_iov = &data; message.msg_iovlen = 1;
    message.msg_control = control.data(); message.msg_controllen = control.size();
    const auto count = recvmsg(socket, &message, MSG_CMSG_CLOEXEC | MSG_TRUNC);
    std::vector<Fd> fds;
    bool other = false;
    for (auto* c = CMSG_FIRSTHDR(&message); count >= 0 && c; c = CMSG_NXTHDR(&message, c)) {
        if (c->cmsg_level != SOL_SOCKET || c->cmsg_type != SCM_RIGHTS ||
            c->cmsg_len < CMSG_LEN(0) || (c->cmsg_len - CMSG_LEN(0)) % sizeof(int)) { other = true; continue; }
        for (size_t i = 0; i < (c->cmsg_len - CMSG_LEN(0)) / sizeof(int); ++i) {
            int fd; memcpy(&fd, CMSG_DATA(c) + i * sizeof(int), sizeof(fd)); fds.emplace_back(fd);
        }
    }
    if (count != sizeof(info) || message.msg_flags & (MSG_TRUNC | MSG_CTRUNC) || other || fds.size() != 2)
        throw std::runtime_error("Invalid DMA descriptor packet");
    const auto extent = validate(info, sequence);
    struct stat image{}, render{};
    if (fstat(fds[0].get(), &image) || fstat(fds[1].get(), &render) || !S_ISCHR(render.st_mode) ||
        image.st_size < 0 || (image.st_size && uint64_t(image.st_size) < extent))
        throw std::runtime_error("DMA descriptors do not match metadata");
    return {info, std::move(fds[0]), std::move(fds[1])};
}
}
