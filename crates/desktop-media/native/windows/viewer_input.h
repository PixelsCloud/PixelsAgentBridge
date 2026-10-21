#pragma once
#include <windowsx.h>

// Per-window events only: no global keyboard/mouse hook, and disabled by default.
class ViewerInput {
    bool enabled_ = false;
    bool fullscreen_ = false, escape_held_ = false;
    unsigned buttons_ = 0;
public:
    bool available = false;
    void fullscreen(bool enabled) { fullscreen_ = enabled; }
    bool enabled() const { return enabled_; }
    void emit(uint8_t kind, uint16_t x = 0, uint16_t y = 0, uint16_t value = 0, bool down = false) {
        const uint8_t bytes[8]{kind, uint8_t(x), uint8_t(x >> 8), uint8_t(y), uint8_t(y >> 8),
            uint8_t(value), uint8_t(value >> 8), uint8_t(down)};
        DWORD written = 0;
        // Interactive mode has a dedicated, continuously drained binary stderr
        // pipe. Frame ACKs remain on stdout and cannot wait behind network input.
        if (!WriteFile(GetStdHandle(STD_ERROR_HANDLE), bytes, sizeof(bytes), &written, nullptr) || written != sizeof(bytes)) {
            enabled_ = false;
        }
    }
    void release(HWND window) {
        if (enabled_) emit(5);
        buttons_ = 0;
        if (GetCapture() == window) ReleaseCapture();
    }
    void enable(HWND window, bool enabled) {
        release(window);
        enabled_ = available && enabled;
        if (available) emit(6, 0, 0, enabled_ ? 1 : 0);
    }
    bool message(HWND window, UINT message, WPARAM wparam, LPARAM lparam, const RECT& viewport) {
        if ((message == WM_KEYDOWN || message == WM_SYSKEYDOWN || message == WM_KEYUP || message == WM_SYSKEYUP) && wparam == VK_ESCAPE) {
            const bool down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            if (escape_held_) { if (!down) escape_held_ = false; return true; }
            if (fullscreen_ && down) { escape_held_ = true; release(window); emit(8); return true; }
        }
        if (!enabled_) return false;
        if (message == WM_KILLFOCUS || (message == WM_CAPTURECHANGED && buttons_)) { release(window); return false; }
        if (message == WM_KEYDOWN || message == WM_SYSKEYDOWN || message == WM_KEYUP || message == WM_SYSKEYUP) {
            unsigned key = static_cast<unsigned>(wparam);
            const bool extended = (lparam & (1LL << 24)) != 0;
            if (key == VK_SHIFT) key = MapVirtualKeyW((lparam >> 16) & 255, MAPVK_VSC_TO_VK_EX);
            if (key == VK_CONTROL) key = extended ? VK_RCONTROL : VK_LCONTROL;
            if (key == VK_MENU) key = extended ? VK_RMENU : VK_LMENU;
            if (key && key <= 254) emit(4, 0, 0, static_cast<uint16_t>(key), message == WM_KEYDOWN || message == WM_SYSKEYDOWN);
            return true;
        }
        unsigned button = 0; bool down = false;
        switch (message) {
            case WM_MOUSEMOVE: case WM_MOUSEWHEEL: break;
            case WM_LBUTTONDOWN: down = true; break;
            case WM_LBUTTONUP: break;
            case WM_RBUTTONDOWN: button = 1; down = true; break;
            case WM_RBUTTONUP: button = 1; break;
            case WM_MBUTTONDOWN: button = 2; down = true; break;
            case WM_MBUTTONUP: button = 2; break;
            default: return false;
        }
        POINT point{GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
        if (message == WM_MOUSEWHEEL) ScreenToClient(window, &point);
        if (!buttons_ && !PtInRect(&viewport, point)) return true; // letterbox is not the remote screen
        const auto width = viewport.right-viewport.left, height = viewport.bottom-viewport.top;
        if (width <= 0 || height <= 0) return true;
        const auto coordinate = [](LONG p, LONG origin, LONG length) {
            return static_cast<uint16_t>(int64_t((std::max)(0L, (std::min)(length-1, p-origin))) * 65535 / (std::max)(1L, length-1));
        };
        const auto x = coordinate(point.x, viewport.left, width), y = coordinate(point.y, viewport.top, height);
        if (message == WM_MOUSEWHEEL) { emit(3, x, y, static_cast<uint16_t>(GET_WHEEL_DELTA_WPARAM(wparam))); return true; }
        if (message == WM_MOUSEMOVE) { emit(1, x, y); return true; }
        if (down) { SetFocus(window); SetCapture(window); buttons_ |= 1U << button; }
        emit(2, x, y, static_cast<uint16_t>(button), down);
        if (!down) { buttons_ &= ~(1U << button); if (!buttons_ && GetCapture() == window) ReleaseCapture(); }
        return true;
    }
};
