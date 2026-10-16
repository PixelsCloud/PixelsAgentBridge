#pragma once

// Opt-in acceptance window. Only aggregate test-key counters for this window
// are emitted. No hooks, character messages, text logging, or SendInput calls.
struct InputFixture {
    unsigned clicks = 0, key_down = 0, key_up = 0, control_down = 0, control_up = 0;
    int wheel = 0;
    POINT last_click{-1, -1};
    static LRESULT CALLBACK procedure(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
        auto* self = reinterpret_cast<InputFixture*>(GetWindowLongPtrW(window, GWLP_USERDATA));
        if (message == WM_NCCREATE) {
            self = static_cast<InputFixture*>(reinterpret_cast<CREATESTRUCTW*>(lparam)->lpCreateParams);
            SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(self));
        }
        if (self) {
            if (message == WM_LBUTTONDOWN) {
                ++self->clicks;
                self->last_click = POINT{GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
                ClientToScreen(window, &self->last_click);
            }
            if (message == WM_MOUSEWHEEL) self->wheel += GET_WHEEL_DELTA_WPARAM(wparam);
            if (message == WM_KEYDOWN && wparam == VK_F24) ++self->key_down;
            if (message == WM_KEYUP && wparam == VK_F24) ++self->key_up;
            if (message == WM_KEYDOWN && wparam == VK_CONTROL) ++self->control_down;
            if (message == WM_KEYUP && wparam == VK_CONTROL) ++self->control_up;
        }
        return DefWindowProcW(window, message, wparam, lparam);
    }
};
static void input_fixture(const Display& display, unsigned seconds) {
    InputFixture state;
    WNDCLASSW cls{}; cls.lpfnWndProc = InputFixture::procedure;
    cls.hInstance = GetModuleHandleW(nullptr); cls.lpszClassName = L"PixelsRemoteDesktopInputAcceptance";
    cls.hbrBackground = reinterpret_cast<HBRUSH>(COLOR_WINDOW + 1);
    if (!RegisterClassW(&cls)) throw std::runtime_error("Cannot register input acceptance window");
    const auto rect = display.screen.DesktopCoordinates;
    HWND window = CreateWindowExW(WS_EX_TOPMOST, cls.lpszClassName,
        L"Pixels input acceptance - isolated test window", WS_OVERLAPPEDWINDOW,
        (rect.left+rect.right)/2-400, (rect.top+rect.bottom)/2-225, 800, 450,
        nullptr, nullptr, cls.hInstance, &state);
    if (!window) throw std::runtime_error("Cannot create input acceptance window");
    ShowWindow(window, SW_SHOWNOACTIVATE);
    const auto until = Clock::now() + std::chrono::seconds(seconds);
    while (IsWindow(window) && Clock::now() < until) {
        MSG message{};
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message); DispatchMessageW(&message);
        }
        std::cout << "{\"foreground\":" << (GetForegroundWindow() == window ? "true" : "false")
            << ",\"clicks\":" << state.clicks << ",\"f24_down\":" << state.key_down << ",\"f24_up\":" << state.key_up
            << ",\"ctrl_down\":" << state.control_down << ",\"ctrl_up\":" << state.control_up
            << ",\"wheel\":" << state.wheel
            << ",\"last_click_x\":" << state.last_click.x << ",\"last_click_y\":" << state.last_click.y
            << ",\"ctrl_held\":" << ((GetAsyncKeyState(VK_CONTROL) & 0x8000) ? "true" : "false")
            << ",\"left_held\":" << ((GetAsyncKeyState(VK_LBUTTON) & 0x8000) ? "true" : "false") << "}\n" << std::flush;
        Sleep(100);
    }
    if (IsWindow(window)) DestroyWindow(window);
}
