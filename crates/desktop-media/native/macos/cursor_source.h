#pragma once
#include "common.h"
#include <dlfcn.h>

namespace pab_mac {
struct CursorShape {
    uint32_t width = 0, height = 0, hotspot_x = 0, hotspot_y = 0;
    std::vector<uint8_t> pixels;
    bool operator==(const CursorShape& other) const {
        return width == other.width && height == other.height && hotspot_x == other.hotspot_x &&
            hotspot_y == other.hotspot_y && pixels == other.pixels;
    }
};
// NSImage representations can be much larger than the on-screen cursor (the
// system arrow currently includes a 10x representation). Rasterize its logical
// size at the selected capture scale, not the first TIFF representation size.
inline CursorShape cursor_shape(NSImage* image, NSPoint hotspot, double scale_x, double scale_y) {
    const NSSize size = image.size;
    const double width = std::round(size.width*scale_x), height = std::round(size.height*scale_y);
    const double hx = std::round(hotspot.x*scale_x), hy = std::round(hotspot.y*scale_y);
    if (!image || !std::isfinite(width) || !std::isfinite(height) || width < 1 || height < 1 ||
        width > 512 || height > 512 || !std::isfinite(hx) || !std::isfinite(hy) ||
        hx < 0 || hy < 0 || hx >= width || hy >= height || scale_x <= 0 || scale_y <= 0)
        throw std::runtime_error("Cursor image geometry outside bounds");
    CursorShape result;
    result.width = uint32_t(width); result.height = uint32_t(height);
    result.hotspot_x = uint32_t(hx); result.hotspot_y = uint32_t(hy);
    result.pixels.resize(size_t(result.width)*result.height*4);
    OwnedCF<CGColorSpaceRef> space(CGColorSpaceCreateWithName(kCGColorSpaceSRGB));
    OwnedCF<CGContextRef> context(CGBitmapContextCreate(result.pixels.data(), result.width,
        result.height, 8, result.width*4, space.get(), kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little));
    if (!context.get()) throw std::runtime_error("Cursor bitmap unavailable");
    // Top-down BGRA, premultiplied by Core Graphics. Only the bounded cursor
    // image is mapped; the video IOSurface never enters this process.
    CGContextTranslateCTM(context.get(), 0, height);
    CGContextScaleCTM(context.get(), 1, -1);
    NSGraphicsContext* graphics = [NSGraphicsContext graphicsContextWithCGContext:context.get() flipped:YES];
    [NSGraphicsContext saveGraphicsState];
    @try {
        NSGraphicsContext.currentContext = graphics;
        [image drawInRect:NSMakeRect(0,0,width,height) fromRect:NSZeroRect operation:NSCompositingOperationCopy
            fraction:1 respectFlipped:YES hints:@{NSImageHintInterpolation: @(NSImageInterpolationHigh)}];
    } @finally { [NSGraphicsContext restoreGraphicsState]; }
    return result;
}

class SystemCursor {
    // Apple deprecated visibility without a replacement. Resolve it at runtime
    // so an OS removing it fails this optional source instead of app startup.
    using Visible = boolean_t(*)();
    using Seed = int(*)();
    Visible visible_ = reinterpret_cast<Visible>(dlsym(RTLD_DEFAULT, "CGCursorIsVisible"));
    // RustDesk uses this seed to avoid repeatedly converting an unchanged
    // shape. It is only an optimization here: without it we compare pixels.
    Seed seed_ = reinterpret_cast<Seed>(dlsym(RTLD_DEFAULT, "CGSCurrentCursorSeed"));
    int last_seed_ = 0;
    bool initialized_ = false;
    NSSize last_size_{};
    NSPoint last_hotspot_{};
public:
    CursorShape shape;
    explicit SystemCursor() {
        if (!visible_) throw std::runtime_error("System cursor visibility is unavailable");
    }
    bool visible() const { return visible_() != 0; }
    bool refresh(double scale_x, double scale_y) {
        const int seed = seed_ ? seed_() : 0;
        NSCursor* cursor = NSCursor.currentSystemCursor;
        if (!cursor || !cursor.image) throw std::runtime_error("System cursor image is unavailable");
        const NSSize size = cursor.image.size;
        const NSPoint hotspot = cursor.hotSpot;
        if (initialized_ && seed_ && seed == last_seed_ && NSEqualSizes(size,last_size_) &&
            NSEqualPoints(hotspot,last_hotspot_)) return false;
        auto next = cursor_shape(cursor.image, hotspot, scale_x, scale_y);
        // NSCursor supplies one image/hotspot snapshot. Do not discard a valid
        // sample when another app repeatedly sets its cursor: its seed can
        // advance throughout rasterization and otherwise starve this source.
        // A racing seed only disables the cache for the next sample.
        const bool stable = !seed_ || seed == seed_();
        const bool changed = !(shape == next);
        shape = std::move(next); initialized_ = stable; last_seed_ = seed;
        last_size_ = size; last_hotspot_ = hotspot;
        return changed;
    }
};

inline void cursor_worker(const Display& display) {
    const auto x0 = read_value<int32_t>(), y0 = read_value<int32_t>();
    const auto width = read_value<uint32_t>(), height = read_value<uint32_t>();
    const auto rotation = read_value<uint32_t>();
    if (x0 != std::lround(display.bounds.origin.x) || y0 != std::lround(display.bounds.origin.y) ||
        width != std::lround(display.bounds.size.width) || height != std::lround(display.bounds.size.height) || rotation)
        throw std::runtime_error("Cursor layout changed; refresh generation");
    SystemCursor cursor;
    write_value(magic); write_value(uint32_t(1));
    uint64_t sequence = 0, image_id = 0, sent_image = 0;
    bool requested = false, dirty = true, visible = false;
    int32_t x = 0, y = 0;
    auto next_layout = Clock::now();
    for (;;) { @autoreleasepool {
        parent_alive();
        pollfd pipe{STDIN_FILENO, POLLIN, 0};
        if (poll(&pipe,1,0) > 0 && (pipe.revents & POLLIN)) {
            const auto operation = read_value<uint8_t>();
            if (!operation) return;
            if (operation != 1 || requested) throw std::runtime_error("Invalid cursor request");
            requested = true;
        }
        if (Clock::now() >= next_layout) {
            const auto found = displays();
            const auto same = std::find_if(found.begin(),found.end(),[&](const Display& value) {
                return value.id == display.id && CGRectEqualToRect(value.bounds,display.bounds) &&
                    value.width == display.width && value.height == display.height;
            });
            if (same == found.end()) throw std::runtime_error("Cursor display changed; refresh generation");
            next_layout = Clock::now() + std::chrono::milliseconds(250);
        }
        OwnedCF<CGEventRef> event(CGEventCreate(nullptr));
        if (!event.get()) throw std::runtime_error("System cursor position is unavailable");
        const CGPoint position = CGEventGetLocation(event.get());
        if (!std::isfinite(position.x) || !std::isfinite(position.y) ||
            position.x < INT32_MIN || position.y < INT32_MIN || position.x > INT32_MAX || position.y > INT32_MAX)
            throw std::runtime_error("Cursor position outside bounds");
        const bool on_display = CGRectContainsPoint(display.bounds,position);
        bool next_visible = cursor.visible() && on_display;
        if (next_visible && cursor.refresh(double(display.width)/width,double(display.height)/height)) {
            if (image_id == UINT64_MAX) throw std::runtime_error("Cursor image sequence exhausted");
            ++image_id; dirty = true;
        }
        // Hiding needs no image to clear the viewer; an initial hidden sample
        // must never reference an image that has not been supplied.
        next_visible = next_visible && image_id != 0;
        const int32_t nx = int32_t(std::lround(position.x)), ny = int32_t(std::lround(position.y));
        dirty = dirty || next_visible != visible || (next_visible && (x != nx || y != ny));
        visible = next_visible; x = nx; y = ny;
        if (requested && dirty) {
            if (sequence == UINT64_MAX) throw std::runtime_error("Cursor sequence exhausted");
            const bool changed = image_id != sent_image;
            write_value(uint8_t(1)); write_value(uint16_t(1)); write_value(++sequence);
            write_value(uint8_t(visible)); write_value(x); write_value(y); write_value(image_id);
            write_value(uint8_t(changed ? 1 : 0));
            if (changed) {
                const auto& image = cursor.shape;
                write_value(image.width); write_value(image.height); write_value(image.hotspot_x); write_value(image.hotspot_y);
                write_bytes(image.pixels.data(),image.pixels.size());
            }
            sent_image = image_id; dirty = requested = false;
        }
        // Sampling continues while video has no credit; retain only the latest
        // shape and position, never a historical queue of mouse movement.
        const auto next = Clock::now() + std::chrono::milliseconds(8);
        pump(); std::this_thread::sleep_until(next);
    } }
}
}
