// Bounded own-window diagnostic for activation, standard controls and pointer gestures.
// Does not change product input semantics or inspect other applications' content.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <windowsx.h>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <iostream>
#include <string>
#include <thread>

namespace {
using Clock = std::chrono::steady_clock;
constexpr ULONG_PTR marker = 0x50414243;
struct Counters {
    unsigned down = 0, up = 0, double_click = 0, actions = 0;
    unsigned drag_starts = 0, drag_ends = 0, held_moves = 0;
    bool dragging = false;
    POINT last_up{-1, -1};
} counts;
WNDPROC original_control = nullptr;
HWND parent_window = nullptr, target_window = nullptr;
bool surface = false;
bool no_activate = false, legacy_input = false;
std::atomic<bool> injected{false};
std::atomic<bool> active_at_input{false};
std::atomic<unsigned> submitted{0};
std::atomic<bool> interrupted{false};
void activation_state(const char* phase, UINT message, LRESULT result) {
    const HWND foreground = GetForegroundWindow(); DWORD pid = 0;
    const DWORD thread = foreground ? GetWindowThreadProcessId(foreground, &pid) : 0;
    // Window class only, never another application's title or content.
    char name[96]{}; if (foreground) GetClassNameA(foreground, name, sizeof(name));
    std::cout << "{\"activation_phase\":\"" << phase << "\",\"message\":" << message
        << ",\"result\":" << result << ",\"foreground_own\":" << (foreground == parent_window ? "true" : "false")
        << ",\"foreground_pid\":" << pid << ",\"foreground_thread\":" << thread
        << ",\"foreground_class\":\"" << name << "\",\"active_own\":" << (GetActiveWindow() == parent_window ? "true" : "false")
        << ",\"focus_target\":" << (GetFocus() == target_window ? "true" : "false") << "}\n" << std::flush;
}

LRESULT CALLBACK control_proc(HWND window, UINT message, WPARAM w, LPARAM l) {
    if (message == WM_KEYDOWN || message == WM_SYSKEYDOWN) interrupted = true;
    if (message == WM_LBUTTONDOWN || message == WM_LBUTTONUP || message == WM_LBUTTONDBLCLK) {
        if (static_cast<ULONG_PTR>(GetMessageExtraInfo()) != marker) interrupted = true;
        std::cout << "{\"message\":" << message << ",\"time\":" << GetMessageTime()
                  << ",\"extra\":" << GetMessageExtraInfo() << "}\n" << std::flush;
        if (message == WM_LBUTTONDOWN) ++counts.down;
        if (message == WM_LBUTTONDBLCLK) ++counts.double_click;
        if (message == WM_LBUTTONUP) {
            ++counts.up;
            counts.last_up = {GET_X_LPARAM(l), GET_Y_LPARAM(l)};
        }
        activation_state("button", message, 0);
    }
    if (surface) {
        if (message == WM_LBUTTONDOWN || message == WM_LBUTTONDBLCLK) {
            ++counts.drag_starts;
            counts.dragging = true;
            SetCapture(window);
        }
        if (message == WM_MOUSEMOVE && counts.dragging && (w & MK_LBUTTON)) ++counts.held_moves;
        if (message == WM_LBUTTONUP && counts.dragging) {
            ++counts.drag_ends;
            counts.dragging = false;
            ReleaseCapture();
        }
    }
    return original_control ? CallWindowProcW(original_control, window, message, w, l)
                            : DefWindowProcW(window, message, w, l);
}
LRESULT CALLBACK parent_proc(HWND window, UINT message, WPARAM w, LPARAM l) {
    if (message == WM_KEYDOWN || message == WM_SYSKEYDOWN) interrupted = true;
    if (message == WM_COMMAND && HWND(l) == target_window && HIWORD(w) == BN_CLICKED) ++counts.actions;
    const auto result = message == WM_MOUSEACTIVATE && no_activate ? MA_NOACTIVATE : DefWindowProcW(window, message, w, l);
    if (message == WM_MOUSEACTIVATE || message == WM_ACTIVATE || message == WM_SETFOCUS)
        activation_state("parent", message, result);
    return result;
}
bool on_own_desktop() {
    auto input = OpenInputDesktop(0, FALSE, DESKTOP_READOBJECTS);
    if (!input) return false;
    wchar_t active[128]{}, own[128]{};
    DWORD needed = 0;
    const bool ok = GetUserObjectInformationW(input, UOI_NAME, active, sizeof(active), &needed)
        && GetUserObjectInformationW(GetThreadDesktop(GetCurrentThreadId()), UOI_NAME, own, sizeof(own), &needed)
        && std::wstring(active) == own;
    CloseDesktop(input);
    return ok;
}
bool owns_point(POINT p) {
    return IsWindow(target_window) && WindowFromPoint(p) == target_window && on_own_desktop();
}
bool send_pointer(POINT point, DWORD flags, bool move = true, bool batch = false) {
    if (interrupted || !owns_point(point)) return false;
    const int x = GetSystemMetrics(SM_XVIRTUALSCREEN), y = GetSystemMetrics(SM_YVIRTUALSCREEN);
    const int width = GetSystemMetrics(SM_CXVIRTUALSCREEN), height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    if (width <= 0 || height <= 0) return false;
    INPUT event{};
    event.type = INPUT_MOUSE;
    event.mi.dx = LONG((int64_t(point.x - x) * 65536 + 32768) / width);
    event.mi.dy = LONG((int64_t(point.y - y) * 65536 + 32768) / height);
    event.mi.dwFlags = (move ? MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK : 0) | flags;
    event.mi.dwExtraInfo = marker;
    INPUT pair[2]{event, {}};
    if (batch && flags) {
        pair[0].mi.dwFlags &= ~flags;
        pair[1].type = INPUT_MOUSE;
        pair[1].mi.dwFlags = flags;
        pair[1].mi.dwExtraInfo = marker;
    }
    const UINT count = batch && flags ? 2 : 1;
    UINT result = 0;
    if (legacy_input) {
        // Diagnostic comparison only; the product continues to use SendInput.
        for (UINT index = 0; index < count; ++index) {
            const auto& m = pair[index].mi;
            mouse_event(m.dwFlags, m.dx, m.dy, m.mouseData, m.dwExtraInfo);
        }
        result = count; // API calls made; correctness still requires observed messages.
    } else result = SendInput(count, pair, sizeof(event));
    submitted += result;
    return result == count;
}
bool release_button() {
    INPUT event{};
    event.type = INPUT_MOUSE;
    event.mi.dwFlags = MOUSEEVENTF_LEFTUP;
    event.mi.dwExtraInfo = marker;
    UINT result = 0;
    if (legacy_input) {
        mouse_event(event.mi.dwFlags, 0, 0, 0, marker);
        result = 1;
    } else result = SendInput(1, &event, sizeof(event));
    submitted += result;
    return result == 1;
}
} // namespace

