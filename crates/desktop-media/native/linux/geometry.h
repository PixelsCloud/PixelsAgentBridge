#pragma once
#include "common.h"

namespace pab_linux {
struct Viewport { int x, y, width, height; };
inline Viewport viewport(unsigned frame_width, unsigned frame_height, unsigned rotation, int width, int height) {
    if (!frame_width || !frame_height || rotation > 3 || width <= 0 || height <= 0)
        throw std::runtime_error("Invalid native viewport");
    const auto w = rotation % 2 ? frame_height : frame_width, h = rotation % 2 ? frame_width : frame_height;
    const double scale = std::min(double(width) / w, double(height) / h);
    const auto dw = std::max(1, int(std::lround(w * scale))), dh = std::max(1, int(std::lround(h * scale)));
    return {(width - dw) / 2, (height - dh) / 2, dw, dh};
}
}
