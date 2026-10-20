#pragma once
#include <cstring>
// Used only after the matching video worker confirms cursor-free WGC capture.
// Read/draw the small cursor bitmap, never the desktop image.
struct CursorIconLease {
    HICON icon = nullptr;
    ICONINFO info{};
    ~CursorIconLease() {
        if (info.hbmColor) DeleteObject(info.hbmColor);
        if (info.hbmMask) DeleteObject(info.hbmMask);
        if (icon) DestroyIcon(icon);
    }
};
struct CursorDib {
    HDC dc = nullptr;
    HBITMAP bitmap = nullptr;
    HGDIOBJ previous = nullptr;
    void* bits = nullptr;
    ~CursorDib() {
        if (previous && previous != HGDI_ERROR) SelectObject(dc, previous);
        if (bitmap) DeleteObject(bitmap);
        if (dc) DeleteDC(dc);
    }
    void initialize(UINT width, UINT height) {
        dc = CreateCompatibleDC(nullptr);
        if (!dc) throw std::runtime_error("Cursor memory DC unavailable");
        BITMAPINFO description{};
        description.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
        description.bmiHeader.biWidth = width;
        description.bmiHeader.biHeight = -LONG(height);
        description.bmiHeader.biPlanes = 1;
        description.bmiHeader.biBitCount = 32;
        description.bmiHeader.biCompression = BI_RGB;
        bitmap = CreateDIBSection(dc, &description, DIB_RGB_COLORS, &bits, nullptr, 0);
        if (!bitmap || !bits) throw std::runtime_error("Cursor bitmap unavailable");
        previous = SelectObject(dc, bitmap);
        if (!previous || previous == HGDI_ERROR) throw std::runtime_error("Cursor bitmap selection failed");
    }
    std::vector<uint8_t> draw(HICON icon, UINT width, UINT height, uint8_t background) {
        const size_t bytes = size_t(width) * height * 4;
        std::memset(bits, background, bytes);
        if (!DrawIconEx(dc, 0, 0, icon, width, height, 0, nullptr, DI_NORMAL | DI_NOMIRROR) || !GdiFlush())
            throw std::runtime_error("Cursor rendering failed");
        const auto* data = static_cast<const uint8_t*>(bits);
        return {data, data + bytes};
    }
};
static CursorImage win32_cursor_image(HCURSOR cursor) {
    CursorIconLease lease;
    lease.icon = CopyIcon(cursor);
    if (!lease.icon || !GetIconInfo(lease.icon, &lease.info) || !lease.info.hbmMask)
        throw std::runtime_error("Cursor shape unavailable");
    BITMAP mask{}, color{};
    if (!GetObjectW(lease.info.hbmMask, sizeof(mask), &mask))
        throw std::runtime_error("Cursor mask unavailable");
    const bool colored = lease.info.hbmColor != nullptr;
    if (mask.bmWidth <= 0 || mask.bmWidth > 512 || mask.bmHeight <= 0 ||
        mask.bmPlanes != 1 || mask.bmBitsPixel != 1 || (!colored && mask.bmHeight % 2))
        throw std::runtime_error("Cursor mask outside bounds");
    const UINT width = mask.bmWidth, height = colored ? mask.bmHeight : mask.bmHeight / 2;
    if (!height || height > 512 || lease.info.xHotspot >= width || lease.info.yHotspot >= height)
        throw std::runtime_error("Cursor geometry outside bounds");
    CursorDib surface; surface.initialize(width, height);
    std::vector<uint8_t> raw(size_t(width) * height * 4, 0);
    bool alpha = false;
    if (colored) {
        if (!GetObjectW(lease.info.hbmColor, sizeof(color), &color) ||
            color.bmWidth != width || color.bmHeight != height || color.bmPlanes != 1)
            throw std::runtime_error("Cursor color geometry changed");
        if (color.bmBitsPixel == 32) {
            BITMAPINFO description{};
            description.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
            description.bmiHeader.biWidth = width; description.bmiHeader.biHeight = -LONG(height);
            description.bmiHeader.biPlanes = 1; description.bmiHeader.biBitCount = 32;
            description.bmiHeader.biCompression = BI_RGB;
            if (GetDIBits(surface.dc, lease.info.hbmColor, 0, height, raw.data(), &description, DIB_RGB_COLORS) != height)
                throw std::runtime_error("Cursor color bitmap unavailable");
            for (size_t i = 3; i < raw.size(); i += 4) alpha = alpha || raw[i] != 0;
        }
    }
    const auto black = surface.draw(lease.icon, width, height, 0);
    CursorImage out;
    out.format = alpha ? 1 : 2; out.width = width; out.height = height;
    out.hotspot_x = lease.info.xHotspot; out.hotspot_y = lease.info.yHotspot;
    if (alpha) {
        // DrawIconEx blends 32-bit alpha cursors. Drawing on black yields the
        // premultiplied color; retain source alpha instead of a GDI DIB's alpha.
        out.pixels = black;
        for (size_t i = 0; i < raw.size(); i += 4) out.pixels[i+3] = raw[i+3];
    } else {
        // Legacy color/monochrome cursors use (background AND mask) XOR image.
        // Black gives XOR, white XOR black gives AND, including invert pixels.
        const auto white = surface.draw(lease.icon, width, height, 255);
        out.pixels.resize(size_t(width) * height * 8, 0);
        for (size_t i = 0; i < size_t(width) * height; ++i)
            for (size_t c = 0; c < 3; ++c) {
                out.pixels[i*8+c] = white[i*4+c] ^ black[i*4+c];
                out.pixels[i*8+4+c] = black[i*4+c];
            }
    }
    return out;
}
