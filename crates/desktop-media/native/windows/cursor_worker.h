#pragma once
// A second, cursor-only duplication client keeps metadata current even when
// video credit is exhausted. It never maps/copies/encodes the desktop surface.
// Only the latest shape/position is retained, and replies are pull based.
#include <limits>

struct CursorImage {
    uint8_t format = 0;
    uint32_t width = 0, height = 0, hotspot_x = 0, hotspot_y = 0;
    std::vector<uint8_t> pixels;
    bool operator==(const CursorImage& other) const {
        return format == other.format && width == other.width && height == other.height &&
            hotspot_x == other.hotspot_x && hotspot_y == other.hotspot_y && pixels == other.pixels;
    }
};
static constexpr uint32_t max_cursor_bytes = 512 * 512 * 8;
#include "win32_cursor.h"
static CursorImage cursor_image(const DXGI_OUTDUPL_POINTER_SHAPE_INFO& shape,
    const std::vector<uint8_t>& raw) {
    const bool mono = shape.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME;
    const bool color = shape.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR;
    if (!mono && !color && shape.Type != DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR)
        throw std::runtime_error("Unknown DXGI cursor format");
    const auto height = mono ? shape.Height / 2 : shape.Height;
    if (!shape.Width || shape.Width > 512 || !height || height > 512 || (mono && shape.Height % 2) ||
        shape.HotSpot.x < 0 || shape.HotSpot.y < 0 || uint32_t(shape.HotSpot.x) >= shape.Width ||
        uint32_t(shape.HotSpot.y) >= height)
        throw std::runtime_error("Cursor dimensions or hotspot outside bounds");
    const uint32_t row = mono ? (shape.Width + 7) / 8 : shape.Width * 4;
    if (shape.Pitch < row || uint64_t(shape.Pitch) * shape.Height > max_cursor_bytes ||
        raw.size() < uint64_t(shape.Pitch) * shape.Height)
        throw std::runtime_error("Invalid DXGI cursor pitch or payload");
    CursorImage out;
    out.format = color ? 1 : 2; out.width = shape.Width; out.height = height;
    out.hotspot_x = shape.HotSpot.x; out.hotspot_y = shape.HotSpot.y;
    out.pixels.resize(size_t(out.width) * height * (color ? 4 : 8));
    for (uint32_t y = 0; y < height; ++y) for (uint32_t x = 0; x < out.width; ++x) {
        const size_t target = (size_t(y) * out.width + x) * (color ? 4 : 8);
        if (mono) {
            const uint8_t bit = uint8_t(0x80 >> (x % 8));
            const uint8_t and_mask = (raw[size_t(y) * shape.Pitch + x / 8] & bit) ? 255 : 0;
            const uint8_t xor_mask = (raw[size_t(y + height) * shape.Pitch + x / 8] & bit) ? 255 : 0;
            for (unsigned c = 0; c < 3; ++c) {
                out.pixels[target + c] = and_mask;
                out.pixels[target + 4 + c] = xor_mask;
            }
        } else {
            const size_t source = size_t(y) * shape.Pitch + x * 4;
            const auto alpha = raw[source + 3];
            if (color) {
                // DXGI color cursors use straight ARGB; the wire format is
                // premultiplied BGRA. This small shape conversion is not a
                // desktop readback or video color conversion.
                for (unsigned c = 0; c < 3; ++c)
                    out.pixels[target + c] = uint8_t((unsigned(raw[source + c]) * alpha + 127) / 255);
                out.pixels[target + 3] = alpha;
            } else {
                if (alpha != 0 && alpha != 255) throw std::runtime_error("Invalid masked cursor alpha");
                for (unsigned c = 0; c < 3; ++c) {
                    out.pixels[target + c] = alpha;
                    out.pixels[target + 4 + c] = raw[source + c];
                }
            }
        }
    }
    return out;
}
static int32_t cursor_coordinate(LONG origin, LONG top_left, uint32_t hotspot) {
    const int64_t value = int64_t(origin) + top_left + hotspot;
    if (value < INT32_MIN || value > INT32_MAX) throw std::runtime_error("Cursor coordinate overflow");
    return int32_t(value);
}
static void cursor_worker(const Display& display, const std::string& capture_token = {}) {
    const auto expected_x = read_value<int32_t>(), expected_y = read_value<int32_t>();
    const auto expected_width = read_value<uint32_t>(), expected_height = read_value<uint32_t>();
    const auto expected_rotation = read_value<uint32_t>();
    const auto& rect = display.screen.DesktopCoordinates;
    const uint32_t rotation = display.screen.Rotation == DXGI_MODE_ROTATION_ROTATE90 ? 1 :
        display.screen.Rotation == DXGI_MODE_ROTATION_ROTATE180 ? 2 :
        display.screen.Rotation == DXGI_MODE_ROTATION_ROTATE270 ? 3 : 0;
    if (expected_x != rect.left || expected_y != rect.top || expected_width != uint32_t(rect.right-rect.left) ||
        expected_height != uint32_t(rect.bottom-rect.top) || expected_rotation != rotation)
        throw std::runtime_error("Cursor layout changed; refresh generation");
    std::unique_ptr<CaptureEvents> coordination;
    if (!capture_token.empty()) coordination = std::make_unique<CaptureEvents>(capture_token, false);
    ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
    ComPtr<IDXGIOutputDuplication> duplication;
    if (!coordination || !coordination->active()) {
        check(D3D11CreateDevice(display.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION,
            &device, nullptr, &context), "Cursor device");
        const auto duplicated = display.output->DuplicateOutput(device.Get(), &duplication);
        if (duplicated == DXGI_ERROR_ACCESS_LOST && coordination) {
            validate_wgc_desktop(display); coordination->request();
        } else check(duplicated, "Cursor DuplicateOutput");
    }
    write_value(magic); write_value(uint32_t(1));
    CursorImage image;
    POINT top_left{};
    uint64_t image_id = 0, sent_image = 0, sequence = 0;
    int32_t x = 0, y = 0;
    bool visible = false, requested = false, dirty = false, initialized = false;
    auto next_poll = Clock::now();
    bool win32 = false;
    HCURSOR last_cursor = nullptr;
    auto next_shape = Clock::now(), next_validation = Clock::now();
    auto fallback_deadline = Clock::now() + std::chrono::seconds(5);
    const auto request_fallback = [&] {
        validate_wgc_desktop(display);
        coordination->request(); duplication.Reset();
        fallback_deadline = Clock::now() + std::chrono::seconds(5);
    };
    while (true) {
        DWORD available = 0;
        if (!PeekNamedPipe(GetStdHandle(STD_INPUT_HANDLE), nullptr, 0, nullptr, &available, nullptr)) return;
        if (available) {
            const auto operation = read_value<uint8_t>();
            if (!operation) return;
            if (operation != 1 || requested) throw std::runtime_error("Invalid cursor request");
            requested = true;
        }
        // Bound native sampling without a busy loop on high-refresh desktops.
        std::this_thread::sleep_until(next_poll);
        next_poll = Clock::now() + std::chrono::milliseconds(8);
        if (coordination && coordination->active()) {
            if (!win32) {
                validate_wgc_desktop(display); duplication.Reset();
                win32 = true; dirty = true; initialized = false;
            }
            if (Clock::now() >= next_validation) {
                validate_wgc_desktop(display);
                next_validation = Clock::now() + std::chrono::milliseconds(100);
            }
            CURSORINFO info{}; info.cbSize = sizeof(info);
            if (!GetCursorInfo(&info)) throw std::runtime_error("System cursor unavailable");
            const bool next_visible = (info.flags & CURSOR_SHOWING) && info.hCursor && PtInRect(&rect, info.ptScreenPos);
            dirty = dirty || !initialized || visible != next_visible;
            visible = next_visible;
            if (visible) {
                if (last_cursor != info.hCursor || Clock::now() >= next_shape || !image.format) {
                    auto next = win32_cursor_image(info.hCursor);
                    if (!(image == next)) {
                        if (image_id == UINT64_MAX) throw std::runtime_error("Cursor image sequence exhausted");
                        image = std::move(next); ++image_id; dirty = true;
                    }
                    last_cursor = info.hCursor;
                    next_shape = Clock::now() + std::chrono::milliseconds(100);
                }
                dirty = dirty || x != info.ptScreenPos.x || y != info.ptScreenPos.y;
                x = info.ptScreenPos.x; y = info.ptScreenPos.y;
            }
            initialized = true;
        } else if (duplication) {
            DXGI_OUTDUPL_FRAME_INFO info{}; ComPtr<IDXGIResource> resource;
            const auto hr = duplication->AcquireNextFrame(16, &info, &resource);
            if (hr == DXGI_ERROR_ACCESS_LOST && coordination) {
                request_fallback(); continue;
            }
            if (hr != DXGI_ERROR_WAIT_TIMEOUT) {
                check(hr, "Cursor AcquireNextFrame");
                FrameLease lease{duplication.Get()};
                const bool first = !initialized;
                bool shape_changed = false;
                if (info.PointerShapeBufferSize || (info.PointerPosition.Visible && !image.format)) {
                    if (info.PointerShapeBufferSize > max_cursor_bytes) throw std::runtime_error("DXGI cursor too large");
                    std::vector<uint8_t> raw(info.PointerShapeBufferSize ? info.PointerShapeBufferSize : max_cursor_bytes);
                    UINT required = 0; DXGI_OUTDUPL_POINTER_SHAPE_INFO shape{};
                    check(duplication->GetFramePointerShape(static_cast<UINT>(raw.size()), raw.data(), &required, &shape),
                        "GetFramePointerShape");
                    if (required > raw.size()) throw std::runtime_error("DXGI cursor payload changed");
                    raw.resize(required);
                    auto next = cursor_image(shape, raw);
                    if (!(image == next)) {
                        image = std::move(next); shape_changed = true;
                        if (image_id == UINT64_MAX) throw std::runtime_error("Cursor image sequence exhausted");
                        ++image_id;
                    }
                }
                if (first || info.LastMouseUpdateTime.QuadPart) {
                    const bool next_visible = info.PointerPosition.Visible != FALSE;
                    // Invisible means embedded, hidden, or on another output.
                    // Never infer visibility from GetCursorInfo instead.
                    if (next_visible) top_left = info.PointerPosition.Position;
                    dirty = dirty || first || visible != next_visible;
                    visible = next_visible;
                }
                if (visible) {
                    if (!image.format) throw std::runtime_error("Visible cursor has no shape");
                    const auto next_x = cursor_coordinate(rect.left, top_left.x, image.hotspot_x);
                    const auto next_y = cursor_coordinate(rect.top, top_left.y, image.hotspot_y);
                    dirty = dirty || x != next_x || y != next_y;
                    x = next_x; y = next_y;
                }
                dirty = dirty || shape_changed;
                initialized = true;
            }
        } else if (Clock::now() >= fallback_deadline) {
            throw std::runtime_error("Video capture fallback timed out");
        } // Always release the desktop lease before any potentially blocked pipe write.
        if (requested && dirty && initialized) {
            if (sequence == UINT64_MAX) throw std::runtime_error("Cursor sequence exhausted");
            const bool shape_changed = image_id != sent_image;
            write_value(uint8_t(1)); write_value(uint16_t(1)); write_value(++sequence);
            write_value(uint8_t(visible)); write_value(x); write_value(y); write_value(image_id);
            write_value(uint8_t(shape_changed ? image.format : 0));
            if (shape_changed) {
                write_value(image.width); write_value(image.height); write_value(image.hotspot_x); write_value(image.hotspot_y);
                write_bytes(image.pixels.data(), static_cast<uint32_t>(image.pixels.size()));
            }
            sent_image = image_id; requested = false; dirty = false;
        }
    }
}
