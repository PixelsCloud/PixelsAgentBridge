#pragma once
#include "capture.h"
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/XShm.h>
#include <X11/extensions/Xrandr.h>
#include <X11/extensions/Xfixes.h>
#include <X11/extensions/Xdamage.h>
#include <sys/ipc.h>
#include <sys/shm.h>
#include <cstdlib>
#include <optional>
#include <array>
#include <utility>

namespace pab_linux::x11 {
// Never mistake the XWayland compatibility server for the whole Wayland
// desktop. The session supervisor supplies this environment, not the network.
inline bool selected() {
    const char* type = std::getenv("XDG_SESSION_TYPE");
    if (type && *type) return std::string(type) == "x11";
    const char* wayland = std::getenv("WAYLAND_DISPLAY");
    const char* display = std::getenv("DISPLAY");
    return (!wayland || !*wayland) && display && *display;
}
class Connection {
    ::Display* display_ = nullptr;
    XErrorHandler previous_ = nullptr;
    static inline int error_ = 0;
    static int record(::Display*, XErrorEvent* event) { error_ = event->error_code; return 0; }
public:
    Connection() {
        display_ = XOpenDisplay(nullptr);
        if (!display_) throw std::runtime_error("Cannot open the selected X11 session");
        previous_ = XSetErrorHandler(record);
        int opcode, event, error, major = 1, minor = 3;
        if (!XQueryExtension(display_, "RANDR", &opcode, &event, &error) ||
            !XRRQueryVersion(display_, &major, &minor) || major < 1 || (major == 1 && minor < 3)) {
            XCloseDisplay(display_); XSetErrorHandler(previous_);
            throw std::runtime_error("X11 RandR 1.3 is required");
        }
        try { check(); } catch (...) { XCloseDisplay(display_); XSetErrorHandler(previous_); throw; }
    }
    ~Connection() { XCloseDisplay(display_); XSetErrorHandler(previous_); }
    Connection(const Connection&) = delete;
    ::Display* get() const { return display_; }
    void check() {
        XSync(display_, False);
        const int error = std::exchange(error_, 0);
        if (error) throw std::runtime_error("X11 operation failed: " + std::to_string(error));
    }
};
struct Output { pab_linux::Display display; int screen; Window root; };
struct ResourceDrop { void operator()(XRRScreenResources* p) const { if (p) XRRFreeScreenResources(p); } };
struct OutputDrop { void operator()(XRROutputInfo* p) const { if (p) XRRFreeOutputInfo(p); } };
struct CrtcDrop { void operator()(XRRCrtcInfo* p) const { if (p) XRRFreeCrtcInfo(p); } };
inline std::vector<Output> outputs(Connection& connection) {
    auto* d = connection.get(); std::vector<Output> result;
    const int count = ScreenCount(d);
    if (count < 1 || count > 32) throw std::runtime_error("X11 screen count outside bounds");
    for (int screen = 0; screen < count; ++screen) {
        const auto root = RootWindow(d, screen);
        std::unique_ptr<XRRScreenResources, ResourceDrop> resources(XRRGetScreenResourcesCurrent(d, root));
        if (!resources || resources->noutput < 0 || resources->noutput > 32)
            throw std::runtime_error("X11 output inventory unavailable");
        const auto primary = XRRGetOutputPrimary(d, root);
        for (int i = 0; i < resources->noutput; ++i) {
            std::unique_ptr<XRROutputInfo, OutputDrop> info(XRRGetOutputInfo(d, resources.get(), resources->outputs[i]));
            if (!info) throw std::runtime_error("X11 output changed during enumeration");
            if (info->connection != RR_Connected || info->crtc == None) continue;
            std::unique_ptr<XRRCrtcInfo, CrtcDrop> crtc(XRRGetCrtcInfo(d, resources.get(), info->crtc));
            if (!crtc) throw std::runtime_error("X11 CRTC changed during enumeration");
            if (crtc->mode == None) continue;
            XRRCrtcTransformAttributes* raw_transform = nullptr;
            const bool has_transform = XRRGetCrtcTransform(d, info->crtc, &raw_transform);
            struct TransformDrop { void operator()(XRRCrtcTransformAttributes* p) const { if (p) XFree(p); } };
            std::unique_ptr<XRRCrtcTransformAttributes, TransformDrop> transform(raw_transform);
            if (has_transform && transform) {
                for (int row = 0; row < 3; ++row) for (int column = 0; column < 3; ++column)
                    if (transform->currentTransform.matrix[row][column] != (row == column ? 65536 : 0))
                        throw std::runtime_error("X11 transformed framebuffer requires a geometry adapter");
            }
            if (info->nameLen < 1 || info->nameLen > 120 || !info->name)
                throw std::runtime_error("Invalid X11 output name");
            std::string name(info->name, size_t(info->nameLen));
            json_string(name); // Validate UTF-8 before it becomes a stable ID.
            const auto size = geometry(crtc->width, crtc->height, 1, 0, 2);
            // RandR CRTC extents are already rotated physical root pixels.
            if (crtc->x < 0 || crtc->y < 0 || int64_t(crtc->x) + size.width > DisplayWidth(d, screen) ||
                int64_t(crtc->y) + size.height > DisplayHeight(d, screen))
                throw std::runtime_error("X11 output outside root framebuffer");
            result.push_back({{std::to_string(screen) + ":" + name, name, crtc->x, crtc->y,
                size, screen == DefaultScreen(d) && primary == resources->outputs[i],
                uint32_t(resources->configTimestamp), "x11"}, screen, root});
            if (result.size() > 32) throw std::runtime_error("X11 display count outside bounds");
        }
    }
    connection.check();
    if (result.empty()) throw std::runtime_error("No active X11 outputs");
    if (std::none_of(result.begin(), result.end(), [](const auto& o) { return o.display.primary; }))
        result.front().display.primary = true;
    return result;
}
inline bool same(const Output& a, const Output& b) {
    const auto& x = a.display; const auto& y = b.display;
    return a.root == b.root && a.screen == b.screen && x.id() == y.id() &&
        x.x == y.x && x.y == y.y && x.size.width == y.size.width &&
        x.size.height == y.size.height && x.layout_serial == y.layout_serial;
}
inline void validate(Connection& connection, const Output& output) {
    const auto current = outputs(connection);
    if (std::none_of(current.begin(), current.end(), [&](const auto& value) { return same(value, output); }))
        throw std::runtime_error("X11 layout changed; refresh capture generation");
}
inline size_t image_size(const XImage& image, unsigned width, unsigned height) {
    if (image.width != int(width) || image.height != int(height) || image.format != ZPixmap ||
        image.bits_per_pixel != 32 || image.byte_order != LSBFirst || image.red_mask != 0xff0000 ||
        image.green_mask != 0xff00 || image.blue_mask != 0xff || image.bytes_per_line < int64_t(width) * 4 ||
        uint64_t(image.bytes_per_line) * height > 256 * 1024 * 1024)
        throw std::runtime_error("Unsupported X11 image format or extent");
    return size_t(image.bytes_per_line) * height;
}
class Image {
    Connection& connection_;
    XImage* image_ = nullptr;
    XShmSegmentInfo shared_{};
    bool attached_ = false;
    bool marked_ = false;
public:
    Image(Connection& c, const Output& o) : connection_(c) {
        shared_.shmid = -1; shared_.shmaddr = reinterpret_cast<char*>(-1);
        try {
            if (!XShmQueryExtension(c.get())) throw std::runtime_error("X11 MIT-SHM is required for local capture");
            image_ = XShmCreateImage(c.get(), DefaultVisual(c.get(), o.screen), DefaultDepth(c.get(), o.screen),
                ZPixmap, nullptr, &shared_, o.display.size.width, o.display.size.height);
            if (!image_) throw std::runtime_error("Cannot allocate X11 shared image");
            const auto bytes = image_size(*image_, o.display.size.width, o.display.size.height);
            shared_.shmid = shmget(IPC_PRIVATE, bytes, IPC_CREAT | 0600);
            if (shared_.shmid < 0) throw std::runtime_error("Cannot allocate X11 shared memory");
            shared_.shmaddr = static_cast<char*>(shmat(shared_.shmid, nullptr, 0));
            if (shared_.shmaddr == reinterpret_cast<char*>(-1)) throw std::runtime_error("Cannot map X11 shared memory");
            image_->data = shared_.shmaddr; shared_.readOnly = False;
            if (!XShmAttach(c.get(), &shared_)) throw std::runtime_error("Cannot attach X11 shared memory");
            c.check(); attached_ = true;
            if (shmctl(shared_.shmid, IPC_RMID, nullptr)) throw std::runtime_error("Cannot retire X11 shared memory ID");
            marked_ = true;
        } catch (...) { cleanup(); throw; }
    }
    ~Image() { cleanup(); }
    void cleanup() noexcept {
        if (attached_) { XShmDetach(connection_.get(), &shared_); XSync(connection_.get(), False); attached_ = false; }
        if (image_) { image_->data = nullptr; XDestroyImage(image_); image_ = nullptr; }
        if (shared_.shmaddr != reinterpret_cast<char*>(-1)) { shmdt(shared_.shmaddr); shared_.shmaddr = reinterpret_cast<char*>(-1); }
        if (shared_.shmid >= 0 && !marked_) shmctl(shared_.shmid, IPC_RMID, nullptr);
        shared_.shmid = -1;
    }
    XImage& read(const Output& output) {
        if (!XShmGetImage(connection_.get(), output.root, image_, output.display.x, output.display.y, AllPlanes))
            throw std::runtime_error("X11 shared capture failed");
        connection_.check(); return *image_;
    }
};
struct CursorDrop { void operator()(XFixesCursorImage* p) const { if (p) XFree(p); } };
using Cursor = std::unique_ptr<XFixesCursorImage, CursorDrop>;
inline void blend_cursor(XImage& image, const Output& output, const XFixesCursorImage& cursor) {
    if (cursor.width > 512 || cursor.height > 512 || !cursor.pixels)
        throw std::runtime_error("X11 cursor outside bounds");
    const int64_t left = int64_t(cursor.x) - cursor.xhot - output.display.x;
    const int64_t top = int64_t(cursor.y) - cursor.yhot - output.display.y;
    for (unsigned y = 0; y < cursor.height; ++y) {
        if (top + y < 0 || top + y >= image.height) continue;
        for (unsigned x = 0; x < cursor.width; ++x) {
            if (left + x < 0 || left + x >= image.width) continue;
            const uint32_t pixel = cursor.pixels[size_t(y) * cursor.width + x];
            const unsigned alpha = pixel >> 24;
            auto* target = reinterpret_cast<uint8_t*>(image.data) + size_t(top+y)*image.bytes_per_line + size_t(left+x)*4;
            for (unsigned channel = 0; channel < 3; ++channel)
                target[channel] = std::min(255u, ((pixel >> (channel*8)) & 255) + (target[channel]*(255-alpha)+127)/255);
        }
    }
}
inline void capture(Connection& connection, const Output& output, unsigned fps) {
    const auto& display = output.display;
    if (display.size.width % 2 || display.size.height % 2) throw std::runtime_error("Software capture requires even dimensions");
    auto* d = connection.get(); int event, error;
    if (!XDamageQueryExtension(d, &event, &error) || !XFixesQueryExtension(d, &event, &error))
        throw std::runtime_error("X11 Damage and XFixes are required");
    const Damage damage = XDamageCreate(d, output.root, XDamageReportNonEmpty);
    struct DamageGuard { ::Display* d; Damage value; ~DamageGuard() { XDamageDestroy(d, value); } } guard{d, damage};
    connection.check();
    Image image(connection, output);
    Encoder encoder(pab_soft_encoder_new(display.size.width, display.size.height, fps));
    if (!encoder) throw std::runtime_error("Cannot initialize X11 software encoder");
    fprintf(stderr, "media source=x11_shm encoder=openh264_software pixel_format=bgrx cpu_conversion=true\n");
    write_value(magic); write_value(display.size.width); write_value(display.size.height);
    auto start = Clock::now(), next = start, inspected = start;
    uint64_t index = 0; bool changed = true;
    std::optional<std::array<unsigned long, 3>> previous_cursor;
    for (;;) {
        const auto command = read_value<uint8_t>();
        if (!command) return;
        if (command != 1) throw std::runtime_error("Invalid capture command");
        for (;;) {
            parent_alive();
            if (Clock::now() - inspected >= std::chrono::milliseconds(100)) {
                validate(connection, output); inspected = Clock::now();
            }
            while (XPending(d)) { XEvent e; XNextEvent(d, &e); changed = true; }
            if (Clock::now() < next) { std::this_thread::sleep_for(std::chrono::milliseconds(2)); continue; }
            Cursor cursor(XFixesGetCursorImage(d));
            connection.check();
            if (!cursor) throw std::runtime_error("Cannot observe X11 cursor");
            Window root, child; int rx, ry, wx, wy; unsigned mask;
            const bool on_root = XQueryPointer(d, output.root, &root, &child, &rx, &ry, &wx, &wy, &mask);
            const std::array<unsigned long, 3> stamp{on_root ? cursor->cursor_serial : 0,
                on_root ? static_cast<unsigned long>(cursor->x) : 0, on_root ? static_cast<unsigned long>(cursor->y) : 0};
            if (!changed && previous_cursor == stamp) {
                std::this_thread::sleep_for(std::chrono::milliseconds(16)); continue;
            }
            // Clear damage before capture so changes arriving during capture
            // remain pending for the next frame. No raw-frame copy is retained.
            XDamageSubtract(d, damage, None, None); changed = false;
            auto& pixels = image.read(output);
            const auto sampled = Clock::now();
            if (on_root) blend_cursor(pixels, output, *cursor);
            previous_cursor = stamp;
            PabCodecBytes packet{};
            if (!pab_soft_encode(encoder.get(), reinterpret_cast<uint8_t*>(pixels.data),
                image_size(pixels, display.size.width, display.size.height), pixels.bytes_per_line, &packet) ||
                !packet.size || packet.size > max_packet) throw std::runtime_error("X11 software encoding failed");
            const bool key = keyframe(packet);
            if (!index && !key) throw std::runtime_error("First X11 frame is not IDR");
            write_value(uint32_t(packet.size)); write_value(index++);
            write_value(uint64_t(std::chrono::duration_cast<std::chrono::microseconds>(sampled-start).count()));
            write_value(uint32_t(key)); write_bytes(packet.data, packet.size);
            next = std::max(next + std::chrono::microseconds(1000000/fps), Clock::now()); break;
        }
    }
}
inline void run(int argc, const char* const* argv) {
    Connection connection; const auto found = outputs(connection);
    if (argc == 2) {
        std::vector<pab_linux::Display> displays;
        for (const auto& output : found) displays.push_back(output.display);
        inventory(displays); return;
    }
    const auto selected = std::find_if(found.begin(), found.end(), [&](const auto& o) { return o.display.id() == argv[2]; });
    if (selected == found.end()) throw std::runtime_error("Selected X11 display is unavailable");
    if (argc == 3) throw std::runtime_error("X11 native input backend is not available yet");
    capture(connection, *selected, number(argv[3], 60));
}
}
