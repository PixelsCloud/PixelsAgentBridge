#pragma once
#include "x11_capture.h"
#include "x11_cursor_visibility.h"

namespace pab_linux::x11 {
// XFixes pixels are native unsigned longs containing premultiplied ARGB32,
// not a tightly packed byte buffer on LP64. Only shape changes convert them.
inline std::shared_ptr<const CursorImage> cursor_image(const XFixesCursorImage& cursor) {
    if (!cursor.width || !cursor.height || cursor.width > 512 || cursor.height > 512 ||
        cursor.xhot >= cursor.width || cursor.yhot >= cursor.height || !cursor.pixels)
        throw std::runtime_error("Invalid X11 cursor shape");
    auto image = std::make_shared<CursorImage>();
    image->width = cursor.width; image->height = cursor.height;
    image->hotspot_x = cursor.xhot; image->hotspot_y = cursor.yhot;
    image->pixels.resize(size_t(cursor.width) * cursor.height * 4);
    for (size_t i = 0; i < image->pixels.size()/4; ++i) {
        const uint32_t pixel = uint32_t(cursor.pixels[i]);
        const auto alpha = pixel >> 24;
        for (unsigned channel = 0; channel < 4; ++channel) {
            const auto value = (pixel >> (channel*8)) & 255;
            if (channel < 3 && value > alpha) throw std::runtime_error("X11 cursor is not premultiplied");
            image->pixels[i*4+channel] = uint8_t(value);
        }
    }
    return image;
}

// This worker owns its own process and X connection. Xlib's global error
// handler is never shared concurrently with the capture/input connections.
class CursorSource {
    Connection& connection_;
    Output output_;
    CursorVisibility visibility_;
    int event_ = 0;
    bool refresh_ = true, opaque_ = false;
    Clock::time_point next_layout_{};
public:
    CursorState state;
    CursorSource(Connection& connection, Output output):connection_(connection),output_(std::move(output)),visibility_(connection) {
        int error;
        if (!XFixesQueryExtension(connection_.get(), &event_, &error))
            throw std::runtime_error("X11 XFixes is required for cursor capture");
        XFixesSelectCursorInput(connection_.get(), output_.root, XFixesDisplayCursorNotifyMask);
        connection_.check();
    }
    void sample() {
        auto* d = connection_.get();
        if (Clock::now() >= next_layout_) {
            validate(connection_, output_);
            next_layout_ = Clock::now() + std::chrono::milliseconds(250);
        }
        while (XPending(d)) {
            XEvent event; XNextEvent(d, &event);
            if (event.type == event_ + XFixesCursorNotify) refresh_ = true;
        }
        Window root, child; int rx, ry, wx, wy; unsigned mask;
        const bool on_root = XQueryPointer(d, output_.root, &root, &child, &rx, &ry, &wx, &wy, &mask);
        connection_.check();
        if (!on_root) {
            state.update(false, 0, 0); refresh_ = true; return;
        }
        std::shared_ptr<const CursorImage> image;
        if (refresh_) {
            Cursor cursor(XFixesGetCursorImage(d)); connection_.check();
            if (!cursor) throw std::runtime_error("Cannot observe X11 cursor");
            image = cursor_image(*cursor);
            opaque_ = false;
            for (size_t i = 3; i < image->pixels.size(); i += 4) opaque_ |= image->pixels[i] != 0;
            refresh_ = false;
        }
        const auto& display = output_.display;
        const bool inside = rx >= display.x && ry >= display.y &&
            int64_t(rx) < int64_t(display.x) + display.size.width &&
            int64_t(ry) < int64_t(display.y) + display.size.height;
        const bool system_hidden = visibility_.hidden();
        state.update(inside && opaque_ && !system_hidden, rx, ry, std::move(image));
    }
};

inline void cursor_worker(Connection& connection, const Output& output) {
    const auto x = read_value<int32_t>(), y = read_value<int32_t>();
    const auto width = read_value<uint32_t>(), height = read_value<uint32_t>(), rotation = read_value<uint32_t>();
    const auto& display = output.display;
    if (x != display.x || y != display.y || width != display.size.logical_width ||
        height != display.size.logical_height || rotation)
        throw std::runtime_error("X11 cursor layout changed; refresh generation");
    CursorSource source(connection, output);
    write_value(magic); write_value(uint32_t(1));
    bool requested = false;
    uint64_t sequence = 0, revision = 0, sent_image = 0;
    for (;;) {
        parent_alive();
        pollfd pipe{STDIN_FILENO, POLLIN, 0};
        if (poll(&pipe, 1, 0) > 0 && (pipe.revents & POLLIN)) {
            const auto command = read_value<uint8_t>();
            if (!command) return;
            if (command != 1 || requested) throw std::runtime_error("Invalid X11 cursor request");
            requested = true;
        }
        source.sample(); const auto snapshot = source.state.snapshot();
        if (requested && revision != snapshot.revision) {
            if (sequence == UINT64_MAX) throw std::runtime_error("Cursor sequence exhausted");
            const bool changed = snapshot.image_id != sent_image;
            write_value(uint8_t(1)); write_value(uint16_t(1)); write_value(++sequence);
            write_value(uint8_t(snapshot.visible)); write_value(snapshot.x); write_value(snapshot.y);
            write_value(snapshot.image_id); write_value(uint8_t(changed ? 1 : 0));
            if (changed) {
                const auto& image = *snapshot.image;
                write_value(image.width); write_value(image.height);
                write_value(image.hotspot_x); write_value(image.hotspot_y);
                write_bytes(image.pixels.data(), image.pixels.size());
            }
            revision = snapshot.revision; sent_image = snapshot.image_id; requested = false;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(8));
    }
}
}
