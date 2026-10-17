// Bounded, generated 1080p desktop pattern. No screen capture or user input.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <vector>

using Clock = std::chrono::steady_clock;
static std::vector<uint32_t> pixels(1920 * 1080);
static unsigned frame;
static void paint(HWND window) {
    for (unsigned y = 0; y < 1080; ++y) for (unsigned x = 0; x < 1920; ++x) {
        const unsigned phase = (frame * 3 + (x / 120) * 10 + (y / 90) * 10) % 128;
        pixels[y * 1920 + x] = (phase * 2 << 16) | ((128 - phase) * 2 << 8) | 89;
    }
    for (unsigned row = 0; row < 2; ++row) for (unsigned i = 0; i < 16; ++i) {
        unsigned bit = i == 0 || i == 15 ? 1 : i == 1 || i == 14 ? 0 : (frame >> (i - 2)) & 1;
        for (unsigned y = 160 + row * 96; y < 240 + row * 96; ++y)
            for (unsigned x = 192 + i * 96; x < 288 + i * 96; ++x)
                pixels[y * 1920 + x] = (bit ^ row) ? 0xffffff : 0;
    }
    InvalidateRect(window, nullptr, FALSE); UpdateWindow(window);
}
static LRESULT CALLBACK procedure(HWND window, UINT message, WPARAM w, LPARAM l) {
    if (message == WM_PAINT) {
        PAINTSTRUCT paint{}; auto dc = BeginPaint(window, &paint);
        BITMAPINFO info{}; info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
        info.bmiHeader.biWidth = 1920; info.bmiHeader.biHeight = -1080;
        info.bmiHeader.biPlanes = 1; info.bmiHeader.biBitCount = 32;
        SetDIBitsToDevice(dc, 0, 0, 1920, 1080, 0, 0, 0, 1080, pixels.data(), &info, DIB_RGB_COLORS);
        EndPaint(window, &paint); return 0;
    }
    return DefWindowProcW(window, message, w, l);
}
int main() {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    if (GetSystemMetrics(SM_CXSCREEN) != 1920 || GetSystemMetrics(SM_CYSCREEN) != 1080) return 2;
    WNDCLASSW cls{}; cls.lpfnWndProc = procedure; cls.hInstance = GetModuleHandleW(nullptr);
    cls.lpszClassName = L"PixelsGeneratedPresentationFixture";
    if (!RegisterClassW(&cls)) return 3;
    auto window = CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        cls.lpszClassName, L"Pixels generated 1080p presentation fixture", WS_POPUP,
        0, 0, 1920, 1080, nullptr, nullptr, cls.hInstance, nullptr);
    if (!window) return 4;
    paint(window); ShowWindow(window, SW_SHOWNOACTIVATE);
    const auto start = Clock::now();
    while (IsWindow(window) && Clock::now() - start < std::chrono::seconds(120)) {
        MSG message{};
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) { TranslateMessage(&message); DispatchMessageW(&message); }
        const unsigned current = unsigned(std::chrono::duration<double>(Clock::now() - start).count() * 30);
        if (current != frame) { frame = current; paint(window); }
        MsgWaitForMultipleObjects(0, nullptr, FALSE, 2, QS_ALLINPUT);
    }
    if (IsWindow(window)) DestroyWindow(window);
    printf("{\"generated_frames\":%u,\"width\":1920,\"height\":1080}\n", frame);
    return 0;
}
