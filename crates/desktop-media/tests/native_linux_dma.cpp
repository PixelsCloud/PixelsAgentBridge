// Native descriptor boundary tests: real SCM_RIGHTS, including rejection paths
// that must close every received fd. No display, GPU, root privilege or input.
#include "../native/linux/dma_frame.h"
#include <cassert>
#include <dirent.h>
#include <sys/mman.h>

using namespace pab_linux::dma;
template<class F> void rejects(F action) {
    bool caught = false;
    try { action(); } catch (const std::runtime_error&) { caught = true; }
    assert(caught);
}
static unsigned count_fds() {
    auto* dir = opendir("/proc/self/fd"); assert(dir);
    unsigned n = 0; while (readdir(dir)) ++n;
    closedir(dir); return n;
}
static void malformed(int socket, const Metadata& info, size_t bytes, int image, int render, unsigned descriptors) {
    assert(descriptors <= 6 && bytes <= sizeof(info) + 4);
    std::array<int, 6> fds{}; fds.fill(image); fds[1] = render;
    alignas(cmsghdr) std::array<unsigned char, CMSG_SPACE(sizeof(fds))> control{};
    std::array<unsigned char, sizeof(info) + 4> payload{}; memcpy(payload.data(), &info, sizeof(info));
    iovec data{payload.data(), bytes}; msghdr message{}; message.msg_iov = &data; message.msg_iovlen = 1;
    if (descriptors) {
        message.msg_control = control.data(); message.msg_controllen = CMSG_SPACE(descriptors * sizeof(int));
        auto* rights = CMSG_FIRSTHDR(&message); rights->cmsg_level = SOL_SOCKET; rights->cmsg_type = SCM_RIGHTS;
        rights->cmsg_len = CMSG_LEN(descriptors * sizeof(int)); memcpy(CMSG_DATA(rights), fds.data(), descriptors * sizeof(int));
    }
    assert(sendmsg(socket, &message, MSG_NOSIGNAL) == ssize_t(bytes));
}
int main() {
    Metadata m; m.width = 64; m.height = 48; m.format = DRM_FORMAT_XRGB8888; m.stride = 256;
    m.crtc = 1; m.framebuffer = 2; m.connector = 3;
    assert(validate(m, 0) == 12288);
    unsigned checked = 1;
    for (auto mutate : std::initializer_list<void(*)(Metadata&)>{
        [](Metadata& a) { a.magic = 0; }, [](Metadata& a) { a.version = 2; },
        [](Metadata& a) { a.sequence = 1; }, [](Metadata& a) { a.reserved = 1; },
        [](Metadata& a) { a.width = 8193; }, [](Metadata& a) { a.height = 0; },
        [](Metadata& a) { a.stride = 255; }, [](Metadata& a) { a.offset = UINT32_MAX; },
        [](Metadata& a) { a.stride = INT32_MAX; }, [](Metadata& a) { a.flags = 1; },
        [](Metadata& a) { a.format = DRM_FORMAT_RGB565; }, [](Metadata& a) { a.connector = 0; }}) {
        auto invalid = m; mutate(invalid); rejects([&] { validate(invalid, 0); }); ++checked;
    }
    int pair[2]; assert(socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0, pair) == 0);
    Fd sender(pair[0]), receiver(pair[1]); channel(sender.get()); channel(receiver.get());
    Fd image(memfd_create("pab-dma-test", MFD_CLOEXEC)), render(open("/dev/null", O_RDWR | O_CLOEXEC));
    assert(image.get() >= 0 && render.get() >= 0); assert(ftruncate(image.get(), 12288) == 0);
    send_request(receiver.get(), 0); assert(request(sender.get(), 0)); ++checked;
    send_request(receiver.get(), 1); rejects([&] { request(sender.get(), 2); }); ++checked;
    const auto before = count_fds();
    {
        Frame frame{m, Fd(fcntl(image.get(), F_DUPFD_CLOEXEC, 3)), Fd(fcntl(render.get(), F_DUPFD_CLOEXEC, 3))};
        send_frame(sender.get(), frame);
        auto read = receive(receiver.get(), 0);
        assert(read.info.width == 64 && read.info.height == 48);
        assert(fcntl(read.image.get(), F_GETFD) & FD_CLOEXEC);
        assert(fcntl(read.render.get(), F_GETFD) & FD_CLOEXEC);
        struct stat a{}, b{}; assert(fstat(image.get(), &a) == 0 && fstat(read.image.get(), &b) == 0 && a.st_ino == b.st_ino);
    }
    assert(count_fds() == before); ++checked;
    for (unsigned descriptors : {0u, 1u, 3u, 6u}) {
        malformed(sender.get(), m, sizeof(m), image.get(), render.get(), descriptors);
        rejects([&] { receive(receiver.get(), 0); }); assert(count_fds() == before); ++checked;
    }
    for (size_t length : {sizeof(m) - 1, sizeof(m) + 4}) {
        malformed(sender.get(), m, length, image.get(), render.get(), 2);
        rejects([&] { receive(receiver.get(), 0); }); assert(count_fds() == before); ++checked;
    }
    malformed(sender.get(), m, sizeof(m), image.get(), render.get(), 2);
    rejects([&] { receive(receiver.get(), 9); }); assert(count_fds() == before); ++checked;
    auto invalid = m; invalid.width = UINT32_MAX;
    malformed(sender.get(), invalid, sizeof(m), image.get(), render.get(), 2);
    rejects([&] { receive(receiver.get(), 0); }); assert(count_fds() == before); ++checked;
    assert(ftruncate(image.get(), 12287) == 0);
    malformed(sender.get(), m, sizeof(m), image.get(), render.get(), 2);
    rejects([&] { receive(receiver.get(), 0); }); assert(count_fds() == before); ++checked;
    assert(ftruncate(image.get(), 12288) == 0);
    malformed(sender.get(), m, sizeof(m), image.get(), image.get(), 2);
    rejects([&] { receive(receiver.get(), 0); }); assert(count_fds() == before); ++checked;
    int stream[2]; assert(socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, stream) == 0);
    Fd stream_a(stream[0]), stream_b(stream[1]); rejects([&] { channel(stream_a.get()); }); ++checked;
    receiver = Fd(); rejects([&] { send_request(sender.get(), 0); }); ++checked;
    printf("Linux DMA descriptor boundaries: %u passed; rejection paths leak no fds\n", checked);
}
