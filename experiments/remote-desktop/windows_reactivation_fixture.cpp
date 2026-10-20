// Opt-in, bounded diagnostic: warm an owned window, activate an owned peer,
// then exercise the original window again. peer-first- instead leaves the target
// untouched and warms only its peer. cross-thread- gives the peer a separate UI
// thread/input queue. Never activates an unrelated app
// during the test, changes system settings, or filters product input.
#define main control_fixture_main
#include "windows_control_fixture.cpp"
#undef main
#include <vector>

namespace {
constexpr UINT switch_peer = WM_APP + 42;
HWND peer_window = nullptr;
std::atomic<bool> switched{false}, switch_failed{false};
bool measuring = false;
std::vector<UINT> measured_messages;
std::atomic<bool> peer_ready{false}, peer_stop{false};
std::atomic<unsigned> peer_down{0}, peer_up{0};
bool peer_first=false;

LRESULT CALLBACK peer_proc(HWND window, UINT message, WPARAM w, LPARAM l) {
    if (message==WM_LBUTTONDOWN) ++peer_down;
    if (message==WM_LBUTTONUP) ++peer_up;
    if (message == WM_KEYDOWN || message == WM_SYSKEYDOWN
            || ((message == WM_LBUTTONDOWN || message == WM_RBUTTONDOWN)
                && static_cast<ULONG_PTR>(GetMessageExtraInfo()) != marker)) interrupted = true;
    return DefWindowProcW(window,message,w,l);
}

LRESULT CALLBACK measured_control(HWND window, UINT message, WPARAM w, LPARAM l) {
    if (measuring && (message == WM_LBUTTONDOWN || message == WM_LBUTTONUP || message == WM_LBUTTONDBLCLK))
        measured_messages.push_back(message);
    return control_proc(window, message, w, l);
}

LRESULT CALLBACK measured_parent(HWND window, UINT message, WPARAM w, LPARAM l) {
    if (message == switch_peer && window == parent_window) {
        const bool warmed = !interrupted && !counts.dragging && !(GetAsyncKeyState(VK_LBUTTON) & 0x8000)
            && (peer_first ? GetForegroundWindow()==peer_window && peer_up==1 && counts.up==0
                           : GetForegroundWindow()==parent_window && counts.up==1);
        std::cout << "{\"phase\":\"warmup\",\"warmup_ready\":" << (warmed ? "true" : "false")
                  << ",\"down\":" << counts.down << ",\"double_click\":" << counts.double_click
                  << ",\"up\":" << counts.up << ",\"peer_down\":" << peer_down
                  << ",\"peer_up\":" << peer_up << "}\n" << std::flush;
        if (!warmed) { switch_failed = true; return 0; }
        if (!peer_first) SetForegroundWindow(peer_window);
        DWORD_PTR ignored=0;
        if (!SendMessageTimeoutW(peer_window,WM_NULL,0,0,SMTO_ABORTIFHUNG,1000,&ignored)) {
            switch_failed=true; return 0;
        }
        if (GetForegroundWindow() != peer_window) { switch_failed = true; return 0; }
        counts = {};
        measuring = true;
        std::cout << "{\"phase\":\"measurement\",\"peer_foreground\":true}\n" << std::flush;
        switched = true;
        return 0;
    }
    return parent_proc(window, message, w, l);
}
}

