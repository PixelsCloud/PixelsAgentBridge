// Own-window diagnostic only. Observe message retrieval, dispatch and callback
// stacks without installing a hook, modifying another process or filtering input.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <iostream>
#include <string>
#include <vector>

ATOM WINAPI trace_register(const WNDCLASSW*);
LONG_PTR WINAPI trace_subclass(HWND, int, LONG_PTR);
BOOL WINAPI trace_peek(LPMSG, HWND, UINT, UINT, UINT);
LRESULT WINAPI trace_dispatch(const MSG*);
UINT WINAPI trace_send(UINT, LPINPUT, int);
BOOL WINAPI trace_show(HWND, int);
#define main input_trace_baseline_main
#define SendInput trace_send
#define RegisterClassW trace_register
#define SetWindowLongPtrW trace_subclass
#define PeekMessageW trace_peek
#define DispatchMessageW trace_dispatch
#define ShowWindow trace_show
#include "windows_control_fixture.cpp"
#undef main
#undef SendInput
#undef RegisterClassW
#undef PeekMessageW
#undef DispatchMessageW
#undef SetWindowLongPtrW
#undef ShowWindow

namespace {
thread_local bool retrieving = false;
thread_local UINT dispatching = 0;
unsigned trace_sequence = 0;
bool external_sender = false;
bool initialize_active = false, reset_active = false;
bool pointer_button(UINT message) {
    return message == WM_LBUTTONDOWN || message == WM_LBUTTONUP || message == WM_LBUTTONDBLCLK;
}
void trace(const char* phase, UINT message, bool stack) {
    INPUT_MESSAGE_SOURCE source{};
    const BOOL source_ok = GetCurrentInputMessageSource(&source);
    const DWORD sent = InSendMessageEx(nullptr);
    void* frames[24]{};
    const unsigned count = stack ? CaptureStackBackTrace(0, 24, frames, nullptr) : 0;
    std::cout << "{\"trace\":\"" << phase << "\",\"sequence\":" << ++trace_sequence
        << ",\"message\":" << message << ",\"time\":" << GetMessageTime()
        << ",\"extra\":" << GetMessageExtraInfo()
        << ",\"retrieving\":" << (retrieving ? "true" : "false")
        << ",\"dispatching\":" << dispatching << ",\"sent_flags\":" << sent
        << ",\"source_ok\":" << (source_ok ? "true" : "false")
        << ",\"device_type\":" << source.deviceType << ",\"origin\":" << source.originId
        << ",\"stack\":[";
    for (unsigned i = 0; i < count; ++i) {
        HMODULE module = nullptr;
        const BOOL found = GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
            | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, reinterpret_cast<LPCWSTR>(frames[i]), &module);
        wchar_t path[1024]{};
        std::string basename = "unknown";
        if (found && GetModuleFileNameW(module, path, 1024)) {
            std::wstring name(path);
            name = name.substr(name.find_last_of(L"\\/") + 1);
            basename.clear();
            for (wchar_t c : name) basename += (c >= 32 && c < 127 && c != '"' && c != '\\') ? char(c) : '_';
        }
        std::cout << (i ? "," : "") << "{\"module\":\"" << basename << "\",\"offset\":"
            << (found ? reinterpret_cast<ULONG_PTR>(frames[i]) - reinterpret_cast<ULONG_PTR>(module) : 0) << "}";
    }
    std::cout << "]}\n" << std::flush;
}
LRESULT CALLBACK traced_control(HWND window, UINT message, WPARAM w, LPARAM l) {
    if (pointer_button(message)) trace("callback", message, true);
    return control_proc(window, message, w, l);
}
}

ATOM WINAPI trace_register(const WNDCLASSW* source) {
    auto cls = *source;
    if (cls.lpfnWndProc == control_proc) cls.lpfnWndProc = traced_control;
    return RegisterClassW(&cls);
}
LONG_PTR WINAPI trace_subclass(HWND window, int index, LONG_PTR value) {
    if (index == GWLP_WNDPROC && value == reinterpret_cast<LONG_PTR>(control_proc))
        value = reinterpret_cast<LONG_PTR>(traced_control);
    return SetWindowLongPtrW(window, index, value);
}
BOOL WINAPI trace_peek(LPMSG message, HWND window, UINT first, UINT last, UINT flags) {
    retrieving = true;
    const BOOL result = PeekMessageW(message, window, first, last, flags);
    retrieving = false;
    if (result && pointer_button(message->message)) trace("removed", message->message, false);
    return result;
}
LRESULT WINAPI trace_dispatch(const MSG* message) {
    const UINT previous = dispatching;
    dispatching = message->message;
    const LRESULT result = DispatchMessageW(message);
    dispatching = previous;
    return result;
}

