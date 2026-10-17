#pragma once
#include "common.h"
#import <Metal/Metal.h>

namespace pab_mac {
// Small immutable shape texture; motion never uploads pixels. Video and cursor
// are composed in the same fragment invocation, with no desktop readback.
class CursorOverlay {
    bool visible_ = false, tracking_ = false;
    int64_t x_ = 0, y_ = 0;
    uint32_t desktop_width_ = 0, desktop_height_ = 0;
    uint32_t width_ = 0, height_ = 0, hotspot_x_ = 0, hotspot_y_ = 0, format_ = 0;
    id<MTLTexture> __strong shape_ = nil, empty_ = nil;
    static id<MTLTexture> texture(id<MTLDevice> device, unsigned width, unsigned height, const uint8_t* bytes) {
        auto descriptor = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
            width:width height:height mipmapped:NO];
        descriptor.usage = MTLTextureUsageShaderRead;
        // Managed textures are needed on discrete Intel Macs; the default
        // descriptor storage mode is CPU-writable on both Mac architectures.
        id<MTLTexture> result = [device newTextureWithDescriptor:descriptor];
        if (!result) throw std::runtime_error("Metal cursor texture unavailable");
        [result replaceRegion:MTLRegionMake2D(0,0,width,height) mipmapLevel:0 withBytes:bytes bytesPerRow:width*4];
        return result;
    }
public:
    struct Parameters {
        float x, y, inverse_x, inverse_y;
        uint32_t width, height, format, visible;
    };
    static_assert(sizeof(Parameters) == 32);
    bool tracking() const { return tracking_; }
    void update(id<MTLDevice> device, const uint8_t* bytes, size_t size) {
        size_t offset = 0;
        const auto read = [&](auto& value) {
            if (sizeof(value) > size-offset) throw std::runtime_error("Truncated cursor command");
            std::memcpy(&value,bytes+offset,sizeof(value)); offset += sizeof(value);
        };
        uint8_t action = 0; read(action);
        if (!action) {
            if (offset != size) throw std::runtime_error("Invalid cursor clear");
            visible_ = tracking_ = false; return;
        }
        if (action != 1) throw std::runtime_error("Invalid cursor operation");
        uint8_t visible = 0, format = 0; int64_t x = 0, y = 0;
        uint32_t desktop_width = 0, desktop_height = 0;
        read(visible); read(x); read(y); read(desktop_width); read(desktop_height); read(format);
        if (visible > 1 || !desktop_width || !desktop_height || desktop_width > 8192 || desktop_height > 8192 ||
            x < -INT64_C(4294967295) || x > INT64_C(4294967295) || y < -INT64_C(4294967295) || y > INT64_C(4294967295))
            throw std::runtime_error("Invalid cursor coordinates");
        if (format) {
            uint32_t width, height, hotspot_x, hotspot_y;
            read(width); read(height); read(hotspot_x); read(hotspot_y);
            if (format > 2 || !width || !height || width > 512 || height > 512 || hotspot_x >= width || hotspot_y >= height ||
                size-offset != size_t(width)*height*(format == 1 ? 4 : 8))
                throw std::runtime_error("Invalid cursor image command");
            auto shape = texture(device,width*(format == 1 ? 1 : 2),height,bytes+offset);
            shape_ = shape; format_ = format; width_ = width; height_ = height;
            hotspot_x_ = hotspot_x; hotspot_y_ = hotspot_y;
        } else if (offset != size) throw std::runtime_error("Unexpected cursor image bytes");
        if (visible && !shape_) throw std::runtime_error("Visible cursor missing image");
        x_ = x; y_ = y; desktop_width_ = desktop_width; desktop_height_ = desktop_height;
        visible_ = visible != 0; tracking_ = true;
    }
    Parameters parameters(const MTLViewport& viewport, double display_width, double display_height) const {
        Parameters p{};
        if (!visible_ || !shape_) return p;
        const double scale_x = viewport.width/display_width, scale_y = viewport.height/display_height;
        if (scale_x <= 0 || scale_y <= 0) return p;
        const double left = viewport.originX + double(x_)*viewport.width/desktop_width_ - hotspot_x_*scale_x;
        const double top = viewport.originY + double(y_)*viewport.height/desktop_height_ - hotspot_y_*scale_y;
        // Clip in double precision before shader conversion, including extreme
        // off-screen coordinates. Shapes are already in displayed orientation.
        if (left >= viewport.originX+viewport.width || top >= viewport.originY+viewport.height ||
            left+width_*scale_x <= viewport.originX || top+height_*scale_y <= viewport.originY) return p;
        return {float(left),float(top),float(1/scale_x),float(1/scale_y),width_,height_,format_,1};
    }
    id<MTLTexture> image(id<MTLDevice> device) {
        if (shape_) return shape_;
        if (!empty_) { const uint8_t zero[4]{}; empty_ = texture(device,1,1,zero); }
        return empty_;
    }
};
}
