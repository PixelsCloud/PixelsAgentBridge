#pragma once
#include "../local_cursor_image.h"

// These handles belong only to the viewer. Never replace the user's global
// cursor scheme or move the pointer in response to remote position packets.
class LocalCursor {
    HCURSOR handle_ = nullptr;
    static HCURSOR create(const pab_cursor::Image& image) {
        if (!image.format) return nullptr;
        struct Bitmap { HBITMAP value = nullptr; ~Bitmap() { if (value) DeleteObject(value); } } color, mask;
        BITMAPINFO info{}; info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
        info.bmiHeader.biWidth = image.width; info.bmiHeader.biHeight = -LONG(image.height);
        info.bmiHeader.biPlanes = 1; info.bmiHeader.biBitCount = 32; info.bmiHeader.biCompression = BI_RGB;
        void* colors = nullptr;
        color.value = CreateDIBSection(nullptr,&info,DIB_RGB_COLORS,&colors,nullptr,0);
        struct MaskInfo { BITMAPINFOHEADER header; RGBQUAD colors[2]; } mono{};
        mono.header = info.bmiHeader; mono.header.biBitCount = 1;
        mono.colors[1].rgbRed = mono.colors[1].rgbGreen = mono.colors[1].rgbBlue = 255;
        void* masks = nullptr;
        mask.value = CreateDIBSection(nullptr,reinterpret_cast<BITMAPINFO*>(&mono),DIB_RGB_COLORS,&masks,nullptr,0);
        if (!color.value || !mask.value || !colors || !masks) return nullptr;
        const size_t stride = ((image.width+31)/32)*4;
        std::memset(masks,0,stride*image.height);
        for (uint32_t y = 0; y < image.height; ++y) for (uint32_t x = 0; x < image.width; ++x) {
            const size_t i = size_t(y)*image.width+x;
            auto* target = static_cast<uint8_t*>(colors)+i*4;
            const auto* source = image.pixels.data()+i*(image.format == 1 ? 4 : 8);
            bool transparent;
            if (image.format == 1) {
                target[3]=source[3]; transparent=!source[3];
                // The OS cursor bitmap consumes straight color; the wire is
                // premultiplied. Passing it through would darken translucent
                // edges a second time when Windows draws the native cursor.
                for (size_t c=0;c<3;++c) target[c]=source[3] ? uint8_t(std::min(255u,(unsigned(source[c])*255+source[3]/2)/source[3])) : 0;
            }
            else {
                target[0]=source[4]; target[1]=source[5]; target[2]=source[6]; target[3]=0;
                transparent = source[0] || source[1] || source[2];
            }
            if (transparent) static_cast<uint8_t*>(masks)[y*stride+x/8] |= uint8_t(0x80>>(x%8));
        }
        ICONINFO cursor{}; cursor.xHotspot=image.hotspot_x; cursor.yHotspot=image.hotspot_y;
        cursor.hbmColor=color.value; cursor.hbmMask=mask.value;
        return static_cast<HCURSOR>(CreateIconIndirect(&cursor));
    }
public:
    LocalCursor() = default;
    LocalCursor(const LocalCursor&) = delete;
    LocalCursor& operator=(const LocalCursor&) = delete;
    ~LocalCursor() { replace({}); }
    HCURSOR get() const { return handle_ ? handle_ : LoadCursorW(nullptr,IDC_ARROW); }
    void replace(const pab_cursor::Image& image) {
        auto next = create(image);
        auto old = handle_; handle_ = next;
        if (old && GetCursor() == old) SetCursor(get());
        if (old) DestroyCursor(old);
    }
};