int main(int argc, char** argv) {
    const std::string requested = argc == 2 ? argv[1] : "";
    const bool cross_thread=requested.find("cross-thread-")==0;
    const std::string suffix=requested.substr(cross_thread ? 13 : 0);
    peer_first=suffix.find("peer-first-")==0;
    const std::string mode = suffix.substr(peer_first ? 11 : 0);
    if (mode != "button-self" && mode != "doubleclick-self" && mode != "drag-self") return 2;
    const UINT double_time = GetDoubleClickTime();
    if (double_time > 1000) { std::cerr << "Double-click interval exceeds bounded diagnostic\n"; return 3; }
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    if (!on_own_desktop()) return 3;
    for (int key : {VK_LBUTTON, VK_RBUTTON, VK_MBUTTON, VK_CONTROL, VK_SHIFT, VK_MENU})
        if (GetAsyncKeyState(key) & 0x8000) return 3;
    const HWND previous_foreground = GetForegroundWindow();
    POINT previous_cursor{}; GetCursorPos(&previous_cursor);
    WNDCLASSW cls{};
    cls.hInstance = GetModuleHandleW(nullptr);
    cls.lpfnWndProc = measured_parent;
    cls.lpszClassName = L"PixelsReactivationParent";
    cls.hbrBackground = reinterpret_cast<HBRUSH>(COLOR_WINDOW + 1);
    if (!RegisterClassW(&cls)) return 4;
    cls.lpfnWndProc=peer_proc;
    cls.lpszClassName=L"PixelsReactivationPeer";
    cls.style=0;
    if (!RegisterClassW(&cls)) return 4;
    cls.lpfnWndProc = measured_control;
    cls.lpszClassName = L"PixelsReactivationSurface";
    cls.style = CS_DBLCLKS;
    if (!RegisterClassW(&cls)) return 4;
    parent_window = CreateWindowExW(WS_EX_TOPMOST, L"PixelsReactivationParent",
        L"Pixels owned reactivation diagnostic (8 seconds)", WS_OVERLAPPEDWINDOW,
        GetSystemMetrics(SM_CXSCREEN)/2-300, GetSystemMetrics(SM_CYSCREEN)/2-160,
        600, 320, nullptr, nullptr, cls.hInstance, nullptr);
    if (!parent_window) return 5;
    surface = mode != "button-self";
    target_window = CreateWindowExW(0, surface ? L"PixelsReactivationSurface" : L"BUTTON",
        L"Owned click target", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
        60, 50, 460, 160, parent_window, HMENU(1), cls.hInstance, nullptr);
    if (!target_window) return 6;
    if (!surface) original_control = reinterpret_cast<WNDPROC>(SetWindowLongPtrW(target_window,
        GWLP_WNDPROC, reinterpret_cast<LONG_PTR>(measured_control)));
    auto create_peer=[] {
        peer_window=CreateWindowExW(WS_EX_TOPMOST,L"PixelsReactivationPeer",
            L"Pixels owned focus peer",WS_OVERLAPPEDWINDOW,10,10,200,100,
            nullptr,nullptr,GetModuleHandleW(nullptr),nullptr);
        if (peer_window) ShowWindow(peer_window,SW_SHOWNOACTIVATE);
        peer_ready=true;
    };
    std::thread peer_thread;
    if (cross_thread) {
        peer_thread=std::thread([create_peer] {
            create_peer();
            const auto until=Clock::now()+std::chrono::seconds(10);
            while (!peer_stop && Clock::now()<until) {
                MSG message{};
                while (PeekMessageW(&message,nullptr,0,0,PM_REMOVE)) {
                    TranslateMessage(&message);DispatchMessageW(&message);
                }
                MsgWaitForMultipleObjects(0,nullptr,FALSE,10,QS_ALLINPUT);
            }
            if (peer_window) DestroyWindow(peer_window);
        });
        while (!peer_ready) Sleep(10);
    } else create_peer();
    if (!peer_window) { if(peer_thread.joinable()){peer_stop=true;peer_thread.join();} return 5; }
    ShowWindow(parent_window, SW_SHOWNOACTIVATE);
    std::thread input([mode, double_time] {
        Sleep(500);
        for (int key : {VK_LBUTTON,VK_RBUTTON,VK_MBUTTON,VK_CONTROL,VK_SHIFT,VK_MENU})
            if (GetAsyncKeyState(key)&0x8000) return;
        POINT point{150,80}; ClientToScreen(target_window, &point);
        if (peer_first) {
            POINT peer_point{50,30};ClientToScreen(peer_window,&peer_point);
            if (interrupted || WindowFromPoint(peer_point)!=peer_window || !on_own_desktop()) return;
            const int x=GetSystemMetrics(SM_XVIRTUALSCREEN),y=GetSystemMetrics(SM_YVIRTUALSCREEN);
            const int width=GetSystemMetrics(SM_CXVIRTUALSCREEN),height=GetSystemMetrics(SM_CYVIRTUALSCREEN);
            if (width<=0 || height<=0) return;
            INPUT event{};event.type=INPUT_MOUSE;
            event.mi.dx=LONG((int64_t(peer_point.x-x)*65536+32768)/width);
            event.mi.dy=LONG((int64_t(peer_point.y-y)*65536+32768)/height);
            event.mi.dwFlags=MOUSEEVENTF_MOVE|MOUSEEVENTF_ABSOLUTE|MOUSEEVENTF_VIRTUALDESK|MOUSEEVENTF_LEFTDOWN;
            event.mi.dwExtraInfo=marker;
            if (SendInput(1,&event,sizeof(event))!=1) return;
        } else if (!send_pointer(point, MOUSEEVENTF_LEFTDOWN)) return;
        Sleep(50);
        if (!release_button()) return;
        Sleep(100);
        if (!PostMessageW(parent_window, switch_peer, 0, 0)) return;
        const auto until = Clock::now() + std::chrono::seconds(1);
        while (!switched && !switch_failed && !interrupted && Clock::now() < until) Sleep(10);
        if (!switched || interrupted) return;
        Sleep(double_time + 100);
        if (GetForegroundWindow() != peer_window) return;
        active_at_input = GetForegroundWindow() == parent_window;
        bool ok = send_pointer(point, MOUSEEVENTF_LEFTDOWN);
        const bool pressed = ok;
        Sleep(50);
        if (mode == "drag-self" && ok) {
            for (unsigned i=0; i<5 && ok; ++i) { point.x+=20; ok=send_pointer(point,0); Sleep(30); }
        }
        if (pressed) ok=release_button() && ok;
        if (mode == "doubleclick-self" && ok) {
            Sleep(50);
            const bool second=send_pointer(point,MOUSEEVENTF_LEFTDOWN);
            ok=second; Sleep(50);
            if (second) ok=release_button() && ok;
        }
        injected=ok;
    });
    const auto deadline=Clock::now()+std::chrono::seconds(8);
    while (Clock::now()<deadline && !interrupted) {
        MSG message{};
        while (PeekMessageW(&message,nullptr,0,0,PM_REMOVE)) {
            TranslateMessage(&message); DispatchMessageW(&message);
        }
        MsgWaitForMultipleObjects(0,nullptr,FALSE,10,QS_ALLINPUT);
    }
    input.join();
    const std::vector<UINT> expected = mode == "doubleclick-self"
        ? std::vector<UINT>{WM_LBUTTONDOWN,WM_LBUTTONUP,WM_LBUTTONDBLCLK,WM_LBUTTONUP}
        : std::vector<UINT>{WM_LBUTTONDOWN,WM_LBUTTONUP};
    const bool held=(GetAsyncKeyState(VK_LBUTTON)&0x8000)!=0;
    const bool action_ok=mode=="button-self" ? counts.actions==1
        : mode=="doubleclick-self" ? counts.drag_starts==2 && counts.drag_ends==2
        : counts.drag_starts==1 && counts.drag_ends==1 && counts.held_moves>0
            && counts.last_up.x==250 && counts.last_up.y==80;
    const bool passed=switched && !interrupted && injected && !held && !counts.dragging
        && !active_at_input && measured_messages==expected && action_ok;
    std::cout << "{\"mode\":\"" << requested << "\",\"cross_thread_peer\":" << (cross_thread?"true":"false")
              << ",\"peer_first\":" << (peer_first?"true":"false")
              << ",\"setup_ready\":" << (switched?"true":"false")
              << ",\"target_previously_clicked\":" << (peer_first?"false":"true")
              << ",\"injected\":" << (injected?"true":"false") << ",\"messages\":[";
    for (size_t i=0;i<measured_messages.size();++i) std::cout << (i?",":"") << measured_messages[i];
    std::cout << "],\"actions\":" << counts.actions << ",\"drag_starts\":" << counts.drag_starts
              << ",\"drag_ends\":" << counts.drag_ends << ",\"left_held\":" << (held?"true":"false")
              << ",\"interrupted_by_external_input\":" << (interrupted?"true":"false")
              << ",\"passed\":" << (passed?"true":"false") << "}\n";
    POINT now{}; GetCursorPos(&now);
    if (!interrupted && GetForegroundWindow()==parent_window && WindowFromPoint(now)==target_window) {
        SetCursorPos(previous_cursor.x,previous_cursor.y);
        if (IsWindow(previous_foreground)) SetForegroundWindow(previous_foreground);
    }
    DestroyWindow(parent_window);
    if (peer_thread.joinable()) { peer_stop=true;peer_thread.join(); }
    else DestroyWindow(peer_window);
    return passed ? 0 : 1;
}
