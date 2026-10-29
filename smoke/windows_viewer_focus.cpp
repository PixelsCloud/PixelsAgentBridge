// Windows interactive smoke: run only on an idle test desktop. Uses generated
// windows in TWO processes; no remote session, clipboard, or desktop capture.
#include <windows.h>
#include <algorithm>
#include <cstdint>
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>
#include "../crates/desktop-media/native/windows/viewer_input.h"

static constexpr UINT ready_message = WM_APP + 100, mode_message = WM_APP + 101;
static HWND surface = nullptr, editor = nullptr;
static unsigned local_keys = 0;
static ViewerInput input;
static void require(bool value, const char* message) { if (!value) throw std::runtime_error(message); }
static void pump(unsigned ms) {
    const auto end = GetTickCount64() + ms;
    do {
        MSG message{};
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message); DispatchMessageW(&message);
        }
        Sleep(1);
    } while (GetTickCount64() < end);
}
static LRESULT CALLBACK host_proc(HWND window, UINT message, WPARAM wp, LPARAM lp) {
    if (message == ready_message) { surface = reinterpret_cast<HWND>(wp); return 0; }
    // Simulate WebView's activation focus policy, including a late focus change.
    if (message == WM_SETFOCUS && editor) SetFocus(editor);
    if (message == WM_KEYDOWN || message == WM_KEYUP) ++local_keys;
    return DefWindowProcW(window, message, wp, lp);
}
static LRESULT CALLBACK child_proc(HWND window, UINT message, WPARAM wp, LPARAM lp) {
    if (message == mode_message) { input.enable(window, wp != 0); return 0; }
    if (message == WM_MOUSEACTIVATE && input.enabled()) return MA_ACTIVATE;
    if (message == WM_GETDLGCODE && input.enabled()) return DLGC_WANTALLKEYS | DLGC_WANTCHARS | DLGC_WANTARROWS | DLGC_WANTTAB;
    RECT bounds{}; GetClientRect(window, &bounds);
    if (input.message(window, message, wp, lp, bounds)) return 0;
    return DefWindowProcW(window, message, wp, lp);
}
static HWND create(const wchar_t* cls, WNDPROC procedure, HWND parent, int y) {
    WNDCLASSW wc{}; wc.hInstance = GetModuleHandleW(nullptr); wc.lpfnWndProc = procedure;
    wc.lpszClassName = cls; wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    RegisterClassW(&wc);
    HWND result = CreateWindowExW(0, cls, L"Pixels generated focus smoke", parent ? WS_CHILD | WS_VISIBLE : WS_OVERLAPPEDWINDOW,
        parent ? 0 : 100, y, 640, parent ? 350 : 480, parent, nullptr, wc.hInstance, nullptr);
    require(result != nullptr, "Cannot create test window"); return result;
}
static HWND focused() {
    GUITHREADINFO info{}; info.cbSize = sizeof(info);
    require(GetGUIThreadInfo(GetWindowThreadProcessId(surface, nullptr), &info) != FALSE, "Cannot inspect child focus");
    return info.hwndFocus;
}
static void point_at(HWND window) {
    RECT rect{}; GetWindowRect(window, &rect);
    SetCursorPos((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
}
static void click() {
    point_at(surface);
    require(SendMessageW(surface, WM_MOUSEACTIVATE, reinterpret_cast<WPARAM>(GetAncestor(surface, GA_ROOT)), MAKELPARAM(HTCLIENT, WM_LBUTTONDOWN)) == MA_ACTIVATE,
        "Native mouse activation delegated to WebView");
    SendMessageW(surface, WM_LBUTTONDOWN, MK_LBUTTON, MAKELPARAM(100,100));
    SendMessageW(surface, WM_LBUTTONUP, 0, MAKELPARAM(100,100));
}
static void key() {
    INPUT events[2]{};
    events[0].type = events[1].type = INPUT_KEYBOARD;
    events[0].ki.wVk = events[1].ki.wVk = VK_F24;
    events[1].ki.dwFlags = KEYEVENTF_KEYUP;
    require(SendInput(2, events, sizeof(INPUT)) == 2, "Cannot send generated F24"); pump(50);
}
int wmain(int argc, wchar_t** argv) {
    if (argc == 3 && std::wstring(argv[1]) == L"--child") {
        HWND host = reinterpret_cast<HWND>(std::stoull(argv[2]));
        HWND child = create(L"PixelsFocusSmokeSurface", child_proc, host, 70);
        input.available = true; input.enable(child, true);
        PostMessageW(host, ready_message, reinterpret_cast<WPARAM>(child), 0);
        const auto deadline = GetTickCount64() + 15000;
        while (IsWindow(host) && GetTickCount64() < deadline) { pump(5); input.observe_focus(child); }
        return 0;
    }
    HANDLE read = nullptr, write = nullptr;
    HWND host = nullptr, other = nullptr;
    PROCESS_INFORMATION process{};
    const HWND previous = GetForegroundWindow(); POINT pointer{}; GetCursorPos(&pointer);
    int result = 1;
    try {
        host = create(L"PixelsFocusSmokeHost", host_proc, nullptr, 100);
        editor = create(L"PixelsFocusSmokeToolbar", host_proc, host, 0);
        SetWindowPos(editor, nullptr, 0, 0, 640, 60, SWP_NOZORDER | SWP_NOACTIVATE);
        ShowWindow(host, SW_SHOW); SetForegroundWindow(host); pump(80);
        require(GetForegroundWindow() == host, "Test desktop must permit foreground activation");
        SECURITY_ATTRIBUTES sa{sizeof(sa), nullptr, TRUE};
        require(CreatePipe(&read, &write, &sa, 8192) != FALSE, "Cannot create input pipe");
        SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0);
        wchar_t executable[32768]{}; GetModuleFileNameW(nullptr, executable, 32768);
        std::wstring command = L"\"" + std::wstring(executable) + L"\" --child " + std::to_wstring(reinterpret_cast<uintptr_t>(host));
        STARTUPINFOW startup{}; startup.cb = sizeof(startup); startup.dwFlags = STARTF_USESTDHANDLES;
        startup.hStdError = write; startup.hStdOutput = write; startup.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
        require(CreateProcessW(executable, command.data(), nullptr, nullptr, TRUE, CREATE_NO_WINDOW, nullptr, nullptr, &startup, &process) != FALSE, "Cannot create separate viewer process");
        const auto deadline = GetTickCount64() + 3000;
        while (!surface && GetTickCount64() < deadline) pump(5);
        require(surface != nullptr, "Viewer did not attach");
        require(GetWindowThreadProcessId(surface, nullptr) != GetCurrentThreadId(), "Fixture must cross GUI threads");
        require((SendMessageW(surface, WM_GETDLGCODE, 0, 0) & DLGC_WANTALLKEYS) != 0, "Tab/key navigation can escape surface");
        click(); SetFocus(editor); // WebView moves focus AFTER mouse-down completes.
        require(focused() == editor, "Late WebView focus takeover was not reproduced");
        pump(180); require(focused() == surface, "Click did not recover late WebView focus takeover");
        key(); require(local_keys == 0, "Keyboard escaped to local toolbar");
        SendMessageW(surface, WM_SYSCHAR, 'f', 1LL << 29); pump(50);
        require(focused() == surface, "Forwarded Alt shortcut activated a local menu");

        click(); point_at(editor); SetFocus(editor); pump(180);
        require(focused() == editor, "Recovery stole deliberate toolbar focus");
        key(); require(local_keys == 2, "Toolbar keyboard stopped working");

        other = create(L"PixelsFocusSmokeOther", DefWindowProcW, nullptr, 150);
        click(); ShowWindow(other, SW_SHOW); SetForegroundWindow(other); pump(180);
        require(GetForegroundWindow() == other, "Recovery stole another application's activation");
        SetForegroundWindow(host); pump(50); click(); pump(180);
        require(focused() == surface, "Click after window switch did not recover keyboard");
        key(); require(local_keys == 2, "Keyboard failed after window switch");

        click(); SendMessageW(surface, mode_message, 0, 0); SetFocus(editor); pump(180);
        require(focused() == editor, "View-only mode left a pending focus recovery");
        SendMessageW(surface, mode_message, 1, 0);
        SetWindowPos(host, nullptr, 100, 100, 700, 500, SWP_NOZORDER); SetFocus(editor);
        click(); pump(180); require(focused() == surface, "Click after resize did not recover keyboard"); key();

        DWORD available = 0; require(PeekNamedPipe(read,nullptr,0,nullptr,&available,nullptr) != FALSE, "Cannot inspect events");
        std::vector<uint8_t> bytes(available); DWORD got = 0;
        require(available && available % 8 == 0 && ReadFile(read,bytes.data(),available,&got,nullptr) && got == available, "Invalid native event framing");
        unsigned keys = 0, releases = 0, diagnostics = 0;
        for (size_t i=0; i<bytes.size(); i+=8) {
            if (bytes[i] == 4) ++keys;
            if (bytes[i] == 5) ++releases;
            if (bytes[i] == 9) ++diagnostics;
        }
        require(keys == 6, "Physical keyboard did not reach native collection exactly once");
        require(releases >= 3 && diagnostics >= 3, "Focus loss did not release keys/report diagnostics");
        std::cout << "{\"passed\":true,\"cross_process\":true,\"native_key_events\":" << keys
            << ",\"toolbar_key_events\":" << local_keys << ",\"focus_diagnostics\":" << diagnostics << "}\n";
        result = 0;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; }
    if (host) DestroyWindow(host); if (other) DestroyWindow(other);
    if (process.hProcess) { WaitForSingleObject(process.hProcess, 3000); CloseHandle(process.hProcess); CloseHandle(process.hThread); }
    if (read) CloseHandle(read); if (write) CloseHandle(write);
    if (previous) SetForegroundWindow(previous); SetCursorPos(pointer.x,pointer.y);
    return result;
}