BOOL WINAPI trace_show(HWND window, int command) {
    const BOOL result = ShowWindow(window, command);
    if (initialize_active && window == parent_window) {
        const HWND previous = GetForegroundWindow();
        SetActiveWindow(window);
        if (reset_active) SetActiveWindow(nullptr);
        std::cout << "{\"active_setup\":true,\"reset\":" << (reset_active ? "true" : "false")
            << ",\"thread_active_own\":" << (GetActiveWindow() == window ? "true" : "false")
            << ",\"foreground_unchanged\":" << (GetForegroundWindow() == previous ? "true" : "false")
            << ",\"foreground_own\":" << (GetForegroundWindow() == window ? "true" : "false") << "}\n";
    }
    return result;
}

UINT WINAPI trace_send(UINT count, LPINPUT inputs, int size) {
    if (!external_sender) return SendInput(count, inputs, size);
    if (!IsWindow(target_window) || count == 0 || count > 2 || size != sizeof(INPUT)) return 0;
    wchar_t path[32768]{};
    if (!GetModuleFileNameW(nullptr, path, 32768)) return 0;
    std::wstring args = L"\"" + std::wstring(path) + L"\" --owned-send "
        + std::to_wstring(reinterpret_cast<ULONG_PTR>(target_window)) + L" "
        + std::to_wstring(GetCurrentProcessId()) + L" " + std::to_wstring(count);
    for (UINT i = 0; i < count; ++i) {
        if (inputs[i].type != INPUT_MOUSE) return 0;
        args += L" " + std::to_wstring(inputs[i].mi.dx) + L" " + std::to_wstring(inputs[i].mi.dy)
            + L" " + std::to_wstring(inputs[i].mi.dwFlags);
    }
    STARTUPINFOW startup{sizeof(startup)};
    PROCESS_INFORMATION child{};
    if (!CreateProcessW(path, args.data(), nullptr, nullptr, FALSE, CREATE_NO_WINDOW,
            nullptr, nullptr, &startup, &child)) return 0;
    const DWORD wait = WaitForSingleObject(child.hProcess, 2000);
    DWORD code = 1;
    if (wait == WAIT_OBJECT_0) GetExitCodeProcess(child.hProcess, &code);
    else { TerminateProcess(child.hProcess, 9); WaitForSingleObject(child.hProcess, 1000); }
    CloseHandle(child.hThread); CloseHandle(child.hProcess);
    return code == 0 ? count : 0;
}

int main(int argc, char** argv) {
    if (argc >= 5 && std::string(argv[1]) == "--owned-send") {
        // The child accepts only one of its own parent fixture's windows and
        // mouse flags used by this diagnostic. No general command interpreter.
        const HWND window = reinterpret_cast<HWND>(std::stoull(argv[2]));
        const DWORD parent = std::stoul(argv[3]);
        const unsigned count = std::stoul(argv[4]);
        DWORD actual = 0; GetWindowThreadProcessId(window, &actual);
        if (actual != parent || count == 0 || count > 2 || argc != 5 + int(count) * 3) return 3;
        HANDLE process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, parent);
        wchar_t parent_path[32768]{}, own_path[32768]{}; DWORD length = 32768;
        const bool owned = process && QueryFullProcessImageNameW(process, 0, parent_path, &length)
            && GetModuleFileNameW(nullptr, own_path, 32768) && std::wstring(parent_path) == own_path;
        if (process) CloseHandle(process);
        if (!owned || !on_own_desktop()) return 3;
        INPUT inputs[2]{};
        for (unsigned i = 0; i < count; ++i) {
            inputs[i].type = INPUT_MOUSE;
            inputs[i].mi.dx = std::stol(argv[5 + i * 3]);
            inputs[i].mi.dy = std::stol(argv[6 + i * 3]);
            inputs[i].mi.dwFlags = std::stoul(argv[7 + i * 3]);
            inputs[i].mi.dwExtraInfo = marker;
            const DWORD allowed = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK
                | MOUSEEVENTF_LEFTDOWN | MOUSEEVENTF_LEFTUP;
            if (inputs[i].mi.dwFlags & ~allowed) return 3;
        }
        return SendInput(count, inputs, sizeof(INPUT)) == count ? 0 : 4;
    }
    std::string mode = argc == 2 ? argv[1] : "";
    if (mode.find("external-") == 0) {
        external_sender = true;
        mode = mode.substr(9);
    }
    if (mode.find("active-only-") == 0 || mode.find("active-reset-") == 0) {
        initialize_active = true;
        reset_active = mode.find("active-reset-") == 0;
        mode = mode.substr(reset_active ? 13 : 12);
    }
    if (argc == 2) argv[1] = mode.data();
    return input_trace_baseline_main(argc, argv);
}