int main(int argc, char** argv) {
    const std::string requested = argc == 2 ? argv[1] : "";
    const bool separate = requested.find("separate-") == 0;
    const bool batch = requested.find("batch-") == 0;
    const bool focus = requested.find("focused-") == 0;
    no_activate = requested.find("noactivate-") == 0;
    legacy_input = requested.find("legacy-") == 0;
    const bool blocking = requested.find("getmessage-") == 0;
    const std::string mode = requested.substr(separate ? 9 : batch ? 6 : focus ? 8 : no_activate ? 11 : legacy_input ? 7 : blocking ? 11 : 0);
    if (mode != "button-self" && mode != "checkbox-self" && mode != "doubleclick-self" && mode != "drag-self") {
        std::cerr << "Specify button-self, checkbox-self, doubleclick-self or drag-self (optional separate-, batch-, focused-, noactivate-, legacy- or getmessage- prefix)\n";
        return 2;
    }
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    if (!on_own_desktop()) { std::cerr << "User desktop is not active\n"; return 3; }
    WNDCLASSW cls{};
    cls.hInstance = GetModuleHandleW(nullptr);
    cls.lpfnWndProc = parent_proc;
    cls.lpszClassName = L"PixelsControlAcceptanceParent";
    cls.hbrBackground = reinterpret_cast<HBRUSH>(COLOR_WINDOW + 1);
    if (!RegisterClassW(&cls)) return 4;
    cls.lpfnWndProc = control_proc;
    cls.lpszClassName = L"PixelsControlAcceptanceSurface";
    cls.style = CS_DBLCLKS;
    if (!RegisterClassW(&cls)) return 4;
    const int left = GetSystemMetrics(SM_CXSCREEN) / 2 - 300;
    const int top = GetSystemMetrics(SM_CYSCREEN) / 2 - 160;
    parent_window = CreateWindowExW(WS_EX_TOPMOST, L"PixelsControlAcceptanceParent",
        L"Pixels isolated standard-control diagnostic (5 seconds)", WS_OVERLAPPEDWINDOW,
        left, top, 600, 320, nullptr, nullptr, cls.hInstance, nullptr);
    if (!parent_window) return 5;
    surface = mode == "doubleclick-self" || mode == "drag-self";
    target_window = CreateWindowExW(0, surface ? L"PixelsControlAcceptanceSurface" : L"BUTTON",
        surface ? L"Gesture surface" : L"Single action", WS_CHILD | WS_VISIBLE |
        (mode == "checkbox-self" ? BS_AUTOCHECKBOX : BS_PUSHBUTTON),
        60, 50, 460, 160, parent_window, HMENU(1), cls.hInstance, nullptr);
    if (!target_window) { DestroyWindow(parent_window); return 6; }
    if (!surface) original_control = reinterpret_cast<WNDPROC>(SetWindowLongPtrW(target_window, GWLP_WNDPROC, reinterpret_cast<LONG_PTR>(control_proc)));
    ShowWindow(parent_window, SW_SHOWNOACTIVATE);
    const HWND previous_foreground = GetForegroundWindow();
    if (focus) SetForegroundWindow(parent_window);
    const bool initially_active = GetForegroundWindow() == parent_window;
    POINT previous_cursor{};
    GetCursorPos(&previous_cursor);
    std::thread input([mode, separate, batch] {
        Sleep(500);
        for (int key : {VK_LBUTTON, VK_RBUTTON, VK_MBUTTON, VK_CONTROL, VK_SHIFT, VK_MENU}) {
            if (GetAsyncKeyState(key) & 0x8000) return;
        }
        POINT point{150, 80};
        ClientToScreen(target_window, &point);
        if (separate) {
            if (!send_pointer(point, 0)) return;
            Sleep(100);
        }
        active_at_input = GetForegroundWindow() == parent_window;
        bool ok = send_pointer(point, MOUSEEVENTF_LEFTDOWN, !separate, batch);
        const bool pressed = ok;
        Sleep(50);
        if (mode == "drag-self" && ok) {
            for (unsigned step = 0; step < 5 && ok; ++step) {
                point.x += 20;
                ok = send_pointer(point, 0);
                Sleep(30);
            }
        }
        if (pressed) ok = release_button() && ok;
        if (mode == "doubleclick-self" && ok) {
            Sleep(50);
            const bool second = send_pointer(point, MOUSEEVENTF_LEFTDOWN, !separate, batch);
            ok = second;
            Sleep(50);
            if (second) ok = release_button() && ok;
        }
        injected = ok;
    });
    const auto deadline = Clock::now() + std::chrono::seconds(5);
    if (blocking && !SetTimer(parent_window, 1, 50, nullptr)) {
        input.join(); DestroyWindow(parent_window); return 7;
    }
    while (IsWindow(parent_window) && Clock::now() < deadline && !interrupted) {
        MSG message{};
        if (blocking) {
            const auto result = GetMessageW(&message, nullptr, 0, 0);
            if (result <= 0) break;
            TranslateMessage(&message);
            DispatchMessageW(&message);
        } else {
            while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            MsgWaitForMultipleObjects(0, nullptr, FALSE, 10, QS_ALLINPUT);
        }
    }
    input.join();
    const auto checked = mode == "checkbox-self" && IsWindow(target_window)
        ? SendMessageW(target_window, BM_GETCHECK, 0, 0) : 0;
    const bool left_held = (GetAsyncKeyState(VK_LBUTTON) & 0x8000) != 0;
    const bool gestures_ok = mode == "doubleclick-self" ? counts.down == 1 && counts.double_click == 1
            && counts.up == 2 && counts.drag_starts == 2 && counts.drag_ends == 2
        : mode == "drag-self" ? counts.drag_starts == 1 && counts.drag_ends == 1 && counts.held_moves >= 1
            && counts.last_up.x == 250 && counts.last_up.y == 80
        : counts.actions == 1 && (mode != "checkbox-self" || checked == BST_CHECKED);
    const bool sequence_ok = mode == "doubleclick-self" ? counts.down == 1 && counts.double_click == 1 && counts.up == 2
        : counts.down == 1 && counts.double_click == 0 && counts.up == 1;
    const bool passed = !interrupted && active_at_input == focus && injected && !left_held && !counts.dragging && gestures_ok && sequence_ok;
    std::cout << "{\"mode\":\"" << requested << "\",\"initially_active\":" << (initially_active ? "true" : "false")
        << ",\"active_at_input\":" << (active_at_input ? "true" : "false")
        << ",\"submitted\":" << submitted << ",\"injected\":" << (injected ? "true" : "false")
        << ",\"raw_down\":" << counts.down << ",\"raw_up\":" << counts.up << ",\"double_click\":" << counts.double_click
        << ",\"actions\":" << counts.actions << ",\"checked\":" << checked << ",\"drag_starts\":" << counts.drag_starts
        << ",\"drag_ends\":" << counts.drag_ends << ",\"held_moves\":" << counts.held_moves
        << ",\"last_up\":[" << counts.last_up.x << "," << counts.last_up.y << "]"
        << ",\"actions_ok\":" << (gestures_ok ? "true" : "false")
        << ",\"interrupted_by_external_input\":" << (interrupted ? "true" : "false")
        << ",\"sequence_ok\":" << (sequence_ok ? "true" : "false")
        << ",\"left_held\":" << (left_held ? "true" : "false") << ",\"passed\":" << (passed ? "true" : "false") << "}\n";
    POINT now{}; GetCursorPos(&now);
    if (!interrupted && (GetForegroundWindow() == parent_window || GetForegroundWindow() == previous_foreground)
            && WindowFromPoint(now) == target_window) {
        SetCursorPos(previous_cursor.x, previous_cursor.y);
        if (IsWindow(previous_foreground)) SetForegroundWindow(previous_foreground);
    }
    if (IsWindow(parent_window)) DestroyWindow(parent_window);
    return passed ? 0 : 1;
}
