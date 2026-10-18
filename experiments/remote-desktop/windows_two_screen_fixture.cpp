// Opt-in generated two-screen surface. Logs only aggregate clicks and Ctrl
// events in its own windows; no hooks, text capture or input injection.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <windowsx.h>
#include <algorithm>
#include <chrono>
#include <iostream>
#include <string>
#include <vector>

struct Screen {
    MONITORINFOEXW monitor{};
    HWND window = nullptr;
    unsigned index = 0, clicks = 0, ups = 0, ctrl_down = 0, ctrl_up = 0;
    POINT last{-1, -1};
};
LRESULT CALLBACK procedure(HWND h, UINT m, WPARAM w, LPARAM l) {
    auto* s = reinterpret_cast<Screen*>(GetWindowLongPtrW(h, GWLP_USERDATA));
    if (m == WM_NCCREATE) {
        s = static_cast<Screen*>(reinterpret_cast<CREATESTRUCTW*>(l)->lpCreateParams);
        SetWindowLongPtrW(h, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(s));
    }
    if (s) {
        if (m == WM_PAINT) {
            PAINTSTRUCT paint{}; HDC dc = BeginPaint(h, &paint);
            RECT rect{}; GetClientRect(h, &rect);
            HBRUSH brush = CreateSolidBrush(s->index == 0 ? RGB(28, 112, 68) : RGB(36, 76, 164));
            FillRect(dc, &rect, brush); DeleteObject(brush);
            SetBkMode(dc, TRANSPARENT); SetTextColor(dc, RGB(255,255,255));
            const std::wstring label = L"Pixels generated screen " + std::to_wstring(s->index + 1);
            TextOutW(dc, 50, 50, label.c_str(), int(label.size()));
            HPEN pen = CreatePen(PS_SOLID, 2, RGB(255,255,255)); auto old = SelectObject(dc, pen);
            MoveToEx(dc, rect.right/2-30, rect.bottom/2, nullptr); LineTo(dc, rect.right/2+30, rect.bottom/2);
            MoveToEx(dc, rect.right/2, rect.bottom/2-30, nullptr); LineTo(dc, rect.right/2, rect.bottom/2+30);
            SelectObject(dc, old); DeleteObject(pen); EndPaint(h, &paint); return 0;
        }
        if (m == WM_LBUTTONDOWN) { ++s->clicks; s->last={GET_X_LPARAM(l),GET_Y_LPARAM(l)}; ClientToScreen(h,&s->last); }
        if (m == WM_LBUTTONUP) ++s->ups;
        if (m == WM_KEYDOWN && w == VK_CONTROL) ++s->ctrl_down;
        if (m == WM_KEYUP && w == VK_CONTROL) ++s->ctrl_up;
    }
    return DefWindowProcW(h,m,w,l);
}
int main(int argc, char** argv) {
    if (argc != 3 || std::string(argv[1]) != "--two-screens") return 2;
    const std::string value(argv[2]);
    if (value.empty() || value.size()>3 || value.find_first_not_of("0123456789")!=std::string::npos) return 2;
    const unsigned seconds=std::stoul(value); if (seconds<5 || seconds>180) return 2;
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    std::vector<Screen> screens;
    EnumDisplayMonitors(nullptr,nullptr,+[](HMONITOR monitor,HDC,LPRECT,LPARAM data)->BOOL {
        Screen s; s.monitor.cbSize=sizeof(s.monitor);
        if (!GetMonitorInfoW(monitor,&s.monitor)) return FALSE;
        reinterpret_cast<std::vector<Screen>*>(data)->push_back(s); return TRUE;
    },reinterpret_cast<LPARAM>(&screens));
    if (screens.size()!=2) return 3;
    std::sort(screens.begin(),screens.end(),[](const Screen& a,const Screen& b){return a.monitor.rcMonitor.left<b.monitor.rcMonitor.left;});
    WNDCLASSW cls{}; cls.hInstance=GetModuleHandleW(nullptr); cls.lpfnWndProc=procedure;
    cls.lpszClassName=L"PixelsTwoScreenAcceptance";
    if (!RegisterClassW(&cls)) return 4;
    for (unsigned i=0;i<screens.size();++i) {
        auto& s=screens[i];s.index=i;const auto& r=s.monitor.rcMonitor;
        s.window=CreateWindowExW(WS_EX_TOPMOST,cls.lpszClassName,L"Pixels generated two-screen acceptance",
            WS_POPUP,r.left,r.top,r.right-r.left,r.bottom-r.top,nullptr,nullptr,cls.hInstance,&s);
        if (!s.window) { for(auto& other:screens) if(other.window) DestroyWindow(other.window); return 5; }
        ShowWindow(s.window,SW_SHOWNORMAL);
    }
    const auto until=std::chrono::steady_clock::now()+std::chrono::seconds(seconds);
    auto next=std::chrono::steady_clock::now();
    while(std::chrono::steady_clock::now()<until) {
        MSG msg{};while(PeekMessageW(&msg,nullptr,0,0,PM_REMOVE)){TranslateMessage(&msg);DispatchMessageW(&msg);}
        for(auto& s:screens) if(s.window) {
            DEVMODEW mode{};mode.dmSize=sizeof(mode);
            if(!EnumDisplaySettingsExW(s.monitor.szDevice,ENUM_CURRENT_SETTINGS,&mode,0)) {
                DestroyWindow(s.window);s.window=nullptr;
            }
        }
        if(std::chrono::steady_clock::now()>=next) {
            std::cout<<"{\"ctrl_held\":"<<((GetAsyncKeyState(VK_CONTROL)&0x8000)?"true":"false")
                <<",\"left_held\":"<<((GetAsyncKeyState(VK_LBUTTON)&0x8000)?"true":"false")<<",\"screens\":[";
            for(unsigned i=0;i<screens.size();++i){const auto& s=screens[i];const auto& r=s.monitor.rcMonitor;
                std::cout<<(i?",":"")<<"{\"index\":"<<i<<",\"visible\":"<<(s.window&&IsWindowVisible(s.window)?"true":"false")
                    <<",\"x\":"<<r.left<<",\"y\":"<<r.top<<",\"width\":"<<r.right-r.left<<",\"height\":"<<r.bottom-r.top
                    <<",\"clicks\":"<<s.clicks<<",\"ups\":"<<s.ups<<",\"last_x\":"<<s.last.x<<",\"last_y\":"<<s.last.y
                    <<",\"ctrl_down\":"<<s.ctrl_down<<",\"ctrl_up\":"<<s.ctrl_up<<"}";}
            std::cout<<"]}\n"<<std::flush;next+=std::chrono::milliseconds(250);
        }
        Sleep(10);
    }
    for(auto& s:screens) if(s.window&&IsWindow(s.window)) DestroyWindow(s.window);
    return 0;
}
