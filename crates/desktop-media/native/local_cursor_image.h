#pragma once
#include <algorithm>
#include <cstdint>
#include <cstring>
#include <stdexcept>
#include <utility>
#include <vector>

namespace pab_cursor {
// One bounded generation-local shape. Remote positions/visibility do not own
// the local OS pointer. Clearing the source restores the normal local arrow.
struct Image {
    uint8_t format = 0;
    uint32_t width = 0, height = 0, hotspot_x = 0, hotspot_y = 0;
    std::vector<uint8_t> pixels;
    bool update(const uint8_t* bytes, size_t size) {
        size_t offset = 0;
        const auto read = [&](auto& value) {
            if (sizeof(value) > size-offset) throw std::runtime_error("Truncated local cursor command");
            std::memcpy(&value,bytes+offset,sizeof(value)); offset += sizeof(value);
        };
        uint8_t action = 0; read(action);
        if (!action) {
            if (offset != size) throw std::runtime_error("Invalid local cursor clear");
            *this = {}; return true;
        }
        if (action != 1) throw std::runtime_error("Invalid local cursor operation");
        uint8_t visible = 0, next_format = 0; int64_t x = 0, y = 0;
        uint32_t desktop_width = 0, desktop_height = 0;
        read(visible); read(x); read(y); read(desktop_width); read(desktop_height); read(next_format);
        if (visible > 1 || !desktop_width || !desktop_height || desktop_width > 8192 || desktop_height > 8192 ||
            x < -INT64_C(4294967295) || x > INT64_C(4294967295) || y < -INT64_C(4294967295) || y > INT64_C(4294967295))
            throw std::runtime_error("Invalid local cursor metadata");
        if (!next_format) {
            if (offset != size || (visible && !format)) throw std::runtime_error("Invalid local cursor reference");
            return false;
        }
        Image next; next.format = next_format;
        read(next.width); read(next.height); read(next.hotspot_x); read(next.hotspot_y);
        if (next.format > 2 || !next.width || !next.height || next.width > 512 || next.height > 512 ||
            next.hotspot_x >= next.width || next.hotspot_y >= next.height ||
            size-offset != size_t(next.width)*next.height*(next.format == 1 ? 4 : 8))
            throw std::runtime_error("Invalid local cursor image");
        next.pixels.assign(bytes+offset,bytes+size);
        *this = std::move(next); return true;
    }
    // AppKit/GDK need straight RGBA. Classic invert pixels cannot depend on
    // the remote video background in a hardware cursor; use a contrasting
    // black/white outline while preserving the original silhouette/hotspot.
    std::vector<uint8_t> rgba() const {
        std::vector<uint8_t> out(size_t(width)*height*4,0);
        for (size_t i = 0; i < size_t(width)*height; ++i) {
            const auto* p = pixels.data()+i*(format == 1 ? 4 : 8);
            auto* q = out.data()+i*4;
            if (format == 1) {
                q[3] = p[3];
                for (size_t c = 0; c < 3; ++c) q[c] = p[3] ? uint8_t(std::min(255u,(unsigned(p[2-c])*255+p[3]/2)/p[3])) : 0;
            } else if (p[0] || p[1] || p[2]) {
                if (p[4] || p[5] || p[6]) { q[0]=q[1]=q[2]=q[3]=255; }
            } else { q[0]=p[6]; q[1]=p[5]; q[2]=p[4]; q[3]=255; }
        }
        if (format == 2) {
            const auto original = out;
            for (uint32_t y = 0; y < height; ++y) for (uint32_t x = 0; x < width; ++x) {
                const size_t i = (size_t(y)*width+x)*4;
                if (original[i+3]) continue;
                bool edge = false;
                for (int dy = -1; dy <= 1; ++dy) for (int dx = -1; dx <= 1; ++dx) {
                    const int xx = int(x)+dx, yy = int(y)+dy;
                    if (xx >= 0 && yy >= 0 && xx < int(width) && yy < int(height)) {
                        const size_t j = (size_t(yy)*width+xx)*4;
                        edge = edge || (original[j+3] && original[j] == 255 && original[j+1] == 255 && original[j+2] == 255);
                    }
                }
                if (edge) out[i+3] = 255;
            }
        }
        return out;
    }
};
}
